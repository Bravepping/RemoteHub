//! Standalone local diagnostic. Only fixed fake events reach the real queue;
//! this executable does not attach an OS input sink or start a connection.
use hub_webrtc::{
    input_approval::{End, Error as ApprovalError, InputScope, SessionSink, SinkError, State},
    input_queue::{Control, Dispatch, Error, InputQueue},
};
use std::{
    cell::RefCell,
    io::{self, BufRead, IsTerminal, Write},
    process::ExitCode,
    rc::Rc,
    time::{Duration, Instant},
};

const SCOPE: InputScope = InputScope {
    room: [1; 16],
    generation: [2; 32],
    viewer_key: [3; 32],
};
const DENIED: Error = Error::Approval(ApprovalError::NotAllowed);

enum FakeEvent {
    SessionPress,
    QueuedRelease,
    QueuedPress,
}

struct FakeState {
    local_held: bool,
    session_held: bool,
    applied: u8,
    released: u8,
}

struct FakeSink(Rc<RefCell<FakeState>>);

impl SessionSink for FakeSink {
    type Event = FakeEvent;

    fn apply(&mut self, event: &FakeEvent) -> Result<(), SinkError> {
        let mut state = self.0.borrow_mut();
        state.applied += 1;
        state.session_held = !matches!(event, FakeEvent::QueuedRelease);
        Ok(())
    }

    fn release_owned(&mut self) -> Result<(), SinkError> {
        let mut state = self.0.borrow_mut();
        if state.session_held {
            state.released += 1;
            state.session_held = false;
        }
        Ok(())
    }
}

fn line(out: &mut impl Write, text: &str) -> Result<(), &'static str> {
    writeln!(out, "{text}").map_err(|_| "OUTPUT_UNAVAILABLE")
}

fn enter(
    out: &mut impl Write,
    input: &mut impl BufRead,
    prompt: &str,
    interactive: bool,
) -> Result<(), &'static str> {
    if !interactive {
        return Ok(());
    }
    line(out, prompt)?;
    out.flush().map_err(|_| "OUTPUT_UNAVAILABLE")?;
    let mut line = String::new();
    if input
        .read_line(&mut line)
        .map_err(|_| "CONSOLE_READ_FAILED")?
        == 0
        || !line.trim().is_empty()
    {
        return Err("CANCELLED");
    }
    Ok(())
}

fn run(
    out: &mut impl Write,
    input: &mut impl BufRead,
    interactive: bool,
) -> Result<(), &'static str> {
    line(out, "RemoteHub local input diagnostic (simulation only)")?;
    line(out, "No network. No real keyboard or mouse control.")?;
    let state = Rc::new(RefCell::new(FakeState {
        local_held: true,
        session_held: false,
        applied: 0,
        released: 0,
    }));
    let mut queue = InputQueue::view_only(
        SCOPE,
        Instant::now() + Duration::from_secs(600),
        FakeSink(state.clone()),
    );
    if queue.enqueue(&SCOPE, FakeEvent::SessionPress) != Err(DENIED) {
        return Err("DENIAL_CHECK_FAILED");
    }
    line(out, "[1] View only: simulated input denied.")?;
    enter(
        out,
        input,
        "Press Enter to approve simulated input.",
        interactive,
    )?;
    queue.decide_locally(true).map_err(|_| "APPROVAL_FAILED")?;
    queue
        .enqueue(&SCOPE, FakeEvent::SessionPress)
        .map_err(|_| "SIMULATED_PRESS_NOT_QUEUED")?;
    if queue.dispatch_next(&SCOPE, Control::Continue) != Ok(Dispatch::Applied)
        || !state.borrow().session_held
    {
        return Err("SIMULATED_PRESS_NOT_APPLIED");
    }
    for event in [FakeEvent::QueuedRelease, FakeEvent::QueuedPress] {
        queue
            .enqueue(&SCOPE, event)
            .map_err(|_| "SIMULATED_QUEUE_FAILED")?;
    }
    line(
        out,
        "[2] Approved: 1 simulated press applied, 2 events waiting.",
    )?;
    enter(out, input, "Press Enter to revoke approval.", interactive)?;
    if queue.dispatch_next(&SCOPE, Control::Revoke) != Ok(Dispatch::Closed)
        || queue.state() != State::Ended(End::Revoked)
        || queue.dispatch_next(&SCOPE, Control::Continue) != Ok(Dispatch::Closed)
        || queue.enqueue(&SCOPE, FakeEvent::QueuedPress) != Err(DENIED)
        || queue.decide_locally(true) != Err(DENIED)
    {
        return Err("REVOCATION_CHECK_FAILED");
    }
    let state = state.borrow();
    if state.applied != 1 || state.released != 1 || state.session_held || !state.local_held {
        return Err("SIMULATED_RELEASE_CHECK_FAILED");
    }
    line(
        out,
        "[3] Revoked: 2 waiting events discarded, no late input.",
    )?;
    line(
        out,
        "[4] Session press released; simulated local key untouched.",
    )?;
    line(out, "INPUT_LOCAL_PASS")
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let interactive = match (args.next(), args.next()) {
        (None, None) => true,
        (Some(arg), None) if arg == "--self-test" => false,
        _ => {
            eprintln!("Usage: remotehub-input-local-check [--self-test]");
            return ExitCode::FAILURE;
        }
    };
    let stdin = io::stdin();
    let pause = interactive && stdin.is_terminal();
    let mut input = stdin.lock();
    let mut out = io::stdout().lock();
    let result = match run(&mut out, &mut input, interactive) {
        Ok(()) => ExitCode::SUCCESS,
        Err("CANCELLED") => {
            let _ = line(&mut out, "INPUT_LOCAL_CANCELLED");
            ExitCode::FAILURE
        }
        Err(code) => {
            let _ = writeln!(out, "INPUT_LOCAL_FAIL: {code}");
            ExitCode::FAILURE
        }
    };
    if pause {
        let _ = enter(&mut out, &mut input, "Press Enter to close.", true);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_two_steps_report_pass() {
        let mut out = Vec::new();
        assert_eq!(run(&mut out, &mut &b"\r\n\r\n"[..], true), Ok(()));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("2 waiting events discarded"));
        assert!(text.ends_with("INPUT_LOCAL_PASS\n"));
    }

    #[test]
    fn cancelled_approval_does_not_pass_or_apply_events() {
        let mut out = Vec::new();
        assert_eq!(run(&mut out, &mut &b"no\n"[..], true), Err("CANCELLED"));
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("Approved:"));
        assert!(!text.contains("INPUT_LOCAL_PASS"));
    }
}

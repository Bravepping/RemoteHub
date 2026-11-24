//! Explicit standalone Windows prompt diagnostic, with a fixed in-memory sink.
//! No socket, desktop capture, identity store or OS input path is used.

#[cfg(any(windows, test))]
mod simulation {
    use hub_webrtc::{
        input_approval::{End, Error as ApprovalError, InputScope, SessionSink, SinkError, State},
        input_queue::{Control, Dispatch, Error, InputQueue},
    };
    use std::{
        cell::RefCell,
        rc::Rc,
        time::{Duration, Instant},
    };

    const SCOPE: InputScope = InputScope {
        room: [1; 16],
        generation: [2; 32],
        viewer_key: [3; 32],
    };
    const DENIED: Error = Error::Approval(ApprovalError::NotAllowed);

    #[derive(Clone, Copy)]
    pub(super) enum Case {
        Timely,
        Late,
    }

    #[derive(Clone, Copy)]
    pub(super) enum Choice {
        Yes,
        No,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum Outcome {
        Declined,
        ApprovedAndRevoked,
        LateYesRejected,
        RetryLateCase,
    }

    enum FakeEvent {
        Press,
        Release,
    }

    struct FakeState {
        local_held: bool,
        session_held: bool,
        applied: u8,
        release_calls: u8,
    }

    struct FakeSink(Rc<RefCell<FakeState>>);

    impl SessionSink for FakeSink {
        type Event = FakeEvent;

        fn apply(&mut self, event: &FakeEvent) -> Result<(), SinkError> {
            let mut state = self.0.borrow_mut();
            state.applied += 1;
            state.session_held = matches!(event, FakeEvent::Press);
            Ok(())
        }

        fn release_owned(&mut self) -> Result<(), SinkError> {
            let mut state = self.0.borrow_mut();
            state.release_calls += 1;
            state.session_held = false;
            Ok(())
        }
    }

    #[cfg(windows)]
    pub(super) fn run(
        case: Case,
        ask: impl FnOnce() -> Result<Choice, &'static str>,
    ) -> Result<Outcome, &'static str> {
        let span = match case {
            Case::Timely => Duration::from_secs(120),
            Case::Late => Duration::from_secs(8),
        };
        run_with_window(case, span, ask)
    }

    fn run_with_window(
        case: Case,
        span: Duration,
        ask: impl FnOnce() -> Result<Choice, &'static str>,
    ) -> Result<Outcome, &'static str> {
        let until = Instant::now() + span;
        let fake = Rc::new(RefCell::new(FakeState {
            local_held: true,
            session_held: false,
            applied: 0,
            release_calls: 0,
        }));
        let mut owner = InputQueue::view_only(SCOPE, until, FakeSink(fake.clone()));
        if owner.enqueue(&SCOPE, FakeEvent::Press) != Err(DENIED) {
            return Err("VIEW_ONLY_NOT_ENFORCED");
        }
        let prompt = owner
            .begin_local_decision(&SCOPE)
            .map_err(|_| "BEGIN_DECISION_FAILED")?;
        if owner.state() != State::ViewOnly
            || owner.enqueue(&SCOPE, FakeEvent::Press) != Err(DENIED)
        {
            return Err("PENDING_INPUT_NOT_BLOCKED");
        }

        let choice = ask()?;
        let now_expired = Instant::now() >= until;
        let (control, expected) = match (case, choice, now_expired) {
            (Case::Timely, _, true) => return Err("TIMELY_TEST_EXPIRED"),
            (Case::Timely, Choice::Yes, false) => (Control::Continue, Outcome::ApprovedAndRevoked),
            (Case::Timely, Choice::No, false) => (Control::Continue, Outcome::Declined),
            (Case::Late, Choice::Yes, true) => (Control::Continue, Outcome::LateYesRejected),
            // This scenario is for a late Yes only. Neither an early Yes nor
            // a No may open the gate on a later retry in the same process.
            (Case::Late, _, _) => (Control::Revoke, Outcome::RetryLateCase),
        };
        let completed =
            owner.complete_local_decision(prompt, &SCOPE, matches!(choice, Choice::Yes), control);

        if expected == Outcome::ApprovedAndRevoked {
            if completed != Ok(()) || owner.state() != State::Allowed {
                return Err("APPROVAL_NOT_RECORDED");
            }
            owner
                .enqueue(&SCOPE, FakeEvent::Press)
                .map_err(|_| "SIMULATED_ENQUEUE_FAILED")?;
            if owner.dispatch_next(&SCOPE, Control::Continue) != Ok(Dispatch::Applied)
                || !fake.borrow().session_held
            {
                return Err("SIMULATED_APPLY_FAILED");
            }
            owner
                .enqueue(&SCOPE, FakeEvent::Release)
                .map_err(|_| "SIMULATED_BACKLOG_FAILED")?;
            owner
                .enqueue(&SCOPE, FakeEvent::Press)
                .map_err(|_| "SIMULATED_BACKLOG_FAILED")?;
            if owner.dispatch_next(&SCOPE, Control::Revoke) != Ok(Dispatch::Closed)
                || owner.state() != State::Ended(End::Revoked)
            {
                return Err("REVOKE_FAILED");
            }
        } else {
            let terminal = match expected {
                Outcome::Declined => (Ok(()), End::Declined),
                Outcome::LateYesRejected => (Err(DENIED), End::Expired),
                Outcome::RetryLateCase => (Err(DENIED), End::Revoked),
                Outcome::ApprovedAndRevoked => unreachable!(),
            };
            if completed != terminal.0 || owner.state() != State::Ended(terminal.1) {
                return Err("TERMINAL_DECISION_NOT_ENFORCED");
            }
        }

        if owner.dispatch_next(&SCOPE, Control::Continue) != Ok(Dispatch::Closed)
            || owner.enqueue(&SCOPE, FakeEvent::Press) != Err(DENIED)
            || owner.decide_locally(true) != Err(DENIED)
        {
            return Err("TERMINAL_INPUT_NOT_BLOCKED");
        }
        let fake = fake.borrow();
        let applied = u8::from(expected == Outcome::ApprovedAndRevoked);
        if fake.applied != applied
            || fake.release_calls != applied
            || fake.session_held
            || !fake.local_held
        {
            return Err("SIMULATED_CLEANUP_MISMATCH");
        }
        Ok(expected)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn native_no_contract_keeps_input_unapplied_and_closed() {
            assert_eq!(
                run_with_window(Case::Timely, Duration::from_secs(2), || Ok(Choice::No)),
                Ok(Outcome::Declined)
            );
        }

        #[test]
        fn native_yes_contract_applies_only_one_fake_event_then_revokes() {
            assert_eq!(
                run_with_window(Case::Timely, Duration::from_secs(2), || Ok(Choice::Yes)),
                Ok(Outcome::ApprovedAndRevoked)
            );
        }

        #[test]
        fn early_yes_in_late_only_case_never_opens_input() {
            assert_eq!(
                run_with_window(Case::Late, Duration::from_secs(2), || Ok(Choice::Yes)),
                Ok(Outcome::RetryLateCase)
            );
        }

        #[test]
        fn genuinely_late_yes_does_not_open_input() {
            let until = Duration::from_millis(200);
            assert_eq!(
                run_with_window(Case::Late, until, || {
                    std::thread::sleep(until + Duration::from_millis(10));
                    Ok(Choice::Yes)
                }),
                Ok(Outcome::LateYesRejected)
            );
        }

        #[test]
        fn prompt_failure_is_not_a_decision() {
            assert_eq!(
                run_with_window(Case::Timely, Duration::from_secs(2), || Err(
                    "DIALOG_FAILED"
                )),
                Err("DIALOG_FAILED")
            );
        }
    }
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use simulation::{Case, Choice, Outcome, run};
    use std::process::ExitCode;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDNO, IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK,
        MB_YESNO,
    };

    let mut args = std::env::args_os().skip(1);
    let case = match (args.next(), args.next()) {
        (None, None) => Case::Timely,
        (Some(arg), None) if arg == "--late" => Case::Late,
        _ => {
            eprintln!("Usage: remotehub-native-consent-check [--late]");
            return ExitCode::FAILURE;
        }
    };

    let result = run(case, || {
        let question = match case {
            Case::Timely => {
                "仅检查 Windows 原生同意流程。不会连接网络、读取屏幕或操作真实键鼠。\n\n是否允许应用一次模拟输入，并随后撤销？"
            }
            Case::Late => {
                "仅测试超时拒绝。不会连接网络、读取屏幕或操作真实键鼠。\n\n请在此窗口出现后等满 10 秒，再点击「是」。同意窗口在 8 秒时已过期。若提前点击，测试会要求重试，且不会授权。"
            }
        };
        match dialog(question, MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION) {
            IDYES => Ok(Choice::Yes),
            IDNO => Ok(Choice::No),
            _ => Err("DIALOG_FAILED"),
        }
    });

    let (label, note, icon, exit) = match result {
        Ok(Outcome::Declined) => (
            "NATIVE_CONSENT_DECLINED_PASS",
            "已拒绝；没有应用模拟输入，也无法重新授权。",
            MB_ICONINFORMATION,
            ExitCode::SUCCESS,
        ),
        Ok(Outcome::ApprovedAndRevoked) => (
            "NATIVE_CONSENT_REVOKE_PASS",
            "已应用 1 个模拟事件并撤销，待处理事件未应用；模拟会话按键已释放，模拟本地按键不受影响。",
            MB_ICONINFORMATION,
            ExitCode::SUCCESS,
        ),
        Ok(Outcome::LateYesRejected) => (
            "NATIVE_CONSENT_LATE_YES_PASS",
            "超时后点击「是」被拒绝；没有应用任何模拟输入。",
            MB_ICONINFORMATION,
            ExitCode::SUCCESS,
        ),
        Ok(Outcome::RetryLateCase) => (
            "NATIVE_CONSENT_LATE_RETRY",
            "没有获得授权。请重启 --late，等满 10 秒后再点击「是」。",
            MB_ICONINFORMATION,
            ExitCode::from(2),
        ),
        Err(code) => {
            eprintln!("NATIVE_CONSENT_FAIL: {code}");
            dialog(
                &format!("自检未通过。\n\nNATIVE_CONSENT_FAIL: {code}"),
                MB_OK | MB_ICONERROR,
            );
            return ExitCode::FAILURE;
        }
    };
    println!("{label}");
    dialog(&format!("{note}\n\n{label}"), MB_OK | icon);
    exit
}

#[cfg(windows)]
fn dialog(body: &str, flags: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
    let title: Vec<_> = "RemoteHub · 原生同意自检（仅模拟）"
        .encode_utf16()
        .chain([0])
        .collect();
    let body: Vec<_> = body.encode_utf16().chain([0]).collect();
    // Both wide strings are NUL-terminated and retained through the call;
    // this standalone process intentionally has no parent window.
    unsafe { MessageBoxW(std::ptr::null_mut(), body.as_ptr(), title.as_ptr(), flags) }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("This native prompt diagnostic only runs on Windows.");
    std::process::ExitCode::FAILURE
}

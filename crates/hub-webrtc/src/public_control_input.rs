//! Serialized device-scoped host input owner for the explicit public-control
//! experiment. It opens no connection, authenticates no peer, and has no
//! authority from a wire message. Only the authenticated native host may
//! supply the scope, signal local consent, and own the input sink.

use crate::{
    input_approval::{Error as ApprovalError, InputScope, PendingDecision, SessionSink, State},
    input_queue::{self, Control, Dispatch, InputQueue},
    remote_input::{Event, Protocol},
};
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidScope,
    InvalidWindow,
    NotAllowed,
    Protocol,
    Queue(input_queue::Error),
    InvalidState,
}

impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidScope => "PUBLIC_CONTROL_INPUT_SCOPE",
            Self::InvalidWindow => "PUBLIC_CONTROL_INPUT_WINDOW",
            Self::NotAllowed => "PUBLIC_CONTROL_INPUT_NOT_ALLOWED",
            Self::Protocol => "PUBLIC_CONTROL_INPUT_PROTOCOL",
            Self::Queue(input_queue::Error::Full) => "PUBLIC_CONTROL_INPUT_FULL",
            Self::Queue(input_queue::Error::Approval(ApprovalError::ReleaseUnconfirmed)) => {
                "PUBLIC_CONTROL_INPUT_RELEASE_UNCONFIRMED"
            }
            Self::Queue(input_queue::Error::Approval(ApprovalError::ApplyFailed)) => {
                "PUBLIC_CONTROL_INPUT_APPLY_FAILED"
            }
            Self::Queue(input_queue::Error::Approval(ApprovalError::WrongScope)) => {
                "PUBLIC_CONTROL_INPUT_SCOPE"
            }
            Self::Queue(input_queue::Error::Approval(ApprovalError::NotAllowed)) => {
                "PUBLIC_CONTROL_INPUT_NOT_ALLOWED"
            }
            Self::InvalidState => "PUBLIC_CONTROL_INPUT_STATE",
        }
    }
}

impl From<input_queue::Error> for Error {
    fn from(value: input_queue::Error) -> Self {
        Self::Queue(value)
    }
}

/// One outstanding v1 event. Only the exact successful dispatch commits its
/// sequence. Keep this owner on the same serialized native execution path:
/// sample Stop, EOF and the current verified scope at each call. While idle
/// drive `advance` on a monotonic timer and signal loss immediately. Do not
/// replace this owner until its explicitly requested teardown has completed.
pub struct PublicControlInput<S: SessionSink<Event = Event>> {
    scope: InputScope,
    queue: InputQueue<S>,
    protocol: Protocol,
    pending_seq: Option<u64>,
}

impl<S: SessionSink<Event = Event>> PublicControlInput<S> {
    /// `scope` must be established by the exact session and live device
    /// transport, never by the v1 envelope. `until` is not renewable.
    pub fn view_only(scope: InputScope, until: Instant, sink: S) -> Result<Self, Error> {
        let room = Uuid::from_bytes(scope.room);
        if room.is_nil() || scope.generation == [0; 32] || scope.viewer_key == [0; 32] {
            return Err(Error::InvalidScope);
        }
        let now = Instant::now();
        if until <= now || until.duration_since(now) > MAX_WINDOW {
            return Err(Error::InvalidWindow);
        }
        Ok(Self {
            scope,
            queue: InputQueue::view_only(scope, until, sink),
            protocol: Protocol::with_deadline(room.to_string(), now, until),
            pending_seq: None,
        })
    }

    pub fn state(&self) -> State {
        self.queue.state()
    }

    pub fn begin_local_decision(
        &mut self,
        current: &InputScope,
        control: Control,
    ) -> Result<PendingDecision, Error> {
        let state = self.advance(control)?;
        self.require_scope(current)?;
        if state != State::ViewOnly {
            return Err(Error::NotAllowed);
        }
        Ok(self.queue.begin_local_decision(current)?)
    }

    /// The result comes only from the original local native input dialog.
    /// A late affirmative result cannot override stop, expiry or new scope.
    pub fn complete_local_decision(
        &mut self,
        pending: PendingDecision,
        current: &InputScope,
        approved: bool,
        control: Control,
    ) -> Result<(), Error> {
        let state = self.advance(control)?;
        self.require_scope(current)?;
        if state != State::ViewOnly {
            return Err(Error::NotAllowed);
        }
        Ok(self
            .queue
            .complete_local_decision(pending, current, approved, control)?)
    }

    /// `ingress` is the scope captured at the registered transport callback,
    /// not relabeled as the currently displayed session. Early bytes are
    /// ignored before the v1 parser, so they neither queue nor spend sequence.
    /// The first public lane does not enable the LAN wheel event.
    pub fn enqueue_v1(
        &mut self,
        ingress: &InputScope,
        bytes: &[u8],
        text: bool,
        control: Control,
    ) -> Result<(), Error> {
        let state = self.advance(control)?;
        self.require_scope(ingress)?;
        if state != State::Allowed {
            return Err(Error::NotAllowed);
        }
        if self.pending_seq.take().is_some() {
            self.queue.close_for_overflow()?;
            return Err(input_queue::Error::Full.into());
        }
        let (seq, event) = match self
            .protocol
            .validate_event(bytes, text, true, Instant::now())
        {
            Ok((seq, event)) if !matches!(event, Event::Wheel { .. }) => (seq, event),
            _ => {
                // A truncated/replayed invalid input must not leave a
                // possibly incomplete held-key sequence with the grant open.
                self.advance(Control::Revoke)?;
                return Err(Error::Protocol);
            }
        };
        self.queue.enqueue(ingress, event)?;
        self.pending_seq = Some(seq);
        Ok(())
    }

    /// ACK only this returned sequence, and only on that same authenticated
    /// channel. Resample local stop/loss after enqueue, before dispatch.
    pub fn dispatch_next(
        &mut self,
        current: &InputScope,
        control: Control,
    ) -> Result<Option<u64>, Error> {
        let state = self.advance(control)?;
        self.require_scope(current)?;
        if state != State::Allowed {
            return Ok(None);
        }
        let pending = self.pending_seq.take();
        match self.queue.dispatch_next(current, control)? {
            Dispatch::Applied if pending.is_some() => {
                Ok(pending.map(|seq| self.protocol.commit_applied(seq)))
            }
            Dispatch::Idle if pending.is_none() => Ok(None),
            Dispatch::Closed => Ok(None),
            _ => {
                self.advance(Control::Revoke)?;
                Err(Error::InvalidState)
            }
        }
    }

    /// End immediately on user stop/EOF, or sample expiry while idle. Cleanup
    /// uncertainty is sticky and outranks later protocol/disconnect errors.
    pub fn advance(&mut self, control: Control) -> Result<State, Error> {
        let result = self.queue.advance(control);
        if !matches!(result, Ok(State::Allowed)) {
            self.pending_seq = None;
        }
        Ok(result?)
    }

    fn require_scope(&mut self, supplied: &InputScope) -> Result<(), Error> {
        if supplied != &self.scope {
            self.advance(Control::TransportLost)?;
            return Err(Error::InvalidScope);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_approval::{End, SinkError};
    use std::{cell::RefCell, rc::Rc};

    const SCOPE: InputScope = InputScope {
        room: [1; 16],
        generation: [2; 32],
        viewer_key: [3; 32],
    };
    const RELEASE_UNCONFIRMED: Error = Error::Queue(input_queue::Error::Approval(
        ApprovalError::ReleaseUnconfirmed,
    ));

    #[derive(Default)]
    struct Fake {
        attempts: usize,
        pressed: bool,
        released: usize,
        release_calls: usize,
        fail_apply: bool,
        fail_release: bool,
    }

    impl SessionSink for Rc<RefCell<Fake>> {
        type Event = Event;

        fn apply(&mut self, event: &Event) -> Result<(), SinkError> {
            let mut fake = self.borrow_mut();
            fake.attempts += 1;
            if let Event::Key { down, .. } = event {
                fake.pressed = *down;
            }
            if fake.fail_apply {
                return Err(SinkError);
            }
            Ok(())
        }

        fn release_owned(&mut self) -> Result<(), SinkError> {
            let mut fake = self.borrow_mut();
            fake.release_calls += 1;
            if fake.fail_release {
                return Err(SinkError);
            }
            fake.released += usize::from(fake.pressed);
            fake.pressed = false;
            Ok(())
        }
    }

    type Owner = PublicControlInput<Rc<RefCell<Fake>>>;

    fn owner() -> (Owner, Rc<RefCell<Fake>>) {
        let sink = Rc::new(RefCell::new(Fake::default()));
        let owner = Owner::view_only(
            SCOPE,
            Instant::now() + Duration::from_secs(10),
            sink.clone(),
        )
        .unwrap();
        (owner, sink)
    }

    fn v1(seq: u64, down: bool) -> Vec<u8> {
        format!(
            r#"{{"v":1,"seq":{seq},"session_id":"{}","event":{{"kind":"key","key":"KeyA","down":{down}}}}}"#,
            Uuid::from_bytes(SCOPE.room),
        ).into_bytes()
    }

    fn approve(owner: &mut Owner) {
        let pending = owner
            .begin_local_decision(&SCOPE, Control::Continue)
            .unwrap();
        owner
            .complete_local_decision(pending, &SCOPE, true, Control::Continue)
            .unwrap();
    }

    fn enqueue(owner: &mut Owner, seq: u64, down: bool) -> Result<(), Error> {
        owner.enqueue_v1(&SCOPE, &v1(seq, down), true, Control::Continue)
    }

    #[test]
    fn preconsent_bytes_spend_no_sequence_and_ack_follows_apply_only() {
        let (mut owner, fake) = owner();
        let pending = owner
            .begin_local_decision(&SCOPE, Control::Continue)
            .unwrap();
        assert_eq!(enqueue(&mut owner, 1, true), Err(Error::NotAllowed));
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Continue), Ok(None));
        assert_eq!(fake.borrow().attempts, 0);
        owner
            .complete_local_decision(pending, &SCOPE, true, Control::Continue)
            .unwrap();

        enqueue(&mut owner, 1, true).unwrap();
        assert_eq!(fake.borrow().attempts, 0);
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Continue), Ok(Some(1)));
        assert_eq!(fake.borrow().attempts, 1);
        assert!(fake.borrow().pressed);

        enqueue(&mut owner, 2, false).unwrap();
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Revoke), Ok(None));
        assert_eq!(owner.state(), State::Ended(End::Revoked));
        assert_eq!(fake.borrow().attempts, 1);
        assert_eq!(fake.borrow().released, 1);
        assert!(!fake.borrow().pressed);
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Continue), Ok(None));
        assert_eq!(fake.borrow().release_calls, 1);
    }

    #[test]
    fn loss_and_wrong_generation_defeat_late_native_consent() {
        let (mut stopped, fake) = owner();
        let pending = stopped
            .begin_local_decision(&SCOPE, Control::Continue)
            .unwrap();
        assert_eq!(
            stopped.complete_local_decision(pending, &SCOPE, true, Control::TransportLost),
            Err(Error::NotAllowed)
        );
        assert_eq!(stopped.state(), State::Ended(End::Disconnected));
        assert_eq!(fake.borrow().attempts, 0);
        assert_eq!(fake.borrow().release_calls, 0);

        let (mut stale, fake) = owner();
        let pending = stale
            .begin_local_decision(&SCOPE, Control::Continue)
            .unwrap();
        let changed = InputScope {
            generation: [9; 32],
            ..SCOPE
        };
        assert_eq!(
            stale.complete_local_decision(pending, &changed, true, Control::Continue),
            Err(Error::InvalidScope)
        );
        assert_eq!(stale.state(), State::Ended(End::Disconnected));
        assert_eq!(fake.borrow().attempts, 0);
        assert_eq!(enqueue(&mut stale, 1, true), Err(Error::NotAllowed));
    }

    #[test]
    fn foreign_prompt_token_cannot_grant_this_owner_even_for_same_scope() {
        let (mut old, _) = owner();
        let token = old.begin_local_decision(&SCOPE, Control::Continue).unwrap();
        let (mut new, fake) = owner();
        let local = new.begin_local_decision(&SCOPE, Control::Continue).unwrap();
        assert_eq!(
            new.complete_local_decision(token, &SCOPE, true, Control::Continue),
            Err(Error::Queue(input_queue::Error::Approval(
                ApprovalError::WrongScope
            )))
        );
        assert_eq!(fake.borrow().attempts, 0);
        assert_eq!(new.state(), State::ViewOnly);
        new.complete_local_decision(local, &SCOPE, false, Control::Continue)
            .unwrap();
        assert_eq!(new.state(), State::Ended(End::Declined));
    }

    #[test]
    fn old_generation_received_after_apply_revokes_and_releases() {
        let (mut owner, fake) = owner();
        approve(&mut owner);
        enqueue(&mut owner, 1, true).unwrap();
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Continue), Ok(Some(1)));
        let old = InputScope {
            generation: [8; 32],
            ..SCOPE
        };
        assert_eq!(
            owner.enqueue_v1(&old, &v1(2, false), true, Control::Continue),
            Err(Error::InvalidScope)
        );
        assert_eq!(owner.state(), State::Ended(End::Disconnected));
        assert_eq!(fake.borrow().attempts, 1);
        assert_eq!(fake.borrow().released, 1);
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Continue), Ok(None));
    }

    #[test]
    fn replay_or_overflow_never_acks_and_ends_grant() {
        let (mut replay, fake) = owner();
        approve(&mut replay);
        enqueue(&mut replay, 1, true).unwrap();
        assert_eq!(replay.dispatch_next(&SCOPE, Control::Continue), Ok(Some(1)));
        assert_eq!(enqueue(&mut replay, 1, true), Err(Error::Protocol));
        assert_eq!(replay.state(), State::Ended(End::Revoked));
        assert_eq!(fake.borrow().released, 1);

        let (mut overflow, fake) = owner();
        approve(&mut overflow);
        enqueue(&mut overflow, 1, true).unwrap();
        assert_eq!(
            enqueue(&mut overflow, 2, false),
            Err(input_queue::Error::Full.into())
        );
        assert_eq!(overflow.state(), State::Ended(End::QueueOverflow));
        assert_eq!(overflow.dispatch_next(&SCOPE, Control::Continue), Ok(None));
        assert_eq!(fake.borrow().attempts, 0);
    }

    #[test]
    fn unsupported_wheel_never_reaches_a_sink() {
        let (mut owner, fake) = owner();
        approve(&mut owner);
        let wheel = format!(
            r#"{{"v":1,"seq":1,"session_id":"{}","event":{{"kind":"wheel","delta":120}}}}"#,
            Uuid::from_bytes(SCOPE.room),
        );
        assert_eq!(
            owner.enqueue_v1(&SCOPE, wheel.as_bytes(), true, Control::Continue),
            Err(Error::Protocol)
        );
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Continue), Ok(None));
        assert_eq!(fake.borrow().attempts, 0);
    }

    #[test]
    fn sink_uncertainty_is_sticky_across_later_transport_loss() {
        let (mut owner, fake) = owner();
        approve(&mut owner);
        {
            let mut fake = fake.borrow_mut();
            fake.fail_apply = true;
            fake.fail_release = true;
        }
        enqueue(&mut owner, 1, true).unwrap();
        assert_eq!(
            owner.dispatch_next(&SCOPE, Control::Continue),
            Err(RELEASE_UNCONFIRMED)
        );
        assert_eq!(owner.state(), State::Ended(End::SinkFailed));
        assert_eq!(fake.borrow().attempts, 1);
        assert_eq!(fake.borrow().release_calls, 1);
        assert_eq!(
            owner.advance(Control::TransportLost),
            Err(RELEASE_UNCONFIRMED)
        );
        assert_eq!(fake.borrow().release_calls, 1);
        assert_eq!(enqueue(&mut owner, 1, false), Err(RELEASE_UNCONFIRMED));
    }

    #[test]
    fn monotonic_expiry_wins_over_a_pending_event_even_while_idle() {
        let until = Instant::now() + Duration::from_millis(100);
        let fake = Rc::new(RefCell::new(Fake::default()));
        let mut owner = Owner::view_only(SCOPE, until, fake.clone()).unwrap();
        approve(&mut owner);
        enqueue(&mut owner, 1, true).unwrap();
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Continue), Ok(Some(1)));
        enqueue(&mut owner, 2, false).unwrap();
        while Instant::now() < until {
            std::thread::sleep(until.saturating_duration_since(Instant::now()));
        }
        assert_eq!(
            owner.advance(Control::Continue),
            Ok(State::Ended(End::Expired))
        );
        assert_eq!(owner.dispatch_next(&SCOPE, Control::Continue), Ok(None));
        assert_eq!(fake.borrow().attempts, 1);
        assert_eq!(fake.borrow().released, 1);
    }
}

//! An isolated, host-owned approval state machine. No sockets, IPC, UI, or
//! operating-system input are wired here.
use std::time::{Duration, Instant};

const MAX_APPROVAL: Duration = Duration::from_secs(60);

/// Values must come from an already authenticated native session. Constructing
/// this value is not proof of device identity or transport authentication.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InputScope {
    pub room: [u8; 16],
    pub generation: [u8; 32],
    pub viewer_key: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Declined,
    Revoked,
    Disconnected,
    Expired,
    SinkFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    ViewOnly,
    Allowed,
    Ended(End),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NotAllowed,
    WrongScope,
    ApplyFailed,
    ReleaseUnconfirmed,
}

#[derive(Clone, Copy, Debug)]
pub struct SinkError;

/// Implementations own their own pressed-state tracking. Release must act only
/// on input applied by this one gate, never on unrelated locally held keys.
pub trait SessionSink {
    type Event;
    fn apply(&mut self, event: &Self::Event) -> Result<(), SinkError>;
    fn release_owned(&mut self) -> Result<(), SinkError>;
}

/// Keep on a single serialized native owner. The owner must call
/// `advance` on a timer even if no input arrives, and `disconnect` on any
/// transport loss. Never give network / UI code a reference to the sink.
pub struct Approval<S: SessionSink> {
    scope: InputScope,
    session_until: Instant,
    permission_until: Option<Instant>,
    state: State,
    release_unconfirmed: bool,
    sink: S,
}

impl<S: SessionSink> Approval<S> {
    pub fn view_only(scope: InputScope, session_until: Instant, sink: S) -> Self {
        Self {
            scope,
            session_until,
            permission_until: None,
            state: State::ViewOnly,
            release_unconfirmed: false,
            sink,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Call only from the host's local native consent path. Viewing consent
    /// must not call this. A grant cannot be renewed or transferred.
    pub fn decide_locally(&mut self, approved: bool, now: Instant) -> Result<(), Error> {
        self.advance(now)?;
        if self.state != State::ViewOnly {
            return Err(Error::NotAllowed);
        }
        if !approved {
            return self.end(End::Declined);
        }
        let until = now
            .checked_add(MAX_APPROVAL)
            .map_or(self.session_until, |max| max.min(self.session_until));
        self.permission_until = Some(until);
        self.state = State::Allowed;
        Ok(())
    }

    /// Synchronous check and application on the same owner; never return a
    /// reusable permission boolean to a separately scheduled task.
    pub fn apply(
        &mut self,
        scope: &InputScope,
        event: &S::Event,
        now: Instant,
    ) -> Result<(), Error> {
        self.advance(now)?;
        if scope != &self.scope {
            return Err(Error::WrongScope);
        }
        if self.state != State::Allowed {
            return Err(Error::NotAllowed);
        }
        if self.sink.apply(event).is_err() {
            self.end(End::SinkFailed)?;
            return Err(Error::ApplyFailed);
        }
        Ok(())
    }

    pub fn advance(&mut self, now: Instant) -> Result<(), Error> {
        if now >= self.session_until || self.permission_until.is_some_and(|until| now >= until) {
            self.end(End::Expired)?;
        }
        Ok(())
    }

    pub fn revoke(&mut self) -> Result<(), Error> {
        self.end(End::Revoked)
    }

    pub fn disconnect(&mut self) -> Result<(), Error> {
        self.end(End::Disconnected)
    }

    fn end(&mut self, reason: End) -> Result<(), Error> {
        if matches!(self.state, State::Ended(_)) {
            return if self.release_unconfirmed {
                Err(Error::ReleaseUnconfirmed)
            } else {
                Ok(())
            };
        }
        // Deny further input even if release fails.
        let had_grant = self.state == State::Allowed;
        self.state = State::Ended(reason);
        self.permission_until = None;
        if had_grant && self.sink.release_owned().is_err() {
            self.release_unconfirmed = true;
            return Err(Error::ReleaseUnconfirmed);
        }
        Ok(())
    }
}

impl<S: SessionSink> Drop for Approval<S> {
    fn drop(&mut self) {
        let _ = self.end(End::Disconnected);
    }
}

#[cfg(test)]
#[path = "input_approval_queue_tests.rs"]
mod queue_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    const SCOPE: InputScope = InputScope {
        room: [1; 16],
        generation: [2; 32],
        viewer_key: [3; 32],
    };

    #[derive(Default)]
    struct Log {
        applied: Vec<u8>,
        releases: usize,
        apply_fails: bool,
        release_fails: bool,
    }

    impl SessionSink for Rc<RefCell<Log>> {
        type Event = u8;
        fn apply(&mut self, event: &u8) -> Result<(), SinkError> {
            let mut log = self.borrow_mut();
            log.applied.push(*event);
            if log.apply_fails {
                Err(SinkError)
            } else {
                Ok(())
            }
        }
        fn release_owned(&mut self) -> Result<(), SinkError> {
            let mut log = self.borrow_mut();
            log.releases += 1;
            if log.release_fails {
                Err(SinkError)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn viewing_or_declining_does_not_enable_input() {
        let now = Instant::now();
        let log = Rc::new(RefCell::new(Log::default()));
        let mut gate = Approval::view_only(SCOPE, now + MAX_APPROVAL, log.clone());
        assert_eq!(gate.apply(&SCOPE, &7, now), Err(Error::NotAllowed));
        assert_eq!(gate.decide_locally(false, now), Ok(()));
        assert_eq!(gate.state(), State::Ended(End::Declined));
        assert_eq!(gate.decide_locally(true, now), Err(Error::NotAllowed));
        assert!(log.borrow().applied.is_empty());
        assert_eq!(log.borrow().releases, 0);
    }

    #[test]
    fn approval_admits_only_this_room_generation_and_viewer() {
        let now = Instant::now();
        let log = Rc::new(RefCell::new(Log::default()));
        let mut gate = Approval::view_only(SCOPE, now + MAX_APPROVAL, log.clone());
        gate.decide_locally(true, now).unwrap();
        for scope in [
            InputScope {
                room: [4; 16],
                ..SCOPE
            },
            InputScope {
                generation: [4; 32],
                ..SCOPE
            },
            InputScope {
                viewer_key: [4; 32],
                ..SCOPE
            },
        ] {
            assert_eq!(gate.apply(&scope, &7, now), Err(Error::WrongScope));
        }
        assert_eq!(gate.apply(&SCOPE, &9, now), Ok(()));
        assert_eq!(log.borrow().applied, [9]);
    }

    #[test]
    fn revoke_or_disconnect_releases_once_and_refuses_late_events() {
        let now = Instant::now();
        for stop in [End::Revoked, End::Disconnected] {
            let log = Rc::new(RefCell::new(Log::default()));
            let mut gate = Approval::view_only(SCOPE, now + MAX_APPROVAL, log.clone());
            gate.decide_locally(true, now).unwrap();
            gate.apply(&SCOPE, &9, now).unwrap();
            if stop == End::Revoked {
                gate.revoke().unwrap();
            } else {
                gate.disconnect().unwrap();
            }
            assert_eq!(gate.state(), State::Ended(stop));
            assert_eq!(gate.apply(&SCOPE, &7, now), Err(Error::NotAllowed));
            assert_eq!(gate.decide_locally(true, now), Err(Error::NotAllowed));
            gate.revoke().unwrap();
            drop(gate);
            assert_eq!(log.borrow().releases, 1);
            assert_eq!(log.borrow().applied, [9]);
        }
    }

    #[test]
    fn either_deadline_expires_without_waiting_for_a_packet() {
        let now = Instant::now();
        for lease in [Duration::from_secs(5), Duration::from_secs(120)] {
            let log = Rc::new(RefCell::new(Log::default()));
            let mut gate = Approval::view_only(SCOPE, now + lease, log.clone());
            gate.decide_locally(true, now).unwrap();
            assert_eq!(gate.decide_locally(true, now), Err(Error::NotAllowed));
            let deadline = now + lease.min(MAX_APPROVAL);
            gate.advance(deadline - Duration::from_millis(1)).unwrap();
            assert_eq!(gate.state(), State::Allowed);
            gate.advance(deadline).unwrap();
            assert_eq!(gate.state(), State::Ended(End::Expired));
            assert_eq!(gate.apply(&SCOPE, &7, deadline), Err(Error::NotAllowed));
            assert_eq!(log.borrow().releases, 1);
        }
    }

    #[test]
    fn an_already_expired_view_cannot_get_input_approval() {
        let now = Instant::now();
        let log = Rc::new(RefCell::new(Log::default()));
        let mut gate = Approval::view_only(SCOPE, now, log.clone());
        assert_eq!(gate.decide_locally(true, now), Err(Error::NotAllowed));
        assert_eq!(gate.state(), State::Ended(End::Expired));
        assert_eq!(log.borrow().releases, 0);
    }

    #[test]
    fn dropping_a_live_gate_releases_its_owned_state() {
        let now = Instant::now();
        let log = Rc::new(RefCell::new(Log::default()));
        let mut gate = Approval::view_only(SCOPE, now + MAX_APPROVAL, log.clone());
        gate.decide_locally(true, now).unwrap();
        drop(gate);
        assert_eq!(log.borrow().releases, 1);
    }

    #[test]
    fn sink_failure_seals_the_gate_even_if_release_cannot_be_confirmed() {
        let now = Instant::now();
        let log = Rc::new(RefCell::new(Log {
            apply_fails: true,
            release_fails: true,
            ..Log::default()
        }));
        let mut gate = Approval::view_only(SCOPE, now + MAX_APPROVAL, log.clone());
        gate.decide_locally(true, now).unwrap();
        assert_eq!(gate.apply(&SCOPE, &7, now), Err(Error::ReleaseUnconfirmed));
        assert_eq!(gate.state(), State::Ended(End::SinkFailed));
        assert_eq!(gate.apply(&SCOPE, &8, now), Err(Error::NotAllowed));
        assert_eq!(gate.revoke(), Err(Error::ReleaseUnconfirmed));
        assert_eq!(log.borrow().releases, 1);
        assert_eq!(log.borrow().applied, [7]);
    }
}

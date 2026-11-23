//! An isolated, host-owned approval state machine. No sockets, IPC, UI, or
//! operating-system input are wired here.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_APPROVAL: Duration = Duration::from_secs(60);

/// Values must come from an already authenticated native session. Constructing
/// this value is not proof of device identity or transport authentication.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InputScope {
    pub room: [u8; 16],
    pub generation: [u8; 32],
    pub viewer_key: [u8; 32],
}

/// Internal exact binding. An address-pair scope is never a device scope.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Binding {
    Device(InputScope),
    #[cfg(feature = "p3-lan-input-queue-test")]
    LegacyLan(crate::lan_input_queue::LanInputScope),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Declined,
    Revoked,
    Disconnected,
    Expired,
    QueueOverflow,
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

/// One in-process native prompt. It is not an input grant or a wire value.
#[must_use]
pub struct PendingDecision(Arc<()>);

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
    scope: Binding,
    session_until: Instant,
    permission_until: Option<Instant>,
    pending_decision: Option<Arc<()>>,
    state: State,
    release_unconfirmed: bool,
    sink: S,
}

impl<S: SessionSink> Approval<S> {
    pub fn view_only(scope: InputScope, session_until: Instant, sink: S) -> Self {
        Self::view_only_bound(Binding::Device(scope), session_until, sink)
    }

    pub(super) fn view_only_bound(scope: Binding, session_until: Instant, sink: S) -> Self {
        Self {
            scope,
            session_until,
            permission_until: None,
            pending_decision: None,
            state: State::ViewOnly,
            release_unconfirmed: false,
            sink,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Reserve one prompt for the current native device session. A prompt
    /// can be open while capture and transport stop independently.
    pub fn begin_local_decision(
        &mut self,
        current: &InputScope,
        now: Instant,
    ) -> Result<PendingDecision, Error> {
        self.advance(now)?;
        if self.scope != Binding::Device(*current) {
            return Err(Error::WrongScope);
        }
        if self.state != State::ViewOnly || self.pending_decision.is_some() {
            return Err(Error::NotAllowed);
        }
        let marker = Arc::new(());
        self.pending_decision = Some(marker.clone());
        Ok(PendingDecision(marker))
    }

    /// Complete only on the original serialized owner, after sampling the
    /// current native session and servicing any stop/disconnect flags. Pass
    /// false for decline or if the dialog could not complete.
    pub fn complete_local_decision(
        &mut self,
        pending: PendingDecision,
        current: &InputScope,
        approved: bool,
        now: Instant,
    ) -> Result<(), Error> {
        self.advance(now)?;
        let marker = self.pending_decision.as_ref().ok_or(Error::NotAllowed)?;
        if !Arc::ptr_eq(marker, &pending.0) {
            // Never consume a different owner's outstanding prompt.
            return Err(Error::WrongScope);
        }
        self.pending_decision = None;
        if self.scope != Binding::Device(*current) {
            self.disconnect()?;
            return Err(Error::WrongScope);
        }
        self.decide_locally(approved, now)
    }

    /// Synchronous host-local decisions only. For a dialog that crosses an
    /// await, reserve and complete a `PendingDecision` instead. Viewing
    /// consent must not call this. A grant cannot be renewed or transferred.
    pub fn decide_locally(&mut self, approved: bool, now: Instant) -> Result<(), Error> {
        self.advance(now)?;
        if self.state != State::ViewOnly || self.pending_decision.is_some() {
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
        self.apply_bound(&Binding::Device(*scope), event, now)
    }

    pub(super) fn apply_bound(
        &mut self,
        scope: &Binding,
        event: &S::Event,
        now: Instant,
    ) -> Result<(), Error> {
        self.advance(now)?;
        self.authorize(scope)?;
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

    /// Scope and consent only. The owner must advance the monotonic clock
    /// first, and still use `apply` to revalidate at dispatch.
    pub(super) fn authorize(&self, scope: &Binding) -> Result<(), Error> {
        if scope != &self.scope {
            return Err(Error::WrongScope);
        }
        if self.state != State::Allowed {
            return Err(Error::NotAllowed);
        }
        Ok(())
    }

    pub(super) fn overflow(&mut self) -> Result<(), Error> {
        self.end(End::QueueOverflow)
    }

    pub(super) fn release_status(&self) -> Result<(), Error> {
        if self.release_unconfirmed {
            return Err(Error::ReleaseUnconfirmed);
        }
        Ok(())
    }

    fn end(&mut self, reason: End) -> Result<(), Error> {
        if matches!(self.state, State::Ended(_)) {
            return self.release_status();
        }
        // Deny further input even if release fails.
        let had_grant = self.state == State::Allowed;
        self.state = State::Ended(reason);
        self.permission_until = None;
        self.pending_decision = None;
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
    fn pending_native_dialog_never_grants_until_matching_local_response() {
        let now = Instant::now();
        let log = Rc::new(RefCell::new(Log::default()));
        let mut gate = Approval::view_only(SCOPE, now + MAX_APPROVAL, log.clone());
        let prompt = gate.begin_local_decision(&SCOPE, now).unwrap();
        assert_eq!(gate.apply(&SCOPE, &5, now), Err(Error::NotAllowed));
        assert_eq!(
            gate.begin_local_decision(&SCOPE, now).err(),
            Some(Error::NotAllowed)
        );
        assert_eq!(gate.decide_locally(true, now), Err(Error::NotAllowed));
        gate.complete_local_decision(prompt, &SCOPE, true, now)
            .unwrap();
        assert_eq!(gate.apply(&SCOPE, &5, now), Ok(()));
        assert_eq!(log.borrow().applied, [5]);
    }

    #[test]
    fn delayed_yes_is_inert_after_stop_disconnect_or_exact_deadline() {
        let now = Instant::now();
        let until = now + MAX_APPROVAL;
        for reason in [End::Revoked, End::Disconnected, End::Expired] {
            let log = Rc::new(RefCell::new(Log::default()));
            let mut gate = Approval::view_only(SCOPE, until, log.clone());
            let prompt = gate.begin_local_decision(&SCOPE, now).unwrap();
            let returned_at = match reason {
                End::Revoked => {
                    gate.revoke().unwrap();
                    now
                }
                End::Disconnected => {
                    gate.disconnect().unwrap();
                    now
                }
                // The dialog finishing exactly on the deadline still loses.
                End::Expired => until,
                _ => unreachable!(),
            };
            assert_eq!(
                gate.complete_local_decision(prompt, &SCOPE, true, returned_at),
                Err(Error::NotAllowed)
            );
            assert_eq!(gate.state(), State::Ended(reason));
            assert_eq!(gate.apply(&SCOPE, &1, returned_at), Err(Error::NotAllowed));
            assert!(log.borrow().applied.is_empty());
            assert_eq!(log.borrow().releases, 0);
        }
    }

    #[test]
    fn a_closed_or_replaced_dialog_cannot_authorize_a_different_owner_or_generation() {
        let now = Instant::now();
        let until = now + MAX_APPROVAL;
        let log = Rc::new(RefCell::new(Log::default()));
        let mut old = Approval::view_only(SCOPE, until, log.clone());
        let stale = old.begin_local_decision(&SCOPE, now).unwrap();
        drop(old);

        // Even if identical fixture session values are accidentally reused,
        // a different gate has a different pending prompt.
        let mut replacement = Approval::view_only(SCOPE, until, log.clone());
        let current_prompt = replacement.begin_local_decision(&SCOPE, now).unwrap();
        assert_eq!(
            replacement.complete_local_decision(stale, &SCOPE, true, now),
            Err(Error::WrongScope)
        );
        assert_eq!(replacement.apply(&SCOPE, &5, now), Err(Error::NotAllowed));
        let new_scope = InputScope {
            generation: [4; 32],
            ..SCOPE
        };
        assert_eq!(
            replacement.complete_local_decision(current_prompt, &new_scope, true, now),
            Err(Error::WrongScope)
        );
        assert_eq!(replacement.state(), State::Ended(End::Disconnected));
        assert!(log.borrow().applied.is_empty());
    }

    #[test]
    fn native_prompt_decline_is_terminal_without_applying_input() {
        let now = Instant::now();
        let log = Rc::new(RefCell::new(Log::default()));
        let mut gate = Approval::view_only(SCOPE, now + MAX_APPROVAL, log.clone());
        let prompt = gate.begin_local_decision(&SCOPE, now).unwrap();
        gate.complete_local_decision(prompt, &SCOPE, false, now)
            .unwrap();
        assert_eq!(gate.state(), State::Ended(End::Declined));
        assert_eq!(
            gate.begin_local_decision(&SCOPE, now).err(),
            Some(Error::NotAllowed)
        );
        assert!(log.borrow().applied.is_empty());
        assert_eq!(log.borrow().releases, 0);
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

//! A synchronous queue owned by the native approval owner. This module does
//! not accept wire messages, authenticate peers or connect to an input driver.

use crate::input_approval::{
    Approval, Binding, Error as ApprovalError, InputScope, PendingDecision, SessionSink, State,
};
use std::{collections::VecDeque, time::Instant};

const CAPACITY: usize = 32;

/// The native owner’s current local signal, never a value from an input event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Continue,
    Revoke,
    TransportLost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch {
    Idle,
    Applied,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Approval(ApprovalError),
    /// Overflow permanently closed the grant and cleanup was confirmed.
    Full,
}

impl From<ApprovalError> for Error {
    fn from(error: ApprovalError) -> Self {
        Self::Approval(error)
    }
}

struct Pending<E> {
    scope: Binding,
    event: E,
}

/// A one-generation local owner with 32 event slots. Both scopes must come from the verified
/// native session; events must already be typed, validated and size-bounded.
/// Keep the sink and this owner on the same serialized execution path;
/// never dispatch events from a separate task or hand the sink out.
///
/// Apply pending control signals before an event (even with a full queue).
/// Drive `advance` on a local monotonic timer even without events. Awaiting
/// neither timer nor another event is necessary to revoke through `advance`.
/// Explicitly close and check its result before dropping this owner; Drop
/// is a best-effort fallback and cannot report release uncertainty.
pub struct InputQueue<S: SessionSink> {
    approval: Approval<S>,
    pending: VecDeque<Pending<S::Event>>,
}

impl<S: SessionSink> InputQueue<S> {
    pub fn view_only(scope: InputScope, session_until: Instant, sink: S) -> Self {
        Self::view_only_bound(Binding::Device(scope), session_until, sink)
    }

    #[cfg(feature = "v2-public-control-sustained")]
    pub(super) fn view_only_sustained(scope: InputScope, session_until: Instant, sink: S) -> Self {
        Self {
            approval: Approval::view_only_sustained(scope, session_until, sink),
            pending: VecDeque::new(),
        }
    }

    pub(super) fn view_only_bound(scope: Binding, session_until: Instant, sink: S) -> Self {
        Self {
            approval: Approval::view_only_bound(scope, session_until, sink),
            pending: VecDeque::new(),
        }
    }

    pub fn state(&self) -> State {
        self.approval.state()
    }

    /// Begin a separate host-local prompt for the current verified device scope.
    /// Input stays view-only while the prompt is pending.
    pub fn begin_local_decision(&mut self, current: &InputScope) -> Result<PendingDecision, Error> {
        self.begin_local_decision_at(current, Instant::now())
    }

    /// Complete one prompt on the original serialized owner. Sample the
    /// current verified scope and local control before entering this turn;
    /// a pending revoke, transport loss, or expiry wins over any response.
    pub fn complete_local_decision(
        &mut self,
        pending: PendingDecision,
        current: &InputScope,
        approved: bool,
        control: Control,
    ) -> Result<(), Error> {
        self.complete_local_decision_at(pending, current, approved, control, Instant::now())
    }

    /// Synchronous host-local input consent only. For a prompt crossing an
    /// await, use the device-scoped begin/complete pair instead.
    pub fn decide_locally(&mut self, approved: bool) -> Result<(), Error> {
        self.decide_at(approved, Instant::now())
    }

    pub fn enqueue(&mut self, scope: &InputScope, event: S::Event) -> Result<(), Error> {
        self.enqueue_bound(&Binding::Device(*scope), event)
    }

    pub(super) fn enqueue_bound(&mut self, scope: &Binding, event: S::Event) -> Result<(), Error> {
        self.enqueue_bound_at(scope, event, Instant::now())
    }

    /// One synchronous owner turn. Always pass the current verified scope,
    /// not one supplied by a dequeued event or an external caller.
    pub fn dispatch_next(
        &mut self,
        scope: &InputScope,
        control: Control,
    ) -> Result<Dispatch, Error> {
        self.dispatch_bound(&Binding::Device(*scope), control)
    }

    pub(super) fn dispatch_bound(
        &mut self,
        scope: &Binding,
        control: Control,
    ) -> Result<Dispatch, Error> {
        self.dispatch_bound_at(scope, control, Instant::now())
    }

    /// Advance expiry or report a pending local revoke/transport loss even
    /// when no input is queued. Release errors remain visible on later calls.
    pub fn advance(&mut self, control: Control) -> Result<State, Error> {
        self.advance_at(control, Instant::now())
    }

    fn begin_local_decision_at(
        &mut self,
        current: &InputScope,
        now: Instant,
    ) -> Result<PendingDecision, Error> {
        self.advance_at(Control::Continue, now)?;
        self.approval
            .begin_local_decision(current, now)
            .map_err(Error::from)
    }

    fn complete_local_decision_at(
        &mut self,
        pending: PendingDecision,
        current: &InputScope,
        approved: bool,
        control: Control,
        now: Instant,
    ) -> Result<(), Error> {
        self.advance_at(control, now)?;
        let result = self
            .approval
            .complete_local_decision(pending, current, approved, now);
        self.finish(result)
    }

    fn decide_at(&mut self, approved: bool, now: Instant) -> Result<(), Error> {
        self.advance_at(Control::Continue, now)?;
        let result = self.approval.decide_locally(approved, now);
        self.finish(result)
    }

    #[cfg(test)]
    fn enqueue_at(
        &mut self,
        scope: &InputScope,
        event: S::Event,
        now: Instant,
    ) -> Result<(), Error> {
        self.enqueue_bound_at(&Binding::Device(*scope), event, now)
    }

    fn enqueue_bound_at(
        &mut self,
        scope: &Binding,
        event: S::Event,
        now: Instant,
    ) -> Result<(), Error> {
        self.advance_at(Control::Continue, now)?;
        self.approval.authorize(scope)?;
        if self.pending.len() == CAPACITY {
            // Do not drop a possible release event and continue applying
            // a now-incomplete stream of presses/releases.
            self.close_for_overflow()?;
            return Err(Error::Full);
        }
        self.pending.push_back(Pending {
            scope: *scope,
            event,
        });
        Ok(())
    }

    pub(super) fn close_for_overflow(&mut self) -> Result<(), Error> {
        self.pending.clear();
        let result = self.approval.overflow();
        self.finish(result)
    }

    #[cfg(test)]
    fn dispatch_at(
        &mut self,
        scope: &InputScope,
        control: Control,
        now: Instant,
    ) -> Result<Dispatch, Error> {
        self.dispatch_bound_at(&Binding::Device(*scope), control, now)
    }

    pub(super) fn dispatch_bound_at(
        &mut self,
        scope: &Binding,
        control: Control,
        now: Instant,
    ) -> Result<Dispatch, Error> {
        if matches!(self.advance_at(control, now)?, State::Ended(_)) {
            return Ok(Dispatch::Closed);
        }
        if let Err(error) = self.approval.authorize(scope) {
            if error == ApprovalError::WrongScope {
                // The current authenticated session changed. The original
                // queue cannot be dispatched or revived under either scope.
                self.advance_at(Control::TransportLost, now)?;
            }
            return Err(error.into());
        }
        let Some(item) = self.pending.pop_front() else {
            return Ok(Dispatch::Idle);
        };
        let result = self.approval.apply_bound(&item.scope, &item.event, now);
        self.finish(result)?;
        Ok(Dispatch::Applied)
    }

    fn advance_at(&mut self, control: Control, now: Instant) -> Result<State, Error> {
        if control != Control::Continue {
            self.pending.clear();
        }
        let result = match control {
            Control::Continue => self.approval.advance(now),
            Control::Revoke => self.approval.revoke(),
            Control::TransportLost => self.approval.disconnect(),
        };
        self.finish(result)?;
        Ok(self.approval.state())
    }

    fn finish(&mut self, result: Result<(), ApprovalError>) -> Result<(), Error> {
        if matches!(self.approval.state(), State::Ended(_)) {
            self.pending.clear();
        }
        self.approval
            .release_status()
            .and(result)
            .map_err(Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_approval::{End, SinkError};
    use std::{cell::RefCell, collections::BTreeSet, rc::Rc, time::Duration};

    const SCOPE: InputScope = InputScope {
        room: [1; 16],
        generation: [2; 32],
        viewer_key: [3; 32],
    };

    #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
    enum Press {
        Key(u8),
        Button(u8),
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Down(Press),
        Up(Press),
    }

    const LOCAL_KEY: Press = Press::Key(1);
    const SESSION_KEY: Press = Press::Key(2);
    const SESSION_BUTTON: Press = Press::Button(1);
    const DOWN: Event = Event::Down(SESSION_KEY);
    const UP: Event = Event::Up(SESSION_KEY);

    #[derive(Default)]
    struct Fake {
        local: BTreeSet<Press>,
        owned: BTreeSet<Press>,
        applied: Vec<Event>,
        released: BTreeSet<Press>,
        release_calls: u8,
        apply_fails: bool,
        release_fails: bool,
    }

    impl SessionSink for Rc<RefCell<Fake>> {
        type Event = Event;

        fn apply(&mut self, event: &Event) -> Result<(), SinkError> {
            let mut fake = self.borrow_mut();
            fake.applied.push(*event);
            match *event {
                Event::Down(press) => fake.owned.insert(press),
                Event::Up(press) => fake.owned.remove(&press),
            };
            if fake.apply_fails {
                Err(SinkError)
            } else {
                Ok(())
            }
        }

        fn release_owned(&mut self) -> Result<(), SinkError> {
            let mut fake = self.borrow_mut();
            fake.release_calls += 1;
            if fake.release_fails {
                return Err(SinkError);
            }
            let owned = std::mem::take(&mut fake.owned);
            fake.released.extend(owned);
            Ok(())
        }
    }

    type Queue = InputQueue<Rc<RefCell<Fake>>>;

    fn view_only(now: Instant, ttl: Duration) -> (Queue, Rc<RefCell<Fake>>) {
        let sink = Rc::new(RefCell::new(Fake {
            local: BTreeSet::from([LOCAL_KEY]),
            ..Fake::default()
        }));
        (InputQueue::view_only(SCOPE, now + ttl, sink.clone()), sink)
    }

    fn allowed(now: Instant, ttl: Duration) -> (Queue, Rc<RefCell<Fake>>) {
        let (mut queue, sink) = view_only(now, ttl);
        queue.decide_at(true, now).unwrap();
        (queue, sink)
    }

    fn fill_with_releases(queue: &mut Queue, now: Instant) {
        // Leave both a keyboard key and a mouse button held by this scope.
        for event in [DOWN, Event::Down(SESSION_BUTTON)] {
            queue.enqueue_at(&SCOPE, event, now).unwrap();
            assert_eq!(
                queue.dispatch_at(&SCOPE, Control::Continue, now),
                Ok(Dispatch::Applied)
            );
        }
        for _ in 0..CAPACITY {
            queue.enqueue_at(&SCOPE, UP, now).unwrap();
        }
        assert_eq!(queue.pending.len(), CAPACITY);
    }

    fn expect_closed(queue: &mut Queue, now: Instant, reason: End, sink: &RefCell<Fake>) {
        assert_eq!(queue.state(), State::Ended(reason));
        assert!(queue.pending.is_empty());
        assert_eq!(
            queue.dispatch_at(&SCOPE, Control::Continue, now),
            Ok(Dispatch::Closed)
        );
        assert_eq!(
            queue.enqueue_at(&SCOPE, DOWN, now),
            Err(ApprovalError::NotAllowed.into())
        );
        assert_eq!(
            queue.decide_at(true, now),
            Err(ApprovalError::NotAllowed.into())
        );
        let sink = sink.borrow();
        assert_eq!(sink.applied, [DOWN, Event::Down(SESSION_BUTTON)]);
        assert_eq!(sink.release_calls, 1);
        assert!(sink.owned.is_empty());
        assert_eq!(sink.released, BTreeSet::from([SESSION_KEY, SESSION_BUTTON]));
        assert_eq!(sink.local, BTreeSet::from([LOCAL_KEY]));
    }

    #[test]
    fn view_only_or_declined_never_enqueues_or_applies() {
        let (mut queue, sink) = view_only(Instant::now(), Duration::from_secs(10));
        assert_eq!(queue.state(), State::ViewOnly);
        assert_eq!(
            queue.enqueue(&SCOPE, DOWN),
            Err(ApprovalError::NotAllowed.into())
        );
        assert_eq!(
            queue.dispatch_next(&SCOPE, Control::Continue),
            Err(ApprovalError::NotAllowed.into())
        );
        assert!(queue.pending.is_empty());
        queue.decide_locally(false).unwrap();
        assert_eq!(queue.state(), State::Ended(End::Declined));
        assert_eq!(
            queue.decide_locally(true),
            Err(ApprovalError::NotAllowed.into())
        );
        assert!(sink.borrow().applied.is_empty());
        assert_eq!(sink.borrow().release_calls, 0);
    }

    #[test]
    fn pending_prompt_keeps_input_out_until_separate_local_approval() {
        let now = Instant::now();
        let (mut queue, sink) = view_only(now, Duration::from_secs(10));
        let prompt = queue.begin_local_decision_at(&SCOPE, now).unwrap();
        assert_eq!(queue.state(), State::ViewOnly);
        assert_eq!(
            queue.begin_local_decision_at(&SCOPE, now).err(),
            Some(ApprovalError::NotAllowed.into())
        );
        assert_eq!(
            queue.decide_at(true, now),
            Err(ApprovalError::NotAllowed.into())
        );
        assert_eq!(
            queue.enqueue_at(&SCOPE, DOWN, now),
            Err(ApprovalError::NotAllowed.into())
        );
        assert_eq!(
            queue.dispatch_at(&SCOPE, Control::Continue, now),
            Err(ApprovalError::NotAllowed.into())
        );
        assert!(queue.pending.is_empty());
        assert!(sink.borrow().applied.is_empty());

        queue
            .complete_local_decision_at(prompt, &SCOPE, true, Control::Continue, now)
            .unwrap();
        assert_eq!(queue.state(), State::Allowed);
        queue.enqueue_at(&SCOPE, DOWN, now).unwrap();
        assert_eq!(
            queue.dispatch_at(&SCOPE, Control::Continue, now),
            Ok(Dispatch::Applied)
        );
        assert_eq!(sink.borrow().applied, [DOWN]);
    }

    #[test]
    fn declining_the_pending_prompt_never_grants_input() {
        let now = Instant::now();
        let (mut queue, sink) = view_only(now, Duration::from_secs(10));
        let prompt = queue.begin_local_decision_at(&SCOPE, now).unwrap();
        queue
            .complete_local_decision_at(prompt, &SCOPE, false, Control::Continue, now)
            .unwrap();
        assert_eq!(queue.state(), State::Ended(End::Declined));
        assert_eq!(
            queue.enqueue_at(&SCOPE, DOWN, now),
            Err(ApprovalError::NotAllowed.into())
        );
        assert_eq!(
            queue.dispatch_at(&SCOPE, Control::Continue, now),
            Ok(Dispatch::Closed)
        );
        assert!(sink.borrow().applied.is_empty());
        assert_eq!(sink.borrow().release_calls, 0);
    }

    #[test]
    fn foreign_prompt_does_not_consume_owners_decision_and_stale_scope_disconnects() {
        let now = Instant::now();
        let (mut queue, sink) = view_only(now, Duration::from_secs(10));
        let (mut other, _) = view_only(now, Duration::from_secs(10));
        let replaced = InputScope {
            generation: [4; 32],
            ..SCOPE
        };

        assert_eq!(
            queue.begin_local_decision_at(&replaced, now).err(),
            Some(ApprovalError::WrongScope.into())
        );
        let prompt = queue.begin_local_decision_at(&SCOPE, now).unwrap();
        let foreign = other.begin_local_decision_at(&SCOPE, now).unwrap();
        assert_eq!(
            queue.complete_local_decision_at(foreign, &SCOPE, true, Control::Continue, now),
            Err(ApprovalError::WrongScope.into())
        );
        assert_eq!(
            queue.begin_local_decision_at(&SCOPE, now).err(),
            Some(ApprovalError::NotAllowed.into()),
            "foreign marker must leave the reserved prompt intact"
        );
        assert_eq!(
            queue.complete_local_decision_at(prompt, &replaced, true, Control::Continue, now),
            Err(ApprovalError::WrongScope.into())
        );
        assert_eq!(queue.state(), State::Ended(End::Disconnected));
        assert!(queue.pending.is_empty());
        assert!(sink.borrow().applied.is_empty());
        assert_eq!(sink.borrow().release_calls, 0);
    }

    #[test]
    fn current_stop_loss_or_exact_deadline_beats_delayed_yes() {
        let now = Instant::now();
        let ttl = Duration::from_secs(10);
        let expires = now + ttl;
        for (control, completed_at, reason) in [
            (Control::Revoke, expires, End::Revoked),
            (Control::TransportLost, expires, End::Disconnected),
            (Control::Continue, expires, End::Expired),
        ] {
            let (mut queue, sink) = view_only(now, ttl);
            let prompt = queue.begin_local_decision_at(&SCOPE, now).unwrap();
            assert_eq!(
                queue.complete_local_decision_at(prompt, &SCOPE, true, control, completed_at),
                Err(ApprovalError::NotAllowed.into())
            );
            assert_eq!(queue.state(), State::Ended(reason));
            assert_eq!(
                queue.dispatch_at(&SCOPE, Control::Continue, completed_at),
                Ok(Dispatch::Closed)
            );
            assert!(queue.pending.is_empty());
            assert!(sink.borrow().applied.is_empty());
            assert_eq!(sink.borrow().release_calls, 0);
        }
    }

    #[test]
    fn one_verified_scope_receives_events_in_order() {
        let now = Instant::now();
        let (mut queue, sink) = allowed(now, Duration::from_secs(10));
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
            assert_eq!(
                queue.enqueue_at(&scope, DOWN, now),
                Err(ApprovalError::WrongScope.into())
            );
        }

        for event in [DOWN, UP] {
            queue.enqueue_at(&SCOPE, event, now).unwrap();
        }
        assert!(sink.borrow().applied.is_empty());
        for _ in 0..2 {
            assert_eq!(
                queue.dispatch_at(&SCOPE, Control::Continue, now),
                Ok(Dispatch::Applied)
            );
        }
        assert_eq!(
            queue.dispatch_at(&SCOPE, Control::Continue, now),
            Ok(Dispatch::Idle)
        );
        assert_eq!(sink.borrow().applied, [DOWN, UP]);
    }

    #[test]
    fn local_revoke_or_transport_loss_beats_a_full_queue() {
        let now = Instant::now();
        for (control, reason) in [
            (Control::Revoke, End::Revoked),
            (Control::TransportLost, End::Disconnected),
        ] {
            let (mut queue, sink) = allowed(now, Duration::from_secs(10));
            fill_with_releases(&mut queue, now);
            assert_eq!(
                queue.dispatch_at(&SCOPE, control, now),
                Ok(Dispatch::Closed)
            );
            expect_closed(&mut queue, now, reason, &sink);
            queue.advance(Control::Revoke).unwrap();
            drop(queue);
            assert_eq!(sink.borrow().release_calls, 1);
        }
    }

    #[test]
    fn dispatch_on_a_replaced_generation_closes_and_clears_the_old_queue() {
        let now = Instant::now();
        let (mut queue, sink) = allowed(now, Duration::from_secs(10));
        fill_with_releases(&mut queue, now);
        let changed = InputScope {
            generation: [9; 32],
            ..SCOPE
        };
        assert_eq!(
            queue.dispatch_at(&changed, Control::Continue, now),
            Err(ApprovalError::WrongScope.into())
        );
        expect_closed(&mut queue, now, End::Disconnected, &sink);
    }

    #[test]
    fn session_and_consent_deadlines_expire_at_dispatch_or_on_idle_timer() {
        let now = Instant::now();
        for (ttl, elapsed, use_timer) in [(5, 5, false), (120, 60, true)] {
            let (mut queue, sink) = allowed(now, Duration::from_secs(ttl));
            fill_with_releases(&mut queue, now);
            let deadline = now + Duration::from_secs(elapsed);
            assert_eq!(
                queue.advance_at(Control::Continue, deadline - Duration::from_millis(1)),
                Ok(State::Allowed)
            );
            if use_timer {
                assert_eq!(
                    queue.advance_at(Control::Continue, deadline),
                    Ok(State::Ended(End::Expired))
                );
            } else {
                assert_eq!(
                    queue.dispatch_at(&SCOPE, Control::Continue, deadline),
                    Ok(Dispatch::Closed)
                );
            }
            expect_closed(&mut queue, deadline, End::Expired, &sink);
        }

        let (mut idle, sink) = allowed(now, Duration::from_secs(3));
        assert!(idle.pending.is_empty());
        assert_eq!(
            idle.advance_at(Control::Continue, now + Duration::from_secs(3)),
            Ok(State::Ended(End::Expired))
        );
        assert_eq!(sink.borrow().release_calls, 1);
    }

    #[test]
    fn overflowing_a_possible_key_up_closes_clears_and_releases() {
        let now = Instant::now();
        let (mut queue, sink) = allowed(now, Duration::from_secs(10));
        fill_with_releases(&mut queue, now);
        assert_eq!(queue.enqueue_at(&SCOPE, UP, now), Err(Error::Full));
        expect_closed(&mut queue, now, End::QueueOverflow, &sink);
    }

    #[test]
    fn uncertain_release_takes_precedence_on_overflow_or_sink_failure_and_stays_visible() {
        let now = Instant::now();
        for trigger in [End::QueueOverflow, End::SinkFailed] {
            let (mut queue, sink) = allowed(now, Duration::from_secs(10));
            fill_with_releases(&mut queue, now);
            sink.borrow_mut().release_fails = true;
            let uncertain = ApprovalError::ReleaseUnconfirmed.into();
            if trigger == End::QueueOverflow {
                assert_eq!(queue.enqueue_at(&SCOPE, UP, now), Err(uncertain));
            } else {
                sink.borrow_mut().apply_fails = true;
                assert_eq!(
                    queue.dispatch_at(&SCOPE, Control::Continue, now),
                    Err(uncertain)
                );
            }
            assert_eq!(queue.state(), State::Ended(trigger));
            assert!(queue.pending.is_empty());
            assert_eq!(queue.advance(Control::TransportLost), Err(uncertain));
            assert_eq!(queue.advance(Control::Continue), Err(uncertain));
            assert_eq!(queue.enqueue(&SCOPE, DOWN), Err(uncertain));
            assert_eq!(
                queue.dispatch_next(&SCOPE, Control::Continue),
                Err(uncertain)
            );
            assert_eq!(queue.decide_locally(true), Err(uncertain));
            drop(queue);
            assert_eq!(sink.borrow().release_calls, 1);
            assert_eq!(sink.borrow().local, BTreeSet::from([LOCAL_KEY]));
        }
    }
}

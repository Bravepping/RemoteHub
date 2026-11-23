//! Feature-gated serialized input owner for an explicitly approved LAN test.
//! This module does not open connections or authenticate peers.

use crate::{
    input_approval::{Binding, SessionSink, State},
    input_queue::{self, Control, Dispatch, InputQueue},
    remote_input::{Event, Protocol},
    scope::LanScope,
};
use std::{net::Ipv4Addr, time::Instant};
use uuid::Uuid;

/// A fresh native session binding for the legacy LAN path, not a device ID.
/// Construct only after the trusted host verifies the room and the ordered
/// host/viewer LAN pair. This constructor validates shape, not that proof.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LanInputScope {
    room: Uuid,
    generation: Uuid,
    local: Ipv4Addr,
    peer: Ipv4Addr,
}

impl LanInputScope {
    pub fn from_verified_lan(room: Uuid, pair: LanScope) -> Result<Self, Error> {
        if room.is_nil() || LanScope::new(pair.local, pair.peer).is_err() {
            return Err(Error::InvalidScope);
        }
        let mut generation = Uuid::new_v4();
        while generation == room {
            generation = Uuid::new_v4();
        }
        Ok(Self {
            room,
            generation,
            local: pair.local,
            peer: pair.peer,
        })
    }

    #[cfg(windows)]
    pub(super) fn matches_pair(&self, pair: LanScope) -> bool {
        self.local == pair.local && self.peer == pair.peer
    }

    #[cfg(windows)]
    pub(super) fn room(&self) -> Uuid {
        self.room
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    InvalidScope,
    NotAllowed,
    Protocol(String),
    Queue(input_queue::Error),
    InvalidState,
}

impl From<input_queue::Error> for Error {
    fn from(error: input_queue::Error) -> Self {
        Self::Queue(error)
    }
}

/// One validated v1 event may be outstanding. A successful enqueue grants
/// no ACK and reserves no sequence permanently. The serialized owner must
/// service local revoke/transport loss before dispatch, or call `advance`
/// immediately when there is no event. Tests use only fake sinks.
pub struct LanInputQueue<S: SessionSink<Event = Event>> {
    scope: LanInputScope,
    protocol: Protocol,
    queue: InputQueue<S>,
    pending_seq: Option<u64>,
}

impl<S: SessionSink<Event = Event>> LanInputQueue<S> {
    pub fn view_only(scope: LanInputScope, session_until: Instant, sink: S) -> Self {
        Self {
            scope,
            protocol: Protocol::with_deadline(
                scope.room.to_string(),
                Instant::now(),
                session_until,
            ),
            queue: InputQueue::view_only_bound(Binding::LegacyLan(scope), session_until, sink),
            pending_seq: None,
        }
    }

    pub fn state(&self) -> State {
        self.queue.state()
    }

    /// Only for the host's separate local consent after room/pair verification.
    pub fn decide_locally(&mut self, approved: bool) -> Result<(), Error> {
        self.queue.decide_locally(approved)?;
        Ok(())
    }

    /// Returns only acceptance into the local owner, never an ACK. Pass the
    /// binding captured with the incoming session; never relabel an older
    /// generation's bytes as belonging to a newly current session.
    pub fn enqueue_v1(
        &mut self,
        ingress: &LanInputScope,
        bytes: &[u8],
        text: bool,
    ) -> Result<(), Error> {
        if self.advance(Control::Continue)? != State::Allowed {
            return Err(Error::NotAllowed);
        }
        if ingress != &self.scope {
            self.advance(Control::TransportLost)?;
            return Err(Error::InvalidScope);
        }
        if self.pending_seq.take().is_some() {
            // A second in-flight message cannot be silently dropped: it
            // could be a key-up required by an already-applied press.
            self.queue.close_for_overflow()?;
            return Err(input_queue::Error::Full.into());
        }
        let (seq, event) = self
            .protocol
            .validate_event(bytes, text, true, Instant::now())
            .map_err(Error::Protocol)?;
        self.queue
            .enqueue_bound(&Binding::LegacyLan(*ingress), event)?;
        self.pending_seq = Some(seq);
        Ok(())
    }

    /// The returned sequence can be ACKed only after successful sink apply.
    /// None means nothing was applied, including when control revoked input.
    pub fn dispatch_next(
        &mut self,
        current: &LanInputScope,
        control: Control,
    ) -> Result<Option<u64>, Error> {
        let seq = self.pending_seq.take();
        match self
            .queue
            .dispatch_bound(&Binding::LegacyLan(*current), control)?
        {
            Dispatch::Closed | Dispatch::Idle => Ok(None),
            Dispatch::Applied => match seq {
                Some(seq) => Ok(Some(self.protocol.commit_applied(seq))),
                None => {
                    self.advance(Control::Revoke)?;
                    Err(Error::InvalidState)
                }
            },
        }
    }

    /// Poll local revocation again after parsing/enqueueing and immediately
    /// before dispatch. The caller holds its one input-owner mutex throughout.
    #[cfg(any(windows, test))]
    pub(super) fn process_v1(
        &mut self,
        scope: &LanInputScope,
        bytes: &[u8],
        text: bool,
        mut controls: impl FnMut() -> Control,
    ) -> Result<Option<u64>, Error> {
        let control = controls();
        if control != Control::Continue {
            self.advance(control)?;
            return Ok(None);
        }
        self.enqueue_v1(scope, bytes, text)?;
        self.dispatch_next(scope, controls())
    }

    /// Drive on a timer even while idle, and on any local loss/revoke signal.
    pub fn advance(&mut self, control: Control) -> Result<State, Error> {
        let result = self.queue.advance(control);
        if !matches!(result, Ok(State::Allowed)) {
            self.pending_seq = None;
        }
        Ok(result?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_approval::{End, Error as ApprovalError, InputScope, SinkError};
    use std::{cell::RefCell, rc::Rc, time::Duration};

    const ROOM: Uuid = Uuid::from_u128(237);
    const PAIR: LanScope = LanScope {
        local: Ipv4Addr::new(192, 168, 50, 1),
        peer: Ipv4Addr::new(192, 168, 50, 2),
    };
    const DOWN: &str = r#"{"kind":"key","key":"KeyA","down":true}"#;
    const UP: &str = r#"{"kind":"key","key":"KeyA","down":false}"#;

    #[derive(Default)]
    struct Fake {
        attempts: u8,
        pressed: bool,
        released: u8,
        release_calls: u8,
        local_press: bool,
        apply_fails: bool,
        release_fails: bool,
    }

    impl SessionSink for Rc<RefCell<Fake>> {
        type Event = Event;

        fn apply(&mut self, event: &Event) -> Result<(), SinkError> {
            let mut sink = self.borrow_mut();
            sink.attempts += 1;
            if let Event::Key { down, .. } = event {
                // Model an input that might have taken effect before a failure.
                sink.pressed = *down;
            }
            if sink.apply_fails {
                Err(SinkError)
            } else {
                Ok(())
            }
        }

        fn release_owned(&mut self) -> Result<(), SinkError> {
            let mut sink = self.borrow_mut();
            sink.release_calls += 1;
            if sink.release_fails {
                return Err(SinkError);
            }
            sink.released += u8::from(sink.pressed);
            sink.pressed = false;
            Ok(())
        }
    }

    type Fixture = LanInputQueue<Rc<RefCell<Fake>>>;

    fn fixture() -> (LanInputScope, Fixture, Rc<RefCell<Fake>>) {
        let scope = LanInputScope::from_verified_lan(ROOM, PAIR).unwrap();
        let sink = Rc::new(RefCell::new(Fake {
            local_press: true,
            ..Fake::default()
        }));
        let queue = LanInputQueue::view_only(
            scope,
            Instant::now() + Duration::from_secs(20),
            sink.clone(),
        );
        (scope, queue, sink)
    }

    fn wire(room: Uuid, seq: u64, event: &str) -> Vec<u8> {
        format!(r#"{{"v":1,"seq":{seq},"session_id":"{room}","event":{event}}}"#).into_bytes()
    }

    #[test]
    fn view_only_rejects_before_parsing_and_does_not_reserve_first_sequence() {
        let (scope, mut queue, sink) = fixture();
        assert_eq!(
            queue.enqueue_v1(&scope, b"nonsense", false),
            Err(Error::NotAllowed)
        );
        assert_eq!(
            queue.enqueue_v1(&scope, &wire(ROOM, 1, DOWN), true),
            Err(Error::NotAllowed)
        );
        assert_eq!(sink.borrow().attempts, 0);
        queue.decide_locally(true).unwrap();
        queue
            .enqueue_v1(&scope, &wire(ROOM, 1, DOWN), true)
            .unwrap();
        assert_eq!(sink.borrow().attempts, 0);
        assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(Some(1)));
        assert_eq!(sink.borrow().attempts, 1);
    }

    #[test]
    fn parser_rejects_invalid_envelope_skips_and_replay_before_a_second_apply() {
        let (scope, mut queue, sink) = fixture();
        let first = wire(ROOM, 1, DOWN);
        queue.decide_locally(true).unwrap();
        assert_eq!(
            queue.enqueue_v1(&scope, &wire(Uuid::from_u128(8), 1, DOWN), true),
            Err(Error::Protocol("INPUT_SESSION_OR_SEQUENCE".into()))
        );
        assert_eq!(
            queue.enqueue_v1(&scope, &wire(ROOM, 2, UP), true),
            Err(Error::Protocol("INPUT_SESSION_OR_SEQUENCE".into()))
        );
        assert_eq!(
            queue.enqueue_v1(
                &scope,
                &wire(ROOM, 1, r#"{"kind":"key","key":"MetaLeft","down":true}"#),
                true
            ),
            Err(Error::Protocol("INPUT_KEY_NOT_ALLOWED".into()))
        );
        assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(None));
        queue.enqueue_v1(&scope, &first, true).unwrap();
        assert_eq!(sink.borrow().attempts, 0);
        assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(Some(1)));
        assert_eq!(
            queue.enqueue_v1(&scope, &first, true),
            Err(Error::Protocol("INPUT_SESSION_OR_SEQUENCE".into()))
        );
        assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(None));
        assert_eq!(sink.borrow().attempts, 1);
    }

    #[test]
    fn synchronous_owner_resamples_revocation_after_enqueuing_before_any_apply() {
        for cause in [Control::Revoke, Control::TransportLost] {
            let (scope, mut queue, sink) = fixture();
            queue.decide_locally(true).unwrap();
            assert_eq!(
                queue.process_v1(&scope, &wire(ROOM, 1, DOWN), true, || Control::Continue),
                Ok(Some(1))
            );
            let mut samples = [Control::Continue, cause].into_iter();
            assert_eq!(
                queue.process_v1(&scope, &wire(ROOM, 2, UP), true, || samples.next().unwrap()),
                Ok(None)
            );
            assert!(samples.next().is_none());
            assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(None));
            let sink = sink.borrow();
            assert_eq!(sink.attempts, 1);
            assert_eq!(sink.release_calls, 1);
            assert_eq!(sink.released, 1);
            assert!(sink.local_press);
        }
    }

    #[test]
    fn second_offer_while_an_event_is_pending_closes_without_ack() {
        for release_fails in [false, true] {
            let (scope, mut queue, sink) = fixture();
            queue.decide_locally(true).unwrap();
            queue
                .enqueue_v1(&scope, &wire(ROOM, 1, DOWN), true)
                .unwrap();
            assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(Some(1)));
            sink.borrow_mut().release_fails = release_fails;
            let second = wire(ROOM, 2, UP);
            queue.enqueue_v1(&scope, &second, true).unwrap();
            let uncertain = Error::Queue(ApprovalError::ReleaseUnconfirmed.into());
            assert_eq!(
                queue.enqueue_v1(&scope, &second, true),
                Err(if release_fails {
                    uncertain
                } else {
                    Error::Queue(input_queue::Error::Full)
                })
            );
            assert_eq!(queue.state(), State::Ended(End::QueueOverflow));
            if release_fails {
                assert_eq!(
                    queue.dispatch_next(&scope, Control::Continue),
                    Err(Error::Queue(ApprovalError::ReleaseUnconfirmed.into()))
                );
            } else {
                assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(None));
            }
            let sink = sink.borrow();
            assert_eq!(sink.attempts, 1);
            assert_eq!(sink.release_calls, 1);
            assert_eq!(sink.released, u8::from(!release_fails));
            assert!(sink.local_press);
        }
    }

    #[test]
    fn revoke_and_transport_loss_drop_pending_without_ack_and_release_only_owned() {
        for (control, end) in [
            (Control::Revoke, End::Revoked),
            (Control::TransportLost, End::Disconnected),
        ] {
            let (scope, mut queue, sink) = fixture();
            queue.decide_locally(true).unwrap();
            queue
                .enqueue_v1(&scope, &wire(ROOM, 1, DOWN), true)
                .unwrap();
            assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(Some(1)));
            queue.enqueue_v1(&scope, &wire(ROOM, 2, UP), true).unwrap();
            assert_eq!(queue.dispatch_next(&scope, control), Ok(None));
            assert_eq!(queue.state(), State::Ended(end));
            assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(None));
            assert_eq!(
                queue.enqueue_v1(&scope, &wire(ROOM, 2, UP), true),
                Err(Error::NotAllowed)
            );
            drop(queue);
            let sink = sink.borrow();
            assert_eq!(sink.attempts, 1);
            assert_eq!(sink.release_calls, 1);
            assert_eq!(sink.released, 1);
            assert!(sink.local_press);
        }
    }

    #[test]
    fn failed_application_never_commits_ack_and_release_uncertainty_stays_visible() {
        for release_fails in [false, true] {
            let (scope, mut queue, sink) = fixture();
            sink.borrow_mut().apply_fails = true;
            sink.borrow_mut().release_fails = release_fails;
            queue.decide_locally(true).unwrap();
            queue
                .enqueue_v1(&scope, &wire(ROOM, 1, DOWN), true)
                .unwrap();
            let error = if release_fails {
                ApprovalError::ReleaseUnconfirmed
            } else {
                ApprovalError::ApplyFailed
            };
            assert_eq!(
                queue.dispatch_next(&scope, Control::Continue),
                Err(Error::Queue(error.into()))
            );
            assert_eq!(queue.state(), State::Ended(End::SinkFailed));
            assert_eq!(sink.borrow().attempts, 1);
            if release_fails {
                assert_eq!(
                    queue.advance(Control::Revoke),
                    Err(Error::Queue(error.into()))
                );
            } else {
                assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(None));
            }
        }
    }

    #[test]
    fn legacy_scope_is_exactly_room_generation_and_ordered_lan_pair_not_a_device_key() {
        let invalid = LanScope {
            peer: Ipv4Addr::LOCALHOST,
            ..PAIR
        };
        assert!(matches!(
            LanInputScope::from_verified_lan(ROOM, invalid),
            Err(Error::InvalidScope)
        ));
        assert!(matches!(
            LanInputScope::from_verified_lan(Uuid::nil(), PAIR),
            Err(Error::InvalidScope)
        ));
        let (scope, queue, sink) = fixture();
        let new_scope = LanInputScope::from_verified_lan(ROOM, PAIR).unwrap();
        assert!(scope != new_scope);

        for changed in [
            LanInputScope {
                room: Uuid::from_u128(13),
                ..scope
            },
            LanInputScope {
                generation: new_scope.generation,
                ..scope
            },
            LanInputScope {
                local: scope.peer,
                peer: scope.local,
                ..scope
            },
        ] {
            let mut queue = LanInputQueue::view_only(
                scope,
                Instant::now() + Duration::from_secs(20),
                sink.clone(),
            );
            queue.decide_locally(true).unwrap();
            queue
                .enqueue_v1(&scope, &wire(ROOM, 1, DOWN), true)
                .unwrap();
            assert_eq!(
                queue.dispatch_next(&changed, Control::Continue),
                Err(Error::Queue(ApprovalError::WrongScope.into()))
            );
            assert_eq!(queue.state(), State::Ended(End::Disconnected));
            assert_eq!(queue.dispatch_next(&scope, Control::Continue), Ok(None));
        }

        let mut new_owner = LanInputQueue::view_only(
            new_scope,
            Instant::now() + Duration::from_secs(20),
            sink.clone(),
        );
        new_owner.decide_locally(true).unwrap();
        assert_eq!(
            new_owner.enqueue_v1(&scope, &wire(ROOM, 1, DOWN), true),
            Err(Error::InvalidScope)
        );
        assert_eq!(
            new_owner.dispatch_next(&new_scope, Control::Continue),
            Ok(None)
        );

        let mut device_queue = InputQueue::view_only(
            InputScope {
                room: *scope.room.as_bytes(),
                generation: [2; 32],
                viewer_key: [3; 32],
            },
            Instant::now() + Duration::from_secs(20),
            sink.clone(),
        );
        device_queue.decide_locally(true).unwrap();
        assert_eq!(
            device_queue.enqueue_bound(&Binding::LegacyLan(scope), Event::Release {}),
            Err(ApprovalError::WrongScope.into())
        );
        assert_eq!(queue.state(), State::ViewOnly);
        assert_eq!(sink.borrow().attempts, 0);
    }
}

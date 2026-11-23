//! Local acceptance fixture, not a production queue or input transport.
use super::*;
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};
use tokio::sync::{mpsc, watch};

const SCOPE: InputScope = InputScope {
    room: [1; 16],
    generation: [2; 32],
    viewer_key: [3; 32],
};
const LOCAL_KEY: u8 = 1;
const SESSION_KEY: u8 = 2;
const QUEUED_KEY: u8 = 3;
const CAPACITY: usize = 4;

#[derive(Default)]
struct SimulatedKeys {
    local_keys: BTreeSet<u8>,
    session_keys: BTreeSet<u8>,
    applied: Vec<u8>,
    released: Vec<u8>,
}

impl SessionSink for Rc<RefCell<SimulatedKeys>> {
    type Event = u8;

    fn apply(&mut self, key: &u8) -> Result<(), SinkError> {
        let mut keys = self.borrow_mut();
        keys.session_keys.insert(*key);
        keys.applied.push(*key);
        Ok(())
    }

    fn release_owned(&mut self) -> Result<(), SinkError> {
        let mut keys = self.borrow_mut();
        let owned = std::mem::take(&mut keys.session_keys);
        keys.released.extend(owned);
        Ok(())
    }
}

type Gate = Approval<Rc<RefCell<SimulatedKeys>>>;

/// Models one serialized owner turn. This intentionally uses an actual
/// bounded queue and ready control messages, but never an operating system
/// sink. The timer must be driven even while no event arrives.
async fn owner_step(
    gate: &mut Gate,
    until: Instant,
    cancel: &mut watch::Receiver<Option<End>>,
    events: &mut mpsc::Receiver<u8>,
) -> Result<bool, Error> {
    let proceed = tokio::select! {
        biased;
        change = cancel.wait_for(Option::is_some) => {
            let signal = change.ok().and_then(|current| *current);
            if signal == Some(End::Revoked) {
                gate.revoke()
            } else {
                gate.disconnect()
            }.map(|()| false)
        }
        _ = tokio::time::sleep_until(until.into()) => {
            gate.advance(Instant::now()).map(|()| false)
        }
        key = events.recv() => match key {
            Some(key) => match gate.apply(&SCOPE, &key, Instant::now()) {
                Err(Error::NotAllowed) if gate.state() == State::Ended(End::Expired) => Ok(false),
                result => result.map(|()| true),
            },
            None => gate.disconnect().map(|()| false)
        }
    };
    if !matches!(proceed, Ok(true)) {
        events.close();
        while events.try_recv().is_ok() {}
    }
    proceed
}

fn approved_fixture(
    now: Instant,
    until: Instant,
) -> (
    Gate,
    Rc<RefCell<SimulatedKeys>>,
    mpsc::Sender<u8>,
    mpsc::Receiver<u8>,
) {
    let keys = Rc::new(RefCell::new(SimulatedKeys::default()));
    keys.borrow_mut().local_keys.insert(LOCAL_KEY);
    let mut gate = Approval::view_only(SCOPE, until, keys.clone());
    gate.decide_locally(true, now).unwrap();
    gate.apply(&SCOPE, &SESSION_KEY, now).unwrap();
    let (tx, rx) = mpsc::channel(CAPACITY);
    for _ in 0..CAPACITY {
        tx.try_send(QUEUED_KEY).unwrap();
    }
    assert!(matches!(
        tx.try_send(QUEUED_KEY),
        Err(mpsc::error::TrySendError::Full(_))
    ));
    (gate, keys, tx, rx)
}

fn expect_drained(
    gate: &Gate,
    reason: End,
    keys: &RefCell<SimulatedKeys>,
    tx: &mpsc::Sender<u8>,
    rx: &mut mpsc::Receiver<u8>,
) {
    assert_eq!(gate.state(), State::Ended(reason));
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    assert!(matches!(
        tx.try_send(QUEUED_KEY),
        Err(mpsc::error::TrySendError::Closed(_))
    ));
    let keys = keys.borrow();
    assert_eq!(keys.applied, [SESSION_KEY]);
    assert_eq!(keys.released, [SESSION_KEY]);
    assert!(keys.session_keys.is_empty());
    assert_eq!(keys.local_keys, BTreeSet::from([LOCAL_KEY]));
}

#[tokio::test]
async fn same_queue_is_consumed_when_authorized_and_not_cancelled() {
    let now = Instant::now();
    let until = now + Duration::from_secs(10);
    let (mut gate, keys, tx, mut rx) = approved_fixture(now, until);
    let (cancel, mut signal) = watch::channel(None);
    assert_eq!(
        owner_step(&mut gate, until, &mut signal, &mut rx).await,
        Ok(true)
    );
    assert_eq!(keys.borrow().applied, [SESSION_KEY, QUEUED_KEY]);
    assert_eq!(rx.len(), CAPACITY - 1);
    assert!(!tx.is_closed());
    drop(cancel);
}

#[tokio::test]
async fn ready_revoke_beats_a_full_input_queue() {
    let now = Instant::now();
    let until = now + Duration::from_secs(10);
    let (mut gate, keys, tx, mut rx) = approved_fixture(now, until);
    let (cancel, mut signal) = watch::channel(Some(End::Revoked));
    assert_eq!(
        owner_step(&mut gate, until, &mut signal, &mut rx).await,
        Ok(false)
    );
    expect_drained(&gate, End::Revoked, &keys, &tx, &mut rx);
    drop(cancel);
}

#[tokio::test]
async fn control_disconnect_beats_a_full_input_queue() {
    let now = Instant::now();
    let until = now + Duration::from_secs(10);
    let (mut gate, keys, tx, mut rx) = approved_fixture(now, until);
    let (cancel, mut signal) = watch::channel(None);
    drop(cancel);
    assert_eq!(
        owner_step(&mut gate, until, &mut signal, &mut rx).await,
        Ok(false)
    );
    expect_drained(&gate, End::Disconnected, &keys, &tx, &mut rx);
}

#[tokio::test]
async fn expired_approval_discards_events_queued_before_deadline() {
    let until = Instant::now();
    let (mut gate, keys, tx, mut rx) = approved_fixture(until - Duration::from_secs(1), until);
    let (cancel, mut signal) = watch::channel(None);
    assert_eq!(
        owner_step(&mut gate, until, &mut signal, &mut rx).await,
        Ok(false)
    );
    expect_drained(&gate, End::Expired, &keys, &tx, &mut rx);
    drop(cancel);
}

//! Fixed in-process fixtures over real IPv4 loopback TCP. The bytes after
//! proof are synthetic test markers, not a product input protocol.

use super::*;
use crate::{
    input_approval::{End, Error, InputScope, SessionSink, SinkError, State},
    input_queue::{Control, Dispatch, InputQueue},
};
use hub_identity::session::CHALLENGE_BYTES;
use std::{
    cell::Cell,
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket, TcpStream},
    sync::oneshot,
    time::timeout,
};

const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;
const WAIT: Duration = Duration::from_secs(3);
const PRE_PROMPT: u8 = 0x10;
const DURING_PROMPT: u8 = 0x11;
const SYNTHETIC_HOLD: u8 = 0x12;
const SYNTHETIC_QUEUED: u8 = 0x13;
const ACK_HOLD: u8 = 0x20;

#[derive(Clone, Copy)]
enum Marker {
    Hold,
    Clear,
}

#[derive(Default)]
struct FakeKeys {
    local_held: Cell<bool>,
    fixture_held: Cell<bool>,
    applied: Cell<u8>,
    released: Cell<u8>,
}

impl SessionSink for &FakeKeys {
    type Event = Marker;

    fn apply(&mut self, event: &Marker) -> Result<(), SinkError> {
        self.applied.set(self.applied.get() + 1);
        self.fixture_held.set(matches!(event, Marker::Hold));
        Ok(())
    }

    fn release_owned(&mut self) -> Result<(), SinkError> {
        if self.fixture_held.replace(false) {
            self.released.set(self.released.get() + 1);
        }
        Ok(())
    }
}

async fn host_proof(
    stream: &mut TcpStream,
    proof: &mut PublicVideoProof<'_>,
    offer: &str,
    answer: &str,
) -> Result<(), &'static str> {
    stream
        .write_all(&proof.challenge().unwrap().to_bytes())
        .await
        .unwrap();
    let mut challenge = [0; CHALLENGE_BYTES];
    let mut signature = [0; 64];
    stream.read_exact(&mut challenge).await.unwrap();
    stream.read_exact(&mut signature).await.unwrap();
    proof.verify_video_sdp(offer, &signature, NOW)?;
    let challenge = SessionChallenge::from_bytes(challenge).unwrap();
    let (signature, _) = proof.seal_video_sdp(challenge, answer, NOW)?;
    stream.write_all(&signature).await.unwrap();
    Ok(())
}

async fn send_viewer_proof(stream: &mut TcpStream, proof: &mut PublicVideoProof<'_>, offer: &str) {
    let mut challenge = [0; CHALLENGE_BYTES];
    stream.read_exact(&mut challenge).await.unwrap();
    let challenge = SessionChallenge::from_bytes(challenge).unwrap();
    let (signature, _) = proof.seal_video_sdp(challenge, offer, NOW).unwrap();
    stream
        .write_all(&proof.challenge().unwrap().to_bytes())
        .await
        .unwrap();
    stream.write_all(&signature).await.unwrap();
}

async fn accept_host(listener: TcpListener, endpoint: SocketAddr) -> TcpStream {
    let (stream, observed_source) = listener.accept().await.unwrap();
    assert_eq!(stream.local_addr().unwrap(), endpoint);
    assert_eq!(stream.peer_addr().unwrap(), observed_source);
    assert_eq!(observed_source.ip(), LOOPBACK);
    stream
}

async fn connect_viewer(endpoint: SocketAddr) -> TcpStream {
    assert_eq!(endpoint.ip(), LOOPBACK);
    let socket = TcpSocket::new_v4().unwrap();
    socket.bind((LOOPBACK, 0).into()).unwrap();
    let stream = socket.connect(endpoint).await.unwrap();
    assert_eq!(stream.local_addr().unwrap().ip(), LOOPBACK);
    assert_eq!(stream.peer_addr().unwrap(), endpoint);
    stream
}

fn expect_no_pending_bytes(stream: &TcpStream) {
    let mut unexpected = [0; 1];
    assert!(matches!(
        stream.try_read(&mut unexpected),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}

#[tokio::test]
async fn synthetic_input_over_loopback_acks_only_applied_and_eof_releases_owned() {
    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    let room = Uuid::new_v4();
    let generation = [9; 32];
    let policy = scope();
    let offer = video_offer(policy);
    let answer = offer.replace("a=recvonly", "a=sendonly");
    let mut host_proof_state = PublicVideoProof::prepare(
        &host,
        &pin(&host, &viewer),
        SessionRole::Host,
        room,
        generation,
        policy,
        NOW,
    )
    .unwrap();
    let mut viewer_proof_state = PublicVideoProof::prepare(
        &viewer,
        &pin(&viewer, &host),
        SessionRole::Viewer,
        room,
        generation,
        policy,
        NOW,
    )
    .unwrap();
    let scoped = InputScope {
        room: *room.as_bytes(),
        generation,
        viewer_key: viewer.public().public_key,
    };
    let replaced = InputScope {
        generation: [8; 32],
        ..scoped
    };

    let (offer, answer) = (offer.as_str(), answer.as_str());
    let listener = TcpListener::bind((LOOPBACK, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let keys = FakeKeys::default();
    keys.local_held.set(true);
    let (prompt_open, viewer_prompt_open) = oneshot::channel();
    let (viewer_pending_checked, operator_pending_checked) = oneshot::channel();
    let (pending_denied, viewer_pending_denied) = oneshot::channel();
    let (native_answer, host_native_answer) = oneshot::channel();
    let (allowed, viewer_allowed) = oneshot::channel();

    let shared = &keys;
    let host_task = async move {
        let mut stream = accept_host(listener, addr).await;
        host_proof(&mut stream, &mut host_proof_state, offer, answer)
            .await
            .unwrap();
        let mut queue = InputQueue::view_only(scoped, Instant::now() + WAIT, shared);
        assert_eq!(stream.read_u8().await.unwrap(), PRE_PROMPT);
        assert_eq!(
            queue.enqueue(&scoped, Marker::Hold),
            Err(Error::NotAllowed.into())
        );
        let prompt = queue.begin_local_decision(&scoped).unwrap();
        prompt_open.send(()).unwrap();

        assert_eq!(stream.read_u8().await.unwrap(), DURING_PROMPT);
        assert_eq!(
            queue.enqueue(&scoped, Marker::Hold),
            Err(Error::NotAllowed.into())
        );
        assert_eq!(shared.applied.get(), 0);
        pending_denied.send(()).unwrap();
        queue
            .complete_local_decision(
                prompt,
                &scoped,
                host_native_answer.await.unwrap(),
                Control::Continue,
            )
            .unwrap();
        assert_eq!(queue.state(), State::Allowed);
        assert_eq!(
            queue.enqueue(&replaced, Marker::Hold),
            Err(Error::WrongScope.into())
        );
        allowed.send(()).unwrap();

        assert_eq!(stream.read_u8().await.unwrap(), SYNTHETIC_HOLD);
        queue.enqueue(&scoped, Marker::Hold).unwrap();
        assert_eq!(shared.applied.get(), 0, "enqueue is not acknowledgement");
        assert_eq!(
            queue.dispatch_next(&scoped, Control::Continue),
            Ok(Dispatch::Applied)
        );
        assert!(shared.fixture_held.get());
        stream.write_u8(ACK_HOLD).await.unwrap();

        assert_eq!(stream.read_u8().await.unwrap(), SYNTHETIC_QUEUED);
        queue.enqueue(&scoped, Marker::Clear).unwrap();
        assert_eq!(shared.applied.get(), 1);
        assert_eq!(
            stream.read(&mut [0; 1]).await.unwrap(),
            0,
            "observed TCP EOF"
        );
        assert_eq!(
            queue.dispatch_next(&scoped, Control::TransportLost),
            Ok(Dispatch::Closed)
        );
        assert_eq!(queue.state(), State::Ended(End::Disconnected));
        assert_eq!(
            queue.dispatch_next(&scoped, Control::Continue),
            Ok(Dispatch::Closed)
        );
        assert_eq!(
            queue.enqueue(&scoped, Marker::Hold),
            Err(Error::NotAllowed.into())
        );
        stream.shutdown().await.unwrap();
    };

    let viewer_task = async move {
        let mut stream = connect_viewer(addr).await;
        send_viewer_proof(&mut stream, &mut viewer_proof_state, offer).await;
        let mut answer_signature = [0; 64];
        stream.read_exact(&mut answer_signature).await.unwrap();
        viewer_proof_state
            .verify_video_sdp(answer, &answer_signature, NOW)
            .unwrap();

        stream.write_u8(PRE_PROMPT).await.unwrap();
        viewer_prompt_open.await.unwrap();
        expect_no_pending_bytes(&stream);
        stream.write_u8(DURING_PROMPT).await.unwrap();
        viewer_pending_denied.await.unwrap();
        expect_no_pending_bytes(&stream);
        viewer_pending_checked.send(()).unwrap();
        viewer_allowed.await.unwrap();
        stream.write_u8(SYNTHETIC_HOLD).await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), ACK_HOLD);

        stream.write_u8(SYNTHETIC_QUEUED).await.unwrap();
        stream.shutdown().await.unwrap();
        assert_eq!(
            stream.read(&mut [0; 1]).await.unwrap(),
            0,
            "queued event must not generate a late ACK after EOF"
        );
    };

    // A separate in-process simulated native owner resolves the prompt only
    // after both preapproval markers were rejected. It never reads TCP.
    let operator_task = async {
        operator_pending_checked.await.unwrap();
        assert_eq!(keys.applied.get(), 0);
        native_answer.send(true).unwrap();
    };
    timeout(WAIT, async {
        tokio::join!(host_task, viewer_task, operator_task)
    })
    .await
    .unwrap();
    assert_eq!(keys.applied.get(), 1);
    assert_eq!(keys.released.get(), 1, "release exactly the held fake key");
    assert!(!keys.fixture_held.get());
    assert!(keys.local_held.get());
}

#[tokio::test]
async fn synthetic_input_loopback_rejects_unpinned_signer_before_prompt() {
    let host = Identity::generate_ephemeral().unwrap();
    let pinned_viewer = Identity::generate_ephemeral().unwrap();
    let other = Identity::generate_ephemeral().unwrap();
    let room = Uuid::new_v4();
    let policy = scope();
    let offer = video_offer(policy);
    let answer = offer.replace("a=recvonly", "a=sendonly");
    let mut h = PublicVideoProof::prepare(
        &host,
        &pin(&host, &pinned_viewer),
        SessionRole::Host,
        room,
        [3; 32],
        policy,
        NOW,
    )
    .unwrap();
    let mut v = PublicVideoProof::prepare(
        &other,
        &pin(&other, &host),
        SessionRole::Viewer,
        room,
        [3; 32],
        policy,
        NOW,
    )
    .unwrap();
    let listener = TcpListener::bind((LOOPBACK, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    timeout(WAIT, async {
        tokio::join!(
            async {
                let mut stream = accept_host(listener, addr).await;
                assert_eq!(
                    host_proof(&mut stream, &mut h, &offer, &answer).await,
                    Err(REJECTED)
                );
                assert!(h.challenge().is_none());
                stream.shutdown().await.unwrap();
            },
            async {
                let mut stream = connect_viewer(addr).await;
                send_viewer_proof(&mut stream, &mut v, &offer).await;
                assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            }
        );
    })
    .await
    .unwrap();
}

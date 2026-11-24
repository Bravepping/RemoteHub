//! Real loopback-only WSS and TURN ICE/DTLS/SCTP with a fake input sink.
//! Public-looking relay tuples are only labels; neither path leaves loopback.
use crate::{
    ice_policy::IceMode,
    input_approval::{End, Error as ApprovalError, InputScope, SessionSink, SinkError, State},
    input_queue::{Control, Dispatch, Error as QueueError, InputQueue},
    pinned_runtime::PinnedRuntime,
    public_fake_input::{
        Error as ProofError, FakeInputProof, FakeInputProofScope, FakeInputTestMode,
    },
    public_fake_input_profile::{check_pair, Error as ProfileError, SyntheticProfile},
    public_fake_input_sdp_fixture::sanitize,
    public_fake_input_wss_fixture::{FixtureSocket, FixtureWss},
    public_fake_input_wss_wire::{self as wire, Context as WireContext, Frame, Kind},
    public_video::PublicVideoScope,
    relay_runtime::RelayRuntime,
};
use hub_identity::{
    pairing::{PairingCandidate, VerifiedPeer},
    session::SessionRole,
    Identity,
};
use rtc::{
    ice::{mdns::MulticastDnsMode, network_type::NetworkType},
    peer_connection::transport::{
        RTCDtlsTransportState, RTCIceTransportState, RTCSctpTransportState,
    },
};
use std::{
    cell::Cell,
    future::Future,
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, UdpSocket},
    pin::Pin,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceGatheringState, RTCIceTransportPolicy, RTCSessionDescription, SettingEngineBuilder,
    },
    runtime::{
        AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, Runtime,
    },
};

#[path = "public_video_turn_fixture_net.rs"]
#[allow(dead_code)]
mod turn_fixture_net;
use turn_fixture_net::{FixtureTurn, RELAY_LABEL, RELAY_PORTS};

const LABEL: &str = "remotehub-turn-fiction";
const PROTOCOL: &str = "remotehub.fixture";
const WAIT: Duration = Duration::from_secs(12);
const DENIED: QueueError = QueueError::Approval(ApprovalError::NotAllowed);
const WSS_CLOSED: &str = "FAKE_INPUT_FIXTURE_WSS_CLOSED";
const TEST_MODE: FakeInputTestMode = FakeInputTestMode::Enabled;

#[derive(Debug)]
struct FixtureRuntime {
    physical: RelayRuntime,
    peer: Mutex<Option<SocketAddrV4>>,
}

impl FixtureRuntime {
    fn new(turn: &FixtureTurn) -> Self {
        Self {
            physical: RelayRuntime {
                base: PinnedRuntime {
                    inner: webrtc::runtime::default_runtime().unwrap(),
                    local: Ipv4Addr::LOCALHOST.into(),
                    peer: Ipv4Addr::LOCALHOST.into(),
                    counters: Arc::default(),
                },
                endpoint: turn.server().into(),
                deadline: Instant::now() + WAIT,
                stopped: Arc::new(AtomicBool::new(false)),
            },
            peer: Mutex::new(None),
        }
    }

    fn admit(&self, peer: SocketAddrV4) {
        assert_eq!(*peer.ip(), RELAY_LABEL);
        assert!((RELAY_PORTS.0..=RELAY_PORTS.1).contains(&peer.port()));
        assert!(self.peer.lock().unwrap().replace(peer).is_none());
    }

    fn revoke(&self) {
        self.physical.stopped.store(true, Ordering::Release);
        if let Ok(mut peer) = self.peer.lock() {
            peer.take();
        }
    }
}

impl Runtime for FixtureRuntime {
    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        self.physical.spawn(task)
    }
    fn wrap_udp_socket(&self, socket: UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        self.physical.wrap_udp_socket(socket)
    }
    fn wrap_tcp_listener(&self, socket: TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        self.physical.wrap_tcp_listener(socket)
    }
    fn connect_tcp(
        &self,
        address: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + '_>> {
        self.physical.connect_tcp(address)
    }
    fn resolve_host<'a>(
        &'a self,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        self.physical.resolve_host(name)
    }
    fn interval(&self, delay: Duration) -> Box<dyn AsyncInterval> {
        self.physical.interval(delay)
    }
    fn sleep(&self, delay: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.physical.sleep(delay)
    }
    fn block_on(&self, task: Pin<Box<dyn Future<Output = ()> + '_>>) {
        self.physical.block_on(task)
    }
    fn requires_exact_turn_source(&self) -> bool {
        true
    }
    fn allows_turn_peer(&self, peer: SocketAddr) -> bool {
        !self.physical.stopped.load(Ordering::Acquire)
            && Instant::now() < self.physical.deadline
            && self
                .peer
                .lock()
                .ok()
                .and_then(|admitted| *admitted)
                .map(SocketAddr::V4)
                == Some(peer)
    }
}

struct Events {
    gathered: watch::Sender<bool>,
    channels: mpsc::Sender<Arc<dyn DataChannel>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.send_replace(true);
        }
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        self.channels
            .try_send(channel)
            .expect("one fixture channel");
    }
}

struct Peer {
    pc: Box<dyn PeerConnection>,
    runtime: Arc<FixtureRuntime>,
    gathered: watch::Receiver<bool>,
    channels: mpsc::Receiver<Arc<dyn DataChannel>>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.runtime.revoke();
    }
}

impl Peer {
    async fn start(turn: &FixtureTurn) -> Self {
        let runtime = Arc::new(FixtureRuntime::new(turn));
        let (gather_tx, gathered) = watch::channel(false);
        let (channel_tx, channels) = mpsc::channel(1);
        let pc = PeerConnectionBuilder::new()
            .with_configuration(
                RTCConfigurationBuilder::default()
                    .with_ice_servers(vec![turn.ice_server()])
                    .with_ice_transport_policy(RTCIceTransportPolicy::Relay)
                    .build(),
            )
            .with_host_candidates(false)
            .with_udp_addrs(vec![SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)])
            .with_setting_engine(
                SettingEngineBuilder::new()
                    .with_multicast_dns_mode(MulticastDnsMode::Disabled)
                    .with_network_types(vec![NetworkType::Udp4])
                    .build(),
            )
            .with_runtime(runtime.clone())
            .with_handler(Arc::new(Events {
                gathered: gather_tx,
                channels: channel_tx,
            }))
            .build()
            .await
            .unwrap();
        Self {
            pc: Box::new(pc),
            runtime,
            gathered,
            channels,
        }
    }

    async fn local(&mut self, answer: bool) -> String {
        let initial = if answer {
            self.pc.create_answer(None).await
        } else {
            self.pc.create_offer(None).await
        }
        .unwrap();
        self.pc.set_local_description(initial).await.unwrap();
        self.gathered.wait_for(|gathered| *gathered).await.unwrap();
        let actual = self.pc.local_description().await.unwrap();
        sanitize(
            &actual.sdp,
            RELAY_LABEL,
            RELAY_PORTS.0,
            RELAY_PORTS.1,
            answer,
        )
        .unwrap_or_else(|error| panic!("native candidate policy rejected fixture SDP: {error}"))
    }

    async fn remote(&self, sdp: &str, answer: bool, verified_peer: SocketAddrV4) {
        self.runtime.admit(verified_peer);
        let desc = if answer {
            RTCSessionDescription::answer(sdp.to_owned())
        } else {
            RTCSessionDescription::offer(sdp.to_owned())
        }
        .unwrap();
        self.pc.set_remote_description(desc).await.unwrap();
    }

    async fn verify_pair(&self, proof: &mut FakeInputProof<'_>) {
        let sctp = self.pc.sctp().await.unwrap();
        let dtls = sctp.transport();
        let ice = dtls.ice_transport();
        assert_eq!(
            sctp.state().await.unwrap(),
            RTCSctpTransportState::Connected
        );
        assert_eq!(
            dtls.state().await.unwrap(),
            RTCDtlsTransportState::Connected
        );
        assert!(matches!(
            ice.state().await.unwrap(),
            RTCIceTransportState::Connected | RTCIceTransportState::Completed
        ));
        let actual = ice.get_selected_candidate_pair().await.unwrap();
        assert_eq!(
            sctp.state().await.unwrap(),
            RTCSctpTransportState::Connected
        );
        assert_eq!(
            dtls.state().await.unwrap(),
            RTCDtlsTransportState::Connected
        );
        assert!(matches!(
            ice.state().await.unwrap(),
            RTCIceTransportState::Connected | RTCIceTransportState::Completed
        ));
        proof
            .verify_selected_relay_pair(actual.as_ref(), wall())
            .unwrap();
    }
}

fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
    let expected = peer.public();
    local
        .verify_peer(
            &PairingCandidate::parse(&expected.pairing_code()).unwrap(),
            &expected.fingerprint,
        )
        .unwrap()
}

fn wall() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn opened(channel: &Arc<dyn DataChannel>) {
    while let Some(event) = channel.poll().await {
        match event {
            DataChannelEvent::OnOpen => return,
            DataChannelEvent::OnClose | DataChannelEvent::OnError => break,
            _ => (),
        }
    }
    panic!("sealed fixture channel did not open");
}

async fn text(channel: &Arc<dyn DataChannel>, next: &str) {
    loop {
        match channel.poll().await {
            Some(DataChannelEvent::OnMessage(message)) => {
                assert!(message.is_string);
                assert_eq!(message.data.as_ref(), next.as_bytes());
                return;
            }
            Some(
                DataChannelEvent::OnBufferedAmountHigh | DataChannelEvent::OnBufferedAmountLow,
            ) => (),
            _ => panic!("sealed fixture channel closed before marker: {next}"),
        }
    }
}

async fn no_ack(channel: &Arc<dyn DataChannel>) {
    let unexpected = tokio::time::timeout(Duration::from_millis(80), async {
        loop {
            match channel.poll().await {
                Some(
                    DataChannelEvent::OnBufferedAmountHigh | DataChannelEvent::OnBufferedAmountLow,
                ) => (),
                other => return other,
            }
        }
    })
    .await;
    assert!(
        unexpected.is_err(),
        "denied marker produced an ACK or terminal event"
    );
}

#[derive(Default)]
struct Fake {
    applied: Cell<u8>,
    pressed: Cell<bool>,
    released: Cell<u8>,
    release_calls: Cell<u8>,
}

impl SessionSink for Rc<Fake> {
    type Event = ();
    fn apply(&mut self, _: &()) -> Result<(), SinkError> {
        self.applied.set(self.applied.get() + 1);
        assert!(!self.pressed.replace(true));
        Ok(())
    }
    fn release_owned(&mut self) -> Result<(), SinkError> {
        self.release_calls.set(self.release_calls.get() + 1);
        if self.pressed.replace(false) {
            self.released.set(self.released.get() + 1);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Approved,
    StopBeforeConsent,
    TamperedOffer,
    DisconnectWhilePressed,
    MismatchedGeneration,
}

#[cfg(feature = "v2-public-fake-input-loopback-check")]
pub async fn run_wss_turn_loopback_check() {
    exercise(Case::Approved).await;
    println!("WSS_TURN_SYNTHETIC_ACK_PASS");
    exercise(Case::StopBeforeConsent).await;
    println!("WSS_TURN_DISCONNECT_PENDING_PASS");
    exercise(Case::TamperedOffer).await;
    println!("WSS_TURN_TAMPER_REJECTED_PASS");
    exercise(Case::DisconnectWhilePressed).await;
    println!("WSS_TURN_DISCONNECT_ACTIVE_PASS");
}

#[cfg(test)]
#[tokio::test]
async fn signed_wss_turn_sctp_applies_only_one_approved_fixture_marker() {
    exercise(Case::Approved).await;
}

#[cfg(test)]
#[tokio::test]
async fn wss_loss_before_consent_prevents_late_yes_and_ack() {
    exercise(Case::StopBeforeConsent).await;
}

#[cfg(test)]
#[tokio::test]
async fn wss_delivered_tampered_offer_never_admits_a_relay_peer() {
    exercise(Case::TamperedOffer).await;
}

#[cfg(test)]
#[tokio::test]
async fn wss_loss_after_ack_releases_press_and_prevents_queued_dispatch() {
    exercise(Case::DisconnectWhilePressed).await;
}

#[cfg(test)]
#[tokio::test]
async fn mismatched_profiles_stop_before_wss_frames_or_turn_allocation() {
    exercise(Case::MismatchedGeneration).await;
}

struct Negotiated<'a> {
    peer: Peer,
    proof: FakeInputProof<'a>,
}

enum HostNegotiation<'a> {
    Ready(Box<Negotiated<'a>>),
    RejectedOffer,
}

enum ViewerNegotiation<'a> {
    Ready(Box<Negotiated<'a>>, Arc<dyn DataChannel>),
    HostClosed(Peer),
}

async fn negotiate_host<'a>(
    wss: &mut FixtureSocket,
    turn: &FixtureTurn,
    id: &'a Identity,
    pinned_viewer: &VerifiedPeer,
    scope: FakeInputProofScope,
) -> HostNegotiation<'a> {
    let mut proof = FakeInputProof::prepare(
        id,
        pinned_viewer,
        SessionRole::Host,
        scope,
        TEST_MODE,
        wall(),
    )
    .unwrap();
    let ctx = WireContext {
        room: scope.room,
        generation: scope.generation,
    };
    let challenge = wire::encode(
        ctx,
        Kind::HostChallenge,
        Frame::Challenge(proof.challenge(wall()).unwrap()),
    )
    .unwrap();
    wss.send(&challenge).await.unwrap();

    let inbound = wss.recv().await.unwrap();
    let Frame::Challenge(viewer_challenge) =
        wire::decode(ctx, Kind::ViewerChallenge, &inbound).unwrap()
    else {
        panic!("expected viewer challenge over WSS")
    };
    let inbound = wss.recv().await.unwrap();
    let Frame::SignedSdp {
        sdp: offer,
        signature,
    } = wire::decode(ctx, Kind::Offer, &inbound).unwrap()
    else {
        panic!("expected signed offer over WSS")
    };
    // Reuse the exact shape checker so this negative control cannot pass
    // merely by delivering malformed SDP. A valid tuple alone authorizes nothing.
    let viewer_relay = scope
        .validate_relay_sctp_description(offer, SessionRole::Viewer, wall())
        .unwrap();
    match proof.verify(offer, &signature, wall()) {
        Ok(()) => (),
        Err(ProofError::Rejected) => {
            assert!(proof.approved_relay_pair(wall()).is_none());
            wss.disconnect().await.unwrap();
            return HostNegotiation::RejectedOffer;
        }
        Err(error) => panic!("unexpected offer proof failure: {error:?}"),
    }

    // No host RTC, remote description or relay admission exists before proof.
    let mut peer = Peer::start(turn).await;
    peer.remote(offer, false, viewer_relay).await;
    let answer = peer.local(true).await;
    let signature = proof.seal(viewer_challenge, &answer, wall()).unwrap();
    let answer_frame = wire::encode(
        ctx,
        Kind::Answer,
        Frame::SignedSdp {
            sdp: &answer,
            signature,
        },
    )
    .unwrap();
    wss.send(&answer_frame).await.unwrap();
    HostNegotiation::Ready(Box::new(Negotiated { peer, proof }))
}

async fn negotiate_viewer<'a>(
    wss: &mut FixtureSocket,
    turn: &FixtureTurn,
    id: &'a Identity,
    pinned_host: &VerifiedPeer,
    scope: FakeInputProofScope,
    tamper_offer: bool,
) -> ViewerNegotiation<'a> {
    let mut proof = FakeInputProof::prepare(
        id,
        pinned_host,
        SessionRole::Viewer,
        scope,
        TEST_MODE,
        wall(),
    )
    .unwrap();
    let ctx = WireContext {
        room: scope.room,
        generation: scope.generation,
    };
    let inbound = wss.recv().await.unwrap();
    let Frame::Challenge(host_challenge) =
        wire::decode(ctx, Kind::HostChallenge, &inbound).unwrap()
    else {
        panic!("expected host challenge over WSS")
    };
    let challenge = wire::encode(
        ctx,
        Kind::ViewerChallenge,
        Frame::Challenge(proof.challenge(wall()).unwrap()),
    )
    .unwrap();
    wss.send(&challenge).await.unwrap();

    let mut peer = Peer::start(turn).await;
    let channel = peer
        .pc
        .create_data_channel(
            LABEL,
            Some(RTCDataChannelInit {
                protocol: PROTOCOL.into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let offer = peer.local(false).await;
    let mut signature = proof.seal(host_challenge, &offer, wall()).unwrap();
    if tamper_offer {
        signature[0] ^= 1;
    }
    let offer_frame = wire::encode(
        ctx,
        Kind::Offer,
        Frame::SignedSdp {
            sdp: &offer,
            signature,
        },
    )
    .unwrap();
    wss.send(&offer_frame).await.unwrap();

    let inbound = match wss.recv().await {
        Ok(bytes) => bytes,
        Err(WSS_CLOSED) => return ViewerNegotiation::HostClosed(peer),
        Err(error) => panic!("unexpected answer receive failure: {error}"),
    };
    let Frame::SignedSdp {
        sdp: answer,
        signature,
    } = wire::decode(ctx, Kind::Answer, &inbound).unwrap()
    else {
        panic!("expected signed answer over WSS")
    };
    proof.verify(answer, &signature, wall()).unwrap();
    let pair = proof.approved_relay_pair(wall()).unwrap();
    peer.remote(answer, true, pair.remote()).await;
    ViewerNegotiation::Ready(Box::new(Negotiated { peer, proof }), channel)
}

async fn assert_rtc_revoked(peer: &Peer, turn: &FixtureTurn) {
    let loopback = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    loopback.set_nonblocking(true).unwrap();
    let socket = peer.runtime.wrap_udp_socket(loopback).unwrap();
    assert_eq!(
        socket
            .send_to(b"stopped fixture", turn.server().into())
            .await
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
}

async fn close_rtc(host: &Peer, viewer: &Peer, turn: &FixtureTurn) {
    host.runtime.revoke();
    viewer.runtime.revoke();
    let (host_close, viewer_close) = tokio::join!(host.pc.close(), viewer.pc.close());
    host_close.unwrap();
    viewer_close.unwrap();
    for peer in [host, viewer] {
        assert_rtc_revoked(peer, turn).await;
    }
}

async fn exercise(case: Case) {
    tokio::time::timeout(WAIT, async {
        // Obtain the real fixture CA and ports. WSS handshakes and both
        // loopback binds happen now, but no diagnostic frames or ICE yet.
        let mut wss = FixtureWss::start().await.unwrap();
        let turn = FixtureTurn::start(20).await.unwrap();
        turn.block_direct(true);
        assert_eq!(*turn.server().ip(), Ipv4Addr::LOCALHOST);
        let host_id = Identity::generate_ephemeral().unwrap();
        let viewer_id = Identity::generate_ephemeral().unwrap();
        let host_pin = pin(&host_id, &viewer_id);
        let viewer_pin = pin(&viewer_id, &host_id);
        let scope = FakeInputProofScope {
            room: Uuid::new_v4(),
            generation: [9; 32],
            route: PublicVideoScope {
                // Bind the real fixture port and CA, but only the socket
                // fixture ever dials it (at its physical loopback IP).
                wss: SocketAddrV4::new(RELAY_LABEL, wss.port()),
                ca_sha256: wss.ca_sha256(),
                turn: SocketAddrV4::new(RELAY_LABEL, turn.server().port()),
                relay_ports: RELAY_PORTS,
                mode: IceMode::RelayOnly,
                expires_at: wall() + 120,
            },
        };
        let mut viewer_scope = scope;
        if case == Case::MismatchedGeneration {
            viewer_scope.generation[0] ^= 1;
        }
        let host_fp = host_id.public().fingerprint;
        let viewer_fp = viewer_id.public().fingerprint;
        let host_profile = SyntheticProfile {
            role: SessionRole::Host,
            mode: TEST_MODE,
            scope,
            local_fingerprint: &host_fp,
            peer_fingerprint: &viewer_fp,
        }
        .validate(SessionRole::Host, &host_id, &host_pin, wall())
        .unwrap();
        let viewer_profile = SyntheticProfile {
            role: SessionRole::Viewer,
            mode: TEST_MODE,
            scope: viewer_scope,
            local_fingerprint: &viewer_fp,
            peer_fingerprint: &host_fp,
        }
        .validate(SessionRole::Viewer, &viewer_id, &viewer_pin, wall())
        .unwrap();
        let preflight = check_pair(&host_profile, &viewer_profile, wall());
        if case == Case::MismatchedGeneration {
            assert_eq!(preflight, Err(ProfileError::PairMismatch));
            let idle = Duration::from_millis(80);
            let (host, viewer) = tokio::join!(
                tokio::time::timeout(idle, wss.host.recv()),
                tokio::time::timeout(idle, wss.viewer.recv()),
            );
            assert!(host.is_err() && viewer.is_err());
            let stats = turn.stats();
            assert_eq!(stats.allocations_created, 0);
            assert_eq!(stats.active, 0);
            assert_eq!(stats.permissions_created, 0);
            assert_eq!(stats.forwarded_packets, 0);
            wss.close().await.unwrap();
            let turn_addr = turn.server();
            drop(turn);
            // Drop aborts the test actor; wait for the UDP listener to be
            // reclaimed rather than assuming abort has run synchronously.
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    match UdpSocket::bind(turn_addr) {
                        Ok(_) => break,
                        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                        Err(error) => panic!("could not verify fixture UDP teardown: {error}"),
                    }
                }
            })
            .await
            .expect("fixture UDP listener should be released");
            return;
        }
        preflight.unwrap();

        let (host_result, viewer_result) = tokio::join!(
            negotiate_host(&mut wss.host, &turn, &host_id, &host_pin, scope),
            negotiate_viewer(
                &mut wss.viewer,
                &turn,
                &viewer_id,
                &viewer_pin,
                viewer_scope,
                case == Case::TamperedOffer,
            ),
        );
        let (
            Negotiated {
                peer: mut host,
                proof: mut host_proof,
            },
            Negotiated {
                peer: viewer,
                proof: mut viewer_proof,
            },
            viewer_channel,
        ) = match (host_result, viewer_result) {
            (HostNegotiation::Ready(host), ViewerNegotiation::Ready(viewer, channel))
                if case != Case::TamperedOffer =>
            {
                (*host, *viewer, channel)
            }
            (HostNegotiation::RejectedOffer, ViewerNegotiation::HostClosed(viewer))
                if case == Case::TamperedOffer =>
            {
                // Only the gathering viewer ever had an RTC. It received real
                // WSS Close, never an answer or a permitted relay destination.
                assert!(viewer.pc.remote_description().await.is_none());
                for port in [RELAY_PORTS.0, RELAY_PORTS.1] {
                    assert!(!viewer
                        .runtime
                        .allows_turn_peer(SocketAddrV4::new(RELAY_LABEL, port).into()));
                }
                let stats = turn.stats();
                assert_eq!(stats.allocations_created, 1);
                assert_eq!(stats.permissions_created, 0);
                assert_eq!(stats.forwarded_packets, 0);
                viewer.runtime.revoke();
                viewer.pc.close().await.unwrap();
                assert_rtc_revoked(&viewer, &turn).await;
                wss.close().await.unwrap();
                return;
            }
            _ => panic!("unexpected host/viewer negotiation outcomes"),
        };
        let viewer_pair = viewer_proof.approved_relay_pair(wall()).unwrap();
        let host_pair = host_proof.approved_relay_pair(wall()).unwrap();
        assert_eq!(host_pair.remote(), viewer_pair.local());
        assert_eq!(host_pair.local(), viewer_pair.remote());

        let host_channel = host.channels.recv().await.unwrap();
        tokio::join!(opened(&host_channel), opened(&viewer_channel));
        assert_eq!(host_channel.label().await.unwrap(), LABEL);
        assert_eq!(host_channel.protocol().await.unwrap(), PROTOCOL);
        assert!(host_channel.ordered().await.unwrap());
        assert!(!host_channel.negotiated().await.unwrap());
        assert!(host_channel.max_retransmits().await.unwrap().is_none());
        assert!(host_channel.max_packet_life_time().await.unwrap().is_none());
        host.verify_pair(&mut host_proof).await;
        viewer.verify_pair(&mut viewer_proof).await;
        for peer in [&host, &viewer] {
            assert!(peer
                .runtime
                .resolve_host(&scope.route.turn.to_string())
                .await
                .is_err());
            assert!(!peer
                .runtime
                .allows_turn_peer(SocketAddrV4::new(RELAY_LABEL, 49154).into()));
        }
        let connected = turn.stats();
        assert_eq!(connected.allocations_created, 2);
        assert_eq!(connected.active, 2);
        assert!(connected.permissions_created >= 2);
        assert_eq!(connected.direct_forwarded, 0);

        let input_scope = InputScope {
            room: *scope.room.as_bytes(),
            generation: scope.generation,
            viewer_key: *host_pin.public_key(),
        };
        let fake = Rc::new(Fake::default());
        let mut input = InputQueue::view_only(input_scope, Instant::now() + WAIT, fake.clone());
        let pending = input.begin_local_decision(&input_scope).unwrap();
        viewer_channel
            .try_send_text("fixture:before")
            .await
            .unwrap();
        text(&host_channel, "fixture:before").await;
        assert_eq!(input.enqueue(&input_scope, ()), Err(DENIED));
        assert_eq!(fake.applied.get(), 0);
        no_ack(&viewer_channel).await;

        if case == Case::StopBeforeConsent {
            // Observe loss from the actual authenticated fixture socket.
            // The serialized owner processes it before the delayed reply.
            wss.viewer.disconnect().await.unwrap();
            assert_eq!(wss.host.recv().await, Err(WSS_CLOSED));
            assert_eq!(
                input.advance(Control::TransportLost),
                Ok(State::Ended(End::Disconnected))
            );
            assert_eq!(
                input.complete_local_decision(pending, &input_scope, true, Control::TransportLost),
                Err(DENIED)
            );
            assert_eq!(input.enqueue(&input_scope, ()), Err(DENIED));
            assert_eq!(
                input.dispatch_next(&input_scope, Control::Continue),
                Ok(Dispatch::Closed)
            );
            assert_eq!(fake.applied.get(), 0);
            assert_eq!(fake.released.get(), 0);
            assert_eq!(fake.release_calls.get(), 0);
            no_ack(&viewer_channel).await;
            close_rtc(&host, &viewer, &turn).await;
            wss.close().await.unwrap();
            return;
        }

        // Test owner's separate consent. Nothing read from SCTP can grant it.
        input
            .complete_local_decision(pending, &input_scope, true, Control::Continue)
            .unwrap();
        viewer_channel
            .try_send_text("fixture:approved")
            .await
            .unwrap();
        text(&host_channel, "fixture:approved").await;
        host.verify_pair(&mut host_proof).await;
        viewer.verify_pair(&mut viewer_proof).await;
        input.enqueue(&input_scope, ()).unwrap();
        assert_eq!(
            input.dispatch_next(&input_scope, Control::Continue),
            Ok(Dispatch::Applied)
        );
        host_channel
            .try_send_text("fixture:applied-ack")
            .await
            .unwrap();
        text(&viewer_channel, "fixture:applied-ack").await;
        assert_eq!(fake.applied.get(), 1);
        assert!(fake.pressed.get());
        assert_eq!(fake.released.get(), 0);
        assert_eq!(fake.release_calls.get(), 0);

        let (end, replay) = if case == Case::DisconnectWhilePressed {
            viewer_channel
                .try_send_text("fixture:queued")
                .await
                .unwrap();
            text(&host_channel, "fixture:queued").await;
            input.enqueue(&input_scope, ()).unwrap();
            assert!(fake.pressed.get());

            // Authenticated WSS loss is processed before the queued marker.
            // Keep SCTP open through the no-ACK check so closing it cannot
            // mask an incorrectly emitted ACK.
            wss.viewer.disconnect().await.unwrap();
            assert_eq!(wss.host.recv().await, Err(WSS_CLOSED));
            assert_eq!(
                input.dispatch_next(&input_scope, Control::TransportLost),
                Ok(Dispatch::Closed)
            );
            (End::Disconnected, "fixture:queued")
        } else {
            input.enqueue(&input_scope, ()).unwrap();
            assert_eq!(
                input.dispatch_next(&input_scope, Control::Revoke),
                Ok(Dispatch::Closed)
            );
            (End::Revoked, "fixture:after")
        };
        assert_eq!(input.state(), State::Ended(end));
        viewer_channel.try_send_text(replay).await.unwrap();
        text(&host_channel, replay).await;
        assert_eq!(input.enqueue(&input_scope, ()), Err(DENIED));
        assert_eq!(
            input.dispatch_next(&input_scope, Control::Continue),
            Ok(Dispatch::Closed)
        );
        assert_eq!(fake.applied.get(), 1);
        assert_eq!(fake.released.get(), 1);
        assert_eq!(fake.release_calls.get(), 1);
        assert!(!fake.pressed.get());
        no_ack(&viewer_channel).await;
        assert!(turn.stats().forwarded_packets > connected.forwarded_packets);
        assert_eq!(turn.stats().direct_forwarded, 0);

        close_rtc(&host, &viewer, &turn).await;
        wss.close().await.unwrap();
        assert!(!host.runtime.allows_turn_peer(host_pair.remote().into()));
        assert!(!viewer.runtime.allows_turn_peer(viewer_pair.remote().into()));
        drop(input);
        assert_eq!(fake.released.get(), 1);
        assert_eq!(fake.release_calls.get(), 1);
    })
    .await
    .expect("bounded TURN fixture SCTP round trip");
}

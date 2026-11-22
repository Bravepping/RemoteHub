// Real ICE/DTLS/SCTP over localhost, with synthetic text only.
use super::{
    tests::{start_server, TOKEN},
    tls_smoke::{fixture, make_ca, start_tls},
};
use hub_identity::{
    pairing::PairingCandidate,
    session::{
        PendingSessionProof, SessionBinding, SessionChallenge, SessionRole, CHALLENGE_BYTES,
    },
    Identity,
};
use hub_webrtc::{
    authenticated_relay::{
        AuthenticatedLoopbackRelay, AuthenticatedRelayEvent, ClientError, LoopbackTlsTrust,
        RelayToken, Role, RoomId, SignalKind,
    },
    lan_wire::Key,
    pinned_runtime::PinnedRuntime,
};
use rtc::ice::{mdns::MulticastDnsMode, network_type::NetworkType};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::mpsc, time::timeout};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceCandidateInit,
        RTCPeerConnectionIceEvent, RTCSessionDescription, SettingEngineBuilder,
    },
};

struct Events {
    ice: mpsc::Sender<RTCIceCandidateInit>,
    channels: mpsc::Sender<Arc<dyn DataChannel>>,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let candidate = event.candidate.to_json().unwrap();
        if candidate.candidate.split_whitespace().nth(1) == Some("1") {
            self.ice.try_send(candidate).unwrap();
        }
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        self.channels.try_send(channel).unwrap();
    }
}
struct Peer {
    pc: Box<dyn PeerConnection>,
    ice: mpsc::Receiver<RTCIceCandidateInit>,
    channels: mpsc::Receiver<Arc<dyn DataChannel>>,
}
impl Peer {
    async fn new() -> Self {
        let (ice_tx, ice) = mpsc::channel(8);
        let (channel_tx, channels) = mpsc::channel(1);
        let local = Ipv4Addr::LOCALHOST.into();
        let pc = PeerConnectionBuilder::new()
            .with_setting_engine(
                SettingEngineBuilder::new()
                    .with_include_loopback_candidate(true)
                    .with_multicast_dns_mode(MulticastDnsMode::Disabled)
                    .with_network_types(vec![NetworkType::Udp4])
                    .build(),
            )
            .with_udp_addrs(vec!["127.0.0.1:0"])
            .with_runtime(Arc::new(PinnedRuntime {
                inner: webrtc::runtime::default_runtime().unwrap(),
                local,
                peer: local,
                counters: Default::default(),
            }))
            .with_handler(Arc::new(Events {
                ice: ice_tx,
                channels: channel_tx,
            }))
            .build()
            .await
            .unwrap();
        Self {
            pc: Box::new(pc),
            ice,
            channels,
        }
    }
}

struct TestDevices {
    host_binding: SessionBinding,
    viewer_binding: SessionBinding,
    host: Identity,
    viewer: Identity,
}

// The test expects one SDP and one trickle ICE message from each peer.
// Each verifier keeps its own slots, while only the public challenges cross WSS.
struct DirectionProofs {
    pending: [PendingSessionProof; 2],
    received: [SessionChallenge; 2],
}
struct DeviceProofs<'a> {
    devices: &'a TestDevices,
    from_host: DirectionProofs,
    from_viewer: DirectionProofs,
}
struct Proof<'a> {
    binding: &'a SessionBinding,
    identity: &'a Identity,
    role: SessionRole,
    challenge: SessionChallenge,
    pending: &'a mut PendingSessionProof,
}
impl DeviceProofs<'_> {
    fn for_signal(&mut self, role: SessionRole, index: usize) -> Proof<'_> {
        let (identity, binding, direction) = match role {
            SessionRole::Host => (
                &self.devices.host,
                &self.devices.host_binding,
                &mut self.from_host,
            ),
            SessionRole::Viewer => (
                &self.devices.viewer,
                &self.devices.viewer_binding,
                &mut self.from_viewer,
            ),
        };
        Proof {
            binding,
            identity,
            role,
            challenge: direction.received[index],
            pending: &mut direction.pending[index],
        }
    }
}

fn issue(binding: &SessionBinding, signer: SessionRole) -> [PendingSessionProof; 2] {
    std::array::from_fn(|_| {
        PendingSessionProof::issue(binding, signer, wall_seconds(), Duration::from_secs(15))
            .unwrap()
    })
}

async fn forward_challenges(
    from: &mut AuthenticatedLoopbackRelay,
    to: &mut AuthenticatedLoopbackRelay,
    kind: SignalKind,
    pending: &[PendingSessionProof; 2],
) -> [SessionChallenge; 2] {
    let packed = pending
        .each_ref()
        .map(|slot| slot.challenge().to_bytes().to_vec());
    let wire = exchange(
        from,
        to,
        kind,
        serde_json::to_string(&packed).unwrap(),
        None,
    )
    .await;
    let payload: [Vec<u8>; 2] = serde_json::from_str(&wire).unwrap();
    payload.map(|entry| {
        let fixed: [u8; CHALLENGE_BYTES] = entry.try_into().unwrap();
        SessionChallenge::from_bytes(fixed).unwrap()
    })
}

async fn exchange_challenges<'a>(
    devices: &'a TestDevices,
    host: &mut AuthenticatedLoopbackRelay,
    viewer: &mut AuthenticatedLoopbackRelay,
) -> DeviceProofs<'a> {
    let pending_viewer = issue(&devices.host_binding, SessionRole::Viewer);
    let pending_host = issue(&devices.viewer_binding, SessionRole::Host);
    let received_viewer =
        forward_challenges(host, viewer, SignalKind::HostChallenge, &pending_viewer).await;
    let received_host =
        forward_challenges(viewer, host, SignalKind::ViewerChallenge, &pending_host).await;
    DeviceProofs {
        devices,
        from_host: DirectionProofs {
            pending: pending_host,
            received: received_host,
        },
        from_viewer: DirectionProofs {
            pending: pending_viewer,
            received: received_viewer,
        },
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedSignal {
    payload: String,
    signature: Vec<u8>,
}

fn signal_bytes(kind: SignalKind, payload: &str) -> Vec<u8> {
    let label = match kind {
        SignalKind::Offer => b'O',
        SignalKind::Answer => b'A',
        SignalKind::Ice => b'I',
        SignalKind::HostChallenge | SignalKind::ViewerChallenge => {
            unreachable!("device signatures apply only to SDP and ICE")
        }
    };
    let mut bytes = Vec::with_capacity(1 + payload.len());
    bytes.push(label);
    bytes.extend_from_slice(payload.as_bytes());
    bytes
}

fn wall_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// The relay forwards these inner authenticated frames without the pairing key.
// For the WSS fixture, peer public keys still come from trusted fixture state.
async fn exchange(
    from: &mut AuthenticatedLoopbackRelay,
    to: &mut AuthenticatedLoopbackRelay,
    kind: SignalKind,
    payload: String,
    proof: Option<Proof<'_>>,
) -> String {
    let (outbound, pending) = match proof {
        Some(proof) => {
            let signature = proof
                .identity
                .sign_session_proof(
                    proof.binding,
                    proof.role,
                    proof.challenge,
                    &signal_bytes(kind, &payload),
                    wall_seconds(),
                )
                .unwrap();
            let signed = SignedSignal {
                payload,
                signature: signature.to_vec(),
            };
            (serde_json::to_string(&signed).unwrap(), Some(proof.pending))
        }
        None => (payload, None),
    };
    from.send_signal(kind, outbound).await.unwrap();
    let AuthenticatedRelayEvent::Signal { kind: signal, data } =
        to.next_event(Duration::from_secs(3)).await.unwrap()
    else {
        panic!("expected authenticated signaling");
    };
    assert_eq!(signal, kind);
    let Some(pending) = pending else {
        return data;
    };
    let received: SignedSignal = serde_json::from_str(&data).unwrap();
    let signature: [u8; 64] = received.signature.try_into().unwrap();
    pending
        .verify(
            &signature,
            &signal_bytes(kind, &received.payload),
            wall_seconds(),
        )
        .unwrap();
    received.payload
}

async fn opened(channel: &Arc<dyn DataChannel>) {
    while let Some(event) = channel.poll().await {
        match event {
            DataChannelEvent::OnOpen => return,
            DataChannelEvent::OnClose | DataChannelEvent::OnError => break,
            _ => (),
        }
    }
    panic!("channel closed before opening");
}

async fn exercise(
    viewer: &mut Peer,
    host: &mut Peer,
    viewer_relay: &mut AuthenticatedLoopbackRelay,
    host_relay: &mut AuthenticatedLoopbackRelay,
    mut proofs: Option<DeviceProofs<'_>>,
) {
    let channel = viewer
        .pc
        .create_data_channel("relay-smoke", None)
        .await
        .unwrap();

    let offer = viewer.pc.create_offer(None).await.unwrap();
    viewer
        .pc
        .set_local_description(offer.clone())
        .await
        .unwrap();
    let offer = exchange(
        viewer_relay,
        host_relay,
        SignalKind::Offer,
        offer.sdp,
        proofs
            .as_mut()
            .map(|p| p.for_signal(SessionRole::Viewer, 0)),
    )
    .await;
    host.pc
        .set_remote_description(RTCSessionDescription::offer(offer).unwrap())
        .await
        .unwrap();

    let answer = host.pc.create_answer(None).await.unwrap();
    host.pc.set_local_description(answer.clone()).await.unwrap();
    let answer = exchange(
        host_relay,
        viewer_relay,
        SignalKind::Answer,
        answer.sdp,
        proofs.as_mut().map(|p| p.for_signal(SessionRole::Host, 0)),
    )
    .await;
    viewer
        .pc
        .set_remote_description(RTCSessionDescription::answer(answer).unwrap())
        .await
        .unwrap();

    let viewer_ice = serde_json::to_string(&viewer.ice.recv().await.unwrap()).unwrap();
    let viewer_ice = exchange(
        viewer_relay,
        host_relay,
        SignalKind::Ice,
        viewer_ice,
        proofs
            .as_mut()
            .map(|p| p.for_signal(SessionRole::Viewer, 1)),
    )
    .await;
    host.pc
        .add_ice_candidate(serde_json::from_str(&viewer_ice).unwrap())
        .await
        .unwrap();
    let host_ice = serde_json::to_string(&host.ice.recv().await.unwrap()).unwrap();
    let host_ice = exchange(
        host_relay,
        viewer_relay,
        SignalKind::Ice,
        host_ice,
        proofs.as_mut().map(|p| p.for_signal(SessionRole::Host, 1)),
    )
    .await;
    viewer
        .pc
        .add_ice_candidate(serde_json::from_str(&host_ice).unwrap())
        .await
        .unwrap();

    opened(&channel).await;
    let incoming = host.channels.recv().await.unwrap();
    opened(&incoming).await;
    assert_eq!(incoming.label().await.unwrap(), "relay-smoke");
    channel.send_text("synthetic-test").await.unwrap();
    loop {
        match incoming.poll().await {
            Some(DataChannelEvent::OnMessage(message)) => {
                assert!(message.is_string);
                assert_eq!(message.data.as_ref(), b"synthetic-test");
                break;
            }
            Some(
                DataChannelEvent::OnBufferedAmountHigh | DataChannelEvent::OnBufferedAmountLow,
            ) => {}
            _ => panic!("channel closed before the test message"),
        }
    }
}

#[tokio::test]
async fn native_webrtc_connects_with_authenticated_sdp_and_trickle_ice_over_v2_signal() {
    let (addr, _, server) = start_server().await;
    let token = RelayToken::new(TOKEN.to_owned()).unwrap();
    native_peers_round_trip(addr, &token, None).await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn native_webrtc_connects_over_wss_with_verified_device_signatures() {
    let (app, token) = fixture();
    let ca = make_ca();
    let trust = LoopbackTlsTrust::from_ca_der(ca.der().as_ref()).unwrap();
    let (addr, server) = start_tls(&ca, "127.0.0.1", app.clone()).await;
    native_peers_round_trip(addr, &token, Some(&trust)).await;
    assert!(app.rooms.lock().unwrap().is_empty());
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

async fn native_peers_round_trip(
    addr: SocketAddr,
    token: &RelayToken,
    trust: Option<&LoopbackTlsTrust>,
) {
    let room = RoomId::random();
    let key = Arc::new(Key::generate());
    let first_nonce = uuid::Uuid::new_v4();
    let nonce = first_nonce.to_string();
    let devices = trust.map(|_| {
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let room = uuid::Uuid::parse_str(room.as_str()).unwrap();
        let mut session_nonce = [0; 32];
        session_nonce[..16].copy_from_slice(first_nonce.as_bytes());
        session_nonce[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let host_public = host.public();
        let viewer_public = viewer.public();
        let host_verified_viewer = host_public
            .verify_peer(
                &PairingCandidate::parse(&viewer_public.pairing_code()).unwrap(),
                &viewer_public.fingerprint,
            )
            .unwrap();
        let viewer_verified_host = viewer_public
            .verify_peer(
                &PairingCandidate::parse(&host_public.pairing_code()).unwrap(),
                &host_public.fingerprint,
            )
            .unwrap();
        let host_binding = host_verified_viewer
            .session_binding(
                &host_public,
                SessionRole::Host,
                *room.as_bytes(),
                session_nonce,
            )
            .unwrap();
        let viewer_binding = viewer_verified_host
            .session_binding(
                &viewer_public,
                SessionRole::Viewer,
                *room.as_bytes(),
                session_nonce,
            )
            .unwrap();
        TestDevices {
            host_binding,
            viewer_binding,
            host,
            viewer,
        }
    });
    let connect = async |role| {
        let room = RoomId::new(room.as_str().to_owned()).unwrap();
        match trust {
            Some(trust) => {
                AuthenticatedLoopbackRelay::connect_wss(
                    addr,
                    token,
                    room,
                    role,
                    key.clone(),
                    nonce.clone(),
                    trust,
                )
                .await
            }
            None => {
                AuthenticatedLoopbackRelay::connect(
                    addr,
                    token,
                    room,
                    role,
                    key.clone(),
                    nonce.clone(),
                )
                .await
            }
        }
        .unwrap()
    };
    let mut host_relay = connect(Role::Host).await;
    assert!(matches!(
        host_relay.next_event(Duration::from_secs(3)).await.unwrap(),
        AuthenticatedRelayEvent::Waiting
    ));
    let mut viewer_relay = connect(Role::Viewer).await;
    for relay in [&mut host_relay, &mut viewer_relay] {
        assert!(matches!(
            relay.next_event(Duration::from_secs(3)).await.unwrap(),
            AuthenticatedRelayEvent::Paired
        ));
    }
    let proofs = match devices.as_ref() {
        Some(d) => Some(exchange_challenges(d, &mut host_relay, &mut viewer_relay).await),
        None => None,
    };
    let mut viewer = Peer::new().await;
    let mut host = Peer::new().await;
    let result = timeout(
        Duration::from_secs(12),
        exercise(
            &mut viewer,
            &mut host,
            &mut viewer_relay,
            &mut host_relay,
            proofs,
        ),
    )
    .await;
    viewer.pc.close().await.unwrap();
    host.pc.close().await.unwrap();
    result.expect("loopback negotiation timed out");
    viewer_relay.leave().await.unwrap();
    assert!(matches!(
        host_relay.next_event(Duration::from_secs(3)).await.unwrap(),
        AuthenticatedRelayEvent::PeerLeft
    ));
    assert_eq!(
        host_relay
            .send_signal(SignalKind::Ice, "ignored".into())
            .await,
        Err(ClientError::Closed)
    );
    host_relay.leave().await.unwrap();
}

async fn mixed_pair(
    addr: std::net::SocketAddr,
    token: &RelayToken,
    room: &RoomId,
    key: Arc<Key>,
    nonce: &str,
) -> (AuthenticatedLoopbackRelay, hub_signal_v2::LoopbackRelay) {
    let mut host = AuthenticatedLoopbackRelay::connect(
        addr,
        token,
        RoomId::new(room.as_str().into()).unwrap(),
        Role::Host,
        key,
        nonce.into(),
    )
    .await
    .unwrap();
    assert!(matches!(
        host.next_event(Duration::from_secs(3)).await.unwrap(),
        AuthenticatedRelayEvent::Waiting
    ));
    let mut viewer = hub_signal_v2::LoopbackRelay::connect(
        addr,
        token,
        RoomId::new(room.as_str().into()).unwrap(),
        Role::Viewer,
    )
    .await
    .unwrap();
    assert!(matches!(
        host.next_event(Duration::from_secs(3)).await.unwrap(),
        AuthenticatedRelayEvent::Paired
    ));
    assert!(matches!(
        viewer.next_event(Duration::from_secs(3)).await.unwrap(),
        hub_signal_v2::RelayEvent::Paired
    ));
    (host, viewer)
}

#[tokio::test]
async fn authenticated_wrapper_rejects_wrong_mac_and_mismatched_signal_kind_then_closes() {
    use hub_webrtc::lan_wire::Codec;

    let (addr, _, server) = start_server().await;
    let token = RelayToken::new(TOKEN.to_owned()).unwrap();
    for wrong_mac in [true, false] {
        let room = RoomId::random();
        let nonce = uuid::Uuid::new_v4().to_string();
        let key = Arc::new(Key::generate());
        let (mut host, mut viewer) = mixed_pair(addr, &token, &room, key.clone(), &nonce).await;
        let mut test_peer = Codec::new_v2(
            if wrong_mac {
                Arc::new(Key::generate())
            } else {
                key
            },
            nonce,
            room.as_str(),
            false,
        )
        .unwrap();
        let fake_offer = test_peer
            .encode(
                if wrong_mac { "offer" } else { "answer" },
                "synthetic text".into(),
            )
            .unwrap();
        viewer
            .send_signal(
                SignalKind::Offer,
                serde_json::to_string(&fake_offer).unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            host.next_event(Duration::from_secs(3)).await,
            Err(ClientError::Protocol)
        ));
        assert!(matches!(
            host.next_event(Duration::ZERO).await,
            Err(ClientError::Closed)
        ));
        assert!(matches!(
            viewer.next_event(Duration::from_secs(3)).await.unwrap(),
            hub_signal_v2::RelayEvent::PeerLeft
        ));
        host.leave().await.unwrap();
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn authenticated_wrapper_timeout_is_retryable_but_cancelled_poll_releases_peer() {
    let (addr, _, server) = start_server().await;
    let token = RelayToken::new(TOKEN.to_owned()).unwrap();
    let room = RoomId::random();
    let nonce = uuid::Uuid::new_v4().to_string();
    let (mut host, mut viewer) =
        mixed_pair(addr, &token, &room, Arc::new(Key::generate()), &nonce).await;
    assert!(matches!(
        host.next_event(Duration::from_millis(10)).await,
        Err(ClientError::Timeout)
    ));
    assert!(timeout(
        Duration::from_millis(10),
        host.next_event(Duration::from_secs(3))
    )
    .await
    .is_err());
    assert!(matches!(
        host.next_event(Duration::ZERO).await,
        Err(ClientError::Closed)
    ));
    assert!(matches!(
        viewer.next_event(Duration::from_secs(3)).await.unwrap(),
        hub_signal_v2::RelayEvent::PeerLeft
    ));
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

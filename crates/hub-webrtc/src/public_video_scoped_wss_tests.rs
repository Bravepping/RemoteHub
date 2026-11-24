//! Actual WSS and scoped ICE workers on explicit IPv4 loopback only.

use super::*;
use crate::{
    direct_policy::{Kind, SelectedRoute},
    ice_policy::IceMode,
    public_video::PublicVideoScopedIce,
};
use futures_util::{SinkExt, StreamExt};
use hub_identity::pairing::PairingCandidate;
use hub_signal_v2::wire::{self, ClientFrame, Flow, ServerFrame, PATH, VERSION};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use sha2::{Digest, Sha256};
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::{server::TlsStream, TlsAcceptor};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        http::header::AUTHORIZATION,
        Message,
    },
    WebSocketStream,
};
use webrtc::peer_connection::RTCConfigurationBuilder;

#[path = "../../hub-rendezvous-probe/src/bin/v2_signaling/public_video_fixture_net.rs"]
mod fixture_net;
use fixture_net::{FixtureNat, FixtureSide};
#[path = "public_video_turn_fixture_net.rs"]
mod turn_fixture;
use turn_fixture::{FixtureDirectSide, FixtureTurn, RELAY_LABEL, RELAY_PORTS};

const WAIT: Duration = Duration::from_secs(6);
const TURN_TTL_SECS: u32 = 8;
#[cfg(feature = "v2-public-video-60s-test")]
const MINUTE_TURN_TTL_SECS: u32 = 75;
const MINUTE_MEDIA_SECS: u64 = 60;
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

type Socket = WebSocketStream<TlsStream<TcpStream>>;

struct RelayFixture {
    address: SocketAddr,
    trust: LoopbackTlsTrust,
    ca_hash: [u8; 32],
    task: JoinHandle<()>,
}

impl RelayFixture {
    async fn start() -> Self {
        let mut ca = CertificateParams::default();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate().unwrap()).unwrap();
        let trust = LoopbackTlsTrust::from_ca_der(ca.der().as_ref()).unwrap();
        let ca_hash = Sha256::digest(ca.der().as_ref()).into();
        let mut params = CertificateParams::new(vec![LOOPBACK.to_string()]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().unwrap();
        let certificate = params.signed_by(&key, &ca).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .unwrap();
        let listener = TcpListener::bind((LOOPBACK, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(serve(listener, TlsAcceptor::from(Arc::new(config))));
        Self {
            address,
            trust,
            ca_hash,
            task,
        }
    }
}

impl Drop for RelayFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn accept_peer(listener: &TcpListener, tls: &TlsAcceptor, role: Role) -> (Socket, String) {
    let (tcp, address) = listener.accept().await.unwrap();
    assert_eq!(address.ip(), std::net::IpAddr::V4(LOOPBACK));
    let stream = tls.accept(tcp).await.unwrap();
    let expected = format!("Bearer {TOKEN}");
    let mut ws = accept_hdr_async(stream, move |request: &Request, response: Response| {
        assert_eq!(request.uri().path_and_query().unwrap().as_str(), PATH);
        assert_eq!(request.headers()[AUTHORIZATION], expected.as_str());
        Ok(response)
    })
    .await
    .unwrap();
    match receive(&mut ws).await {
        Some(ClientFrame::Hello {
            version,
            role: actual,
            room,
        }) => {
            assert_eq!(version, VERSION);
            assert_eq!(actual, role);
            (ws, room)
        }
        _ => panic!("expected one fixture hello"),
    }
}

async fn receive(from: &mut Socket) -> Option<ClientFrame> {
    match from.next().await {
        Some(Ok(Message::Text(text))) => Some(wire::decode(&text).unwrap()),
        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => None,
        _ => panic!("unexpected fixture WebSocket frame"),
    }
}

async fn send(to: &mut Socket, frame: ServerFrame) {
    to.send(Message::Text(wire::encode(&frame).unwrap().into()))
        .await
        .unwrap();
}

async fn serve(listener: TcpListener, tls: TlsAcceptor) {
    let (mut host, room) = accept_peer(&listener, &tls, Role::Host).await;
    send(&mut host, ServerFrame::Waiting {}).await;
    let (mut viewer, viewer_room) = accept_peer(&listener, &tls, Role::Viewer).await;
    assert_eq!(room, viewer_room);
    drop(listener);
    send(&mut host, ServerFrame::Paired {}).await;
    send(&mut viewer, ServerFrame::Paired {}).await;
    let mut flow = Flow::default();
    let mut next_seq = [1, 1];
    loop {
        let (role, frame) = tokio::select! {
            value = receive(&mut host) => (Role::Host, value),
            value = receive(&mut viewer) => (Role::Viewer, value),
        };
        let (target, seq) = match role {
            Role::Host => (&mut viewer, &mut next_seq[0]),
            Role::Viewer => (&mut host, &mut next_seq[1]),
        };
        match frame {
            Some(ClientFrame::Signal {
                seq: given,
                signal,
                data,
            }) => {
                assert_eq!(given, *seq);
                assert!(signal.allows_sender(role));
                *seq += 1;
                flow = flow.after(signal).expect("ordered fixture signal");
                send(
                    target,
                    ServerFrame::Signal {
                        from: role,
                        seq: given,
                        signal,
                        data,
                    },
                )
                .await;
            }
            Some(ClientFrame::Leave {}) | None => {
                send(target, ServerFrame::PeerLeft {}).await;
                return;
            }
            _ => panic!("unexpected fixture hello"),
        }
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

fn owner<'a>(
    proof: PublicVideoProof<'a>,
    now: u64,
    nat: &FixtureNat,
    room: Uuid,
    side: FixtureSide,
) -> Result<PublicVideoScopedIce<'a>, &'static str> {
    let stun = nat.stun_server();
    let endpoint: SocketAddrV4 = stun.urls[0].strip_prefix("stun:").unwrap().parse().unwrap();
    assert_eq!(*endpoint.ip(), LOOPBACK);
    PublicVideoScopedIce::new_loopback_fixture(
        proof,
        room,
        SocketAddrV4::new(LOOPBACK, 0),
        endpoint,
        RTCConfigurationBuilder::default()
            .with_ice_servers(vec![stun])
            .build(),
        nat.runtime(side),
        now,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TestPath {
    DirectOnly,
    RelayOnly,
    PreferDirect { blocked: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TestMedia {
    None,
    Burst,
    Sustained,
    #[cfg(feature = "v2-public-video-60s-test")]
    SustainedMinute,
}

async fn exercise(abrupt: bool, path: TestPath, traffic: TestMedia) {
    let mut tls = RelayFixture::start().await;
    let relay_only = path == TestPath::RelayOnly;
    let turn_ttl = match traffic {
        TestMedia::Sustained => 12,
        // The fixed fixture lease must cover ICE/DTLS setup and the
        // minute of media. The signed scoped WSS lifetime is still 120s.
        #[cfg(feature = "v2-public-video-60s-test")]
        TestMedia::SustainedMinute => MINUTE_TURN_TTL_SECS,
        _ => TURN_TTL_SECS,
    };
    let (nat, turn) = if path == TestPath::DirectOnly {
        (Some(FixtureNat::start().await.unwrap()), None)
    } else {
        (None, Some(FixtureTurn::start(turn_ttl).await.unwrap()))
    };
    let blocked = matches!(path, TestPath::PreferDirect { blocked: true });
    if let Some(turn) = &turn {
        turn.block_direct(blocked);
    }
    let route = if relay_only || blocked {
        (SelectedRoute::TurnRelay, Kind::Relay)
    } else {
        (SelectedRoute::ServerReflexiveDirect, Kind::Srflx)
    };
    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    let host_pin = pin(&host, &viewer);
    let viewer_pin = pin(&viewer, &host);
    let token = RelayToken::new(TOKEN.into()).unwrap();
    let key = Arc::new(Key::generate());
    let room = Uuid::new_v4();
    let wire_nonce = Uuid::new_v4().to_string();
    let label = RELAY_LABEL; // logical/signature label, never dialed
    let scope = PublicVideoScope {
        wss: SocketAddrV4::new(label, 8443),
        ca_sha256: tls.ca_hash,
        turn: SocketAddrV4::new(label, 3478),
        relay_ports: (49152, 49200),
        mode: if relay_only {
            IceMode::RelayOnly
        } else {
            IceMode::DirectPreferred
        },
        expires_at: wall_seconds().unwrap() + 120,
    };
    let cfg = |role, local_identity, peer| PublicVideoWssConfig {
        address: tls.address,
        token: &token,
        room,
        role,
        pairing_key: key.clone(),
        wire_nonce: wire_nonce.clone(),
        session_nonce: [9; 32],
        trust: &tls.trust,
        local_identity,
        peer,
        scope,
    };
    let h = PublicVideoWss::connect_loopback_wss(cfg(Role::Host, &host, &host_pin))
        .await
        .unwrap()
        .wait_for_host_registration()
        .await
        .unwrap();
    let v = PublicVideoWss::connect_loopback_wss(cfg(Role::Viewer, &viewer, &viewer_pin))
        .await
        .unwrap();
    let make = |proof, now, side| match (&nat, &turn) {
        (Some(nat), None) => owner(proof, now, nat, room, side),
        (None, Some(turn)) if relay_only => PublicVideoScopedIce::new_turn_loopback_fixture(
            proof,
            turn.server(),
            turn.ice_server(),
            now,
        ),
        (None, Some(turn)) => {
            let side = match side {
                FixtureSide::Host => FixtureDirectSide::Host,
                FixtureSide::Viewer => FixtureDirectSide::Viewer,
            };
            PublicVideoScopedIce::new_loopback_fixture(
                proof,
                room,
                SocketAddrV4::new(LOOPBACK, 0),
                turn.server(),
                RTCConfigurationBuilder::default()
                    .with_ice_servers(vec![turn.combined_ice_server()])
                    .build(),
                turn.runtime(side),
                now,
            )
        }
        _ => unreachable!("exact fixture mode"),
    };
    let (h, v) = tokio::try_join!(
        h.establish_scoped_fixture(|proof, now| make(proof, now, FixtureSide::Host)),
        v.establish_scoped_fixture(|proof, now| make(proof, now, FixtureSide::Viewer)),
    )
    .unwrap();
    let host_runtime = h.ice.fixture_runtime();
    let viewer_runtime = v.ice.fixture_runtime();
    let (host_stop, host_rx) = watch::channel(false);
    let (_viewer_stop, viewer_rx) = watch::channel(false);
    let (host_state_tx, mut host_state) = watch::channel(ScopedWssState::Negotiating);
    let (viewer_state_tx, mut viewer_state) = watch::channel(ScopedWssState::Negotiating);
    #[cfg(feature = "v2-public-control")]
    let (first_au, sent_au) = watch::channel(false);
    let (host_media, viewer_media, mut producer, mut consumer) = if traffic != TestMedia::None {
        let (host_media, producer) = ScopedVideoMedia::host();
        #[cfg(feature = "v2-public-control")]
        let host_media = {
            let mut host_media = host_media;
            host_media.notify_first_host_video(first_au);
            host_media
        };
        let (viewer_media, consumer) = ScopedVideoMedia::viewer();
        (
            Some(host_media),
            Some(viewer_media),
            Some(producer),
            Some(consumer),
        )
    } else {
        (None, None, None, None)
    };
    let control = async {
        let selected = *host_state
            .wait_for(|s| {
                matches!(
                    s,
                    ScopedWssState::HostPathVerified(..) | ScopedWssState::Closed
                )
            })
            .await
            .unwrap();
        assert_eq!(
            selected,
            ScopedWssState::HostPathVerified(route.0, route.1, route.1),
            "first admitted native ICE pair",
        );
        viewer_state
            .wait_for(|s| matches!(s, ScopedWssState::Established))
            .await
            .unwrap();
        if let Some(turn) = &turn {
            let active = turn.stats();
            assert_eq!(active.allocations_created, 2);
            assert_eq!(active.active, 2);
            if relay_only || blocked {
                assert!(active.permissions_created >= 2);
                assert!(active.forwarded_packets > 0);
            }
            match path {
                TestPath::PreferDirect { blocked: true } => {
                    assert!(active.direct_dropped > 0);
                    assert!(active.direct_checks_dropped > 0);
                    assert_eq!(active.direct_forwarded, 0);
                }
                TestPath::PreferDirect { blocked: false } => {
                    assert_eq!(active.direct_dropped, 0);
                    assert!(active.direct_forwarded > 0);
                }
                _ => {}
            }
            if let TestPath::PreferDirect { .. } = path {
                let host_direct = turn.direct_candidate(FixtureDirectSide::Host).unwrap();
                let viewer_direct = turn.direct_candidate(FixtureDirectSide::Viewer).unwrap();
                // The alternate genuinely existed in signed SDP even when
                // connectivity checks were intentionally dropped.
                assert!(host_runtime.allows_turn_peer(viewer_direct.into()));
                assert!(viewer_runtime.allows_turn_peer(host_direct.into()));
                assert!(!host_runtime.allows_turn_peer(host_direct.into()));
                assert!(!viewer_runtime.allows_turn_peer(viewer_direct.into()));
            }
            let allowed = |runtime: &dyn webrtc::runtime::Runtime| {
                assert!(!runtime.allows_turn_peer(SocketAddrV4::new(RELAY_LABEL, 49154).into()));
                [RELAY_PORTS.0, RELAY_PORTS.1]
                    .into_iter()
                    .filter(|&port| {
                        runtime.allows_turn_peer(SocketAddrV4::new(RELAY_LABEL, port).into())
                    })
                    .collect::<Vec<_>>()
            };
            let host_peer = allowed(host_runtime.as_ref());
            let viewer_peer = allowed(viewer_runtime.as_ref());
            assert_eq!(host_peer.len(), 1);
            assert_eq!(viewer_peer.len(), 1);
            assert_ne!(host_peer, viewer_peer);
        }
        if let (Some(producer), Some(consumer)) = (producer.as_mut(), consumer.as_mut()) {
            #[cfg(feature = "v2-public-control")]
            assert!(!*sent_au.borrow(), "a connected path alone is not media");
            loopback_access_units(traffic, producer, consumer).await;
            assert_eq!(
                *viewer_state.borrow(),
                ScopedWssState::ViewerPathVerified(route.0, route.1, route.1),
                "viewer reports the actual verified RTP path",
            );
            #[cfg(feature = "v2-public-control")]
            assert!(*sent_au.borrow(), "complete access unit routed");
        }
        if abrupt {
            tls.task.abort();
        } else {
            host_stop.send_replace(true);
        }
    };
    let active_budget = match traffic {
        // Windows relay-only gathering consumes about 5s before the 7s
        // paced-media budget starts. Reserve teardown time independently.
        TestMedia::Sustained => WAIT + Duration::from_secs(9),
        #[cfg(feature = "v2-public-video-60s-test")]
        TestMedia::SustainedMinute => Duration::from_secs(73),
        _ => WAIT,
    };
    let (host_result, viewer_result, ()) = timeout(active_budget, async {
        tokio::join!(
            async {
                if let Some(media) = host_media {
                    h.run_with_media(host_rx, host_state_tx, media).await
                } else {
                    h.run(host_rx, host_state_tx).await
                }
            },
            async {
                if let Some(media) = viewer_media {
                    v.run_with_media(viewer_rx, viewer_state_tx, media).await
                } else {
                    v.run(viewer_rx, viewer_state_tx).await
                }
            },
            control
        )
    })
    .await
    .expect("scoped native session and both teardown paths");
    if abrupt {
        assert!(host_result.is_err());
        assert!(viewer_result.is_err());
        assert!((&mut tls.task).await.unwrap_err().is_cancelled());
    } else {
        assert!(matches!(host_result, Ok(ScopedWssEnd::Stopped)));
        assert!(matches!(viewer_result, Ok(ScopedWssEnd::PeerLeft)));
        timeout(WAIT, &mut tls.task).await.unwrap().unwrap();
    }
    assert!(matches!(&*host_state.borrow(), ScopedWssState::Closed));
    assert!(matches!(&*viewer_state.borrow(), ScopedWssState::Closed));
    if let Some(consumer) = consumer.as_mut() {
        assert!(
            consumer.next_frame().await.is_err(),
            "closed viewer cannot release frames"
        );
    }
    if let Some(producer) = producer.as_mut() {
        assert_eq!(
            producer.send_access_unit(Vec::new()).await,
            Err("PUBLIC_VIDEO_MEDIA_CLOSED")
        );
    }

    if let Some(turn) = &turn {
        for runtime in [&host_runtime, &viewer_runtime] {
            for relay_port in [RELAY_PORTS.0, RELAY_PORTS.1] {
                assert!(
                    !runtime.allows_turn_peer(SocketAddrV4::new(RELAY_LABEL, relay_port).into())
                );
            }
        }
        if !relay_only {
            // The DirectRuntime also denies every physical destination once
            // stopped, including the shared STUN/TURN listener.
            assert!(host_runtime
                .resolve_host(&turn.server().to_string())
                .await
                .is_err());
            assert!(viewer_runtime
                .resolve_host(&turn.server().to_string())
                .await
                .is_err());
            return;
        }
        // A local close is not evidence of a server deallocation. Both
        // allocations remain until lifetime expiry in this fixture.
        let closed = turn.stats();
        assert_eq!(closed.active, 2);
        assert_eq!(closed.released, 0);
        for runtime in [&host_runtime, &viewer_runtime] {
            let local = std::net::UdpSocket::bind((LOOPBACK, 0)).unwrap();
            local.set_nonblocking(true).unwrap();
            let scoped = runtime.wrap_udp_socket(local).unwrap();
            assert_eq!(
                scoped
                    .send_to(b"closed", turn.server().into())
                    .await
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::PermissionDenied,
            );
        }
        // Separate setup and expiry budgets; the first allocation can
        // precede the second, and pre-stop datagrams may already be in flight.
        timeout(Duration::from_secs(u64::from(turn_ttl) + 2), async {
            while turn.stats().active != 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("server's advertised allocation lifetimes expired");
        let ended = turn.stats();
        assert_eq!(ended.expired, 2);
        assert_eq!(ended.released, 0);
    }
}

#[tokio::test]
async fn scoped_loopback_wss_applies_signed_video_and_closes_both_roles() {
    exercise(false, TestPath::DirectOnly, TestMedia::None).await;
}

#[tokio::test]
async fn scoped_loopback_wss_closes_both_roles_when_transport_drops() {
    exercise(true, TestPath::DirectOnly, TestMedia::None).await;
}

#[tokio::test]
async fn scoped_loopback_turn_selects_relay_and_allocations_expire_after_stop() {
    exercise(false, TestPath::RelayOnly, TestMedia::None).await;
}

#[tokio::test]
async fn scoped_combined_prefers_srflx_when_both_routes_are_available() {
    exercise(
        false,
        TestPath::PreferDirect { blocked: false },
        TestMedia::None,
    )
    .await;
}

#[tokio::test]
async fn scoped_combined_falls_back_to_turn_when_direct_checks_are_blocked() {
    exercise(
        false,
        TestPath::PreferDirect { blocked: true },
        TestMedia::None,
    )
    .await;
}

async fn loopback_access_units(
    traffic: TestMedia,
    producer: &mut crate::public_video_media::ScopedVideoSender,
    consumer: &mut crate::public_video_media::ScopedVideoReceiver,
) {
    use hub_encode_win::h264::{normalize, EncodedVideoFrame, Framing};
    const AU: &[u8] =
        include_bytes!("../../hub-rendezvous-probe/src/bin/v2_signaling/synthetic_32x32.h264");
    let (count, cadence, step_us, budget) = match traffic {
        TestMedia::Burst => (64, 5, 25_000, 2500),
        TestMedia::Sustained => (100, 40, 40_000, 7000),
        #[cfg(feature = "v2-public-video-60s-test")]
        TestMedia::SustainedMinute => (MINUTE_MEDIA_SECS * 25, 40, 40_000, 66_000),
        TestMedia::None => unreachable!("no media fixture"),
    };
    let (annex_b, _) = normalize(AU, Framing::AnnexB).unwrap();
    let begin = std::time::Instant::now();
    let period = Duration::from_millis(cadence);
    let mut tick = tokio::time::interval_at(Instant::now() + period, period);
    // No catch-up burst after a delayed poll of the one-minute fixture.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (mut packetizer, minute) = match traffic {
        #[cfg(feature = "v2-public-video-60s-test")]
        TestMedia::SustainedMinute => (crate::video::Packetizer::public_video_60s_test(), true),
        _ => (crate::video::Packetizer::default(), false),
    };
    let mut packets_for = |ordinal: u64| {
        let frame = EncodedVideoFrame {
            annex_b: AU.to_vec(),
            pts_100ns: (ordinal * step_us * 10) as i64,
            duration_100ns: (step_us * 10) as i64,
            nal_types: vec![7, 8, 5],
            is_idr: true,
            encode_elapsed_ms: 0.0,
        };
        let packets = packetizer.packets(frame, 102).unwrap();
        assert!(packets.len() >= 2);
        packets
    };
    timeout(Duration::from_millis(budget), async {
        tokio::join!(
            async {
                for ordinal in 0..count {
                    if minute {
                        tick.tick().await;
                    }
                    producer
                        .send_access_unit(packets_for(ordinal))
                        .await
                        .unwrap();
                    if !minute {
                        // Give the finite display queue room without polling
                        // the signaling path serially for each RTP packet.
                        tokio::time::sleep(period).await;
                    }
                }
            },
            async {
                for ordinal in 0..count {
                    let got = consumer.next_frame().await.unwrap();
                    assert_eq!(got.sequence, ordinal);
                    assert_eq!(got.timestamp_us, ordinal * step_us);
                    assert!(got.keyframe);
                    assert_eq!(got.annex_b, annex_b);
                }
            },
        );
    })
    .await
    .expect("continuous synthetic H.264 must not wait for WSS per packet");
    if traffic == TestMedia::Sustained || minute {
        let elapsed = begin.elapsed();
        let minimum = if minute {
            eprintln!("verified {count} paced loopback TURN access units in {elapsed:?}");
            MINUTE_MEDIA_SECS
        } else {
            3
        };
        assert!(elapsed >= Duration::from_secs(minimum));
        producer.send_access_unit(packets_for(count)).await.unwrap();
        timeout(Duration::from_secs(1), async {
            while !consumer.fixture_has_queued_frame() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("at least one verified but unconsumed frame at teardown");
    }
}

#[tokio::test]
async fn scoped_wss_carries_continuous_synthetic_h264_and_clears_media_on_stop() {
    exercise(false, TestPath::DirectOnly, TestMedia::Burst).await;
}

#[tokio::test]
async fn scoped_wss_closes_media_when_signaling_drops() {
    exercise(true, TestPath::DirectOnly, TestMedia::Burst).await;
}

#[tokio::test]
async fn scoped_wss_carries_continuous_synthetic_h264_through_mock_turn() {
    exercise(false, TestPath::RelayOnly, TestMedia::Sustained).await;
}

#[tokio::test]
async fn scoped_wss_clears_media_when_signaling_drops_during_sustained_mock_turn() {
    exercise(true, TestPath::RelayOnly, TestMedia::Sustained).await;
}

#[cfg(feature = "v2-public-video-60s-test")]
#[tokio::test]
#[ignore = "real-time 60 s loopback TURN soak test, followed by allocation expiry"]
async fn scoped_wss_sustains_minute_of_h264_over_mock_turn_and_revokes_on_stop() {
    exercise(false, TestPath::RelayOnly, TestMedia::SustainedMinute).await;
}

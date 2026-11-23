//! Native signed signaling over the actual Axum TLS route. No WebRTC/media I/O.
use super::{
    tls_smoke::{fixture, make_ca, start_tls},
    *,
};
use hub_identity::{
    pairing::{PairingCandidate, VerifiedPeer},
    Identity,
};
use hub_webrtc::{
    authenticated_relay::LoopbackTlsTrust,
    ice_policy::IceMode,
    lan_wire::Key,
    public_video::PublicVideoScope,
    public_video_wss::{PublicVideoWss, PublicVideoWssConfig, PublicVideoWssEvent},
};
use hub_webrtc::{pinned_runtime::PinnedRuntime, video::FMTP};
use rtc::rtp::{header::Header, packet::Packet};
use rtc::{
    ice::{mdns::MulticastDnsMode, network_type::NetworkType},
    peer_connection::transport::RTCDtlsTransportState,
    rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
        RtpCodecKind,
    },
};
use std::time::{SystemTime, UNIX_EPOCH};
use webrtc::media_stream::track_remote::TrackRemote;
#[path = "public_video_fixture_net.rs"]
mod fixture_net;
use fixture_net::{FixtureNat, FixtureSide};
use tokio::time::{sleep, timeout};
use webrtc::peer_connection::{RTCIceCandidate, RTCIceCandidateType, RTCIceProtocol};
use webrtc::{
    media_stream::{track_local::static_rtp::TrackLocalStaticRTP, MediaStreamTrack},
    peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCIceCandidateInit, RTCPeerConnectionIceEvent, RTCSessionDescription,
        SettingEngineBuilder,
    },
    rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit},
};

fn fixture_sdp(answer: bool, candidate: &RTCIceCandidate) -> String {
    let direction = if answer { "sendonly" } else { "recvonly" };
    let setup = if answer { "active" } else { "actpass" };
    let hash = vec!["AA"; 32].join(":");
    format!(
        concat!("v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0\r\n",
        "m=video 9 UDP/TLS/RTP/SAVPF 102\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\na={}\r\na=setup:{}\r\n",
        "a=ice-ufrag:abcd\r\na=ice-pwd:abcdefghijklmnopqrstuvwx\r\na=fingerprint:sha-256 {}\r\n",
        "a=rtcp-mux\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\n",
        "a=candidate:x 1 udp 1 {} {} typ {} raddr 0.0.0.0 rport 0\r\na=end-of-candidates\r\n"),
        direction, setup, hash, candidate.address, candidate.port, candidate.typ
    )
}

fn packet_candidate(relay: bool, answer: bool) -> RTCIceCandidate {
    RTCIceCandidate {
        address: if relay {
            "93.184.216.34"
        } else if answer {
            "8.8.4.4"
        } else {
            "8.8.8.8"
        }
        .into(),
        port: if answer { 49172 } else { 49171 },
        typ: if relay {
            RTCIceCandidateType::Relay
        } else {
            RTCIceCandidateType::Srflx
        },
        protocol: RTCIceProtocol::Udp,
        component: 1,
        ..Default::default()
    }
}
const WAIT: Duration = Duration::from_secs(3);

struct VideoIce {
    candidates: mpsc::Sender<RTCIceCandidateInit>,
    tracks: mpsc::Sender<()>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for VideoIce {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if event.candidate.component == 1 {
            let _ = self.candidates.try_send(event.candidate.to_json().unwrap());
        }
    }

    async fn on_track(&self, _track: Arc<dyn TrackRemote>) {
        let _ = self.tracks.try_send(());
    }
}

fn fixture_codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: "video/H264".into(),
        clock_rate: 90_000,
        sdp_fmtp_line: FMTP.into(),
        ..Default::default()
    }
}

async fn video_peer(
    nat: Option<(&FixtureNat, FixtureSide)>,
) -> (
    impl PeerConnection,
    mpsc::Receiver<RTCIceCandidateInit>,
    mpsc::Receiver<()>,
) {
    let (ice_tx, ice_rx) = mpsc::channel(8);
    let (track_tx, track_rx) = mpsc::channel(1);
    let local = Ipv4Addr::LOCALHOST.into();
    let mut engine = MediaEngine::default();
    engine
        .register_codec(
            RTCRtpCodecParameters {
                rtp_codec: fixture_codec(),
                payload_type: 102,
            },
            RtpCodecKind::Video,
        )
        .unwrap();
    let builder = PeerConnectionBuilder::new()
        .with_media_engine(engine)
        .with_setting_engine(
            SettingEngineBuilder::new()
                .with_include_loopback_candidate(true)
                .with_multicast_dns_mode(MulticastDnsMode::Disabled)
                .with_network_types(vec![NetworkType::Udp4])
                .build(),
        )
        .with_udp_addrs(vec!["127.0.0.1:0"])
        .with_handler(Arc::new(VideoIce {
            candidates: ice_tx,
            tracks: track_tx,
        }));
    let pc = if let Some((fixture, side)) = nat {
        builder
            .with_host_candidates(false)
            .with_configuration(
                webrtc::peer_connection::RTCConfigurationBuilder::default()
                    .with_ice_servers(vec![fixture.stun_server()])
                    .build(),
            )
            .with_runtime(fixture.runtime(side))
    } else {
        builder.with_runtime(Arc::new(PinnedRuntime {
            inner: webrtc::runtime::default_runtime().unwrap(),
            local,
            peer: local,
            counters: Default::default(),
        }))
    };
    let pc = pc.build().await.unwrap();
    (pc, ice_rx, track_rx)
}

/// Real native ICE on loopback, no data channel, no screen capture and no RTP write.
/// This pair is deliberately *not* the separate connection described by the signed fixture SDP.
async fn check_native_unsigned_pair(wss: &mut PublicVideoWss<'_>) {
    let (host, mut host_ice, _) = video_peer(None).await;
    let (viewer, mut viewer_ice, mut viewer_tracks) = video_peer(None).await;
    viewer
        .add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let sender = host
        .add_track(Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            "test-video".into(),
            "fixture-only".into(),
            "no capture".into(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(42),
                    ..Default::default()
                },
                codec: fixture_codec(),
                active: true,
                max_bitrate: 100_000,
                max_framerate: Some(30.0),
                scale_resolution_down_by: None,
            }],
        ))))
        .await
        .unwrap();

    let offer = viewer.create_offer(None).await.unwrap();
    viewer.set_local_description(offer.clone()).await.unwrap();
    host.set_remote_description(RTCSessionDescription::offer(offer.sdp).unwrap())
        .await
        .unwrap();
    let answer = host.create_answer(None).await.unwrap();
    host.set_local_description(answer.clone()).await.unwrap();
    viewer
        .set_remote_description(RTCSessionDescription::answer(answer.sdp).unwrap())
        .await
        .unwrap();

    let (local, remote) = timeout(WAIT, async {
        tokio::join!(host_ice.recv(), viewer_ice.recv())
    })
    .await
    .unwrap();
    host.add_ice_candidate(remote.unwrap()).await.unwrap();
    viewer.add_ice_candidate(local.unwrap()).await.unwrap();
    timeout(WAIT, async {
        loop {
            if let Some(dtls) = sender.transport().await.unwrap() {
                if dtls.state().await.unwrap() != RTCDtlsTransportState::Connected {
                    sleep(Duration::from_millis(10)).await;
                    continue;
                }
                if let Some(pair) = dtls
                    .ice_transport()
                    .get_selected_candidate_pair()
                    .await
                    .unwrap()
                {
                    assert_eq!(pair.local().address, "127.0.0.1");
                    assert_eq!(pair.remote().address, "127.0.0.1");
                    break;
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("video sender must have a live UDP loopback pair");

    assert!(matches!(
        wss.send_checked_rtp(&host, synthetic_rtp(1)).await,
        Err("PUBLIC_VIDEO_ICE_PAIR_REJECTED")
    ));
    assert!(timeout(Duration::from_millis(100), viewer_tracks.recv())
        .await
        .is_err());
    host.close().await.unwrap();
    viewer.close().await.unwrap();
}

fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
    let peer = peer.public();
    local
        .verify_peer(
            &PairingCandidate::parse(&peer.pairing_code()).unwrap(),
            &peer.fingerprint,
        )
        .unwrap()
}

async fn assert_room_released(app: &App) {
    timeout(WAIT, async {
        while !app.rooms.lock().unwrap().is_empty() || app.active.load(Ordering::Acquire) != 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("server did not release ended room");
}

#[derive(Clone, Copy)]
enum Scenario {
    RoundTrip,
    RelayOnly,
    UnsignedSelectedPort,
    LiveUnsignedPair,
    LiveSignedMedia,
    LiveMediaStop,
    LiveMediaPeerDrop,
    LiveUnavailable,
    BadLocalSdp,
    WrongTlsTrust,
    OtherRouteScope,
    OtherPinnedDevice,
    DuplicateOffer,
    PeerLeavesBeforeAnswer,
}

async fn exchange(scenario: Scenario) {
    let (app, token) = fixture();
    let ca = make_ca();
    let wrong_ca = make_ca();
    let trusted_ca = if matches!(scenario, Scenario::WrongTlsTrust) {
        &wrong_ca
    } else {
        &ca
    };
    let trust = LoopbackTlsTrust::from_ca_der(trusted_ca.der().as_ref()).unwrap();
    let (address, server) = start_tls(&ca, "127.0.0.1", app.clone()).await;
    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    let other = Identity::generate_ephemeral().unwrap();
    let host_pin = pin(
        &host,
        if matches!(scenario, Scenario::OtherPinnedDevice) {
            &other
        } else {
            &viewer
        },
    );
    let viewer_pin = pin(&viewer, &host);

    // Checked, signed context only. All socket I/O is to the localhost TLS fixture.
    let scope = PublicVideoScope {
        wss: "93.184.216.34:8443".parse().unwrap(),
        ca_sha256: ring::digest::digest(&ring::digest::SHA256, ca.der().as_ref())
            .as_ref()
            .try_into()
            .unwrap(),
        turn: "93.184.216.34:3478".parse().unwrap(),
        relay_ports: (49152, 49200),
        mode: if matches!(scenario, Scenario::RelayOnly) {
            IceMode::RelayOnly
        } else {
            IceMode::DirectPreferred
        },
        expires_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 120,
    };
    let room = Uuid::new_v4();
    let wire_nonce = Uuid::new_v4().to_string();
    let key = Arc::new(Key::generate());
    let host_pending = PublicVideoWss::connect_loopback_wss(PublicVideoWssConfig {
        address,
        token: &token,
        room,
        role: Role::Host,
        pairing_key: key.clone(),
        wire_nonce: wire_nonce.clone(),
        session_nonce: [9; 32],
        trust: &trust,
        local_identity: &host,
        peer: &host_pin,
        scope,
    })
    .await;
    if matches!(scenario, Scenario::WrongTlsTrust) {
        assert!(matches!(host_pending, Err("PUBLIC_VIDEO_WSS_TRANSPORT")));
        assert_room_released(&app).await;
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }
    let host_pending = host_pending
        .unwrap()
        .wait_for_host_registration()
        .await
        .unwrap();
    let viewer_scope = if matches!(scenario, Scenario::OtherRouteScope) {
        PublicVideoScope {
            mode: IceMode::RelayOnly,
            ..scope
        }
    } else {
        scope
    };
    let viewer_pending = PublicVideoWss::connect_loopback_wss(PublicVideoWssConfig {
        address,
        token: &token,
        room,
        role: Role::Viewer,
        pairing_key: key,
        wire_nonce,
        session_nonce: [9; 32],
        trust: &trust,
        local_identity: &viewer,
        peer: &viewer_pin,
        scope: viewer_scope,
    })
    .await
    .unwrap();
    let (h, v) = tokio::join!(host_pending.establish(), viewer_pending.establish());
    let (mut h, mut v) = (h.unwrap(), v.unwrap());
    if matches!(
        scenario,
        Scenario::LiveMediaStop | Scenario::LiveMediaPeerDrop
    ) {
        signed_media_lifecycle(h, v, scenario).await;
        assert_room_released(&app).await;
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }
    if matches!(scenario, Scenario::LiveSignedMedia) {
        positive_signed_rtp(&mut h, &mut v).await;
        h.leave().await.unwrap();
        assert!(matches!(
            v.next_event(WAIT).await.unwrap(),
            PublicVideoWssEvent::PeerLeft
        ));
        assert_room_released(&app).await;
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }
    let is_relay = matches!(scenario, Scenario::RelayOnly | Scenario::OtherRouteScope);
    let offer_candidate = packet_candidate(is_relay, false);
    let mut answer_candidate = packet_candidate(is_relay, true);
    let offer = fixture_sdp(false, &offer_candidate);
    let answer = fixture_sdp(true, &answer_candidate);
    if matches!(scenario, Scenario::BadLocalSdp) {
        assert_eq!(
            v.send_signal(
                SignalKind::Offer,
                offer.replace("a=rtcp-mux", "a=sctp-port:5000")
            )
            .await,
            Err("PUBLIC_VIDEO_SDP_POLICY")
        );
        assert!(matches!(
            h.next_event(WAIT).await.unwrap(),
            PublicVideoWssEvent::PeerLeft
        ));
    } else {
        v.send_signal(SignalKind::Offer, offer.clone())
            .await
            .unwrap();
    }
    match scenario {
        Scenario::WrongTlsTrust
        | Scenario::LiveSignedMedia
        | Scenario::LiveMediaStop
        | Scenario::LiveMediaPeerDrop => unreachable!(),
        Scenario::BadLocalSdp => (),
        Scenario::OtherRouteScope | Scenario::OtherPinnedDevice => {
            assert!(matches!(
                h.next_event(WAIT).await,
                Err("PUBLIC_VIDEO_PROOF_REJECTED")
            ));
            assert!(matches!(
                h.next_event(WAIT).await,
                Err("PUBLIC_VIDEO_WSS_CLOSED")
            ));
            assert!(matches!(
                v.next_event(WAIT).await.unwrap(),
                PublicVideoWssEvent::PeerLeft
            ));
        }
        Scenario::RoundTrip
        | Scenario::RelayOnly
        | Scenario::UnsignedSelectedPort
        | Scenario::LiveUnsignedPair
        | Scenario::LiveUnavailable
        | Scenario::DuplicateOffer
        | Scenario::PeerLeavesBeforeAnswer => {
            assert!(matches!(
                h.next_event(WAIT).await.unwrap(),
                PublicVideoWssEvent::Signal { kind: SignalKind::Offer, data } if data == offer
            ));
            if matches!(scenario, Scenario::DuplicateOffer) {
                assert_eq!(
                    v.send_signal(SignalKind::Offer, offer).await,
                    Err("PUBLIC_VIDEO_WSS_PROTOCOL")
                );
                assert!(matches!(
                    h.next_event(WAIT).await.unwrap(),
                    PublicVideoWssEvent::PeerLeft
                ));
            } else if matches!(scenario, Scenario::PeerLeavesBeforeAnswer) {
                drop(v);
                assert!(matches!(
                    h.next_event(WAIT).await.unwrap(),
                    PublicVideoWssEvent::PeerLeft
                ));
            } else {
                h.send_signal(SignalKind::Answer, answer.clone())
                    .await
                    .unwrap();
                assert!(matches!(
                    v.next_event(WAIT).await.unwrap(),
                    PublicVideoWssEvent::Signal { kind: SignalKind::Answer, data } if data == answer
                ));
                let expected = if is_relay { "relay" } else { "direct" };
                assert_eq!(
                    v.check_selected_pair(&offer_candidate, &answer_candidate)
                        .unwrap()
                        .path,
                    expected
                );
                assert_eq!(
                    h.check_selected_pair(&answer_candidate, &offer_candidate)
                        .unwrap()
                        .path,
                    expected
                );
                if matches!(scenario, Scenario::LiveUnsignedPair) {
                    check_native_unsigned_pair(&mut h).await;
                } else if matches!(scenario, Scenario::LiveUnavailable) {
                    let (unstarted, _, _) = video_peer(None).await;
                    assert!(matches!(
                        h.check_live_video_pair(&unstarted).await,
                        Err("PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE")
                    ));
                    unstarted.close().await.unwrap();
                } else if matches!(scenario, Scenario::UnsignedSelectedPort) {
                    answer_candidate.port += 1;
                    assert!(matches!(
                        h.check_selected_pair(&answer_candidate, &offer_candidate),
                        Err("PUBLIC_VIDEO_ICE_PAIR_REJECTED")
                    ));
                } else {
                    h.leave().await.unwrap();
                }
                assert!(matches!(
                    v.next_event(WAIT).await.unwrap(),
                    PublicVideoWssEvent::PeerLeft
                ));
            }
        }
    }
    assert_room_released(&app).await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn public_video_wss_signs_exact_offer_and_answer_then_releases_on_stop() {
    exchange(Scenario::RoundTrip).await;
}

#[tokio::test]
async fn public_video_wss_rejects_scope_mismatch_and_wrong_device_pin() {
    exchange(Scenario::OtherRouteScope).await;
    exchange(Scenario::OtherPinnedDevice).await;
}

#[tokio::test]
async fn public_video_wss_rejects_duplicate_offer_and_ends_on_peer_disconnect() {
    exchange(Scenario::DuplicateOffer).await;
    exchange(Scenario::PeerLeavesBeforeAnswer).await;
}

#[tokio::test]
async fn public_video_wss_rejects_an_untrusted_tls_ca() {
    exchange(Scenario::WrongTlsTrust).await;
}

#[tokio::test]
async fn public_video_wss_checks_relay_only_and_rejects_unsigned_selected_port() {
    exchange(Scenario::RelayOnly).await;
    exchange(Scenario::UnsignedSelectedPort).await;
}

#[tokio::test]
async fn public_video_wss_refuses_invalid_local_video_before_sending() {
    exchange(Scenario::BadLocalSdp).await;
}

#[tokio::test]
async fn public_video_wss_rejects_real_video_transport_pair_absent_from_signed_sdp() {
    exchange(Scenario::LiveUnsignedPair).await;
}

#[tokio::test]
async fn public_video_wss_rejects_unavailable_live_video_pair() {
    exchange(Scenario::LiveUnavailable).await;
}

fn signed_fixture_video_sdp(
    sdp: &str,
    candidate: RTCIceCandidateInit,
    side: FixtureSide,
) -> String {
    let fields: Vec<_> = candidate.candidate.split_whitespace().collect();
    assert!(fields.len() >= 8);
    assert_eq!(fields[4], side.ip().to_string());
    assert_eq!(fields[7], "srflx");
    let mut lines = Vec::new();
    for line in sdp.lines() {
        if line.starts_with("a=candidate:") || line == "a=end-of-candidates" {
            continue;
        }
        if line.starts_with("o=") {
            lines.push("o=- 1 1 IN IP4 0.0.0.0");
        } else if line.starts_with("c=") {
            lines.push("c=IN IP4 0.0.0.0");
        } else if line.starts_with("a=rtcp:") {
            lines.push("a=rtcp:9 IN IP4 0.0.0.0");
        } else {
            lines.push(line);
        }
    }
    format!(
        "{}\r\na={} raddr 0.0.0.0 rport 0\r\na=end-of-candidates\r\n",
        lines.join("\r\n"),
        fields[..8].join(" ")
    )
}

// Synthetic H.264 RTP payload for transport tests, not a decoder fixture.
fn synthetic_rtp(sequence_number: u16) -> Packet {
    Packet {
        header: Header {
            version: 2,
            marker: true,
            payload_type: 102,
            sequence_number,
            timestamp: 9_000 * u32::from(sequence_number),
            ssrc: 42,
            ..Default::default()
        },
        payload: vec![0x65, 0x88, 0x84, 0x00].into(),
    }
}

struct SignedVideoPeers {
    host: Arc<dyn PeerConnection>,
    viewer: Arc<dyn PeerConnection>,
    viewer_tracks: mpsc::Receiver<()>,
    _nat: FixtureNat,
}

async fn negotiate_signed_video(
    h: &mut PublicVideoWss<'_>,
    v: &mut PublicVideoWss<'_>,
) -> SignedVideoPeers {
    let nat = FixtureNat::start().await.unwrap();
    let (host, mut host_ice, _) = video_peer(Some((&nat, FixtureSide::Host))).await;
    let (viewer, mut viewer_ice, viewer_tracks) =
        video_peer(Some((&nat, FixtureSide::Viewer))).await;
    viewer
        .add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let sender = host
        .add_track(Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            "signed-fixture-video".into(),
            "fixture-only".into(),
            "no capture".into(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(42),
                    ..Default::default()
                },
                codec: fixture_codec(),
                active: true,
                max_bitrate: 100_000,
                max_framerate: Some(30.0),
                scale_resolution_down_by: None,
            }],
        ))))
        .await
        .unwrap();

    let offer = viewer.create_offer(None).await.unwrap();
    viewer.set_local_description(offer.clone()).await.unwrap();
    let offer = signed_fixture_video_sdp(
        &offer.sdp,
        timeout(WAIT, viewer_ice.recv()).await.unwrap().unwrap(),
        FixtureSide::Viewer,
    );
    v.send_signal(SignalKind::Offer, offer).await.unwrap();
    let PublicVideoWssEvent::Signal {
        kind: SignalKind::Offer,
        data,
    } = h.next_event(WAIT).await.unwrap()
    else {
        panic!("expected verified offer");
    };
    host.set_remote_description(RTCSessionDescription::offer(data).unwrap())
        .await
        .unwrap();

    let answer = host.create_answer(None).await.unwrap();
    host.set_local_description(answer.clone()).await.unwrap();
    let answer = signed_fixture_video_sdp(
        &answer.sdp,
        timeout(WAIT, host_ice.recv()).await.unwrap().unwrap(),
        FixtureSide::Host,
    );
    h.send_signal(SignalKind::Answer, answer).await.unwrap();
    let PublicVideoWssEvent::Signal {
        kind: SignalKind::Answer,
        data,
    } = v.next_event(WAIT).await.unwrap()
    else {
        panic!("expected verified answer");
    };
    viewer
        .set_remote_description(RTCSessionDescription::answer(data).unwrap())
        .await
        .unwrap();

    timeout(WAIT, async {
        loop {
            if let Some(dtls) = sender.transport().await.unwrap() {
                if dtls.state().await.unwrap() == RTCDtlsTransportState::Connected {
                    break;
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("simulated NAT media DTLS failed");

    assert_eq!(h.check_live_video_pair(&host).await.unwrap().path, "direct");
    SignedVideoPeers {
        host: Arc::new(host),
        viewer: Arc::new(viewer),
        viewer_tracks,
        _nat: nat,
    }
}

async fn positive_signed_rtp(h: &mut PublicVideoWss<'_>, v: &mut PublicVideoWss<'_>) {
    let SignedVideoPeers {
        host,
        viewer,
        mut viewer_tracks,
        _nat,
    } = negotiate_signed_video(h, v).await;
    h.send_checked_rtp(host.as_ref(), synthetic_rtp(1))
        .await
        .unwrap();
    timeout(WAIT, viewer_tracks.recv())
        .await
        .expect("viewer track startup")
        .unwrap();
    h.send_checked_rtp(host.as_ref(), synthetic_rtp(2))
        .await
        .unwrap();
    let observed = v.recv_checked_rtp(viewer.as_ref()).await.unwrap();
    assert_eq!(observed.header.payload_type, 102);
    assert_eq!(observed.payload, synthetic_rtp(1).payload);
    assert!([1, 2].contains(&observed.header.sequence_number));
    host.close().await.unwrap();
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn public_video_wss_carries_synthetic_rtp_after_live_pair_verification() {
    exchange(Scenario::LiveSignedMedia).await;
}

async fn signed_media_lifecycle(
    mut h: PublicVideoWss<'_>,
    mut v: PublicVideoWss<'_>,
    scenario: Scenario,
) {
    use hub_webrtc::public_video_lifecycle::{supervise_video, PublicVideoEnd};
    let SignedVideoPeers {
        host,
        viewer,
        mut viewer_tracks,
        _nat,
    } = negotiate_signed_video(&mut h, &mut v).await;
    if matches!(scenario, Scenario::LiveMediaPeerDrop) {
        // Open the viewer's native track, then drop the transport without Leave.
        // Keep the host PC alive while checking the viewer closes on the WSS event.
        h.send_checked_rtp(host.as_ref(), synthetic_rtp(1))
            .await
            .unwrap();
        timeout(WAIT, viewer_tracks.recv()).await.unwrap().unwrap();
        let (watch, driver) = supervise_video(v, viewer.clone());
        drop(h);
        let (end, observed) = timeout(WAIT, async { tokio::join!(driver, watch.wait_closed()) })
            .await
            .unwrap();
        assert_eq!(end.reason, PublicVideoEnd::PeerLeft);
        assert_eq!(end.cleanup, Ok(()));
        assert_eq!(observed.unwrap().reason, PublicVideoEnd::PeerLeft);
        assert!(watch.recv_rtp().await.is_err());
        assert!(viewer.create_offer(None).await.is_err());
        host.close().await.unwrap();
        return;
    }

    let (source, host_driver) = supervise_video(h, host.clone());
    let (sink, viewer_driver) = supervise_video(v, viewer.clone());
    let exercise = async {
        source.send_rtp(synthetic_rtp(1)).await.unwrap();
        timeout(WAIT, viewer_tracks.recv()).await.unwrap().unwrap();
        source.send_rtp(synthetic_rtp(2)).await.unwrap();
        let packet = sink.recv_rtp().await.unwrap();
        assert_eq!(packet.payload, synthetic_rtp(1).payload);
        source.stop();
        source.stop(); // idempotent
        assert!(source.send_rtp(synthetic_rtp(3)).await.is_err());
        let host_end = source.wait_closed().await.unwrap();
        let viewer_end = sink.wait_closed().await.unwrap();
        assert!(sink.recv_rtp().await.is_err());
        (host_end, viewer_end)
    };
    let (host_end, viewer_end, (seen_host, seen_viewer)) = timeout(WAIT, async {
        tokio::join!(host_driver, viewer_driver, exercise)
    })
    .await
    .expect("stopping must finish both loopback media actors");
    for (actual, observed, expected) in [
        (host_end, seen_host, PublicVideoEnd::Stopped),
        (viewer_end, seen_viewer, PublicVideoEnd::PeerLeft),
    ] {
        assert_eq!(actual.reason, expected);
        assert_eq!(observed.reason, expected);
        assert_eq!(actual.cleanup, Ok(()));
    }
    assert!(host.create_offer(None).await.is_err());
    assert!(viewer.create_offer(None).await.is_err());
}

#[tokio::test]
async fn public_video_media_stop_releases_both_native_peers() {
    exchange(Scenario::LiveMediaStop).await;
}

#[tokio::test]
async fn public_video_media_abrupt_signal_loss_closes_live_viewer() {
    exchange(Scenario::LiveMediaPeerDrop).await;
}

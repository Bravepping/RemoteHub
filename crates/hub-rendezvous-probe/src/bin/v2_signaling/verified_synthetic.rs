//! A bounded synthetic run for the opt-in desktop diagnostic. Public-looking
//! SDP labels are fixture metadata; the only real I/O is IPv4 loopback.
use super::{router, Access, App};
use hub_identity::{
    pairing::{PairingCandidate, VerifiedPeer},
    Identity,
};
use hub_signal_v2::{LoopbackTlsTrust, RelayToken, Role, SignalKind};
use hub_webrtc::{
    ice_policy::IceMode,
    lan_wire::Key,
    public_video::PublicVideoScope,
    public_video_lifecycle::{supervise_video, PublicVideoEnd, VerifiedVideoFrame},
    public_video_wss::{PublicVideoWss, PublicVideoWssConfig, PublicVideoWssEvent},
    video::FMTP,
};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use ring::rand::{SecureRandom, SystemRandom};
use rtc::{
    ice::{mdns::MulticastDnsMode, network_type::NetworkType},
    peer_connection::transport::RTCDtlsTransportState,
    rtp::{codec::h264::H264Payloader, header::Header, packet::Packet, packetizer::Payloader},
    rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
        RtpCodecKind,
    },
};
use std::{
    collections::HashMap,
    future::Future,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, watch},
    time::{sleep, timeout},
};
use uuid::Uuid;
use webrtc::{
    media_stream::{
        track_local::static_rtp::TrackLocalStaticRTP, track_remote::TrackRemote, MediaStreamTrack,
    },
    peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfigurationBuilder, RTCIceCandidateInit, RTCPeerConnectionIceEvent,
        RTCSessionDescription, SettingEngineBuilder,
    },
    rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit},
};
use zeroize::Zeroizing;

#[path = "public_video_fixture_net.rs"]
mod fixture_net;
use fixture_net::{FixtureNat, FixtureSide};

const WAIT: Duration = Duration::from_secs(3);
const DEADLINE: Duration = Duration::from_secs(9);
const STOPPED: &str = "VERIFIED_SYNTHETIC_STOPPED";
const TIMEOUT: &str = "VERIFIED_SYNTHETIC_TIMEOUT";
const TLS: &str = "VERIFIED_SYNTHETIC_TLS";
const IDENTITY: &str = "VERIFIED_SYNTHETIC_IDENTITY";
const NATIVE: &str = "VERIFIED_SYNTHETIC_WEBRTC";
const FIXTURE: &str = "VERIFIED_SYNTHETIC_H264";
const CLEANUP: &str = "VERIFIED_SYNTHETIC_CLEANUP";
const SSRC: u32 = 42;
const PAYLOAD_TYPE: u8 = 102;
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// Verify two ephemeral pinned identities, then send and reassemble two real
/// H.264 access units through native DTLS/SRTP over loopback. The built-in
/// source is a 32×32 grayscale pattern with luma quadrants 36, 94, 154, 218.
/// There is no decoder, capture, input, file writes, or service configuration.
pub async fn verified_synthetic_frames(
    stop: watch::Receiver<bool>,
) -> Result<Vec<VerifiedVideoFrame>, &'static str> {
    timeout(DEADLINE, run(stop)).await.map_err(|_| TIMEOUT)?
}

async fn run(mut stop: watch::Receiver<bool>) -> Result<Vec<VerifiedVideoFrame>, &'static str> {
    let relay = startup(&mut stop, FixtureRelay::start()).await?;
    let result = run_session(&mut stop, &relay).await;
    let released = relay.release().await;
    relay.shutdown().await?;
    released?;
    if *stop.borrow() || stop.has_changed().is_err() {
        return result.and(Err(STOPPED));
    }
    result
}

async fn run_session(
    stop: &mut watch::Receiver<bool>,
    relay: &FixtureRelay,
) -> Result<Vec<VerifiedVideoFrame>, &'static str> {
    let nat = startup(stop, async {
        FixtureNat::start().await.map_err(|_| NATIVE)
    })
    .await?;
    let host = Identity::generate_ephemeral().map_err(|_| IDENTITY)?;
    let viewer = Identity::generate_ephemeral().map_err(|_| IDENTITY)?;
    let host_pin = pin(&host, &viewer)?;
    let viewer_pin = pin(&viewer, &host)?;
    let scope = relay.scope()?;
    let room = Uuid::new_v4();
    let wire_nonce = Uuid::new_v4().to_string();
    let mut session_nonce = [0; 32];
    SystemRandom::new()
        .fill(&mut session_nonce)
        .map_err(|_| IDENTITY)?;
    let key = Arc::new(Key::generate());
    let make = |role, local_identity, peer| PublicVideoWssConfig {
        address: relay.address,
        token: &relay.token,
        room,
        role,
        pairing_key: key.clone(),
        wire_nonce: wire_nonce.clone(),
        session_nonce,
        trust: &relay.trust,
        local_identity,
        peer,
        scope,
    };
    let (h, v) = startup(stop, async {
        let h = PublicVideoWss::connect_loopback_wss(make(Role::Host, &host, &host_pin))
            .await?
            .wait_for_host_registration()
            .await?;
        let v =
            PublicVideoWss::connect_loopback_wss(make(Role::Viewer, &viewer, &viewer_pin)).await?;
        tokio::try_join!(h.establish(), v.establish())
    })
    .await?;
    let mut h = h;
    let mut v = v;
    let (host_pc, viewer_pc, mut tracks) = startup(stop, negotiate(&nat, &mut h, &mut v)).await?;
    let payload = fixture_payload()?;
    let (sender, host_task) = supervise_video(h, host_pc);
    let (viewer, viewer_task) = supervise_video(v, viewer_pc);
    let media = async {
        let mut display = viewer.into_display_reader(SSRC);
        let transfer = async {
            sender.send_rtp(picture(1, &payload)).await?;
            tracks.recv().await.ok_or(NATIVE)?;
            sender.send_rtp(picture(2, &payload)).await?;
            let first = display.next_frame().await?;
            sender.send_rtp(picture(3, &payload)).await?;
            let second = display.next_frame().await?;
            if !first.keyframe
                || !second.keyframe
                || first.sequence != 0
                || second.sequence != 1
                || first.timestamp_us != 0
                || second.timestamp_us <= first.timestamp_us
            {
                return Err(FIXTURE);
            }
            Ok(vec![first, second])
        };
        let result = startup(stop, transfer).await;
        // The test owns both peers. Stop the viewer first so it can no longer
        // release a frame even while the sender is finishing cleanup.
        display.stop();
        sender.stop();
        result
    };
    let (host_end, viewer_end, frames) = tokio::join!(host_task, viewer_task, media);
    if host_end.cleanup.is_err() || viewer_end.cleanup.is_err() {
        return Err(CLEANUP);
    }
    let frames = frames?;
    if host_end.reason != PublicVideoEnd::Stopped
        || !matches!(
            viewer_end.reason,
            PublicVideoEnd::Stopped | PublicVideoEnd::PeerLeft
        )
    {
        return Err(CLEANUP);
    }
    Ok(frames)
}

async fn startup<T>(
    stop: &mut watch::Receiver<bool>,
    future: impl Future<Output = Result<T, &'static str>>,
) -> Result<T, &'static str> {
    if *stop.borrow() {
        return Err(STOPPED);
    }
    tokio::select! {
        biased;
        _ = stop.changed() => Err(STOPPED),
        result = timeout(WAIT, future) => result.map_err(|_| TIMEOUT)?,
    }
}

fn pin(local: &Identity, peer: &Identity) -> Result<VerifiedPeer, &'static str> {
    let peer = peer.public();
    let candidate = PairingCandidate::parse(&peer.pairing_code()).map_err(|_| IDENTITY)?;
    local
        .verify_peer(&candidate, &peer.fingerprint)
        .map_err(|_| IDENTITY)
}

struct FixtureRelay {
    address: SocketAddr,
    app: Arc<App>,
    token: RelayToken,
    trust: LoopbackTlsTrust,
    ca_sha256: [u8; 32],
    server: tokio::task::JoinHandle<()>,
}

impl FixtureRelay {
    async fn start() -> Result<Self, &'static str> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut ca = CertificateParams::default();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate().map_err(|_| TLS)?)
            .map_err(|_| TLS)?;
        let trust = LoopbackTlsTrust::from_ca_der(ca.der().as_ref()).map_err(|_| TLS)?;
        let ca_sha256 = ring::digest::digest(&ring::digest::SHA256, ca.der().as_ref())
            .as_ref()
            .try_into()
            .map_err(|_| TLS)?;
        let mut params = CertificateParams::new(vec![LOOPBACK.to_string()]).map_err(|_| TLS)?;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().map_err(|_| TLS)?;
        let certificate = params.signed_by(&key, &ca).map_err(|_| TLS)?;
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            certificate.pem().into_bytes(),
            key.serialize_pem().into_bytes(),
        )
        .await
        .map_err(|_| TLS)?;
        let listener = TcpListener::bind((LOOPBACK, 0)).map_err(|_| TLS)?;
        listener.set_nonblocking(true).map_err(|_| TLS)?;
        let address = listener.local_addr().map_err(|_| TLS)?;
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let typed = RelayToken::new(token.clone()).map_err(|_| TLS)?;
        let app = Arc::new(App {
            access: Access::Loopback(Zeroizing::new(token)),
            rooms: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
        });
        let service = router(app.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let server = axum_server::from_tcp_rustls(listener, tls).map_err(|_| TLS)?;
        let server = tokio::spawn(async move {
            let _ = server.serve(service).await;
        });
        Ok(Self {
            address,
            app,
            token: typed,
            trust,
            ca_sha256,
            server,
        })
    }

    fn scope(&self) -> Result<PublicVideoScope, &'static str> {
        // This literal is solely inside the signature, never a dial target.
        let label = Ipv4Addr::new(93, 184, 216, 34);
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TLS)?
            .as_secs()
            .checked_add(60)
            .ok_or(TLS)?;
        Ok(PublicVideoScope {
            wss: SocketAddrV4::new(label, 8443),
            ca_sha256: self.ca_sha256,
            turn: SocketAddrV4::new(label, 3478),
            relay_ports: (49152, 49200),
            mode: IceMode::DirectPreferred,
            expires_at,
        })
    }

    async fn release(&self) -> Result<(), &'static str> {
        timeout(Duration::from_secs(1), async {
            loop {
                let empty = self.app.rooms.lock().map_err(|_| CLEANUP)?.is_empty();
                if empty && self.app.active.load(Ordering::Acquire) == 0 {
                    return Ok(());
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| CLEANUP)?
    }

    async fn shutdown(mut self) -> Result<(), &'static str> {
        self.server.abort();
        match timeout(Duration::from_secs(1), &mut self.server).await {
            Ok(Err(error)) if error.is_cancelled() => Ok(()),
            Ok(Ok(())) => Ok(()),
            _ => Err(CLEANUP),
        }
    }
}

impl Drop for FixtureRelay {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct VideoEvents {
    ice: mpsc::Sender<RTCIceCandidateInit>,
    tracks: mpsc::Sender<()>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for VideoEvents {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if event.candidate.component == 1 {
            if let Ok(candidate) = event.candidate.to_json() {
                let _ = self.ice.try_send(candidate);
            }
        }
    }

    async fn on_track(&self, _: Arc<dyn TrackRemote>) {
        let _ = self.tracks.try_send(());
    }
}

fn codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: "video/H264".into(),
        clock_rate: 90_000,
        sdp_fmtp_line: FMTP.into(),
        ..Default::default()
    }
}

async fn peer(
    nat: &FixtureNat,
    side: FixtureSide,
) -> Result<
    (
        Arc<dyn PeerConnection>,
        mpsc::Receiver<RTCIceCandidateInit>,
        mpsc::Receiver<()>,
    ),
    &'static str,
> {
    let (ice, rx) = mpsc::channel(1);
    let (tracks, events) = mpsc::channel(1);
    let mut engine = MediaEngine::default();
    engine
        .register_codec(
            RTCRtpCodecParameters {
                rtp_codec: codec(),
                payload_type: PAYLOAD_TYPE,
            },
            RtpCodecKind::Video,
        )
        .map_err(|_| NATIVE)?;
    let pc = PeerConnectionBuilder::new()
        .with_media_engine(engine)
        .with_setting_engine(
            SettingEngineBuilder::new()
                .with_include_loopback_candidate(true)
                .with_multicast_dns_mode(MulticastDnsMode::Disabled)
                .with_network_types(vec![NetworkType::Udp4])
                .build(),
        )
        .with_host_candidates(false)
        .with_configuration(
            RTCConfigurationBuilder::default()
                .with_ice_servers(vec![nat.stun_server()])
                .build(),
        )
        .with_udp_addrs(vec![SocketAddr::from((LOOPBACK, 0))])
        .with_runtime(nat.runtime(side))
        .with_handler(Arc::new(VideoEvents { ice, tracks }))
        .build()
        .await
        .map_err(|_| NATIVE)?;
    Ok((Arc::new(pc), rx, events))
}

async fn negotiate(
    nat: &FixtureNat,
    h: &mut PublicVideoWss<'_>,
    v: &mut PublicVideoWss<'_>,
) -> Result<
    (
        Arc<dyn PeerConnection>,
        Arc<dyn PeerConnection>,
        mpsc::Receiver<()>,
    ),
    &'static str,
> {
    let (host, mut host_ice, _) = peer(nat, FixtureSide::Host).await?;
    let (viewer, mut viewer_ice, tracks) = peer(nat, FixtureSide::Viewer).await?;
    viewer
        .add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                ..Default::default()
            }),
        )
        .await
        .map_err(|_| NATIVE)?;
    let sender = host
        .add_track(Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            "signed-fixture-video".into(),
            "fixture-only".into(),
            "no capture".into(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(SSRC),
                    ..Default::default()
                },
                codec: codec(),
                active: true,
                max_bitrate: 100_000,
                max_framerate: Some(30.0),
                scale_resolution_down_by: None,
            }],
        ))))
        .await
        .map_err(|_| NATIVE)?;
    let offer = viewer.create_offer(None).await.map_err(|_| NATIVE)?;
    viewer
        .set_local_description(offer.clone())
        .await
        .map_err(|_| NATIVE)?;
    let offer = signed_sdp(
        &offer.sdp,
        viewer_ice.recv().await.ok_or(NATIVE)?,
        FixtureSide::Viewer,
    )?;
    v.send_signal(SignalKind::Offer, offer).await?;
    let offer = verified_sdp(h, SignalKind::Offer).await?;
    host.set_remote_description(RTCSessionDescription::offer(offer).map_err(|_| NATIVE)?)
        .await
        .map_err(|_| NATIVE)?;

    let answer = host.create_answer(None).await.map_err(|_| NATIVE)?;
    host.set_local_description(answer.clone())
        .await
        .map_err(|_| NATIVE)?;
    let answer = signed_sdp(
        &answer.sdp,
        host_ice.recv().await.ok_or(NATIVE)?,
        FixtureSide::Host,
    )?;
    h.send_signal(SignalKind::Answer, answer).await?;
    let answer = verified_sdp(v, SignalKind::Answer).await?;
    viewer
        .set_remote_description(RTCSessionDescription::answer(answer).map_err(|_| NATIVE)?)
        .await
        .map_err(|_| NATIVE)?;
    loop {
        if let Some(dtls) = sender.transport().await.map_err(|_| NATIVE)? {
            if dtls.state().await.map_err(|_| NATIVE)? == RTCDtlsTransportState::Connected {
                break;
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    if h.check_live_video_pair(host.as_ref()).await?.path != "direct" {
        return Err(NATIVE);
    }
    Ok((host, viewer, tracks))
}

async fn verified_sdp(
    wss: &mut PublicVideoWss<'_>,
    expected: SignalKind,
) -> Result<String, &'static str> {
    match wss.next_event(WAIT).await? {
        PublicVideoWssEvent::Signal { kind, data } if kind == expected => Ok(data),
        _ => Err(NATIVE),
    }
}

fn signed_sdp(
    sdp: &str,
    candidate: RTCIceCandidateInit,
    side: FixtureSide,
) -> Result<String, &'static str> {
    let fields: Vec<_> = candidate.candidate.split_whitespace().collect();
    if fields.len() < 8 || fields[4] != side.ip().to_string() || fields[7] != "srflx" {
        return Err(NATIVE);
    }
    let lines = sdp
        .lines()
        .filter_map(|line| {
            if line.starts_with("a=candidate:") || line == "a=end-of-candidates" {
                None
            } else if line.starts_with("o=") {
                Some("o=- 1 1 IN IP4 0.0.0.0")
            } else if line.starts_with("c=") {
                Some("c=IN IP4 0.0.0.0")
            } else if line.starts_with("a=rtcp:") {
                Some("a=rtcp:9 IN IP4 0.0.0.0")
            } else {
                Some(line)
            }
        })
        .collect::<Vec<_>>();
    Ok(format!(
        "{}\r\na={} raddr 0.0.0.0 rport 0\r\na=end-of-candidates\r\n",
        lines.join("\r\n"),
        fields[..8].join(" ")
    ))
}

fn fixture_payload() -> Result<Vec<u8>, &'static str> {
    // Locally encoded synthetic raw YUV420, Annex B Baseline L3.1, one IDR,
    // SPS/PPS repeated, encoder SEI omitted; source pattern described above.
    let encoded = include_bytes!("synthetic_32x32.h264");
    let payloads = H264Payloader::default()
        .payload(1188, &encoded.as_slice().into())
        .map_err(|_| FIXTURE)?;
    let mut stap = vec![0x78];
    for payload in payloads {
        let (&header, body) = payload.split_first().ok_or(FIXTURE)?;
        match header & 31 {
            24 => stap.extend_from_slice(body),
            1..=23 => {
                let len = u16::try_from(payload.len()).map_err(|_| FIXTURE)?;
                stap.extend_from_slice(&len.to_be_bytes());
                stap.extend_from_slice(&payload);
            }
            _ => return Err(FIXTURE),
        }
    }
    if !(2..=1188).contains(&stap.len()) {
        return Err(FIXTURE);
    }
    Ok(stap)
}

fn picture(sequence_number: u16, payload: &[u8]) -> Packet {
    Packet {
        header: Header {
            version: 2,
            marker: true,
            payload_type: PAYLOAD_TYPE,
            sequence_number,
            timestamp: 3000 * u32::from(sequence_number),
            ssrc: SSRC,
            ..Default::default()
        },
        payload: payload.to_vec().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bundled_frames_use_signed_loopback_video_and_release_resources() {
        let (_control, stop) = watch::channel(false);
        let frames = verified_synthetic_frames(stop)
            .await
            .expect("signed loopback run");
        assert_eq!(frames.len(), 2);
        for (sequence, frame) in frames.into_iter().enumerate() {
            assert_eq!(frame.sequence, sequence as u64);
            assert_eq!(frame.timestamp_us, sequence as u64 * 33_333);
            assert!(frame.keyframe);
            assert_eq!(frame.annex_b, normalize_fixture());
        }
    }

    #[tokio::test]
    async fn signed_loopback_can_stop_before_or_during_startup() {
        let (_control, stop) = watch::channel(true);
        assert!(matches!(
            verified_synthetic_frames(stop).await,
            Err(STOPPED)
        ));
        let (control, stop) = watch::channel(false);
        let (result, ()) = tokio::join!(verified_synthetic_frames(stop), async {
            sleep(Duration::from_millis(25)).await;
            control.send_replace(true);
        });
        assert!(matches!(result, Err(STOPPED)));
    }

    fn normalize_fixture() -> Vec<u8> {
        // Build expected Annex B from the bundled SPS/PPS/IDR independently
        // of native RTP sequence or the display reader.
        let data = include_bytes!("synthetic_32x32.h264");
        let mut expected = Vec::new();
        let mut cursor = 0;
        while let Some(prefix) = data[cursor..].windows(3).position(|w| w == [0, 0, 1]) {
            let start = cursor + prefix + 3;
            let end = data[start..]
                .windows(3)
                .position(|w| w == [0, 0, 1])
                .map(|pos| start + pos)
                .unwrap_or(data.len());
            let nal_end = if end < data.len() && data[end - 1] == 0 {
                end - 1
            } else {
                end
            };
            expected.extend_from_slice(&[0, 0, 0, 1]);
            expected.extend_from_slice(&data[start..nal_end]);
            cursor = end;
        }
        expected
    }
}

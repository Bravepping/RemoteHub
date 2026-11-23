//! Own the native ICE driver, signed SDP and its exact UDP boundaries.
//! The physical socket and TURN relayer enforce separate address checks.

use crate::{
    direct_policy::{self, PathSummary, VerifiedCandidates},
    direct_runtime::{Boundary, DirectRuntime},
    pinned_runtime::PinnedRuntime,
    public_video::{PublicVideoIcePlan, PublicVideoProof},
    public_video_route::RouteGrant,
    public_video_wss::{read_authorizable_video_pair, Role},
    relay_runtime::RelayRuntime,
};
use hub_identity::session::{SessionChallenge, SessionRole};
use std::{
    fmt,
    future::Future,
    io,
    net::{SocketAddr, SocketAddrV4, TcpListener, UdpSocket},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::{Duration, Instant},
};

use webrtc::runtime::{
    AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, Runtime,
};

use rtc::{
    ice::{mdns::MulticastDnsMode, network_type::NetworkType},
    rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
        RtpCodecKind,
    },
};
use tokio::{sync::watch, time::timeout};
use webrtc::{
    media_stream::{track_local::static_rtp::TrackLocalStaticRTP, MediaStreamTrack},
    peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfiguration, RTCIceGatheringState, RTCSessionDescription, SettingEngineBuilder,
    },
    rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit},
};

const DRIVER: &str = "PUBLIC_VIDEO_ICE_DRIVER";
const WAIT: &str = "PUBLIC_VIDEO_ICE_DRIVER_TIMEOUT";
const DRIVER_WAIT: Duration = Duration::from_secs(8);
const CLOSE_WAIT: Duration = Duration::from_secs(1);

const CLOSED: &str = "PUBLIC_VIDEO_ICE_BOUNDARY_CLOSED";
const SETUP: &str = "PUBLIC_VIDEO_ICE_BOUNDARY_SETUP";
const PEER: &str = "PUBLIC_VIDEO_ICE_BOUNDARY_PEER";

enum RouteRuntime {
    Direct(DirectRuntime),
    Relay(RelayRuntime),
}

impl RouteRuntime {
    fn as_runtime(&self) -> &dyn Runtime {
        match self {
            Self::Direct(runtime) => runtime,
            Self::Relay(runtime) => runtime,
        }
    }
}

/// Shared by the proof owner and (when wired to ICE) the relayer/driver.
/// An exact signed address, not a server IP, port range, or TURN permission.
struct ScopedRuntime {
    physical: RouteRuntime,
    peers: RwLock<Option<VerifiedCandidates>>,
    stopped: Arc<AtomicBool>,
    deadline: Instant,
}

impl fmt::Debug for ScopedRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PublicVideoScopedRuntime([REDACTED])")
    }
}

impl ScopedRuntime {
    fn active(&self) -> bool {
        !self.stopped.load(Ordering::Acquire) && Instant::now() < self.deadline
    }

    fn stop(&self) {
        // Revoke the physical socket before any previously encoded writes
        // could leave the driver. Already sent datagrams cannot be recalled.
        if let RouteRuntime::Direct(runtime) = &self.physical {
            runtime.boundary.stop();
        }
        self.stopped.store(true, Ordering::Release);
    }

    fn admit(&self, candidates: VerifiedCandidates) -> Result<(), &'static str> {
        let mut peers = self.peers.write().map_err(|_| PEER)?;
        if !self.active() || peers.is_some() {
            return Err(PEER);
        }
        if let RouteRuntime::Direct(runtime) = &self.physical {
            runtime
                .boundary
                .install_peer(candidates.clone())
                .map_err(|_| PEER)?;
        }
        *peers = Some(candidates);
        Ok(())
    }
}

impl Runtime for ScopedRuntime {
    fn requires_exact_turn_source(&self) -> bool {
        true
    }

    fn allows_turn_peer(&self, peer: SocketAddr) -> bool {
        let SocketAddr::V4(peer) = peer else {
            return false;
        };
        self.active()
            && self.peers.read().ok().is_some_and(|peers| {
                peers
                    .as_ref()
                    .is_some_and(|verified| verified.candidates().iter().any(|c| c.address == peer))
            })
            && self.active()
    }

    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        self.physical.as_runtime().spawn(future)
    }
    fn spawn_reactor(
        &self,
        pool_size: usize,
        future: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Box<dyn JoinHandle> {
        self.physical.as_runtime().spawn_reactor(pool_size, future)
    }
    fn wrap_udp_socket(&self, socket: UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        self.physical.as_runtime().wrap_udp_socket(socket)
    }
    fn wrap_tcp_listener(&self, listener: TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        self.physical.as_runtime().wrap_tcp_listener(listener)
    }
    fn connect_tcp(
        &self,
        addr: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + '_>> {
        self.physical.as_runtime().connect_tcp(addr)
    }
    fn resolve_host<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        self.physical.as_runtime().resolve_host(host)
    }
    fn now(&self) -> Instant {
        self.physical.as_runtime().now()
    }
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.physical.as_runtime().sleep(duration)
    }
    fn interval(&self, period: Duration) -> Box<dyn AsyncInterval> {
        self.physical.as_runtime().interval(period)
    }
    fn block_on(&self, future: Pin<Box<dyn Future<Output = ()> + '_>>) {
        self.physical.as_runtime().block_on(future)
    }
    fn yield_now(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.physical.as_runtime().yield_now()
    }
}

struct DriverStart {
    config: RTCConfiguration,
    local: SocketAddrV4,
    host_candidates: bool,
}

struct Gather(watch::Sender<bool>);
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Gather {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.0.send_replace(true);
        }
    }
}

// Dropping a canceled or failed construction/negotiation future immediately
// revokes the shared runtime, including while the driver is still running.
struct RevokeOnCancel(Option<Arc<ScopedRuntime>>);
impl Drop for RevokeOnCancel {
    fn drop(&mut self) {
        if let Some(runtime) = &self.0 {
            runtime.stop();
        }
    }
}

/// Owns one scoped video ICE driver. No raw peer connection, track, or
/// trickle API is exported. Call close on normal/terminal signaling exit.
pub struct PublicVideoScopedIce<'a> {
    proof: PublicVideoProof<'a>,
    runtime: Arc<ScopedRuntime>,
    issued_at: u64,
    expires_at: u64,
    start: Option<DriverStart>,
    pc: Option<Arc<dyn PeerConnection>>,
    gathered: Option<watch::Receiver<bool>>,
    local_candidates: Option<VerifiedCandidates>,
    remote_applied: bool,
    path_verified: bool,
}

impl<'a> PublicVideoScopedIce<'a> {
    pub(crate) fn new(
        proof: PublicVideoProof<'a>,
        plan: PublicVideoIcePlan,
        now: u64,
    ) -> Result<Self, &'static str> {
        // Check the credentials again at handoff. There is no independent
        // endpoint or optional unsecured runtime to substitute here.
        let expires_at = plan.expires_at();
        let deadline = Instant::now() + Duration::from_secs(expires_at.saturating_sub(now));
        let start = DriverStart {
            config: plan.rtc_configuration(now)?,
            local: SocketAddrV4::new(plan.local_ip(), 0),
            host_candidates: plan.host_candidates_allowed(),
        };
        let inner = webrtc::runtime::default_runtime().ok_or(SETUP)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let physical = match &plan.grant {
            RouteGrant::Direct(grant) => RouteRuntime::Direct(DirectRuntime {
                inner,
                boundary: Boundary::new(grant, now).map_err(|_| SETUP)?,
            }),
            RouteGrant::Relay(grant) => RouteRuntime::Relay(RelayRuntime {
                base: PinnedRuntime {
                    inner,
                    local: grant.target().local.into(),
                    peer: (*grant.target().turn.ip()).into(),
                    counters: Arc::default(),
                },
                endpoint: grant.target().turn.into(),
                deadline,
                stopped: stopped.clone(),
            }),
        };
        // The direct UDP guard and logical TURN gate share the same expiry.
        let deadline = match &physical {
            RouteRuntime::Direct(runtime) => runtime.boundary.deadline(),
            RouteRuntime::Relay(_) => deadline,
        };
        Ok(Self {
            proof,
            runtime: Arc::new(ScopedRuntime {
                physical,
                peers: RwLock::new(None),
                stopped,
                deadline,
            }),
            issued_at: now,
            expires_at,
            start: Some(start),
            pc: None,
            gathered: None,
            local_candidates: None,
            remote_applied: false,
            path_verified: false,
        })
    }

    #[cfg(test)]
    fn new_loopback_fixture(
        proof: PublicVideoProof<'a>,
        room: uuid::Uuid,
        local: SocketAddrV4,
        server: SocketAddrV4,
        config: RTCConfiguration,
        inner: Arc<dyn Runtime>,
        now: u64,
    ) -> Result<Self, &'static str> {
        assert_eq!(*local.ip(), std::net::Ipv4Addr::LOCALHOST);
        let boundary = Boundary::loopback_fixture(server.into(), room);
        let deadline = boundary.deadline();
        Ok(Self {
            proof,
            runtime: Arc::new(ScopedRuntime {
                physical: RouteRuntime::Direct(DirectRuntime { inner, boundary }),
                peers: RwLock::new(None),
                stopped: Arc::default(),
                deadline,
            }),
            issued_at: now,
            expires_at: now + 120,
            start: Some(DriverStart {
                config,
                local,
                host_candidates: false,
            }),
            pc: None,
            gathered: None,
            local_candidates: None,
            remote_applied: false,
            path_verified: false,
        })
    }

    /// Bind one UDP4 interface only; caller must obtain native consent and
    /// ensure the approved signaling lifetime owns the returned driver.
    pub async fn start(&mut self, now: u64) -> Result<(), &'static str> {
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        self.check_current(now)?;
        let start = self.start.take().ok_or(DRIVER)?;
        let (gather_tx, gather_rx) = watch::channel(false);
        let runtime = self.runtime.clone();
        let pc = timeout(self.wait(DRIVER_WAIT), async {
            let mut media = MediaEngine::default();
            let codec = RTCRtpCodec {
                mime_type: "video/H264".into(),
                clock_rate: 90_000,
                sdp_fmtp_line: crate::video::FMTP.into(),
                ..Default::default()
            };
            media
                .register_codec(
                    RTCRtpCodecParameters {
                        rtp_codec: codec.clone(),
                        payload_type: 102,
                    },
                    RtpCodecKind::Video,
                )
                .map_err(|_| DRIVER)?;
            let pc = PeerConnectionBuilder::new()
                .with_configuration(start.config)
                .with_host_candidates(start.host_candidates)
                .with_udp_addrs(vec![start.local])
                .with_setting_engine(
                    SettingEngineBuilder::new()
                        .with_multicast_dns_mode(MulticastDnsMode::Disabled)
                        .with_network_types(vec![NetworkType::Udp4])
                        .with_include_loopback_candidate(false)
                        .build(),
                )
                .with_media_engine(media)
                // The dedicated driver also terminates on PeerConnection drop
                // if the caller cannot await the normal close path.
                .with_dedicated_reactor_pool_size(1)
                .with_runtime(runtime)
                .with_handler(Arc::new(Gather(gather_tx)))
                .build()
                .await
                .map_err(|_| DRIVER)?;
            match self.proof.role() {
                SessionRole::Host => {
                    pc.add_track(Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
                        "remotehub-video".into(),
                        "remotehub-session".into(),
                        "video".into(),
                        RtpCodecKind::Video,
                        vec![RTCRtpEncodingParameters {
                            active: true,
                            rtp_coding_parameters: RTCRtpCodingParameters {
                                ssrc: Some(crate::video::SSRC),
                                ..Default::default()
                            },
                            codec,
                            ..Default::default()
                        }],
                    ))))
                    .await
                    .map_err(|_| DRIVER)?;
                }
                SessionRole::Viewer => {
                    pc.add_transceiver_from_kind(
                        RtpCodecKind::Video,
                        Some(RTCRtpTransceiverInit {
                            direction: RTCRtpTransceiverDirection::Recvonly,
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(|_| DRIVER)?;
                }
            }
            Ok::<_, &'static str>(Arc::new(pc) as Arc<dyn PeerConnection>)
        })
        .await
        .map_err(|_| WAIT)??;
        self.check_current(now)?;
        self.pc = Some(pc);
        self.gathered = Some(gather_rx);
        guard.0 = None;
        Ok(())
    }

    /// Return the signed, complete local offer (viewer) or answer (host).
    /// Never publish private related addresses from the driver's SDP.
    pub async fn local_signal(
        &mut self,
        challenge: SessionChallenge,
        now: u64,
    ) -> Result<(String, [u8; 64]), &'static str> {
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        self.check_current(now)?;
        let host = self.proof.role() == SessionRole::Host;
        if self.local_candidates.is_some() || self.remote_applied != host {
            return Err(DRIVER);
        }
        let pc = self.pc.as_ref().ok_or(DRIVER)?;
        let gathered = self.gathered.as_mut().ok_or(DRIVER)?;
        let wait = self
            .runtime
            .deadline
            .saturating_duration_since(Instant::now())
            .min(DRIVER_WAIT);
        let sdp = timeout(wait, async {
            let desc = if host {
                pc.create_answer(None).await
            } else {
                pc.create_offer(None).await
            }
            .map_err(|_| DRIVER)?;
            pc.set_local_description(desc).await.map_err(|_| DRIVER)?;
            gathered.wait_for(|done| *done).await.map_err(|_| DRIVER)?;
            let desc = pc.local_description().await.ok_or(DRIVER)?;
            redact_gathered_sdp(&desc.sdp)
        })
        .await
        .map_err(|_| WAIT)??;
        self.check_current(now)?;
        let (signature, candidates) = self.proof.seal_video_sdp(challenge, &sdp, now)?;
        self.local_candidates = Some(candidates);
        guard.0 = None;
        Ok((sdp, signature))
    }

    /// Install the exact device-signed UDP addresses before the RTC driver
    /// can start checks from the corresponding remote offer/answer.
    pub async fn apply_peer_signal(
        &mut self,
        sdp: &str,
        signature: &[u8; 64],
        now: u64,
    ) -> Result<(), &'static str> {
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        self.check_current(now)?;
        let host = self.proof.role() == SessionRole::Host;
        if self.remote_applied || self.local_candidates.is_some() == host {
            return Err(DRIVER);
        }
        let pc = self.pc.as_ref().ok_or(DRIVER)?;
        let verified = self.proof.verify_video_sdp(sdp, signature, now)?;
        let remote = if host {
            RTCSessionDescription::offer(sdp.to_owned())
        } else {
            RTCSessionDescription::answer(sdp.to_owned())
        }
        .map_err(|_| DRIVER)?;
        self.runtime.admit(verified)?;
        timeout(self.wait(DRIVER_WAIT), pc.set_remote_description(remote))
            .await
            .map_err(|_| WAIT)?
            .map_err(|_| DRIVER)?;
        self.check_current(now)?;
        self.remote_applied = true;
        guard.0 = None;
        Ok(())
    }

    /// Observe the actual video transport; no SDP-only success. The viewer
    /// does not expose a receiver transport until the first RTP track.
    pub async fn selected_path(&mut self, now: u64) -> Result<PathSummary, &'static str> {
        self.check_current(now)?;
        let pc = self.pc.as_ref().ok_or(DRIVER)?;
        let role = if self.proof.role() == SessionRole::Host {
            Role::Host
        } else {
            Role::Viewer
        };
        let pair = timeout(
            self.wait(Duration::from_millis(500)),
            read_authorizable_video_pair(pc.as_ref(), role),
        )
        .await
        .map_err(|_| WAIT)
        .and_then(|r| r);
        let (local, remote) = match pair {
            Ok(pair) => pair,
            Err(error) => {
                if self.path_verified || error == "PUBLIC_VIDEO_ICE_PAIR_REJECTED" {
                    self.stop();
                }
                return Err(error);
            }
        };
        let result = (|| {
            self.check_current(now)?;
            let approved_local = self.local_candidates.as_ref().ok_or(PEER)?;
            let approved_remote = self.runtime.peers.read().map_err(|_| PEER)?;
            direct_policy::selected(
                local,
                remote,
                approved_local,
                approved_remote.as_ref().ok_or(PEER)?,
            )
            .map_err(|_| PEER)
        })();
        if result.is_ok() {
            self.path_verified = true;
        } else {
            self.stop();
        }
        result
    }

    /// Revoke first and wait for the local driver to shut down. This does
    /// not claim that a remote TURN allocation was released immediately.
    pub async fn close(&mut self) -> Result<(), &'static str> {
        self.stop();
        if let Some(pc) = self.pc.take() {
            timeout(CLOSE_WAIT, pc.close())
                .await
                .map_err(|_| WAIT)?
                .map_err(|_| DRIVER)?;
        }
        Ok(())
    }

    fn wait(&self, limit: Duration) -> Duration {
        self.runtime
            .deadline
            .saturating_duration_since(Instant::now())
            .min(limit)
    }

    pub fn challenge(&self) -> Option<SessionChallenge> {
        if !self.runtime.active() {
            return None;
        }
        self.proof.challenge()
    }

    #[cfg(test)]
    fn seal_local_sdp(
        &mut self,
        challenge: SessionChallenge,
        sdp: &str,
        now: u64,
    ) -> Result<[u8; 64], &'static str> {
        let result = (|| {
            self.check_current(now)?;
            self.proof
                .seal_video_sdp(challenge, sdp, now)
                .map(|(sig, _)| sig)
        })();
        self.stop_on_error(result)
    }

    /// Admit only the exact signed SDP to the physical direct and logical
    /// TURN guards before the caller could apply this SDP to a peer connection.
    #[cfg(test)]
    fn admit_peer_sdp<'s>(
        &mut self,
        sdp: &'s str,
        signature: &[u8; 64],
        now: u64,
    ) -> Result<&'s str, &'static str> {
        let result = (|| {
            self.check_current(now)?;
            let verified = self.proof.verify_video_sdp(sdp, signature, now)?;
            self.runtime.admit(verified)?;
            Ok(sdp)
        })();
        self.stop_on_error(result)
    }

    pub fn stop(&mut self) {
        self.runtime.stop();
        self.start = None;
    }

    fn check_current(&self, now: u64) -> Result<(), &'static str> {
        if !self.runtime.active() || !(self.issued_at..self.expires_at).contains(&now) {
            return Err(CLOSED);
        }
        Ok(())
    }

    #[cfg(test)]
    fn stop_on_error<T>(&mut self, result: Result<T, &'static str>) -> Result<T, &'static str> {
        if result.is_err() {
            self.stop();
        }
        result
    }
}

impl Drop for PublicVideoScopedIce<'_> {
    fn drop(&mut self) {
        self.stop();
    }
}

// Strip only connection defaults and related-address metadata. The signed
// SDP policy then rejects unknown fields and validates every exact candidate.
fn redact_gathered_sdp(sdp: &str) -> Result<String, &'static str> {
    if !sdp.is_ascii() || sdp.len() > hub_identity::session::MAX_SIGNAL_BYTES {
        return Err(DRIVER);
    }
    let mut clean = String::with_capacity(sdp.len());
    for line in sdp.lines() {
        if let Some(value) = line.strip_prefix("o=") {
            let fields: Vec<_> = value.split(' ').collect();
            if fields.len() != 6 {
                return Err(DRIVER);
            }
            clean.push_str(&format!("o=- {} {} IN IP4 0.0.0.0", fields[1], fields[2]));
        } else if line.starts_with("c=") {
            clean.push_str("c=IN IP4 0.0.0.0");
        } else if line.starts_with("a=rtcp:") {
            clean.push_str("a=rtcp:9 IN IP4 0.0.0.0");
        } else if let Some(candidate) = line.strip_prefix("a=candidate:") {
            let fields: Vec<_> = candidate.split_ascii_whitespace().collect();
            if fields.len() < 8 {
                return Err(DRIVER);
            }
            // The native SDP emits a component-2 copy even with rtcp-mux.
            // Public video advertises only the multiplexed RTP component;
            // verification of incoming SDP remains strict.
            match fields[1] {
                "1" => {}
                "2" => continue,
                _ => return Err(DRIVER),
            }
            clean.push_str("a=candidate:");
            clean.push_str(&fields[..8].join(" "));
            if matches!(fields[7], "srflx" | "relay") {
                clean.push_str(" raddr 0.0.0.0 rport 0");
            } else if fields[7] != "host" {
                return Err(DRIVER);
            }
        } else {
            clean.push_str(line);
        }
        clean.push_str("\r\n");
    }
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ice_policy::{IceConfig, IceMode, IceServer, RelayGrant, RelayTarget};
    use crate::public_video::PublicVideoScope;
    use hub_identity::{pairing::PairingCandidate, session::SessionRole, Identity};
    use std::net::SocketAddr;
    use uuid::Uuid;
    use webrtc::runtime::Runtime;

    const NOW: u64 = 50_000;
    const TURN: &str = "93.184.216.34:3478";
    const DIRECT: &str = "1.1.1.1:50002";
    const RELAY: &str = "93.184.216.34:49152";

    fn scope(mode: IceMode) -> PublicVideoScope {
        PublicVideoScope {
            wss: "93.184.216.34:8443".parse().unwrap(),
            ca_sha256: [3; 32],
            turn: TURN.parse().unwrap(),
            relay_ports: (49152, 49200),
            mode,
            expires_at: NOW + 120,
        }
    }

    fn stage<'a>(
        local: &'a Identity,
        peer: &Identity,
        role: SessionRole,
        room: Uuid,
        scope: PublicVideoScope,
    ) -> PublicVideoScopedIce<'a> {
        let pin = local
            .verify_peer(
                &PairingCandidate::parse(&peer.pairing_code()).unwrap(),
                &peer.public().fingerprint,
            )
            .unwrap();
        let proof =
            PublicVideoProof::prepare(local, &pin, role, room, [1; 32], scope, NOW).unwrap();
        let target = RelayTarget {
            local: "192.168.1.2".parse().unwrap(),
            turn: scope.turn,
            relay: *scope.turn.ip(),
            port_min: scope.relay_ports.0,
            port_max: scope.relay_ports.1,
        };
        let expires_at = NOW + 90;
        let config = IceConfig {
            mode: scope.mode,
            expires_at,
            session_id: room.to_string(),
            ice_servers: vec![IceServer {
                urls: vec![format!("turn:{TURN}?transport=udp")],
                username: format!("{expires_at}:{room}"),
                credential: "AAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
            }],
        };
        let grant = RelayGrant::new(target, &room.to_string(), scope.expires_at, true).unwrap();
        proof.stage_ice(grant, config, NOW).unwrap()
    }

    fn offer(candidate: &str) -> String {
        format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\nc=IN IP4 0.0.0.0\r\na=rtcp-mux\r\na=recvonly\r\na=fingerprint:sha-256 {}\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\na=candidate:{candidate}\r\n",
            ["AB"; 32].join(":")
        )
    }

    #[tokio::test]
    async fn direct_admits_only_verified_exact_peer_and_revokes_on_replay_or_stop() {
        let h = Identity::generate_ephemeral().unwrap();
        let v = Identity::generate_ephemeral().unwrap();
        let scope = scope(IceMode::DirectPreferred);
        for outcome in ["replay", "stop", "drop"] {
            let room = Uuid::new_v4();
            let mut host = stage(&h, &v, SessionRole::Host, room, scope);
            let mut viewer = stage(&v, &h, SessionRole::Viewer, room, scope);
            let runtime = host.runtime.clone();
            assert!(runtime.requires_exact_turn_source());
            assert!(matches!(&runtime.physical, RouteRuntime::Direct(_)));
            let address: SocketAddr = DIRECT.parse().unwrap();
            let other_port: SocketAddr = "1.1.1.1:50003".parse().unwrap();
            assert!(!runtime.allows_turn_peer(address));
            assert!(!runtime.allows_turn_peer(scope.turn.into()));
            // This resolver only checks numeric addresses; it does not perform I/O.
            assert_eq!(
                runtime.resolve_host(TURN).await.unwrap(),
                [SocketAddr::V4(scope.turn)]
            );
            assert!(runtime.resolve_host(DIRECT).await.is_err());
            assert!(runtime.resolve_host("93.184.216.34:3479").await.is_err());
            assert!(runtime.resolve_host("localhost:3478").await.is_err());
            let sdp = offer("s 1 udp 1 1.1.1.1 50002 typ srflx raddr 0.0.0.0 rport 0");
            let signature = viewer
                .seal_local_sdp(host.challenge().unwrap(), &sdp, NOW)
                .unwrap();
            assert_eq!(host.admit_peer_sdp(&sdp, &signature, NOW), Ok(sdp.as_str()));
            assert!(runtime.allows_turn_peer(address));
            assert!(!runtime.allows_turn_peer(other_port));
            assert!(!runtime.allows_turn_peer(scope.turn.into()));
            assert_eq!(runtime.resolve_host(DIRECT).await.unwrap(), [address]);
            assert!(runtime.resolve_host("1.1.1.1:50003").await.is_err());
            assert!(runtime.connect_tcp(address).await.is_err());
            match outcome {
                "replay" => assert!(host.admit_peer_sdp(&sdp, &signature, NOW).is_err()),
                "stop" => host.stop(),
                _ => drop(host),
            }
            assert!(runtime.resolve_host(TURN).await.is_err());
            assert!(runtime.resolve_host(DIRECT).await.is_err());
            assert!(!runtime.allows_turn_peer(address));
        }
    }

    #[tokio::test]
    async fn direct_rejects_tamper_even_with_eligible_sdp_and_revokes_bootstrap() {
        let h = Identity::generate_ephemeral().unwrap();
        let v = Identity::generate_ephemeral().unwrap();
        let scope = scope(IceMode::DirectPreferred);
        let room = Uuid::new_v4();
        let mut host = stage(&h, &v, SessionRole::Host, room, scope);
        let mut viewer = stage(&v, &h, SessionRole::Viewer, room, scope);
        let sdp = offer("s 1 udp 1 1.1.1.1 50002 typ srflx raddr 0.0.0.0 rport 0");
        let signature = viewer
            .seal_local_sdp(host.challenge().unwrap(), &sdp, NOW)
            .unwrap();
        assert!(host
            .admit_peer_sdp(&sdp.replace("50002", "50003"), &signature, NOW)
            .is_err());
        let runtime = host.runtime.clone();
        assert!(runtime.resolve_host(TURN).await.is_err());
        assert!(runtime.resolve_host(DIRECT).await.is_err());
        assert!(runtime.resolve_host("1.1.1.1:50003").await.is_err());
        assert!(!runtime.allows_turn_peer(DIRECT.parse().unwrap()));
        assert!(host.challenge().is_none());
        assert_eq!(host.admit_peer_sdp(&sdp, &signature, NOW), Err(CLOSED));
    }

    #[tokio::test]
    async fn turn_peer_permission_expires_without_another_proof_operation() {
        let h = Identity::generate_ephemeral().unwrap();
        let v = Identity::generate_ephemeral().unwrap();
        let scope = scope(IceMode::RelayOnly);
        let room = Uuid::new_v4();
        let mut host = stage(&h, &v, SessionRole::Host, room, scope);
        let mut viewer = stage(&v, &h, SessionRole::Viewer, room, scope);
        let offer = offer("r 1 udp 1 93.184.216.34 49152 typ relay raddr 0.0.0.0 rport 0");
        let sig = viewer
            .seal_local_sdp(host.challenge().unwrap(), &offer, NOW)
            .unwrap();
        host.admit_peer_sdp(&offer, &sig, NOW).unwrap();
        let relay = RELAY.parse().unwrap();
        assert!(host.runtime.allows_turn_peer(relay));

        Arc::get_mut(&mut host.runtime).unwrap().deadline = Instant::now();
        assert!(!host.runtime.allows_turn_peer(relay));
        assert!(!host.runtime.stopped.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn relay_body_can_only_name_relay_and_outer_guard_never_opens_peer_port() {
        let h = Identity::generate_ephemeral().unwrap();
        let v = Identity::generate_ephemeral().unwrap();
        let scope = scope(IceMode::RelayOnly);
        let room = Uuid::new_v4();
        let mut host = stage(&h, &v, SessionRole::Host, room, scope);
        let mut viewer = stage(&v, &h, SessionRole::Viewer, room, scope);
        let sdp = offer("r 1 udp 1 93.184.216.34 49152 typ relay raddr 0.0.0.0 rport 0");
        let sig = viewer
            .seal_local_sdp(host.challenge().unwrap(), &sdp, NOW)
            .unwrap();
        let runtime = host.runtime.clone();
        assert!(matches!(&runtime.physical, RouteRuntime::Relay(_)));
        let relay = RELAY.parse().unwrap();
        assert!(!runtime.allows_turn_peer(relay));
        assert_eq!(host.admit_peer_sdp(&sdp, &sig, NOW), Ok(sdp.as_str()));
        assert!(runtime.allows_turn_peer(relay));
        assert!(!runtime.allows_turn_peer("93.184.216.34:49153".parse().unwrap()));
        assert!(!runtime.allows_turn_peer(scope.turn.into()));
        assert!(!runtime.allows_turn_peer(DIRECT.parse().unwrap()));
        assert!(!runtime.allows_turn_peer("[::ffff:93.184.216.34]:49152".parse().unwrap()));
        assert!(runtime.resolve_host(TURN).await.is_ok());
        assert!(runtime.resolve_host(RELAY).await.is_err());
        assert!(runtime.resolve_host(DIRECT).await.is_err());
        // Credential expiry is earlier than the signed room's expiry.
        assert_eq!(host.check_current(NOW + 89), Ok(()));
        assert_eq!(host.admit_peer_sdp(&sdp, &sig, NOW + 90), Err(CLOSED));
        assert!(runtime.stopped.load(Ordering::Acquire));
        assert!(!runtime.allows_turn_peer(relay));

        let fresh_room = Uuid::new_v4();
        let mut fresh_host = stage(&h, &v, SessionRole::Host, fresh_room, scope);
        let mut fresh = stage(&v, &h, SessionRole::Viewer, fresh_room, scope);
        let bad = offer("s 1 udp 1 1.1.1.1 50002 typ srflx raddr 0.0.0.0 rport 0");
        let challenge = fresh_host.challenge().unwrap();
        // Even a validly signed bypass of the sender-side SDP policy is
        // never admitted at the receiving boundary.
        let signature = fresh.proof.seal(challenge, &bad, NOW).unwrap();
        assert!(fresh_host.admit_peer_sdp(&bad, &signature, NOW).is_err());
        assert!(fresh_host.challenge().is_none());
    }
}

#[cfg(test)]
#[path = "public_video_native_ice_tests.rs"]
mod native_tests;

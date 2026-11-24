//! Own the native ICE driver, signed SDP and its exact UDP boundaries.
//! The physical socket and TURN relayer enforce separate address checks.

use crate::{
    direct_policy::{self, Candidate, PathSummary, VerifiedCandidates},
    direct_runtime::{Boundary, DirectRuntime},
    pinned_runtime::PinnedRuntime,
    public_video::{PublicVideoIcePlan, PublicVideoProof},
    public_video_route::RouteGrant,
    public_video_wss::{read_video_pair, to_candidate, Role, VideoTrack},
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
    rtp::packet::Packet,
    rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
        RtpCodecKind,
    },
};
use tokio::{sync::watch, time::timeout};
use webrtc::{
    media_stream::{
        track_local::static_rtp::TrackLocalStaticRTP,
        track_remote::{TrackRemote, TrackRemoteEvent},
        MediaStreamTrack,
    },
    peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfiguration, RTCIceGatheringState, RTCSessionDescription, SettingEngineBuilder,
    },
    rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit},
};

#[cfg(feature = "v2-public-control")]
use crate::{control::MAX_BYTES as MAX_CONTROL_BYTES, input_approval::InputScope};
#[cfg(feature = "v2-public-control")]
use rtc::peer_connection::{
    configuration::setting_engine::SctpMaxMessageSize,
    transport::{RTCDtlsTransportState, RTCIceTransportState, RTCSctpTransportState},
};
#[cfg(feature = "v2-public-control")]
use tokio::sync::mpsc;
#[cfg(feature = "v2-public-control")]
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit, RTCDataChannelState},
    peer_connection::transport::SctpTransport,
};

const DRIVER: &str = "PUBLIC_VIDEO_ICE_DRIVER";
const WAIT: &str = "PUBLIC_VIDEO_ICE_DRIVER_TIMEOUT";
const DRIVER_WAIT: Duration = Duration::from_secs(8);
const CLOSE_WAIT: Duration = Duration::from_secs(1);
const PAIR_WAIT: Duration = Duration::from_millis(500);
const MEDIA_POLL: Duration = Duration::from_millis(20);
const VIDEO_PT: u8 = 102;
const MAX_RTP_PAYLOAD: usize = crate::public_video_frames::MAX_VIDEO_RTP_PAYLOAD_BYTES;
const PAIR_UNAVAILABLE: &str = "PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE";
const PAIR_REJECTED: &str = "PUBLIC_VIDEO_ICE_PAIR_REJECTED";
const RTP_INVALID: &str = "PUBLIC_VIDEO_RTP_INVALID";
const RTP_IO: &str = "PUBLIC_VIDEO_RTP_UNAVAILABLE";

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
    fn effective_deadline(&self) -> Instant {
        match &self.physical {
            RouteRuntime::Relay(relay) => relay
                .round_deadline
                .as_ref()
                .and_then(|d| d.get())
                .copied()
                .map_or(self.deadline, |d| self.deadline.min(d)),
            RouteRuntime::Direct(runtime) => self.deadline.min(runtime.boundary.deadline()),
        }
    }

    fn active(&self) -> bool {
        !self.stopped.load(Ordering::Acquire) && Instant::now() < self.effective_deadline()
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

#[cfg(feature = "v2-public-control")]
const CONTROL_LABEL: &str = "remotehub-public-control.v1";
#[cfg(feature = "v2-public-control")]
const CONTROL_PROTOCOL: &str = "remotehub.device-consented.v1";
#[cfg(feature = "v2-public-control")]
const CONTROL_INVALID: &str = "PUBLIC_CONTROL_CHANNEL_INVALID";
#[cfg(feature = "v2-public-control")]
const CONTROL_IO: &str = "PUBLIC_CONTROL_CHANNEL_UNAVAILABLE";

#[cfg(feature = "v2-public-control")]
struct ControlEvents {
    host: bool,
    claimed: AtomicBool,
    ready: mpsc::Sender<Arc<dyn DataChannel>>,
    runtime: Arc<ScopedRuntime>,
}

#[cfg(feature = "v2-public-control")]
#[derive(Default)]
struct ControlLane {
    incoming: Option<mpsc::Receiver<Arc<dyn DataChannel>>>,
    channel: Option<Arc<dyn DataChannel>>,
    opened: bool,
}

struct DriverEvents {
    gathered: watch::Sender<bool>,
    seen_track: Arc<AtomicBool>,
    #[cfg(feature = "v2-public-control")]
    control: Option<ControlEvents>,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for DriverEvents {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.send_replace(true);
        }
    }

    #[cfg(feature = "v2-public-control")]
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        let Some(control) = &self.control else {
            return;
        };
        if !control.host
            || control.claimed.swap(true, Ordering::AcqRel)
            || control.ready.try_send(channel).is_err()
        {
            // A remote-created second or reflected channel must not continue.
            control.runtime.stop();
        }
    }

    async fn on_track(&self, _: Arc<dyn TrackRemote>) {
        self.seen_track.store(true, Ordering::Release);
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

struct CheckedTrack {
    path: PathSummary,
    pair: (Candidate, Candidate),
    track: VideoTrack,
}

fn validate_rtp(packet: &Packet) -> Result<(), &'static str> {
    if packet.header.version != 2
        || packet.header.payload_type != VIDEO_PT
        || packet.header.ssrc != crate::video::SSRC
        || packet.payload.is_empty()
        || packet.payload.len() > MAX_RTP_PAYLOAD
    {
        return Err(RTP_INVALID);
    }
    Ok(())
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
    seen_track: Arc<AtomicBool>,
    local_candidates: Option<VerifiedCandidates>,
    remote_applied: bool,
    path_verified: bool,
    #[cfg(feature = "v2-public-control")]
    control: ControlLane,
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
        #[cfg(feature = "v2-public-control")]
        let round_deadline = proof.has_round_deadline().then(Arc::default);
        #[cfg(not(feature = "v2-public-control"))]
        let round_deadline = None;
        let physical = match &plan.grant {
            RouteGrant::Direct(grant) => {
                let mut boundary = Boundary::new(grant, now).map_err(|_| SETUP)?;
                Arc::get_mut(&mut boundary).ok_or(SETUP)?.round_deadline = round_deadline;
                RouteRuntime::Direct(DirectRuntime { inner, boundary })
            }
            RouteGrant::Relay(grant) => RouteRuntime::Relay(RelayRuntime {
                base: PinnedRuntime {
                    inner,
                    local: grant.target().local.into(),
                    peer: (*grant.target().turn.ip()).into(),
                    counters: Arc::default(),
                },
                endpoint: grant.target().turn.into(),
                deadline,
                round_deadline,
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
            seen_track: Arc::default(),
            local_candidates: None,
            remote_applied: false,
            path_verified: false,
            #[cfg(feature = "v2-public-control")]
            control: ControlLane::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_loopback_fixture(
        proof: PublicVideoProof<'a>,
        room: uuid::Uuid,
        local: SocketAddrV4,
        server: SocketAddrV4,
        config: RTCConfiguration,
        inner: Arc<dyn Runtime>,
        now: u64,
    ) -> Result<Self, &'static str> {
        assert_eq!(*local.ip(), std::net::Ipv4Addr::LOCALHOST);
        #[allow(unused_mut)]
        let mut boundary = Boundary::loopback_fixture(server.into(), room);
        #[cfg(feature = "v2-public-control")]
        {
            Arc::get_mut(&mut boundary).unwrap().round_deadline =
                proof.has_round_deadline().then(Arc::default);
        }
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
            seen_track: Arc::default(),
            local_candidates: None,
            remote_applied: false,
            path_verified: false,
            #[cfg(feature = "v2-public-control")]
            control: ControlLane::default(),
        })
    }

    // Reuse the production relay socket guard, logical TURN gate and ICE
    // driver. The test seam fixes the physical listener to loopback.
    #[cfg(test)]
    pub(crate) fn new_turn_loopback_fixture(
        proof: PublicVideoProof<'a>,
        endpoint: SocketAddrV4,
        server: webrtc::peer_connection::RTCIceServer,
        now: u64,
    ) -> Result<Self, &'static str> {
        use std::net::Ipv4Addr;
        use webrtc::peer_connection::{RTCConfigurationBuilder, RTCIceTransportPolicy};

        if *endpoint.ip() != Ipv4Addr::LOCALHOST
            || endpoint.port() == 0
            || server.urls != [format!("turn:{endpoint}?transport=udp")]
        {
            return Err(SETUP);
        }
        let seconds = {
            #[cfg(feature = "v2-public-control-sustained")]
            if proof.is_sustained_control() {
                crate::public_video::SUSTAINED_CONTROL_SCOPE_TTL_SECONDS
            } else {
                120
            }
            #[cfg(not(feature = "v2-public-control-sustained"))]
            120
        };
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let stopped = Arc::new(AtomicBool::new(false));
        #[cfg(feature = "v2-public-control")]
        let round_deadline = proof.has_round_deadline().then(Arc::default);
        #[cfg(not(feature = "v2-public-control"))]
        let round_deadline = None;
        Ok(Self {
            proof,
            runtime: Arc::new(ScopedRuntime {
                physical: RouteRuntime::Relay(RelayRuntime {
                    base: PinnedRuntime {
                        inner: webrtc::runtime::default_runtime().ok_or(SETUP)?,
                        local: Ipv4Addr::LOCALHOST.into(),
                        peer: Ipv4Addr::LOCALHOST.into(),
                        counters: Arc::default(),
                    },
                    endpoint: endpoint.into(),
                    deadline,
                    round_deadline,
                    stopped: stopped.clone(),
                }),
                peers: RwLock::new(None),
                stopped,
                deadline,
            }),
            issued_at: now,
            expires_at: now + seconds,
            start: Some(DriverStart {
                config: RTCConfigurationBuilder::default()
                    .with_ice_servers(vec![server])
                    .with_ice_transport_policy(RTCIceTransportPolicy::Relay)
                    .build(),
                local: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
                host_candidates: false,
            }),
            pc: None,
            gathered: None,
            seen_track: Arc::default(),
            local_candidates: None,
            remote_applied: false,
            path_verified: false,
            #[cfg(feature = "v2-public-control")]
            control: ControlLane::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture_runtime(&self) -> Arc<dyn Runtime> {
        self.runtime.clone()
    }

    /// Bind one UDP4 interface only; caller must obtain native consent and
    /// ensure the approved signaling lifetime owns the returned driver.
    pub async fn start(&mut self, now: u64) -> Result<(), &'static str> {
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        self.check_current(now)?;
        let start = self.start.take().ok_or(DRIVER)?;
        let (gather_tx, gather_rx) = watch::channel(false);
        let runtime = self.runtime.clone();
        #[cfg(feature = "v2-public-control")]
        let control_events = self.proof.control_scope().map(|_| {
            let (ready, incoming) = mpsc::channel(1);
            self.control.incoming = Some(incoming);
            ControlEvents {
                host: self.proof.role() == SessionRole::Host,
                claimed: AtomicBool::new(false),
                ready,
                runtime: runtime.clone(),
            }
        });
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
                        payload_type: VIDEO_PT,
                    },
                    RtpCodecKind::Video,
                )
                .map_err(|_| DRIVER)?;
            let settings = SettingEngineBuilder::new()
                .with_multicast_dns_mode(MulticastDnsMode::Disabled)
                .with_network_types(vec![NetworkType::Udp4])
                .with_include_loopback_candidate(false);
            #[cfg(feature = "v2-public-control")]
            let settings = if control_events.is_some() {
                settings.with_sctp_max_message_size(SctpMaxMessageSize::Bounded(
                    MAX_CONTROL_BYTES as u32,
                ))
            } else {
                settings
            };
            let pc = PeerConnectionBuilder::new()
                .with_configuration(start.config)
                .with_host_candidates(start.host_candidates)
                .with_udp_addrs(vec![start.local])
                .with_setting_engine(settings.build())
                .with_media_engine(media)
                // The dedicated driver also terminates on PeerConnection drop
                // if the caller cannot await the normal close path.
                .with_dedicated_reactor_pool_size(1)
                .with_runtime(runtime)
                .with_handler(Arc::new(DriverEvents {
                    gathered: gather_tx,
                    seen_track: self.seen_track.clone(),
                    #[cfg(feature = "v2-public-control")]
                    control: control_events,
                }))
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
            #[cfg(feature = "v2-public-control")]
            if self.proof.control_scope().is_some() && self.proof.role() == SessionRole::Viewer {
                self.control.channel = Some(
                    pc.create_data_channel(
                        CONTROL_LABEL,
                        Some(RTCDataChannelInit {
                            protocol: CONTROL_PROTOCOL.into(),
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(|_| CONTROL_IO)?,
                );
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
            .effective_deadline()
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
            let clean = redact_gathered_sdp(&desc.sdp)?;
            #[cfg(feature = "v2-public-control")]
            if self.proof.control_scope().is_some() {
                return Ok(clean.replace("a=ice-options:trickle\r\n", ""));
            }
            Ok::<_, &'static str>(clean)
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
        self.checked_track(now).await.map(|checked| checked.path)
    }

    async fn checked_track(&mut self, now: u64) -> Result<CheckedTrack, &'static str> {
        self.check_current(now)?;
        let pc = self.pc.as_ref().ok_or(DRIVER)?;
        let role = match self.proof.role() {
            SessionRole::Host => Role::Host,
            SessionRole::Viewer => Role::Viewer,
        };
        let checked = timeout(self.wait(PAIR_WAIT), read_video_pair(pc.as_ref(), role))
            .await
            .map_err(|_| WAIT)
            .and_then(|r| r)
            .and_then(|(pair, track)| {
                Ok((
                    to_candidate(pair.local())?,
                    to_candidate(pair.remote())?,
                    track,
                ))
            });
        let (local, remote, track) = match checked {
            Ok(checked) => checked,
            Err(error) => {
                if self.path_verified || error == PAIR_REJECTED {
                    self.stop();
                }
                return Err(error);
            }
        };
        let result = (|| {
            self.check_current(now)?;
            let approved_local = self.local_candidates.as_ref().ok_or(PEER)?;
            let approved_remote = self.runtime.peers.read().map_err(|_| PEER)?;
            let path = direct_policy::selected(
                local,
                remote,
                approved_local,
                approved_remote.as_ref().ok_or(PEER)?,
            )
            .map_err(|_| PEER)?;
            Ok(CheckedTrack {
                path,
                pair: (local, remote),
                track,
            })
        })();
        if result.is_ok() {
            self.path_verified = true;
        } else {
            self.stop();
        }
        result
    }

    /// Validate the negotiated video sender and its actual signed path
    /// before handing a packet to the driver. A canceled write revokes
    /// the runtime since its handoff status is not known.
    pub(crate) async fn send_rtp(&mut self, packet: Packet, now: u64) -> Result<(), &'static str> {
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        self.check_current(now)?;
        if self.proof.role() != SessionRole::Host {
            return Err(RTP_INVALID);
        }
        validate_rtp(&packet)?;
        let VideoTrack::Local(track) = self.checked_track(now).await?.track else {
            return Err(RTP_IO);
        };
        self.check_current(now)?;
        timeout(self.wait(PAIR_WAIT), track.write_rtp(packet))
            .await
            .map_err(|_| RTP_IO)?
            .map_err(|_| RTP_IO)?;
        self.check_current(now)?;
        guard.0 = None;
        Ok(())
    }

    /// Receive only from the same signed live video transport before and
    /// after dequeuing RTP. Not ready or no packet during the short poll
    /// is nonterminal; wrong RTP/path and cancellation revoke the runtime.
    pub(crate) async fn recv_rtp(
        &mut self,
        now: u64,
    ) -> Result<Option<(Packet, PathSummary)>, &'static str> {
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        self.check_current(now)?;
        if self.proof.role() != SessionRole::Viewer {
            return Err(RTP_INVALID);
        }
        if !self.seen_track.load(Ordering::Acquire) {
            guard.0 = None;
            return Ok(None);
        }
        let CheckedTrack {
            pair,
            track: VideoTrack::Remote(track),
            ..
        } = (match self.checked_track(now).await {
            Err(PAIR_UNAVAILABLE) if !self.path_verified => {
                guard.0 = None;
                return Ok(None);
            }
            other => other?,
        })
        else {
            return Err(RTP_IO);
        };
        let packet = timeout(self.wait(MEDIA_POLL), async {
            loop {
                match track.poll().await {
                    Some(TrackRemoteEvent::OnRtpPacket(packet)) => return Ok(packet),
                    Some(
                        TrackRemoteEvent::OnError
                        | TrackRemoteEvent::OnEnding
                        | TrackRemoteEvent::OnEnded,
                    )
                    | None => return Err(RTP_IO),
                    Some(_) => continue,
                }
            }
        })
        .await;
        self.check_current(now)?;
        let Ok(packet) = packet else {
            guard.0 = None;
            return Ok(None);
        };
        let packet = packet?;
        validate_rtp(&packet)?;
        let CheckedTrack {
            pair: current_pair,
            track: VideoTrack::Remote(current_track),
            path,
        } = self.checked_track(now).await?
        else {
            return Err(RTP_IO);
        };
        if pair != current_pair || !Arc::ptr_eq(&track, &current_track) {
            return Err(PAIR_REJECTED);
        }
        self.check_current(now)?;
        guard.0 = None;
        Ok(Some((packet, path)))
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn is_immediate_control_batch(&self) -> bool {
        self.proof.is_immediate_control_batch()
    }

    #[cfg(feature = "v2-public-control-sustained")]
    pub(crate) fn is_sustained_control(&self) -> bool {
        self.proof.is_sustained_control()
    }

    /// Poll for the one reliable device-bound control channel. A video-only
    /// owner cannot create or claim it. The returned transport does not grant
    /// consent; its lifetime shares the revocable video/ICE boundary.
    #[cfg(feature = "v2-public-control")]
    pub async fn open_control(
        &mut self,
        now: u64,
    ) -> Result<Option<PublicControlTransport>, &'static str> {
        let scope = self.proof.control_scope().ok_or(CONTROL_INVALID)?;
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        self.check_current(now)?;
        if self.control.opened {
            return Err(CONTROL_INVALID);
        }
        let ready = (|| {
            if !self.remote_applied || self.local_candidates.is_none() {
                return Ok(false);
            }
            if self.control.channel.is_none() {
                match self.control.incoming.as_mut().ok_or(CONTROL_IO)?.try_recv() {
                    Ok(channel) => self.control.channel = Some(channel),
                    Err(mpsc::error::TryRecvError::Empty) => return Ok(false),
                    Err(mpsc::error::TryRecvError::Disconnected) => return Err(CONTROL_IO),
                }
            }
            Ok(true)
        })()?;
        if !ready {
            guard.0 = None;
            return Ok(None);
        }
        let channel = self.control.channel.as_ref().ok_or(CONTROL_IO)?;
        let pc = self.pc.as_ref().ok_or(DRIVER)?;
        let result = timeout(self.wait(PAIR_WAIT), async {
            match channel.ready_state().await.map_err(|_| CONTROL_IO)? {
                RTCDataChannelState::Connecting => return Ok(None),
                RTCDataChannelState::Open => {}
                _ => return Err(CONTROL_IO),
            }
            if channel.label().await.map_err(|_| CONTROL_IO)? != CONTROL_LABEL
                || channel.protocol().await.map_err(|_| CONTROL_IO)? != CONTROL_PROTOCOL
                || !channel.ordered().await.map_err(|_| CONTROL_IO)?
                || channel.negotiated().await.map_err(|_| CONTROL_IO)?
                || channel
                    .max_retransmits()
                    .await
                    .map_err(|_| CONTROL_IO)?
                    .is_some()
                || channel
                    .max_packet_life_time()
                    .await
                    .map_err(|_| CONTROL_IO)?
                    .is_some()
            {
                return Err(CONTROL_INVALID);
            }
            let sctp = pc.sctp().await.ok_or(CONTROL_IO)?;
            let local = self.local_candidates.as_ref().ok_or(PEER)?;
            let selected = sctp
                .transport()
                .ice_transport()
                .get_selected_candidate_pair()
                .await
                .map_err(|_| CONTROL_IO)?
                .ok_or(CONTROL_IO)?;
            let pair = {
                let remote = self.runtime.peers.read().map_err(|_| PEER)?;
                let pair = (
                    to_candidate(selected.local())?,
                    to_candidate(selected.remote())?,
                );
                direct_policy::selected(pair.0, pair.1, local, remote.as_ref().ok_or(PEER)?)
                    .map_err(|_| PEER)?;
                pair
            };
            check_control_pair(&*sctp, pair).await?;
            Ok(Some((sctp, pair)))
        })
        .await
        .map_err(|_| CONTROL_IO)??;
        self.check_current(now)?;
        guard.0 = None;
        let Some((sctp, pair)) = result else {
            return Ok(None);
        };
        self.control.opened = true;
        Ok(Some(PublicControlTransport {
            role: self.proof.role(),
            scope,
            immediate_batch: self.proof.is_immediate_control_batch(),
            #[cfg(feature = "v2-public-control-sustained")]
            sustained: self.proof.is_sustained_control(),
            runtime: self.runtime.clone(),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            channel: self.control.channel.take().ok_or(CONTROL_IO)?,
            sctp,
            pair,
        }))
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
            .effective_deadline()
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

    pub(crate) fn check_current(&self, now: u64) -> Result<(), &'static str> {
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

#[cfg(feature = "v2-public-control")]
async fn check_control_pair(
    sctp: &dyn SctpTransport,
    exact: (Candidate, Candidate),
) -> Result<(), &'static str> {
    let dtls = sctp.transport();
    let ice = dtls.ice_transport();
    let connected = async || -> Result<(), &'static str> {
        if sctp.state().await.map_err(|_| CONTROL_IO)? != RTCSctpTransportState::Connected
            || dtls.state().await.map_err(|_| CONTROL_IO)? != RTCDtlsTransportState::Connected
            || !matches!(
                ice.state().await.map_err(|_| CONTROL_IO)?,
                RTCIceTransportState::Connected | RTCIceTransportState::Completed
            )
        {
            return Err(CONTROL_IO);
        }
        Ok(())
    };
    connected().await?;
    let pair = ice
        .get_selected_candidate_pair()
        .await
        .map_err(|_| CONTROL_IO)?
        .ok_or(CONTROL_IO)?;
    let actual = (to_candidate(pair.local())?, to_candidate(pair.remote())?);
    connected().await?;
    if actual != exact || exact.0.address == exact.1.address {
        return Err(PAIR_REJECTED);
    }
    Ok(())
}

/// The only native holder of this physical session's reliable control lane.
/// No caller-supplied identity, candidate, alternate channel, or renewal.
#[cfg(feature = "v2-public-control")]
pub struct PublicControlTransport {
    role: SessionRole,
    scope: InputScope,
    immediate_batch: bool,
    #[cfg(feature = "v2-public-control-sustained")]
    sustained: bool,
    runtime: Arc<ScopedRuntime>,
    issued_at: u64,
    expires_at: u64,
    channel: Arc<dyn DataChannel>,
    sctp: Arc<dyn SctpTransport>,
    pair: (Candidate, Candidate),
}

#[cfg(feature = "v2-public-control")]
impl PublicControlTransport {
    pub fn scope(&self) -> InputScope {
        self.scope
    }

    pub fn role(&self) -> SessionRole {
        self.role
    }

    pub fn deadline(&self) -> Instant {
        self.runtime.effective_deadline()
    }

    pub(crate) fn begin_immediate_batch_round(&self) -> Result<Instant, &'static str> {
        self.current()?;
        if !self.immediate_batch {
            return Err(CONTROL_INVALID);
        }
        let until = (Instant::now() + Duration::from_secs(60)).min(self.runtime.deadline);
        self.set_round_deadline(until)
    }

    #[cfg(feature = "v2-public-control-sustained")]
    pub fn is_sustained(&self) -> bool {
        self.sustained
    }

    #[cfg(feature = "v2-public-control-sustained")]
    pub(crate) fn begin_sustained_round(&self) -> Result<Instant, &'static str> {
        self.current()?;
        if !self.sustained {
            return Err(CONTROL_INVALID);
        }
        let until = Instant::now()
            + Duration::from_secs(crate::public_video::SUSTAINED_CONTROL_MEDIA_BOUND_SECONDS);
        // Do not advertise a complete run from a partial capability; no
        // fallback and no TURN or socket lease extension.
        if until > self.runtime.deadline {
            return Err(CONTROL_INVALID);
        }
        self.set_round_deadline(until)
    }

    fn set_round_deadline(&self, until: Instant) -> Result<Instant, &'static str> {
        let round = match &self.runtime.physical {
            RouteRuntime::Relay(relay) => &relay.round_deadline,
            RouteRuntime::Direct(direct) => &direct.boundary.round_deadline,
        }
        .as_ref()
        .ok_or(CONTROL_INVALID)?;
        round.set(until).map_err(|_| CONTROL_INVALID)?;
        Ok(until)
    }

    fn current(&self) -> Result<(), &'static str> {
        let now = crate::public_video_wss::wall_seconds()?;
        if self.runtime.active() && (self.issued_at..self.expires_at).contains(&now) {
            Ok(())
        } else {
            Err(CLOSED)
        }
    }

    async fn check(&self) -> Result<(), &'static str> {
        self.current()?;
        let wait = self
            .deadline()
            .saturating_duration_since(Instant::now())
            .min(PAIR_WAIT);
        timeout(wait, check_control_pair(&*self.sctp, self.pair))
            .await
            .map_err(|_| CONTROL_IO)??;
        self.current()
    }

    /// A fresh selected path, checked before and after I/O. Only an already
    /// authorized native owner may make input/ACK decisions.
    pub async fn send_text(&mut self, text: &str) -> Result<(), &'static str> {
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        if text.is_empty() || text.len() > MAX_CONTROL_BYTES || text.contains('\0') {
            return Err(CONTROL_INVALID);
        }
        self.check().await?;
        timeout(PAIR_WAIT, self.channel.try_send_text(text))
            .await
            .map_err(|_| CONTROL_IO)?
            .map_err(|_| CONTROL_IO)?;
        self.check().await?;
        guard.0 = None;
        Ok(())
    }

    /// Short nonterminal poll; unexpected binary, bad UTF-8, closure or path
    /// loss is terminal. Callers must still enforce consent and typed syntax.
    pub async fn recv_text(&mut self) -> Result<Option<bytes::Bytes>, &'static str> {
        let mut guard = RevokeOnCancel(Some(self.runtime.clone()));
        self.check().await?;
        let packet = timeout(MEDIA_POLL, async {
            loop {
                match self.channel.poll().await {
                    Some(DataChannelEvent::OnMessage(packet))
                        if packet.is_string
                            && !packet.data.is_empty()
                            && packet.data.len() <= MAX_CONTROL_BYTES
                            && std::str::from_utf8(&packet.data).is_ok()
                            && !packet.data.contains(&0) =>
                    {
                        return Ok(packet.data.freeze())
                    }
                    Some(
                        DataChannelEvent::OnOpen
                        | DataChannelEvent::OnBufferedAmountHigh
                        | DataChannelEvent::OnBufferedAmountLow,
                    ) => {}
                    Some(DataChannelEvent::OnMessage(_)) => return Err(CONTROL_INVALID),
                    _ => return Err(CONTROL_IO),
                }
            }
        })
        .await;
        let packet = packet.ok().transpose()?;
        self.check().await?;
        guard.0 = None;
        Ok(packet)
    }
}

#[cfg(feature = "v2-public-control")]
impl Drop for PublicControlTransport {
    fn drop(&mut self) {
        self.runtime.stop();
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

#[cfg(all(test, feature = "v2-public-control"))]
#[path = "public_control_scoped_ice_tests.rs"]
mod control_tests;

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

    #[cfg(feature = "v2-public-control")]
    #[tokio::test]
    async fn automatic_batch_production_plan_preserves_scope_and_deadlines() {
        let identity = Identity::generate_ephemeral().unwrap();
        let peer = Identity::generate_ephemeral().unwrap();
        let pin = identity
            .verify_peer(
                &PairingCandidate::parse(&peer.pairing_code()).unwrap(),
                &peer.public().fingerprint,
            )
            .unwrap();
        let room = Uuid::new_v4();
        let scope = PublicVideoScope {
            expires_at: NOW + 1800,
            ..scope(IceMode::DirectPreferred)
        };
        let proof = crate::public_video::PublicControlProof::prepare_immediate_batch(
            &identity,
            &pin,
            SessionRole::Host,
            room,
            [1; 32],
            scope,
            NOW,
        )
        .unwrap()
        .into_inner();
        let target = RelayTarget {
            local: "192.168.1.2".parse().unwrap(),
            turn: scope.turn,
            relay: *scope.turn.ip(),
            port_min: scope.relay_ports.0,
            port_max: scope.relay_ports.1,
        };
        let grant = || RelayGrant::new(target, &room.to_string(), scope.expires_at, true).unwrap();
        let config = |mode| IceConfig {
            mode,
            expires_at: scope.expires_at,
            session_id: room.to_string(),
            ice_servers: vec![IceServer {
                urls: vec![format!("turn:{TURN}?transport=udp")],
                username: format!("{}:{room}", scope.expires_at),
                credential: "AAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
            }],
        };
        assert!(proof
            .prepare_ice(grant(), config(IceMode::RelayOnly), NOW)
            .is_err());
        assert!(
            crate::direct_policy::DirectGrant::new(grant(), vec![], &config(scope.mode), NOW)
                .is_err()
        );
        let plan = proof.prepare_ice(grant(), config(scope.mode), NOW).unwrap();
        assert!(!plan.host_candidates_allowed());
        let rtc = plan.rtc_configuration(NOW).unwrap();
        assert_eq!(
            rtc.ice_transport_policy(),
            webrtc::peer_connection::RTCIceTransportPolicy::All
        );
        assert_eq!(rtc.ice_servers().len(), 2);
        assert!(plan.rtc_configuration(scope.expires_at).is_err());
        let owner = PublicVideoScopedIce::new(proof, plan, NOW).unwrap();
        let RouteRuntime::Direct(direct) = &owner.runtime.physical else {
            panic!("direct runtime");
        };
        let round = direct.boundary.round_deadline.as_ref().unwrap();
        round.set(Instant::now()).unwrap();
        assert!(direct.boundary.deadline() <= Instant::now());
        assert!(!owner.runtime.active());
    }

    #[cfg(feature = "v2-public-control-sustained")]
    #[tokio::test]
    async fn sustained_signed_direct_production_plan_uses_scope_not_media_limit() {
        use crate::public_video::{
            PublicControlProof, SUSTAINED_CONTROL_MEDIA_BOUND_SECONDS,
            SUSTAINED_CONTROL_SCOPE_TTL_SECONDS,
        };
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let room = Uuid::new_v4();
        let scope = PublicVideoScope {
            expires_at: NOW + SUSTAINED_CONTROL_SCOPE_TTL_SECONDS,
            ..scope(IceMode::DirectPreferred)
        };
        let pin = |local: &Identity, peer: &Identity| {
            local
                .verify_peer(
                    &PairingCandidate::parse(&peer.pairing_code()).unwrap(),
                    &peer.public().fingerprint,
                )
                .unwrap()
        };
        let make = |ttl: u64| {
            let expires_at = NOW + ttl;
            let relay = RelayGrant::new(
                RelayTarget {
                    local: "192.168.1.2".parse().unwrap(),
                    turn: scope.turn,
                    relay: *scope.turn.ip(),
                    port_min: scope.relay_ports.0,
                    port_max: scope.relay_ports.1,
                },
                &room.to_string(),
                expires_at,
                true,
            )
            .unwrap();
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
            (relay, config)
        };
        let host_pin = pin(&host, &viewer);
        let viewer_pin = pin(&viewer, &host);
        assert!(PublicControlProof::prepare_immediate_batch(
            &host, &host_pin, SessionRole::Host, room, [1; 32], scope, NOW,
        )
        .is_err());
        assert!(PublicControlProof::prepare_sustained(
            &host, &host_pin, SessionRole::Host, room, [1; 32],
            PublicVideoScope { expires_at: scope.expires_at + 1, ..scope }, NOW,
        )
        .is_err());
        fn stage<'a>(
            proof: PublicVideoProof<'a>,
            (grant, config): (RelayGrant, IceConfig),
            expires_at: u64,
        ) -> PublicVideoScopedIce<'a> {
            let plan = proof.prepare_ice(grant, config, NOW).unwrap();
            let RouteGrant::Direct(direct) = &plan.grant else {
                panic!("direct plan");
            };
            assert!(Boundary::new(direct, expires_at).is_err());
            PublicVideoScopedIce::new(proof, plan, NOW).unwrap()
        }
        let host_proof = PublicControlProof::prepare_sustained(
            &host, &host_pin, SessionRole::Host, room, [1; 32], scope, NOW,
        )
        .unwrap()
        .into_inner();
        let viewer_proof = PublicControlProof::prepare_sustained(
            &viewer, &viewer_pin, SessionRole::Viewer, room, [1; 32], scope, NOW,
        )
        .unwrap()
        .into_inner();
        let mut host_ice = stage(
            host_proof,
            make(SUSTAINED_CONTROL_SCOPE_TTL_SECONDS),
            scope.expires_at,
        );
        let mut viewer_ice = stage(
            viewer_proof,
            make(SUSTAINED_CONTROL_SCOPE_TTL_SECONDS),
            scope.expires_at,
        );
        let fingerprint = ["AB"; 32].join(":");
        let sdp = format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
             a=fingerprint:sha-256 {fingerprint}\r\na=group:BUNDLE 0 1\r\n\
             m=video 9 UDP/TLS/RTP/SAVPF 102\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\n\
             a=ice-ufrag:abcd\r\na=ice-pwd:abcdefghijklmnopqrstuv\r\na=setup:actpass\r\n\
             a=recvonly\r\na=rtcp-mux\r\na=rtpmap:102 H264/90000\r\n\
             a=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\n\
             a=candidate:s 1 udp 1 1.1.1.1 50002 typ srflx raddr 0.0.0.0 rport 0\r\n\
             a=end-of-candidates\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
             c=IN IP4 0.0.0.0\r\na=mid:1\r\na=ice-ufrag:abcd\r\n\
             a=ice-pwd:abcdefghijklmnopqrstuv\r\na=setup:actpass\r\na=sendrecv\r\n\
             a=sctp-port:5000\r\na=max-message-size:65536\r\n",
        );
        let signature = viewer_ice
            .seal_local_sdp(host_ice.challenge().unwrap(), &sdp, NOW)
            .unwrap();
        assert_eq!(host_ice.admit_peer_sdp(&sdp, &signature, NOW), Ok(sdp.as_str()));

        let RouteRuntime::Direct(direct) = &host_ice.runtime.physical else {
            panic!("direct runtime");
        };
        let round = direct.boundary.round_deadline.as_ref().unwrap();
        let media_end = Instant::now() + Duration::from_secs(SUSTAINED_CONTROL_MEDIA_BOUND_SECONDS);
        assert!(direct.boundary.deadline() > media_end);
        round.set(media_end).unwrap();
        assert_eq!(host_ice.runtime.effective_deadline(), media_end);
        assert!(round.set(media_end + Duration::from_secs(1)).is_err());

        let (grant, config) = make(SUSTAINED_CONTROL_SCOPE_TTL_SECONDS + 1);
        assert!(crate::direct_policy::DirectGrant::new_control_sustained(grant, &config, NOW).is_err());
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
    async fn rtp_role_or_header_failure_revokes_before_driver_access() {
        let h = Identity::generate_ephemeral().unwrap();
        let v = Identity::generate_ephemeral().unwrap();
        let scope = scope(IceMode::DirectPreferred);
        let room = Uuid::new_v4();
        let valid = Packet {
            header: rtc::rtp::header::Header {
                version: 2,
                payload_type: VIDEO_PT,
                ssrc: crate::video::SSRC,
                ..Default::default()
            },
            payload: bytes::Bytes::from_static(&[0x65, 0x88]),
        };
        assert_eq!(validate_rtp(&valid), Ok(()));
        let mut invalid = valid.clone();
        invalid.header.ssrc ^= 1;
        assert_eq!(validate_rtp(&invalid), Err(RTP_INVALID));
        let mut invalid = valid.clone();
        invalid.header.payload_type += 1;
        assert_eq!(validate_rtp(&invalid), Err(RTP_INVALID));
        let mut host = stage(&h, &v, SessionRole::Host, room, scope);
        assert_eq!(host.send_rtp(invalid, NOW).await, Err(RTP_INVALID));
        assert!(host.runtime.stopped.load(Ordering::Acquire));
        let mut viewer = stage(&v, &h, SessionRole::Viewer, room, scope);
        assert_eq!(viewer.send_rtp(valid, NOW).await, Err(RTP_INVALID));
        assert!(viewer.runtime.stopped.load(Ordering::Acquire));
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

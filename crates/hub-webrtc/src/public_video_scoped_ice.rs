//! Prepare a proof-owned UDP boundary without opening network sockets.
//! The physical socket and TURN relayer enforce separate address checks.

use crate::{
    direct_policy::VerifiedCandidates,
    direct_runtime::{Boundary, DirectRuntime},
    pinned_runtime::PinnedRuntime,
    public_video::{PublicVideoIcePlan, PublicVideoProof},
    public_video_route::RouteGrant,
    relay_runtime::RelayRuntime,
};
use hub_identity::session::SessionChallenge;
use std::{
    fmt,
    future::Future,
    io,
    net::{SocketAddr, TcpListener, UdpSocket},
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

/// Owns the transport and its one-shot device proof together. The provisional
/// boundary stays in native memory and cannot be used to dial via this API.
pub struct PublicVideoScopedIce<'a> {
    proof: PublicVideoProof<'a>,
    runtime: Arc<ScopedRuntime>,
    issued_at: u64,
    expires_at: u64,
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
        plan.rtc_configuration(now)?;
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
        })
    }

    pub fn challenge(&self) -> Option<SessionChallenge> {
        if !self.runtime.active() {
            return None;
        }
        self.proof.challenge()
    }

    pub fn seal_local_sdp(
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
    pub fn admit_peer_sdp<'s>(
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
    }

    fn check_current(&self, now: u64) -> Result<(), &'static str> {
        if !self.runtime.active() || !(self.issued_at..self.expires_at).contains(&now) {
            return Err(CLOSED);
        }
        Ok(())
    }

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

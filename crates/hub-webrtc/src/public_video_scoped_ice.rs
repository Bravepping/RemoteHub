//! Prepare a proof-owned UDP boundary without opening network sockets.
//! The outer TURN endpoint check is not authorization of TURN's inner peer.

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
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

const CLOSED: &str = "PUBLIC_VIDEO_ICE_BOUNDARY_CLOSED";
const SETUP: &str = "PUBLIC_VIDEO_ICE_BOUNDARY_SETUP";
const PEER: &str = "PUBLIC_VIDEO_ICE_BOUNDARY_PEER";

enum RouteRuntime {
    Direct(DirectRuntime),
    Relay(RelayRuntime),
}

impl RouteRuntime {
    fn stop(&self) {
        match self {
            Self::Direct(runtime) => runtime.boundary.stop(),
            Self::Relay(runtime) => runtime.stopped.store(true, Ordering::Release),
        }
    }

    fn admit(&self, candidates: VerifiedCandidates) -> Result<(), &'static str> {
        match self {
            Self::Direct(runtime) => runtime.boundary.install_peer(candidates).map_err(|_| PEER),
            // The physical relay socket can only reach the TURN listener.
            // Exact logical TURN peer enforcement belongs at the relayer.
            Self::Relay(_) => Ok(()),
        }
    }
}

/// Owns the transport and its one-shot device proof together. The provisional
/// boundary stays in native memory and cannot be used to dial via this API.
pub struct PublicVideoScopedIce<'a> {
    proof: PublicVideoProof<'a>,
    runtime: RouteRuntime,
    issued_at: u64,
    expires_at: u64,
    deadline: Instant,
    stopped: bool,
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
        let runtime = match &plan.grant {
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
                stopped: Arc::default(),
            }),
        };
        Ok(Self {
            proof,
            runtime,
            issued_at: now,
            expires_at,
            deadline,
            stopped: false,
        })
    }

    pub fn challenge(&self) -> Option<SessionChallenge> {
        if self.stopped || Instant::now() >= self.deadline {
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

    /// Accept only the exact signed SDP. The direct runtime admits its peer
    /// addresses before any caller could apply this SDP to a peer connection.
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
        self.stopped = true;
        self.runtime.stop();
    }

    fn check_current(&self, now: u64) -> Result<(), &'static str> {
        if self.stopped
            || !(self.issued_at..self.expires_at).contains(&now)
            || Instant::now() >= self.deadline
        {
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
            let RouteRuntime::Direct(runtime) = &host.runtime else {
                panic!("expected direct runtime");
            };
            let runtime = DirectRuntime {
                inner: runtime.inner.clone(),
                boundary: runtime.boundary.clone(),
            };
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
            let address: SocketAddr = DIRECT.parse().unwrap();
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
        let RouteRuntime::Direct(runtime) = &host.runtime else {
            panic!("expected direct runtime");
        };
        assert!(runtime.resolve_host(TURN).await.is_err());
        assert!(runtime.resolve_host(DIRECT).await.is_err());
        assert!(runtime.resolve_host("1.1.1.1:50003").await.is_err());
        assert!(host.challenge().is_none());
        assert_eq!(host.admit_peer_sdp(&sdp, &signature, NOW), Err(CLOSED));
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
        assert_eq!(host.admit_peer_sdp(&sdp, &sig, NOW), Ok(sdp.as_str()));
        let RouteRuntime::Relay(runtime) = &host.runtime else {
            panic!("expected relay runtime");
        };
        assert!(runtime.resolve_host(TURN).await.is_ok());
        assert!(runtime.resolve_host(RELAY).await.is_err());
        assert!(runtime.resolve_host(DIRECT).await.is_err());
        let stopped = runtime.stopped.clone();
        // Credential expiry is earlier than the signed room's expiry.
        assert_eq!(host.check_current(NOW + 89), Ok(()));
        assert_eq!(host.admit_peer_sdp(&sdp, &sig, NOW + 90), Err(CLOSED));
        assert!(stopped.load(Ordering::Acquire));

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

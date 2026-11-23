//! Signed video-only WSS: a reserved native scope binds the relay's literal
//! endpoint and CA to its device proof and scoped ICE driver. Loopback
//! fixtures are separate and never become a public-route fallback.

use crate::{
    authenticated_relay::{
        AuthenticatedLoopbackRelay, AuthenticatedRelayEvent, ClientError, PublicVideoEndpoint,
        RoomId,
    },
    direct_policy::{self, Candidate, Kind, PathSummary, SelectedRoute, VerifiedCandidates},
    ice_policy::{IceConfig, RelayGrant},
    lan_wire::Key,
    public_video::{PublicVideoIcePlan, PublicVideoProof, PublicVideoScope, PublicVideoScopedIce},
    public_video_media::ScopedVideoMedia,
};
use hub_identity::{
    pairing::VerifiedPeer,
    session::{SessionChallenge, SessionRole, CHALLENGE_BYTES},
    Identity,
};
use rtc::{
    peer_connection::transport::{
        RTCDtlsTransportState, RTCIceCandidatePair, RTCIceTransportState,
    },
    rtp::packet::Packet,
    rtp_transceiver::rtp_sender::RtpCodecKind,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    net::{SocketAddr, SocketAddrV4},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::watch,
    time::{timeout, timeout_at, Instant},
};
use uuid::Uuid;
use webrtc::{
    media_stream::{
        track_local::TrackLocal,
        track_remote::{TrackRemote, TrackRemoteEvent},
    },
    peer_connection::{PeerConnection, RTCIceCandidate, RTCIceCandidateType, RTCIceProtocol},
    rtp_transceiver::RTCRtpTransceiverDirection,
};

pub use crate::authenticated_relay::{LoopbackTlsTrust, RelayToken, Role, SignalKind};

const JOIN_WAIT: Duration = Duration::from_secs(5);
const CHALLENGE_WAIT: Duration = Duration::from_secs(3);
const PROOF_TTL: Duration = Duration::from_secs(15);
const MEDIA_IO: &str = "PUBLIC_VIDEO_RTP_UNAVAILABLE";
const PAIR_READ_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_CHALLENGE_WIRE: usize = CHALLENGE_BYTES * 4 + 3;
const PROTOCOL: &str = "PUBLIC_VIDEO_WSS_PROTOCOL";
const ENDPOINT: &str = "PUBLIC_VIDEO_WSS_LOOPBACK_REQUIRED";
const CLOSED: &str = "PUBLIC_VIDEO_WSS_CLOSED";
const TIMEOUT: &str = "PUBLIC_VIDEO_WSS_TIMEOUT";
const PAIR_REJECTED: &str = "PUBLIC_VIDEO_ICE_PAIR_REJECTED";
const PAIR_UNAVAILABLE: &str = "PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE";
const TRANSPORT: &str = "PUBLIC_VIDEO_WSS_TRANSPORT";

/// TLS and relay data are for an actual loopback fixture. The independently
/// known scope nominates a future public context for the exact signed bytes,
/// not the loopback server's endpoint or certificate.
pub struct PublicVideoWssConfig<'a> {
    pub address: SocketAddr,
    pub token: &'a RelayToken,
    pub room: Uuid,
    pub role: Role,
    pub pairing_key: Arc<Key>,
    pub wire_nonce: String,
    pub session_nonce: [u8; 32],
    pub trust: &'a LoopbackTlsTrust,
    pub local_identity: &'a Identity,
    pub peer: &'a VerifiedPeer,
    pub scope: PublicVideoScope,
}

/// One native provisioned WSS token bound to a role and room. It authorizes
/// access to the signaling relay, not device trust or permission to capture.
pub struct PublicVideoWssInvitation {
    room: Uuid,
    role: Role,
    expires_at: u64,
    token: RelayToken,
}

impl PublicVideoWssInvitation {
    pub fn new(
        room: Uuid,
        role: Role,
        expires_at: u64,
        token: RelayToken,
    ) -> Result<Self, &'static str> {
        if room.is_nil() || expires_at == 0 {
            return Err("PUBLIC_VIDEO_WSS_INVITATION");
        }
        Ok(Self {
            room,
            role,
            expires_at,
            token,
        })
    }
}

/// Native-only inputs, independently provisioned and approved. No frontend
/// message can select the public endpoint or reach the socket directly.
pub struct PublicVideoNetworkConfig<'a> {
    pub invitation: PublicVideoWssInvitation,
    pub pairing_key: Arc<Key>,
    pub wire_nonce: String,
    pub session_nonce: [u8; 32],
    pub ca_der: &'a [u8],
    pub local_identity: &'a Identity,
    pub peer: &'a VerifiedPeer,
    pub scope: PublicVideoScope,
    pub relay_grant: RelayGrant,
    pub ice_config: IceConfig,
}

struct PreparedPublicVideoRoute {
    endpoint: PublicVideoEndpoint,
    trust: LoopbackTlsTrust,
    plan: PublicVideoIcePlan,
    deadline: Instant,
}

/// The production route can't enter the caller-supplied/unsigned fixture
/// interface. It owns its validated media plan until challenges are exchanged.
pub struct PendingPublicNetworkVideoWss<'a> {
    wss: PendingPublicVideoWss<'a>,
    plan: PublicVideoIcePlan,
}

impl<'a> PendingPublicNetworkVideoWss<'a> {
    /// Host only: confirm signaling registration before sharing an invitation.
    pub async fn wait_for_host_registration(mut self) -> Result<Self, &'static str> {
        self.wss = timeout_at(
            self.wss.context.scope_deadline,
            self.wss.wait_for_host_registration(),
        )
        .await
        .map_err(|_| TIMEOUT)??;
        Ok(self)
    }

    pub async fn establish(self) -> Result<ScopedPublicVideoWss<'a>, &'static str> {
        let deadline = self.wss.context.scope_deadline;
        let (wire, proof) = timeout_at(deadline, self.wss.establish_proof())
            .await
            .map_err(|_| TIMEOUT)??;
        let ice = PublicVideoScopedIce::new(proof, self.plan, wire.check()?)?;
        Ok(ScopedPublicVideoWss { wire, ice })
    }

    pub async fn leave(self) -> Result<(), &'static str> {
        self.wss.leave().await
    }
}

fn prepare_public_route(
    scope: PublicVideoScope,
    invitation: &PublicVideoWssInvitation,
    ca_der: &[u8],
    grant: RelayGrant,
    config: IceConfig,
    now: u64,
) -> Result<PreparedPublicVideoRoute, &'static str> {
    scope.checked_mode(now)?;
    if invitation.expires_at != scope.expires_at {
        return Err("PUBLIC_VIDEO_WSS_INVITATION");
    }
    if ca_der.is_empty()
        || ca_der.len() > 64 * 1024
        || <[u8; 32]>::from(Sha256::digest(ca_der)) != scope.ca_sha256
    {
        return Err("PUBLIC_VIDEO_WSS_CA_PIN");
    }
    let trust = LoopbackTlsTrust::from_ca_der(ca_der).map_err(|_| "PUBLIC_VIDEO_WSS_CA_PIN")?;
    let plan = PublicVideoIcePlan::prepare(scope, invitation.room, grant, config, now)?;
    let deadline = Instant::now() + Duration::from_secs(plan.expires_at() - now);
    let endpoint = PublicVideoEndpoint::new(plan.local_ip(), scope.wss, plan.expires_at(), now)
        .map_err(|_| "PUBLIC_VIDEO_WSS_ENDPOINT")?;
    Ok(PreparedPublicVideoRoute {
        endpoint,
        trust,
        plan,
        deadline,
    })
}

struct ProofContext<'a> {
    room: Uuid,
    role: Role,
    session_nonce: [u8; 32],
    identity: &'a Identity,
    peer: &'a VerifiedPeer,
    scope: PublicVideoScope,
    scope_deadline: Instant,
}

pub struct PendingPublicVideoWss<'a> {
    relay: AuthenticatedLoopbackRelay,
    context: ProofContext<'a>,
    host_registered: bool,
}

/// Calling code only receives the exact bytes that passed the public-video
/// device proof and the video-only ICE policy. This is not media consent.
pub enum PublicVideoWssEvent {
    Signal {
        kind: SignalKind,
        data: String,
    },
    /// A server lifecycle event, not a signed application message.
    PeerLeft,
}

struct ScopedWire {
    relay: AuthenticatedLoopbackRelay,
    role: Role,
    peer_challenge: SessionChallenge,
    proof_end: Instant,
    scope_end: Instant,
    expires_at: u64,
}

pub struct PublicVideoWss<'a> {
    relay: Option<AuthenticatedLoopbackRelay>,
    proof: PublicVideoProof<'a>,
    role: Role,
    peer_challenge: Option<SessionChallenge>,
    local_candidates: Option<VerifiedCandidates>,
    remote_candidates: Option<VerifiedCandidates>,
    deadline: Instant,
    scope_expires_at: u64,
    scope_deadline: Instant,
}

impl<'a> PublicVideoWss<'a> {
    /// Native public client only. Reject inconsistent scope, role-bound
    /// invitation, trust and ICE plan before attempting any TCP connection.
    /// No implicit private/loopback route, media capture, or input.
    pub async fn connect_public_wss(
        cfg: PublicVideoNetworkConfig<'a>,
    ) -> Result<PendingPublicNetworkVideoWss<'a>, &'static str> {
        let now = wall_seconds()?;
        let room = cfg.invitation.room;
        let role = cfg.invitation.role;
        cfg.peer
            .session_binding(
                &cfg.local_identity.public(),
                session_role(role),
                *room.as_bytes(),
                cfg.session_nonce,
            )
            .map_err(|_| "PUBLIC_VIDEO_IDENTITY_MISMATCH")?;
        let route = prepare_public_route(
            cfg.scope,
            &cfg.invitation,
            cfg.ca_der,
            cfg.relay_grant,
            cfg.ice_config,
            now,
        )?;
        let room_id = RoomId::new(room.simple().to_string()).map_err(relay_error)?;
        let relay = timeout_at(
            route.deadline,
            AuthenticatedLoopbackRelay::connect_public_video_wss(
                route.endpoint,
                &cfg.invitation.token,
                room_id,
                role,
                cfg.pairing_key,
                cfg.wire_nonce,
                &route.trust,
            ),
        )
        .await
        .map_err(|_| TIMEOUT)?
        .map_err(public_connect_error)?;
        Ok(PendingPublicNetworkVideoWss {
            wss: PendingPublicVideoWss {
                relay,
                context: ProofContext {
                    room,
                    role,
                    session_nonce: cfg.session_nonce,
                    identity: cfg.local_identity,
                    peer: cfg.peer,
                    scope: cfg.scope,
                    scope_deadline: route.deadline,
                },
                host_registered: false,
            },
            plan: route.plan,
        })
    }

    /// Only the existing explicit loopback-WSS transport is available.
    /// Use the same local room and role for the transport and device proof.
    pub async fn connect_loopback_wss(
        config: PublicVideoWssConfig<'a>,
    ) -> Result<PendingPublicVideoWss<'a>, &'static str> {
        if !config.address.ip().is_loopback() || config.address.port() == 0 {
            return Err(ENDPOINT);
        }
        let now = wall_seconds()?;
        config.scope.checked_mode(now)?;
        let scope_deadline = Instant::now() + Duration::from_secs(config.scope.expires_at - now);
        config
            .peer
            .session_binding(
                &config.local_identity.public(),
                session_role(config.role),
                *config.room.as_bytes(),
                config.session_nonce,
            )
            .map_err(|_| "PUBLIC_VIDEO_IDENTITY_MISMATCH")?;
        let room = crate::authenticated_relay::RoomId::new(config.room.simple().to_string())
            .map_err(relay_error)?;
        let relay = AuthenticatedLoopbackRelay::connect_wss(
            config.address,
            config.token,
            room,
            config.role,
            config.pairing_key,
            config.wire_nonce,
            config.trust,
        )
        .await
        .map_err(relay_error)?;
        Ok(PendingPublicVideoWss {
            relay,
            context: ProofContext {
                room: config.room,
                role: config.role,
                session_nonce: config.session_nonce,
                identity: config.local_identity,
                peer: config.peer,
                scope: config.scope,
                scope_deadline,
            },
            host_registered: false,
        })
    }

    /// The viewer sends one offer and the host sends one answer. The proof
    /// binds the answer to the offer that the host first received and verified.
    pub async fn send_signal(&mut self, kind: SignalKind, sdp: String) -> Result<(), &'static str> {
        // Never restore this connection on cancellation, encoding, proof, or
        // protocol failure. A caller cannot reuse partially consumed state.
        let mut relay = self.relay.take().ok_or(CLOSED)?;
        let challenge = self.peer_challenge.take().ok_or(PROTOCOL)?;
        if kind != sdp_kind(self.role) {
            return Err(PROTOCOL);
        }
        let now = self.check_scope()?;
        if Instant::now() >= self.deadline {
            return Err(TIMEOUT);
        }
        let (signature, verified) = self.proof.seal_video_sdp(challenge, &sdp, now)?;
        let data = serde_json::to_string(&SignedSdp {
            payload: sdp,
            signature: signature.to_vec(),
        })
        .map_err(|_| PROTOCOL)?;
        timeout_at(self.deadline, relay.send_signal(kind, data))
            .await
            .map_err(|_| TIMEOUT)?
            .map_err(relay_error)?;
        self.check_scope()?;
        self.local_candidates = Some(verified);
        self.relay = Some(relay);
        Ok(())
    }

    /// Timeout is retryable only inside the proof/scope window. Peer departure,
    /// unexpected events (including ICE), and invalid signatures close the link.
    pub async fn next_event(
        &mut self,
        wait: Duration,
    ) -> Result<PublicVideoWssEvent, &'static str> {
        let mut relay = self.relay.take().ok_or(CLOSED)?;
        let scope_remaining = self.scope_expires_at - self.check_scope()?;
        let now = Instant::now();
        let incomplete = self.remote_candidates.is_none() || self.local_candidates.is_none();
        if now >= self.scope_deadline || (incomplete && now >= self.deadline) {
            return Err(TIMEOUT);
        }
        let mut wait = wait
            .min(Duration::from_secs(scope_remaining))
            .min(self.scope_deadline - now);
        if incomplete {
            wait = wait.min(self.deadline - now);
        }
        let incoming = relay.next_event(wait).await;
        let received_at = self.check_scope()?;
        let event = match incoming {
            Ok(AuthenticatedRelayEvent::PeerLeft) => return Ok(PublicVideoWssEvent::PeerLeft),
            Ok(AuthenticatedRelayEvent::Signal { kind, data })
                if self.remote_candidates.is_none() && kind == sdp_kind(self.role.peer()) =>
            {
                if Instant::now() >= self.deadline {
                    return Err(TIMEOUT);
                }
                let signed: SignedSdp = serde_json::from_str(&data).map_err(|_| PROTOCOL)?;
                let signature = signed.signature.try_into().map_err(|_| PROTOCOL)?;
                let verified =
                    self.proof
                        .verify_video_sdp(&signed.payload, &signature, received_at)?;
                self.check_scope()?;
                self.remote_candidates = Some(verified);
                PublicVideoWssEvent::Signal {
                    kind,
                    data: signed.payload,
                }
            }
            Ok(_) => return Err(PROTOCOL),
            Err(ClientError::Timeout) if !incomplete || Instant::now() < self.deadline => {
                self.relay = Some(relay);
                return Err(TIMEOUT);
            }
            Err(error) => return Err(relay_error(error)),
        };
        self.relay = Some(relay);
        Ok(event)
    }

    /// Validate a candidate snapshot. Prefer `check_live_video_pair` for a
    /// live connection; this method alone does not prove its inputs were selected.
    pub fn check_selected_pair(
        &mut self,
        actual_local: &RTCIceCandidate,
        actual_remote: &RTCIceCandidate,
    ) -> Result<PathSummary, &'static str> {
        let relay = self.relay.take().ok_or(CLOSED)?;
        self.check_scope()?;
        let path = self.validate_selected_pair(actual_local, actual_remote)?;
        self.relay = Some(relay);
        Ok(path)
    }

    /// Read the actual connected RTP DTLS transport and verify its exact
    /// selected ICE pair against the signed offer/answer. No SCTP fallback.
    /// Call before media use and again after a path change. On this wrapper,
    /// the viewer's receiver becomes available on the first RTP track event;
    /// validate before consuming or presenting that track.
    ///
    /// Failure or cancellation closes this signaling object, not the caller's
    /// peer connection. It never starts media or grants permission to use it.
    pub async fn check_live_video_pair(
        &mut self,
        pc: &dyn PeerConnection,
    ) -> Result<PathSummary, &'static str> {
        let relay = self.relay.take().ok_or(CLOSED)?;
        let (path, _) = self.checked_live_track(pc).await?;
        self.relay = Some(relay);
        Ok(path)
    }

    /// Write one video RTP packet using the same sender whose live path
    /// passed verification. This neither starts capture nor grants consent.
    pub async fn send_checked_rtp(
        &mut self,
        pc: &dyn PeerConnection,
        packet: Packet,
    ) -> Result<(), &'static str> {
        let relay = self.relay.take().ok_or(CLOSED)?;
        if self.role != Role::Host {
            return Err(PROTOCOL);
        }
        let (_, VideoTrack::Local(track)) = self.checked_live_track(pc).await? else {
            return Err(MEDIA_IO);
        };
        let deadline = self.scope_deadline.min(Instant::now() + PAIR_READ_TIMEOUT);
        timeout_at(deadline, track.write_rtp(packet))
            .await
            .map_err(|_| MEDIA_IO)?
            .map_err(|_| MEDIA_IO)?;
        self.check_scope()?;
        self.relay = Some(relay);
        Ok(())
    }

    /// Release one incoming packet to the caller after checking the receiver
    /// path. Call only after the track-open event; RTP itself creates the
    /// receiver handle in this wrapper, but need not reach a decoder yet.
    pub async fn recv_checked_rtp(
        &mut self,
        pc: &dyn PeerConnection,
    ) -> Result<Packet, &'static str> {
        let relay = self.relay.take().ok_or(CLOSED)?;
        if self.role != Role::Viewer {
            return Err(PROTOCOL);
        }
        let (_, VideoTrack::Remote(track)) = self.checked_live_track(pc).await? else {
            return Err(MEDIA_IO);
        };
        let deadline = self.scope_deadline.min(Instant::now() + PAIR_READ_TIMEOUT);
        let packet = timeout_at(deadline, async {
            loop {
                match track.poll().await {
                    Some(TrackRemoteEvent::OnRtpPacket(packet)) => return Ok(packet),
                    Some(
                        TrackRemoteEvent::OnError
                        | TrackRemoteEvent::OnEnding
                        | TrackRemoteEvent::OnEnded,
                    )
                    | None => return Err(MEDIA_IO),
                    Some(_) => continue,
                }
            }
        })
        .await
        .map_err(|_| MEDIA_IO)??;
        let (_, VideoTrack::Remote(current)) = self.checked_live_track(pc).await? else {
            return Err(MEDIA_IO);
        };
        if !Arc::ptr_eq(&track, &current) {
            return Err(MEDIA_IO);
        }
        self.relay = Some(relay);
        Ok(packet)
    }

    async fn checked_live_track(
        &self,
        pc: &dyn PeerConnection,
    ) -> Result<(PathSummary, VideoTrack), &'static str> {
        self.check_scope()?;
        if self.local_candidates.is_none() || self.remote_candidates.is_none() {
            return Err(PAIR_REJECTED);
        }
        let deadline = self.scope_deadline.min(Instant::now() + PAIR_READ_TIMEOUT);
        let read = timeout_at(deadline, read_video_pair(pc, self.role)).await;
        self.check_scope()?;
        let (pair, track) = read.map_err(|_| PAIR_UNAVAILABLE)??;
        let path = self.validate_selected_pair(pair.local(), pair.remote())?;
        Ok((path, track))
    }

    fn validate_selected_pair(
        &self,
        actual_local: &RTCIceCandidate,
        actual_remote: &RTCIceCandidate,
    ) -> Result<PathSummary, &'static str> {
        let local = self.local_candidates.as_ref().ok_or(PAIR_REJECTED)?;
        let remote = self.remote_candidates.as_ref().ok_or(PAIR_REJECTED)?;
        direct_policy::selected(
            to_candidate(actual_local)?,
            to_candidate(actual_remote)?,
            local,
            remote,
        )
        .map_err(|_| PAIR_REJECTED)
    }

    fn check_scope(&self) -> Result<u64, &'static str> {
        let now = wall_seconds()?;
        if now >= self.scope_expires_at || Instant::now() >= self.scope_deadline {
            return Err(TIMEOUT);
        }
        Ok(now)
    }

    /// Stop signaling. The underlying relay is consumed even if its leave
    /// notice cannot be delivered.
    pub async fn leave(mut self) -> Result<(), &'static str> {
        if let Some(relay) = self.relay.take() {
            relay.leave().await.map_err(relay_error)?;
        }
        Ok(())
    }
}

pub(crate) enum VideoTrack {
    Local(Arc<dyn TrackLocal>),
    Remote(Arc<dyn TrackRemote>),
}

pub(crate) async fn read_video_pair(
    pc: &dyn PeerConnection,
    role: Role,
) -> Result<(RTCIceCandidatePair, VideoTrack), &'static str> {
    let transceivers = pc.get_transceivers().await;
    let [video] = transceivers.as_slice() else {
        return Err(PAIR_UNAVAILABLE);
    };
    let expected_direction = match role {
        Role::Host => RTCRtpTransceiverDirection::Sendonly,
        Role::Viewer => RTCRtpTransceiverDirection::Recvonly,
    };
    if video.mid().await.map_err(|_| PAIR_UNAVAILABLE)?.is_none()
        || video
            .current_direction()
            .await
            .map_err(|_| PAIR_UNAVAILABLE)?
            != expected_direction
    {
        return Err(PAIR_UNAVAILABLE);
    }
    let (dtls, track) = match role {
        Role::Host => {
            let sender = video
                .sender()
                .await
                .map_err(|_| PAIR_UNAVAILABLE)?
                .ok_or(PAIR_UNAVAILABLE)?;
            if sender.track().kind().await != RtpCodecKind::Video {
                return Err(PAIR_UNAVAILABLE);
            }
            (
                sender.transport().await,
                VideoTrack::Local(sender.track().clone()),
            )
        }
        Role::Viewer => {
            let receiver = video
                .receiver()
                .await
                .map_err(|_| PAIR_UNAVAILABLE)?
                .ok_or(PAIR_UNAVAILABLE)?;
            if receiver.track().kind().await != RtpCodecKind::Video {
                return Err(PAIR_UNAVAILABLE);
            }
            (
                receiver.transport().await,
                VideoTrack::Remote(receiver.track().clone()),
            )
        }
    };
    let dtls = dtls
        .map_err(|_| PAIR_UNAVAILABLE)?
        .ok_or(PAIR_UNAVAILABLE)?;
    if dtls.state().await.map_err(|_| PAIR_UNAVAILABLE)? != RTCDtlsTransportState::Connected {
        return Err(PAIR_UNAVAILABLE);
    }
    let ice = dtls.ice_transport();
    let connected = |state| {
        matches!(
            state,
            RTCIceTransportState::Connected | RTCIceTransportState::Completed
        )
    };
    if !connected(ice.state().await.map_err(|_| PAIR_UNAVAILABLE)?) {
        return Err(PAIR_UNAVAILABLE);
    }
    let pair = ice
        .get_selected_candidate_pair()
        .await
        .map_err(|_| PAIR_UNAVAILABLE)?
        .ok_or(PAIR_UNAVAILABLE)?;
    if !connected(ice.state().await.map_err(|_| PAIR_UNAVAILABLE)?) {
        return Err(PAIR_UNAVAILABLE);
    }
    Ok((pair, track))
}

impl<'a> PendingPublicVideoWss<'a> {
    /// Confirm the host's Waiting event before constructing the other role.
    pub async fn wait_for_host_registration(mut self) -> Result<Self, &'static str> {
        if self.context.role != Role::Host {
            return Err(PROTOCOL);
        }
        self.register_host().await?;
        Ok(self)
    }

    async fn register_host(&mut self) -> Result<(), &'static str> {
        if !self.host_registered {
            if !matches!(
                self.relay
                    .next_event(JOIN_WAIT)
                    .await
                    .map_err(relay_error)?,
                AuthenticatedRelayEvent::Waiting
            ) {
                return Err(PROTOCOL);
            }
            self.host_registered = true;
        }
        Ok(())
    }

    /// Run both roles concurrently. A fresh proof clock starts after Paired.
    /// No SDP methods are available while the challenges are exchanged.
    pub async fn establish(self) -> Result<PublicVideoWss<'a>, &'static str> {
        let (wire, proof) = self.establish_proof().await?;
        Ok(PublicVideoWss {
            relay: Some(wire.relay),
            proof,
            role: wire.role,
            peer_challenge: Some(wire.peer_challenge),
            local_candidates: None,
            remote_candidates: None,
            deadline: wire.proof_end,
            scope_expires_at: wire.expires_at,
            scope_deadline: wire.scope_end,
        })
    }

    // Loopback signaling must not open real public UDP traffic. Only the
    // cfg(test) fixture can bind its restricted local ICE driver here.
    #[cfg(test)]
    async fn establish_scoped_fixture(
        self,
        make_ice: impl FnOnce(
            PublicVideoProof<'a>,
            u64,
        ) -> Result<PublicVideoScopedIce<'a>, &'static str>,
    ) -> Result<ScopedPublicVideoWss<'a>, &'static str> {
        let (wire, proof) = self.establish_proof().await?;
        let ice = make_ice(proof, wire.check()?)?;
        Ok(ScopedPublicVideoWss { wire, ice })
    }

    async fn establish_proof(mut self) -> Result<(ScopedWire, PublicVideoProof<'a>), &'static str> {
        if self.context.role == Role::Host {
            self.register_host().await?;
        }
        if !matches!(
            self.relay
                .next_event(JOIN_WAIT)
                .await
                .map_err(relay_error)?,
            AuthenticatedRelayEvent::Paired
        ) {
            return Err(PROTOCOL);
        }

        let ctx = self.context;
        let now = wall_seconds()?;
        let monotonic_now = Instant::now();
        let scope_deadline = ctx
            .scope_deadline
            .min(monotonic_now + Duration::from_secs(ctx.scope.expires_at.saturating_sub(now)));
        if monotonic_now >= scope_deadline {
            return Err(TIMEOUT);
        }
        let deadline = scope_deadline.min(monotonic_now + PROOF_TTL);
        let proof = PublicVideoProof::prepare(
            ctx.identity,
            ctx.peer,
            session_role(ctx.role),
            ctx.room,
            ctx.session_nonce,
            ctx.scope,
            now,
        )?;
        let challenge = proof.challenge().ok_or(PROTOCOL)?;
        let data = serde_json::to_string(&[challenge.to_bytes().to_vec()]).map_err(|_| PROTOCOL)?;
        let peer_challenge = timeout_at(deadline, async {
            self.relay
                .send_signal(challenge_kind(ctx.role), data)
                .await
                .map_err(relay_error)?;
            match self
                .relay
                .next_event(CHALLENGE_WAIT)
                .await
                .map_err(relay_error)?
            {
                AuthenticatedRelayEvent::Signal { kind, data }
                    if kind == challenge_kind(ctx.role.peer()) =>
                {
                    parse_challenge(&data, wall_seconds()?)
                }
                _ => Err(PROTOCOL),
            }
        })
        .await
        .map_err(|_| TIMEOUT)??;
        if wall_seconds()? >= ctx.scope.expires_at || Instant::now() >= scope_deadline {
            return Err(TIMEOUT);
        }
        Ok((
            ScopedWire {
                relay: self.relay,
                role: ctx.role,
                peer_challenge,
                proof_end: deadline,
                scope_end: scope_deadline,
                expires_at: ctx.scope.expires_at,
            },
            proof,
        ))
    }

    pub async fn leave(self) -> Result<(), &'static str> {
        self.relay.leave().await.map_err(relay_error)
    }
}

const SCOPED_POLL: Duration = Duration::from_millis(50);
const SCOPED_LEAVE: Duration = Duration::from_millis(500);
const SCOPED_PAIR_WAIT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopedWssState {
    Negotiating,
    /// Both signed descriptions applied. This is not proof of a usable path
    /// or of media, and must not be surfaced as video success.
    Established,
    /// Host RTP transport selected an exact signed pair. Not a media result.
    HostPathVerified(SelectedRoute, Kind, Kind),
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopedWssEnd {
    Stopped,
    PeerLeft,
}

/// One owner for the authenticated wire and the exact ICE driver. No raw
/// PeerConnection, unsigned SDP injection, or input is exposed.
pub struct ScopedPublicVideoWss<'a> {
    wire: ScopedWire,
    ice: PublicVideoScopedIce<'a>,
}

impl ScopedWire {
    fn check(&self) -> Result<u64, &'static str> {
        let now = wall_seconds()?;
        if now >= self.expires_at || Instant::now() >= self.scope_end {
            return Err(TIMEOUT);
        }
        Ok(now)
    }

    fn check_proof(&self) -> Result<u64, &'static str> {
        let now = self.check()?;
        if Instant::now() >= self.proof_end {
            return Err(TIMEOUT);
        }
        Ok(now)
    }

    async fn send(&mut self, payload: String, signature: [u8; 64]) -> Result<(), &'static str> {
        self.check_proof()?;
        let data = serde_json::to_string(&SignedSdp {
            payload,
            signature: signature.to_vec(),
        })
        .map_err(|_| PROTOCOL)?;
        timeout_at(
            self.proof_end,
            self.relay.send_signal(sdp_kind(self.role), data),
        )
        .await
        .map_err(|_| TIMEOUT)?
        .map_err(relay_error)?;
        self.check_proof()?;
        Ok(())
    }

    async fn receive(&mut self) -> Result<Option<(String, [u8; 64])>, &'static str> {
        self.check_proof()?;
        let left = self.proof_end.saturating_duration_since(Instant::now());
        match self.relay.next_event(left).await {
            Ok(AuthenticatedRelayEvent::PeerLeft) => Ok(None),
            Ok(AuthenticatedRelayEvent::Signal { kind, data })
                if kind == sdp_kind(self.role.peer()) =>
            {
                self.check_proof()?;
                let desc: SignedSdp = serde_json::from_str(&data).map_err(|_| PROTOCOL)?;
                let signature = desc.signature.try_into().map_err(|_| PROTOCOL)?;
                Ok(Some((desc.payload, signature)))
            }
            Ok(_) => Err(PROTOCOL),
            Err(error) => Err(relay_error(error)),
        }
    }

    async fn watch_after_negotiation(&mut self) -> Result<ScopedWssEnd, &'static str> {
        loop {
            self.check()?;
            let poll = SCOPED_POLL.min(self.scope_end.saturating_duration_since(Instant::now()));
            match self.relay.next_event(poll).await {
                Ok(AuthenticatedRelayEvent::PeerLeft) => return Ok(ScopedWssEnd::PeerLeft),
                Ok(_) => return Err(PROTOCOL),
                Err(ClientError::Timeout) => {}
                Err(error) => return Err(relay_error(error)),
            }
        }
    }

    // Keep reading short polls while ICE gathers. Never drop a pending WSS
    // read to start a send: cancellation consumes the authenticated stream.
    // A peer cannot legitimately send another signature in this phase.
    async fn while_no_signal<T>(
        &mut self,
        work: impl std::future::Future<Output = Result<T, &'static str>>,
    ) -> Result<Option<T>, &'static str> {
        let mut work = std::pin::pin!(work);
        loop {
            self.check_proof()?;
            let poll = SCOPED_POLL.min(self.proof_end.saturating_duration_since(Instant::now()));
            let mut read = std::pin::pin!(self.relay.next_event(poll));
            let (event, ready) = tokio::select! {
                biased;
                event = &mut read => (event, None),
                value = &mut work => (read.await, Some(value)),
            };
            match event {
                Ok(AuthenticatedRelayEvent::PeerLeft) => return Ok(None),
                Ok(_) => return Err(PROTOCOL),
                Err(ClientError::Timeout) => {
                    if let Some(value) = ready {
                        return value.map(Some);
                    }
                }
                Err(error) => return Err(relay_error(error)),
            }
        }
    }
}

impl ScopedPublicVideoWss<'_> {
    /// Consume the single session. An explicit stop, control-handle loss,
    /// peer departure, timeout or protocol failure revokes UDP before close.
    pub async fn run(
        self,
        stop: watch::Receiver<bool>,
        state: watch::Sender<ScopedWssState>,
    ) -> Result<ScopedWssEnd, &'static str> {
        self.run_inner(stop, state, None).await
    }

    /// Run a single bounded video-only session. Call only from native code
    /// after local capture/viewing approval; this does not start capture.
    pub async fn run_with_media(
        self,
        stop: watch::Receiver<bool>,
        state: watch::Sender<ScopedWssState>,
        media: ScopedVideoMedia,
    ) -> Result<ScopedWssEnd, &'static str> {
        if media.role() != self.wire.role {
            return Err(PROTOCOL);
        }
        self.run_inner(stop, state, Some(media)).await
    }

    async fn run_inner(
        mut self,
        mut stop: watch::Receiver<bool>,
        state: watch::Sender<ScopedWssState>,
        mut media: Option<ScopedVideoMedia>,
    ) -> Result<ScopedWssEnd, &'static str> {
        state.send_replace(ScopedWssState::Negotiating);
        let result = tokio::select! {
            biased;
            () = stop_requested(&mut stop) => Ok(ScopedWssEnd::Stopped),
            result = self.active(&state, media.as_mut()) => result,
        };
        if let Some(media) = &media {
            media.revoke();
        }
        self.ice.stop();
        // Leave is best effort (the socket may already be gone). Do not
        // make it a prerequisite for closing the native driver.
        let (closed, _) = tokio::join!(
            self.ice.close(),
            timeout(SCOPED_LEAVE, self.wire.relay.leave()),
        );
        state.send_replace(ScopedWssState::Closed);
        result.and_then(|end| closed.map(|()| end))
    }

    async fn active(
        &mut self,
        state: &watch::Sender<ScopedWssState>,
        media: Option<&mut ScopedVideoMedia>,
    ) -> Result<ScopedWssEnd, &'static str> {
        let now = self.wire.check_proof()?;
        timeout_at(self.wire.proof_end, self.ice.start(now))
            .await
            .map_err(|_| TIMEOUT)??;
        if self.wire.role == Role::Viewer {
            if !self.publish_local().await? || !self.apply_remote().await? {
                return Ok(ScopedWssEnd::PeerLeft);
            }
        } else if !self.apply_remote().await? || !self.publish_local().await? {
            return Ok(ScopedWssEnd::PeerLeft);
        }
        self.ice.check_current(self.wire.check()?)?;
        state.send_replace(ScopedWssState::Established);
        if let Some(media) = media {
            // WSS I/O remains pending independently of frame processing.
            // Only terminal failure/stop cancels either side of this select.
            return tokio::select! {
                biased;
                result = self.wire.watch_after_negotiation() => result,
                result = media.pump(&mut self.ice, state) => result.map(|()| ScopedWssEnd::Stopped),
            };
        }
        let pair_end = Instant::now() + SCOPED_PAIR_WAIT;
        let mut path = None;
        loop {
            let now = self.wire.check()?;
            self.ice.check_current(now)?;
            if self.wire.role == Role::Host {
                match self.ice.selected_path(now).await {
                    Ok(selected) => {
                        let key = (selected.route, selected.local_type, selected.remote_type);
                        if path != Some(key) {
                            path = Some(key);
                            state.send_replace(ScopedWssState::HostPathVerified(
                                key.0, key.1, key.2,
                            ));
                        }
                    }
                    Err(PAIR_UNAVAILABLE) if path.is_none() && Instant::now() < pair_end => {}
                    Err(error) => return Err(error),
                }
            }
            let poll = SCOPED_POLL.min(
                self.wire
                    .scope_end
                    .saturating_duration_since(Instant::now()),
            );
            match self.wire.relay.next_event(poll).await {
                Ok(AuthenticatedRelayEvent::PeerLeft) => return Ok(ScopedWssEnd::PeerLeft),
                Ok(_) => return Err(PROTOCOL),
                Err(ClientError::Timeout) => {}
                Err(error) => return Err(relay_error(error)),
            }
        }
    }

    async fn publish_local(&mut self) -> Result<bool, &'static str> {
        let now = self.wire.check_proof()?;
        let challenge = self.wire.peer_challenge;
        let Some((sdp, signature)) = self
            .wire
            .while_no_signal(self.ice.local_signal(challenge, now))
            .await?
        else {
            return Ok(false);
        };
        self.wire.send(sdp, signature).await?;
        Ok(true)
    }

    async fn apply_remote(&mut self) -> Result<bool, &'static str> {
        let Some((payload, signature)) = self.wire.receive().await? else {
            return Ok(false);
        };
        let now = self.wire.check_proof()?;
        Ok(self
            .wire
            .while_no_signal(self.ice.apply_peer_signal(&payload, &signature, now))
            .await?
            .is_some())
    }
}

async fn stop_requested(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedSdp {
    payload: String,
    signature: Vec<u8>,
}

pub(crate) fn to_candidate(actual: &RTCIceCandidate) -> Result<Candidate, &'static str> {
    if actual.protocol != RTCIceProtocol::Udp || actual.component != 1 {
        return Err(PAIR_REJECTED);
    }
    let ip = actual.address.parse().map_err(|_| PAIR_REJECTED)?;
    let kind = match actual.typ {
        RTCIceCandidateType::Host => Kind::Host,
        RTCIceCandidateType::Srflx => Kind::Srflx,
        RTCIceCandidateType::Relay => Kind::Relay,
        _ => return Err(PAIR_REJECTED),
    };
    Ok(Candidate {
        address: SocketAddrV4::new(ip, actual.port),
        kind,
    })
}

fn parse_challenge(data: &str, now: u64) -> Result<SessionChallenge, &'static str> {
    if data.len() > MAX_CHALLENGE_WIRE {
        return Err(PROTOCOL);
    }
    let [challenge]: [Vec<u8>; 1] = serde_json::from_str(data).map_err(|_| PROTOCOL)?;
    let challenge = SessionChallenge::from_bytes(challenge.try_into().map_err(|_| PROTOCOL)?)
        .map_err(|_| PROTOCOL)?;
    challenge.validate_for_signing(now).map_err(|_| PROTOCOL)?;
    Ok(challenge)
}

pub(crate) fn wall_seconds() -> Result<u64, &'static str> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|_| "PUBLIC_VIDEO_WSS_CLOCK")
}

fn public_connect_error(error: ClientError) -> &'static str {
    match error {
        ClientError::PublicVideoEndpoint | ClientError::PublicVideoBind => {
            "PUBLIC_VIDEO_WSS_ENDPOINT"
        }
        ClientError::TlsTrust | ClientError::TlsHandshake => "PUBLIC_VIDEO_WSS_TLS",
        ClientError::Unauthorized => "PUBLIC_VIDEO_WSS_ADMISSION",
        _ => relay_error(error),
    }
}

fn relay_error(error: ClientError) -> &'static str {
    match error {
        ClientError::Endpoint => ENDPOINT,
        ClientError::Closed => CLOSED,
        ClientError::Timeout | ClientError::PublicVideoExpiry => TIMEOUT,
        ClientError::Protocol | ClientError::Payload | ClientError::Rejected => PROTOCOL,
        _ => TRANSPORT,
    }
}

fn session_role(role: Role) -> SessionRole {
    match role {
        Role::Host => SessionRole::Host,
        Role::Viewer => SessionRole::Viewer,
    }
}

fn challenge_kind(role: Role) -> SignalKind {
    match role {
        Role::Host => SignalKind::HostChallenge,
        Role::Viewer => SignalKind::ViewerChallenge,
    }
}

fn sdp_kind(role: Role) -> SignalKind {
    match role {
        Role::Host => SignalKind::Answer,
        Role::Viewer => SignalKind::Offer,
    }
}

#[cfg(test)]
#[path = "public_video_scoped_wss_tests.rs"]
mod scoped_tests;

#[cfg(test)]
#[path = "public_video_network_tests.rs"]
mod network_tests;

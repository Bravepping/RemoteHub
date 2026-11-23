//! Loopback TLS fixture for the public-video proof protocol. It does not
//! connect to the public address carried in the proof, confer consent, or
//! open an ICE or media connection. RTP helpers only operate on an existing
//! caller-owned peer; product consent and socket scoping remain separate.

use crate::{
    authenticated_relay::{AuthenticatedLoopbackRelay, AuthenticatedRelayEvent, ClientError},
    direct_policy::{self, Candidate, Kind, PathSummary, VerifiedCandidates},
    lan_wire::Key,
    public_video::{PublicVideoProof, PublicVideoScope},
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
use std::{
    net::{SocketAddr, SocketAddrV4},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::{timeout_at, Instant};
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

enum VideoTrack {
    Local(Arc<dyn TrackLocal>),
    Remote(Arc<dyn TrackRemote>),
}

async fn read_video_pair(
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
    pub async fn establish(mut self) -> Result<PublicVideoWss<'a>, &'static str> {
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
        Ok(PublicVideoWss {
            relay: Some(self.relay),
            proof,
            role: ctx.role,
            peer_challenge: Some(peer_challenge),
            local_candidates: None,
            remote_candidates: None,
            deadline,
            scope_expires_at: ctx.scope.expires_at,
            scope_deadline,
        })
    }

    pub async fn leave(self) -> Result<(), &'static str> {
        self.relay.leave().await.map_err(relay_error)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedSdp {
    payload: String,
    signature: Vec<u8>,
}

fn to_candidate(actual: &RTCIceCandidate) -> Result<Candidate, &'static str> {
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

fn wall_seconds() -> Result<u64, &'static str> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|_| "PUBLIC_VIDEO_WSS_CLOCK")
}

fn relay_error(error: ClientError) -> &'static str {
    match error {
        ClientError::Endpoint => ENDPOINT,
        ClientError::Closed => CLOSED,
        ClientError::Timeout => TIMEOUT,
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

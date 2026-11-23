//! Loopback TLS fixture for the public-video proof protocol. It does not
//! connect to the public address carried in the proof, confer consent, parse
//! media SDP, or grant use of any ICE route.

use crate::{
    authenticated_relay::{AuthenticatedLoopbackRelay, AuthenticatedRelayEvent, ClientError},
    lan_wire::Key,
    public_video::{PublicVideoProof, PublicVideoScope},
};
use hub_identity::{
    pairing::VerifiedPeer,
    session::{SessionChallenge, SessionRole, CHALLENGE_BYTES},
    Identity,
};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::{timeout_at, Instant};
use uuid::Uuid;

pub use crate::authenticated_relay::{LoopbackTlsTrust, RelayToken, Role, SignalKind};

const JOIN_WAIT: Duration = Duration::from_secs(5);
const CHALLENGE_WAIT: Duration = Duration::from_secs(3);
const PROOF_TTL: Duration = Duration::from_secs(15);
const MAX_CHALLENGE_WIRE: usize = CHALLENGE_BYTES * 4 + 3;
const PROTOCOL: &str = "PUBLIC_VIDEO_WSS_PROTOCOL";
const ENDPOINT: &str = "PUBLIC_VIDEO_WSS_LOOPBACK_REQUIRED";
const CLOSED: &str = "PUBLIC_VIDEO_WSS_CLOSED";
const TIMEOUT: &str = "PUBLIC_VIDEO_WSS_TIMEOUT";
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
/// device proof. This is not media/route validation or authorization.
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
    received: bool,
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
        let signature = self.proof.seal(challenge, &sdp, now)?;
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
        let incomplete = !self.received || self.peer_challenge.is_some();
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
                if !self.received && kind == sdp_kind(self.role.peer()) =>
            {
                if Instant::now() >= self.deadline {
                    return Err(TIMEOUT);
                }
                let signed: SignedSdp = serde_json::from_str(&data).map_err(|_| PROTOCOL)?;
                let signature = signed.signature.try_into().map_err(|_| PROTOCOL)?;
                self.proof.open(&signed.payload, &signature, received_at)?;
                self.check_scope()?;
                self.received = true;
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
            received: false,
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

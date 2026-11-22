//! Opt-in native signaling for one device-verified SDP offer and answer over
//! loopback WSS. Device proof does not authorize media, input, or ICE routes:
//! callers still validate the authenticated bytes against their path policy.

use crate::{
    authenticated_relay::{AuthenticatedLoopbackRelay, AuthenticatedRelayEvent},
    lan_wire::Key,
};
use hub_identity::{
    pairing::VerifiedPeer,
    session::{
        PendingSessionProof, SessionBinding, SessionChallenge, SessionProofError, SessionRole,
        CHALLENGE_BYTES, MAX_SIGNAL_BYTES,
    },
    Identity,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::time::{timeout_at, Instant};

pub use crate::authenticated_relay::{
    ClientError, LoopbackTlsTrust, RelayToken, Role, RoomId, SignalKind,
};

const JOIN_WAIT: Duration = Duration::from_secs(5);
const CHALLENGE_WAIT: Duration = Duration::from_secs(3);
const PROOF_TTL: Duration = Duration::from_secs(15);
const MAX_CHALLENGE_WIRE: usize = CHALLENGE_BYTES * 4 + 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifiedWssError {
    Relay(ClientError),
    Proof(SessionProofError),
    Protocol,
}

impl fmt::Display for VerifiedWssError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relay(error) => error.fmt(formatter),
            Self::Proof(error) => error.fmt(formatter),
            Self::Protocol => formatter.write_str("VERIFIED_WSS_PROTOCOL"),
        }
    }
}
impl std::error::Error for VerifiedWssError {}
impl From<ClientError> for VerifiedWssError {
    fn from(error: ClientError) -> Self {
        Self::Relay(error)
    }
}
impl From<SessionProofError> for VerifiedWssError {
    fn from(error: SessionProofError) -> Self {
        Self::Proof(error)
    }
}

/// Context selected independently of the relay. Both nonces and the pairing
/// key must be fresh and established out of band for this room.
pub struct VerifiedWssConfig<'a> {
    pub address: SocketAddr,
    pub token: &'a RelayToken,
    pub room: RoomId,
    pub role: Role,
    pub pairing_key: Arc<Key>,
    pub wire_nonce: String,
    pub session_nonce: [u8; 32],
    pub trust: &'a LoopbackTlsTrust,
    pub local_identity: &'a Identity,
    pub peer: &'a VerifiedPeer,
}

/// Joined WSS transport that cannot send or receive SDP until both
/// role-separated HMAC-authenticated challenges have been exchanged.
pub struct PendingVerifiedWssSignaling<'a> {
    relay: AuthenticatedLoopbackRelay,
    role: Role,
    binding: SessionBinding,
    identity: &'a Identity,
    host_registered: bool,
}

pub enum VerifiedWssEvent {
    /// Exposes the exact device-signed bytes, not a network or consent approval.
    Signal {
        kind: SignalKind,
        data: String,
    },
    PeerLeft,
}

/// Resource for exactly one SDP message in each direction. No trickle ICE or
/// renegotiation. Each peer's identity is checked when its own SDP is received.
pub struct VerifiedWssSignaling<'a> {
    relay: Option<AuthenticatedLoopbackRelay>,
    binding: SessionBinding,
    identity: &'a Identity,
    role: Role,
    outbound: Option<SessionChallenge>,
    incoming: PendingSessionProof,
    incoming_verified: bool,
    deadline: Instant,
}

impl<'a> VerifiedWssSignaling<'a> {
    /// Validate pinned peer, actual local identity and room before any socket
    /// activity. This API selects WSS explicitly and cannot downgrade.
    pub async fn connect(
        config: VerifiedWssConfig<'a>,
    ) -> Result<PendingVerifiedWssSignaling<'a>, VerifiedWssError> {
        let room =
            uuid::Uuid::parse_str(config.room.as_str()).map_err(|_| VerifiedWssError::Protocol)?;
        let binding = config.peer.session_binding(
            &config.local_identity.public(),
            session_role(config.role),
            *room.as_bytes(),
            config.session_nonce,
        )?;
        let relay = AuthenticatedLoopbackRelay::connect_wss(
            config.address,
            config.token,
            config.room,
            config.role,
            config.pairing_key,
            config.wire_nonce,
            config.trust,
        )
        .await?;
        Ok(PendingVerifiedWssSignaling {
            relay,
            role: config.role,
            binding,
            identity: config.local_identity,
            host_registered: false,
        })
    }

    /// Send the nominated role's sole signed offer/answer. No application
    /// content may pass in an unsupported signal kind or second SDP.
    pub async fn send_signal(
        &mut self,
        kind: SignalKind,
        payload: String,
    ) -> Result<(), VerifiedWssError> {
        let mut relay = self.relay.take().ok_or(ClientError::Closed)?;
        let challenge = self.outbound.take().ok_or(VerifiedWssError::Protocol)?;
        if kind != sdp_kind(self.role) {
            return Err(VerifiedWssError::Protocol);
        }
        let signature = self.identity.sign_session_proof(
            &self.binding,
            session_role(self.role),
            challenge,
            &signal_bytes(kind, &payload)?,
            wall_seconds()?,
        )?;
        let signed = SignedSignal {
            payload,
            signature: signature.to_vec(),
        };
        let data = serde_json::to_string(&signed).map_err(|_| VerifiedWssError::Protocol)?;
        if Instant::now() >= self.deadline {
            return Err(ClientError::Timeout.into());
        }
        timeout_at(self.deadline, relay.send_signal(kind, data))
            .await
            .map_err(|_| ClientError::Timeout)??;
        self.relay = Some(relay);
        Ok(())
    }

    /// Return only fully authenticated SDP. A normal poll timeout may retry;
    /// cancellation, bad proof, unexpected event or a broken link closes it.
    pub async fn next_event(
        &mut self,
        wait: Duration,
    ) -> Result<VerifiedWssEvent, VerifiedWssError> {
        let mut relay = self.relay.take().ok_or(ClientError::Closed)?;
        // Once both SDP messages succeed, the caller may still observe a
        // PeerLeft lifecycle event. An incomplete exchange cannot wait or
        // retry beyond its single overall deadline.
        let incomplete = !self.incoming_verified || self.outbound.is_some();
        let wait = if incomplete {
            let now = Instant::now();
            if now >= self.deadline {
                return Err(ClientError::Timeout.into());
            }
            wait.min(self.deadline - now)
        } else {
            wait
        };
        let event = match relay.next_event(wait).await {
            Ok(AuthenticatedRelayEvent::Signal { kind, data })
                if kind == sdp_kind(self.role.peer()) =>
            {
                let signed: SignedSignal =
                    serde_json::from_str(&data).map_err(|_| VerifiedWssError::Protocol)?;
                let signature: [u8; 64] = signed
                    .signature
                    .try_into()
                    .map_err(|_| VerifiedWssError::Protocol)?;
                if Instant::now() >= self.deadline {
                    return Err(ClientError::Timeout.into());
                }
                self.incoming.verify(
                    &signature,
                    &signal_bytes(kind, &signed.payload)?,
                    wall_seconds()?,
                )?;
                self.incoming_verified = true;
                VerifiedWssEvent::Signal {
                    kind,
                    data: signed.payload,
                }
            }
            Ok(AuthenticatedRelayEvent::PeerLeft) => return Ok(VerifiedWssEvent::PeerLeft),
            Ok(_) => return Err(VerifiedWssError::Protocol),
            Err(ClientError::Timeout) if !incomplete || Instant::now() < self.deadline => {
                self.relay = Some(relay);
                return Err(ClientError::Timeout.into());
            }
            Err(error) => return Err(error.into()),
        };
        self.relay = Some(relay);
        Ok(event)
    }

    pub async fn leave(mut self) -> Result<(), VerifiedWssError> {
        if let Some(relay) = self.relay.take() {
            relay.leave().await?;
        }
        Ok(())
    }
}

impl<'a> PendingVerifiedWssSignaling<'a> {
    /// The host can await registration before asking the viewer to connect.
    /// It remains pending: no SDP methods are available in this state.
    pub async fn wait_for_host_registration(mut self) -> Result<Self, VerifiedWssError> {
        if self.role != Role::Host {
            return Err(VerifiedWssError::Protocol);
        }
        self.register_host().await?;
        Ok(self)
    }

    async fn register_host(&mut self) -> Result<(), VerifiedWssError> {
        if !self.host_registered {
            if !matches!(
                self.relay.next_event(JOIN_WAIT).await?,
                AuthenticatedRelayEvent::Waiting
            ) {
                return Err(VerifiedWssError::Protocol);
            }
            self.host_registered = true;
        }
        Ok(())
    }

    /// Run on the two peers concurrently. Challenge expiry starts only after
    /// Paired, not while the host is waiting for the viewer. Any mismatch,
    /// deadline, or canceled establishment drops the connection.
    pub async fn establish(mut self) -> Result<VerifiedWssSignaling<'a>, VerifiedWssError> {
        if self.role == Role::Host {
            self.register_host().await?;
        }
        if !matches!(
            self.relay.next_event(JOIN_WAIT).await?,
            AuthenticatedRelayEvent::Paired
        ) {
            return Err(VerifiedWssError::Protocol);
        }
        let deadline = Instant::now() + PROOF_TTL;
        let incoming = PendingSessionProof::issue(
            &self.binding,
            session_role(self.role.peer()),
            wall_seconds()?,
            PROOF_TTL,
        )?;
        let data = serde_json::to_string(&[incoming.challenge().to_bytes().to_vec()])
            .map_err(|_| VerifiedWssError::Protocol)?;
        self.relay
            .send_signal(challenge_kind(self.role), data)
            .await?;
        let outbound = match self.relay.next_event(CHALLENGE_WAIT).await? {
            AuthenticatedRelayEvent::Signal { kind, data }
                if kind == challenge_kind(self.role.peer()) =>
            {
                parse_challenge(&data, wall_seconds()?)?
            }
            _ => return Err(VerifiedWssError::Protocol),
        };
        Ok(VerifiedWssSignaling {
            relay: Some(self.relay),
            binding: self.binding,
            identity: self.identity,
            role: self.role,
            outbound: Some(outbound),
            incoming,
            incoming_verified: false,
            deadline,
        })
    }

    pub async fn leave(self) -> Result<(), VerifiedWssError> {
        self.relay.leave().await?;
        Ok(())
    }
}

fn parse_challenge(data: &str, now: u64) -> Result<SessionChallenge, VerifiedWssError> {
    if data.len() > MAX_CHALLENGE_WIRE {
        return Err(VerifiedWssError::Protocol);
    }
    let [challenge]: [Vec<u8>; 1] =
        serde_json::from_str(data).map_err(|_| VerifiedWssError::Protocol)?;
    let challenge = SessionChallenge::from_bytes(
        challenge
            .try_into()
            .map_err(|_| VerifiedWssError::Protocol)?,
    )?;
    challenge.validate_for_signing(now)?;
    Ok(challenge)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedSignal {
    payload: String,
    signature: Vec<u8>,
}

fn signal_bytes(kind: SignalKind, payload: &str) -> Result<Vec<u8>, VerifiedWssError> {
    let label = match kind {
        SignalKind::Offer => b'O',
        SignalKind::Answer => b'A',
        _ => return Err(VerifiedWssError::Protocol),
    };
    if payload.is_empty() || payload.len() >= MAX_SIGNAL_BYTES || payload.contains('\0') {
        return Err(SessionProofError::Payload.into());
    }
    let mut bytes = Vec::with_capacity(1 + payload.len());
    bytes.push(label);
    bytes.extend_from_slice(payload.as_bytes());
    Ok(bytes)
}

fn wall_seconds() -> Result<u64, SessionProofError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| SessionProofError::Clock)
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
mod tests {
    use super::*;

    #[test]
    fn challenge_parser_accepts_exactly_one_bounded_slot() {
        let mut bytes = [1; CHALLENGE_BYTES];
        bytes[32..40].copy_from_slice(&1_800_000_000_u64.to_be_bytes());
        bytes[40..48].copy_from_slice(&1_800_000_015_u64.to_be_bytes());
        let wire = serde_json::to_string(&[bytes.to_vec()]).unwrap();
        assert_eq!(
            parse_challenge(&wire, 1_800_000_000).unwrap().to_bytes(),
            bytes
        );
        assert!(matches!(
            parse_challenge(&wire, 1_800_000_015),
            Err(VerifiedWssError::Proof(SessionProofError::Expired))
        ));
        for invalid in [
            serde_json::to_string(&[bytes.to_vec(), bytes.to_vec()]).unwrap(),
            serde_json::to_string(&[bytes[..47].to_vec()]).unwrap(),
            " ".repeat(MAX_CHALLENGE_WIRE + 1),
        ] {
            assert!(parse_challenge(&invalid, 1_800_000_000).is_err());
        }
    }

    #[test]
    fn signed_inline_signals_are_kind_separated_bounded_and_not_trickle() {
        let offer = signal_bytes(SignalKind::Offer, "exact\r\n").unwrap();
        let answer = signal_bytes(SignalKind::Answer, "exact\r\n").unwrap();
        assert_eq!(offer, b"Oexact\r\n");
        assert_eq!(answer, b"Aexact\r\n");
        for kind in [
            SignalKind::Ice,
            SignalKind::HostChallenge,
            SignalKind::ViewerChallenge,
        ] {
            assert!(signal_bytes(kind, "synthetic").is_err());
        }
        for oversized in [String::new(), "\0".into(), "x".repeat(MAX_SIGNAL_BYTES)] {
            assert!(signal_bytes(SignalKind::Offer, &oversized).is_err());
        }
    }
}

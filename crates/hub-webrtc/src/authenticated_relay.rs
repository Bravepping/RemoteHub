//! Loopback-only V2 transport with room-bound authenticated opaque signaling.
//! Neither a valid relay session nor a verified payload authorizes capture,
//! input, or a network path. Validate SDP and ICE separately before use.

use crate::lan_wire::{Codec, Key};
use hub_signal_v2::{wire, LoopbackRelay, RelayEvent};
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub use hub_signal_v2::{ClientError, LoopbackTlsTrust, RelayToken, Role, RoomId, SignalKind};

/// Payloads are intentionally not serializable or Debug; do not log raw SDP/ICE
/// or send it through a diagnostic UI bridge.
pub enum AuthenticatedRelayEvent {
    Waiting,
    Paired,
    Signal { kind: SignalKind, data: String },
    PeerLeft,
}

pub struct AuthenticatedLoopbackRelay {
    // Move out across I/O so cancellation closes the TCP connection instead
    // of leaving partially sent frames or advanced sequence state reusable.
    relay: Option<LoopbackRelay>,
    codec: Codec,
}

impl AuthenticatedLoopbackRelay {
    /// Both native peers must possess the same independently established
    /// fresh key and nonce. Neither may come from the untrusted relay.
    pub async fn connect(
        address: SocketAddr,
        token: &RelayToken,
        room: RoomId,
        role: Role,
        key: Arc<Key>,
        nonce: String,
    ) -> Result<Self, ClientError> {
        Self::connect_with_trust(address, token, room, role, key, nonce, None).await
    }

    /// Use WSS with independently provisioned trust, checked against the
    /// literal loopback address. TLS failure is terminal, never a WS fallback.
    /// The same room-bound HMAC still authenticates every peer signal.
    pub async fn connect_wss(
        address: SocketAddr,
        token: &RelayToken,
        room: RoomId,
        role: Role,
        key: Arc<Key>,
        nonce: String,
        trust: &LoopbackTlsTrust,
    ) -> Result<Self, ClientError> {
        Self::connect_with_trust(address, token, room, role, key, nonce, Some(trust)).await
    }

    async fn connect_with_trust(
        address: SocketAddr,
        token: &RelayToken,
        room: RoomId,
        role: Role,
        key: Arc<Key>,
        nonce: String,
        trust: Option<&LoopbackTlsTrust>,
    ) -> Result<Self, ClientError> {
        let codec = Codec::new_v2(key, nonce, room.as_str(), role == Role::Host)
            .map_err(|_| ClientError::Protocol)?;
        let relay = match trust {
            Some(trust) => LoopbackRelay::connect_wss(address, token, room, role, trust).await?,
            None => LoopbackRelay::connect(address, token, room, role).await?,
        };
        Ok(Self {
            relay: Some(relay),
            codec,
        })
    }

    /// An unsuccessful send closes this single-use session. Reconnect with
    /// fresh credentials rather than risking reuse of an advanced sequence.
    pub async fn send_signal(&mut self, kind: SignalKind, data: String) -> Result<(), ClientError> {
        let mut relay = self.relay.take().ok_or(ClientError::Closed)?;
        if !wire::valid_signal(&data) {
            return Err(ClientError::Payload);
        }
        let frame = self
            .codec
            .encode(kind_name(kind), data)
            .map_err(|_| ClientError::Payload)?;
        let opaque = serde_json::to_string(&frame).map_err(|_| ClientError::Payload)?;
        relay.send_signal(kind, opaque).await?;
        self.relay = Some(relay);
        Ok(())
    }

    /// A normal poll timeout does not close the connection. A canceled poll or
    /// failed verification does. PeerLeft is a relay lifecycle notification,
    /// not an authenticated application payload.
    pub async fn next_event(
        &mut self,
        wait: Duration,
    ) -> Result<AuthenticatedRelayEvent, ClientError> {
        let mut relay = self.relay.take().ok_or(ClientError::Closed)?;
        let event = match relay.next_event(wait).await {
            Ok(RelayEvent::Waiting) => AuthenticatedRelayEvent::Waiting,
            Ok(RelayEvent::Paired) => AuthenticatedRelayEvent::Paired,
            Ok(RelayEvent::PeerLeft) => return Ok(AuthenticatedRelayEvent::PeerLeft),
            Ok(RelayEvent::Signal { kind, data }) => {
                let frame = serde_json::from_str(&data).map_err(|_| ClientError::Protocol)?;
                let (inner_kind, data) = self
                    .codec
                    .accept(frame)
                    .map_err(|_| ClientError::Protocol)?;
                if inner_kind != kind_name(kind) || !wire::valid_signal(&data) {
                    return Err(ClientError::Protocol);
                }
                AuthenticatedRelayEvent::Signal { kind, data }
            }
            Err(ClientError::Timeout) => {
                self.relay = Some(relay);
                return Err(ClientError::Timeout);
            }
            Err(error) => return Err(error),
        };
        self.relay = Some(relay);
        Ok(event)
    }

    /// Consumes the connection even if sending the leave notice fails.
    pub async fn leave(mut self) -> Result<(), ClientError> {
        match self.relay.take() {
            Some(relay) => relay.leave().await,
            None => Ok(()),
        }
    }
}

fn kind_name(kind: SignalKind) -> &'static str {
    match kind {
        SignalKind::Offer => "offer",
        SignalKind::Answer => "answer",
        SignalKind::Ice => "ice",
        SignalKind::HostChallenge => "host_challenge",
        SignalKind::ViewerChallenge => "viewer_challenge",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_kind_matches_exact_wire_label_for_both_roles() {
        for kind in [
            SignalKind::Offer,
            SignalKind::Answer,
            SignalKind::Ice,
            SignalKind::HostChallenge,
            SignalKind::ViewerChallenge,
        ] {
            assert_eq!(
                Some(kind_name(kind)),
                serde_json::to_value(kind).unwrap().as_str()
            );
        }
        assert_ne!(
            kind_name(SignalKind::HostChallenge),
            kind_name(SignalKind::ViewerChallenge)
        );
    }
}

//! Loopback V2 transport and explicit private-LAN WSS with room-bound
//! authenticated opaque signaling.
//! Neither a valid relay session nor a verified payload authorizes capture,
//! input, or a network path. Validate SDP and ICE separately before use.

use crate::lan_wire::{Codec, Key};
use hub_signal_v2::{connect_private_lan_wss, wire, LoopbackRelay, PrivateLanRelay, RelayEvent};
#[cfg(feature = "v2-public-control")]
use hub_signal_v2::{
    connect_public_control_wss, PublicControlEndpoint, PublicControlRelay, PublicControlToken,
};
#[cfg(feature = "v2-public-video-wss")]
use hub_signal_v2::{connect_public_video_wss, PublicVideoRelay};
use std::{net::SocketAddr, sync::Arc, time::Duration};

#[cfg(feature = "v2-public-video-wss")]
pub(crate) use hub_signal_v2::PublicVideoEndpoint;

pub use hub_signal_v2::{
    ClientError, LoopbackTlsTrust, PrivateLanEndpoint, RelayToken, Role, RoomId, SignalKind,
};

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
    relay: Option<RelayConnection>,
    codec: Codec,
}

enum RelayConnection {
    Loopback(LoopbackRelay),
    PrivateLan(PrivateLanRelay),
    #[cfg(feature = "v2-public-video-wss")]
    PublicVideo(PublicVideoRelay),
    #[cfg(feature = "v2-public-control")]
    PublicControl(PublicControlRelay),
}

impl RelayConnection {
    async fn send_signal(&mut self, kind: SignalKind, data: String) -> Result<(), ClientError> {
        match self {
            Self::Loopback(relay) => relay.send_signal(kind, data).await,
            Self::PrivateLan(relay) => relay.send_signal(kind, data).await,
            #[cfg(feature = "v2-public-video-wss")]
            Self::PublicVideo(relay) => relay.send_signal(kind, data).await,
            #[cfg(feature = "v2-public-control")]
            Self::PublicControl(relay) => relay.send_signal(kind, data).await,
        }
    }

    async fn next_event(&mut self, wait: Duration) -> Result<RelayEvent, ClientError> {
        match self {
            Self::Loopback(relay) => relay.next_event(wait).await,
            Self::PrivateLan(relay) => relay.next_event(wait).await,
            #[cfg(feature = "v2-public-video-wss")]
            Self::PublicVideo(relay) => relay.next_event(wait).await,
            #[cfg(feature = "v2-public-control")]
            Self::PublicControl(relay) => relay.next_event(wait).await,
        }
    }

    async fn leave(self) -> Result<(), ClientError> {
        match self {
            Self::Loopback(relay) => relay.leave().await,
            Self::PrivateLan(relay) => relay.leave().await,
            #[cfg(feature = "v2-public-video-wss")]
            Self::PublicVideo(relay) => relay.leave().await,
            #[cfg(feature = "v2-public-control")]
            Self::PublicControl(relay) => relay.leave().await,
        }
    }
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

    /// Connect only by exact IPv4 source and destination with explicitly
    /// provisioned trust. This path never tries loopback, WS, DNS or redirects.
    pub async fn connect_private_lan_wss(
        endpoint: PrivateLanEndpoint,
        token: &RelayToken,
        room: RoomId,
        role: Role,
        key: Arc<Key>,
        nonce: String,
        trust: &LoopbackTlsTrust,
    ) -> Result<Self, ClientError> {
        let codec = Codec::new_v2(key, nonce, room.as_str(), role == Role::Host)
            .map_err(|_| ClientError::Protocol)?;
        let relay = connect_private_lan_wss(endpoint, token, room, role, trust).await?;
        Ok(Self {
            relay: Some(RelayConnection::PrivateLan(relay)),
            codec,
        })
    }

    #[cfg(feature = "v2-public-video-wss")]
    pub(crate) async fn connect_public_video_wss(
        endpoint: PublicVideoEndpoint,
        token: &RelayToken,
        room: RoomId,
        role: Role,
        key: Arc<Key>,
        nonce: String,
        trust: &LoopbackTlsTrust,
    ) -> Result<Self, ClientError> {
        let codec = Codec::new_v2(key, nonce, room.as_str(), role == Role::Host)
            .map_err(|_| ClientError::Protocol)?;
        let relay = connect_public_video_wss(endpoint, token, room, role, trust).await?;
        Ok(Self {
            relay: Some(RelayConnection::PublicVideo(relay)),
            codec,
        })
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) async fn connect_public_control_wss(
        endpoint: PublicControlEndpoint,
        token: &PublicControlToken,
        room: RoomId,
        role: Role,
        key: Arc<Key>,
        nonce: String,
        trust: &LoopbackTlsTrust,
    ) -> Result<Self, ClientError> {
        let codec = Codec::new_public_control(key, nonce, room.as_str(), role == Role::Host)
            .map_err(|_| ClientError::Protocol)?;
        let relay = connect_public_control_wss(endpoint, token, room, role, trust).await?;
        Ok(Self {
            relay: Some(RelayConnection::PublicControl(relay)),
            codec,
        })
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
            relay: Some(RelayConnection::Loopback(relay)),
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

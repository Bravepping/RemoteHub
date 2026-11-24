//! V2 experimental signaling only. Connecting to this relay never authorizes
//! a peer, screen sharing, input, or any media path.

mod client;
pub use client::{
    connect_private_lan_wss, connect_public_video_wss, ClientError, LoopbackRelay,
    LoopbackTlsTrust, PrivateLanEndpoint, PrivateLanRelay, PublicVideoEndpoint, PublicVideoRelay,
    RelayEvent, RelayToken, RoomId,
};

pub mod wire;
pub use wire::{Role, SignalKind};

#[cfg(feature = "synthetic-admission")]
pub mod synthetic;

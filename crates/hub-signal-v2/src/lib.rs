//! V2 experimental signaling only. Connecting to this relay never authorizes
//! a peer, screen sharing, input, or off-host network access.

mod client;
pub use client::{ClientError, LoopbackRelay, LoopbackTlsTrust, RelayEvent, RelayToken, RoomId};

pub mod wire;
pub use wire::{Role, SignalKind};

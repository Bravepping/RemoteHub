//! Same typed JSON on the native client and the Axum relay. Signal bodies
//! remain opaque: decoding this envelope is not verification of SDP or ICE.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const VERSION: u8 = 2;
pub const PATH: &str = "/v2/ws";
pub const MAX_WIRE: usize = 64 * 1024;
pub const MAX_SIGNAL: usize = 32 * 1024;
pub const ROOM_LIFETIME: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Host,
    Viewer,
}

impl Role {
    pub fn peer(self) -> Self {
        match self {
            Self::Host => Self::Viewer,
            Self::Viewer => Self::Host,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Offer,
    Answer,
    Ice,
}

impl SignalKind {
    pub fn allows_sender(self, role: Role) -> bool {
        matches!(
            (self, role),
            (Self::Offer, Role::Viewer) | (Self::Answer, Role::Host) | (Self::Ice, _)
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Hello {
        version: u8,
        room: String,
        role: Role,
    },
    Signal {
        seq: u64,
        signal: SignalKind,
        data: String,
    },
    Leave {},
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerFrame {
    Waiting {},
    Paired {},
    Signal {
        from: Role,
        seq: u64,
        signal: SignalKind,
        data: String,
    },
    PeerLeft {},
    Error {
        code: String,
    },
}

#[derive(Clone, Copy, Default)]
pub enum Flow {
    #[default]
    BeforeOffer,
    BeforeAnswer,
    Established,
}

impl Flow {
    pub fn after(self, signal: SignalKind) -> Option<Self> {
        match (self, signal) {
            (Self::BeforeOffer, SignalKind::Offer) => Some(Self::BeforeAnswer),
            (Self::BeforeAnswer, SignalKind::Answer) => Some(Self::Established),
            (Self::BeforeAnswer | Self::Established, SignalKind::Ice) => Some(self),
            _ => None,
        }
    }
}

pub fn valid_room(room: &str) -> bool {
    room.len() == 32
        && room
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn valid_signal(data: &str) -> bool {
    !data.is_empty() && data.len() <= MAX_SIGNAL && !data.contains('\0')
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("signaling frame could not be encoded or decoded")]
    Invalid,
    #[error("signaling frame exceeds the wire size limit")]
    Size,
}

pub fn encode(frame: &impl Serialize) -> Result<String, WireError> {
    let json = serde_json::to_string(frame).map_err(|_| WireError::Invalid)?;
    if json.len() > MAX_WIRE {
        return Err(WireError::Size);
    }
    Ok(json)
}

pub fn decode<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, WireError> {
    if json.len() > MAX_WIRE {
        return Err(WireError::Size);
    }
    serde_json::from_str(json).map_err(|_| WireError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_wire_keeps_existing_json_and_rejects_unknown_fields() {
        assert_eq!(
            encode(&ClientFrame::Leave {}).unwrap(),
            r#"{"type":"leave"}"#
        );
        assert!(matches!(
            decode::<ServerFrame>(r#"{"type":"paired"}"#),
            Ok(ServerFrame::Paired {})
        ));
        assert!(matches!(
            decode::<ClientFrame>(r#"{"type":"leave","extra":true}"#),
            Err(WireError::Invalid)
        ));
        assert!(matches!(
            decode::<ServerFrame>(r#"{"type":"paired","extra":true}"#),
            Err(WireError::Invalid)
        ));
        assert!(matches!(
            decode::<ServerFrame>(&" ".repeat(MAX_WIRE + 1)),
            Err(WireError::Size)
        ));
    }

    #[test]
    fn offer_answer_roles_and_order_are_shared() {
        assert!(SignalKind::Offer.allows_sender(Role::Viewer));
        assert!(!SignalKind::Offer.allows_sender(Role::Host));
        assert!(SignalKind::Answer.allows_sender(Role::Host));
        assert!(!SignalKind::Answer.allows_sender(Role::Viewer));
        let begin = Flow::default();
        assert!(begin.after(SignalKind::Ice).is_none());
        assert!(begin.after(SignalKind::Answer).is_none());
        let negotiating = begin.after(SignalKind::Offer).unwrap();
        assert!(negotiating.after(SignalKind::Offer).is_none());
        assert!(negotiating.after(SignalKind::Ice).is_some());
        let established = negotiating.after(SignalKind::Answer).unwrap();
        assert!(established.after(SignalKind::Answer).is_none());
        assert!(established.after(SignalKind::Ice).is_some());
    }
}

//! Same typed JSON on the native client and the Axum relay. Signal bodies
//! remain opaque: decoding this envelope is not verification of SDP or ICE.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const VERSION: u8 = 2;
pub const PATH: &str = "/v2/ws";
/// Reserved for the explicitly admitted public-video profile; not a fallback.
pub const PUBLIC_VIDEO_PATH: &str = "/v2/public-video/ws";
pub const MAX_WIRE: usize = 64 * 1024;
pub const MAX_SIGNAL: usize = 32 * 1024;
pub const MAX_CHALLENGE_DATA: usize = 1024;
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
    /// Host-issued challenge for the viewer's subsequent proof.
    HostChallenge,
    /// Viewer-issued challenge for the host's subsequent proof.
    ViewerChallenge,
    Offer,
    Answer,
    Ice,
}

impl SignalKind {
    pub fn allows_sender(self, role: Role) -> bool {
        matches!(
            (self, role),
            (Self::Offer | Self::ViewerChallenge, Role::Viewer)
                | (Self::Answer | Self::HostChallenge, Role::Host)
                | (Self::Ice, _)
        )
    }

    pub fn is_challenge(self) -> bool {
        matches!(self, Self::HostChallenge | Self::ViewerChallenge)
    }

    /// The end-to-end envelope is opaque here; only enforce small wire bounds.
    pub fn valid_payload(self, data: &str) -> bool {
        (!self.is_challenge() || data.len() <= MAX_CHALLENGE_DATA) && valid_signal(data)
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
    HostChallengeOnly,
    ViewerChallengeOnly,
    BothChallenges,
    BeforeAnswer,
    Established,
}

impl Flow {
    /// Public video always performs both device challenges and signs inline
    /// offer/answer. Legacy signaling may still omit challenges or trickle ICE.
    pub fn after_public_video(self, signal: SignalKind) -> Option<Self> {
        match (self, signal) {
            (Self::BeforeOffer, SignalKind::Offer) | (_, SignalKind::Ice) => None,
            _ => self.after(signal),
        }
    }

    pub fn after(self, signal: SignalKind) -> Option<Self> {
        match (self, signal) {
            (Self::BeforeOffer, SignalKind::HostChallenge) => Some(Self::HostChallengeOnly),
            (Self::BeforeOffer, SignalKind::ViewerChallenge) => Some(Self::ViewerChallengeOnly),
            (Self::HostChallengeOnly, SignalKind::ViewerChallenge)
            | (Self::ViewerChallengeOnly, SignalKind::HostChallenge) => Some(Self::BothChallenges),
            // Existing clients can still start with Offer. Once either peer
            // opts into challenge exchange, both challenges must precede it.
            (Self::BeforeOffer | Self::BothChallenges, SignalKind::Offer) => {
                Some(Self::BeforeAnswer)
            }
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
    fn public_video_requires_both_challenges_and_never_trickles() {
        let initial = Flow::default();
        assert!(initial.after(SignalKind::Offer).is_some());
        assert!(initial.after_public_video(SignalKind::Offer).is_none());
        for (first, second) in [
            (SignalKind::HostChallenge, SignalKind::ViewerChallenge),
            (SignalKind::ViewerChallenge, SignalKind::HostChallenge),
        ] {
            let one = initial.after_public_video(first).unwrap();
            assert!(one.after_public_video(SignalKind::Offer).is_none());
            assert!(one.after_public_video(first).is_none());
            let both = one.after_public_video(second).unwrap();
            let offer = both.after_public_video(SignalKind::Offer).unwrap();
            assert!(offer.after_public_video(SignalKind::Ice).is_none());
            let answer = offer.after_public_video(SignalKind::Answer).unwrap();
            assert!(answer.after_public_video(SignalKind::Ice).is_none());
            assert!(answer.after_public_video(SignalKind::Answer).is_none());
        }
    }

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
    fn existing_offer_answer_and_ice_frames_keep_identical_json() {
        for (kind, encoded) in [
            (SignalKind::Offer, "offer"),
            (SignalKind::Answer, "answer"),
            (SignalKind::Ice, "ice"),
        ] {
            assert_eq!(
                encode(&ClientFrame::Signal {
                    seq: 1,
                    signal: kind,
                    data: "fixture".into(),
                })
                .unwrap(),
                format!(r#"{{"type":"signal","seq":1,"signal":"{encoded}","data":"fixture"}}"#)
            );
        }
    }

    #[test]
    fn either_challenge_order_requires_both_and_never_repeats() {
        for (first, first_role, second, second_role) in [
            (
                SignalKind::HostChallenge,
                Role::Host,
                SignalKind::ViewerChallenge,
                Role::Viewer,
            ),
            (
                SignalKind::ViewerChallenge,
                Role::Viewer,
                SignalKind::HostChallenge,
                Role::Host,
            ),
        ] {
            assert!(first.allows_sender(first_role));
            assert!(!first.allows_sender(second_role));
            let first_only = Flow::default().after(first).unwrap();
            for denied in [
                first,
                SignalKind::Offer,
                SignalKind::Answer,
                SignalKind::Ice,
            ] {
                assert!(first_only.after(denied).is_none());
            }
            let both = first_only.after(second).unwrap();
            assert!(both.after(first).is_none());
            assert!(both.after(second).is_none());
            let before_answer = both.after(SignalKind::Offer).unwrap();
            assert!(before_answer.after(first).is_none());
            assert!(before_answer.after(second).is_none());
            assert!(before_answer.after(SignalKind::Answer).is_some());
        }
        assert!(SignalKind::HostChallenge.valid_payload(&"x".repeat(MAX_CHALLENGE_DATA)));
        assert!(!SignalKind::ViewerChallenge.valid_payload(&"x".repeat(MAX_CHALLENGE_DATA + 1)));
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

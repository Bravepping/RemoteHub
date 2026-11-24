//! Test-only WSS framing. The relay handles opaque binary messages; peers
//! independently check the expected phase/scope and authenticate the proof.
use hub_identity::session::{SessionChallenge, CHALLENGE_BYTES};
use uuid::Uuid;

const PROTOCOL: &[u8; 4] = b"RHF1";
const HEADER_BYTES: usize = PROTOCOL.len() + 2 + 16 + 32;
// Same exact SDP payload bound as the strict fake-input SDP policy.
const MAX_SDP_BYTES: usize = 4096;
const SIGNATURE_BYTES: usize = 64;
const SDP_OVERHEAD: usize = 2 + SIGNATURE_BYTES;
pub(crate) const MAX_FRAME_BYTES: usize = HEADER_BYTES + SDP_OVERHEAD + MAX_SDP_BYTES;
const REJECTED: &str = "PUBLIC_FAKE_INPUT_FIXTURE_WSS_FRAME";

#[derive(Clone, Copy)]
pub(crate) struct Context {
    pub room: Uuid,
    pub generation: [u8; 32],
}

impl Context {
    fn valid(self) -> bool {
        !self.room.is_nil() && self.generation != [0; 32]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    HostChallenge,
    ViewerChallenge,
    Offer,
    Answer,
}

impl Kind {
    fn tag(self) -> [u8; 2] {
        // Independent message discriminator and sender role (host 1, viewer 2).
        match self {
            Self::HostChallenge => [1, 1],
            Self::ViewerChallenge => [2, 2],
            Self::Offer => [3, 2],
            Self::Answer => [4, 1],
        }
    }

    fn is_challenge(self) -> bool {
        matches!(self, Self::HostChallenge | Self::ViewerChallenge)
    }
}

pub(crate) enum Frame<'a> {
    Challenge(SessionChallenge),
    SignedSdp {
        sdp: &'a str,
        signature: [u8; SIGNATURE_BYTES],
    },
}

pub(crate) fn encode(
    context: Context,
    kind: Kind,
    frame: Frame<'_>,
) -> Result<Vec<u8>, &'static str> {
    if !context.valid() {
        return Err(REJECTED);
    }
    let body_len = match &frame {
        Frame::Challenge(_) if kind.is_challenge() => CHALLENGE_BYTES,
        Frame::SignedSdp { sdp, .. } if !kind.is_challenge() && valid_sdp(sdp) => {
            SDP_OVERHEAD + sdp.len()
        }
        _ => return Err(REJECTED),
    };
    let mut bytes = Vec::with_capacity(HEADER_BYTES + body_len);
    bytes.extend_from_slice(PROTOCOL);
    bytes.extend_from_slice(&kind.tag());
    bytes.extend_from_slice(context.room.as_bytes());
    bytes.extend_from_slice(&context.generation);
    match frame {
        Frame::Challenge(challenge) => bytes.extend_from_slice(&challenge.to_bytes()),
        Frame::SignedSdp { sdp, signature } => {
            let size = u16::try_from(sdp.len()).map_err(|_| REJECTED)?;
            bytes.extend_from_slice(&size.to_be_bytes());
            bytes.extend_from_slice(&signature);
            bytes.extend_from_slice(sdp.as_bytes());
        }
    }
    Ok(bytes)
}

pub(crate) fn decode(
    context: Context,
    expected_kind: Kind,
    bytes: &[u8],
) -> Result<Frame<'_>, &'static str> {
    if !context.valid() || !(HEADER_BYTES..=MAX_FRAME_BYTES).contains(&bytes.len()) {
        return Err(REJECTED);
    }
    let (header, body) = bytes.split_at(HEADER_BYTES);
    if &header[..4] != PROTOCOL
        || header[4..6] != expected_kind.tag()
        || &header[6..22] != context.room.as_bytes()
        || header[22..] != context.generation
    {
        return Err(REJECTED);
    }
    if expected_kind.is_challenge() {
        let exact: [u8; CHALLENGE_BYTES] = body.try_into().map_err(|_| REJECTED)?;
        return SessionChallenge::from_bytes(exact)
            .map(Frame::Challenge)
            .map_err(|_| REJECTED);
    }
    if body.len() < SDP_OVERHEAD {
        return Err(REJECTED);
    }
    let expected_size = usize::from(u16::from_be_bytes([body[0], body[1]]));
    let sdp = std::str::from_utf8(&body[SDP_OVERHEAD..]).map_err(|_| REJECTED)?;
    if sdp.len() != expected_size || !valid_sdp(sdp) {
        return Err(REJECTED);
    }
    let signature = body[2..SDP_OVERHEAD].try_into().map_err(|_| REJECTED)?;
    Ok(Frame::SignedSdp { sdp, signature })
}

// Framing only. This is not permission to apply the SDP: the exact bytes
// must still verify under the one-use proof and strict native SCTP SDP policy.
fn valid_sdp(sdp: &str) -> bool {
    sdp.len() <= MAX_SDP_BYTES
        && sdp.starts_with("v=0\r\n")
        && sdp.strip_suffix("\r\n").is_some_and(|part| {
            part.split("\r\n")
                .all(|line| !line.is_empty() && line.bytes().all(|b| (b' '..=b'~').contains(&b)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDP: &str = "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n";
    const CTX: Context = Context {
        room: Uuid::from_bytes([3; 16]),
        generation: [7; 32],
    };
    const SIGNATURE: [u8; 64] = [0x9e; 64];

    fn challenge() -> SessionChallenge {
        let mut bytes = [0x81; CHALLENGE_BYTES];
        bytes[32..40].copy_from_slice(&500_u64.to_be_bytes());
        bytes[40..48].copy_from_slice(&510_u64.to_be_bytes());
        SessionChallenge::from_bytes(bytes).unwrap()
    }

    fn signed(sdp: &str) -> Frame<'_> {
        Frame::SignedSdp {
            sdp,
            signature: SIGNATURE,
        }
    }

    #[test]
    fn four_distinct_kinds_round_trip_exact_bytes() {
        for kind in [Kind::HostChallenge, Kind::ViewerChallenge] {
            let encoded = encode(CTX, kind, Frame::Challenge(challenge())).unwrap();
            let Frame::Challenge(received) = decode(CTX, kind, &encoded).unwrap() else {
                panic!("expected challenge")
            };
            assert_eq!(received.to_bytes(), challenge().to_bytes());
        }
        for kind in [Kind::Offer, Kind::Answer] {
            let encoded = encode(CTX, kind, signed(SDP)).unwrap();
            let Frame::SignedSdp { sdp, signature } = decode(CTX, kind, &encoded).unwrap() else {
                panic!("expected SDP")
            };
            assert_eq!(sdp.as_bytes(), SDP.as_bytes());
            assert_eq!(signature, SIGNATURE);
        }
        assert!(encode(CTX, Kind::HostChallenge, signed(SDP)).is_err());
        assert!(encode(CTX, Kind::Answer, Frame::Challenge(challenge())).is_err());
    }

    #[test]
    fn protocol_kind_role_room_and_generation_must_match() {
        for kind in [
            Kind::HostChallenge,
            Kind::ViewerChallenge,
            Kind::Offer,
            Kind::Answer,
        ] {
            let frame = if kind.is_challenge() {
                Frame::Challenge(challenge())
            } else {
                signed(SDP)
            };
            let bytes = encode(CTX, kind, frame).unwrap();
            for wrong in [
                Kind::HostChallenge,
                Kind::ViewerChallenge,
                Kind::Offer,
                Kind::Answer,
            ] {
                if wrong != kind {
                    assert!(decode(CTX, wrong, &bytes).is_err());
                }
            }
            for offset in [0, 3, 4, 5, 6, 21, 22, HEADER_BYTES - 1] {
                let mut wrong = bytes.clone();
                wrong[offset] ^= 0b10000000;
                assert!(decode(CTX, kind, &wrong).is_err());
            }
            for wrong in [
                Context {
                    room: Uuid::from_bytes([4; 16]),
                    ..CTX
                },
                Context {
                    generation: [0; 32],
                    ..CTX
                },
                Context {
                    generation: [8; 32],
                    ..CTX
                },
            ] {
                assert!(decode(wrong, kind, &bytes).is_err());
            }
        }
        assert!(encode(
            Context {
                room: Uuid::nil(),
                ..CTX
            },
            Kind::Offer,
            signed(SDP)
        )
        .is_err());
    }

    #[test]
    fn missing_extra_invalid_and_oversize_bytes_are_rejected() {
        for (kind, body) in [
            (Kind::HostChallenge, Frame::Challenge(challenge())),
            (Kind::Offer, signed(SDP)),
        ] {
            let mut bytes = encode(CTX, kind, body).unwrap();
            for size in 0..bytes.len() {
                assert!(decode(CTX, kind, &bytes[..size]).is_err());
            }
            bytes.push(b'X');
            assert!(decode(CTX, kind, &bytes).is_err());
        }
        let mut wrong = encode(CTX, Kind::Answer, signed(SDP)).unwrap();
        wrong[HEADER_BYTES + 1] ^= 1;
        assert!(decode(CTX, Kind::Answer, &wrong).is_err());
        let mut wrong = encode(CTX, Kind::Answer, signed(SDP)).unwrap();
        wrong[HEADER_BYTES + SDP_OVERHEAD + 6] = 0x80;
        assert!(decode(CTX, Kind::Answer, &wrong).is_err());
        let mut wrong = encode(CTX, Kind::HostChallenge, Frame::Challenge(challenge())).unwrap();
        wrong[HEADER_BYTES..].fill(0);
        assert!(decode(CTX, Kind::HostChallenge, &wrong).is_err());
        let full = format!("v=0\r\n{}\r\n", "x".repeat(MAX_SDP_BYTES - 7));
        let max = encode(CTX, Kind::Offer, signed(&full)).unwrap();
        assert_eq!(max.len(), MAX_FRAME_BYTES);
        assert!(decode(CTX, Kind::Offer, &max).is_ok());
        let mut oversized = max;
        oversized.push(b'x');
        assert!(decode(CTX, Kind::Offer, &oversized).is_err());
        for bad in [
            "".to_owned(),
            "press-approved".to_owned(),
            SDP.replace("\r\n", "\n"),
            SDP.replace("1 2", "1\t2"),
            SDP.replace("1 2", "1\0 2"),
            SDP.replace("1 2", "1 元 2"),
            full.replace("v=0", "v=0\r\ns=-"),
        ] {
            assert!(encode(CTX, Kind::Offer, signed(&bad)).is_err());
        }
    }
}

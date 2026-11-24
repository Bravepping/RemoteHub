//! No-I/O admission and framing for the separate synthetic-only WSS test.
//! A router must establish TLS, authenticate each one-use role, apply its
//! deadline, and close the pair on rejection. This module never authorizes
//! SDP installation, SCTP messages, media, or keyboard/mouse input.

use crate::Role;
use uuid::Uuid;

/// This literal is not a fallback for an ordinary or public-video route.
pub const PATH: &str = "/v2/synthetic-input-test/ws";

// RHF1 compatibility: hub-webrtc/src/public_fake_input_wss_wire.rs.
// Keep ordinary JSON signaling and cryptographic verification out of this layer.
const MAGIC: &[u8; 4] = b"RHF1";
const HEADER: usize = 4 + 1 + 1 + 16 + 32;
const CHALLENGE_BYTES: usize = 48;
const MAX_CHALLENGE_TTL_S: u64 = 120;
const SDP_OVERHEAD: usize = 2 + 64;
const MAX_SDP_BYTES: usize = 4_096;
const MEDIA: &str = "m=application 9 UDP/DTLS/SCTP webrtc-datachannel";
pub const MAX_FRAME_BYTES: usize = HEADER + SDP_OVERHEAD + MAX_SDP_BYTES;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    room: Uuid,
    generation: [u8; 32],
}

impl Scope {
    pub fn new(room: Uuid, generation: [u8; 32]) -> Result<Self, Error> {
        if room.is_nil() || generation == [0; 32] {
            return Err(Error::Admission);
        }
        Ok(Self { room, generation })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("SYNTHETIC_ADMISSION_REJECTED")]
    Admission,
    #[error("SYNTHETIC_FRAME_REJECTED")]
    Frame,
    #[error("SYNTHETIC_FLOW_REJECTED")]
    Phase,
    #[error("SYNTHETIC_FLOW_CLOSED")]
    Closed,
}

/// Compare to separately configured approval, not claims supplied by the
/// requester. Pass the exact request target (path and query if present).
pub fn admit(
    request_path: &str,
    approved: Scope,
    offered: Scope,
    approved_role: Role,
    offered_role: Role,
) -> Result<(), Error> {
    if request_path != PATH || approved != offered || approved_role != offered_role {
        return Err(Error::Admission);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    HostChallenge,
    ViewerChallenge,
    Offer,
    Answer,
}

impl Kind {
    fn is_challenge(self) -> bool {
        matches!(self, Self::HostChallenge | Self::ViewerChallenge)
    }
}

/// Inspect a bounded binary envelope without interpreting or returning its
/// signature or its SDP. `role` comes from separate transport authorization.
pub fn parse(scope: Scope, role: Role, bytes: &[u8]) -> Result<Kind, Error> {
    if !(HEADER..=MAX_FRAME_BYTES).contains(&bytes.len()) {
        return Err(Error::Frame);
    }
    let (header, body) = bytes.split_at(HEADER);
    if &header[..4] != MAGIC
        || &header[6..22] != scope.room.as_bytes()
        || header[22..] != scope.generation
    {
        return Err(Error::Frame);
    }
    let kind = match (header[4], header[5], role) {
        (1, 1, Role::Host) => Kind::HostChallenge,
        (2, 2, Role::Viewer) => Kind::ViewerChallenge,
        (3, 2, Role::Viewer) => Kind::Offer,
        (4, 1, Role::Host) => Kind::Answer,
        _ => return Err(Error::Frame),
    };
    let valid = if kind.is_challenge() {
        valid_challenge(body)
    } else {
        valid_sdp_body(body)
    };
    if !valid {
        return Err(Error::Frame);
    }
    Ok(kind)
}

fn valid_challenge(body: &[u8]) -> bool {
    let Ok(body) = <&[u8; CHALLENGE_BYTES]>::try_from(body) else {
        return false;
    };
    // The peers enforce wall/monotonic freshness; the relay checks only shape.
    let issued = u64::from_be_bytes(body[32..40].try_into().expect("eight-byte issue time"));
    let expires = u64::from_be_bytes(body[40..48].try_into().expect("eight-byte expiry"));
    body[..32] != [0; 32]
        && expires
            .checked_sub(issued)
            .is_some_and(|ttl| (1..=MAX_CHALLENGE_TTL_S).contains(&ttl))
}

fn valid_sdp_body(body: &[u8]) -> bool {
    let [high, low, ..] = body else {
        return false;
    };
    let Some(sdp_bytes) = body.get(SDP_OVERHEAD..) else {
        return false;
    };
    if usize::from(u16::from_be_bytes([*high, *low])) != sdp_bytes.len() {
        return false;
    }
    let Ok(sdp) = std::str::from_utf8(sdp_bytes) else {
        return false;
    };
    let Some(lines) = sdp
        .strip_prefix("v=0\r\n")
        .and_then(|sdp| sdp.strip_suffix("\r\n"))
    else {
        return false;
    };
    let mut media_seen = false;
    for line in lines.split("\r\n") {
        if !matches!(line.as_bytes(), [b'a'..=b'z', b'=', ..])
            || line.starts_with("v=")
            || !line.bytes().all(|b| (b' '..=b'~').contains(&b))
        {
            return false;
        }
        // Defense in depth against video/other-purpose envelopes. This is not
        // the endpoints' strict SDP/candidate policy or signature verification.
        if line.starts_with("m=") {
            if media_seen || line != MEDIA {
                return false;
            }
            media_seen = true;
        }
    }
    media_seen
}

enum Phase {
    Challenges,
    HostOnly,
    ViewerOnly,
    Offer,
    Answer,
    Complete,
}

/// Single admitted pair. A bad frame, explicit close, or frame after answer
/// makes the flow terminal. The caller forwards only successful frames.
pub struct Flow {
    scope: Scope,
    phase: Option<Phase>,
}

impl Flow {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            phase: Some(Phase::Challenges),
        }
    }

    pub fn accept(&mut self, role: Role, bytes: &[u8]) -> Result<Kind, Error> {
        let current = self.phase.take().ok_or(Error::Closed)?;
        let kind = parse(self.scope, role, bytes)?;
        self.phase = Some(match (current, kind) {
            (Phase::Challenges, Kind::HostChallenge) => Phase::HostOnly,
            (Phase::Challenges, Kind::ViewerChallenge) => Phase::ViewerOnly,
            (Phase::HostOnly, Kind::ViewerChallenge) | (Phase::ViewerOnly, Kind::HostChallenge) => {
                Phase::Offer
            }
            (Phase::Offer, Kind::Offer) => Phase::Answer,
            (Phase::Answer, Kind::Answer) => Phase::Complete,
            _ => return Err(Error::Phase),
        });
        Ok(kind)
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.phase, Some(Phase::Complete))
    }

    pub fn close(&mut self) {
        self.phase = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire;

    const SDP: &str = concat!(
        "v=0\r\n",
        "o=- 1 2 IN IP4 0.0.0.0\r\n",
        "s=-\r\n",
        "t=0 0\r\n",
        "m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n",
        "a=sctp-port:5000\r\n",
    );

    fn scope() -> Scope {
        Scope::new(Uuid::from_bytes([3; 16]), [7; 32]).unwrap()
    }

    fn header(kind: Kind) -> Vec<u8> {
        let tag = match kind {
            Kind::HostChallenge => [1, 1],
            Kind::ViewerChallenge => [2, 2],
            Kind::Offer => [3, 2],
            Kind::Answer => [4, 1],
        };
        [
            MAGIC.as_slice(),
            &tag,
            scope().room.as_bytes(),
            &scope().generation,
        ]
        .concat()
    }

    fn challenge(kind: Kind) -> Vec<u8> {
        let mut bytes = header(kind);
        bytes.extend_from_slice(&[0x81; 32]);
        bytes.extend_from_slice(&500_u64.to_be_bytes());
        bytes.extend_from_slice(&510_u64.to_be_bytes());
        bytes
    }

    fn signed(kind: Kind, sdp: &str) -> Vec<u8> {
        let mut bytes = header(kind);
        bytes.extend_from_slice(&(sdp.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&[0x9e; 64]);
        bytes.extend_from_slice(sdp.as_bytes());
        bytes
    }

    #[test]
    fn admission_requires_literal_target_approved_scope_and_role() {
        let good = scope();
        let offered = Scope::new(Uuid::from_bytes([4; 16]), good.generation).unwrap();
        let stale = Scope::new(good.room, [8; 32]).unwrap();
        assert!(Scope::new(Uuid::nil(), [1; 32]).is_err());
        assert!(Scope::new(good.room, [0; 32]).is_err());
        assert!(admit(PATH, good, good, Role::Host, Role::Host).is_ok());
        for target in [
            wire::PATH,
            wire::PUBLIC_VIDEO_PATH,
            "/v2/synthetic-input-test/ws/",
            "/v2/synthetic-input-test/ws?role=host",
            "/v2/synthetic-input-test/%77s",
        ] {
            assert_eq!(
                admit(target, good, good, Role::Host, Role::Host),
                Err(Error::Admission)
            );
        }
        assert!(admit(PATH, good, good, Role::Host, Role::Viewer).is_err());
        assert!(admit(PATH, good, offered, Role::Host, Role::Host).is_err());
        assert!(admit(PATH, good, stale, Role::Host, Role::Host).is_err());
    }

    #[test]
    fn accepts_one_pair_four_frames_and_either_challenge_order() {
        let host = challenge(Kind::HostChallenge);
        let viewer = challenge(Kind::ViewerChallenge);
        let offer = signed(Kind::Offer, SDP);
        let answer = signed(Kind::Answer, SDP);
        for challenges in [
            [(Role::Host, &host), (Role::Viewer, &viewer)],
            [(Role::Viewer, &viewer), (Role::Host, &host)],
        ] {
            let mut flow = Flow::new(scope());
            for (role, frame) in challenges {
                assert!(flow.accept(role, frame).is_ok());
            }
            assert_eq!(flow.accept(Role::Viewer, &offer), Ok(Kind::Offer));
            assert!(!flow.is_complete());
            assert_eq!(flow.accept(Role::Host, &answer), Ok(Kind::Answer));
            assert!(flow.is_complete());
            assert_eq!(flow.accept(Role::Host, &answer), Err(Error::Phase));
            assert_eq!(flow.accept(Role::Host, &host), Err(Error::Closed));
        }
    }

    #[test]
    fn wrong_phase_role_or_payload_is_terminal() {
        let host = challenge(Kind::HostChallenge);
        let offer = signed(Kind::Offer, SDP);
        let answer = signed(Kind::Answer, SDP);
        for (role, packet, error) in [
            (Role::Viewer, offer.as_slice(), Error::Phase),
            (Role::Host, answer.as_slice(), Error::Phase),
            (Role::Viewer, host.as_slice(), Error::Frame),
            (Role::Host, b"{}".as_slice(), Error::Frame),
        ] {
            let mut flow = Flow::new(scope());
            assert_eq!(flow.accept(role, packet), Err(error));
            assert_eq!(flow.accept(Role::Host, &host), Err(Error::Closed));
        }
        let mut flow = Flow::new(scope());
        flow.accept(Role::Host, &host).unwrap();
        assert_eq!(flow.accept(Role::Host, &host), Err(Error::Phase));
        let mut flow = Flow::new(scope());
        flow.close();
        assert_eq!(flow.accept(Role::Host, &host), Err(Error::Closed));
    }

    #[test]
    fn rhf1_frame_bounds_and_no_video_or_extra_media() {
        let challenge = challenge(Kind::HostChallenge);
        let offer = signed(Kind::Offer, SDP);
        for (role, packet) in [(Role::Host, &challenge), (Role::Viewer, &offer)] {
            assert!(parse(scope(), role, packet).is_ok());
            for n in 0..packet.len() {
                assert!(parse(scope(), role, &packet[..n]).is_err());
            }
            for offset in [0, 4, 5, 6, 21, 22, HEADER - 1] {
                let mut bad = packet.clone();
                bad[offset] ^= 128;
                assert!(parse(scope(), role, &bad).is_err());
            }
            let mut extra = packet.clone();
            extra.push(b'x');
            assert!(parse(scope(), role, &extra).is_err());
        }
        let video = "m=video 9 UDP/TLS/RTP/SAVPF 102";
        for invalid in [
            SDP.replace(MEDIA, video),
            format!("{SDP}{video}\r\n"),
            format!("{SDP}{MEDIA}\r\n"),
            SDP.replace("\r\n", "\n"),
            SDP.replace("1 2", "1 \0 2"),
            SDP.replace("1 2", "1 ß 2"),
        ] {
            assert!(parse(scope(), Role::Viewer, &signed(Kind::Offer, &invalid)).is_err());
        }
        let mut invalid_challenge = challenge;
        invalid_challenge[HEADER..HEADER + 32].fill(0);
        assert!(parse(scope(), Role::Host, &invalid_challenge).is_err());
        invalid_challenge[HEADER] = 1;
        invalid_challenge[HEADER + 40..].copy_from_slice(&621_u64.to_be_bytes());
        assert!(parse(scope(), Role::Host, &invalid_challenge).is_err());

        let start = format!("{SDP}a=");
        let max = format!("{start}{}\r\n", "x".repeat(MAX_SDP_BYTES - start.len() - 2));
        let packet = signed(Kind::Offer, &max);
        assert_eq!(packet.len(), MAX_FRAME_BYTES);
        assert!(parse(scope(), Role::Viewer, &packet).is_ok());
        let over = signed(Kind::Offer, &max.replace("a=x", "a=xx"));
        assert_eq!(over.len(), MAX_FRAME_BYTES + 1);
        assert!(parse(scope(), Role::Viewer, &over).is_err());
    }
}

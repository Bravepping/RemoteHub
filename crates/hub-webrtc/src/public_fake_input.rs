//! Offline protocol proof for an explicitly opted-in synthetic-marker test.
//! Cryptographic agreement is not an input grant or evidence of a native
//! approval dialog. Neither the proof nor the companion SDP policy opens a
//! socket or authorizes input.

use crate::{ice_policy::IceMode, public_video::PublicVideoScope};
use hub_identity::{
    pairing::VerifiedPeer,
    session::{PendingSessionProof, SessionBinding, SessionChallenge, SessionRole},
    Identity,
};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddrV4,
    time::{Duration, Instant},
};
use uuid::Uuid;

const PROOF_TTL: u64 = 15;
const DOMAIN: &[u8] = b"RemoteHub.V2.public-fake-input.synthetic-marker-only.v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeInputTestMode {
    Disabled,
    Enabled,
}

/// Independently selected native values. The relay must not choose them.
#[derive(Clone, Copy)]
pub struct FakeInputProofScope {
    pub room: Uuid,
    pub generation: [u8; 32],
    pub route: PublicVideoScope,
}

impl FakeInputProofScope {
    /// Parse a relay-only descriptor for the first synthetic-marker test.
    /// This is shape validation only: a future transport must verify a
    /// signature over the exact SDP and compare the live ICE pair to it.
    pub fn validate_relay_sctp_description(
        &self,
        sdp: &str,
        signer: SessionRole,
        now: u64,
    ) -> Result<SocketAddrV4, &'static str> {
        if self.route.mode != IceMode::RelayOnly {
            return Err("PUBLIC_FAKE_INPUT_RELAY_ONLY_REQUIRED");
        }
        self.route.checked_mode(now)?;
        crate::public_fake_input_sdp::validate(
            sdp,
            *self.route.turn.ip(),
            self.route.relay_ports.0,
            self.route.relay_ports.1,
            signer == SessionRole::Host,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    ModeRequired,
    Scope,
    Identity,
    Expired,
    Rejected,
}

impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::ModeRequired => "PUBLIC_FAKE_INPUT_MODE_REQUIRED",
            Self::Scope => "PUBLIC_FAKE_INPUT_SCOPE_INVALID",
            Self::Identity => "PUBLIC_FAKE_INPUT_IDENTITY_MISMATCH",
            Self::Expired => "PUBLIC_FAKE_INPUT_PROOF_EXPIRED",
            Self::Rejected => "PUBLIC_FAKE_INPUT_PROOF_REJECTED",
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for Error {}

enum Progress {
    ViewerSeal,
    ViewerVerify([u8; 32]),
    HostVerify,
    HostSeal([u8; 32]),
    Complete,
    Terminal,
}

#[derive(Clone, Copy)]
struct Clock {
    wall: u64,
    mono: Instant,
}

impl Clock {
    fn sample(wall: u64) -> Self {
        Self {
            wall,
            mono: Instant::now(),
        }
    }
}

pub struct FakeInputProof<'a> {
    local: &'a Identity,
    role: SessionRole,
    binding: SessionBinding,
    route_digest: [u8; 32],
    incoming: PendingSessionProof,
    progress: Progress,
    issued: Clock,
    expires_wall: u64,
    expires_mono: Instant,
}

impl<'a> FakeInputProof<'a> {
    pub fn prepare(
        local: &'a Identity,
        pinned_peer: &VerifiedPeer,
        role: SessionRole,
        scope: FakeInputProofScope,
        mode: FakeInputTestMode,
        now: u64,
    ) -> Result<Self, Error> {
        Self::prepare_at(local, pinned_peer, role, scope, mode, Clock::sample(now))
    }

    fn prepare_at(
        local: &'a Identity,
        pinned_peer: &VerifiedPeer,
        role: SessionRole,
        scope: FakeInputProofScope,
        mode: FakeInputTestMode,
        issued: Clock,
    ) -> Result<Self, Error> {
        if mode != FakeInputTestMode::Enabled {
            return Err(Error::ModeRequired);
        }
        if scope.room.is_nil() || scope.generation == [0; 32] {
            return Err(Error::Scope);
        }
        if issued.wall >= scope.route.expires_at {
            return Err(Error::Expired);
        }
        let route_mode = scope
            .route
            .checked_mode(issued.wall)
            .map_err(|_| Error::Scope)?;
        let binding = pinned_peer
            .session_binding(
                &local.public(),
                role,
                *scope.room.as_bytes(),
                scope.generation,
            )
            .map_err(|_| Error::Identity)?;
        let ttl_secs = PROOF_TTL.min(scope.route.expires_at - issued.wall);
        let ttl = Duration::from_secs(ttl_secs);
        let (expected, progress) = match role {
            SessionRole::Host => (SessionRole::Viewer, Progress::HostVerify),
            SessionRole::Viewer => (SessionRole::Host, Progress::ViewerSeal),
        };
        let incoming = PendingSessionProof::issue(&binding, expected, issued.wall, ttl)
            .map_err(|_| Error::Rejected)?;
        let expires_mono = issued.mono.checked_add(ttl).ok_or(Error::Expired)?;
        Ok(Self {
            local,
            role,
            binding,
            route_digest: route_digest(scope.route, route_mode),
            incoming,
            progress,
            issued,
            expires_wall: issued.wall + ttl_secs,
            expires_mono,
        })
    }

    pub fn challenge(&mut self, now: u64) -> Option<SessionChallenge> {
        self.expire(Clock::sample(now)).ok()?;
        matches!(
            self.progress,
            Progress::ViewerSeal | Progress::ViewerVerify(_) | Progress::HostVerify
        )
        .then(|| self.incoming.challenge())
    }

    /// Viewer signs first. Host signs only after verifying the viewer's
    /// proof, binding its response to that exact verified signature.
    pub fn seal(&mut self, peer_challenge: SessionChallenge, now: u64) -> Result<[u8; 64], Error> {
        self.seal_at(peer_challenge, Clock::sample(now))
    }

    fn seal_at(&mut self, challenge: SessionChallenge, now: Clock) -> Result<[u8; 64], Error> {
        let step = std::mem::replace(&mut self.progress, Progress::Terminal);
        self.fresh(now)?;
        let prior = match step {
            Progress::ViewerSeal => None,
            Progress::HostSeal(signature_hash) => Some(signature_hash),
            _ => return Err(Error::Rejected),
        };
        let bytes = self.statement(self.role, prior);
        let signature = self
            .local
            .sign_session_proof(&self.binding, self.role, challenge, &bytes, now.wall)
            .map_err(|_| Error::Rejected)?;
        self.progress = match self.role {
            SessionRole::Viewer => Progress::ViewerVerify(Sha256::digest(signature).into()),
            SessionRole::Host => Progress::Complete,
        };
        Ok(signature)
    }

    /// Accept one attempt; a bad signature must not leave a retry oracle.
    pub fn verify(&mut self, signature: &[u8; 64], now: u64) -> Result<(), Error> {
        self.verify_at(signature, Clock::sample(now))
    }

    fn verify_at(&mut self, signature: &[u8; 64], now: Clock) -> Result<(), Error> {
        let step = std::mem::replace(&mut self.progress, Progress::Terminal);
        self.fresh(now)?;
        let (signer, prior) = match step {
            Progress::HostVerify => (SessionRole::Viewer, None),
            Progress::ViewerVerify(signature_hash) => (SessionRole::Host, Some(signature_hash)),
            _ => return Err(Error::Rejected),
        };
        let bytes = self.statement(signer, prior);
        self.incoming
            .verify(signature, &bytes, now.wall)
            .map_err(|_| Error::Rejected)?;
        self.progress = match self.role {
            SessionRole::Host => Progress::HostSeal(Sha256::digest(signature).into()),
            SessionRole::Viewer => Progress::Complete,
        };
        Ok(())
    }

    /// Indicates the local proof exchange finished, not whether the remote
    /// side received the last response or that any input may be applied.
    pub fn is_complete(&mut self, now: u64) -> bool {
        self.expire(Clock::sample(now)).is_ok() && matches!(self.progress, Progress::Complete)
    }

    fn expire(&mut self, now: Clock) -> Result<(), Error> {
        self.fresh(now)
            .inspect_err(|_| self.progress = Progress::Terminal)
    }

    fn fresh(&self, now: Clock) -> Result<(), Error> {
        if !(self.issued.wall..self.expires_wall).contains(&now.wall)
            || !(self.issued.mono..self.expires_mono).contains(&now.mono)
        {
            return Err(Error::Expired);
        }
        Ok(())
    }

    fn statement(&self, signer: SessionRole, prior: Option<[u8; 32]>) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(DOMAIN);
        hash.update([signer as u8]);
        hash.update(self.route_digest);
        if let Some(viewer_signature) = prior {
            hash.update(viewer_signature);
        }
        hash.finalize().into()
    }
}

/// Fixed-width context; both endpoints sign every public route boundary.
/// SessionBinding separately incorporates the room, generation, both pinned
/// keys, role and the receiver's exact challenge.
fn route_digest(scope: PublicVideoScope, route_mode: u8) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update([route_mode]);
    hash.update(scope.wss.ip().octets());
    hash.update(scope.wss.port().to_be_bytes());
    hash.update(scope.ca_sha256);
    hash.update(scope.turn.ip().octets());
    hash.update(scope.turn.port().to_be_bytes());
    hash.update(scope.relay_ports.0.to_be_bytes());
    hash.update(scope.relay_ports.1.to_be_bytes());
    hash.update(scope.expires_at.to_be_bytes());
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ice_policy::IceMode, public_video::PublicVideoProof};
    use hub_identity::pairing::PairingCandidate;

    const NOW: u64 = 50_000;
    const SIGNAL: &str = "v=0\r\ns=test video-only offer\r\n";

    fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
        let peer = peer.public();
        local
            .verify_peer(
                &PairingCandidate::parse(&peer.pairing_code()).unwrap(),
                &peer.fingerprint,
            )
            .unwrap()
    }

    fn scope() -> FakeInputProofScope {
        FakeInputProofScope {
            room: Uuid::new_v4(),
            generation: [7; 32],
            route: PublicVideoScope {
                // Offline digest labels only.
                wss: "93.184.216.34:8443".parse().unwrap(),
                ca_sha256: [3; 32],
                turn: "93.184.216.34:3478".parse().unwrap(),
                relay_ports: (49152, 49200),
                mode: IceMode::RelayOnly,
                expires_at: NOW + 120,
            },
        }
    }

    fn proof<'a>(
        local: &'a Identity,
        peer: &Identity,
        role: SessionRole,
        scope: FakeInputProofScope,
    ) -> FakeInputProof<'a> {
        FakeInputProof::prepare(
            local,
            &pin(local, peer),
            role,
            scope,
            FakeInputTestMode::Enabled,
            NOW,
        )
        .unwrap()
    }

    #[test]
    fn descriptor_entry_refuses_direct_scope_or_expired_relay_scope() {
        let mut direct = scope();
        direct.route.mode = IceMode::DirectPreferred;
        assert_eq!(
            direct.validate_relay_sctp_description("", SessionRole::Viewer, NOW),
            Err("PUBLIC_FAKE_INPUT_RELAY_ONLY_REQUIRED")
        );
        let relay = scope();
        assert_eq!(
            relay.validate_relay_sctp_description("", SessionRole::Viewer, relay.route.expires_at,),
            Err("PUBLIC_VIDEO_SCOPE_EXPIRED")
        );
        assert_eq!(
            relay.validate_relay_sctp_description("", SessionRole::Viewer, NOW),
            Err("PUBLIC_FAKE_INPUT_SDP_POLICY")
        );
    }

    #[test]
    fn mutual_test_mode_is_one_use_and_separate_from_any_input_grant() {
        let scope = scope();
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        for (local, peer, role) in [
            (&host, &viewer, SessionRole::Host),
            (&viewer, &host, SessionRole::Viewer),
        ] {
            assert_eq!(
                FakeInputProof::prepare(
                    local,
                    &pin(local, peer),
                    role,
                    scope,
                    FakeInputTestMode::Disabled,
                    NOW
                )
                .err(),
                Some(Error::ModeRequired)
            );
        }

        let mut h = proof(&host, &viewer, SessionRole::Host, scope);
        let mut v = proof(&viewer, &host, SessionRole::Viewer, scope);
        let challenge = h.challenge(NOW).unwrap();
        let viewer_challenge = v.challenge(NOW).unwrap();
        assert!(!h.is_complete(NOW) && !v.is_complete(NOW));
        let viewer_signature = v.seal(challenge, NOW).unwrap();
        h.verify(&viewer_signature, NOW).unwrap();
        assert!(!h.is_complete(NOW) && !v.is_complete(NOW));
        let host_signature = h.seal(viewer_challenge, NOW).unwrap();
        v.verify(&host_signature, NOW).unwrap();
        assert!(h.is_complete(NOW) && v.is_complete(NOW));
        assert!(h.challenge(NOW).is_none() && v.challenge(NOW).is_none());
        assert_eq!(h.verify(&viewer_signature, NOW), Err(Error::Rejected));
        assert_eq!(v.seal(challenge, NOW), Err(Error::Rejected));
        assert!(!h.is_complete(NOW) && !v.is_complete(NOW));

        let mut replay_verifier = proof(&host, &viewer, SessionRole::Host, scope);
        assert_eq!(
            replay_verifier.verify(&viewer_signature, NOW),
            Err(Error::Rejected)
        );
        assert!(replay_verifier.challenge(NOW).is_none());
    }

    #[test]
    fn exact_room_generation_route_and_pinned_device_must_match() {
        let s = scope();
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let wrong = Identity::generate_ephemeral().unwrap();
        let cases = [
            (
                FakeInputProofScope {
                    room: Uuid::new_v4(),
                    ..s
                },
                &viewer,
            ),
            (
                FakeInputProofScope {
                    generation: [8; 32],
                    ..s
                },
                &viewer,
            ),
            (s, &wrong),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        mode: IceMode::DirectPreferred,
                        ..s.route
                    },
                    ..s
                },
                &viewer,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        wss: "93.184.216.34:8444".parse().unwrap(),
                        ..s.route
                    },
                    ..s
                },
                &viewer,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        ca_sha256: [6; 32],
                        ..s.route
                    },
                    ..s
                },
                &viewer,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        turn: "93.184.216.34:3479".parse().unwrap(),
                        ..s.route
                    },
                    ..s
                },
                &viewer,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        relay_ports: (49152, 49201),
                        ..s.route
                    },
                    ..s
                },
                &viewer,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        expires_at: NOW + 119,
                        ..s.route
                    },
                    ..s
                },
                &viewer,
            ),
        ];
        for (other_scope, signer) in cases {
            let mut h = proof(&host, &viewer, SessionRole::Host, s);
            let challenge = h.challenge(NOW).unwrap();
            let mut other = proof(signer, &host, SessionRole::Viewer, other_scope);
            let bad = other.seal(challenge, NOW).unwrap();
            assert_eq!(h.verify(&bad, NOW), Err(Error::Rejected));
            let mut correct = proof(&viewer, &host, SessionRole::Viewer, s);
            let good = correct.seal(challenge, NOW).unwrap();
            assert_eq!(h.verify(&good, NOW), Err(Error::Rejected));
        }
    }

    #[test]
    fn generic_or_video_only_signature_is_not_a_fake_test_proof() {
        let s = scope();
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();

        let mut h = proof(&host, &viewer, SessionRole::Host, s);
        let challenge = h.challenge(NOW).unwrap();
        let binding = pin(&viewer, &host)
            .session_binding(
                &viewer.public(),
                SessionRole::Viewer,
                *s.room.as_bytes(),
                s.generation,
            )
            .unwrap();
        let generic = viewer
            .sign_session_proof(
                &binding,
                SessionRole::Viewer,
                challenge,
                SIGNAL.as_bytes(),
                NOW,
            )
            .unwrap();
        assert_eq!(h.verify(&generic, NOW), Err(Error::Rejected));

        let mut h = proof(&host, &viewer, SessionRole::Host, s);
        let mut video = PublicVideoProof::prepare(
            &viewer,
            &pin(&viewer, &host),
            SessionRole::Viewer,
            s.room,
            s.generation,
            s.route,
            NOW,
        )
        .unwrap();
        let video_signature = video.seal(h.challenge(NOW).unwrap(), SIGNAL, NOW).unwrap();
        assert_eq!(h.verify(&video_signature, NOW), Err(Error::Rejected));

        let mut video_host = PublicVideoProof::prepare(
            &host,
            &pin(&host, &viewer),
            SessionRole::Host,
            s.room,
            s.generation,
            s.route,
            NOW,
        )
        .unwrap();
        let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
        let fake_signature = v.seal(video_host.challenge().unwrap(), NOW).unwrap();
        assert!(video_host.open(SIGNAL, &fake_signature, NOW).is_err());
    }

    #[test]
    fn host_response_cannot_be_substituted_between_two_viewer_transcripts() {
        let s = scope();
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let mut original_host = proof(&host, &viewer, SessionRole::Host, s);
        let mut original_viewer = proof(&viewer, &host, SessionRole::Viewer, s);
        let original_challenge = original_viewer.challenge(NOW).unwrap();
        original_viewer
            .seal(original_host.challenge(NOW).unwrap(), NOW)
            .unwrap();

        let mut another_host = proof(&host, &viewer, SessionRole::Host, s);
        let mut another_viewer = proof(&viewer, &host, SessionRole::Viewer, s);
        let another_signature = another_viewer
            .seal(another_host.challenge(NOW).unwrap(), NOW)
            .unwrap();
        another_host.verify(&another_signature, NOW).unwrap();
        let substituted = another_host.seal(original_challenge, NOW).unwrap();

        // Both signatures are from the pinned keys under the same room and
        // generation, but the host signed a different exact viewer proof.
        assert_eq!(
            original_viewer.verify(&substituted, NOW),
            Err(Error::Rejected)
        );
        assert!(!original_viewer.is_complete(NOW));
    }

    #[test]
    fn scoped_expiry_clock_rollback_and_wrong_role_order_close_attempt() {
        let mut s = scope();
        s.route.expires_at = NOW + 2;
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let hpin = pin(&host, &viewer);
        let issued = Clock::sample(NOW);
        let prepare_host = || {
            FakeInputProof::prepare_at(
                &host,
                &hpin,
                SessionRole::Host,
                s,
                FakeInputTestMode::Enabled,
                issued,
            )
            .unwrap()
        };

        for elapsed in [
            Clock {
                wall: NOW + 2,
                ..issued
            },
            Clock {
                wall: NOW - 1,
                ..issued
            },
            Clock {
                mono: issued.mono + Duration::from_secs(2),
                ..issued
            },
        ] {
            let mut h = prepare_host();
            let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
            let signature = v.seal(h.challenge(NOW).unwrap(), NOW).unwrap();
            assert_eq!(h.verify_at(&signature, elapsed), Err(Error::Expired));
            assert_eq!(h.verify(&signature, NOW), Err(Error::Rejected));
        }

        let mut observed_expiry = prepare_host();
        assert!(observed_expiry.challenge(NOW + 2).is_none());
        assert!(observed_expiry.challenge(NOW).is_none());

        let mut h = prepare_host();
        let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
        assert_eq!(h.seal(v.challenge(NOW).unwrap(), NOW), Err(Error::Rejected));
        assert!(h.challenge(NOW).is_none());
    }
}

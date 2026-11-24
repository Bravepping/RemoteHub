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
use rtc::peer_connection::transport::RTCIceCandidatePair;
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddrV4,
    time::{Duration, Instant},
};
use uuid::Uuid;

const PROOF_TTL: u64 = 15;
const DOMAIN: &[u8] = b"RemoteHub.V2.public-fake-input.synthetic-marker-only.v2\0";

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
    Description,
    Expired,
    Rejected,
}

impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::ModeRequired => "PUBLIC_FAKE_INPUT_MODE_REQUIRED",
            Self::Scope => "PUBLIC_FAKE_INPUT_SCOPE_INVALID",
            Self::Identity => "PUBLIC_FAKE_INPUT_IDENTITY_MISMATCH",
            Self::Description => "PUBLIC_FAKE_INPUT_SDP_POLICY",
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

/// Both ordered endpoint expectations authenticated by this local proof.
/// These are not observations of a connected path or permission to use it.
#[derive(Clone, Copy)]
pub struct ApprovedRelayPair {
    local: SocketAddrV4,
    remote: SocketAddrV4,
}

impl ApprovedRelayPair {
    pub fn local(&self) -> SocketAddrV4 {
        self.local
    }

    pub fn remote(&self) -> SocketAddrV4 {
        self.remote
    }
}

struct OfferProof {
    digest: [u8; 32],
    signature_digest: [u8; 32],
    relay: SocketAddrV4,
}

impl OfferProof {
    fn new(digest: [u8; 32], signature: &[u8; 64], relay: SocketAddrV4) -> Self {
        Self {
            digest,
            signature_digest: Sha256::digest(signature).into(),
            relay,
        }
    }
}

enum Progress {
    ViewerSeal,
    ViewerVerify(OfferProof),
    HostVerify,
    HostSeal(OfferProof),
    Complete(ApprovedRelayPair),
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
    scope: FakeInputProofScope,
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
            scope,
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

    /// Sign the exact policy-validated SDP bytes, with no normalization.
    /// Viewer signs its offer first. Host signs an answer only after
    /// verifying the offer, and binds both exact digests to that response.
    pub fn seal(
        &mut self,
        peer_challenge: SessionChallenge,
        exact_sdp: &str,
        now: u64,
    ) -> Result<[u8; 64], Error> {
        self.seal_at(peer_challenge, exact_sdp, Clock::sample(now))
    }

    fn seal_at(
        &mut self,
        challenge: SessionChallenge,
        exact_sdp: &str,
        now: Clock,
    ) -> Result<[u8; 64], Error> {
        let step = std::mem::replace(&mut self.progress, Progress::Terminal);
        self.fresh(now)?;
        let prior = match step {
            Progress::ViewerSeal => None,
            Progress::HostSeal(offer) => Some(offer),
            _ => return Err(Error::Rejected),
        };
        let local = self.checked_relay(exact_sdp, self.role, prior.as_ref(), now.wall)?;
        let digest = Sha256::digest(exact_sdp).into();
        let bytes = self.statement(self.role, &digest, prior.as_ref());
        let signature = self
            .local
            .sign_session_proof(&self.binding, self.role, challenge, &bytes, now.wall)
            .map_err(|_| Error::Rejected)?;
        self.progress = match prior {
            None => Progress::ViewerVerify(OfferProof::new(digest, &signature, local)),
            Some(offer) => Progress::Complete(ApprovedRelayPair {
                local,
                remote: offer.relay,
            }),
        };
        Ok(signature)
    }

    /// Validate policy and verify exactly one attempt, returning no partial
    /// route information. A failure permanently closes this proof.
    pub fn verify(&mut self, exact_sdp: &str, signature: &[u8; 64], now: u64) -> Result<(), Error> {
        self.verify_at(exact_sdp, signature, Clock::sample(now))
    }

    fn verify_at(
        &mut self,
        exact_sdp: &str,
        signature: &[u8; 64],
        now: Clock,
    ) -> Result<(), Error> {
        let step = std::mem::replace(&mut self.progress, Progress::Terminal);
        self.fresh(now)?;
        let (signer, prior) = match step {
            Progress::HostVerify => (SessionRole::Viewer, None),
            Progress::ViewerVerify(offer) => (SessionRole::Host, Some(offer)),
            _ => return Err(Error::Rejected),
        };
        let remote = self.checked_relay(exact_sdp, signer, prior.as_ref(), now.wall)?;
        let digest = Sha256::digest(exact_sdp).into();
        let bytes = self.statement(signer, &digest, prior.as_ref());
        self.incoming
            .verify(signature, &bytes, now.wall)
            .map_err(|_| Error::Rejected)?;
        self.progress = match prior {
            None => Progress::HostSeal(OfferProof::new(digest, signature, remote)),
            Some(offer) => Progress::Complete(ApprovedRelayPair {
                local: offer.relay,
                remote,
            }),
        };
        Ok(())
    }

    /// Only expose the ordered pair after the local role completes the
    /// exchange. Host completion does not imply the viewer got its answer.
    pub fn approved_relay_pair(&mut self, now: u64) -> Option<ApprovedRelayPair> {
        self.expire(Clock::sample(now)).ok()?;
        match self.progress {
            Progress::Complete(pair) => Some(pair),
            _ => None,
        }
    }

    pub fn is_complete(&mut self, now: u64) -> bool {
        self.approved_relay_pair(now).is_some()
    }

    /// Compare a caller-supplied native observation only against this
    /// fresh, completed proof. This function does not read RTC state.
    pub fn verify_selected_relay_pair(
        &mut self,
        observed: Option<&RTCIceCandidatePair>,
        now: u64,
    ) -> Result<(), &'static str> {
        self.expire(Clock::sample(now)).map_err(Error::code)?;
        let Progress::Complete(pair) = self.progress else {
            return Err(Error::Rejected.code());
        };
        crate::public_fake_input_pair::verify(pair.local, pair.remote, observed)
            .inspect_err(|_| self.progress = Progress::Terminal)
    }

    fn checked_relay(
        &self,
        exact_sdp: &str,
        signer: SessionRole,
        offer: Option<&OfferProof>,
        now: u64,
    ) -> Result<SocketAddrV4, Error> {
        let relay = self
            .scope
            .validate_relay_sctp_description(exact_sdp, signer, now)
            .map_err(|_| Error::Description)?;
        if offer.is_some_and(|offer| offer.relay == relay) {
            return Err(Error::Description);
        }
        Ok(relay)
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

    fn statement(
        &self,
        signer: SessionRole,
        exact_sdp_digest: &[u8; 32],
        offer: Option<&OfferProof>,
    ) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(DOMAIN);
        hash.update([signer as u8]);
        hash.update(self.route_digest);
        if let Some(offer) = offer {
            hash.update(offer.digest);
            hash.update(offer.signature_digest);
        }
        hash.update(exact_sdp_digest);
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

    fn descriptor(setup: &str, port: u16) -> String {
        format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=fingerprint:sha-256 {}\r\na=group:BUNDLE 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\na=setup:{setup}\r\na=mid:0\r\na=sctp-port:5000\r\na=ice-ufrag:ABCD\r\na=ice-pwd:abcdefghijklmnopqrstuv\r\na=candidate:r 1 udp 16777215 93.184.216.34 {port} typ relay raddr 0.0.0.0 rport 0\r\na=end-of-candidates\r\n",
            ["AB"; 32].join(":")
        )
    }

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
        let offer = descriptor("actpass", 49160);
        let answer = descriptor("active", 49161);
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
        let viewer_signature = v.seal(challenge, &offer, NOW).unwrap();
        h.verify(&offer, &viewer_signature, NOW).unwrap();
        assert!(h.approved_relay_pair(NOW).is_none());
        assert!(v.approved_relay_pair(NOW).is_none());
        let host_signature = h.seal(viewer_challenge, &answer, NOW).unwrap();
        v.verify(&answer, &host_signature, NOW).unwrap();
        let host_pair = h.approved_relay_pair(NOW).unwrap();
        let viewer_pair = v.approved_relay_pair(NOW).unwrap();
        assert_eq!(
            host_pair.local(),
            "93.184.216.34:49161".parse::<SocketAddrV4>().unwrap()
        );
        assert_eq!(
            host_pair.remote(),
            "93.184.216.34:49160".parse::<SocketAddrV4>().unwrap()
        );
        assert_eq!(viewer_pair.local(), host_pair.remote());
        assert_eq!(viewer_pair.remote(), host_pair.local());
        assert!(h.approved_relay_pair(NOW + PROOF_TTL).is_none());
        assert!(h.approved_relay_pair(NOW).is_none());
        assert!(h.challenge(NOW).is_none() && v.challenge(NOW).is_none());
        assert_eq!(
            h.verify(&offer, &viewer_signature, NOW),
            Err(Error::Rejected)
        );
        assert_eq!(v.seal(challenge, &offer, NOW), Err(Error::Rejected));
        assert!(!h.is_complete(NOW) && !v.is_complete(NOW));

        let mut replay_verifier = proof(&host, &viewer, SessionRole::Host, scope);
        assert_eq!(
            replay_verifier.verify(&offer, &viewer_signature, NOW),
            Err(Error::Rejected)
        );
        assert!(replay_verifier.challenge(NOW).is_none());
    }

    #[test]
    fn exact_room_generation_route_and_pinned_device_must_match() {
        let offer = descriptor("actpass", 49160);
        let s = scope();
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let wrong = Identity::generate_ephemeral().unwrap();
        let mut direct = s;
        direct.route.mode = IceMode::DirectPreferred;
        let mut h = proof(&host, &viewer, SessionRole::Host, s);
        let mut other = proof(&viewer, &host, SessionRole::Viewer, direct);
        assert_eq!(
            other.seal(h.challenge(NOW).unwrap(), &offer, NOW),
            Err(Error::Description)
        );
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
            let bad = other.seal(challenge, &offer, NOW).unwrap();
            assert_eq!(h.verify(&offer, &bad, NOW), Err(Error::Rejected));
            let mut correct = proof(&viewer, &host, SessionRole::Viewer, s);
            let good = correct.seal(challenge, &offer, NOW).unwrap();
            assert_eq!(h.verify(&offer, &good, NOW), Err(Error::Rejected));
        }
    }

    #[test]
    fn altered_exact_descriptions_or_matching_allocations_never_release_a_pair() {
        let s = scope();
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let offer = descriptor("actpass", 49160);
        let answer = descriptor("active", 49161);
        for changed_offer in [
            offer.replace("ABCD", "WXYZ"),
            offer.replace("49160", "49162"),
        ] {
            assert!(s
                .validate_relay_sctp_description(&changed_offer, SessionRole::Viewer, NOW)
                .is_ok());
            let mut h = proof(&host, &viewer, SessionRole::Host, s);
            let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
            let offer_sig = v.seal(h.challenge(NOW).unwrap(), &offer, NOW).unwrap();
            assert_eq!(
                h.verify(&changed_offer, &offer_sig, NOW),
                Err(Error::Rejected)
            );
            assert_eq!(h.verify(&offer, &offer_sig, NOW), Err(Error::Rejected));
            assert!(h.approved_relay_pair(NOW).is_none());
        }
        for changed_answer in [
            answer.replace("ABCD", "WXYZ"),
            answer.replace("49161", "49162"),
        ] {
            assert!(s
                .validate_relay_sctp_description(&changed_answer, SessionRole::Host, NOW)
                .is_ok());
            let mut h = proof(&host, &viewer, SessionRole::Host, s);
            let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
            let offer_sig = v.seal(h.challenge(NOW).unwrap(), &offer, NOW).unwrap();
            h.verify(&offer, &offer_sig, NOW).unwrap();
            let answer_sig = h.seal(v.challenge(NOW).unwrap(), &answer, NOW).unwrap();
            assert_eq!(
                v.verify(&changed_answer, &answer_sig, NOW),
                Err(Error::Rejected)
            );
            assert_eq!(v.verify(&answer, &answer_sig, NOW), Err(Error::Rejected));
            assert!(v.approved_relay_pair(NOW).is_none());
        }

        let mut h = proof(&host, &viewer, SessionRole::Host, s);
        let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
        let host_challenge = h.challenge(NOW).unwrap();
        assert_eq!(
            v.seal(host_challenge, &offer.replace("typ relay", "typ host"), NOW),
            Err(Error::Description)
        );
        assert_eq!(v.seal(host_challenge, &offer, NOW), Err(Error::Rejected));

        let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
        let offer_sig = v.seal(host_challenge, &offer, NOW).unwrap();
        h.verify(&offer, &offer_sig, NOW).unwrap();
        let viewer_challenge = v.challenge(NOW).unwrap();
        assert_eq!(
            h.seal(viewer_challenge, &descriptor("active", 49160), NOW),
            Err(Error::Description)
        );
        assert!(h.approved_relay_pair(NOW).is_none());
    }

    #[test]
    fn selected_pair_requires_fresh_completed_proof_and_exact_order() {
        use rtc::peer_connection::transport::{
            RTCIceCandidate, RTCIceCandidateType, RTCIceProtocol,
        };

        fn candidate(port: u16) -> RTCIceCandidate {
            RTCIceCandidate {
                address: "93.184.216.34".into(),
                port,
                typ: RTCIceCandidateType::Relay,
                protocol: RTCIceProtocol::Udp,
                component: 1,
                ..Default::default()
            }
        }
        let s = scope();
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let offer = descriptor("actpass", 49160);
        let answer = descriptor("active", 49161);
        let mut h = proof(&host, &viewer, SessionRole::Host, s);
        let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
        let host_order = RTCIceCandidatePair::new(candidate(49161), candidate(49160));
        let viewer_order = RTCIceCandidatePair::new(candidate(49160), candidate(49161));
        assert_eq!(
            h.verify_selected_relay_pair(Some(&host_order), NOW),
            Err(Error::Rejected.code())
        );
        let offer_sig = v.seal(h.challenge(NOW).unwrap(), &offer, NOW).unwrap();
        h.verify(&offer, &offer_sig, NOW).unwrap();
        let answer_sig = h.seal(v.challenge(NOW).unwrap(), &answer, NOW).unwrap();
        v.verify(&answer, &answer_sig, NOW).unwrap();

        assert_eq!(h.verify_selected_relay_pair(Some(&host_order), NOW), Ok(()));
        assert_eq!(
            v.verify_selected_relay_pair(Some(&viewer_order), NOW),
            Ok(())
        );
        assert_eq!(
            v.verify_selected_relay_pair(Some(&host_order), NOW),
            Err("PUBLIC_FAKE_INPUT_SELECTED_PAIR_REJECTED")
        );
        assert!(v.approved_relay_pair(NOW).is_none());
        assert_eq!(
            h.verify_selected_relay_pair(Some(&host_order), NOW + PROOF_TTL),
            Err(Error::Expired.code())
        );
        assert!(h.approved_relay_pair(NOW).is_none());
    }

    #[test]
    fn generic_or_video_only_signature_is_not_a_fake_test_proof() {
        let offer = descriptor("actpass", 49160);
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
        assert_eq!(h.verify(&offer, &generic, NOW), Err(Error::Rejected));

        let mut h = proof(&host, &viewer, SessionRole::Host, s);
        let mut legacy_digest = Sha256::new();
        legacy_digest.update(b"RemoteHub.V2.public-fake-input.synthetic-marker-only.v1\0");
        legacy_digest.update([SessionRole::Viewer as u8]);
        legacy_digest.update(route_digest(s.route, s.route.checked_mode(NOW).unwrap()));
        let legacy = viewer
            .sign_session_proof(
                &binding,
                SessionRole::Viewer,
                h.challenge(NOW).unwrap(),
                &legacy_digest.finalize(),
                NOW,
            )
            .unwrap();
        assert_eq!(h.verify(&offer, &legacy, NOW), Err(Error::Rejected));

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
        assert_eq!(
            h.verify(&offer, &video_signature, NOW),
            Err(Error::Rejected)
        );

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
        let fake_signature = v
            .seal(video_host.challenge().unwrap(), &offer, NOW)
            .unwrap();
        assert!(video_host.open(SIGNAL, &fake_signature, NOW).is_err());
    }

    #[test]
    fn host_response_cannot_be_substituted_between_two_viewer_transcripts() {
        let offer = descriptor("actpass", 49160);
        let answer = descriptor("active", 49161);
        let s = scope();
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let mut original_host = proof(&host, &viewer, SessionRole::Host, s);
        let mut original_viewer = proof(&viewer, &host, SessionRole::Viewer, s);
        let original_challenge = original_viewer.challenge(NOW).unwrap();
        original_viewer
            .seal(original_host.challenge(NOW).unwrap(), &offer, NOW)
            .unwrap();

        let mut another_host = proof(&host, &viewer, SessionRole::Host, s);
        let mut another_viewer = proof(&viewer, &host, SessionRole::Viewer, s);
        let another_signature = another_viewer
            .seal(another_host.challenge(NOW).unwrap(), &offer, NOW)
            .unwrap();
        another_host
            .verify(&offer, &another_signature, NOW)
            .unwrap();
        let substituted = another_host.seal(original_challenge, &answer, NOW).unwrap();

        // Both signatures are from the pinned keys under the same room and
        // generation, but the host signed a different exact viewer proof.
        assert_eq!(
            original_viewer.verify(&answer, &substituted, NOW),
            Err(Error::Rejected)
        );
        assert!(!original_viewer.is_complete(NOW));
    }

    #[test]
    fn scoped_expiry_clock_rollback_and_wrong_role_order_close_attempt() {
        let offer = descriptor("actpass", 49160);
        let answer = descriptor("active", 49161);
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
            let signature = v.seal(h.challenge(NOW).unwrap(), &offer, NOW).unwrap();
            assert_eq!(
                h.verify_at(&offer, &signature, elapsed),
                Err(Error::Expired)
            );
            assert_eq!(h.verify(&offer, &signature, NOW), Err(Error::Rejected));
        }

        let mut observed_expiry = prepare_host();
        assert!(observed_expiry.challenge(NOW + 2).is_none());
        assert!(observed_expiry.challenge(NOW).is_none());

        let mut h = prepare_host();
        let mut v = proof(&viewer, &host, SessionRole::Viewer, s);
        assert_eq!(
            h.seal(v.challenge(NOW).unwrap(), &answer, NOW),
            Err(Error::Rejected)
        );
        assert!(h.challenge(NOW).is_none());
    }
}

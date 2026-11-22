//! Bounded, domain-separated device proofs. Callers must independently know
//! the expected full keys before creating a binding; never learn trust from a
//! relay-supplied proof. Successful verification covers only the exact bytes
//! passed to it. Both roles require separate challenges for mutual identity,
//! and neither proof grants viewing or input permission.

use crate::Identity;
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    time::{Duration, Instant},
};

const DOMAIN: &[u8] = b"RemoteHub.identity.v2-session-proof.ed25519.v1\0";
const COUNT_BYTES: usize = 8;
pub const CHALLENGE_BYTES: usize = 32 + 2 * COUNT_BYTES;
pub const MAX_SIGNAL_BYTES: usize = 32 * 1024;
pub const MAX_TTL: Duration = Duration::from_secs(120);
const CLOCK_SKEW_SECONDS: u64 = 5;
const MESSAGE_BYTES: usize = DOMAIN.len() + 16 + 32 + 1 + 2 * 32 + CHALLENGE_BYTES + 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SessionRole {
    Host = 1,
    Viewer = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionProofError {
    Binding,
    Clock,
    Challenge,
    Expired,
    Payload,
    Random,
    Replay,
    Signature,
}

impl fmt::Display for SessionProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Binding => "SESSION_PROOF_BINDING",
            Self::Clock => "SESSION_PROOF_CLOCK",
            Self::Challenge => "SESSION_PROOF_CHALLENGE",
            Self::Expired => "SESSION_PROOF_EXPIRED",
            Self::Payload => "SESSION_PROOF_PAYLOAD",
            Self::Random => "SESSION_PROOF_RANDOM",
            Self::Replay => "SESSION_PROOF_REPLAY",
            Self::Signature => "SESSION_PROOF_SIGNATURE",
        })
    }
}
impl std::error::Error for SessionProofError {}

/// The locally accepted room, session nonce and exact peer keys; verifier and
/// signer construct this independently from already trusted native state.
#[derive(Clone)]
pub struct SessionBinding {
    room: [u8; 16],
    nonce: [u8; 32],
    host: VerifyingKey,
    viewer: VerifyingKey,
}

impl SessionBinding {
    pub fn new(
        room: [u8; 16],
        nonce: [u8; 32],
        host_public_key: [u8; 32],
        viewer_public_key: [u8; 32],
    ) -> Result<Self, SessionProofError> {
        if room == [0; 16] || nonce == [0; 32] || host_public_key == viewer_public_key {
            return Err(SessionProofError::Binding);
        }
        let host = checked_key(&host_public_key)?;
        let viewer = checked_key(&viewer_public_key)?;
        Ok(Self {
            room,
            nonce,
            host,
            viewer,
        })
    }

    fn key_for(&self, role: SessionRole) -> &VerifyingKey {
        match role {
            SessionRole::Host => &self.host,
            SessionRole::Viewer => &self.viewer,
        }
    }

    fn message(
        &self,
        signer: SessionRole,
        challenge: SessionChallenge,
        signal: &[u8],
    ) -> Result<[u8; MESSAGE_BYTES], SessionProofError> {
        if signal.is_empty() || signal.len() > MAX_SIGNAL_BYTES {
            return Err(SessionProofError::Payload);
        }
        let challenge = challenge.to_bytes();
        let digest = Sha256::digest(signal);
        let fields: [&[u8]; 8] = [
            DOMAIN,
            &self.room,
            &self.nonce,
            &[signer as u8],
            self.host.as_bytes(),
            self.viewer.as_bytes(),
            &challenge,
            digest.as_ref(),
        ];
        let mut bytes = [0; MESSAGE_BYTES];
        let mut offsets = &mut bytes[..];
        for field in fields {
            let (dst, rest) = offsets.split_at_mut(field.len());
            dst.copy_from_slice(field);
            offsets = rest;
        }
        Ok(bytes)
    }
}

pub(crate) fn checked_key(bytes: &[u8; 32]) -> Result<VerifyingKey, SessionProofError> {
    let key = VerifyingKey::from_bytes(bytes).map_err(|_| SessionProofError::Binding)?;
    let point = key.to_edwards();
    if key.is_weak() || !point.is_torsion_free() || point.compress().to_bytes() != *bytes {
        return Err(SessionProofError::Binding);
    }
    Ok(key)
}

/// The public challenge sent to the other peer. Its fixed representation is
/// random bytes followed by issue and expiry UTC Unix seconds (big-endian).
/// Receiving this value alone does not recreate a verifier's pending state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SessionChallenge {
    random: [u8; 32],
    issued_at: u64,
    expires_at: u64,
}

impl SessionChallenge {
    pub fn from_bytes(bytes: [u8; CHALLENGE_BYTES]) -> Result<Self, SessionProofError> {
        let mut random = [0; 32];
        random.copy_from_slice(&bytes[..32]);
        let mut issued_at = [0; 8];
        issued_at.copy_from_slice(&bytes[32..40]);
        let mut expires_at = [0; 8];
        expires_at.copy_from_slice(&bytes[40..48]);
        let challenge = Self {
            random,
            issued_at: u64::from_be_bytes(issued_at),
            expires_at: u64::from_be_bytes(expires_at),
        };
        if !challenge.valid_duration() {
            return Err(SessionProofError::Challenge);
        }
        Ok(challenge)
    }

    pub fn to_bytes(self) -> [u8; CHALLENGE_BYTES] {
        let mut bytes = [0; CHALLENGE_BYTES];
        bytes[..32].copy_from_slice(&self.random);
        bytes[32..40].copy_from_slice(&self.issued_at.to_be_bytes());
        bytes[40..48].copy_from_slice(&self.expires_at.to_be_bytes());
        bytes
    }

    fn valid_duration(self) -> bool {
        self.random != [0; 32]
            && self
                .expires_at
                .checked_sub(self.issued_at)
                .is_some_and(|ttl| (1..=MAX_TTL.as_secs()).contains(&ttl))
    }

    fn check_sign_time(self, now: u64) -> Result<(), SessionProofError> {
        if !self.valid_duration() {
            return Err(SessionProofError::Challenge);
        }
        if now >= self.expires_at {
            return Err(SessionProofError::Expired);
        }
        if self.issued_at > now.saturating_add(CLOCK_SKEW_SECONDS) {
            return Err(SessionProofError::Clock);
        }
        Ok(())
    }
}

impl Identity {
    /// Fixed proof for the nominated role and one exact signal, not a generic
    /// signing primitive. The caller has already checked the peer trust.
    pub fn sign_session_proof(
        &self,
        binding: &SessionBinding,
        local_role: SessionRole,
        challenge: SessionChallenge,
        signal: &[u8],
        now: u64,
    ) -> Result<[u8; 64], SessionProofError> {
        if &self.key.verifying_key() != binding.key_for(local_role) {
            return Err(SessionProofError::Binding);
        }
        challenge.check_sign_time(now)?;
        let message = binding.message(local_role, challenge, signal)?;
        Ok(self.key.sign(&message).to_bytes())
    }
}

/// Receiver-owned proof slot. A fresh instance is required for every
/// expected proof. Valid signatures consume it atomically through &mut self.
pub struct PendingSessionProof {
    binding: SessionBinding,
    expected_signer: SessionRole,
    challenge: SessionChallenge,
    deadline: Instant,
    consumed: bool,
}

impl PendingSessionProof {
    /// Issue a local one-time proof challenge. `now` is UTC Unix seconds and
    /// `ttl` is a whole number of seconds; an independent monotonic deadline
    /// prevents wall-clock rollback from extending the slot.
    pub fn issue(
        binding: &SessionBinding,
        expected_signer: SessionRole,
        now: u64,
        ttl: Duration,
    ) -> Result<Self, SessionProofError> {
        if ttl.is_zero() || ttl > MAX_TTL || ttl.subsec_nanos() != 0 {
            return Err(SessionProofError::Challenge);
        }
        let expires_at = now
            .checked_add(ttl.as_secs())
            .ok_or(SessionProofError::Clock)?;
        let deadline = Instant::now()
            .checked_add(ttl)
            .ok_or(SessionProofError::Clock)?;
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| SessionProofError::Random)?;
        if random == [0; 32] {
            return Err(SessionProofError::Random);
        }
        Ok(Self {
            binding: binding.clone(),
            expected_signer,
            challenge: SessionChallenge {
                random,
                issued_at: now,
                expires_at,
            },
            deadline,
            consumed: false,
        })
    }

    pub fn challenge(&self) -> SessionChallenge {
        self.challenge
    }

    /// The caller may use only the signal bytes authenticated by this call.
    /// Invalid attempts do not consume the pending challenge; the caller is
    /// responsible for limiting failed wire attempts and for consent/ACLs.
    pub fn verify(
        &mut self,
        signature: &[u8; 64],
        signal: &[u8],
        now: u64,
    ) -> Result<(), SessionProofError> {
        if self.consumed {
            return Err(SessionProofError::Replay);
        }
        if Instant::now() >= self.deadline || now >= self.challenge.expires_at {
            return Err(SessionProofError::Expired);
        }
        if now < self.challenge.issued_at {
            return Err(SessionProofError::Clock);
        }
        let message = self
            .binding
            .message(self.expected_signer, self.challenge, signal)?;
        self.binding
            .key_for(self.expected_signer)
            .verify_strict(&message, &Signature::from_bytes(signature))
            .map_err(|_| SessionProofError::Signature)?;
        self.consumed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const ROOM: [u8; 16] = [7; 16];
    const NONCE: [u8; 32] = [9; 32];
    const TTL: Duration = Duration::from_secs(30);
    const OFFER: &[u8] = b"test viewer offer\r\n";
    const ANSWER: &[u8] = b"test host answer\r\n";

    fn keys() -> (Identity, Identity, SessionBinding) {
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let binding = SessionBinding::new(
            ROOM,
            NONCE,
            host.public().public_key,
            viewer.public().public_key,
        )
        .unwrap();
        (host, viewer, binding)
    }

    #[test]
    fn mutual_device_proofs_cover_exact_bytes_and_consume_distinct_challenges() {
        let (host, viewer, binding) = keys();
        let mut host_proof =
            PendingSessionProof::issue(&binding, SessionRole::Host, NOW, TTL).unwrap();
        let mut viewer_proof =
            PendingSessionProof::issue(&binding, SessionRole::Viewer, NOW, TTL).unwrap();
        let host_challenge = host_proof.challenge().to_bytes();
        let viewer_challenge = viewer_proof.challenge().to_bytes();
        assert_ne!(host_challenge, viewer_challenge);

        let offer_proof = viewer
            .sign_session_proof(
                &binding,
                SessionRole::Viewer,
                SessionChallenge::from_bytes(viewer_challenge).unwrap(),
                OFFER,
                NOW,
            )
            .unwrap();
        let answer_proof = host
            .sign_session_proof(
                &binding,
                SessionRole::Host,
                SessionChallenge::from_bytes(host_challenge).unwrap(),
                ANSWER,
                NOW,
            )
            .unwrap();

        assert_eq!(
            host_proof.verify(&offer_proof, OFFER, NOW + 1),
            Err(SessionProofError::Signature)
        );
        assert_eq!(
            viewer_proof.verify(&offer_proof, b"test viewer offer\n", NOW + 1),
            Err(SessionProofError::Signature)
        );
        assert_eq!(viewer_proof.verify(&offer_proof, OFFER, NOW + 1), Ok(()));
        assert_eq!(
            viewer_proof.verify(&offer_proof, OFFER, NOW + 1),
            Err(SessionProofError::Replay)
        );
        assert_eq!(host_proof.verify(&answer_proof, ANSWER, NOW + 1), Ok(()));
        assert_eq!(
            host_proof.verify(&answer_proof, ANSWER, NOW + 1),
            Err(SessionProofError::Replay)
        );
    }

    #[test]
    fn rejects_wrong_room_nonce_role_key_challenge_and_local_proof_domain() {
        let (host, viewer, binding) = keys();
        let stranger = Identity::generate_ephemeral().unwrap();
        let hp = host.public().public_key;
        let vp = viewer.public().public_key;
        let mut pending =
            PendingSessionProof::issue(&binding, SessionRole::Host, NOW, TTL).unwrap();
        let challenge = pending.challenge();
        for different in [
            SessionBinding::new([1; 16], NONCE, hp, vp).unwrap(),
            SessionBinding::new(ROOM, [1; 32], hp, vp).unwrap(),
            SessionBinding::new(ROOM, NONCE, hp, stranger.public().public_key).unwrap(),
        ] {
            let wrong = host
                .sign_session_proof(&different, SessionRole::Host, challenge, ANSWER, NOW)
                .unwrap();
            assert_eq!(
                pending.verify(&wrong, ANSWER, NOW),
                Err(SessionProofError::Signature)
            );
        }
        assert_eq!(
            host.sign_session_proof(&binding, SessionRole::Viewer, challenge, ANSWER, NOW),
            Err(SessionProofError::Binding)
        );
        assert_eq!(
            stranger.sign_session_proof(&binding, SessionRole::Host, challenge, ANSWER, NOW),
            Err(SessionProofError::Binding)
        );
        let reflected = viewer
            .sign_session_proof(&binding, SessionRole::Viewer, challenge, ANSWER, NOW)
            .unwrap();
        assert_eq!(
            pending.verify(&reflected, ANSWER, NOW),
            Err(SessionProofError::Signature)
        );

        for offset in [0, 47] {
            let mut wire = challenge.to_bytes();
            wire[offset] ^= 1;
            let tampered = SessionChallenge::from_bytes(wire).unwrap();
            let proof = host
                .sign_session_proof(&binding, SessionRole::Host, tampered, ANSWER, NOW)
                .unwrap();
            assert_eq!(
                pending.verify(&proof, ANSWER, NOW),
                Err(SessionProofError::Signature)
            );
        }
        let local = host.local_proof(&challenge.random);
        assert_eq!(
            pending.verify(&local, ANSWER, NOW),
            Err(SessionProofError::Signature)
        );
        let valid = host
            .sign_session_proof(&binding, SessionRole::Host, challenge, ANSWER, NOW)
            .unwrap();
        assert!(!crate::verify_local_proof(&hp, &challenge.random, &valid));
        assert_eq!(pending.verify(&valid, ANSWER, NOW), Ok(()));
    }

    #[test]
    fn rejects_unbounded_payload_invalid_challenge_and_expiry_without_waiting() {
        let (host, viewer, binding) = keys();
        let public = host.public().public_key;
        let mut weak = [0; 32];
        weak[0] = 1;
        for other in [weak, public] {
            assert!(matches!(
                SessionBinding::new(ROOM, NONCE, public, other),
                Err(SessionProofError::Binding)
            ));
        }
        assert!(matches!(
            SessionBinding::new([0; 16], NONCE, public, viewer.public().public_key),
            Err(SessionProofError::Binding)
        ));
        assert!(matches!(
            SessionChallenge::from_bytes([0; CHALLENGE_BYTES]),
            Err(SessionProofError::Challenge)
        ));
        for invalid in [
            Duration::ZERO,
            Duration::from_millis(250),
            MAX_TTL + Duration::from_secs(1),
        ] {
            assert!(matches!(
                PendingSessionProof::issue(&binding, SessionRole::Host, NOW, invalid),
                Err(SessionProofError::Challenge)
            ));
        }
        assert!(matches!(
            PendingSessionProof::issue(&binding, SessionRole::Host, u64::MAX, TTL),
            Err(SessionProofError::Clock)
        ));
        let mut pending =
            PendingSessionProof::issue(&binding, SessionRole::Host, NOW, TTL).unwrap();
        let challenge = pending.challenge();
        let sign = |data: &[u8], now| {
            host.sign_session_proof(&binding, SessionRole::Host, challenge, data, now)
        };
        assert_eq!(sign(ANSWER, NOW - 6), Err(SessionProofError::Clock));
        assert_eq!(
            sign(ANSWER, NOW + TTL.as_secs()),
            Err(SessionProofError::Expired)
        );
        assert_eq!(sign(&[], NOW), Err(SessionProofError::Payload));
        assert_eq!(
            sign(&vec![0; MAX_SIGNAL_BYTES + 1], NOW),
            Err(SessionProofError::Payload)
        );
        let signature = sign(ANSWER, NOW).unwrap();
        assert_eq!(
            pending.verify(&signature, ANSWER, NOW - 1),
            Err(SessionProofError::Clock)
        );
        assert_eq!(
            pending.verify(&signature, ANSWER, NOW + TTL.as_secs()),
            Err(SessionProofError::Expired)
        );
        pending.deadline = Instant::now();
        assert_eq!(
            pending.verify(&signature, ANSWER, NOW),
            Err(SessionProofError::Expired)
        );
    }
}

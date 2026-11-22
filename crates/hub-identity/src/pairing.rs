//! Manual local pairing of public identities. A pasted key is untrusted until
//! its full fingerprint matches one independently confirmed with the intended
//! peer. This does not persist trust or authorize viewing and input.

use crate::{
    lower_hex, public_fingerprint,
    session::{checked_key, SessionBinding, SessionProofError, SessionRole},
    Identity, PublicIdentity,
};
use std::fmt;

const CODE_PREFIX: &str = "RH2-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingError {
    CodeFormat,
    PublicKey,
    FingerprintFormat,
    FingerprintMismatch,
    SelfPairing,
}

impl PairingError {
    pub fn code(self) -> &'static str {
        match self {
            Self::CodeFormat => "PAIRING_CODE_FORMAT",
            Self::PublicKey => "PAIRING_PUBLIC_KEY_INVALID",
            Self::FingerprintFormat => "PAIRING_FINGERPRINT_FORMAT",
            Self::FingerprintMismatch => "PAIRING_FINGERPRINT_MISMATCH",
            Self::SelfPairing => "PAIRING_SELF",
        }
    }
}
impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for PairingError {}

/// Syntactically valid candidate; still no evidence that the intended peer
/// owns this key.
pub struct PairingCandidate {
    public_key: [u8; 32],
    fingerprint: String,
}

impl PairingCandidate {
    /// Accept exactly `RH2-` and 64 lowercase ASCII hex digits, including no
    /// surrounding whitespace. The carried key must be canonical and strong.
    pub fn parse(code: &str) -> Result<Self, PairingError> {
        let hex = code
            .strip_prefix(CODE_PREFIX)
            .ok_or(PairingError::CodeFormat)?;
        if !is_full_hex(hex) {
            return Err(PairingError::CodeFormat);
        }
        let mut public_key = [0; 32];
        for (octet, pair) in public_key.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            // Every nibble was checked as canonical lowercase hex.
            *octet = (nibble(pair[0]) << 4) | nibble(pair[1]);
        }
        checked_key(&public_key).map_err(|_| PairingError::PublicKey)?;
        let fingerprint = public_fingerprint(&public_key);
        Ok(Self {
            public_key,
            fingerprint,
        })
    }

    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Proof that the fingerprint was manually matched for this local identity.
/// It is not an authenticated network session or a persistent permission.
pub struct VerifiedPeer {
    public_key: [u8; 32],
    fingerprint: String,
    confirmed_by: [u8; 32],
}

impl VerifiedPeer {
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn is_for(&self, local: &PublicIdentity) -> bool {
        self.confirmed_by == local.public_key
    }

    /// Use the exact peer key only for the local identity that confirmed it.
    pub fn session_binding(
        &self,
        local: &PublicIdentity,
        role: SessionRole,
        room: [u8; 16],
        nonce: [u8; 32],
    ) -> Result<SessionBinding, SessionProofError> {
        if !self.is_for(local) {
            return Err(SessionProofError::Binding);
        }
        let (host, viewer) = match role {
            SessionRole::Host => (local.public_key, self.public_key),
            SessionRole::Viewer => (self.public_key, local.public_key),
        };
        SessionBinding::new(room, nonce, host, viewer)
    }
}

impl PublicIdentity {
    /// Copyable code for the public key. No signing key or disk access is required.
    pub fn pairing_code(&self) -> String {
        format!("{CODE_PREFIX}{}", lower_hex(&self.public_key))
    }

    /// Confirm a fingerprint obtained from the intended peer separately from
    /// the pairing-code transfer. Never pass the copied candidate's own
    /// computed fingerprint back to this method as confirmation.
    pub fn verify_peer(
        &self,
        candidate: &PairingCandidate,
        independently_confirmed_fingerprint: &str,
    ) -> Result<VerifiedPeer, PairingError> {
        let confirmed_by = self.public_key;
        checked_key(&confirmed_by).map_err(|_| PairingError::PublicKey)?;
        if confirmed_by == candidate.public_key {
            return Err(PairingError::SelfPairing);
        }
        if !is_full_hex(independently_confirmed_fingerprint) {
            return Err(PairingError::FingerprintFormat);
        }
        if independently_confirmed_fingerprint != candidate.fingerprint {
            return Err(PairingError::FingerprintMismatch);
        }
        Ok(VerifiedPeer {
            public_key: candidate.public_key,
            fingerprint: candidate.fingerprint.clone(),
            confirmed_by,
        })
    }
}

impl Identity {
    pub fn pairing_code(&self) -> String {
        self.public().pairing_code()
    }

    pub fn verify_peer(
        &self,
        candidate: &PairingCandidate,
        independently_confirmed_fingerprint: &str,
    ) -> Result<VerifiedPeer, PairingError> {
        self.public()
            .verify_peer(candidate, independently_confirmed_fingerprint)
    }
}

fn is_full_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn nibble(byte: u8) -> u8 {
    if byte.is_ascii_digit() {
        byte - b'0'
    } else {
        byte - b'a' + 10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_code_roundtrips_current_key_and_full_peer_fingerprint() {
        let local = Identity::generate_ephemeral().unwrap();
        let peer = Identity::generate_ephemeral().unwrap();
        let expected = peer.public();
        let local_public = local.public();
        let code = expected.pairing_code();
        assert_eq!(code.len(), 68);
        assert_eq!(peer.pairing_code(), code);
        let candidate = PairingCandidate::parse(&code).unwrap();
        assert_eq!(candidate.public_key(), &expected.public_key);
        assert_eq!(candidate.fingerprint(), expected.fingerprint);

        let verified = local
            .verify_peer(&candidate, &expected.fingerprint)
            .unwrap();
        assert_eq!(verified.public_key(), &expected.public_key);
        assert_eq!(verified.fingerprint(), expected.fingerprint);
        assert!(verified.is_for(&local_public));
        assert!(!verified.is_for(&expected));

        let mut invalid_local = local_public;
        invalid_local.public_key = [0; 32];
        assert!(matches!(
            invalid_local.verify_peer(&candidate, &expected.fingerprint),
            Err(PairingError::PublicKey)
        ));
    }

    #[test]
    fn both_confirmations_bind_the_same_ordered_session_keys() {
        use crate::session::PendingSessionProof;
        use std::time::Duration;

        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let host_public = host.public();
        let viewer_public = viewer.public();
        let host_code = PairingCandidate::parse(&host_public.pairing_code()).unwrap();
        let viewer_code = PairingCandidate::parse(&viewer_public.pairing_code()).unwrap();
        let host_pair = host_public
            .verify_peer(&viewer_code, &viewer_public.fingerprint)
            .unwrap();
        let viewer_pair = viewer_public
            .verify_peer(&host_code, &host_public.fingerprint)
            .unwrap();

        let room = [5; 16];
        let nonce = [6; 32];
        let host_binding = host_pair
            .session_binding(&host_public, SessionRole::Host, room, nonce)
            .unwrap();
        let viewer_binding = viewer_pair
            .session_binding(&viewer_public, SessionRole::Viewer, room, nonce)
            .unwrap();
        assert!(matches!(
            host_pair.session_binding(&viewer_public, SessionRole::Host, room, nonce),
            Err(SessionProofError::Binding)
        ));

        let time = 100;
        let mut challenge = PendingSessionProof::issue(
            &host_binding,
            SessionRole::Viewer,
            time,
            Duration::from_secs(10),
        )
        .unwrap();
        let fixture = b"test signal";
        let signature = viewer
            .sign_session_proof(
                &viewer_binding,
                SessionRole::Viewer,
                challenge.challenge(),
                fixture,
                time,
            )
            .unwrap();
        assert_eq!(challenge.verify(&signature, fixture, time), Ok(()));
    }

    #[test]
    fn malformed_short_and_noncanonical_codes_are_not_candidates() {
        let peer = Identity::generate_ephemeral().unwrap();
        let valid = peer.pairing_code();
        for bad in [
            peer.public().device_id,
            valid[..67].into(),
            format!("{valid}\n"),
            valid.replacen(CODE_PREFIX, "rh2-", 1),
            format!("{CODE_PREFIX}A{}", &valid[5..]),
            format!("{CODE_PREFIX}g{}", &valid[5..]),
            format!("{CODE_PREFIX}０{}", &valid[5..]),
        ] {
            assert!(matches!(
                PairingCandidate::parse(&bad),
                Err(PairingError::CodeFormat)
            ));
        }
        let mut weak = [0; 32];
        weak[0] = 1; // Ed25519 identity point (small order).
        for bad in [weak, [0xff; 32]] {
            let bad = format!("{CODE_PREFIX}{}", lower_hex(&bad));
            assert!(matches!(
                PairingCandidate::parse(&bad),
                Err(PairingError::PublicKey)
            ));
        }
    }

    #[test]
    fn only_full_exact_external_fingerprint_confirms_and_self_is_rejected() {
        let local = Identity::generate_ephemeral().unwrap();
        let peer = Identity::generate_ephemeral().unwrap();
        let candidate = PairingCandidate::parse(&peer.pairing_code()).unwrap();
        let expected = peer.public();
        for bad in [
            expected.device_id,
            expected.fingerprint[..16].into(),
            format!("{} ", expected.fingerprint),
            format!("A{}", &expected.fingerprint[1..]),
        ] {
            assert!(matches!(
                local.verify_peer(&candidate, &bad),
                Err(PairingError::FingerprintFormat)
            ));
        }
        assert!(matches!(
            local.verify_peer(&candidate, &local.public().fingerprint),
            Err(PairingError::FingerprintMismatch)
        ));
        let own = PairingCandidate::parse(&local.pairing_code()).unwrap();
        assert!(matches!(
            local.verify_peer(&own, &local.public().fingerprint),
            Err(PairingError::SelfPairing)
        ));
    }
}

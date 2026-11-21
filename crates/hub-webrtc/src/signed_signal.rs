//! P0-04b prerequisite only: authenticate an exact SDP transcript against a
//! paired device key and a one-time, locally held challenge. This is not a
//! product wire format, key store, pairing flow, or authorization to share.

use ring::{
    digest::{digest, SHA256},
    rand::{SecureRandom, SystemRandom},
    signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519},
};

use std::time::{Duration, Instant};

const DOMAIN: &[u8] = b"RemoteHub/P0-04b/spike/signed-sdp/v1\0";
const MAX_SDP_SIZE: usize = 16 * 1024;
pub const MAX_VALIDITY_SECONDS: u64 = 120;
const CLOCK_SKEW_SECONDS: u64 = 5;

pub type Hash = [u8; 32];

/// Device identifiers are SHA-256 fingerprints of full, already paired public
/// keys; an advertised public key or short device code cannot establish trust.
pub fn device_fingerprint(public_key: &[u8; 32]) -> Hash {
    hash(public_key)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Offer,
    /// Binds the answer to the exact signed offer from the same session.
    Answer {
        offer_transcript: Hash,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub purpose: Purpose,
    pub session_id: [u8; 16],
    pub sender: Hash,
    pub recipient: Hash,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChallengeValue {
    pub context: Context,
    pub nonce: Hash,
}

pub struct SignedSignal<'a> {
    pub challenge: ChallengeValue,
    pub issued_at: u64,
    pub expires_at: u64,
    pub sdp: &'a str,
    pub signature: [u8; 64],
}

/// Owned exclusively by the receiving session. Issue independent challenges
/// for the offer and answer. Never recreate this state from an incoming proof.
/// At most one verified envelope consumes it; no persistent nonce list needed.
pub struct OneTimeChallenge {
    value: ChallengeValue,
    issued_at: u64,
    expires_at: u64,
    deadline: Instant,
    used: bool,
}

pub struct VerifiedSignal<'a> {
    /// Only these exact bytes have been authenticated.
    pub sdp: &'a str,
    pub transcript: Hash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignalError {
    #[error("invalid signaling context")]
    Context,
    #[error("invalid or expired signaling validity window")]
    Freshness,
    #[error("signaling challenge already used")]
    Replay,
    #[error("invalid SDP or SHA-256 DTLS fingerprint")]
    Sdp,
    #[error("signaling signature did not verify")]
    Signature,
    #[error("secure challenge generation failed")]
    Random,
}

impl OneTimeChallenge {
    /// Obtain `context` from local session state and the paired public key, not
    /// from a received envelope. Times are UTC Unix seconds; clocks must agree
    /// within five seconds for this lab prerequisite.
    pub fn issue(context: Context, now: u64, lifetime_seconds: u64) -> Result<Self, SignalError> {
        if context.sender == context.recipient {
            return Err(SignalError::Context);
        }
        if !(1..=MAX_VALIDITY_SECONDS).contains(&lifetime_seconds) {
            return Err(SignalError::Freshness);
        }
        let expires_at = now
            .checked_add(lifetime_seconds)
            .ok_or(SignalError::Freshness)?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(lifetime_seconds))
            .ok_or(SignalError::Freshness)?;
        let mut nonce = [0u8; 32];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| SignalError::Random)?;
        Ok(Self {
            value: ChallengeValue { context, nonce },
            issued_at: now,
            expires_at,
            deadline,
            used: false,
        })
    }

    pub fn value(&self) -> ChallengeValue {
        self.value
    }

    /// `trusted_sender_key` must come from the local pairing store. A valid
    /// proof does not replace receiver consent, policy, or network checks.
    pub fn verify<'s>(
        &mut self,
        signal: &SignedSignal<'s>,
        trusted_sender_key: &[u8; 32],
        now: u64,
    ) -> Result<VerifiedSignal<'s>, SignalError> {
        if self.used {
            return Err(SignalError::Replay);
        }
        if self.value != signal.challenge
            || self.value.context.sender != device_fingerprint(trusted_sender_key)
        {
            return Err(SignalError::Context);
        }
        if Instant::now() >= self.deadline
            || !(self.issued_at..self.expires_at).contains(&now)
            || signal.issued_at.saturating_add(CLOCK_SKEW_SECONDS) < self.issued_at
            || signal.issued_at > now.saturating_add(CLOCK_SKEW_SECONDS)
            || signal.expires_at <= now
        {
            return Err(SignalError::Freshness);
        }
        let canonical = signal.canonical()?;
        UnparsedPublicKey::new(&ED25519, trusted_sender_key)
            .verify(&canonical, &signal.signature)
            .map_err(|_| SignalError::Signature)?;
        self.used = true;
        Ok(VerifiedSignal {
            sdp: signal.sdp,
            transcript: transcript(&canonical, &signal.signature),
        })
    }
}

impl<'a> SignedSignal<'a> {
    /// Signs the SDP and all context fields with the long-term key supplied by
    /// the native key store. The caller still owns the protected key lifecycle.
    pub fn sign(
        challenge: ChallengeValue,
        issued_at: u64,
        expires_at: u64,
        sdp: &'a str,
        sender_key: &Ed25519KeyPair,
    ) -> Result<Self, SignalError> {
        if challenge.context.sender != hash(sender_key.public_key().as_ref())
            || challenge.context.sender == challenge.context.recipient
        {
            return Err(SignalError::Context);
        }
        let mut signal = Self {
            challenge,
            issued_at,
            expires_at,
            sdp,
            signature: [0; 64],
        };
        let signature = sender_key.sign(&signal.canonical()?);
        signal.signature.copy_from_slice(signature.as_ref());
        Ok(signal)
    }

    /// Offer creators may use this result to issue the challenge for an answer
    /// to their own signed offer. Receivers use `VerifiedSignal::transcript`.
    pub fn transcript(&self) -> Result<Hash, SignalError> {
        Ok(transcript(&self.canonical()?, &self.signature))
    }

    fn canonical(&self) -> Result<Vec<u8>, SignalError> {
        let duration = self
            .expires_at
            .checked_sub(self.issued_at)
            .ok_or(SignalError::Freshness)?;
        if !(1..=MAX_VALIDITY_SECONDS).contains(&duration) {
            return Err(SignalError::Freshness);
        }
        let fingerprint = dtls_fingerprint(self.sdp)?;
        // Fixed-width fields in a domain-separated proof; this private
        // transcript construction is not a wire serialization contract.
        let mut bytes = Vec::with_capacity(DOMAIN.len() + 1 + 16 + 6 * 32 + 16);
        bytes.extend_from_slice(DOMAIN);
        let ctx = self.challenge.context;
        match ctx.purpose {
            Purpose::Offer => {
                bytes.push(1);
                bytes.extend_from_slice(&[0; 32]);
            }
            Purpose::Answer { offer_transcript } => {
                bytes.push(2);
                bytes.extend_from_slice(&offer_transcript);
            }
        }
        bytes.extend_from_slice(&ctx.session_id);
        bytes.extend_from_slice(&ctx.sender);
        bytes.extend_from_slice(&ctx.recipient);
        bytes.extend_from_slice(&self.challenge.nonce);
        bytes.extend_from_slice(&self.issued_at.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        bytes.extend_from_slice(&hash(self.sdp.as_bytes()));
        bytes.extend_from_slice(&fingerprint);
        Ok(bytes)
    }
}

fn hash(bytes: &[u8]) -> Hash {
    let mut value = [0; 32];
    value.copy_from_slice(digest(&SHA256, bytes).as_ref());
    value
}

fn transcript(canonical: &[u8], signature: &[u8; 64]) -> Hash {
    let mut sha = ring::digest::Context::new(&SHA256);
    sha.update(canonical);
    sha.update(signature);
    let mut value = [0; 32];
    value.copy_from_slice(sha.finish().as_ref());
    value
}

fn dtls_fingerprint(sdp: &str) -> Result<Hash, SignalError> {
    if sdp.len() > MAX_SDP_SIZE
        || !sdp
            .bytes()
            .all(|v| matches!(v, b'\r' | b'\n' | b' '..=b'~'))
    {
        return Err(SignalError::Sdp);
    }
    let mut found = None;
    for line in sdp.lines() {
        if line != line.trim() || line.contains('\r') {
            return Err(SignalError::Sdp);
        }
        let Some(attr) = line
            .get(..14)
            .filter(|s| s.eq_ignore_ascii_case("a=fingerprint:"))
        else {
            continue;
        };
        let Some((alg, hex)) = line[attr.len()..].split_once(' ') else {
            return Err(SignalError::Sdp);
        };
        if !alg.eq_ignore_ascii_case("sha-256") || hex.len() != 95 {
            return Err(SignalError::Sdp);
        }
        let mut fingerprint = [0; 32];
        for (i, octet) in hex.split(':').enumerate() {
            if i >= fingerprint.len()
                || octet.len() != 2
                || !octet.bytes().all(|v| v.is_ascii_hexdigit())
            {
                return Err(SignalError::Sdp);
            }
            fingerprint[i] = u8::from_str_radix(octet, 16).map_err(|_| SignalError::Sdp)?;
        }
        if found.is_some_and(|old| old != fingerprint) {
            return Err(SignalError::Sdp);
        }
        found = Some(fingerprint);
    }
    found.ok_or(SignalError::Sdp)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT: &str = "AA:AB:AC:AD:AE:AF:B0:B1:B2:B3:B4:B5:B6:B7:B8:B9:BA:BB:BC:BD:BE:BF:C0:C1:C2:C3:C4:C5:C6:C7:C8:C9";
    const NOW: u64 = 1_800_000_000;

    fn key(marker: u8) -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&[marker; 32]).unwrap()
    }

    fn public(key: &Ed25519KeyPair) -> [u8; 32] {
        key.public_key().as_ref().try_into().unwrap()
    }

    fn context(sender: &Ed25519KeyPair, recipient: &Ed25519KeyPair, purpose: Purpose) -> Context {
        Context {
            purpose,
            session_id: [0x12; 16],
            sender: device_fingerprint(&public(sender)),
            recipient: device_fingerprint(&public(recipient)),
        }
    }

    fn sdp() -> String {
        format!("v=0\r\na=fingerprint:sha-256 {FINGERPRINT}\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n")
    }

    #[test]
    fn exact_sdp_authenticates_once_and_answer_binds_to_offer() {
        let (sender, receiver) = (key(1), key(2));
        let sdp = sdp();
        let mut offer_challenge =
            OneTimeChallenge::issue(context(&sender, &receiver, Purpose::Offer), NOW, 20).unwrap();
        let offer =
            SignedSignal::sign(offer_challenge.value(), NOW, NOW + 20, &sdp, &sender).unwrap();
        let verified = offer_challenge
            .verify(&offer, &public(&sender), NOW + 1)
            .unwrap();
        assert_eq!(verified.sdp.as_bytes(), sdp.as_bytes());
        assert_eq!(verified.transcript, offer.transcript().unwrap());
        assert_eq!(
            offer_challenge
                .verify(&offer, &public(&sender), NOW + 1)
                .err(),
            Some(SignalError::Replay)
        );

        let answer_ctx = context(
            &receiver,
            &sender,
            Purpose::Answer {
                offer_transcript: verified.transcript,
            },
        );
        let mut answer_challenge = OneTimeChallenge::issue(answer_ctx, NOW + 1, 20).unwrap();
        let mut wrong_offer = answer_challenge.value();
        wrong_offer.context.purpose = Purpose::Answer {
            offer_transcript: [0; 32],
        };
        let wrong_answer =
            SignedSignal::sign(wrong_offer, NOW + 1, NOW + 20, &sdp, &receiver).unwrap();
        assert_eq!(
            answer_challenge
                .verify(&wrong_answer, &public(&receiver), NOW + 2)
                .err(),
            Some(SignalError::Context)
        );
        let answer =
            SignedSignal::sign(answer_challenge.value(), NOW + 1, NOW + 20, &sdp, &receiver)
                .unwrap();
        assert!(answer_challenge
            .verify(&answer, &public(&receiver), NOW + 2)
            .is_ok());
    }

    #[test]
    fn forged_modified_or_cross_session_proofs_do_not_consume_the_challenge() {
        let (sender, recipient, stranger) = (key(1), key(2), key(3));
        let mut challenge =
            OneTimeChallenge::issue(context(&sender, &recipient, Purpose::Offer), NOW, 20).unwrap();
        let sdp = sdp();
        let good = SignedSignal::sign(challenge.value(), NOW, NOW + 20, &sdp, &sender).unwrap();
        let modified = sdp.replace(" 96\r\n", " 98\r\n");
        let modified_dtls = sdp.replace("AA:AB", "AB:AB");
        for altered in [&modified, &modified_dtls] {
            let fake = SignedSignal {
                sdp: altered,
                ..good
            };
            assert_eq!(
                challenge.verify(&fake, &public(&sender), NOW + 1).err(),
                Some(SignalError::Signature)
            );
        }
        assert_eq!(
            challenge.verify(&good, &public(&stranger), NOW + 1).err(),
            Some(SignalError::Context)
        );
        assert_eq!(
            SignedSignal::sign(challenge.value(), NOW, NOW + 20, &sdp, &stranger).err(),
            Some(SignalError::Context)
        );
        let mut other_session = challenge.value();
        other_session.context.session_id[0] ^= 1;
        let mut other_recipient = challenge.value();
        other_recipient.context.recipient = device_fingerprint(&public(&stranger));
        let mut other_nonce = challenge.value();
        other_nonce.nonce[0] ^= 1;
        for context in [other_session, other_recipient, other_nonce] {
            let signed = SignedSignal::sign(context, NOW, NOW + 20, &sdp, &sender).unwrap();
            assert_eq!(
                challenge.verify(&signed, &public(&sender), NOW + 1).err(),
                Some(SignalError::Context)
            );
        }
        let forged = SignedSignal {
            signature: [0; 64],
            ..good
        };
        assert_eq!(
            challenge.verify(&forged, &public(&sender), NOW + 1).err(),
            Some(SignalError::Signature)
        );
        assert!(challenge.verify(&good, &public(&sender), NOW + 1).is_ok());
    }

    #[test]
    fn binding_rejects_expired_future_and_unbounded_proofs() {
        let (sender, receiver) = (key(1), key(2));
        let sdp = sdp();
        let ctx = context(&sender, &receiver, Purpose::Offer);
        assert_eq!(
            OneTimeChallenge::issue(ctx, NOW, MAX_VALIDITY_SECONDS + 1).err(),
            Some(SignalError::Freshness)
        );
        let mut challenge = OneTimeChallenge::issue(ctx, NOW, 10).unwrap();
        let future =
            SignedSignal::sign(challenge.value(), NOW + 20, NOW + 25, &sdp, &sender).unwrap();
        let valid = SignedSignal::sign(challenge.value(), NOW, NOW + 20, &sdp, &sender).unwrap();
        let expired = SignedSignal::sign(challenge.value(), NOW, NOW + 2, &sdp, &sender).unwrap();
        for (bad, now) in [
            (&future, NOW + 1),
            (&valid, NOW + 10),
            (&expired, NOW + 2),
            (&valid, NOW - 1),
        ] {
            assert_eq!(
                challenge.verify(bad, &public(&sender), now).err(),
                Some(SignalError::Freshness)
            );
        }
        assert_eq!(
            SignedSignal::sign(
                challenge.value(),
                NOW,
                NOW + MAX_VALIDITY_SECONDS + 1,
                &sdp,
                &sender
            )
            .err(),
            Some(SignalError::Freshness)
        );
        challenge.deadline = Instant::now();
        assert_eq!(
            challenge.verify(&valid, &public(&sender), NOW + 1).err(),
            Some(SignalError::Freshness)
        );
    }

    #[test]
    fn sdp_must_have_a_single_unambiguous_sha256_fingerprint() {
        let sdp = sdp();
        assert_eq!(dtls_fingerprint(&sdp).unwrap()[0], 0xaa);
        assert_eq!(
            dtls_fingerprint(&format!("{sdp}a=fingerprint:sha-256 {FINGERPRINT}\r\n")).unwrap()[0],
            0xaa
        );
        for bad in [
            "v=0".to_owned(),
            sdp.replace("sha-256", "sha-1"),
            sdp.replace("AA:AB", "ZZ:AB"),
            sdp.replace("AA:AB", "AB"),
            format!(
                "{sdp}a=fingerprint:sha-256 {}\r\n",
                FINGERPRINT.replace("AA:AB", "AB:AB")
            ),
            format!("{}{sdp}", " ".repeat(MAX_SDP_SIZE)),
        ] {
            assert_eq!(dtls_fingerprint(&bad), Err(SignalError::Sdp));
        }
    }
}

//! Offline parser for an explicit synthetic TURN-only role profile. Profile
//! fields do not select native identities; neither parsing nor a validated
//! snapshot authorizes connecting to a server, SCTP, media or real input.
use crate::{
    ice_policy::IceMode,
    public_fake_input::{FakeInputProofScope, FakeInputTestMode},
    public_fake_input_profile::{
        Error as ProfileError, SyntheticProfile, ValidatedSyntheticProfile,
    },
    public_video::PublicVideoScope,
};
use hub_identity::{pairing::VerifiedPeer, session::SessionRole, Identity};
use rustls::{pki_types::CertificateDer, RootCertStore};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{io::Read, net::SocketAddrV4};
use uuid::{Uuid, Variant, Version};

pub const MAX_PROFILE_BYTES: usize = 32 * 1024;
const MAX_CA_DER_BYTES: usize = 8 * 1024;
const MAX_APPROVAL_SECONDS: u64 = 300;
const VERSION: u8 = 1;
const MODE: &str = "synthetic-turn-only";

/// Supply independently chosen role, enrolled native identity, manually
/// verified peer and preapproved CA digest. Do not infer any from the file.
pub struct Approval<'a> {
    pub role: SessionRole,
    pub local: &'a Identity,
    pub peer: &'a VerifiedPeer,
    pub ca_sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Read,
    Size,
    Format,
    Version,
    Mode,
    Role,
    Scope,
    Expired,
    CaPin,
    CaDer,
    Identity,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Read => "SYNTHETIC_PROFILE_READ",
            Self::Size => "SYNTHETIC_PROFILE_SIZE",
            Self::Format => "SYNTHETIC_PROFILE_FORMAT",
            Self::Version => "SYNTHETIC_PROFILE_VERSION",
            Self::Mode => "SYNTHETIC_PROFILE_MODE",
            Self::Role => "SYNTHETIC_PROFILE_ROLE",
            Self::Scope => "SYNTHETIC_PROFILE_SCOPE",
            Self::Expired => "SYNTHETIC_PROFILE_EXPIRED",
            Self::CaPin => "SYNTHETIC_PROFILE_CA_PIN",
            Self::CaDer => "SYNTHETIC_PROFILE_CA_DER",
            Self::Identity => "SYNTHETIC_PROFILE_IDENTITY",
        })
    }
}

impl std::error::Error for Error {}

impl From<ProfileError> for Error {
    fn from(error: ProfileError) -> Self {
        match error {
            ProfileError::Mode => Self::Mode,
            ProfileError::Role => Self::Role,
            ProfileError::Scope | ProfileError::PairMismatch => Self::Scope,
            ProfileError::Expired => Self::Expired,
            ProfileError::Identity => Self::Identity,
        }
    }
}

// Serde rejects unknown and repeated fields; borrowed values avoid allocating
// from untrusted string lengths. No token, key, password or other secret is
// part of the accepted schema.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    version: u8,
    mode: &'a str,
    role: &'a str,
    room: &'a str,
    generation: &'a str,
    expires_at: u64,
    wss: &'a str,
    turn: &'a str,
    relay_ports: (u16, u16),
    ca_der_hex: &'a str,
    ca_sha256: &'a str,
    local_fingerprint: &'a str,
    peer_fingerprint: &'a str,
}

/// Read from the caller's already-opened local profile, up to 32 KiB plus
/// one overflow byte. The caller owns platform-specific file/path policy.
pub fn read_and_validate(
    reader: impl Read,
    approval: Approval<'_>,
    now: u64,
) -> Result<ValidatedSyntheticProfile, Error> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_PROFILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Read)?;
    parse_and_validate(&bytes, approval, now)
}

/// Validate shape and internal consistency only, including the digest of
/// the included CA bytes, without opening native identity state. A match
/// does not verify the claimed peer or approve this CA or route.
pub fn check_format(bytes: &[u8], expected_role: SessionRole, now: u64) -> Result<(), Error> {
    checked(bytes, expected_role, now).map(|_| ())
}

/// Parse the complete file against independent native approval. A nonzero
/// generation and random-form room do not by themselves provide replay
/// protection; the later one-use session proof still has to verify.
pub fn parse_and_validate(
    bytes: &[u8],
    approval: Approval<'_>,
    now: u64,
) -> Result<ValidatedSyntheticProfile, Error> {
    let profile = checked(bytes, approval.role, now)?;
    if profile.scope.route.ca_sha256 != approval.ca_sha256 {
        return Err(Error::CaPin);
    }
    profile
        .validate(approval.role, approval.local, approval.peer, now)
        .map_err(Into::into)
}

fn checked(
    bytes: &[u8],
    expected_role: SessionRole,
    now: u64,
) -> Result<SyntheticProfile<'_>, Error> {
    if bytes.is_empty() || bytes.len() > MAX_PROFILE_BYTES {
        return Err(Error::Size);
    }
    let doc: Document<'_> = serde_json::from_slice(bytes).map_err(|_| Error::Format)?;
    if doc.version != VERSION {
        return Err(Error::Version);
    }
    if doc.mode != MODE {
        return Err(Error::Mode);
    }
    let role = match doc.role {
        "host" => SessionRole::Host,
        "viewer" => SessionRole::Viewer,
        _ => return Err(Error::Role),
    };
    if role != expected_role {
        return Err(Error::Role);
    }
    let room = Uuid::from_bytes(lower_hex(doc.room).ok_or(Error::Scope)?);
    if room.get_version() != Some(Version::Random) || room.get_variant() != Variant::RFC4122 {
        return Err(Error::Scope);
    }
    let generation = lower_hex(doc.generation).ok_or(Error::Scope)?;
    let wss = socket(doc.wss)?;
    let turn = socket(doc.turn)?;
    let pinned = lower_hex(doc.ca_sha256).ok_or(Error::CaPin)?;
    if pinned == [0; 32] {
        return Err(Error::CaPin);
    }
    if generation == [0; 32] {
        return Err(Error::Scope);
    }
    if doc.expires_at <= now {
        return Err(Error::Expired);
    }
    if doc.expires_at - now > MAX_APPROVAL_SECONDS {
        return Err(Error::Scope);
    }
    let profile = SyntheticProfile {
        role,
        mode: FakeInputTestMode::Enabled,
        scope: FakeInputProofScope {
            room,
            generation,
            route: PublicVideoScope {
                wss,
                turn,
                ca_sha256: pinned,
                relay_ports: doc.relay_ports,
                mode: IceMode::RelayOnly,
                expires_at: doc.expires_at,
            },
        },
        local_fingerprint: doc.local_fingerprint,
        peer_fingerprint: doc.peer_fingerprint,
    };
    profile
        .scope
        .route
        .checked_mode(now)
        .map_err(|_| Error::Scope)?;
    for fingerprint in [doc.local_fingerprint, doc.peer_fingerprint] {
        if lower_hex::<32>(fingerprint).is_none_or(|value| value == [0; 32]) {
            return Err(Error::Identity);
        }
    }
    if doc.local_fingerprint == doc.peer_fingerprint {
        return Err(Error::Identity);
    }

    if !(2..=MAX_CA_DER_BYTES * 2).contains(&doc.ca_der_hex.len()) {
        return Err(Error::CaDer);
    }
    let (chunks, remainder) = doc.ca_der_hex.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(Error::CaDer);
    }
    let der: Vec<u8> = chunks
        .iter()
        .map(hex_byte)
        .collect::<Option<_>>()
        .ok_or(Error::CaDer)?;
    if <[u8; 32]>::from(Sha256::digest(&der)) != pinned {
        return Err(Error::CaPin);
    }
    // Verify that the referenced bytes can be parsed as an X.509 trust
    // anchor. This doesn't install trust or constitute a TLS verification.
    RootCertStore::empty()
        .add(CertificateDer::from(der))
        .map_err(|_| Error::CaDer)?;
    Ok(profile)
}

fn socket(value: &str) -> Result<SocketAddrV4, Error> {
    let parsed: SocketAddrV4 = value.parse().map_err(|_| Error::Scope)?;
    if parsed.to_string() != value {
        return Err(Error::Scope);
    }
    Ok(parsed)
}

fn lower_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N * 2 {
        return None;
    }
    let mut out = [0; N];
    for (slot, digits) in out.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        *slot = hex_byte(digits)?;
    }
    Some(out)
}

fn hex_byte([high, low]: &[u8; 2]) -> Option<u8> {
    fn digit(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            _ => None,
        }
    }
    Some((digit(*high)? << 4) | digit(*low)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hub_identity::pairing::PairingCandidate;
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    use serde_json::{json, Value};

    const NOW: u64 = 50_000;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn document(local: &Identity, peer: &Identity) -> (Value, [u8; 32]) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let anchor = params.self_signed(&key).unwrap();
        let der = anchor.der().as_ref();
        let pin: [u8; 32] = Sha256::digest(der).into();
        let pin_hex = hex(&pin);
        (
            json!({
                "version": 1,
                "mode": MODE,
                "role": "host",
                "room": Uuid::new_v4().simple().to_string(),
                "generation": "03".repeat(32),
                "expires_at": NOW + MAX_APPROVAL_SECONDS,
                "wss": "93.184.216.34:8443",
                "turn": "93.184.216.34:3478",
                "relay_ports": [49152, 49183],
                "ca_der_hex": hex(der),
                "ca_sha256": pin_hex,
                "local_fingerprint": local.public().fingerprint,
                "peer_fingerprint": peer.public().fingerprint,
            }),
            pin,
        )
    }

    fn identities() -> (Identity, Identity) {
        (
            Identity::generate_ephemeral().unwrap(),
            Identity::generate_ephemeral().unwrap(),
        )
    }

    fn pin(local: &Identity, remote: &Identity) -> VerifiedPeer {
        let public = remote.public();
        let candidate = PairingCandidate::parse(&public.pairing_code()).unwrap();
        local.verify_peer(&candidate, &public.fingerprint).unwrap()
    }

    #[test]
    fn self_consistent_profile_and_independent_approval_are_separate() {
        let (local, remote) = identities();
        let other = Identity::generate_ephemeral().unwrap();
        let peer = pin(&local, &remote);
        let wrong_owner = pin(&other, &remote);
        let (doc, ca_sha256) = document(&local, &remote);
        let bytes = serde_json::to_vec(&doc).unwrap();
        let approval = || Approval {
            role: SessionRole::Host,
            local: &local,
            peer: &peer,
            ca_sha256,
        };
        assert_eq!(check_format(&bytes, SessionRole::Host, NOW), Ok(()));
        assert!(read_and_validate(bytes.as_slice(), approval(), NOW).is_ok());
        assert_eq!(
            parse_and_validate(
                &bytes,
                Approval {
                    ca_sha256: [1; 32],
                    ..approval()
                },
                NOW
            )
            .err(),
            Some(Error::CaPin)
        );
        assert_eq!(
            parse_and_validate(
                &bytes,
                Approval {
                    peer: &wrong_owner,
                    ..approval()
                },
                NOW
            )
            .err(),
            Some(Error::Identity)
        );
        assert_eq!(
            read_and_validate(std::io::repeat(b' '), approval(), NOW).err(),
            Some(Error::Size)
        );
    }

    #[test]
    fn rejects_wrong_purpose_shape_scope_identity_or_ca() {
        let (local, peer) = identities();
        let (doc, _) = document(&local, &peer);
        for (key, replacement, expected) in [
            ("version", json!(2), Error::Version),
            ("mode", json!("public-video"), Error::Mode),
            ("role", json!("viewer"), Error::Role),
            ("room", json!("00".repeat(16)), Error::Scope),
            ("room", json!(Uuid::new_v4().to_string()), Error::Scope),
            ("generation", json!("00".repeat(32)), Error::Scope),
            ("generation", json!("AA".repeat(32)), Error::Scope),
            ("expires_at", json!(NOW), Error::Expired),
            ("expires_at", json!(NOW + 301), Error::Scope),
            ("wss", json!("example.org:8443"), Error::Scope),
            ("wss", json!("[::1]:8443"), Error::Scope),
            ("wss", json!("93.184.216.34:08443"), Error::Scope),
            ("turn", json!("8.8.8.8:3478"), Error::Scope),
            ("relay_ports", json!([49152, 49408]), Error::Scope),
            ("ca_der_hex", json!("0"), Error::CaDer),
            ("ca_der_hex", json!("00"), Error::CaPin),
            ("ca_sha256", json!("ab".repeat(32)), Error::CaPin),
            (
                "peer_fingerprint",
                doc["local_fingerprint"].clone(),
                Error::Identity,
            ),
            ("local_fingerprint", json!("AB".repeat(32)), Error::Identity),
            ("bearer", json!("unexpected"), Error::Format),
        ] {
            let mut invalid = doc.clone();
            invalid[key] = replacement;
            let bytes = serde_json::to_vec(&invalid).unwrap();
            assert_eq!(
                check_format(&bytes, SessionRole::Host, NOW),
                Err(expected),
                "unexpected outcome for {key}"
            );
        }
        let mut inconsistent = doc.clone();
        inconsistent["ca_der_hex"] = json!("00");
        inconsistent["ca_sha256"] = json!(hex(&Sha256::digest([0])));
        assert_eq!(
            check_format(
                &serde_json::to_vec(&inconsistent).unwrap(),
                SessionRole::Host,
                NOW
            ),
            Err(Error::CaDer)
        );
        let duplicate = format!("{{\"role\":\"viewer\",{}", &doc.to_string()[1..]);
        assert_eq!(
            check_format(duplicate.as_bytes(), SessionRole::Host, NOW),
            Err(Error::Format)
        );
    }
}

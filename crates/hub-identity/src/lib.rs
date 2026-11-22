//! Local identity only. No credentials, private-key export or network/IPC signing surface.
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::{fmt, path::Path};
#[cfg(any(windows, test))]
use zeroize::Zeroizing;

#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Random,
    Protect,
    Unprotect,
    Storage,
    UnsafeStorage,
    Busy,
    InvalidStore,
    IncompleteStore,
}
impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "IDENTITY_UNSUPPORTED",
            Self::Random => "IDENTITY_RANDOM_FAILED",
            Self::Protect => "IDENTITY_PROTECT_FAILED",
            Self::Unprotect => "IDENTITY_UNPROTECT_FAILED",
            Self::Storage => "IDENTITY_STORAGE_FAILED",
            Self::UnsafeStorage => "IDENTITY_STORAGE_UNSAFE",
            Self::Busy => "IDENTITY_BUSY",
            Self::InvalidStore => "IDENTITY_STORE_INVALID",
            Self::IncompleteStore => "IDENTITY_STORE_INCOMPLETE",
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for Error {}

#[derive(Clone, PartialEq, Eq)]
pub struct PublicIdentity {
    pub device_id: String,
    pub fingerprint: String,
    pub public_key: [u8; 32],
}

// Intentionally no Debug, Clone, Serialize, seed getter or general-purpose signing API.
pub struct Identity {
    key: SigningKey,
}
#[cfg(any(windows, test))]
const PAYLOAD_MAGIC: &[u8; 8] = b"RHIDKEY1";
const PROOF_DOMAIN: &[u8] = b"RemoteHub.identity.local-proof.v1\0";

impl Identity {
    pub fn public(&self) -> PublicIdentity {
        let public_key = self.key.verifying_key().to_bytes();
        let fingerprint: String = Sha256::digest(public_key)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let short = fingerprint[..12].to_uppercase();
        PublicIdentity {
            device_id: format!("RH-{}-{}-{}", &short[..4], &short[4..8], &short[8..]),
            fingerprint,
            public_key,
        }
    }

    /// Fixed-domain local self-test only. Never exposed as a Tauri command.
    pub fn local_proof(&self, nonce: &[u8; 32]) -> [u8; 64] {
        self.key.sign(&proof_message(nonce)).to_bytes()
    }

    #[cfg(windows)]
    fn payload(&self) -> Zeroizing<Vec<u8>> {
        let mut value = Zeroizing::new(Vec::with_capacity(72));
        value.extend_from_slice(PAYLOAD_MAGIC);
        value.extend_from_slice(self.key.as_bytes());
        value.extend_from_slice(self.key.verifying_key().as_bytes());
        value
    }

    #[cfg(any(windows, test))]
    fn from_payload(value: &[u8]) -> Result<Self, Error> {
        if value.len() != 72 || &value[..8] != PAYLOAD_MAGIC {
            return Err(Error::InvalidStore);
        }
        let mut seed = Zeroizing::new([0u8; 32]);
        seed.copy_from_slice(&value[8..40]);
        let key = SigningKey::from_bytes(&seed);
        if key.verifying_key().as_bytes() != &value[40..] {
            return Err(Error::InvalidStore);
        }
        Ok(Self { key })
    }
}

fn proof_message(nonce: &[u8; 32]) -> Vec<u8> {
    [PROOF_DOMAIN, nonce].concat()
}
pub fn verify_local_proof(public_key: &[u8; 32], nonce: &[u8; 32], signature: &[u8; 64]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    !key.is_weak()
        && key
            .verify_strict(&proof_message(nonce), &Signature::from_bytes(signature))
            .is_ok()
}

pub fn open_or_create(root: &Path) -> Result<Identity, Error> {
    #[cfg(windows)]
    return windows::open_or_create(root);
    #[cfg(not(windows))]
    {
        let _ = root;
        Err(Error::Unsupported)
    }
}

pub fn open_default() -> Result<Identity, Error> {
    #[cfg(windows)]
    return windows::open_default();
    #[cfg(not(windows))]
    Err(Error::Unsupported)
}

#[cfg(test)]
mod tests;

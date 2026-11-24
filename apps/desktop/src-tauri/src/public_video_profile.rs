//! Native-only, one-use input for the separately consented public video path.
//! This reads a bounded local snapshot; it never contacts the listed server.

use base64::{engine::general_purpose::STANDARD, Engine};
use hub_identity::{pairing::VerifiedPeer, Identity};
#[cfg(feature = "v2-public-control-desktop")]
use hub_webrtc::public_video_wss::{
    PublicControlNetworkConfig, PublicControlToken, PublicControlWssInvitation,
};
use hub_webrtc::{
    ice_policy::{public_v4, IceConfig, IceMode, IceServer, RelayGrant, RelayTarget},
    lan_wire::Key,
    public_video::PublicVideoScope,
    public_video_wss::{
        LoopbackTlsTrust, PublicVideoNetworkConfig, PublicVideoWssInvitation, RelayToken, Role,
    },
};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
    sync::Arc,
};
use uuid::Uuid;
use zeroize::Zeroizing;

pub(crate) const MAX_FILE: usize = 32 * 1024;
const MAX_CA_DER: usize = 16 * 1024;
const MAX_TTL: u64 = 600;
const MAX_IMMEDIATE_BATCH_TTL: u64 = 1800;
#[cfg(not(feature = "v2-public-video-60s-test"))]
const MIN_TURN_TTL: u64 = 60;
// The extended build must have enough already-approved lease time for
// pairing, path verification and a full 60-second capture. Never renew.
#[cfg(feature = "v2-public-video-60s-test")]
const MIN_TURN_TTL: u64 = 150;
const INVALID: &str = "PUBLIC_VIDEO_PROFILE_INVALID";
const FILE: &str = "PUBLIC_VIDEO_PROFILE_FILE";
const SIZE: &str = "PUBLIC_VIDEO_PROFILE_SIZE";
const PIN: &str = "PUBLIC_VIDEO_PROFILE_CA_PIN";
const EXPIRED: &str = "PUBLIC_VIDEO_PROFILE_EXPIRED";
const DENIED: &str = "PUBLIC_VIDEO_PROFILE_APPROVAL_REQUIRED";
const PEER: &str = "PUBLIC_VIDEO_PROFILE_PEER_MISMATCH";
const CONSUMED: &str = "PUBLIC_VIDEO_PROFILE_CONSUMED";

#[derive(Clone, Copy)]
pub(crate) struct PublicVideoProfileApproval {
    pub(crate) role: Role,
    pub(crate) room: Uuid,
    pub(crate) local_ipv4: Ipv4Addr,
    pub(crate) scope: PublicVideoScope,
}

pub(crate) struct PublicVideoProfile {
    display: PublicVideoProfileApproval,
    expected_peer_fingerprint: String,
    ca_der: Vec<u8>,
    credentials: Option<SessionInputs>,
    revision: ProfileRevision,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ProfileRevision {
    Legacy,
    ImmediateBatch,
    #[cfg(feature = "v2-session-window-sustained")]
    Sustained,
}

enum Invitation {
    Video(PublicVideoWssInvitation),
    #[cfg(feature = "v2-public-control-desktop")]
    Control(PublicControlWssInvitation),
}

struct SessionInputs {
    invitation: Invitation,
    pairing_key: Arc<Key>,
    wire_nonce: String,
    session_nonce: Zeroizing<[u8; 32]>,
    ice_config: IceConfig,
}

struct Sensitive(Zeroizing<String>);

impl<'de> Deserialize<'de> for Sensitive {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        String::deserialize(input).map(|text| Self(Zeroizing::new(text)))
    }
}

impl Sensitive {
    fn into_string(mut self) -> String {
        std::mem::take(&mut *self.0)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileJson {
    version: u8,
    purpose: Option<String>,
    role: Role,
    room: String,
    expires_at: u64,
    local_ipv4: Ipv4Addr,
    wss: SocketAddrV4,
    turn: SocketAddrV4,
    relay_ports: (u16, u16),
    mode: IceMode,
    ca_sha256: String,
    ca_der_base64: String,
    expected_peer_fingerprint: String,
    relay_token: Sensitive,
    pairing_key: Sensitive,
    wire_nonce: String,
    session_nonce: Sensitive,
    turn_username: Sensitive,
    turn_credential: Sensitive,
    turn_expires_at: u64,
}

impl PublicVideoProfile {
    /// Call with an exact path chosen in the native UI, never an IPC payload.
    /// The accepted bytes are snapshotted once; consent never reopens the file.
    pub(crate) fn load(path: &Path, now: u64) -> Result<Self, &'static str> {
        Self::load_with_purpose(path, now, false)
    }

    #[cfg(feature = "v2-public-control-desktop")]
    pub(crate) fn load_control(path: &Path, now: u64) -> Result<Self, &'static str> {
        Self::load_with_purpose(path, now, true)
    }

    fn load_with_purpose(path: &Path, now: u64, control: bool) -> Result<Self, &'static str> {
        let file = open_local_regular(path, MAX_FILE as u64)?;
        let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_FILE + 1));
        file.take((MAX_FILE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| FILE)?;
        Self::parse_with_purpose(&bytes, now, control)
    }

    #[cfg(test)]
    fn parse(bytes: &[u8], now: u64) -> Result<Self, &'static str> {
        Self::parse_with_purpose(bytes, now, false)
    }

    pub(super) fn parse_with_purpose(
        bytes: &[u8],
        now: u64,
        control: bool,
    ) -> Result<Self, &'static str> {
        if bytes.is_empty() || bytes.len() > MAX_FILE {
            return Err(SIZE);
        }
        let input: ProfileJson = serde_json::from_slice(bytes).map_err(|_| INVALID)?;
        let room = canonical_uuid(&input.room)?;
        let wire_nonce = canonical_uuid(&input.wire_nonce)?;
        let mut session_nonce = Zeroizing::new([0; 32]);
        *session_nonce = parse_hex::<32>(&input.session_nonce.0)?;
        let ca_sha256 = parse_hex::<32>(&input.ca_sha256)?;
        let fingerprint = parse_hex::<32>(&input.expected_peer_fingerprint)?;
        let pair_bytes = Zeroizing::new(parse_pairing_key(&input.pairing_key.0)?);
        let revision = match input.version {
            1 => ProfileRevision::Legacy,
            2 if control => ProfileRevision::ImmediateBatch,
            #[cfg(feature = "v2-session-window-sustained")]
            3 if control && !cfg!(feature = "v2-public-video-60s-test") => {
                ProfileRevision::Sustained
            }
            _ => return Err(INVALID),
        };
        let purpose_ok = if control {
            cfg!(feature = "v2-public-control-desktop")
                && input.purpose.as_deref() == Some("public-control")
        } else {
            input.purpose.is_none()
        };
        if !purpose_ok
            || (revision != ProfileRevision::Legacy
                && (cfg!(feature = "v2-public-video-60s-test")
                    || room.get_version() != Some(uuid::Version::Random)
                    || room.get_variant() != uuid::Variant::RFC4122))
            || !matches!(input.mode, IceMode::DirectPreferred | IceMode::RelayOnly)
            || wire_nonce == room
            || *session_nonce == [0; 32]
            || fingerprint == [0; 32]
            || *pair_bytes == [0; 16]
            || *pair_bytes == *room.as_bytes()
            || *pair_bytes == *wire_nonce.as_bytes()
            || !(input.local_ipv4.is_private() || public_v4(input.local_ipv4))
            || !public_v4(*input.wss.ip())
            || input.turn.ip() != input.wss.ip()
            || input.wss.port() < 1024
            || input.turn.port() < 1024
            || input.relay_ports.0 < 49152
            || !input
                .relay_ports
                .1
                .checked_sub(input.relay_ports.0)
                .is_some_and(|span| span <= 255)
        {
            return Err(INVALID);
        }
        ensure_profile_fresh(input.expires_at, input.turn_expires_at, now, revision)?;

        let ca_der = STANDARD.decode(input.ca_der_base64).map_err(|_| PIN)?;
        if ca_der.is_empty()
            || ca_der.len() > MAX_CA_DER
            || ca_sha256 == [0; 32]
            || <[u8; 32]>::from(Sha256::digest(&ca_der)) != ca_sha256
        {
            return Err(PIN);
        }
        // The hash pins exact bytes; rustls must also accept those bytes as
        // the only CA root independently of any runtime platform trust store.
        LoopbackTlsTrust::from_ca_der(&ca_der).map_err(|_| PIN)?;

        let expected_username = format!("{}:{room}", input.turn_expires_at);
        let turn_raw = Zeroizing::new(
            STANDARD
                .decode(&*input.turn_credential.0)
                .map_err(|_| INVALID)?,
        );
        if input.turn_username.0.as_str() != expected_username
            || input.turn_credential.0.len() != 28
            || turn_raw.len() != 20
        {
            return Err(INVALID);
        }
        let token_hex = if control {
            input.relay_token.0.strip_prefix("rhctl1-").ok_or(INVALID)?
        } else {
            input.relay_token.0.as_str()
        };
        let token_bytes = Zeroizing::new(parse_hex::<32>(token_hex)?);
        if *token_bytes == [0; 32] || *token_bytes == *session_nonce {
            return Err(INVALID);
        }
        let invitation = if control {
            #[cfg(feature = "v2-public-control-desktop")]
            {
                let token = PublicControlToken::new(input.relay_token.into_string())
                    .map_err(|_| INVALID)?;
                let invitation = match revision {
                    ProfileRevision::Legacy => {
                        PublicControlWssInvitation::new(room, input.role, input.expires_at, token)
                    }
                    ProfileRevision::ImmediateBatch => {
                        PublicControlWssInvitation::new_immediate_batch(
                            room,
                            input.role,
                            input.expires_at,
                            token,
                        )
                    }
                    #[cfg(feature = "v2-session-window-sustained")]
                    ProfileRevision::Sustained => PublicControlWssInvitation::new_basic_sustained(
                        room,
                        input.role,
                        input.expires_at,
                        token,
                        now,
                    ),
                }?;
                Invitation::Control(invitation)
            }
            #[cfg(not(feature = "v2-public-control-desktop"))]
            {
                return Err(INVALID);
            }
        } else {
            let token = RelayToken::new(input.relay_token.into_string()).map_err(|_| INVALID)?;
            Invitation::Video(PublicVideoWssInvitation::new(
                room,
                input.role,
                input.expires_at,
                token,
            )?)
        };
        let pairing_key = Arc::new(Key::parse(&input.pairing_key.0).map_err(|_| INVALID)?);
        let ice_config = IceConfig {
            mode: input.mode,
            ice_servers: vec![IceServer {
                urls: vec![format!("turn:{}?transport=udp", input.turn)],
                username: input.turn_username.into_string(),
                credential: input.turn_credential.into_string(),
            }],
            expires_at: input.turn_expires_at,
            session_id: room.to_string(),
        };
        Ok(Self {
            display: PublicVideoProfileApproval {
                role: input.role,
                room,
                local_ipv4: input.local_ipv4,
                scope: PublicVideoScope {
                    wss: input.wss,
                    ca_sha256,
                    turn: input.turn,
                    relay_ports: input.relay_ports,
                    mode: input.mode,
                    expires_at: input.expires_at,
                },
            },
            expected_peer_fingerprint: input.expected_peer_fingerprint,
            ca_der,
            revision,
            credentials: Some(SessionInputs {
                invitation,
                pairing_key,
                wire_nonce: wire_nonce.to_string(),
                session_nonce,
                ice_config,
            }),
        })
    }

    pub(crate) fn approval(&self) -> PublicVideoProfileApproval {
        self.display
    }

    #[cfg(feature = "v2-public-control-desktop")]
    pub(crate) fn is_control_batch(&self) -> bool {
        self.revision == ProfileRevision::ImmediateBatch
    }

    #[cfg(feature = "v2-session-window-sustained")]
    pub(crate) fn is_sustained_control(&self) -> bool {
        self.revision == ProfileRevision::Sustained
    }

    #[cfg(feature = "v2-control-invitations")]
    pub(crate) fn verify_invitation_time(&self, now: u64) -> Result<(), &'static str> {
        ensure_profile_fresh(
            self.display.scope.expires_at,
            self.credentials
                .as_ref()
                .ok_or(CONSUMED)?
                .ice_config
                .expires_at,
            now,
            self.revision,
        )
    }

    pub(crate) fn expected_peer_fingerprint(&self) -> &str {
        &self.expected_peer_fingerprint
    }

    /// Move short-lived secrets exactly once after the local native prompt.
    /// The remote role's actual approval/identity must still be proven by the
    /// independent approved profile and signed session exchange before ICE.
    pub(crate) fn take_config<'a>(
        &'a mut self,
        local: &'a Identity,
        peer: &'a VerifiedPeer,
        now: u64,
        approved: bool,
    ) -> Result<PublicVideoNetworkConfig<'a>, &'static str> {
        if !matches!(
            self.credentials.as_ref().map(|c| &c.invitation),
            Some(Invitation::Video(_))
        ) {
            return Err(if self.credentials.is_none() {
                CONSUMED
            } else {
                INVALID
            });
        }
        let (credentials, grant) = self.take_inputs(local, peer, now, approved)?;
        #[allow(irrefutable_let_patterns)]
        let Invitation::Video(invitation) = credentials.invitation
        else {
            return Err(INVALID);
        };
        Ok(PublicVideoNetworkConfig {
            invitation,
            pairing_key: credentials.pairing_key,
            wire_nonce: credentials.wire_nonce,
            session_nonce: *credentials.session_nonce,
            ca_der: &self.ca_der,
            local_identity: local,
            peer,
            scope: self.display.scope,
            relay_grant: grant,
            ice_config: credentials.ice_config,
        })
    }

    #[cfg(feature = "v2-public-control-desktop")]
    pub(crate) fn take_control_config<'a>(
        &'a mut self,
        local: &'a Identity,
        peer: &'a VerifiedPeer,
        now: u64,
        approved: bool,
    ) -> Result<PublicControlNetworkConfig<'a>, &'static str> {
        if !matches!(
            self.credentials.as_ref().map(|c| &c.invitation),
            Some(Invitation::Control(_))
        ) {
            return Err(if self.credentials.is_none() {
                CONSUMED
            } else {
                INVALID
            });
        }
        let (credentials, grant) = self.take_inputs(local, peer, now, approved)?;
        let Invitation::Control(invitation) = credentials.invitation else {
            return Err(INVALID);
        };
        Ok(PublicControlNetworkConfig {
            invitation,
            pairing_key: credentials.pairing_key,
            wire_nonce: credentials.wire_nonce,
            session_nonce: *credentials.session_nonce,
            ca_der: &self.ca_der,
            local_identity: local,
            peer,
            scope: self.display.scope,
            relay_grant: grant,
            ice_config: credentials.ice_config,
        })
    }

    fn take_inputs(
        &mut self,
        local: &Identity,
        peer: &VerifiedPeer,
        now: u64,
        approved: bool,
    ) -> Result<(SessionInputs, RelayGrant), &'static str> {
        if !approved {
            return Err(DENIED);
        }
        if peer.fingerprint() != self.expected_peer_fingerprint || !peer.is_for(&local.public()) {
            return Err(PEER);
        }
        let approval = self.display;
        let credentials = self.credentials.as_ref().ok_or(CONSUMED)?;
        ensure_profile_fresh(
            approval.scope.expires_at,
            credentials.ice_config.expires_at,
            now,
            self.revision,
        )?;
        let grant = RelayGrant::new(
            RelayTarget {
                local: approval.local_ipv4,
                turn: approval.scope.turn,
                relay: *approval.scope.turn.ip(),
                port_min: approval.scope.relay_ports.0,
                port_max: approval.scope.relay_ports.1,
            },
            &approval.room.to_string(),
            approval.scope.expires_at,
            approved,
        )
        .map_err(|_| INVALID)?;
        Ok((self.credentials.take().ok_or(CONSUMED)?, grant))
    }
}

fn ensure_profile_fresh(
    scope_expires: u64,
    turn_expires: u64,
    now: u64,
    revision: ProfileRevision,
) -> Result<(), &'static str> {
    let remaining = match revision {
        ProfileRevision::Legacy => return ensure_fresh(scope_expires, turn_expires, now),
        ProfileRevision::ImmediateBatch => MIN_TURN_TTL..=MAX_IMMEDIATE_BATCH_TTL,
        #[cfg(feature = "v2-session-window-sustained")]
        ProfileRevision::Sustained => {
            hub_webrtc::public_video::SUSTAINED_CONTROL_BOOTSTRAP_MIN_SECONDS
                ..=hub_webrtc::public_video::SUSTAINED_CONTROL_SCOPE_TTL_SECONDS
        }
    };
    if turn_expires != scope_expires || !remaining.contains(&scope_expires.saturating_sub(now)) {
        return Err(EXPIRED);
    }
    Ok(())
}

fn ensure_fresh(scope_expires: u64, turn_expires: u64, now: u64) -> Result<(), &'static str> {
    let left = turn_expires.saturating_sub(now);
    if turn_expires > scope_expires
        || !(1..=MAX_TTL).contains(&scope_expires.saturating_sub(now))
        || !(MIN_TURN_TTL..=MAX_TTL).contains(&left)
    {
        return Err(EXPIRED);
    }
    Ok(())
}

fn canonical_uuid(text: &str) -> Result<Uuid, &'static str> {
    let uuid = Uuid::try_parse(text).map_err(|_| INVALID)?;
    if uuid.is_nil() || uuid.to_string() != text {
        return Err(INVALID);
    }
    Ok(uuid)
}

fn parse_hex<const N: usize>(text: &str) -> Result<[u8; N], &'static str> {
    fn nibble(byte: u8) -> Result<u8, &'static str> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(INVALID),
        }
    }
    if text.len() != N * 2 {
        return Err(INVALID);
    }
    let mut out = Zeroizing::new([0; N]);
    for (slot, pair) in out.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        *slot = nibble(pair[0])? << 4 | nibble(pair[1])?;
    }
    Ok(*out)
}

fn parse_pairing_key(text: &str) -> Result<[u8; 16], &'static str> {
    if text.len() == 32 {
        return parse_hex(text);
    }
    canonical_uuid(text).map(|uuid| *uuid.as_bytes())
}

#[cfg(windows)]
pub(crate) fn open_local_regular(path: &Path, max_size: u64) -> Result<File, &'static str> {
    use std::{
        os::windows::fs::{MetadataExt, OpenOptionsExt},
        path::{Component, Prefix},
    };
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const REPARSE_POINT: u32 = 0x0400;
    const SHARE_READ_ONLY: u32 = 1;

    use windows::{core::PCWSTR, Win32::Storage::FileSystem::GetDriveTypeW};
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_RAMDISK: u32 = 6;
    let Some(Component::Prefix(component)) = path.components().next() else {
        return Err(FILE);
    };
    let letter = match component.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) if path.is_absolute() => letter,
        _ => return Err(FILE),
    };
    let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
    // SAFETY: root is a valid NUL-terminated UTF-16 drive root and lives
    // through the call. Reject network and unknown drives before file I/O.
    let drive_type = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
    if !matches!(drive_type, DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_RAMDISK) {
        return Err(FILE);
    }
    let mut prefix = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {}
            Component::Normal(name) => {
                // Disallow ADS and ambiguous DOS path forms without a canonicalization read.
                let name = name.to_str().ok_or(FILE)?;
                if !normal_path_name(name) {
                    return Err(FILE);
                }
            }
            _ => return Err(FILE),
        }
        prefix.push(component.as_os_str());
        if matches!(component, Component::Normal(_))
            && std::fs::symlink_metadata(&prefix)
                .map_err(|_| FILE)?
                .file_attributes()
                & REPARSE_POINT
                != 0
        {
            return Err(FILE);
        }
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .share_mode(SHARE_READ_ONLY)
        .open(path)
        .map_err(|_| FILE)?;
    let metadata = file.metadata().map_err(|_| FILE)?;
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(FILE);
    }
    if metadata.len() > max_size {
        return Err(SIZE);
    }
    Ok(file)
}

#[cfg(any(windows, test))]
fn normal_path_name(name: &str) -> bool {
    if name.contains(':') || name.ends_with(['.', ' ']) {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return false;
    }
    !stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

#[cfg(not(windows))]
fn open_local_regular(_: &Path, _: u64) -> Result<File, &'static str> {
    // This module belongs to the Windows-only feature. Unit parser fixtures
    // also run on Linux without introducing a portable profile file opener.
    Err(FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const NOW: u64 = 10_000;
    const FIXTURE_TURN_TTL: u64 = MIN_TURN_TTL + 40;
    const ROOM: &str = "12345678-1234-4234-8234-123456789abc";
    // Public, independently generated test CA; no corresponding key is stored.
    const FIXTURE_CA: &str = "MIIBpzCCAU2gAwIBAgIUZF+y9UzgIF1CLzMjQUhR/B5D5S4wCgYIKoZIzj0EAwIwKTEnMCUGA1UEAwweUmVtb3RlSHViLXByb2ZpbGUtZml4dHVyZS1vbmx5MB4XDTI2MTEyMzEwMDA1OFoXDTI3MTEyMzEwMDA1OFowKTEnMCUGA1UEAwweUmVtb3RlSHViLXByb2ZpbGUtZml4dHVyZS1vbmx5MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEi3gv4JryBxxuCU9DDEICKUJafSQ3O/uKz8CrHciFnMOVSPS9rupgHZeyMogWSOEQ4HSz43yO2dHac0HcIZ99+KNTMFEwHQYDVR0OBBYEFHNCmZBsyU07uAny4QluUTX070w9MB8GA1UdIwQYMBaAFHNCmZBsyU07uAny4QluUTX070w9MA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSAAwRQIhAIdgwr/NL8t/fpSawiLUKOuZvS4inrClYYEb2KOsYrGsAiAsUCOv4eUoKHwcYOdR63r5PHPcKi+QOyRgWt6qkLfoLw==";

    fn fixture() -> Value {
        let der = STANDARD.decode(FIXTURE_CA).unwrap();
        json!({
            "version": 1,
            "role": "host",
            "room": ROOM,
            "expires_at": NOW + FIXTURE_TURN_TTL + 20,
            "local_ipv4": "192.168.2.3",
            "wss": "93.184.216.34:8443",
            "turn": "93.184.216.34:3478",
            "relay_ports": [49152, 49183],
            "mode": "direct_preferred",
            "ca_sha256": format!("{:x}", Sha256::digest(&der)),
            "ca_der_base64": FIXTURE_CA,
            "expected_peer_fingerprint": "f1".repeat(32),
            "relay_token": "45".repeat(32),
            "pairing_key": "6789".repeat(8),
            "wire_nonce": "12345678-1234-4234-8234-123456789abd",
            "session_nonce": "ab".repeat(32),
            "turn_username": format!("{}:{ROOM}", NOW + FIXTURE_TURN_TTL),
            "turn_credential": STANDARD.encode([7; 20]),
            "turn_expires_at": NOW + FIXTURE_TURN_TTL
        })
    }

    fn parse_fixture(input: &Value, now: u64) -> Result<PublicVideoProfile, &'static str> {
        PublicVideoProfile::parse(&serde_json::to_vec(input).unwrap(), now)
    }

    #[cfg(feature = "v2-control-invitations")]
    #[test]
    fn role_zip_lists_sorted_fresh_rounds_and_remembers_consumption() {
        use crate::control_invitations::{load, Invitations};
        use std::io::{Cursor, Write};
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        for role in [Role::Host, Role::Viewer] {
            let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for number in (1..=8).rev() {
                let mut doc = fixture();
                let room = Uuid::new_v4();
                let expires = if number == 1 {
                    now
                } else {
                    now + FIXTURE_TURN_TTL
                };
                doc["version"] = json!(2);
                doc["room"] = json!(room.to_string());
                doc["role"] = json!(if role == Role::Host { "host" } else { "viewer" });
                doc["purpose"] = json!("public-control");
                doc["mode"] = json!("relay_only");
                doc["relay_token"] = json!(format!("rhctl1-{}", "45".repeat(32)));
                doc["expires_at"] = json!(expires);
                doc["turn_expires_at"] = json!(expires);
                doc["turn_username"] = json!(format!("{expires}:{room}"));
                archive
                    .start_file(format!("round-{number:02}.json"), opts)
                    .unwrap();
                archive
                    .write_all(&serde_json::to_vec(&doc).unwrap())
                    .unwrap();
            }
            // The portable packs also contain client binaries; nothing is unpacked.
            archive.start_file("remotehub-desktop.exe", opts).unwrap();
            archive.write_all(b"fixture").unwrap();
            let path = std::env::temp_dir().join(format!("邀请-{}.ZIP", Uuid::new_v4()));
            std::fs::write(&path, archive.finish().unwrap().into_inner()).unwrap();
            let rounds = load(&path, now).unwrap();
            let reopened = load(&path, now).unwrap();
            std::fs::remove_file(path).unwrap();
            assert_eq!(
                rounds.iter().map(|(n, _)| n.unwrap()).collect::<Vec<_>>(),
                (2..=8).collect::<Vec<_>>()
            );
            let mut invites = Invitations::default();
            let previews = invites.select(rounds, now).unwrap();
            let used = invites.take(&previews[4].id, role, now).unwrap().approval();
            invites.consume(used.room, used.scope.expires_at).unwrap();
            assert!(invites.take(&previews[4].id, role, now).is_err());
            assert!(invites.take(&previews[0].id, role, now).is_ok());
            let list = invites.select(reopened, now).unwrap();
            let value = serde_json::to_value(&list).unwrap();
            assert_eq!(list.len(), 6);
            assert!(!value
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["round"] == 6));
            assert!(value
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item.as_object().unwrap().len() == 4));
        }
    }

    #[cfg(feature = "v2-control-invitations")]
    #[test]
    fn invalid_zip_does_not_replace_the_selected_batch() {
        use crate::control_invitations::{load, Invitations};
        use std::io::{Cursor, Write};
        let mut doc = fixture();
        doc["purpose"] = json!("public-control");
        doc["mode"] = json!("relay_only");
        doc["relay_token"] = json!(format!("rhctl1-{}", "45".repeat(32)));
        let host = serde_json::to_vec(&doc).unwrap();
        let mut invitations = Invitations::default();
        let profile = PublicVideoProfile::parse_with_purpose(&host, NOW, true).unwrap();
        let good = invitations.select(vec![(None, profile)], NOW).unwrap();
        let room = Uuid::new_v4();
        doc["role"] = json!("viewer");
        doc["room"] = json!(room.to_string());
        doc["turn_username"] = json!(format!("{}:{room}", NOW + FIXTURE_TURN_TTL));
        for (name, bytes) in [
            ("round-02.json", serde_json::to_vec(&doc).unwrap()),
            ("round-02.json", host.clone()),
            ("nested/round-02.json", host.clone()),
            ("round-02.json", vec![0; MAX_FILE + 1]),
        ] {
            let buf = Cursor::new(Vec::new());
            let mut zip = zip::ZipWriter::new(buf);
            for (name, data) in [("round-01.json", &host), (name, &bytes)] {
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(data).unwrap();
            }
            let path = std::env::temp_dir().join(format!("{}.zip", Uuid::new_v4()));
            std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
            let result = load(&path, NOW);
            std::fs::remove_file(path).unwrap();
            assert_eq!(result.err(), Some("PUBLIC_INVITATION_ARCHIVE_INVALID"));
        }
        assert!(invitations.take(&good[0].id, Role::Host, NOW).is_ok());
    }

    #[cfg(feature = "v2-control-invitations")]
    #[test]
    fn invitation_selection_binds_snapshot_role_time_and_single_use_room() {
        let mut file = fixture();
        file["purpose"] = json!("public-control");
        file["mode"] = json!("relay_only");
        file["relay_token"] = json!(format!("rhctl1-{}", "45".repeat(32)));
        let profile = |doc: &Value| {
            PublicVideoProfile::parse_with_purpose(&serde_json::to_vec(doc).unwrap(), NOW, true)
                .unwrap()
        };
        let mut invitations = crate::control_invitations::Invitations::default();
        let stale = invitations
            .select(vec![(None, profile(&file))], NOW)
            .unwrap()
            .remove(0);
        let current = invitations
            .select(vec![(None, profile(&file))], NOW)
            .unwrap()
            .remove(0);
        assert!(invitations.take(&stale.id, Role::Host, NOW).is_err());
        assert!(invitations.take(&current.id, Role::Viewer, NOW).is_err());
        assert!(invitations
            .take(&current.id, Role::Host, NOW + FIXTURE_TURN_TTL)
            .is_err());
        let first = invitations
            .take(&current.id, Role::Host, NOW)
            .unwrap()
            .approval();
        assert!(invitations.take(&current.id, Role::Host, NOW).is_err());
        invitations
            .consume(first.room, first.scope.expires_at)
            .unwrap();
        assert!(invitations
            .select(vec![(None, profile(&file))], NOW)
            .is_err());
        file["room"] = json!(Uuid::new_v4().to_string());
        file["turn_username"] = json!(format!(
            "{}:{}",
            NOW + FIXTURE_TURN_TTL,
            file["room"].as_str().unwrap()
        ));
        let next = invitations
            .select(vec![(None, profile(&file))], NOW)
            .unwrap()
            .remove(0);
        assert_ne!(
            invitations
                .take(&next.id, Role::Host, NOW)
                .unwrap()
                .approval()
                .room,
            first.room
        );
    }

    #[cfg(feature = "v2-public-control-desktop")]
    #[test]
    fn purpose_and_bearer_cannot_be_reused_between_video_and_control() {
        let mut control = fixture();
        let video = serde_json::to_vec(&control).unwrap();
        assert!(PublicVideoProfile::parse_with_purpose(&video, NOW, true).is_err());

        control["purpose"] = json!("public-control");
        control["mode"] = json!("relay_only");
        control["relay_token"] = json!(format!("rhctl1-{}", "45".repeat(32)));
        let bytes = serde_json::to_vec(&control).unwrap();
        assert!(PublicVideoProfile::parse_with_purpose(&bytes, NOW, true).is_ok());
        assert!(PublicVideoProfile::parse(&bytes, NOW).is_err());

        control["mode"] = json!("direct_preferred");
        assert!(PublicVideoProfile::parse_with_purpose(
            &serde_json::to_vec(&control).unwrap(),
            NOW,
            true
        )
        .is_ok());
    }

    #[cfg(all(
        feature = "v2-public-control-desktop",
        not(feature = "v2-public-video-60s-test")
    ))]
    #[test]
    fn immediate_batch_profile_is_control_only_and_uses_one_shared_deadline() {
        let mut control = fixture();
        control["purpose"] = json!("public-control");
        control["mode"] = json!("relay_only");
        control["relay_token"] = json!(format!("rhctl1-{}", "45".repeat(32)));
        let parse = |doc: &Value, now| {
            PublicVideoProfile::parse_with_purpose(&serde_json::to_vec(doc).unwrap(), now, true)
        };
        assert!(!parse(&control, NOW).unwrap().is_control_batch());
        // The immediate transport invitation independently rechecks real
        // wall time; do not pass the synthetic 1970 fixture clock to it.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let expiry = now + MAX_IMMEDIATE_BATCH_TTL;
        control["expires_at"] = json!(expiry);
        control["turn_expires_at"] = json!(expiry);
        control["turn_username"] = json!(format!("{expiry}:{ROOM}"));
        assert!(parse(&control, now).is_err()); // v1 cannot widen its own lease
        control["version"] = json!(2);
        assert!(parse(&control, now).unwrap().is_control_batch());
        assert!(parse_fixture(&control, now).is_err());
        control["mode"] = json!("direct_preferred");
        assert!(parse(&control, now).unwrap().is_control_batch());
        for (key, value) in [
            ("mode", json!("host_only")),
            ("purpose", json!("public-video")),
            ("relay_token", json!("45".repeat(32))),
            ("expires_at", json!(expiry + 1)),
            ("turn_expires_at", json!(expiry - 1)),
        ] {
            let mut wrong = control.clone();
            wrong[key] = value;
            assert!(parse(&wrong, now).is_err(), "{key}");
        }
    }

    #[cfg(feature = "v2-session-window-sustained")]
    #[test]
    fn sustained_role_profile_requires_v3_exact_expiry_and_no_legacy_fallback() {
        let mut value = fixture();
        let scope = NOW + 2400;
        value["version"] = json!(3);
        value["purpose"] = json!("public-control");
        value["expires_at"] = json!(scope);
        value["turn_expires_at"] = json!(scope);
        value["turn_username"] = json!(format!("{scope}:{ROOM}"));
        value["relay_token"] = json!(format!("rhctl1-{}", "7a".repeat(32)));
        let read = |value: &Value, now, control| {
            PublicVideoProfile::parse_with_purpose(&serde_json::to_vec(value).unwrap(), now, control)
        };
        let good = read(&value, NOW, true).unwrap();
        assert!(good.is_sustained_control());
        assert!(!good.is_control_batch());
        assert!(read(&value, NOW, false).is_err());
        assert!(read(&value, NOW + 541, true).is_err());
        let mut old = value.clone();
        old["version"] = json!(2);
        assert!(read(&old, NOW, true).is_err());
        value["turn_expires_at"] = json!(scope - 1);
        assert!(read(&value, NOW, true).is_err());
    }

    #[test]
    fn minimum_remaining_lease_is_checked_without_widening_the_maximum() {
        assert_eq!(
            ensure_fresh(NOW + 600, NOW + MIN_TURN_TTL - 1, NOW),
            Err(EXPIRED)
        );
        assert_eq!(ensure_fresh(NOW + 600, NOW + MIN_TURN_TTL, NOW), Ok(()));
        assert_eq!(
            ensure_fresh(NOW + 601, NOW + MIN_TURN_TTL, NOW),
            Err(EXPIRED)
        );
        assert_eq!(
            ensure_fresh(NOW + MIN_TURN_TTL, NOW + MIN_TURN_TTL + 1, NOW),
            Err(EXPIRED)
        );
    }

    #[test]
    fn profile_path_refuses_streams_and_reserved_windows_devices() {
        for bad in [
            "profile.json:token",
            "profile.json.",
            "profile.json ",
            "NUL.json",
            "con .json",
            "COM¹",
            "lpt9.txt",
            "CONIN$",
        ] {
            assert!(!normal_path_name(bad), "{bad}");
        }
        assert!(normal_path_name("public.video.json"));
    }

    #[test]
    fn strict_profile_constructs_inert_literal_scoped_settings() {
        let profile = parse_fixture(&fixture(), NOW).unwrap();
        let approval = profile.approval();
        assert_eq!(approval.role, Role::Host);
        assert_eq!(approval.room.to_string(), ROOM);
        assert_eq!(approval.local_ipv4, Ipv4Addr::new(192, 168, 2, 3));
        assert_eq!(approval.scope.mode, IceMode::DirectPreferred);
        assert_eq!(approval.scope.relay_ports, (49152, 49183));
        assert_eq!(profile.expected_peer_fingerprint(), "f1".repeat(32));
        let config = profile.credentials.unwrap().ice_config;
        assert_eq!(config.session_id, ROOM);
        assert_eq!(
            config.ice_servers[0].urls,
            ["turn:93.184.216.34:3478?transport=udp"]
        );
    }

    #[test]
    fn malformed_profile_cannot_supply_a_different_route_or_expired_pin() {
        for (field, value, expected) in [
            ("version", json!(2), INVALID),
            ("uri", json!("turn:unapproved.invalid:3478"), INVALID),
            ("wss", json!("127.0.0.1:8443"), INVALID),
            ("turn", json!("93.184.216.35:3478"), INVALID),
            ("relay_ports", json!([49152, 50000]), INVALID),
            ("mode", json!("lan_pinned"), INVALID),
            ("ca_sha256", json!("12".repeat(32)), PIN),
            ("expected_peer_fingerprint", json!("AB".repeat(32)), INVALID),
            (
                "room",
                json!("00000000-0000-0000-0000-000000000000"),
                INVALID,
            ),
            ("wire_nonce", json!(ROOM), INVALID),
            ("session_nonce", json!("00".repeat(32)), INVALID),
            ("turn_username", json!("unexpected-username"), INVALID),
            ("turn_credential", json!("not-a-credential"), INVALID),
            ("turn_expires_at", json!(NOW + 59), EXPIRED),
        ] {
            let mut input = fixture();
            input[field] = value;
            assert_eq!(parse_fixture(&input, NOW).err(), Some(expected), "{field}");
        }
        assert_eq!(parse_fixture(&fixture(), NOW + 41).err(), Some(EXPIRED));
        assert_eq!(
            PublicVideoProfile::parse(&vec![0; MAX_FILE + 1], NOW).err(),
            Some(SIZE)
        );
    }
}

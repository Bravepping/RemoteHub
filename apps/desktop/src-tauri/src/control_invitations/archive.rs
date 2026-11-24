use crate::public_video_profile::{open_local_regular, PublicVideoProfile, MAX_FILE};
use hub_webrtc::public_video_wss::Role;
use serde::Deserialize;
use std::{
    collections::HashSet,
    io::{Read, Seek},
    path::Path,
};
use zeroize::Zeroizing;
use zip::{unstable::read::ZipIterable, ZipArchive};

const INVALID: &str = "PUBLIC_INVITATION_ARCHIVE_INVALID";
// Also accept the portable two-round kits, which include a client executable.
const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) type Rounds = Vec<(Option<u8>, PublicVideoProfile)>;

pub(crate) fn load(path: &Path, now: u64) -> Result<Rounds, &'static str> {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("zip") => {
            read(open_local_regular(path, MAX_ARCHIVE_BYTES)?, now)
        }
        Some(ext) if ext.eq_ignore_ascii_case("json") => {
            Ok(vec![(None, PublicVideoProfile::load_control(path, now)?)])
        }
        _ => Err(INVALID),
    }
}

#[derive(Deserialize)]
struct RoundIdentity {
    role: Role,
    room: String,
}

pub(super) fn read(mut reader: impl Read + Seek, now: u64) -> Result<Rounds, &'static str> {
    // Count before allocating the ZIP index. File contents are never extracted to disk.
    if !(1..=24).contains(
        &ZipIterable::try_new(&mut reader, Default::default())
            .map_err(|_| INVALID)?
            .len(),
    ) {
        return Err(INVALID);
    }
    reader.rewind().map_err(|_| INVALID)?;
    let mut zip = ZipArchive::new(reader).map_err(|_| INVALID)?;
    if zip.has_duplicate_names() || zip.has_overlapping_entries().map_err(|_| INVALID)? {
        return Err(INVALID);
    }
    let mut role = None;
    let mut rooms = HashSet::new();
    let mut rounds = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index_data(i).map_err(|_| INVALID)?;
        let name = entry.name().map_err(|_| INVALID)?;
        // Only these root files from the portable packaging workflow may accompany invitations.
        if matches!(
            name.as_ref(),
            "remotehub-desktop.exe"
                | "WebView2Loader.dll"
                | "使用说明.txt"
                | "第1轮.cmd"
                | "第2轮.cmd"
                | "SHA256SUMS.txt"
                | "THIRD_PARTY_NOTICES.md"
        ) {
            continue;
        }
        let digit = name
            .strip_prefix("round-0")
            .and_then(|s| s.strip_suffix(".json"))
            .ok_or(INVALID)?;
        let [digit @ b'1'..=b'8'] = digit.as_bytes() else {
            return Err(INVALID);
        };
        let number = digit - b'0';
        if entry.is_symlink() || entry.encrypted() || entry.size() > MAX_FILE as u64 {
            return Err(INVALID);
        }
        let expected = entry.size() as usize;
        let mut bytes = Zeroizing::new(Vec::with_capacity(expected));
        zip.by_index(i)
            .map_err(|_| INVALID)?
            .take(MAX_FILE as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| INVALID)?;
        if bytes.len() != expected {
            return Err(INVALID);
        }
        let identity: RoundIdentity = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
        if role
            .replace(identity.role)
            .is_some_and(|previous| previous != identity.role)
            || !rooms.insert(identity.room)
        {
            return Err(INVALID);
        }
        match PublicVideoProfile::parse_with_purpose(&bytes, now, true) {
            Ok(profile) => rounds.push((Some(number), profile)),
            Err("PUBLIC_VIDEO_PROFILE_EXPIRED") => continue,
            Err(error) => return Err(error),
        }
    }
    if rooms.is_empty() {
        return Err(INVALID);
    }
    if rounds.is_empty() {
        return Err("PUBLIC_VIDEO_PROFILE_EXPIRED");
    }
    rounds.sort_by_key(|(number, _)| *number);
    Ok(rounds)
}

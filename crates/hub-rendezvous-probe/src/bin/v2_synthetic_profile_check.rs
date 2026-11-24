//! Opt-in offline configuration check; opens no network or identity store.
//! Use a caller-controlled private local directory, e.g. per-user LocalAppData
//! on Windows. Parent checks reject present links, not concurrent parent
//! replacements; this tool does not rewrite or audit Windows ACLs.

use hub_identity::session::SessionRole;
use hub_webrtc::synthetic_role_profile::{check_format, MAX_PROFILE_BYTES};
use std::{
    ffi::OsStr,
    fs::{File, Metadata},
    io::Read,
    path::{Component, Path},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const USAGE: &str =
    "Usage: v2_synthetic_profile_check --check-profile --host|--viewer <private local file>";
const FILE_ERROR: &str = "SYNTHETIC_PROFILE_CHECK_FILE_REJECTED";
const PROFILE_ERROR: &str = "SYNTHETIC_PROFILE_CHECK_PROFILE_REJECTED";
const PASS: &str = "SYNTHETIC_PROFILE_FORMAT_SCOPE_PASS";

#[cfg(any(target_os = "linux", windows))]
fn check_parents(path: &Path, ordinary: impl Fn(&Metadata) -> bool) -> Option<()> {
    let parents: Vec<_> = path.parent()?.ancestors().collect();
    // Don't traverse a link or junction while checking its descendants.
    for parent in parents.into_iter().rev() {
        let meta = std::fs::symlink_metadata(parent).ok()?;
        if !meta.is_dir() || !ordinary(&meta) {
            return None;
        }
    }
    Some(())
}

#[cfg(target_os = "linux")]
fn open_profile(path: &Path) -> Option<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    if path.components().any(|part| part == Component::ParentDir) {
        return None;
    }
    let path = std::path::absolute(path).ok()?;
    check_parents(&path, |meta| !meta.file_type().is_symlink())?;
    let file = File::options()
        .read(true)
        // Check the opened object, but never follow a leaf link or block on
        // a FIFO switched into place before the open.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    // SAFETY: This takes no pointers and reads this process's effective UID.
    let owner = unsafe { libc::geteuid() };
    if !meta.is_file() || meta.uid() != owner || meta.mode() & 0o077 != 0 {
        return None;
    }
    Some(file)
}

#[cfg(windows)]
fn open_profile(path: &Path) -> Option<File> {
    use std::{
        os::windows::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawHandle,
        },
        path::Prefix,
    };
    use windows_sys::Win32::{
        Storage::FileSystem::{
            GetDriveTypeW, GetFileType, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_TYPE_DISK,
        },
        System::WindowsProgramming::DRIVE_FIXED,
    };

    let ordinary_path = |path: &Path| {
        path.components().all(|part| match part {
            Component::Prefix(prefix) => matches!(prefix.kind(), Prefix::Disk(_)),
            Component::Normal(name) => !name.encode_wide().any(|byte| byte == u16::from(b':')),
            Component::ParentDir => false,
            _ => true,
        })
    };
    // Reject explicit UNC, device/verbatim namespaces, and alternate streams
    // before opening. Only an ordinary fixed-drive path reaches the read.
    if !ordinary_path(path) {
        return None;
    }
    let path = std::path::absolute(path).ok()?;
    if !ordinary_path(&path) {
        return None;
    }
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return None;
    };
    let Prefix::Disk(letter) = prefix.kind() else {
        return None;
    };
    let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
    // SAFETY: root is a live, NUL-terminated Windows drive root string.
    if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
        return None;
    }
    check_parents(&path, |meta| {
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
    })?;

    let file = File::options()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    // SAFETY: The non-owning handle remains live throughout this call.
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
        return None;
    }
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return None;
    }
    Some(file)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn open_profile(_: &Path) -> Option<File> {
    None
}

fn read_profile(path: &Path) -> Option<Zeroizing<Vec<u8>>> {
    let file = open_profile(path)?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || !(1..=MAX_PROFILE_BYTES as u64).contains(&meta.len()) {
        return None;
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_PROFILE_BYTES + 1));
    file.take((MAX_PROFILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    (!bytes.is_empty() && bytes.len() <= MAX_PROFILE_BYTES).then_some(bytes)
}

fn check(role: SessionRole, path: &Path) -> Result<(), &'static str> {
    let bytes = read_profile(path).ok_or(FILE_ERROR)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PROFILE_ERROR)?
        .as_secs();
    // Do not import native identities, pins, or OS settings. Untrusted file
    // fields and parser details are never printed, even on failure.
    check_format(&bytes, role, now).map_err(|_| PROFILE_ERROR)
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let [Some(command), Some(raw_role), Some(path), None] =
        [args.next(), args.next(), args.next(), args.next()]
    else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let role = match (command.as_os_str(), raw_role.as_os_str()) {
        (cmd, role) if cmd == OsStr::new("--check-profile") && role == OsStr::new("--host") => {
            SessionRole::Host
        }
        (cmd, role) if cmd == OsStr::new("--check-profile") && role == OsStr::new("--viewer") => {
            SessionRole::Viewer
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match check(role, Path::new(&path)) {
        Ok(()) => {
            println!("{PASS}");
            ExitCode::SUCCESS
        }
        Err(code) => {
            eprintln!("{code}");
            ExitCode::FAILURE
        }
    }
}

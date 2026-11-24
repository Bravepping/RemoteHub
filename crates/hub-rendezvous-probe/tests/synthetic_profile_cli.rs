//! Runs the standalone offline checker on temporary synthetic fixtures only.
#![cfg(any(target_os = "linux", windows))]

use hub_webrtc::synthetic_role_profile::MAX_PROFILE_BYTES;
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};
use ring::digest::{digest, SHA256};
use serde_json::{json, Value};
use std::{
    fs::{DirBuilder, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const EXE: &str = env!("CARGO_BIN_EXE_v2_synthetic_profile_check");
const PASS: &[u8] = b"SYNTHETIC_PROFILE_FORMAT_SCOPE_PASS\n";
const PROFILE_ERROR: &[u8] = b"SYNTHETIC_PROFILE_CHECK_PROFILE_REJECTED\n";
const FILE_ERROR: &[u8] = b"SYNTHETIC_PROFILE_CHECK_FILE_REJECTED\n";

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("rh-profile-fixture-{}", Uuid::new_v4().simple()));
        let mut builder = DirBuilder::new();
        builder.recursive(false);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        let mut options = File::options();
        options.write(true).create_new(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path).unwrap().write_all(contents).unwrap();
        path
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(role: &str, path: &Path) -> Output {
    Command::new(EXE)
        .args(["--check-profile", role])
        .arg(path)
        .output()
        .unwrap()
}

fn assert_rejected(output: Output, error: &[u8]) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, error);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn profile(role: &str) -> Value {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = params.self_signed(&KeyPair::generate().unwrap()).unwrap();
    json!({
        "version": 1,
        "mode": "synthetic-turn-only",
        "role": role,
        "room": Uuid::new_v4().simple().to_string(),
        "generation": "13".repeat(32),
        "expires_at": now() + 90,
        // Shape-only labels; this executable must never resolve or dial them.
        "wss": "93.184.216.34:8443",
        "turn": "93.184.216.34:3478",
        "relay_ports": [49152, 49153],
        "ca_der_hex": hex(ca.der().as_ref()),
        "ca_sha256": hex(digest(&SHA256, ca.der().as_ref()).as_ref()),
        "local_fingerprint": "ab".repeat(32),
        "peer_fingerprint": "cd".repeat(32),
    })
}

#[test]
fn exact_roles_validate_without_printing_profile_values() {
    let tmp = FixtureDir::new();
    for (role, flag, wrong_flag) in [
        ("host", "--host", "--viewer"),
        ("viewer", "--viewer", "--host"),
    ] {
        let content = serde_json::to_vec(&profile(role)).unwrap();
        let path = tmp.write(&format!("{role} profile.json"), &content);
        let output = run(flag, &path);
        assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
        assert_eq!(output.stdout, PASS);
        assert!(output.stderr.is_empty());
        assert_rejected(run(wrong_flag, &path), PROFILE_ERROR);
    }
}

#[test]
fn refuses_unknown_arguments_or_fields_expiry_and_invalid_files() {
    let tmp = FixtureDir::new();
    let mut expired = profile("host");
    expired["expires_at"] = json!(now().saturating_sub(1));
    assert_rejected(
        run(
            "--host",
            &tmp.write("expired.json", &serde_json::to_vec(&expired).unwrap()),
        ),
        PROFILE_ERROR,
    );
    let mut unexpected = profile("host");
    unexpected["unexpected_fixture_secret"] = json!("must-never-be-printed");
    assert_rejected(
        run(
            "--host",
            &tmp.write("unknown.json", &serde_json::to_vec(&unexpected).unwrap()),
        ),
        PROFILE_ERROR,
    );
    let oversized = tmp.write("oversized.json", &vec![b'x'; MAX_PROFILE_BYTES + 1]);
    assert_rejected(run("--host", &oversized), FILE_ERROR);
    assert_rejected(run("--host", &tmp.0), FILE_ERROR);
    assert_rejected(run("--host", &tmp.0.join("missing.json")), FILE_ERROR);

    let output = Command::new(EXE)
        .args(["--check-profile", "--host"])
        .arg(&oversized)
        .arg("--extra")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"Usage: v2_synthetic_profile_check --check-profile --host|--viewer <private local file>\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn refuses_symlink_nonprivate_regular_file_and_fifo_without_blocking() {
    use std::{
        ffi::CString,
        os::unix::{
            ffi::OsStrExt,
            fs::{symlink, PermissionsExt},
        },
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };

    let tmp = FixtureDir::new();
    let path = tmp.write(
        "private.json",
        &serde_json::to_vec(&profile("host")).unwrap(),
    );
    let link = tmp.0.join("symlink.json");
    symlink(&path, &link).unwrap();
    assert_rejected(run("--host", &link), FILE_ERROR);
    let parent_link = tmp.0.join("linked-directory");
    symlink(&tmp.0, &parent_link).unwrap();
    assert_rejected(run("--host", &parent_link.join("private.json")), FILE_ERROR);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_rejected(run("--host", &path), FILE_ERROR);

    let fifo = tmp.0.join("fifo");
    let path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: path points to a live, NUL-terminated temporary fixture path.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let mut child = Command::new(EXE)
        .args(["--check-profile", "--host"])
        .arg(fifo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("checker must not block opening a FIFO");
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_rejected(child.wait_with_output().unwrap(), FILE_ERROR);
}

#[cfg(windows)]
#[test]
fn refuses_windows_device_namespace() {
    assert_rejected(run("--host", Path::new(r"\\.\NUL")), FILE_ERROR);
}

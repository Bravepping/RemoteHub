//! Offline short-lived private-LAN WSS test profiles; no listeners or launches.
#[path = "v2_lan_test_setup/private_output.rs"]
mod private_output;

use private_output::PrivateOutput;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use ring::{
    digest::{digest, SHA256},
    rand::{SecureRandom, SystemRandom},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Component, Path, PathBuf},
};
use time::{Duration, OffsetDateTime};
use zeroize::Zeroizing;

const PORT: u16 = 48620;
const USAGE: &str = "usage: v2_lan_test_setup <HOST_IPV4> <VIEWER_IPV4> <NEW_ABSOLUTE_OUTPUT_DIR>";
type SetupResult<T> = Result<T, &'static str>;

struct Options {
    host: Ipv4Addr,
    viewer: Ipv4Addr,
    output: PathBuf,
}

fn lan_ip(value: &OsString) -> SetupResult<Ipv4Addr> {
    let ip: Ipv4Addr = value
        .to_str()
        .and_then(|text| text.parse().ok())
        .ok_or("V2_SETUP_PRIVATE_IPV4_REQUIRED")?;
    if ip.is_private() && !matches!(ip.octets()[3], 0 | 255) {
        Ok(ip)
    } else {
        Err("V2_SETUP_PRIVATE_IPV4_REQUIRED")
    }
}

fn options(args: impl IntoIterator<Item = OsString>) -> SetupResult<Options> {
    let mut args = args.into_iter();
    let host = lan_ip(&args.next().ok_or(USAGE)?)?;
    let viewer = lan_ip(&args.next().ok_or(USAGE)?)?;
    let output = PathBuf::from(args.next().ok_or(USAGE)?);
    if args.next().is_some() {
        return Err(USAGE);
    }
    if host == viewer {
        return Err("V2_SETUP_DISTINCT_PEERS_REQUIRED");
    }
    if !output.is_absolute()
        || output.file_name().is_none()
        || output.to_str().is_none()
        || output
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        || !private_output::local_path(&output)
    {
        return Err("V2_SETUP_LOCAL_ABSOLUTE_OUTPUT_REQUIRED");
    }
    Ok(Options {
        host,
        viewer,
        output,
    })
}

fn hex(bytes: &[u8]) -> String {
    const LOWER: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(char::from(LOWER[(byte >> 4) as usize]));
        result.push(char::from(LOWER[(byte & 15) as usize]));
    }
    result
}

fn token(rng: &dyn SecureRandom) -> SetupResult<Zeroizing<String>> {
    let mut value = Zeroizing::new([0u8; 32]);
    rng.fill(&mut *value)
        .map_err(|_| "V2_SETUP_RANDOM_FAILED")?;
    Ok(Zeroizing::new(hex(&*value)))
}

struct TlsBundle {
    ca_der: Vec<u8>,
    cert_pem: String,
    key_pem: Zeroizing<String>,
}

fn certificates(host: Ipv4Addr, now: OffsetDateTime) -> SetupResult<TlsBundle> {
    fn generate(host: Ipv4Addr, now: OffsetDateTime) -> Result<TlsBundle, rcgen::Error> {
        // Allow a small clock skew but prevent new handshakes after the test hour.
        let not_before = now - Duration::minutes(5);
        let not_after = now + Duration::hours(1);
        let mut ca = CertificateParams::default();
        ca.not_before = not_before;
        ca.not_after = not_after;
        ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let issuer = CertifiedIssuer::self_signed(ca, KeyPair::generate()?)?;

        let mut leaf = CertificateParams::new(vec![host.to_string()])?;
        leaf.not_before = not_before;
        leaf.not_after = not_after;
        leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate()?;
        let cert = leaf.signed_by(&key, &issuer)?;

        // No CA private key serialization: drop the in-memory issuer here.
        Ok(TlsBundle {
            ca_der: issuer.der().to_vec(),
            cert_pem: cert.pem(),
            key_pem: Zeroizing::new(key.serialize_pem()),
        })
    }
    generate(host, now).map_err(|_| "V2_SETUP_CERTIFICATE_FAILED")
}

fn profile(values: BTreeMap<&str, &str>) -> SetupResult<Zeroizing<Vec<u8>>> {
    serde_json::to_vec_pretty(&values)
        .map(Zeroizing::new)
        .map_err(|_| "V2_SETUP_PROFILE_FAILED")
}

fn desktop_profile<'a>(
    relay: &'a str,
    ca_der_hex: &'a str,
    token: &'a str,
) -> BTreeMap<&'static str, &'a str> {
    BTreeMap::from([
        ("REMOTEHUB_V2_DESKTOP_RELAY", relay),
        ("REMOTEHUB_V2_DESKTOP_CA_DER_HEX", ca_der_hex),
        ("REMOTEHUB_V2_DESKTOP_TOKEN", token),
    ])
}

fn path_text(path: &Path) -> SetupResult<&str> {
    path.to_str()
        .ok_or("V2_SETUP_LOCAL_ABSOLUTE_OUTPUT_REQUIRED")
}

fn setup(options: &Options, now: OffsetDateTime) -> SetupResult<String> {
    let fs = PrivateOutput::new(&options.output)?;
    let root = fs.root();
    let host_dir = root.join("host");
    let viewer_dir = root.join("viewer");
    let cert_path = host_dir.join("relay-cert.pem");
    let key_path = host_dir.join("relay-key.pem");

    let tls = certificates(options.host, now)?;
    let rng = SystemRandom::new();
    let host_token = token(&rng)?;
    let viewer_token = token(&rng)?;
    if host_token.as_str() == viewer_token.as_str() {
        // Do not output the kit even on a theoretically improbable collision.
        return Err("V2_SETUP_RANDOM_FAILED");
    }
    let ca_der_hex = hex(&tls.ca_der);
    let ca_fingerprint = hex(digest(&SHA256, &tls.ca_der).as_ref());
    let relay = SocketAddrV4::new(options.host, PORT).to_string();
    let host = options.host.to_string();
    let viewer = options.viewer.to_string();

    let mut host_profile = desktop_profile(&relay, &ca_der_hex, &host_token);
    host_profile.extend([
        ("REMOTEHUB_V2_MODE", "private-lan"),
        ("REMOTEHUB_V2_BIND", relay.as_str()),
        ("REMOTEHUB_V2_TLS_CERT", path_text(&cert_path)?),
        ("REMOTEHUB_V2_TLS_KEY", path_text(&key_path)?),
        ("REMOTEHUB_V2_LAN_HOST_IP", host.as_str()),
        ("REMOTEHUB_V2_LAN_VIEWER_IP", viewer.as_str()),
        ("REMOTEHUB_V2_LAN_HOST_TOKEN", host_token.as_str()),
        ("REMOTEHUB_V2_LAN_VIEWER_TOKEN", viewer_token.as_str()),
    ]);
    let host_profile = profile(host_profile)?;
    let viewer_profile = profile(desktop_profile(&relay, &ca_der_hex, &viewer_token))?;

    // All writes are new only; write profiles last so they cannot refer to a
    // missing key after a partial filesystem failure.
    let _root = fs.create_directory(root)?;
    let _host = fs.create_directory(&host_dir)?;
    let _viewer = fs.create_directory(&viewer_dir)?;
    fs.write_file(&cert_path, tls.cert_pem.as_bytes())?;
    fs.write_file(&key_path, tls.key_pem.as_bytes())?;
    fs.write_file(&host_dir.join("host.json"), &host_profile)?;
    fs.write_file(&viewer_dir.join("viewer.json"), &viewer_profile)?;
    Ok(ca_fingerprint)
}

fn main() {
    let result = options(std::env::args_os().skip(1))
        .and_then(|options| setup(&options, OffsetDateTime::now_utc()));
    match result {
        Ok(fingerprint) => {
            println!("V2_LAN_TEST_KIT_READY");
            println!("CA_SHA256={fingerprint}");
        }
        Err(code) => {
            eprintln!("{code}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, fs};

    #[cfg(windows)]
    fn fixture_parent() -> PathBuf {
        private_output::known_local_app_data().unwrap()
    }

    #[cfg(not(windows))]
    fn fixture_parent() -> PathBuf {
        std::env::temp_dir()
    }

    fn args(host: &str, viewer: &str, root: &Path) -> [OsString; 3] {
        [host.into(), viewer.into(), root.as_os_str().to_owned()]
    }

    #[test]
    fn credentials_require_exact_private_peer_ips_and_new_local_destination() {
        let root = fixture_parent().join("unused-test-destination");
        for host in [
            "0.0.0.0",
            "127.0.0.1",
            "100.64.1.1",
            "10.2.3.0",
            "10.2.3.255",
            "8.8.8.8",
            "192.168.2.5:456",
            "host.example",
            "::1",
        ] {
            assert!(options(args(host, "192.168.2.5", &root)).is_err());
        }
        assert!(options(args("192.168.2.5", "192.168.2.5", &root)).is_err());
        assert!(options(args("10.0.0.1", "10.0.0.2", Path::new("relative"))).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_a_writable_parent_other_than_the_known_folder() {
        let parent = fixture_parent().join(format!("remotehub-nonroot-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&parent).unwrap();
        let root = parent.join("kit");
        let option = options(args("192.168.2.4", "192.168.2.5", &root)).unwrap();
        assert_eq!(
            setup(&option, OffsetDateTime::now_utc()),
            Err("V2_SETUP_LOCALAPPDATA_CHILD_REQUIRED")
        );
        assert!(!root.exists());
        fs::remove_dir(&parent).unwrap();
    }

    #[test]
    fn synthetic_certificate_checks_exact_host_san_and_expires() {
        use rustls::{
            client::{danger::ServerCertVerifier, WebPkiServerVerifier},
            pki_types::{pem::PemObject, CertificateDer, ServerName, UnixTime},
            RootCertStore,
        };
        use std::sync::Arc;

        let now = OffsetDateTime::now_utc();
        let host = Ipv4Addr::new(192, 168, 2, 4);
        let other = Ipv4Addr::new(192, 168, 2, 5);
        let tls = certificates(host, now).unwrap();
        let leaf = CertificateDer::from_pem_slice(tls.cert_pem.as_bytes()).unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(tls.ca_der)).unwrap();
        let verifier = WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .unwrap();
        let at = |timestamp: OffsetDateTime| {
            UnixTime::since_unix_epoch(std::time::Duration::from_secs(
                timestamp.unix_timestamp().try_into().unwrap(),
            ))
        };
        let check = |ip: Ipv4Addr, time| {
            verifier.verify_server_cert(
                &leaf,
                &[],
                &ServerName::IpAddress(ip.into()),
                &[],
                at(time),
            )
        };
        assert!(check(host, now).is_ok());
        assert!(check(other, now).is_err());
        assert!(check(host, now + Duration::hours(1) + Duration::seconds(1)).is_err());
    }

    #[test]
    fn isolated_fixture_keeps_roles_and_private_files_separate_and_never_overwrites() {
        // Temp-only synthetic credentials; never touch a user path or start a socket.
        let root = fixture_parent().join(format!("remotehub-kit 测试 = {}", uuid::Uuid::new_v4()));
        let option = options(args("192.168.2.4", "192.168.2.5", &root)).unwrap();
        let fingerprint = setup(&option, OffsetDateTime::now_utc()).unwrap();
        let host_path = root.join("host/host.json");
        let viewer_path = root.join("viewer/viewer.json");
        let host_bytes = fs::read(&host_path).unwrap();
        let viewer_bytes = fs::read(&viewer_path).unwrap();
        let host: BTreeMap<String, String> = serde_json::from_slice(&host_bytes).unwrap();
        let viewer: BTreeMap<String, String> = serde_json::from_slice(&viewer_bytes).unwrap();
        assert_eq!(host.len(), 11);
        assert_eq!(viewer.len(), 3);
        assert!(viewer
            .keys()
            .all(|key| key.starts_with("REMOTEHUB_V2_DESKTOP_")));
        for (name, expected) in [
            ("REMOTEHUB_V2_TLS_CERT", root.join("host/relay-cert.pem")),
            ("REMOTEHUB_V2_TLS_KEY", root.join("host/relay-key.pem")),
        ] {
            assert_eq!(OsStr::new(&host[name]), expected.as_os_str());
        }
        let ca_hex = "REMOTEHUB_V2_DESKTOP_CA_DER_HEX";
        assert!(host[ca_hex] == viewer[ca_hex]);
        let ca_der = host[ca_hex]
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|digits| u8::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(fingerprint, hex(digest(&SHA256, &ca_der).as_ref()));
        let token = "REMOTEHUB_V2_DESKTOP_TOKEN";
        assert!(host[token] != viewer[token]);
        assert!(host[token] == host["REMOTEHUB_V2_LAN_HOST_TOKEN"]);
        assert!(viewer[token] == host["REMOTEHUB_V2_LAN_VIEWER_TOKEN"]);
        assert_eq!(host[token].len(), 64);
        assert_eq!(viewer[token].len(), 64);
        assert_eq!(fs::read_dir(root.join("host")).unwrap().count(), 3);
        assert_eq!(fs::read_dir(root.join("viewer")).unwrap().count(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&root, &root.join("host"), &root.join("viewer")] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
            for path in [&host_path, &viewer_path, &root.join("host/relay-key.pem")] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        assert_eq!(
            setup(&option, OffsetDateTime::now_utc()),
            Err("V2_SETUP_OUTPUT_EXISTS")
        );
        assert!(fs::read(&host_path).unwrap() == host_bytes);
        assert!(fs::read(&viewer_path).unwrap() == viewer_bytes);
        fs::remove_dir_all(root).unwrap();
    }
}

//! Explicit public-control-only one-listener admission. Batch files contain
//! short-lived role bearers, never identities or an input grant.
use super::*;
use serde::Deserialize;
use std::{collections::HashSet, io::Read};

const MAX_ROOMS: usize = 8;
const MAX_BATCH_BYTES: usize = 16 * 1024;
pub(super) const MAX_LIFETIME: Duration = Duration::from_secs(1800);
const BATCH_HEADER: &str = "x-rh-public-control-batch";
const IMMEDIATE_V1: &str = "immediate-v1";
const INVALID: &str = "V2_PUBLIC_CONTROL_BATCH_INVALID";
const FILE: &str = "V2_PUBLIC_CONTROL_BATCH_FILE_REJECTED";

pub(super) fn request_purpose(headers: &HeaderMap) -> Result<bool, &'static str> {
    let mut values = headers.get_all(BATCH_HEADER).iter();
    match (values.next(), values.next()) {
        (None, None) => Ok(false),
        (Some(value), None) if value == IMMEDIATE_V1 => Ok(true),
        _ => Err(INVALID),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config<'a> {
    version: u8,
    expires_at: u64,
    host_ipv4: &'a str,
    viewer_ipv4: &'a str,
    #[serde(borrow)]
    rooms: Vec<Invitation<'a>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invitation<'a> {
    room: &'a str,
    not_before: u64,
    expires_at: u64,
    host_token: &'a str,
    viewer_token: &'a str,
}

pub(super) struct Batch {
    expires_at: u64,
    deadline: Instant,
    rooms: Vec<Arc<PublicVideoApp>>,
}

impl Batch {
    pub(super) fn load(path: &Path) -> Result<Self, &'static str> {
        let file = open_private(path)?;
        let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_BATCH_BYTES + 1));
        file.take((MAX_BATCH_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| FILE)?;
        Self::parse(&bytes, now()?, Instant::now())
    }

    fn parse(bytes: &[u8], wall: u64, mono: Instant) -> Result<Self, &'static str> {
        if bytes.is_empty() || bytes.len() > MAX_BATCH_BYTES {
            return Err(INVALID);
        }
        let config: Config<'_> = serde_json::from_slice(bytes).map_err(|_| INVALID)?;
        let lifetime = config.expires_at.checked_sub(wall).ok_or(INVALID)?;
        if !(1..=MAX_LIFETIME.as_secs()).contains(&lifetime) {
            return Err(INVALID);
        }
        let immediate_start = match (config.version, config.rooms.as_slice()) {
            (1, rooms) if (1..=MAX_ROOMS).contains(&rooms.len()) => None,
            (2, rooms) if rooms.len() == MAX_ROOMS => Some(rooms[0].not_before),
            _ => return Err(INVALID),
        };
        let sources = PeerSources {
            host: source(config.host_ipv4)?,
            viewer: source(config.viewer_ipv4)?,
        };
        let purpose = Purpose::Control;
        let mut ids = HashSet::with_capacity(config.rooms.len());
        let mut bearers = HashSet::with_capacity(config.rooms.len() * 2);
        let mut rooms = Vec::with_capacity(config.rooms.len());
        // Existing listener-wide resource bound, not a fresh quota per room.
        let active: Arc<AtomicUsize> = Arc::default();
        for invitation in config.rooms {
            let id = Uuid::try_parse(invitation.room).map_err(|_| INVALID)?;
            let window = invitation
                .expires_at
                .checked_sub(invitation.not_before)
                .ok_or(INVALID)?;
            let valid_window = match immediate_start {
                Some(start) => {
                    start <= wall
                        && invitation.not_before == start
                        && invitation.expires_at == config.expires_at
                        && (1..=MAX_LIFETIME.as_secs()).contains(&window)
                }
                None => (1..=ROOM_LIFETIME.as_secs()).contains(&window),
            };
            if id.get_version() != Some(uuid::Version::Random)
                || id.get_variant() != uuid::Variant::RFC4122
                || id.simple().to_string() != invitation.room
                || !valid_window
                || invitation.expires_at <= wall
                || invitation.expires_at > config.expires_at
                || !ids.insert(id)
                || !bearers.insert(invitation.host_token)
                || !bearers.insert(invitation.viewer_token)
            {
                return Err(INVALID);
            }
            PublicVideoApproval::check_invitation(
                purpose,
                id,
                invitation.host_token,
                invitation.viewer_token,
            )
            .map_err(|_| INVALID)?;
            let start = mono + Duration::from_secs(invitation.not_before.saturating_sub(wall));
            let mut app = PublicVideoApp::new(PublicVideoApproval {
                purpose,
                room: id,
                sources,
                host_token: Zeroizing::new(invitation.host_token.to_owned()),
                viewer_token: Zeroizing::new(invitation.viewer_token.to_owned()),
                expires_at: invitation.expires_at,
                deadline: mono + Duration::from_secs(invitation.expires_at - wall),
                not_before: Some((invitation.not_before, start)),
                immediate_batch: immediate_start.is_some(),
            });
            app.active = active.clone();
            rooms.push(Arc::new(app));
        }
        Ok(Self {
            expires_at: config.expires_at,
            deadline: mono + Duration::from_secs(lifetime),
            rooms,
        })
    }

    pub(super) async fn serve(
        self,
        addr: SocketAddr,
        tls: axum_server::tls_rustls::RustlsConfig,
    ) -> Result<(), &'static str> {
        if Instant::now() >= self.deadline || now()? >= self.expires_at {
            return Err(INVALID);
        }
        let service = router_with_rooms(Purpose::Control, self.rooms)
            .into_make_service_with_connect_info::<SocketAddr>();
        let serving = axum_server::bind_rustls(addr, tls).serve(service);
        tokio::select! {
            biased;
            result = serving => result.map_err(|_| "V2_SERVER_FAILED"),
            // Cancellation closes this listener. A backward wall-clock jump
            // can never extend its original monotonic limit.
            _ = wait_for_expiry(self.expires_at, self.deadline) => Ok(()),
        }
    }
}

async fn wait_for_expiry(expires_at: u64, deadline: Instant) {
    let mut wall_check = tokio::time::interval(Duration::from_millis(250));
    loop {
        if !now().is_ok_and(|wall| wall < expires_at) {
            return;
        }
        tokio::select! {
            _ = sleep_until(deadline) => return,
            _ = wall_check.tick() => {},
        }
    }
}

fn source(text: &str) -> Result<Ipv4Addr, &'static str> {
    let ip = text.parse::<Ipv4Addr>().map_err(|_| INVALID)?;
    if ip.to_string() != text || !public_unicast(ip) {
        return Err(INVALID);
    }
    Ok(ip)
}

#[cfg(target_os = "linux")]
fn open_private(path: &Path) -> Result<std::fs::File, &'static str> {
    use std::{
        ffi::CString,
        fs::File,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::Component,
    };
    let mut parts = path.components().peekable();
    if parts.next() != Some(Component::RootDir) || parts.peek().is_none() {
        return Err(FILE);
    }
    let mut dir = File::open("/").map_err(|_| FILE)?;
    // SAFETY: no parameters; reads only this process's effective UID.
    let uid = unsafe { libc::geteuid() };
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(FILE);
        };
        let name = CString::new(name.as_bytes()).map_err(|_| FILE)?;
        let leaf = parts.peek().is_none();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if leaf {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        // SAFETY: valid directory fd and NUL-terminated component; no
        // creation flags are set. Each opened directory is pinned by fd.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(FILE);
        }
        // SAFETY: successful openat returns a new owned fd.
        let opened = unsafe { File::from_raw_fd(fd) };
        let meta = opened.metadata().map_err(|_| FILE)?;
        if leaf {
            if !meta.is_file()
                || meta.uid() != uid
                || meta.mode() & 0o077 != 0
                || !(1..=MAX_BATCH_BYTES as u64).contains(&meta.len())
            {
                return Err(FILE);
            }
            return Ok(opened);
        }
        // Protect private descendants against replacement by unrelated
        // users; a root-owned sticky /tmp is OK for per-user test folders.
        if !meta.is_dir()
            || !(meta.uid() == 0 || meta.uid() == uid)
            || (meta.mode() & 0o022 != 0 && !(meta.uid() == 0 && meta.mode() & 0o1000 != 0))
        {
            return Err(FILE);
        }
        dir = opened;
    }
    Err(FILE)
}

#[cfg(not(target_os = "linux"))]
fn open_private(_: &Path) -> Result<std::fs::File, &'static str> {
    // Never fall back to a less restrictive path-open implementation.
    Err(FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const WALL: u64 = 50_000;
    const HOST: &str = "93.184.216.34";
    const VIEWER: &str = "93.184.216.35";

    fn fixture(wall: u64, offsets: &[u64]) -> Value {
        let rooms: Vec<_> = offsets
            .iter()
            .enumerate()
            .map(|(index, offset)| {
                let host = index * 2 + 1;
                let viewer = index * 2 + 2;
                json!({
                    "room": Uuid::new_v4().simple().to_string(),
                    "not_before": wall + offset,
                    "expires_at": wall + offset + 600,
                    "host_token": format!("rhctl1-{host:064x}"),
                    "viewer_token": format!("rhctl1-{viewer:064x}"),
                })
            })
            .collect();
        json!({
            "version": 1, "expires_at": wall + 1800,
            "host_ipv4": HOST, "viewer_ipv4": VIEWER, "rooms": rooms,
        })
    }

    fn parse(value: &Value, wall: u64, mono: Instant) -> Result<Batch, &'static str> {
        Batch::parse(&serde_json::to_vec(value).unwrap(), wall, mono)
    }

    #[test]
    fn v2_makes_exactly_eight_independent_rooms_immediately_available() {
        let start = Instant::now();
        let mut doc = fixture(WALL, &[0; 8]);
        doc["version"] = json!(2);
        for room in doc["rooms"].as_array_mut().unwrap() {
            room["expires_at"] = json!(WALL + 1800);
        }
        let batch = parse(&doc, WALL, start).unwrap();
        assert_eq!(batch.rooms.len(), 8);
        for app in &batch.rooms {
            assert!(app.approval.immediate_batch);
            assert_eq!(
                app.approval.deadline_at(WALL, start),
                Ok(start + Duration::from_secs(1800))
            );
            assert!(app.approval.deadline_at(WALL - 1, start).is_err());
            assert!(app.approval.deadline_at(WALL + 1800, start).is_err());
        }
        let mut wrong = doc.clone();
        wrong["version"] = json!(1);
        assert!(parse(&wrong, WALL, start).is_err());
        let mut wrong = doc.clone();
        wrong["rooms"].as_array_mut().unwrap().pop();
        assert!(parse(&wrong, WALL, start).is_err());
        for (field, value) in [
            ("expires_at", json!(WALL + 1799)),
            ("not_before", json!(WALL + 1)),
            ("not_before", json!(WALL - 1)),
            ("host_token", doc["rooms"][1]["viewer_token"].clone()),
        ] {
            let mut wrong = doc.clone();
            wrong["rooms"][0][field] = value;
            assert!(parse(&wrong, WALL, start).is_err(), "{field}");
        }
    }

    #[test]
    fn immediate_purpose_header_is_exact_and_not_repeatable() {
        let mut headers = HeaderMap::new();
        assert_eq!(request_purpose(&headers), Ok(false));
        headers.insert(BATCH_HEADER, IMMEDIATE_V1.parse().unwrap());
        assert_eq!(request_purpose(&headers), Ok(true));
        headers.append(BATCH_HEADER, IMMEDIATE_V1.parse().unwrap());
        assert!(request_purpose(&headers).is_err());
        headers.insert(BATCH_HEADER, "immediate-v2".parse().unwrap());
        assert!(request_purpose(&headers).is_err());
    }

    #[test]
    fn bounded_staggered_windows_bind_both_clocks_and_exact_deadlines() {
        let start = Instant::now();
        let batch = parse(
            &fixture(WALL, &[0, 0, 0, 0, 450, 450, 900, 1200]),
            WALL,
            start,
        )
        .unwrap();
        assert_eq!(batch.rooms.len(), 8);
        assert_eq!(batch.deadline, start + Duration::from_secs(1800));
        let first = &batch.rooms[0].approval;
        let later = &batch.rooms[4].approval;
        assert_eq!(
            first.deadline_at(WALL, start),
            Ok(start + Duration::from_secs(600))
        );
        assert!(first
            .deadline_at(WALL + 600, start + Duration::from_secs(599))
            .is_err());
        assert!(first
            .deadline_at(WALL + 599, start + Duration::from_secs(600))
            .is_err());
        assert!(later
            .deadline_at(WALL + 449, start + Duration::from_secs(450))
            .is_err());
        assert!(later
            .deadline_at(WALL + 450, start + Duration::from_secs(449))
            .is_err());
        assert_eq!(
            later.deadline_at(WALL + 450, start + Duration::from_secs(450)),
            Ok(start + Duration::from_secs(1050))
        );
    }

    #[test]
    fn rejects_duplicates_bad_windows_excess_counts_and_untrusted_sources() {
        let good = fixture(WALL, &[0, 1200]);
        let now = Instant::now();
        for (key, val) in [
            ("version", json!(2)),
            ("expires_at", json!(WALL + 1801)),
            ("expires_at", json!(WALL - 1)),
            ("host_ipv4", json!("127.0.0.1")),
            ("viewer_ipv4", json!("192.168.1.1")),
            ("rooms", json!([])),
            ("unknown", json!(true)),
        ] {
            let mut bad = good.clone();
            bad[key] = val;
            assert!(parse(&bad, WALL, now).is_err(), "{key}");
        }
        for (slot, key, val) in [
            (1, "room", good["rooms"][0]["room"].clone()),
            (1, "viewer_token", good["rooms"][0]["host_token"].clone()),
            (0, "viewer_token", good["rooms"][0]["host_token"].clone()),
            (0, "viewer_token", json!("a".repeat(64))),
            (0, "not_before", json!(WALL - 1)),
            (1, "not_before", json!(WALL + 1800)),
            (1, "expires_at", json!(WALL + 1801)),
        ] {
            let mut bad = good.clone();
            bad["rooms"][slot][key] = val;
            assert!(parse(&bad, WALL, now).is_err(), "{key}");
        }
        assert!(parse(&fixture(WALL, &[0; 9]), WALL, now).is_err());
        assert!(parse(&good, WALL + 600, now).is_err());
        let duplicate = serde_json::to_string(&good).unwrap().replacen(
            "\"version\":1",
            "\"version\":1,\"version\":1",
            1,
        );
        assert!(Batch::parse(duplicate.as_bytes(), WALL, now).is_err());
    }

    #[test]
    fn role_bearers_and_joined_rounds_stay_isolated_under_one_global_cap() {
        let wall = now().unwrap();
        let doc = fixture(wall, &[0; 2]);
        let batch = parse(&doc, wall, Instant::now()).unwrap();
        let [a, b] = batch.rooms.as_slice() else {
            panic!("expected two rounds")
        };
        let host_ip = HOST.parse().unwrap();
        let viewer_ip = VIEWER.parse().unwrap();
        let a_token = doc["rooms"][0]["host_token"].as_str().unwrap();
        let b_token = doc["rooms"][1]["host_token"].as_str().unwrap();
        assert_eq!(
            a.approval.permitted_role(host_ip, a_token),
            Some(Role::Host)
        );
        assert_eq!(a.approval.permitted_role(viewer_ip, a_token), None);
        assert_eq!(a.approval.permitted_role(host_ip, b_token), None);
        assert_eq!(b.approval.permitted_role(host_ip, a_token), None);
        assert_eq!(
            b.approval.permitted_role(host_ip, b_token),
            Some(Role::Host)
        );

        let reservations: Vec<_> = (0..MAX_CONNECTIONS).map(|_| a.reserve().unwrap()).collect();
        assert!(b.reserve().is_none());
        drop(reservations);
        assert!(b.reserve().is_some());

        let a_room = a.approval.room.simple().to_string();
        let b_room = b.approval.room.simple().to_string();
        let host = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(2);
        assert_eq!(
            a.join(&b_room, Role::Host, host, tx.clone()),
            Err("ROOM_NOT_FOUND")
        );
        assert!(a.join(&a_room, Role::Host, host, tx.clone()).is_ok());
        a.leave(host);
        assert_eq!(a.join(&a_room, Role::Host, host, tx.clone()), Err(CLOSED));
        assert!(b.join(&b_room, Role::Host, host, tx).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_batch_open_rejects_relative_symlink_and_shared_files() {
        use std::{
            fs,
            os::unix::fs::{symlink, DirBuilderExt, PermissionsExt},
        };
        let dir = std::path::PathBuf::from("/tmp").join(format!("rh-batch-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        struct Remove(std::path::PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _remove = Remove(dir.clone());
        let private = dir.join("batch.json");
        fs::write(&private, b"{}").unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(open_private(&private).is_ok());
        assert!(open_private(Path::new("batch.json")).is_err());
        let leaf_link = dir.join("leaf");
        symlink(&private, &leaf_link).unwrap();
        assert!(open_private(&leaf_link).is_err());
        let aliased_dir = dir.join("alias");
        symlink(&dir, &aliased_dir).unwrap();
        assert!(open_private(&aliased_dir.join("batch.json")).is_err());
        fs::set_permissions(&private, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(open_private(&private).is_err());
    }
}

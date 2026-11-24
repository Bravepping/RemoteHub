//! A dedicated TLS-only signaling listener, for one explicitly approved room.
//! A bearer authorizes its role's relay slot, not a device or media permission.

use super::{
    ClientFrame, ConnectInfo, Flow, HeaderMap, IntoResponse, IpAddr, Ipv4Addr, MAX_CONNECTIONS,
    MAX_WIRE, Message, PathBuf, Peer, PeerSources, ROOM_LIFETIME, Response, Role, Router,
    ServerFrame, SignalKind, SocketAddr, SocketPeer, State, StatusCode, VERSION, WebSocket,
    WebSocketUpgrade, header, signal_wire, token_matches, wire,
};
#[cfg(feature = "v2-public-control")]
use axum::extract::OriginalUri;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use hub_signal_v2::wire::PUBLIC_VIDEO_PATH;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[cfg(feature = "v2-public-control")]
use tokio::sync::Notify;
use tokio::{
    sync::mpsc,
    time::{Instant, sleep_until, timeout_at},
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[cfg(feature = "v2-public-control")]
#[path = "public_control_batch.rs"]
mod batch;

const EXPIRED: &str = "V2_PUBLIC_VIDEO_EXPIRED";
const DENIED: &str = "V2_PUBLIC_VIDEO_APPROVAL_INVALID";
const CLOSED: &str = "V2_PUBLIC_VIDEO_ROOM_CLOSED";

#[derive(Clone, Copy)]
pub(super) enum Purpose {
    Video,
    #[cfg(feature = "v2-public-control")]
    Control,
}

impl Purpose {
    fn path(self) -> &'static str {
        match self {
            Self::Video => PUBLIC_VIDEO_PATH,
            #[cfg(feature = "v2-public-control")]
            Self::Control => signal_wire::PUBLIC_CONTROL_PATH,
        }
    }

    fn profile_name(self) -> &'static str {
        match self {
            Self::Video => "VIDEO",
            #[cfg(feature = "v2-public-control")]
            Self::Control => "CONTROL",
        }
    }

    fn validate_token(self, bearer: &str) -> bool {
        match self {
            Self::Video => valid_role_token(bearer),
            #[cfg(feature = "v2-public-control")]
            Self::Control => signal_wire::valid_public_control_token(bearer),
        }
    }

    fn matches(self, expected: &str, supplied: &str) -> bool {
        match self {
            Self::Video => token_matches(expected, supplied),
            #[cfg(feature = "v2-public-control")]
            Self::Control => {
                if !self.validate_token(supplied) {
                    return false;
                }
                let context = b"RemoteHub.v2.PublicControlRelayToken";
                let tag = ring::hmac::sign(
                    &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, expected.as_bytes()),
                    context,
                );
                ring::hmac::verify(
                    &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, supplied.as_bytes()),
                    context,
                    tag.as_ref(),
                )
                .is_ok()
            }
        }
    }
}

/// The address in ConnectInfo is the only source identity. Do not install
/// this router on a plaintext listener or use proxy/forwarded headers.
pub(super) struct PublicVideoApproval {
    purpose: Purpose,
    room: Uuid,
    sources: PeerSources,
    host_token: Zeroizing<String>,
    viewer_token: Zeroizing<String>,
    expires_at: u64,
    deadline: Instant,
    #[cfg(feature = "v2-public-control")]
    not_before: Option<(u64, Instant)>,
    #[cfg(feature = "v2-public-control")]
    admission: signal_wire::PublicControlAdmission,
}

impl PublicVideoApproval {
    fn new(
        purpose: Purpose,
        room: Uuid,
        sources: PeerSources,
        host_token: Zeroizing<String>,
        viewer_token: Zeroizing<String>,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, &'static str> {
        Self::check_invitation(purpose, room, &host_token, &viewer_token)?;
        Ok(Self {
            purpose,
            room,
            sources,
            host_token,
            viewer_token,
            expires_at,
            deadline: Instant::now() + remaining(expires_at, now)?,
            #[cfg(feature = "v2-public-control")]
            not_before: None,
            #[cfg(feature = "v2-public-control")]
            admission: signal_wire::PublicControlAdmission::Legacy,
        })
    }

    fn check_invitation(
        purpose: Purpose,
        room: Uuid,
        host_token: &str,
        viewer_token: &str,
    ) -> Result<(), &'static str> {
        if room.is_nil()
            || !purpose.validate_token(host_token)
            || !purpose.validate_token(viewer_token)
            || purpose.matches(host_token, viewer_token)
        {
            return Err(DENIED);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn loopback_fixture(
        room: Uuid,
        sources: PeerSources,
        host_token: String,
        viewer_token: String,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, &'static str> {
        if !sources.host.is_loopback() || !sources.viewer.is_loopback() {
            return Err(DENIED);
        }
        Self::new(
            Purpose::Video,
            room,
            sources,
            Zeroizing::new(host_token),
            Zeroizing::new(viewer_token),
            expires_at,
            now,
        )
    }

    fn deadline(&self) -> Result<Instant, &'static str> {
        self.deadline_at(now()?, Instant::now())
    }

    fn deadline_at(&self, wall: u64, mono: Instant) -> Result<Instant, &'static str> {
        if mono >= self.deadline {
            return Err(EXPIRED);
        }
        #[cfg(feature = "v2-public-control")]
        if self
            .not_before
            .is_some_and(|(begin_wall, begin_mono)| wall < begin_wall || mono < begin_mono)
        {
            return Err(EXPIRED);
        }
        #[cfg(feature = "v2-public-control")]
        if self.admission != signal_wire::PublicControlAdmission::Legacy {
            let ttl = Duration::from_secs(self.expires_at.saturating_sub(wall));
            if ttl.is_zero() || ttl > self.admission.max_lease() {
                return Err(EXPIRED);
            }
            return Ok(self.deadline.min(mono + ttl));
        }
        Ok(self.deadline.min(mono + remaining(self.expires_at, wall)?))
    }

    fn admission_deadline(&self) -> Result<Instant, &'static str> {
        self.admission_deadline_at(now()?, Instant::now())
    }

    fn admission_deadline_at(&self, wall: u64, mono: Instant) -> Result<Instant, &'static str> {
        let deadline = self.deadline_at(wall, mono)?;
        #[cfg(feature = "v2-public-control")]
        if deadline.saturating_duration_since(mono) < self.admission.minimum_at_admission() {
            return Err(EXPIRED);
        }
        Ok(deadline)
    }

    fn has_source(&self, observed_ip: IpAddr) -> bool {
        observed_ip == IpAddr::V4(self.sources.host)
            || observed_ip == IpAddr::V4(self.sources.viewer)
    }

    fn permitted_role(&self, observed_ip: IpAddr, bearer: &str) -> Option<Role> {
        // Co-located NAT peers are acceptable: the separate token and
        // observed TCP source are both needed to identify the role.
        let host = self.purpose.matches(&self.host_token, bearer);
        let viewer = self.purpose.matches(&self.viewer_token, bearer);
        if host && observed_ip == IpAddr::V4(self.sources.host) {
            Some(Role::Host)
        } else if viewer && observed_ip == IpAddr::V4(self.sources.viewer) {
            Some(Role::Viewer)
        } else {
            None
        }
    }

    fn same_room(&self, requested: &str) -> bool {
        signal_wire::valid_room(requested)
            && Uuid::try_parse(requested).is_ok_and(|id| id == self.room)
    }
}

fn now() -> Result<u64, &'static str> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|age| age.as_secs())
        .map_err(|_| EXPIRED)
}

fn remaining(expires_at: u64, now: u64) -> Result<Duration, &'static str> {
    let ttl = Duration::from_secs(expires_at.saturating_sub(now));
    if ttl.is_zero() || ttl > ROOM_LIFETIME {
        return Err(EXPIRED);
    }
    Ok(ttl)
}

fn valid_role_token(token: &str) -> bool {
    signal_wire::valid_token(token) && !token.bytes().any(|byte| byte.is_ascii_uppercase())
}

#[derive(Default)]
struct OneRoom {
    host: Option<Peer>,
    viewer: Option<Peer>,
    negotiation: Flow,
    closed: bool,
}

struct PublicVideoApp {
    approval: PublicVideoApproval,
    room: Mutex<OneRoom>,
    active: Arc<AtomicUsize>,
    #[cfg(feature = "v2-public-control")]
    exit_wake: Option<Notify>,
}

impl PublicVideoApp {
    fn new(approval: PublicVideoApproval) -> Self {
        Self {
            approval,
            room: Mutex::new(OneRoom::default()),
            active: Arc::default(),
            #[cfg(feature = "v2-public-control")]
            exit_wake: None,
        }
    }

    #[cfg(feature = "v2-public-control")]
    async fn wait_for_one_shot_exit(&self, wake: &Notify) {
        let mut wall_check = tokio::time::interval(Duration::from_millis(250));
        loop {
            let Ok(deadline) = self.approval.deadline() else {
                return;
            };
            let closed = match self.room.lock() {
                Ok(room) => room.closed,
                // Never hold the listener open with poisoned state.
                Err(_) => return,
            };
            if closed && self.active.load(Ordering::Acquire) == 0 {
                // Both role handlers had a chance to send PeerLeft/close.
                return;
            }
            tokio::select! {
                _ = sleep_until(deadline) => return,
                _ = wall_check.tick() => {},
                _ = wake.notified() => {},
            }
        }
    }

    fn reserve(self: &Arc<Self>) -> Option<Slot> {
        let mut active = self.active.load(Ordering::Acquire);
        while active < MAX_CONNECTIONS {
            match self.active.compare_exchange_weak(
                active,
                active + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Slot(self.clone())),
                Err(actual) => active = actual,
            }
        }
        None
    }

    fn join(
        &self,
        room_id: &str,
        role: Role,
        id: Uuid,
        tx: mpsc::Sender<Message>,
    ) -> Result<Instant, &'static str> {
        // Only a correct HELLO competes for the one-use slot. Wrong room or
        // role attempts never consume a role or create a room.
        if !self.approval.same_room(room_id) {
            return Err("ROOM_NOT_FOUND");
        }
        let mut room = self.room.lock().map_err(|_| "STATE_UNAVAILABLE")?;
        let deadline = self.approval.deadline()?;
        if room.closed {
            return Err(CLOSED);
        }
        match role {
            Role::Host if room.host.is_some() => return Err("ROLE_OCCUPIED"),
            Role::Viewer if room.host.is_none() => return Err("ROOM_NOT_FOUND"),
            Role::Viewer if room.viewer.is_some() => return Err("ROLE_OCCUPIED"),
            Role::Host => {
                room.host = Some(Peer::new(id, tx.clone()));
                tx.try_send(wire(&ServerFrame::Waiting {})?)
                    .map_err(|_| "CLIENT_BACKPRESSURE")?;
            }
            Role::Viewer => {
                room.viewer = Some(Peer::new(id, tx.clone()));
                let paired = wire(&ServerFrame::Paired {})?;
                room.host
                    .as_ref()
                    .ok_or("ROLE_MISSING")?
                    .tx
                    .try_send(paired.clone())
                    .map_err(|_| "CLIENT_BACKPRESSURE")?;
                tx.try_send(paired).map_err(|_| "CLIENT_BACKPRESSURE")?;
            }
        }
        Ok(deadline)
    }

    fn relay(
        &self,
        role: Role,
        id: Uuid,
        kind: SignalKind,
        seq: u64,
        data: String,
    ) -> Result<(), &'static str> {
        if !kind.valid_payload(&data) || !kind.allows_sender(role) {
            return Err("SIGNAL_INVALID");
        }
        let mut room = self.room.lock().map_err(|_| "STATE_UNAVAILABLE")?;
        self.approval.deadline()?;
        if room.closed {
            return Err(CLOSED);
        }
        let next = room
            .negotiation
            .after_public_video(kind)
            .ok_or("SIGNAL_ORDER")?;
        let OneRoom { host, viewer, .. } = &mut *room;
        let (from, to) = match role {
            Role::Host => (host, viewer),
            Role::Viewer => (viewer, host),
        };
        let from = from.as_mut().ok_or("ROLE_MISSING")?;
        if from.id != id {
            return Err("PEER_MISMATCH");
        }
        from.accept_seq(seq)?;
        to.as_ref()
            .ok_or("PEER_NOT_JOINED")?
            .tx
            .try_send(wire(&ServerFrame::Signal {
                from: role,
                seq,
                signal: kind,
                data,
            })?)
            .map_err(|_| "CLIENT_BACKPRESSURE")?;
        room.negotiation = next;
        Ok(())
    }

    fn leave(&self, id: Uuid) {
        let Ok(mut room) = self.room.lock() else {
            return;
        };
        if ![room.host.as_ref(), room.viewer.as_ref()]
            .into_iter()
            .flatten()
            .any(|peer| peer.id == id)
        {
            return;
        }
        // First participant to leave burns the entire approval, including
        // an unclaimed role. Removing peers never makes either slot reusable.
        room.closed = true;
        let others = [room.host.take(), room.viewer.take()];
        if let Ok(message) = wire(&ServerFrame::PeerLeft {}) {
            for peer in others.into_iter().flatten().filter(|peer| peer.id != id) {
                let _ = peer.tx.try_send(message.clone());
            }
        }
        #[cfg(feature = "v2-public-control")]
        if let Some(wake) = &self.exit_wake {
            wake.notify_one();
        }
    }
}

struct Slot(Arc<PublicVideoApp>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
        #[cfg(feature = "v2-public-control")]
        if let Some(wake) = &self.0.exit_wake {
            wake.notify_one();
        }
    }
}

struct Joined<'a>(&'a PublicVideoApp, Uuid);

impl Drop for Joined<'_> {
    fn drop(&mut self) {
        self.0.leave(self.1);
    }
}

pub(super) fn public_video_router(approval: PublicVideoApproval) -> Router {
    router(Arc::new(PublicVideoApp::new(approval)))
}

fn router(app: Arc<PublicVideoApp>) -> Router {
    router_with_rooms(app.approval.purpose, vec![app])
}

#[derive(Clone)]
struct Admission {
    #[cfg(feature = "v2-public-control")]
    purpose: Purpose,
    rooms: Arc<[Arc<PublicVideoApp>]>,
}

fn router_with_rooms(purpose: Purpose, rooms: Vec<Arc<PublicVideoApp>>) -> Router {
    Router::new()
        .route(purpose.path(), get(upgrade))
        .with_state(Admission {
            #[cfg(feature = "v2-public-control")]
            purpose,
            rooms: rooms.into(),
        })
}

async fn upgrade(
    State(admission): State<Admission>,
    peer: SocketPeer,
    #[cfg(feature = "v2-public-control")] OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    #[cfg(feature = "v2-public-control")]
    let requested_admission = match batch::request_purpose(&headers) {
        Ok(admission) => admission,
        Err(_) => return StatusCode::FORBIDDEN.into_response(),
    };
    #[cfg(feature = "v2-public-control")]
    if matches!(admission.purpose, Purpose::Control)
        && uri.path_and_query().map(|target| target.as_str()) != Some(admission.purpose.path())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if headers.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let observed_ip = match peer {
        Ok(ConnectInfo(address)) => address.ip(),
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    let eligible = || {
        admission.rooms.iter().filter(|app| {
            #[cfg(feature = "v2-public-control")]
            if app.approval.admission != requested_admission {
                return false;
            }
            app.approval.admission_deadline().is_ok() && app.approval.has_source(observed_ip)
        })
    };
    if eligible().next().is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let values = headers.get_all(header::AUTHORIZATION);
    let mut headers = values.iter();
    let given = headers
        .next()
        .and_then(|entry| entry.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if headers.next().is_some() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Scan the small bounded pool, even after finding a match; no headers
    // can nominate an app. Its exact room is still checked by HELLO.
    let mut matches = eligible().filter_map(|app| {
        app.approval
            .permitted_role(observed_ip, given)
            .map(|role| (app, role))
    });
    let (Some((app, role)), None) = (matches.next(), matches.next()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let app = app.clone();
    let Some(guard) = app.reserve() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    ws.max_message_size(MAX_WIRE)
        .max_frame_size(MAX_WIRE)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            connection(socket, app, role).await;
        })
        .into_response()
}

async fn send_error(socket: &mut WebSocket, approval: &PublicVideoApproval, code: &'static str) {
    if let (Ok(message), Ok(deadline)) = (
        wire(&ServerFrame::Error { code: code.into() }),
        approval.deadline(),
    ) {
        let send_until = deadline.min(Instant::now() + Duration::from_secs(1));
        let _ = timeout_at(send_until, socket.send(message)).await;
    }
}

async fn connection(mut socket: WebSocket, app: Arc<PublicVideoApp>, role: Role) {
    let Ok(deadline) = app.approval.deadline() else {
        return;
    };
    let until_hello = deadline.min(Instant::now() + Duration::from_secs(5));
    let Ok(Some(Ok(Message::Text(first)))) = timeout_at(until_hello, socket.next()).await else {
        send_error(&mut socket, &app.approval, "HELLO_REQUIRED").await;
        return;
    };
    let Ok(ClientFrame::Hello {
        version: VERSION,
        room,
        role: requested,
    }) = signal_wire::decode(&first)
    else {
        send_error(&mut socket, &app.approval, "HELLO_INVALID").await;
        return;
    };
    if requested != role {
        send_error(&mut socket, &app.approval, "ROLE_SOURCE_MISMATCH").await;
        return;
    }
    let id = Uuid::new_v4();
    let (tx, mut rx) = mpsc::channel(8);
    let joined = Joined(&app, id);
    let deadline = match app.join(&room, role, id, tx) {
        Ok(deadline) => deadline,
        Err(code) => {
            drop(joined);
            send_error(&mut socket, &app.approval, code).await;
            return;
        }
    };
    let mut wall_check = tokio::time::interval(Duration::from_millis(250));
    loop {
        if app.approval.deadline().is_err() {
            break;
        }
        tokio::select! {
            biased;
            _ = sleep_until(deadline) => break,
            _ = wall_check.tick() => continue,
            outgoing = rx.recv() => {
                let Some(message) = outgoing else { break; };
                let Ok(deadline) = app.approval.deadline() else { break; };
                let send_until = deadline.min(Instant::now() + Duration::from_secs(2));
                if !matches!(timeout_at(send_until, socket.send(message)).await, Ok(Ok(()))) {
                    break;
                }
            }
            incoming = socket.next() => {
                let error = match incoming {
                    Some(Ok(Message::Text(text))) => match signal_wire::decode(&text) {
                        Ok(ClientFrame::Signal { seq, signal, data }) => {
                            match app.relay(role, id, signal, seq, data) {
                                Ok(()) => continue,
                                Err(code) => code,
                            }
                        }
                        Ok(ClientFrame::Leave {}) => break,
                        _ => "FRAME_INVALID",
                    },
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    _ => break,
                };
                // Revoke both slots before sending an error to this peer.
                app.leave(id);
                send_error(&mut socket, &app.approval, error).await;
                break;
            }
        }
    }
    drop(joined);
    if let Ok(deadline) = app.approval.deadline() {
        let until = deadline.min(Instant::now() + Duration::from_secs(1));
        let _ = timeout_at(until, socket.close()).await;
    }
}

fn public_unicast(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a != 0
        && a < 224
        && !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_documentation()
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 192 && b == 0)
        && !(a == 198 && (b == 18 || b == 19))
}

fn validate_public_bind(addr: SocketAddr, cert: &Path, key: &Path) -> Result<(), &'static str> {
    if !matches!(addr, SocketAddr::V4(addr)
        if (public_unicast(*addr.ip()) || super::valid_lan_ip(*addr.ip()))
        && addr.port() >= 1024)
    {
        return Err("V2_PUBLIC_VIDEO_BIND_IPV4_REQUIRED");
    }
    if cert.as_os_str().is_empty() || key.as_os_str().is_empty() {
        return Err("V2_PUBLIC_VIDEO_TLS_REQUIRED");
    }
    Ok(())
}

#[cfg(feature = "v2-public-control")]
fn exit_after_room_enabled(value: Option<&std::ffi::OsStr>) -> Result<bool, &'static str> {
    match value {
        None => Ok(false),
        Some(value) if value == "1" => Ok(true),
        _ => Err("V2_PUBLIC_CONTROL_EXIT_AFTER_ROOM_INVALID"),
    }
}

#[cfg(feature = "v2-public-control")]
async fn serve_one_shot(
    app: &PublicVideoApp,
    wake: &Notify,
    server: impl std::future::Future<Output = std::io::Result<()>>,
) -> std::io::Result<()> {
    tokio::select! {
        biased;
        result = server => result,
        // Dropping the serving future releases the listener, including at the
        // hard deadline when an upgraded connection may not yet have drained.
        _ = app.wait_for_one_shot_exit(wake) => Ok(()),
    }
}

pub(super) async fn run_from_env(purpose: Purpose) -> Result<(), &'static str> {
    fn env(name: &str) -> Result<String, &'static str> {
        std::env::var(name).map_err(|_| DENIED)
    }
    let profile = |field: &str| {
        env(&format!(
            "REMOTEHUB_V2_PUBLIC_{}_{field}",
            purpose.profile_name()
        ))
    };
    #[cfg(feature = "v2-public-control")]
    let one_shot = matches!(purpose, Purpose::Control)
        && exit_after_room_enabled(
            std::env::var_os("REMOTEHUB_V2_PUBLIC_CONTROL_EXIT_AFTER_ROOM").as_deref(),
        )?;
    #[cfg(feature = "v2-public-control")]
    let batch_path = if matches!(purpose, Purpose::Control) {
        std::env::var_os("REMOTEHUB_V2_PUBLIC_CONTROL_BATCH_FILE").map(PathBuf::from)
    } else {
        None
    };
    #[cfg(feature = "v2-public-control")]
    if one_shot && batch_path.is_some() {
        return Err("V2_PUBLIC_CONTROL_BATCH_CONFLICT");
    }

    let addr = env("REMOTEHUB_V2_BIND")?
        .parse::<SocketAddr>()
        .map_err(|_| "V2_PUBLIC_VIDEO_BIND_IPV4_REQUIRED")?;
    let cert = std::env::var_os("REMOTEHUB_V2_TLS_CERT")
        .map(PathBuf::from)
        .ok_or("V2_PUBLIC_VIDEO_TLS_REQUIRED")?;
    let key = std::env::var_os("REMOTEHUB_V2_TLS_KEY")
        .map(PathBuf::from)
        .ok_or("V2_PUBLIC_VIDEO_TLS_REQUIRED")?;
    validate_public_bind(addr, &cert, &key)?;
    #[cfg(feature = "v2-public-control")]
    if let Some(path) = batch_path {
        let batch = batch::Batch::load(&path)?;
        return batch.serve(addr, load_tls(cert, key).await?).await;
    }
    let room = profile("ROOM")?;
    if !signal_wire::valid_room(&room) {
        return Err(DENIED);
    }
    let room = Uuid::try_parse(&room).map_err(|_| DENIED)?;
    let sources = PeerSources {
        host: profile("HOST_IP")?
            .parse::<Ipv4Addr>()
            .map_err(|_| DENIED)?,
        viewer: profile("VIEWER_IP")?
            .parse::<Ipv4Addr>()
            .map_err(|_| DENIED)?,
    };
    if !public_unicast(sources.host) || !public_unicast(sources.viewer) {
        return Err(DENIED);
    }
    let approval = PublicVideoApproval::new(
        purpose,
        room,
        sources,
        Zeroizing::new(profile("HOST_TOKEN")?),
        Zeroizing::new(profile("VIEWER_TOKEN")?),
        profile("EXPIRES_AT")?.parse::<u64>().map_err(|_| DENIED)?,
        now()?,
    )?;
    let tls = load_tls(cert, key).await?;
    approval.deadline()?;
    let server = axum_server::bind_rustls(addr, tls);
    #[cfg(feature = "v2-public-control")]
    if one_shot {
        let mut app = PublicVideoApp::new(approval);
        app.exit_wake = Some(Notify::new());
        let app = Arc::new(app);
        let service = router(app.clone()).into_make_service_with_connect_info::<SocketAddr>();
        return serve_one_shot(
            &app,
            app.exit_wake.as_ref().expect("configured above"),
            server.serve(service),
        )
        .await
        .map_err(|_| "V2_SERVER_FAILED");
    }
    server
        .serve(public_video_router(approval).into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(|_| "V2_SERVER_FAILED")
}

async fn load_tls(
    cert: PathBuf,
    key: PathBuf,
) -> Result<axum_server::tls_rustls::RustlsConfig, &'static str> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .map_err(|_| "V2_TLS_CONFIG_FAILED")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval() -> PublicVideoApproval {
        let now = now().unwrap();
        PublicVideoApproval::loopback_fixture(
            Uuid::new_v4(),
            PeerSources {
                host: Ipv4Addr::LOCALHOST,
                viewer: Ipv4Addr::LOCALHOST,
            },
            "a".repeat(64),
            "b".repeat(64),
            now + 15,
            now,
        )
        .unwrap()
    }

    #[cfg(feature = "v2-public-control")]
    fn one_shot_app() -> PublicVideoApp {
        let now = now().unwrap();
        let approval = PublicVideoApproval::new(
            Purpose::Control,
            Uuid::new_v4(),
            PeerSources {
                host: Ipv4Addr::LOCALHOST,
                viewer: Ipv4Addr::LOCALHOST,
            },
            Zeroizing::new(format!("rhctl1-{}", "a".repeat(64))),
            Zeroizing::new(format!("rhctl1-{}", "b".repeat(64))),
            now + 15,
            now,
        )
        .unwrap();
        let mut app = PublicVideoApp::new(approval);
        app.exit_wake = Some(Notify::new());
        app
    }

    #[cfg(feature = "v2-public-control")]
    #[test]
    fn shutdown_flag_is_explicit_and_fail_closed() {
        use std::ffi::OsStr;
        assert_eq!(exit_after_room_enabled(None), Ok(false));
        assert_eq!(exit_after_room_enabled(Some(OsStr::new("1"))), Ok(true));
        for invalid in ["0", "", "true", "01", "1 ", " 1"] {
            assert!(exit_after_room_enabled(Some(OsStr::new(invalid))).is_err());
        }
    }

    #[cfg(feature = "v2-public-control")]
    #[tokio::test]
    async fn one_shot_requires_joined_room_teardown_and_both_handlers_drained() {
        let app = Arc::new(one_shot_app());
        let monitor = app.clone();
        let mut serve = tokio::spawn(async move {
            serve_one_shot(
                &monitor,
                monitor.exit_wake.as_ref().unwrap(),
                std::future::pending(),
            )
            .await
        });
        let (wrong_tx, _wrong_rx) = mpsc::channel(2);
        let unexpected = Uuid::new_v4();
        assert!(
            app.join(
                &unexpected.simple().to_string(),
                Role::Host,
                unexpected,
                wrong_tx
            )
            .is_err()
        );
        app.leave(unexpected);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut serve)
                .await
                .is_err()
        );

        let host_slot = app.reserve().unwrap();
        let viewer_slot = app.reserve().unwrap();
        let (host_tx, _host_rx) = mpsc::channel(2);
        let (viewer_tx, mut viewer_rx) = mpsc::channel(2);
        let room = app.approval.room.simple().to_string();
        let host = Uuid::new_v4();
        let viewer = Uuid::new_v4();
        app.join(&room, Role::Host, host, host_tx).unwrap();
        app.join(&room, Role::Viewer, viewer, viewer_tx).unwrap();
        app.leave(host);
        drop(host_slot);
        let Some(Message::Text(paired)) = viewer_rx.recv().await else {
            panic!("missing Paired")
        };
        let Some(Message::Text(left)) = viewer_rx.recv().await else {
            panic!("missing PeerLeft")
        };
        assert!(matches!(
            signal_wire::decode::<ServerFrame>(&paired),
            Ok(ServerFrame::Paired {})
        ));
        assert!(matches!(
            signal_wire::decode::<ServerFrame>(&left),
            Ok(ServerFrame::PeerLeft {})
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut serve)
                .await
                .is_err()
        );
        drop(viewer_slot);
        tokio::time::timeout(Duration::from_secs(2), serve)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[cfg(feature = "v2-public-control")]
    #[tokio::test]
    async fn one_shot_exits_at_invitation_deadline_with_an_unjoined_slot() {
        let mut app = one_shot_app();
        app.approval.deadline = Instant::now() + Duration::from_millis(40);
        let app = Arc::new(app);
        let _unjoined_slot = app.reserve().unwrap();
        tokio::time::timeout(
            Duration::from_secs(2),
            serve_one_shot(
                &app,
                app.exit_wake.as_ref().unwrap(),
                std::future::pending(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(app.approval.deadline().is_err());
        assert!(!app.room.lock().unwrap().closed);
    }

    #[test]
    fn same_observed_ipv4_still_requires_the_distinct_role_bearer() {
        let approval = approval();
        assert_eq!(
            approval.permitted_role(Ipv4Addr::LOCALHOST.into(), &"a".repeat(64)),
            Some(Role::Host)
        );
        assert_eq!(
            approval.permitted_role(Ipv4Addr::LOCALHOST.into(), &"b".repeat(64)),
            Some(Role::Viewer)
        );
        assert_eq!(
            approval.permitted_role(Ipv4Addr::new(127, 0, 0, 2).into(), &"a".repeat(64)),
            None
        );
        assert_eq!(
            approval.permitted_role("::ffff:127.0.0.1".parse().unwrap(), &"a".repeat(64)),
            None
        );
    }

    #[cfg(feature = "v2-public-control")]
    #[tokio::test]
    async fn product_control_and_video_mount_only_their_own_path_and_tokens() {
        use super::super::tls_smoke::{make_ca, start_tls_router};
        use rustls::{ClientConfig, RootCertStore};
        use tokio_tungstenite::{
            Connector, client_async_tls_with_config,
            tungstenite::{Error as WsError, client::IntoClientRequest},
        };
        let raw_host = "a".repeat(64);
        let raw_viewer = "b".repeat(64);
        let control_host = format!("rhctl1-{raw_host}");
        let control_viewer = format!("rhctl1-{raw_viewer}");
        let ca = make_ca();
        let mut roots = RootCertStore::empty();
        roots.add(ca.der().clone()).unwrap();
        let trust = Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );

        use signal_wire::{PUBLIC_CONTROL_ADMISSION_HEADER, PublicControlAdmission as Mode};
        for (purpose, mode, good, viewer, wrong, wrong_path) in [
            (
                Purpose::Video,
                Mode::Legacy,
                &raw_host,
                &raw_viewer,
                &control_host,
                Purpose::Control.path(),
            ),
            (
                Purpose::Control,
                Mode::Legacy,
                &control_host,
                &control_viewer,
                &raw_host,
                Purpose::Video.path(),
            ),
            (
                Purpose::Control,
                Mode::ImmediateV1,
                &control_host,
                &control_viewer,
                &raw_host,
                Purpose::Video.path(),
            ),
            (
                Purpose::Control,
                Mode::BasicSustainedV1,
                &control_host,
                &control_viewer,
                &raw_host,
                Purpose::Video.path(),
            ),
        ] {
            let now = now().unwrap();
            let mut approval = PublicVideoApproval::new(
                purpose,
                Uuid::new_v4(),
                PeerSources {
                    host: Ipv4Addr::LOCALHOST,
                    viewer: Ipv4Addr::LOCALHOST,
                },
                Zeroizing::new(good.to_owned()),
                Zeroizing::new(viewer.to_owned()),
                now + 15,
                now,
            )
            .unwrap();
            approval.admission = mode;
            if mode == Mode::BasicSustainedV1 {
                // Test-only loopback policy. Production version-3 files still
                // require independent public source and strict room bounds.
                approval.expires_at = now + 2400;
                approval.deadline = Instant::now() + Duration::from_secs(2400);
            }
            assert_eq!(
                approval.permitted_role(Ipv4Addr::LOCALHOST.into(), wrong),
                None
            );
            let (addr, server) =
                start_tls_router(&ca, "127.0.0.1", public_video_router(approval)).await;
            let mut requests = vec![
                (wrong_path, good.as_str(), mode, Some(StatusCode::NOT_FOUND)),
                (
                    purpose.path(),
                    wrong.as_str(),
                    mode,
                    Some(StatusCode::UNAUTHORIZED),
                ),
            ];
            for other in [Mode::Legacy, Mode::ImmediateV1, Mode::BasicSustainedV1] {
                if other != mode {
                    requests.push((
                        purpose.path(),
                        good.as_str(),
                        other,
                        Some(StatusCode::FORBIDDEN),
                    ));
                }
            }
            requests.push((purpose.path(), good.as_str(), mode, None));
            for (path, token, marker, expected) in requests {
                let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
                let mut request = format!("wss://{addr}{path}").into_client_request().unwrap();
                request.headers_mut().insert(
                    header::AUTHORIZATION,
                    format!("Bearer {token}").parse().unwrap(),
                );
                if let Some(marker) = marker.header_value() {
                    request
                        .headers_mut()
                        .insert(PUBLIC_CONTROL_ADMISSION_HEADER, marker.parse().unwrap());
                }
                let result = tokio::time::timeout(
                    Duration::from_secs(3),
                    client_async_tls_with_config(
                        request,
                        tcp,
                        None,
                        Some(Connector::Rustls(trust.clone())),
                    ),
                )
                .await
                .unwrap();
                match expected {
                    Some(status) => assert!(
                        matches!(result, Err(WsError::Http(response)) if response.status() == status)
                    ),
                    None => assert!(result.is_ok()),
                }
            }
            server.abort();
            assert!(server.await.unwrap_err().is_cancelled());
        }
    }

    #[test]
    fn public_room_does_not_reopen_after_either_slot_leaves() {
        let approval = approval();
        let room = approval.room.simple().to_string();
        let app = PublicVideoApp::new(approval);
        let (other_tx, _other_rx) = mpsc::channel(8);
        assert_eq!(
            app.join(&room, Role::Viewer, Uuid::new_v4(), other_tx.clone()),
            Err("ROOM_NOT_FOUND")
        );
        assert_eq!(
            app.join(
                &Uuid::new_v4().simple().to_string(),
                Role::Host,
                Uuid::new_v4(),
                other_tx.clone()
            ),
            Err("ROOM_NOT_FOUND")
        );
        let (host_tx, _host_rx) = mpsc::channel(8);
        let host = Uuid::new_v4();
        app.join(&room, Role::Host, host, host_tx).unwrap();
        assert_eq!(
            app.join(&room, Role::Host, Uuid::new_v4(), other_tx.clone()),
            Err("ROLE_OCCUPIED")
        );
        app.leave(host);
        for role in [Role::Host, Role::Viewer] {
            assert_eq!(
                app.join(&room, role, Uuid::new_v4(), other_tx.clone()),
                Err(CLOSED)
            );
        }
    }
}

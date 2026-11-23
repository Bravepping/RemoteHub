//! A dedicated TLS-only signaling listener, for one explicitly approved room.
//! A bearer authorizes its role's relay slot, not a device or media permission.

use super::{
    header, signal_wire, token_matches, wire, ClientFrame, ConnectInfo, Flow, HeaderMap,
    IntoResponse, IpAddr, Ipv4Addr, Message, PathBuf, Peer, PeerSources, Response, Role, Router,
    ServerFrame, SignalKind, SocketAddr, SocketPeer, State, StatusCode, WebSocket,
    WebSocketUpgrade, MAX_CONNECTIONS, MAX_WIRE, ROOM_LIFETIME, VERSION,
};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use hub_signal_v2::wire::PUBLIC_VIDEO_PATH;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::mpsc,
    time::{sleep_until, timeout_at, Instant},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const EXPIRED: &str = "V2_PUBLIC_VIDEO_EXPIRED";
const DENIED: &str = "V2_PUBLIC_VIDEO_APPROVAL_INVALID";
const CLOSED: &str = "V2_PUBLIC_VIDEO_ROOM_CLOSED";

/// The address in ConnectInfo is the only source identity. Do not install
/// this router on a plaintext listener or use proxy/forwarded headers.
pub(super) struct PublicVideoApproval {
    room: Uuid,
    sources: PeerSources,
    host_token: Zeroizing<String>,
    viewer_token: Zeroizing<String>,
    expires_at: u64,
    deadline: Instant,
}

impl PublicVideoApproval {
    fn new(
        room: Uuid,
        sources: PeerSources,
        host_token: Zeroizing<String>,
        viewer_token: Zeroizing<String>,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, &'static str> {
        if room.is_nil()
            || !valid_role_token(&host_token)
            || !valid_role_token(&viewer_token)
            || token_matches(&host_token, &viewer_token)
        {
            return Err(DENIED);
        }
        Ok(Self {
            room,
            sources,
            host_token,
            viewer_token,
            expires_at,
            deadline: Instant::now() + remaining(expires_at, now)?,
        })
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
            room,
            sources,
            Zeroizing::new(host_token),
            Zeroizing::new(viewer_token),
            expires_at,
            now,
        )
    }

    fn deadline(&self) -> Result<Instant, &'static str> {
        let mono = Instant::now();
        if mono >= self.deadline {
            return Err(EXPIRED);
        }
        Ok(self
            .deadline
            .min(mono + remaining(self.expires_at, now()?)?))
    }

    fn has_source(&self, observed_ip: IpAddr) -> bool {
        observed_ip == IpAddr::V4(self.sources.host)
            || observed_ip == IpAddr::V4(self.sources.viewer)
    }

    fn permitted_role(&self, observed_ip: IpAddr, bearer: &str) -> Option<Role> {
        // Co-located NAT peers are acceptable: the separate token and
        // observed TCP source are both needed to identify the role.
        let host = token_matches(&self.host_token, bearer);
        let viewer = token_matches(&self.viewer_token, bearer);
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
    active: AtomicUsize,
}

impl PublicVideoApp {
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
    }
}

struct Slot(Arc<PublicVideoApp>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Joined<'a>(&'a PublicVideoApp, Uuid);

impl Drop for Joined<'_> {
    fn drop(&mut self) {
        self.0.leave(self.1);
    }
}

pub(super) fn public_video_router(approval: PublicVideoApproval) -> Router {
    let app = Arc::new(PublicVideoApp {
        approval,
        room: Mutex::new(OneRoom::default()),
        active: AtomicUsize::new(0),
    });
    Router::new()
        .route(PUBLIC_VIDEO_PATH, get(upgrade))
        .with_state(app)
}

async fn upgrade(
    State(app): State<Arc<PublicVideoApp>>,
    peer: SocketPeer,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if app.approval.deadline().is_err() || headers.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let observed_ip = match peer {
        Ok(ConnectInfo(address)) if app.approval.has_source(address.ip()) => address.ip(),
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
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
    let Some(role) = app.approval.permitted_role(observed_ip, given) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
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

pub(super) async fn run_from_env() -> Result<(), &'static str> {
    fn env(name: &str) -> Result<String, &'static str> {
        std::env::var(name).map_err(|_| DENIED)
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
    let room = env("REMOTEHUB_V2_PUBLIC_VIDEO_ROOM")?;
    if !signal_wire::valid_room(&room) {
        return Err(DENIED);
    }
    let room = Uuid::try_parse(&room).map_err(|_| DENIED)?;
    let sources = PeerSources {
        host: env("REMOTEHUB_V2_PUBLIC_VIDEO_HOST_IP")?
            .parse::<Ipv4Addr>()
            .map_err(|_| DENIED)?,
        viewer: env("REMOTEHUB_V2_PUBLIC_VIDEO_VIEWER_IP")?
            .parse::<Ipv4Addr>()
            .map_err(|_| DENIED)?,
    };
    if !public_unicast(sources.host) || !public_unicast(sources.viewer) {
        return Err(DENIED);
    }
    let approval = PublicVideoApproval::new(
        room,
        sources,
        Zeroizing::new(env("REMOTEHUB_V2_PUBLIC_VIDEO_HOST_TOKEN")?),
        Zeroizing::new(env("REMOTEHUB_V2_PUBLIC_VIDEO_VIEWER_TOKEN")?),
        env("REMOTEHUB_V2_PUBLIC_VIDEO_EXPIRES_AT")?
            .parse::<u64>()
            .map_err(|_| DENIED)?,
        now()?,
    )?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .map_err(|_| "V2_TLS_CONFIG_FAILED")?;
    approval.deadline()?;
    axum_server::bind_rustls(addr, tls)
        .serve(public_video_router(approval).into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(|_| "V2_SERVER_FAILED")
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

    #[test]
    fn public_room_does_not_reopen_after_either_slot_leaves() {
        let approval = approval();
        let room = approval.room.simple().to_string();
        let app = PublicVideoApp {
            approval,
            room: Mutex::new(OneRoom::default()),
            active: AtomicUsize::new(0),
        };
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

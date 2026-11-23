//! V2 signaling relay: loopback by default; explicit WSS-only two-peer private LAN.
use axum::{
    extract::{
        rejection::ExtensionRejection,
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use hub_signal_v2::wire::{
    self as signal_wire, ClientFrame, Flow, Role, ServerFrame, SignalKind, WireError, MAX_WIRE,
    PATH, ROOM_LIFETIME, VERSION,
};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{sync::mpsc, time::Instant};
use uuid::Uuid;
use zeroize::Zeroizing;

const MAX_ROOMS: usize = 4;
const MAX_CONNECTIONS: usize = 8;

fn wire(frame: &ServerFrame) -> Result<Message, &'static str> {
    signal_wire::encode(frame)
        .map(|text| Message::Text(text.into()))
        .map_err(|error| match error {
            WireError::Invalid => "ENCODE_FAILED",
            WireError::Size => "SIGNAL_SIZE",
        })
}

struct Peer {
    id: Uuid,
    tx: mpsc::Sender<Message>,
    last_seq: u64,
    window: Instant,
    events: u32,
}
impl Peer {
    fn new(id: Uuid, tx: mpsc::Sender<Message>) -> Self {
        Self {
            id,
            tx,
            last_seq: 0,
            window: Instant::now(),
            events: 0,
        }
    }
    fn accept_seq(&mut self, seq: u64) -> Result<(), &'static str> {
        if self.last_seq.checked_add(1) != Some(seq) {
            return Err("SIGNAL_SEQUENCE");
        }
        if self.window.elapsed() >= Duration::from_secs(1) {
            self.window = Instant::now();
            self.events = 0;
        }
        if self.events >= 60 {
            return Err("SIGNAL_RATE_LIMIT");
        }
        self.events += 1;
        self.last_seq = seq;
        Ok(())
    }
}
struct Room {
    deadline: Instant,
    negotiation: Flow,
    host: Option<Peer>,
    viewer: Option<Peer>,
}
#[derive(Clone, Copy)]
struct PeerSources {
    host: Ipv4Addr,
    viewer: Ipv4Addr,
}

enum Access {
    Loopback(Zeroizing<String>),
    PrivateLan {
        sources: PeerSources,
        host_token: Zeroizing<String>,
        viewer_token: Zeroizing<String>,
    },
}

type SocketPeer = Result<ConnectInfo<SocketAddr>, ExtensionRejection>;

impl Access {
    fn credentials(&self, ip: IpAddr) -> Option<(Option<Role>, &str)> {
        match self {
            Self::Loopback(token) if ip.is_loopback() => Some((None, token.as_str())),
            Self::PrivateLan {
                sources,
                host_token,
                ..
            } if ip == IpAddr::V4(sources.host) => Some((Some(Role::Host), host_token.as_str())),
            Self::PrivateLan {
                sources,
                viewer_token,
                ..
            } if ip == IpAddr::V4(sources.viewer) => {
                Some((Some(Role::Viewer), viewer_token.as_str()))
            }
            _ => None,
        }
    }

    fn sources(&self) -> Option<PeerSources> {
        match self {
            Self::Loopback(_) => None,
            Self::PrivateLan { sources, .. } => Some(*sources),
        }
    }
}

struct App {
    access: Access,
    rooms: Mutex<HashMap<String, Room>>,
    active: AtomicUsize,
}
impl App {
    fn reserve(&self) -> bool {
        let mut n = self.active.load(Ordering::Acquire);
        loop {
            if n >= MAX_CONNECTIONS {
                return false;
            }
            match self
                .active
                .compare_exchange_weak(n, n + 1, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return true,
                Err(actual) => n = actual,
            }
        }
    }
    fn join(
        &self,
        key: &str,
        role: Role,
        id: Uuid,
        tx: mpsc::Sender<Message>,
    ) -> Result<Instant, &'static str> {
        if !signal_wire::valid_room(key) {
            return Err("ROOM_FORMAT");
        }
        let mut rooms = self.rooms.lock().map_err(|_| "STATE_UNAVAILABLE")?;
        if !rooms.contains_key(key) {
            if role != Role::Host {
                return Err("ROOM_NOT_FOUND");
            }
            if rooms.len() >= MAX_ROOMS {
                return Err("ROOM_LIMIT");
            }
            rooms.insert(
                key.to_owned(),
                Room {
                    deadline: Instant::now() + ROOM_LIFETIME,
                    negotiation: Flow::default(),
                    host: None,
                    viewer: None,
                },
            );
        }
        let room = rooms.get_mut(key).ok_or("ROOM_NOT_FOUND")?;
        if Instant::now() >= room.deadline {
            return Err("ROOM_EXPIRED");
        }
        let occupied = match role {
            Role::Host => room.host.is_some(),
            Role::Viewer => room.viewer.is_some(),
        };
        if occupied {
            return Err("ROLE_OCCUPIED");
        }
        let new_peer = Peer::new(id, tx);
        match role {
            Role::Host => room.host = Some(new_peer),
            Role::Viewer => room.viewer = Some(new_peer),
        }
        if room.host.is_some() && room.viewer.is_some() {
            let msg = wire(&ServerFrame::Paired {})?;
            for peer in [room.host.as_ref(), room.viewer.as_ref()]
                .into_iter()
                .flatten()
            {
                peer.tx
                    .try_send(msg.clone())
                    .map_err(|_| "CLIENT_BACKPRESSURE")?;
            }
        } else {
            let peer = match role {
                Role::Host => room.host.as_ref(),
                Role::Viewer => room.viewer.as_ref(),
            };
            peer.ok_or("ROLE_MISSING")?
                .tx
                .try_send(wire(&ServerFrame::Waiting {})?)
                .map_err(|_| "CLIENT_BACKPRESSURE")?;
        }
        Ok(room.deadline)
    }
    fn relay(
        &self,
        key: &str,
        role: Role,
        id: Uuid,
        signal: SignalKind,
        seq: u64,
        data: String,
    ) -> Result<(), &'static str> {
        if !signal.valid_payload(&data) {
            return Err("SIGNAL_SIZE");
        }
        if !signal.allows_sender(role) {
            return Err("SIGNAL_ROLE_DENIED");
        }
        let mut rooms = self.rooms.lock().map_err(|_| "STATE_UNAVAILABLE")?;
        let room = rooms.get_mut(key).ok_or("ROOM_NOT_FOUND")?;
        if Instant::now() >= room.deadline {
            return Err("ROOM_EXPIRED");
        }
        let (sender, target) = match role {
            Role::Host => (&mut room.host, &room.viewer),
            Role::Viewer => (&mut room.viewer, &room.host),
        };
        let from = sender.as_mut().ok_or("ROLE_MISSING")?;
        if from.id != id {
            return Err("PEER_MISMATCH");
        }
        let to = target.as_ref().ok_or("PEER_NOT_JOINED")?;
        // Private LAN accepts only the protocol path with challenges in both
        // directions. Their cryptographic validity is still verified by peers.
        if self.access.sources().is_some()
            && matches!(
                (room.negotiation, signal),
                (Flow::BeforeOffer, SignalKind::Offer)
            )
        {
            return Err("CHALLENGES_REQUIRED");
        }
        let negotiation = room.negotiation.after(signal).ok_or("SIGNAL_ORDER")?;
        from.accept_seq(seq)?;
        to.tx
            .try_send(wire(&ServerFrame::Signal {
                from: role,
                seq,
                signal,
                data,
            })?)
            .map_err(|_| "CLIENT_BACKPRESSURE")?;
        room.negotiation = negotiation;
        Ok(())
    }
    fn leave(&self, key: &str, id: Uuid) {
        let Ok(mut rooms) = self.rooms.lock() else {
            return;
        };
        let owned = rooms.get(key).is_some_and(|room| {
            room.host.as_ref().is_some_and(|p| p.id == id)
                || room.viewer.as_ref().is_some_and(|p| p.id == id)
        });
        if owned {
            if let Some(room) = rooms.remove(key) {
                if let Ok(msg) = wire(&ServerFrame::PeerLeft {}) {
                    for peer in [room.host, room.viewer].into_iter().flatten() {
                        if peer.id != id {
                            let _ = peer.tx.try_send(msg.clone());
                        }
                    }
                }
            }
        }
    }
}
struct Lease(Arc<App>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}
fn token_matches(expected: &str, given: &str) -> bool {
    if !signal_wire::valid_token(given) {
        return false;
    }
    let message = b"RemoteHub.v2.SignalRelayToken";
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, expected.as_bytes()),
        message,
    );
    ring::hmac::verify(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, given.as_bytes()),
        message,
        tag.as_ref(),
    )
    .is_ok()
}
async fn upgrade(
    State(app): State<Arc<App>>,
    peer: SocketPeer,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // Only the socket peer supplied by the server may establish source
    // identity. In particular, never consult Forwarded or X-Forwarded-For.
    let Some((permitted_role, token)) = peer
        .ok()
        .and_then(|ConnectInfo(address)| app.access.credentials(address.ip()))
    else {
        return StatusCode::FORBIDDEN.into_response();
    };
    // This is a native-client relay. Do not accept a browser Origin
    // or two Authorization headers that proxies could interpret differently.
    if headers.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let authorization_values = headers.get_all(header::AUTHORIZATION);
    let mut authorization = authorization_values.iter();
    let candidate = authorization
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if authorization.next().is_some() || !token_matches(token, candidate) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !app.reserve() {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let guard = Lease(app.clone());
    ws.max_message_size(MAX_WIRE)
        .max_frame_size(MAX_WIRE)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            connection(socket, app, permitted_role).await;
        })
        .into_response()
}
async fn send_error(socket: &mut WebSocket, code: &'static str) {
    if let Ok(message) = wire(&ServerFrame::Error { code: code.into() }) {
        let _ = tokio::time::timeout(Duration::from_secs(1), socket.send(message)).await;
    }
}
async fn connection(mut socket: WebSocket, app: Arc<App>, permitted_role: Option<Role>) {
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
    let Some(Ok(Message::Text(first))) = first.ok().flatten() else {
        send_error(&mut socket, "HELLO_REQUIRED").await;
        return;
    };
    let Ok(ClientFrame::Hello {
        version: VERSION,
        room,
        role,
    }) = signal_wire::decode(&first)
    else {
        send_error(&mut socket, "HELLO_INVALID").await;
        return;
    };
    if permitted_role.is_some_and(|expected| role != expected) {
        send_error(&mut socket, "ROLE_SOURCE_MISMATCH").await;
        return;
    }
    let id = Uuid::new_v4();
    let (tx, mut rx) = mpsc::channel(16);
    let deadline = match app.join(&room, role, id, tx) {
        Ok(deadline) => deadline,
        Err(code) => {
            app.leave(&room, id);
            send_error(&mut socket, code).await;
            return;
        }
    };
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => break,
            outgoing = rx.recv() => {
                let Some(message) = outgoing else { break; };
                let send_deadline = deadline.min(Instant::now() + Duration::from_secs(2));
                if !matches!(tokio::time::timeout_at(send_deadline, socket.send(message)).await, Ok(Ok(()))) {
                    break;
                }
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let frame = signal_wire::decode(&text);
                        match frame {
                            Ok(ClientFrame::Signal { seq, signal, data }) => {
                                if let Err(code) = app.relay(&room, role, id, signal, seq, data) {
                                    send_error(&mut socket, code).await;
                                    break;
                                }
                            }
                            Ok(ClientFrame::Leave {}) => break,
                            _ => { send_error(&mut socket, "FRAME_INVALID").await; break; }
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    _ => break,
                }
            }
        }
    }
    app.leave(&room, id);
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close()).await;
}
fn valid_lan_ip(ip: Ipv4Addr) -> bool {
    // Without subnet discovery, conservatively reject address forms often
    // used as local network/broadcast endpoints; client applies same policy.
    ip.is_private() && !matches!(ip.octets()[3], 0 | 255)
}

fn validate_bind(
    addr: SocketAddr,
    cert: Option<&Path>,
    key: Option<&Path>,
    sources: Option<PeerSources>,
) -> Result<(), &'static str> {
    if cert.is_some() != key.is_some() {
        return Err("V2_TLS_PAIR_REQUIRED");
    }
    if let Some(sources) = sources {
        if cert.is_none_or(|value| value.as_os_str().is_empty())
            || key.is_none_or(|value| value.as_os_str().is_empty())
        {
            return Err("V2_LAN_TLS_REQUIRED");
        }
        if !matches!(addr, SocketAddr::V4(addr) if valid_lan_ip(*addr.ip()) && addr.port() != 0) {
            return Err("V2_LAN_BIND_PRIVATE_IPV4_REQUIRED");
        }
        if !valid_lan_ip(sources.host)
            || !valid_lan_ip(sources.viewer)
            || sources.host == sources.viewer
        {
            return Err("V2_LAN_DISTINCT_PRIVATE_PEERS_REQUIRED");
        }
    } else if !addr.ip().is_loopback() {
        return Err("V2_BIND_LOOPBACK_ONLY");
    }
    Ok(())
}

async fn health(State(app): State<Arc<App>>, peer: SocketPeer) -> Response {
    if peer
        .ok()
        .is_some_and(|ConnectInfo(address)| app.access.credentials(address.ip()).is_some())
    {
        "ok".into_response()
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route(PATH, get(upgrade))
        .with_state(app)
}

fn read_lan_ip(name: &str) -> Result<Ipv4Addr, &'static str> {
    std::env::var(name)
        .map_err(|_| "V2_LAN_DISTINCT_PRIVATE_PEERS_REQUIRED")?
        .parse()
        .map_err(|_| "V2_LAN_DISTINCT_PRIVATE_PEERS_REQUIRED")
}

fn read_token(name: &str, lan: bool) -> Result<Zeroizing<String>, &'static str> {
    let token = Zeroizing::new(std::env::var(name).map_err(|_| {
        if lan {
            "V2_LAN_ROLE_TOKENS_REQUIRED"
        } else {
            "V2_TOKEN_REQUIRED"
        }
    })?);
    if !signal_wire::valid_token(&token) {
        return Err(if lan {
            "V2_LAN_ROLE_TOKENS_INVALID"
        } else {
            "V2_TOKEN_MUST_BE_64_HEX"
        });
    }
    if lan && token.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err("V2_LAN_ROLE_TOKENS_INVALID");
    }
    Ok(token)
}

fn read_access() -> Result<Access, &'static str> {
    match std::env::var("REMOTEHUB_V2_MODE").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("loopback") => {
            Ok(Access::Loopback(read_token("REMOTEHUB_V2_TOKEN", false)?))
        }
        Ok("private-lan") => {
            let sources = PeerSources {
                host: read_lan_ip("REMOTEHUB_V2_LAN_HOST_IP")?,
                viewer: read_lan_ip("REMOTEHUB_V2_LAN_VIEWER_IP")?,
            };
            // A role token is only an access gate. It neither asserts a device
            // identity nor authorizes media/input; end users prove both separately.
            let host_token = read_token("REMOTEHUB_V2_LAN_HOST_TOKEN", true)?;
            let viewer_token = read_token("REMOTEHUB_V2_LAN_VIEWER_TOKEN", true)?;
            if token_matches(host_token.as_str(), viewer_token.as_str()) {
                return Err("V2_LAN_DISTINCT_ROLE_TOKENS_REQUIRED");
            }
            Ok(Access::PrivateLan {
                sources,
                host_token,
                viewer_token,
            })
        }
        _ => Err("V2_MODE_INVALID"),
    }
}

async fn run() -> Result<(), &'static str> {
    let access = read_access()?;
    let addr: SocketAddr = match std::env::var("REMOTEHUB_V2_BIND") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "127.0.0.1:48620".into(),
        Err(_) => return Err("V2_BIND_INVALID"),
    }
    .parse()
    .map_err(|_| "V2_BIND_INVALID")?;
    let cert = std::env::var_os("REMOTEHUB_V2_TLS_CERT").map(PathBuf::from);
    let key = std::env::var_os("REMOTEHUB_V2_TLS_KEY").map(PathBuf::from);
    validate_bind(addr, cert.as_deref(), key.as_deref(), access.sources())?;
    let router = router(Arc::new(App {
        access,
        rooms: Mutex::new(HashMap::new()),
        active: AtomicUsize::new(0),
    }));
    if let (Some(cert), Some(key)) = (cert, key) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
            .await
            .map_err(|_| "V2_TLS_CONFIG_FAILED")?;
        axum_server::bind_rustls(addr, tls)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .map_err(|_| "V2_SERVER_FAILED")
    } else {
        axum_server::bind(addr)
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .map_err(|_| "V2_SERVER_FAILED")
    }
}
#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("{code}");
        std::process::exit(1);
    }
}
#[cfg(test)]
#[path = "v2_signaling/data_smoke.rs"]
mod data_smoke;

#[cfg(test)]
#[path = "v2_signaling/tls_smoke.rs"]
mod tls_smoke;

#[cfg(test)]
#[path = "v2_signaling/public_video_smoke.rs"]
mod public_video_smoke;

#[cfg(feature = "v2-synthetic-video")]
#[path = "v2_signaling/verified_synthetic.rs"]
pub mod verified_synthetic;

#[cfg(test)]
mod tests {
    use super::*;
    use hub_signal_v2::{
        wire::MAX_SIGNAL, ClientError, LoopbackRelay, RelayEvent, RelayToken, RoomId,
    };
    use serde_json::{json, Value};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{
            client::IntoClientRequest, handshake::client::Request, http::HeaderValue,
            Error as WsError, Message as WsMessage,
        },
        MaybeTlsStream, WebSocketStream,
    };

    const ROOM: &str = "0123456789abcdef0123456789abcdef";
    pub(super) const TOKEN: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    pub(super) const WRONG_TOKEN: &str =
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

    fn app() -> Arc<App> {
        Arc::new(App {
            access: Access::Loopback(Zeroizing::new(TOKEN.into())),
            rooms: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
        })
    }

    #[test]
    fn token_requires_exact_secret() {
        assert!(token_matches(TOKEN, TOKEN));
        assert!(!token_matches(TOKEN, WRONG_TOKEN));
        assert!(!token_matches(TOKEN, "a"));
        assert!(!token_matches(TOKEN, &"z".repeat(64)));
    }

    #[test]
    fn rejects_non_loopback_even_with_tls_files_and_rejects_missing_tls_half() {
        let cert = Some(Path::new("example-cert"));
        let key = Some(Path::new("example-key"));
        let loopback = "127.0.0.1:0".parse().unwrap();
        assert_eq!(validate_bind(loopback, None, None, None), Ok(()));
        assert_eq!(validate_bind(loopback, cert, key, None), Ok(()));
        assert_eq!(
            validate_bind(loopback, cert, None, None),
            Err("V2_TLS_PAIR_REQUIRED")
        );
        for invalid in ["0.0.0.0:0", "[::]:0", "192.0.2.1:48620"] {
            assert_eq!(
                validate_bind(invalid.parse().unwrap(), cert, key, None),
                Err("V2_BIND_LOOPBACK_ONLY")
            );
        }
    }

    #[test]
    fn private_lan_requires_both_tls_files_distinct_peers_and_explicit_private_bind() {
        let cert = Some(Path::new("test-cert"));
        let key = Some(Path::new("test-key"));
        let sources = PeerSources {
            host: Ipv4Addr::new(192, 168, 10, 2),
            viewer: Ipv4Addr::new(192, 168, 10, 3),
        };
        // The relay can run on the host PC; its address need not be a third machine.
        let bind = "192.168.10.2:48620".parse().unwrap();
        assert_eq!(validate_bind(bind, cert, key, Some(sources)), Ok(()));
        assert_eq!(
            validate_bind(bind, None, None, Some(sources)),
            Err("V2_LAN_TLS_REQUIRED")
        );
        assert_eq!(
            validate_bind(bind, cert, None, Some(sources)),
            Err("V2_TLS_PAIR_REQUIRED")
        );
        assert_eq!(
            validate_bind(bind, Some(Path::new("")), key, Some(sources)),
            Err("V2_LAN_TLS_REQUIRED")
        );
        for invalid in [
            "0.0.0.0:48620",
            "127.0.0.1:48620",
            "192.0.2.1:48620",
            "100.64.1.1:48620",
            "169.254.1.1:48620",
            "192.168.10.2:0",
            "192.168.10.0:48620",
            "192.168.10.255:48620",
            "[::]:48620",
            "[fd00::1]:48620",
            "[::ffff:192.168.10.2]:48620",
        ] {
            assert_eq!(
                validate_bind(invalid.parse().unwrap(), cert, key, Some(sources)),
                Err("V2_LAN_BIND_PRIVATE_IPV4_REQUIRED")
            );
        }
        for viewer in [
            sources.host,
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::new(192, 0, 2, 1),
            Ipv4Addr::new(192, 168, 10, 0),
            Ipv4Addr::new(192, 168, 10, 255),
        ] {
            assert_eq!(
                validate_bind(bind, cert, key, Some(PeerSources { viewer, ..sources }),),
                Err("V2_LAN_DISTINCT_PRIVATE_PEERS_REQUIRED")
            );
        }
    }

    pub(super) fn private_app(sources: PeerSources) -> Arc<App> {
        Arc::new(App {
            access: Access::PrivateLan {
                sources,
                host_token: Zeroizing::new(TOKEN.into()),
                viewer_token: Zeroizing::new(WRONG_TOKEN.into()),
            },
            rooms: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
        })
    }

    #[test]
    fn private_lan_role_and_token_are_bound_to_exact_source() {
        let host = Ipv4Addr::new(10, 1, 2, 3);
        let viewer = Ipv4Addr::new(10, 1, 2, 4);
        let app = private_app(PeerSources { host, viewer });
        let (role, token) = app.access.credentials(host.into()).unwrap();
        assert_eq!(role, Some(Role::Host));
        assert!(token_matches(token, TOKEN));
        assert!(!token_matches(token, WRONG_TOKEN));
        let (role, token) = app.access.credentials(viewer.into()).unwrap();
        assert_eq!(role, Some(Role::Viewer));
        assert!(token_matches(token, WRONG_TOKEN));
        assert!(!token_matches(token, TOKEN));
        assert!(app
            .access
            .credentials(Ipv4Addr::new(10, 1, 2, 5).into())
            .is_none());
        assert!(app
            .access
            .credentials(host.to_ipv6_mapped().into())
            .is_none());
    }

    #[test]
    fn private_lan_requires_two_challenges_before_offer() {
        let app = private_app(PeerSources {
            host: Ipv4Addr::new(10, 1, 2, 3),
            viewer: Ipv4Addr::new(10, 1, 2, 4),
        });
        let (host_tx, _host_rx) = mpsc::channel(16);
        let (viewer_tx, _viewer_rx) = mpsc::channel(16);
        let host = Uuid::new_v4();
        let viewer = Uuid::new_v4();
        app.join(ROOM, Role::Host, host, host_tx).unwrap();
        app.join(ROOM, Role::Viewer, viewer, viewer_tx).unwrap();
        let offer = |seq| {
            app.relay(
                ROOM,
                Role::Viewer,
                viewer,
                SignalKind::Offer,
                seq,
                "fixture".into(),
            )
        };
        assert_eq!(offer(1), Err("CHALLENGES_REQUIRED"));
        app.relay(
            ROOM,
            Role::Host,
            host,
            SignalKind::HostChallenge,
            1,
            "fixture".into(),
        )
        .unwrap();
        assert_eq!(offer(1), Err("SIGNAL_ORDER"));
        app.relay(
            ROOM,
            Role::Viewer,
            viewer,
            SignalKind::ViewerChallenge,
            1,
            "fixture".into(),
        )
        .unwrap();
        assert_eq!(offer(2), Ok(()));
        app.leave(ROOM, host);
        assert!(app.rooms.lock().unwrap().is_empty());
    }

    #[test]
    fn relay_output_respects_encoded_size_limit() {
        let oversized = ServerFrame::Signal {
            from: Role::Host,
            seq: 1,
            signal: SignalKind::Answer,
            // JSON escapes this control character into six bytes each.
            data: "\u{1}".repeat(MAX_SIGNAL),
        };
        assert!(matches!(wire(&oversized), Err("SIGNAL_SIZE")));
    }

    #[test]
    fn host_owns_room_and_both_peers_share_absolute_deadline() {
        let app = app();
        let (host_tx, mut host_rx) = mpsc::channel(8);
        let (viewer_tx, mut viewer_rx) = mpsc::channel(8);
        let host = Uuid::new_v4();
        let viewer = Uuid::new_v4();
        assert_eq!(
            app.join("123", Role::Host, host, host_tx.clone()),
            Err("ROOM_FORMAT")
        );
        assert_eq!(
            app.join(ROOM, Role::Viewer, viewer, viewer_tx.clone()),
            Err("ROOM_NOT_FOUND")
        );
        let deadline = app.join(ROOM, Role::Host, host, host_tx).unwrap();
        assert_eq!(
            app.join(ROOM, Role::Viewer, viewer, viewer_tx),
            Ok(deadline)
        );
        assert_eq!(
            read_server_frame(host_rx.try_recv().unwrap()),
            json!({"type":"waiting"})
        );
        assert_eq!(
            read_server_frame(host_rx.try_recv().unwrap()),
            json!({"type":"paired"})
        );
        assert_eq!(
            read_server_frame(viewer_rx.try_recv().unwrap()),
            json!({"type":"paired"})
        );

        assert_eq!(
            app.relay(ROOM, Role::Host, host, SignalKind::Offer, 1, "TEST".into()),
            Err("SIGNAL_ROLE_DENIED")
        );
        assert_eq!(
            app.relay(
                ROOM,
                Role::Viewer,
                viewer,
                SignalKind::Ice,
                1,
                "TEST".into()
            ),
            Err("SIGNAL_ORDER")
        );
        assert_eq!(
            app.relay(ROOM, Role::Host, host, SignalKind::Answer, 1, "TEST".into()),
            Err("SIGNAL_ORDER")
        );
        assert_eq!(
            app.relay(
                ROOM,
                Role::Viewer,
                host,
                SignalKind::Offer,
                1,
                "TEST".into()
            ),
            Err("PEER_MISMATCH")
        );
        assert_eq!(
            app.relay(
                ROOM,
                Role::Viewer,
                viewer,
                SignalKind::Offer,
                1,
                "x".repeat(MAX_SIGNAL + 1)
            ),
            Err("SIGNAL_SIZE")
        );

        app.rooms.lock().unwrap().get_mut(ROOM).unwrap().deadline = Instant::now();
        assert_eq!(
            app.relay(
                ROOM,
                Role::Viewer,
                viewer,
                SignalKind::Offer,
                1,
                "TEST".into()
            ),
            Err("ROOM_EXPIRED")
        );
        app.leave(ROOM, viewer);
        assert_eq!(
            read_server_frame(host_rx.try_recv().unwrap()),
            json!({"type":"peer_left"})
        );
        assert!(app.rooms.lock().unwrap().is_empty());
    }

    fn read_server_frame(message: Message) -> Value {
        let Message::Text(text) = message else {
            panic!("expected text");
        };
        serde_json::from_str(&text).unwrap()
    }

    pub(super) async fn start_server() -> (SocketAddr, Arc<App>, tokio::task::JoinHandle<()>) {
        let app = app();
        let router = router(app.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        (address, app, task)
    }

    #[tokio::test]
    async fn missing_socket_connect_info_fails_closed_before_reserving() {
        let app = app();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        // Intentionally omit ConnectInfo, to exercise the fail-closed route.
        let broken_router = router(app.clone());
        let task = tokio::spawn(async move { axum::serve(listener, broken_router).await.unwrap() });
        let mut candidate = request(address, Some(TOKEN));
        candidate
            .headers_mut()
            .insert("x-forwarded-for", HeaderValue::from_static("127.0.0.1"));
        let result = tokio::time::timeout(Duration::from_secs(3), connect_async(candidate))
            .await
            .unwrap();
        assert!(
            matches!(result, Err(WsError::Http(response)) if response.status() == StatusCode::FORBIDDEN)
        );
        assert_eq!(app.active.load(Ordering::Acquire), 0);
        assert!(app.rooms.lock().unwrap().is_empty());
        task.abort();
    }

    fn request(address: SocketAddr, token: Option<&str>) -> Request {
        let mut request = format!("ws://{address}{PATH}")
            .into_client_request()
            .unwrap();
        if let Some(token) = token {
            request.headers_mut().insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
        request
    }

    async fn receive(client: &mut Client) -> Value {
        let message = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        serde_json::from_str(&message).unwrap()
    }

    async fn send(client: &mut Client, value: Value) {
        tokio::time::timeout(
            Duration::from_secs(3),
            client.send(WsMessage::Text(value.to_string().into())),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[tokio::test]
    async fn real_loopback_ws_rejects_missing_wrong_duplicate_and_browser_auth() {
        let (address, app, task) = start_server().await;
        let mut duplicate = request(address, Some(TOKEN));
        duplicate.headers_mut().append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer other"),
        );
        let mut browser = request(address, Some(TOKEN));
        browser.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.invalid"),
        );
        for (req, expected) in [
            (request(address, None), StatusCode::UNAUTHORIZED),
            (
                request(address, Some(WRONG_TOKEN)),
                StatusCode::UNAUTHORIZED,
            ),
            (duplicate, StatusCode::UNAUTHORIZED),
            (browser, StatusCode::FORBIDDEN),
        ] {
            let outcome = tokio::time::timeout(Duration::from_secs(3), connect_async(req))
                .await
                .unwrap();
            assert!(
                matches!(outcome, Err(WsError::Http(response)) if response.status() == expected)
            );
        }
        assert_eq!(app.active.load(Ordering::Acquire), 0);
        assert!(app.rooms.lock().unwrap().is_empty());
        task.abort();
    }

    #[tokio::test]
    async fn real_loopback_ws_handshake_relays_both_directions_and_replay_closes_room() {
        let (address, app, task) = start_server().await;
        let (mut host, _) = connect_async(request(address, Some(TOKEN))).await.unwrap();
        send(
            &mut host,
            json!({"type":"hello","version":2,"room":ROOM,"role":"host"}),
        )
        .await;
        assert_eq!(receive(&mut host).await, json!({"type":"waiting"}));

        let (mut viewer, _) = connect_async(request(address, Some(TOKEN))).await.unwrap();
        send(
            &mut viewer,
            json!({"type":"hello","version":2,"room":ROOM,"role":"viewer"}),
        )
        .await;
        for client in [&mut host, &mut viewer] {
            assert_eq!(receive(client).await, json!({"type":"paired"}));
        }

        send(
            &mut viewer,
            json!({"type":"signal","seq":1,"signal":"offer","data":"TEST_ONLY_OFFER"}),
        )
        .await;
        assert_eq!(
            receive(&mut host).await,
            json!({"type":"signal","from":"viewer","seq":1,"signal":"offer","data":"TEST_ONLY_OFFER"})
        );
        send(
            &mut host,
            json!({"type":"signal","seq":1,"signal":"answer","data":"TEST_ONLY_ANSWER"}),
        )
        .await;
        assert_eq!(
            receive(&mut viewer).await,
            json!({"type":"signal","from":"host","seq":1,"signal":"answer","data":"TEST_ONLY_ANSWER"})
        );
        let ice = json!({"type":"signal","seq":2,"signal":"ice","data":"TEST_ONLY_ICE"});
        send(&mut viewer, ice.clone()).await;
        assert_eq!(
            receive(&mut host).await,
            json!({"type":"signal","from":"viewer","seq":2,"signal":"ice","data":"TEST_ONLY_ICE"})
        );

        send(&mut viewer, ice).await;
        assert_eq!(
            receive(&mut viewer).await,
            json!({"type":"error","code":"SIGNAL_SEQUENCE"})
        );
        assert_eq!(receive(&mut host).await, json!({"type":"peer_left"}));
        assert!(app.rooms.lock().unwrap().is_empty());
        task.abort();
    }
    #[tokio::test]
    async fn native_clients_exchange_typed_loopback_signals_and_leave() {
        let (address, app, task) = start_server().await;
        let token = RelayToken::new(TOKEN.to_owned()).unwrap();
        let room = || RoomId::new(ROOM.to_owned()).unwrap();
        let wait = Duration::from_secs(3);
        let mut host = LoopbackRelay::connect(address, &token, room(), Role::Host)
            .await
            .unwrap();
        assert!(matches!(
            host.next_event(wait).await,
            Ok(RelayEvent::Waiting)
        ));
        assert_eq!(
            host.send_signal(SignalKind::Answer, "TEST_BEFORE_PAIR".into())
                .await,
            Err(ClientError::Protocol)
        );
        let mut viewer = LoopbackRelay::connect(address, &token, room(), Role::Viewer)
            .await
            .unwrap();
        for client in [&mut host, &mut viewer] {
            assert!(matches!(
                client.next_event(wait).await,
                Ok(RelayEvent::Paired)
            ));
        }
        assert_eq!(
            viewer
                .send_signal(SignalKind::Ice, "TEST_PREMATURE_ICE".into())
                .await,
            Err(ClientError::Protocol)
        );
        viewer
            .send_signal(SignalKind::Offer, "TEST_NATIVE_OFFER".into())
            .await
            .unwrap();
        assert!(matches!(host.next_event(wait).await,
            Ok(RelayEvent::Signal { kind: SignalKind::Offer, data }) if data == "TEST_NATIVE_OFFER"));
        viewer
            .send_signal(SignalKind::Ice, "TEST_VIEWER_ICE".into())
            .await
            .unwrap();
        assert!(matches!(host.next_event(wait).await,
            Ok(RelayEvent::Signal { kind: SignalKind::Ice, data }) if data == "TEST_VIEWER_ICE"));
        host.send_signal(SignalKind::Answer, "TEST_NATIVE_ANSWER".into())
            .await
            .unwrap();
        assert!(matches!(viewer.next_event(wait).await,
            Ok(RelayEvent::Signal { kind: SignalKind::Answer, data }) if data == "TEST_NATIVE_ANSWER"));
        host.send_signal(SignalKind::Ice, "TEST_HOST_ICE".into())
            .await
            .unwrap();
        assert!(matches!(viewer.next_event(wait).await,
            Ok(RelayEvent::Signal { kind: SignalKind::Ice, data }) if data == "TEST_HOST_ICE"));
        viewer.leave().await.unwrap();
        assert!(matches!(
            host.next_event(wait).await,
            Ok(RelayEvent::PeerLeft)
        ));
        assert!(matches!(
            host.next_event(wait).await,
            Err(ClientError::Closed)
        ));
        assert!(app.rooms.lock().unwrap().is_empty());
        task.abort();
    }

    #[tokio::test]
    async fn native_client_blocks_off_host_and_redacts_relay_errors() {
        let (address, app, task) = start_server().await;
        let token = RelayToken::new(TOKEN.to_owned()).unwrap();
        let wrong = RelayToken::new(WRONG_TOKEN.to_owned()).unwrap();
        let room = || RoomId::new(ROOM.to_owned()).unwrap();
        assert!(matches!(
            LoopbackRelay::connect(
                "192.0.2.1:48620".parse().unwrap(),
                &token,
                room(),
                Role::Host
            )
            .await,
            Err(ClientError::Endpoint)
        ));
        assert!(matches!(
            LoopbackRelay::connect(address, &wrong, room(), Role::Host).await,
            Err(ClientError::Unauthorized)
        ));
        let mut viewer = LoopbackRelay::connect(address, &token, room(), Role::Viewer)
            .await
            .unwrap();
        let rejected = viewer.next_event(Duration::from_secs(3)).await;
        assert!(matches!(rejected, Err(ClientError::Rejected)));
        assert!(app.rooms.lock().unwrap().is_empty());
        task.abort();

        // A test relay sends a fake sensitive value in its protocol error.
        // The client must expose only its generic local diagnostic.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let fake = Router::new().route(
            PATH,
            get(|ws: WebSocketUpgrade| async {
                ws.on_upgrade(|mut socket| async move {
                    let _ = socket.next().await;
                    let frame = ServerFrame::Error {
                        code: "TEST_SECRET_DO_NOT_ECHO".into(),
                    };
                    socket.send(wire(&frame).unwrap()).await.unwrap();
                })
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, fake).await.unwrap();
        });
        let mut client = LoopbackRelay::connect(address, &token, room(), Role::Host)
            .await
            .unwrap();
        let Err(error) = client.next_event(Duration::from_secs(3)).await else {
            panic!("expected generic relay rejection");
        };
        assert_eq!(error, ClientError::Rejected);
        assert!(!format!("{error:?} {error}").contains("TEST_SECRET_DO_NOT_ECHO"));
        task.abort();
    }
    async fn client_pair(address: SocketAddr) -> (LoopbackRelay, LoopbackRelay) {
        let token = RelayToken::new(TOKEN.to_owned()).unwrap();
        let room = || RoomId::new(ROOM.to_owned()).unwrap();
        let wait = Duration::from_secs(3);
        let mut host = LoopbackRelay::connect(address, &token, room(), Role::Host)
            .await
            .unwrap();
        assert!(matches!(
            host.next_event(wait).await,
            Ok(RelayEvent::Waiting)
        ));
        let mut viewer = LoopbackRelay::connect(address, &token, room(), Role::Viewer)
            .await
            .unwrap();
        assert!(matches!(
            host.next_event(wait).await,
            Ok(RelayEvent::Paired)
        ));
        assert!(matches!(
            viewer.next_event(wait).await,
            Ok(RelayEvent::Paired)
        ));
        (host, viewer)
    }

    #[tokio::test]
    async fn native_challenges_relay_in_either_order_or_concurrently_before_offer() {
        let (address, app, task) = start_server().await;
        let wait = Duration::from_secs(3);
        for turn in 0..3 {
            let (mut host, mut viewer) = client_pair(address).await;
            if turn == 0 {
                host.send_signal(SignalKind::HostChallenge, "TEST_HOST_CHALLENGE".into())
                    .await
                    .unwrap();
                assert!(
                    matches!(viewer.next_event(wait).await, Ok(RelayEvent::Signal { kind: SignalKind::HostChallenge, data }) if data == "TEST_HOST_CHALLENGE")
                );
                assert_eq!(
                    viewer
                        .send_signal(SignalKind::Offer, "TEST_PREMATURE_OFFER".into())
                        .await,
                    Err(ClientError::Protocol)
                );
                viewer
                    .send_signal(SignalKind::ViewerChallenge, "TEST_VIEWER_CHALLENGE".into())
                    .await
                    .unwrap();
                assert!(
                    matches!(host.next_event(wait).await, Ok(RelayEvent::Signal { kind: SignalKind::ViewerChallenge, data }) if data == "TEST_VIEWER_CHALLENGE")
                );
            } else if turn == 1 {
                viewer
                    .send_signal(SignalKind::ViewerChallenge, "TEST_VIEWER_CHALLENGE".into())
                    .await
                    .unwrap();
                assert!(
                    matches!(host.next_event(wait).await, Ok(RelayEvent::Signal { kind: SignalKind::ViewerChallenge, data }) if data == "TEST_VIEWER_CHALLENGE")
                );
                assert_eq!(
                    viewer
                        .send_signal(SignalKind::Offer, "TEST_PREMATURE_OFFER".into())
                        .await,
                    Err(ClientError::Protocol)
                );
                host.send_signal(SignalKind::HostChallenge, "TEST_HOST_CHALLENGE".into())
                    .await
                    .unwrap();
                assert!(
                    matches!(viewer.next_event(wait).await, Ok(RelayEvent::Signal { kind: SignalKind::HostChallenge, data }) if data == "TEST_HOST_CHALLENGE")
                );
            } else {
                tokio::try_join!(
                    host.send_signal(SignalKind::HostChallenge, "TEST_HOST_CHALLENGE".into()),
                    viewer.send_signal(SignalKind::ViewerChallenge, "TEST_VIEWER_CHALLENGE".into()),
                )
                .unwrap();
                assert!(matches!(
                    host.next_event(wait).await,
                    Ok(RelayEvent::Signal {
                        kind: SignalKind::ViewerChallenge,
                        ..
                    })
                ));
                assert!(matches!(
                    viewer.next_event(wait).await,
                    Ok(RelayEvent::Signal {
                        kind: SignalKind::HostChallenge,
                        ..
                    })
                ));
            }
            assert_eq!(
                host.send_signal(SignalKind::HostChallenge, "TEST_DUPLICATE_CHALLENGE".into())
                    .await,
                Err(ClientError::Protocol)
            );
            viewer
                .send_signal(SignalKind::Offer, "TEST_OFFER_AFTER_BOTH".into())
                .await
                .unwrap();
            assert!(
                matches!(host.next_event(wait).await, Ok(RelayEvent::Signal { kind: SignalKind::Offer, data }) if data == "TEST_OFFER_AFTER_BOTH")
            );
            host.send_signal(SignalKind::Answer, "TEST_ANSWER_AFTER_BOTH".into())
                .await
                .unwrap();
            assert!(
                matches!(viewer.next_event(wait).await, Ok(RelayEvent::Signal { kind: SignalKind::Answer, data }) if data == "TEST_ANSWER_AFTER_BOTH")
            );
            viewer.leave().await.unwrap();
            assert!(matches!(
                host.next_event(wait).await,
                Ok(RelayEvent::PeerLeft)
            ));
            assert!(app.rooms.lock().unwrap().is_empty());
        }
        task.abort();
    }

    #[tokio::test]
    async fn raw_ws_cannot_bypass_challenge_roles_or_offer_order() {
        let (address, app, task) = start_server().await;
        for bad in ["wrong-role", "duplicate", "premature"] {
            let (mut host, _) = connect_async(request(address, Some(TOKEN))).await.unwrap();
            send(
                &mut host,
                json!({"type":"hello","version":2,"room":ROOM,"role":"host"}),
            )
            .await;
            assert_eq!(receive(&mut host).await, json!({"type":"waiting"}));
            let (mut viewer, _) = connect_async(request(address, Some(TOKEN))).await.unwrap();
            send(
                &mut viewer,
                json!({"type":"hello","version":2,"room":ROOM,"role":"viewer"}),
            )
            .await;
            assert_eq!(receive(&mut host).await, json!({"type":"paired"}));
            assert_eq!(receive(&mut viewer).await, json!({"type":"paired"}));
            if bad == "wrong-role" {
                send(
                    &mut viewer,
                    json!({"type":"signal","seq":1,"signal":"host_challenge","data":"TEST"}),
                )
                .await;
                assert_eq!(
                    receive(&mut viewer).await,
                    json!({"type":"error","code":"SIGNAL_ROLE_DENIED"})
                );
                assert_eq!(receive(&mut host).await, json!({"type":"peer_left"}));
            } else {
                send(
                    &mut viewer,
                    json!({"type":"signal","seq":1,"signal":"viewer_challenge","data":"TEST"}),
                )
                .await;
                assert_eq!(
                    receive(&mut host).await,
                    json!({"type":"signal","from":"viewer","seq":1,"signal":"viewer_challenge","data":"TEST"})
                );
                let second = if bad == "duplicate" {
                    "viewer_challenge"
                } else {
                    "offer"
                };
                send(
                    &mut viewer,
                    json!({"type":"signal","seq":2,"signal":second,"data":"TEST"}),
                )
                .await;
                assert_eq!(
                    receive(&mut viewer).await,
                    json!({"type":"error","code":"SIGNAL_ORDER"})
                );
                assert_eq!(receive(&mut host).await, json!({"type":"peer_left"}));
            }
            assert!(app.rooms.lock().unwrap().is_empty());
        }
        task.abort();
    }
}

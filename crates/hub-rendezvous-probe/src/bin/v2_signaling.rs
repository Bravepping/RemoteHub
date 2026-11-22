//! RemoteHub V2 P1: ephemeral, authenticated signaling only. No media or input API.
use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use uuid::Uuid;
use zeroize::Zeroizing;

const MAX_WIRE: usize = 64 * 1024;
const MAX_SIGNAL: usize = 32 * 1024;
const MAX_ROOMS: usize = 4;
const MAX_CONNECTIONS: usize = 8;
const ROOM_LIFETIME: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Role { Host, Viewer }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SignalKind { Offer, Answer, Ice }

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientFrame {
    Hello { version: u8, room: String, role: Role },
    Signal { seq: u64, signal: SignalKind, data: String },
    Leave,
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerFrame {
    Waiting,
    Paired,
    Signal { from: Role, seq: u64, signal: SignalKind, data: String },
    PeerLeft,
    Error { code: &'static str },
}
fn wire(frame: &ServerFrame) -> Result<Message, &'static str> {
    Ok(Message::Text(serde_json::to_string(frame).map_err(|_| "ENCODE_FAILED")?.into()))
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
        Self { id, tx, last_seq: 0, window: Instant::now(), events: 0 }
    }
    fn accept_seq(&mut self, seq: u64) -> Result<(), &'static str> {
        if seq == 0 || seq != self.last_seq + 1 { return Err("SIGNAL_SEQUENCE"); }
        if self.window.elapsed() >= Duration::from_secs(1) {
            self.window = Instant::now();
            self.events = 0;
        }
        if self.events >= 60 { return Err("SIGNAL_RATE_LIMIT"); }
        self.events += 1;
        self.last_seq = seq;
        Ok(())
    }
}
struct Room {
    created: Instant,
    host: Option<Peer>,
    viewer: Option<Peer>,
}
struct App {
    token: Zeroizing<String>,
    rooms: Mutex<HashMap<String, Room>>,
    active: AtomicUsize,
}
impl App {
    fn reserve(&self) -> bool {
        let mut n = self.active.load(Ordering::Acquire);
        loop {
            if n >= MAX_CONNECTIONS { return false; }
            match self.active.compare_exchange_weak(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return true,
                Err(actual) => n = actual,
            }
        }
    }
    fn join(&self, key: &str, role: Role, id: Uuid, tx: mpsc::Sender<Message>) -> Result<(), &'static str> {
        if key.len() != 32 || !key.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Err("ROOM_FORMAT");
        }
        let mut rooms = self.rooms.lock().map_err(|_| "STATE_UNAVAILABLE")?;
        if !rooms.contains_key(key) {
            if role != Role::Host { return Err("ROOM_NOT_FOUND"); }
            if rooms.len() >= MAX_ROOMS { return Err("ROOM_LIMIT"); }
            rooms.insert(key.to_owned(), Room { created: Instant::now(), host: None, viewer: None });
        }
        let room = rooms.get_mut(key).ok_or("ROOM_NOT_FOUND")?;
        if room.created.elapsed() >= ROOM_LIFETIME { return Err("ROOM_EXPIRED"); }
        let slot = match role { Role::Host => &mut room.host, Role::Viewer => &mut room.viewer };
        if slot.is_some() { return Err("ROLE_OCCUPIED"); }
        *slot = Some(Peer::new(id, tx));
        if room.host.is_some() && room.viewer.is_some() {
            let msg = wire(&ServerFrame::Paired)?;
            for peer in [room.host.as_ref(), room.viewer.as_ref()].into_iter().flatten() {
                peer.tx.try_send(msg.clone()).map_err(|_| "CLIENT_BACKPRESSURE")?;
            }
        } else {
            slot.as_ref().ok_or("ROLE_MISSING")?.tx.try_send(wire(&ServerFrame::Waiting)?)
                .map_err(|_| "CLIENT_BACKPRESSURE")?;
        }
        Ok(())
    }
    fn relay(&self, key: &str, role: Role, id: Uuid, signal: SignalKind, seq: u64, data: String)
        -> Result<(), &'static str>
    {
        if data.is_empty() || data.len() > MAX_SIGNAL || data.contains('\0') {
            return Err("SIGNAL_SIZE");
        }
        if matches!((role, signal), (Role::Host, SignalKind::Offer) | (Role::Viewer, SignalKind::Answer)) {
            return Err("SIGNAL_ROLE_DENIED");
        }
        let mut rooms = self.rooms.lock().map_err(|_| "STATE_UNAVAILABLE")?;
        let room = rooms.get_mut(key).ok_or("ROOM_NOT_FOUND")?;
        if room.created.elapsed() >= ROOM_LIFETIME { return Err("ROOM_EXPIRED"); }
        let (sender, target) = match role {
            Role::Host => (&mut room.host, &room.viewer),
            Role::Viewer => (&mut room.viewer, &room.host),
        };
        let from = sender.as_mut().ok_or("ROLE_MISSING")?;
        if from.id != id { return Err("PEER_MISMATCH"); }
        from.accept_seq(seq)?;
        let to = target.as_ref().ok_or("PEER_NOT_JOINED")?;
        to.tx.try_send(wire(&ServerFrame::Signal { from: role, seq, signal, data })?)
            .map_err(|_| "CLIENT_BACKPRESSURE")
    }
    fn leave(&self, key: &str, id: Uuid) {
        let Ok(mut rooms) = self.rooms.lock() else { return; };
        let owned = rooms.get(key).is_some_and(|room| {
            room.host.as_ref().is_some_and(|p| p.id == id) ||
            room.viewer.as_ref().is_some_and(|p| p.id == id)
        });
        if owned {
            if let Some(room) = rooms.remove(key) {
                if let Ok(msg) = wire(&ServerFrame::PeerLeft) {
                    for peer in [room.host, room.viewer].into_iter().flatten() {
                        if peer.id != id { let _ = peer.tx.try_send(msg.clone()); }
                    }
                }
            }
        }
    }
}
struct Lease(Arc<App>);
impl Drop for Lease {
    fn drop(&mut self) { self.0.active.fetch_sub(1, Ordering::AcqRel); }
}
fn token_matches(expected: &str, given: &str) -> bool {
    given.len() == 64 &&
        ring::constant_time::verify_slices_are_equal(expected.as_bytes(), given.as_bytes()).is_ok()
}
async fn upgrade(State(app): State<Arc<App>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let candidate = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    if !token_matches(&app.token, candidate) { return StatusCode::UNAUTHORIZED.into_response(); }
    if !app.reserve() { return StatusCode::TOO_MANY_REQUESTS.into_response(); }
    let guard = Lease(app.clone());
    ws.max_message_size(MAX_WIRE).max_frame_size(MAX_WIRE)
        .on_upgrade(move |socket| async move { let _guard = guard; connection(socket, app).await; })
        .into_response()
}
async fn send_error(socket: &mut WebSocket, code: &'static str) {
    if let Ok(message) = wire(&ServerFrame::Error { code }) {
        let _ = tokio::time::timeout(Duration::from_secs(1), socket.send(message)).await;
    }
}
async fn connection(mut socket: WebSocket, app: Arc<App>) {
    let first = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
    let Some(Ok(Message::Text(first))) = first.ok().flatten() else {
        send_error(&mut socket, "HELLO_REQUIRED").await;
        return;
    };
    let Ok(ClientFrame::Hello { version: 2, room, role }) = serde_json::from_str::<ClientFrame>(&first) else {
        send_error(&mut socket, "HELLO_INVALID").await;
        return;
    };
    let id = Uuid::new_v4();
    let (tx, mut rx) = mpsc::channel(16);
    if let Err(code) = app.join(&room, role, id, tx) {
        app.leave(&room, id);
        send_error(&mut socket, code).await;
        return;
    }
    let started = Instant::now();
    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(message) = outgoing else { break; };
                if tokio::time::timeout(Duration::from_secs(2), socket.send(message)).await
                    .is_err() { break; }
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let frame = serde_json::from_str::<ClientFrame>(&text);
                        match frame {
                            Ok(ClientFrame::Signal { seq, signal, data }) => {
                                if let Err(code) = app.relay(&room, role, id, signal, seq, data) {
                                    send_error(&mut socket, code).await;
                                    break;
                                }
                            }
                            Ok(ClientFrame::Leave) => break,
                            _ => { send_error(&mut socket, "FRAME_INVALID").await; break; }
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    _ => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if started.elapsed() > ROOM_LIFETIME { break; }
            }
        }
    }
    app.leave(&room, id);
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close()).await;
}
async fn run() -> Result<(), &'static str> {
    let token = std::env::var("REMOTEHUB_V2_TOKEN").map_err(|_| "V2_TOKEN_REQUIRED")?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("V2_TOKEN_MUST_BE_64_HEX");
    }
    let addr: SocketAddr = std::env::var("REMOTEHUB_V2_BIND")
        .unwrap_or_else(|_| "127.0.0.1:48620".into())
        .parse().map_err(|_| "V2_BIND_INVALID")?;
    let cert = std::env::var("REMOTEHUB_V2_TLS_CERT").ok();
    let key = std::env::var("REMOTEHUB_V2_TLS_KEY").ok();
    if cert.is_some() != key.is_some() { return Err("V2_TLS_PAIR_REQUIRED"); }
    if !addr.ip().is_loopback() && cert.is_none() { return Err("V2_PUBLIC_BIND_REQUIRES_TLS"); }
    let app = Arc::new(App {
        token: Zeroizing::new(token),
        rooms: Mutex::new(HashMap::new()),
        active: AtomicUsize::new(0),
    });
    let router = Router::new().route("/healthz", get(|| async { "ok" }))
        .route("/v2/ws", get(upgrade)).with_state(app);
    if let (Some(cert), Some(key)) = (cert, key) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await
            .map_err(|_| "V2_TLS_CONFIG_FAILED")?;
        axum_server::bind_rustls(addr, tls).serve(router.into_make_service()).await
            .map_err(|_| "V2_SERVER_FAILED")
    } else {
        axum_server::bind(addr).serve(router.into_make_service()).await
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
mod tests {
    use super::*;
    #[test]
    fn token_requires_exact_secret() {
        let s = "a".repeat(64);
        assert!(token_matches(&s, &s));
        assert!(!token_matches(&s, &"b".repeat(64)));
        assert!(!token_matches(&s, "a"));
    }
    #[test]
    fn two_roles_relay_and_reject_wrong_role_replay() {
        let app = App { token: Zeroizing::new("a".repeat(64)),
            rooms: Mutex::new(HashMap::new()), active: AtomicUsize::new(0) };
        let room = "0123456789abcdef0123456789abcdef";
        let (host_tx, mut host_rx) = mpsc::channel(8);
        let (viewer_tx, mut viewer_rx) = mpsc::channel(8);
        let host_id = Uuid::new_v4();
        let viewer_id = Uuid::new_v4();
        assert!(app.join(room, Role::Viewer, viewer_id, viewer_tx.clone()).is_err());
        app.join(room, Role::Host, host_id, host_tx).unwrap();
        app.join(room, Role::Viewer, viewer_id, viewer_tx).unwrap();
        assert!(host_rx.try_recv().is_ok()); // waiting
        assert!(host_rx.try_recv().is_ok()); // paired
        assert!(viewer_rx.try_recv().is_ok()); // paired
        assert!(app.relay(room, Role::Viewer, viewer_id, SignalKind::Offer, 1, "P1_TEST_ONLY".into()).is_ok());
        assert!(host_rx.try_recv().is_ok());
        assert!(app.relay(room, Role::Viewer, viewer_id, SignalKind::Offer, 1, "REPLAY".into()).is_err());
        assert!(app.relay(room, Role::Host, host_id, SignalKind::Offer, 1, "WRONG_ROLE".into()).is_err());
        app.leave(room, viewer_id);
        assert!(host_rx.try_recv().is_ok()); // peer_left
        assert!(app.rooms.lock().unwrap().is_empty());
    }
    #[test]
    fn room_id_and_signal_validation_are_strict() {
        let app = App { token: Zeroizing::new("a".repeat(64)),
            rooms: Mutex::new(HashMap::new()), active: AtomicUsize::new(0) };
        let (tx, _) = mpsc::channel(2);
        assert!(app.join("123", Role::Host, Uuid::new_v4(), tx.clone()).is_err());
        assert!(app.join(&"a".repeat(32), Role::Viewer, Uuid::new_v4(), tx).is_err());
    }
}

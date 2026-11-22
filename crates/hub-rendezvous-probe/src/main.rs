//! Explicit short-lived experimental WSS relay. No media or input events are proxied.
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use hub_probe_wire::{now, Frame, Hello, IceServer, Ready, Role, Sequence, ServerEvent, MAX_FRAME};
use hub_turn_credentials::{ApprovedSession, Issuer, Request, Role as CredentialRole};
use serde::Deserialize;
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use zeroize::Zeroize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: SocketAddr,
    public_ipv4: Ipv4Addr,
    sender_invitation: String,
    receiver_invitation: String,
    invitation_expires_at: u64,
    realm: String,
    certificate: String,
    private_key: String,
}
impl Drop for Config {
    fn drop(&mut self) {
        self.sender_invitation.zeroize();
        self.receiver_invitation.zeroize();
    }
}
struct Room {
    mode: hub_probe_wire::PathMode,
    session: String,
    nonce: String,
    expires: u64,
    created: Instant,
    seconds: u32,
    channels: [Option<mpsc::Sender<Message>>; 2],
    sequence: [Sequence; 2],
    approved: ApprovedSession,
    ready: bool,
}
struct App {
    config: Config,
    issuer: Issuer,
    rooms: Mutex<HashMap<String, Room>>,
    retired: Mutex<Vec<String>>,
    connections: AtomicUsize,
}
struct ConnectionLease(Arc<App>);
impl Drop for ConnectionLease {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::AcqRel);
    }
}
fn index(role: Role) -> usize {
    if role == Role::Sender {
        0
    } else {
        1
    }
}
fn event(e: ServerEvent) -> Result<Message, String> {
    Ok(Message::Text(
        serde_json::to_string(&e)
            .map_err(|_| "SERVER_ENCODING")?
            .into(),
    ))
}
fn invitation_matches(expected: &str, supplied: &str) -> bool {
    if supplied.len() != 64 {
        return false;
    }
    let message = b"RemoteHub.P006b.Invitation";
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, expected.as_bytes()),
        message,
    );
    ring::hmac::verify(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, supplied.as_bytes()),
        message,
        tag.as_ref(),
    )
    .is_ok()
}
async fn upgrade(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if now() >= app.config.invitation_expires_at
        || app.connections.load(Ordering::Acquire) >= 8
        || headers.get("origin").and_then(|x| x.to_str().ok()) != Some("remotehub-p006b")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token = headers
        .get("authorization")
        .and_then(|x| x.to_str().ok())
        .and_then(|x| x.strip_prefix("Bearer "))
        .unwrap_or_default();
    let role = if invitation_matches(&app.config.sender_invitation, token) {
        Role::Sender
    } else if invitation_matches(&app.config.receiver_invitation, token) {
        Role::Receiver
    } else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !app.reserve_connection() {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let lease = ConnectionLease(app.clone());
    ws.max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| async move {
            let _lease = lease;
            let _ = connection(socket, app.clone(), role).await;
        })
        .into_response()
}
impl App {
    fn reserve_connection(&self) -> bool {
        let mut current = self.connections.load(Ordering::Acquire);
        loop {
            if current >= 8 {
                return false;
            }
            match self.connections.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }
    fn leave(&self, id: &str, code: &str) {
        let room = self
            .rooms
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .remove(id);
        if let Some(mut r) = room {
            r.approved.revoke();
            for tx in r.channels.into_iter().flatten() {
                if let Ok(m) = event(ServerEvent::Failed { code: code.into() }) {
                    let _ = tx.try_send(m);
                }
            }
            let mut retired = self.retired.lock().unwrap_or_else(|x| x.into_inner());
            retired.push(id.into());
        }
    }
    fn join(&self, h: &Hello, tx: mpsc::Sender<Message>) -> Result<(), String> {
        h.validate()?;
        if now() >= self.config.invitation_expires_at
            || self
                .retired
                .lock()
                .unwrap_or_else(|x| x.into_inner())
                .iter()
                .any(|id| id == &h.room)
        {
            return Err("ROOM_RETIRED".into());
        }
        let mut rooms = self.rooms.lock().unwrap_or_else(|x| x.into_inner());
        // A bounded experiment may retire at most 256 rooms before new pairing is disabled.
        if self.retired.lock().unwrap_or_else(|x| x.into_inner()).len() >= 256 {
            return Err("EXPERIMENT_ROOM_BUDGET".into());
        }
        if h.role == Role::Sender {
            if rooms.contains_key(&h.room) || rooms.len() >= 2 {
                return Err("ROOM_LIMIT_OR_USED".into());
            }
            let session = uuid::Uuid::new_v4().to_string();
            let expires = (now()
                + if h.mode == hub_probe_wire::PathMode::DirectPreferred {
                    300
                } else {
                    180
                })
            .min(self.config.invitation_expires_at);
            if expires.saturating_sub(now()) < 60 {
                return Err("INVITATION_EXPIRED".into());
            }
            let mut approved = ApprovedSession::from_authenticated_context(true, &session, expires)
                .map_err(str::to_owned)?;
            approved.set_manual_approval(CredentialRole::Host, true);
            rooms.insert(
                h.room.clone(),
                Room {
                    mode: h.mode,
                    session,
                    nonce: uuid::Uuid::new_v4().to_string(),
                    expires,
                    created: Instant::now(),
                    seconds: h.seconds,
                    channels: [Some(tx.clone()), None],
                    sequence: [Sequence::default(), Sequence::default()],
                    approved,
                    ready: false,
                },
            );
            tx.try_send(event(ServerEvent::Waiting)?)
                .map_err(|_| "PEER_SLOW")?;
        } else {
            let r = rooms.get_mut(&h.room).ok_or("ROOM_NOT_FOUND")?;
            if r.mode != h.mode {
                return Err("PATH_MODE_MISMATCH".into());
            }
            if r.channels[1].is_some()
                || r.seconds != h.seconds
                || r.created.elapsed() > Duration::from_secs(120)
            {
                return Err("ROOM_USED_OR_EXPIRED".into());
            }
            r.channels[1] = Some(tx);
            r.approved
                .set_manual_approval(CredentialRole::Controller, true);
            // Both authenticated role connections originate after their native manual consent.
            let issued_at = now();
            let ttl = r.expires.saturating_sub(issued_at);
            for (i, role) in [CredentialRole::Host, CredentialRole::Controller]
                .into_iter()
                .enumerate()
            {
                let t = self
                    .issuer
                    .issue(
                        &mut r.approved,
                        Request {
                            session_id: &r.session,
                            realm: &self.config.realm,
                            role,
                            seq: 1,
                            issued_at,
                            ttl,
                        },
                        issued_at,
                    )
                    .map_err(str::to_owned)?;
                let data = Ready {
                    session_id: r.session.clone(),
                    nonce: r.nonce.clone(),
                    expires_at: t.expires_at,
                    ice_servers: vec![IceServer {
                        urls: vec![format!(
                            "turn:{}:3478?transport=udp",
                            self.config.public_ipv4
                        )],
                        username: t.username().into(),
                        credential: t.credential().into(),
                    }],
                };
                r.channels[i]
                    .as_ref()
                    .ok_or("PEER_MISSING")?
                    .try_send(event(ServerEvent::Ready(data))?)
                    .map_err(|_| "PEER_SLOW")?;
            }
            r.ready = true;
        }
        Ok(())
    }
    fn forward(&self, id: &str, role: Role, text: String) -> Result<bool, String> {
        if text.len() > MAX_FRAME {
            return Err("FRAME_SIZE".into());
        }
        let frame: Frame = serde_json::from_str(&text).map_err(|_| "FRAME_SCHEMA")?;
        let mut rooms = self.rooms.lock().unwrap_or_else(|x| x.into_inner());
        let r = rooms.get_mut(id).ok_or("ROOM_CLOSED")?;
        if !r.ready
            || (frame.kind == "path" && r.mode != hub_probe_wire::PathMode::DirectPreferred)
            || (frame.kind == "offer" && role != Role::Receiver)
            || (frame.kind == "answer" && role != Role::Sender)
            || (frame.kind == "metrics" && role != Role::Receiver)
        {
            return Err("ROLE_NOT_ALLOWED".into());
        }
        r.sequence[index(role)].check(&frame, &r.session, &r.nonce, role, r.expires, now())?;
        r.channels[index(role.opposite())]
            .as_ref()
            .ok_or("PEER_MISSING")?
            .try_send(Message::Text(text.into()))
            .map_err(|_| "PEER_SLOW")?;
        Ok(frame.kind == "stop")
    }
}
async fn connection(mut ws: WebSocket, app: Arc<App>, role: Role) -> Result<(), String> {
    let first = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .map_err(|_| "HELLO_TIMEOUT")?
        .ok_or("HELLO_DISCONNECTED")?
        .map_err(|_| "HELLO_DISCONNECTED")?;
    let Message::Text(text) = first else {
        return Err("HELLO_REQUIRED".into());
    };
    let h: Hello = serde_json::from_str(&text).map_err(|_| "HELLO_SCHEMA")?;
    if h.role != role {
        return Err("INVITATION_ROLE_MISMATCH".into());
    }
    let (tx, mut rx) = mpsc::channel(8);
    if let Err(code) = app.join(&h, tx) {
        let _ = ws
            .send(event(ServerEvent::Failed { code: code.clone() })?)
            .await;
        let _ = ws.close().await;
        return Err(code);
    }
    let result=async {
        loop {tokio::select! {
            message=rx.recv()=>{let Some(message)=message else{break;};let fail=matches!(&message,Message::Text(s) if s.contains("\"event\":\"failed\""));tokio::time::timeout(Duration::from_secs(2),ws.send(message)).await.map_err(|_|"WSS_WRITE_TIMEOUT")?.map_err(|_|"WSS_WRITE_FAILED")?;if fail{break;}}
            input=ws.next()=>{match input{Some(Ok(Message::Text(s)))=>{if app.forward(&h.room,role,s.to_string())?{break;}},Some(Ok(Message::Close(_)))|None=>break,Some(Ok(Message::Ping(_)|Message::Pong(_)))=>{},_=>return Err("WSS_FRAME_REJECTED".to_string())}}
        }}Ok::<(),String>(())
    }.await;
    app.leave(
        &h.room,
        if result.is_ok() {
            "PEER_STOPPED"
        } else {
            "PEER_DISCONNECTED"
        },
    );
    let _ = tokio::time::timeout(Duration::from_secs(1), ws.close()).await;
    result
}
async fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 || args[1] != "--private-config" {
        return Err("EXPLICIT_PRIVATE_CONFIG_REQUIRED".into());
    }
    let text = zeroize::Zeroizing::new(
        std::fs::read_to_string(&args[2]).map_err(|_| "CONFIG_UNAVAILABLE")?,
    );
    if text.len() > 16384 {
        return Err("CONFIG_SIZE".into());
    }
    let config: Config = serde_json::from_str(&text).map_err(|_| "CONFIG_SCHEMA")?;
    if config.listen.port() < 1024
        || config.sender_invitation.len() != 64
        || config.receiver_invitation.len() != 64
        || config.sender_invitation == config.receiver_invitation
        || !config
            .sender_invitation
            .bytes()
            .chain(config.receiver_invitation.bytes())
            .all(|b| b.is_ascii_hexdigit())
        || !(60..=7200).contains(&config.invitation_expires_at.saturating_sub(now()))
    {
        return Err("CONFIG_POLICY".into());
    }
    let issuer = Issuer::from_server_environment(config.realm.clone()).map_err(str::to_owned)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &config.certificate,
        &config.private_key,
    )
    .await
    .map_err(|_| "TLS_CONFIG_FAILED")?;
    let listen = config.listen;
    let app = Arc::new(App {
        config,
        issuer,
        rooms: Mutex::new(HashMap::new()),
        retired: Mutex::new(Vec::new()),
        connections: AtomicUsize::new(0),
    });
    let cleanup = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let expired: Vec<_> = cleanup
                .rooms
                .lock()
                .unwrap_or_else(|x| x.into_inner())
                .iter()
                .filter(|(_, r)| {
                    now() >= r.expires
                        || (!r.ready && r.created.elapsed() > Duration::from_secs(120))
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in expired {
                cleanup.leave(&id, "ROOM_EXPIRED");
            }
        }
    });
    let router = Router::new().route("/probe", get(upgrade)).with_state(app);
    axum_server::bind_rustls(listen, tls)
        .serve(router.into_make_service())
        .await
        .map_err(|_| "WSS_SERVER_FAILED".into())
}
#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("{code}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invitation_is_role_specific_and_bounded() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        assert!(invitation_matches(&a, &a));
        assert!(!invitation_matches(&a, &b));
        assert!(!invitation_matches(&a, ""));
    }
    #[test]
    fn hello_rejects_invalid_duration_and_room() {
        let mut h = Hello {
            mode: Default::default(),
            version: 1,
            role: Role::Sender,
            room: "a".repeat(64),
            seconds: 30,
        };
        assert!(h.validate().is_ok());
        h.seconds = 31;
        assert!(h.validate().is_err());
        h.seconds = 30;
        h.room = "x".repeat(65);
        assert!(h.validate().is_err());
    }
}

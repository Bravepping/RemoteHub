use crate::{
    lan_wire::{self, Codec, Key},
    scope::{LanScope, NetworkScope},
    session::{Session, Stats},
    signaling::validate_description,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpListener, TcpSocket, TcpStream},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{sleep, timeout},
};

#[derive(Clone, Serialize)]
pub struct LanStatus {
    pub browser_control: Option<crate::control::BrowserStats>,
    pub role: String,
    pub state: String,
    pub elapsed_ms: u64,
    pub authenticated: bool,
    pub messages_received: u32,
    pub source: Option<Stats>,
    pub receiver_samples: Vec<Value>,
    pub resources_released: bool,
    pub error: Option<String>,
}
// Finish is a graceful notice, not proof of resources released or permission to
// hide errors. Only verified LAN input gets advance notice during media drain.
fn should_announce_finish(stats: &Stats, input_mode: bool) -> bool {
    stats.error.is_none()
        && stats
            .control
            .as_ref()
            .is_none_or(|control| control.last_error.is_none())
        && (stats.resources_released
            || (input_mode && stats.state == "draining" && stats.frames_sent > 0))
}

/// Caller-pinned peer and local identity for one explicitly approved video
/// session. Pairing does not grant permission to share screen or input.
#[cfg(feature = "v2-verified-lan")]
pub struct VerifiedLanConfig {
    pub local_identity: Arc<hub_identity::Identity>,
    pub peer: Arc<hub_identity::pairing::VerifiedPeer>,
}

#[cfg(feature = "v2-verified-lan")]
impl VerifiedLanConfig {
    fn validate(&self, scope: LanScope) -> Result<(), String> {
        LanScope::new(scope.local, scope.peer)?;
        if !self.peer.is_for(&self.local_identity.public()) {
            return Err("LAN_VERIFIED_PEER_BINDING".into());
        }
        Ok(())
    }
}

#[cfg(feature = "v2-private-wss-lan")]
#[path = "private_wss_lan.rs"]
mod private_wss;
#[cfg(feature = "v2-private-wss-lan")]
pub use private_wss::{LoopbackTlsTrust, PrivateWssLanConfig, RelayToken};

#[cfg(feature = "v2-verified-lan")]
fn validate_verified_sdp(sdp: &str, scope: LanScope, answer: bool) -> Result<(), String> {
    let negotiation = validate_description(sdp, NetworkScope::Lan(scope), answer)?;
    if !sdp.lines().map(str::trim).any(|line| line == "a=rtcp-mux") {
        return Err("LAN_VERIFIED_RTCP_MUX_REQUIRED".into());
    }
    // All candidates were checked against the selected interface above.
    // A component-2-only or empty inline set cannot connect video.
    if negotiation.candidate_types.is_empty()
        || !sdp.lines().map(str::trim).any(|line| {
            line.strip_prefix("a=candidate:")
                .and_then(|candidate| candidate.split_whitespace().nth(1))
                == Some("1")
        })
    {
        return Err("LAN_VERIFIED_INLINE_CANDIDATE_REQUIRED".into());
    }
    // Repeated identical SHA-256 fingerprints are harmless; alternative
    // digests or algorithms make the DTLS identity ambiguous.
    let mut fingerprints = sdp
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("a=fingerprint:"));
    let fingerprint = fingerprints
        .next()
        .ok_or("SHA256_DTLS_FINGERPRINT_REQUIRED")?;
    if !fingerprint.starts_with("sha-256 ")
        || fingerprints.any(|next| !fingerprint.eq_ignore_ascii_case(next))
    {
        return Err("LAN_VERIFIED_DTLS_FINGERPRINT_AMBIGUOUS".into());
    }
    Ok(())
}

#[cfg(feature = "v2-verified-lan")]
fn reversed_scope(scope: LanScope) -> LanScope {
    LanScope {
        local: scope.peer,
        peer: scope.local,
    }
}

#[cfg(feature = "v2-verified-lan")]
fn check_verified_socket(
    socket: &TcpStream,
    scope: LanScope,
    host_port: u16,
    host: bool,
) -> Result<(), String> {
    let local = socket.local_addr().map_err(|_| "LAN_LOCAL_ADDRESS")?;
    let peer = socket.peer_addr().map_err(|_| "LAN_PEER_ADDRESS")?;
    if local.ip() != std::net::IpAddr::V4(scope.local)
        || peer.ip() != std::net::IpAddr::V4(scope.peer)
        || (if host { local.port() } else { peer.port() }) != host_port
    {
        return Err("LAN_VERIFIED_SOCKET_SCOPE".into());
    }
    Ok(())
}

pub struct LanSession {
    #[cfg(feature = "p3-lan")]
    input_enabled: AtomicBool,
    control_enabled: bool,
    control_session: Mutex<Option<String>>,
    scope: LanScope,
    seconds: u32,
    port: u16,
    role: &'static str,
    created: Instant,
    cancelled: AtomicBool,
    started: AtomicBool,
    key: Mutex<Option<Arc<Key>>>,
    #[cfg(feature = "v2-verified-lan")]
    verified: Mutex<Option<VerifiedLanConfig>>,
    #[cfg(feature = "v2-private-wss-lan")]
    private_wss: Mutex<Option<PrivateWssLanConfig>>,
    source: Mutex<Option<Arc<Session>>>,
    status: Mutex<LanStatus>,
    task: Mutex<Option<JoinHandle<()>>>,
    stop_lock: tokio::sync::Mutex<()>,
    latest_metrics: Mutex<Option<String>>,
}
impl LanSession {
    fn new(
        scope: LanScope,
        seconds: u32,
        port: u16,
        role: &'static str,
        key: Key,
    ) -> Result<Arc<Self>, String> {
        Self::new_mode(scope, seconds, port, role, key, false)
    }
    fn new_mode(
        scope: LanScope,
        seconds: u32,
        port: u16,
        role: &'static str,
        key: Key,
        control_enabled: bool,
    ) -> Result<Arc<Self>, String> {
        if !(5..=30).contains(&seconds) || port < 1024 {
            return Err("INVALID_LAN_CONFIG".into());
        }
        Ok(Arc::new(Self {
            #[cfg(feature = "p3-lan")]
            input_enabled: AtomicBool::new(false),
            control_enabled,
            control_session: Mutex::new(if control_enabled && role == "receiver" {
                Some(uuid::Uuid::new_v4().to_string())
            } else {
                None
            }),
            scope,
            seconds,
            port,
            role,
            created: Instant::now(),
            cancelled: AtomicBool::new(false),
            started: AtomicBool::new(false),
            key: Mutex::new(Some(Arc::new(key))),
            #[cfg(feature = "v2-verified-lan")]
            verified: Mutex::new(None),
            #[cfg(feature = "v2-private-wss-lan")]
            private_wss: Mutex::new(None),
            source: Mutex::new(None),
            status: Mutex::new(LanStatus {
                browser_control: None,
                role: role.into(),
                state: "waiting".into(),
                elapsed_ms: 0,
                authenticated: false,
                messages_received: 0,
                source: None,
                receiver_samples: vec![],
                resources_released: false,
                error: None,
            }),
            task: Mutex::new(None),
            stop_lock: tokio::sync::Mutex::new(()),
            latest_metrics: Mutex::new(None),
        }))
    }
    pub async fn host(
        scope: LanScope,
        seconds: u32,
        port: u16,
    ) -> Result<(Arc<Self>, String), String> {
        Self::host_mode(scope, seconds, port, false, false).await
    }
    pub async fn host_control(
        scope: LanScope,
        seconds: u32,
        port: u16,
    ) -> Result<(Arc<Self>, String), String> {
        Self::host_mode(scope, seconds, port, true, false).await
    }
    #[cfg(feature = "p3-lan")]
    pub async fn host_input(
        scope: LanScope,
        seconds: u32,
        port: u16,
    ) -> Result<(Arc<Self>, String), String> {
        Self::host_mode(scope, seconds, port, true, true).await
    }
    #[cfg(feature = "p3-lan")]
    pub fn receiver_input(
        scope: LanScope,
        seconds: u32,
        port: u16,
        code: &str,
    ) -> Result<Arc<Self>, String> {
        let this = Self::receiver_control(scope, seconds, port, code)?;
        this.input_enabled.store(true, Ordering::Release);
        Ok(this)
    }
    async fn host_mode(
        scope: LanScope,
        seconds: u32,
        port: u16,
        enabled: bool,
        input: bool,
    ) -> Result<(Arc<Self>, String), String> {
        let key = Key::generate();
        let display = key.display();
        let this = Self::new_mode(scope, seconds, port, "sender", key, enabled)?;
        #[cfg(feature = "p3-lan")]
        this.input_enabled.store(input, Ordering::Release);
        #[cfg(not(feature = "p3-lan"))]
        let _ = input;
        this.start_host().await?;
        Ok((this, display))
    }
    async fn start_host(self: &Arc<Self>) -> Result<(), String> {
        let listener = TcpListener::bind((self.scope.local, self.port))
            .await
            .map_err(|_| "LAN_LISTEN_FAILED")?;
        self.started.store(true, Ordering::Release);
        let owner = self.clone();
        let task = tokio::spawn(async move {
            let result = owner.host_main(listener).await;
            owner.finish(result).await;
        });
        *self.task.lock().unwrap_or_else(|p| p.into_inner()) = Some(task);
        Ok(())
    }
    #[cfg(feature = "v2-verified-lan")]
    pub async fn host_verified(
        scope: LanScope,
        seconds: u32,
        port: u16,
        auth: VerifiedLanConfig,
    ) -> Result<(Arc<Self>, String), String> {
        auth.validate(scope)?;
        let key = Key::generate();
        let display = key.display();
        let this = Self::new(scope, seconds, port, "sender", key)?;
        *this.verified.lock().unwrap_or_else(|p| p.into_inner()) = Some(auth);
        this.start_host().await?;
        Ok((this, display))
    }

    #[cfg(feature = "v2-private-wss-lan")]
    pub async fn host_private_wss(
        scope: LanScope,
        seconds: u32,
        port: u16,
        auth: PrivateWssLanConfig,
    ) -> Result<(Arc<Self>, String), String> {
        auth.validate(scope)?;
        let key = Key::generate();
        let display = key.display();
        let this = Self::new(scope, seconds, port, "sender", key)?;
        *this.private_wss.lock().unwrap_or_else(|p| p.into_inner()) = Some(auth);
        this.start_host().await?;
        Ok((this, display))
    }
    pub fn receiver(
        scope: LanScope,
        seconds: u32,
        port: u16,
        code: &str,
    ) -> Result<Arc<Self>, String> {
        Self::new(scope, seconds, port, "receiver", Key::parse(code)?)
    }
    pub fn receiver_control(
        scope: LanScope,
        seconds: u32,
        port: u16,
        code: &str,
    ) -> Result<Arc<Self>, String> {
        Self::new_mode(scope, seconds, port, "receiver", Key::parse(code)?, true)
    }
    #[cfg(feature = "v2-verified-lan")]
    pub fn receiver_verified(
        scope: LanScope,
        seconds: u32,
        port: u16,
        code: &str,
        auth: VerifiedLanConfig,
    ) -> Result<Arc<Self>, String> {
        auth.validate(scope)?;
        let this = Self::receiver(scope, seconds, port, code)?;
        *this.verified.lock().unwrap_or_else(|p| p.into_inner()) = Some(auth);
        Ok(this)
    }
    #[cfg(feature = "v2-private-wss-lan")]
    pub fn receiver_private_wss(
        scope: LanScope,
        seconds: u32,
        port: u16,
        code: &str,
        auth: PrivateWssLanConfig,
    ) -> Result<Arc<Self>, String> {
        auth.validate(scope)?;
        let this = Self::receiver(scope, seconds, port, code)?;
        *this.private_wss.lock().unwrap_or_else(|p| p.into_inner()) = Some(auth);
        Ok(this)
    }
    #[cfg(feature = "v2-private-wss-lan")]
    fn take_private_wss(&self) -> Option<PrivateWssLanConfig> {
        self.private_wss
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }
    #[cfg(feature = "v2-verified-lan")]
    fn take_verified(&self) -> Option<VerifiedLanConfig> {
        self.verified
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }
    fn input_mode(&self) -> bool {
        #[cfg(feature = "p3-lan")]
        return self.input_enabled.load(Ordering::Acquire);
        #[cfg(not(feature = "p3-lan"))]
        false
    }
    pub fn control_session(&self) -> Option<String> {
        self.control_session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub fn record_browser_control(&self, data: crate::control::BrowserStats) -> Result<(), String> {
        if !self.control_enabled || self.role != "receiver" {
            return Err("CONTROL_STATS_NOT_ALLOWED".into());
        }
        data.validate()?;
        self.update(|s| s.browser_control = Some(data));
        Ok(())
    }
    fn validate_sdp(&self, sdp: &str, scope: NetworkScope, answer: bool) -> Result<(), String> {
        if self.control_enabled {
            crate::signaling::validate_control_description(sdp, scope, answer)?;
        } else {
            validate_description(sdp, scope, answer)?;
        }
        Ok(())
    }
    fn update(&self, f: impl FnOnce(&mut LanStatus)) {
        let mut s = self.status.lock().unwrap_or_else(|p| p.into_inner());
        s.elapsed_ms = self.created.elapsed().as_millis() as u64;
        f(&mut s);
    }
    fn secret(&self) -> Result<Arc<Key>, String> {
        self.key
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or("AUTH_EXPIRED".into())
    }
    pub fn stats(&self) -> LanStatus {
        let mut s = self
            .status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(source) = self
            .source
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            s.source = Some(source.stats());
        }
        if !s.resources_released {
            s.elapsed_ms = self.created.elapsed().as_millis() as u64;
        }
        s
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(source) = self
            .source
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            source.cancel();
        }
    }
    async fn cancelled(&self) {
        while !self.cancelled.load(Ordering::Acquire) {
            sleep(Duration::from_millis(30)).await;
        }
    }
    pub async fn stop(&self) -> Result<(), String> {
        let _guard = self.stop_lock.lock().await;
        self.cancel();
        let task = self.task.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(mut task) = task {
            match timeout(Duration::from_secs(12), &mut task).await {
                Err(_) => {
                    *self.task.lock().unwrap_or_else(|p| p.into_inner()) = Some(task);
                    return Err("LAN_CLEANUP_PENDING".into());
                }
                Ok(Err(_)) => return Err("LAN_TASK_FAILED".into()),
                Ok(Ok(())) => (),
            }
        } else if !self.started.load(Ordering::Acquire) {
            self.update(|s| {
                s.state = "stopped".into();
                s.resources_released = true;
            });
            self.key.lock().unwrap_or_else(|p| p.into_inner()).take();
            #[cfg(feature = "v2-verified-lan")]
            drop(self.take_verified());
            #[cfg(feature = "v2-private-wss-lan")]
            drop(self.take_private_wss());
        }
        if !self.stats().resources_released {
            return Err("LAN_CLEANUP_PENDING".into());
        }
        Ok(())
    }
    async fn finish(&self, result: Result<(), String>) {
        self.cancel();
        let source = self
            .source
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let clean = if let Some(source) = source {
            source.stop().await.is_ok() && source.stats().resources_released
        } else {
            true
        };
        self.key.lock().unwrap_or_else(|p| p.into_inner()).take();
        #[cfg(feature = "v2-verified-lan")]
        drop(self.take_verified());
        #[cfg(feature = "v2-private-wss-lan")]
        drop(self.take_private_wss());
        self.update(|s| {
            if let Err(error) = result {
                if error != "CANCELLED" {
                    s.error = Some(error);
                }
            }
            if !clean {
                s.error = Some("LAN_CLEANUP_PENDING".into());
            }
            s.resources_released = clean;
            s.state = if s.error.is_some() {
                "failed"
            } else {
                "stopped"
            }
            .into();
        });
    }
    async fn host_main(&self, listener: TcpListener) -> Result<(), String> {
        #[cfg(feature = "v2-private-wss-lan")]
        let private_wss = self.take_private_wss();
        #[cfg(feature = "v2-verified-lan")]
        let verified = self.take_verified();
        #[cfg(feature = "v2-verified-lan")]
        let mut signed_sdp = None;
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut accepted = None;
        let mut attempts = 0;
        while Instant::now() < deadline && !self.cancelled.load(Ordering::Acquire) && attempts < 5 {
            let result = tokio::select! {v=timeout(deadline.saturating_duration_since(Instant::now()),listener.accept())=>v.map_err(|_|"LAN_WAIT_EXPIRED")?.map_err(|_|"LAN_ACCEPT_FAILED")?,_=self.cancelled()=>return Ok(())};
            let (mut socket, address) = result;
            attempts += 1;
            if address.ip() != std::net::IpAddr::V4(self.scope.peer) {
                continue;
            }
            #[cfg(feature = "v2-private-wss-lan")]
            if let Some(auth) = private_wss {
                drop(listener);
                return self.host_wss_main(socket, auth).await;
            }
            #[cfg(feature = "v2-verified-lan")]
            if let Some(auth) = verified.as_ref() {
                check_verified_socket(&socket, self.scope, self.port, true)?;
                let (codec, proof, offer) = tokio::select! {
                    biased;
                    _ = self.cancelled() => return Ok(()),
                    result = timeout(Duration::from_secs(5), async {
                        let (mut codec, mut proof) = crate::lan_device::authenticate_host(
                            &mut socket, self.secret()?, &auth.local_identity, &auth.peer,
                            self.scope, self.port,
                        ).await?;
                        let (kind, payload) = codec.accept(lan_wire::read(&mut socket).await?)?;
                        if kind != "offer" {
                            return Err("EXPECTED_SIGNED_LAN_OFFER".to_owned());
                        }
                        let offer = proof.open(&payload)?;
                        Ok::<_, String>((codec, proof, offer))
                    }) => result
                        .map_err(|_| "LAN_DEVICE_AUTH_TIMEOUT")?
                        .map_err(|_| "LAN_DEVICE_AUTH_FAILED")?,
                };
                validate_verified_sdp(&offer, self.scope, false)?;
                signed_sdp = Some(proof);
                accepted = Some((socket, codec, offer));
                break;
            }
            let frame = tokio::select! {r=timeout(Duration::from_secs(5),lan_wire::read(&mut socket))=>match r{Ok(Ok(f))=>f,_=>continue},_=self.cancelled()=>return Ok(())};
            let mut codec = match Codec::new(self.secret()?, frame.nonce.clone(), true) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let (kind, offer) = match codec.accept(frame) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if kind
                != if self.control_enabled {
                    "offer.control.v1"
                } else {
                    "offer"
                }
            {
                continue;
            }
            let offer = if self.control_enabled {
                let envelope: ControlSignal =
                    serde_json::from_str(&offer).map_err(|_| "CONTROL_SIGNAL_SCHEMA")?;
                if envelope.input_enabled != self.input_mode() {
                    return Err("INPUT_MODE_MISMATCH".into());
                }
                uuid::Uuid::parse_str(&envelope.control_session)
                    .map_err(|_| "CONTROL_SESSION_INVALID")?;
                *self
                    .control_session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(envelope.control_session);
                envelope.sdp
            } else {
                offer
            };
            self.validate_sdp(&offer, NetworkScope::Lan(self.scope), false)?;
            accepted = Some((socket, codec, offer));
            break;
        }
        drop(listener);
        if self.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let (mut socket, mut codec, offer) = accepted.ok_or("LAN_WAIT_EXPIRED_OR_AUTH_REJECTED")?;
        self.update(|s| {
            s.state = "negotiating".into();
            s.authenticated = true;
        });
        #[cfg(feature = "p3-lan")]
        let input_source = if self.input_enabled.load(Ordering::Acquire) {
            Some(Session::new_lan_input(
                self.seconds,
                self.scope,
                self.control_session().ok_or("CONTROL_CONTEXT_MISSING")?,
            )?)
        } else {
            None
        };
        #[cfg(not(feature = "p3-lan"))]
        let input_source: Option<Arc<Session>> = None;
        let source = if let Some(source) = input_source {
            source
        } else if self.control_enabled {
            Session::new_lan_control(
                self.seconds,
                self.scope,
                self.control_session().ok_or("CONTROL_CONTEXT_MISSING")?,
            )?
        } else {
            Session::new_lan(self.seconds, self.scope)?
        };
        *self.source.lock().unwrap_or_else(|p| p.into_inner()) = Some(source.clone());
        #[cfg(feature = "v2-verified-lan")]
        if let Some(mut proof) = signed_sdp {
            let answer = tokio::select! {
                biased;
                _ = self.cancelled() => return Ok(()),
                result = source.signal(offer) => result?,
            };
            validate_verified_sdp(&answer, reversed_scope(self.scope), true)?;
            let payload = proof.seal(answer).map_err(|_| "LAN_DEVICE_AUTH_FAILED")?;
            let frame = codec
                .encode("answer", payload)
                .map_err(|_| "LAN_DEVICE_AUTH_FAILED")?;
            drop(verified);
            tokio::select! {
                biased;
                _ = self.cancelled() => return Ok(()),
                result = timeout(Duration::from_secs(2), lan_wire::write(&mut socket, frame)) => {
                    result.map_err(|_| "LAN_ANSWER_TIMEOUT")??;
                }
            }
            return self.control(socket, codec).await;
        }
        let answer = source.signal(offer).await?;
        let (kind, payload) = if self.control_enabled {
            (
                "answer.control.v1",
                serde_json::to_string(&ControlSignal {
                    sdp: answer,
                    control_session: self.control_session().ok_or("CONTROL_CONTEXT_MISSING")?,
                    input_enabled: self.input_mode(),
                })
                .map_err(|_| "CONTROL_SIGNAL_SCHEMA")?,
            )
        } else {
            ("answer", answer)
        };
        lan_wire::write(&mut socket, codec.encode(kind, payload)?).await?;
        self.control(socket, codec).await
    }
    pub async fn signal_receiver(self: &Arc<Self>, offer: String) -> Result<String, String> {
        if self.role != "receiver" || self.cancelled.load(Ordering::Acquire) {
            return Err("INVALID_RECEIVER_SESSION".into());
        }
        // The receiver's own offer must only advertise its explicitly selected interface.
        self.validate_sdp(
            &offer,
            NetworkScope::Lan(LanScope {
                local: self.scope.peer,
                peer: self.scope.local,
            }),
            false,
        )?;
        let (tx, rx) = oneshot::channel();
        {
            let mut slot = self.task.lock().unwrap_or_else(|p| p.into_inner());
            if self.cancelled.load(Ordering::Acquire) {
                return Err("CANCELLED".into());
            }
            if self.started.swap(true, Ordering::AcqRel) {
                return Err("ALREADY_SIGNALED".into());
            }
            let owner = self.clone();
            *slot = Some(tokio::spawn(async move {
                let mut response = Some(tx);
                let result = owner.receiver_main(offer, &mut response).await;
                if let (Err(error), Some(response)) = (&result, response.take()) {
                    let _ = response.send(Err(error.clone()));
                }
                owner.finish(result).await;
            }));
        }
        timeout(Duration::from_secs(15), rx)
            .await
            .map_err(|_| "LAN_SIGNAL_TIMEOUT")?
            .map_err(|_| "LAN_SIGNAL_FAILED")?
    }
    async fn receiver_main(
        &self,
        offer: String,
        response: &mut Option<oneshot::Sender<Result<String, String>>>,
    ) -> Result<(), String> {
        #[cfg(feature = "v2-private-wss-lan")]
        let private_wss = self.take_private_wss();
        #[cfg(feature = "v2-verified-lan")]
        let (verified, deadline) = (
            self.take_verified(),
            Instant::now() + Duration::from_secs(14),
        );
        #[cfg(feature = "v2-verified-lan")]
        if verified.is_some() {
            validate_verified_sdp(&offer, reversed_scope(self.scope), false)?;
        }
        #[cfg(feature = "v2-private-wss-lan")]
        if private_wss.is_some() {
            validate_verified_sdp(&offer, reversed_scope(self.scope), false)?;
        }
        let socket = TcpSocket::new_v4().map_err(|_| "LAN_SOCKET_FAILED")?;
        socket
            .bind((self.scope.local, 0).into())
            .map_err(|_| "LAN_LOCAL_BIND_FAILED")?;
        let mut socket = tokio::select! {r=timeout(Duration::from_secs(5),socket.connect((self.scope.peer,self.port).into()))=>r.map_err(|_|"LAN_CONNECT_TIMEOUT")?.map_err(|_|"LAN_CONNECT_FAILED")?,_=self.cancelled()=>return Err("CANCELLED".into())};
        #[cfg(feature = "v2-private-wss-lan")]
        if let Some(auth) = private_wss {
            let (codec, answer) = self
                .receive_wss_answer(&mut socket, auth, offer, deadline)
                .await?;
            return self
                .accept_receiver_answer(socket, codec, answer, response)
                .await;
        }
        #[cfg(feature = "v2-verified-lan")]
        if let Some(auth) = verified {
            check_verified_socket(&socket, self.scope, self.port, false)?;
            let (mut codec, mut proof) = tokio::select! {
                biased;
                _ = self.cancelled() => return Err("CANCELLED".into()),
                result = timeout(Duration::from_secs(5), crate::lan_device::authenticate_viewer(
                    &mut socket, self.secret()?, &auth.local_identity, &auth.peer,
                    self.scope, self.port,
                )) => result
                    .map_err(|_| "LAN_DEVICE_AUTH_TIMEOUT")?
                    .map_err(|_| "LAN_DEVICE_AUTH_FAILED")?,
            };
            let answer = tokio::select! {
                biased;
                _ = self.cancelled() => return Err("CANCELLED".into()),
                result = timeout(deadline.saturating_duration_since(Instant::now()), async {
                    let payload = proof.seal(offer)?;
                    lan_wire::write(&mut socket, codec.encode("offer", payload)?).await?;
                    let (kind, payload) = codec.accept(lan_wire::read(&mut socket).await?)?;
                    if kind != "answer" {
                        return Err("EXPECTED_SIGNED_LAN_ANSWER".to_owned());
                    }
                    proof.open(&payload)
                }) => result.map_err(|_| "LAN_ANSWER_TIMEOUT")?
                    .map_err(|_| "LAN_DEVICE_AUTH_FAILED")?,
            };
            drop(auth);
            validate_verified_sdp(&answer, self.scope, true)?;
            if self.cancelled.load(Ordering::Acquire) {
                return Err("CANCELLED".into());
            }
            return self
                .accept_receiver_answer(socket, codec, answer, response)
                .await;
        }
        let mut codec = Codec::new(self.secret()?, uuid::Uuid::new_v4().to_string(), false)?;
        let (kind, payload) = if self.control_enabled {
            (
                "offer.control.v1",
                serde_json::to_string(&ControlSignal {
                    sdp: offer,
                    control_session: self.control_session().ok_or("CONTROL_CONTEXT_MISSING")?,
                    input_enabled: self.input_mode(),
                })
                .map_err(|_| "CONTROL_SIGNAL_SCHEMA")?,
            )
        } else {
            ("offer", offer)
        };
        lan_wire::write(&mut socket, codec.encode(kind, payload)?).await?;
        let frame = tokio::select! {result=timeout(Duration::from_secs(10),lan_wire::read(&mut socket))=>result.map_err(|_|"LAN_ANSWER_TIMEOUT")??,_=self.cancelled()=>return Err("CANCELLED".into())};
        let (kind, answer) = codec.accept(frame)?;
        if kind
            != if self.control_enabled {
                "answer.control.v1"
            } else {
                "answer"
            }
        {
            return Err("EXPECTED_AUTHENTICATED_ANSWER".into());
        }
        let answer = if self.control_enabled {
            let envelope: ControlSignal =
                serde_json::from_str(&answer).map_err(|_| "CONTROL_SIGNAL_SCHEMA")?;
            if envelope.input_enabled != self.input_mode() {
                return Err("INPUT_MODE_MISMATCH".into());
            }
            if Some(envelope.control_session) != self.control_session() {
                return Err("CONTROL_SESSION_MISMATCH".into());
            }
            envelope.sdp
        } else {
            answer
        };
        self.validate_sdp(&answer, NetworkScope::Lan(self.scope), true)?;
        self.accept_receiver_answer(socket, codec, answer, response)
            .await
    }
    async fn accept_receiver_answer(
        &self,
        socket: TcpStream,
        codec: Codec,
        answer: String,
        response: &mut Option<oneshot::Sender<Result<String, String>>>,
    ) -> Result<(), String> {
        self.update(|s| {
            s.state = "connected".into();
            s.authenticated = true;
        });
        response
            .take()
            .ok_or("RECEIVER_VIEW_CLOSED")?
            .send(Ok(answer))
            .map_err(|_| "RECEIVER_VIEW_CLOSED")?;
        self.control(socket, codec).await
    }
    pub fn metrics(&self, data: Value) -> Result<(), String> {
        if self.role != "receiver" || self.cancelled.load(Ordering::Acquire) {
            return Err("RECEIVER_STOPPED".into());
        }
        validate_metrics(&data)?;
        if let Some(control) = data.get("control") {
            self.record_browser_control(
                serde_json::from_value(control.clone()).map_err(|_| "CONTROL_METRICS_INVALID")?,
            )?;
        }
        self.update(|s| {
            s.receiver_samples.push(data.clone());
            if s.receiver_samples.len() > 60 {
                s.receiver_samples.remove(0);
            }
        });
        let text = serde_json::to_string(&data).map_err(|_| "METRICS_INVALID")?;
        *self
            .latest_metrics
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(text);
        Ok(())
    }
    async fn control(&self, socket: TcpStream, mut codec: Codec) -> Result<(), String> {
        let (mut read, mut write) = socket.into_split();
        let (tx, mut rx) = mpsc::channel(2);
        let reader = tokio::spawn(async move {
            loop {
                let frame = timeout(Duration::from_secs(3), lan_wire::read(&mut read))
                    .await
                    .unwrap_or_else(|_| Err("LAN_LEASE_EXPIRED".into()));
                let end = frame.is_err();
                if tx.send(frame).await.is_err() || end {
                    break;
                }
            }
        });
        let result=async {
            let mut beat = tokio::time::interval(Duration::from_secs(1));
            let end = Instant::now() + Duration::from_secs(self.seconds as u64 + 15);
            let mut announced_finish = false;
            let mut released_at = None;
            loop {
                if self.cancelled.load(Ordering::Acquire) {
                    let _ = timeout(
                        Duration::from_millis(300),
                        lan_wire::write(&mut write, codec.encode("stop", String::new())?),
                    ).await;
                    return Ok(());
                }
                if Instant::now() > end {
                    return Err("LAN_SESSION_DEADLINE".into());
                }
                let source = self.source.lock().unwrap_or_else(|p| p.into_inner()).clone();
                if let Some(source) = source {
                    let stats = source.stats();
                    if !announced_finish && should_announce_finish(&stats, self.input_mode()) {
                        timeout(
                            Duration::from_secs(2),
                            lan_wire::write(&mut write, codec.encode("finish", String::new())?),
                        ).await.map_err(|_| "CONTROL_WRITE_TIMEOUT")??;
                        announced_finish = true;
                    }
                    if stats.resources_released {
                        let released = *released_at.get_or_insert_with(Instant::now);
                        let decoded = self.stats().receiver_samples.last()
                            .and_then(|data| data["framesDecoded"].as_u64()).unwrap_or(0);
                        if decoded >= u64::from(stats.frames_sent)
                            || released.elapsed() > Duration::from_secs(2)
                            || stats.error.is_some()
                        {
                            let signal = timeout(
                                Duration::from_secs(2),
                                lan_wire::write(&mut write, codec.encode("stop", String::new())?),
                            ).await.map_err(|_| "CONTROL_WRITE_TIMEOUT".to_owned()).and_then(|value| value);
                            // An early notice or a follow-up write failure may
                            // not replace the real source failure.
                            if let Some(error) = stats.error {
                                return Err(error);
                            }
                            signal?;
                            return Ok(());
                        }
                    }
                }
    tokio::select!{
     frame=rx.recv()=>{
      let frame=frame.ok_or("SIGNAL_DISCONNECTED")??;let(kind,payload)=codec.accept(frame)?;
      self.update(|s|s.messages_received+=1);
      match kind.as_str(){"beat" if payload.is_empty()=>(),"stop" if payload.is_empty()=>return Ok(()),"finish" if self.role=="receiver"&&payload.is_empty()=>self.update(|s|s.state="finishing".into()),"metrics" if self.role=="sender"=>{
       let data:Value=serde_json::from_str(&payload).map_err(|_|"METRICS_INVALID")?;validate_metrics(&data)?;
       self.update(|s|{s.receiver_samples.push(data);if s.receiver_samples.len()>60{s.receiver_samples.remove(0);}});
      },_=>return Err("UNEXPECTED_CONTROL_MESSAGE".into())}
     },
     _=beat.tick()=>{
      let metric=self.latest_metrics.lock().unwrap_or_else(|p|p.into_inner()).take();
      let(kind,payload)=if let Some(data)=metric{("metrics",data)}else{("beat",String::new())};
      timeout(Duration::from_secs(2),lan_wire::write(&mut write,codec.encode(kind,payload)?)).await.map_err(|_|"CONTROL_WRITE_TIMEOUT")??;
     },
     _=sleep(Duration::from_millis(30))=>(),
    }
   }
  }.await;
        reader.abort();
        let _ = reader.await;
        use tokio::io::AsyncWriteExt;
        let _ = write.shutdown().await;
        result
    }
}

fn input_disabled(value: &bool) -> bool {
    !*value
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlSignal {
    sdp: String,
    control_session: String,
    #[serde(default, skip_serializing_if = "input_disabled")]
    input_enabled: bool,
}

pub fn validate_metrics(data: &Value) -> Result<(), String> {
    let object = data.as_object().ok_or("METRICS_INVALID")?;
    let allowed = [
        "elapsedMs",
        "framesDecoded",
        "framesReceived",
        "packetsReceived",
        "packetsLost",
        "bytesReceived",
        "videoWidth",
        "videoHeight",
        "framesRendered",
        "currentTime",
        "firstFrameMs",
        "connectionState",
        "iceState",
        "codec",
        "fmtp",
        "selectedRemoteLoopback",
        "localCandidateType",
        "remoteCandidateType",
        "freezeCount",
        "totalFreezesDuration",
        "peerMatches",
        "decoderImplementation",
        "control",
    ];
    if object.keys().any(|k| !allowed.contains(&k.as_str()))
        || serde_json::to_vec(data)
            .map_err(|_| "METRICS_INVALID")?
            .len()
            > 4096
    {
        return Err("METRICS_INVALID".into());
    }
    if data["peerMatches"].as_bool() == Some(false) {
        return Err("BROWSER_SELECTED_OFF_PEER".into());
    }
    if let Some(control) = data.get("control") {
        let stats: crate::control::BrowserStats =
            serde_json::from_value(control.clone()).map_err(|_| "CONTROL_METRICS_INVALID")?;
        stats.validate()?;
    }
    if data["framesDecoded"].as_u64().unwrap_or(0) > 0
        && data["peerMatches"].as_bool() != Some(true)
    {
        return Err("BROWSER_PEER_UNVERIFIED".into());
    }
    for value in object.values() {
        if value.as_str().is_some_and(|s| s.len() > 256) {
            return Err("METRICS_INVALID".into());
        }
    }
    for key in [
        "framesDecoded",
        "framesReceived",
        "packetsReceived",
        "bytesReceived",
        "videoWidth",
        "videoHeight",
        "framesRendered",
        "freezeCount",
    ] {
        if !data[key].is_null() && !data[key].is_u64() {
            return Err("METRICS_INVALID_NUMBER".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "v2-verified-lan")]
    #[test]
    fn verified_video_requires_local_inline_rtp_and_unambiguous_dtls_identity() {
        let scope = LanScope::new(
            "192.168.1.2".parse().unwrap(),
            "192.168.1.3".parse().unwrap(),
        )
        .unwrap();
        let fingerprint = ["AA"; 32].join(":");
        let video = format!(
            "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\na=recvonly\r\na=rtcp-mux\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\na=fingerprint:sha-256 {fingerprint}\r\n"
        );
        assert!(validate_description(&video, NetworkScope::Lan(scope), false).is_ok());
        assert_eq!(
            validate_verified_sdp(&video, scope, false).unwrap_err(),
            "LAN_VERIFIED_INLINE_CANDIDATE_REQUIRED"
        );
        let offer = format!("{video}a=candidate:1 1 UDP 1 192.168.1.3 49152 typ host\r\n");
        assert!(validate_verified_sdp(&offer, scope, false).is_ok());
        assert!(validate_verified_sdp(&offer, reversed_scope(scope), false).is_err());
        assert_eq!(
            validate_verified_sdp(
                &format!("{offer}a=fingerprint:sha-256 {}\r\n", ["BB"; 32].join(":")),
                scope,
                false,
            )
            .unwrap_err(),
            "LAN_VERIFIED_DTLS_FINGERPRINT_AMBIGUOUS"
        );
        assert!(validate_verified_sdp(
            &format!("{offer}m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n"),
            scope,
            false
        )
        .is_err());
    }

    #[cfg(feature = "v2-verified-lan")]
    #[test]
    fn verified_receiver_rejects_a_pin_owned_by_another_local_device() {
        use hub_identity::{pairing::PairingCandidate, Identity};
        let local_identity = Arc::new(Identity::generate_ephemeral().unwrap());
        let other_local = Identity::generate_ephemeral().unwrap();
        let peer_identity = Identity::generate_ephemeral().unwrap();
        let peer = Arc::new(
            other_local
                .verify_peer(
                    &PairingCandidate::parse(&peer_identity.pairing_code()).unwrap(),
                    &peer_identity.public().fingerprint,
                )
                .unwrap(),
        );
        let scope = LanScope::new(
            "192.168.1.2".parse().unwrap(),
            "192.168.1.3".parse().unwrap(),
        )
        .unwrap();
        assert!(matches!(
            LanSession::receiver_verified(
                scope,
                5,
                48604,
                &Key::generate().display(),
                VerifiedLanConfig { local_identity, peer },
            ),
            Err(error) if error == "LAN_VERIFIED_PEER_BINDING"
        ));
    }

    #[test]
    fn finish_is_only_sent_during_successful_input_drain_or_release() {
        let mut stats = Stats {
            state: "draining".into(),
            frames_sent: 10,
            ..Default::default()
        };
        assert!(should_announce_finish(&stats, true));
        assert!(!should_announce_finish(&stats, false));
        stats.frames_sent = 0;
        assert!(!should_announce_finish(&stats, true));
        stats.frames_sent = 10;
        stats.control = Some(crate::control::ControlStats {
            last_error: Some("CONTROL_CHANNEL_ERROR".into()),
            ..Default::default()
        });
        assert!(!should_announce_finish(&stats, true));
        stats.control = None;
        stats.state = "stopped".into();
        assert!(!should_announce_finish(&stats, true));
        stats.resources_released = true;
        assert!(should_announce_finish(&stats, true));
        assert!(should_announce_finish(&stats, false));
        stats.error = Some("PEER_CONNECTION_FAILED".into());
        assert!(!should_announce_finish(&stats, true));
    }
    #[tokio::test]
    async fn receiver_returns_the_actual_authenticated_answer_validation_error() {
        let listener = TcpListener::bind("127.0.0.2:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = Arc::new(Key::generate());
        let code = key.display();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let frame = lan_wire::read(&mut socket).await.unwrap();
            let mut codec = Codec::new(key, frame.nonce.clone(), true).unwrap();
            codec.accept(frame).unwrap();
            lan_wire::write(
                &mut socket,
                codec.encode("answer", "invalid-sdp".into()).unwrap(),
            )
            .await
            .unwrap();
        });
        let receiver = LanSession::new(
            LanScope {
                local: "127.0.0.1".parse().unwrap(),
                peer: "127.0.0.2".parse().unwrap(),
            },
            5,
            port,
            "receiver",
            Key::parse(&code).unwrap(),
        )
        .unwrap();
        let offer=format!("v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\na=recvonly\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\na=fingerprint:sha-256 {}\r\n",["AA";32].join(":"));
        assert_eq!(
            receiver.signal_receiver(offer).await.unwrap_err(),
            "SINGLE_RECVONLY_VIDEO_REQUIRED"
        );
        receiver.stop().await.unwrap();
        server.await.unwrap();
        assert!(receiver.stats().resources_released);
    }
    #[tokio::test]
    async fn off_peer_tcp_creates_no_source_and_cancel_releases_the_listener() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        // Loopback transport fixture only; the public UI validates RFC1918 addresses.
        let scope = LanScope {
            local: "127.0.0.1".parse().unwrap(),
            peer: "127.0.0.2".parse().unwrap(),
        };
        let (session, _) = LanSession::host(scope, 5, port).await.unwrap();
        let socket = TcpStream::connect((scope.local, port)).await.unwrap();
        sleep(Duration::from_millis(40)).await;
        assert!(session.stats().source.is_none());
        assert!(!session.stats().authenticated);
        session.stop().await.unwrap();
        drop(socket);
        assert!(session.stats().resources_released);
        assert!(session.secret().is_err());
        assert!(TcpListener::bind((scope.local, port)).await.is_ok());
    }
    #[tokio::test]
    async fn silent_control_peer_expires_lease_and_releases_without_capture() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        drop(listener);
        let key = Key::generate();
        let code = key.display();
        let session = LanSession::new(
            LanScope {
                local: "127.0.0.1".parse().unwrap(),
                peer: "127.0.0.2".parse().unwrap(),
            },
            5,
            48604,
            "receiver",
            Key::parse(&code).unwrap(),
        )
        .unwrap();
        let codec = Codec::new(
            session.secret().unwrap(),
            uuid::Uuid::new_v4().to_string(),
            false,
        )
        .unwrap();
        let result = timeout(Duration::from_secs(5), session.control(client, codec))
            .await
            .unwrap();
        assert_eq!(result.as_ref().unwrap_err(), "LAN_LEASE_EXPIRED");
        session.finish(result).await;
        drop(server);
        assert!(session.stats().resources_released);
        assert!(session.stats().source.is_none());
    }
    #[tokio::test]
    async fn stopped_receiver_cannot_be_signaled_or_keep_its_secret() {
        let scope = LanScope::new(
            "192.168.10.2".parse().unwrap(),
            "192.168.10.3".parse().unwrap(),
        )
        .unwrap();
        let receiver = LanSession::receiver(scope, 10, 48604, &Key::generate().display()).unwrap();
        receiver.stop().await.unwrap();
        receiver.stop().await.unwrap();
        assert!(receiver.secret().is_err());
        assert!(receiver.signal_receiver(String::new()).await.is_err());
        assert!(receiver.stats().resources_released);
    }
    #[test]
    fn metrics_never_accept_an_unverified_playing_peer_or_extra_payload() {
        assert!(
            validate_metrics(&serde_json::json!({"framesDecoded":1,"peerMatches":null})).is_err()
        );
        assert!(
            validate_metrics(&serde_json::json!({"framesDecoded":1,"peerMatches":true})).is_ok()
        );
        assert!(validate_metrics(&serde_json::json!({"sdp":"secret"})).is_err());
        assert!(validate_metrics(&serde_json::json!({"framesDecoded":-1})).is_err());
    }
}

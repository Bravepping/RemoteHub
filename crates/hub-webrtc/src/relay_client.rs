//! Native WSS signaling only. Video/control payloads remain on the same WebRTC PC.
pub use crate::relay_config::PrivateConfig;
use crate::{
    ice_policy::{IceConfig, RelayGrant, RelayTarget},
    lan::LanStatus,
    session::{RelaySettings, Session},
};
use futures_util::{SinkExt, StreamExt};
use hub_probe_wire::{now, Codec, Hello, PairKey, PathMode, Ready, Role, ServerEvent, MAX_FRAME};
use serde::Serialize;
use std::{
    io::BufReader,
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, oneshot, Notify},
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Message},
    Connector,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserReady {
    pub mode: PathMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_scope: Option<BrowserDirectScope>,
    pub control_session: String,
    pub server_ip: String,
    pub expires_at: u64,
    pub ice: IceConfig,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDirectScope {
    pub server_ip: String,
    pub local_ip: String,
    pub private_peer_ips: Vec<String>,
    pub publish_private_local: bool,
    pub expires_at: u64,
}
struct Context {
    grant: Option<Arc<RelayGrant>>,
    ready: Ready,
    #[cfg(feature = "p006-direct")]
    direct: Option<Arc<crate::direct_policy::DirectGrant>>,
    #[cfg(feature = "p006-direct")]
    exchange: Option<hub_probe_wire::PathExchange>,
}
pub struct RelayClient {
    receiver_journal: Mutex<crate::receiver_evidence::Journal>,
    mode: PathMode,
    #[cfg(feature = "p006-direct")]
    private_peers: Vec<Ipv4Addr>,
    signaled: AtomicBool,
    role: Role,
    seconds: u32,
    local: Ipv4Addr,
    config: Arc<PrivateConfig>,
    created: Instant,
    cancelled: AtomicBool,
    task: Mutex<Option<JoinHandle<()>>>,
    stop_lock: tokio::sync::Mutex<()>,
    status: Mutex<LanStatus>,
    source: Mutex<Option<Arc<Session>>>,
    context: Mutex<Option<Context>>,
    notify: Notify,
    outbound: mpsc::Sender<(String, String)>,
    receiver: Mutex<Option<mpsc::Receiver<(String, String)>>>,
    answer: Mutex<Option<oneshot::Sender<Result<String, String>>>>,
}
impl RelayClient {
    pub fn start(
        config: Arc<PrivateConfig>,
        role: Role,
        local: Ipv4Addr,
        seconds: u32,
        code: Option<&str>,
    ) -> Result<(Arc<Self>, Option<String>), String> {
        Self::start_mode(
            config,
            role,
            local,
            seconds,
            code,
            PathMode::RelayOnly,
            vec![],
        )
    }
    #[cfg(feature = "p006-direct")]
    pub fn start_direct(
        config: Arc<PrivateConfig>,
        role: Role,
        local: Ipv4Addr,
        seconds: u32,
        code: Option<&str>,
        private_peer: Option<Ipv4Addr>,
    ) -> Result<(Arc<Self>, Option<String>), String> {
        Self::start_mode(
            config,
            role,
            local,
            seconds,
            code,
            PathMode::DirectPreferred,
            private_peer.into_iter().collect(),
        )
    }
    fn start_mode(
        config: Arc<PrivateConfig>,
        role: Role,
        local: Ipv4Addr,
        seconds: u32,
        code: Option<&str>,
        mode: PathMode,
        private_peers: Vec<Ipv4Addr>,
    ) -> Result<(Arc<Self>, Option<String>), String> {
        config.authorize_start(role, mode)?;
        if mode == PathMode::DirectPreferred && !cfg!(feature = "p006-direct") {
            return Err("DIRECT_MODE_NOT_ENABLED".into());
        }
        hub_probe_wire::PathPolicy::new(private_peers.clone())?;
        if !(5..=30).contains(&seconds) || !local.is_private() {
            return Err("P006_CONFIG_INVALID".into());
        }
        let key = Arc::new(if role == Role::Sender {
            PairKey::generate()?
        } else {
            PairKey::parse(code.unwrap_or_default())?
        });
        let display = (role == Role::Sender).then(|| key.display());
        let (tx, rx) = mpsc::channel(4);
        let this = Arc::new(Self {
            receiver_journal: Mutex::new(crate::receiver_evidence::Journal::default()),
            mode,
            #[cfg(feature = "p006-direct")]
            private_peers,
            signaled: AtomicBool::new(false),
            role,
            seconds,
            local,
            config,
            created: Instant::now(),
            cancelled: AtomicBool::new(false),
            task: Mutex::new(None),
            stop_lock: tokio::sync::Mutex::new(()),
            status: Mutex::new(LanStatus {
                browser_control: None,
                role: if role == Role::Sender {
                    "sender"
                } else {
                    "receiver"
                }
                .into(),
                state: "waiting".into(),
                elapsed_ms: 0,
                authenticated: false,
                messages_received: 0,
                source: None,
                receiver_samples: vec![],
                resources_released: false,
                error: None,
            }),
            source: Mutex::new(None),
            context: Mutex::new(None),
            notify: Notify::new(),
            outbound: tx,
            receiver: Mutex::new(Some(rx)),
            answer: Mutex::new(None),
        });
        let owner = this.clone();
        *this.task.lock().unwrap_or_else(|x| x.into_inner()) = Some(tokio::spawn(async move {
            let result = owner.worker(key).await;
            owner.finish(result).await;
        }));
        Ok((this, display))
    }
    fn update(&self, f: impl FnOnce(&mut LanStatus)) {
        let mut s = self.status.lock().unwrap_or_else(|x| x.into_inner());
        s.elapsed_ms = self.created.elapsed().as_millis() as u64;
        f(&mut s);
    }
    pub fn stats(&self) -> LanStatus {
        let mut s = self
            .status
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .clone();
        if !s.resources_released {
            s.elapsed_ms = self.created.elapsed().as_millis() as u64;
        }
        if let Some(source) = self
            .source
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .as_ref()
        {
            s.source = Some(source.stats());
        }
        s
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(s) = self
            .source
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .as_ref()
        {
            s.cancel();
        }
        self.notify.notify_one();
    }
    pub async fn stop(&self) -> Result<(), String> {
        let _lock = self.stop_lock.lock().await;
        self.cancel();
        let task = self.task.lock().unwrap_or_else(|x| x.into_inner()).take();
        if let Some(mut task) = task {
            match timeout(Duration::from_secs(14), &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err("P006_TASK_FAILED".into()),
                Err(_) => {
                    *self.task.lock().unwrap_or_else(|x| x.into_inner()) = Some(task);
                    return Err("P006_CLEANUP_PENDING".into());
                }
            }
        }
        if !self.stats().resources_released {
            return Err("P006_CLEANUP_PENDING".into());
        }
        Ok(())
    }
    pub async fn browser_ready(&self) -> Result<BrowserReady, String> {
        timeout(Duration::from_secs(125), async {
            loop {
                let notified = self.notify.notified();
                {
                    let context = self.context.lock().unwrap_or_else(|x| x.into_inner());
                    if let Some(c) = context
                        .as_ref()
                        .filter(|c| !self.cancelled.load(Ordering::Acquire) && self.path_ready(c))
                    {
                        return Ok(BrowserReady {
                            mode: self.mode,
                            direct_scope: self.browser_scope(c)?,
                            control_session: c.ready.session_id.clone(),
                            server_ip: self.media_server_ip().to_string(),
                            expires_at: c.ready.expires_at,
                            ice: ice_mode(&c.ready, self.mode)?,
                        });
                    }
                }
                let s = self.stats();
                if s.resources_released || self.cancelled.load(Ordering::Acquire) {
                    return Err(s.error.unwrap_or("P006_STOPPED".into()));
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| "P006_PAIRING_TIMEOUT")?
    }
    pub async fn signal(&self, offer: String) -> Result<String, String> {
        if self.role != Role::Receiver || self.cancelled.load(Ordering::Acquire) {
            return Err("P006_NOT_RECEIVER".into());
        }
        {
            let c = self.context.lock().unwrap_or_else(|x| x.into_inner());
            self.check_sdp(c.as_ref().ok_or("P006_NOT_READY")?, &offer, false, true)?;
        }
        if self.signaled.swap(true, Ordering::AcqRel) {
            self.cancel();
            return Err("P006_ALREADY_SIGNALED".into());
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut slot = self.answer.lock().unwrap_or_else(|x| x.into_inner());
            if slot.is_some() {
                return Err("P006_ALREADY_SIGNALED".into());
            }
            *slot = Some(tx);
        }
        self.outbound
            .send(("offer".into(), offer))
            .await
            .map_err(|_| "P006_STOPPED")?;
        timeout(Duration::from_secs(22), rx)
            .await
            .map_err(|_| "P006_ANSWER_TIMEOUT")?
            .map_err(|_| "P006_SIGNAL_CLOSED")?
    }
    pub fn metrics(&self, data: serde_json::Value) -> Result<(), String> {
        if self.role != Role::Receiver || self.cancelled.load(Ordering::Acquire) {
            return Err("P006_RECEIVER_STOPPED".into());
        }
        self.validate_metrics(&data)?;
        self.update(|s| {
            s.receiver_samples.push(data.clone());
            if s.receiver_samples.len() > 60 {
                s.receiver_samples.remove(0);
            }
        });
        let payload = serde_json::to_string(&data).map_err(|_| "METRICS_INVALID")?;
        self.outbound
            .try_send(("metrics".into(), payload))
            .map_err(|_| "P006_SIGNAL_BACKPRESSURE".into())
    }
    pub fn record_control(&self, c: crate::control::BrowserStats) -> Result<(), String> {
        c.validate()?;
        self.update(|s| s.browser_control = Some(c));
        Ok(())
    }
    pub fn save_receiver_evidence(
        &self,
        dir: &std::path::Path,
        id: &str,
        e: crate::receiver_evidence::ReceiverEvidence,
    ) -> Result<String, String> {
        if self.role != Role::Receiver {
            return Err("RECEIVER_EVIDENCE_WRONG_ROLE".into());
        }
        self.receiver_journal
            .lock()
            .map_err(|_| "RECEIVER_EVIDENCE_LOCK")?
            .save(dir, id, e, self.stats().resources_released)
    }
    pub fn receiver_evidence_complete(&self) -> bool {
        self.role != Role::Receiver || self.receiver_journal.lock().is_ok_and(|j| j.complete())
    }
    pub fn save_missing_receiver_evidence(
        &self,
        dir: &std::path::Path,
        id: &str,
    ) -> Result<(), String> {
        if self.role != Role::Receiver {
            return Ok(());
        }
        self.receiver_journal
            .lock()
            .map_err(|_| "RECEIVER_EVIDENCE_LOCK")?
            .save_missing(dir, id, self.stats().resources_released)
    }
    fn path_ready(&self, c: &Context) -> bool {
        if self.mode == PathMode::RelayOnly {
            return true;
        }
        #[cfg(feature = "p006-direct")]
        {
            c.exchange.as_ref().is_some_and(|e| e.ready())
        }
        #[cfg(not(feature = "p006-direct"))]
        {
            let _ = c;
            false
        }
    }
    fn media_server_ip(&self) -> Ipv4Addr {
        // An unused config fixture in the no-TURN local operator harness. The
        // browser harness removes ICE servers before constructing its real PC.
        if self.config.is_local_synthetic() {
            Ipv4Addr::new(8, 8, 8, 8)
        } else {
            self.config.server_ipv4
        }
    }
    fn browser_scope(&self, c: &Context) -> Result<Option<BrowserDirectScope>, String> {
        if self.mode == PathMode::RelayOnly {
            return Ok(None);
        }
        #[cfg(feature = "p006-direct")]
        {
            let e = c
                .exchange
                .as_ref()
                .filter(|e| e.ready())
                .ok_or("PATH_POLICY_NOT_READY")?;
            Ok(Some(BrowserDirectScope {
                server_ip: self.media_server_ip().to_string(),
                local_ip: self.local.to_string(),
                private_peer_ips: e.private_peers().iter().map(ToString::to_string).collect(),
                publish_private_local: e.may_publish_private_local(self.local),
                expires_at: c.ready.expires_at,
            }))
        }
        #[cfg(not(feature = "p006-direct"))]
        {
            let _ = c;
            Err("DIRECT_MODE_NOT_ENABLED".into())
        }
    }
    fn check_sdp(&self, c: &Context, sdp: &str, answer: bool, local: bool) -> Result<(), String> {
        if !self.path_ready(c) {
            return Err("PATH_POLICY_NOT_READY".into());
        }
        #[cfg(feature = "p006-direct")]
        if let Some(g) = &c.direct {
            let (_, candidates) =
                crate::signaling::validate_direct_video_description(sdp, g, now(), answer, local)?;
            let allowed = c
                .exchange
                .as_ref()
                .is_some_and(|e| e.may_publish_private_local(self.local));
            if local
                && !allowed
                && candidates.candidates().iter().any(|c| {
                    c.kind == crate::direct_policy::Kind::Host && c.address.ip().is_private()
                })
            {
                return Err("PRIVATE_LOCAL_NOT_APPROVED_BY_PEER".into());
            }
            return Ok(());
        }
        crate::signaling::validate_relay_description(
            sdp,
            c.grant.as_ref().ok_or("RELAY_GRANT_REQUIRED")?,
            now(),
            answer,
        )?;
        Ok(())
    }
    fn source_session(&self, c: &Context) -> Result<Arc<Session>, String> {
        if !self.path_ready(c) {
            return Err("PATH_POLICY_NOT_READY".into());
        }
        #[cfg(feature = "p006-direct")]
        if let Some(g) = &c.direct {
            #[cfg(feature = "p006-session-probe")]
            if self.config.synthetic_source {
                return Session::new_direct_synthetic_transport_test(
                    self.seconds,
                    crate::session::DirectSettings {
                        grant: g.clone(),
                        config: if self.config.is_local_synthetic() {
                            None
                        } else {
                            Some(ice_mode(&c.ready, self.mode)?)
                        },
                        publish_private_local: c
                            .exchange
                            .as_ref()
                            .is_some_and(|e| e.may_publish_private_local(self.local)),
                    },
                    c.ready.session_id.clone(),
                    self.config.diagnostic_drop_direct,
                    self.config.operator_role_scoped,
                );
            }
            return Session::new_direct(
                self.seconds,
                crate::session::DirectSettings {
                    grant: g.clone(),
                    config: Some(ice_mode(&c.ready, self.mode)?),
                    publish_private_local: c
                        .exchange
                        .as_ref()
                        .is_some_and(|e| e.may_publish_private_local(self.local)),
                },
                c.ready.session_id.clone(),
            );
        }
        if self.mode != PathMode::RelayOnly {
            return Err("DIRECT_GRANT_REQUIRED".into());
        }
        let relay = RelaySettings {
            grant: c.grant.as_ref().ok_or("RELAY_GRANT_REQUIRED")?.clone(),
            config: ice_mode(&c.ready, self.mode)?,
        };
        #[cfg(feature = "p006-session-probe")]
        if self.config.synthetic_source {
            return Session::new_relay_synthetic(self.seconds, relay, c.ready.session_id.clone());
        }
        Session::new_relay(self.seconds, relay, c.ready.session_id.clone())
    }
    #[cfg(feature = "p006-direct")]
    fn approved_direct_grant(
        &self,
        ready: &Ready,
        target: RelayTarget,
    ) -> Result<Arc<crate::direct_policy::DirectGrant>, String> {
        #[cfg(feature = "p006-session-probe")]
        if self.config.is_local_synthetic() {
            return Ok(Arc::new(
                crate::direct_policy::DirectGrant::local_session_diagnostic(
                    self.local,
                    ready.expires_at,
                    &ready.session_id,
                    now(),
                )?,
            ));
        }
        Ok(Arc::new(crate::direct_policy::DirectGrant::new(
            RelayGrant::new(target, &ready.session_id, ready.expires_at, true)?,
            self.private_peers.clone(),
            &ice_mode(ready, self.mode)?,
            now(),
        )?))
    }
    fn validate_metrics(&self, data: &serde_json::Value) -> Result<(), String> {
        if self.mode == PathMode::RelayOnly {
            #[cfg(feature = "p006-session-probe")]
            if self.config.synthetic_source {
                return crate::lan::validate_metrics(
                    &crate::relay_diagnostic::metrics_without_path(data)?,
                );
            }
            return crate::lan::validate_metrics(data);
        }
        let mut limited = data.clone();
        let value = limited
            .as_object_mut()
            .ok_or("METRICS_INVALID")?
            .remove("connectionPath")
            .ok_or("DIRECT_PATH_METRIC_REQUIRED")?;
        let path = value.as_str().ok_or("DIRECT_PATH_METRIC_INVALID")?;
        if !["direct", "relay", "unknown"].contains(&path) {
            return Err("DIRECT_PATH_METRIC_INVALID".into());
        }
        let a = data["localCandidateType"].as_str();
        let b = data["remoteCandidateType"].as_str();
        let actual = if a == Some("relay") || b == Some("relay") {
            "relay"
        } else if a.is_some_and(|c| ["host", "srflx"].contains(&c))
            && b.is_some_and(|c| ["host", "srflx"].contains(&c))
        {
            "direct"
        } else {
            "unknown"
        };
        if data["framesDecoded"].as_u64().unwrap_or(0) > 0 && (path == "unknown" || path != actual)
        {
            return Err("DIRECT_PATH_METRIC_MISMATCH".into());
        }
        crate::lan::validate_metrics(&limited)
    }
    async fn worker(&self, key: Arc<PairKey>) -> Result<(), String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut roots = rustls::RootCertStore::empty();
        let certs = rustls_pemfile::certs(&mut BufReader::new(self.config.ca_pem.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "P006_CA_INVALID")?;
        for c in certs {
            roots.add(c).map_err(|_| "P006_CA_INVALID")?;
        }
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let mut request = format!(
            "wss://{}:{}/probe",
            self.config.server_ipv4, self.config.signaling_port
        )
        .into_client_request()
        .map_err(|_| "P006_URL_INVALID")?;
        let token = self.config.invitation(self.role)?;
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(|_| "P006_INVITATION_INVALID")?,
        );
        request.headers_mut().insert(
            "origin",
            "remotehub-p006b".parse().map_err(|_| "P006_ORIGIN")?,
        );
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME))
            .max_frame_size(Some(MAX_FRAME));
        let (mut ws, _) = timeout(
            Duration::from_secs(8),
            tokio_tungstenite::connect_async_tls_with_config(
                request,
                Some(config),
                true,
                Some(Connector::Rustls(Arc::new(tls))),
            ),
        )
        .await
        .map_err(|_| "P006_WSS_CONNECT_TIMEOUT")?
        .map_err(|_| "P006_WSS_TLS_OR_AUTH_FAILED")?;
        let hello = Hello {
            mode: self.mode,
            version: 1,
            role: self.role,
            room: key.room(),
            seconds: self.seconds,
        };
        ws.send(Message::Text(
            serde_json::to_string(&hello)
                .map_err(|_| "P006_ENCODING")?
                .into(),
        ))
        .await
        .map_err(|_| "P006_WSS_WRITE")?;
        let ready=timeout(Duration::from_secs(122),async{loop{if self.cancelled.load(Ordering::Acquire){return Err("P006_STOPPED".to_string());}tokio::select!{m=ws.next()=>{let m=m.ok_or("P006_WSS_CLOSED")?.map_err(|_|"P006_WSS_CLOSED")?;if let Message::Text(text)=m{match serde_json::from_str::<ServerEvent>(&text).map_err(|_|"P006_SERVER_EVENT")?{ServerEvent::Waiting=>self.update(|s|s.state="waiting_peer".into()),ServerEvent::Failed{code}=>return Err(code),ServerEvent::Ready(r)=>return Ok(r)}}},_=sleep(Duration::from_millis(30))=>{}}}}).await.map_err(|_|"P006_PAIRING_TIMEOUT")??;
        let target = RelayTarget {
            local: self.local,
            turn: SocketAddrV4::new(self.media_server_ip(), 3478),
            relay: self.media_server_ip(),
            port_min: 49160,
            port_max: 49259,
        };
        let grant = if self.config.is_local_synthetic() {
            None
        } else {
            Some(Arc::new(RelayGrant::new(
                target,
                &ready.session_id,
                ready.expires_at,
                true,
            )?))
        };
        if self.mode == PathMode::RelayOnly {
            ice_mode(&ready, self.mode)?
                .validate(grant.as_ref().ok_or("RELAY_GRANT_REQUIRED")?, now())?;
        }
        #[cfg(feature = "p006-direct")]
        let direct = if self.mode == PathMode::DirectPreferred {
            Some(self.approved_direct_grant(&ready, target)?)
        } else {
            None
        };
        #[cfg(feature = "p006-direct")]
        let exchange = if self.mode == PathMode::DirectPreferred {
            Some(hub_probe_wire::PathExchange::new(
                self.private_peers.clone(),
            )?)
        } else {
            None
        };
        let mut codec = Codec::new(key, &ready, self.role, now())?;
        #[cfg(feature = "p006-direct")]
        if let Some(e) = &exchange {
            timeout(
                Duration::from_secs(2),
                ws.send(Message::Text(
                    codec.encode("path", e.payload()?, now())?.into(),
                )),
            )
            .await
            .map_err(|_| "P006_WSS_WRITE_TIMEOUT")?
            .map_err(|_| "P006_WSS_WRITE")?;
        }
        *self.context.lock().unwrap_or_else(|x| x.into_inner()) = Some(Context {
            grant: grant.clone(),
            ready,
            #[cfg(feature = "p006-direct")]
            direct,
            #[cfg(feature = "p006-direct")]
            exchange,
        });
        self.update(|s| {
            s.authenticated = self.mode == PathMode::RelayOnly;
            s.state = "negotiating".into();
        });
        self.notify.notify_one();
        let mut rx = self
            .receiver
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .take()
            .ok_or("P006_WORKER_ALREADY_STARTED")?;
        let mut beat = tokio::time::interval(Duration::from_secs(1));
        let mut last_rx = Instant::now();
        let started = Instant::now();
        let mut source_finished = None;
        let mut signal_task: Option<JoinHandle<Result<String, String>>> = None;
        let result=async {loop{
            if self.cancelled.load(Ordering::Acquire){let _=timeout(Duration::from_millis(600),ws.send(Message::Text(codec.encode("stop",String::new(),now())?.into()))).await;return Ok(());}
            if last_rx.elapsed()>Duration::from_secs(4){return Err("P006_SIGNAL_LEASE_EXPIRED".into());}
            if started.elapsed()>Duration::from_secs(if self.mode==PathMode::DirectPreferred {110}else{60}){return Err("P006_SESSION_DEADLINE".into());}
            let source=self.source.lock().unwrap_or_else(|x|x.into_inner()).clone();
            if let Some(source)=source {
                let s=source.stats();
                if self.mode==PathMode::DirectPreferred&&s.state=="draining" {
                    let _=timeout(Duration::from_secs(1),ws.send(Message::Text(codec.encode("stop",String::new(),now())?.into()))).await;
                    return Ok(());
                }
                if s.resources_released {
                    let ended=source_finished.get_or_insert_with(Instant::now);
                    if ended.elapsed()>Duration::from_secs(2)||s.error.is_some() {
                        let _=timeout(Duration::from_secs(1),ws.send(Message::Text(codec.encode("stop",String::new(),now())?.into()))).await;
                        return if let Some(e)=s.error{Err(e)}else{Ok(())};
                    }
                }
            }
            tokio::select!{
                input=ws.next()=>{let input=input.ok_or("P006_WSS_CLOSED")?.map_err(|_|"P006_WSS_CLOSED")?;match input{Message::Text(text)=>{
                    if let Ok(ServerEvent::Failed{code})=serde_json::from_str::<ServerEvent>(&text){if code=="PEER_STOPPED"{return Ok(());}return Err(code);}
                    let(kind,payload)=codec.accept(&text,now())?;last_rx=Instant::now();self.update(|s|s.messages_received+=1);
                    match kind.as_str(){
                        "beat" if payload.is_empty()=>{},"stop" if payload.is_empty()=>return Ok(()),
                        #[cfg(feature="p006-direct")]
                        "path" if self.mode==PathMode::DirectPreferred=>{
                            let mut c=self.context.lock().unwrap_or_else(|x|x.into_inner());c.as_mut().ok_or("P006_CONTEXT_CLOSED")?.exchange.as_mut().ok_or("PATH_POLICY_NOT_ENABLED")?.accept(&payload)?;
                            self.update(|s|s.authenticated=true);self.notify.notify_one();
                        }
                        "offer" if self.role==Role::Sender=>{
                            if self.source.lock().unwrap_or_else(|x|x.into_inner()).is_some(){return Err("P006_OFFER_REPLAY".into());}
                            let source={let context=self.context.lock().unwrap_or_else(|x|x.into_inner());let c=context.as_ref().ok_or("P006_CONTEXT_CLOSED")?;self.check_sdp(c,&payload,false,false)?;self.source_session(c)?};
                            *self.source.lock().unwrap_or_else(|x|x.into_inner())=Some(source.clone());
                            signal_task=Some(tokio::spawn(async move{source.signal(payload).await}));
                        }
                        "answer" if self.role==Role::Receiver=>{let c=self.context.lock().unwrap_or_else(|x|x.into_inner());self.check_sdp(c.as_ref().ok_or("P006_CONTEXT_CLOSED")?,&payload,true,false)?;self.answer.lock().unwrap_or_else(|x|x.into_inner()).take().ok_or("P006_UNEXPECTED_ANSWER")?.send(Ok(payload)).map_err(|_|"P006_RECEIVER_VIEW_CLOSED")?;self.update(|s|s.state="connected".into());}
                        "metrics" if self.role==Role::Sender=>{let data=serde_json::from_str(&payload).map_err(|_|"METRICS_INVALID")?;self.validate_metrics(&data)?;self.update(|s|{s.receiver_samples.push(data);if s.receiver_samples.len()>60{s.receiver_samples.remove(0);}});}
                        _=>return Err("P006_SIGNAL_KIND_REJECTED".into())
                    }
                },Message::Ping(_)|Message::Pong(_)=>{},Message::Close(_)=>return Err("P006_WSS_CLOSED".into()),_=>return Err("P006_WSS_FRAME".into())}},
                item=rx.recv()=>{let(kind,payload)=item.ok_or("P006_STOPPED")?;timeout(Duration::from_secs(2),ws.send(Message::Text(codec.encode(&kind,payload,now())?.into()))).await.map_err(|_|"P006_WSS_WRITE_TIMEOUT")?.map_err(|_|"P006_WSS_WRITE")?;},
                answer=async{signal_task.as_mut().expect("guarded signal task").await},if signal_task.is_some()=>{signal_task.take();let answer=answer.map_err(|_|"P006_SIGNAL_TASK_FAILED")??;timeout(Duration::from_secs(2),ws.send(Message::Text(codec.encode("answer",answer,now())?.into()))).await.map_err(|_|"P006_WSS_WRITE_TIMEOUT")?.map_err(|_|"P006_WSS_WRITE")?;},
                _=beat.tick()=>{timeout(Duration::from_secs(2),ws.send(Message::Text(codec.encode("beat",String::new(),now())?.into()))).await.map_err(|_|"P006_WSS_WRITE_TIMEOUT")?.map_err(|_|"P006_WSS_WRITE")?;},
                _=sleep(Duration::from_millis(30))=>{}
            }
        }}.await;
        if let Some(task) = signal_task {
            task.abort();
            let _ = task.await;
        }
        let _ = timeout(Duration::from_secs(1), ws.close(None)).await;
        result
    }
    async fn finish(&self, result: Result<(), String>) {
        self.cancel();
        let source = self
            .source
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .clone();
        let clean = if let Some(source) = source {
            source.stop().await.is_ok() && source.stats().resources_released
        } else {
            true
        };
        if let Some(tx) = self.answer.lock().unwrap_or_else(|x| x.into_inner()).take() {
            let _ = tx.send(Err(result
                .as_ref()
                .err()
                .cloned()
                .unwrap_or("P006_STOPPED".into())));
        }
        self.context
            .lock()
            .unwrap_or_else(|x| x.into_inner())
            .take();
        self.update(|s| {
            s.error = result.err().filter(|e| e != "P006_STOPPED");
            if !clean {
                s.error = Some("P006_CLEANUP_INCOMPLETE".into());
            }
            s.resources_released = clean;
            s.state = if s.error.is_some() {
                "failed"
            } else {
                "stopped"
            }
            .into();
        });
        self.notify.notify_one();
    }
}
fn ice_mode(ready: &Ready, mode: PathMode) -> Result<IceConfig, String> {
    let value = serde_json::json!({"mode":mode,"session_id":ready.session_id,"expires_at":ready.expires_at,"ice_servers":ready.ice_servers});
    serde_json::from_value(value).map_err(|_| "P006_ICE_CONFIG_INVALID".into())
}

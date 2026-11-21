//! Native WSS signaling only. Video/control payloads remain on the same WebRTC PC.
use crate::{
    ice_policy::{IceConfig, RelayGrant, RelayTarget},
    lan::LanStatus,
    session::{RelaySettings, Session},
};
use futures_util::{SinkExt, StreamExt};
use hub_probe_wire::{now, Codec, Hello, PairKey, Ready, Role, ServerEvent, MAX_FRAME};
use serde::{Deserialize, Serialize};
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
use zeroize::Zeroize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateConfig {
    pub server_ipv4: Ipv4Addr,
    pub signaling_port: u16,
    pub ca_pem: String,
    pub sender_invitation: String,
    pub receiver_invitation: String,
    pub invitation_expires_at: u64,
}
impl Drop for PrivateConfig {
    fn drop(&mut self) {
        self.sender_invitation.zeroize();
        self.receiver_invitation.zeroize();
    }
}
impl PrivateConfig {
    pub fn load(path: &std::path::Path) -> Result<Arc<Self>, String> {
        let info = std::fs::symlink_metadata(path).map_err(|_| "P006_PRIVATE_CONFIG_REQUIRED")?;
        if !info.is_file() || info.file_type().is_symlink() || info.len() > 16384 {
            return Err("P006_CONFIG_FILE_POLICY".into());
        }
        let data =
            zeroize::Zeroizing::new(std::fs::read_to_string(path).map_err(|_| "P006_CONFIG_READ")?);
        let c: Self = serde_json::from_str(&data).map_err(|_| "P006_CONFIG_SCHEMA")?;
        if !crate::ice_policy::public_v4(c.server_ipv4)
            || c.signaling_port < 1024
            || c.sender_invitation.len() != 64
            || c.receiver_invitation.len() != 64
            || c.sender_invitation == c.receiver_invitation
            || !c
                .sender_invitation
                .bytes()
                .chain(c.receiver_invitation.bytes())
                .all(|b| b.is_ascii_hexdigit())
            || !(60..=7200).contains(&c.invitation_expires_at.saturating_sub(now()))
        {
            return Err("P006_CONFIG_EXPIRED_OR_INVALID".into());
        }
        Ok(Arc::new(c))
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserReady {
    pub control_session: String,
    pub server_ip: String,
    pub expires_at: u64,
    pub ice: IceConfig,
}
struct Context {
    grant: Arc<RelayGrant>,
    ready: Ready,
}
pub struct RelayClient {
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
                        .filter(|_| !self.cancelled.load(Ordering::Acquire))
                    {
                        return Ok(BrowserReady {
                            control_session: c.ready.session_id.clone(),
                            server_ip: self.config.server_ipv4.to_string(),
                            expires_at: c.ready.expires_at,
                            ice: ice(&c.ready)?,
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
            crate::signaling::validate_relay_description(
                &offer,
                &c.as_ref().ok_or("P006_NOT_READY")?.grant,
                now(),
                false,
            )?;
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
        crate::lan::validate_metrics(&data)?;
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
        let token = if self.role == Role::Sender {
            &self.config.sender_invitation
        } else {
            &self.config.receiver_invitation
        };
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
        let ready=timeout(Duration::from_secs(122),async{loop{if self.cancelled.load(Ordering::Acquire){return Err("P006_STOPPED".to_string());}tokio::select!{m=ws.next()=>{let m=m.ok_or("P006_WSS_CLOSED")?.map_err(|_|"P006_WSS_CLOSED")?;if let Message::Text(text)=m{match serde_json::from_str::<ServerEvent>(&text).map_err(|_|"P006_SERVER_EVENT")?{ServerEvent::Waiting=>{},ServerEvent::Failed{code}=>return Err(code),ServerEvent::Ready(r)=>return Ok(r)}}},_=sleep(Duration::from_millis(30))=>{}}}}).await.map_err(|_|"P006_PAIRING_TIMEOUT")??;
        let target = RelayTarget {
            local: self.local,
            turn: SocketAddrV4::new(self.config.server_ipv4, 3478),
            relay: self.config.server_ipv4,
            port_min: 49160,
            port_max: 49259,
        };
        let grant = Arc::new(RelayGrant::new(
            target,
            &ready.session_id,
            ready.expires_at,
            true,
        )?);
        ice(&ready)?.validate(&grant, now())?;
        let mut codec = Codec::new(key, &ready, self.role, now())?;
        *self.context.lock().unwrap_or_else(|x| x.into_inner()) = Some(Context {
            grant: grant.clone(),
            ready,
        });
        self.update(|s| {
            s.authenticated = true;
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
        let result=async {loop{
            if self.cancelled.load(Ordering::Acquire){let _=timeout(Duration::from_millis(600),ws.send(Message::Text(codec.encode("stop",String::new(),now())?.into()))).await;return Ok(());}
            if last_rx.elapsed()>Duration::from_secs(4){return Err("P006_SIGNAL_LEASE_EXPIRED".into());}
            if started.elapsed()>Duration::from_secs(60){return Err("P006_SESSION_DEADLINE".into());}
            let source=self.source.lock().unwrap_or_else(|x|x.into_inner()).clone();
            if let Some(source)=source{let s=source.stats();if s.resources_released{let ended=source_finished.get_or_insert_with(Instant::now);if ended.elapsed()>Duration::from_secs(2)||s.error.is_some(){let _=ws.send(Message::Text(codec.encode("stop",String::new(),now())?.into())).await;return if let Some(e)=s.error{Err(e)}else{Ok(())};}}}
            tokio::select!{
                input=ws.next()=>{let input=input.ok_or("P006_WSS_CLOSED")?.map_err(|_|"P006_WSS_CLOSED")?;match input{Message::Text(text)=>{
                    if let Ok(ServerEvent::Failed{code})=serde_json::from_str::<ServerEvent>(&text){if code=="PEER_STOPPED"{return Ok(());}return Err(code);}
                    let(kind,payload)=codec.accept(&text,now())?;last_rx=Instant::now();self.update(|s|s.messages_received+=1);
                    match kind.as_str(){
                        "beat" if payload.is_empty()=>{},"stop" if payload.is_empty()=>return Ok(()),
                        "offer" if self.role==Role::Sender=>{
                            crate::signaling::validate_relay_description(&payload,&grant,now(),false)?;
                            if self.source.lock().unwrap_or_else(|x|x.into_inner()).is_some(){return Err("P006_OFFER_REPLAY".into());}
                            let source={let context=self.context.lock().unwrap_or_else(|x|x.into_inner());let r=&context.as_ref().ok_or("P006_CONTEXT_CLOSED")?.ready;Session::new_relay(self.seconds,RelaySettings{grant:grant.clone(),config:ice(r)?},r.session_id.clone())?};
                            *self.source.lock().unwrap_or_else(|x|x.into_inner())=Some(source.clone());
                            let answer=source.signal(payload).await?;
                            ws.send(Message::Text(codec.encode("answer",answer,now())?.into())).await.map_err(|_|"P006_WSS_WRITE")?;last_rx=Instant::now();
                        }
                        "answer" if self.role==Role::Receiver=>{crate::signaling::validate_relay_description(&payload,&grant,now(),true)?;self.answer.lock().unwrap_or_else(|x|x.into_inner()).take().ok_or("P006_UNEXPECTED_ANSWER")?.send(Ok(payload)).map_err(|_|"P006_RECEIVER_VIEW_CLOSED")?;self.update(|s|s.state="connected".into());}
                        "metrics" if self.role==Role::Sender=>{let data=serde_json::from_str(&payload).map_err(|_|"METRICS_INVALID")?;crate::lan::validate_metrics(&data)?;self.update(|s|{s.receiver_samples.push(data);if s.receiver_samples.len()>60{s.receiver_samples.remove(0);}});}
                        _=>return Err("P006_SIGNAL_KIND_REJECTED".into())
                    }
                },Message::Ping(_)|Message::Pong(_)=>{},Message::Close(_)=>return Err("P006_WSS_CLOSED".into()),_=>return Err("P006_WSS_FRAME".into())}},
                item=rx.recv()=>{let(kind,payload)=item.ok_or("P006_STOPPED")?;timeout(Duration::from_secs(2),ws.send(Message::Text(codec.encode(&kind,payload,now())?.into()))).await.map_err(|_|"P006_WSS_WRITE_TIMEOUT")?.map_err(|_|"P006_WSS_WRITE")?;},
                _=beat.tick()=>{timeout(Duration::from_secs(2),ws.send(Message::Text(codec.encode("beat",String::new(),now())?.into()))).await.map_err(|_|"P006_WSS_WRITE_TIMEOUT")?.map_err(|_|"P006_WSS_WRITE")?;},
                _=sleep(Duration::from_millis(30))=>{}
            }
        }}.await;
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
fn ice(ready: &Ready) -> Result<IceConfig, String> {
    let value = serde_json::json!({"mode":"relay_only","session_id":ready.session_id,"expires_at":ready.expires_at,"ice_servers":ready.ice_servers});
    serde_json::from_value(value).map_err(|_| "P006_ICE_CONFIG_INVALID".into())
}

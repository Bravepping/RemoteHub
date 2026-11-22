use crate::control::{ControlStats, Protocol, LABEL};
use hub_encode_win::lifecycle::StopToken;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};
use webrtc::data_channel::{DataChannel, DataChannelEvent};

pub struct Endpoint {
    close_grace: Duration,
    stop: StopToken,
    claimed: AtomicBool,
    permitted: AtomicBool,
    protocol: Mutex<Protocol>,
    incoming: mpsc::Sender<Arc<dyn DataChannel>>,
    receiver: Mutex<Option<mpsc::Receiver<Arc<dyn DataChannel>>>>,
}
impl Endpoint {
    pub fn new(session: String, stop: StopToken, seconds: u32) -> Result<Arc<Self>, String> {
        Self::with_permission(session, stop, seconds, true)
    }
    #[cfg(feature = "p006-relay")]
    pub(crate) fn new_guarded(
        session: String,
        stop: StopToken,
        seconds: u32,
    ) -> Result<Arc<Self>, String> {
        Self::with_permission(session, stop, seconds, false)
    }
    #[cfg(feature = "p006-direct")]
    pub(crate) fn new_direct_guarded(
        session: String,
        stop: StopToken,
        seconds: u32,
    ) -> Result<Arc<Self>, String> {
        let mut endpoint = Self::with_permission(session, stop, seconds, false)?;
        Arc::get_mut(&mut endpoint)
            .ok_or("CONTROL_ALREADY_SHARED")?
            .close_grace = Duration::from_secs(2);
        Ok(endpoint)
    }
    #[cfg(feature = "p006-relay")]
    pub(crate) fn approve_verified_path(&self) {
        if !self.stop.is_stopped() {
            self.permitted.store(true, Ordering::Release);
        }
    }
    fn with_permission(
        session: String,
        stop: StopToken,
        seconds: u32,
        permitted: bool,
    ) -> Result<Arc<Self>, String> {
        let (tx, rx) = mpsc::channel(1);
        Ok(Arc::new(Self {
            close_grace: Duration::ZERO,
            stop,
            claimed: AtomicBool::new(false),
            permitted: AtomicBool::new(permitted),
            protocol: Mutex::new(Protocol::new(
                session,
                Duration::from_secs(seconds as u64 + 15),
            )?),
            incoming: tx,
            receiver: Mutex::new(Some(rx)),
        }))
    }
    fn update(&self, f: impl FnOnce(&mut ControlStats)) {
        f(&mut self
            .protocol
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stats);
    }
    pub fn stats(&self) -> ControlStats {
        self.protocol
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stats
            .clone()
    }
    #[cfg(all(test, feature = "p006-direct"))]
    pub(crate) fn is_permitted(&self) -> bool {
        self.permitted.load(Ordering::Acquire)
    }
    pub fn joined(&self) {
        self.update(|s| s.task_joined = true);
    }
    pub fn accept(&self, channel: Arc<dyn DataChannel>) {
        if self.stop.is_stopped()
            || self.claimed.swap(true, Ordering::AcqRel)
            || self.incoming.try_send(channel).is_err()
        {
            self.fail("CONTROL_CHANNEL_NOT_ALLOWED");
        }
    }
    fn fail(&self, error: &str) {
        self.update(|s| {
            s.last_error = Some(error.into());
            s.state = "failed".into();
        });
        self.stop.stop();
    }
    pub async fn run(&self) {
        let mut rx = self
            .receiver
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .expect("control worker starts once");
        let channel = loop {
            tokio::select! {v=rx.recv()=>{match v{Some(c)=>break c,None=>return}},_=sleep(Duration::from_millis(25))=>{if self.stop.is_stopped(){self.update(|s|s.state="closed".into());return;}}}
        };
        let result = self.events(&channel).await;
        if let Err(error) = result {
            self.fail(&error);
        }
        self.update(|s| s.close_requested = true);
        // StopToken has already revoked message processing. Give the authenticated
        // signaling owner time to notify the viewer before tearing down SCTP.
        if self.stats().close_events == 0 {
            sleep(self.close_grace).await;
        }
        let _ = timeout(Duration::from_secs(1), channel.close()).await;
        let close = timeout(Duration::from_millis(800), async {
            while let Some(event) = channel.poll().await {
                if matches!(event, DataChannelEvent::OnClose) {
                    return true;
                }
            }
            false
        })
        .await;
        if matches!(close, Ok(true)) {
            self.update(|s| s.close_events += 1);
        }
        self.update(|s| {
            if s.last_error.is_none() {
                s.state = "closed".into();
            }
        });
    }
    async fn events(&self, channel: &Arc<dyn DataChannel>) -> Result<(), String> {
        let valid = timeout(Duration::from_secs(1), async {
            Ok::<bool, webrtc::error::Error>(
                channel.label().await? == LABEL
                    && channel.protocol().await? == LABEL
                    && channel.ordered().await?
                    && channel.max_retransmits().await?.is_none()
                    && channel.max_packet_life_time().await?.is_none()
                    && !channel.negotiated().await?,
            )
        })
        .await;
        if !matches!(valid, Ok(Ok(true))) {
            return Err("CONTROL_CHANNEL_PROPERTIES".into());
        }
        loop {
            if self.stop.is_stopped() {
                return Ok(());
            }
            tokio::select! {
             event=channel.poll()=>match event{
              Some(DataChannelEvent::OnOpen)=>self.update(|s|{s.open_events+=1;s.state="open".into();}),
              Some(DataChannelEvent::OnMessage(message))=>{
               self.update(|s|s.message_events+=1);
               let response=self.protocol.lock().unwrap_or_else(|p|p.into_inner()).process(&message.data,message.is_string,!self.stop.is_stopped() && self.permitted.load(Ordering::Acquire),Instant::now())?;
               let text=serde_json::to_string(&response).map_err(|_|"CONTROL_SERIALIZE")?;
               if self.stop.is_stopped() || !self.permitted.load(Ordering::Acquire){return Err("CONTROL_NOT_AUTHORIZED".into());}
               timeout(Duration::from_millis(500),channel.try_send_text(&text)).await.map_err(|_|"CONTROL_SEND_TIMEOUT")?.map_err(|_|"CONTROL_SEND_FAILED")?;
               self.protocol.lock().unwrap_or_else(|p|p.into_inner()).sent(&response);
              },
              Some(DataChannelEvent::OnError)=>{self.update(|s|s.error_events+=1);return Err("CONTROL_CHANNEL_ERROR".into());},
              Some(DataChannelEvent::OnClose)=>{self.update(|s|s.close_events+=1);self.stop.stop();return Ok(());},
              None=>{self.stop.stop();return Ok(());},
              _=>(),
             },
             _=sleep(Duration::from_millis(25))=>(),
            }
        }
    }
}

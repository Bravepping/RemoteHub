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

#[cfg(feature = "p3-lan")]
struct InputLane {
    protocol: crate::remote_input::Protocol,
    injector: crate::remote_input::Injector,
}

#[cfg(all(test, feature = "p3-lan"))]
mod input_tests {
    use super::*;
    #[test]
    fn input_requires_verified_permission_and_cannot_resume_after_stop() {
        let id = uuid::Uuid::new_v4().to_string();
        let stop = StopToken::default();
        let endpoint = Endpoint::new_input(id.clone(), stop.clone(), 30).unwrap();
        let msg = |seq| {
            serde_json::json!({"v":1,"seq":seq,"session_id":id,"event":{"kind":"release_all"}})
                .to_string()
        };
        assert!(endpoint.process(msg(1).as_bytes(), true).is_err());
        assert_eq!(endpoint.stats().accepted_moves, 0);
        endpoint.approve_verified_path();
        assert!(endpoint.process(msg(1).as_bytes(), true).is_ok());
        stop.stop();
        endpoint.revoke_input();
        endpoint.approve_verified_path();
        assert!(endpoint.process(msg(2).as_bytes(), true).is_err());
        assert_eq!(endpoint.stats().accepted_moves, 1);
        assert!(endpoint.cleanup_complete());
    }
}
pub struct Endpoint {
    #[cfg(feature = "p3-lan")]
    input_release_failed: AtomicBool,
    #[cfg(feature = "p3-lan")]
    input: Mutex<Option<InputLane>>,
    close_grace: Duration,
    stop: StopToken,
    claimed: AtomicBool,
    permitted: AtomicBool,
    protocol: Mutex<Protocol>,
    incoming: mpsc::Sender<Arc<dyn DataChannel>>,
    receiver: Mutex<Option<mpsc::Receiver<Arc<dyn DataChannel>>>>,
}
impl Endpoint {
    #[cfg(feature = "p3-lan")]
    pub(crate) fn new_input(
        session: String,
        stop: StopToken,
        seconds: u32,
    ) -> Result<Arc<Self>, String> {
        let this = Self::with_permission(session.clone(), stop, seconds, false)?;
        *this.input.lock().map_err(|_| "INPUT_STATE_FAILED")? = Some(InputLane {
            protocol: crate::remote_input::Protocol::new(session, seconds + 15),
            injector: Default::default(),
        });
        Ok(this)
    }
    #[cfg(feature = "p3-lan")]
    pub(crate) fn is_input(&self) -> bool {
        self.input.lock().map(|v| v.is_some()).unwrap_or(true)
    }
    #[cfg(feature = "p3-lan")]
    pub(crate) fn revoke_input(&self) {
        if !self.is_input() {
            return;
        }
        self.permitted.store(false, Ordering::Release);
        if let Ok(mut lane) = self.input.lock() {
            if let Some(lane) = lane.as_mut() {
                if lane.injector.release().is_err() {
                    self.input_release_failed.store(true, Ordering::Release);
                    self.update(|s| s.last_error = Some("INPUT_RELEASE_FAILED".into()));
                } else {
                    self.input_release_failed.store(false, Ordering::Release);
                }
            }
        }
    }
    fn label(&self) -> &'static str {
        #[cfg(feature = "p3-lan")]
        if self.is_input() {
            return crate::remote_input::LABEL;
        }
        LABEL
    }
    pub(crate) fn cleanup_complete(&self) -> bool {
        #[cfg(feature = "p3-lan")]
        {
            !self.input_release_failed.load(Ordering::Acquire)
        }
        #[cfg(not(feature = "p3-lan"))]
        {
            true
        }
    }
    fn process(&self, bytes: &[u8], text: bool) -> Result<crate::control::Message, String> {
        #[cfg(feature = "p3-lan")]
        {
            let mut lane = self.input.lock().map_err(|_| "INPUT_STATE_FAILED")?;
            if let Some(lane) = lane.as_mut() {
                // Read revocation after acquiring the same lock used by release/stop.
                let permitted = !self.stop.is_stopped() && self.permitted.load(Ordering::Acquire);
                let result = lane.protocol.process(
                    bytes,
                    text,
                    permitted,
                    Instant::now(),
                    &mut lane.injector,
                );
                let seq = match result {
                    Ok(seq) => seq,
                    Err(e) => {
                        self.update(|s| s.rejected += 1);
                        return Err(e);
                    }
                };
                let mut p = self.protocol.lock().map_err(|_| "INPUT_STATE_FAILED")?;
                p.stats.accepted_moves += 1;
                p.stats.last_rx_seq = seq;
                return Ok(crate::control::Message::Ack {
                    v: 1,
                    seq: p.stats.last_tx_seq + 1,
                    session_id: p.session_id().into(),
                    ack_seq: seq,
                });
            }
        }
        self.protocol
            .lock()
            .map_err(|_| "CONTROL_STATE_FAILED")?
            .process(
                bytes,
                text,
                !self.stop.is_stopped() && self.permitted.load(Ordering::Acquire),
                Instant::now(),
            )
    }

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
    #[cfg(any(feature = "p006-relay", feature = "p3-lan"))]
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
            #[cfg(feature = "p3-lan")]
            input_release_failed: AtomicBool::new(false),
            #[cfg(feature = "p3-lan")]
            input: Mutex::new(None),
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
        #[cfg(feature = "p3-lan")]
        self.revoke_input();
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
                channel.label().await? == self.label()
                    && channel.protocol().await? == self.label()
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
            #[cfg(feature = "p3-lan")]
            if self.is_input() && crate::remote_input::Injector::emergency_pressed() {
                self.stop.stop();
            }
            if self.stop.is_stopped() {
                return Ok(());
            }
            tokio::select! {
             event=channel.poll()=>match event{
              Some(DataChannelEvent::OnOpen)=>self.update(|s|{s.open_events+=1;s.state="open".into();}),
              Some(DataChannelEvent::OnMessage(message))=>{
               self.update(|s|s.message_events+=1);
               let response=self.process(&message.data,message.is_string)?;
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

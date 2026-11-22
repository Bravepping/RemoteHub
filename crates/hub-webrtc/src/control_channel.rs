use crate::control::{ControlStats, Protocol, LABEL};
use hub_encode_win::lifecycle::StopToken;
use std::{
    future::Future,
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

    #[test]
    fn draining_never_reopens_input_and_releases_empty_injector() {
        let id = uuid::Uuid::new_v4().to_string();
        let stop = StopToken::default();
        let endpoint = Endpoint::new_input(id.clone(), stop.clone(), 30).unwrap();
        endpoint.approve_verified_path();
        endpoint.begin_input_drain();
        endpoint.approve_verified_path();
        let release = serde_json::json!({
            "v":1,"seq":1,"session_id":id,"event":{"kind":"release_all"},
        })
        .to_string();
        assert!(endpoint.process(release.as_bytes(), true).is_err());
        assert!(!stop.is_stopped());
        assert!(endpoint.cleanup_complete());
    }
}
pub struct Endpoint {
    #[cfg(feature = "p3-lan")]
    input_release_failed: AtomicBool,
    #[cfg(feature = "p3-lan")]
    input_quiesced: AtomicBool,
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
    #[cfg(feature = "p3-lan")]
    pub(crate) fn begin_input_drain(&self) {
        self.input_quiesced.store(true, Ordering::Release);
        self.revoke_input();
    }

    fn input_quiesced(&self) -> bool {
        #[cfg(feature = "p3-lan")]
        {
            self.input_quiesced.load(Ordering::Acquire)
        }
        #[cfg(not(feature = "p3-lan"))]
        {
            false
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
                let permitted = !self.stop.is_stopped()
                    && !self.input_quiesced()
                    && self.permitted.load(Ordering::Acquire);
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
        if !self.stop.is_stopped() && !self.input_quiesced() {
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
            input_quiesced: AtomicBool::new(false),
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
        // The authenticated peer can finish DCEP at the same moment as local
        // teardown. No worker may claim permission after the stop token is set.
        if self.stop.is_stopped() {
            return;
        }
        if (self.claimed.swap(true, Ordering::AcqRel) || self.incoming.try_send(channel).is_err())
            && !self.stop.is_stopped()
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
                match event {
                    DataChannelEvent::OnClose => return true,
                    DataChannelEvent::OnError => self.update(|s| s.error_events += 1),
                    _ => (),
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
        if self.stop.is_stopped() {
            return Ok(());
        }
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
        if self.stop.is_stopped() {
            return Ok(());
        }
        if !matches!(valid, Ok(Ok(true))) {
            return Err("CONTROL_CHANNEL_PROPERTIES".into());
        }
        self.event_loop(
            || channel.poll(),
            |text| async move {
                timeout(Duration::from_millis(500), channel.try_send_text(&text))
                    .await
                    .map_err(|_| "CONTROL_SEND_TIMEOUT")?
                    .map_err(|_| "CONTROL_SEND_FAILED")
                    .map(|_| ())
            },
        )
        .await
    }

    async fn event_loop<Poll, Send, Event, Sent>(
        &self,
        mut poll: Poll,
        mut send: Send,
    ) -> Result<(), String>
    where
        Poll: FnMut() -> Event,
        Event: Future<Output = Option<DataChannelEvent>>,
        Send: FnMut(String) -> Sent,
        Sent: Future<Output = Result<(), &'static str>>,
    {
        loop {
            #[cfg(feature = "p3-lan")]
            if self.is_input() && crate::remote_input::Injector::emergency_pressed() {
                self.stop.stop();
            }
            if self.stop.is_stopped() {
                return Ok(());
            }
            let event = tokio::select! {
                event = poll() => event,
                _ = sleep(Duration::from_millis(25)) => continue,
            };
            // In particular, an error event already queued when the owner
            // stopped should remain diagnostic, not turn teardown into failure.
            if matches!(&event, Some(DataChannelEvent::OnError)) {
                self.update(|s| s.error_events += 1);
                if self.stop.is_stopped() {
                    return Ok(());
                }
                return Err("CONTROL_CHANNEL_ERROR".into());
            }
            if matches!(&event, Some(DataChannelEvent::OnClose)) {
                self.update(|s| s.close_events += 1);
                self.stop.stop();
                return Ok(());
            }
            if self.stop.is_stopped() {
                return Ok(());
            }
            match event {
                Some(DataChannelEvent::OnOpen) => self.update(|s| {
                    s.open_events += 1;
                    s.state = "open".into();
                }),
                Some(DataChannelEvent::OnClosing) => {
                    // A peer-initiated SCTP stream reset revokes input at the
                    // start of closing, not after the final close notification.
                    self.stop.stop();
                    self.update(|s| s.state = "closing".into());
                    return Ok(());
                }
                Some(DataChannelEvent::OnMessage(message)) => {
                    self.update(|s| s.message_events += 1);
                    if self.input_quiesced() {
                        continue;
                    }
                    let response = self.process(&message.data, message.is_string);
                    if self.stop.is_stopped() {
                        return Ok(());
                    }
                    if self.input_quiesced() {
                        continue;
                    }
                    let response = response?;
                    let text = serde_json::to_string(&response).map_err(|_| "CONTROL_SERIALIZE")?;
                    if self.stop.is_stopped() {
                        return Ok(());
                    }
                    if self.input_quiesced() {
                        continue;
                    }
                    if !self.permitted.load(Ordering::Acquire) {
                        return Err("CONTROL_NOT_AUTHORIZED".into());
                    }
                    let result = send(text).await;
                    if result.is_err() && self.stop.is_stopped() {
                        return Ok(());
                    }
                    result?;
                    self.protocol
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .sent(&response);
                }
                // A terminated event stream without trusted local stop or
                // ordered OnClosing/OnClose is an unexpected disconnect.
                None => return Err("CONTROL_CHANNEL_ERROR".into()),
                _ => (),
            }
        }
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::collections::VecDeque;
    use std::future::ready;
    use webrtc::data_channel::RTCDataChannelMessage;

    #[tokio::test]
    async fn local_stop_wins_over_queued_transport_error() {
        let stop = StopToken::default();
        let endpoint = Endpoint::new(uuid::Uuid::new_v4().to_string(), stop.clone(), 10).unwrap();
        endpoint
            .event_loop(
                || {
                    stop.stop();
                    ready(Some(DataChannelEvent::OnError))
                },
                |_| ready(Err("unexpected send")),
            )
            .await
            .unwrap();
        assert!(stop.is_stopped());
        assert_eq!(endpoint.stats().error_events, 1);
        assert!(endpoint.stats().last_error.is_none());
    }

    #[tokio::test]
    async fn clean_peer_close_halts_before_buffered_messages() {
        for closing in [DataChannelEvent::OnClosing, DataChannelEvent::OnClose] {
            let stop = StopToken::default();
            let endpoint =
                Endpoint::new(uuid::Uuid::new_v4().to_string(), stop.clone(), 10).unwrap();
            let mut events = VecDeque::from([
                closing,
                DataChannelEvent::OnMessage(RTCDataChannelMessage::default()),
            ]);
            endpoint
                .event_loop(
                    || ready(events.pop_front()),
                    |_| ready(Err("unexpected send")),
                )
                .await
                .unwrap();
            assert!(stop.is_stopped());
            assert_eq!(events.len(), 1);
            assert_eq!(endpoint.stats().message_events, 0);
        }
    }

    #[tokio::test]
    async fn active_channel_error_and_unannounced_eof_are_failures() {
        for first in [Some(DataChannelEvent::OnError), None] {
            let stop = StopToken::default();
            let endpoint =
                Endpoint::new(uuid::Uuid::new_v4().to_string(), stop.clone(), 10).unwrap();
            let mut events = VecDeque::from([first.clone(), Some(DataChannelEvent::OnClose)]);
            let error = endpoint
                .event_loop(
                    || ready(events.pop_front().flatten()),
                    |_| ready(Err("unexpected send")),
                )
                .await
                .unwrap_err();
            assert_eq!(error, "CONTROL_CHANNEL_ERROR");
            assert_eq!(endpoint.stats().error_events, u32::from(first.is_some()));
            assert_eq!(events.len(), 1);
        }
    }

    #[tokio::test]
    async fn ack_failure_is_only_suppressed_when_owner_is_already_stopping() {
        for stopping in [false, true] {
            let stop = StopToken::default();
            let id = uuid::Uuid::new_v4().to_string();
            let endpoint = Endpoint::new(id.clone(), stop.clone(), 10).unwrap();
            let request = serde_json::to_vec(&crate::control::Message::Ping {
                v: 1,
                seq: 1,
                session_id: id,
            })
            .unwrap();
            let mut event = Some(DataChannelEvent::OnMessage(RTCDataChannelMessage {
                is_string: true,
                data: request.as_slice().into(),
            }));
            let result = endpoint
                .event_loop(
                    || ready(event.take()),
                    |_| {
                        if stopping {
                            stop.stop();
                        }
                        ready(Err("CONTROL_SEND_TIMEOUT"))
                    },
                )
                .await;
            if stopping {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.unwrap_err(), "CONTROL_SEND_TIMEOUT");
            }
            assert_eq!(endpoint.stats().pings_received, 1);
            assert_eq!(endpoint.stats().pongs_sent, 0);
        }
    }
}

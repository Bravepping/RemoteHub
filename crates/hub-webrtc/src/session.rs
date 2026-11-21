use crate::{
    signaling::{validate_offer, Negotiation},
    video::{Packetizer, FMTP, SSRC},
};
use hub_encode_win::{lifecycle::StopToken, stream::StreamReport, EncodeError, EncoderConfig};
use rtc::{
    ice::{mdns::MulticastDnsMode, network_type::NetworkType},
    rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
        RtpCodecKind,
    },
};
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{sleep, timeout},
};
use webrtc::{
    media_stream::{
        track_local::{static_rtp::TrackLocalStaticRTP, TrackLocal},
        MediaStreamTrack,
    },
    peer_connection::{
        register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder,
        PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceConnectionState,
        RTCIceGatheringState, RTCPeerConnectionState, RTCSessionDescription, RTCStatsReportEntry,
        Registry, SettingEngineBuilder, StatsSelector,
    },
};

#[derive(Clone, Default, Serialize)]
pub struct Stats {
    pub control: Option<crate::control::ControlStats>,
    pub state: String,
    pub elapsed_ms: u64,
    pub stream: StreamReport,
    pub frames_sent: u32,
    pub rtp_payload_bytes: u64,
    pub rtp_packets_written: u64,
    pub rtp_packets_sent: Option<u64>,
    pub rtp_bytes_sent: Option<u64>,
    pub remote_packets_lost: Option<i64>,
    pub rtcp_round_trip_time: Option<f64>,
    pub native_ice_state: String,
    pub native_connection_state: String,
    pub native_candidates: Vec<String>,
    pub negotiation: Negotiation,
    pub first_connected_ms: Option<u64>,
    pub first_au_ms: Option<u64>,
    pub last_rtp_timestamp: Option<u32>,
    pub source_sps_profile_level_id: Option<String>,
    pub ignored_constraint_set5: bool,
    pub backpressure_failures: u32,
    pub rtcp_packets_received: u64,
    pub pli_requests: u32,
    pub fir_requests: u32,
    pub resources_released: bool,
    pub rejected_udp_rx: u64,
    pub rejected_udp_tx: u64,
    pub error: Option<String>,
}

struct Shared {
    stats: Mutex<Stats>,
    stop: StopToken,
    keyframe: AtomicBool,
    connected: AtomicBool,
    gathering_done: AtomicBool,
    created: Instant,
}
impl Shared {
    fn update(&self, f: impl FnOnce(&mut Stats)) {
        let mut stats = self.stats.lock().unwrap_or_else(|p| p.into_inner());
        stats.elapsed_ms = self.created.elapsed().as_millis() as u64;
        f(&mut stats);
    }
}
struct Handler(Arc<Shared>, Option<Arc<crate::control_channel::Endpoint>>);
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_data_channel(&self, channel: Arc<dyn webrtc::data_channel::DataChannel>) {
        if let Some(control) = &self.1 {
            control.accept(channel);
        } else {
            self.0.stop.stop();
        }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let value = state.to_string();
        self.0.update(|s| {
            s.native_connection_state = value.clone();
            if value == "connected" {
                s.first_connected_ms.get_or_insert(s.elapsed_ms);
                s.state = "connected".into();
            }
        });
        if value == "connected" {
            self.0.connected.store(true, Ordering::Release);
        }
        if value == "failed" {
            self.0
                .update(|s| s.error = Some("PEER_CONNECTION_FAILED".into()));
            self.0.stop.stop();
        }
    }
    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        self.0.update(|s| s.native_ice_state = state.to_string());
    }
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state.to_string() == "complete" {
            self.0.gathering_done.store(true, Ordering::Release);
        }
    }
}

pub struct Session {
    control: Option<Arc<crate::control_channel::Endpoint>>,
    network: crate::scope::NetworkScope,
    socket_counters: Arc<crate::pinned_runtime::SocketCounters>,
    seconds: u32,
    shared: Arc<Shared>,
    started: AtomicBool,
    task: Mutex<Option<JoinHandle<()>>>,
    stop_lock: tokio::sync::Mutex<()>,
}
impl Session {
    pub fn new(seconds: u32) -> Result<Arc<Self>, String> {
        Self::with_network(seconds, crate::scope::NetworkScope::Loopback)
    }
    pub fn new_lan(seconds: u32, scope: crate::scope::LanScope) -> Result<Arc<Self>, String> {
        Self::with_network(seconds, crate::scope::NetworkScope::Lan(scope))
    }
    pub fn new_lan_control(
        seconds: u32,
        scope: crate::scope::LanScope,
        session: String,
    ) -> Result<Arc<Self>, String> {
        Self::configured(
            seconds,
            crate::scope::NetworkScope::Lan(scope),
            Some(session),
        )
    }
    fn with_network(
        seconds: u32,
        network: crate::scope::NetworkScope,
    ) -> Result<Arc<Self>, String> {
        Self::configured(seconds, network, None)
    }
    fn configured(
        seconds: u32,
        network: crate::scope::NetworkScope,
        control_session: Option<String>,
    ) -> Result<Arc<Self>, String> {
        if !(5..=30).contains(&seconds) {
            return Err("INVALID_DURATION_5_TO_30".into());
        }
        let stop = StopToken::default();
        let control = control_session
            .map(|id| crate::control_channel::Endpoint::new(id, stop.clone(), seconds))
            .transpose()?;
        Ok(Arc::new(Self {
            control,
            network,
            socket_counters: Arc::new(Default::default()),
            seconds,
            shared: Arc::new(Shared {
                stats: Mutex::new(Stats {
                    state: "starting".into(),
                    ..Default::default()
                }),
                stop,
                keyframe: AtomicBool::new(false),
                connected: AtomicBool::new(false),
                gathering_done: AtomicBool::new(false),
                created: Instant::now(),
            }),
            started: AtomicBool::new(false),
            task: Mutex::new(None),
            stop_lock: tokio::sync::Mutex::new(()),
        }))
    }
    pub fn stats(&self) -> Stats {
        let mut stats = self
            .shared
            .stats
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if !stats.resources_released {
            stats.elapsed_ms = self.shared.created.elapsed().as_millis() as u64;
        }
        stats.rejected_udp_rx = self.socket_counters.rejected_rx.load(Ordering::Relaxed);
        stats.rejected_udp_tx = self.socket_counters.rejected_tx.load(Ordering::Relaxed);
        stats.control = self.control.as_ref().map(|c| c.stats());
        stats
    }
    pub fn cancel(&self) {
        self.shared.stop.stop();
    }

    pub async fn signal(self: &Arc<Self>, offer: String) -> Result<String, String> {
        // Validate before starting any sockets or media. Only the native window owner calls this.
        let negotiation = if self.control.is_some() {
            crate::signaling::validate_control_description(&offer, self.network, false)?
        } else if self.network.is_lan() {
            crate::signaling::validate_description(&offer, self.network, false)?
        } else {
            validate_offer(&offer)?
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut slot = self.task.lock().unwrap_or_else(|p| p.into_inner());
            if self.shared.stop.is_stopped() {
                return Err("SESSION_STOPPED".into());
            }
            if self.started.swap(true, Ordering::AcqRel) {
                return Err("ALREADY_SIGNALED".into());
            }
            let this = self.clone();
            *slot = Some(tokio::spawn(async move {
                this.worker(offer, negotiation, tx).await;
            }));
        }
        timeout(Duration::from_secs(12), rx)
            .await
            .map_err(|_| "SIGNAL_TIMEOUT")?
            .map_err(|_| "SIGNAL_TASK_CLOSED")?
    }
    pub async fn stop(&self) -> Result<(), String> {
        let _stop = self.stop_lock.lock().await;
        self.cancel();
        self.shared.update(|s| {
            if !s.resources_released {
                s.state = "stopping".into();
            }
        });
        let task = self.task.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(mut task) = task {
            match timeout(Duration::from_secs(10), &mut task).await {
                Ok(result) => result.map_err(|_| "SESSION_TASK_PANICKED")?,
                Err(_) => {
                    *self.task.lock().unwrap_or_else(|p| p.into_inner()) = Some(task);
                    return Err("CLEANUP_PENDING".into());
                }
            }
        } else if !self.started.load(Ordering::Acquire) {
            self.shared.update(|s| {
                s.state = "stopped".into();
                s.resources_released = true;
            });
        }
        Ok(())
    }

    async fn worker(
        &self,
        offer: String,
        negotiation: Negotiation,
        answer_tx: oneshot::Sender<Result<String, String>>,
    ) {
        let mut answer_tx = Some(answer_tx);
        let mut producer = None;
        self.shared.update(|s| {
            s.state = "negotiating".into();
            s.negotiation = negotiation.clone();
        });
        let result = self.build_pc(negotiation.payload_type).await;
        if let Ok(pc) = result {
            let control_task = self.control.as_ref().map(|control| {
                let control = control.clone();
                tokio::spawn(async move {
                    control.run().await;
                })
            });
            let outcome = timeout(
                Duration::from_secs(self.seconds as u64 + 15),
                self.media(&pc, offer, &negotiation, &mut answer_tx, &mut producer),
            )
            .await
            .unwrap_or_else(|_| Err("SESSION_DEADLINE".into()));
            self.shared.stop.stop();
            let close = timeout(Duration::from_secs(2), pc.close()).await;
            let close_ok = matches!(close, Ok(Ok(())));
            let control_ok = if let Some(mut task) = control_task {
                match timeout(Duration::from_secs(3), &mut task).await {
                    Ok(Ok(())) => {
                        if let Some(c) = &self.control {
                            c.joined();
                        }
                        true
                    }
                    _ => {
                        task.abort();
                        let _ = task.await;
                        false
                    }
                }
            } else {
                true
            };
            let mut join_ok = true;
            if let Some(thread) = producer {
                let joined = tokio::task::spawn_blocking(move || thread.join());
                join_ok = matches!(
                    timeout(Duration::from_secs(5), joined).await,
                    Ok(Ok(Ok(())))
                );
            }
            self.shared.update(|s| {
                if let Err(error) = outcome {
                    if s.error.is_none() {
                        s.error = Some(error);
                    }
                }
                if !close_ok || !join_ok || !control_ok {
                    s.error = Some("RESOURCE_CLEANUP_INCOMPLETE".into());
                }
                if s.error.is_none() {
                    s.error = self.control.as_ref().and_then(|c| c.stats().last_error);
                }
                s.resources_released = close_ok && join_ok && control_ok;
                s.state = if s.error.is_some() {
                    "failed"
                } else {
                    "stopped"
                }
                .into();
            });
        } else if let Err(error) = result {
            self.shared.update(|s| {
                s.error = Some(error);
                s.state = "failed".into();
                s.resources_released = true;
            });
        }
        if let Some(tx) = answer_tx {
            let _ = tx.send(Err(self.stats().error.unwrap_or("SESSION_STOPPED".into())));
        }
    }

    async fn build_pc(&self, payload_type: u8) -> Result<Arc<dyn PeerConnection>, String> {
        let mut engine = MediaEngine::default();
        engine
            .register_codec(
                RTCRtpCodecParameters {
                    rtp_codec: codec(),
                    payload_type,
                },
                RtpCodecKind::Video,
            )
            .map_err(|e| e.to_string())?;
        let registry = register_default_interceptors(Registry::new(), &mut engine)
            .map_err(|e| e.to_string())?
            .with(
                rtc::interceptor::Slot::Custom(9500),
                Feedback::new(self.shared.clone()),
            );
        let settings = SettingEngineBuilder::new()
            .with_include_loopback_candidate(!self.network.is_lan())
            .with_network_types(vec![NetworkType::Udp4])
            .with_multicast_dns_mode(MulticastDnsMode::Disabled)
            .with_ice_timeouts(
                Some(Duration::from_secs(3)),
                Some(Duration::from_secs(6)),
                Some(Duration::from_secs(1)),
            );
        let settings=if self.control.is_some(){settings.with_sctp_max_message_size(rtc::peer_connection::configuration::setting_engine::SctpMaxMessageSize::Bounded(16*1024)).with_sctp_max_receive_buffer_size(64*1024)}else{settings}.build();
        let mut builder = PeerConnectionBuilder::new()
            .with_media_engine(engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(settings)
            .with_configuration(
                RTCConfigurationBuilder::default()
                    .with_ice_servers(vec![])
                    .build(),
            )
            .with_handler(Arc::new(Handler(self.shared.clone(), self.control.clone())))
            .with_udp_addrs(vec![format!("{}:0", self.network.local())]);
        if let crate::scope::NetworkScope::Lan(scope) = self.network {
            builder = builder.with_runtime(Arc::new(crate::pinned_runtime::PinnedRuntime {
                inner: webrtc::runtime::default_runtime().ok_or("NO_ASYNC_RUNTIME")?,
                local: scope.local.into(),
                peer: scope.peer.into(),
                counters: self.socket_counters.clone(),
            }));
        }
        if self.control.is_some() {
            builder = builder.with_data_channel_send_buffer_limit(16 * 1024);
        }
        let pc = builder.build().await.map_err(|e| e.to_string())?;
        Ok(Arc::new(pc))
    }

    async fn media(
        &self,
        pc: &Arc<dyn PeerConnection>,
        offer: String,
        negotiation: &Negotiation,
        answer_tx: &mut Option<oneshot::Sender<Result<String, String>>>,
        producer: &mut Option<std::thread::JoinHandle<()>>,
    ) -> Result<(), String> {
        let track = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            "remotehub-local".into(),
            "desktop".into(),
            "Live DXGI".into(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(SSRC),
                    ..Default::default()
                },
                codec: codec(),
                active: true,
                max_bitrate: 4_000_000,
                max_framerate: Some(30.0),
                ..Default::default()
            }],
        )));
        pc.add_track(track.clone())
            .await
            .map_err(|e| e.to_string())?;
        pc.set_remote_description(RTCSessionDescription::offer(offer).map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?;
        let answer = pc.create_answer(None).await.map_err(|e| e.to_string())?;
        pc.set_local_description(answer)
            .await
            .map_err(|e| e.to_string())?;
        timeout(Duration::from_secs(4), async {
            while !self.shared.gathering_done.load(Ordering::Acquire)
                && !self.shared.stop.is_stopped()
            {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| "ICE_GATHER_TIMEOUT")?;
        if self.shared.stop.is_stopped() {
            return Ok(());
        }
        let answer = pc
            .local_description()
            .await
            .ok_or("NO_LOCAL_DESCRIPTION")?
            .sdp;
        let candidates: Vec<_> = answer
            .lines()
            .filter(|l| l.starts_with("a=candidate:"))
            .collect();
        if candidates.is_empty()
            || candidates.iter().any(|c| {
                !c.contains(&format!(" {} ", self.network.local())) || !c.contains(" typ host")
            })
        {
            return Err("NATIVE_ICE_NOT_LOOPBACK".into());
        }
        self.shared.update(|s| {
            s.native_candidates = candidates
                .iter()
                .map(|_| {
                    if self.network.is_lan() {
                        "host/pinned-lan/udp"
                    } else {
                        "host/127.0.0.1/udp"
                    }
                    .into()
                })
                .collect()
        });
        if let Some(tx) = answer_tx.take() {
            tx.send(Ok(answer)).map_err(|_| "UI_SIGNAL_CANCELLED")?;
        }
        timeout(Duration::from_secs(8), async {
            while !self.shared.connected.load(Ordering::Acquire) && !self.shared.stop.is_stopped() {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| "ICE_CONNECT_TIMEOUT")?;
        if self.shared.stop.is_stopped() {
            return Ok(());
        }
        let (tx, mut rx) = mpsc::channel(3);
        let shared = self.shared.clone();
        let seconds = self.seconds;
        *producer = Some(
            std::thread::Builder::new()
                .name("remotehub-dxgi-mf".into())
                .spawn(move || {
                    let result = hub_encode_win::stream::run(
                        true,
                        EncoderConfig {
                            seconds,
                            ..Default::default()
                        },
                        shared.stop.clone(),
                        &shared.keyframe,
                        |report| shared.update(|s| s.stream = report.clone()),
                        |frame| enqueue(&tx, &shared, frame),
                    );
                    if let Err(error) = result {
                        if !shared.stop.is_stopped() {
                            shared.update(|s| s.error = Some(error.to_string()));
                        }
                        shared.stop.stop();
                    }
                })
                .map_err(|_| "MEDIA_THREAD_START_FAILED")?,
        );
        let mut packetizer = Packetizer::default();
        let mut last_stats = Instant::now();
        loop {
            if self.shared.stop.is_stopped() {
                break;
            }
            match timeout(Duration::from_millis(20), rx.recv()).await {
                Ok(Some(frame)) => {
                    let packets = packetizer.packets(frame, negotiation.payload_type)?;
                    self.shared.update(|s| {
                        s.source_sps_profile_level_id =
                            packetizer.source_sps_profile_level_id.clone();
                        s.ignored_constraint_set5 = packetizer.ignored_constraint_set5;
                    });
                    let last_timestamp = packets.last().map(|p| p.header.timestamp);
                    let count = packets.len() as u64;
                    let bytes = packets.iter().map(|p| p.payload.len() as u64).sum::<u64>();
                    timeout(Duration::from_secs(1), async {
                        for packet in packets {
                            track.write_rtp(packet).await.map_err(|e| e.to_string())?;
                        }
                        Ok::<(), String>(())
                    })
                    .await
                    .map_err(|_| "RTP_WRITE_TIMEOUT")??;
                    self.shared.update(|s| {
                        s.first_au_ms.get_or_insert(s.elapsed_ms);
                        s.frames_sent += 1;
                        s.rtp_payload_bytes += bytes;
                        s.rtp_packets_written += count;
                        s.last_rtp_timestamp = last_timestamp;
                    });
                }
                Ok(None) => break,
                Err(_) => (),
            }
            if last_stats.elapsed() > Duration::from_secs(1) {
                self.update_stats(pc).await;
                last_stats = Instant::now();
            }
        }
        drop(rx);
        if !self.shared.stop.is_stopped() {
            sleep(Duration::from_millis(700)).await;
        }
        self.update_stats(pc).await;
        Ok(())
    }

    async fn update_stats(&self, pc: &Arc<dyn PeerConnection>) {
        let report = pc.get_stats(Instant::now(), StatsSelector::None).await;
        for entry in report.iter() {
            match entry {
                RTCStatsReportEntry::OutboundRtp(s) => {
                    let sent = &s.sent_rtp_stream_stats;
                    self.shared.update(|v| {
                        v.rtp_packets_sent = Some(sent.packets_sent);
                        v.rtp_bytes_sent = Some(sent.bytes_sent);
                    });
                }
                RTCStatsReportEntry::RemoteInboundRtp(s) => {
                    let value = serde_json::to_value(s).unwrap_or_default();
                    self.shared.update(|v| {
                        v.remote_packets_lost = value["packetsLost"].as_i64();
                        v.rtcp_round_trip_time =
                            (s.round_trip_time_measurements > 0).then_some(s.round_trip_time);
                    });
                }
                _ => (),
            }
        }
    }
}

fn codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: "video/H264".into(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line: FMTP.into(),
        rtcp_feedback: vec![],
    }
}

fn enqueue(
    tx: &mpsc::Sender<hub_encode_win::h264::EncodedVideoFrame>,
    shared: &Shared,
    frame: hub_encode_win::h264::EncodedVideoFrame,
) -> Result<(), EncodeError> {
    if shared.stop.is_stopped() {
        return Err(EncodeError::Native("STREAM_CANCELLED".into()));
    }
    tx.try_send(frame).map_err(|_| {
        shared.update(|s| s.backpressure_failures += 1);
        EncodeError::Native("AU_QUEUE_BACKPRESSURE_ABORT".into())
    })
}

/// Observe feedback while preserving the default NACK/report chain. No RTCP task is leaked.
struct Feedback {
    shared: Arc<Shared>,
    read: std::collections::VecDeque<rtc::interceptor::TaggedPacket>,
    write: std::collections::VecDeque<rtc::interceptor::TaggedPacket>,
}
impl Feedback {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            read: Default::default(),
            write: Default::default(),
        }
    }
}
impl rtc::sansio::Protocol<rtc::interceptor::TaggedPacket, rtc::interceptor::TaggedPacket, ()>
    for Feedback
{
    type Rout = rtc::interceptor::TaggedPacket;
    type Wout = rtc::interceptor::TaggedPacket;
    type Eout = ();
    type Error = rtc::shared::error::Error;
    type Time = Instant;
    fn handle_read(&mut self, msg: rtc::interceptor::TaggedPacket) -> Result<(), Self::Error> {
        use rtc::rtcp::payload_feedbacks::{
            full_intra_request::FullIntraRequest, picture_loss_indication::PictureLossIndication,
        };
        if let rtc::interceptor::Packet::Rtcp(packets) = &msg.message.packet {
            for packet in packets {
                let pli = packet.as_any().is::<PictureLossIndication>();
                let fir = packet.as_any().is::<FullIntraRequest>();
                self.shared.update(|s| {
                    s.rtcp_packets_received += 1;
                    s.pli_requests += u32::from(pli);
                    s.fir_requests += u32::from(fir);
                });
                if pli || fir {
                    self.shared.keyframe.store(true, Ordering::Release);
                }
            }
        }
        self.read.push_back(msg);
        Ok(())
    }
    fn poll_read(&mut self) -> Option<Self::Rout> {
        self.read.pop_front()
    }
    fn handle_write(&mut self, msg: rtc::interceptor::TaggedPacket) -> Result<(), Self::Error> {
        self.write.push_back(msg);
        Ok(())
    }
    fn poll_write(&mut self) -> Option<Self::Wout> {
        self.write.pop_front()
    }
}
impl rtc::interceptor::Interceptor for Feedback {
    fn bind_local_stream(&mut self, _: &rtc::interceptor::StreamInfo) {}
    fn unbind_local_stream(&mut self, _: &rtc::interceptor::StreamInfo) {}
    fn bind_remote_stream(&mut self, _: &rtc::interceptor::StreamInfo) {}
    fn unbind_remote_stream(&mut self, _: &rtc::interceptor::StreamInfo) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn data_pair() -> (
        Arc<Session>,
        Arc<dyn PeerConnection>,
        Arc<dyn PeerConnection>,
        Arc<dyn webrtc::data_channel::DataChannel>,
        JoinHandle<()>,
        String,
    ) {
        use webrtc::data_channel::RTCDataChannelInit;
        use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};
        let id = uuid::Uuid::new_v4().to_string();
        let scope = crate::scope::LanScope {
            local: "127.0.0.1".parse().unwrap(),
            peer: "127.0.0.1".parse().unwrap(),
        };
        let host = Session::new_lan_control(30, scope, id.clone()).unwrap();
        let a = host.build_pc(102).await.unwrap();
        let browser_fixture = Session::new(30).unwrap();
        let b = browser_fixture.build_pc(102).await.unwrap();
        b.add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        a.add_track(Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            "control-test".into(),
            "fixture".into(),
            "no capture".into(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(SSRC),
                    ..Default::default()
                },
                codec: codec(),
                active: true,
                max_bitrate: 4_000_000,
                max_framerate: Some(30.0),
                scale_resolution_down_by: None,
            }],
        ))))
        .await
        .unwrap();
        let dc = b
            .create_data_channel(
                crate::control::LABEL,
                Some(RTCDataChannelInit {
                    protocol: crate::control::LABEL.into(),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let endpoint = host.control.as_ref().unwrap().clone();
        let task = tokio::spawn(async move {
            endpoint.run().await;
        });
        b.set_local_description(b.create_offer(None).await.unwrap())
            .await
            .unwrap();
        timeout(Duration::from_secs(4), async {
            while !browser_fixture
                .shared
                .gathering_done
                .load(Ordering::Acquire)
            {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        // Match the browser's existing candidate filter; the Rust fixture also emits RTCP.
        let offer = b
            .local_description()
            .await
            .unwrap()
            .sdp
            .lines()
            .filter(|line| {
                !line.starts_with("a=candidate:") || line.split_whitespace().nth(1) == Some("1")
            })
            .collect::<Vec<_>>()
            .join("\r\n")
            + "\r\n";
        crate::signaling::validate_control_description(&offer, host.network, false).unwrap();
        assert!(validate_offer(&offer).is_err());
        a.set_remote_description(RTCSessionDescription::offer(offer).unwrap())
            .await
            .unwrap();
        a.set_local_description(a.create_answer(None).await.unwrap())
            .await
            .unwrap();
        timeout(Duration::from_secs(4), async {
            while !host.shared.gathering_done.load(Ordering::Acquire) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let answer = a.local_description().await.unwrap().sdp;
        crate::signaling::validate_control_description(&answer, host.network, true).unwrap();
        b.set_remote_description(RTCSessionDescription::answer(answer).unwrap())
            .await
            .unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                if matches!(
                    dc.poll().await,
                    Some(webrtc::data_channel::DataChannelEvent::OnOpen)
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        (host, a, b, dc, task, id)
    }
    #[tokio::test]
    async fn actual_sctp_hundred_events_ping_and_protocol_rejections_without_screen_capture() {
        use crate::control::Message;
        use webrtc::data_channel::DataChannelEvent;
        let (host, a, b, dc, task, id) = data_pair().await;
        for seq in 1..=100 {
            let text = serde_json::to_string(&Message::Move {
                v: 1,
                seq,
                session_id: id.clone(),
                x_norm: (seq % 100) as f64 / 100.0,
                y_norm: 0.5,
            })
            .unwrap();
            dc.send_text(&text).await.unwrap();
            let message = timeout(Duration::from_secs(2), async {
                loop {
                    match dc.poll().await {
                        Some(DataChannelEvent::OnMessage(m)) => break m,
                        Some(
                            DataChannelEvent::OnBufferedAmountLow
                            | DataChannelEvent::OnBufferedAmountHigh,
                        ) => (),
                        event => panic!("unexpected SCTP event {event:?}"),
                    }
                }
            })
            .await
            .unwrap();
            assert!(
                matches!(serde_json::from_slice::<Message>(&message.data).unwrap(),Message::Ack{seq:reply,ack_seq,..}if reply==seq&&ack_seq==seq)
            );
            sleep(Duration::from_millis(20)).await;
        }
        dc.send_text(
            &serde_json::to_string(&Message::Ping {
                v: 1,
                seq: 101,
                session_id: id.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
        let pong = timeout(Duration::from_secs(2), async {
            loop {
                match dc.poll().await {
                    Some(DataChannelEvent::OnMessage(m)) => break m,
                    Some(
                        DataChannelEvent::OnBufferedAmountLow
                        | DataChannelEvent::OnBufferedAmountHigh,
                    ) => (),
                    event => panic!("unexpected SCTP event {event:?}"),
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            serde_json::from_slice::<Message>(&pong.data).unwrap(),
            Message::Pong { reply_to: 101, .. }
        ));
        // Replay a previously accepted sequence through real SCTP, never through IPC/TCP.
        dc.send_text(
            &serde_json::to_string(&Message::Ping {
                v: 1,
                seq: 101,
                session_id: id,
            })
            .unwrap(),
        )
        .await
        .unwrap();
        timeout(Duration::from_secs(2), async {
            while !host.shared.stop.is_stopped() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        a.close().await.unwrap();
        b.close().await.unwrap();
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        let stats = host.control.as_ref().unwrap().stats();
        assert_eq!(stats.accepted_moves, 100);
        assert_eq!(stats.open_events, 1);
        assert_eq!(stats.acks_sent, 100);
        assert_eq!(stats.pongs_sent, 1);
        assert_eq!(stats.rejected, 1);
        assert_eq!(stats.last_error.as_deref(), Some("CONTROL_SEQUENCE"));
        assert_eq!(host.stats().stream.frames_input, 0);
    }
    #[tokio::test]
    async fn actual_sctp_rejects_wrong_session_oversized_and_malformed_messages() {
        for case in 0..3 {
            let (host, a, b, dc, task, id) = data_pair().await;
            let (text, error) = match case {
                0 => (
                    serde_json::to_string(&crate::control::Message::Ping {
                        v: 1,
                        seq: 1,
                        session_id: uuid::Uuid::new_v4().to_string(),
                    })
                    .unwrap(),
                    "CONTROL_SESSION_MISMATCH",
                ),
                1 => ("x".repeat(1025), "CONTROL_MESSAGE_SIZE"),
                _ => (
                    format!("{{\"kind\":\"arbitrary_input\",\"session_id\":\"{id}\"}}"),
                    "CONTROL_SCHEMA",
                ),
            };
            dc.send_text(&text).await.unwrap();
            timeout(Duration::from_secs(2), async {
                while !host.shared.stop.is_stopped() {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            a.close().await.unwrap();
            b.close().await.unwrap();
            timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap();
            let stats = host.control.as_ref().unwrap().stats();
            assert_eq!(stats.accepted_moves, 0);
            assert_eq!(stats.acks_sent, 0);
            assert_eq!(stats.last_error.as_deref(), Some(error));
            assert_eq!(host.stats().stream.frames_input, 0);
        }
    }
    #[tokio::test]
    async fn actual_sctp_rate_limit_and_closed_session_cannot_continue() {
        let (host, a, b, dc, task, id) = data_pair().await;
        for seq in 1..=61 {
            let data = serde_json::to_string(&crate::control::Message::Ping {
                v: 1,
                seq,
                session_id: id.clone(),
            })
            .unwrap();
            let _ = dc.send_text(&data).await;
        }
        timeout(Duration::from_secs(2), async {
            while !host.shared.stop.is_stopped() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let stats = host.control.as_ref().unwrap().stats();
        assert_eq!(stats.last_error.as_deref(), Some("CONTROL_RATE_LIMIT"));
        assert_eq!(stats.pings_received, 60);
        assert_eq!(stats.rejected, 1);
        a.close().await.unwrap();
        b.close().await.unwrap();
        timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert!(dc.send_text("no authorization after close").await.is_err());
        assert_eq!(host.stats().stream.frames_input, 0);
    }
    #[tokio::test]
    async fn stop_without_signal_is_idempotent_and_invalid_offer_creates_no_media() {
        let s = Session::new(10).unwrap();
        assert!(s
            .signal("m=application 9 UDP/DTLS/SCTP webrtc-datachannel".into())
            .await
            .is_err());
        assert_eq!(s.stats().stream.frames_encoded, 0);
        s.stop().await.unwrap();
        s.stop().await.unwrap();
        assert!(s.stats().resources_released);
        assert!(Session::new(31).is_err());
    }
    #[tokio::test]
    async fn real_loopback_negotiation_timeout_closes_driver_without_starting_capture() {
        use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};
        let receiver = Session::new(5).unwrap();
        let pc = receiver.build_pc(102).await.unwrap();
        pc.add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        let offer = pc.create_offer(None).await.unwrap();
        pc.set_local_description(offer.clone()).await.unwrap();
        let native = Session::new(5).unwrap();
        let answer = native.signal(offer.sdp).await.unwrap();
        assert!(answer.contains("127.0.0.1"));
        // Deliberately never apply the answer at the other peer: real ICE times out.
        timeout(Duration::from_secs(12), async {
            while !native.stats().resources_released {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let stats = native.stats();
        assert_eq!(stats.state, "failed");
        assert_eq!(stats.stream.frames_encoded, 0);
        assert_eq!(stats.rtp_packets_written, 0);
        assert_eq!(stats.error.as_deref(), Some("ICE_CONNECT_TIMEOUT"));
        native.stop().await.unwrap();
        pc.close().await.unwrap();
        // Apply the LAN answer policy to a real library-generated answer, using loopback
        // only as the no-capture transport fixture. No SDP/ICE credentials are printed.
        let scope = crate::scope::NetworkScope::Lan(crate::scope::LanScope {
            local: "127.0.0.2".parse().unwrap(),
            peer: "127.0.0.1".parse().unwrap(),
        });
        let checked = crate::signaling::validate_description(&answer, scope, true);
        assert!(
            checked.is_ok(),
            "real answer rejected: {:?}; candidate components {:?}",
            checked.err(),
            answer
                .lines()
                .filter_map(|line| line.strip_prefix("a=candidate:"))
                .map(|line| line.split_whitespace().nth(1).unwrap_or("?"))
                .collect::<Vec<_>>()
        );
    }
    #[tokio::test]
    async fn fourth_au_backpressure_is_an_explicit_failure() {
        let session = Session::new(5).unwrap();
        let (tx, _rx) = mpsc::channel(3);
        for index in 0..4 {
            let frame = hub_encode_win::h264::EncodedVideoFrame {
                annex_b: vec![0, 0, 0, 1, 0x61],
                pts_100ns: index * 333333,
                duration_100ns: 333333,
                nal_types: vec![1],
                is_idr: false,
                encode_elapsed_ms: 0.0,
            };
            assert_eq!(enqueue(&tx, &session.shared, frame).is_ok(), index < 3);
        }
        assert_eq!(session.stats().backpressure_failures, 1);
        session.stop().await.unwrap();
    }
}

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
#[cfg(feature = "p006-direct")]
#[path = "session_direct.rs"]
mod direct;
#[cfg(feature = "p006-direct")]
pub use direct::DirectSettings;

#[derive(Clone, Default, Serialize)]
pub struct Stats {
    #[cfg(feature = "p3-lan")]
    pub input_pair_approved: Option<bool>,
    #[cfg(feature = "p3-lan")]
    pub input_pair_diagnostic: Option<crate::input_pair::PairDiagnostic>,
    #[cfg(feature = "p006-direct")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_pair_diagnostic: Option<crate::direct_diagnostics::PairDiagnostic>,
    pub ice_transport_policy: String,
    #[cfg(feature = "p006-session-probe")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_transport: Option<DiagnosticTransport>,
    #[cfg(feature = "p006-direct")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_path: Option<crate::direct_policy::PathSummary>,
    pub source_kind: String,
    #[cfg(feature = "p006-relay")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_selected_pair: Option<crate::turn_probe::PairSummary>,
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
#[cfg(feature = "p006-session-probe")]
#[derive(Clone, Serialize)]
pub struct DiagnosticTransport {
    pub drop_direct: bool,
    pub dropped_packets: u64,
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
    #[cfg(feature = "p006-direct")]
    direct: Option<DirectSettings>,
    #[cfg(feature = "p006-direct")]
    direct_boundary: Option<Arc<crate::direct_runtime::Boundary>>,
    #[cfg(feature = "p006-direct")]
    direct_local: Mutex<Option<crate::direct_policy::VerifiedCandidates>>,
    #[cfg(feature = "p006-direct")]
    direct_remote: Mutex<Option<crate::direct_policy::VerifiedCandidates>>,
    #[cfg(feature = "p006-session-probe")]
    synthetic: bool,
    #[cfg(feature = "p006-session-probe")]
    redact_direct_sdp: bool,
    #[cfg(feature = "p006-session-probe")]
    synthetic_relay_pair: Mutex<
        Option<(
            crate::relay_diagnostic::Endpoints,
            crate::relay_diagnostic::Endpoints,
        )>,
    >,
    #[cfg(feature = "p006-relay")]
    relay: Option<RelaySettings>,
    #[cfg(feature = "p006-relay")]
    relay_stopped: Arc<AtomicBool>,
    control: Option<Arc<crate::control_channel::Endpoint>>,
    network: crate::scope::NetworkScope,
    socket_counters: Arc<crate::pinned_runtime::SocketCounters>,
    seconds: u32,
    shared: Arc<Shared>,
    started: AtomicBool,
    task: Mutex<Option<JoinHandle<()>>>,
    stop_lock: tokio::sync::Mutex<()>,
}
#[cfg(feature = "p006-relay")]
pub struct RelaySettings {
    pub grant: Arc<crate::ice_policy::RelayGrant>,
    pub config: crate::ice_policy::IceConfig,
}
impl Session {
    #[cfg(feature = "p006-relay")]
    pub fn new_relay(
        seconds: u32,
        relay: RelaySettings,
        control_session: String,
    ) -> Result<Arc<Self>, String> {
        relay.config.validate(&relay.grant, hub_probe_wire::now())?;
        let mut session = Self::configured(
            seconds,
            crate::scope::NetworkScope::Loopback,
            Some(control_session.clone()),
        )?;
        let inner = Arc::get_mut(&mut session).ok_or("SESSION_ALREADY_SHARED")?;
        // Legacy network is never consulted when this explicit isolated policy is present.
        inner.control = Some(crate::control_channel::Endpoint::new_guarded(
            control_session,
            inner.shared.stop.clone(),
            seconds + 30,
        )?);
        inner.relay = Some(relay);
        Ok(session)
    }
    #[cfg(feature = "p006-session-probe")]
    pub(crate) fn new_relay_synthetic(
        seconds: u32,
        relay: RelaySettings,
        control_session: String,
    ) -> Result<Arc<Self>, String> {
        let mut session = Self::new_relay(seconds, relay, control_session)?;
        let inner = Arc::get_mut(&mut session).ok_or("SESSION_ALREADY_SHARED")?;
        inner.synthetic = true;
        inner.shared.update(|s| s.source_kind = "synthetic_nv12".into());
        Ok(session)
    }
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
    #[cfg(feature = "p3-lan")]
    pub fn new_lan_input(
        seconds: u32,
        scope: crate::scope::LanScope,
        id: String,
    ) -> Result<Arc<Self>, String> {
        let mut this = Self::configured(
            seconds,
            crate::scope::NetworkScope::Lan(scope),
            Some(id.clone()),
        )?;
        let inner = Arc::get_mut(&mut this).ok_or("SESSION_ALREADY_SHARED")?;
        inner.control = Some(crate::control_channel::Endpoint::new_input(
            id,
            inner.shared.stop.clone(),
            seconds,
        )?);
        Ok(this)
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
            #[cfg(feature = "p006-direct")]
            direct: None,
            #[cfg(feature = "p006-direct")]
            direct_boundary: None,
            #[cfg(feature = "p006-direct")]
            direct_local: Mutex::new(None),
            #[cfg(feature = "p006-direct")]
            direct_remote: Mutex::new(None),
            #[cfg(feature = "p006-session-probe")]
            synthetic: false,
            #[cfg(feature = "p006-session-probe")]
            redact_direct_sdp: false,
            #[cfg(feature = "p006-session-probe")]
            synthetic_relay_pair: Mutex::new(None),
            #[cfg(feature = "p006-relay")]
            relay: None,
            #[cfg(feature = "p006-relay")]
            relay_stopped: Arc::new(AtomicBool::new(false)),
            control,
            network,
            socket_counters: Arc::new(Default::default()),
            seconds,
            shared: Arc::new(Shared {
                stats: Mutex::new(Stats {
                    state: "starting".into(),
                    source_kind: "desktop".into(),
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
        #[cfg(feature = "p006-direct")]
        if let Some(b) = &self.direct_boundary {
            stats.rejected_udp_rx = b.counters.rejected_rx.load(Ordering::Relaxed);
            stats.rejected_udp_tx = b.counters.rejected_tx.load(Ordering::Relaxed);
            #[cfg(feature = "p006-session-probe")]
            if let Some(f) = &mut stats.diagnostic_transport {
                f.dropped_packets = b.counters.fault_dropped.load(Ordering::Relaxed);
            }
        }
        stats
    }
    pub fn cancel(&self) {
        self.shared.stop.stop();
        #[cfg(feature = "p3-lan")]
        if let Some(c) = &self.control {
            c.revoke_input();
        }
    }

    pub async fn signal(self: &Arc<Self>, offer: String) -> Result<String, String> {
        // Validate before starting any sockets or media. Only the native window owner calls this.
        #[cfg(feature = "p006-direct")]
        let direct_negotiation = self.accept_direct_offer(&offer)?;
        #[cfg(not(feature = "p006-direct"))]
        let direct_negotiation: Option<Negotiation> = None;
        #[cfg(feature = "p006-relay")]
        let relay_negotiation = self
            .relay
            .as_ref()
            .map(|r| {
                crate::signaling::validate_relay_description(
                    &offer,
                    &r.grant,
                    hub_probe_wire::now(),
                    false,
                )
            })
            .transpose()?;
        #[cfg(not(feature = "p006-relay"))]
        let relay_negotiation: Option<Negotiation> = None;
        let negotiation = if let Some(value) = direct_negotiation.or(relay_negotiation) {
            value
        } else if self.control.is_some() {
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
        timeout(
            Duration::from_secs(if self.is_direct() { 15 } else { 12 }),
            rx,
        )
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
                Duration::from_secs(self.seconds as u64 + if self.is_direct() { 60 } else { 15 }),
                self.media(&pc, offer, &negotiation, &mut answer_tx, &mut producer),
            )
            .await
            .unwrap_or_else(|_| Err("SESSION_DEADLINE".into()));
            self.shared.stop.stop();
            #[cfg(feature = "p3-lan")]
            if let Some(c) = &self.control {
                c.revoke_input();
            }
            // Let DCEP/SCTP close finish while ICE/DTLS are still alive. Closing
            // the PC first turns a normal source stop into a browser channel error.
            let control_ok = if let Some(mut task) = control_task {
                match timeout(
                    Duration::from_secs(if self.is_direct() { 5 } else { 3 }),
                    &mut task,
                )
                .await
                {
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
            let close = timeout(Duration::from_secs(2), pc.close()).await;
            let close_ok = matches!(close, Ok(Ok(())));
            #[cfg(feature = "p006-relay")]
            self.relay_stopped.store(true, Ordering::Release);
            #[cfg(feature = "p006-direct")]
            if let Some(b) = &self.direct_boundary {
                b.stop();
            }
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
                s.resources_released = close_ok
                    && join_ok
                    && control_ok
                    && self.control.as_ref().is_none_or(|c| c.cleanup_complete());
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
        #[cfg(feature = "p006-relay")]
        let is_relay = self.relay.is_some();
        #[cfg(not(feature = "p006-relay"))]
        let is_relay = false;
        let settings = SettingEngineBuilder::new()
            .with_include_loopback_candidate(
                !is_relay && !self.is_direct() && !self.network.is_lan(),
            )
            .with_network_types(vec![NetworkType::Udp4])
            .with_multicast_dns_mode(MulticastDnsMode::Disabled)
            .with_ice_timeouts(
                Some(Duration::from_secs(3)),
                Some(Duration::from_secs(6)),
                Some(Duration::from_secs(1)),
            );
        let settings=if self.control.is_some(){settings.with_sctp_max_message_size(rtc::peer_connection::configuration::setting_engine::SctpMaxMessageSize::Bounded(16*1024)).with_sctp_max_receive_buffer_size(64*1024)}else{settings}.build();
        let configuration = RTCConfigurationBuilder::default()
            .with_ice_servers(vec![])
            .build();
        let local = self.network.local();
        #[cfg(feature = "p006-relay")]
        let (configuration, local) = if let Some(r) = &self.relay {
            (
                r.config
                    .native_configuration(&r.grant, hub_probe_wire::now())?,
                r.grant.target().local,
            )
        } else {
            (configuration, local)
        };
        #[cfg(feature = "p006-direct")]
        let (configuration, local) = if let Some(d) = &self.direct {
            (
                d.grant
                    .configuration(d.config.as_ref(), hub_probe_wire::now())?,
                d.grant.local(),
            )
        } else {
            (configuration, local)
        };
        self.shared.update(|s| {
            s.ice_transport_policy =
                format!("{:?}", configuration.ice_transport_policy()).to_lowercase()
        });
        let mut builder = PeerConnectionBuilder::new()
            .with_media_engine(engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(settings)
            .with_configuration(configuration)
            .with_handler(Arc::new(Handler(self.shared.clone(), self.control.clone())))
            .with_udp_addrs(vec![format!("{local}:0")]);
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
        #[cfg(feature = "p006-relay")]
        if let Some(r) = &self.relay {
            let t = r.grant.target();
            builder = builder.with_runtime(Arc::new(crate::relay_runtime::RelayRuntime {
                base: crate::pinned_runtime::PinnedRuntime {
                    inner: webrtc::runtime::default_runtime().ok_or("NO_ASYNC_RUNTIME")?,
                    local: t.local.into(),
                    peer: (*t.turn.ip()).into(),
                    counters: self.socket_counters.clone(),
                },
                endpoint: t.turn.into(),
                deadline: Instant::now()
                    + Duration::from_secs(
                        r.config.expires_at.saturating_sub(hub_probe_wire::now()),
                    ),
                stopped: self.relay_stopped.clone(),
            }));
        }
        #[cfg(feature = "p006-direct")]
        if let Some(d) = &self.direct {
            builder = builder.with_host_candidates(
                d.grant.host_candidates_allowed(d.publish_private_local),
            );
        }
        #[cfg(feature = "p006-direct")]
        if let Some(boundary) = &self.direct_boundary {
            builder = builder.with_runtime(Arc::new(crate::direct_runtime::DirectRuntime {
                inner: webrtc::runtime::default_runtime().ok_or("NO_ASYNC_RUNTIME")?,
                boundary: boundary.clone(),
            }));
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
            self.stats().source_kind,
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
        #[cfg(feature = "p3-lan")]
        let input_offer = offer.clone();
        #[cfg(feature = "p006-session-probe")]
        let relay_offer = if self.synthetic && self.relay.is_some() {
            Some(crate::relay_diagnostic::endpoints(&offer)?)
        } else {
            None
        };
        pc.set_remote_description(RTCSessionDescription::offer(offer).map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?;
        let answer = pc.create_answer(None).await.map_err(|e| e.to_string())?;
        pc.set_local_description(answer)
            .await
            .map_err(|e| e.to_string())?;
        #[cfg(feature = "p006-relay")]
        let is_relay = self.relay.is_some();
        #[cfg(not(feature = "p006-relay"))]
        let is_relay = false;
        let gather_seconds = if is_relay || self.is_direct() { 10 } else { 4 };
        let gather = timeout(Duration::from_secs(gather_seconds), async {
            while !self.shared.gathering_done.load(Ordering::Acquire)
                && !self.shared.stop.is_stopped()
            {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if gather.is_err() && !self.is_direct() {
            return Err("ICE_GATHER_TIMEOUT".into());
        }
        if self.shared.stop.is_stopped() {
            return Ok(());
        }
        let answer = pc
            .local_description()
            .await
            .ok_or("NO_LOCAL_DESCRIPTION")?
            .sdp;
        #[cfg(feature = "p006-direct")]
        let answer = self.direct_answer(answer)?;
        #[cfg(feature = "p006-session-probe")]
        let answer = if let Some(remote) = relay_offer {
            let safe = crate::relay_diagnostic::redact(&answer)?;
            let local = crate::relay_diagnostic::endpoints(&safe)?;
            *self
                .synthetic_relay_pair
                .lock()
                .map_err(|_| "R06_RELAY_PAIR_LOCK")? = Some((local, remote));
            safe
        } else {
            answer
        };
        let candidates: Vec<_> = answer
            .lines()
            .filter(|l| l.starts_with("a=candidate:"))
            .collect();
        #[cfg(feature = "p006-relay")]
        let relay_answer = if let Some(r) = &self.relay {
            crate::signaling::validate_relay_description(
                &answer,
                &r.grant,
                hub_probe_wire::now(),
                true,
            )?;
            true
        } else {
            false
        };
        #[cfg(not(feature = "p006-relay"))]
        let relay_answer = false;
        if !relay_answer
            && !self.is_direct()
            && (candidates.is_empty()
                || candidates.iter().any(|c| {
                    !c.contains(&format!(" {} ", self.network.local())) || !c.contains(" typ host")
                }))
        {
            return Err("NATIVE_ICE_NOT_LOOPBACK".into());
        }
        self.shared.update(|s| {
            s.native_candidates = candidates
                .iter()
                .map(|_| {
                    if relay_answer {
                        "relay/approved-range/udp"
                    } else if self.network.is_lan() {
                        "host/pinned-lan/udp"
                    } else {
                        "host/127.0.0.1/udp"
                    }
                    .into()
                })
                .collect()
        });
        #[cfg(feature = "p006-direct")]
        self.publish_direct_candidate_stats();
        #[cfg(feature = "p3-lan")]
        let input_answer = answer.clone();
        if let Some(tx) = answer_tx.take() {
            tx.send(Ok(answer)).map_err(|_| "UI_SIGNAL_CANCELLED")?;
        }
        timeout(
            Duration::from_secs(if self.is_direct() { 40 } else { 8 }),
            async {
                while !self.shared.connected.load(Ordering::Acquire)
                    && !self.shared.stop.is_stopped()
                {
                    sleep(Duration::from_millis(20)).await;
                }
            },
        )
        .await
        .map_err(|_| "ICE_CONNECT_TIMEOUT")?;
        if self.shared.stop.is_stopped() {
            return Ok(());
        }
        #[cfg(feature = "p006-relay")]
        if self.relay.is_some() {
            self.verify_relay_pair(pc).await?;
        }
        #[cfg(feature = "p006-direct")]
        if self.is_direct() {
            self.verify_direct_pair(pc).await?;
        }
        #[cfg(feature = "p3-lan")]
        if let Some(c) = &self.control {
            if c.is_input() {
                let sctp = pc.sctp().await.ok_or("INPUT_SCTP_MISSING")?;
                let pair = sctp
                    .transport()
                    .ice_transport()
                    .get_selected_candidate_pair()
                    .await
                    .map_err(|_| "INPUT_PAIR_READ_FAILED")?
                    .ok_or("INPUT_PAIR_MISSING")?;
                let crate::scope::NetworkScope::Lan(scope) = self.network else {
                    return Err("INPUT_LAN_SCOPE_REQUIRED".into());
                };
                let diagnostic =
                    |sdp: &str, c: &webrtc::peer_connection::RTCIceCandidate, expected| {
                        crate::input_pair::diagnose(
                            sdp,
                            &c.address,
                            c.port,
                            &c.protocol.to_string(),
                            &c.typ.to_string(),
                            expected,
                        )
                    };
                let diagnostic = crate::input_pair::PairDiagnostic {
                    local: diagnostic(&input_answer, pair.local(), scope.local),
                    remote: diagnostic(&input_offer, pair.remote(), scope.peer),
                };
                let pair_ok = diagnostic.local.approved && diagnostic.remote.approved;
                self.shared.update(|s| {
                    s.input_pair_approved = Some(pair_ok);
                    s.input_pair_diagnostic = Some(diagnostic);
                });
                if !pair_ok {
                    return Err("INPUT_SELECTED_PAIR_NOT_APPROVED".into());
                }
                c.approve_verified_path();
            }
        }
        let (tx, mut rx) = mpsc::channel(3);
        let shared = self.shared.clone();
        let seconds = self.seconds;
        #[cfg(feature = "p006-session-probe")]
        let synthetic = self.synthetic;
        *producer = Some(
            std::thread::Builder::new()
                .name("remotehub-dxgi-mf".into())
                .spawn(move || {
                    #[cfg(feature = "p006-session-probe")]
                    let run = if synthetic {
                        hub_encode_win::stream::run_synthetic
                    } else {
                        hub_encode_win::stream::run
                    };
                    #[cfg(not(feature = "p006-session-probe"))]
                    let run = hub_encode_win::stream::run;
                    let result = run(
                        true,
                        EncoderConfig {
                            seconds,
                            ..Default::default()
                        },
                        shared.stop.clone(),
                        &shared.keyframe,
                        |report: &StreamReport| shared.update(|s| s.stream = report.clone()),
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
            #[cfg(feature = "p3-lan")]
            let lan_input_draining =
                if let Some(input) = self.control.as_ref().filter(|control| control.is_input()) {
                    // The last AU was handed off. Keep SCTP alive for the authenticated
                    // finishing notification, but never accept new keyboard/mouse input.
                    input.begin_input_drain();
                    true
                } else {
                    false
                };
            #[cfg(not(feature = "p3-lan"))]
            let lan_input_draining = false;
            if self.is_direct() || lan_input_draining {
                self.shared.update(|s| s.state = "draining".into());
            }
            sleep(Duration::from_millis(if self.is_direct() {
                2000
            } else {
                700
            }))
            .await;
        }
        self.update_stats(pc).await;
        Ok(())
    }

    async fn update_stats(&self, pc: &Arc<dyn PeerConnection>) {
        #[cfg(feature = "p006-direct")]
        if self.is_direct() && !self.shared.stop.is_stopped() {
            if let Err(e) = self.verify_direct_pair(pc).await {
                self.shared.update(|s| s.error = Some(e));
                self.shared.stop.stop();
            }
        }
        #[cfg(feature = "p006-relay")]
        if self.relay.is_some() {
            if let Err(error) = self.verify_relay_pair(pc).await {
                self.shared.update(|s| s.error = Some(error));
                self.shared.stop.stop();
            }
        }
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
    #[cfg(feature = "p006-relay")]
    async fn verify_relay_pair(&self, pc: &Arc<dyn PeerConnection>) -> Result<(), String> {
        let r = self.relay.as_ref().ok_or("RELAY_POLICY_MISSING")?;
        #[cfg(feature = "p006-session-probe")]
        if self.synthetic {
            self.shared.update(|s| s.native_selected_pair = None);
        }
        let sctp = pc.sctp().await.ok_or("RELAY_SCTP_MISSING")?;
        let pair = sctp
            .transport()
            .ice_transport()
            .get_selected_candidate_pair()
            .await
            .map_err(|_| "RELAY_PAIR_READ_FAILED")?
            .ok_or("RELAY_PAIR_NOT_SELECTED")?;
        let permitted = |c: &webrtc::peer_connection::RTCIceCandidate| {
            c.typ.to_string() == "relay"
                && c.protocol.to_string() == "udp"
                && c.address
                    .parse()
                    .is_ok_and(|ip| r.grant.permits_candidate(ip, c.port))
        };
        let valid = permitted(pair.local()) && permitted(pair.remote());
        #[cfg(feature = "p006-session-probe")]
        let valid = valid
            && (!self.synthetic
                || self
                    .synthetic_relay_pair
                    .lock()
                    .map_err(|_| "R06_RELAY_PAIR_LOCK")?
                    .as_ref()
                    .is_some_and(|(local, remote)| {
                        crate::relay_diagnostic::matches(local, pair.local())
                            && crate::relay_diagnostic::matches(remote, pair.remote())
                    }));
        self.shared.update(|s| {
            s.native_selected_pair = Some(crate::turn_probe::PairSummary {
                local_type: pair.local().typ.to_string(),
                remote_type: pair.remote().typ.to_string(),
                protocol: pair.local().protocol.to_string(),
                matches_relay_policy: valid,
            })
        });
        if !valid {
            return Err("NATIVE_SELECTED_NON_RELAY".into());
        }
        if let Some(control) = &self.control {
            control.approve_verified_path();
        }
        Ok(())
    }
    fn is_direct(&self) -> bool {
        #[cfg(feature = "p006-direct")]
        {
            self.direct.is_some()
        }
        #[cfg(not(feature = "p006-direct"))]
        {
            false
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
    async fn data_pair(
        input: bool,
    ) -> (
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
        #[cfg(feature = "p3-lan")]
        let host = if input {
            Session::new_lan_input(30, scope, id.clone()).unwrap()
        } else {
            Session::new_lan_control(30, scope, id.clone()).unwrap()
        };
        #[cfg(not(feature = "p3-lan"))]
        let host = {
            let _ = input;
            Session::new_lan_control(30, scope, id.clone()).unwrap()
        };
        #[cfg(feature = "p3-lan")]
        let label = if input {
            crate::remote_input::LABEL
        } else {
            crate::control::LABEL
        };
        #[cfg(not(feature = "p3-lan"))]
        let label = crate::control::LABEL;
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
                label,
                Some(RTCDataChannelInit {
                    protocol: label.into(),
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
    #[cfg(feature = "p3-lan")]
    #[tokio::test]
    async fn actual_input_sctp_release_only_and_revocation_without_any_input_injection() {
        use webrtc::data_channel::DataChannelEvent;
        let (host, a, b, dc, task, id) = data_pair(true).await;
        let endpoint = host.control.as_ref().unwrap();
        assert!(endpoint.is_input());
        // This fixture bypasses capture and selected-pair gating only for a release_all on an
        // empty injector. No key/mouse packet is sent; SendInput is never invoked.
        endpoint.approve_verified_path();
        dc.send_text(
            &serde_json::json!({"v":1,"seq":1,"session_id":id,"event":{"kind":"release_all"}})
                .to_string(),
        )
        .await
        .unwrap();
        let response = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(DataChannelEvent::OnMessage(m)) = dc.poll().await {
                    break m;
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            serde_json::from_slice::<crate::control::Message>(&response.data).unwrap(),
            crate::control::Message::Ack { ack_seq: 1, .. }
        ));
        assert_eq!(endpoint.stats().accepted_moves, 1);
        host.cancel();
        task.await.unwrap();
        assert!(endpoint.cleanup_complete());
        a.close().await.unwrap();
        b.close().await.unwrap();
        assert_eq!(host.stats().stream.frames_input, 0);
    }
    #[tokio::test]
    async fn actual_sctp_hundred_events_ping_and_protocol_rejections_without_screen_capture() {
        use crate::control::Message;
        use webrtc::data_channel::DataChannelEvent;
        let (host, a, b, dc, task, id) = data_pair(false).await;
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
            let (host, a, b, dc, task, id) = data_pair(false).await;
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
        let (host, a, b, dc, task, id) = data_pair(false).await;
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

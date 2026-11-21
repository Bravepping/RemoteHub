//! Feature-gated no-capture, no-remote-SDP TURN gathering framework. No desktop IPC uses it.
use crate::{
    ice_policy::{IceConfig, RelayGrant},
    pinned_runtime::{PinnedRuntime, SocketCounters},
    relay_runtime::RelayRuntime,
};
use rtc::ice::{mdns::MulticastDnsMode, network_type::NetworkType};
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState,
    RTCPeerConnectionIceEvent, SettingEngineBuilder,
};
#[derive(Clone, Default, Serialize)]
pub struct Diagnostic {
    pub gather_elapsed_ms: Option<u64>,
    pub relay_candidates: u32,
    pub selected_pair: Option<PairSummary>,
    pub round_trip_time: Option<f64>,
    pub packets_lost: Option<i64>,
    pub resources_released: bool,
    pub error: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct PairSummary {
    pub local_type: String,
    pub remote_type: String,
    pub protocol: String,
    pub matches_relay_policy: bool,
}
struct Handler {
    target: crate::ice_policy::RelayTarget,
    state: Arc<Mutex<Diagnostic>>,
    complete: Arc<AtomicBool>,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let c = event.candidate;
        let ok = c.typ.to_string() == "relay"
            && c.protocol.to_string() == "udp"
            && c.address == self.target.relay.to_string()
            && (self.target.port_min..=self.target.port_max).contains(&c.port);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if ok {
            state.relay_candidates += 1;
        } else {
            state.error = Some("RELAY_GATHERED_CANDIDATE_REJECTED".into());
        }
    }
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state.to_string() == "complete" {
            self.complete.store(true, Ordering::Release);
        }
    }
}
pub struct TurnProbe {
    pc: Arc<dyn PeerConnection>,
    grant: RelayGrant,
    state: Arc<Mutex<Diagnostic>>,
    complete: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    deadline: Instant,
}
impl TurnProbe {
    /// Building alone does not gather or contact TURN. gather() is a separate explicit action.
    pub async fn new(config: &IceConfig, grant: RelayGrant, now: u64) -> Result<Self, String> {
        let native = config.native_configuration(&grant, now)?;
        let target = grant.target();
        let state = Arc::new(Mutex::new(Diagnostic::default()));
        let complete = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_secs(config.expires_at - now);
        let runtime = RelayRuntime {
            base: PinnedRuntime {
                inner: webrtc::runtime::default_runtime().ok_or("RUNTIME_UNAVAILABLE")?,
                local: target.local.into(),
                peer: (*target.turn.ip()).into(),
                counters: Arc::new(SocketCounters::default()),
            },
            endpoint: target.turn.into(),
            deadline,
            stopped: stopped.clone(),
        };
        let settings = SettingEngineBuilder::new()
            .with_network_types(vec![NetworkType::Udp4])
            .with_multicast_dns_mode(MulticastDnsMode::Disabled)
            .with_include_loopback_candidate(false)
            .build();
        let pc = PeerConnectionBuilder::new()
            .with_configuration(native)
            .with_setting_engine(settings)
            .with_runtime(Arc::new(runtime))
            .with_udp_addrs(vec![format!("{}:0", target.local)])
            .with_handler(Arc::new(Handler {
                target,
                state: state.clone(),
                complete: complete.clone(),
            }))
            .build()
            .await
            .map_err(|_| "TURN_PROBE_BUILD_FAILED")?;
        Ok(Self {
            pc: Arc::new(pc),
            grant,
            state,
            complete,
            stopped,
            deadline,
        })
    }
    pub async fn gather(self) -> Result<Diagnostic, String> {
        let started = Instant::now();
        let gathered = self.gather_once().await;
        let gather_elapsed_ms = started.elapsed().as_millis() as u64;
        let closed = self.stop().await?;
        if !closed.resources_released {
            return Err("TURN_PROBE_CLEANUP_INCOMPLETE".into());
        }
        match gathered {
            Ok(mut data) => {
                data.resources_released = true;
                data.gather_elapsed_ms = Some(gather_elapsed_ms);
                Ok(data)
            }
            Err(error) => Err(error),
        }
    }
    async fn gather_once(&self) -> Result<Diagnostic, String> {
        if self.stopped.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            return Err("TURN_PROBE_EXPIRED".into());
        }
        // Only an application m-line; no capture, video track or control events are created.
        self.pc
            .create_data_channel("allocate.probe", None)
            .await
            .map_err(|_| "TURN_PROBE_CHANNEL_FAILED")?;
        let offer = self
            .pc
            .create_offer(None)
            .await
            .map_err(|_| "TURN_PROBE_OFFER_FAILED")?;
        self.pc
            .set_local_description(offer)
            .await
            .map_err(|_| "TURN_PROBE_GATHER_FAILED")?;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.complete.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| "TURN_PROBE_GATHER_TIMEOUT")?;
        let state = self.diagnostic().await?;
        if state.error.is_some() {
            return Err("TURN_PROBE_POLICY_FAILED".into());
        }
        if state.relay_candidates == 0 {
            return Err("TURN_ALLOCATION_NOT_OBSERVED".into());
        }
        Ok(state)
    }
    pub async fn diagnostic(&self) -> Result<Diagnostic, String> {
        let mut result = self.state.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(sctp) = self.pc.sctp().await {
            let ice = sctp.transport().ice_transport();
            if let Some(pair) = ice
                .get_selected_candidate_pair()
                .await
                .map_err(|_| "ICE_DIAGNOSTIC_FAILED")?
            {
                let a = pair.local();
                let b = pair.remote();
                let permitted = |c: &webrtc::peer_connection::RTCIceCandidate| {
                    c.typ.to_string() == "relay"
                        && c.protocol.to_string() == "udp"
                        && c.address
                            .parse()
                            .is_ok_and(|ip| self.grant.permits_candidate(ip, c.port))
                };
                let matches_relay_policy = permitted(a) && permitted(b);
                result.selected_pair = Some(PairSummary {
                    local_type: a.typ.to_string(),
                    remote_type: b.typ.to_string(),
                    protocol: a.protocol.to_string(),
                    matches_relay_policy,
                });
                if !matches_relay_policy {
                    self.stopped.store(true, Ordering::Release);
                    let _ = tokio::time::timeout(Duration::from_secs(3), self.pc.close()).await;
                    self.state.lock().unwrap_or_else(|p| p.into_inner()).error =
                        Some("RELAY_SELECTED_PAIR_REJECTED".into());
                    return Err("RELAY_SELECTED_PAIR_REJECTED".into());
                }
            }
        }
        Ok(result)
    }
    pub async fn stop(self) -> Result<Diagnostic, String> {
        let close = tokio::time::timeout(Duration::from_secs(3), self.pc.close()).await;
        self.stopped.store(true, Ordering::Release);
        let clean = matches!(close, Ok(Ok(())));
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.resources_released = clean;
        if !clean {
            state.error = Some("TURN_PROBE_CLEANUP_INCOMPLETE".into());
        }
        Ok(state.clone())
    }
}
impl Drop for TurnProbe {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn real_library_build_close_without_gather_reports_no_fabricated_pair_or_rtt() {
        let (g, c) = crate::ice_policy::tests::fixture();
        let probe = TurnProbe::new(&c, g, 1000).await.unwrap();
        let d = probe.diagnostic().await.unwrap();
        assert!(d.selected_pair.is_none());
        assert!(d.round_trip_time.is_none());
        assert!(d.packets_lost.is_none());
        assert!(d.gather_elapsed_ms.is_none());
        assert_eq!(d.relay_candidates, 0);
        assert!(probe.stop().await.unwrap().resources_released);
    }
}

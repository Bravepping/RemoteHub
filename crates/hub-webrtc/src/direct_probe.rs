//! Opt-in operator-only pair of real native PCs. No screen, video or input APIs.
use crate::{
    direct_policy::{self, Candidate, DirectGrant, Kind, PathSummary, VerifiedCandidates},
    direct_runtime::{Boundary, DirectRuntime},
    ice_policy::IceConfig,
};
use rtc::ice::{mdns::MulticastDnsMode, network_type::NetworkType};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, Mutex};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState,
        RTCSessionDescription, SettingEngineBuilder,
    },
};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn stamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
        .to_string()
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Case {
    Direct,
    Fallback,
    TurnUnavailable,
    AllUnavailable,
}
#[derive(Default, Serialize)]
pub struct Counts {
    host: u32,
    srflx: u32,
    relay: u32,
}
impl Counts {
    fn from(c: &VerifiedCandidates) -> Self {
        let mut s = Self::default();
        for c in c.candidates() {
            match c.kind {
                Kind::Host => s.host += 1,
                Kind::Srflx => s.srflx += 1,
                Kind::Relay => s.relay += 1,
            }
        }
        s
    }
}
#[derive(Serialize)]
pub struct Report {
    pub case: Case,
    pub started_at_ns: String,
    pub finished_at_ns: String,
    pub elapsed_ms: u64,
    pub policy: &'static str,
    pub media_tracks: u32,
    pub physical_machines: u32,
    pub fault_drop_direct: bool,
    pub fault_drop_turn: bool,
    pub fault_dropped_packets: u64,
    pub gathered_a: Option<Counts>,
    pub gathered_b: Option<Counts>,
    pub gather_complete_a: bool,
    pub gather_complete_b: bool,
    pub selected_a: Option<PathSummary>,
    pub selected_b: Option<PathSummary>,
    pub selected_phase: Option<&'static str>,
    pub sent: u32,
    pub received: u32,
    pub acknowledged: u32,
    pub resources_released: bool,
    pub error: Option<String>,
    pub closes: Vec<CloseObservation>,
}
#[derive(Serialize)]
pub struct CloseObservation {
    endpoint: &'static str,
    call_ns: String,
    returned_ns: String,
    local_close_succeeded: bool,
}
struct Handler {
    complete: Arc<AtomicBool>,
    incoming: mpsc::Sender<Arc<dyn DataChannel>>,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, s: RTCIceGatheringState) {
        if s.to_string() == "complete" {
            self.complete.store(true, Ordering::Release);
        }
    }
    async fn on_data_channel(&self, c: Arc<dyn DataChannel>) {
        if self.incoming.try_send(c.clone()).is_err() {
            let _ = c.close().await;
        }
    }
}
struct Peer {
    pc: Arc<dyn PeerConnection>,
    complete: Arc<AtomicBool>,
    incoming: Mutex<mpsc::Receiver<Arc<dyn DataChannel>>>,
    boundary: Arc<Boundary>,
    grant: DirectGrant,
}
impl Peer {
    async fn new(
        grant: DirectGrant,
        config: Option<&IceConfig>,
        case: Case,
    ) -> Result<Self, String> {
        let native = grant.configuration(config, now())?;
        let complete = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel(1);
        let boundary = Boundary::diagnostic(
            &grant,
            now(),
            matches!(case, Case::Fallback | Case::AllUnavailable),
            matches!(case, Case::TurnUnavailable | Case::AllUnavailable),
        )?;
        let settings = SettingEngineBuilder::new()
            .with_network_types(vec![NetworkType::Udp4])
            .with_multicast_dns_mode(MulticastDnsMode::Disabled)
            .with_include_loopback_candidate(cfg!(test) && grant.local().is_loopback())
            .with_sctp_max_message_size(
                rtc::peer_connection::configuration::setting_engine::SctpMaxMessageSize::Bounded(
                    16384,
                ),
            )
            .with_sctp_max_receive_buffer_size(65536)
            .build();
        let pc = PeerConnectionBuilder::new()
            .with_configuration(native)
            .with_setting_engine(settings)
            .with_udp_addrs(vec![format!("{}:0", grant.local())])
            .with_runtime(Arc::new(DirectRuntime {
                inner: webrtc::runtime::default_runtime().ok_or("DIRECT_RUNTIME_UNAVAILABLE")?,
                boundary: boundary.clone(),
            }))
            .with_handler(Arc::new(Handler {
                complete: complete.clone(),
                incoming: tx,
            }))
            .build()
            .await
            .map_err(|_| "DIRECT_PC_BUILD")?;
        Ok(Self {
            pc: Arc::new(pc),
            complete,
            incoming: Mutex::new(rx),
            boundary,
            grant,
        })
    }
    async fn gathered(&self) -> Result<String, String> {
        // An unavailable TURN server must not prevent a viable host pair from being tried.
        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            while !self.complete.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let sdp = self
            .pc
            .local_description()
            .await
            .ok_or("DIRECT_LOCAL_SDP_MISSING")?
            .sdp;
        Ok(sdp
            .lines()
            // rtc emits an extra RTCP component even for an application m-line.
            // Signal only RTP/ICE component 1, matching the existing browser fixture.
            .filter(|l| !l.starts_with("a=candidate:") || l.split_whitespace().nth(1) == Some("1"))
            .map(|l| {
                if l.starts_with("c=") {
                    "c=IN IP4 0.0.0.0"
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\r\n")
            + "\r\n")
    }
    async fn pair(
        &self,
        local: &VerifiedCandidates,
        remote: &VerifiedCandidates,
    ) -> Result<PathSummary, String> {
        if local.scope != self.grant.scope() || remote.scope != self.grant.scope() {
            return Err("DIRECT_SELECTED_PAIR_SCOPE_MISMATCH".into());
        }
        let sctp = self.pc.sctp().await.ok_or("DIRECT_SCTP_MISSING")?;
        let pair = sctp
            .transport()
            .ice_transport()
            .get_selected_candidate_pair()
            .await
            .map_err(|_| "DIRECT_STATS")?
            .ok_or("DIRECT_SELECTED_PAIR_MISSING")?;
        let convert = |c: &webrtc::peer_connection::RTCIceCandidate| -> Result<Candidate, String> {
            if c.protocol.to_string() != "udp" {
                return Err("DIRECT_SELECTED_PROTOCOL".into());
            }
            Candidate::parse(&format!(
                "x {} udp 1 {} {} typ {}",
                c.component, c.address, c.port, c.typ
            ))
        };
        direct_policy::selected(
            convert(pair.local())?,
            convert(pair.remote())?,
            local,
            remote,
        )
    }
    async fn close(&self) -> bool {
        let clean = matches!(
            tokio::time::timeout(Duration::from_secs(3), self.pc.close()).await,
            Ok(Ok(()))
        );
        self.boundary.stop();
        clean
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.boundary.stop();
    }
}
async fn message(dc: &Arc<dyn DataChannel>) -> Result<Vec<u8>, String> {
    loop {
        match dc.poll().await {
            Some(DataChannelEvent::OnMessage(m)) if m.is_string && m.data.len() <= 64 => {
                return Ok(m.data.to_vec());
            }
            Some(
                DataChannelEvent::OnBufferedAmountLow | DataChannelEvent::OnBufferedAmountHigh,
            ) => (),
            _ => return Err("DIRECT_CHANNEL_EVENT_OR_PAYLOAD".into()),
        }
    }
}
async fn wait_open(dc: &Arc<dyn DataChannel>) -> Result<(), String> {
    loop {
        match dc.poll().await {
            Some(DataChannelEvent::OnOpen) => return Ok(()),
            Some(DataChannelEvent::OnClose | DataChannelEvent::OnError) | None => {
                return Err("DIRECT_CHANNEL_FAILED".into());
            }
            _ => (),
        }
    }
}
struct ScopedSdp {
    sender: VerifiedCandidates,
    receiver: VerifiedCandidates,
}

fn scoped_sdp(
    sdp: &str,
    sender: &DirectGrant,
    receiver: &DirectGrant,
    now: u64,
) -> Result<ScopedSdp, String> {
    // Each peer uses its own grant for both sets, even with distinct TURN tickets.
    Ok(ScopedSdp {
        sender: direct_policy::data_sdp(sdp, sender, now)?,
        receiver: direct_policy::data_sdp(sdp, receiver, now)?,
    })
}

async fn record_selected(
    a: &Peer,
    b: &Peer,
    offer: &ScopedSdp,
    answer: &ScopedSdp,
    report: &mut Report,
    phase: &'static str,
) -> Result<(), String> {
    // If either read now fails, don't report the previous pair as current or mix snapshots.
    report.selected_phase = None;
    report.selected_a = None;
    report.selected_b = None;
    let (a_path, b_path) = tokio::try_join!(
        a.pair(&offer.sender, &answer.receiver),
        b.pair(&answer.sender, &offer.receiver),
    )?;
    let expected = match report.case {
        Case::Direct | Case::TurnUnavailable => Some("direct"),
        Case::Fallback => Some("relay"),
        Case::AllUnavailable => None,
    };
    let matched = expected.is_some_and(|path| a_path.path == path && b_path.path == path);
    report.selected_a = Some(a_path);
    report.selected_b = Some(b_path);
    report.selected_phase = Some(phase);
    if !matched {
        return Err("DIRECT_UNEXPECTED_SELECTED_PATH".into());
    }
    Ok(())
}

async fn exercise(a: &Peer, b: &Peer, report: &mut Report) -> Result<(), String> {
    let dc =
        a.pc.create_data_channel("ice.path.probe", None)
            .await
            .map_err(|_| "DIRECT_CREATE_CHANNEL")?;
    a.pc.set_local_description(a.pc.create_offer(None).await.map_err(|_| "DIRECT_OFFER")?)
        .await
        .map_err(|_| "DIRECT_SET_OFFER")?;
    let offer = a.gathered().await?;
    let offer_sets = scoped_sdp(&offer, &a.grant, &b.grant, now())?;
    report.gathered_a = Some(Counts::from(&offer_sets.sender));
    b.boundary.install_peer(offer_sets.receiver.clone())?;
    b.pc.set_remote_description(
        RTCSessionDescription::offer(offer).map_err(|_| "DIRECT_OFFER_PARSE")?,
    )
    .await
    .map_err(|_| "DIRECT_REMOTE_OFFER")?;
    b.pc.set_local_description(
        b.pc.create_answer(None)
            .await
            .map_err(|_| "DIRECT_ANSWER")?,
    )
    .await
    .map_err(|_| "DIRECT_SET_ANSWER")?;
    let answer = b.gathered().await?;
    let answer_sets = scoped_sdp(&answer, &b.grant, &a.grant, now())?;
    report.gathered_b = Some(Counts::from(&answer_sets.sender));
    a.boundary.install_peer(answer_sets.receiver.clone())?;
    a.pc.set_remote_description(
        RTCSessionDescription::answer(answer).map_err(|_| "DIRECT_ANSWER_PARSE")?,
    )
    .await
    .map_err(|_| "DIRECT_REMOTE_ANSWER")?;
    tokio::time::timeout(Duration::from_secs(15), wait_open(&dc))
        .await
        .map_err(|_| "DIRECT_CONNECT_TIMEOUT")??;
    let incoming = tokio::time::timeout(Duration::from_secs(2), b.incoming.lock().await.recv())
        .await
        .map_err(|_| "DIRECT_REMOTE_CHANNEL_TIMEOUT")?
        .ok_or("DIRECT_REMOTE_CHANNEL_MISSING")?;
    if incoming.label().await.map_err(|_| "DIRECT_REMOTE_LABEL")? != "ice.path.probe" {
        return Err("DIRECT_REMOTE_LABEL".into());
    }
    // The receiving channel also reports OnOpen; consume it before reading messages.
    tokio::time::timeout(Duration::from_secs(2), wait_open(&incoming))
        .await
        .map_err(|_| "DIRECT_REMOTE_OPEN_TIMEOUT")??;
    record_selected(a, b, &offer_sets, &answer_sets, report, "before_traffic").await?;
    for seq in 1..=100 {
        dc.send_text(&format!("path:{seq}"))
            .await
            .map_err(|_| "DIRECT_SEND")?;
        report.sent += 1;
        let received = tokio::time::timeout(Duration::from_secs(2), message(&incoming))
            .await
            .map_err(|_| "DIRECT_RECEIVE_TIMEOUT")??;
        if received != format!("path:{seq}").as_bytes() {
            return Err("DIRECT_SEQUENCE".into());
        }
        report.received += 1;
        incoming
            .send_text(&format!("ack:{seq}"))
            .await
            .map_err(|_| "DIRECT_ACK_SEND")?;
        let ack = tokio::time::timeout(Duration::from_secs(2), message(&dc))
            .await
            .map_err(|_| "DIRECT_ACK_TIMEOUT")??;
        if ack != format!("ack:{seq}").as_bytes() {
            return Err("DIRECT_ACK_SEQUENCE".into());
        }
        report.acknowledged += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Verify the path again after actual traffic; never label a stale initial pair as current.
    record_selected(a, b, &offer_sets, &answer_sets, report, "after_traffic").await
}
pub async fn run(
    a: DirectGrant,
    config_a: Option<&IceConfig>,
    b: DirectGrant,
    config_b: Option<&IceConfig>,
    case: Case,
) -> Report {
    let start = Instant::now();
    let mut report = Report {
        case,
        started_at_ns: stamp(),
        finished_at_ns: String::new(),
        elapsed_ms: 0,
        policy: "all",
        media_tracks: 0,
        physical_machines: 1,
        fault_drop_direct: matches!(case, Case::Fallback | Case::AllUnavailable),
        fault_drop_turn: matches!(case, Case::TurnUnavailable | Case::AllUnavailable),
        fault_dropped_packets: 0,
        gathered_a: None,
        gathered_b: None,
        gather_complete_a: false,
        gather_complete_b: false,
        selected_a: None,
        selected_b: None,
        selected_phase: None,
        sent: 0,
        received: 0,
        acknowledged: 0,
        resources_released: false,
        error: None,
        closes: vec![],
    };
    let mut peers = vec![];
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        peers.push(Peer::new(a, config_a, case).await?);
        peers.push(Peer::new(b, config_b, case).await?);
        exercise(&peers[0], &peers[1], &mut report).await
    })
    .await
    .unwrap_or_else(|_| Err("DIRECT_PROBE_TIMEOUT".into()));
    for (index, p) in peers.iter().enumerate() {
        if index == 0 {
            report.gather_complete_a = p.complete.load(Ordering::Acquire);
        } else {
            report.gather_complete_b = p.complete.load(Ordering::Acquire);
        }
        report.fault_dropped_packets += p.boundary.counters.fault_dropped.load(Ordering::Relaxed);
    }
    let mut released = peers.len() == 2;
    for (index, p) in peers.iter().enumerate() {
        let call_ns = stamp();
        let clean = p.close().await;
        report.closes.push(CloseObservation {
            endpoint: if index == 0 { "a" } else { "b" },
            call_ns,
            returned_ns: stamp(),
            local_close_succeeded: clean,
        });
        released &= clean;
    }
    report.resources_released = released;
    report.error = outcome.err();
    if !released && report.error.is_none() {
        report.error = Some("DIRECT_CLEANUP_INCOMPLETE".into());
    }
    report.elapsed_ms = start.elapsed().as_millis() as u64;
    report.finished_at_ns = stamp();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offer_and_answer_are_validated_under_each_observer_scope() {
        let ip = "192.168.1.2".parse().unwrap();
        let a = DirectGrant::local_diagnostic(ip, 1300, true, 1000).unwrap();
        let b = DirectGrant::local_diagnostic(ip, 1300, true, 1000).unwrap();
        assert_ne!(a.scope(), b.scope());
        let sdp = |port| {
            format!(
                "v=0\r\na=group:BUNDLE 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:0\r\nc=IN IP4 0.0.0.0\r\na=ice-ufrag:abcd\r\na=ice-pwd:abcdefghijklmnopqrstuvwx\r\na=fingerprint:sha-256 {}\r\na=sctp-port:5000\r\na=candidate:x 1 udp 1 {ip} {port} typ host\r\n",
                ["AB"; 32].join(":")
            )
        };
        let offer = scoped_sdp(&sdp(50000), &a, &b, 1000).unwrap();
        let answer = scoped_sdp(&sdp(50001), &b, &a, 1000).unwrap();
        let a_candidate = offer.sender.candidates()[0];
        let b_candidate = answer.sender.candidates()[0];
        assert!(
            direct_policy::selected(a_candidate, b_candidate, &offer.sender, &answer.receiver)
                .is_ok()
        );
        assert!(
            direct_policy::selected(b_candidate, a_candidate, &answer.sender, &offer.receiver)
                .is_ok()
        );
        assert_eq!(
            direct_policy::selected(a_candidate, b_candidate, &offer.sender, &answer.sender)
                .err()
                .as_deref(),
            Some("DIRECT_SELECTED_PAIR_SCOPE_MISMATCH")
        );
        assert!(scoped_sdp(&sdp(50000), &a, &b, 1300).is_err());
    }

    #[tokio::test]
    async fn loopback_peers_exchange_text_and_verify_actual_selected_pair() {
        let start = now();
        let ip = std::net::Ipv4Addr::LOCALHOST;
        assert!(DirectGrant::local_diagnostic(ip, start + 120, true, start).is_err());
        let a = DirectGrant::loopback_test(start);
        let b = DirectGrant::loopback_test(start);
        assert_ne!(a.scope(), b.scope());

        let report = run(a, None, b, None, Case::Direct).await;
        assert_eq!(report.error, None);
        assert!(report.resources_released);
        assert_eq!(report.media_tracks, 0);
        assert_eq!(report.physical_machines, 1);
        assert_eq!(report.selected_phase, Some("after_traffic"));
        assert_eq!(
            (report.sent, report.received, report.acknowledged),
            (100, 100, 100)
        );
        for pair in [report.selected_a.unwrap(), report.selected_b.unwrap()] {
            assert_eq!(
                (pair.path, pair.local_type, pair.remote_type, pair.protocol),
                ("direct", Kind::Host, Kind::Host, "udp")
            );
        }
    }

    #[tokio::test]
    async fn closed_real_channel_terminates_instead_of_spinning_on_eof() {
        let (tx, _rx) = mpsc::channel(1);
        let pc = PeerConnectionBuilder::new()
            .with_udp_addrs(vec!["127.0.0.1:0".to_string()])
            .with_setting_engine(
                SettingEngineBuilder::new()
                    .with_multicast_dns_mode(MulticastDnsMode::Disabled)
                    .build(),
            )
            .with_handler(Arc::new(Handler {
                complete: Arc::new(AtomicBool::new(false)),
                incoming: tx,
            }))
            .build()
            .await
            .unwrap();
        let dc = pc.create_data_channel("eof.probe", None).await.unwrap();
        pc.close().await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), wait_open(&dc))
            .await
            .unwrap()
            .is_err());
    }
}

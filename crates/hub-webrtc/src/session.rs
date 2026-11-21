use crate::{
    sdp::{self, H264_FMTP, RTP_PAYLOAD_TYPE},
    video::Packetizer,
    MediaFrame, TransportError,
};
use rtc::{
    ice::mdns::MulticastDnsMode,
    media_stream::MediaStreamTrack,
    rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
        RtpCodecKind,
    },
    statistics::report::RTCStatsReportEntry,
};
use serde::Serialize;
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{watch, OnceCell};
use webrtc::{
    media_stream::track_local::{static_rtp::TrackLocalStaticRTP, TrackLocal},
    peer_connection::{
        register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder,
        PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceCandidate, RTCIceCandidateType,
        RTCIceGatheringState, RTCIceProtocol, RTCPeerConnectionState, RTCSessionDescription,
        SettingEngineBuilder,
    },
    rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit, RtpSender},
};

const SETUP_TIMEOUT: Duration = Duration::from_secs(6);
const WRITE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportStats {
    /// Complete access units handed to the RTP driver, not a browser receipt count.
    pub frames_sent: u64,
    /// H.264 RTP payload bytes handed to the driver (excludes headers/SRTP).
    pub bytes_sent: u64,
    /// RTP packets handed to the driver, not a wire or browser receipt count.
    pub packets_sent: u64,
    /// WebRTC driver's outbound RTP counters, not browser delivery evidence.
    pub rtp_bytes_sent: Option<u64>,
    pub rtp_packets_sent: Option<u64>,
    pub rtcp_pli_received: Option<u32>,
    pub rtcp_fir_received: Option<u32>,
    pub rtcp_nack_received: Option<u32>,
    pub rtcp_reported_packets_lost: Option<i64>,
    pub profile_level_id: Option<String>,
    pub sps_profile_level_id: Option<String>,
    pub packetization_mode: Option<u8>,
    pub payload_type: Option<u8>,
    pub candidate_pair_type: Option<&'static str>,
    pub setup_ms: Option<u64>,
}

struct Handler {
    gather: watch::Sender<RTCIceGatheringState>,
    connection: watch::Sender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        self.gather.send_replace(state);
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        self.connection.send_replace(state);
    }
}

pub struct Session {
    pc: Box<dyn PeerConnection>,
    track: Arc<TrackLocalStaticRTP>,
    sender: Arc<dyn RtpSender>,
    packetizer: Packetizer,
    gather: watch::Receiver<RTCIceGatheringState>,
    connection: watch::Receiver<RTCPeerConnectionState>,
    level: OnceCell<u8>,
    stats: TransportStats,
    started: Instant,
}

impl Session {
    pub async fn new() -> Result<Self, TransportError> {
        let started = Instant::now();
        let (gather, gather_rx) = watch::channel(RTCIceGatheringState::New);
        let (connection, connection_rx) = watch::channel(RTCPeerConnectionState::New);
        let (ssrc, sequence, timestamp) = random_rtp()?;
        let mut media_engine = MediaEngine::default();
        let codec = codec();
        media_engine.register_codec(
            RTCRtpCodecParameters {
                rtp_codec: codec.clone(),
                payload_type: RTP_PAYLOAD_TYPE,
            },
            RtpCodecKind::Video,
        )?;
        let interceptors = register_default_interceptors(Default::default(), &mut media_engine)?;
        let track = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            "p004-loopback".to_owned(),
            "p004-video".to_owned(),
            "P0-04a Video".to_owned(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(ssrc),
                    ..Default::default()
                },
                codec,
                ..Default::default()
            }],
        )));
        let pc = loopback_peer(media_engine, interceptors, Handler { gather, connection }).await?;
        let setup = async {
            let transceiver = pc
                .add_transceiver_from_track(
                    track.clone(),
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Sendonly,
                        ..Default::default()
                    }),
                )
                .await?;
            transceiver.sender().await?.ok_or(TransportError::WebRtc)
        }
        .await;
        let sender = match setup {
            Ok(sender) => sender,
            Err(err) => {
                let _ = pc.close().await;
                return Err(err);
            }
        };
        Ok(Self {
            pc: Box::new(pc),
            track,
            sender,
            packetizer: Packetizer::new(ssrc, sequence, timestamp),
            gather: gather_rx,
            connection: connection_rx,
            level: OnceCell::new(),
            stats: TransportStats::default(),
            started,
        })
    }

    /// The offer includes completed host gathering. Signaling stays in Tauri IPC.
    pub async fn create_offer(&self) -> Result<String, TransportError> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer).await?;
        wait_gathered(&self.gather).await?;
        let sdp = self
            .pc
            .local_description()
            .await
            .ok_or(TransportError::Sdp)?
            .sdp;
        let sdp = sdp::omit_redundant_rtcp_candidate(&sdp);
        sdp::validate(&sdp, "sendonly")?;
        Ok(sdp)
    }

    pub async fn accept_answer(&self, sdp: String) -> Result<(), TransportError> {
        let level = sdp::validate(&sdp, "recvonly")?;
        self.level
            .get_or_try_init(|| async {
                self.pc
                    .set_remote_description(RTCSessionDescription::answer(sdp)?)
                    .await?;
                Ok::<_, TransportError>(level)
            })
            .await?;
        Ok(())
    }

    pub async fn wait_connected(&self) -> Result<(), TransportError> {
        let mut connection = self.connection.clone();
        tokio::time::timeout(SETUP_TIMEOUT, async {
            loop {
                let state = *connection.borrow_and_update();
                match state {
                    RTCPeerConnectionState::Connected => return Ok(()),
                    RTCPeerConnectionState::Closed | RTCPeerConnectionState::Failed => {
                        return Err(TransportError::Disconnected);
                    }
                    _ => {}
                }
                connection
                    .changed()
                    .await
                    .map_err(|_| TransportError::Disconnected)?;
            }
        })
        .await
        .map_err(|_| TransportError::Timeout)??;
        self.check_selected_pair().await
    }

    pub fn is_connected(&self) -> bool {
        *self.connection.borrow() == RTCPeerConnectionState::Connected
    }

    /// Re-check the real selected pair before any media can leave the process.
    async fn check_selected_pair(&self) -> Result<(), TransportError> {
        let transport = self
            .sender
            .transport()
            .await?
            .ok_or(TransportError::IcePolicy)?;
        let pair = transport
            .ice_transport()
            .get_selected_candidate_pair()
            .await?
            .ok_or(TransportError::IcePolicy)?;
        if !safe_candidate(pair.local()) || !safe_candidate(pair.remote()) {
            return Err(TransportError::IcePolicy);
        }
        Ok(())
    }

    /// Write exactly one AU. Any error terminates the session; a partially enqueued
    /// frame may be undecodable and must not be followed silently by dependent P frames.
    pub async fn write_frame(&mut self, frame: MediaFrame) -> Result<(), TransportError> {
        if !self.is_connected() {
            return Err(TransportError::Disconnected);
        }
        let level = *self.level.get().ok_or(TransportError::Sdp)?;
        self.check_selected_pair().await?;
        if self.stats.setup_ms.is_none() {
            self.stats.setup_ms = u64::try_from(self.started.elapsed().as_millis()).ok();
        }

        let packetized = self.packetizer.packetize(&frame, RTP_PAYLOAD_TYPE, level)?;
        let [profile_idc, iop, level_idc] = packetized.sps;
        self.stats.sps_profile_level_id =
            Some(format!("{profile_idc:02x}{iop:02x}{level_idc:02x}"));
        self.stats.profile_level_id = Some(format!("4200{level:02x}"));
        self.stats.packetization_mode = Some(1);
        self.stats.payload_type = Some(RTP_PAYLOAD_TYPE);
        self.stats.candidate_pair_type = Some("host/udp/ipv4-loopback");
        tokio::time::timeout(WRITE_TIMEOUT, async {
            for packet in packetized.packets {
                let len = packet.payload.len() as u64;
                self.track.write_rtp(packet).await?;
                self.stats.bytes_sent += len;
                self.stats.packets_sent += 1;
            }
            Ok::<_, TransportError>(())
        })
        .await
        .map_err(|_| TransportError::Timeout)??;
        self.stats.frames_sent += 1;
        Ok(())
    }

    pub fn stats(&self) -> TransportStats {
        self.stats.clone()
    }

    pub async fn refresh_stats(&mut self) -> TransportStats {
        if let Ok(report) = self.sender.get_stats(Instant::now()).await {
            if let Some(outbound) = report.outbound_rtp_streams().next() {
                self.stats.rtp_packets_sent = Some(outbound.sent_rtp_stream_stats.packets_sent);
                self.stats.rtp_bytes_sent = Some(outbound.sent_rtp_stream_stats.bytes_sent);
                self.stats.rtcp_pli_received = Some(outbound.pli_count);
                self.stats.rtcp_fir_received = Some(outbound.fir_count);
                self.stats.rtcp_nack_received = Some(outbound.nack_count);
            }
            if let Some(remote) = report.iter().find_map(|entry| match entry {
                RTCStatsReportEntry::RemoteInboundRtp(remote) => Some(remote),
                _ => None,
            }) {
                self.stats.rtcp_reported_packets_lost =
                    Some(remote.received_rtp_stream_stats.packets_lost);
            }
        }
        self.stats()
    }

    pub async fn close(&self) -> Result<(), TransportError> {
        self.pc.close().await?;
        Ok(())
    }
}

fn codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: "video/H264".to_owned(),
        clock_rate: 90_000,
        sdp_fmtp_line: H264_FMTP.to_owned(),
        ..Default::default()
    }
}

async fn loopback_peer(
    media_engine: MediaEngine,
    interceptors: rtc::interceptor::Registry,
    handler: impl PeerConnectionEventHandler,
) -> Result<impl PeerConnection, TransportError> {
    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![])
        .build();
    let settings = SettingEngineBuilder::new()
        .with_include_loopback_candidate(true)
        .with_multicast_dns_mode(MulticastDnsMode::Disabled)
        .build();
    Ok(PeerConnectionBuilder::new()
        .with_configuration(config)
        .with_setting_engine(settings)
        .with_media_engine(media_engine)
        .with_interceptor_registry(interceptors)
        .with_handler(Arc::new(handler))
        // In 0.21.1 this enables the library's shutdown-on-drop path for canceled setup.
        .with_dedicated_reactor_pool_size(1)
        .with_udp_addrs(vec![SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)])
        .build()
        .await?)
}

fn safe_candidate(candidate: &RTCIceCandidate) -> bool {
    candidate.address == "127.0.0.1"
        && candidate.protocol == RTCIceProtocol::Udp
        && candidate.typ == RTCIceCandidateType::Host
        && candidate.port != 0
        && candidate.component == 1
}

async fn wait_gathered(
    receiver: &watch::Receiver<RTCIceGatheringState>,
) -> Result<(), TransportError> {
    let mut receiver = receiver.clone();
    tokio::time::timeout(
        SETUP_TIMEOUT,
        receiver.wait_for(|state| *state == RTCIceGatheringState::Complete),
    )
    .await
    .map_err(|_| TransportError::Timeout)?
    .map_err(|_| TransportError::Disconnected)?;
    Ok(())
}

fn random_rtp() -> Result<(u32, u16, u32), TransportError> {
    let mut timestamp = [0_u8; 4];
    let mut sequence = [0_u8; 2];
    let mut ssrc = [0_u8; 4];
    getrandom::fill(&mut timestamp).map_err(|_| TransportError::Random)?;
    getrandom::fill(&mut sequence).map_err(|_| TransportError::Random)?;
    getrandom::fill(&mut ssrc).map_err(|_| TransportError::Random)?;
    Ok((
        u32::from_ne_bytes(ssrc).max(1),
        u16::from_ne_bytes(sequence),
        u32::from_ne_bytes(timestamp),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};

    struct Receiver {
        gather: watch::Sender<RTCIceGatheringState>,
        track: watch::Sender<Option<Arc<dyn TrackRemote>>>,
    }

    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for Receiver {
        async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
            self.gather.send_replace(state);
        }

        async fn on_track(&self, track: Arc<dyn TrackRemote>) {
            self.track.send_replace(Some(track));
        }
    }

    /// Exercises actual loopback UDP/ICE/DTLS/SRTP and RTP reception with two
    /// Rust peers. Synthetic NALs are intentionally not a decoder test.
    #[tokio::test]
    async fn sends_and_receives_rtp_over_real_loopback_only() {
        let mut source = Session::new().await.unwrap();
        let mut engine = MediaEngine::default();
        engine
            .register_codec(
                RTCRtpCodecParameters {
                    rtp_codec: codec(),
                    payload_type: RTP_PAYLOAD_TYPE,
                },
                RtpCodecKind::Video,
            )
            .unwrap();
        let interceptors = register_default_interceptors(Default::default(), &mut engine).unwrap();
        let (gather, gather_rx) = watch::channel(RTCIceGatheringState::New);
        let (track, mut track_rx) = watch::channel(None);
        let sink = loopback_peer(engine, interceptors, Receiver { gather, track })
            .await
            .unwrap();

        let offer = source.create_offer().await.unwrap();
        sink.set_remote_description(RTCSessionDescription::offer(offer).unwrap())
            .await
            .unwrap();
        let answer = sink.create_answer(None).await.unwrap();
        sink.set_local_description(answer).await.unwrap();
        wait_gathered(&gather_rx).await.unwrap();
        let answer = sink.local_description().await.unwrap().sdp;
        source
            .accept_answer(sdp::omit_redundant_rtcp_candidate(&answer))
            .await
            .unwrap();
        source.wait_connected().await.unwrap();

        let annex_b = vec![
            0, 0, 0, 1, 0x67, 0x42, 0xe0, 31, 0, 0, 0, 1, 0x68, 0xee, 0, 0, 0, 1, 0x65, 0xab,
        ];
        source
            .write_frame(MediaFrame {
                annex_b,
                pts_100ns: 0,
                duration_100ns: 333_333,
            })
            .await
            .unwrap();

        let (packet_type, pt, marked) = tokio::time::timeout(SETUP_TIMEOUT, async {
            let track = track_rx
                .wait_for(Option::is_some)
                .await
                .unwrap()
                .clone()
                .unwrap();
            while let Some(event) = track.poll().await {
                if let TrackRemoteEvent::OnRtpPacket(packet) = event {
                    if packet.header.marker {
                        return (packet.payload[0] & 31, packet.header.payload_type, true);
                    }
                }
            }
            (0, 0, false)
        })
        .await
        .unwrap();
        assert_eq!((packet_type, pt, marked), (5, 96, true));
        let stats = source.refresh_stats().await;
        assert_eq!(stats.frames_sent, 1);
        assert_eq!(stats.packets_sent, 3);
        assert_eq!(stats.rtp_packets_sent, Some(3));
        assert_eq!(stats.candidate_pair_type, Some("host/udp/ipv4-loopback"));
        source.close().await.unwrap();
        sink.close().await.unwrap();
    }
}

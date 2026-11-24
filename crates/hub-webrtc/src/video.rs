use bytes::Bytes;
use hub_encode_win::h264::{normalize, EncodedVideoFrame, Framing};
use rtc::rtp::{codec::h264::H264Payloader, header::Header, packet::Packet, packetizer::Payloader};

pub const FMTP: &str = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f";
pub const SSRC: u32 = 0x5248_0004;

pub fn timestamp(pts: i64) -> Result<u32, String> {
    timestamp_with_limit(pts, 300_000_000)
}

fn timestamp_with_limit(pts: i64, max_pts: i64) -> Result<u32, String> {
    if !(0..=max_pts).contains(&pts) {
        return Err("INVALID_MEDIA_PTS".into());
    }
    Ok(((pts as u64 * 9 + 500) / 1000) as u32)
}

#[derive(Default)]
pub struct Packetizer {
    payloader: H264Payloader,
    sequence: u16,
    last_pts: Option<i64>,
    initialized: bool,
    #[cfg(feature = "v2-public-video-60s-test")]
    public_video_60s_test: bool,
    #[cfg(feature = "v2-public-control-sustained")]
    public_video_sustained: bool,
    pub source_sps_profile_level_id: Option<String>,
    pub ignored_constraint_set5: bool,
}
impl Packetizer {
    /// Only the separately consented 60-second video test uses this clock.
    #[cfg(feature = "v2-public-video-60s-test")]
    pub fn public_video_60s_test() -> Self {
        Self {
            public_video_60s_test: true,
            ..Self::default()
        }
    }

    /// Native sustained-purpose actor only. This does not choose an RTP
    /// route or replace the actor's independent monotonic cutoff.
    #[cfg(feature = "v2-public-control-sustained")]
    pub fn public_video_sustained() -> Self {
        Self {
            public_video_sustained: true,
            ..Self::default()
        }
    }

    fn frame_timestamp(&self, pts: i64) -> Result<u32, String> {
        #[cfg(feature = "v2-public-control-sustained")]
        if self.public_video_sustained {
            return timestamp_with_limit(
                pts,
                (crate::public_video::SUSTAINED_CONTROL_SESSION_SECONDS as i64) * 10_000_000,
            );
        }
        #[cfg(feature = "v2-public-video-60s-test")]
        if self.public_video_60s_test {
            return timestamp_with_limit(pts, 600_000_000);
        }
        timestamp(pts)
    }

    pub fn packets(
        &mut self,
        frame: EncodedVideoFrame,
        payload_type: u8,
    ) -> Result<Vec<Packet>, String> {
        if self.last_pts.is_some_and(|last| frame.pts_100ns <= last) || frame.duration_100ns <= 0 {
            return Err("NON_MONOTONIC_MEDIA_PTS".into());
        }
        let ts = self.frame_timestamp(frame.pts_100ns)?;
        let (normalized, types) =
            normalize(&frame.annex_b, Framing::AnnexB).map_err(|e| e.to_string())?;
        if !self.initialized {
            if !types.contains(&7) || !types.contains(&8) || !types.contains(&5) {
                return Err("FIRST_AU_REQUIRES_SPS_PPS_IDR".into());
            }
            // Read the real sequence parameter set; no SPS bytes are logged.
            let profile = normalized
                .windows(5)
                .position(|v| v[..4] == [0, 0, 0, 1] && v[4] & 31 == 7)
                .and_then(|i| normalized.get(i + 5..i + 8))
                .ok_or("TRUNCATED_SPS")?;
            // H.264 7.4.2.1.1 requires decoders to ignore constraint_set5 for profile_idc=66.
            // RFC 6184 section 8.1, Table 5: 42/x0xx0000 is Baseline and
            // 42/x1xx0000 is Constrained Baseline, a subset of Baseline decoder tools.
            // Keep the fixed level 3.1, reject other profile_idc / low reserved bits,
            // and retain the existing documented AMD constraint_set5 compatibility.
            // Never rewrite the source SPS or alter the negotiated SDP profile.
            if profile[0] != 0x42 || profile[1] & !0xf4 != 0 || profile[2] != 0x1f {
                return Err(format!(
                    "SPS_PROFILE_INCOMPATIBLE_{:02x}{:02x}{:02x}",
                    profile[0], profile[1], profile[2]
                ));
            }
            self.source_sps_profile_level_id = Some(format!(
                "{:02x}{:02x}{:02x}",
                profile[0], profile[1], profile[2]
            ));
            self.ignored_constraint_set5 = profile[1] & 0x04 != 0;
            self.initialized = true;
        }
        // The pinned RFC 6184 payloader consumes Annex B and emits single NAL/STAP-A/FU-A payloads.
        let payloads = self
            .payloader
            .payload(1188, &Bytes::from(normalized))
            .map_err(|e| e.to_string())?;
        if payloads.is_empty() {
            return Err("EMPTY_RTP_ACCESS_UNIT".into());
        }
        let count = payloads.len();
        let packets = payloads
            .into_iter()
            .enumerate()
            .map(|(i, payload)| {
                let sequence_number = self.sequence;
                self.sequence = self.sequence.wrapping_add(1);
                Packet {
                    header: Header {
                        version: 2,
                        payload_type,
                        sequence_number,
                        timestamp: ts,
                        ssrc: SSRC,
                        marker: i + 1 == count,
                        ..Default::default()
                    },
                    payload,
                }
            })
            .collect();
        self.last_pts = Some(frame.pts_100ns);
        Ok(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(pts: i64) -> EncodedVideoFrame {
        let mut au = vec![
            0, 0, 0, 1, 0x67, 0x42, 0, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65,
        ];
        au.extend(vec![0x55; 5000]);
        EncodedVideoFrame {
            annex_b: au,
            pts_100ns: pts,
            duration_100ns: 333333,
            nal_types: vec![7, 8, 5],
            is_idr: true,
            encode_elapsed_ms: 0.0,
        }
    }

    #[test]
    fn default_clock_rejects_pts_beyond_thirty_seconds() {
        assert_eq!(timestamp(300_000_000).unwrap(), 2_700_000);
        assert!(timestamp(300_000_001).is_err());
        let mut default = Packetizer::default();
        default.packets(frame(0), 102).unwrap();
        assert!(default.packets(frame(300_000_001), 102).is_err());
    }

    #[cfg(feature = "v2-public-video-60s-test")]
    #[test]
    fn explicit_extended_packetizer_admits_sixty_seconds_but_not_more() {
        let mut long = Packetizer::public_video_60s_test();
        long.packets(frame(0), 102).unwrap();
        assert_eq!(
            long.packets(frame(600_000_000), 102).unwrap()[0]
                .header
                .timestamp,
            5_400_000
        );
        assert!(long.packets(frame(600_000_001), 102).is_err());
    }
    #[cfg(feature = "v2-public-control-sustained")]
    #[test]
    fn sustained_timestamp_is_fixed_and_sequence_wrap_is_exact() {
        let mut p = Packetizer::public_video_sustained();
        p.sequence = u16::MAX - 1;
        let first = p.packets(frame(0), 102).unwrap();
        assert_eq!(first[0].header.sequence_number, u16::MAX - 1);
        assert_eq!(first[1].header.sequence_number, u16::MAX);
        assert_eq!(first[2].header.sequence_number, 0);
        let last = p.packets(frame(18_000_000_000), 102).unwrap();
        assert_eq!(last[0].header.timestamp, 162_000_000);
        assert!(p.packets(frame(18_000_000_001), 102).is_err());
    }

    #[test]
    fn exact_clock_and_fua_payloads_preserve_access_unit() {
        assert_eq!(
            (0..=300)
                .map(|n| timestamp(n * 10_000_000 / 30).unwrap())
                .collect::<Vec<_>>(),
            (0..=300).map(|n| n as u32 * 3000).collect::<Vec<_>>()
        );
        let mut p = Packetizer::default();
        let packets = p.packets(frame(0), 102).unwrap();
        assert!(packets.len() > 4);
        assert!(packets.iter().all(|v| v.payload.len() <= 1188
            && v.header.payload_type == 102
            && v.header.timestamp == 0));
        assert_eq!(packets.iter().filter(|p| p.header.marker).count(), 1);
        let fu: Vec<_> = packets.iter().filter(|p| p.payload[0] & 31 == 28).collect();
        assert!(fu[0].payload[1] & 0x80 != 0);
        assert!(fu.last().unwrap().payload[1] & 0x40 != 0);
        let rebuilt: Vec<_> = fu
            .iter()
            .flat_map(|p| p.payload[2..].iter().copied())
            .collect();
        assert_eq!(rebuilt, vec![0x55; 5000]);
        assert!(p.packets(frame(0), 102).is_err());
        assert_eq!(
            p.packets(frame(333333), 102).unwrap()[0].header.timestamp,
            3000
        );
    }
    #[test]
    fn constrained_baseline_level31_is_a_baseline_decoder_subset_without_sps_rewrite() {
        for constraints in [0x00, 0x80, 0x40, 0xc0, 0xe0, 0xf0, 0x04, 0xc4] {
            let mut au = frame(0);
            au.annex_b[6] = constraints;
            let mut p = Packetizer::default();
            let packets = p
                .packets(au, 102)
                .expect("Baseline-family constraints require no extra decoder tools");
            assert_eq!(
                p.source_sps_profile_level_id,
                Some(format!("42{constraints:02x}1f"))
            );
            assert!(packets.iter().any(|packet| packet
                .payload
                .windows(4)
                .any(|b| b == [0x67, 0x42, constraints, 0x1f])));
            assert_eq!(p.ignored_constraint_set5, constraints & 4 != 0);
        }
        for (idc, constraints, level) in [
            (0x4d, 0xc0, 0x1f),
            (0x64, 0, 0x1f),
            (0x42, 0xc0, 0x20),
            (0x42, 0xc1, 0x1f),
            (0x42, 0xc2, 0x1f),
            (0x42, 0xc8, 0x1f),
        ] {
            let mut au = frame(0);
            au.annex_b[5..8].copy_from_slice(&[idc, constraints, level]);
            assert!(Packetizer::default().packets(au, 102).is_err());
        }
    }
    #[test]
    fn amd_reserved_constraint5_is_disclosed_and_never_rewrites_the_sps() {
        let mut au = frame(0);
        au.annex_b[6] = 0x04;
        let mut p = Packetizer::default();
        let packets = p.packets(au, 102).unwrap();
        assert_eq!(p.source_sps_profile_level_id.as_deref(), Some("42041f"));
        assert!(p.ignored_constraint_set5);
        assert!(packets.iter().any(|packet| packet
            .payload
            .windows(4)
            .any(|b| b == [0x67, 0x42, 0x04, 0x1f])));
        let mut high = frame(0);
        high.annex_b[5] = 0x64;
        assert!(Packetizer::default().packets(high, 102).is_err());
    }
}

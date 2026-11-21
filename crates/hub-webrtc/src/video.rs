use bytes::Bytes;
use hub_encode_win::h264::{normalize, EncodedVideoFrame, Framing};
use rtc::rtp::{codec::h264::H264Payloader, header::Header, packet::Packet, packetizer::Payloader};

pub const FMTP: &str = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f";
pub const SSRC: u32 = 0x5248_0004;

pub fn timestamp(pts: i64) -> Result<u32, String> {
    if !(0..=300_000_000).contains(&pts) {
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
    pub source_sps_profile_level_id: Option<String>,
    pub ignored_constraint_set5: bool,
}
impl Packetizer {
    pub fn packets(
        &mut self,
        frame: EncodedVideoFrame,
        payload_type: u8,
    ) -> Result<Vec<Packet>, String> {
        if self.last_pts.is_some_and(|last| frame.pts_100ns <= last) || frame.duration_100ns <= 0 {
            return Err("NON_MONOTONIC_MEDIA_PTS".into());
        }
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
            // AMD emits 42041f on this driver. Keep the original SPS unchanged and disclose it.
            // This is decoder compatibility, not a claim that the encoder's reserved flag conforms.
            if profile[0] != 0x42 || profile[1] & !0x04 != 0 || profile[2] != 0x1f {
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
        let ts = timestamp(frame.pts_100ns)?;
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

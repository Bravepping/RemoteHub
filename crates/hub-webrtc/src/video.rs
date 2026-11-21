use crate::TransportError;
use bytes::Bytes;
use rtc::rtp::{header::Header, packet::Packet};

const MAX_AU_BYTES: usize = 8 * 1024 * 1024;
const MAX_NALS: usize = 256;
// Allow ample room for RTP, SRTP, UDP and IPv4 headers under a 1200-byte datagram.
const MAX_RTP_PAYLOAD: usize = 1100;

pub struct MediaFrame {
    pub annex_b: Vec<u8>,
    pub pts_100ns: i64,
    pub duration_100ns: i64,
}

/// Validates the default P003 720p30 Baseline stream against RFC 6184 profile semantics.
/// Constrained Baseline is a proper Baseline subset; no other profile is accepted.
fn baseline_sps(nalu: &[u8], max_level_idc: u8) -> Result<[u8; 3], TransportError> {
    let [0x67 | 0x27 | 0x47, 0x42, compatibility, level, ..] = nalu else {
        return Err(TransportError::Profile);
    };
    if compatibility & 0x0f != 0 || *level == 0 || *level > max_level_idc {
        return Err(TransportError::Profile);
    }
    Ok([0x42, *compatibility, *level])
}

fn annex_b_units(data: &[u8]) -> Result<Vec<&[u8]>, TransportError> {
    if data.is_empty() || data.len() > MAX_AU_BYTES {
        return Err(TransportError::AccessUnit);
    }
    let mut units = Vec::new();
    let mut offset = 0;
    let mut nal_start = None;
    while offset + 3 <= data.len() {
        let prefix_len = if data[offset..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if data[offset..].starts_with(&[0, 0, 1]) {
            3
        } else {
            offset += 1;
            continue;
        };
        if let Some(start) = nal_start {
            units.push(&data[start..offset]);
        } else if offset != 0 {
            return Err(TransportError::AccessUnit);
        }
        if units.len() == MAX_NALS {
            return Err(TransportError::AccessUnit);
        }
        offset += prefix_len;
        nal_start = Some(offset);
    }
    units.push(&data[nal_start.ok_or(TransportError::AccessUnit)?..]);
    for unit in &mut units {
        while let Some(without_end) = unit.strip_suffix(&[0]) {
            *unit = without_end;
        }
    }
    if units
        .iter()
        .any(|unit| unit.len() < 2 || unit[0] & 0x80 != 0 || !(1..=23).contains(&(unit[0] & 31)))
    {
        return Err(TransportError::AccessUnit);
    }
    Ok(units)
}

pub(crate) struct Packetizer {
    initial_timestamp: u32,
    first_pts_100ns: Option<i64>,
    last_timestamp: Option<u32>,
    sequence: u16,
    ssrc: u32,
    sps: Option<[u8; 3]>,
}

pub(crate) struct Packetized {
    pub packets: Vec<Packet>,
    pub sps: [u8; 3],
}

impl Packetizer {
    pub fn new(ssrc: u32, sequence: u16, initial_timestamp: u32) -> Self {
        Self {
            ssrc,
            sequence,
            initial_timestamp,
            first_pts_100ns: None,
            last_timestamp: None,
            sps: None,
        }
    }

    pub fn packetize(
        &mut self,
        frame: &MediaFrame,
        pt: u8,
        max_level_idc: u8,
    ) -> Result<Packetized, TransportError> {
        let base_pts = self.first_pts_100ns.unwrap_or(frame.pts_100ns);
        if frame.pts_100ns < 0 || frame.duration_100ns <= 0 || frame.pts_100ns < base_pts {
            return Err(TransportError::Timestamp);
        }
        // Round each absolute offset independently, avoiding accumulated 100ns truncation.
        // 90,000 / 10,000,000 = 9 / 1000.
        let elapsed = u32::try_from((i128::from(frame.pts_100ns - base_pts) * 9 + 500) / 1000)
            .map_err(|_| TransportError::Timestamp)?;
        if self.last_timestamp.is_some_and(|prev| elapsed <= prev) {
            return Err(TransportError::Timestamp);
        }
        let ts = self.initial_timestamp.wrapping_add(elapsed);

        let units = annex_b_units(&frame.annex_b)?;
        let mut sps = self.sps;
        let mut has_sps_before_vcl = false;
        let mut has_pps_before_vcl = false;
        let mut has_vcl = false;
        let mut has_idr = false;
        for unit in &units {
            match unit[0] & 31 {
                7 => {
                    let profile = baseline_sps(unit, max_level_idc)?;
                    if sps.is_some_and(|previous| previous != profile) {
                        return Err(TransportError::Profile);
                    }
                    sps = Some(profile);
                    has_sps_before_vcl |= !has_vcl;
                }
                8 => has_pps_before_vcl |= !has_vcl,
                1..=4 => has_vcl = true,
                5 => {
                    has_idr = true;
                    has_vcl = true;
                }
                _ => (),
            }
        }

        // First frame and each periodic IDR must be an independently restartable AU.
        if !has_vcl
            || (self.first_pts_100ns.is_none() && !has_idr)
            || (has_idr && !(has_sps_before_vcl && has_pps_before_vcl))
        {
            return Err(TransportError::AccessUnit);
        }
        let sps = sps.ok_or(TransportError::AccessUnit)?;
        let mut packets = Vec::new();
        let mut sequence = self.sequence;
        for unit in units {
            let unit_type = unit[0] & 31;
            // The AU already has framing. AUD and filler aren't needed over RTP.
            if unit_type == 9 || unit_type == 12 {
                continue;
            }
            let mut append = |payload| {
                packets.push(Packet {
                    header: Header {
                        version: 2,
                        marker: false,
                        payload_type: pt,
                        sequence_number: sequence,
                        timestamp: ts,
                        ssrc: self.ssrc,
                        ..Default::default()
                    },
                    payload,
                });
                sequence = sequence.wrapping_add(1);
            };
            if unit.len() <= MAX_RTP_PAYLOAD {
                append(Bytes::copy_from_slice(unit));
                continue;
            }

            // RFC 6184, non-interleaved packetization mode: FU-A for oversized NALs.
            let fu_indicator = (unit[0] & 0xe0) | 28;
            let segments = unit[1..].chunks(MAX_RTP_PAYLOAD - 2);
            let total = segments.len();
            for (i, segment) in segments.enumerate() {
                let mut payload = Vec::with_capacity(segment.len() + 2);
                let first = if i == 0 { 0x80 } else { 0 };
                let last = if i + 1 == total { 0x40 } else { 0 };
                payload.extend([fu_indicator, unit_type | first | last]);
                payload.extend_from_slice(segment);
                append(Bytes::from(payload));
            }
        }
        if let Some(packet) = packets.last_mut() {
            packet.header.marker = true;
        }

        self.first_pts_100ns = Some(base_pts);
        self.last_timestamp = Some(elapsed);
        self.sequence = sequence;
        self.sps = Some(sps);
        Ok(Packetized { packets, sps })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h264(pts: i64, big: bool) -> MediaFrame {
        let mut annex_b = vec![0, 0, 0, 1, 0x67, 0x42, 0xe0, 31, 0, 0, 1, 0x68, 0xff];
        annex_b.extend([0, 0, 0, 1, 0x65]);
        annex_b.extend(vec![0x55; if big { 2300 } else { 5 }]);
        MediaFrame {
            annex_b,
            pts_100ns: pts,
            duration_100ns: 333_333,
        }
    }

    #[test]
    fn packetizes_idr_with_fu_a_and_single_marker() {
        let mut packetizer = Packetizer::new(7, u16::MAX - 1, 100);
        let result = packetizer.packetize(&h264(91, true), 96, 31).unwrap();
        assert_eq!(result.sps, [0x42, 0xe0, 31]);
        let packets = result.packets;
        assert_eq!(packets.len(), 5);
        assert_eq!(packets[0].payload[0], 0x67);
        assert_eq!(packets[1].payload[0], 0x68);
        assert_eq!(packets[2].payload[..2], [0x7c, 0x85]);
        assert_eq!(packets[3].payload[..2], [0x7c, 0x05]);
        assert_eq!(packets[4].payload[..2], [0x7c, 0x45]);
        assert!(!packets[..4].iter().any(|p| p.header.marker));
        assert!(packets[4].header.marker);
        assert_eq!(packets[2].header.sequence_number, 0);
        assert!(packets.iter().all(|p| p.header.timestamp == 100));
        assert!(packets.iter().all(|p| p.payload.len() <= MAX_RTP_PAYLOAD));
    }

    #[test]
    fn exact_30hz_offsets_with_rtp_wrap_and_pts_validation() {
        let mut packetizer = Packetizer::new(7, 0, u32::MAX - 20);
        for index in 0..10 {
            let pts = 50 + index * 10_000_000 / 30;
            let r = packetizer.packetize(&h264(pts, false), 96, 31).unwrap();
            assert_eq!(
                r.packets[0].header.timestamp,
                (u32::MAX - 20).wrapping_add(index as u32 * 3000)
            );
        }
        assert!(matches!(
            packetizer.packetize(&h264(99, false), 96, 31),
            Err(TransportError::Timestamp)
        ));
    }

    #[test]
    fn rejects_wrong_profile_and_unprimed_non_idr() {
        let mut packetizer = Packetizer::new(7, 0, 100);
        let mut frame = h264(50, false);
        frame.annex_b[5] = 0x64;
        assert!(matches!(
            packetizer.packetize(&frame, 96, 31),
            Err(TransportError::Profile)
        ));
        frame.annex_b = vec![0, 0, 1, 0x41, 0x55];
        assert!(matches!(
            packetizer.packetize(&frame, 96, 31),
            Err(TransportError::AccessUnit)
        ));
    }
}

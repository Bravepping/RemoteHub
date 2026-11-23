//! Bounded RFC 6184 packetization-mode 1 input for the native video fixture.
//! Call only on RTP already admitted by the signed live transport gate.

use rtc::rtp::packet::Packet;

pub const MAX_VIDEO_AU_PACKETS: usize = 32;
pub const MAX_VIDEO_AU_BYTES: usize = 1024 * 1024;
const PAYLOAD_TYPE: u8 = 102;
const RTP_HZ: u128 = 90_000;
const START_CODE: &[u8] = &[0, 0, 0, 1];
const MALFORMED: &str = "PUBLIC_VIDEO_H264_MALFORMED";
const LIMIT: &str = "PUBLIC_VIDEO_H264_LIMIT";
const TIMESTAMP: &str = "PUBLIC_VIDEO_RTP_TIMESTAMP";
const GAP: &str = "PUBLIC_VIDEO_RTP_GAP";
const SOURCE: &str = "PUBLIC_VIDEO_RTP_SOURCE";
const BOOTSTRAP: &str = "PUBLIC_VIDEO_H264_BOOTSTRAP";
const CLOSED: &str = "PUBLIC_VIDEO_H264_CLOSED";

/// A complete, marker-delimited Annex B access unit. No decode or presentation
/// has taken place. The sequence is the AU ordinal, not an RTP sequence number.
pub struct VerifiedVideoFrame {
    pub sequence: u64,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub annex_b: Vec<u8>,
}

pub struct PublicVideoFrameAssembler {
    expected_ssrc: u32,
    next_packet: Option<u16>,
    next_frame: u64,
    last_timestamp: Option<(u32, u64)>,
    pending: Option<AccessUnit>,
    failed: bool,
}

impl PublicVideoFrameAssembler {
    pub fn new(expected_ssrc: u32) -> Self {
        Self {
            expected_ssrc,
            next_packet: None,
            next_frame: 0,
            last_timestamp: None,
            pending: None,
            failed: false,
        }
    }

    /// Process exactly one authenticated RTP packet. A malformed packet or
    /// missing sequence poisons this decoder input rather than releasing a
    /// truncated frame or silently resynchronizing.
    pub fn push(&mut self, packet: &Packet) -> Result<Option<VerifiedVideoFrame>, &'static str> {
        if self.failed {
            return Err(CLOSED);
        }
        let result = self.push_packet(packet);
        if result.is_err() {
            self.pending = None;
            self.failed = true;
        }
        result
    }

    fn push_packet(&mut self, packet: &Packet) -> Result<Option<VerifiedVideoFrame>, &'static str> {
        let header = &packet.header;
        if header.version != 2
            || header.payload_type != PAYLOAD_TYPE
            || header.ssrc != self.expected_ssrc
        {
            return Err(SOURCE);
        }
        if self
            .next_packet
            .is_some_and(|next| header.sequence_number != next)
        {
            return Err(GAP);
        }
        if packet.payload.len() > MAX_VIDEO_AU_BYTES {
            return Err(LIMIT);
        }
        let bootstrap = self.next_frame == 0;
        let au = match self.pending.as_mut() {
            Some(au) if au.timestamp == header.timestamp => au,
            Some(_) => return Err(TIMESTAMP), // No marker on the previous timestamp.
            None => {
                let elapsed = match self.last_timestamp {
                    None => 0,
                    Some((last, elapsed)) => {
                        let delta = header.timestamp.wrapping_sub(last);
                        if delta == 0 || delta >= (1 << 31) {
                            return Err(TIMESTAMP);
                        }
                        elapsed.checked_add(u64::from(delta)).ok_or(TIMESTAMP)?
                    }
                };
                self.pending
                    .insert(AccessUnit::new(header.timestamp, elapsed))
            }
        };
        if au.packets == MAX_VIDEO_AU_PACKETS {
            return Err(LIMIT);
        }
        au.packets += 1;
        au.read_payload(&packet.payload, bootstrap)?;
        self.next_packet = Some(header.sequence_number.wrapping_add(1));
        if !header.marker {
            return Ok(None);
        }

        let au = self.pending.take().ok_or(MALFORMED)?;
        if au.fragment.is_some() {
            return Err(MALFORMED);
        }
        let keyframe = au
            .vcl
            .ok_or(if bootstrap { BOOTSTRAP } else { MALFORMED })?;
        if bootstrap && !(au.sps && au.pps && keyframe) {
            return Err(BOOTSTRAP);
        }
        let sequence = self.next_frame;
        self.next_frame = sequence.checked_add(1).ok_or(LIMIT)?;
        self.last_timestamp = Some((au.timestamp, au.elapsed));
        let timestamp_us =
            u64::try_from(u128::from(au.elapsed) * 1_000_000 / RTP_HZ).map_err(|_| TIMESTAMP)?;
        Ok(Some(VerifiedVideoFrame {
            sequence,
            timestamp_us,
            keyframe,
            annex_b: au.annex_b,
        }))
    }
}

struct AccessUnit {
    timestamp: u32,
    elapsed: u64,
    packets: usize,
    annex_b: Vec<u8>,
    fragment: Option<Fragment>,
    sps: bool,
    pps: bool,
    vcl: Option<bool>,
}

struct Fragment {
    header: u8,
    body_start: usize,
}

impl AccessUnit {
    fn new(timestamp: u32, elapsed: u64) -> Self {
        Self {
            timestamp,
            elapsed,
            packets: 0,
            annex_b: Vec::new(),
            fragment: None,
            sps: false,
            pps: false,
            vcl: None,
        }
    }

    fn read_payload(&mut self, payload: &[u8], bootstrap: bool) -> Result<(), &'static str> {
        let (indicator, _) = payload.split_first().ok_or(MALFORMED)?;
        if indicator & 0x80 != 0 {
            return Err(MALFORMED);
        }
        match indicator & 0x1f {
            28 => self.read_fu(payload, bootstrap),
            _ if self.fragment.is_some() => Err(MALFORMED),
            24 => self.read_stap(payload, bootstrap),
            _ => self.add_single(payload, bootstrap),
        }
    }

    fn add_single(&mut self, nal: &[u8], bootstrap: bool) -> Result<(), &'static str> {
        let (&header, body) = nal.split_first().ok_or(MALFORMED)?;
        validate_nal_header(header)?;
        validate_nal_body(body)?;
        self.record_nal(header, bootstrap)?;
        self.append(START_CODE, nal)
    }

    fn read_stap(&mut self, payload: &[u8], bootstrap: bool) -> Result<(), &'static str> {
        let (&indicator, mut remaining) = payload.split_first().ok_or(MALFORMED)?;
        if remaining.is_empty() {
            return Err(MALFORMED);
        }
        let mut max_nri = 0;
        while !remaining.is_empty() {
            let [high, low, rest @ ..] = remaining else {
                return Err(MALFORMED);
            };
            let n = usize::from(u16::from_be_bytes([*high, *low]));
            let (nal, more) = rest.split_at_checked(n).ok_or(MALFORMED)?;
            let header = *nal.first().ok_or(MALFORMED)?;
            max_nri = max_nri.max(header & 0x60);
            self.add_single(nal, bootstrap)?;
            remaining = more;
        }
        if max_nri != (indicator & 0x60) {
            return Err(MALFORMED);
        }
        Ok(())
    }

    fn read_fu(&mut self, payload: &[u8], bootstrap: bool) -> Result<(), &'static str> {
        let [indicator, flags, body @ ..] = payload else {
            return Err(MALFORMED);
        };
        let header = (indicator & 0xe0) | (flags & 0x1f);
        validate_nal_header(header)?;
        let start = flags & 0x80 != 0;
        let end = flags & 0x40 != 0;
        if body.is_empty() || flags & 0x20 != 0 || (start && end) {
            return Err(MALFORMED);
        }
        if start {
            if self.fragment.is_some() {
                return Err(MALFORMED);
            }
            self.append(START_CODE, &[header])?;
            let body_start = self.annex_b.len();
            self.append(&[], body)?;
            self.fragment = Some(Fragment { header, body_start });
            return Ok(());
        }
        let fragment = self.fragment.as_ref().ok_or(MALFORMED)?;
        if fragment.header != header {
            return Err(MALFORMED);
        }
        let body_start = fragment.body_start;
        self.append(&[], body)?;
        if end {
            validate_nal_body(self.annex_b.get(body_start..).ok_or(MALFORMED)?)?;
            self.record_nal(header, bootstrap)?;
            self.fragment = None;
        }
        Ok(())
    }

    fn record_nal(&mut self, header: u8, bootstrap: bool) -> Result<(), &'static str> {
        match header & 0x1f {
            7 if self.vcl.is_none() => self.sps = true,
            8 if self.vcl.is_none() => self.pps = true,
            7 | 8 => return Err(MALFORMED),
            kind @ (1 | 5) => {
                let idr = kind == 5;
                if bootstrap && !(self.sps && self.pps && idr) {
                    return Err(BOOTSTRAP);
                }
                if self.vcl.is_some_and(|previous| previous != idr) {
                    return Err(MALFORMED);
                }
                self.vcl = Some(idr);
            }
            _ => {}
        }
        Ok(())
    }

    fn append(&mut self, prefix: &[u8], body: &[u8]) -> Result<(), &'static str> {
        let len = self.annex_b.len();
        let total = len
            .checked_add(prefix.len())
            .and_then(|len| len.checked_add(body.len()))
            .filter(|len| *len <= MAX_VIDEO_AU_BYTES)
            .ok_or(LIMIT)?;
        let capacity = self.annex_b.capacity();
        if total > capacity {
            let target = total
                .max(capacity.saturating_mul(2))
                .min(MAX_VIDEO_AU_BYTES);
            self.annex_b
                .try_reserve_exact(target - len)
                .map_err(|_| LIMIT)?;
        }
        self.annex_b.extend_from_slice(prefix);
        self.annex_b.extend_from_slice(body);
        Ok(())
    }
}

fn validate_nal_header(header: u8) -> Result<(), &'static str> {
    // Baseline/AVC slices and common non-VCL units only. Data partitions,
    // extension NALs, nested aggregation and interleaved modes are unsupported.
    if header & 0x80 != 0 || !matches!(header & 0x1f, 1 | 5..=12) {
        return Err(MALFORMED);
    }
    Ok(())
}

fn validate_nal_body(body: &[u8]) -> Result<(), &'static str> {
    // Annex B reassembly must not expose NALs smuggled inside an RTP NAL's
    // escaped byte sequence, including across FU-A boundaries.
    if body.is_empty() || body.windows(3).any(|window| window == [0, 0, 1]) {
        return Err(MALFORMED);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use rtc::rtp::header::Header;

    const SSRC: u32 = 42;
    // SPS, PPS and IDR: structure-only bytes, not sent to a decoder.
    const FIRST: &[u8] = &[0x78, 0, 2, 0x67, 7, 0, 2, 0x68, 8, 0, 2, 0x65, 5];

    fn packet(seq: u16, stamp: u32, marker: bool, payload: &[u8]) -> Packet {
        Packet {
            header: Header {
                version: 2,
                marker,
                payload_type: PAYLOAD_TYPE,
                sequence_number: seq,
                timestamp: stamp,
                ssrc: SSRC,
                ..Default::default()
            },
            payload: Bytes::copy_from_slice(payload),
        }
    }

    #[test]
    fn stap_bootstrap_and_fragmented_access_unit_unwrap_sequence_and_timestamp() {
        let mut assembler = PublicVideoFrameAssembler::new(SSRC);
        let first = assembler
            .push(&packet(u16::MAX, u32::MAX - 4499, true, FIRST))
            .unwrap()
            .unwrap();
        assert_eq!(
            (first.sequence, first.timestamp_us, first.keyframe),
            (0, 0, true)
        );
        assert_eq!(
            first.annex_b,
            [0, 0, 0, 1, 0x67, 7, 0, 0, 0, 1, 0x68, 8, 0, 0, 0, 1, 0x65, 5]
        );

        assert!(assembler
            .push(&packet(0, 4500, false, &[0x7c, 0x81, 3, 0, 0]))
            .unwrap()
            .is_none());
        let second = assembler
            .push(&packet(1, 4500, true, &[0x7c, 0x41, 3, 1]))
            .unwrap()
            .unwrap();
        assert_eq!(
            (second.sequence, second.timestamp_us, second.keyframe),
            (1, 100_000, false)
        );
        assert_eq!(second.annex_b, [0, 0, 0, 1, 0x61, 3, 0, 0, 3, 1]);
    }

    #[test]
    fn gaps_bad_sources_and_missing_bootstrap_poison_the_assembler() {
        let mut assembler = PublicVideoFrameAssembler::new(SSRC);
        assert!(assembler
            .push(&packet(10, 0, false, &[0x67, 7]))
            .unwrap()
            .is_none());
        assert!(matches!(
            assembler.push(&packet(12, 0, true, FIRST)),
            Err(GAP)
        ));
        assert!(matches!(
            assembler.push(&packet(11, 0, true, FIRST)),
            Err(CLOSED)
        ));

        assert!(matches!(
            PublicVideoFrameAssembler::new(SSRC).push(&packet(1, 1, true, &[0x65, 1])),
            Err(BOOTSTRAP)
        ));
        let mut wrong = packet(1, 1, true, FIRST);
        wrong.header.payload_type -= 1;
        assert!(matches!(
            PublicVideoFrameAssembler::new(SSRC).push(&wrong),
            Err(SOURCE)
        ));
        assert!(matches!(
            PublicVideoFrameAssembler::new(SSRC + 1).push(&packet(1, 1, true, FIRST)),
            Err(SOURCE)
        ));
    }

    #[test]
    fn malformed_stap_fu_and_missing_markers_never_release_partial_access_units() {
        for payload in [
            &[0x78, 0, 3, 0x67, 7][..],
            &[0x78, 0, 0][..],
            &[0x78, 0, 2, 0x78, 7][..],
            &[0x7c, 0xc5, 1][..],
            &[0x7c, 0x45, 1][..],
            &[0x7c, 0x85, 1][..],
        ] {
            assert!(matches!(
                PublicVideoFrameAssembler::new(SSRC).push(&packet(1, 1, true, payload)),
                Err(MALFORMED)
            ));
        }
        let mut assembler = PublicVideoFrameAssembler::new(SSRC);
        assembler.push(&packet(1, 0, true, FIRST)).unwrap().unwrap();
        assert!(assembler
            .push(&packet(2, 1, false, &[0x7c, 0x81, 0, 0]))
            .unwrap()
            .is_none());
        assert!(matches!(
            assembler.push(&packet(3, 1, true, &[0x7c, 0x41, 1])),
            Err(MALFORMED)
        ));

        let mut assembler = PublicVideoFrameAssembler::new(SSRC);
        assert!(assembler
            .push(&packet(1, 1, false, FIRST))
            .unwrap()
            .is_none());
        assert!(matches!(
            assembler.push(&packet(2, 2, true, FIRST)),
            Err(TIMESTAMP)
        ));
        let mut assembler = PublicVideoFrameAssembler::new(SSRC);
        assembler.push(&packet(1, 2, true, FIRST)).unwrap().unwrap();
        assert!(matches!(
            assembler.push(&packet(2, 1, true, FIRST)),
            Err(TIMESTAMP)
        ));
    }

    #[test]
    fn packet_budget_and_annex_b_overhead_are_bounded() {
        let mut assembler = PublicVideoFrameAssembler::new(SSRC);
        for seq in 0..MAX_VIDEO_AU_PACKETS as u16 {
            assert!(assembler
                .push(&packet(seq, 0, false, &[0x09, 0x10]))
                .unwrap()
                .is_none());
        }
        assert!(matches!(
            assembler.push(&packet(MAX_VIDEO_AU_PACKETS as u16, 0, true, FIRST)),
            Err(LIMIT)
        ));

        let mut large = vec![0xab; MAX_VIDEO_AU_BYTES];
        large[0] = 0x67;
        assert!(matches!(
            PublicVideoFrameAssembler::new(SSRC).push(&packet(0, 0, false, &large)),
            Err(LIMIT)
        ));
    }
}

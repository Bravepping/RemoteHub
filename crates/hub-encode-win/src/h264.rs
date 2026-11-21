use crate::EncodeError;

pub const MAX_ACCESS_UNIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct EncodedVideoFrame {
    pub annex_b: Vec<u8>,
    pub pts_100ns: i64,
    pub duration_100ns: i64,
    pub nal_types: Vec<u8>,
    pub is_idr: bool,
    pub encode_elapsed_ms: f64,
}

#[derive(Clone, Copy, Debug)]
pub enum Framing {
    AnnexB,
    Avcc { length_size: usize },
}

pub fn normalize(data: &[u8], framing: Framing) -> Result<(Vec<u8>, Vec<u8>), EncodeError> {
    if data.is_empty() || data.len() > MAX_ACCESS_UNIT {
        return Err(EncodeError::Bitstream("empty or oversized access unit"));
    }
    let units = match framing {
        Framing::AnnexB => {
            let mut starts = Vec::new();
            let mut i = 0;
            while i + 3 <= data.len() {
                let n = if data[i..].starts_with(&[0, 0, 0, 1]) {
                    4
                } else if data[i..].starts_with(&[0, 0, 1]) {
                    3
                } else {
                    i += 1;
                    continue;
                };
                starts.push((i, i + n));
                if starts.len() > 256 {
                    return Err(EncodeError::Bitstream("too many NAL units"));
                }
                i += n;
            }
            if starts.first().map(|p| p.0) != Some(0) {
                return Err(EncodeError::Bitstream("missing Annex B start code"));
            }
            starts
                .iter()
                .enumerate()
                .map(|(index, &(_, from))| {
                    let to = starts.get(index + 1).map(|p| p.0).unwrap_or(data.len());
                    &data[from..to]
                })
                .collect::<Vec<_>>()
        }
        Framing::Avcc { length_size } => {
            if !(1..=4).contains(&length_size) {
                return Err(EncodeError::Bitstream("invalid AVCC length width"));
            }
            let mut units = Vec::new();
            let mut i = 0;
            while i < data.len() {
                if data.len() - i < length_size {
                    return Err(EncodeError::Bitstream("truncated AVCC length"));
                }
                let len = data[i..i + length_size]
                    .iter()
                    .fold(0usize, |n, b| (n << 8) | *b as usize);
                i += length_size;
                if len == 0 || len > data.len() - i {
                    return Err(EncodeError::Bitstream("invalid AVCC NAL length"));
                }
                units.push(&data[i..i + len]);
                if units.len() > 256 {
                    return Err(EncodeError::Bitstream("too many NAL units"));
                }
                i += len;
            }
            units
        }
    };
    let mut out = Vec::new();
    let mut types = Vec::new();
    for unit in units {
        if unit.is_empty() || unit[0] & 0x80 != 0 || !(1..=23).contains(&(unit[0] & 31)) {
            return Err(EncodeError::Bitstream("invalid NAL header"));
        }
        types.push(unit[0] & 31);
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(unit);
        if out.len() > MAX_ACCESS_UNIT {
            return Err(EncodeError::Bitstream("normalized AU too large"));
        }
    }
    Ok((out, types))
}

/// Parse an AVCDecoderConfigurationRecord, retaining its SPS/PPS for Annex B IDR initialization.
pub fn avcc_configuration(data: &[u8]) -> Result<(usize, Vec<u8>), EncodeError> {
    if data.len() < 7 || data.len() > MAX_ACCESS_UNIT || data[0] != 1 {
        return Err(EncodeError::Bitstream("invalid AVC configuration"));
    }
    let length_size = (data[4] & 3) as usize + 1;
    let mut cursor = 6;
    let sps_count = data[5] & 31;
    let mut header = Vec::new();
    for expected in [7u8, 8] {
        let count = if expected == 7 {
            sps_count
        } else {
            let n = *data
                .get(cursor)
                .ok_or(EncodeError::Bitstream("missing PPS count"))?;
            cursor += 1;
            n
        };
        if count == 0 {
            return Err(EncodeError::Bitstream("AVC configuration lacks SPS/PPS"));
        }
        for _ in 0..count {
            let field = data
                .get(cursor..cursor + 2)
                .ok_or(EncodeError::Bitstream("truncated parameter length"))?;
            let size = u16::from_be_bytes([field[0], field[1]]) as usize;
            cursor += 2;
            let nal = data
                .get(cursor..cursor + size)
                .ok_or(EncodeError::Bitstream("truncated parameter set"))?;
            if nal.is_empty() || nal[0] & 0x80 != 0 || nal[0] & 31 != expected {
                return Err(EncodeError::Bitstream("invalid parameter set type"));
            }
            header.extend_from_slice(&[0, 0, 0, 1]);
            header.extend_from_slice(nal);
            cursor += size;
        }
    }
    Ok((length_size, header))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annex_b_and_avcc_have_identical_nals() {
        let expected = vec![
            0, 0, 0, 1, 0x67, 4, 0, 0, 0, 1, 0x68, 5, 0, 0, 0, 1, 0x65, 6,
        ];
        let (a, types) = normalize(
            &[0, 0, 1, 0x67, 4, 0, 0, 0, 1, 0x68, 5, 0, 0, 1, 0x65, 6],
            Framing::AnnexB,
        )
        .unwrap();
        assert_eq!(a, expected);
        assert_eq!(types, [7, 8, 5]);
        assert_eq!(
            normalize(
                &[0, 2, 0x67, 4, 0, 2, 0x68, 5, 0, 2, 0x65, 6],
                Framing::Avcc { length_size: 2 }
            )
            .unwrap()
            .0,
            expected
        );
    }
    #[test]
    fn rejects_empty_corrupt_and_truncated_nals() {
        for bytes in [
            &[][..],
            &[0, 0, 1][..],
            &[0, 0, 1, 0x80][..],
            &[1, 2, 3][..],
        ] {
            assert!(normalize(bytes, Framing::AnnexB).is_err());
        }
        for bytes in [&[0][..], &[0, 3, 0x65][..], &[0, 0][..]] {
            assert!(normalize(bytes, Framing::Avcc { length_size: 2 }).is_err());
        }
        assert!(normalize(&[0, 1, 0x65], Framing::Avcc { length_size: 0 }).is_err());
    }

    #[test]
    fn avcc_configuration_retains_parameter_sets_and_rejects_truncation() {
        let config = [1, 66, 0, 31, 255, 225, 0, 2, 0x67, 1, 1, 0, 2, 0x68, 2];
        let (width, header) = avcc_configuration(&config).unwrap();
        assert_eq!(width, 4);
        assert_eq!(normalize(&header, Framing::AnnexB).unwrap().1, [7, 8]);
        for end in 0..config.len() {
            assert!(avcc_configuration(&config[..end]).is_err());
        }
    }
}

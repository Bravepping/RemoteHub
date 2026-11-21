use crate::EncodeError;

/// CPU bilinear BGRA resize followed by BT.601 limited-range NV12 (Y then interleaved UV).
pub fn bgra_to_nv12(
    source: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
    out_width: u32,
    out_height: u32,
) -> Result<Vec<u8>, EncodeError> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || out_width == 0
        || out_height == 0
        || out_width > 1920
        || out_height > 1080
        || !out_width.is_multiple_of(2)
        || !out_height.is_multiple_of(2)
    {
        return Err(EncodeError::InvalidArgument(
            "unsupported BGRA/NV12 dimensions",
        ));
    }
    let needed = pitch
        .checked_mul(height as usize)
        .ok_or(EncodeError::InvalidArgument("BGRA size overflow"))?;
    if pitch < width as usize * 4 || source.len() < needed {
        return Err(EncodeError::InvalidArgument(
            "truncated BGRA or invalid stride",
        ));
    }
    let ow = out_width as usize;
    let oh = out_height as usize;
    let mut scaled = vec![0u8; ow * oh * 4];
    // Pixel-center mapping, clamped at the image edge, with fixed-point interpolation weights.
    let coordinate = |out: usize, src: u32, dst: usize| {
        let pos = (((2 * out + 1) as u64 * src as u64 * 256 / (2 * dst) as u64) as i64 - 128)
            .clamp(0, (src as i64 - 1) * 256);
        (pos as usize / 256, pos as u32 % 256)
    };
    let xs: Vec<_> = (0..ow).map(|x| coordinate(x, width, ow)).collect();
    for y in 0..oh {
        let (y0, fy) = coordinate(y, height, oh);
        let y1 = (y0 + 1).min(height as usize - 1);
        for (x, &(x0, fx)) in xs.iter().enumerate() {
            let x1 = (x0 + 1).min(width as usize - 1);
            for c in 0..3 {
                let a = source[y0 * pitch + x0 * 4 + c] as u32;
                let b = source[y0 * pitch + x1 * 4 + c] as u32;
                let d = source[y1 * pitch + x0 * 4 + c] as u32;
                let e = source[y1 * pitch + x1 * 4 + c] as u32;
                scaled[(y * ow + x) * 4 + c] = (((a * (256 - fx) + b * fx) * (256 - fy)
                    + (d * (256 - fx) + e * fx) * fy
                    + 32768)
                    >> 16) as u8;
            }
        }
    }
    let mut nv12 = vec![0; ow * oh * 3 / 2];
    for y in 0..oh {
        for x in 0..ow {
            let i = (y * ow + x) * 4;
            let (b, g, r) = (scaled[i] as i32, scaled[i + 1] as i32, scaled[i + 2] as i32);
            nv12[y * ow + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(16, 235) as u8;
        }
    }
    for y in (0..oh).step_by(2) {
        for x in (0..ow).step_by(2) {
            let mut rgb = [0i32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    for c in 0..3 {
                        rgb[c] += scaled[((y + dy) * ow + x + dx) * 4 + c] as i32;
                    }
                }
            }
            let (b, g, r) = ((rgb[0] + 2) / 4, (rgb[1] + 2) / 4, (rgb[2] + 2) / 4);
            let i = ow * oh + y / 2 * ow + x;
            nv12[i] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
            nv12[i + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
        }
    }
    Ok(nv12)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_black_white_and_red_vectors() {
        for (pixel, expected) in [
            ([0, 0, 0, 255], [16, 128, 128]),
            ([255, 255, 255, 255], [235, 128, 128]),
            ([0, 0, 255, 255], [82, 90, 240]),
        ] {
            let out = bgra_to_nv12(&pixel.repeat(4), 2, 2, 8, 2, 2).unwrap();
            assert_eq!(&out[..4], &[expected[0]; 4]);
            assert_eq!(&out[4..], &expected[1..]);
        }
    }
    #[test]
    fn resize_respects_stride_and_uv_order() {
        let mut src = vec![99; 4 * 24];
        for y in 0..4 {
            for x in 0..4 {
                src[y * 24 + x * 4..y * 24 + x * 4 + 4].copy_from_slice(&[255, 0, 0, 0]);
            }
        }
        let out = bgra_to_nv12(&src, 4, 4, 24, 2, 2).unwrap();
        assert_eq!(out, [41, 41, 41, 41, 240, 110]);
    }
    #[test]
    fn rejects_truncation_odd_output_and_overflow() {
        assert!(bgra_to_nv12(&[], 2, 2, 8, 2, 2).is_err());
        assert!(bgra_to_nv12(&[0; 16], 2, 2, 4, 2, 2).is_err());
        assert!(bgra_to_nv12(&[0; 16], 2, 2, 8, 3, 2).is_err());
        assert!(bgra_to_nv12(&[], 2, 2, usize::MAX, 2, 2).is_err());
    }
}

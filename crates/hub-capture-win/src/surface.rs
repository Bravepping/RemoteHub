//! Exact byte layouts accepted from DXGI desktop duplication. This module is
//! platform-neutral for synthetic tests. Only the documented FP16 scRGB
//! desktop composition representation has an HDR conversion here.

use crate::{pack_rows, CaptureError};

// Numeric DXGI_FORMAT values are shared by the SDK and tested off Windows.
// See https://learn.microsoft.com/windows/win32/api/dxgiformat/ne-dxgiformat-dxgi_format
const BGRA8_UNORM: u32 = 87;
const BGRA8_TYPELESS: u32 = 90;
const BGRA8_SRGB: u32 = 91;
const BGRX8_UNORM: u32 = 88;
const BGRX8_TYPELESS: u32 = 92;
const BGRX8_SRGB: u32 = 93;
const RGBA8_TYPELESS: u32 = 27;
const RGBA8_UNORM: u32 = 28;
const RGBA8_SRGB: u32 = 29;
const SCRGB16_FLOAT: u32 = 10;
const MAX_SCRGB_PIXELS: usize = 16 * 1024 * 1024;
const SRGB_STEPS: usize = 4096;

/// CopyResource needs the original DXGI layout. A same-size mode change
/// must not reuse the old staging texture, even for formats with equal bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SurfaceKey {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) format: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PixelLayout {
    Bgra,
    Bgrx,
    Rgba,
    ScRgb16,
}

impl PixelLayout {
    pub(super) fn classify(format: u32) -> Result<Self, CaptureError> {
        match format {
            BGRA8_UNORM | BGRA8_TYPELESS | BGRA8_SRGB => Ok(Self::Bgra),
            BGRX8_UNORM | BGRX8_TYPELESS | BGRX8_SRGB => Ok(Self::Bgrx),
            RGBA8_UNORM | RGBA8_TYPELESS | RGBA8_SRGB => Ok(Self::Rgba),
            SCRGB16_FLOAT => Ok(Self::ScRgb16),
            _ => Err(CaptureError::FrameFormat(format)),
        }
    }

    pub(super) fn row_bytes(self, width: u32, height: u32) -> Result<usize, CaptureError> {
        const GEOMETRY: CaptureError = CaptureError::DxgiState("FRAME_GEOMETRY");
        if !(1..=16_384).contains(&width) || !(1..=16_384).contains(&height) {
            return Err(GEOMETRY);
        }
        let fp16 = self == Self::ScRgb16;
        if fp16 && width as usize * height as usize > MAX_SCRGB_PIXELS {
            return Err(GEOMETRY);
        }
        Ok(width as usize * if fp16 { 8 } else { 4 })
    }

    /// Exact consumed bytes in the mapped input. FP16 uses eight source
    /// bytes per pixel and never interprets padding as another pixel.
    pub(super) fn mapped_len(
        self,
        width: u32,
        height: u32,
        pitch: usize,
    ) -> Result<usize, CaptureError> {
        const GEOMETRY: CaptureError = CaptureError::DxgiState("FRAME_GEOMETRY");
        let row_bytes = self.row_bytes(width, height)?;
        let height = height as usize;
        let fp16 = self == Self::ScRgb16;
        if pitch < row_bytes {
            return Err(GEOMETRY);
        }
        let len = if fp16 {
            (height - 1)
                .checked_mul(pitch)
                .and_then(|before| before.checked_add(row_bytes))
        } else {
            height.checked_mul(pitch)
        };
        len.filter(|size| *size <= isize::MAX as usize)
            .ok_or(GEOMETRY)
    }

    pub(super) fn pack_bgra(
        self,
        bytes: &[u8],
        width: u32,
        height: u32,
        pitch: usize,
    ) -> Result<Vec<u8>, CaptureError> {
        if self == Self::ScRgb16 {
            return pack_scrgb16(bytes, width, height, pitch);
        }
        let mut pixels = pack_rows(bytes, width, height, pitch)?;
        match self {
            Self::Bgra => (),
            Self::Bgrx => pixels.chunks_exact_mut(4).for_each(|p| p[3] = u8::MAX),
            Self::Rgba => pixels.chunks_exact_mut(4).for_each(|p| p.swap(0, 2)),
            Self::ScRgb16 => unreachable!(),
        }
        Ok(pixels)
    }
}

/// Decode the positive finite IEEE 754 binary16 subset. Negative scRGB can
/// encode colors beyond the SDR gamut; this bounded conversion clips that
/// channel to black. NaN/±Inf are invalid samples and are also black, so
/// they cannot wash out the other channels or poison the transfer math.
fn positive_half(bytes: [u8; 2]) -> f32 {
    let value = u16::from_le_bytes(bytes);
    match value {
        0..=0x03ff => f32::from(value) * (1.0 / 16_777_216.0),
        0x0400..=0x7bff => f32::from_bits((u32::from(value) << 13) + 0x3800_0000),
        _ => 0.0,
    }
}

fn srgb_lut() -> &'static [u8; SRGB_STEPS + 1] {
    static LUT: std::sync::OnceLock<[u8; SRGB_STEPS + 1]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|index| {
            let linear = index as f32 / SRGB_STEPS as f32;
            let srgb = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            (srgb * 255.0).round() as u8
        })
    })
}

fn to_srgb8(linear: f32, lut: &[u8; SRGB_STEPS + 1]) -> u8 {
    let index = (linear * SRGB_STEPS as f32 + 0.5) as usize;
    lut[index.min(SRGB_STEPS)]
}

fn pack_scrgb16(
    bytes: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
) -> Result<Vec<u8>, CaptureError> {
    let required = PixelLayout::ScRgb16.mapped_len(width, height, pitch)?;
    let bytes = bytes
        .get(..required)
        .ok_or(CaptureError::DxgiState("FRAME_GEOMETRY"))?;
    let width = width as usize;
    let output_len = width * height as usize * 4;
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_len)
        .map_err(|_| CaptureError::DxgiState("FRAME_ALLOCATION"))?;
    output.resize(output_len, 0);
    let lut = srgb_lut();
    for (y, dst) in output.chunks_exact_mut(width * 4).enumerate() {
        let row = &bytes[y * pitch..y * pitch + width * 8];
        for (src, out) in row.chunks_exact(8).zip(dst.chunks_exact_mut(4)) {
            let r = positive_half([src[0], src[1]]);
            let g = positive_half([src[2], src[3]]);
            let b = positive_half([src[4], src[5]]);
            let peak = r.max(g).max(b);
            // Preserve ordinary linear midtones. Above the knee, compress
            // the peak smoothly and apply the same scale to all channels;
            // unlike per-channel clipping this keeps their linear ratios.
            const KNEE: f32 = 0.9;
            const HEADROOM: f32 = 1.0 - KNEE;
            let scale = if peak <= KNEE {
                1.0
            } else {
                (1.0 - HEADROOM / (1.0 + (peak - KNEE) / HEADROOM)) / peak
            };
            out.copy_from_slice(&[
                to_srgb8(b * scale, lut),
                to_srgb8(g * scale, lut),
                to_srgb8(r * scale, lut),
                255, // desktop composition; captured alpha is not a mask
            ]);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_and_bgrx_preserve_colors_and_strip_row_padding() {
        let source = [
            11, 22, 33, 0, 99, 99, 99, 99, 44, 55, 66, 12, 99, 99, 99, 99,
        ];
        for format in [BGRA8_UNORM, BGRA8_TYPELESS, BGRA8_SRGB] {
            let layout = PixelLayout::classify(format).unwrap();
            assert_eq!(
                layout.pack_bgra(&source, 1, 2, 8).unwrap(),
                [11, 22, 33, 0, 44, 55, 66, 12]
            );
        }
        for format in [BGRX8_UNORM, BGRX8_TYPELESS, BGRX8_SRGB] {
            let layout = PixelLayout::classify(format).unwrap();
            assert_eq!(
                layout.pack_bgra(&source, 1, 2, 8).unwrap(),
                [11, 22, 33, 255, 44, 55, 66, 255]
            );
        }
    }

    #[test]
    fn rgba_swaps_only_red_and_blue_and_respects_stride() {
        let source = [231, 51, 19, 128, 71, 91, 11, 255, 0, 0, 0, 0];
        for format in [RGBA8_UNORM, RGBA8_TYPELESS, RGBA8_SRGB] {
            let layout = PixelLayout::classify(format).unwrap();
            assert_eq!(
                layout.pack_bgra(&source, 2, 1, 12).unwrap(),
                [19, 51, 231, 128, 11, 91, 71, 255]
            );
            assert!(layout.pack_bgra(&source[..7], 2, 1, 8).is_err());
        }
    }

    #[test]
    fn rejects_hdr_integer_other_and_unknown_pixel_formats_by_numeric_code() {
        for format in [9, 11, 12, 13, 14, 23, 24, 25, 30, 31, 32, 89, 0, u32::MAX] {
            assert!(
                matches!(PixelLayout::classify(format), Err(CaptureError::FrameFormat(code)) if code == format)
            );
        }
    }

    fn samples(values: &[[u16; 4]]) -> Vec<u8> {
        values
            .iter()
            .flatten()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn scrgb16_rows_red_gray_alpha_and_padding() {
        let layout = PixelLayout::classify(10).unwrap();
        let mut bytes = samples(&[[0x3800, 0, 0, 0], [0x3800, 0x3800, 0x3800, 0x7e00]]);
        bytes.extend_from_slice(&[255; 4]);
        bytes.extend(samples(&[
            [0, 0, 0x3800, 0x7c00],
            [0x3400, 0x3400, 0x3400, 0x8000],
        ]));
        assert_eq!(
            layout.pack_bgra(&bytes, 2, 2, 20).unwrap(),
            [0, 0, 188, 255, 188, 188, 188, 255, 188, 0, 0, 255, 137, 137, 137, 255,]
        );
        assert!(layout
            .pack_bgra(&bytes[..bytes.len() - 1], 2, 2, 20)
            .is_err());
        assert!(layout.pack_bgra(&bytes, 2, 2, 15).is_err());
        assert!(layout.mapped_len(8192, 8192, 65536).is_err());
        assert!(layout.mapped_len(1, 2, usize::MAX).is_err());
    }

    #[test]
    fn scrgb16_highlights_remain_monotonic_and_nonfinite_never_poison_other_channels() {
        let layout = PixelLayout::classify(10).unwrap();
        let bytes = samples(&[
            [0x3c00, 0x3c00, 0x3c00, 0], // 1.0
            [0x4000, 0x4000, 0x4000, 0], // 2.0
            [0x4600, 0x4600, 0x4600, 0], // 6.0
            [0x4000, 0x3c00, 0x3800, 0], // R > G > B
            [0x7c00, 0x3800, 0x7e00, 0xfc00],
            [0xbc00, 0xfc00, 0x0001, 0],
        ]);
        let bgra = layout.pack_bgra(&bytes, 6, 1, 48).unwrap();
        assert!(bgra[0] < bgra[4] && bgra[4] <= bgra[8]);
        for gray in bgra[..12].chunks_exact(4) {
            assert_eq!(gray, [gray[0], gray[0], gray[0], 255]);
        }
        assert!(bgra[12] < bgra[13] && bgra[13] < bgra[14]);
        assert_eq!(&bgra[16..], [0, 188, 0, 255, 0, 0, 0, 255]);
        assert_eq!(positive_half([1, 0]), 1.0 / 16_777_216.0);
        assert_eq!(positive_half([0xff, 0x7b]), 65_504.0);
    }

    #[test]
    fn staging_key_changes_on_exact_dxgi_format_or_dimensions() {
        let first = SurfaceKey {
            width: 1920,
            height: 1080,
            format: BGRA8_UNORM,
        };
        assert_ne!(
            first,
            SurfaceKey {
                format: BGRA8_SRGB,
                ..first
            }
        );
        assert_ne!(
            first,
            SurfaceKey {
                format: RGBA8_UNORM,
                ..first
            }
        );
        assert_ne!(
            first,
            SurfaceKey {
                width: 1280,
                ..first
            }
        );
        assert_ne!(
            first,
            SurfaceKey {
                height: 720,
                ..first
            }
        );
        assert_eq!(first, SurfaceKey { ..first });
    }
}

//! Exact byte layouts accepted from DXGI desktop duplication. This module is
//! platform-neutral for synthetic tests; it never interprets HDR samples.

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
}

impl PixelLayout {
    pub(super) fn classify(format: u32) -> Result<Self, CaptureError> {
        match format {
            BGRA8_UNORM | BGRA8_TYPELESS | BGRA8_SRGB => Ok(Self::Bgra),
            BGRX8_UNORM | BGRX8_TYPELESS | BGRX8_SRGB => Ok(Self::Bgrx),
            RGBA8_UNORM | RGBA8_TYPELESS | RGBA8_SRGB => Ok(Self::Rgba),
            _ => Err(CaptureError::FrameFormat(format)),
        }
    }

    pub(super) fn pack_bgra(
        self,
        bytes: &[u8],
        width: u32,
        height: u32,
        pitch: usize,
    ) -> Result<Vec<u8>, CaptureError> {
        let mut pixels = pack_rows(bytes, width, height, pitch)?;
        match self {
            Self::Bgra => (),
            Self::Bgrx => pixels.chunks_exact_mut(4).for_each(|p| p[3] = u8::MAX),
            Self::Rgba => pixels.chunks_exact_mut(4).for_each(|p| p.swap(0, 2)),
        }
        Ok(pixels)
    }
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
        for format in [10, 24, 25, 30, 31, 32, 0, u32::MAX] {
            assert!(
                matches!(PixelLayout::classify(format), Err(CaptureError::FrameFormat(code)) if code == format)
            );
        }
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

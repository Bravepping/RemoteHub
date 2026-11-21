use serde::Serialize;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;
use thiserror::Error;

#[cfg(windows)]
mod dxgi;
#[cfg(windows)]
pub use dxgi::DxgiCapture;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("CONSENT_REQUIRED: 本机采集实验需要明确同意")]
    ConsentRequired,
    #[error("INVALID_ARGUMENT: {0}")]
    InvalidArgument(&'static str),
    #[error("CAPTURE_UNAVAILABLE: {0}")]
    Unavailable(String),
    #[error("CAPTURE_ACCESS_LOST: 桌面或分辨率已改变，请重新运行实验")]
    AccessLost,
    #[error("CAPTURE_TIMEOUT: 时间范围内没有收到桌面帧")]
    NoFrames,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub timestamp_us: u64,
    pub bgra: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct ProbeReport {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub timeouts: u32,
    pub elapsed_ms: u64,
    pub observed_fps: f64,
    pub frame_timestamps_us: Vec<u64>,
    pub adapter: String,
}

pub struct ProbeResult {
    pub report: ProbeReport,
    pub first_frame: CapturedFrame,
}

/// Permission is per local experiment. This API must not be used as remote session authorization.
pub fn run_probe(
    consent: bool,
    max_frames: u32,
    budget: Duration,
) -> Result<ProbeResult, CaptureError> {
    validate_probe(consent, max_frames, budget)?;
    #[cfg(windows)]
    {
        let start = Instant::now();
        let mut capture = DxgiCapture::new()?;
        let mut first_frame = None;
        let mut timestamps = Vec::new();
        let mut timeouts = 0;
        while timestamps.len() < max_frames as usize && start.elapsed() < budget {
            let tick = Instant::now();
            match capture.next_frame(100)? {
                Some(frame) => {
                    timestamps.push(frame.timestamp_us);
                    if first_frame.is_none() {
                        first_frame = Some(frame);
                    }
                }
                None => timeouts += 1,
            }
            // Bound CPU load and request at most 30 updates/s; duplication can return fewer.
            if let Some(remaining) = Duration::from_micros(33_333).checked_sub(tick.elapsed()) {
                std::thread::sleep(remaining);
            }
        }
        let frame = first_frame.ok_or(CaptureError::NoFrames)?;
        let elapsed = start.elapsed();
        Ok(ProbeResult {
            report: ProbeReport {
                width: frame.width,
                height: frame.height,
                frames: timestamps.len() as u32,
                timeouts,
                elapsed_ms: elapsed.as_millis() as u64,
                observed_fps: timestamps.len() as f64 / elapsed.as_secs_f64(),
                frame_timestamps_us: timestamps,
                adapter: capture.adapter_name().to_owned(),
            },
            first_frame: frame,
        })
    }
    #[cfg(not(windows))]
    Err(CaptureError::Unavailable("DXGI requires Windows".into()))
}

pub fn validate_probe(consent: bool, frames: u32, budget: Duration) -> Result<(), CaptureError> {
    if !consent {
        return Err(CaptureError::ConsentRequired);
    }
    if !(1..=300).contains(&frames) || budget.is_zero() || budget > Duration::from_secs(15) {
        return Err(CaptureError::InvalidArgument(
            "frames must be 1..300, duration 1ns..15s",
        ));
    }
    Ok(())
}

/// Packs BGRA rows after validating both the GPU stride and mapped-buffer length.
pub fn pack_rows(
    source: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
) -> Result<Vec<u8>, CaptureError> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(CaptureError::InvalidArgument(
            "unsupported frame dimensions",
        ));
    }
    let row = width as usize * 4;
    let required = pitch
        .checked_mul(height as usize)
        .ok_or(CaptureError::InvalidArgument("frame buffer overflow"))?;
    if pitch < row || source.len() < required {
        return Err(CaptureError::InvalidArgument(
            "invalid frame stride or buffer length",
        ));
    }
    let mut packed = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        packed.extend_from_slice(&source[y * pitch..y * pitch + row]);
    }
    Ok(packed)
}

pub fn write_bmp(frame: &CapturedFrame, path: &std::path::Path) -> Result<(), CaptureError> {
    use std::io::Write;
    if frame.width == 0 || frame.height == 0 || frame.width > 16384 || frame.height > 16384 {
        return Err(CaptureError::InvalidArgument("unsupported BMP dimensions"));
    }
    let expected = frame.width as usize * frame.height as usize * 4;
    if frame.bgra.len() != expected || frame.width == 0 || frame.height == 0 {
        return Err(CaptureError::InvalidArgument("invalid BGRA frame"));
    }
    let mut file = std::fs::File::create_new(path)?;
    let mut header = [0u8; 54];
    header[0..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&((54 + expected) as u32).to_le_bytes());
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&(frame.width as i32).to_le_bytes());
    header[22..26].copy_from_slice(&(-(frame.height as i32)).to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&32u16.to_le_bytes());
    header[34..38].copy_from_slice(&(expected as u32).to_le_bytes());
    file.write_all(&header)?;
    file.write_all(&frame.bgra)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_capture_before_native_api_without_consent() {
        assert!(matches!(
            run_probe(false, 1, Duration::from_secs(1)),
            Err(CaptureError::ConsentRequired)
        ));
    }

    #[test]
    fn rejects_unbounded_capture() {
        for count in [0, 301, u32::MAX] {
            assert!(validate_probe(true, count, Duration::from_secs(1)).is_err());
        }
        for budget in [Duration::ZERO, Duration::from_secs(16)] {
            assert!(validate_probe(true, 1, budget).is_err());
        }
        assert!(validate_probe(true, 300, Duration::from_secs(15)).is_ok());
    }

    #[test]
    fn removes_gpu_row_padding_without_mixing_rows() {
        let input = [1, 2, 3, 4, 99, 99, 99, 99, 5, 6, 7, 8, 88, 88, 88, 88];
        assert_eq!(
            pack_rows(&input, 1, 2, 8).unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn rejects_short_buffers_bad_stride_and_dimensions() {
        assert!(pack_rows(&[0; 8], 2, 2, 8).is_err());
        assert!(pack_rows(&[0; 16], 2, 2, 4).is_err());
        assert!(pack_rows(&[], 0, 2, 8).is_err());
        assert!(pack_rows(&[], u32::MAX, 2, 8).is_err());
        assert!(pack_rows(&[], 1, 2, usize::MAX).is_err());
    }

    #[test]
    fn bmp_is_top_down_bgra_and_never_overwrites() {
        let path = std::env::temp_dir().join(format!(
            "remotehub-bmp-{}-{}.bmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let frame = CapturedFrame {
            width: 1,
            height: 1,
            timestamp_us: 0,
            bgra: vec![1, 2, 3, 255],
        };
        write_bmp(&frame, &path).unwrap();
        assert!(write_bmp(&frame, &path).is_err());
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[0..2], b"BM");
        assert_eq!(&bytes[22..26], &(-1i32).to_le_bytes());
        assert_eq!(&bytes[54..], &[1, 2, 3, 255]);
    }

    #[test]
    fn bmp_rejects_oversized_and_incomplete_frames() {
        let path = std::env::temp_dir().join("remotehub-invalid-frame.bmp");
        let frame = CapturedFrame {
            width: u32::MAX,
            height: u32::MAX,
            timestamp_us: 0,
            bgra: vec![],
        };
        assert!(write_bmp(&frame, &path).is_err());
        let frame = CapturedFrame {
            width: 1,
            height: 1,
            timestamp_us: 0,
            bgra: vec![1],
        };
        assert!(write_bmp(&frame, &path).is_err());
    }
}

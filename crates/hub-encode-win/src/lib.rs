pub mod color;
pub mod h264;
pub mod lifecycle;
#[cfg(windows)]
pub mod metrics;
#[cfg(windows)]
pub mod mf;
#[cfg(windows)]
pub mod probe;
#[cfg(windows)]
pub mod stream;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("CONSENT_REQUIRED: local desktop encoding requires explicit consent")]
    ConsentRequired,
    #[error("INVALID_ARGUMENT: {0}")]
    InvalidArgument(&'static str),
    #[error("ENCODER_UNSUPPORTED: {0}")]
    Unsupported(String),
    #[error("ENCODER_FAILED: {0}")]
    Native(String),
    #[error("ENCODER_TIMEOUT: {0}")]
    Timeout(&'static str),
    #[error("ENCODER_BUSY: another local encoding probe is active")]
    Busy,
    #[error("INVALID_H264: {0}")]
    Bitstream(&'static str),
    #[error(transparent)]
    Capture(#[from] hub_capture_win::CaptureError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[cfg(windows)]
    #[error("MEDIA_FOUNDATION: {0}")]
    Windows(#[from] windows::core::Error),
}

#[derive(Clone, Copy, Debug)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub seconds: u32,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30,
            bitrate: 4_000_000,
            seconds: 10,
        }
    }
}

impl EncoderConfig {
    pub fn validate(&self, consent: bool) -> Result<(), EncodeError> {
        if !consent {
            return Err(EncodeError::ConsentRequired);
        }
        if self.width != 1280
            || self.height != 720
            || self.fps != 30
            || !(100_000..=20_000_000).contains(&self.bitrate)
            || !(1..=30).contains(&self.seconds)
        {
            return Err(EncodeError::InvalidArgument(
                "probe requires 1280x720@30, 100kbps..20Mbps, 1..30 seconds",
            ));
        }
        Ok(())
    }

    pub fn pts(&self, tick: u64) -> i64 {
        (tick * 10_000_000 / self.fps as u64) as i64
    }
    pub fn duration(&self, tick: u64) -> i64 {
        self.pts(tick + 1) - self.pts(tick)
    }

    /// Tolerate at most one late video tick; skip fresh desktop conversion when over half a tick late.
    pub fn schedule(&self, tick: u64, elapsed: std::time::Duration) -> (u64, u32, bool) {
        let due = (elapsed.as_nanos() * self.fps as u128 / 1_000_000_000) as u64;
        let skipped = if due > tick + 1 { due - tick } else { 0 };
        let selected = tick + skipped;
        let target = selected as u128 * 1_000_000_000 / self.fps as u128;
        let late = elapsed.as_nanos().saturating_sub(target);
        (
            selected,
            skipped as u32,
            skipped > 0 || late > 1_000_000_000 / self.fps as u128 / 2,
        )
    }
}

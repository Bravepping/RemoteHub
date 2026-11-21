use crate::{color::bgra_to_nv12, lifecycle::StopToken, EncodeError, EncoderConfig};
use hub_capture_win::{CaptureError, CapturedFrame, DxgiCapture};
use std::time::{Duration, Instant};

pub(crate) struct DesktopCapture {
    capture: DxgiCapture,
    pub(crate) last_frame: CapturedFrame,
    pub(crate) nv12: Vec<u8>,
}

impl DesktopCapture {
    pub(crate) fn start(config: &EncoderConfig, stop: &StopToken) -> Result<Self, EncodeError> {
        let mut capture = DxgiCapture::new()?;
        let first_deadline = Instant::now() + Duration::from_secs(3);
        let last_frame = loop {
            if stop.is_stopped() {
                return Err(EncodeError::Cancelled);
            }
            if let Some(frame) = capture.next_frame(100)? {
                break frame;
            }
            if Instant::now() >= first_deadline {
                return Err(EncodeError::Timeout("first DXGI frame"));
            }
        };
        let nv12 = convert(&last_frame, config)?;
        Ok(Self {
            capture,
            last_frame,
            nv12,
        })
    }

    pub(crate) fn poll(&mut self) -> Result<Option<CapturedFrame>, EncodeError> {
        checked_capture(
            || self.capture.next_frame(0),
            self.last_frame.width,
            self.last_frame.height,
        )
    }

    pub(crate) fn replace(
        &mut self,
        frame: CapturedFrame,
        config: &EncoderConfig,
    ) -> Result<(), EncodeError> {
        self.nv12 = convert(&frame, config)?;
        self.last_frame = frame;
        Ok(())
    }

    pub(crate) fn adapter_name(&self) -> &str {
        self.capture.adapter_name()
    }
}

fn convert(frame: &CapturedFrame, config: &EncoderConfig) -> Result<Vec<u8>, EncodeError> {
    bgra_to_nv12(
        &frame.bgra,
        frame.width,
        frame.height,
        frame.width as usize * 4,
        config.width,
        config.height,
    )
}

pub(crate) fn checked_capture(
    fetch: impl FnOnce() -> Result<Option<CapturedFrame>, CaptureError>,
    width: u32,
    height: u32,
) -> Result<Option<CapturedFrame>, EncodeError> {
    let frame = fetch()?;
    if frame
        .as_ref()
        .is_some_and(|frame| frame.width != width || frame.height != height)
    {
        return Err(EncodeError::Native(
            "display dimensions changed; restart probe".into(),
        ));
    }
    Ok(frame)
}

pub(crate) fn pace_tick(
    config: &EncoderConfig,
    tick: u64,
    start: Instant,
    stop: &StopToken,
) -> Option<(u64, u32, bool)> {
    let target = Duration::from_nanos(tick * 1_000_000_000 / config.fps as u64);
    while !stop.is_stopped() {
        let elapsed = start.elapsed();
        if elapsed >= target {
            return Some(config.schedule(tick, elapsed));
        }
        std::thread::sleep((target - elapsed).min(Duration::from_millis(5)));
    }
    None
}

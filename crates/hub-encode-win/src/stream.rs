//! Windows-only, in-memory DXGI -> NV12 -> MF producer. No network or file I/O.
use crate::{
    desktop::{pace_tick, DesktopCapture},
    h264::EncodedVideoFrame,
    lifecycle::{ProcessLease, StopToken, GATE},
    mf::{self, MfRuntime},
    stream_output::Output,
    EncodeError, EncoderConfig,
};
use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize)]
pub struct StreamInfo {
    pub encoder: String,
    pub hardware: bool,
    pub source_width: u32,
    pub source_height: u32,
    pub setup_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StreamReport {
    pub info: StreamInfo,
    pub submitted_frames: u32,
    pub encoded_frames: u32,
    pub encoded_bytes: u64,
    pub missed_ticks: u32,
    pub repeated_frames: u32,
    pub elapsed_ms: f64,
    pub stopped: bool,
}

/// Run synchronously on a dedicated blocking OS thread. COM, DXGI and MF state
/// is created, used and dropped on that thread. Only owned AU data leaves it.
///
/// The caller is responsible for a non-blocking, bounded sink (e.g. `try_send`
/// on a 2–4 AU queue). Return `EncodeError::Backpressure` on a full queue.
/// Return an error when the consumer disappears, or stop the shared token.
/// Callbacks must not block: in-flight system/driver calls themselves cannot be
/// preempted. No raw video bytes are written to storage or logs.
pub fn run(
    consent: bool,
    config: EncoderConfig,
    stop: StopToken,
    mut on_ready: impl FnMut(StreamInfo),
    mut on_frame: impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<StreamReport, EncodeError> {
    config.validate_stream(consent)?;
    let _lease = GATE.enter(consent, &config)?;
    if stop.is_stopped() {
        return Err(EncodeError::Cancelled);
    }
    let setup_start = Instant::now();
    let _process_lease = ProcessLease::acquire()?;
    let runtime = MfRuntime::new()?;
    let mut capture = DesktopCapture::start(&config, &stop)?;
    let setup_limit = Duration::from_secs(12);
    let encoder = mf::select_until(&runtime, &config, "auto", &capture.nv12, || {
        stop.is_stopped() || setup_start.elapsed() >= setup_limit
    });
    if stop.is_stopped() {
        return Err(EncodeError::Cancelled);
    }
    if setup_start.elapsed() >= setup_limit {
        return Err(EncodeError::Timeout("live MF setup"));
    }
    let mut encoder = encoder?;
    let info = StreamInfo {
        encoder: encoder.name.clone(),
        hardware: encoder.hardware,
        source_width: capture.last_frame.width,
        source_height: capture.last_frame.height,
        setup_ms: setup_start.elapsed().as_secs_f64() * 1_000.0,
    };
    on_ready(info.clone());

    let start = Instant::now();
    let tick_limit = u64::from(config.seconds * config.fps);
    let mut tick = 0;
    let mut missed_ticks = 0;
    let mut repeated_frames = 0;
    let mut submitted_frames = 0;
    let mut output = Output::default();
    while tick < tick_limit {
        let Some((selected, _, reuse)) = pace_tick(&config, tick, start, &stop) else {
            break;
        };
        // Missed source ticks are not encoded at all; no dependent encoded AU is
        // silently dropped, and the next submitted AU retains real PTS.
        missed_ticks += (selected.min(tick_limit) - tick) as u32;
        tick = selected;
        if tick >= tick_limit {
            break;
        }
        let update = if reuse { None } else { capture.poll()? };
        if let Some(frame) = update {
            capture.replace(frame, &config)?;
        } else {
            repeated_frames += 1;
        }
        let frames = encoder.encode_until(
            &capture.nv12,
            config.pts(tick),
            config.duration(tick),
            || stop.is_stopped(),
        );
        let frames = match frames {
            Err(EncodeError::Cancelled) => break,
            result => result?,
        };
        submitted_frames += 1;
        output.publish(frames, &stop, &mut on_frame)?;
        tick += 1;
    }
    if !stop.is_stopped() {
        match encoder.drain_until(|| stop.is_stopped()) {
            Ok(frames) => output.publish(frames, &stop, &mut on_frame)?,
            Err(EncodeError::Cancelled) => (),
            Err(error) => return Err(error),
        }
    }
    if output.frames == 0 && !stop.is_stopped() {
        return Err(EncodeError::Native("no live H.264 frames produced".into()));
    }
    Ok(StreamReport {
        info,
        submitted_frames,
        encoded_frames: output.frames,
        encoded_bytes: output.bytes,
        missed_ticks,
        repeated_frames,
        elapsed_ms: start.elapsed().as_secs_f64() * 1_000.0,
        stopped: stop.is_stopped(),
    })
}

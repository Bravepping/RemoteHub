//! Live AU producer. All COM/DXGI/MF objects stay on the calling Windows thread.
//! No sample files, network handles, Tokio tasks or frontend pixels are owned here.
use crate::{
    color::bgra_to_nv12,
    h264::EncodedVideoFrame,
    lifecycle::StopToken,
    mf::{self, MfRuntime},
    probe::ProcessLease,
    EncodeError, EncoderConfig,
};
use hub_capture_win::DxgiCapture;
use serde::Serialize;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Clone, Default, Serialize)]
pub struct StreamReport {
    pub encoder: String,
    pub hardware: bool,
    pub source_width: u32,
    pub source_height: u32,
    pub frames_input: u32,
    pub frames_encoded: u32,
    pub bytes_encoded: u64,
    pub missed_ticks: u32,
    pub desktop_updates: u32,
    pub reused_frames: u32,
    pub idr_frames: u32,
    pub keyframe_requests: u32,
    pub keyframe_rejections: u32,
    pub first_pts_100ns: Option<i64>,
    pub last_pts_100ns: Option<i64>,
    pub setup_ms: f64,
    pub media_wall_ms: f64,
    pub stopped: bool,
}

const STANDARD_OUTPUT_FRAMES: u32 = 900;

pub fn run(
    consent: bool,
    config: EncoderConfig,
    stop: StopToken,
    keyframe: &AtomicBool,
    publish: impl FnMut(&StreamReport),
    sink: impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<StreamReport, EncodeError> {
    config.validate(consent)?;
    run_validated(
        config,
        config.seconds * config.fps,
        STANDARD_OUTPUT_FRAMES,
        stop,
        keyframe,
        publish,
        sink,
    )
}

/// Fixed-length experiment entry point for the approved native public-video
/// host. It does not change the limits of ordinary callers or probes.
#[cfg(feature = "v2-public-video-60s-test")]
pub fn run_public_video_60s_test(
    consent: bool,
    stop: StopToken,
    publish: impl FnMut(&StreamReport),
    sink: impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<StreamReport, EncodeError> {
    let (config, ticks) = crate::public_video_test_config(consent)?;
    run_validated(
        config,
        ticks,
        ticks,
        stop,
        &AtomicBool::new(false),
        publish,
        sink,
    )
}

fn run_validated(
    config: EncoderConfig,
    max_ticks: u32,
    max_frames: u32,
    stop: StopToken,
    keyframe: &AtomicBool,
    mut publish: impl FnMut(&StreamReport),
    mut sink: impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<StreamReport, EncodeError> {
    let setup = Instant::now();
    let _exclusive = ProcessLease::acquire()?;
    let runtime = MfRuntime::new()?;
    let mut capture = DxgiCapture::new()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let first = loop {
        if stop.is_stopped() {
            return Err(EncodeError::Native("CANCELLED_DURING_SETUP".into()));
        }
        if let Some(frame) = capture.next_frame(100)? {
            break frame;
        }
        if Instant::now() >= deadline {
            return Err(EncodeError::Timeout("live first DXGI frame"));
        }
    };
    let mut nv12 = bgra_to_nv12(
        &first.bgra,
        first.width,
        first.height,
        first.width as usize * 4,
        config.width,
        config.height,
    )?;
    let (mut encoder, _) = mf::select(&runtime, &config, "auto", &nv12)?;
    let mut report = StreamReport {
        encoder: encoder.name.clone(),
        hardware: encoder.hardware,
        source_width: first.width,
        source_height: first.height,
        desktop_updates: 1,
        setup_ms: setup.elapsed().as_secs_f64() * 1000.0,
        ..Default::default()
    };
    let start = Instant::now();
    publish(&report);
    let mut tick = 0;
    while tick < u64::from(max_ticks) && !stop.is_stopped() {
        let target = Duration::from_nanos(tick * 1_000_000_000 / config.fps as u64);
        while start.elapsed() < target && !stop.is_stopped() {
            std::thread::sleep(
                target
                    .saturating_sub(start.elapsed())
                    .min(Duration::from_millis(5)),
            );
        }
        if stop.is_stopped() {
            break;
        }
        let (selected, missed, reuse) = config.schedule(tick, start.elapsed());
        tick = selected;
        report.missed_ticks += missed;
        if tick >= u64::from(max_ticks) {
            break;
        }
        let update = if reuse { None } else { capture.next_frame(0)? };
        if let Some(frame) = update {
            if frame.width != report.source_width || frame.height != report.source_height {
                return Err(EncodeError::Native("LIVE_DISPLAY_RESIZED".into()));
            }
            nv12 = bgra_to_nv12(
                &frame.bgra,
                frame.width,
                frame.height,
                frame.width as usize * 4,
                config.width,
                config.height,
            )?;
            report.desktop_updates += 1;
        } else {
            report.reused_frames += 1;
        }
        if keyframe.swap(false, Ordering::AcqRel) {
            report.keyframe_requests += 1;
            if encoder.request_keyframe().is_err() {
                report.keyframe_rejections += 1;
            }
        }
        report.frames_input += 1;
        let frames = encoder.encode(&nv12, config.pts(tick), config.duration(tick))?;
        for frame in frames {
            if stop.is_stopped() {
                break;
            }
            if let Err(error) = deliver(frame, &mut report, max_frames, &mut sink) {
                publish(&report);
                return Err(error);
            }
        }
        report.media_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        publish(&report);
        tick += 1;
    }
    if !stop.is_stopped() {
        for frame in encoder.drain()? {
            if let Err(error) = deliver(frame, &mut report, max_frames, &mut sink) {
                publish(&report);
                return Err(error);
            }
        }
    }
    // Cancellation deliberately flushes queued references by dropping the whole stream, not resuming P frames.
    report.stopped = stop.is_stopped();
    report.media_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
    publish(&report);
    Ok(report)
}

fn deliver(
    frame: EncodedVideoFrame,
    report: &mut StreamReport,
    max_frames: u32,
    sink: &mut impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    if report.frames_encoded >= max_frames
        || report.bytes_encoded + frame.annex_b.len() as u64 > 128 * 1024 * 1024
    {
        return Err(EncodeError::Native("LIVE_OUTPUT_LIMIT".into()));
    }
    if report
        .last_pts_100ns
        .is_some_and(|pts| frame.pts_100ns <= pts)
    {
        return Err(EncodeError::Native("LIVE_NON_MONOTONIC_PTS".into()));
    }
    report.first_pts_100ns.get_or_insert(frame.pts_100ns);
    report.last_pts_100ns = Some(frame.pts_100ns);
    report.frames_encoded += 1;
    report.bytes_encoded += frame.annex_b.len() as u64;
    report.idr_frames += u32::from(frame.is_idr);
    sink(frame)
}

/// Explicit operator-only generated source. This function never opens DXGI or reads a desktop.
#[cfg(feature = "synthetic-stream")]
pub fn run_synthetic(
    approved: bool,
    config: EncoderConfig,
    stop: StopToken,
    keyframe: &AtomicBool,
    mut publish: impl FnMut(&StreamReport),
    mut sink: impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<StreamReport, EncodeError> {
    config.validate(approved)?;
    let setup = Instant::now();
    let _exclusive = ProcessLease::acquire()?;
    let runtime = MfRuntime::new()?;
    let pixels = (config.width * config.height) as usize;
    let mut nv12 = vec![128u8; pixels * 3 / 2];
    nv12[..pixels].fill(64);
    let (mut encoder, _) = mf::select(&runtime, &config, "auto", &nv12)?;
    let mut report = StreamReport {
        encoder: encoder.name.clone(),
        hardware: encoder.hardware,
        source_width: config.width,
        source_height: config.height,
        setup_ms: setup.elapsed().as_secs_f64() * 1000.0,
        ..Default::default()
    };
    let started = Instant::now();
    publish(&report);
    let mut tick = 0u64;
    while tick < (config.seconds * config.fps) as u64 && !stop.is_stopped() {
        let due = Duration::from_nanos(tick * 1_000_000_000 / config.fps as u64);
        while started.elapsed() < due && !stop.is_stopped() {
            std::thread::sleep(
                due.saturating_sub(started.elapsed())
                    .min(Duration::from_millis(5)),
            );
        }
        if stop.is_stopped() {
            break;
        }
        let (selected, missed, _) = config.schedule(tick, started.elapsed());
        tick = selected;
        report.missed_ticks += missed;
        if tick >= (config.seconds * config.fps) as u64 {
            break;
        }
        for (y, row) in nv12[..pixels]
            .chunks_exact_mut(config.width as usize)
            .enumerate()
        {
            row.fill(32 + ((y / 90 + tick as usize / 10) % 8) as u8 * 24);
            if (200..328).contains(&y) {
                let left = (tick as usize * 12) % ((config.width - 128) as usize);
                row[left..left + 128].fill(235);
            }
        }
        if keyframe.swap(false, Ordering::AcqRel) {
            report.keyframe_requests += 1;
            if encoder.request_keyframe().is_err() {
                report.keyframe_rejections += 1;
            }
        }
        report.frames_input += 1;
        for frame in encoder.encode(&nv12, config.pts(tick), config.duration(tick))? {
            if stop.is_stopped() {
                break;
            }
            deliver(frame, &mut report, STANDARD_OUTPUT_FRAMES, &mut sink)?;
        }
        report.media_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        publish(&report);
        tick += 1;
    }
    if !stop.is_stopped() {
        for frame in encoder.drain()? {
            deliver(frame, &mut report, STANDARD_OUTPUT_FRAMES, &mut sink)?;
        }
    }
    report.stopped = stop.is_stopped();
    report.media_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    publish(&report);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(pts: i64) -> EncodedVideoFrame {
        EncodedVideoFrame {
            annex_b: vec![0, 0, 0, 1, 0x65],
            pts_100ns: pts,
            duration_100ns: 333333,
            nal_types: vec![5],
            is_idr: true,
            encode_elapsed_ms: 0.0,
        }
    }
    #[test]
    fn live_sink_failure_aborts_and_time_cannot_reorder() {
        let mut report = StreamReport::default();
        let error = deliver(frame(0), &mut report, STANDARD_OUTPUT_FRAMES, &mut |_| {
            Err(EncodeError::Native("QUEUE_BACKPRESSURE".into()))
        });
        assert!(error.is_err());
        assert!(
            deliver(frame(0), &mut report, STANDARD_OUTPUT_FRAMES, &mut |_| Ok(
                ()
            ))
            .is_err()
        );
        assert_eq!(report.frames_encoded, 1);
    }

    #[test]
    fn output_never_exceeds_selected_frame_or_existing_byte_cap() {
        for max_frames in [STANDARD_OUTPUT_FRAMES, 1800] {
            let mut report = StreamReport {
                frames_encoded: max_frames - 1,
                ..Default::default()
            };
            deliver(frame(0), &mut report, max_frames, &mut |_| Ok(())).unwrap();
            assert_eq!(report.frames_encoded, max_frames);
            assert!(deliver(frame(1), &mut report, max_frames, &mut |_| Ok(())).is_err());
            let mut report = StreamReport {
                bytes_encoded: 128 * 1024 * 1024,
                ..Default::default()
            };
            assert!(deliver(frame(0), &mut report, max_frames, &mut |_| Ok(())).is_err());
        }
    }
}

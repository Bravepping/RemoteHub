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

pub fn run(
    consent: bool,
    config: EncoderConfig,
    stop: StopToken,
    keyframe: &AtomicBool,
    mut publish: impl FnMut(&StreamReport),
    mut sink: impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<StreamReport, EncodeError> {
    config.validate(consent)?;
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
    while tick < (config.seconds * config.fps) as u64 && !stop.is_stopped() {
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
        if tick >= (config.seconds * config.fps) as u64 {
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
            if let Err(error) = deliver(frame, &mut report, &mut sink) {
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
            if let Err(error) = deliver(frame, &mut report, &mut sink) {
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
    sink: &mut impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    if report.frames_encoded >= 900
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
        let error = deliver(frame(0), &mut report, &mut |_| {
            Err(EncodeError::Native("QUEUE_BACKPRESSURE".into()))
        });
        assert!(error.is_err());
        assert!(deliver(frame(0), &mut report, &mut |_| Ok(())).is_err());
        assert_eq!(report.frames_encoded, 1);
    }
}

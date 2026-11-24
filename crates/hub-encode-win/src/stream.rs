//! Live AU producer. All COM/DXGI/MF objects stay on the calling Windows thread.
//! No sample files, network handles, Tokio tasks or frontend pixels are owned here.
use crate::{
    EncodeError, EncoderConfig,
    color::bgra_to_nv12,
    h264::EncodedVideoFrame,
    lifecycle::StopToken,
    mf::{self, MfRuntime},
    probe::ProcessLease,
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
const STANDARD_OUTPUT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy)]
struct OutputLimits {
    frames: u32,
    bytes: u64,
    /// Opt-in public transport also limits each Annex B access unit.
    public_au_bytes: Option<u64>,
    pts_end_100ns: Option<i64>,
}

impl OutputLimits {
    const fn standard(frames: u32) -> Self {
        Self {
            frames,
            bytes: STANDARD_OUTPUT_BYTES,
            public_au_bytes: None,
            pts_end_100ns: None,
        }
    }
}

struct StreamLimits {
    ticks: u32,
    output: OutputLimits,
    deadline: Option<Instant>,
}

impl StreamLimits {
    fn standard(ticks: u32, frames: u32) -> Self {
        Self {
            ticks,
            output: OutputLimits::standard(frames),
            deadline: None,
        }
    }

    fn within_deadline(&self, at: Instant) -> bool {
        self.deadline.is_none_or(|deadline| at < deadline)
    }

    fn active(&self, stop: &StopToken) -> bool {
        !stop.is_stopped() && self.within_deadline(Instant::now())
    }
}

#[cfg(feature = "v2-public-video-sustained")]
fn sustained_limits(
    consent: bool,
    deadline: Instant,
    now: Instant,
) -> Result<(EncoderConfig, StreamLimits), EncodeError> {
    let config = EncoderConfig::default();
    config.validate(consent)?;
    let seconds = crate::PUBLIC_VIDEO_SUSTAINED_SECONDS;
    let remaining = deadline
        .checked_duration_since(now)
        .filter(|remaining| {
            !remaining.is_zero() && *remaining <= Duration::from_secs(seconds.into())
        })
        .ok_or(EncodeError::InvalidArgument(
            "invalid sustained video deadline",
        ))?;
    // Count only ticks that start within the caller's remaining interval.
    // At 30 FPS this value is capped at 55,800 before conversion.
    let ticks =
        u32::try_from((remaining.as_nanos() * u128::from(config.fps)).div_ceil(1_000_000_000))
            .map_err(|_| EncodeError::InvalidArgument("sustained video tick budget overflow"))?;
    // 4 Mb/s nominal rate, plus 50% headroom for rate-control variance.
    // This is only a cumulative counter, not reserved or buffered memory.
    let bytes = u128::from(config.bitrate)
        .checked_mul(remaining.as_nanos())
        .and_then(|bits| bits.checked_mul(3))
        .and_then(|bits| u64::try_from(bits.div_ceil(16_000_000_000)).ok())
        .ok_or(EncodeError::InvalidArgument(
            "sustained video byte budget overflow",
        ))?;
    Ok((
        config,
        StreamLimits {
            ticks,
            output: OutputLimits {
                frames: ticks,
                bytes,
                public_au_bytes: Some(256 * 1024),
                pts_end_100ns: Some(config.pts(u64::from(ticks))),
            },
            deadline: Some(deadline),
        },
    ))
}

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
        StreamLimits::standard(config.seconds * config.fps, STANDARD_OUTPUT_FRAMES),
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
        StreamLimits::standard(ticks, ticks),
        stop,
        &AtomicBool::new(false),
        publish,
        sink,
    )
}

/// Product encoder policy only. The native session actor must pass a deadline
/// from this exact authorized host session, not an invitation's creation time
/// or a caller-controlled duration. This does not authorize capture or input
/// by itself. The caller must also independently enforce its transport and
/// decoder/presentation cutoff, including discarding already queued AUs.
#[cfg(feature = "v2-public-video-sustained")]
pub fn run_public_video_sustained(
    consent: bool,
    deadline: Instant,
    stop: StopToken,
    keyframe: &AtomicBool,
    publish: impl FnMut(&StreamReport),
    sink: impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<StreamReport, EncodeError> {
    let (config, limits) = sustained_limits(consent, deadline, Instant::now())?;
    run_validated(config, limits, stop, keyframe, publish, sink)
}

fn run_validated(
    config: EncoderConfig,
    limits: StreamLimits,
    stop: StopToken,
    keyframe: &AtomicBool,
    mut publish: impl FnMut(&StreamReport),
    mut sink: impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<StreamReport, EncodeError> {
    let setup = Instant::now();
    if !limits.within_deadline(setup) {
        return Err(EncodeError::Native("CANCELLED_DURING_SETUP".into()));
    }
    let _exclusive = ProcessLease::acquire()?;
    let runtime = MfRuntime::new()?;
    let mut capture = DxgiCapture::new()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let first = loop {
        if !limits.active(&stop) {
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
    if !limits.within_deadline(Instant::now()) {
        return Err(EncodeError::Native("CANCELLED_DURING_SETUP".into()));
    }
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
    while tick < u64::from(limits.ticks) && limits.active(&stop) {
        let target = Duration::from_nanos(tick * 1_000_000_000 / config.fps as u64);
        while start.elapsed() < target && limits.active(&stop) {
            std::thread::sleep(
                target
                    .saturating_sub(start.elapsed())
                    .min(Duration::from_millis(5)),
            );
        }
        if !limits.active(&stop) {
            break;
        }
        let (selected, missed, reuse) = config.schedule(tick, start.elapsed());
        tick = selected;
        report.missed_ticks += missed;
        if tick >= u64::from(limits.ticks) {
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
        if !limits.within_deadline(Instant::now()) {
            break;
        }
        report.frames_input += 1;
        let frames = encoder.encode(&nv12, config.pts(tick), config.duration(tick))?;
        if let Err(error) = deliver_available(
            frames,
            &mut report,
            limits.output,
            || limits.active(&stop),
            &mut sink,
        ) {
            publish(&report);
            return Err(error);
        }
        report.media_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        publish(&report);
        tick += 1;
    }
    if limits.active(&stop) {
        // MFT calls are synchronous; they can return after the cutoff. Drop
        // queued product output at that point. Legacy drains retain their
        // prior behavior.
        if let Err(error) = deliver_available(
            encoder.drain()?,
            &mut report,
            limits.output,
            || limits.deadline.is_none() || limits.active(&stop),
            &mut sink,
        ) {
            publish(&report);
            return Err(error);
        }
    }
    // Cancellation deliberately flushes queued references by dropping the whole stream, not resuming P frames.
    report.stopped = stop.is_stopped();
    report.media_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
    publish(&report);
    Ok(report)
}

fn deliver_available(
    frames: Vec<EncodedVideoFrame>,
    report: &mut StreamReport,
    limits: OutputLimits,
    mut active: impl FnMut() -> bool,
    sink: &mut impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    for frame in frames {
        if !active() {
            break;
        }
        deliver(frame, report, limits, sink)?;
    }
    Ok(())
}

fn deliver(
    frame: EncodedVideoFrame,
    report: &mut StreamReport,
    limits: OutputLimits,
    sink: &mut impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    let exceeded = || EncodeError::Native("LIVE_OUTPUT_LIMIT".into());
    let frames = report
        .frames_encoded
        .checked_add(1)
        .filter(|count| *count <= limits.frames)
        .ok_or_else(exceeded)?;
    let frame_bytes = u64::try_from(frame.annex_b.len()).map_err(|_| exceeded())?;
    let bytes = report
        .bytes_encoded
        .checked_add(frame_bytes)
        .filter(|count| *count <= limits.bytes)
        .ok_or_else(exceeded)?;
    if limits
        .public_au_bytes
        .is_some_and(|max| !(1..=max).contains(&frame_bytes))
    {
        return Err(exceeded());
    }
    if limits.pts_end_100ns.is_some_and(|end| {
        frame.pts_100ns < 0
            || frame.duration_100ns <= 0
            || frame
                .pts_100ns
                .checked_add(frame.duration_100ns)
                .is_none_or(|pts| pts > end)
    }) {
        return Err(EncodeError::Native("LIVE_OUTPUT_TIMESTAMP_LIMIT".into()));
    }
    if report
        .last_pts_100ns
        .is_some_and(|pts| frame.pts_100ns <= pts)
    {
        return Err(EncodeError::Native("LIVE_NON_MONOTONIC_PTS".into()));
    }
    let idrs = report
        .idr_frames
        .checked_add(u32::from(frame.is_idr))
        .ok_or_else(exceeded)?;
    report.first_pts_100ns.get_or_insert(frame.pts_100ns);
    report.last_pts_100ns = Some(frame.pts_100ns);
    report.frames_encoded = frames;
    report.bytes_encoded = bytes;
    report.idr_frames = idrs;
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
            deliver(
                frame,
                &mut report,
                OutputLimits::standard(STANDARD_OUTPUT_FRAMES),
                &mut sink,
            )?;
        }
        report.media_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        publish(&report);
        tick += 1;
    }
    if !stop.is_stopped() {
        for frame in encoder.drain()? {
            deliver(
                frame,
                &mut report,
                OutputLimits::standard(STANDARD_OUTPUT_FRAMES),
                &mut sink,
            )?;
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
        let limits = OutputLimits::standard(STANDARD_OUTPUT_FRAMES);
        let error = deliver(frame(0), &mut report, limits, &mut |_| {
            Err(EncodeError::Native("QUEUE_BACKPRESSURE".into()))
        });
        assert!(error.is_err());
        assert!(deliver(frame(0), &mut report, limits, &mut |_| Ok(())).is_err());
        assert_eq!(report.frames_encoded, 1);
    }

    #[test]
    fn output_never_exceeds_selected_frame_or_existing_byte_cap() {
        for max_frames in [STANDARD_OUTPUT_FRAMES, 1800] {
            let limits = OutputLimits::standard(max_frames);
            let mut report = StreamReport {
                frames_encoded: max_frames - 1,
                ..Default::default()
            };
            deliver(frame(0), &mut report, limits, &mut |_| Ok(())).unwrap();
            assert_eq!(report.frames_encoded, max_frames);
            assert!(deliver(frame(1), &mut report, limits, &mut |_| Ok(())).is_err());
            let mut report = StreamReport {
                bytes_encoded: STANDARD_OUTPUT_BYTES,
                ..Default::default()
            };
            assert!(deliver(frame(0), &mut report, limits, &mut |_| Ok(())).is_err());
        }
    }

    #[cfg(feature = "v2-public-video-sustained")]
    #[test]
    fn sustained_limit_requires_consent_and_a_live_bounded_actor_deadline() {
        let now = Instant::now();
        let maximum = Duration::from_secs(u64::from(crate::PUBLIC_VIDEO_SUSTAINED_SECONDS));
        assert!(matches!(
            sustained_limits(false, now + maximum, now),
            Err(EncodeError::ConsentRequired)
        ));
        for deadline in [
            now - Duration::from_secs(1),
            now,
            now + maximum + Duration::from_nanos(1),
        ] {
            assert!(matches!(
                sustained_limits(true, deadline, now),
                Err(EncodeError::InvalidArgument(
                    "invalid sustained video deadline"
                ))
            ));
        }
        let (cfg, limits) = sustained_limits(true, now + maximum, now).unwrap();
        assert_eq!(
            (cfg.width, cfg.height, cfg.fps, cfg.bitrate),
            (1280, 720, 30, 4_000_000)
        );
        assert!(
            cfg.validate(true).is_ok(),
            "must pass the existing MFT validation"
        );
        assert_eq!((limits.ticks, limits.output.frames), (55_800, 55_800));
        assert_eq!(limits.output.bytes, 1_395_000_000);
        let early = now + Duration::from_millis(999);
        let (_, early_limits) = sustained_limits(true, early, now).unwrap();
        assert!(early_limits.within_deadline(early - Duration::from_nanos(1)));
        assert!(!early_limits.within_deadline(early));
        assert_eq!(
            (early_limits.ticks, early_limits.output.bytes),
            (30, 749_250)
        );
    }

    #[cfg(feature = "v2-public-video-sustained")]
    #[test]
    fn sustained_sink_path_admits_all_30_minute_ticks_and_rejects_the_next() {
        let now = Instant::now();
        let (cfg, limits) =
            sustained_limits(true, now + Duration::from_secs(30 * 60), now).unwrap();
        assert_eq!(limits.ticks, 54_000);
        let end = cfg.pts(u64::from(limits.ticks));
        assert_eq!(end, 18_000_000_000);
        let mut report = StreamReport::default();
        let mut last_end = 0;
        for tick in 0..u64::from(limits.ticks) {
            let mut au = frame(cfg.pts(tick));
            au.duration_100ns = cfg.duration(tick);
            deliver(au, &mut report, limits.output, &mut |accepted| {
                assert_eq!(accepted.pts_100ns, last_end);
                last_end = accepted.pts_100ns + accepted.duration_100ns;
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(last_end, end);
        assert_eq!(report.frames_encoded, 54_000);
        assert!(
            deliver(frame(end), &mut report, limits.output, &mut |_| {
                panic!("no sink handoff outside the frame ceiling")
            })
            .is_err()
        );
    }

    #[cfg(feature = "v2-public-video-sustained")]
    #[test]
    fn pending_output_is_discarded_at_the_actor_deadline_even_after_partial_delivery() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(30 * 60);
        let (cfg, limits) = sustained_limits(true, deadline, now).unwrap();
        let mut report = StreamReport::default();
        let mut times = [deadline - Duration::from_nanos(1), deadline].into_iter();
        let mut handoffs = 0;
        deliver_available(
            vec![frame(0), frame(cfg.pts(1)), frame(cfg.pts(2))],
            &mut report,
            limits.output,
            || limits.within_deadline(times.next().expect("discard the remaining batch")),
            &mut |_| {
                handoffs += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!((report.frames_encoded, handoffs), (1, 1));
        deliver_available(
            vec![frame(cfg.pts(2))],
            &mut report,
            limits.output,
            || limits.within_deadline(deadline),
            &mut |_| panic!("a late MFT drain reached the sink"),
        )
        .unwrap();
        assert_eq!(report.frames_encoded, 1);
    }

    #[cfg(feature = "v2-public-video-sustained")]
    #[test]
    fn sustained_output_rejects_bad_sample_size_time_and_accumulation_before_handoff() {
        let now = Instant::now();
        let (_, limits) = sustained_limits(true, now + Duration::from_secs(31 * 60), now).unwrap();
        let limits = limits.output;
        let mut empty = frame(0);
        empty.annex_b.clear();
        let mut oversize = frame(0);
        oversize.annex_b.resize(256 * 1024 + 1, 0);
        let mut zero_duration = frame(0);
        zero_duration.duration_100ns = 0;
        for invalid in [
            empty,
            oversize,
            zero_duration,
            frame(-1),
            frame(limits.pts_end_100ns.unwrap() - 1),
            frame(i64::MAX),
        ] {
            let mut report = StreamReport::default();
            assert!(
                deliver(invalid, &mut report, limits, &mut |_| {
                    panic!("invalid sustained AU reached the sink")
                })
                .is_err()
            );
            assert_eq!(
                (
                    report.frames_encoded,
                    report.bytes_encoded,
                    report.last_pts_100ns
                ),
                (0, 0, None)
            );
        }
        for initial_bytes in [limits.bytes, u64::MAX] {
            let mut report = StreamReport {
                bytes_encoded: initial_bytes,
                ..Default::default()
            };
            assert!(
                deliver(frame(0), &mut report, limits, &mut |_| {
                    panic!("cumulative overflow reached the sink")
                })
                .is_err()
            );
            assert_eq!(
                (report.frames_encoded, report.bytes_encoded),
                (0, initial_bytes)
            );
        }
        let mut report = StreamReport {
            bytes_encoded: limits.bytes - 5,
            ..Default::default()
        };
        deliver(frame(0), &mut report, limits, &mut |_| Ok(())).unwrap();
        assert_eq!(report.bytes_encoded, limits.bytes);
    }
}

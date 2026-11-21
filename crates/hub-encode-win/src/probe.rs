use crate::{
    color::bgra_to_nv12,
    h264::EncodedVideoFrame,
    lifecycle::{ProbeGate, StopToken},
    mf::{self, MfRuntime},
    EncodeError, EncoderConfig,
};
use hub_capture_win::DxgiCapture;
use serde::Serialize;
use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
use windows::{
    core::w,
    Win32::{
        Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, FILETIME, HANDLE},
        System::{
            ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::{CreateMutexW, GetCurrentProcess, GetProcessTimes, ReleaseMutex},
        },
    },
};

static GATE: ProbeGate = ProbeGate::new();

fn checked_capture(
    fetch: impl FnOnce()
        -> Result<Option<hub_capture_win::CapturedFrame>, hub_capture_win::CaptureError>,
    width: u32,
    height: u32,
) -> Result<Option<hub_capture_win::CapturedFrame>, EncodeError> {
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

struct ProcessLease(HANDLE);
impl ProcessLease {
    fn acquire() -> Result<Self, EncodeError> {
        unsafe {
            let handle = CreateMutexW(None, true, w!("Local\\RemoteHub.P003.EncodeProbe"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Err(EncodeError::Busy);
            }
            Ok(Self(handle))
        }
    }
}
impl Drop for ProcessLease {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Serialize)]
pub struct FrameMetadata {
    pub pts_100ns: i64,
    pub duration_100ns: i64,
    pub bytes: usize,
    pub nal_types: Vec<u8>,
    pub idr: bool,
    pub encode_elapsed_ms: f64,
}
#[derive(Serialize)]
pub struct Timing {
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}
fn timing(data: &[f64]) -> Timing {
    let mut sorted = data.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        if sorted.is_empty() {
            0.0
        } else {
            sorted[((sorted.len() - 1) as f64 * p).ceil() as usize]
        }
    };
    Timing {
        samples: sorted.len(),
        p50_ms: percentile(0.5),
        p95_ms: percentile(0.95),
        max_ms: percentile(1.0),
    }
}
#[derive(Serialize)]
pub struct ProbeReport {
    pub encoder: String,
    pub hardware: bool,
    pub candidates: Vec<mf::CandidateDiagnostic>,
    pub codec_properties: Vec<String>,
    pub source_framing: String,
    pub exported_framing: &'static str,
    pub input_format: &'static str,
    pub conversion: &'static str,
    pub width: u32,
    pub height: u32,
    pub requested_fps: u32,
    pub requested_bitrate: u32,
    pub adapter: String,
    pub source_width: u32,
    pub source_height: u32,
    pub desktop_updates: u32,
    pub distinct_desktop_images: usize,
    pub repeated_frames: u32,
    pub repeated_due_to_lateness: u32,
    pub missed_ticks: u32,
    pub encoded_frames: usize,
    pub wall_ms: f64,
    pub setup_ms: f64,
    pub video_duration_ms: f64,
    pub encoded_fps_wall: f64,
    pub actual_bitrate_bps: f64,
    pub encoded_bytes: u64,
    pub stopped: bool,
    pub capture_time: Timing,
    pub conversion_time: Timing,
    pub encode_submission_time: Timing,
    pub encode_completion_time: Timing,
    pub cpu_seconds: f64,
    pub cpu_one_core_percent: f64,
    pub cpu_machine_percent: f64,
    pub peak_working_set_bytes: usize,
    pub gpu: crate::metrics::GpuReport,
    pub frames: Vec<FrameMetadata>,
}

struct SampleWriter {
    file: std::fs::File,
    frames: Vec<FrameMetadata>,
    bytes: u64,
}
impl SampleWriter {
    fn write(&mut self, frames: Vec<EncodedVideoFrame>) -> Result<(), EncodeError> {
        for f in frames {
            if self.frames.len() >= 900 || self.bytes + f.annex_b.len() as u64 > 128 * 1024 * 1024 {
                return Err(EncodeError::Native("probe output limit reached".into()));
            }
            if f.duration_100ns <= 0
                || self
                    .frames
                    .last()
                    .is_some_and(|last| f.pts_100ns <= last.pts_100ns)
            {
                return Err(EncodeError::Native("non-monotonic MFT output time".into()));
            }
            self.file.write_all(&f.annex_b)?;
            self.bytes += f.annex_b.len() as u64;
            self.frames.push(FrameMetadata {
                pts_100ns: f.pts_100ns,
                duration_100ns: f.duration_100ns,
                bytes: f.annex_b.len(),
                nal_types: f.nal_types,
                idr: f.is_idr,
                encode_elapsed_ms: f.encode_elapsed_ms,
            });
        }
        Ok(())
    }
}

fn cpu_time() -> Result<f64, EncodeError> {
    unsafe {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )?;
        let value = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        Ok((value(kernel) + value(user)) as f64 / 10_000_000.0)
    }
}
fn memory() -> usize {
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        if K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .as_bool()
        {
            counters.PeakWorkingSetSize
        } else {
            0
        }
    }
}

pub fn run(
    consent: bool,
    config: EncoderConfig,
    mode: &str,
    output: &Path,
    stop: StopToken,
) -> Result<ProbeReport, EncodeError> {
    let _lease = GATE.enter(consent, &config)?;
    let setup_start = Instant::now();
    if !["auto", "software", "hardware"].contains(&mode) {
        return Err(EncodeError::InvalidArgument(
            "encoder mode must be auto/software/hardware",
        ));
    }
    if output.exists() {
        return Err(EncodeError::InvalidArgument(
            "output directory already exists",
        ));
    }
    let _process_lease = ProcessLease::acquire()?;
    let runtime = MfRuntime::new()?;
    let mut capture = DxgiCapture::new()?;
    let first_deadline = Instant::now() + Duration::from_secs(3);
    let mut first = loop {
        if stop.is_stopped() {
            return Err(EncodeError::Native(
                "stopped before first desktop frame".into(),
            ));
        }
        if let Some(frame) = capture.next_frame(100)? {
            break frame;
        }
        if Instant::now() >= first_deadline {
            return Err(EncodeError::Timeout("first DXGI frame"));
        }
    };
    let source_width = first.width;
    let source_height = first.height;
    let mut nv12 = bgra_to_nv12(
        &first.bgra,
        first.width,
        first.height,
        first.width as usize * 4,
        config.width,
        config.height,
    )?;
    // Compatibility preflight is encoded from this real DXGI image; preflight data is discarded.
    let (mut encoder, candidates) = mf::select(&runtime, &config, mode, &nv12)?;
    std::fs::create_dir(output)?;
    let mut writer = SampleWriter {
        file: std::fs::File::create_new(output.join("desktop.h264"))?,
        frames: Vec::new(),
        bytes: 0,
    };
    let mut capture_times = Vec::new();
    let mut convert_times = Vec::new();
    let mut encode_times = Vec::new();
    let mut hashes = HashSet::new();
    let mut desktop_updates = 1;
    let mut repeated = 0;
    let mut missed = 0;
    let mut late_repeats = 0;
    let mut gpu = crate::metrics::GpuSampler::new();
    let setup_ms = setup_start.elapsed().as_secs_f64() * 1000.0;
    let cpu_start = cpu_time()?;
    let mut peak = memory();
    let start = Instant::now();
    let mut tick = 0u64;
    while tick < (config.seconds * config.fps) as u64 && !stop.is_stopped() {
        let target = Duration::from_nanos(tick * 1_000_000_000 / config.fps as u64);
        while start.elapsed() < target && !stop.is_stopped() {
            if let Some(remaining) = target.checked_sub(start.elapsed()) {
                std::thread::sleep(remaining.min(Duration::from_millis(5)));
            }
        }
        if stop.is_stopped() {
            break;
        }
        let (selected, skipped, reuse_desktop) = config.schedule(tick, start.elapsed());
        tick = selected;
        missed += skipped;
        if tick >= (config.seconds * config.fps) as u64 {
            break;
        }
        let cap_start = Instant::now();
        let update = if reuse_desktop {
            late_repeats += 1;
            None
        } else {
            checked_capture(|| capture.next_frame(0), source_width, source_height)?
        };
        capture_times.push(cap_start.elapsed().as_secs_f64() * 1000.0);
        if let Some(frame) = update {
            let convert_start = Instant::now();
            nv12 = bgra_to_nv12(
                &frame.bgra,
                frame.width,
                frame.height,
                frame.width as usize * 4,
                config.width,
                config.height,
            )?;
            convert_times.push(convert_start.elapsed().as_secs_f64() * 1000.0);
            first = frame;
            desktop_updates += 1;
        } else {
            repeated += 1;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        first.bgra.hash(&mut hasher);
        hashes.insert(hasher.finish());
        let encode_start = Instant::now();
        writer.write(encoder.encode(&nv12, config.pts(tick), config.duration(tick))?)?;
        encode_times.push(encode_start.elapsed().as_secs_f64() * 1000.0);
        peak = peak.max(memory());
        gpu.sample();
        tick += 1;
    }
    writer.write(encoder.drain()?)?;
    writer.file.flush()?;
    let wall = start.elapsed().as_secs_f64();
    let cpu = cpu_time()? - cpu_start;
    let duration = writer
        .frames
        .last()
        .map(|f| f.pts_100ns + f.duration_100ns)
        .unwrap_or(0) as f64
        / 10_000_000.0;
    if writer.frames.is_empty() {
        return Err(EncodeError::Native("no encoded frames".into()));
    }
    let report = ProbeReport {
        encoder: encoder.name.clone(),
        hardware: encoder.hardware,
        candidates,
        codec_properties: encoder.property_results.clone(),
        source_framing: encoder.source_framing.clone(),
        exported_framing: "Annex B",
        input_format: "NV12 (Y + interleaved UV, stride=width)",
        conversion: "CPU bilinear BGRA resize + BT.601 limited-range",
        width: config.width,
        height: config.height,
        requested_fps: config.fps,
        requested_bitrate: config.bitrate,
        adapter: capture.adapter_name().into(),
        source_width,
        source_height,
        desktop_updates,
        distinct_desktop_images: hashes.len(),
        repeated_frames: repeated,
        repeated_due_to_lateness: late_repeats,
        missed_ticks: missed,
        encoded_frames: writer.frames.len(),
        wall_ms: wall * 1000.0,
        setup_ms,
        video_duration_ms: duration * 1000.0,
        encoded_fps_wall: writer.frames.len() as f64 / wall,
        actual_bitrate_bps: writer.bytes as f64 * 8.0 / duration,
        encoded_bytes: writer.bytes,
        stopped: stop.is_stopped(),
        capture_time: timing(&capture_times),
        conversion_time: timing(&convert_times),
        encode_submission_time: timing(&encode_times),
        encode_completion_time: timing(
            &writer
                .frames
                .iter()
                .map(|f| f.encode_elapsed_ms)
                .collect::<Vec<_>>(),
        ),
        cpu_seconds: cpu,
        cpu_one_core_percent: cpu / wall * 100.0,
        cpu_machine_percent: cpu / wall * 100.0
            / std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1) as f64,
        peak_working_set_bytes: peak,
        gpu: gpu.finish(),
        frames: writer.frames,
    };
    std::fs::write(
        output.join("encode-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| EncodeError::Native(e.to_string()))?,
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_access_lost_and_resize_failures_propagate_and_release_probe_lease() {
        let gate = ProbeGate::new();
        let config = EncoderConfig::default();
        let failed = (|| -> Result<(), EncodeError> {
            let _lease = gate.enter(true, &config)?;
            checked_capture(
                || Err(hub_capture_win::CaptureError::AccessLost),
                2560,
                1440,
            )?;
            Ok(())
        })();
        assert!(matches!(
            failed,
            Err(EncodeError::Capture(
                hub_capture_win::CaptureError::AccessLost
            ))
        ));
        let _again = gate.enter(true, &config).unwrap();
        let resized = checked_capture(
            || {
                Ok(Some(hub_capture_win::CapturedFrame {
                    width: 1920,
                    height: 1080,
                    timestamp_us: 1,
                    bgra: vec![],
                }))
            },
            2560,
            1440,
        );
        assert!(matches!(resized, Err(EncodeError::Native(_))));
        assert!(checked_capture(|| Ok(None), 2560, 1440).unwrap().is_none());
    }
}

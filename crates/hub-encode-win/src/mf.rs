use crate::{
    h264::{avcc_configuration, normalize, EncodedVideoFrame, Framing, MAX_ACCESS_UNIT},
    EncodeError, EncoderConfig,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    marker::PhantomData,
    mem::ManuallyDrop,
    rc::Rc,
    time::{Duration, Instant},
};
use windows::{
    core::{Interface, GUID},
    Win32::{
        Media::MediaFoundation::*,
        System::{Com::*, Variant::VARIANT},
    },
};

pub struct MfRuntime(PhantomData<Rc<()>>);
impl MfRuntime {
    pub fn new() -> Result<Self, EncodeError> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                CoUninitialize();
                return Err(error.into());
            }
        }
        Ok(Self(PhantomData))
    }
}
impl Drop for MfRuntime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CandidateDiagnostic {
    pub name: String,
    pub hardware: bool,
    pub result: String,
}

struct ActivationArray {
    ptr: *mut Option<IMFActivate>,
    len: u32,
}
impl Drop for ActivationArray {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                for slot in std::slice::from_raw_parts_mut(self.ptr, self.len as usize) {
                    drop(slot.take());
                }
                CoTaskMemFree(Some(self.ptr.cast()));
            }
        }
    }
}

pub fn enumerate(hardware: bool) -> Result<Vec<(String, IMFActivate)>, EncodeError> {
    unsafe {
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let output = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let flags = if hardware {
            MFT_ENUM_FLAG_HARDWARE
        } else {
            MFT_ENUM_FLAG_SYNCMFT
        };
        let mut array = ActivationArray {
            ptr: std::ptr::null_mut(),
            len: 0,
        };
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut array.ptr,
            &mut array.len,
        )?;
        if array.len == 0 {
            return Ok(Vec::new());
        }
        if array.ptr.is_null() || array.len > 256 {
            return Err(EncodeError::Native("invalid MFT enumeration".into()));
        }
        let mut result = Vec::new();
        for activate in std::slice::from_raw_parts(array.ptr, array.len as usize)
            .iter()
            .flatten()
        {
            let length = activate.GetStringLength(&MFT_FRIENDLY_NAME_Attribute)?;
            if length > 4096 {
                return Err(EncodeError::Native("MFT name too long".into()));
            }
            let mut name = vec![0; length as usize + 1];
            activate.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut name, None)?;
            result.push((
                String::from_utf16_lossy(&name[..length as usize]),
                activate.clone(),
            ));
        }
        Ok(result)
    }
}

pub struct MfEncoder<'a> {
    transform: IMFTransform,
    activation: IMFActivate,
    events: Option<IMFMediaEventGenerator>,
    input_credits: u32,
    output_credits: u32,
    drained: bool,
    pub name: String,
    pub hardware: bool,
    pub property_results: Vec<String>,
    pub source_framing: String,
    avcc_length: Option<usize>,
    parameter_sets: Vec<u8>,
    expected_input_bytes: usize,
    last_input_pts: Option<i64>,
    pending: BTreeMap<i64, Instant>,
    _runtime: &'a MfRuntime,
}

fn media_type(config: &EncoderConfig, subtype: GUID) -> Result<IMFMediaType, EncodeError> {
    unsafe {
        let media = MFCreateMediaType()?;
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
        media.SetUINT64(
            &MF_MT_FRAME_SIZE,
            ((config.width as u64) << 32) | config.height as u64,
        )?;
        media.SetUINT64(&MF_MT_FRAME_RATE, ((config.fps as u64) << 32) | 1)?;
        media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
        media.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        Ok(media)
    }
}

impl<'a> MfEncoder<'a> {
    /// Request the next keyframe on the owning COM thread. Some drivers may reject it.
    pub fn request_keyframe(&self) -> Result<(), EncodeError> {
        unsafe {
            self.transform
                .cast::<ICodecAPI>()?
                .SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32))?;
        }
        Ok(())
    }

    pub fn open(
        runtime: &'a MfRuntime,
        activation: IMFActivate,
        name: String,
        hardware: bool,
        config: &EncoderConfig,
    ) -> Result<Self, EncodeError> {
        config.validate(true)?;
        unsafe {
            let transform: IMFTransform = activation.ActivateObject()?;
            // Construct ownership before format negotiation, so every failure shuts the MFT down.
            let mut this = Self {
                transform,
                activation,
                events: None,
                input_credits: 0,
                output_credits: 0,
                drained: false,
                name,
                hardware,
                property_results: Vec::new(),
                source_framing: String::new(),
                avcc_length: None,
                parameter_sets: Vec::new(),
                expected_input_bytes: config.width as usize * config.height as usize * 3 / 2,
                last_input_pts: None,
                pending: BTreeMap::new(),
                _runtime: runtime,
            };
            let attributes = this.transform.GetAttributes()?;
            let asynchronous = attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
            if asynchronous {
                attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            }
            if asynchronous {
                this.events = Some(this.transform.cast()?);
            }
            if let Ok(codec) = this.transform.cast::<ICodecAPI>() {
                for (key, value, label) in [
                    (CODECAPI_AVEncMPVGOPSize, VARIANT::from(60u32), "gop_60"),
                    (
                        CODECAPI_AVLowLatencyMode,
                        VARIANT::from(true),
                        "low_latency",
                    ),
                    (
                        CODECAPI_AVEncMPVDefaultBPictureCount,
                        VARIANT::from(0u32),
                        "b_frames_0",
                    ),
                ] {
                    let status = codec.SetValue(&key, &value);
                    this.property_results.push(format!(
                        "{label}: {}",
                        match status {
                            Ok(()) => "accepted".into(),
                            Err(e) => format!("rejected {}", e.code()),
                        }
                    ));
                }
            }
            let output = media_type(config, MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, config.bitrate)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
            this.transform.SetOutputType(0, &output, 0)?;
            let input = media_type(config, MFVideoFormat_NV12)?;
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, config.width)?;
            input.SetUINT32(&MF_MT_SAMPLE_SIZE, config.width * config.height * 3 / 2)?;
            this.transform.SetInputType(0, &input, 0)?;
            if let Ok(current) = this.transform.GetOutputCurrentType(0) {
                if let Ok(length) = current.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                    if length > MAX_ACCESS_UNIT as u32 {
                        return Err(EncodeError::Bitstream("sequence header too large"));
                    }
                    let mut header = vec![0; length as usize];
                    current.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None)?;
                    if header.first() == Some(&1) {
                        let (width, sets) = avcc_configuration(&header)?;
                        this.avcc_length = Some(width);
                        this.parameter_sets = sets;
                    } else if header.starts_with(&[0, 0, 1]) || header.starts_with(&[0, 0, 0, 1]) {
                        this.parameter_sets = normalize(&header, Framing::AnnexB)?.0;
                    }
                }
            }
            this.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            this.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(this)
        }
    }

    fn poll_events(&mut self) -> Result<(), EncodeError> {
        if let Some(events) = &self.events {
            unsafe {
                for _ in 0..64 {
                    let event = match events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                        Ok(e) => e,
                        Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                        Err(e) => return Err(e.into()),
                    };
                    event.GetStatus()?.ok()?;
                    let kind = event.GetType()?;
                    if kind == METransformNeedInput.0 as u32 {
                        self.input_credits = self.input_credits.saturating_add(1).min(64);
                    }
                    if kind == METransformHaveOutput.0 as u32 {
                        self.output_credits = self.output_credits.saturating_add(1).min(64);
                    }
                    if kind == METransformDrainComplete.0 as u32 {
                        self.drained = true;
                    }
                }
            }
        }
        Ok(())
    }

    fn output_one(&mut self) -> Result<Option<EncodedVideoFrame>, EncodeError> {
        unsafe {
            let info = self.transform.GetOutputStreamInfo(0)?;
            let sample = if info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 == 0 {
                let size = info.cbSize.max(1024 * 1024);
                if size as usize > MAX_ACCESS_UNIT {
                    return Err(EncodeError::Bitstream("MFT output allocation too large"));
                }
                let sample = MFCreateSample()?;
                sample.AddBuffer(&MFCreateAlignedMemoryBuffer(
                    size,
                    info.cbAlignment.saturating_sub(1),
                )?)?;
                Some(sample)
            } else {
                None
            };
            let mut output = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0;
            let result =
                self.transform
                    .ProcessOutput(0, std::slice::from_mut(&mut output), &mut status);
            // These are COM owning fields hidden behind ManuallyDrop by windows-rs.
            let sample = ManuallyDrop::take(&mut output.pSample);
            drop(ManuallyDrop::take(&mut output.pEvents));
            match result {
                Ok(()) => (),
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) => return Err(e.into()),
            }
            let sample =
                sample.ok_or_else(|| EncodeError::Native("MFT returned no sample".into()))?;
            let pts = sample.GetSampleTime()?;
            let duration = sample.GetSampleDuration()?;
            let buffer = sample.ConvertToContiguousBuffer()?;
            let data = read_buffer(&buffer)?;
            let framing = if data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1]) {
                Framing::AnnexB
            } else if let Some(length_size) = self.avcc_length {
                Framing::Avcc { length_size }
            } else {
                return Err(EncodeError::Bitstream(
                    "non-Annex-B output without AVCC configuration",
                ));
            };
            self.source_framing = format!("{framing:?}");
            let (mut annex_b, mut nal_types) = normalize(&data, framing)?;
            let is_idr = nal_types.contains(&5);
            if is_idr
                && !(nal_types.contains(&7) && nal_types.contains(&8))
                && !self.parameter_sets.is_empty()
            {
                let mut initialized = self.parameter_sets.clone();
                initialized.extend_from_slice(&annex_b);
                let normalized = normalize(&initialized, Framing::AnnexB)?;
                annex_b = normalized.0;
                nal_types = normalized.1;
            }
            let submitted = self.pending.remove(&pts).ok_or_else(|| {
                EncodeError::Native("MFT output PTS did not match an input sample".into())
            })?;
            Ok(Some(EncodedVideoFrame {
                annex_b,
                pts_100ns: pts,
                duration_100ns: duration,
                nal_types,
                is_idr,
                encode_elapsed_ms: submitted.elapsed().as_secs_f64() * 1000.0,
            }))
        }
    }

    pub fn take_available(&mut self) -> Result<Vec<EncodedVideoFrame>, EncodeError> {
        self.poll_events()?;
        let mut frames = Vec::new();
        for _ in 0..32 {
            if self.events.is_some() && self.output_credits == 0 {
                break;
            }
            if self.events.is_some() {
                self.output_credits -= 1;
            }
            match self.output_one()? {
                Some(f) => frames.push(f),
                None => break,
            }
        }
        Ok(frames)
    }

    pub fn encode(
        &mut self,
        nv12: &[u8],
        pts: i64,
        duration: i64,
    ) -> Result<Vec<EncodedVideoFrame>, EncodeError> {
        if nv12.len() != self.expected_input_bytes
            || pts < 0
            || duration <= 0
            || self.last_input_pts.is_some_and(|last| pts <= last)
        {
            return Err(EncodeError::InvalidArgument(
                "invalid NV12 sample size or non-monotonic input time",
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut frames = Vec::new();
        while self.events.is_some() && self.input_credits == 0 {
            frames.extend(self.take_available()?);
            if self.input_credits > 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err(EncodeError::Timeout("waiting for MFT input credit"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut submitted;
        unsafe {
            let sample = MFCreateSample()?;
            let buffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
            let mut pointer = std::ptr::null_mut();
            buffer.Lock(&mut pointer, None, None)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), pointer, nv12.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(nv12.len() as u32)?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(duration)?;
            if self.pending.len() >= 64 {
                return Err(EncodeError::Native(
                    "MFT in-flight queue exceeded 64 samples".into(),
                ));
            }
            submitted = Instant::now();
            match self.transform.ProcessInput(0, &sample, 0) {
                Ok(()) => (),
                Err(e) if e.code() == MF_E_NOTACCEPTING && self.events.is_none() => {
                    frames.extend(self.take_available()?);
                    submitted = Instant::now();
                    self.transform.ProcessInput(0, &sample, 0)?;
                }
                Err(e) => return Err(e.into()),
            }
        }
        if self.events.is_some() {
            self.input_credits -= 1;
        }
        self.last_input_pts = Some(pts);
        self.pending.insert(pts, submitted);
        frames.extend(self.take_available()?);
        Ok(frames)
    }

    pub fn drain(&mut self) -> Result<Vec<EncodedVideoFrame>, EncodeError> {
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)?;
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut frames = Vec::new();
        loop {
            let new = self.take_available()?;
            let empty = new.is_empty();
            frames.extend(new);
            if frames.len() > 64 {
                return Err(EncodeError::Native(
                    "MFT drain queue exceeded 64 frames".into(),
                ));
            }
            if self.events.is_none() && empty
                || self.events.is_some() && self.drained && self.output_credits == 0
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err(EncodeError::Timeout("MFT drain"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(frames)
    }
}

fn read_buffer(buffer: &IMFMediaBuffer) -> Result<Vec<u8>, EncodeError> {
    unsafe {
        let mut pointer = std::ptr::null_mut();
        let mut length = 0;
        buffer.Lock(&mut pointer, None, Some(&mut length))?;
        struct Lock<'a>(&'a IMFMediaBuffer);
        impl Drop for Lock<'_> {
            fn drop(&mut self) {
                unsafe {
                    let _ = self.0.Unlock();
                }
            }
        }
        let _lock = Lock(buffer);
        if length == 0 || length as usize > MAX_ACCESS_UNIT || pointer.is_null() {
            return Err(EncodeError::Bitstream("invalid MFT sample length"));
        }
        Ok(std::slice::from_raw_parts(pointer, length as usize).to_vec())
    }
}

impl Drop for MfEncoder<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.activation.ShutdownObject();
        }
    }
}

pub fn select<'a>(
    runtime: &'a MfRuntime,
    config: &EncoderConfig,
    mode: &str,
    real_nv12: &[u8],
) -> Result<(MfEncoder<'a>, Vec<CandidateDiagnostic>), EncodeError> {
    select_with(runtime, config, mode, real_nv12, enumerate)
}

fn select_with<'a>(
    runtime: &'a MfRuntime,
    config: &EncoderConfig,
    mode: &str,
    real_nv12: &[u8],
    enumerator: impl Fn(bool) -> Result<Vec<(String, IMFActivate)>, EncodeError>,
) -> Result<(MfEncoder<'a>, Vec<CandidateDiagnostic>), EncodeError> {
    let mut diagnostics = Vec::new();
    for hardware in [true, false] {
        if mode == "software" && hardware || mode == "hardware" && !hardware {
            continue;
        }
        let candidates = match enumerator(hardware) {
            Ok(candidates) => candidates,
            Err(error) => {
                diagnostics.push(CandidateDiagnostic {
                    name: "enumeration".into(),
                    hardware,
                    result: error.to_string(),
                });
                continue;
            }
        };
        if candidates.is_empty() {
            diagnostics.push(CandidateDiagnostic {
                name: "none".into(),
                hardware,
                result: "no matching NV12/H264 MFT registered".into(),
            });
        }
        for (name, activation) in candidates {
            let result = (|| {
                let mut test =
                    MfEncoder::open(runtime, activation.clone(), name.clone(), hardware, config)?;
                let mut frames = 0;
                for i in 0..3 {
                    frames += test
                        .encode(real_nv12, config.pts(i), config.duration(i))?
                        .len();
                }
                frames += test.drain()?.len();
                if frames == 0 {
                    return Err(EncodeError::Unsupported(
                        "preflight produced no real-frame H264".into(),
                    ));
                }
                drop(test);
                MfEncoder::open(runtime, activation.clone(), name.clone(), hardware, config)
            })();
            match result {
                Ok(encoder) => {
                    diagnostics.push(CandidateDiagnostic {
                        name,
                        hardware,
                        result: "actual DXGI-frame encoding preflight passed".into(),
                    });
                    return Ok((encoder, diagnostics));
                }
                Err(error) => diagnostics.push(CandidateDiagnostic {
                    name,
                    hardware,
                    result: error.to_string(),
                }),
            }
        }
    }
    Err(EncodeError::Unsupported(
        serde_json::to_string(&diagnostics).unwrap_or_else(|_| "no MFT".into()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_mft_failure_releases_thread_com_runtime() {
        assert!(unsafe { CoGetContextToken() }.is_err());
        {
            let runtime = MfRuntime::new().unwrap();
            assert!(unsafe { CoGetContextToken() }.is_ok());
            let result = select_with(&runtime, &EncoderConfig::default(), "auto", &[], |_| {
                Ok(Vec::new())
            });
            assert!(matches!(result, Err(EncodeError::Unsupported(_))));
        }
        assert!(unsafe { CoGetContextToken() }.is_err());
    }
}

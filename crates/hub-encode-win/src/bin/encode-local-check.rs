//! Short on-device DXGI/Media Foundation check. It never connects to any
//! service or writes media files. The live stream's output is discarded.

use std::{ffi::OsStr, process::ExitCode};

#[cfg(windows)]
fn classify(error: &hub_encode_win::EncodeError) -> String {
    use hub_capture_win::CaptureError;
    use hub_encode_win::EncodeError;

    match error {
        EncodeError::Busy => "ENCODER_BUSY".into(),
        EncodeError::Capture(CaptureError::AccessLost) => "DXGI_ACCESS_LOST".into(),
        EncodeError::Capture(CaptureError::Dxgi { stage, code }) => {
            format!("dxgi_stage={stage} HRESULT=0x{code:08X}")
        }
        EncodeError::Capture(CaptureError::DxgiState(state)) => format!("dxgi_stage={state}"),
        EncodeError::Capture(CaptureError::NoFrames)
        | EncodeError::Timeout("live first DXGI frame") => "NO_DESKTOP_FRAME".into(),
        EncodeError::Capture(_) => "DXGI_UNAVAILABLE".into(),
        EncodeError::Unsupported(_) => "H264_UNSUPPORTED".into(),
        EncodeError::Windows(error) => format!("WINDOWS_HRESULT=0x{:08X}", error.code().0 as u32),
        EncodeError::Bitstream(_) => "INVALID_H264".into(),
        EncodeError::Native(detail) => match detail.as_str() {
            "LIVE_DISPLAY_RESIZED" => "DISPLAY_CHANGED".into(),
            "LIVE_OUTPUT_LIMIT" => "OUTPUT_LIMIT".into(),
            "LIVE_NON_MONOTONIC_PTS" => "NON_MONOTONIC_PTS".into(),
            "CANCELLED_DURING_SETUP" => "CANCELLED_DURING_SETUP".into(),
            _ => "ENCODER_FAILED".into(),
        },
        EncodeError::Timeout(_) => "ENCODER_TIMEOUT".into(),
        _ => "ENCODER_FAILED".into(),
    }
}

#[cfg(windows)]
fn check() -> Result<u32, String> {
    use hub_encode_win::{lifecycle::StopToken, stream, EncoderConfig};
    use std::{cell::Cell, sync::atomic::AtomicBool};

    let ready = Cell::new(false);
    let mut frames = 0_u32;
    let result = stream::run(
        true,
        EncoderConfig {
            seconds: 3,
            ..EncoderConfig::default()
        },
        StopToken::default(),
        &AtomicBool::new(false),
        |_| ready.set(true),
        |frame| {
            frames += 1;
            drop(frame);
            Ok(())
        },
    );
    match result {
        Ok(report) if frames > 0 && report.frames_encoded == frames => Ok(frames),
        Ok(_) => Err("stage=STREAM NO_ENCODED_FRAMES".into()),
        Err(error) => {
            let phase = if ready.get() { "STREAM" } else { "SETUP" };
            Err(format!("stage={phase} {}", classify(&error)))
        }
    }
}

fn main() -> ExitCode {
    if !std::env::args_os()
        .skip(1)
        .eq([OsStr::new("--consent"), OsStr::new("--local-only")])
    {
        eprintln!("Local three-second encode check, memory only. Run with --consent --local-only.");
        return ExitCode::from(2);
    }
    #[cfg(windows)]
    match check() {
        Ok(frames) => {
            println!("ENCODE_LOCAL_CHECK_PASS encoded_frames={frames}");
            ExitCode::SUCCESS
        }
        Err(code) => {
            eprintln!("ENCODE_LOCAL_CHECK_FAILED {code}");
            ExitCode::FAILURE
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("ENCODE_LOCAL_CHECK_FAILED WINDOWS_REQUIRED");
        ExitCode::FAILURE
    }
}

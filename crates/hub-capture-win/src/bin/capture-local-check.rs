//! One-frame on-device DXGI diagnostic. No network, encoding or output files.
#[cfg(windows)]
use hub_capture_win::{CaptureError, DxgiCapture};

#[cfg(windows)]
fn status(error: &CaptureError) -> String {
    match error {
        CaptureError::AccessLost => "DXGI_ACCESS_LOST".into(),
        CaptureError::Dxgi { stage, code } => format!("api={stage} HRESULT=0x{code:08x}"),
        CaptureError::DxgiState(state) => format!("api={state}"),
        CaptureError::Unavailable(_) => "DXGI_UNAVAILABLE".into(),
        CaptureError::InvalidArgument(_) => "INVALID_FRAME".into(),
        _ => "CAPTURE_FAILED".into(),
    }
}

#[cfg(windows)]
fn check() -> Result<(), String> {
    use std::time::{Duration, Instant};
    let mut capture =
        DxgiCapture::new().map_err(|error| format!("stage=OPEN {}", status(&error)))?;
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        match capture.next_frame(100) {
            Ok(Some(frame)) => {
                drop(frame);
                return Ok(());
            }
            Ok(None) => (),
            Err(error) => return Err(format!("stage=FRAME {}", status(&error))),
        }
    }
    Err("stage=FRAME NO_DESKTOP_FRAME".into())
}

fn main() -> std::process::ExitCode {
    use std::process::ExitCode;
    if !std::env::args()
        .skip(1)
        .eq(["--consent", "--local-only"].into_iter())
    {
        eprintln!("Local one-frame check, memory only. Run with --consent --local-only.");
        return ExitCode::from(2);
    }
    #[cfg(windows)]
    match check() {
        Ok(()) => {
            println!("DXGI_LOCAL_CHECK_PASS");
            ExitCode::SUCCESS
        }
        Err(code) => {
            eprintln!("DXGI_LOCAL_CHECK_FAILED {code}");
            ExitCode::FAILURE
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("DXGI_LOCAL_CHECK_FAILED WINDOWS_REQUIRED");
        ExitCode::FAILURE
    }
}

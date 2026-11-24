//! Explicit single-entry Linux diagnostic, not a libtest harness.

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    hub_webrtc::run_wss_signed_sdp_process_check()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("This synthetic signed SDP process diagnostic only runs on Linux.");
    std::process::ExitCode::from(2)
}

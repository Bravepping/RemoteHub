//! Explicit single-entry synthetic diagnostic, not a libtest harness.

#[cfg(any(target_os = "linux", all(windows, target_arch = "x86_64")))]
fn main() -> std::process::ExitCode {
    hub_webrtc::run_wss_signed_sdp_process_check()
}

#[cfg(not(any(target_os = "linux", all(windows, target_arch = "x86_64"))))]
fn main() -> std::process::ExitCode {
    eprintln!("This synthetic signed SDP process diagnostic only runs on Linux and Windows x64.");
    std::process::ExitCode::from(2)
}

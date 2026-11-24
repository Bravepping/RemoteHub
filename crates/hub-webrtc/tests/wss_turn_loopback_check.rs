//! Single entry for the opt-in synthetic loopback check. Not a libtest harness.
use std::{ffi::OsStr, process::ExitCode};

const ENABLE: &str = "--loopback-only";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(arg) = args.next() else {
        eprintln!("Manual synthetic diagnostic: run with {ENABLE}");
        return ExitCode::SUCCESS;
    };
    if arg != OsStr::new(ENABLE) || args.next().is_some() {
        eprintln!("Only {ENABLE} is accepted. No diagnostic was started.");
        return ExitCode::from(2);
    }
    println!("RemoteHub WSS/TURN diagnostic: localhost only, simulated input only.");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("could not start the loopback test runtime");
    runtime.block_on(hub_webrtc::run_wss_turn_loopback_check());
    drop(runtime);
    println!("WSS_TURN_LOOPBACK_PASS");
    ExitCode::SUCCESS
}

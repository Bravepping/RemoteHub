//! Single-entry wrapper for the native, loopback-only fake-input fixture.
//! This target does not register a libtest harness or expose a service CLI.
use std::{ffi::OsStr, process::ExitCode};

const ENABLE: &str = "--native-loopback-only";

#[cfg(windows)]
#[allow(dead_code, unused_imports)]
#[path = "../src/bin/v2_signaling.rs"]
mod fixture;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(arg) = args.next() else {
        // A routine Cargo test run must not open a native dialog.
        eprintln!("Manual check only: run with {ENABLE}");
        return ExitCode::SUCCESS;
    };
    if arg != OsStr::new(ENABLE) || args.next().is_some() {
        eprintln!("Only {ENABLE} is accepted.");
        return ExitCode::from(2);
    }

    #[cfg(windows)]
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("could not start the loopback test runtime");
        runtime.block_on(fixture::run_native_wss_consent_fixture());
        ExitCode::SUCCESS
    }
    #[cfg(not(windows))]
    {
        eprintln!("The native prompt is available only on Windows.");
        ExitCode::from(2)
    }
}

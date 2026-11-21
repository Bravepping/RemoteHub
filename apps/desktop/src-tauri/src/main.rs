#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use hub_capture_win::ProbeReport;
use serde::Serialize;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Default)]
struct ProbeState(AtomicBool);

struct ProbeLease<'a>(&'a AtomicBool);
impl Drop for ProbeLease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Serialize)]
struct RuntimeInfo {
    app_version: &'static str,
    os: &'static str,
    arch: &'static str,
    capture_backend: &'static str,
    remote_access_enabled: bool,
}

#[tauri::command]
fn get_runtime_info() -> RuntimeInfo {
    RuntimeInfo {
        app_version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        capture_backend: "DXGI Desktop Duplication (probe on demand)",
        remote_access_enabled: false,
    }
}

#[tauri::command]
async fn run_capture_probe(
    consent: bool,
    state: tauri::State<'_, std::sync::Arc<ProbeState>>,
) -> Result<ProbeReport, String> {
    if !consent {
        return Err("CONSENT_REQUIRED: 需要同意本次本机采集".into());
    }
    if state.0.swap(true, Ordering::AcqRel) {
        return Err("CAPTURE_BUSY: 已有采集实验正在进行".into());
    }
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = ProbeLease(&shared.0);
        // The first frame is discarded when returning. UI never receives raw desktop pixels.
        hub_capture_win::run_probe(consent, 30, Duration::from_secs(5))
            .map(|result| result.report)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn main() {
    tauri::Builder::default()
        .manage(std::sync::Arc::new(ProbeState::default()))
        .invoke_handler(tauri::generate_handler![
            get_runtime_info,
            run_capture_probe
        ])
        .run(tauri::generate_context!())
        .expect("RemoteHub desktop startup failed");
}

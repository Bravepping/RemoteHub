#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(any(feature = "p004-probe", feature = "p006-relay"))]
mod experiment_origin;
#[cfg(all(feature = "p004-probe", not(feature = "p006-relay")))]
mod p004;
#[cfg(all(feature = "p004-lan", not(feature = "p006-relay")))]
mod p004_lan;
#[cfg(feature = "p006-relay")]
mod p006;

#[cfg(not(feature = "p006-relay"))]
use hub_capture_win::ProbeReport;
use serde::Serialize;
#[cfg(not(feature = "p006-relay"))]
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[cfg(not(feature = "p006-relay"))]
#[derive(Default)]
struct ProbeState(AtomicBool);

#[cfg(not(feature = "p006-relay"))]
struct ProbeLease<'a>(&'a AtomicBool);
#[cfg(not(feature = "p006-relay"))]
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
    p004_enabled: bool,
    p004_lan_enabled: bool,
    p005_enabled: bool,
    p006_enabled: bool,
}

#[tauri::command]
fn get_runtime_info() -> RuntimeInfo {
    RuntimeInfo {
        app_version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        capture_backend: "DXGI Desktop Duplication (probe on demand)",
        remote_access_enabled: false,
        p004_enabled: cfg!(feature = "p004-probe"),
        p004_lan_enabled: cfg!(feature = "p004-lan"),
        p005_enabled: cfg!(feature = "p005-control"),
        p006_enabled: cfg!(feature = "p006-relay"),
    }
}

#[cfg(not(feature = "p006-relay"))]
#[tauri::command]
async fn run_capture_probe(
    window: tauri::WebviewWindow,
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
    let lease = ProbeLease(&shared.0);
    #[cfg(feature = "p004-probe")]
    {
        use tauri::Manager;
        if window.state::<std::sync::Arc<p004::P004State>>().busy() {
            return Err("CAPTURE_BUSY".into());
        }
    }
    #[cfg(feature = "p004-lan")]
    {
        use tauri::Manager;
        if window.state::<std::sync::Arc<p004_lan::LanState>>().busy() {
            return Err("CAPTURE_BUSY".into());
        }
    }
    let _ = window;
    // Hand the already reserved lease to the blocking capture task.
    std::mem::forget(lease);
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
    let builder = tauri::Builder::default();
    #[cfg(not(feature = "p006-relay"))]
    let builder = builder.manage(std::sync::Arc::new(ProbeState::default()));
    #[cfg(not(feature = "p004-probe"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        get_runtime_info,
        run_capture_probe
    ]);
    #[cfg(all(feature = "p004-probe", not(feature = "p004-lan")))]
    let builder = builder
        .manage(std::sync::Arc::new(p004::P004State::default()))
        .setup(|app| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                window.set_title("RemoteHub · P0-04a 本机视频实验")?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_runtime_info,
            run_capture_probe,
            p004::p004_start_loopback_probe,
            p004::p004_signal,
            p004::p004_get_stats,
            p004::p004_stop_loopback_probe,
            p004::p004_save_report
        ])
        .on_window_event(p004::on_window_event);
    #[cfg(all(feature = "p004-lan", not(feature = "p006-relay")))]
    let builder = builder
        .manage(std::sync::Arc::new(p004::P004State::default()))
        .manage(std::sync::Arc::new(p004_lan::LanState::default()))
        .setup(|app| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                window.set_title(if cfg!(feature = "p005-control") {
                    "RemoteHub · P0-05 视频与模拟控制实验 v1"
                } else {
                    "RemoteHub · P0-04b 双机视频实验 v2"
                })?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_runtime_info,
            run_capture_probe,
            p004::p004_start_loopback_probe,
            p004::p004_signal,
            p004::p004_get_stats,
            p004::p004_stop_loopback_probe,
            p004::p004_save_report,
            p004_lan::p004_lan_local_ip,
            p004_lan::p004_lan_start,
            p004_lan::p004_lan_signal,
            p004_lan::p004_lan_stats,
            p004_lan::p004_lan_metrics,
            p004_lan::p004_lan_stop,
            p004_lan::p004_lan_save
        ])
        .on_window_event(p004_lan::on_window_event);
    #[cfg(feature = "p006-relay")]
    let builder = builder
        .manage(std::sync::Arc::new(p006::State::default()))
        .setup(|app| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                window.set_title("RemoteHub · P0-06b 跨网络 TURN 实验 v1")?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_runtime_info,
            p006::p006_interfaces,
            p006::p006_start,
            p006::p006_ready,
            p006::p006_signal,
            p006::p006_stats,
            p006::p006_metrics,
            p006::p006_stop,
            p006::p006_save
        ])
        .on_window_event(p006::on_window_event);
    builder
        .run(tauri::generate_context!())
        .expect("RemoteHub desktop startup failed");
}

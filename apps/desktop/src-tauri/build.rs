fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_runtime_info",
            "run_capture_probe",
            "p004_start_loopback_probe",
            "p004_signal",
            "p004_get_stats",
            "p004_stop_loopback_probe",
            "p004_save_report",
            "p004_lan_local_ip",
            "p004_lan_start",
            "p004_lan_signal",
            "p004_lan_stats",
            "p004_lan_metrics",
            "p004_lan_stop",
            "p004_lan_save",
        ]),
    ))
    .expect("RemoteHub build configuration failed");
}

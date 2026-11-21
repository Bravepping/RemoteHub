fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_runtime_info",
            "run_capture_probe",
            "p004_start_loopback_probe",
            "p004_signal",
            "p004_get_stats",
            "p004_stop_loopback_probe",
        ]),
    ))
    .expect("failed to build RemoteHub command permissions");
}

// Native Wry integration probe. No capture, input, credentials, network or product CLI hooks.
#[cfg(not(feature = "p004-probe"))]
#[path = "../src/lifecycle.rs"]
mod lifecycle;
#[cfg(not(feature = "p004-probe"))]
#[path = "../src/lifecycle_policy.rs"]
mod lifecycle_policy;

#[cfg(not(feature = "p004-probe"))]
mod native {
    use super::lifecycle;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use tauri::Manager;
    type Checks = Arc<Mutex<std::collections::BTreeMap<&'static str, bool>>>;

    fn action<T: Send + 'static>(
        app: &tauri::AppHandle,
        f: impl FnOnce(tauri::AppHandle) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let _ = tx.send(f(handle));
        })
        .map_err(|_| "NATIVE_DISPATCH_FAILED")?;
        rx.recv_timeout(std::time::Duration::from_secs(4))
            .map_err(|_| "NATIVE_ACTION_TIMEOUT")?
    }
    fn record(checks: &Checks, name: &'static str, value: bool) -> Result<(), String> {
        checks
            .lock()
            .map_err(|_| "NATIVE_RECORD_FAILED")?
            .insert(name, value);
        if value {
            Ok(())
        } else {
            Err("NATIVE_OBSERVATION_FAILED".into())
        }
    }
    fn exercise(
        app: tauri::AppHandle,
        case: String,
        busy: Arc<AtomicBool>,
        checks: Checks,
    ) -> Result<(), String> {
        let tray = action(&app, |app| Ok(app.tray_by_id(lifecycle::TRAY_ID).is_some()))?;
        record(&checks, "actual_tray_created", tray)?;
        let visible = action(&app, |app| {
            let w = app.get_webview_window("main").ok_or("NO_NATIVE_WINDOW")?;
            w.show().map_err(|_| "NATIVE_SHOW_FAILED")?;
            w.is_visible().map_err(|_| "NATIVE_VISIBLE_FAILED".into())
        })?;
        record(&checks, "actual_window_visible", visible)?;
        if case == "resident-menu-exit" || case == "disable-then-close" {
            action(&app, |app| lifecycle::set_resident(&app, true))?;
            record(
                &checks,
                "resident_capture_rejected",
                action(&app, |app| {
                    Ok(app
                        .state::<Arc<lifecycle::Lifecycle>>()
                        .reserve_capture()
                        .is_err())
                })?,
            )?;
            action(&app, |app| {
                app.get_webview_window("main")
                    .unwrap()
                    .close()
                    .map_err(|_| "NATIVE_CLOSE_FAILED".into())
            })?;
            std::thread::sleep(std::time::Duration::from_millis(150));
            record(
                &checks,
                "x_hid_instead_of_destroyed",
                action(&app, |app| {
                    Ok(app
                        .get_webview_window("main")
                        .is_some_and(|w| matches!(w.is_visible(), Ok(false))))
                })?,
            )?;
            action(&app, |app| {
                lifecycle::show(&app)?;
                lifecycle::show(&app)
            })?;
            record(
                &checks,
                "show_twice_visible",
                action(&app, |app| {
                    Ok(app
                        .get_webview_window("main")
                        .is_some_and(|w| matches!(w.is_visible(), Ok(true))))
                })?,
            )?;
            if case == "disable-then-close" {
                action(&app, |app| {
                    lifecycle::set_resident(&app, false)?;
                    lifecycle::set_resident(&app, false)
                })?;
                action(&app, |app| {
                    app.get_webview_window("main")
                        .unwrap()
                        .close()
                        .map_err(|_| "NATIVE_CLOSE_FAILED".into())
                })?;
            } else {
                action(&app, |app| {
                    app.get_webview_window("main")
                        .unwrap()
                        .close()
                        .map_err(|_| "NATIVE_CLOSE_FAILED".into())
                })?;
                std::thread::sleep(std::time::Duration::from_millis(100));
                record(
                    &checks,
                    "hidden_before_explicit_exit",
                    action(&app, |app| {
                        Ok(app
                            .get_webview_window("main")
                            .is_some_and(|w| matches!(w.is_visible(), Ok(false))))
                    })?,
                )?;
                action(&app, |app| {
                    lifecycle::request_exit(&app);
                    lifecycle::request_exit(&app);
                    Ok(())
                })?;
            }
        } else if case == "busy-close" {
            action(&app, |app| {
                app.state::<Arc<lifecycle::Lifecycle>>().reserve_capture()
            })?;
            record(
                &checks,
                "busy_residency_rejected",
                action(&app, |app| Ok(lifecycle::set_resident(&app, true).is_err()))?,
            )?;
            action(&app, |app| {
                app.get_webview_window("main")
                    .unwrap()
                    .close()
                    .map_err(|_| "NATIVE_CLOSE_FAILED".into())
            })?;
            std::thread::sleep(std::time::Duration::from_millis(150));
            record(
                &checks,
                "exit_waited_for_busy_lease",
                action(&app, |app| {
                    Ok(app.state::<Arc<lifecycle::Lifecycle>>().exiting()
                        && app.get_webview_window("main").is_some())
                })?,
            )?;
            record(
                &checks,
                "exit_rejects_new_capture",
                action(&app, |app| {
                    Ok(app
                        .state::<Arc<lifecycle::Lifecycle>>()
                        .reserve_capture()
                        .is_err())
                })?,
            )?;
            busy.store(false, Ordering::Release);
        } else {
            action(&app, |app| {
                app.get_webview_window("main")
                    .unwrap()
                    .close()
                    .map_err(|_| "NATIVE_CLOSE_FAILED".into())
            })?;
        }
        Ok(())
    }

    pub(super) fn run() {
        let case = std::env::args().nth(1).unwrap_or_default();
        if ![
            "default-close",
            "resident-menu-exit",
            "disable-then-close",
            "busy-close",
        ]
        .contains(&case.as_str())
            || std::env::args().len() != 2
        {
            eprintln!("NATIVE_PROBE_CASE_REQUIRED");
            std::process::exit(2);
        }
        // Test-only watchdog: forced termination is FAIL, never evidence of normal resource cleanup.
        let finished = Arc::new(AtomicBool::new(false));
        let watchdog = finished.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(15));
            if !watchdog.load(Ordering::Acquire) {
                std::process::exit(3);
            }
        });
        let busy = Arc::new(AtomicBool::new(false));
        let checks: Checks = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows[0].visible = false;
        context.config_mut().app.windows[0].title = "RemoteHub · 本机无媒体生命周期验证".into();
        let app = lifecycle::install(tauri::Builder::default(), busy.clone())
            .build(context)
            .expect("NATIVE_PROBE_BUILD_FAILED");
        let callback_checks = checks.clone();
        let callback_case = case.clone();
        let exit_code = app.run_return(move |app, event| {
            lifecycle::run_event(app, &event);
            match event {
                tauri::RunEvent::Ready => {
                    let app = app.clone();
                    let busy = busy.clone();
                    let checks = callback_checks.clone();
                    let case = callback_case.clone();
                    std::thread::spawn(move || {
                        if exercise(app.clone(), case, busy.clone(), checks.clone()).is_err() {
                            let _ = record(&checks, "exercise_succeeded", false);
                            busy.store(false, Ordering::Release);
                            lifecycle::request_exit(&app);
                        }
                    });
                }
                tauri::RunEvent::WindowEvent {
                    event: tauri::WindowEvent::CloseRequested { .. },
                    ..
                } => {
                    let _ = record(&callback_checks, "actual_close_event", true);
                }
                tauri::RunEvent::ExitRequested { .. } => {
                    let _ = record(&callback_checks, "actual_exit_requested", true);
                }
                tauri::RunEvent::Exit => {
                    let _ = record(&callback_checks, "actual_exit_event", true);
                    let _ = record(
                        &callback_checks,
                        "tray_removed_before_process_exit",
                        app.tray_by_id(lifecycle::TRAY_ID).is_none(),
                    );
                }
                _ => {}
            }
        });
        finished.store(true, Ordering::Release);
        let checks = checks.lock().unwrap();
        let passed = exit_code == 0
            && checks.values().all(|v| *v)
            && [
                "actual_tray_created",
                "actual_window_visible",
                "actual_close_event",
                "actual_exit_requested",
                "actual_exit_event",
                "tray_removed_before_process_exit",
            ]
            .iter()
            .all(|key| checks.get(key) == Some(&true));
        let fields = checks
            .iter()
            .map(|(k, v)| format!("\"{k}\":{v}"))
            .collect::<Vec<_>>()
            .join(",");
        println!("{{\"case\":\"{case}\",\"passed\":{passed},\"checks\":{{{fields}}},\"source\":\"Actual Windows Tauri/Wry/tray API-driven integration; no manual menu clicks, capture or input\"}}");
        if !passed {
            std::process::exit(1);
        }
    }
}
#[cfg(not(feature = "p004-probe"))]
fn main() {
    native::run();
}
#[cfg(feature = "p004-probe")]
fn main() {
    eprintln!("NATIVE_LIFECYCLE_PROBE_REQUIRES_DEFAULT_BUILD");
    std::process::exit(2);
}

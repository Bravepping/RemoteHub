use crate::lifecycle_policy::{CloseAction, Policy};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

pub(crate) const TRAY_ID: &str = "remotehub-main-tray";
const RESIDENT_ID: &str = "remotehub-resident";
const SHOW_ID: &str = "remotehub-show";
const EXIT_ID: &str = "remotehub-exit";

pub(crate) struct Lifecycle {
    policy: Mutex<Policy>,
    busy: Arc<AtomicBool>,
    resident_item: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
}

impl Lifecycle {
    pub(crate) fn reserve_capture(&self) -> Result<(), String> {
        let policy = self
            .policy
            .lock()
            .map_err(|_| "LIFECYCLE_STATE_UNAVAILABLE")?;
        policy.allow_capture()?;
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err("CAPTURE_BUSY".into());
        }
        Ok(())
    }

    pub(crate) fn exiting(&self) -> bool {
        self.policy.lock().map(|p| p.exiting).unwrap_or(true)
    }
}

pub(crate) fn show(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<Arc<Lifecycle>>();
    {
        let policy = state
            .policy
            .lock()
            .map_err(|_| "LIFECYCLE_STATE_UNAVAILABLE")?;
        if policy.exiting {
            return Err("APPLICATION_EXITING".into());
        }
    }
    // A timeout worker may call show: never hold policy while waiting for the UI thread.
    show_window(app)
}

fn show_window(app: &tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("MAIN_WINDOW_UNAVAILABLE")?;
    window.show().map_err(|_| "WINDOW_SHOW_FAILED")?;
    window.unminimize().map_err(|_| "WINDOW_SHOW_FAILED")?;
    window.set_focus().map_err(|_| "WINDOW_FOCUS_FAILED")?;
    Ok(())
}

pub(crate) fn set_resident(app: &tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let state = app.state::<Arc<Lifecycle>>();
    let mut policy = state
        .policy
        .lock()
        .map_err(|_| "LIFECYCLE_STATE_UNAVAILABLE")?;
    if policy.exiting {
        return Err("APPLICATION_EXITING".into());
    }
    // This action runs on the native menu/UI thread. Keep capture gated until visible.
    if !enabled && show_window(app).is_err() {
        drop(policy);
        request_exit(app);
        return Err("WINDOW_SHOW_FAILED".into());
    }
    policy.set_resident(enabled, state.busy.load(Ordering::Acquire))?;
    let item = state
        .resident_item
        .lock()
        .map_err(|_| "LIFECYCLE_STATE_UNAVAILABLE")?
        .clone();
    if let Some(item) = item {
        if item.set_checked(enabled).is_err() {
            policy.resident = false;
            return Err("TRAY_RESIDENCY_UNAVAILABLE".into());
        }
    }
    Ok(())
}

pub(crate) fn request_exit(app: &tauri::AppHandle) {
    let state = app.state::<Arc<Lifecycle>>().inner().clone();
    let Ok(mut policy) = state.policy.lock() else {
        return;
    };
    if !policy.begin_exit() {
        return;
    }
    drop(policy);
    let item = state
        .resident_item
        .lock()
        .ok()
        .and_then(|item| item.clone());
    if let Some(item) = item {
        let _ = item.set_checked(false);
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(7);
        while state.busy.load(Ordering::Acquire) {
            if std::time::Instant::now() >= deadline {
                if let Ok(mut policy) = state.policy.lock() {
                    policy.exiting = false;
                }
                if let Some(tray) = app.tray_by_id(TRAY_ID) {
                    let _ = tray.set_tooltip(Some("RemoteHub：本机任务尚未结束，请稍后重试退出"));
                }
                let _ = show(&app);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.exit(0);
    });
}

fn close(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let state = window.state::<Arc<Lifecycle>>();
        let action = state
            .policy
            .lock()
            .map(|p| p.close_action(state.busy.load(Ordering::Acquire)))
            .unwrap_or(CloseAction::Exit);
        match action {
            CloseAction::Hide => {
                if window.hide().is_err() {
                    request_exit(window.app_handle());
                }
            }
            CloseAction::Exit => request_exit(window.app_handle()),
        }
    }
}

pub(crate) fn run_event(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    if matches!(event, tauri::RunEvent::Exit) {
        let _ = app.remove_tray_by_id(TRAY_ID);
    }
    if let tauri::RunEvent::ExitRequested { api, .. } = event {
        let state = app.state::<Arc<Lifecycle>>();
        if !state.exiting() || state.busy.load(Ordering::Acquire) {
            api.prevent_exit();
            request_exit(app);
        }
    }
}

pub(crate) fn install(
    builder: tauri::Builder<tauri::Wry>,
    busy: Arc<AtomicBool>,
) -> tauri::Builder<tauri::Wry> {
    builder
        .manage(Arc::new(Lifecycle {
            policy: Mutex::new(Policy::default()),
            busy,
            resident_item: Mutex::new(None),
        }))
        .setup(|app| {
            // A missing tray must never turn X into an unrecoverable hidden process.
            let result = (|| -> tauri::Result<()> {
                let show_item =
                    MenuItem::with_id(app, SHOW_ID, "显示 RemoteHub", true, None::<&str>)?;
                let resident = CheckMenuItem::with_id(
                    app,
                    RESIDENT_ID,
                    "本次允许托盘驻留",
                    true,
                    false,
                    None::<&str>,
                )?;
                if let Ok(mut item) = app.state::<Arc<Lifecycle>>().resident_item.lock() {
                    *item = Some(resident.clone());
                }
                let exit_item =
                    MenuItem::with_id(app, EXIT_ID, "真正退出 RemoteHub", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show_item, &resident, &exit_item])?;
                let icon = app
                    .default_window_icon()
                    .ok_or_else(|| tauri::Error::AssetNotFound("default-window-icon".into()))?
                    .clone();
                TrayIconBuilder::with_id(TRAY_ID)
                    .icon(icon)
                    .tooltip("RemoteHub：默认关窗退出；可主动开启本次驻留")
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .on_menu_event(move |app, event| match event.id().as_ref() {
                        SHOW_ID => {
                            let _ = show(app);
                        }
                        EXIT_ID => request_exit(app),
                        RESIDENT_ID => {
                            let enabled = resident.is_checked().unwrap_or(false);
                            if set_resident(app, enabled).is_err() {
                                let _ = resident.set_checked(false);
                            }
                        }
                        _ => {}
                    })
                    .build(app)?;
                Ok(())
            })();
            app.state::<Arc<Lifecycle>>()
                .policy
                .lock()
                .map_err(|_| "LIFECYCLE_STATE_UNAVAILABLE")?
                .tray_available = result.is_ok();
            Ok(())
        })
        .on_window_event(close)
}

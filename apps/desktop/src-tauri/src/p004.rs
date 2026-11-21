use hub_webrtc::session::{Session, Stats};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{Manager, WebviewWindow};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO},
    },
};

struct Active {
    id: String,
    session: Arc<Session>,
}
#[derive(Default)]
pub struct P004State {
    active: Mutex<Option<Active>>,
    starting: AtomicBool,
    closing: AtomicBool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    consent: bool,
    seconds: u32,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
}
impl Config {
    fn validate(&self) -> Result<(), String> {
        if !self.consent {
            return Err("CONSENT_REQUIRED".into());
        }
        if !(5..=30).contains(&self.seconds)
            || self.width != 1280
            || self.height != 720
            || self.fps != 30
            || self.bitrate_bps != 4_000_000
        {
            return Err("INVALID_PROBE_CONFIG".into());
        }
        Ok(())
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    session_id: String,
}
struct StartingLease<'a>(&'a AtomicBool);
impl Drop for StartingLease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(crate) use crate::experiment_origin::origin;
#[cfg(test)]
use crate::experiment_origin::origin_url;
impl P004State {
    pub(crate) fn busy(&self) -> bool {
        self.starting.load(Ordering::Acquire)
            || self
                .active
                .lock()
                .map(|a| {
                    a.as_ref()
                        .is_some_and(|a| !a.session.stats().resources_released)
                })
                .unwrap_or(true)
    }
    fn reserve(&self) -> Result<StartingLease<'_>, String> {
        if self.starting.swap(true, Ordering::AcqRel) {
            return Err("BUSY".into());
        }
        let lease = StartingLease(&self.starting);
        if self.closing.load(Ordering::Acquire) {
            return Err("WINDOW_CLOSING".into());
        }
        if self
            .active
            .lock()
            .map_err(|_| "STATE_LOCK_FAILED")?
            .as_ref()
            .is_some_and(|a| !a.session.stats().resources_released)
        {
            return Err("BUSY".into());
        }
        Ok(lease)
    }
}
fn find(state: &P004State, id: &str) -> Result<Arc<Session>, String> {
    state
        .active
        .lock()
        .map_err(|_| "STATE_LOCK_FAILED")?
        .as_ref()
        .filter(|a| a.id == id)
        .map(|a| a.session.clone())
        .ok_or("INVALID_SESSION_ID".into())
}

#[tauri::command]
pub async fn p004_start_loopback_probe(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<P004State>>,
    config: Config,
) -> Result<Started, String> {
    origin(&window)?;
    config.validate()?;
    let gate = window.state::<Arc<crate::ProbeState>>();
    if gate.0.swap(true, Ordering::AcqRel) {
        return Err("CAPTURE_BUSY".into());
    }
    let _gate = crate::ProbeLease(&gate.0);
    #[cfg(feature = "p004-lan")]
    if window.state::<Arc<crate::p004_lan::LanState>>().busy() {
        return Err("LAN_BUSY".into());
    }
    let _lease = state.reserve()?;
    let hwnd = window.hwnd().map_err(|_| "NATIVE_WINDOW_REQUIRED")?.0 as usize;
    let seconds = config.seconds;
    let allowed=tauri::async_runtime::spawn_blocking(move || unsafe {
        MessageBoxW(Some(HWND(hwnd as *mut _)),
            &HSTRING::from(format!("本次将采集主屏并仅在这台电脑的实验窗口内实时播放，最长 {seconds} 秒。不会保存画面或连接其他电脑。\n\n是否同意本次实验？")),
            &HSTRING::from("RemoteHub · 本次本机视频实验"), MB_YESNO|MB_DEFBUTTON2|MB_ICONQUESTION)==IDYES
    }).await.map_err(|_| "CONSENT_DIALOG_FAILED")?;
    if !allowed {
        return Err("NATIVE_CONSENT_DENIED".into());
    }
    if state.closing.load(Ordering::Acquire) {
        return Err("WINDOW_CLOSING".into());
    }
    let session = Session::new(seconds)?;
    let id = uuid::Uuid::new_v4().to_string();
    *state.active.lock().map_err(|_| "STATE_LOCK_FAILED")? = Some(Active {
        id: id.clone(),
        session: session.clone(),
    });
    // Expire unused authorizations and abandoned negotiations, independently of JS timers.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(12)).await;
        if session.stats().state == "starting" {
            let _ = session.stop().await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(seconds as u64 + 15)).await;
        if !session.stats().resources_released {
            let _ = session.stop().await;
        }
    });
    Ok(Started { session_id: id })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_gate_rejects_missing_consent_concurrent_start_and_foreign_session() {
        let mut config = Config {
            consent: false,
            seconds: 10,
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 4_000_000,
        };
        assert_eq!(config.validate().unwrap_err(), "CONSENT_REQUIRED");
        config.consent = true;
        config.seconds = 31;
        assert!(config.validate().is_err());
        config.seconds = 10;
        assert!(config.validate().is_ok());
        let state = P004State::default();
        let lease = state.reserve().unwrap();
        assert!(state.reserve().is_err());
        drop(lease);
        let session = Session::new(10).unwrap();
        *state.active.lock().unwrap() = Some(Active {
            id: "owned".into(),
            session: session.clone(),
        });
        assert!(state.reserve().is_err());
        assert!(find(&state, "foreign").is_err());
        session.stop().await.unwrap();
        assert!(state.reserve().is_ok());
        let local = tauri::Url::parse("http://tauri.localhost/").unwrap();
        assert!(origin_url("main", &local).is_ok());
        assert!(origin_url("other", &local).is_err());
        assert!(origin_url("main", &tauri::Url::parse("https://example.com/").unwrap()).is_err());
    }
}

#[tauri::command]
pub async fn p004_signal(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<P004State>>,
    session_id: String,
    offer: String,
) -> Result<String, String> {
    origin(&window)?;
    let session = find(&state, &session_id)?;
    match session.signal(offer).await {
        Ok(answer) => Ok(answer),
        Err(error) => {
            let _ = session.stop().await;
            Err(error)
        }
    }
}
#[tauri::command]
pub fn p004_get_stats(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<P004State>>,
    session_id: String,
) -> Result<Stats, String> {
    origin(&window)?;
    Ok(find(&state, &session_id)?.stats())
}
#[tauri::command]
pub async fn p004_stop_loopback_probe(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<P004State>>,
    session_id: String,
) -> Result<(), String> {
    origin(&window)?;
    find(&state, &session_id)?.stop().await
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiverSnapshot {
    elapsed_ms: f64,
    frames_decoded: Option<u64>,
    frames_received: Option<u64>,
    packets_received: Option<u64>,
    packets_lost: Option<i64>,
    bytes_received: Option<u64>,
    video_width: u32,
    video_height: u32,
    frames_rendered: Option<u64>,
    current_time: f64,
    first_frame_ms: Option<f64>,
    connection_state: String,
    ice_state: String,
    codec: Option<String>,
    fmtp: Option<String>,
    selected_remote_loopback: Option<bool>,
    local_candidate_type: Option<String>,
    remote_candidate_type: Option<String>,
    freeze_count: Option<u64>,
    total_freezes_duration: Option<f64>,
}
#[tauri::command]
pub fn p004_save_report(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<P004State>>,
    session_id: String,
    receiver: Vec<ReceiverSnapshot>,
    browser_closed: bool,
) -> Result<String, String> {
    origin(&window)?;
    if receiver.len() > 60 {
        return Err("REPORT_TOO_LARGE".into());
    }
    let session = find(&state, &session_id)?;
    let data = serde_json::to_vec_pretty(
        &serde_json::json!({"sessionId":session_id,"native":session.stats(),"receiver":receiver,"browserClosed":browser_closed}),
    )
    .map_err(|_| "REPORT_SERIALIZATION_FAILED")?;
    if data.len() > 128 * 1024 {
        return Err("REPORT_TOO_LARGE".into());
    }
    let dir = report_directory()?;
    std::fs::create_dir_all(&dir).map_err(|_| "REPORT_DIRECTORY_FAILED")?;
    let name = format!("{session_id}.json");
    use std::io::Write;
    std::fs::File::create_new(dir.join(&name))
        .and_then(|mut f| f.write_all(&data))
        .map_err(|_| "REPORT_EXISTS_OR_WRITE_FAILED")?;
    Ok(format!("artifacts/private/p004/{name}"))
}

fn report_directory() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|_| "REPORT_DIRECTORY_UNAVAILABLE")?;
    Ok(exe
        .parent()
        .ok_or("REPORT_DIRECTORY_UNAVAILABLE")?
        .join("artifacts/private/p004"))
}

#[cfg(feature = "p004-lan")]
pub(crate) async fn close_for_lan(state: &P004State) -> bool {
    state.closing.store(true, Ordering::Release);
    let session = state
        .active
        .lock()
        .ok()
        .and_then(|s| s.as_ref().map(|a| a.session.clone()));
    if let Some(session) = session {
        session.stop().await.is_ok() && session.stats().resources_released
    } else {
        true
    }
}

#[cfg(not(feature = "p004-lan"))]
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        let state = window.state::<Arc<P004State>>();
        api.prevent_close();
        if state.closing.swap(true, Ordering::AcqRel) {
            return;
        }
        let session = state
            .active
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|a| (a.id.clone(), a.session.clone())));
        let window = window.clone();
        let shared = state.inner().clone();
        tauri::async_runtime::spawn(async move {
            let clean = if let Some((id, session)) = session {
                let clean = session.stop().await.is_ok() && session.stats().resources_released;
                if let Ok(dir) = report_directory() {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Ok(data) = serde_json::to_vec_pretty(
                        &serde_json::json!({"event":"window-close","native":session.stats(),"cleanupSucceeded":clean}),
                    ) {
                        use std::io::Write;
                        if let Ok(mut file) =
                            std::fs::File::create_new(dir.join(format!("{id}-window-close.json")))
                        {
                            let _ = file.write_all(&data);
                        }
                    }
                }
                clean
            } else {
                true
            };
            if clean {
                let _ = window.destroy();
            } else {
                shared.closing.store(false, Ordering::Release);
            }
        });
    }
}

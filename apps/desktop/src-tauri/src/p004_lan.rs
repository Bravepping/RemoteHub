use hub_webrtc::{
    lan::{LanSession, LanStatus},
    scope::LanScope,
};
use serde::{Deserialize, Serialize};
use std::{
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
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
    session: Arc<LanSession>,
}
#[derive(Default)]
pub struct LanState {
    active: Mutex<Option<Active>>,
    starting: AtomicBool,
    closing: AtomicBool,
}
impl LanState {
    pub(crate) fn busy(&self) -> bool {
        self.starting.load(Ordering::Acquire)
            || self.closing.load(Ordering::Acquire)
            || self
                .active
                .lock()
                .map(|s| {
                    s.as_ref()
                        .is_some_and(|a| !a.session.stats().resources_released)
                })
                .unwrap_or(true)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    control_sim: bool,
    #[serde(default)]
    real_control: bool,
    role: String,
    consent: bool,
    local_ip: String,
    peer_ip: String,
    seconds: u32,
    port: u16,
    pairing_code: Option<String>,
}
impl Config {
    fn validate(&self) -> Result<LanScope, String> {
        if self.control_sim && !cfg!(feature = "p005-control") {
            return Err("CONTROL_FEATURE_DISABLED".into());
        }
        if self.real_control && (!cfg!(feature = "p3-lan") || self.control_sim) {
            return Err("INPUT_FEATURE_OR_MODE_DENIED".into());
        }
        if !self.consent {
            return Err("CONSENT_REQUIRED".into());
        }
        if !["sender", "receiver"].contains(&self.role.as_str())
            || !(5..=30).contains(&self.seconds)
            || self.port < 1024
        {
            return Err("INVALID_LAN_CONFIG".into());
        }
        LanScope::new(
            self.local_ip.parse().map_err(|_| "INVALID_LOCAL_IP")?,
            self.peer_ip.parse().map_err(|_| "INVALID_PEER_IP")?,
        )
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    control_session: Option<String>,
    session_id: String,
    pairing_code: Option<String>,
}
struct Lease<'a>(&'a AtomicBool);
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn find(state: &LanState, id: &str) -> Result<Arc<LanSession>, String> {
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
pub fn p004_lan_local_ip(window: WebviewWindow, peer_ip: String) -> Result<String, String> {
    crate::p004::origin(&window)?;
    let peer: Ipv4Addr = peer_ip.parse().map_err(|_| "INVALID_PEER_IP")?;
    if !peer.is_private() {
        return Err("PRIVATE_IPV4_REQUIRED".into());
    }
    // connect() on an unconnected UDP socket selects the route; no datagram is sent.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|_| "ROUTE_UNAVAILABLE")?;
    socket
        .connect((peer, 48604))
        .map_err(|_| "ROUTE_UNAVAILABLE")?;
    let std::net::IpAddr::V4(local) = socket.local_addr().map_err(|_| "ROUTE_UNAVAILABLE")?.ip()
    else {
        return Err("IPV4_REQUIRED".into());
    };
    LanScope::new(local, peer)?;
    Ok(local.to_string())
}

#[tauri::command]
pub async fn p004_lan_start(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<LanState>>,
    config: Config,
) -> Result<Started, String> {
    crate::p004::origin(&window)?;
    let scope = config.validate()?;
    let gate = window.state::<Arc<crate::ProbeState>>();
    if gate.0.swap(true, Ordering::AcqRel) {
        return Err("CAPTURE_BUSY".into());
    }
    let _gate = crate::ProbeLease(&gate.0);
    if state.busy() || window.state::<Arc<crate::p004::P004State>>().busy() {
        return Err("BUSY".into());
    }
    state.starting.store(true, Ordering::Release);
    let _lease = Lease(&state.starting);
    if config.role == "receiver" {
        hub_webrtc::lan_wire::Key::parse(config.pairing_code.as_deref().unwrap_or_default())?;
    }
    // Validate ownership of the selected interface before asking for consent.
    let check = std::net::UdpSocket::bind((scope.local, 0)).map_err(|_| "LAN_LOCAL_BIND_FAILED")?;
    drop(check);
    let hwnd = window.hwnd().map_err(|_| "NATIVE_WINDOW_REQUIRED")?.0 as usize;
    let action = if config.role == "sender" {
        "采集这台电脑的主屏并发送给"
    } else {
        "接收并播放来自"
    };
    let control_text = if config.real_control {
        if config.role == "sender" {
            "\n允许指定对端在本次短会话通过 DataChannel 实际操作本机主屏鼠标和基本键盘。可随时点停止、关窗或在本机按 Ctrl+Alt+F12 紧急结束。不会授予无人值守或管理员权限。"
        } else {
            "\n本机将接收另一台主屏画面；只有你主动启用本次键鼠控制后，操作才经 DataChannel 发送给被控电脑。本机不会接收输入注入。Esc或画面失焦会暂停控制。"
        }
    } else if config.control_sim {
        "\n同时允许 control.v1 通道传输模拟鼠标坐标与确认，仅记录统计，不执行实际键盘鼠标输入。"
    } else {
        ""
    };
    let message=format!("本次将{action} {}，视频最长 {} 秒。仅连接这一台指定电脑。{control_text}\n\n发送端最多等待配对 120 秒；连接中断会停止采集。不会保存画面。\n\n是否同意本次实验？",scope.peer,config.seconds);
    let allowed = tauri::async_runtime::spawn_blocking(move || unsafe {
        MessageBoxW(
            Some(HWND(hwnd as *mut _)),
            &HSTRING::from(message),
            &HSTRING::from(if cfg!(feature = "p3-lan") {
                "RemoteHub · 本次局域网远控授权"
            } else {
                "RemoteHub · 本次双机视频实验"
            }),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        ) == IDYES
    })
    .await
    .map_err(|_| "CONSENT_DIALOG_FAILED")?;
    if !allowed {
        return Err("NATIVE_CONSENT_DENIED".into());
    }
    if state.closing.load(Ordering::Acquire) {
        return Err("WINDOW_CLOSING".into());
    }
    let (session, code) = if config.role == "sender" {
        #[cfg(feature = "p3-lan")]
        let input = if config.real_control {
            Some(LanSession::host_input(scope, config.seconds, config.port).await?)
        } else {
            None
        };
        #[cfg(not(feature = "p3-lan"))]
        let input: Option<(Arc<LanSession>, String)> = None;
        let (s, c) = if let Some(input) = input {
            input
        } else if config.control_sim {
            LanSession::host_control(scope, config.seconds, config.port).await?
        } else {
            LanSession::host(scope, config.seconds, config.port).await?
        };
        (s, Some(c))
    } else {
        (
            if config.real_control {
                #[cfg(feature = "p3-lan")]
                {
                    LanSession::receiver_input(
                        scope,
                        config.seconds,
                        config.port,
                        config.pairing_code.as_deref().unwrap_or_default(),
                    )?
                }
                #[cfg(not(feature = "p3-lan"))]
                {
                    return Err("INPUT_FEATURE_DISABLED".into());
                }
            } else if config.control_sim {
                LanSession::receiver_control(
                    scope,
                    config.seconds,
                    config.port,
                    config.pairing_code.as_deref().unwrap_or_default(),
                )?
            } else {
                LanSession::receiver(
                    scope,
                    config.seconds,
                    config.port,
                    config.pairing_code.as_deref().unwrap_or_default(),
                )?
            },
            None,
        )
    };
    let id = uuid::Uuid::new_v4().to_string();
    *state.active.lock().map_err(|_| "STATE_LOCK_FAILED")? = Some(Active {
        id: id.clone(),
        session: session.clone(),
    });
    let control_session = session.control_session();
    let report_id = id.clone();
    let receiver = config.role == "receiver";
    #[cfg(feature = "p3-lan")]
    let input_host = config.real_control && config.role == "sender";
    tauri::async_runtime::spawn(async move {
        for tick in 0..320 {
            #[cfg(feature = "p3-lan")]
            if input_host && hub_webrtc::remote_input::Injector::emergency_pressed() {
                session.cancel();
                let _ = session.stop().await;
            }
            let snapshot = session.stats();
            let _ = write_report(&report_id, "progress", &snapshot, None);
            if snapshot.resources_released {
                let _ = write_report(&report_id, "native-final", &snapshot, None);
                return;
            }
            if (receiver && tick == 24 && snapshot.state == "waiting") || tick == 310 {
                let _ = session.stop().await;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
    Ok(Started {
        control_session,
        session_id: id,
        pairing_code: code,
    })
}
#[tauri::command]
pub async fn p004_lan_signal(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<LanState>>,
    session_id: String,
    offer: String,
) -> Result<String, String> {
    crate::p004::origin(&window)?;
    let session = find(&state, &session_id)?;
    match session.signal_receiver(offer).await {
        Ok(answer) => Ok(answer),
        Err(error) => {
            let _ = session.stop().await;
            Err(error)
        }
    }
}
#[tauri::command]
pub fn p004_lan_stats(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<LanState>>,
    session_id: String,
) -> Result<LanStatus, String> {
    crate::p004::origin(&window)?;
    Ok(find(&state, &session_id)?.stats())
}
#[tauri::command]
pub fn p004_lan_metrics(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<LanState>>,
    session_id: String,
    data: serde_json::Value,
) -> Result<(), String> {
    crate::p004::origin(&window)?;
    find(&state, &session_id)?.metrics(data)
}
#[tauri::command]
pub async fn p004_lan_stop(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<LanState>>,
    session_id: String,
) -> Result<(), String> {
    crate::p004::origin(&window)?;
    find(&state, &session_id)?.stop().await
}
#[tauri::command]
pub fn p004_lan_save(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<LanState>>,
    session_id: String,
    browser_closed: bool,
    control: Option<hub_webrtc::control::BrowserStats>,
    receiver_final: Option<serde_json::Value>,
    candidate_summary: Option<CandidateSummary>,
) -> Result<String, String> {
    crate::p004::origin(&window)?;
    let session = find(&state, &session_id)?;
    if let Some(control) = control {
        session.record_browser_control(control)?;
    }
    let status = session.stats();
    if !status.resources_released {
        return Err("LAN_CLEANUP_PENDING".into());
    }
    if let Some(value) = &receiver_final {
        if status.role != "receiver" {
            return Err("RECEIVER_FINAL_ROLE_DENIED".into());
        }
        hub_webrtc::lan::validate_metrics(value)?;
    }
    if let Some(value) = &candidate_summary {
        if !cfg!(feature = "p3-lan") || status.role != "receiver" {
            return Err("CANDIDATE_SUMMARY_ROLE_DENIED".into());
        }
        value.validate()?;
    }
    write_report_with_final(
        &session_id,
        "final",
        &status,
        Some(browser_closed),
        receiver_final.as_ref(),
        candidate_summary.as_ref(),
    )
}
fn write_report(
    id: &str,
    event: &str,
    status: &LanStatus,
    browser_closed: Option<bool>,
) -> Result<String, String> {
    write_report_with_final(id, event, status, browser_closed, None, None)
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateSummary {
    before: u16,
    mdns: u16,
    published: u16,
    mdns_published: u16,
}
impl CandidateSummary {
    fn validate(&self) -> Result<(), String> {
        if self.before > 128
            || self.mdns > self.before
            || self.published > self.before
            || self.mdns_published > self.mdns
            || self.mdns_published > self.published
        {
            return Err("CANDIDATE_SUMMARY_INVALID".into());
        }
        Ok(())
    }
}
fn write_report_with_final(
    id: &str,
    event: &str,
    status: &LanStatus,
    browser_closed: Option<bool>,
    receiver_final: Option<&serde_json::Value>,
    candidate_summary: Option<&CandidateSummary>,
) -> Result<String, String> {
    let dir = std::env::current_exe()
        .map_err(|_| "REPORT_DIRECTORY_UNAVAILABLE")?
        .parent()
        .ok_or("REPORT_DIRECTORY_UNAVAILABLE")?
        .join(if cfg!(feature = "p3-lan") {
            "artifacts/private/p3"
        } else if cfg!(feature = "p005-control") {
            "artifacts/private/p005"
        } else {
            "artifacts/private/p004b"
        });
    std::fs::create_dir_all(&dir).map_err(|_| "REPORT_DIRECTORY_FAILED")?;
    let name = format!("{id}-{event}.json");
    let tag = if cfg!(feature = "p3-lan") {
        "p3-lan-v3"
    } else if cfg!(feature = "p005-control") {
        "p005-v1"
    } else {
        "p004b-v2"
    };
    let data=serde_json::to_vec_pretty(&serde_json::json!({"event":event,"buildTag":tag,"appVersion":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"native":status,"browserClosed":browser_closed,"receiverFinal":receiver_final,"browserCandidateSummary":candidate_summary})).map_err(|_|"REPORT_SERIALIZATION_FAILED")?;
    if data.len() > 128 * 1024 {
        return Err("REPORT_TOO_LARGE".into());
    }
    use std::io::Write;
    let path = dir.join(&name);
    let mut file = if event == "progress" {
        std::fs::File::create(&path)
    } else {
        std::fs::File::create_new(&path)
    }
    .map_err(|_| "REPORT_EXISTS_OR_WRITE_FAILED")?;
    file.write_all(&data).map_err(|_| "REPORT_WRITE_FAILED")?;
    Ok(format!(
        "artifacts/private/{}/{name}",
        if cfg!(feature = "p3-lan") {
            "p3"
        } else if cfg!(feature = "p005-control") {
            "p005"
        } else {
            "p004b"
        }
    ))
}
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let state = window.state::<Arc<LanState>>();
        if state.closing.swap(true, Ordering::AcqRel) {
            return;
        }
        let active = state
            .active
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|a| (a.id.clone(), a.session.clone())));
        if let Some((_, session)) = &active {
            session.cancel();
        }
        let window = window.clone();
        let shared = state.inner().clone();
        let old = window
            .state::<Arc<crate::p004::P004State>>()
            .inner()
            .clone();
        tauri::async_runtime::spawn(async move {
            let old_clean = crate::p004::close_for_lan(&old).await;
            let clean = if let Some((id, session)) = active {
                let clean = session.stop().await.is_ok() && session.stats().resources_released;
                let _ = write_report(&id, "window-close", &session.stats(), None);
                clean
            } else {
                true
            };
            if clean && old_clean {
                let _ = window.destroy();
            } else {
                shared.closing.store(false, Ordering::Release);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidate_summary_is_bounded_counts_only() {
        let valid = serde_json::json!({"before":2,"mdns":1,"published":1,"mdnsPublished":1});
        assert!(serde_json::from_value::<CandidateSummary>(valid.clone())
            .unwrap()
            .validate()
            .is_ok());
        for (key, value) in [("before", 129), ("published", 3), ("mdnsPublished", 2)] {
            let mut invalid = valid.clone();
            invalid[key] = value.into();
            assert!(serde_json::from_value::<CandidateSummary>(invalid)
                .unwrap()
                .validate()
                .is_err());
        }
        let mut invalid = valid;
        invalid["address"] = "excluded".into();
        assert!(serde_json::from_value::<CandidateSummary>(invalid).is_err());
    }
    #[test]
    fn native_lan_config_rejects_absent_consent_public_peer_and_unbounded_duration() {
        let mut config = Config {
            real_control: false,
            control_sim: false,
            role: "sender".into(),
            consent: false,
            local_ip: "192.168.10.2".into(),
            peer_ip: "192.168.10.3".into(),
            seconds: 30,
            port: 48604,
            pairing_code: None,
        };
        assert!(config.validate().is_err());
        config.consent = true;
        assert!(config.validate().is_ok());
        config.peer_ip = "8.8.8.8".into();
        assert!(config.validate().is_err());
        config.peer_ip = "192.168.10.3".into();
        config.seconds = 31;
        assert!(config.validate().is_err());
        config.seconds = 30;
        config.real_control = true;
        assert_eq!(config.validate().is_ok(), cfg!(feature = "p3-lan"));
        config.control_sim = true;
        assert!(config.validate().is_err());
    }
}

use hub_probe_wire::{PathMode, Role};
use hub_webrtc::{
    lan::LanStatus,
    relay_client::{BrowserReady, PrivateConfig, RelayClient},
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
use tauri::{Emitter, Manager, WebviewWindow};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::HWND,
        NetworkManagement::IpHelper::{GetAdaptersInfo, IP_ADAPTER_INFO},
        UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO},
    },
};
struct Active {
    id: String,
    session: Arc<RelayClient>,
}
#[derive(Default)]
pub struct State {
    active: Mutex<Option<Active>>,
    starting: AtomicBool,
    closing: AtomicBool,
}
struct Lease<'a>(&'a AtomicBool);
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    mode: PathMode,
    peer_ip: Option<Ipv4Addr>,
    role: Role,
    consent: bool,
    seconds: u32,
    local_ip: Ipv4Addr,
    pairing_code: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    session_id: String,
    pairing_code: Option<String>,
}
fn find(state: &State, id: &str) -> Result<Arc<RelayClient>, String> {
    state
        .active
        .lock()
        .map_err(|_| "STATE_LOCK")?
        .as_ref()
        .filter(|x| x.id == id)
        .map(|x| x.session.clone())
        .ok_or("P006_SESSION_INVALID".into())
}
#[tauri::command]
pub fn p006_interfaces(window: WebviewWindow) -> Result<Vec<String>, String> {
    crate::experiment_origin::origin(&window)?;
    let mut size = 0u32;
    unsafe {
        GetAdaptersInfo(None, &mut size);
    }
    if size == 0 || size > 1024 * 1024 {
        return Err("ADAPTERS_UNAVAILABLE".into());
    }
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let ptr = storage.as_mut_ptr().cast::<IP_ADAPTER_INFO>();
    let code = unsafe { GetAdaptersInfo(Some(ptr), &mut size) };
    if code != 0 {
        return Err("ADAPTERS_UNAVAILABLE".into());
    }
    let mut result = vec![];
    let mut p = ptr;
    let mut count = 0;
    unsafe {
        while !p.is_null() && count < 64 {
            let a = &*p;
            let mut ip = &a.IpAddressList
                as *const windows::Win32::NetworkManagement::IpHelper::IP_ADDR_STRING;
            let mut n = 0;
            while !ip.is_null() && n < 16 {
                let bytes = (*ip).IpAddress.String;
                let end = bytes.iter().position(|c| *c == 0).unwrap_or(bytes.len());
                let s = String::from_utf8_lossy(
                    &bytes[..end].iter().map(|c| *c as u8).collect::<Vec<_>>(),
                )
                .into_owned();
                if let Ok(address) = s.parse::<Ipv4Addr>() {
                    if address.is_private() && std::net::UdpSocket::bind((address, 0)).is_ok() {
                        result.push(s);
                    }
                }
                ip = (*ip).Next;
                n += 1;
            }
            p = a.Next;
            count += 1;
        }
    }
    result.sort();
    result.dedup();
    Ok(result)
}
#[tauri::command]
pub async fn p006_start(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<State>>,
    config: Config,
) -> Result<Started, String> {
    crate::experiment_origin::origin(&window)?;
    if !config.consent {
        return Err("CONSENT_REQUIRED".into());
    }
    if !(5..=30).contains(&config.seconds) || !config.local_ip.is_private() {
        return Err("P006_CONFIG_INVALID".into());
    }
    if (config.mode == PathMode::DirectPreferred && !cfg!(feature = "p006-direct"))
        || config.peer_ip.is_some_and(|ip| !ip.is_private())
        || (config.mode == PathMode::RelayOnly && config.peer_ip.is_some())
    {
        return Err("P006_PATH_MODE_OR_PEER_NOT_APPROVED".into());
    }
    if state.closing.load(Ordering::Acquire) || state.starting.swap(true, Ordering::AcqRel) {
        return Err("P006_BUSY".into());
    }
    let _lease = Lease(&state.starting);
    if state
        .active
        .lock()
        .map_err(|_| "STATE_LOCK")?
        .as_ref()
        .is_some_and(|a| !a.session.stats().resources_released)
    {
        return Err("P006_BUSY".into());
    }
    let path = std::env::current_exe()
        .map_err(|_| "EXE_PATH")?
        .parent()
        .ok_or("EXE_PATH")?
        .join(if cfg!(feature = "p006-direct") {
            "p006c-private.json"
        } else {
            "p006b-private.json"
        });
    let private = PrivateConfig::load(&path)?;
    drop(
        std::net::UdpSocket::bind((config.local_ip, 0))
            .map_err(|_| "LOCAL_INTERFACE_BIND_FAILED")?,
    );
    if config.role == Role::Receiver {
        hub_probe_wire::PairKey::parse(config.pairing_code.as_deref().unwrap_or_default())?;
    }
    let hwnd = window.hwnd().map_err(|_| "NATIVE_WINDOW_REQUIRED")?.0 as usize;
    let action = if config.role == Role::Sender {
        if config.mode == PathMode::DirectPreferred {
            "采集本机主屏，优先加密直连，直连不可用时自动使用指定TURN中继"
        } else {
            "采集本机主屏并经加密 TURN 发送到你亲自配对的另一台电脑"
        }
    } else {
        "接收配对电脑的加密视频，并发送100条模拟鼠标事件"
    };
    let private_peer = if config.mode == PathMode::DirectPreferred {
        format!(
            "\n本次允许直连的对端局域网地址：{}。",
            config.peer_ip.map_or_else(
                || "未指定（仅尝试公网直连及中继）".into(),
                |ip| ip.to_string()
            )
        )
    } else {
        String::new()
    };
    let strategy = if config.mode == PathMode::DirectPreferred {
        "优先加密直连，必要时自动中继"
    } else {
        "强制TURN中继"
    };
    let message=format!("本次将{action}，视频最长{}秒。\n连接策略：{strategy}。{private_peer}\n\n使用包内指定服务器和应用专用证书；只记录脱敏统计，不保存画面，不执行键盘鼠标输入。\n配对最长等待120秒，随时停止，断线终止。\n\n是否批准本次实验？",config.seconds);
    let allowed = tauri::async_runtime::spawn_blocking(move || unsafe {
        MessageBoxW(
            Some(HWND(hwnd as *mut _)),
            &HSTRING::from(message),
            &HSTRING::from("RemoteHub · 本次跨网络视频实验"),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        ) == IDYES
    })
    .await
    .map_err(|_| "CONSENT_DIALOG_FAILED")?;
    if !allowed || state.closing.load(Ordering::Acquire) {
        return Err("NATIVE_CONSENT_DENIED".into());
    }
    #[cfg(feature = "p006-direct")]
    let result = if config.mode == PathMode::DirectPreferred {
        RelayClient::start_direct(
            private,
            config.role,
            config.local_ip,
            config.seconds,
            config.pairing_code.as_deref(),
            config.peer_ip,
        )
    } else {
        RelayClient::start(
            private,
            config.role,
            config.local_ip,
            config.seconds,
            config.pairing_code.as_deref(),
        )
    };
    #[cfg(not(feature = "p006-direct"))]
    let result = RelayClient::start(
        private,
        config.role,
        config.local_ip,
        config.seconds,
        config.pairing_code.as_deref(),
    );
    let (session, code) = result?;
    let id = uuid::Uuid::new_v4().to_string();
    *state.active.lock().map_err(|_| "STATE_LOCK")? = Some(Active {
        id: id.clone(),
        session: session.clone(),
    });
    let report_id = id.clone();
    tauri::async_runtime::spawn(async move {
        for _ in 0..400 {
            let s = session.stats();
            let _ = write(&report_id, "progress", &s, None);
            if s.resources_released {
                let _ = write(&report_id, "native-final", &s, None);
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let _ = session.stop().await;
    });
    Ok(Started {
        session_id: id,
        pairing_code: code,
    })
}
#[tauri::command]
pub async fn p006_ready(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<State>>,
    session_id: String,
) -> Result<BrowserReady, String> {
    crate::experiment_origin::origin(&window)?;
    find(&state, &session_id)?.browser_ready().await
}
#[tauri::command]
pub async fn p006_signal(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<State>>,
    session_id: String,
    offer: String,
) -> Result<String, String> {
    crate::experiment_origin::origin(&window)?;
    find(&state, &session_id)?.signal(offer).await
}
#[tauri::command]
pub fn p006_stats(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<State>>,
    session_id: String,
) -> Result<LanStatus, String> {
    crate::experiment_origin::origin(&window)?;
    Ok(find(&state, &session_id)?.stats())
}
#[tauri::command]
pub fn p006_metrics(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<State>>,
    session_id: String,
    data: serde_json::Value,
) -> Result<(), String> {
    crate::experiment_origin::origin(&window)?;
    find(&state, &session_id)?.metrics(data)
}
#[tauri::command]
pub async fn p006_stop(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<State>>,
    session_id: String,
) -> Result<(), String> {
    crate::experiment_origin::origin(&window)?;
    find(&state, &session_id)?.stop().await
}
#[tauri::command]
pub fn p006_save(
    window: WebviewWindow,
    state: tauri::State<'_, Arc<State>>,
    session_id: String,
    browser_closed: Option<bool>,
    control: Option<hub_webrtc::control::BrowserStats>,
    receiver_evidence: Option<hub_webrtc::receiver_evidence::ReceiverEvidence>,
) -> Result<String, String> {
    crate::experiment_origin::origin(&window)?;
    let s = find(&state, &session_id)?;
    if let Some(e) = receiver_evidence {
        let name = s.save_receiver_evidence(&report_dir()?, &session_id, e)?;
        return Ok(format!(
            "artifacts/private/{}/{name}",
            if cfg!(feature = "p006-direct") {
                "p006c"
            } else {
                "p006b"
            }
        ));
    }
    if let Some(c) = control {
        s.record_control(c)?;
    }
    let stats = s.stats();
    if !stats.resources_released {
        return Err("P006_CLEANUP_PENDING".into());
    }
    write(&session_id, "final", &stats, browser_closed)
}
fn report_dir() -> Result<std::path::PathBuf, String> {
    Ok(std::env::current_exe()
        .map_err(|_| "REPORT_DIR")?
        .parent()
        .ok_or("REPORT_DIR")?
        .join(if cfg!(feature = "p006-direct") {
            "artifacts/private/p006c"
        } else {
            "artifacts/private/p006b"
        }))
}
fn write(
    id: &str,
    event: &str,
    status: &LanStatus,
    browser_closed: Option<bool>,
) -> Result<String, String> {
    let dir = std::env::current_exe()
        .map_err(|_| "REPORT_DIR")?
        .parent()
        .ok_or("REPORT_DIR")?
        .join(if cfg!(feature = "p006-direct") {
            "artifacts/private/p006c"
        } else {
            "artifacts/private/p006b"
        });
    std::fs::create_dir_all(&dir).map_err(|_| "REPORT_DIR")?;
    let bytes=serde_json::to_vec_pretty(&serde_json::json!({"event":event,"buildTag":if cfg!(feature="p006-direct") {"p006c-v1"}else{"p006b-v1"},"native":status,"browserClosed":browser_closed})).map_err(|_|"REPORT_ENCODING")?;
    if bytes.len() > 160 * 1024 {
        return Err("REPORT_SIZE".into());
    }
    use std::io::Write;
    let path = dir.join(format!("{id}-{event}.json"));
    let mut f = if event == "progress" {
        std::fs::File::create(&path)
    } else {
        std::fs::File::create_new(&path)
    }
    .map_err(|_| "REPORT_EXISTS_OR_WRITE")?;
    f.write_all(&bytes).map_err(|_| "REPORT_WRITE")?;
    Ok(format!(
        "artifacts/private/{}/{id}-{event}.json",
        if cfg!(feature = "p006-direct") {
            "p006c"
        } else {
            "p006b"
        }
    ))
}
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let state = window.state::<Arc<State>>();
        if state.closing.swap(true, Ordering::AcqRel) {
            return;
        }
        let active = state
            .active
            .lock()
            .ok()
            .and_then(|x| x.as_ref().map(|a| (a.id.clone(), a.session.clone())));
        if let Some((_, s)) = &active {
            s.cancel();
        }
        let _ = window.emit("p006-close-requested", ());
        let window = window.clone();
        let state = state.inner().clone();
        tauri::async_runtime::spawn(async move {
            let clean = if let Some((id, s)) = active {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
                while !s.receiver_evidence_complete() && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                let ok = s.stop().await.is_ok();
                if let Ok(dir) = report_dir() {
                    let _ = s.save_missing_receiver_evidence(&dir, &id);
                }
                let _ = write(&id, "window-close", &s.stats(), None);
                ok
            } else {
                true
            };
            if clean {
                let _ = window.destroy();
            } else {
                state.closing.store(false, Ordering::Release);
            }
        });
    }
}

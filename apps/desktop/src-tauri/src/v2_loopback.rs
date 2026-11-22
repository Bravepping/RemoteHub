use serde::Serialize;
use tauri::WebviewWindow;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    authenticated_signals: u8,
}

// Always registered in the local LAN app so the manifest stays consistent,
// but ordinary builds cannot open a V2 socket through this command.
#[tauri::command]
pub async fn run_v2_loopback_diagnostic(
    window: WebviewWindow,
    consent: bool,
) -> Result<Summary, String> {
    crate::experiment_origin::origin(&window)?;
    if !consent {
        return Err("V2_CONSENT_REQUIRED".into());
    }
    #[cfg(not(feature = "v2-loopback-diagnostic"))]
    return Err("V2_DISABLED".into());
    #[cfg(feature = "v2-loopback-diagnostic")]
    lab::run(window).await
}

#[cfg(feature = "v2-loopback-diagnostic")]
mod lab {
    use super::*;
    use hub_webrtc::{
        authenticated_relay::{
            AuthenticatedLoopbackRelay, AuthenticatedRelayEvent, RelayToken, Role, RoomId,
            SignalKind,
        },
        lan_wire::Key,
    };
    use std::{
        net::{Ipv4Addr, SocketAddrV4},
        sync::{atomic::Ordering, Arc},
        time::Duration,
    };
    use tauri::Manager;
    use windows::{
        core::HSTRING,
        Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{
                MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO,
            },
        },
    };

    const ENDPOINT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 48620);
    const POLL_LIMIT: Duration = Duration::from_secs(2);

    pub(super) async fn run(window: WebviewWindow) -> Result<Summary, String> {
        let shared = window.state::<Arc<crate::ProbeState>>().inner().clone();
        if shared.0.swap(true, Ordering::AcqRel) {
            return Err("V2_BUSY".into());
        }
        let _lease = crate::ProbeLease(&shared.0);
        if window.state::<Arc<crate::p004_lan::LanState>>().busy()
            || window.state::<Arc<crate::p004::P004State>>().busy()
        {
            return Err("V2_BUSY".into());
        }

        let hwnd = window.hwnd().map_err(|_| "V2_NATIVE_WINDOW_REQUIRED")?.0 as usize;
        let allowed = tauri::async_runtime::spawn_blocking(move || unsafe {
            MessageBoxW(
                Some(HWND(hwnd as *mut _)),
                &HSTRING::from(
                    "向本机 127.0.0.1:48620 的 V2 信令服务发送四条内置测试消息。不会连接其他电脑、读取画面或操作键鼠。是否继续？",
                ),
                &HSTRING::from("RemoteHub · V2 本机信令诊断"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
            ) == IDYES
        })
        .await
        .map_err(|_| "V2_CONSENT_FAILED")?;
        if !allowed {
            return Err("V2_NATIVE_CONSENT_DENIED".into());
        }
        crate::experiment_origin::origin(&window)?;
        let token =
            RelayToken::new(std::env::var("REMOTEHUB_V2_TOKEN").map_err(|_| "V2_TOKEN_REQUIRED")?)
                .map_err(|_| "V2_TOKEN_REQUIRED")?;
        // An outer deadline drops both connections even if a send or receive is in progress.
        tokio::time::timeout(Duration::from_secs(12), roundtrip(&token))
            .await
            .map_err(|_| "V2_TIMEOUT")?
            .map_err(str::to_owned)
    }

    async fn roundtrip(token: &RelayToken) -> Result<Summary, &'static str> {
        let room = RoomId::random();
        let second_room = RoomId::new(room.as_str().to_owned()).map_err(|_| "V2_SIGNAL_FAILED")?;
        let key = Arc::new(Key::generate());
        let nonce = uuid::Uuid::new_v4().to_string();
        let mut host = AuthenticatedLoopbackRelay::connect(
            ENDPOINT.into(),
            token,
            room,
            Role::Host,
            key.clone(),
            nonce.clone(),
        )
        .await
        .map_err(|_| "V2_CONNECT_FAILED")?;
        if !matches!(
            host.next_event(POLL_LIMIT).await,
            Ok(AuthenticatedRelayEvent::Waiting)
        ) {
            return Err("V2_SIGNAL_FAILED");
        }
        let mut viewer = AuthenticatedLoopbackRelay::connect(
            ENDPOINT.into(),
            token,
            second_room,
            Role::Viewer,
            key,
            nonce,
        )
        .await
        .map_err(|_| "V2_CONNECT_FAILED")?;
        for relay in [&mut host, &mut viewer] {
            if !matches!(
                relay.next_event(POLL_LIMIT).await,
                Ok(AuthenticatedRelayEvent::Paired)
            ) {
                return Err("V2_SIGNAL_FAILED");
            }
        }
        exchange(&mut viewer, &mut host, SignalKind::Offer, "lab-offer").await?;
        exchange(&mut host, &mut viewer, SignalKind::Answer, "lab-answer").await?;
        exchange(&mut viewer, &mut host, SignalKind::Ice, "lab-viewer-ice").await?;
        exchange(&mut host, &mut viewer, SignalKind::Ice, "lab-host-ice").await?;
        viewer.leave().await.map_err(|_| "V2_SIGNAL_FAILED")?;
        if !matches!(
            host.next_event(POLL_LIMIT).await,
            Ok(AuthenticatedRelayEvent::PeerLeft)
        ) {
            return Err("V2_SIGNAL_FAILED");
        }
        Ok(Summary {
            authenticated_signals: 4,
        })
    }

    async fn exchange(
        from: &mut AuthenticatedLoopbackRelay,
        to: &mut AuthenticatedLoopbackRelay,
        kind: SignalKind,
        payload: &'static str,
    ) -> Result<(), &'static str> {
        from.send_signal(kind, payload.to_owned())
            .await
            .map_err(|_| "V2_SIGNAL_FAILED")?;
        match to.next_event(POLL_LIMIT).await {
            Ok(AuthenticatedRelayEvent::Signal { kind: actual, data })
                if actual == kind && data == payload =>
            {
                Ok(())
            }
            _ => Err("V2_SIGNAL_FAILED"),
        }
    }
}

//! Native confirmation of a public key only; no remote session is ever opened.
use serde::Serialize;
use tauri::WebviewWindow;

#[derive(Serialize)]
pub(crate) struct PairingCard {
    card: String,
    fingerprint: String,
}

#[derive(Serialize)]
pub(crate) struct Verified {
    fingerprint: String,
}

#[tauri::command]
pub(crate) fn get_v2_pairing_card(window: WebviewWindow) -> Result<PairingCard, String> {
    crate::experiment_origin::origin(&window)?;
    #[cfg(not(feature = "v2-peer-pairing"))]
    return Err("V2_PAIRING_DISABLED".into());
    #[cfg(feature = "v2-peer-pairing")]
    pairing::card(&window)
}

#[tauri::command]
pub(crate) async fn verify_v2_peer(
    window: WebviewWindow,
    card: String,
    fingerprint: String,
    consent: bool,
) -> Result<Verified, String> {
    crate::experiment_origin::origin(&window)?;
    if !consent {
        return Err("V2_PAIRING_CONSENT_REQUIRED".into());
    }
    #[cfg(not(feature = "v2-peer-pairing"))]
    {
        let _ = (card, fingerprint);
        Err("V2_PAIRING_DISABLED".into())
    }
    #[cfg(feature = "v2-peer-pairing")]
    pairing::verify(window, card, fingerprint).await
}

#[cfg(feature = "v2-peer-pairing")]
pub(crate) use pairing::PairingState;

#[cfg(feature = "v2-peer-pairing")]
mod pairing {
    use super::{PairingCard, Verified};
    use crate::identity::DesktopIdentity;
    use hub_identity::pairing::{PairingCandidate, PairingError, VerifiedPeer};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };
    use tauri::{Manager, WebviewWindow};
    use windows::{
        core::HSTRING,
        Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{
                MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO,
            },
        },
    };

    #[derive(Default)]
    pub(crate) struct PairingState {
        busy: AtomicBool,
        // A single in-memory result, never read by capture/input/connection code.
        peer: Mutex<Option<VerifiedPeer>>,
    }

    fn error_code(error: PairingError) -> &'static str {
        match error {
            PairingError::CodeFormat | PairingError::PublicKey => "V2_PAIRING_INVALID_CARD",
            PairingError::FingerprintFormat | PairingError::FingerprintMismatch => {
                "V2_PAIRING_FINGERPRINT_MISMATCH"
            }
            PairingError::SelfPairing => "V2_PAIRING_SELF",
        }
    }

    pub(super) fn card(window: &WebviewWindow) -> Result<PairingCard, String> {
        let identity = window.state::<DesktopIdentity>();
        let public = identity.public().ok_or("V2_PAIRING_IDENTITY_UNAVAILABLE")?;
        Ok(PairingCard {
            card: public.pairing_code(),
            fingerprint: public.fingerprint.clone(),
        })
    }

    pub(super) async fn verify(
        window: WebviewWindow,
        card: String,
        fingerprint: String,
    ) -> Result<Verified, String> {
        let identity = window.state::<DesktopIdentity>();
        let public = identity.public().ok_or("V2_PAIRING_IDENTITY_UNAVAILABLE")?;
        let candidate = PairingCandidate::parse(&card).map_err(error_code)?;
        let peer = public
            .verify_peer(&candidate, &fingerprint)
            .map_err(error_code)?;
        let fingerprint = peer.fingerprint().to_owned();

        let state = window.state::<PairingState>();
        if state.busy.swap(true, Ordering::AcqRel) {
            return Err("V2_PAIRING_BUSY".into());
        }
        let _lease = crate::ProbeLease(&state.busy);
        let hwnd = window.hwnd().map_err(|_| "V2_PAIRING_DENIED")?.0 as usize;
        let prompt = format!(
            "请在对方电脑上核对完整设备指纹：\n\n{fingerprint}\n\n仅在指纹完全一致时确认。本次核验只保留到退出应用，不会连接设备，也不会授予观看或键鼠权限。"
        );
        let approved = tauri::async_runtime::spawn_blocking(move || unsafe {
            MessageBoxW(
                Some(HWND(hwnd as *mut _)),
                &HSTRING::from(prompt),
                &HSTRING::from("RemoteHub · 核验对方设备"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
            ) == IDYES
        })
        .await
        .map_err(|_| "V2_PAIRING_DENIED")?;
        if !approved {
            return Err("V2_PAIRING_DENIED".into());
        }
        crate::experiment_origin::origin(&window)?;
        *state.peer.lock().map_err(|_| "V2_PAIRING_BUSY")? = Some(peer);
        Ok(Verified { fingerprint })
    }
}

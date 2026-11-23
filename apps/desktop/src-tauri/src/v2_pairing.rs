//! Native confirmation of a public key only; no remote session is ever opened.
use serde::Serialize;
use tauri::WebviewWindow;

#[derive(Serialize)]
pub(crate) struct PairingCard {
    card: String,
    fingerprint: String,
}

/// Untrusted form defaults for a private portable bundle. Neither accepts
/// consent nor writes PairingState.
#[cfg(all(feature = "v2-public-video-desktop", windows))]
pub(crate) fn bundle_prefill(window: &WebviewWindow) -> Option<PairingCard> {
    use std::io::Read;
    use tauri::Manager;

    let identity = window.state::<crate::identity::DesktopIdentity>();
    let public = identity.public()?;
    let dir = std::env::current_exe().ok()?;
    let path = dir.parent()?.join("remotehub-peer-prefill.json");
    let file = crate::public_video_profile::open_local_regular(&path).ok()?;
    let mut bytes = Vec::new();
    const MAX_BYTES: usize = 512;
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_BYTES {
        return None;
    }
    parse_prefill(&bytes, &public.fingerprint)
}

#[cfg(all(feature = "v2-public-video-desktop", any(windows, test)))]
fn parse_prefill(bytes: &[u8], local_fingerprint: &str) -> Option<PairingCard> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Prefill {
        local_fingerprint: String,
        card: String,
        fingerprint: String,
    }
    let Prefill {
        local_fingerprint: intended_local,
        card,
        fingerprint,
    } = serde_json::from_slice(bytes).ok()?;
    let candidate = hub_identity::pairing::PairingCandidate::parse(&card).ok()?;
    if intended_local != local_fingerprint
        || fingerprint == local_fingerprint
        || fingerprint != candidate.fingerprint()
    {
        return None;
    }
    Some(PairingCard { card, fingerprint })
}

#[cfg(all(test, feature = "v2-public-video-desktop"))]
mod bundle_tests {
    use super::parse_prefill;
    use hub_identity::pairing::PairingCandidate;

    #[test]
    fn role_bound_bundle_only_accepts_public_fields_for_this_device() {
        let local = "a".repeat(64);
        let card = "RH2-d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
        let peer = PairingCandidate::parse(card).unwrap();
        let mut data = serde_json::json!({
            "local_fingerprint": local,
            "card": card,
            "fingerprint": peer.fingerprint(),
        });
        let bytes = serde_json::to_vec(&data).unwrap();
        assert!(parse_prefill(&bytes, &local).is_some());
        assert!(parse_prefill(&bytes, peer.fingerprint()).is_none());
        data["consent"] = serde_json::Value::Bool(true);
        assert!(parse_prefill(&serde_json::to_vec(&data).unwrap(), &local).is_none());
        data.as_object_mut().unwrap().remove("consent");
        data["fingerprint"] = serde_json::Value::String("0".repeat(64));
        assert!(parse_prefill(&serde_json::to_vec(&data).unwrap(), &local).is_none());
    }
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
        Arc, Mutex,
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
        // Replacing the record cannot alter a previously approved session's snapshot.
        peer: Mutex<Option<Arc<VerifiedPeer>>>,
    }

    #[cfg(feature = "v2-verified-lan")]
    impl PairingState {
        pub(crate) fn current(&self) -> Result<Arc<VerifiedPeer>, &'static str> {
            if self.busy.load(Ordering::Acquire) {
                return Err("LAN_VERIFIED_PEER_REQUIRED");
            }
            self.peer
                .lock()
                .map_err(|_| "LAN_VERIFIED_PEER_REQUIRED")?
                .as_ref()
                .cloned()
                .ok_or("LAN_VERIFIED_PEER_REQUIRED")
        }
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
        *state.peer.lock().map_err(|_| "V2_PAIRING_BUSY")? = Some(Arc::new(peer));
        Ok(Verified { fingerprint })
    }
}

//! Explicit synthetic-only operator build. No cloud activation or capture bypass.
use hub_probe_wire::{now, PathMode};
use hub_webrtc::relay_client::PrivateConfig;
use std::{path::Path, sync::Arc};

pub const SECONDS: u32 = 8;
pub const PRIVATE_FILE: &str = "r06-private.json";

pub fn validate(mode: PathMode, seconds: u32) -> Result<(), String> {
    if mode != PathMode::DirectPreferred || seconds != SECONDS {
        return Err("R06_SYNTHETIC_DIRECT_8_SECONDS_REQUIRED".into());
    }
    Ok(())
}
pub fn load(path: &Path) -> Result<Arc<PrivateConfig>, String> {
    // JSON cannot select desktop, local bypass or a packet fault. Keep real cloud ICE.
    let config = PrivateConfig::load_operator_synthetic(path, false)?;
    if config.invitation_expires_at.saturating_sub(now()) > 1200 {
        return Err("R06_INVITATION_WINDOW_EXCEEDS_20_MINUTES".into());
    }
    Ok(config)
}
pub fn build_info() -> serde_json::Value {
    serde_json::json!({
        "buildTag":"p007b-r06-v1", "sourceKind":"synthetic_nv12", "desktopCapture":false,
        "inputInjection":false, "mode":"direct_preferred", "seconds":SECONDS,
        "privateConfigFile":PRIVATE_FILE, "reportDirectory":"artifacts/private/p007b-r06",
        "nativeConsentRequired":true, "maximumInvitationRemainingSeconds":1200,
        "candidateValidation":"exact_authenticated_sdp", "automaticNetworkStart":false
    })
}
pub fn cli() -> bool {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return false;
    }
    if args == ["--r06-self-check"] {
        println!("{}", build_info());
        return true;
    }
    eprintln!("R06_UNKNOWN_ARGUMENT");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dedicated_build_rejects_other_modes_and_durations() {
        assert!(validate(PathMode::DirectPreferred, SECONDS).is_ok());
        assert!(validate(PathMode::RelayOnly, SECONDS).is_err());
        for n in [0, 5, 9, 30, u32::MAX] {
            assert!(validate(PathMode::DirectPreferred, n).is_err());
        }
        assert_eq!(build_info()["desktopCapture"], false);
        assert_eq!(build_info()["inputInjection"], false);
    }
    #[test]
    fn no_private_local_override_or_overlong_invitation() {
        let path =
            std::env::temp_dir().join(format!("remotehub-r06-{}.json", uuid::Uuid::new_v4()));
        let value = serde_json::json!({"server_ipv4":"8.8.8.8","signaling_port":8443,
            "ca_pem":"not-a-real-certificate", "sender_invitation":"a".repeat(64),
            "receiver_invitation":"b".repeat(64),"invitation_expires_at":now()+180});
        let write =
            |v: &serde_json::Value| std::fs::write(&path, serde_json::to_vec(v).unwrap()).unwrap();
        write(&value);
        assert!(load(&path).is_ok()); // TLS is validated on connect; no network in this test.
        for (key, v) in [
            ("server_ipv4", serde_json::json!("127.0.0.1")),
            ("invitation_expires_at", serde_json::json!(now() + 1800)),
            ("invitation_expires_at", serde_json::json!(now() - 1)),
            ("synthetic_source", serde_json::json!(false)),
            ("diagnostic_drop_direct", serde_json::json!(true)),
            ("source", serde_json::json!("desktop")),
        ] {
            let mut bad = value.clone();
            bad[key] = v;
            write(&bad);
            assert!(load(&path).is_err(), "{key}");
        }
        std::fs::remove_file(path).unwrap();
    }
}

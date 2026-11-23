//! Explicit synthetic-only operator build. No cloud activation or capture bypass.
use hub_probe_wire::{now, PathMode, Role};
use hub_webrtc::relay_client::PrivateConfig;
use std::{path::Path, sync::Arc};

pub const SECONDS: u32 = 8;
pub const BUILD_TAG: &str = "p007b-r06-v6";
pub const PRIVATE_FILE: &str = "r06-private.json";

pub fn validate(mode: PathMode, seconds: u32, role: Role, drop_direct: bool) -> Result<(), String> {
    match mode {
        PathMode::DirectPreferred | PathMode::RelayOnly if seconds == SECONDS => (),
        _ => return Err("R06_SYNTHETIC_8_SECONDS_REQUIRED".into()),
    }
    if drop_direct && (role != Role::Sender || mode != PathMode::DirectPreferred) {
        return Err("R06_FALLBACK_SENDER_DIRECT_ONLY".into());
    }
    Ok(())
}
pub fn load(
    path: &Path,
    role: Role,
    mode: PathMode,
    drop_direct: bool,
) -> Result<Arc<PrivateConfig>, String> {
    // This per-session opt-in comes from the native consent flow, never from JSON.
    let config =
        PrivateConfig::load_operator_role_synthetic_with_fault(path, role, mode, drop_direct)?;
    if config.invitation_expires_at.saturating_sub(now()) > 1200 {
        return Err("R06_INVITATION_WINDOW_EXCEEDS_20_MINUTES".into());
    }
    Ok(config)
}
pub fn build_info() -> serde_json::Value {
    serde_json::json!({
        "buildTag":BUILD_TAG, "sourceKind":"synthetic_nv12", "desktopCapture":false,
        "inputInjection":false, "modes":["direct_preferred","relay_only"], "explicitModeRequired":true, "senderOnlySessionDirectFault":true, "seconds":SECONDS,
        "privateConfigFile":PRIVATE_FILE, "roleScopedInvitationRequired":true, "reportDirectory":"artifacts/private/p007b-r06",
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
    fn dedicated_build_allows_exactly_eight_seconds_in_either_mode() {
        assert!(validate(PathMode::DirectPreferred, SECONDS, Role::Sender, false).is_ok());
        assert!(validate(PathMode::RelayOnly, SECONDS, Role::Sender, false).is_ok());
        for n in [0, 5, 9, 30, u32::MAX] {
            assert!(validate(PathMode::DirectPreferred, n, Role::Sender, false).is_err());
            assert!(validate(PathMode::RelayOnly, n, Role::Sender, false).is_err());
        }
        assert_eq!(build_info()["desktopCapture"], false);
        assert_eq!(build_info()["inputInjection"], false);
    }
    #[test]
    fn fault_opt_in_requires_sender_and_direct_preferred() {
        assert!(validate(PathMode::DirectPreferred, SECONDS, Role::Sender, true).is_ok());
        assert!(validate(PathMode::DirectPreferred, SECONDS, Role::Receiver, true).is_err());
        assert!(validate(PathMode::RelayOnly, SECONDS, Role::Sender, true).is_err());
    }

    #[test]
    fn no_private_local_override_or_overlong_invitation() {
        let path =
            std::env::temp_dir().join(format!("remotehub-r06-{}.json", uuid::Uuid::new_v4()));
        let value = serde_json::json!({"server_ipv4":"8.8.8.8","signaling_port":8443,
            "ca_pem":"not-a-real-certificate", "sender_invitation":"a".repeat(64),
            "invitation_expires_at":now()+180});
        let write =
            |v: &serde_json::Value| std::fs::write(&path, serde_json::to_vec(v).unwrap()).unwrap();
        write(&value);
        assert!(load(&path, Role::Sender, PathMode::DirectPreferred, false).is_ok()); // TLS is validated on connect; no network in this test.
        assert!(load(&path, Role::Receiver, PathMode::DirectPreferred, false).is_err());
        for (key, v) in [
            ("receiver_invitation", serde_json::json!("b".repeat(64))),
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
            assert!(
                load(&path, Role::Sender, PathMode::DirectPreferred, false).is_err(),
                "{key}"
            );
        }
        std::fs::remove_file(path).unwrap();
    }
}

//! Default-denied operator diagnostic; never invoked by desktop IPC.
use hub_webrtc::{
    direct_policy::DirectGrant,
    direct_probe::{self, Case},
    ice_policy::{IceConfig, RelayGrant, RelayTarget},
};
use serde::Deserialize;
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operator_approved: bool,
    local: Ipv4Addr,
    approval_expires_at: u64,
    turn: Option<Turn>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    endpoint: SocketAddrV4,
    relay: Ipv4Addr,
    port_min: u16,
    port_max: u16,
    a: IceConfig,
    b: IceConfig,
}
async fn run(path: &Path, case: Case) -> Result<bool, String> {
    let m = std::fs::symlink_metadata(path).map_err(|_| "PATH_PROBE_INPUT_UNAVAILABLE")?;
    if !m.is_file() || m.file_type().is_symlink() || m.len() > 32768 {
        return Err("PATH_PROBE_FILE_POLICY".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o077 != 0 {
            return Err("PATH_PROBE_FILE_PRIVATE_REQUIRED".into());
        }
    }
    let text =
        zeroize::Zeroizing::new(std::fs::read_to_string(path).map_err(|_| "PATH_PROBE_INPUT")?);
    let input: Input = serde_json::from_str(&text).map_err(|_| "PATH_PROBE_SCHEMA")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "PATH_PROBE_CLOCK")?
        .as_secs();
    if !input.operator_approved
        || !input.local.is_private()
        || !(60..=600).contains(&input.approval_expires_at.saturating_sub(now))
    {
        return Err("PATH_PROBE_APPROVAL_REQUIRED".into());
    }
    let (a, b) = if let Some(t) = &input.turn {
        if t.a.session_id == t.b.session_id {
            return Err("PATH_PROBE_UNIQUE_TICKETS_REQUIRED".into());
        }
        let target = RelayTarget {
            local: input.local,
            turn: t.endpoint,
            relay: t.relay,
            port_min: t.port_min,
            port_max: t.port_max,
        };
        (
            DirectGrant::new(
                RelayGrant::new(target, &t.a.session_id, input.approval_expires_at, true)?,
                vec![input.local],
                &t.a,
                now,
            )?,
            DirectGrant::new(
                RelayGrant::new(target, &t.b.session_id, input.approval_expires_at, true)?,
                vec![input.local],
                &t.b,
                now,
            )?,
        )
    } else {
        if matches!(case, Case::Fallback | Case::TurnUnavailable) {
            return Err("PATH_PROBE_TURN_TARGET_REQUIRED".into());
        }
        (
            DirectGrant::local_diagnostic(input.local, input.approval_expires_at, true, now)?,
            DirectGrant::local_diagnostic(input.local, input.approval_expires_at, true, now)?,
        )
    };
    let report = direct_probe::run(
        a,
        input.turn.as_ref().map(|t| &t.a),
        b,
        input.turn.as_ref().map(|t| &t.b),
        case,
    )
    .await;
    let passed = report.error.is_none() && report.resources_released;
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|_| "PATH_PROBE_REPORT")?
    );
    Ok(passed)
}
#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--help" {
        println!("ice-path-probe --approved-target PRIVATE_JSON --case direct|fallback|turn-unavailable|all-unavailable\nTwo real native PCs on one Windows machine. No capture/input. Default: no network access.\nFaults drop packets only inside this diagnostic process; all cases retain ICE All. A failed connectivity case exits 1 even when expected by the test runner.");
        return;
    }
    if args.len() != 5 || args[1] != "--approved-target" || args[3] != "--case" {
        eprintln!("PATH_PROBE_EXPLICIT_APPROVAL_REQUIRED");
        std::process::exit(2);
    }
    let case = match args[4].as_str() {
        "direct" => Case::Direct,
        "fallback" => Case::Fallback,
        "turn-unavailable" => Case::TurnUnavailable,
        "all-unavailable" => Case::AllUnavailable,
        _ => {
            eprintln!("PATH_PROBE_CASE_INVALID");
            std::process::exit(2);
        }
    };
    match run(Path::new(&args[2]), case).await {
        Ok(true) => (),
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

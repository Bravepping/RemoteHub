//! Explicit operator-only Allocate probe. Never invoked by the desktop app.
use hub_webrtc::{
    ice_policy::{IceConfig, RelayGrant, RelayTarget},
    turn_probe::TurnProbe,
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
    ice: IceConfig,
    target: Target,
    approval_expires_at: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    local: Ipv4Addr,
    turn: SocketAddrV4,
    relay: Ipv4Addr,
    port_min: u16,
    port_max: u16,
}
async fn run(path: &Path, lifecycle: bool, connected: bool) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "PROBE_CONFIG_UNAVAILABLE")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16384 {
        return Err("PROBE_CONFIG_FILE_POLICY".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("PROBE_CONFIG_MUST_BE_PRIVATE".into());
        }
    }
    let text = zeroize::Zeroizing::new(
        std::fs::read_to_string(path).map_err(|_| "PROBE_CONFIG_UNREADABLE")?,
    );
    let input: Input = serde_json::from_str(&text).map_err(|_| "PROBE_CONFIG_SCHEMA")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "PROBE_CLOCK_INVALID")?
        .as_secs();
    if input.approval_expires_at <= now || input.approval_expires_at > now.saturating_add(600) {
        return Err("PROBE_APPROVAL_EXPIRY_INVALID".into());
    }
    let target = RelayTarget {
        local: input.target.local,
        turn: input.target.turn,
        relay: input.target.relay,
        port_min: input.target.port_min,
        port_max: input.target.port_max,
    };
    let grant = RelayGrant::new(
        target,
        &input.ice.session_id,
        input.approval_expires_at,
        true,
    )?;
    let probe = TurnProbe::new(&input.ice, grant, now).await?;
    let result = if connected {
        probe.connected_lifecycle().await?
    } else if lifecycle {
        let origin = std::time::Instant::now();
        probe.gather_observed(std::time::Duration::from_secs(2), |event, data| {
            println!("{}", serde_json::json!({"case":"native-close","event":event,"timeNs":SystemTime::now().duration_since(UNIX_EPOCH).map_or(0,|x|x.as_nanos()),"elapsedMs":origin.elapsed().as_millis(),"relayCandidates":data.relay_candidates,"localPeerConnectionReleased":data.resources_released}));
        }).await?
    } else {
        probe.gather().await?
    };
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|_| "PROBE_REPORT_ENCODING")?
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    let started = std::time::Instant::now();
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--help" {
        println!("turn-allocate-probe --approved-target PRIVATE_JSON [--lifecycle|--connected-lifecycle]\nRequires explicit ownership/authorization of that exact TURN endpoint and fresh short-term credentials.\n--lifecycle records gather/close timestamps; --connected-lifecycle uses a private local stdio data-only SDP broker and fixed ping/pong. Server release needs independent observation.\nNo capture, video, input injection or cloud deployment. Default invocation performs no network access.");
        return;
    }
    let lifecycle = args.len() == 4 && args[3] == "--lifecycle";
    let connected = args.len() == 4 && args[3] == "--connected-lifecycle";
    if (args.len() != 3 && !lifecycle && !connected) || args[1] != "--approved-target" {
        eprintln!("PROBE_EXPLICIT_TARGET_APPROVAL_REQUIRED; use --help");
        std::process::exit(2);
    }
    if let Err(error) = run(Path::new(&args[2]), lifecycle, connected).await {
        eprintln!(
            "{}",
            serde_json::json!({"status":"failed","error":error,"probe_elapsed_ms":started.elapsed().as_millis() as u64,"selected_pair":null,"round_trip_time":null})
        );
        std::process::exit(1);
    }
}

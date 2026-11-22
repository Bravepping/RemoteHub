//! Explicit no-desktop local TLS/HMAC + Session + browser RPC harness. No cloud services.
#[cfg(windows)]
mod probe {
    use hub_probe_wire::Role;
    use hub_webrtc::relay_client::{PrivateConfig, RelayClient};
    use std::{
        io::{BufRead, Read, Write},
        net::Ipv4Addr,
        path::Path,
        time::{Duration, Instant},
    };
    fn emit(v: serde_json::Value) {
        println!("{v}");
        let _ = std::io::stdout().flush();
    }
    async fn input() -> Result<String, String> {
        tokio::task::spawn_blocking(|| {
            let mut s = String::new();
            std::io::stdin()
                .lock()
                .take(96 * 1024)
                .read_line(&mut s)
                .map_err(|_| "LOCAL_RPC_READ")?;
            if s.len() > 96 * 1024 - 1 {
                return Err("LOCAL_RPC_SIZE".into());
            }
            Ok(s)
        })
        .await
        .map_err(|_| "LOCAL_RPC_TASK")?
    }
    pub async fn run(
        config: &Path,
        local: Ipv4Addr,
        seconds: u32,
        public: bool,
        drop_direct: bool,
    ) -> Result<(), String> {
        if !local.is_private() {
            return Err("LOCAL_RPC_PRIVATE_INTERFACE".into());
        }
        if drop_direct && !public {
            return Err("DIRECT_FAULT_REQUIRES_APPROVED_WSS_TARGET".into());
        }
        let private = if public {
            PrivateConfig::load_operator_synthetic(config, drop_direct)?
        } else {
            PrivateConfig::load_local_synthetic(config)?
        };
        let (sender, code) = RelayClient::start_direct(
            private.clone(),
            Role::Sender,
            local,
            seconds,
            None,
            Some(local),
        )?;
        let mut receiver = None;
        let owner_id = uuid::Uuid::new_v4().to_string();
        let evidence_dir = config
            .parent()
            .ok_or("LOCAL_RPC_REPORT_DIRECTORY")?
            .join("receiver-evidence");
        let result = async {
            tokio::time::timeout(Duration::from_secs(8), async {
                while sender.stats().state != "waiting_peer" {
                    let s = sender.stats();
                    if s.resources_released {
                        return Err(s.error.unwrap_or("LOCAL_SENDER_STOPPED".into()));
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Ok::<(), String>(())
            })
            .await
            .map_err(|_| "LOCAL_SENDER_JOIN_TIMEOUT")??;
            let (viewer, _) = RelayClient::start_direct(
                private,
                Role::Receiver,
                local,
                seconds,
                code.as_deref(),
                Some(local),
            )?;
            receiver = Some(viewer.clone());
            emit(serde_json::json!({"event":"private_rpc_ready"}));
            let started = Instant::now();
            loop {
                if started.elapsed() > Duration::from_secs(110) {
                    return Err("LOCAL_RPC_DEADLINE".into());
                }
                let raw = tokio::time::timeout(Duration::from_secs(40), input())
                    .await
                    .map_err(|_| "LOCAL_RPC_IDLE_TIMEOUT")??;
                let request: serde_json::Value =
                    serde_json::from_str(&raw).map_err(|_| "LOCAL_RPC_SCHEMA")?;
                let command = request["command"].as_str().ok_or("LOCAL_RPC_COMMAND")?;
                let id = request["id"].as_u64().ok_or("LOCAL_RPC_ID")?;
                if command == "finish" {
                    break;
                }
                let args = &request["args"];
                if command=="p006_save"&&args["sessionId"].as_str()!=Some(owner_id.as_str()) {
                    emit(serde_json::json!({"event":"private_rpc_response","id":id,"error":"LOCAL_RPC_OWNER"}));continue;
                }
                let response: Result<serde_json::Value, String> = match command {
                    "test_sender_report_denial"=>{
                        let e=hub_webrtc::receiver_evidence::ReceiverEvidence::unavailable(hub_webrtc::receiver_evidence::Stage::PreClose);
                        let denied=sender.save_receiver_evidence(&evidence_dir,&owner_id,e).err().ok_or("LOCAL_RPC_ROLE_NOT_DENIED")?;
                        Ok(serde_json::json!({"denial":denied}))
                    }
                    "p006_start" => {
                        Ok(serde_json::json!({"sessionId":owner_id,"pairingCode":null}))
                    }
                    "p006_ready" => viewer.browser_ready().await.and_then(|r| {
                        serde_json::to_value(r).map_err(|_| "LOCAL_RPC_ENCODING".into())
                    }),
                    "p006_signal" => viewer
                        .signal(args["offer"].as_str().ok_or("LOCAL_RPC_OFFER")?.to_string())
                        .await
                        .map(serde_json::Value::String),
                    "p006_stats" => serde_json::to_value(viewer.stats())
                        .map_err(|_| "LOCAL_RPC_ENCODING".into()),
                    "p006_metrics" => viewer
                        .metrics(args["data"].clone())
                        .map(|_| serde_json::Value::Null),
                    "p006_stop" => viewer.stop().await.map(|_| serde_json::Value::Null),
                    "p006_save" => {
                        if let Some(e)=args.get("receiverEvidence") {
                            let e=serde_json::from_value(e.clone()).map_err(|_|"LOCAL_RPC_EVIDENCE_SCHEMA")?;
                            let result=viewer.save_receiver_evidence(&evidence_dir,&owner_id,e);
                            emit(match result {Ok(name)=>serde_json::json!({"event":"private_rpc_response","id":id,"value":name}),Err(e)=>serde_json::json!({"event":"private_rpc_response","id":id,"error":e})});
                            continue;
                        }
                        if let Some(c) = args.get("control") {
                            viewer.record_control(
                                serde_json::from_value(c.clone())
                                    .map_err(|_| "LOCAL_RPC_CONTROL_SCHEMA")?,
                            )?;
                        }
                        Ok(serde_json::Value::String("local-operator-report".into()))
                    }
                    // Sender/viewer have independent command loops in production.
                    // Only accept intent here; final cleanup is still joined below.
                    "sender_cancel" => {
                        sender.cancel();
                        Ok(serde_json::Value::Null)
                    }
                    _ => Err("LOCAL_RPC_COMMAND_DENIED".into()),
                };
                emit(match response {
                    Ok(v) => serde_json::json!({"event":"private_rpc_response","id":id,"value":v}),
                    Err(e) => serde_json::json!({"event":"private_rpc_response","id":id,"error":e}),
                });
            }
            Ok::<(), String>(())
        }
        .await;
        if let Some(r) = &receiver {
            let _ = r.stop().await;
        }
        let _ = sender.stop().await;
        let a = sender.stats();
        let b = receiver.as_ref().map(|r| r.stats());
        emit(
            serde_json::json!({"event":"native_final","sender":a,"receiver":b,"localSynthetic":true}),
        );
        result?;
        if !a.resources_released || b.as_ref().is_none_or(|b| !b.resources_released) {
            return Err("LOCAL_RPC_CLEANUP_PENDING".into());
        }
        if let Some(e) = a.error.or_else(|| b.and_then(|b| b.error)) {
            return Err(e);
        }
        Ok(())
    }
}
fn main() {
    let a: Vec<_> = std::env::args().collect();
    if a.len() == 2 && a[1] == "--help" {
        println!("authenticated-session-probe --synthetic --private-local-wss PRIVATE_JSON --approved-local PRIVATE_IPV4 --seconds 5..15\nAuthorized cloud variant: replace --private-local-wss with --approved-wss, optional --drop-direct for process-local packet-fault test. Requires fresh target-specific operator authorization.\nGenerated Session video only; no desktop capture or input. Default denied.");
        return;
    }
    if ![8, 9].contains(&a.len())
        || a[1] != "--synthetic"
        || !["--private-local-wss", "--approved-wss"].contains(&a[2].as_str())
        || a[4] != "--approved-local"
        || a[6] != "--seconds"
        || (a.len() == 9 && a[8] != "--drop-direct")
    {
        eprintln!("LOCAL_RPC_EXPLICIT_APPROVAL_REQUIRED");
        std::process::exit(2);
    }
    #[cfg(windows)]
    {
        let ip = match a[5].parse() {
            Ok(v) => v,
            Err(_) => {
                eprintln!("LOCAL_RPC_IP");
                std::process::exit(1);
            }
        };
        let seconds = match a[7].parse() {
            Ok(v) if (5..=15).contains(&v) => v,
            _ => {
                eprintln!("LOCAL_RPC_DURATION");
                std::process::exit(1);
            }
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let r = runtime.block_on(probe::run(
            std::path::Path::new(&a[3]),
            ip,
            seconds,
            a[2] == "--approved-wss",
            a.len() == 9,
        ));
        runtime.shutdown_timeout(std::time::Duration::from_millis(200));
        if let Err(e) = r {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("WINDOWS_REQUIRED");
        std::process::exit(1);
    }
}

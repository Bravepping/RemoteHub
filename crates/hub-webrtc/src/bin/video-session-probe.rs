//! Operator-only synthetic-video Session broker. No desktop capture and no TURN access.
#[cfg(windows)]
mod windows_probe {
    use hub_webrtc::{
        direct_policy::DirectGrant,
        session::{DirectSettings, Session},
    };
    use std::{
        io::{BufRead, Read, Write},
        net::Ipv4Addr,
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }
    fn emit(v: serde_json::Value) {
        println!("{v}");
        let _ = std::io::stdout().flush();
    }
    async fn input(seconds: u64) -> Result<String, String> {
        tokio::time::timeout(
            Duration::from_secs(seconds),
            tokio::task::spawn_blocking(|| {
                let mut s = String::new();
                std::io::stdin()
                    .lock()
                    .take(96 * 1024)
                    .read_line(&mut s)
                    .map_err(|_| "VIDEO_PROBE_INPUT")?;
                if s.len() > 65536 {
                    return Err("VIDEO_PROBE_INPUT_SIZE".into());
                }
                Ok(s)
            }),
        )
        .await
        .map_err(|_| "VIDEO_PROBE_INPUT_TIMEOUT")?
        .map_err(|_| "VIDEO_PROBE_INPUT_TASK")?
    }
    pub async fn run(local: Ipv4Addr, seconds: u32) -> Result<(), String> {
        if !local.is_private() {
            return Err("VIDEO_PROBE_PRIVATE_INTERFACE_REQUIRED".into());
        }
        let _bind = std::net::UdpSocket::bind((local, 0))
            .map_err(|_| "VIDEO_PROBE_INTERFACE_NOT_ASSIGNED")?;
        let grant = Arc::new(DirectGrant::local_diagnostic(
            local,
            now() + 180,
            true,
            now(),
        )?);
        let id = grant.session_id();
        emit(serde_json::json!({"event":"private_ready","sessionId":id}));
        let raw = input(15).await?;
        let request: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| "VIDEO_PROBE_OFFER_SCHEMA")?;
        let offer = request
            .get("offer")
            .and_then(|s| s.as_str())
            .ok_or("VIDEO_PROBE_OFFER_REQUIRED")?
            .to_string();
        let session = Session::new_direct_synthetic(
            seconds,
            DirectSettings {
                grant,
                config: None,
                publish_private_local: true,
            },
            id,
        )?;
        let result=async {
            let answer=session.signal(offer).await?;
            emit(serde_json::json!({"event":"private_answer","sdp":answer}));
            let stop=input(seconds as u64+50);tokio::pin!(stop);
            tokio::time::timeout(Duration::from_secs(seconds as u64+55),async {
                let mut notified=false;
                loop {
                    if !notified&&session.stats().state=="draining" {emit(serde_json::json!({"event":"native_draining"}));notified=true;}
                    if session.stats().resources_released {break;}
                    tokio::select!{
                        input=&mut stop=>{match input {Ok(s) if s.trim()=="stop"=>{emit(serde_json::json!({"event":"native_draining"}));session.stop().await?;break;},Err(e) if e=="VIDEO_PROBE_INPUT_TIMEOUT"=>{break;},_=>return Err("VIDEO_PROBE_STOP_SCHEMA".into())}},
                        _=tokio::time::sleep(Duration::from_millis(100))=>{},
                    }
                }Ok::<(),String>(())
            }).await.map_err(|_|"VIDEO_PROBE_DEADLINE")??;
            Ok::<(),String>(())
        }.await;
        let clean = session.stop().await;
        let stats = session.stats();
        emit(serde_json::json!({"event":"native_final","native":stats,"synthetic":true}));
        clean?;
        result?;
        if let Some(e) = stats.error {
            return Err(e);
        }
        if !stats.resources_released {
            return Err("VIDEO_PROBE_CLEANUP_PENDING".into());
        }
        Ok(())
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--help" {
        println!("video-session-probe --synthetic --approved-local PRIVATE_IPV4 --seconds 5..15\nNo desktop capture, input injection, TURN or WSS access. Default invocation performs no networking. Private local stdio SDP broker only.");
        return;
    }
    if args.len() != 6
        || args[1] != "--synthetic"
        || args[2] != "--approved-local"
        || args[4] != "--seconds"
    {
        eprintln!("VIDEO_PROBE_EXPLICIT_LOCAL_SYNTHETIC_APPROVAL_REQUIRED");
        std::process::exit(2);
    }
    #[cfg(windows)]
    {
        let local = match args[3].parse() {
            Ok(ip) => ip,
            Err(_) => {
                eprintln!("VIDEO_PROBE_IP");
                std::process::exit(1);
            }
        };
        let seconds = match args[5].parse::<u32>() {
            Ok(s) if (5..=15).contains(&s) => s,
            _ => {
                eprintln!("VIDEO_PROBE_DURATION");
                std::process::exit(1);
            }
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime.block_on(windows_probe::run(local, seconds));
        runtime.shutdown_timeout(std::time::Duration::from_millis(200));
        if let Err(e) = result {
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

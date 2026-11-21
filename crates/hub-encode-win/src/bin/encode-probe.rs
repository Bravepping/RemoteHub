#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("ENCODER_UNSUPPORTED: Windows is required");
    std::process::exit(1);
}

#[cfg(windows)]
static CONSOLE_STOP: std::sync::OnceLock<hub_encode_win::lifecycle::StopToken> =
    std::sync::OnceLock::new();
#[cfg(windows)]
unsafe extern "system" fn console_control(kind: u32) -> windows::core::BOOL {
    use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    if kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT {
        if let Some(stop) = CONSOLE_STOP.get() {
            stop.stop();
            return true.into();
        }
    }
    false.into()
}
#[cfg(windows)]
struct ConsoleLease;
#[cfg(windows)]
impl Drop for ConsoleLease {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Console::SetConsoleCtrlHandler(
                Some(console_control),
                false,
            );
        }
    }
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use hub_encode_win::{lifecycle::StopToken, EncoderConfig};
    let mut config = EncoderConfig::default();
    let mut consent = false;
    let mut mode = String::from("auto");
    let mut name = format!(
        "p003-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    );
    let mut stop_after_ms = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--consent" => consent = true,
            "--seconds" => {
                config.seconds = args.next().ok_or("--seconds needs a value")?.parse()?
            }
            "--bitrate" => {
                config.bitrate = args.next().ok_or("--bitrate needs a value")?.parse()?
            }
            "--encoder" => mode = args.next().ok_or("--encoder needs a value")?,
            "--name" => name = args.next().ok_or("--name needs a value")?,
            "--stop-after-ms" => {
                stop_after_ms = Some(
                    args.next()
                        .ok_or("--stop-after-ms needs a value")?
                        .parse::<u64>()?,
                )
            }
            "--help" => {
                println!("encode-probe --consent --seconds 10 --encoder auto|software|hardware --name p003-run\nActual local desktop -> MF H264. Private output: artifacts/private/NAME. Max 30 seconds. No network.");
                return Ok(());
            }
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    config.validate(consent)?;
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("INVALID_ARGUMENT: name must be 1..64 ASCII letters/digits/-/_".into());
    }
    if !["auto", "software", "hardware"].contains(&mode.as_str()) {
        return Err("INVALID_ARGUMENT: invalid encoder mode".into());
    }
    let root = std::env::current_dir()?.join("artifacts/private");
    std::fs::create_dir_all(&root)?;
    let stop = StopToken::default();
    let _ = CONSOLE_STOP.set(stop.clone());
    unsafe {
        windows::Win32::System::Console::SetConsoleCtrlHandler(Some(console_control), true)?;
    }
    let _console = ConsoleLease;
    if let Some(ms) = stop_after_ms {
        let token = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            token.stop();
        });
    }
    let report = hub_encode_win::probe::run(consent, config, &mode, &root.join(&name), stop)?;
    println!("encoder={} hardware={} frames={} wall_fps={:.3} missed_ticks={} stopped={}\nPrivate sample: artifacts/private/{name}/desktop.h264",report.encoder,report.hardware,report.encoded_frames,report.encoded_fps_wall,report.missed_ticks,report.stopped);
    Ok(())
}

use hub_capture_win::{run_probe, write_bmp};
use std::{path::PathBuf, time::Duration};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        println!("capture-probe --consent --frames 30 --out artifacts/private/run-001\nCaptures your local desktop for <= 5 seconds. Writes one BMP and metadata. No network.");
        return Ok(());
    }
    let mut consent = false;
    let mut frames = 30;
    let mut out = PathBuf::from("artifacts/private/capture");
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--consent" => consent = true,
            "--frames" => frames = iter.next().ok_or("--frames requires a value")?.parse()?,
            "--out" => out = PathBuf::from(iter.next().ok_or("--out requires a directory")?),
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    // Validate before creating directories, touching the display, or acquiring a GPU resource.
    hub_capture_win::validate_probe(consent, frames, Duration::from_secs(5))?;
    if out.exists() {
        return Err("Output directory already exists; choose a new --out path".into());
    }
    let result = run_probe(consent, frames, Duration::from_secs(5))?;
    std::fs::create_dir_all(&out)?;
    write_bmp(&result.first_frame, &out.join("first-frame.bmp"))?;
    let json = serde_json::to_string_pretty(&result.report)?;
    std::fs::write(out.join("capture-report.json"), &json)?;
    println!("{json}");
    Ok(())
}

//! Isolated local test program, never used by the desktop or any remote session.
use hub_identity::{open_or_create, verify_local_proof};
use std::{fs::OpenOptions, io::Write, path::PathBuf};

fn run() -> Result<(), &'static str> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 || !["prepare", "restore"].contains(&args[1].as_str()) {
        return Err("LOCAL_PROBE_ARGUMENT_REJECTED");
    }
    let root = PathBuf::from(&args[2]);
    let allowed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/private/p201")
        .canonicalize()
        .map_err(|_| "LOCAL_PROBE_ROOT_REJECTED")?;
    if root.parent().and_then(|p| p.canonicalize().ok()).as_ref() != Some(&allowed)
        || !root
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("run-"))
    {
        return Err("LOCAL_PROBE_ROOT_REJECTED");
    }
    let identity = open_or_create(&root).map_err(|_| "LOCAL_PROBE_IDENTITY_FAILED")?;
    let public = identity.public();
    let proof = root.join("proof.bin");
    if args[1] == "prepare" {
        let nonce = [61; 32]; // Public local challenge fixture; not authentication or randomness.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(proof)
            .map_err(|_| "LOCAL_PROBE_WRITE_FAILED")?;
        file.write_all(&public.public_key)
            .and_then(|_| file.write_all(&nonce))
            .and_then(|_| file.write_all(&identity.local_proof(&nonce)))
            .and_then(|_| file.sync_all())
            .map_err(|_| "LOCAL_PROBE_WRITE_FAILED")?;
    } else {
        let bytes = std::fs::read(proof).map_err(|_| "LOCAL_PROBE_READ_FAILED")?;
        if bytes.len() != 128 || bytes[..32] != public.public_key {
            return Err("LOCAL_PROBE_IDENTITY_CHANGED");
        }
        let nonce = bytes[32..64]
            .try_into()
            .map_err(|_| "LOCAL_PROBE_FORMAT_FAILED")?;
        let signature = bytes[64..]
            .try_into()
            .map_err(|_| "LOCAL_PROBE_FORMAT_FAILED")?;
        if !verify_local_proof(&public.public_key, &nonce, &signature) {
            return Err("LOCAL_PROBE_SIGNATURE_FAILED");
        }
    }
    println!(
        "{{\"stage\":\"{}\",\"passed\":true,\"identityOrKeyPrinted\":false,\"processId\":{}}}",
        args[1],
        std::process::id()
    );
    Ok(())
}
fn main() {
    if let Err(code) = run() {
        eprintln!("{code}");
        std::process::exit(1);
    }
}

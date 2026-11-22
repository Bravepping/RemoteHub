$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo clippy -p hub-identity --all-targets --locked --offline -- -D warnings
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo test -p hub-identity --locked --offline
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo build -p hub-identity --example restart_probe --profile probe --locked --offline
    exit $LASTEXITCODE
} finally { Pop-Location }

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    cargo build -p remotehub-desktop --example lifecycle_native --profile probe --features custom-protocol --locked --offline
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    node scripts/p106-native-checks.mjs
    exit $LASTEXITCODE
} finally { Pop-Location }

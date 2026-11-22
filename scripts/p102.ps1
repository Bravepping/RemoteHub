$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
Push-Location (Split-Path $PSScriptRoot -Parent)
try { node scripts/p102-checks.mjs; exit $LASTEXITCODE } finally { Pop-Location }

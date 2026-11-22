param([switch]$PrivacyRound)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
$env:REMOTEHUB_VERIFY_DIR = if ($PrivacyRound) { 'artifacts/p007b-privacy/automatic' } else { 'artifacts/p007b/automatic' }
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  node scripts/p007b-checks.mjs
  exit $LASTEXITCODE
} finally { Pop-Location }

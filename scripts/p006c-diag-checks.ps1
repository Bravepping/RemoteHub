$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
$env:REMOTEHUB_VERIFY_DIR = 'artifacts/p006c-diag/automatic'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  node scripts/p006c-video-checks.mjs
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  $taskPython = if ($env:REMOTEHUB_PYTHON) { $env:REMOTEHUB_PYTHON } else { 'python' }
  & $taskPython scripts/p006c-diag-audit.py
  exit $LASTEXITCODE
} finally { Pop-Location }

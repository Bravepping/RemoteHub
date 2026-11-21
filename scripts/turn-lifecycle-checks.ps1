$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  $env:REMOTEHUB_VERIFY_DIR = 'artifacts/turn-lifecycle/automatic'
  node scripts/turn-lifecycle-checks.mjs
  if ($LASTEXITCODE -ne 0) { throw 'TURN lifecycle offline verification failed' }
} finally { Pop-Location }

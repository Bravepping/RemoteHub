$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  $env:REMOTEHUB_VERIFY_DIR = 'artifacts/p006c-video/automatic'
  node scripts/p006c-video-checks.mjs
  if ($LASTEXITCODE -ne 0) { throw 'P006c video offline verification failed' }
} finally { Pop-Location }

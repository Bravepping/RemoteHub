$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  $env:REMOTEHUB_VERIFY_DIR = 'artifacts/p006c/automatic'
  node scripts/p006c-checks.mjs
  if ($LASTEXITCODE -ne 0) { throw 'P006c offline verification failed' }
} finally { Pop-Location }

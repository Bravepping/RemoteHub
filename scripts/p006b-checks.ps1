$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  node scripts/p006b-checks.mjs
  if ($LASTEXITCODE -ne 0) { throw 'P006b automatic verification failed' }
} finally { Pop-Location }

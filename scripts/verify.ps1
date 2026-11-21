$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  node scripts/checks.mjs
  if ($LASTEXITCODE -ne 0) { throw 'RemoteHub verification failed; see verification.txt in the selected evidence directory' }
} finally {
  Pop-Location
}

param([switch]$BuildOnly)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  pnpm --dir apps/desktop build
  if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed' }
  cargo build -p remotehub-desktop --profile probe --features custom-protocol,p006-direct --locked --offline
  if ($LASTEXITCODE -ne 0) { throw 'Native direct-preferred build failed' }
  if (-not $BuildOnly) {
    Start-Process -FilePath (Join-Path (Get-Location).Path 'target/probe/remotehub-desktop.exe') -WorkingDirectory (Get-Location).Path -WindowStyle Normal
  }
} finally { Pop-Location }

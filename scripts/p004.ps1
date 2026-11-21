param([switch]$BuildOnly)
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  pnpm build
  if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed' }
  cargo build -p remotehub-desktop --profile probe --features custom-protocol,p004-probe --locked
  if ($LASTEXITCODE -ne 0) { throw 'Native loopback build failed' }
  if (-not $BuildOnly) {
    Start-Process -FilePath (Join-Path (Get-Location).Path 'target/probe/remotehub-desktop.exe') -WorkingDirectory (Get-Location).Path -WindowStyle Normal
  }
} finally { Pop-Location }

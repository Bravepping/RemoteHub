param([ValidateSet('Desktop','Browser','Build','Capture')][string]$Mode = 'Desktop')
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
  switch ($Mode) {
    'Desktop' { pnpm --filter @remotehub/desktop tauri dev }
    'Browser' { pnpm run dev }
    'Build' { pnpm --filter @remotehub/desktop tauri build --no-bundle }
    'Capture' {
      $runDirectory = 'artifacts/private/capture-' + [DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss')
      Write-Output 'This explicitly captures your local primary desktop for up to 5 seconds and saves one BMP.'
      cargo run -p hub-capture-win --bin capture-probe --locked -- --consent --frames 30 --out $runDirectory
    }
  }
  if ($LASTEXITCODE -ne 0) { throw "RemoteHub command failed: exit $LASTEXITCODE" }
} finally { Pop-Location }

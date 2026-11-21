$projectRoot = Split-Path $PSScriptRoot -Parent
# Normalize malformed quoted PATH entries from the host's interactive profile.
$env:PATH = (($env:PATH -split ';' | ForEach-Object { $_.Trim().Trim('"') } | Where-Object { $_ }) -join ';')
foreach ($toolName in @('node', 'pnpm')) {
  $toolCommand = Get-Command $toolName -ErrorAction SilentlyContinue
  if ($toolCommand) { $env:PATH = (Split-Path $toolCommand.Source -Parent) + ';' + $env:PATH }
}
$localCargo = Join-Path $projectRoot '.tools\cargo\bin'
if (Test-Path -LiteralPath $localCargo) {
  $env:CARGO_HOME = Join-Path $projectRoot '.tools\cargo'
  $env:RUSTUP_HOME = Join-Path $projectRoot '.tools\rustup'
  $env:PATH = $localCargo + ';' + $env:PATH
}
$portableMsvc = Join-Path $projectRoot '.tools\msvc'
if (Test-Path -LiteralPath (Join-Path $portableMsvc 'VC\Tools\MSVC')) {
  $vc = Get-ChildItem -LiteralPath (Join-Path $portableMsvc 'VC\Tools\MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1
  $sdk = Get-ChildItem -LiteralPath (Join-Path $portableMsvc 'Windows Kits\10\Lib') -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
  if ($vc -and $sdk) {
    $kitRoot = Join-Path $portableMsvc 'Windows Kits\10'
    $env:VCToolsInstallDir = $vc.FullName + '\'
    $env:VCToolsVersion = $vc.Name
    $env:WindowsSdkDir = $kitRoot + '\'
    $env:WindowsSDKVersion = $sdk.Name + '\'
    $env:VSCMD_ARG_HOST_ARCH = 'x64'
    $env:VSCMD_ARG_TGT_ARCH = 'x64'
    $env:PATH = (Join-Path $vc.FullName 'bin\Hostx64\x64') + ';' + (Join-Path $kitRoot ('bin\' + $sdk.Name + '\x64')) + ';' + $env:PATH
    $env:LIB = (Join-Path $vc.FullName 'lib\x64') + ';' + (Join-Path $sdk.FullName 'ucrt\x64') + ';' + (Join-Path $sdk.FullName 'um\x64')
    $includeRoot = Join-Path $kitRoot ('Include\' + $sdk.Name)
    $env:INCLUDE = (Join-Path $vc.FullName 'include') + ';' + (Join-Path $includeRoot 'ucrt') + ';' + (Join-Path $includeRoot 'shared') + ';' + (Join-Path $includeRoot 'um')
  }
}

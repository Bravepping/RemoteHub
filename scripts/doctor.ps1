param([string]$OutputPath = '')
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
function Find-Tool([string]$Name) {
  $command = Get-Command $Name -ErrorAction SilentlyContinue
  if ($command) { return $command.Source }
  return $null
}
$os = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$webviews = @(Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\*' -ErrorAction SilentlyContinue | Where-Object name -Like '*WebView2*' | Select-Object name,pv)
$tools = [ordered]@{}
foreach ($name in @('git','rustc','cargo','node','pnpm','cl','link','rc','docker')) { $tools[$name] = Find-Tool $name }
$linkIsMsvc = $tools.link -and (-not $tools.link.ToLowerInvariant().Contains('\git\usr\bin\'))
$nativeReady = [bool]($tools.rustc -and $tools.cargo -and $tools.cl -and $linkIsMsvc -and $tools.rc -and $webviews.Count -gt 0)
$report = [ordered]@{
  checked_at = [DateTimeOffset]::Now.ToString('o')
  os = @{ name=$os.ProductName; version=$os.DisplayVersion; build=($os.CurrentBuild+'.'+$os.UBR) }
  tools = $tools
  versions = [ordered]@{}
  webview2 = $webviews
  gpu_registry = @(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Video\*\0000' -ErrorAction SilentlyContinue | Where-Object DriverDesc | Select-Object DriverDesc,DriverVersion)
  native_build_prerequisites_detected = $nativeReady
  note = 'Tool presence does not prove build or hardware success. GPU registry may include inactive adapters.'
}
foreach ($name in @('git','rustc','cargo','node','pnpm')) {
  if ($tools[$name]) { $report.versions[$name] = (& $tools[$name] --version 2>&1 | Out-String).Trim() }
}
$json = $report | ConvertTo-Json -Depth 7
Write-Output $json
if ($OutputPath) { [IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath), $json, [Text.UTF8Encoding]::new($false)) }
if (-not $nativeReady) { exit 2 }

param(
  [ValidateSet('Inspect','Install','Remove')][string]$Mode = 'Inspect',
  [Parameter(Mandatory=$true)][string]$Program,
  [Parameter(Mandatory=$true)][string]$LocalIp,
  [Parameter(Mandatory=$true)][string]$PeerIp,
  [ValidateRange(1024,65535)][int]$Port = 48604
)
$ErrorActionPreference = 'Stop'
$exe = (Resolve-Path -LiteralPath $Program).Path
if ([IO.Path]::GetExtension($exe) -ne '.exe') { throw 'Program must be an existing .exe' }
function Test-PrivateIp([string]$Value) {
  $ip = [Net.IPAddress]::Parse($Value)
  if ($ip.AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork) { return $false }
  $b = $ip.GetAddressBytes()
  return (($b[0] -eq 10) -or ($b[0] -eq 172 -and $b[1] -ge 16 -and $b[1] -le 31) -or ($b[0] -eq 192 -and $b[1] -eq 168)) -and $b[3] -ne 0 -and $b[3] -ne 255
}
if (!(Test-PrivateIp $LocalIp) -or !(Test-PrivateIp $PeerIp) -or $LocalIp -eq $PeerIp) { throw 'Distinct private IPv4 addresses required' }
$assigned = [Net.NetworkInformation.NetworkInterface]::GetAllNetworkInterfaces() | ForEach-Object { $_.GetIPProperties().UnicastAddresses } | ForEach-Object { $_.Address.IPAddressToString }
if ($LocalIp -notin $assigned) { throw 'LocalIp is not assigned to this PC' }
$hash = [Security.Cryptography.SHA256]::Create()
try { $suffix = ([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes("$exe|$LocalIp|$PeerIp|$Port")))).Replace('-','').Substring(0,16) } finally { $hash.Dispose() }
$names = @("RemoteHub-P004b-$suffix-TCP", "RemoteHub-P004b-$suffix-UDP")
Write-Host "Mode=$Mode`nProgram=$exe`nLocalIp=$LocalIp`nOnlyPeer=$PeerIp`nTCP=$Port; UDP=dynamic; profiles=Private,Domain"
if ($Mode -eq 'Inspect') { return }
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run this reviewed script in an Administrator PowerShell window' }
if ($Mode -eq 'Remove') {
  foreach ($name in $names) { Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue | Where-Object { $_.Group -eq 'RemoteHub P004b experiment' } | Remove-NetFirewallRule }
  return
}
if (Get-NetFirewallRule -Name $names -ErrorAction SilentlyContinue) { throw 'Rules already exist; inspect or remove the same scoped rules first' }
$common = @{Group='RemoteHub P004b experiment';Direction='Inbound';Action='Allow';Enabled='True';Profile=@('Private','Domain');Program=$exe;LocalAddress=$LocalIp;RemoteAddress=$PeerIp;EdgeTraversalPolicy='Block'}
try {
  New-NetFirewallRule @common -Name $names[0] -DisplayName $names[0] -Protocol TCP -LocalPort $Port | Out-Null
  New-NetFirewallRule @common -Name $names[1] -DisplayName $names[1] -Protocol UDP | Out-Null
} catch {
  foreach ($name in $names) { Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue | Where-Object { $_.Group -eq 'RemoteHub P004b experiment' } | Remove-NetFirewallRule }
  throw
}
Write-Host 'Installed two scoped rules. Use the same arguments with -Mode Remove after testing.'

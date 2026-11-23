[CmdletBinding()]
param(
    [ValidateSet('Relay', 'Host', 'Viewer')][string]$Role,
    [string]$Profile,
    [string]$BinDir = $PSScriptRoot
)

function New-RemoteHubWssStartInfo {
    param(
        [ValidateSet('Relay', 'Host', 'Viewer')][string]$Role,
        [string]$ProfilePath,
        [string]$BinDir,
        [string]$ExpectedCa
    )

    $desktopKeys = @('REMOTEHUB_V2_DESKTOP_RELAY', 'REMOTEHUB_V2_DESKTOP_CA_DER_HEX', 'REMOTEHUB_V2_DESKTOP_TOKEN')
    $relayKeys = @(
        'REMOTEHUB_V2_MODE', 'REMOTEHUB_V2_BIND', 'REMOTEHUB_V2_TLS_CERT', 'REMOTEHUB_V2_TLS_KEY',
        'REMOTEHUB_V2_LAN_HOST_IP', 'REMOTEHUB_V2_LAN_VIEWER_IP',
        'REMOTEHUB_V2_LAN_HOST_TOKEN', 'REMOTEHUB_V2_LAN_VIEWER_TOKEN'
    )
    $expectedKeys = if ($Role -eq 'Viewer') { $desktopKeys } else { $desktopKeys + $relayKeys }
    try {
        $file = Get-Item -LiteralPath $ProfilePath -ErrorAction Stop
        if ($file -isnot [IO.FileInfo] -or $file.Length -gt 65536) { throw 'Invalid profile' }
        $config = Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8 -ErrorAction Stop |
            ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw 'REMOTEHUB_WSS_PROFILE_INVALID'
    }

    $fields = @($config.PSObject.Properties)
    if ($config -isnot [pscustomobject] -or $fields.Count -ne $expectedKeys.Count) {
        throw 'REMOTEHUB_WSS_PROFILE_WRONG_ROLE'
    }
    foreach ($key in $expectedKeys) {
        $field = $config.PSObject.Properties[$key]
        if ($null -eq $field -or $field.Name -cne $key -or $field.Value -isnot [string] -or
            [string]::IsNullOrWhiteSpace($field.Value) -or $field.Value -match '[\x00-\x1f]') {
            throw 'REMOTEHUB_WSS_PROFILE_INVALID'
        }
    }

    $hex = $config.REMOTEHUB_V2_DESKTOP_CA_DER_HEX
    if ($hex -cnotmatch '\A(?:[0-9a-f]{2}){1,16384}\z' -or
        $config.REMOTEHUB_V2_DESKTOP_TOKEN -cnotmatch '\A[0-9a-f]{64}\z') {
        throw 'REMOTEHUB_WSS_PROFILE_INVALID'
    }
    if ($Role -ne 'Viewer' -and ($config.REMOTEHUB_V2_MODE -cne 'private-lan' -or
        $config.REMOTEHUB_V2_BIND -cne $config.REMOTEHUB_V2_DESKTOP_RELAY -or
        $config.REMOTEHUB_V2_LAN_HOST_TOKEN -cne $config.REMOTEHUB_V2_DESKTOP_TOKEN -or
        $config.REMOTEHUB_V2_LAN_VIEWER_TOKEN -cnotmatch '\A[0-9a-f]{64}\z' -or
        $config.REMOTEHUB_V2_LAN_VIEWER_TOKEN -ceq $config.REMOTEHUB_V2_LAN_HOST_TOKEN)) {
        throw 'REMOTEHUB_WSS_PROFILE_INVALID'
    }

    if ($Role -eq 'Viewer') {
        $expected = $ExpectedCa.Trim().ToLowerInvariant()
        if ($expected -cnotmatch '\A[0-9a-f]{64}\z') { throw 'REMOTEHUB_WSS_CA_FORMAT' }
        [byte[]]$der = for ($i = 0; $i -lt $hex.Length; $i += 2) {
            [Convert]::ToByte($hex.Substring($i, 2), 16)
        }
        $sha = [Security.Cryptography.SHA256]::Create()
        try { $actual = ([BitConverter]::ToString($sha.ComputeHash($der))).Replace('-', '').ToLowerInvariant() }
        finally { $sha.Dispose() }
        if ($actual -cne $expected) { throw 'REMOTEHUB_WSS_CA_MISMATCH' }
    }

    $exe = if ($Role -eq 'Relay') { 'v2_signaling.exe' } else { 'remotehub-desktop.exe' }
    $exeItem = Get-Item -LiteralPath (Join-Path $BinDir $exe) -ErrorAction SilentlyContinue
    if ($exeItem -isnot [IO.FileInfo]) { throw 'REMOTEHUB_WSS_BINARY_MISSING' }
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $exeItem.FullName
    $info.WorkingDirectory = $exeItem.DirectoryName
    $info.UseShellExecute = $false

    # Don't let an older manually loaded session leak into a different child role.
    foreach ($key in @($info.EnvironmentVariables.Keys)) {
        if ($key -like 'REMOTEHUB_V2_*') { $info.EnvironmentVariables.Remove($key) }
    }
    $childKeys = if ($Role -eq 'Relay') { $relayKeys } else { $desktopKeys }
    foreach ($key in $childKeys) {
        $info.EnvironmentVariables[$key] = $config.PSObject.Properties[$key].Value
    }
    return $info
}

if ($MyInvocation.InvocationName -ne '.') {
    if ($env:OS -ne 'Windows_NT') { throw 'REMOTEHUB_WSS_WINDOWS_REQUIRED' }
    if (!$Role -or !$Profile) { throw 'Specify -Role Relay|Host|Viewer and -Profile <path>' }
    $expectedCa = if ($Role -eq 'Viewer') {
        Read-Host 'Enter the full 64-character CA SHA-256 shown on the sharing PC'
    } else { '' }
    $startInfo = New-RemoteHubWssStartInfo $Role $Profile $BinDir $expectedCa
    if ($Role -eq 'Relay') { Write-Host 'Keep this relay window open. Press Ctrl+C to stop the relay.' }
    try { $process = [Diagnostics.Process]::Start($startInfo) }
    catch { throw 'REMOTEHUB_WSS_START_FAILED' }
    try {
        if ($Role -eq 'Relay') {
            # Poll so PowerShell can process Ctrl+C while the relay shares its console.
            while (!$process.WaitForExit(250)) {}
            if ($process.ExitCode -ne 0) { exit $process.ExitCode }
        }
    } finally { $process.Dispose() }
}

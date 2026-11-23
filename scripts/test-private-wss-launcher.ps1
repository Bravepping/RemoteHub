$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'start-private-wss.ps1')

function Assert-True([bool]$Value, [string]$Case) {
    if (!$Value) { throw "ASSERT_FAILED: $Case" }
}
function Assert-Throws([string]$Code, [scriptblock]$Action) {
    try { $null = & $Action }
    catch {
        if ($_.Exception.Message -ceq $Code) { return }
        throw "Wrong error: $($_.Exception.Message) instead of $Code"
    }
    throw "Expected error: $Code"
}

function Test-RemoteHubProfilePrompt {
    $script:rhPrompt = ''
    $script:rhAnswer = ' "C:\Test Folder\host.json" '
    function Read-Host([string]$Prompt) {
        $script:rhPrompt = $Prompt
        return $script:rhAnswer
    }
    $resolved = Resolve-RemoteHubWssProfile -Role Relay -Prompt
    Assert-True ($resolved -ceq 'C:\Test Folder\host.json') 'pasted path with quotes'
    Assert-True ($script:rhPrompt -like '*host.json*') 'relay asks for host profile'
    $script:rhAnswer = 'C:\Test Folder\viewer.json'
    $resolved = Resolve-RemoteHubWssProfile -Role Viewer -Prompt
    Assert-True ($resolved -ceq $script:rhAnswer -and $script:rhPrompt -like '*viewer.json*') 'viewer asks for viewer profile'
    $script:rhPrompt = ''
    $resolved = Resolve-RemoteHubWssProfile -Role Host -ProfilePath $script:rhAnswer
    Assert-True ($resolved -ceq $script:rhAnswer -and !$script:rhPrompt) 'explicit profile does not prompt'
    $script:rhAnswer = ' '
    Assert-Throws 'REMOTEHUB_WSS_PROFILE_REQUIRED' { Resolve-RemoteHubWssProfile -Role Viewer -Prompt }
    Assert-Throws 'REMOTEHUB_WSS_PROFILE_REQUIRED' { Resolve-RemoteHubWssProfile -Role Viewer -ProfilePath '' }
}
Test-RemoteHubProfilePrompt

# Stub file names exercise path selection; no executable or relay is started.
$root = Join-Path ([IO.Path]::GetTempPath()) ('remotehub-wss tests ' + [guid]::NewGuid().ToString('N'))
$originalToken = $env:REMOTEHUB_V2_TOKEN
$originalRelay = $env:REMOTEHUB_V2_LAN_VIEWER_TOKEN
try {
    $null = [IO.Directory]::CreateDirectory($root)
    $desktop = Join-Path $root 'remotehub-desktop.exe'
    $relay = Join-Path $root 'v2_signaling.exe'
    $viewerPath = Join-Path $root 'viewer.json'
    $hostPath = Join-Path $root 'host.json'
    $badPath = Join-Path $root 'bad.json'
    [IO.File]::WriteAllText($desktop, '')
    [IO.File]::WriteAllText($relay, '')
    $hash = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
    $viewer = @{
        REMOTEHUB_V2_DESKTOP_RELAY = '192.168.1.20:48620'
        REMOTEHUB_V2_DESKTOP_CA_DER_HEX = '616263'
        REMOTEHUB_V2_DESKTOP_TOKEN = 'b' * 64
    }
    $hostConfig = $viewer.Clone()
    $hostConfig.REMOTEHUB_V2_DESKTOP_TOKEN = 'a' * 64
    $hostConfig.REMOTEHUB_V2_MODE = 'private-lan'
    $hostConfig.REMOTEHUB_V2_BIND = $viewer.REMOTEHUB_V2_DESKTOP_RELAY
    $hostConfig.REMOTEHUB_V2_TLS_CERT = Join-Path $root 'relay-cert.pem'
    $hostConfig.REMOTEHUB_V2_TLS_KEY = Join-Path $root 'relay-key.pem'
    $hostConfig.REMOTEHUB_V2_LAN_HOST_IP = '192.168.1.20'
    $hostConfig.REMOTEHUB_V2_LAN_VIEWER_IP = '192.168.1.21'
    $hostConfig.REMOTEHUB_V2_LAN_HOST_TOKEN = 'a' * 64
    $hostConfig.REMOTEHUB_V2_LAN_VIEWER_TOKEN = 'b' * 64
    [IO.File]::WriteAllText($viewerPath, ($viewer | ConvertTo-Json))
    [IO.File]::WriteAllText($hostPath, ($hostConfig | ConvertTo-Json))

    $env:REMOTEHUB_V2_TOKEN = 'unrelated old token'
    $env:REMOTEHUB_V2_LAN_VIEWER_TOKEN = 'old viewer token'
    $v = New-RemoteHubWssStartInfo Viewer $viewerPath $root (" " + $hash.ToUpperInvariant() + " ")
    $h = New-RemoteHubWssStartInfo Host $hostPath $root
    $r = New-RemoteHubWssStartInfo Relay $hostPath $root
    Assert-True ($v.FileName -eq $desktop -and $h.FileName -eq $desktop -and $r.FileName -eq $relay) 'role binary'
    foreach ($info in @($v, $h, $r)) {
        Assert-True ($info.WorkingDirectory -eq $root -and !$info.UseShellExecute -and !$info.Arguments) 'safe start info'
        Assert-True (!$info.EnvironmentVariables.ContainsKey('REMOTEHUB_V2_TOKEN')) 'stale token stripped'
    }
    foreach ($info in @($v, $h)) {
        Assert-True (@($info.EnvironmentVariables.Keys | Where-Object { $_ -like 'REMOTEHUB_V2_*' }).Count -eq 3) 'desktop keys only'
    }
    Assert-True ($v.EnvironmentVariables['REMOTEHUB_V2_DESKTOP_TOKEN'] -ceq ('b' * 64)) 'viewer token'
    Assert-True ($h.EnvironmentVariables['REMOTEHUB_V2_DESKTOP_TOKEN'] -ceq ('a' * 64)) 'host token'
    Assert-True (@($r.EnvironmentVariables.Keys | Where-Object { $_ -like 'REMOTEHUB_V2_*' }).Count -eq 8) 'relay keys only'
    Assert-True ($r.EnvironmentVariables['REMOTEHUB_V2_MODE'] -ceq 'private-lan') 'relay restricted mode'
    Assert-True ($r.EnvironmentVariables['REMOTEHUB_V2_LAN_VIEWER_TOKEN'] -ceq ('b' * 64)) 'relay viewer token'
    Assert-True ($env:REMOTEHUB_V2_LAN_VIEWER_TOKEN -ceq 'old viewer token') 'parent environment untouched'

    Assert-Throws 'REMOTEHUB_WSS_CA_MISMATCH' { New-RemoteHubWssStartInfo Viewer $viewerPath $root ('0' * 64) }
    Assert-Throws 'REMOTEHUB_WSS_CA_FORMAT' { New-RemoteHubWssStartInfo Viewer $viewerPath $root '' }
    Assert-Throws 'REMOTEHUB_WSS_PROFILE_WRONG_ROLE' { New-RemoteHubWssStartInfo Viewer $hostPath $root $hash }
    Assert-Throws 'REMOTEHUB_WSS_PROFILE_WRONG_ROLE' { New-RemoteHubWssStartInfo Host $viewerPath $root }
    Assert-Throws 'REMOTEHUB_WSS_PROFILE_WRONG_ROLE' { New-RemoteHubWssStartInfo Relay $viewerPath $root }
    Assert-Throws 'REMOTEHUB_WSS_BINARY_MISSING' { New-RemoteHubWssStartInfo Viewer $viewerPath (Join-Path $root 'missing') $hash }
    [IO.File]::WriteAllText($badPath, '{ invalid and possibly sensitive')
    Assert-Throws 'REMOTEHUB_WSS_PROFILE_INVALID' { New-RemoteHubWssStartInfo Viewer $badPath $root $hash }
    [IO.File]::WriteAllText($badPath, (' ' * 65537))
    Assert-Throws 'REMOTEHUB_WSS_PROFILE_INVALID' { New-RemoteHubWssStartInfo Viewer $badPath $root $hash }
    $hostConfig.REMOTEHUB_V2_MODE = 'loopback'
    [IO.File]::WriteAllText($hostPath, ($hostConfig | ConvertTo-Json))
    Assert-Throws 'REMOTEHUB_WSS_PROFILE_INVALID' { New-RemoteHubWssStartInfo Relay $hostPath $root }
    Write-Output 'PASS: private WSS launcher role, environment isolation, CA and rejection checks (no processes started)'
} finally {
    $env:REMOTEHUB_V2_TOKEN = $originalToken
    $env:REMOTEHUB_V2_LAN_VIEWER_TOKEN = $originalRelay
    if ([IO.Directory]::Exists($root)) { [IO.Directory]::Delete($root, $true) }
}

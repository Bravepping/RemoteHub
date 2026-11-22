param(
    [ValidateSet('Watch','Snapshot')][string]$Action = 'Snapshot',
    [ValidatePattern('^[ABC](-[1-3])?$')][string]$Case = 'A',
    [ValidateSet('opened','hidden','restored','residency-disabled','five-pages','after-exit')][string]$Stage = 'opened',
    [ValidateRange(30,1800)][int]$Seconds = 1200
)
# Read-only process observation. Never launches, hides, clicks, captures or terminates an app.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$public = Join-Path $root 'artifacts/p106-manual'
$private = Join-Path $root 'artifacts/private/p106-manual'
$bindingFile = Join-Path $private ('case-' + $Case + '-binding.json')
$events = Join-Path $public ('case-' + $Case + '-process.jsonl')
$utf8 = [Text.UTF8Encoding]::new($false)
function Record($value) {
    $value.recordedAt = [DateTimeOffset]::Now.ToString('o')
    [IO.File]::AppendAllText($events, ($value | ConvertTo-Json -Depth 5 -Compress) + "`n", $utf8)
    $value | ConvertTo-Json -Depth 5 -Compress
}
function WindowMetadata($process) {
    try {
        $process.Refresh()
        return [ordered]@{
            mainWindowHandlePresent = $process.MainWindowHandle -ne [IntPtr]::Zero
            titleIsRemoteHub = $process.MainWindowTitle -ceq 'RemoteHub'
            responding = $process.Responding
            visibilityVerdict = 'NOT_INFERRED_FROM_PROCESS_METADATA'
        }
    } catch { return [ordered]@{ status = 'WINDOW_METADATA_UNAVAILABLE' } }
}
try {
    $manifest = Get-Content -Raw -Encoding UTF8 (Join-Path $public 'build.json') | ConvertFrom-Json
    if ($manifest.programRelativePath -cne 'artifacts/private/p106-manual/RemoteHub-P106-Manual.exe') {
        throw 'MANUAL_PROGRAM_PATH_REJECTED'
    }
    $exe = Join-Path $root $manifest.programRelativePath
    if ((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant() -cne $manifest.sha256) {
        throw 'MANUAL_PROGRAM_HASH_CHANGED'
    }
    $instances = @(Get-Process -Name 'RemoteHub-P106-Manual' -ErrorAction SilentlyContinue)
    if ($Action -eq 'Watch') {
        if (Test-Path -LiteralPath $bindingFile) { throw 'CASE_ALREADY_BOUND' }
        if ($instances.Count -ne 1) { throw 'NO_UNIQUE_CURRENT_TEST_INSTANCE' }
        $process = $instances[0]
        if ($process.Path -ine $exe) { throw 'CURRENT_TEST_PATH_MISMATCH' }
        # Retain this instance's process handle before asking the user to close it.
        $retainedHandle = $process.Handle
        if ($retainedHandle -eq [IntPtr]::Zero) { throw 'PROCESS_HANDLE_UNAVAILABLE' }
        $started = $process.StartTime.ToUniversalTime().ToString('o')
        $binding = [ordered]@{ processId = $process.Id; startedUtc = $started; sha256 = $manifest.sha256 }
        [IO.File]::WriteAllText($bindingFile, ($binding | ConvertTo-Json) + "`n", $utf8)
        Record ([ordered]@{ kind='BOUND'; case=$Case; processId=$process.Id; startedUtc=$started; sha256=$manifest.sha256; namedInstanceCount=$instances.Count; window=(WindowMetadata $process) })
        $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
        $last = ''
        while ([DateTime]::UtcNow -lt $deadline) {
            if ($process.WaitForExit(500)) {
                $remaining = @(Get-Process -Name 'RemoteHub-P106-Manual' -ErrorAction SilentlyContinue)
                $observedExitCode = $process.ExitCode
                Record ([ordered]@{ kind='EXIT_OBSERVED'; case=$Case; processId=$binding.processId; startedUtc=$started; exitCode=$observedExitCode; exitCodeAvailable=$null -ne $observedExitCode; processWaitSignaled=$true; remainingNamedInstanceCount=$remaining.Count; agentTerminatedApplication=$false })
                exit 0
            }
            $window = WindowMetadata $process
            $serialized = $window | ConvertTo-Json -Compress
            if ($serialized -cne $last) {
                Record ([ordered]@{ kind='WINDOW_METADATA'; case=$Case; processId=$process.Id; window=$window })
                $last = $serialized
            }
        }
        Record ([ordered]@{ kind='OBSERVER_TIMEOUT'; case=$Case; processId=$binding.processId; exitVerdict='NOT_OBSERVED'; agentTerminatedApplication=$false })
        exit 2
    }
    if (!(Test-Path -LiteralPath $bindingFile)) { throw 'CURRENT_CASE_NOT_BOUND' }
    $binding = Get-Content -Raw -Encoding UTF8 $bindingFile | ConvertFrom-Json
    $process = $instances | Where-Object Id -eq $binding.processId
    $state = 'NOT_FOUND_IN_PROCESS_ENUMERATION'
    $window = $null
    if ($process) {
        if ($process.StartTime.ToUniversalTime().ToString('o') -cne $binding.startedUtc -or $process.Path -ine $exe) {
            throw 'BOUND_PROCESS_IDENTITY_CHANGED'
        }
        $state = 'RUNNING_BOUND_INSTANCE'
        $window = WindowMetadata $process
    }
    Record ([ordered]@{ kind='SNAPSHOT'; case=$Case; stage=$Stage; processId=$binding.processId; state=$state; namedInstanceCount=$instances.Count; window=$window })
} catch {
    $code = if ($_.Exception.Message -match '^[A-Z_]+$') { $_.Exception.Message } else { 'PROCESS_OBSERVATION_UNAVAILABLE' }
    if (Test-Path -LiteralPath $public) {
        Record ([ordered]@{ kind='OBSERVER_ERROR'; case=$Case; error=$code; noApplicationActionTaken=$true })
    } else { Write-Output $code }
    exit 1
}

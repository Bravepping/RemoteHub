# Read only fixed experiment reports; export a bounded, typed whitelist, never raw JSON.
param([string]$SessionId = '')
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
& (Join-Path $PSScriptRoot 'Verify-SHA256.ps1') | Out-Null
$programSha = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot 'RemoteHub-R06.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
function Read-Report($Path) {
  $item = Get-Item -LiteralPath $Path
  if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $item.Length -gt 204800) { throw 'R06_REPORT_FILE_POLICY' }
  return (Get-Content -LiteralPath $item.FullName -Raw -Encoding UTF8 | ConvertFrom-Json)
}
function Field($Object, $Key) {
  if ($null -eq $Object) { return $null }
  $p = $Object.PSObject.Properties[$Key]
  if ($null -ne $p) { return $p.Value }
  return $null
}
function Number($Value) {
  if ($null -eq $Value) { return $null }
  if ($Value -is [bool] -or $Value -is [string] -or $Value -isnot [ValueType]) { throw 'R06_UNSAFE_NUMBER' }
  $n = [double]$Value
  if ([double]::IsNaN($n) -or [double]::IsInfinity($n) -or $n -lt 0 -or $n -gt 9007199254740991) { throw 'R06_UNSAFE_NUMBER' }
  return $Value
}
function Boolean($Value) {
  if ($null -ne $Value -and $Value -isnot [bool]) { throw 'R06_UNSAFE_BOOLEAN' }
  return $Value
}
function Safe-Enum($Value, $Allowed) {
  if ($null -eq $Value) { return $null }
  if ($Value -isnot [string] -or $Value -notin $Allowed) { throw 'R06_UNSAFE_ENUM' }
  return $Value
}
$errors = @('RECEIVER_INTERNAL_ERROR','RECEIVER_EVIDENCE_TIMEOUT','RECEIVER_EVIDENCE_SCHEMA','RECEIVER_EVIDENCE_METRICS','RECEIVER_EVIDENCE_SIZE','RECEIVER_EVIDENCE_ENCODING','RECEIVER_EVIDENCE_DIRECTORY','RECEIVER_EVIDENCE_OWNER','RECEIVER_EVIDENCE_STAGE_REPLAY','RECEIVER_EVIDENCE_EXISTS_OR_WRITE','RECEIVER_EVIDENCE_WRITE','RECEIVER_EVIDENCE_WRONG_ROLE','RECEIVER_EVIDENCE_LOCK','BROWSER_PC_MISSING','BROWSER_SELECTED_DIRECT_PATH_UNVERIFIED','BROWSER_SELECTED_RELAY_UNVERIFIED','P006_CLEANUP_PENDING','P006_WSS_CLOSED','P006_SIGNAL_LEASE_EXPIRED','P006_TASK_FAILED','P006_STOPPED','P006_RECEIVER_STOPPED','P006_SIGNAL_CLOSED','REPORT_WRITE','REPORT_EXISTS_OR_WRITE','WINDOW_FINAL_STATS_UNAVAILABLE','DIRECT_SELECTED_PAIR_NOT_APPROVED','CONTROL_CHANNEL_ERROR','CONTROL_RESPONSE_INVALID','CONTROL_SEND_FAILED','CONTROL_SEND_LIMIT','DIAG_GETSTATS_FAILURE','DIRECT_PAIR_NOT_SELECTED','DIRECT_CONNECT_TIMEOUT','P006_SESSION_DEADLINE','DIRECT_SESSION_INACTIVE')
function Error-Code($Value) {
  if ($null -eq $Value) { return $null }
  if ($Value -is [string] -and $Value -in $errors) { return $Value }
  return 'REDACTED_UNKNOWN_ERROR'
}
function Numbers($Object, $Keys) {
  $result = [ordered]@{}
  foreach ($k in $Keys) { $result[$k] = Number (Field $Object $k) }
  return $result
}
function Diagnostic($d) {
  if ($null -eq $d) { return $null }
  if ((Field $d 'version') -ne 1) { throw 'R06_DIAGNOSTIC_VERSION' }
  $r = [ordered]@{ version=1; approved=(Boolean (Field $d 'approved')); privateLocalPublicationApproved=(Boolean (Field $d 'private_local_publication_approved')) }
  $r.localSetSource = Safe-Enum (Field $d 'local_set_source') @('validated_published_answer')
  $r.remoteSetSource = Safe-Enum (Field $d 'remote_set_source') @('authenticated_validated_offer')
  foreach ($side in @('local','remote')) {
    $s = Field $d $side
    $v = Numbers $s @('approved_count','same_address_count','same_port_count','same_type_count','same_endpoint_count','exact_match_count')
    $v.reason = Safe-Enum (Field $s 'reason') @('matched','session_inactive','candidate_set_missing','session_scope_mismatch','protocol_rejected','component_rejected','candidate_type_rejected','address_range_rejected','type_mismatch','candidate_type_absent_from_sdp','port_changed','address_changed','endpoint_not_in_approved_sdp')
    $v.candidateType = Safe-Enum (Field $s 'candidate_type') @('host','srflx','relay','prflx','unsupported')
    $v.protocol = Safe-Enum (Field $s 'protocol') @('udp','tcp','unsupported')
    $v.addressClass = Safe-Enum (Field $s 'address_class') @('invalid_or_ipv6','loopback','unspecified','link_local','multicast','private','shared','benchmark','public','reserved')
    $r[$side] = $v
  }
  return $r
}
function Evidence($Path, $Expected) {
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
  $e = Read-Report $Path
  if ((Field $e 'version') -ne 1 -or (Field $e 'stage') -ne $Expected) { throw 'R06_EVIDENCE_SCHEMA' }
  $s = Field $e 'snapshot'
  $c = Field $e 'cleanup'
  $r = [ordered]@{ version=1; stage=$Expected; capture=(Safe-Enum (Field $e 'capture') @('live','last_known','unavailable')); sampledAtMs=(Number (Field $e 'sampledAtMs')); recordedAtMs=(Number (Field $e 'recordedAtMs')); snapshot=$null; cleanup=[ordered]@{}; errors=@() }
  if ($r.capture -eq 'unavailable') {
    if ($null -ne $s -or $null -ne $r.sampledAtMs) { throw 'R06_CAPTURE_SCHEMA' }
  } elseif ($null -eq $s -or $null -eq $r.sampledAtMs) { throw 'R06_CAPTURE_SCHEMA' }
  if ($null -ne $r.sampledAtMs -and $r.sampledAtMs -gt $r.recordedAtMs) { throw 'R06_SAMPLE_TIME_INVALID' }
  foreach ($k in @('controlClosed','peerClosed','tracksStopped','nativeResourcesReleased')) { $r.cleanup[$k] = Boolean (Field $c $k) }
  $errs = @(Field $e 'errors')
  if ($errs.Count -gt 8) { throw 'R06_TOO_MANY_ERRORS' }
  $r.errors = @($errs | ForEach-Object { if ($null -ne $_) { Error-Code $_ } })
  if ($null -ne $s) {
    $snapshot = Numbers $s @('elapsedMs','framesReceived','framesDecoded','framesRendered','videoWidth','videoHeight','packetsReceived','bytesReceived','freezeCount')
    $snapshot.peerMatches = Boolean (Field $s 'peerMatches')
    foreach ($k in @('localCandidateType','remoteCandidateType')) { $snapshot[$k] = Safe-Enum (Field $s $k) @('host','srflx','relay','prflx') }
    $snapshot.connectionPath = Safe-Enum (Field $s 'connectionPath') @('direct','relay','unknown')
    $snapshot.connectionState = Safe-Enum (Field $s 'connectionState') @('new','connecting','connected','disconnected','failed','closed')
    $snapshot.iceState = Safe-Enum (Field $s 'iceState') @('new','checking','connected','completed','disconnected','failed','closed')
    $snapshot.control = Numbers (Field $s 'control') @('sentMoves','ackReceived','pending','anomalies','lastTxSeq','lastRxSeq','openEvents','messageEvents','closeEvents','errorEvents')
    $r.snapshot = $snapshot
  }
  return $r
}
$dir = Join-Path $PSScriptRoot 'artifacts/private/p007b-r06'
$info = Get-Item -LiteralPath $dir
if (-not $info.PSIsContainer -or ($info.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'R06_REPORT_DIRECTORY_POLICY' }
$idPattern = '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'
if ($SessionId -and $SessionId -notmatch ('^'+$idPattern+'$')) { throw 'R06_OWNER_FORMAT' }
if (-not $SessionId) {
  $last = Get-ChildItem -LiteralPath $dir -File | Where-Object { $_.Name -match ('^'+$idPattern+'-(final|native-final|window-close)\.json$') } | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
  if ($null -eq $last) { throw 'R06_NO_TERMINAL_REPORT' }
  $SessionId = $last.Name.Substring(0,36)
}
$native = $null
foreach ($suffix in @('final','window-close','native-final')) {
  $p = Join-Path $dir "$SessionId-$suffix.json"
  if (Test-Path -LiteralPath $p -PathType Leaf) { $native = Read-Report $p; break }
}
if ($null -eq $native -or (Field $native 'buildTag') -ne 'p007b-r06-v1') { throw 'R06_DEDICATED_REPORT_REQUIRED' }
$n = Field $native 'native'; $source = Field $n 'source'
if ($null -ne $source -and (Field $source 'source_kind') -ne 'synthetic_nv12') { throw 'R06_NON_SYNTHETIC_REPORT_REJECTED' }
$sourceSummary = $null
if ($null -ne $source) {
  $sourceSummary = Numbers $source @('frames_sent','elapsed_ms')
  $sourceSummary.sourceKind = 'synthetic_nv12'
  $sourceSummary.resourcesReleased = Boolean (Field $source 'resources_released')
  $sourceSummary.error = Error-Code (Field $source 'error')
  $sourceSummary.control = Numbers (Field $source 'control') @('accepted_moves','acks_sent','rejected','last_rx_seq','last_tx_seq')
  $sourceSummary.directPairDiagnostic = Diagnostic (Field $source 'direct_pair_diagnostic')
}
$result = [ordered]@{ version=1; programSha256=$programSha; scope='R06 dedicated synthetic-only local reports; not a complete-stream or historical-root-cause verdict'; role=(Safe-Enum (Field $n 'role') @('sender','receiver')); recordedAtMs=(Number (Field $native 'recordedAtMs')); nativeResourcesReleased=(Boolean (Field $n 'resources_released')); error=(Error-Code (Field $n 'error')); source=$sourceSummary; receiverPreClose=(Evidence (Join-Path $dir "$SessionId-receiver-pre-close.json") 'pre_close'); receiverFinal=(Evidence (Join-Path $dir "$SessionId-receiver-final.json") 'cleanup') }
$text = $result | ConvertTo-Json -Depth 12
if ($text.Length -gt 16384) { throw 'R06_EXPORT_SIZE' }
$output = Join-Path $PSScriptRoot ('R06-summary-'+[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')+'.json')
$bytes = [Text.Encoding]::UTF8.GetBytes($text+"`n")
$stream = [IO.File]::Open($output,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
try { $stream.Write($bytes,0,$bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
Write-Output 'R06_SUMMARY_EXPORTED'

# Run from the x64 Native Tools PowerShell for Visual Studio; installs no SDK.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'

if ($env:OS -ne 'Windows_NT') { throw 'R06_BUILD_REQUIRES_WINDOWS' }
foreach ($tool in @('git', 'pnpm', 'cargo', 'cl.exe', 'lib.exe', 'link.exe')) {
    if (!(Get-Command $tool -ErrorAction SilentlyContinue)) { throw "R06_BUILD_TOOL_MISSING: $tool" }
}

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "R06_BUILD_FAILED: $Program ($LASTEXITCODE)" }
}

$repo = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repo
try {
    $branch = & git symbolic-ref --quiet --short HEAD
    if ($LASTEXITCODE -ne 0 -or $branch -cne 'v2/clean-core') { throw 'R06_BUILD_WRONG_BRANCH' }
    $revision = & git rev-parse --verify --short=12 HEAD
    if ($LASTEXITCODE -ne 0 -or $revision -cnotmatch '\A[0-9a-f]{12,40}\z') {
        throw 'R06_BUILD_REVISION_INVALID'
    }
    $work = & git status --porcelain
    if ($LASTEXITCODE -ne 0 -or $work) { throw 'R06_BUILD_WORKTREE_NOT_CLEAN' }
    $out = Join-Path $repo 'artifacts'
    $null = [IO.Directory]::CreateDirectory($out)
    $zipName = "RemoteHub-R06-Synthetic-Windows-x64-$revision.zip"
    $zip = Join-Path $out $zipName
    if (Test-Path -LiteralPath $zip) { throw 'R06_BUILD_ARCHIVE_EXISTS' }
    $temp = Join-Path $out ('r06-build-' + [guid]::NewGuid().ToString('N'))
    $target = Join-Path $temp 'target'
    $stage = Join-Path $temp 'stage'
    $null = [IO.Directory]::CreateDirectory($stage)
    $oldTarget = $env:CARGO_TARGET_DIR
    try {
        Invoke-Checked 'pnpm' @('install', '--frozen-lockfile')
        Invoke-Checked 'pnpm' @('build')
        $env:CARGO_TARGET_DIR = $target
        Invoke-Checked 'cargo' @(
            'build', '-p', 'remotehub-desktop', '--target', 'x86_64-pc-windows-msvc',
            '--profile', 'probe', '--features', 'p007b-diag,custom-protocol', '--locked'
        )
        $built = Join-Path $target 'x86_64-pc-windows-msvc\probe\remotehub-desktop.exe'
        $exe = Join-Path $stage 'remotehub-r06-synthetic.exe'
        Copy-Item -LiteralPath $built -Destination $exe
        $exeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash.ToLowerInvariant()
        $checksum = Join-Path $stage 'SHA256SUMS.txt'
        Set-Content -LiteralPath $checksum -Value "$exeHash *remotehub-r06-synthetic.exe" -Encoding ASCII
        # Explicit allowlist: never package profiles, certificates, logs or other checkout files.
        $tempZip = Join-Path $temp $zipName
        Compress-Archive -LiteralPath $exe,$checksum -DestinationPath $tempZip -ErrorAction Stop
        [IO.File]::Move($tempZip, $zip)
        $zipHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
        Write-Output "R06_ARCHIVE=$zip"
        Write-Output "R06_ARCHIVE_SHA256=$zipHash"
    } finally {
        $env:CARGO_TARGET_DIR = $oldTarget
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
} finally {
    Pop-Location
}

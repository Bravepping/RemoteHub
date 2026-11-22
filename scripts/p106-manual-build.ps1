$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    if ($env:TAURI_CONFIG) { throw 'UNEXPECTED_TAURI_OVERRIDE' }
    if (@(Get-Process 'RemoteHub-P106-Manual' -ErrorAction SilentlyContinue).Count) {
        throw 'MANUAL_TEST_INSTANCE_ALREADY_RUNNING'
    }
    pnpm build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo build -p remotehub-desktop --bin remotehub-desktop --profile probe --no-default-features --features custom-protocol --locked --offline
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $metadataText = cargo metadata --format-version 1 --no-default-features --features remotehub-desktop/custom-protocol --locked --offline
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $metadata = $metadataText | ConvertFrom-Json
    $desktop = $metadata.packages | Where-Object name -eq 'remotehub-desktop'
    $features = @($metadata.resolve.nodes | Where-Object id -eq $desktop.id | Select-Object -ExpandProperty features)
    if ($features.Count -ne 1 -or $features[0] -ne 'custom-protocol') {
        throw 'NON_DEFAULT_DESKTOP_FEATURES_REJECTED'
    }
    $private = Join-Path (Get-Location) 'artifacts/private/p106-manual'
    $public = Join-Path (Get-Location) 'artifacts/p106-manual'
    New-Item -ItemType Directory -Force -Path $private,$public | Out-Null
    $exe = Join-Path $private 'RemoteHub-P106-Manual.exe'
    Copy-Item -LiteralPath 'target/probe/remotehub-desktop.exe' -Destination $exe
    $sha = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($sha -ne (Get-FileHash -LiteralPath 'target/probe/remotehub-desktop.exe' -Algorithm SHA256).Hash.ToLowerInvariant()) {
        throw 'DELIVERY_HASH_MISMATCH'
    }
    $commit = (git -c ('safe.directory=' + (Get-Location).Path.Replace('\','/')) rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $result = [ordered]@{
        recordedAt = [DateTimeOffset]::Now.ToString('o')
        sourceCommit = $commit
        programRelativePath = 'artifacts/private/p106-manual/RemoteHub-P106-Manual.exe'
        sha256 = $sha
        desktopCargoFeatures = $features
        profile = 'probe'
        ordinaryDesktop = $true
        buildStartedApplication = $false
        mediaOrCloudConfigIncluded = $false
        testStatus = 'PENDING_USER_ACTION'
    }
    [IO.File]::WriteAllText((Join-Path $public 'build.json'), ($result | ConvertTo-Json -Depth 5) + "`n", [Text.UTF8Encoding]::new($false))
    $result | ConvertTo-Json -Depth 5
} finally { Pop-Location }

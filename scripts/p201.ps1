$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'use-toolchain.ps1')
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
# Trust only this known checkout for child Git reads; do not change global Git config/ownership.
$taskGitIndex = if ($env:GIT_CONFIG_COUNT) { [int]$env:GIT_CONFIG_COUNT } else { 0 }
[Environment]::SetEnvironmentVariable("GIT_CONFIG_KEY_$taskGitIndex", 'safe.directory', 'Process')
[Environment]::SetEnvironmentVariable("GIT_CONFIG_VALUE_$taskGitIndex", (Split-Path $PSScriptRoot -Parent).Replace('\','/'), 'Process')
$env:GIT_CONFIG_COUNT = [string]($taskGitIndex + 1)
Push-Location (Split-Path $PSScriptRoot -Parent)
try { node scripts/p201-checks.mjs; exit $LASTEXITCODE } finally { Pop-Location }

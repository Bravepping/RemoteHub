$ErrorActionPreference = 'Stop'
$info = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'BUILD_INFO.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if ($info.buildTag -ne 'p007b-r06-v1' -or $info.activeCredentialsIncluded -ne $false -or $info.desktopCapture -ne $false) { throw 'R06_BUILD_METADATA_INVALID' }
$exe = Join-Path $PSScriptRoot 'RemoteHub-R06.exe'
$file = Get-Item -LiteralPath $exe
if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'R06_FILE_POLICY' }
$actual = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $info.programSha256) { throw 'R06_PROGRAM_SHA256_MISMATCH' }
Write-Output ('SHA256 OK: '+$actual)

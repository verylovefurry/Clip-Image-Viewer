[CmdletBinding()]
param([Parameter(Mandatory)][string]$Executable)

$ErrorActionPreference = 'Stop'
$repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [IO.Path]::GetFullPath((Join-Path $repository '..\..\outputs\rust-drop-tests'))
$session = Join-Path $output ([Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session -Force | Out-Null
$fixtures = Join-Path $session '유니코드 경로'
New-Item -ItemType Directory -Path $fixtures | Out-Null
Copy-Item -LiteralPath (Join-Path $repository 'test\sample.png') -Destination (Join-Path $fixtures '이미지.png')
Copy-Item -LiteralPath (Join-Path $repository 'test\sample.clip') -Destination (Join-Path $fixtures '문서.clip')
$executablePath = (Resolve-Path -LiteralPath $Executable).Path
foreach ($type in @('image-to-clip','clip-to-image')) {
  $initial = if ($type -eq 'image-to-clip') { Join-Path $fixtures '이미지.png' } else { Join-Path $fixtures '문서.clip' }
  $drop = if ($type -eq 'image-to-clip') { Join-Path $fixtures '문서.clip' } else { Join-Path $fixtures '이미지.png' }
  $env:CLIPVIEW_USER_DATA_DIR = Join-Path $session "$type-profile"
  $env:CLIPVIEW_RUST_SMOKE = Join-Path $session "$type.json"
  $env:CLIPVIEW_RUST_DROP_TEST = $drop
  $stdout = Join-Path $session "$type.stdout.log"
  $stderr = Join-Path $session "$type.stderr.log"
  $process = Start-Process -FilePath $executablePath -ArgumentList ('"' + $initial + '"') -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
  if (-not $process.WaitForExit(60000)) { $process.Kill(); $process.WaitForExit(); throw "$type timed out" }
  $process.WaitForExit()
  if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $env:CLIPVIEW_RUST_SMOKE)) {
    Get-Content -LiteralPath $stderr
    throw "$type failed, exit $($process.ExitCode)"
  }
  $result = Get-Content -Raw -LiteralPath $env:CLIPVIEW_RUST_SMOKE | ConvertFrom-Json
  if (-not $result.loaded -or -not $result.dropAccepted) { throw "$type failed: $($result | ConvertTo-Json -Compress)" }
  Write-Output "$type passed: $($result.width)x$($result.height)"
}

[CmdletBinding()]
param([switch]$UiOnly, [switch]$DebugNative, [switch]$Package)

$ErrorActionPreference = 'Stop'
$repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [IO.Path]::GetFullPath((Join-Path $repository '..\..\outputs'))
$env:CARGO_HOME = Join-Path $output 'rust-cargo'
$uiTarget = Join-Path $output 'rust-ui-target'
$nativeTarget = Join-Path $output 'rust-target'
$dist = Join-Path $repository 'rust\dist'
$bindgen = Join-Path $output 'rust-tools\bin\wasm-bindgen.exe'
if (-not (Test-Path -LiteralPath $bindgen)) {
  throw "wasm-bindgen-cli 0.2.130 is required at $bindgen. See docs/RUST_MIGRATION.md."
}

Push-Location $repository
try {
  $env:CARGO_TARGET_DIR = $uiTarget
  & cargo build --manifest-path rust/Cargo.toml --locked -p clipview-ui --target wasm32-unknown-unknown --release
  if ($LASTEXITCODE -ne 0) { throw 'Rust UI compilation failed.' }
  New-Item -ItemType Directory -Path $dist -Force | Out-Null
  & $bindgen (Join-Path $uiTarget 'wasm32-unknown-unknown\release\clipview_ui.wasm') --target web --out-dir $dist --out-name clipview_ui --no-typescript
  if ($LASTEXITCODE -ne 0) { throw 'WASM browser bindings failed.' }

  # The page and stylesheet are the visual authority carried over from the Electron UI.
  # Tauri supplies the CSP header, and the page loads the Rust/WASM entry point.
  foreach ($asset in 'index.html', 'styles.css') {
    Copy-Item -LiteralPath (Join-Path $repository "rust\ui\assets\$asset") -Destination (Join-Path $dist $asset)
  }
  # This loader is platform glue only. All application state and event handling live in ui/src/lib.rs.
  [IO.File]::WriteAllText((Join-Path $dist 'bootstrap.js'), 'import init from "./clipview_ui.js"; await init();', [Text.UTF8Encoding]::new($false))

  if (-not $UiOnly) {
    $env:CARGO_TARGET_DIR = $nativeTarget
    if ($Package) {
      if ($DebugNative) { throw '-Package requires a release build.' }
      # License notices are regenerated from Cargo.lock so they always match the shipped crates.
      & node (Join-Path $repository 'scripts\collect-rust-licenses.js')
      if ($LASTEXITCODE -ne 0) { throw 'Rust license notice collection failed.' }
      # The Tauri CLI builds the release executable and the NSIS installer in one step.
      $tauri = Join-Path $output 'rust-tools\bin\cargo-tauri.exe'
      if (-not (Test-Path -LiteralPath $tauri)) { throw "tauri-cli 2 is required at $tauri. See docs/RUST_MIGRATION.md." }
      Push-Location (Join-Path $repository 'rust\native')
      try { & $tauri build --bundles nsis --ci } finally { Pop-Location }
      if ($LASTEXITCODE -ne 0) { throw 'Tauri release build failed.' }
    } else {
      $arguments = @('build', '--manifest-path', 'rust/Cargo.toml', '--locked', '-p', 'clip-image-viewer-native')
      if (-not $DebugNative) { $arguments += '--release' }
      & cargo @arguments
      if ($LASTEXITCODE -ne 0) { throw 'Native Rust compilation failed.' }
    }
    $profile = if ($DebugNative) { 'debug' } else { 'release' }
    $executable = Join-Path $nativeTarget "$profile\ClipImageViewer.exe"
    Write-Output $executable

    if ($Package) {
      $version = (Get-Content -Raw -LiteralPath (Join-Path $repository 'rust\native\tauri.conf.json') | ConvertFrom-Json).version
      # Asset names are what the updater looks for; keep them in sync with src/updater.rs.
      $release = Join-Path $output "rust-release-$version"
      if (Test-Path -LiteralPath $release) { Remove-Item -LiteralPath $release -Recurse -Force }
      New-Item -ItemType Directory -Path $release | Out-Null
      $installer = Get-ChildItem -LiteralPath (Join-Path $nativeTarget 'release\bundle\nsis') -Filter '*-setup.exe' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
      if (-not $installer) { throw 'NSIS installer was not produced.' }
      $setupName = "Clip-Image-Viewer-Setup-$version-rust-x64.exe"
      Copy-Item -LiteralPath $installer.FullName -Destination (Join-Path $release $setupName)

      $name = "Clip-Image-Viewer-$version-rust-windows-x64-portable"
      $stage = Join-Path $release $name
      $licenses = Join-Path $stage 'licenses'
      New-Item -ItemType Directory -Path $licenses -Force | Out-Null
      Copy-Item -LiteralPath $executable -Destination $stage
      Copy-Item -LiteralPath (Join-Path $repository 'LICENSE') -Destination (Join-Path $stage 'LICENSE.txt')
      Copy-Item -LiteralPath (Join-Path $repository 'licenses\RUST_THIRD_PARTY_NOTICES.md') -Destination (Join-Path $stage 'THIRD_PARTY_NOTICES.md')
      Copy-Item -LiteralPath (Join-Path $repository 'licenses\rust') -Destination $licenses -Recurse
      # Same set as the installer resources in rust/native/tauri.conf.json.
      foreach ($file in 'Clip-Image-Viewer-LICENSE.txt', 'clip_to_psd-LICENSE.txt', 'libbpg-LICENSE.txt',
          'LibRaw-0.22.3-LICENSE.CDDL.txt', 'LibRaw-0.22.3-COPYRIGHT.txt',
          'MIT.txt', 'Apache-2.0.txt', 'BSD-3-Clause.txt', 'BSL-1.0.txt', 'MPL-2.0.txt') {
        Copy-Item -LiteralPath (Join-Path $repository "licenses\$file") -Destination $licenses
      }
      # portable.flag selects the portable updater and disables file associations.
      New-Item -ItemType File -Path (Join-Path $stage 'portable.flag') | Out-Null
      # Korean file name built from code points so the script itself stays ASCII for Windows PowerShell 5.1.
      $noticeName = (-join ([char[]](0xD3EC, 0xD130, 0xBE14, 0x20, 0xC0AC, 0xC6A9, 0xC790, 0x20, 0xC548, 0xB0B4))) + '.txt'
      # UTF-8 with BOM so Notepad shows the Korean notice correctly on every Windows version.
      $notice = [IO.File]::ReadAllText((Join-Path $repository 'docs\PORTABLE_NOTICE.ko.txt'), [Text.Encoding]::UTF8)
      [IO.File]::WriteAllText((Join-Path $stage $noticeName), $notice.Replace("`r`n", "`n").Replace("`n", "`r`n"), [Text.UTF8Encoding]::new($true))
      $archive = Join-Path $release "$name.zip"
      # UTF-8 entry names keep the Korean file name intact on every Windows locale and in the
      # updater's ZIP extraction; Compress-Archive stores names in the OEM code page.
      Add-Type -AssemblyName System.IO.Compression.FileSystem
      [IO.Compression.ZipFile]::CreateFromDirectory($stage, $archive, [IO.Compression.CompressionLevel]::Optimal, $false, [Text.Encoding]::UTF8)
      Remove-Item -LiteralPath $stage -Recurse -Force

      $sums = foreach ($asset in $setupName, "$name.zip") {
        '{0}  {1}' -f (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $release $asset)).Hash.ToLowerInvariant(), $asset
      }
      [IO.File]::WriteAllLines((Join-Path $release 'SHA256SUMS.txt'), [string[]]$sums, [Text.UTF8Encoding]::new($false))
      Get-ChildItem -LiteralPath $release | ForEach-Object { Write-Output $_.FullName }
    }
  }
} finally { Pop-Location }

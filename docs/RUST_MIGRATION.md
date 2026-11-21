# Rust Migration

## Scope and Release Gate

The application logic, including UI event handling, has been migrated to Rust
without changing the established interface. On 2026-11-22 the owner removed the
Electron source (`src/`, npm packaging, JavaScript tests) and its build outputs
from the working tree; the 1.x code remains in Git history up to the v1.8.0
commit and is the behavioural reference for the open items below. The checklist
under "Required Before Replacement" records what is still unverified; a
successful build alone does not establish parity.

The Rust build has its own update channel (see "Packaging, license notices and
updates"); it only installs Rust release assets and never touches the Electron
release assets or the Electron updater.

## Architecture

- `rust/native`: Rust/Tauri host, filesystem and dialogs, image processing,
  document parsing, bounded binary asset delivery, and OS integration.
- `rust/ui`: Rust compiled to WebAssembly. UI state, DOM events, pointer zoom,
  keyboard shortcuts, playback, panels, and settings are authored in Rust.
- `rust/core`: dependency-free compositor, viewport, and window-bounds rules.
- `rust/dist`: generated output; never edit or commit it.

The native PSD/PSB path currently reads saved merged previews in 8/16-bit RGB,
grayscale and CMYK, including raw, RLE, ZIP and ZIP prediction. Embedded ICC
profiles are converted to sRGB; unprofiled CMYK is approximate. PSD/PSB layer
inspection, thumbnails, picking, and visibility changes are available for
supported stored raster layers. Edited compositions are approximate: layer
styles, adjustments, vector masks, and some blend modes are not reproduced.
Large or unsupported documents may expose metadata without layer toggles.
32-bit HDR and documents without a merged preview are not yet supported.
Standalone PSD/PSB files can be up to 2 GiB when their
saved composite is at most 128 MiB; other source files remain limited to
128 MiB. Decoded dimensions are limited to 64 megapixels, lower than the
released app. On Windows, otherwise unsupported ordinary disk images can use
installed Windows Imaging Component codecs (verified with a generated JPEG XR
file); availability depends on the machine. The fallback converts recognized
embedded RGB ICC profiles and common EXIF orientation tags, but does not
guarantee color or orientation parity for every Windows codec.
Archive entries use a temporary file that is removed after decoding.
RAW files are opened in three steps. (1) A structurally referenced embedded
camera JPEG of at least 1024 px is parsed in-process (strict DNG/CR2/NEF/ARW/
RW2/ORF readers, then a generic container scan). (2) Otherwise LibRaw 0.22.3,
built from unmodified sources in `third_party/` and linked statically under
CDDL-1.0, runs in an isolated worker process (`src/libraw.rs`): the viewer
starts its own executable with a Low-integrity copy of the user token, assigns
it to a Job Object before it runs (kill on close, one process, 3 GiB commit,
die on unhandled exception, no desktop/clipboard/global-atom access), passes the
file bytes and result only through stdin/stdout and kills it after 60 s. LibRaw
returns its embedded thumbnail when large enough, or a half-size (above 24 MP)
or full bilinear development. (3) Windows RAW codecs are the last attempt. A
debug-only probe test confirmed the worker cannot create a file in the user's
temp folder. This is not an AppContainer: Low integrity does not stop reading
files the user can read, and network access is not blocked. The Electron app
ran LibRaw inside its own process. The Microsoft Raw Image Extension itself is
a Store package with no published redistribution rights and is not bundled.
Still JPEG XL uses a native Rust decoder with ICC conversion; animated RGB(A)
JPEG XL is converted to APNG under frame, pixel, allocation and time limits
(associated alpha is rejected). Its decoder's initial header allocation is not strictly
bounded, and broader ICC, CMYK and HDR compatibility remains unverified.
Baseline JPEG 2000 JP2 and simple J2K codestreams use a bounded native decoder.
Raw J2K has no color profile, so one- and three-component images are interpreted
as grayscale and sRGB respectively. ICC, JPX, palette and some alpha layouts are
not supported by this path, and the pixel limit is 16 megapixels.

### CLIP/CSP layers: separated from the first frame (intentional divergence)

The Electron app shows CSP's saved `CanvasPreview` first and, after a toggle,
replaces only pixels where two independent composites differ. Hidden layers
could therefore leave preview residue, and the result mixed two renderers. The
Rust build no longer does this. When a document has independently renderable
layers and fits the composite budget, it composes every layer from its own
stored data on open and after every toggle; the merged preview is never used as
a base. Restoring the saved visibility returns exactly the initial image. The
merged preview is still used for the file-list thumbnail, for documents without
renderable layers, and as an explicitly labelled fallback (`note`, toggles off)
if layer composition fails. `layerDocument.independentRendering` and
`unrenderedLayers` report which path was taken and how many visible leaves have
no independent data.

Ported from the reference renderer: flat and linear/radial gradient fills
(`GradationFillInfo`, pad/repeat/reflect), halftone tone effects on groups and
non-raster sources, frame-folder panel masks from `VectorObjectList` closed
paths, and the vector stroke renderer (`clip/vector.rs`). The stroke renderer
mirrors `clipVectorSvg`/`clip-brush-renderer.js`: brush styles with pressure,
velocity, smoothing and tilt effector graphs, curve handles, elliptical stamps
with the same spacing and stamp budget, brush-tip patterns, spray with saved
seeds, dual brushes, and closed-path fills drawn between ordinary and spray
strokes. It draws vector layers, balloon shapes under their text objects and
comic frame lines. It renders analytically at output resolution instead of
librsvg at 2× plus Lanczos, so edges differ slightly from Electron; CSP's brush
engine (textures, ribbons, its random sequence) is still not reproduced. Unit
tests caught two parsing bugs (gradient section ends, CR3-style box sizes)
during the port. Tile inflation, text sprites, edge dilation, tone, vector
compositing and full canvas blending were optimized; outputs are unchanged by
these optimizations.

Local measurement, release build, one warm-up: RecWild1_KR pages 0006–0019 open
in about 150–340 ms (the stored preview alone took about 8 ms); Tos_Tos_F_ski
pages in about 100–150 ms. Toggles compose once instead of twice. Against the
stored CSP preview, displayed-over-white RMSE is about 45–84 (RecWild1_KR) and
38–51 (Tos_Tos_F_ski), mean 52.7 over all 24 files; page0006 went from 158.8 to
50.8 after the panel mask and vector strokes. **No file matches CSP pixel for
pixel.** Still missing in Rust: font-based text fallback when a text object has
no render cache, transformed image-material originals, low-resolution thumbnail
fallback, and correction/3D layers. These are omitted, not approximated, and
are counted in `unrenderedLayers`.

### Image formats added in Rust

| Extensions | Rust path | Verified with |
|---|---|---|
| HEIC/HEIF/HIF | `heif-oxide` 0.2 + `rust_h265` 0.4 (pure Rust, MIT/Apache-2.0); Windows codecs only as a second attempt | libheif example and four Nokia HEIF conformance files: RMSE 0–1.43 against the Windows HEIF codec |
| BPG (still and animated, colour only) | `bpg.rs`: BPG header and extensions, VPS/SPS rebuilt from the compact header, `rust_h265` decode, libbpg 0.9.8 chroma interpolation and colour matrices; animations become APNG | libbpg reference RGBA in `outputs/bpg-eval`: byte-identical for a 10-bit file; the colour planes of alpha fixtures (including 40 animated frames) match libbpg on opaque pixels through a test-only path |
| PCX | `pcx.rs`: 1/2/4/8-bit indexed, EGA planar, 8-bit RGB/RGBA, RLE | Pillow-written RGB, palette, grey and 1-bit files: identical pixels |
| Animated JXL | existing `jxl_animation.rs` (allocation tracker, frame/pixel/time limits) is now wired in; output is APNG | three animated fixtures from `outputs/jxl-test-harness` |
| RAW beyond DNG/CR2/NEF/ARW/RW2/ORF | `raw_preview_generic.rs`: largest validated embedded JPEG from any TIFF-structured RAW, RAF, CR3 (full-size track and PRVW) and MRW; then the isolated LibRaw worker | synthetic containers, a real RAF, NEF/ORF/RW2/ARW/CR2/DNG samples; LibRaw thumbnails and half-size developments of CR2, RW2 and NEF |

Support ended (by decision, rarely used): BPG with alpha, grayscale BPG, CRW and
X3F. CRW and X3F are no longer listed extensions; alpha and grayscale BPG files
are rejected with a message (the pure-Rust HEVC decoders cannot decode their
monochrome streams). 4:2:2 and CMYK BPG are rejected too. HEIF monochrome alpha
and 4:4:4 files fall back to Windows codecs. The local ExifTool CR3 test file
has its images removed, so CR3 parsing is verified with synthetic files. Fixtures stay outside the repository (`outputs/format-samples`,
`outputs/raw-samples`, `outputs/heif-samples`); run
`CLIPVIEW_FORMAT_SAMPLES=<folder> cargo test --release -p clip-image-viewer-native opens_local_format -- --ignored --nocapture`.

### Packaging, license notices and updates

`scripts/build-rust.ps1 -Package` regenerates the license notices, builds the
release executable and NSIS installer with tauri-cli 2 (install it to
`outputs/rust-tools` with `cargo install tauri-cli --version ^2 --locked`), and
writes `outputs/rust-release-<version>/` containing
`Clip-Image-Viewer-Setup-<version>-rust-x64.exe`,
`Clip-Image-Viewer-<version>-rust-windows-x64-portable.zip` and `SHA256SUMS.txt`.
Upload all three to a GitHub release for the updater to find them.

`scripts/collect-rust-licenses.js` walks `cargo metadata` from the native host
(x86_64-pc-windows-msvc) and the WASM UI, skipping dev and build-script-only
crates, and writes each crate's license files to `licenses/rust/` plus
`licenses/RUST_THIRD_PARTY_NOTICES.md` (340 crates at the time of writing;
crates without a license file get the SPDX standard text). LibRaw's CDDL text
and COPYRIGHT, the libbpg MIT notice and the clip_to_psd notice are added.
Both packages ship `THIRD_PARTY_NOTICES.md` and `licenses/`; the portable ZIP
also ships a Korean notice recommending that portable users delete all files of
an older (Electron) portable folder before copying the Rust version in.

`src/updater.rs` checks GitHub Releases shortly after start and on request,
for release builds only. It accepts only Rust asset names for the running
flavour (portable when `portable.flag` is present, installer otherwise),
offers prereleases only to prerelease users, accepts downloads only under the
repository's release download URL, and requires a matching `SHA256SUMS.txt`
line. A portable update is extracted with path, count and size limits and
applied by the new executable after the old process exits, backing up each
replaced file and restoring all of them on failure; the installer runs with
`/P /R /UPDATE`. Like the Electron updater, integrity relies on the release's
checksum file rather than a separate signature. Tests cover asset selection,
checksum parsing, rollback and a full download through a local HTTP server; a
live check against the current GitHub releases reports no Rust update yet.

The page and stylesheet carried over from the Electron UI live in
`rust/ui/assets/` and remain the visual authority; the page differs from the
Electron one only by its CSP meta tag (Tauri sends the header) and its WASM
entry point. No handwritten application JavaScript is shipped and Node is not
used at run time. Compiler-generated
wasm-bindgen browser bindings and a two-line loader are still JavaScript platform
glue; they contain no handwritten application behavior. The Rust application
also still uses Windows WebView2, so a language migration does not eliminate the
web-renderer processes or the memory needed for decoded image pixels.

References: [Tauri Rust/WASM frontend](https://v2.tauri.app/start/frontend/trunk/),
[wasm-bindgen browser bindings](https://rustwasm.github.io/docs/wasm-bindgen/examples/hello-world.html).

## Building Locally

Requirements: Rust MSVC toolchain, Visual C++ Build Tools, Windows SDK, WebView2,
the `wasm32-unknown-unknown` Rust target, `wasm-bindgen-cli` 0.2.130, Node.js
(license notice generation) and, for `-Package`, `tauri-cli` 2.
Cargo dependencies are locked in `rust/Cargo.lock`.

From the repository root, the current build script stores Cargo caches and
compiled output in `../../outputs`, separate from the released binaries:

```powershell
rustup target add wasm32-unknown-unknown
$env:CARGO_HOME = (Resolve-Path ../../outputs).Path + '\rust-cargo'
$env:CARGO_TARGET_DIR = (Resolve-Path ../../outputs).Path + '\rust-tool-target'
cargo install wasm-bindgen-cli --version 0.2.130 --locked --root ../../outputs/rust-tools
cargo install tauri-cli --version "^2" --locked --root ../../outputs/rust-tools
./scripts/build-rust.ps1
```

`-UiOnly` builds just the Rust/WASM UI and copied visual assets.
`-DebugNative` builds the native host in the debug profile.
`-Package` builds the release installer, portable ZIP and checksums.

## Verification

```powershell
node test/rust-assets.test.js
cargo test --manifest-path rust/Cargo.toml -p clipview-core
cargo test --manifest-path rust/Cargo.toml -p clip-image-viewer-native
# Private CLIP samples (test/samples, not distributed):
cargo test --release --manifest-path rust/Cargo.toml -p clip-image-viewer-native separates_real -- --ignored
```

The asset check ensures the shipped page and stylesheet match `rust/ui/assets`.
Before the Electron source was removed, a differential test compared the Rust
compositor with the JavaScript one over 174 configurations and 712,704 pixels
(identical, including clipping and fractional opacity); the `clipview-core`
unit tests remain.

Native smoke runs use an isolated `CLIPVIEW_USER_DATA_DIR` and
`CLIPVIEW_RUST_SMOKE=<absolute-output-json>`. The UI reports a diagnostic snapshot
and exits. **Inspect `loaded`, `error`, dimensions and centering in that JSON;
an exit code of zero alone is not a passing image test.**

The Electron-versus-Rust memory comparison script was removed with the
Electron build; the measurements below are historical.

## Required Before Replacement

- [ ] All previously supported codecs and animation paths, including RAW,
  HEIC/HEIF, JXL, JXR, BPG, AVIF and PSD/PSB variants. Every listed extension
  now has a Rust path (see "Image formats added in Rust"). Remaining gaps: BPG
  4:2:2 (alpha/grayscale, CRW and X3F support ended), JPEG 2000 ICC/JPX/palette,
  PSD without merged image or 32-bit, and broader real-file coverage.
- [ ] Profiled and unprofiled CMYK/RGB behavior, orientation and alpha parity.
- [ ] CLIP/CSP preview, all saved raster/vector/text/brush/tone/effect paths,
  metadata, masks, clipping, folder visibility and original-state restoration
  across both private sample sets, without modifying or distributing sources.
  Done in Rust: raster, text render caches, paper, flat/gradient fills, tone,
  edge, raster masks, frame-folder panel masks, vector strokes, balloons and
  frame lines, clipping, independent display. Remaining: font text fallback,
  image-material transforms, thumbnail fallback, correction/3D.
- [ ] PSD layer compositing fidelity and performance across real PSD/PSB files,
  including styles, adjustment layers, vector masks and unsupported blend modes.
- [ ] CMC crop modes, page order, archive traversal and natural sorting.
- [ ] Lazy thumbnails, rapid navigation cancellation and prefetch behavior.
- [ ] Audio/video codec matrix, subtitle conversion, playback controls,
  shortcuts, cursor/control timeouts, menus and repeat modes.
- [ ] Existing settings/window state migration, file associations and Explorer
  thumbnail integration without changing Windows defaults unexpectedly.
- [ ] Clipboard/export after rotation, scaling, crop and layer changes.
- [ ] Update-signature/channel migration and rollback, portable and installer
  packaging, and complete Rust/native dependency license notices. Done: separate
  Rust update channel with checksum verification and portable rollback, NSIS and
  portable packages, generated notices. Remaining: release signing, migration of
  Electron installs to the Rust channel, and an end-to-end update between two
  published Rust releases.
- [ ] Same viewport/DPI/state visual comparison and keyboard accessibility.
- [ ] Repeated whole-process release-build memory measurements for ordinary
  images and layered documents, including peaks and navigation stress.

A later elevated local run sampled the complete process trees twice with the
small `test/sample.png`. Electron private bytes were approximately 265–270 MiB;
Rust private bytes were approximately 199–209 MiB. Working-set figures overlap
and include shared pages. The Electron baseline had a fixed startup delay rather
than a verified image-ready signal, so this is preliminary, not a release gate or
evidence for large/layered documents. That run also registered the test binary as
a Windows file handler and replaced the capabilities list. The 13 affected
executable/icon values and 111 missing capability mappings were restored using
the installed app path and matching existing OpenWith registrations.
The comparison script is disabled until a baseline with registration and updates
disabled is available. Raw results are local to `outputs/rust-memory`.

## Image-Open Benchmark

The local `image_open` benchmark measures native loading, not process startup,
first display, or cold-disk reads. With one warm-up and nine timed opens per
file, the supplied PSD median changed from 238.85 ms to 173.50 ms. Across CLIP
pages 0006–0019, the mean of per-page medians changed from 10.06 ms to 7.61 ms.
The 180 matched before/after records had identical output size, image or pixel
fingerprints, and metadata fingerprints. Private source files are not included.

The Rust application targets Windows directly and uses WebView2; it does not
launch WSL or require `vmmemWSL` to run. `is-wsl` in the dependency lockfile is
an operating-environment detection dependency, not a request to start WSL.

Unsupported Rust paths must return explicit errors rather than success-shaped
placeholders or silently falling back to the original JavaScript engine.

# Clip Image Viewer

Windows용 오픈소스 미디어 뷰어입니다. 일반 이미지, 최신 이미지 형식,
동영상, 카메라 RAW, PSD, CLIP STUDIO PAINT 문서와 CMC 만화 프로젝트를 한 앱에서 봅니다.

2.0부터 앱은 Rust(Tauri + WebAssembly UI)로 작성됩니다. 1.x Electron 버전의 코드는
Git 기록(v1.8.0 커밋까지)에 남아 있습니다.

> [!IMPORTANT]
> 2.0은 코드를 Electron(JavaScript)에서 Rust로 전환했기 때문에 프로그램 구성 파일이 완전히 다릅니다.
> **포터블 버전 사용자는 기존 포터블 폴더의 파일을 모두 삭제한 뒤 새 파일로 교체하는 것을 권장합니다.**
> 덮어쓰기만 하면 더 이상 쓰이지 않는 Electron 파일(`resources`, `locales`, `*.pak`, `*.dll` 등)이 남습니다.
> 창 크기·위치와 보기 설정은 포터블 폴더 밖(`%APPDATA%`, `%LOCALAPPDATA%`)에 저장되어 유지됩니다.

## 주요 기능

- 이미지를 상단바와 하단바 사이에 맞춰 중앙 표시, 마지막 창 크기와 위치 복원
- 이전·다음 이미지 사전 로딩과 빠른 이동 시 이전 요청 취소
- 확대/축소(포인터 기준), 이동, 회전, 반전, 전체 화면, 슬라이드쇼
- 설정 > 보기에서 마우스 휠을 확대·축소 또는 이전·다음 파일 이동으로 선택
- 동영상·오디오 재생, 같은 폴더의 SRT/VTT/ASS/SSA 자막 표시, 반복·무작위 재생
- 미리보기 목록, 이미지 정보, 클립보드 복사, PNG 사본 저장
- CMYK JPEG의 내장 ICC 프로파일을 적용한 sRGB 표시
- ZIP/CBZ 내부 이미지 순차 보기, CMC 페이지 순서와 재단선·재단 여백·전체 보기
- PSD 레이어 목록과 표시 ON/OFF, `Ctrl+클릭` 레이어 선택
- CLIP/CSP 레이어를 처음부터 분리해 합성하고, 레이어 이름·계층·혼합 모드·불투명도·클리핑·마스크 확인
- CLIP 래스터·텍스트 개체 렌더 캐시·벡터 선·말풍선·컷 테두리·단색/그레이디언트 채우기·톤 재구성
- GitHub Releases 기반 자동 업데이트(설치형·포터블), `SHA256SUMS.txt` 검증

## CLIP 레이어 기능 중요 안내

> [!WARNING]
> CLIP 레이어 기능은 CLIP STUDIO PAINT의 완전한 렌더러가 아닙니다.
> 앱은 CLIP 파일에 저장된 레이어 데이터를 처음부터 분리해 합성하므로, 레이어를 켜고 끈 결과가
> 서로 일관됩니다. 그러나 렌더 캐시가 없는 텍스트, 이미지 소재 변형, 보정·3D 레이어는 그리지 못하며
> 벡터 브러시·톤·효과는 근사이므로 CSP와 다를 수 있습니다. 그리지 못한 레이어 수는 레이어 정보에 표시됩니다.
> CSP 설치나 실행은 필요하지 않고 원본 CLIP 파일은 수정하지 않습니다. 최종 결과는 CSP에서 확인하세요.

## 지원 형식

- 기본: BMP, JPG/JPEG, GIF, PNG/APNG, WebP, ICO, SVG
- 편집/특수: PSD/PSB, DDS, JXR/HDP/WDP, J2K/JP2, TGA, TIFF,
  PCX, PGM/PNM/PPM/PBM/PAM, BPG(컬러), EXR, QOI
- 최신 형식: AVIF, JXL, HEIC/HEIF/HIF
- 애니메이션: GIF, WebP, BPG, PNG/APNG, AVIF, JXL
- RAW: DNG, CR2/CR3, NEF/NRW, ORF, RW2, PEF, SR2/SRF, ARW, RAF 등 LibRaw가 인식하는 카메라 RAW
- CLIP STUDIO PAINT: CLIP/CSP, CMC
- 압축 파일: ZIP, CBZ
- 동영상: MP4/M4V, MOV/QT, WebM, MKV, AVI, WMV/ASF, FLV/F4V,
  OGV/OGG/OGM, 3GP/3G2, MPG/MPEG/MPE, M1V/M2V, TS/MTS/M2TS,
  VOB, DivX/Xvid, MXF, DV, AMV, RM/RMVB
- 오디오: MP3, WAV/WAVE, FLAC, OGG/OGA, Opus, M4A/M4B, AAC, WebM Audio(WEBA)

RAW 파일은 카메라가 저장한 내장 미리보기를 먼저 사용하고, 없거나 작으면 LibRaw로 엽니다.
LibRaw는 앱 본체가 아니라 권한을 낮추고 메모리·시간을 제한한 별도 작업 프로세스에서만 실행됩니다.
동영상·오디오 코덱 지원 범위는 Microsoft Edge WebView2와 Windows 코덱에 따릅니다.
알파 채널·흑백 BPG, CRW, X3F는 2.0부터 지원하지 않습니다.

## 파일 연결

Windows 설치형은 기본적으로 JPG/JPEG, PNG, GIF, WebP, BMP만 연결하며, 설정에서 이미지·동영상·오디오
확장자를 각각 켜거나 끌 수 있습니다. 포터블 버전에서는 확장자 파일 연결을 지원하지 않습니다.
Windows 10/11 보안 정책상 기본 앱은 사용자 확인 없이 바꿀 수 없으므로, 설정에서 형식을 등록한 뒤
열리는 Windows 기본 앱 화면에서 한 번 선택해야 합니다.

## 자동 업데이트

앱 실행 후 GitHub Releases에서 Rust 버전용 새 파일을 확인하고 내려받아 `SHA256SUMS.txt`로 검증합니다.
설치형은 재시작할 때 새 설치 파일이 자동으로 실행되어 교체되고, 포터블은 재시작할 때 폴더의 파일을
교체한 뒤 다시 실행합니다. 포터블 교체가 실패하면 원래 파일로 되돌립니다.

## 배포 파일

- Windows 설치형: `Clip-Image-Viewer-Setup-버전-rust-x64.exe`
- Windows 포터블: `Clip-Image-Viewer-버전-rust-windows-x64-portable.zip`
- 체크섬: `SHA256SUMS.txt` (자동 업데이트에 필요)

오픈소스 라이선스 고지는 두 배포판 모두 `THIRD_PARTY_NOTICES.md`와 `licenses/` 폴더에 포함됩니다.

## 단축키

| 키 | 기능 |
|---|---|
| `Ctrl+O` / `Ctrl+Shift+O` | 파일 / 폴더 열기 |
| `←` / `→` | 이전 / 다음 |
| `+` / `-` | 확대 / 축소 |
| 마우스 휠 | 이미지의 마우스 포인터 위치를 중심으로 확대 / 축소 |
| `0` / `1` | 창 맞춤 / 100% |
| `R` / `Shift+R` | 오른쪽 / 왼쪽 회전 |
| `F` | 좌우 반전 |
| `I` / `T` | 정보 / 미리보기 목록 |
| `L` | 레이어 패널 |
| `Ctrl+클릭` | 이미지 위치의 레이어 선택 |
| `S` | 슬라이드쇼 |
| `Enter` / `F11` | 전체 화면 |
| `Ctrl+C` / `Ctrl+S` | 복사 / PNG 저장 |
| `Space` / `Ctrl+Space` | 동영상 재생 / 일시정지 |
| `Ctrl+←` / `Ctrl+→` | 동영상 5초 뒤로 / 앞으로 |
| `Ctrl+↑` / `Ctrl+↓` | 동영상 볼륨 올림 / 내림 |
| `Ctrl+M` | 동영상 음소거 |

## 개발

필요한 도구: Rust MSVC 툴체인과 `wasm32-unknown-unknown` 대상, Visual C++ Build Tools와 Windows SDK,
WebView2, Node.js(라이선스 고지 생성), `wasm-bindgen-cli` 0.2.130, 패키징용 `tauri-cli` 2.
설치 방법과 검증 범위는 [Rust 전환 문서](docs/RUST_MIGRATION.md)에 정리했습니다.

```powershell
./scripts/build-rust.ps1            # UI + 네이티브 릴리스 빌드
./scripts/build-rust.ps1 -Package   # 라이선스 고지, 설치형, 포터블 ZIP, SHA256SUMS.txt
cargo test --manifest-path rust/Cargo.toml -p clip-image-viewer-native -p clipview-core
node test/rust-assets.test.js
```

개인 CLIP·RAW 샘플을 쓰는 회귀 테스트는 `--ignored`로 실행하며, 샘플은 Git과 배포물에서 제외합니다.

## 제한

- 레이어 목록과 속성은 래스터화 전의 원본 구조를 사용하며 원본 파일은 수정하지 않습니다.
- PSD 레이어 합성은 일반 혼합 모드·불투명도·래스터 마스크를 반영하지만, 일부 레이어 효과와
  특수 혼합 모드는 원본과 다를 수 있습니다. 합성 이미지가 저장된 PSD/PSB가 가장 정확합니다.
- RAW는 현상 대신 카메라 내장 미리보기를 우선 표시합니다.
- ASS/SSA 자막은 기본 텍스트만 표시하며 고급 스타일 효과는 반영하지 않습니다.

## 라이선스

Clip Image Viewer는 [MIT License](LICENSE)로 배포되는 오픈소스 프로젝트입니다.
포함된 서드파티 구성 요소의 라이선스는 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)를 참고하세요.

Copyright (c) 2026 verylovefurry

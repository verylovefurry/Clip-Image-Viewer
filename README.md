# Clip Image Viewer

Windows용 오픈소스 미디어 뷰어입니다. 일반 이미지, 최신 이미지 형식,
동영상, 카메라 RAW, PSD, CLIP STUDIO PAINT 문서와 CMC 만화 프로젝트를 한 앱에서 봅니다.

## 주요 기능

- 이미지를 상단바와 하단바 사이에 맞춰 중앙 표시
- 최초 실행 3:4 창, 이후 마지막 창 크기와 위치 복원
- 이전·다음 이미지 사전 로딩
- GitHub Releases 기반 자동 업데이트와 재시작 설치
- 확대/축소, 이동, 회전, 반전, 전체 화면, 슬라이드쇼
- 커스텀 동영상 플레이어 UI, 같은 폴더의 SRT/VTT/ASS/SSA 자막 표시
- 미리보기 목록, 이미지 정보, 클립보드 복사, PNG 사본 저장
- ZIP/CBZ 내부 이미지 순차 보기
- CLIP/CSP 내장 캔버스 미리보기
- CMC 페이지 순서와 재단선·재단 여백·전체 보기
- PSD 레이어별 비파괴 래스터 썸네일과 목록, 표시 ON/OFF, 혼합 모드·불투명도·클리핑·마스크·효과 확인
- `Ctrl+클릭`으로 PSD 이미지의 해당 픽셀을 포함하는 레이어 선택
- CLIP/CSP 래스터·벡터·텍스트·효과 레이어의 비파괴 래스터 미리보기와 빠른 표시 ON/OFF
- CLIP/CSP 3D·톤·조정 필터·CSP 전용 레이어의 원본 합성 문맥 미리보기
- CLIP/CSP 레이어 이름·계층·가시성·혼합 모드·불투명도·클리핑·마스크 정보 확인
- 하위 트리 사전 합성과 캐시를 이용한 빠른 폴더 ON/OFF
- 대용량 이미지 디코딩을 별도 워커에서 처리하고 비정상 파일 크기 제한

## CLIP 레이어 기능 중요 안내

> [!WARNING]
> CLIP 레이어 기능은 CLIP STUDIO PAINT의 완전한 렌더러가 아닙니다.
> 레이어를 조작하기 전에는 CLIP 파일에 저장된 CSP 캔버스 미리보기를 표시하지만,
> 레이어 ON/OFF 후 변경된 영역은 앱이 CLIP 내부 데이터를 해석해 근사 합성합니다.
> 독립 래스터나 완전한 렌더 매개변수가 저장되지 않은 텍스트·벡터·톤·테두리·3D·필터·효과
> 레이어는 폰트, 자간, 정렬, 필압, 선·테두리 두께, 톤, 혼합 및 마스크 결과가 CSP와
> 다를 수 있습니다. 변경과 무관한 영역은 CSP 원본 미리보기 픽셀을 유지하며 원본 CLIP
> 파일은 수정하지 않습니다. 최종 출력과 정확한 편집 결과는 반드시 CSP에서 확인해야 합니다.

## 지원 형식

- 기본: BMP, JPG/JPEG, GIF, PNG/APNG, WebP, ICO, SVG
- 편집/특수: PSD/PSB, DDS, JXR/HDP/WDP, J2K/JP2, TGA, TIFF,
  PCX, PGM/PNM/PPM/PBM/PAM, BPG, EXR, QOI
- 최신 형식: AVIF, JXL, HEIC/HEIF/HIF
- 애니메이션: GIF, WebP, BPG, PNG/APNG, AVIF, JXL
- RAW: DNG, CR2/CR3/CRW, NEF/NRW, ORF, RW2, PEF, SR2/SRF,
  ARW, RAF 및 LibRaw가 인식하는 주요 카메라 RAW
- CLIP STUDIO PAINT: CLIP/CSP, CMC
- 압축 파일: ZIP, CBZ
- 동영상: MP4/M4V, MOV/QT, WebM, MKV, AVI, WMV/ASF, FLV/F4V,
  OGV/OGG/OGM, 3GP/3G2, MPG/MPEG/MPE, M1V/M2V, TS/MTS/M2TS,
  VOB, DivX/Xvid, MXF, DV, AMV, RM/RMVB

새 카메라 RAW 지원 범위는 함께 제공되는 LibRaw 버전에 따릅니다.
동영상 코덱 지원 범위는 Electron/Chromium 및 Windows 코덱 지원 범위에 따릅니다.

## 파일 연결

Windows 설치형은 기본적으로 JPG/JPEG, PNG, GIF, WebP, BMP만 연결합니다.
설정에서 이미지 확장자와 동영상 확장자를 구분해 나머지 확장자를 각각 켜거나 끌 수 있습니다.

포터블 ZIP에서는 확장자 파일 연결을 지원하지 않습니다.

Windows 10/11 보안 정책상 앱이 기본 앱을 사용자 확인 없이 강제로 변경할
수는 없습니다. 설정에서 형식을 등록한 뒤 열리는 Clip Image Viewer 전용
Windows 기본 앱 화면에서 원하는 형식을 한 번 선택해야 합니다.

## 자동 업데이트

앱 실행 후 GitHub Releases에서 새 버전을 자동으로 확인하고 다운로드합니다.
설치형은 재시작 시 새 버전으로 교체되며, 포터블판은 ZIP을 받은 뒤 기존
폴더를 교체하고 다시 실행합니다.

포터블 업데이트는 릴리스의 `SHA256SUMS.txt`와 ZIP을 대조한 뒤 적용합니다.
업데이트 배포에는 설치 파일, 블록맵, `latest.yml`, 포터블 ZIP,
`SHA256SUMS.txt`가 모두 필요합니다.

## 배포 파일

- Windows 설치형: `Clip-Image-Viewer-Setup-버전-x64.exe`
- Windows 포터블: `Clip-Image-Viewer-버전-windows-x64-portable.zip`

## 단축키

| 키 | 기능 |
|---|---|
| `Ctrl+O` / `Ctrl+Shift+O` | 파일 / 폴더 열기 |
| `←` / `→` | 이전 / 다음 |
| `+` / `-` | 확대 / 축소 |
| `0` / `1` | 창 맞춤 / 100% |
| `R` / `Shift+R` | 오른쪽 / 왼쪽 회전 |
| `F` | 좌우 반전 |
| `I` / `T` | 정보 / 미리보기 목록 |
| `L` | 레이어 패널 |
| `Ctrl+클릭` | PSD 이미지 위치의 레이어 선택 |
| `S` | 슬라이드쇼 |
| `Enter` / `F11` | 전체 화면 |
| `Ctrl+C` / `Ctrl+S` | 복사 / PNG 저장 |
| `Ctrl+Space` | 동영상 재생 / 일시정지 |
| `Ctrl+←` / `Ctrl+→` | 동영상 5초 뒤로 / 앞으로 |
| `Ctrl+↑` / `Ctrl+↓` | 동영상 볼륨 올림 / 내림 |
| `Ctrl+M` | 동영상 음소거 |

## 개발

Node.js 22 이상을 권장합니다.

```bash
npm install
npm test
npm start
```

```bash
npm run build
```

위 명령은 설치형, `latest.yml`, 블록맵, 포터블 ZIP을 함께 만듭니다.

## 제한

- 레이어 목록과 속성은 래스터화 전의 원본 구조를 사용하며 원본 파일은 수정하지 않습니다.
- CLIP/CSP 레이어 조작 전에는 파일에 저장된 CSP 캔버스 미리보기를 그대로 사용합니다.
  레이어 ON/OFF 후에는 변경 영역만 비파괴 근사 합성하고 나머지 원본 픽셀을 보존합니다.
- CLIP/CSP가 저장한 독립 레이어 래스터가 있으면 이를 우선 사용합니다. 독립 래스터가
  없는 벡터·텍스트·톤·테두리·3D·효과는 CELSYS 렌더러와 완전히 동일하게 재현할 수 없습니다.
- CLIP/CSP의 저장된 래스터 mipmap, 벡터 스트로크, 용지, 기본 텍스트,
  단색·그레이디언트 채우기는 지원 데이터 범위에서 ON/OFF 재합성합니다.
- CLIP/CSP 전용 레이어와 원래 숨겨진 레이어를 새로 표시할 때는 근사 결과의 오차가 더
  커질 수 있으며, CLIP 이미지 위치 기반 레이어 선택은 지원하지 않습니다.
- PSD 레이어 ON/OFF 합성은 일반 혼합 모드·불투명도·래스터 마스크를 반영합니다.
  일부 고급 레이어 효과, 특수 혼합 모드, 클리핑 합성은 원본과 다를 수 있습니다.
- PSD/PSB는 저장된 합성 이미지가 있는 문서에서 가장 정확합니다.
- 일부 RAW는 전체 현상 이미지 대신 카메라가 저장한 고해상도 미리보기를 표시합니다.
- ASS/SSA 자막은 기본 텍스트만 표시하며 고급 스타일 효과는 반영하지 않습니다.

## 라이선스

Clip Image Viewer는 [MIT License](LICENSE)로 배포되는 오픈소스 프로젝트입니다.

Copyright (c) 2026 verylovefurry

!include "nsDialogs.nsh"
!include "LogicLib.nsh"
!include "MUI2.nsh"

!ifndef BUILD_UNINSTALLER
  Var AssocBasicCheckbox
  Var AssocBasicEnabled

  !macro customWelcomePage
    !insertmacro MUI_PAGE_WELCOME
    Page custom AssociationPageCreate AssociationPageLeave
  !macroend

Function AssociationPageCreate
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  !insertmacro MUI_HEADER_TEXT "파일 연결 선택" "기본 이미지 형식 연결 여부를 선택하세요."

  ${NSD_CreateLabel} 0 0 100% 38u "설치 후 Clip Image Viewer로 열 기본 이미지 형식을 선택합니다."
  Pop $0

  ${NSD_CreateCheckbox} 0 48u 100% 16u "JPG, PNG, GIF, WebP, BMP 연결"
  Pop $AssocBasicCheckbox
  ${NSD_Check} $AssocBasicCheckbox

  ${NSD_CreateLabel} 0 82u 100% 50u "다른 지원 형식은 설치 후 앱 설정에서 확장자별로 켜고 끌 수 있습니다."
  Pop $0

  ${NSD_CreateLabel} 0 148u 100% 48u "Windows 10/11 정책에 따라 기본 앱 설정 화면에서 한 번 더 확인해야 할 수 있습니다."
  Pop $0

  nsDialogs::Show
FunctionEnd

Function AssociationPageLeave
  ${NSD_GetState} $AssocBasicCheckbox $AssocBasicEnabled
FunctionEnd
!endif

!macro RegisterClipImageViewerExtension EXT
  WriteRegStr HKCU "Software\Classes\ClipImageViewer${EXT}" "" "Clip Image Viewer 이미지"
  WriteRegStr HKCU "Software\Classes\ClipImageViewer${EXT}\DefaultIcon" "" "$INSTDIR\ClipImageViewer.exe,0"
  WriteRegStr HKCU "Software\Classes\ClipImageViewer${EXT}\shell\open\command" "" '"$INSTDIR\ClipImageViewer.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\${EXT}\OpenWithProgids" "ClipImageViewer${EXT}" ""
  WriteRegStr HKCU "Software\ClipImageViewer\Capabilities\FileAssociations" "${EXT}" "ClipImageViewer${EXT}"
!macroend

!macro RegisterClipImageViewerApplication
  WriteRegStr HKCU "Software\RegisteredApplications" "Clip Image Viewer" "Software\ClipImageViewer\Capabilities"
  WriteRegStr HKCU "Software\ClipImageViewer\Capabilities" "ApplicationName" "Clip Image Viewer"
  WriteRegStr HKCU "Software\ClipImageViewer\Capabilities" "ApplicationDescription" "다양한 이미지와 CLIP STUDIO PAINT 문서를 보는 이미지 뷰어"
  WriteRegStr HKCU "Software\ClipImageViewer\Capabilities" "ApplicationIcon" "$INSTDIR\ClipImageViewer.exe,0"
!macroend

!macro UnregisterClipImageViewerExtension EXT
  ReadRegStr $0 HKCU "Software\Classes\${EXT}" ""
  ${If} $0 == "ClipImageViewer${EXT}"
  ${OrIf} $0 == "ClipView${EXT}"
    DeleteRegValue HKCU "Software\Classes\${EXT}" ""
  ${EndIf}
  DeleteRegValue HKCU "Software\Classes\${EXT}\OpenWithProgids" "ClipImageViewer${EXT}"
  DeleteRegValue HKCU "Software\Classes\${EXT}\OpenWithProgids" "ClipView${EXT}"
  DeleteRegKey HKCU "Software\Classes\ClipImageViewer${EXT}"
  DeleteRegKey HKCU "Software\Classes\ClipView${EXT}"
!macroend

!macro RegisterBasicExtensions
  !insertmacro RegisterClipImageViewerApplication
  !insertmacro RegisterClipImageViewerExtension ".jpg"
  !insertmacro RegisterClipImageViewerExtension ".jpeg"
  !insertmacro RegisterClipImageViewerExtension ".png"
  !insertmacro RegisterClipImageViewerExtension ".gif"
  !insertmacro RegisterClipImageViewerExtension ".webp"
  !insertmacro RegisterClipImageViewerExtension ".bmp"
!macroend

!macro UnregisterAllExtensions
  !insertmacro UnregisterClipImageViewerExtension ".3fr"
  !insertmacro UnregisterClipImageViewerExtension ".apng"
  !insertmacro UnregisterClipImageViewerExtension ".ari"
  !insertmacro UnregisterClipImageViewerExtension ".arw"
  !insertmacro UnregisterClipImageViewerExtension ".avif"
  !insertmacro UnregisterClipImageViewerExtension ".bay"
  !insertmacro UnregisterClipImageViewerExtension ".bmp"
  !insertmacro UnregisterClipImageViewerExtension ".bpg"
  !insertmacro UnregisterClipImageViewerExtension ".cap"
  !insertmacro UnregisterClipImageViewerExtension ".cbz"
  !insertmacro UnregisterClipImageViewerExtension ".clip"
  !insertmacro UnregisterClipImageViewerExtension ".cmc"
  !insertmacro UnregisterClipImageViewerExtension ".cr2"
  !insertmacro UnregisterClipImageViewerExtension ".cr3"
  !insertmacro UnregisterClipImageViewerExtension ".crw"
  !insertmacro UnregisterClipImageViewerExtension ".csp"
  !insertmacro UnregisterClipImageViewerExtension ".dcr"
  !insertmacro UnregisterClipImageViewerExtension ".dcs"
  !insertmacro UnregisterClipImageViewerExtension ".dds"
  !insertmacro UnregisterClipImageViewerExtension ".dng"
  !insertmacro UnregisterClipImageViewerExtension ".drf"
  !insertmacro UnregisterClipImageViewerExtension ".eip"
  !insertmacro UnregisterClipImageViewerExtension ".erf"
  !insertmacro UnregisterClipImageViewerExtension ".exr"
  !insertmacro UnregisterClipImageViewerExtension ".fff"
  !insertmacro UnregisterClipImageViewerExtension ".gif"
  !insertmacro UnregisterClipImageViewerExtension ".gpr"
  !insertmacro UnregisterClipImageViewerExtension ".hdp"
  !insertmacro UnregisterClipImageViewerExtension ".heic"
  !insertmacro UnregisterClipImageViewerExtension ".heif"
  !insertmacro UnregisterClipImageViewerExtension ".hif"
  !insertmacro UnregisterClipImageViewerExtension ".ico"
  !insertmacro UnregisterClipImageViewerExtension ".iiq"
  !insertmacro UnregisterClipImageViewerExtension ".j2c"
  !insertmacro UnregisterClipImageViewerExtension ".j2k"
  !insertmacro UnregisterClipImageViewerExtension ".jfif"
  !insertmacro UnregisterClipImageViewerExtension ".jp2"
  !insertmacro UnregisterClipImageViewerExtension ".jpc"
  !insertmacro UnregisterClipImageViewerExtension ".jpf"
  !insertmacro UnregisterClipImageViewerExtension ".jpg"
  !insertmacro UnregisterClipImageViewerExtension ".jpeg"
  !insertmacro UnregisterClipImageViewerExtension ".jpx"
  !insertmacro UnregisterClipImageViewerExtension ".jxl"
  !insertmacro UnregisterClipImageViewerExtension ".jxr"
  !insertmacro UnregisterClipImageViewerExtension ".k25"
  !insertmacro UnregisterClipImageViewerExtension ".kdc"
  !insertmacro UnregisterClipImageViewerExtension ".mdc"
  !insertmacro UnregisterClipImageViewerExtension ".mef"
  !insertmacro UnregisterClipImageViewerExtension ".mos"
  !insertmacro UnregisterClipImageViewerExtension ".mrw"
  !insertmacro UnregisterClipImageViewerExtension ".nef"
  !insertmacro UnregisterClipImageViewerExtension ".nrw"
  !insertmacro UnregisterClipImageViewerExtension ".obm"
  !insertmacro UnregisterClipImageViewerExtension ".orf"
  !insertmacro UnregisterClipImageViewerExtension ".pam"
  !insertmacro UnregisterClipImageViewerExtension ".pbm"
  !insertmacro UnregisterClipImageViewerExtension ".pcx"
  !insertmacro UnregisterClipImageViewerExtension ".pef"
  !insertmacro UnregisterClipImageViewerExtension ".pgm"
  !insertmacro UnregisterClipImageViewerExtension ".png"
  !insertmacro UnregisterClipImageViewerExtension ".pnm"
  !insertmacro UnregisterClipImageViewerExtension ".ppm"
  !insertmacro UnregisterClipImageViewerExtension ".psb"
  !insertmacro UnregisterClipImageViewerExtension ".psd"
  !insertmacro UnregisterClipImageViewerExtension ".ptx"
  !insertmacro UnregisterClipImageViewerExtension ".pxn"
  !insertmacro UnregisterClipImageViewerExtension ".qoi"
  !insertmacro UnregisterClipImageViewerExtension ".r3d"
  !insertmacro UnregisterClipImageViewerExtension ".raf"
  !insertmacro UnregisterClipImageViewerExtension ".raw"
  !insertmacro UnregisterClipImageViewerExtension ".rw2"
  !insertmacro UnregisterClipImageViewerExtension ".rwl"
  !insertmacro UnregisterClipImageViewerExtension ".rwz"
  !insertmacro UnregisterClipImageViewerExtension ".sr2"
  !insertmacro UnregisterClipImageViewerExtension ".srf"
  !insertmacro UnregisterClipImageViewerExtension ".srw"
  !insertmacro UnregisterClipImageViewerExtension ".sti"
  !insertmacro UnregisterClipImageViewerExtension ".svg"
  !insertmacro UnregisterClipImageViewerExtension ".tga"
  !insertmacro UnregisterClipImageViewerExtension ".tif"
  !insertmacro UnregisterClipImageViewerExtension ".tiff"
  !insertmacro UnregisterClipImageViewerExtension ".wdp"
  !insertmacro UnregisterClipImageViewerExtension ".webp"
  !insertmacro UnregisterClipImageViewerExtension ".x3f"
  !insertmacro UnregisterClipImageViewerExtension ".zip"
  DeleteRegValue HKCU "Software\RegisteredApplications" "Clip Image Viewer"
  DeleteRegKey HKCU "Software\ClipImageViewer\Capabilities"
!macroend

!macro customInstall
  ${If} ${isUpdated}
    ClearErrors
    ReadRegDWORD $0 HKCU "Software\ClipImageViewer\Settings" "AssociationsEnabled"
    ${If} ${Errors}
      !insertmacro RegisterBasicExtensions
    ${ElseIf} $0 == 1
      !insertmacro RegisterBasicExtensions
    ${EndIf}
  ${Else}
    ${If} $AssocBasicEnabled == ${BST_CHECKED}
      !insertmacro RegisterBasicExtensions
      WriteRegDWORD HKCU "Software\ClipImageViewer\Settings" "AssociationsEnabled" 1
    ${Else}
      WriteRegDWORD HKCU "Software\ClipImageViewer\Settings" "AssociationsEnabled" 0
    ${EndIf}
  ${EndIf}
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro customUnInstall
  ${IfNot} ${isUpdated}
    !insertmacro UnregisterAllExtensions
    DeleteRegKey HKCU "Software\ClipImageViewer\Settings"
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  ${EndIf}
!macroend

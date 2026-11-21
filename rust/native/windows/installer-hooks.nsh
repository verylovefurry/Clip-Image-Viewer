; Hooks for the Tauri NSIS installer (bundle.windows.nsis.installerHooks).
;
; 1.x was an Electron app installed per user by electron-builder. Its update check
; (electron-updater) downloads this installer through latest.yml and runs it with
; "--updated /S --force-run". Before installing, remove the Electron copy so users do not
; end up with two apps, stale shortcuts and associations pointing at the old executable.
; Its uninstaller keeps the user data (deleteAppDataOnUninstall was false), so the Rust app
; still imports the saved window state.

; electron-builder derives this key from appId "com.clipimageviewer.app"; it is the same on
; every machine.
!define CLIPVIEW_ELECTRON_KEY "Software\234213fa-0577-5840-9762-41a24312129a"
!define CLIPVIEW_ELECTRON_UNINSTALLER "Uninstall ClipImageViewer.exe"

!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $R0 HKCU "${CLIPVIEW_ELECTRON_KEY}" "InstallLocation"
  ${If} $R0 != ""
  ${AndIf} ${FileExists} "$R0\${CLIPVIEW_ELECTRON_UNINSTALLER}"
    DetailPrint "Removing Clip Image Viewer 1.x (Electron) from $R0"
    ; "_?=" runs the uninstaller in place, so ExecWait returns only when it has finished.
    ExecWait '"$R0\${CLIPVIEW_ELECTRON_UNINSTALLER}" /currentuser /S _?=$R0' $R1
    DetailPrint "Electron uninstaller exit code: $R1"
    Delete "$R0\${CLIPVIEW_ELECTRON_UNINSTALLER}"
    RMDir "$R0"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; electron-updater asks for a restart with --force-run; Tauri's own flag is /R.
  ClearErrors
  ${GetOptions} $CMDLINE "--force-run" $R0
  ${IfNot} ${Errors}
    Exec '"$INSTDIR\${MAINBINARYNAME}.exe"'
  ${EndIf}
!macroend

; Minimal per-user NSIS fixture mirroring Tauri v2 layout for Agent Lounge OS.
; Main binary name = Cargo package name (agent-lounge-os.exe).
; Product / Start Menu / HKCU Uninstall = "Agent Lounge OS".
; Silent: setup.exe /S /D=C:\path

Name "Agent Lounge OS"
OutFile "Agent-Lounge-OS-windows-x64-setup.exe"
Unicode true
RequestExecutionLevel user
SilentInstall silent

; /D= overrides this default.
InstallDir "$LOCALAPPDATA\AgentLounge\app"

!define PRODUCTNAME "Agent Lounge OS"
!define MAINBINARYNAME "agent-lounge-os"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"

Section "Install"
  SetOutPath "$INSTDIR"
  File "${MAINBINARYNAME}.exe"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\${MAINBINARYNAME}.exe"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "Agent Lounge"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\${PRODUCTNAME}"
  CreateShortCut "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${PRODUCTNAME}\${PRODUCTNAME}.lnk"
  RMDir "$SMPROGRAMS\${PRODUCTNAME}"

  DeleteRegKey HKCU "${UNINSTKEY}"
SectionEnd

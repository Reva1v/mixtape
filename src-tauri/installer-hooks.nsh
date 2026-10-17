; Mixtape additions to Tauri's NSIS installer (bundle.windows.nsis.installerHooks).
; Included at the top of the script, before the pages are declared.

; The finish page's "Create desktop shortcut" box starts unchecked.
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

; Silent and passive installs (updates) create the desktop shortcut on their own:
; keep it only if the user had one before.
Var HadDesktopShortcut

!macro NSIS_HOOK_PREINSTALL
  StrCpy $HadDesktopShortcut 0
  ${If} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
    StrCpy $HadDesktopShortcut 1
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${If} $HadDesktopShortcut = 0
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}
!macroend

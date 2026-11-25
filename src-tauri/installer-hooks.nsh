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

  ; mixtape:// links (open from the browser extension, Droplet Island). Per user, no admin needed.
  ; Rewritten on every install, so an update (which runs the old uninstaller first) keeps it.
  WriteRegStr HKCU "Software\Classes\mixtape" "" "URL:Mixtape"
  WriteRegStr HKCU "Software\Classes\mixtape" "URL Protocol" ""
  WriteRegStr HKCU "Software\Classes\mixtape\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
  WriteRegStr HKCU "Software\Classes\mixtape\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; An update runs the old uninstaller with /UPDATE and installs again right after: keep the key then.
  ${If} $UpdateMode <> 1
    DeleteRegKey HKCU "Software\Classes\mixtape"
  ${EndIf}
!macroend

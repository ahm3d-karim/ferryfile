; Ferryfile's additions to Tauri's NSIS installer (`bundle.windows.nsis.installerHooks`):
;  - the "Convert with Ferryfile" right-click entry and the Send to shortcut,
;    both per-user (HKCU and the user's SendTo folder). Keep in sync with
;    src-tauri/src/explorer.rs. Turning one off in Settings writes
;    NoContextMenu / NoSendTo = 1 under HKCU\Software\Ferryfile, so
;    reinstalling or updating doesn't bring it back.
;  - the install folder on the user's PATH, so `ferryfile convert …` works in
;    any terminal. The PATH edit runs in PowerShell rather than NSIS string
;    handling, which truncates long values. It reads the raw registry value, so
;    entries such as %USERPROFILE%\bin stay unexpanded, and adding twice
;    doesn't duplicate.

!macro FERRYFILE_ADD_VERB KEY
  WriteRegStr HKCU "${KEY}" "" "Convert with Ferryfile"
  WriteRegStr HKCU "${KEY}" "Icon" '"$INSTDIR\${MAINBINARYNAME}.exe",0'
  WriteRegStr HKCU "${KEY}" "MultiSelectModel" "Player"
  WriteRegStr HKCU "${KEY}\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" --open "%1"'
!macroend

!macro FERRYFILE_EDIT_PATH ADD
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d='$INSTDIR'; $$k=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $$o=[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames; $$parts=@(([string]$$k.GetValue('Path','',$$o)).Split(';') | Where-Object { $$_ -and $$_.TrimEnd('\') -ne $$d.TrimEnd('\') }); if (${ADD}) { $$parts+=$$d }; if ($$parts.Count -eq 0) { $$k.DeleteValue('Path',$$false) } else { $$k.SetValue('Path',($$parts -join ';'),'ExpandString') }"`
  Pop $0
  ; Tell open Explorer windows (and new terminals) that PATH changed.
  SendMessage 0xFFFF 0x1A 0 "STR:Environment" /TIMEOUT=2000
!macroend

!macro NSIS_HOOK_POSTINSTALL
  SetShellVarContext current
  ClearErrors
  ReadRegDWORD $0 HKCU "Software\Ferryfile" "NoContextMenu"
  ${If} $0 <> 1
    !insertmacro FERRYFILE_ADD_VERB "Software\Classes\*\shell\Ferryfile"
    !insertmacro FERRYFILE_ADD_VERB "Software\Classes\Directory\shell\Ferryfile"
  ${EndIf}
  ClearErrors
  ReadRegDWORD $0 HKCU "Software\Ferryfile" "NoSendTo"
  ${If} $0 <> 1
    CreateShortCut "$APPDATA\Microsoft\Windows\SendTo\Ferryfile.lnk" "$INSTDIR\${MAINBINARYNAME}.exe" "--open" "$INSTDIR\${MAINBINARYNAME}.exe" 0 SW_SHOWNORMAL "" "Convert with Ferryfile"
  ${EndIf}
  ClearErrors
  !insertmacro FERRYFILE_EDIT_PATH "$$true"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  SetShellVarContext current
  DeleteRegKey HKCU "Software\Classes\*\shell\Ferryfile"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\Ferryfile"
  Delete "$APPDATA\Microsoft\Windows\SendTo\Ferryfile.lnk"
  ; An update reinstalls right after; keep the user's on/off choices for it.
  ${If} $UpdateMode <> 1
    DeleteRegKey HKCU "Software\Ferryfile"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro FERRYFILE_EDIT_PATH "$$false"
!macroend

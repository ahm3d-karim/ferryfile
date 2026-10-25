; Ferryfile's additions to Tauri's NSIS installer: the "Convert with Ferryfile"
; right-click entry and the Send to shortcut. Both are per-user (HKCU and the
; user's SendTo folder). Keep in sync with src-tauri/src/explorer.rs.
;
; Turning one off in Settings writes NoContextMenu / NoSendTo = 1 under
; HKCU\Software\Ferryfile, so reinstalling or updating doesn't bring it back.

!macro FERRYFILE_ADD_VERB KEY
  WriteRegStr HKCU "${KEY}" "" "Convert with Ferryfile"
  WriteRegStr HKCU "${KEY}" "Icon" '"$INSTDIR\${MAINBINARYNAME}.exe",0'
  WriteRegStr HKCU "${KEY}" "MultiSelectModel" "Player"
  WriteRegStr HKCU "${KEY}\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" --open "%1"'
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

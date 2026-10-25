; NSIS installer hooks (Tauri's `bundle.windows.nsis.installerHooks`).
; Put the install folder on the user's PATH so `ferryfile convert …` works in
; any terminal, and take it off again on uninstall.
;
; The PATH edit runs in PowerShell rather than NSIS string handling, which
; truncates long values. It reads the raw registry value, so entries such as
; %USERPROFILE%\bin stay unexpanded, and adding twice doesn't duplicate.

!macro FERRYFILE_EDIT_PATH ADD
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d='$INSTDIR'; $$k=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $$o=[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames; $$parts=@(([string]$$k.GetValue('Path','',$$o)).Split(';') | Where-Object { $$_ -and $$_.TrimEnd('\') -ne $$d.TrimEnd('\') }); if (${ADD}) { $$parts+=$$d }; if ($$parts.Count -eq 0) { $$k.DeleteValue('Path',$$false) } else { $$k.SetValue('Path',($$parts -join ';'),'ExpandString') }"`
  Pop $0
  ; Tell open Explorer windows (and new terminals) that PATH changed.
  SendMessage 0xFFFF 0x1A 0 "STR:Environment" /TIMEOUT=2000
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro FERRYFILE_EDIT_PATH "$$true"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro FERRYFILE_EDIT_PATH "$$false"
!macroend

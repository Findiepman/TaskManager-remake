; Installer hooks. $UpdateMode is 1 when Tauri's installer runs an update (/UPDATE); the old
; uninstaller then runs with /UPDATE too.

!define TM_IFEO "SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe"
!define TM_KEY "SOFTWARE\TaskManagerRemake"

!macro NSIS_HOOK_POSTINSTALL
  SetRegView 64
  ; Upgrading by "uninstall, then install the new version": the uninstaller left a note with the
  ; clock time (FILETIME, 100 ns units) at which it removed our Task Manager redirect. If that was
  ; less than 15 minutes ago, put the redirect back. Older notes (a real uninstall) are ignored.
  ReadRegStr $0 HKLM "${TM_KEY}" "RestoreRedirectAt"
  ${If} $0 != ""
    System::Call 'kernel32::GetSystemTimeAsFileTime(*l .r2)'
    System::Int64Op $2 - $0
    Pop $3
    System::Int64Op $3 < 9000000000
    Pop $4
    System::Int64Op $3 < 0
    Pop $5
    ReadRegStr $1 HKLM "${TM_IFEO}" "Debugger"
    ${If} $4 = 1
    ${AndIf} $5 = 0
    ${AndIf} $1 == ""
      WriteRegStr HKLM "${TM_IFEO}" "Debugger" '"$INSTDIR\${MAINBINARYNAME}.exe"'
    ${EndIf}
  ${EndIf}
  DeleteRegKey HKLM "${TM_KEY}"

  ; Offer the official PawnIO driver installer (its own window) so the AMD CPU temperature works
  ; right away. Skipped on non-AMD CPUs, when PawnIO is already there, and for silent installs
  ; and updates (the app's Settings page offers it too).
  ReadRegStr $0 HKLM "HARDWARE\DESCRIPTION\System\CentralProcessor\0" "VendorIdentifier"
  ReadRegStr $1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO" "DisplayVersion"
  SetRegView lastused
  ${IfNot} ${Silent}
  ${AndIf} $UpdateMode <> 1
  ${AndIf} $0 == "AuthenticAMD"
  ${AndIf} $1 == ""
  ${AndIf} ${FileExists} "$INSTDIR\PawnIO_setup.exe"
    ExecWait '"$INSTDIR\PawnIO_setup.exe" -install'
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop the per-app network trace in case the app was force-closed and could not stop it.
  nsExec::Exec 'logman stop "TaskManagerRemake-Net" -ets'
  Pop $0

  ; Turn the Task Manager redirect off on a real uninstall (not an update), otherwise
  ; Ctrl+Shift+Esc would point at a deleted exe and no task manager could open. Only remove it
  ; when it points at this install, never another program's redirect.
  ${If} $UpdateMode <> 1
    SetRegView 64
    ReadRegStr $0 HKLM "${TM_IFEO}" "Debugger"
    ${If} $0 == '"$INSTDIR\${MAINBINARYNAME}.exe"'
    ${OrIf} $0 == "$INSTDIR\${MAINBINARYNAME}.exe"
      DeleteRegValue HKLM "${TM_IFEO}" "Debugger"
      ; Note the clock time, so installing a newer version right after can restore it.
      System::Call 'kernel32::GetSystemTimeAsFileTime(*l .r1)'
      WriteRegStr HKLM "${TM_KEY}" "RestoreRedirectAt" $1
    ${EndIf}
    SetRegView lastused
  ${EndIf}
!macroend

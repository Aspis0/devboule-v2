; Devboule's NSIS hooks (bundle > windows > nsis > installerHooks).
;
; Both pre hooks stop the app first and the daemon second. The template's own
; app check runs after these hooks, but a live GUI restarts a killed daemon at
; once (src-tauri/src/client/mod.rs:1647-1668), so the check that removes the
; parent has to come first; the template's check then finds no process. The
; prompt text is a literal because the installer ships one language (no
; `languages` under bundle.windows.nsis).

!macro DevbouleStopDaemon
  !define UniqueID ${__LINE__}

  StrCpy $R1 "Devboule's background daemon is still running. Stop it before continuing."
  StrCpy $R2 "Devboule's background daemon is running!$\nStopping it stops every running agent and terminal.$\nClick OK to kill it"
  StrCpy $R3 "Failed to kill Devboule's background daemon. Close Devboule from the notification area, then try again."

  !if "${INSTALLMODE}" == "currentUser"
    nsis_tauri_utils::FindProcessCurrentUser "devboule-daemon.exe"
  !else
    nsis_tauri_utils::FindProcess "devboule-daemon.exe"
  !endif
  Pop $R0
  ${If} $R0 = 0
      IfSilent devboule_daemon_kill_${UniqueID} 0
      ${IfThen} $PassiveMode != 1 ${|} MessageBox MB_OKCANCEL $R2 IDOK devboule_daemon_kill_${UniqueID} IDCANCEL devboule_daemon_cancel_${UniqueID} ${|}
      devboule_daemon_kill_${UniqueID}:
        !if "${INSTALLMODE}" == "currentUser"
          nsis_tauri_utils::KillProcessCurrentUser "devboule-daemon.exe"
        !else
          nsis_tauri_utils::KillProcess "devboule-daemon.exe"
        !endif
        Pop $R0
        Sleep 500
        ${If} $R0 = 0
        ${OrIf} $R0 = 2
          Goto devboule_daemon_done_${UniqueID}
        ${Else}
          IfSilent devboule_daemon_silent_${UniqueID} devboule_daemon_ui_${UniqueID}
          devboule_daemon_silent_${UniqueID}:
            System::Call 'kernel32::AttachConsole(i -1)i.r0'
            ${If} $0 != 0
              System::Call 'kernel32::GetStdHandle(i -11)i.r0'
              System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)'
              FileWrite $0 "$R1$\n"
            ${EndIf}
            Abort
          devboule_daemon_ui_${UniqueID}:
            Abort $R3
        ${EndIf}
      devboule_daemon_cancel_${UniqueID}:
        Abort $R1
  ${EndIf}
  devboule_daemon_done_${UniqueID}:
    !undef UniqueID
!macroend

!macro DevbouleStopAppThenDaemon
  ; The template's own check below runs the same macro; ${__LINE__} keeps the labels apart.
  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro DevbouleStopDaemon
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro DevbouleStopAppThenDaemon
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro DevbouleStopAppThenDaemon
!macroend

; The uninstaller's "delete app data" checkbox: the template's own cleanup
; covers $APPDATA\$BUNDLEID and $LOCALAPPDATA\$BUNDLEID, never the daemon's
; runtime dir. Deleting the default %LOCALAPPDATA%\Devboule only is deliberate:
; a custom DEVBOULE_RUNTIME_DIR names a directory the daemon resolved, not this.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    ; $LOCALAPPDATA empty would turn the delete into "\Devboule" on the current drive.
    ${If} $LOCALAPPDATA != ""
      RmDir /r "$LOCALAPPDATA\Devboule"
    ${EndIf}
  ${EndIf}
!macroend

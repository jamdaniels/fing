; Fing NSIS installer hooks (bundle.windows.nsis.installerHooks).
;
; Why this exists
; ---------------
; Tauri's NSIS template registers the app under
; SHCTX\Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME},
; i.e. "Uninstall\Fing". winget matches installed apps to packages by that key
; name (the ARP "ProductCode"), and the unrelated winget package Fing.Fing
; (Fing Ltd's network scanner) declares ProductCode "Fing". `winget list` then
; reports this app as Fing.Fing, and `winget upgrade --all` would install Fing
; Ltd's app over ours.
;
; Fix: keep everything user-visible unchanged (DisplayName "Fing", Publisher
; "jamdaniels", install dir, exe name, shortcuts) and move only the hidden
; uninstall key to "Uninstall\com.jamdaniels.fing" (the bundle identifier).
; The winget manifest jamdaniels.Fing declares that ProductCode.
;
; How
; ---
; The template keeps reading and writing Uninstall\${PRODUCTNAME}, so:
; - POSTINSTALL: after the template has written Uninstall\Fing, replace
;   Uninstall\com.jamdaniels.fing with a copy of it and delete Uninstall\Fing.
;   This runs for every install, including the in-app updater's /UPDATE run,
;   so 1.4.0 and older installs migrate on their next update.
; - GUI init: the template's reinstall/downgrade page (PageReinstall) runs
;   before any section and only looks at Uninstall\Fing. When a migrated
;   install exists, mirror the new key back to Uninstall\Fing while the
;   installer UI is open; POSTINSTALL or .onGUIEnd removes it again. Silent
;   installs (winget's /S) skip both that page and this hook.
; - POSTUNINSTALL: delete Uninstall\com.jamdaniels.fing (the template deletes
;   Uninstall\Fing itself). Post rather than pre, because the uninstaller
;   aborts when the app is running and the user cancels, and the entry must
;   survive that.
;
; Uninstall\Fing is only deleted when its Publisher is ours. Fing Ltd's
; installer is per-machine (HKLM); ours is per-user (HKCU), so their key is
; never in the hive this file works on.
;
; Written against the template in tauri-cli 2.11.4 and re-checked for 2.12.4
; (crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi). When upgrading
; @tauri-apps/cli, re-check UNINSTKEY, PageReinstall, the downgrade check in
; .onInit and the hook insertion points. The checks in FING_ASSERT_TEMPLATE
; fail the build on obvious drift.

; This file is included before the template defines PRODUCTNAME, BUNDLEID and
; UNINSTKEY, so top-level code here cannot use them. The constants below are
; asserted against the template's values inside the hook macros, which expand
; after those defines exist.
!define FING_PRODUCT_NAME "Fing"
!define FING_PUBLISHER "jamdaniels"
!define FING_LEGACY_UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${FING_PRODUCT_NAME}"
!define FING_UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\com.jamdaniels.fing"

; Fing Dev builds (tauri.dev.conf.json) keep the template's default key.
!macro FING_ASSERT_TEMPLATE
  !ifndef FING_TEMPLATE_CHECKED
  !define FING_TEMPLATE_CHECKED
  !if "${PRODUCTNAME}" == "${FING_PRODUCT_NAME}"
    !define FING_MIGRATE_UNINSTKEY
    !if "${UNINSTKEY}" != "${FING_LEGACY_UNINSTKEY}"
      !error "hooks.nsh: the Tauri template's UNINSTKEY changed; re-check the uninstall key migration"
    !endif
    !if "${BUNDLEID}" != "com.jamdaniels.fing"
      !error "hooks.nsh: the bundle identifier changed; FING_UNINSTKEY must match the winget ProductCode"
    !endif
    !if "${MANUFACTURER}" != "${FING_PUBLISHER}"
      !error "hooks.nsh: the publisher changed; the legacy key's Publisher check relies on it"
    !endif
    ; With downgrades disabled, the template (since tauri-cli 2.12) checks the
    ; installed version in .onInit for silent, passive and update installs by
    ; reading Uninstall\${PRODUCTNAME}. That runs before FingGuiInit can mirror
    ; the key, so the check would silently never fire.
    !if "${ALLOWDOWNGRADES}" == "false"
      !error "hooks.nsh: allowDowngrades=false needs the downgrade check in .onInit to read FING_UNINSTKEY"
    !endif
  !endif
  !endif
!macroend

Var FingMirroredLegacyKey

; Replaces SHCTX\$R1 with a copy of every value in SHCTX\$R0. Values are
; enumerated generically; REG_SZ/REG_EXPAND_SZ and REG_DWORD (all the template
; writes) are copied, anything else is skipped.
Function FingCopyRegKey
  Push $0 ; value index
  Push $1 ; value name
  Push $2 ; value data
  Push $3 ; 1 while the source's unnamed default value is still to come

  DeleteRegKey SHCTX "$R1"

  ; EnumRegValue returns "" both for the unnamed default value and at the end
  ; of the list, so accept exactly one "" when a default value exists.
  StrCpy $3 0
  ClearErrors
  ReadRegStr $2 SHCTX "$R0" ""
  ${IfNot} ${Errors}
    StrCpy $3 1
  ${Else}
    ClearErrors
    ReadRegDWORD $2 SHCTX "$R0" ""
    ${IfNot} ${Errors}
      StrCpy $3 1
    ${EndIf}
  ${EndIf}

  StrCpy $0 0
  ${Do}
    EnumRegValue $1 SHCTX "$R0" $0
    IntOp $0 $0 + 1
    ${If} $1 == ""
      ${If} $3 = 1
        StrCpy $3 0
      ${Else}
        ${Break}
      ${EndIf}
    ${EndIf}

    ; ReadRegStr sets the error flag for REG_DWORD values.
    ClearErrors
    ReadRegStr $2 SHCTX "$R0" "$1"
    ${IfNot} ${Errors}
      WriteRegStr SHCTX "$R1" "$1" "$2"
    ${Else}
      ClearErrors
      ReadRegDWORD $2 SHCTX "$R0" "$1"
      ${IfNot} ${Errors}
        WriteRegDWORD SHCTX "$R1" "$1" "$2"
      ${Else}
        DetailPrint "Skipped uninstall registry value with an unsupported type: $1"
      ${EndIf}
    ${EndIf}
  ${Loop}

  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; Moves Uninstall\Fing to Uninstall\com.jamdaniels.fing, but only when
; Uninstall\Fing exists and is ours.
Function FingMigrateUninstallKey
  Push $0
  Push $R0
  Push $R1

  ReadRegStr $0 SHCTX "${FING_LEGACY_UNINSTKEY}" "Publisher"
  ${If} $0 S== "${FING_PUBLISHER}"
    StrCpy $R0 "${FING_LEGACY_UNINSTKEY}"
    StrCpy $R1 "${FING_UNINSTKEY}"
    Call FingCopyRegKey

    ; Only drop the old key once the new one is complete.
    ClearErrors
    ReadRegStr $0 SHCTX "${FING_UNINSTKEY}" "UninstallString"
    ${IfNot} ${Errors}
    ${AndIf} $0 != ""
      DeleteRegKey SHCTX "${FING_LEGACY_UNINSTKEY}"
    ${EndIf}
  ${EndIf}

  Pop $R1
  Pop $R0
  Pop $0
FunctionEnd

; MUI calls this from .onGUIInit, after the template's .onInit has set the
; shell context and registry view, and before PageReinstall runs.
!define MUI_CUSTOMFUNCTION_GUIINIT FingGuiInit
Function FingGuiInit
  StrCpy $FingMirroredLegacyKey 0
  ${If} "$(^Name)" S!= "${FING_PRODUCT_NAME}"
    Return
  ${EndIf}

  Push $0
  Push $R0
  Push $R1

  ; Mirror only when a migrated install exists and nothing is at the old key.
  ClearErrors
  ReadRegStr $0 SHCTX "${FING_UNINSTKEY}" "UninstallString"
  ${IfNot} ${Errors}
    EnumRegValue $0 SHCTX "${FING_LEGACY_UNINSTKEY}" 0
    ${If} ${Errors}
    ${OrIf} $0 == ""
      StrCpy $R0 "${FING_UNINSTKEY}"
      StrCpy $R1 "${FING_LEGACY_UNINSTKEY}"
      Call FingCopyRegKey
      StrCpy $FingMirroredLegacyKey 1
    ${EndIf}
  ${EndIf}

  Pop $R1
  Pop $R0
  Pop $0
FunctionEnd

; Runs whenever the installer UI closes: finished, cancelled or failed. After
; a successful install POSTINSTALL has already removed the mirror and this is
; a no-op. Otherwise it folds the mirror (or whatever a failed install wrote
; to it) back into the new key.
Function .onGUIEnd
  ${If} $FingMirroredLegacyKey = 1
    Call FingMigrateUninstallKey
  ${EndIf}
FunctionEnd

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro FING_ASSERT_TEMPLATE
  !ifdef FING_MIGRATE_UNINSTKEY
    Call FingMigrateUninstallKey
  !endif
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro FING_ASSERT_TEMPLATE
  !ifdef FING_MIGRATE_UNINSTKEY
    DeleteRegKey SHCTX "${FING_UNINSTKEY}"
  !endif
!macroend

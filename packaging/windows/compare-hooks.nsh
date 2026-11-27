; Opt in with /GALLCOMPARE=1 and remove the action with /GALLCOMPARE=0.
; Register a single-file verb. Gall asks for the other file in a picker.
!define GALL_COMPARE_VERB "Software\Classes\*\shell\org.gall.app.compare"
!define GALL_DEFAULT_FM_VERB "org.gall.app.open"
!define GALL_PREVIOUS_VERB_SAVED "GallPreviousDefaultVerbSaved"
!define GALL_PREVIOUS_VERB "GallPreviousDefaultVerb"
!define GALL_DIR_SHELL "Software\Classes\Directory\shell"
!define GALL_DRIVE_SHELL "Software\Classes\Drive\shell"

!macro GALL_REMOVE_COMPARE_VERB
  ReadRegStr $R0 HKCU "${GALL_COMPARE_VERB}\command" ""
  ${If} $R0 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" compare-files -- $\"%1$\""
    DeleteRegKey HKCU "${GALL_COMPARE_VERB}"
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  ${EndIf}
!macroend

!macro GALL_REMOVE_DEFAULT_FM_KEY SHELL_KEY
  ClearErrors
  ReadRegStr $R0 HKCU "${SHELL_KEY}" ""
  ${IfNot} ${Errors}
    ${If} $R0 == "${GALL_DEFAULT_FM_VERB}"
      ReadRegStr $R1 HKCU "${SHELL_KEY}\${GALL_DEFAULT_FM_VERB}" "${GALL_PREVIOUS_VERB_SAVED}"
      ${If} $R1 == "1"
        ReadRegStr $R2 HKCU "${SHELL_KEY}\${GALL_DEFAULT_FM_VERB}" "${GALL_PREVIOUS_VERB}"
        ${If} $R2 == ""
          DeleteRegValue HKCU "${SHELL_KEY}" ""
        ${Else}
          WriteRegStr HKCU "${SHELL_KEY}" "" "$R2"
        ${EndIf}
      ${Else}
        ; Older installs did not save this registry value; fall back to Explorer's normal action.
        WriteRegStr HKCU "${SHELL_KEY}" "" "open"
      ${EndIf}
    ${EndIf}
  ${EndIf}
  DeleteRegKey HKCU "${SHELL_KEY}\${GALL_DEFAULT_FM_VERB}"
!macroend

!macro GALL_REMOVE_DEFAULT_FM
  !insertmacro GALL_REMOVE_DEFAULT_FM_KEY "${GALL_DIR_SHELL}"
  !insertmacro GALL_REMOVE_DEFAULT_FM_KEY "${GALL_DRIVE_SHELL}"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Push $R0
  Push $R1
  Push $R2
  ${GetParameters} $R0
  ${GetOptions} $R0 "/GALLCOMPARE=" $R1
  ${IfNot} ${Errors}
    ${If} $R1 == "1"
      StrCpy $R2 "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" compare-files -- $\"%1$\""
      ClearErrors
      ReadRegStr $R0 HKCU "${GALL_COMPARE_VERB}\command" ""
      ${If} ${Errors}
      ${OrIf} $R0 == $R2
        WriteRegStr HKCU "${GALL_COMPARE_VERB}\command" "" "$R2"
        WriteRegStr HKCU "${GALL_COMPARE_VERB}" "" "Compare with Gall..."
        WriteRegStr HKCU "${GALL_COMPARE_VERB}" "Icon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
        WriteRegStr HKCU "${GALL_COMPARE_VERB}" "MultiSelectModel" "Single"
        System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
      ${EndIf}
    ${ElseIf} $R1 == "0"
      !insertmacro GALL_REMOVE_COMPARE_VERB
    ${EndIf}
  ${EndIf}
  Pop $R2
  Pop $R1
  Pop $R0
  ClearErrors
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; An in-place update must retain the user's choice.
  ${If} $UpdateMode <> 1
    Push $R0
    Push $R1
    Push $R2
    !insertmacro GALL_REMOVE_COMPARE_VERB
    !insertmacro GALL_REMOVE_DEFAULT_FM
    Pop $R2
    Pop $R1
    Pop $R0
    ClearErrors
  ${EndIf}
!macroend

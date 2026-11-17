Unicode True
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"

!ifndef VERSION
 !define VERSION "1.0.0"
!endif
!ifdef TEST_MODE
 !define PRODUCT_KEY "Software\KeyisleInstallerTests\Install"
 !define UNINSTALL_KEY "Software\KeyisleInstallerTests\Uninstall"
 !define RUN_KEY "Software\PasswordHelperTests\InstallerRun"
 !define MENU_FOLDER "$INSTDIR\test-shortcuts\menu"
 !define DESKTOP_FOLDER "$INSTDIR\test-shortcuts\desktop"
!else
 !define PRODUCT_KEY "Software\Keyisle"
 !define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Keyisle"
 !define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
 !define MENU_FOLDER "$SMPROGRAMS\Keyisle"
 !define DESKTOP_FOLDER "$DESKTOP"
!endif

Name "Keyisle"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\Programs\Keyisle"
InstallDirRegKey HKCU "${PRODUCT_KEY}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetDatablockOptimize on
ManifestDPIAware true
BrandingText "Keyisle"
VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "Keyisle"
VIAddVersionKey /LANG=1033 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "FileDescription" "Keyisle Setup"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Keyisle contributors"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_WELCOMEFINISHPAGE_BITMAP "${BANNER}"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "${BANNER}"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "安装 Keyisle"
!define MUI_WELCOMEPAGE_TEXT "密屿，一款轻量的本地密码管理器。$\r$\n$\r$\n安装到当前 Windows 用户，无需管理员权限。安装与升级会保留已有密码库、主密码、备份和偏好。$\r$\n$\r$\n请先从托盘退出正在运行的旧版程序。"
!define MUI_FINISHPAGE_RUN "$INSTDIR\Keyisle.exe"
!define MUI_FINISHPAGE_RUN_TEXT "启动 Keyisle"
!define MUI_UNWELCOMEPAGE_TITLE "卸载 Keyisle"
!define MUI_UNWELCOMEPAGE_TEXT "仅移除程序、快捷方式和开机启动登记。密码库、备份及偏好会保留。$\r$\n$\r$\n如需彻底清空软件管理的数据，请先在应用设置中使用“一键还原”。"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "SimpChinese"
SetFont /LANG=${LANG_SIMPCHINESE} "Microsoft YaHei UI" 9

Var OldInstallDir

!macro CheckRunning
 System::Call 'user32::FindWindowW(w "PasswordManagerPrototypeEvents", w "PasswordManagerPrototypeEvents") p.r0'
 ${If} $0 != 0
  IfSilent +2
  MessageBox MB_ICONEXCLAMATION|MB_OK "请先从托盘退出 Keyisle／旧版密码助手，再运行安装或卸载。未修改任何文件。"
  SetErrorLevel 10
  Abort
 ${EndIf}
!macroend

Function .onInit
 ${IfNot} ${RunningX64}
  IfSilent +2
  MessageBox MB_ICONSTOP|MB_OK "此安装包需要 64 位 Windows。"
  SetErrorLevel 11
  Abort
 ${EndIf}
 SetShellVarContext current
 SetRegView 64
 ReadRegStr $OldInstallDir HKCU "${PRODUCT_KEY}" "InstallDir"
 !insertmacro CheckRunning
FunctionEnd

Function .onVerifyInstDir
 ${If} $INSTDIR == ""
  Abort
 ${EndIf}
FunctionEnd

Function un.onInit
 SetShellVarContext current
 SetRegView 64
 !insertmacro CheckRunning
FunctionEnd

Section "Keyisle（必需）" Application
 SectionIn RO
 SetOutPath "$INSTDIR"
 SetOverwrite on
 File /oname=Keyisle.exe "${APP_EXE}"
 File /oname=README.md "${README}"
 File /oname=THIRD-PARTY-NOTICES.txt "${NOTICES}"
 WriteUninstaller "$INSTDIR\Uninstall.exe"
 WriteRegStr HKCU "${PRODUCT_KEY}" "InstallDir" "$INSTDIR"
 WriteRegStr HKCU "${PRODUCT_KEY}" "Version" "${VERSION}"
 WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Keyisle"
 WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
 WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\Keyisle.exe,0"
 WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
 WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
 WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
 WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
 WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1

 ; Preserve the user's startup choice when the installation directory changes.
 ReadRegStr $0 HKCU "${RUN_KEY}" "WindowsPasswordManager"
 ${If} $OldInstallDir != ""
 ${AndIf} $0 == '$\"$OldInstallDir\Keyisle.exe$\" --autostart'
  WriteRegStr HKCU "${RUN_KEY}" "WindowsPasswordManager" '$\"$INSTDIR\Keyisle.exe$\" --autostart'
 ${EndIf}
 CreateDirectory "${MENU_FOLDER}"
 CreateShortcut "${MENU_FOLDER}\Keyisle.lnk" "$INSTDIR\Keyisle.exe"
 CreateShortcut "${MENU_FOLDER}\卸载 Keyisle.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "桌面快捷方式（可选）" DesktopShortcut
 CreateDirectory "${DESKTOP_FOLDER}"
 CreateShortcut "${DESKTOP_FOLDER}\Keyisle.lnk" "$INSTDIR\Keyisle.exe"
SectionEnd

Section "Uninstall"
 ; Delete only owned files; user-added files and all vault directories survive.
 ClearErrors
 Delete "$INSTDIR\Keyisle.exe"
 ${If} ${Errors}
  IfSilent +2
  MessageBox MB_ICONEXCLAMATION|MB_OK "程序文件仍被占用，卸载未完成。请关闭占用程序后重试；密码库不受影响。"
  SetErrorLevel 12
  Quit
 ${EndIf}
 Delete "$INSTDIR\README.md"
 Delete "$INSTDIR\THIRD-PARTY-NOTICES.txt"
 Delete "$INSTDIR\Uninstall.exe"
 Delete "${DESKTOP_FOLDER}\Keyisle.lnk"
 Delete "${MENU_FOLDER}\Keyisle.lnk"
 Delete "${MENU_FOLDER}\卸载 Keyisle.lnk"
 RMDir "${MENU_FOLDER}"
 !ifdef TEST_MODE
  RMDir "${DESKTOP_FOLDER}"
  RMDir "$INSTDIR\test-shortcuts"
 !endif
 ReadRegStr $0 HKCU "${RUN_KEY}" "WindowsPasswordManager"
 ${If} $0 == '$\"$INSTDIR\Keyisle.exe$\" --autostart'
  DeleteRegValue HKCU "${RUN_KEY}" "WindowsPasswordManager"
 ${EndIf}
 DeleteRegKey HKCU "${UNINSTALL_KEY}"
 DeleteRegKey HKCU "${PRODUCT_KEY}"
 RMDir "$INSTDIR"
SectionEnd

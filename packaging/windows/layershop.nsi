; LayerShop setup .exe (NSIS 3), the alternative to the MSI that photocraft.wxs makes. Built by
; layershop-nsis.ps1, which passes:
;
;   makensis /DVERSION=1.2.3-rc.1 /DPAYLOAD=<folder with photocraft.exe, photocraft-cli.exe, licences>
;            /DICON=<.ico> /DWELCOMEBMP=<164x314 BMP> /DOUTFILE=<setup .exe> layershop.nsi
;
; It does what the MSI does: a per-machine install into Program Files\LayerShop (the folder can be
; changed), a Start Menu shortcut and an optional desktop shortcut, .pcraft owned by LayerShop and
; LayerShop offered under "Open with" for PSD/PSB and images, an entry in Apps & features, and a
; Finish page that offers to start the app. Uninstalling asks whether the local AI and the settings
; go too (`photocraft.exe --uninstall-cleanup`). `setup.exe /S` installs silently.
;
; It refuses to install over the MSI version: uninstall that first (Apps & features).

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma
RequestExecutionLevel admin

!include "MUI2.nsh"
!include "x64.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!define PRODUCT "LayerShop"
!define PUBLISHER "Slavomir Durej"
!define WEBSITE "https://layershop.aardvarkstudios.net"
!define REGKEY "Software\${PRODUCT}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}"

; An image format LayerShop is offered for under "Open with" (and listed as supporting).
!macro OpenWithImage EXT
  WriteRegStr HKLM "Software\Classes\${EXT}\OpenWithProgids" "Photocraft.Image" ""
  WriteRegStr HKLM "Software\Classes\Applications\photocraft.exe\SupportedTypes" "${EXT}" ""
!macroend
!macro un.OpenWithImage EXT
  DeleteRegValue HKLM "Software\Classes\${EXT}\OpenWithProgids" "Photocraft.Image"
!macroend

Name "${PRODUCT} ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\${PRODUCT}"
InstallDirRegKey HKLM "${REGKEY}" "InstallDir"
BrandingText "${PRODUCT} ${VERSION}"

VIProductVersion "${VERSION_NUMERIC}.0"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION_NUMERIC}.0"
VIAddVersionKey "FileDescription" "${PRODUCT} Setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "MIT OR Apache-2.0"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_WELCOMEFINISHPAGE_BITMAP "${WELCOMEBMP}"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "${WELCOMEBMP}"
!define MUI_ABORTWARNING

!define MUI_WELCOMEPAGE_TEXT "This will install ${PRODUCT} ${VERSION} on your computer.$\r$\n$\r$\nClick Next to continue."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TEXT "${PRODUCT} ${VERSION} is installed. You can start it any time from the Start menu."
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Launch ${PRODUCT}"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchApp
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "${PRODUCT} needs 64-bit Windows." /SD IDOK
    Abort
  ${EndIf}
  SetRegView 64
  ; The MSI writes InstallDir too but no NSIS uninstall entry: installing over it would leave two
  ; owners of one folder.
  ReadRegStr $0 HKLM "${REGKEY}" "InstallDir"
  ReadRegStr $1 HKLM "${UNINSTKEY}" "UninstallString"
  ${If} $0 != ""
  ${AndIf} $1 == ""
    MessageBox MB_ICONEXCLAMATION "${PRODUCT} is already installed from its .msi installer.$\r$\n$\r$\nUninstall it first (Settings > Apps), then run this setup again." /SD IDOK
    Abort
  ${EndIf}
FunctionEnd

; The app starts as the signed-in user, not with this installer's admin rights: Explorer runs it.
Function LaunchApp
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\photocraft.exe"'
FunctionEnd

Section "${PRODUCT}" SecMain
  SectionIn RO
  SetShellVarContext all
  SetOutPath "$INSTDIR"
  File "${PAYLOAD}\photocraft.exe"
  File "${PAYLOAD}\photocraft-cli.exe"
  File /nonfatal "${PAYLOAD}\LICENSE-MIT"
  File /nonfatal "${PAYLOAD}\LICENSE-APACHE"
  File /nonfatal "${PAYLOAD}\README.md"
  ; Font licences: only in builds made with craft-fonts.
  !pragma warning disable 7010
  File /nonfatal "${PAYLOAD}\OFL-*.txt"
  !pragma warning default 7010
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\photocraft.exe" "" "$INSTDIR\photocraft.exe" 0 SW_SHOWNORMAL "" "${PRODUCT} image editor"

  WriteRegStr HKLM "${REGKEY}" "InstallDir" "$INSTDIR"

  ; .pcraft: PhotoCraft's own format; LayerShop is its default app.
  WriteRegStr HKLM "Software\Classes\.pcraft" "" "Photocraft.Document"
  WriteRegStr HKLM "Software\Classes\.pcraft" "Content Type" "application/x-photocraft"
  WriteRegStr HKLM "Software\Classes\Photocraft.Document" "" "${PRODUCT} Document"
  WriteRegStr HKLM "Software\Classes\Photocraft.Document\DefaultIcon" "" '"$INSTDIR\photocraft.exe",0'
  WriteRegStr HKLM "Software\Classes\Photocraft.Document\shell\open\command" "" '"$INSTDIR\photocraft.exe" "%1"'
  ; PSD/PSB and images: offered under "Open with", never forced as the default.
  WriteRegStr HKLM "Software\Classes\Photocraft.PhotoshopDocument" "" "PSD Document"
  WriteRegStr HKLM "Software\Classes\Photocraft.PhotoshopDocument\DefaultIcon" "" '"$INSTDIR\photocraft.exe",0'
  WriteRegStr HKLM "Software\Classes\Photocraft.PhotoshopDocument\shell\open\command" "" '"$INSTDIR\photocraft.exe" "%1"'
  WriteRegStr HKLM "Software\Classes\Photocraft.Image" "" "Image"
  WriteRegStr HKLM "Software\Classes\Photocraft.Image\DefaultIcon" "" '"$INSTDIR\photocraft.exe",0'
  WriteRegStr HKLM "Software\Classes\Photocraft.Image\shell\open\command" "" '"$INSTDIR\photocraft.exe" "%1"'
  WriteRegStr HKLM "Software\Classes\.psd\OpenWithProgids" "Photocraft.PhotoshopDocument" ""
  WriteRegStr HKLM "Software\Classes\.psb\OpenWithProgids" "Photocraft.PhotoshopDocument" ""
  WriteRegStr HKLM "Software\Classes\Applications\photocraft.exe" "FriendlyAppName" "${PRODUCT}"
  WriteRegStr HKLM "Software\Classes\Applications\photocraft.exe\shell\open\command" "" '"$INSTDIR\photocraft.exe" "%1"'
  WriteRegStr HKLM "Software\Classes\Applications\photocraft.exe\SupportedTypes" ".pcraft" ""
  WriteRegStr HKLM "Software\Classes\Applications\photocraft.exe\SupportedTypes" ".psd" ""
  WriteRegStr HKLM "Software\Classes\Applications\photocraft.exe\SupportedTypes" ".psb" ""
  !insertmacro OpenWithImage ".pdn"
  !insertmacro OpenWithImage ".ora"
  !insertmacro OpenWithImage ".kra"
  !insertmacro OpenWithImage ".krz"
  !insertmacro OpenWithImage ".png"
  !insertmacro OpenWithImage ".jpg"
  !insertmacro OpenWithImage ".jpeg"
  !insertmacro OpenWithImage ".tif"
  !insertmacro OpenWithImage ".tiff"
  !insertmacro OpenWithImage ".webp"
  !insertmacro OpenWithImage ".gif"
  !insertmacro OpenWithImage ".bmp"
  !insertmacro OpenWithImage ".tga"
  !insertmacro OpenWithImage ".exr"
  !insertmacro OpenWithImage ".hdr"
  !insertmacro OpenWithImage ".avif"
  !insertmacro OpenWithImage ".qoi"
  !insertmacro OpenWithImage ".ico"
  ; The Run dialog (Win+R "photocraft").
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\App Paths\photocraft.exe" "" "$INSTDIR\photocraft.exe"

  ; Apps & features.
  WriteRegStr HKLM "${UNINSTKEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKLM "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTKEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINSTKEY}" "DisplayIcon" '"$INSTDIR\photocraft.exe",0'
  WriteRegStr HKLM "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTKEY}" "URLInfoAbout" "${WEBSITE}"
  WriteRegStr HKLM "${UNINSTKEY}" "HelpLink" "https://github.com/SlavomirDurej/layershop/issues"
  WriteRegStr HKLM "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKLM "${UNINSTKEY}" "EstimatedSize" $0

  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

Section "Desktop shortcut" SecDesktop
  SetShellVarContext all
  CreateShortcut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\photocraft.exe" "" "$INSTDIR\photocraft.exe" 0 SW_SHOWNORMAL "" "${PRODUCT} image editor"
  WriteRegDWORD HKLM "${REGKEY}" "DesktopShortcut" 1
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The ${PRODUCT} app and its command-line tool."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A ${PRODUCT} shortcut on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Function un.onInit
  SetRegView 64
FunctionEnd

Section "Uninstall"
  ; The app asks whether its local AI (engine and models, often tens of GB in
  ; %LOCALAPPDATA%\LayerShop) and the user's settings go too, and removes what is ticked
  ; (apps/photocraft/src/uninstall.rs). A silent uninstall keeps everything. The "." keeps the
  ; folders' trailing backslash from escaping the closing quote.
  SetShellVarContext current
  ${IfNot} ${Silent}
    ExecWait '"$INSTDIR\photocraft.exe" --uninstall-cleanup --local "$LOCALAPPDATA\." --roaming "$APPDATA\."'
  ${EndIf}

  SetShellVarContext all
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  Delete "$DESKTOP\${PRODUCT}.lnk"
  Delete "$INSTDIR\photocraft.exe"
  Delete "$INSTDIR\photocraft-cli.exe"
  Delete "$INSTDIR\LICENSE-MIT"
  Delete "$INSTDIR\LICENSE-APACHE"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\OFL-*.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKLM "Software\Classes\Photocraft.Document"
  DeleteRegKey HKLM "Software\Classes\Photocraft.PhotoshopDocument"
  DeleteRegKey HKLM "Software\Classes\Photocraft.Image"
  ReadRegStr $0 HKLM "Software\Classes\.pcraft" ""
  ${If} $0 == "Photocraft.Document"
    DeleteRegKey HKLM "Software\Classes\.pcraft"
  ${EndIf}
  DeleteRegValue HKLM "Software\Classes\.psd\OpenWithProgids" "Photocraft.PhotoshopDocument"
  DeleteRegValue HKLM "Software\Classes\.psb\OpenWithProgids" "Photocraft.PhotoshopDocument"
  !insertmacro un.OpenWithImage ".pdn"
  !insertmacro un.OpenWithImage ".ora"
  !insertmacro un.OpenWithImage ".kra"
  !insertmacro un.OpenWithImage ".krz"
  !insertmacro un.OpenWithImage ".png"
  !insertmacro un.OpenWithImage ".jpg"
  !insertmacro un.OpenWithImage ".jpeg"
  !insertmacro un.OpenWithImage ".tif"
  !insertmacro un.OpenWithImage ".tiff"
  !insertmacro un.OpenWithImage ".webp"
  !insertmacro un.OpenWithImage ".gif"
  !insertmacro un.OpenWithImage ".bmp"
  !insertmacro un.OpenWithImage ".tga"
  !insertmacro un.OpenWithImage ".exr"
  !insertmacro un.OpenWithImage ".hdr"
  !insertmacro un.OpenWithImage ".avif"
  !insertmacro un.OpenWithImage ".qoi"
  !insertmacro un.OpenWithImage ".ico"
  DeleteRegKey HKLM "Software\Classes\Applications\photocraft.exe"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\App Paths\photocraft.exe"
  DeleteRegKey HKLM "${REGKEY}"
  DeleteRegKey HKLM "${UNINSTKEY}"

  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

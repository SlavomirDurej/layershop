<#
.SYNOPSIS
  Build, sign and package LayerShop (this fork of PhotoCraft) for Windows.

.DESCRIPTION
  Produces, in $env:DIST (default: dist/release):
    layershop-<version>-windows-<arch>.msi             per-machine installer (WiX v5)
    layershop-<version>-windows-<arch>-portable.zip    photocraft.exe + photocraft-cli.exe + portable.txt
    layershop-<version>-windows-<arch>-symbols.zip     photocraft.pdb + photocraft_cli.pdb (optional, for profilers)

  The release assets carry the LayerShop name ($AssetName); the binaries keep their machine names.

  The binaries link the C runtime statically (+crt-static), so neither the MSI nor the portable
  zip needs the Visual C++ redistributable. The x64 packages still carry Microsoft's C++ runtime
  DLLs in vc-runtime\, for the local AI engine the app installs (ComfyUI's PyTorch needs them). Signing is delegated to sign.ps1 (skipped with a
  warning when no signing secrets are set).

  Needs: Rust (MSVC toolchain + the target), the Windows SDK (rc.exe, signtool.exe),
  and WiX v5: dotnet tool install --global wix --version 5.0.2
             wix extension add -g WixToolset.UI.wixext/5.0.2 WixToolset.Util.wixext/5.0.2

.EXAMPLE
  pwsh packaging/windows/package.ps1 -Arch x64
  pwsh packaging/windows/package.ps1 -Arch x86 -SkipBuild
  pwsh packaging/windows/package.ps1 -Arch arm64     # cross-compiled; needs the MSVC ARM64 build tools
#>
param(
  [ValidateSet('x64', 'x86', 'arm64')] [string] $Arch = 'x64',
  [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
# Release asset file names start with this (the product name, lowercase).
$AssetName = 'layershop'

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
$Version = $env:PHOTOCRAFT_VERSION
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
    if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
# MSI ProductVersion is numeric (major.minor.build); pre-release tags are dropped there.
$MsiVersion = ($Version -split '-')[0]

$Target = switch ($Arch) { 'x64' { 'x86_64-pc-windows-msvc' } 'x86' { 'i686-pc-windows-msvc' } 'arm64' { 'aarch64-pc-windows-msvc' } }
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

if (-not $env:PHOTOCRAFT_BUILD_SHA) { $env:PHOTOCRAFT_BUILD_SHA = (git -C $Root rev-parse HEAD 2>$null) }
if (-not $env:PHOTOCRAFT_BUILD_DATE) { $env:PHOTOCRAFT_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }

Write-Output "LayerShop $Version for Windows $Arch ($Target)"

if (-not $SkipBuild) {
  # Static CRT: no VC++ redistributable needed. Scoped to the target so host build scripts and
  # proc-macros are unaffected.
  $flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
  [Environment]::SetEnvironmentVariable($flagVar, '-C target-feature=+crt-static')
  # Fail the build (rather than warn) if the icon/VERSIONINFO can't be embedded.
  $env:PHOTOCRAFT_REQUIRE_WINRES = '1'
  Invoke-Native "cargo build ($Target)" { cargo build --release --locked -p photocraft -p photocraft-cli --features heif --target $Target }
}

$Bin = Join-Path $TargetDir "$Target\release"

# Check both binaries' PE headers before packaging. Machine (COFF header) must match -Arch, so an
# x64 build can never ship labelled arm64. Subsystem (optional header): 2 = Windows GUI, 3 = console.
# The app must be GUI (no console window opens with it); the CLI must stay console so its output
# reaches the terminal.
function Get-PeHeader([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}
$Machine = switch ($Arch) { 'x64' { 0x8664 } 'x86' { 0x14C } 'arm64' { 0xAA64 } }
foreach ($check in @(@('photocraft.exe', 2), @('photocraft-cli.exe', 3))) {
  $h = Get-PeHeader (Join-Path $Bin $check[0])
  if ($h.Machine -ne $Machine) { throw "$($check[0]) is for machine 0x$('{0:X}' -f $h.Machine), expected 0x$('{0:X}' -f $Machine) ($Arch)" }
  if ($h.Subsystem -ne $check[1]) { throw "$($check[0]) has PE subsystem $($h.Subsystem), expected $($check[1])" }
  Write-Output "ok $($check[0]): $Arch, PE subsystem $($h.Subsystem)"
}
$Stage = Join-Path $TargetDir "windows-package\$Arch"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item (Join-Path $Bin 'photocraft.exe'), (Join-Path $Bin 'photocraft-cli.exe') $Stage

& (Join-Path $PSScriptRoot 'sign.ps1') (Join-Path $Stage 'photocraft.exe') (Join-Path $Stage 'photocraft-cli.exe')

# ---- the Microsoft C++ runtime, for the local AI engine -------------------------------------------
# ComfyUI's portable build has no msvcp140.dll nor vcruntime140_threads.dll, which its PyTorch
# needs (c10.dll and torch_cpu.dll import them): on a PC without the
# Visual C++ Redistributable (a new one, Windows Sandbox) the engine stops while starting with
# WinError 126. The app carries Microsoft's runtime DLLs in vc-runtime\ and the Local AI setup puts
# them beside the engine's Python (crates/aisetup/src/vcrt.rs), which Microsoft allows and which
# needs no administrator rights. The set is the newest complete, Microsoft-signed one on this PC:
# a Visual Studio redistributable folder (Microsoft.VC14x.CRT), else System32 (where the
# Redistributable installs it). PyTorch is built with MSVC 14.42; an older msvcp140 can crash it.
$VcDlls = 'msvcp140.dll', 'msvcp140_1.dll', 'msvcp140_2.dll', 'msvcp140_atomic_wait.dll', 'msvcp140_codecvt_ids.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'vcruntime140_threads.dll', 'concrt140.dll'
$VcMinimum = [version]'14.42.0.0'
function Copy-VcRuntime([string] $Dest) {
  $candidates = @(Get-ChildItem -Directory -ErrorAction SilentlyContinue -Path @(
      "$env:ProgramFiles\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT",
      "${env:ProgramFiles(x86)}\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT"
    ) | ForEach-Object FullName) + @(Join-Path $env:SystemRoot 'System32')
  $best = $null
  foreach ($dir in $candidates) {
    $files = $VcDlls | ForEach-Object { Join-Path $dir $_ }
    if (@($files | Where-Object { -not (Test-Path $_) }).Count) { continue }
    $signed = @($files | Where-Object { $s = Get-AuthenticodeSignature $_; $s.Status -eq 'Valid' -and $s.SignerCertificate.Subject -like 'CN=Microsoft*' })
    if ($signed.Count -ne $files.Count) { continue }
    # The numbers, not FileVersion's text (which can read "14.29.30157.0 built by: …").
    $info = (Get-Item (Join-Path $dir 'msvcp140.dll')).VersionInfo
    $version = [version]::new($info.FileMajorPart, $info.FileMinorPart, $info.FileBuildPart, $info.FilePrivatePart)
    # The redistributable folders before System32 at the same version: their files are the ones to redistribute.
    if ($version -ge $VcMinimum -and (-not $best -or $version -gt $best.Version)) { $best = @{ Dir = $dir; Version = $version } }
  }
  if (-not $best) {
    throw "no Microsoft C++ runtime $VcMinimum or newer was found (Visual Studio's Microsoft.VC14x.CRT, or System32): install the latest Visual C++ Redistributable x64 (https://aka.ms/vs/17/release/vc_redist.x64.exe) or the Visual Studio Build Tools"
  }
  New-Item -ItemType Directory -Force -Path $Dest | Out-Null
  foreach ($n in $VcDlls) { Copy-Item (Join-Path $best.Dir $n) $Dest }
  Set-Content -Path (Join-Path $Dest 'README.txt') -Encoding ascii -Value @(
    "Microsoft Visual C++ runtime $($best.Version), x64, redistributed with LayerShop as Microsoft allows.",
    "LayerShop's Local AI setup copies these files beside the AI engine's Python (ComfyUI), whose",
    "PyTorch needs them, so the engine runs on a PC without the Visual C++ Redistributable.",
    "LayerShop itself does not use them."
  )
  Write-Output "ok vc-runtime: Microsoft C++ runtime $($best.Version) from $($best.Dir)"
}
# ComfyUI's portable build is x64 only: so is the runtime it gets.
if ($Arch -eq 'x64') { Copy-VcRuntime (Join-Path $Stage 'vc-runtime') }
# ---- font notices for the MSI ------------------------------------------------------------------
# The portable ZIP already carries these notices; stage the complete files before MSI creation too.
$Docs = Join-Path $Stage 'font-licences'
New-Item -ItemType Directory -Force -Path $Docs | Out-Null
# The existing MSI document group also carries notices for bundled egui font inputs.
Copy-Item -LiteralPath (Join-Path $Root 'NOTICE'), (Join-Path $Root 'ATTRIBUTION.md') -Destination $Docs
$HasCraftFontLicences = 1
if ($env:CRAFT_FONTS_DIR) {
  foreach ($ofl in Get-ChildItem -Path (Join-Path $env:CRAFT_FONTS_DIR 'fonts\*\OFL.txt') -File -ErrorAction SilentlyContinue) {
    Copy-Item -LiteralPath $ofl.FullName -Destination (Join-Path $Docs "OFL-$($ofl.Directory.Name).txt")
    $HasCraftFontLicences = 1
  }
}
# ---- end font notices for the MSI --------------------------------------------------------------

# ---- MSI ---------------------------------------------------------------------------------------
# The setup wizard's art, drawn here from the app icon so the MSI shows LayerShop rather than
# WiX's stock bitmaps: a 493x58 banner across the top of the inner pages (the page title is drawn
# over its left side, so that stays white) and a 493x312 backdrop for the Welcome and Finish pages
# (their text sits on the right; the left 164 px are ours). Both must be 24-bit BMPs.
# The left panel is the icon's own background colour, so the icon's tile melts into it. The right
# side is the Windows dialog grey, not white: Windows Installer can't draw a checkbox transparently,
# so on white the Finish page's "Launch" checkbox sat on a visible grey box.
function New-InstallerArt([string] $IconPng, [string] $BannerOut, [string] $DialogOut) {
  Add-Type -AssemblyName System.Drawing
  $paper = [System.Drawing.ColorTranslator]::FromHtml('#1b1f26')
  $accent = [System.Drawing.ColorTranslator]::FromHtml('#2f7bf5')
  $dialogGrey = [System.Drawing.ColorTranslator]::FromHtml('#f0f0f0')
  $icon = [System.Drawing.Image]::FromFile($IconPng)
  try {
    foreach ($spec in @(@($BannerOut, 493, 58), @($DialogOut, 493, 312))) {
      $bmp = New-Object System.Drawing.Bitmap $spec[1], $spec[2], ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
      $g = [System.Drawing.Graphics]::FromImage($bmp)
      try {
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.Clear([System.Drawing.Color]::White)
        if ($spec[2] -eq 58) {
          $g.DrawImage($icon, 493 - 46 - 8, 6, 46, 46)
        } else {
          $g.Clear($dialogGrey)
          $panel = New-Object System.Drawing.SolidBrush $paper
          $stripe = New-Object System.Drawing.SolidBrush $accent
          $g.FillRectangle($panel, 0, 0, 164, 312)
          $g.FillRectangle($stripe, 160, 0, 4, 312)
          # The icon without its tile's 1 px light border (24 px cropped off each side of the
          # 256 px source, same scale), so only the layers sit on the panel.
          $crop = 24; $scale = 124 / $icon.Width
          $src = New-Object System.Drawing.RectangleF $crop, $crop, ($icon.Width - 2 * $crop), ($icon.Height - 2 * $crop)
          $dst = New-Object System.Drawing.RectangleF (18 + $crop * $scale), (64 + $crop * $scale), ($src.Width * $scale), ($src.Height * $scale)
          $g.DrawImage($icon, $dst, $src, [System.Drawing.GraphicsUnit]::Pixel)
          $panel.Dispose(); $stripe.Dispose()
        }
      } finally { $g.Dispose() }
      $bmp.Save($spec[0], [System.Drawing.Imaging.ImageFormat]::Bmp)
      $bmp.Dispose()
    }
  } finally { $icon.Dispose() }
}
$Banner = Join-Path $Stage 'installer-banner.bmp'
$Dialog = Join-Path $Stage 'installer-dialog.bmp'
New-InstallerArt (Join-Path $Root 'assets\app-icon\hicolor\256x256\apps\ai.storyteller.photocraft.png') $Banner $Dialog

$Msi = Join-Path $Dist "$AssetName-$Version-windows-$Arch.msi"
& (Join-Path $PSScriptRoot 'check-icons.ps1')
Invoke-Native 'wix build' {
  wix build (Join-Path $PSScriptRoot 'photocraft.wxs') -arch $Arch `
    -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext `
    -culture en-US -loc (Join-Path $PSScriptRoot 'photocraft.en-us.wxl') `
    -d "Version=$MsiVersion" -d "DisplayVersion=$Version" -d "BinDir=$Stage" -d "IconPath=$(Join-Path $Root 'assets\app-icon\photocraft.ico')" `
    -d "BannerBmp=$Banner" -d "DialogBmp=$Dialog" `
    -d "DocsDir=$Docs" -d "HasCraftFontLicences=$HasCraftFontLicences" `
    -o $Msi
}
Invoke-Native 'MSI shortcut icon validation (ICE50)' {
  wix msi validate $Msi -ice ICE50 -intermediateFolder (Join-Path $Stage 'msi-validation')
}
# Upstream's check-install-policy.ps1 is not run: it checks PhotoCraft's per-user installer
# (install-policy.wxi, Programs\ArtCraft\PhotoCraft), and LayerShop's MSI installs per machine.
# wix writes its debug symbols (.wixpdb) next to the MSI; keep them out of the release assets.
Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($Msi, '.wixpdb'))
& (Join-Path $PSScriptRoot 'sign.ps1') $Msi

# ---- portable zip ------------------------------------------------------------------------------
$Portable = Join-Path $TargetDir "windows-package\$AssetName-$Version-windows-$Arch-portable"
Remove-Item -Recurse -Force $Portable -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Portable | Out-Null
Copy-Item (Join-Path $Stage '*.exe') $Portable
$VcStage = Join-Path $Stage 'vc-runtime'
if (Test-Path $VcStage) { Copy-Item -Recurse $VcStage $Portable }
foreach ($f in 'README.md', 'LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'ATTRIBUTION.md') {
  $p = Join-Path $Root $f
  if (Test-Path $p) { Copy-Item $p $Portable }
}
# Builds made with craft-fonts (CRAFT_FONTS_DIR, all official releases) embed its OFL-1.1 fonts:
# ship each font's licence as OFL-<family-dir>.txt.
if ($env:CRAFT_FONTS_DIR) {
  Get-ChildItem -Path (Join-Path $env:CRAFT_FONTS_DIR 'fonts') -Directory -ErrorAction SilentlyContinue | ForEach-Object {
    $lic = Join-Path $_.FullName 'OFL.txt'
    if (Test-Path $lic) { Copy-Item $lic (Join-Path $Portable "OFL-$($_.Name).txt") }
  }
}
# portable.txt beside photocraft.exe switches on portable mode: settings, presets and recovery
# files go to PhotoCraftData\ next to the exe instead of %APPDATA% (#228; see app_dirs.rs).
Copy-Item (Join-Path $PSScriptRoot 'portable.txt') $Portable
$Zip = Join-Path $Dist "$AssetName-$Version-windows-$Arch-portable.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
# The archive holds the files themselves. `-Path <folder>` puts the folder *in* the archive, so
# photocraft.exe came out one level down and portable mode needs portable.txt beside the exe
# wherever it is extracted (#7699); `<folder>\*` archives the contents, as windows7.ps1 does in
# VectorCraft. Checked below, since nothing else reads the layout back.
Compress-Archive -Path (Join-Path $Portable '*') -DestinationPath $Zip
$Check = Join-Path $TargetDir 'windows-package\portable-zip-check'
Remove-Item -Recurse -Force $Check -ErrorAction SilentlyContinue
Expand-Archive -Path $Zip -DestinationPath $Check -Force
foreach ($f in 'photocraft.exe', 'photocraft-cli.exe', 'portable.txt') {
  if (-not (Test-Path (Join-Path $Check $f))) { throw "the portable zip has no $f at its root: $Zip" }
}
Remove-Item -Recurse -Force $Check -ErrorAction SilentlyContinue

# ---- debug symbols -----------------------------------------------------------------------------
# The linker writes a PDB beside each exe, and the exe names it without a path (photocraft.pdb,
# photocraft_cli.pdb). They ship as a separate, optional download so profilers (Windows Performance
# Analyzer, Superluminal, Visual Studio) show PhotoCraft's function names for the released
# binaries (#688). Signing doesn't change the PDB signature an exe records, so they match the
# signed exes; a rebuild of the same commit writes a PDB that doesn't.
$Symbols = Join-Path $TargetDir "windows-package\$AssetName-$Version-windows-$Arch-symbols"
Remove-Item -Recurse -Force $Symbols -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Symbols | Out-Null
foreach ($pdb in 'photocraft.pdb', 'photocraft_cli.pdb') {
  $p = Join-Path $Bin $pdb
  if (-not (Test-Path $p)) { throw "$pdb is missing from $($Bin), where the release build writes it" }
  Copy-Item $p $Symbols
}
Copy-Item (Join-Path $PSScriptRoot 'symbols.txt') $Symbols
$SymbolsZip = Join-Path $Dist "$AssetName-$Version-windows-$Arch-symbols.zip"
Remove-Item -Force $SymbolsZip -ErrorAction SilentlyContinue
Compress-Archive -Path $Symbols -DestinationPath $SymbolsZip

# Smoke-test the CLI when this machine can run it. An ARM64 build made on an x64 runner can't run
# here; .github/workflows/windows-arm64.yml installs and runs it on ARM64 instead.
$HostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($Arch -ne 'arm64' -or $HostArch -eq 'arm64') {
  Invoke-Native 'photocraft-cli --version' { & (Join-Path $Stage 'photocraft-cli.exe') --version }
} else {
  Write-Output "skipping photocraft-cli --version: an $Arch build doesn't run on this $HostArch machine"
}
Get-Item $Msi, $Zip, $SymbolsZip | Format-Table Name, Length

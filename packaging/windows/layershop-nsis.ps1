<#
.SYNOPSIS
  Make LayerShop's setup .exe (NSIS) from a portable zip that layershop-build.ps1 made.

.DESCRIPTION
  The setup .exe installs exactly the files of the portable zip it is made from (by default
  dist\release\layershop-<version>-windows-x64-portable.zip), with what layershop.nsi adds:
  shortcuts, file associations, an Apps & features entry and an uninstaller. The result goes to
  dist\nsis\layershop-<version>-windows-x64-setup.exe, signed by sign.ps1 when signing is set up.

  makensis (NSIS 3) is used from PATH, from an NSIS install in Program Files, or from
  electron-builder's cache (%LOCALAPPDATA%\electron-builder\Cache\nsis); set MAKENSIS to choose one.

.EXAMPLE
  pwsh -NoProfile -File packaging/windows/layershop-nsis.ps1
  pwsh -NoProfile -File packaging/windows/layershop-nsis.ps1 -Zip path\to\layershop-0.9.0-windows-x64-portable.zip
#>
param([string] $Zip)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Find-MakeNsis {
  if ($env:MAKENSIS) { return $env:MAKENSIS }
  $onPath = Get-Command makensis.exe -ErrorAction SilentlyContinue
  if ($onPath) { return $onPath.Source }
  foreach ($p in @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe")) {
    if (Test-Path $p) { return $p }
  }
  $cached = Get-ChildItem -Path (Join-Path $env:LOCALAPPDATA 'electron-builder\Cache\nsis') -Filter makensis.exe -Recurse -ErrorAction SilentlyContinue |
    Where-Object { $_.Directory.Name -ne 'Bin' } | Sort-Object FullName -Descending | Select-Object -First 1
  if ($cached) { return $cached.FullName }
  throw 'makensis.exe was not found: install NSIS 3 (https://nsis.sourceforge.io) or set MAKENSIS'
}

if (-not $Zip) {
  $Zip = Get-ChildItem -Path (Join-Path $Root 'dist\release') -Filter 'layershop-*-windows-x64-portable.zip' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
  if (-not $Zip) { throw 'no dist\release\layershop-*-windows-x64-portable.zip: run layershop-build.ps1 first, or pass -Zip' }
}
$ZipName = [IO.Path]::GetFileNameWithoutExtension($Zip)
if ($ZipName -notmatch '^layershop-(.+)-windows-x64-portable$') { throw "not a LayerShop portable zip: $Zip" }
$Version = $Matches[1]
# NSIS's VIProductVersion is numeric: a pre-release tag is dropped there.
$Numeric = ($Version -split '-')[0]
if ($Numeric -notmatch '^\d+\.\d+\.\d+$') { throw "can't read a version from $ZipName" }

$Work = Join-Path $Root 'target\nsis'
Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Work | Out-Null
Expand-Archive -Path $Zip -DestinationPath $Work
$Payload = Join-Path $Work $ZipName
if (-not (Test-Path (Join-Path $Payload 'photocraft.exe'))) { throw "$Zip has no $ZipName\photocraft.exe" }

# The Welcome and Finish pages' 164x314 left panel: the app icon on the dark panel colour with the
# accent stripe, as the MSI's (package.ps1, New-InstallerArt). A 24-bit BMP.
Add-Type -AssemblyName System.Drawing
$WelcomeBmp = Join-Path $Work 'welcome.bmp'
$icon = [System.Drawing.Image]::FromFile((Join-Path $Root 'assets\app-icon\hicolor\256x256\apps\ai.storyteller.photocraft.png'))
$bmp = New-Object System.Drawing.Bitmap 164, 314, ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
try {
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $g.Clear([System.Drawing.ColorTranslator]::FromHtml('#1b1f26'))
  $stripe = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml('#2f7bf5'))
  $g.FillRectangle($stripe, 160, 0, 4, 314)
  $stripe.Dispose()
  $crop = 24; $scale = 124 / $icon.Width
  $src = New-Object System.Drawing.RectangleF $crop, $crop, ($icon.Width - 2 * $crop), ($icon.Height - 2 * $crop)
  $dst = New-Object System.Drawing.RectangleF (18 + $crop * $scale), (64 + $crop * $scale), ($src.Width * $scale), ($src.Height * $scale)
  $g.DrawImage($icon, $dst, $src, [System.Drawing.GraphicsUnit]::Pixel)
} finally { $g.Dispose(); $icon.Dispose() }
$bmp.Save($WelcomeBmp, [System.Drawing.Imaging.ImageFormat]::Bmp)
$bmp.Dispose()

$Out = Join-Path $Root 'dist\nsis'
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Setup = Join-Path $Out "layershop-$Version-windows-x64-setup.exe"
$MakeNsis = Find-MakeNsis
Write-Output "==> $MakeNsis"
& $MakeNsis /V2 /INPUTCHARSET UTF8 "/DVERSION=$Version" "/DVERSION_NUMERIC=$Numeric" "/DPAYLOAD=$Payload" `
  "/DICON=$(Join-Path $Root 'assets\app-icon\photocraft.ico')" "/DWELCOMEBMP=$WelcomeBmp" "/DOUTFILE=$Setup" `
  (Join-Path $PSScriptRoot 'layershop.nsi')
if ($LASTEXITCODE -ne 0) { throw "makensis failed with exit code $LASTEXITCODE" }
& (Join-Path $PSScriptRoot 'sign.ps1') $Setup

$Sha = (Get-FileHash $Setup -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Output ''
Write-Output "LayerShop $Version setup: $Setup"
Write-Output ('{0:N1} MB, SHA-256 {1}' -f ((Get-Item $Setup).Length / 1MB), $Sha)

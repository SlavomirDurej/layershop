<#
.SYNOPSIS
  Build LayerShop's Windows release files on this PC: the MSI installer, the portable zip and
  SHA256SUMS.txt, in dist\release.

.DESCRIPTION
  What ProjectManager's "Build installers (Windows x64)" runs; layershop-publish.ps1 then puts
  the files on GitHub. It wraps package.ps1 (the build and the packaging) with what a PC needs
  for it:

  - WiX 5.0.2. Used from PATH when it is there. Otherwise its official NuGet package (the file
    `dotnet tool install wix` downloads, pinned here by SHA-256) is unpacked into
    %LOCALAPPDATA%\LayerShop-build\wix-5.0.2 and run on the installed .NET runtime, so no .NET
    SDK is needed. Its UI and Util extensions are added when missing.
  - craft-fonts: used when a ..\craft-fonts checkout exists (official builds embed its fonts).
  - SHA256SUMS.txt for the files made (`sha256sum -c` reads it), and build-info.json: the
    version, the commit and whether the tree had uncommitted changes, which the publish step
    checks. The debug symbols zip package.ps1 also writes (for profilers) stays in distelease
    but is not a release file: it is in neither, so it is not published.

  The build itself takes several minutes (longer the first time).

.EXAMPLE
  pwsh -NoProfile -ExecutionPolicy Bypass -File packaging/windows/layershop-build.ps1
  pwsh -NoProfile -File packaging/windows/layershop-build.ps1 -SkipBuild   # repackage the last build
#>
param([switch] $SkipBuild)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Arch = 'x64'
$WixVersion = '5.0.2'
# SHA-256 of wix.5.0.2.nupkg as nuget.org serves it (its published SHA-512 checked 2026-10-16).
$WixSha256 = 'f30ef0c74e2a986126539c5780be93ac24e8136eaf723b1937b26272703ae173'

function Need([string] $Command, [string] $Hint) {
  if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) { throw "$Command was not found. $Hint" }
}
Need cargo 'Install Rust with the MSVC toolchain (https://rustup.rs).'
Need git 'Install Git.'
Need dotnet 'Install the .NET 8 runtime: WiX runs on it.'

# ---- WiX ---------------------------------------------------------------------------------------
if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
  $Tools = Join-Path $env:LOCALAPPDATA "LayerShop-build\wix-$WixVersion"
  $Dll = Join-Path $Tools 'pkg\tools\net6.0\any\wix.dll'
  if (-not (Test-Path $Dll)) {
    New-Item -ItemType Directory -Force -Path $Tools | Out-Null
    $Nupkg = Join-Path $Tools 'wix.nupkg'
    $Have = (Test-Path $Nupkg) -and (Get-FileHash $Nupkg -Algorithm SHA256).Hash.ToLowerInvariant() -eq $WixSha256
    if (-not $Have) {
      Write-Output "==> downloading WiX $WixVersion from nuget.org"
      Invoke-WebRequest -Uri "https://api.nuget.org/v3-flatcontainer/wix/$WixVersion/wix.$WixVersion.nupkg" -OutFile $Nupkg -UserAgent 'Photocraft-dev'
    }
    $Sha = (Get-FileHash $Nupkg -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Sha -ne $WixSha256) {
      Remove-Item -Force $Nupkg
      throw "the WiX package's SHA-256 is $Sha, expected ${WixSha256}: not using it"
    }
    Remove-Item -Recurse -Force (Join-Path $Tools 'pkg') -ErrorAction SilentlyContinue
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::ExtractToDirectory($Nupkg, (Join-Path $Tools 'pkg'))
  }
  $Bin = Join-Path $Tools 'bin'
  New-Item -ItemType Directory -Force -Path $Bin | Out-Null
  Set-Content -Path (Join-Path $Bin 'wix.cmd') -Value "@dotnet `"$Dll`" %*" -Encoding ascii
  $env:PATH = "$Bin;$env:PATH"
  Write-Output "using WiX $WixVersion from $Tools"
}
$Extensions = Join-Path $env:USERPROFILE '.wix\extensions'
foreach ($Ext in 'WixToolset.UI.wixext', 'WixToolset.Util.wixext') {
  if (-not (Test-Path (Join-Path $Extensions "$Ext\$WixVersion"))) {
    Write-Output "==> wix extension add -g $Ext/$WixVersion"
    wix extension add -g "$Ext/$WixVersion"
    if ($LASTEXITCODE -ne 0) { throw "adding the WiX extension $Ext failed" }
  }
}

# ---- fonts ------------------------------------------------------------------------------------
if (-not $env:CRAFT_FONTS_DIR) {
  $Fonts = Join-Path (Split-Path $Root) 'craft-fonts'
  if (Test-Path (Join-Path $Fonts 'fonts')) {
    $env:CRAFT_FONTS_DIR = $Fonts
  } else {
    Write-Warning "no ..\craft-fonts checkout: this build lacks the fonts official releases embed (Japanese)"
  }
}

# ---- build and package -------------------------------------------------------------------------
$Version = $null
$InPackage = $false
foreach ($Line in Get-Content (Join-Path $Root 'Cargo.toml')) {
  if ($Line -match '^\s*\[') { $InPackage = ($Line.Trim() -eq '[workspace.package]'); continue }
  if ($InPackage -and $Line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
$Dist = Join-Path $Root 'dist\release'
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

$Commit = (git -C $Root rev-parse HEAD).Trim()
$Dirty = [bool](git -C $Root status --porcelain --untracked-files=no)
if ($Dirty) { Write-Warning 'the working tree has uncommitted changes: these files can be tested, not published' }

# Nothing already in dist\release is touched until the new files are there: a build that fails
# leaves the last good one as it was.
$Started = (Get-Date).AddSeconds(-2)
$Package = Join-Path $PSScriptRoot 'package.ps1'
if ($SkipBuild) { & $Package -Arch $Arch -SkipBuild } else { & $Package -Arch $Arch }

# This build's files: the ones written since it started. The release files are all of them but the
# debug symbols.
$Made = @(Get-ChildItem -Path $Dist -File | Where-Object { $_.Name -like "layershop-$Version-windows-$Arch*" -and $_.LastWriteTime -ge $Started } | Sort-Object Name)
$Files = @($Made | Where-Object { $_.Name -notlike '*-symbols.zip' })
if (-not $Files) { throw "package.ps1 made no layershop-$Version-windows-$Arch files in $Dist" }
# Any other release file there is an older build's: it goes, so only this build can be published
# (notes written by hand stay).
Get-ChildItem -Path $Dist -File | Where-Object { $_.Name -like 'layershop-*' -and $Made.Name -notcontains $_.Name } | Remove-Item -Force
$Sums = $Files | ForEach-Object { '{0} *{1}' -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.Name }
# LF line ends, as sha256sum writes and reads them.
[IO.File]::WriteAllText((Join-Path $Dist 'SHA256SUMS.txt'), (($Sums -join "`n") + "`n"))
[ordered]@{
  version = $Version
  commit = $Commit
  dirty = $Dirty
  fonts = [bool]$env:CRAFT_FONTS_DIR
  built = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
  files = @($Files | ForEach-Object { $_.Name })
} | ConvertTo-Json | Set-Content -Path (Join-Path $Dist 'build-info.json') -Encoding utf8

Write-Output ''
Write-Output "LayerShop $Version for Windows $Arch, from $($Commit.Substring(0, 8))$(if ($Dirty) { ' with uncommitted changes' })"
Get-ChildItem -Path $Dist -File | Format-Table Name, @{ Name = 'MB'; Expression = { [math]::Round($_.Length / 1MB, 1) } }
Write-Output "in $Dist"

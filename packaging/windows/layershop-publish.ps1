<#
.SYNOPSIS
  Publish the files layershop-build.ps1 made as a GitHub release of the fork.

.DESCRIPTION
  What ProjectManager's "Publish release to GitHub" actions run. The release is tagged
  layershop-v<version>-preview.<N> (upstream owns the plain v<version> tags), N one more than the
  fork's last preview of this version unless -Preview says otherwise, and titled
  "LayerShop <version> Preview <N>". Its files: the MSI, the portable zip and SHA256SUMS.txt.

  Before anything is sent it checks, and stops with what to do when one fails:
  - gh is installed and signed in;
  - the files are there, were built from the commit being released, without uncommitted changes;
  - that commit is on the fork (pushed), so the tag points at code people can see;
  - the tag is not taken.

  Notes: dist\release\RELEASE-NOTES.md (or -NotesFile) when there is one, written for the
  release; otherwise a standard text listing the fork's commits since its last release.

  -Draft makes a draft, seen only by the repo's owner until it is published on GitHub; without
  it the release is public at once, as a pre-release.

  A public release then goes on the website (the layer-shop-website checkout beside this repo, or
  -Website): its download page is pointed at the new release (version, date, direct links to the
  MSI, the zip and the checksums; scripts/sync-release.mjs writes release.json), and the site is
  built and deployed to production (npm run deploy). That checkout is checked before anything is
  published: it must be there, with Node and its FTP settings (.env.deploy.local). -NoWebsite
  leaves the site alone. A draft never touches it: after publishing the draft on GitHub, run
  `npm run release:sync` and `npm run deploy` in the website checkout.

.EXAMPLE
  pwsh -NoProfile -File packaging/windows/layershop-publish.ps1 -Draft
  pwsh -NoProfile -File packaging/windows/layershop-publish.ps1 -Preview 3
  pwsh -NoProfile -File packaging/windows/layershop-publish.ps1 -NoWebsite
#>
param(
  [int] $Preview = 0,
  [switch] $Draft,
  [string] $Repo = 'SlavomirDurej/layershop',
  [string] $NotesFile,
  [string] $Website = (Join-Path $PSScriptRoot '..\..\..\layer-shop-website'),
  [switch] $NoWebsite
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Dist = Join-Path $Root 'dist\release'

function Stop-With([string] $Why) {
  Write-Output ''
  Write-Output "Not published: $Why"
  exit 1
}

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { Stop-With 'the GitHub CLI (gh) is not installed: https://cli.github.com' }
gh auth status --hostname github.com *> $null
if ($LASTEXITCODE -ne 0) { Stop-With 'gh is not signed in to GitHub. Run: gh auth login' }

# ---- the website can be updated afterwards ------------------------------------------------------
$UpdateSite = -not ($Draft -or $NoWebsite)
if ($UpdateSite) {
  $Site = if (Test-Path $Website) { (Resolve-Path $Website).Path } else { $null }
  if (-not $Site -or -not (Test-Path (Join-Path $Site 'scripts\sync-release.mjs'))) {
    Stop-With "the website checkout is not at $Website (with scripts\sync-release.mjs). Pass -Website <folder>, or -NoWebsite to publish without updating the site."
  }
  if (-not (Get-Command node -ErrorAction SilentlyContinue)) { Stop-With 'Node is not installed, so the website cannot be built. Install it, or pass -NoWebsite.' }
  if (-not ($env:FTP_HOST -or (Test-Path (Join-Path $Site '.env.deploy.local')))) {
    Stop-With "the website has no FTP settings ($Site\.env.deploy.local), so it cannot be deployed. Add them, or pass -NoWebsite."
  }
}

# ---- the files ---------------------------------------------------------------------------------
$InfoFile = Join-Path $Dist 'build-info.json'
if (-not (Test-Path $InfoFile)) { Stop-With 'there are no release files. Run Build installers (Windows x64) first.' }
$Info = Get-Content $InfoFile -Raw | ConvertFrom-Json
$Head = (git -C $Root rev-parse HEAD).Trim()
if ($Info.dirty) { Stop-With 'the files were built from uncommitted changes. Commit them, push, and build again.' }
if ($Info.commit -ne $Head) {
  Stop-With "the files were built from $($Info.commit.Substring(0, 8)), but the checkout is at $($Head.Substring(0, 8)). Build again."
}
$Assets = @($Info.files | ForEach-Object { Join-Path $Dist $_ }) + (Join-Path $Dist 'SHA256SUMS.txt')
foreach ($File in $Assets) {
  if (-not (Test-Path $File)) { Stop-With "$File is missing. Build again." }
}
# The checksums still match the files (nothing was swapped since the build).
foreach ($Line in Get-Content (Join-Path $Dist 'SHA256SUMS.txt')) {
  if ($Line -notmatch '^([0-9a-f]{64}) \*(.+)$') { continue }
  $Actual = (Get-FileHash (Join-Path $Dist $Matches[2]) -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($Actual -ne $Matches[1]) { Stop-With "$($Matches[2]) no longer matches SHA256SUMS.txt. Build again." }
}

# ---- the commit is on the fork ---------------------------------------------------------------
git -C $Root fetch --quiet origin
if ($LASTEXITCODE -ne 0) { Stop-With 'could not reach the fork (git fetch origin failed).' }
$OnFork = git -C $Root branch -r --contains $Head | Where-Object { $_.Trim() -like 'origin/*' }
if (-not $OnFork) { Stop-With "commit $($Head.Substring(0, 8)) is not on the fork yet. Push it first (Deploy: Push main to fork)." }

# ---- the tag ----------------------------------------------------------------------------------
$Version = $Info.version
$Prefix = "layershop-v$Version-preview."
# Tags on the fork, and releases (a draft has no tag until it is published).
$Names = @(git -C $Root ls-remote --tags origin "refs/tags/$Prefix*") + @(gh release list --repo $Repo --limit 200 --json tagName --jq '.[].tagName')
$Taken = @($Names | ForEach-Object {
    if ($_ -match "$([regex]::Escape($Prefix))(\d+)$") { [int]$Matches[1] }
  } | Sort-Object -Unique)
if ($Preview -le 0) { $Preview = 1 + (($Taken + 0) | Measure-Object -Maximum).Maximum }
if ($Taken -contains $Preview) { Stop-With "the tag $Prefix$Preview is already on the fork. Use another -Preview number." }
$Tag = "$Prefix$Preview"
$Title = "LayerShop $Version Preview $Preview"

# ---- the notes --------------------------------------------------------------------------------
if (-not $NotesFile) {
  $Written = Join-Path $Dist 'RELEASE-NOTES.md'
  if (Test-Path $Written) { $NotesFile = $Written }
}
if (-not $NotesFile) {
  # The fork's own commits since its last release (upstream merges show as one line each).
  $Previous = git -C $Root describe --tags --abbrev=0 --match 'layershop-v*' $Head 2>$null
  $Range = if ($Previous) { "$Previous..$Head" } else { $Head }
  $Changes = @(git -C $Root log --first-parent --pretty=format:'- %s' -n 40 $Range)
  $Msi = $Info.files | Where-Object { $_ -like '*.msi' } | Select-Object -First 1
  $Zip = $Info.files | Where-Object { $_ -like '*-portable.zip' } | Select-Object -First 1
  $Notes = @(
    "**LayerShop** is a Photoshop-style image editor with generative AI built in. It is based on [PhotoCraft](https://github.com/storytold/photocraft) by the ArtCraft team, and this preview includes PhotoCraft $Version."
    ''
    'It runs on **Windows 10 and 11 (x64)** only.'
    ''
    $(if ($Previous) { "### What's new since $Previous" } else { "### What's in it" })
    $(if ($Changes) { $Changes } else { '- See the commits on the `main` branch.' })
    ''
    '### Downloads'
    '| File | What it is |'
    '|---|---|'
    "| ``$Msi`` | Installer (Start Menu shortcut, file associations). Upgrades an earlier preview |"
    "| ``$Zip`` | Portable: unzip and run ``photocraft.exe``. Settings stay in the folder |"
    '| `SHA256SUMS.txt` | Checksums |'
    ''
    '### Known limitations'
    '- **The files are not code-signed**, so Windows SmartScreen may say "Windows protected your PC". Click **More info › Run anyway**. To check a download, compare its SHA-256 with `SHA256SUMS.txt`.'
    '- Model downloads are large, several GB each. Some recommended models are licensed for **non-commercial use only**: check a model''s licence before using its images commercially.'
    $(if (-not $Info.fonts) { '- The Japanese fonts that official PhotoCraft releases embed are not in this build.' })
    ''
    "Please report problems on [Issues](https://github.com/$Repo/issues)."
  ) | Where-Object { $null -ne $_ }
  $NotesFile = Join-Path ([IO.Path]::GetTempPath()) "layershop-notes-$Tag.md"
  [IO.File]::WriteAllText($NotesFile, (($Notes -join "`n") + "`n"))
}

# ---- publish ----------------------------------------------------------------------------------
Write-Output "$Title ($Tag) from $($Head.Substring(0, 8)) to $Repo, $(if ($Draft) { 'as a draft' } else { 'public, as a pre-release' })"
$GhArgs = @('release', 'create', $Tag) + $Assets + @('--repo', $Repo, '--target', $Head, '--title', $Title, '--notes-file', $NotesFile, '--prerelease')
if ($Draft) { $GhArgs += '--draft' }
gh @GhArgs
if ($LASTEXITCODE -ne 0) { Stop-With 'gh release create failed (see above).' }
git -C $Root fetch --quiet --tags origin
Write-Output ''
Write-Output "Released: https://github.com/$Repo/releases/tag/$Tag$(if ($Draft) { ' (a draft: publish it on GitHub when it reads right)' })"

# ---- the website ------------------------------------------------------------------------------
if (-not $UpdateSite) { exit 0 }
function Stop-Site([string] $Why) {
  Write-Output ''
  Write-Output "The release is published, but the website was not updated: $Why"
  Write-Output "To retry: cd `"$Site`"; npm run release:sync -- $Tag; npm run deploy"
  exit 1
}
Write-Output ''
Write-Output "Website ($Site): download page -> $Tag"
Push-Location $Site
try {
  # The release API can lag a moment behind gh, so try a few times.
  for ($Try = 1; $Try -le 5; $Try++) {
    node scripts/sync-release.mjs $Tag
    if ($LASTEXITCODE -eq 0) { break }
    if ($Try -lt 5) { Start-Sleep -Seconds (3 * $Try) }
  }
  if ($LASTEXITCODE -ne 0) { Stop-Site 'release.json could not be written (see above).' }
  node scripts/deploy.mjs
  if ($LASTEXITCODE -ne 0) { Stop-Site 'the deploy failed (see above).' }
} finally {
  Pop-Location
}
Write-Output ''
Write-Output "Done: the download page offers $Title. release.json in the website checkout changed; commit it there."

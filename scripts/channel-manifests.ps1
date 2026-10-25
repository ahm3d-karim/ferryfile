# Writes the winget and Scoop manifests for a published GitHub release, using
# the release's own .sha256 assets. Needs the GitHub CLI (gh).
#
#   ./scripts/channel-manifests.ps1 -Tag v0.1.0
#
# Output:
#   packaging/winget/manifests/f/Ferryfile/Ferryfile/<version>/*.yaml  (submit to microsoft/winget-pkgs)
#   packaging/scoop/ferryfile.json                                     (copy into the Scoop bucket)
param([Parameter(Mandatory)][string]$Tag)
$ErrorActionPreference = 'Stop'

$repo = 'ahm3d-karim/ferryfile'
$id = 'Ferryfile.Ferryfile'
$schema = '1.12.0'
$version = $Tag.TrimStart('v')
$root = Split-Path -Parent $PSScriptRoot
$base = "https://github.com/$repo/releases/download/$Tag"
$setup = "Ferryfile_${version}_x64-setup.exe"
$portable = "Ferryfile_${version}_x64-portable.zip"

# UTF-8 without a BOM: winget validation and Scoop both prefer it.
function Write-Text($path, $text) { [IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding $false)) }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ferryfile-manifests-" + [guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    gh release download $Tag --repo $repo --pattern '*.sha256' --dir $tmp
    if ($LASTEXITCODE) { throw "Couldn't download checksums for $Tag (is the release published?)" }
    $hashOf = { param($file) (Get-Content (Join-Path $tmp "$file.sha256") -Raw).Split(' ')[0].Trim() }
    $setupHash = (& $hashOf $setup).ToUpper()
    $portableHash = & $hashOf $portable
    $date = (gh release view $Tag --repo $repo --json publishedAt -q .publishedAt).Substring(0, 10)
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# --- winget -----------------------------------------------------------------
$wingetDir = Join-Path $root "packaging\winget\manifests\f\Ferryfile\Ferryfile\$version"
New-Item -ItemType Directory -Force $wingetDir | Out-Null
$header = { param($type) "# yaml-language-server: `$schema=https://aka.ms/winget-manifest.$type.$schema.schema.json`n" }

(& $header 'version') + @"
PackageIdentifier: $id
PackageVersion: $version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: $schema
"@ | ForEach-Object { Write-Text (Join-Path $wingetDir "$id.yaml") $_ }

(& $header 'installer') + @"
PackageIdentifier: $id
PackageVersion: $version
InstallerType: nullsoft
Scope: user
InstallModes:
- interactive
- silent
UpgradeBehavior: install
ReleaseDate: $date
Installers:
- Architecture: x64
  InstallerUrl: $base/$setup
  InstallerSha256: $setupHash
ManifestType: installer
ManifestVersion: $schema
"@ | ForEach-Object { Write-Text (Join-Path $wingetDir "$id.installer.yaml") $_ }

(& $header 'defaultLocale') + @"
PackageIdentifier: $id
PackageVersion: $version
PackageLocale: en-US
Publisher: Ferryfile
Author: Ahmad Karim
PublisherUrl: https://github.com/ahm3d-karim
PublisherSupportUrl: https://github.com/$repo/issues
PackageName: Ferryfile
PackageUrl: https://github.com/$repo
License: MIT
LicenseUrl: https://github.com/$repo/blob/main/LICENSE
Copyright: Copyright (c) 2026 Ahmad Karim
ShortDescription: Offline drag-and-drop file converter for Windows.
Description: Start dragging a file in Explorer and a drawer slides in from the screen edge. Drop the file to convert images, audio, video, PDFs, documents and spreadsheets locally, run saved recipes, or keep files on a floating shelf.
Moniker: ferryfile
Tags:
- converter
- file-converter
- drag-and-drop
- pdf
- video
- audio
- image
- markdown
ReleaseNotesUrl: https://github.com/$repo/releases/tag/$Tag
ManifestType: defaultLocale
ManifestVersion: $schema
"@ | ForEach-Object { Write-Text (Join-Path $wingetDir "$id.locale.en-US.yaml") $_ }

# --- Scoop ------------------------------------------------------------------
$scoop = [ordered]@{
    version      = $version
    description  = 'Offline drag-and-drop file converter for Windows'
    homepage     = "https://github.com/$repo"
    license      = 'MIT'
    notes        = 'Ferryfile runs in the system tray. Drag a file from Explorer to open the drawer. Run `ferryfile --help` for the command line.'
    architecture = [ordered]@{ '64bit' = [ordered]@{ url = "$base/$portable"; hash = $portableHash } }
    bin          = 'ferryfile.exe'
    shortcuts    = @(, @('ferryfile.exe', 'Ferryfile'))
    checkver     = [ordered]@{ github = "https://github.com/$repo" }
    autoupdate   = [ordered]@{
        architecture = [ordered]@{
            '64bit' = [ordered]@{
                url  = "https://github.com/$repo/releases/download/v`$version/Ferryfile_`$version_x64-portable.zip"
                hash = [ordered]@{ url = '$url.sha256' }
            }
        }
    }
}
$scoopDir = Join-Path $root 'packaging\scoop'
New-Item -ItemType Directory -Force $scoopDir | Out-Null
Write-Text (Join-Path $scoopDir 'ferryfile.json') ($scoop | ConvertTo-Json -Depth 6)

Write-Host "winget manifests: $wingetDir"
Write-Host "Scoop manifest:   $(Join-Path $scoopDir 'ferryfile.json')"

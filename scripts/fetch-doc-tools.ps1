# Downloads Ferryfile's document tools into src-tauri/binaries:
#   - pandoc (latest Windows release, github.com/jgm/pandoc)  -> pandoc-x86_64-pc-windows-msvc.exe (sidecar)
#   - pdfium (bblanchon/pdfium-binaries, pinned to the API pdfium-render targets) -> pdfium.dll
#
# pandoc is GPL-2.0-or-later and ships unmodified as a separate executable;
# pdfium is BSD-3-Clause / Apache-2.0.
$ErrorActionPreference = 'Stop'

$pdfiumTag = 'chromium/7881'   # keep in sync with pdfium-render's `pdfium_latest`
$root = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $root 'src-tauri\binaries'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("ferryfile-doctools-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $bin, $tmp | Out-Null

try {
    # pandoc
    $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/jgm/pandoc/releases/latest' -UseBasicParsing
    $asset = $release.assets | Where-Object { $_.name -like '*windows-x86_64.zip' } | Select-Object -First 1
    if (-not $asset) { throw 'No Windows x86_64 pandoc asset in the latest release' }
    Write-Host "Downloading $($asset.browser_download_url)"
    $zip = Join-Path $tmp 'pandoc.zip'
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zip -UseBasicParsing
    Expand-Archive -Path $zip -DestinationPath (Join-Path $tmp 'pandoc')
    $exe = Get-ChildItem -Path (Join-Path $tmp 'pandoc') -Recurse -Filter 'pandoc.exe' | Select-Object -First 1
    Copy-Item $exe.FullName (Join-Path $bin 'pandoc-x86_64-pc-windows-msvc.exe') -Force
    Write-Host "Installed pandoc $($release.tag_name)"

    # pdfium
    $url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$([uri]::EscapeDataString($pdfiumTag))/pdfium-win-x64.tgz"
    Write-Host "Downloading $url"
    $tgz = Join-Path $tmp 'pdfium.tgz'
    Invoke-WebRequest -Uri $url -OutFile $tgz -UseBasicParsing
    $pdfDir = Join-Path $tmp 'pdfium'
    New-Item -ItemType Directory -Force $pdfDir | Out-Null
    tar -xzf $tgz -C $pdfDir
    Copy-Item (Join-Path $pdfDir 'bin\pdfium.dll') (Join-Path $bin 'pdfium.dll') -Force
    Write-Host "Installed pdfium ($pdfiumTag)"
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

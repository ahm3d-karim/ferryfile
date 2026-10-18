# Packs the release build into a portable zip (no installer): ferryfile.exe plus
# the bundled tools, exactly as Tauri lays them out next to the exe. Scoop and
# the npm launcher ship this zip. Run after `npx tauri build`.
#
# Writes to src-tauri/target/release/bundle/portable/:
#   Ferryfile_<version>_x64-portable.zip and a matching .sha256 file.
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root 'src-tauri\target\release'
$version = (Get-Content (Join-Path $root 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version
$outDir = Join-Path $release 'bundle\portable'
$zip = Join-Path $outDir "Ferryfile_${version}_x64-portable.zip"

$files = @(
    (Join-Path $release 'ferryfile.exe'),
    (Join-Path $release 'ffmpeg.exe'),
    (Join-Path $release 'pandoc.exe'),
    (Join-Path $release 'pdfium.dll'),
    (Join-Path $root 'THIRD_PARTY_NOTICES.md'),
    (Join-Path $root 'LICENSE')
)
foreach ($f in $files) {
    if (-not (Test-Path $f)) { throw "Missing $f. Run 'npx tauri build' first." }
}

New-Item -ItemType Directory -Force $outDir | Out-Null
Remove-Item $zip, "$zip.sha256" -ErrorAction SilentlyContinue
Compress-Archive -Path $files -DestinationPath $zip -CompressionLevel Optimal

foreach ($artifact in @($zip) + @(Get-ChildItem (Join-Path $release 'bundle\nsis\*-setup.exe') | ForEach-Object FullName)) {
    $hash = (Get-FileHash $artifact -Algorithm SHA256).Hash.ToLower()
    "$hash  $([IO.Path]::GetFileName($artifact))" | Set-Content -NoNewline -Encoding ascii "$artifact.sha256"
    Write-Host "$hash  $([IO.Path]::GetFileName($artifact))"
}

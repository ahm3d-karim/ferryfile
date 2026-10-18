# Downloads an LGPL ffmpeg build (BtbN/FFmpeg-Builds, win64) and installs it as
# Skiff's Tauri sidecar: src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe
#
# LGPL builds omit GPL encoders such as x264; Skiff falls back to h264_mf
# (Windows Media Foundation) for H.264.
$ErrorActionPreference = 'Stop'

$url = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-lgpl.zip'
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("skiff-ffmpeg-" + [guid]::NewGuid())

New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    $zip = Join-Path $tmp 'ffmpeg.zip'
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    Expand-Archive -Path $zip -DestinationPath $tmp
    $exe = Get-ChildItem -Path $tmp -Recurse -Filter 'ffmpeg.exe' | Select-Object -First 1
    if (-not $exe) { throw 'ffmpeg.exe not found in the archive' }
    New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
    Copy-Item $exe.FullName $dest -Force
    Write-Host "Installed $dest"
    & $dest -hide_banner -version | Select-Object -First 1
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

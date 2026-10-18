# Code-signs one file; Tauri calls this for every binary it bundles.
#
# Configure ONE of these (environment variables or GitHub secrets):
#   SKIFF_SIGN_THUMBPRINT                     certificate already in the Windows cert store
#   SKIFF_SIGN_PFX + SKIFF_SIGN_PFX_PASSWORD  base64-encoded .pfx and its password
# With neither set, the file is left unsigned so local builds still work.
param([Parameter(Mandatory)][string]$File)
$ErrorActionPreference = 'Stop'

$timestamp = 'http://timestamp.digicert.com'
$name = [IO.Path]::GetFileName($File)

if (-not $env:SKIFF_SIGN_THUMBPRINT -and -not $env:SKIFF_SIGN_PFX) {
    Write-Host "skiff-sign: no certificate configured, leaving $name unsigned"
    exit 0
}

$signtool = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source
if (-not $signtool) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $signtool) { throw 'signtool.exe not found; install the Windows SDK' }

$common = @('sign', '/fd', 'sha256', '/tr', $timestamp, '/td', 'sha256', '/d', 'Skiff')
if ($env:SKIFF_SIGN_THUMBPRINT) {
    & $signtool @common /sha1 $env:SKIFF_SIGN_THUMBPRINT $File
} else {
    $pfx = Join-Path ([IO.Path]::GetTempPath()) ("skiff-sign-" + [guid]::NewGuid() + '.pfx')
    try {
        [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:SKIFF_SIGN_PFX))
        & $signtool @common /f $pfx /p $env:SKIFF_SIGN_PFX_PASSWORD $File
    } finally {
        Remove-Item $pfx -Force -ErrorAction SilentlyContinue
    }
}
if ($LASTEXITCODE) { throw "signtool failed for $name (exit $LASTEXITCODE)" }
Write-Host "skiff-sign: signed $name"

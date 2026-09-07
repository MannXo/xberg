# vendor-windows-native-closure.ps1
#
# Generic Windows counterpart to scripts/ci/vendor-native-closure.sh for
# archive-packaged artifacts (.zip / .tar.gz) that are NOT Python wheels (see
# vendor-windows-wheel-closure.ps1 for the RECORD-aware wheel variant).
#
# Copies every *.dll from $DllSourceDir into the directory (inside the
# archive) containing the file matching $NativeGlob, then repacks the archive
# in place, preserving its original format. Used to vendor the ONNX Runtime
# DLL closure into archives produced by opaque external actions (PHP PIE
# packages, Elixir NIF tarballs) where this repo cannot control the packaging
# step directly, only its input/output.
#
# Usage:
#   vendor-windows-native-closure.ps1 <Archive> <DllSourceDir> <NativeGlob>

param(
  [Parameter(Mandatory = $true)][string]$Archive,
  [Parameter(Mandatory = $true)][string]$DllSourceDir,
  [Parameter(Mandatory = $true)][string]$NativeGlob
)

$ErrorActionPreference = "Stop"

function Write-Log([string]$Message) {
  Write-Host "vendor-windows-native-closure: $Message"
}

if (-not (Test-Path $Archive)) { throw "archive not found: '$Archive'" }
if (-not (Test-Path $DllSourceDir)) { throw "DLL source dir not found: '$DllSourceDir'" }

$archivePath = (Resolve-Path $Archive).Path
$isTarGz = $archivePath -match '\.tar\.gz$|\.tgz$'
$isZip = $archivePath -match '\.zip$'
if (-not ($isTarGz -or $isZip)) { throw "unsupported archive type: '$archivePath' (expected .zip or .tar.gz)" }

$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ("vendor-native-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $workDir -Force | Out-Null

try {
  if ($isZip) {
    Expand-Archive -Path $archivePath -DestinationPath $workDir -Force
  }
  else {
    & tar -xzf $archivePath -C $workDir
    if ($LASTEXITCODE -ne 0) { throw "tar extraction failed for '$archivePath'" }
  }

  $natives = @(Get-ChildItem -Path $workDir -Recurse -File -Filter $NativeGlob -ErrorAction SilentlyContinue)
  if ($natives.Count -eq 0) {
    throw "no native file matching '$NativeGlob' found in archive '$archivePath'"
  }
  $targetDirs = $natives | ForEach-Object { $_.Directory.FullName } | Select-Object -Unique

  $dlls = @(Get-ChildItem -Path $DllSourceDir -Filter "*.dll" -File -ErrorAction SilentlyContinue)
  if ($dlls.Count -eq 0) { throw "no *.dll files found under DLL source dir '$DllSourceDir'" }

  foreach ($dir in $targetDirs) {
    foreach ($dll in $dlls) {
      $destPath = Join-Path $dir $dll.Name
      if (Test-Path $destPath) {
        Write-Log "skipping $($dll.Name), already present at $destPath"
        continue
      }
      Copy-Item -Path $dll.FullName -Destination $destPath -Force
      Write-Log "vendored $($dll.Name) into $dir"
    }
  }

  if ($isZip) {
    $repacked = Join-Path ([System.IO.Path]::GetTempPath()) ("vendor-native-out-" + [Guid]::NewGuid().ToString("N") + ".zip")
    if (Test-Path $repacked) { Remove-Item $repacked -Force }
    Compress-Archive -Path (Join-Path $workDir '*') -DestinationPath $repacked -Force
    Move-Item -Path $repacked -Destination $archivePath -Force
  }
  else {
    Push-Location $workDir
    try {
      $entries = Get-ChildItem -Path $workDir -Name
      & tar -czf $archivePath @entries
      if ($LASTEXITCODE -ne 0) { throw "tar repack failed for '$archivePath'" }
    }
    finally {
      Pop-Location
    }
  }
  Write-Log "repacked $archivePath"
}
finally {
  if (Test-Path $workDir) { Remove-Item -Recurse -Force $workDir }
}

# The .zip branch above repacks with Compress-Archive and runs no native command, so this script
# can return without ever setting $LASTEXITCODE. Callers in publish.yaml gate on it, and an unset
# $LASTEXITCODE compares as `$null -ne 0` -> true, so every Windows PHP archive threw immediately
# after being vendored and repacked successfully -- which is why v1.1.0 and v1.1.1 shipped no PHP
# assets (xberg-io/xberg#1585). Sibling scripts vendor-windows-wheel-closure.ps1 and
# verify-windows-dll-closure.ps1 already set this contract explicitly; this one was missed. ~keep
exit 0

#requires -Version 5
<#
.SYNOPSIS
  Build a self-contained Windows bundle of nucleus-server (exe + ONNX Runtime).
.EXAMPLE
  pwsh packaging/build-release.ps1 -Version 0.1.0
#>
param(
  [string]$Version = "0.1.0",
  [string]$OutDir = "dist",
  [switch]$Gpu
)
$ErrorActionPreference = "Stop"
$env:CARGO_INCREMENTAL = "0"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$features = @()
if ($Gpu) { $features = @("--features", "gpu") }
Write-Host "Building nucleus-server (release)$(if($Gpu){' +gpu'})..."
# cargo logs progress to stderr; under $ErrorActionPreference='Stop' that would
# be treated as a terminating error, so relax it around the native call and rely
# on the exit code instead.
$prevEAP = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& cargo build --release -p nucleus-server @features
$code = $LASTEXITCODE
$ErrorActionPreference = $prevEAP
if ($code -ne 0) { throw "cargo build failed (exit $code)" }

$stage = Join-Path $OutDir "nucleus-$Version-windows-x64"
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item "target/release/nucleus-server.exe" $stage -Force

# ONNX Runtime native libraries, if `ort` placed any next to the binary. On
# Windows ort statically links ONNX Runtime into the exe, so there is usually
# none — the binary is self-contained.
$dlls = Get-ChildItem "target/release" -Filter "onnxruntime*.dll" -ErrorAction SilentlyContinue
if ($dlls) {
  $dlls | ForEach-Object { Copy-Item $_.FullName $stage -Force }
  Write-Host "Bundled $($dlls.Count) ONNX Runtime DLL(s)."
} else {
  Write-Host "No separate ONNX Runtime DLL (statically linked into the exe)."
}

Copy-Item "packaging/README.md" (Join-Path $stage "README.md") -Force
Copy-Item "packaging/install.ps1" (Join-Path $stage "install.ps1") -Force

$zip = Join-Path $OutDir "nucleus-$Version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path "$stage/*" -DestinationPath $zip
Write-Host "Bundle: $zip"

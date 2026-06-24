#requires -Version 5
<#
.SYNOPSIS
  Build a Windows bundle of Nucleus in embedded (DLL) mode: nucleus.dll + import
  lib + C header + the C# P/Invoke binding.
.EXAMPLE
  pwsh packaging/build-dll.ps1 -Version 0.1.0
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
Write-Host "Building nucleus-ffi (release)$(if($Gpu){' +gpu'})..."
# cargo logs progress to stderr; under 'Stop' that reads as a terminating error,
# so relax it around the native call and rely on the exit code instead.
$prevEAP = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& cargo build --release -p nucleus-ffi @features
$code = $LASTEXITCODE
$ErrorActionPreference = $prevEAP
if ($code -ne 0) { throw "cargo build failed (exit $code)" }

$stage = Join-Path $OutDir "nucleus-dll-$Version-windows-x64"
New-Item -ItemType Directory -Force -Path $stage | Out-Null

# The DLL and its import library (for C/C++ link-time references). We ship only
# the import lib (`nucleus.dll.lib`), not the huge `nucleus.lib` staticlib — that
# is only useful for statically linking the whole engine, a separate use case.
Copy-Item "target/release/nucleus.dll" $stage -Force
$implib = "target/release/nucleus.dll.lib"
if (Test-Path $implib) { Copy-Item $implib $stage -Force }

# On Windows ort statically links ONNX Runtime into the DLL; bundle any separate
# runtime DLL only if one was actually produced (e.g. a GPU provider).
$dlls = Get-ChildItem "target/release" -Filter "onnxruntime*.dll" -ErrorAction SilentlyContinue
if ($dlls) {
  $dlls | ForEach-Object { Copy-Item $_.FullName $stage -Force }
  Write-Host "Bundled $($dlls.Count) ONNX Runtime DLL(s)."
} else {
  Write-Host "No separate ONNX Runtime DLL (statically linked into nucleus.dll)."
}

# C header + docs.
Copy-Item "crates/ffi/include/nucleus.h" $stage -Force
Copy-Item "packaging/dll-README.md" (Join-Path $stage "README.md") -Force

# C# P/Invoke binding (source — consumers add it as a project reference). Ship all
# of the binding's .cs files (NucleusNative.cs + Models.cs) plus the csproj.
$csDst = Join-Path $stage "csharp"
New-Item -ItemType Directory -Force -Path $csDst | Out-Null
Get-ChildItem "clients/csharp/Nucleus.Native" -Filter *.cs | ForEach-Object { Copy-Item $_.FullName $csDst -Force }
Copy-Item "clients/csharp/Nucleus.Native/Nucleus.Native.csproj" $csDst -Force

$zip = Join-Path $OutDir "nucleus-dll-$Version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path "$stage/*" -DestinationPath $zip
Write-Host "Bundle: $zip"

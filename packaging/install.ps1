#requires -Version 5
<#
.SYNOPSIS
  Install a nucleus-server Windows bundle: copy files, create the data dir, set
  machine environment variables, and (optionally) register a startup service via
  Task Scheduler. Run from inside the unzipped bundle, as Administrator.
.EXAMPLE
  pwsh install.ps1 -RegisterService
#>
param(
  [string]$Source = $PSScriptRoot,
  [string]$InstallDir = "$env:ProgramFiles\Nucleus",
  [string]$DataDir = "$env:ProgramData\Nucleus",
  [string]$ListenAddr = "127.0.0.1:8080",
  [switch]$RegisterService
)
$ErrorActionPreference = "Stop"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
  ).IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)
if (-not $isAdmin) { throw "Run this script as Administrator." }

$exe = Join-Path $Source "nucleus-server.exe"
if (-not (Test-Path $exe)) { throw "nucleus-server.exe not found in '$Source'." }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null

Copy-Item $exe $InstallDir -Force
Get-ChildItem $Source -Filter "onnxruntime*.dll" -ErrorAction SilentlyContinue |
  ForEach-Object { Copy-Item $_.FullName $InstallDir -Force }

# Persist configuration as machine environment variables (the service inherits them).
$envVars = @{
  NUCLEUS_ADDR             = $ListenAddr
  NUCLEUS_DB               = (Join-Path $DataDir "nucleus.redb")
  NUCLEUS_MODEL_CACHE      = (Join-Path $DataDir "models")
  NUCLEUS_INDEX_DIR        = (Join-Path $DataDir "indexes")
  NUCLEUS_ADMIN_TOKEN_FILE = (Join-Path $DataDir "admin_token.txt")
}
foreach ($k in $envVars.Keys) {
  [Environment]::SetEnvironmentVariable($k, $envVars[$k], "Machine")
}

Write-Host "Installed to $InstallDir; data in $DataDir"

if ($RegisterService) {
  $installedExe = Join-Path $InstallDir "nucleus-server.exe"
  $action = New-ScheduledTaskAction -Execute $installedExe
  $trigger = New-ScheduledTaskTrigger -AtStartup
  $principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
  $settings = New-ScheduledTaskSettingsSet -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
  Register-ScheduledTask -TaskName "Nucleus" -Action $action -Trigger $trigger `
    -Principal $principal -Settings $settings -Force | Out-Null
  Start-ScheduledTask -TaskName "Nucleus"
  Write-Host "Registered + started scheduled task 'Nucleus' (runs at startup)."
  Write-Host "Admin token will appear at: $($envVars.NUCLEUS_ADMIN_TOKEN_FILE)"
} else {
  Write-Host "Run it with:  & '$([IO.Path]::Combine($InstallDir,'nucleus-server.exe'))'"
  Write-Host "(open a new shell so the machine env vars are picked up)"
}

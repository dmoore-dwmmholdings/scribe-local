<#
.SYNOPSIS
  Install scribe `serve` + `worker` as always-on Windows services (auto-start on
  boot, auto-restart on crash) via NSSM, and pin the Postgres container to
  auto-restart. Run as Administrator from inside the deploy bundle.

  Prereqs: NSSM on PATH (winget install NSSM.NSSM  OR  choco install nssm),
  Docker Desktop running, and deploy\server.toml configured. Docker Desktop
  itself should be set to start on login (Settings -> General).

.EXAMPLE
  # In an elevated PowerShell, from the bundle root:
  .\scripts\install-service.ps1
  # Remove later:
  .\scripts\install-service.ps1 -Uninstall
#>
[CmdletBinding()]
param(
  [int]$DbPort   = 5433,
  [int]$ApiPort  = 8443,
  [string]$Config = "deploy\server.toml",
  [switch]$Uninstall
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$bin = Join-Path $root "scribe.exe"
$dbUrl = "postgres://scribe:scribe@localhost:$DbPort/scribe?sslmode=disable"
$services = @("scribe-serve", "scribe-worker")

# --- guards ------------------------------------------------------------------
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)
if (-not $admin) { throw "Run this in an ELEVATED PowerShell (Administrator)." }
$nssm = (Get-Command nssm -ErrorAction SilentlyContinue).Source
if (-not $nssm) { throw "nssm not found on PATH. Install it (winget install NSSM.NSSM  or  choco install nssm), open a new admin shell, and re-run." }

# --- uninstall ---------------------------------------------------------------
if ($Uninstall) {
  foreach ($s in $services) {
    if (Get-Service $s -ErrorAction SilentlyContinue) {
      & $nssm stop $s | Out-Null
      & $nssm remove $s confirm | Out-Null
      Write-Host "removed $s" -ForegroundColor Yellow
    }
  }
  return
}

if (-not (Test-Path $bin)) { throw "scribe.exe not found at $bin - run from the bundle root." }

# --- Postgres: start + pin to auto-restart -----------------------------------
Write-Host "[1/4] Postgres (pgvector) on :$DbPort ..." -ForegroundColor Cyan
$env:POSTGRES_PORT = "$DbPort"
docker compose up -d postgres | Out-Host
$cid = (docker compose ps -q postgres).Trim()
for ($i = 0; $i -lt 60; $i++) {
  if ((docker inspect --format '{{.State.Health.Status}}' $cid 2>$null) -eq "healthy") { break }
  Start-Sleep -Seconds 1
}
docker update --restart unless-stopped $cid | Out-Null
Write-Host "      container pinned to restart unless-stopped. (Ensure Docker Desktop starts on login.)" -ForegroundColor Green

# --- migrate -----------------------------------------------------------------
Write-Host "[2/4] Applying migrations ..." -ForegroundColor Cyan
$env:SCRIBE_DATABASE__URL = $dbUrl
& $bin --config $Config migrate | Out-Host

# --- services ----------------------------------------------------------------
Write-Host "[3/4] Installing services via nssm ..." -ForegroundColor Cyan
$logs = Join-Path $root "logs"
New-Item -ItemType Directory -Force -Path $logs | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $root "data\blobs") | Out-Null

function Set-ScribeService($name, $subcmd) {
  if (Get-Service $name -ErrorAction SilentlyContinue) { & $nssm stop $name | Out-Null; & $nssm remove $name confirm | Out-Null }
  & $nssm install $name $bin | Out-Null
  & $nssm set $name AppParameters "--config `"$Config`" $subcmd" | Out-Null
  & $nssm set $name AppDirectory $root | Out-Null
  & $nssm set $name AppEnvironmentExtra "SCRIBE_DATABASE__URL=$dbUrl" | Out-Null
  & $nssm set $name Start SERVICE_AUTO_START | Out-Null
  & $nssm set $name AppExit Default Restart | Out-Null
  & $nssm set $name AppRestartDelay 2000 | Out-Null
  & $nssm set $name AppStdout (Join-Path $logs "$name.log") | Out-Null
  & $nssm set $name AppStderr (Join-Path $logs "$name.log") | Out-Null
  & $nssm set $name AppRotateFiles 1 | Out-Null
  & $nssm set $name AppRotateBytes 10485760 | Out-Null
  Write-Host "      configured $name" -ForegroundColor Green
}
Set-ScribeService "scribe-serve" "serve"
Set-ScribeService "scribe-worker" "worker"
& $nssm start scribe-serve | Out-Null
& $nssm start scribe-worker | Out-Null

# --- health + Tailscale note --------------------------------------------------
Write-Host "[4/4] Verifying ..." -ForegroundColor Cyan
Start-Sleep -Seconds 4
try { $h = Invoke-RestMethod "http://127.0.0.1:$ApiPort/health" -TimeoutSec 5; Write-Host "      API health: $($h.status) (db=$($h.db))" -ForegroundColor Green }
catch { Write-Warning "API not healthy yet - check logs\scribe-serve.log." }
Write-Host "`nServices installed (auto-start on boot, restart on crash):" -ForegroundColor Green
Write-Host "  scribe-serve, scribe-worker   (manage with: nssm restart scribe-serve / Get-Service scribe-*)"
Write-Host "  logs: $logs"
Write-Host "`nTailscale Serve persists across reboots; set it once:" -ForegroundColor Yellow
Write-Host "  tailscale serve --bg http://127.0.0.1:$ApiPort"

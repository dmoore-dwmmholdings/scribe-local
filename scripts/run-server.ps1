<#
.SYNOPSIS
  Run the Scribe backend on a Windows SERVER from a deploy bundle (no build).
  Starts Postgres (Docker), applies migrations, fronts the API with Tailscale
  Serve, and launches `serve` + `worker` in two windows.

  Use this for the first run / smoke test. For an always-on server, install the
  services instead:  (admin)  .\scripts\install-service.ps1

  Run from the bundle root or from scripts\ - paths resolve to the bundle root.
#>
[CmdletBinding()]
param(
  [int]$DbPort   = 5433,
  [int]$ApiPort  = 8443,
  [string]$Config = "deploy\server.toml"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$bin = Join-Path $root "scribe.exe"
if (-not (Test-Path $bin)) { throw "scribe.exe not found at $bin - run this from inside the deploy bundle." }
$dbUrl = "postgres://scribe:scribe@localhost:$DbPort/scribe?sslmode=disable"
$env:SCRIBE_DATABASE__URL = $dbUrl

function Find-Tailscale {
  $c = Get-Command tailscale -ErrorAction SilentlyContinue
  if ($c) { return $c.Source }
  if (Test-Path "C:\Program Files\Tailscale\tailscale.exe") { return "C:\Program Files\Tailscale\tailscale.exe" }
  return $null
}

Write-Host "== Scribe server: run ==" -ForegroundColor Cyan

# 1. Postgres (pgvector) -------------------------------------------------------
Write-Host "`n[1/4] Postgres (pgvector) on :$DbPort ..." -ForegroundColor Cyan
$env:POSTGRES_PORT = "$DbPort"
docker compose up -d postgres | Out-Host
$cid = (docker compose ps -q postgres).Trim()
for ($i = 0; $i -lt 60; $i++) {
  if ((docker inspect --format '{{.State.Health.Status}}' $cid 2>$null) -eq "healthy") { Write-Host "      healthy." -ForegroundColor Green; break }
  Start-Sleep -Seconds 1
}

# 2. Migrate -------------------------------------------------------------------
Write-Host "`n[2/4] Applying migrations ..." -ForegroundColor Cyan
& $bin --config $Config migrate | Out-Host
if ($LASTEXITCODE -ne 0) { Write-Warning "migrate returned non-zero - check Postgres is up on :$DbPort." }

# 3. Tailscale serve -----------------------------------------------------------
Write-Host "`n[3/4] Tailscale serve -> http://127.0.0.1:$ApiPort ..." -ForegroundColor Cyan
$ts = Find-Tailscale
if (-not $ts) {
  Write-Warning "tailscale.exe not found - skipping. The phone won't reach the server until Tailscale + Serve are set up."
} else {
  $out = (& $ts serve --bg "http://127.0.0.1:$ApiPort" 2>&1 | Out-String)
  if ($out -match "not enabled") {
    Write-Warning "Tailscale 'Serve' is not enabled for your tailnet. Enable it once, then re-run:"
    Write-Host   "    https://login.tailscale.com/f/serve" -ForegroundColor Yellow
  } elseif ($out.Trim()) { Write-Host $out }
  $dns = (& $ts status --json 2>$null | ConvertFrom-Json).Self.DNSName.TrimEnd('.')
  if ($dns) { Write-Host "      tailnet API URL:  https://$dns   (use this in the app, and as api.public_base_url)" -ForegroundColor Green }
}

# 4. serve + worker in their own windows ---------------------------------------
Write-Host "`n[4/4] Starting serve + worker ..." -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path (Join-Path $root "data\blobs") | Out-Null
$envPrefix = "`$env:SCRIBE_DATABASE__URL='$dbUrl'; Set-Location '$root';"
Start-Process powershell -ArgumentList "-NoExit","-Command","$envPrefix & '$bin' --config '$Config' serve"
Start-Process powershell -ArgumentList "-NoExit","-Command","$envPrefix & '$bin' --config '$Config' worker"
Start-Sleep -Seconds 4
try { $h = Invoke-RestMethod "http://127.0.0.1:$ApiPort/health" -TimeoutSec 5; Write-Host "      API health: $($h.status) (db=$($h.db))" -ForegroundColor Green }
catch { Write-Warning "API not healthy yet - the serve window should show why (DB? models? config?)." }
Write-Host "`nTwo windows opened (serve + worker). Close them to stop. For always-on, use install-service.ps1." -ForegroundColor Yellow

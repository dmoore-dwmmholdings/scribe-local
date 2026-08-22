<#
.SYNOPSIS
  Set up the whole Scribe stack on this machine with one command.

  Builds the scribe image, starts Postgres, downloads the speech models, applies
  migrations, and runs the API and the worker. Then it prints the two things you
  need on the phone: the server URL and the device token.

  The only prerequisite is Docker Desktop. Nothing needs editing first.

.PARAMETER Tailscale
  Also publish the API on your tailnet with `tailscale serve`, and use the
  resulting MagicDNS name as the public base URL. Needed for the phone to reach
  the server from outside this machine.

.PARAMETER Ollama
  Also run Ollama in a container and pull the summarization model, so summaries
  and Q&A work. Adds a multi-gigabyte download.

.PARAMETER ApiPort
  Host port for the API. Default 8443.

.EXAMPLE
  .\scripts\quickstart.ps1
  .\scripts\quickstart.ps1 -Tailscale -Ollama
#>
[CmdletBinding()]
param(
  [switch]$Tailscale,
  [switch]$Ollama,
  [int]$ApiPort = 8443
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

function Step($n, $text) { Write-Host "`n[$n] $text" -ForegroundColor Cyan }
function Ok($text)       { Write-Host "      $text" -ForegroundColor Green }
function Note($text)     { Write-Host "      $text" -ForegroundColor DarkGray }

Write-Host "== Scribe quickstart ==" -ForegroundColor Cyan

# --- 1. Docker ---------------------------------------------------------------
Step 1 "Checking Docker ..."
if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
  throw "Docker is not installed. Install Docker Desktop (winget install Docker.DockerDesktop), then re-run this script."
}
# `docker version` talks to the daemon; the client alone answering is not enough.
& docker version --format '{{.Server.Version}}' *> $null
if ($LASTEXITCODE -ne 0) {
  $desktop = "$env:ProgramFiles\Docker\Docker\Docker Desktop.exe"
  if (Test-Path $desktop) {
    Note "Docker Desktop is not running - starting it (this takes a minute) ..."
    Start-Process $desktop
    for ($i = 0; $i -lt 120; $i++) {
      Start-Sleep -Seconds 2
      & docker version --format '{{.Server.Version}}' *> $null
      if ($LASTEXITCODE -eq 0) { break }
    }
  }
  & docker version --format '{{.Server.Version}}' *> $null
  if ($LASTEXITCODE -ne 0) { throw "Docker Desktop did not become ready. Start it manually, wait for the whale icon to settle, then re-run." }
}
Ok "Docker $(& docker version --format '{{.Server.Version}}') is ready."

# --- 2. Local settings -------------------------------------------------------
Step 2 "Writing .env ..."
$envPath = Join-Path $repo ".env"
if (Test-Path $envPath) {
  Ok ".env already exists - leaving it alone."
} else {
  Copy-Item (Join-Path $repo ".env.example") $envPath
  Ok "created .env from .env.example"
}
if ($ApiPort -ne 8443) {
  Add-Content $envPath "SCRIBE_API_PORT=$ApiPort"
  Note "API port set to $ApiPort"
}

# --- 3. Tailscale (optional) -------------------------------------------------
$publicUrl = "http://127.0.0.1:$ApiPort"
if ($Tailscale) {
  Step 3 "Publishing the API on your tailnet ..."
  $ts = (Get-Command tailscale -ErrorAction SilentlyContinue).Source
  if (-not $ts -and (Test-Path "$env:ProgramFiles\Tailscale\tailscale.exe")) { $ts = "$env:ProgramFiles\Tailscale\tailscale.exe" }
  if (-not $ts) {
    Write-Warning "tailscale.exe not found - skipping. Install Tailscale and re-run with -Tailscale."
  } else {
    $out = (& $ts serve --bg "http://127.0.0.1:$ApiPort" 2>&1 | Out-String)
    if ($out -match "not enabled") {
      Write-Warning "Tailscale 'Serve' is off for your tailnet. Turn it on once at https://login.tailscale.com/f/serve then re-run."
    }
    $dns = (& $ts status --json 2>$null | ConvertFrom-Json).Self.DNSName
    if ($dns) {
      $publicUrl = "https://$($dns.TrimEnd('.'))"
      Ok "tailnet URL: $publicUrl"
    }
  }
} else {
  Step 3 "Skipping Tailscale (re-run with -Tailscale to reach this server from your phone)."
}
# The worker and the app both resolve audio URLs against this value.
$lines = @(Get-Content $envPath | Where-Object { $_ -notmatch '^\s*SCRIBE_PUBLIC_BASE_URL\s*=' })
$lines += "SCRIBE_PUBLIC_BASE_URL=$publicUrl"
Set-Content -Encoding utf8 $envPath $lines

# --- 4. Build and start ------------------------------------------------------
Step 4 "Building and starting the stack ..."
Note "The first build compiles the ML stack and downloads ~750 MB of models."
Note "Expect 15-30 minutes. Later runs take seconds."
$profiles = @()
if ($Ollama) { $profiles += @("--profile", "ollama") }
& docker @("compose") @profiles @("up", "-d", "--build") | Out-Host
if ($LASTEXITCODE -ne 0) { throw "docker compose up failed - see the output above." }

# --- 5. Ollama model (optional) ----------------------------------------------
if ($Ollama) {
  Step 5 "Pulling the summarization model ..."
  $model = (Select-String -Path $envPath -Pattern '^\s*SCRIBE_SUMMARIZE_MODEL\s*=\s*(.+)$').Matches.Groups[1].Value
  if (-not $model) { $model = "gemma3:12b" }
  & docker compose exec -T ollama ollama pull $model.Trim() | Out-Host
}

# --- 6. Verify ---------------------------------------------------------------
Step 6 "Waiting for the API ..."
$healthy = $false
for ($i = 0; $i -lt 60; $i++) {
  try {
    $h = Invoke-RestMethod "http://127.0.0.1:$ApiPort/health" -TimeoutSec 3
    Ok "API health: $($h.status) (db=$($h.db))"
    $healthy = $true
    break
  } catch { Start-Sleep -Seconds 2 }
}
if (-not $healthy) {
  Write-Warning "The API did not answer yet. Check the logs:  docker compose logs -f scribe-serve"
}

# The token is minted inside the data volume on first start; read it from there
# rather than from the log, which may already have scrolled away.
$token = (& docker compose exec -T scribe-serve sh -c "sed -n 's/^phone = \`"\(.*\)\`"/\1/p' /data/devices.toml" 2>$null | Out-String).Trim()

Write-Host "`n== Scribe is running ==" -ForegroundColor Green
Write-Host "  Server URL (enter in the app):  $publicUrl"
if ($token) { Write-Host "  Device token (enter in the app): $token" }
else        { Write-Host "  Device token: docker compose exec scribe-serve cat /data/devices.toml" }
Write-Host ""
Write-Host "  Logs:     docker compose logs -f scribe-serve scribe-worker"
Write-Host "  Stop:     docker compose down"
Write-Host "  Restart:  docker compose up -d"
if (-not $Tailscale) {
  Write-Host ""
  Write-Host "  The phone cannot reach 127.0.0.1. Re-run with -Tailscale to publish" -ForegroundColor Yellow
  Write-Host "  the API on your tailnet." -ForegroundColor Yellow
}

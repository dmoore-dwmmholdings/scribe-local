# scripts/dev-db.ps1 — start/stop the development pgvector container
#                      and (optionally) run `scribe migrate`.
#
# Usage (PowerShell):
#   .\scripts\dev-db.ps1 up        # Start the container and run migrations
#   .\scripts\dev-db.ps1 start     # Start only (skip migrate)
#   .\scripts\dev-db.ps1 stop      # Stop the container (data preserved)
#   .\scripts\dev-db.ps1 down      # Stop + remove container
#   .\scripts\dev-db.ps1 reset     # Stop + destroy data (destructive!)
#   .\scripts\dev-db.ps1 status    # Show container status
#
# Requires: Docker Desktop for Windows (or Rancher Desktop), PowerShell 5.1+
# The script is idempotent: running 'up' when already running is safe.
#
# NOTE on ONNX / Windows builds
# ─────────────────────────────
# The real-ML build (default features) requires the MSVC toolchain and the
# ONNX Runtime native library. If you are on the GNU toolchain:
#   cargo build -p scribe-cli --no-default-features
# See docs/deployment.md for the full toolchain caveat.

param(
    [ValidateSet("up","start","migrate","stop","down","reset","status")]
    [string]$Action = "up"
)

$ErrorActionPreference = "Stop"

$ContainerName = "scribe-postgres-dev"
$Image         = "pgvector/pgvector:pg17"
$DbName        = "scribe"
$DbUser        = "scribe"
$DbPass        = "scribe"
# Use 5433 to avoid colliding with a local Postgres on 5432.
$HostPort      = if ($env:SCRIBE_DEV_DB_PORT) { $env:SCRIBE_DEV_DB_PORT } else { "5433" }
$DatabaseUrl   = "postgres://${DbUser}:${DbPass}@127.0.0.1:${HostPort}/${DbName}?sslmode=disable"

function Write-Log { param([string]$Msg) Write-Host "[dev-db] $Msg" }
function Write-Err { param([string]$Msg) Write-Error "[dev-db] ERROR: $Msg" }

function Test-ContainerRunning {
    $state = docker inspect -f '{{.State.Running}}' $ContainerName 2>$null
    return ($state -eq "true")
}

function Test-ContainerExists {
    docker inspect $ContainerName 2>$null | Out-Null
    return ($LASTEXITCODE -eq 0)
}

function Wait-ForPostgres {
    Write-Log "Waiting for Postgres to become ready..."
    $attempts = 30
    do {
        $ready = docker exec $ContainerName pg_isready -U $DbUser -d $DbName -q 2>$null
        if ($LASTEXITCODE -eq 0) { break }
        $attempts--
        if ($attempts -le 0) { Write-Err "Postgres did not become ready in time." }
        Start-Sleep -Seconds 1
    } while ($true)
    Write-Log "Postgres is ready."
}

function Start-Container {
    if (Test-ContainerRunning) {
        Write-Log "Container '$ContainerName' is already running on port $HostPort."
        return
    }
    if (Test-ContainerExists) {
        Write-Log "Starting existing container '$ContainerName'..."
        docker start $ContainerName | Out-Null
    } else {
        Write-Log "Creating container '$ContainerName' from $Image..."
        docker run -d `
            --name $ContainerName `
            -e POSTGRES_DB=$DbName `
            -e POSTGRES_USER=$DbUser `
            -e POSTGRES_PASSWORD=$DbPass `
            -p "${HostPort}:5432" `
            $Image | Out-Null
    }
    Wait-ForPostgres
    Write-Log "DB available at: $DatabaseUrl"
}

function Invoke-Migrate {
    Write-Log "Running scribe migrate..."
    # Find the binary
    $binary = $null
    if (Get-Command scribe -ErrorAction SilentlyContinue) {
        $binary = "scribe"
    } elseif (Test-Path "target\release\scribe.exe") {
        $binary = ".\target\release\scribe.exe"
    } elseif (Test-Path "target\debug\scribe.exe") {
        $binary = ".\target\debug\scribe.exe"
    } else {
        Write-Log "scribe binary not found; skipping migrate."
        Write-Log "Build with: cargo build -p scribe-cli --no-default-features"
        return
    }
    $env:SCRIBE_DATABASE__URL = $DatabaseUrl
    & $binary migrate
    if ($LASTEXITCODE -ne 0) { Write-Err "scribe migrate failed (exit $LASTEXITCODE)." }
    Write-Log "Migrations complete."
}

function Stop-Container {
    if (Test-ContainerRunning) {
        Write-Log "Stopping container '$ContainerName'..."
        docker stop $ContainerName | Out-Null
    } else {
        Write-Log "Container '$ContainerName' is not running."
    }
}

function Remove-Container {
    Stop-Container
    if (Test-ContainerExists) {
        Write-Log "Removing container '$ContainerName'..."
        docker rm $ContainerName | Out-Null
    }
}

function Reset-Database {
    Write-Log "WARNING: this will destroy all data in the dev database."
    $confirm = Read-Host "Type 'yes' to confirm"
    if ($confirm -ne "yes") { Write-Log "Aborted."; return }
    Remove-Container
    Write-Log "Dev database reset complete."
}

function Show-Status {
    if (Test-ContainerRunning) {
        Write-Log "Container '$ContainerName' is RUNNING on host port $HostPort."
    } elseif (Test-ContainerExists) {
        Write-Log "Container '$ContainerName' EXISTS but is stopped."
    } else {
        Write-Log "Container '$ContainerName' does not exist."
    }
}

switch ($Action) {
    "up"      { Start-Container; Invoke-Migrate }
    "start"   { Start-Container }
    "migrate" { Invoke-Migrate }
    "stop"    { Stop-Container }
    "down"    { Remove-Container }
    "reset"   { Reset-Database }
    "status"  { Show-Status }
}

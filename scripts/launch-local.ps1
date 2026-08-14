<#
.SYNOPSIS
  Launch Scribe on this Windows PC: Postgres (Docker) + migrate + serve + worker.

.DESCRIPTION
  One command to get the backend running locally for development/personal use.
  Starts the pgvector Postgres container, applies migrations, then opens the
  API server and the worker in two new PowerShell windows so you can see logs.

  Build modes:
    (default) stub  - no ONNX runtime/models needed; transcripts are placeholders.
                      Proves the whole pipeline end-to-end. Builds on any toolchain.
    -Real           - real models on the GPU. Requires the MSVC Rust toolchain,
                      ONNX models under .\models, and Ollama running. See the
                      launch guide / docs/self-update.md notes.

.EXAMPLE
  .\scripts\launch-local.ps1                 # stub build, DB on 5433
  .\scripts\launch-local.ps1 -Real           # real-ML build (needs MSVC + models)
  .\scripts\launch-local.ps1 -DbPort 5433    # override the host DB port
#>
[CmdletBinding()]
param(
    [int]$DbPort = 5433,
    [switch]$Real,
    [string]$Config = "deploy/local.toml"
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

Write-Host "== Scribe local launch ==" -ForegroundColor Cyan
Write-Host "repo:   $repo"
Write-Host "config: $Config"
Write-Host "db port: $DbPort   build: $(if ($Real) { 'REAL (MSVC + models + Ollama)' } else { 'stub' })"

# 1. Postgres (pgvector) via docker compose. POSTGRES_PORT feeds the compose file.
Write-Host "`n[1/4] Starting Postgres (pgvector) on :$DbPort ..." -ForegroundColor Cyan
$env:POSTGRES_PORT = "$DbPort"
docker compose up -d postgres | Out-Host

Write-Host "      waiting for healthy ..."
$cid = (docker compose ps -q postgres).Trim()
for ($i = 0; $i -lt 30; $i++) {
    $status = (docker inspect --format '{{.State.Health.Status}}' $cid 2>$null)
    if ($status -eq "healthy") { Write-Host "      postgres healthy." -ForegroundColor Green; break }
    Start-Sleep -Seconds 1
}

# Keep the binary's DB URL in lockstep with the chosen port (overrides the TOML).
$env:SCRIBE_DATABASE__URL = "postgres://scribe:scribe@localhost:$DbPort/scribe?sslmode=disable"

# 2. Build the single `scribe` binary.
Write-Host "`n[2/4] Building scribe ..." -ForegroundColor Cyan
if ($Real) {
    cargo build --release -p scribe-cli
    $bin = Join-Path $repo "target\release\scribe.exe"
} else {
    cargo build --release -p scribe-cli --no-default-features
    $bin = Join-Path $repo "target\release\scribe.exe"
}
if (-not (Test-Path $bin)) { throw "build did not produce $bin" }
Write-Host "      binary: $bin" -ForegroundColor Green

# 3. Migrate.
Write-Host "`n[3/4] Applying migrations ..." -ForegroundColor Cyan
& $bin --config $Config migrate | Out-Host

# 4. Launch serve + worker in their own windows.
Write-Host "`n[4/4] Starting serve + worker ..." -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path (Join-Path $repo "data\blobs") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $repo "models") | Out-Null

$envPrefix = "`$env:SCRIBE_DATABASE__URL='$($env:SCRIBE_DATABASE__URL)';"
Start-Process powershell -ArgumentList "-NoExit", "-Command", "$envPrefix & '$bin' --config '$Config' serve"
Start-Process powershell -ArgumentList "-NoExit", "-Command", "$envPrefix & '$bin' --config '$Config' worker"

Start-Sleep -Seconds 2
Write-Host "`nScribe is starting." -ForegroundColor Green
Write-Host "  API:    http://127.0.0.1:8443/health"
Write-Host "  Test:   & '$bin' --config $Config ingest <file>.m4a --title test --participants 2"
Write-Host "  Doctor: & '$bin' --config $Config doctor"
Write-Host "`nTwo windows opened (serve + worker). Close them to stop." -ForegroundColor Yellow
if (-not $Real) {
    Write-Host "`nNOTE: stub build - transcripts are placeholders. Re-run with -Real once the" -ForegroundColor Yellow
    Write-Host "      MSVC toolchain + ONNX models + Ollama are installed for real transcription."
}

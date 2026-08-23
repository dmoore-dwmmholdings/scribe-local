<#
.SYNOPSIS
  One command to (re)build AND bring up the whole Scribe stack for end-to-end
  phone testing: rebuild scribe.exe -> migrate -> Postgres -> Tailscale serve
  (HTTPS in front of the API) -> backend (serve & worker, each in its own
  window) -> the Expo dev server.

  It ALWAYS rebuilds the backend first, so the running server can never drift
  from your source. `cargo build` is incremental, so a no-change rebuild is
  near-instant. The old serve/worker are stopped first to free the exe lock
  (Windows won't let cargo overwrite a running .exe).

  The Expo QR stays in THIS window - scan it with the dev client. Ctrl-C stops
  Expo; the backend windows and `tailscale serve` keep running.

  Build modes:
    (default) real  - real ML models. Needs the MSVC Rust toolchain, ONNX models
                      under .\models, and Ollama/LM Studio running.
    -Stub           - no ONNX toolchain/models needed; transcripts are
                      placeholders. The whole pipeline still runs end-to-end.

  Prereqs (one-time):
    - Docker Desktop running (for the Postgres container).
    - Tailscale signed in AND "Serve" enabled for the tailnet:
        https://login.tailscale.com/f/serve   (admin console toggle)
    - For -Stub you need nothing extra; for the real build see docs/.

.EXAMPLE
  .\scripts\launch-all.ps1            # rebuild (real) + relaunch everything
  .\scripts\launch-all.ps1 -Stub      # rebuild without the ONNX toolchain
  .\scripts\launch-all.ps1 -NoExpo    # rebuild + backend only (leave Metro alone)
#>
[CmdletBinding()]
param(
  [int]$DbPort    = 5433,
  [int]$ApiPort   = 8443,
  [string]$Config = "deploy/local.toml",
  [switch]$Stub,
  [switch]$NoExpo
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo
$bin = Join-Path $repo "target\release\scribe.exe"
$dbUrl = "postgres://scribe:scribe@localhost:$DbPort/scribe?sslmode=disable"
# Keep the binary's DB URL in lockstep with the chosen port (overrides the TOML)
# for the migrate step below and for the serve/worker windows.
$env:SCRIBE_DATABASE__URL = $dbUrl

# A pinned ffmpeg in .\tools\ffmpeg takes priority over any older copy on PATH.
# The worker calls `ffmpeg` by name, and support for the 'chnl' channel-layout
# box at version 1 - which iOS writes - only reached FFmpeg in mid-2026. Any
# build older than that fails every phone recording in the transcode stage with
# "Unsupported 'chnl' box with version 1", including builds from late 2025.
$ffmpegDir  = Join-Path $repo "tools\ffmpeg"
$ffmpegPinned = Test-Path (Join-Path $ffmpegDir "ffmpeg.exe")
if ($ffmpegPinned) { $env:PATH = "$ffmpegDir;$env:PATH" }

function Find-Tailscale {
  $c = Get-Command tailscale -ErrorAction SilentlyContinue
  if ($c) { return $c.Source }
  if (Test-Path "C:\Program Files\Tailscale\tailscale.exe") { return "C:\Program Files\Tailscale\tailscale.exe" }
  return $null
}

function Enter-MsvcBuildEnv {
  # The real ML build needs the MSVC toolchain + the VS Developer Shell env
  # (link.exe, libs). The default rustup toolchain here is windows-gnu, for which
  # ort-sys ships no prebuilt ONNX Runtime, so the build fails without this.
  $env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-msvc'
  if (Get-Command link.exe -ErrorAction SilentlyContinue) { return }  # already in a dev env
  $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
  if (-not (Test-Path $vswhere)) {
    Write-Warning "Visual Studio not found. The real build needs VS + the MSVC toolchain - use -Stub, or run from a 'Developer PowerShell for VS'."
    return
  }
  $vs = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
  if (-not $vs) { $vs = (& $vswhere -latest -property installationPath | Select-Object -First 1) }
  $devShell = if ($vs) { Join-Path $vs "Common7\Tools\Microsoft.VisualStudio.DevShell.dll" } else { $null }
  if (-not $devShell -or -not (Test-Path $devShell)) {
    Write-Warning "VS DevShell module not found. Use -Stub, or run from a 'Developer PowerShell for VS'."
    return
  }
  Import-Module $devShell
  Enter-VsDevShell -VsInstallPath $vs -DevCmdArguments '-arch=x64' -SkipAutomaticLocation | Out-Null
  Set-Location $repo
  Write-Host "      MSVC dev environment loaded." -ForegroundColor Green
}

$build = if ($Stub) { "stub" } else { "real" }
Write-Host "== Scribe: rebuild + launch all ($build) ==" -ForegroundColor Cyan

# 1. Postgres (pgvector) ------------------------------------------------------
Write-Host "`n[1/7] Postgres (pgvector) on :$DbPort ..." -ForegroundColor Cyan
$env:POSTGRES_PORT = "$DbPort"
docker compose up -d postgres | Out-Host
$cid = (docker compose ps -q postgres).Trim()
for ($i = 0; $i -lt 30; $i++) {
  if ((docker inspect --format '{{.State.Health.Status}}' $cid 2>$null) -eq "healthy") { Write-Host "      healthy." -ForegroundColor Green; break }
  Start-Sleep -Seconds 1
}

# 2. Stop any previous backend so the port/binary are free to rebuild ---------
Write-Host "`n[2/7] Stopping any running scribe processes (frees the exe lock) ..." -ForegroundColor Cyan
Get-Process scribe -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force }
Start-Sleep -Seconds 1

# 3. Build the single `scribe` binary -----------------------------------------
Write-Host "`n[3/7] Building scribe ($build) ..." -ForegroundColor Cyan
if ($Stub) {
  cargo build --release -p scribe-cli --no-default-features
} else {
  Enter-MsvcBuildEnv
  cargo build --release -p scribe-cli
}
if ($LASTEXITCODE -ne 0) {
  Write-Warning "Build failed. If you don't have the MSVC + ONNX toolchain yet, retry with -Stub:"
  Write-Host   "    .\scripts\launch-all.ps1 -Stub" -ForegroundColor Yellow
  exit 1
}
if (-not (Test-Path $bin)) { throw "build did not produce $bin" }
Write-Host "      binary: $bin" -ForegroundColor Green

# 4. Apply migrations (idempotent) --------------------------------------------
Write-Host "`n[4/7] Applying migrations ..." -ForegroundColor Cyan
& $bin --config $Config migrate | Out-Host
if ($LASTEXITCODE -ne 0) { Write-Warning "migrate returned non-zero - check the DB is up and reachable on :$DbPort." }

# 5. Tailscale serve in front of the API --------------------------------------
Write-Host "`n[5/7] Tailscale serve -> http://127.0.0.1:$ApiPort ..." -ForegroundColor Cyan
$ts = Find-Tailscale
if (-not $ts) {
  Write-Warning "tailscale.exe not found -skipping serve. Phone access over the tailnet won't work."
} else {
  $out = (& $ts serve --bg "http://127.0.0.1:$ApiPort" 2>&1 | Out-String)
  if ($out -match "not enabled") {
    Write-Warning "Tailscale 'Serve' is not enabled for your tailnet. Enable it once here, then re-run:"
    Write-Host   "    https://login.tailscale.com/f/serve" -ForegroundColor Yellow
  } elseif ($out.Trim()) {
    Write-Host $out
  }
  $dns = (& $ts status --json 2>$null | ConvertFrom-Json).Self.DNSName.TrimEnd('.')
  if ($dns) { Write-Host "      tailnet API URL:  https://$dns" -ForegroundColor Green }
  & $ts serve status 2>&1 | Out-Host
}

# 6. Backend: serve + worker, each in its own window --------------------------
Write-Host "`n[6/7] Backend (serve + worker) in separate windows ..." -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path (Join-Path $repo "data\blobs") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $repo "models")     | Out-Null
$pathPrefix = if ($ffmpegPinned) { "`$env:PATH='$ffmpegDir;' + `$env:PATH; " } else { "" }
if ($ffmpegPinned) { Write-Host "      ffmpeg: $ffmpegDir (pinned, ahead of PATH)" -ForegroundColor Green }
$envPrefix = "`$env:SCRIBE_DATABASE__URL='$dbUrl'; $pathPrefix Set-Location '$repo';"
Start-Process powershell -ArgumentList "-NoExit","-Command","$envPrefix & '$bin' --config '$Config' serve"
Start-Process powershell -ArgumentList "-NoExit","-Command","$envPrefix & '$bin' --config '$Config' worker"
Start-Sleep -Seconds 4
try { $h = Invoke-RestMethod "http://127.0.0.1:$ApiPort/health" -TimeoutSec 5; Write-Host "      API health: $($h.status) (db=$($h.db))" -ForegroundColor Green }
catch { Write-Warning "API not healthy yet -the serve window should show why." }

# 7. Expo dev server (QR stays in THIS window) --------------------------------
if ($NoExpo) {
  Write-Host "`n[7/7] -NoExpo set: backend is up; leaving the Expo dev server alone." -ForegroundColor Cyan
  Write-Host "      Backend rebuilt + restarted. Metro (if already running) keeps its connection." -ForegroundColor Green
  return
}
Write-Host "`n[7/7] Starting Expo -scan the QR below with the dev client." -ForegroundColor Cyan
# First run convenience: install mobile deps if they're missing.
if (-not (Test-Path (Join-Path $repo "mobile\node_modules"))) {
  Write-Host "      mobile\node_modules missing - running npm install (one-time) ..." -ForegroundColor Yellow
  Push-Location (Join-Path $repo "mobile")
  npm install | Out-Host
  Pop-Location
}
# Free Metro's port if a stale instance is squatting on it (avoids the bind error).
$stale = Get-NetTCPConnection -LocalPort 8081 -State Listen -ErrorAction SilentlyContinue
if ($stale) { $stale.OwningProcess | Select-Object -Unique | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue } }
Write-Host "      (Ctrl-C stops Expo; the backend + tailscale serve keep running.)`n" -ForegroundColor Yellow
Set-Location (Join-Path $repo "mobile")
npx expo start

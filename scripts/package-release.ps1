<#
.SYNOPSIS
  Build the real (MSVC + GPU) scribe binary on THIS dev PC and assemble a
  self-contained deploy bundle to copy to a Windows server. The server needs NO
  Rust/Visual Studio - only the runtime prereqs (see docs/deploy-windows-server.md).

  The bundle contains: scribe.exe + all sibling DLLs (ONNX Runtime, sherpa-onnx,
  and - unless -Cpu - the CUDA/cuDNN GPU DLLs), the server config + devices
  example, docker-compose.yml, the server scripts, and a README.

  The ONNX models are NOT bundled by default - `scribe.exe models pull` downloads
  them on the server (about 750 MB), which keeps the bundle small enough to ship
  as a release asset. Use -WithModels for an offline server.

.PARAMETER OutDir      Output folder (relative to repo). Default dist\scribe-server.
.PARAMETER Cpu         Skip the GPU DLLs (CPU-only server).
.PARAMETER WithModels  Copy the local models/ folder into the bundle (adds ~750 MB).
.PARAMETER Zip         Also produce <OutDir>.zip.

.EXAMPLE
  .\scripts\package-release.ps1            # GPU bundle in dist\scribe-server
  .\scripts\package-release.ps1 -Zip       # ...and a .zip to copy over
  .\scripts\package-release.ps1 -Cpu       # CPU-only server bundle
  .\scripts\package-release.ps1 -WithModels  # include models/ (offline server)
#>
[CmdletBinding()]
param(
  [string]$OutDir = "dist\scribe-server",
  [switch]$Cpu,
  [switch]$WithModels,
  [switch]$Zip
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# Windows PowerShell 5.1 wraps a native command's stderr in an ErrorRecord, and
# with $ErrorActionPreference = 'Stop' that terminates the script. cargo writes
# its progress ("Compiling ...") to stderr, so calling it directly kills this
# script whenever stderr is redirected - which is what happens in a non-interactive
# run. Drop to 'Continue' for the call and judge the result by the exit code.
function Invoke-Native {
  param([Parameter(Mandatory)][scriptblock]$Command, [string]$What = "command")
  $prev = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try { & $Command 2>&1 | Out-Host; $code = $LASTEXITCODE }
  finally { $ErrorActionPreference = $prev }
  if ($code -ne 0) { throw "$What failed (exit $code)" }
}

function Enter-MsvcBuildEnv {
  # Real ML build needs the MSVC toolchain + VS Developer Shell (link.exe + libs).
  $env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-msvc'
  if (Get-Command link.exe -ErrorAction SilentlyContinue) { return }
  $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
  if (-not (Test-Path $vswhere)) { throw "Visual Studio not found - install VS 2022 + 'Desktop development with C++', or run from a Developer PowerShell." }
  $vs = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
  if (-not $vs) { $vs = (& $vswhere -latest -property installationPath | Select-Object -First 1) }
  $devShell = Join-Path $vs "Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
  if (-not (Test-Path $devShell)) { throw "VS DevShell module not found under $vs." }
  Import-Module $devShell
  Enter-VsDevShell -VsInstallPath $vs -DevCmdArguments '-arch=x64' -SkipAutomaticLocation | Out-Null
  Set-Location $repo
}

$build = if ($Cpu) { "CPU" } else { "GPU (CUDA)" }
Write-Host "== Packaging Scribe server bundle ($build) ==" -ForegroundColor Cyan

# 1. Build the real binary -----------------------------------------------------
Write-Host "`n[1/4] Building scribe.exe (release, MSVC) ..." -ForegroundColor Cyan
Enter-MsvcBuildEnv
Invoke-Native { cargo build --release -p scribe-cli } "cargo build"
$rel = Join-Path $repo "target\release"
$bin = Join-Path $rel "scribe.exe"
if (-not (Test-Path $bin)) { throw "build did not produce $bin" }

# 2. GPU DLLs (CUDA onnxruntime + cuDNN) ---------------------------------------
if (-not $Cpu) {
  $gpu = Join-Path $repo "scripts\setup-gpu.ps1"
  if (Test-Path $gpu) {
    Write-Host "`n[2/4] Staging CUDA/cuDNN DLLs into target\release ..." -ForegroundColor Cyan
    Invoke-Native { & $gpu } "setup-gpu.ps1"
  } else {
    Write-Warning "[2/4] scripts\setup-gpu.ps1 not found - CUDA DLLs may be missing from the bundle. Use -Cpu for a CPU-only server."
  }
} else {
  Write-Host "`n[2/4] CPU bundle - skipping GPU DLLs." -ForegroundColor Cyan
}

# 3. Assemble the bundle -------------------------------------------------------
Write-Host "`n[3/4] Assembling bundle ..." -ForegroundColor Cyan
$dest = Join-Path $repo $OutDir
if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
New-Item -ItemType Directory -Force -Path $dest | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $dest "deploy") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $dest "scripts") | Out-Null

Copy-Item $bin $dest
# -Cpu must EXCLUDE the CUDA/cuDNN DLLs, not merely skip staging them: a previous
# GPU build (or a plain setup-gpu.ps1 run) leaves them in target\release, and a
# blind *.dll copy would put ~3 GB of them in a "CPU" bundle. The GPU DLLs all
# start with cu*/nv*, plus the CUDA execution provider.
$gpuDllPattern = '^(cu|nv)|^onnxruntime_providers_cuda\.dll$'
Get-ChildItem (Join-Path $rel "*.dll") | Where-Object {
  -not ($Cpu -and $_.Name -match $gpuDllPattern)
} | Copy-Item -Destination $dest
if ($WithModels) {
  if (-not (Test-Path (Join-Path $repo "models"))) { Write-Warning "-WithModels given, but models\ is empty. The server will have to run: scribe.exe models pull" }
  else { Copy-Item (Join-Path $repo "models") (Join-Path $dest "models") -Recurse }
} else {
  Write-Host "      models/ not bundled - the server downloads them with: scribe.exe models pull" -ForegroundColor DarkGray
}
Copy-Item (Join-Path $repo "deploy\server.toml") (Join-Path $dest "deploy")
if (Test-Path (Join-Path $repo "deploy\devices.toml.example")) { Copy-Item (Join-Path $repo "deploy\devices.toml.example") (Join-Path $dest "deploy") }
# The bundle gets the POSTGRES-ONLY compose file, not the repo's full-stack one:
# a bundle has no Dockerfile and no sources, so `docker compose up --build` there
# fails with "failed to read dockerfile". Native install = host binary + pgvector.
Copy-Item (Join-Path $repo "deploy\docker-compose.postgres.yml") (Join-Path $dest "docker-compose.yml")
Copy-Item (Join-Path $repo "scripts\run-server.ps1") (Join-Path $dest "scripts")
Copy-Item (Join-Path $repo "scripts\install-service.ps1") (Join-Path $dest "scripts")

@'
# Scribe server bundle

Runtime-only. No Rust/Visual Studio needed here. See docs/deploy-windows-server.md
in the repo for the full walkthrough. Quick start on the server:

This is the NATIVE install: scribe.exe runs on the host, and Docker only
provides Postgres. Do NOT run `docker compose up --build` here - there is no
Dockerfile in a bundle. That command belongs to the containerized install, which
starts from a clone of the repo (see docs/install.md).

1. Install prereqs: Docker Desktop, Visual C++ Redistributable 2015-2022 x64,
   ffmpeg (on PATH), an LLM (LM Studio or Ollama), Tailscale, and an NVIDIA
   driver (GPU bundle). No CUDA toolkit needed - the CUDA DLLs are bundled.
2. Download the speech models (~750 MB; skips anything already present):
      .\scribe.exe --config deploy\server.toml models pull
3. Edit deploy\server.toml (signing_secret, public_base_url, summarize_model)
   and create deploy\devices.toml from the example (device-token auth is ON).
4. First run / test:        .\scripts\run-server.ps1
5. Install as services:     (admin) .\scripts\install-service.ps1
6. Enable Tailscale Serve once: https://login.tailscale.com/f/serve

Logs (services): .\logs\scribe-serve.log and .\logs\scribe-worker.log

CPU bundle note: deploy\server.toml ships with whisper-large-v3-turbo on CUDA.
A CPU-only bundle has no CUDA DLLs, and Whisper-large on a CPU is slow. Set
model = "parakeet-tdt-0.6b-v3" and device = "cpu" before step 2.

Prefer containers? Clone the repo and run `docker compose up -d --build` there -
it does all of the above on any OS. See docs/install.md. This bundle is the
native Windows path, and it is the one that can use an NVIDIA GPU.
'@ | Set-Content -Encoding utf8 (Join-Path $dest "README.md")

# 4. Optional zip --------------------------------------------------------------
Write-Host "`n[4/4] Done." -ForegroundColor Cyan
$sizeGB = [math]::Round(((Get-ChildItem $dest -Recurse | Measure-Object Length -Sum).Sum / 1GB), 2)
Write-Host "      bundle: $dest  (${sizeGB} GB)" -ForegroundColor Green
if ($Zip) {
  # Compress-Archive caps at ~2 GB ("Stream was too long"). Use bsdtar (built into
  # Windows 10/11), which handles multi-GB archives. -a selects zip from the .zip ext.
  $zipPath = "$dest.zip"
  if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
  # Must be the Windows bsdtar, not MSYS/Git Bash tar - the latter is usually
  # first on PATH in a developer shell and cannot handle a Windows path here.
  $tar = Join-Path $env:SystemRoot "System32\tar.exe"
  if (-not (Test-Path $tar)) { $tar = (Get-Command tar -ErrorAction SilentlyContinue).Source }
  if ($tar) {
    Write-Host "      archiving (tar) -> $zipPath ..." -ForegroundColor Cyan
    $prev = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    & $tar -a -c -f $zipPath -C (Split-Path -Parent $dest) (Split-Path -Leaf $dest) 2>&1 | Out-Host
    $tarCode = $LASTEXITCODE; $ErrorActionPreference = $prev
    if ($tarCode -eq 0) { Write-Host "      archive: $zipPath (extract on the server with: tar -xf scribe-server.zip)" -ForegroundColor Green }
    else { Write-Warning "tar failed - just copy the folder $dest to the server instead." }
  } else {
    Write-Warning "tar not found, and Compress-Archive can't handle >2 GB. Just copy the folder $dest to the server."
  }
}
Write-Host "`nCopy the bundle to the server, then run scripts\run-server.ps1 (test) or scripts\install-service.ps1 (services)." -ForegroundColor Yellow

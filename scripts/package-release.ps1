<#
.SYNOPSIS
  Build the real (MSVC + GPU) scribe binary on THIS dev PC and assemble a
  self-contained deploy bundle to copy to a Windows server. The server needs NO
  Rust/Visual Studio - only the runtime prereqs (see docs/deploy-windows-server.md).

  The bundle contains: scribe.exe + all sibling DLLs (ONNX Runtime, sherpa-onnx,
  and - unless -Cpu - the CUDA/cuDNN GPU DLLs), the models/ folder, the server
  config + devices example, docker-compose.yml, the server scripts, and a README.

.PARAMETER OutDir   Output folder (relative to repo). Default dist\scribe-server.
.PARAMETER Cpu      Skip the GPU DLLs (CPU-only server).
.PARAMETER Zip      Also produce <OutDir>.zip.

.EXAMPLE
  .\scripts\package-release.ps1            # GPU bundle in dist\scribe-server
  .\scripts\package-release.ps1 -Zip       # ...and a .zip to copy over
  .\scripts\package-release.ps1 -Cpu       # CPU-only server bundle
#>
[CmdletBinding()]
param(
  [string]$OutDir = "dist\scribe-server",
  [switch]$Cpu,
  [switch]$Zip
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

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
cargo build --release -p scribe-cli
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$rel = Join-Path $repo "target\release"
$bin = Join-Path $rel "scribe.exe"
if (-not (Test-Path $bin)) { throw "build did not produce $bin" }

# 2. GPU DLLs (CUDA onnxruntime + cuDNN) ---------------------------------------
if (-not $Cpu) {
  $gpu = Join-Path $repo "scripts\setup-gpu.ps1"
  if (Test-Path $gpu) {
    Write-Host "`n[2/4] Staging CUDA/cuDNN DLLs into target\release ..." -ForegroundColor Cyan
    & $gpu
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
Copy-Item (Join-Path $rel "*.dll") $dest
if (-not (Test-Path (Join-Path $repo "models"))) { Write-Warning "models\ folder is empty/missing - the server will fall back to STUB transcription. Download models first (see models\README.md)." }
else { Copy-Item (Join-Path $repo "models") (Join-Path $dest "models") -Recurse }
Copy-Item (Join-Path $repo "deploy\server.toml") (Join-Path $dest "deploy")
if (Test-Path (Join-Path $repo "deploy\devices.toml.example")) { Copy-Item (Join-Path $repo "deploy\devices.toml.example") (Join-Path $dest "deploy") }
Copy-Item (Join-Path $repo "docker-compose.yml") $dest
Copy-Item (Join-Path $repo "scripts\run-server.ps1") (Join-Path $dest "scripts")
Copy-Item (Join-Path $repo "scripts\install-service.ps1") (Join-Path $dest "scripts")

@'
# Scribe server bundle

Runtime-only. No Rust/Visual Studio needed here. See docs/deploy-windows-server.md
in the repo for the full walkthrough. Quick start on the server:

1. Install prereqs: Docker Desktop, Visual C++ Redistributable 2015-2022 x64,
   ffmpeg (on PATH), an LLM (LM Studio or Ollama), Tailscale, and an NVIDIA
   driver (GPU bundle). No CUDA toolkit needed - the CUDA DLLs are bundled.
2. Edit deploy\server.toml (signing_secret, public_base_url, summarize_model)
   and create deploy\devices.toml from the example (device-token auth is ON).
3. First run / test:        .\scripts\run-server.ps1
4. Install as services:     (admin) .\scripts\install-service.ps1
5. Enable Tailscale Serve once: https://login.tailscale.com/f/serve

Logs (services): .\logs\scribe-serve.log and .\logs\scribe-worker.log
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
  $tar = (Get-Command tar -ErrorAction SilentlyContinue).Source
  if ($tar) {
    Write-Host "      archiving (tar) -> $zipPath ..." -ForegroundColor Cyan
    & $tar -a -c -f $zipPath -C (Split-Path -Parent $dest) (Split-Path -Leaf $dest)
    if ($LASTEXITCODE -eq 0) { Write-Host "      archive: $zipPath (extract on the server with: tar -xf scribe-server.zip)" -ForegroundColor Green }
    else { Write-Warning "tar failed - just copy the folder $dest to the server instead." }
  } else {
    Write-Warning "tar not found, and Compress-Archive can't handle >2 GB. Just copy the folder $dest to the server."
  }
}
Write-Host "`nCopy the bundle to the server, then run scripts\run-server.ps1 (test) or scripts\install-service.ps1 (services)." -ForegroundColor Yellow

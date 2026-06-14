<#
.SYNOPSIS
  Enable NVIDIA GPU (CUDA) acceleration for the real-ML build's ASR/diarization.

.DESCRIPTION
  The sherpa-onnx `shared` prebuilt ships a CPU-only onnxruntime.dll, so
  `[asr].device = "cuda"` silently falls back to CPU. This script installs the
  GPU execution provider next to the binary:

    1. Microsoft's onnxruntime-win-x64-gpu (matched to the onnxruntime version
       sherpa-onnx ships, so it stays ABI-compatible with sherpa-onnx-c-api.dll
       AND with `ort`/fastembed) — provides onnxruntime.dll + providers_cuda.
    2. The CUDA 12 + cuDNN 9 runtime DLLs (via pip nvidia-* wheels) that the
       provider dlopens at runtime.

  All DLLs land in target\<profile>\ beside scribe.exe. A clean `cargo build`
  re-copies the CPU sherpa DLLs, so re-run this after a clean rebuild.

  Requires: the real build (`launch-local.ps1 -Real`), an NVIDIA driver
  (`nvidia-smi`), and Python+pip. ~1.5 GB of downloads the first time.

.NOTES
  If a future sherpa-onnx version bundles a different onnxruntime version, update
  $OrtVersion below to match (check: the ProductVersion of the onnxruntime.dll
  that the build drops in target\<profile>\ before running this).

.EXAMPLE
  .\scripts\setup-gpu.ps1
  # then set [asr].device = "cuda" (deploy/local.toml already does) and restart.
#>
[CmdletBinding()]
param(
    [string]$Profile = "release",
    [string]$OrtVersion = "1.24.4"
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$rel  = Join-Path $repo "target\$Profile"
$work = Join-Path $repo "data\cuda-setup"
New-Item -ItemType Directory -Force -Path $work | Out-Null

$bin = Join-Path $rel "scribe.exe"
if (-not (Test-Path $bin)) { throw "real binary not found at $bin — build it first (launch-local.ps1 -Real)" }
if (-not (Get-Command nvidia-smi -ErrorAction SilentlyContinue)) { Write-Warning "nvidia-smi not found; is the NVIDIA driver installed?" }

Write-Host "== Scribe GPU setup ==" -ForegroundColor Cyan
Write-Host "binary:      $bin"
$shipped = (Get-Item $bin | ForEach-Object { (Get-Item (Join-Path $rel 'onnxruntime.dll') -ErrorAction SilentlyContinue).VersionInfo.ProductVersion })
Write-Host "onnxruntime currently beside binary: $shipped (target GPU version: $OrtVersion)"

# 1. onnxruntime GPU (matching the shipped version) ---------------------------
$zip = Join-Path $work "onnxruntime-gpu-$OrtVersion.zip"
$ortDir = Join-Path $work "ort-gpu-$OrtVersion"
if (-not (Test-Path $zip)) {
    $url = "https://github.com/microsoft/onnxruntime/releases/download/v$OrtVersion/onnxruntime-win-x64-gpu-$OrtVersion.zip"
    Write-Host "[1/3] downloading $url ..." -ForegroundColor Cyan
    Invoke-WebRequest -Uri $url -OutFile $zip
}
if (-not (Test-Path $ortDir)) { Expand-Archive -Path $zip -DestinationPath $ortDir -Force }
foreach ($d in @("onnxruntime.dll","onnxruntime_providers_cuda.dll","onnxruntime_providers_shared.dll")) {
    $src = Get-ChildItem -Path $ortDir -Recurse -Filter $d | Select-Object -First 1
    if ($src) { Copy-Item -Force $src.FullName (Join-Path $rel $d) }
}
Write-Host "      onnxruntime GPU provider installed." -ForegroundColor Green

# 2. CUDA 12 + cuDNN 9 runtime DLLs (pip wheels) ------------------------------
$cudalibs = Join-Path $work "cudalibs"
Write-Host "[2/3] installing CUDA 12 + cuDNN 9 runtime DLLs (pip)..." -ForegroundColor Cyan
python -m pip install --target $cudalibs --upgrade --no-cache-dir `
    nvidia-cuda-runtime-cu12 nvidia-cublas-cu12 nvidia-cufft-cu12 `
    nvidia-curand-cu12 nvidia-cusparse-cu12 nvidia-cudnn-cu12 | Out-Null
Get-ChildItem -Path $cudalibs -Recurse -Filter "*.dll" | ForEach-Object {
    Copy-Item -Force $_.FullName (Join-Path $rel $_.Name)
}
Write-Host "      CUDA/cuDNN runtime DLLs installed." -ForegroundColor Green

# 3. Verify the provider's imports are all satisfied --------------------------
Write-Host "[3/3] verifying provider dependencies..." -ForegroundColor Cyan
$needed = @("cudart64_12.dll","cublas64_12.dll","cublasLt64_12.dll","cudnn64_9.dll","cufft64_11.dll")
$missing = $needed | Where-Object { -not (Test-Path (Join-Path $rel $_)) }
if ($missing) { Write-Warning "missing: $($missing -join ', ')" } else { Write-Host "      all CUDA EP dependencies present." -ForegroundColor Green }

Write-Host "`nGPU provider ready. Ensure [asr].device = `"cuda`" and restart serve+worker." -ForegroundColor Green
Write-Host "Verify: nvidia-smi should show scribe.exe as a compute app and GPU util spike during a transcription." -ForegroundColor Yellow

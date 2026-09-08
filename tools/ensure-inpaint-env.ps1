$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Venv = Join-Path $Root '.img-inpaint-venv'
$Pip = Join-Path $Venv 'Scripts\pip.exe'
$Iopaint = Join-Path $Venv 'Scripts\iopaint.exe'
$Log = Join-Path $Venv 'install.log'
$PipIndex = if ($env:PIP_INDEX_URL) { $env:PIP_INDEX_URL } else { 'https://mirrors.aliyun.com/pypi/simple/' }
$ModelUrl = if ($env:LAMA_MODEL_URL) { $env:LAMA_MODEL_URL } else { 'https://github.com/Sanster/models/releases/download/add_big_lama/big-lama.pt' }
$ModelPath = Join-Path $env:USERPROFILE '.cache\torch\hub\checkpoints\big-lama.pt'

New-Item -ItemType Directory -Force -Path $Venv, (Split-Path -Parent $ModelPath) | Out-Null

function Check-Env {
  & $Iopaint list *> $Log
  if ($LASTEXITCODE -ne 0) {
    Write-Error "project inpaint env check failed; see $Log"
    exit 1
  }
}

function Get-Model {
  if (Test-Path $ModelPath) { return }
  Write-Host "downloading LaMa model (~200MB): $ModelUrl"
  & curl.exe -L --retry 5 --retry-delay 5 --connect-timeout 60 --continue-at - -o "$ModelPath" "$ModelUrl" 2>> $Log
  if ($LASTEXITCODE -ne 0) {
    Write-Error "model download failed; see $Log"
    exit 1
  }
}

if (Test-Path $Iopaint) {
  Get-Model
  Check-Env
  Write-Host "project inpaint env ready: $Iopaint"
  exit 0
}

$Python = (Get-Command 'python' -ErrorAction SilentlyContinue).Source
if (-not $Python) {
  $Py = Get-Command 'py' -ErrorAction SilentlyContinue
  if ($Py) { $Python = 'py'; $PythonArgs = @('-3') } else {
    Write-Error 'python not found; install Python 3.10+ from https://www.python.org/downloads/ first'
    exit 1
  }
}

Write-Host "creating venv and installing deps via $PipIndex (~2-4GB, be patient)..."
if ($Python -eq 'py') {
  & py -3 -m venv $Venv
} else {
  & $Python -m venv $Venv
}
& $Pip install --upgrade pip >> $Log 2>&1
& $Pip install --index-url $PipIndex `
  'iopaint==1.6.0' `
  'transformers==4.48.3' `
  'opencv-python==4.11.0.86' `
  'scikit-image==0.24.0' >> $Log 2>&1
if ($LASTEXITCODE -ne 0) {
  Write-Error "pip install failed; see $Log"
  exit 1
}

Get-Model
Check-Env
Write-Host "project inpaint env ready: $Iopaint"

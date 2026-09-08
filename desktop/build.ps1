$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

function Require-Cmd($name, $hint) {
  if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
    Write-Host "[ERROR] '$name' not found. $hint" -ForegroundColor Red
    exit 1
  }
}

Write-Host '[check] build prerequisites...' -ForegroundColor Cyan
Require-Cmd 'node' 'Install Node.js 20+ from https://nodejs.org/'
Require-Cmd 'cargo' 'Install Rust stable from https://rustup.rs/'
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere) -or -not (& $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)) {
  Write-Host '[ERROR] Visual Studio Build Tools with the "Desktop development with C++" workload not found.' -ForegroundColor Red
  Write-Host '        Install from: https://visualstudio.microsoft.com/visual-cpp-build-tools/'
  exit 1
}
if (-not (Get-Command 'pnpm' -ErrorAction SilentlyContinue)) {
  if (Get-Command 'corepack' -ErrorAction SilentlyContinue) {
    Write-Host '[setup] enabling pnpm via corepack...'
    corepack enable pnpm
  } else {
    Write-Host "[ERROR] 'pnpm' not found. Run: npm install -g pnpm" -ForegroundColor Red
    exit 1
  }
}

Write-Host '[1/3] pnpm install...' -ForegroundColor Cyan
pnpm install
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Host '[2/3] pnpm tauri build (5-10 min on first run)...' -ForegroundColor Cyan
pnpm tauri build
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Host '[3/3] done. Installer at:' -ForegroundColor Green
Get-ChildItem "$PSScriptRoot\src-tauri\target\release\bundle\nsis\*.exe" | ForEach-Object { Write-Host "  $($_.FullName)" }

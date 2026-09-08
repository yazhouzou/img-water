#!/usr/bin/env bash
# 一键打包桌面应用（macOS DMG / Windows NSIS 安装包），产物统一输出到项目根目录 dist/
#
# 用法:
#   tools/package-app.sh mac
#       在 macOS 上构建 DMG（自动处理 ~/.zshrc 旧编译变量污染和 DMG 架构命名）
#   tools/package-app.sh win
#       在 Windows 的 Git Bash 里运行时本机构建 NSIS 安装包
#   tools/package-app.sh win --remote user@host --win-repo 'C:/code/remove_watermark'
#       在 macOS 上运行时，通过 SSH 触发 Windows 机器构建（自动 git pull）并拉回安装包
#   tools/package-app.sh both --remote user@host --win-repo 'C:/code/remove_watermark'
#       一同打包：macOS 本机构建 + 远程 Windows 构建
#
# Windows 机器需要: 开启 OpenSSH 服务器（设置 -> 可选功能）、克隆好仓库、装好
# Node/Rust/VS Build Tools（详见 desktop/README.md）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONF="$ROOT/desktop/src-tauri/tauri.conf.json"
DIST="$ROOT/dist"
VERSION="$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$CONF" | head -n 1)"
OS="$(uname -s)"
ARCH="$(uname -m)"
REMOTE=""
WIN_REPO=""

die() { echo "[ERROR] $*" >&2; exit 1; }
usage() { grep '^#' "$0" | sed 's/^# \{0,1\}//; 1d'; exit "${1:-1}"; }

build_mac() {
  [[ "$OS" == Darwin ]] || die "macOS 打包需要在 macOS 上执行"
  "$ROOT/desktop/build.sh"
  local dmg_dir="$ROOT/desktop/src-tauri/target/release/bundle/dmg"
  local x64="$dmg_dir/doubao-watermark-remover_${VERSION}_x64.dmg"
  local arm="$dmg_dir/doubao-watermark-remover_${VERSION}_aarch64.dmg"
  if [[ "$ARCH" == arm64 && -f "$x64" ]]; then
    mv -f "$x64" "$arm"
  fi
  local dmg="$arm"
  [[ -f "$dmg" ]] || dmg="$x64"
  [[ -f "$dmg" ]] || die "未找到 DMG 产物: $dmg_dir"
  mkdir -p "$DIST"
  cp -f "$dmg" "$DIST/"
  echo "[OK] macOS 产物: $DIST/$(basename "$dmg")"
}

build_win_local() {
  echo "[build] Windows 本机构建..."
  (cd "$ROOT/desktop" && powershell.exe -NoProfile -ExecutionPolicy Bypass -File build.ps1)
  mkdir -p "$DIST"
  local copied=0
  for exe in "$ROOT"/desktop/src-tauri/target/release/bundle/nsis/*-setup.exe; do
    [[ -f "$exe" ]] || continue
    cp -f "$exe" "$DIST/"
    echo "[OK] Windows 产物: $DIST/$(basename "$exe")"
    copied=1
  done
  [[ "$copied" == 1 ]] || die "未找到 NSIS 安装包"
}

build_win_remote() {
  [[ -n "$REMOTE" ]] || die "macOS 上构建 Windows 包需要 --remote user@host（或在 Windows 的 Git Bash 里运行本脚本）"
  [[ -n "$WIN_REPO" ]] || die "需要 --win-repo 'C:/path/to/repo' 指定 Windows 机器上的仓库路径"
  local repo="${WIN_REPO//\\//}"
  echo "[ssh] 远程构建: $REMOTE ($repo)"
  ssh "$REMOTE" "powershell -NoProfile -Command \"Set-Location '$repo'; git pull --ff-only; & '$repo/desktop/build.ps1'\""
  echo "[ssh] 拉取安装包..."
  local exes
  exes="$(ssh "$REMOTE" "powershell -NoProfile -Command \"(Get-ChildItem '$repo/desktop/src-tauri/target/release/bundle/nsis/*.exe').FullName\"")"
  mkdir -p "$DIST"
  local copied=0
  while IFS= read -r exe; do
    [[ -z "$exe" ]] && continue
    scp -q "$REMOTE:$exe" "$DIST/"
    echo "[OK] Windows 产物: $DIST/$(basename "$exe")"
    copied=1
  done <<< "$exes"
  [[ "$copied" == 1 ]] || die "远程未找到 NSIS 安装包"
}

is_windows_shell() {
  [[ "$OS" == MINGW* || "$OS" == MSYS* || "$OS" == CYGWIN* ]]
}

cmd="${1:-}"
[[ $# -gt 0 ]] && shift
while [[ $# -gt 0 ]]; do
  case "$1" in
    --remote) [[ $# -ge 2 ]] || die "--remote 需要参数 user@host"; REMOTE="$2"; shift 2 ;;
    --win-repo) [[ $# -ge 2 ]] || die "--win-repo 需要参数"; WIN_REPO="$2"; shift 2 ;;
    -h|--help) usage 0 ;;
    *) die "未知参数: $1（用 -h 查看用法）" ;;
  esac
done

case "$cmd" in
  mac) build_mac ;;
  win)
    if is_windows_shell; then build_win_local; else build_win_remote; fi
    ;;
  both)
    if is_windows_shell; then die "Windows 上无法构建 macOS 包；请在 macOS 上执行 both"; fi
    build_mac
    build_win_remote
    ;;
  *) usage 1 ;;
esac

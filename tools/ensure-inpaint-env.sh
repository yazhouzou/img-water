#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENV="$ROOT/.img-inpaint-venv"
PYTHON="$VENV/bin/python"
IOPAINT="$VENV/bin/iopaint"
LOG="$VENV/install.log"
PIP_INDEX_URL="${PIP_INDEX_URL:-https://mirrors.aliyun.com/pypi/simple/}"
MODEL_URL="${LAMA_MODEL_URL:-https://github.com/Sanster/models/releases/download/add_big_lama/big-lama.pt}"
MODEL_PATH="${HOME}/.cache/torch/hub/checkpoints/big-lama.pt"

mkdir -p "$VENV" "$(dirname "$MODEL_PATH")"

check() {
  if ! "$IOPAINT" list >/dev/null 2>>"$LOG"; then
    printf 'project inpaint env check failed; see %s\n' "$LOG" >&2
    exit 1
  fi
}

download_model() {
  if [[ -s "$MODEL_PATH" ]]; then
    return 0
  fi
  printf 'downloading LaMa model (~200MB): %s\n' "$MODEL_URL"
  if ! curl -L --retry 5 --retry-delay 5 --connect-timeout 60 --continue-at - \
    -o "$MODEL_PATH" "$MODEL_URL" 2>>"$LOG"; then
    printf 'model download failed; see %s\n' "$LOG" >&2
    exit 1
  fi
}

if [[ -x "$IOPAINT" ]]; then
  download_model
  check
  printf 'project inpaint env ready: %s\n' "$IOPAINT"
  exit 0
fi

if ! command -v python3 >/dev/null 2>&1; then
  printf 'python3 not found; install Python 3.10+ first (macOS: xcode-select --install)\n' >&2
  exit 1
fi

printf 'creating venv and installing deps via %s (~2-4GB, be patient)...\n' "$PIP_INDEX_URL"
python3 -m venv "$VENV"
"$PYTHON" -m pip install --upgrade pip >"$LOG" 2>&1
PIP_INDEX_URL="$PIP_INDEX_URL" "$PYTHON" -m pip install \
  "iopaint==1.6.0" \
  "transformers==4.48.3" \
  "opencv-python==4.11.0.86" \
  "scikit-image==0.24.0" >>"$LOG" 2>&1

download_model
check
printf 'project inpaint env ready: %s\n' "$IOPAINT"

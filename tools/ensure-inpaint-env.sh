#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENV="$ROOT/.img-inpaint-venv"
PYTHON="$VENV/bin/python"
IOPAINT="$VENV/bin/iopaint"
LOG="$VENV/install.log"

if [[ -x "$IOPAINT" ]]; then
  if ! "$IOPAINT" list >/dev/null 2>>"$LOG"; then
    printf 'project inpaint env check failed; see %s\n' "$LOG" >&2
    exit 1
  fi
  printf 'project inpaint env ready: %s\n' "$IOPAINT"
  exit 0
fi

python3 -m venv "$VENV"
"$PYTHON" -m pip install --upgrade pip >"$LOG" 2>&1
"$PYTHON" -m pip install \
  "iopaint==1.6.0" \
  "transformers==4.48.3" \
  "opencv-python==4.11.0.86" \
  "scikit-image==0.24.0" >>"$LOG" 2>&1

if ! "$IOPAINT" list >/dev/null 2>>"$LOG"; then
  printf 'project inpaint env check failed; see %s\n' "$LOG" >&2
  exit 1
fi
printf 'project inpaint env ready: %s\n' "$IOPAINT"

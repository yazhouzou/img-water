#!/usr/bin/env bash
# Build the desktop app, neutralizing legacy shell vars from ~/.zshrc
# (old MacPorts config: -arch i386/-arch x86_64 + MACOSX_DEPLOYMENT_TARGET=10.6)
# which otherwise corrupt objc2-exception-helper's static archive on Apple Silicon.
set -euo pipefail

unset CFLAGS CXXFLAGS CCFLAGS LDFLAGS MACOSX_DEPLOYMENT_TARGET

cd "$(dirname "${BASH_SOURCE[0]}")"
exec pnpm tauri build "$@"

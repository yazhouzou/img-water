#!/usr/bin/env bash
# Dev-run the desktop app, neutralizing legacy shell vars from ~/.zshrc
# (old MacPorts config: -arch i386/-arch x86_64 + MACOSX_DEPLOYMENT_TARGET=10.6)
# which otherwise break the Objective-C build (mac-notification-sys/objc2)
# on Apple Silicon. Same rationale as build.sh.
set -euo pipefail

unset CFLAGS CXXFLAGS CCFLAGS LDFLAGS MACOSX_DEPLOYMENT_TARGET

cd "$(dirname "${BASH_SOURCE[0]}")"
exec pnpm tauri dev "$@"

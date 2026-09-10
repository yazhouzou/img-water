#!/usr/bin/env bash
# 本地 Android target 编译预检（无需 CI，秒级发现 Android 专属编译错误）
# 依赖：brew 安装的 android-commandlinetools + openjdk@17 + NDK 26.3.11579264（见 ~/.zshrc 环境变量）
set -euo pipefail
cd "$(dirname "$0")/../desktop/src-tauri"

export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home}"
export ANDROID_HOME="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"
export NDK_HOME="${NDK_HOME:-$ANDROID_HOME/ndk/26.3.11579264}"
export PATH="$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin:$PATH"

env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET \
  cargo check --quiet --target aarch64-linux-android

echo "Android target cargo check 通过"

#!/usr/bin/env bash
# 本地 Android target 编译预检（无需 CI，秒级发现 Android 专属编译错误）
# 依赖：brew 安装的 android-commandlinetools + openjdk@17 + NDK 26.3.11579264（见 ~/.zshrc 环境变量）
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/../desktop/src-tauri"

export JAVA_HOME="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home}"
BREW_NDK="/opt/homebrew/share/android-commandlinetools/ndk/26.3.11579264"
export NDK_HOME="${NDK_HOME:-$BREW_NDK}"
if [ ! -x "$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android21-clang" ]; then
  echo "错误：NDK 不存在或不含工具链：$NDK_HOME（应 brew 安装 android-commandlinetools）" >&2
  exit 1
fi
export PATH="$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin:$PATH"

TOOLCHAIN_BIN="$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN_BIN/aarch64-linux-android21-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_AR="$TOOLCHAIN_BIN/llvm-ar"
export CC_aarch64_linux_android="$TOOLCHAIN_BIN/aarch64-linux-android21-clang"
export CXX_aarch64_linux_android="$TOOLCHAIN_BIN/aarch64-linux-android21-clang++"
export AR_aarch64_linux_android="$TOOLCHAIN_BIN/llvm-ar"

env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET \
  cargo check --quiet --target aarch64-linux-android

echo "Android target cargo check 通过"

# 测试代码也要能在 android target 编译（不运行，运行需真机）。
# ort 静态库按更高 API level 编译，测试可执行文件链接时缺少量 bionic 符号，
# 用空符号桩满足链接（见 tools/android-test-stubs.c；APK cdylib 不受影响）。
STUBS_SRC="$SCRIPT_DIR/android-test-stubs.c"
STUBS_OBJ="${TMPDIR:-/tmp}/android-test-stubs.o"
"$TOOLCHAIN_BIN/aarch64-linux-android21-clang" -c "$STUBS_SRC" -o "$STUBS_OBJ"
env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET \
  RUSTFLAGS="-C link-arg=$STUBS_OBJ" \
  cargo test --quiet --no-run --target aarch64-linux-android 2>&1 | tail -3

echo "Android target 测试代码编译通过"

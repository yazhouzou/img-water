#!/usr/bin/env bash
# 一键发布新版本：本地预检 → 改版本号 → 提交推送双远端 → 打 tag 触发 CI 自动发布 Release
# 用法: ./tools/release.sh 0.3.0
set -euo pipefail

VER="${1:?用法: ./tools/release.sh <版本号，如 0.3.0>}"
TAG="v$VER"
cd "$(dirname "$0")/.."

if ! echo "$VER" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "版本号格式应为 x.y.z，收到: $VER"
  exit 1
fi

if [ "${SKIP_CHECK:-0}" != "1" ]; then
  echo "[1/4] 本地预检 cargo check（SKIP_CHECK=1 可跳过）…"
  (cd desktop/src-tauri && env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET cargo check --quiet) \
    && echo "  cargo check 通过" || { echo "  cargo check 失败，中止"; exit 1; }
fi

echo "[2/4] 更新版本号 → $VER"
perl -pi -e "s/\"version\": \"[0-9.]+\"/\"version\": \"$VER\"/" desktop/src-tauri/tauri.conf.json

git add desktop/src-tauri/tauri.conf.json
if ! git diff --cached --quiet; then
  git commit -m "Release $TAG"
fi

echo "[3/4] 推送 main 到双远端"
git push github main
git push origin main

echo "[4/4] 推送 tag $TAG（触发 CI 构建并自动发布）"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "  本地已存在 tag $TAG，跳过创建"
else
  git tag "$TAG"
fi
git push github "$TAG"

echo
echo "完成。CI 后台构建约 10-15 分钟，产物将自动出现在："
echo "  https://github.com/yazhouzou/img-water/releases/tag/$TAG"
echo "若只有部分产物或 release 步骤失败：到 Actions 对应 run 页面点 Re-run failed jobs"
echo "（只重跑附加步骤约 1 分钟，无需重新构建）。"

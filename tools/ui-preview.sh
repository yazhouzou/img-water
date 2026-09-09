#!/usr/bin/env bash
# 本地浏览器联调 UI（无需 Android SDK）
# 用法: ./tools/ui-preview.sh [端口号，默认 8765]
set -euo pipefail
PORT="${1:-8765}"
cd "$(dirname "$0")/../desktop/ui"

echo "UI 联调地址："
echo "  移动端视图  http://localhost:$PORT/index.html?mobile=1"
echo "  桌面视图    http://localhost:$PORT/index.html?desktop=1"
echo "  模拟未下载模型 http://localhost:$PORT/index.html?mobile=1&nomodel=1"
echo "提示：按 F12 打开 DevTools，切换设备工具栏可模拟手机屏宽；改 ui/*.css|js|html 后直接刷新即可"
echo "Ctrl+C 退出"
open "http://localhost:$PORT/index.html?mobile=1" || true
python3 -m http.server "$PORT"

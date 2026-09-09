#!/usr/bin/env bash
# 本地浏览器联调 UI（无需 Android SDK）
# 用法: ./tools/ui-preview.sh [端口号，默认 8765]
# 内置 HTTP 服务禁用缓存（Cache-Control: no-store），改完 CSS/JS 刷新即生效，不会出现新旧混搭
set -euo pipefail
PORT="${1:-8765}"
cd "$(dirname "$0")/../desktop/ui"

echo "UI 联调地址："
echo "  移动端视图  http://localhost:$PORT/index.html?mobile=1"
echo "  桌面视图    http://localhost:$PORT/index.html?desktop=1"
echo "  模拟未下载模型 http://localhost:$PORT/index.html?mobile=1&nomodel=1"
echo "提示：F12 打开 DevTools，设备工具栏可模拟手机屏宽；若端口被占用，先 Ctrl+C 旧服务或换端口"
echo "Ctrl+C 退出"
open "http://localhost:$PORT/index.html?mobile=1" || true
PORT="$PORT" python3 - <<'PY'
import os, http.server, socketserver
PORT = int(os.environ["PORT"])

class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=os.getcwd(), **kwargs)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("", PORT), Handler) as httpd:
    httpd.serve_forever()
PY

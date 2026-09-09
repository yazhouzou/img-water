# 豆包水印清理助手

批量去除 PNG 图片右下角“豆包AI生成”水印。Rust + ONNX Runtime 内核（无需 Python），支持 macOS / Windows / Android。

## 下载安装（免登录，永久有效）

**最新版**：https://github.com/yazhouzou/img-water/releases/latest

| 平台 | 文件 | 大小 |
|---|---|---|
| Android（arm64） | `doubao-watermark-remover-*-android.apk` | ~276MB |
| Windows x64 | `doubao-watermark-remover-*-windows-x64-setup.exe` | ~15MB |
| macOS Apple Silicon | `doubao-watermark-remover-*-macos-aarch64.dmg` | ~21MB |

首次启动点击“一键下载修复模型”（约 200MB，国内镜像），之后离线可用。

## 打包命令速查

| 目标 | 在哪执行 | 命令 | 产物 |
|---|---|---|---|
| **macOS 安装包** | 本机 (macOS) | `tools/package-app.sh mac` | `dist/doubao-watermark-remover_*_aarch64.dmg` |
| **Windows 安装包**（远程） | 本机 (macOS) | `tools/package-app.sh win --remote user@host --win-repo 'C:/path/to/repo'` | `dist/*_x64-setup.exe` |
| **Windows 安装包**（本机） | Windows 机器 Git Bash | `tools/package-app.sh win` | `dist/*_x64-setup.exe` |
| **mac + win 一同打包** | 本机 (macOS) | `tools/package-app.sh both --remote user@host --win-repo 'C:/path/to/repo'` | `dist/` 下两件 |
| **Windows 安装包**（无 Windows 机器，推荐） | 本机 (macOS) | `tools/package-app.sh ci` | GitHub Actions artifacts |
| **Android APK** | 无需本机环境 | `git push github main`（或 `tools/package-app.sh ci`） | GitHub Actions artifacts |

- 所有本地打包产物统一输出到项目根目录 **`dist/`**
- `--remote` 需要 Windows 机器开启 OpenSSH 服务器并克隆好仓库（详见 `desktop/README.md`）
- Android APK：推送后打开 https://github.com/yazhouzou/img-water/actions → 最新 run → **Artifacts** 下载（APK 70MB / Windows exe 15MB / macOS dmg 21MB）

## 发新版本

1. 改 `desktop/src-tauri/tauri.conf.json` 的 `version`（如 `0.1.0` → `0.2.0`），提交推送
2. 等 CI 构建完成，从 Actions artifacts 下载三个产物
3. 发布 Release（两种方式）：
   - 网页：Releases → Draft a new release → 新建 tag（如 `v0.2.0`）→ 拖入产物 → Publish
   - API：需要 fine-grained token（Contents 读写），用 `POST /repos/yazhouzou/img-water/releases` + `uploads.github.com` 上传，发布后立即撤销 token
4. `releases/latest` 会自动指向最新版，README 下载地址无需改动

## 日常使用

```bash
# 终端一键处理（需要先初始化 Python 回退环境）
./tools/remove_doubao_watermark.py run [文件列表]

# 或使用图形界面（推荐，普通用户零环境）
cd desktop && pnpm install && pnpm tauri dev
```

应用首次启动点击“一键下载修复模型”（约 200MB，国内镜像），之后离线可用。

## 环境变量（可选）

| 变量 | 作用 | 默认 |
|---|---|---|
| `LAMA_ONNX_URL` | 模型下载地址 | hf-mirror 国内镜像 |
| `LAMA_ONNX_PATH` | 模型存放位置 | `<项目>/.models/lama_fp32.onnx` |
| `LAMA_ONNX_DIR` | 模型目录 | `<项目>/.models/` |
| `DOUBAO_WATERMARK_WORKDIR` | 流水线临时目录 | `/tmp/doubao-watermark-work` |

## 开发调试

```bash
cd desktop && pnpm install && pnpm tauri dev   # 桌面端调试
cd desktop && env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET cargo build --bin clean-cli
# clean-cli 参数与 Python 脚本一致：--root <文件夹> --mask-box x1,y1,x2,y2 run
```

> 本机直接跑 cargo 必须先 unset 上述变量（`~/.zshrc` 旧 MacPorts 配置会破坏编译），macOS 打包请统一用 `tools/package-app.sh mac` 或 `desktop/build.sh`。

## 目录结构

```
tools/                # Python 流水线（终端回退方案）+ 打包脚本
desktop/              # Tauri 应用（Rust ONNX 内核 + 前端）
  src-tauri/src/      #   lama.rs 推理内核 / pipeline.rs 流水线 / app.rs 命令
  build.sh            #   macOS 一键构建
  build.ps1           #   Windows 一键构建
dist/                 # 本地打包产物输出目录
```

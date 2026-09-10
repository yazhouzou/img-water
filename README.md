# 豆包水印清理助手

批量去除 PNG 图片右下角“豆包AI生成”水印。Rust + ONNX Runtime 内核（无需 Python），支持 macOS / Windows / Android。

## 下载安装（免登录，永久有效）

**最新版**：https://github.com/yazhouzou/img-water/releases/latest

| 平台 | 文件 | 大小 |
|---|---|---|
| Android（arm64） | `doubao-watermark-remover-*-android.apk` | ~276MB |
| Windows x64 | `doubao-watermark-remover-*-windows-x64-setup.exe` | ~15MB |
| macOS Apple Silicon | `doubao-watermark-remover-*-macos-aarch64.dmg` | ~21MB |

首次启动自动下载修复模型（约 200MB，国内镜像多连接加速），之后离线可用。

## 打包命令速查

| 目标 | 在哪执行 | 命令 |
|---|---|---|
| macOS 安装包 | 本机 (macOS) | `tools/package-app.sh mac` |
| Windows 安装包 | Windows 机器 Git Bash | `tools/package-app.sh win` |
| 无 Windows 机器 | 本机 (macOS) | `tools/package-app.sh ci`（走 CI，产物在 Release） |

- 本地打包产物统一输出到项目根目录 **`dist/`**
- 正式发布一律用下面的发版脚本，产物自动附加到 Release（免登录下载）

## 日常使用

```bash
# 终端一键处理（需要先初始化 Python 回退环境）
./tools/remove_doubao_watermark.py run [文件列表]

# 或使用图形界面（推荐，普通用户零环境）
cd desktop && pnpm install && pnpm tauri dev
```

- 遮罩自动检测：脚本在图片右下角搜索“豆包AI生成”白字水印并自动生成遮罩，位置变化也能处理；检测不到时回退内置尺寸规则（支持 2848x1600 / 2278x1280 / 2048x2048）
- 特殊水印可手动指定遮罩：`./tools/remove_doubao_watermark.py --mask-box=x1,y1,x2,y2 run ...`（负数表示距右下角偏移）

## 发新版本

```bash
./tools/release.sh 0.3.0
```

自动完成：本地预检 → 改 `tauri.conf.json` 版本号 → 提交并推送双远端 → 打 tag 触发 CI。约 10-15 分钟后产物自动出现在 [Releases](https://github.com/yazhouzou/img-water/releases/latest)（手动方式等效：改版本号提交推送 + `git tag vX.Y.Z && git push github vX.Y.Z`）。

偶发失败处理：若 Release 缺少部分产物（构建均成功、仅附加步骤失败），到 Actions 对应 run 页面点 **Re-run failed jobs**，只重跑附加步骤约 1 分钟，无需重新构建。

## 环境变量（可选）

| 变量 | 作用 | 默认 |
|---|---|---|
| `LAMA_ONNX_URL` | 模型下载地址 | hf-mirror 国内镜像 |
| `LAMA_ONNX_PATH` | 模型存放位置 | `<项目>/.models/lama_fp32.onnx` |
| `DOUBAO_WATERMARK_WORKDIR` | 流水线临时目录 | `/tmp/doubao-watermark-work` |

## 开发调试

```bash
cd desktop && pnpm install && pnpm tauri dev   # 桌面端调试（cargo 需先 unset ~/.zshrc 的 MacPorts 变量）
./tools/ui-preview.sh                          # UI 浏览器联调（零 SDK，?mobile=1 移动端视图）
./tools/android-check.sh                       # Android target 编译预检
adb devices && cd desktop && pnpm tauri android dev   # 真机 USB 调试热重载
```

## 目录结构

```
tools/                # Python 流水线（终端回退方案）+ 打包脚本
desktop/              # Tauri 应用（Rust ONNX 内核 + 前端）
  src-tauri/src/      #   lama.rs 推理内核 / pipeline.rs 流水线 / app.rs 命令
  build.sh            #   macOS 一键构建
  build.ps1           #   Windows 一键构建
dist/                 # 本地打包产物输出目录
```

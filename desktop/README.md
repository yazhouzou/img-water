# 豆包水印清理助手（桌面端）

基于 Tauri v2 的跨平台桌面应用，内置 Rust + ONNX Runtime 修复内核（无需 Python），批量去除 PNG 右下角“豆包AI生成”水印。

## 架构（ONNX 内核，v0.2 起）

- Rust 端直接用 `ort`（ONNX Runtime）推理 LaMa 模型：遮罩 bbox → 512×512 窗口 → 推理 → 合成回原图
- 首次使用只需下载约 200MB 的 `lama_fp32.onnx` 模型（hf-mirror，支持断点续传，`LAMA_ONNX_URL`/`LAMA_ONNX_PATH` 可覆盖），无 Python / PyTorch / iopaint
- 流水线（备份 → 遮罩 → 修复 → 覆盖 → 复查 → 清理）完全在 Rust 进程内执行；CLI 兼容命令见 `clean-cli`（`cargo build --bin clean-cli`）
- 终端用户仍可用旧 Python 流水线 `tools/remove_doubao_watermark.py`（保留作回退，需要 `tools/ensure-inpaint-env.sh`）

## 功能

- 选择任意图片文件夹，自动列出 PNG
- 勾选需要处理的图片，一键批量处理（备份 → 遮罩 → LaMa 修复 → 覆盖 → 复查）
- 界面内直接预览“修复候选复查图”和“落盘最终复查图”
- 可选保留备份与 `/tmp`（Windows 下为系统临时目录）产物，人工确认后一键清理

## 前置条件

- Node.js 20+ 与 pnpm（用于 Tauri CLI）
- Rust stable（macOS：`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`）
- 项目内修复环境：在仓库根目录运行 `./tools/ensure-inpaint-env.sh`（macOS/Linux）或 `tools/ensure-inpaint-env.ps1`（Windows），或直接在应用内点“一键初始化修复环境”
- LaMa 模型缓存：`~/.cache/torch/hub/checkpoints/big-lama.pt`（初始化脚本会自动断点续传下载；缺失时首次 `inpaint` 也会自动下载）

## Android / 移动端（代码就绪，待真机验证）

Rust 内核与流水线可直接交叉编译到 Android（Tauri v2 mobile），移动端适配已就绪：

- 路径重定向：模型放应用数据目录 `app_data_dir/models/`，工作目录放 `app_cache_dir/`（`MODEL_DIR_OVERRIDE`/`WORKDIR_OVERRIDE`，桌面行为不变）
- 文件访问：移动端“选择图片”（可多选）→ 拷入 `app_data_dir/imports/<时间戳>/` → 复用同一流水线 → 结果回写到该目录
- UI：移动端单列布局，“选文件夹”自动切换为“选择图片”

待办：Android 构建需要 SDK/NDK（本机未安装，按要求不装），后续走 CI 构建 APK 并真机验证（模型 200MB 下载、512 窗口推理内存/速度需真机确认）。

## 一键在线下载模型（小体积分发）

应用检测到 `.models/lama_fp32.onnx` 缺失时，右上角会出现“一键下载修复模型”按钮：

- 默认从 hf-mirror 下载约 200MB（`LAMA_ONNX_URL` 可覆盖）
- 进度实时显示在日志区，完成后环境徽章自动刷新
- 模型存放位置可用 `LAMA_ONNX_PATH` 覆盖；默认在项目根目录 `.models/`

## 开发调试

```bash
cd desktop
pnpm install
pnpm tauri dev
```

## 构建

```bash
./desktop/build.sh            # macOS（推荐，自动屏蔽 ~/.zshrc 里的旧 MacPorts 编译变量）
cd desktop && pnpm tauri build  # 等价原始命令；但本机 ~/.zshrc 的 CFLAGS 会破坏构建
```

原因：`~/.zshrc` 中的 `CFLAGS=-arch i386 -arch x86_64` 和 `MACOSX_DEPLOYMENT_TARGET=10.6`（旧 MacPorts 配置）会让 `objc2-exception-helper` 编译出错误架构的 fat 静态库导致构建失败。`build.sh` 已自动 `unset` 这些变量。

macOS Intel 版本：

```bash
source "$HOME/.cargo/env" && rustup target add x86_64-apple-darwin
cd desktop && env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET pnpm tauri build --target x86_64-apple-darwin
```

macOS 产物：`src-tauri/target/release/bundle/dmg/*.dmg`

## 一键打包（推荐）

统一入口 `tools/package-app.sh`，产物输出到项目根目录 `dist/`：

```bash
tools/package-app.sh mac        # 在 macOS 上构建 DMG
tools/package-app.sh win        # 在 Windows 的 Git Bash 里本机构建 NSIS
tools/package-app.sh win --remote user@host --win-repo 'C:/code/remove_watermark'
                                # 在 macOS 上远程触发 Windows 机器构建（自动 git pull）并拉回安装包
tools/package-app.sh both --remote user@host --win-repo 'C:/code/remove_watermark'
                                # 一同打包：macOS 本机 + 远程 Windows
```

远程 Windows 机器需要：开启 OpenSSH 服务器（设置 → 可选功能 → OpenSSH 服务器）、克隆好仓库并装好构建前置（Node/Rust/VS Build Tools）。

## Windows 本地构建

在一台 Windows 10/11 机器上（无法从 macOS 交叉编译）：

1. 安装前置组件：
   - Node.js 20+：https://nodejs.org/
   - Rust stable：https://rustup.rs/
   - Visual Studio Build Tools，勾选“使用 C++ 的桌面开发”工作负载：https://visualstudio.microsoft.com/visual-cpp-build-tools/
2. 拉取仓库后执行：

   ```powershell
   git clone <仓库地址>
   cd <仓库>\desktop
   .\build.ps1
   ```

   `build.ps1` 会自动检查 node/cargo/pnpm/VS C++ 工作负载（pnpm 缺失时尝试 corepack 启用），然后 `pnpm install && pnpm tauri build`。Windows 下只出 NSIS 安装包（`tauri.windows.conf.json` 覆盖，避免 MSI 需在线下载 WiX 工具链导致失败）。
3. 产物：`src-tauri\target\release\bundle\nsis\doubao-watermark-remover_0.1.0_x64-setup.exe`
4. 分发时连同仓库一起给目标用户（小安装包 + 在线初始化运行时依赖）；目标机器运行应用后点“一键初始化修复环境”即可（需 Python 3.10+，脚本会自动创建 venv 并走国内 PyPI 镜像下载依赖和 LaMa 模型）

## 分发注意事项（当前里程碑）

- 发布包内的 Rust 外壳已支持在安装目录附近查找 `tools/remove_doubao_watermark.py`
- 修复环境 `.img-inpaint-venv/`（含 PyTorch）体积较大，当前版本不在安装包内捆绑；目标机器用应用内“一键初始化修复环境”或初始化脚本在线拉取（走国内镜像）
- 注意：在目标机器上执行 `pnpm tauri build` 不可行——需要完整 Node/Rust/Xcode/VS 工具链（数 GB、编译 10 分钟以上、易失败），且省不掉运行时的 Python/PyTorch/模型。小体积分发 = 小安装包 + 首次运行在线初始化运行时依赖；未来最优解是 ONNX 内核（免 Python，预计安装包 ~200MB）
- Windows 构建采用本地构建方案：在一台 Windows 机器上执行 `desktop\build.ps1`，产物为 NSIS 安装包；备选方案是 GitHub Actions（`.github/workflows/desktop-build.yml`）或云效 Flow

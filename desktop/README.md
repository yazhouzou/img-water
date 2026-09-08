# 豆包水印清理助手（桌面端）

基于 Tauri v2 的跨平台桌面应用，复用项目内 `tools/remove_doubao_watermark.py` 流水线，批量去除 PNG 右下角“豆包AI生成”水印。

## 功能

- 选择任意图片文件夹，自动列出 PNG
- 勾选需要处理的图片，一键批量处理（备份 → 遮罩 → LaMa 修复 → 覆盖 → 复查）
- 界面内直接预览“修复候选复查图”和“落盘最终复查图”
- 可选保留备份与 `/tmp`（Windows 下为系统临时目录）产物，人工确认后一键清理

## 前置条件

- Node.js 20+ 与 pnpm（用于 Tauri CLI）
- Rust stable（macOS：`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`）
- 项目内修复环境：在仓库根目录运行 `./tools/ensure-inpaint-env.sh`
- LaMa 模型缓存：`~/.cache/torch/hub/checkpoints/big-lama.pt`（缺失时首次 `inpaint` 会自动下载）

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
Windows 产物（需在 Windows 机器或 CI 上构建）：`src-tauri/target/release/bundle/nsis/*.exe`

## 分发注意事项（当前里程碑）

- 发布包内的 Rust 外壳已支持在安装目录附近查找 `tools/remove_doubao_watermark.py`
- 修复环境 `.img-inpaint-venv/`（含 PyTorch）体积较大，当前版本不在安装包内捆绑；目标机器需执行 `./tools/ensure-inpaint-env.sh` 或后续引入捆绑方案
- Windows 构建请使用 GitHub Actions（`.github/workflows/desktop-build.yml`），在 macOS 上无法交叉编译 Windows 安装包

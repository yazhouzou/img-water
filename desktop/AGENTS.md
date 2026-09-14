# desktop/AGENTS.md — 桌面端/App 细节（按需加载）

本文件只在改 `desktop/`（Tauri 应用、Rust 内核、CI、打包）时才需要读。根 `AGENTS.md` 保持精简并指向这里。

## 定位
项目提供 Tauri 桌面应用（`desktop/`），内置 Rust + ONNX Runtime 修复内核（v0.2 起，无需 Python）。旧 Python 流水线（`tools/remove_doubao_watermark.py` + `.img-inpaint-venv/`）保留作终端回退方案，不再被桌面端依赖。

## Rust 内核（`desktop/src-tauri/src/lama.rs`）
- 用 `ort` 推理 LaMa ONNX 模型（`.models/lama_fp32.onnx`，约 200MB）；输入固定 [batch,3,512,512]（ONNX 导出时 H/W 静态，实测与 JIT big-lama 输出逐像素一致、无质量阉割；输入 0..1、输出 0..255、mask 二值不敏感）。
- v0.5.1 起推理策略对齐 iopaint CROP：每个遮罩连通块 crop bbox+128px margin（`crop_box()` 贴边补偿同 iopaint）；crop ≤512 原分辨率居中 pad 到 512 推理，更大时等比缩放到 512 推理后 Lanczos 还原写回。取代 v0.3.9 的 tile 分块（tile 每块只看半截水印导致接缝与模糊，是"App 端部分图模糊"根因之一）；图像源函数内 clone 自当前已修复结果（级联）。
- **pad 区必须用图像均值色常数填充，禁止反射 pad**：反射会把贴近 crop 边缘的水印文字镜像进模型上下文，模型照字形延续产生残影（v0.5.3 排查结论；水印贴图底时 mask 必然贴 crop 边，此坑必现；iopaint 原分辨率推理 pad 仅 ≤7px 故无此问题）。
- FFT 算子不可导出 ONNX（`aten::fft_rfftn` 不支持），Carve 固定 512 正因 FFT 在固定尺寸下可预计算为矩阵乘——动态尺寸 ONNX 导出已验证不可行。

## 流水线 / CLI
- `desktop/src-tauri/src/pipeline.rs` 是 `tools/remove_doubao_watermark.py` 的 Rust 移植（备份/遮罩/修复/复查/覆盖/清理），进程内调用；`run` 模式清理前会把两张复查拼图复制到系统临时目录 `doubao-watermark-review/` 再输出路径，避免被清理后失效。
- CLI `clean-cli`（`cargo build --bin clean-cli`），参数与 Python 脚本一致（`--root`/`--mask-box`/`--keep-work` + `run|prepare|inpaint|review-lama|overwrite-review|cleanup`）。
- 模型下载：启动检测到缺失即自动下载（多连接分段+分段重试+源回退 hf-mirror→huggingface），顶栏进度条；`LAMA_ONNX_URL` 覆盖下载源，`LAMA_ONNX_PATH` 覆盖模型位置。

## 开发 / 打包
- `cd desktop && pnpm install && pnpm tauri dev`。直接跑 `cargo build`/`cargo check` 必须先 `env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET`（`.zshrc` 旧 MacPorts 变量会破坏 `objc2-exception-helper` 编译），或统一用 `desktop/build.sh`。
- Windows 安装包无法在 macOS 交叉编译；一键打包 `tools/package-app.sh`：`mac`（本机 DMG）、`win`（Windows Git Bash 本机构建，或 macOS 上 `--remote user@host --win-repo C:/path` 经 SSH 触发远程 Windows 构建并拉回）、`both`；产物统一在根 `dist/`。

## Android
- 代码层适配已就绪（路径重定向到应用目录、相册 `content://` URI 经 JNI/ContentResolver 拷贝导入见 `android_uri.rs`、移动端状态驱动单列 UI）；APK 由 CI 出（debug 签名，约 70MB）；运行时问题优先真机复现 + 截图报错定位。
- v0.3.5 起 `gen/android` 已入库（根 `.gitignore` 用 `gen/*` + `!gen/android/`），`MainActivity.kt` 用 `WindowInsetsCompat` 给 content 加四边避让 padding——targetSdk 36 强制 edge-to-edge 且 Android WebView 不支持 CSS `env(safe-area-inset-*)`，去掉 `enableEdgeToEdge()` 无效；本地 `pnpm tauri android build` 的 gradle rustBuild 任务有 WebSocket 环境问题（CI 正常），本地验证 Kotlin 用 `./gradlew :app:compileUniversalDebugKotlin`。
- 本机 Android 工具链（2026-09 已装）：brew `android-commandlinetools`（`/opt/homebrew/share/android-commandlinetools`）+ `openjdk@17`（`/opt/homebrew/opt/openjdk@17`）+ NDK 26.3.11579264；环境变量已写入 `~/.zshrc`（JAVA_HOME/ANDROID_HOME/NDK_HOME）。推送前预检 `./tools/android-check.sh`（cargo check + 测试编译，--target aarch64-linux-android，NDK 工具链、`tools/android-test-stubs.c` bionic 符号桩均在脚本内处理）。真机调试：`adb devices` 确认后 `cd desktop && pnpm tauri android dev` 部署热重载。
- 本机访问 GitHub：`github.com:443` 常被阻断，SSH 走 `ssh.github.com:443`（已写入 `~/.ssh/config` 的 `Host github.com`）；`api.github.com` 可直连，匿名 API 可查 CI 状态/产物（日志需登录）。

## UI 本地联调（零 SDK）
`./tools/ui-preview.sh` 起 http 服务并打开 `desktop/ui/index.html`；`ui/mock.js` 在浏览器环境 mock 全部 Tauri API（打包应用内自动失效），`?mobile=1/0` 强制移动/桌面视图、`?nomodel=1` 模拟未下载模型；改 ui 下 HTML/CSS/JS 后浏览器刷新即可，不依赖 CI。

## 测试 / CI / 发版
- 自动化测试（三端）：`pipeline.rs` 内置单元测试（遮罩规则/负数坐标/文件名排序/无模型全流程）+ `#[ignore]` E2E（真实 LaMa 推理，断言水印白像素下降与自动清理）。本地跑 E2E：模型放 `desktop/src-tauri/.models/lama_fp32.onnx` 后 `LAMA_MODEL=.models/lama_fp32.onnx cargo test --quiet -- --ignored`（清 MacPorts 变量，cd src-tauri）；CI `desktop-build.yml` 跑单测 + 模型缓存 + E2E。已修复 `numeric_key` 大写 `.PNG` 排序 bug。
- CI：GitHub Actions（仓库 `github.com/yazhouzou/img-water`，remote 名 `github`；origin 仍是 Codeup，双远端都推）；产物自动附加到 Release（免登录）。注意：x86_64 macOS 已从矩阵移除（ort-sys 无该平台预编译库）；构建步骤必须 `shell: bash`（Windows runner 默认 pwsh）；Windows 产物路径含 `target/<triple>/`。
- 发版：`./tools/release.sh <x.y.z>`（本地预检 cargo check → 改版本号 → 提交推送双远端 → 打 tag 触发 CI），产物自动附加到 GitHub Release（免登录，`releases/latest` 永久地址）。执行后立即结束回复并标注"CI 后台构建中"，不轮询；若构建成功但 Release 缺产物，让用户在 Actions run 页面点 Re-run failed jobs（仅重跑附加步骤约 1 分钟）。踩坑：matrix 内并发 softprops 附加同一 Release 会竞态失败（须独立 release job）；release job 无 checkout，gh 命令必须设 `GH_REPO`；APK artifact 解压带嵌套目录，需拍平后 `find dist -type f` 上传；删 tag 会把已发布 Release 转为 draft，release job 启动时自动清理同 tag draft。不要删 tag 重推来修 release 问题，除非同时改了构建代码。v0.3.2 已发布；v0.2.0 含新图标；v0.1.0 为旧图标版。

## 双端一致性回归测试
`tools/compare_pipelines.py` 生成多场景合成图（小水印/828 大水印/贴边/400 小图/多位置），同一遮罩框分别跑终端 iopaint 与 clean-cli，量化对比修复区 MAD。改 `lama.rs`/`pipeline.rs` 推理链路后必须跑：

```bash
cd desktop/src-tauri && cargo build --release --bin clean-cli
.img-inpaint-venv/bin/python tools/compare_pipelines.py
```

无缩放场景阈值 MAD<8（双端同为原分辨率推理应高度一致），缩放场景 <25（App 侧 resize 策略差异，视觉等价）；未处理区 PSNR>100dB。历史结论：ONNX 与 JIT 模型逐像素一致（输入 0..1/输出 0..255/mask 二值不敏感），差异只可能出在工程链路。**compare_pipelines.py 必须用 `--model lama` 对齐双端**（Python 端默认 MAT，MAT 与 LaMa 是不同模型，不指定会 MAD 翻倍误报 FAIL）。

# docs/ci.md — 测试 / CI / 发版（按需加载）

只在推送、发版、或改 CI/构建时读。根 `AGENTS.md` 与 `desktop/AGENTS.md` 只留指针。

## 推送前预检
- 改 Rust：macOS 跑 `cargo check`（先清 MacPorts 变量：`env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET`），或用 `desktop/build.sh`。
- 改 JS：`node --check desktop/ui/*.js`。
- 改 Android/Rust：`./tools/android-check.sh`。
- 改 `lama.rs`/`pipeline.rs` 推理链路：`tools/compare_pipelines.py`（**必须加 `--model lama`**）。

## 测试
- `pipeline.rs` 单元测试（遮罩/负数坐标/文件名排序/无模型全流程）+ `#[ignore]` E2E（真实 LaMa，断言白像素下降与自动清理）。
- 本地 E2E：模型放 `desktop/src-tauri/.models/lama_fp32.onnx` 后，`cd desktop/src-tauri && LAMA_MODEL=.models/lama_fp32.onnx cargo test --quiet -- --ignored`（清 MacPorts 变量）。
- CI `desktop-build.yml` 跑单测 + 模型缓存 + E2E。

## CI（GitHub Actions）
- 仓库 `github.com/yazhouzou/img-water`，remote `github`；origin 仍 Codeup，**双推**。产物自动附 Release（免登录）。
- 矩阵：`macos-latest`（aarch64）+ `windows-latest`（x86_64）；x86_64 macOS 已从矩阵移除（`ort-sys` 无预编译库）。
- 构建步骤须 `shell: bash`（Windows runner 默认 pwsh）；Windows 产物路径含 `target/<triple>/`。
- **CI 默认异步**：推送后标注"CI 后台验证中"即结束，下次用 `api.github.com` 查；仅明确要求才轮询（≥90s）。
- 本机网络：`github.com:443` 常被阻断，SSH 走 `ssh.github.com:443`（已在 `~/.ssh/config`）；`api.github.com` 可直连查状态/产物。

## 发版
- `./tools/release.sh <x.y.z>`：预检 cargo check → 改版本号 → 提交推送双远端 → 打 tag 触发 CI；产物自动附 GitHub Release（`releases/latest` 永久）。执行后立即结束并标注"CI 后台构建中"，不轮询；缺产物让用户在 Actions 点 Re-run failed jobs。
- 踩坑：
  - matrix 内并发 softprops 附加同一 Release 竞态失败（须独立 release job）。
  - release job 无 checkout，gh 须设 `GH_REPO`。
  - APK artifact 解压有嵌套目录，须拍平后 `find dist -type f` 上传。
  - 删 tag 会把已发布 Release 转 draft（release job 启动自动清同 tag draft）；勿删 tag 重推修 release（除非同时改构建代码）。

（v0.3.2 已发布。）

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
- 水印管线回归 `tools/watermark_regression.py`（L1，无模型）已接入 `watermark-regression.yml`：改 `tools/**` 的推送/PR 必跑，非零退出即失败。CI 无 `dist/`/根目录成品图，故正样本走"合成带水印图 + 资产模板"，覆盖 mask footprint、逐图墨色标定、逆解背景泛化（渐变/强纹理/硬边缘）、MAT 低频带偏。需模型/真实图的 L3（`--e2e`）不在 CI。

## CI（GitHub Actions）
- 仓库 `github.com/yazhouzou/img-water`，remote `github`；origin 仍 Codeup，**双推**。产物自动附 Release（免登录）。
- 矩阵：`macos-latest`（aarch64）+ `windows-latest`（x86_64）；x86_64 macOS 已从矩阵移除（`ort-sys` 无预编译库）。
- 构建步骤须 `shell: bash`（Windows runner 默认 pwsh）；Windows 产物路径含 `target/<triple>/`。
- **CI 默认异步**：推送后标注"CI 后台验证中"即结束，下次用 `api.github.com` 查；仅明确要求才轮询（≥90s）。
- 本机网络：`github.com:443` 常被阻断，SSH 走 `ssh.github.com:443`（已在 `~/.ssh/config`）；`api.github.com` 可直连查状态/产物。

## macOS 签名与公证
- CI 构建步骤把仓库 Secrets 中的 `APPLE_CERTIFICATE`/`APPLE_CERTIFICATE_PASSWORD`/`APPLE_SIGNING_IDENTITY`/`APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` 透传给 `pnpm tauri build`（Tauri 原生识别这些变量）。
- 构建后 `Verify macOS signing & notarization` 步骤分级校验（避免"以为签了其实没签"）：
  - 未配 `APPLE_SIGNING_IDENTITY` → `::warning::`，不失败（产物可用，用户首次打开需右键「打开」绕过 Gatekeeper）。
  - 配了证书 → `codesign --verify --deep --strict` 必须通过，否则失败。
  - 证书 + 公证账号齐全 → 追加 `spctl -a -t exec` 与 `xcrun stapler validate`（`.app` 与 `.dmg` 各一次），任一不过即失败。
- 本机 `tools/package-app.sh mac` 在复制 DMG 前跑同一套校验（未配证书只告警）。
- Secrets 6 项：① `APPLE_CERTIFICATE`=Developer ID Application `.p12` 的 base64（`base64 -i cert.p12 | pbcopy`）② `APPLE_CERTIFICATE_PASSWORD`=导出时的密码 ③ `APPLE_SIGNING_IDENTITY`=`Developer ID Application: <姓名> (<TEAMID>)` ④ `APPLE_ID`=Apple ID 邮箱 ⑤ `APPLE_PASSWORD`=App 专用密码（appleid.apple.com 生成，非账号密码）⑥ `APPLE_TEAM_ID`=10 位团队 ID。
- 只配 ①–③：消除"已损坏，无法打开"，但仍有"来自身份不明的开发者"；配齐 ④–⑥ 才是双击即开的完整体验。

## 自动更新（**未启用**：卡在需要你先生成密钥对）
代码里**没有**接 `tauri-plugin-updater`。原因：`bundle.createUpdaterArtifacts: true` 之后，本地与 CI 的 `tauri build` 都**必须**能拿到签名私钥，否则直接构建失败——所以必须先有密钥再合代码。启用步骤（约 5 分钟）：
1. `cd desktop && pnpm tauri signer generate -w ~/.tauri/watermark-cleaner.key`（打印公钥；`~/.tauri/*.key` 离线备份，丢了就无法再发增量更新）。
2. `tauri.conf.json`：`bundle.createUpdaterArtifacts: true`、`plugins.updater.pubkey: "<上一步公钥>"`、`plugins.updater.endpoints: ["https://github.com/yazhouzou/img-water/releases/latest/download/latest.json"]`。
3. 仓库 Secrets 加 `TAURI_SIGNING_PRIVATE_KEY`（私钥文件内容）+ `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；release job 汇总各平台 `*.sig` 生成并上传 `latest.json`（`{version, notes, pub_date, platforms: {<target>: {signature, url}}}`）。
4. 前端：启动时 `check()`（`@tauri-apps/plugin-updater`），有更新则 `downloadAndInstall()` 后 `relaunch()`；桌面端可用菜单/静默检查。

## 发版
- `./tools/release.sh <x.y.z>`：预检 cargo check → 改版本号 → 提交推送双远端 → 打 tag 触发 CI；产物自动附 GitHub Release（`releases/latest` 永久）。执行后立即结束并标注"CI 后台构建中"，不轮询；缺产物让用户在 Actions 点 Re-run failed jobs。
- 踩坑：
  - matrix 内并发 softprops 附加同一 Release 竞态失败（须独立 release job）。
  - release job 无 checkout，gh 须设 `GH_REPO`。
  - APK artifact 解压有嵌套目录，须拍平后 `find dist -type f` 上传。
  - 删 tag 会把已发布 Release 转 draft（release job 启动自动清同 tag draft）；勿删 tag 重推修 release（除非同时改构建代码）。

（v0.3.2 已发布。）

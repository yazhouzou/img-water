# AGENTS.md

## 项目目标

本项目主要处理 PNG 图片右下角的“豆包AI生成”水印。常见需求是批量处理项目根目录中的 `*.png`，去掉右下角水印并覆盖原文件。

## 会话触发含义

用户在会话框里说“处理图片”“去掉水印”“清除水印”“去水印”等类似需求时，默认含义是：项目根目录下的 PNG 图片都需要处理，目标是去掉图片右下角的“豆包AI生成”水印，并按本文件的备份、修复、复查和清理流程完整执行。

用户在会话框里明确说“去掉其它水印”“清除其它水印”时，默认含义是：去掉非“豆包AI生成”的可见水印，例如“千问AI”“AI生成”“AI生成，非临床诊断依据”“3DMGAME”“小可的拍摄笔记”等。其它水印不假设一定在右下角，必须先通过复查图确认水印文字、位置和遮罩范围，再用自定义遮罩区域处理。

如果用户明确指定了文件范围，例如 `1.png 到 6.png` 或 `@1.png @2.png`，只处理指定图片。否则直接处理项目根目录下所有 `*.png`，不要先花时间筛选哪些图片还带水印。

## 默认工作流

1. 确定目标图片：用户指定范围时按指定范围；未指定时取项目根目录下所有 `*.png`。
2. 确认目标图片都存在，并读取尺寸。
3. 处理前备份原图到 `original-watermark-backup/`，备份文件已存在时不要覆盖。
4. 生成右下角裁剪复查图，只用于确认水印位置、尺寸和遮罩范围，不用于筛选是否处理。
5. 遮罩两级来源：自动检测（默认，检测不到就跳过）→ `--mask-box` 手动指定；只覆盖水印文字区域，保留少量边距。
6. 优先使用 `iopaint` 批量修复；`inpaint` 支持 `--model mat|lama`（默认 **mat**）：MAT（mask-aware transformer）对结构边界重建显著优于 LaMa——光斑锐度、阴影层次、花瓣形态保留更完整（6.png 花墙对比验证），代价推理约慢 12 倍（单图 ~2 分钟 vs ~10 秒）。MAT 模型 `Places_512_FullData_G.pth`（250MB）在 GitHub release 直连被墙，**用 ghfast.top 镜像前缀**手动下载到 `~/.cache/torch/hub/checkpoints/`（iopaint 自动下载会无限挂起）。`inpaint` 阶段先做 **TELEA 粗填预处理**（`cv2.inpaint` 半径 7，对 source 的 mask 区先插值填充，再交模型精修）——直接修复时模型会"延续"水印白色笔画生成白色伪块（深色背景场景），粗填后模型看到中性底色，生成纹理与周围更协调（6.png 花影红色延续、2.png 黑底均验证更优）。**粗填适用边界**：背景均匀时有效；水印横跨强对比边界（亮墙/暗花）时 TELEA 从边界插值会把亮侧颜色大量涌入 mask 区，反而把白块喂给模型——此时改用模板 mask 方案（见下）且**不做粗填**直接推理。
7. **模板笔画 mask 方案（已集成 `prepare` 管线，复杂场景精确修复）**：豆包水印字形全图固定（"豆包AI生成"半透明白字，α≈0.6 叠加），模板资产 `tools/doubao-wm-template.png` + `.json`（从黑底 2.png 提取：阈值 ≥125 + 3x3 闭运算，填充率仅 ~21%，远小于整框；元数据含参考短边 1600 与模板内笔画 bbox）。`prepare` 三级策略：① **模板优先**——按图片短边比例缩放模板（水印尺寸随短边等比：2848x1600 → 1728x2304 图放大 1728/1600≈1.08，不是固定像素），右下角 40px 窗口内 gap-score 匹配（`cv2.matchTemplate` 两次：0/1 模板核取 S_in、全 1 核取窗口和，gap = S_in/N_in − S_out/N_out；实测分数 2.png 144 / 5.png 136 / 4.png 102 / 1.png 62 / 6.png 57 / 3.png 15），阈值 ≥40 命中即写笔画 mask（膨胀核按模型：MAT 配粗填用 7x7 最小侵入 / LaMa 无粗填用 19x11 连片）+ `.tpl` sidecar；② **整框检测回退**（分数不足时走原多阈值+顶帽检测，3.png 低对比纸面场景走此路径）；③ 检测不到 → 空 mask 跳过。`inpaint` 按 sidecar 与模型区分粗填策略：**MAT 一律先粗填**——TELEA 从 mask 边界真实背景插值出结构底色（如 6.png 花墙交界红棕带的走向），MAT 在底色上精修出锐利细节，宏观结构与原图一致性显著优于 direct MAT（6.png "AI" 附近红棕带：direct 断裂发暗，粗填后连续贯穿）；**LaMa + 模板 mask 跳过粗填**（LaMa 会把粗填底色延续成白色伪块），LaMa + 整框 mask 仍粗填。crop margin 维持默认 128：margin 256 会让 MAT 在"成"字区生成暗色伪影块。
8. **LaMa"见字生字"机制（关键教训）**：笔画级 mask 若字符间隙未被覆盖，间隙里残留的水印字形轮廓（描边/字符边界）会被 LaMa 的 FFT 全局感受野捕捉，模型按"字形延续"填充每个笔画洞 → 文字形状复活（4.png d5 膨胀时"豆"字复活、6.png 3px 膨胀时整行文字回归）。**破解**：水平膨胀到字符间距填满（±9px）使 mask 连片，间隙上下文消失后模型只能用整体纹理填充，文字不再复活且重绘面积远小于整框；垂直方向只需 ±5px 盖住抗锯齿带（19x11 矩形核）——全向 9px 会让 mask 垂直侵入花墙棱线等强结构边界，LaMa 重绘垂直宽带产生混沌（6.png 教训）。**位置对齐比膨胀参数更关键**：mask 手工放置偏 8px 时小膨胀盖不住字形，会误判为"垂直膨胀不足"（6.png 扁核曾因放置在旧位置 (2538,1498) 而残块，正确位置 (2541,1490) 下 ±3px 即可）。**粗填与膨胀的联动**（6.png 定量验证）：粗填（TELEA 插值）已把 mask 区填成背景延续、消除字形亮度信息，因此 **MAT+粗填下 mask 可缩到 ±3px（7x7）**——最小化重绘面积，最大保留字符间隙里的真实画面（修改面积 14506→10707px、阴影丢失 6047→4856、变暗 2693→944，花丛形态与原图高度一致）；±3px 直接 MAT（无粗填）会字形复活，勿去掉粗填。这与 v0.5.3 的"pad 反射镜像水印"是同一类上下文污染坑。**α 反解路线已验证不可行（勿重试）**：理论上可从混合公式 bg=(obs−255α)/(1−α) 数学还原笔画下真实背景，实测失败——① 水印并非纯 α 混合（花丛暗区笔画实测亮度高于模型预测，反解值系统性偏高成白残影）；② 反解噪声放大 1/(1−α)≈2.4 倍，亮墙区笔画颗粒明显亮于周围产生颗粒残影；③ 用粗填做背景先验的一致性校验也无法消除（先验本身是低频插值，无法验证像素级正确性）。被水印笔画直接压住的像素信息已被破坏，生成式修复（粗填+MAT）是质量上限。
7. 覆盖目标 PNG 前，先生成候选结果右下角复查图。
8. 候选结果确认无文字残留、无明显糊块后，再覆盖原图。
9. 覆盖后必须再生成一次落盘复查图，确认当前目录中的文件已是去水印版本。
10. 使用项目内脚本完成处理，不再临时新建处理脚本；最终复查通过后删除 `original-watermark-backup/` 里的备份原图和 `/tmp` 中本次生成的复查拼图。

## 工具约定

优先使用项目内持久修复环境，避免 `/tmp` 被清理后反复安装。`iopaint` 下面只是工具路径，不能单独运行；直接运行会提示 `Missing command`，需要带 `list`、`run` 等子命令：

```bash
.img-inpaint-venv/bin/iopaint list
.img-inpaint-venv/bin/iopaint run --model lama --device mps --image /tmp/doubao-watermark-work/source --mask /tmp/doubao-watermark-work/masks --output /tmp/doubao-watermark-work/lama
```

如果环境不存在，直接运行项目内初始化脚本，固定关键版本，减少依赖解析等待：

```bash
./tools/ensure-inpaint-env.sh
```

LaMa 模型通常缓存于：

```text
/Users/yazhouzou/.cache/torch/hub/checkpoints/big-lama.pt
```

后续任务优先使用项目内固定脚本，不要再临时编写 Python 脚本。会话中由 AI 处理“去掉水印”时，默认仍按分步流程执行，必须读取候选复查图和落盘复查图确认质量后再清理。

用户自己在终端处理同类型图片时，可以使用一键快捷命令：

```bash
./tools/remove_doubao_watermark.py run [可选文件列表]
```

**双端一致性回归测试**（防 App 内核细节 bug 依赖人工排查）：`tools/compare_pipelines.py` 生成多场景合成图（小水印/828 大水印/贴边/400 小图/多位置），同一遮罩框分别跑终端 iopaint 与 clean-cli，量化对比修复区 MAD。改 lama.rs / pipeline.rs 推理链路后必须跑：

```bash
cd desktop/src-tauri && cargo build --release --bin clean-cli
.img-inpaint-venv/bin/python tools/compare_pipelines.py
```

无缩放场景阈值 MAD<8（双端同为原分辨率推理应高度一致），缩放场景 <25（App 侧 resize 策略差异，视觉等价）；未处理区 PSNR>100dB。历史排查结论：ONNX 与 JIT 模型逐像素一致（输入 0..1/输出 0..255/mask 二值不敏感），差异只可能出在工程链路。

首次使用或环境损坏时先运行：

```bash
./tools/ensure-inpaint-env.sh
```

指定文件示例：

```bash
./tools/remove_doubao_watermark.py run 1.png 2.png
```

处理项目根目录以外的图片文件夹（不改变会话默认行为）：

```bash
./tools/remove_doubao_watermark.py --root /path/to/images run
```

`run` 会自动完成备份、遮罩、LaMa 修复、候选复查图生成、覆盖、落盘复查图生成和清理。它主要用于用户自助调用或明确要求“一键快速处理”的场景；默认成功后会删除 `original-watermark-backup/` 和 `/tmp/doubao-watermark-work`。如果需要人工查看复查图，可临时保留本轮产物：

```bash
./tools/remove_doubao_watermark.py --keep-work run [可选文件列表]
```

会话中默认使用下面的分步命令，便于在覆盖前后读取复查图并判断质量：

```bash
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py prepare [可选文件列表]
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py inpaint [可选文件列表]
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py review-lama [可选文件列表]
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py overwrite-review [可选文件列表]
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py cleanup [可选文件列表]
```

脚本约定：未传文件列表时处理项目根目录下所有 `*.png`；传入文件列表时只处理指定文件。

默认不传 `--mask-box` 时使用“豆包AI生成”的右下角遮罩规则。处理其它水印时，先确认水印位置，再给 `prepare` 传自定义遮罩区域：

```bash
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py --mask-box x1,y1,x2,y2 prepare [可选文件列表]
```

`--mask-box` 支持绝对坐标，也支持负数表示相对右下边界，例如 `-330,-118,-8,-8`（argparse 负数需用 `=` 传参）。

## 桌面端

项目提供 Tauri 桌面应用（`desktop/`），内置 Rust + ONNX Runtime 修复内核（v0.2 起，无需 Python）：

- 内核：`desktop/src-tauri/src/lama.rs` 用 `ort` 推理 LaMa ONNX 模型（`.models/lama_fp32.onnx`，约 200MB）；模型输入固定 [batch,3,512,512]（ONNX 导出时 H/W 静态，实测与 JIT big-lama 输出逐像素一致、无质量阉割；输入 0..1、输出 0..255、mask 二值不敏感）。v0.5.1 起推理策略对齐 iopaint CROP：每个遮罩连通块 crop bbox+128px margin（`crop_box()` 贴边补偿同 iopaint），crop ≤512 原分辨率 pad 512（居中）推理；更大时等比缩放到 512 推理后 Lanczos 还原写回——大水印整体修复，取代 v0.3.9 的 tile 分块（tile 每块只看半截水印导致接缝与模糊，是"App 端部分图模糊"的根因之一）；图像源函数内 clone 自当前已修复结果（级联）。**pad 区必须用图像均值色常数填充，禁止反射 pad**：反射会把贴近 crop 边缘的水印文字镜像进模型上下文，模型照字形延续产生残影（v0.5.3 排查结论，水印贴图底时 mask 必然贴 crop 边，此坑必现；iopaint 原分辨率推理 pad 仅 ≤7px 故无此问题）。FFT 算子不可导出 ONNX（aten::fft_rfftn 不支持），Carve 固定 512 正因 FFT 在固定尺寸下可预计算为矩阵乘——动态尺寸 ONNX 导出已验证不可行。
- 流水线：`desktop/src-tauri/src/pipeline.rs` 是 `tools/remove_doubao_watermark.py` 的 Rust 移植（备份/遮罩/修复/复查/覆盖/清理），桌面端进程内调用；`run` 模式清理前会把两张复查拼图复制到系统临时目录 `doubao-watermark-review/` 再输出路径，避免复查图被清理后失效
- CLI：`clean-cli`（`cargo build --bin clean-cli`），参数与 Python 脚本一致（`--root`/`--mask-box`/`--keep-work` + `run|prepare|inpaint|review-lama|overwrite-review|cleanup`）
- 模型下载：启动检测到模型缺失即自动开始下载（多连接分段+分段重试+源回退 hf-mirror→huggingface），顶栏进度条；`LAMA_ONNX_URL` 可覆盖下载源（单一地址），`LAMA_ONNX_PATH` 可覆盖模型位置
- 开发调试：`cd desktop && pnpm install && pnpm tauri dev`；直接跑 `cargo build`/`cargo check` 必须先 `env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET`（`.zshrc` 旧 MacPorts 变量会破坏 `objc2-exception-helper` 编译），或统一用 `desktop/build.sh`
- Windows 安装包无法在 macOS 上交叉编译；一键打包入口 `tools/package-app.sh`：`mac`（本机 DMG）、`win`（Windows Git Bash 本机构建，或 macOS 上 `--remote user@host --win-repo C:/path` 经 SSH 触发远程 Windows 构建并拉回）、`both`（两者一同打包）；产物统一在项目根目录 `dist/`
- 旧 Python 流水线（`tools/remove_doubao_watermark.py` + `.img-inpaint-venv/`）保留作终端回退方案，不再被桌面端依赖
- Android：代码层适配已就绪（路径重定向到应用目录、相册 `content://` URI 经 JNI/ContentResolver 拷贝导入见 `android_uri.rs`、移动端状态驱动单列 UI）；APK 由 CI 出（debug 签名，约 70MB）；运行时问题优先真机复现 + 截图报错定位。v0.3.5 起 `gen/android` 已入库（根 .gitignore 用 `gen/*` + `!gen/android/`），`MainActivity.kt` 用 `WindowInsetsCompat` 给 content 加四边避让 padding——targetSdk 36 强制 edge-to-edge 且 Android WebView 不支持 CSS `env(safe-area-inset-*)`，去掉 `enableEdgeToEdge()` 无效；本地 `pnpm tauri android build` 的 gradle rustBuild 任务有 WebSocket 环境问题（CI 正常），本地验证 Kotlin 用 `./gradlew :app:compileUniversalDebugKotlin`
- UI 本地联调（零 SDK）：`./tools/ui-preview.sh` 起 http 服务并打开 `desktop/ui/index.html`；`ui/mock.js` 在浏览器环境 mock 全部 Tauri API（打包应用内自动失效），`?mobile=1/0` 强制移动/桌面视图、`?nomodel=1` 模拟未下载模型；改 ui 下 HTML/CSS/JS 后浏览器刷新即可，不依赖 CI
- CI：GitHub Actions（仓库 `github.com/yazhouzou/img-water`，remote 名 `github`；origin 仍是 Codeup，双远端都推）；产物自动附加到 Release（免登录下载）。注意：x86_64 macOS 已从矩阵移除（ort-sys 无该平台预编译库）；构建步骤必须 `shell: bash`（Windows runner 默认 pwsh）；Windows 产物路径含 `target/<triple>/`
- 自动化测试（三端）：`pipeline.rs` 内置单元测试（遮罩规则/负数坐标/文件名排序/无模型全流程）+ `#[ignore]` E2E（真实 LaMa 推理，断言水印白像素下降与自动清理）。本地跑 E2E：模型放 `desktop/src-tauri/.models/lama_fp32.onnx` 后 `LAMA_MODEL=.models/lama_fp32.onnx cargo test --quiet -- --ignored`（清 MacPorts 变量，cd src-tauri）；CI desktop-build.yml 会跑单测 + 模型缓存 + E2E。android-check.sh 另含测试代码编译检查：cargo test 需要 NDK linker + ort 静态库的 bionic 符号桩（`tools/android-test-stubs.c`，仅链接检查用永不执行，APK cdylib 不受影响）；脚本强制用 brew NDK 路径并校验存在（父环境旧 `ANDROID_HOME` 会指向无 NDK 的旧 SDK）。已修复 `numeric_key` 大写 `.PNG` 排序 bug
- 本机 Android 工具链（2026-09 已装，用户已同意）：brew `android-commandlinetools`（`/opt/homebrew/share/android-commandlinetools`）+ `openjdk@17`（`/opt/homebrew/opt/openjdk@17`，无需 sudo）+ NDK 26.3.11579264；环境变量已写入 `~/.zshrc`（JAVA_HOME/ANDROID_HOME/NDK_HOME）。推送前 Android 编译预检：`./tools/android-check.sh`（cargo check + 测试编译，--target aarch64-linux-android，NDK 工具链已在脚本内加 PATH）。真机调试：手机开 USB 调试连 Mac，`adb devices` 确认后 `cd desktop && pnpm tauri android dev` 直接部署热重载，UI/运行时行为本地闭环，不再依赖 CI+真机装包迭代。桌面端预检仍是 macOS `cargo check` + `node --check ui/*.js`
- 本机访问 GitHub：`github.com:443` 常被阻断，SSH 走 `ssh.github.com:443`（已写入 `~/.ssh/config` 的 `Host github.com`）；`api.github.com` 可直连，匿名 API 可查询 CI 状态/产物（日志需登录）
- 发版：一键 `./tools/release.sh <x.y.z>`（本地预检 cargo check → 改版本号 → 提交推送双远端 → 打 tag 触发 CI），产物自动附加到 GitHub Release（免登录，`releases/latest` 永久地址）。会话中执行发布后立即结束回复并标注“CI 后台构建中”，不轮询；若构建成功但 Release 缺产物，让用户在 Actions run 页面点 Re-run failed jobs（仅重跑附加步骤约 1 分钟，不重构建）。踩坑记录：matrix 内并发 softprops 附加同一 Release 会竞态失败（须用独立 release job）；release job 无 checkout，gh 命令必须设 `GH_REPO`；APK artifact 解压带嵌套目录，需拍平后用 `find dist -type f` 上传；删除 tag 会把已发布 Release 转为 draft，release job 启动时会自动清理同 tag draft。不要删 tag 重推来修 release 问题，除非同时改了构建代码。v0.3.2 已发布（相册 content:// 导入、自动检测遮罩待同步到桌面端）；v0.2.0 含新图标；v0.1.0 为旧图标版

## 提效规则

1. 修复环境固定放在项目根目录 `.img-inpaint-venv/`，不要优先使用 `/tmp/img-inpaint-venv`。
2. 会话中处理图片时，优先使用 `tools/remove_doubao_watermark.py` 的分步命令复用备份、遮罩、复查、覆盖和清理逻辑，不要每次创建新的临时脚本。
3. `./tools/remove_doubao_watermark.py run` 是用户自助的一键快捷命令，或用户明确要求“一键快速处理”时使用；不要让它替代会话里的人工复查判断。
4. 出现“去掉水印”等泛化指令时，直接处理根目录所有 `*.png`，不做“是否仍有水印”的筛选。
5. 同一批图片只执行一次 `iopaint run`，让模型只加载一次。
6. 复查保持两次关键检查：候选右下角复查一次、覆盖后的落盘右下角复查一次；不要反复生成多轮候选，除非质量有疑问。
7. `/tmp/doubao-watermark-work` 只作为本轮中间产物目录，最终复查通过后必须清理。
8. 首次创建 `.img-inpaint-venv/` 会慢，后续不要删除该目录；初始化和健康检查日志在 `.img-inpaint-venv/install.log`。
9. `iopaint list` 可能输出 INFO 或 FutureWarning，这不是失败；`tools/ensure-inpaint-env.sh` 会把这些噪声写入日志，终端只保留成功或真正失败提示。
10. 每轮仍会有 LaMa 模型加载耗时，提效重点是把同一批图片合并到一次 `inpaint` 命令中，不要逐张运行。
11. 图片复查只读取拼图，不逐张读取完整原图，减少图片解析和回传耗时。
12. 涉及 CI 构建的会话默认异步等待：推送成功后在回复中标注“CI 后台验证中”即可，不原地轮询阻塞会话；下次会话开头先用 `api.github.com` 匿名 API 查上一轮结果再继续。仅当用户明确要求“等 CI 结果”时才轮询，间隔 ≥90s。
13. 减少CI 白跑：涉及 Rust/JS 修改时，推送前必须先跑 macOS `cargo check`（清 MacPorts 变量）和 `node --check ui/*.js`；涉及 Android 行为/Rust 改动时再跑 `./tools/android-check.sh`（NDK 已就绪，本地可过 android target 编译）。
14. Android 平台差异防御清单（写代码时对照）：dialog 选择器在 Android 返回 `content://` URI（已由 android_uri.rs 处理）；路径必须用 `app_data_dir`/`app_cache_dir` 重定向（MODEL_DIR_OVERRIDE/WORKDIR_OVERRIDE 已就绪）；command 失败必须有可见反馈（原生弹窗或 banner），不能只写折叠日志；新平台行为不确定时先查 `~/.cargo/registry/src/` 里 tauri/插件源码（Kotlin 实现都在 crate 内），再真机验证。

## 遮罩规则

遮罩按两级优先级生成，遮罩必须按当前图片尺寸生成，不能假设所有任务尺寸相同。

1. **自动检测（默认）**：全图纯白文字检测 + 右下角兜底三级策略（Python `_detect_corner_faded` / Rust `detect_corner_faded` 同步）：① 多阈值扫描 248→150 全量收集后**重叠融合**（`_fuse_boxes`/`fuse_boxes`）——暗水印（2.png 亮度 150~162）高阈值只能切出局部组件，必须靠低阈值补全，单阈值"首个检出即用"会漏字（2.png 曾因此留"包/"残影）；② 顶帽变换兜底（31x31 椭圆开运算）：背景光照不均、水印与亮背景亮度重叠时（6.png 橙红墙+花影）固定阈值不可分；顶帽需叠加**低饱和过滤 sat≤60**（灰白水印 RGB 均衡，红花绿叶高饱和，否则花斑并入 mask 重绘毁花）；**顶帽只配字符行模式，不配行块模式**（顶帽图中花影/墙面亮斑与水印粘连，行块模式会把大片画面罩进框整块重绘——6.png 花丛曾被行块大框毁图，宁漏检不误修）；③ 所有候选框过"文字性验证"（原始白像素 fill ≤0.6 且 x 投影列段数 ≥3，必须用区域局部坐标，Python 端曾因误传全局坐标把兜底整个废掉）。corner 兜底检出框同样过贴边约束（±40px）。处理日志打印 `auto-detected` 或 `no watermark detected, skipped`
2. **字符行分析（防过度修复的核心）**：corner 检出必须过"字符行"判据（`_corner_char_boxes`/`corner_text_boxes`）——水印是单行文字，字符级组件高度统一（占图高 1.2%~4.5%、行内 ≥4 个字符、高度比 ≤1.8）；沙滩亮斑/花影粘连块高度杂乱或超高不成行，自然排除。**双模式互补**（Python/Rust 同步）：字符行模式（5x5 膨胀一次，背景干净时精确紧贴）+ **行块模式**（`_corner_row_boxes`/`corner_row_boxes`：9x3 膨胀两次 + 行框高度上限 6%H + 组件必须整体在 corner 区内 + 文字性验证）——字符与背景亮斑粘连、字符级分离失败时（4.png 雪景雪点把字符粘成 213x98 大块）行块模式仍能定位整行；两模式结果融合去重。**血泪教训**：旧版连通域无高度上限会把沙滩亮斑与水印粘连成全区域大块（1.png mask 曾达 828x196，实际水印仅 197x47），LaMa 重绘 10 倍于水印的区域 → "水印附近原图模糊、擦除原图元素"（用户实际投诉）。膨胀只可用 5x5 一次（合并字符内笔画碎片，字符间距不会粘连）。**inpaint 空 source**：全部图无水印跳过时 source 被清空，必须跳过 iopaint 调用（空目录会让 iopaint 报 `invalid --image` 退出 255，run 崩溃）
2. **prepare 默认只保留贴右下角的检出框**（x2>w-40 且 y2>h-40）：豆包水印必贴右下角，雪景白点/白墙/栏杆/天空等画面内容会被全图检测误检成多个框，硬修即毁图（4.png 雪景、6.png 白墙曾全部误检）。分散水印场景用 `--mask-box` 手动指定
3. **检测不到水印 → 跳过**（不再走默认尺寸规则硬修）：对已无水印的图硬修会把真实画面重绘成模糊块（空 mask 图直接透传不进模型）。`--mask-box` 显式指定时仍强制处理
4. **手动指定**：`--mask-box=x1,y1,x2,y2`（argparse 负数需用 `=` 传参），优先级最高；未显式传框时旧尺寸规则表（2848x1600 等）仅保留在 `default_mask_box()` 供相对坐标解析，不再作为自动兜底
5. **框内笔画精分割是实验性能力（`--refine` 显式开启，默认关闭）**：`refine_box_mask()`（仅 Python 端，Rust 未同步）把检测框/手动框缩到笔画级 mask，仅适用于均匀背景 + 白/灰水印。**已证伪的场景勿重试**：纸面纹理（3.png 案例 90% 漏检——纸面 std 抬高自适应阈值）、复杂花丛+低对比水印（6.png 案例误检花丛碎块+漏检低对比笔画）、黑底暗侧误检（TELEA 粗填痕迹被暗侧分割滚雪球）。四种自检修判据（面积占比/组件数/多假设 IoU/粗填-复检收敛性）逐一验证均无可靠区分度——**无先验、无参考的笔画分割自检是信息论限制，不是工程问题，勿继续投入**。默认路径整框 + 粗填 + MAT 是兜底（6.png 整框实测 changed 21998/lost 9089/gain 3303，及格线以下：光斑连片、花影被涂抹，但可接受）；复杂场景质量上限靠模板字库扩展/泛化字形检测解决

已知尺寸以外的图片，自动检测失败时必须先用右下角裁剪图实测水印位置，确认后再决定是补检测规则还是 `--mask-box`。桌面端 `pipeline.rs` 的检测规则与 Python 脚本必须同步，改动时两边都要更新。模板笔画 mask 方案已双端同步（Python `template_stroke_mask` / Rust `template_stroke_mask`）：模板资产经 `include_bytes!` 编译进 Rust 二进制（CLI 与打包 App 均可用）；Rust 匹配用积分图（S_all）+ 稀疏笔画点遍历（S_in）实现，窗口仅右下角 41×41 位置故无需 FFT；19x11 膨胀用横向+纵向两次一维分解。**模型差异（App 端暂不同步 MAT）**：Python 端默认 MAT，App 端（`lama.rs`）仍是 LaMa ONNX——MAT 的 ONNX 导出与 App 端集成待 Python 端确认完美后再做（用户明确要求），期间 App 端复杂场景质量略低于终端脚本（终端 iopaint MAT vs App LaMa）。**--any-position/DBNet 已同步 macOS App 端（v0.5.4 起）**：Rust `dbnet.rs` 用 `ort` 推理内嵌的 `tools/models/ch_pp-ocrv4_det.onnx`（include_bytes，OnceLock<Mutex<Session>> 全局缓存，CPU ~0.2s）；prepare 语义与 Python 一致——模板命中时非 any_position 即完成、any_position 时模板与 DBNet **叠加**（DBNet 框与模板笔画重叠的丢弃，避免重复修复右下角豆包水印）；DBNet 未检出回退传统扫描（Rust 未同步传统扩展扫描 faded/color/tophat/dark，DBNet 缺失场景极少）；`--any-position`（clean-cli）与 UI「任意位置文字水印」开关（`anyPosition` 参数）均可启用。Windows/Android 构建含同样代码（include_bytes 跨平台）但本轮仅 macOS 验证。**compare_pipelines.py 必须用 `--model lama` 对齐双端**（Python 端默认 MAT，MAT 与 LaMa 是不同模型，不指定会 MAD 翻倍误报 FAIL，2026-09 踩坑）。

典型案例（2.png）：黑背景上的灰白色“豆包AI生成”（亮度 150~162，远低于纯白 248），全图检测扫不到，靠 corner 兜底检出；修复后黑背景干净无糊块。注意“右下角亮像素 ≥228 统计”测不出这类灰白水印，不能用它判断图是否已处理。

实际处理前必须用右下角裁剪图确认水印没有超出遮罩。如果水印样式变化（不再是白字）导致检测和规则都失效，按“其它水印流程”处理。

## 其它水印规则

1. “去掉水印”默认只指“豆包AI生成”，不要误改为其它水印流程。
2. “去掉其它水印”才进入其它水印流程；如果用户同时给出水印文字或截图，按用户指定目标处理。
3. v0.3.4 起检测全图化、v0.3.8 起带文字性特征过滤：`detect_watermark_boxes()`（Rust + Python 同步）全图扫描白字聚类，任意位置/多处分散水印返回多框画多矩形遮罩，`lama.rs` 按 mask 连通块多窗口推理（每块 ≤496px）。防误擦判据：组件 bbox 内原始白像素填充率 ≤0.6 且 x 投影列段数 ≥3（实心白块如灯罩/瓷盘 fill 0.8+/段数 1，文字水印 fill ~0.2/段数=字符数）；Python 连通域用 scipy（该环境 cv2.connectedComponents 会段错误）。右下角另有自适应阈值兜底（阈值=背景均值+60，识别半透明灰白粗体水印，贴右下边约束）。非白字水印（深色、彩色、半透明全图）检测不到，仍需 `--mask-box` 手动指定。
4. 手动遮罩：`tools/remove_doubao_watermark.py --mask-box x1,y1,x2,y2 prepare ...`（单框）；遮罩必须只覆盖水印文字和必要边缘，不要覆盖大块画面。
5. **任意位置文字水印（`--any-position`，默认关）**：默认管线只保留贴右下角的检出框（防雪景/busy photo 误检毁图）；显式开启后启用 **OCR 文字检测模型（DBNet/PP-OCRv4 det，`tools/models/ch_pp-ocrv4_det.onnx` 4.7MB，onnxruntime CPU ~0.2s）**：命中即完全独挑（传统扫描在照片上误检率高反而拖累），按字高 unclip 外扩 1.0 成遮罩框（概率图 0.3 截断使检出框小于字面，外扩不足会留字两端残迹——photo:2__red 实测）；模型缺失或未检出时退回传统扫描（纯白 ≥248 + 灰白多阈值 230→160 + 深色负片 ≤40 + 彩色色度 sat>60 + 低对比顶帽 ≥12，纯背景可靠、照片误检多需复查）。能力边界以 `tools/synthetic_watermark_test.py` 合成矩阵为准（分层量化：检测层/管线默认层/--any-position 层，当前 **71/75**）：纯白/灰白/半透明/深色/彩色字在纯色/渐变/照片背景几乎全通——**传统 CV 确认不可分的场景（真实彩色照片彩色字、雪景白字）由 DBNet 解决**；DBNet 对雪景仅出 1-2 精准框（传统扫描 15 误检框），雪景误检防御随之解决。**剩余不可分边界（--mask-box 手动兜底）**：①物理零对比（黑底黑字、220 字 vs 220 底）；②极低对比（暗照片深字 ~10 灰度差、雪景极浅灰/半透明字，DBNet 概率图无响应，降阈值无效——是模型能力边界非调参问题）；③照片内**真实文字**（书页/招牌字）与水印同为文字，DBNet 一并检出会误修，不想擦需 --mask-box 限定。检出>6 框时 prepare 打印复查警告。`--mask-box` 支持分号分隔多框。App 端（pipeline.rs）未同步 `--any-position`/DBNet/多框/扩展检测（onnxruntime 可行，待 Python 端确认后同步）。
6. 单个水印区域超过 496px（LaMa 512 窗口上限）时桌面端自动走 tile 分块推理（v0.3.9），不再报错；候选框分数低于最高分 4% 会被丢弃（防噪声误擦）。
7. 最终质量标准仍相同：无目标水印残留，无明显糊块，不破坏主体和关键纹理。
## 质量标准

最终结果必须满足：

1. 目标区域看不到"豆包AI生成"或本次指定的其它水印文字残留。
2. 水印区域背景纹理自然，没有明显矩形糊块。
3. 不破坏画面主体、边缘线条、地面纹理、桌面纹理、水面纹理等关键视觉元素。
4. 最终回复前必须完成落盘复查，不要停在读取复查图之后。

## 修复方法论（普适原则，勿因单图调参违背）

1. **mask 最小化是第一原则**：修复质量的上限由 mask 决定——mask 每多盖 1px 真实画面，模型就多 1px 需要编造。mask 只应覆盖被水印真正破坏的像素（笔画核心 + 抗锯齿带 ±2-3px），膨胀参数永远从最小开始向上试，不要从大向下调。
2. **先消除水印信息泄漏，再最小化 mask**："字形复活"有两条路径：(a) mask 外残留的水印边缘像素；(b) mask 内未填充的水印像素被模型上下文感知。**注意：(b) 在 iopaint 下不存在——iopaint 推理时会把 mask 区 source 像素置零（MAT/LaMa 受控实验输出逐像素相同，2026-09 确证），粗填/预处理无法影响模型输入，管线里的 TELEA 粗填已作为死代码移除；历史"粗填有效"结论系旧环境误导，勿重试。** mask 只需盖住 (a)，(a) 决定膨胀下限：3x3 会暗色描边复活（量化抓不到，必须目检）、5x5 轻度、7x7 (±3px) 是模板方案必要最小膨胀，9x9+ 重绘面积增大花丛保留减少。
3. **模型能力要匹配场景复杂度**：LaMa 对结构边界（色带/棱线/阴影）重建差，会断裂发暗；MAT 在相同 mask 下光斑/花瓣/阴影显著更优。均匀背景（黑底/纸面/沙滩）LaMa 够用；复杂结构场景必须 MAT。**mask 边缘羽化混合已证伪**（2026-09）：mask 外残留的水印抗锯齿像素会被羽化权重混回，字形半透明浮现——mask 精确覆盖时任何"往回混"的操作都带回水印。
4. **量化验证优先于目检**：每次改动必须输出与原图的 diff 量化（changed / lost=阴影丢失 / gain=变暗）+ "mask 外零变化"断言（mask 外任何像素被改动都是 bug）+ **字形复活检测**（核心区/环带内输出比原图亮>40 的白复活 + 输出比原图暗>40 的暗描边复活——量化会漏暗色复活，必须配合 3x 放大目检）。crop margin 64/128 实测持平（crop 缩放非质量瓶颈，128 即可）。
5. **区分"被破坏像素"与"生成像素"的极限**：被水印笔画直接压住的像素信息已不可逆，生成式修复是上限；评价标准是宏观结构（色带走向/棱线/光斑位置）与原图一致 + 衔接自然，而非像素级相同。**混合边界区（如花丛/亮墙交界被"包"字下半压住）是偏差最大区域**——模型从混合上下文生成，花瓣形态/棱线走向与原图必有形态差，这是信息论极限不是流程缺陷；所有管线参数（膨胀核/margin/粗填/羽化/模型）穷尽实验后仍存在的差异即为此类极限，勿继续投入调参，转向"mask 是否能更精确"的唯一杠杆。不要追求像素级还原而引入反解/变换等高风险手段（见 α 反解教训）。

## 备份规则

原图备份目录固定为：

```text
original-watermark-backup/
```

备份策略：

1. 备份文件不存在时，复制当前目标图片作为备份。
2. 备份文件已存在时，不要覆盖。
3. 每次会话完成且最终落盘复查通过后，删除 `original-watermark-backup/` 里的备份原图；如目录为空，可以保留空目录或删除目录。
4. 同一时机删除 `/tmp` 中本次任务生成的复查拼图和临时任务目录，避免后续会话误读旧结果。

**覆盖安全（md5 校验，防再犯）**：prepare 会把本轮处理源文件的 md5 记入 `WORK/manifest.json`；`overwrite-review` 覆盖前校验目标文件 md5 与之一致，不一致（--root 传错目录/文件被改动）直接拒绝覆盖并退出。`overwrite-review` 不带 manifest 会拒绝执行（先跑 prepare）。**事故教训（2026-09）**：曾误用 `--root dist` 执行 overwrite-review + cleanup，把处理结果覆盖进 dist/ 原图目录且备份被清理，原图丢失（用户手动恢复）。`--root` 始终是"被处理文件所在的输出目录"，本项目内 dist/ 是只读原图源，任何写操作（overwrite-review/cleanup/run）的 --root 都不得指向 dist/。

## 用户沟通

处理过程中保持简洁更新。不要反复展示多个中间候选，除非质量有疑问。最终回复只说明已处理的图片范围、项目内修复环境是否可复用、备份和 `/tmp` 复查产物是否已清理，以及是否存在残留或风险。

## 会话压缩续接

- 压缩历史消息后，先以用户最新消息为准，再只接续摘要中明确标记为 `In Progress`、`Blocked` 或最后一次未完成的验证/收口动作。
- 不要把摘要里的长期候选 `Next Steps`、历史背景、已完成事项重新展开成新的泛化任务队列。
- 如果压缩摘要无法判断下一步，应先用一句话向用户确认，不要自行发散扫描或改造。

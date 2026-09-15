# desktop/AGENTS.md — 桌面端/App 细节（按需加载）

只在改 `desktop/` 时读；根 `AGENTS.md` 精简并指向这里。CI/发版见 `docs/ci.md`，Android 见 `docs/android.md`。

## 定位
Tauri 桌面应用，内置 Rust + ONNX Runtime 修复内核（v0.2 起，无需 Python）；旧 Python 流水线（`tools/remove_doubao_watermark.py` + `.img-inpaint-venv/`）仅作终端回退。

## Rust 内核（`src/lama.rs`）
- `ort` 推理 LaMa ONNX（`.models/lama_fp32.onnx`，~200MB）；输入固定 `[b,3,512,512]`（ONNX 静态 H/W，与 JIT big-lama 逐像素一致；输入 0..1、输出 0..255、mask 二值不敏感）。
- 对齐 iopaint CROP：每遮罩连通块 crop bbox+128px（`crop_box()` 贴边补偿同 iopaint）；crop ≤512 原分辨率居中 pad 到 512，更大则等比缩到 512 推理后 Lanczos 还原。
- **pad 必须用图像均值色常数填充，禁止反射 pad**：反射会把贴边水印镜像进上下文 → 残影（水印贴图底时必现）。
- FFT 不可导出 ONNX（`aten::fft_rfftn` 不支持）；Carve 固定 512 正因 FFT 可预计算为矩阵乘。
- 演进史与教训（tile→crop 等）见 `docs/lessons.md` §11。

## 流水线 / CLI
- `pipeline.rs` 是 Python 脚本的 Rust 移植（备份/遮罩/修复/复查/覆盖/清理）；`run` 清理前把两张复查拼图复制到系统临时目录 `doubao-watermark-review/` 再输出路径。
- CLI `clean-cli`（`cargo build --bin clean-cli`），参数同 Python（`--root`/`--mask-box`/`--keep-work`/`--force`（验证 FAIL 时强制落盘） + `run|prepare|inpaint|review-lama|overwrite-review|cleanup`）。
- 模型下载：启动缺失即自动下载（多连接分段+重试+源回退 hf-mirror→huggingface），顶栏进度条；`LAMA_ONNX_URL` 覆盖源，`LAMA_ONNX_PATH` 覆盖位置。

## 水印档案库（Rust 侧）
概念/建档案流程/阈值见 `docs/non-doubao-watermarks.md`；这里只记 Rust 侧差异与踩坑。
- `watermark_profiles.rs`：α/C 档案读写、NCC ±35% 两级定位（`NCC_SCALE_SPAN`）、`inverse_image`、学习（pair/solid/batch/auto）。`pipeline.rs` 加 `use_profile`（默认 true）：prepare 匹配写 `.wprof`，inpaint 后 `apply_profile_inverse`。qwen 档案 `include_bytes!` 内嵌；目录 `WATERMARK_PROFILES_DIR` 可覆盖，默认 `project_root/tools/watermarks`。
- CLI：`profiles`、`match <img>`、`learn-pair/learn-solid/learn-auto/learn-batch`（参数同 Python）、`--no-profile`。
- **踩坑**：连通域筛选（`labels_areas`）必须跳过背景 label 0，否则 `areas[0] >= min_area` 把全图判成前景（`stroke_mask` 全屏 mask、`clean_alpha` 残留微 α 使裁剪不收缩）。
- 自实现替代 imageproc：`CrossCorrelationNormalized` 是 CCORR 不减均值 → 自写 zero-mean NCC；形态学太慢 → 自写 O(n) 滑窗方形核（`rect_morph`/`slide_extreme`）。

## 开发 / 打包 / 测试
- `cd desktop && pnpm install && pnpm tauri dev`。
- 跑 `cargo build`/`check` 必须先 `env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET`（`.zshrc` 旧 MacPorts 变量破坏 `objc2-exception-helper`），或用 `desktop/build.sh`。
- Windows 包无法在 macOS 交叉编译；`tools/package-app.sh`：`mac`（本机 DMG）/`win`（Windows Git Bash 本机，或 macOS `--remote user@host --win-repo C:/path` 经 SSH 远程构建拉回）/`both`；产物在根 `dist/`。
- `cargo test --lib`（常规 18）+ `cargo test --lib -- --ignored --nocapture`（含 `detect_box_on_qwen_samples`/`qwen_profile_matches_real_images`/`qwen_pair_learning_residual_is_tiny`/`full_run_with_lama_e2e`）。
- 推送前预检、CI 工作流、本地 E2E 与发版：见 `docs/ci.md`。

## UI 本地联调（零 SDK）
`./tools/ui-preview.sh` 起 http 服务打开 `desktop/ui/index.html`；`ui/mock.js` 在浏览器 mock 全部 Tauri API（打包内自动失效），`?mobile=1/0` 强制移动/桌面、`?nomodel=1` 模拟未下载；改 ui 下文件刷新即可，不依赖 CI；Windows: PowerShell 需 `chcp 65001`。

## 双端同步
Python（终端）与 Rust 口径**除修复模型外已对齐**：

| 能力 | Python | Rust/App |
|---|---|---|
| 模板笔画 mask（连续 α>0.03 + 1px 膨胀） | ✓ | ✓ |
| 顶帽 gap-score + 阈值 20 | ✓ | ✓ |
| 结果级验证（mask 外零改动，FAIL 拒绝落盘） | ✓ | ✓（`--force` 强制） |
| 豆包 stamp 逆解（默认开） | ✓ | ✓（`--no-inverse` 关） |
| 残留自动重试（1 轮，默认开） | ✓ | ✓（`--no-retry` 关） |
| 框选/检测框内笔画精分割（`--refine`，实验性） | ✓ | ✓（`--refine`；App 默认勾选；残留自动退化整框，见下） |
| `--any-position` / DBNet | ✓ | ✓（macOS v0.5.4） |
| 水印档案库（α/C + 逆解 + NCC ±35%） | ✓ | ✓（macOS） |
| 修复模型 | 默认 MAT | LaMa ONNX |

**唯一残余差异**：修复模型（MAT vs LaMa ONNX）——两者都是生成式路线，复杂纹理（花墙/花丛）MAT 更稳；`scale≈1.0 + 邻域纹理复杂` 的图会被 stamp 逆解接管，此时模型差异不重要。

**框选精分割（`--refine` / App「框选区域精细处理」）**：框内顶帽局部对比 + 低饱和过滤 + 局部自适应阈值 + 行带约束 → 笔画级 mask（实测千问 7.png：12083px vs 整框 53592px，少重绘 4.4 倍）。关键设计：
- 精分割**不写 `.tpl`**（写了会强制启用豆包模板残留检查，非豆包水印碰巧高分 → 假残留），只写 `{name}.refinebox`（记录了原始框）；
- `verify_paths` 的 `template_applied` 支持 `None`＝自动判定（原图模板分 ≥20 且 `min(w,h)/ref_short≈1.0`）才查模板残留——千问图 scale=1.1 自动跳过，豆包图 scale=1.0 生效；
- 精分割留残留时 `residual_retry` **退化用 `.refinebox` 整框**重跑（豆包 1/6.png 实测：精分割残留 → 整框 → PASS、tmpl→0.0），保证「必然去除」，代价只是这一次多一次 inpaint。
- **与 Python 的有意分歧**：Python 的精分割也写 `.tpl`（会在非豆包图误报残留）；Rust 已改为只写 `.refinebox`。故"双端同步"此格仅指能力对齐，不是逐字节行为一致。精分割本身逐像素一致：`python tools/refine_parity_dump.py` + `cargo test --lib -- --ignored refine_box_mask_matches_python`（IoU=1.0000）。

**"App 影响周边元素"根因（v0.5.5 已修）**：旧 Rust 模板 mask 用「二值核(α>0.5) + 19x11 膨胀」（19/11 本是 Python `REFINE_DILATE_LAMA` 给退化框精分割用的），靠大膨胀补抗锯齿 → 比 Python 的「连续 α>0.03 + 1px」多盖约 38% 干净画面被模型重绘；且无顶帽 + 阈值 40 使亮背景（纸面/花墙/雪/沙滩）分数跌破阈值 → 回退整框 mask（面积再涨 3~5 倍）。修复后 Rust mask 与 Python **逐像素一致**（实测 IoU=1.0000，dist/1、3、6）。

**验证基线（v0.5.5）**：dist/1、3、6 双端 `mask 10172px`、`outside changed 0`；6.png 双端 stamp 逆解同取 `gain 1.05`（水印区 MAD 0.46、P95=2，此前 1.85/P95=16）；1、3.png 双端均按门控跳过逆解。`cargo test --lib` 19+4 passed；`compare_pipelines.py` 5/5 PASS。

一致性回归：`tools/compare_pipelines.py` 生成多场景合成图（小水印/828 大水印/贴边/400 小图/多位置），同遮罩框分别跑终端 iopaint 与 clean-cli，对比修复区 MAD。改 `lama.rs`/`pipeline.rs` 推理链路后必跑，**必须加 `--model lama`**（Python 默认 MAT，模型不同会误报 FAIL）。阈值：无缩放 MAD<8、缩放 <25；未处理区 PSNR>100dB。

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
- CLI `clean-cli`（`cargo build --bin clean-cli`），参数同 Python（`--root`/`--mask-box`/`--keep-work`/`--force`（验证 FAIL 时强制落盘）/`--output-dir <dir>`（另存模式自定义输出目录） + `run|prepare|inpaint|review-lama|overwrite-review|cleanup`）。
- 模型下载：启动缺失即自动下载（多连接分段+重试+源回退 hf-mirror→huggingface），顶栏进度条；`LAMA_ONNX_URL` 覆盖源，`LAMA_ONNX_PATH` 覆盖位置。

## 水印档案库（Rust 侧）
概念/建档案流程/阈值见 `docs/non-doubao-watermarks.md`；这里只记 Rust 侧差异与踩坑。
- `watermark_profiles.rs`：α/C 档案读写、NCC ±35% 两级定位（`NCC_SCALE_SPAN`）、`inverse_image`、学习（pair/solid/batch/auto）。`pipeline.rs` 加 `use_profile`（默认 true）：prepare 匹配写 `.wprof`，inpaint 后 `apply_profile_inverse`。qwen 档案 `include_bytes!` 内嵌；目录 `WATERMARK_PROFILES_DIR` 可覆盖，默认 `project_root/tools/watermarks`。
- CLI：`profiles`、`match <img>`、`match-profile <id> <img>`、`learn-pair/learn-solid/learn-auto/learn-batch/learn-mine`（参数同 Python）、`--no-profile`、`--profile <id>`。
- **`learn-mine`（文件夹自动聚类建档）**：`learn-mine <dir|images...> [--label-prefix X] [--min-samples N] [--corner-tol F] [--recursive] [--dry-run] [--out DIR]`。逐图检测水印 → 按「**同尺寸 + 水印框右下角相对落位** `(x2/w, y2/h)`」聚簇 → 每簇 ≥3 张交给 `auto_discover`（多候选框 + `fit` 择优），再要求自检 ≥2/3 才收，最后写复查拼图 `--out/<id>-review.png`。用它把"一堆混杂的实拍图"直接变成多个档案（`mine_watermarks`，`watermark_profiles.rs`）。**聚类标识不能用检测框尺寸**：检测框逐帧漂移大（豆包 853x200 / 1149x298 / 1014x227），落位才稳（豆包 0.964~1.000、千问 0.969~1.000）；也**不能两个坐标都除以短边**（跨长宽比不可比，还把同款水印拆成三簇——实测踩过）。只在右下角找水印，别处用 `--box`（此时按尺寸聚簇）。
- **「精确模式」**：`--profile <id>`／App 选定档案 → `forced_profile` 只用该档案定位（`match_specific`，阈值 `FORCED_MIN_SCORE=0.35`），定位失败**回落自动识别**（不硬伤）。App 侧学习档案存应用数据目录（`PROFILES_DIR_OVERRIDE`），命令 `learn_watermark`/`list_watermarks`/`delete_watermark`。
- **学习自检（必守）**：`learn_watermark` 学完先在**学习用的原图**上重新定位（`profile_self_check`），≥2/3 命中才落盘——学出来定位不到的档案存了也没用。
- **两条定位通路（别混）**：①自动路径 `match_image`→`locate_ncc_impl`（连续 α 的对比归一化 NCC，±35% 尺度搜索），**未改动**，qwen 实测分数 0.751/0.807/0.667/0.612 与改动前逐图一致；②精确模式/学习自检 `match_specific`/`profile_self_check`→`locate_profile_dense`→`locate_gap_impl`，用**顶帽 gap-score**（笔画区均亮 − 间隙区均亮，与内置豆包模板匹配同口径）。**为什么不用 NCC**：水印在纹理背景上的相关性被背景高频压垮（学习出的豆包 α 在 dist/3、6 上 NCC <0.35 定位失败、dist/1 仅 0.5）→ 定位不到就回落自动识别，精确模式与学习自检形同虚设。
- **`place` 锚点（关键）**：档案 `extra.place` 记录水印**相对右下角的偏移**（按短边归一，`place_by_anchor` 反算位置）。定位窗锚定到它 ±40px，而不是死守"贴右下角"——千问距右/下各 ~0.032 短边（55px），窄窗根本不含真值、只会在角上找到伪峰（实测偏 21px、模板分只掉到 29.3）。`learn_from_batch`/`extract_from_pair`/`build_from_uniform` 都写 `place`；老档案缺 `place` 时退化为贴角（对豆包正确）。阈值 `PROFILE_GAP_MIN_SCORE=12`（实测含水印 17.8~130、不含水印 1.1~7.3）。
- **踩坑（曾误判）**：`doubao-stamp` 不在 `EMBEDDED` 里（独立 const，只给 stamp 逆解用），`load_profile("doubao-stamp")` 必然失败 → `match-profile doubao-stamp` 打印"not located"。别当成"真值 α 也定位不了"的证据；实测真值 stamp α（覆盖率 27.8%）标准 per-window zero-mean NCC **0.772**、qwen α（38%）0.870。
- **`learn-batch` 保真度（v0.5.5 已修）**：原先逐样本"顶帽 >12 取并集"，单张图的背景强结构会把并集污染 → 框/α 形状不符、残差 13.5 过不了 12 的门限，学习直接失败。改为**逐样本顶帽后跨样本取中位数**（水印是各图共性结构，各图背景高频互不相关，中位数保留水印、压掉背景伪结构）：同召回下精确率 0.55→0.81。再加 `LEARN_CORE_ALPHA` 0.35→0.15（豆包 α≈0.53，0.35 门限把抗锯齿边与淡笔画全丢了）。**实测（1,2,3 学、6 留出）α 对真值 stamp 墨迹 IoU 0.914**（改前 0.664），`--profile` 在 dist/1/3/6 全部命中真值 (2568,1511)、模板分 58.6/33.0/53.2 → **0.0**，与内置 stamp 通路等效。两个阈值可用环境变量 `LEARN_BATCH_TOPHAT_MIN`/`LEARN_CORE_ALPHA` 微调。
- **`learn-auto` 已转正（v0.5.5）**：`auto_discover` 改为 **batch-first**——多帧 batch 与 `background_score` 的"纯色背景"资格门槛**解耦**（那套门槛 cover./contrast 按纯色假设设计，真实照片覆盖率 ~0.2% 必然被过滤，曾使该分支**永远走不到**）。现在只要 ≥3 张同尺寸图就学，选出档案的门槛是 `learn_from_batch` 自带的 `residual`+**`fit`**。
  - **`fit` 指标（关键）**：掩码内相对拟合优度 = `num/den`（整个 `dil` 掩码上"obs 偏离背景的能量"被 α·C 解释的比例，0=完美）。**`residual` 可被"缩小 α"刷低**（碎片 α 只拟合少数像素 → 残差极小，正是旧门限把覆盖率 0.2% 退化解判 ok 的原因）；`fit` 补上这一刀，`LEARN_MAX_FIT=0.4`。实测：豆包好档案 fit 0.24~0.30、松框碎片解 0.36~0.58。
  - **候选框要取多个、按 `fit` 择优**：通用检测器**有的帧准、有的帧离谱**（千问 dist/10 检出 428x83 = 真值 → residual 7.75；dist/9 714x437 → residual 12.9 学不出）。取"第一个有框的帧"是碰运气；改为逐帧检测框按**面积升序**去重取前 3 + 默认右下角框，全试一遍取 `fit` 最小。
  - **`strategy`/`self_check` 必须同时写回档案 `extra`**：曾只写进返回报告 → 落盘档案查不到来源（`learn-auto` 看起来走了 batch、meta.json 里却没有）。
  - 实测：`learn-auto` 豆包 4 张 → batch fit 0.301/self_check 59.1；千问 3 张 → batch fit 0.179/self_check 27.8；`--profile` 在 dist/1/3/6（tmpl 58.6/33.0/53.2→0.0）与 dist/7/9/10（tmpl→0.0）全部 PASS。
  - 仍有 ≥3 张同尺寸图却学不出来时**如实报"学不到"**，不回退单帧纯色猜测（那是静默失败）。App `learn_watermark` 仍并发跑两法按自检均分择优。
- **踩坑**：连通域筛选（`labels_areas`）必须跳过背景 label 0，否则 `areas[0] >= min_area` 把全图判成前景（`stroke_mask` 全屏 mask、`clean_alpha` 残留微 α 使裁剪不收缩）。
- 自实现替代 imageproc：`CrossCorrelationNormalized` 是 CCORR 不减均值 → 自写 zero-mean NCC；形态学太慢 → 自写 O(n) 滑窗方形核（`rect_morph`/`slide_extreme`）。

## 开发 / 打包 / 测试
- `cd desktop && pnpm install && pnpm tauri dev`。
- 跑 `cargo build`/`check` 必须先 `env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET`（`.zshrc` 旧 MacPorts 变量破坏 `objc2-exception-helper`），或用 `desktop/build.sh`。
- Windows 包无法在 macOS 交叉编译；`tools/package-app.sh`：`mac`（本机 DMG）/`win`（Windows Git Bash 本机，或 macOS `--remote user@host --win-repo C:/path` 经 SSH 远程构建拉回）/`both`；产物在根 `dist/`。
- `cargo test --lib`（常规 20：含 `output_dir_modes`、`restore_backup_copies_originals_back`）+ `cargo test --lib -- --ignored --nocapture`（含 `detect_box_on_qwen_samples`/`qwen_profile_matches_real_images`/`qwen_pair_learning_residual_is_tiny`/`doubao_batch_learning_recovers_stamp`/`gap_locator_finds_qwen_place_anchor`/`learn_auto_uses_batch_on_aligned_photos`/`full_run_with_lama_e2e`）。
- 推送前预检、CI 工作流、本地 E2E 与发版：见 `docs/ci.md`。
- macOS `tauri.conf.json` 的 `bundle` 带 `category`/`copyright`（Dock/Finder 显示）；窗口 `label` 固定 `main`（单实例聚焦用）。
- 桌面端单实例 + 窗口状态：`tauri-plugin-single-instance`（第二个实例聚焦已有窗口后自身退出，避免两进程共用同一 workdir/输出目录）+ `tauri-plugin-window-state`（记忆尺寸/位置）。两者在移动端整 crate 为空，注册处必须 `#[cfg(desktop)]` 守卫（否则 Android 编译失败）。**不设 `visible:false`**：该配置在移动端同样生效会让 Android 白屏，因此接受窗口恢复时短暂闪现默认尺寸。
- **关窗 / 退出语义**：`.on_window_event` 拦 `CloseRequested`——处理中 `api.prevent_close()` + 把窗口带回前台 + 发 `quit-blocked` 事件（前端在日志区提示"先取消或等完成"，Android 上 `unminimize` 不存在故单独 `#[cfg(desktop)]`）；空闲时 `app_handle().exit(0)`——macOS 默认"关最后窗口不退出进程"，会留下无窗口却仍占 ONNX 模型内存的后台进程。`.run` 里处理 `RunEvent::Reopen`（**必须 `#[cfg(target_os = "macos")]`**，该 variant 只在 macOS 存在）：点 Dock 图标把窗口 show/前置，否则看起来像卡死。
- **Dock / 任务栏进度**：处理中 `set_progress_bar`（`ProgressBarState` + `Normal`），`finish()` 必须显式发 `None` 收回——macOS 进度条是**应用级**，不收回会一直挂在 Dock 上。整段 `#[cfg(desktop)]` + `#[cfg(not(desktop))]` 空实现：`ProgressBarState`/`set_progress_bar` 在移动端不存在，不守卫就 Android 编译失败。
- **完成通知**：`tauri-plugin-notification`（capability 需 `notification:default`）。前端 `initNotifications()` 启动时申请权限（用户拒绝则静默不发），且只在窗口**未聚焦**时 `sendNotification`（正看着结果就不再弹）。`withGlobalTauri` 下插件 API 由 `tauri-codegen` 的 `read_global_api_scripts` 全局注入为 `window.__TAURI__.notification`；缺失时只会静默不弹、不会崩。
- **覆盖模式保留备份（可撤销）**：覆盖成功后只调 `cleanup_work_only()`（清 workdir、**保留** `original-watermark-backup/`），另存模式仍 `cleanup()`；前端覆盖模式给「恢复原图」按钮 → `restore_backup(root)` 把备份拷回 `root` 并删备份、返回张数。**安全前提**：`prepare` 在备份已存在时以它为 origin，重复覆盖同一目录仍基于最初原图（幂等），故备份可长期留作撤销依据；日志区「清理临时文件」会连备份一起删，前端已加二次确认。
- 结果区「逐张缩略图」：pipeline-exit 带 `outputs`，前端用 `read_thumbnail_base64`（解码后 `thumbnail(200,200)` 再编码 PNG，3 并发）渲染；桌面每张可 `reveal_path`（macOS `open -R`）。拖拽导入有高亮遮罩（enter/over/leave/drop），非图片给提示；快捷键 Cmd/Ctrl+O 选图、Cmd/Ctrl+Enter 开始（输入框内不触发）。日志区「导出日志」走 dialog `save` + `write_text_file`，便于用户反馈问题。
- 签名/公证：CI 与 `tools/package-app.sh mac` 均做分级校验（未配证书只告警），详见 `docs/ci.md`「macOS 签名与公证」。
- 图片格式：`image` crate 显式只开 `png`（jpeg/webp 由 `imageproc` 默认特性带入，`Cargo.lock` 有 `zune-jpeg`/`image-webp`）；**HEIC 不支持**（要 libheif/平台解码器，代价大），picker 与拖拽过滤都只认 png/jpg/jpeg/webp。
- 自动更新**尚未接入**，启用步骤（需先有签名密钥对）见 `docs/ci.md`「自动更新」。

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

**仅 Rust/App、Python 无对应参数**：`--output-dir <dir>`（App「选择输出目录」）自定义另存目录，`output_dir()` 在**覆盖模式下忽略**该值（否则"以为替换了原图"却写到别处）；`None`（默认）行为与历史版本逐字节一致，回归用例 `output_dir_modes` 已覆盖三种情形。

**框选精分割（`--refine` / App「框选区域精细处理」）**：框内顶帽局部对比 + 低饱和过滤 + 局部自适应阈值 + 行带约束 → 笔画级 mask（实测千问 7.png：12083px vs 整框 53592px，少重绘 4.4 倍）。关键设计：
- 精分割**不写 `.tpl`**（写了会强制启用豆包模板残留检查，非豆包水印碰巧高分 → 假残留），只写 `{name}.refinebox`（记录了原始框）；
- `verify_paths` 的 `template_applied` 支持 `None`＝自动判定（原图模板分 ≥20 且 `min(w,h)/ref_short≈1.0`）才查模板残留——千问图 scale=1.1 自动跳过，豆包图 scale=1.0 生效；
- 精分割留残留时 `residual_retry` **退化用 `.refinebox` 整框**重跑（豆包 1/6.png 实测：精分割残留 → 整框 → PASS、tmpl→0.0），保证「必然去除」，代价只是这一次多一次 inpaint。
- **与 Python 的有意分歧**：Python 的精分割也写 `.tpl`（会在非豆包图误报残留）；Rust 已改为只写 `.refinebox`。故"双端同步"此格仅指能力对齐，不是逐字节行为一致。精分割本身逐像素一致：`python tools/refine_parity_dump.py` + `cargo test --lib -- --ignored refine_box_mask_matches_python`（IoU=1.0000）。

**"App 影响周边元素"根因（v0.5.5 已修）**：旧 Rust 模板 mask 用「二值核(α>0.5) + 19x11 膨胀」（19/11 本是 Python `REFINE_DILATE_LAMA` 给退化框精分割用的），靠大膨胀补抗锯齿 → 比 Python 的「连续 α>0.03 + 1px」多盖约 38% 干净画面被模型重绘；且无顶帽 + 阈值 40 使亮背景（纸面/花墙/雪/沙滩）分数跌破阈值 → 回退整框 mask（面积再涨 3~5 倍）。修复后 Rust mask 与 Python **逐像素一致**（实测 IoU=1.0000，dist/1、3、6）。

**验证基线（v0.5.5）**：dist/1、3、6 双端 `mask 10172px`、`outside changed 0`；6.png 双端 stamp 逆解同取 `gain 1.05`（水印区 MAD 0.46、P95=2，此前 1.85/P95=16）；1、3.png 双端均按门控跳过逆解。`cargo test --lib` 19+4 passed；`compare_pipelines.py` 5/5 PASS。

一致性回归：`tools/compare_pipelines.py` 生成多场景合成图（小水印/828 大水印/贴边/400 小图/多位置），同遮罩框分别跑终端 iopaint 与 clean-cli，对比修复区 MAD。改 `lama.rs`/`pipeline.rs` 推理链路后必跑，**必须加 `--model lama`**（Python 默认 MAT，模型不同会误报 FAIL）。阈值：无缩放 MAD<8、缩放 <25；未处理区 PSNR>100dB。

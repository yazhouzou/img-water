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
- **模型下载断点续传（`fetch.rs`）**：失败/退出**不再清理分段**，下次（含重启 App）从断点续传；单连接路径也用 `Range` 接着下。**同一性判定 `PartMeta{url,total,etag}`**（`dest.with_extension("part.json")`）：双方都有 ETag 时以 ETag 为准、否则退化为"总大小一致"，判定不符即丢分段重下——否则换镜像/远端换文件后会拼出**大小对但内容错**的模型（静默损坏）。**期望大小以 HEAD 的 content-length 为准**：截断响应的 content-length 会"自圆其说"（谎报为已发出长度），只看 GET 头会把半截文件当完整文件改名落盘。并行合并先写 `.part.merge`、**改成功后**才删 `.partN`（旧实现边合并边删，崩在中间要全部重下）。临时文件名基于**去扩展名的 stem**（`lama_fp32.onnx` → `lama_fp32.part0`），`prune_temps` 用 `file_stem()` 匹配。

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
- `cd desktop && pnpm install && pnpm dev`（= `dev.sh`：先 unset `.zshrc` 旧 MacPorts 变量再 `pnpm tauri dev`；**直接 `pnpm tauri dev` 会被污染、objc 编译报错**——mac-notification-sys 头文件在 `-arch i386 -arch x86_64` + `mmacosx-version-min=10.6` 下编译失败）。
- 跑 `cargo build`/`check` 必须先 `env -u CFLAGS -u CXXFLAGS -u CCFLAGS -u LDFLAGS -u MACOSX_DEPLOYMENT_TARGET`（`.zshrc` 旧 MacPorts 变量破坏 `objc2-exception-helper`/mac-notification-sys 的 objc 编译），或用 `desktop/build.sh` / `pnpm dev`。
- Windows 包无法在 macOS 交叉编译；`tools/package-app.sh`：`mac`（本机 DMG）/`win`（Windows Git Bash 本机，或 macOS `--remote user@host --win-repo C:/path` 经 SSH 远程构建拉回）/`both`；产物在根 `dist/`。
- `cargo test --lib`（常规 20：含 `output_dir_modes`、`restore_backup_copies_originals_back`）+ `cargo test --lib -- --ignored --nocapture`（含 `detect_box_on_qwen_samples`/`qwen_profile_matches_real_images`/`qwen_pair_learning_residual_is_tiny`/`doubao_batch_learning_recovers_stamp`/`gap_locator_finds_qwen_place_anchor`/`learn_auto_uses_batch_on_aligned_photos`/`full_run_with_lama_e2e`）。
- 推送前预检、CI 工作流、本地 E2E 与发版：见 `docs/ci.md`。
- **App 图标**：矢量源 `desktop/icon-source.svg`（rsvg-convert 渲染为 `desktop/icon-source.png`，1024，圆角矩形+透明四角），生成走 manifest `desktop/icon.json`（tauri icon 支持 `default`/`bg_color` 等；`bg_color: #6a4ef0` 品牌紫——填 iOS 图标透明区、写 Android 自适应图标 `ic_launcher_background`，纯 `icon-source.png` 会重置回 #fff 白角）。命令：`cd desktop && pnpm tauri icon icon.json`；**Android 生效路径是 `src-tauri/gen/android/app/src/main/res/`（tauri icon 直接写这里，CI 同）**，`src-tauri/icons/android/` 只是历史快照、tauri icon 不再更新它（改了图标需手工 `cp -R gen/android/app/src/main/res/mipmap-* icons/android/` + `values/ic_launcher_background.xml` 保持一致）。
- macOS `tauri.conf.json` 的 `bundle` 带 `category`/`copyright`（Dock/Finder 显示）；窗口 `label` 固定 `main`（单实例聚焦用）。
- 桌面端单实例 + 窗口状态：`tauri-plugin-single-instance`（第二个实例聚焦已有窗口后自身退出，避免两进程共用同一 workdir/输出目录）+ `tauri-plugin-window-state`（记忆尺寸/位置）。两者在移动端整 crate 为空，注册处必须 `#[cfg(desktop)]` 守卫（否则 Android 编译失败）。**不设 `visible:false`**：该配置在移动端同样生效会让 Android 白屏，因此接受窗口恢复时短暂闪现默认尺寸。
- **关窗 / 退出语义**：`.on_window_event` 拦 `CloseRequested`——处理中 `api.prevent_close()` + 把窗口带回前台 + 发 `quit-blocked` 事件（前端在日志区提示"先取消或等完成"，Android 上 `unminimize` 不存在故单独 `#[cfg(desktop)]`）；空闲时 **macOS 走惯例语义：`api.prevent_close()` + `window.hide()`**——关窗只隐藏、App 留在 Dock（点 Dock 图标经 `RunEvent::Reopen` 恢复窗口），Cmd+Q / 菜单「退出」才真正结束进程（代价：ONNX 模型内存常驻，是用户明确选择的标准行为）；**Windows/Linux 保持 `app_handle().exit(0)`**——无 Reopen 事件、隐藏了回不来，会留无窗口后台进程。`.run` 里处理 `RunEvent::Reopen`（**必须 `#[cfg(target_os = "macos")]`**，该 variant 只在 macOS 存在）：点 Dock 图标把窗口 show/前置，否则看起来像卡死。
- **Dock / 任务栏进度**：处理中 `set_progress_bar`（`ProgressBarState` + `Normal`），`finish()` 必须显式发 `None` 收回——macOS 进度条是**应用级**，不收回会一直挂在 Dock 上。整段 `#[cfg(desktop)]` + `#[cfg(not(desktop))]` 空实现：`ProgressBarState`/`set_progress_bar` 在移动端不存在，不守卫就 Android 编译失败。
- **完成通知**：`tauri-plugin-notification`（capability 需 `notification:default`；插件 Android manifest 自带 `POST_NOTIFICATIONS`，Android 12 及以下无需运行时申请）。前端 `initNotifications()` 启动时申请权限（用户拒绝则静默不发），且只在用户**没在看**窗口时 `sendNotification`：桌面看 `document.hasFocus()`；**移动端必须用 `document.visibilityState`——Android WebView 切后台后 `hasFocus()` 恒为 true（实测后台轮询 35 秒全 1，用 hasFocus 会把通知永远挡掉）**。移动端切后台后 WebView 的 JS 定时器仍持续执行、IPC 事件回调可用（实测），故通知逻辑留在前端即可。`withGlobalTauri` 下插件 API 由 `tauri-codegen` 的 `read_global_api_scripts` 全局注入为 `window.__TAURI__.notification`；缺失时只会静默不弹、不会崩。
- **覆盖模式保留备份（可撤销）**：覆盖成功后只调 `cleanup_work_only()`（清 workdir、**保留** `original-watermark-backup/`），另存模式仍 `cleanup()`；前端覆盖模式给「恢复原图」按钮（`danger ghost` 弱化 + 原生二次确认）→ `restore_backup(root)` 把备份拷回 `root` 并删备份、返回张数。**安全前提**：`prepare` 在备份已存在时以它为 origin，重复覆盖同一目录仍基于最初原图（幂等），故备份可长期留作撤销依据。**已移除手动「清理备份与临时文件」入口**（主界面按钮 + 菜单项 `ID_CLEANUP`）：临时文件本就随 `run` 自动清理，备份是"恢复原图"的前提、不算无用文件（恢复后自动删）；普通用户无需也不应手动处理这些细节。
- 结果区「逐张缩略图」：pipeline-exit 带 `outputs`，前端用 `read_thumbnail_base64`（解码后 `thumbnail(200,200)` 再编码 PNG，3 并发）渲染；桌面每张可 `reveal_path`（macOS `open -R`）。拖拽导入有高亮遮罩（enter/over/leave/drop），非图片给提示；快捷键 Cmd/Ctrl+O 选图、Cmd/Ctrl+Enter 开始（输入框内不触发）。日志区「导出日志」走 dialog `save` + `write_text_file`，便于用户反馈问题。
- **移动端（Android）交互（v0.5.7）**：
  - **Toast 反馈**（`showToast`/`dismissToast`，`ui/main.js`）：移动端日志区默认折叠（`log-box` 不展开），导出/分享/恢复/导入/返回键拦截的结果**必须浮层提示**，只写日志等于没有反馈；同 `key` 原地复用（导入进度刷新不闪烁）、`sticky` 不自动消失（进行中态）；同屏最多 3 条、点击关闭、`aria-live`。
  - **导入（`import_files`）三项修复**：① **先全部拷贝成功后才删旧批次**——旧实现先删后拷，任一文件失败（HEIC/权限/超 200MB）会把上一批删掉而新批残缺、界面文件列表变成幽灵文件，点处理全报"文件不存在"；② 单张失败**只跳过**并在返回值 `skipped` 里带原因（前端 Toast「N 张被跳过」+ 日志明细），不再一张坏图拖垮整批；③ 按**魔数**校验 PNG/JPEG/WebP（HEIC/HEIF 拒收并提示"先导出为 JPEG/PNG"——`image` crate 不支持 HEIC，不拦会拖到"开始处理"才报难懂的解码错），拷贝中途失败删除半截文件（本地 `fs::copy` 与 content:// 流式拷贝同样处理）。每张拷完 emit `import-progress`（导入期锁「选择图片/开始处理/刷新」）。④ **字节拷贝移出主线程**（P3-11，`android_uri.rs`）：主线程 JNI 只做解析 URI/查显示名/`openFileDescriptor("r")`+`detachFd()`（毫秒级），读循环在 Rust 线程 `copy_from_fd`（fd 由 native 持有、读完 drop 即关）；provider 不支持 fd 时回退旧的主线程流式拷贝并打 `[import] provider 不支持 fd 打开…` 日志（正常相册 provider 都走 fd 快车道）。此前整个拷贝在主线程，29 张大图有 ANR 风险。
  - **导入名规范化**（`to_png_name`）：`.jpg/.jpeg/.webp` → `.png`；管线输出恒 PNG，保留原名会"名字 jpg、内容 PNG"错配（分享给其它应用可能解码失败），旧实现追加成 `.jpg.png` 也很难看。
  - **移动端输出语义**：处理的始终是应用私有副本——`overwriteMode()` 在移动端恒 false、CSS 隐藏 `output-row`/`output-hint`、不显示「恢复原图」（覆盖的是副本不是相册原图，误导）；banner 用 `doneSubSaveMobile`（不提 watermark-cleaned 目录，用户看不到应用私有目录）；「刷新」按钮也隐藏（导入模式目录内容不会变）。
  - **系统栏随主题（图标 + 背景）**：edge-to-edge 下状态栏/导航栏透明，图标深浅与避让区露出的**窗口背景色**都由系统按"系统主题"定，App 内手动切主题时两者都会不一致（深色 App + 浅色系统 → 图标看不清；浅色 App + 深色系统 → 顶部/底部仍是黑的）；`set_system_bars` 命令经 JNI 调 `MainActivity.setSystemBarAppearance`——`WindowInsetsControllerCompat` 设图标深浅 + `window.setBackgroundDrawable` 把窗口背景设为 CSS `--bg` 同值（#12141a/#f4f5f9，content 的避让 padding 区露出的正是窗口背景），前端 `applyTheme` 每次调用、失败静默。
  - **返回键二次确认**：空闲且有导入批次时退出会丢整个会话（框选/结果无持久化），2.5 秒内连按两次才真正退出，首次 Toast「再按一次返回键退出应用（当前结果将丢失）」；无批次时空闲返回直接退出（常规预期）。
  - **noWatermark 反馈**：`RunSummary.no_watermark`（verify passthrough 的图）→ pipeline-exit payload `noWatermark` → banner 全部未命中时显示「未检测到水印 · N 张（已原样输出）」、部分命中显示「其中 N 张未检测到水印，已原样输出」——此前全部 passthrough 也显示"处理完成"，用户以为去了水印实际没改。
  - **阶段级进度**：`run()` 在 prepare/save 两端也发 progress 事件（此前只有 inpaint 内部发，进度条长时间停在 0% 看不出在动）；`body[aria-busy='true'] .run-progress-track::after` 叠加流动高光动画（1.2s 循环），单图长推理期间也有"正在进行"的视觉反馈。
  - **灯箱双指缩放**：多指下 pointer 事件语义乱，单独用 touch 事件实现（第二指 `!ev.isPrimary` 进入 pinch 并终止单指拖动；两指中点锚定；松开一指无缝回到拖动）；长按图片不弹系统菜单（`-webkit-touch-callout:none` + contextmenu 拦截）；处理中按返回键 Toast「完成或取消后再退出」（原来静默拦截像卡死）；处理中锁定文件勾选（改选择会让"开始处理（N）"与任务对不上）。
  - **UI 布局要点（v0.5.7）**：移动端顶栏单行 53px——`body.mobile .subtitle` 隐藏（品牌区过宽会把主题/语言按钮挤成第二行，实测顶栏 98px 两行）；主题/EN 按钮贴右上角（`env-area { margin-left: auto }`）；`.btn` 全局 `white-space: nowrap`（灯箱"适应窗口/关闭"、框选"取消"曾被挤成竖排）；`banner-actions` 移动端 `width: 100%`（否则 flex:1 按钮按内容宽均分成 ~63px，"保存到相册"被折成两行——真机横幅按钮换行 bug 的根因）。
- **移动端 App 化改版（v0.6.0，2026-11）**：
  - **四视图整页滚动**：`body.mobile` + `state-empty/picked/running/done` 驱动——空态（大图标引导）、列表（两列缩略图网格 `.m-file-card` + 底部 fixed 操作栏）、处理中（全屏 `.m-run` 进度页）、结果（banner + 3 列结果网格 + `details.collapse` 折叠对比/日志）。旧版把桌面"固定一屏+面板内滚"搬到手机：右面板 950px 内容挤进 368px 小窗、file-list 仅 120px、处理完成找不到保存按钮（真机 CDP 实测）。
  - **「添加图片」可追加**：`import_files` 带 `dir`（`resolve_append_dir` 校验须为 app_data/imports 下 `import-*` 目录）追加到当前批；`remove_import_file`（`safe_import_name` 防路径穿越）、`clear_import_batch`（返回删除张数）；`ImportResult.total` 为批次总数（toast「已导入 N 张，共 M 张」）。
  - **系统相册 Picker**：`pick_images` → `android_pick.rs` JNI 调 `MainActivity.pickImages(max)`（`ActivityResultContracts.PickMultipleVisualMedia`，Android 13+ 系统 Photo Picker、旧版自动回退；fire-and-forget，15s 超时仅判定"启动"成败）；结果走 Kotlin `evaluateJavascript("window.__onAndroidPickResult(JSON)")` 全局回调（与返回键同模式，不走 IPC）；`pick_images` 抛错时前端回退 dialog 文件选择器。JSONArray 由 org.json 生成防注入。**Kotlin 侧直拷**：回调里后台线程 `contentResolver.openInputStream(uri)` 拷到 `cacheDir/import-pick/`（每次清目录重建、`queryDisplayName` 取名、重名 `-1/-2` 后缀），完成后回传**本地路径数组**（JS 零改动；Rust `copy_one` 本地路径走 `fs::copy`）。**旧 content:// 跨 JNI 拷贝链（`with_webview`→`MainPipe`(bounded 8)→`exec`→`copy_from_fd`）在 Ace 2 上稳定卡死**（批次目录已建、>6 分钟不返回，远超 120s `recv_timeout` 理论上限；独立验证 MediaProvider 读文件正常、无线程卡点，根因未 100% 锁定，架构绕开）。
  - **picker 返回后触摸卡死 → rebuild + 批次恢复**：ColorOS 上从 picker 返回后 WebView 输入分发整体卡死（轻量 `onResume`/解冻无效），导入完成后 JS `invoke('rebuild_webview')` → `android_pick.rs` JNI → `MainActivity.rebuildWebView()`（`recreate()`，28ms）；重建后 `restoreLastFolder` 移动端分支读 `localStorage['wm-mobile-batch']`（`MOBILE_BATCH_KEY`：导入成功写、清批删）恢复已导入列表——冷启动进程重启同靠它。Ace 2 实测全链路（picker→直拷→copy→rebuild→恢复）2.5 秒。
  - **设置页 `.m-settings`**：全屏覆盖（`body.m-settings-open`）；外观 segmented（跟随系统/浅色/深色）/ 语言（中文/English）；高级选项与精确模式（DOM 仅 `isMobile` 时从 `.run-area` 搬入 `#m-settings-options`）；清除本批导出（confirm → `clear_import_batch` → 空态）。主题/语言逻辑从 init 闭包**外提为模块级**（`applyTheme`/`setThemeMode`/`syncMenuLang`/`applyLang`/`syncSettingsUi`），顶栏按钮与设置页共用。
  - **返回键链路扩展**：灯箱/框选/免责声明 → 设置页 → 处理中拦截 → 结果页回列表（结果保留）→ 有批次二次确认（2.5s）→ 退出。`setState` 里同步 `btn-back-list.hidden`（移动端仅结果页显示）；`.icon-btn` 的 `display` 会盖掉 `[hidden]` 的 UA 样式，CSS 需显式 `#btn-back-list { display: none }` + `body.mobile #btn-back-list:not([hidden]) { display: grid }`（踩过：桌面右面板露出返回箭头）。
  - **`setupMobileLayout()` 仅 `isMobile` 的 DOM 重排**：`btn-pick` 搬入 `.run-buttons`（与「开始处理」并排）、两个 `details.advanced-options` 搬入设置页、`compare-box`+`review-grid` 包进新建 `details.collapse`（插在 `#log-box` 前）。顶层（init 前）执行避免首帧闪桌面结构；桌面端 DOM 与样式逐字节不变（回归：`?mobile=0` 检查 btnPickParent/advancedInRunArea/compareBoxParent）。
  - 已知布局坑：`body.mobile .file-list` 是两列 grid，空态需 `body.mobile.state-empty .file-list { display: block }` 覆盖（否则 empty-state 只占半列、按钮贴左）。
- 签名/公证：CI 与 `tools/package-app.sh mac` 均做分级校验（未配证书只告警），详见 `docs/ci.md`「macOS 签名与公证」。
- 图片格式：`image` crate 显式只开 `png`（jpeg/webp 由 `imageproc` 默认特性带入，`Cargo.lock` 有 `zune-jpeg`/`image-webp`）；**HEIC 不支持**（要 libheif/平台解码器，代价大），picker 与拖拽过滤都只认 png/jpg/jpeg/webp。
- **自定义系统菜单（`menu.rs`，仅桌面端，整模块 `#[cfg(desktop)]`）**：应用/文件/编辑/帮助；文件项有标准加速键（Cmd+O 选文件夹、**Cmd+Return 开始处理**）。**加速键键名必须用 muda 的 token（`ENTER`，不是 `Return`）**：tauri 侧是 `s.parse().ok()`，写错**不报错**、只是静默变成"没有快捷键"（菜单在、点着能用、按键无反应）——踩过 `CmdOrCtrl+Return`，故加速键集中定义 + 单测 `accelerator_strings_use_known_modifiers_and_keys` 兜住同类拼写错误。**菜单项只发 `menu-action` 事件**，动作仍由前端已有按钮的 `click` 实现——逻辑只写一次，「处理中禁止开始/选图」等守卫都在按钮 `disabled` 上，因此菜单项不随运行状态灰显。帮助项在 Rust 侧直接开链接。**菜单语言运行时同步**：`set_menu_lang` 重建菜单（前端在 init 与语言切换时调用），失败只记一行日志、不影响处理。**有菜单时前端跳过自己的 Cmd+O/Cmd+Enter 处理**（macOS 菜单会先消费按键，否则弹两次对话框）：Rust 在 `on_page_load` 里 `eval` 置 `window.__WM_HAS_MENU`（页面加载与 JS 初始化先后不定，故同时回调 `window.__WM_MENU_READY`），浏览器预览无此标志、仍走 JS 快捷键。**菜单 API 在移动端不存在**：`pub mod menu`、`.on_menu_event`、`set_menu` 全部要 `#[cfg(desktop)]` 守卫，否则 Android 编译失败（`tools/android-check.sh` 可验）。
- **panic 兜底**：后台任务一律走 `spawn_guarded`（`run_pipeline`/`setup_model`）——panic 会被 `catch_unwind` 转成失败 `pipeline-exit` 并复位 `running`；**不做的话**线程静默死掉、`running` 永远 true，前端卡"处理中"、关窗被永久 `prevent_close`、Dock 进度条不收回，只能强杀。同步重型命令 `learn_watermark` 走 `guard`（panic 会 unwind 击穿 Tauri 事件循环 → App 直接崩掉）。
- **无障碍**：图标型按钮（主题 🌙/EN）用 `data-i18n-aria` + `aria-hidden` 包 emoji；`#log` 是 `role="log" aria-live="polite"`；两个遮罩与灯箱是 `role="dialog" aria-modal`；处理中 `body[aria-busy]`；动态插入的图片有 `alt`；`:focus-visible` 全局焦点环；`applyI18n` 同步 `<html lang>`。
- **Intel / universal 包不做**（已定：macOS 全线 Apple Silicon；`ort-sys` 的 `download-binaries` 也无 `x86_64-apple-darwin` 产物）。
- 自动更新**尚未接入**，启用步骤（需先有签名密钥对）见 `docs/ci.md`「自动更新」。

## UI 本地联调（零 SDK）
`./tools/ui-preview.sh` 起 http 服务打开 `desktop/ui/index.html`；`ui/mock.js` 在浏览器 mock 全部 Tauri API（打包内自动失效），`?mobile=1/0` 强制移动/桌面、`?nomodel=1` 模拟未下载；改 ui 下文件刷新即可，不依赖 CI；Windows: PowerShell 需 `chcp 65001`。

## 双端同步
Python（终端）与 Rust 口径**除修复模型外已对齐**：

| 能力 | Python | Rust/App |
|---|---|---|
| 模板笔画 mask（**默认亮字 α** 连续 α>0.03 + 1px 膨胀） | ✓ | ✓ |
| 顶帽 gap-score + 阈值 20 | ✓ | ✓ |
| 结果级验证（mask 外零改动，FAIL 拒绝落盘） | ✓ | ✓（`--force` 强制） |
| 豆包 stamp 逆解（**默认关**，`--inverse` 开；App「逆解还原真实纹理」勾选） | ✓ | ✓ |
| 水印档案库逆解（**随 `--inverse`**，默认关；档案仍作精确 mask） | ✓ | ✓ |
| 结果驱动 footprint 升级（残留 >5 时，1 轮，默认开） | ✓ | ✓ |
| 过冲（暗字形）自动重试（1 轮，默认开） | ✓ | ✓ |
| 残留自动重试（1 轮，默认开，逆解图跳过） | ✓ | ✓（`--no-retry` 关） |
| 框选/检测框内笔画精分割（`--refine`，实验性） | ✓ | ✓（`--refine`；App 默认勾选；残留自动退化整框，见下） |
| `--any-position` / DBNet | ✓ | ✓（macOS v0.5.4） |
| 水印档案库（α/C 精确定位与 mask；逆解随 `--inverse`） | ✓ | ✓（macOS） |
| 修复模型 | 默认 MAT | LaMa ONNX |

**唯一残余差异**：修复模型（MAT vs LaMa ONNX）——两者都是生成式路线，复杂纹理（花墙/花丛）MAT 更稳；`scale≈1.0 + 邻域纹理复杂` 的图会被 stamp 逆解接管，此时模型差异不重要。

**逆解默认关 + 两轮自动兜底（v0.5.5+，与 Python 对齐）**：`options.inverse` 默认 **false**（CLI `--inverse`、App「逆解还原真实纹理」勾选才开）——实拍图上逆解与资产 α/C 不完全一致，会留白线/彩点/暗字形伪影；实测 7 张实拍图纯生成式在 6 张上模板残留更低。逆解成功写 `{name}.inv`：① `verify_repaired` 见到 `.inv` 就关掉模板残留判据（gap-score 对逆解恢复的真实纹理同样有响应，会误报）；② `residual_retry`/`overshoot_retry` 跳过 `.inv` 图（精确解别被生成式重跑盖掉）。两轮兜底在 `run()` 里 order 固定：**先 `overshoot_retry`**（`const INVERSE_OVERSHOOT_MAX=10`、`OVERSHOOT_DILATE=(15,15)`；用 SOURCE 定位水印、量结果图 `watermark_profiles::result_overshoot_score` 的笔画区−间隙区低频差，负得太多=暗字形 → 扩 mask 重画），**再 `residual_retry`**（5x5 膨胀）。

**仅 Rust/App、Python 无对应参数**：`--output-dir <dir>`（App「选择输出目录」）自定义另存目录，`output_dir()` 在**覆盖模式下忽略**该值（否则"以为替换了原图"却写到别处）；`None`（默认）行为与历史版本逐字节一致，回归用例 `output_dir_modes` 已覆盖三种情形。

**框选精分割（`--refine` / App「框选区域精细处理」）**：框内顶帽局部对比 + 低饱和过滤 + 局部自适应阈值 + 行带约束 → 笔画级 mask（实测千问 7.png：12083px vs 整框 53592px，少重绘 4.4 倍）。关键设计：
- 精分割**不写 `.tpl`**（写了会强制启用豆包模板残留检查，非豆包水印碰巧高分 → 假残留），只写 `{name}.refinebox`（记录了原始框）；
- `verify_paths` 的 `template_applied` 支持 `None`＝自动判定（原图模板分 ≥20 且 `min(w,h)/ref_short≈1.0`）才查模板残留——千问图 scale=1.1 自动跳过，豆包图 scale=1.0 生效；
- 精分割留残留时 `residual_retry` **退化用 `.refinebox` 整框**重跑（豆包 1/6.png 实测：精分割残留 → 整框 → PASS、tmpl→0.0），保证「必然去除」，代价只是这一次多一次 inpaint。
- **与 Python 的有意分歧**：Python 的精分割也写 `.tpl`（会在非豆包图误报残留）；Rust 已改为只写 `.refinebox`。故"双端同步"此格仅指能力对齐，不是逐字节行为一致。精分割本身逐像素一致：`python tools/refine_parity_dump.py` + `cargo test --lib -- --ignored refine_box_mask_matches_python`（IoU=1.0000）。

**"App 影响周边元素"根因（v0.5.5 已修）**：旧 Rust 模板 mask 用「二值核(α>0.5) + 19x11 膨胀」（19/11 本是 Python `REFINE_DILATE_LAMA` 给退化框精分割用的），靠大膨胀补抗锯齿 → 比 Python 的「连续 α>0.03 + 1px」多盖约 38% 干净画面被模型重绘；且无顶帽 + 阈值 40 使亮背景（纸面/花墙/雪/沙滩）分数跌破阈值 → 回退整框 mask（面积再涨 3~5 倍）。修复后 Rust mask 与 Python **逐像素一致**（实测 IoU=1.0000，dist/1、3、6）。

**模板 mask 默认亮字 α + 结果驱动 footprint 升级（v0.5.6）**：`6e0823a` 曾把两端默认一刀切换成"含暗描边的完整 footprint"（防低对比背景暗字形残影），但 footprint 会圈进水印周围高频纹理 → 6.png 花枝被重绘抹平（Python 10172→14685px）。现改为**默认亮字 α**（只盖笔画、保纹理，两端一致），仅当结果模板残留 >`FOOTPRINT_RETRY_MIN`(5) 时升级 footprint 重跑（Python `_footprint_retry`、Rust `footprint_retry`，先于过冲/残留重试，逆解图跳过）。footprint mask 逐像素一致：`python tools/footprint_parity_dump.py` + `cargo test --lib -- --ignored template_footprint_mask_matches_python`（非缩放图 IoU=1.0000；缩放图 ~0.95，源于 `image` crate Triangle 与 cv2 INTER_LINEAR 重采样差异，亮字 α 通路同样存在）。Python footprint 用 `doubao-wm-stamp-alpha.png`（8-bit，与 Rust 同源）而非 npz float32——后者逐像素差 ~0.002，在 α≈0.031 阈值处翻边界像素使两端 mask 对不齐（IoU 0.978）。

**验证基线（v0.5.5）**：dist/1、3、6 双端 `mask 10172px`、`outside changed 0`；6.png 双端 stamp 逆解同取 `gain 1.05`（水印区 MAD 0.46、P95=2，此前 1.85/P95=16）；1、3.png 双端均按门控跳过逆解。`cargo test --lib` 19+4 passed；`compare_pipelines.py` 5/5 PASS。

一致性回归：`tools/compare_pipelines.py` 生成多场景合成图（小水印/828 大水印/贴边/400 小图/多位置），同遮罩框分别跑终端 iopaint 与 clean-cli，对比修复区 MAD。改 `lama.rs`/`pipeline.rs` 推理链路后必跑，**必须加 `--model lama`**（Python 默认 MAT，模型不同会误报 FAIL）。阈值：无缩放 MAD<8、缩放 <25；未处理区 PSNR>100dB。

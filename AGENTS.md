# AGENTS.md

> 精简行动手册。深度理由/踩坑史见 `docs/lessons.md`，其它水印/档案库见 `docs/non-doubao-watermarks.md`，桌面端/App 见 `desktop/AGENTS.md`，CI/发版见 `docs/ci.md`，Android 见 `docs/android.md`（**均按需读取，勿默认全读**）。

## 目标
处理 PNG 的"豆包AI生成"水印（及用户指定其它水印）：去水印、覆盖原文件，**不影响水印外任何像素**。

## 触发
- "去水印/处理图片" → 根目录所有 `*.png` 豆包水印，走完整流程。
- "其它水印" → 非豆包（千问AI/AI生成/3DMGAME…）。**不一定在右下角**，先看复查图确认文字/位置/范围再 `--mask-box`。
- 指定范围（`1.png 到 6.png`、`@1.png @2.png`）只处理指定图；否则根目录全部 `*.png`，**不筛选**。

## 工作流与命令
1. 确定目标图、读尺寸。
2. 备份到 `original-watermark-backup/`（已存在不覆盖）。
3. 出右下角裁剪复查图，确认水印位置/尺寸/范围（不用于筛选）。
4. 遮罩：自动检测（默认，检不到跳过）→ 检不到或其它水印用 `--mask-box` 手动。只覆盖水印文字。
5. 模型默认 **mat**（复杂结构优于 LaMa）；MAT 权重缺失用 **ghfast.top 镜像**下到 `~/.cache/torch/hub/checkpoints/`。**自裁小块推理**（`_iopaint_batch`）：iopaint 的 MAT 会把输入补齐成 512 的方形（`min_size=512`/`mod=512`/`pad_to_square`），整图会被补到 1024²、耗时非线性暴涨（实测同图整图 ~90s、512² 裁块 ~8s，**约 10x**）——故按水印位置自裁 ≤512（`CROP_SIDE_MAX`）方块，只把 **mask 像素**贴回原图（mask 外逐字节不变）。`--model lama` 回退。
6. 覆盖前出候选复查图，确认无残留/糊块；`overwrite-review` FAIL 拒绝覆盖。
7. 覆盖后出落盘复查图确认；通过后清理备份与 `/tmp` 复查产物。

会话用分步命令（便于覆盖前后读复查图）；勿临时新建脚本。环境 `.img-inpaint-venv/`（非 `/tmp`，勿删）；`iopaint` 必带子命令（`list`/`run`）否则 `Missing command`；环境缺失跑 `./tools/ensure-inpaint-env.sh`。

```bash
# 分步（默认）：prepare→inpaint→review-lama→overwrite-review→cleanup
P=.img-inpaint-venv/bin/python; S=tools/remove_doubao_watermark.py
$P $S prepare [文件]
$P $S inpaint [文件]              # 默认**纯 MAT**；随后两轮自动兜底：① 暗字形（过冲）→ mask 膨胀 15x15 重跑（1 轮）；② 水印残留 → mask 膨胀 5x5 重跑（1 轮）
$P $S --inverse inpaint [文件]     # 额外启用 stamp 逆解（逐图标定 gain+墨色 C + 结果择优，可保真实纹理，但在实拍图上会留暗字形/白线/彩点等伪影，故默认关）
$P $S --no-retry inpaint [文件]   # 关残留重试
$P $S --sd inpaint [文件]         # 水印压在高频纹理（花丛/枝叶）上时用 SD 生成式修补（默认关：要下 ~4GB 模型、单图分钟级；按背景纹理自动路由，平滑图仍走 MAT）
$P $S review-lama [文件]          # 候选复查图 + PASS/WARN/FAIL
$P $S overwrite-review [文件]     # FAIL 拒绝覆盖
$P $S --force overwrite-review [文件]
$P $S cleanup [文件]
# 一键（用户自助/明确"一键快速处理"）
$P $S run [文件]                  # 需看复查图加 --keep-work
$P $S --root /path run            # 项目外目录
```
- 不传文件 = 根目录全部 `*.png`。
- 其它水印 `--mask-box x1,y1,x2,y2`（负数=相对右下；argparse 负数需 `=`；分号多框）。
- 非豆包（任意位置/多框、`--any-position`/DBNet、档案库 `learn-*`）见 `docs/non-doubao-watermarks.md`。

## 遮罩规则
按当前图尺寸生成，不可假设尺寸相同。
1. **自动检测（默认）**：全图白字 + 右下兜底，框过**字符行判据**，膨胀只可 5x5 一次（阈值/判据参数见 `docs/lessons.md` §10）。右下兜底另有**字符高带**：字符/行块高须落在短边 `2.6%~5.0%`（豆包字形 ≈3.5%），否则已去水印图右下角的背景碎块（地毯/纸面/花丛，1.3%~2.0%）会被聚成"字符行"误修；行块宽高比 `2.5 ≤ 比 ≤ 8`（水印整行 4.0~4.6，**矮胖单块 1.2~1.9** 会被 9x3 膨胀连成"行"，下限挡掉）。
2. **只留贴右下角的框**（豆包必贴右下），分散水印用 `--mask-box`。
3. **检不到 → 跳过**（空 mask 不进模型）；`--mask-box` 指定则强制。
4. **模板 footprint mask**：豆包字形固定，资产 `tools/doubao-wm-template.png`/`.json`；按短边缩放后在右下角 40px 窗口用**顶帽 gap-score** 匹配；命中写 mask 并写 `.tpl`。**命中须 raw gap≥`TEMPLATE_MIN_SCORE`(20) 且顶帽 gap≥`TEMPLATE_TOPHAT_MIN`(15) 双过**——raw 会把"水印处整体偏亮"（强纹理背景亮块，尺度>顶帽核）计入而误检（已去水印 1/6.png raw 27.3/34.6、顶帽仅 ≤6.4；真水印顶帽 ≥19.9）；未过顶帽按未命中处理（上报顶帽分）。mask 来源优先级：**stamp 完整 footprint**（`tools/doubao-wm-stamp-alpha.png`，含暗色描边/抗锯齿，α>0.03）> 模板亮字 α（`tools/doubao-wm-alpha.png`，**仅亮字核心、漏描边**）> 二值模板。**必须用含描边的完整 footprint**——只盖亮字会在低对比背景（木纹/纸面）MAT 重绘后留暗字形残影，且 gap-score 判据检测不到暗描边（会误判 PASS）。不足回退整框，再不到则空 mask 跳过。
5. **模板命中即完成**（非 `--any-position`）。
6. **空 source 防御**：全部无 mask 透传时 source 为空，必须跳过 iopaint（空目录报 `invalid --image` 退出 255）。
7. **`--refine`（框内笔画精分割）默认关、实验性**：Python 由 `--refine` 开；Rust/App 由 `--refine`／UI「框选区域精细处理」（默认勾选）开，精分割留残留自动退化整框（详见 `desktop/AGENTS.md`）。

（判据成因与反例见 `docs/lessons.md` §1–§2、§10。）

## 验证、备份与质量标准
- **验证（`verify_paths`）**：`overwrite-review`/`review-lama` 打印 `[PASS]/[WARN]/[FAIL]`。落盘**逐张判定**（与 Rust `finalize_outputs` 对齐）：FAIL 的图**保留原图不落盘**、其余照写，`--force` 跳过自检全部照写——**不再"任何一张 FAIL 就整批拒绝"**（旧行为下单张复杂背景的模板残留误报会让整批看似"跑了没用、水印还在"）。判据：① mask 外零改动（mask 外差异 >2 即 FAIL，必是 bug）；② 模板残留（`.tpl` 原图命中而修复后仍命中 → FAIL/WARN）——**结果分取源图水印锚点处的分**（非角窗最大分），且须 `≥ TEMPLATE_RESIDUAL_DOMINANCE`(0.85)×角窗最大分（"水印处即主导峰"）；强纹理背景（沙地/花墙）在角窗别处凑的高分不再误判已去干净图为 FAIL（实测 1.png 锚点 11.0/窗 27.3、6.png 锚点 24.6/窗 34.6）；③ mask 面积 >8% 告警。
- **逆解默认关（`--inverse` 开）**：逆解按 stamp α/C 逐像素反解，能保真实纹理，但前提是"该图水印与资产 α/C 完全一致"——实拍图上并不总成立，会留**伪影**（4.png 白线、2.png 彩点、1.png 暗字形）。实测 7 张实拍图中 MAT 版模板残留**在 6 张上更低或相等**，故默认纯 MAT；需要保纹理的复杂背景（花丛类）可显式 `--inverse`。
- **SD 生成式修补（默认关、`--sd` 开）**：MAT/LaMa 是"平滑填充器"，水印压在**高频纹理**（花丛/枝叶）上时只会插值、留可见糊块（6.png 实测花瓣被抹平成红块）。`--sd` 改用 Stable Diffusion（SD1.5 inpainting + LCM-LoRA 6 步、fp16/MPS、`diffusers`）生成式修补，能"脑补"可信纹理。按**水印周边环带高频能量**路由（`_ring_hf ≥ SD_TEXTURE_MIN`(9)；平滑图 1–5、花丛 ~10），只对纹理图启用；自裁 ≤512 方块、只贴 mask 像素；SD 图跳过逆解与两轮重试（生成式，MAT 重试会把它糊掉）。代价：要下模型（`runwayml/stable-diffusion-inpainting`；直连 HF 不通自动走 `hf-mirror.com`）、单图分钟级（M1 8GB 实测 LCM 6 步 2–6min，MAT 仅 ~10s）。**与 App/Rust 无关**（App 用自带 ONNX LaMa，不支持 SD）。
- **暗字形（过冲）自动重试（默认开、1 轮）**：MAT 在 mask 盖不住水印**淡边缘/暗描边**时会把残留暗边当内容保留 → 结果字形区比周围暗（`_result_overshoot_score` 负值，1.png 地毯实测 −23）。检测到即把 mask 膨胀 `OVERSHOOT_DILATE=15x15` 重跑该图（实测 −23 → −3~−6），先于残留重试执行。这是"换一张图就失效"的兜底：判据只关乎结果本身、与图无关。
- **残留重试（默认开、1 轮、`--no-retry` 关）**：残留判定 = 绝对分 ≥ `TEMPLATE_MIN_SCORE`(20) 或 相对分 ≥ 原分 `TEMPLATE_RESIDUAL_RATIO`(0.2) 且 ≥ `TEMPLATE_RESIDUAL_FLOOR`(8)——低对比残影达不到 20，靠相对判据兜底。命中即 mask 膨胀一级（`RETRY_DILATE=5x5`）隔离重跑；仍残留交 `overwrite-review` 拒绝。**逆解已生效的图跳过重试**——逆解是精确物理恢复，生成式 MAT 重试会把它重新糊掉（`.tpl`/`.wprof` 侧车标记者不入重试候选）。
- **备份**：`original-watermark-backup/` 不存在才复制、不覆盖；通过后删备份与 `/tmp` 复查产物。
- **覆盖安全（md5）**：prepare 记源 md5 到 `WORK/manifest.json`，`overwrite-review` 覆盖前校验，不一致/无 manifest 拒绝。**`--root` 不得指向 `dist/`**。
- **回归**：`tools/watermark_regression.py`（L1 快；`--e2e --model lama` 慢）；**L1 已接入 CI**（`.github/workflows/watermark-regression.yml`，改 `tools/**` 的推送/PR 必跑，含 dist 无关的合成泛化 + 墨色标定 + MAT 低频带偏用例）；改管线后先 `verify` 再回归。
- **新增功能不得影响老功能**：改 mask/检测/管线时，老路径（豆包模板命中、已去水印跳过）的默认行为必须逐字节不变；任何新能力须同时在 `watermark_regression.py` 增加防回归用例（尤其"暗描边/低对比残影"这类 gap-score 测不到的盲区），CI 不绿不许合并。
- **质量标准**：① 无水印残留；② 背景纹理自然无矩形糊块；③ 不破坏主体/边缘/地面/水面；④ 回复前必须完成**落盘复查**。

## 会话与提效
- 定位优先 `Grep`/`Glob`，再用 `Read` 带 `offset/limit` 读窗口；**不整读大文件**；命令输出收敛。
- `run` 仅用户自助/明确"一键快速处理"时用；同批图只跑一次 `iopaint run`（模型只加载一次）。
- 图片复查只读拼图，不逐张读整图；处理中简洁更新，不反复展示中间候选（质量有疑问除外）。
- 最终只说：处理范围、环境可否复用、备份与 `/tmp` 是否清理、有无残留/风险。
- 压缩续接：以用户最新消息为准，只接续 `In Progress`/`Blocked`/未完成的验证或收口；不展开历史/已完成；判断不了先一句话确认。

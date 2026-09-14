# AGENTS.md

> 精简版行动手册。深度理由/已证伪路线见 `docs/lessons.md`；桌面端/App/CI 细节见 `desktop/AGENTS.md`（均按需读取，不自动加载）。

## 项目目标

本项目处理 PNG 图片的"豆包AI生成"水印（以及用户指定的其它水印）：去掉水印、覆盖原文件，且**不影响水印之外的任何像素**。

## 会话触发含义

- "处理图片/去掉水印/清除水印/去水印" → 项目根目录所有 `*.png` 的"豆包AI生成"水印，按本文件完整流程执行。
- "去掉其它水印/清除其它水印" → 非豆包水印（如"千问AI""AI生成""3DMGAME"等）。**其它水印不一定在右下角**，必须先通过复查图确认文字、位置与遮罩范围，再用 `--mask-box` 指定。
- 用户指定范围（`1.png 到 6.png` 或 `@1.png @2.png`）时只处理指定图片；否则处理根目录全部 `*.png`，**不做"是否还有水印"的筛选**。

## 默认工作流

1. 确定目标图片；确认存在并读取尺寸。
2. 备份原图到 `original-watermark-backup/`（已存在不覆盖）。
3. 生成右下角裁剪复查图确认水印位置/尺寸/遮罩范围（不用于筛选）。
4. 遮罩两级来源：自动检测（默认，检测不到就跳过）→ `--mask-box` 手动指定。只覆盖水印文字区域。
5. 修复模型默认 **mat**（结构边界重建优于 LaMa，代价单图 ~2 分钟 vs ~10 秒）；`--model lama` 可回退。MAT 权重 `Places_512_FullData_G.pth` 若缺失，用 **ghfast.top 镜像前缀**手动下载到 `~/.cache/torch/hub/checkpoints/`（iopaint 自动下载会挂起）。
6. 覆盖前生成候选复查图；确认无残留、无明显糊块后再覆盖。
7. `overwrite-review` 落盘前过验证闭环，FAIL 拒绝覆盖。
8. 覆盖后再生成落盘复查图确认。
9. 复查通过后清理 `original-watermark-backup/` 备份与 `/tmp` 复查产物。

会话语境默认用分步命令（见下），便于覆盖前后读复查图判断质量；不要临时新建处理脚本。

## 工具约定

持久环境固定在项目根 `.img-inpaint-venv/`（不要用 `/tmp`，不要删除）。`iopaint` 必须带子命令（`list`/`run`），直接运行会报 `Missing command`。环境缺失时先跑 `./tools/ensure-inpaint-env.sh`。

```bash
# 分步流程（会话默认）：prepare → inpaint → review-lama → overwrite-review → cleanup
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py prepare [文件列表]
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py inpaint [文件列表]          # 默认逐图择优：复杂纹理 scale≈1.0 走 stamp 逆解，其余 MAT；残留自动重试 1 轮
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py --no-inverse inpaint [文件列表]  # 强制全 MAT
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py --no-retry inpaint [文件列表]    # 关闭残留自动重试
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py review-lama [文件列表]        # 候选复查图 + 逐图打印 PASS/WARN/FAIL
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py overwrite-review [文件列表]   # FAIL 拒绝覆盖
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py --force overwrite-review [文件列表]  # 人工确认后强制覆盖
.img-inpaint-venv/bin/python tools/remove_doubao_watermark.py cleanup [文件列表]

# 一键（用户自助或明确"一键快速处理"时）
./tools/remove_doubao_watermark.py run [文件列表]              # 失败/需人工看复查图可加 --keep-work
./tools/remove_doubao_watermark.py --root /path/to/images run  # 处理项目外目录
```

- 不传文件列表 = 根目录全部 `*.png`。
- 其它水印用 `--mask-box x1,y1,x2,y2`（负数表示相对右下；argparse 负数需 `=` 传参，分号可多框）。
- 首次使用/环境损坏：`./tools/ensure-inpaint-env.sh`。

## 双端同步状态

Python（终端脚本）与 Rust（`desktop/`）口径**暂不一致**，改动前先看此表；Rust 细节见 `desktop/AGENTS.md`。

| 能力 | Python | Rust/App |
|---|---|---|
| 模板笔画 mask | 连续 α（α>0.03+1px） | 仍二值 + 19x11 膨胀 |
| 顶帽模板判据（阈值 20） | ✓ | ✗ |
| stamp 逆解 `--inverse` | ✓（默认开，逐图择优） | ✗ |
| 结果级验证闭环 + 残留自动重试 | ✓ | ✗ |
| `--any-position` / DBNet | ✓ | ✓（macOS，v0.5.4） |
| 修复模型 | 默认 MAT | LaMa ONNX（MAT 待确认后同步） |

`tools/compare_pipelines.py` 会因 mask 口径不同产生差异，须用 `--model lama` 对齐双端。

## 遮罩规则

遮罩按当前图片尺寸生成，不可假设尺寸相同。

1. **自动检测（默认）**：全图白字检测 + 右下角兜底（多阈值 248→150 融合 + 顶帽 31x31 低饱和过滤 + 文字性验证），检出框再过**字符行判据**（字符高度统一、行内 ≥4 字、高度比 ≤1.8）防过度修复。血泪教训：连通域无高度上限会把沙滩亮斑与水印粘成大块（1.png 曾达 828x196，实际仅 197x47）→ LaMa 重绘 10 倍区域、擦掉原图元素。膨胀只可 5x5 一次。
2. **只保留贴右下角的检出框**（豆包水印必贴右下角）；雪景白点/白墙/栏杆等会被全图检测误检。分散水印用 `--mask-box`。
3. **检测不到 → 跳过**（空 mask 透传不进模型）；对已无水印图硬修会把画面重绘成糊块。`--mask-box` 显式指定时强制处理。
4. **模板笔画 mask（复杂场景精确修复）**：豆包水印字形固定，模板资产 `tools/doubao-wm-template.png`/`.json`；按短边比例缩放后在右下角 40px 窗口用**顶帽 gap-score** 匹配，阈值 ≥20 命中即写连续 α 笔画 mask（`tools/doubao-wm-alpha.png`，α>0.03+1px）并写 `.tpl` sidecar；分数不足回退整框检测；再检测不到则空 mask 跳过。
5. **模板命中即完成**（非 `--any-position`）。
6. **inpaint 空 source 防御**：全部图无 mask 透传时 source 为空，必须跳过 iopaint 调用（空目录报 `invalid --image` 退出 255）。
7. **`--refine`（框内笔画精分割）默认关闭、实验性、仅 Python**，多数复杂场景已证伪，勿投入。

## 其它水印规则

1. "去掉水印"默认只指豆包；"去掉其它水印"才进入本流程。
2. `detect_watermark_boxes()`（Rust+Python 同步）全图扫描白字聚类，支持任意位置/多框；防误擦判据：白像素填充率 ≤0.6 且 x 投影列段数 ≥3。非白字（深色/彩色/半透明全图）检测不到，需 `--mask-box`。
3. 手动遮罩：`--mask-box x1,y1,x2,y2`；只覆盖文字与必要边缘，不覆盖大块画面。
4. **`--any-position`（默认关）**：显式开启后用 **DBNet/PP-OCRv4 det**（`tools/models/ch_pp-ocrv4_det.onnx`，onnxruntime CPU ~0.2s）检任意位置文字水印，命中即独挑；缺失/未检出回退传统扫描。已解决真实照片彩色字、雪景白字误检；剩余不可分边界（物理零对比、极低对比、照片内真实文字）用 `--mask-box` 兜底。能力矩阵以 `tools/synthetic_watermark_test.py` 为准（当前 71/75）。检出 >6 框打印复查警告。
5. 单区超过 496px 时（>512 窗口）桌面端走 tile；候选框分数低于最高分 4% 丢弃。

## 验证闭环与回归

**结果级验证（`verify_paths`/`verify_repaired`，Python，已接入管线）**：修复后逐图客观自检，`overwrite-review`/`review-lama` 打印 `[PASS]/[WARN]/[FAIL]`，**FAIL 拒绝覆盖**（`--force` 强制）。判据：
1. **mask 外零改动**（硬性）：结果与备份原图在 mask 外差异 >2 即 FAIL，被抓到一定是 bug。
2. **模板残留**（仅模板路径，`.tpl`）：原图命中豆包模板而修复后仍命中即 FAIL/WARN。
3. **mask 面积占比 > 8%** 告警（过度重绘）。

**残留自动重试（默认开，最多 1 轮，`--no-retry` 关）**：`inpaint` 修复后判**残留 FAIL** 时，把该图 mask 膨胀一级（`RETRY_DILATE=5x5`）放进隔离子目录单独重跑并复验；仍残留则保留 FAIL 交 `overwrite-review` 拒绝落盘。仅对残留触发（mask 外改动属工程 bug，重试无意义）。

**回归语料库**：`tools/watermark_regression.py`（L1 快/无模型；`--e2e --model lama` 慢），非零退出码可接 CI。改管线后先跑 `verify` 再跑回归：

```bash
.img-inpaint-venv/bin/python tools/watermark_regression.py
.img-inpaint-venv/bin/python tools/watermark_regression.py --e2e --model lama
```

## 修复方法论（普适原则）

1. **mask 最小化第一**：修复上限由 mask 决定，只覆盖被水印真正破坏的像素（核心 + 抗锯齿 ±1–3px），膨胀从最小向上试。
2. **先消除字形信息泄漏**：mask 盖住水印边缘即可（iopaint 会把 mask 区 source 置零，粗填无效已证伪）；用连续 α mask，勿回退"二值+大膨胀"。
3. **模型匹配复杂度**：均匀背景 LaMa 够用；复杂结构（花丛/棱线/阴影）必须 MAT。
4. **量化优先于目检**：改动必须输出 changed/lost/gain + "mask 外零变化"断言 + 字形复活检测；已固化为 `verify_paths`。
5. **接受信息论极限**：被笔画完全压住的像素不可逆，生成式修复是上限；混合边界区形态差异属极限，勿继续调参，唯一杠杆是"mask 是否更精确"。像素级还原走 `--inverse`（完整 stamp 反解），不要临时反解/变换脚本。

## 备份规则与覆盖安全

- 备份目录固定 `original-watermark-backup/`；不存在才复制，不覆盖已有；最终复查通过后删除备份与 `/tmp` 复查产物。
- **覆盖安全（md5）**：prepare 记录源文件 md5 到 `WORK/manifest.json`，`overwrite-review` 覆盖前校验目标 md5，不一致直接拒绝；无 manifest 拒绝执行。
- **`--root` 不得指向 `dist/`**（只读原图源），所有写操作都不例外。

## 质量标准

1. 目标区域无水印文字残留。
2. 水印区域背景纹理自然，无明显矩形糊块。
3. 不破坏主体、边缘线条、地面/桌面/水面纹理等关键元素。
4. 最终回复前必须完成**落盘复查**，不要停在读取复查图之后。

## 提效规则

1. 修复环境固定 `.img-inpaint-venv/`，不要删除。
2. 用项目脚本，不临时新建脚本。
3. `run` 仅用户自助或明确"一键快速处理"时用，不替代人工复查。
4. 泛化指令直接处理根目录全部 `*.png`，不做筛选。
5. 同一批图只跑一次 `iopaint run`（模型只加载一次），不逐张运行。
6. 复查只做两次关键检查（候选 + 落盘），不反复生成多轮候选。
7. `/tmp/doubao-watermark-work` 是本轮中间产物，复查通过后必须清理。
8. 图片复查只读拼图，不逐张读整图（省解析与回传）。
9. CI 默认异步：推送后标注"CI 后台验证中"即结束，下次会话开头用 `api.github.com` 查结果；仅用户明确要求时轮询（间隔 ≥90s）。
10. 减少 CI 白跑：改 Rust/JS 推送前先跑 macOS `cargo check`（清 MacPorts 变量）与 `node --check ui/*.js`；改 Android 行为/Rust 再跑 `./tools/android-check.sh`。

## 用户沟通

处理中保持简洁更新；不反复展示中间候选（除非质量有疑问）。最终回复只说明：已处理的图片范围、修复环境可否复用、备份与 `/tmp` 是否已清理、是否存在残留或风险。

## 会话压缩续接

- 压缩后先以用户最新消息为准，只接续摘要中标记为 `In Progress`/`Blocked`/最后一次未完成的验证或收口动作。
- 不要把摘要里的长期 `Next Steps`、历史背景、已完成事项重新展开成新的泛化任务。
- 无法判断下一步时，先用一句话向用户确认，不要自行发散扫描或改造。

# docs/non-doubao-watermarks.md — 其它水印与档案库（按需加载）

非豆包水印（任意位置/多框）与"水印档案库"细节；默认豆包流程见根 `AGENTS.md`。下文 `$S` = `tools/remove_doubao_watermark.py`（`$P` = `.img-inpaint-venv/bin/python`）。

## 其它水印规则
1. 默认"去水印"仅豆包；"其它水印"进本流程。
2. `detect_watermark_boxes()`（Python/Rust 同步）全图扫白字聚类，任意位置/多框；防误擦：白像素填充率 ≤0.6 且 x 投影列段数 ≥3。非白字（深/彩/半透明）检不到，用 `--mask-box`。
3. 手动遮罩只覆盖文字与必要边缘，不覆盖大块画面。
4. **`--any-position`（默认关）**：开启用 **DBNet/PP-OCRv4 det**（`tools/models/ch_pp-ocrv4_det.onnx`，CPU ~0.2s）检任意位置文字，命中即独挑；缺失/未检出回退传统扫描。能力矩阵 `tools/synthetic_watermark_test.py`（71/75）。检出 >6 框打印警告。
5. 单区 >496px（>512 窗口）桌面端走 tile；候选框分数低于最高 4% 丢弃。

## 水印档案库（自动识别 + 逆解，Python）
目的：任意 AI 工具半透明水印，去除且不影响周边元素。生成式（MAT/LaMa）会重绘水印处背景；**唯一能恢复真实背景的是按混合模型逆解** `bg=(obs−α·C)/(1−α)`，前提已知 α（不透明度）与 C（颜色）。

- 模块 `tools/watermark_profiles.py`，档案 `tools/watermarks/<id>/{alpha.png,color.png,meta.json}`。
- **流程**：`prepare` 中档案匹配**优先于**豆包模板；命中即用 α 图作精确 mask 并写 `.wprof` sidecar，`inpaint` 据此逆解覆盖 MAT。未命中 → 原生成式流程不变。`--no-profile` 关。
- **建档案（每工具一次）**：
  - **黑白对（最精确，带描边/彩字首选）**：`$S learn-pair <黑底> <白底> --label <名> --ref-short <短边>`。同款水印、同分辨率、一近黑一近白即可**逐像素分离 α 与 C**（`a=1-(obs_w-obs_b)/(bg_w-bg_b)`，`C=(obs_b-(1-a)bg_b)/a`），暗描边一并解出。样张须由该工具自己出图，存 `samples/`。
  - **自动挑帧（无纯色底次选）**：`$S learn-auto --label <名> <同款图...>`。自动定位并按"背景均匀度+对比度"挑最干净帧解 α（近黑/纯色最佳；**纯白底对比度不足被拒**）。单色解不出 C，带描边会残留 → 门控回退生成式。
  - 纯色样张：`learn-solid <黑底> --box x1,y1,x2,y2 --bg 0,0,0 --ref-short <短边> --label <名>`（单张无法分离 α/C，默认按白字解 α；非白需多图）。
  - 批量：`learn-batch <多张同尺寸同位置图> --label <名>`（逐像素回归；**±0.1~0.2，实验性**，需看复查图）。
  - **前提**：样张须与目标**同分辨率档**；跨档（1248 样张套 1760）即使等比重采样也残留描边。找不到同档纯色底时 `build_from_uniform` 写 `place` 锚点供 `place_by_anchor` 无匹配定位。
  - **尺寸容错**：NCC 定位**粗→细两级**，基准比例 **±35%**（`NCC_SCALE_SPAN`）内搜尺度；不纯等比时逆解门控（ghost/model-fit）回退生成式（mask 对准、周边零改动）。合成验证：0.70–1.35× 尺度均准确定位（dpos 0，~0.8s/图）。
- 查看：`$S profiles` 或 `watermark_profiles.py list`。
- 阈值：档案匹配 12（豆包模板 20）；档案逆解 ghost 门控 0.6（豆包 stamp 0.12）。
- 精度参考（合成黑白图）：水印区误差 80→3，mask 外零改动。**没有档案也无纯色样张时无法自动完美**：单张任意图的 α 数学上不可解，只能生成式（水印处背景为重建）。

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
5. 只给右下角水印文字区域生成遮罩，保留少量边距，不要大范围涂抹。
6. 优先使用 LaMa / `iopaint` 批量修复。
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

`--mask-box` 支持绝对坐标，也支持负数表示相对右下边界，例如 `-330,-118,-8,-8`。

底层批量修复命令格式：

```bash
.img-inpaint-venv/bin/iopaint run --model lama --device mps --image /tmp/doubao-watermark-work/source --mask /tmp/doubao-watermark-work/masks --output /tmp/doubao-watermark-work/lama
```

## 桌面端

项目提供 Tauri 桌面应用（`desktop/`），供 macOS / Windows 用户自助使用，复用同一流水线脚本：

- 开发调试：`cd desktop && pnpm install && pnpm tauri dev`
- 构建：`cd desktop && pnpm tauri build`
- 脚本已支持 `--root <文件夹>`，桌面端用它在用户选择的任意文件夹上执行处理；CLI 不传 `--root` 时默认仍是项目根目录，会话流程不受影响
- Windows 安装包无法在 macOS 上交叉编译，用 `.github/workflows/desktop-build.yml` 在 CI 构建
- 当前里程碑：安装包不捆绑 `.img-inpaint-venv/`（体积大），目标机器用应用内“一键初始化修复环境”按钮或初始化脚本在线拉取依赖
- 一键初始化：Rust 端 `setup_env` 命令执行 `tools/ensure-inpaint-env.sh`（macOS/Linux）或 `tools/ensure-inpaint-env.ps1`（Windows），pip 默认走阿里云 PyPI 镜像（`PIP_INDEX_URL` 可覆盖），自动断点续传下载 LaMa 模型（`LAMA_MODEL_URL` 可覆盖），日志实时回传到界面
- 不要建议在目标机器上执行 `pnpm tauri build` 作为安装方式：需要完整 Node/Rust/Xcode/VS 工具链，编译慢且易失败，且省不掉运行时的 Python/PyTorch/模型；小体积分发的正确形态是小安装包 + 首次运行在线初始化，长期方向是 ONNX 内核（免 Python）
- macOS 构建必须用 `desktop/build.sh`（自动 unset `~/.zshrc` 里的旧 MacPorts 编译变量，否则构建失败）

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

## 遮罩规则

遮罩必须按当前图片尺寸生成，不能假设所有任务尺寸相同。

对 `2848x1600` 图片，水印通常在右下角，可用近似区域：

```text
x: width - 330 到 width - 8
y: height - 118 到 height - 8
```

对 `2278x1280` 图片，水印通常在右下角，可用近似区域：

```text
x: width - 275 到 width - 7
y: height - 92 到 height - 8
```

实际处理前必须用右下角裁剪图确认水印没有超出遮罩。如果水印位置、大小或图片尺寸不同，应按复查图调整遮罩。

## 其它水印规则

1. “去掉水印”默认只指“豆包AI生成”，不要误改为其它水印流程。
2. “去掉其它水印”才进入其它水印流程；如果用户同时给出水印文字或截图，按用户指定目标处理。
3. 其它水印可能位于左上、右上、居中、底部横条或多处重复区域，不能套用固定右下角遮罩。
4. 使用 `tools/remove_doubao_watermark.py --mask-box x1,y1,x2,y2 prepare ...` 生成自定义遮罩；遮罩必须只覆盖水印文字和必要边缘，不要覆盖大块画面。
5. 如果一张图有多个分散水印，优先分批处理不同区域，或扩展脚本支持多遮罩框后再处理；不要一次用超大矩形覆盖多个区域。
6. 最终质量标准仍相同：无目标水印残留，无明显糊块，不破坏主体和关键纹理。

## 质量标准

最终结果必须满足：

1. 目标区域看不到“豆包AI生成”或本次指定的其它水印文字残留。
2. 水印区域背景纹理自然，没有明显矩形糊块。
3. 不破坏画面主体、边缘线条、地面纹理、桌面纹理、水面纹理等关键视觉元素。
4. 最终回复前必须完成落盘复查，不要停在读取复查图之后。

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

## 用户沟通

处理过程中保持简洁更新。不要反复展示多个中间候选，除非质量有疑问。最终回复只说明已处理的图片范围、项目内修复环境是否可复用、备份和 `/tmp` 复查产物是否已清理，以及是否存在残留或风险。

## 会话压缩续接

- 压缩历史消息后，先以用户最新消息为准，再只接续摘要中明确标记为 `In Progress`、`Blocked` 或最后一次未完成的验证/收口动作。
- 不要把摘要里的长期候选 `Next Steps`、历史背景、已完成事项重新展开成新的泛化任务队列。
- 如果压缩摘要无法判断下一步，应先用一句话向用户确认，不要自行发散扫描或改造。

#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from shutil import copyfile, rmtree


ROOT = Path(__file__).resolve().parents[1]
VENV = ROOT / '.img-inpaint-venv'
IS_WINDOWS = os.name == 'nt'
if IS_WINDOWS:
    VENV_PYTHON = VENV / 'Scripts' / 'python.exe'
else:
    VENV_PYTHON = VENV / 'bin' / 'python'
if VENV_PYTHON.exists() and Path(sys.prefix).resolve() != VENV.resolve():
    argv = [str(VENV_PYTHON), str(Path(__file__).resolve()), *sys.argv[1:]]
    if IS_WINDOWS:
        sys.exit(subprocess.call(argv))
    os.execv(str(VENV_PYTHON), argv)

try:
    from PIL import Image, ImageDraw, ImageFont
except ModuleNotFoundError as exc:
    if exc.name == 'PIL':
        raise SystemExit('missing Pillow; run ./tools/ensure-inpaint-env.sh first') from exc
    raise


if IS_WINDOWS:
    DEFAULT_WORKDIR = Path(tempfile.gettempdir()) / 'doubao-watermark-work'
else:
    DEFAULT_WORKDIR = Path('/tmp/doubao-watermark-work')
WORK = Path(os.environ.get('DOUBAO_WATERMARK_WORKDIR', str(DEFAULT_WORKDIR)))
SOURCE = WORK / 'source'
MASKS = WORK / 'masks'
LAMA = WORK / 'lama'
REVIEW = WORK / 'review'
DEFAULT_ROOT = ROOT
if IS_WINDOWS:
    IOPAINT = VENV / 'Scripts' / 'iopaint.exe'
else:
    IOPAINT = VENV / 'bin' / 'iopaint'
DEVICE = 'mps' if sys.platform == 'darwin' else 'cpu'

TEMPLATE_ASSET = Path(__file__).resolve().parent / 'doubao-wm-template.png'
TEMPLATE_ALPHA_ASSET = Path(__file__).resolve().parent / 'doubao-wm-alpha.png'
TEMPLATE_META = TEMPLATE_ASSET.with_suffix('.json')
# 顶帽 gap-score（笔画区均亮 - 间隙区均亮）实测：黑底 145 / 雪景 59 / 花墙 53 /
# 沙滩 96 / 纸面 33；已去水印图（负样本）≤8.4。阈值 20 取中间空档：3.png 纸面
# 低对比水印并入模板路径走笔画级 α mask，避免回退整框重绘抹平纸面折痕。
TEMPLATE_MIN_SCORE = 20.0
# 顶帽口径下限（**防"已去水印图被再误检"**，2026-09）：raw 口径把"水印处整体偏亮"
# 也计入 gap，强纹理背景（花丛/地毯）即使在**无水印**处也能凑出高分——已去水印的
# 1.png/6.png raw 27.3/34.6（≥20）被再检出，重跑会把成品再修坏。顶帽口径（局部背景
# 扣除）只有"字形相对**紧邻**背景更亮"才给分：真水印顶帽 ≥19.9（最弱 12.png），
# 误检仅 ≤6.4（1.png 6.4、6.png 5.7）。故在 raw 阈值之外**另要求顶帽过线**，
# 取两者之间空档 15。实测全 23 张原图真水印顶帽均 ≥19.9、全部通过。
TEMPLATE_TOPHAT_MIN = 15.0
# 相对残留判据（触发自动重试）：模板笔画 α mask 只覆盖亮字核心，盖不住豆包水印的
# 暗色描边——低对比背景（4.png 纸面）上 MAT 只重绘笔画区，暗描边残留成字形凹痕。
# 修复后模板分本应大幅下降（高对比图 120→<5，去除率 >95%）；残影图的残留占比很高
# （4.png 43.2→10.7，约 25%），故用相对下降比例而非绝对分兜底：残留 ≥ 原分 20% 且
# ≥ 8（高于已去水印负样本 ≤8.4 的底噪，避免干净图误触发）时重试。绝对阈值 20 只
# 用于 FAIL 判定，低对比残影达不到 20 会被放过（本 bug 的根因）。
TEMPLATE_RESIDUAL_RATIO = 0.2
TEMPLATE_RESIDUAL_FLOOR = 8.0
# 残留判据用"源图水印锚点处的分"而非"角窗最大分"：真实残留必然在锚点处最强；而沙地/
# 花墙等强纹理背景会在角窗别处凑出高分（实测 1.png 锚点 11.0 而窗最大 27.3、6.png
# 锚点 24.6 而窗最大 34.6 → 旧口径把已去干净的两张误判 FAIL、整批拒绝）。要求锚点分
# ≥ 0.85×窗最大（"水印处即主导峰"）才判残留/FAIL，避免背景巧合误报。
TEMPLATE_RESIDUAL_DOMINANCE = 0.85
# 连续 α mask：水印真正污染的像素是 α>0（含抗锯齿带），二值模板（α>0.5）只覆盖
# 笔画核心，只能靠大膨胀补抗锯齿，代价是多盖 ~30% 干净画面被模型重绘（"影响周边
# 元素"的根因）。改用从黑底 2.png 提取的连续 α 图（tools/doubao-wm-alpha.png，
# 已扣底噪 18）：mask = α>0.03 的像素 + 1px 缓冲，只覆盖真正被污染的像素。α 图
# 已完整盖住抗锯齿，字符间隙无字形上下文，MAT/LaMa 实测均无字形复活（6.png 花丛、
# 1.png 沙粒保留明显多于二值+7x7；改动面积 -10%~-30%）。
TEMPLATE_ALPHA_THRESHOLD = 8  # 0..255，约 α>0.03
TEMPLATE_STROKE_DILATE = (3, 3)  # ±1px，仅补偿缩放/对齐误差
# 模板 mask 的来源优先级：stamp 完整 footprint（含暗色描边/抗锯齿，α>0.03）> 模板亮字
# α（仅亮字核心）> 二值模板。只盖亮字的 mask 漏掉水印暗色描边——低对比背景（2.png
# 木纹、4.png 纸面）上 MAT 重绘后描边残留成暗字形（模板分消不掉，因为判据只看"亮于
# 背景"的 gap）。stamp α 从多样张联立标定，覆盖整条字形+描边，用它做 mask 后模板分
# 直接归零。OPEN 清掉标定背景残差产生的孤立低 α 点，避免干净背景被点状重绘。
STAMP_MASK_THRESHOLD = 8  # 0..255，约 α>0.03
STAMP_MASK_OPEN = (3, 3)
# refine（--refine 实验性框内笔画精分割）仍用较大核连接笔画碎片
REFINE_DILATE_MAT = (7, 7)
REFINE_DILATE_LAMA = (19, 11)
# 逆解 stamp（**默认关**，--inverse 开；历史教训见下）：完整水印模型 obs = α·C + (1−α)·bg，α 为覆盖度、
# C 为逐像素颜色（含暗色描边——纯白字模型反解不掉它）。从 4 张同款水印、不同背景
# 的图（黑底/纸面/沙滩/花墙）联立标定。逆解恢复的是**真实背景**（非生成），对复杂
# 纹理背景（花丛类）明显优于 MAT 的平滑重绘；但对低纹理背景（暗底/沙面/纸面）会
# 放大噪声、对 scale≠1.0 会因 stamp 缩放插值失配而留字形。故不再用预判 gating 一刀切，
# 而是**统一尝试逆解 + 结果择优**：gating 只保留极端尺度上限与失解（ghost）回退，是否
# 采用交由结果层指标决定（见 INVERSE_RESIDUAL_TOLERANCE）。
STAMP_ASSET = Path(__file__).resolve().parent / 'doubao-wm-stamp.npz'
STAMP_REF_SHORT = 1600.0
INVERSE_SCALE_TOL = 0.15
INVERSE_TEXTURE_MIN = 0.0  # 不再作硬门槛（保留计算供日志）；由结果择优决定采用
INVERSE_ALPHA_GAIN_RANGE = (0.8, 1.25)
# 逐图自标定（关键，2026-09 根治"3.png 周边元素被影响"）：stamp 的 α/C 是跨图标定值，
# 单图实际墨色 C_true 会有偏差 → 逆解残差 ∝ [α/(1−α)]·(C_true−C_est)，在 α 高处被放大成
# **颜色鬼影**（3.png 实测金色字形：gain=1.10 时蓝通道欠还原 13 级）。旧做法用"高频残影
# 与 α 的相关"选 gain——目标错误、且对低频颜色鬼影不敏感，治标不治本。改为以 MAT 结果的
# 低频（生成式、无鬼影）为参考，对每个 gain 用最小二乘拟合每通道墨色偏移 ΔC
# （inv−MAT_low ≈ [α/(1−α)]·ΔC），取低频失配最小的 gain。
# 等价于把墨色校正为 C+ΔC——是**物理量（墨色）的标定**，不是事后抹平，故能同时消除鬼影
# 与保住真实纹理（3.png 标定后模板分 1.82 ≤ MAT 1.44×1.75，被择优选为逆解）。
INVERSE_ALPHA_GAIN_STEP = 0.02
INVERSE_CALIB_SIGMA = 6.0
INVERSE_MAX_DC = 48.0
# 择优放宽 + 字形残影闸门（2026-09，"3.png 周边元素被影响"收口）：gap-score 只测
# "笔画区−间隙区"亮度差，对**保纹理的逆解不公平**——真实高频纹理本身会抬高该分
# （3.png 标定逆解 1.9 > MAT 1.0，纯按 ×1.75 会被误拒、退回抹平纹理的 MAT）。故允许
# 逆解在 MAT 分之上再高 INVERSE_RESIDUAL_SLACK；但必须同时过"无正字形残影"闸门：
# 低频 (逆解−MAT) 与字形掩码的相关，**正值=水印残留**（字形处偏亮，2.png 逆解 0.396，
# 且其暗底噪声团肉眼可见）判负，负值=真实纹理（3.png −0.199、1.png −0.798）无害。
INVERSE_RESIDUAL_SLACK = 2.0
INVERSE_GHOST_MAX = 0.25
# 逆解色度去噪：只替换"**色度离群但亮度正常**"的孤立彩点（色度差 > 阈值，0..255）。
# 逆解 (obs−αC)/(1−α) 把噪声放大 ~2 倍，mask 内会冒出橙/蓝彩点；但**全域**色度中值会
# 一并抹掉真实色度细节（回归实测 core err 0.96→11.26）。故只对离群点动刀，并用亮度
# 判据把"色度离群但亮度也离群的正常噪声点"排除在外。
INVERSE_CHROMA_OUTLIER = 25
INVERSE_CHROMA_LUMA_TOL = 12
# 逆解**过冲**（负字形残影）闸门：结果图在 stamp 笔画区的低频亮度相对间隙区不得低于
# −INVERSE_OVERSHOOT_MAX。逆解按 stamp α 减去水印贡献，若实际水印弱于资产/背景更亮就会
# 减过头 → 笔画处变暗，肉眼即"暗色字形印记"（1.png 地毯案例：字形区比间隙暗 22，而
# ghost 判据只看正值、过冲为负故漏网）。
INVERSE_OVERSHOOT_MAX = 10.0
# 结果择优（逆解 vs MAT）：逆解是精确恢复、保留真实纹理，但只在模型适用的图（scale≈1.0
# 且 stamp 匹配好）上可靠；对 scale≠1.0（4/5.png 1728 竖图，scale=1.08）会因 stamp
# 缩放插值失配产生字形残留（实测模板分 29/34），对平滑背景会放大噪声。故不再只靠预
# 判 gating，而是**同时产出逆解与 MAT，按结果层指标择优**：逆解模板残留分必须
# < TEMPLATE_MIN_SCORE(20) 且 ≤ MAT 残留分 × RATIO(1.75) 才采用逆解，否则保留 MAT。
# 实测：6.png 12.7 ≤ 8.6×1.75=15.1 → 逆解（保橙墙黑斑）；3.png 3.0 > 1.2×1.75 → MAT；
# 4/5.png 29/34 ≥ 20 → MAT。这样"能逆解的都逆解、逆解会退步的自动落回 MAT"。
INVERSE_RESIDUAL_TOLERANCE = 1.75


try:
    import watermark_profiles as wprof
except Exception:  # pragma: no cover - optional profile library
    wprof = None


def match_profile(backup, any_position=False):
    """Match stored watermark profiles against an image; None when unavailable."""
    if wprof is None:
        return None
    try:
        with Image.open(backup) as probe:
            return wprof.match_image(probe, any_position=any_position)
    except SystemExit:
        return None


def load_template():
    """加载笔画级水印模板（黑底图提取），返回 (tpl_bool, alpha_float, meta)；
    资产缺失时 alpha 为 None（调用方回退二值模板膨胀）。"""
    if not TEMPLATE_ASSET.exists() or not TEMPLATE_META.exists():
        return None, None, None
    import json

    import numpy as np

    tpl = np.array(Image.open(TEMPLATE_ASSET).convert('L')) > 127
    alpha = None
    if TEMPLATE_ALPHA_ASSET.exists():
        alpha = np.array(Image.open(TEMPLATE_ALPHA_ASSET).convert('L')).astype(np.float32) / 255.0
    meta = json.loads(TEMPLATE_META.read_text())
    return tpl, alpha, meta


def _template_response(gray, width, height, with_tophat=False):
    """模板 gap-score 响应图 + 模板尺寸 (th, tw)；模板缺失/大于图时返回 None。
    两种预处理逐像素取更强者（峰值都落在水印处，命中位置不变）：
      - 顶帽（局部背景扣除）抑制纹理/光照梯度，救"亮背景压平笔画"（3.png raw 15 → 顶帽 33）；
      - 原始 max(RGB) 保留绝对亮度差，救"背景亮块尺度 > 顶帽核"（2.png 顶帽 19.9、raw 22.3）。
    分离度实测：正样本（原图）max ≥22.3，负样本（已去水印）≤9.3，阈值 20 干净。
    抽出来供"定位"（template_stroke_mask 取角窗峰）与"定点打分"（verify_paths 在源图
    锚点处量结果，见 TEMPLATE_RESIDUAL_DOMINANCE）共用，保证两处口径完全一致。"""
    import cv2
    import numpy as np
    tpl, _alpha, meta = load_template()
    if tpl is None:
        return None
    scale = min(height, width) / meta['ref_short_side']
    t = cv2.resize(tpl.astype(np.float32), (0, 0), fx=scale, fy=scale,
                   interpolation=cv2.INTER_NEAREST)
    th_t, tw_t = t.shape
    if th_t >= height or tw_t >= width:
        return None
    k = min(31, max(3, (min(height, width) - 1) | 1))
    tophat = gray - cv2.morphologyEx(
        gray, cv2.MORPH_OPEN, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (k, k)))
    t01 = (t > 0.5).astype(np.float32)
    ones = np.ones(t.shape, np.float32)
    n_in = float(t01.sum())
    n_out = float(t01.size) - n_in

    def _gap_score(src):
        s_in = cv2.matchTemplate(src, t01, cv2.TM_CCORR)
        s_all = cv2.matchTemplate(src, ones, cv2.TM_CCORR)
        return s_in / n_in - (s_all - s_in) / n_out

    raw = _gap_score(gray)
    top = _gap_score(tophat)
    combined = np.maximum(raw, top)
    if with_tophat:
        return combined, th_t, tw_t, top
    return combined, th_t, tw_t


def template_stroke_mask(gray, width, height, model='mat'):
    """在右下角窗口内用模板做 gap-score 匹配（0/1 模板核取 S_in、全 1 核取窗口和，
    gap = S_in/N_in − S_out/N_out），返回 (mask_uint8, score, info)。
    模板按图片短边比例缩放（豆包水印随短边等比）。mask 优先用连续 α 图
    （α>0.03 的污染像素 + 1px 缓冲，最小侵入）；α 资产缺失时回退二值模板膨胀。
    分数低于阈值返回 (None, score, info) 交给整框检测回退。"""
    try:
        import cv2
        import numpy as np
    except ImportError:
        raise SystemExit('missing cv2/numpy; run ./tools/ensure-inpaint-env.sh first')
    tpl, alpha, meta = load_template()
    if tpl is None:
        return None, 0.0, 'template asset missing'
    resp = _template_response(gray, width, height, with_tophat=True)
    if resp is None:
        return None, 0.0, 'template larger than image'
    response, th_t, tw_t, tophat_resp = resp
    # 水印必贴右下角：只在右下角 40px 余量窗口内取峰
    y0 = max(0, response.shape[0] - 41)
    x0 = max(0, response.shape[1] - 41)
    window = response[y0:, x0:]
    _, _, _, peak = cv2.minMaxLoc(window)
    px, py = peak[0] + x0, peak[1] + y0
    score = float(response[py, px])
    tophat_score = float(tophat_resp[py, px])
    if score < TEMPLATE_MIN_SCORE:
        return None, score, f'template score {score:.1f} < {TEMPLATE_MIN_SCORE}'
    if tophat_score < TEMPLATE_TOPHAT_MIN:
        # 顶帽过不了线＝"水印处整体偏亮"而非"字形相对**紧邻**背景更亮" → 纹理误检。
        # 上报的 score 用（过不了线的）顶帽分，使调用方（prepare/回归/looks_processed）
        # 一律按"未命中"处理——避免把已去水印的强纹理图再检出、重跑把成品修坏。
        return None, tophat_score, (f'template tophat {tophat_score:.1f} < {TEMPLATE_TOPHAT_MIN}'
                                    f' (raw score {score:.1f}) — texture false positive, skipped')
    stamp_a, _ = _load_stamp()
    if stamp_a is not None:
        # stamp 完整 footprint（含暗色描边）：只盖亮字的 mask 会在低对比背景留暗字形
        sa = cv2.resize(stamp_a, (tw_t, th_t), interpolation=cv2.INTER_LINEAR)
        core = (sa * 255.0 > STAMP_MASK_THRESHOLD).astype(np.uint8)
        core = cv2.morphologyEx(
            core, cv2.MORPH_OPEN,
            cv2.getStructuringElement(cv2.MORPH_ELLIPSE, STAMP_MASK_OPEN))
        how = 'stamp footprint'
    elif alpha is not None:
        a = cv2.resize(alpha, (tw_t, th_t), interpolation=cv2.INTER_LINEAR)
        core = (a * 255.0 > TEMPLATE_ALPHA_THRESHOLD).astype(np.uint8)
        how = 'alpha'
    else:
        t = cv2.resize(tpl.astype(np.float32), (tw_t, th_t), interpolation=cv2.INTER_NEAREST)
        core = (t > 0.5).astype(np.uint8)
        how = 'binary fallback'
    mask = cv2.dilate(core,
                      cv2.getStructuringElement(cv2.MORPH_RECT, TEMPLATE_STROKE_DILATE), 1)
    full = np.zeros((height, width), np.uint8)
    full[py:py + th_t, px:px + tw_t] = mask * 255
    return full, score, f'template matched at ({px},{py}) score {score:.1f} ({how})'


def _result_template_score(image):
    """对修复结果（RGB ndarray 或文件路径）算豆包模板 gap-score，供逆解/MAT 结果择优。"""
    import numpy as np

    if isinstance(image, np.ndarray):
        gray = image[..., :3].max(axis=2).astype(np.float32)
        height, width = gray.shape
    else:
        with Image.open(image) as im:
            rgb = im.convert('RGB')
            width, height = rgb.size
            gray = np.array(rgb).max(axis=2).astype(np.float32)
    _, score, _ = template_stroke_mask(gray, width, height)
    return float(score)


def _result_ghost_score(out, mat_path, px, py):
    """逆解相对 MAT 的**低频字形残影**指标：corr(blur(逆解−MAT), 字形掩码)。
    正值 = 水印残留（字形处偏亮，须拒绝）；负值 = 真实纹理（字形处偏暗，无害）。
    用于择优时把"保纹理但含残影"的逆解与"真纹理"的逆解区分开（见 INVERSE_GHOST_MAX）。"""
    import cv2
    import numpy as np

    alpha0, _ = _load_stamp()
    if alpha0 is None:
        return None
    h, w = out.shape[:2]
    scale = min(h, w) / STAMP_REF_SHORT
    tw = int(round(alpha0.shape[1] * scale))
    th = int(round(alpha0.shape[0] * scale))
    if py + th > h or px + tw > w:
        return None
    a0 = cv2.resize(alpha0, (tw, th), interpolation=cv2.INTER_LINEAR)
    with Image.open(mat_path) as im:
        mat = np.array(im.convert('RGB')).astype(np.float32)
    o = out[py:py + th, px:px + tw].astype(np.float32)
    m = mat[py:py + th, px:px + tw]
    glyph = np.repeat((a0 > 0.1).astype(np.float32)[..., None], 3, axis=2)
    r = cv2.GaussianBlur(o, (0, 0), INVERSE_CALIB_SIGMA) - cv2.GaussianBlur(m, (0, 0), INVERSE_CALIB_SIGMA)
    r = r - r.mean()
    glyph = glyph - glyph.mean()
    denom = float(np.linalg.norm(r) * np.linalg.norm(glyph))
    return float((r * glyph).sum() / denom) if denom > 1e-6 else 0.0


def _result_overshoot_score(out, px, py):
    """逆解**过冲**指标：结果图在 stamp 笔画区与间隙区的**低频亮度差**（负值=过冲）。
    与 `_result_ghost_score`（看 逆解−MAT，负值可能只是真实纹理）不同，这里只看结果自身：
    笔画处若明显比背景暗，说明水印被减过头，会留下肉眼可见的暗字形（见 INVERSE_OVERSHOOT_MAX）。"""
    import cv2
    import numpy as np

    alpha0, _ = _load_stamp()
    if alpha0 is None:
        return None
    h, w = out.shape[:2]
    scale = min(h, w) / STAMP_REF_SHORT
    tw = int(round(alpha0.shape[1] * scale))
    th = int(round(alpha0.shape[0] * scale))
    if py + th > h or px + tw > w:
        return None
    a0 = cv2.resize(alpha0, (tw, th), interpolation=cv2.INTER_LINEAR)
    core = a0 > 0.5
    gap = a0 < 0.02
    if not core.any() or not gap.any():
        return None
    gray = out[py:py + th, px:px + tw].astype(np.float32).max(axis=2)
    blur = cv2.GaussianBlur(gray, (0, 0), INVERSE_CALIB_SIGMA)
    return float(blur[core].mean() - blur[gap].mean())


def inverse_apply(obs_path, mat_path, model='mat'):
    """对模板命中的图做完整 stamp 逆解（obs = α·C + (1−α)·bg），逐图标定 gain 与墨色 C，
    返回处理后整图；不满足尺度上限或 stamp 无法定位时返回 None（保持 MAT）。
    标定：以 MAT 结果低频为"无鬼影"参考，最小二乘拟合每通道墨色偏移 ΔC，取低频失配最小
    的 gain（见 INVERSE_ALPHA_GAIN_* 常量注释）——消除颜色失配鬼影的同时保住真实纹理。
    失解（raw 超出 [0,255] 或全通道饱和）像素回退 MAT 结果。"""
    import cv2
    import numpy as np

    alpha0, color0 = _load_stamp()
    if alpha0 is None:
        return None
    with Image.open(obs_path) as probe:
        obs = np.array(probe.convert('RGB')).astype(np.float32)
    height, width = obs.shape[:2]
    short = min(height, width)
    if abs(short / STAMP_REF_SHORT - 1.0) > INVERSE_SCALE_TOL:
        return None
    gray = obs.max(axis=2)
    mask, score, info = template_stroke_mask(gray, width, height, model)
    if mask is None:
        return None
    mm = re.search(r'at \((\d+),(\d+)\)', info)
    if not mm:
        return None
    px, py = int(mm.group(1)), int(mm.group(2))
    scale = short / STAMP_REF_SHORT
    th = int(round(alpha0.shape[0] * scale))
    tw = int(round(alpha0.shape[1] * scale))
    if py + th > height or px + tw > width:
        return None
    # 水印邻域纹理复杂度（仅日志：gating 不再据此预判，是否采用交给结果择优）
    y0, x0 = max(0, py - 40), max(0, px - 60)
    y1, x1 = min(height, py + th + 40), min(width, px + tw + 60)
    reg = obs[y0:y1, x0:x1]
    hf = float(np.abs(reg - cv2.GaussianBlur(reg, (0, 0), 2.0)).mean())
    if hf < INVERSE_TEXTURE_MIN:
        return None
    a0 = cv2.resize(alpha0, (tw, th), interpolation=cv2.INTER_LINEAR)
    color = cv2.resize(color0, (tw, th), interpolation=cv2.INTER_LINEAR)
    win = obs[py:py + th, px:px + tw]
    mat = np.array(Image.open(mat_path).convert('RGB')).astype(np.float32)[py:py + th, px:px + tw]
    # 标定参考：MAT 结果的低频。MAT 是生成式的、无字形鬼影，其低频可作"背景真值"的估计。
    mat_low = cv2.GaussianBlur(mat, (0, 0), INVERSE_CALIB_SIGMA)
    best = None
    lo, hi = INVERSE_ALPHA_GAIN_RANGE
    for k in np.arange(lo, hi + 1e-6, INVERSE_ALPHA_GAIN_STEP):
        a = np.clip(a0 * k, 0, 1)
        a3 = a[..., None]
        inv = np.clip((win - a3 * color) / np.maximum(1 - a3, 1e-3), 0, 255)
        m = a > 0.05
        if int(m.sum()) < 50:
            continue
        # 残差 inv−MAT_low ∝ [α/(1−α)]·ΔC：对每通道 ΔC 做最小二乘，再按下标定后的
        # 低频失配挑 gain。ΔC 即"墨色标定误差"，校正它=用 C+ΔC 重算逆解。
        kf = a / np.maximum(1 - a, 1e-3)
        err = inv - mat_low
        denom = float((kf[m] ** 2).sum())
        if denom < 1e-6:
            continue
        dc = np.array([float((kf[m] * err[..., c][m]).sum()) / denom for c in range(3)])
        dc = np.clip(dc, -INVERSE_MAX_DC, INVERSE_MAX_DC)
        corr = inv - kf[..., None] * dc
        resid = float(np.abs(cv2.GaussianBlur(corr, (0, 0), INVERSE_CALIB_SIGMA) - mat_low)[m].mean())
        if best is None or resid < best[0]:
            best = (resid, corr, a, float(k))
    if best is None:
        return None
    resid, inv, a, gain = best
    # ① 只改写"声明的 mask"内（stamp α 比模板 mask 略宽，放任越界会破坏
    #    "mask 外零改动"这条场景无关的硬性保证）。
    # ② 回退 MAT 仅在【逆解失解】时：模型不适用（raw 超出 [0,255]，obs 无法由
    #    α·C+(1-α)·bg 解释）或全通道饱和。**关键教训**：旧策略"任一通道饱和即回退
    #    MAT"会让 MAT 在木纹/亮背景的饱和像素上生成深色斑点（6.png 实测黑点 artifact，
    #    原图 239→MAT 37）；改为仅在失解时回退后该黑点消失、模板残留不变（10.1→10.2）。
    a3 = a[..., None]
    raw = (win - a3 * color) / np.maximum(1 - a3, 1e-3)
    ill_posed = (((raw < -0.5) | (raw > 255.5)).any(axis=2)
                 | (win >= 252).all(axis=2))
    m = (a > 0.03) & (mask[py:py + th, px:px + tw] > 0)
    hybrid = np.where(ill_posed[..., None], mat, inv)
    out = obs.copy()
    sub = out[py:py + th, px:px + tw]
    sub[m] = hybrid[m]
    # 色度去噪（仅 mask 内）：逆解 (obs−αC)/(1−α) 把噪声放大约 1/(1−α)≈2 倍，mask 内会
    # 留下孤立**彩色**噪点（2.png 肉眼可见的橙/蓝点）。只替换"色度离群、亮度却正常"的
    # 像素：亮度通道与 mask 外一概不动，真实色度细节与正常噪声点都保留。写回只覆盖 mask
    # 内像素，保证"mask 外零改动"（cvtColor 往返的 ±1 舍入不会外泄）。
    m_full = np.zeros(out.shape[:2], bool)
    m_full[py:py + th, px:px + tw] = m
    ycc = cv2.cvtColor(out.astype(np.uint8), cv2.COLOR_RGB2YCrCb)
    luma_delta = np.abs(ycc[:, :, 0].astype(np.int16) - cv2.medianBlur(ycc[:, :, 0], 3).astype(np.int16))
    for ch in (1, 2):
        med = cv2.medianBlur(ycc[:, :, ch], 3)
        chroma_delta = np.abs(ycc[:, :, ch].astype(np.int16) - med.astype(np.int16))
        replace = m_full & (chroma_delta > INVERSE_CHROMA_OUTLIER) & (luma_delta < INVERSE_CHROMA_LUMA_TOL)
        ycc[:, :, ch] = np.where(replace, med, ycc[:, :, ch])
    den = cv2.cvtColor(ycc, cv2.COLOR_YCrCb2RGB).astype(np.float32)
    sub[m] = den[py:py + th, px:px + tw][m]
    out[py:py + th, px:px + tw] = sub
    return out.astype(np.uint8), (px, py, hf, gain, resid)


def _load_stamp():
    import numpy as np
    if not STAMP_ASSET.exists():
        return None, None
    data = np.load(STAMP_ASSET)
    return data['alpha'], data['color']


def backup_dir(root):
    return Path(root) / 'original-watermark-backup'


def numeric_key(path):
    return (0, int(path.stem)) if path.stem.isdigit() else (1, path.stem)


def target_names(files, root):
    root = Path(root)
    paths = [root / item for item in files] if files else sorted(root.glob('*.png'), key=numeric_key)
    names = []
    for path in paths:
        if path.suffix.lower() != '.png':
            raise SystemExit(f'not a png: {path.name}')
        if not path.exists():
            raise SystemExit(f'missing file: {path.name}')
        names.append(path.name)
    if not names:
        raise SystemExit(
            f'no png files found in: {root}\n'
            "hint: pass a folder with --root, e.g. ./tools/remove_doubao_watermark.py --root /path/to/images run"
        )
    return names


def mask_box(width, height):
    if (width, height) == (2848, 1600):
        return (width - 330, height - 118, width - 8, height - 8)
    if (width, height) == (2278, 1280):
        return (width - 275, height - 92, width - 7, height - 8)
    if (width, height) == (2048, 2048):
        return (width - 380, height - 125, width - 8, height - 40)
    scale = min(width / 2848, height / 1600)
    box_width = max(220, int(330 * scale))
    box_height = max(78, int(118 * scale))
    return (max(0, width - box_width), max(0, height - box_height), width - 8, height - 8)


def detect_watermark_boxes(image, extended=False):
    """两级检测：
    1) 全图扫纯白文字（≥248），用"文字性特征"过滤画面主体误检：组件内原始白像素
       填充率 ≤0.6 且 x 投影列段数 ≥3（实心块如灯罩 fill 0.8+、段数 1，文字水印
       fill ~0.2、段数=字符数）；
    2) 右下角自适应阈值兜底（识别半透明/灰白粗体水印），与第 1 级合并去重。
    extended=True（--any-position 时）：优先用 OCR 文字检测模型（DBNet，任意颜色/
    低对比/复杂背景泛化，雪点/花瓣不误检）；模型命中即完全独挑（传统扫描在照片
    上误检率高反而拖累），模型文件缺失或未检出时退回传统扫描（灰白多阈值/深色
    负片/彩色色度/顶帽，纯背景可靠、照片误检多需复查）。"""
    full = _detect_full_white(image)
    corner = _detect_corner_faded(image)
    found = full + [b for b in corner if not any(_overlap(b, f) for f in full)]
    if extended:
        dbnet = _detect_dbnet(image)
        if dbnet:
            return dbnet
        extra = _detect_full_faded(image) + _detect_full_color(image)
        extra += _detect_full_tophat(image) + _detect_full_dark(image)
        for b in extra:
            if not any(_overlap(b, f) for f in found):
                found.append(b)
    return found


DBNET_MODEL = ROOT / 'tools' / 'models' / 'ch_pp-ocrv4_det.onnx'
_DBNET_SESSION = None


def _detect_dbnet(image, thr=0.3, unclip=1.0):
    """OCR 文字检测（DBNet/PP-OCRv4 det）：以文字为训练目标，天然过滤雪点/花瓣/
    纹理误检，对彩色字、低对比字、复杂照片背景泛化——传统扫描确认不可分的场景
    （真实彩色照片红字、雪景白字）由它解决。框为文本行紧贴框，按字高 unclip
    外扩成遮罩框。模型缺失返回 []（调用方回退传统扫描）。"""
    try:
        import numpy as np
        import cv2
        import onnxruntime as ort
    except ImportError:
        return []
    global _DBNET_SESSION
    if not DBNET_MODEL.exists():
        return []
    if _DBNET_SESSION is None:
        _DBNET_SESSION = ort.InferenceSession(str(DBNET_MODEL), providers=['CPUExecutionProvider'])
    sess = _DBNET_SESSION
    input_name = sess.get_inputs()[0].name
    img = np.array(image.convert('RGB'))[:, :, ::-1]  # RGB→BGR
    h, w = img.shape[:2]
    ratio = 960 / max(h, w)
    rw = max(32, int(w * ratio) // 32 * 32)
    rh = max(32, int(h * ratio) // 32 * 32)
    x = cv2.resize(img, (rw, rh)).astype(np.float32) / np.float32(255)
    x = (x - np.array([0.485, 0.456, 0.406], np.float32)) / np.array([0.229, 0.224, 0.225], np.float32)
    x = x.transpose(2, 0, 1)[None].astype(np.float32)
    prob = sess.run(None, {input_name: x})[0][0, 0]
    m = (prob > thr).astype(np.uint8)
    n, _, stats, _ = cv2.connectedComponentsWithStats(m, 8)
    pad_ratio = 10
    boxes = []
    for i in range(1, n):
        x1, y1, cw, ch, area = (int(v) for v in stats[i])
        if area < 200 or cw < ch:
            continue
        pad = max(6, int(ch * unclip))
        gx1 = max(0, round(x1 / rw * w) - pad)
        gy1 = max(0, round(y1 / rh * h) - pad)
        gx2 = min(w - 1, round((x1 + cw) / rw * w) + pad)
        gy2 = min(h - 1, round((y1 + ch) / rh * h) + pad)
        if gx2 - gx1 < 30 or gy2 - gy1 < 12:
            continue
        boxes.append((gx1, gy1, gx2, gy2))
    return boxes


def _boxes_from_mask(mask, h, w):
    """从候选像素 mask 提取符合文字性特征的框（膨胀成行 → 组件过滤 → 低分裁剪）。"""
    import numpy as np
    import scipy.ndimage as ndi
    merged = mask.astype(np.uint8)
    kernel = np.ones((3, 9), dtype=np.uint8)
    for _ in range(2):
        merged = ndi.binary_dilation(merged, structure=kernel).astype(np.uint8)
    labeled, count = ndi.label(merged)
    if count == 0:
        return []
    areas = np.bincount(labeled.ravel())
    slices = ndi.find_objects(labeled)
    candidates = []
    for i, sl in enumerate(slices, start=1):
        y1, y2, x1, x2 = sl[0].start, sl[0].stop, sl[1].start, sl[1].stop
        cw, ch, area = x2 - x1, y2 - y1, int(areas[i])
        if area < 1200 or ch < h * 0.012 or ch > h * 0.09 or cw < ch:
            continue
        ratio = cw / ch
        if ratio < 2.0 or ratio > 15:
            continue
        fill = area / float(cw * ch)
        if fill < 0.2 or fill > 0.95:
            continue
        fill_raw, segments = _text_likeness(mask, x1, y1, x2, y2)
        if fill_raw > 0.6 or segments < 3:
            continue
        corner_dist = (w - x2) + (h - y2)
        score = area * min(ratio / 6.0, 1.0) / (1.0 + corner_dist / (w * 0.1))
        candidates.append(((x1, y1, x2, y2), score))
    if not candidates:
        return []
    top = max(score for _, score in candidates)
    threshold = top * 0.04
    pad = max(10, h // 150)
    boxes = []
    for (x1, y1, x2, y2), score in candidates:
        if score < threshold:
            continue
        boxes.append((
            max(0, x1 - pad),
            max(0, y1 - pad),
            min(w - 6, x2 + pad),
            min(h - 6, y2 + pad),
        ))
    return boxes


def _is_corner_box(box, image):
    _, _, x2, y2 = box
    w, h = image.size
    return x2 > w * 0.85 and y2 > h * 0.85


def _overlap(a, b):
    return not (a[2] <= b[0] or b[2] <= a[0] or a[3] <= b[1] or b[3] <= a[1])


def _text_likeness(white, x1, y1, x2, y2):
    """组件 bbox 内的原始白像素填充率与 x 投影列段数。"""
    sub = white[y1:y2, x1:x2]
    fill_raw = float(sub.mean()) if sub.size else 1.0
    proj = sub.sum(axis=0).astype(float)
    thr = max(1.0, sub.shape[0] * 0.08) if sub.size else 1.0
    segments = 0
    prev = 0
    for v in (proj >= thr).astype(int):
        if v == 1 and prev == 0:
            segments += 1
        prev = v
    return fill_raw, segments


def _detect_full_white(image):
    try:
        import numpy as np
        import scipy.ndimage as ndi
    except ImportError:
        return []
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    white = (img >= 248).all(axis=2)
    return _boxes_from_mask(white, h, w)


def _detect_full_faded(image):
    """全图灰白/半透明文字检测（--any-position opt-in）：多阈值扫描 230→160，
    每阈值独立做文字性过滤后融合去重（暗水印高阈值只能切出局部组件，
    必须靠低阈值补全——corner 兜底的同款经验推广到全图）。"""
    try:
        import numpy as np
        import scipy.ndimage as ndi
    except ImportError:
        return []
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    boxes = []
    for th in (230, 210, 190, 170, 160):
        cand = _boxes_from_mask((img >= th).all(axis=2), h, w)
        boxes.extend(cand)
    return _fuse_boxes(boxes)


def _detect_full_color(image):
    """全图彩色文字检测（--any-position opt-in）：色度分量（max-min 通道差）
    显著的区域——纯色水印叠加使色度远高于灰白/低饱和背景；抗锯齿边缘色度
    低但笔画核心密度（~20%）与文字水印相当，膨胀成行后文字性过滤可判。
    高饱和背景（红花绿叶/彩色墙面）必误检，由 opt-in 承担。"""
    try:
        import numpy as np
        import scipy.ndimage as ndi
    except ImportError:
        return []
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    sat = img.max(axis=2) - img.min(axis=2)
    return _boxes_from_mask(sat > 60, h, w)


def _detect_full_tophat(image):
    """低对比亮字顶帽检测（--any-position opt-in）：亮背景上的浅灰字亮度
    阈值不可分（如 220 字 vs 235 墙），形态学顶帽（原图-开运算）提取局部
    高亮结构。纹理背景响应同样高（纸面/织物必误检），由 opt-in 承担。"""
    try:
        import numpy as np
        import cv2
        import scipy.ndimage as ndi
    except ImportError:
        return []
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    gray = img.max(axis=2).astype(np.uint8)
    k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (31, 31))
    opened = cv2.morphologyEx(gray, cv2.MORPH_OPEN, k)
    tophat = gray.astype(np.int16) - opened.astype(np.int16)
    return _boxes_from_mask(tophat >= 12, h, w)


def _detect_full_dark(image):
    """全图深色文字检测（--any-position opt-in）：亮背景上的暗字负片扫描。
    大面积暗区（黑底/深色书本）会因组件超高（>9%H）或填充率过高被拒。"""
    try:
        import numpy as np
        import scipy.ndimage as ndi
    except ImportError:
        return []
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    dark = (img <= 40).all(axis=2)
    return _boxes_from_mask(dark, h, w)


# 字符高度（相对短边）判据：水印随短边等比缩放，字形高度稳定落在带内——
#   · 豆包 stamp：字符高 ≈ 3.5% 短边（1600 → 56~59px）
#   · 合成通用文字（回归样本）：≈ 3.4% 短边
# 而"已去水印图"右下角的背景亮斑/纹理，字符高普遍只有 1.3%~2.0% 短边
# （地毯/木纹/花丛的碎块），旧下限 1.2%*H 太低 → 兜底检测把背景当水印硬修。
# 收紧到 2.6%~5.0% 短边后，真实水印（3.4~3.7%）仍稳过，背景碎块被挡在门外。
CORNER_CHAR_H_MIN_REL = 0.026
CORNER_CHAR_H_MAX_REL = 0.050
CORNER_ROW_H_MIN_REL = 0.026
CORNER_ROW_H_MAX_REL = 0.046
# 行块模式：合并后的整行宽高比上限。水印行块实测 4.0~4.6（豆包）/ 4.6~9（通用
# 文字），而背景亮斑成行后 6.8~10.8（已去水印图 6.png 花丛 859x79）→ 上限 8。
CORNER_ROW_ASPECT_MAX = 8.0
# 行块模式宽高比**下限**（2026-09，防"已去水印图再误检"）：真水印整行 ≈ 5 字符 → 宽高比
# 4.0~4.6（豆包）；而已去水印图右下角的**单个**背景亮斑（地毯 1.png 30x25 比 1.20、草丛
# 7.png 80x60 比 1.33）会被行块模式当"粘连成行的水印"检出（`_text_likeness` 段数判据在
# 纹理上也会过）。加下限 2.5 把这类"矮胖单块"挡在门外，真水印行（≥4.0）稳过。
CORNER_ROW_ASPECT_MIN = 2.5


def _corner_row_boxes(white, H, W, x0, y0, pad):
    """行块模式：9x3 膨胀两次直接合并字符成行（水印字符与背景亮斑粘连、
    字符级分离失败时——如雪景雪点——仍能定位整行）。防御：
    1) 行框高度须落在水印字形高度带（相对短边 2.6%~4.6%）内，宽高比 ≤8
       （排除大面积粘连块，如 1.png 沙滩亮斑 12%、已去水印图背景碎块）；
    2) 组件必须整体位于 corner 检测区内（排除从区外伸进来的画面内容）；
    3) 文字性验证 + 贴边约束同字符行模式。"""
    import cv2
    short = min(H, W)
    merged = cv2.dilate(white, cv2.getStructuringElement(cv2.MORPH_RECT, (9, 3)), iterations=2)
    count, _, stats, _ = cv2.connectedComponentsWithStats(merged, 8)
    boxes = []
    for i in range(1, count):
        x, y, cw, ch, area = (int(v) for v in stats[i])
        gx1, gy1, gx2, gy2 = x0 + x, y0 + y, x0 + x + cw, y0 + y + ch
        if area < 400 or cw < ch * CORNER_ROW_ASPECT_MIN or cw / ch > CORNER_ROW_ASPECT_MAX:
            continue
        fill = area / float(cw * ch)
        if not (0.15 <= fill <= 0.95):
            continue
        if ch < short * CORNER_ROW_H_MIN_REL or ch > short * CORNER_ROW_H_MAX_REL:
            continue
        if gx1 < x0 - 20:
            continue
        if gx2 < W - 40 or gy2 < H - 40:
            continue
        fill_raw, segments = _text_likeness(white, x, y, x + cw, y + ch)
        if fill_raw > 0.6 or segments < 3:
            continue
        boxes.append((
            max(0, gx1 - pad), max(0, gy1 - pad), min(W - 6, gx2 + pad), min(H - 6, gy2 + pad)
        ))
    return boxes


def _text_rows_from_components(chars, H, W, x0, y0, pad):
    """把字符组件按 y 中心聚类成行，返回贴右下角的行框（含 pad）。"""
    if len(chars) < 4:
        return []
    chars.sort(key=lambda b: b[1] + b[3])
    rows = []
    for c in chars:
        cy = (c[1] + c[3]) / 2
        for row in rows:
            if abs(cy - row[0]) < max(c[3] - c[1], row[1]) * 0.7:
                row[2].append(c)
                row[0] = sum((b[1] + b[3]) / 2 for b in row[2]) / len(row[2])
                row[1] = max(row[1], c[3] - c[1])
                break
        else:
            rows.append([cy, c[3] - c[1], [c]])
    boxes = []
    for row in rows:
        if len(row[2]) < 4:
            continue
        hs = [b[3] - b[1] for b in row[2]]
        if max(hs) / max(1, min(hs)) > 1.8:
            continue
        bx1 = min(b[0] for b in row[2])
        by1 = min(b[1] for b in row[2])
        bx2 = max(b[2] for b in row[2])
        by2 = max(b[3] for b in row[2])
        if x0 + bx2 < W - 40 or y0 + by2 < H - 40:
            continue
        boxes.append((
            x0 + bx1 - pad, y0 + by1 - pad, min(W - 6, x0 + bx2 + pad), min(H - 6, y0 + by2 + pad)
        ))
    return boxes


def _corner_char_boxes(white, H, W, x0, y0, pad):
    """字符行模式：5x5 轻度膨胀合并字符内笔画，字符组件高度一致成行
    （水印是单行文字；沙滩亮斑高度杂乱不成行，自然排除）。字符高须落在
    水印字形高度带（相对短边 2.6%~5.0%）——否则已去水印图的背景碎块
    （1.3%~2.0%）会被当字符聚成行（地毯 2.png / 花丛 6.png 均实测）。"""
    import cv2
    short = min(H, W)
    white = cv2.dilate(white, cv2.getStructuringElement(cv2.MORPH_RECT, (5, 5)))
    count, _, stats, _ = cv2.connectedComponentsWithStats(white, 8)
    chars = []
    for i in range(1, count):
        x, y, cw, ch, area = (int(v) for v in stats[i])
        if ch < short * CORNER_CHAR_H_MIN_REL or ch > short * CORNER_CHAR_H_MAX_REL:
            continue
        if cw < ch * 0.25 or cw > ch * 7 or area < 120:
            continue
        chars.append((x, y, x + cw, y + ch))
    return _text_rows_from_components(chars, H, W, x0, y0, pad)


def _fuse_boxes(boxes):
    """迭代合并重叠框：水印在不同阈值下切出的组件不完整，融合后覆盖完整水印。"""
    boxes = list(boxes)
    changed = True
    while changed:
        changed = False
        result = []
        while boxes:
            cur = boxes.pop()
            i = 0
            while i < len(boxes):
                if _overlap(cur, boxes[i]):
                    o = boxes.pop(i)
                    cur = (
                        min(cur[0], o[0]),
                        min(cur[1], o[1]),
                        max(cur[2], o[2]),
                        max(cur[3], o[3]),
                    )
                    changed = True
                else:
                    i += 1
            result.append(cur)
        boxes = result
    return boxes


def refine_box_mask(image, box, model='mat'):
    """框内笔画精分割：把任意来源的候选框（检测框/手动框）缩小到笔画级 mask。

    方法：框内顶帽局部对比度分割（亮暗水印自适应）——背景用 31x31 椭圆开运算
    估计，diff = gray - opened（亮水印）或 opened - gray（暗水印），取响应更强的
    一侧，自适应阈值切笔画，小噪点剔除后小核膨胀。框内局部化比全图/corner 检测
    更精确，是"mask 最小化"原则对非模板水印的泛化。

    失败退回整框（返回 None）：分割结果几乎填满整框（背景与水印不可分）或过度
    碎化（组件数异常）。调用方需保证失败时使用整框 mask + 粗填（粗填弥补 mask 粗糙）。
    """
    try:
        import cv2
        import numpy as np
    except ImportError:
        return None
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    x1, y1, x2, y2 = box
    pad = 16
    rx1, ry1 = max(0, x1 - pad), max(0, y1 - pad)
    rx2, ry2 = min(w, x2 + pad), min(h, y2 + pad)
    region = img[ry1:ry2, rx1:rx2]
    if region.size == 0:
        return None
    gray = region.max(axis=2).astype(np.int16)
    # 低饱和过滤（灰白水印 RGB 均衡，彩色背景纹理/花瓣高光高饱和被剔除）——
    # 顶帽对复杂纹理的响应常强于微弱水印笔画，单靠对比度不可分
    sat = region.max(axis=2).astype(np.int16) - region.min(axis=2).astype(np.int16)
    low_sat = sat <= 60
    k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (31, 31))
    opened = cv2.morphologyEx(gray.astype(np.uint8), cv2.MORPH_OPEN, k).astype(np.int16)
    # 亮/暗水印自适应：哪侧局部对比度响应强用哪侧
    diff_bright = gray - opened
    diff_dark = opened - gray
    sides = [(diff_bright, 1), (diff_dark, -1)]
    sides.sort(key=lambda s: -np.percentile(np.abs(s[0]), 99))
    diff, _sign = sides[0]
    # 局部自适应阈值：背景非均匀（如同时含亮墙与暗花丛）时全局阈值顾此失彼
    #（花丛纹理抬高 std → 微弱笔画漏检）。用大窗口局部 mean+1.8*std 作为逐像素
    # 阈值，亮墙区阈值低（切出微弱笔画）、花丛区阈值高（抑制纹理误检）
    diff_f = diff.astype(np.float32)
    win = 96
    mean = cv2.boxFilter(diff_f, -1, (win, win))
    sq = cv2.boxFilter(diff_f * diff_f, -1, (win, win))
    std = np.sqrt(np.maximum(sq - mean * mean, 0))
    th_map = np.maximum(10.0, mean + 1.8 * std)
    strokes = ((diff_f >= th_map) & low_sat).astype(np.uint8)
    # 剔除小噪点（<9px 的孤立像素）
    n, lab, stats, centroids = cv2.connectedComponentsWithStats(strokes, 8)
    # 行带约束：水印是单行文字，组件 y 中心应聚在一条行带上；
    # 远离行带的组件多为背景纹理误检（花丛碎块散布无行结构）
    keep_ids = []
    ys = []
    for i in range(1, n):
        if stats[i, cv2.CC_STAT_AREA] >= 9:
            keep_ids.append(i)
            ys.append(centroids[i][1])
    if not keep_ids:
        return None
    ys_arr = np.array(ys)
    band_center = float(np.median(ys_arr))
    band_half = max(15.0, float(np.percentile(np.abs(ys_arr - band_center), 80)) * 1.5)
    cleaned = np.zeros_like(strokes)
    kept = 0
    for i, yc in zip(keep_ids, ys):
        if abs(yc - band_center) <= band_half:
            cleaned[lab == i] = 1
            kept += 1
    stroke_px = int(cleaned.sum())
    box_area = (x2 - x1) * (y2 - y1)
    # 失败判定：几乎填满整框（不可分）或过度碎化（背景细节误检）
    if stroke_px == 0 or stroke_px > box_area * 0.6 or kept > 300:
        return None
    kernel = REFINE_DILATE_MAT if model == 'mat' else REFINE_DILATE_LAMA
    mask = cv2.dilate(cleaned, cv2.getStructuringElement(cv2.MORPH_RECT, kernel), 1)
    full = np.zeros((h, w), np.uint8)
    # 裁剪回框内+pad（膨胀略超出候选框属正常：水印边缘本可能在框外 1-2px）
    full[ry1:ry2, rx1:rx2] = mask * 255
    if full.sum() == 0:
        return None
    return full


def refine_boxes_masks(image, boxes, model='mat'):
    """对多个候选框逐个精分割并合并；全部失败返回 None。"""
    import numpy as np
    merged = None
    for box in boxes:
        m = refine_box_mask(image, box, model)
        if m is not None:
            merged = m if merged is None else np.maximum(merged, m)
    return merged


def _detect_corner_faded(image):
    """右下角半透明/灰白水印兜底，双模式互补 + 顶帽兜底：
    1) 多阈值扫描 [248..150]，每个阈值同时跑两种模式并融合：
       字符行模式（5x5 膨胀，字符高度一致成行）——背景干净时精确紧贴水印；
       行块模式（9x3 膨胀两次 + 高度上限 6%H）——字符与背景亮斑粘连、
       字符级分离失败时（雪景雪点）仍能定位整行；
    2) 顶帽变换兜底（31x31 椭圆开运算 + 低饱和过滤 sat≤60）：背景光照不均、
       水印与亮背景亮度重叠时（橙红墙/花影）固定阈值不可分。
    所有模式都要求候选贴右下边缘，沙滩亮斑/花斑等画面内容被高度、
    位置与文字性判据自然排除，避免过度修复。"""
    try:
        import cv2
        import numpy as np
    except ImportError:
        return []
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    x0, y0 = int(w * 0.70), int(h * 0.88)
    region = img[y0:, x0:]
    if region.size == 0:
        return []
    gray = region.max(axis=2)
    pad = max(10, h // 150)
    all_boxes = []
    for threshold in (248, 240, 230, 220, 210, 200, 190, 180, 170, 160, 150):
        white = (gray >= threshold).astype(np.uint8)
        all_boxes += _corner_char_boxes(white, h, w, x0, y0, pad)
        all_boxes += _corner_row_boxes(white, h, w, x0, y0, pad)
    if all_boxes:
        return _fuse_boxes(all_boxes)
    k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (31, 31))
    tophat = cv2.morphologyEx(gray, cv2.MORPH_TOPHAT, k)
    sat = region.astype(np.int16).max(axis=2) - region.astype(np.int16).min(axis=2)
    all_boxes = []
    for threshold in (60, 50, 40, 30):
        white = ((tophat >= threshold) & (sat <= 60)).astype(np.uint8)
        # 顶帽只配字符行模式：顶帽图中花影/墙面亮斑与水印粘连，行块模式会把
        # 大片画面罩进框（6.png 花丛曾被整块重绘成墙面），宁可不检也不误修
        all_boxes += _corner_char_boxes(white, h, w, x0, y0, pad)
    if all_boxes:
        return _fuse_boxes(all_boxes)
    return []
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    x0, y0 = int(w * 0.70), int(h * 0.88)
    region = img[y0:, x0:]
    if region.size == 0:
        return []
    gray = region.max(axis=2)
    pad = max(10, h // 150)
    all_boxes = []
    for threshold in (248, 240, 230, 220, 210, 200, 190, 180, 170, 160, 150):
        white = (gray >= threshold).astype(np.uint8)
        all_boxes += _corner_text_boxes(white, h, w, x0, y0, pad)
    if all_boxes:
        return _fuse_boxes(all_boxes)
    k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (31, 31))
    tophat = cv2.morphologyEx(gray, cv2.MORPH_TOPHAT, k)
    sat = region.astype(np.int16).max(axis=2) - region.astype(np.int16).min(axis=2)
    for threshold in (60, 50, 40, 30):
        white = ((tophat >= threshold) & (sat <= 60)).astype(np.uint8)
        all_boxes += _corner_text_boxes(white, h, w, x0, y0, pad)
    if all_boxes:
        return _fuse_boxes(all_boxes)
    return []


def detect_watermark_box(image):
    """兼容旧接口：返回最高优先级（最靠右下）的一个水印框，检测不到返回 None。"""
    boxes = detect_watermark_boxes(image)
    if not boxes:
        return None
    return max(boxes, key=lambda box: box[0] + box[1])


def parse_box(value):
    try:
        parts = [int(part.strip()) for part in value.split(',')]
    except ValueError as exc:
        raise argparse.ArgumentTypeError('box must be four integers: x1,y1,x2,y2') from exc
    if len(parts) != 4:
        raise argparse.ArgumentTypeError('box must be four integers: x1,y1,x2,y2')
    return tuple(parts)


def parse_boxes(value):
    """一个或多个遮罩框，分号分隔：x1,y1,x2,y2;x1,y1,x2,y2（argparse 负数需用 = 传参）。"""
    boxes = [parse_box(part) for part in value.split(';') if part.strip()]
    if not boxes:
        raise argparse.ArgumentTypeError('at least one box is required')
    return boxes


def resolve_box(width, height, raw_box):
    if raw_box is None:
        return mask_box(width, height)
    x1, y1, x2, y2 = raw_box
    if x1 < 0:
        x1 += width
    if x2 <= 0:
        x2 += width
    if y1 < 0:
        y1 += height
    if y2 <= 0:
        y2 += height
    x1 = max(0, min(width, x1))
    x2 = max(0, min(width, x2))
    y1 = max(0, min(height, y1))
    y2 = max(0, min(height, y2))
    if x1 >= x2 or y1 >= y2:
        raise SystemExit(f'invalid mask box after resolving: {(x1, y1, x2, y2)}')
    return (x1, y1, x2, y2)


def review_box(name, width, height):
    mask_path = MASKS / name
    bbox = None
    if mask_path.exists():
        with Image.open(mask_path) as mask:
            bbox = mask.convert('L').getbbox()
    if bbox is None:
        bbox = (max(0, width - 330), max(0, height - 118), width - 8, height - 8)
    x1, y1, x2, y2 = bbox
    margin_x = max(80, (x2 - x1) // 2)
    margin_y = max(50, (y2 - y1) // 2)
    return (
        max(0, x1 - margin_x),
        max(0, y1 - margin_y),
        min(width, x2 + margin_x),
        min(height, y2 + margin_y),
    )


def ensure_work_dirs(root):
    for path in (SOURCE, MASKS, LAMA, REVIEW, backup_dir(root)):
        path.mkdir(parents=True, exist_ok=True)


MANIFEST = WORK / 'manifest.json'


def _md5(path):
    h = hashlib.md5()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


def prepare(names, custom_box, root, emit=True, model='mat', refine=False,
            any_position=False, use_profile=True):
    if WORK.exists():
        rmtree(WORK)
    ensure_work_dirs(root)
    backup_root = backup_dir(root)
    manifest = {}
    for name in names:
        current = Path(root) / name
        backup = backup_root / name
        if backup.exists() and _md5(backup) != _md5(current):
            # 备份与当前文件内容不一致：用户换了一批图 / 改了同名文件。备份的语义是
            # "本轮处理前的原图"（可撤销、可重跑）。若沿用旧备份当 origin，prepare 会
            # 一直在**错的图**上跑（白耗整批），最后 overwrite-review 的 md5 校验再把
            # 全部结果拒绝——用户看到"跑完但一张都没去水印"。故把旧备份挪到 .previous/
            # 留档，用当前文件刷新备份（幂等：旧备份留存不覆盖）。
            stale = backup_root / '.previous' / name
            stale.parent.mkdir(parents=True, exist_ok=True)
            if not stale.exists():
                copyfile(backup, stale)
            copyfile(current, backup)
            print(f'{name}: backup differs from the current file — refreshed '
                  f'(previous copy kept at {stale})')
        elif not backup.exists():
            copyfile(current, backup)
        copyfile(backup, SOURCE / name)
        # 记录本轮处理源文件的 md5：overwrite-review 覆盖前校验目标文件
        # 与源文件一致，防止 --root 传错目录时静默毁掉其它文件
        manifest[name] = _md5(backup)

        with Image.open(backup) as image:
            width, height = image.size
        boxes = []
        skipped = False
        mask = Image.new('L', (width, height), 0)
        if custom_box is None:
            import cv2
            import numpy as np
            with Image.open(backup) as probe:
                gray = np.array(probe.convert('RGB')).max(axis=2).astype(np.float32)
            if use_profile:
                # 水印档案库优先：命中已知水印时用其 α 图作为精确 mask，并标记
                # 该图走通用逆解（恢复真实背景）。档案来自用户一次学习，比启发式
                # 模板更可信，故排在豆包模板匹配之前，避免被误命中抢注。
                hit = match_profile(backup, any_position=any_position)
                if hit is not None:
                    profile, px, py, score, scale = hit
                    pmask = wprof.mask_for(profile, px, py, width, height, scale=scale)
                    if pmask is not None and int((pmask > 0).sum()) >= 20:
                        (MASKS / f'{name}.wprof').write_text(json.dumps({
                            'profile': profile.id, 'px': int(px), 'py': int(py),
                            'scale': float(scale), 'score': float(score)}))
                        Image.fromarray(pmask).save(MASKS / name)
                        print(f'{name}: profile {profile.id} matched at ({px},{py}) '
                              f'score {score:.1f} -> alpha mask + inverse')
                        continue
            tpl_mask, tpl_score, tpl_info = template_stroke_mask(gray, width, height, model)
            if tpl_mask is not None:
                # 模板笔画 mask：精确贴笔画+小膨胀，复杂背景（书本/花丛）
                # 也不会整块重绘
                mask = Image.fromarray(tpl_mask)
                (MASKS / f'{name}.tpl').write_text('')
                print(f'{name}: template mask ({tpl_info})')
                if not any_position:
                    # 默认模式只处理贴右下角的豆包水印，模板命中即完成
                    mask.save(MASKS / name)
                    continue
                # any_position：模板与其它位置检测叠加——DBNet 检出的右下角
                # 豆包水印框与模板重叠时丢弃，避免重复修复
                with Image.open(backup) as probe:
                    boxes = detect_watermark_boxes(probe, extended=True)
                boxes = [b for b in boxes if not tpl_mask[max(0, b[1]):b[3], max(0, b[0]):b[2]].any()]
                if boxes:
                    print(f'{name}: OCR detected {len(boxes)} extra watermark box(es) {boxes}')
                    draw = ImageDraw.Draw(mask)
                    for box in boxes:
                        draw.rounded_rectangle(box, radius=4, fill=255)
                mask.save(MASKS / name)
                continue
            with Image.open(backup) as probe:
                boxes = detect_watermark_boxes(probe, extended=any_position)
                # 豆包水印必贴右下角：丢弃远离右下角的检出框，
                # 否则雪景白点/白墙/栏杆等画面内容会被误检硬修（毁图）。
                # --any-position 显式开启时保留全部文字性通过的框（处理
                # 任意位置的其它文字水印），误检风险由调用方承担
                pw, ph = probe.size
                if not any_position:
                    boxes = [b for b in boxes if b[2] > pw - 40 and b[3] > ph - 40]
            if boxes:
                print(f'{name}: auto-detected {len(boxes)} watermark box(es) {boxes} (template fallback: {tpl_info})')
                if len(boxes) > 6:
                    print(f'{name}: WARNING {len(boxes)} boxes detected — busy photo '
                          f'false positives are likely; review the candidate review '
                          f'image carefully before overwriting')
            else:
                # 检测不到水印：写全空 mask 跳过修复，绝不用默认规则硬修——
                # 对已无水印的图硬修会把真实画面重绘成模糊块
                skipped = True
                print(f'{name}: no watermark detected, skipped (template fallback: {tpl_info})')
        else:
            boxes = [resolve_box(width, height, b) for b in custom_box]
        if not skipped and not (MASKS / f'{name}.tpl').exists() and boxes:
            # 框内笔画精分割（mask 最小化原则的泛化）：手动框/检测框都先尝试
            # 缩小到笔画级；失败（背景与水印不可分/过度碎化）退回整框+粗填。
            # refine 是实验性能力（--refine 显式开启）：复杂纹理背景可能严重漏检
            # 笔画（3.png 纸面纹理案例漏检 90%），默认整框 + 粗填 + MAT 兜底
            refined = None
            if refine:
                with Image.open(backup) as probe:
                    refined = refine_boxes_masks(probe, boxes, model)
            if refined is not None:
                mask = Image.fromarray(refined)
                (MASKS / f'{name}.tpl').write_text('')
                px = int((refined > 0).sum())
                print(f'{name}: box-refined stroke mask ({px}px from {len(boxes)} box(es))')
            else:
                draw = ImageDraw.Draw(mask)
                for box in boxes:
                    draw.rounded_rectangle(box, radius=4, fill=255)
                reason = 'refine failed' if refine else 'refine disabled'
                print(f'{name}: {reason}, fallback to full box mask ({len(boxes)} box(es))')
        # skipped 也保存空 mask：inpaint 据此透传原图，不进模型
        mask.save(MASKS / name)

    MANIFEST.write_text(json.dumps(manifest))
    output = REVIEW / 'source-corner-review.png'
    review(SOURCE, output.name, names)
    if emit:
        print(output)
    return output

# ---------------------------------------------------------------------------
# 结果级验证闭环（场景无关）：修复后客观自检 + 低置信度主动告警。
# 核心保证：① "mask 外零改动"——任何水印、任何场景都成立，被破坏即工程 bug；
# ② 模板残留——豆包水印修复后不应再匹配到模板字形；③ mask 面积占比（过度重绘告警）。
# 不依赖 ground truth，故可泛化到未见过的场景：修完必须过检，否则拒绝落盘。
# ---------------------------------------------------------------------------

def verify_paths(orig_path, res_path, mask_path, outside_tol=2, warn_ratio=0.08,
                 template_applied=None):
    """对 (原图, 修复结果, mask) 三元组做客观验证，返回报告 dict；缺文件返回 None。
    template_applied 为 True 时才做"豆包模板残留"检查（否则非豆包水印可能碰巧匹配模板
    而误报）；为 None 时按 scale≈1.0 自动判断。"""
    import numpy as np

    if not (orig_path and res_path and mask_path):
        return None
    orig_path, res_path, mask_path = Path(orig_path), Path(res_path), Path(mask_path)
    if not (orig_path.exists() and res_path.exists() and mask_path.exists()):
        return None
    with Image.open(orig_path) as im:
        orig = np.array(im.convert('RGB')).astype(np.int16)
    with Image.open(res_path) as im:
        res = np.array(im.convert('RGB')).astype(np.int16)
    if orig.shape != res.shape:
        return None
    with Image.open(mask_path) as im:
        m = im.convert('L')
        if m.size != (orig.shape[1], orig.shape[0]):
            m = m.resize((orig.shape[1], orig.shape[0]))
        mask = np.array(m)
    active = mask > 0
    area = int(active.sum())
    total = int(mask.shape[0] * mask.shape[1])
    diff = np.abs(orig - res).max(axis=2)
    report = {
        'mask_area': area,
        'mask_ratio': area / float(total) if total else 0.0,
        'passthrough': area == 0,
        'inside_changed': int((diff[active] > 10).sum()) if area else 0,
        'outside_changed': int((diff[~active] > outside_tol).sum()),
        'outside_max': int(diff[~active].max()) if (~active).any() else 0,
        'reasons': [],
    }
    # 模板残留：原图命中模板 → 修复后不应再命中（豆包水印专用判据，非豆包图自动跳过）。
    # 结果分取**源图水印锚点处**的分（而非角窗最大分）——真实残留必然在锚点处最强，而
    # 强纹理背景（沙地/花墙）会在角窗别处凑高分，用最大分会误报 FAIL（见
    # TEMPLATE_RESIDUAL_DOMINANCE 注释）。res_win 仅用于"锚点是否主导峰"的判定。
    orig_score = res_score = res_win = 0.0
    try:
        import cv2
        gray_o = np.array(Image.open(orig_path).convert('RGB')).max(axis=2).astype(np.float32)
        gray_r = np.array(Image.open(res_path).convert('RGB')).max(axis=2).astype(np.float32)
        resp_o = _template_response(gray_o, gray_o.shape[1], gray_o.shape[0])
        resp_r = _template_response(gray_r, gray_r.shape[1], gray_r.shape[0])
        if resp_o is not None:
            ro, _th, _tw = resp_o
            y0 = max(0, ro.shape[0] - 41)
            x0 = max(0, ro.shape[1] - 41)
            _, _, _, loc = cv2.minMaxLoc(ro[y0:, x0:])
            ox, oy = loc[0] + x0, loc[1] + y0
            orig_score = float(ro[oy, ox])
            if resp_r is not None:
                rr, _th, _tw = resp_r
                if oy < rr.shape[0] and ox < rr.shape[1]:
                    res_score = float(rr[oy, ox])
                yr = max(0, rr.shape[0] - 41)
                xr = max(0, rr.shape[1] - 41)
                res_win = float(rr[yr:, xr:].max())
    except Exception:
        pass
    report['orig_template_score'] = round(float(orig_score), 1)
    report['res_template_score'] = round(float(res_score), 1)
    report['res_template_window_max'] = round(float(res_win), 1)
    # 锚点分须≥0.85×角窗最大分（"水印处即主导峰"）：背景巧合高分达不到 → 不算残留。
    residual_dominant = res_score >= TEMPLATE_RESIDUAL_DOMINANCE * res_win if res_win > 0 else False
    # 是否做模板残留检查：优先用调用方给的 template_applied；未给则要求
    # scale≈1.0（合成/缩放图上的非豆包水印可能碰巧高分，会误报）
    if template_applied is None:
        scale = min(orig.shape[0], orig.shape[1]) / STAMP_REF_SHORT
        template_applied = (orig_score >= TEMPLATE_MIN_SCORE
                            and abs(scale - 1.0) <= INVERSE_SCALE_TOL)
    # 结构化残留标记：供自动重试逻辑判定"是否因水印残留而不完美"
    # （区别于 mask 外改动——那是工程 bug，重试无法修复）
    report['residual'] = bool(
        template_applied and orig_score >= TEMPLATE_MIN_SCORE and residual_dominant
        and (res_score >= TEMPLATE_MIN_SCORE
             or (res_score >= TEMPLATE_RESIDUAL_FLOOR
                 and res_score >= orig_score * TEMPLATE_RESIDUAL_RATIO)))
    verdict = 'PASS'
    if report['outside_changed'] > 0:
        verdict = 'FAIL'
        report['reasons'].append(
            f"mask 外有 {report['outside_changed']} px 被改动 (max {report['outside_max']})")
    if (template_applied and orig_score >= TEMPLATE_MIN_SCORE
            and res_score >= TEMPLATE_MIN_SCORE and residual_dominant):
        verdict = 'FAIL'
        report['reasons'].append(
            f'修复后仍匹配豆包模板 (score {res_score:.1f} >= {TEMPLATE_MIN_SCORE:.0f})，疑有残留')
    elif (template_applied and orig_score >= TEMPLATE_MIN_SCORE
          and res_score >= TEMPLATE_MIN_SCORE * 0.6 and residual_dominant):
        if verdict != 'FAIL':
            verdict = 'WARN'
        report['reasons'].append(
            f'修复后模板分数偏高 ({res_score:.1f})，可能有残留，请放大复查')
    if (not report['passthrough']) and report['mask_ratio'] > warn_ratio:
        if verdict == 'PASS':
            verdict = 'WARN'
        report['reasons'].append(
            f"mask 占图 {report['mask_ratio'] * 100:.1f}% (> {warn_ratio * 100:.0f}%)，可能过度重绘")
    report['verdict'] = verdict
    return report


def verify_repaired(name, root):
    """按工作目录约定验证本轮修复结果：原图取备份（无则 source），结果取 lama。
    模板 sidecar 存在说明本轮走了豆包模板路径，则启用模板残留检查。"""
    orig = backup_dir(root) / name
    if not orig.exists():
        orig = SOURCE / name
    # 逆解已采用的图不做模板残留判据：gap-score 是为"亮于背景的笔画"设计的，对逆解
    # 恢复的真实纹理（同样有高频结构）会误报 WARN/FAIL，而逆解正确性已由结果择优把关
    # （inverse 残留 <20 且 ≤ MAT×1.75 才采用）。mask 外零改动等检查仍照常。
    tpl_applied = ((MASKS / f'{name}.tpl').exists()
                   and not (MASKS / f'{name}.inv').exists())
    return verify_paths(orig, LAMA / name, MASKS / name, template_applied=tpl_applied)


def format_verify(report):
    if report is None:
        return None
    if report['passthrough']:
        head = 'no mask (passthrough)'
    else:
        head = (f"mask {report['mask_area']}px ({report['mask_ratio'] * 100:.1f}%), "
                f"inside changed {report['inside_changed']}")
    return (f"[{report['verdict']}] {head}, outside changed {report['outside_changed']} "
            f"(max {report['outside_max']}), tmpl {report['orig_template_score']}"
            f"->{report['res_template_score']}")


def report_verify(name, report):
    line = format_verify(report)
    if line is None:
        return
    print(f'verify {name}: {line}')
    for reason in report['reasons']:
        print(f'verify {name}: {reason}')


def review(input_dir, output_name, names):
    REVIEW.mkdir(parents=True, exist_ok=True)
    crops = []
    labels = []
    for name in names:
        with Image.open(Path(input_dir) / name) as image:
            image = image.convert('RGB')
            width, height = image.size
            crop = image.crop(review_box(name, width, height))
            crop = crop.resize((max(1, crop.width // 2), max(1, crop.height // 2)), Image.Resampling.LANCZOS)
            crops.append(crop)
            labels.append(name)

    pad = 16
    label_height = 28
    header_height = 34
    sheet_width = max(crop.width for crop in crops) + pad * 2
    sheet_height = header_height + pad + sum(label_height + crop.height + pad for crop in crops)
    sheet = Image.new('RGB', (sheet_width, sheet_height), (30, 30, 30))
    draw = ImageDraw.Draw(sheet)
    font = ImageFont.load_default()
    draw.text((pad, pad), 'corner review', fill=(240, 240, 240), font=font)
    y = pad + header_height
    for label, crop in zip(labels, crops):
        draw.text((pad, y), label, fill=(240, 240, 240), font=font)
        y += label_height
        sheet.paste(crop, (pad, y))
        y += crop.height + pad
    sheet.save(REVIEW / output_name)


# 残留自动重试（默认开，最多 1 轮）：自校验判定"水印残留"时把 mask 逐级膨胀一级
# 后重跑 MAT。这是"不完美就重新处理"的落点——残留来自 mask 盖不住（对齐/抗锯齿
# 误差），小一级膨胀即可；只在明确的残留 FAIL 上触发，mask 外改动属工程 bug 不重试。
RETRY_DILATE = (5, 5)
# 暗字形（过冲）自动重试：当 mask 盖不住水印的淡边缘/暗描边时，MAT 会把残留的暗边
# 当内容保留 → 结果字形区比周围暗（1.png 地毯流苏实测 foot-gap −23，肉眼即"暗色水印
# 印记"）。检测到即把 mask 膨胀到 OVERSHOOT_DILATE 重跑该图（实测 −23 → −1.3）。
OVERSHOOT_DILATE = (15, 15)

# 自裁小块推理：iopaint 的 MAT 会把输入补齐成 512 的方形（min_size=512 / pad_mod=512 /
# pad_to_square），整图或大裁块会被补到 1024²，而 MAT 耗时随尺寸非线性暴涨——实测同一张
# 2848×1600：整图 63s、802×626 裁块 53s、480×480 裁块仅 10s。故按水印位置自裁 ≤512 的
# 方块（补齐后仍是 512²），只把 **mask 区域** 贴回原图 → mask 外逐字节不变。
CROP_SIDE_MAX = 512
CROP_MARGIN = 128
# 扩散修补（**可选**，--sd 开；解决"水印压在高频纹理上"的固有难题）：MAT/LaMa 是"平滑
# 填充器"，只会插值、不会生成花瓣/枝叶，水印压在花丛上时会留下可见糊块（6.png 实测）。
# 用 Stable Diffusion 生成式修补可"脑补"出可信纹理，肉眼明显更自然。代价：要下模型
# （~4GB）、单图分钟级（M1 8GB 实测 LCM 6 步 ≈ 2–4min，MAT 仅 ~10s），故**默认关**，
# 且只对"背景有高频纹理"的图启用（按水印周边环带高频能量路由，平滑背景仍走 MAT）。
SD_MODEL = 'runwayml/stable-diffusion-inpainting'
SD_LCM_LORA = 'latent-consistency/lcm-lora-sdv1-5'
SD_STEPS = 6              # LCM-LoRA 少步采样（4–8 步即可，实测与 25 步质量相当）
SD_GUIDANCE = 1.5         # LCM 用低 CFG
SD_SEED = 42
SD_PROMPT = ''
SD_TEXTURE_MIN = 9.0      # 水印周边环带高频能量阈值（>此值才走 SD）；平滑图 1–5、花丛 ~10
SD_TEXTURE_RING = 90      # 环带外扩像素
_sd_pipe_cache = None


def _crop_box(mask, w, h):
    """按 mask 包围盒居中取 ≤CROP_SIDE_MAX 的方块（保证把水印连同上下文都框进去，
    且补齐后仍是 512²）。返回 (x1, y1, x2, y2)，无 mask 像素时返回 None。
    设 `WM_INPAINT_NO_CROP=1` 可禁用自裁（整图进 iopaint，仅供排查/对照）。"""
    import numpy as np
    if os.environ.get('WM_INPAINT_NO_CROP'):
        return None
    ys, xs = np.nonzero(np.asarray(mask) > 0)
    if len(xs) == 0:
        return None
    x1, x2 = int(xs.min()), int(xs.max()) + 1
    y1, y2 = int(ys.min()), int(ys.max()) + 1
    core = max(x2 - x1, y2 - y1)
    side = core + 2 * CROP_MARGIN
    if core <= CROP_SIDE_MAX:
        side = min(CROP_SIDE_MAX, side)
    side = min(side, w, h)
    cx, cy = (x1 + x2) // 2, (y1 + y2) // 2
    X1 = min(max(cx - side // 2, 0), w - side)
    Y1 = min(max(cy - side // 2, 0), h - side)
    return X1, Y1, X1 + side, Y1 + side


def _iopaint_batch(names, model, src_dir, mask_dir, out_dir):
    """自裁小块跑一次 iopaint，再把每个裁块结果的 **mask 像素** 贴回原图（mask 外保持
    逐字节不变）。无有效 mask 的图退化为整图交给 iopaint（安全兜底）。"""
    import numpy as np
    crop_root = WORK / 'crop'
    sub_src = crop_root / 'source'
    sub_msk = crop_root / 'masks'
    sub_out = crop_root / 'lama'
    if crop_root.exists():
        rmtree(crop_root)
    for path in (sub_src, sub_msk, sub_out):
        path.mkdir(parents=True, exist_ok=True)
    boxes = {}
    for name in names:
        with Image.open(src_dir / name) as im:
            rgb = im.convert('RGB')
            w, h = rgb.size
        with Image.open(mask_dir / name) as mk:
            mask = np.array(mk.convert('L'))
        box = _crop_box(mask, w, h)
        if box is None:
            rgb.save(sub_src / name)
            Image.fromarray(mask).save(sub_msk / name)
        else:
            rgb.crop(box).save(sub_src / name)
            Image.fromarray(mask[box[1]:box[3], box[0]:box[2]]).save(sub_msk / name)
        boxes[name] = box
    subprocess.run(
        [
            str(IOPAINT), 'run', '--model', model, '--device', DEVICE,
            '--image', str(sub_src), '--mask', str(sub_msk), '--output', str(sub_out),
        ],
        check=True,
    )
    for name in names:
        box = boxes[name]
        if box is None:
            copyfile(sub_out / name, out_dir / name)
            continue
        X1, Y1, X2, Y2 = box
        out = np.array(Image.open(src_dir / name).convert('RGB'))
        res = np.array(Image.open(sub_out / name).convert('RGB'))
        with Image.open(mask_dir / name) as mk:
            sel = np.array(mk.convert('L'))[Y1:Y2, X1:X2] > 0
        region = out[Y1:Y2, X1:X2]
        region[sel] = res[sel]
        out[Y1:Y2, X1:X2] = region
        Image.fromarray(out).save(out_dir / name)
    rmtree(crop_root, ignore_errors=True)


def _hf_reachable(timeout=4.0):
    """直连 HuggingFace 是否可用（HTTPS 实测；TCP 握手常被透明代理放行、TLS 才被墙）。"""
    import urllib.request
    try:
        urllib.request.urlopen(
            'https://huggingface.co/api/models/runwayml/stable-diffusion-inpainting',
            timeout=timeout).read(1)
        return True
    except Exception:
        return False


def _ring_hf(obs, mask, margin=SD_TEXTURE_RING):
    """水印**周边环带**（排除字形框本身）的高频能量——衡量背景纹理复杂度。字形自身
    边缘也是高频，故必须排除，否则所有图都会被误判为"有纹理"。"""
    import cv2
    import numpy as np

    ys, xs = np.nonzero(np.asarray(mask) > 0)
    if len(xs) == 0:
        return 0.0
    h, w = mask.shape
    x1, x2 = max(0, int(xs.min()) - margin), min(w, int(xs.max()) + 1 + margin)
    y1, y2 = max(0, int(ys.min()) - margin), min(h, int(ys.max()) + 1 + margin)
    reg = obs[y1:y2, x1:x2].astype(np.float32)
    ring = np.ones(reg.shape[:2], bool)
    ring[int(ys.min()) - y1:int(ys.max()) + 1 - y1, int(xs.min()) - x1:int(xs.max()) + 1 - x1] = False
    if not ring.any():
        return 0.0
    hf = np.abs(reg - cv2.GaussianBlur(reg, (0, 0), 2.0)).mean(2)
    return float(hf[ring].mean())


def _sd_pipe():
    """惰性加载 SD 修补管线（fp16 + MPS + LCM-LoRA，少步采样）。进程内单例。"""
    global _sd_pipe_cache
    if _sd_pipe_cache is not None:
        return _sd_pipe_cache
    # 必须在 import huggingface_hub 之前设置（其 ENDPOINT 常量在 import 时读取环境变量）。
    if not os.environ.get('HF_ENDPOINT') and not _hf_reachable():
        # 直连 HuggingFace 不通（常见于国内）时自动走镜像；已缓存模型同样受益。
        os.environ['HF_ENDPOINT'] = 'https://hf-mirror.com'
        print('HF unreachable, using HF_ENDPOINT=https://hf-mirror.com')
    import torch
    from diffusers import LCMScheduler, StableDiffusionInpaintPipeline

    device = 'mps' if torch.backends.mps.is_available() else 'cpu'
    dtype = torch.float16 if device == 'mps' else torch.float32
    pipe = StableDiffusionInpaintPipeline.from_pretrained(
        SD_MODEL, torch_dtype=dtype, safety_checker=None, requires_safety_checker=False)
    pipe = pipe.to(device)
    pipe.set_progress_bar_config(disable=True)
    pipe.load_lora_weights(SD_LCM_LORA)
    pipe.fuse_lora()
    pipe.unload_lora_weights()
    pipe.scheduler = LCMScheduler.from_config(pipe.scheduler.config)
    _sd_pipe_cache = (pipe, device)
    return _sd_pipe_cache


def _sd_generator(device, seed):
    """独立成函数便于回归打桩（CI 无 torch）。"""
    import torch
    return torch.Generator(device).manual_seed(seed)


def _sd_inpaint(names, texture_min=SD_TEXTURE_MIN):
    """对**背景有高频纹理**的图用 SD 生成式修补水印区（平滑图跳过，仍走 MAT）。
    自裁 ≤512 方块推理，只把 **mask 像素** 贴回（mask 外逐字节不变），写回 LAMA。"""
    import numpy as np

    routed = []
    for name in names:
        mask_path = MASKS / name
        if not mask_path.exists() or not (SOURCE / name).exists():
            continue
        mask = np.array(Image.open(mask_path).convert('L'))
        if mask.max() == 0:
            continue
        with Image.open(SOURCE / name) as im:
            obs = np.array(im.convert('RGB'))
        hf = _ring_hf(obs, mask)
        if hf >= texture_min:
            routed.append((name, hf))
    if not routed:
        print(f'sd: no textured image (all background hf < {texture_min:g}), keep MAT')
        return set()
    print('sd routing (textured background): '
          + ', '.join(f'{n} (hf {v:.1f})' for n, v in routed))
    pipe, device = _sd_pipe()
    applied = set()
    for name, hf in routed:
        with Image.open(SOURCE / name) as im:
            rgb = im.convert('RGB')
            w, h = rgb.size
        mask = np.array(Image.open(MASKS / name).convert('L'))
        box = _crop_box(mask, w, h)
        if box is None:
            continue
        X1, Y1, X2, Y2 = box
        crop = rgb.crop(box)
        cmask = Image.fromarray(mask[Y1:Y2, X1:X2])
        side = (X2 - X1, Y2 - Y1)
        if side != (512, 512):
            crop = crop.resize((512, 512))
            cmask = cmask.resize((512, 512))
        out = pipe(
            prompt=SD_PROMPT, image=crop, mask_image=cmask,
            num_inference_steps=SD_STEPS, guidance_scale=SD_GUIDANCE,
            output_type='np', generator=_sd_generator(device, SD_SEED),
        ).images[0]
        out = Image.fromarray((out * 255).round().astype('uint8'))
        if side != (512, 512):
            out = out.resize(side)
        out = np.array(out)
        base = np.array(Image.open(LAMA / name).convert('RGB'))
        sel = mask[Y1:Y2, X1:X2] > 0
        region = base[Y1:Y2, X1:X2]
        region[sel] = out[sel]
        base[Y1:Y2, X1:X2] = region
        Image.fromarray(base).save(LAMA / name)
        (MASKS / f'{name}.sd').write_text('')
        applied.add(name)
        print(f'{name}: sd applied (bg hf {hf:.1f}, {SD_STEPS} steps)')
    return applied


def _expand_mask(mask_path, kernel=RETRY_DILATE):
    """把笔画 mask 往外膨胀一级（仍紧贴水印，最小侵入），返回新增像素数；空 mask 返回 0。"""
    import cv2
    import numpy as np

    m = np.array(Image.open(mask_path).convert('L'))
    if not (m > 0).any():
        return 0
    grown = cv2.dilate(m, cv2.getStructuringElement(cv2.MORPH_RECT, kernel), 1)
    Image.fromarray(grown).save(mask_path)
    return int((grown > m).sum())


def _residual_state(name):
    """本轮修复结果是否"因水印残留而不完美"：原图取 SOURCE（本轮未改动原图），
    结果取 LAMA，.tpl sidecar 存在即启用豆包模板残留判据。返回 (是否残留, 报告)。"""
    report = verify_paths(SOURCE / name, LAMA / name, MASKS / name,
                          template_applied=(MASKS / f'{name}.tpl').exists())
    return bool(report and report.get('residual')), report


def _retry_inpaint(names, model):
    """把待重试的图单独重跑一次 iopaint（自裁小块，避免整批重复推理），结果写回 LAMA。"""
    _iopaint_batch(names, model, SOURCE, MASKS, LAMA)


def _residual_retry(names, model, skip=()):
    """对残留图扩 mask 并用 MAT 重跑复验；仍残留则打印 FAIL（交给 overwrite-review 拒绝落盘）。
    skip 中的图（逆解已成功）不参与 MAT 重试——避免精确逆解被生成式重绘覆盖。"""
    skip = set(skip)
    candidates = [name for name in names
                  if name not in skip and _residual_state(name)[0]]
    if not candidates:
        return
    print(f'verify: watermark residual detected in {", ".join(candidates)} — '
          f'expanding mask and retrying once (max 1 round)')
    for name in candidates:
        added = _expand_mask(MASKS / name)
        print(f'{name}: retry mask +{added}px (dilate {RETRY_DILATE[0]}x{RETRY_DILATE[1]})')
    _retry_inpaint(candidates, model)
    for name in candidates:
        residual, report = _residual_state(name)
        report_verify(name, report)
        if residual:
            print(f'{name}: STILL residual after retry — overwrite-review will refuse '
                  f'unless --force is used')


def _overshoot_footgap(name):
    """结果图的**暗字形（过冲）**指标：用原图（SOURCE）定位水印位置（结果图已无水印，
    模板定位会失败），再量 LAMA 结果在 stamp 笔画区相对间隙区的低频亮度差
    （负值=笔画偏暗=暗字形）。仅对模板命中的图有意义。"""
    import cv2
    import numpy as np

    with Image.open(SOURCE / name) as im:
        rgb = im.convert('RGB')
        w, h = rgb.size
        gray = np.array(rgb).max(axis=2).astype(np.float32)
    _mask, _score, info = template_stroke_mask(gray, w, h)
    mm = re.search(r'at \((\d+),(\d+)\)', info)
    if not mm:
        return None
    px, py = int(mm.group(1)), int(mm.group(2))
    with Image.open(LAMA / name) as im:
        out = np.array(im.convert('RGB')).astype(np.float32)
    return _result_overshoot_score(out, px, py)


def _overshoot_retry(names, model, skip=()):
    """暗字形（过冲）自动重试（最多 1 轮）：mask 盖不住水印淡边缘/暗描边时，MAT 会把
    残留暗边当内容保留 → 字形区比周围暗。检测到即扩 mask 重跑——这是"换一张图就失效"
    的兜底：判据与图无关，只要结果出现暗字形就重画。"""
    skip = set(skip)
    candidates = []
    for name in names:
        if name in skip or not (MASKS / f'{name}.tpl').exists():
            continue
        ov = _overshoot_footgap(name)
        if ov is not None and ov < -INVERSE_OVERSHOOT_MAX:
            candidates.append(name)
            print(f'{name}: dark glyph detected (foot-gap {ov:.1f}) — expanding mask and retrying')
    if not candidates:
        return
    for name in candidates:
        added = _expand_mask(MASKS / name, OVERSHOOT_DILATE)
        print(f'{name}: overshoot retry mask +{added}px '
              f'(dilate {OVERSHOOT_DILATE[0]}x{OVERSHOOT_DILATE[1]})')
    _retry_inpaint(candidates, model)
    for name in candidates:
        ov = _overshoot_footgap(name)
        if ov is not None:
            print(f'{name}: after overshoot retry foot-gap {ov:.1f}')


def inpaint(model='mat', inverse=False, retry=True, use_profile=True, sd=False,
            sd_threshold=SD_TEXTURE_MIN):
    if not IOPAINT.exists():
        raise SystemExit('missing project env; run tools/ensure-inpaint-env.sh (or .ps1 on Windows) first')
    try:
        import numpy as np
    except ImportError:
        raise SystemExit('missing numpy in project env')
    # 注：iopaint 推理时会把 mask 区域的 source 像素置零（MAT/LaMa 均如此，
    # 受控实验输出逐像素相同），任何粗填/预处理都无法影响模型输入——
    # 修复质量完全由 mask 外的上下文决定，mask 精度是唯一杠杆。
    # 全部透传后 source 为空：无需进模型，直接结束（避免 iopaint 空目录报错）
    for mask_path in sorted(MASKS.glob('*.png')):
        name = mask_path.name
        mask = np.array(Image.open(mask_path).convert('L'))
        if mask.max() == 0:
            # 空 mask（未检测到水印）的图直接原样通过，不进模型
            copyfile(SOURCE / name, LAMA / name)
            (SOURCE / name).unlink()
            mask_path.unlink()
            print(f'{name}: empty mask, passed through without inpainting')
    if not any(SOURCE.iterdir()):
        print('all images passed through: no watermark to remove')
        return
    active_names = [p.name for p in sorted(SOURCE.glob('*.png'), key=numeric_key)]
    # MAT（mask-aware transformer）对结构边界的重建显著优于 LaMa：
    # 光斑/阴影/花瓣形态保留更完整（6.png 花墙阴影、光斑锐度对比验证），
    # 代价是推理约慢 12 倍（单图 ~2 分钟 vs ~10 秒）。lama 可用 --model lama 回退。
    # 自裁小块推理（见 _iopaint_batch）：MAT 补齐成 512²，避开整图补到 1024² 的非线性耗时。
    _iopaint_batch(active_names, model, SOURCE, MASKS, LAMA)
    # 扩散修补（可选）：水印压在高频纹理（花丛等）上时 MAT 会糊，改用 SD 生成式修补。
    sd_done = set()
    if sd:
        sd_done = _sd_inpaint(active_names, sd_threshold)
    inverse_done = set()
    if inverse:
        # inverse（默认开，逐图自动择优）：模板命中且纹理复杂的 scale≈1.0 图用
        # 完整 stamp 逆解恢复真实背景（覆盖 MAT 结果）；其余图 gating 判定后保持
        # MAT。见 inverse_apply gating。
        applied = []
        for sidecar in sorted(MASKS.glob('*.tpl')):
            name = sidecar.stem
            if name in sd_done:  # SD 已生成式修补（保纹理），不再叠逆解
                continue
            obs_path, mat_path = SOURCE / name, LAMA / name
            if not obs_path.exists() or not mat_path.exists():
                continue
            res = inverse_apply(obs_path, mat_path, model)
            if res is None:
                print(f'{name}: inverse skipped (gating), keep MAT')
                continue
            out, (px, py, hf, gain, calib) = res
            # 结果择优：逆解 vs MAT。四条件同时满足才采用逆解（保住真实纹理）：
            # ① 绝对分 < TEMPLATE_MIN_SCORE（无强字形残留）；
            # ② 不显著差于 MAT（×TOL 之上再放 SLACK，抵消"真实纹理抬高 gap-score"的偏差）；
            # ③ 无**正字形残影**（≤ GHOST_MAX，区分"真纹理"与"水印残留"，见 _result_ghost_score）；
            # ④ 无**过冲**（笔画区低频不低于间隙区 −INVERSE_OVERSHOOT_MAX；③ 只查正值，
            #    减过头留下的**暗字形**会漏网，见 _result_overshoot_score）。
            # 任一不过 → 保留 MAT（避免暗底噪声团/失配字形/暗字形覆盖更干净的 MAT）。
            inv_score = _result_template_score(out)
            mat_score = _result_template_score(mat_path)
            ghost = _result_ghost_score(out, mat_path, px, py)
            overshoot = _result_overshoot_score(out, px, py)
            if (inv_score < TEMPLATE_MIN_SCORE
                    and inv_score <= mat_score * INVERSE_RESIDUAL_TOLERANCE + INVERSE_RESIDUAL_SLACK
                    and (ghost is None or ghost <= INVERSE_GHOST_MAX)
                    and (overshoot is None or overshoot >= -INVERSE_OVERSHOOT_MAX)):
                Image.fromarray(out).save(mat_path)
                inverse_done.add(name)
                # .inv 标记：verify 对逆解结果放宽模板残留判据（gap-score 对逆解恢复的
                # 真实纹理会误报，见 verify_repaired）
                (MASKS / f'{name}.inv').write_text('')
                applied.append(f'{name} (pos {px},{py} hf {hf:.1f} gain {gain:.2f} '
                               f'calib {calib:.2f} ghost {ghost:.2f} overshoot {overshoot:.1f}; '
                               f'resid inv {inv_score:.1f} vs mat {mat_score:.1f})')
            else:
                print(f'{name}: inverse rejected by result comparison '
                      f'(resid inv {inv_score:.1f} vs mat {mat_score:.1f} ghost {ghost:.2f} '
                      f'overshoot {overshoot:.1f}), keep MAT')
        if applied:
            print('inverse stamp applied: ' + '; '.join(applied))
    if inverse and use_profile and wprof is not None:
        # 档案库逆解：对命中档案的图用其 α/C 恢复真实背景（覆盖 MAT 结果）。
        applied = []
        for sidecar in sorted(MASKS.glob('*.wprof')):
            name = sidecar.stem
            if name in sd_done:
                continue
            obs_path, mat_path = SOURCE / name, LAMA / name
            if not obs_path.exists() or not mat_path.exists():
                continue
            info = json.loads(sidecar.read_text())
            try:
                profile = wprof.load_profile(info['profile'])
            except SystemExit:
                print(f'{name}: profile {info["profile"]} missing, keep MAT')
                continue
            if profile.extra and profile.extra.get('inverse') is False:
                # profile declares its blend model is not invertible (e.g. the
                # watermark has a dark outline a single-colour model cannot
                # undo): keep the stroke-level MAT result instead.
                print(f'{name}: profile {profile.id} not invertible (outline), keep MAT')
                continue
            obs = np.array(Image.open(obs_path).convert('RGB'))
            mat = np.array(Image.open(mat_path).convert('RGB'))
            out, detail = wprof.inverse_image(obs, mat, profile,
                                              info['px'], info['py'], scale=info['scale'])
            if out is None:
                print(f'{name}: profile inverse skipped ({detail}), keep MAT')
                continue
            Image.fromarray(out).save(mat_path)
            inverse_done.add(name)
            (MASKS / f'{name}.inv').write_text('')
            applied.append(f'{name} ({detail})')
        if applied:
            print('profile inverse applied: ' + '; '.join(applied))
    if retry:
        # 暗字形（过冲）优先重试：mask 不足导致 MAT 保留水的暗边，先扩 mask 重画。
        _overshoot_retry(active_names, model, skip=inverse_done | sd_done)
    if retry:
        # 自校验不完美 → 重新处理（最多 1 轮）：见 _residual_retry。
        # 逆解成功的图不重试：逆解是精确物理恢复（已覆盖 MAT），retry 会用生成式
        # MAT 把它重新糊掉；且 gap-score 对逆解恢复的真实背景仍有残余响应（6.png
        # 逆解后 12.0，被判"残留"），据此重试会破坏逆解。
        _residual_retry(active_names, model, skip=inverse_done | sd_done)


def review_lama(names, emit=True, root=None, verify=True):
    output = REVIEW / 'lama-corner-review.png'
    review(LAMA, output.name, names)
    if verify:
        for name in names:
            report_verify(name, verify_repaired(name, root or DEFAULT_ROOT))
    if emit:
        print(output)
    return output


def overwrite_review(names, root, emit=True, verify=True, force=False):
    if not MANIFEST.exists():
        raise SystemExit(
            'overwrite-review: no prepare manifest found in workdir — refusing to '
            'overwrite anything (run prepare first, it records the source md5 used '
            'to verify the overwrite target)')
    manifest = json.loads(MANIFEST.read_text())
    pending = []
    for name in names:
        src = LAMA / name
        dst = Path(root) / name
        if not src.exists():
            raise SystemExit(f'overwrite-review: missing inpainted file {src}')
        expected = manifest.get(name)
        if expected is None or _md5(dst) != expected:
            raise SystemExit(
                f'overwrite-review: {dst} does not match the file processed this run '
                f'(md5 mismatch) — refusing to overwrite. Check that --root points to '
                f'the same folder used by prepare, and that the file was not modified '
                f'in between.')
        pending.append((src, dst))
    # 验证闭环：落盘前逐张客观自检。**逐张判定**——FAIL 的图不落盘（覆盖模式下保持原图、
    # 另存模式下该图不产出），其余图照常写入；`--force` 跳过自检、全部照写。旧行为是
    # "任何一张 FAIL 就整批拒绝、一张都不写"：单张复杂背景的模板残留**误报**会让整批看似
    # "跑了没用、水印还在"（与 Rust `finalize_outputs` 的逐张判定对齐，见 desktop/AGENTS.md）。
    skipped = []
    if verify:
        writable = []
        for src, dst in pending:
            name = dst.name
            report = verify_repaired(name, root)
            report_verify(name, report)
            if report and report['verdict'] == 'FAIL' and not force:
                print(f'skip {name}: failed self-check, kept original '
                      f'(review the candidate image, or use --force)')
                skipped.append(name)
                continue
            writable.append((src, dst))
        pending = writable
    for src, dst in pending:
        copyfile(src, dst)
    output = REVIEW / 'overwritten-corner-review.png'
    if pending:
        review(root, output.name, [dst.name for _, dst in pending])
    if emit:
        print(output)
    if skipped:
        print(f'skipped (kept original, failed self-check): {", ".join(skipped)}')
    return output, skipped


def cleanup(names, root):
    backup = backup_dir(root)
    for name in names:
        backup_file = backup / name
        if backup_file.exists():
            backup_file.unlink()
    if backup.exists() and not any(backup.iterdir()):
        backup.rmdir()
    if WORK.exists():
        rmtree(WORK)


def learn_auto(names, root, label, custom_box=None, any_position=False, ref_short=None):
    """Auto-discover a watermark profile from a set of same-watermark images.

    For every input the watermark box is located with the normal detector (or a
    manual ``--mask-box``), then the frame with the calmest background is picked
    and its coverage solved against an inpainted background. This is what lets a
    new AI tool (e.g. Qwen) reach Doubao-level fidelity without a hand-supplied
    sample: just point it at a few images of that tool's watermark, ideally one
    sitting on a plain/dark area.
    """
    if wprof is None:
        raise SystemExit('profile library unavailable (missing numpy/cv2?)')
    if not label:
        raise SystemExit('learn-auto needs --label <name> (profile id / tool name)')
    import numpy as np

    frames = []
    for name in names:
        path = Path(root) / name
        with Image.open(path) as im:
            pil = im.convert('RGB')
            width, height = pil.size
            arr = np.array(pil)
        if custom_box:
            boxes = [resolve_box(width, height, b) for b in custom_box]
        else:
            boxes = detect_watermark_boxes(pil, extended=any_position)
            if not any_position:
                boxes = [b for b in boxes if b[2] > width - 40 and b[3] > height - 40]
        if not boxes:
            print(f'{name}: no watermark box detected, skipped')
            continue
        x1 = min(b[0] for b in boxes)
        y1 = min(b[1] for b in boxes)
        x2 = max(b[2] for b in boxes)
        y2 = max(b[3] for b in boxes)
        pad = 8
        box = (max(0, x1 - pad), max(0, y1 - pad),
               min(width, x2 + pad), min(height, y2 + pad))
        frames.append((name, str(path), box))
    if not frames:
        raise SystemExit('learn-auto: no watermark found in any input image')
    profile, report = wprof.auto_discover(frames, label=label,
                                          ref_short_side=ref_short)
    print(json.dumps(report, ensure_ascii=False, indent=1))
    if profile is None:
        raise SystemExit('learn-auto: could not build a reliable profile; nothing saved')
    base = wprof.save_profile(profile)
    print(f'saved profile {profile.id} -> {base}')


def run_all(names, custom_box, keep_work, root, model='mat', refine=False, any_position=False, inverse=False, force=False, retry=True, use_profile=True, sd=False, sd_threshold=SD_TEXTURE_MIN):
    prepare(names, custom_box, root, emit=False, model=model, refine=refine,
            any_position=any_position, use_profile=use_profile)
    inpaint(model, inverse=inverse, retry=retry, use_profile=use_profile,
            sd=sd, sd_threshold=sd_threshold)
    candidate_review = review_lama(names, emit=False, root=root)
    final_review, skipped = overwrite_review(names, root, emit=False, force=force)
    skipped_set = set(skipped)
    written = [n for n in names if n not in skipped_set]
    print(f'processed {len(written)} file(s): {", ".join(written)}')
    if skipped:
        print(f'NOT processed (kept original, failed self-check): {", ".join(skipped)}'
              f' — review the candidate images, or re-run with --force to accept them')
    print(f'candidate review: {candidate_review}')
    print(f'final review: {final_review}')
    if keep_work:
        print(f'kept workdir: {WORK}')
    else:
        cleanup(names, root)
        print(f'cleaned: {backup_dir(root)} and {WORK}')


def main():
    parser = argparse.ArgumentParser(description='Remove Doubao or custom text watermark from PNG images.')
    parser.add_argument(
        '--mask-box',
        type=parse_boxes,
        help='custom watermark box(es) as x1,y1,x2,y2[;x1,y1,x2,y2...]; negative '
             'values are relative to right/bottom, e.g. -330,-118,-8,-8',
    )
    parser.add_argument(
        '--root',
        help='target folder containing png files; defaults to this project root',
    )
    parser.add_argument(
        '--keep-work',
        action='store_true',
        help='keep original-watermark-backup and the workdir after the run command for manual review',
    )
    parser.add_argument(
        '--model',
        default='mat',
        choices=['mat', 'lama'],
        help='inpainting model: mat (better structure/shadow recovery, ~2min/image) or lama (fast, ~10s/image); default mat',
    )
    parser.add_argument(
        '--refine',
        action='store_true',
        help='EXPERIMENTAL: refine boxes into stroke-level masks before inpainting. '
             'Works on uniform backgrounds; on complex textures (paper grain, flowers, '
             'highlights) it can miss most strokes, so it is off by default and the '
             'full box + coarse fill + MAT path is used instead.',
    )
    parser.add_argument(
        '--any-position',
        action='store_true',
        help='keep watermark boxes detected anywhere in the image, not only the '
             'bottom-right corner (for text watermarks at arbitrary positions). '
             'Detections already pass text-likeness filters, but busy photos can '
             'still produce false positives, so review the corner review image '
             'before overwriting.',
    )
    parser.add_argument(
        '--inverse',
        dest='inverse',
        action='store_true',
        default=False,
        help='enable stamp inverse where it beats MAT (off by default: on real photos it '
             'can leave dark-glyph/streak artifacts that plain MAT does not)',
    )
    parser.add_argument(
        '--sd',
        dest='sd',
        action='store_true',
        default=False,
        help='enable Stable Diffusion generative inpainting for images whose watermark '
             'sits on a high-frequency background (flowers/foliage), where MAT leaves a '
             'visible smear. OFF by default: it needs a ~4GB model download and costs '
             'minutes per image (M1 8GB: LCM 6 steps ~2-4min vs MAT ~10s), so smooth '
             'backgrounds still use MAT. Route is chosen per image by background texture.',
    )
    parser.add_argument(
        '--sd-threshold',
        type=float,
        default=SD_TEXTURE_MIN,
        help=f'--sd routing threshold: watermark-surround background high-frequency energy '
             f'(default {SD_TEXTURE_MIN:g}). Lower routes more images to SD (slower).',
    )
    parser.add_argument(
        '--no-retry',
        dest='retry',
        action='store_false',
        default=True,
        help='disable the automatic residual retry. DEFAULT ON: when result-level '
             'verification finds leftover watermark after inpainting, the mask is '
             'expanded one level and the model is re-run once (max 1 round); if it is '
             'still not perfect, overwrite-review refuses to write. Use this flag to '
             'keep only the refusing gate without auto-reprocessing.',
    )
    parser.add_argument(
        '--force',
        action='store_true',
        help='overwrite-review only: override the result-level verification gate and '
             'overwrite even when verification FAILS (e.g. mask-outside changes or '
             'detected residual). Use only after manual review — it disables the '
             'scene-independent safety net.',
    )
    parser.add_argument(
        '--no-profile',
        dest='profile',
        action='store_false',
        default=True,
        help='disable watermark profile library matching. DEFAULT ON: if a stored '
             'profile (tools/watermarks/<id>) matches, its alpha map is used as an '
             'exact mask and the watermark is removed by reversing the blend model '
             '(obs = a*C + (1-a)*bg), restoring the true background. No profiles or '
             'no match -> the normal generative pipeline runs unchanged.',
    )
    parser.add_argument(
        '--label',
        help='learn-auto only: human label / profile id for the discovered '
             'watermark, e.g. "qwen" (stored under tools/watermarks/<id>).',
    )
    parser.add_argument(
        '--ref-short',
        type=float,
        help='learn-auto only: reference short side the profile is defined at '
             '(defaults to the selected sample image short side).',
    )
    parser.add_argument('command', choices=['run', 'prepare', 'inpaint', 'review-lama', 'overwrite-review', 'cleanup', 'profiles', 'learn-auto'])
    parser.add_argument('files', nargs='*')
    args = parser.parse_args()
    root = Path(args.root).resolve() if args.root else DEFAULT_ROOT
    if not root.exists():
        raise SystemExit(f'root folder not found: {root}')

    if args.command == 'profiles':
        if wprof is None:
            raise SystemExit('profile library unavailable (missing numpy/cv2?)')
        found = wprof.list_profiles()
        if not found:
            print(f'no profiles in {wprof.PROFILE_DIR}')
        for profile in found:
            print(f'{profile.id}: label={profile.label!r} shape={profile.shape} '
                  f'ref_short={profile.ref_short_side:g} source={profile.source}')
        return

    names = target_names(args.files, root)

    if args.command == 'run':
        run_all(names, args.mask_box, args.keep_work, root, args.model, args.refine,
                args.any_position, args.inverse, force=args.force, retry=args.retry,
                use_profile=args.profile, sd=args.sd, sd_threshold=args.sd_threshold)
    elif args.command == 'prepare':
        prepare(names, args.mask_box, root, model=args.model, refine=args.refine,
                any_position=args.any_position, use_profile=args.profile)
    elif args.command == 'inpaint':
        inpaint(args.model, inverse=args.inverse, retry=args.retry, use_profile=args.profile,
                sd=args.sd, sd_threshold=args.sd_threshold)
    elif args.command == 'review-lama':
        review_lama(names, root=root)
    elif args.command == 'overwrite-review':
        overwrite_review(names, root, force=args.force)
    elif args.command == 'learn-auto':
        learn_auto(names, root, args.label, args.mask_box, args.any_position,
                   args.ref_short)
    elif args.command == 'cleanup':
        cleanup(names, root)


if __name__ == '__main__':
    main()

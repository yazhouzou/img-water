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
# 相对残留判据（触发自动重试）：模板笔画 α mask 只覆盖亮字核心，盖不住豆包水印的
# 暗色描边——低对比背景（4.png 纸面）上 MAT 只重绘笔画区，暗描边残留成字形凹痕。
# 修复后模板分本应大幅下降（高对比图 120→<5，去除率 >95%）；残影图的残留占比很高
# （4.png 43.2→10.7，约 25%），故用相对下降比例而非绝对分兜底：残留 ≥ 原分 20% 且
# ≥ 8（高于已去水印负样本 ≤8.4 的底噪，避免干净图误触发）时重试。绝对阈值 20 只
# 用于 FAIL 判定，低对比残影达不到 20 会被放过（本 bug 的根因）。
TEMPLATE_RESIDUAL_RATIO = 0.2
TEMPLATE_RESIDUAL_FLOOR = 8.0
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
# 逆解 stamp（默认开，--no-inverse 关）：完整水印模型 obs = α·C + (1−α)·bg，α 为覆盖度、
# C 为逐像素颜色（含暗色描边——纯白字模型反解不掉它）。从 4 张同款水印、不同背景
# 的图（黑底/纸面/沙滩/花墙）联立标定。逆解恢复的是**真实背景**（非生成），对复杂
# 纹理背景（花丛类）明显优于 MAT 的平滑重绘；但对低纹理背景（暗底/沙面/纸面）会
# 放大噪声，故用 gating 只在 scale≈1.0 且水印邻域纹理复杂时启用，其余走 MAT。
STAMP_ASSET = Path(__file__).resolve().parent / 'doubao-wm-stamp.npz'
STAMP_REF_SHORT = 1600.0
INVERSE_SCALE_TOL = 0.03
INVERSE_TEXTURE_MIN = 9.0
INVERSE_ALPHA_GAIN_RANGE = (0.8, 1.25)
INVERSE_MAX_GHOST = 0.12


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
    # 顶帽（局部背景扣除）后再匹配：亮背景（纸面/花墙/雪）会压低"笔画-间隙"绝对差，
    # 使 3.png 这类低对比水印漏判（raw max(RGB) gap 仅 15，甚至低于已去水印图的负样本）。
    # 顶帽只保留局部高于背景的亮结构，水印笔画凸显、纹理与光照梯度被抑制，且与背景
    # 亮度无关：实测正样本 ≥33（3.png 33）、负样本 ≤8.4，分离更干净。
    k = max(3, (min(height, width) - 1) | 1)
    k = min(31, k)
    gray = gray - cv2.morphologyEx(
        gray, cv2.MORPH_OPEN, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (k, k)))
    scale = min(height, width) / meta['ref_short_side']
    t = cv2.resize(tpl.astype(np.float32), (0, 0), fx=scale, fy=scale,
                   interpolation=cv2.INTER_NEAREST)
    th_t, tw_t = t.shape
    if th_t >= height or tw_t >= width:
        return None, 0.0, 'template larger than image'
    t01 = (t > 0.5).astype(np.float32)
    ones = np.ones(t.shape, np.float32)
    n_in = float(t01.sum())
    n_out = float(t01.size) - n_in
    s_in = cv2.matchTemplate(gray, t01, cv2.TM_CCORR)
    s_all = cv2.matchTemplate(gray, ones, cv2.TM_CCORR)
    response = s_in / n_in - (s_all - s_in) / n_out
    # 水印必贴右下角：只在右下角 40px 余量窗口内取峰
    y0 = max(0, response.shape[0] - 41)
    x0 = max(0, response.shape[1] - 41)
    window = response[y0:, x0:]
    _, _, _, peak = cv2.minMaxLoc(window)
    px, py = peak[0] + x0, peak[1] + y0
    score = float(response[py, px])
    if score < TEMPLATE_MIN_SCORE:
        return None, score, f'template score {score:.1f} < {TEMPLATE_MIN_SCORE}'
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
        core = (t > 0.5).astype(np.uint8)
        how = 'binary fallback'
    mask = cv2.dilate(core,
                      cv2.getStructuringElement(cv2.MORPH_RECT, TEMPLATE_STROKE_DILATE), 1)
    full = np.zeros((height, width), np.uint8)
    full[py:py + th_t, px:px + tw_t] = mask * 255
    return full, score, f'template matched at ({px},{py}) score {score:.1f} ({how})'


def inverse_apply(obs_path, mat_path, model='mat'):
    """对模板命中的图做完整 stamp 逆解（obs = α·C + (1−α)·bg），返回处理后整图；
    不满足 gating（scale≈1.0 + 水印邻域纹理复杂）或逆解不可靠时返回 None（保持 MAT）。
    逐图自校正 α 增益（最小化残影与 α 的相关性），饱和像素回退 MAT 结果。"""
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
    # 水印邻域纹理复杂度（排除背景过于平滑的场景：MAT 已足够，逆解只会放大噪声）
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
    best = None
    lo, hi = INVERSE_ALPHA_GAIN_RANGE
    for k in np.arange(lo, hi + 1e-6, 0.05):
        a = np.clip(a0 * k, 0, 1)
        a3 = a[..., None]
        inv = np.clip((win - a3 * color) / np.maximum(1 - a3, 1e-3), 0, 255)
        g = cv2.GaussianBlur(inv, (0, 0), 2.5)
        hfm = (inv - g).max(axis=2)
        m = a > 0.03
        if int(m.sum()) < 50:
            continue
        ghost = abs(float(np.corrcoef(hfm[m], a[m])[0, 1]))
        if best is None or ghost < best[0]:
            best = (ghost, inv, a, float(k))
    if best is None or best[0] > INVERSE_MAX_GHOST:
        return None
    _, inv, a, gain = best
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
    hyb = np.where(ill_posed[..., None], mat, inv)
    out = obs.copy()
    sub = out[py:py + th, px:px + tw]
    sub[m] = hyb[m]
    out[py:py + th, px:px + tw] = sub
    return out.astype(np.uint8), (px, py, hf, gain, best[0])


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


def _corner_row_boxes(white, H, W, x0, y0, pad):
    """行块模式：9x3 膨胀两次直接合并字符成行（水印字符与背景亮斑粘连、
    字符级分离失败时——如雪景雪点——仍能定位整行）。防御：
    1) 行框高度上限 6% 图高（排除大面积粘连块，如 1.png 沙滩亮斑 12%）；
    2) 组件必须整体位于 corner 检测区内（排除从区外伸进来的画面内容）；
    3) 文字性验证 + 贴边约束同字符行模式。"""
    import cv2
    merged = cv2.dilate(white, cv2.getStructuringElement(cv2.MORPH_RECT, (9, 3)), iterations=2)
    count, _, stats, _ = cv2.connectedComponentsWithStats(merged, 8)
    boxes = []
    for i in range(1, count):
        x, y, cw, ch, area = (int(v) for v in stats[i])
        gx1, gy1, gx2, gy2 = x0 + x, y0 + y, x0 + x + cw, y0 + y + ch
        if area < 400 or cw < ch or cw / ch > 20:
            continue
        fill = area / float(cw * ch)
        if not (0.15 <= fill <= 0.95):
            continue
        if ch > H * 0.06:
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
    （水印是单行文字；沙滩亮斑高度杂乱不成行，自然排除）。"""
    import cv2
    white = cv2.dilate(white, cv2.getStructuringElement(cv2.MORPH_RECT, (5, 5)))
    count, _, stats, _ = cv2.connectedComponentsWithStats(white, 8)
    chars = []
    for i in range(1, count):
        x, y, cw, ch, area = (int(v) for v in stats[i])
        if ch < H * 0.012 or ch > H * 0.045:
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
        if not backup.exists():
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
    # 模板残留：原图命中模板 → 修复后不应再命中（豆包水印专用判据，非豆包图自动跳过）
    orig_score = res_score = 0.0
    try:
        gray_o = np.array(Image.open(orig_path).convert('RGB')).max(axis=2).astype(np.float32)
        _, orig_score, _ = template_stroke_mask(gray_o, gray_o.shape[1], gray_o.shape[0])
        gray_r = np.array(Image.open(res_path).convert('RGB')).max(axis=2).astype(np.float32)
        _, res_score, _ = template_stroke_mask(gray_r, gray_r.shape[1], gray_r.shape[0])
    except Exception:
        pass
    report['orig_template_score'] = round(float(orig_score), 1)
    report['res_template_score'] = round(float(res_score), 1)
    # 是否做模板残留检查：优先用调用方给的 template_applied；未给则要求
    # scale≈1.0（合成/缩放图上的非豆包水印可能碰巧高分，会误报）
    if template_applied is None:
        scale = min(orig.shape[0], orig.shape[1]) / STAMP_REF_SHORT
        template_applied = (orig_score >= TEMPLATE_MIN_SCORE
                            and abs(scale - 1.0) <= INVERSE_SCALE_TOL)
    # 结构化残留标记：供自动重试逻辑判定"是否因水印残留而不完美"
    # （区别于 mask 外改动——那是工程 bug，重试无法修复）
    report['residual'] = bool(
        template_applied and orig_score >= TEMPLATE_MIN_SCORE
        and (res_score >= TEMPLATE_MIN_SCORE
             or (res_score >= TEMPLATE_RESIDUAL_FLOOR
                 and res_score >= orig_score * TEMPLATE_RESIDUAL_RATIO)))
    verdict = 'PASS'
    if report['outside_changed'] > 0:
        verdict = 'FAIL'
        report['reasons'].append(
            f"mask 外有 {report['outside_changed']} px 被改动 (max {report['outside_max']})")
    if template_applied and orig_score >= TEMPLATE_MIN_SCORE and res_score >= TEMPLATE_MIN_SCORE:
        verdict = 'FAIL'
        report['reasons'].append(
            f'修复后仍匹配豆包模板 (score {res_score:.1f} >= {TEMPLATE_MIN_SCORE:.0f})，疑有残留')
    elif template_applied and orig_score >= TEMPLATE_MIN_SCORE and res_score >= TEMPLATE_MIN_SCORE * 0.6:
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
    tpl_applied = (MASKS / f'{name}.tpl').exists()
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
    """把待重试的图单独放进隔离子目录重跑一次 iopaint（避免整批重复推理），
    结果写回 LAMA。"""
    retry_root = WORK / 'retry'
    sub_src = retry_root / 'source'
    sub_msk = retry_root / 'masks'
    sub_out = retry_root / 'lama'
    if retry_root.exists():
        rmtree(retry_root)
    for path in (sub_src, sub_msk, sub_out):
        path.mkdir(parents=True, exist_ok=True)
    for name in names:
        copyfile(SOURCE / name, sub_src / name)
        copyfile(MASKS / name, sub_msk / name)
    subprocess.run(
        [
            str(IOPAINT),
            'run',
            '--model',
            model,
            '--device',
            DEVICE,
            '--image',
            str(sub_src),
            '--mask',
            str(sub_msk),
            '--output',
            str(sub_out),
        ],
        check=True,
    )
    for name in names:
        copyfile(sub_out / name, LAMA / name)


def _residual_retry(names, model):
    """对残留图扩 mask 并用 MAT 重跑复验；仍残留则打印 FAIL（交给 overwrite-review 拒绝落盘）。"""
    candidates = [name for name in names if _residual_state(name)[0]]
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


def inpaint(model='mat', inverse=True, retry=True, use_profile=True):
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
    subprocess.run(
        [
            str(IOPAINT),
            'run',
            '--model',
            model,
            '--device',
            DEVICE,
            '--image',
            str(SOURCE),
            '--mask',
            str(MASKS),
            '--output',
            str(LAMA),
        ],
        check=True,
    )
    if inverse:
        # inverse（默认开，逐图自动择优）：模板命中且纹理复杂的 scale≈1.0 图用
        # 完整 stamp 逆解恢复真实背景（覆盖 MAT 结果）；其余图 gating 判定后保持
        # MAT。见 inverse_apply gating。
        applied = []
        for sidecar in sorted(MASKS.glob('*.tpl')):
            name = sidecar.stem
            obs_path, mat_path = SOURCE / name, LAMA / name
            if not obs_path.exists() or not mat_path.exists():
                continue
            res = inverse_apply(obs_path, mat_path, model)
            if res is None:
                print(f'{name}: inverse skipped (gating), keep MAT')
                continue
            out, (px, py, hf, gain, ghost) = res
            Image.fromarray(out).save(mat_path)
            applied.append(f'{name} (pos {px},{py} hf {hf:.1f} gain {gain:.2f} ghost {ghost:.3f})')
        if applied:
            print('inverse stamp applied: ' + '; '.join(applied))
    if inverse and use_profile and wprof is not None:
        # 档案库逆解：对命中档案的图用其 α/C 恢复真实背景（覆盖 MAT 结果）。
        applied = []
        for sidecar in sorted(MASKS.glob('*.wprof')):
            name = sidecar.stem
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
            applied.append(f'{name} ({detail})')
        if applied:
            print('profile inverse applied: ' + '; '.join(applied))
    if retry:
        # 自校验不完美 → 重新处理（最多 1 轮）：见 _residual_retry
        _residual_retry(active_names, model)


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
    # 验证闭环：落盘前客观自检，FAIL 拒绝覆盖（--force 强制），WARN 提示
    if verify:
        failures = []
        for name in names:
            report = verify_repaired(name, root)
            report_verify(name, report)
            if report and report['verdict'] == 'FAIL':
                failures.append(name)
        if failures and not force:
            raise SystemExit(
                'overwrite-review: verification FAILED for ' + ', '.join(failures)
                + ' — refusing to overwrite (use --force to override, or review manually)')
    for src, dst in pending:
        copyfile(src, dst)
    output = REVIEW / 'overwritten-corner-review.png'
    review(root, output.name, names)
    if emit:
        print(output)
    return output


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


def run_all(names, custom_box, keep_work, root, model='mat', refine=False, any_position=False, inverse=True, force=False, retry=True, use_profile=True):
    prepare(names, custom_box, root, emit=False, model=model, refine=refine,
            any_position=any_position, use_profile=use_profile)
    inpaint(model, inverse=inverse, retry=retry, use_profile=use_profile)
    candidate_review = review_lama(names, emit=False, root=root)
    final_review = overwrite_review(names, root, emit=False, force=force)
    print(f'processed {len(names)} file(s): {", ".join(names)}')
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
        default=True,
        help='DEFAULT ON: per-image auto selection. For template-matched scale~1.0 '
             'images with complex texture (e.g. flowers), recover the real background '
             'under the watermark via the calibrated full stamp model '
             '(obs = a*C + (1-a)*bg) instead of MAT generation; other images '
             '(smooth backgrounds, scaled images, saturated pixels) automatically '
             'keep MAT. Restores real texture but can amplify noise on smooth '
             'backgrounds, hence gated. Use --no-inverse to force MAT everywhere.',
    )
    parser.add_argument(
        '--no-inverse',
        dest='inverse',
        action='store_false',
        help='disable auto inverse and force MAT for every image',
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
                use_profile=args.profile)
    elif args.command == 'prepare':
        prepare(names, args.mask_box, root, model=args.model, refine=args.refine,
                any_position=args.any_position, use_profile=args.profile)
    elif args.command == 'inpaint':
        inpaint(args.model, inverse=args.inverse, retry=args.retry, use_profile=args.profile)
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

#!/usr/bin/env python3
import argparse
import os
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
TEMPLATE_META = TEMPLATE_ASSET.with_suffix('.json')
# gap-score（笔画区均亮 - 间隙区均亮）实测：黑底 144 / 雪景 102-136 / 花墙 57 /
# 沙滩 62；纸面低对比 15 回退整框检测。阈值取中间空档。
TEMPLATE_MIN_SCORE = 40.0
# 膨胀核按模型区分：
# - MAT（配粗填）：7x7（±3px）。粗填（TELEA 从 mask 边界插值）已把 mask 区填成
#   背景延续、消除字形上下文，±3px 只需盖住抗锯齿带——最小化 mask 才能最大
#   保留字符间隙里的真实画面（6.png 红棕带/花瓣：19x11 时修改面积 14506px、
#   阴影丢失 6047px；7x7 降至 10707/4866，花丛形态与原图高度一致）。
#   直接 MAT（无粗填）下 ±3px 会字形复活，勿去掉粗填。
# - LaMa（无粗填）：19x11（水平 ±9 填满字符间隙防"见字生字"，垂直 ±5 盖抗锯齿）。
TEMPLATE_DILATE_MAT = (7, 7)
TEMPLATE_DILATE_LAMA = (19, 11)


def load_template():
    """加载笔画级水印模板（黑底图提取），返回 (tpl_bool, meta) 或 (None, None)。"""
    if not TEMPLATE_ASSET.exists() or not TEMPLATE_META.exists():
        return None, None
    import json

    import numpy as np

    tpl = np.array(Image.open(TEMPLATE_ASSET).convert('L')) > 127
    meta = json.loads(TEMPLATE_META.read_text())
    return tpl, meta


def template_stroke_mask(gray, width, height, model='mat'):
    """在右下角窗口内用模板做 gap-score 匹配（0/1 模板核取 S_in、全 1 核取窗口和，
    gap = S_in/N_in − S_out/N_out），返回 (mask_uint8, score, info)。
    模板按图片短边比例缩放（豆包水印随短边等比）。膨胀核按模型区分：
    MAT 配粗填用 7x7（最小侵入），LaMa 无粗填用 19x11（连片防字形复活）。
    分数低于阈值返回 (None, score, info) 交给整框检测回退。"""
    try:
        import cv2
        import numpy as np
    except ImportError:
        raise SystemExit('missing cv2/numpy; run ./tools/ensure-inpaint-env.sh first')
    tpl, meta = load_template()
    if tpl is None:
        return None, 0.0, 'template asset missing'
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
    kernel = TEMPLATE_DILATE_MAT if model == 'mat' else TEMPLATE_DILATE_LAMA
    mask = cv2.dilate((t > 0.5).astype(np.uint8),
                      cv2.getStructuringElement(cv2.MORPH_RECT, kernel), 1)
    full = np.zeros((height, width), np.uint8)
    full[py:py + th_t, px:px + tw_t] = mask * 255
    return full, score, f'template matched at ({px},{py}) score {score:.1f}'



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


def detect_watermark_boxes(image):
    """两级检测：
    1) 全图扫纯白文字（≥248），用“文字性特征”过滤画面主体误检：组件内原始白像素
       填充率 ≤0.6 且 x 投影列段数 ≥3（实心块如灯罩 fill 0.8+、段数 1，文字水印
       fill ~0.2、段数=字符数）；
    2) 右下角自适应阈值兜底（识别半透明/灰白粗体水印），与第 1 级合并去重。"""
    full = _detect_full_white(image)
    corner = _detect_corner_faded(image)
    return full + [b for b in corner if not any(_overlap(b, f) for f in full)]


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
    kernel = np.ones((3, 9), dtype=np.uint8)
    merged = white.astype(np.uint8)
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
        fill_raw, segments = _text_likeness(white, x1, y1, x2, y2)
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
    kernel = TEMPLATE_DILATE_MAT if model == 'mat' else TEMPLATE_DILATE_LAMA
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


def prepare(names, custom_box, root, emit=True, model='mat', refine=False):
    if WORK.exists():
        rmtree(WORK)
    ensure_work_dirs(root)
    backup_root = backup_dir(root)
    for name in names:
        current = Path(root) / name
        backup = backup_root / name
        if not backup.exists():
            copyfile(current, backup)
        copyfile(backup, SOURCE / name)

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
            tpl_mask, tpl_score, tpl_info = template_stroke_mask(gray, width, height, model)
            if tpl_mask is not None:
                # 模板笔画 mask：精确贴笔画+小膨胀，复杂背景（书本/花丛）
                # 也不会整块重绘
                mask = Image.fromarray(tpl_mask)
                (MASKS / f'{name}.tpl').write_text('')
                print(f'{name}: template mask ({tpl_info})')
            else:
                with Image.open(backup) as probe:
                    boxes = detect_watermark_boxes(probe)
                    # 豆包水印必贴右下角：丢弃远离右下角的检出框，
                    # 否则雪景白点/白墙/栏杆等画面内容会被误检硬修（毁图）
                    pw, ph = probe.size
                    boxes = [b for b in boxes if b[2] > pw - 40 and b[3] > ph - 40]
                if boxes:
                    print(f'{name}: auto-detected {len(boxes)} watermark box(es) {boxes} (template fallback: {tpl_info})')
                else:
                    # 检测不到水印：写全空 mask 跳过修复，绝不用默认规则硬修——
                    # 对已无水印的图硬修会把真实画面重绘成模糊块
                    skipped = True
                    print(f'{name}: no watermark detected, skipped (template fallback: {tpl_info})')
        else:
            boxes = [resolve_box(width, height, custom_box)]
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

    output = REVIEW / 'source-corner-review.png'
    review(SOURCE, output.name, names)
    if emit:
        print(output)
    return output

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


def inpaint(model='mat'):
    if not IOPAINT.exists():
        raise SystemExit('missing project env; run tools/ensure-inpaint-env.sh (or .ps1 on Windows) first')
    # 粗填预处理：先把 mask 区域用周围背景插值填充（TELEA），再交给修复模型精修。
    # 直接修复时模型会"延续"水印的白色笔画（深色背景场景生成白色伪块）；
    # 粗填后模型看到的是中性底色，生成纹理与周围更协调。
    try:
        import cv2
        import numpy as np
    except ImportError:
        raise SystemExit('missing cv2/numpy in project env')
    for mask_path in sorted(MASKS.glob('*.png')):
        name = mask_path.name
        mask = np.array(Image.open(mask_path).convert('L'))
        if mask.max() == 0:
            # 空 mask（未检测到水印）的图直接原样通过，不进模型
            copyfile(SOURCE / name, LAMA / name)
            (SOURCE / name).unlink()
            mask_path.unlink()
            print(f'{name}: empty mask, passed through without inpainting')
            continue
        img = np.array(Image.open(SOURCE / name).convert('RGB'))
        if model == 'lama' and (MASKS / f'{name}.tpl').exists():
            # LaMa + 模板 mask：不做 TELEA 粗填——LaMa 会把粗填的模糊底色
            # 延续成白色伪块（深色背景场景）
            pass
        else:
            # MAT：一律先粗填。TELEA 从 mask 边界真实背景插值出结构底色
            #（如 6.png 花墙交界红棕带的走向），MAT 在底色上精修出锐利细节——
            # 粗填+MAT 的宏观结构与原图一致性显著优于 direct MAT
            #（6.png "AI" 附近红棕带：direct 断裂发暗，粗填后连续贯穿）。
            # LaMa + 整框 mask 仍走粗填（均匀背景场景验证更优）。
            coarse = cv2.inpaint(img, mask, 7, cv2.INPAINT_TELEA)
            Image.fromarray(coarse).save(SOURCE / name)
    # 全部透传后 source 为空：无需进模型，直接结束（避免 iopaint 空目录报错）
    if not any(SOURCE.iterdir()):
        print('all images passed through: no watermark to remove')
        return
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


def review_lama(names, emit=True):
    output = REVIEW / 'lama-corner-review.png'
    review(LAMA, output.name, names)
    if emit:
        print(output)
    return output


def overwrite_review(names, root, emit=True):
    for name in names:
        copyfile(LAMA / name, Path(root) / name)
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


def run_all(names, custom_box, keep_work, root, model='mat', refine=False):
    prepare(names, custom_box, root, emit=False, model=model, refine=refine)
    inpaint(model)
    candidate_review = review_lama(names, emit=False)
    final_review = overwrite_review(names, root, emit=False)
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
        type=parse_box,
        help='custom watermark box as x1,y1,x2,y2; negative values are relative to right/bottom, e.g. -330,-118,-8,-8',
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
    parser.add_argument('command', choices=['run', 'prepare', 'inpaint', 'review-lama', 'overwrite-review', 'cleanup'])
    parser.add_argument('files', nargs='*')
    args = parser.parse_args()
    root = Path(args.root).resolve() if args.root else DEFAULT_ROOT
    if not root.exists():
        raise SystemExit(f'root folder not found: {root}')
    names = target_names(args.files, root)

    if args.command == 'run':
        run_all(names, args.mask_box, args.keep_work, root, args.model, args.refine)
    elif args.command == 'prepare':
        prepare(names, args.mask_box, root, model=args.model, refine=args.refine)
    elif args.command == 'inpaint':
        inpaint(args.model)
    elif args.command == 'review-lama':
        review_lama(names)
    elif args.command == 'overwrite-review':
        overwrite_review(names, root)
    elif args.command == 'cleanup':
        cleanup(names, root)


if __name__ == '__main__':
    main()

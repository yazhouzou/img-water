"""Generic, self-learning watermark profile library.

A watermark profile models a fixed, semi-transparent overlay: a per-pixel
coverage map (``alpha``, 0..1) and a per-pixel color (``C``, RGB), defined at a
reference image short side so it can be rescaled and relocated on any image.
With a profile the watermark is removed by *reversing the blending equation*

    observed = alpha * C + (1 - alpha) * background

which restores the true background instead of letting a generative model redraw
it, so pixels the watermark never touched stay byte-identical.

Profiles live under ``tools/watermarks/<id>/`` as ``alpha.png`` (8-bit
coverage), ``color.png`` (8-bit RGB) and ``meta.json``.

Two ways to build a profile:

* :func:`extract_from_solid` - from one sample on a known solid background
  (e.g. the tool's watermark rendered on a pure black image).
* :func:`learn_from_batch` - unsupervised, from several images of the *same*
  watermark at the *same* position over different backgrounds; per-image
  coverage estimates are median-combined, cancelling most background noise.
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from pathlib import Path

PROFILE_DIR = Path(__file__).resolve().parent / 'watermarks'
ALPHA_FILENAME = 'alpha.png'
COLOR_FILENAME = 'color.png'
META_FILENAME = 'meta.json'

DEFAULT_REF_SHORT = 1600.0
MASK_ALPHA_THRESHOLD = 8
MASK_DILATE = (3, 3)
INVERSE_ALPHA_THRESHOLD = 0.03
INVERSE_GAIN_RANGE = (0.8, 1.25)
INVERSE_MAX_GHOST = 0.6
# Model-fit gate: after the best inverse, how well does obs = a*C + (1-a)*bg
# explain the observations, using the MAT result as the background? A profile
# that assumes a single colour C (white) cannot model a watermark that also has
# a dark outline, so its fit is bad and we keep MAT instead of imprinting an
# outline. Measured: correct solid profiles ~<8, outline-y watermarks ~15+.
INVERSE_MAX_FIT = 14.0
# NCC scale search: how far the watermark may deviate from the profile's
# reference size and still be localised. The old +-10% only covered proportional
# rescaling within one output tier; some tools re-render the watermark at a
# different size per tier, so the search is widened to +-35% and run coarse to
# fine (a wide coarse sweep for the size, a narrow fine refinement for subpixel
# placement). Localisation is cheap; a wrong size still fails the inverse gate,
# but at least the mask lands at the right size for the generative fallback.
NCC_SCALE_SPAN = 0.35
NCC_COARSE_STEP = 0.05
NCC_FINE_SPAN = 0.05
NCC_FINE_STEP = 0.0125


@dataclass
class Profile:
    id: str
    alpha: 'object'
    color: 'object'
    ref_short_side: float = DEFAULT_REF_SHORT
    label: str = ''
    source: str = ''
    created: str = ''
    extra: dict = field(default_factory=dict)

    @property
    def shape(self):
        return self.alpha.shape


def _np():
    try:
        import numpy as np
    except ImportError as exc:  # pragma: no cover
        raise SystemExit('missing numpy; run ./tools/ensure-inpaint-env.sh first') from exc
    return np


def _cv2():
    try:
        import cv2
    except ImportError as exc:  # pragma: no cover
        raise SystemExit('missing cv2; run ./tools/ensure-inpaint-env.sh first') from exc
    return cv2


def _resize(arr, width, height, nearest=False):
    cv2 = _cv2()
    interp = cv2.INTER_NEAREST if nearest else cv2.INTER_LINEAR
    if arr.ndim == 2:
        return cv2.resize(arr, (width, height), interpolation=interp)
    return cv2.resize(arr, (width, height), interpolation=interp)


def profile_path(pid):
    return PROFILE_DIR / pid


def list_profile_ids():
    if not PROFILE_DIR.exists():
        return []
    return sorted(p.name for p in PROFILE_DIR.iterdir() if (p / META_FILENAME).exists())


def load_profile(pid):
    import numpy as _n
    from PIL import Image

    base = profile_path(pid)
    meta_path = base / META_FILENAME
    if not meta_path.exists():
        raise SystemExit(f'profile not found: {pid}')
    meta = json.loads(meta_path.read_text())
    alpha = _n.array(Image.open(base / ALPHA_FILENAME).convert('L')).astype(_n.float32) / 255.0
    color = _n.array(Image.open(base / COLOR_FILENAME).convert('RGB')).astype(_n.float32)
    return Profile(
        id=meta.get('id', pid),
        alpha=alpha,
        color=color,
        ref_short_side=float(meta.get('ref_short_side', DEFAULT_REF_SHORT)),
        label=meta.get('label', ''),
        source=meta.get('source', ''),
        created=meta.get('created', ''),
        extra=meta.get('extra', {}),
    )


def list_profiles():
    out = []
    for pid in list_profile_ids():
        try:
            out.append(load_profile(pid))
        except SystemExit:
            continue
    return out


def save_profile(profile, overwrite=True):
    import numpy as _n
    from PIL import Image

    base = profile_path(profile.id)
    if base.exists() and not overwrite:
        raise SystemExit(f'profile already exists: {profile.id}')
    base.mkdir(parents=True, exist_ok=True)
    alpha_u8 = _n.clip(profile.alpha * 255.0, 0, 255).astype(_n.uint8)
    color_u8 = _n.clip(profile.color, 0, 255).astype(_n.uint8)
    Image.fromarray(alpha_u8).save(base / ALPHA_FILENAME)
    Image.fromarray(color_u8).save(base / COLOR_FILENAME)
    meta = {
        'id': profile.id,
        'label': profile.label,
        'ref_short_side': profile.ref_short_side,
        'source': profile.source,
        'created': profile.created or time.strftime('%Y-%m-%d %H:%M:%S'),
        'extra': profile.extra,
    }
    (base / META_FILENAME).write_text(json.dumps(meta, ensure_ascii=False, indent=1))
    return base


def _crop_to_alpha(alpha, color, pad=2):
    np = _np()
    ys, xs = np.where(alpha > 0)
    if len(xs) == 0:
        raise SystemExit('empty alpha profile; nothing to crop')
    x0, x1 = max(0, xs.min() - pad), min(alpha.shape[1], xs.max() + pad + 1)
    y0, y1 = max(0, ys.min() - pad), min(alpha.shape[0], ys.max() + pad + 1)
    return alpha[y0:y1, x0:x1], color[y0:y1, x0:x1], (int(x0), int(y0), int(x1), int(y1))


def _clean_alpha(alpha, thr=0.02, min_area=24):
    cv2 = _cv2()
    np = _np()
    core = (alpha > thr).astype(np.uint8)
    n, labels, stats, _ = cv2.connectedComponentsWithStats(core)
    keep = np.zeros_like(core)
    for i in range(1, n):
        if stats[i, cv2.CC_STAT_AREA] >= min_area:
            keep[labels == i] = 1
    return alpha * keep


def extract_from_solid(path, box=None, bg=(0, 0, 0), color=(255, 255, 255),
                       ref_short_side=None, label='', min_area=24):
    """Build a profile from a watermark sample rendered on a known solid bg.

    A single solid sample cannot separate coverage from color (``obs = a*C``),
    so the watermark ``color`` must be supplied - default white, which is right
    for the common white/grey-white AI watermark. Coverage is then solved from
    ``obs = a*C + (1-a)*bg`` per channel and averaged.
    """
    from PIL import Image

    np = _np()
    obs = np.array(Image.open(path).convert('RGB')).astype(np.float32)
    height, width = obs.shape[:2]
    if box is None:
        box = (0, 0, width, height)
    x1, y1, x2, y2 = [int(v) for v in box]
    x1, y1 = max(0, x1), max(0, y1)
    x2, y2 = min(width, x2), min(height, y2)
    if x2 - x1 < 2 or y2 - y1 < 2:
        raise SystemExit(f'invalid box: {box}')
    win = obs[y1:y2, x1:x2]
    bg_arr = np.array(bg, dtype=np.float32).reshape(1, 1, 3)
    c_arr = np.array(color, dtype=np.float32).reshape(1, 1, 3)
    if float((c_arr - bg_arr).max()) < 8:
        raise SystemExit('watermark color and background are too close')
    a_ch = (win - bg_arr) / np.maximum(c_arr - bg_arr, 1e-3)
    alpha = np.clip(a_ch.mean(2), 0, 1)
    alpha = _clean_alpha(alpha, min_area=min_area)
    color_map = np.broadcast_to(c_arr, win.shape).copy()
    alpha, color_map, sbox = _crop_to_alpha(alpha, color_map)
    ref = float(ref_short_side or DEFAULT_REF_SHORT)
    return Profile(
        id=_slug(label or Path(path).stem),
        alpha=alpha,
        color=color_map,
        ref_short_side=ref,
        label=label,
        source=f'{path} box={box} bg={tuple(int(v) for v in bg)} '
               f'color={tuple(int(v) for v in color)}',
        extra={'stroke_box': list(sbox)},
    )


def learn_from_batch(paths, box=None, ref_short_side=None, label='',
                     min_area=24, max_resid=12.0):
    """Unsupervised profile learning from several aligned watermark samples.

    All images must share the same size and have the watermark at the same
    position. Per-image coverage is estimated against a local (median) background
    and median-combined across images. Returns ``(profile, report)`` or
    ``(None, report)`` when the batch is too small/inconsistent to trust.
    """
    from PIL import Image

    np = _np()
    cv2 = _cv2()
    if len(paths) < 3:
        return None, {'reason': f'need >=3 aligned samples, got {len(paths)}'}
    obs_list = [np.array(Image.open(p).convert('RGB')).astype(np.float32) for p in paths]
    shape = obs_list[0].shape
    if any(o.shape != shape for o in obs_list):
        return None, {'reason': 'images differ in size; align them first'}
    height, width = shape[:2]
    if box is None:
        box = _guess_corner_box(width, height)
    x1, y1, x2, y2 = [int(v) for v in box]
    x1, y1 = max(0, x1), max(0, y1)
    x2, y2 = min(width, x2), min(height, y2)
    sub = [o[y1:y2, x1:x2] for o in obs_list]
    box_h = y2 - y1
    k = min(31, max(15, (box_h // 3) | 1))
    mask = np.zeros((height, width), np.uint8)
    for o in obs_list:
        l = o.max(2)
        bgm = cv2.medianBlur(l.astype(np.uint8), k).astype(np.float32)
        mask = np.maximum(mask, ((l - bgm) > 12).astype(np.uint8))
    mask = cv2.dilate(mask, np.ones((3, 3), np.uint8), 1)
    mask_box = mask[y1:y2, x1:x2].astype(bool)
    if int(mask_box.sum()) < 50:
        return None, {'reason': 'no spatial watermark structure found in batch'}
    bg_hat = []
    for o in obs_list:
        filled = cv2.inpaint(o.astype(np.uint8), mask, 4, cv2.INPAINT_TELEA)
        bg_hat.append(filled.astype(np.float32)[y1:y2, x1:x2])
    obs_box = np.stack(sub, 0)
    bg_box = np.stack(bg_hat, 0)
    lum_obs = obs_box.max(3)
    lum_bg = bg_box.max(3)
    mo = lum_obs.mean(0)
    mb = lum_bg.mean(0)
    var = ((lum_bg - mb) ** 2).mean(0)
    cov = ((lum_bg - mb) * (lum_obs - mo)).mean(0)
    alpha = np.clip(1 - cov / np.maximum(var, 1e-3), 0, 1)
    active = (var > 4.0) & mask_box
    alpha = np.where(active & (alpha > 0.3), alpha, 0.0)
    core = alpha > 0.35
    if int(core.sum()) < 50:
        return None, {'reason': 'not enough separable watermark coverage across batch'}
    acc = np.zeros_like(alpha)
    for _ in sub:
        acc += core
    color = np.zeros_like(sub[0])
    for c in range(3):
        num = np.zeros_like(alpha)
        for i in range(len(sub)):
            num += (obs_box[i][..., c] - (1 - alpha) * bg_box[i][..., c]) * core
        color[..., c] = num / np.maximum(acc * np.maximum(alpha, 1e-3), 1e-3)
    color = np.clip(color, 0, 255)
    resid = 0.0
    for i in range(len(sub)):
        hat = alpha[..., None] * color + (1 - alpha[..., None]) * bg_box[i]
        resid += np.abs(hat - obs_box[i]).mean(2)
    resid = float((resid / len(sub))[core].mean())
    coverage = float((alpha > 0.3).mean())
    report = {
        'reason': 'ok',
        'samples': len(paths),
        'coverage': round(coverage, 4),
        'residual': round(resid, 3),
        'active': round(float(active.mean()), 4),
        'box': [x1, y1, x2, y2],
    }
    if resid > max_resid:
        report['reason'] = f'reconstruction residual {resid:.2f} too high'
        return None, report
    if not (0.003 <= coverage <= 0.6):
        report['reason'] = f'coverage {coverage:.3f} outside sane range'
        return None, report
    alpha = _clean_alpha(alpha, thr=0.08, min_area=min_area)
    color[alpha <= 0.03] = 0
    alpha_c, color_c, sbox = _crop_to_alpha(alpha, color)
    report['stroke_box'] = list(sbox)
    ref = float(ref_short_side or DEFAULT_REF_SHORT)
    profile = Profile(
        id=_slug(label or 'learned'),
        alpha=alpha_c,
        color=color_c,
        ref_short_side=ref,
        label=label,
        source='learn_from_batch: ' + ', '.join(Path(p).name for p in paths),
        extra={'stroke_box': list(sbox), 'learn_report': report},
    )
    return profile, report


def _resolve_box(box, width, height):
    """Normalise a box; negative values count from the right/bottom edge."""
    if box is None:
        return _guess_corner_box(width, height)
    x1, y1, x2, y2 = [int(v) for v in box]
    if x1 < 0:
        x1 = width + x1
    if y1 < 0:
        y1 = height + y1
    if x2 < 0:
        x2 = width + x2
    if y2 < 0:
        y2 = height + y2
    x1, y1 = max(0, x1), max(0, y1)
    x2, y2 = min(width, x2), min(height, y2)
    return (x1, y1, x2, y2)


def _stroke_mask(win, sat_max=60, rel=1.8, min_area=6):
    """Bright low-saturation watermark strokes inside a window.

    Top-hat (bright) response with a local mean+rel*std threshold, matching the
    corner-detection heuristic used elsewhere: grey/white overlays are the
    low-saturation structures lighter than their surroundings, while colourful
    texture (petals, foliage) is rejected by the saturation gate.
    """
    cv2 = _cv2()
    np = _np()
    win = win.astype(np.float32)
    gray = win.max(2)
    sat = win.max(2) - win.min(2)
    k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (31, 31))
    opened = cv2.morphologyEx(gray.astype(np.uint8), cv2.MORPH_OPEN, k).astype(np.float32)
    diff = gray - opened
    w = 96
    mean = cv2.boxFilter(diff, -1, (w, w))
    sq = cv2.boxFilter(diff * diff, -1, (w, w))
    std = np.sqrt(np.maximum(sq - mean * mean, 0.0))
    thr = np.maximum(10.0, mean + rel * std)
    mask = ((diff >= thr) & (sat <= sat_max)).astype(np.uint8)
    n, labels, stats, _ = cv2.connectedComponentsWithStats(mask, 8)
    keep = np.zeros_like(mask)
    for i in range(1, n):
        if stats[i, cv2.CC_STAT_AREA] >= min_area:
            keep[labels == i] = 1
    return keep


def _detect_watermark_box(rgb, corner_frac=(0.60, 0.82), pad=10):
    """Lightweight bottom-right watermark box detector (for the standalone CLI).

    Returns ``(x1, y1, x2, y2)`` or ``None``. The main pipeline has a richer
    detector; this one keeps the profile library self-sufficient.
    """
    np = _np()
    h, w = rgb.shape[:2]
    stroke = _stroke_mask(rgb.astype(np.float32))
    x0, y0 = int(w * corner_frac[0]), int(h * corner_frac[1])
    sub = stroke[y0:, x0:]
    ys, xs = np.where(sub > 0)
    if len(xs) < 30:
        return None
    x1 = max(0, int(xs.min()) + x0 - pad)
    y1 = max(0, int(ys.min()) + y0 - pad)
    x2 = min(w, int(xs.max()) + x0 + pad + 1)
    y2 = min(h, int(ys.max()) + y0 + pad + 1)
    return (x1, y1, x2, y2)


def background_score(rgb, box=None, color=(255, 255, 255)):
    """Score how usable the background under the watermark is as a sample.

    The strokes are inpainted to estimate the clean background; the closer that
    estimate is to a constant, the more trustworthy the single-frame coverage
    solve is (lower ``uniformity`` == better).

    ``contrast`` is how far the background sits from the watermark colour: a
    uniform *white* background is a terrible sample for a white watermark
    because ``C - bg`` is tiny and the coverage solve divides by nothing. A dark
    frame has both low uniformity *and* high contrast, which is exactly why the
    Doubao alpha came from the near-black ``2.png``.

    Returns ``None`` when no strokes are found.
    """
    cv2 = _cv2()
    np = _np()
    obs = rgb.astype(np.float32)
    h, w = obs.shape[:2]
    x1, y1, x2, y2 = _resolve_box(box, w, h)
    if x2 - x1 < 4 or y2 - y1 < 4:
        return None
    win = obs[y1:y2, x1:x2]
    stroke = _stroke_mask(win)
    if int(stroke.sum()) < 20:
        return None
    filled = cv2.inpaint(win.astype(np.uint8), stroke * 255, 4, cv2.INPAINT_TELEA)
    bg_hat = filled.astype(np.float32)
    med = np.median(bg_hat.reshape(-1, 3), axis=0)
    uniformity = float(np.mean(np.abs(bg_hat - med.reshape(1, 1, 3))))
    contrast = float(np.mean(np.array(color, np.float32) - med))
    return {
        'box': [x1, y1, x2, y2],
        'short': min(w, h),
        'size': [w, h],
        'coverage': float((stroke > 0).mean()),
        'bg_median': [int(v) for v in med],
        'uniformity': uniformity,
        'contrast': contrast,
    }


def build_from_uniform(path, box=None, color=(255, 255, 255), ref_short_side=None,
                       label='', min_area=24, bg=None, max_resid=12.0):
    """Build a profile from one sample whose background is (near) uniform.

    The background under the strokes is estimated by inpainting them, then
    coverage is solved from ``obs = a*C + (1-a)*bg``. A *single* uniform sample
    is enough because ``bg`` is well approximated by its inpaint; the watermark
    ``color`` must still be supplied (white by default) since alpha and C cannot
    be separated from one frame otherwise.

    Returns ``(profile, report)`` or ``(None, report)``.
    """
    from PIL import Image

    cv2 = _cv2()
    np = _np()
    with Image.open(path) as im:
        rgb = im.convert('RGB')
        width, height = rgb.size
        obs = np.array(rgb).astype(np.float32)
    x1, y1, x2, y2 = _resolve_box(box, width, height)
    if x2 - x1 < 4 or y2 - y1 < 4:
        return None, {'reason': f'invalid box: {box}'}
    win = obs[y1:y2, x1:x2]
    if bg is None:
        stroke = _stroke_mask(win)
        if int(stroke.sum()) < 20:
            return None, {'reason': 'no watermark strokes found in box'}
        filled = cv2.inpaint(win.astype(np.uint8), stroke * 255, 4, cv2.INPAINT_TELEA)
        bg_map = filled.astype(np.float32)
    else:
        bg_map = np.broadcast_to(np.array(bg, np.float32).reshape(1, 1, 3), win.shape).copy()
    c_arr = np.array(color, np.float32).reshape(1, 1, 3)
    denom = c_arr - bg_map
    if float(np.median(denom)) < 8:
        return None, {'reason': 'background too close to watermark color'}
    a_ch = (win - bg_map) / np.maximum(denom, 1e-3)
    alpha = np.clip(a_ch.mean(2), 0, 1)
    alpha = _clean_alpha(alpha, min_area=min_area)
    core = alpha > 0.3
    if int(core.sum()) < 20:
        return None, {'reason': 'not enough separable watermark coverage'}
    hat = alpha[..., None] * c_arr + (1 - alpha[..., None]) * bg_map
    resid = float(np.abs(hat - win).mean(2)[core].mean())
    report = {
        'reason': 'ok',
        'residual': round(resid, 3),
        'coverage': round(float(core.mean()), 4),
        'box': [x1, y1, x2, y2],
        'bg_median': [int(v) for v in np.median(bg_map.reshape(-1, 3), 0)],
    }
    if resid > max_resid:
        report['reason'] = f'reconstruction residual {resid:.2f} too high'
        return None, report
    color_map = np.broadcast_to(c_arr, win.shape).copy()
    alpha_c, color_c, sbox = _crop_to_alpha(alpha, color_map)
    ref = float(ref_short_side or min(width, height))
    short = min(width, height)
    # Deterministic bottom-right anchor: lets a profile be placed without the
    # (contrast-sensitive) gap-score matcher when the tool's overlay position is
    # fixed, e.g. a grey watermark on a pale background where matching is weak.
    place = {
        'ref': short,
        'right': (width - (x1 + sbox[0]) - alpha_c.shape[1]) / short,
        'bottom': (height - (y1 + sbox[1]) - alpha_c.shape[0]) / short,
    }
    profile = Profile(
        id=_slug(label or Path(path).stem),
        alpha=alpha_c,
        color=color_c,
        ref_short_side=ref,
        label=label,
        source=f'auto: {Path(path).name} box={[x1, y1, x2, y2]} '
               f'bg_median={report["bg_median"]} color={tuple(int(v) for v in color)}',
        extra={'stroke_box': list(sbox), 'learn_report': report,
               'bg_median': report['bg_median'], 'place': place},
    )
    return profile, report


def extract_from_pair(path_black, path_white, box=None, ref_short_side=None,
                      label='', min_area=24, max_resid=6.0):
    """Exact per-pixel profile from a near-black / near-white sample pair.

    A single solid sample can only recover the product ``a*C`` and must assume
    the watermark colour, so a tool whose watermark also draws a dark outline
    keeps that outline after the inverse. Two frames of the *same* watermark over
    a near-black and a near-white uniform background close the system
    ``obs = a*C + (1-a)*bg`` and separate the unknowns per pixel::

        obs_b = a*C + (1-a)*bg_b
        obs_w = a*C + (1-a)*bg_w
      =>  a = 1 - (obs_w - obs_b) / (bg_w - bg_b)
          C = (obs_b - (1-a)*bg_b) / a

    The recovered C is per-pixel, so the analytic inverse restores the glyph and
    its outline exactly (measured pair residual < 0.5/255). Returns
    ``(profile, report)`` or ``(None, report)``.
    """
    from PIL import Image

    cv2 = _cv2()
    np = _np()
    with Image.open(path_black) as im:
        black = np.array(im.convert('RGB')).astype(np.float32)
    with Image.open(path_white) as im:
        white = np.array(im.convert('RGB')).astype(np.float32)
    if black.shape != white.shape:
        return None, {'reason': f'sample sizes differ: {black.shape} vs {white.shape}'}
    height, width = black.shape[:2]
    if box is None:
        found = [b for b in (_detect_watermark_box(black.astype(np.uint8)),
                             _detect_watermark_box(white.astype(np.uint8))) if b]
        if found:
            box = (min(b[0] for b in found), min(b[1] for b in found),
                   max(b[2] for b in found), max(b[3] for b in found))
    x1, y1, x2, y2 = _resolve_box(box, width, height)
    if x2 - x1 < 4 or y2 - y1 < 4:
        return None, {'reason': f'invalid box: {box}'}
    # Both frames come from the same generator and should coincide; guard against
    # a sub-pixel/one-pixel drift so the per-pixel subtraction stays valid.
    pad = 40
    cx1, cy1 = max(0, x1 - pad), max(0, y1 - pad)
    cx2, cy2 = min(width, x2 + pad), min(height, y2 + pad)
    try:
        (dx, dy), _ = cv2.phaseCorrelate(
            black[cy1:cy2, cx1:cx2].max(2).astype(np.float64),
            white[cy1:cy2, cx1:cx2].max(2).astype(np.float64))
    except cv2.error:  # pragma: no cover - tiny crops
        dx = dy = 0.0
    if abs(dx) > 1.5 or abs(dy) > 1.5:
        m = np.array([[1, 0, -dx], [0, 1, -dy]], np.float32)
        white = cv2.warpAffine(white, m, (width, height), borderMode=cv2.BORDER_REPLICATE)
    # Background levels from a flat region away from the watermark.
    outside = np.ones((height, width), bool)
    outside[max(0, y1 - 8):min(height, y2 + 8),
            max(0, x1 - 8):min(width, x2 + 8)] = False
    bg_b = np.median(black[outside].reshape(-1, 3), 0).astype(np.float32)
    bg_w = np.median(white[outside].reshape(-1, 3), 0).astype(np.float32)
    db = float(bg_w.mean() - bg_b.mean())
    if db < 32:
        return None, {'reason': f'backgrounds too close (delta={db:.0f})'}
    win_b = black[y1:y2, x1:x2]
    win_w = white[y1:y2, x1:x2]
    alpha = _clean_alpha(np.clip(1.0 - (win_w - win_b).mean(2) / db, 0.0, 1.0),
                         thr=0.02, min_area=min_area)
    core = alpha > 0.05
    if int(core.sum()) < 50:
        return None, {'reason': 'not enough separable watermark coverage'}
    color = (win_b - (1 - alpha)[..., None] * bg_b.reshape(1, 1, 3)) \
        / np.maximum(alpha, 1e-3)[..., None]
    color = np.clip(color, 0, 255)
    color[alpha <= 0.03] = 0
    pred_b = alpha[..., None] * color + (1 - alpha)[..., None] * bg_b.reshape(1, 1, 3)
    pred_w = alpha[..., None] * color + (1 - alpha)[..., None] * bg_w.reshape(1, 1, 3)
    resid_b = float(np.abs(pred_b - win_b).mean(2)[core].mean())
    resid_w = float(np.abs(pred_w - win_w).mean(2)[core].mean())
    report = {
        'reason': 'ok',
        'residual_black': round(resid_b, 3),
        'residual_white': round(resid_w, 3),
        'coverage': round(float(core.mean()), 4),
        'box': [x1, y1, x2, y2],
        'bg_black': [int(v) for v in bg_b],
        'bg_white': [int(v) for v in bg_w],
        'shift': [round(float(dx), 2), round(float(dy), 2)],
    }
    if max(resid_b, resid_w) > max_resid:
        report['reason'] = f'pair residual {max(resid_b, resid_w):.2f} too high'
        return None, report
    alpha_c, color_c, sbox = _crop_to_alpha(alpha, color)
    short = min(width, height)
    ref = float(ref_short_side or short)
    place = {
        'ref': short,
        'right': (width - (x1 + sbox[0]) - alpha_c.shape[1]) / short,
        'bottom': (height - (y1 + sbox[1]) - alpha_c.shape[0]) / short,
    }
    profile = Profile(
        id=_slug(label or Path(path_black).stem),
        alpha=alpha_c,
        color=color_c,
        ref_short_side=ref,
        label=label,
        source=f'pair: {Path(path_black).name} + {Path(path_white).name} '
               f'box={[x1, y1, x2, y2]}',
        extra={'stroke_box': list(sbox), 'learn_report': report,
               'bg_median': report['bg_black'], 'place': place},
    )
    return profile, report


def place_by_anchor(profile, width, height):
    """Locate ``(px, py, scale)`` deterministically from the learned anchor.

    Returns ``None`` when the profile carries no anchor (e.g. built by
    ``learn-solid``) or the placement would fall outside the image.
    """
    place = profile.extra.get('place') if profile.extra else None
    if not place:
        return None
    short = min(width, height)
    ref = float(place.get('ref') or profile.ref_short_side)
    scale = short / ref
    aw = max(1, int(round(profile.alpha.shape[1] * scale)))
    ah = max(1, int(round(profile.alpha.shape[0] * scale)))
    px = int(round(width - float(place['right']) * short - aw))
    py = int(round(height - float(place['bottom']) * short - ah))
    if px < 0 or py < 0 or px + aw > width or py + ah > height:
        return None
    return px, py, scale


def auto_discover(frames, label, color=(255, 255, 255), ref_short_side=None,
                  min_area=24, max_resid=12.0, min_coverage=0.01,
                  min_contrast=60.0):
    """Pick the most uniform-background frame among same-watermark samples and
    build a profile from it, fully automatically.

    ``frames`` is a list of ``(name, path, box)`` (box may be ``None`` for the
    default bottom-right corner). Every frame is scored by how constant its
    background is; the calmest one is solved first, and the unsupervised
    multi-frame learner is tried as a fallback and kept when it fits better.
    Returns ``(profile, report)``.
    """
    from PIL import Image

    np = _np()
    scored = []
    eligible = []
    for item in frames:
        name, path, box = item[0], item[1], item[2]
        try:
            with Image.open(path) as im:
                rgb = np.array(im.convert('RGB'))
        except Exception as exc:  # pragma: no cover - unreadable input
            scored.append({'name': name, 'reason': f'unreadable: {exc}'})
            continue
        s = background_score(rgb, box, color=color)
        if s is None:
            scored.append({'name': name, 'reason': 'no watermark strokes found'})
            continue
        entry = {'name': name, 'path': path, 'box': s['box'],
                 'uniformity': round(s['uniformity'], 3),
                 'contrast': round(s['contrast'], 1),
                 'coverage': round(s['coverage'], 4),
                 'bg_median': s['bg_median'],
                 'short': s['short'], 'size': s['size']}
        if not (min_coverage <= s['coverage'] <= 0.7):
            entry['reason'] = f'coverage {s["coverage"]:.3f} out of range'
            scored.append(entry)
            continue
        scored.append(entry)
        # A uniform but low-contrast frame (white watermark on a white wall)
        # cannot separate coverage from background, so it is not a valid sample.
        if s['contrast'] < min_contrast:
            entry['reason'] = f'contrast {s["contrast"]:.0f} < {min_contrast:.0f}'
            continue
        eligible.append(entry)
    report = {'reason': 'no usable frame', 'candidates': scored}
    if not eligible:
        return None, report
    eligible.sort(key=lambda e: e['uniformity'])

    best = None
    for cand in eligible:
        profile, rep = build_from_uniform(
            cand['path'], box=cand['box'], color=color,
            ref_short_side=ref_short_side, label=label, min_area=min_area,
            max_resid=max_resid)
        if profile is not None:
            best = (profile, rep, cand)
            break

    # Unsupervised multi-frame learner over same-size frames, kept when better.
    groups = {}
    for e in eligible:
        groups.setdefault(tuple(e['size']), []).append(e)
    group = max(groups.values(), key=len) if groups else []
    if len(group) >= 3:
        ref = ref_short_side or group[0]['short']
        profile, rep = learn_from_batch([e['path'] for e in group],
                                        box=group[0]['box'], ref_short_side=ref,
                                        label=label, min_area=min_area,
                                        max_resid=max_resid)
        if profile is not None:
            rep = dict(rep)
            rep['method'] = 'batch'
            rep['frames'] = [e['name'] for e in group]
            if best is None or rep.get('residual', 1e9) < best[1].get('residual', 1e9):
                best = (profile, rep, {'name': f'batch x{len(group)}'})

    if best is None:
        report['reason'] = 'no frame produced a reliable profile'
        return None, report
    profile, rep, cand = best
    rep = dict(rep)
    rep.setdefault('method', 'uniform')
    rep['selected'] = cand['name']
    rep['candidates'] = scored
    return profile, rep


def _guess_corner_box(width, height):
    scale = min(width / 2848.0, height / 1600.0)
    bw = max(220, int(330 * scale))
    bh = max(78, int(118 * scale))
    return (max(0, width - bw), max(0, height - bh), width, height)


def _slug(text):
    keep = []
    for ch in text.strip().lower():
        if ch.isalnum() or ch in '-_':
            keep.append(ch)
        elif ch in ' \t':
            keep.append('-')
    slug = ''.join(keep).strip('-')
    return slug or 'profile'


def locate(gray, profile, any_position=False, min_score=12.0, margin=40):
    """Scale + gap-score match a profile on a grayscale image.

    Returns ``(px, py, score, scale)`` for the best right-bottom placement
    (whole image when ``any_position``), or ``None`` when below ``min_score``.
    """
    np = _np()
    cv2 = _cv2()
    height, width = gray.shape[:2]
    short = min(height, width)
    k = max(3, min(31, (short - 1) | 1))
    top = gray - cv2.morphologyEx(
        gray, cv2.MORPH_OPEN, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (k, k)))
    scale = short / profile.ref_short_side
    tw = max(2, int(round(profile.alpha.shape[1] * scale)))
    th = max(2, int(round(profile.alpha.shape[0] * scale)))
    if th >= height or tw >= width:
        return None
    t = _resize(profile.alpha, tw, th)
    t01 = (t > 0.5).astype(np.float32)
    n_in = float(t01.sum())
    if n_in < 20:
        return None
    n_out = float(t01.size) - n_in
    s_in = cv2.matchTemplate(top, t01, cv2.TM_CCORR)
    s_all = cv2.matchTemplate(top, np.ones_like(t01), cv2.TM_CCORR)
    response = s_in / n_in - (s_all - s_in) / max(n_out, 1.0)
    if any_position:
        window = response
        off = (0, 0)
    else:
        y0 = max(0, response.shape[0] - (margin + 1))
        x0 = max(0, response.shape[1] - (margin + 1))
        window = response[y0:, x0:]
        off = (x0, y0)
    _, _, _, peak = cv2.minMaxLoc(window)
    px, py = peak[0] + off[0], peak[1] + off[1]
    score = float(response[py, px])
    if score < min_score:
        return None
    return px, py, score, scale


def _ncc_scan(crops, profile, scales, ox, oy, w, h, any_position):
    np = _np()
    cv2 = _cv2()
    best = None
    for sc in scales:
        tw = max(2, int(round(profile.alpha.shape[1] * sc)))
        th = max(2, int(round(profile.alpha.shape[0] * sc)))
        t = cv2.resize(profile.alpha, (tw, th)).astype(np.float32)
        if float(t.std()) < 1e-6:
            continue
        t = (t - t.mean()) / (t.std() + 1e-6)
        for ch in crops:
            if th >= ch.shape[0] or tw >= ch.shape[1]:
                continue
            r = cv2.matchTemplate(ch, t, cv2.TM_CCOEFF_NORMED)
            _, mx, _, loc = cv2.minMaxLoc(r)
            px, py = loc[0] + ox, loc[1] + oy
            if not any_position and (px + tw < w * 0.9 or py + th < h * 0.9):
                continue
            if best is None or mx > best[0]:
                best = (float(mx), int(px), int(py), float(sc))
    return best


def locate_ncc(gray, profile, any_position=False, min_score=0.5, margin=60,
               scale_span=NCC_SCALE_SPAN, dark=True):
    """Robust localization by normalised cross-correlation of the alpha shape.

    The gap-score matcher (:func:`locate`) requires the watermark to be brighter
    than its surroundings, so it fails on pale or saturated backgrounds (a
    grey-white logo on white sand scores ~2). NCC is contrast-normalised and also
    matches the *dark* top-hat channel (dark stroke on a bright wall), which is
    what makes arbitrary backgrounds tractable. Returns ``(px, py, score, scale)``
    (score in 0..1) or ``None``.

    The scale search is two-stage: a wide coarse sweep (``scale_span``, +-35% by
    default) finds the size, then a narrow fine pass refines placement, so a
    watermark re-rendered at a different size is still localised (and its mask
    lands correctly even when the inverse is gated and the generative model
    takes over).
    """
    np = _np()
    cv2 = _cv2()
    h, w = gray.shape[:2]
    short = min(h, w)
    base = short / profile.ref_short_side
    k = max(3, min(31, (short - 1) | 1))
    se = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (k, k))
    g8 = gray.astype(np.uint8)
    chans = [gray - cv2.morphologyEx(g8, cv2.MORPH_OPEN, se).astype(np.float32)]
    if dark:
        chans.append(cv2.morphologyEx(g8, cv2.MORPH_CLOSE, se).astype(np.float32) - gray)
    chans = [(c - c.mean()) / (c.std() + 1e-6) for c in chans]
    # non-any-position: only the bottom-right region can host the watermark, so
    # crop the correlation search space (big speed-up) and keep the response
    # coordinates in full-image space via the offset.
    if any_position:
        ox, oy = 0, 0
        crops = chans
    else:
        ox, oy = int(w * 0.5), int(h * 0.72)
        crops = [c[oy:, ox:] for c in chans]
    coarse = [base * f for f in np.arange(1 - scale_span, 1 + scale_span + 1e-6,
                                          NCC_COARSE_STEP)]
    best = _ncc_scan(crops, profile, coarse, ox, oy, w, h, any_position)
    if best is not None:
        factor = best[3] / base
        fine = [base * factor * f
                for f in np.arange(1 - NCC_FINE_SPAN, 1 + NCC_FINE_SPAN + 1e-6,
                                   NCC_FINE_STEP)]
        refined = _ncc_scan(crops, profile, fine, ox, oy, w, h, any_position)
        if refined is not None and refined[0] > best[0]:
            best = refined
    if best is None or best[0] < min_score:
        return None
    _, px, py, sc = best
    return px, py, best[0], sc


def scaled_layers(profile, scale):
    tw = max(1, int(round(profile.alpha.shape[1] * scale)))
    th = max(1, int(round(profile.alpha.shape[0] * scale)))
    alpha = _resize(profile.alpha.astype('float32'), tw, th)
    color = _resize(profile.color.astype('float32'), tw, th)
    return alpha, color


def mask_for(profile, px, py, width, height, scale=1.0, thr=MASK_ALPHA_THRESHOLD):
    np = _np()
    cv2 = _cv2()
    a, _ = scaled_layers(profile, scale)
    th, tw = a.shape
    if py + th > height or px + tw > width or px < 0 or py < 0:
        return None
    core = (a * 255.0 > thr).astype(np.uint8)
    # Some tools draw a dark outline/shadow around the glyphs that is invisible
    # on the (black) calibration sample, so the alpha core misses it. Such a
    # profile can carry a wider ``mask_dilate`` so the outline is covered too.
    dilate = MASK_DILATE
    if profile.extra:
        d = profile.extra.get('mask_dilate')
        if d:
            dilate = (int(d[0]), int(d[1]))
    mask = cv2.dilate(core, cv2.getStructuringElement(cv2.MORPH_RECT, dilate), 1)
    full = np.zeros((height, width), np.uint8)
    full[py:py + th, px:px + tw] = mask * 255
    return full


def inverse_image(obs, mat, profile, px, py, scale=1.0, gain=None, model_name='generic'):
    """Reverse the blending equation with a matched profile.

    Returns ``(result_uint8, info)`` or ``(None, reason)`` when the profile does
    not overlap a plausible region or all pixels are ill-posed.
    """
    np = _np()
    cv2 = _cv2()
    height, width = obs.shape[:2]
    palpha, pcolor = scaled_layers(profile, scale)
    th, tw = palpha.shape
    if py < 0 or px < 0 or py + th > height or px + tw > width:
        return None, 'profile outside image'
    win = obs[py:py + th, px:px + tw].astype(np.float32)
    matw = mat[py:py + th, px:px + tw].astype(np.float32)

    def _apply(k):
        a = np.clip(palpha * k, 0, 1)[..., None]
        raw = (win - a * pcolor) / np.maximum(1 - a, 1e-3)
        inv = np.clip(raw, 0, 255)
        g = cv2.GaussianBlur(inv, (0, 0), 2.5)
        hf = (inv - g).max(2)
        m = (a > INVERSE_ALPHA_THRESHOLD)[..., 0]
        if int(m.sum()) < 50:
            return None
        ghost = abs(float(np.corrcoef(hf[m], a[..., 0][m])[0, 1])) if m.sum() > 2 else 1.0
        return ghost, inv, a

    best = None
    gains = [gain] if gain is not None else list(np.arange(*INVERSE_GAIN_RANGE, 0.05))
    for k in gains:
        got = _apply(float(k))
        if got is None:
            continue
        ghost, inv, a = got
        if best is None or ghost < best[0]:
            best = (ghost, inv, a, float(k))
    if best is None:
        return None, 'empty alpha overlap'
    ghost, inv, a, k = best
    if ghost > INVERSE_MAX_GHOST:
        return None, f'ghost {ghost:.3f} too high'
    # model-fit gate: reject when a single-colour C cannot explain the blend
    core = a[..., 0] > 0.3
    if int(core.sum()) < 20:
        return None, 'too few core pixels'
    pred = a * pcolor + (1 - a) * matw
    fit = float(np.abs(pred - win).mean(2)[core].mean())
    if fit > INVERSE_MAX_FIT:
        return None, f'model fit {fit:.1f} too high (single-colour model, keep MAT)'
    a3 = a
    raw = (win - a3 * pcolor) / np.maximum(1 - a3, 1e-3)
    ill = (((raw < -0.5) | (raw > 255.5)).any(2)) | ((win >= 252).all(2))
    hyb = np.where(ill[..., None], matw, inv)
    m = (a3[..., 0] > INVERSE_ALPHA_THRESHOLD)
    out = obs.copy()
    region = out[py:py + th, px:px + tw]
    region[m] = np.clip(hyb[m], 0, 255).astype(np.uint8)
    out[py:py + th, px:px + tw] = region
    return out, f'profile {profile.id} pos {px},{py} gain {k:.2f} ghost {ghost:.3f}'


def match_image(image, any_position=False, min_score=12.0, ncc_min=0.5):
    """Try every stored profile against a PIL image, return best match.

    Localisation is decided by the contrast-normalised NCC shape score, which
    cleanly separates a true watermark (>= ~0.5) from a different tool's
    same-looking watermark (<= ~0.4). The bright gap-score (``locate``) is kept
    as a fast pre-filter for reporting, but never decides on its own: it happily
    scores a *different* white watermark above threshold on dark backgrounds.
    Returns ``(profile, px, py, score, scale)`` or ``None``.
    """
    np = _np()
    gray = np.array(image.convert('RGB')).max(2).astype(np.float32)
    best = None
    for profile in list_profiles():
        ncc = locate_ncc(gray, profile, any_position=any_position, min_score=ncc_min)
        if ncc is None:
            continue
        px, py, score, scale = ncc
        if best is None or score > best[3]:
            best = (profile, px, py, score, scale)
    return best


def _cli():
    import argparse

    parser = argparse.ArgumentParser(description='Watermark profile library.')
    sub = parser.add_subparsers(dest='command', required=True)

    sub.add_parser('list', help='list stored watermark profiles')

    p_solid = sub.add_parser('learn-solid', help='profile from a solid-background sample')
    p_solid.add_argument('image')
    p_solid.add_argument('--box', help='x1,y1,x2,y2')
    p_solid.add_argument('--bg', default='0,0,0')
    p_solid.add_argument('--ref-short', type=float)
    p_solid.add_argument('--label', required=True)

    p_batch = sub.add_parser('learn-batch', help='unsupervised profile from aligned samples')
    p_batch.add_argument('images', nargs='+')
    p_batch.add_argument('--box')
    p_batch.add_argument('--ref-short', type=float)
    p_batch.add_argument('--label', required=True)

    p_match = sub.add_parser('match', help='match stored profiles against an image')
    p_match.add_argument('image')
    p_match.add_argument('--any-position', action='store_true')

    p_auto = sub.add_parser(
        'learn-auto',
        help='auto-pick the most uniform same-watermark sample and learn a profile')
    p_auto.add_argument('images', nargs='+')
    p_auto.add_argument('--label', required=True)
    p_auto.add_argument('--box', help='x1,y1,x2,y2 (same for all images)')
    p_auto.add_argument('--ref-short', type=float)
    p_auto.add_argument('--color', default='255,255,255')

    p_pair = sub.add_parser(
        'learn-pair',
        help='exact per-pixel profile from a near-black / near-white sample pair')
    p_pair.add_argument('black')
    p_pair.add_argument('white')
    p_pair.add_argument('--box', help='x1,y1,x2,y2')
    p_pair.add_argument('--ref-short', type=float)
    p_pair.add_argument('--label', required=True)

    args = parser.parse_args()
    if args.command == 'list':
        for profile in list_profiles():
            print(f'{profile.id}: label={profile.label!r} shape={profile.shape} '
                  f'ref_short={profile.ref_short_side:g} source={profile.source}')
        return
    if args.command == 'learn-solid':
        box = tuple(int(v) for v in args.box.split(',')) if args.box else None
        bg = tuple(int(v) for v in args.bg.split(','))
        profile = extract_from_solid(args.image, box=box, bg=bg,
                                     ref_short_side=args.ref_short, label=args.label)
        base = save_profile(profile)
        print(f'saved profile {profile.id} -> {base}')
        return
    if args.command == 'learn-batch':
        box = tuple(int(v) for v in args.box.split(',')) if args.box else None
        profile, report = learn_from_batch(args.images, box=box,
                                           ref_short_side=args.ref_short, label=args.label)
        print(json.dumps(report, ensure_ascii=False))
        if profile is None:
            raise SystemExit('batch learning failed; not saving a profile')
        base = save_profile(profile)
        print(f'saved profile {profile.id} -> {base}')
        return
    if args.command == 'learn-auto':
        from PIL import Image

        np = _np()
        box = tuple(int(v) for v in args.box.split(',')) if args.box else None
        color = tuple(int(v) for v in args.color.split(','))
        frames = []
        for p in args.images:
            with Image.open(p) as im:
                rgb = np.array(im.convert('RGB'))
            frames.append((Path(p).name, p, box if box else _detect_watermark_box(rgb)))
        profile, report = auto_discover(frames, label=args.label, color=color,
                                        ref_short_side=args.ref_short)
        print(json.dumps(report, ensure_ascii=False, indent=1))
        if profile is None:
            raise SystemExit('auto learning failed; not saving a profile')
        base = save_profile(profile)
        print(f'saved profile {profile.id} -> {base}')
        return
    if args.command == 'learn-pair':
        box = tuple(int(v) for v in args.box.split(',')) if args.box else None
        profile, report = extract_from_pair(args.black, args.white, box=box,
                                            ref_short_side=args.ref_short,
                                            label=args.label)
        print(json.dumps(report, ensure_ascii=False, indent=1))
        if profile is None:
            raise SystemExit('pair learning failed; not saving a profile')
        base = save_profile(profile)
        print(f'saved profile {profile.id} -> {base}')
        return
    if args.command == 'match':
        from PIL import Image

        with Image.open(args.image) as im:
            best = match_image(im, any_position=args.any_position)
        if best is None:
            print('no profile matched')
        else:
            profile, px, py, score, scale = best
            print(f'matched {profile.id} at ({px},{py}) score {score:.1f} scale {scale:.3f}')


if __name__ == '__main__':
    _cli()

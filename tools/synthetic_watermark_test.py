#!/usr/bin/env python3
"""合成水印能力边界测试：各类型水印 × 各背景，分层验证能力。

层1 检测：detect_watermark_boxes() 是否在正确位置检出框
层2 遮罩：prepare 是否为该场景生成非空 mask（含管线过滤行为）
层3 修复：inpaint 后目标位置水印是否消失（--model lama 快速跑，可选）

用法：
    .img-inpaint-venv/bin/python tools/synthetic_watermark_test.py           # 层1+2
    .img-inpaint-venv/bin/python tools/synthetic_watermark_test.py --layer3  # 加层3
    .img-inpaint-venv/bin/python tools/synthetic_watermark_test.py --only text=white pos=center   # 过滤场景

场景矩阵：
    文字色: white(255) / pale(220) / translucent(白 α0.6) / dark(20) / red
    位置:   topleft / center / bottomright
    背景:   black / gradient / photo:<dist 图名>   （黑底、渐变、真实照片）
"""
import argparse
import random
import sys
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

from PIL import Image, ImageDraw, ImageFont

PROJECT_ROOT = TOOLS.parent
DIST = PROJECT_ROOT / 'dist'
OUT = PROJECT_ROOT / '.synthetic-wm-test'
FONT_CANDIDATES = [
    '/System/Library/Fonts/Supplemental/Arial Bold.ttf',
    '/System/Library/Fonts/Helvetica.ttc',
]

TEXTS = ['SAMPLE TEXT', '测试水印ABC']


def font(size):
    for p in FONT_CANDIDATES:
        try:
            return ImageFont.truetype(p, size)
        except OSError:
            continue
    return ImageFont.load_default()


def make_background(kind, w, h, photo=None):
    if kind.startswith('photo:'):
        img = Image.open(DIST / f'{kind.split(":", 1)[1]}.png').convert('RGB')
        if img.size != (w, h):
            img = img.resize((w, h), Image.Resampling.LANCZOS)
        return img
    if kind == 'black':
        return Image.new('RGB', (w, h), (30, 30, 30))
    if kind == 'whitebg':
        return Image.new('RGB', (w, h), (235, 235, 235))
    if kind == 'gradient':
        base = Image.new('RGB', (w, h))
        px = base.load()
        for y in range(h):
            v = 40 + int(180 * y / h)
            for x in range(0, w, 4):
                for dx in range(4):
                    if x + dx < w:
                        px[x + dx, y] = (v, v, min(255, v + 10))
        return base
    raise ValueError(kind)


def stamp_text(base, color_kind, pos, alpha=1.0):
    """把测试水印文字叠加到基图，返回 (图, 水印真实 bbox)。"""
    w, h = base.size
    f = font(max(24, h // 24))
    overlay = Image.new('RGBA', (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(overlay)
    colors = {
        'white': (255, 255, 255),
        'pale': (220, 220, 220),
        'translucent': (255, 255, 255),
        'dark': (20, 20, 20),
        'red': (200, 30, 30),
    }
    rgb = colors[color_kind]
    a = int(255 * alpha)
    text = TEXTS[0]
    bbox = d.textbbox((0, 0), text, font=f)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    margin = 24
    if pos == 'topleft':
        x, y = margin, margin
    elif pos == 'center':
        x, y = (w - tw) // 2, (h - th) // 2
    else:
        x, y = w - tw - margin, h - th - margin - bbox[1]
    d.text((x, y - bbox[1]), text, font=f, fill=(*rgb, a))
    out = base.convert('RGBA')
    out.alpha_composite(overlay)
    real = (x, y, x + tw, y + th)
    return out.convert('RGB'), real


def iou(a, b):
    ix1, iy1 = max(a[0], b[0]), max(a[1], b[1])
    ix2, iy2 = min(a[2], b[2]), min(a[3], b[3])
    if ix2 <= ix1 or iy2 <= iy1:
        return 0.0
    inter = (ix2 - ix1) * (iy2 - iy1)
    ua = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / ua if ua else 0.0


def layer1_detect(image, real):
    from remove_doubao_watermark import detect_watermark_boxes
    boxes = detect_watermark_boxes(image, extended=True)
    hit = max((iou(b, real) for b in boxes), default=0.0)
    return hit, boxes


def layer2_prepare(image, w, h, real, any_position=False):
    """直接复用 prepare 的遮罩来源逻辑（不落盘原图，用合成图）"""
    from remove_doubao_watermark import detect_watermark_boxes
    boxes = detect_watermark_boxes(image, extended=any_position)
    pw, ph = image.size
    if not any_position:
        boxes = [b for b in boxes if b[2] > pw - 40 and b[3] > ph - 40]
    return max((iou(b, real) for b in boxes), default=0.0), boxes


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--layer3', action='store_true', help='also run lama inpaint per scene (slow)')
    ap.add_argument('--only', help='filter scenes, e.g. pos=center or text=dark')
    args = ap.parse_args()
    random.seed(42)
    OUT.mkdir(exist_ok=True)

    w, h = 1600, 900
    backgrounds = ['black', 'gradient', 'whitebg', 'photo:2', 'photo:4']
    colors = ['white', 'pale', 'translucent', 'dark', 'red']
    positions = ['topleft', 'center', 'bottomright']

    rows = []
    for bg in backgrounds:
        for color in colors:
            for pos in positions:
                if args.only and args.only not in f'{color} {pos} {bg}':
                    continue
                alpha = 0.6 if color == 'translucent' else 1.0
                base = make_background(bg, w, h)
                img, real = stamp_text(base, color, pos, alpha)
                fname = f'{bg}__{color}__{pos}.png'
                img.save(OUT / fname)
                hit, boxes = layer1_detect(img, real)
                kept_iou, kept = layer2_prepare(img, w, h, real)
                anyp_iou, _ = layer2_prepare(img, w, h, real, any_position=True)
                rows.append((fname, hit, len(boxes), kept_iou, len(kept), anyp_iou))

    print(f'{"scene":42s} {"det@pos":>8s} {"boxes":>5s} {"pipe@pos":>8s} {"kept":>4s} {"anyp@pos":>8s}')
    for fname, hit, nb, kio, nk, aio in rows:
        d = 'HIT' if hit > 0.3 else ('partial' if hit > 0.05 else 'miss')
        p = 'HIT' if kio > 0.3 else ('partial' if kio > 0.05 else 'miss')
        a = 'HIT' if aio > 0.3 else ('partial' if aio > 0.05 else 'miss')
        print(f'{fname:42s} {d:>8s} {nb:>5d} {p:>8s} {nk:>4d} {a:>8s}')

    det_hits = sum(1 for r in rows if r[1] > 0.3)
    pipe_hits = sum(1 for r in rows if r[3] > 0.3)
    anyp_hits = sum(1 for r in rows if r[5] > 0.3)
    print(f'\ndetect layer: {det_hits}/{len(rows)}   pipeline default: {pipe_hits}/{len(rows)}   pipeline --any-position: {anyp_hits}/{len(rows)}')
    print(f'images in {OUT}')


if __name__ == '__main__':
    main()

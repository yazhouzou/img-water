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
    """两级检测，只信右下角：
    1) 全图扫纯白文字（≥248），但仅保留落在右下角区域的候选——其它位置的纯白块
       （灯罩、餐盘、白墙等画面主体）形态上与文字水印无法区分，擦掉会毁图；
    2) 右下角自适应阈值兜底（识别半透明/灰白粗体水印），与第 1 级合并去重。
    多位置水印请用 --mask-box 手动指定。"""
    full = [box for box in _detect_full_white(image) if _is_corner_box(box, image)]
    corner = _detect_corner_faded(image)
    boxes = full + [b for b in corner if not any(_overlap(b, f) for f in full)]
    return boxes


def _is_corner_box(box, image):
    _, _, x2, y2 = box
    w, h = image.size
    return x2 > w * 0.85 and y2 > h * 0.85


def _overlap(a, b):
    return not (a[2] <= b[0] or b[2] <= a[0] or a[3] <= b[1] or b[3] <= a[1])


def _detect_full_white(image):
    try:
        import cv2
        import numpy as np
    except ImportError:
        return []
    img = np.array(image.convert('RGB'))
    h, w = img.shape[:2]
    white = (img >= 248).all(axis=2).astype(np.uint8)
    kernel = cv2.getStructuringElement(cv2.MORPH_RECT, (9, 3))
    merged = cv2.dilate(white, kernel, iterations=2)
    count, _, stats, _ = cv2.connectedComponentsWithStats(merged, 8)
    candidates = []
    for i in range(1, count):
        x, y, cw, ch, area = (int(v) for v in stats[i])
        if area < 1200 or ch < h * 0.012 or ch > h * 0.09 or cw < ch:
            continue
        ratio = cw / ch
        if ratio < 2.0 or ratio > 15:
            continue
        fill = area / float(cw * ch)
        if fill < 0.2 or fill > 0.95:
            continue
        corner_dist = (w - (x + cw)) + (h - (y + ch))
        score = area * min(ratio / 6.0, 1.0) / (1.0 + corner_dist / (w * 0.1))
        candidates.append(((x, y, x + cw, y + ch), score))
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


def _detect_corner_faded(image):
    """右下角自适应阈值兜底：识别半透明/灰白粗体水印（如豆包新样式，亮度 150~240 不等）。
    仅扫右下角区域，阈值取背景中位数 +60，要求候选框贴近右下边缘，避免误擦画面元素。"""
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
    threshold = max(150, int(gray.mean()) + 60)
    white = (gray >= threshold).astype(np.uint8)
    kernel = cv2.getStructuringElement(cv2.MORPH_RECT, (9, 3))
    merged = cv2.dilate(white, kernel, iterations=2)
    count, _, stats, _ = cv2.connectedComponentsWithStats(merged, 8)
    boxes = []
    for i in range(1, count):
        x, y, cw, ch, area = (int(v) for v in stats[i])
        gx1, gy1, gx2, gy2 = x0 + x, y0 + y, x0 + x + cw, y0 + y + ch
        if area < 400 or cw < ch or cw / ch > 20:
            continue
        fill = area / float(cw * ch)
        if fill < 0.15 or fill > 0.95:
            continue
        # 水印贴右下角：右缘距图右 <40px、底缘距图底 <40px
        if gx2 < w - 40 or gy2 < h - 40:
            continue
        boxes.append((gx1, gy1, gx2, gy2))
    if not boxes:
        return []
    pad = max(10, h // 150)
    return [
        (max(0, x1 - pad), max(0, y1 - pad), min(w - 6, x2 + pad), min(h - 6, y2 + pad))
        for x1, y1, x2, y2 in boxes
    ]


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


def prepare(names, custom_box, root, emit=True):
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
        if custom_box is None:
            with Image.open(backup) as probe:
                boxes = detect_watermark_boxes(probe)
            if boxes:
                print(f'{name}: auto-detected {len(boxes)} watermark box(es) {boxes}')
            else:
                box = resolve_box(width, height, None)
                boxes = [box]
                print(f'{name}: detection failed, using default rule {box}')
        else:
            boxes = [resolve_box(width, height, custom_box)]
        mask = Image.new('L', (width, height), 0)
        draw = ImageDraw.Draw(mask)
        for box in boxes:
            draw.rounded_rectangle(box, radius=4, fill=255)
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


def inpaint():
    if not IOPAINT.exists():
        raise SystemExit('missing project env; run tools/ensure-inpaint-env.sh (or .ps1 on Windows) first')
    subprocess.run(
        [
            str(IOPAINT),
            'run',
            '--model',
            'lama',
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


def run_all(names, custom_box, keep_work, root):
    prepare(names, custom_box, root, emit=False)
    inpaint()
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
    parser.add_argument('command', choices=['run', 'prepare', 'inpaint', 'review-lama', 'overwrite-review', 'cleanup'])
    parser.add_argument('files', nargs='*')
    args = parser.parse_args()
    root = Path(args.root).resolve() if args.root else DEFAULT_ROOT
    if not root.exists():
        raise SystemExit(f'root folder not found: {root}')
    names = target_names(args.files, root)

    if args.command == 'run':
        run_all(names, args.mask_box, args.keep_work, root)
    elif args.command == 'prepare':
        prepare(names, args.mask_box, root)
    elif args.command == 'inpaint':
        inpaint()
    elif args.command == 'review-lama':
        review_lama(names)
    elif args.command == 'overwrite-review':
        overwrite_review(names, root)
    elif args.command == 'cleanup':
        cleanup(names, root)


if __name__ == '__main__':
    main()

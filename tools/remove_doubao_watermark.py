#!/usr/bin/env python3
import argparse
import os
import subprocess
import sys
from pathlib import Path
from shutil import copyfile, rmtree


ROOT = Path(__file__).resolve().parents[1]
VENV = ROOT / '.img-inpaint-venv'
VENV_PYTHON = VENV / 'bin' / 'python'
if VENV_PYTHON.exists() and Path(sys.prefix).resolve() != VENV.resolve():
    os.execv(str(VENV_PYTHON), [str(VENV_PYTHON), str(Path(__file__).resolve()), *sys.argv[1:]])

try:
    from PIL import Image, ImageDraw, ImageFont
except ModuleNotFoundError as exc:
    if exc.name == 'PIL':
        raise SystemExit('missing Pillow; run ./tools/ensure-inpaint-env.sh first') from exc
    raise


WORK = Path(os.environ.get('DOUBAO_WATERMARK_WORKDIR', '/tmp/doubao-watermark-work'))
SOURCE = WORK / 'source'
MASKS = WORK / 'masks'
LAMA = WORK / 'lama'
REVIEW = WORK / 'review'
BACKUP = ROOT / 'original-watermark-backup'
IOPAINT = VENV / 'bin' / 'iopaint'


def numeric_key(path):
    return (0, int(path.stem)) if path.stem.isdigit() else (1, path.stem)


def target_names(files):
    paths = [ROOT / item for item in files] if files else sorted(ROOT.glob('*.png'), key=numeric_key)
    names = []
    for path in paths:
        if path.suffix.lower() != '.png':
            raise SystemExit(f'not a png: {path.name}')
        if not path.exists():
            raise SystemExit(f'missing file: {path.name}')
        names.append(path.name)
    if not names:
        raise SystemExit('no root png files found')
    return names


def mask_box(width, height):
    if (width, height) == (2848, 1600):
        return (width - 330, height - 118, width - 8, height - 8)
    if (width, height) == (2278, 1280):
        return (width - 275, height - 92, width - 7, height - 8)
    scale = min(width / 2848, height / 1600)
    box_width = max(220, int(330 * scale))
    box_height = max(78, int(118 * scale))
    return (max(0, width - box_width), max(0, height - box_height), width - 8, height - 8)


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


def ensure_work_dirs():
    for path in (SOURCE, MASKS, LAMA, REVIEW, BACKUP):
        path.mkdir(parents=True, exist_ok=True)


def prepare(names, custom_box, emit=True):
    if WORK.exists():
        rmtree(WORK)
    ensure_work_dirs()
    for name in names:
        current = ROOT / name
        backup = BACKUP / name
        if not backup.exists():
            copyfile(current, backup)
        copyfile(backup, SOURCE / name)

        with Image.open(backup) as image:
            width, height = image.size
        mask = Image.new('L', (width, height), 0)
        ImageDraw.Draw(mask).rounded_rectangle(resolve_box(width, height, custom_box), radius=4, fill=255)
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
        raise SystemExit('missing project env; run tools/ensure-inpaint-env.sh first')
    subprocess.run(
        [
            str(IOPAINT),
            'run',
            '--model',
            'lama',
            '--device',
            'mps',
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


def overwrite_review(names, emit=True):
    for name in names:
        copyfile(LAMA / name, ROOT / name)
    output = REVIEW / 'overwritten-corner-review.png'
    review(ROOT, output.name, names)
    if emit:
        print(output)
    return output


def cleanup(names):
    for name in names:
        backup = BACKUP / name
        if backup.exists():
            backup.unlink()
    if BACKUP.exists() and not any(BACKUP.iterdir()):
        BACKUP.rmdir()
    if WORK.exists():
        rmtree(WORK)


def run_all(names, custom_box, keep_work):
    prepare(names, custom_box, emit=False)
    inpaint()
    candidate_review = review_lama(names, emit=False)
    final_review = overwrite_review(names, emit=False)
    print(f'processed {len(names)} file(s): {", ".join(names)}')
    print(f'candidate review: {candidate_review}')
    print(f'final review: {final_review}')
    if keep_work:
        print(f'kept workdir: {WORK}')
    else:
        cleanup(names)
        print('cleaned: original-watermark-backup and /tmp/doubao-watermark-work')


def main():
    parser = argparse.ArgumentParser(description='Remove Doubao or custom text watermark from root PNG images.')
    parser.add_argument(
        '--mask-box',
        type=parse_box,
        help='custom watermark box as x1,y1,x2,y2; negative values are relative to right/bottom, e.g. -330,-118,-8,-8',
    )
    parser.add_argument(
        '--keep-work',
        action='store_true',
        help='keep original-watermark-backup and /tmp/doubao-watermark-work after the run command for manual review',
    )
    parser.add_argument('command', choices=['run', 'prepare', 'inpaint', 'review-lama', 'overwrite-review', 'cleanup'])
    parser.add_argument('files', nargs='*')
    args = parser.parse_args()
    names = target_names(args.files)

    if args.command == 'run':
        run_all(names, args.mask_box, args.keep_work)
    elif args.command == 'prepare':
        prepare(names, args.mask_box)
    elif args.command == 'inpaint':
        inpaint()
    elif args.command == 'review-lama':
        review_lama(names)
    elif args.command == 'overwrite-review':
        overwrite_review(names)
    elif args.command == 'cleanup':
        cleanup(names)


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""为 Rust `refine_box_mask` 逐像素一致性测试生成参考产物。

用法（仓库根目录）：
    .img-inpaint-venv/bin/python tools/refine_parity_dump.py [输出目录]

默认输出 /tmp/refine-parity（Rust 测试 `refine_box_mask_matches_python` 的默认目录，
该测试是 #[ignore]，用 `cargo test --lib -- --ignored refine_box_mask_matches_python`
运行）。必须有 cv2/numpy。

注意：Python 里 `refine_box_mask(..., model)` 的最终膨胀核随模型不同
（MAT 7x7 / LaMa 19x11），Rust 内核固定 LaMa，故这里固定传 'lama'。
"""
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location(
    'rdw', ROOT / 'tools' / 'remove_doubao_watermark.py'
)
rdw = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rdw)

CASES = [
    ('1.png', (-120, -90, -10, -10)),
    ('2.png', (-160, -120, -20, -30)),
    ('3.png', (-140, -100, -6, -14)),
    ('6.png', (-130, -95, -8, -12)),
]


def main() -> int:
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('/tmp/refine-parity')
    out_dir.mkdir(parents=True, exist_ok=True)
    from PIL import Image

    out = []
    for name, rel in CASES:
        img_path = ROOT / 'dist' / name
        if not img_path.exists():
            print(f'skip {name}: {img_path} not found')
            continue
        im = Image.open(img_path).convert('RGB')
        box = rdw.resolve_box(*im.size, rel)
        mask = rdw.refine_box_mask(im, box, 'lama')
        if mask is None:
            out.append({'name': name, 'box': list(box), 'mask': None})
            print(f'{name}: python refine returned None')
        else:
            fn = out_dir / f'{name}.mask.png'
            Image.fromarray(mask).save(fn)
            out.append({'name': name, 'box': list(box), 'mask': str(fn),
                        'px': int((mask > 0).sum())})
            print(f'{name}: python refine {int((mask > 0).sum())}px')
    manifest = out_dir / 'manifest.json'
    manifest.write_text(json.dumps(out, indent=1))
    print(f'wrote {manifest}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

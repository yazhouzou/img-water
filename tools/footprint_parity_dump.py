#!/usr/bin/env python3
"""为 Rust `template_footprint_mask` 逐像素一致性测试生成参考产物。

用法（仓库根目录）：
    .img-inpaint-venv/bin/python tools/footprint_parity_dump.py [输出目录]

默认输出 /tmp/footprint-parity（Rust 测试 `template_footprint_mask_matches_python`
的默认目录，该测试是 #[ignore]，用
`cargo test --lib -- --ignored template_footprint_mask_matches_python` 运行）。
必须有 cv2/numpy。
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

CASES = ['1.png', '2.png', '3.png', '4.png', '6.png']


def main() -> int:
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('/tmp/footprint-parity')
    out_dir.mkdir(parents=True, exist_ok=True)
    from PIL import Image
    import numpy as np

    manifest = []
    for name in CASES:
        src = ROOT / 'dist' / name
        if not src.exists():
            continue
        with Image.open(src) as im:
            rgb = im.convert('RGB')
            w, h = rgb.size
            gray = np.array(rgb).max(axis=2).astype(np.float32)
        mask, score, info = rdw.template_stroke_mask(gray, w, h, source='footprint')
        entry = {'name': name, 'score': float(score), 'info': info}
        if mask is not None:
            p = out_dir / f'py-{name}.png'
            Image.fromarray(mask).save(p)
            entry['mask'] = str(p)
        manifest.append(entry)
    (out_dir / 'manifest.json').write_text(
        json.dumps(manifest, ensure_ascii=False, indent=1)
    )
    print(f'wrote {len(manifest)} cases to {out_dir}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

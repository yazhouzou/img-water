#!/usr/bin/env python3
"""双端修复结果自动对比：终端 iopaint(big-lama JIT) vs App 内核(clean-cli ONNX)。

生成多场景合成图，用同一遮罩框分别跑两条管线，量化对比修复区域差异。
用于捕获 App 内核的回归（值域/遮罩/写回/缩放等细节 bug），无需人工复查。

用法:
  .img-inpaint-venv/bin/python tools/compare_pipelines.py [--keep]
退出码: 0=全部通过 1=存在 FAIL。
"""
import argparse
import pathlib
import shutil
import subprocess
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
CLEAN_CLI = ROOT / "desktop/src-tauri/target/release/clean-cli"
WORK = pathlib.Path("/tmp/wm-pipeline-compare")

FONT = "/System/Library/Fonts/Helvetica.ttc"


def make_case(case_dir, name, size, text, xy, font_size, bg, texture):
    case_dir.mkdir(parents=True, exist_ok=True)
    w, h = size
    rng = np.random.default_rng(abs(hash(name)) % 2**31)
    base = np.ones((h, w, 3), dtype=np.float32) * np.array(bg, dtype=np.float32)
    if texture:
        yy, xx = np.mgrid[0:h, 0:w]
        base += (12 * np.sin(xx / 5.3) * np.cos(yy / 7.1))[:, :, None]
        base += rng.normal(0, 9, (h, w, 3))
    img = Image.fromarray(np.clip(base, 0, 255).astype(np.uint8))
    d = ImageDraw.Draw(img)
    font = ImageFont.truetype(FONT, font_size)
    d.text(xy, text, fill=(252, 252, 253), font=font)
    d.text((xy[0] + 2, xy[1] + 2), text, fill=(255, 255, 255), font=font)
    path = case_dir / name
    img.save(path)
    # 记录文字实际 bbox（供 mask-box 使用，含少量余量）
    b = d.textbbox(xy, text, font=font)
    return (b[0] - 14, b[1] - 14, b[2] + 14, b[3] + 14)


def cases():
    return [
        # name, size, 文本, xy, 字号, bg, 纹理, 期望路径(same=双端同 crop 原分辨率)
        ("small-corner", (2048, 2048), "豆包AI生成", (1600, 1880), 64, (96, 104, 116), True, "tight"),
        ("wide-828", (2048, 2048), "豆包AI生成·转载", (1190, 1888), 96, (60, 70, 80), True, "scaled"),
        ("edge-clamp", (1600, 1200), "AI生成", (1360, 1080), 72, (180, 170, 150), True, "scaled"),
        ("tiny-image", (400, 400), "AI", (280, 320), 48, (90, 130, 90), True, "tight"),
        ("multi-pos", (2048, 1600), "AI生成", (300, 200), 64, (70, 90, 110), True, "tight"),
    ]


def run_cmd(cmd):
    return subprocess.run(cmd, capture_output=True, text=True)


def mad_psnr(a, b):
    diff = np.abs(a.astype(np.int16) - b.astype(np.int16))
    mad = diff.mean()
    mse = (diff.astype(np.float64) ** 2).mean()
    psnr = 10 * np.log10(255**2 / max(mse, 1e-9))
    return mad, psnr


def compare_case(name, size, text, xy, font_size, bg, texture, expect):
    case_root = WORK / name
    shutil.rmtree(case_root, ignore_errors=True)
    box = make_case(case_root / "orig", "1.png", size, text, xy, font_size, bg, texture)

    # 终端管线（覆盖模式）
    term_dir = case_root / "term"
    shutil.copytree(case_root / "orig", term_dir)
    r = run_cmd([str(ROOT / ".img-inpaint-venv/bin/python"), str(ROOT / "tools/remove_doubao_watermark.py"),
                 "--root", str(term_dir), "--mask-box", ",".join(map(str, box)), "run"])
    if r.returncode != 0:
        return ("FAIL-run", 999, 0, f"terminal pipeline failed: {r.stderr[-200:]}")

    # App 内核（覆盖模式，与终端同语义）
    app_dir = case_root / "app"
    shutil.copytree(case_root / "orig", app_dir)
    r = run_cmd([str(CLEAN_CLI), "--root", str(app_dir), "--mask-box", ",".join(map(str, box)), "--overwrite", "run"])
    if r.returncode != 0:
        return ("FAIL-run", 999, 0, f"clean-cli failed: {r.stderr[-200:]}")

    orig = np.array(Image.open(case_root / "orig/1.png").convert("RGB"))
    term = np.array(Image.open(term_dir / "1.png").convert("RGB"))
    app = np.array(Image.open(app_dir / "1.png").convert("RGB"))

    mask = np.zeros(orig.shape[:2], dtype=bool)
    mask[box[1]:box[3], box[0]:box[2]] = True
    mad, psnr = mad_psnr(term[mask], app[mask])
    # 双端对“未处理区”都应保持原图
    _, psnr_out = mad_psnr(orig[~mask], app[~mask])

    limit = 25.0 if expect == "scaled" else 8.0
    status = "PASS" if mad <= limit else "FAIL"
    note = f"修复区差异 MAD={mad:.2f} PSNR={psnr:.1f}dB (阈值{limit}); 未处理区 PSNR={psnr_out:.1f}dB"
    return (status, mad, psnr, note)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--keep", action="store_true", help="保留 /tmp 中间产物")
    args = ap.parse_args()

    if not CLEAN_CLI.exists():
        print(f"clean-cli 不存在: {CLEAN_CLI}\n先构建: cd desktop/src-tauri && cargo build --release --bin clean-cli")
        return 1
    shutil.rmtree(WORK, ignore_errors=True)
    WORK.mkdir(parents=True)

    print(f"{'场景':<14}{'结果':<8}{'修复区MAD':>10}{'PSNR':>9}  说明")
    failed = 0
    for case in cases():
        name = case[0]
        status, mad, psnr, note = compare_case(*case)
        if status != "PASS":
            failed += 1
        print(f"{name:<14}{status:<8}{mad:>10.2f}{psnr:>9.1f}  {note}")

    if not args.keep:
        shutil.rmtree(WORK, ignore_errors=True)
    print("\n结论:", "全部通过" if failed == 0 else f"{failed} 个场景 FAIL")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

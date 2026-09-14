#!/usr/bin/env python3
"""水印处理回归语料库：把每个已解决/已知边界的场景固化成测试点，改动后量化 PASS/FAIL。

设计目标（防退化 + 接住新场景）：
  - 每个新遇到的场景修好后，都变成一个永久测试点，下次改动不许退化；
  - 未见过的新场景由同一套机制判定，失败会以非零退出码暴露，不再靠人肉发现。

分层（与 AGENTS.md 的三层能力对应）：
  L1 检测/遮罩（快，无模型）：真实豆包图是否命中模板、已去水印图是否误命中、
      合成文字水印是否被检测/遮罩命中已知位置；
  L3 修复（--e2e，慢，需模型）：真实跑一遍 CLI（prepare+inpaint+overwrite-review），
      再用结果级验证（mask 外零改动 / 模板残留 / 合成水印残留）判定。

用法：
  .img-inpaint-venv/bin/python tools/watermark_regression.py          # L1
  .img-inpaint-venv/bin/python tools/watermark_regression.py --e2e    # L1 + L3
  .img-inpaint-venv/bin/python tools/watermark_regression.py --only doubao
  .img-inpaint-venv/bin/python tools/watermark_regression.py --e2e --model mat
退出码：0 全过；1 有 FAIL（可用于 CI / 提交前预检）。
"""
import argparse
import hashlib
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from shutil import rmtree

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

# 与 remove_doubao_watermark 相同：确保在项目 venv 内运行（导入该模块前完成，
# 否则模块的 re-exec 会把本进程替换成主管线脚本）
ROOT = TOOLS.parent
VENV = ROOT / '.img-inpaint-venv'
_VENV_PY = VENV / ('Scripts/python.exe' if os.name == 'nt' else 'bin/python')
if _VENV_PY.exists() and Path(sys.prefix).resolve() != VENV.resolve():
    argv = [str(_VENV_PY), str(Path(__file__).resolve()), *sys.argv[1:]]
    if os.name == 'nt':
        sys.exit(subprocess.call(argv))
    os.execv(str(_VENV_PY), argv)

from PIL import Image  # noqa: E402

DIST = ROOT / 'dist'


def md5(path):
    h = hashlib.md5()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


# ---------------------------------------------------------------------------
# L1：检测/遮罩层
# ---------------------------------------------------------------------------

def eval_doubao(rdw, path):
    """豆包模板是否命中（正样本应命中、负样本应不命中）。"""
    import numpy as np
    with Image.open(path) as im:
        rgb = im.convert('RGB')
        w, h = rgb.size
        gray = np.array(rgb).max(axis=2).astype(np.float32)
    _, score, _ = rdw.template_stroke_mask(gray, w, h)
    return score >= rdw.TEMPLATE_MIN_SCORE, score


def eval_synth(rdw, swt, img, real, any_position):
    """合成文字水印：检测框与已知 bbox 的最大 IoU（含管线位置过滤）。
    real=None（负样本）时，任何检出都算误检。"""
    boxes = rdw.detect_watermark_boxes(img, extended=any_position)
    w, h = img.size
    if not any_position:
        boxes = [b for b in boxes if b[2] > w - 40 and b[3] > h - 40]
    if real is None:
        return 1.0 if boxes else 0.0
    return max((swt.iou(b, real) for b in boxes), default=0.0)


def build_cases(rdw, swt):
    cases = []
    # A. 真实豆包正样本（dist 带水印原图）：应命中模板
    for path in sorted(DIST.glob('*.png')):
        if 'backup' in path.parts:
            continue
        cases.append({'group': 'doubao', 'name': f'dist/{path.name}', 'path': path,
                      'layer': 1, 'expect': 'hit'})
    # B. 真实豆包负样本（根目录已去水印图，与 dist 不同者）：不应命中（防误检）
    clean = []
    for path in sorted(ROOT.glob('*.png')):
        ref = DIST / path.name
        if ref.exists() and md5(path) != md5(ref):
            clean.append(path)
            cases.append({'group': 'doubao-neg', 'name': f'{path.name} (processed)',
                          'path': path, 'layer': 1, 'expect': 'miss'})

    w, h = 1600, 900
    # 干净背景：用已去水印的根目录图【裁切】（不缩放，避免重采样伪影导致误检）
    def bg(kind):
        if kind.startswith('clean:'):
            im = Image.open(ROOT / f'{kind.split(":", 1)[1]}.png').convert('RGB')
            return im.crop((0, 0, w, h)) if im.width >= w and im.height >= h else im.resize((w, h))
        return swt.make_background(kind, w, h)

    photo_bgs = [f'clean:{p.stem}' for p in clean[:3]]
    # C. 支持能力：亮色文字水印（右下角，默认管线）应命中
    for bgk in ['black', 'gradient', 'whitebg', *photo_bgs]:
        for color in ['white', 'translucent']:
            img, real = swt.stamp_text(bg(bgk), color, 'bottomright', 1.0)
            cases.append({'group': 'synth-corner', 'name': f'{bgk}/{color}/bottomright',
                          'img': img, 'real': real, 'any_position': False,
                          'layer': 1, 'expect': 'hit'})
    # D. 支持能力：深色/彩色文字（任意位置，--any-position 扩展检测）应命中
    for pos in ['topleft', 'center', 'bottomright']:
        for color in ['white', 'dark', 'red']:
            img, real = swt.stamp_text(bg('gradient'), color, pos, 1.0)
            cases.append({'group': 'synth-any', 'name': f'gradient/{color}/{pos}',
                          'img': img, 'real': real, 'any_position': True,
                          'layer': 1, 'expect': 'hit'})
    # E. 支持能力：无水印纯背景/干净照片不应误检
    for bgk in ['black', 'whitebg', 'gradient', 'clean:2', 'clean:3']:
        try:
            img = bg(bgk)
        except FileNotFoundError:
            continue
        cases.append({'group': 'synth-neg', 'name': f'{bgk}/no-watermark',
                      'img': img, 'real': None,
                      'any_position': False, 'layer': 1, 'expect': 'miss'})
    # F. 已知边界（物理低对比 / busy photo 误检；不算回归失败，只锁定现状）
    for bgk, color in [('gradient', 'pale'), ('whitebg', 'pale'), ('clean:3', 'pale')]:
        try:
            img, real = swt.stamp_text(bg(bgk), color, 'bottomright', 1.0)
        except FileNotFoundError:
            continue
        cases.append({'group': 'boundary', 'name': f'{bgk}/{color} (low-contrast)',
                      'img': img, 'real': real, 'any_position': False,
                      'layer': 1, 'expect': 'miss', 'boundary': True})
    return cases


def run_layer1(rdw, swt, cases, only):
    rows = []
    for c in cases:
        if only and only not in c['group'] and only not in c['name']:
            continue
        if c['group'].startswith('doubao'):
            hit, score = eval_doubao(rdw, c['path'])
            detail = f'tmpl score {score:.1f}'
        else:
            iou = eval_synth(rdw, swt, c['img'], c['real'], c['any_position'])
            hit = iou > 0.3
            detail = f'max IoU {iou:.2f}'
        rows.append((c, hit == (c['expect'] == 'hit'), detail))
    return rows


def run_verify_checks(rdw, only):
    """验证闭环自检（无模型）：残留标记 / mask 膨胀（自动重试的落点）/ 空 mask 保护。"""
    import numpy as np
    rows = []
    if only and 'verify' not in only:
        return rows
    dist2, clean2 = DIST / '2.png', ROOT / '2.png'
    if not (dist2.exists() and clean2.exists()):
        return rows
    with Image.open(dist2) as im:
        rgb = im.convert('RGB')
        w, h = rgb.size
        gray = np.array(rgb).max(axis=2).astype(np.float32)
    mask_arr, _, _ = rdw.template_stroke_mask(gray, w, h)
    if mask_arr is None:
        return rows
    tmp = Path(tempfile.mkdtemp(prefix='wm-verify-'))
    mask_path = tmp / '2.png'
    Image.fromarray(mask_arr).save(mask_path)
    base = {'group': 'verify', 'expect': 'ok'}
    # 残留正例：结果仍是带水印原图 → residual=True 且 verdict=FAIL（驱动自动重试）
    rep = rdw.verify_paths(dist2, dist2, mask_path, template_applied=True)
    rows.append(({**base, 'name': 'residual: unchanged(2.png)'},
                 bool(rep and rep.get('residual') and rep['verdict'] == 'FAIL'),
                 f"residual={rep and rep.get('residual')} verdict={rep and rep['verdict']}"))
    # 残留负例：结果是已去水印图 → residual=False（不误触发重试）
    rep = rdw.verify_paths(dist2, clean2, mask_path, template_applied=True)
    rows.append(({**base, 'name': 'residual: processed(2.png)'},
                 bool(rep and not rep.get('residual')),
                 f"residual={rep and rep.get('residual')} verdict={rep and rep['verdict']}"))
    # mask 膨胀：新增像素数应等于面积增量（自动重试用它扩 mask）
    before = int((np.array(Image.open(mask_path).convert('L')) > 0).sum())
    added = rdw._expand_mask(mask_path)
    after = int((np.array(Image.open(mask_path).convert('L')) > 0).sum())
    rows.append(({**base, 'name': 'expand mask grows'},
                 added > 0 and after == before + added, f'+{added}px {before}->{after}'))
    # 空 mask 膨胀保护（透传图不应产生 mask）
    empty = tmp / 'empty.png'
    Image.new('L', (50, 50), 0).save(empty)
    rows.append(({**base, 'name': 'expand empty mask noop'},
                 rdw._expand_mask(empty) == 0, 'noop'))
    # 自动重试编排（打桩 _retry_inpaint，避免真实推理）：残留图应被扩 mask 并复验
    import shutil as _sh
    work = tmp / 'work'
    for sub in ('source', 'masks', 'lama'):
        (work / sub).mkdir(parents=True, exist_ok=True)
    _sh.copy(dist2, work / 'source' / '2.png')
    Image.fromarray(mask_arr).save(work / 'masks' / '2.png')
    (work / 'masks' / '2.png.tpl').write_text('')
    _sh.copy(dist2, work / 'lama' / '2.png')  # 结果未修复 → 残留
    saved = (rdw.SOURCE, rdw.MASKS, rdw.LAMA)
    saved_retry = rdw._retry_inpaint
    rdw.SOURCE, rdw.MASKS, rdw.LAMA = work / 'source', work / 'masks', work / 'lama'
    rdw._retry_inpaint = lambda names, model: None  # 打桩：跳过真实推理
    try:
        before = int((np.array(Image.open(work / 'masks' / '2.png').convert('L')) > 0).sum())
        rdw._residual_retry(['2.png'], 'lama')
        after = int((np.array(Image.open(work / 'masks' / '2.png').convert('L')) > 0).sum())
    finally:
        rdw.SOURCE, rdw.MASKS, rdw.LAMA = saved
        rdw._retry_inpaint = saved_retry
    rows.append(({**base, 'name': 'auto-retry expands mask'},
                 after > before, f'{before}->{after}'))
    rmtree(tmp, ignore_errors=True)
    return rows


# ---------------------------------------------------------------------------
# L3：修复层（真实跑 CLI + 结果级验证）
# ---------------------------------------------------------------------------

def e2e_cases(rdw, swt):
    """端到端用真实豆包图（产品主路径，含纸面 3.png / 花墙 6.png / 缩放 4/5.png）；
    合成泛化场景的检测能力已在 L1 覆盖，不重复进模型。"""
    cases = []
    for path in sorted(DIST.glob('*.png')):
        if 'backup' in path.parts:
            continue
        cases.append({'name': path.name, 'img': Image.open(path).convert('RGB'), 'real': None})
    return cases


def run_e2e(rdw, swt, cases, model):
    tmp = Path(tempfile.mkdtemp(prefix='wm-regression-'))
    root = tmp / 'root'
    root.mkdir(parents=True)
    work = tmp / 'work'
    for c in cases:
        c['img'].save(root / c['name'])
    env = os.environ.copy()
    env['DOUBAO_WATERMARK_WORKDIR'] = str(work)
    cmd = [str(rdw.VENV_PYTHON), str(TOOLS / 'remove_doubao_watermark.py'),
           '--root', str(root), '--model', model, '--keep-work', 'run',
           *[c['name'] for c in cases]]
    print(f'\n[e2e] {" ".join(cmd[1:])}')
    proc = subprocess.run(cmd, env=env, capture_output=True, text=True)
    tail = (proc.stdout or '').strip().splitlines()[-6:]
    for line in tail:
        print(f'[e2e] {line}')
    if proc.returncode != 0:
        err = (proc.stderr or '').strip().splitlines()[-6:]
        for line in err:
            print(f'[e2e:err] {line}')
    rows = []
    for c in cases:
        name = c['name']
        tpl_applied = (work / 'masks' / f'{name}.tpl').exists()
        report = rdw.verify_paths(root / 'original-watermark-backup' / name,
                                  work / 'lama' / name, work / 'masks' / name,
                                  template_applied=tpl_applied)
        detail = 'no output'
        ok = False
        if report is not None:
            detail = rdw.format_verify(report)
            ok = report['verdict'] != 'FAIL'
        # 合成水印另做残留检测（已知真实 bbox，与模板无关，泛化到非豆包文字水印）
        residual = ''
        if c['real'] is not None:
            res_path = work / 'lama' / name
            if res_path.exists():
                with Image.open(res_path) as im:
                    boxes = rdw.detect_watermark_boxes(im.convert('RGB'), extended=False)
                    w, h = im.size
                    boxes = [b for b in boxes if b[2] > w - 40 and b[3] > h - 40]
                    iou = max((swt.iou(b, c['real']) for b in boxes), default=0.0)
                residual = f', residual IoU {iou:.2f}'
                if iou > 0.3:
                    ok = False
        rows.append((c, ok, detail + residual))
    return rows, tmp


# ---------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(description='watermark processing regression corpus')
    ap.add_argument('--e2e', action='store_true', help='also run full pipeline + result verification (slow)')
    ap.add_argument('--model', default='lama', choices=['mat', 'lama'], help='model for --e2e (default lama)')
    ap.add_argument('--only', help='filter by group or name substring')
    args = ap.parse_args()

    import remove_doubao_watermark as rdw
    import synthetic_watermark_test as swt

    cases = build_cases(rdw, swt)
    rows = run_layer1(rdw, swt, cases, args.only)
    rows += run_verify_checks(rdw, args.only)

    print(f'{"layer":5s} {"case":40s} {"expect":6s} {"mark":5s} detail')
    fails = 0
    boundary_changed = 0
    for c, ok, detail in rows:
        if c.get('boundary'):
            mark = 'known' if ok else 'CHG'
            if not ok:
                boundary_changed += 1
            print(f'{"L1b":5s} {c["name"]:40s} {c["expect"]:6s} {mark:5s} {detail}')
            continue
        mark = 'ok' if ok else 'FAIL'
        if not ok:
            fails += 1
        print(f'{"L1":5s} {c["name"]:40s} {c["expect"]:6s} {mark:5s} {detail}')
    hard = [r for r in rows if not r[0].get('boundary')]
    print(f'\nL1: {len(hard) - fails}/{len(hard)} passed'
          + (f'  ({boundary_changed} known boundary changed)' if boundary_changed else ''))

    if args.e2e:
        e2e = e2e_cases(rdw, swt)
        erows, tmp = run_e2e(rdw, swt, e2e, args.model)
        efails = 0
        for c, ok, detail in erows:
            if not ok:
                efails += 1
            print(f'{"L3":5s} {c["name"]:38s} {"clean":6s} {"":5s} {"ok " if ok else "FAIL":3s} {detail}')
        print(f'\nL3: {len(erows) - efails}/{len(erows)} passed')
        print(f'e2e artifacts: {tmp}')
        fails += efails

    print(f'\nTOTAL: {"ALL PASS" if fails == 0 else f"{fails} FAILED"}')
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()

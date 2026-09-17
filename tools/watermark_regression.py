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
# dist/ 现在混装两种工具的原图：1-6 是豆包，7-10 是千问。豆包模板正样本只对
# 1-6 断言；千问图会偶然触发模板（7/8 高分）或低于阈值（9/10），都不是豆包语义。
NON_DOUBAO_DIST = {'7.png', '8.png', '9.png', '10.png'}


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
        if 'backup' in path.parts or path.name in NON_DOUBAO_DIST:
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


def run_auto_profile_checks(rdw, swt, only):
    """自动挑帧建档案（learn-auto）：同款水印×多背景，应自动选中对比度最高+最均匀
    的黑底帧，且用其 α 逆解其它背景时能把水印误差压到接近 0。"""
    import numpy as np
    import cv2
    from shutil import rmtree
    rows = []
    if only and 'autoprofile' not in only:
        return rows
    wprof = getattr(rdw, 'wprof', None)
    if wprof is None:
        return rows
    tmp = Path(tempfile.mkdtemp(prefix='wm-autoprof-'))
    old_dir = wprof.PROFILE_DIR
    wprof.PROFILE_DIR = tmp / 'profiles'
    try:
        w, h = 1600, 900
        clean = {}
        for kind in ['black', 'gradient', 'whitebg', 'photo:2']:
            clean[kind] = swt.make_background(kind, w, h)
            img, _ = swt.stamp_text(clean[kind], 'translucent', 'bottomright', 0.5)
            img.save(tmp / f'{kind.replace(":", "_")}.png')
        names = sorted(p.name for p in tmp.glob('*.png'))
        rdw.learn_auto(names, tmp, 'synthauto')
        prof = wprof.load_profile('synthauto')
        base = {'group': 'autoprofile', 'expect': 'ok'}
        bg = prof.extra.get('learn_report', {}).get('bg_median', [255, 255, 255])
        rows.append(({**base, 'name': 'picks dark/uniform frame'},
                     max(bg) < 80, f'bg_median={bg}'))

        gray_obs = np.array(Image.open(tmp / 'gradient.png').convert('RGB')).astype(np.float32)
        truth = np.array(clean['gradient']).astype(np.float32)
        gray = gray_obs.max(2)
        kk = max(3, min(31, (min(h, w) - 1) | 1))
        top = gray - cv2.morphologyEx(gray.astype(np.uint8), cv2.MORPH_OPEN,
                                      cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (kk, kk))).astype(np.float32)
        top = (top - top.mean()) / (top.std() + 1e-6)
        best = None
        for sc in np.arange(0.97, 1.04, 0.005):
            a = wprof.scaled_layers(prof, sc)[0]
            th, tw = a.shape
            t = (a - a.mean()).astype(np.float32)
            t /= (t.std() + 1e-6)
            r = cv2.matchTemplate(top, t, cv2.TM_CCOEFF_NORMED)
            _, mx, _, loc = cv2.minMaxLoc(r)
            if best is None or mx > best[0]:
                best = (mx, sc, int(loc[0]), int(loc[1]))
        mx, sc, px, py = best
        a = wprof.scaled_layers(prof, sc)[0]
        th, tw = a.shape
        win = gray_obs[py:py + th, px:px + tw]
        ref = truth[py:py + th, px:px + tw]
        inv = np.clip((win - (a * 255.0)[..., None]) / np.maximum(1 - a[..., None], 1e-3), 0, 255)
        m = a > 0.05
        err_inv = float(np.abs(inv - ref).mean(2)[m].mean())
        err_obs = float(np.abs(win - ref).mean(2)[m].mean())
        rows.append(({**base, 'name': 'inverse generalizes (gradient)'},
                     err_inv < 5.0 and err_inv < err_obs * 0.4,
                     f'err {err_obs:.1f}->{err_inv:.1f} NCC {mx:.2f}'))
        rmtree(tmp, ignore_errors=True)
    finally:
        wprof.PROFILE_DIR = old_dir
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
    mask_arr, _, info = rdw.template_stroke_mask(gray, w, h)
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
    # mask 必须覆盖水印完整 footprint（含暗色描边），而非仅亮字核心：只盖亮字的 mask
    # 会在低对比背景（木纹/纸面）MAT 重绘后留暗字形残影，而 gap-score 判据测不到暗
    # 描边会误判 PASS（2.png/4.png 的真实回归）。这条锁死"mask 来源必须是含描边的
    # stamp footprint"，防止日后又退回只用模板亮字 α。
    import re as _re
    import cv2 as _cv2
    m_pos = _re.search(r'at \((\d+),(\d+)\)', info or '')
    tpl, alpha, meta = rdw.load_template()
    stamp_a, _ = rdw._load_stamp()
    if m_pos and stamp_a is not None and alpha is not None:
        px, py = int(m_pos.group(1)), int(m_pos.group(2))
        scale = min(h, w) / meta['ref_short_side']
        th = int(round(stamp_a.shape[0] * scale))
        tw = int(round(stamp_a.shape[1] * scale))
        sa = _cv2.resize(stamp_a, (tw, th), interpolation=_cv2.INTER_LINEAR)
        ta = _cv2.resize(alpha, (tw, th), interpolation=_cv2.INTER_LINEAR)
        # 描边区 = stamp footprint 有、模板亮字 α 没有的像素
        outline = ((sa * 255.0 > rdw.STAMP_MASK_THRESHOLD)
                   & (ta * 255.0 <= rdw.TEMPLATE_ALPHA_THRESHOLD))
        sub = mask_arr[py:py + th, px:px + tw] > 0
        cov = float(sub[outline].mean()) if outline.any() else 1.0
        rows.append(({'group': 'footprint', 'name': 'mask covers dark outline',
                      'expect': 'ok'},
                     bool(outline.any()) and cov >= 0.9,
                     f'outline {int(outline.sum())}px covered {cov * 100:.1f}%'))
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
        if 'backup' in path.parts or path.name in NON_DOUBAO_DIST:
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
    rows += run_auto_profile_checks(rdw, swt, args.only)

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

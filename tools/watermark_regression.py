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
    # 逆解成功的图不参与 MAT 重试（_residual_retry skip）：逆解是精确物理恢复，
    # 用生成式 MAT 重试覆盖它会把恢复的真实纹理重新糊掉（6.png 真实回归：橙墙
    # 黑斑纹理被逆解恢复后又被 retry 的 MAT 抹平）。skip 命中时不得触发重试。
    retry_calls = []
    saved_retry2 = rdw._retry_inpaint
    rdw._retry_inpaint = lambda names, model: retry_calls.append(list(names))
    try:
        rdw._residual_retry(['2.png'], 'lama', skip={'2.png'})
    finally:
        rdw._retry_inpaint = saved_retry2
    rows.append(({**base, 'name': 'inverse result skips MAT retry'},
                 retry_calls == [], f'retry calls {len(retry_calls)}'))
    # mask 必须覆盖水印完整 footprint（含暗色描边），而非仅亮字核心：只盖亮字的 mask
    # 会在低对比背景（木纹/纸面）MAT 重绘后留暗字形残影，而 gap-score 判据测不到暗
    # 描边会误判 PASS（2.png/4.png 的真实回归）。这条锁死"mask 来源必须是含描边的
    # stamp footprint"，防止日后又退回只用模板亮字 α。
    import re as _re
    import cv2 as _cv2
    m_pos = _re.search(r'at \((\d+),(\d+)\)', info or '')
    tpl, alpha, meta = rdw.load_template()
    stamp_a, stamp_color = rdw._load_stamp()
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
        # 阈值 0.75：mask 与测试用的 stamp 缩放路径一致，但 OPEN 会削掉描边最外缘，
        # 实测 88~93%；而退回"仅亮字"时覆盖骤降到 ~41%，0.75 有足够判别裕度。
        rows.append(({'group': 'footprint', 'name': 'mask covers dark outline',
                      'expect': 'ok'},
                     bool(outline.any()) and cov >= 0.75,
                     f'outline {int(outline.sum())}px covered {cov * 100:.1f}%'))
    # 统一尝试逆解 + 结果择优（配置守卫）：gating 不得退回"纹理门槛"硬预判，否则
    # 1-3.png 根本进不了择优流程，又变成一刀切；尺度容差须覆盖 1728 竖图（scale 1.08）。
    rows.append(({**base, 'name': 'inverse tries all (no hf gate)'},
                 rdw.INVERSE_TEXTURE_MIN <= 0.0 and rdw.INVERSE_SCALE_TOL >= 0.1,
                 f'TEXTURE_MIN={rdw.INVERSE_TEXTURE_MIN} SCALE_TOL={rdw.INVERSE_SCALE_TOL}'))
    # 结果择优必须可用：逆解残留 ≤ MAT×tol 才采用，否则落回 MAT（防 4/5.png 逆解退步）
    rows.append(({**base, 'name': 'inverse vs MAT compared'},
                 float(getattr(rdw, 'INVERSE_RESIDUAL_TOLERANCE', 0)) > 1.0
                 and callable(getattr(rdw, '_result_template_score', None)),
                 f'tol={getattr(rdw, "INVERSE_RESIDUAL_TOLERANCE", None)}'))
    # 逐图墨色标定（关键能力，2026-09 根治"3.png 周边元素被影响"）：stamp 的 α/C 是跨图
    # 标定值，单图实际墨色 C_true 有偏差 → 逆解残差 ∝[α/(1−α)]·ΔC，在 α 高处放大成颜色
    # 鬼影（3.png 实测金色字形）。用 MAT 低频（生成式、无鬼影）为参考做最小二乘标定后
    # 应消除，同时保住真实纹理。合成"已知背景 + 偏色墨色"验证：标定后须远优于未标定。
    if stamp_a is not None and stamp_color is not None:
        import numpy as _np2
        wf, hf_ = 2848, 1600
        yy, xx = _np2.mgrid[0:hf_, 0:wf]
        bg_f = _np2.stack([205.0 + 0.015 * xx, 195.0 + 0.010 * yy, 150.0 + 0.012 * xx],
                          axis=2).astype(_np2.float32)
        bg_f += _np2.random.RandomState(0).normal(0, 0.4, bg_f.shape).astype(_np2.float32)
        px, py = 2541, 1490
        th_f, tw_f = stamp_a.shape[0], stamp_a.shape[1]
        a0_f = _np2.clip(stamp_a, 0, 1)
        c_est = stamp_color.astype(_np2.float32)
        # 真实墨色 = 假设值 + 偏移（取负避免 >255 截断）：未标定逆解会留 ∝[α/(1−α)]·ΔC 的
        # 颜色鬼影，标定后应消除。
        delta = _np2.array([0.0, -12.0, -30.0], _np2.float32)
        bg_win = bg_f[py:py + th_f, px:px + tw_f]
        obs_f = bg_f.copy()
        obs_f[py:py + th_f, px:px + tw_f] = (a0_f[..., None] * (c_est + delta)
                                             + (1 - a0_f[..., None]) * bg_win)
        f_obs, f_mat = tmp / 'calib-obs.png', tmp / 'calib-mat.png'
        Image.fromarray(_np2.clip(obs_f, 0, 255).astype(_np2.uint8)).save(f_obs)
        Image.fromarray(_np2.clip(bg_f, 0, 255).astype(_np2.uint8)).save(f_mat)
        res_c = rdw.inverse_apply(str(f_obs), str(f_mat))
        if res_c is not None:
            out_c = res_c[0].astype(_np2.float32)[py:py + th_f, px:px + tw_f]
            obs_win = obs_f[py:py + th_f, px:px + tw_f]
            core = a0_f > 0.4  # 不透明核：颜色鬼影最强处
            inv_raw = _np2.clip((obs_win - a0_f[..., None] * c_est)
                                / _np2.maximum(1 - a0_f[..., None], 1e-3), 0, 255)
            err_c = float(_np2.abs(out_c - bg_win)[core].mean())
            err_raw = float(_np2.abs(inv_raw - bg_win)[core].mean())
            rows.append(({'group': 'calib', 'name': 'per-image ink calibration (kills color ghost)',
                          'expect': 'ok'},
                         bool(err_c < 2.0 and err_c < err_raw * 0.3),
                         f'core err calib {err_c:.2f} < uncalib {err_raw:.2f}'))
    # 泛化防回归：逐图墨色标定必须对**任意背景**成立（水印可能落在渐变/强纹理/硬
    # 边缘上）。同一"已知背景 + 偏色墨色"合成在三种强结构背景上验证：标定后核心区
    # 须远优于未标定，且不得为某张图特调——修复须通用。锁死这条，防止日后又退回
    # "针对特定图片"的救火式修补。
    if stamp_a is not None and stamp_color is not None:
        wg, hg = 2848, 1600
        yyg, xxg = _np2.mgrid[0:hg, 0:wg]
        bgs = {
            'gradient': _np2.stack([205 + 0.015 * xxg, 195 + 0.010 * yyg, 150 + 0.012 * xxg], 2),
            'texture': _np2.clip(
                _np2.stack([180 + 0.01 * xxg, 170 + 0.008 * yyg, 140 + 0.01 * xxg], 2)
                + _np2.random.RandomState(0).normal(0, 18, (hg, wg, 3))
                + (20 * _np2.sin(xxg / 7.0))[..., None], 0, 255),
            'edge': _np2.where(
                ((xxg + 0.6 * yyg - 2600) > 0)[..., None],
                _np2.stack([210 + 0 * xxg, 200 + 0 * yyg, 160 + 0 * xxg], 2),
                _np2.stack([40 + 0 * xxg, 38 + 0 * yyg, 30 + 0 * xxg], 2)),
        }
        pxg, pyg = 2541, 1490
        thg, twg = stamp_a.shape[0], stamp_a.shape[1]
        for bname, bg_g in bgs.items():
            bg_g = bg_g.astype(_np2.float32)
            bw_g = bg_g[pyg:pyg + thg, pxg:pxg + twg]
            obs_g = bg_g.copy()
            obs_g[pyg:pyg + thg, pxg:pxg + twg] = (a0_f[..., None] * (c_est + delta)
                                                   + (1 - a0_f[..., None]) * bw_g)
            f_og, f_mg = tmp / f'gen-{bname}-obs.png', tmp / f'gen-{bname}-mat.png'
            Image.fromarray(_np2.clip(obs_g, 0, 255).astype(_np2.uint8)).save(f_og)
            Image.fromarray(_np2.clip(bg_g, 0, 255).astype(_np2.uint8)).save(f_mg)
            res_g = rdw.inverse_apply(str(f_og), str(f_mg))
            if res_g is None:
                rows.append(({'group': 'calib', 'name': f'inverse generalizes ({bname})',
                              'expect': 'ok'}, False, 'no inverse'))
                continue
            ow_g = res_g[0].astype(_np2.float32)[pyg:pyg + thg, pxg:pxg + twg]
            inv_rg = _np2.clip((bw_g - a0_f[..., None] * c_est)
                               / _np2.maximum(1 - a0_f[..., None], 1e-3), 0, 255)
            ec_g = float(_np2.abs(ow_g - bw_g)[core].mean())
            er_g = float(_np2.abs(inv_rg - bw_g)[core].mean())
            rows.append(({'group': 'calib', 'name': f'inverse generalizes ({bname})',
                          'expect': 'ok'},
                         bool(ec_g < 2.5 and ec_g < er_g * 0.15),
                         f'core err {ec_g:.2f} < uncalib {er_g:.2f}'))
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

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
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from shutil import copyfile, rmtree

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

# ---------------------------------------------------------------------------
# 固化 fixtures：回归**只读** tests/fixtures，绝不读可变的工作目录（根目录 *.png）。
# 目的：新增/替换图片不再静默改变回归基线（"换一张图就要重修老图"的结构性根因）。
#   - tests/fixtures/manifest.json：全部原图的 md5/尺寸/模板分/掩码框基线；
#   - tests/fixtures/images/{original,processed}/：入库（CI 可跑）的精选子集；
#   - 本地跑时 original 可回退 dist/（md5 校验），processed 只认入库文件；
#   - md5 不符 = 该输入被换过 → **直接报错**，逼你显式 `--update-golden`。
# ---------------------------------------------------------------------------
FIXTURES = ROOT / 'tests' / 'fixtures'
FIXTURE_MANIFEST = FIXTURES / 'manifest.json'
FIXTURE_IMAGES = FIXTURES / 'images'
FIXTURE_VERSION = 1
# 入库的精选子集（见 --update-golden）：正样本覆盖地毯/岩石/纸面/木纹/花丛/沙地等
# 典型背景；负样本为对应已去水印成品。CI 只跑入库子集；本地可回退 dist/ 跑全量。
FIXTURE_CORE_ORIGINALS = ('1.png', '2.png', '3.png', '4.png', '6.png', '7.png', '22.png')
FIXTURE_CORE_PROCESSED = ('1.png', '3.png', '6.png')


def load_manifest():
    if not FIXTURE_MANIFEST.exists():
        return {'version': FIXTURE_VERSION, 'originals': {}, 'processed': {}}
    return json.loads(FIXTURE_MANIFEST.read_text())


def probe(rdw, path):
    """探测一张图的检测指纹：md5 / 尺寸 / 豆包模板分 / 掩码框。"""
    import numpy as np
    with Image.open(path) as im:
        rgb = im.convert('RGB')
        w, h = rgb.size
        gray = np.array(rgb).max(axis=2).astype(np.float32)
    mask, score, _info = rdw.template_stroke_mask(gray, w, h)
    box = None
    if mask is not None:
        ys, xs = np.where(mask > 0)
        box = [int(xs.min()), int(ys.min()), int(xs.max() + 1), int(ys.max() + 1)]
    return {'md5': md5(path), 'w': w, 'h': h, 'tmpl': round(float(score), 1), 'box': box}


def fixture_path(kind, name, manifest):
    """解析 fixture 文件：仓内 images/<kind>/ 优先；original 可回退本地 dist/。
    命中即校验 md5——**不符直接报错**，杜绝"换图"静默改变回归基线。"""
    entry = manifest.get(kind, {}).get(name)
    fx = FIXTURE_IMAGES / kind / name
    fallback = (DIST / name) if kind == 'original' else None
    for p in (fx, fallback):
        if p is not None and p.exists():
            if entry and entry.get('md5') and md5(p) != entry['md5']:
                raise SystemExit(
                    f'[fixture] {kind}/{name} 与 manifest 的 md5 不符：{p}\n'
                    f'  manifest {entry["md5"]}\n  实际     {md5(p)}\n'
                    '  该输入被替换/改动过——若确属有意，请跑 --update-golden 显式更新基线。')
            return p
    return None



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
    """豆包模板是否命中（正样本应命中、负样本应不命中）；附 mask 框供金标准比对。"""
    import numpy as np
    with Image.open(path) as im:
        rgb = im.convert('RGB')
        w, h = rgb.size
        gray = np.array(rgb).max(axis=2).astype(np.float32)
    mask, score, _ = rdw.template_stroke_mask(gray, w, h)
    box = None
    if mask is not None:
        ys, xs = np.where(mask > 0)
        if len(xs):
            box = [int(xs.min()), int(ys.min()), int(xs.max() + 1), int(ys.max() + 1)]
    return score >= rdw.TEMPLATE_MIN_SCORE, score, box


def looks_processed(rdw, path):
    """根目录图是否可当作"已去水印图"：与 dist 原图不同 **且** 豆包模板不再命中。

    仅凭 md5 不同不够——dist 若与根目录同步为带水印原图、或用户直接往根目录放入带水印
    新图，md5 虽不同但图其实仍带水印，此时把它当"干净背景/已处理图"会令"不应误检"负例
    假 FAIL（实测：dist 与原图同步后 clean:3/no-watermark、residual: processed 两个负例
    被真实水印触发）。这里用贴合字形的模板命中做判据——它走的是模板路径，与被测的兜底
    检测不同，不构成循环依赖。"""
    import numpy as np
    ref = DIST / path.name
    if not (ref.exists() and md5(path) != md5(ref)):
        return False
    try:
        with Image.open(path) as im:
            rgb = im.convert('RGB')
            gray = np.array(rgb).max(axis=2).astype(np.float32)
            _, score, _ = rdw.template_stroke_mask(gray, rgb.size[0], rgb.size[1])
    except Exception:
        return False
    return score < rdw.TEMPLATE_MIN_SCORE


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
    manifest = load_manifest()
    cases = []
    # A. 真实豆包正样本（带水印原图）：应命中模板。读 fixtures（入库子集），其余回退本地
    #    dist/ 并校验 md5——"换图"会直接报错而非静默改基线。
    for name, entry in sorted(manifest['originals'].items()):
        if not entry.get('doubao'):
            continue
        path = fixture_path('original', name, manifest)
        if path is None:
            continue
        cases.append({'group': 'doubao', 'name': f'original/{name}', 'path': path,
                      'layer': 1, 'expect': 'hit', 'golden': entry})
    # B. 真实豆包负样本（已去水印图，入库 fixture）：不应命中（防误检）
    clean = []
    for name in sorted(manifest['processed']):
        path = fixture_path('processed', name, manifest)
        if path is None:
            continue
        clean.append(path)
        cases.append({'group': 'doubao-neg', 'name': f'{name} (processed)',
                      'path': path, 'layer': 1, 'expect': 'miss',
                      'golden': manifest['processed'][name]})

    w, h = 1600, 900
    # 干净背景：用已去水印的 fixture【裁切】（不缩放，避免重采样伪影导致误检）
    def bg(kind):
        if kind.startswith('clean:'):
            p = fixture_path('processed', f'{kind.split(":", 1)[1]}.png', manifest)
            if p is None:
                raise FileNotFoundError(kind)
            with Image.open(p) as im:
                im = im.convert('RGB')
                return im.crop((0, 0, w, h)) if im.width >= w and im.height >= h else im.resize((w, h))
        return swt.make_background(kind, w, h)

    # E1 合成负例的"照片背景"池：排除 clean:3 的左上 1600x900 裁片。该裁片恰好从 3.png
    # 画面里的真实深色文字行（"HUMAN BOND"）中间切开，使该行贴到裁片底边并被
    # `_detect_corner_faded` 的右下兜底当成水印（生产路径用**全图**，见 E2 的 3.png，不误检
    # ——属兜底检测在"裁片尺度"下的已知弱点，非本次改动引入）。显式登记而非悄悄换背景。
    CROP_CUT_TEXT_BGS = {'clean:3'}
    photo_bgs = [f'clean:{p.stem}' for p in clean if f'clean:{p.stem}' not in CROP_CUT_TEXT_BGS][:3]
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
    # E. 支持能力：无水印纯背景/干净照片不应误检（干净照片仅取确实已去水印者）
    for bgk in ['black', 'whitebg', 'gradient', *photo_bgs]:
        try:
            img = bg(bgk)
        except FileNotFoundError:
            continue
        cases.append({'group': 'synth-neg', 'name': f'{bgk}/no-watermark',
                      'img': img, 'real': None,
                      'any_position': False, 'layer': 1, 'expect': 'miss'})
    # E2. 已去水印图（整图，默认模式只留右下角框）不得误检——防"兜底检测把右下角
    #     背景碎块当字形硬修"回归。旧判据字符高下限 1.2%*H 太低，地毯 1/2.png、纸面
    #     3.png、花丛 6.png 的背景碎块被聚成"字符行"硬修；收紧到 2.6%~5.0% 短边后
    #     应回归"跳过"。用整图（而非裁角）复现：检测窗按图幅比例，裁角会改变场景。
    for path in clean:
        try:
            with Image.open(path) as fp:
                im = fp.convert('RGB')
        except OSError:
            continue
        cases.append({'group': 'synth-neg', 'name': f'{path.name} (processed)',
                      'img': im, 'real': None,
                      'any_position': False, 'layer': 1, 'expect': 'miss'})
    # E3. 合成"已去水印图"右下角背景碎块不得误检：不依赖 dist/根目录，CI 恒可跑。
    #     碎块字形高 26px（1.63% 短边）低于字符高下限 2.6%（41.6px），收紧前旧判据
    #     下限 1.2%*H 会把它聚成"字符行"误检硬修；与 Rust 同名单测同几何。
    try:
        import numpy as np
        W, H = 2848, 1600
        arr = np.tile(np.array([236, 233, 228], np.uint8), (H, W, 1))
        arr[::53, :] = np.array([226, 222, 216], np.uint8)
        y1 = H - 45
        for i in range(6):
            x1 = W - 40 - 30 * i - 22
            arr[y1:y1 + 26, x1:x1 + 22] = np.array([250, 248, 244], np.uint8)
        cases.append({'group': 'synth-neg', 'name': 'specks/bottomright (processed-like)',
                      'img': Image.fromarray(arr), 'real': None,
                      'any_position': False, 'layer': 1, 'expect': 'miss'})
    except ImportError:
        pass
    # F. 已知边界（物理低对比 / busy photo 误检；不算回归失败，只锁定现状）
    for bgk, color in [('gradient', 'pale'), ('whitebg', 'pale')]:
        try:
            img, real = swt.stamp_text(bg(bgk), color, 'bottomright', 1.0)
        except FileNotFoundError:
            continue
        cases.append({'group': 'boundary', 'name': f'{bgk}/{color} (low-contrast)',
                      'img': img, 'real': real, 'any_position': False,
                      'layer': 1, 'expect': 'miss', 'boundary': True})
    # clean:3 上的淡字（220）原先被同图背景碎块干扰、融合框偏大而 miss；字符高带
    # 收紧后碎块被排除，检出框回到淡字本体（IoU 0.43）→ 转正为应命中。
    if fixture_path('processed', '3.png', manifest) is not None:
        try:
            img, real = swt.stamp_text(bg('clean:3'), 'pale', 'bottomright', 1.0)
            cases.append({'group': 'synth-corner', 'name': 'clean:3/pale (low-contrast)',
                          'img': img, 'real': real, 'any_position': False,
                          'layer': 1, 'expect': 'hit'})
        except FileNotFoundError:
            pass
    return cases


def run_layer1(rdw, swt, cases, only):
    rows = []
    for c in cases:
        if only and only not in c['group'] and only not in c['name']:
            continue
        if c['group'].startswith('doubao'):
            hit, score, box = eval_doubao(rdw, c['path'])
            ok = hit == (c['expect'] == 'hit')
            detail = f'tmpl score {score:.1f}'
            gold = c.get('golden') or {}
            gt = gold.get('tmpl')
            if gt is not None:
                tol = max(3.0, 0.25 * float(gt))
                if abs(score - float(gt)) > tol:
                    ok = False
                    detail += f' OUT-OF-GOLDEN {float(gt):.1f}±{tol:.1f}'
                else:
                    detail += f' (golden {float(gt):.1f})'
            gb = gold.get('box')
            if gb and box:
                if swt.iou(box, gb) < 0.5:
                    ok = False
                    detail += f' box moved {box} vs golden {gb}'
            elif gb and not box:
                ok = False
                detail += f' box missing (golden {gb})'
            rows.append((c, ok, detail))
            continue
        iou = eval_synth(rdw, swt, c['img'], c['real'], c['any_position'])
        hit = iou > 0.3
        detail = f'max IoU {iou:.2f}'
        rows.append((c, hit == (c['expect'] == 'hit'), detail))
    return rows


def run_lowcontrast_check(rdw, only):
    """低对比盲区：背景亮块尺度≈顶帽核时，模板 gap-score 必须**同时**用 raw 与顶帽
    两种口径取强者（`template_stroke_mask` 的 max 规则）。

    只算顶帽口径时，岩石/树皮这类背景（亮块尺度大于顶帽核、顶帽跟不上）会把水印
    gap 压到阈值下：真实图 2.png 顶帽 19.9 < 20（水印漏检、完全没去除），raw 22.3
    仍可判。这里用随机大亮块背景 + 豆包 stamp 合成复现同一机制（不依赖 dist）：
    修复前顶帽口径 19.4 < 20 → FAIL，修复后 max 22.0 → PASS。"""
    import numpy as np
    import cv2
    if only and 'lowcontrast' not in only:
        return []
    tpl, _alpha, meta = rdw.load_template()
    stamp, _ = rdw._load_stamp()
    if tpl is None or stamp is None:
        return []
    h, w = 900, 1600
    scale = min(h, w) / meta['ref_short_side']
    sh = max(1, int(round(stamp.shape[0] * scale)))
    sw = max(1, int(round(stamp.shape[1] * scale)))
    sa = cv2.resize(stamp, (sw, sh), interpolation=cv2.INTER_LINEAR)
    rng = np.random.default_rng(3)
    blk = 30  # 亮块尺度≈顶帽核(31)：顶帽无法把背景压平
    small = rng.random((h // blk + 1, w // blk + 1)).astype(np.float32)
    big = cv2.resize(small, (w, h), interpolation=cv2.INTER_CUBIC)
    big = (big - big.min()) / (big.max() - big.min() + 1e-6)
    lo, hi = 188, 245
    img = (lo + big * (hi - lo))[..., None] * np.array([1.0, 0.92, 0.80], np.float32)
    x1, y1 = w - sw - 8, h - sh - 8
    region = img[y1:y1 + sh, x1:x1 + sw]
    a = sa[..., None]
    img[y1:y1 + sh, x1:x1 + sw] = region * (1 - a) + 255.0 * a
    gray = np.clip(img, 0, 255).astype(np.uint8).max(axis=2).astype(np.float32)
    _mask, score, _info = rdw.template_stroke_mask(gray, w, h)
    case = {'group': 'doubao-lowcontrast', 'name': 'big-blocks/white', 'expect': 'hit'}
    return [(case, score >= rdw.TEMPLATE_MIN_SCORE, f'tmpl score {score:.1f}')]


def run_inverse_quality_checks(rdw, only):
    """逆解质量闸门（gap-score 测不到的盲区）：

    ① **过冲**（减过头留暗字形）：1.png 地毯案例字形区比间隙暗 22，模板 gap-score 只查
       "字形偏亮"（残留）故漏网；`_result_overshoot_score` 必须能把它判出来（< −阈值）。
    ② 正常图（字形区与间隙区同分布）不得误判过冲。"""
    import numpy as np
    if only and 'inverse-quality' not in only:
        return []
    stamp, _ = rdw._load_stamp()
    if stamp is None:
        return []
    px, py = 2541, 1490
    h, w = 1600, 2848
    sh, sw = stamp.shape
    rng = np.random.default_rng(11)
    base = (rng.random((h, w, 3)) * 30 + 150).astype(np.float32)
    clean = rdw._result_overshoot_score(base, px, py)
    rows = [({'group': 'inverse-quality', 'name': 'overshoot: clean image', 'expect': 'ok'},
             clean is not None and clean > -rdw.INVERSE_OVERSHOOT_MAX,
             f'overshoot {clean:.1f}' if clean is not None else 'n/a')]
    bad = base.copy()
    reg = bad[py:py + sh, px:px + sw]
    reg[stamp > 0.5] -= 40.0  # 模拟"减过头"：字形核心被压暗
    over = rdw._result_overshoot_score(bad, px, py)
    rows.append(({'group': 'inverse-quality', 'name': 'overshoot: darkened glyph', 'expect': 'reject'},
                 over is not None and over < -rdw.INVERSE_OVERSHOOT_MAX,
                 f'overshoot {over:.1f}' if over is not None else 'n/a'))

    # ③ 墨色校正后必须 clip 回 [0,255]：`corr = inv − kf·ΔC`（kf=α/(1−α)）在高 α 处会越过
    #    0；漏 clip 时负值在 `out.astype(np.uint8)` 上**回绕**（−1→255），生成刺眼彩点
    #    （6.png 实测 [164,157,152]→[20,10,254]）。构造"深色纹理背景 + 白字水印 + 偏暗 MAT"
    #    把 ΔC 拉大、局部 corr 转负，锁死"字形核区不得出现比真实背景亮 >100 级"。
    import cv2
    scale = min(h, w) / rdw.STAMP_REF_SHORT
    th = int(round(stamp.shape[0] * scale))
    tw = int(round(stamp.shape[1] * scale))
    aa = cv2.resize(stamp, (tw, th), interpolation=cv2.INTER_LINEAR)[..., None]
    true_bg = (np.random.default_rng(5).random((th, tw, 3)) * 35 + 8).astype(np.float32)
    obs = np.full((h, w, 3), 30, np.float32)
    obs[py:py + th, px:px + tw] = true_bg * (1 - aa) + 255.0 * aa
    mat = np.full((h, w, 3), 20, np.float32)
    tmp = Path(tempfile.mkdtemp(prefix='wm-invclip-'))
    try:
        op, mp = tmp / 'o.png', tmp / 'm.png'
        Image.fromarray(obs.astype(np.uint8)).save(op)
        Image.fromarray(mat.astype(np.uint8)).save(mp)
        res = rdw.inverse_apply(str(op), str(mp))
        core = stamp > 0.5
        wrapped = None
        if res is not None:
            v = np.array(res[0])[py:py + th, px:px + tw][core].astype(np.float32)
            wrapped = int((v - true_bg[core] > 180).sum())
        rows.append(({'group': 'inverse-quality', 'name': 'ink-calib clip: no wrapped bright specks',
                      'expect': 'ok'},
                     wrapped == 0,
                     f'pixels >true_bg+180: {wrapped} (un-clipped wraps {989}; calib residual <=110)'
                     if wrapped is not None else 'n/a'))
    finally:
        rmtree(tmp, ignore_errors=True)
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
        # photo:2 是 dist 真实图；CI 无 dist 时跳过该帧（其余纯合成帧照跑）。
        kinds = ['black', 'gradient', 'whitebg']
        if (DIST / '2.png').exists():
            kinds.append('photo:2')
        for kind in kinds:
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


def run_stamp_checks(rdw, only):
    """无模型、**不依赖 dist/ 与根目录成品图**的资产自检：合成一张带豆包水印的图
    （模板可稳定命中），直接验证 mask footprint / 逐图墨色标定 / 逆解背景泛化 /
    MAT 低频带偏。CI 无 dist 也会真实执行——是"换任意新图不出颜色鬼影"的核心防线。"""
    import numpy as np
    import cv2
    rows = []
    if only and not any(k in only for k in ('stamp', 'footprint', 'calib', 'inverse', 'verify')):
        return rows
    stamp_a, stamp_color = rdw._load_stamp()
    if stamp_a is None or stamp_color is None:
        return rows
    _, alpha, meta = rdw.load_template()
    tmp = Path(tempfile.mkdtemp(prefix='wm-stamp-'))
    try:
        w, h = 2848, 1600
        px, py = 2541, 1490
        a0 = np.clip(stamp_a, 0, 1).astype(np.float32)
        c_est = stamp_color.astype(np.float32)
        th, tw = a0.shape
        yy, xx = np.mgrid[0:h, 0:w]
        gx = (150.0 + 0.01 * xx).astype(np.float32)
        mid = np.stack([gx, gx * 0.97, gx * 0.9], 2)
        synth = mid.copy()
        synth[py:py + th, px:px + tw] = (a0[..., None] * c_est
                                         + (1 - a0[..., None]) * mid[py:py + th, px:px + tw])
        synth_path = tmp / 'synth.png'
        Image.fromarray(np.clip(synth, 0, 255).astype(np.uint8)).save(synth_path)
        # 1) mask 必须覆盖完整 footprint（含暗描边），不能只盖亮字核心：只盖亮字的 mask
        # 会在低对比背景 MAT 重绘后留暗字形残影，而 gap-score 判据测不到暗描边会误判
        # PASS。这条锁死"mask 来源必须是含描边的 stamp footprint"（用合成图，免 dist）。
        gray = np.array(Image.open(synth_path)).max(2).astype(np.float32)
        mask_arr, _score, _info = rdw.template_stroke_mask(gray, w, h)
        if mask_arr is not None and alpha is not None:
            scale = min(h, w) / meta['ref_short_side']
            thm = int(round(stamp_a.shape[0] * scale))
            twm = int(round(stamp_a.shape[1] * scale))
            sa = cv2.resize(stamp_a, (twm, thm), interpolation=cv2.INTER_LINEAR)
            ta = cv2.resize(alpha, (twm, thm), interpolation=cv2.INTER_LINEAR)
            # 描边区 = stamp footprint 有、模板亮字 α 没有的像素
            outline = ((sa * 255.0 > rdw.STAMP_MASK_THRESHOLD)
                       & (ta * 255.0 <= rdw.TEMPLATE_ALPHA_THRESHOLD))
            sub = mask_arr[py:py + thm, px:px + twm] > 0
            cov = float(sub[outline].mean()) if outline.any() else 1.0
            # 阈值 0.75：mask 与测试用的 stamp 缩放路径一致，但 OPEN 会削掉描边最外缘，
            # 实测 88~93%；退回"仅亮字"时覆盖骤降到 ~41%，0.75 有足够判别裕度。
            rows.append(({'group': 'footprint', 'name': 'mask covers dark outline',
                          'expect': 'ok'},
                         bool(outline.any()) and cov >= 0.75,
                         f'outline {int(outline.sum())}px covered {cov * 100:.1f}%'))
        # 2) 配置守卫：不得退回"纹理门槛"硬预判逆解，择优须可用。
        rows.append(({'group': 'verify', 'name': 'inverse tries all (no hf gate)',
                      'expect': 'ok'},
                     rdw.INVERSE_TEXTURE_MIN <= 0.0 and rdw.INVERSE_SCALE_TOL >= 0.1,
                     f'TEXTURE_MIN={rdw.INVERSE_TEXTURE_MIN} SCALE_TOL={rdw.INVERSE_SCALE_TOL}'))
        rows.append(({'group': 'verify', 'name': 'inverse vs MAT compared',
                      'expect': 'ok'},
                     float(getattr(rdw, 'INVERSE_RESIDUAL_TOLERANCE', 0)) > 1.0
                     and callable(getattr(rdw, '_result_template_score', None)),
                     f'tol={getattr(rdw, "INVERSE_RESIDUAL_TOLERANCE", None)}'))

        core = a0 > 0.4  # 不透明核：颜色鬼影/纹理保持最敏感处
        delta = np.array([0.0, -12.0, -30.0], np.float32)

        def _inv_err(tag, bg, proxy):
            """合成 obs=α·(C+ΔC)+(1−α)·bg，以 proxy 为 MAT 参考跑逆解；返回
            (核心区误差, 未标定基线误差, 逆解核心区窗口)。"""
            bg = bg.astype(np.float32)
            bw = bg[py:py + th, px:px + tw]
            obs = bg.copy()
            obs[py:py + th, px:px + tw] = (a0[..., None] * (c_est + delta)
                                           + (1 - a0[..., None]) * bw)
            f_o, f_m = tmp / f'{tag}-obs.png', tmp / f'{tag}-mat.png'
            Image.fromarray(np.clip(obs, 0, 255).astype(np.uint8)).save(f_o)
            Image.fromarray(np.clip(proxy, 0, 255).astype(np.uint8)).save(f_m)
            inv_raw = np.clip((bw - a0[..., None] * c_est)
                              / np.maximum(1 - a0[..., None], 1e-3), 0, 255)
            raw = float(np.abs(inv_raw - bw)[core].mean())
            res = rdw.inverse_apply(str(f_o), str(f_m))
            if res is None:
                return None, raw, None
            ow = res[0].astype(np.float32)[py:py + th, px:px + tw]
            return float(np.abs(ow - bw)[core].mean()), raw, ow

        # 3) 逐图墨色标定（关键能力，2026-09 根治"3.png 周边元素被影响"）：stamp 的
        # α/C 是跨图标定值，单图墨色偏差 → 残差 ∝[α/(1−α)]·ΔC，在 α 高处放大成颜色
        # 鬼影（实测金色字形）。以 MAT 低频为参考最小二乘拟合每通道偏移后应消除。
        bg_f = np.stack([(205 + 0.015 * xx).astype(np.float32),
                         (195 + 0.010 * yy).astype(np.float32),
                         (150 + 0.012 * xx).astype(np.float32)], 2)
        bg_f += np.random.RandomState(0).normal(0, 0.4, bg_f.shape).astype(np.float32)
        ec, raw, _ = _inv_err('calib', bg_f, bg_f)
        if ec is not None:
            rows.append(({'group': 'calib', 'name': 'per-image ink calibration (kills color ghost)',
                          'expect': 'ok'},
                         bool(ec < 2.0 and ec < raw * 0.3),
                         f'core err calib {ec:.2f} < uncalib {raw:.2f}'))
        # 4) 泛化：标定必须对任意背景成立（渐变/强纹理/硬边缘），证明修复与背景无关、
        # 非针对特定图。
        gen_bgs = {
            'gradient': bg_f,
            'texture': np.clip(np.stack([(180 + 0.01 * xx).astype(np.float32),
                                         (170 + 0.008 * yy).astype(np.float32),
                                         (140 + 0.01 * xx).astype(np.float32)], 2)
                               + np.random.RandomState(0).normal(0, 18, (h, w, 3)).astype(np.float32)
                               + (20 * np.sin(xx / 7.0))[..., None].astype(np.float32), 0, 255),
            'edge': np.where(((xx + 0.6 * yy - 2600) > 0)[..., None],
                             np.stack([(210 + 0 * xx).astype(np.float32),
                                       (200 + 0 * yy).astype(np.float32),
                                       (160 + 0 * xx).astype(np.float32)], 2),
                             np.stack([(40 + 0 * xx).astype(np.float32),
                                       (38 + 0 * yy).astype(np.float32),
                                       (30 + 0 * xx).astype(np.float32)], 2)).astype(np.float32),
        }
        for bname, bgg in gen_bgs.items():
            ec, raw, _ = _inv_err(f'gen-{bname}', bgg, bgg)
            if ec is None:
                rows.append(({'group': 'calib', 'name': f'inverse generalizes ({bname})',
                              'expect': 'ok'}, False, 'no inverse'))
                continue
            rows.append(({'group': 'calib', 'name': f'inverse generalizes ({bname})',
                          'expect': 'ok'},
                         bool(ec < 2.5 and ec < raw * 0.15),
                         f'core err {ec:.2f} < uncalib {raw:.2f}'))
        # 5) MAT 低频带偏（CI 盲区补丁）：MAT 在强结构背景上会丢失高频、低频也可能带偏，
        # 故"拿真背景当 MAT"不足以代表真实交互。这里用真实 MAT 代理——结构丢失
        # (blur σ=12) + 字形低频偏移：标定后核心区仍须接近真背景（<8 且 <未标定×0.15），
        # 且**高频纹理须被保住**（逆解恢复、MAT 会抹平）。锁死"标定只借 MAT 的整体低频
        # 偏置，不继承其结构误差"，防止有人改用原图 MAT 或让标定拟合空间结构。
        tex = gen_bgs['texture']
        proxy_m = tex.copy()
        glyph = cv2.GaussianBlur(a0, (0, 0), 8.0)
        proxy_m[py:py + th, px:px + tw] = np.clip(
            cv2.GaussianBlur(tex[py:py + th, px:px + tw], (0, 0), 12.0)
            - glyph[..., None] * 10.0, 0, 255)
        ec, raw, ow = _inv_err('matbias', tex, proxy_m)
        if ec is None:
            rows.append(({'group': 'calib', 'name': 'inverse survives MAT low-freq bias',
                          'expect': 'ok'}, False, 'no inverse'))
        else:
            def _hf(x):
                return cv2.GaussianBlur(x, (0, 0), 1.5) - x
            s_out = float(_hf(ow)[core].mean(1).std())
            s_true = float(_hf(tex[py:py + th, px:px + tw])[core].mean(1).std())
            ratio = s_out / max(s_true, 1e-6)
            rows.append(({'group': 'calib', 'name': 'inverse survives MAT low-freq bias',
                          'expect': 'ok'},
                         bool(ec < 8.0 and ec < raw * 0.15 and ratio > 0.5),
                         f'core err {ec:.2f} < uncalib {raw:.2f}, tex kept x{ratio:.2f}'))
    finally:
        rmtree(tmp, ignore_errors=True)
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
    # 残留负例：结果是已去水印图 → residual=False（不误触发重试）。仅当根目录 2.png
    # 确实已去水印（dist 若与根目录同步为原图则不成立）才纳入，否则负例本身无效。
    if looks_processed(rdw, clean2):
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

def run_anchor_checks(rdw, only):
    """残留判据"锚定 + 主导峰"（无模型）：结果分取**源图水印锚点处**的分，且须
    ≥ TEMPLATE_RESIDUAL_DOMINANCE×角窗最大分。防"强纹理背景在角窗别处凑高分 → 已去
    干净的图被误判 FAIL 整批拒绝"回归（实测 1.png/6.png）；同时保证真残留（锚点处仍
    最强）依旧 FAIL。打桩 `_template_response` 精确构造"锚点低/别处高"与"锚点最高"。"""
    import numpy as np
    from shutil import rmtree
    rows = []
    if only and 'anchor' not in only:
        return rows
    tmp = Path(tempfile.mkdtemp(prefix='wm-anchor-'))
    saved = rdw._template_response
    try:
        w = h = 200
        img = np.full((h, w, 3), 128, np.uint8)
        op, rp, mp = tmp / 'o.png', tmp / 'r.png', tmp / 'm.png'
        Image.fromarray(img).save(op)
        Image.fromarray(img).save(rp)
        small = np.zeros((h, w), np.uint8)
        small[100:120, 100:120] = 255          # 小 mask，避开 mask 占比 WARN 干扰
        Image.fromarray(small).save(mp)

        def mk(anchor, other=None):
            r = np.zeros((60, 60), np.float32)
            r[30, 30] = anchor          # 落在角窗内（窗=最后 41 行/列，索引 19..59）
            if other is not None:
                r[22, 22] = other       # 角窗别处（背景巧合），仍在窗内
            return (r, 40, 40)

        def run(seq):
            it = list(seq)
            rdw._template_response = lambda *a, **k: it.pop(0)
            return rdw.verify_paths(op, rp, mp, template_applied=True)

        # ① 结果锚点分低(10)、角窗别处高(40) → 不得 FAIL（旧口径会误判 FAIL）
        rep = run([mk(60.0), mk(10.0, 40.0)])
        rows.append(({'group': 'anchor', 'name': 'off-anchor background peak not FAIL', 'expect': 'ok'},
                     bool(rep and rep['verdict'] == 'PASS' and not rep['residual']),
                     f"verdict={rep and rep['verdict']} residual={rep and rep['residual']} "
                     f"anchor={rep and rep['res_template_score']} win={rep and rep.get('res_template_window_max')}"))
        # ② 结果锚点处仍最强(30) → 真残留必须 FAIL
        rep2 = run([mk(60.0), mk(30.0)])
        rows.append(({'group': 'anchor', 'name': 'anchored residual still FAIL', 'expect': 'ok'},
                     bool(rep2 and rep2['verdict'] == 'FAIL' and rep2['residual']),
                     f"verdict={rep2 and rep2['verdict']} residual={rep2 and rep2['residual']} "
                     f"anchor={rep2 and rep2['res_template_score']} win={rep2 and rep2.get('res_template_window_max')}"))
    finally:
        rdw._template_response = saved
        rmtree(tmp, ignore_errors=True)
    return rows


def run_skip_checks(rdw, only):
    """落盘逐张判定（无模型）：overwrite-review 对 FAIL 的图**保留原图**、其余照写——
    旧行为是"任何一张 FAIL 就整批拒绝、一张都不写"，单张复杂背景的模板残留误报会让整批
    看似"跑了没用、水印还在"。这里用打桩的 verify_repaired 直接验证编排（good 落盘、
    bad 保留 + 计入 skipped）。"""
    import json as _json
    import numpy as np
    from shutil import rmtree
    rows = []
    if only and 'skip' not in only:
        return rows
    tmp = Path(tempfile.mkdtemp(prefix='wm-skip-'))
    saved = (rdw.MANIFEST, rdw.LAMA, rdw.REVIEW, rdw.verify_repaired, rdw.report_verify, rdw.review)
    try:
        root = tmp / 'root'; lama = tmp / 'lama'; review = tmp / 'review'
        for d in (root, lama, review):
            d.mkdir(parents=True, exist_ok=True)
        rng = np.random.default_rng(4)
        orig = np.zeros((120, 160, 3), np.uint8)
        cleaned = rng.integers(0, 255, (120, 160, 3), dtype=np.uint8)
        manifest = {}
        for name, content in (('good.png', orig), ('bad.png', orig)):
            Image.fromarray(content).save(root / name)
            Image.fromarray(cleaned if name == 'good.png' else rng.integers(1, 9, (120, 160, 3), np.uint8)).save(lama / name)
            manifest[name] = md5(root / name)
        (tmp / 'manifest.json').write_text(_json.dumps(manifest))
        rdw.MANIFEST = tmp / 'manifest.json'
        rdw.LAMA = lama
        rdw.REVIEW = review
        rdw.verify_repaired = lambda name, _root: {'verdict': 'FAIL' if name == 'bad.png' else 'PASS', 'reasons': []}
        rdw.report_verify = lambda *a, **k: None
        rdw.review = lambda *a, **k: review
        _, skipped = rdw.overwrite_review(['good.png', 'bad.png'], root)
        good_written = np.array(Image.open(root / 'good.png').convert('RGB')).mean() > 20
        bad_kept = bool((np.array(Image.open(root / 'bad.png').convert('RGB')) == orig).all())
        rows.append(({'group': 'skip', 'name': 'overwrite skips only FAIL images', 'expect': 'ok'},
                     good_written and bad_kept and skipped == ['bad.png'],
                     f'good_written={good_written} bad_kept={bad_kept} skipped={skipped}'))
    finally:
        (rdw.MANIFEST, rdw.LAMA, rdw.REVIEW, rdw.verify_repaired,
         rdw.report_verify, rdw.review) = saved
        rmtree(tmp, ignore_errors=True)
    return rows


def run_crop_checks(rdw, only):
    """自裁小块推理（提速）的防回归（无模型）：
    ① `_crop_box` 必须把完整水印框进去且边长 ≤ CROP_SIDE_MAX（只有 ≤512 时 MAT 补齐后才是
       512²；一旦超过就会被补到 1024²，耗时非线性暴涨——实测同图 8s vs 90s）；
    ② 用 stub iopaint 验证 `_iopaint_batch` **只贴回 mask 像素**：mask 外逐字节不变
       （AGENTS 硬约束），mask 内取裁块结果。"""
    import numpy as np
    from shutil import rmtree
    rows = []
    if only and 'crop' not in only:
        return rows
    for (w, h, bw, bh) in [(2848, 1600, 287, 99), (1728, 2304, 316, 92), (1248, 1664, 303, 64)]:
        m = np.zeros((h, w), np.uint8)
        x1, y1 = w - bw - 15, h - bh - 20
        m[y1:y1 + bh, x1:x1 + bw] = 255
        box = rdw._crop_box(m, w, h)
        ok = (box is not None and box[0] <= x1 and box[1] <= y1
              and box[2] >= x1 + bw and box[3] >= y1 + bh
              and max(box[2] - box[0], box[3] - box[1]) <= rdw.CROP_SIDE_MAX)
        rows.append(({'group': 'crop', 'name': f'crop box {w}x{h} covers+<=512', 'expect': 'ok'},
                     ok, f'box={box}'))
    tmp = Path(tempfile.mkdtemp(prefix='wm-crop-'))
    old_work, old_sub = rdw.WORK, rdw.subprocess
    try:
        src, msk, out = tmp / 'source', tmp / 'masks', tmp / 'out'
        for d in (src, msk, out):
            d.mkdir(parents=True, exist_ok=True)
        rdw.WORK = tmp / 'work'
        rng = np.random.default_rng(0)
        base = rng.integers(0, 255, (900, 1400, 3), dtype=np.uint8)  # 大图 → 必走裁块
        mask = np.zeros((900, 1400), np.uint8)
        mask[700:760, 1000:1180] = 255
        Image.fromarray(base).save(src / 'x.png')
        Image.fromarray(mask).save(msk / 'x.png')
        calls = {}
        class _Fake:  # 只保留 run，供 _iopaint_batch 调
            @staticmethod
            def run(argv, **kw):
                img_dir = Path(argv[argv.index('--image') + 1])
                msk_dir = Path(argv[argv.index('--mask') + 1])
                out_dir = Path(argv[argv.index('--output') + 1])
                calls['crop_size'] = Image.open(img_dir / 'x.png').size
                arr = np.array(Image.open(img_dir / 'x.png').convert('RGB'))
                mk = np.array(Image.open(msk_dir / 'x.png').convert('L'))
                arr[mk > 0] = (0, 255, 0)  # stub：mask 内填纯绿
                Image.fromarray(arr).save(out_dir / 'x.png')
        rdw.subprocess = _Fake
        rdw._iopaint_batch(['x.png'], 'mat', src, msk, out)
        res = np.array(Image.open(out / 'x.png').convert('RGB'))
        inside = mask > 0
        outside_same = bool((res[~inside] == base[~inside]).all())
        inside_filled = bool((res[inside] == np.array([0, 255, 0])).all())
        cropped = calls.get('crop_size') is not None and max(calls['crop_size']) <= rdw.CROP_SIDE_MAX
        rows.append(({'group': 'crop', 'name': 'batch pastes mask only, crops <=512', 'expect': 'ok'},
                     outside_same and inside_filled and cropped,
                     f'outside_unchanged={outside_same} inside_filled={inside_filled} crop={calls.get("crop_size")}'))
    finally:
        rdw.WORK, rdw.subprocess = old_work, old_sub
        rmtree(tmp, ignore_errors=True)
    return rows


def run_sd_checks(rdw, only):
    """扩散修补（--sd）的防回归（无模型，打桩 pipe）：
    ① `_ring_hf` 只测**水印周边环带**、排除字形框本身——否则字形边缘也是高频、所有图都会
       被误判为"有纹理"而全走慢速 SD；
    ② `_sd_inpaint` 只对高纹理图路由、且只贴回 **mask 像素**（mask 外逐字节不变）。"""
    import numpy as np
    from shutil import rmtree
    rows = []
    if only and 'sd' not in only:
        return rows
    # ① 环带纹理判据
    box = np.zeros((400, 600), np.uint8)
    box[150:250, 250:450] = 255
    rng = np.random.default_rng(1)
    flat = np.full((400, 600, 3), 128, np.uint8)
    flat[150:250, 250:450] = rng.integers(0, 255, (100, 200, 3), dtype=np.uint8)  # 字形框内塞噪声
    noisy = rng.integers(0, 255, (400, 600, 3), dtype=np.uint8)
    hf_flat = rdw._ring_hf(flat, box)
    hf_noisy = rdw._ring_hf(noisy, box)
    rows.append(({'group': 'sd', 'name': 'ring_hf excludes glyph box', 'expect': 'ok'},
                 hf_flat < 3 < hf_noisy, f'flat={hf_flat:.2f} noisy={hf_noisy:.2f}'))

    # ② 路由 + 只贴 mask
    tmp = Path(tempfile.mkdtemp(prefix='wm-sd-'))
    old = (rdw.SOURCE, rdw.MASKS, rdw.LAMA, rdw._sd_pipe, rdw._sd_generator)
    try:
        src, msk, out = tmp / 'source', tmp / 'masks', tmp / 'lama'
        for d in (src, msk, out):
            d.mkdir(parents=True, exist_ok=True)
        tex = rng.integers(0, 255, (900, 1400, 3), dtype=np.uint8)  # 高纹理
        smooth = np.full((900, 1400, 3), 100, np.uint8)             # 平滑
        mask = np.zeros((900, 1400), np.uint8)
        mask[700:760, 1000:1180] = 255
        Image.fromarray(tex).save(src / 'tex.png')
        Image.fromarray(smooth).save(src / 'smooth.png')
        for n in ('tex.png', 'smooth.png'):
            Image.fromarray(mask).save(msk / n)
            Image.fromarray(np.full((900, 1400, 3), 50, np.uint8)).save(out / n)  # LAMA 底

        class _FakePipe:  # 返回纯 (0,255,0)，供断言"mask 内被 SD 覆盖"
            def __call__(self, **kw):
                w, h = kw['image'].size
                arr = np.zeros((h, w, 3), np.float32)  # diffusers output_type='np' → [0,1]
                arr[..., 1] = 1.0
                return type('R', (), {'images': [arr]})()

        rdw.SOURCE, rdw.MASKS, rdw.LAMA = src, msk, out
        rdw._sd_pipe = lambda: (_FakePipe(), 'cpu')
        rdw._sd_generator = lambda device, seed: None  # 免 torch（CI 无 torch）
        applied = rdw._sd_inpaint(['tex.png', 'smooth.png'], texture_min=rdw.SD_TEXTURE_MIN)
        rt = np.array(Image.open(out / 'tex.png').convert('RGB'))
        rs = np.array(Image.open(out / 'smooth.png').convert('RGB'))
        inside = mask > 0
        # 只贴 mask：mask 外应保持 LAMA 底（50），mask 内应为 stub 的纯绿
        tex_ok = (bool((rt[~inside] == 50).all())
                  and bool((rt[inside] == np.array([0, 255, 0])).all()))
        smooth_ok = bool((rs == 50).all())  # 平滑图未被路由、LAMA 原样
        rows.append(({'group': 'sd', 'name': 'sd routes textured, pastes mask only', 'expect': 'ok'},
                     applied == {'tex.png'} and tex_ok and smooth_ok,
                     f'applied={sorted(applied)} tex_ok={tex_ok} smooth_skipped={smooth_ok}'))
    finally:
        rdw.SOURCE, rdw.MASKS, rdw.LAMA, rdw._sd_pipe, rdw._sd_generator = old
        rmtree(tmp, ignore_errors=True)
    return rows


def run_detect_floor_checks(rdw, only):
    """再误检防线（无模型）：已去水印的强纹理图右下角会被**再检测**成水印，重跑把成品
    修坏。两道独立下限（均在原判据**之外另加**，且新增后老的正样本仍过）：

    ① 模板命中除 raw gap-score ≥ TEMPLATE_MIN_SCORE 外，另须**顶帽**分 ≥
       TEMPLATE_TOPHAT_MIN——raw 会把"水印处整体偏亮"（花丛/地毯亮块，尺度 > 顶帽核）
       计入，顶帽（扣局部背景）只认"字形相对**紧邻**背景更亮"。用大尺度亮台阶复现
       raw 高(36.8)/顶帽平(2.6) 的误检（实测 1.png raw 27.3、6.png raw 34.6 皆≤6.4 顶帽）；
    ② 右下角兜底行块除宽高比 ≤8 外，另须 ≥ CORNER_ROW_ASPECT_MIN(=2.5)——防"矮胖单块"
       （地毯/纸面背景碎块 比 1.2~1.9）被 9x3 膨胀粘连成"水印行"。附**宽行正样本**
       （比 ~5.5）仍须检出，保证下限不误伤真水印。"""
    import cv2
    import numpy as np
    if only and 'detect-floor' not in only:
        return []
    tpl, _a, meta = rdw.load_template()
    if tpl is None:
        return []
    rows = []

    # ① 顶帽下限：背景在字形处整体偏亮（大尺度台阶），raw 过线、顶帽不过 → 拒绝
    h, w = 900, 1600
    scale = min(h, w) / meta['ref_short_side']
    th = max(1, int(round(tpl.shape[0] * scale)))
    tw = max(1, int(round(tpl.shape[1] * scale)))
    px, py = w - tw - 8, h - th - 8
    gray = np.full((h, w), 100, np.float32)
    gray[py:py + th, px:px + tw // 2] = 220      # 左半亮块：尺度远大于 31px 顶帽核
    raw_peak = top_peak = 0.0
    resp = rdw._template_response(gray, w, h, with_tophat=True)
    if resp is not None:
        response, _t, _tw, top = resp
        wx, wy = max(0, response.shape[1] - 41), max(0, response.shape[0] - 41)
        _, _, _, peak = cv2.minMaxLoc(response[wy:, wx:])
        raw_peak = float(response[peak[1] + wy, peak[0] + wx])
        top_peak = float(top[peak[1] + wy, peak[0] + wx])
    mask, _score, _info = rdw.template_stroke_mask(gray, w, h)
    rows.append(({'group': 'detect-floor', 'name': 'tophat floor rejects flat bright block', 'expect': 'ok'},
                 raw_peak >= rdw.TEMPLATE_MIN_SCORE and top_peak < rdw.TEMPLATE_TOPHAT_MIN and mask is None,
                 f'raw_peak={raw_peak:.1f} tophat_peak={top_peak:.1f} '
                 f'mask={"hit" if mask is not None else "none"}'))

    # ② 行块宽高比下限：矮胖单块（旧判据误当"水印行"）→ 拒绝；宽行正样本 → 仍检出
    W, H = 2848, 1600
    x0, y0 = int(W * 0.70), int(H * 0.88)
    pad = max(10, H // 150)
    rh, rw = H - y0, W - x0

    def skyline(heights, gap, bar):
        white = np.zeros((rh, rw), np.uint8)
        by1 = rh - 8 - 56
        xx = rw - 8 - (len(heights) * (bar + gap) - gap)
        for hgt in heights:
            white[by1:by1 + hgt, xx:xx + bar] = 1
            xx += bar + gap
        return white

    fat = skyline([52, 30, 46, 22, 40], 13, 8)                              # 比 ~1.9（矮胖误检）
    wide = skyline([(56 if k % 2 == 0 else 42) for k in range(18)], 8, 10)  # 比 ~5.5（真水印行）
    saved = rdw.CORNER_ROW_ASPECT_MIN
    try:
        rdw.CORNER_ROW_ASPECT_MIN = 0.0                                     # 旧口径（无下限）
        fat_pre = rdw._corner_row_boxes(fat, H, W, x0, y0, pad)
    finally:
        rdw.CORNER_ROW_ASPECT_MIN = saved
    fat_post = rdw._corner_row_boxes(fat, H, W, x0, y0, pad)
    rows.append(({'group': 'detect-floor', 'name': 'row aspect floor rejects fat corner block', 'expect': 'ok'},
                 bool(fat_pre) and not fat_post,
                 f'pre_floor_boxes={len(fat_pre)} post_floor_boxes={len(fat_post)}'))
    wide_boxes = rdw._corner_row_boxes(wide, H, W, x0, y0, pad)
    rows.append(({'group': 'detect-floor', 'name': 'wide genuine row still detected', 'expect': 'ok'},
                 bool(wide_boxes), f'boxes={len(wide_boxes)}'))

    # ③ 行块 x 投影段数下限（4）：3 段的"宽亮带"（亮沙地/栏杆，6.png 逆解后 386x71 seg=3）
    #    不得当水印行；真水印行（上例 seg=18）仍检。
    seg3 = skyline([56, 40, 30], 13, 42)   # cw168 ch60 比2.8 fill0.77 fr0.53 seg3（其余合格）
    saved_seg = rdw.CORNER_ROW_SEG_MIN
    try:
        rdw.CORNER_ROW_SEG_MIN = 3
        seg_pre = rdw._corner_row_boxes(seg3, H, W, x0, y0, pad)
    finally:
        rdw.CORNER_ROW_SEG_MIN = saved_seg
    seg_post = rdw._corner_row_boxes(seg3, H, W, x0, y0, pad)
    rows.append(({'group': 'detect-floor', 'name': 'row segment floor rejects 3-seg wide band', 'expect': 'ok'},
                 bool(seg_pre) and not seg_post,
                 f'pre_floor_boxes={len(seg_pre)} post_floor_boxes={len(seg_post)}'))
    return rows


def run_fixture_checks(rdw, only):
    """固化 fixtures 完整性（CI 可跑、不依赖 dist/根目录）：入库的 original/processed
    fixture 必须与 manifest 的 md5 一致——任何"换图/改图"在此**显式红灯**，而不是让回归
    基线悄悄漂移（"换一张图就要重修老图"的根因）。同时汇报覆盖率。"""
    rows = []
    if only and 'fixtures' not in only:
        return rows
    manifest = load_manifest()
    if not manifest.get('originals'):
        return rows
    bad = []
    for kind in ('original', 'processed'):
        for name, entry in sorted(manifest.get(kind, {}).items()):
            fx = FIXTURE_IMAGES / kind / name
            if fx.exists() and entry.get('md5') and md5(fx) != entry['md5']:
                bad.append(f'{kind}/{name}')
    n_orig = len(list((FIXTURE_IMAGES / 'original').glob('*.png'))) if (FIXTURE_IMAGES / 'original').exists() else 0
    n_proc = len(list((FIXTURE_IMAGES / 'processed').glob('*.png'))) if (FIXTURE_IMAGES / 'processed').exists() else 0
    rows.append(({'group': 'fixtures', 'name': 'committed fixtures match manifest md5', 'expect': 'ok'},
                 not bad,
                 ('mismatch: ' + ','.join(bad)) if bad
                 else f'committed originals={n_orig} processed={n_proc} '
                      f'(manifest originals={len(manifest["originals"])} processed={len(manifest["processed"])})'))
    return rows


def update_golden(rdw):
    """重算并写入 tests/fixtures/manifest.json，并把精选子集拷入 tests/fixtures/images/。
    **只在确认检测行为变更是有意为之**时运行；随后的 git diff 即审计记录。"""
    (FIXTURE_IMAGES / 'original').mkdir(parents=True, exist_ok=True)
    (FIXTURE_IMAGES / 'processed').mkdir(parents=True, exist_ok=True)

    def _key(p):
        return (0, int(p.stem)) if p.stem.isdigit() else (1, p.stem)

    originals = {}
    for p in sorted(DIST.glob('*.png'), key=_key):
        originals[p.name] = {**probe(rdw, p), 'doubao': p.name not in NON_DOUBAO_DIST}
    processed = {}
    for p in sorted(ROOT.glob('*.png'), key=_key):
        ref = DIST / p.name
        if ref.exists() and md5(p) != md5(ref):
            processed[p.name] = probe(rdw, p)
    for name in FIXTURE_CORE_ORIGINALS:
        if name in originals:
            copyfile(DIST / name, FIXTURE_IMAGES / 'original' / name)
    for name in FIXTURE_CORE_PROCESSED:
        if name in processed:
            copyfile(ROOT / name, FIXTURE_IMAGES / 'processed' / name)
    FIXTURE_MANIFEST.write_text(json.dumps({
        'version': FIXTURE_VERSION,
        'note': '固化回归基线：md5 校验杜绝"换图"静默漂移；--update-golden 显式重算',
        'originals': originals,
        'processed': processed,
    }, ensure_ascii=False, indent=1) + '\n')
    print(f'wrote {FIXTURE_MANIFEST}: originals={len(originals)} processed={len(processed)} '
          f'committed originals={len(FIXTURE_CORE_ORIGINALS)} processed={len(FIXTURE_CORE_PROCESSED)}')


def main():
    ap = argparse.ArgumentParser(description='watermark processing regression corpus')
    ap.add_argument('--e2e', action='store_true', help='also run full pipeline + result verification (slow)')
    ap.add_argument('--model', default='lama', choices=['mat', 'lama'], help='model for --e2e (default lama)')
    ap.add_argument('--only', help='filter by group or name substring')
    ap.add_argument('--update-golden', action='store_true',
                    help='重算 tests/fixtures/manifest.json 并拷入库精选子集（确认基线变更有意为之才跑）')
    args = ap.parse_args()

    import remove_doubao_watermark as rdw
    import synthetic_watermark_test as swt

    if args.update_golden:
        update_golden(rdw)
        return

    cases = build_cases(rdw, swt)
    rows = run_fixture_checks(rdw, args.only)
    rows += run_layer1(rdw, swt, cases, args.only)
    rows += run_stamp_checks(rdw, args.only)
    rows += run_verify_checks(rdw, args.only)
    rows += run_lowcontrast_check(rdw, args.only)
    rows += run_inverse_quality_checks(rdw, args.only)
    rows += run_auto_profile_checks(rdw, swt, args.only)
    rows += run_crop_checks(rdw, args.only)
    rows += run_skip_checks(rdw, args.only)
    rows += run_anchor_checks(rdw, args.only)
    rows += run_sd_checks(rdw, args.only)
    rows += run_detect_floor_checks(rdw, args.only)

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

import numpy as np
from PIL import Image, ImageDraw

S = 4
W = 1024 * S
INDIGO = np.array([55, 48, 163], dtype=float)
VIOLET = np.array([139, 92, 246], dtype=float)
CYAN1 = (34, 211, 238)
CYAN2 = (6, 182, 212)
AMBER = (245, 158, 11)

xx, yy = np.meshgrid(np.arange(W), np.arange(W))
t = ((xx + yy) / (2 * W - 2))[:, :, None]
rgb = INDIGO * (1 - t) + VIOLET * t

cx, cy, r = 0.32 * W, 0.18 * W, 0.65 * W
dist = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2)
glow = np.clip(1 - dist / r, 0, 1) ** 2
rgb = np.clip(rgb + glow[:, :, None] * 255 * 0.16, 0, 255)

bg = Image.fromarray(rgb.astype(np.uint8), 'RGB')
draw = ImageDraw.Draw(bg)


def rounded_rect(radius):
    mask = Image.new('L', (W, W), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, W - 1, W - 1], radius=radius, fill=255)
    return mask


def star(draw, cx, cy, size, fill):
    k = size * 0.22
    draw.polygon([
        (cx, cy - size), (cx + k, cy - k), (cx + size, cy), (cx + k, cy + k),
        (cx, cy + size), (cx - k, cy + k), (cx - size, cy), (cx - k, cy - k),
    ], fill=fill)


def lerp_color(c1, c2, t):
    return tuple(int(a + (b - a) * t) for a, b in zip(c1, c2))


gx, gy, gw, gh = 252 * S, 292 * S, 520 * S, 400 * S
gr = 44 * S

shadow = Image.new('RGBA', (W, W), (0, 0, 0, 0))
sd = ImageDraw.Draw(shadow)
sd.rounded_rectangle([gx, gy + 30 * S, gx + gw, gy + gh + 30 * S], radius=gr, fill=(30, 20, 80, 110))
from PIL import ImageFilter
shadow = shadow.filter(ImageFilter.GaussianBlur(28 * S))
bg.paste(shadow, (0, 0), shadow)
draw = ImageDraw.Draw(bg)

photo = Image.new('RGBA', (gw, gh), (0, 0, 0, 0))
pd = ImageDraw.Draw(photo)
pd.rounded_rectangle([0, 0, gw - 1, gh - 1], radius=gr, fill=(255, 255, 255, 255))

inner = Image.new('RGBA', (gw, gh), (0, 0, 0, 0))
id_ = ImageDraw.Draw(inner)
clip = Image.new('L', (gw, gh), 0)
ImageDraw.Draw(clip).rounded_rectangle([0, 0, gw - 1, gh - 1], radius=gr, fill=255)

sky_h = int(gh * 0.62)
sky = Image.new('RGBA', (gw, gh), (0, 0, 0, 0))
sd = ImageDraw.Draw(sky)
for i in range(sky_h):
    t = i / sky_h
    sd.line([(0, i), (gw, i)], fill=lerp_color((224, 231, 255), (196, 181, 253), t))
inner.paste(sky, (0, 0), clip)

id_.ellipse([630 * S - gx - 30 * S, 372 * S - gy - 30 * S, 630 * S - gx + 30 * S, 372 * S - gy + 30 * S], fill=AMBER)
m1 = [(60 * S, gh), ((420 - 252) * S, int(gh * 0.28)), ((560 - 252) * S, gh)]
m2 = [((400 - 252) * S, gh), ((590 - 252) * S, int(gh * 0.45)), ((760 - 252) * S, gh)]
id_.polygon(m2, fill=(129, 140, 248))
id_.polygon(m1, fill=(99, 102, 241))
inner.putalpha(Image.composite(inner.split()[3], Image.new('L', (gw, gh), 0), clip))

photo.paste(inner, (0, 0), inner)
bg.paste(photo, (gx, gy), photo)
draw = ImageDraw.Draw(bg)

badge_cx, badge_cy, badge_r = 716 * S, 716 * S, 128 * S
for i in range(badge_r, 0, -1):
    t = i / badge_r
    col = lerp_color(CYAN1, CYAN2, t)
    draw.ellipse([badge_cx - i, badge_cy - i, badge_cx + i, badge_cy + i], fill=col)
ring = badge_r + 14 * S
draw.ellipse([badge_cx - ring, badge_cy - ring, badge_cx + ring, badge_cy + ring], outline=(255, 255, 255, 255), width=10 * S)
draw.line([(badge_cx - 52 * S, badge_cy + 2 * S), (badge_cx - 12 * S, badge_cy + 44 * S)], fill=(255, 255, 255), width=30 * S)
draw.line([(badge_cx - 12 * S, badge_cy + 44 * S), (badge_cx + 58 * S, badge_cy - 38 * S)], fill=(255, 255, 255), width=30 * S)

star(draw, 836 * S, 236 * S, 52 * S, (255, 255, 255, 235))
star(draw, 190 * S, 700 * S, 34 * S, (255, 255, 255, 190))
star(draw, 772 * S, 130 * S, 26 * S, (255, 255, 255, 160))

alpha = rounded_rect(230 * S)
out = bg.convert('RGBA')
out.putalpha(alpha)
import os
out_path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'desktop', 'icon-source.png')
out.resize((1024, 1024), Image.LANCZOS).save(out_path)
print('icon source saved')

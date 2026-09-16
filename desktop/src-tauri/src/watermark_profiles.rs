//! Watermark profile library — Rust port of `tools/watermark_profiles.py`.
//!
//! A profile models a fixed semi-transparent overlay: a per-pixel coverage map
//! (`alpha`, 0..1) plus a per-pixel colour (`color`, RGB 0..255), captured at a
//! reference short side so it can be rescaled and relocated on any image. With a
//! profile the watermark is removed by *reversing the blending equation*
//!
//! ```text
//! observed = alpha * C + (1 - alpha) * background
//! ```
//!
//! which restores the true background instead of letting the generative model
//! redraw it. Profiles live under `tools/watermarks/<id>/` as `alpha.png`
//! (8-bit coverage), `color.png` (8-bit RGB) and `meta.json`; the built-in qwen
//! profile is also compiled into the binary.
//!
//! Localisation is contrast-normalised NCC of the alpha shape (bright *and* dark
//! top-hat channels), searched coarse-to-fine over a +-35% scale span so a
//! watermark re-rendered at a different size is still found.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use image::{GrayImage, Luma, RgbImage};
use imageproc::region_labelling::{connected_components, Connectivity};
use serde_json::{Map, Value};

pub const DEFAULT_REF_SHORT: f64 = 1600.0;
pub const MASK_ALPHA_THRESHOLD: f64 = 8.0;
pub const MASK_DILATE: (usize, usize) = (3, 3);
pub const INVERSE_ALPHA_THRESHOLD: f64 = 0.03;
pub const INVERSE_MAX_GHOST: f64 = 0.6;
pub const INVERSE_MAX_FIT: f64 = 14.0;
pub const NCC_SCALE_SPAN: f64 = 0.35;
pub const NCC_COARSE_STEP: f64 = 0.05;
pub const NCC_FINE_SPAN: f64 = 0.05;
pub const NCC_FINE_STEP: f64 = 0.0125;
pub const NCC_MIN_SCORE: f64 = 0.5;
/// 用户显式指定档案（精确模式）时的放宽阈值。
pub const FORCED_MIN_SCORE: f64 = 0.35;
/// 学习档案用顶帽 gap-score 定位时的最小分数（单位是顶帽亮度差，不是 NCC 的 0..1）。
/// 实测：含该水印 17.8~45.5，不含水印的图 1.1~7.3 —— 取 12 两侧都有充足裕度。
pub const PROFILE_GAP_MIN_SCORE: f64 = 12.0;
/// 定位模板把档案 α 二值化的阈值（与内置豆包模板 `TEMPLATE_ALPHA_THRESHOLD` 同口径）。
const LEARN_TEMPLATE_BIN: f32 = 0.15;
const NCC_PYRAMID: usize = 4;
/// 跨样本中位数顶帽的笔画判据（0..255）。水印是各图共性结构，取中位数可压掉各图
/// 背景的高频伪结构；阈值偏低是有意的——漏检会在 α 上留下缺口、逆解不干净，而
/// 误检只扩大框内重绘面积（回归门限与 clean_alpha 会再收敛）。可用环境变量微调。
const LEARN_BATCH_TOPHAT_MIN: f32 = 2.0;
/// 逆解 α 的最低保留值：低于此值的像素不参与求 C、不计残差、逆解时当纯背景。
/// 豆包水印实测 α≈0.53，0.15 能覆盖抗锯齿边与淡笔画。
const LEARN_CORE_ALPHA: f32 = 0.15;

fn learn_batch_tophat_min() -> f32 {
    std::env::var("LEARN_BATCH_TOPHAT_MIN")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(LEARN_BATCH_TOPHAT_MIN)
}

fn learn_core_alpha() -> f32 {
    std::env::var("LEARN_CORE_ALPHA")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(LEARN_CORE_ALPHA)
}

const ALPHA_FILENAME: &str = "alpha.png";
const COLOR_FILENAME: &str = "color.png";
const META_FILENAME: &str = "meta.json";

/// Built-in profiles compiled into the binary (id, alpha.png, color.png, meta).
const EMBEDDED: &[(&str, &[u8], &[u8], &str)] = &[(
    "qwen",
    include_bytes!("../../../tools/watermarks/qwen/alpha.png"),
    include_bytes!("../../../tools/watermarks/qwen/color.png"),
    include_str!("../../../tools/watermarks/qwen/meta.json"),
)];

/// 豆包完整水印 stamp（α 覆盖度 + 逐像素颜色 C，含暗描边），由
/// `tools/doubao-wm-stamp.npz` 导出（4 张同款水印不同背景联立标定）。
/// **只用于模板命中后的逐像素解析逆解**，不进入 `EMBEDDED`/`match_image`：
/// 豆包的定位仍走 gap-score 模板（`pipeline::template_stroke_mask`）。
const STAMP_ALPHA_PNG: &[u8] = include_bytes!("../../../tools/doubao-wm-stamp-alpha.png");
const STAMP_COLOR_PNG: &[u8] = include_bytes!("../../../tools/doubao-wm-stamp-color.png");
const STAMP_META_JSON: &str = r#"{"id":"doubao-stamp","label":"doubao","ref_short_side":1600.0,"source":"tools/doubao-wm-stamp.npz","extra":{}}"#;

pub fn doubao_stamp() -> Result<Profile, String> {
    profile_from_bytes("doubao-stamp", STAMP_ALPHA_PNG, STAMP_COLOR_PNG, STAMP_META_JSON)
}

#[derive(Clone)]
pub struct Profile {
    pub id: String,
    pub label: String,
    pub ref_short_side: f64,
    pub source: String,
    pub created: String,
    pub extra: Map<String, Value>,
    pub aw: usize,
    pub ah: usize,
    pub alpha: Vec<f32>,
    pub color: Vec<f32>,
}

/// Where user-learned profiles are stored. `WATERMARK_PROFILES_DIR` overrides;
/// 打包后的 App 用 `PROFILES_DIR_OVERRIDE`（应用数据目录，bundle 内只读）。
pub fn profiles_dir() -> PathBuf {
    if let Ok(path) = std::env::var("WATERMARK_PROFILES_DIR") {
        return PathBuf::from(path);
    }
    if let Some(dir) = crate::PROFILES_DIR_OVERRIDE.get() {
        return dir.clone();
    }
    crate::project_root().join("tools/watermarks")
}

fn profile_from_bytes(id: &str, alpha_png: &[u8], color_png: &[u8], meta_json: &str) -> Result<Profile, String> {
    let a = image::load_from_memory_with_format(alpha_png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .to_luma8();
    let c = image::load_from_memory_with_format(color_png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .to_rgb8();
    if a.dimensions() != c.dimensions() {
        return Err(format!("alpha/color size mismatch for profile {id}"));
    }
    let (aw, ah) = (a.width() as usize, a.height() as usize);
    let alpha = a.iter().map(|&v| v as f32 / 255.0).collect::<Vec<f32>>();
    let color = c
        .pixels()
        .flat_map(|p| [p.0[0] as f32, p.0[1] as f32, p.0[2] as f32])
        .collect::<Vec<f32>>();
    let meta: Value = serde_json::from_str(meta_json).map_err(|e| e.to_string())?;
    Ok(Profile {
        id: meta.get("id").and_then(Value::as_str).unwrap_or(id).to_string(),
        label: meta.get("label").and_then(Value::as_str).unwrap_or("").to_string(),
        ref_short_side: meta
            .get("ref_short_side")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_REF_SHORT),
        source: meta.get("source").and_then(Value::as_str).unwrap_or("").to_string(),
        created: meta.get("created").and_then(Value::as_str).unwrap_or("").to_string(),
        extra: meta.get("extra").and_then(Value::as_object).cloned().unwrap_or_default(),
        aw,
        ah,
        alpha,
        color,
    })
}

pub fn load_profile_dir(dir: &Path) -> Result<Profile, String> {
    let id = dir.file_name().and_then(|s| s.to_str()).unwrap_or("profile").to_string();
    let alpha = fs::read(dir.join(ALPHA_FILENAME)).map_err(|e| e.to_string())?;
    let color = fs::read(dir.join(COLOR_FILENAME)).map_err(|e| e.to_string())?;
    let meta = fs::read_to_string(dir.join(META_FILENAME)).map_err(|e| e.to_string())?;
    profile_from_bytes(&id, &alpha, &color, &meta)
}

pub fn load_profile(id: &str) -> Result<Profile, String> {
    let dir = profiles_dir().join(id);
    if dir.join(META_FILENAME).exists() {
        return load_profile_dir(&dir);
    }
    for (bid, a, c, m) in EMBEDDED {
        if *bid == id {
            return profile_from_bytes(bid, a, c, m);
        }
    }
    Err(format!("profile not found: {id}"))
}

/// All profiles: external directory first, then embedded built-ins not shadowed.
pub fn list_profiles() -> Vec<Profile> {
    let mut out: Vec<Profile> = Vec::new();
    if let Ok(read) = fs::read_dir(profiles_dir()) {
        let mut entries: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        entries.sort();
        for dir in entries {
            if let Ok(p) = load_profile_dir(&dir) {
                out.push(p);
            }
        }
    }
    for (id, a, c, m) in EMBEDDED {
        if !out.iter().any(|p| p.id == *id) {
            if let Ok(p) = profile_from_bytes(id, a, c, m) {
                out.push(p);
            }
        }
    }
    out
}

pub fn save_profile(profile: &Profile, overwrite: bool) -> Result<PathBuf, String> {
    let base = profiles_dir().join(&profile.id);
    if base.exists() && !overwrite {
        return Err(format!("profile already exists: {}", profile.id));
    }
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let mut alpha_u8 = vec![0u8; profile.aw * profile.ah];
    for (i, &v) in profile.alpha.iter().enumerate() {
        alpha_u8[i] = (v * 255.0).clamp(0.0, 255.0) as u8;
    }
    let mut color_u8 = vec![0u8; profile.aw * profile.ah * 3];
    for (i, &v) in profile.color.iter().enumerate() {
        color_u8[i] = v.clamp(0.0, 255.0) as u8;
    }
    let alpha_img = GrayImage::from_raw(profile.aw as u32, profile.ah as u32, alpha_u8)
        .ok_or("bad alpha buffer")?;
    let color_img = RgbImage::from_raw(profile.aw as u32, profile.ah as u32, color_u8)
        .ok_or("bad color buffer")?;
    alpha_img
        .save_with_format(base.join(ALPHA_FILENAME), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    color_img
        .save_with_format(base.join(COLOR_FILENAME), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let meta = serde_json::json!({
        "id": profile.id,
        "label": profile.label,
        "ref_short_side": profile.ref_short_side,
        "source": profile.source,
        "created": profile.created,
        "extra": Value::Object(profile.extra.clone()),
    });
    fs::write(base.join(META_FILENAME), serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(base)
}

// ---------------------------------------------------------------------------
// numeric helpers
// ---------------------------------------------------------------------------

fn max3(p: &[f32]) -> f32 {
    p[0].max(p[1]).max(p[2])
}

/// Bilinear resize of an interleaved f32 buffer with `nc` channels.
fn resize_f32(src: &[f32], sw: usize, sh: usize, nc: usize, dw: usize, dh: usize) -> Vec<f32> {
    let mut out = vec![0f32; dw.saturating_mul(dh).saturating_mul(nc)];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    let sx = sw as f32 / dw as f32;
    let sy = sh as f32 / dh as f32;
    for y in 0..dh {
        let fy = (y as f32 + 0.5) * sy - 0.5;
        let y0 = fy.floor();
        let wy = fy - y0;
        let y0i = (y0.max(0.0) as isize).min(sh as isize - 1) as usize;
        let y1i = (y0i as isize + 1).clamp(0, sh as isize - 1) as usize;
        for x in 0..dw {
            let fx = (x as f32 + 0.5) * sx - 0.5;
            let x0 = fx.floor();
            let wx = fx - x0;
            let x0i = (x0.max(0.0) as isize).min(sw as isize - 1) as usize;
            let x1i = (x0i as isize + 1).clamp(0, sw as isize - 1) as usize;
            for ch in 0..nc {
                let p00 = src[(y0i * sw + x0i) * nc + ch];
                let p01 = src[(y0i * sw + x1i) * nc + ch];
                let p10 = src[(y1i * sw + x0i) * nc + ch];
                let p11 = src[(y1i * sw + x1i) * nc + ch];
                let top = p00 * (1.0 - wx) + p01 * wx;
                let bot = p10 * (1.0 - wx) + p11 * wx;
                out[(y * dw + x) * nc + ch] = top * (1.0 - wy) + bot * wy;
            }
        }
    }
    out
}

/// Nearest-neighbour resize is unused for now; kept out to avoid dead code.
#[allow(dead_code)]
fn resize_nn(src: &[f32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<f32> {
    let mut out = vec![0f32; dw * dh];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh {
        let sy = ((y as f32 + 0.5) * sh as f32 / dh as f32) as usize;
        let sy = sy.min(sh - 1);
        for x in 0..dw {
            let sx = ((x as f32 + 0.5) * sw as f32 / dw as f32) as usize;
            out[y * dw + x] = src[sy * sw + sx.min(sw - 1)];
        }
    }
    out
}

/// Downscale by integer factor `d` using a box average.
fn downscale(src: &[f32], w: usize, h: usize, nc: usize, d: usize) -> (Vec<f32>, usize, usize) {
    let dw = (w + d - 1) / d;
    let dh = (h + d - 1) / d;
    let mut out = vec![0f32; dw * dh * nc];
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = vec![0f64; nc];
            let mut cnt = 0f64;
            for yy in (y * d)..((y + 1) * d).min(h) {
                for xx in (x * d)..((x + 1) * d).min(w) {
                    for c in 0..nc {
                        acc[c] += src[(yy * w + xx) * nc + c] as f64;
                    }
                    cnt += 1.0;
                }
            }
            if cnt > 0.0 {
                for c in 0..nc {
                    out[(y * dw + x) * nc + c] = (acc[c] / cnt) as f32;
                }
            }
        }
    }
    (out, dw, dh)
}

/// 1-D sliding-window extremum (deque), O(n) for any radius.
fn slide_extreme(v: &[f32], r: usize, dilate: bool) -> Vec<f32> {
    let n = v.len();
    let mut out = vec![0f32; n];
    if n == 0 {
        return out;
    }
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut next = 0usize;
    for i in 0..n {
        let hi = (i + r).min(n - 1);
        while next <= hi {
            while let Some(&b) = dq.back() {
                let drop = if dilate { v[b] <= v[next] } else { v[b] >= v[next] };
                if drop {
                    dq.pop_back();
                } else {
                    break;
                }
            }
            dq.push_back(next);
            next += 1;
        }
        let lo = i.saturating_sub(r);
        while let Some(&f) = dq.front() {
            if f < lo {
                dq.pop_front();
            } else {
                break;
            }
        }
        out[i] = v[*dq.front().unwrap()];
    }
    out
}

/// Separable rectangular grayscale dilation/erosion (square `(2r+1)^2` kernel).
fn rect_morph(src: &[f32], w: usize, h: usize, r: usize, dilate: bool) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..y * w + w];
        let out = slide_extreme(row, r, dilate);
        tmp[y * w..y * w + w].copy_from_slice(&out);
    }
    let mut out = vec![0f32; w * h];
    let mut col = vec![0f32; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = tmp[y * w + x];
        }
        let res = slide_extreme(&col, r, dilate);
        for y in 0..h {
            out[y * w + x] = res[y];
        }
    }
    out
}

fn open_morph(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let e = rect_morph(src, w, h, r, false);
    rect_morph(&e, w, h, r, true)
}

fn close_morph(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let d = rect_morph(src, w, h, r, true);
    rect_morph(&d, w, h, r, false)
}

struct SummedArea {
    w: usize,
    h: usize,
    data: Vec<f64>,
}

impl SummedArea {
    fn new(src: &[f32], w: usize, h: usize) -> Self {
        let mut data = vec![0f64; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = 0f64;
            for x in 0..w {
                row += src[y * w + x] as f64;
                data[(y + 1) * (w + 1) + (x + 1)] = data[y * (w + 1) + (x + 1)] + row;
            }
        }
        SummedArea { w, h, data }
    }

    /// Sum over [x0, x1) x [y0, y1).
    fn rect(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> f64 {
        let w1 = self.w + 1;
        let x1 = x1.min(self.w);
        let y1 = y1.min(self.h);
        self.data[y1 * w1 + x1] + self.data[y0 * w1 + x0]
            - self.data[y0 * w1 + x1]
            - self.data[y1 * w1 + x0]
    }
}

fn box_mean(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let sa = SummedArea::new(src, w, h);
    let mut out = vec![0f32; w * h];
    let r = radius as isize;
    for y in 0..h {
        for x in 0..w {
            let x0 = (x as isize - r).max(0) as usize;
            let y0 = (y as isize - r).max(0) as usize;
            let x1 = (x as isize + r + 1).min(w as isize) as usize;
            let y1 = (y as isize + r + 1).min(h as isize) as usize;
            let area = ((x1 - x0) * (y1 - y0)).max(1) as f64;
            out[y * w + x] = (sa.rect(x0, y0, x1, y1) / area) as f32;
        }
    }
    out
}

fn median(mut vals: Vec<f32>) -> f32 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = vals.len();
    if n % 2 == 1 {
        vals[n / 2]
    } else {
        (vals[n / 2 - 1] + vals[n / 2]) * 0.5
    }
}

fn median_rgb(vals: &[f32], nc: usize) -> [f32; 3] {
    let mut out = [0f32; 3];
    for c in 0..nc.min(3) {
        let chan: Vec<f32> = vals.iter().skip(c).step_by(nc).copied().collect();
        out[c] = median(chan);
    }
    out
}

/// Connected components (8-connectivity); returns per-pixel labels and areas.
fn labels_areas(bin: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<u32>) {
    let mut img = GrayImage::new(w as u32, h as u32);
    for (i, &b) in bin.iter().enumerate() {
        if b {
            img.put_pixel((i % w) as u32, (i / w) as u32, Luma([255]));
        }
    }
    let lab = connected_components(&img, Connectivity::Eight, Luma([0u8]));
    let raw = lab.into_raw();
    let maxl = raw.iter().copied().max().unwrap_or(0) as usize;
    let mut areas = vec![0u32; maxl + 1];
    for &l in &raw {
        areas[l as usize] += 1;
    }
    (raw, areas)
}

fn clean_alpha(alpha: &mut [f32], w: usize, h: usize, thr: f32, min_area: u32) {
    let bin: Vec<bool> = alpha.iter().map(|&a| a > thr).collect();
    let (labels, areas) = labels_areas(&bin, w, h);
    for i in 0..alpha.len() {
        let l = labels[i] as usize;
        // Mirror Python's ``alpha * keep``: anything that is not part of a
        // large-enough foreground blob (background label 0, tiny specks) is
        // zeroed, so the later alpha crop stays tight.
        if l == 0 || areas[l] < min_area {
            alpha[i] = 0.0;
        }
    }
}

/// Crop an alpha(+colour) pair to the alpha bounding box (2px pad).
fn crop_to_alpha(alpha: &[f32], color: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>, usize, usize, usize, usize) {
    let mut x0 = w;
    let mut y0 = h;
    let mut x1 = 0usize;
    let mut y1 = 0usize;
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if alpha[y * w + x] > 0.0 {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if !any {
        return (Vec::new(), Vec::new(), 0, 0, 0, 0);
    }
    let pad = 2usize;
    let x0 = x0.saturating_sub(pad);
    let y0 = y0.saturating_sub(pad);
    let x1 = (x1 + pad + 1).min(w);
    let y1 = (y1 + pad + 1).min(h);
    let cw = x1 - x0;
    let ch = y1 - y0;
    let mut a = vec![0f32; cw * ch];
    let mut c = vec![0f32; cw * ch * 3];
    for y in 0..ch {
        for x in 0..cw {
            a[y * cw + x] = alpha[(y0 + y) * w + x0 + x];
            for ch2 in 0..3 {
                c[(y * cw + x) * 3 + ch2] = color[((y0 + y) * w + x0 + x) * 3 + ch2];
            }
        }
    }
    (a, c, cw, ch, x0, y0)
}

fn stroke_mask(rgb: &[f32], w: usize, h: usize, sat_max: f32, rel: f32, min_area: u32) -> Vec<bool> {
    let n = w * h;
    let mut gray = vec![0f32; n];
    let mut sat = vec![0f32; n];
    for i in 0..n {
        let p = &rgb[i * 3..i * 3 + 3];
        let mx = max3(p);
        let mn = p[0].min(p[1]).min(p[2]);
        gray[i] = mx;
        sat[i] = mx - mn;
    }
    let opened = open_morph(&gray, w, h, 15);
    let diff: Vec<f32> = (0..n).map(|i| gray[i] - opened[i]).collect();
    let diff_sq: Vec<f32> = diff.iter().map(|&v| v * v).collect();
    let mean = box_mean(&diff, w, h, 48);
    let sq = box_mean(&diff_sq, w, h, 48);
    let mut mask = vec![false; n];
    for i in 0..n {
        let std = (sq[i] - mean[i] * mean[i]).max(0.0).sqrt();
        let thr = 10.0f32.max(mean[i] + rel * std);
        mask[i] = diff[i] >= thr && sat[i] <= sat_max;
    }
    let (labels, areas) = labels_areas(&mask, w, h);
    let mut keep = vec![false; n];
    for i in 0..n {
        let l = labels[i] as usize;
        // Label 0 is the background component; it must never be kept (its area
        // dwarfs the threshold and would flood the whole image).
        if l > 0 && areas[l] >= min_area {
            keep[i] = true;
        }
    }
    keep
}

pub fn guess_corner_box(width: usize, height: usize) -> (usize, usize, usize, usize) {
    let scale = (width as f64 / 2848.0).min(height as f64 / 1600.0);
    let bw = (330.0 * scale).max(220.0) as usize;
    let bh = (118.0 * scale).max(78.0) as usize;
    (width.saturating_sub(bw), height.saturating_sub(bh), width, height)
}

fn resolve_box(box_: Option<(i64, i64, i64, i64)>, width: usize, height: usize) -> (usize, usize, usize, usize) {
    match box_ {
        None => guess_corner_box(width, height),
        Some((x1, y1, x2, y2)) => {
            let fx = |v: i64, dim: usize| -> usize {
                let v = if v < 0 { dim as i64 + v } else { v };
                v.clamp(0, dim as i64) as usize
            };
            let x1 = fx(x1, width);
            let y1 = fx(y1, height);
            let x2 = fx(x2, width);
            let y2 = fx(y2, height);
            let (x1, x2) = (x1.min(x2), x1.max(x2));
            let (y1, y2) = (y1.min(y2), y1.max(y2));
            (x1, y1, x2, y2)
        }
    }
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.trim().to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch == ' ' || ch == '\t' {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "profile".to_string()
    } else {
        trimmed
    }
}

pub fn detect_watermark_box(rgb: &[f32], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    // Compute the stroke mask on the full image (Python-compatible: the borders
    // are the image borders, not a crop, so no edge artefacts arise).
    let mask = stroke_mask(rgb, w, h, 60.0, 1.8, 6);
    let x0 = (w as f64 * 0.60) as usize;
    let y0 = (h as f64 * 0.82) as usize;
    let mut minx = w;
    let mut miny = h;
    let mut maxx = 0usize;
    let mut maxy = 0usize;
    let mut count = 0usize;
    for y in y0..h {
        for x in x0..w {
            if mask[y * w + x] {
                count += 1;
                minx = minx.min(x);
                miny = miny.min(y);
                maxx = maxx.max(x);
                maxy = maxy.max(y);
            }
        }
    }
    if count < 30 {
        return None;
    }
    let pad = 10usize;
    Some((
        minx.saturating_sub(pad),
        miny.saturating_sub(pad),
        (maxx + 1 + pad).min(w),
        (maxy + 1 + pad).min(h),
    ))
}

pub fn background_score(
    rgb: &[f32],
    w: usize,
    h: usize,
    box_: Option<(i64, i64, i64, i64)>,
    color: [f32; 3],
) -> Option<Value> {
    let (x1, y1, x2, y2) = resolve_box(box_, w, h);
    if x2 <= x1 + 4 || y2 <= y1 + 4 {
        return None;
    }
    let (rw, rh) = (x2 - x1, y2 - y1);
    let mut win = vec![0f32; rw * rh * 3];
    for y in 0..rh {
        for x in 0..rw {
            for c in 0..3 {
                win[(y * rw + x) * 3 + c] = rgb[((y1 + y) * w + x1 + x) * 3 + c];
            }
        }
    }
    let stroke = stroke_mask(&win, rw, rh, 60.0, 1.8, 6);
    let count = stroke.iter().filter(|&&b| b).count();
    if count < 20 {
        return None;
    }
    let filled = fill_inpaint(&win, rw, rh, 3, &stroke);
    let bg_med = median_rgb(&filled, 3);
    let mut acc = 0f64;
    for i in 0..rw * rh {
        for c in 0..3 {
            acc += (filled[i * 3 + c] - bg_med[c]).abs() as f64;
        }
    }
    let uniformity = acc / (rw * rh * 3) as f64;
    let contrast = (0..3).map(|c| color[c] - bg_med[c]).sum::<f32>() / 3.0;
    Some(serde_json::json!({
        "box": [x1, y1, x2, y2],
        "short": w.min(h),
        "size": [w, h],
        "coverage": count as f64 / (rw * rh) as f64,
        "bg_median": [bg_med[0].round() as i64, bg_med[1].round() as i64, bg_med[2].round() as i64],
        "uniformity": uniformity,
        "contrast": contrast as f64,
    }))
}

/// Simple diffusion fill of masked pixels (background estimate under strokes).
fn fill_inpaint(src: &[f32], w: usize, h: usize, nc: usize, mask: &[bool]) -> Vec<f32> {
    let mut out = src.to_vec();
    let mut known: Vec<bool> = mask.iter().map(|&m| !m).collect();
    let iters = (w.max(h)).min(400);
    for _ in 0..iters {
        let mut next = out.clone();
        let mut nk = known.clone();
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if known[i] {
                    continue;
                }
                let mut acc = vec![0f64; nc];
                let mut cnt = 0f64;
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x as isize + dx;
                        let ny = y as isize + dy;
                        if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                            continue;
                        }
                        let j = ny as usize * w + nx as usize;
                        if known[j] {
                            for c in 0..nc {
                                acc[c] += out[j * nc + c] as f64;
                            }
                            cnt += 1.0;
                        }
                    }
                }
                if cnt > 0.0 {
                    for c in 0..nc {
                        next[i * nc + c] = (acc[c] / cnt) as f32;
                    }
                    nk[i] = true;
                    changed = true;
                }
            }
        }
        out = next;
        known = nk;
        if !changed {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// profile learning
// ---------------------------------------------------------------------------

fn rgb_f32_from(img: &RgbImage) -> Vec<f32> {
    img.pixels()
        .flat_map(|p| [p.0[0] as f32, p.0[1] as f32, p.0[2] as f32])
        .collect()
}

/// Estimate the integer shift of `b` relative to `a` on the grayscale crops.
fn estimate_shift(a: &[f32], b: &[f32], w: usize, h: usize, max_shift: isize) -> (isize, isize) {
    let mut best = (f64::MAX, 0isize, 0isize);
    for dy in -max_shift..=max_shift {
        for dx in -max_shift..=max_shift {
            let mut acc = 0f64;
            let mut cnt = 0f64;
            for y in (max_shift..(h as isize - max_shift)).step_by(2) {
                for x in (max_shift..(w as isize - max_shift)).step_by(2) {
                    let bx = x + dx;
                    let by = y + dy;
                    let ia = (y as usize) * w + x as usize;
                    let ib = (by as usize) * w + bx as usize;
                    let d = (a[ia] - b[ib]) as f64;
                    acc += d * d;
                    cnt += 1.0;
                }
            }
            let score = if cnt > 0.0 { acc / cnt } else { f64::MAX };
            if score < best.0 {
                best = (score, dx, dy);
            }
        }
    }
    (best.1, best.2)
}

fn translate(src: &[f32], w: usize, h: usize, nc: usize, dx: isize, dy: isize) -> Vec<f32> {
    let mut out = vec![0f32; src.len()];
    for y in 0..h {
        for x in 0..w {
            let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
            let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
            for c in 0..nc {
                out[(y * w + x) * nc + c] = src[(sy * w + sx) * nc + c];
            }
        }
    }
    out
}

/// Exact per-pixel profile from a near-black / near-white sample pair.
pub fn extract_from_pair(
    path_black: &Path,
    path_white: &Path,
    box_: Option<(i64, i64, i64, i64)>,
    ref_short_side: Option<f64>,
    label: &str,
    min_area: u32,
    max_resid: f64,
) -> Result<(Option<Profile>, Value), String> {
    let black_img = image::open(path_black).map_err(|e| e.to_string())?.to_rgb8();
    let white_img = image::open(path_white).map_err(|e| e.to_string())?.to_rgb8();
    if black_img.dimensions() != white_img.dimensions() {
        return Ok((None, serde_json::json!({"reason": "sample sizes differ"})));
    }
    let (w, h) = (black_img.width() as usize, black_img.height() as usize);
    let black = rgb_f32_from(&black_img);
    let mut white = rgb_f32_from(&white_img);
    let box_ = match box_ {
        Some(b) => Some(b),
        None => {
            let db = detect_watermark_box(&black, w, h);
            let dw = detect_watermark_box(&white, w, h);
            match (db, dw) {
                (Some(a), Some(b)) => Some((
                    a.0.min(b.0) as i64,
                    a.1.min(b.1) as i64,
                    a.2.max(b.2) as i64,
                    a.3.max(b.3) as i64,
                )),
                (Some(a), None) => Some((a.0 as i64, a.1 as i64, a.2 as i64, a.3 as i64)),
                (None, Some(b)) => Some((b.0 as i64, b.1 as i64, b.2 as i64, b.3 as i64)),
                (None, None) => None,
            }
        }
    };
    let (x1, y1, x2, y2) = resolve_box(box_, w, h);
    if x2 <= x1 + 4 || y2 <= y1 + 4 {
        return Ok((None, serde_json::json!({"reason": "invalid box"})));
    }
    // Align the two frames (same generator ⇒ already coincident).
    let pad = 40usize;
    let cx1 = x1.saturating_sub(pad);
    let cy1 = y1.saturating_sub(pad);
    let cx2 = (x2 + pad).min(w);
    let cy2 = (y2 + pad).min(h);
    let (cw, ch) = (cx2 - cx1, cy2 - cy1);
    let mut ga = vec![0f32; cw * ch];
    let mut gb = vec![0f32; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            let ia = ((cy1 + y) * w + cx1 + x) * 3;
            ga[y * cw + x] = max3(&black[ia..ia + 3]);
            gb[y * cw + x] = max3(&white[ia..ia + 3]);
        }
    }
    let (dx, dy) = estimate_shift(&ga, &gb, cw, ch, 4);
    if dx.abs() > 1 || dy.abs() > 1 {
        white = translate(&white, w, h, 3, -dx, -dy);
    }
    // Background levels from a flat region away from the watermark.
    let bx0 = x1.saturating_sub(8);
    let by0 = y1.saturating_sub(8);
    let bx1 = (x2 + 8).min(w);
    let by1 = (y2 + 8).min(h);
    let mut vals_b: Vec<f32> = Vec::new();
    let mut vals_w: Vec<f32> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if x >= bx0 && x < bx1 && y >= by0 && y < by1 {
                continue;
            }
            for c in 0..3 {
                vals_b.push(black[(y * w + x) * 3 + c]);
                vals_w.push(white[(y * w + x) * 3 + c]);
            }
        }
    }
    let bg_b = median_rgb(&vals_b, 3);
    let bg_w = median_rgb(&vals_w, 3);
    let db = ((bg_w[0] + bg_w[1] + bg_w[2]) - (bg_b[0] + bg_b[1] + bg_b[2])) / 3.0;
    if db < 32.0 {
        return Ok((None, serde_json::json!({"reason": format!("backgrounds too close (delta={db:.0})")})));
    }
    let (rw, rh) = (x2 - x1, y2 - y1);
    let mut alpha = vec![0f32; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let i = ((y1 + y) * w + x1 + x) * 3;
            let diff = ((white[i] + white[i + 1] + white[i + 2])
                - (black[i] + black[i + 1] + black[i + 2]))
                / 3.0;
            alpha[y * rw + x] = (1.0 - diff / db).clamp(0.0, 1.0);
        }
    }
    clean_alpha(&mut alpha, rw, rh, 0.02, min_area);
    let core = alpha.iter().filter(|&&a| a > 0.05).count();
    if core < 50 {
        return Ok((None, serde_json::json!({"reason": "not enough separable watermark coverage"})));
    }
    let mut color = vec![0f32; rw * rh * 3];
    for y in 0..rh {
        for x in 0..rw {
            let i3 = ((y1 + y) * w + x1 + x) * 3;
            let a = alpha[y * rw + x];
            for c in 0..3 {
                let v = (black[i3 + c] - (1.0 - a) * bg_b[c]) / a.max(1e-3);
                color[(y * rw + x) * 3 + c] = if a <= 0.03 { 0.0 } else { v.clamp(0.0, 255.0) };
            }
        }
    }
    let mut resid_b = 0f64;
    let mut resid_w = 0f64;
    let mut cnt = 0f64;
    for y in 0..rh {
        for x in 0..rw {
            let a = alpha[y * rw + x];
            if a <= 0.05 {
                continue;
            }
            let i3 = ((y1 + y) * w + x1 + x) * 3;
            let c3 = (y * rw + x) * 3;
            for c in 0..3 {
                let pb = a * color[c3 + c] + (1.0 - a) * bg_b[c];
                let pw = a * color[c3 + c] + (1.0 - a) * bg_w[c];
                resid_b += (pb - black[i3 + c]).abs() as f64;
                resid_w += (pw - white[i3 + c]).abs() as f64;
            }
            cnt += 3.0;
        }
    }
    let rb = if cnt > 0.0 { resid_b / cnt } else { 999.0 };
    let rw_ = if cnt > 0.0 { resid_w / cnt } else { 999.0 };
    let short = w.min(h);
    let report = serde_json::json!({
        "reason": if rb.max(rw_) > max_resid { "pair residual too high" } else { "ok" },
        "residual_black": rb,
        "residual_white": rw_,
        "coverage": core as f64 / (rw * rh) as f64,
        "box": [x1, y1, x2, y2],
        "bg_black": [bg_b[0].round() as i64, bg_b[1].round() as i64, bg_b[2].round() as i64],
        "bg_white": [bg_w[0].round() as i64, bg_w[1].round() as i64, bg_w[2].round() as i64],
        "shift": [dx, dy],
    });
    if rb.max(rw_) > max_resid {
        return Ok((None, report));
    }
    let (a_c, c_c, acw, ach, sx0, sy0) = crop_to_alpha(&alpha, &color, rw, rh);
    if acw == 0 || ach == 0 {
        return Ok((None, serde_json::json!({"reason": "empty alpha profile"})));
    }
    let ref_short = ref_short_side.unwrap_or(short as f64);
    let mut extra = Map::new();
    extra.insert("stroke_box".into(), serde_json::json!([x1 + sx0, y1 + sy0, x1 + sx0 + acw, y1 + sy0 + ach]));
    extra.insert("learn_report".into(), report.clone());
    extra.insert("bg_median".into(), serde_json::json!([bg_b[0].round() as i64, bg_b[1].round() as i64, bg_b[2].round() as i64]));
    extra.insert(
        "place".into(),
        serde_json::json!({
            "ref": short,
            "right": (w - (x1 + sx0) - acw) as f64 / short as f64,
            "bottom": (h - (y1 + sy0) - ach) as f64 / short as f64,
        }),
    );
    let profile = Profile {
        id: slug(label),
        label: label.to_string(),
        ref_short_side: ref_short,
        source: format!("pair: {} + {}", path_black.display(), path_white.display()),
        created: String::new(),
        extra,
        aw: acw,
        ah: ach,
        alpha: a_c,
        color: c_c,
    };
    Ok((Some(profile), report))
}

pub fn extract_from_solid(
    path: &Path,
    box_: Option<(i64, i64, i64, i64)>,
    bg: [f32; 3],
    color: [f32; 3],
    ref_short_side: Option<f64>,
    label: &str,
    min_area: u32,
) -> Result<Profile, String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let obs = rgb_f32_from(&img);
    let (x1, y1, x2, y2) = resolve_box(box_, w, h);
    if x2 <= x1 + 2 || y2 <= y1 + 2 {
        return Err("invalid box".into());
    }
    if (color[0] - bg[0]).abs() + (color[1] - bg[1]).abs() + (color[2] - bg[2]).abs() < 8.0 {
        return Err("watermark color and background are too close".into());
    }
    let (rw, rh) = (x2 - x1, y2 - y1);
    let mut alpha = vec![0f32; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let i = ((y1 + y) * w + x1 + x) * 3;
            let mut acc = 0f32;
            for c in 0..3 {
                acc += (obs[i + c] - bg[c]) / (color[c] - bg[c]).max(1e-3);
            }
            alpha[y * rw + x] = (acc / 3.0).clamp(0.0, 1.0);
        }
    }
    clean_alpha(&mut alpha, rw, rh, 0.02, min_area);
    let mut color_map = vec![0f32; rw * rh * 3];
    for i in 0..rw * rh {
        for c in 0..3 {
            color_map[i * 3 + c] = color[c];
        }
    }
    let (a_c, c_c, acw, ach, _, _) = crop_to_alpha(&alpha, &color_map, rw, rh);
    Ok(Profile {
        id: slug(label),
        label: label.to_string(),
        ref_short_side: ref_short_side.unwrap_or(DEFAULT_REF_SHORT),
        source: format!("learn-solid: {}", path.display()),
        created: String::new(),
        extra: Map::new(),
        aw: acw,
        ah: ach,
        alpha: a_c,
        color: c_c,
    })
}

pub fn build_from_uniform(
    path: &Path,
    box_: Option<(i64, i64, i64, i64)>,
    color: [f32; 3],
    ref_short_side: Option<f64>,
    label: &str,
    min_area: u32,
    bg: Option<[f32; 3]>,
    max_resid: f64,
) -> Result<(Option<Profile>, Value), String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let obs = rgb_f32_from(&img);
    let (x1, y1, x2, y2) = resolve_box(box_, w, h);
    if x2 <= x1 + 4 || y2 <= y1 + 4 {
        return Ok((None, serde_json::json!({"reason": "invalid box"})));
    }
    let (rw, rh) = (x2 - x1, y2 - y1);
    let mut win = vec![0f32; rw * rh * 3];
    for y in 0..rh {
        for x in 0..rw {
            for c in 0..3 {
                win[(y * rw + x) * 3 + c] = obs[((y1 + y) * w + x1 + x) * 3 + c];
            }
        }
    }
    let bg_map: Vec<f32> = match bg {
        Some(b) => (0..rw * rh).flat_map(|_| b).collect(),
        None => {
            let stroke = stroke_mask(&win, rw, rh, 60.0, 1.8, 6);
            if stroke.iter().filter(|&&b| b).count() < 20 {
                return Ok((None, serde_json::json!({"reason": "no watermark strokes found in box"})));
            }
            fill_inpaint(&win, rw, rh, 3, &stroke)
        }
    };
    let denom_med = median((0..rw * rh).map(|i| (0..3).map(|c| color[c] - bg_map[i * 3 + c]).sum::<f32>() / 3.0).collect());
    if denom_med < 8.0 {
        return Ok((None, serde_json::json!({"reason": "background too close to watermark color"})));
    }
    let mut alpha = vec![0f32; rw * rh];
    for i in 0..rw * rh {
        let mut acc = 0f32;
        for c in 0..3 {
            acc += (win[i * 3 + c] - bg_map[i * 3 + c]) / (color[c] - bg_map[i * 3 + c]).max(1e-3);
        }
        alpha[i] = (acc / 3.0).clamp(0.0, 1.0);
    }
    clean_alpha(&mut alpha, rw, rh, 0.02, min_area);
    let core = alpha.iter().filter(|&&a| a > 0.3).count();
    if core < 20 {
        return Ok((None, serde_json::json!({"reason": "not enough separable watermark coverage"})));
    }
    let mut resid = 0f64;
    let mut cnt = 0f64;
    for i in 0..rw * rh {
        if alpha[i] <= 0.3 {
            continue;
        }
        for c in 0..3 {
            let hat = alpha[i] * color[c] + (1.0 - alpha[i]) * bg_map[i * 3 + c];
            resid += (hat - win[i * 3 + c]).abs() as f64;
        }
        cnt += 3.0;
    }
    let resid = if cnt > 0.0 { resid / cnt } else { 999.0 };
    let short = w.min(h);
    let bg_med = median_rgb(&bg_map, 3);
    let report = serde_json::json!({
        "reason": if resid > max_resid { "reconstruction residual too high" } else { "ok" },
        "residual": resid,
        "coverage": core as f64 / (rw * rh) as f64,
        "box": [x1, y1, x2, y2],
        "bg_median": [bg_med[0].round() as i64, bg_med[1].round() as i64, bg_med[2].round() as i64],
    });
    if resid > max_resid {
        return Ok((None, report));
    }
    let mut color_map = vec![0f32; rw * rh * 3];
    for i in 0..rw * rh {
        for c in 0..3 {
            color_map[i * 3 + c] = color[c];
        }
    }
    let (a_c, c_c, acw, ach, sx0, sy0) = crop_to_alpha(&alpha, &color_map, rw, rh);
    let ref_short = ref_short_side.unwrap_or(short as f64);
    let mut extra = Map::new();
    extra.insert("stroke_box".into(), serde_json::json!([x1 + sx0, y1 + sy0, x1 + sx0 + acw, y1 + sy0 + ach]));
    extra.insert("learn_report".into(), report.clone());
    extra.insert("bg_median".into(), report.get("bg_median").cloned().unwrap_or(Value::Null));
    extra.insert(
        "place".into(),
        serde_json::json!({
            "ref": short,
            "right": (w - (x1 + sx0) - acw) as f64 / short as f64,
            "bottom": (h - (y1 + sy0) - ach) as f64 / short as f64,
        }),
    );
    Ok((Some(Profile {
        id: slug(label),
        label: label.to_string(),
        ref_short_side: ref_short,
        source: format!("auto: {}", path.display()),
        created: String::new(),
        extra,
        aw: acw,
        ah: ach,
        alpha: a_c,
        color: c_c,
    }), report))
}

fn median_blur_gray(src: &[f32], w: usize, h: usize, k: usize) -> Vec<f32> {
    let r = (k / 2) as isize;
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut vals = Vec::with_capacity(((2 * r + 1) * (2 * r + 1)) as usize);
            for dy in -r..=r {
                for dx in -r..=r {
                    let nx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    let ny = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    vals.push(src[ny * w + nx]);
                }
            }
            out[y * w + x] = median(vals);
        }
    }
    out
}

pub fn learn_from_batch(
    paths: &[PathBuf],
    box_: Option<(i64, i64, i64, i64)>,
    ref_short_side: Option<f64>,
    label: &str,
    min_area: u32,
    max_resid: f64,
) -> Result<(Option<Profile>, Value), String> {
    if paths.len() < 3 {
        return Ok((None, serde_json::json!({"reason": format!("need >=3 aligned samples, got {}", paths.len())})));
    }
    let mut obs_list: Vec<Vec<f32>> = Vec::new();
    let mut dims: Option<(usize, usize)> = None;
    for p in paths {
        let img = image::open(p).map_err(|e| e.to_string())?.to_rgb8();
        let d = (img.width() as usize, img.height() as usize);
        if let Some(prev) = dims {
            if prev != d {
                return Ok((None, serde_json::json!({"reason": "images differ in size; align them first"})));
            }
        }
        dims = Some(d);
        obs_list.push(rgb_f32_from(&img));
    }
    let (w, h) = dims.unwrap();
    let (x1, y1, x2, y2) = resolve_box(box_, w, h);
    let (rw, rh) = (x2 - x1, y2 - y1);
    let k = (rh / 3).clamp(15, 31) | 1;
    // 逐样本顶帽 → 跨样本取中位数：水印是各图共性结构，各图背景高频互不相关，
    // 中位数保留水印结构、抑制背景伪结构（实测同召回下精确率 0.55→0.81、IoU 0.42→0.57）。
    let mut hf_stack: Vec<Vec<f32>> = Vec::with_capacity(obs_list.len());
    for obs in &obs_list {
        let mut lum = vec![0f32; rw * rh];
        for y in 0..rh {
            for x in 0..rw {
                let i = ((y1 + y) * w + x1 + x) * 3;
                lum[y * rw + x] = max3(&obs[i..i + 3]);
            }
        }
        let bgm = median_blur_gray(&lum, rw, rh, k);
        hf_stack.push((0..rw * rh).map(|i| lum[i] - bgm[i]).collect());
    }
    let n_samples = hf_stack.len();
    let mut med_hf = vec![0f32; rw * rh];
    let mut buf: Vec<f32> = Vec::with_capacity(n_samples);
    for i in 0..rw * rh {
        buf.clear();
        for s in &hf_stack {
            buf.push(s[i]);
        }
        buf.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        med_hf[i] = if n_samples % 2 == 1 {
            buf[n_samples / 2]
        } else {
            0.5 * (buf[n_samples / 2 - 1] + buf[n_samples / 2])
        };
    }
    let tophat_min = learn_batch_tophat_min();
    let mut mask = vec![false; rw * rh];
    for i in 0..rw * rh {
        if med_hf[i] > tophat_min {
            mask[i] = true;
        }
    }
    // dilate 3x3
    let mut dil = mask.clone();
    for y in 0..rh {
        for x in 0..rw {
            if mask[y * rw + x] {
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let nx = x as isize + dx;
                        let ny = y as isize + dy;
                        if nx >= 0 && ny >= 0 && nx < rw as isize && ny < rh as isize {
                            dil[ny as usize * rw + nx as usize] = true;
                        }
                    }
                }
            }
        }
    }
    if dil.iter().filter(|&&b| b).count() < 50 {
        return Ok((None, serde_json::json!({"reason": "no spatial watermark structure found in batch"})));
    }
    let mut bg_box: Vec<Vec<f32>> = Vec::new();
    let mut obs_box: Vec<Vec<f32>> = Vec::new();
    for obs in &obs_list {
        let mut win = vec![0f32; rw * rh * 3];
        for y in 0..rh {
            for x in 0..rw {
                for c in 0..3 {
                    win[(y * rw + x) * 3 + c] = obs[((y1 + y) * w + x1 + x) * 3 + c];
                }
            }
        }
        let filled = fill_inpaint(&win, rw, rh, 3, &dil);
        bg_box.push(filled);
        obs_box.push(win);
    }
    let n = obs_list.len();
    let core_alpha = learn_core_alpha();
    let mut alpha = vec![0f32; rw * rh];
    let mut mb = vec![0f32; rw * rh];
    let mut mo = vec![0f32; rw * rh];
    for i in 0..rw * rh {
        let mut sb = 0f32;
        let mut so = 0f32;
        for j in 0..n {
            sb += max3(&bg_box[j][i * 3..i * 3 + 3]);
            so += max3(&obs_box[j][i * 3..i * 3 + 3]);
        }
        mb[i] = sb / n as f32;
        mo[i] = so / n as f32;
    }
    for i in 0..rw * rh {
        let mut var = 0f32;
        let mut cov = 0f32;
        for j in 0..n {
            let b = max3(&bg_box[j][i * 3..i * 3 + 3]) - mb[i];
            let o = max3(&obs_box[j][i * 3..i * 3 + 3]) - mo[i];
            var += b * b;
            cov += b * o;
        }
        var /= n as f32;
        cov /= n as f32;
        let a = (1.0 - cov / var.max(1e-3)).clamp(0.0, 1.0);
        if var > 4.0 && dil[i] && a > core_alpha {
            alpha[i] = a;
        }
    }
    let core_count = alpha.iter().filter(|&&a| a > core_alpha).count();
    if core_count < 50 {
        return Ok((None, serde_json::json!({"reason": "not enough separable watermark coverage across batch"})));
    }
    let mut color = vec![0f32; rw * rh * 3];
    for c in 0..3 {
        for i in 0..rw * rh {
            if alpha[i] <= core_alpha {
                continue;
            }
            let mut num = 0f32;
            for j in 0..n {
                num += obs_box[j][i * 3 + c] - (1.0 - alpha[i]) * bg_box[j][i * 3 + c];
            }
            color[i * 3 + c] = (num / (n as f32 * alpha[i].max(1e-3))).clamp(0.0, 255.0);
        }
    }
    let mut resid = 0f64;
    let mut cnt = 0f64;
    for j in 0..n {
        for i in 0..rw * rh {
            if alpha[i] <= core_alpha {
                continue;
            }
            for c in 0..3 {
                let hat = alpha[i] * color[i * 3 + c] + (1.0 - alpha[i]) * bg_box[j][i * 3 + c];
                resid += (hat - obs_box[j][i * 3 + c]).abs() as f64;
            }
            cnt += 3.0;
        }
    }
    let resid = if cnt > 0.0 { resid / cnt } else { 999.0 };
    let coverage = alpha.iter().filter(|&&a| a > 0.3).count() as f64 / (rw * rh) as f64;
    let report = serde_json::json!({
        "reason": if resid > max_resid { "reconstruction residual too high" } else { "ok" },
        "samples": n,
        "coverage": coverage,
        "residual": resid,
        "box": [x1, y1, x2, y2],
    });
    if resid > max_resid || !(0.003..=0.6).contains(&coverage) {
        return Ok((None, report));
    }
    clean_alpha(&mut alpha, rw, rh, 0.08, min_area);
    for i in 0..rw * rh {
        if alpha[i] <= 0.03 {
            for c in 0..3 {
                color[i * 3 + c] = 0.0;
            }
        }
    }
    let (a_c, c_c, acw, ach, sx0, sy0) = crop_to_alpha(&alpha, &color, rw, rh);
    let mut extra = Map::new();
    extra.insert("stroke_box".into(), serde_json::json!([x1 + sx0, y1 + sy0, x1 + sx0 + acw, y1 + sy0 + ach]));
    extra.insert("learn_report".into(), report.clone());
    // 水印相对右下角的偏移（按短边归一），定位时据此锚定搜索窗：
    // 只靠"贴右下角"假设会在水印离角有距离时（如千问距右/下各 ~0.032）搜错位置。
    let short = w.min(h) as f64;
    extra.insert(
        "place".into(),
        serde_json::json!({
            "ref": short,
            "right": (w - (x1 + sx0) - acw) as f64 / short,
            "bottom": (h - (y1 + sy0) - ach) as f64 / short,
        }),
    );
    Ok((Some(Profile {
        id: slug(label),
        label: label.to_string(),
        ref_short_side: ref_short_side.unwrap_or(DEFAULT_REF_SHORT),
        source: format!("learn_from_batch: {}", n),
        created: String::new(),
        extra,
        aw: acw,
        ah: ach,
        alpha: a_c,
        color: c_c,
    }), report))
}

pub fn auto_discover(
    frames: &[(String, PathBuf, Option<(i64, i64, i64, i64)>)],
    label: &str,
    color: [f32; 3],
    ref_short_side: Option<f64>,
    min_area: u32,
    max_resid: f64,
    min_coverage: f64,
    min_contrast: f64,
) -> Result<(Option<Profile>, Value), String> {
    let mut scored: Vec<Value> = Vec::new();
    let mut eligible: Vec<(PathBuf, (usize, usize, usize, usize), (usize, usize), f64)> = Vec::new();
    for (name, path, box_) in frames {
        let img = match image::open(path) {
            Ok(i) => i.to_rgb8(),
            Err(e) => {
                scored.push(serde_json::json!({"name": name, "reason": e.to_string()}));
                continue;
            }
        };
        let (w, h) = (img.width() as usize, img.height() as usize);
        let obs = rgb_f32_from(&img);
        let box_eff = match box_ {
            Some(b) => Some(*b),
            None => detect_watermark_box(&obs, w, h).map(|(a, b, c, d)| (a as i64, b as i64, c as i64, d as i64)),
        };
        let score = background_score(&obs, w, h, box_eff, color);
        let score = match score {
            Some(s) => s,
            None => {
                scored.push(serde_json::json!({"name": name, "reason": "no watermark strokes found"}));
                continue;
            }
        };
        let b = score.get("box").and_then(Value::as_array).map(|a| {
            (
                a[0].as_i64().unwrap_or(0) as usize,
                a[1].as_i64().unwrap_or(0) as usize,
                a[2].as_i64().unwrap_or(0) as usize,
                a[3].as_i64().unwrap_or(0) as usize,
            )
        });
        let coverage = score.get("coverage").and_then(Value::as_f64).unwrap_or(0.0);
        let contrast = score.get("contrast").and_then(Value::as_f64).unwrap_or(0.0);
        let uniformity = score.get("uniformity").and_then(Value::as_f64).unwrap_or(9.9);
        let mut entry = score.clone();
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("name".into(), Value::String(name.clone()));
        }
        if !(min_coverage..=0.7).contains(&coverage) {
            if let Some(obj) = entry.as_object_mut() {
                obj.insert("reason".into(), Value::String(format!("coverage {coverage:.3} out of range")));
            }
            scored.push(entry);
            continue;
        }
        scored.push(entry);
        if contrast < min_contrast {
            continue;
        }
        if let Some(box_usize) = b {
            eligible.push((path.clone(), box_usize, (w, h), uniformity));
        }
    }
    let report = serde_json::json!({"reason": "no usable frame", "candidates": scored});
    if eligible.is_empty() {
        return Ok((None, report));
    }
    eligible.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
    let mut best: Option<(Profile, Value)> = None;
    for (path, box_usize, _size, _u) in &eligible {
        let box_i = Some((box_usize.0 as i64, box_usize.1 as i64, box_usize.2 as i64, box_usize.3 as i64));
        if let Ok((Some(p), rep)) = build_from_uniform(path, box_i, color, ref_short_side, label, min_area, None, max_resid) {
            best = Some((p, rep));
            break;
        }
    }
    // unsupervised batch over the largest same-size group
    let mut groups: BTreeMap<(usize, usize), Vec<(PathBuf, (usize, usize, usize, usize))>> = BTreeMap::new();
    for (path, box_usize, size, _u) in &eligible {
        groups.entry(*size).or_default().push((path.clone(), *box_usize));
    }
    if let Some(group) = groups.values().max_by_key(|g| g.len()).cloned() {
        if group.len() >= 3 {
            let box_i = Some((group[0].1 .0 as i64, group[0].1 .1 as i64, group[0].1 .2 as i64, group[0].1 .3 as i64));
            let paths: Vec<PathBuf> = group.iter().map(|g| g.0.clone()).collect();
            if let Ok((Some(p), rep)) = learn_from_batch(&paths, box_i, ref_short_side, label, min_area, max_resid) {
                let better = match &best {
                    None => true,
                    Some((_, prev)) => rep.get("residual").and_then(Value::as_f64).unwrap_or(1e9)
                        < prev.get("residual").and_then(Value::as_f64).unwrap_or(1e9),
                };
                if better {
                    best = Some((p, rep));
                }
            }
        }
    }
    match best {
        None => Ok((None, serde_json::json!({"reason": "no frame produced a reliable profile"}))),
        Some((p, rep)) => Ok((Some(p), rep)),
    }
}

// ---------------------------------------------------------------------------
// localisation + removal
// ---------------------------------------------------------------------------

pub fn scaled_layers(profile: &Profile, scale: f64) -> (Vec<f32>, Vec<f32>, usize, usize) {
    let tw = ((profile.aw as f64 * scale).round() as usize).max(1);
    let th = ((profile.ah as f64 * scale).round() as usize).max(1);
    let alpha = resize_f32(&profile.alpha, profile.aw, profile.ah, 1, tw, th);
    let color = resize_f32(&profile.color, profile.aw, profile.ah, 3, tw, th);
    (alpha, color, tw, th)
}

pub fn place_by_anchor(profile: &Profile, width: usize, height: usize) -> Option<(usize, usize, f64)> {
    let place = profile.extra.get("place")?.as_object()?;
    let short = width.min(height) as f64;
    let ref_short = place.get("ref").and_then(Value::as_f64).unwrap_or(profile.ref_short_side);
    let right = place.get("right").and_then(Value::as_f64)?;
    let bottom = place.get("bottom").and_then(Value::as_f64)?;
    let scale = short / ref_short;
    let aw = ((profile.aw as f64 * scale).round() as usize).max(1);
    let ah = ((profile.ah as f64 * scale).round() as usize).max(1);
    let px = (width as f64 - right * short - aw as f64).round() as isize;
    let py = (height as f64 - bottom * short - ah as f64).round() as isize;
    if px < 0 || py < 0 || px as usize + aw > width || py as usize + ah > height {
        return None;
    }
    Some((px as usize, py as usize, scale))
}

pub fn mask_for(
    profile: &Profile,
    px: usize,
    py: usize,
    width: usize,
    height: usize,
    scale: f64,
) -> Option<GrayImage> {
    let (alpha, _c, tw, th) = scaled_layers(profile, scale);
    if py + th > height || px + tw > width {
        return None;
    }
    let thr = MASK_ALPHA_THRESHOLD as f32;
    let mut core = vec![false; tw * th];
    for i in 0..tw * th {
        core[i] = alpha[i] * 255.0 > thr;
    }
    let (dx, dy) = profile
        .extra
        .get("mask_dilate")
        .and_then(Value::as_array)
        .and_then(|a| Some((a.first()?.as_u64()? as usize, a.get(1)?.as_u64()? as usize)))
        .unwrap_or(MASK_DILATE);
    let rx = dx / 2;
    let ry = dy / 2;
    let mut mask = GrayImage::new(width as u32, height as u32);
    for y in 0..th {
        for x in 0..tw {
            if !core[y * tw + x] {
                continue;
            }
            for yy in y.saturating_sub(ry)..=(y + ry).min(th - 1) {
                for xx in x.saturating_sub(rx)..=(x + rx).min(tw - 1) {
                    mask.put_pixel((px + xx) as u32, (py + yy) as u32, Luma([255]));
                }
            }
        }
    }
    Some(mask)
}

/// NCC of the alpha shape against bright/dark top-hat channels, coarse-to-fine
/// over `NCC_SCALE_SPAN`. Returns `(px, py, score, scale)` or `None`.
pub fn locate_ncc(
    gray: &[f32],
    w: usize,
    h: usize,
    profile: &Profile,
    any_position: bool,
) -> Option<(usize, usize, f64, f64)> {
    locate_ncc_impl(gray, w, h, profile, any_position)
}

/// 自动路径的档案定位（NCC）。不用于精确模式/学习自检——那两处走 `locate_gap_impl`。
pub fn locate_ncc_impl(
    gray: &[f32],
    w: usize,
    h: usize,
    profile: &Profile,
    any_position: bool,
) -> Option<(usize, usize, f64, f64)> {
    let short = w.min(h) as f64;
    let base = short / profile.ref_short_side;
    let k = ((short as usize) | 1).clamp(3, 31);
    let r = k / 2;
    let bright: Vec<f32> = {
        let o = open_morph(gray, w, h, r);
        (0..w * h).map(|i| gray[i] - o[i]).collect()
    };
    let dark: Vec<f32> = {
        let c = close_morph(gray, w, h, r);
        (0..w * h).map(|i| c[i] - gray[i]).collect()
    };
    let (x0, y0) = if any_position { (0, 0) } else { ((w / 2), (h as f64 * 0.72) as usize) };
    let (rw, rh) = (w - x0, h - y0);
    if rw < 8 || rh < 8 {
        return None;
    }
    let crop = |src: &[f32]| -> Vec<f32> {
        let mut out = vec![0f32; rw * rh];
        for y in 0..rh {
            for x in 0..rw {
                out[y * rw + x] = src[(y0 + y) * w + x0 + x];
            }
        }
        out
    };
    let mut chans: Vec<Vec<f32>> = vec![crop(&bright), crop(&dark)];
    // normalise each channel (zero-mean, unit-std) over the search crop
    for ch in chans.iter_mut() {
        let mean = ch.iter().sum::<f32>() / ch.len() as f32;
        let var = ch.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / ch.len() as f32;
        let std = var.sqrt().max(1e-6);
        for v in ch.iter_mut() {
            *v = (*v - mean) / std;
        }
    }
    let coarse: Vec<(Vec<f32>, usize, usize)> = chans
        .iter()
        .map(|ch| downscale(ch, rw, rh, 1, NCC_PYRAMID))
        .collect();
    let coarse_cw = coarse[0].1;
    let coarse_ch = coarse[0].2;

    let mut coarse_scales: Vec<f64> = Vec::new();
    let mut f = 1.0 - NCC_SCALE_SPAN;
    while f <= 1.0 + NCC_SCALE_SPAN + 1e-9 {
        coarse_scales.push(base * f);
        f += NCC_COARSE_STEP;
    }
    let mut best: Option<(f64, usize, usize, f64)> = None;
    let consider = |score: f64, px: usize, py: usize, sc: f64, best: &mut Option<(f64, usize, usize, f64)>| {
        if !any_position {
            let tw = (profile.aw as f64 * sc).round() as usize;
            let th = (profile.ah as f64 * sc).round() as usize;
            if px + tw < (w as f64 * 0.9) as usize || py + th < (h as f64 * 0.9) as usize {
                return;
            }
        }
        if best.map(|b| score > b.0).unwrap_or(true) {
            *best = Some((score, px, py, sc));
        }
    };
    for &sc in &coarse_scales {
        let ctw = ((profile.aw as f64 * sc).round() as usize).max(2) / NCC_PYRAMID;
        let cth = ((profile.ah as f64 * sc).round() as usize).max(2) / NCC_PYRAMID;
        if ctw < 2 || cth < 2 || ctw >= coarse_cw || cth >= coarse_ch {
            continue;
        }
        let mut t = resize_f32(&profile.alpha, profile.aw, profile.ah, 1, ctw, cth);
        normalize_unit(&mut t);
        for (data, cw, ch2) in coarse.iter() {
            if let Some((s, cx, cy)) = cosine_scan(data, *cw, *ch2, &t, ctw, cth, 0, *cw - ctw, 0, *ch2 - cth) {
                let px = x0 + cx * NCC_PYRAMID;
                let py = y0 + cy * NCC_PYRAMID;
                consider(s, px, py, sc, &mut best);
            }
        }
    }
    if best.is_none() {
        return None;
    }
    // fine pass: refine scale and position at full resolution
    let (_, bpx, bpy, bsc) = best.unwrap();
    let factor = bsc / base;
    let mut f = 1.0 - NCC_FINE_SPAN;
    let mut fine_scales = Vec::new();
    while f <= 1.0 + NCC_FINE_SPAN + 1e-9 {
        fine_scales.push(base * factor * f);
        f += NCC_FINE_STEP;
    }
    for &sc in &fine_scales {
        let tw = ((profile.aw as f64 * sc).round() as usize).max(2);
        let th = ((profile.ah as f64 * sc).round() as usize).max(2);
        if tw >= rw || th >= rh {
            continue;
        }
        let mut t = resize_f32(&profile.alpha, profile.aw, profile.ah, 1, tw, th);
        normalize_unit(&mut t);
        let x_lo = bpx.saturating_sub(x0).saturating_sub(NCC_PYRAMID);
        let y_lo = bpy.saturating_sub(y0).saturating_sub(NCC_PYRAMID);
        let x_hi = (bpx.saturating_sub(x0) + NCC_PYRAMID).min(rw - tw);
        let y_hi = (bpy.saturating_sub(y0) + NCC_PYRAMID).min(rh - th);
        for ch in chans.iter() {
            if let Some((s, cx, cy)) = cosine_scan(ch, rw, rh, &t, tw, th, x_lo, x_hi, y_lo, y_hi) {
                consider(s, x0 + cx, y0 + cy, sc, &mut best);
            }
        }
    }
    let (score, px, py, sc) = best.unwrap();
    if score < NCC_MIN_SCORE {
        return None;
    }
    Some((px, py, score, sc))
}

/// 档案定位（`match_specific` / `profile_self_check`）用**顶帽 gap-score**
/// （笔画区均亮 − 间隙区均亮），而不是 NCC。
///
/// 为什么不用 NCC：水印在纹理背景上的相关性会被背景高频压垮——实测学习出的豆包 α
/// 在 dist/3、6 上 NCC < 0.35 定位失败（dist/1 也仅 0.5），于是"定位不到 → 回落自动
/// 识别"，精确模式与学习自检都形同虚设。gap-score 只比较"笔画处 vs 非笔画处"，背景
/// 均值被差分抵消，对背景复杂度不敏感：同一档案在 dist/1/3/6 上都精确命中真值
/// （45.5/17.8/43.8），在不含该水印的 dist/7/9/10 上只有 7.3/1.1/5.9。
///
/// 与内置豆包模板匹配（`pipeline::template_stroke_mask`）同口径：同顶帽半径、同
/// "α 二值化成稀疏笔画点 + 笔画/间隙均亮差"的模板；差别是模板来自档案 α、搜索窗
/// **锚定到档案的 `place`（水印相对右下角偏移）** 再 ±40px、并做 ±5% 尺度微扫。
/// 锚点很关键：死守"贴右下角"的窄窗对离角水印（千问距右/下各 ~55px）不含真值，
/// 只会在角上找到伪峰（实测偏 21px）。
fn locate_gap_impl(gray: &[f32], w: usize, h: usize, profile: &Profile) -> Option<(usize, usize, f64, f64)> {
    let short = w.min(h) as f64;
    let base = short / profile.ref_short_side;
    let r = (((short as usize) | 1).clamp(3, 31)) / 2;
    let opened = open_morph(gray, w, h, r);
    let top: Vec<f32> = (0..w * h).map(|i| gray[i] - opened[i]).collect();
    // 前缀和求任意矩形和 O(1)（与内置模板匹配同一技巧）
    let iw = w + 1;
    let mut integral = vec![0f64; iw * (h + 1)];
    for y in 0..h {
        let mut acc = 0f64;
        for x in 0..w {
            acc += top[y * w + x] as f64;
            integral[(y + 1) * iw + x + 1] = integral[y * iw + x + 1] + acc;
        }
    }
    let rect_sum = |x1: usize, y1: usize, x2: usize, y2: usize| -> f64 {
        integral[y2 * iw + x2] + integral[y1 * iw + x1] - integral[y1 * iw + x2] - integral[y2 * iw + x1]
    };
    let margin = 40usize;
    let mut best: Option<(f64, usize, usize, f64)> = None;
    for sc in [base * 0.95, base, base * 1.05] {
        let tw = (profile.aw as f64 * sc).round() as usize;
        let th = (profile.ah as f64 * sc).round() as usize;
        if tw < 4 || th < 4 || tw >= w || th >= h {
            continue;
        }
        let t = resize_f32(&profile.alpha, profile.aw, profile.ah, 1, tw, th);
        let pts: Vec<(usize, usize)> = (0..tw * th)
            .filter(|&i| t[i] > LEARN_TEMPLATE_BIN)
            .map(|i| (i % tw, i / tw))
            .collect();
        let n_in = pts.len() as f64;
        let n_out = (tw * th) as f64 - n_in;
        if n_in < 8.0 || n_out <= 0.0 {
            continue;
        }
        // 搜索窗锚定在档案记录的右下角偏移上（`place`），而不是死板的"贴右下角"：
        // 豆包贴角（right=bottom=0）→ 窗就是右下角；千问距右/下各 ~0.032（55px）→ 窗
        // 跟着移到水印处。否则窄窗根本不含真值，只会在角上找到伪峰。
        let (ax, ay) = place_by_anchor(profile, w, h)
            .map(|(px, py, _)| (px, py))
            .unwrap_or((w - tw, h - th));
        let (min_px, max_px) = (ax.saturating_sub(margin), (ax + margin).min(w - tw));
        let (min_py, max_py) = (ay.saturating_sub(margin), (ay + margin).min(h - th));
        if max_px < min_px || max_py < min_py {
            continue;
        }
        for py in min_py..=max_py {
            for px in min_px..=max_px {
                let mut s_in = 0f64;
                for &(dx, dy) in &pts {
                    s_in += top[(py + dy) * w + px + dx] as f64;
                }
                let s_all = rect_sum(px, py, px + tw, py + th);
                let gap = s_in / n_in - (s_all - s_in) / n_out;
                if best.map(|b| gap > b.0).unwrap_or(true) {
                    best = Some((gap, px, py, sc));
                }
            }
        }
    }
    best.map(|(gap, px, py, sc)| (px, py, gap, sc))
}

fn normalize_unit(v: &mut [f32]) {
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let var = v.iter().map(|&x| (x - mean) * (x - mean)).sum::<f32>() / v.len() as f32;
    let std = var.sqrt().max(1e-6);
    for x in v.iter_mut() {
        *x = (*x - mean) / std;
    }
}

#[allow(clippy::too_many_arguments)]
fn cosine_scan(
    ch: &[f32],
    cw: usize,
    chh: usize,
    tmpl: &[f32],
    tw: usize,
    th: usize,
    x_lo: usize,
    x_hi: usize,
    y_lo: usize,
    y_hi: usize,
) -> Option<(f64, usize, usize)> {
    if tw >= cw || th >= chh || x_lo > x_hi || y_lo > y_hi {
        return None;
    }
    let mut best: Option<(f64, usize, usize)> = None;
    // `tmpl` is zero-mean/unit-variance, so its sum of squares is tw*th.
    let tnorm = ((tw * th) as f64).sqrt();
    for py in y_lo..=y_hi {
        for px in x_lo..=x_hi {
            let mut dot = 0f64;
            let mut norm2 = 0f64;
            for j in 0..th {
                let base = (py + j) * cw + px;
                let c = &ch[base..base + tw];
                let t = &tmpl[j * tw..j * tw + tw];
                for i in 0..tw {
                    let v = c[i] as f64;
                    dot += t[i] as f64 * v;
                    norm2 += v * v;
                }
            }
            let score = dot / (tnorm * norm2.sqrt()).max(1e-9);
            if best.map(|b| score > b.0).unwrap_or(true) {
                best = Some((score, px, py));
            }
        }
    }
    best
}

/// 逆解的可选门控与写入范围限制（默认 = 全关闭，保持档案原有行为）。
#[derive(Default)]
pub struct InverseParams<'a> {
    /// Some(tol)：|短边/ref_short_side − 1| ≤ tol 才逆解（stamp 只在 scale≈1 标定）。
    pub scale_tol: Option<f64>,
    /// Some(min)：水印邻域高频能量 ≥ min 才逆解（低纹理背景逆解只会放大噪声）。
    pub texture_min: Option<f64>,
    /// 残影上限覆盖（豆包 stamp 要求 0.12，档案默认 0.6）。
    pub ghost_max: Option<f64>,
    /// 额外写入范围（只写该 mask 内像素，保住"mask 外零改动"硬保证）。
    pub write_mask: Option<&'a GrayImage>,
}

/// 兼容旧签名：无门控，写入范围由 α>0.03 决定。
pub fn inverse_image(
    obs: &RgbImage,
    mat: &RgbImage,
    profile: &Profile,
    px: usize,
    py: usize,
    scale: f64,
) -> Result<(RgbImage, String), String> {
    match inverse_image_gated(obs, mat, profile, px, py, scale, &InverseParams::default())? {
        Some(v) => Ok(v),
        None => Err("inverse skipped (gated)".into()),
    }
}

/// 水印邻域高频能量（Python INVERSE_TEXTURE_MIN 判据）：区域内 |原图 − Gaussian(σ2.0)| 均值。
fn texture_hf(obs_f: &[f32], w: usize, h: usize, px: usize, py: usize, tw: usize, th: usize) -> f64 {
    let y0 = py.saturating_sub(40);
    let x0 = px.saturating_sub(60);
    let y1 = (py + th + 40).min(h);
    let x1 = (px + tw + 60).min(w);
    let (rw, rh) = (x1 - x0, y1 - y0);
    if rw == 0 || rh == 0 {
        return 0.0;
    }
    let mut reg = vec![0f32; rw * rh * 3];
    for y in 0..rh {
        for x in 0..rw {
            let si = ((y0 + y) * w + x0 + x) * 3;
            let di = (y * rw + x) * 3;
            reg[di..di + 3].copy_from_slice(&obs_f[si..si + 3]);
        }
    }
    let blur = gauss_blur(&reg, rw, rh, 3, 2.0);
    let mut sum = 0f64;
    for i in 0..rw * rh * 3 {
        sum += (reg[i] as f64 - blur[i] as f64).abs();
    }
    sum / (rw * rh * 3) as f64
}

/// 解析逆解（obs = α·C + (1−α)·bg）。门控不通过或逆解不可靠时返回 Ok(None)
/// （调用方保持生成式结果）。逐图自校正 α 增益（最小化残影与 α 的相关性），
/// 失解/饱和像素逐像素回退生成式结果。
pub fn inverse_image_gated(
    obs: &RgbImage,
    mat: &RgbImage,
    profile: &Profile,
    px: usize,
    py: usize,
    scale: f64,
    params: &InverseParams,
) -> Result<Option<(RgbImage, String)>, String> {
    let (w, h) = (obs.width() as usize, obs.height() as usize);
    if let Some(tol) = params.scale_tol {
        let short = w.min(h) as f64;
        if (short / profile.ref_short_side - 1.0).abs() > tol {
            return Ok(None);
        }
    }
    let (palpha, pcolor, tw, th) = scaled_layers(profile, scale);
    if px + tw > w || py + th > h {
        return Err("profile outside image".into());
    }
    let obs_f = rgb_f32_from(obs);
    let mat_f = rgb_f32_from(mat);
    if let Some(tmin) = params.texture_min {
        if texture_hf(&obs_f, w, h, px, py, tw, th) < tmin {
            return Ok(None);
        }
    }
    // model-fit gate: can a per-pixel colour C explain the blend at all?
    let mut core = 0usize;
    let mut fit = 0f64;
    for y in 0..th {
        for x in 0..tw {
            let a = palpha[y * tw + x];
            if a <= 0.3 {
                continue;
            }
            core += 1;
            for c in 0..3 {
                let oi = ((py + y) * w + px + x) * 3 + c;
                let pred = a * pcolor[(y * tw + x) * 3 + c] + (1.0 - a) * mat_f[oi];
                fit += (pred - obs_f[oi]).abs() as f64;
            }
        }
    }
    if core < 20 {
        return Ok(None);
    }
    let fit = fit / (core * 3) as f64;
    if fit > INVERSE_MAX_FIT {
        return Ok(None);
    }
    // gain search minimising ghosting of the alpha into the high-frequency residual
    let mut best: Option<(f64, f64)> = None;
    let mut k = 0.8f64;
    while k <= 1.25 + 1e-9 {
        if let Some(ghost) = ghost_for(&obs_f, &palpha, &pcolor, w, h, px, py, tw, th, k) {
            if best.map(|b| ghost < b.0).unwrap_or(true) {
                best = Some((ghost, k));
            }
        }
        k += 0.05;
    }
    let Some((ghost, k)) = best else { return Ok(None) };
    if ghost > INVERSE_MAX_GHOST {
        return Ok(None);
    }
    let mut out = obs.clone();
    for y in 0..th {
        for x in 0..tw {
            let a = (palpha[y * tw + x] as f64 * k).clamp(0.0, 1.0) as f32;
            if a as f64 <= INVERSE_ALPHA_THRESHOLD {
                continue;
            }
            if let Some(wm) = params.write_mask {
                if wm.get_pixel((px + x) as u32, (py + y) as u32).0[0] == 0 {
                    continue;
                }
            }
            let oi = ((py + y) * w + px + x) * 3;
            let mut inv = [0f32; 3];
            let mut ill = false;
            if obs_f[oi] >= 252.0 && obs_f[oi + 1] >= 252.0 && obs_f[oi + 2] >= 252.0 {
                ill = true;
            }
            for c in 0..3 {
                let raw = (obs_f[oi + c] - a * pcolor[(y * tw + x) * 3 + c]) / (1.0 - a).max(1e-3);
                if raw < -0.5 || raw > 255.5 {
                    ill = true;
                }
                inv[c] = raw.clamp(0.0, 255.0);
            }
            let val = if ill {
                [mat_f[oi], mat_f[oi + 1], mat_f[oi + 2]]
            } else {
                inv
            };
            out.put_pixel(
                (px + x) as u32,
                (py + y) as u32,
                image::Rgb([val[0].round().clamp(0.0, 255.0) as u8,
                            val[1].round().clamp(0.0, 255.0) as u8,
                            val[2].round().clamp(0.0, 255.0) as u8]),
            );
        }
    }
    Ok(Some((
        out,
        format!("profile {} pos {px},{py} gain {k:.2} ghost {ghost:.3}", profile.id),
    )))
}

/// Separable Gaussian blur of an interleaved f32 window.
fn gauss_blur(src: &[f32], w: usize, h: usize, nc: usize, sigma: f64) -> Vec<f32> {
    let radius = (2.0 * sigma).ceil() as isize;
    let mut kernel = Vec::with_capacity((2 * radius + 1) as usize);
    for i in -radius..=radius {
        let x = i as f64;
        kernel.push((-(x * x) / (2.0 * sigma * sigma)).exp());
    }
    let sum: f64 = kernel.iter().sum();
    for k in kernel.iter_mut() {
        *k /= sum;
    }
    let mut tmp = vec![0f32; w * h * nc];
    for y in 0..h {
        for x in 0..w {
            for c in 0..nc {
                let mut acc = 0f64;
                for (ki, &kv) in kernel.iter().enumerate() {
                    let sx = (x as isize + ki as isize - radius).clamp(0, w as isize - 1) as usize;
                    acc += kv * src[(y * w + sx) * nc + c] as f64;
                }
                tmp[(y * w + x) * nc + c] = acc as f32;
            }
        }
    }
    let mut out = vec![0f32; w * h * nc];
    for y in 0..h {
        for x in 0..w {
            for c in 0..nc {
                let mut acc = 0f64;
                for (ki, &kv) in kernel.iter().enumerate() {
                    let sy = (y as isize + ki as isize - radius).clamp(0, h as isize - 1) as usize;
                    acc += kv * tmp[(sy * w + x) * nc + c] as f64;
                }
                out[(y * w + x) * nc + c] = acc as f32;
            }
        }
    }
    out
}

/// Ghost proxy: correlation between the coverage map and the high-frequency
/// content of the analytic inverse (a correct profile leaves no residual
/// structure shaped like the alpha mask).
#[allow(clippy::too_many_arguments)]
fn ghost_for(
    obs: &[f32],
    palpha: &[f32],
    pcolor: &[f32],
    w: usize,
    _h: usize,
    px: usize,
    py: usize,
    tw: usize,
    th: usize,
    k: f64,
) -> Option<f64> {
    let n = tw * th;
    let mut a_map = vec![0f32; n];
    let mut inv = vec![0f32; n * 3];
    let mut active = 0usize;
    for y in 0..th {
        for x in 0..tw {
            let i = y * tw + x;
            let a = (palpha[i] as f64 * k).clamp(0.0, 1.0);
            a_map[i] = a as f32;
            if a > INVERSE_ALPHA_THRESHOLD {
                active += 1;
            }
            let oi = ((py + y) * w + px + x) * 3;
            for c in 0..3 {
                let raw = (obs[oi + c] as f64 - a * pcolor[i * 3 + c] as f64) / (1.0 - a).max(1e-3);
                inv[i * 3 + c] = raw.clamp(0.0, 255.0) as f32;
            }
        }
    }
    if active < 50 {
        return None;
    }
    let blurred = gauss_blur(&inv, tw, th, 3, 2.5);
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    for i in 0..n {
        if a_map[i] as f64 <= INVERSE_ALPHA_THRESHOLD {
            continue;
        }
        // 与 Python 一致：跨通道取**有符号** max（(inv − blur).max(axis=2)），
        // 不是 abs——abs 会改变残影与 α 的相关方向，导致增益选偏。
        let hf = (0..3)
            .map(|c| inv[i * 3 + c] - blurred[i * 3 + c])
            .fold(f32::MIN, f32::max);
        xs.push(a_map[i] as f64);
        ys.push(hf as f64);
    }
    let cnt = xs.len() as f64;
    let mx = xs.iter().sum::<f64>() / cnt;
    let my = ys.iter().sum::<f64>() / cnt;
    let mut cov = 0f64;
    let mut vx = 0f64;
    let mut vy = 0f64;
    for i in 0..xs.len() {
        cov += (xs[i] - mx) * (ys[i] - my);
        vx += (xs[i] - mx).powi(2);
        vy += (ys[i] - my).powi(2);
    }
    if vx <= 0.0 || vy <= 0.0 {
        return Some(1.0);
    }
    Some((cov / (vx * vy).sqrt()).abs())
}

fn gray_max_channel(img: &image::DynamicImage) -> (Vec<f32>, usize, usize) {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let gray = rgb
        .pixels()
        .map(|p| max3(&[p.0[0] as f32, p.0[1] as f32, p.0[2] as f32]))
        .collect::<Vec<f32>>();
    (gray, w, h)
}

/// 用一个具体档案在图上定位（自动路径，NCC，真实 α）。
pub fn locate_profile(
    img: &image::DynamicImage,
    profile: &Profile,
    any_position: bool,
) -> Option<(usize, usize, f64, f64)> {
    let (gray, w, h) = gray_max_channel(img);
    locate_ncc_impl(&gray, w, h, profile, any_position)
}

/// 精确模式 / 学习自检用的定位（顶帽 gap-score，见 `locate_gap_impl`）。
pub fn locate_profile_dense(
    img: &image::DynamicImage,
    profile: &Profile,
) -> Option<(usize, usize, f64, f64)> {
    let (gray, w, h) = gray_max_channel(img);
    locate_gap_impl(&gray, w, h, profile)
}

/// 用户显式指定的档案：只用该档案定位（"精确模式"），阈值放宽到
/// `FORCED_MIN_SCORE`——用户已声明"这组图就是这个水印"，定位分数偏低
/// （压缩/缩放/裁剪痕迹）也应接受。
pub fn match_specific(
    img: &image::DynamicImage,
    id: &str,
) -> Option<(Profile, usize, usize, f64, f64)> {
    let profile = load_profile(id).ok()?;
    let (px, py, score, scale) = locate_profile_dense(img, &profile)?;
    if score < PROFILE_GAP_MIN_SCORE {
        return None;
    }
    Some((profile, px, py, score, scale))
}

/// 档案摘要（列表展示用，不含大图数据）。
#[derive(serde::Serialize)]
pub struct ProfileSummary {
    pub id: String,
    pub label: String,
    pub created: String,
    pub source: String,
    pub width: usize,
    pub height: usize,
    pub builtin: bool,
}

pub fn profile_summaries() -> Vec<ProfileSummary> {
    let builtin_ids: Vec<String> = EMBEDDED.iter().map(|(id, _, _, _)| id.to_string()).collect();
    list_profiles()
        .into_iter()
        .map(|p| ProfileSummary {
            builtin: builtin_ids.contains(&p.id),
            id: p.id,
            label: p.label,
            created: p.created,
            source: p.source,
            width: p.aw,
            height: p.ah,
        })
        .collect()
}

/// 删除用户学习的档案（内置档案不可删）。
pub fn delete_profile(id: &str) -> Result<(), String> {
    if EMBEDDED.iter().any(|(eid, _, _, _)| *eid == id) {
        return Err(format!("builtin profile cannot be deleted: {id}"));
    }
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!("invalid profile id: {id}"));
    }
    let dir = profiles_dir().join(id);
    if !dir.exists() {
        return Err(format!("profile not found: {id}"));
    }
    fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}

pub fn match_image(img: &image::DynamicImage) -> Option<(Profile, usize, usize, f64, f64)> {
    let (gray, w, h) = gray_max_channel(img);
    let mut best: Option<(Profile, usize, usize, f64, f64)> = None;
    for profile in list_profiles() {
        if let Some((px, py, score, scale)) = locate_ncc(&gray, w, h, &profile, false) {
            if best.as_ref().map(|b| score > b.3).unwrap_or(true) {
                best = Some((profile, px, py, score, scale));
            }
        }
    }
    best
}

/// 学习结果自检：把候选档案在**用于学习的原图**上重新定位，返回
/// (命中数, 样本数, 平均分数)。定位不到 = 档案无法复用（α 太弱/不成形），
/// 这种档案存了也没用，应在保存前拦掉。
pub fn profile_self_check(
    profile: &Profile,
    paths: &[PathBuf],
) -> (usize, usize, f64) {
    let mut hit = 0usize;
    let mut total = 0usize;
    let mut score_sum = 0.0f64;
    for p in paths {
        let Ok(img) = image::open(p) else { continue };
        total += 1;
        if let Some((_, _, score, _)) = locate_profile_dense(&img, profile) {
            if score >= PROFILE_GAP_MIN_SCORE {
                hit += 1;
                score_sum += score;
            }
        }
    }
    let mean = if hit > 0 { score_sum / hit as f64 } else { 0.0 };
    (hit, total, mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_keeps_constant() {
        let src = vec![7f32; 10 * 10];
        let out = resize_f32(&src, 10, 10, 1, 5, 5);
        assert!(out.iter().all(|&v| (v - 7.0).abs() < 1e-4));
    }

    #[test]
    fn clean_alpha_drops_specks() {
        let (w, h) = (20usize, 20usize);
        let mut a = vec![0f32; w * h];
        a[5 * w + 5] = 0.9;
        for y in 8..12 {
            for x in 8..12 {
                a[y * w + x] = 0.9;
            }
        }
        clean_alpha(&mut a, w, h, 0.02, 8);
        assert_eq!(a[5 * w + 5], 0.0);
        assert!(a[8 * w + 8] > 0.5);
    }

    #[test]
    fn pair_roundtrip_recovers_background() {
        // synthetic: 120x90 near-black + near-white with a 0.6-alpha white blob
        let (w, h) = (120usize, 90usize);
        let mut black = vec![1f32; w * h * 3];
        let mut white = vec![254f32; w * h * 3];
        let (bx0, by0, bx1, by1) = (60usize, 50usize, 110usize, 80usize);
        for y in by0..by1 {
            for x in bx0..bx1 {
                if (x + y) % 3 != 0 {
                    continue; // glyph-like sparse strokes
                }
                for c in 0..3 {
                    let i = (y * w + x) * 3 + c;
                    black[i] = 0.6 * 255.0 + 0.4 * black[i];
                    white[i] = 0.6 * 255.0 + 0.4 * white[i];
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("wmprof-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let bl = dir.join("black.png");
        let wh = dir.join("white.png");
        RgbImage::from_fn(w as u32, h as u32, |x, y| {
            let i = ((y * w as u32 + x) * 3) as usize;
            image::Rgb([black[i] as u8, black[i + 1] as u8, black[i + 2] as u8])
        })
        .save(&bl)
        .unwrap();
        RgbImage::from_fn(w as u32, h as u32, |x, y| {
            let i = ((y * w as u32 + x) * 3) as usize;
            image::Rgb([white[i] as u8, white[i + 1] as u8, white[i + 2] as u8])
        })
        .save(&wh)
        .unwrap();
        let box_ = Some((bx0 as i64 - 5, by0 as i64 - 5, bx1 as i64 + 5, by1 as i64 + 5));
        let (profile, report) = extract_from_pair(&bl, &wh, box_, Some(90.0), "synthetic", 4, 6.0).unwrap();
        let profile = profile.expect("pair profile must be learned");
        assert!(report["residual_black"].as_f64().unwrap() < 2.0);
        let anchor = place_by_anchor(&profile, w, h).expect("anchor");
        let mask = mask_for(&profile, anchor.0, anchor.1, w, h, anchor.2).expect("mask");
        assert!(mask.iter().any(|&v| v == 255));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Real-asset smoke test (requires the repo checkout); run with `--ignored`.
    #[test]
    #[ignore]
    fn qwen_profile_matches_real_images() {
        let root = crate::project_root();
        for (name, expect_scale) in [
            ("dist/7.png", 1.0),
            ("dist/8.png", 1248.0 / 1760.0),
            ("dist/9.png", 1.0),
            ("dist/10.png", 1.0),
        ] {
            let path = root.join(name);
            if !path.exists() {
                continue;
            }
            let img = image::open(&path).unwrap();
            let (w, h) = (img.width() as usize, img.height() as usize);
            let rgb = img.to_rgb8();
            let mut gray = vec![0f32; w * h];
            for (i, p) in rgb.pixels().enumerate() {
                gray[i] = max3(&[p.0[0] as f32, p.0[1] as f32, p.0[2] as f32]);
            }
            let profile = load_profile("qwen").unwrap();
            let got = locate_ncc(&gray, w, h, &profile, false)
                .unwrap_or_else(|| panic!("{name}: qwen profile must match"));
            let (px, py, score, scale) = got;
            assert!(score >= NCC_MIN_SCORE, "{name}: low score {score}");
            assert!((scale - expect_scale).abs() < 0.05, "{name}: scale {scale} vs {expect_scale}");
            println!("{name}: px={px} py={py} score={score:.3} scale={scale:.3}");
        }
    }

    /// The standalone detector must find the tight bottom-right box on both
    /// shipped samples (guards the "background component floods the mask" bug);
    /// `--ignored`.
    #[test]
    #[ignore]
    fn detect_box_on_qwen_samples() {
        let root = crate::project_root();
        for (name, want) in [
            ("qwen-black.png", (1269usize, 2223usize, 1711usize, 2319usize)),
            ("qwen-white.png", (1386, 2236, 1701, 2306)),
        ] {
            let path = root.join("tools/watermarks/qwen/samples").join(name);
            let img = image::open(&path).unwrap().to_rgb8();
            let (w, h) = (img.width() as usize, img.height() as usize);
            let rgb = rgb_f32_from(&img);
            let got = detect_watermark_box(&rgb, w, h).expect("box");
            let short = w.min(h) as f64;
            for (g, e) in [(got.0, want.0), (got.1, want.1), (got.2, want.2), (got.3, want.3)] {
                assert!(
                    (g as f64 - e as f64).abs() <= short * 0.01,
                    "{name}: box {got:?} far from {want:?}"
                );
            }
            println!("{name}: box {got:?}");
        }
    }

    /// Pair learning on the shipped qwen black/white samples; `--ignored`.
    #[test]
    #[ignore]
    fn qwen_pair_learning_residual_is_tiny() {
        let root = crate::project_root();
        let bl = root.join("tools/watermarks/qwen/samples/qwen-black.png");
        let wh = root.join("tools/watermarks/qwen/samples/qwen-white.png");
        if !bl.exists() || !wh.exists() {
            return;
        }
        let (profile, report) = extract_from_pair(&bl, &wh, None, Some(1760.0), "qwen-test", 24, 6.0).unwrap();
        assert!(profile.is_some(), "pair learning must succeed: {report}");
        assert!(report["residual_black"].as_f64().unwrap() < 1.0, "{report}");
        assert!(report["residual_white"].as_f64().unwrap() < 1.0, "{report}");
        println!("pair report: {report}");
    }

    /// Batch learning on the aligned doubao samples must recover the stamp shape from pixels
    /// alone (`--ignored`); guards the median-across-samples stroke detector against regressions.
    #[test]
    #[ignore]
    fn doubao_batch_learning_recovers_stamp() {
        let root = crate::project_root();
        let paths: Vec<std::path::PathBuf> = ["dist/1.png", "dist/2.png", "dist/3.png", "dist/6.png"]
            .iter()
            .map(|n| root.join(n))
            .filter(|p| p.exists())
            .collect();
        if paths.len() < 3 {
            return;
        }
        let (profile, report) =
            learn_from_batch(&paths, None, None, "doubao-test", 24, 12.0).unwrap();
        assert!(report["residual"].as_f64().unwrap() < 12.0, "{report}");
        let profile = profile.unwrap_or_else(|| panic!("batch learning must succeed: {report}"));
        println!("learn report: {report}");

        let truth_path = root.join("tools/doubao-wm-stamp-alpha.png");
        if !truth_path.exists() {
            return;
        }
        let truth = image::open(&truth_path).unwrap().to_luma8();
        let (tw, th) = truth.dimensions();
        let (mut tx1, mut ty1, mut tx2, mut ty2) = (tw, th, 0u32, 0u32);
        for (x, y, p) in truth.enumerate_pixels() {
            if p.0[0] > 38 {
                tx1 = tx1.min(x);
                ty1 = ty1.min(y);
                tx2 = tx2.max(x + 1);
                ty2 = ty2.max(y + 1);
            }
        }
        let (tox, toy) = (2541u32, 1490u32); // 真值 stamp 在 dist 图中的摆放位置
        let sb = profile.extra["stroke_box"].as_array().expect("stroke_box");
        let (sx, sy) = (sb[0].as_u64().unwrap() as u32, sb[1].as_u64().unwrap() as u32);

        let (ox, oy, cw, ch) = (2400u32, 1430u32, 560u32, 250u32);
        let mut ca = vec![false; (cw * ch) as usize];
        for y in 0..profile.ah {
            for x in 0..profile.aw {
                if profile.alpha[y * profile.aw + x] > 8.0 / 255.0 {
                    let (px, py) = (sx + x as u32, sy + y as u32);
                    if px >= ox && py >= oy && px < ox + cw && py < oy + ch {
                        ca[((py - oy) * cw + px - ox) as usize] = true;
                    }
                }
            }
        }
        let mut ct = vec![false; (cw * ch) as usize];
        for (x, y, p) in truth.enumerate_pixels() {
            if p.0[0] > 38 {
                let (px, py) = (tox + x, toy + y);
                if px >= ox && py >= oy && px < ox + cw && py < oy + ch {
                    ct[((py - oy) * cw + px - ox) as usize] = true;
                }
            }
        }
        let inter = (0..ca.len()).filter(|&i| ca[i] && ct[i]).count();
        let union = (0..ca.len()).filter(|&i| ca[i] || ct[i]).count();
        let recall = inter as f64 / (tx2 - tx1 + 1).max(1) as f64;
        println!(
            "learned {}x{} at ({sx},{sy}) vs truth ink {tox},{toy},{}x{} IoU={:.3}",
            profile.aw,
            profile.ah,
            tx2 - tx1,
            ty2 - ty1,
            inter as f64 / union.max(1) as f64
        );
        assert!(
            inter as f64 / union.max(1) as f64 >= 0.85,
            "learned alpha IoU too low vs ground-truth stamp"
        );
        assert!(recall >= 0.6, "learned alpha misses too much of the stamp ink");
    }

    /// 精确模式 / 学习自检的定位器：顶帽 gap-score + 档案 `place` 锚点。`--ignored`.
    /// 锚点很关键——千问水印距右/下各 ~0.032 短边（55px），死守"贴右下角"的窄窗
    /// 根本不含真值，只会在角上找到伪峰（实测 (1297,2253) 差 21px）。
    #[test]
    #[ignore]
    fn gap_locator_finds_qwen_place_anchor() {
        let root = crate::project_root();
        let profile = load_profile("qwen").unwrap();
        for (name, want) in [
            ("dist/7.png", (1276usize, 2230usize)),
            ("dist/8.png", (906, 1567)),
            ("dist/9.png", (1276, 2230)),
            ("dist/10.png", (1276, 2230)),
        ] {
            let path = root.join(name);
            if !path.exists() {
                continue;
            }
            let img = image::open(&path).unwrap();
            let (px, py, score, _) = locate_profile_dense(&img, &profile)
                .unwrap_or_else(|| panic!("{name}: qwen profile must locate via gap-score"));
            assert!(score >= PROFILE_GAP_MIN_SCORE, "{name}: low gap score {score}");
            assert!(
                (px as isize - want.0 as isize).abs() <= 3 && (py as isize - want.1 as isize).abs() <= 3,
                "{name}: pos ({px},{py}) far from {want:?}"
            );
            println!("{name}: gap score {score:.1} at ({px},{py})");
        }
    }
}

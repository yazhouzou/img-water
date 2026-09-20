use std::fs;
use std::path::{Path, PathBuf};

use ab_glyph::FontArc;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, RgbImage};
use imageproc::drawing::draw_text_mut;

use crate::lama::Lama;

const WINDOW: u32 = 512;

// 模板笔画 mask：豆包水印字形全图固定（半透明白字 α≈0.6 叠加）。二值笔画模板
// （阈值 ≥125 + 3x3 闭运算，填充率仅 ~21%）只用于 gap-score 匹配；mask 本身用
// 连续 α 图（黑底样张提取）取 α>0.03 的真实污染像素，最小侵入。
// 资产编译进二进制，CLI 与打包 App 均可用。
const TEMPLATE_PNG: &[u8] = include_bytes!("../../../tools/doubao-wm-template.png");
const TEMPLATE_ALPHA_PNG: &[u8] = include_bytes!("../../../tools/doubao-wm-alpha.png");
const TEMPLATE_META_JSON: &str = include_str!("../../../tools/doubao-wm-template.json");
// 顶帽 gap-score（笔画区均亮 - 间隙区均亮）阈值：正样本 ≥33 / 负样本 ≤8.4，
// 取中间空档 20（对齐 Python TEMPLATE_MIN_SCORE）。
const TEMPLATE_MIN_SCORE: f64 = 20.0;
// 连续 α mask 阈值 8/255 ≈ α>0.03：水印真正污染的像素是 α>0（含抗锯齿带）。
// 二值核（α>0.5）只覆盖笔画核心，只能靠大膨胀补抗锯齿 → 多盖干净画面被模型
// 重绘，正是"影响周边元素"的根因（Python 实测改动面积 -10%~-30%）。
const TEMPLATE_ALPHA_THRESHOLD: u8 = 8;
// mask 膨胀 ±1px，仅补偿缩放/对齐误差（对齐 Python TEMPLATE_STROKE_DILATE）。
const TEMPLATE_DILATE_W: usize = 3;
const TEMPLATE_DILATE_H: usize = 3;
// 顶帽开运算核半径（31x31 椭圆，对齐 Python k=min(31, ...)）。
const TOPHAT_RADIUS: usize = 15;

#[derive(serde::Deserialize)]
struct TemplateMeta {
    #[serde(rename = "ref_short_side")]
    ref_short_side: f64,
}

fn load_template() -> Result<(GrayImage, GrayImage, TemplateMeta), String> {
    let tpl = image::load_from_memory_with_format(TEMPLATE_PNG, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .to_luma8();
    let alpha = image::load_from_memory_with_format(TEMPLATE_ALPHA_PNG, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .to_luma8();
    let meta: TemplateMeta =
        serde_json::from_str(TEMPLATE_META_JSON).map_err(|e| e.to_string())?;
    Ok((tpl, alpha, meta))
}

/// 椭圆核半径表（完全对齐 cv2.getStructuringElement(MORPH_ELLIPSE, (2r+1,2r+1))：
/// 半宽 = round(r·√(1-(dy/r)²))，cvRound 为四舍五入，注意不是 (r+0.5)/floor）。
/// 按 dy 给出水平半宽，用于把椭圆形态学分解成"逐行一维滑窗"的 O(r·n) 算法。
fn ellipse_half(r: usize, dy: i64) -> usize {
    let rf = r as f64;
    let v = 1.0 - (dy as f64).powi(2) / (rf * rf);
    if v <= 0.0 { 0 } else { (rf * v.sqrt()).round() as usize }
}

/// 一维居中滑窗极值（窗口 [x-half, x+half]，越界位置忽略——与 cv2 形态学
/// 默认边界一致）。单调队列实现，O(n)。
fn slide_center(src: &[f64], half: usize, dilate: bool) -> Vec<f64> {
    let n = src.len();
    let mut out = vec![0f64; n];
    if n == 0 {
        return out;
    }
    let half = half as i64;
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut filled: i64 = -1;
    for i in 0..n as i64 {
        while let Some(&b) = dq.back() {
            let better = if dilate { src[i as usize] >= src[b] } else { src[i as usize] <= src[b] };
            if better { dq.pop_back(); } else { break; }
        }
        dq.push_back(i as usize);
        let center = i - half;
        if center >= 0 {
            let left = center - half;
            while let Some(&f) = dq.front() {
                if (f as i64) < left { dq.pop_front(); } else { break; }
            }
            out[center as usize] = src[*dq.front().unwrap()];
            filled = center;
        }
    }
    // 右端收尾：窗口右缘被图界截断的中心点
    for center in (filled + 1).max(0)..n as i64 {
        let left = center - half;
        while let Some(&f) = dq.front() {
            if (f as i64) < left { dq.pop_front(); } else { break; }
        }
        out[center as usize] = src[*dq.front().unwrap()];
    }
    out
}

/// 椭圆核形态学（f64，可腐蚀/膨胀）：分解为逐行一维居中滑窗，O(r·n)。
fn ellipse_morph(src: &[f64], w: usize, h: usize, r: usize, dilate: bool) -> Vec<f64> {
    let ident = if dilate { f64::NEG_INFINITY } else { f64::INFINITY };
    let mut acc = vec![ident; w * h];
    for dy in -(r as i64)..=(r as i64) {
        let half = ellipse_half(r, dy);
        for y in 0..h {
            let sy = y as i64 + dy;
            if sy < 0 || sy >= h as i64 {
                continue;
            }
            let row = &src[sy as usize * w..(sy as usize + 1) * w];
            let ext = slide_center(row, half, dilate);
            let base = y * w;
            for x in 0..w {
                let v = ext[x];
                let slot = &mut acc[base + x];
                *slot = if dilate { slot.max(v) } else { slot.min(v) };
            }
        }
    }
    acc
}

/// 模板命中结果：笔画 mask + 匹配描述 + gap-score + 模板左上角位置。
pub struct TemplateHit {
    pub mask: GrayImage,
    pub info: String,
    pub score: f64,
    pub px: usize,
    pub py: usize,
}

/// 模板笔画 mask 匹配：按图片短边比例缩放模板（水印尺寸随短边等比），在右下角
/// 40px 余量窗口内做**顶帽 gap-score** 匹配（顶帽扣局部背景后笔画区均亮 - 间隙区
/// 均亮），命中返回 TemplateHit。mask 取连续 α 图 α>0.03 的污染像素 + ±1px（最小
/// 侵入）；分数不足返回 None 交给整框检测回退。
fn template_stroke_mask(image: &DynamicImage) -> Result<Option<TemplateHit>, String> {
    let (tpl, alpha, meta) = load_template()?;
    let rgb = image.to_rgb8();
    let (iw, ih) = (rgb.width() as usize, rgb.height() as usize);
    let scale = (iw.min(ih) as f64) / meta.ref_short_side;
    let tw = ((tpl.width() as f64) * scale) as u32;
    let th = ((tpl.height() as f64) * scale) as u32;
    if tw == 0 || th == 0 || tw >= rgb.width() || th >= rgb.height() {
        return Ok(None);
    }
    let t = image::imageops::resize(&tpl, tw, th, FilterType::Nearest);
    // 稀疏笔画点（相对模板左上角的偏移）与计数
    let mut pts: Vec<(usize, usize)> = Vec::new();
    for y in 0..th as usize {
        for x in 0..tw as usize {
            if t.get_pixel(x as u32, y as u32).0[0] > 127 {
                pts.push((x, y));
            }
        }
    }
    if pts.is_empty() {
        return Ok(None);
    }
    let n_in = pts.len() as f64;
    let n_out = (tw as usize * th as usize) as f64 - n_in;
    // 水印必贴右下角：模板左上角只可能在 (w-tw-40..=w-tw, h-th-40..=h-th)
    let max_py = ih - th as usize;
    let max_px = iw - tw as usize;
    let min_py = max_py.saturating_sub(40);
    let min_px = max_px.saturating_sub(40);

    // 顶帽（局部背景扣除）后再匹配：亮背景（纸面/花墙/雪/沙滩）会压低"笔画-间隙"
    // 绝对差，使低对比水印漏判并回退整框重绘；顶帽只保留局部高于背景的亮结构，
    // 与背景亮度无关。全图朴素形态学过慢，只算搜索窗 + 核半径范围，窗口内数值与
    // 全图计算一致。
    let r = TOPHAT_RADIUS;
    let rx0 = min_px.saturating_sub(r);
    let ry0 = min_py.saturating_sub(r);
    let rx1 = (max_px + tw as usize + r).min(iw);
    let ry1 = (max_py + th as usize + r).min(ih);
    let (rw, rh) = (rx1 - rx0, ry1 - ry0);
    if rw == 0 || rh == 0 {
        return Ok(None);
    }
    let mut g = vec![0f64; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let p = rgb.get_pixel((rx0 + x) as u32, (ry0 + y) as u32);
            g[y * rw + x] = p.0.iter().copied().max().unwrap_or(0) as f64;
        }
    }
    let opened = ellipse_morph(&ellipse_morph(&g, rw, rh, r, false), rw, rh, r, true);
    let tophat: Vec<f64> = g.iter().zip(opened.iter()).map(|(&o, &op)| o - op).collect();

    // 积分图（前缀和）求任意矩形和：S_all 与匹配窗口内的 S_out
    let iw1 = rw + 1;
    let mut integral = vec![0f64; iw1 * (rh + 1)];
    for y in 0..rh {
        let mut row_acc = 0f64;
        for x in 0..rw {
            row_acc += tophat[y * rw + x];
            integral[(y + 1) * iw1 + (x + 1)] = integral[y * iw1 + (x + 1)] + row_acc;
        }
    }
    let rect_sum = |x1: usize, y1: usize, x2: usize, y2: usize| -> f64 {
        integral[y2 * iw1 + x2] + integral[y1 * iw1 + x1] - integral[y1 * iw1 + x2]
            - integral[y2 * iw1 + x1]
    };
    let mut best = (f64::MIN, 0usize, 0usize);
    for py in min_py..=max_py {
        for px in min_px..=max_px {
            let (lx, ly) = (px - rx0, py - ry0);
            let mut s_in = 0f64;
            for &(dx, dy) in &pts {
                s_in += tophat[(ly + dy) * rw + lx + dx];
            }
            let s_all = rect_sum(lx, ly, lx + tw as usize, ly + th as usize);
            let gap = s_in / n_in - (s_all - s_in) / n_out;
            if gap > best.0 {
                best = (gap, px, py);
            }
        }
    }
    let (score, px, py) = best;
    if score < TEMPLATE_MIN_SCORE {
        return Ok(None);
    }
    // 连续 α mask（α>0.03 的真实污染像素，含抗锯齿带）+ ±1px 膨胀：只覆盖真正被
    // 水印污染的像素，不再靠大膨胀补抗锯齿（后者会多盖干净画面被模型重绘）。
    let a = image::imageops::resize(&alpha, tw, th, FilterType::Triangle);
    let mut bin = vec![false; (tw as usize) * (th as usize)];
    for y in 0..th as usize {
        for x in 0..tw as usize {
            if a.get_pixel(x as u32, y as u32).0[0] > TEMPLATE_ALPHA_THRESHOLD {
                bin[y * tw as usize + x] = true;
            }
        }
    }
    let dilate_1d = |src: &[bool], w: usize, h: usize, horizontal: bool| -> Vec<bool> {
        let (rx, ry) = if horizontal {
            (TEMPLATE_DILATE_W / 2, 0)
        } else {
            (0, TEMPLATE_DILATE_H / 2)
        };
        let mut dst = vec![false; src.len()];
        for y in 0..h {
            for x in 0..w {
                if src[y * w + x] {
                    dst[y * w + x] = true;
                    continue;
                }
                let lo = x.saturating_sub(rx);
                let hi = (x + rx).min(w - 1);
                let lo_y = y.saturating_sub(ry);
                let hi_y = (y + ry).min(h - 1);
                if horizontal {
                    for xx in lo..=hi {
                        if src[y * w + xx] {
                            dst[y * w + x] = true;
                            break;
                        }
                    }
                } else {
                    for yy in lo_y..=hi_y {
                        if src[yy * w + x] {
                            dst[y * w + x] = true;
                            break;
                        }
                    }
                }
            }
        }
        dst
    };
    let dilated = dilate_1d(&dilate_1d(&bin, tw as usize, th as usize, true), tw as usize, th as usize, false);
    let mut mask = GrayImage::from_pixel(rgb.width(), rgb.height(), image::Luma([0]));
    for y in 0..th as usize {
        for x in 0..tw as usize {
            if dilated[y * tw as usize + x] {
                mask.put_pixel((px + x) as u32, (py + y) as u32, image::Luma([255]));
            }
        }
    }
    let info = format!("template alpha mask at ({px},{py}) score {score:.1}");
    Ok(Some(TemplateHit { mask, info, score, px, py }))
}


pub type Logger<'a> = &'a dyn Fn(&str);

#[derive(Clone, Copy, Debug)]
pub struct MaskBox {
    pub x1: i64,
    pub y1: i64,
    pub x2: i64,
    pub y2: i64,
}

pub struct PipelineOptions {
    pub root: PathBuf,
    pub files: Vec<String>,
    pub keep_work: bool,
    pub mask_box: Option<MaskBox>,
    /// false（默认）：只保留贴右下角的检出框（防雪景/busy photo 误检毁图）；
    /// true：启用 OCR 文字检测（DBNet）处理任意位置的文字水印，
    /// 模型命中即完全独挑，缺失/未检出回退传统扫描（不做贴角过滤）。
    pub any_position: bool,
    /// false（默认）：结果另存到 root/watermark-cleaned/，原图不动；
    /// true：直接覆盖原图（旧模式，需备份 + 用户显式确认）。
    pub overwrite_original: bool,
    /// true（默认）：先尝试水印档案库（`tools/watermarks/` + 内置档案）做逐像素
    /// 解析逆解；命中即精确去除且周边零改动。false 完全跳过档案路径。
    pub use_profile: bool,
    /// false（默认）：结果级验证 FAIL（如 mask 外被改动）时拒绝落盘。
    /// true：跳过拒绝，强制写入（对应 CLI `--force`）。
    pub force: bool,
    /// true（默认）：模板命中后做豆包 stamp 逐像素解析逆解（还原真实背景）；
    /// false 只用生成式结果（对应 CLI `--no-inverse`）。
    pub inverse: bool,
    /// true（默认）：结果残留时膨胀 mask 隔离重跑一轮（对应 CLI `--no-retry`）。
    pub retry: bool,
    /// 实验性：手动框选（mask_box）时把框内做笔画精分割（顶帽局部对比 + 低饱和过滤 +
    /// 局部自适应阈值 + 行带约束），只重绘笔画而非整框；失败自动退回整框
    /// （对应 CLI `--refine`，App 侧默认勾选）。
    pub refine: bool,
    /// 「精确模式」：只用该 id 的水印档案定位（不做多档案择优），
    /// 定位成功即逐像素解析逆解；`None` 为自动匹配全部档案。
    pub forced_profile: Option<String>,
    /// 另存模式（`overwrite_original=false`）下的自定义输出目录。
    /// `None`（默认）＝沿用 `root/watermark-cleaned/`，行为与历史版本逐字节一致。
    pub output_dir_override: Option<PathBuf>,
}

pub const SUPPORTED_EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

pub fn is_supported_image(name: &str) -> bool {
    let lower = name.to_lowercase();
    SUPPORTED_EXTS.iter().any(|ext| lower.rsplit('.').next() == Some(*ext) && lower.contains('.'))
}

fn image_format_for(name: &str) -> image::ImageFormat {
    let lower = name.to_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        image::ImageFormat::Jpeg
    } else if lower.ends_with(".webp") {
        image::ImageFormat::WebP
    } else {
        image::ImageFormat::Png
    }
}

/// 按原文件格式保存结果（JPEG 质量 92，其余走默认编码器）。
fn save_result(image: DynamicImage, path: &Path) -> Result<(), String> {
    let format = image_format_for(path.file_name().unwrap_or_default().to_string_lossy().as_ref());
    if format == image::ImageFormat::Jpeg {
        let rgb = image.to_rgb8();
        let file = fs::File::create(path).map_err(|e| e.to_string())?;
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 92);
        rgb.write_with_encoder(encoder).map_err(|e| e.to_string())
    } else {
        image.save_with_format(path, format).map_err(|e| e.to_string())
    }
}

pub fn default_mask_box(width: u32, height: u32) -> (i64, i64, i64, i64) {
    if (width, height) == (2848, 1600) {
        return (width as i64 - 330, height as i64 - 118, width as i64 - 8, height as i64 - 8);
    }
    if (width, height) == (2278, 1280) {
        return (width as i64 - 275, height as i64 - 92, width as i64 - 7, height as i64 - 8);
    }
    if (width, height) == (2048, 2048) {
        return (width as i64 - 380, height as i64 - 125, width as i64 - 8, height as i64 - 40);
    }
    let scale = (width as f64 / 2848.0).min(height as f64 / 1600.0);
    let box_width = (330.0 * scale).max(220.0) as i64;
    let box_height = (118.0 * scale).max(78.0) as i64;
    (
        (width as i64 - box_width).max(0),
        (height as i64 - box_height).max(0),
        width as i64 - 8,
        height as i64 - 8,
    )
}

pub fn resolve_box(width: u32, height: u32, raw: Option<MaskBox>) -> Result<(i64, i64, i64, i64), String> {
    let (w, h) = (width as i64, height as i64);
    let (mut x1, mut y1, mut x2, mut y2) = match raw {
        None => return Ok(default_mask_box(width, height)),
        Some(box_) => (box_.x1, box_.y1, box_.x2, box_.y2),
    };
    if x1 < 0 {
        x1 += w;
    }
    if x2 <= 0 {
        x2 += w;
    }
    if y1 < 0 {
        y1 += h;
    }
    if y2 <= 0 {
        y2 += h;
    }
    x1 = x1.clamp(0, w);
    x2 = x2.clamp(0, w);
    y1 = y1.clamp(0, h);
    y2 = y2.clamp(0, h);
    if x1 >= x2 || y1 >= y2 {
        return Err(format!("invalid mask box after resolving: ({}, {}, {}, {})", x1, y1, x2, y2));
    }
    Ok((x1, y1, x2, y2))
}

/// 与 Python `detect_watermark_boxes` 对齐：两级检测。
/// 1) 全图扫纯白文字（≥248），用“文字性特征”过滤画面主体误检：
///    组件 bbox 内原始（膨胀前）白像素填充率 ≤0.6 且 x 投影列段数 ≥3
///    （实心白块如灯罩/瓷盘 fill 0.8~1.0、段数 1，文字水印 fill ~0.2、段数=字符数）；
/// 2) 右下角自适应阈值兜底（识别半透明/灰白粗体水印），与第 1 级合并去重。
pub fn detect_watermark_boxes(image: &DynamicImage) -> Vec<(i64, i64, i64, i64)> {
    let mut full = detect_full_white(image);
    let corner = detect_corner_faded(image);
    for box_ in corner {
        if !full.iter().any(|f| boxes_overlap(&box_, f)) {
            full.push(box_);
        }
    }
    full
}

fn boxes_overlap(a: &(i64, i64, i64, i64), b: &(i64, i64, i64, i64)) -> bool {
    !(a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1)
}

fn detect_full_white(image: &DynamicImage) -> Vec<(i64, i64, i64, i64)> {
    let rgb = image.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let rw = w;
    let rh = h;
    if rw == 0 || rh == 0 {
        return Vec::new();
    }
    let mut grid = vec![false; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let p = rgb.get_pixel(x as u32, y as u32);
            grid[y * rw + x] = p.0[0] >= 248 && p.0[1] >= 248 && p.0[2] >= 248;
        }
    }
    let dilated = dilate_rect_9x3_twice(&grid, rw, rh);

    let mut visited = vec![false; rw * rh];
    let mut boxes: Vec<((usize, usize, usize, usize), f64)> = Vec::new();
    let (wf, hf) = (w as f64, h as f64);
    for sy in 0..rh {
        for sx in 0..rw {
            let idx = sy * rw + sx;
            if !dilated[idx] || visited[idx] {
                continue;
            }
            let mut stack = vec![idx];
            visited[idx] = true;
            let (mut minx, mut miny, mut maxx, mut maxy) = (sx, sy, sx, sy);
            let mut area = 0usize;
            while let Some(cur) = stack.pop() {
                area += 1;
                let cx = cur % rw;
                let cy = cur / rw;
                minx = minx.min(cx);
                maxx = maxx.max(cx);
                miny = miny.min(cy);
                maxy = maxy.max(cy);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let nx = cx as i64 + dx;
                        let ny = cy as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= rw as i64 || ny >= rh as i64 {
                            continue;
                        }
                        let ni = ny as usize * rw + nx as usize;
                        if dilated[ni] && !visited[ni] {
                            visited[ni] = true;
                            stack.push(ni);
                        }
                    }
                }
            }
            let (cw, ch) = (maxx - minx + 1, maxy - miny + 1);
            if area < 1200 {
                continue;
            }
            // 文字性特征：原始（膨胀前）白像素在膨胀 bbox 内的填充率 + x 投影列段数
            let mut area_raw = 0usize;
            let mut col_counts = vec![0usize; cw];
            for y in miny..=maxy {
                for x in minx..=maxx {
                    if grid[y * rw + x] {
                        area_raw += 1;
                        col_counts[x - minx] += 1;
                    }
                }
            }
            let fill_raw = area_raw as f64 / (cw as f64 * ch as f64);
            let col_thr = (ch as f64 * 0.08).max(1.0);
            let mut segments = 0usize;
            let mut prev_on = false;
            for c in &col_counts {
                let on = *c as f64 >= col_thr;
                if on && !prev_on {
                    segments += 1;
                }
                prev_on = on;
            }
            if fill_raw > 0.6 || segments < 3 {
                continue;
            }
            let (cwf, chf) = (cw as f64, ch as f64);
            if !(hf * 0.012..=hf * 0.09).contains(&chf) || cw < ch {
                continue;
            }
            let ratio = cwf / chf;
            if !(2.0..=15.0).contains(&ratio) {
                continue;
            }
            let fill = area as f64 / (cwf * chf);
            if !(0.2..=0.95).contains(&fill) {
                continue;
            }
            let corner_dist = ((w - (maxx + 1)) + (h - (maxy + 1))) as f64;
            let score = area as f64 * (ratio / 6.0).min(1.0) / (1.0 + corner_dist / (wf * 0.1));
            boxes.push(((minx, miny, maxx + 1, maxy + 1), score));
        }
    }
    if boxes.is_empty() {
        return Vec::new();
    }
    // 只保留与最高分同量级的候选，避免低分噪声框误擦画面
    let top = boxes.iter().map(|(_, s)| *s).fold(0.0f64, f64::max);
    let threshold = top * 0.04;
    let pad = (h / 150).max(10) as i64;
    let (iw, ih) = (w as i64, h as i64);
    boxes
        .into_iter()
        .filter(|(_, s)| *s >= threshold)
        .map(|((bx1, by1, bx2, by2), _)| {
            (
                (bx1 as i64 - pad).max(0),
                (by1 as i64 - pad).max(0),
                (bx2 as i64 + pad).min(iw - 6),
                (by2 as i64 + pad).min(ih - 6),
            )
        })
        .collect()
}
/// 迭代合并重叠框：水印在不同阈值下切出的组件不完整，融合后覆盖完整水印。
fn fuse_boxes(boxes: Vec<(i64, i64, i64, i64)>) -> Vec<(i64, i64, i64, i64)> {
    let mut boxes = boxes;
    let mut changed = true;
    while changed {
        changed = false;
        let mut result = Vec::new();
        while let Some(mut cur) = boxes.pop() {
            let mut i = 0;
            while i < boxes.len() {
                let o = boxes[i];
                let overlaps = !(cur.2 <= o.0 || o.2 <= cur.0 || cur.3 <= o.1 || o.3 <= cur.1);
                if overlaps {
                    boxes.remove(i);
                    cur = (
                        cur.0.min(o.0),
                        cur.1.min(o.1),
                        cur.2.max(o.2),
                        cur.3.max(o.3),
                    );
                    changed = true;
                } else {
                    i += 1;
                }
            }
            result.push(cur);
        }
        boxes = result;
    }
    boxes
}

/// 形态学顶帽：原图减开运算（31x31 椭圆核），突出局部亮结构。
fn top_hat(gray: &[u8], rw: usize, rh: usize) -> Vec<u8> {
    let r = 15usize;
    let mut se = vec![false; (2 * r + 1) * (2 * r + 1)];
    for dy in 0..=(2 * r) {
        for dx in 0..=(2 * r) {
            let fx = (dx as f64 - r as f64) / (r as f64 + 0.5);
            let fy = (dy as f64 - r as f64) / (r as f64 + 0.5);
            if fx * fx + fy * fy <= 1.0 {
                se[dy * (2 * r + 1) + dx] = true;
            }
        }
    }
    let eroded = morph_min(gray, rw, rh, &se, r);
    morph_max(&eroded, rw, rh, &se, r)
        .iter()
        .zip(gray.iter())
        .map(|(&opened, &orig)| orig.saturating_sub(opened))
        .collect()
}

fn morph_min(gray: &[u8], rw: usize, rh: usize, se: &[bool], r: usize) -> Vec<u8> {
    let mut out = vec![255u8; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let mut m = 255u8;
            for dy in 0..=(2 * r) {
                let iy = y as i64 + dy as i64 - r as i64;
                if iy < 0 || iy >= rh as i64 {
                    continue;
                }
                for dx in 0..=(2 * r) {
                    if !se[dy * (2 * r + 1) + dx] {
                        continue;
                    }
                    let ix = x as i64 + dx as i64 - r as i64;
                    if ix < 0 || ix >= rw as i64 {
                        continue;
                    }
                    m = m.min(gray[iy as usize * rw + ix as usize]);
                }
            }
            out[y * rw + x] = m;
        }
    }
    out
}

fn morph_max(gray: &[u8], rw: usize, rh: usize, se: &[bool], r: usize) -> Vec<u8> {
    let mut out = vec![0u8; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let mut m = 0u8;
            for dy in 0..=(2 * r) {
                let iy = y as i64 + dy as i64 - r as i64;
                if iy < 0 || iy >= rh as i64 {
                    continue;
                }
                for dx in 0..=(2 * r) {
                    if !se[dy * (2 * r + 1) + dx] {
                        continue;
                    }
                    let ix = x as i64 + dx as i64 - r as i64;
                    if ix < 0 || ix >= rw as i64 {
                        continue;
                    }
                    m = m.max(gray[iy as usize * rw + ix as usize]);
                }
            }
            out[y * rw + x] = m;
        }
    }
    out
}

/// 字符行分析：从二值图中找“高度一致的字符序列”（水印是单行文字，
/// 字符高度统一；沙滩亮斑/花影粘连块高度杂乱或超高，不成行即排除）。
fn corner_text_boxes(
    grid: &[bool],
    rw: usize,
    rh: usize,
    h: i64,
    w: i64,
    x0: usize,
    y0: usize,
    pad: i64,
) -> Vec<(i64, i64, i64, i64)> {
    // 轻度膨胀：合并字符内笔画碎片，字符间距不会粘连
    let dilated = dilate_square5(grid, rw, rh);
    let mut visited = vec![false; rw * rh];
    let mut chars: Vec<(usize, usize, usize, usize)> = Vec::new();
    for sy in 0..rh {
        for sx in 0..rw {
            let idx = sy * rw + sx;
            if !dilated[idx] || visited[idx] {
                continue;
            }
            let mut stack = vec![idx];
            visited[idx] = true;
            let (mut minx, mut miny, mut maxx, mut maxy) = (sx, sy, sx, sy);
            let mut area = 0usize;
            while let Some(cur) = stack.pop() {
                area += 1;
                let cx = cur % rw;
                let cy = cur / rw;
                minx = minx.min(cx);
                maxx = maxx.max(cx);
                miny = miny.min(cy);
                maxy = maxy.max(cy);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let nx = cx as i64 + dx;
                        let ny = cy as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= rw as i64 || ny >= rh as i64 {
                            continue;
                        }
                        let ni = ny as usize * rw + nx as usize;
                        if dilated[ni] && !visited[ni] {
                            visited[ni] = true;
                            stack.push(ni);
                        }
                    }
                }
            }
            let (cw, ch) = (maxx - minx + 1, maxy - miny + 1);
            // 字符级组件：高度占图高 1.2%~4.5%（豆包水印 ~2.9%），宽高比合理
            let chf = ch as f64;
            if (chf < h as f64 * 0.012) || (chf > h as f64 * 0.045) {
                continue;
            }
            if (cw as f64) < chf * 0.25 || (cw as f64) > chf * 7.0 || area < 120 {
                continue;
            }
            chars.push((minx, miny, maxx + 1, maxy + 1));
        }
    }
    if chars.len() < 4 {
        return Vec::new();
    }
    // 按 y 中心聚类成行
    chars.sort_by_key(|b| b.1 + b.3);
    let mut rows: Vec<(Vec<(usize, usize, usize, usize)>, i64, i64)> = Vec::new();
    for c in chars {
        let cy = (c.1 + c.3) as f64 / 2.0;
        let ch = (c.3 - c.1) as i64;
        let mut placed = false;
        for row in rows.iter_mut() {
            let tol = (ch.max(row.2) as f64 * 0.7) as i64;
            if (cy as i64 - row.1).abs() < tol {
                row.0.push(c);
                let n = row.0.len() as f64;
                row.1 = (row.0.iter().map(|b| (b.1 + b.3) as f64 / 2.0).sum::<f64>() / n) as i64;
                row.2 = row.2.max(ch);
                placed = true;
                break;
            }
        }
        if !placed {
            rows.push((vec![c], cy as i64, ch));
        }
    }
    let mut boxes = Vec::new();
    for row in rows {
        if row.0.len() < 4 {
            continue;
        }
        let hs: Vec<i64> = row.0.iter().map(|b| (b.3 - b.1) as i64).collect();
        let hmin = *hs.iter().min().unwrap_or(&1);
        let hmax = *hs.iter().max().unwrap_or(&1);
        if hmax * 10 > hmin.max(1) * 18 {
            continue;
        }
        let x1 = row.0.iter().map(|b| b.0).min().unwrap();
        let y1 = row.0.iter().map(|b| b.1).min().unwrap();
        let x2 = row.0.iter().map(|b| b.2).max().unwrap();
        let y2 = row.0.iter().map(|b| b.3).max().unwrap();
        let (gx2, gy2) = ((x0 + x2) as i64, (y0 + y2) as i64);
        // 水印贴右下角：右缘距图右 <40px、底缘距图底 <40px
        if gx2 < w - 40 || gy2 < h - 40 {
            continue;
        }
        boxes.push((
            ((x0 + x1) as i64 - pad).max(0),
            ((y0 + y1) as i64 - pad).max(0),
            (gx2 + pad).min(w - 6),
            (gy2 + pad).min(h - 6),
        ));
    }
    boxes
}

fn detect_corner_faded(image: &DynamicImage) -> Vec<(i64, i64, i64, i64)> {
    let rgb = image.to_rgb8();
    let (w, h) = (rgb.width() as i64, rgb.height() as i64);
    let x0 = (w as f64 * 0.70) as usize;
    let y0 = (h as f64 * 0.88) as usize;
    let rw = w as usize - x0.min(w as usize);
    let rh = h as usize - y0.min(h as usize);
    if rw == 0 || rh == 0 {
        return Vec::new();
    }
    let mut gray = vec![0u8; rw * rh];
    let mut sat = vec![0u8; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let p = rgb.get_pixel((x0 + x) as u32, (y0 + y) as u32);
            let (mut mx, mut mn) = (p.0[0], p.0[0]);
            for c in p.0.iter().take(3) {
                mx = mx.max(*c);
                mn = mn.min(*c);
            }
            gray[y * rw + x] = mx;
            sat[y * rw + x] = mx.saturating_sub(mn);
        }
    }
    let pad = (h / 150).max(10) as i64;
    // 1) 多阈值扫描：全部阈值的字符行 + 行块双模式检出融合（高阈值只能切出暗水印
    //    最亮部分，低阈值才切出完整文字，如 2.png 水印亮度 150~162）
    let mut all_boxes = Vec::new();
    for threshold in [248i64, 240, 230, 220, 210, 200, 190, 180, 170, 160, 150] {
        let grid: Vec<bool> = gray.iter().map(|&v| v as i64 >= threshold).collect();
        all_boxes.extend(corner_text_boxes(&grid, rw, rh, h, w, x0, y0, pad));
        all_boxes.extend(corner_row_boxes(&grid, rw, rh, h, w, x0, y0, pad));
    }
    if !all_boxes.is_empty() {
        return fuse_boxes(all_boxes);
    }
    // 2) 顶帽兜底：突出局部亮结构，对光照不均鲁棒；
    //    再用低饱和过滤（灰白水印 RGB 均衡，彩色背景如红花绿叶高饱和）防止花斑并入。
    //    顶帽只配字符行模式：顶帽图里花影/墙面亮斑与水印粘连，行块模式会把大片
    //    画面罩进框整块重绘（6.png 花丛曾被行块大框毁图）——宁漏检不误修。
    let tophat = top_hat(&gray, rw, rh);
    for threshold in [60i64, 50, 40, 30] {
        let grid: Vec<bool> = (0..rw * rh)
            .map(|i| tophat[i] as i64 >= threshold && sat[i] <= 60)
            .collect();
        let boxes = corner_text_boxes(&grid, rw, rh, h, w, x0, y0, pad);
        if !boxes.is_empty() {
            return fuse_boxes(boxes);
        }
    }
    Vec::new()
}

/// 行块模式：9x3 膨胀两次直接合并字符成行（水印字符与背景亮斑粘连、
/// 字符级分离失败时——如雪景雪点——仍能定位整行）。防御：
/// 1) 行框高度上限 6% 图高（排除大面积粘连块，如 1.png 沙滩亮斑 12%）；
/// 2) 组件必须整体位于 corner 检测区内（排除从区外伸进来的画面内容）；
/// 3) 文字性验证 + 贴边约束同字符行模式。
fn corner_row_boxes(
    grid: &[bool],
    rw: usize,
    rh: usize,
    h: i64,
    w: i64,
    x0: usize,
    y0: usize,
    pad: i64,
) -> Vec<(i64, i64, i64, i64)> {
    let dilated = dilate_rect_9x3_twice(grid, rw, rh);
    let mut visited = vec![false; rw * rh];
    let mut boxes = Vec::new();
    for sy in 0..rh {
        for sx in 0..rw {
            let idx = sy * rw + sx;
            if !dilated[idx] || visited[idx] {
                continue;
            }
            let mut stack = vec![idx];
            visited[idx] = true;
            let (mut minx, mut miny, mut maxx, mut maxy) = (sx, sy, sx, sy);
            let mut area = 0usize;
            while let Some(cur) = stack.pop() {
                area += 1;
                let cx = cur % rw;
                let cy = cur / rw;
                minx = minx.min(cx);
                maxx = maxx.max(cx);
                miny = miny.min(cy);
                maxy = maxy.max(cy);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let nx = cx as i64 + dx;
                        let ny = cy as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= rw as i64 || ny >= rh as i64 {
                            continue;
                        }
                        let ni = ny as usize * rw + nx as usize;
                        if dilated[ni] && !visited[ni] {
                            visited[ni] = true;
                            stack.push(ni);
                        }
                    }
                }
            }
            let (cw, ch) = (maxx - minx + 1, maxy - miny + 1);
            let (gy1, gx2, gy2) = (
                (y0 + miny) as i64,
                (x0 + maxx + 1) as i64,
                (y0 + maxy + 1) as i64,
            );
            if area < 400 || cw < ch || cw / ch > 20 {
                continue;
            }
            let fill = area as f64 / (cw as f64 * ch as f64);
            if !(0.15..=0.95).contains(&fill) {
                continue;
            }
            if ch as f64 > h as f64 * 0.06 {
                continue;
            }
            let gx1 = (x0 + minx) as i64;
            if gx1 < x0 as i64 - 20 {
                continue;
            }
            if gx2 < w - 40 || gy2 < h - 40 {
                continue;
            }
            // 文字性验证：膨胀前白像素填充率 + x 投影列段数（区域局部坐标）
            let mut area_raw = 0usize;
            let mut col_counts = vec![0usize; cw];
            for y in miny..=maxy {
                for x in minx..=maxx {
                    if grid[y * rw + x] {
                        area_raw += 1;
                        col_counts[x - minx] += 1;
                    }
                }
            }
            let fill_raw = area_raw as f64 / (cw as f64 * ch as f64);
            let col_thr = (ch as f64 * 0.08).max(1.0);
            let mut segments = 0usize;
            let mut prev_on = false;
            for c in &col_counts {
                let on = *c as f64 >= col_thr;
                if on && !prev_on {
                    segments += 1;
                }
                prev_on = on;
            }
            if fill_raw > 0.6 || segments < 3 {
                continue;
            }
            boxes.push((
                (gx1 - pad).max(0),
                (gy1 - pad).max(0),
                (gx2 + pad).min(w - 6),
                (gy2 + pad).min(h - 6),
            ));
        }
    }
    boxes
}

/// 5x5 方形膨胀一次（可分离：水平半径 2 + 垂直半径 2），合并字符内笔画碎片。
fn dilate_square5(grid: &[bool], rw: usize, rh: usize) -> Vec<bool> {
    let mut horiz = vec![false; grid.len()];
    for y in 0..rh {
        for x in 0..rw {
            if grid[y * rw + x] {
                for nx in x.saturating_sub(2)..=(x + 2).min(rw - 1) {
                    horiz[y * rw + nx] = true;
                }
            }
        }
    }
    let mut out = vec![false; grid.len()];
    for y in 0..rh {
        for x in 0..rw {
            if horiz[y * rw + x] {
                for ny in y.saturating_sub(2)..=(y + 2).min(rh - 1) {
                    out[ny * rw + x] = true;
                }
            }
        }
    }
    out
}

fn dilate_rect_9x3_twice(grid: &[bool], rw: usize, rh: usize) -> Vec<bool> {
    let mut cur = grid.to_vec();
    for _ in 0..2 {
        let mut horiz = vec![false; cur.len()];
        for y in 0..rh {
            for x in 0..rw {
                if cur[y * rw + x] {
                    for nx in x.saturating_sub(4)..=(x + 4).min(rw - 1) {
                        horiz[y * rw + nx] = true;
                    }
                }
            }
        }
        let mut out = vec![false; cur.len()];
        for y in 0..rh {
            for x in 0..rw {
                if horiz[y * rw + x] {
                    for ny in y.saturating_sub(1)..=(y + 1).min(rh - 1) {
                        out[ny * rw + x] = true;
                    }
                }
            }
        }
        cur = out;
    }
    cur
}

fn numeric_key(name: &str) -> (u8, u64, String) {
    let lower = name.to_lowercase();
    let stem = SUPPORTED_EXTS
        .iter()
        .find_map(|ext| {
            let dot = format!(".{}", ext);
            lower.strip_suffix(&dot)
        })
        .unwrap_or(&lower);
    match stem.parse::<u64>() {
        Ok(number) => (0, number, String::new()),
        Err(_) => (1, 0, name.to_string()),
    }
}

pub fn target_names(root: &Path, files: &[String]) -> Result<Vec<String>, String> {
    let names: Vec<String> = if files.is_empty() {
        let mut entries: Vec<String> = fs::read_dir(root)
            .map_err(|e| e.to_string())?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| is_supported_image(name))
            .collect();
        entries.sort_by_key(|name| numeric_key(name));
        entries
    } else {
        for name in files {
            if !is_supported_image(name) {
                return Err(format!(
                    "unsupported image: {} (supported: {})",
                    name,
                    SUPPORTED_EXTS.join(", ")
                ));
            }
            if !root.join(name).exists() {
                return Err(format!("missing file: {}", name));
            }
        }
        files.to_vec()
    };
    if names.is_empty() {
        return Err(format!(
            "no image files (png/jpg/webp) found in: {}\nhint: pass a folder, e.g. clean-cli --root /path/to/images run",
            root.display()
        ));
    }
    Ok(names)
}

fn backup_dir(root: &Path) -> PathBuf {
    root.join("original-watermark-backup")
}

/// 结果输出目录：覆盖模式就是原图目录，另存模式是 watermark-cleaned/。
pub fn output_dir(options: &PipelineOptions) -> PathBuf {
    if options.overwrite_original {
        // 覆盖模式忽略自定义目录：否则会"以为替换了原图"却写到了别处
        return options.root.clone();
    }
    if let Some(dir) = &options.output_dir_override {
        return dir.clone();
    }
    options.root.join("watermark-cleaned")
}

fn work_dirs() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let work = crate::workdir();
    (
        work.join("source"),
        work.join("masks"),
        work.join("lama"),
        work.join("review"),
    )
}

fn ensure_dirs(root: &Path, with_backup: bool) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), String> {
    let (source, masks, lama, review) = work_dirs();
    fs::create_dir_all(&source).map_err(|e| e.to_string())?;
    fs::create_dir_all(&masks).map_err(|e| e.to_string())?;
    fs::create_dir_all(&lama).map_err(|e| e.to_string())?;
    fs::create_dir_all(&review).map_err(|e| e.to_string())?;
    if with_backup {
        fs::create_dir_all(backup_dir(root)).map_err(|e| e.to_string())?;
    }
    Ok((source, masks, lama, review))
}

fn load_image(path: &Path) -> Result<DynamicImage, String> {
    image::open(path).map_err(|e| format!("打开图片失败 {}: {}", path.display(), e))
}

fn review_box(mask_path: &Path, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let mut bbox: Option<(u32, u32, u32, u32)> = None;
    if mask_path.exists() {
        if let Ok(mask) = image::open(mask_path) {
            let gray = mask.to_luma8();
            let mut x1 = width;
            let mut y1 = height;
            let mut x2 = 0u32;
            let mut y2 = 0u32;
            for (x, y, value) in gray.enumerate_pixels() {
                if value.0[0] > 0 {
                    x1 = x1.min(x);
                    y1 = y1.min(y);
                    x2 = x2.max(x);
                    y2 = y2.max(y);
                }
            }
            if x2 >= x1 && y2 >= y1 {
                bbox = Some((x1, y1, x2 + 1, y2 + 1));
            }
        }
    }
    let (x1, y1, x2, y2) = bbox.unwrap_or((
        (width as i64 - 330).max(0) as u32,
        (height as i64 - 118).max(0) as u32,
        width - 8,
        height - 8,
    ));
    let margin_x = (80i64).max((x2 as i64 - x1 as i64) / 2) as u32;
    let margin_y = (50i64).max((y2 as i64 - y1 as i64) / 2) as u32;
    (
        x1.saturating_sub(margin_x),
        y1.saturating_sub(margin_y),
        (x2 + margin_x).min(width),
        (y2 + margin_y).min(height),
    )
}

fn font() -> FontArc {
    FontArc::try_from_slice(include_bytes!("../assets/DejaVuSans.ttf")).expect("embedded font")
}

fn review(input_dir: &Path, review_dir: &Path, output_name: &str, names: &[String]) -> Result<PathBuf, String> {
    fs::create_dir_all(review_dir).map_err(|e| e.to_string())?;
    let font = font();
    let pad = 16u32;
    let label_height = 28u32;
    let header_height = 34u32;
    let mut crops: Vec<RgbImage> = Vec::new();
    for name in names {
        let image = load_image(&input_dir.join(name))?.to_rgb8();
        let (width, height) = (image.width(), image.height());
        let (x1, y1, x2, y2) = review_box(&review_dir.join("..").join("masks").join(name), width, height);
        let crop_w = x2.saturating_sub(x1).max(1);
        let crop_h = y2.saturating_sub(y1).max(1);
        let crop = image::imageops::crop_imm(&image, x1, y1, crop_w, crop_h).to_image();
        let small = image::imageops::resize(
            &crop,
            (crop.width() / 2).max(1),
            (crop.height() / 2).max(1),
            FilterType::Lanczos3,
        );
        crops.push(small);
    }
    let sheet_width = crops.iter().map(|c| c.width()).max().unwrap_or(1) + pad * 2;
    let sheet_height =
        header_height + pad + names.iter().map(|_| label_height).sum::<u32>() + crops.iter().map(|c| c.height()).sum::<u32>()
            + pad * names.len() as u32;
    let mut sheet = RgbImage::from_pixel(sheet_width, sheet_height, image::Rgb([30, 30, 30]));
    draw_text_mut(&mut sheet, image::Rgb([240, 240, 240]), pad as i32, pad as i32, 20.0, &font, "corner review");
    let mut y = (pad + header_height) as i32;
    for (label, crop) in names.iter().zip(&crops) {
        draw_text_mut(&mut sheet, image::Rgb([240, 240, 240]), pad as i32, y, 16.0, &font, label);
        y += label_height as i32;
        image::imageops::overlay(&mut sheet, crop, pad as i64, y as i64);
        y += (crop.height() + pad) as i32;
    }
    let output = review_dir.join(output_name);
    sheet
        .save(&output)
        .map_err(|e| format!("保存复查图失败: {}", e))?;
    Ok(output)
}

fn save_png(image: DynamicImage, path: &Path) -> Result<(), String> {
    image.save_with_format(path, image::ImageFormat::Png).map_err(|e| e.to_string())
}

pub fn prepare(options: &PipelineOptions, names: &[String], log: Logger) -> Result<(), String> {
    let work = crate::workdir();
    if work.exists() {
        fs::remove_dir_all(&work).map_err(|e| e.to_string())?;
    }
    let (source, masks, _lama, review_dir) = ensure_dirs(&options.root, options.overwrite_original)?;
    let backup_root = backup_dir(&options.root);
    if options.refine && options.mask_box.is_none() {
        log("--refine only applies to a manual mask box (--mask-box); ignored");
    }
    for (index, name) in names.iter().enumerate() {
        log(&format!(
            "prepare {}/{}: {}",
            index + 1,
            names.len(),
            name
        ));
        let current = options.root.join(name);
        let origin = if options.overwrite_original {
            let backup = backup_root.join(name);
            if !backup.exists() {
                fs::copy(&current, &backup).map_err(|e| e.to_string())?;
            }
            backup
        } else {
            current.clone()
        };
        fs::copy(&origin, source.join(name)).map_err(|e| e.to_string())?;

        let image = load_image(&origin)?;
        let (width, height) = (image.width(), image.height());
        let mut tpl_mask: Option<GrayImage> = None;
        let boxes: Vec<(i64, i64, i64, i64)> = match options.mask_box {
            Some(box_) => vec![resolve_box(width, height, Some(box_))?],
            None => {
                // 档案库优先（对齐 Python）：命中即逐像素解析逆解，写 .wprof sidecar；
                // 未命中回落到豆包模板/整框检测，原有行为不变。
                if options.use_profile {
                    let matched = match &options.forced_profile {
                        Some(id) => {
                            let hit = crate::watermark_profiles::match_specific(&image, id);
                            if hit.is_none() {
                                log(&format!(
                                    "{}: forced profile {} not located (score below threshold), falling back to auto detect",
                                    name, id
                                ));
                            }
                            hit
                        }
                        None => crate::watermark_profiles::match_image(&image),
                    };
                    if let Some((profile, px, py, score, scale)) = matched {
                        if let Some(mask) = crate::watermark_profiles::mask_for(
                            &profile,
                            px,
                            py,
                            width as usize,
                            height as usize,
                            scale,
                        ) {
                            let sidecar = masks.join(format!("{}.wprof", name));
                            let info = serde_json::json!({
                                "profile": profile.id,
                                "px": px,
                                "py": py,
                                "scale": scale,
                            });
                            fs::write(&sidecar, info.to_string()).map_err(|e| e.to_string())?;
                            log(&format!(
                                "{}: profile {} matched at ({},{}) score {:.1} -> alpha mask + inverse",
                                name, profile.id, px, py, score
                            ));
                            save_png(DynamicImage::ImageLuma8(mask), &masks.join(name))?;
                            continue;
                        }
                    }
                }
                // 三级策略 ①：模板笔画 mask（复杂场景精确修复，见 template_stroke_mask）
                if let Some(hit) = template_stroke_mask(&image)? {
                    log(&format!("{}: {}", name, hit.info));
                    // 标记本轮走了豆包模板路径，供结果级验证做"模板残留"检查
                    let _ = fs::write(masks.join(format!("{}.tpl", name)), "");
                    tpl_mask = Some(hit.mask);
                }
                let mut detected: Vec<(i64, i64, i64, i64)> = if options.any_position {
                    // 任意位置模式：OCR 文字检测（DBNet）优先，命中即完全独挑——
                    // 传统扫描在照片上误检率高反而拖累；未检出回退传统扫描
                    match crate::dbnet::detect(&image)? {
                        db if !db.is_empty() => {
                            log(&format!("{}: OCR detected {} watermark box(es)", name, db.len()));
                            db
                        }
                        _ => {
                            let scanned = detect_watermark_boxes(&image);
                            if scanned.is_empty() {
                                log(&format!("{}: no watermark detected, skipped (ocr: no text)", name));
                                Vec::new()
                            } else {
                                log(&format!(
                                    "{}: auto-detected {} watermark box(es) (ocr fallback)",
                                    name,
                                    scanned.len()
                                ));
                                scanned
                            }
                        }
                    }
                } else if tpl_mask.is_some() {
                    // 模板命中即完成（默认模式只处理贴右下角的豆包水印）
                    save_png(DynamicImage::ImageLuma8(tpl_mask.take().unwrap()), &masks.join(name))?;
                    continue;
                } else {
                    // 三级策略 ②：整框检测回退
                    let mut scanned = detect_watermark_boxes(&image);
                    // 豆包水印必贴右下角：丢弃远离右下角的检出框，
                    // 否则雪景白点/白墙/栏杆等画面内容会被误检硬修（毁图）
                    let (pw, ph) = (image.width() as i64, image.height() as i64);
                    scanned.retain(|b| b.2 > pw - 40 && b.3 > ph - 40);
                    if scanned.is_empty() {
                        // 三级策略 ③：检测不到水印，写全空 mask 跳过修复，绝不用默认
                        // 规则硬修——对已无水印的图硬修会把真实画面重绘成模糊块
                        log(&format!("{}: no watermark detected, skipped", name));
                        Vec::new()
                    } else {
                        log(&format!("{}: auto-detected {} watermark box(es)", name, scanned.len()));
                        scanned
                    }
                };
                if let Some(tpl) = &tpl_mask {
                    // 任意位置模式下模板与其它位置检测叠加：丢弃与模板笔画重叠的
                    // 检出框（DBNet 也会检出右下角豆包水印，避免重复修复）
                    detected.retain(|&(x1, y1, x2, y2)| {
                        let mut overlap = false;
                        'outer: for y in y1.clamp(0, height as i64)..y2.clamp(0, height as i64) {
                            for x in x1.clamp(0, width as i64)..x2.clamp(0, width as i64) {
                                if tpl.get_pixel(x as u32, y as u32).0[0] > 0 {
                                    overlap = true;
                                    break 'outer;
                                }
                            }
                        }
                        if overlap {
                            log(&format!("{}: box ({},{},{},{}) overlaps template mask, skipped", name, x1, y1, x2, y2));
                        }
                        !overlap
                    });
                }
                if tpl_mask.is_some() && detected.is_empty() {
                    save_png(DynamicImage::ImageLuma8(tpl_mask.take().unwrap()), &masks.join(name))?;
                    continue;
                }
                detected
            }
        };
        let mut mask = GrayImage::from_pixel(width, height, image::Luma([0]));
        // 手动框选 + refine：框内笔画精分割（框大也不整块重绘），失败退回整框
        let mut refine_mask: Option<GrayImage> = None;
        if options.mask_box.is_some() && options.refine {
            for box_ in &boxes {
                if let Some(m) = refine_box_mask(&image, *box_) {
                    refine_mask = Some(match refine_mask {
                        Some(prev) => {
                            let mut merged = prev;
                            for (x, y, p) in m.enumerate_pixels() {
                                if p.0[0] > 0 {
                                    merged.put_pixel(x, y, image::Luma([255]));
                                }
                            }
                            merged
                        }
                        None => m,
                    });
                }
            }
            match &refine_mask {
                Some(m) => {
                    // 不写 .tpl（避免强制启用豆包模板残留检查误伤非豆包水印：
                    // 千问等水印可能碰巧拿高分）；只用 .refinebox 标记精分割来源，
                    // verify_paths 会按 scale≈1.0 + 原图得分自动决定是否查模板残留。
                    let box_lines: Vec<String> =
                        boxes.iter().map(|(a, b, c, d)| format!("{a},{b},{c},{d}")).collect();
                    let _ = fs::write(
                        masks.join(format!("{}.refinebox", name)),
                        box_lines.join("\n"),
                    );
                    log(&format!(
                        "{}: box-refined stroke mask ({}px from {} box(es))",
                        name,
                        m.iter().filter(|&&v| v > 0).count(),
                        boxes.len()
                    ))
                }
                None => log(&format!("{}: refine failed, fallback to full box mask", name)),
            }
        }
        if let Some(rm) = &refine_mask {
            for (x, y, p) in rm.enumerate_pixels() {
                if p.0[0] > 0 {
                    mask.put_pixel(x, y, image::Luma([255]));
                }
            }
        } else {
            for (x1, y1, x2, y2) in &boxes {
                for y in *y1..*y2 {
                    for x in *x1..*x2 {
                        mask.put_pixel(x as u32, y as u32, image::Luma([255]));
                    }
                }
            }
        }
        // 模板命中时把模板笔画也画进 mask（与检出框叠加修复）
        if let Some(tpl) = &tpl_mask {
            for (x, y, p) in tpl.enumerate_pixels() {
                if p.0[0] > 0 {
                    mask.put_pixel(x, y, image::Luma([255]));
                }
            }
        }
        save_png(DynamicImage::ImageLuma8(mask), &masks.join(name))?;
    }
    let output = review(&source, &review_dir, "source-corner-review.png", names)?;
    println!("{}", output.display());
    Ok(())
}

/// 取消标记：返回该错误的 run 会被上层识别为“用户取消”，不算失败。
pub const CANCELLED: &str = "\u{0}cancelled";

pub type ProgressFn<'a> = &'a dyn Fn(&str, usize, usize, &str);

/// 对命中档案的图应用逐像素解析逆解（在生成式结果之上）。
/// 返回 Ok(None) 表示档案声明不可逆解（如带暗描边的水印）。
fn apply_profile_inverse(
    obs: &DynamicImage,
    mat: &RgbImage,
    sidecar: &Path,
    log: Logger,
    name: &str,
) -> Result<Option<RgbImage>, String> {
    let text = fs::read_to_string(sidecar).map_err(|e| e.to_string())?;
    let info: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let id = info.get("profile").and_then(|v| v.as_str()).ok_or("sidecar missing profile id")?;
    let px = info.get("px").and_then(|v| v.as_u64()).ok_or("sidecar missing px")? as usize;
    let py = info.get("py").and_then(|v| v.as_u64()).ok_or("sidecar missing py")? as usize;
    let scale = info.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let profile = crate::watermark_profiles::load_profile(id)?;
    if profile.extra.get("inverse").and_then(|v| v.as_bool()) == Some(false) {
        log(&format!("{}: profile {} not invertible (outline), keep generative result", name, id));
        return Ok(None);
    }
    let obs_rgb = obs.to_rgb8();
    let (out, detail) =
        crate::watermark_profiles::inverse_image(&obs_rgb, mat, &profile, px, py, scale)?;
    log(&format!("{}: {}", name, detail));
    Ok(Some(out))
}

// 豆包 stamp 逆解门控（对齐 Python）：scale 只在 ≈1.0 标定；低纹理背景逆解只会
// 放大噪声；stamp 的残影阈值远严于档案默认（0.6）。
const STAMP_SCALE_TOL: f64 = 0.03;
const STAMP_TEXTURE_MIN: f64 = 9.0;
const STAMP_MAX_GHOST: f64 = 0.12;

/// 豆包 stamp 逐像素解析逆解（对齐 Python inverse_apply）：obs = α·C + (1−α)·bg，
/// 用完整水印模型（含暗描边）恢复**真实背景**，覆盖生成式结果。门控不通过/未命中
/// 返回 None（保持生成式结果）；写入限定在模板 mask ∩ α>0.03，保住"mask 外零改动"。
fn apply_stamp_inverse(
    obs: &DynamicImage,
    mat: &RgbImage,
    template_mask: &GrayImage,
    name: &str,
    log: Logger,
) -> Option<RgbImage> {
    let stamp = match crate::watermark_profiles::doubao_stamp() {
        Ok(p) => p,
        Err(e) => {
            log(&format!("{}: stamp unavailable ({}), keep generative result", name, e));
            return None;
        }
    };
    // 与 prepare 同一把尺子：gap-score 模板匹配给出位置
    let hit = match template_stroke_mask(obs) {
        Ok(Some(h)) => h,
        Ok(None) => {
            log(&format!("{}: stamp inverse skipped (template not matched), keep generative result", name));
            return None;
        }
        Err(e) => {
            log(&format!("{}: stamp inverse skipped ({}), keep generative result", name, e));
            return None;
        }
    };
    let rgb = obs.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let scale = (w.min(h) as f64) / stamp.ref_short_side;
    let params = crate::watermark_profiles::InverseParams {
        scale_tol: Some(STAMP_SCALE_TOL),
        texture_min: Some(STAMP_TEXTURE_MIN),
        ghost_max: Some(STAMP_MAX_GHOST),
        write_mask: Some(template_mask),
    };
    match crate::watermark_profiles::inverse_image_gated(
        &rgb, mat, &stamp, hit.px, hit.py, scale, &params,
    ) {
        Ok(Some((out, detail))) => {
            log(&format!("{}: stamp inverse {}", name, detail));
            Some(out)
        }
        Ok(None) => {
            log(&format!("{}: stamp inverse skipped (gating), keep generative result", name));
            None
        }
        Err(e) => {
            log(&format!("{}: stamp inverse skipped ({}), keep generative result", name, e));
            None
        }
    }
}

/// 单图修复：空 mask 透传 → 生成式修复 → 逆解（档案优先；否则豆包 stamp，默认开）。
fn inpaint_one(
    engine: &mut Lama,
    path: &Path,
    masks: &Path,
    lama_dir: &Path,
    inverse: bool,
    log: Logger,
) -> Result<(), String> {
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let image = load_image(path)?;
    let mask = load_image(&masks.join(&name))?.to_luma8();
    // 空 mask（未检测到水印）的图直接原样通过，不进模型
    if mask.iter().all(|&v| v == 0) {
        fs::copy(path, lama_dir.join(&name)).map_err(|e| e.to_string())?;
        log(&format!("{}: empty mask, passed through without inpainting", name));
        return Ok(());
    }
    log(&format!("inpainting {}...", name));
    let mut result = engine.inpaint_image(&image, &mask, log)?;
    // 逆解：命中档案（.wprof）走档案逆解；否则豆包模板命中（.tpl）或框选精分割
    // （.refinebox，内部仍靠模板自定位）走 stamp 逆解。
    // 都在生成式结果之上做逐像素解析还原真实背景，门控不过则保留生成式结果。
    let sidecar = masks.join(format!("{}.wprof", name));
    if sidecar.exists() {
        match apply_profile_inverse(&image, &result, &sidecar, log, &name) {
            Ok(Some(inv)) => result = inv,
            Ok(None) => {}
            Err(err) => log(&format!("{}: profile inverse skipped ({}), keep generative result", name, err)),
        }
    } else if inverse
        && (masks.join(format!("{}.tpl", name)).exists()
            || masks.join(format!("{}.refinebox", name)).exists())
    {
        if let Some(inv) = apply_stamp_inverse(&image, &result, &mask, &name, log) {
            result = inv;
        }
    }
    save_png(DynamicImage::ImageRgb8(result), &lama_dir.join(&name))?;
    log(&format!("done {}", name));
    Ok(())
}

pub fn inpaint(
    model_path: &Path,
    inverse: bool,
    log: Logger,
    progress: ProgressFn,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let (source, masks, lama_dir, _) = work_dirs();
    let mut entries: Vec<PathBuf> = fs::read_dir(&source)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_supported_image(&path.file_name().unwrap_or_default().to_string_lossy()))
        .collect();
    entries.sort_by_key(|path| numeric_key(&path.file_name().unwrap_or_default().to_string_lossy()));
    if entries.is_empty() {
        return Err("workdir has no prepared source images; run prepare first".into());
    }
    let total = entries.len();
    let mut engine = Lama::load(model_path)?;
    for (index, path) in entries.iter().enumerate() {
        if is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        progress("inpaint", index, total, &name);
        inpaint_one(&mut engine, path, &masks, &lama_dir, inverse, log)?;
        progress("inpaint", index + 1, total, &name);
    }
    Ok(())
}

/// 只对指定图重跑推理（残留重试用）——单独加载一次模型，不影响整批。
fn inpaint_subset(
    model_path: &Path,
    names: &[String],
    inverse: bool,
    log: Logger,
) -> Result<(), String> {
    let (source, masks, lama_dir, _) = work_dirs();
    let mut engine = Lama::load(model_path)?;
    for name in names {
        let path = source.join(name);
        if path.exists() {
            inpaint_one(&mut engine, &path, &masks, &lama_dir, inverse, log)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 结果级验证闭环（场景无关，对齐 Python verify_paths）：修复后客观自检 + 低置信度
// 告警。核心保证：① mask 外零改动——任何水印/场景都成立，被破坏即工程 bug；
// ② 模板残留——豆包水印修复后不应再匹配到模板字形；③ mask 面积占比（过度重绘）。
// 不依赖 ground truth，可泛化到未见场景：修完必须过检，否则拒绝落盘。
// ---------------------------------------------------------------------------

pub struct VerifyReport {
    pub mask_area: usize,
    pub mask_ratio: f64,
    pub passthrough: bool,
    pub inside_changed: usize,
    pub outside_changed: usize,
    pub outside_max: i32,
    pub orig_score: f64,
    pub res_score: f64,
    /// 结构化残留标记：修复后仍匹配豆包模板 → 供残留重试判定（区别于 mask 外改动，
    /// 那是工程 bug，重试无法修复）。
    pub residual: bool,
    pub verdict: &'static str,
    pub reasons: Vec<String>,
}

/// 对 (原图, 修复结果, mask) 三元组做客观验证；缺文件/尺寸不一致返回 None。
/// template_applied 为 true 时才做"豆包模板残留"检查（否则非豆包水印可能碰巧
/// 匹配模板而误报）。
pub fn verify_paths(
    orig_path: &Path,
    res_path: &Path,
    mask_path: &Path,
    template_applied: Option<bool>,
) -> Option<VerifyReport> {
    if !(orig_path.exists() && res_path.exists() && mask_path.exists()) {
        return None;
    }
    let orig_img = image::open(orig_path).ok()?;
    let res_img = image::open(res_path).ok()?;
    let orig = orig_img.to_rgb8();
    let res = res_img.to_rgb8();
    if orig.dimensions() != res.dimensions() {
        return None;
    }
    let (w, h) = orig.dimensions();
    let m = image::open(mask_path).ok()?.to_luma8();
    let m = if m.dimensions() != (w, h) {
        image::imageops::resize(&m, w, h, FilterType::Nearest)
    } else {
        m
    };
    let mut area = 0usize;
    let mut inside_changed = 0usize;
    let mut outside_changed = 0usize;
    let mut outside_max = 0i32;
    for y in 0..h {
        for x in 0..w {
            let a = orig.get_pixel(x, y).0;
            let b = res.get_pixel(x, y).0;
            let mut d = 0i32;
            for c in 0..3 {
                d = d.max((a[c] as i32 - b[c] as i32).abs());
            }
            if m.get_pixel(x, y).0[0] > 0 {
                area += 1;
                if d > 10 {
                    inside_changed += 1;
                }
            } else if d > 2 {
                outside_changed += 1;
                outside_max = outside_max.max(d);
            }
        }
    }
    let total = (w as usize) * (h as usize);
    let orig_score = template_stroke_mask(&orig_img)
        .ok()
        .flatten()
        .map(|h| h.score)
        .unwrap_or(0.0);
    let res_score = template_stroke_mask(&res_img)
        .ok()
        .flatten()
        .map(|h| h.score)
        .unwrap_or(0.0);
    let passthrough = area == 0;
    // template_applied 未显式给出时自动判定（对齐 Python）：原图命中模板且 scale≈1.0
    // 才做模板残留检查——否则非豆包水印（如千问）碰巧高分会被误判成残留。
    let template_applied = template_applied.unwrap_or_else(|| {
        let ref_short = load_template()
            .map(|(_, _, m)| m.ref_short_side)
            .unwrap_or(1600.0);
        let scale = (w.min(h) as f64) / ref_short;
        orig_score >= TEMPLATE_MIN_SCORE && (scale - 1.0).abs() <= STAMP_SCALE_TOL
    });
    let residual =
        template_applied && orig_score >= TEMPLATE_MIN_SCORE && res_score >= TEMPLATE_MIN_SCORE;
    let mask_ratio = if total > 0 { area as f64 / total as f64 } else { 0.0 };
    let mut reasons: Vec<String> = Vec::new();
    let mut verdict: &'static str = "PASS";
    if outside_changed > 0 {
        verdict = "FAIL";
        reasons.push(format!(
            "mask 外有 {} px 被改动 (max {})",
            outside_changed, outside_max
        ));
    }
    if template_applied && orig_score >= TEMPLATE_MIN_SCORE && res_score >= TEMPLATE_MIN_SCORE {
        verdict = "FAIL";
        reasons.push(format!(
            "修复后仍匹配豆包模板 (score {:.1} >= {:.0})，疑有残留",
            res_score, TEMPLATE_MIN_SCORE
        ));
    } else if template_applied
        && orig_score >= TEMPLATE_MIN_SCORE
        && res_score >= TEMPLATE_MIN_SCORE * 0.6
    {
        if verdict != "FAIL" {
            verdict = "WARN";
        }
        reasons.push(format!(
            "修复后模板分数偏高 ({:.1})，可能有残留，请放大复查",
            res_score
        ));
    }
    if !passthrough && mask_ratio > 0.08 {
        if verdict == "PASS" {
            verdict = "WARN";
        }
        reasons.push(format!(
            "mask 占图 {:.1}% (> 8%)，可能过度重绘",
            mask_ratio * 100.0
        ));
    }
    Some(VerifyReport {
        mask_area: area,
        mask_ratio,
        passthrough,
        inside_changed,
        outside_changed,
        outside_max,
        orig_score,
        res_score,
        residual,
        verdict,
        reasons,
    })
}

pub fn format_verify(report: &VerifyReport) -> String {
    let head = if report.passthrough {
        "no mask (passthrough)".to_string()
    } else {
        format!(
            "mask {}px ({:.1}%), inside changed {}",
            report.mask_area,
            report.mask_ratio * 100.0,
            report.inside_changed
        )
    };
    format!(
        "[{}] {}, outside changed {} (max {}), tmpl {:.1}->{:.1}",
        report.verdict,
        head,
        report.outside_changed,
        report.outside_max,
        report.orig_score,
        report.res_score
    )
}

/// 按工作目录约定验证本轮修复结果：原图取备份（无则 source），结果取 lama。
/// `.tpl` sidecar 存在说明本轮确实命中豆包模板，则强制启用模板残留检查；
/// 框选精分割只写 `.refinebox`（不强制），交由 verify_paths 按 scale/分数自动判定。
fn verify_repaired(name: &str, root: &Path) -> Option<VerifyReport> {
    let (source, masks, lama, _) = work_dirs();
    let backup = backup_dir(root).join(name);
    let orig = if backup.exists() { backup } else { source.join(name) };
    let applied = masks.join(format!("{}.tpl", name)).exists().then_some(true);
    verify_paths(&orig, &lama.join(name), &masks.join(name), applied)
}

/// 打印验证结果（仅报告，不拒绝）。返回是否有 FAIL。
fn report_verify(names: &[String], root: &Path, log: Logger) -> bool {
    let mut failed = false;
    for name in names {
        if let Some(report) = verify_repaired(name, root) {
            log(&format!("verify {}: {}", name, format_verify(&report)));
            for reason in &report.reasons {
                log(&format!("verify {}: {}", name, reason));
            }
            if report.verdict == "FAIL" {
                failed = true;
            }
        }
    }
    failed
}

// ---------------------------------------------------------------------------
// 残留自动重试（对齐 Python _residual_retry）：结果残留来自 mask 盖不住（对齐/
// 抗锯齿误差），把 mask 膨胀一级隔离重跑一轮；mask 外改动属工程 bug，不重试。
// ---------------------------------------------------------------------------

const RETRY_DILATE: (usize, usize) = (5, 5);

/// 灰度图矩形核（kw x kh）膨胀：横向 + 纵向两次一维最大。
fn dilate_gray_rect(src: &GrayImage, kw: usize, kh: usize) -> GrayImage {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let rx = kw / 2;
    let ry = kh / 2;
    let px: Vec<u8> = src.pixels().map(|p| p.0[0]).collect();
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(rx);
            let hi = (x + rx).min(w - 1);
            let mut m = 0u8;
            for xx in lo..=hi {
                m = m.max(px[y * w + xx]);
            }
            tmp[y * w + x] = m;
        }
    }
    let mut out = GrayImage::new(src.width(), src.height());
    for y in 0..h {
        for x in 0..w {
            let lo = y.saturating_sub(ry);
            let hi = (y + ry).min(h - 1);
            let mut m = 0u8;
            for yy in lo..=hi {
                m = m.max(tmp[yy * w + x]);
            }
            out.put_pixel(x as u32, y as u32, image::Luma([m]));
        }
    }
    out
}

/// 线性插值分位（对齐 numpy.percentile）。
fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = (p / 100.0) * (v.len() as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        v[lo]
    } else {
        v[lo] + (v[hi] - v[lo]) * (rank - lo as f64)
    }
}

/// BORDER_REFLECT_101 索引映射（gfedcb|abcdefgh|gfedcba，边界像素不重复）。
fn reflect101(i: i64, n: i64) -> i64 {
    if n <= 1 {
        return 0;
    }
    let mut i = i;
    loop {
        if i < 0 {
            i = -i;
        } else if i >= n {
            i = 2 * (n - 1) - i;
        } else {
            return i;
        }
    }
}

/// 盒滤波的局部均值与标准差（96x96 窗口，对齐 cv2.boxFilter 默认 BORDER_REFLECT_101；
/// 偶数核锚点在 ksize/2，窗口为 [x-48, x+47] 共 96 项，面积恒为 win*win）。
fn box_mean_std(src: &[f64], w: usize, h: usize, win: usize) -> (Vec<f64>, Vec<f64>) {
    let lo = (win / 2) as i64;
    let hi = (win - win / 2 - 1) as i64;
    // 水平方向滑窗和（reflect-101），再做垂直方向
    let mut hs = vec![0f64; w * h];
    let mut hsq = vec![0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut s, mut sq) = (0f64, 0f64);
            for k in -lo..=hi {
                let xx = reflect101(x as i64 + k, w as i64) as usize;
                let v = src[y * w + xx];
                s += v;
                sq += v * v;
            }
            hs[y * w + x] = s;
            hsq[y * w + x] = sq;
        }
    }
    let area = ((lo + hi + 1) * (lo + hi + 1)) as f64;
    let mut mean = vec![0f64; w * h];
    let mut std = vec![0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut s, mut sq) = (0f64, 0f64);
            for k in -lo..=hi {
                let yy = reflect101(y as i64 + k, h as i64) as usize;
                s += hs[yy * w + x];
                sq += hsq[yy * w + x];
            }
            let m = s / area;
            let m2 = sq / area;
            mean[y * w + x] = m;
            std[y * w + x] = (m2 - m * m).max(0.0).sqrt();
        }
    }
    (mean, std)
}

/// 8 连通标记：返回 (labels 1-based, 各连通域面积, 各连通域 y 质心)。
fn label_components(active: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<usize>, Vec<f64>) {
    let mut labels = vec![0u32; w * h];
    let mut areas: Vec<usize> = Vec::new();
    let mut cy: Vec<f64> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut cur = 0u32;
    for sy in 0..h {
        for sx in 0..w {
            let idx = sy * w + sx;
            if !active[idx] || labels[idx] != 0 {
                continue;
            }
            cur += 1;
            labels[idx] = cur;
            stack.push(idx);
            let (mut area, mut ysum) = (0usize, 0f64);
            while let Some(c) = stack.pop() {
                area += 1;
                let (cxx, cyy) = (c % w, c / w);
                ysum += cyy as f64;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let nx = cxx as i64 + dx;
                        let ny = cyy as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let ni = ny as usize * w + nx as usize;
                        if active[ni] && labels[ni] == 0 {
                            labels[ni] = cur;
                            stack.push(ni);
                        }
                    }
                }
            }
            areas.push(area);
            cy.push(ysum / area as f64);
        }
    }
    (labels, areas, cy)
}

/// 框内笔画精分割（对齐 Python refine_box_mask）：把任意来源的候选框（检测框/手动框）
/// 缩小到笔画级 mask——框内顶帽局部对比度分割（亮/暗水印自适应）+ 低饱和过滤 +
/// 局部自适应阈值（96x96 的 mean+1.8σ）+ 连通域面积≥9 与"单行文字行带"约束。
/// 失败（空 / 几乎填满整框 / 过度碎化）返回 None，调用方退回整框。
pub fn refine_box_mask(image: &DynamicImage, box_: (i64, i64, i64, i64)) -> Option<GrayImage> {
    const PAD: i64 = 16;
    const WIN: usize = 96;
    const LOW_SAT: i32 = 60;
    let rgb = image.to_rgb8();
    let (iw, ih) = (rgb.width() as i64, rgb.height() as i64);
    let (x1, y1, x2, y2) = box_;
    let rx1 = (x1 - PAD).max(0);
    let ry1 = (y1 - PAD).max(0);
    let rx2 = (x2 + PAD).min(iw);
    let ry2 = (y2 + PAD).min(ih);
    if rx2 <= rx1 || ry2 <= ry1 {
        return None;
    }
    let (rw, rh) = ((rx2 - rx1) as usize, (ry2 - ry1) as usize);
    if rw < 8 || rh < 8 {
        return None;
    }
    let mut gray = vec![0f64; rw * rh];
    let mut low_sat = vec![false; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let p = rgb
                .get_pixel((rx1 + x as i64) as u32, (ry1 + y as i64) as u32)
                .0;
            let mx = *p.iter().max().unwrap() as i32;
            let mn = *p.iter().min().unwrap() as i32;
            gray[y * rw + x] = mx as f64;
            low_sat[y * rw + x] = (mx - mn) <= LOW_SAT;
        }
    }
    // 31x31 椭圆开运算估局部背景；亮/暗水印自适应：哪侧局部对比度响应强用哪侧
    let opened = ellipse_morph(&ellipse_morph(&gray, rw, rh, 15, false), rw, rh, 15, true);
    let bright: Vec<f64> = gray.iter().zip(opened.iter()).map(|(&g, &o)| g - o).collect();
    let dark: Vec<f64> = gray.iter().zip(opened.iter()).map(|(&g, &o)| o - g).collect();
    let abs99 = |v: &[f64]| percentile(&v.iter().map(|x| x.abs()).collect::<Vec<f64>>(), 99.0);
    let diff = if abs99(&bright) >= abs99(&dark) { bright } else { dark };
    // 局部自适应阈值：背景非均匀（同时含亮墙与暗花丛）时全局阈值顾此失彼
    let (mean, std) = box_mean_std(&diff, rw, rh, WIN);
    let mut active = vec![false; rw * rh];
    for i in 0..rw * rh {
        let th = (mean[i] + 1.8 * std[i]).max(10.0);
        active[i] = diff[i] >= th && low_sat[i];
    }
    let (labels, areas, cy) = label_components(&active, rw, rh);
    let keep: Vec<usize> = (1..=areas.len()).filter(|&i| areas[i - 1] >= 9).collect();
    if keep.is_empty() {
        return None;
    }
    // 行带约束：水印是单行文字，组件 y 质心应聚在一条行带；远离的判为背景纹理误检
    let ys: Vec<f64> = keep.iter().map(|&i| cy[i - 1]).collect();
    let band_center = percentile(&ys, 50.0);
    let dev: Vec<f64> = ys.iter().map(|y| (y - band_center).abs()).collect();
    let band_half = (percentile(&dev, 80.0) * 1.5).max(15.0);
    let mut keep_flag = vec![false; areas.len() + 1];
    let mut kept = 0usize;
    for &i in &keep {
        if (cy[i - 1] - band_center).abs() <= band_half {
            keep_flag[i] = true;
            kept += 1;
        }
    }
    let mut cleaned = vec![false; rw * rh];
    let mut stroke_px = 0usize;
    for k in 0..rw * rh {
        let l = labels[k] as usize;
        if l > 0 && keep_flag[l] {
            cleaned[k] = true;
            stroke_px += 1;
        }
    }
    let box_area = ((x2 - x1) * (y2 - y1)) as f64;
    // 失败判定：几乎填满整框（背景与水印不可分）或过度碎化（背景细节误检）
    if stroke_px == 0 || stroke_px as f64 > box_area * 0.6 || kept > 300 {
        return None;
    }
    let mut strokes = GrayImage::new(rw as u32, rh as u32);
    for k in 0..rw * rh {
        if cleaned[k] {
            strokes.put_pixel((k % rw) as u32, (k / rw) as u32, image::Luma([255]));
        }
    }
    // Rust 内核固定 LaMa → 用 Python 的 REFINE_DILATE_LAMA(19,11) 连接笔画碎片
    let strokes = dilate_gray_rect(&strokes, 19, 11);
    let mut full = GrayImage::from_pixel(rgb.width(), rgb.height(), image::Luma([0]));
    for y in 0..rh {
        for x in 0..rw {
            if strokes.get_pixel(x as u32, y as u32).0[0] > 0 {
                full.put_pixel((rx1 + x as i64) as u32, (ry1 + y as i64) as u32, image::Luma([255]));
            }
        }
    }
    if full.iter().all(|&v| v == 0) {
        return None;
    }
    Some(full)
}

/// 对残留图扩 mask 并重跑复验（最多 1 轮）；仍残留则打印 FAIL（交给 finalize 拒绝落盘）。
pub fn residual_retry(
    options: &PipelineOptions,
    names: &[String],
    model_path: &Path,
    log: Logger,
) -> Result<(), String> {
    let masks = work_dirs().1;
    let candidates: Vec<String> = names
        .iter()
        .filter(|name| {
            verify_repaired(name, &options.root)
                .map(|r| r.residual)
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    log(&format!(
        "verify: watermark residual detected in {} — expanding mask and retrying once (max 1 round)",
        candidates.join(", ")
    ));
    for name in &candidates {
        let mask_path = masks.join(name);
        let m = load_image(&mask_path)?.to_luma8();
        // 框选精分割留残留 → 退化为用户框选的整框（旧行为，保证必然去除）；否则膨胀一级。
        let refined_box = masks.join(format!("{}.refinebox", name));
        let grown = if let Ok(text) = fs::read_to_string(&refined_box) {
            let mut full = GrayImage::from_pixel(m.width(), m.height(), image::Luma([0]));
            let mut n = 0usize;
            for line in text.lines() {
                let parts: Vec<i64> = line.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                if parts.len() != 4 {
                    continue;
                }
                let (x1, y1, x2, y2) = (parts[0], parts[1], parts[2], parts[3]);
                for y in y1.max(0)..y2.min(m.height() as i64) {
                    for x in x1.max(0)..x2.min(m.width() as i64) {
                        full.put_pixel(x as u32, y as u32, image::Luma([255]));
                    }
                }
                n += 1;
            }
            let added = full.iter().zip(m.iter()).filter(|(a, b)| a > b).count();
            log(&format!(
                "{}: refine left residual -> fallback to full box mask ({} box(es), +{}px)",
                name, n, added
            ));
            full
        } else {
            let grown = dilate_gray_rect(&m, RETRY_DILATE.0, RETRY_DILATE.1);
            let added = grown.iter().zip(m.iter()).filter(|(a, b)| a > b).count();
            log(&format!(
                "{}: retry mask +{}px (dilate {}x{})",
                name, added, RETRY_DILATE.0, RETRY_DILATE.1
            ));
            grown
        };
        save_png(DynamicImage::ImageLuma8(grown), &mask_path)?;
    }
    inpaint_subset(model_path, &candidates, options.inverse, log)?;
    for name in &candidates {
        if let Some(report) = verify_repaired(name, &options.root) {
            log(&format!("verify {}: {}", name, format_verify(&report)));
            for reason in &report.reasons {
                log(&format!("verify {}: {}", name, reason));
            }
            if report.residual {
                log(&format!(
                    "{}: STILL residual after retry — finalize will refuse unless --force is used",
                    name
                ));
            }
        }
    }
    Ok(())
}

pub fn review_lama(names: &[String], root: &Path) -> Result<PathBuf, String> {
    let (_, _, lama_dir, review_dir) = work_dirs();
    report_verify(names, root, &|line| println!("{}", line));
    let output = review(&lama_dir, &review_dir, "lama-corner-review.png", names)?;
    println!("{}", output.display());
    Ok(output)
}

/// 把修复结果按原格式写入输出目录（覆盖模式=原图位置，另存模式=watermark-cleaned/）。
pub fn finalize_outputs(options: &PipelineOptions, names: &[String], log: Logger) -> Result<PathBuf, String> {
    let (_, _, lama_dir, review_dir) = work_dirs();
    let dest_dir = output_dir(options);
    if !options.overwrite_original {
        fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    }
    // 验证闭环：落盘前客观自检，FAIL 拒绝写入（--force 强制），WARN 提示
    if !options.force {
        let mut failures: Vec<&String> = Vec::new();
        for name in names {
            if let Some(report) = verify_repaired(name, &options.root) {
                log(&format!("verify {}: {}", name, format_verify(&report)));
                for reason in &report.reasons {
                    log(&format!("verify {}: {}", name, reason));
                }
                if report.verdict == "FAIL" {
                    failures.push(name);
                }
            }
        }
        if !failures.is_empty() {
            let list = failures
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "verification FAILED for {list} — refusing to write output (use --force to override, or review manually)"
            ));
        }
    }
    for name in names {
        let result = load_image(&lama_dir.join(name))?;
        save_result(result, &dest_dir.join(name))?;
    }
    if options.overwrite_original {
        log("outputs written in place (originals overwritten)");
    } else {
        log(&format!("outputs saved to: {}", dest_dir.display()));
    }
    let output = review(&dest_dir, &review_dir, "overwritten-corner-review.png", names)?;
    println!("{}", output.display());
    Ok(output)
}

/// 兼容旧 CLI 命令名：等价于 finalize_outputs。
pub fn overwrite_review(options: &PipelineOptions, names: &[String]) -> Result<PathBuf, String> {
    finalize_outputs(options, names, &|_| {})
}

pub fn cleanup(options: &PipelineOptions, names: &[String]) -> Result<(), String> {
    let backup = backup_dir(&options.root);
    for name in names {
        let backup_file = backup.join(name);
        if backup_file.exists() {
            fs::remove_file(backup_file).map_err(|e| e.to_string())?;
        }
    }
    if backup.exists() {
        let empty = fs::read_dir(&backup).map_err(|e| e.to_string())?.next().is_none();
        if empty {
            fs::remove_dir(&backup).map_err(|e| e.to_string())?;
        }
    }
    let work = crate::workdir();
    if work.exists() {
        fs::remove_dir_all(&work).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 只清临时工作目录，保留 original-watermark-backup/（覆盖模式要留作"撤销"依据）。
pub fn cleanup_work_only() -> Result<(), String> {
    let work = crate::workdir();
    if work.exists() {
        fs::remove_dir_all(&work).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 把 `original-watermark-backup/` 里的原图恢复回 `root`，成功后删除备份目录。
/// 返回恢复的张数；没有备份时报错（前端据此提示"无可恢复内容"）。
pub fn restore_backup(root: &Path) -> Result<usize, String> {
    let backup = backup_dir(root);
    if !backup.is_dir() {
        return Err("no backup to restore".to_string());
    }
    let mut restored = 0usize;
    for entry in fs::read_dir(&backup).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        fs::copy(&path, root.join(entry.file_name())).map_err(|e| e.to_string())?;
        restored += 1;
    }
    if restored > 0 {
        fs::remove_dir_all(&backup).map_err(|e| e.to_string())?;
    }
    Ok(restored)
}

pub fn cleanup_preserved() -> Result<(), String> {
    let preserved = preserved_review_dir();
    if preserved.exists() {
        fs::remove_dir_all(&preserved).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn preserved_review_dir() -> PathBuf {
    crate::workdir()
        .parent()
        .map(|parent| parent.join("watermark-cleaner-review"))
        .unwrap_or_else(|| std::env::temp_dir().join("watermark-cleaner-review"))
}

fn preserve_reviews(source: &Path, candidate: &Path, final_: &Path) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let dest_dir = preserved_review_dir();
    fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let copy_one = |src: &Path| -> Result<PathBuf, String> {
        let dest = dest_dir.join(src.file_name().ok_or("bad review path")?);
        fs::copy(src, &dest).map_err(|e| e.to_string())?;
        Ok(dest)
    };
    let s = copy_one(source)?;
    let c = copy_one(candidate)?;
    let f = copy_one(final_)?;
    Ok((s, c, f))
}

pub struct RunSummary {
    pub source_review: PathBuf,
    pub candidate_review: PathBuf,
    pub final_review: PathBuf,
    pub kept_work: bool,
    pub cancelled: bool,
    pub processed: usize,
    pub output_dir: PathBuf,
    /// 落盘后的结果文件绝对路径（供移动端「保存到相册/分享」直接使用）。
    pub outputs: Vec<PathBuf>,
}

pub fn run(
    options: &PipelineOptions,
    model_path: &Path,
    log: Logger,
    progress: ProgressFn,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<RunSummary, String> {
    let names = target_names(&options.root, &options.files)?;
    log(&format!("processing {} file(s)", names.len()));
    if is_cancelled() {
        return Err(CANCELLED.to_string());
    }
    prepare(options, &names, log)?;
    let (_, _, _, review_dir) = work_dirs();
    let source_review = review_dir.join("source-corner-review.png");
    inpaint(model_path, options.inverse, log, progress, is_cancelled)?;
    if options.retry && !is_cancelled() {
        residual_retry(options, &names, model_path, log)?;
    }
    let candidate_review = review_lama(&names, &options.root)?;
    let final_review = finalize_outputs(options, &names, log)?;
    let output_dir_path = output_dir(options);
    let outputs: Vec<PathBuf> = names.iter().map(|n| output_dir_path.join(n)).collect();
    if options.keep_work {
        return Ok(RunSummary {
            source_review,
            candidate_review,
            final_review,
            kept_work: true,
            cancelled: false,
            processed: names.len(),
            output_dir: output_dir_path,
            outputs,
        });
    }
    let (source_review, candidate_review, final_review) =
        preserve_reviews(&source_review, &candidate_review, &final_review)?;
    if options.overwrite_original {
        // 覆盖模式保留 original-watermark-backup/：让"替换原图"可撤销（前端可一键恢复）。
        // prepare 在备份存在时以它为 origin，因此重复覆盖同一文件夹仍基于最初原图，结果幂等。
        cleanup_work_only()?;
        log(&format!(
            "kept backup for undo: {}",
            backup_dir(&options.root).display()
        ));
    } else {
        cleanup(options, &names)?;
        log(&format!(
            "cleaned: {} and {}",
            backup_dir(&options.root).display(),
            crate::workdir().display()
        ));
    }
    Ok(RunSummary {
        source_review,
        candidate_review,
        final_review,
        kept_work: false,
        cancelled: false,
        processed: names.len(),
        output_dir: output_dir_path,
        outputs,
    })
}

pub const WINDOW_SIZE: u32 = WINDOW;

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dwm-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 生成合成水印图：非纯白背景 + 右下角白色文字水印
    fn make_watermark_image(path: &Path, width: u32, height: u32, text: &str) -> (i64, i64, i64, i64) {
        let mut img = RgbImage::from_pixel(width, height, Rgb([240, 240, 233]));
        // 一些背景纹理
        for y in (0..height).step_by(97) {
            for x in 0..width {
                img.put_pixel(x, y, Rgb([230, 232, 224]));
            }
        }
        let scale = width as f32 / 2848.0;
        let size = (62.0 * scale).max(24.0) as f32;
        let font = font();
        let glyph_width = size * 0.62 * text.chars().count() as f32;
        let x1 = width as i64 - glyph_width as i64 - 12;
        let y1 = height as i64 - size as i64 - 60;
        draw_text_mut(
            &mut img,
            Rgb([150, 150, 150]),
            x1 as i32 - 2,
            y1 as i32 - 2,
            size,
            &font,
            text,
        );
        draw_text_mut(&mut img, Rgb([255, 255, 255]), x1 as i32, y1 as i32, size, &font, text);
        save_png(DynamicImage::ImageRgb8(img), path).unwrap();
        (x1, y1, x1 + glyph_width as i64, y1 + size as i64)
    }

    fn count_white_pixels(path: &Path, box_: (i64, i64, i64, i64)) -> usize {
        let img = image::open(path).unwrap().to_rgb8();
        let mut count = 0usize;
        for y in box_.1.max(0)..box_.3.min(img.height() as i64) {
            for x in box_.0.max(0)..box_.2.min(img.width() as i64) {
                let p = img.get_pixel(x as u32, y as u32);
                if p.0.iter().all(|c| *c >= 250) {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn default_mask_box_known_sizes() {
        assert_eq!(default_mask_box(2848, 1600), (2518, 1482, 2840, 1592));
        assert_eq!(default_mask_box(2278, 1280), (2003, 1188, 2271, 1272));
        assert_eq!(default_mask_box(2048, 2048), (1668, 1923, 2040, 2008));
        let (x1, y1, x2, y2) = default_mask_box(1024, 768);
        assert!(x1 < x2 && y1 < y2 && x2 <= 1024 - 8 && y2 <= 768 - 8);
    }

    #[test]
    fn template_assets_embedded() {
        let (tpl, alpha, meta) = load_template().expect("template assets must compile into binary");
        assert_eq!(meta.ref_short_side, 1600.0);
        assert!(tpl.width() > 200 && tpl.height() > 60);
        assert_eq!(alpha.dimensions(), tpl.dimensions(), "alpha asset must match template");
        let filled = tpl.pixels().filter(|p| p.0[0] > 127).count();
        // 笔画填充率 ~21%（远小于整框）
        let ratio = filled as f64 / (tpl.width() as f64 * tpl.height() as f64);
        assert!(ratio < 0.35, "template fill ratio too high: {ratio}");
    }

    #[test]
    fn template_stroke_mask_hits_real_style_watermark() {
        // 用真实模板字形以 α=0.6 白色叠加合成水印（同豆包混合模型），
        // template_stroke_mask 应命中且 mask 覆盖笔画区
        let (tpl, _alpha, _meta) = load_template().unwrap();
        let (w, h) = (1728u32, 2304u32);
        let scale = 1728.0 / 1600.0;
        let t = image::imageops::resize(&tpl, (tpl.width() as f64 * scale) as u32, (tpl.height() as f64 * scale) as u32, FilterType::Nearest);
        let mut img = RgbImage::from_pixel(w, h, Rgb([100, 105, 110]));
        // 贴右下角放置：笔画右缘贴图右缘（对齐提取时的边界关系）
        let px = (w - t.width()) as i32;
        let py = (h - t.height()) as i32;
        for ty in 0..t.height() {
            for tx in 0..t.width() {
                if t.get_pixel(tx, ty).0[0] > 127 {
                    let p = img.get_pixel((px + tx as i32) as u32, (py + ty as i32) as u32);
                    let blend = |c: u8| -> u8 { (c as f64 * 0.4 + 255.0 * 0.6) as u8 };
                    img.put_pixel((px + tx as i32) as u32, (py + ty as i32) as u32, Rgb([blend(p.0[0]), blend(p.0[1]), blend(p.0[2])]));
                }
            }
        }
        let hit = template_stroke_mask(&DynamicImage::ImageRgb8(img)).unwrap();
        assert!(hit.is_some(), "template should match real-style watermark");
        let TemplateHit { mask, info, .. } = hit.unwrap();
        let whites = mask.pixels().filter(|p| p.0[0] > 0).count();
        // 连续 α mask（α>0.03）覆盖全部污染像素（含抗锯齿），面积应显著小于整框
        assert!(whites > 10000, "mask should cover strokes ({whites}px): {info}");
        let box_area = (t.width() as usize) * (t.height() as usize);
        assert!(whites < box_area, "stroke mask must be smaller than the full box");
        // mask 必须集中在右下角（水印贴角）
        let bbox = mask_pixels(&mask);
        assert!(bbox.2 >= (w as i64 - 60) && bbox.3 >= (h as i64 - 60), "mask should hug bottom-right: {bbox:?}");
    }

    fn mask_pixels(mask: &GrayImage) -> (i64, i64, i64, i64) {
        let mut b = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for (x, y, p) in mask.enumerate_pixels() {
            if p.0[0] > 0 {
                b.0 = b.0.min(x as i64);
                b.1 = b.1.min(y as i64);
                b.2 = b.2.max(x as i64);
                b.3 = b.3.max(y as i64);
            }
        }
        b
    }

    #[test]
    fn template_stroke_mask_misses_clean_image() {
        let img = RgbImage::from_pixel(1600, 900, Rgb([240, 240, 233]));
        assert!(template_stroke_mask(&DynamicImage::ImageRgb8(img)).unwrap().is_none());
    }

    #[test]
    fn verify_paths_flags_outside_changes() {
        let root = temp_root("verify");
        let orig = RgbImage::from_pixel(400, 300, Rgb([100, 110, 120]));
        let orig_p = root.join("o.png");
        save_png(DynamicImage::ImageRgb8(orig.clone()), &orig_p).unwrap();
        let mut mask = GrayImage::from_pixel(400, 300, image::Luma([0]));
        for y in 100..140 {
            for x in 100..160 {
                mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        let mask_p = root.join("m.png");
        save_png(DynamicImage::ImageLuma8(mask), &mask_p).unwrap();

        // 只改 mask 内 → PASS，mask 外零改动
        let mut ok = orig.clone();
        for y in 100..140 {
            for x in 100..160 {
                ok.put_pixel(x, y, Rgb([10, 10, 10]));
            }
        }
        let ok_p = root.join("ok.png");
        save_png(DynamicImage::ImageRgb8(ok), &ok_p).unwrap();
        let rep = verify_paths(&orig_p, &ok_p, &mask_p, Some(false)).unwrap();
        assert_eq!(rep.verdict, "PASS", "{:?}", rep.reasons);
        assert_eq!(rep.outside_changed, 0);
        assert_eq!(rep.mask_area, 40 * 60);

        // 改 mask 外 1px → FAIL（mask 外零改动是硬保证）
        let mut bad = orig.clone();
        bad.put_pixel(10, 10, Rgb([200, 200, 200]));
        let bad_p = root.join("bad.png");
        save_png(DynamicImage::ImageRgb8(bad), &bad_p).unwrap();
        let rep = verify_paths(&orig_p, &bad_p, &mask_p, Some(false)).unwrap();
        assert_eq!(rep.verdict, "FAIL");
        assert_eq!(rep.outside_changed, 1);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_watermark_boxes_hits_synthetic() {
        let root = temp_root("detect");
        let path = root.join("w.png");
        let (tx1, ty1, tx2, ty2) = make_watermark_image(&path, 2848, 1600, "AI GENERATED");
        let img = image::open(&path).unwrap();
        let boxes = detect_watermark_boxes(&img);
        assert!(!boxes.is_empty(), "should detect watermark");
        // 命中的框应与文字框相交且高度对齐
        assert!(
            boxes.iter().any(|&(dx1, dy1, dx2, dy2)| {
                dx1 < tx2 && dx2 > tx1 && (dy1 - ty1).abs() <= 8 && (dy2 - ty2).abs() <= 8
            }),
            "one of {:?} should cover text ({}, {}, {}, {})",
            boxes, tx1, ty1, tx2, ty2
        );
        // 无水印的纯背景图应返回空
        let clean = root.join("clean.png");
        save_png(DynamicImage::ImageRgb8(RgbImage::from_pixel(1600, 900, Rgb([240, 240, 233]))), &clean).unwrap();
        assert!(detect_watermark_boxes(&image::open(&clean).unwrap()).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_watermark_boxes_multi_position() {
        let root = temp_root("detect-multi");
        let path = root.join("multi.png");
        let mut img = RgbImage::from_pixel(1728, 2304, Rgb([70, 80, 90]));
        let font = font();
        // 三个位置的水印 + 两个实心白块（拟灯罩/瓷盘，不得误擦）
        let spots = [(150i64, 100i64), (600, 1200), (1300, 2100)];
        for &(x, y) in &spots {
            draw_text_mut(&mut img, Rgb([120, 120, 120]), x as i32 - 2, y as i32 - 2, 60.0, &font, "AI GENERATE");
            draw_text_mut(&mut img, Rgb([255, 255, 255]), x as i32, y as i32, 60.0, &font, "AI GENERATE");
        }
        for y in 200..330 {
            for x in 900..1270 {
                let dx = (x as f64 - 1085.0) / 185.0;
                let dy = (y as f64 - 265.0) / 65.0;
                if dx * dx + dy * dy <= 1.0 {
                    img.put_pixel(x, y, Rgb([255, 255, 255]));
                }
            }
        }
        for y in 1650..1710 {
            for x in 1150..1400 {
                img.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        let path_ref = path.clone();
        save_png(DynamicImage::ImageRgb8(img), &path_ref).unwrap();
        let boxes = detect_watermark_boxes(&image::open(&path).unwrap());
        assert_eq!(boxes.len(), 3, "should detect exactly 3 watermarks, got {:?}", boxes);
        // 检测框不应互相重叠（分散水印）
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                let disjoint = a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1;
                assert!(disjoint, "boxes should be disjoint: {:?} vs {:?}", a, b);
            }
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detect_corner_faded_semitransparent() {
        // 豆包新样式：粗体灰白半透明水印（亮度 ~210，不到纯白 248），贴右下边缘
        let root = temp_root("detect-faded");
        let path = root.join("faded.png");
        let mut img = RgbImage::from_pixel(1728, 2304, Rgb([40, 45, 50]));
        let font = font();
        draw_text_mut(&mut img, Rgb([100, 100, 100]), 1520, 2232, 62.0, &font, "AI GEN");
        draw_text_mut(&mut img, Rgb([210, 212, 215]), 1522, 2234, 62.0, &font, "AI GEN");
        save_png(DynamicImage::ImageRgb8(img), &path).unwrap();
        let boxes = detect_watermark_boxes(&image::open(&path).unwrap());
        assert_eq!(boxes.len(), 1, "faded watermark should be detected, got {:?}", boxes);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolve_box_negative_and_invalid() {
        let box_ = resolve_box(1000, 800, Some(MaskBox { x1: -110, y1: -60, x2: -10, y2: -10 })).unwrap();
        assert_eq!(box_, (890, 740, 990, 790));
        assert!(resolve_box(1000, 800, Some(MaskBox { x1: 50, y1: 50, x2: 40, y2: 60 })).is_err());
    }

    /// 与 Python `refine_box_mask` 的逐像素一致性（固定 LaMa 膨胀核）。先用
    /// `python tools/refine_parity_dump.py` 生成 /tmp/refine-parity（`REFINE_PARITY_DIR`
    /// 可覆盖），产物缺失时自动跳过。
    #[test]
    #[ignore]
    fn refine_box_mask_matches_python() {
        let dir = match std::env::var("REFINE_PARITY_DIR") {
            Ok(v) => PathBuf::from(v),
            Err(_) => PathBuf::from("/tmp/refine-parity"),
        };
        let manifest = match fs::read_to_string(dir.join("manifest.json")) {
            Ok(t) => t,
            Err(_) => return,
        };
        let cases: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        let mut fails: Vec<String> = Vec::new();
        for case in cases.as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let box_ = case["box"].as_array().unwrap();
            let b = (
                box_[0].as_i64().unwrap(),
                box_[1].as_i64().unwrap(),
                box_[2].as_i64().unwrap(),
                box_[3].as_i64().unwrap(),
            );
            let image = load_image(&PathBuf::from("../../dist").join(name)).unwrap();
            let got = refine_box_mask(&image, b);
            if let Some(m) = &got {
                let _ = m.save(dir.join(format!("rust-{name}.png")));
            }
            let expected_path = case.get("mask").and_then(|v| v.as_str());
            match (got, expected_path) {
                (None, None) => {}
                (None, Some(p)) => fails.push(format!("{name}: rust refine None, python {p}")),
                (Some(_), None) => fails.push(format!("{name}: rust refine Some, python None")),
                (Some(m), Some(p)) => {
                    let exp = image::open(p).unwrap().to_luma8();
                    assert_eq!(m.dimensions(), exp.dimensions(), "{name}: size mismatch");
                    let (mut inter, mut union) = (0f64, 0f64);
                    for (a, b) in m.pixels().zip(exp.pixels()) {
                        let (a, b) = (a.0[0] > 0, b.0[0] > 0);
                        if a || b {
                            union += 1.0;
                        }
                        if a && b {
                            inter += 1.0;
                        }
                    }
                    let iou = if union == 0.0 { 1.0 } else { inter / union };
                    println!("{name}: IOU {iou:.4}");
                    if iou < 0.995 {
                        fails.push(format!("{name}: IOU {iou:.4}"));
                    }
                }
            }
        }
        assert!(fails.is_empty(), "refine parity failures: {fails:?}");
    }

    #[test]
    fn output_dir_modes() {
        let root = temp_root("outdir");
        let options = PipelineOptions {
            root: root.clone(),
            files: vec![],
            keep_work: false,
            mask_box: None,
            any_position: false,
            overwrite_original: true,
            use_profile: true,
            force: false,
            inverse: true,
            retry: true,
            refine: false,
            forced_profile: None,
            output_dir_override: None,
        };
        assert_eq!(output_dir(&options), root);
        let options = PipelineOptions { overwrite_original: false, ..options };
        assert_eq!(output_dir(&options), root.join("watermark-cleaned"));
        // 自定义输出目录：另存模式生效；覆盖模式忽略（保证"原图已替换"的语义不被改写）
        let custom = root.join("custom-out");
        let options = PipelineOptions { output_dir_override: Some(custom.clone()), ..options };
        assert_eq!(output_dir(&options), custom);
        let options = PipelineOptions { overwrite_original: true, ..options };
        assert_eq!(output_dir(&options), root, "覆盖模式必须忽略自定义目录");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn restore_backup_copies_originals_back() {
        let root = temp_root("restore");
        fs::write(root.join("a.png"), b"cleaned").unwrap();
        let backup = root.join("original-watermark-backup");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("a.png"), b"original").unwrap();
        assert_eq!(restore_backup(&root).unwrap(), 1);
        assert_eq!(fs::read(root.join("a.png")).unwrap(), b"original");
        assert!(!backup.exists(), "恢复后应删除备份目录，避免重复恢复");
        // 没有备份时必须是 Err（前端据此提示"无可恢复内容"），不能静默成功
        assert!(restore_backup(&root).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn target_names_sorted_and_filtered() {
        let root = temp_root("names");
        for name in ["2.png", "10.png", "1.png", "ignore.jpg", "3.PNG", "4.webp", "5.jpeg", "skip.gif"] {
            fs::write(root.join(name), b"x").unwrap();
        }
        fs::create_dir_all(root.join("sub")).unwrap();
        let names = target_names(&root, &[]).unwrap();
        assert_eq!(names, vec!["1.png", "2.png", "3.PNG", "4.webp", "5.jpeg", "10.png", "ignore.jpg"]);
        let picked = target_names(&root, &["3.PNG".to_string()]).unwrap();
        assert_eq!(picked, vec!["3.PNG"]);
        assert!(target_names(&root, &["skip.gif".to_string()]).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    /// 两个测试都要改 DOUBAO_WATERMARK_WORKDIR 环境变量，必须串行执行
    static WORKDIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn pipeline_file_flow_without_model() {
        let _guard = WORKDIR_ENV_LOCK.lock().unwrap();
        let prev_work = std::env::var("DOUBAO_WATERMARK_WORKDIR").ok();
        let root = temp_root("flow");
        std::env::set_var(
            "DOUBAO_WATERMARK_WORKDIR",
            temp_root("flow-work"),
        );
        let (x1, y1, _x2, _y2) = make_watermark_image(&root.join("b.png"), 1024, 1024, "AI");
        let _ = x1;
        let _ = y1;
        let options = PipelineOptions { root: root.clone(), files: vec!["b.png".to_string()], keep_work: false, mask_box: None, any_position: false, overwrite_original: true, use_profile: true, force: false, inverse: true, retry: false, refine: false, forced_profile: None, output_dir_override: None };
        let names = target_names(&root, &options.files).unwrap();
        let noop_log: Logger = &|_| {};
        prepare(&options, &names, &noop_log).unwrap();

        let backup = root.join("original-watermark-backup/b.png");
        assert!(backup.exists(), "backup created");
        let (source, masks, _lama, review) = work_dirs();
        assert!(source.join("b.png").exists());
        assert!(masks.join("b.png").exists());
        assert!(review.join("source-corner-review.png").exists());

        // 伪造推理输出：只改遮罩内像素（验证闭环要求 mask 外零改动）
        let img = image::open(&backup).unwrap().to_rgb8();
        let mut fake = img.clone();
        let pattern = image::open(masks.join("b.png")).unwrap().to_luma8();
        for (x, y, p) in pattern.enumerate_pixels() {
            if p.0[0] > 0 {
                fake.put_pixel(x, y, Rgb([240, 240, 233]));
            }
        }
        save_png(DynamicImage::ImageRgb8(fake), &_lama.join("b.png")).unwrap();

        let candidate = review_lama(&names, &root).unwrap();
        assert!(candidate.exists());
        let final_ = overwrite_review(&options, &names).unwrap();
        assert!(final_.exists());
        assert!(root.join("b.png").exists());

        cleanup(&options, &names).unwrap();
        assert!(!root.join("original-watermark-backup").exists(), "backup cleaned");
        assert!(!crate::workdir().exists(), "workdir cleaned");

        match prev_work {
            Some(v) => std::env::set_var("DOUBAO_WATERMARK_WORKDIR", v),
            None => std::env::remove_var("DOUBAO_WATERMARK_WORKDIR"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// 完整 LaMa 推理 E2E（需要真实模型，CI 下载后运行：cargo test -- --ignored）
    #[test]
    #[ignore]
    fn full_run_with_lama_e2e() {
        let _guard = WORKDIR_ENV_LOCK.lock().unwrap();
        let model = std::path::PathBuf::from(
            std::env::var("LAMA_MODEL").unwrap_or_else(|_| ".models/lama_fp32.onnx".into()),
        );
        assert!(model.exists(), "model not found at {}; set LAMA_MODEL", model.display());

        let prev_work = std::env::var("DOUBAO_WATERMARK_WORKDIR").ok();
        let root = temp_root("e2e");
        std::env::set_var("DOUBAO_WATERMARK_WORKDIR", temp_root("e2e-work"));
        let (x1, y1, x2, y2) = make_watermark_image(&root.join("case.png"), 2048, 2048, "AI");
        let white_before = count_white_pixels(&root.join("case.png"), (x1, y1, x2, y2));
        assert!(white_before > 200, "fixture should contain white text");

        let options = PipelineOptions {
            root: root.clone(),
            files: vec!["case.png".to_string()],
            keep_work: false,
            mask_box: Some(MaskBox { x1: x1 - 20, y1: y1 - 20, x2: x2 + 20, y2: y2 + 20 }),
            any_position: false,
            overwrite_original: true,
            use_profile: true,
            force: false,
            inverse: true,
            retry: true,
            refine: false,
            forced_profile: None,
            output_dir_override: None,
        };
        let log = |_line: &str| {};
        let summary = run(&options, &model, &log, &|_, _, _, _| {}, &|| false).expect("pipeline run failed");
        assert!(summary.final_review.exists());

        let white_after = count_white_pixels(&root.join("case.png"), (x1, y1, x2, y2));
        println!("white pixels before={}, after={}", white_before, white_after);
        assert!(
            white_after * 2 < white_before,
            "watermark text should be largely removed (before={}, after={})",
            white_before,
            white_after
        );

        assert!(!root.join("original-watermark-backup").exists(), "backup cleaned");
        assert!(!crate::workdir().exists(), "workdir cleaned");

        match prev_work {
            Some(v) => std::env::set_var("DOUBAO_WATERMARK_WORKDIR", v),
            None => std::env::remove_var("DOUBAO_WATERMARK_WORKDIR"),
        }
        let _ = fs::remove_dir_all(&root);
    }
}

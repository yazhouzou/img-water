use std::fs;
use std::path::{Path, PathBuf};

use ab_glyph::FontArc;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, RgbImage};
use imageproc::drawing::draw_text_mut;

use crate::lama::Lama;

const WINDOW: u32 = 512;

// 模板笔画 mask：豆包水印字形全图固定（半透明白字 α≈0.6 叠加），从黑底图提取
// 笔画级模板（阈值 ≥125 + 3x3 闭运算，填充率仅 ~21%）。资产编译进二进制，
// CLI 与打包 App 均可用。
const TEMPLATE_PNG: &[u8] = include_bytes!("../../../tools/doubao-wm-template.png");
const TEMPLATE_META_JSON: &str = include_str!("../../../tools/doubao-wm-template.json");
// gap-score（笔画均亮 - 间隙均亮）实测：黑底 144 / 雪景 102-136 / 花墙 57 /
// 沙滩 62；纸面低对比 15 回退整框检测。阈值取中间空档。
const TEMPLATE_MIN_SCORE: f64 = 40.0;
// 膨胀核 19x11 矩形（水平 ±9 / 垂直 ±5）：水平填满字符间距使 mask 连成片，
// 消除间隙里的字形上下文，防止 LaMa FFT 感受野"见字生字"；垂直只需盖住
// 抗锯齿带（±5px），少侵入 mask 上下画面——水印横跨花墙棱线等强结构边界时，
// 全向 9px 会让 LaMa 重绘垂直宽带产生混沌（6.png 教训）。
// mask 必须放在 gap-score 匹配位置：手工放置偏 8px 时小膨胀盖不住字形，
// 会误判为"垂直膨胀不足"（位置对齐比膨胀参数更关键）。
const TEMPLATE_DILATE_W: usize = 19;
const TEMPLATE_DILATE_H: usize = 11;

#[derive(serde::Deserialize)]
struct TemplateMeta {
    #[serde(rename = "ref_short_side")]
    ref_short_side: f64,
}

fn load_template() -> Result<(GrayImage, TemplateMeta), String> {
    let tpl = image::load_from_memory_with_format(TEMPLATE_PNG, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .to_luma8();
    let meta: TemplateMeta =
        serde_json::from_str(TEMPLATE_META_JSON).map_err(|e| e.to_string())?;
    Ok((tpl, meta))
}

/// 模板笔画 mask 匹配：按图片短边比例缩放模板（水印尺寸随短边等比），在右下角
/// 40px 余量窗口内做 gap-score 匹配（笔画区均亮 - 间隙区均亮），命中返回
/// (mask, 描述)。分数不足返回 None 交给整框检测回退。
fn template_stroke_mask(image: &DynamicImage) -> Result<Option<(GrayImage, String)>, String> {
    let (tpl, meta) = load_template()?;
    let rgb = image.to_rgb8();
    let (iw, ih) = (rgb.width() as usize, rgb.height() as usize);
    let mut gray = vec![0f64; iw * ih];
    for (i, p) in rgb.pixels().enumerate() {
        gray[i] = p.0.iter().copied().fold(0u8, u8::max) as f64;
    }
    let scale = (iw.min(ih) as f64) / meta.ref_short_side;
    let tw = ((tpl.width() as f64) * scale) as u32;
    let th = ((tpl.height() as f64) * scale) as u32;
    if tw >= rgb.width() || th >= rgb.height() {
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
    let n_in = pts.len() as f64;
    let n_out = (tw as usize * th as usize) as f64 - n_in;
    // 积分图（前缀和）求任意矩形和：S_all 与匹配窗口内的 S_out
    let iw1 = iw + 1;
    let mut integral = vec![0f64; iw1 * (ih + 1)];
    for y in 0..ih {
        let mut row_acc = 0f64;
        for x in 0..iw {
            row_acc += gray[y * iw + x];
            integral[(y + 1) * iw1 + (x + 1)] = integral[y * iw1 + (x + 1)] + row_acc;
        }
    }
    let rect_sum = |x1: usize, y1: usize, x2: usize, y2: usize| -> f64 {
        integral[y2 * iw1 + x2] + integral[y1 * iw1 + x1] - integral[y1 * iw1 + x2]
            - integral[y2 * iw1 + x1]
    };
    // 水印必贴右下角：模板左上角只可能在 (w-tw-40..=w-tw, h-th-40..=h-th)
    let max_py = ih - th as usize;
    let max_px = iw - tw as usize;
    let min_py = max_py.saturating_sub(40);
    let min_px = max_px.saturating_sub(40);
    let mut best = (f64::MIN, 0usize, 0usize);
    for py in min_py..=max_py {
        for px in min_px..=max_px {
            let mut s_in = 0f64;
            for &(dx, dy) in &pts {
                s_in += gray[(py + dy) * iw + px + dx];
            }
            let s_all = rect_sum(px, py, px + tw as usize, py + th as usize);
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
    // 膨胀矩形核（水平 ±9 / 垂直 ±5）：方形核可分解，横向 19 + 纵向 11 两次一维扩展
    let mut bin = vec![false; (tw as usize) * (th as usize)];
    for &(dx, dy) in &pts {
        bin[dy * tw as usize + dx] = true;
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
    let info = format!("template mask matched at ({px},{py}) score {score:.1}");
    Ok(Some((mask, info)))
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
        options.root.clone()
    } else {
        options.root.join("watermark-cleaned")
    }
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
                // 三级策略 ①：模板笔画 mask（复杂场景精确修复，见 template_stroke_mask）
                if let Some((tpl, info)) = template_stroke_mask(&image)? {
                    log(&format!("{}: {}", name, info));
                    tpl_mask = Some(tpl);
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
        for (x1, y1, x2, y2) in &boxes {
            for y in *y1..*y2 {
                for x in *x1..*x2 {
                    mask.put_pixel(x as u32, y as u32, image::Luma([255]));
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

pub fn inpaint(
    model_path: &Path,
    log: Logger,
    progress: ProgressFn,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let (source, masks, lama_dir, _) = work_dirs();
    let mut engine = Lama::load(model_path)?;
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
    for (index, path) in entries.iter().enumerate() {
        if is_cancelled() {
            return Err(CANCELLED.to_string());
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let image = load_image(path)?;
        let mask = load_image(&masks.join(&name))?.to_luma8();
        // 空 mask（未检测到水印）的图直接原样通过，不进模型
        if mask.iter().all(|&v| v == 0) {
            fs::copy(path, lama_dir.join(&name)).map_err(|e| e.to_string())?;
            log(&format!("{}: empty mask, passed through without inpainting", name));
            progress("inpaint", index + 1, total, &name);
            continue;
        }
        log(&format!("inpainting {}...", name));
        progress("inpaint", index, total, &name);
        let result = engine.inpaint_image(&image, &mask, log)?;
        save_png(DynamicImage::ImageRgb8(result), &lama_dir.join(&name))?;
        log(&format!("done {}", name));
        progress("inpaint", index + 1, total, &name);
    }
    Ok(())
}

pub fn review_lama(names: &[String]) -> Result<PathBuf, String> {
    let (_, _, lama_dir, review_dir) = work_dirs();
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
    inpaint(model_path, log, progress, is_cancelled)?;
    let candidate_review = review_lama(&names)?;
    let final_review = finalize_outputs(options, &names, log)?;
    let output_dir_path = output_dir(options);
    if options.keep_work {
        return Ok(RunSummary {
            source_review,
            candidate_review,
            final_review,
            kept_work: true,
            cancelled: false,
            processed: names.len(),
            output_dir: output_dir_path,
        });
    }
    let (source_review, candidate_review, final_review) =
        preserve_reviews(&source_review, &candidate_review, &final_review)?;
    cleanup(options, &names)?;
    log(&format!(
        "cleaned: {} and {}",
        backup_dir(&options.root).display(),
        crate::workdir().display()
    ));
    Ok(RunSummary {
        source_review,
        candidate_review,
        final_review,
        kept_work: false,
        cancelled: false,
        processed: names.len(),
        output_dir: output_dir_path,
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
        let (tpl, meta) = load_template().expect("template assets must compile into binary");
        assert_eq!(meta.ref_short_side, 1600.0);
        assert!(tpl.width() > 200 && tpl.height() > 60);
        let filled = tpl.pixels().filter(|p| p.0[0] > 127).count();
        // 笔画填充率 ~21%（远小于整框）
        let ratio = filled as f64 / (tpl.width() as f64 * tpl.height() as f64);
        assert!(ratio < 0.35, "template fill ratio too high: {ratio}");
    }

    #[test]
    fn template_stroke_mask_hits_real_style_watermark() {
        // 用真实模板字形以 α=0.6 白色叠加合成水印（同豆包混合模型），
        // template_stroke_mask 应命中且 mask 覆盖笔画区
        let (tpl, _meta) = load_template().unwrap();
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
        let (mask, info) = hit.unwrap();
        let whites = mask.pixels().filter(|p| p.0[0] > 0).count();
        assert!(whites > 10000, "mask should cover strokes ({whites}px): {info}");
        // mask 必须集中在右下角（水印贴角）
        let bbox = maskPixels(&mask);
        assert!(bbox.2 >= (w as i64 - 60) && bbox.3 >= (h as i64 - 60), "mask should hug bottom-right: {bbox:?}");
    }

    fn maskPixels(mask: &GrayImage) -> (i64, i64, i64, i64) {
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
        };
        assert_eq!(output_dir(&options), root);
        let options = PipelineOptions { overwrite_original: false, ..options };
        assert_eq!(output_dir(&options), root.join("watermark-cleaned"));
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
        let options = PipelineOptions { root: root.clone(), files: vec!["b.png".to_string()], keep_work: false, mask_box: None, any_position: false, overwrite_original: true };
        let names = target_names(&root, &options.files).unwrap();
        let noop_log: Logger = &|_| {};
        prepare(&options, &names, &noop_log).unwrap();

        let backup = root.join("original-watermark-backup/b.png");
        assert!(backup.exists(), "backup created");
        let (source, masks, _lama, review) = work_dirs();
        assert!(source.join("b.png").exists());
        assert!(masks.join("b.png").exists());
        assert!(review.join("source-corner-review.png").exists());

        // 伪造推理输出（遮罩外的内容保持，遮罩内填背景色）
        let img = image::open(&backup).unwrap().to_rgb8();
        let mut fake = img.clone();
        let (bx1, by1, bx2, by2) = resolve_box(1024, 1024, None).unwrap();
        for y in by1..by2 {
            for x in bx1..bx2 {
                fake.put_pixel(x as u32, y as u32, Rgb([240, 240, 233]));
            }
        }
        save_png(DynamicImage::ImageRgb8(fake), &_lama.join("b.png")).unwrap();

        let candidate = review_lama(&names).unwrap();
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

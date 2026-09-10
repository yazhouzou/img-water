use std::fs;
use std::path::{Path, PathBuf};

use ab_glyph::FontArc;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, RgbImage};
use imageproc::drawing::draw_text_mut;

use crate::lama::Lama;

const WINDOW: u32 = 512;

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

/// 与 Python `detect_watermark_box` 对齐：右下角搜索“纯白文字”聚类（白字+灰描边特征）。
/// cv2 依赖不可用，膨胀用可分离矩形核、连通域用 BFS，行为与 Python 版一致。
pub fn detect_watermark_box(image: &DynamicImage) -> Option<(i64, i64, i64, i64)> {
    let rgb = image.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let x0 = w * 55 / 100;
    let y0 = h * 60 / 100;
    let rw = w - x0;
    let rh = h - y0;
    if rw == 0 || rh == 0 {
        return None;
    }
    let mut grid = vec![false; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let p = rgb.get_pixel((x0 + x) as u32, (y0 + y) as u32);
            grid[y * rw + x] = p.0[0] >= 248 && p.0[1] >= 248 && p.0[2] >= 248;
        }
    }
    let dilated = dilate_rect_9x3_twice(&grid, rw, rh);

    let mut visited = vec![false; rw * rh];
    let mut best: Option<(usize, usize, usize, usize)> = None;
    let mut best_score = 0.0f64;
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
            let (cwf, chf) = (cw as f64, ch as f64);
            if !(hf * 0.012..=hf * 0.09).contains(&chf) || cw < ch {
                continue;
            }
            let ratio = cwf / chf;
            if !(2.0..=15.0).contains(&ratio) {
                continue;
            }
            let fill = area as f64 / (cwf * chf);
            if !(0.2..=0.9).contains(&fill) {
                continue;
            }
            let corner_dist = ((w - (x0 + maxx + 1)) + (h - (y0 + maxy + 1))) as f64;
            let score = area as f64 * (ratio / 6.0).min(1.0) / (1.0 + corner_dist / (wf * 0.1));
            if score > best_score {
                best_score = score;
                best = Some((x0 + minx, y0 + miny, x0 + maxx + 1, y0 + maxy + 1));
            }
        }
    }
    let (bx1, by1, bx2, by2) = best?;
    let pad = (h / 150).max(10) as i64;
    let (iw, ih) = (w as i64, h as i64);
    Some((
        (bx1 as i64 - pad).max(0),
        (by1 as i64 - pad).max(0),
        (bx2 as i64 + pad).min(iw - 6),
        (by2 as i64 + pad).min(ih - 6),
    ))
}

/// 矩形核 9x3 膨胀两次（可分离实现：水平半径 4 + 垂直半径 1）
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
    let stem = lower.strip_suffix(".png").unwrap_or(&lower);
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
            .filter(|name| name.to_lowercase().ends_with(".png"))
            .collect();
        entries.sort_by_key(|name| numeric_key(name));
        entries
    } else {
        for name in files {
            if !name.to_lowercase().ends_with(".png") {
                return Err(format!("not a png: {}", name));
            }
            if !root.join(name).exists() {
                return Err(format!("missing file: {}", name));
            }
        }
        files.to_vec()
    };
    if names.is_empty() {
        return Err(format!(
            "no png files found in: {}\nhint: pass a folder, e.g. clean-cli --root /path/to/images run",
            root.display()
        ));
    }
    Ok(names)
}

fn backup_dir(root: &Path) -> PathBuf {
    root.join("original-watermark-backup")
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

fn ensure_dirs(root: &Path) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), String> {
    let (source, masks, lama, review) = work_dirs();
    for path in [&source, &masks, &lama, &review, &backup_dir(root)] {
        fs::create_dir_all(path).map_err(|e| e.to_string())?;
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
    let (source, masks, _lama, review_dir) = ensure_dirs(&options.root)?;
    let backup_root = backup_dir(&options.root);
    for name in names {
        let current = options.root.join(name);
        let backup = backup_root.join(name);
        if !backup.exists() {
            fs::copy(&current, &backup).map_err(|e| e.to_string())?;
        }
        fs::copy(&backup, source.join(name)).map_err(|e| e.to_string())?;

        let image = load_image(&backup)?;
        let (width, height) = (image.width(), image.height());
        let (x1, y1, x2, y2) = match options.mask_box {
            Some(box_) => resolve_box(width, height, Some(box_))?,
            None => match detect_watermark_box(&image) {
                Some((bx1, by1, bx2, by2)) => {
                    log(&format!("{}: auto-detected mask box ({}, {}, {}, {})", name, bx1, by1, bx2, by2));
                    (bx1, by1, bx2, by2)
                }
                None => {
                    let (bx1, by1, bx2, by2) = default_mask_box(width, height);
                    log(&format!("{}: detection failed, using default rule ({}, {}, {}, {})", name, bx1, by1, bx2, by2));
                    (bx1, by1, bx2, by2)
                }
            },
        };
        let mut mask = GrayImage::from_pixel(width, height, image::Luma([0]));
        for y in y1..y2 {
            for x in x1..x2 {
                mask.put_pixel(x as u32, y as u32, image::Luma([255]));
            }
        }
        save_png(DynamicImage::ImageLuma8(mask), &masks.join(name))?;
    }
    let output = review(&source, &review_dir, "source-corner-review.png", names)?;
    println!("{}", output.display());
    Ok(())
}

pub fn inpaint(model_path: &Path, log: Logger) -> Result<(), String> {
    let (source, masks, lama_dir, _) = work_dirs();
    let mut engine = Lama::load(model_path)?;
    let entries: Vec<PathBuf> = fs::read_dir(&source)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|e| e.eq_ignore_ascii_case("png")).unwrap_or(false))
        .collect();
    if entries.is_empty() {
        return Err("workdir has no prepared source images; run prepare first".into());
    }
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let image = load_image(&path)?;
        let mask = load_image(&masks.join(&name))?.to_luma8();
        log(&format!("inpainting {}...", name));
        let result = engine.inpaint_image(&image, &mask, log)?;
        save_png(DynamicImage::ImageRgb8(result), &lama_dir.join(&name))?;
        log(&format!("done {}", name));
    }
    Ok(())
}

pub fn review_lama(names: &[String]) -> Result<PathBuf, String> {
    let (_, _, lama_dir, review_dir) = work_dirs();
    let output = review(&lama_dir, &review_dir, "lama-corner-review.png", names)?;
    println!("{}", output.display());
    Ok(output)
}

pub fn overwrite_review(options: &PipelineOptions, names: &[String]) -> Result<PathBuf, String> {
    let (_, _, lama_dir, review_dir) = work_dirs();
    for name in names {
        fs::copy(lama_dir.join(name), options.root.join(name)).map_err(|e| e.to_string())?;
    }
    let output = review(&options.root, &review_dir, "overwritten-corner-review.png", names)?;
    println!("{}", output.display());
    Ok(output)
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
        .map(|parent| parent.join("doubao-watermark-review"))
        .unwrap_or_else(|| std::env::temp_dir().join("doubao-watermark-review"))
}

fn preserve_reviews(candidate: &Path, final_: &Path) -> Result<(PathBuf, PathBuf), String> {
    let dest_dir = preserved_review_dir();
    fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let c = dest_dir.join(candidate.file_name().ok_or("bad review path")?);
    let f = dest_dir.join(final_.file_name().ok_or("bad review path")?);
    fs::copy(candidate, &c).map_err(|e| e.to_string())?;
    fs::copy(final_, &f).map_err(|e| e.to_string())?;
    Ok((c, f))
}

pub struct RunSummary {
    pub candidate_review: PathBuf,
    pub final_review: PathBuf,
    pub kept_work: bool,
}

pub fn run(options: &PipelineOptions, model_path: &Path, log: Logger) -> Result<RunSummary, String> {
    let names = target_names(&options.root, &options.files)?;
    log(&format!("processing {} file(s)", names.len()));
    prepare(options, &names, log)?;
    inpaint(model_path, log)?;
    let candidate_review = review_lama(&names)?;
    let final_review = overwrite_review(options, &names)?;
    if options.keep_work {
        return Ok(RunSummary {
            candidate_review,
            final_review,
            kept_work: true,
        });
    }
    let (candidate_review, final_review) = preserve_reviews(&candidate_review, &final_review)?;
    cleanup(options, &names)?;
    log(&format!(
        "cleaned: {} and {}",
        backup_dir(&options.root).display(),
        crate::workdir().display()
    ));
    Ok(RunSummary {
        candidate_review,
        final_review,
        kept_work: false,
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
    pub(super) fn make_watermark_image(path: &Path, width: u32, height: u32, text: &str) -> (i64, i64, i64, i64) {
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
    fn detect_watermark_box_hits_synthetic() {
        let root = temp_root("detect");
        let path = root.join("w.png");
        let (tx1, ty1, tx2, ty2) = make_watermark_image(&path, 2848, 1600, "AI GENERATED");
        let img = image::open(&path).unwrap();
        let (dx1, dy1, dx2, dy2) = detect_watermark_box(&img).expect("should detect watermark");
        // 字符可能断开成多个连通域（与 cv2 行为一致），主组件应覆盖大部分文字
        let (dw, dh) = (dx2 - dx1, dy2 - dy1);
        assert!(dw * 2 >= tx2 - tx1, "detected width {} should cover most of text width {}", dw, tx2 - tx1);
        assert!(dy1 <= ty1 + 8 && dy2 >= ty2 - 8, "detected height range ({}, {}) should cover text ({}, {})", dy1, dy2, ty1, ty2);
        assert!(dx1 >= tx1 - 100 && dx2 <= tx2 + 100, "detected box should stay near text box");
        // 无水印的纯背景图应回退 None
        let clean = root.join("clean.png");
        save_png(DynamicImage::ImageRgb8(RgbImage::from_pixel(1600, 900, Rgb([240, 240, 233]))), &clean).unwrap();
        assert_eq!(detect_watermark_box(&image::open(&clean).unwrap()), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolve_box_negative_and_invalid() {
        let box_ = resolve_box(1000, 800, Some(MaskBox { x1: -110, y1: -60, x2: -10, y2: -10 })).unwrap();
        assert_eq!(box_, (890, 740, 990, 790));
        assert!(resolve_box(1000, 800, Some(MaskBox { x1: 50, y1: 50, x2: 40, y2: 60 })).is_err());
    }

    #[test]
    fn target_names_sorted_and_filtered() {
        let root = temp_root("names");
        for name in ["2.png", "10.png", "1.png", "ignore.jpg", "3.PNG"] {
            fs::write(root.join(name), b"x").unwrap();
        }
        fs::create_dir_all(root.join("sub")).unwrap();
        let names = target_names(&root, &[]).unwrap();
        assert_eq!(names, vec!["1.png", "2.png", "3.PNG", "10.png"]);
        let picked = target_names(&root, &["3.PNG".to_string()]).unwrap();
        assert_eq!(picked, vec!["3.PNG"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn pipeline_file_flow_without_model() {
        let prev_work = std::env::var("DOUBAO_WATERMARK_WORKDIR").ok();
        let root = temp_root("flow");
        std::env::set_var(
            "DOUBAO_WATERMARK_WORKDIR",
            temp_root("flow-work"),
        );
        let (x1, y1, _x2, _y2) = make_watermark_image(&root.join("b.png"), 1024, 1024, "AI");
        let _ = x1;
        let _ = y1;
        let options = PipelineOptions { root: root.clone(), files: vec!["b.png".to_string()], keep_work: false, mask_box: None };
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
        };
        let log = |_line: &str| {};
        let summary = run(&options, &model, &log).expect("pipeline run failed");
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

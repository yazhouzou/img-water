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

fn numeric_key(name: &str) -> (u8, u64, String) {
    let stem = name.strip_suffix(".png").unwrap_or(name);
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

pub fn prepare(options: &PipelineOptions, names: &[String]) -> Result<(), String> {
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
        let (x1, y1, x2, y2) = resolve_box(width, height, options.mask_box)?;
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
    prepare(options, &names)?;
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

use std::path::Path;

use image::DynamicImage;
use ort::session::{builder::GraphOptimizationLevel, Session};

const WINDOW: u32 = 512;
const MAX_BBOX: u32 = 496;

pub struct Lama {
    session: Session,
}

struct BBox {
    x1: u32,
    y1: u32,
    x2: u32,
    y2: u32,
}

fn mask_bbox(mask: &image::GrayImage) -> Option<BBox> {
    let (width, height) = (mask.width(), mask.height());
    let mut x1 = width;
    let mut y1 = height;
    let mut x2 = 0u32;
    let mut y2 = 0u32;
    for (x, y, value) in mask.enumerate_pixels() {
        if value.0[0] > 0 {
            x1 = x1.min(x);
            y1 = y1.min(y);
            x2 = x2.max(x);
            y2 = y2.max(y);
        }
    }
    if x2 < x1 || y2 < y1 {
        return None;
    }
    Some(BBox {
        x1,
        y1,
        x2: x2 + 1,
        y2: y2 + 1,
    })
}

fn reflect_pad(img: &DynamicImage, win: u32, ox: i64, oy: i64) -> image::RgbImage {
    let rgba = img.to_rgb8();
    let (w, h) = (rgba.width() as i64, rgba.height() as i64);
    let mut out = image::RgbImage::new(win, win);
    for y in 0..win as i64 {
        for x in 0..win as i64 {
            let sx = reflect_coord(x + ox, w);
            let sy = reflect_coord(y + oy, h);
            out.put_pixel(x as u32, y as u32, rgba.get_pixel(sx as u32, sy as u32).clone());
        }
    }
    out
}

fn reflect_coord(mut v: i64, size: i64) -> i64 {
    if size == 1 {
        return 0;
    }
    let period = size * 2 - 2;
    v = v.rem_euclid(period);
    if v >= size {
        v = period - v;
    }
    v.clamp(0, size - 1)
}

impl Lama {
    pub fn load(path: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?
            .with_intra_threads(num_cpus_hint())
            .map_err(|e| e.to_string())?
            .commit_from_file(path)
            .map_err(|e| format!("加载 ONNX 模型失败: {}", e))?;
        Ok(Self { session })
    }

    pub fn inpaint_image(
        &mut self,
        image: &DynamicImage,
        mask: &image::GrayImage,
        log: &dyn Fn(&str),
    ) -> Result<image::RgbImage, String> {
        let (width, height) = (image.width(), image.height());
        if width != mask.width() || height != mask.height() {
            return Err("image and mask size mismatch".into());
        }
        if width < WINDOW || height < WINDOW {
            return Err(format!("image {}x{} smaller than {}px window", width, height, WINDOW));
        }
        let bbox = mask_bbox(mask).ok_or("mask is empty")?;
        let bw = bbox.x2 - bbox.x1;
        let bh = bbox.y2 - bbox.y1;
        if bw > MAX_BBOX || bh > MAX_BBOX {
            return Err(format!(
                "mask region {}x{} exceeds {}x{} window support",
                bw, bh, MAX_BBOX, MAX_BBOX
            ));
        }

        let ox = {
            let center = (bbox.x1 + bbox.x2) as i64 / 2 - WINDOW as i64 / 2;
            center.clamp(0, width as i64 - WINDOW as i64).max(0)
        };
        let oy = {
            let center = (bbox.y1 + bbox.y2) as i64 / 2 - WINDOW as i64 / 2;
            center.clamp(0, height as i64 - WINDOW as i64).max(0)
        };

        let win_img = reflect_pad(image, WINDOW, ox, oy);
        let mut win_mask = image::GrayImage::new(WINDOW, WINDOW);
        let oxu = ox as u32;
        let oyu = oy as u32;
        let y_end = (oyu + WINDOW).min(height);
        let x_end = (oxu + WINDOW).min(width);
        for y in oyu..y_end {
            for x in oxu..x_end {
                let value = mask.get_pixel(x, y).0[0];
                if value > 0 {
                    win_mask.put_pixel(x - oxu, y - oyu, image::Luma([value]));
                }
            }
        }
        if std::env::var("LAMA_DEBUG_DUMP").is_ok() {
            let dir = std::path::PathBuf::from("lama-debug");
            let _ = std::fs::create_dir_all(&dir);
            let _ = win_img.save(dir.join("win_img.png"));
            let _ = win_mask.save(dir.join("win_mask.png"));
        }

        let mut chw = vec![0f32; 3 * WINDOW as usize * WINDOW as usize];
        for y in 0..WINDOW {
            for x in 0..WINDOW {
                let p = win_img.get_pixel(x, y);
                let base = (y * WINDOW + x) as usize;
                for c in 0..3 {
                    chw[c * WINDOW as usize * WINDOW as usize + base] = p.0[c] as f32 / 255.0;
                }
            }
        }
        let mask_data: Vec<f32> = win_mask
            .pixels()
            .map(|p| if p.0[0] > 0 { 1.0f32 } else { 0.0 })
            .collect();

        let outputs = self
            .session
            .run(
                ort::inputs![
                    "image" => ort::value::Tensor::from_array((vec![1i64, 3, WINDOW as i64, WINDOW as i64], chw)).map_err(|e| e.to_string())?,
                    "mask" => ort::value::Tensor::from_array((vec![1i64, 1, WINDOW as i64, WINDOW as i64], mask_data)).map_err(|e| e.to_string())?,
                ],
            )
            .map_err(|e| format!("ONNX 推理失败: {}", e))?;
        let output = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        let expected = WINDOW as usize * WINDOW as usize * 3;
        let data: &[f32] = output.1;
        if data.len() < expected {
            return Err(format!("unexpected model output size {}", data.len()));
        }
        let _ = output.0;

        let mut result = image.to_rgb8();
        let plane = WINDOW as usize * WINDOW as usize;
        let m = WINDOW as i64;
        for y in 0..(height as i64 - oy).min(m) {
            for x in 0..(width as i64 - ox).min(m) {
                let sx = (x + ox) as u32;
                let sy = (y + oy) as u32;
                if mask.get_pixel(sx, sy).0[0] == 0 {
                    continue;
                }
                let base = (y * m + x) as usize;
                let pixel = image::Rgb([
                    data[base].clamp(0.0, 255.0) as u8,
                    data[plane + base].clamp(0.0, 255.0) as u8,
                    data[2 * plane + base].clamp(0.0, 255.0) as u8,
                ]);
                result.put_pixel(sx, sy, pixel);
            }
        }
        log(&format!(
            "inferred {}x{} window at ({}, {})",
            WINDOW, WINDOW, ox, oy
        ));
        Ok(result)
    }
}

fn num_cpus_hint() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

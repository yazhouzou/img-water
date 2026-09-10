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

/// 把遮罩按 8 连通分解成多个独立区域（支持多位置分散水印），返回每块的包围盒。
fn mask_components(mask: &image::GrayImage) -> Vec<BBox> {
    let (width, height) = (mask.width() as usize, mask.height() as usize);
    let active = |x: usize, y: usize| mask.get_pixel(x as u32, y as u32).0[0] > 0;
    let mut visited = vec![false; width * height];
    let mut boxes = Vec::new();
    for sy in 0..height {
        for sx in 0..width {
            let idx = sy * width + sx;
            if visited[idx] || !active(sx, sy) {
                continue;
            }
            let mut stack = vec![idx];
            visited[idx] = true;
            let (mut minx, mut miny, mut maxx, mut maxy) = (sx, sy, sx, sy);
            while let Some(cur) = stack.pop() {
                let cx = cur % width;
                let cy = cur / width;
                minx = minx.min(cx);
                maxx = maxx.max(cx);
                miny = miny.min(cy);
                maxy = maxy.max(cy);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let nx = cx as i64 + dx;
                        let ny = cy as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                            continue;
                        }
                        let ni = ny as usize * width + nx as usize;
                        if !visited[ni] && active(nx as usize, ny as usize) {
                            visited[ni] = true;
                            stack.push(ni);
                        }
                    }
                }
            }
            boxes.push(BBox {
                x1: minx as u32,
                y1: miny as u32,
                x2: (maxx + 1) as u32,
                y2: (maxy + 1) as u32,
            });
        }
    }
    boxes
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
        let components = mask_components(mask);
        if components.is_empty() {
            return Err("mask is empty".into());
        }
        let mut result = image.to_rgb8();
        for (i, bbox) in components.iter().enumerate() {
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
                "inferred region {}/{}: {}x{} window at ({}, {})",
                i + 1,
                components.len(),
                WINDOW,
                WINDOW,
                ox,
                oy
            ));
        }
        Ok(result)
    }
}

fn num_cpus_hint() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

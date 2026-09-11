use std::path::Path;

use image::DynamicImage;
use ort::session::{builder::GraphOptimizationLevel, Session};

const WINDOW: u32 = 512;

pub struct Lama {
    session: Session,
}

#[derive(Clone)]
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

/// 对一个水印连通块做 crop 推理（对齐 iopaint 的 CROP 策略）：
/// bbox 向外扩 128px 上下文，crop ≤512 时原分辨率 pad 到 512 推理；
/// 更大时等比缩放到 512 内推理后还原写回——大水印整体修复，无 tile 接缝。
fn infer_crop(
    &mut self,
    mask: &image::GrayImage,
    bbox: &BBox,
    label: &str,
    log: &dyn Fn(&str),
    out: &mut image::RgbImage,
) -> Result<(), String> {
    const MARGIN: i64 = 128;
    const WIN: i64 = WINDOW as i64;
    let image = out.clone();
    let (iw, ih) = (image.width() as i64, image.height() as i64);

    let (l, t, sw, sh) = crop_box(bbox, iw, ih, MARGIN);
    if sw == 0 || sh == 0 {
        return Err("empty crop region".into());
    }

    // 等比缩放到 512 内
    let long_side = sw.max(sh) as f64;
    let scale = if long_side > WIN as f64 { WIN as f64 / long_side } else { 1.0 };
    let (rw, rh) = if scale < 1.0 {
        (((sw as f64 * scale).round() as u32).max(1), ((sh as f64 * scale).round() as u32).max(1))
    } else {
        (sw, sh)
    };
    log(&format!(
        "{}: bbox {}x{} -> crop {}x{} at ({}, {}){}",
        label,
        bbox.x2 - bbox.x1,
        bbox.y2 - bbox.y1,
        sw, sh, l, t,
        if scale < 1.0 { format!(", scaled to {}x{}", rw, rh) } else { String::new() }
    ));

    let crop_img = image::imageops::crop_imm(&image, l as u32, t as u32, sw, sh).to_image();
    let crop_mask = image::imageops::crop_imm(mask, l as u32, t as u32, sw, sh).to_image();
    let crop_img = if scale < 1.0 {
        image::imageops::resize(&crop_img, rw, rh, image::imageops::FilterType::Lanczos3)
    } else {
        crop_img
    };
    let mask_for_infer = if scale < 1.0 {
        let soft = image::imageops::resize(&crop_mask, rw, rh, image::imageops::FilterType::Lanczos3);
        let mut bin = soft.clone();
        for p in bin.pixels_mut() {
            p.0[0] = if p.0[0] > 64 { 255 } else { 0 };
        }
        bin
    } else {
        crop_mask.clone()
    };

    // 内容居中贴进 512 窗口。pad 区用图像均值色常数填充：
    // 反射 pad 会把贴近 crop 边缘的水印文字镜像进模型上下文，导致模型照字形延续产生残影。
    let dx = ((WIN - rw as i64) / 2).max(0) as u32;
    let dy = ((WIN - rh as i64) / 2).max(0) as u32;
    let mut sum = [0f64; 3];
    for p in crop_img.pixels() {
        for c in 0..3 {
            sum[c] += p.0[c] as f64;
        }
    }
    let n = (rw as f64 * rh as f64).max(1.0);
    let mean = image::Rgb([
        (sum[0] / n).round() as u8,
        (sum[1] / n).round() as u8,
        (sum[2] / n).round() as u8,
    ]);
    let mut win_img = image::RgbImage::from_pixel(WINDOW, WINDOW, mean);
    for y in 0..rh {
        for x in 0..rw {
            win_img.put_pixel(x + dx, y + dy, *crop_img.get_pixel(x, y));
        }
    }
    let mut win_mask = image::GrayImage::from_pixel(WINDOW, WINDOW, image::Luma([0]));
    for y in 0..rh {
        for x in 0..rw {
            let v = mask_for_infer.get_pixel(x, y).0[0];
            if v > 0 {
                win_mask.put_pixel(x + dx, y + dy, image::Luma([255]));
            }
        }
    }

    // CHW float 输入
    let plane = (WINDOW * WINDOW) as usize;
    let mut chw = vec![0f32; 3 * plane];
    for y in 0..WINDOW {
        for x in 0..WINDOW {
            let p = win_img.get_pixel(x, y);
            let base = (y * WINDOW + x) as usize;
            for c in 0..3 {
                chw[c * plane + base] = p.0[c] as f32 / 255.0;
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
    let data: &[f32] = output.1;
    if data.len() < 3 * plane {
        return Err(format!("unexpected model output size {}", data.len()));
    }
    let _ = output.0;

    // 取回内容区，必要时缩放回原 crop 尺寸
    let mut fixed = image::RgbImage::new(rw, rh);
    for y in 0..rh {
        for x in 0..rw {
            let base = ((y + dy) * WINDOW + x + dx) as usize;
            fixed.put_pixel(
                x,
                y,
                image::Rgb([
                    data[base].clamp(0.0, 255.0) as u8,
                    data[plane + base].clamp(0.0, 255.0) as u8,
                    data[2 * plane + base].clamp(0.0, 255.0) as u8,
                ]),
            );
        }
    }
    let fixed = if scale < 1.0 {
        image::imageops::resize(&fixed, sw, sh, image::imageops::FilterType::Lanczos3)
    } else {
        fixed
    };

    // 只写回 mask 像素（crop 坐标 -> 全图坐标）
    for y in 0..sh {
        for x in 0..sw {
            if crop_mask.get_pixel(x, y).0[0] == 0 {
                continue;
            }
            out.put_pixel((l as u32) + x, (t as u32) + y, *fixed.get_pixel(x, y));
        }
    }
    Ok(())
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
    let components = mask_components(mask);
    if components.is_empty() {
        return Err("mask is empty".into());
    }
    let mut result = image.to_rgb8();
    let total = components.len();
    for (i, bbox) in components.clone().iter().enumerate() {
        let label = format!("region {}/{}", i + 1, total);
        self.infer_crop(mask, bbox, &label, log, &mut result)?;
    }
    Ok(result)
}
}


/// 计算遮罩连通块的 crop 区域：bbox 中心扩展 margin，clamp 图界，贴边时向另一侧补偿（对齐 iopaint _crop_box）。
/// 返回 (l, t, w, h)；空区域返回 (0,0,0,0)。
fn crop_box(bbox: &BBox, iw: i64, ih: i64, margin: i64) -> (i64, i64, u32, u32) {
    let cx = (bbox.x1 as i64 + bbox.x2 as i64) / 2;
    let cy = (bbox.y1 as i64 + bbox.y2 as i64) / 2;
    let cw = (bbox.x2 - bbox.x1) as i64 + margin * 2;
    let ch = (bbox.y2 - bbox.y1) as i64 + margin * 2;
    let mut l = cx - cw / 2;
    let mut r = cx + cw / 2;
    let mut t = cy - ch / 2;
    let mut b = cy + ch / 2;
    if l < 0 { r += -l; l = 0; }
    if r > iw { l -= r - iw; r = iw; }
    if t < 0 { b += -t; t = 0; }
    if b > ih { t -= b - ih; b = ih; }
    l = l.clamp(0, iw);
    r = r.clamp(0, iw);
    t = t.clamp(0, ih);
    b = b.clamp(0, ih);
    if r <= l || b <= t {
        return (0, 0, 0, 0);
    }
    (l, t, (r - l) as u32, (b - t) as u32)
}

fn num_cpus_hint() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_box_margin_and_edge_compensation() {
        let bbox = |x1, y1, x2, y2| BBox { x1, y1, x2, y2 };
        // 居中：四边各留 margin
        assert_eq!(crop_box(&bbox(100, 100, 200, 200), 1000, 1000, 128), (0, 0, 356, 356));
        // 右下贴边：向左上补偿，保持完整 margin 尺寸
        let (l, t, w, h) = crop_box(&bbox(800, 900, 990, 995), 1000, 1000, 128);
        assert_eq!((l + w as i64, t + h as i64), (1000, 1000));
        assert!(w >= (990 - 800) as u32 + 128 && h >= (995 - 900) as u32 + 128);
        // bbox 比图还大：裁成整图
        assert_eq!(crop_box(&bbox(10, 10, 990, 990), 500, 400, 128), (0, 0, 500, 400));
    }
}

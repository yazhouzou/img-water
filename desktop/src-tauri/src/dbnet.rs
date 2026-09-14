//! OCR 文字检测（DBNet/PP-OCRv4 det）：--any-position 模式的一级检测器。
//! 以文字为训练目标，天然过滤雪点/花瓣/纹理误检，对彩色字、低对比字、
//! 复杂照片背景泛化——传统扫描确认不可分的场景（真实彩色照片彩色字、
//! 雪景白字）由它解决。模型 4.7MB 内嵌进二进制（CLI 与打包 App 均可用），
//! onnxruntime CPU 推理约 0.2s/张。

use std::sync::{Mutex, OnceLock};

use image::DynamicImage;
use ort::session::{builder::GraphOptimizationLevel, Session};

const MODEL_BYTES: &[u8] = include_bytes!("../../../tools/models/ch_pp-ocrv4_det.onnx");
const PROB_THRESHOLD: f32 = 0.3;
/// 概率图 0.3 截断使检出框小于字面，按概率图字高 unclip 外扩补齐；
/// 外扩不足会留字两端残迹（photo:2__red 实测），1.0 实测覆盖完整。
const UNCLIP: f32 = 1.0;

fn session() -> Result<&'static Mutex<Session>, String> {
    static SESSION: OnceLock<Mutex<Session>> = OnceLock::new();
    if let Some(s) = SESSION.get() {
        return Ok(s);
    }
    let session = Session::builder()
        .map_err(|e| e.to_string())?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| e.to_string())?
        .with_intra_threads(2)
        .map_err(|e| e.to_string())?
        .commit_from_memory(MODEL_BYTES)
        .map_err(|e| format!("加载 OCR 检测模型失败: {}", e))?;
    let _ = SESSION.set(Mutex::new(session));
    Ok(SESSION.get().expect("session just set"))
}

/// 返回原图坐标系的文本框（含 unclip 外扩），仅供 any_position 管线当整框遮罩用。
pub fn detect(image: &DynamicImage) -> Result<Vec<(i64, i64, i64, i64)>, String> {
    let sess = session()?;
    let rgb = image.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let long_side = w.max(h) as f64;
    let ratio = (960.0 / long_side).min(1.0);
    let rw = ((w as f64 * ratio).round() as usize).max(32) / 32 * 32;
    let rh = ((h as f64 * ratio).round() as usize).max(32) / 32 * 32;
    let rw = rw.max(32);
    let rh = rh.max(32);
    let resized = image::imageops::resize(&rgb, rw as u32, rh as u32, image::imageops::FilterType::Lanczos3);

    // PaddleOCR 预处理：RGB→BGR，/255，mean/std（与 Python 端 _detect_dbnet 一致）
    let mean = [0.485f32, 0.456, 0.406];
    let std = [0.229f32, 0.224, 0.225];
    let plane = rw * rh;
    let mut chw = vec![0f32; 3 * plane];
    for y in 0..rh {
        for x in 0..rw {
            let p = resized.get_pixel(x as u32, y as u32);
            let base = y * rw + x;
            // RGB → BGR：通道 0 = 蓝
            chw[base] = (p.0[2] as f32 / 255.0 - mean[0]) / std[0];
            chw[plane + base] = (p.0[1] as f32 / 255.0 - mean[1]) / std[1];
            chw[2 * plane + base] = (p.0[0] as f32 / 255.0 - mean[2]) / std[2];
        }
    }

    let mut guard = sess.lock().map_err(|e| e.to_string())?;
    let outputs = guard
        .run(
            ort::inputs![
                "x" => ort::value::Tensor::from_array((vec![1i64, 3, rh as i64, rw as i64], chw))
                    .map_err(|e| e.to_string())?,
            ],
        )
        .map_err(|e| format!("OCR 检测推理失败: {}", e))?;
    let output = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| e.to_string())?;
    let data: &[f32] = output.1;
    if data.len() != plane {
        return Err(format!("unexpected OCR output size {}", data.len()));
    }

    // 阈值化 + 8 连通域包围盒（概率图空间）
    let active = |x: usize, y: usize| data[y * rw + x] > PROB_THRESHOLD;
    let mut visited = vec![false; plane];
    let mut boxes = Vec::new();
    for sy in 0..rh {
        for sx in 0..rw {
            let idx = sy * rw + sx;
            if visited[idx] || !active(sx, sy) {
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
                        if !visited[ni] && active(nx as usize, ny as usize) {
                            visited[ni] = true;
                            stack.push(ni);
                        }
                    }
                }
            }
            if area < 200 {
                continue;
            }
            let cw = maxx - minx + 1;
            let ch = maxy - miny + 1;
            if cw <= ch {
                continue;
            }
            // unclip 外扩：pad 按概率图字高算，映射回原图前先叠加（与 Python 端一致）
            let pad = ((ch as f32 * UNCLIP) as i64).max(6);
            let gx1 = ((minx as f64 / rw as f64 * w as f64).round() as i64 - pad).max(0);
            let gy1 = ((miny as f64 / rh as f64 * h as f64).round() as i64 - pad).max(0);
            let gx2 = (((maxx + 1) as f64 / rw as f64 * w as f64).round() as i64 + pad).min(w as i64 - 1);
            let gy2 = (((maxy + 1) as f64 / rh as f64 * h as f64).round() as i64 + pad).min(h as i64 - 1);
            if gx2 - gx1 < 30 || gy2 - gy1 < 12 {
                continue;
            }
            boxes.push((gx1, gy1, gx2, gy2));
        }
    }
    Ok(boxes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> DynamicImage {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        image::open(path).expect("fixture image")
    }

    fn covers_text(boxes: &[(i64, i64, i64, i64)]) -> bool {
        // PIL d.text((200,180)) 实际字形约在 (200,182)-(458,221)，判据留少量余量
        boxes.iter().any(|&(x1, y1, x2, y2)| x1 <= 210 && x2 >= 440 && y1 <= 190 && y2 >= 215)
    }

    #[test]
    fn dbnet_detects_text_on_dark_background() {
        let boxes = detect(&fixture("dbnet-dark.png")).expect("dbnet infer");
        assert!(covers_text(&boxes), "expected a box covering the text, got {:?}", boxes);
    }

    #[test]
    fn dbnet_detects_red_text_on_light_background() {
        let boxes = detect(&fixture("dbnet-light-red.png")).expect("dbnet infer");
        assert!(covers_text(&boxes), "expected a box covering the red text, got {:?}", boxes);
    }

    #[test]
    fn dbnet_ignores_clean_image() {
        let base = image::RgbImage::from_pixel(640, 400, image::Rgb([30, 30, 30]));
        let boxes = detect(&DynamicImage::ImageRgb8(base)).expect("dbnet infer");
        assert!(boxes.is_empty(), "clean image should have no boxes, got {:?}", boxes);
    }
}


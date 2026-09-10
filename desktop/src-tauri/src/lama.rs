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

fn reflect_pad_rgb(img: &image::RgbImage, win: u32, ox: i64, oy: i64) -> image::RgbImage {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let mut out = image::RgbImage::new(win, win);
    for y in 0..win as i64 {
        for x in 0..win as i64 {
            let sx = reflect_coord(x + ox, w);
            let sy = reflect_coord(y + oy, h);
            out.put_pixel(x as u32, y as u32, img.get_pixel(sx as u32, sy as u32).clone());
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

    /// 对一个 512 窗口推理并写回 `out`。
    /// 图像源取自 `out` 的当前内容（已含此前窗口的修复结果，级联保证接缝连续）；
    /// `write_rect` 为 None 时写回窗口内全部 mask 像素，Some(tile) 时只写回 tile 内的 mask 像素。
    fn infer_window(
        &mut self,
        mask: &image::GrayImage,
        ox: i64,
        oy: i64,
        write_rect: Option<Rect>,
        out: &mut image::RgbImage,
    ) -> Result<(), String> {
        let current = out.clone();
        let (width, height) = (current.width() as i64, current.height() as i64);
        let window = WINDOW as i64;
        let win_img = reflect_pad_rgb(&current, WINDOW, ox, oy);
        let mut win_mask = image::GrayImage::new(WINDOW, WINDOW);
        let oxu = ox as u32;
        let oyu = oy as u32;
        let y_end = (oyu + WINDOW).min(height as u32);
        let x_end = (oxu + WINDOW).min(width as u32);
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

        // 写回范围：窗口 ∩ 图像 ∩ (write_rect 或全部)
        let wx1 = ox.max(0);
        let wy1 = oy.max(0);
        let wx2 = (ox + window).min(width);
        let wy2 = (oy + window).min(height);
        let (rx1, ry1, rx2, ry2) = match write_rect {
            Some(t) => (wx1.max(t.x1), wy1.max(t.y1), wx2.min(t.x2), wy2.min(t.y2)),
            None => (wx1, wy1, wx2, wy2),
        };
        if rx1 >= rx2 || ry1 >= ry2 {
            return Ok(());
        }
        let plane = WINDOW as usize * WINDOW as usize;
        let m = WINDOW as i64;
        for sy in ry1..ry2 {
            for sx in rx1..rx2 {
                if mask.get_pixel(sx as u32, sy as u32).0[0] == 0 {
                    continue;
                }
                let x = sx - ox;
                let y = sy - oy;
                let base = (y * m + x) as usize;
                let pixel = image::Rgb([
                    data[base].clamp(0.0, 255.0) as u8,
                    data[plane + base].clamp(0.0, 255.0) as u8,
                    data[2 * plane + base].clamp(0.0, 255.0) as u8,
                ]);
                out.put_pixel(sx as u32, sy as u32, pixel);
            }
        }
        Ok(())
    }
}

impl Lama {
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
            let bw = (bbox.x2 - bbox.x1) as i64;
            let bh = (bbox.y2 - bbox.y1) as i64;
            let window = WINDOW as i64;
            if bw <= MAX_BBOX as i64 && bh <= MAX_BBOX as i64 && bw <= window && bh <= window {
                // 单窗口：遮罩居中，上下文最优
                let ox = ((bbox.x1 + bbox.x2) as i64 / 2 - window / 2)
                    .clamp(0, width as i64 - window)
                    .max(0);
                let oy = ((bbox.y1 + bbox.y2) as i64 / 2 - window / 2)
                    .clamp(0, height as i64 - window)
                    .max(0);
                self.infer_window(mask, ox, oy, None, &mut result)?;
                log(&format!(
                    "inferred region {}/{}: {}x{} window at ({}, {})",
                    i + 1,
                    components.len(),
                    WINDOW,
                    WINDOW,
                    ox,
                    oy
                ));
            } else {
                // 大遮罩：滑动窗口 tile 推理，窗口间重叠 96px 提供接缝上下文，
                // 每个窗口只写回自己负责的 tile 区域（级联：后续窗口能看到前面修复结果）
                const OVERLAP: i64 = 96;
                let step = window - OVERLAP;
                let xs = window_starts(bbox.x1 as i64, bbox.x2 as i64, window, step, width as i64);
                let ys = window_starts(bbox.y1 as i64, bbox.y2 as i64, window, step, height as i64);
                let total = xs.len() * ys.len();
                log(&format!(
                    "region {}/{}: {}x{} exceeds window, tiling into {} window(s)",
                    i + 1,
                    components.len(),
                    bw,
                    bh,
                    total
                ));
                let mut done = 0;
                for (yi, &oy) in ys.iter().enumerate() {
                    for (xi, &ox) in xs.iter().enumerate() {
                        // 负责 tile：到下一窗口起点为止（最后一个窗口到 bbox 边缘）
                        let tx1 = bbox.x1 as i64;
                        let ty1 = bbox.y1 as i64;
                        let tx2 = xs.get(xi + 1).map(|&n| n.min(bbox.x2 as i64)).unwrap_or(bbox.x2 as i64);
                        let ty2 = ys.get(yi + 1).map(|&n| n.min(bbox.y2 as i64)).unwrap_or(bbox.y2 as i64);
                        let tile = Rect { x1: tx1, y1: ty1, x2: tx2, y2: ty2 };
                        self.infer_window(mask, ox, oy, Some(tile), &mut result)?;
                        done += 1;
                        log(&format!(
                            "tile {}/{}: window at ({}, {})",
                            done, total, ox, oy
                        ));
                    }
                }
            }
        }
        Ok(result)
    }
}

struct Rect {
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
}

/// 覆盖 [lo, hi) 区域的窗口起点序列；起点必须让窗口完整落在 [0, bound_hi] 内。
/// 区域小于窗口时返回单个居中起点，否则步进覆盖、最后一窗右对齐区域末尾。
fn window_starts(lo: i64, hi: i64, window: i64, step: i64, bound_hi: i64) -> Vec<i64> {
    let max_start = (bound_hi - window).max(0);
    if hi - lo <= window {
        let center = (lo + hi) / 2 - window / 2;
        return vec![center.clamp(0, max_start)];
    }
    let mut starts = Vec::new();
    let mut s = lo.clamp(0, max_start);
    loop {
        let covered = s + window;
        if covered >= hi {
            starts.push(s);
            break;
        }
        let next = (hi - window).min(s + step);
        if next <= s {
            starts.push(s);
            break;
        }
        starts.push(s);
        s = next;
    }
    starts
}

fn num_cpus_hint() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

//! 原图元数据保真：EXIF 方向 / EXIF / ICC 色彩配置。
//!
//! 落盘是"整图解码 → 重编码"，会把 EXIF（拍摄时间/GPS/机型）与 ICC 色彩配置一起丢掉；
//! 更严重的是解出来的像素是**存储方向**，而用户看到的是**视觉方向**——手机竖拍图
//! （存储为横向 + EXIF Orientation=6）的水印在视觉右下角、却不在存储右下角，掩码直接
//! 找不到或找偏。这里做两件事：
//!
//! 1. `bake_orientation_into`：prepare 阶段把方向"烤"进工作副本 `source/`，后续遮罩/
//!    修复/复查全在视觉方向上进行；`original-watermark-backup/` 仍是逐字节原图，
//!    「恢复原图」不受影响。
//! 2. `CarryMeta`：落盘时把原图的 EXIF / ICC 段**原样字节**搬回新文件（不做重压缩、
//!    不重算校验和），并把 EXIF 里的方向标记清零——方向已经烤进像素了，不清零看图
//!    软件会再转一次。
//!
//! 只处理 JPEG（APP1/APP2）与 PNG（iCCP/eXIf/色彩块）；WebP 是 RIFF 容器，改写风险大，
//! 暂不搬（只做方向烘焙）。

use std::fs;
use std::path::Path;

use image::{metadata::Orientation, ImageDecoder};

const JPEG_SOI: [u8; 2] = [0xff, 0xd8];
const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const EXIF_HEADER: &[u8] = b"Exif\0\0";
const ICC_HEADER: &[u8] = b"ICC_PROFILE\0";

/// 原图 EXIF 方向；无 EXIF、无该标记或解码失败时 None（等价于"无需转正"）。
pub fn read_orientation(path: &Path) -> Option<Orientation> {
    let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    decoder.orientation().ok()
}

/// 按**内容**识别格式解码（不信任扩展名）：工作副本可能已被上游改过容器格式。
fn open_any(path: &Path) -> Result<image::DynamicImage, String> {
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?;
    let reader = reader.with_guessed_format().map_err(|e| e.to_string())?;
    reader.decode().map_err(|e| e.to_string())
}

/// 按原图 EXIF 方向把工作副本转正，返回是否真的转了。
/// 只改工作副本（`source/`），备份保持逐字节原图。
pub fn bake_orientation_into(orig: &Path, copy: &Path) -> Result<bool, String> {
    match read_orientation(orig) {
        None | Some(Orientation::NoTransforms) => Ok(false),
        Some(o) => {
            let mut img =
                open_any(copy).map_err(|e| format!("打开图片失败 {}: {}", copy.display(), e))?;
            img.apply_orientation(o);
            // 工作副本一律写 PNG（无损），下游按内容识别格式、不看扩展名
            img.save_with_format(copy, image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            Ok(true)
        }
    }
}

/// JPEG 段表：(marker, 段起始, 段总长)。段含前导 0xFF 与长度字段本身。
fn jpeg_segments(bytes: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    if bytes.len() < 4 || bytes[..2] != JPEG_SOI {
        return out;
    }
    let mut i = 2usize;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xff {
            break;
        }
        let marker = bytes[i + 1];
        if marker == 0xff {
            i += 1; // 填充字节
            continue;
        }
        // SOS 之后是熵编码数据；EOI 结束
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            i += 2; // 无长度字段的独立标记
            continue;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            break;
        }
        out.push((marker, i, len + 2));
        i += 2 + len;
    }
    out
}

/// PNG 块表：(四字节类型, 块起始, 块总长 = 4 长度 + 4 类型 + 数据 + 4 CRC)。
fn png_chunks(bytes: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    if bytes.len() < 8 || bytes[..8] != PNG_SIG {
        return out;
    }
    let mut i = 8usize;
    while i + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let total = 12 + len;
        if i + total > bytes.len() {
            break;
        }
        let mut ty = [0u8; 4];
        ty.copy_from_slice(&bytes[i + 4..i + 8]);
        let is_end = &ty == b"IEND";
        out.push((ty, i, total));
        if is_end {
            break;
        }
        i += total;
    }
    out
}

/// 把 EXIF 块里的方向标记写成"无变换"；EXIF 结构异常时返回 false（保持原样）。
fn clear_exif_orientation(tiff: &mut [u8]) -> bool {
    Orientation::remove_from_exif_chunk(tiff).is_some()
}

/// 需要搬回结果的原图元数据段/块（保留原始字节，不重新编码）。
#[derive(Default, Clone)]
pub struct CarryMeta {
    jpeg_exif: Option<Vec<u8>>,
    jpeg_icc: Vec<Vec<u8>>,
    png_chunks: Vec<Vec<u8>>,
}

impl CarryMeta {
    /// 从原图（未经改动的文件）字节里取出要搬回的段/块。非 JPEG/PNG 返回空。
    pub fn read(path: &Path) -> Self {
        let mut meta = Self::default();
        let Ok(bytes) = fs::read(path) else {
            return meta;
        };
        if bytes.starts_with(&JPEG_SOI) {
            for (marker, start, len) in jpeg_segments(&bytes) {
                let seg = &bytes[start..start + len];
                if seg.len() < 4 {
                    continue;
                }
                if marker == 0xe1 && seg[4..].starts_with(EXIF_HEADER) {
                    if meta.jpeg_exif.is_none() {
                        meta.jpeg_exif = Some(seg.to_vec());
                    }
                } else if marker == 0xe2 && seg[4..].starts_with(ICC_HEADER) {
                    meta.jpeg_icc.push(seg.to_vec());
                }
            }
        } else if bytes.starts_with(&PNG_SIG) {
            for (ty, start, len) in png_chunks(&bytes) {
                if matches!(&ty, b"iCCP" | b"eXIf" | b"sRGB" | b"gAMA" | b"cHRM") {
                    meta.png_chunks.push(bytes[start..start + len].to_vec());
                }
            }
        }
        meta
    }

    pub fn is_empty(&self) -> bool {
        self.jpeg_exif.is_none() && self.jpeg_icc.is_empty() && self.png_chunks.is_empty()
    }

    /// 像素已按 EXIF 方向转正：把方向标记清零，否则看图软件会再转一次。
    pub fn clear_orientation(&mut self) {
        if let Some(exif) = self.jpeg_exif.as_mut() {
            if exif.len() > 10 {
                clear_exif_orientation(&mut exif[10..]);
            }
        }
        for chunk in self.png_chunks.iter_mut() {
            if chunk.len() > 12 && &chunk[4..8] == b"eXIf" {
                let end = chunk.len() - 4;
                clear_exif_orientation(&mut chunk[8..end]);
            }
        }
    }

    /// 把 EXIF/ICC 段插到 JPEG 的 SOI 之后（EXIF APP1 应在最前，ICC 紧随其后）。
    pub fn splice_jpeg(&self, encoded: Vec<u8>) -> Vec<u8> {
        let mut extra: Vec<u8> = Vec::new();
        if let Some(exif) = &self.jpeg_exif {
            extra.extend_from_slice(exif);
        }
        for seg in &self.jpeg_icc {
            extra.extend_from_slice(seg);
        }
        if extra.is_empty() || encoded.len() < 2 || !encoded.starts_with(&JPEG_SOI) {
            return encoded;
        }
        let mut out = Vec::with_capacity(encoded.len() + extra.len());
        out.extend_from_slice(&encoded[..2]);
        out.extend_from_slice(&extra);
        out.extend_from_slice(&encoded[2..]);
        out
    }

    /// 把元数据块插到 PNG 的 IHDR 之后（须在 IDAT 之前）。
    pub fn splice_png(&self, encoded: Vec<u8>) -> Vec<u8> {
        if self.png_chunks.is_empty() {
            return encoded;
        }
        let chunks = png_chunks(&encoded);
        let Some((_, _, ihdr_total)) = chunks.first().copied() else {
            return encoded;
        };
        if chunks[0].0 != *b"IHDR" {
            return encoded;
        }
        let at = 8 + ihdr_total;
        if at > encoded.len() {
            return encoded;
        }
        let mut out = Vec::with_capacity(encoded.len() + 64);
        out.extend_from_slice(&encoded[..at]);
        for c in &self.png_chunks {
            out.extend_from_slice(c);
        }
        out.extend_from_slice(&encoded[at..]);
        out
    }
}

/// 测试用：合成带 EXIF（含方向标记）与 ICC 的 JPEG。APP1 EXIF 用最小 little-endian TIFF。
#[cfg(test)]
pub mod testutil {
    use super::*;
    use image::RgbImage;

    pub fn write_jpeg_with_meta(path: &Path, w: u32, h: u32, exif_orientation: u16, icc: &[u8]) {
        let img = RgbImage::from_pixel(w, h, image::Rgb([10, 20, 30]));
        let mut buf = Vec::new();
        img.write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 95))
            .unwrap();

        // TIFF: II*0 <ifd_offset=8> | 1 entry | tag=0x0112 type=3 count=1 value=orientation | next=0
        let mut tiff: Vec<u8> = Vec::new();
        tiff.extend_from_slice(&[0x49, 0x49, 42, 0]);
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&exif_orientation.to_le_bytes());
        tiff.extend_from_slice(&0u16.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());

        let mut extra: Vec<u8> = Vec::new();
        let mut app1: Vec<u8> = vec![0xff, 0xe1];
        let payload_len = 2 + EXIF_HEADER.len() + tiff.len();
        app1.extend_from_slice(&(payload_len as u16).to_be_bytes());
        app1.extend_from_slice(EXIF_HEADER);
        app1.extend_from_slice(&tiff);
        extra.extend_from_slice(&app1);
        if !icc.is_empty() {
            let mut payload: Vec<u8> = Vec::new();
            payload.extend_from_slice(ICC_HEADER);
            payload.push(1);
            payload.push(1);
            payload.extend_from_slice(icc);
            let mut app2: Vec<u8> = vec![0xff, 0xe2];
            app2.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            app2.extend_from_slice(&payload);
            extra.extend_from_slice(&app2);
        }

        let mut out = Vec::with_capacity(buf.len() + extra.len());
        out.extend_from_slice(&buf[..2]);
        out.extend_from_slice(&extra);
        out.extend_from_slice(&buf[2..]);
        fs::write(path, out).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, RgbImage};

    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dwm-meta-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_jpeg_with_meta(path: &Path, exif_orientation: u16, icc: &[u8]) {
        testutil::write_jpeg_with_meta(path, 8, 8, exif_orientation, icc);
    }

    #[test]
    fn jpeg_meta_round_trip_keeps_exif_and_icc() {
        let dir = tmp("jpeg");
        let src = dir.join("a.jpg");
        let icc = b"fake-icc-profile-bytes".to_vec();
        make_jpeg_with_meta(&src, 6, &icc);

        assert_eq!(read_orientation(&src), Some(Orientation::Rotate90));

        let meta = CarryMeta::read(&src);
        assert!(!meta.is_empty(), "应读出 EXIF 与 ICC");
        assert_eq!(meta.jpeg_icc.len(), 1, "ICC 分片应被完整取出");

        // 方向已烤进像素 → 落盘前必须清零，否则会被再转一次
        let mut cleared = meta.clone();
        cleared.clear_orientation();
        let exif = cleared.jpeg_exif.clone().unwrap();
        assert_eq!(
            Orientation::from_exif_chunk(&exif[10..]),
            Some(Orientation::NoTransforms)
        );
        // 原始 EXIF 不能被就地改坏（clear 只作用于副本）
        assert_eq!(
            Orientation::from_exif_chunk(&meta.jpeg_exif.clone().unwrap()[10..]),
            Some(Orientation::Rotate90)
        );

        // 段搬回：新文件仍能被解出方向 1，且 ICC 字节在位
        let img = RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]));
        let mut buf = Vec::new();
        img.write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 95))
            .unwrap();
        let merged = cleared.splice_jpeg(buf);
        assert!(merged.starts_with(&JPEG_SOI));
        let out = dir.join("out.jpg");
        fs::write(&out, &merged).unwrap();
        assert_eq!(read_orientation(&out), Some(Orientation::NoTransforms));
        assert!(
            merged.windows(icc.len()).any(|w| w == icc.as_slice()),
            "ICC 数据必须原样保留"
        );
        // 新文件仍是合法 JPEG（能被解码）
        assert_eq!(image::open(&out).unwrap().dimensions(), (8, 8));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn png_color_chunks_are_carried_over() {
        let dir = tmp("png");
        let src = dir.join("a.png");
        // 用 png crate 直接造源 PNG：image 的 PNG 编码器不写色彩块，测不到搬运逻辑
        {
            let file = fs::File::create(&src).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 4, 4);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
            let mut writer = enc.write_header().unwrap();
            writer.write_image_data(&[0u8; 4 * 4 * 3]).unwrap();
        }

        let meta = CarryMeta::read(&src);
        assert!(!meta.is_empty(), "应取出 sRGB 等色彩块");
        let source_chunks = png_chunks(&fs::read(&src).unwrap());
        assert!(
            source_chunks.iter().any(|(t, _, _)| t == b"sRGB"),
            "源文件应含 sRGB 块"
        );

        let img = RgbImage::from_pixel(4, 4, image::Rgb([9, 9, 9]));
        let mut buf = Vec::new();
        img.write_with_encoder(image::codecs::png::PngEncoder::new(&mut buf))
            .unwrap();
        let merged = meta.splice_png(buf);
        let out = dir.join("out.png");
        fs::write(&out, &merged).unwrap();
        // 解码仍正常；且搬过来的块紧跟 IHDR、在 IDAT 之前
        assert_eq!(image::open(&out).unwrap().dimensions(), (4, 4));
        let chunks = png_chunks(&merged);
        assert_eq!(chunks[0].0, *b"IHDR");
        assert_eq!(chunks[1].0, *b"sRGB", "色彩块必须紧跟 IHDR");
        assert!(
            chunks.iter().any(|(t, _, _)| t == b"IDAT"),
            "IDAT 应仍在末尾"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bake_orientation_rotates_work_copy_only() {
        let dir = tmp("bake");
        let src = dir.join("a.jpg");
        make_jpeg_with_meta(&src, 6, &[]);
        let copy = dir.join("work.png");
        fs::copy(&src, &copy).unwrap();
        let original_bytes = fs::read(&copy).unwrap();

        assert!(bake_orientation_into(&src, &copy).unwrap(), "方向 6 应该被烤进去");
        // 存储方向 8x8 → 视觉方向 8x8（正方形看不出），但至少必须真的重写了像素
        assert_ne!(fs::read(&copy).unwrap(), original_bytes);
        // 备份/原图不受影响
        assert_eq!(read_orientation(&src), Some(Orientation::Rotate90));

        // 方向为 1 时必须是 no-op（老路径逐字节不变）
        let plain = dir.join("plain.png");
        RgbImage::from_pixel(4, 2, image::Rgb([1, 1, 1]))
            .save(&plain)
            .unwrap();
        let before = fs::read(&plain).unwrap();
        let copy2 = dir.join("work2.png");
        fs::copy(&plain, &copy2).unwrap();
        assert!(!bake_orientation_into(&plain, &copy2).unwrap());
        assert_eq!(fs::read(&copy2).unwrap(), before);
        fs::remove_dir_all(&dir).unwrap();
    }
}

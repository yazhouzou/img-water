pub mod android_uri;
pub mod app;
pub mod fetch;
pub mod lama;
pub mod pipeline;

pub use app::run_tauri_app;

use std::path::PathBuf;
use std::sync::OnceLock;

pub static MODEL_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();
pub static WORKDIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

pub fn project_root() -> PathBuf {
    if !cfg!(debug_assertions) {
        if let Ok(exe) = std::env::current_exe() {
            let mut dir = exe.parent().map(|p| p.to_path_buf());
            for _ in 0..4 {
                if let Some(current) = dir {
                    if current.join("tools/remove_doubao_watermark.py").exists() {
                        return current;
                    }
                    dir = current.parent().map(|p| p.to_path_buf());
                }
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .expect("cannot resolve project root")
}

pub const MODEL_FILENAME: &str = "lama_fp32.onnx";
/// 按顺序尝试的模型下载源：国内镜像优先，失败后回退官方源。
pub const MODEL_URLS: &[&str] = &[
    "https://hf-mirror.com/Carve/LaMa-ONNX/resolve/main/lama_fp32.onnx",
    "https://huggingface.co/Carve/LaMa-ONNX/resolve/main/lama_fp32.onnx",
];

pub fn model_dir() -> PathBuf {
    if let Ok(path) = std::env::var("LAMA_ONNX_DIR") {
        return PathBuf::from(path);
    }
    if let Some(dir) = MODEL_DIR_OVERRIDE.get() {
        return dir.clone();
    }
    project_root().join(".models")
}

pub fn model_path() -> PathBuf {
    if let Ok(path) = std::env::var("LAMA_ONNX_PATH") {
        return PathBuf::from(path);
    }
    model_dir().join(MODEL_FILENAME)
}

pub fn workdir() -> PathBuf {
    if let Ok(path) = std::env::var("DOUBAO_WATERMARK_WORKDIR") {
        return PathBuf::from(path);
    }
    if let Some(dir) = WORKDIR_OVERRIDE.get() {
        return dir.join("doubao-watermark-work");
    }
    if cfg!(windows) {
        std::env::temp_dir().join("doubao-watermark-work")
    } else {
        PathBuf::from("/tmp/doubao-watermark-work")
    }
}

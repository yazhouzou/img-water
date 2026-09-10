
use base64::Engine as _;
use crate::{fetch, pipeline, MODEL_URLS};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tauri::{AppHandle, Emitter, Manager, State};

const EVENT_LOG: &str = "pipeline-log";
const EVENT_EXIT: &str = "pipeline-exit";
const EVENT_MODEL_PROGRESS: &str = "model-progress";

#[derive(Default)]
struct AppStorage {
    target_root: std::sync::Mutex<Option<PathBuf>>,
    last_files: std::sync::Mutex<Vec<String>>,
    running: AtomicBool,
}

fn finish(app: &AppHandle, payload: serde_json::Value) {
    let _ = app.emit(EVENT_EXIT, payload);
    if let Some(state) = app.try_state::<AppStorage>() {
        state.running.store(false, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
struct EnvStatus {
    ready: bool,
    model_path: String,
    hint: String,
}

#[tauri::command]
fn env_status() -> EnvStatus {
    let path = crate::model_path();
    let ready = path.exists();
    EnvStatus {
        ready,
        model_path: path.display().to_string(),
        hint: if ready {
            String::new()
        } else {
            "修复模型未下载：点击“一键下载修复模型”在线获取（约 200MB，无需 Python）".into()
        },
    }
}

#[tauri::command]
fn setup_model(app: AppHandle, storage: State<'_, AppStorage>) -> Result<(), String> {
    if storage.running.swap(true, Ordering::SeqCst) {
        return Err("已有任务在运行中，请等待完成".into());
    }
    let dest = crate::model_path();
    if dest.exists() {
        finish(&app, serde_json::json!({ "code": 0, "success": true }));
        return Ok(());
    }
    let urls: Vec<String> = match std::env::var("LAMA_ONNX_URL") {
        Ok(url) => vec![url],
        Err(_) => MODEL_URLS.iter().map(|s| s.to_string()).collect(),
    };
    let app_handle = app.clone();
    thread::spawn(move || {
        let log = |line: &str| {
            let _ = app_handle.emit(EVENT_LOG, line);
        };
        log(&format!("[模型] 开始下载（{} 个下载源，多连接分段）", urls.len()));
        let app_progress = app_handle.clone();
        // 每 10% 记一条日志，避免刷屏
        let last_milestone = std::sync::atomic::AtomicU64::new(0);
        let result = fetch::download_model(&dest, &urls, &|done, total| {
            let _ = app_progress.emit(
                EVENT_MODEL_PROGRESS,
                serde_json::json!({ "done": done, "total": total }),
            );
            if total > 0 {
                let percent = done * 100 / total;
                let milestone = percent / 10;
                if milestone > last_milestone.load(Ordering::Relaxed) {
                    last_milestone.store(milestone, Ordering::Relaxed);
                    let _ = app_progress.emit(
                        EVENT_LOG,
                        format!(
                            "[模型] {:.1} / {:.1} MB ({}%)",
                            done as f64 / 1048576.0,
                            total as f64 / 1048576.0,
                            percent
                        ),
                    );
                }
            }
        });
        match result {
            Ok(()) => {
                log(&format!("[模型] 下载完成: {}", dest.display()));
                finish(
                    &app_handle,
                    serde_json::json!({ "code": 0, "success": true }),
                );
            }
            Err(err) => {
                log(&format!("[模型] 下载失败: {}", err));
                finish(
                    &app_handle,
                    serde_json::json!({ "code": -1, "success": false, "error": err }),
                );
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn list_pngs(root: String) -> Result<Vec<String>, String> {
    let dir = PathBuf::from(&root);
    if !dir.is_dir() {
        return Err(format!("文件夹不存在: {}", root));
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
        })
        .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    names.sort_by(|a, b| {
        let key = |name: &str| -> (u8, String, u64) {
            match name.strip_suffix(".png").unwrap_or(name).parse::<u64>() {
                Ok(number) => (0, String::new(), number),
                Err(_) => (1, name.to_string(), 0),
            }
        };
        let (ka, na, ia) = key(a);
        let (kb, nb, ib) = key(b);
        (ka, ia, na).cmp(&(kb, ib, nb))
    });
    Ok(names)
}

#[tauri::command]
fn run_pipeline(
    app: AppHandle,
    storage: State<'_, AppStorage>,
    root: String,
    files: Vec<String>,
    keep_work: bool,
) -> Result<(), String> {
    if storage.running.swap(true, Ordering::SeqCst) {
        return Err("已有任务在运行中，请等待完成".into());
    }
    let model_path = crate::model_path();
    if !model_path.exists() {
        storage.running.store(false, Ordering::SeqCst);
        return Err("修复模型未下载，请先点击“一键下载修复模型”".into());
    }

    {
        let mut last = storage.last_files.lock().map_err(|e| e.to_string())?;
        *last = files.clone();
        storage
            .target_root
            .lock()
            .map_err(|e| e.to_string())?
            .replace(PathBuf::from(&root));
    }

    let app_handle = app.clone();
    thread::spawn(move || {
        let options = pipeline::PipelineOptions {
            root: PathBuf::from(&root),
            files,
            keep_work,
            mask_box: None,
        };
        let log = |line: &str| {
            let _ = app_handle.emit(EVENT_LOG, line);
        };
        let result = pipeline::run(&options, &model_path, &log);
        match result {
            Ok(summary) => {
                let _ = app_handle.emit(
                    EVENT_LOG,
                    format!(
                        "candidate review: {}\nfinal review: {}",
                        summary.candidate_review.display(),
                        summary.final_review.display()
                    ),
                );
                finish(&app_handle, serde_json::json!({ "code": 0, "success": true }));
            }
            Err(err) => {
                let _ = app_handle.emit(EVENT_LOG, format!("[错误] {}", err));
                finish(
                    &app_handle,
                    serde_json::json!({ "code": -1, "success": false, "error": err }),
                );
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn cleanup_pipeline(storage: State<'_, AppStorage>) -> Result<(), String> {
    let root: PathBuf = storage
        .target_root
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("还没有处理记录")?;
    let files: Vec<String> = storage.last_files.lock().map_err(|e| e.to_string())?.clone();
    if files.is_empty() {
        return Err("没有可清理的记录".into());
    }
    let options = pipeline::PipelineOptions {
        root,
        files,
        keep_work: false,
        mask_box: None,
    };
    let names = pipeline::target_names(&options.root, &options.files)?;
    pipeline::cleanup(&options, &names)?;
    pipeline::cleanup_preserved()
}

#[tauri::command]
fn import_files(app: AppHandle, window: tauri::WebviewWindow, paths: Vec<String>) -> Result<ImportResult, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("imports");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let dir = base.join(format!("import-{}", stamp));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = Vec::new();
    for (index, raw) in paths.iter().enumerate() {
        let name = copy_one(&window, raw, &dir, index, &names)?;
        names.push(name);
    }
    if names.is_empty() {
        return Err("未选择图片".into());
    }
    Ok(ImportResult { dir: dir.display().to_string(), names })
}

fn copy_one(
    #[cfg_attr(not(target_os = "android"), allow(unused_variables))]
    window: &tauri::WebviewWindow,
    raw: &str,
    dir: &std::path::Path,
    index: usize,
    existing: &[String],
) -> Result<String, String> {
    #[cfg(target_os = "android")]
    if raw.starts_with("content://") {
        return crate::android_uri::copy_content_uri(window, raw, dir, index, existing);
    }
    let source = PathBuf::from(raw);
    if !source.is_file() {
        return Err(format!("文件不存在: {}", raw));
    }
    let mut name = source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("image-{}.png", index));
    if !name.to_lowercase().ends_with(".png") {
        name.push_str(".png");
    }
    let final_name = {
        let mut final_name = name.clone();
        let mut n = 1;
        while existing.iter().any(|e| e == &final_name) {
            let stem = name.trim_end_matches(".png");
            final_name = format!("{}-{}.png", stem, n);
            n += 1;
        }
        final_name
    };
    std::fs::copy(&source, dir.join(&final_name)).map_err(|e| format!("拷贝 {} 失败: {}", final_name, e))?;
    Ok(final_name)
}

#[derive(Serialize)]
struct ImportResult {
    dir: String,
    names: Vec<String>,
}

#[tauri::command]
fn read_image_base64(path: String) -> Result<String, String> {
    let file = PathBuf::from(&path);
    if !file.is_file() {
        return Err(format!("文件不存在: {}", path));
    }
    if file
        .extension()
        .map(|ext| !ext.eq_ignore_ascii_case("png"))
        .unwrap_or(true)
    {
        return Err("只支持预览 PNG 图片".into());
    }
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    if bytes.len() > 10 * 1024 * 1024 {
        return Err("图片过大，无法预览".into());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:image/png;base64,{}", encoded))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_tauri_app() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppStorage::default())
        .setup(|app| {
            if cfg!(target_os = "android") || cfg!(target_os = "ios") {
                let data_dir = app
                    .path()
                    .app_data_dir()
                    .expect("no app data dir");
                let cache_dir = app
                    .path()
                    .app_cache_dir()
                    .expect("no app cache dir");
                let _ = crate::MODEL_DIR_OVERRIDE.set(data_dir.join("models"));
                let _ = crate::WORKDIR_OVERRIDE.set(cache_dir);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            env_status,
            setup_model,
            list_pngs,
            import_files,
            run_pipeline,
            cleanup_pipeline,
            read_image_base64
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

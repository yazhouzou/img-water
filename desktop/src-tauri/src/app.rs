
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
const EVENT_PROGRESS: &str = "pipeline-progress";

use crate::i18n::{load as lang_of, tr};

#[derive(Default)]
struct AppStorage {
    target_root: std::sync::Mutex<Option<PathBuf>>,
    last_files: std::sync::Mutex<Vec<String>>,
    running: AtomicBool,
    cancel: AtomicBool,
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
            "Inpainting model not downloaded yet: click \"Download Model\" to fetch it online (~200MB, no Python required)".to_string()
        },
    }
}

#[tauri::command]
fn setup_model(app: AppHandle, storage: State<'_, AppStorage>, lang: Option<String>) -> Result<(), String> {
    let lang = lang_of(lang);
    if storage.running.swap(true, Ordering::SeqCst) {
        return Err(tr(&lang, "已有任务在运行中，请等待完成", "A task is already running, please wait"));
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
        log(&format!("[Model] Downloading ({} sources, multi-connection)", urls.len()));
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
                            "[Model] {:.1} / {:.1} MB ({}%)",
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
                log(&format!("[Model] Download complete: {}", dest.display()));
                finish(
                    &app_handle,
                    serde_json::json!({ "code": 0, "success": true }),
                );
            }
            Err(err) => {
                log(&format!("[Model] Download failed: {}", err));
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
fn list_pngs(root: String, lang: Option<String>) -> Result<Vec<String>, String> {
    let lang = lang_of(lang);
    let dir = PathBuf::from(&root);
    if !dir.is_dir() {
        return Err(tr(&lang, &format!("文件夹不存在: {}", root), &format!("Folder not found: {}", root)));
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .map(|name| pipeline::is_supported_image(&name.to_string_lossy()))
                    .unwrap_or(false)
        })
        .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    names.sort_by(|a, b| {
        let key = |name: &str| -> (u8, String, u64) {
            let stem = name
                .rsplit_once('.')
                .map(|(s, _)| s)
                .unwrap_or(name);
            match stem.parse::<u64>() {
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

/// 「精确模式」：用同一款水印的多张图自举学习一个逐像素档案。
///
/// 依次尝试 auto（无监督聚类，不要求纯色背景）与 batch（要求背景一致/纯色），
/// 取"学习后在原图上能重新定位"的那个（`profile_self_check`）；都过不了就报错。
/// 只有自检通过的档案才落盘——否则存下来也定位不到，等于白学。
#[tauri::command]
fn learn_watermark(
    root: String,
    files: Vec<String>,
    label: String,
    lang: Option<String>,
) -> Result<serde_json::Value, String> {
    let lang = lang_of(lang);
    if files.len() < 2 {
        return Err(tr(
            &lang,
            "请至少选中 2 张含同一款水印的图片",
            "Select at least 2 images with the same watermark",
        ));
    }
    let label = if label.trim().is_empty() {
        tr(&lang, "learned", "learned")
    } else {
        label.trim().to_string()
    };
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let _ = root;

    let frames: Vec<(String, PathBuf, Option<(i64, i64, i64, i64)>)> = paths
        .iter()
        .map(|p| {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            (name, p.clone(), None)
        })
        .collect();

    let mut attempts: Vec<serde_json::Value> = Vec::new();
    let mut best: Option<(crate::watermark_profiles::Profile, serde_json::Value, f64)> = None;
    for (mode, result) in [
        (
            "auto",
            crate::watermark_profiles::auto_discover(
                &frames,
                &label,
                [0f32, 0f32, 0f32],
                None,
                24,
                12.0,
                0.01,
                60.0,
            ),
        ),
        (
            "batch",
            crate::watermark_profiles::learn_from_batch(&paths, None, None, &label, 24, 12.0),
        ),
    ] {
        match result {
            Ok((Some(profile), report)) => {
                let (hit, total, mean) =
                    crate::watermark_profiles::profile_self_check(&profile, &paths);
                let needed = (total * 2).div_ceil(3);
                let ok = hit >= needed && hit > 0;
                attempts.push(serde_json::json!({
                    "mode": mode, "ok": ok, "located": hit, "samples": total, "mean_score": mean, "report": report,
                }));
                if ok && best.as_ref().map(|b| mean > b.2).unwrap_or(true) {
                    best = Some((profile, report, mean));
                }
            }
            Ok((None, report)) => attempts.push(serde_json::json!({
                "mode": mode, "ok": false, "report": report,
            })),
            Err(e) => attempts.push(serde_json::json!({
                "mode": mode, "ok": false, "error": e,
            })),
        }
    }

    let Some((profile, report, mean)) = best else {
        return Err(tr(
            &lang,
            "学习失败：这批图里没有学到可复用的水印（请选 3 张以上、水印清晰且位置一致的图；\
             若水印压在复杂风景上，改用框选后再处理）",
            "Learning failed: no reusable watermark found (pick 3+ images with a clear, \
             consistently placed watermark; for watermarks over busy photos, draw a box instead)",
        ));
    };
    let saved = crate::watermark_profiles::save_profile(&profile, true)?;
    Ok(serde_json::json!({
        "id": profile.id,
        "label": profile.label,
        "path": saved.display().to_string(),
        "mean_score": mean,
        "report": report,
        "attempts": attempts,
    }))
}

#[tauri::command]
fn list_watermarks() -> Result<serde_json::Value, String> {
    let list = crate::watermark_profiles::profile_summaries();
    serde_json::to_value(list).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_watermark(id: String, lang: Option<String>) -> Result<(), String> {
    let lang = lang_of(lang);
    crate::watermark_profiles::delete_profile(&id)
        .map_err(|e| tr(&lang, &format!("删除失败：{}", e), &format!("Delete failed: {}", e)))
}

#[tauri::command]
fn run_pipeline(
    app: AppHandle,
    storage: State<'_, AppStorage>,
    root: String,
    files: Vec<String>,
    keep_work: bool,
    overwrite_original: bool,
    mask_box: Option<Vec<i64>>,
    any_position: Option<bool>,
    refine: Option<bool>,
    profile_id: Option<String>,
    lang: Option<String>,
) -> Result<(), String> {
    let lang = lang_of(lang);
    if storage.running.swap(true, Ordering::SeqCst) {
        return Err(tr(&lang, "已有任务在运行中，请等待完成", "A task is already running, please wait"));
    }
    let model_path = crate::model_path();
    if !model_path.exists() {
        storage.running.store(false, Ordering::SeqCst);
        return Err(tr(
            &lang,
            "修复模型未下载，请先点击“一键下载修复模型”",
            "Inpainting model not downloaded yet; click \"Download Model\" first",
        ));
    }
    storage.cancel.store(false, Ordering::SeqCst);

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
        let mask = mask_box.and_then(|b| {
            (b.len() == 4).then(|| pipeline::MaskBox {
                x1: b[0],
                y1: b[1],
                x2: b[2],
                y2: b[3],
            })
        });
        let options = pipeline::PipelineOptions {
            root: PathBuf::from(&root),
            files,
            keep_work,
            mask_box: mask,
            any_position: any_position.unwrap_or(false),
            overwrite_original,
            use_profile: true,
            force: false,
            inverse: true,
            retry: true,
            refine: refine.unwrap_or(true),
            forced_profile: profile_id.clone(),
        };
        let log = |line: &str| {
            let _ = app_handle.emit(EVENT_LOG, line);
        };
        let progress_app = app_handle.clone();
        let progress = move |stage: &str, done: usize, total: usize, name: &str| {
            let _ = progress_app.emit(
                EVENT_PROGRESS,
                serde_json::json!({ "stage": stage, "done": done, "total": total, "name": name }),
            );
        };
        let cancel_flag = app_handle.clone();
        let is_cancelled = move || {
            cancel_flag
                .try_state::<AppStorage>()
                .map(|s| s.cancel.load(Ordering::SeqCst))
                .unwrap_or(false)
        };
        let result = pipeline::run(&options, &model_path, &log, &progress, &is_cancelled);
        match result {
            Ok(summary) => {
                let _ = app_handle.emit(
                    EVENT_LOG,
                    format!(
                        "source review: {}\nfinal review: {}",
                        summary.source_review.display(),
                        summary.final_review.display()
                    ),
                );
                finish(
                    &app_handle,
                    serde_json::json!({
                        "code": 0,
                        "success": true,
                        "outputDir": summary.output_dir.display().to_string(),
                        "processed": summary.processed,
                        "overwritten": options.overwrite_original,
                    }),
                );
            }
            Err(err) if err == pipeline::CANCELLED => {
                let _ = app_handle.emit(EVENT_LOG, "[Cancel] Task cancelled; already-processed images remain valid");
                finish(
                    &app_handle,
                    serde_json::json!({ "code": 2, "success": false, "cancelled": true }),
                );
            }
            Err(err) => {
                // 结果级验证 FAIL：落盘被拒（保护原图，避免"影响周边元素"的坏结果写出）
                if err.contains("verification FAILED") {
                    let msg = tr(
                        &lang,
                        "去水印结果未通过自检（水印外像素被改动或仍有残留），已拒绝写入以免损坏图片。请查看候选复查图后重试。",
                        "The result failed self-check (pixels outside the watermark changed or residue remains); writing was refused to avoid damaging the image. Review the candidate image and try again.",
                    );
                    let _ = app_handle.emit(EVENT_LOG, format!("[Error] {}", msg));
                    finish(
                        &app_handle,
                        serde_json::json!({ "code": -1, "success": false, "error": msg }),
                    );
                } else {
                    let _ = app_handle.emit(EVENT_LOG, format!("[Error] {}", err));
                    finish(
                        &app_handle,
                        serde_json::json!({ "code": -1, "success": false, "error": err }),
                    );
                }
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn cancel_pipeline(storage: State<'_, AppStorage>) -> Result<(), String> {
    storage.cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 用系统文件管理器打开目录（结果文件夹）。
#[tauri::command]
fn open_path(path: String, lang: Option<String>) -> Result<(), String> {
    let lang = lang_of(lang);
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(&path)
        .spawn()
        .map_err(|e| tr(&lang, &format!("打开失败: {}", e), &format!("Failed to open: {}", e)))?;
    Ok(())
}

#[tauri::command]
fn cleanup_pipeline(storage: State<'_, AppStorage>, lang: Option<String>) -> Result<(), String> {
    let lang = lang_of(lang);
    let root: PathBuf = storage
        .target_root
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| tr(&lang, "还没有处理记录", "No processing record yet"))?;
    let files: Vec<String> = storage.last_files.lock().map_err(|e| e.to_string())?.clone();
    if files.is_empty() {
        return Err(tr(&lang, "没有可清理的记录", "Nothing to clean up"));
    }
    let options = pipeline::PipelineOptions {
        root,
        files,
        keep_work: false,
        mask_box: None,
        any_position: false,
        overwrite_original: true,
        use_profile: true,
        force: false,
        inverse: true,
        retry: true,
        refine: false,
        forced_profile: None,
    };
    let names = pipeline::target_names(&options.root, &options.files)?;
    pipeline::cleanup(&options, &names)?;
    pipeline::cleanup_preserved()
}

#[tauri::command]
fn import_files(app: AppHandle, window: tauri::WebviewWindow, paths: Vec<String>, lang: Option<String>) -> Result<ImportResult, String> {
    let lang = lang_of(lang);
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
        let name = copy_one(&window, raw, &dir, index, &names, &lang)?;
        names.push(name);
    }
    if names.is_empty() {
        return Err(tr(&lang, "未选择图片", "No images selected"));
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
    lang: &str,
) -> Result<String, String> {
    #[cfg(target_os = "android")]
    if raw.starts_with("content://") {
        return crate::android_uri::copy_content_uri(window, raw, dir, index, existing);
    }
    let source = PathBuf::from(raw);
    if !source.is_file() {
        return Err(tr(lang, &format!("文件不存在: {}", raw), &format!("File not found: {}", raw)));
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
    std::fs::copy(&source, dir.join(&final_name)).map_err(|e| {
        tr(
            lang,
            &format!("拷贝 {} 失败: {}", final_name, e),
            &format!("Failed to copy {}: {}", final_name, e),
        )
    })?;
    Ok(final_name)
}

#[derive(Serialize)]
struct ImportResult {
    dir: String,
    names: Vec<String>,
}

#[tauri::command]
fn read_image_base64(path: String, lang: Option<String>) -> Result<String, String> {
    let lang = lang_of(lang);
    let file = PathBuf::from(&path);
    if !file.is_file() {
        return Err(tr(&lang, &format!("文件不存在: {}", path), &format!("File not found: {}", path)));
    }
    if file
        .file_name()
        .map(|name| !pipeline::is_supported_image(&name.to_string_lossy()))
        .unwrap_or(true)
    {
        return Err(tr(&lang, "只支持预览 png/jpg/webp 图片", "Only png/jpg/webp images can be previewed"));
    }
    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
    if bytes.len() > 10 * 1024 * 1024 {
        return Err(tr(&lang, "图片过大，无法预览", "Image too large to preview"));
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
                // 学习到的水印档案存应用数据目录（bundle 内 tools/ 只读且不打包）
                let _ = crate::PROFILES_DIR_OVERRIDE.set(data_dir.join("profiles"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            env_status,
            setup_model,
            list_pngs,
            import_files,
            learn_watermark,
            list_watermarks,
            delete_watermark,
            run_pipeline,
            cancel_pipeline,
            open_path,
            cleanup_pipeline,
            read_image_base64
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // 退出前清理保留的临时复查产物（/tmp/doubao-watermark-review）
            if matches!(event, tauri::RunEvent::Exit { .. }) {
                let _ = pipeline::cleanup_preserved();
            }
        });
}

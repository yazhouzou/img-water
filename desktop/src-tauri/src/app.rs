
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
/// 处理中用户尝试关窗：窗口被拦下，前端提示"任务进行中"。
const EVENT_QUIT_BLOCKED: &str = "quit-blocked";

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
    clear_dock_progress(app);
}

/// 清除 Dock/任务栏进度条（macOS 上进度条是应用级，任务结束必须显式收回）。
#[cfg(desktop)]
fn clear_dock_progress(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_progress_bar(tauri::window::ProgressBarState {
            status: Some(tauri::window::ProgressBarStatus::None),
            progress: Some(0),
        });
    }
}

#[cfg(not(desktop))]
fn clear_dock_progress(_app: &AppHandle) {}

/// 处理中在 Dock/任务栏显示进度（不支持进度条的平台静默忽略）。
#[cfg(desktop)]
fn set_dock_progress(app: &AppHandle, done: usize, total: usize) {
    if total == 0 {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let pct = ((done * 100) / total).min(100) as u64;
        let _ = window.set_progress_bar(tauri::window::ProgressBarState {
            status: Some(tauri::window::ProgressBarStatus::Normal),
            progress: Some(pct),
        });
    }
}

#[cfg(not(desktop))]
fn set_dock_progress(_app: &AppHandle, _done: usize, _total: usize) {}

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
/// 依次尝试 auto（多图自举 + 单帧纯色回退）与 batch（多图统一步长），取"学习后
/// 在原图上能重新定位"的那个（`profile_self_check`，按均分择优）；都过不了就报错。
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
                [255f32, 255f32, 255f32],
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
    inverse: Option<bool>,
    profile_id: Option<String>,
    output_dir: Option<String>,
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
            inverse: inverse.unwrap_or(false),
            retry: true,
            refine: refine.unwrap_or(true),
            forced_profile: profile_id.clone(),
            // 覆盖模式下由 output_dir() 统一忽略
            output_dir_override: output_dir.as_ref().map(PathBuf::from),
        };
        let log = |line: &str| {
            let _ = app_handle.emit(EVENT_LOG, line);
        };
        let progress_app = app_handle.clone();
        let progress = move |stage: &str, done: usize, total: usize, name: &str| {
            set_dock_progress(&progress_app, done, total);
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
                        "outputs": summary.outputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                        "processed": summary.processed,
                        "skipped": summary.skipped.iter().map(|(n, r)| serde_json::json!({ "name": n, "reason": r })).collect::<Vec<_>>(),
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

/// 「保存到相册」：Android 用 MediaStore 写入 Pictures/WatermarkCleaner（免权限）。
#[tauri::command]
fn export_results(
    window: tauri::WebviewWindow,
    paths: Vec<String>,
    lang: Option<String>,
) -> Result<usize, String> {
    let lang = lang_of(lang);
    if paths.is_empty() {
        return Err(tr(&lang, "没有可保存的图片", "No images to export"));
    }
    do_export(&window, &paths)
}

#[cfg(target_os = "android")]
fn do_export(window: &tauri::WebviewWindow, paths: &[String]) -> Result<usize, String> {
    crate::android_export::export_to_gallery(window, paths)
}

#[cfg(not(target_os = "android"))]
fn do_export(_window: &tauri::WebviewWindow, _paths: &[String]) -> Result<usize, String> {
    Err("Save to gallery is only available on Android".to_string())
}

/// 「分享」：Android 走系统分享面板（FileProvider + ACTION_SEND），接收方应用可存相册。
#[tauri::command]
fn share_results(
    app: AppHandle,
    window: tauri::WebviewWindow,
    paths: Vec<String>,
    lang: Option<String>,
) -> Result<usize, String> {
    let lang = lang_of(lang);
    if paths.is_empty() {
        return Err(tr(&lang, "没有可分享的图片", "No images to share"));
    }
    let cache = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    do_share(&window, &cache, &paths)
}

#[cfg(target_os = "android")]
fn do_share(
    window: &tauri::WebviewWindow,
    cache: &std::path::Path,
    paths: &[String],
) -> Result<usize, String> {
    crate::android_export::share_via_system(window, cache, paths)
}

#[cfg(not(target_os = "android"))]
fn do_share(
    _window: &tauri::WebviewWindow,
    _cache: &std::path::Path,
    _paths: &[String],
) -> Result<usize, String> {
    Err("Sharing is only available on Android".to_string())
}

/// 撤销「替换原图」：把 original-watermark-backup/ 里的原图恢复回 root。
#[tauri::command]
fn restore_backup(root: String, lang: Option<String>) -> Result<usize, String> {
    let lang = lang_of(lang);
    pipeline::restore_backup(&PathBuf::from(&root)).map_err(|e| {
        tr(
            &lang,
            &format!("没有可恢复的原图备份（{}）", e),
            &format!("No original backup to restore ({})", e),
        )
    })
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
        inverse: false,
        retry: true,
        refine: false,
        forced_profile: None,
        output_dir_override: None,
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
    // 换批即替换：删掉上一次导入的副本目录（含其 watermark-cleaned 结果）。否则每次选图
    // 都往应用私有目录里塞一份照片副本，永久累积且用户无法感知。结果导出走「保存到相册/
    // 分享」，故这里保留当前这一批即可。
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let p = entry.path();
            let is_old_import = p.file_name()
                .map(|n| n.to_string_lossy().starts_with("import-"))
                .unwrap_or(false);
            if is_old_import && p != dir {
                let _ = std::fs::remove_dir_all(&p);
            }
        }
    }
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

/// 预览图：`width`/`height` 是**原图**尺寸（`data_url` 可能是缩小后的预览）。
#[derive(Serialize)]
struct PreviewImage {
    data_url: String,
    width: u32,
    height: u32,
}

/// 预览图片（data URL）。`max` 给定时等比缩到最长边 ≤ `max`（框选大图用，避免把整张
/// 几十 MB 的图塞进 data URL）；同时返回**原图**尺寸，前端据此把框选坐标映射回原图。
/// `max` 为 None 时按原始字节返回（复查图/大图查看要求全分辨率，不能缩）。
#[tauri::command]
fn read_image_base64(
    path: String,
    max: Option<u32>,
    lang: Option<String>,
) -> Result<PreviewImage, String> {
    let lang = lang_of(lang);
    let file = PathBuf::from(&path);
    if !file.is_file() {
        return Err(tr(&lang, &format!("文件不存在: {}", path), &format!("File not found: {}", path)));
    }
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
        return Err(tr(&lang, "只支持预览 png/jpg/webp 图片", "Only png/jpg/webp images can be previewed"));
    }
    let reader = image::ImageReader::open(&file)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let (ow, oh) = reader
        .into_dimensions()
        .map_err(|e| tr(&lang, &format!("无法读取图片: {}", e), &format!("Failed to read image: {}", e)))?;
    let need_resize = max.map(|m| ow.max(oh) > m).unwrap_or(false);
    let data_url = if need_resize {
        let limit = max.unwrap_or(2048);
        let img = image::open(&file).map_err(|e| {
            tr(&lang, &format!("无法读取图片: {}", e), &format!("Failed to read image: {}", e))
        })?;
        let preview = img.thumbnail(limit, limit);
        let mut buf = std::io::Cursor::new(Vec::new());
        preview
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
        )
    } else {
        // 全分辨率：直接搬原始字节（不重编码），mime 按扩展名给对
        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(tr(&lang, "图片过大，无法预览", "Image too large to preview"));
        }
        let mime = match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => "image/png",
        };
        format!(
            "data:{};base64,{}",
            mime,
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    };
    Ok(PreviewImage {
        data_url,
        width: ow,
        height: oh,
    })
}

/// 拖入路径是否为目录（桌面端拖文件夹＝直接把它作为处理目录，原地处理）。
#[tauri::command]
fn is_directory(path: String) -> bool {
    PathBuf::from(path).is_dir()
}

/// 结果列表缩略图：解码后等比缩到最长边 `max` 像素再编码 PNG，
/// 避免把整张大图塞进列表（原图预览仍走 `read_image_base64`）。
#[tauri::command]
fn read_thumbnail_base64(path: String, max: Option<u32>, lang: Option<String>) -> Result<String, String> {
    let lang = lang_of(lang);
    let max = max.unwrap_or(160).clamp(32, 512);
    let file = PathBuf::from(&path);
    if !file.is_file() {
        return Err(tr(&lang, "文件不存在", "File not found"));
    }
    let img = image::open(&file).map_err(|e| {
        tr(
            &lang,
            &format!("无法读取图片: {}", e),
            &format!("Failed to read image: {}", e),
        )
    })?;
    let thumb = img.thumbnail(max, max);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/png;base64,{}", encoded))
}

/// 在系统文件管理器中定位并选中文件（结果列表「在文件夹中显示」）。仅桌面可用。
#[cfg(desktop)]
#[tauri::command]
fn reveal_path(path: String, lang: Option<String>) -> Result<(), String> {
    let lang = lang_of(lang);
    if !PathBuf::from(&path).exists() {
        return Err(tr(&lang, "文件不存在", "File not found"));
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(&path);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path));
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let dir = PathBuf::from(&path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut c = std::process::Command::new("xdg-open");
        c.arg(dir);
        c
    };
    cmd.spawn().map_err(|e| {
        tr(
            &lang,
            &format!("打开失败: {}", e),
            &format!("Failed to open: {}", e),
        )
    })?;
    Ok(())
}

#[cfg(not(desktop))]
#[tauri::command]
fn reveal_path(path: String, lang: Option<String>) -> Result<(), String> {
    let _ = path;
    let lang = lang_of(lang);
    Err(tr(&lang, "仅桌面端支持定位文件", "Reveal in folder is desktop-only"))
}

/// 导出运行日志到用户选定的文件（配合 dialog 插件的保存对话框）。
#[tauri::command]
fn write_text_file(path: String, content: String, lang: Option<String>) -> Result<(), String> {
    let lang = lang_of(lang);
    std::fs::write(&path, content).map_err(|e| {
        tr(
            &lang,
            &format!("写入失败: {}", e),
            &format!("Failed to write: {}", e),
        )
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_tauri_app() {
    // 移动端 cfg(desktop) 块被编译掉，builder 不再需要 mut
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // 桌面专属插件（两者在移动端整 crate 为空，必须 cfg 守卫，否则 Android 编译报错）：
    // - single-instance 必须最先注册：第二个实例把已有窗口带到前台后自身退出，避免两个
    //   进程共用同一 workdir（/tmp/doubao-watermark-work）与输出/备份目录互相覆盖。
    // - window-state 记忆窗口尺寸/位置（关闭时保存、窗口就绪时恢复）。
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }))
            .plugin(tauri_plugin_window_state::Builder::default().build());
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
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
            restore_backup,
            export_results,
            share_results,
            read_image_base64,
            read_thumbnail_base64,
            is_directory,
            reveal_path,
            write_text_file
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let running = window
                    .app_handle()
                    .try_state::<AppStorage>()
                    .map(|s| s.running.load(Ordering::SeqCst))
                    .unwrap_or(false);
                if running {
                    // 处理中不许关：否则进程被杀在写一半，留下半成品结果与半截备份。
                    api.prevent_close();
                    let _ = window.show();
                    #[cfg(desktop)]
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                    let _ = window.app_handle().emit(EVENT_QUIT_BLOCKED, ());
                } else {
                    // 关窗即退出。macOS 默认"关最后一个窗口不退出进程"，会留下一个
                    // 没有窗口、仍占着 ONNX 模型内存的后台进程（点 Dock 也回不来）。
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            let _ = app;
            match event {
                // 退出前清理保留的临时复查产物（/tmp/doubao-watermark-review）
                tauri::RunEvent::Exit { .. } => {
                    let _ = pipeline::cleanup_preserved();
                }
                // macOS：点 Dock 图标（或 Cmd+H 后回来）时把窗口带回前台，否则看起来像卡死
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
                _ => {}
            }
        });
}

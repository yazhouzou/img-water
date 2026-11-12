
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
/// 移动端导入相册副本：每张复制完成上报一次（导入几十张大图要数秒~十几秒，
/// 没有进度用户会以为应用卡死）。
const EVENT_IMPORT_PROGRESS: &str = "import-progress";
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

/// 从 panic 载荷里取出可读文本（`panic!("{}", x)` / `panic!("literal")` 都能取到）。
fn panic_text(payload: &Box<dyn std::any::Any + Send + 'static>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// 同步 Tauri 命令的 panic 兜底：把 panic 转成 `Err`。
///
/// 必要性：同步命令跑在 Tauri 事件循环线程上，panic 会一路 unwind 击穿事件循环 → 整个
/// App 直接崩掉（用户还没看到任何提示）。图片/模型解码这类第三方代码最容易踩到。
fn guard<T>(lang: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let detail = panic_text(&payload);
        Err(tr(
            lang,
            &format!("内部错误：{}", detail),
            &format!("Internal error: {}", detail),
        ))
    })
}

/// 后台任务统一入口：**兜住 panic 并按失败收尾**。
///
/// 必要性：任务跑在 `thread::spawn` 里，一旦 panic（解码/推理/索引越界）线程静默死掉，
/// `running` 永远停在 true——前端卡在"处理中"、关窗被 `prevent_close` 永久拦下、Dock
/// 进度条不收回，用户只能强杀进程。这里把 panic 转成失败事件，行为与"任务返回 Err"一致。
fn spawn_guarded<F>(app: AppHandle, lang: String, task: F)
where
    F: FnOnce() + Send + 'static,
{
    thread::spawn(move || {
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)) {
            let detail = panic_text(&payload);
            let _ = app.emit(EVENT_LOG, format!("[Panic] {}", detail));
            finish(
                &app,
                serde_json::json!({
                    "code": -1,
                    "success": false,
                    "error": tr(
                        &lang,
                        &format!("内部错误（本次任务已中止，原图未被改动）：{}", detail),
                        &format!("Internal error (task aborted, originals untouched): {}", detail),
                    ),
                }),
            );
        }
    });
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

/// 按当前界面语言重建系统菜单（语言可运行时切换；菜单文案在 Rust，故由前端通知）。
#[tauri::command]
fn set_menu_lang(app: AppHandle, lang: Option<String>) -> Result<(), String> {
    let lang = lang_of(lang);
    #[cfg(desktop)]
    return crate::menu::apply(&app, &lang).map_err(|e| e.to_string());
    #[cfg(not(desktop))]
    {
        let _ = (app, lang);
        Ok(())
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
    spawn_guarded(app.clone(), lang.clone(), move || {
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
    let l = lang_of(lang.clone());
    guard(&l, move || learn_watermark_impl(root, files, label, lang))
}

fn learn_watermark_impl(
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
    spawn_guarded(app.clone(), lang.clone(), move || {
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
                        "noWatermark": summary.no_watermark,
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

/// 移动端系统栏图标外观跟随 App 主题：targetSdk 36 的 edge-to-edge 下状态栏/导航栏
/// 透明、图标深浅由系统按"系统主题"决定；App 内手动切主题时会与系统主题不一致
/// （深色 App + 浅色系统 → 深色图标压在深色背景上看不清），故由前端在切换时同步。
#[tauri::command]
fn set_system_bars(window: tauri::WebviewWindow, dark: bool) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        return crate::android_export::set_system_bar_appearance(&window, dark);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (&window, dark);
        Ok(())
    }
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

/// 追加模式判定：`dir` 只有确属应用私有 imports/ 下的已有批次目录（import-*）才接受，
/// 防止前端误传任意路径把照片拷进不该去的地方。
fn resolve_append_dir(base: &std::path::Path, dir: &str) -> Option<PathBuf> {
    let p = PathBuf::from(dir);
    if p.parent() != Some(base) {
        return None;
    }
    let name = p.file_name()?.to_string_lossy();
    if !name.starts_with("import-") || !p.is_dir() {
        return None;
    }
    Some(p)
}

/// 列出批次目录顶层的 PNG 名（排序，稳定）。用于追加导入的重名去重基准与合并计数；
/// 不递归，故不会把 watermark-cleaned/ 里的结果算进来。
fn list_png_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_file())
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .filter(|n| n.to_lowercase().ends_with(".png"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// 导入图片到应用私有副本目录。`dir` 给定且合法时**追加**到该批次（不清旧图、
/// 重名加后缀），否则新建批次并替换旧批（移动端处理的是副本，结果走保存到相册）。
#[tauri::command]
fn import_files(
    app: AppHandle,
    window: tauri::WebviewWindow,
    paths: Vec<String>,
    dir: Option<String>,
    lang: Option<String>,
) -> Result<ImportResult, String> {
    let lang = lang_of(lang);
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("imports");
    // 追加模式：dir 必须是 base 下的合法批次目录，否则按未指定处理（新建批次）
    let append_dir = dir.as_deref().and_then(|d| resolve_append_dir(&base, d));
    let append = append_dir.is_some();
    let dir = match append_dir {
        Some(d) => d,
        None => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis();
            let d = base.join(format!("import-{}", stamp));
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            d
        }
    };

    // 重名去重基准：追加模式下必须包含目录里已有的图，否则同名新图会覆盖旧图
    let mut existing: Vec<String> = if append { list_png_names(&dir) } else { Vec::new() };
    let total = paths.len();
    let mut names: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut first_error: Option<String> = None;
    for (index, raw) in paths.iter().enumerate() {
        match copy_one(&window, raw, &dir, index, &existing, &lang) {
            Ok(name) => {
                existing.push(name.clone());
                names.push(name);
            }
            Err(err) => {
                // 单张失败（HEIC/权限/超限）只跳过这一张，不拖垮整批——一次误选
                // 一张 HEIC 就让 29 张好图全部导入失败，会让用户以为应用坏了。
                // 明细由返回值 skipped 带回前端（日志 + 提示），不在这里重复发日志。
                if first_error.is_none() {
                    first_error = Some(err.clone());
                }
                skipped.push(err);
            }
        }
        let _ = app.emit(
            EVENT_IMPORT_PROGRESS,
            serde_json::json!({ "done": index + 1, "total": total }),
        );
    }
    if names.is_empty() {
        // 全部失败：新批次清掉空目录、**不碰上一批**（旧批次是用户的当前工作集；
        // 旧实现在拷贝前就删旧批次，一次导入失败会让界面上的文件列表变成幽灵文件）；
        // 追加模式则什么都不动，旧批次原样保留。
        if !append {
            let _ = std::fs::remove_dir_all(&dir);
        }
        return Err(first_error.unwrap_or_else(|| tr(&lang, "未选择图片", "No images selected")));
    }
    if !append {
        // 换批即替换：**全部拷贝成功之后**才删上一次导入的副本目录（含其 watermark-cleaned
        // 结果）。否则每次选图都往应用私有目录里塞一份照片副本，永久累积且用户无法感知。
        // 结果导出走「保存到相册/分享」，故这里保留当前这一批即可。
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
    }
    Ok(ImportResult {
        dir: dir.display().to_string(),
        names,
        total: existing.len(),
        skipped,
    })
}

/// 导入副本统一用 `.png` 扩展名（替换原扩展名，而不是追加）：处理管线输出恒为 PNG，
/// 若保留 `.jpg` 名字会出现"名字是 jpg、内容是 PNG"的错配（分享给其它应用可能解码
/// 失败）；旧实现追加成 `.jpg.png` 也让文件名列表很难看。
pub(crate) fn to_png_name(name: &str) -> String {
    let lower = name.to_lowercase();
    for ext in [".jpg", ".jpeg", ".webp"] {
        if lower.ends_with(ext) && name.len() > ext.len() {
            return format!("{}.png", &name[..name.len() - ext.len()]);
        }
    }
    if lower.ends_with(".png") {
        name.to_string()
    } else {
        format!("{}.png", name)
    }
}

/// 校验导入文件确实是 PNG/JPEG/WebP（按魔数）。Android 相册里常见的 HEIC/HEIF
/// 不在 `image` crate 支持范围内，若不在这里拦下，用户会在"开始处理"后才看到
/// 难以理解的解码失败；且 HEIC 文件被改名为 .png 后更看不出原因。
fn image_magic_ok(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 12];
    let n = file.read(&mut head).unwrap_or(0);
    let h = &head[..n];
    let png = h.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let jpeg = h.starts_with(&[0xFF, 0xD8, 0xFF]);
    let webp = h.len() >= 12 && &h[..4] == b"RIFF" && &h[8..12] == b"WEBP";
    png || jpeg || webp
}

/// 拷完再验：不符即删文件并给出可读错误（返回的错误文本会进跳过列表/界面日志）。
fn validate_imported(dir: &std::path::Path, name: String, lang: &str) -> Result<String, String> {
    if image_magic_ok(&dir.join(&name)) {
        return Ok(name);
    }
    let _ = std::fs::remove_file(dir.join(&name));
    Err(tr(
        lang,
        &format!("{} 不是支持的图片格式（可能是 HEIC），请先在系统相册里导出为 JPEG/PNG 再试", name),
        &format!("{} is not a supported image format (possibly HEIC). Export it as JPEG/PNG first.", name),
    ))
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
        let name = crate::android_uri::copy_content_uri(window, raw, dir, index, existing)?;
        return validate_imported(dir, name, lang);
    }
    let source = PathBuf::from(raw);
    if !source.is_file() {
        return Err(tr(lang, &format!("文件不存在: {}", raw), &format!("File not found: {}", raw)));
    }
    let name = source
        .file_name()
        .map(|n| to_png_name(&n.to_string_lossy()))
        .unwrap_or_else(|| format!("image-{}.png", index));
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
    let dest = dir.join(&final_name);
    if let Err(e) = std::fs::copy(&source, &dest) {
        // copy 中途失败会留下部分文件，清掉避免污染批次
        let _ = std::fs::remove_file(&dest);
        return Err(tr(
            lang,
            &format!("拷贝 {} 失败: {}", final_name, e),
            &format!("Failed to copy {}: {}", final_name, e),
        ));
    }
    validate_imported(dir, final_name, lang)
}

#[derive(Serialize)]
struct ImportResult {
    dir: String,
    /// 本次成功导入（新增）的文件名
    names: Vec<String>,
    /// 合并后批次内的 PNG 总数（追加模式下 > names.len()）
    total: usize,
    /// 无法导入的文件（含原因）：前端提示"N 张被跳过"并在日志里列出明细。
    skipped: Vec<String>,
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

/// 移动端：调起系统图片选择器（Android 13+ 系统 Photo Picker / 旧版本回退）。
/// 启动即返回；选择结果由 Kotlin 侧 `evaluateJavascript` 调
/// `window.__onAndroidPickResult(uris)` 异步回传（与 `__onAndroidBack` 同模式）。
#[tauri::command]
fn pick_images(window: tauri::WebviewWindow, max: Option<i32>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let max = max.unwrap_or(0).clamp(0, 200);
        return crate::android_pick::pick_images(&window, max);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (window, max);
        Err("当前平台不支持系统相册选择器".to_string())
    }
}

/// 移动端：重建 WebView，修复从系统 picker 返回后的触摸卡死（ColorOS 12 实测，
/// 详见 android_pick.rs）。桌面端为 no-op——前端只在移动端 pick 导入完成后调用。
#[tauri::command]
fn rebuild_webview(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        return crate::android_pick::rebuild_webview(&window);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = window;
        Ok(())
    }
}

/// 文件名安全校验：只允许批次目录下的直接子文件名（防路径穿越）。
fn safe_import_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.starts_with('.')
        && !name.contains("..")
}

/// 从当前导入批次中移除单张图片（列表页"移除"用）。文件已不存在时按成功处理（幂等）；
/// 结果目录（watermark-cleaned）里的旧结果不清理（无引用、随批次一起删除）。
#[tauri::command]
fn remove_import_file(
    app: AppHandle,
    dir: String,
    name: String,
    lang: Option<String>,
) -> Result<(), String> {
    let lang = lang_of(lang);
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("imports");
    let batch = resolve_append_dir(&base, &dir)
        .ok_or_else(|| tr(&lang, "无效的批次目录", "Invalid batch directory"))?;
    if !safe_import_name(&name) {
        return Err(tr(&lang, "无效的文件名", "Invalid file name"));
    }
    let path = batch.join(&name);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|e| {
            tr(
                &lang,
                &format!("移除失败: {}", e),
                &format!("Failed to remove: {}", e),
            )
        })?;
    }
    Ok(())
}

/// 清空当前导入批次（设置页"清除图片"）：删除应用私有 imports/ 下的全部批次目录，
/// 含各自的结果副本。用户相册原图不受影响（处理的始终是副本）。
#[tauri::command]
fn clear_import_batch(app: AppHandle) -> Result<usize, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("imports");
    let mut removed = 0usize;
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let p = entry.path();
            let is_import = p
                .file_name()
                .map(|n| n.to_string_lossy().starts_with("import-"))
                .unwrap_or(false);
            if is_import && std::fs::remove_dir_all(&p).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
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
        // 菜单事件（菜单 API 在移动端不存在，必须 cfg 守卫）
        builder = builder.on_menu_event(|app, event| {
            let _ = crate::menu::forward(app, &event);
        });
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppStorage::default())
        .setup(|app| {
            // 桌面端安装自定义系统菜单（语言先给默认值，前端 init 时用 set_menu_lang 同步）。
            // 失败要吵：菜单静默不出现很难排查，故这里显式打 stderr。
            #[cfg(desktop)]
            if let Err(err) = crate::menu::apply(app.handle(), "zh") {
                eprintln!("[menu] 创建系统菜单失败: {err}");
            }
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
            set_menu_lang,
            list_pngs,
            import_files,
            pick_images,
            rebuild_webview,
            remove_import_file,
            clear_import_batch,
            learn_watermark,
            list_watermarks,
            delete_watermark,
            run_pipeline,
            cancel_pipeline,
            open_path,
            cleanup_pipeline,
            restore_backup,
            set_system_bars,
            export_results,
            share_results,
            read_image_base64,
            read_thumbnail_base64,
            is_directory,
            reveal_path,
            write_text_file
        ])
        .on_page_load(|window, _payload| {
            // 告诉前端"系统菜单已就绪"：带标准快捷键的菜单项在 macOS 上会先消费按键，
            // 前端据此跳过自己的 Cmd+O / Cmd+Enter 处理，避免弹两次对话框。
            // 在 page_load 时注入（setup 里网页还没加载，赋值会丢）。
            #[cfg(desktop)]
            {
                if window.label() == "main" {
                    // 置标志 + 若前端已初始化则回调它（页面加载与 JS 初始化的先后不确定，
                    // 两边都要能生效），前端据此同步菜单语言并跳过本地快捷键处理。
                    let _ = window.eval(
                        "window.__WM_HAS_MENU = true; \
                         if (window.__WM_MENU_READY) window.__WM_MENU_READY();",
                    );
                }
            }
            #[cfg(not(desktop))]
            {
                let _ = window;
            }
        })
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
                    // macOS 惯例：关窗只隐藏窗口，App 留在 Dock（点 Dock 图标经
                    // RunEvent::Reopen 恢复），Cmd+Q / 菜单「退出」才真正结束进程。
                    // 代价是进程与 ONNX 模型内存常驻，属用户明确选择的标准语义。
                    #[cfg(target_os = "macos")]
                    {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    // Windows/Linux 没有 Reopen 事件，窗口一关就再也回不来，
                    // 隐藏等于留下无窗口的后台进程，故保持关窗即退出。
                    #[cfg(not(target_os = "macos"))]
                    {
                        window.app_handle().exit(0);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_png_name_replaces_supported_extensions() {
        assert_eq!(to_png_name("IMG_0001.jpg"), "IMG_0001.png");
        assert_eq!(to_png_name("IMG_0001.JPEG"), "IMG_0001.png");
        assert_eq!(to_png_name("shot.WebP"), "shot.png");
        assert_eq!(to_png_name("already.png"), "already.png");
        // 无扩展名/奇怪名字：追加 .png 兜底（扩展名校验交给魔数，不看名字）
        assert_eq!(to_png_name("noext"), "noext.png");
        assert_eq!(to_png_name("photo.heic"), "photo.heic.png");
    }

    #[test]
    fn image_magic_accepts_supported_rejects_heic_like() {
        let dir = std::env::temp_dir().join(format!("wm-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0]).unwrap();
        let jpg = dir.join("b.png");
        std::fs::write(&jpg, [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0]).unwrap();
        let webp = dir.join("c.png");
        std::fs::write(&webp, *b"RIFF\x00\x00\x00\x00WEBP").unwrap();
        // HEIC（ftyp box 第三、四字节起）
        let heic = dir.join("d.png");
        std::fs::write(&heic, *b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00").unwrap();
        let empty = dir.join("e.png");
        std::fs::write(&empty, []).unwrap();
        assert!(image_magic_ok(&png));
        assert!(image_magic_ok(&jpg));
        assert!(image_magic_ok(&webp));
        assert!(!image_magic_ok(&heic));
        assert!(!image_magic_ok(&empty));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_dir_only_accepts_owned_batch_dirs() {
        let base = std::env::temp_dir().join(format!("wm-append-base-{}", std::process::id()));
        let batch = base.join("import-123");
        std::fs::create_dir_all(&batch).unwrap();
        let outside = std::env::temp_dir().join(format!("wm-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        // 合法：base 下的 import-* 目录
        assert_eq!(resolve_append_dir(&base, batch.to_str().unwrap()), Some(batch.clone()));
        // 非法：目录不存在 / 名字不是 import-* / 非 base 直属（防目录穿越）
        assert!(resolve_append_dir(&base, base.join("import-404").to_str().unwrap()).is_none());
        assert!(resolve_append_dir(&base, base.join("evil").to_str().unwrap()).is_none());
        assert!(resolve_append_dir(&base, outside.to_str().unwrap()).is_none());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn safe_import_name_rejects_traversal() {
        assert!(safe_import_name("IMG_0001.png"));
        assert!(safe_import_name("a b-c.png"));
        assert!(!safe_import_name("../evil.png"));
        assert!(!safe_import_name("a/b.png"));
        assert!(!safe_import_name("a\\b.png"));
        assert!(!safe_import_name(".hidden"));
        assert!(!safe_import_name(""));
    }

    #[test]
    fn list_png_names_only_top_level_png_sorted() {
        let dir = std::env::temp_dir().join(format!("wm-list-png-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("watermark-cleaned")).unwrap();
        std::fs::write(dir.join("b.png"), []).unwrap();
        std::fs::write(dir.join("a.PNG"), []).unwrap();
        std::fs::write(dir.join("note.txt"), []).unwrap();
        std::fs::write(dir.join("c.png.tpl"), []).unwrap();
        std::fs::write(dir.join("watermark-cleaned/out.png"), []).unwrap();
        let names = list_png_names(&dir);
        assert_eq!(names, vec!["a.PNG".to_string(), "b.png".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

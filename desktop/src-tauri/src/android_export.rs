//! Android 结果导出：写入系统相册（MediaStore）/ 调起系统分享（FileProvider + ACTION_SEND）。
//! 与 android_uri.rs 同样走 JNI，在 webview 线程上直接调用 MainActivity 的实例方法；
//! 桌面端也参与编译（仅类型检查），无人调用属预期 dead_code。
#![allow(dead_code)]

use std::path::Path;

/// 按扩展名取 MIME；未知返回 image/*（分享时兼容性最好）。
pub fn mime_of(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "image/*"
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "image.png".to_string())
}

#[cfg(target_os = "android")]
fn on_activity<T, F>(window: &tauri::WebviewWindow, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut jni::JNIEnv, &jni::objects::JObject) -> Result<T, String> + Send + 'static,
{
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _| {
                let _ = tx.send(f(env, activity));
            });
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(120))
        .map_err(|_| "Android 操作超时".to_string())?
}

/// 逐张写入系统相册，返回成功的张数。
#[cfg(target_os = "android")]
pub fn export_to_gallery(window: &tauri::WebviewWindow, paths: &[String]) -> Result<usize, String> {
    let paths = paths.to_vec();
    on_activity(window, move |env, activity| {
        let mut ok = 0usize;
        for p in &paths {
            let name = basename(p);
            let mime = crate::android_export::mime_of(&name);
            let jp = env.new_string(p).map_err(|e| e.to_string())?;
            let jn = env.new_string(&name).map_err(|e| e.to_string())?;
            let jm = env.new_string(mime).map_err(|e| e.to_string())?;
            let res = env.call_method(
                activity,
                "exportToGallery",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[
                    jni::objects::JValue::Object(&jp),
                    jni::objects::JValue::Object(&jn),
                    jni::objects::JValue::Object(&jm),
                ],
            );
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
                continue;
            }
            let Ok(value) = res else { continue };
            let Ok(obj) = value.l() else { continue };
            let jstr = jni::objects::JString::from(obj);
            let Ok(s) = env.get_string(&jstr) else { continue };
            if !String::from(s).trim().is_empty() {
                ok += 1;
            }
        }
        Ok(ok)
    })
}

/// 把结果暂存到缓存目录后用 FileProvider 调起系统分享，返回分享的张数。
#[cfg(target_os = "android")]
pub fn share_via_system(
    window: &tauri::WebviewWindow,
    cache_dir: &Path,
    paths: &[String],
) -> Result<usize, String> {
    let share_root = cache_dir.join("share");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let dir = share_root.join(stamp.to_string());
    // 先清掉上一轮的暂存，避免缓存累积（<cache-path> 已覆盖该目录，无需改 file_paths.xml）
    if let Ok(entries) = std::fs::read_dir(&share_root) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut staged: Vec<String> = Vec::new();
    for p in paths {
        let name = basename(p);
        let dest = dir.join(&name);
        std::fs::copy(p, &dest).map_err(|e| format!("暂存分享文件失败: {}", e))?;
        staged.push(dest.to_string_lossy().to_string());
    }
    if staged.is_empty() {
        return Err("没有可分享的图片".to_string());
    }
    let count = staged.len();
    // 同一类型用具体 MIME，混合用 image/*
    let mime = {
        let first = mime_of(&basename(&staged[0]));
        if staged.iter().all(|s| mime_of(&basename(s)) == first) {
            first
        } else {
            "image/*"
        }
    };
    on_activity(window, move |env, activity| {
        let arr = env
            .new_object_array(staged.len() as i32, "java/lang/String", jni::objects::JObject::null())
            .map_err(|e| e.to_string())?;
        for (i, s) in staged.iter().enumerate() {
            let js = env.new_string(s).map_err(|e| e.to_string())?;
            env.set_object_array_element(&arr, i as i32, &js)
                .map_err(|e| e.to_string())?;
        }
        let jm = env.new_string(mime).map_err(|e| e.to_string())?;
        env.call_method(
            activity,
            "shareFiles",
            "([Ljava/lang/String;Ljava/lang/String;)V",
            &[
                jni::objects::JValue::Object(&arr),
                jni::objects::JValue::Object(&jm),
            ],
        )
        .map_err(|e| e.to_string())?;
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            return Err("调起分享失败".to_string());
        }
        Ok(count)
    })
}

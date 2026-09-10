//! Android 相册导入：把 content:// URI 通过 ContentResolver 拷贝到应用目录。
//! 纯 JNI 逻辑（copy_with_jni）不依赖 android 编译目标，桌面端也参与编译以便本地类型检查。

use jni::objects::{JObject, JString, JValue};
use jni::sys::jint;
use jni::JNIEnv;
use std::io::Write;

const BUF_SIZE: jint = 64 * 1024;
const MAX_SIZE: usize = 200 * 1024 * 1024;

#[cfg(target_os = "android")]
pub fn copy_content_uri(
    window: &tauri::WebviewWindow,
    uri: &str,
    dir: &std::path::Path,
    index: usize,
    existing: &[String],
) -> Result<String, String> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let uri = uri.to_string();
    let dir = dir.to_path_buf();
    let existing = existing.to_vec();
    window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _| {
                let result = copy_with_jni(env, activity, &uri, &dir, index, &existing);
                let _ = tx.send(result);
            });
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(120))
        .map_err(|_| "读取系统相册文件超时".to_string())?
}

fn jni_err<E: std::fmt::Display>(e: E) -> String {
    format!("JNI 调用失败: {}", e)
}

fn dedupe(name: &str, existing: &[String]) -> String {
    let mut final_name = name.to_string();
    let mut n = 1;
    while existing.iter().any(|e| e == &final_name) {
        let stem = name.trim_end_matches(".png");
        final_name = format!("{}-{}.png", stem, n);
        n += 1;
    }
    final_name
}

fn copy_with_jni(
    env: &mut JNIEnv,
    activity: &JObject,
    uri: &str,
    dir: &std::path::Path,
    index: usize,
    existing: &[String],
) -> Result<String, String> {
    let uri_str = env.new_string(uri).map_err(jni_err)?;
    let class_uri = env.find_class("android/net/Uri").map_err(jni_err)?;
    let uri_obj = env
        .call_static_method(
            &class_uri,
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&uri_str)],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("解析 URI 失败: {}", e))?;

    let resolver = env
        .call_method(
            activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("获取 ContentResolver 失败: {}", e))?;

    let mut name = format!("image-{}.png", index);
    if let Ok(Some(display)) = query_display_name(env, &resolver, &uri_obj) {
        if !display.trim().is_empty() {
            name = display;
        }
    }
    if !name.to_lowercase().ends_with(".png") {
        name.push_str(".png");
    }
    let final_name = dedupe(&name, existing);

    let stream = env
        .call_method(
            &resolver,
            "openInputStream",
            "(Landroid/net/Uri;)Ljava/io/InputStream;",
            &[JValue::Object(&uri_obj)],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("打开相册文件失败: {}", e))?;
    if env.exception_check().map_err(jni_err)? {
        let _ = env.exception_clear();
        return Err("打开相册文件失败（无访问权限）".into());
    }

    let file = std::fs::File::create(dir.join(&final_name)).map_err(|e| e.to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    let buf = env.new_byte_array(BUF_SIZE).map_err(jni_err)?;
    let mut total: usize = 0;
    loop {
        let n = env
            .call_method(&stream, "read", "([B)I", &[JValue::Object(&buf)])
            .and_then(|v| v.i())
            .map_err(|e| format!("读取相册文件失败: {}", e))?;
        if env.exception_check().map_err(jni_err)? {
            let _ = env.exception_clear();
            return Err("读取相册文件失败".into());
        }
        if n <= 0 {
            break;
        }
        let mut chunk = vec![0i8; n as usize];
        env.get_byte_array_region(&buf, 0, &mut chunk)
            .map_err(jni_err)?;
        let bytes: Vec<u8> = chunk.into_iter().map(|b| b as u8).collect();
        writer.write_all(&bytes).map_err(|e| e.to_string())?;
        total += n as usize;
        if total > MAX_SIZE {
            return Err("图片过大，无法导入".into());
        }
    }
    writer.flush().map_err(|e| e.to_string())?;
    if total == 0 {
        return Err("相册文件内容为空".into());
    }
    Ok(final_name)
}

fn query_display_name(
    env: &mut JNIEnv,
    resolver: &JObject,
    uri: &JObject,
) -> Result<Option<String>, String> {
    let cursor = env
        .call_method(
            resolver,
            "query",
            "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
            &[
                JValue::Object(uri),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
            ],
        )
        .and_then(|v| v.l())
        .map_err(jni_err)?;
    if cursor.is_null() {
        return Ok(None);
    }
    let key = env.new_string("_display_name").map_err(jni_err)?;
    let idx = env
        .call_method(&cursor, "getColumnIndex", "(Ljava/lang/String;)I", &[JValue::Object(&key)])
        .and_then(|v| v.i())
        .map_err(jni_err)?;
    if idx < 0 {
        return Ok(None);
    }
    let moved = env
        .call_method(&cursor, "moveToFirst", "()Z", &[])
        .and_then(|v| v.z())
        .map_err(jni_err)?;
    if !moved {
        return Ok(None);
    }
    let value = env
        .call_method(&cursor, "getString", "(I)Ljava/lang/String;", &[JValue::Int(idx)])
        .and_then(|v| v.l())
        .map_err(jni_err)?;
    let s: String = env.get_string(&JString::from(value)).map_err(jni_err)?.into();
    Ok(Some(s))
}

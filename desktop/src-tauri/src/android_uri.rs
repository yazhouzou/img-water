//! Android 相册导入：把 content:// URI 通过 ContentResolver 拷贝到应用目录。
//! 纯 JNI 逻辑（copy_with_jni）不依赖 android 编译目标，桌面端也参与编译以便本地类型检查；
//! 桌面端没人调用这些函数，属预期的 dead_code，桌面构建时静默。
#![allow(dead_code)]

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
                match prepare_copy(env, activity, &uri, &dir, index, &existing) {
                    PreparedCopy::Fd { fd, dest, name } => {
                        // 快车道：主线程只做轻量 JNI（解析/查名/openFileDescriptor/detachFd），
                        // 字节读循环搬进 Rust 线程——此前整个拷贝都在主线程，29 张大图会把
                        // UI 线程堵死（ANR 风险）。fd 由 native 持有，读完 drop 即关闭。
                        std::thread::spawn(move || {
                            let result = match copy_from_fd(fd, &dest) {
                                Ok(()) => Ok(name),
                                Err(e) => {
                                    let _ = std::fs::remove_file(&dest);
                                    Err(e)
                                }
                            };
                            let _ = tx.send(result);
                        });
                    }
                    PreparedCopy::Stream { stream, dest, name } => {
                        // 回退：稀有 provider 不支持 openFileDescriptor，保持旧行为（主线程流式拷贝）
                        eprintln!("[import] provider 不支持 fd 打开，回退流式拷贝（主线程）");
                        let result = match copy_stream(env, &stream, &dest) {
                            Ok(total) if total > 0 => Ok(name),
                            Ok(_) => {
                                let _ = std::fs::remove_file(&dest);
                                Err("相册文件内容为空".into())
                            }
                            Err(e) => {
                                let _ = std::fs::remove_file(&dest);
                                Err(e)
                            }
                        };
                        let _ = tx.send(result);
                    }
                    PreparedCopy::Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            });
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(120))
        .map_err(|_| "读取系统相册文件超时".to_string())?
}

/// 主线程 JNI 部分的结果：要么拿到 fd（快车道，在 Rust 线程读），
/// 要么打开的是 InputStream（回退，主线程读）。
enum PreparedCopy<'l> {
    Fd {
        fd: i32,
        dest: std::path::PathBuf,
        name: String,
    },
    Stream {
        stream: JObject<'l>,
        dest: std::path::PathBuf,
        name: String,
    },
    Err(String),
}

/// 只做轻量 JNI：解析 URI、查显示名、尝试 openFileDescriptor + detachFd。
/// 返回后主线程不再参与字节传输（fd 路径）——这是与旧实现的关键差异。
/// 注意：`env` 与 `activity` 的生命周期相互独立（wry exec 闭包给的是两个不同的
/// 高阶生命周期，不能绑成同一个）。
fn prepare_copy<'e>(
    env: &mut JNIEnv<'e>,
    activity: &JObject<'_>,
    uri: &str,
    dir: &std::path::Path,
    index: usize,
    existing: &[String],
) -> PreparedCopy<'e> {
    let uri_str = match env.new_string(uri) {
        Ok(s) => s,
        Err(e) => return PreparedCopy::Err(jni_err(e)),
    };
    let class_uri = match env.find_class("android/net/Uri") {
        Ok(c) => c,
        Err(e) => return PreparedCopy::Err(jni_err(e)),
    };
    let uri_obj = match env
        .call_static_method(
            &class_uri,
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&uri_str)],
        )
        .and_then(|v| v.l())
    {
        Ok(u) => u,
        Err(e) => return PreparedCopy::Err(format!("解析 URI 失败: {}", e)),
    };

    let resolver = match env
        .call_method(
            activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )
        .and_then(|v| v.l())
    {
        Ok(r) => r,
        Err(e) => return PreparedCopy::Err(format!("获取 ContentResolver 失败: {}", e)),
    };

    let mut name = format!("image-{}.png", index);
    if let Ok(Some(display)) = query_display_name(env, &resolver, &uri_obj) {
        if !display.trim().is_empty() {
            name = display;
        }
    }
    let name = dedupe(&crate::app::to_png_name(&name), existing);
    let dest = dir.join(&name);

    match try_open_fd(env, &resolver, &uri_obj) {
        Some(fd) => PreparedCopy::Fd { fd, dest, name },
        None => {
            // 回退：打开 InputStream（旧路径）
            let stream = match env
                .call_method(
                    &resolver,
                    "openInputStream",
                    "(Landroid/net/Uri;)Ljava/io/InputStream;",
                    &[JValue::Object(&uri_obj)],
                )
                .and_then(|v| v.l())
            {
                Ok(s) => s,
                Err(e) => return PreparedCopy::Err(format!("打开相册文件失败: {}", e)),
            };
            if matches!(env.exception_check(), Ok(true)) {
                let _ = env.exception_clear();
                return PreparedCopy::Err("打开相册文件失败（无访问权限）".into());
            }
            PreparedCopy::Stream { stream, dest, name }
        }
    }
}

/// 尝试 `openFileDescriptor(uri, "r")` + `detachFd()`；任何异常/不支持都返回 None
/// （由调用方回退到 openInputStream）。detachFd 后 fd 所有权归 native。
fn try_open_fd(
    env: &mut JNIEnv<'_>,
    resolver: &JObject<'_>,
    uri_obj: &JObject<'_>,
) -> Option<i32> {
    let mode = env.new_string("r").ok()?;
    let pfd = env
        .call_method(
            resolver,
            "openFileDescriptor",
            "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
            &[JValue::Object(uri_obj), JValue::Object(&mode)],
        )
        .and_then(|v| v.l())
        .ok()?;
    if matches!(env.exception_check(), Ok(true)) {
        let _ = env.exception_clear();
        return None;
    }
    if pfd.is_null() {
        return None;
    }
    let fd = env
        .call_method(&pfd, "detachFd", "()I", &[])
        .and_then(|v| v.i())
        .ok();
    if matches!(env.exception_check(), Ok(true)) {
        let _ = env.exception_clear();
        // detachFd 抛异常时 fd 仍归 PFD，补偿关闭避免 fd 泄漏
        let _ = env.call_method(&pfd, "close", "()V", &[]);
        let _ = env.exception_clear();
        return None;
    }
    match fd {
        Some(n) if n >= 0 => Some(n),
        _ => {
            let _ = env.call_method(&pfd, "close", "()V", &[]);
            None
        }
    }
}

/// 从裸 fd 读循环（Rust 线程执行；fd 读尽自动关闭）。
#[cfg(target_os = "android")]
fn copy_from_fd(fd: i32, dest: &std::path::Path) -> Result<(), String> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let mut src = unsafe { std::fs::File::from_raw_fd(fd) };
    let file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    let mut buf = vec![0u8; 128 * 1024];
    let mut total: usize = 0;
    loop {
        let n = src.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        total += n;
        if total > MAX_SIZE {
            return Err("图片过大，无法导入".into());
        }
    }
    writer.flush().map_err(|e| e.to_string())?;
    Ok(())
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

fn copy_stream(env: &mut JNIEnv, stream: &JObject, dest: &std::path::Path) -> Result<usize, String> {
    let file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    let buf = env.new_byte_array(BUF_SIZE).map_err(jni_err)?;
    let mut total: usize = 0;
    loop {
        let n = env
            .call_method(stream, "read", "([B)I", &[JValue::Object(&buf)])
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
    Ok(total)
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

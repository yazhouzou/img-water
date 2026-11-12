//! Android 系统图片选择器：经 JNI 调 MainActivity.pickImages 启动系统 Photo Picker。
//! **结果不经本模块返回**——Kotlin 侧拿到 Activity 结果后直接
//! `evaluateJavascript("window.__onAndroidPickResult(...)")` 回传前端（与
//! `__onAndroidBack` 同一模式）。这里只负责在主线程触发启动、把启动异常带回来。
//! 桌面端也参与编译（仅类型检查），无人调用属预期 dead_code。
#![allow(dead_code)]

/// 启动系统多选图片选择器。`max <= 0` 时由 Kotlin 侧取默认上限。
#[cfg(target_os = "android")]
pub fn pick_images(window: &tauri::WebviewWindow, max: i32) -> Result<(), String> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _| {
                let res = (|| -> Result<(), String> {
                    env.call_method(
                        activity,
                        "pickImages",
                        "(I)V",
                        &[jni::objects::JValue::Int(max)],
                    )
                    .map_err(|e| format!("启动图片选择器失败: {}", e))?;
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_clear();
                        return Err("启动图片选择器失败".to_string());
                    }
                    Ok(())
                })();
                let _ = tx.send(res);
            });
        })
        .map_err(|e| e.to_string())?;
    // 启动是毫秒级的：给 15s 余量，超时说明主线程异常（不该发生）
    rx.recv_timeout(std::time::Duration::from_secs(15))
        .map_err(|_| "启动图片选择器超时".to_string())?
}

/// 重建宿主 Activity/WebView：ColorOS 12 从 picker 返回后 WebView 输入分发卡死
/// （点击全部失效）的最终修复。由前端在导入流程完成后调用——recreate 会销毁
/// 调用方页面，属 fire-and-forget；Kotlin 侧 runOnUiThread post 后立即返回。
#[cfg(target_os = "android")]
pub fn rebuild_webview(window: &tauri::WebviewWindow) -> Result<(), String> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _| {
                let res = (|| -> Result<(), String> {
                    env.call_method(activity, "rebuildWebView", "()V", &[])
                        .map_err(|e| format!("重建 WebView 失败: {}", e))?;
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_clear();
                        return Err("重建 WebView 失败".to_string());
                    }
                    Ok(())
                })();
                let _ = tx.send(res);
            });
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "重建 WebView 超时".to_string())?
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine as _;
use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tauri::{AppHandle, Emitter, Manager, State};

const EVENT_LOG: &str = "pipeline-log";
const EVENT_EXIT: &str = "pipeline-exit";

#[derive(Default)]
struct AppStorage {
    target_root: std::sync::Mutex<Option<PathBuf>>,
    last_files: std::sync::Mutex<Vec<String>>,
    running: AtomicBool,
}

fn project_root() -> PathBuf {
    // Release builds: look for tools/ next to the executable (bundled resources).
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
    // Dev builds: desktop/src-tauri -> project root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .expect("cannot resolve project root")
}

fn venv_python() -> Option<PathBuf> {
    let root = project_root();
    if cfg!(windows) {
        let path = root.join(".img-inpaint-venv/Scripts/python.exe");
        if path.exists() {
            return Some(path);
        }
    } else {
        let path = root.join(".img-inpaint-venv/bin/python");
        if path.exists() {
            return Some(path);
        }
    }
    None
}

#[derive(Serialize)]
struct EnvStatus {
    ready: bool,
    python_path: String,
    script_path: String,
    hint: String,
}

#[tauri::command]
fn env_status() -> EnvStatus {
    let root = project_root();
    let python = venv_python();
    let script = root.join("tools/remove_doubao_watermark.py");
    let ready = python.is_some() && script.exists();
    EnvStatus {
        ready,
        python_path: python
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "missing".into()),
        script_path: script.display().to_string(),
        hint: if ready {
            String::new()
        } else {
            "修复环境未就绪：点击“一键初始化修复环境”在线安装（PyPI 国内镜像 + LaMa 模型）".into()
        },
    }
}

fn setup_script_command() -> Command {
    let root = project_root();
    if cfg!(windows) {
        let script = root.join("tools/ensure-inpaint-env.ps1");
        let mut command = Command::new("powershell");
        command
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script);
        command
    } else {
        let script = root.join("tools/ensure-inpaint-env.sh");
        let mut command = Command::new("bash");
        command.arg(&script);
        command
    }
}

#[tauri::command]
fn setup_env(app: AppHandle, storage: State<'_, AppStorage>) -> Result<(), String> {
    if storage.running.swap(true, Ordering::SeqCst) {
        return Err("已有任务在运行中，请等待完成".into());
    }
    let mut command = setup_script_command();
    command
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().map_err(|e| {
        storage.running.store(false, Ordering::SeqCst);
        format!("启动初始化脚本失败: {}", e)
    })?;
    spawn_and_wait(app, child);
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

fn emit_line(app: &AppHandle, line: String) {
    let _ = app.emit(EVENT_LOG, line);
}

fn spawn_stream(app: AppHandle, stream: impl std::io::Read + Send + 'static) {
    thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(text) => emit_line(&app, text),
                Err(_) => break,
            }
        }
    });
}

fn spawn_and_wait(app: AppHandle, mut child: Child) {
    if let Some(stdout) = child.stdout.take() {
        spawn_stream(app.clone(), stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_stream(app.clone(), stderr);
    }
    let app_handle = app.clone();
    thread::spawn(move || {
        let status = child.wait();
        match status {
            Ok(code) => {
                let _ = app_handle.emit(
                    EVENT_EXIT,
                    serde_json::json!({ "code": code.code().unwrap_or(-1), "success": code.success() }),
                );
            }
            Err(err) => {
                let _ = app_handle.emit(
                    EVENT_EXIT,
                    serde_json::json!({ "code": -1, "success": false, "error": err.to_string() }),
                );
            }
        }
        if let Some(state) = app_handle.try_state::<AppStorage>() {
            state.running.store(false, Ordering::SeqCst);
        }
    });
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
    let python = venv_python().ok_or_else(|| {
        storage.running.store(false, Ordering::SeqCst);
        "缺少 .img-inpaint-venv，请先运行 ./tools/ensure-inpaint-env.sh".to_string()
    })?;
    let script = project_root().join("tools/remove_doubao_watermark.py");
    if !script.exists() {
        storage.running.store(false, Ordering::SeqCst);
        return Err(format!("找不到流水线脚本: {}", script.display()));
    }

    let workdir = std::env::temp_dir().join("doubao-watermark-work");
    let mut command = Command::new(&python);
    command
        .arg(&script)
        .arg("--root")
        .arg(&root)
        .env("DOUBAO_WATERMARK_WORKDIR", &workdir)
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if keep_work {
        command.arg("--keep-work");
    }
    command.arg("run");
    for file in &files {
        command.arg(file);
    }

    let child = command.spawn().map_err(|e| {
        storage.running.store(false, Ordering::SeqCst);
        format!("启动流水线失败: {}", e)
    })?;

    spawn_and_wait(app, child);

    let mut last = storage.last_files.lock().map_err(|e| e.to_string())?;
    *last = files;
    storage
        .target_root
        .lock()
        .map_err(|e| e.to_string())?
        .replace(PathBuf::from(root));
    Ok(())
}

#[tauri::command]
fn cleanup_pipeline(storage: State<'_, AppStorage>) -> Result<(), String> {
    let python = venv_python().ok_or("缺少 .img-inpaint-venv，请先运行 ./tools/ensure-inpaint-env.sh")?;
    let script = project_root().join("tools/remove_doubao_watermark.py");
    let files: Vec<String> = storage.last_files.lock().map_err(|e| e.to_string())?.clone();
    let root: PathBuf = storage
        .target_root
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("还没有处理记录")?;
    if files.is_empty() {
        return Err("没有可清理的记录".into());
    }
    let workdir = std::env::temp_dir().join("doubao-watermark-work");
    let mut command = Command::new(&python);
    command
        .arg(&script)
        .arg("--root")
        .arg(&root)
        .env("DOUBAO_WATERMARK_WORKDIR", &workdir)
        .arg("cleanup");
    for file in &files {
        command.arg(file);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
fn read_image_base64(path: String) -> Result<String, String> {
    let file = Path::new(&path);
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
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    if bytes.len() > 10 * 1024 * 1024 {
        return Err("图片过大，无法预览".into());
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:image/png;base64,{}", encoded))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppStorage::default())
        .invoke_handler(tauri::generate_handler![
            env_status,
            setup_env,
            list_pngs,
            run_pipeline,
            cleanup_pipeline,
            read_image_base64
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    link_macos_clang_rt();
    tauri_build::build()
}

/// ort-sys rc.15 移除了 `clang_rt.osx` 链接（pykeio/ort#625 "remove
/// unnecessary linking to libclang_rt"），但官方下载的 ONNX Runtime 静态库
/// （CoreML 部分 host_utils.mm / model.mm）引用 clang builtin 符号
/// `___isPlatformVersionAtLeast`，macOS release 链接实测 Undefined symbols
/// （ort rc.13 自链 clang_rt.osx 故无此问题）。这里用与 ort-sys rc.13 相同的
/// 方式定位 rtlib 目录并补回链接——仅 mac target 生效（按 target_os 判定，
/// 交叉编译 Android/Windows 不受影响）。
fn link_macos_clang_rt() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    use std::path::Path;
    use std::process::Command;

    let cc = std::env::var("CC").unwrap_or_else(|_| "clang".to_string());
    let Ok(output) = Command::new(&cc).arg("--print-search-dirs").output() else {
        println!("cargo:warning=无法运行 `{cc} --print-search-dirs`，跳过 libclang_rt.osx 补链");
        return;
    };
    if !output.status.success() {
        println!("cargo:warning=`{cc} --print-search-dirs` 退出非零，跳过 libclang_rt.osx 补链");
        return;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("libraries: =") {
            let dir = format!("{}/lib/darwin", path.trim_end());
            if Path::new(&dir).is_dir() {
                println!("cargo:rustc-link-search={dir}");
                println!("cargo:rustc-link-lib=clang_rt.osx");
            } else {
                println!("cargo:warning=rtlib 目录不存在: {dir}，跳过 libclang_rt.osx 补链");
            }
            return;
        }
    }
    println!("cargo:warning=clang 输出中未找到 libraries 行，跳过 libclang_rt.osx 补链");
}

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

pub fn download_model(dest: &Path, url: &str, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let response = ureq::get(url)
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|e| format!("连接失败 {}: {}", url, e))?;
    let total: u64 = response
        .header("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut reader = response.into_reader();
    let tmp = dest.with_extension("part");
    let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 65536];
    let mut downloaded: u64 = 0;
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        downloaded += read as u64;
        progress(downloaded, total);
    }
    file.flush().ok();
    drop(file);
    fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

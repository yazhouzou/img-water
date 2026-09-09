use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// 并行分段下载数
const SEGMENTS: u64 = 6;
/// 总大小超过该值才启用多连接（小于则单连接足够）
const PARALLEL_MIN_SIZE: u64 = 16 * 1024 * 1024;
/// 每个分段的重试次数
const SEGMENT_RETRIES: u32 = 3;
const CHUNK: usize = 64 * 1024;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(90))
        .build()
}

/// 依次尝试 urls 列表中的下载源，任一成功即返回。
/// 进度回调返回 (已下载字节, 总字节)，总字节未知时为 0。
pub fn download_model(dest: &Path, urls: &[String], progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut errors: Vec<String> = Vec::new();
    for url in urls {
        match download_from(dest, url, progress) {
            Ok(()) => return Ok(()),
            Err(err) => {
                cleanup_parts(dest);
                errors.push(format!("{}: {}", url, err));
            }
        }
    }
    Err(format!("全部下载源均失败 → {}", errors.join(" | ")))
}

fn cleanup_parts(dest: &Path) {
    let dir = match dest.parent() {
        Some(d) => d,
        None => return,
    };
    let stem = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&stem) && (name.ends_with(".part") || is_numbered_part(&name)) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn is_numbered_part(name: &str) -> bool {
    name.rsplitn(2, '.').next().map(|s| s.bytes().all(|b| b.is_ascii_digit())).unwrap_or(false)
}

fn download_from(dest: &Path, url: &str, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), String> {
    let agent = agent();
    // 先用 HEAD 探测大小与是否支持 Range
    let probe = agent
        .head(url)
        .call()
        .map_err(|e| format!("连接失败: {}", e))?;
    let total: u64 = probe
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let accept_ranges = probe
        .header("accept-ranges")
        .map(|v| v.to_ascii_lowercase().contains("bytes"))
        .unwrap_or(false);

    if total > 0 && accept_ranges && total >= PARALLEL_MIN_SIZE {
        download_parallel(&agent, url, dest, total, progress)
    } else {
        download_stream(&agent, url, dest, total, progress)
    }
}

fn download_stream(agent: &ureq::Agent, url: &str, dest: &Path, total: u64, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), String> {
    let response = agent
        .get(url)
        .call()
        .map_err(|e| format!("请求失败: {}", e))?;
    let reported: u64 = response
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(total);
    let mut reader = response.into_reader();
    let tmp = dest.with_extension("part");
    let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; CHUNK];
    let mut downloaded: u64 = 0;
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        downloaded += read as u64;
        progress(downloaded, reported);
    }
    file.flush().ok();
    drop(file);
    if reported > 0 && downloaded != reported {
        return Err(format!("下载不完整：{} / {} 字节", downloaded, reported));
    }
    fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

fn download_parallel(agent: &ureq::Agent, url: &str, dest: &Path, total: u64, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), String> {
    let seg_size = total.div_ceil(SEGMENTS);
    let seg_count = total.div_ceil(seg_size);
    let done = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));

    let mut join_errs: Vec<String> = Vec::new();
    thread::scope(|s| {
        // 后台进度上报线程
        let reporter_done = done.clone();
        let reporter_stop = stop.clone();
        s.spawn(move || {
            while !reporter_stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(300));
                progress(reporter_done.load(Ordering::Relaxed), total);
            }
        });

        // 分段下载线程
        let mut handles = Vec::new();
        for i in 0..seg_count {
            let seg_start = i * seg_size;
            let seg_end = ((i + 1) * seg_size).min(total) - 1;
            let agent = agent.clone();
            let url = url.to_string();
            let done = done.clone();
            let failed = failed.clone();
            let part: PathBuf = dest.with_extension(format!("part{}", i));
            handles.push(s.spawn(move || -> Result<(), String> {
                for attempt in 0..SEGMENT_RETRIES {
                    if failed.load(Ordering::Relaxed) {
                        return Err("其它分段失败，提前终止".into());
                    }
                    match fetch_range(&agent, &url, seg_start, seg_end, &part, &done) {
                        Ok(()) => return Ok(()),
                        Err(err) => {
                            if attempt == SEGMENT_RETRIES - 1 {
                                failed.store(true, Ordering::Relaxed);
                                return Err(err);
                            }
                            thread::sleep(Duration::from_secs(1));
                        }
                    }
                }
                unreachable!()
            }));
        }

        for handle in handles {
            if let Err(err) = handle.join().unwrap_or_else(|_| Err("分段线程崩溃".into())) {
                join_errs.push(err);
            }
        }
        stop.store(true, Ordering::Relaxed);
    });
    progress(done.load(Ordering::Relaxed), total);

    if !join_errs.is_empty() {
        return Err(format!("分段下载失败：{}", join_errs.join("；")));
    }

    // 合并分段文件
    let combined = dest.with_extension("part");
    let mut out = fs::File::create(&combined).map_err(|e| e.to_string())?;
    for i in 0..seg_count {
        let part: PathBuf = dest.with_extension(format!("part{}", i));
        let data = fs::read(&part).map_err(|e| e.to_string())?;
        out.write_all(&data).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&part);
    }
    out.flush().ok();
    drop(out);
    if combined.metadata().map(|m| m.len()).unwrap_or(0) != total {
        return Err(format!("合并后大小不符：{} / {} 字节", combined.metadata().map(|m| m.len()).unwrap_or(0), total));
    }
    fs::rename(&combined, dest).map_err(|e| e.to_string())?;
    Ok(())
}

fn fetch_range(agent: &ureq::Agent, url: &str, seg_start: u64, seg_end: u64, part: &Path, done: &AtomicU64) -> Result<(), String> {
    let seg_total = seg_end - seg_start + 1;
    let existing = if part.exists() {
        fs::metadata(part).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    if existing >= seg_total {
        return Ok(());
    }
    let range_start = seg_start + existing;
    let response = agent
        .get(url)
        .set("Range", &format!("bytes={}-{}", range_start, seg_end))
        .call()
        .map_err(|e| format!("Range 请求失败: {}", e))?;
    if response.status() != 206 {
        return Err(format!("服务器未返回 206 分段响应（status {}）", response.status()));
    }
    let mut reader = response.into_reader();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(part)
        .map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        done.fetch_add(read as u64, Ordering::Relaxed);
    }
    file.flush().ok();
    let written = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if written < seg_total {
        return Err(format!("分段不完整：{} / {} 字节", written, seg_total));
    }
    Ok(())
}

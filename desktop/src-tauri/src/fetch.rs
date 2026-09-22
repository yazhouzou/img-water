use serde::{Deserialize, Serialize};
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

/// 断点续传的伴随元数据：只有远端文件"可确认是同一个"才敢沿用已下载的字节。
///
/// 缺了它，切换下载源、或远端文件被替换后，会拿旧分段的字节拼出新文件——大小可能刚好
/// 对得上，内容却是错的（ONNX 加载失败或推理出垃圾），属于**静默损坏**。故一旦无法确认
/// 同一性就丢弃分段重下（代价只是重下，不会留下坏模型）。
#[derive(Serialize, Deserialize, Default, PartialEq, Eq, Debug, Clone)]
struct PartMeta {
    /// 上次使用的下载源（仅作诊断，不参与同一性判定——换镜像仍可续传）
    url: String,
    /// 远端文件总大小（0 = 未知，此时不允许续传）
    total: u64,
    /// 远端 ETag（拿不到时为空串）
    etag: String,
}

fn meta_path(dest: &Path) -> PathBuf {
    dest.with_extension("part.json")
}

fn read_meta(dest: &Path) -> Option<PartMeta> {
    serde_json::from_str(&fs::read_to_string(meta_path(dest)).ok()?).ok()
}

fn write_meta(dest: &Path, meta: &PartMeta) {
    if let Ok(text) = serde_json::to_string(meta) {
        let _ = fs::write(meta_path(dest), text);
    }
}

/// 旧分段能否续传：总大小必须一致且已知；双方都有 ETag 时以 ETag 为准判定"是不是同一个
/// 文件"（大小相同内容不同也能挡住），有一方拿不到 ETag 才退化为"大小一致"。
fn meta_compatible(old: &PartMeta, new: &PartMeta) -> bool {
    if new.total == 0 || old.total != new.total {
        return false;
    }
    if !old.etag.is_empty() && !new.etag.is_empty() {
        return old.etag == new.etag;
    }
    true
}

/// 该文件名是否为本模型下载的临时产物：`.part`（单连接）/ `.part.json`（元数据）/
/// `.partN`（并行分段）/ `.part.merge`（并行合并中转）。
fn is_temp_of(name: &str, stem: &str) -> bool {
    let Some(rest) = name.strip_prefix(stem).and_then(|r| r.strip_prefix(".part")) else {
        return false;
    };
    rest.is_empty()
        || rest == ".json"
        || rest == ".merge"
        || (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

fn prune_temps(dest: &Path) {
    let Some(dir) = dest.parent() else { return };
    // 临时文件由 `with_extension` 生成，名字基于**去扩展名的 stem**（`a.b.onnx` → `a.b.part0`）
    let stem = dest
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_temp_of(&name, &stem) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// 依次尝试 urls 列表中的下载源，任一成功即返回。
/// 进度回调返回 (已下载字节, 总字节)，总字节未知时为 0。
///
/// **失败不清理分段**：下次重试（含重启 App / 换回同一源）从断点继续；只有"无法确认是
/// 同一个远端文件"时才丢弃（见 `meta_compatible`）。
pub fn download_model(dest: &Path, urls: &[String], progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut errors: Vec<String> = Vec::new();
    for url in urls {
        match download_from(dest, url, progress) {
            Ok(()) => return Ok(()),
            Err(err) => errors.push(format!("{}: {}", url, err)),
        }
    }
    Err(format!(
        "全部下载源均失败（已保留断点，可重试续传） → {}",
        errors.join(" | ")
    ))
}

fn download_from(dest: &Path, url: &str, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<(), String> {
    download_from_with(dest, url, progress, PARALLEL_MIN_SIZE)
}

fn download_from_with(
    dest: &Path,
    url: &str,
    progress: &(dyn Fn(u64, u64) + Sync),
    parallel_min: u64,
) -> Result<(), String> {
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
    let etag = probe.header("etag").unwrap_or_default().to_string();

    let meta = PartMeta {
        url: url.to_string(),
        total,
        etag,
    };
    // 续传门槛：远端文件必须"可确认是同一个"，否则丢弃旧分段重下（防拼出损坏模型）。
    let resumable = read_meta(dest)
        .map(|old| meta_compatible(&old, &meta))
        .unwrap_or(false);
    if !resumable {
        prune_temps(dest);
    }
    write_meta(dest, &meta);

    let result = if total > 0 && accept_ranges && total >= parallel_min {
        download_parallel(&agent, url, dest, total, progress)
    } else {
        download_stream(&agent, url, dest, total, accept_ranges, progress)
    };
    if result.is_ok() {
        prune_temps(dest);
    }
    result
}

fn download_stream(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    total: u64,
    accept_ranges: bool,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    let tmp = dest.with_extension("part");
    let mut existing = fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    // 只有大小已知、服务器支持 Range、且现有字节是有用的前缀时才续传。
    let mut resume = accept_ranges && total > 0 && existing > 0 && existing < total;

    let request = |from: u64| {
        let req = agent.get(url);
        if from > 0 {
            req.set("Range", &format!("bytes={}-", from)).call()
        } else {
            req.call()
        }
    };
    let mut response = request(if resume { existing } else { 0 })
        .map_err(|e| format!("请求失败: {}", e))?;
    if resume && response.status() != 206 {
        // 服务器忽略了 Range（回了 200 全量）：清掉半截文件从头下，绝不能把全量追加其后。
        let _ = fs::remove_file(&tmp);
        existing = 0;
        resume = false;
        response = request(0).map_err(|e| format!("请求失败: {}", e))?;
    }

    let reported: u64 = response
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // HEAD 的总大小优先（GET 的 content-length 可能被截断的响应"自圆其说"）
    let expected_total = if total > 0 {
        total
    } else if resume {
        existing + reported
    } else {
        reported
    };

    let mut reader = response.into_reader();
    let mut file = if resume {
        fs::OpenOptions::new()
            .append(true)
            .open(&tmp)
            .map_err(|e| e.to_string())?
    } else {
        fs::File::create(&tmp).map_err(|e| e.to_string())?
    };
    let mut buffer = vec![0u8; CHUNK];
    let mut downloaded: u64 = existing;
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        downloaded += read as u64;
        progress(downloaded, expected_total);
    }
    file.flush().ok();
    drop(file);
    if expected_total > 0 && downloaded != expected_total {
        return Err(format!(
            "下载不完整：{} / {} 字节（已保留断点，下次续传）",
            downloaded, expected_total
        ));
    }
    fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

fn download_parallel(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    total: u64,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(), String> {
    let seg_size = total.div_ceil(SEGMENTS);
    let seg_count = total.div_ceil(seg_size);
    // 续传：把已有分段的字节计入起点，进度条从上次的位置接着走（而不是从 0 跳一下）
    let mut seeded: u64 = 0;
    for i in 0..seg_count {
        let part: PathBuf = dest.with_extension(format!("part{}", i));
        if let Ok(meta) = fs::metadata(&part) {
            seeded += meta.len().min(seg_size);
        }
    }
    let done = Arc::new(AtomicU64::new(seeded.min(total)));
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
        return Err(format!("分段下载失败（已保留断点，下次续传）：{}", join_errs.join("；")));
    }

    // 合并分段：先写中转文件，**全部成功后再改名**，最后才删分段。中途崩溃也不会
    // 丢掉已下好的分段（旧实现边合并边删，崩在中间就得全部重下）。
    let merged = dest.with_extension("part.merge");
    let mut out = fs::File::create(&merged).map_err(|e| e.to_string())?;
    for i in 0..seg_count {
        let part: PathBuf = dest.with_extension(format!("part{}", i));
        let data = fs::read(&part).map_err(|e| e.to_string())?;
        out.write_all(&data).map_err(|e| e.to_string())?;
    }
    out.flush().ok();
    drop(out);
    let merged_len = merged.metadata().map(|m| m.len()).unwrap_or(0);
    if merged_len != total {
        return Err(format!("合并后大小不符：{} / {} 字节", merged_len, total));
    }
    fs::rename(&merged, dest).map_err(|e| e.to_string())?;
    for i in 0..seg_count {
        let _ = fs::remove_file(dest.with_extension(format!("part{}", i)));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fetch-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn handle(stream: &mut TcpStream, body: &[u8], etag: &str, truncate: Option<usize>) {
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let mut lines = req.lines();
        let first = lines.next().unwrap_or("").to_string();
        let is_head = first.starts_with("HEAD");
        let range: Option<(usize, usize)> = lines
            .find_map(|l| {
                let low = l.to_ascii_lowercase();
                low.strip_prefix("range:")
                    .map(|v| v.trim().to_string())
            })
            .and_then(|r| {
                let spec = r.strip_prefix("bytes=")?.to_string();
                let (s, e) = spec.split_once('-')?;
                let start: usize = s.parse().ok()?;
                let end: usize = e.parse().unwrap_or(usize::MAX);
                Some((start, end))
            });
        let start = range.map(|(s, _)| s).unwrap_or(0);
        let stop = range
            .and_then(|(_, e)| (e != usize::MAX).then(|| e + 1))
            .unwrap_or(body.len())
            .min(body.len());
        let rest = &body[start.min(body.len())..stop.max(start.min(body.len()))];
        let send_len = match truncate {
            Some(t) if !is_head => rest.len().min(t),
            _ => rest.len(),
        };
        if is_head {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"{}\"\r\nConnection: close\r\n\r\n",
                body.len(),
                etag
            );
        } else {
            // 只要请求带了 Range 就必须回 206（哪怕从 0 开始），否则客户端会当成"服务器
            // 不支持分段"（200 意味着全量）
            let status = if range.is_some() { "206 Partial Content" } else { "200 OK" };
            let _ = write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"{}\"\r\nConnection: close\r\n\r\n",
                status,
                send_len,
                etag
            );
            let _ = stream.write_all(&rest[..send_len]);
        }
        let _ = stream.flush();
    }

    /// 起一个只服务本地回环的最小 HTTP 源（支持 HEAD / GET / Range，可模拟"传到一半断连"）。
    fn spawn_server(body: Vec<u8>, etag: &'static str, truncate: Option<usize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        handle(&mut stream, &body, etag, truncate);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        format!("http://{}/model.onnx", addr)
    }

    #[test]
    fn temp_file_names_are_recognised() {
        for name in ["m.onnx.part", "m.onnx.part.json", "m.onnx.part0", "m.onnx.part11", "m.onnx.part.merge"] {
            assert!(is_temp_of(name, "m.onnx"), "{} 应算临时产物", name);
        }
        for name in ["m.onnx", "m.onnx.bak", "m.onnx.partx", "m.onnx.part.json2", "other.onnx.part0"] {
            assert!(!is_temp_of(name, "m.onnx"), "{} 不应算临时产物", name);
        }
    }

    #[test]
    fn resume_requires_same_remote_file() {
        let base = PartMeta { url: "a".into(), total: 100, etag: "W/\"x\"".into() };
        // 同源、同大小、同 ETag → 续传
        assert!(meta_compatible(&base, &PartMeta { url: "b".into(), ..base.clone() }));
        // 大小不同 → 重下
        assert!(!meta_compatible(&base, &PartMeta { total: 101, ..base.clone() }));
        // ETag 不同（远端换了文件）→ 重下
        assert!(!meta_compatible(&base, &PartMeta { etag: "W/\"y\"".into(), ..base.clone() }));
        // 总大小未知 → 不允许续传
        assert!(!meta_compatible(&base, &PartMeta { total: 0, ..base.clone() }));
        // 有一方拿不到 ETag → 退化为"大小一致即可"
        assert!(meta_compatible(&base, &PartMeta { etag: String::new(), ..base.clone() }));
    }

    /// 单连接路径：传到一半断连 → 保留断点；重试从断点续传并得到逐字节正确的文件。
    #[test]
    fn stream_download_resumes_after_truncation() {
        let body = payload(200_000);
        let etag = "W/\"stream\"";
        let url = spawn_server(body.clone(), etag, Some(60_000));
        let dir = tmp_dir("stream");
        let dest = dir.join("lama_fp32.onnx");

        let err = download_from(&dest, &url, &|_, _| {}).unwrap_err();
        assert!(err.contains("不完整"), "应报下载不完整，实际: {}", err);
        let tmp = dest.with_extension("part");
        assert_eq!(fs::metadata(&tmp).unwrap().len(), 60_000, "断点应被保留");

        // 第二次：同一源、同一 ETag → 续传（服务器不再截断）
        let url2 = spawn_server(body.clone(), etag, None);
        let seen = AtomicU64::new(0);
        download_from(&dest, &url2, &|done, _total| {
            seen.store(done, Ordering::Relaxed);
        })
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body, "续传后内容必须逐字节正确");
        assert_eq!(seen.load(Ordering::Relaxed), 200_000, "进度应接着断点走到 100%");
        assert!(!tmp.exists(), "成功后应清掉临时文件");
        assert!(!meta_path(&dest).exists(), "成功后应清掉元数据");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 远端文件变了（ETag 不同）→ 必须丢弃旧断点，绝不能把两个版本拼在一起。
    #[test]
    fn stale_parts_are_discarded_when_remote_changed() {
        let dir = tmp_dir("stale");
        let dest = dir.join("lama_fp32.onnx");
        let tmp = dest.with_extension("part");
        fs::write(&tmp, payload(60_000)).unwrap();
        write_meta(
            &dest,
            &PartMeta { url: "old".into(), total: 200_000, etag: "W/\"old\"".into() },
        );

        let body = payload(200_000);
        let url = spawn_server(body.clone(), "W/\"new\"", None);
        download_from(&dest, &url, &|_, _| {}).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert!(!tmp.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 分段（并行）路径：小阈值强制走 6 段，内容逐字节正确且不留临时文件。
    #[test]
    fn parallel_download_merges_segments() {
        let body = payload(300_000);
        let url = spawn_server(body.clone(), "W/\"par\"", None);
        let dir = tmp_dir("parallel");
        let dest = dir.join("lama_fp32.onnx");
        // parallel_min=1 → 强制多连接
        download_from_with(&dest, &url, &|_, _| {}, 1).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        for i in 0..6 {
            assert!(!dest.with_extension(format!("part{}", i)).exists(), "分段 {} 应被清理", i);
        }
        assert!(!dest.with_extension("part.merge").exists());
        assert!(!meta_path(&dest).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 截断响应的 content-length 会"自圆其说"（谎报为已发出的长度）：必须以 HEAD 的总
    /// 大小为准，否则半截文件会被当成完整文件改名落盘。
    #[test]
    fn truncated_response_never_becomes_final_file() {
        let body = payload(120_000);
        let url = spawn_server(body.clone(), "W/\"trunc\"", Some(50_000));
        let dir = tmp_dir("trunc");
        let dest = dir.join("lama_fp32.onnx");
        assert!(download_from(&dest, &url, &|_, _| {}).is_err());
        assert!(!dest.exists(), "半截文件绝不能落盘");
        let _ = fs::remove_dir_all(&dir);
    }
}

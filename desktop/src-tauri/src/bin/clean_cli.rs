use std::path::PathBuf;
use std::process::exit;

use doubao_clean::pipeline::{self, MaskBox, PipelineOptions};
use doubao_clean::watermark_profiles as wp;

fn parse_box(value: &str) -> MaskBox {
    let parts: Vec<i64> = value.split(',').map(|p| p.trim().parse::<i64>().unwrap_or_else(|_| {
        eprintln!("box must be four integers: x1,y1,x2,y2");
        exit(1);
    })).collect();
    if parts.len() != 4 {
        eprintln!("box must be four integers: x1,y1,x2,y2");
        exit(1);
    }
    MaskBox { x1: parts[0], y1: parts[1], x2: parts[2], y2: parts[3] }
}

fn parse_triple(value: &str) -> [f32; 3] {
    let parts: Vec<f32> = value.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if parts.len() != 3 {
        eprintln!("expected three comma-separated numbers (r,g,b): {value}");
        exit(1);
    }
    [parts[0], parts[1], parts[2]]
}

fn opt_box(value: Option<MaskBox>) -> Option<(i64, i64, i64, i64)> {
    value.map(|b| (b.x1, b.y1, b.x2, b.y2))
}

fn main() {
    let mut root: Option<String> = None;
    let mut mask_box: Option<MaskBox> = None;
    let mut box_raw: Option<MaskBox> = None;
    let mut keep_work = false;
    let mut overwrite = false;
    let mut any_position = false;
    let mut use_profile = true;
    let mut force = false;
    let mut no_inverse = false;
    let mut no_retry = false;
    let mut refine = false;
    let mut profile: Option<String> = None;
    let mut model: Option<String> = None;
    let mut command: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut label: Option<String> = None;
    let mut ref_short: Option<f64> = None;
    let mut bg = [0f32, 0f32, 0f32];
    let mut color = [255f32, 255f32, 255f32];
    let mut mine = MineOpts { min_samples: 3, corner_tol: 0.05, recursive: false, dry_run: false, out: None };

    let mut args = std::env::args().skip(1);
    while let Some(raw) = args.next() {
        // 支持 `--opt value` 与 `--opt=value` 两种形式（后者与 argparse 对齐，
        // 便于传负数 --mask-box=-120,-90,-10,-10）
        let (arg, inline): (String, Option<String>) = match raw.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (raw.clone(), None),
        };
        let take = |args: &mut std::iter::Skip<std::env::Args>| inline.clone().or_else(|| args.next());
        match arg.as_str() {
            "--root" => root = take(&mut args),
            "--mask-box" => mask_box = Some(take(&mut args).map(|v| parse_box(&v)).unwrap_or_else(|| {
                eprintln!("--mask-box needs a value");
                exit(1);
            })),
            "--box" => box_raw = Some(take(&mut args).map(|v| parse_box(&v)).unwrap_or_else(|| {
                eprintln!("--box needs a value");
                exit(1);
            })),
            "--label" | "--label-prefix" => label = take(&mut args),
            "--ref-short" => ref_short = take(&mut args).and_then(|v| v.parse().ok()),
            "--bg" => bg = take(&mut args).map(|v| parse_triple(&v)).unwrap_or(bg),
            "--color" => color = take(&mut args).map(|v| parse_triple(&v)).unwrap_or(color),
            "--keep-work" => keep_work = true,
            "--overwrite" | "--in-place" => overwrite = true,
            "--any-position" => any_position = true,
            "--no-profile" | "--no-profiles" => use_profile = false,
            "--force" => force = true,
            "--no-inverse" => no_inverse = true,
            "--no-retry" => no_retry = true,
            "--refine" => refine = true,
            "--profile" => profile = take(&mut args),
            "--model" => model = take(&mut args),
            "--min-samples" => mine.min_samples = take(&mut args).and_then(|v| v.parse().ok()).unwrap_or(mine.min_samples),
            "--corner-tol" => mine.corner_tol = take(&mut args).and_then(|v| v.parse().ok()).unwrap_or(mine.corner_tol),
            "--recursive" => mine.recursive = true,
            "--dry-run" => mine.dry_run = true,
            "--out" => mine.out = take(&mut args),
            "-h" | "--help" => {
                print_help();
                return;
            }
            other => {
                if command.is_none() {
                    command = Some(other.to_string());
                } else {
                    files.push(other.to_string());
                }
            }
        }
    }

    let command = command.unwrap_or_else(|| {
        eprintln!("missing command; use run|prepare|inpaint|review-lama|overwrite-review|cleanup|profiles|match|learn-pair|learn-solid|learn-auto|learn-batch|learn-mine");
        exit(1);
    });

    // 档案库子命令（不依赖 root/model）
    if let Some(code) = run_profile_command(&command, &files, label.as_deref(), ref_short, box_raw, bg, color, &mine) {
        exit(code);
    }

    let root_path: PathBuf = root
        .map(PathBuf::from)
        .unwrap_or_else(doubao_clean::project_root);
    if !root_path.exists() {
        eprintln!("root folder not found: {}", root_path.display());
        exit(1);
    }

    let model_path: PathBuf = model.map(PathBuf::from).unwrap_or_else(doubao_clean::model_path);
    let options = PipelineOptions {
        root: root_path.clone(),
        files,
        keep_work,
        mask_box,
        any_position,
        overwrite_original: overwrite,
        use_profile,
        force,
        inverse: !no_inverse,
        retry: !no_retry,
        refine,
        forced_profile: profile,
    };

    let log = |line: &str| println!("{}", line);
    let no_progress = |_: &str, _: usize, _: usize, _: &str| {};
    let not_cancelled = || false;
    let result = match command.as_str() {
        "run" => pipeline::run(&options, &model_path, &log, &no_progress, &not_cancelled).map(|summary| {
            println!(
                "source review: {}\ncandidate review: {}\nfinal review: {}\noutput dir: {}",
                summary.source_review.display(),
                summary.candidate_review.display(),
                summary.final_review.display(),
                summary.output_dir.display()
            );
        }),
        "prepare" => {
            let names = pipeline::target_names(&options.root, &options.files).unwrap_or_else(|e| exit_with(&e));
            pipeline::prepare(&options, &names, &log)
        }
        "inpaint" => pipeline::inpaint(&model_path, !no_inverse, &log, &no_progress, &not_cancelled),
        "review-lama" => {
            let names = pipeline::target_names(&options.root, &options.files).unwrap_or_else(|e| exit_with(&e));
            pipeline::review_lama(&names, &root_path).map(|_| ())
        }
        "overwrite-review" => {
            let names = pipeline::target_names(&options.root, &options.files).unwrap_or_else(|e| exit_with(&e));
            pipeline::finalize_outputs(&options, &names, &log).map(|_| ())
        }
        "cleanup" => {
            let names = pipeline::target_names(&options.root, &options.files).unwrap_or_else(|e| exit_with(&e));
            pipeline::cleanup(&options, &names).and_then(|_| pipeline::cleanup_preserved())
        }
        other => {
            eprintln!("unknown command: {}", other);
            exit(1);
        }
    };
    if let Err(err) = result {
        eprintln!("{}", err);
        exit(1);
    }
}

/// `learn-mine` 的开关（文件夹自动聚类建档）。
struct MineOpts {
    min_samples: usize,
    corner_tol: f64,
    recursive: bool,
    dry_run: bool,
    out: Option<String>,
}

fn is_image_path(p: &std::path::Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "bmp" | "webp" | "tif" | "tiff")
    )
}

/// 把参数里的目录展开成图片文件（`--recursive` 决定是否下钻）；非目录、非图片的
/// 原样保留，好让 `image::open` 报出到底是哪个文件打不开。
fn expand_inputs(files: &[String], recursive: bool) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = files.iter().map(std::path::PathBuf::from).collect();
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            let Ok(rd) = std::fs::read_dir(&p) else { continue };
            let mut entries: Vec<std::path::PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            entries.sort();
            for e in entries {
                if e.is_dir() {
                    if recursive {
                        stack.push(e);
                    }
                } else if is_image_path(&e) {
                    out.push(e);
                }
            }
        } else {
            out.push(p);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 返回 Some(exit_code) 表示这是档案库子命令，已处理完毕。
fn run_profile_command(
    command: &str,
    files: &[String],
    label: Option<&str>,
    ref_short: Option<f64>,
    box_raw: Option<MaskBox>,
    bg: [f32; 3],
    color: [f32; 3],
    mine: &MineOpts,
) -> Option<i32> {
    match command {
        "profiles" | "list-profiles" => {
            let list = wp::list_profiles();
            if list.is_empty() {
                println!("no profiles in {}", wp::profiles_dir().display());
            }
            for p in list {
                println!(
                    "{}: label={:?} shape=({},{}) ref_short={} source={}",
                    p.id, p.label, p.ah, p.aw, p.ref_short_side, p.source
                );
            }
            Some(0)
        }
        "match" => {
            let path = files.first().unwrap_or_else(|| {
                eprintln!("match needs an image path");
                exit(1);
            });
            match image::open(path) {
                Ok(img) => match wp::match_image(&img) {
                    Some((p, px, py, score, scale)) => {
                        println!("matched {} at ({},{}) score {:.3} scale {:.3}", p.id, px, py, score, scale);
                    }
                    None => println!("no profile matched"),
                },
                Err(e) => {
                    eprintln!("failed to open {path}: {e}");
                    return Some(1);
                }
            }
            Some(0)
        }
        "match-profile" => {
            if files.len() < 2 {
                eprintln!("match-profile needs <id> <image>");
                return Some(1);
            }
            let (id, path) = (files[0].clone(), files[1].clone());
            match image::open(&path) {
                Ok(img) => match wp::match_specific(&img, &id) {
                    Some((p, px, py, score, scale)) => {
                        println!("matched {} at ({},{}) score {:.3} scale {:.3}", p.id, px, py, score, scale);
                    }
                    None => println!("profile {id} not located (score below {:.2})", wp::FORCED_MIN_SCORE),
                },
                Err(e) => {
                    eprintln!("failed to open {path}: {e}");
                    return Some(1);
                }
            }
            Some(0)
        }
        "learn-pair" => {
            if files.len() < 2 {
                eprintln!("learn-pair needs <black.png> <white.png> [--label X]");
                return Some(1);
            }
            let label = label.unwrap_or("learned");
            let (profile, report) = match wp::extract_from_pair(
                std::path::Path::new(&files[0]),
                std::path::Path::new(&files[1]),
                opt_box(box_raw),
                ref_short,
                label,
                24,
                6.0,
            ) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("pair learning failed: {e}");
                    return Some(1);
                }
            };
            println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            match profile {
                None => {
                    eprintln!("pair learning failed; not saving a profile");
                    Some(1)
                }
                Some(p) => match wp::save_profile(&p, true) {
                    Ok(base) => {
                        println!("saved profile {} -> {}", p.id, base.display());
                        Some(0)
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        Some(1)
                    }
                },
            }
        }
        "learn-solid" => {
            if files.is_empty() {
                eprintln!("learn-solid needs <image> --label X [--bg r,g,b] [--color r,g,b]");
                return Some(1);
            }
            let label = label.unwrap_or("learned");
            match wp::extract_from_solid(
                std::path::Path::new(&files[0]),
                opt_box(box_raw),
                bg,
                color,
                ref_short,
                label,
                24,
            ) {
                Ok(p) => match wp::save_profile(&p, true) {
                    Ok(base) => {
                        println!("saved profile {} -> {}", p.id, base.display());
                        Some(0)
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        Some(1)
                    }
                },
                Err(e) => {
                    eprintln!("{e}");
                    Some(1)
                }
            }
        }
        "learn-mine" => {
            if files.is_empty() {
                eprintln!("learn-mine needs <dir|images...> [--label-prefix X]");
                return Some(1);
            }
            let paths = expand_inputs(files, mine.recursive);
            if paths.is_empty() {
                eprintln!("learn-mine: no images found");
                return Some(1);
            }
            let prefix = label.unwrap_or("mined");
            let out = mine
                .out
                .clone()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| doubao_clean::workdir().join("mined-review"));
            match wp::mine_watermarks(
                &paths,
                prefix,
                color,
                ref_short,
                opt_box(box_raw),
                mine.min_samples,
                12.0,
                mine.corner_tol,
                Some(&out),
                !mine.dry_run,
            ) {
                Ok((profiles, report)) => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
                    for p in &profiles {
                        println!("mined profile {} shape=({},{}) ref_short={}", p.id, p.ah, p.aw, p.ref_short_side);
                    }
                    println!(
                        "{} profile(s) {}; review montages in {}",
                        profiles.len(),
                        if mine.dry_run { "learned (dry-run, not saved)" } else { "saved" },
                        out.display()
                    );
                    Some(0)
                }
                Err(e) => {
                    eprintln!("{e}");
                    Some(1)
                }
            }
        }
        "learn-auto" => {
            if files.is_empty() {
                eprintln!("learn-auto needs <images...> --label X");
                return Some(1);
            }
            let label = label.unwrap_or("learned");
            let frames: Vec<(String, PathBuf, Option<(i64, i64, i64, i64)>)> = files
                .iter()
                .map(|f| {
                    let path = PathBuf::from(f);
                    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    (name, path, opt_box(box_raw))
                })
                .collect();
            match wp::auto_discover(&frames, label, color, ref_short, 24, 12.0, 0.01, 60.0) {
                Ok((Some(p), report)) => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
                    match wp::save_profile(&p, true) {
                        Ok(base) => {
                            println!("saved profile {} -> {}", p.id, base.display());
                            Some(0)
                        }
                        Err(e) => {
                            eprintln!("{e}");
                            Some(1)
                        }
                    }
                }
                Ok((None, report)) => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
                    eprintln!("auto learning failed; not saving a profile");
                    Some(1)
                }
                Err(e) => {
                    eprintln!("{e}");
                    Some(1)
                }
            }
        }
        "learn-batch" => {
            if files.is_empty() {
                eprintln!("learn-batch needs <images...> --label X");
                return Some(1);
            }
            let label = label.unwrap_or("learned");
            let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
            match wp::learn_from_batch(&paths, opt_box(box_raw), ref_short, label, 24, 12.0) {
                Ok((Some(p), report)) => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
                    match wp::save_profile(&p, true) {
                        Ok(base) => {
                            println!("saved profile {} -> {}", p.id, base.display());
                            Some(0)
                        }
                        Err(e) => {
                            eprintln!("{e}");
                            Some(1)
                        }
                    }
                }
                Ok((None, report)) => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
                    eprintln!("batch learning failed; not saving a profile");
                    Some(1)
                }
                Err(e) => {
                    eprintln!("{e}");
                    Some(1)
                }
            }
        }
        _ => None,
    }
}

fn print_help() {
    println!("usage: clean-cli [--root <dir>] [--mask-box x1,y1,x2,y2] [--any-position] [--refine] [--profile <id>] [--no-profile] [--no-inverse] [--no-retry] [--force] [--keep-work] [--overwrite] [--model <onnx>] <command> [files...]");
    println!("  run|prepare|inpaint|review-lama|overwrite-review|cleanup");
    println!("默认结果另存到 <root>/watermark-cleaned/；--overwrite 直接覆盖原图（自动备份 original-watermark-backup/）");
    println!("--any-position: 处理任意位置的文字水印（OCR 检测），默认只处理贴右下角的豆包水印");
    println!("--no-profile: 跳过水印档案库（逐像素逆解），只用模板/检测 + 生成式修复");
    println!("--no-inverse: 关闭豆包 stamp 逐像素逆解（默认开）");
    println!("--no-retry: 关闭残留自动重试（默认开，最多 1 轮）");
    println!("--refine: 实验性：手动框选时框内笔画精分割（只重绘笔画，失败退回整框）");
    println!("--profile <id>: 精确模式：只用指定水印档案定位（见 profiles 子命令）");
    println!();
    println!("水印档案库（逐像素逆解，去水印且不改周边元素）:");
    println!("  profiles                                  列出已装档案（含内置）");
    println!("  match <image>                             对图片匹配档案");
    println!("  learn-pair <black.png> <white.png> --label <name> [--ref-short N] [--box x1,y1,x2,y2]");
    println!("  learn-solid <image> --label <name> [--bg r,g,b] [--color r,g,b] [--ref-short N]");
    println!("  learn-auto <images...> --label <name> [--color r,g,b] [--ref-short N]");
    println!("  match-profile <id> <image>");
    println!("  learn-batch <images...> --label <name> [--ref-short N]");
    println!("  learn-mine <dir|images...> [--label-prefix <name>] [--min-samples N] [--corner-tol F] [--recursive] [--dry-run] [--out <dir>]");
    println!("     文件夹自动聚类建档：逐图检测水印 → 按「尺寸 + 水印相对右下角位置」聚簇 →");
    println!("     每簇 ≥3 张就学习并自检，只留能定位的档；复查拼图写到 --out（默认 <workdir>/mined-review）");
    println!("     自动聚类只在右下角找水印；水印在别处时用 --box 显式指定（此时按尺寸聚簇）");
    println!("档案目录：{}（可用 WATERMARK_PROFILES_DIR 覆盖）", wp::profiles_dir().display());
}

fn exit_with(message: &str) -> ! {
    eprintln!("{}", message);
    exit(1);
}

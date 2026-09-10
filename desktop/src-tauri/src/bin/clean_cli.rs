use std::path::{Path, PathBuf};
use std::process::exit;

use doubao_clean::pipeline::{self, MaskBox, PipelineOptions};

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

fn main() {
    let mut root: Option<String> = None;
    let mut mask_box: Option<MaskBox> = None;
    let mut keep_work = false;
    let mut model: Option<String> = None;
    let mut command: Option<String> = None;
    let mut files: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next(),
            "--mask-box" => mask_box = Some(args.next().map(|v| parse_box(&v)).unwrap_or_else(|| {
                eprintln!("--mask-box needs a value");
                exit(1);
            })),
            "--keep-work" => keep_work = true,
            "--model" => model = args.next(),
            "-h" | "--help" => {
                println!("usage: clean-cli [--root <dir>] [--mask-box x1,y1,x2,y2] [--keep-work] [--model <onnx>] <run|prepare|inpaint|review-lama|overwrite-review|cleanup> [files...]");
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
        eprintln!("missing command; use run|prepare|inpaint|review-lama|overwrite-review|cleanup");
        exit(1);
    });

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
    };

    let log = |line: &str| println!("{}", line);
    let result = match command.as_str() {
        "run" => pipeline::run(&options, &model_path, &log).map(|summary| {
            println!(
                "source review: {}\ncandidate review: {}\nfinal review: {}",
                summary.source_review.display(),
                summary.candidate_review.display(),
                summary.final_review.display()
            );
        }),
        "prepare" => {
            let names = pipeline::target_names(&options.root, &options.files).unwrap_or_else(|e| exit_with(&e));
            pipeline::prepare(&options, &names, &log)
        }
        "inpaint" => pipeline::inpaint(&model_path, &log),
        "review-lama" => {
            let names = pipeline::target_names(&options.root, &options.files).unwrap_or_else(|e| exit_with(&e));
            pipeline::review_lama(&names).map(|_| ())
        }
        "overwrite-review" => {
            let names = pipeline::target_names(&options.root, &options.files).unwrap_or_else(|e| exit_with(&e));
            pipeline::overwrite_review(&options, &names).map(|_| ())
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

fn exit_with(message: &str) -> ! {
    eprintln!("{}", message);
    exit(1);
}

#[allow(dead_code)]
fn _unused(_p: &Path) {}

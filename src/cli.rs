use std::path::Path;
use crate::catalog::{load_folder, Mark};
use crate::xmp::write_mark;

#[derive(clap::ValueEnum, Clone)]
pub enum MarkArg {
    Pick,
    Reject,
    None,
}

impl From<MarkArg> for Mark {
    fn from(m: MarkArg) -> Self {
        match m {
            MarkArg::Pick => Mark::Pick,
            MarkArg::Reject => Mark::Reject,
            MarkArg::None => Mark::None,
        }
    }
}

pub fn cmd_picks(folder: &Path) {
    for img in load_folder(folder).iter().filter(|i| i.mark == Mark::Pick) {
        println!("{}", img.path.display());
    }
}

pub fn cmd_stats(folder: &Path) {
    let images = load_folder(folder);
    let picks = images.iter().filter(|i| i.mark == Mark::Pick).count();
    let rejects = images.iter().filter(|i| i.mark == Mark::Reject).count();
    let unrated = images.iter().filter(|i| i.mark == Mark::None).count();
    let total = images.len();
    println!("total:   {total}");
    println!("picks:   {picks}");
    println!("rejects: {rejects}");
    println!("unrated: {unrated}");
}

pub fn cmd_export(folder: &Path) {
    let images = load_folder(folder);
    let picks: Vec<_> = images.iter().filter(|i| i.mark == Mark::Pick).collect();

    if picks.is_empty() {
        eprintln!("no picks found in {}", folder.display());
        std::process::exit(1);
    }

    let dest_dir = folder.join("_picks");
    std::fs::create_dir_all(&dest_dir).unwrap_or_else(|e| {
        eprintln!("failed to create _picks/: {e}");
        std::process::exit(1);
    });

    let mut copied = 0usize;
    for img in &picks {
        if let Some(name) = img.path.file_name() {
            match std::fs::copy(&img.path, dest_dir.join(name)) {
                Ok(_) => {
                    println!("{}", img.path.display());
                    copied += 1;
                }
                Err(e) => eprintln!("skip {}: {e}", img.path.display()),
            }
        }
    }
    eprintln!("{copied} files copied to {}/", dest_dir.display());
}

pub fn cmd_mark(file: &Path, mark: MarkArg) {
    if !file.exists() {
        eprintln!("file not found: {}", file.display());
        std::process::exit(1);
    }
    write_mark(file, &mark.into());
    println!("{}", file.display());
}

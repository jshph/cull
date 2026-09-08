use crate::catalog::{load_folder, Mark};
use crate::xmp::write_mark;
use std::path::Path;

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
    match crate::shoot::export_snapshot(folder) {
        Ok(path) => println!("{}", path.display()),
        Err(error) => {
            eprintln!("Export failed: {error:#}");
            std::process::exit(1);
        }
    }
}

pub fn cmd_mark(file: &Path, mark: MarkArg) {
    if !file.exists() {
        eprintln!("file not found: {}", file.display());
        std::process::exit(1);
    }
    if let Err(error) = write_mark(file, &mark.into()) {
        eprintln!("Could not save decision: {error:#}");
        std::process::exit(1);
    }
    println!("{}", file.display());
}

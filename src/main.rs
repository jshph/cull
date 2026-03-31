// Only hide the console window on Windows release GUI builds.
// CLI mode needs it visible, so we handle this at runtime instead.

mod app;
mod catalog;
mod cli;
mod preview;
mod xmp;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cull", about = "Blazing-fast photo culling")]
struct Cli {
    /// Open the GUI with this folder pre-loaded
    folder: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List all picked files (one path per line)
    Picks { folder: PathBuf },

    /// Show pick / reject / unrated counts
    Stats { folder: PathBuf },

    /// Copy all picks to <folder>/_picks/
    Export { folder: PathBuf },

    /// Mark a file: pick | reject | none
    Mark {
        file: PathBuf,
        #[arg(value_enum)]
        mark: cli::MarkArg,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Picks { folder }) => cli::cmd_picks(&folder),
        Some(Command::Stats { folder }) => cli::cmd_stats(&folder),
        Some(Command::Export { folder }) => cli::cmd_export(&folder),
        Some(Command::Mark { file, mark }) => cli::cmd_mark(&file, mark),
        None => run_gui(cli.folder),
    }
}

fn run_gui(preload: Option<PathBuf>) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cull")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "Cull",
        options,
        Box::new(move |cc| Ok(Box::new(app::CullApp::new(cc, preload.clone())))),
    )
    .unwrap();
}

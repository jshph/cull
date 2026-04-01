// Only hide the console window on Windows release GUI builds.
// CLI mode needs it visible, so we handle this at runtime instead.

mod app;
mod catalog;
mod cli;
mod exif;
mod license;
mod preview;
mod update;
mod xmp;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

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
        None => {
            // If a folder was explicitly passed, open it. Otherwise launch empty
            // — avoids scanning CWD (which is "/" when launched from Finder).
            let folder = cli.folder.map(|f| std::fs::canonicalize(&f).unwrap_or(f));
            run_gui(folder);
        }
    }
}

/// Install a `cull` symlink into /usr/local/bin if running from an .app bundle
/// and the symlink doesn't already exist.
fn install_cli_symlink() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };

    // Only do this when running from a .app bundle (path contains ".app/Contents/MacOS")
    let exe_str = exe.to_string_lossy();
    if !exe_str.contains(".app/Contents/MacOS") {
        return;
    }

    let symlink = Path::new("/usr/local/bin/cull");
    if symlink.exists() {
        // Already installed — check if it points to us
        if let Ok(target) = std::fs::read_link(symlink) {
            if target == exe {
                return; // already correct
            }
        }
        return; // exists but points elsewhere, don't overwrite
    }

    // Create /usr/local/bin if needed, then symlink
    // This may fail without admin privileges — that's fine, we just skip
    let _ = std::fs::create_dir_all("/usr/local/bin");
    match std::os::unix::fs::symlink(&exe, symlink) {
        Ok(_) => eprintln!("Installed CLI: /usr/local/bin/cull → {}", exe.display()),
        Err(_) => {
            // Try with osascript for admin privileges
            let script = format!(
                "do shell script \"ln -sf '{}' /usr/local/bin/cull\" with administrator privileges",
                exe.display()
            );
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output();
        }
    }
}

fn run_gui(preload: Option<PathBuf>) {
    install_cli_symlink();
    let saved = app::SavedState::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cull")
            .with_inner_size([saved.window_width, saved.window_height])
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

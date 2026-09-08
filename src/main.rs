// Only hide the console window on Windows release GUI builds.
// CLI mode needs it visible, so we handle this at runtime instead.

mod app;
mod catalog;
mod cli;
mod editor;
mod exif;
mod export;
mod license;
mod preview;
mod shoot;
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

    /// Export a fresh picks snapshot into Exports/
    Export { folder: PathBuf },

    /// Create an empty shoot with Originals/ and Exports/
    NewShoot { folder: PathBuf },
    /// Open/refresh native editor views for this shoot
    Handoff {
        folder: PathBuf,
        #[arg(long)]
        editor: String,
        /// Gracefully restart Lightroom Local to refresh cached metadata
        #[arg(long)]
        restart_lightroom: bool,
    },
    /// Browse a folder in an editor (Capture One requires an open session)
    OpenFolder { folder: PathBuf, #[arg(long)] editor: String },
    /// Install Cull's Lightroom Classic plugin
    InstallLightroomPlugin,
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
        Some(Command::NewShoot { folder }) => {
            report(crate::shoot::create(&folder).map(|s| s.root.display().to_string()))
        }
        Some(Command::Handoff { folder, editor, restart_lightroom }) => {
            report(if restart_lightroom {
                crate::editor::restart_lightroom_and_handoff(&folder, &editor)
            } else {
                crate::editor::handoff(&folder, &editor)
            })
        }
        Some(Command::OpenFolder { folder, editor }) => report(crate::editor::open_in_editor(&editor, &[folder]).map(|_| "Opened folder in editor".into())),
        Some(Command::InstallLightroomPlugin) => {
            report(crate::editor::install_lightroom_plugin().map(|p| p.display().to_string()))
        }
        None => {
            // If a folder was explicitly passed, open it. Otherwise launch empty
            // — avoids scanning CWD (which is "/" when launched from Finder).
            let folder = cli.folder.map(|f| std::fs::canonicalize(&f).unwrap_or(f));
            run_gui(folder);
        }
    }
}

fn report(result: anyhow::Result<String>) {
    match result {
        Ok(message) => println!("{message}"),
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
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

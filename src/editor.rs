use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

fn open_command(editor: &str, paths: &[PathBuf]) -> Command {
    let mut command = Command::new("/usr/bin/open");
    command.arg("-a").arg(editor).arg("--").args(paths);
    command
}

fn run_open(command: &mut Command) -> Result<()> {
    let output = command.output().context("Could not start macOS open")?;
    if !output.status.success() {
        bail!(
            "macOS open failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn open_in_editor(editor: &str, paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        bail!("No photos or folder to open");
    }
    for path in paths {
        if !path.exists() {
            bail!("Path no longer exists: {}", path.display());
        }
    }
    if paths.len() == 1 && paths[0].is_dir() && editor.to_lowercase().contains("capture one") {
        return browse_capture_one(editor, &paths[0]);
    }
    let name = editor.to_lowercase();
    if name.contains("lightroom") && !name.contains("classic") {
        // Lightroom routes the same open event to Cloud import or Local browse
        // depending on its active tab. It exposes no documented mode selector.
        // Bring it forward without files, then let the user establish Local mode.
        run_open(&mut open_command(editor, &[]))?;
        let answer = rfd::MessageDialog::new()
            .set_title("Open in Lightroom Local")
            .set_description("In Lightroom, select Local at the top left, then return here and continue.\n\nLightroom must be in Local mode to browse this folder. Cloud mode opens an import dialog instead.")
            .set_buttons(rfd::MessageButtons::OkCancelCustom("Open Local folder".into(), "Cancel".into()))
            .show();
        if answer != rfd::MessageDialogResult::Custom("Open Local folder".into()) {
            bail!("Lightroom handoff canceled; no files sent");
        }
    }
    // Never silently widen a large selection to the whole folder.
    run_open(&mut open_command(editor, paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_selection_and_spaces_are_preserved_exactly() {
        let paths: Vec<_> = (0..501)
            .map(|i| PathBuf::from(format!("/photos with spaces/{i}.DNG")))
            .collect();
        let command = open_command("/Applications/Capture One 22.app", &paths);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args.len(), 504);
        assert_eq!(args[1], "/Applications/Capture One 22.app");
        assert_eq!(args[3], paths[0].as_os_str());
        assert_eq!(args[503], paths[500].as_os_str());
    }

    #[test]
    fn nonzero_exit_is_an_error_even_when_spawn_succeeds() {
        assert!(run_open(&mut Command::new("/usr/bin/false")).is_err());
    }
}

fn apple_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn color_rule(colors: &[u8]) -> String {
    let conditions: String = colors.iter().map(|color| format!("<Condition Enabled=\"YES\"><Key>IB_S_BASIC_URGENCY</Key><Operator>0</Operator><Criterion>{color}</Criterion></Condition>")).collect();
    format!("<?xml version=\"1.0\"?><MatchOperator Kind=\"OR\">{conditions}</MatchOperator>")
}

pub fn handoff(folder: &std::path::Path, editor: &str) -> Result<String> {
    if !std::path::Path::new(editor).is_dir() {
        bail!("Editor is not installed: {editor}");
    }
    let shoot = crate::shoot::prepare(folder)?;
    if editor.to_lowercase().contains("capture one") {
        capture_one(&shoot, editor)
    } else if editor.to_lowercase().contains("lightroom classic") {
        lightroom(&shoot, editor)
    } else if editor.to_lowercase().contains("lightroom") {
        let photos = shoot.decisions()?;
        if photos.is_empty() {
            bail!("No supported photos in this shoot");
        }
        for photo in &photos {
            crate::xmp::migrate_native_flag(&photo.path)?;
        }
        shoot.write_manifest(&shoot.decisions()?)?;
        open_in_editor(editor, &[shoot.originals_path()])?;
        Ok("Sent to Lightroom Local; restart via Shoot for stale flags".into())
    } else {
        bail!("Shoot integration requires Capture One or Lightroom");
    }
}

/// Explicit user action: gracefully restart Local's in-memory metadata cache.
/// Never force-quit: cancellation, permission errors, or a busy editor fail closed.
pub fn restart_lightroom_and_handoff(folder: &std::path::Path, editor: &str) -> Result<String> {
    let name = editor.to_lowercase();
    if !name.contains("lightroom") || name.contains("classic") || !std::path::Path::new(editor).is_dir() {
        bail!("Choose the installed Lightroom (Local) editor");
    }
    let shoot = crate::shoot::prepare(folder)?;
    if shoot.decisions()?.is_empty() {
        bail!("No supported photos in this shoot");
    }
    let app = apple_literal(editor);
    let script = format!("with timeout of 90 seconds\nif application {app} is running then\ntell application {app} to quit\nend if\nrepeat 120 times\nif not (application {app} is running) then return\ndelay 0.25\nend repeat\nerror \"Lightroom did not quit; finish any pending dialog and retry\"\nend timeout");
    let output = Command::new("/usr/bin/osascript").arg("-e").arg(script).output()
        .context("Could not request Lightroom restart")?;
    if !output.status.success() {
        bail!("Lightroom restart failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    handoff(folder, editor)?;
    Ok("Restarted Lightroom; opened shoot in Local".into())
}

fn capture_one(shoot: &crate::shoot::Shoot, editor: &str) -> Result<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let photos = shoot.decisions()?;
    if photos.is_empty() {
        bail!("No supported photos in this shoot");
    }
    let container = shoot.state_dir().join("Capture One");
    std::fs::create_dir_all(&container)?;
    let legacy_name = format!("Cull-{}", shoot.id);
    let readable_name: String = shoot
        .name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | ':' | '\\'))
        .take(60)
        .collect();
    let name = if container.join(&legacy_name).is_dir() {
        legacy_name
    } else {
        format!("{readable_name} — Cull {}", shoot.id)
    };
    let session = container.join(&name).join(format!("{name}.cosessiondb"));
    let folders: BTreeSet<_> = photos.iter().filter_map(|p| p.path.parent()).collect();
    let inventory: BTreeMap<_, _> = photos.iter().map(|p| (&p.path, true)).collect();
    let mut folder_counts = BTreeMap::new();
    for photo in &photos {
        *folder_counts.entry(photo.path.parent().unwrap()).or_insert(0usize) += 1;
    }
    let decisions: BTreeMap<_, _> = photos
        .iter()
        .filter(|p| p.authored)
        .map(|p| {
            let color = match p.metadata.mark {
                crate::catalog::Mark::Pick => 4,
                crate::catalog::Mark::Reject => 1,
                crate::catalog::Mark::None => match p.metadata.label.to_lowercase().as_str() {
                    "orange" => 2,
                    "yellow" => 3,
                    "blue" => 5,
                    "pink" => 6,
                    "purple" => 7,
                    _ => 0,
                },
            };
            (&p.path, color)
        })
        .collect();
    let config = serde_json::json!({"name":name,"container":container,"session":session,
        "originals":shoot.originals_path(),"exports":shoot.exports_path(),"folders":folders,"decisions":decisions,
        "labels":shoot.state_dir().join("capture-one-labels.json"),
        "inventory":inventory,"folderCounts":folder_counts,
        "rules":{"Picks":color_rule(&[4]),"Rejects":color_rule(&[1]),"Unmarked":color_rule(&[0,2,3,5,6,7])}});
    let config_path = shoot.state_dir().join("capture-one.json");
    crate::xmp::atomic_write(&config_path, &serde_json::to_vec_pretty(&config)?)?;
    let script = include_str!("../integrations/capture-one/handoff.applescript")
        .replace("__EDITOR__", &apple_literal(editor));
    let mut file = tempfile::NamedTempFile::new()?;
    use std::io::Write;
    file.write_all(script.as_bytes())?;
    let output = Command::new("/usr/bin/osascript")
        .arg(file.path())
        .arg(&config_path)
        .output()
        .context("Could not run Capture One integration")?;
    if !output.status.success() {
        bail!(
            "Capture One handoff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

pub fn install_lightroom_plugin() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("Home folder unavailable")?;
    let path = PathBuf::from(home)
        .join("Library/Application Support/Adobe/Lightroom/Modules/Cull.lrplugin");
    std::fs::create_dir_all(&path)?;
    for (name, contents) in [
        (
            "Info.lua",
            include_str!("../integrations/Cull.lrplugin/Info.lua"),
        ),
        (
            "Metadata.lua",
            include_str!("../integrations/Cull.lrplugin/Metadata.lua"),
        ),
        (
            "Manifest.lua",
            include_str!("../integrations/Cull.lrplugin/Manifest.lua"),
        ),
        (
            "Handoff.lua",
            include_str!("../integrations/Cull.lrplugin/Handoff.lua"),
        ),
        (
            "Run.lua",
            include_str!("../integrations/Cull.lrplugin/Run.lua"),
        ),
        (
            "Open.lua",
            include_str!("../integrations/Cull.lrplugin/Open.lua"),
        ),
        (
            "UrlHandler.lua",
            include_str!("../integrations/Cull.lrplugin/UrlHandler.lua"),
        ),
    ] {
        crate::xmp::atomic_write(&path.join(name), contents.as_bytes())?;
    }
    Ok(path)
}

fn lightroom(shoot: &crate::shoot::Shoot, editor: &str) -> Result<String> {
    install_lightroom_plugin()?;
    // Explicit app target prevents the cloud Lightroom URL handler intercepting.
    let path = shoot
        .manifest_path()
        .to_string_lossy()
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect::<String>();
    let url = format!("lightroom://com.getcull.shoot/open?manifest={path}");
    run_open(Command::new("/usr/bin/open").arg("-a").arg(editor).arg(url))?;
    Ok("Sent shoot to Lightroom Classic. First use: enable Cull in File → Plug-in Manager, then retry; Lightroom confirms completed import.".into())
}

/// Capture One's ordinary open-document event can acknowledge a folder without
/// navigating its current session. Use the native browse command and verify it.
fn browse_capture_one(editor: &str, folder: &std::path::Path) -> Result<()> {
    let script = r#"use framework "Foundation"
use scripting additions
on run argv
    set folderPath to item 1 of argv
    tell application __EDITOR__
        activate
        if (count of documents) is 0 then error "Open a shoot first to create a Capture One session."
        set targetDoc to current document
        browse targetDoc to path folderPath
        set shownFolder to POSIX path of (folder of current collection of targetDoc as alias)
        set normalized to (current application's NSString's stringWithString:shownFolder)'s stringByStandardizingPath()
        if (normalized as text) is not folderPath then error "Capture One did not select the requested folder."
    end tell
end run"#.replace("__EDITOR__", &apple_literal(editor));
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(script.as_bytes())?;
    let output = Command::new("/usr/bin/osascript").arg(file.path()).arg(folder.canonicalize()?).output()?;
    if !output.status.success() { bail!("Capture One folder navigation failed: {}", String::from_utf8_lossy(&output.stderr).trim()); }
    Ok(())
}

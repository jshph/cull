use crate::{catalog, xmp};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Shoot {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub originals: PathBuf,
    #[serde(skip)]
    pub root: PathBuf,
}

#[derive(Debug)]
pub struct PhotoDecision {
    pub path: PathBuf,
    pub metadata: xmp::Metadata,
    pub authored: bool,
}

impl Shoot {
    pub fn originals_path(&self) -> PathBuf {
        if self.originals == Path::new(".") {
            self.root.clone()
        } else {
            self.root.join(&self.originals)
        }
    }
    pub fn state_dir(&self) -> PathBuf {
        self.root.join(".cull")
    }
    pub fn exports_path(&self) -> PathBuf {
        self.root.join("Exports")
    }
    pub fn manifest_path(&self) -> PathBuf {
        self.state_dir().join("handoff.cull")
    }
    pub fn decisions(&self) -> Result<Vec<PhotoDecision>> {
        catalog::try_load_folder(&self.originals_path())?
            .into_iter()
            .map(|image| {
                let metadata = xmp::read_metadata(&image.path).with_context(|| {
                    format!("Cannot prepare metadata for {}", image.path.display())
                })?;
                let authored = xmp::has_cull_decision(&image.path)?;
                Ok(PhotoDecision {
                    path: image.path,
                    metadata,
                    authored,
                })
            })
            .collect()
    }
    pub fn write_manifest(&self, decisions: &[PhotoDecision]) -> Result<()> {
        let mut text = format!(
            "CULL\t1\t{}\t{}\t{}\n",
            self.id,
            encode(&self.name),
            encode(&self.originals_path().to_string_lossy())
        );
        for decision in decisions {
            let flag = match decision.metadata.mark {
                catalog::Mark::Pick => 1,
                catalog::Mark::Reject => -1,
                catalog::Mark::None => 0,
            };
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                encode(&decision.path.to_string_lossy()),
                flag,
                u8::from(decision.authored),
                encode(&decision.metadata.label),
                decision
                    .metadata
                    .rating
                    .map(|rating| rating.to_string())
                    .unwrap_or_default()
            ));
        }
        xmp::atomic_write(&self.manifest_path(), text.as_bytes())
    }
}

pub fn prepare(folder: &Path) -> Result<Shoot> {
    let folder = folder
        .canonicalize()
        .context("Shoot folder does not exist")?;
    if !folder.is_dir() {
        bail!("Choose a photo folder");
    }
    let root = if folder.file_name().is_some_and(|n| n == "Originals") {
        folder.parent().unwrap().to_path_buf()
    } else {
        folder
    };
    let state = root.join(".cull");
    let config = state.join("shoot.json");
    let mut shoot: Shoot = if config.exists() {
        serde_json::from_slice(&std::fs::read(&config)?).context("Invalid shoot configuration")?
    } else {
        Shoot {
            version: 1,
            id: format!("{:016x}", rand::random::<u64>()),
            name: root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            originals: if root.join("Originals").is_dir() {
                "Originals".into()
            } else {
                ".".into()
            },
            root: root.clone(),
        }
    };
    if shoot.version != 1
        || shoot.id.len() != 16
        || !shoot.id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("Unsupported shoot identity");
    }
    if shoot.originals != Path::new(".") && shoot.originals != Path::new("Originals") {
        bail!("Invalid originals location");
    }
    shoot.root = root;
    if !shoot.originals_path().is_dir() {
        bail!("Originals folder is missing");
    }
    std::fs::create_dir_all(&state)?;
    std::fs::create_dir_all(shoot.exports_path())?;
    xmp::atomic_write(&config, &serde_json::to_vec_pretty(&shoot)?)?;
    let decisions = shoot.decisions()?;
    shoot.write_manifest(&decisions)?;
    Ok(shoot)
}

/// Creates a new empty shoot. Existing shoots are adopted through prepare,
/// never reorganized by moving originals behind either editor's catalog.
pub fn create(root: &Path) -> Result<Shoot> {
    std::fs::create_dir(root).context("Choose a new shoot folder name")?;
    std::fs::create_dir(root.join("Originals"))?;
    prepare(root)
}

pub fn encode(text: &str) -> String {
    let mut result = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"/._- ".contains(&byte) {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

pub fn export_snapshot(folder: &Path) -> Result<PathBuf> {
    let shoot = prepare(folder)?;
    let images = catalog::try_load_folder(&shoot.originals_path())?;
    if !images.iter().any(|i| i.mark == catalog::Mark::Pick) {
        bail!("No picks to export");
    }
    let path = shoot.exports_path().join(format!(
        "Picks-{}-{:04x}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        rand::random::<u16>()
    ));
    let staging = tempfile::Builder::new()
        .prefix(".cull-export-")
        .tempdir_in(shoot.exports_path())?;
    crate::export::export_picks_relative(&images, &shoot.originals_path(), staging.path())?;
    if path.exists() {
        bail!("Snapshot destination already exists; retry export");
    }
    std::fs::rename(staging.path(), &path)
        .context("Could not publish completed export snapshot")?;
    xmp::atomic_write(
        &shoot.state_dir().join("last-export.txt"),
        path.to_string_lossy().as_bytes(),
    )?;
    Ok(path)
}

pub fn latest_export(folder: &Path) -> Option<PathBuf> {
    let folder = folder.canonicalize().ok()?;
    let root = if folder.file_name()? == "Originals" {
        folder.parent()?
    } else {
        &folder
    };
    let path = PathBuf::from(std::fs::read_to_string(root.join(".cull/last-export.txt")).ok()?);
    let path = path.canonicalize().ok()?;
    if path.starts_with(root.join("Exports").canonicalize().ok()?) && path.is_dir() {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_stable_and_never_moves_originals() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.DNG");
        std::fs::write(&file, b"raw").unwrap();
        let first = prepare(dir.path()).unwrap();
        let second = prepare(dir.path()).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.originals, Path::new("."));
        assert_eq!(std::fs::read(file).unwrap(), b"raw");
    }
    #[test]
    fn structured_shoot_ignores_exports_and_preserves_identity_from_originals() {
        let dir = tempfile::tempdir().unwrap();
        let shoot = create(&dir.path().join("Shoot")).unwrap();
        std::fs::write(shoot.originals_path().join("a.DNG"), b"raw").unwrap();
        std::fs::write(shoot.exports_path().join("old.DNG"), b"raw").unwrap();
        assert_eq!(prepare(&shoot.originals_path()).unwrap().id, shoot.id);
        assert_eq!(shoot.decisions().unwrap().len(), 1);
    }
    #[test]
    fn percent_encoding_cannot_add_rows_or_execute_code() {
        assert_eq!(encode("a\t\n%\"東京"), "a%09%0A%25%22%E6%9D%B1%E4%BA%AC");
    }
    #[test]
    fn snapshots_have_exact_current_picks_preserve_subfolders_and_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let shoot = create(&dir.path().join("Shoot")).unwrap();
        let root = shoot.originals_path();
        let a = root.join("same.DNG");
        let b = root.join("nested/same.DNG");
        std::fs::create_dir(root.join("nested")).unwrap();
        for (file, content) in [(&a, "first"), (&b, "second")] {
            std::fs::write(file, content).unwrap();
            xmp::write_mark(file, &catalog::Mark::Pick).unwrap();
        }
        let first = export_snapshot(&shoot.root).unwrap();
        assert_eq!(std::fs::read(first.join("same.DNG")).unwrap(), b"first");
        assert_eq!(
            std::fs::read(first.join("nested/same.DNG")).unwrap(),
            b"second"
        );
        assert_eq!(
            std::fs::read(first.join("same.xmp")).unwrap(),
            std::fs::read(root.join("same.xmp")).unwrap()
        );
        xmp::write_mark(&a, &catalog::Mark::Reject).unwrap();
        let second = export_snapshot(&root).unwrap();
        assert_ne!(first, second);
        assert!(!second.join("same.DNG").exists());
        assert!(first.join("same.DNG").exists());
        assert!(second.join("nested/same.DNG").exists());
        assert_eq!(latest_export(&root), Some(second));
        assert_eq!(catalog::load_folder(&shoot.root).len(), 2);
        assert_eq!(std::fs::read(&a).unwrap(), b"first");
        xmp::write_mark(&b, &catalog::Mark::None).unwrap();
        assert!(export_snapshot(&root).is_err());
    }
}

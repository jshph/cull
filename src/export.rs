use crate::catalog::{ImageEntry, Mark};
use anyhow::{bail, Context, Result};
use std::path::Path;

/// A fresh delivery snapshot preserves subfolders and copies each shared RAW /
/// JPEG sidecar once. It never overwrites an earlier delivery or source photo.
pub fn export_picks_relative(
    images: &[ImageEntry],
    originals: &Path,
    destination: &Path,
) -> Result<usize> {
    let picks: Vec<_> = images
        .iter()
        .filter(|image| image.mark == Mark::Pick)
        .collect();
    if picks.is_empty() {
        bail!("No picks to export");
    }
    for image in &picks {
        let relative = image
            .path
            .strip_prefix(originals)
            .context("Photo is outside the shoot originals")?;
        let target = destination.join(relative);
        if target.exists() {
            bail!("Snapshot already contains {}", target.display());
        }
        std::fs::create_dir_all(target.parent().unwrap())?;
        std::fs::copy(&image.path, &target)?;
        crate::xmp::copy_metadata(&image.path, &target)?;
    }
    Ok(picks.len())
}

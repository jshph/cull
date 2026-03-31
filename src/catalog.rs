use std::path::{Path, PathBuf};

/// RAW and JPEG extensions we handle.
const IMAGE_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "orf", "rw2", "dng", "raf", "pef", "srw",
    "jpg", "jpeg",
];

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Mark {
    #[default]
    None,
    Pick,
    Reject,
}

#[derive(Debug, Clone)]
pub struct ImageEntry {
    pub path: PathBuf,
    pub mark: Mark,
}

impl ImageEntry {
    pub fn filename(&self) -> &str {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
    }
}

pub fn load_folder(folder: &Path) -> Vec<ImageEntry> {
    walkdir::WalkDir::new(folder)
        .max_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let path = e.path().to_path_buf();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase());

            let is_image = IMAGE_EXTS.contains(&ext.as_deref().unwrap_or(""));
            if is_image {
                let mark = crate::xmp::read_mark(&path).unwrap_or_default();
                Some(ImageEntry { path, mark })
            } else {
                None
            }
        })
        .collect()
}

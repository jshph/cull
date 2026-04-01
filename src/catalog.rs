use std::path::{Path, PathBuf};
use std::time::SystemTime;

const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "orf", "rw2", "dng", "raf", "pef", "srw",
];
const JPEG_EXTS: &[&str] = &["jpg", "jpeg"];

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
    /// 0 = no rotation, 1 = 90° CCW, 2 = 180°, 3 = 90° CW
    pub rotation: u8,
    /// File modification time — cameras set this to capture time.
    pub modified: SystemTime,
    /// Keywords / tags — stored as dc:subject in XMP.
    pub tags: Vec<String>,
}

impl ImageEntry {
    pub fn filename(&self) -> &str {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
    }

    fn ext_lower(&self) -> String {
        self.path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default()
    }

    pub fn is_raw(&self) -> bool {
        RAW_EXTS.contains(&self.ext_lower().as_str())
    }

    pub fn is_jpeg(&self) -> bool {
        JPEG_EXTS.contains(&self.ext_lower().as_str())
    }
}

fn collect_images(folder: &Path, max_depth: usize) -> Vec<ImageEntry> {
    let all_exts: Vec<&str> = RAW_EXTS.iter().chain(JPEG_EXTS.iter()).cloned().collect();

    walkdir::WalkDir::new(folder)
        .max_depth(max_depth)
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

            let is_image = all_exts.contains(&ext.as_deref().unwrap_or(""));
            if is_image {
                let (mark, rotation, tags) = crate::xmp::read_sidecar(&path).unwrap_or_default();
                let modified = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                Some(ImageEntry { path, mark, rotation, modified, tags })
            } else {
                None
            }
        })
        .collect()
}

pub fn load_folder(folder: &Path) -> Vec<ImageEntry> {
    let images = collect_images(folder, 1);
    if images.is_empty() {
        // No direct images — gather from all subfolders
        collect_images(folder, usize::MAX)
    } else {
        images
    }
}

use std::path::Path;
use anyhow::{anyhow, Result};

/// Max dimension for the full display texture. Keeps memory sane for 50MP cameras.
const MAX_DIM: u32 = 2400;

/// Max dimension for filmstrip thumbnails.
/// 300px decoded image → 240 KB RGBA upload vs 15 MB for the full preview.
const THUMB_DIM: u32 = 300;

/// Full-resolution preview for the main view (2400px max).
pub fn load_preview(path: &Path) -> Result<egui::ColorImage> {
    decode_jpeg(extract_jpeg(path)?, MAX_DIM)
}

/// Tiny thumbnail for the filmstrip (300px max).
/// Same JPEG extract + decode path — win is the much smaller GPU upload.
pub fn load_thumbnail(path: &Path) -> Result<egui::ColorImage> {
    decode_jpeg(extract_jpeg(path)?, THUMB_DIM)
}

fn decode_jpeg(jpeg_bytes: Vec<u8>, max_dim: u32) -> Result<egui::ColorImage> {
    let img = image::load_from_memory(&jpeg_bytes)
        .map_err(|e| anyhow!("decode failed: {e}"))?;

    let img = if img.width() > max_dim || img.height() > max_dim {
        img.thumbnail(max_dim, max_dim)
    } else {
        img
    };

    let rgba = img.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, &pixels))
}

fn extract_jpeg(path: &Path) -> Result<Vec<u8>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    // Plain JPEG — read directly
    if matches!(ext.as_deref(), Some("jpg" | "jpeg")) {
        return Ok(std::fs::read(path)?);
    }

    let data = std::fs::read(path)?;

    // Fuji RAF: proprietary container. The embedded JPEG offset/size are
    // stored explicitly in the header — byte scanning finds thumbnails, not
    // the full-res preview. Parse the header directly instead.
    if data.starts_with(b"FUJIFILMCCD-RAW") {
        return extract_raf_jpeg(&data)
            .ok_or_else(|| anyhow!("no JPEG preview found in RAF file {:?}", path));
    }

    // TIFF-based RAW formats (CR2, NEF, ARW, ORF, DNG, RW2, PEF, SRW…)
    // all embed a full-resolution JPEG. Scan for the largest one.
    find_largest_jpeg(&data)
        .ok_or_else(|| anyhow!("no embedded JPEG found in {:?}", path))
}

/// Fuji RAF header layout (all versions):
///   0x00  16 bytes  magic "FUJIFILMCCD-RAW"
///   0x10   4 bytes  format version
///   0x14   8 bytes  camera model ID
///   0x1C  32 bytes  camera model string
///   0x54   4 bytes  JPEG preview offset (big-endian u32)
///   0x58   4 bytes  JPEG preview length (big-endian u32)
fn extract_raf_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    // Minimum header size check
    if data.len() < 100 {
        return None;
    }

    // Try the standard offset (0x54 / 84) used by all known Fuji bodies
    let jpeg = try_raf_at(data, 84);
    if jpeg.is_some() {
        return jpeg;
    }

    // Older RAF variants (some pre-2010 bodies) used 0x44 / 68
    let jpeg = try_raf_at(data, 68);
    if jpeg.is_some() {
        return jpeg;
    }

    // Last resort: fall back to byte scanner (will at least find a thumbnail)
    find_largest_jpeg(data)
}

fn try_raf_at(data: &[u8], off_field: usize) -> Option<Vec<u8>> {
    let off = u32::from_be_bytes(data[off_field..off_field + 4].try_into().ok()?) as usize;
    let len = u32::from_be_bytes(data[off_field + 4..off_field + 8].try_into().ok()?) as usize;

    if off == 0 || len == 0 || off + len > data.len() {
        return None;
    }
    // Validate it's actually a JPEG
    if data[off] != 0xFF || data[off + 1] != 0xD8 {
        return None;
    }

    Some(data[off..off + len].to_vec())
}

fn find_largest_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    let mut best: Option<(usize, usize)> = None; // (start, len)
    let len = data.len();
    let mut i = 0;

    while i + 3 <= len {
        // JPEG SOI + first marker byte: FF D8 FF xx
        if data[i] == 0xFF && data[i + 1] == 0xD8 && data[i + 2] == 0xFF {
            if let Some(end) = find_jpeg_end(data, i) {
                let jpeg_len = end - i;
                if best.map_or(true, |(_, bl)| jpeg_len > bl) {
                    best = Some((i, jpeg_len));
                }
                i = end; // skip past this JPEG
                continue;
            }
        }
        i += 1;
    }

    best.map(|(start, length)| data[start..start + length].to_vec())
}

fn find_jpeg_end(data: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 2;
    while i + 1 < data.len() {
        if data[i] == 0xFF && data[i + 1] == 0xD9 {
            return Some(i + 2);
        }
        i += 1;
    }
    None
}

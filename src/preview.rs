use std::path::Path;
use anyhow::{anyhow, Result};

/// Max dimension for display texture. Keeps memory sane for 50MP cameras.
const MAX_DIM: u32 = 2400;

/// Load a displayable ColorImage from any supported image or RAW file.
/// For RAW files, extracts the embedded JPEG preview (no RAW decoding).
pub fn load_preview(path: &Path) -> Result<egui::ColorImage> {
    let jpeg_bytes = extract_jpeg(path)?;

    let img = image::load_from_memory(&jpeg_bytes)
        .map_err(|e| anyhow!("decode failed: {e}"))?;

    // Downscale large previews — 24MP embedded JPEGs are ~6000x4000 = 96MB RGBA
    let img = if img.width() > MAX_DIM || img.height() > MAX_DIM {
        img.thumbnail(MAX_DIM, MAX_DIM)
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

    // RAW formats (CR2, NEF, ARW, DNG, etc.) all embed a full-resolution JPEG.
    // Locate it by scanning for JPEG SOI (FF D8 FF) and EOI (FF D9) markers.
    // We take the largest JPEG found, which is always the full-res preview.
    let data = std::fs::read(path)?;
    find_largest_jpeg(&data)
        .ok_or_else(|| anyhow!("no embedded JPEG found in {:?}", path))
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

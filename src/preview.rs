use anyhow::{anyhow, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAX_DIM: u32 = 2400;
const THUMB_DIM: u32 = 300;

// ── public API ─────────────────────────────────────────────────────────────

/// Full-resolution preview for the main view.
pub fn load_preview(path: &Path, rotation: u8) -> Result<egui::ColorImage> {
    decode_jpeg(extract_full_jpeg(path)?, MAX_DIM, rotation)
}

/// Filmstrip thumbnail. Uses tiny embedded thumbnails from file headers where
/// possible (format-specific, targeted seeks) so decode is nearly instant.
/// Falls back to decoding the full preview at small size.
pub fn load_thumbnail(path: &Path, rotation: u8) -> Result<egui::ColorImage> {
    if let Some(tiny) = extract_tiny_jpeg(path) {
        if let Ok(img) = decode_jpeg(tiny, THUMB_DIM, rotation) {
            return Ok(img);
        }
    }
    decode_jpeg(extract_full_jpeg(path)?, THUMB_DIM, rotation)
}

// ── shared decode ─────────────────────────────────────────────────────────

fn decode_jpeg(jpeg: Vec<u8>, max_dim: u32, rotation: u8) -> Result<egui::ColorImage> {
    let img = image::load_from_memory(&jpeg).map_err(|e| anyhow!("decode failed: {e}"))?;

    // Apply rotation before downscaling so aspect ratio is correct after rotate
    let img = match rotation {
        1 => img.rotate270(), // 90° CCW
        2 => img.rotate180(),
        3 => img.rotate90(), // 90° CW
        _ => img,
    };

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

// ── full preview extraction (existing logic) ──────────────────────────────

fn extract_full_jpeg(path: &Path) -> Result<Vec<u8>> {
    if matches!(ext(path).as_str(), "jpg" | "jpeg") {
        return Ok(std::fs::read(path)?);
    }
    let data = std::fs::read(path)?;
    if data.starts_with(b"FUJIFILMCCD-RAW") {
        return extract_raf_full_jpeg(&data)
            .ok_or_else(|| anyhow!("no JPEG preview in RAF {:?}", path));
    }
    find_largest_jpeg(&data).ok_or_else(|| anyhow!("no embedded JPEG in {:?}", path))
}

// ── tiny thumbnail extraction (fast path) ─────────────────────────────────

/// Try to extract a tiny embedded thumbnail without decoding the full preview.
/// Returns None on any failure — caller falls back to full decode.
fn extract_tiny_jpeg(path: &Path) -> Option<Vec<u8>> {
    match ext(path).as_str() {
        "jpg" | "jpeg" => {
            // Standalone JPEGs may have an EXIF thumbnail in APP1.
            // Camera-original JPEGs (Fuji, Canon, Sony, iPhone) always do.
            // Lightroom/Capture One exports usually do too.
            // We only read the first 64 KB — APP markers come before image data.
            let data = read_first_bytes(path, 65536)?;
            exif_thumbnail_in_jpeg(&data)
        }
        "raf" => extract_raf_tiny(path),
        _ => {
            // All other supported formats are TIFF-based (CR2, CR3, NEF, ARW,
            // ORF, RW2, DNG, PEF, SRW). Parse the TIFF IFD chain with targeted
            // seeks — only reads ~200 bytes + the thumbnail itself.
            tiff_ifd1_jpeg_seekable(path)
        }
    }
}

// ── TIFF IFD thumbnail (CR2, NEF, ARW, DNG, ORF, RW2, PEF, SRW) ──────────
//
// TIFF IFD layout:
//   Bytes 0-1:  endianness ("II" = LE, "MM" = BE)
//   Bytes 4-7:  IFD0 offset
//   IFD0:       count u16 + (count × 12-byte entries) + IFD1_offset u32
//   IFD1:       count u16 + entries containing:
//                 tag 0x0201 = JpegInterchangeFormat (thumbnail offset)
//                 tag 0x0202 = JpegInterchangeFormatLength (thumbnail size)

fn tiff_ifd1_jpeg_seekable(path: &Path) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;

    let mut hdr = [0u8; 8];
    f.read_exact(&mut hdr).ok()?;

    let le = match &hdr[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };

    let ifd0_off = rd32(&hdr[4..], le)? as u64;

    // Read IFD0 entry count → skip entries → read IFD1 pointer
    f.seek(SeekFrom::Start(ifd0_off)).ok()?;
    let n0 = rd16_file(&mut f, le)? as i64;
    f.seek(SeekFrom::Current(n0 * 12)).ok()?;
    let ifd1_off = rd32_file(&mut f, le)? as u64;
    if ifd1_off == 0 {
        return None;
    }

    // Parse IFD1 for JPEG thumbnail tags
    f.seek(SeekFrom::Start(ifd1_off)).ok()?;
    let n1 = rd16_file(&mut f, le)? as usize;

    let (mut jpeg_off, mut jpeg_len) = (None::<u32>, None::<u32>);
    for _ in 0..n1 {
        let mut entry = [0u8; 12];
        f.read_exact(&mut entry).ok()?;
        let tag = rd16(&entry[0..], le)?;
        let val = rd32(&entry[8..], le)?;
        match tag {
            0x0201 => jpeg_off = Some(val),
            0x0202 => jpeg_len = Some(val),
            _ => {}
        }
    }

    let off = jpeg_off? as u64;
    let len = jpeg_len? as usize;
    if len == 0 {
        return None;
    }

    f.seek(SeekFrom::Start(off)).ok()?;
    let mut thumb = vec![0u8; len];
    f.read_exact(&mut thumb).ok()?;

    // Validate JPEG magic
    if thumb.get(0..2) == Some(&[0xFF, 0xD8]) {
        Some(thumb)
    } else {
        None
    }
}

// ── RAF thumbnail ──────────────────────────────────────────────────────────
//
// The full preview in RAF is a standard JPEG that contains EXIF with its own
// IFD1 thumbnail. We read only the first 64 KB of the embedded JPEG
// (enough to contain all APP markers) rather than the full multi-MB preview.

fn extract_raf_tiny(path: &Path) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;

    let mut hdr = [0u8; 100];
    f.read_exact(&mut hdr).ok()?;
    if !hdr.starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }

    // Try standard offset 0x54 (84) first, then 0x44 (68) for older bodies.
    // RAF is always big-endian.
    let jpeg_off = [84usize, 68]
        .iter()
        .filter_map(|&o| {
            let v = rd32(&hdr[o..], false)?;
            if v > 0 {
                Some(v as u64)
            } else {
                None
            }
        })
        .find(|&off| {
            f.seek(SeekFrom::Start(off)).ok();
            let mut magic = [0u8; 2];
            f.read_exact(&mut magic).ok();
            magic == [0xFF, 0xD8]
        })?;

    // Read only the first 64 KB of the embedded JPEG — APP markers come first
    f.seek(SeekFrom::Start(jpeg_off)).ok()?;
    let mut buf = vec![0u8; 65536];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);

    exif_thumbnail_in_jpeg(&buf)
}

// ── EXIF / JFIF thumbnail parser ───────────────────────────────────────────
//
// JPEG structure: FF D8  [FF <marker> <len-u16> <data>]...
// APP1 (FF E1) with "Exif\0\0" header contains a TIFF block.
// That TIFF block's IFD1 holds the thumbnail JPEG.

fn exif_thumbnail_in_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    if data.get(0..2) != Some(&[0xFF, 0xD8]) {
        return None;
    }

    let mut i = 2usize;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            break;
        }
        let marker = data[i + 1];
        let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;

        if marker == 0xE1 && seg_len >= 8 {
            // APP1 — check for Exif header
            let payload = &data[i + 4..];
            if payload.get(0..6) == Some(b"Exif\0\0") {
                let tiff = &payload[6..];
                if let Some(thumb) = tiff_ifd1_jpeg_in_mem(tiff) {
                    return Some(thumb);
                }
            }
        }

        if marker == 0xDA {
            break;
        } // SOS — image data starts, no more APPs
        i += 2 + seg_len;
    }
    None
}

/// Parse a TIFF block (in memory) and return the IFD1 JPEG thumbnail.
/// Offsets are relative to the start of `data`.
fn tiff_ifd1_jpeg_in_mem(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 8 {
        return None;
    }

    let le = match data.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };

    let ifd0_off = rd32(&data[4..], le)? as usize;
    let n0 = rd16(&data[ifd0_off..], le)? as usize;

    let ifd1_ptr = ifd0_off + 2 + n0 * 12;
    let ifd1_off = rd32(&data[ifd1_ptr..], le)? as usize;
    if ifd1_off == 0 {
        return None;
    }

    let n1 = rd16(&data[ifd1_off..], le)? as usize;
    let (mut jpeg_off, mut jpeg_len) = (None::<u32>, None::<u32>);

    for i in 0..n1 {
        let e = ifd1_off + 2 + i * 12;
        let tag = rd16(&data[e..], le)?;
        let val = rd32(&data[e + 8..], le)?;
        match tag {
            0x0201 => jpeg_off = Some(val),
            0x0202 => jpeg_len = Some(val),
            _ => {}
        }
    }

    let off = jpeg_off? as usize;
    let len = jpeg_len? as usize;
    if off + len > data.len() || len == 0 {
        return None;
    }
    if data[off] != 0xFF || data[off + 1] != 0xD8 {
        return None;
    }

    Some(data[off..off + len].to_vec())
}

// ── RAF full preview (existing logic, kept here) ──────────────────────────

fn extract_raf_full_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    try_raf_at(data, 84)
        .or_else(|| try_raf_at(data, 68))
        .or_else(|| find_largest_jpeg(data))
}

fn try_raf_at(data: &[u8], off_field: usize) -> Option<Vec<u8>> {
    let off = rd32(&data[off_field..], false)? as usize;
    let len = rd32(&data[off_field + 4..], false)? as usize;
    if off == 0 || len == 0 || off + len > data.len() {
        return None;
    }
    if data[off] != 0xFF || data[off + 1] != 0xD8 {
        return None;
    }
    Some(data[off..off + len].to_vec())
}

// ── JPEG byte scanner (fallback for TIFF-based RAW) ───────────────────────

fn find_largest_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0xFF && data[i + 1] == 0xD8 && data[i + 2] == 0xFF {
            if let Some(end) = jpeg_end(data, i) {
                let len = end - i;
                // DNGs can contain a much larger SOF3 lossless JPEG holding
                // Bayer RAW data. It is not a display preview.
                if is_display_jpeg(&data[i..end]) && best.map_or(true, |(_, bl)| len > bl) {
                    best = Some((i, len));
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    best.map(|(s, l)| data[s..s + l].to_vec())
}

fn jpeg_end(data: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 2;
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        match marker {
            0xD9 => return Some(i + 2),
            0xFF => i += 1,
            0x00 | 0xD0..=0xD7 | 0x01 => i += 2,
            _ => {
                // Skip length-delimited headers (including EXIF thumbnails),
                // then scan entropy data, respecting escaped/restart markers.
                let length = rd16(data.get(i + 2..)?, false)? as usize;
                if length < 2 {
                    return None;
                }
                i = i.checked_add(2 + length)?;
                if i > data.len() {
                    return None;
                }
            }
        }
    }
    None
}

fn is_display_jpeg(data: &[u8]) -> bool {
    let mut i = 2;
    while i + 3 < data.len() && data[i] == 0xFF {
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // Only 8-bit sequential/progressive photographic previews.
        if matches!(marker, 0xC0 | 0xC1 | 0xC2) {
            return data.get(i + 4) == Some(&8);
        }
        if matches!(marker, 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDA | 0xD9) {
            return false;
        }
        let length = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if length < 2 {
            return false;
        }
        i += 2 + length;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode(&[80; 12], 2, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    #[test]
    fn dng_uses_display_preview_instead_of_larger_lossless_raw() {
        let preview = jpeg();
        let mut raw = vec![0xFF, 0xD8, 0xFF, 0xC3, 0, 8, 14, 0, 2, 0, 2, 0];
        raw.extend(vec![0; preview.len() * 2]);
        raw.extend([0xFF, 0xD9]);
        let mut dng = raw;
        dng.extend(&preview);
        assert_eq!(find_largest_jpeg(&dng).unwrap(), preview);
        assert_eq!(
            decode_jpeg(find_largest_jpeg(&dng).unwrap(), MAX_DIM, 0)
                .unwrap()
                .size,
            [2, 2]
        );
    }

    #[test]
    fn embedded_exif_thumbnail_does_not_truncate_outer_preview() {
        let preview = jpeg();
        let thumbnail = jpeg();
        let mut outer = vec![0xFF, 0xD8, 0xFF, 0xE1];
        outer.extend(((thumbnail.len() + 2) as u16).to_be_bytes());
        outer.extend(thumbnail);
        outer.extend(&preview[2..]);
        assert_eq!(find_largest_jpeg(&outer).unwrap(), outer);
    }

    #[test]
    fn truncated_header_is_rejected() {
        assert!(find_largest_jpeg(&[0xFF, 0xD8, 0xFF, 0xE1, 0xFF, 0xFF, 0xFF, 0xD9]).is_none());
    }

    #[test]
    #[ignore = "set CULL_TEST_PHOTO to a local camera file"]
    fn local_camera_preview() {
        let photo = std::env::var_os("CULL_TEST_PHOTO").expect("CULL_TEST_PHOTO");
        let path = Path::new(&photo);
        let full = load_preview(path, 0).unwrap();
        let thumb = load_thumbnail(path, 0).unwrap();
        assert!(full.size[0] > 300 && full.size[1] > 300);
        assert!(thumb.size[0] <= THUMB_DIM as usize && thumb.size[1] <= THUMB_DIM as usize);
    }
}

// ── helpers ───────────────────────────────────────────────────────────────

/// Lowercase file extension — cameras often write .JPG, .ARW, .RAF uppercase.
fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

fn read_first_bytes(path: &Path, n: usize) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; n];
    let read = f.read(&mut buf).ok()?;
    buf.truncate(read);
    Some(buf)
}

// Endian-aware reads from byte slices
fn rd16(data: &[u8], le: bool) -> Option<u16> {
    let b: [u8; 2] = data.get(0..2)?.try_into().ok()?;
    Some(if le {
        u16::from_le_bytes(b)
    } else {
        u16::from_be_bytes(b)
    })
}
fn rd32(data: &[u8], le: bool) -> Option<u32> {
    let b: [u8; 4] = data.get(0..4)?.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(b)
    } else {
        u32::from_be_bytes(b)
    })
}

// Endian-aware reads from file handle
fn rd16_file(f: &mut std::fs::File, le: bool) -> Option<u16> {
    let mut b = [0u8; 2];
    f.read_exact(&mut b).ok()?;
    Some(if le {
        u16::from_le_bytes(b)
    } else {
        u16::from_be_bytes(b)
    })
}
fn rd32_file(f: &mut std::fs::File, le: bool) -> Option<u32> {
    let mut b = [0u8; 4];
    f.read_exact(&mut b).ok()?;
    Some(if le {
        u32::from_le_bytes(b)
    } else {
        u32::from_be_bytes(b)
    })
}

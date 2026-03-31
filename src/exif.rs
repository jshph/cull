//! Lightweight EXIF reader — extracts only the fields we need for filtering.
//! Works on TIFF-based RAW (CR2/NEF/ARW/DNG/ORF/RW2/PEF/SRW), RAF, and JPEG.
//! Uses targeted file seeks — never reads more than ~8 KB per file.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct ExifInfo {
    pub camera: String,    // "FUJIFILM X-T5", "NIKON Z6III", etc.
    pub lens: String,      // "XF35mmF1.4 R" etc.
    pub iso: u32,          // 0 = unknown
    pub focal_mm: f32,     // 0.0 = unknown
}

/// Read EXIF info from an image file. Returns None on any failure.
pub fn read_exif(path: &Path) -> Option<ExifInfo> {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "jpg" | "jpeg" => read_exif_jpeg(path),
        "raf" => read_exif_raf(path),
        _ => read_exif_tiff(path), // CR2, NEF, ARW, DNG, ORF, etc.
    }
}

// ── TIFF-based RAW ────────────────────────────────────────────────────────

fn read_exif_tiff(path: &Path) -> Option<ExifInfo> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut hdr = [0u8; 8];
    f.read_exact(&mut hdr).ok()?;

    let le = match &hdr[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };

    let ifd0_off = rd32(&hdr[4..], le)? as u64;
    parse_tiff_exif(&mut f, le, ifd0_off)
}

// ── JPEG ──────────────────────────────────────────────────────────────────

fn read_exif_jpeg(path: &Path) -> Option<ExifInfo> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 65536];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);

    if buf.get(0..2) != Some(&[0xFF, 0xD8]) { return None; }

    let mut i = 2usize;
    while i + 4 <= buf.len() {
        if buf[i] != 0xFF { break; }
        let marker = buf[i + 1];
        let seg_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;

        if marker == 0xE1 && seg_len >= 8 {
            let payload = &buf[i + 4..];
            if payload.get(0..6) == Some(b"Exif\0\0") {
                let tiff = &payload[6..];
                return parse_tiff_exif_mem(tiff);
            }
        }

        if marker == 0xDA { break; }
        i += 2 + seg_len;
    }
    None
}

// ── RAF ───────────────────────────────────────────────────────────────────

fn read_exif_raf(path: &Path) -> Option<ExifInfo> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut hdr = [0u8; 100];
    f.read_exact(&mut hdr).ok()?;
    if !hdr.starts_with(b"FUJIFILMCCD-RAW") { return None; }

    // Find the embedded JPEG and parse its EXIF
    let jpeg_off = [84usize, 68].iter()
        .filter_map(|&o| {
            let v = rd32(&hdr[o..], false)?;
            if v > 0 { Some(v as u64) } else { None }
        })
        .next()?;

    f.seek(SeekFrom::Start(jpeg_off)).ok()?;
    let mut buf = vec![0u8; 65536];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);

    if buf.get(0..2) != Some(&[0xFF, 0xD8]) { return None; }

    let mut i = 2usize;
    while i + 4 <= buf.len() {
        if buf[i] != 0xFF { break; }
        let marker = buf[i + 1];
        let seg_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        if marker == 0xE1 && seg_len >= 8 {
            let payload = &buf[i + 4..];
            if payload.get(0..6) == Some(b"Exif\0\0") {
                return parse_tiff_exif_mem(&payload[6..]);
            }
        }
        if marker == 0xDA { break; }
        i += 2 + seg_len;
    }
    None
}

// ── TIFF IFD parser (file-based) ──────────────────────────────────────────

fn parse_tiff_exif(f: &mut std::fs::File, le: bool, ifd0_off: u64) -> Option<ExifInfo> {
    let mut info = ExifInfo::default();
    let mut exif_ifd_off: Option<u64> = None;

    // Parse IFD0
    f.seek(SeekFrom::Start(ifd0_off)).ok()?;
    let n = rd16_f(f, le)? as usize;

    for _ in 0..n {
        let mut entry = [0u8; 12];
        f.read_exact(&mut entry).ok()?;
        let tag = rd16(&entry[0..], le)?;
        let typ = rd16(&entry[2..], le)?;
        let count = rd32(&entry[4..], le)? as usize;
        let val = rd32(&entry[8..], le)?;

        match tag {
            0x010F => { // Make
                info.camera = read_string(f, le, typ, count, val)?;
            }
            0x0110 => { // Model
                let model = read_string(f, le, typ, count, val)?;
                if !info.camera.is_empty() && !model.starts_with(&info.camera) {
                    info.camera = format!("{} {}", info.camera.trim(), model.trim());
                } else {
                    info.camera = model;
                }
            }
            0x8769 => { // ExifIFD pointer
                exif_ifd_off = Some(val as u64);
            }
            _ => {}
        }
    }

    // Parse ExifIFD for ISO, focal length, lens
    if let Some(exif_off) = exif_ifd_off {
        f.seek(SeekFrom::Start(exif_off)).ok()?;
        let n = rd16_f(f, le)? as usize;

        for _ in 0..n {
            let mut entry = [0u8; 12];
            f.read_exact(&mut entry).ok()?;
            let tag = rd16(&entry[0..], le)?;
            let typ = rd16(&entry[2..], le)?;
            let count = rd32(&entry[4..], le)? as usize;
            let val = rd32(&entry[8..], le)?;

            match tag {
                0x8827 => { // ISOSpeedRatings
                    info.iso = if typ == 3 { // SHORT
                        rd16(&entry[8..], le).unwrap_or(0) as u32
                    } else {
                        val
                    };
                }
                0x920A => { // FocalLength (RATIONAL = type 5)
                    if typ == 5 {
                        let saved = f.stream_position().ok()?;
                        f.seek(SeekFrom::Start(val as u64)).ok()?;
                        let num = rd32_f(f, le)?;
                        let den = rd32_f(f, le)?;
                        if den > 0 { info.focal_mm = num as f32 / den as f32; }
                        f.seek(SeekFrom::Start(saved)).ok()?;
                    }
                }
                0xA434 => { // LensModel
                    if let Some(s) = read_string(f, le, typ, count, val) {
                        info.lens = s;
                    }
                }
                _ => {}
            }
        }
    }

    info.camera = info.camera.trim().to_string();
    info.lens = info.lens.trim().to_string();
    Some(info)
}

// ── TIFF IFD parser (in-memory, for EXIF inside JPEG) ─────────────────────

fn parse_tiff_exif_mem(data: &[u8]) -> Option<ExifInfo> {
    if data.len() < 8 { return None; }
    let le = match data.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };

    let mut info = ExifInfo::default();
    let ifd0_off = rd32(&data[4..], le)? as usize;
    let n = rd16(&data[ifd0_off..], le)? as usize;

    let mut exif_ifd_off: Option<usize> = None;

    for i in 0..n {
        let e = ifd0_off + 2 + i * 12;
        if e + 12 > data.len() { break; }
        let tag = rd16(&data[e..], le)?;
        let typ = rd16(&data[e + 2..], le)?;
        let count = rd32(&data[e + 4..], le)? as usize;
        let val = rd32(&data[e + 8..], le)? as usize;

        match tag {
            0x010F => { info.camera = read_string_mem(data, le, typ, count, val)?; }
            0x0110 => {
                let model = read_string_mem(data, le, typ, count, val)?;
                if !info.camera.is_empty() && !model.starts_with(&info.camera) {
                    info.camera = format!("{} {}", info.camera.trim(), model.trim());
                } else {
                    info.camera = model;
                }
            }
            0x8769 => { exif_ifd_off = Some(val); }
            _ => {}
        }
    }

    if let Some(exif_off) = exif_ifd_off {
        if exif_off + 2 > data.len() { return Some(info); }
        let n = rd16(&data[exif_off..], le)? as usize;
        for i in 0..n {
            let e = exif_off + 2 + i * 12;
            if e + 12 > data.len() { break; }
            let tag = rd16(&data[e..], le)?;
            let typ = rd16(&data[e + 2..], le)?;
            let count = rd32(&data[e + 4..], le)? as usize;
            let val = rd32(&data[e + 8..], le)? as usize;

            match tag {
                0x8827 => {
                    info.iso = if typ == 3 {
                        rd16(&data[e + 8..], le).unwrap_or(0) as u32
                    } else {
                        val as u32
                    };
                }
                0x920A => {
                    if typ == 5 && val + 8 <= data.len() {
                        let num = rd32(&data[val..], le)?;
                        let den = rd32(&data[val + 4..], le)?;
                        if den > 0 { info.focal_mm = num as f32 / den as f32; }
                    }
                }
                0xA434 => {
                    if let Some(s) = read_string_mem(data, le, typ, count, val) {
                        info.lens = s;
                    }
                }
                _ => {}
            }
        }
    }

    info.camera = info.camera.trim().to_string();
    info.lens = info.lens.trim().to_string();
    Some(info)
}

// ── endian helpers ────────────────────────────────────────────────────────

fn rd16(d: &[u8], le: bool) -> Option<u16> {
    let b: [u8; 2] = d.get(0..2)?.try_into().ok()?;
    Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
}
fn rd32(d: &[u8], le: bool) -> Option<u32> {
    let b: [u8; 4] = d.get(0..4)?.try_into().ok()?;
    Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
}
fn rd16_f(f: &mut std::fs::File, le: bool) -> Option<u16> {
    let mut b = [0u8; 2]; f.read_exact(&mut b).ok()?;
    Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
}
fn rd32_f(f: &mut std::fs::File, le: bool) -> Option<u32> {
    let mut b = [0u8; 4]; f.read_exact(&mut b).ok()?;
    Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
}

/// Read a TIFF ASCII string from file.
fn read_string(f: &mut std::fs::File, le: bool, typ: u16, count: usize, val: u32) -> Option<String> {
    if typ != 2 || count == 0 { return None; }
    let saved = f.stream_position().ok()?;
    if count <= 4 {
        // Value is inline in the 4-byte field
        let bytes = if le { val.to_le_bytes() } else { val.to_be_bytes() };
        let s = String::from_utf8_lossy(&bytes[..count.min(4)]).trim_end_matches('\0').to_string();
        Some(s)
    } else {
        f.seek(SeekFrom::Start(val as u64)).ok()?;
        let mut buf = vec![0u8; count];
        f.read_exact(&mut buf).ok()?;
        f.seek(SeekFrom::Start(saved)).ok()?;
        Some(String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string())
    }
}

/// Read a TIFF ASCII string from memory buffer.
fn read_string_mem(data: &[u8], le: bool, typ: u16, count: usize, val: usize) -> Option<String> {
    if typ != 2 || count == 0 { return None; }
    if count <= 4 {
        let bytes = if le { (val as u32).to_le_bytes() } else { (val as u32).to_be_bytes() };
        Some(String::from_utf8_lossy(&bytes[..count.min(4)]).trim_end_matches('\0').to_string())
    } else {
        if val + count > data.len() { return None; }
        Some(String::from_utf8_lossy(&data[val..val + count]).trim_end_matches('\0').to_string())
    }
}

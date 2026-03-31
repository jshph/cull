use std::path::{Path, PathBuf};
use crate::catalog::Mark;

pub fn sidecar_path(image: &Path) -> PathBuf {
    image.with_extension("xmp")
}

// ── public write API ───────────────────────────────────────────────────────

/// Write mark, preserving existing rotation.
pub fn write_mark(image: &Path, mark: &Mark) {
    let (_, rotation) = read_sidecar(image).unwrap_or_default();
    write_sidecar(image, mark, rotation);
}

/// Write rotation, preserving existing mark.
pub fn write_rotation(image: &Path, rotation: u8) {
    let (mark, _) = read_sidecar(image).unwrap_or_default();
    write_sidecar(image, &mark, rotation);
}

// ── canonical sidecar write ────────────────────────────────────────────────

/// Write mark + rotation to XMP sidecar.
///
/// Lightroom Classic / Capture One compatibility:
///
///   Pick   → xmp:Rating 0, xmp:Label "Green"
///            LR has no XMP pick flag (catalog-only). Green label is the
///            standard visual proxy. Rating stays 0 so stars are free for
///            the user's own grading (1-5 within picks).
///
///   Reject → xmp:Rating -1, xmp:Label "Red"
///            LR reads Rating=-1 as its native Reject flag (the X mark).
///            Red label provides visual filtering in both LR and C1.
///
///   None   → xmp:Rating 0, no label
///            No mark — untouched image.
///
///   tiff:Orientation  (1=normal, 3=180°, 6=90°CW, 8=90°CCW)
pub fn write_sidecar(image: &Path, mark: &Mark, rotation: u8) {
    let (rating, label) = match mark {
        Mark::Pick   => (0i8,  Some("Green")),
        Mark::Reject => (-1i8, Some("Red")),
        Mark::None   => (0i8,  None),
    };

    // tiff:Orientation values Lightroom understands
    let orientation: u8 = match rotation {
        1 => 8, // 90° CCW
        2 => 3, // 180°
        3 => 6, // 90° CW
        _ => 1, // no rotation
    };

    let label_line = label
        .map(|l| format!("      <xmp:Label>{l}</xmp:Label>\n"))
        .unwrap_or_default();

    let orient_line = if orientation != 1 {
        format!("      <tiff:Orientation>{orientation}</tiff:Orientation>\n")
    } else {
        String::new()
    };

    let xmp = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"cull\">\n\
           <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\"\n\
                 xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
                 xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\">\n\
               <xmp:Rating>{rating}</xmp:Rating>\n\
         {label_line}{orient_line}    </rdf:Description>\n\
           </rdf:RDF>\n\
         </x:xmpmeta>\n"
    );

    let _ = std::fs::write(sidecar_path(image), xmp);
}

// ── read ───────────────────────────────────────────────────────────────────

/// Read (mark, rotation) from XMP sidecar. Returns None if no sidecar exists.
///
/// Recognizes both cull's format and Lightroom-written sidecars:
///   Label "Green" → Pick
///   Label "Red"   → Reject
///   Rating -1     → Reject (LR native reject flag)
pub fn read_sidecar(image: &Path) -> Option<(Mark, u8)> {
    let content = std::fs::read_to_string(sidecar_path(image)).ok()?;

    let label = extract_tag(&content, "xmp:Label").unwrap_or_default();
    let rating = extract_tag(&content, "xmp:Rating")
        .and_then(|v| v.parse::<i8>().ok())
        .unwrap_or(0);

    let mark = if label == "Green" {
        Mark::Pick
    } else if label == "Red" || rating == -1 {
        Mark::Reject
    } else {
        Mark::None
    };

    let orientation = extract_tag(&content, "tiff:Orientation")
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(1);
    let rotation = match orientation {
        8 => 1, // 90° CCW
        3 => 2, // 180°
        6 => 3, // 90° CW
        _ => 0, // normal
    };

    Some((mark, rotation))
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open  = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end   = xml[start..].find(&close).map(|e| e + start)?;
    Some(xml[start..end].trim().to_string())
}

use std::path::{Path, PathBuf};
use crate::catalog::Mark;

pub fn sidecar_path(image: &Path) -> PathBuf {
    image.with_extension("xmp")
}

/// Write a minimal XMP sidecar. Lightroom reads xmp:Rating and xmp:Label on import.
/// Pick   → Rating 5 + Label "Green"
/// Reject → Rating 1 + Label "Red"
/// None   → Rating 0, no label
pub fn write_mark(image: &Path, mark: &Mark) {
    let (rating, label) = match mark {
        Mark::Pick => (5u8, Some("Green")),
        Mark::Reject => (1u8, Some("Red")),
        Mark::None => (0u8, None),
    };

    let label_line = label
        .map(|l| format!("      <xmp:Label>{l}</xmp:Label>\n"))
        .unwrap_or_default();

    let xmp = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"cull\">\n\
           <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
             <rdf:Description rdf:about=\"\"\n\
                 xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n\
               <xmp:Rating>{rating}</xmp:Rating>\n\
         {label_line}    </rdf:Description>\n\
           </rdf:RDF>\n\
         </x:xmpmeta>\n"
    );

    // Best-effort — never crash the UI over a sidecar write failure
    let _ = std::fs::write(sidecar_path(image), xmp);
}

/// Read mark from an existing XMP sidecar, if present.
pub fn read_mark(image: &Path) -> Option<Mark> {
    let content = std::fs::read_to_string(sidecar_path(image)).ok()?;
    let label = extract_tag(&content, "xmp:Label").unwrap_or_default();
    let mark = match label.as_str() {
        "Green" => Mark::Pick,
        "Red" => Mark::Reject,
        _ => Mark::None,
    };
    Some(mark)
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close).map(|e| e + start)?;
    Some(xml[start..end].trim().to_string())
}

use crate::catalog::Mark;
use anyhow::{bail, Context, Result};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XMP: &str = "http://ns.adobe.com/xap/1.0/";
const DM: &str = "http://ns.adobe.com/xmp/1.0/DynamicMedia/";
const CULL: &str = "https://getcull.fyi/ns/1.0/";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const TIFF: &str = "http://ns.adobe.com/tiff/1.0/";
const EMPTY: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"></rdf:RDF></x:xmpmeta>";
const MAX_PACKET: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub mark: Mark,
    pub rating: Option<u8>,
    pub label: String,
    pub rotation: u8,
    pub tags: Vec<String>,
}

pub fn sidecar_path(image: &Path) -> PathBuf {
    let lower = image.with_extension("xmp");
    let upper = image.with_extension("XMP");
    if !lower.exists() && upper.exists() {
        upper
    } else {
        lower
    }
}

fn property(doc: &roxmltree::Document<'_>, ns: &str, local: &str) -> Option<String> {
    for desc in doc
        .descendants()
        .filter(|n| n.has_tag_name((RDF, "Description")))
    {
        if let Some(value) = desc.attribute((ns, local)) {
            return Some(value.to_owned());
        }
        if let Some(node) = desc.children().find(|n| n.has_tag_name((ns, local))) {
            return Some(node.text().unwrap_or_default().trim().to_owned());
        }
    }
    None
}

fn label_mark(label: &str) -> Mark {
    match label {
        "Green" => Mark::Pick,
        "Red" => Mark::Reject,
        _ => Mark::None,
    }
}
fn flag_mark(flag: &str) -> Option<Mark> {
    match flag {
        "1" => Some(Mark::Pick),
        "-1" => Some(Mark::Reject),
        "0" => Some(Mark::None),
        _ => None,
    }
}
fn mark_flag(mark: &Mark) -> &'static str {
    match mark {
        Mark::Pick => "1",
        Mark::Reject => "-1",
        Mark::None => "0",
    }
}

fn parse_metadata(packet: &str) -> Result<Metadata> {
    let doc = roxmltree::Document::parse(packet)
        .context("Invalid XMP; existing metadata was not changed")?;
    let label = property(&doc, XMP, "Label").unwrap_or_default();
    let previous = property(&doc, CULL, "Decision").as_deref().and_then(flag_mark);
    let good = property(&doc, DM, "good");
    let flag = match good.as_deref() {
        Some("True" | "true" | "1") => Some(Mark::Pick),
        Some("False" | "false" | "0") => Some(Mark::Reject),
        _ if property(&doc, CULL, "FlagEncoding").as_deref() == Some("good-v1") => Some(Mark::None),
        _ if previous.is_some() => property(&doc, DM, "pick").as_deref().and_then(flag_mark),
        _ => None,
    };
    let applied = property(&doc, CULL, "AppliedLabel");
    // Detect which editor changed the decision since Cull last wrote it.
    // Lightroom flags win if both flags and labels changed incompatibly.
    let mark = if previous.is_some() && flag.is_some() && flag != previous {
        flag.clone().unwrap()
    } else if previous.is_some() && applied.as_deref().is_some_and(|old| old != label) {
        label_mark(&label)
    } else if let Some(flag) = flag {
        flag
    } else if matches!(label.as_str(), "Green" | "Red") {
        label_mark(&label)
    } else if property(&doc, XMP, "Rating").as_deref() == Some("-1") {
        Mark::Reject // Compatibility with old Cull / XMP rejection metadata.
    } else {
        Mark::None
    };
    let rating = property(&doc, XMP, "Rating")
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|v| *v <= 5);
    let rotation = match property(&doc, TIFF, "Orientation").as_deref() {
        Some("8") => 1,
        Some("3") => 2,
        Some("6") => 3,
        _ => 0,
    };
    let tags = doc
        .descendants()
        .filter(|n| n.has_tag_name((DC, "subject")))
        .flat_map(|n| n.descendants().filter(|v| v.has_tag_name((RDF, "li"))))
        .filter_map(|n| n.text().map(str::to_owned))
        .collect();
    Ok(Metadata {
        mark,
        rating,
        label,
        rotation,
        tags,
    })
}

pub fn has_cull_decision(image: &Path) -> Result<bool> {
    let packet = read_packet(image)?;
    let doc = roxmltree::Document::parse(&packet)?;
    Ok(property(&doc, CULL, "Decision").is_some())
}

/// Upgrade only metadata authored by older Cull versions. Foreign packets and
/// already migrated packets remain byte-for-byte unchanged.
pub fn migrate_native_flag(image: &Path) -> Result<()> {
    let packet = read_packet(image)?;
    let doc = roxmltree::Document::parse(&packet)?;
    if property(&doc, CULL, "Decision").is_some()
        && property(&doc, CULL, "FlagEncoding").as_deref() != Some("good-v1")
    {
        write_mark(image, &parse_metadata(&packet)?.mark)?;
    }
    Ok(())
}

pub fn copy_metadata(source: &Path, target: &Path) -> Result<()> {
    let packet = read_packet(source)?;
    roxmltree::Document::parse(&packet).context("Invalid source metadata")?;
    atomic_write(&sidecar_path(target), packet.as_bytes())
}

pub fn read_metadata(image: &Path) -> Result<Metadata> {
    parse_metadata(&read_packet(image)?)
}

// Retained for the catalog/preview API. Mutations always use the fallible API.
pub fn read_sidecar(image: &Path) -> Option<(Mark, u8, Vec<String>)> {
    read_metadata(image)
        .ok()
        .map(|m| (m.mark, m.rotation, m.tags))
}

fn read_packet(image: &Path) -> Result<String> {
    match std::fs::read_to_string(sidecar_path(image)) {
        Ok(packet) => Ok(packet),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(embedded_xmp(image)?.unwrap_or_else(|| EMPTY.to_owned()))
        }
        Err(error) => Err(error).context("Cannot read existing XMP"),
    }
}

/// Read embedded XMP in TIFF/DNG and JPEG without rewriting source images.
/// Only seek to the XMP packet, not the multi-megabyte RAW image data.
fn embedded_xmp(image: &Path) -> Result<Option<String>> {
    let mut file =
        std::fs::File::open(image).with_context(|| format!("Cannot read {}", image.display()))?;
    let mut header = [0u8; 8];
    if file.read_exact(&mut header).is_err() {
        return Ok(None);
    }
    let packet = if header.starts_with(&[0xFF, 0xD8]) {
        file.seek(SeekFrom::Start(2))?;
        loop {
            let mut marker = [0u8; 2];
            if file.read_exact(&mut marker).is_err() {
                return Ok(None);
            }
            if marker[0] != 0xFF || matches!(marker[1], 0xDA | 0xD9) {
                return Ok(None);
            }
            let mut len = [0u8; 2];
            file.read_exact(&mut len)?;
            let length = u16::from_be_bytes(len) as usize;
            if length < 2 {
                bail!("Invalid JPEG segment length");
            }
            let mut bytes = vec![0; length - 2];
            file.read_exact(&mut bytes)?;
            const PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
            if marker[1] == 0xE1 && bytes.starts_with(PREFIX) {
                break bytes[PREFIX.len()..].to_vec();
            }
        }
    } else if header.starts_with(b"II*\0") || header.starts_with(b"MM\0*") {
        let le = header[0] == b'I';
        let u16val = |b: [u8; 2]| {
            if le {
                u16::from_le_bytes(b)
            } else {
                u16::from_be_bytes(b)
            }
        };
        let u32val = |b: [u8; 4]| {
            if le {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            }
        };
        let offset = u32val(header[4..8].try_into().unwrap());
        file.seek(SeekFrom::Start(offset as u64))?;
        let mut count = [0u8; 2];
        file.read_exact(&mut count)?;
        let mut location = None;
        for _ in 0..u16val(count) {
            let mut entry = [0u8; 12];
            file.read_exact(&mut entry)?;
            if u16val(entry[..2].try_into().unwrap()) == 700 {
                let count = u32val(entry[4..8].try_into().unwrap()) as usize;
                if count > MAX_PACKET {
                    bail!("Embedded XMP is too large");
                }
                location = Some((count, entry[8..12].to_vec()));
                break;
            }
        }
        let Some((count, value)) = location else {
            return Ok(None);
        };
        if count <= 4 {
            value[..count].to_vec()
        } else {
            file.seek(SeekFrom::Start(u32val(value.try_into().unwrap()) as u64))?;
            let mut bytes = vec![0; count];
            file.read_exact(&mut bytes)?;
            bytes
        }
    } else {
        return Ok(None);
    };
    Ok(Some(
        String::from_utf8(packet)
            .context("Embedded XMP is not UTF-8")?
            .trim_end_matches('\0')
            .to_owned(),
    ))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

struct Change<'a> {
    ns: &'a str,
    name: &'a str,
    value: Option<String>,
}
fn scalar<'a>(ns: &'a str, name: &'a str, value: impl AsRef<str>) -> Change<'a> {
    Change {
        ns,
        name,
        value: Some(escape(value.as_ref())),
    }
}

/// Surgically replace only selected properties. Unknown XML, editor settings,
/// namespace aliases, comments, and unrelated attributes remain byte-for-byte.
fn update_packet(packet: &str, changes: &[Change<'_>]) -> Result<String> {
    let doc = roxmltree::Document::parse(packet).context("Invalid XMP; refusing to replace it")?;
    let rdf = doc
        .descendants()
        .find(|n| n.has_tag_name((RDF, "RDF")))
        .context("XMP has no RDF element")?;
    let mut edits = Vec::new();
    for desc in rdf
        .children()
        .filter(|n| n.has_tag_name((RDF, "Description")))
    {
        for attr in desc.attributes() {
            if changes
                .iter()
                .any(|c| attr.namespace() == Some(c.ns) && attr.name() == c.name)
            {
                edits.push((attr.range(), String::new()));
            }
        }
        for node in desc.children().filter(|n| n.is_element()) {
            if changes.iter().any(|c| {
                node.tag_name().namespace() == Some(c.ns) && node.tag_name().name() == c.name
            }) {
                edits.push((node.range(), String::new()));
            }
        }
    }
    let mut addition = String::new();
    for change in changes {
        if let Some(value) = &change.value {
            addition.push_str(&format!(
                "\n<p:{} xmlns:p=\"{}\">{}</p:{}>",
                change.name, change.ns, value, change.name
            ));
        }
    }
    let target = rdf
        .children()
        .find(|n| n.has_tag_name((RDF, "Description")));
    if target.is_none() {
        addition = format!("<cullrdf:Description xmlns:cullrdf=\"{RDF}\" cullrdf:about=\"\">{addition}</cullrdf:Description>");
    }
    let range = target.unwrap_or(rdf).range();
    let text = &packet[range.clone()];
    if text.ends_with("/>") {
        let name = text[1..]
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap();
        edits.push((range.end - 2..range.end, format!(">{addition}</{name}>")));
    } else {
        let end = range.start + text.rfind("</").context("Invalid RDF closing element")?;
        edits.push((end..end, addition));
    }
    edits.sort_by(|a, b| b.0.start.cmp(&a.0.start));
    let mut result = packet.to_owned();
    for (range, replacement) in edits {
        result.replace_range(range, &replacement);
    }
    roxmltree::Document::parse(&result).context("Refusing to write malformed XMP")?;
    Ok(result)
}

pub(crate) fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    if let Ok(meta) = std::fs::metadata(path) {
        file.as_file().set_permissions(meta.permissions())?;
    }
    file.write_all(data)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|e| e.error)
        .context("Could not save metadata atomically")?;
    Ok(())
}

fn save_changes(image: &Path, packet: &str, changes: &[Change<'_>]) -> Result<()> {
    let new = update_packet(packet, changes)?;
    atomic_write(&sidecar_path(image), new.as_bytes())
}

pub fn write_mark(image: &Path, mark: &Mark) -> Result<()> {
    let packet = read_packet(image)?;
    let doc = roxmltree::Document::parse(&packet).context("Invalid existing XMP")?;
    let label = property(&doc, XMP, "Label").unwrap_or_default();
    let applied = property(&doc, CULL, "AppliedLabel");
    let saved = property(&doc, CULL, "PreviousLabel");
    // Green and red are reserved decisions. Preserve another workflow's labels
    // so unmark can restore them; do not erase a later external label change.
    let prior = if applied.as_deref() == Some(label.as_str()) {
        saved.unwrap_or_default()
    } else if !matches!(label.as_str(), "Green" | "Red") {
        label.clone()
    } else {
        String::new()
    };
    let next_label = match mark {
        Mark::Pick => "Green",
        Mark::Reject => "Red",
        Mark::None => &prior,
    };
    let mut changes = vec![
        Change { ns: DM, name: "good", value: match mark {
            Mark::Pick => Some("True".into()), Mark::Reject => Some("False".into()), Mark::None => None,
        } },
        scalar(CULL, "FlagEncoding", "good-v1"),
        scalar(CULL, "Decision", mark_flag(mark)),
        scalar(CULL, "AppliedLabel", next_label),
        scalar(CULL, "PreviousLabel", &prior),
        Change {
            ns: XMP,
            name: "Label",
            value: if next_label.is_empty() {
                None
            } else {
                Some(escape(next_label))
            },
        },
    ];
    // Older Cull incorrectly used xmpDM:pick as the flag. Remove that property
    // only from packets carrying our decision marker; preserve foreign metadata.
    if property(&doc, CULL, "Decision").is_some() {
        changes.push(Change { ns: DM, name: "pick", value: None });
    }
    // Legacy Cull stored a reject in Rating. Migrate that sentinel to the flag;
    // never reset a genuine 0–5 star rating or invent a lost prior rating.
    if property(&doc, XMP, "Rating").as_deref() == Some("-1") {
        changes.push(scalar(XMP, "Rating", "0"));
    }
    save_changes(image, &packet, &changes)
}

pub fn write_rotation(image: &Path, rotation: u8) -> Result<()> {
    let packet = read_packet(image)?;
    let value = match rotation % 4 {
        1 => "8",
        2 => "3",
        3 => "6",
        _ => "1",
    };
    save_changes(image, &packet, &[scalar(TIFF, "Orientation", value)])
}
pub fn write_tags(image: &Path, tags: &[String]) -> Result<()> {
    let packet = read_packet(image)?;
    let items: String = tags
        .iter()
        .map(|s| format!("<r:li>{}</r:li>", escape(s)))
        .collect();
    let value = format!("<r:Bag xmlns:r=\"{RDF}\">{items}</r:Bag>");
    save_changes(
        image,
        &packet,
        &[Change {
            ns: DC,
            name: "subject",
            value: Some(value),
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(packet: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("test.DNG");
        std::fs::write(&photo, b"camera bytes").unwrap();
        std::fs::write(sidecar_path(&photo), packet).unwrap();
        (dir, photo)
    }
    fn packet(properties: &str) -> String {
        format!("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><r:RDF xmlns:r=\"{RDF}\"><r:Description xmlns:z=\"{XMP}\" xmlns:dm=\"{DM}\" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" {properties}/></r:RDF></x:xmpmeta>")
    }
    #[test]
    fn flags_preserve_stars_custom_labels_and_develop_attributes() {
        let (_dir, photo) = fixture(&packet(
            "z:Rating='4' z:Label='Blue' crs:Exposure2012='+1.25'",
        ));
        for mark in [Mark::Pick, Mark::Reject, Mark::None] {
            write_mark(&photo, &mark).unwrap();
            let metadata = read_metadata(&photo).unwrap();
            assert_eq!(metadata.mark, mark);
            assert_eq!(metadata.rating, Some(4));
            assert!(std::fs::read_to_string(sidecar_path(&photo))
                .unwrap()
                .contains("crs:Exposure2012='+1.25'"));
        }
        assert_eq!(read_metadata(&photo).unwrap().label, "Blue");
        assert_eq!(std::fs::read(&photo).unwrap(), b"camera bytes");
    }
    #[test]
    fn legacy_flag_migration_is_scoped_and_idempotent() {
        let (_dir, photo) = fixture(&packet("z:Rating='4'"));
        let foreign = read_packet(&photo).unwrap();
        migrate_native_flag(&photo).unwrap();
        assert_eq!(read_packet(&photo).unwrap(), foreign);
        write_mark(&photo, &Mark::Pick).unwrap();
        let current = read_packet(&photo).unwrap();
        let legacy = update_packet(&current, &[
            Change { ns: CULL, name: "FlagEncoding", value: None },
            Change { ns: DM, name: "good", value: None },
            scalar(DM, "pick", "1"),
        ]).unwrap();
        std::fs::write(sidecar_path(&photo), legacy).unwrap();
        migrate_native_flag(&photo).unwrap();
        let migrated = read_packet(&photo).unwrap();
        let doc = roxmltree::Document::parse(&migrated).unwrap();
        assert_eq!(property(&doc, DM, "good").as_deref(), Some("True"));
        assert!(property(&doc, DM, "pick").is_none());
        assert_eq!(read_metadata(&photo).unwrap().rating, Some(4));
        migrate_native_flag(&photo).unwrap();
        assert_eq!(read_packet(&photo).unwrap(), migrated);
    }
    #[test]
    fn attributes_aliases_keywords_and_rotation_roundtrip() {
        let (_dir, photo) = fixture(&packet("z:Rating='5'"));
        let tags = vec!["Family & Friends".into(), "<東京>".into()];
        write_tags(&photo, &tags).unwrap();
        write_rotation(&photo, 1).unwrap();
        write_mark(&photo, &Mark::Reject).unwrap();
        let m = read_metadata(&photo).unwrap();
        assert_eq!(m.rating, Some(5));
        assert_eq!(m.tags, tags);
        assert_eq!(m.rotation, 1);
        write_rotation(&photo, 0).unwrap();
        assert_eq!(read_metadata(&photo).unwrap().rotation, 0);
    }
    #[test]
    fn malformed_sidecar_fails_without_touching_either_file() {
        let (_dir, photo) = fixture("<broken");
        assert!(write_mark(&photo, &Mark::Pick).is_err());
        assert_eq!(
            std::fs::read_to_string(sidecar_path(&photo)).unwrap(),
            "<broken"
        );
        assert_eq!(std::fs::read(&photo).unwrap(), b"camera bytes");
    }
    #[test]
    fn external_flag_and_color_changes_are_reconciled() {
        let (_dir, photo) = fixture(EMPTY);
        write_mark(&photo, &Mark::Pick).unwrap();
        let packet = read_packet(&photo).unwrap();
        save_changes(&photo, &packet, &[scalar(XMP, "Label", "Red")]).unwrap();
        assert_eq!(read_metadata(&photo).unwrap().mark, Mark::Reject);
        write_mark(&photo, &Mark::Pick).unwrap();
        let packet = read_packet(&photo).unwrap();
        save_changes(&photo, &packet, &[Change { ns: DM, name: "good", value: None }]).unwrap();
        assert_eq!(read_metadata(&photo).unwrap().mark, Mark::None);
    }
    #[test]
    fn unmark_does_not_erase_a_later_foreign_label() {
        let (_dir, photo) = fixture(EMPTY);
        write_mark(&photo, &Mark::Pick).unwrap();
        let packet = read_packet(&photo).unwrap();
        save_changes(&photo, &packet, &[scalar(XMP, "Label", "Purple")]).unwrap();
        write_mark(&photo, &Mark::None).unwrap();
        assert_eq!(read_metadata(&photo).unwrap().label, "Purple");
    }
    #[test]
    fn uppercase_sidecar_and_legacy_reject_migrate() {
        let (_dir, photo) = fixture(&packet("z:Rating='-1' z:Label='Red'"));
        std::fs::rename(photo.with_extension("xmp"), photo.with_extension("XMP")).unwrap();
        write_mark(&photo, &Mark::Reject).unwrap();
        assert_eq!(
            std::fs::read_dir(photo.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e
                    .path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("xmp")))
                .count(),
            1
        );
        let m = read_metadata(&photo).unwrap();
        assert_eq!(m.mark, Mark::Reject);
        assert_eq!(m.rating, Some(0));
    }
    #[test]
    fn embedded_dng_xmp_is_preserved_in_new_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("embedded.DNG");
        let xml = packet("z:Rating='3' crs:Exposure2012='0.50'");
        let mut bytes = b"II*\0\x08\0\0\0".to_vec();
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(700u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend((xml.len() as u32).to_le_bytes());
        bytes.extend(26u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(xml.as_bytes());
        std::fs::write(&photo, &bytes).unwrap();
        write_mark(&photo, &Mark::Pick).unwrap();
        assert_eq!(read_metadata(&photo).unwrap().rating, Some(3));
        assert_eq!(std::fs::read(&photo).unwrap(), bytes);
        assert!(read_packet(&photo)
            .unwrap()
            .contains("crs:Exposure2012='0.50'"));
    }
    #[test]
    fn adobe_native_good_flags_override_legacy_pick_and_roundtrip_unmark() {
        let (_dir, photo) = fixture(&packet("z:Rating='4' dm:pick='1' dm:good='False'"));
        assert_eq!(read_metadata(&photo).unwrap().mark, Mark::Reject);
        write_mark(&photo, &Mark::Pick).unwrap();
        assert!(read_packet(&photo).unwrap().contains(">True</p:good>"));
        write_mark(&photo, &Mark::Reject).unwrap();
        assert_eq!(read_metadata(&photo).unwrap().mark, Mark::Reject);
        let xml = read_packet(&photo).unwrap();
        assert!(xml.contains(">False</p:good>"));
        assert!(property(&roxmltree::Document::parse(&xml).unwrap(), DM, "pick").is_none());
        // Lightroom removes good when unflagging, without changing its color.
        save_changes(&photo, &xml, &[Change { ns: DM, name: "good", value: None }]).unwrap();
        assert_eq!(read_metadata(&photo).unwrap().mark, Mark::None);
        assert_eq!(read_metadata(&photo).unwrap().rating, Some(4));
    }

    #[test]
    fn embedded_jpeg_xmp_keeps_grades_and_image_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("embedded.JPG");
        let xml = packet("z:Rating='5' crs:Exposure2012='+0.25'");
        let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        payload.extend(xml.as_bytes());
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend(((payload.len() + 2) as u16).to_be_bytes());
        bytes.extend(payload);
        bytes.extend([0xff, 0xd9]);
        std::fs::write(&photo, &bytes).unwrap();
        write_mark(&photo, &Mark::Reject).unwrap();
        assert_eq!(read_metadata(&photo).unwrap().rating, Some(5));
        assert!(read_packet(&photo)
            .unwrap()
            .contains("crs:Exposure2012='+0.25'"));
        assert_eq!(std::fs::read(&photo).unwrap(), bytes);
    }

    #[test]
    fn repeated_changes_do_not_accumulate_description_elements() {
        let (_dir, photo) = fixture(EMPTY);
        for _ in 0..10 {
            write_mark(&photo, &Mark::Pick).unwrap();
            write_mark(&photo, &Mark::None).unwrap();
        }
        let xml = read_packet(&photo).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        assert_eq!(
            doc.descendants()
                .filter(|n| n.has_tag_name((RDF, "Description")))
                .count(),
            1
        );
    }
}

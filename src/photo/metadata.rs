//! Metadata privacy (`docs/photography-v1.md`, "Metadata privacy"): the
//! categories of metadata a photo carries, export policies, a deterministic EXIF
//! and XMP writer for PNG and JPEG, and the privacy report.
use super::tiff::{Entry, Role, Tiff};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::Path;

/// Every metadata category, in report order.
pub const CATEGORIES: [&str; 11] = [
    "copyright",
    "creator",
    "description",
    "keywords",
    "location",
    "camera",
    "timestamps",
    "gps",
    "serials",
    "identity",
    "software",
];
/// Categories redacted by `photo metadata` and written only on explicit request.
pub const PRIVATE: [&str; 3] = ["gps", "serials", "identity"];
pub const POLICIES: [&str; 4] = ["none", "copyright", "public", "all-including-private"];

const MAX_VALUES: usize = 256;
const MAX_TEXT: usize = 1024;
/// Largest XMP packet or EXIF block one JPEG APP1 segment holds.
const MAX_APP1: usize = 65_533;
const MAX_JPEG_SEGMENTS: usize = 4096;

fn category(name: &str) -> Result<&'static str> {
    CATEGORIES
        .iter()
        .find(|c| **c == name)
        .copied()
        .with_context(|| {
            format!(
                "[invalid-input] unknown metadata category {name:?}; expected one of {}",
                CATEGORIES.join(", ")
            )
        })
}

/// Which categories an export writes.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub name: String,
    pub keep: BTreeSet<&'static str>,
}

impl Policy {
    /// A named policy adjusted by `include` and `exclude` categories.
    pub fn new(name: &str, include: &[String], exclude: &[String]) -> Result<Policy> {
        let base: &[&str] = match name {
            "none" => &[],
            "copyright" => &["copyright", "creator"],
            "public" => &[
                "copyright",
                "creator",
                "description",
                "keywords",
                "location",
                "camera",
                "software",
            ],
            "all-including-private" => &CATEGORIES,
            _ => bail!(
                "[invalid-input] unknown metadata policy {name:?}; expected one of {}",
                POLICIES.join(", ")
            ),
        };
        let mut keep: BTreeSet<&'static str> =
            base.iter().map(|c| category(c)).collect::<Result<_>>()?;
        let include: BTreeSet<&'static str> =
            include.iter().map(|c| category(c)).collect::<Result<_>>()?;
        let exclude: BTreeSet<&'static str> =
            exclude.iter().map(|c| category(c)).collect::<Result<_>>()?;
        if let Some(both) = include.intersection(&exclude).next() {
            bail!("[invalid-input] metadata category {both} is both included and excluded")
        }
        keep.extend(include);
        keep.retain(|c| !exclude.contains(c));
        Ok(Policy {
            name: name.to_string(),
            keep,
        })
    }

    /// `{"policy": P, "include": [...], "exclude": [...]}` as a recipe stores it.
    pub fn from_json(value: &Value) -> Result<Policy> {
        let object = value
            .as_object()
            .context("[invalid-input] metadata must be an object")?;
        if let Some(key) = object
            .keys()
            .find(|k| !["policy", "include", "exclude"].contains(&k.as_str()))
        {
            bail!("[invalid-input] metadata has unknown key {key}")
        }
        let list = |key: &str| -> Result<Vec<String>> {
            match object.get(key) {
                None => Ok(Vec::new()),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|i| {
                        i.as_str().map(str::to_string).with_context(|| {
                            format!("[invalid-input] metadata {key} must be category names")
                        })
                    })
                    .collect(),
                Some(_) => bail!("[invalid-input] metadata {key} must be an array"),
            }
        };
        let name = match object.get("policy") {
            None => "none",
            Some(p) => p
                .as_str()
                .context("[invalid-input] metadata policy must be a string")?,
        };
        Policy::new(name, &list("include")?, &list("exclude")?)
    }

    pub fn keeps(&self, category: &str) -> bool {
        self.keep.contains(category)
    }

    pub fn report(&self) -> Value {
        json!({"policy": self.name, "categories": self.keep.iter().collect::<Vec<_>>()})
    }
}

/// The IFD an EXIF field belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dir {
    Ifd0,
    Exif,
    Gps,
}

/// A TIFF field value, written little-endian.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Byte(Vec<u8>),
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<[u32; 2]>),
    Undefined(Vec<u8>),
    SRational(Vec<[i32; 2]>),
}

impl Field {
    fn encode(&self) -> (u16, u32, Vec<u8>) {
        match self {
            Field::Byte(v) => (1, v.len() as u32, v.clone()),
            Field::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                (2, b.len() as u32, b)
            }
            Field::Short(v) => (
                3,
                v.len() as u32,
                v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            Field::Long(v) => (
                4,
                v.len() as u32,
                v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            Field::Rational(v) => (
                5,
                v.len() as u32,
                v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            Field::Undefined(v) => (7, v.len() as u32, v.clone()),
            Field::SRational(v) => (
                10,
                v.len() as u32,
                v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect(),
            ),
        }
    }

    /// The value as `photo metadata` shows it.
    fn display(&self) -> Value {
        let list = |values: Vec<Value>| {
            if values.len() == 1 {
                values.into_iter().next().unwrap()
            } else {
                Value::Array(values)
            }
        };
        let ratio = |n: f64, d: f64| {
            if d == 0.0 {
                Value::Null
            } else {
                super::dng::number(n / d)
            }
        };
        match self {
            Field::Ascii(s) => json!(s),
            Field::Byte(v) => list(v.iter().map(|x| json!(x)).collect()),
            Field::Short(v) => list(v.iter().map(|x| json!(x)).collect()),
            Field::Long(v) => list(v.iter().map(|x| json!(x)).collect()),
            Field::Rational(v) => list(
                v.iter()
                    .map(|[n, d]| ratio(f64::from(*n), f64::from(*d)))
                    .collect(),
            ),
            Field::SRational(v) => list(
                v.iter()
                    .map(|[n, d]| ratio(f64::from(*n), f64::from(*d)))
                    .collect(),
            ),
            Field::Undefined(v) => json!({"bytes": v.len()}),
        }
    }
}

/// One metadata value of a photo.
#[derive(Clone, Debug)]
pub struct Item {
    pub category: &'static str,
    pub key: String,
    pub value: Value,
    /// Where an export writes it in EXIF; `None` for values that are only shown
    /// (XMP-only fields, oversized source fields, XMP found in the source).
    pub exif: Option<(Dir, u16, Field)>,
}

/// Source EXIF fields that pentool understands, by IFD and tag.
const SOURCE_TAGS: [(Dir, u16, &str, &str); 27] = [
    (Dir::Ifd0, 271, "camera", "make"),
    (Dir::Ifd0, 272, "camera", "model"),
    (Dir::Exif, 42035, "camera", "lens_make"),
    (Dir::Exif, 42036, "camera", "lens_model"),
    (Dir::Exif, 37386, "camera", "focal_length"),
    (Dir::Exif, 33437, "camera", "aperture"),
    (Dir::Exif, 33434, "camera", "exposure_time"),
    (Dir::Exif, 34855, "camera", "iso"),
    (Dir::Exif, 37385, "camera", "flash"),
    (Dir::Ifd0, 306, "timestamps", "modify_date"),
    (Dir::Exif, 36867, "timestamps", "date_time_original"),
    (Dir::Exif, 36868, "timestamps", "create_date"),
    (Dir::Exif, 36880, "timestamps", "offset_time"),
    (Dir::Exif, 36881, "timestamps", "offset_time_original"),
    (Dir::Exif, 36882, "timestamps", "offset_time_digitized"),
    (Dir::Exif, 37520, "timestamps", "sub_sec_time"),
    (Dir::Exif, 37521, "timestamps", "sub_sec_time_original"),
    (Dir::Exif, 37522, "timestamps", "sub_sec_time_digitized"),
    (Dir::Ifd0, 50735, "serials", "camera_serial"),
    (Dir::Exif, 42033, "serials", "body_serial"),
    (Dir::Exif, 42037, "serials", "lens_serial"),
    (Dir::Exif, 42016, "serials", "image_unique_id"),
    (Dir::Exif, 42032, "identity", "camera_owner"),
    (Dir::Ifd0, 315, "identity", "artist"),
    // Recorded so the report shows them; never written (values come from the
    // photo entry instead).
    (Dir::Ifd0, 33432, "copyright", "source_copyright"),
    (Dir::Ifd0, 270, "description", "source_description"),
    (Dir::Ifd0, 305, "software", "source_software"),
];

/// Tags from the source that are reported but not written by an export.
const SHOWN_ONLY: [u16; 3] = [33432, 270, 305];

const GPS_NAMES: [&str; 32] = [
    "version",
    "latitude_ref",
    "latitude",
    "longitude_ref",
    "longitude",
    "altitude_ref",
    "altitude",
    "time_stamp",
    "satellites",
    "status",
    "measure_mode",
    "dop",
    "speed_ref",
    "speed",
    "track_ref",
    "track",
    "img_direction_ref",
    "img_direction",
    "map_datum",
    "dest_latitude_ref",
    "dest_latitude",
    "dest_longitude_ref",
    "dest_longitude",
    "dest_bearing_ref",
    "dest_bearing",
    "dest_distance_ref",
    "dest_distance",
    "processing_method",
    "area_information",
    "date_stamp",
    "differential",
    "h_positioning_error",
];

/// XMP properties that put a source in a private category. XMP is never copied
/// to an export; pentool writes its own packet.
const XMP_MARKERS: [(&str, &str); 10] = [
    ("GPSLatitude", "gps"),
    ("GPSLongitude", "gps"),
    ("GPSAltitude", "gps"),
    ("SerialNumber", "serials"),
    ("ImageUniqueID", "serials"),
    ("CameraOwnerName", "identity"),
    ("OwnerName", "identity"),
    ("PersonInImage", "identity"),
    ("mwg-rs:Regions", "identity"),
    ("MP:RegionInfo", "identity"),
];

/// Read one source field; oversized or unsupported fields are `None`.
fn read_field(tiff: &Tiff, entry: Entry, tag: u16) -> Option<Field> {
    let count = entry.count as usize;
    let at = entry.at;
    match entry.kind {
        2 => tiff
            .text_of(entry, tag, MAX_TEXT)
            .ok()
            .map(|t| Field::Ascii(t.trim().to_string())),
        _ if count > MAX_VALUES => None,
        1 => tiff
            .bytes_of(entry, tag, MAX_VALUES)
            .ok()
            .map(|b| Field::Byte(b.to_vec())),
        7 => tiff
            .bytes_of(entry, tag, MAX_VALUES)
            .ok()
            .map(|b| Field::Undefined(b.to_vec())),
        3 => (0..count)
            .map(|i| tiff.u16_at(at + 2 * i).ok())
            .collect::<Option<Vec<_>>>()
            .map(Field::Short),
        4 => (0..count)
            .map(|i| tiff.u32_at(at + 4 * i).ok())
            .collect::<Option<Vec<_>>>()
            .map(Field::Long),
        5 => (0..count)
            .map(|i| {
                Some([
                    tiff.u32_at(at + 8 * i).ok()?,
                    tiff.u32_at(at + 8 * i + 4).ok()?,
                ])
            })
            .collect::<Option<Vec<_>>>()
            .map(Field::Rational),
        10 => (0..count)
            .map(|i| {
                Some([
                    tiff.u32_at(at + 8 * i).ok()? as i32,
                    tiff.u32_at(at + 8 * i + 4).ok()? as i32,
                ])
            })
            .collect::<Option<Vec<_>>>()
            .map(Field::SRational),
        _ => None,
    }
}

fn source_item(
    tiff: &Tiff,
    entry: Entry,
    dir: Dir,
    tag: u16,
    category: &'static str,
    key: String,
) -> Option<Item> {
    match read_field(tiff, entry, tag) {
        Some(Field::Ascii(text)) if text.is_empty() => None,
        Some(field) => Some(Item {
            category,
            key,
            value: field.display(),
            exif: (!SHOWN_ONLY.contains(&tag)).then_some((dir, tag, field)),
        }),
        None => Some(Item {
            category,
            key,
            value: json!({"omitted": "too large or of an unsupported type"}),
            exif: None,
        }),
    }
}

/// The metadata items in an EXIF (TIFF) structure.
fn tiff_items(tiff: &Tiff, items: &mut Vec<Item>) {
    for (dir, tag, category, key) in SOURCE_TAGS {
        let ifd = match dir {
            Dir::Ifd0 => Some(tiff.ifd0()),
            Dir::Exif => tiff.find(Role::Exif),
            Dir::Gps => None,
        };
        if let Some(entry) = ifd.and_then(|ifd| ifd.entries.get(&tag)) {
            items.extend(source_item(tiff, *entry, dir, tag, category, key.into()));
        }
    }
    if let Some(gps) = tiff.find(Role::Gps) {
        for (tag, entry) in &gps.entries {
            let key = GPS_NAMES
                .get(usize::from(*tag))
                .map_or_else(|| format!("tag_{tag}"), |n| n.to_string());
            items.extend(source_item(tiff, *entry, Dir::Gps, *tag, "gps", key));
        }
    }
}

fn xmp_items(xmp: &[u8], items: &mut Vec<Item>) {
    let text = String::from_utf8_lossy(xmp);
    for (marker, category) in XMP_MARKERS {
        if text.contains(marker) {
            items.push(Item {
                category,
                key: format!("xmp:{marker}"),
                value: json!(true),
                exif: None,
            });
        }
    }
}

const XMP_SIGNATURE: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// The metadata in a source file: DNG/TIFF, JPEG or PNG. Other formats carry none
/// that pentool reads.
pub fn read_source(bytes: &[u8]) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    if super::tiff::is_tiff(bytes) {
        let tiff = Tiff::parse(bytes)?;
        tiff_items(&tiff, &mut items);
        if let Some(xmp) = tiff.ifd0().raw(&tiff, 700, 16 * 1024 * 1024)? {
            xmp_items(xmp, &mut items);
        }
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut at = 2;
        for _ in 0..MAX_JPEG_SEGMENTS {
            let Some(&[0xFF, marker]) = bytes.get(at..at + 2) else {
                break;
            };
            if marker == 0xDA || marker == 0xD9 {
                break;
            }
            if (0xD0..=0xD7).contains(&marker) || marker == 0x01 || marker == 0xFF {
                at += if marker == 0xFF { 1 } else { 2 };
                continue;
            }
            let length = bytes
                .get(at + 2..at + 4)
                .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
                .filter(|l| *l >= 2)
                .context("[malformed-resource] JPEG segment has no length")?;
            let body = bytes
                .get(at + 4..at + 2 + length)
                .context("[malformed-resource] JPEG segment runs past the end of the file")?;
            if marker == 0xE1 {
                if let Some(exif) = body.strip_prefix(b"Exif\0\0") {
                    let tiff = Tiff::parse(exif).context("JPEG EXIF block")?;
                    tiff_items(&tiff, &mut items);
                } else if let Some(xmp) = body.strip_prefix(XMP_SIGNATURE) {
                    xmp_items(xmp, &mut items);
                }
            }
            at += 2 + length;
        }
    } else if bytes.starts_with(b"\x89PNG") {
        for (kind, data) in super::png::chunks(bytes)? {
            match &kind {
                b"eXIf" => {
                    let tiff = Tiff::parse(&data).context("PNG eXIf chunk")?;
                    tiff_items(&tiff, &mut items);
                }
                b"iTXt" if data.starts_with(b"XML:com.adobe.xmp\0") => {
                    if data.get(18) == Some(&0) {
                        xmp_items(&data[18..], &mut items);
                    } else {
                        items.push(Item {
                            category: "identity",
                            key: "xmp:compressed".into(),
                            value: json!("compressed XMP is not scanned; treat it as private"),
                            exif: None,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    Ok(items)
}

fn ascii(text: &str) -> Option<Field> {
    text.is_ascii().then(|| Field::Ascii(text.to_string()))
}

/// The items a photo entry contributes: copyright, creator, description,
/// keywords and location, plus the `software` value an export writes.
fn entry_items(photo: &Value) -> Vec<Item> {
    let mut items = Vec::new();
    let text = |key: &str| photo[key].as_str().filter(|t| !t.is_empty());
    let mut push = |category, key: &str, value: Value, exif| {
        items.push(Item {
            category,
            key: key.into(),
            value,
            exif,
        })
    };
    if let Some(v) = text("copyright") {
        push(
            "copyright",
            "copyright",
            json!(v),
            ascii(v).map(|f| (Dir::Ifd0, 33432, f)),
        );
    }
    if let Some(v) = text("creator") {
        push(
            "creator",
            "creator",
            json!(v),
            ascii(v).map(|f| (Dir::Ifd0, 315, f)),
        );
    }
    if let Some(v) = text("title") {
        let exif = if text("caption").is_none() {
            ascii(v).map(|f| (Dir::Ifd0, 270, f))
        } else {
            None
        };
        push("description", "title", json!(v), exif);
    }
    if let Some(v) = text("caption") {
        push(
            "description",
            "caption",
            json!(v),
            ascii(v).map(|f| (Dir::Ifd0, 270, f)),
        );
    }
    if let Some(k) = photo["keywords"].as_array().filter(|k| !k.is_empty()) {
        push("keywords", "keywords", json!(k), None);
    }
    if let Some(l) = photo["location"].as_object().filter(|l| !l.is_empty()) {
        push("location", "location", json!(l), None);
    }
    let software = format!("pentool {}", env!("CARGO_PKG_VERSION"));
    push(
        "software",
        "software",
        json!(software),
        Some((Dir::Ifd0, 305, Field::Ascii(software.clone()))),
    );
    items
}

fn find_photo<'a>(raw: &'a Value, photo: &str) -> Result<&'a Value> {
    raw["photography"]["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"] == photo)
        .with_context(|| format!("[missing-resource] photo {photo} is not in the catalog"))
}

/// Every metadata item of a photo: its entry first, then its source bytes.
pub fn collect(raw: &Value, document: &Path, photo: &str) -> Result<Vec<Item>> {
    let entry = find_photo(raw, photo)?;
    let digest = entry["source"].as_str().unwrap_or_default();
    let asset = &raw["photography"]["assets"][digest];
    let bytes = super::catalog::stored_bytes(document, &asset["storage"], digest)?;
    let mut items = entry_items(entry);
    items.extend(read_source(&bytes).with_context(|| format!("photo {photo} source metadata"))?);
    Ok(items)
}

/// `photo metadata`: items grouped by category; private categories are redacted
/// unless revealed.
pub fn show(raw: &Value, document: &Path, photo: &str, reveal: &[String]) -> Result<Value> {
    crate::scene::validate(raw)?;
    for name in reveal {
        let name = category(name)?;
        if !PRIVATE.contains(&name) {
            bail!(
                "[invalid-input] --reveal {name}: only {} are redacted",
                PRIVATE.join(", ")
            )
        }
    }
    let items = collect(raw, document, photo)?;
    let mut categories = Map::new();
    let mut redacted = Vec::new();
    for name in CATEGORIES {
        if name == "software" {
            continue;
        }
        let fields: Vec<&Item> = items.iter().filter(|i| i.category == name).collect();
        if fields.is_empty() {
            continue;
        }
        if PRIVATE.contains(&name) && !reveal.iter().any(|r| r == name) {
            redacted.push(name);
            categories.insert(
                name.into(),
                json!({"redacted": true, "fields": fields.len()}),
            );
            continue;
        }
        let mut group = Map::new();
        for item in fields {
            group.insert(item.key.clone(), item.value.clone());
        }
        categories.insert(name.into(), Value::Object(group));
    }
    Ok(json!({
        "photo": photo,
        "source": find_photo(raw, photo)?["source"],
        "categories": categories,
        "redacted": redacted,
    }))
}

/// `photo privacy-report`: sources whose bytes hold private categories.
pub fn privacy_report(raw: &Value, document: &Path) -> Result<Value> {
    crate::scene::validate(raw)?;
    let catalog = raw.get("photography").context(
        "[missing-resource] the document has no photography catalog; add a photo with `raw add` first",
    )?;
    let photos = catalog["photos"].as_array().map_or(&[][..], Vec::as_slice);
    let mut flagged = Vec::new();
    let mut unreadable = Vec::new();
    let assets = catalog["assets"].as_object().cloned().unwrap_or_default();
    for (digest, asset) in &assets {
        super::check_cancelled()?;
        let users: Vec<&Value> = photos
            .iter()
            .filter(|p| p["source"] == *digest)
            .map(|p| &p["id"])
            .collect();
        let storage = if asset["storage"].get("path").is_some() {
            "external"
        } else {
            "embedded"
        };
        let items = super::catalog::stored_bytes(document, &asset["storage"], digest)
            .and_then(|bytes| read_source(&bytes));
        let items = match items {
            Ok(items) => items,
            Err(error) => {
                unreadable.push(
                    json!({"source": digest, "photos": users, "error": format!("{error:#}")}),
                );
                continue;
            }
        };
        let mut counts = Map::new();
        for name in PRIVATE {
            let n = items.iter().filter(|i| i.category == name).count();
            if n > 0 {
                counts.insert(name.into(), json!(n));
            }
        }
        if !counts.is_empty() {
            flagged.push(json!({
                "source": digest,
                "kind": asset["kind"],
                "storage": storage,
                "photos": users,
                "categories": counts,
            }));
        }
    }
    Ok(json!({
        "sources": assets.len(),
        "flagged": flagged.len(),
        "private": flagged,
        "unreadable": unreadable,
        "note": "private metadata stays inside source bytes; exports omit it unless a policy writes gps, serials or identity",
    }))
}

/// What an export writes: an EXIF TIFF block and an XMP packet.
#[derive(Debug, Default)]
pub struct Embedded {
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<String>,
    /// Categories that contributed at least one value.
    pub written: BTreeSet<&'static str>,
}

impl Embedded {
    pub fn report(&self, policy: &Policy) -> Value {
        json!({
            "policy": policy.name,
            "categories": self.written.iter().collect::<Vec<_>>(),
            "exif_bytes": self.exif.as_ref().map_or(0, Vec::len),
            "xmp_bytes": self.xmp.as_ref().map_or(0, String::len),
        })
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// A tag with its encoded type, count and value bytes.
type Encoded = (u16, (u16, u32, Vec<u8>));

/// A little-endian TIFF block of IFD0, the EXIF IFD and the GPS IFD.
fn exif_block(fields: &[(Dir, u16, Field)]) -> Vec<u8> {
    let group = |dir: Dir| -> Vec<Encoded> {
        let mut entries: Vec<_> = fields
            .iter()
            .filter(|(d, _, _)| *d == dir)
            .map(|(_, tag, f)| (*tag, f.encode()))
            .collect();
        entries.sort_by_key(|(tag, _)| *tag);
        entries.dedup_by_key(|(tag, _)| *tag);
        entries
    };
    let exif = group(Dir::Exif);
    let gps = group(Dir::Gps);
    let mut ifd0 = group(Dir::Ifd0);
    let size = |entries: &[Encoded]| -> usize {
        2 + 12 * entries.len()
            + 4
            + entries
                .iter()
                .map(|(_, (_, _, b))| {
                    if b.len() > 4 {
                        b.len() + b.len() % 2
                    } else {
                        0
                    }
                })
                .sum::<usize>()
    };
    // Pointer entries are inline LONGs, so sizes do not depend on their values.
    let pointer = |tag: u16| (tag, (4u16, 1u32, vec![0; 4]));
    if !exif.is_empty() {
        ifd0.push(pointer(34665));
    }
    if !gps.is_empty() {
        ifd0.push(pointer(34853));
    }
    ifd0.sort_by_key(|(tag, _)| *tag);
    let exif_at = 8 + size(&ifd0);
    let gps_at = exif_at + if exif.is_empty() { 0 } else { size(&exif) };
    for (tag, (_, _, bytes)) in &mut ifd0 {
        if *tag == 34665 {
            *bytes = (exif_at as u32).to_le_bytes().to_vec();
        } else if *tag == 34853 {
            *bytes = (gps_at as u32).to_le_bytes().to_vec();
        }
    }
    let mut out = b"II*\0".to_vec();
    out.extend(8u32.to_le_bytes());
    for entries in [&ifd0, &exif, &gps] {
        if entries.is_empty() {
            continue;
        }
        let start = out.len();
        let mut data_at = start + 2 + 12 * entries.len() + 4;
        let mut data = Vec::new();
        out.extend((entries.len() as u16).to_le_bytes());
        for (tag, (kind, count, bytes)) in entries.iter() {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            if bytes.len() <= 4 {
                let mut inline = bytes.clone();
                inline.resize(4, 0);
                out.extend(inline);
            } else {
                out.extend((data_at as u32).to_le_bytes());
                data.extend(bytes);
                data_at += bytes.len();
                if bytes.len() % 2 == 1 {
                    data.push(0);
                    data_at += 1;
                }
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(data);
    }
    out
}

fn xmp_packet(
    photo: &Value,
    policy: &Policy,
    written: &mut BTreeSet<&'static str>,
) -> Option<String> {
    let text = |key: &str| photo[key].as_str().filter(|t| !t.is_empty());
    let alt = |tag: &str, value: &str| {
        format!(
            "<{tag}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{tag}>",
            escape(value)
        )
    };
    let mut body = String::new();
    if policy.keeps("copyright") {
        if let Some(v) = text("copyright") {
            body.push_str(&alt("dc:rights", v));
            written.insert("copyright");
        }
    }
    if policy.keeps("creator") {
        if let Some(v) = text("creator") {
            body.push_str(&format!(
                "<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>",
                escape(v)
            ));
            written.insert("creator");
        }
    }
    if policy.keeps("description") {
        for (key, tag) in [("title", "dc:title"), ("caption", "dc:description")] {
            if let Some(v) = text(key) {
                body.push_str(&alt(tag, v));
                written.insert("description");
            }
        }
    }
    if policy.keeps("keywords") {
        if let Some(keywords) = photo["keywords"].as_array().filter(|k| !k.is_empty()) {
            body.push_str("<dc:subject><rdf:Bag>");
            for k in keywords.iter().filter_map(Value::as_str) {
                body.push_str(&format!("<rdf:li>{}</rdf:li>", escape(k)));
            }
            body.push_str("</rdf:Bag></dc:subject>");
            written.insert("keywords");
        }
    }
    if policy.keeps("location") {
        for (key, tag) in [
            ("sublocation", "Iptc4xmpCore:Location"),
            ("city", "photoshop:City"),
            ("state", "photoshop:State"),
            ("country", "photoshop:Country"),
        ] {
            if let Some(v) = photo["location"][key].as_str().filter(|v| !v.is_empty()) {
                body.push_str(&format!("<{tag}>{}</{tag}>", escape(v)));
                written.insert("location");
            }
        }
    }
    if body.is_empty() {
        return None;
    }
    if policy.keeps("software") {
        body.push_str(&format!(
            "<xmp:CreatorTool>pentool {}</xmp:CreatorTool>",
            env!("CARGO_PKG_VERSION")
        ));
        written.insert("software");
    }
    Some(format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:photoshop=\"http://ns.adobe.com/photoshop/1.0/\" xmlns:Iptc4xmpCore=\"http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">{body}</rdf:Description></rdf:RDF></x:xmpmeta>\n<?xpacket end=\"r\"?>"
    ))
}

/// Choose what an export of `photo` writes under `policy`.
pub fn prepare(raw: &Value, document: &Path, photo: &str, policy: &Policy) -> Result<Embedded> {
    let mut embedded = Embedded::default();
    if policy.keep.is_empty() {
        return Ok(embedded);
    }
    let entry = find_photo(raw, photo)?;
    let items = collect(raw, document, photo)?;
    let creator_chosen =
        policy.keeps("creator") && entry["creator"].as_str().is_some_and(|c| !c.is_empty());
    let mut fields = Vec::new();
    for item in &items {
        let Some((dir, tag, field)) = &item.exif else {
            continue;
        };
        if !policy.keeps(item.category) || item.category == "software" {
            continue;
        }
        // The source's EXIF Artist is identity; a chosen creator replaces it.
        if item.category == "identity" && *tag == 315 && creator_chosen {
            continue;
        }
        fields.push((*dir, *tag, field.clone()));
        embedded.written.insert(item.category);
    }
    embedded.xmp = xmp_packet(entry, policy, &mut embedded.written);
    if !fields.is_empty() && policy.keeps("software") {
        let software = format!("pentool {}", env!("CARGO_PKG_VERSION"));
        fields.push((Dir::Ifd0, 305, Field::Ascii(software)));
        embedded.written.insert("software");
    }
    if fields.iter().any(|(_, tag, _)| *tag != 305) {
        embedded.exif = Some(exif_block(&fields));
    }
    if embedded.exif.is_none() && embedded.xmp.is_none() {
        embedded.written.clear();
    }
    Ok(embedded)
}

/// Insert `eXIf` and an iTXt `XML:com.adobe.xmp` chunk before the first IDAT.
pub fn embed_png(png: &[u8], embedded: &Embedded) -> Result<Vec<u8>> {
    if embedded.exif.is_none() && embedded.xmp.is_none() {
        return Ok(png.to_vec());
    }
    let mut at = 8usize;
    loop {
        let header = png
            .get(at..at + 8)
            .context("[malformed-resource] PNG has no IDAT")?;
        if &header[4..8] == b"IDAT" {
            break;
        }
        let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        at += 12 + length;
    }
    let chunk = |kind: &[u8; 4], data: &[u8]| {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = super::png::crc32(&out[4..]);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    };
    let mut out = png[..at].to_vec();
    if let Some(exif) = &embedded.exif {
        out.extend(chunk(b"eXIf", exif));
    }
    if let Some(xmp) = &embedded.xmp {
        let mut data = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        data.extend_from_slice(xmp.as_bytes());
        out.extend(chunk(b"iTXt", &data));
    }
    out.extend_from_slice(&png[at..]);
    Ok(out)
}

/// Insert EXIF and XMP APP1 segments after SOI and any APP0 (JFIF) segment.
pub fn embed_jpeg(jpeg: &[u8], embedded: &Embedded) -> Result<Vec<u8>> {
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        bail!("[malformed-resource] not a JPEG")
    }
    let mut at = 2;
    if jpeg.get(2..4) == Some(&[0xFF, 0xE0]) {
        let length = jpeg
            .get(4..6)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
            .context("[malformed-resource] JPEG APP0 has no length")?;
        at = 4 + length;
    }
    let mut out = jpeg[..at.min(jpeg.len())].to_vec();
    let mut segment = |prefix: &[u8], body: &[u8], what: &str| -> Result<()> {
        let length = prefix.len() + body.len() + 2;
        if length - 2 > MAX_APP1 {
            bail!("[limit-exceeded] {what} metadata is {} bytes; a JPEG segment holds at most {MAX_APP1}", length - 2)
        }
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&(length as u16).to_be_bytes());
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
        Ok(())
    };
    if let Some(exif) = &embedded.exif {
        segment(b"Exif\0\0", exif, "EXIF")?;
    }
    if let Some(xmp) = &embedded.xmp {
        segment(XMP_SIGNATURE, xmp.as_bytes(), "XMP")?;
    }
    out.extend_from_slice(&jpeg[at.min(jpeg.len())..]);
    Ok(out)
}

/// What `photo describe` sets; `Some("")` removes a field.
#[derive(Default)]
pub struct Description {
    pub title: Option<String>,
    pub caption: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    pub sublocation: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
}

/// `photo describe`: set the descriptive fields that exports may write.
pub fn describe(
    raw: &mut Value,
    selection: &super::organize::Selection,
    description: &Description,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let top = [
        ("title", &description.title),
        ("caption", &description.caption),
        ("creator", &description.creator),
        ("copyright", &description.copyright),
    ];
    let location = [
        ("sublocation", &description.sublocation),
        ("city", &description.city),
        ("state", &description.state),
        ("country", &description.country),
    ];
    if top.iter().chain(&location).all(|(_, v)| v.is_none()) {
        bail!("[invalid-input] photo describe needs at least one of --title, --caption, --creator, --copyright, --sublocation, --city, --state or --country")
    }
    for (key, value) in top.iter().chain(&location) {
        if value
            .as_deref()
            .is_some_and(|v| v.chars().any(|c| c.is_control() && c != '\n'))
        {
            bail!("[invalid-input] --{key} must not contain control characters")
        }
    }
    let ids = selection.resolve(raw)?;
    let mut changed = raw.clone();
    let mut count = 0;
    for id in &ids {
        let photo = changed["photography"]["photos"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["id"] == id.as_str())
            .unwrap();
        let before = photo.clone();
        let object = photo.as_object_mut().unwrap();
        for (key, value) in top {
            match value.as_deref().map(str::trim) {
                Some("") => {
                    object.remove(key);
                }
                Some(v) => {
                    object.insert(key.into(), json!(v));
                }
                None => {}
            }
        }
        let mut place = object
            .get("location")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (key, value) in location {
            match value.as_deref().map(str::trim) {
                Some("") => {
                    place.remove(key);
                }
                Some(v) => {
                    place.insert(key.into(), json!(v));
                }
                None => {}
            }
        }
        if place.is_empty() {
            object.remove("location");
        } else {
            object.insert("location".into(), Value::Object(place));
        }
        count += usize::from(*photo != before);
    }
    crate::scene::validate(&changed)?;
    *raw = changed;
    Ok(json!({"photos": ids, "changed": count}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_add_and_remove_categories() {
        let p = Policy::new("public", &[], &["camera".into()]).unwrap();
        assert!(p.keeps("keywords") && !p.keeps("camera") && !p.keeps("gps"));
        assert!(!p.keeps("timestamps"));
        let p = Policy::new("copyright", &["gps".into()], &[]).unwrap();
        assert_eq!(
            p.keep.iter().copied().collect::<Vec<_>>(),
            ["copyright", "creator", "gps"]
        );
        assert!(Policy::new("none", &[], &[]).unwrap().keep.is_empty());
        assert!(Policy::new("all-including-private", &[], &[])
            .unwrap()
            .keeps("serials"));
        assert!(Policy::new("everything", &[], &[]).is_err());
        assert!(Policy::new("none", &["faces".into()], &[]).is_err());
        assert!(Policy::new("none", &["gps".into()], &["gps".into()]).is_err());
        let p = Policy::from_json(&json!({"policy": "public", "exclude": ["location"]})).unwrap();
        assert!(!p.keeps("location"));
        assert!(Policy::from_json(&json!({"policy": "public", "extra": 1})).is_err());
    }

    #[test]
    fn exif_blocks_read_back_with_pointers_to_exif_and_gps() {
        let fields = vec![
            (Dir::Ifd0, 271, Field::Ascii("Maker".into())),
            (Dir::Exif, 42033, Field::Ascii("SN-123".into())),
            (Dir::Exif, 34855, Field::Short(vec![400])),
            (Dir::Gps, 2, Field::Rational(vec![[52, 1], [30, 1], [0, 1]])),
            (Dir::Gps, 1, Field::Ascii("N".into())),
        ];
        let block = exif_block(&fields);
        assert_eq!(exif_block(&fields), block, "deterministic");
        let items = read_source(&block).unwrap();
        let get = |key: &str| items.iter().find(|i| i.key == key).unwrap();
        assert_eq!(get("make").value, "Maker");
        assert_eq!(get("body_serial").category, "serials");
        assert_eq!(get("iso").value, 400);
        assert_eq!(get("latitude").value, json!([52, 30, 0]));
        assert_eq!(get("latitude_ref").category, "gps");
    }

    #[test]
    fn png_and_jpeg_carry_exif_and_xmp_back_to_the_reader() {
        let embedded = Embedded {
            exif: Some(exif_block(&[(
                Dir::Exif,
                42032,
                Field::Ascii("Owner".into()),
            )])),
            xmp: Some("<x:xmpmeta><exif:GPSLatitude>1</exif:GPSLatitude></x:xmpmeta>".into()),
            written: BTreeSet::new(),
        };
        let image = image::RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9]));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image.write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
        for file in [
            embed_png(&png.into_inner(), &embedded).unwrap(),
            embed_jpeg(&jpeg.into_inner(), &embedded).unwrap(),
        ] {
            image::load_from_memory(&file).unwrap();
            let items = read_source(&file).unwrap();
            let categories: BTreeSet<&str> = items.iter().map(|i| i.category).collect();
            assert_eq!(categories, BTreeSet::from(["gps", "identity"]));
        }
    }

    #[test]
    fn xmp_text_is_escaped() {
        assert_eq!(escape("a<b & \"c\">"), "a&lt;b &amp; &quot;c&quot;&gt;");
    }
}

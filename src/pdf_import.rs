//! Incremental, bounded importer for classic-xref and constrained xref-stream PDF documents.
//!
//! This intentionally does not claim general PDF support. It accepts unencrypted
//! classic xref tables and plain/FlateDecode xref streams, including bounded
//! type-2 entries for non-stream values inside plain/FlateDecode object streams.
//! It also accepts direct/indirect objects, page trees, unfiltered or FlateDecode
//! page content, and a constrained subset of vector operators. Nested object
//! streams, hybrid/incremental xrefs, forms, clipping, transparency, and
//! arbitrary filters are rejected. Basic 8-bit DeviceRGB/DeviceGray FlateDecode and baseline
//! JPEG DCTDecode image XObjects are
//! embedded as ordinary image assets; unsupported image features are reported.

use crate::interchange::{
    validate_limits, FidelityReport, FidelityReportBuilder, FidelityStatus, ImportLimits,
};
use anyhow::{bail, ensure, Context, Result};
use flate2::read::ZlibDecoder;
use image::ImageDecoder;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::Path;

const MAX_OBJECTS: usize = 100_000;
const MAX_PDF_INPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_PAGES: usize = 64;
const MAX_OBJECT_DEPTH: usize = 64;
const MAX_CONTENT_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_DECODED_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONTENT_OPERATORS: usize = 100_000;
const MAX_CONTENT_TOKENS: usize = 600_000;
const MAX_PATH_BYTES: usize = 1_000_000;
const MAX_PAGE_DIMENSION: f64 = 32_768.0;

/// Result of importing one PDF into a standalone v4 or v5 scene document.
pub struct PdfImport {
    pub document: Value,
    pub report: FidelityReport,
}

#[derive(Clone)]
struct PdfTextFont {
    family: String,
    to_unicode: BTreeMap<u8, String>,
    widths: BTreeMap<u8, f64>,
}

/// Import a bounded, unencrypted classic-xref and constrained xref-stream PDF subset.
///
/// This does not modify `source` or access the filesystem/network. Unsupported
/// structural features fail before a partial document is returned. Simple
/// ASCII `Tj` text is extracted with an explicit approximate-fidelity entry;
/// unsupported image filters/color spaces and text encodings/operators are
/// reported as dropped.
pub fn import(bytes: &[u8], source: &str, limits: ImportLimits) -> Result<PdfImport> {
    validate_limits(limits)?;
    limits.check_input(bytes)?;
    ensure!(
        bytes.len() <= MAX_PDF_INPUT_BYTES,
        "[limit-exceeded] PDF input exceeds the importer hard cap of {MAX_PDF_INPUT_BYTES} bytes"
    );
    ensure!(
        bytes.starts_with(b"%PDF-"),
        "[invalid-input] input is not a PDF file"
    );
    let mut parser = PdfParser::new(bytes, limits)?;
    let mut report = FidelityReportBuilder::new("pdf", source);
    let pages = parser.pages(&mut report)?;
    ensure!(!pages.is_empty(), "[invalid-input] PDF contains no pages");

    let width = pages
        .iter()
        .map(|page| page.width)
        .fold(1.0f64, f64::max)
        .ceil() as u32;
    let height = pages
        .iter()
        .map(|page| page.height)
        .fold(1.0f64, f64::max)
        .ceil() as u32;
    let mut document = crate::scene::new_document(width, height);
    let name = Path::new(source)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Imported PDF");
    document["name"] = json!(name);
    document["pages"] = Value::Array(
        pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| {
                json!({
                    "id": format!("pdf-page-{:03}", index + 1),
                    "name": format!("Page {}", index + 1),
                    "canvas": {
                        "width": page.width.ceil() as u32,
                        "height": page.height.ceil() as u32,
                        "background": "#ffffff"
                    },
                    "layers": [{
                        "id": format!("pdf-layer-{:03}", index + 1),
                        "name": "Imported PDF vectors",
                        "visible": true,
                        "locked": false,
                        "nodes": page.nodes
                    }]
                })
            })
            .collect(),
    );
    if !parser.image_assets.is_empty() {
        document["image_assets"] = Value::Object(
            std::mem::take(&mut parser.image_assets)
                .into_iter()
                .collect(),
        );
        document = crate::scene::migrate_to_v5(document)?;
    }
    if !parser.embedded_fonts.is_empty() {
        document["fonts"] = Value::Array(
            std::mem::take(&mut parser.embedded_fonts)
                .into_values()
                .collect(),
        );
    }
    crate::scene::validate(&document).map_err(anyhow::Error::msg)?;
    Ok(PdfImport {
        document,
        report: report.finish(),
    })
}

#[derive(Clone, Debug)]
enum PdfValue {
    Null,
    Bool,
    Number(f64),
    Name(Vec<u8>),
    String,
    Array(Vec<PdfValue>),
    Dict(BTreeMap<Vec<u8>, PdfValue>),
    Ref(u32, u16),
}

impl PdfValue {
    fn dict(&self) -> Option<&BTreeMap<Vec<u8>, PdfValue>> {
        match self {
            Self::Dict(dict) => Some(dict),
            _ => None,
        }
    }
    fn get(&self, key: &[u8]) -> Option<&PdfValue> {
        self.dict()?.get(key)
    }
    fn name(&self) -> Option<&[u8]> {
        match self {
            Self::Name(name) => Some(name),
            _ => None,
        }
    }
    fn number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }
    fn reference(&self) -> Option<(u32, u16)> {
        match self {
            Self::Ref(object, generation) => Some((*object, *generation)),
            _ => None,
        }
    }
    fn array(&self) -> Option<&[PdfValue]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }
}

#[derive(Clone)]
struct PdfObject {
    value: PdfValue,
    stream: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct XrefEntry {
    offset: usize,
    generation: u16,
    compressed: Option<(u32, u32)>,
}

struct PdfParser<'a> {
    bytes: &'a [u8],
    limits: ImportLimits,
    trailer: PdfValue,
    xref: BTreeMap<u32, XrefEntry>,
    objects: BTreeMap<(u32, u16), PdfObject>,
    total_decoded: usize,
    image_assets: BTreeMap<String, Value>,
    image_asset_bytes: usize,
    embedded_fonts: BTreeMap<String, Value>,
    embedded_font_bytes: usize,
}

impl<'a> PdfParser<'a> {
    fn new(bytes: &'a [u8], limits: ImportLimits) -> Result<Self> {
        ensure!(
            bytes.len() <= limits.max_input_bytes as usize,
            "[limit-exceeded] PDF exceeds input size limit"
        );
        let (xref, trailer) = parse_classic_xref(bytes)?;
        ensure!(
            xref.len() <= MAX_OBJECTS,
            "[limit-exceeded] PDF object count exceeds {MAX_OBJECTS}"
        );
        ensure!(
            trailer.get(&b"Encrypt"[..]).is_none(),
            "[unsupported-capability] encrypted PDF import is not supported"
        );
        ensure!(
            trailer.get(&b"Prev"[..]).is_none() && trailer.get(&b"XRefStm"[..]).is_none(),
            "[unsupported-capability] incremental updates and hybrid xref PDFs are not supported"
        );
        let mut parser = Self {
            bytes,
            limits,
            trailer,
            xref,
            objects: BTreeMap::new(),
            total_decoded: 0,
            image_assets: BTreeMap::new(),
            image_asset_bytes: 0,
            embedded_fonts: BTreeMap::new(),
            embedded_font_bytes: 0,
        };
        parser.parse_objects()?;
        parser.reject_active_actions()?;
        Ok(parser)
    }

    fn parse_objects(&mut self) -> Result<()> {
        let entries = self
            .xref
            .iter()
            .map(|(id, entry)| (*id, *entry))
            .collect::<Vec<_>>();
        for (id, entry) in entries {
            if id == 0 {
                continue;
            }
            if entry.compressed.is_some() {
                continue;
            }
            let object = parse_indirect_object(self.bytes, id, entry, self.limits, &self.xref)?;
            self.objects.insert((id, entry.generation), object);
        }
        let compressed = self
            .xref
            .iter()
            .filter_map(|(id, entry)| entry.compressed.map(|(stream, index)| (*id, stream, index)))
            .collect::<Vec<_>>();
        let stream_ids = compressed
            .iter()
            .map(|(_, stream, _)| *stream)
            .collect::<HashSet<_>>();
        let mut parsed_streams = BTreeMap::<u32, Vec<(u32, PdfObject)>>::new();
        for stream_id in stream_ids {
            let entry = *self
                .xref
                .get(&stream_id)
                .context("[invalid-input] object stream xref entry is missing")?;
            ensure!(
                entry.compressed.is_none(),
                "[invalid-input] an object stream cannot itself be compressed"
            );
            let object = self
                .objects
                .get(&(stream_id, entry.generation))
                .context("[invalid-input] compressed object references a missing object stream")?
                .clone();
            ensure!(
                object.value.get(&b"Type"[..]).and_then(PdfValue::name) == Some(b"ObjStm"),
                "[invalid-input] type-2 xref entry does not reference an /ObjStm"
            );
            let dict = object
                .value
                .dict()
                .context("[invalid-input] object stream has no dictionary")?;
            let count = direct_pdf_u32(dict, b"N", "object stream /N")? as usize;
            let first = direct_pdf_u32(dict, b"First", "object stream /First")? as usize;
            ensure!(
                count <= MAX_OBJECTS,
                "[limit-exceeded] object stream exceeds object limit"
            );
            let raw = object
                .stream
                .as_deref()
                .context("[invalid-input] object stream has no stream data")?;
            let decoded = self.decode_stream(dict, raw)?;
            ensure!(
                first <= decoded.len(),
                "[invalid-input] object stream /First is outside decoded data"
            );
            let mut header = ValueParser::new(&decoded[..first], 0);
            let mut headers = Vec::with_capacity(count);
            let mut ids = HashSet::new();
            for index in 0..count {
                let id = header.u32()?;
                let offset = header.u32()? as usize;
                ensure!(
                    id > 0 && ids.insert(id),
                    "[invalid-input] duplicate or zero object ID in object stream"
                );
                ensure!(
                    self.xref.get(&id).and_then(|entry| entry.compressed)
                        == Some((stream_id, index as u32)),
                    "[invalid-input] object stream header disagrees with the xref index"
                );
                ensure!(
                    index != 0 || offset == 0,
                    "[invalid-input] first object stream member offset must be zero"
                );
                headers.push((id, offset));
            }
            header.skip_ws();
            ensure!(
                header.pos == first,
                "[invalid-input] object stream header does not end at /First"
            );
            let body = &decoded[first..];
            let mut values = Vec::with_capacity(count);
            let mut last = 0usize;
            for index in 0..count {
                let (id, offset) = headers[index];
                let end = headers
                    .get(index + 1)
                    .map(|(_, offset)| *offset)
                    .unwrap_or(body.len());
                ensure!(
                    offset >= last && end >= offset && end <= body.len(),
                    "[invalid-input] object stream member bounds are invalid"
                );
                let mut parser = ValueParser::new(&body[..end], offset);
                let value = parser.value(0)?;
                parser.skip_ws();
                ensure!(
                    parser.pos == end,
                    "[invalid-input] object stream member contains trailing tokens"
                );
                values.push((
                    id,
                    PdfObject {
                        value,
                        stream: None,
                    },
                ));
                last = offset;
            }
            parsed_streams.insert(stream_id, values);
        }
        for (id, stream_id, index) in compressed {
            let (declared_id, object) = parsed_streams
                .get(&stream_id)
                .and_then(|objects| objects.get(index as usize))
                .context("[invalid-input] object stream xref index exceeds /N")?;
            ensure!(
                *declared_id == id,
                "[invalid-input] object stream xref index names a different object"
            );
            self.objects.insert((id, 0), object.clone());
        }
        Ok(())
    }

    fn resolve(&self, reference: (u32, u16)) -> Result<&PdfObject> {
        self.objects.get(&reference).with_context(|| {
            format!(
                "[invalid-input] PDF reference {} {} R is missing",
                reference.0, reference.1
            )
        })
    }

    fn deref<'b>(&'b self, value: &'b PdfValue) -> Result<&'b PdfValue> {
        if let Some(reference) = value.reference() {
            Ok(&self.resolve(reference)?.value)
        } else {
            Ok(value)
        }
    }

    fn resolve_text_font(
        &mut self,
        resources: Option<&PdfValue>,
        resource_name: &str,
        source_id: &str,
        report: &mut FidelityReportBuilder,
    ) -> Result<Option<PdfTextFont>> {
        let item_id = format!("{source_id}:{resource_name}");
        let unsupported = |report: &mut FidelityReportBuilder, reason: &str| {
            report.record(
                &item_id,
                "embedded_font",
                FidelityStatus::Dropped,
                reason,
                Some("use a permitted embedded TrueType simple font with one-byte ToUnicode bfchar/bfrange mappings and explicit widths".into()),
            );
        };
        let Some(resources) = resources else {
            unsupported(report, "PDF text font has no page resources");
            return Ok(None);
        };
        let resources = self.deref(resources)?.clone();
        let Some(fonts_value) = resources.get(&b"Font"[..]).cloned() else {
            unsupported(report, "PDF page resources have no /Font dictionary");
            return Ok(None);
        };
        let fonts = self.deref(&fonts_value)?.clone();
        let Some(reference) = fonts
            .dict()
            .and_then(|dict| dict.get(resource_name.as_bytes()))
            .and_then(PdfValue::reference)
        else {
            unsupported(
                report,
                "PDF font resource does not resolve to an indirect font object",
            );
            return Ok(None);
        };
        let font = self.resolve(reference)?.clone();
        let Some(font_dict) = font.value.dict() else {
            unsupported(report, "PDF font resource is not a dictionary");
            return Ok(None);
        };
        let font_dict = font_dict.clone();
        if font_dict.get(&b"Subtype"[..]).and_then(PdfValue::name) != Some(b"TrueType") {
            unsupported(
                report,
                "only simple PDF /TrueType fonts are imported with embedding",
            );
            return Ok(None);
        }
        let Some(descriptor_value) = font_dict.get(&b"FontDescriptor"[..]).cloned() else {
            unsupported(report, "PDF TrueType font has no /FontDescriptor");
            return Ok(None);
        };
        let descriptor = self.deref(&descriptor_value)?.clone();
        let Some(descriptor) = descriptor.dict() else {
            unsupported(report, "PDF /FontDescriptor is not a dictionary");
            return Ok(None);
        };
        let Some(file_ref) = descriptor
            .get(&b"FontFile2"[..])
            .and_then(PdfValue::reference)
        else {
            unsupported(
                report,
                "PDF TrueType font has no embedded /FontFile2 program",
            );
            return Ok(None);
        };
        let font_file = self.resolve(file_ref)?.clone();
        let raw_font = font_file
            .stream
            .as_deref()
            .context("[invalid-input] PDF /FontFile2 has no stream")?;
        let font_bytes = self.decode_stream(
            font_file
                .value
                .dict()
                .context("[invalid-input] PDF /FontFile2 has no dictionary")?,
            raw_font,
        )?;
        ensure!(
            font_bytes.len() <= 6 * 1024 * 1024,
            "[limit-exceeded] embedded PDF font exceeds 6 MiB"
        );
        let Some(fs_type) = sfnt_fs_type(&font_bytes) else {
            unsupported(
                report,
                "embedded PDF TrueType font has no readable OS/2 embedding permissions",
            );
            return Ok(None);
        };
        // Restrict this first implementation to installable-embedding fonts.
        if fs_type != 0 {
            unsupported(
                report,
                "PDF font embedding permission is not installable (OS/2 fsType must be zero)",
            );
            return Ok(None);
        }
        let Some(cmap_ref) = font_dict
            .get(&b"ToUnicode"[..])
            .and_then(PdfValue::reference)
        else {
            unsupported(
                report,
                "PDF TrueType font has no indirect /ToUnicode stream",
            );
            return Ok(None);
        };
        let cmap_object = self.resolve(cmap_ref)?.clone();
        let cmap_dict = cmap_object
            .value
            .dict()
            .context("[invalid-input] PDF /ToUnicode has no stream dictionary")?
            .clone();
        let cmap_stream = cmap_object
            .stream
            .as_deref()
            .context("[invalid-input] PDF /ToUnicode has no stream")?;
        let cmap_bytes = self.decode_stream(&cmap_dict, cmap_stream)?;
        let Some(to_unicode) = parse_simple_tounicode(&cmap_bytes)? else {
            unsupported(
                report,
                "PDF /ToUnicode CMap is outside the bounded one-byte bfchar/bfrange subset",
            );
            return Ok(None);
        };
        let Some(first_char) = font_dict
            .get(&b"FirstChar"[..])
            .and_then(PdfValue::number)
            .filter(|n| n.fract() == 0.0 && (0.0..=255.0).contains(n))
        else {
            unsupported(report, "PDF TrueType font has no valid /FirstChar");
            return Ok(None);
        };
        let Some(widths) = font_dict.get(&b"Widths"[..]).and_then(PdfValue::array) else {
            unsupported(report, "PDF TrueType font has no direct /Widths array");
            return Ok(None);
        };
        let mut width_map = BTreeMap::new();
        for (index, width) in widths.iter().enumerate() {
            let code = first_char as usize + index;
            if code > u8::MAX as usize {
                break;
            }
            let Some(width) = width
                .number()
                .filter(|w| w.is_finite() && (0.0..=100_000.0).contains(w))
            else {
                unsupported(report, "PDF TrueType font contains a malformed width");
                return Ok(None);
            };
            width_map.insert(code as u8, width);
        }
        if to_unicode.keys().any(|code| !width_map.contains_key(code)) {
            unsupported(
                report,
                "PDF font lacks an explicit width for a ToUnicode character",
            );
            return Ok(None);
        }
        let Some(face) = rustybuzz::ttf_parser::Face::parse(&font_bytes, 0).ok() else {
            unsupported(report, "embedded PDF font is not a parseable TrueType face");
            return Ok(None);
        };
        if to_unicode
            .values()
            .flat_map(|text| text.chars())
            .any(|character| face.glyph_index(character).is_none())
        {
            unsupported(
                report,
                "embedded PDF TrueType cmap does not cover every ToUnicode character",
            );
            return Ok(None);
        }
        let font_id = format!("pdf-{}", &hex::encode(Sha256::digest(&font_bytes))[..16]);
        if !self.embedded_fonts.contains_key(&font_id) {
            ensure!(
                self.embedded_fonts.len() < 64,
                "[limit-exceeded] imported PDF exceeds 64 embedded fonts"
            );
            let asset = crate::fonts::asset(font_id.clone(), font_bytes.clone()).context(
                "[unsupported-capability] embedded PDF font failed Pentool font validation",
            )?;
            let new_total = self
                .embedded_font_bytes
                .checked_add(asset.data.len())
                .context("[limit-exceeded] embedded PDF font byte count overflow")?;
            ensure!(
                new_total <= 16 * 1024 * 1024,
                "[limit-exceeded] PDF embedded fonts exceed Pentool's 16 MiB limit"
            );
            self.embedded_font_bytes = new_total;
            self.embedded_fonts
                .insert(font_id, json!({"id":asset.id,"data":asset.data}));
        }
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_font_data(font_bytes);
        let Some(family) = db
            .faces()
            .next()
            .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
        else {
            unsupported(report, "embedded PDF TrueType font has no family name");
            return Ok(None);
        };
        report.record(
            &item_id,
            "embedded_font",
            FidelityStatus::Approximated,
            "permitted TrueType font embedded; one-byte text codes mapped through ToUnicode and explicit PDF widths",
            Some("review glyph shaping and PDF text-matrix differences".into()),
        );
        Ok(Some(PdfTextFont {
            family,
            to_unicode,
            widths: width_map,
        }))
    }

    fn pages(&mut self, report: &mut FidelityReportBuilder) -> Result<Vec<ImportedPage>> {
        let root = self
            .trailer
            .get(&b"Root"[..])
            .and_then(PdfValue::reference)
            .context("[invalid-input] PDF trailer has no Root reference")?;
        let catalog = &self.resolve(root)?.value;
        ensure!(
            catalog.get(&b"OpenAction"[..]).is_none(),
            "[unsafe-input] PDF catalog has an OpenAction"
        );
        let pages = catalog
            .get(&b"Pages"[..])
            .and_then(PdfValue::reference)
            .context("[invalid-input] PDF catalog has no Pages reference")?;
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        self.walk_pages(
            pages,
            None,
            None,
            None,
            0,
            &mut visited,
            &mut result,
            report,
        )?;
        ensure!(
            result.len() <= MAX_PAGES,
            "[limit-exceeded] PDF page count exceeds {MAX_PAGES}"
        );
        limits_node_count(
            self.limits,
            result.iter().map(|page| page.nodes.len()).sum(),
        )?;
        Ok(result)
    }

    // Page-tree state stays explicit because inheritance is scoped per child path.
    #[allow(clippy::too_many_arguments)]
    fn walk_pages(
        &mut self,
        reference: (u32, u16),
        inherited_box: Option<[f64; 4]>,
        inherited_resources: Option<PdfValue>,
        inherited_rotation: Option<f64>,
        depth: usize,
        visited: &mut HashSet<(u32, u16)>,
        output: &mut Vec<ImportedPage>,
        report: &mut FidelityReportBuilder,
    ) -> Result<()> {
        ensure!(
            depth <= MAX_OBJECT_DEPTH,
            "[limit-exceeded] PDF page tree nesting exceeds {MAX_OBJECT_DEPTH}"
        );
        self.limits.check_nesting(depth)?;
        ensure!(
            visited.insert(reference),
            "[invalid-input] PDF page tree contains a cycle"
        );
        let object = self.resolve(reference)?.clone();
        let value = object.value;
        let resources = value
            .get(&b"Resources"[..])
            .cloned()
            .or(inherited_resources);
        let kind = value
            .get(&b"Type"[..])
            .and_then(PdfValue::name)
            .unwrap_or_default();
        let page_box = value
            .get(&b"MediaBox"[..])
            .and_then(|value| self.deref(value).ok())
            .and_then(parse_box)
            .or(inherited_box);
        let page_rotation = value
            .get(&b"Rotate"[..])
            .and_then(PdfValue::number)
            .or(inherited_rotation);
        if kind == b"Pages" {
            let kids = value
                .get(&b"Kids"[..])
                .and_then(|value| self.deref(value).ok())
                .and_then(PdfValue::array)
                .context("[invalid-input] Pages node has no Kids array")?;
            let children = kids
                .iter()
                .map(|kid| {
                    kid.reference()
                        .context("[invalid-input] Pages Kids entry is not an indirect reference")
                })
                .collect::<Result<Vec<_>>>()?;
            for child in children {
                self.walk_pages(
                    child,
                    page_box,
                    resources.clone(),
                    page_rotation,
                    depth + 1,
                    visited,
                    output,
                    report,
                )?;
                ensure!(
                    output.len() <= MAX_PAGES,
                    "[limit-exceeded] PDF page count exceeds {MAX_PAGES}"
                );
            }
        } else if kind == b"Page" {
            let rect =
                page_box.context("[unsupported-capability] PDF page has no inherited MediaBox")?;
            let box_width = rect[2] - rect[0];
            let box_height = rect[3] - rect[1];
            ensure!(
                box_width.is_finite()
                    && box_height.is_finite()
                    && box_width > 0.0
                    && box_height > 0.0
                    && box_width <= MAX_PAGE_DIMENSION
                    && box_height <= MAX_PAGE_DIMENSION,
                "[limit-exceeded] PDF page dimensions are invalid or too large"
            );
            ensure!(
                page_rotation.is_none_or(|rotation| rotation.is_finite()
                    && rotation.fract() == 0.0
                    && (rotation as i32) % 90 == 0),
                "[unsupported-capability] PDF page rotation must be a multiple of 90 degrees"
            );
            let rotate = (page_rotation.unwrap_or(0.0) as i32).rem_euclid(360) as u16;
            let transform_box = PdfPageBox { rect, rotate };
            let (width, height) = if rotate == 90 || rotate == 270 {
                (box_height, box_width)
            } else {
                (box_width, box_height)
            };
            let mut page = ImportedPage {
                index: output.len() + 1,
                width,
                height,
                nodes: Vec::new(),
                operator_count: 0,
            };
            if rotate != 0 {
                report.record(
                    format!("{} {} R", reference.0, reference.1),
                    "page_rotation",
                    FidelityStatus::Approximated,
                    format!("PDF page rotation of {rotate} degrees applied to vector and image coordinates"),
                    Some("PDF text glyph orientation is not rotated; review text on this page".into()),
                );
            }
            let contents = value.get(&b"Contents"[..]);
            if let Some(contents) = contents {
                let existing_nodes = output
                    .iter()
                    .map(|page: &ImportedPage| page.nodes.len())
                    .sum::<usize>();
                let remaining_nodes = self.limits.max_nodes.saturating_sub(existing_nodes);
                self.import_contents(
                    contents,
                    transform_box,
                    resources.as_ref(),
                    &mut page,
                    report,
                    remaining_nodes,
                )?;
            }
            report.record(
                format!("{} {} R", reference.0, reference.1),
                "page",
                FidelityStatus::Exact,
                "PDF page imported as an ordinary v4 scene page",
                None,
            );
            output.push(page);
        } else {
            bail!("[invalid-input] page tree reference is neither /Pages nor /Page");
        }
        Ok(())
    }

    fn import_contents(
        &mut self,
        contents: &PdfValue,
        page_box: PdfPageBox,
        resources: Option<&PdfValue>,
        page: &mut ImportedPage,
        report: &mut FidelityReportBuilder,
        remaining_nodes: usize,
    ) -> Result<()> {
        let references: Vec<(u32, u16)> = match contents {
            PdfValue::Ref(id, generation) => vec![(*id, *generation)],
            PdfValue::Array(values) => values
                .iter()
                .map(|value| {
                    value.reference().context(
                        "[invalid-input] Contents array entry must be an indirect reference",
                    )
                })
                .collect::<Result<_>>()?,
            _ => bail!("[unsupported-capability] direct PDF content streams are not supported"),
        };
        for reference in references {
            let object = self.resolve(reference)?.clone();
            let dictionary = object
                .value
                .dict()
                .context("[invalid-input] PDF content stream object is not a dictionary")?;
            let raw = object
                .stream
                .context("[invalid-input] PDF Contents object has no stream")?;
            let source_id = format!("{} {} R", reference.0, reference.1);
            let decoded = self.decode_stream(dictionary, &raw)?;
            interpret_content(
                self,
                &decoded,
                page_box,
                resources,
                page,
                report,
                &source_id,
                remaining_nodes,
            )?;
        }
        Ok(())
    }

    fn decode_stream(
        &mut self,
        dictionary: &BTreeMap<Vec<u8>, PdfValue>,
        raw: &[u8],
    ) -> Result<Vec<u8>> {
        ensure!(
            raw.len() <= MAX_CONTENT_STREAM_BYTES,
            "[limit-exceeded] PDF encoded stream exceeds {MAX_CONTENT_STREAM_BYTES} bytes"
        );
        ensure!(
            dictionary
                .get(&b"DecodeParms"[..])
                .is_none_or(|value| matches!(value, PdfValue::Null)),
            "[unsupported-capability] PDF DecodeParms/predictors are not supported"
        );
        let filter = dictionary.get(&b"Filter"[..]);
        let decoded = match filter {
            None | Some(PdfValue::Null) => raw.to_vec(),
            Some(PdfValue::Name(name)) if name == b"FlateDecode" || name == b"Fl" => {
                let decoder = ZlibDecoder::new(raw);
                let mut limited = decoder.take((MAX_CONTENT_STREAM_BYTES + 1) as u64);
                let mut output = Vec::new();
                limited
                    .read_to_end(&mut output)
                    .context("[invalid-input] malformed FlateDecode stream")?;
                ensure!(
                    output.len() <= MAX_CONTENT_STREAM_BYTES,
                    "[limit-exceeded] PDF decoded stream exceeds {MAX_CONTENT_STREAM_BYTES} bytes"
                );
                ensure!(
                    raw.is_empty() || output.len() as u64 <= raw.len() as u64 * 100,
                    "[limit-exceeded] PDF stream expansion ratio exceeds 100:1"
                );
                output
            }
            Some(PdfValue::Array(filters))
                if filters.len() == 1
                    && filters[0]
                        .name()
                        .is_some_and(|name| name == b"FlateDecode" || name == b"Fl") =>
            {
                let decoder = ZlibDecoder::new(raw);
                let mut limited = decoder.take((MAX_CONTENT_STREAM_BYTES + 1) as u64);
                let mut output = Vec::new();
                limited
                    .read_to_end(&mut output)
                    .context("[invalid-input] malformed FlateDecode stream")?;
                ensure!(
                    output.len() <= MAX_CONTENT_STREAM_BYTES,
                    "[limit-exceeded] PDF decoded stream exceeds {MAX_CONTENT_STREAM_BYTES} bytes"
                );
                ensure!(
                    raw.is_empty() || output.len() as u64 <= raw.len() as u64 * 100,
                    "[limit-exceeded] PDF stream expansion ratio exceeds 100:1"
                );
                output
            }
            _ => bail!(
                "[unsupported-capability] PDF stream filter is outside the bounded vector subset"
            ),
        };
        self.total_decoded = self
            .total_decoded
            .checked_add(decoded.len())
            .context("[limit-exceeded] decoded PDF byte count overflow")?;
        ensure!(
            self.total_decoded <= MAX_TOTAL_DECODED_BYTES,
            "[limit-exceeded] PDF decoded streams exceed {MAX_TOTAL_DECODED_BYTES} bytes total"
        );
        Ok(decoded)
    }

    fn decode_image_stream(
        &mut self,
        dictionary: &BTreeMap<Vec<u8>, PdfValue>,
        raw: &[u8],
        predictor: Option<PngPredictor>,
    ) -> Result<Vec<u8>> {
        let mut decode_dictionary = dictionary.clone();
        decode_dictionary.remove(&b"DecodeParms"[..]);
        let decoded = self.decode_stream(&decode_dictionary, raw)?;
        match predictor {
            Some(predictor) => undo_png_predictor(decoded, predictor),
            None => Ok(decoded),
        }
    }

    fn reject_active_actions(&self) -> Result<()> {
        for object in self.objects.values() {
            reject_action_keys(&object.value, 0)?;
        }
        Ok(())
    }
}

fn reject_action_keys(value: &PdfValue, depth: usize) -> Result<()> {
    ensure!(
        depth <= MAX_OBJECT_DEPTH,
        "[limit-exceeded] PDF dictionary nesting exceeds limit"
    );
    match value {
        PdfValue::Dict(dict) => {
            for (key, child) in dict {
                ensure!(
                    !matches!(
                        key.as_slice(),
                        b"A" | b"AA"
                            | b"OpenAction"
                            | b"JavaScript"
                            | b"JS"
                            | b"Launch"
                            | b"URI"
                            | b"EmbeddedFiles"
                    ),
                    "[unsafe-input] PDF contains active action or embedded-file key /{}",
                    String::from_utf8_lossy(key)
                );
                reject_action_keys(child, depth + 1)?;
            }
        }
        PdfValue::Array(values) => {
            for child in values {
                reject_action_keys(child, depth + 1)?;
            }
        }
        PdfValue::Name(name) => ensure!(
            !matches!(
                name.as_slice(),
                b"JavaScript"
                    | b"JS"
                    | b"Launch"
                    | b"URI"
                    | b"GoToR"
                    | b"SubmitForm"
                    | b"ImportData"
            ),
            "[unsafe-input] PDF contains active action name /{}",
            String::from_utf8_lossy(name)
        ),
        _ => {}
    }
    Ok(())
}

struct ImportedPage {
    index: usize,
    width: f64,
    height: f64,
    nodes: Vec<Value>,
    operator_count: usize,
}

#[derive(Clone, Copy)]
struct PdfPageBox {
    rect: [f64; 4],
    rotate: u16,
}

fn limits_node_count(limits: ImportLimits, count: usize) -> Result<()> {
    limits.check_node_count(count)
}

fn parse_box(value: &PdfValue) -> Option<[f64; 4]> {
    let values = value.array()?;
    if values.len() != 4 {
        return None;
    }
    Some([
        values[0].number()?,
        values[1].number()?,
        values[2].number()?,
        values[3].number()?,
    ])
}

fn parse_classic_xref(bytes: &[u8]) -> Result<(BTreeMap<u32, XrefEntry>, PdfValue)> {
    let marker =
        find_last(bytes, b"startxref").context("[invalid-input] PDF has no startxref marker")?;
    let mut cursor = marker + b"startxref".len();
    skip_ws(bytes, &mut cursor);
    let xref_offset = parse_usize_token(bytes, &mut cursor)?;
    ensure!(
        xref_offset < bytes.len(),
        "[invalid-input] startxref offset is outside the file"
    );
    let mut pos = xref_offset;
    skip_ws(bytes, &mut pos);
    if bytes.get(pos..pos.saturating_add(4)) != Some(b"xref") {
        return parse_xref_stream_at(bytes, xref_offset);
    }
    ensure!(
        take_word(bytes, &mut pos)? == b"xref",
        "[invalid-input] malformed PDF cross-reference table"
    );
    let mut entries = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut total_entries = 0usize;
    let mut found_free_zero = false;
    loop {
        skip_ws(bytes, &mut pos);
        let token = take_word(bytes, &mut pos)?;
        if token == b"trailer" {
            break;
        }
        let first = std::str::from_utf8(token)?
            .parse::<u32>()
            .context("[invalid-input] expected xref subsection first object number")?;
        let count = parse_u32_token(bytes, &mut pos)? as usize;
        total_entries = total_entries
            .checked_add(count)
            .context("[limit-exceeded] xref count overflow")?;
        ensure!(
            total_entries <= MAX_OBJECTS,
            "[limit-exceeded] xref table exceeds {MAX_OBJECTS} entries"
        );
        for index in 0..count {
            let offset = parse_usize_token(bytes, &mut pos)?;
            let generation = parse_u16_token(bytes, &mut pos)?;
            let flag = take_word(bytes, &mut pos)?;
            let id = first
                .checked_add(index as u32)
                .context("[invalid-input] xref object number overflow")?;
            ensure!(
                seen.insert(id),
                "[invalid-input] duplicate xref object number"
            );
            if flag == b"n" {
                ensure!(
                    offset < bytes.len(),
                    "[invalid-input] xref offset is outside the file"
                );
                ensure!(
                    entries
                        .insert(
                            id,
                            XrefEntry {
                                offset,
                                generation,
                                compressed: None
                            }
                        )
                        .is_none(),
                    "[invalid-input] duplicate xref object number"
                );
            } else {
                ensure!(flag == b"f", "[invalid-input] invalid xref entry flag");
                if id == 0 {
                    found_free_zero = true;
                }
            }
        }
    }
    let mut parser = ValueParser::new(bytes, pos);
    let trailer = parser.value(0)?;
    ensure!(
        trailer.dict().is_some(),
        "[invalid-input] PDF trailer is not a dictionary"
    );
    ensure!(
        trailer.get(&b"XRefStm"[..]).is_none(),
        "[unsupported-capability] hybrid xref PDFs are not supported"
    );
    ensure!(
        trailer.get(&b"Prev"[..]).is_none(),
        "[unsupported-capability] incremental PDF revisions are not supported"
    );
    ensure!(
        found_free_zero,
        "[invalid-input] xref table has no free object zero entry"
    );
    Ok((entries, trailer))
}

fn parse_xref_stream_at(
    bytes: &[u8],
    offset: usize,
) -> Result<(BTreeMap<u32, XrefEntry>, PdfValue)> {
    let mut parser = ValueParser::new(bytes, offset);
    let object_id = parser.u32()?;
    let generation = parser.u16()?;
    parser.expect_word(b"obj")?;
    let dictionary = parser.value(0)?;
    ensure!(
        dictionary.get(&b"Type"[..]).and_then(PdfValue::name) == Some(b"XRef"),
        "[invalid-input] startxref does not point to an xref table or stream"
    );
    let dict = dictionary
        .dict()
        .context("[invalid-input] xref stream dictionary is malformed")?;
    parser.skip_ws();
    parser.expect_word(b"stream")?;
    parser.stream_eol()?;
    let length = dict
        .get(&b"Length"[..])
        .and_then(PdfValue::number)
        .filter(|number| {
            number.is_finite()
                && number.fract() == 0.0
                && *number >= 0.0
                && *number <= MAX_CONTENT_STREAM_BYTES as f64
        })
        .context("[unsupported-capability] xref stream /Length must be a direct bounded integer")?
        as usize;
    let end = parser
        .pos
        .checked_add(length)
        .context("[invalid-input] xref stream length overflow")?;
    ensure!(
        end <= bytes.len(),
        "[invalid-input] xref stream extends beyond EOF"
    );
    let raw = &bytes[parser.pos..end];
    let decoded = decode_xref_stream(dict, raw)?;

    let widths = dict
        .get(&b"W"[..])
        .and_then(PdfValue::array)
        .context("[invalid-input] xref stream is missing /W")?;
    ensure!(
        widths.len() == 3,
        "[invalid-input] xref stream /W must have three widths"
    );
    let mut field_widths = [0usize; 3];
    for (index, width) in widths.iter().enumerate() {
        let width = width
            .number()
            .filter(|number| {
                number.is_finite() && number.fract() == 0.0 && (0.0..=8.0).contains(number)
            })
            .context(
                "[invalid-input] xref stream field width must be an integer from 0 through 8",
            )?;
        field_widths[index] = width as usize;
    }
    let row_width = field_widths.iter().sum::<usize>();
    ensure!(
        row_width > 0,
        "[invalid-input] xref stream row width is zero"
    );
    let size = direct_pdf_u32(dict, b"Size", "xref stream /Size")?;
    ensure!(
        size as usize <= MAX_OBJECTS,
        "[limit-exceeded] PDF xref stream exceeds {MAX_OBJECTS} objects"
    );
    let ranges = dict
        .get(&b"Index"[..])
        .and_then(PdfValue::array)
        .map(|values| values.to_vec())
        .unwrap_or_else(|| vec![PdfValue::Number(0.0), PdfValue::Number(size as f64)]);
    ensure!(
        ranges.len() % 2 == 0,
        "[invalid-input] xref stream /Index must contain start/count pairs"
    );
    let mut total_entries = 0usize;
    let mut entries = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut cursor = 0usize;
    let mut found_free_zero = false;
    for pair in ranges.chunks_exact(2) {
        let first = pdf_array_u32(&pair[0], "xref /Index start")?;
        let count = pdf_array_u32(&pair[1], "xref /Index count")? as usize;
        total_entries = total_entries
            .checked_add(count)
            .context("[limit-exceeded] xref stream entry count overflow")?;
        ensure!(
            total_entries <= MAX_OBJECTS,
            "[limit-exceeded] PDF xref stream exceeds {MAX_OBJECTS} entries"
        );
        ensure!(
            first
                .checked_add(count as u32)
                .is_some_and(|end| end <= size),
            "[invalid-input] xref /Index range exceeds /Size"
        );
        for index in 0..count {
            ensure!(
                cursor
                    .checked_add(row_width)
                    .is_some_and(|end| end <= decoded.len()),
                "[invalid-input] truncated xref stream entry data"
            );
            let type_value = if field_widths[0] == 0 {
                1
            } else {
                pdf_be_integer(&decoded[cursor..cursor + field_widths[0]])?
            };
            cursor += field_widths[0];
            let field2 = pdf_be_integer(&decoded[cursor..cursor + field_widths[1]])?;
            cursor += field_widths[1];
            let field3 = pdf_be_integer(&decoded[cursor..cursor + field_widths[2]])?;
            cursor += field_widths[2];
            let id = first
                .checked_add(index as u32)
                .context("[invalid-input] xref object number overflow")?;
            ensure!(
                seen.insert(id),
                "[invalid-input] duplicate xref stream object number"
            );
            match type_value {
                0 => {
                    if id == 0 {
                        found_free_zero = true;
                    }
                }
                1 => {
                    let object_offset = usize::try_from(field2)
                        .context("[invalid-input] xref object offset exceeds platform size")?;
                    let object_generation = u16::try_from(field3)
                        .context("[invalid-input] xref generation exceeds supported range")?;
                    ensure!(
                        object_offset < bytes.len(),
                        "[invalid-input] xref object offset is outside the file"
                    );
                    ensure!(
                        entries
                            .insert(
                                id,
                                XrefEntry {
                                    offset: object_offset,
                                    generation: object_generation,
                                    compressed: None
                                }
                            )
                            .is_none(),
                        "[invalid-input] duplicate xref stream object number"
                    );
                }
                2 => {
                    let stream_id = u32::try_from(field2)
                        .context("[invalid-input] object stream number exceeds supported range")?;
                    let index = u32::try_from(field3)
                        .context("[invalid-input] object stream index exceeds supported range")?;
                    ensure!(
                        stream_id > 0 && id > 0,
                        "[invalid-input] compressed xref entry has zero object number"
                    );
                    entries.insert(
                        id,
                        XrefEntry {
                            offset: 0,
                            generation: 0,
                            compressed: Some((stream_id, index)),
                        },
                    );
                }
                _ => bail!("[invalid-input] xref stream contains an invalid entry type"),
            }
        }
    }
    ensure!(
        cursor == decoded.len(),
        "[invalid-input] xref stream has trailing entry data"
    );
    ensure!(
        found_free_zero,
        "[invalid-input] xref stream has no free object zero entry"
    );
    ensure!(
        entries
            .get(&object_id)
            .is_some_and(|entry| entry.offset == offset && entry.generation == generation),
        "[invalid-input] xref stream does not contain a matching entry for itself"
    );
    Ok((entries, dictionary))
}

fn decode_xref_stream(dictionary: &BTreeMap<Vec<u8>, PdfValue>, raw: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        dictionary
            .get(&b"DecodeParms"[..])
            .is_none_or(|value| matches!(value, PdfValue::Null)),
        "[unsupported-capability] xref stream DecodeParms are not supported"
    );
    match dictionary.get(&b"Filter"[..]) {
        None | Some(PdfValue::Null) => {
            ensure!(
                raw.len() <= MAX_CONTENT_STREAM_BYTES,
                "[limit-exceeded] xref stream exceeds size limit"
            );
            Ok(raw.to_vec())
        }
        Some(PdfValue::Name(name)) if name == b"FlateDecode" || name == b"Fl" => {
            let decoder = ZlibDecoder::new(raw);
            let mut limited = decoder.take((MAX_CONTENT_STREAM_BYTES + 1) as u64);
            let mut output = Vec::new();
            limited
                .read_to_end(&mut output)
                .context("[invalid-input] malformed xref stream FlateDecode")?;
            ensure!(
                output.len() <= MAX_CONTENT_STREAM_BYTES,
                "[limit-exceeded] decoded xref stream exceeds size limit"
            );
            ensure!(
                raw.is_empty() || output.len() as u64 <= raw.len() as u64 * 100,
                "[limit-exceeded] xref stream expansion ratio exceeds 100:1"
            );
            Ok(output)
        }
        _ => bail!("[unsupported-capability] xref stream filter is unsupported"),
    }
}

fn direct_pdf_u32(
    dictionary: &BTreeMap<Vec<u8>, PdfValue>,
    key: &[u8],
    label: &str,
) -> Result<u32> {
    dictionary
        .get(key)
        .and_then(PdfValue::number)
        .filter(|number| {
            number.is_finite() && number.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(number)
        })
        .map(|number| number as u32)
        .with_context(|| format!("[invalid-input] {label} must be a direct bounded integer"))
}

fn pdf_array_u32(value: &PdfValue, label: &str) -> Result<u32> {
    value
        .number()
        .filter(|number| {
            number.is_finite() && number.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(number)
        })
        .map(|number| number as u32)
        .with_context(|| format!("[invalid-input] {label} must be a bounded integer"))
}

fn pdf_be_integer(bytes: &[u8]) -> Result<u64> {
    ensure!(
        bytes.len() <= 8,
        "[invalid-input] xref stream integer exceeds 8 bytes"
    );
    bytes.iter().try_fold(0u64, |value, byte| {
        value
            .checked_mul(256)
            .and_then(|value| value.checked_add(*byte as u64))
            .context("[invalid-input] xref stream integer overflow")
    })
}

fn parse_indirect_object(
    bytes: &[u8],
    expected_id: u32,
    entry: XrefEntry,
    limits: ImportLimits,
    xref: &BTreeMap<u32, XrefEntry>,
) -> Result<PdfObject> {
    let mut parser = ValueParser::new(bytes, entry.offset);
    let id = parser.u32()?;
    let generation = parser.u16()?;
    ensure!(
        id == expected_id && generation == entry.generation,
        "[invalid-input] xref offset does not point to its declared object"
    );
    parser.expect_word(b"obj")?;
    let value = parser.value(0)?;
    let mut stream = None;
    parser.skip_ws();
    if parser.consume_word(b"stream") {
        let dictionary = value
            .dict()
            .context("[invalid-input] stream object has no dictionary")?;
        let length = dictionary
            .get(&b"Length"[..])
            .context("[invalid-input] stream has no Length")?;
        let length = match length {
            PdfValue::Number(value) if *value >= 0.0 && value.fract() == 0.0 => *value as usize,
            PdfValue::Ref(id, generation) => {
                indirect_integer(bytes, limits, xref, *id, *generation, 0)?
            }
            _ => {
                bail!("[unsupported-capability] stream Length must be a direct or indirect integer")
            }
        };
        ensure!(
            length <= MAX_CONTENT_STREAM_BYTES,
            "[limit-exceeded] PDF stream is larger than {MAX_CONTENT_STREAM_BYTES} bytes"
        );
        parser.stream_eol()?;
        let end = parser
            .pos
            .checked_add(length)
            .context("[invalid-input] stream boundary overflow")?;
        ensure!(
            end <= bytes.len(),
            "[invalid-input] PDF stream extends past EOF"
        );
        let content = bytes[parser.pos..end].to_vec();
        parser.pos = end;
        parser.skip_ws();
        parser.expect_word(b"endstream")?;
        stream = Some(content);
    }
    parser.skip_ws();
    parser.expect_word(b"endobj")?;
    Ok(PdfObject { value, stream })
}

fn indirect_integer(
    bytes: &[u8],
    _limits: ImportLimits,
    xref: &BTreeMap<u32, XrefEntry>,
    id: u32,
    generation: u16,
    depth: usize,
) -> Result<usize> {
    ensure!(
        depth < MAX_OBJECT_DEPTH,
        "[limit-exceeded] PDF indirect Length nesting exceeds limit"
    );
    let entry = xref
        .get(&id)
        .with_context(|| format!("[invalid-input] stream Length object {id} is missing"))?;
    ensure!(
        entry.generation == generation,
        "[invalid-input] indirect Length generation mismatch"
    );
    let mut parser = ValueParser::new(bytes, entry.offset);
    ensure!(
        parser.u32()? == id && parser.u16()? == generation,
        "[invalid-input] indirect Length object offset mismatch"
    );
    parser.expect_word(b"obj")?;
    let value = parser.value(depth + 1)?;
    parser.skip_ws();
    parser.expect_word(b"endobj")?;
    match value {
        PdfValue::Number(number)
            if number >= 0.0
                && number.fract() == 0.0
                && number <= MAX_CONTENT_STREAM_BYTES as f64 =>
        {
            Ok(number as usize)
        }
        _ => bail!("[invalid-input] indirect stream Length is not a bounded nonnegative integer"),
    }
}

#[derive(Clone)]
enum ContentToken {
    Number(f64),
    Name(String),
    String(Vec<u8>),
    ArrayStart,
    ArrayEnd,
    Word(String),
    Other,
}

fn content_tokens(data: &[u8]) -> Result<Vec<ContentToken>> {
    let mut pos = 0usize;
    let mut tokens = Vec::new();
    while pos < data.len() {
        skip_ws(data, &mut pos);
        if pos >= data.len() {
            break;
        }
        if data[pos] == b'(' {
            let (value, next) = content_literal_string(data, pos)?;
            pos = next;
            tokens.push(ContentToken::String(value));
        } else if data[pos] == b'<' && data.get(pos + 1) != Some(&b'<') {
            let (value, next) = content_hex_string(data, pos)?;
            pos = next;
            tokens.push(ContentToken::String(value));
        } else if data[pos] == b'/' {
            let mut parser = ValueParser::new(data, pos);
            let PdfValue::Name(name) = parser.name()? else {
                unreachable!()
            };
            pos = parser.pos;
            tokens.push(ContentToken::Name(
                String::from_utf8_lossy(&name).into_owned(),
            ));
        } else if data[pos] == b'[' {
            pos += 1;
            tokens.push(ContentToken::ArrayStart);
        } else if data[pos] == b']' {
            pos += 1;
            tokens.push(ContentToken::ArrayEnd);
        } else if matches!(data[pos], b'<' | b'>') {
            let b = data[pos];
            pos += 1;
            if matches!(b, b'<' | b'>') && data.get(pos) == Some(&b) {
                pos += 1;
            }
            tokens.push(ContentToken::Other);
        } else {
            let start = pos;
            while data.get(pos).is_some_and(|byte| !is_delimiter(*byte)) {
                pos += 1;
            }
            ensure!(
                pos > start,
                "[invalid-input] malformed content token at byte {pos}"
            );
            let token = std::str::from_utf8(&data[start..pos])
                .context("[invalid-input] non-ASCII PDF content token")?;
            if let Ok(number) = token.parse::<f64>() {
                ensure!(
                    number.is_finite(),
                    "[invalid-input] non-finite PDF content number"
                );
                tokens.push(ContentToken::Number(number));
            } else if matches!(token, "true" | "false" | "null") {
                tokens.push(ContentToken::Other);
            } else {
                tokens.push(ContentToken::Word(token.to_owned()));
            }
        }
        ensure!(
            tokens.len() <= MAX_CONTENT_TOKENS,
            "[limit-exceeded] PDF content token count exceeds {MAX_CONTENT_TOKENS}"
        );
    }
    Ok(tokens)
}

#[derive(Clone, Copy)]
struct Affine([f64; 6]);
impl Affine {
    const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    fn transform(self, x: f64, y: f64, page_box: PdfPageBox) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        let px = a * x + c * y + e;
        let py = b * x + d * y + f;
        let [x0, y0, x1, y1] = page_box.rect;
        let width = x1 - x0;
        let height = y1 - y0;
        match page_box.rotate {
            90 => (py - y0, width - (px - x0)),
            180 => (width - (px - x0), py - y0),
            270 => (height - (py - y0), px - x0),
            _ => (px - x0, y1 - py),
        }
    }
    fn then(self, next: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [g, h, i, j, k, l] = next.0;
        Self([
            a * g + c * h,
            b * g + d * h,
            a * i + c * j,
            b * i + d * j,
            a * k + c * l + e,
            b * k + d * l + f,
        ])
    }
}

#[derive(Clone)]
struct GraphicsState {
    ctm: Affine,
    fill: String,
    stroke: String,
    width: f64,
    miter_limit: f64,
    cap: &'static str,
    join: &'static str,
    device_cmyk_approximated: bool,
}
impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Affine::IDENTITY,
            fill: "#000000".into(),
            stroke: "#000000".into(),
            width: 1.0,
            // PDF's graphics-state default is 10; the SVG/scene default differs.
            miter_limit: 10.0,
            cap: "butt",
            join: "miter",
            device_cmyk_approximated: false,
        }
    }
}

// PDF graphics state and per-page diagnostics are intentionally threaded explicitly.
#[allow(clippy::too_many_arguments)]
fn interpret_content(
    parser: &mut PdfParser<'_>,
    data: &[u8],
    page_box: PdfPageBox,
    resources: Option<&PdfValue>,
    page: &mut ImportedPage,
    report: &mut FidelityReportBuilder,
    stream_id: &str,
    remaining_nodes: usize,
) -> Result<()> {
    let tokens = content_tokens(data)?;
    let mut operands = Vec::<ContentToken>::new();
    let mut state = GraphicsState::default();
    let mut stack = Vec::<GraphicsState>::new();
    let mut path = String::new();
    let mut current_point = None::<(f64, f64)>;
    let mut subpath_start = None::<(f64, f64)>;
    let mut node_index = page.nodes.len();
    let mut operator_count = page.operator_count;
    let mut dropped_text = false;
    let mut text_x = 0.0;
    let mut text_y = 0.0;
    let mut text_leading = 0.0;
    let mut font_size = 12.0;
    let mut font_name = "Helvetica".to_owned();
    let mut active_font = None::<PdfTextFont>;

    for token in tokens {
        let ContentToken::Word(op) = token else {
            operands.push(token);
            ensure!(
                operands.len() <= 64,
                "[limit-exceeded] PDF content operand stack exceeds 64 values"
            );
            continue;
        };
        operator_count += 1;
        ensure!(
            operator_count <= MAX_CONTENT_OPERATORS,
            "[limit-exceeded] PDF page exceeds {MAX_CONTENT_OPERATORS} content operators"
        );
        let numbers = operands
            .iter()
            .filter_map(|value| match value {
                ContentToken::Number(number) => Some(*number),
                _ => None,
            })
            .collect::<Vec<_>>();
        match op.as_str() {
            "q" => {
                ensure!(numbers.is_empty(), "[invalid-input] q takes no operands");
                ensure!(
                    stack.len() < MAX_OBJECT_DEPTH,
                    "[limit-exceeded] PDF graphics state nesting exceeds limit"
                );
                stack.push(state.clone());
            }
            "Q" => {
                ensure!(numbers.is_empty(), "[invalid-input] Q takes no operands");
                state = stack
                    .pop()
                    .context("[invalid-input] unbalanced PDF Q graphics-state restore")?;
            }
            "cm" => {
                ensure!(
                    numbers.len() == 6,
                    "[invalid-input] cm requires six numbers"
                );
                let matrix = Affine([
                    numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
                ]);
                ensure!(
                    matrix
                        .0
                        .iter()
                        .all(|value| value.is_finite() && value.abs() <= 1.0e9),
                    "[limit-exceeded] PDF transform coefficient exceeds bound"
                );
                state.ctm = state.ctm.then(matrix);
            }
            "w" => {
                ensure!(
                    numbers.len() == 1 && (0.0..=4096.0).contains(&numbers[0]),
                    "[invalid-input] PDF stroke width must be in 0..=4096"
                );
                state.width = numbers[0];
            }
            "J" => {
                ensure!(
                    numbers.len() == 1 && (0.0..=2.0).contains(&numbers[0]),
                    "[invalid-input] invalid PDF line cap"
                );
                state.cap = ["butt", "round", "square"][numbers[0] as usize];
            }
            "j" => {
                ensure!(
                    numbers.len() == 1 && (0.0..=2.0).contains(&numbers[0]),
                    "[invalid-input] invalid PDF line join"
                );
                state.join = ["miter", "round", "bevel"][numbers[0] as usize];
            }
            "M" => {
                ensure!(
                    numbers.len() == 1 && (1.0..=1_000_000.0).contains(&numbers[0]),
                    "[invalid-input] PDF miter limit must be within 1..=1000000"
                );
                state.miter_limit = numbers[0];
            }
            "rg" | "RG" => {
                ensure!(
                    numbers.len() == 3 && numbers.iter().all(|value| (0.0..=1.0).contains(value)),
                    "[invalid-input] RGB components must be in 0..=1"
                );
                let color = rgb_hex(numbers[0], numbers[1], numbers[2]);
                if op == "rg" {
                    state.fill = color;
                } else {
                    state.stroke = color;
                }
            }
            "g" | "G" => {
                ensure!(
                    numbers.len() == 1 && (0.0..=1.0).contains(&numbers[0]),
                    "[invalid-input] gray component must be in 0..=1"
                );
                let color = rgb_hex(numbers[0], numbers[0], numbers[0]);
                if op == "g" {
                    state.fill = color;
                } else {
                    state.stroke = color;
                }
            }
            "k" | "K" => {
                ensure!(
                    numbers.len() == 4 && numbers.iter().all(|value| (0.0..=1.0).contains(value)),
                    "[invalid-input] CMYK components must be in 0..=1"
                );
                let (c, m, y, k) = (numbers[0], numbers[1], numbers[2], numbers[3]);
                let color = rgb_hex(
                    (1.0 - c) * (1.0 - k),
                    (1.0 - m) * (1.0 - k),
                    (1.0 - y) * (1.0 - k),
                );
                if op == "k" {
                    state.fill = color;
                } else {
                    state.stroke = color;
                }
                state.device_cmyk_approximated = true;
                report.record(
                    stream_id,
                    "cmyk_color",
                    FidelityStatus::Approximated,
                    "CMYK color converted to RGB for the v4 scene",
                    Some("Review this color against a color-managed PDF renderer".into()),
                );
            }
            "m" => {
                ensure!(numbers.len() == 2, "[invalid-input] m requires two numbers");
                let point = state.ctm.transform(numbers[0], numbers[1], page_box);
                append_path(&mut path, &format!("M {} {}", point.0, point.1))?;
                current_point = Some(point);
                subpath_start = Some(point);
            }
            "l" => {
                ensure!(numbers.len() == 2, "[invalid-input] l requires two numbers");
                let point = state.ctm.transform(numbers[0], numbers[1], page_box);
                append_path(&mut path, &format!("L {} {}", point.0, point.1))?;
                current_point = Some(point);
            }
            "c" => {
                ensure!(numbers.len() == 6, "[invalid-input] c requires six numbers");
                let p1 = state.ctm.transform(numbers[0], numbers[1], page_box);
                let p2 = state.ctm.transform(numbers[2], numbers[3], page_box);
                let endpoint = state.ctm.transform(numbers[4], numbers[5], page_box);
                append_path(
                    &mut path,
                    &format!(
                        "C {} {} {} {} {} {}",
                        p1.0, p1.1, p2.0, p2.1, endpoint.0, endpoint.1
                    ),
                )?;
                current_point = Some(endpoint);
            }
            "v" => {
                ensure!(
                    numbers.len() == 4,
                    "[invalid-input] v requires four numbers"
                );
                let p1 = state.ctm.transform(numbers[0], numbers[1], page_box);
                let endpoint = state.ctm.transform(numbers[2], numbers[3], page_box);
                let start = current_point
                    .context("[invalid-input] PDF v curve operator has no current point")?;
                append_path(
                    &mut path,
                    &format!(
                        "C {} {} {} {} {} {}",
                        start.0, start.1, p1.0, p1.1, endpoint.0, endpoint.1
                    ),
                )?;
                current_point = Some(endpoint);
            }
            "y" => {
                ensure!(
                    numbers.len() == 4,
                    "[invalid-input] y requires four numbers"
                );
                let p1 = state.ctm.transform(numbers[0], numbers[1], page_box);
                let endpoint = state.ctm.transform(numbers[2], numbers[3], page_box);
                append_path(
                    &mut path,
                    &format!(
                        "C {} {} {} {} {} {}",
                        p1.0, p1.1, endpoint.0, endpoint.1, endpoint.0, endpoint.1
                    ),
                )?;
                current_point = Some(endpoint);
            }
            "h" => {
                ensure!(numbers.is_empty(), "[invalid-input] h takes no operands");
                append_path(&mut path, "Z")?;
                current_point = subpath_start;
            }
            "re" => {
                ensure!(
                    numbers.len() == 4,
                    "[invalid-input] re requires four numbers"
                );
                let (x, y, w, h) = (numbers[0], numbers[1], numbers[2], numbers[3]);
                let p0 = state.ctm.transform(x, y, page_box);
                let p1 = state.ctm.transform(x + w, y, page_box);
                let p2 = state.ctm.transform(x + w, y + h, page_box);
                let p3 = state.ctm.transform(x, y + h, page_box);
                append_path(
                    &mut path,
                    &format!(
                        "M {} {} L {} {} L {} {} L {} {} Z",
                        p0.0, p0.1, p1.0, p1.1, p2.0, p2.1, p3.0, p3.1
                    ),
                )?;
                current_point = Some(p0);
                subpath_start = Some(p0);
            }
            "S" | "s" | "f" | "F" | "B" | "b" | "n" => {
                ensure!(
                    numbers.is_empty(),
                    "[invalid-input] painting operator {op} takes no operands"
                );
                if op == "s" || op == "b" {
                    append_path(&mut path, "Z")?;
                }
                let (fill, stroke) = match op.as_str() {
                    "S" | "s" => ("none", state.stroke.as_str()),
                    "f" | "F" => (state.fill.as_str(), "none"),
                    "B" | "b" => (state.fill.as_str(), state.stroke.as_str()),
                    _ => ("none", "none"),
                };
                if !path.is_empty() && op != "n" {
                    ensure!(
                        page.nodes.len() < remaining_nodes,
                        "[limit-exceeded] imported PDF node count exceeds configured limit"
                    );
                    node_index += 1;
                    let id = format!("pdf-page-{:03}-path-{node_index:06}", page.index);
                    page.nodes.push(json!({
                        "kind":"path", "id":id, "d":path, "closed":path.trim_end().ends_with('Z'),
                        "style":{
                            "fill":{"fallback":fill}, "stroke":{"fallback":stroke},
                            "stroke_width":{"fallback":state.width}, "stroke_linecap":{"fallback":state.cap},
                            "stroke_linejoin":{"fallback":state.join}, "stroke_miterlimit":{"fallback":state.miter_limit}
                        }
                    }));
                    report.record(
                        id,
                        "vector_path",
                        if state.device_cmyk_approximated { FidelityStatus::Approximated } else { FidelityStatus::Exact },
                        if state.device_cmyk_approximated { "PDF DeviceCMYK color converted to sRGB using the simple device formula; path and supported paint style mapped to a v4 path node" } else { "PDF path and supported paint style mapped to a v4 path node" },
                        state.device_cmyk_approximated.then(|| "Review the imported color against the source PDF output intent or calibrated profile".into()),
                    );
                }
                path.clear();
                current_point = None;
                subpath_start = None;
            }
            "Do" => {
                let name = operands
                    .iter()
                    .rev()
                    .find_map(|token| {
                        if let ContentToken::Name(name) = token {
                            Some(name.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("unknown");
                import_xobject(
                    parser,
                    resources,
                    name,
                    state.ctm,
                    page_box,
                    page,
                    report,
                    stream_id,
                    &mut node_index,
                    remaining_nodes,
                )?;
            }
            "BT" => {
                text_x = 0.0;
                text_y = 0.0;
            }
            "ET" => {}
            "Tf" => {
                ensure!(
                    operands.len() >= 2,
                    "[invalid-input] Tf requires a font resource name and size"
                );
                font_name = operands
                    .iter()
                    .rev()
                    .find_map(|token| match token {
                        ContentToken::Name(name) => Some(name.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "Helvetica".into());
                active_font = parser.resolve_text_font(resources, &font_name, stream_id, report)?;
                ensure!(
                    numbers.len() == 1 && (0.0..=4096.0).contains(&numbers[0]),
                    "[invalid-input] PDF font size must be in 0..=4096"
                );
                font_size = numbers[0];
            }
            "Td" | "TD" => {
                ensure!(
                    numbers.len() == 2,
                    "[invalid-input] {op} requires two numbers"
                );
                text_x += numbers[0];
                text_y += numbers[1];
                if op == "TD" {
                    text_leading = -numbers[1];
                }
            }
            "Tm" => {
                ensure!(
                    numbers.len() == 6,
                    "[invalid-input] Tm requires six numbers"
                );
                ensure!(
                    (numbers[0] - 1.0).abs() <= 1.0e-9
                        && numbers[1].abs() <= 1.0e-9
                        && numbers[2].abs() <= 1.0e-9
                        && (numbers[3] - 1.0).abs() <= 1.0e-9,
                    "[unsupported-capability] rotated/scaled PDF text matrices are not supported"
                );
                text_x = numbers[4];
                text_y = numbers[5];
            }
            "TL" => {
                ensure!(numbers.len() == 1, "[invalid-input] TL requires one number");
                text_leading = numbers[0];
            }
            "T*" => {
                ensure!(numbers.is_empty(), "[invalid-input] T* takes no operands");
                text_y -= text_leading;
            }
            "Tj" | "'" => {
                if op == "'" {
                    ensure!(
                        numbers.is_empty(),
                        "[invalid-input] quote operator takes no numbers"
                    );
                    text_y -= text_leading;
                }
                let bytes = operands.iter().rev().find_map(|token| match token {
                    ContentToken::String(bytes) => Some(bytes.as_slice()),
                    _ => None,
                });
                ensure!(
                    bytes.is_some(),
                    "[invalid-input] {op} requires a PDF string"
                );
                let bytes = bytes.unwrap_or_default();
                let mapped = active_font
                    .as_ref()
                    .and_then(|font| decode_pdf_font_text(bytes, font, font_size));
                let fallback = if mapped.is_none() {
                    supported_pdf_text(bytes)
                } else {
                    None
                };
                if mapped.is_some() || fallback.is_some() {
                    ensure!(
                        page.nodes.len() < remaining_nodes,
                        "[limit-exceeded] imported PDF node count exceeds configured limit"
                    );
                    node_index += 1;
                    let (x, y) = state.ctm.transform(text_x, text_y, page_box);
                    let id = format!("pdf-page-{:03}-text-{node_index:06}", page.index);
                    let content = mapped
                        .as_ref()
                        .map(|value| value.0.as_str())
                        .or(fallback.as_deref())
                        .unwrap_or_default();
                    let family = active_font
                        .as_ref()
                        .filter(|_| mapped.is_some())
                        .map(|font| font.family.as_str())
                        .unwrap_or("Helvetica");
                    page.nodes.push(json!({
                        "kind":"text", "id":id, "content":content, "x":x, "y":y,
                        "font_family":family, "font_size":font_size,
                        "style":{"fill":{"fallback":state.fill}}
                    }));
                    report.record(
                        id,
                        "text",
                        FidelityStatus::Approximated,
                        if mapped.is_some() { format!("text extracted from PDF resource /{font_name} using its embedded font and ToUnicode map") } else { format!("ASCII text extracted from PDF using resource /{font_name}; font mapping and glyph advances use a Helvetica fallback") },
                        Some("Review font rendering and text placement; outline text for exact appearance".into()),
                    );
                    let advance = mapped.as_ref().map(|value| value.1).unwrap_or_else(|| {
                        content
                            .chars()
                            .map(|character| if character == '\n' { 0.0 } else { 0.5 })
                            .sum::<f64>()
                            * font_size
                    });
                    text_x += advance;
                } else {
                    report.record(
                        stream_id,
                        "text",
                        FidelityStatus::Dropped,
                        "PDF text string uses a non-ASCII or control-byte encoding that this importer cannot map safely",
                        Some("Use a Unicode ToUnicode font map or outline the text before import".into()),
                    );
                }
            }
            "TJ" => {
                ensure!(
                    operands.len() >= 2
                        && matches!(operands.first(), Some(ContentToken::ArrayStart))
                        && matches!(operands.last(), Some(ContentToken::ArrayEnd)),
                    "[invalid-input] TJ requires one string/number array"
                );
                let Some(font) = active_font.as_ref() else {
                    report.record(
                        stream_id,
                        "text",
                        FidelityStatus::Dropped,
                        "PDF TJ array has no supported embedded simple TrueType font mapping",
                        Some("use a permitted embedded TrueType font with a one-byte ToUnicode map, or outline the text".into()),
                    );
                    operands.clear();
                    page.operator_count = operator_count;
                    continue;
                };
                let items = &operands[1..operands.len() - 1];
                ensure!(
                    items.iter().all(|item| matches!(
                        item,
                        ContentToken::String(_) | ContentToken::Number(_)
                    )),
                    "[invalid-input] TJ array contains nested or unsupported values"
                );
                for item in items {
                    match item {
                        ContentToken::Number(adjustment) => {
                            ensure!(
                                adjustment.abs() <= 1_000_000.0,
                                "[limit-exceeded] PDF TJ adjustment exceeds supported bounds"
                            );
                            text_x -= adjustment * font_size / 1000.0;
                        }
                        ContentToken::String(bytes) if !bytes.is_empty() => {
                            let Some((content, advance)) =
                                decode_pdf_font_text(bytes, font, font_size)
                            else {
                                report.record(
                                    stream_id,
                                    "text",
                                    FidelityStatus::Dropped,
                                    "PDF TJ string contains a code not covered by the embedded font ToUnicode map and widths",
                                    Some("provide complete one-byte ToUnicode mappings and explicit PDF widths, or outline the text".into()),
                                );
                                continue;
                            };
                            ensure!(
                                page.nodes.len() < remaining_nodes,
                                "[limit-exceeded] imported PDF node count exceeds configured limit"
                            );
                            node_index += 1;
                            let (x, y) = state.ctm.transform(text_x, text_y, page_box);
                            let id = format!("pdf-page-{:03}-text-{node_index:06}", page.index);
                            page.nodes.push(json!({
                                "kind":"text", "id":id, "content":content, "x":x, "y":y,
                                "font_family":font.family, "font_size":font_size,
                                "style":{"fill":{"fallback":state.fill}}
                            }));
                            report.record(
                                id,
                                "text",
                                FidelityStatus::Approximated,
                                "PDF TJ string mapped through its permitted embedded font; numeric character positioning retained",
                                Some("review text appearance in the target renderer; outline text for exact glyph placement".into()),
                            );
                            text_x += advance;
                        }
                        ContentToken::String(_) => {}
                        _ => unreachable!("TJ array item was validated above"),
                    }
                }
            }
            "Tc" | "Tw" | "Tz" | "Tr" | "Ts" | "\"" => {
                if !dropped_text {
                    report.record(stream_id, "text", FidelityStatus::Dropped, "PDF text spacing, text rendering modes, or TJ arrays are not supported", Some("Convert this text run to Tj with a simple text matrix, or outline it before import".into()));
                    dropped_text = true;
                }
            }
            "BI" => bail!(
                "[unsupported-capability] inline images are not supported; import stopped without returning a partial scene"
            ),
            "W" | "W*" | "gs" | "CS" | "cs" | "SC" | "SCN" | "sc" | "scn" | "sh" | "d"
            | "ri" | "i" => bail!(
                "[unsupported-capability] PDF content operator {op} changes clipping, transparency, color, or stroke state outside the supported subset"
            ),
            _ => bail!(
                "[unsupported-capability] unsupported PDF content operator {op}; import stopped without returning a partial scene"
            ),
        }
        operands.clear();
        page.operator_count = operator_count;
    }
    ensure!(
        stack.is_empty(),
        "[invalid-input] unbalanced PDF q/Q graphics-state stack"
    );
    if !path.is_empty() {
        report.record(
            stream_id,
            "unpainted_path",
            FidelityStatus::Dropped,
            "PDF path was constructed but not painted",
            None,
        );
    }
    Ok(())
}

fn append_path(path: &mut String, command: &str) -> Result<()> {
    if !path.is_empty() {
        path.push(' ');
    }
    path.push_str(command);
    ensure!(
        path.len() <= MAX_PATH_BYTES,
        "[limit-exceeded] PDF vector path exceeds {MAX_PATH_BYTES} bytes"
    );
    Ok(())
}

fn rgb_hex(red: f64, green: f64, blue: f64) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (red * 255.0).round() as u8,
        (green * 255.0).round() as u8,
        (blue * 255.0).round() as u8
    )
}

#[allow(clippy::too_many_arguments)]
fn import_xobject(
    parser: &mut PdfParser<'_>,
    resources: Option<&PdfValue>,
    name: &str,
    ctm: Affine,
    page_box: PdfPageBox,
    page: &mut ImportedPage,
    report: &mut FidelityReportBuilder,
    stream_id: &str,
    node_index: &mut usize,
    remaining_nodes: usize,
) -> Result<()> {
    let report_id = format!("{stream_id}:{name}");
    let Some(resources) = resources else {
        report.record(
            &report_id,
            "xobject",
            FidelityStatus::Dropped,
            "PDF XObject has no page resources dictionary",
            Some("provide a page /Resources /XObject entry or convert the object to paths".into()),
        );
        return Ok(());
    };
    let resources = parser.deref(resources)?.clone();
    let Some(xobjects_value) = resources.get(&b"XObject"[..]).cloned() else {
        report.record(
            &report_id,
            "xobject",
            FidelityStatus::Dropped,
            "PDF page resources contain no /XObject dictionary",
            Some("convert the placed object to editable paths before import".into()),
        );
        return Ok(());
    };
    let xobjects = parser.deref(&xobjects_value)?.clone();
    let Some(reference) = xobjects
        .dict()
        .and_then(|dict| dict.get(name.as_bytes()))
        .and_then(PdfValue::reference)
    else {
        report.record(
            &report_id,
            "xobject",
            FidelityStatus::Dropped,
            "PDF XObject resource name does not resolve to an indirect object",
            Some("repair the resource dictionary or convert the object to paths".into()),
        );
        return Ok(());
    };
    let object = parser.resolve(reference)?.clone();
    let Some(dictionary) = object.value.dict() else {
        report.record(
            format!("{} {} R", reference.0, reference.1),
            "xobject",
            FidelityStatus::Dropped,
            "PDF XObject is not a stream dictionary",
            None,
        );
        return Ok(());
    };
    let source_id = format!("{} {} R", reference.0, reference.1);
    if dictionary.get(&b"Subtype"[..]).and_then(PdfValue::name) != Some(b"Image") {
        report.record(
            &source_id,
            "xobject",
            FidelityStatus::Dropped,
            "PDF form and other non-image XObjects are outside the supported importer subset",
            Some("convert the XObject to editable paths before import".into()),
        );
        return Ok(());
    }
    if object.stream.is_none() {
        bail!("[invalid-input] PDF image XObject {source_id} has no stream");
    }
    if dictionary.contains_key(&b"Decode"[..]) {
        report.record(
            &source_id,
            "image_decode",
            FidelityStatus::Dropped,
            "PDF custom Decode arrays are not supported",
            Some("normalize samples before import".into()),
        );
        return Ok(());
    }
    let image_filter = match dictionary.get(&b"Filter"[..]) {
        Some(PdfValue::Name(filter)) if filter == b"FlateDecode" || filter == b"Fl" => {
            Some("flate")
        }
        Some(PdfValue::Name(filter)) if filter == b"DCTDecode" || filter == b"DCT" => Some("jpeg"),
        Some(PdfValue::Array(filters)) => {
            if filters.len() == 1 {
                match filters[0].name() {
                    Some(b"FlateDecode" | b"Fl") => Some("flate"),
                    Some(b"DCTDecode" | b"DCT") => Some("jpeg"),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    };
    let Some(image_filter) = image_filter else {
        report.record(
            &source_id,
            "image_filter",
            FidelityStatus::Dropped,
            "PDF image filter is unsupported; only a single FlateDecode or baseline JPEG DCTDecode filter is imported",
            Some("re-encode the image with FlateDecode or baseline JPEG DCTDecode before import".into()),
        );
        return Ok(());
    };
    let color_space_value = dictionary
        .get(&b"ColorSpace"[..])
        .context("[invalid-input] PDF image XObject has no ColorSpace")?;
    let color_space = parser.deref(color_space_value)?.name().map(<[u8]>::to_vec);
    let channels = match color_space.as_deref() {
        Some(b"DeviceRGB") => 3usize,
        Some(b"DeviceGray") => 1usize,
        Some(b"DeviceCMYK") => 4usize,
        _ => {
            report.record(
                &source_id,
                "image_color_space",
                FidelityStatus::Dropped,
            "PDF image color space is unsupported; only DeviceRGB, DeviceGray, and FlateDecode DeviceCMYK are imported",
            Some("convert the image to DeviceRGB or DeviceGray, or use FlateDecode for DeviceCMYK samples".into()),
            );
            return Ok(());
        }
    };
    if channels == 4 && image_filter != "flate" {
        report.record(
            &source_id,
            "image_color_space",
            FidelityStatus::Dropped,
            "DeviceCMYK PDF images are imported only with FlateDecode; JPEG CMYK decoding is not supported",
            Some("re-encode the image as FlateDecode DeviceCMYK or convert it to DeviceRGB".into()),
        );
        return Ok(());
    }
    let width = image_dimension(dictionary, b"Width", &source_id)?;
    let height = image_dimension(dictionary, b"Height", &source_id)?;
    ensure!(
        width <= MAX_PAGE_DIMENSION as usize && height <= MAX_PAGE_DIMENSION as usize,
        "[limit-exceeded] PDF image dimensions exceed {MAX_PAGE_DIMENSION} pixels per axis"
    );
    let bits = dictionary
        .get(&b"BitsPerComponent"[..])
        .and_then(PdfValue::number)
        .context("[invalid-input] PDF image XObject has no BitsPerComponent")?;
    let has_soft_mask = dictionary
        .get(&b"SMask"[..])
        .is_some_and(|mask| !matches!(mask, PdfValue::Null));
    let (color_key_mask, dropped_color_key) = match dictionary.get(&b"Mask"[..]) {
        None => (None, false),
        Some(_) if has_soft_mask => (None, true),
        Some(mask) => match parse_color_key_mask(parser.deref(mask)?, channels, bits) {
            Some(ranges) if channels == 1 || channels == 3 => (Some(ranges), false),
            _ => (None, true),
        },
    };
    if dropped_color_key {
        report.record(
            &source_id,
            "image_mask",
            FidelityStatus::Dropped,
            "PDF explicit mask is outside the supported 8-bit DeviceGray/DeviceRGB color-key subset; the image was imported without that mask",
            Some("flatten the mask into the image before import".into()),
        );
    } else if color_key_mask.is_some() {
        report.record(
            &source_id,
            "image_mask",
            FidelityStatus::Approximated,
            "An 8-bit DeviceGray/DeviceRGB PDF color-key mask was applied as binary image transparency",
            Some("review the result against the source PDF renderer".into()),
        );
    }
    let predictor = match image_png_predictor(
        dictionary,
        image_filter,
        channels,
        bits as usize,
        width,
        height,
    ) {
        Ok(predictor) => predictor,
        Err(error) if error.to_string().contains("[unsupported-capability]") => {
            report.record(
                &source_id,
                "image_filter_parameters",
                FidelityStatus::Dropped,
                format!("PDF image DecodeParms are unsupported: {error}"),
                Some("use FlateDecode with a PNG predictor and parameters matching the image, or remove DecodeParms".into()),
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if bits != 8.0 && !(channels == 1 && bits == 1.0) {
        report.record(
            &source_id,
            "image_bit_depth",
            FidelityStatus::Dropped,
            "PDF image bit depth is unsupported; this subset accepts 8-bit RGB/gray/CMYK and 1-bit grayscale",
            Some("convert the image to 8-bit DeviceRGB, DeviceGray, or DeviceCMYK samples before import".into()),
        );
        return Ok(());
    }
    let pixels = width
        .checked_mul(height)
        .context("[limit-exceeded] PDF image decoded size overflow")?;
    let decoded_len = if bits == 1.0 {
        width
            .checked_add(7)
            .map(|row| row / 8)
            .and_then(|row| row.checked_mul(height))
            .context("[limit-exceeded] packed PDF image size overflow")?
    } else {
        pixels
            .checked_mul(channels)
            .context("[limit-exceeded] PDF image decoded size overflow")?
    };
    crate::interchange::check_image_bytes(decoded_len as u64, parser.limits)?;
    if bits == 1.0 {
        crate::interchange::check_image_bytes(pixels as u64, parser.limits)?;
    }
    ensure!(
        page.nodes.len() < remaining_nodes,
        "[limit-exceeded] imported PDF node count exceeds configured limit"
    );
    let stream = object.stream.as_deref().unwrap();
    let soft_mask = match dictionary.get(&b"SMask"[..]) {
        None | Some(PdfValue::Null) => None,
        Some(mask_reference) => {
            match decode_soft_mask(parser, mask_reference, width, height, &source_id)? {
                Some(alpha) => {
                    report.record(
                        &source_id,
                        "soft_mask",
                        FidelityStatus::Approximated,
                        "A matching 8-bit FlateDecode DeviceGray soft mask was applied as image alpha",
                        Some("review the result against the source PDF renderer".into()),
                    );
                    Some(alpha)
                }
                None => {
                    report.record(
                        &source_id,
                        "soft_mask",
                        FidelityStatus::Dropped,
                        "PDF soft mask is outside the supported matching 8-bit FlateDecode DeviceGray subset; the image was imported without its mask",
                        Some("flatten the soft mask into the image before import".into()),
                    );
                    None
                }
            }
        }
    };
    let mut png = std::io::Cursor::new(Vec::new());
    if image_filter == "jpeg" {
        ensure!(
            bits == 8.0,
            "[invalid-input] JPEG PDF image samples must use 8 bits per component"
        );
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(stream), image::ImageFormat::Jpeg);
        let mut decode_limits = image::Limits::default();
        decode_limits.max_image_width = Some(width as u32);
        decode_limits.max_image_height = Some(height as u32);
        decode_limits.max_alloc = Some(MAX_TOTAL_DECODED_BYTES as u64);
        reader.limits(decode_limits);
        let decoder = reader
            .into_decoder()
            .context("[invalid-input] malformed PDF JPEG image stream")?;
        let dimensions = decoder.dimensions();
        ensure!(
            dimensions == (width as u32, height as u32),
            "[invalid-input] PDF JPEG dimensions {:?} disagree with image dictionary {}x{}",
            dimensions,
            width,
            height
        );
        let expected_color = if channels == 3 {
            image::ColorType::Rgb8
        } else {
            image::ColorType::L8
        };
        ensure!(
            decoder.color_type() == expected_color,
            "[unsupported-capability] PDF JPEG color components disagree with declared ColorSpace"
        );
        let decoded = image::DynamicImage::from_decoder(decoder)
            .context("[invalid-input] could not decode bounded PDF JPEG image")?;
        decoded.write_to(&mut png, image::ImageFormat::Png)?;
    } else {
        let decoded = parser.decode_image_stream(dictionary, stream, predictor)?;
        ensure!(
            decoded.len() == decoded_len,
            "[invalid-input] PDF image decoded to {} bytes, expected {decoded_len}",
            decoded.len()
        );
        if channels == 3 {
            let image = image::RgbImage::from_raw(width as u32, height as u32, decoded)
                .context("[invalid-input] PDF RGB image samples do not match dimensions")?;
            image::DynamicImage::ImageRgb8(image).write_to(&mut png, image::ImageFormat::Png)?;
        } else if channels == 4 {
            // DeviceCMYK has no embedded profile here; use the conventional
            // subtractive conversion and mark the object as approximated.
            let output_len = pixels
                .checked_mul(3)
                .context("[limit-exceeded] PDF CMYK conversion size overflow")?;
            crate::interchange::check_image_bytes(output_len as u64, parser.limits)?;
            let mut rgb = Vec::with_capacity(output_len);
            for samples in decoded.chunks_exact(4) {
                let (c, m, y, k) = (
                    samples[0] as f64,
                    samples[1] as f64,
                    samples[2] as f64,
                    samples[3] as f64,
                );
                rgb.push(((255.0 - c) * (255.0 - k) / 255.0).round() as u8);
                rgb.push(((255.0 - m) * (255.0 - k) / 255.0).round() as u8);
                rgb.push(((255.0 - y) * (255.0 - k) / 255.0).round() as u8);
            }
            let image = image::RgbImage::from_raw(width as u32, height as u32, rgb)
                .context("[invalid-input] PDF CMYK image samples do not match dimensions")?;
            image::DynamicImage::ImageRgb8(image).write_to(&mut png, image::ImageFormat::Png)?;
        } else if bits == 1.0 {
            // PDF packs each row independently, most-significant bit first. A set
            // bit is sample 1 (white) and a clear bit is sample 0 (black).
            let row_bytes = width.div_ceil(8);
            let mut expanded = Vec::with_capacity(pixels);
            for y in 0..height {
                let row = &decoded[y * row_bytes..(y + 1) * row_bytes];
                for x in 0..width {
                    let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
                    expanded.push(if bit == 1 { 255 } else { 0 });
                }
            }
            let image = image::GrayImage::from_raw(width as u32, height as u32, expanded)
                .context("[invalid-input] packed PDF grayscale samples do not match dimensions")?;
            image::DynamicImage::ImageLuma8(image).write_to(&mut png, image::ImageFormat::Png)?;
        } else {
            let image = image::GrayImage::from_raw(width as u32, height as u32, decoded)
                .context("[invalid-input] PDF grayscale image samples do not match dimensions")?;
            image::DynamicImage::ImageLuma8(image).write_to(&mut png, image::ImageFormat::Png)?;
        }
    }
    let mut png = png.into_inner();
    if color_key_mask.is_some() || soft_mask.is_some() {
        let mut rgba = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .context(
                "[invalid-input] could not reopen bounded PDF image for soft-mask application",
            )?
            .into_rgba8();
        ensure!(
            rgba.width() as usize == width && rgba.height() as usize == height,
            "[invalid-input] PDF image dimensions changed while applying soft mask"
        );
        if let Some(ranges) = color_key_mask {
            for pixel in rgba.pixels_mut() {
                let matches = ranges.iter().enumerate().all(|(channel, (low, high))| {
                    let value = if channels == 1 {
                        pixel.0[0]
                    } else {
                        pixel.0[channel]
                    };
                    (*low..=*high).contains(&value)
                });
                if matches {
                    pixel.0[3] = 0;
                }
            }
        }
        if let Some(alpha) = soft_mask {
            ensure!(
                alpha.len() == rgba.as_raw().len() / 4,
                "[invalid-input] PDF soft-mask sample count disagrees with image dimensions"
            );
            for (pixel, alpha) in rgba.pixels_mut().zip(alpha) {
                pixel.0[3] = alpha;
            }
        }
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba).write_to(&mut encoded, image::ImageFormat::Png)?;
        png = encoded.into_inner();
    }
    crate::interchange::check_image_bytes(png.len() as u64, parser.limits)?;
    let info = crate::image::decode_source(&png)
        .context("[invalid-input] encoded PDF image failed Pentool image validation")?;
    if !parser.image_assets.contains_key(&info.digest) {
        let next_total = parser
            .image_asset_bytes
            .checked_add(png.len())
            .context("[limit-exceeded] embedded PDF image byte count overflow")?;
        ensure!(
            next_total <= MAX_TOTAL_DECODED_BYTES,
            "[limit-exceeded] embedded PDF image assets exceed {MAX_TOTAL_DECODED_BYTES} bytes"
        );
        parser.image_asset_bytes = next_total;
        parser.image_assets.insert(
            info.digest.clone(),
            json!({
                "media_type":info.media_type,
                "byte_length":info.byte_length,
                "pixel_width":info.pixel_width,
                "pixel_height":info.pixel_height,
                "color_space":info.color_space,
                "orientation":info.orientation,
                "storage":crate::image::embedded_storage(&png)
            }),
        );
    }
    let top_left = ctm.transform(0.0, 1.0, page_box);
    let top_right = ctm.transform(1.0, 1.0, page_box);
    let bottom_left = ctm.transform(0.0, 0.0, page_box);
    *node_index += 1;
    let id = format!("pdf-page-{:03}-image-{:06}", page.index, *node_index);
    page.nodes.push(json!({
        "kind":"image", "id":id, "asset":info.digest,
        "x":0, "y":0, "width":1, "height":1,
        "fit":"fill", "position":[0.5,0.5], "crop":[0,0,1,1],
        "opacity":1, "blend_mode":"normal",
        "transform":[
            top_right.0 - top_left.0,
            top_right.1 - top_left.1,
            bottom_left.0 - top_left.0,
            bottom_left.1 - top_left.1,
            top_left.0,
            top_left.1
        ],
        "operations":[], "name":name
    }));
    report.record(
        &source_id,
        "image_xobject",
        FidelityStatus::Approximated,
        if channels == 4 {
            "FlateDecode DeviceCMYK image samples were converted to sRGB with the simple device formula; color profiles and output intents were not applied"
        } else if image_filter == "jpeg" {
            "Baseline JPEG DeviceRGB or DeviceGray samples were embedded as sRGB; PDF device calibration and output intent were not applied"
        } else if predictor.is_some() {
            "FlateDecode image samples were reconstructed with a PNG predictor and embedded as sRGB; PDF device calibration and output intent were not applied"
        } else {
            "8-bit DeviceRGB or DeviceGray samples were embedded as sRGB; PDF device calibration and output intent were not applied"
        },
        Some("review image colors against the source PDF renderer".into()),
    );
    Ok(())
}

/// Parse the deliberately small PDF color-key mask subset. The component
/// ranges apply to decoded source samples, so only 8-bit DeviceGray/RGB are
/// accepted; CMYK conversion and packed 1-bit samples need distinct handling.
fn parse_color_key_mask(value: &PdfValue, channels: usize, bits: f64) -> Option<Vec<(u8, u8)>> {
    if bits != 8.0 || !matches!(channels, 1 | 3) {
        return None;
    }
    let values = value.array()?;
    if values.len() != channels * 2 {
        return None;
    }
    values
        .chunks_exact(2)
        .map(|pair| {
            let low = pair[0].number()?;
            let high = pair[1].number()?;
            if !low.is_finite()
                || !high.is_finite()
                || low.fract() != 0.0
                || high.fract() != 0.0
                || !(0.0..=255.0).contains(&low)
                || !(0.0..=255.0).contains(&high)
                || low > high
            {
                return None;
            }
            Some((low as u8, high as u8))
        })
        .collect()
}

/// Decode the deliberately small soft-mask subset used by PDF image XObjects.
/// Unsupported mask features return `None` so the caller can retain the base
/// image and record that transparency was omitted.
fn decode_soft_mask(
    parser: &mut PdfParser<'_>,
    reference: &PdfValue,
    width: usize,
    height: usize,
    source_id: &str,
) -> Result<Option<Vec<u8>>> {
    let Some(reference) = reference.reference() else {
        return Ok(None);
    };
    let mask = parser.resolve(reference)?.clone();
    let Some(dictionary) = mask.value.dict() else {
        return Ok(None);
    };
    if dictionary.get(&b"Subtype"[..]).and_then(PdfValue::name) != Some(b"Image")
        || dictionary.get(&b"ColorSpace"[..]).and_then(PdfValue::name) != Some(b"DeviceGray")
        || dictionary
            .get(&b"BitsPerComponent"[..])
            .and_then(PdfValue::number)
            != Some(8.0)
        || dictionary.contains_key(&b"SMask"[..])
        || dictionary.contains_key(&b"Mask"[..])
        || dictionary.contains_key(&b"Decode"[..])
        || dictionary.contains_key(&b"Matte"[..])
    {
        return Ok(None);
    }
    let mask_width = image_dimension(dictionary, b"Width", source_id)?;
    let mask_height = image_dimension(dictionary, b"Height", source_id)?;
    if mask_width != width || mask_height != height {
        return Ok(None);
    }
    let filter = match dictionary.get(&b"Filter"[..]) {
        Some(PdfValue::Name(filter)) if filter == b"FlateDecode" || filter == b"Fl" => "flate",
        Some(PdfValue::Array(filters))
            if filters.len() == 1
                && filters[0]
                    .name()
                    .is_some_and(|filter| filter == b"FlateDecode" || filter == b"Fl") =>
        {
            "flate"
        }
        _ => return Ok(None),
    };
    let pixels = width
        .checked_mul(height)
        .context("[limit-exceeded] PDF soft-mask size overflow")?;
    crate::interchange::check_image_bytes(pixels as u64, parser.limits)?;
    let raw = mask
        .stream
        .as_deref()
        .context("[invalid-input] PDF soft-mask image has no stream")?;
    let predictor = match image_png_predictor(dictionary, filter, 1, 8, width, height) {
        Ok(predictor) => predictor,
        Err(error) if error.to_string().contains("[unsupported-capability]") => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let decoded = parser.decode_image_stream(dictionary, raw, predictor)?;
    ensure!(
        decoded.len() == pixels,
        "[invalid-input] PDF soft mask decoded to {} bytes, expected {pixels}",
        decoded.len()
    );
    Ok(Some(decoded))
}

fn image_dimension(
    dictionary: &BTreeMap<Vec<u8>, PdfValue>,
    key: &[u8],
    source_id: &str,
) -> Result<usize> {
    let value = dictionary
        .get(key)
        .and_then(PdfValue::number)
        .context("[invalid-input] PDF image XObject has a missing or invalid dimension")?;
    ensure!(
        value.is_finite() && value.fract() == 0.0 && value >= 1.0 && value <= usize::MAX as f64,
        "[limit-exceeded] PDF image XObject {source_id} has an invalid dimension"
    );
    Ok(value as usize)
}

#[derive(Clone, Copy)]
struct PngPredictor {
    fixed_filter: Option<u8>,
    row_bytes: usize,
    bytes_per_pixel: usize,
    rows: usize,
}

fn undo_png_predictor(data: Vec<u8>, predictor: PngPredictor) -> Result<Vec<u8>> {
    let PngPredictor {
        fixed_filter,
        row_bytes,
        bytes_per_pixel,
        rows,
    } = predictor;
    let row_stride = row_bytes
        .checked_add(usize::from(fixed_filter.is_none()))
        .context("[limit-exceeded] PDF predictor row size overflow")?;
    let expected = row_stride
        .checked_mul(rows)
        .context("[limit-exceeded] PDF predictor decoded size overflow")?;
    ensure!(
        data.len() == expected,
        "[invalid-input] PDF image predictor stream has {} bytes, expected {expected}",
        data.len()
    );
    let output_len = row_bytes
        .checked_mul(rows)
        .context("[limit-exceeded] PDF predictor output size overflow")?;
    let mut output = vec![0; output_len];
    for row_index in 0..rows {
        let input_start = row_index * row_stride;
        let (filter, filtered_start) = match fixed_filter {
            Some(filter) => (filter, input_start),
            None => (data[input_start], input_start + 1),
        };
        ensure!(
            filter <= 4,
            "[invalid-input] PDF PNG predictor has unsupported row filter {filter}"
        );
        let output_start = row_index * row_bytes;
        for column in 0..row_bytes {
            let raw = data[filtered_start + column];
            let left = if column >= bytes_per_pixel {
                output[output_start + column - bytes_per_pixel]
            } else {
                0
            };
            let up = if row_index > 0 {
                output[output_start + column - row_bytes]
            } else {
                0
            };
            let upper_left = if row_index > 0 && column >= bytes_per_pixel {
                output[output_start + column - row_bytes - bytes_per_pixel]
            } else {
                0
            };
            let prediction = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth_predictor(left, up, upper_left),
                _ => unreachable!("filter tag was checked above"),
            };
            output[output_start + column] = raw.wrapping_add(prediction);
        }
    }
    Ok(output)
}

fn paeth_predictor(left: u8, up: u8, upper_left: u8) -> u8 {
    let (left, up, upper_left) = (i32::from(left), i32::from(up), i32::from(upper_left));
    let estimate = left + up - upper_left;
    let distances = [
        (estimate - left).abs(),
        (estimate - up).abs(),
        (estimate - upper_left).abs(),
    ];
    if distances[0] <= distances[1] && distances[0] <= distances[2] {
        left as u8
    } else if distances[1] <= distances[2] {
        up as u8
    } else {
        upper_left as u8
    }
}

fn image_png_predictor(
    dictionary: &BTreeMap<Vec<u8>, PdfValue>,
    image_filter: &str,
    channels: usize,
    bits: usize,
    width: usize,
    height: usize,
) -> Result<Option<PngPredictor>> {
    let Some(parameters) = dictionary.get(&b"DecodeParms"[..]) else {
        return Ok(None);
    };
    if matches!(parameters, PdfValue::Null) {
        return Ok(None);
    }
    ensure!(
        image_filter == "flate",
        "[unsupported-capability] PDF image DecodeParms are supported only with FlateDecode"
    );
    let parameters = match parameters {
        PdfValue::Dict(_) => parameters,
        PdfValue::Array(values) if values.len() == 1 => &values[0],
        _ => bail!("[invalid-input] PDF image DecodeParms must be a dictionary"),
    };
    if matches!(parameters, PdfValue::Null) {
        return Ok(None);
    }
    let parameters = parameters
        .dict()
        .context("[invalid-input] PDF image DecodeParms is not a dictionary")?;
    ensure!(
        parameters.keys().all(|key| {
            matches!(
                key.as_slice(),
                b"Predictor" | b"Colors" | b"BitsPerComponent" | b"Columns"
            )
        }),
        "[unsupported-capability] PDF image DecodeParms contains unsupported parameters"
    );
    let integer = |key: &[u8], default: usize| -> Result<usize> {
        match parameters.get(key) {
            None => Ok(default),
            Some(PdfValue::Number(value))
                if value.is_finite()
                    && value.fract() == 0.0
                    && *value >= 1.0
                    && *value <= 65536.0 =>
            {
                Ok(*value as usize)
            }
            _ => bail!(
                "[invalid-input] PDF image DecodeParms /{} must be a positive integer",
                String::from_utf8_lossy(key)
            ),
        }
    };
    let predictor = integer(b"Predictor", 1)?;
    if predictor == 1 {
        return Ok(None);
    }
    if !(10..=15).contains(&predictor) {
        bail!(
            "[unsupported-capability] PDF image predictor {predictor} is outside PNG predictor subset 10..=15"
        );
    }
    let colors = integer(b"Colors", 1)?;
    let parameter_bits = integer(b"BitsPerComponent", 8)?;
    let columns = integer(b"Columns", 1)?;
    ensure!(
        bits == 8 && colors == channels && parameter_bits == bits && columns == width,
        "[invalid-input] PDF image PNG predictor parameters disagree with image dimensions or samples"
    );
    let row_bytes = width
        .checked_mul(channels)
        .context("[limit-exceeded] PDF PNG predictor row size overflow")?;
    Ok(Some(PngPredictor {
        fixed_filter: (predictor < 15).then_some((predictor - 10) as u8),
        row_bytes,
        bytes_per_pixel: channels,
        rows: height,
    }))
}

fn supported_pdf_text(bytes: &[u8]) -> Option<String> {
    if !bytes
        .iter()
        .all(|byte| matches!(*byte, b'\n' | b'\r' | b'\t' | 0x20..=0x7e))
    {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.replace("\r\n", "\n").replace('\r', "\n"))
}

fn sfnt_fs_type(data: &[u8]) -> Option<u16> {
    let table_count = u16::from_be_bytes(data.get(4..6)?.try_into().ok()?) as usize;
    let directory_end = 12usize.checked_add(table_count.checked_mul(16)?)?;
    if data.len() < directory_end {
        return None;
    }
    for record in data[12..directory_end].chunks_exact(16) {
        if &record[..4] == b"OS/2" {
            let offset = u32::from_be_bytes(record[8..12].try_into().ok()?) as usize;
            let length = u32::from_be_bytes(record[12..16].try_into().ok()?) as usize;
            if length < 10 {
                return None;
            }
            let fs_type = data.get(offset.checked_add(8)?..offset.checked_add(10)?)?;
            return Some(u16::from_be_bytes(fs_type.try_into().ok()?));
        }
    }
    None
}

fn parse_simple_tounicode(data: &[u8]) -> Result<Option<BTreeMap<u8, String>>> {
    ensure!(
        data.len() <= 256 * 1024,
        "[limit-exceeded] PDF ToUnicode CMap exceeds 256 KiB"
    );
    let mut tokens = Vec::<String>::new();
    let mut pos = 0usize;
    while pos < data.len() {
        skip_ws(data, &mut pos);
        if pos >= data.len() {
            break;
        }
        if data[pos] == b'<' && data.get(pos + 1) != Some(&b'<') {
            let start = pos;
            pos += 1;
            while data.get(pos).is_some_and(|b| *b != b'>') {
                pos += 1;
            }
            ensure!(
                pos < data.len(),
                "[invalid-input] unterminated PDF CMap hex token"
            );
            let value = std::str::from_utf8(&data[start + 1..pos])?;
            ensure!(
                value.len() % 2 == 0 && value.bytes().all(|b| b.is_ascii_hexdigit()),
                "[invalid-input] invalid PDF CMap hex token"
            );
            tokens.push(format!("<{}>", value));
            pos += 1;
        } else {
            let start = pos;
            while data
                .get(pos)
                .is_some_and(|b| !is_ws(*b) && !matches!(*b, b'<' | b'>' | b'%' | b'[' | b']'))
            {
                pos += 1;
            }
            if data.get(pos) == Some(&b'%') {
                while data.get(pos).is_some_and(|b| !matches!(*b, b'\r' | b'\n')) {
                    pos += 1;
                }
            } else if pos > start {
                tokens.push(std::str::from_utf8(&data[start..pos])?.to_owned());
            } else {
                pos += 1;
            }
        }
        ensure!(
            tokens.len() <= 20_000,
            "[limit-exceeded] PDF ToUnicode CMap token count exceeds 20000"
        );
    }
    if tokens.iter().any(|token| token == "/UseCMap") {
        return Ok(None);
    }
    let mut map = BTreeMap::new();
    for (index, token) in tokens.iter().enumerate() {
        if token != "beginbfchar" {
            continue;
        }
        let Some(count) = index
            .checked_sub(1)
            .and_then(|i| tokens[i].parse::<usize>().ok())
        else {
            return Ok(None);
        };
        if count > 4096 {
            bail!("[limit-exceeded] PDF ToUnicode bfchar count exceeds 4096");
        }
        for pair in 0..count {
            let src = tokens
                .get(index + 1 + pair * 2)
                .and_then(|token| parse_cmap_hex(token));
            let dst = tokens
                .get(index + 2 + pair * 2)
                .and_then(|token| parse_cmap_hex(token));
            let (Some(src), Some(dst)) = (src, dst) else {
                return Ok(None);
            };
            if src.len() != 1 || dst.is_empty() || dst.len() % 2 != 0 || dst.len() > 64 {
                return Ok(None);
            }
            let mut units = dst
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            if units.first() == Some(&0xfeff) {
                units.remove(0);
            }
            let Ok(text) = String::from_utf16(&units) else {
                return Ok(None);
            };
            if map.insert(src[0], text).is_some() {
                return Ok(None);
            }
        }
        if tokens
            .get(index + count * 2 + 1)
            .is_none_or(|token| token != "endbfchar")
        {
            return Ok(None);
        }
    }
    for (index, token) in tokens.iter().enumerate() {
        if token != "beginbfrange" {
            continue;
        }
        let Some(count) = index
            .checked_sub(1)
            .and_then(|i| tokens[i].parse::<usize>().ok())
        else {
            return Ok(None);
        };
        if count > 256 {
            bail!("[limit-exceeded] PDF ToUnicode bfrange count exceeds 256");
        }
        for range in 0..count {
            let low = tokens
                .get(index + 1 + range * 3)
                .and_then(|token| parse_cmap_hex(token));
            let high = tokens
                .get(index + 2 + range * 3)
                .and_then(|token| parse_cmap_hex(token));
            let destination = tokens
                .get(index + 3 + range * 3)
                .and_then(|token| parse_cmap_hex(token));
            let (Some(low), Some(high), Some(destination)) = (low, high, destination) else {
                return Ok(None);
            };
            if low.len() != 1 || high.len() != 1 || destination.len() != 2 || low[0] > high[0] {
                return Ok(None);
            }
            let span = usize::from(high[0] - low[0]) + 1;
            if span > 256 || map.len().saturating_add(span) > 256 {
                bail!("[limit-exceeded] PDF ToUnicode bfrange expands beyond 256 one-byte codes");
            }
            let base = u16::from_be_bytes([destination[0], destination[1]]);
            for offset in 0..span {
                let Some(scalar) = u32::from(base).checked_add(offset as u32) else {
                    return Ok(None);
                };
                let Some(character) = char::from_u32(scalar) else {
                    return Ok(None);
                };
                let code = low[0] + offset as u8;
                if map.insert(code, character.to_string()).is_some() {
                    return Ok(None);
                }
            }
        }
        if tokens
            .get(index + count * 3 + 1)
            .is_none_or(|token| token != "endbfrange")
        {
            return Ok(None);
        }
    }
    if map.is_empty() {
        return Ok(None);
    }
    Ok(Some(map))
}

fn parse_cmap_hex(token: &str) -> Option<Vec<u8>> {
    let hex = token.strip_prefix('<')?.strip_suffix('>')?;
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn decode_pdf_font_text(bytes: &[u8], font: &PdfTextFont, font_size: f64) -> Option<(String, f64)> {
    let mut text = String::new();
    let mut advance = 0.0;
    for code in bytes {
        text.push_str(font.to_unicode.get(code)?);
        advance += font.widths.get(code)? * font_size / 1000.0;
    }
    Some((text, advance))
}

fn content_literal_string(data: &[u8], start: usize) -> Result<(Vec<u8>, usize)> {
    let mut pos = start + 1;
    let mut depth = 1usize;
    let mut output = Vec::new();
    while let Some(byte) = data.get(pos).copied() {
        pos += 1;
        match byte {
            b'(' => {
                depth += 1;
                output.push(byte);
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((output, pos));
                }
                output.push(byte);
            }
            b'\\' => {
                let escaped = *data
                    .get(pos)
                    .context("[invalid-input] trailing escape in PDF content string")?;
                pos += 1;
                match escaped {
                    b'n' => output.push(b'\n'),
                    b'r' => output.push(b'\r'),
                    b't' => output.push(b'\t'),
                    b'b' => output.push(8),
                    b'f' => output.push(12),
                    b'(' | b')' | b'\\' => output.push(escaped),
                    b'\r' => {
                        if data.get(pos) == Some(&b'\n') {
                            pos += 1;
                        }
                    }
                    b'\n' => {}
                    b'0'..=b'7' => {
                        let mut value = (escaped - b'0') as u16;
                        for _ in 0..2 {
                            if data
                                .get(pos)
                                .is_some_and(|next| (b'0'..=b'7').contains(next))
                            {
                                value = value * 8 + (data[pos] - b'0') as u16;
                                pos += 1;
                            } else {
                                break;
                            }
                        }
                        output.push(value as u8);
                    }
                    _ => output.push(escaped),
                }
            }
            _ => output.push(byte),
        }
        ensure!(
            output.len() <= MAX_CONTENT_STREAM_BYTES,
            "[limit-exceeded] decoded PDF text string exceeds stream limit"
        );
    }
    bail!("[invalid-input] unterminated PDF content literal string")
}

fn content_hex_string(data: &[u8], start: usize) -> Result<(Vec<u8>, usize)> {
    let mut pos = start + 1;
    let mut high = None;
    let mut output = Vec::new();
    while let Some(byte) = data.get(pos).copied() {
        pos += 1;
        if byte == b'>' {
            if let Some(nibble) = high {
                output.push(nibble << 4);
            }
            return Ok((output, pos));
        }
        if is_ws(byte) {
            continue;
        }
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => bail!("[invalid-input] invalid hex digit in PDF text string"),
        };
        if let Some(first) = high.take() {
            output.push((first << 4) | nibble);
        } else {
            high = Some(nibble);
        }
        ensure!(
            output.len() <= MAX_CONTENT_STREAM_BYTES,
            "[limit-exceeded] decoded PDF text string exceeds stream limit"
        );
    }
    bail!("[invalid-input] unterminated PDF content hex string")
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    loop {
        while bytes.get(*pos).is_some_and(|byte| is_ws(*byte)) {
            *pos += 1;
        }
        if bytes.get(*pos) == Some(&b'%') {
            while bytes
                .get(*pos)
                .is_some_and(|byte| !matches!(byte, b'\r' | b'\n'))
            {
                *pos += 1;
            }
        } else {
            break;
        }
    }
}

fn is_ws(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn is_delimiter(byte: u8) -> bool {
    is_ws(byte)
        || matches!(
            byte,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

fn take_word<'a>(bytes: &'a [u8], pos: &mut usize) -> Result<&'a [u8]> {
    skip_ws(bytes, pos);
    let start = *pos;
    while bytes.get(*pos).is_some_and(|byte| !is_delimiter(*byte)) {
        *pos += 1;
    }
    ensure!(
        *pos > start,
        "[invalid-input] expected PDF token at byte {start}"
    );
    Ok(&bytes[start..*pos])
}

fn parse_usize_token(bytes: &[u8], pos: &mut usize) -> Result<usize> {
    let token = take_word(bytes, pos)?;
    std::str::from_utf8(token)?
        .parse()
        .context("[invalid-input] expected a nonnegative integer")
}
fn parse_u32_token(bytes: &[u8], pos: &mut usize) -> Result<u32> {
    let token = take_word(bytes, pos)?;
    std::str::from_utf8(token)?
        .parse()
        .context("[invalid-input] expected a u32 integer")
}
fn parse_u16_token(bytes: &[u8], pos: &mut usize) -> Result<u16> {
    let token = take_word(bytes, pos)?;
    std::str::from_utf8(token)?
        .parse()
        .context("[invalid-input] expected a u16 integer")
}

struct ValueParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    items: usize,
}

impl<'a> ValueParser<'a> {
    fn new(bytes: &'a [u8], pos: usize) -> Self {
        Self {
            bytes,
            pos,
            items: 0,
        }
    }
    fn skip_ws(&mut self) {
        skip_ws(self.bytes, &mut self.pos);
    }
    fn u32(&mut self) -> Result<u32> {
        parse_u32_token(self.bytes, &mut self.pos)
    }
    fn u16(&mut self) -> Result<u16> {
        parse_u16_token(self.bytes, &mut self.pos)
    }
    fn consume_word(&mut self, word: &[u8]) -> bool {
        self.skip_ws();
        if self.bytes.get(self.pos..self.pos + word.len()) == Some(word)
            && self
                .bytes
                .get(self.pos + word.len())
                .is_none_or(|byte| is_delimiter(*byte))
        {
            self.pos += word.len();
            true
        } else {
            false
        }
    }
    fn expect_word(&mut self, word: &[u8]) -> Result<()> {
        ensure!(
            self.consume_word(word),
            "[invalid-input] expected PDF token {}",
            String::from_utf8_lossy(word)
        );
        Ok(())
    }
    fn stream_eol(&mut self) -> Result<()> {
        ensure!(
            self.bytes
                .get(self.pos)
                .is_some_and(|byte| *byte == b'\r' || *byte == b'\n'),
            "[invalid-input] stream keyword must be followed by an end-of-line"
        );
        if self.bytes.get(self.pos) == Some(&b'\r') {
            self.pos += 1;
        }
        if self.bytes.get(self.pos) == Some(&b'\n') {
            self.pos += 1;
        }
        Ok(())
    }
    fn value(&mut self, depth: usize) -> Result<PdfValue> {
        ensure!(
            depth <= MAX_OBJECT_DEPTH,
            "[limit-exceeded] PDF object nesting exceeds {MAX_OBJECT_DEPTH}"
        );
        self.items += 1;
        ensure!(
            self.items <= MAX_OBJECTS * 8,
            "[limit-exceeded] PDF object token count exceeds limit"
        );
        self.skip_ws();
        let byte = *self
            .bytes
            .get(self.pos)
            .context("[invalid-input] unexpected EOF in PDF object")?;
        match byte {
            b'<' if self.bytes.get(self.pos + 1) == Some(&b'<') => self.dictionary(depth + 1),
            b'[' => self.array(depth + 1),
            b'/' => self.name(),
            b'(' => {
                self.literal_string()?;
                Ok(PdfValue::String)
            }
            b'<' => {
                self.hex_string()?;
                Ok(PdfValue::String)
            }
            _ => self.word_or_number(),
        }
    }
    fn dictionary(&mut self, depth: usize) -> Result<PdfValue> {
        self.pos += 2;
        let mut values = BTreeMap::new();
        loop {
            self.skip_ws();
            if self.bytes.get(self.pos..self.pos + 2) == Some(b">>") {
                self.pos += 2;
                break;
            }
            let key = match self.value(depth)? {
                PdfValue::Name(key) => key,
                _ => bail!("[invalid-input] PDF dictionary key is not a name"),
            };
            let value = self.value(depth)?;
            ensure!(
                values.insert(key, value).is_none(),
                "[invalid-input] duplicate PDF dictionary key"
            );
            ensure!(
                values.len() <= MAX_OBJECTS,
                "[limit-exceeded] PDF dictionary has too many entries"
            );
        }
        Ok(PdfValue::Dict(values))
    }
    fn array(&mut self, depth: usize) -> Result<PdfValue> {
        self.pos += 1;
        let mut values = Vec::new();
        loop {
            self.skip_ws();
            if self.bytes.get(self.pos) == Some(&b']') {
                self.pos += 1;
                break;
            }
            values.push(self.value(depth)?);
            ensure!(
                values.len() <= MAX_OBJECTS,
                "[limit-exceeded] PDF array has too many values"
            );
        }
        Ok(PdfValue::Array(values))
    }
    fn name(&mut self) -> Result<PdfValue> {
        self.pos += 1;
        let mut name = Vec::new();
        while let Some(byte) = self.bytes.get(self.pos).copied() {
            if is_delimiter(byte) {
                break;
            }
            if byte == b'#' {
                ensure!(
                    self.pos + 2 < self.bytes.len(),
                    "[invalid-input] truncated escaped PDF name"
                );
                let hex = std::str::from_utf8(&self.bytes[self.pos + 1..self.pos + 3])?;
                name.push(
                    u8::from_str_radix(hex, 16)
                        .context("[invalid-input] invalid escaped PDF name")?,
                );
                self.pos += 3;
            } else {
                name.push(byte);
                self.pos += 1;
            }
            ensure!(
                name.len() <= 256,
                "[limit-exceeded] PDF name exceeds 256 bytes"
            );
        }
        Ok(PdfValue::Name(name))
    }
    fn literal_string(&mut self) -> Result<()> {
        self.pos += 1;
        let mut nesting = 1usize;
        let mut length = 0usize;
        while let Some(byte) = self.bytes.get(self.pos).copied() {
            self.pos += 1;
            length += 1;
            ensure!(
                length <= 1_000_000,
                "[limit-exceeded] PDF literal string exceeds limit"
            );
            match byte {
                b'\\' => {
                    ensure!(
                        self.pos < self.bytes.len(),
                        "[invalid-input] truncated PDF string escape"
                    );
                    self.pos += 1;
                }
                b'(' => {
                    nesting += 1;
                    ensure!(
                        nesting <= 64,
                        "[limit-exceeded] PDF string nesting exceeds limit"
                    );
                }
                b')' => {
                    nesting -= 1;
                    if nesting == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        bail!("[invalid-input] unterminated PDF literal string")
    }
    fn hex_string(&mut self) -> Result<()> {
        self.pos += 1;
        while let Some(byte) = self.bytes.get(self.pos).copied() {
            self.pos += 1;
            if byte == b'>' {
                return Ok(());
            }
            ensure!(
                is_ws(byte) || byte.is_ascii_hexdigit(),
                "[invalid-input] invalid PDF hex string"
            );
        }
        bail!("[invalid-input] unterminated PDF hex string")
    }
    fn word_or_number(&mut self) -> Result<PdfValue> {
        let first = take_word(self.bytes, &mut self.pos)?;
        if first == b"null" {
            return Ok(PdfValue::Null);
        }
        if first == b"true" || first == b"false" {
            return Ok(PdfValue::Bool);
        }
        if let Ok(text) = std::str::from_utf8(first) {
            if let Ok(number) = text.parse::<f64>() {
                ensure!(number.is_finite(), "[invalid-input] non-finite PDF number");
                if number.fract() == 0.0 && number >= 0.0 && number <= u32::MAX as f64 {
                    let after_first = self.pos;
                    if let Ok(generation) = parse_u16_token(self.bytes, &mut self.pos) {
                        if self.consume_word(b"R") {
                            return Ok(PdfValue::Ref(number as u32, generation));
                        }
                    }
                    // Reference lookahead must not consume a normal operator.
                    self.pos = after_first;
                }
                return Ok(PdfValue::Number(number));
            }
        }
        bail!(
            "[invalid-input] unexpected PDF keyword {}",
            String::from_utf8_lossy(first)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use std::fs;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn pdf(content: &[u8], trailer_extra: &str) -> Vec<u8> {
        let stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        classic_pdf(
            vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R >>".to_vec(),
                stream,
            ],
            trailer_extra,
        )
    }

    #[test]
    fn imports_pdf_miter_limit_and_uses_pdf_default_when_unset() {
        let custom = import(
            &pdf(b"2.5 M 0 0 m 20 20 l S", ""),
            "miter-limit.pdf",
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(
            custom.document["pages"][0]["layers"][0]["nodes"][0]["style"]["stroke_miterlimit"]
                ["fallback"],
            2.5
        );
        let default = import(
            &pdf(b"0 0 m 20 20 l S", ""),
            "default-miter-limit.pdf",
            ImportLimits::default(),
        )
        .unwrap();
        assert_eq!(
            default.document["pages"][0]["layers"][0]["nodes"][0]["style"]["stroke_miterlimit"]
                ["fallback"],
            10.0
        );
    }

    #[test]
    fn rejects_non_positive_or_unbounded_pdf_miter_limits() {
        for input in [b"0 M 0 0 m 1 1 l S".as_slice(), b"1000001 M 0 0 m 1 1 l S"] {
            let error = match import(
                &pdf(input, ""),
                "invalid-miter-limit.pdf",
                ImportLimits::default(),
            ) {
                Ok(_) => panic!("invalid PDF miter limit unexpectedly imported"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("PDF miter limit"));
        }
    }

    fn pdf_with_image(content: &[u8], image_dictionary: &str, image_data: &[u8]) -> Vec<u8> {
        pdf_with_image_dimensions(content, image_dictionary, image_data, 2, 1)
    }

    fn pdf_with_image_dimensions(
        content: &[u8],
        image_dictionary: &str,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let content_stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let image_stream = [
            format!(
                "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /Length {} {image_dictionary} >>\nstream\n",
                image_data.len()
            )
            .into_bytes(),
            image_data.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        classic_pdf(
            vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>".to_vec(),
                content_stream,
                image_stream,
            ],
            "",
        )
    }

    fn pdf_with_embedded_font(content: &[u8], font_data: &[u8]) -> Vec<u8> {
        let content_stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let font_stream = [
            format!(
                "<< /Length {} /Length1 {} >>\nstream\n",
                font_data.len(),
                font_data.len()
            )
            .into_bytes(),
            font_data.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let cmap = b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange <00> <FF> endcodespacerange\n1 beginbfrange <41> <43> <0041> endbfrange\nendcmap\nend\nend";
        let cmap_stream = [
            format!("<< /Length {} >>\nstream\n", cmap.len()).into_bytes(),
            cmap.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        classic_pdf(
            vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
                content_stream,
                b"<< /Type /Font /Subtype /TrueType /BaseFont /AtkinsonHyperlegible-Regular /FirstChar 65 /Widths [600 600 600] /FontDescriptor 6 0 R /ToUnicode 8 0 R >>".to_vec(),
                b"<< /Type /FontDescriptor /FontName /AtkinsonHyperlegible-Regular /FontFile2 7 0 R >>".to_vec(),
                font_stream,
                cmap_stream,
            ],
            "",
        )
    }

    fn with_sfnt_fs_type(mut data: Vec<u8>, fs_type: u16) -> Vec<u8> {
        let count = u16::from_be_bytes(data[4..6].try_into().unwrap()) as usize;
        for record in data[12..12 + count * 16].chunks_exact(16) {
            if &record[..4] == b"OS/2" {
                let offset = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
                data[offset + 8..offset + 10].copy_from_slice(&fs_type.to_be_bytes());
                return data;
            }
        }
        panic!("test font has no OS/2 table")
    }

    fn flate(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn classic_pdf(objects: Vec<Vec<u8>>, trailer_extra: &str) -> Vec<u8> {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = vec![0usize];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets.into_iter().skip(1) {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer << /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        bytes
    }

    fn xref_stream_pdf(use_flate: bool) -> Vec<u8> {
        let content = b"0 0.5 1 rg 10 10 30 20 re f";
        let content_stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R >>".to_vec(),
            content_stream,
        ];
        let mut bytes = b"%PDF-1.5\n".to_vec();
        let mut offsets = vec![0usize];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref_offset = bytes.len();
        offsets.push(xref_offset);
        let mut rows = Vec::new();
        rows.extend_from_slice(&[0, 0, 0, 0, 0, 0xff, 0xff]);
        for offset in offsets.iter().skip(1) {
            rows.push(1);
            rows.extend_from_slice(&(*offset as u32).to_be_bytes());
            rows.extend_from_slice(&0u16.to_be_bytes());
        }
        let stream = if use_flate { flate(&rows) } else { rows };
        let filter = if use_flate {
            " /Filter /FlateDecode"
        } else {
            ""
        };
        bytes.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XRef /Size 6 /Root 1 0 R /W [1 4 2] /Length {}{filter} >>\nstream\n",
                stream.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&stream);
        bytes.extend_from_slice(b"\nendstream\nendobj\nstartxref\n");
        bytes.extend_from_slice(xref_offset.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        bytes
    }

    fn object_stream_pdf() -> Vec<u8> {
        object_stream_pdf_with_count(1)
    }

    fn object_stream_pdf_with_count(count: usize) -> Vec<u8> {
        fn add_object(bytes: &mut Vec<u8>, offsets: &mut [usize], id: usize, object: &[u8]) {
            offsets[id] = bytes.len();
            bytes.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
            bytes.extend_from_slice(object);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = vec![0usize; 7];
        add_object(
            &mut bytes,
            &mut offsets,
            1,
            b"<< /Type /Catalog /Pages 2 0 R >>",
        );
        add_object(
            &mut bytes,
            &mut offsets,
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R >>",
        );
        let content = b"10 10 m 30 30 l S";
        let content_object = [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat();
        add_object(&mut bytes, &mut offsets, 4, &content_object);
        let compressed_value = b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>";
        let object_stream_data = [b"2 0 ".as_slice(), compressed_value].concat();
        let object_stream = [
            format!(
                "<< /Type /ObjStm /N {count} /First 4 /Length {} >>\nstream\n",
                object_stream_data.len()
            )
            .as_bytes(),
            &object_stream_data,
            b"\nendstream",
        ]
        .concat();
        add_object(&mut bytes, &mut offsets, 5, &object_stream);
        let xref_offset = bytes.len();
        offsets[6] = xref_offset;
        let mut rows = vec![0, 0, 0, 0, 0, 0xff, 0xff];
        for (id, offset) in offsets.iter().enumerate().skip(1) {
            if id == 2 {
                rows.extend_from_slice(&[2, 0, 0, 0, 5, 0, 0]);
            } else {
                rows.push(1);
                rows.extend_from_slice(&(*offset as u32).to_be_bytes());
                rows.extend_from_slice(&0u16.to_be_bytes());
            }
        }
        add_object(
            &mut bytes,
            &mut offsets,
            6,
            &[
                format!(
                    "<< /Type /XRef /Size 7 /Root 1 0 R /W [1 4 2] /Length {} >>\nstream\n",
                    rows.len()
                )
                .as_bytes(),
                &rows,
                b"\nendstream",
            ]
            .concat(),
        );
        bytes.extend_from_slice(b"startxref\n");
        bytes.extend_from_slice(xref_offset.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        bytes
    }

    #[test]
    fn imports_classic_xref_vector_paths_as_v4_nodes() {
        let input = pdf(b"0.2 0.4 0.6 rg 10 20 m 30 40 l 50 10 l h f", "");
        let imported = import(&input, "shapes.pdf", ImportLimits::default()).unwrap();
        assert_eq!(imported.document["version"], 4);
        assert_eq!(imported.document["pages"].as_array().unwrap().len(), 1);
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        assert_eq!(node["kind"], "path");
        assert_eq!(node["d"], "M 10 60 L 30 40 L 50 70 Z");
        assert_eq!(node["style"]["fill"]["fallback"], "#336699");
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn imports_uncompressed_objects_from_plain_and_flate_xref_streams() {
        for flate in [false, true] {
            let bytes = xref_stream_pdf(flate);
            let imported = import(&bytes, "xref-stream.pdf", ImportLimits::default()).unwrap();
            let nodes = imported.document["pages"][0]["layers"][0]["nodes"]
                .as_array()
                .unwrap();
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0]["kind"], "path");
            assert_eq!(nodes[0]["style"]["fill"]["fallback"], "#0080ff");
        }

        let mut malformed = xref_stream_pdf(false);
        let size_marker = b"/Size 6";
        let size_offset = malformed
            .windows(size_marker.len())
            .position(|window| window == size_marker)
            .unwrap();
        malformed[size_offset + size_marker.len() - 1] = b'5';
        let error = import(&malformed, "bad-xref-stream.pdf", ImportLimits::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("trailing entry data"));
    }

    #[test]
    fn imports_bounded_type_two_xref_object_stream_members() {
        let imported = import(
            &object_stream_pdf(),
            "object-stream.pdf",
            ImportLimits::default(),
        )
        .unwrap();
        let nodes = imported.document["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["kind"], "path");
        assert_eq!(nodes[0]["d"], "M 10 70 L 30 50");
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn rejects_object_stream_index_mismatch_and_over_limit_member_count() {
        let mut wrong_index = object_stream_pdf();
        let type_two_entry = [2, 0, 0, 0, 5, 0, 0];
        let entry_offset = wrong_index
            .windows(type_two_entry.len())
            .position(|window| window == type_two_entry)
            .unwrap();
        wrong_index[entry_offset + 6] = 1;
        let error = import(
            &wrong_index,
            "bad-object-index.pdf",
            ImportLimits::default(),
        )
        .err()
        .unwrap();
        assert!(format!("{error:#}").contains("object stream header disagrees"));

        let excessive_count = object_stream_pdf_with_count(100_001);
        let error = import(
            &excessive_count,
            "over-limit-object-stream.pdf",
            ImportLimits::default(),
        )
        .err()
        .unwrap();
        assert!(format!("{error:#}").contains("object stream exceeds object limit"));
    }

    #[test]
    fn imports_device_cmyk_paints_as_reported_srgb_approximations() {
        let input = pdf(
            b"0 1 1 0 k 0 0 100 100 re f 1 0 0 0 K 10 10 m 90 70 l S",
            "",
        );
        let imported = import(&input, "device-cmyk.pdf", ImportLimits::default()).unwrap();
        let nodes = imported.document["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["style"]["fill"]["fallback"], "#ff0000");
        assert_eq!(nodes[1]["style"]["stroke"]["fallback"], "#00ffff");
        assert!(imported.report.items.iter().any(|entry| {
            entry.feature == "cmyk_color" && entry.status == FidelityStatus::Approximated
        }));
        assert!(imported
            .report
            .items
            .iter()
            .filter(|entry| entry.feature == "vector_path")
            .all(|entry| entry.status == FidelityStatus::Approximated));
    }

    #[test]
    fn v_curve_uses_current_point_as_first_control_point() {
        let input = pdf(b"10 10 m 20 10 l 20 20 30 10 v S", "");
        let imported = import(&input, "v-curve.pdf", ImportLimits::default()).unwrap();
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        assert_eq!(node["d"], "M 10 70 L 20 70 C 20 70 20 60 30 70");
    }

    #[test]
    fn imports_bounded_flate_rgb_image_xobject_as_embedded_asset() {
        let input = pdf_with_image(
            b"q 40 0 0 30 10 20 cm /Im0 Do Q",
            "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode",
            &flate(&[255, 0, 0, 0, 255, 0]),
        );
        let imported = import(&input, "image.pdf", ImportLimits::default()).unwrap();
        assert_eq!(imported.document["version"], 5);
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        assert_eq!(node["kind"], "image");
        assert_eq!(node["width"], 1);
        assert_eq!(node["height"], 1);
        assert_eq!(node["transform"], json!([40.0, 0.0, 0.0, 30.0, 10.0, 30.0]));
        let asset = node["asset"].as_str().unwrap();
        assert_eq!(imported.document["image_assets"][asset]["pixel_width"], 2);
        assert_eq!(imported.document["image_assets"][asset]["pixel_height"], 1);
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "image_xobject" && item.status == FidelityStatus::Approximated
        }));
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn quarter_turn_rotations_preserve_pdf_image_placement_and_bounds() {
        let content = b"q 40 0 0 30 10 20 cm /Im0 Do Q";
        let samples = flate(&[255, 0, 0, 0, 255, 0]);
        let content_stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let image_stream = [
            format!(
                "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /Length {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode >>\nstream\n",
                samples.len()
            )
            .into_bytes(),
            samples,
            b"\nendstream".to_vec(),
        ]
        .concat();
        for (rotation, expected_canvas, expected_transform) in [
            (90, [80, 100], [0.0, -40.0, -30.0, 0.0, 50.0, 90.0]),
            (270, [80, 100], [0.0, 40.0, 30.0, 0.0, 30.0, 10.0]),
        ] {
            let input = classic_pdf(
                vec![
                    b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                    b"<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 100 80] >>".to_vec(),
                    format!("<< /Type /Page /Parent 2 0 R /Rotate {rotation} /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>").into_bytes(),
                    content_stream.clone(),
                    image_stream.clone(),
                ],
                "",
            );
            let imported = import(&input, "rotated-image.pdf", ImportLimits::default()).unwrap();
            let page = &imported.document["pages"][0];
            assert_eq!(page["canvas"]["width"], expected_canvas[0]);
            assert_eq!(page["canvas"]["height"], expected_canvas[1]);
            let node = &page["layers"][0]["nodes"][0];
            assert_eq!(node["kind"], "image");
            assert_eq!(node["transform"], json!(expected_transform));
            let [a, b, c, d, e, f] = expected_transform;
            let corners = [
                (e, f),
                (a + e, b + f),
                (c + e, d + f),
                (a + c + e, b + d + f),
            ];
            assert!(corners.iter().all(|(x, y)| {
                *x >= 0.0
                    && *x <= expected_canvas[0] as f64
                    && *y >= 0.0
                    && *y <= expected_canvas[1] as f64
            }));
            crate::scene::validate(&imported.document).unwrap();
        }
    }

    #[test]
    fn imports_bounded_flate_grayscale_image_xobject() {
        let input = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",
            &flate(&[64, 192]),
        );
        let imported = import(&input, "gray-image.pdf", ImportLimits::default()).unwrap();
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        assert_eq!(node["kind"], "image");
        assert_eq!(
            imported.document["image_assets"][node["asset"].as_str().unwrap()]["color_space"],
            "srgb8"
        );
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn applies_matching_flate_device_gray_soft_mask_as_image_alpha() {
        let content = b"/Im0 Do";
        let content_stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let color_samples = flate(&[255, 0, 0, 0, 255, 0]);
        let color_stream = [
            format!(
                "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /Length {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /SMask 6 0 R >>\nstream\n",
                color_samples.len()
            )
            .into_bytes(),
            color_samples,
            b"\nendstream".to_vec(),
        ]
        .concat();
        let alpha_samples = flate(&[0, 128]);
        let alpha_stream = [
            format!(
                "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /Length {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode >>\nstream\n",
                alpha_samples.len()
            )
            .into_bytes(),
            alpha_samples,
            b"\nendstream".to_vec(),
        ]
        .concat();
        let input = classic_pdf(
            vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>".to_vec(),
                content_stream,
                color_stream,
                alpha_stream,
            ],
            "",
        );
        let imported = import(&input, "soft-mask.pdf", ImportLimits::default()).unwrap();
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        let asset = &imported.document["image_assets"][node["asset"].as_str().unwrap()];
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            asset["storage"]["data"].as_str().unwrap(),
        )
        .unwrap();
        let rgba = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [255, 0, 0, 0]);
        assert_eq!(rgba.get_pixel(1, 0).0, [0, 255, 0, 128]);
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "soft_mask" && item.status == FidelityStatus::Approximated
        }));
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn applies_bounded_rgb_and_gray_pdf_color_key_masks() {
        let rgb = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Mask [255 255 0 0 0 0]",
            &flate(&[255, 0, 0, 0, 255, 0]),
        );
        let imported = import(&rgb, "rgb-color-key.pdf", ImportLimits::default()).unwrap();
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            imported.document["image_assets"][node["asset"].as_str().unwrap()]["storage"]["data"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let rgba = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [255, 0, 0, 0]);
        assert_eq!(rgba.get_pixel(1, 0).0, [0, 255, 0, 255]);
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "image_mask" && item.status == FidelityStatus::Approximated
        }));

        let gray = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Mask [0 63]",
            &flate(&[32, 220]),
        );
        let imported = import(&gray, "gray-color-key.pdf", ImportLimits::default()).unwrap();
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            imported.document["image_assets"][node["asset"].as_str().unwrap()]["storage"]["data"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let rgba = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [32, 32, 32, 0]);
        assert_eq!(rgba.get_pixel(1, 0).0, [220, 220, 220, 255]);
    }

    #[test]
    fn unsupported_pdf_explicit_masks_are_reported_and_base_image_is_kept() {
        let input = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Mask [0 255]",
            &flate(&[10, 20, 30, 40, 50, 60]),
        );
        let imported = import(&input, "unsupported-mask.pdf", ImportLimits::default()).unwrap();
        assert_eq!(
            imported.document["pages"][0]["layers"][0]["nodes"][0]["kind"],
            "image"
        );
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "image_mask" && item.status == FidelityStatus::Dropped
        }));
    }

    #[test]
    fn imports_flate_device_cmyk_images_with_an_approximation_and_checks_decoded_size() {
        let input = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceCMYK /BitsPerComponent 8 /Filter /FlateDecode",
            &flate(&[0, 255, 255, 0, 255, 0, 0, 0]),
        );
        let imported = import(&input, "cmyk-image.pdf", ImportLimits::default()).unwrap();
        assert_eq!(imported.document["version"], 5);
        assert_eq!(
            imported.document["pages"][0]["layers"][0]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "image_xobject"
                && item.status == FidelityStatus::Approximated
                && item.message.contains("DeviceCMYK")
        }));

        let limits = ImportLimits {
            max_image_bytes: 7,
            ..ImportLimits::default()
        };
        let error = import(&input, "oversized-cmyk-image.pdf", limits)
            .err()
            .unwrap();
        assert!(error.to_string().contains("limit-exceeded"));
    }

    #[test]
    fn imports_flate_image_with_png_predictor_rows() {
        let samples = vec![10, 20, 30, 40, 50, 60, 15, 25, 35, 50, 60, 70];
        let cases: [(usize, &[u8]); 2] = [
            // Predictor 11: fixed Sub filter, one encoded row per row.
            (11, &[10, 20, 30, 30, 30, 30, 15, 25, 35, 35, 35, 35]),
            // Predictor 15: per-row filter byte; Sub on row 0 and Up on row 1.
            (15, &[1, 10, 20, 30, 30, 30, 30, 2, 5, 5, 5, 10, 10, 10]),
        ];
        for (predictor, encoded_rows) in cases {
            let dictionary = format!(
                "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode \
                 /DecodeParms << /Predictor {predictor} /Colors 3 /BitsPerComponent 8 /Columns 2 >>"
            );
            let input =
                pdf_with_image_dimensions(b"/Im0 Do", &dictionary, &flate(encoded_rows), 2, 2);
            let imported = import(&input, "png-predictor.pdf", ImportLimits::default()).unwrap();
            let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
            let asset = &imported.document["image_assets"][node["asset"].as_str().unwrap()];
            let png = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                asset["storage"]["data"].as_str().unwrap(),
            )
            .unwrap();
            let image = image::load_from_memory(&png).unwrap().to_rgb8();
            assert_eq!(image.as_raw(), &samples);
            assert!(imported.report.items.iter().any(|item| {
                item.feature == "image_xobject"
                    && item.status == FidelityStatus::Approximated
                    && item.message.contains("PNG predictor")
            }));
        }
    }

    #[test]
    fn rejects_mismatched_png_predictor_parameters_and_reports_unsupported_predictor() {
        let encoded = flate(&[10, 20, 30, 30, 30, 30]);
        let mismatch = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 11 /Colors 3 /BitsPerComponent 8 /Columns 3 >>",
            &encoded,
        );
        let error = import(&mismatch, "predictor-mismatch.pdf", ImportLimits::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("predictor parameters disagree"));

        let unsupported = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 2 >>",
            &encoded,
        );
        let imported = import(&unsupported, "tiff-predictor.pdf", ImportLimits::default()).unwrap();
        assert_eq!(imported.report.counts.dropped, 1);
        assert!(imported.document.get("image_assets").is_none());
    }

    #[test]
    fn imports_packed_one_bit_grayscale_with_row_padding_and_reports_approximation() {
        // Width is two pixels: the high bit is white, the next bit black, and
        // the remaining six padding bits must not become extra pixels.
        let input = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /FlateDecode",
            &flate(&[0b1000_0000]),
        );
        let imported = import(&input, "one-bit-gray.pdf", ImportLimits::default()).unwrap();
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        let asset = &imported.document["image_assets"][node["asset"].as_str().unwrap()];
        assert_eq!(asset["pixel_width"], 2);
        assert_eq!(asset["pixel_height"], 1);
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            asset["storage"]["data"].as_str().unwrap(),
        )
        .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_luma8();
        assert_eq!(decoded.as_raw(), &[255, 0]);
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "image_xobject" && item.status == FidelityStatus::Approximated
        }));
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn unsupported_pdf_image_filters_and_color_spaces_are_reported_as_dropped() {
        let (dictionary, data) = (
            "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /RunLengthDecode",
            &[1, 2, 3, 4][..],
        );
        let imported = import(
            &pdf_with_image(b"/Im0 Do", dictionary, data),
            "unsupported-image.pdf",
            ImportLimits::default(),
        )
        .unwrap();
        assert!(imported.document.get("image_assets").is_none());
        assert_eq!(
            imported.report.counts.dropped, 1,
            "{dictionary} should be disclosed as dropped"
        );
    }

    #[test]
    fn imports_bounded_jpeg_rgb_and_grayscale_image_xobjects() {
        let mut rgb = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(2, 1, vec![255, 0, 0, 0, 255, 0]).unwrap(),
        )
        .write_to(&mut rgb, image::ImageFormat::Jpeg)
        .unwrap();
        let imported = import(
            &pdf_with_image(
                b"/Im0 Do",
                "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
                rgb.get_ref(),
            ),
            "rgb-jpeg.pdf",
            ImportLimits::default(),
        )
        .unwrap();
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        let asset = &imported.document["image_assets"][node["asset"].as_str().unwrap()];
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            asset["storage"]["data"].as_str().unwrap(),
        )
        .unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!((decoded.width(), decoded.height()), (2, 1));
        assert!(decoded.get_pixel(0, 0)[0] > decoded.get_pixel(0, 0)[1]);
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "image_xobject"
                && item.status == FidelityStatus::Approximated
                && item.message.contains("JPEG")
        }));

        let mut gray = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(2, 1, vec![32, 220]).unwrap())
            .write_to(&mut gray, image::ImageFormat::Jpeg)
            .unwrap();
        let imported = import(
            &pdf_with_image(
                b"/Im0 Do",
                "/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",
                gray.get_ref(),
            ),
            "gray-jpeg.pdf",
            ImportLimits::default(),
        )
        .unwrap();
        let node = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        let asset = &imported.document["image_assets"][node["asset"].as_str().unwrap()];
        assert_eq!(asset["pixel_width"], 2);
        assert_eq!(asset["pixel_height"], 1);
    }

    #[test]
    fn jpeg_image_dictionary_dimensions_must_match_the_bounded_decoder() {
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(2, 1, vec![1; 6]).unwrap())
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let input = pdf_with_image_dimensions(
            b"/Im0 Do",
            "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
            jpeg.get_ref(),
            3,
            1,
        );
        let error = import(&input, "mismatched-jpeg.pdf", ImportLimits::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("dimensions"));
    }

    #[test]
    fn pdf_image_decoded_size_obeys_import_image_limit() {
        let input = pdf_with_image(
            b"/Im0 Do",
            "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode",
            &flate(&[255, 0, 0, 0, 255, 0]),
        );
        let limits = ImportLimits {
            max_image_bytes: 5,
            ..ImportLimits::default()
        };
        let error = import(&input, "too-large-image.pdf", limits).err().unwrap();
        assert!(error.to_string().contains("[limit-exceeded]"));
    }

    #[test]
    fn multipage_nodes_receive_document_unique_ids() {
        let content = b"0 0 m 10 10 l S";
        let stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let input = classic_pdf(
            vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Count 2 /Kids [3 0 R 5 0 R] >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R >>".to_vec(),
                stream,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R >>".to_vec(),
            ],
            "",
        );
        let imported = import(&input, "two-pages.pdf", ImportLimits::default()).unwrap();
        let first = imported.document["pages"][0]["layers"][0]["nodes"][0]["id"]
            .as_str()
            .unwrap();
        let second = imported.document["pages"][1]["layers"][0]["nodes"][0]["id"]
            .as_str()
            .unwrap();
        assert_ne!(first, second);
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn imports_quarter_turn_pdf_page_rotation_and_inherited_rotation() {
        let content = b"10 10 m 30 10 l S";
        let stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        for (rotation, inherited, expected_path, expected_size) in [
            (0, false, "M 10 70 L 30 70", [100, 80]),
            (90, true, "M 10 90 L 10 70", [80, 100]),
            (180, false, "M 90 10 L 70 10", [100, 80]),
            (270, false, "M 70 10 L 70 30", [80, 100]),
        ] {
            let pages_rotation = if inherited {
                format!(" /Rotate {rotation}")
            } else {
                String::new()
            };
            let page_rotation = if inherited {
                String::new()
            } else {
                format!(" /Rotate {rotation}")
            };
            let input = classic_pdf(
                vec![
                    b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                    format!("<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 100 80]{pages_rotation} >>").into_bytes(),
                    format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R{page_rotation} >>").into_bytes(),
                    stream.clone(),
                ],
                "",
            );
            let imported = import(&input, "rotated-page.pdf", ImportLimits::default()).unwrap();
            let page = &imported.document["pages"][0];
            assert_eq!(page["canvas"]["width"], expected_size[0]);
            assert_eq!(page["canvas"]["height"], expected_size[1]);
            let node = &page["layers"][0]["nodes"][0];
            assert_eq!(node["d"], expected_path);
            if rotation != 0 {
                assert!(imported.report.items.iter().any(|item| {
                    item.feature == "page_rotation" && item.status == FidelityStatus::Approximated
                }));
            }
            crate::scene::validate(&imported.document).unwrap();
        }
    }

    #[test]
    fn extracts_simple_text_and_reports_dropped_xobjects() {
        let input = pdf(b"(Hello) Tj /Im0 Do 1 1 m 10 10 l S", "");
        let imported = import(&input, "mixed.pdf", ImportLimits::default()).unwrap();
        assert_eq!(imported.report.counts.dropped, 1);
        assert_eq!(imported.report.counts.approximated, 1);
        let text = imported.document["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["kind"] == "text")
            .unwrap();
        assert_eq!(text["content"], "Hello");
        assert_eq!(text["kind"], "text");
        assert!(imported
            .report
            .items
            .iter()
            .any(|item| item.feature == "text" && item.status == FidelityStatus::Approximated));
        assert!(imported
            .report
            .items
            .iter()
            .any(|item| item.feature == "xobject"));
    }

    #[test]
    fn imports_positioned_ascii_text_and_reports_unsupported_encoding() {
        let input = pdf(
            b"BT /F1 14 Tf 1 0 0 1 12 25 Tm (Pack\\050aging\\051) Tj <FEFF00E9> Tj ET",
            "",
        );
        let imported = import(&input, "text.pdf", ImportLimits::default()).unwrap();
        let nodes = imported.document["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["content"], "Pack(aging)");
        assert_eq!(nodes[0]["x"], 12.0);
        assert_eq!(nodes[0]["y"], 55.0);
        assert_eq!(nodes[0]["font_size"], 14.0);
        assert_eq!(imported.report.counts.approximated, 1);
        assert_eq!(imported.report.counts.dropped, 2);
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "embedded_font" && item.status == FidelityStatus::Dropped
        }));
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn embeds_permitted_simple_truetype_font_and_maps_text_through_tounicode() {
        let pdf = pdf_with_embedded_font(
            b"BT /F1 14 Tf 1 0 0 1 12 25 Tm <41> Tj ET",
            crate::fonts::BUNDLED[0].3,
        );
        let imported = import(&pdf, "embedded-font.pdf", ImportLimits::default()).unwrap();
        let text = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        assert_eq!(text["content"], "A");
        assert_eq!(text["font_family"], "Atkinson Hyperlegible");
        assert_eq!(imported.document["fonts"].as_array().unwrap().len(), 1);
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "embedded_font" && item.status == FidelityStatus::Approximated
        }));
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn decodes_bounded_sequential_bfrange_tounicode_mappings() {
        let pdf = pdf_with_embedded_font(
            b"BT /F1 14 Tf 1 0 0 1 12 25 Tm <414243> Tj ET",
            crate::fonts::BUNDLED[0].3,
        );
        let imported = import(&pdf, "bfrange-font.pdf", ImportLimits::default()).unwrap();
        let text = &imported.document["pages"][0]["layers"][0]["nodes"][0];
        assert_eq!(text["content"], "ABC");
        assert_eq!(imported.document["fonts"].as_array().unwrap().len(), 1);
        crate::scene::validate(&imported.document).unwrap();
    }

    #[test]
    fn imports_embedded_font_tj_strings_and_character_adjustments() {
        let pdf = pdf_with_embedded_font(
            b"BT /F1 14 Tf 1 0 0 1 12 25 Tm [(A) -100 (B)] TJ ET",
            crate::fonts::BUNDLED[0].3,
        );
        let imported = import(&pdf, "tj-font.pdf", ImportLimits::default()).unwrap();
        let nodes = imported.document["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["content"], "A");
        assert_eq!(nodes[1]["content"], "B");
        assert_eq!(nodes[0]["x"], 12.0);
        // A is 600/1000 of 14 points; TJ -100 adds 100/1000 of 14.
        assert!((nodes[1]["x"].as_f64().unwrap() - 21.8).abs() < 1e-9);
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "text"
                && item.message.contains("TJ string")
                && item.status == FidelityStatus::Approximated
        }));
    }

    #[test]
    fn refuses_to_embed_font_with_restricted_embedding_permission() {
        let font = with_sfnt_fs_type(crate::fonts::BUNDLED[0].3.to_vec(), 0x0002);
        let pdf = pdf_with_embedded_font(b"BT /F1 14 Tf <41> Tj ET", &font);
        let imported = import(&pdf, "restricted-font.pdf", ImportLimits::default()).unwrap();
        assert!(imported.document["fonts"].as_array().unwrap().is_empty());
        assert_eq!(
            imported.document["pages"][0]["layers"][0]["nodes"][0]["font_family"],
            "Helvetica"
        );
        assert!(imported.report.items.iter().any(|item| {
            item.feature == "embedded_font"
                && item.status == FidelityStatus::Dropped
                && item.message.contains("permission is not installable")
        }));
    }

    #[test]
    fn exporter_round_trip_preserves_path_geometry_and_paint() {
        let document: crate::document::Document = serde_json::from_value(json!({
            "format":"pentool", "version":2, "name":"round-trip",
            "canvas":{"width":100,"height":80,"background":"none"},
            "layers":[{"id":"layer-1","name":"Layer 1","paths":[{
                "id":"shape","d":"M 10 20 L 30 40 L 50 10 Z",
                "fill":"#123456","stroke":"#000000","stroke_width":2.0
            }],"texts":[]}]
        }))
        .unwrap();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pentool-pdf-import-{}-{stamp}.pdf",
            std::process::id()
        ));
        crate::pdf::write(&[document], &path).unwrap();
        let bytes = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);
        let imported = import(&bytes, "round-trip.pdf", ImportLimits::default()).unwrap();
        let nodes = imported.document["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap();
        let shape = nodes
            .iter()
            .find(|node| node["style"]["fill"]["fallback"] == "#123456")
            .unwrap();
        assert_eq!(shape["d"], "M 10 20 L 30 40 L 50 10 Z");
        assert_eq!(shape["style"]["stroke_width"]["fallback"], 2.0);
    }

    #[test]
    fn rejects_encryption_xref_streams_malformed_input_and_limits() {
        assert!(import(
            &pdf(b"", " /Encrypt 8 0 R"),
            "encrypted.pdf",
            ImportLimits::default()
        )
        .err()
        .unwrap()
        .to_string()
        .contains("encrypted"));
        let xref_stream = b"%PDF-1.7\n1 0 obj\n<< /Type /XRef >>\nendobj\nstartxref\n9\n%%EOF";
        assert!(import(xref_stream, "xref-stream.pdf", ImportLimits::default()).is_err());
        assert!(import(
            b"%PDF-1.7\nnot a document",
            "bad.pdf",
            ImportLimits::default()
        )
        .is_err());
        let input = pdf(b"1 0 m 2 2 l S 3 3 m 4 4 l S", "");
        let limits = ImportLimits {
            max_nodes: 1,
            ..ImportLimits::default()
        };
        assert!(import(&input, "too-many-nodes.pdf", limits)
            .err()
            .unwrap()
            .to_string()
            .contains("limit-exceeded"));
    }

    #[test]
    fn rejects_active_actions_and_unsupported_clip_ops() {
        let content = b"1 1 m 2 2 l S";
        let stream = [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat();
        let active = classic_pdf(
            vec![
                b"<< /Type /Catalog /Pages 2 0 R /OpenAction [4 0 R /GoTo] >>".to_vec(),
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 80] /Contents 4 0 R >>".to_vec(),
                stream,
            ],
            "",
        );
        assert!(import(&active, "active.pdf", ImportLimits::default())
            .err()
            .unwrap()
            .to_string()
            .contains("unsafe-input"));
        let clipping = pdf(b"1 1 m 10 10 l W n", "");
        assert!(import(&clipping, "clip.pdf", ImportLimits::default())
            .err()
            .unwrap()
            .to_string()
            .contains("unsupported-capability"));
    }
}

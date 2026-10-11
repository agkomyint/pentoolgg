//! Supported PDF/X-4 export: a flattened CMYK page with an output intent.
//! See "PDF/X export (implemented subset)" in `docs/professional-color-v1.md`.
//!
//! The page is rendered on float surfaces, delivered once to the document's
//! CMYK output profile (alpha flattened over paper white) and stored as one
//! ICCBased CMYK image whose crop covers the media box. TrimBox and BleedBox
//! come from the page's `production` geometry; the ICC profile is embedded as
//! the `GTS_PDFX` output intent. Output is deterministic: no wall clock, a
//! content-derived trailer `/ID`, and a fixed Flate level.
//!
//! This module never claims conformance: the report records
//! `independent_validation.status = "not-run"` until a named external
//! validator's result is recorded. The built-in structural check only proves
//! that the file is internally consistent.
use super::{
    catalog,
    icc::Intent,
    output::{self, Alpha, Depth},
    production,
    value::Destination,
};
use anyhow::{bail, ensure, Context, Result};
use flate2::{write::ZlibEncoder, Compression};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

pub const MAX_PIXELS: u64 = 64_000_000;
pub const DEFAULT_PPI: f64 = 300.0;

#[derive(Debug, Clone)]
pub struct Options {
    /// CMYK destination; the document `output` role when absent.
    pub destination: Option<Destination>,
    pub intent: Option<Intent>,
    pub ppi: f64,
    pub title: Option<String>,
    /// `YYYY-MM-DDTHH:MM:SSZ`; `SOURCE_DATE_EPOCH` or the Unix epoch when absent.
    pub created: Option<String>,
    /// Flate-compress the page image and ICC profile streams (default); when
    /// false the streams are stored raw for RIPs and diff tools that want them.
    pub compress: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            destination: None,
            intent: None,
            ppi: DEFAULT_PPI,
            title: None,
            created: None,
            compress: true,
        }
    }
}

fn parse_date(text: &str) -> Result<(i64, u32, u32, u32, u32, u32)> {
    let b = text.as_bytes();
    let ok = b.len() == 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z'
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .iter()
            .all(|i| b[*i].is_ascii_digit());
    ensure!(
        ok,
        "[invalid-input] creation date must be YYYY-MM-DDTHH:MM:SSZ"
    );
    let n = |a: usize, z: usize| text[a..z].parse::<u32>().unwrap();
    let (y, mo, d, h, mi, s) = (
        i64::from(n(0, 4)),
        n(5, 7),
        n(8, 10),
        n(11, 13),
        n(14, 16),
        n(17, 19),
    );
    ensure!(
        (1..=12).contains(&mo) && (1..=31).contains(&d) && h < 24 && mi < 60 && s < 60,
        "[invalid-input] creation date has an out-of-range field"
    );
    Ok((y, mo, d, h, mi, s))
}

fn from_epoch(seconds: u64) -> String {
    // Civil-from-days (Howard Hinnant).
    let days = (seconds / 86_400) as i64;
    let rem = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn creation_date(options: &Options) -> Result<String> {
    if let Some(date) = &options.created {
        parse_date(date)?;
        return Ok(date.clone());
    }
    let seconds = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(v) => v
            .trim()
            .parse::<u64>()
            .context("[invalid-input] SOURCE_DATE_EPOCH must be unsigned seconds")?,
        Err(_) => 0,
    };
    ensure!(
        seconds < 253_402_300_800,
        "[invalid-input] SOURCE_DATE_EPOCH is out of range"
    );
    Ok(from_epoch(seconds))
}

fn pdf_string(text: &str) -> String {
    // UTF-16BE with BOM keeps arbitrary titles valid in the Info dictionary.
    let mut out = String::from("<FEFF");
    for unit in text.encode_utf16() {
        out.push_str(&format!("{unit:04X}"));
    }
    out.push('>');
    out
}

fn xml_escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            c if (c as u32) < 0x20 => " ".to_owned(),
            c => c.to_string(),
        })
        .collect()
}

/// `(filter entry, bytes)` of one stream under the compression policy.
fn encode(data: &[u8], compress: bool) -> Result<(&'static str, Vec<u8>)> {
    if compress {
        Ok(("/Filter/FlateDecode", flate(data)?))
    } else {
        Ok(("", data.to_vec()))
    }
}

fn flate(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

fn number(v: f64) -> String {
    let text = format!("{v:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

fn rect(r: &[f64; 4]) -> String {
    format!(
        "[{} {} {} {}]",
        number(r[0]),
        number(r[1]),
        number(r[2]),
        number(r[3])
    )
}

fn contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => map.contains_key(key) || map.values().any(|v| contains_key(v, key)),
        Value::Array(items) => items.iter().any(|v| contains_key(v, key)),
        _ => false,
    }
}

struct Builder {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
}
impl Builder {
    fn object(&mut self, body: &[u8]) {
        self.offsets.push(self.bytes.len());
        let n = self.offsets.len();
        self.bytes
            .extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        self.bytes.extend_from_slice(body);
        self.bytes.extend_from_slice(b"\nendobj\n");
    }
    fn stream(&mut self, dict: &str, data: &[u8]) {
        let mut body = format!("<<{dict} /Length {}>>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.object(&body);
    }
}

/// Internal consistency check of a generated file. This is not a conformance
/// validator and says nothing about PDF/X rules beyond the structure it reads.
pub fn verify_structure(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.starts_with(b"%PDF-1.6\n"),
        "[internal] missing PDF header"
    );
    ensure!(
        bytes.ends_with(b"%%EOF\n"),
        "[internal] missing end-of-file marker"
    );
    let tail_start = bytes.len().saturating_sub(128);
    let tail = String::from_utf8_lossy(&bytes[tail_start..]).into_owned();
    let start = tail
        .rfind(
            "startxref
",
        )
        .context("[internal] missing startxref")?;
    let offset: usize = tail[start + 10..]
        .lines()
        .next()
        .and_then(|l| l.trim().parse().ok())
        .context("[internal] unreadable startxref")?;
    ensure!(
        bytes.get(offset..offset + 4) == Some(b"xref"),
        "[internal] startxref does not point at the xref table"
    );
    // The table and trailer are ASCII; objects before it may hold binary streams.
    let table = String::from_utf8_lossy(&bytes[offset..]).into_owned();
    let mut lines = table.lines().skip(1);
    let range = lines.next().context("[internal] truncated xref")?;
    let count: usize = range
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .context("[internal] unreadable xref range")?;
    lines.next().context("[internal] truncated xref")?;
    for index in 1..count {
        let entry = lines.next().context("[internal] truncated xref")?;
        let at: usize = entry[..10]
            .parse()
            .context("[internal] unreadable xref offset")?;
        let marker = format!("{index} 0 obj");
        ensure!(
            bytes.get(at..at + marker.len()) == Some(marker.as_bytes()),
            "[internal] xref entry {index} does not point at its object"
        );
    }
    Ok(())
}

#[derive(Debug)]
pub struct Exported {
    pub bytes: Vec<u8>,
    pub report: Value,
}

/// Build the PDF/X-4 file in memory; nothing is written.
pub fn build(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    options: &Options,
) -> Result<Exported> {
    ensure!(
        raw["version"] == super::VERSION,
        "[unsupported-version] PDF/X export requires a version {} document",
        super::VERSION
    );
    ensure!(
        (36.0..=1200.0).contains(&options.ppi),
        "[invalid-input] export resolution must be 36..1200 ppi"
    );
    crate::scene::validate(raw)?;
    let pages = raw["pages"].as_array().context("pages missing")?;
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|page| page["id"] == id)
            .with_context(|| format!("[missing-resource] page not found: {id}"))?,
        None => pages.first().context("document has no pages")?,
    };
    let id = page["id"].as_str().unwrap_or("page");
    let geometry = production::resolve(page, true)?.with_context(|| {
        format!("[invalid-input] page {id} has no production geometry; set a trim box (pentool color production set) before PDF/X export")
    })?;
    for (key, what) in [
        ("spot", "spot-ink paints"),
        ("overprint", "overprint declarations"),
    ] {
        ensure!(
            !contains_key(page, key),
            "[unsupported-capability] page {id} uses {what}; this PDF/X subset flattens to process CMYK and would lose them. Use plate separations for spot work"
        );
    }
    let (destination, intent) = match (&options.destination, options.intent) {
        (Some(d), Some(i)) => (d.clone(), i),
        _ => {
            let role = super::roles::effective(raw, super::roles::Role::Output)?;
            (
                options
                    .destination
                    .clone()
                    .unwrap_or_else(|| role.destination.clone()),
                options.intent.unwrap_or(role.intent),
            )
        }
    };
    ensure!(
        destination.space() == "icc-cmyk",
        "[unsupported-capability] PDF/X export needs an ICC CMYK destination, not {}",
        destination.space()
    );
    let reference = destination
        .profile()
        .context("[invalid-input] CMYK destination has no profile")?
        .to_owned();
    let (identity, profile) = catalog::load(raw, document, &reference)?;
    ensure!(
        profile.len() >= 128 && &profile[16..20] == b"CMYK" && &profile[12..16] == b"prtr",
        "[invalid-input] output intent profile {reference} must be a CMYK output (prtr) profile"
    );
    ensure!(
        profile[8] <= 4,
        "[unsupported-capability] output intent profile {reference} is ICC v{}; PDF/X-4 embeds profiles up to v4",
        profile[8]
    );
    let canvas = geometry.canvas;
    let scale = options.ppi / geometry.units_per_inch;
    ensure!(
        (0.1..=16.0).contains(&scale),
        "[limit-exceeded] export scale {scale} is outside 0.1..16; adjust --ppi or units_per_inch"
    );
    let pixels = (canvas.0 * scale).ceil() * (canvas.1 * scale).ceil();
    ensure!(
        pixels <= MAX_PIXELS as f64,
        "[limit-exceeded] page raster of {pixels:.0} pixels exceeds {MAX_PIXELS}; lower --ppi or shrink the canvas"
    );
    let created = creation_date(options)?;
    let title = options
        .title
        .clone()
        .or_else(|| page["name"].as_str().map(str::to_owned))
        .unwrap_or_else(|| id.to_owned());
    ensure!(
        !title.is_empty() && title.len() <= 256,
        "[invalid-input] title must be 1..=256 bytes"
    );

    let image = output::render(
        raw,
        document,
        page_id,
        scale as f32,
        &output::Options {
            destination: Some(destination.clone()),
            intent: Some(intent),
            depth: Depth::Eight,
            alpha: Alpha::FlattenOnWhite,
        },
    )?;
    ensure!(
        image.channels == 4,
        "[internal] CMYK delivery is not 4-channel"
    );
    let (width, height) = (image.width as usize, image.height as usize);
    let sx = width as f64 / canvas.0;
    let sy = height as f64 / canvas.1;
    let m = geometry.media;
    let x0 = ((m.x * sx).floor().max(0.0)) as usize;
    let y0 = ((m.y * sy).floor().max(0.0)) as usize;
    let x1 = (((m.x + m.width) * sx).ceil() as usize).min(width);
    let y1 = (((m.y + m.height) * sy).ceil() as usize).min(height);
    ensure!(x1 > x0 && y1 > y0, "[internal] empty media crop");
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut samples = Vec::with_capacity(cw * ch * 4);
    for row in y0..y1 {
        for s in &image.samples[(row * width + x0) * 4..(row * width + x1) * 4] {
            samples.push(*s as u8);
        }
    }
    let marks = geometry.paint_marks(image.width, image.height, |x, y, coverage| {
        let (x, y) = (x as usize, y as usize);
        if x >= x0 && x < x1 && y >= y0 && y < y1 {
            let at = ((y - y0) * cw + (x - x0)) * 4;
            for sample in &mut samples[at..at + 4] {
                let old = f64::from(*sample);
                *sample = (old + (255.0 - old) * coverage).round() as u8;
            }
        }
    });
    let patches = geometry.paint_color_bars(image.width, image.height, |x, y, coverage, cmyk| {
        let (x, y) = (x as usize, y as usize);
        if x >= x0 && x < x1 && y >= y0 && y < y1 {
            let at = ((y - y0) * cw + (x - x0)) * 4;
            for (sample, value) in samples[at..at + 4].iter_mut().zip(cmyk) {
                let old = f64::from(*sample);
                *sample = (old + (value * 255.0 - old) * coverage).round() as u8;
            }
        }
    });
    let slug_text =
        geometry.paint_slug_text(raw, page, document, scale as f32, |x, y, coverage| {
            let (x, y) = (x as usize, y as usize);
            if x >= x0 && x < x1 && y >= y0 && y < y1 {
                let at = ((y - y0) * cw + (x - x0)) * 4;
                for sample in &mut samples[at..at + 4] {
                    let old = f64::from(*sample);
                    *sample = (old + (255.0 - old) * coverage).round() as u8;
                }
            }
        })?;
    let k = 72.0 / geometry.units_per_inch;
    let media_pts = [0.0, 0.0, m.width * k, m.height * k];
    let place = [
        (x0 as f64 / sx - m.x) * k,
        (m.y + m.height - y1 as f64 / sy) * k,
        (cw as f64 / sx) * k,
        (ch as f64 / sy) * k,
    ];
    let boxes = geometry.pdf_boxes();
    let box_of = |key: &str| -> Result<[f64; 4]> {
        let v = boxes[key].as_array().context("[internal] missing box")?;
        let mut out = [0.0; 4];
        for (slot, value) in out.iter_mut().zip(v) {
            *slot = value.as_f64().context("[internal] box value")?;
        }
        Ok(out)
    };
    let trim = box_of("trim")?;
    let bleed = box_of("bleed")?;

    let mut digest = Sha256::new();
    digest.update(&samples);
    digest.update(&profile);
    digest.update(format!("{trim:?}{bleed:?}{media_pts:?}{title}{created}").as_bytes());
    let id_hex = hex::encode(&digest.finalize()[..16]);

    let date = parse_date(&created)?;
    let info_date = format!(
        "D:{:04}{:02}{:02}{:02}{:02}{:02}Z",
        date.0, date.1, date.2, date.3, date.4, date.5
    );
    let xmp = format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" \
xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\" xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\" \
xmlns:pdfx=\"http://ns.adobe.com/pdfx/1.3/\">\n\
<dc:format>application/pdf</dc:format>\n\
<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{title}</rdf:li></rdf:Alt></dc:title>\n\
<xmp:CreateDate>{created}</xmp:CreateDate>\n<xmp:ModifyDate>{created}</xmp:ModifyDate>\n\
<xmp:MetadataDate>{created}</xmp:MetadataDate>\n<xmp:CreatorTool>Pentool</xmp:CreatorTool>\n\
<pdf:Producer>Pentool</pdf:Producer>\n<pdf:Trapped>False</pdf:Trapped>\n\
<xmpMM:DocumentID>uuid:{docid}</xmpMM:DocumentID>\n<xmpMM:InstanceID>uuid:{docid}</xmpMM:InstanceID>\n\
<xmpMM:RenditionClass>default</xmpMM:RenditionClass>\n<xmpMM:VersionID>1</xmpMM:VersionID>\n\
<pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>\n\
</rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>",
        title = xml_escape(&title),
        created = created,
        docid = format!(
            "{}-{}-{}-{}-{}",
            &id_hex[0..8],
            &id_hex[8..12],
            &id_hex[12..16],
            &id_hex[16..20],
            &id_hex[20..32]
        ),
    );

    let mut b = Builder {
        bytes: b"%PDF-1.6\n%\xE2\xE3\xCF\xD3\n".to_vec(),
        offsets: Vec::new(),
    };
    b.object(b"<</Type/Catalog/Pages 2 0 R/OutputIntents[5 0 R]/Metadata 7 0 R>>");
    b.object(b"<</Type/Pages/Kids[3 0 R]/Count 1>>");
    b.object(
        format!(
            "<</Type/Page/Parent 2 0 R/MediaBox{}/BleedBox{}/TrimBox{}/Contents 4 0 R\
/Resources<</XObject<</Im0 6 0 R>>>>>>",
            rect(&media_pts),
            rect(&bleed),
            rect(&trim)
        )
        .as_bytes(),
    );
    let content = format!(
        "q {} 0 0 {} {} {} cm /Im0 Do Q",
        number(place[2]),
        number(place[3]),
        number(place[0]),
        number(place[1])
    );
    b.stream("", content.as_bytes());
    let condition = xml_escape(&reference);
    b.object(
        format!(
            "<</Type/OutputIntent/S/GTS_PDFX/OutputConditionIdentifier({cond})\
/OutputCondition({cond})/Info({cond})/DestOutputProfile 8 0 R>>",
            cond = condition.replace(['(', ')', '\\'], "_")
        )
        .as_bytes(),
    );
    let (image_filter, image_bytes) = encode(&samples, options.compress)?;
    b.stream(
        &format!(
            "/Type/XObject/Subtype/Image/Width {cw}/Height {ch}/ColorSpace[/ICCBased 8 0 R]\
/BitsPerComponent 8{image_filter}"
        ),
        &image_bytes,
    );
    b.stream("/Type/Metadata/Subtype/XML", xmp.as_bytes());
    let (profile_filter, profile_bytes) = encode(&profile, options.compress)?;
    b.stream(
        &format!("/N 4/Alternate/DeviceCMYK{profile_filter}"),
        &profile_bytes,
    );
    b.object(
        format!(
            "<</Title{t}/Producer(Pentool)/Creator(Pentool)/CreationDate({d})/ModDate({d})\
/Trapped/False/GTS_PDFXVersion(PDF/X-4)>>",
            t = pdf_string(&title),
            d = info_date
        )
        .as_bytes(),
    );
    let xref = b.bytes.len();
    let count = b.offsets.len() + 1;
    let mut table = format!("xref\n0 {count}\n0000000000 65535 f \n");
    for offset in &b.offsets {
        table.push_str(&format!("{offset:010} 00000 n \n"));
    }
    b.bytes.extend_from_slice(table.as_bytes());
    b.bytes.extend_from_slice(
        format!(
            "trailer\n<</Size {count}/Root 1 0 R/Info 9 0 R/ID[<{id_hex}><{id_hex}>]>>\n\
startxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    verify_structure(&b.bytes)?;
    let report = json!({
        "scope": "pdfx-4-flattened-cmyk-export",
        "standard": "PDF/X-4",
        "page": id,
        "bytes": b.bytes.len(),
        "sha256": hex::encode(Sha256::digest(&b.bytes)),
        "pixels": {"width": cw, "height": ch, "ppi": options.ppi, "channels": 4, "bits": 8},
        "compression": if options.compress { "flate" } else { "none" },
        "boxes": {"media": media_pts, "bleed": bleed, "trim": trim},
        "output_intent": {
            "profile": reference,
            "identity": identity,
            "profile_sha256": hex::encode(Sha256::digest(&profile)),
            "profile_bytes": profile.len(),
            "intent": format!("{intent:?}"),
        },
        "creation_date": created,
        "transparency": "flattened-to-raster",
        "marks": {"rendered": marks, "ink": "registration", "color_bar_patches": patches, "slug_text": slug_text},
        "fonts": "rasterized; no font programs embedded",
        "structural_check": "passed",
        "independent_validation": {
            "status": "not-run",
            "validator": null,
            "version": null,
            "ruleset": null,
            "note": "Unvalidated draft: no PDF/X conformance claim until a named validator result is recorded."
        },
    });
    Ok(Exported {
        bytes: b.bytes,
        report,
    })
}

/// Export to `out`. Failure at any step leaves no file behind; an existing
/// file is never replaced.
pub fn export(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    out: &Path,
    options: &Options,
) -> Result<Value> {
    if out.exists() {
        bail!(
            "[invalid-input] {} already exists; choose a new output path",
            out.display()
        );
    }
    let built = build(raw, document, page_id, options)?;
    crate::editing::atomic_write(out, &built.bytes)?;
    let mut report = built.report;
    report["output"] = json!(out.display().to_string());
    Ok(report)
}

//! IDML ⇄ `.pen` interchange (v0.14.0 tasks 13 and 14).
//!
//! IDML is Adobe's published XML interchange format (a ZIP of XML parts).
//! This module reads and writes only that published format. Everything is
//! processed in memory: archives are never extracted to disk, DTDs are
//! refused, no URI is ever resolved, and every count and size is bounded
//! before the expensive step (`docs/layout-v1.md` sections 12 and 14).
//!
//! The mapping contract is `docs/idml-mapping-v1.md`. Import returns the new
//! document plus a fidelity report that lists every approximated, carried or
//! unsupported item; nothing is dropped silently.

use anyhow::{bail, ensure, Context, Result};
use roxmltree::{Document, Node, NodeType, ParsingOptions};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read};

pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_UNCOMPRESSED: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_RATIO: u64 = 100;
pub const MAX_PART_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_XML_DEPTH: usize = 128;
pub const MAX_ATTRIBUTES: usize = 256;
const MAX_XML_NODES: u32 = 4_000_000;
const MAX_DIMENSION: f64 = 1_000_000.0;
const MAX_REPORT_ITEMS: usize = 5_000;
pub(crate) const MIMETYPE: &str = "application/vnd.adobe.indesign-idml-package";
const NATIVE_MESSAGE: &str = "Pentool cannot open native InDesign files. In InDesign choose File \u{203a} Export \u{203a} InDesign Markup (IDML) and open that file.";

// ------------------------------------------------------------ report

#[derive(Default)]
struct Report {
    items: Vec<Value>,
    counts: [usize; 4],
    dropped: usize,
}

impl Report {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        status: &str,
        part: &str,
        element: &str,
        attribute: Option<&str>,
        reference: &str,
        message: &str,
        action: &str,
    ) {
        let slot = match status {
            "supported" => 0,
            "approximated" => 1,
            "carried" => 2,
            _ => 3,
        };
        self.counts[slot] += 1;
        if status == "supported" {
            return;
        }
        if self.items.len() >= MAX_REPORT_ITEMS {
            self.dropped += 1;
            return;
        }
        self.items.push(json!({
            "status": status, "part": part, "element": element,
            "attribute": attribute, "ref": reference,
            "message": message, "action": action,
        }));
    }

    fn supported(&mut self) {
        self.counts[0] += 1;
    }

    fn finish(self, source: &str, extra: Value) -> Value {
        let mut out = json!({
            "source": source,
            "counts": {
                "supported": self.counts[0], "approximated": self.counts[1],
                "carried": self.counts[2], "unsupported": self.counts[3],
            },
            "items": self.items,
            "items_omitted": self.dropped,
        });
        if let (Some(map), Some(extra)) = (out.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                map.insert(k.clone(), v.clone());
            }
        }
        out
    }
}

// ------------------------------------------------------------ archive

/// Read the text parts of an IDML package with every hostile-input bound.
fn read_package(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    ensure!(
        bytes.len() as u64 <= MAX_ARCHIVE_BYTES,
        "[limit-exceeded] IDML archive is {} bytes; the limit is {MAX_ARCHIVE_BYTES}",
        bytes.len()
    );
    if bytes.starts_with(&[0x06, 0x06, 0xED, 0xF5, 0xD8, 0x1D, 0x46, 0xE5]) {
        bail!("[unsupported-format] {NATIVE_MESSAGE}");
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| anyhow::anyhow!("[malformed-resource] not a ZIP archive: {e}"))?;
    ensure!(
        zip.len() <= MAX_ENTRIES,
        "[limit-exceeded] IDML archive has {} entries; the limit is {MAX_ENTRIES}",
        zip.len()
    );
    let mut parts = BTreeMap::new();
    let mut total: u64 = 0;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| anyhow::anyhow!("[malformed-resource] archive entry {index}: {e}"))?;
        let name = entry.name().to_string();
        check_entry_name(&name)?;
        if entry.is_dir() {
            continue;
        }
        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            bail!("[unsafe-path] archive entry {name:?} is a symbolic link");
        }
        let declared = entry.size();
        ensure!(
            declared <= MAX_PART_BYTES,
            "[limit-exceeded] archive entry {name:?} is {declared} bytes; the limit is {MAX_PART_BYTES}"
        );
        let wanted = name == "mimetype" || name.to_ascii_lowercase().ends_with(".xml");
        let mut data = Vec::new();
        let mut limited = (&mut entry).take(MAX_PART_BYTES + 1);
        if wanted {
            limited
                .read_to_end(&mut data)
                .map_err(|e| anyhow::anyhow!("[malformed-resource] reading {name:?}: {e}"))?;
        } else {
            // Binary parts are measured, never kept.
            let mut sink = std::io::sink();
            std::io::copy(&mut limited, &mut sink)
                .map_err(|e| anyhow::anyhow!("[malformed-resource] reading {name:?}: {e}"))?;
        }
        let actual = if wanted { data.len() as u64 } else { declared };
        ensure!(
            actual <= MAX_PART_BYTES,
            "[limit-exceeded] archive entry {name:?} exceeds {MAX_PART_BYTES} bytes while streaming"
        );
        total = total.saturating_add(actual.max(declared.min(MAX_PART_BYTES)));
        ensure!(
            total <= MAX_UNCOMPRESSED,
            "[limit-exceeded] IDML archive expands past {MAX_UNCOMPRESSED} bytes"
        );
        let compressed = entry.compressed_size().max(1);
        ensure!(
            declared <= (1 << 20) || declared / compressed <= MAX_RATIO,
            "[limit-exceeded] archive entry {name:?} has a compression ratio above {MAX_RATIO}:1"
        );
        if wanted {
            let text = String::from_utf8(data).map_err(|_| {
                anyhow::anyhow!("[malformed-resource] archive entry {name:?} is not UTF-8")
            })?;
            let text = text
                .strip_prefix('\u{feff}')
                .map(str::to_string)
                .unwrap_or(text);
            parts.insert(name, text);
        }
    }
    Ok(parts)
}

pub(crate) fn check_entry_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.contains(':')
        || name.split('/').any(|s| s == "..");
    ensure!(
        !bad,
        "[unsafe-path] archive entry {name:?} escapes the package (absolute, `..`, drive or NUL)"
    );
    Ok(())
}

// ------------------------------------------------------------ xml

pub(crate) fn parse<'a>(text: &'a str, part: &str) -> Result<Document<'a>> {
    ensure!(
        !text.contains("<!DOCTYPE") && !text.contains("<!ENTITY"),
        "[unsafe-input] {part}: DTDs and entity declarations are refused"
    );
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_XML_NODES,
    };
    let doc = Document::parse_with_options(text, options)
        .map_err(|e| anyhow::anyhow!("[malformed-resource] {part}: {e}"))?;
    for node in doc.descendants().filter(Node::is_element) {
        let depth = node.ancestors().count();
        ensure!(
            depth <= MAX_XML_DEPTH,
            "[limit-exceeded] {part}: XML nesting {depth} exceeds the limit of {MAX_XML_DEPTH}"
        );
        ensure!(
            node.attributes().len() <= MAX_ATTRIBUTES,
            "[limit-exceeded] {part}: element {} has {} attributes; the limit is {MAX_ATTRIBUTES}",
            node.tag_name().name(),
            node.attributes().len()
        );
    }
    Ok(doc)
}

fn is(node: &Node, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}

fn kids<'a, 'i>(node: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(move |c| is(c, name))
}

/// An attribute, or the text of the same-named element under `Properties`.
fn prop(node: &Node, name: &str) -> Option<String> {
    if let Some(v) = node.attribute(name) {
        return Some(v.to_string());
    }
    node.children()
        .find(|c| is(c, "Properties"))
        .and_then(|p| p.children().find(|c| is(c, name)))
        .map(|c| c.text().unwrap_or_default().trim().to_string())
}

fn num(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && n.abs() <= MAX_DIMENSION)
}

fn boolean(text: &str) -> Option<bool> {
    match text.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Identifier made of letters, digits and `_ - . :` (unique per call site).
fn safe_id(raw: &str) -> String {
    let decoded = decode(raw);
    let tail = decoded.rsplit('/').next().unwrap_or(&decoded);
    let mut out: String = tail
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out = "item".into();
    }
    out.truncate(100);
    out
}

fn style_name(raw: &str) -> Option<String> {
    let decoded = decode(raw);
    let name = decoded
        .split_once('/')
        .map_or(decoded.as_str(), |(_, n)| n)
        .to_string();
    if name.starts_with("$ID/") || name.is_empty() || name == "None" {
        None
    } else {
        Some(
            name.chars()
                .filter(|c| !c.is_control())
                .map(|c| if c == '/' || c == '\\' { '-' } else { c })
                .collect::<String>()
                .chars()
                .take(128)
                .collect(),
        )
    }
}

fn serialize(node: Node) -> String {
    fn esc(text: &str, attr: bool) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' if attr => out.push_str("&quot;"),
                _ => out.push(c),
            }
        }
        out
    }
    fn walk(node: Node, out: &mut String) {
        match node.node_type() {
            NodeType::Element => {
                let name = qualified(node);
                out.push('<');
                out.push_str(&name);
                for a in node.attributes() {
                    out.push(' ');
                    out.push_str(a.name());
                    out.push_str("=\"");
                    out.push_str(&esc(a.value(), true));
                    out.push('"');
                }
                if node.has_children() {
                    out.push('>');
                    for c in node.children() {
                        walk(c, out);
                    }
                    out.push_str("</");
                    out.push_str(&name);
                    out.push('>');
                } else {
                    out.push_str("/>");
                }
            }
            NodeType::Text => out.push_str(&esc(node.text().unwrap_or_default(), false)),
            NodeType::PI => {
                if let Some(pi) = node.pi() {
                    out.push_str("<?");
                    out.push_str(pi.target);
                    if let Some(v) = pi.value {
                        out.push(' ');
                        out.push_str(v);
                    }
                    out.push_str("?>");
                }
            }
            _ => {}
        }
    }
    fn qualified(node: Node) -> String {
        let name = node.tag_name().name();
        match node
            .tag_name()
            .namespace()
            .and_then(|ns| node.lookup_prefix(ns))
        {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}:{name}"),
            _ => name.to_string(),
        }
    }
    let mut out = String::new();
    walk(node, &mut out);
    out
}

// ------------------------------------------------------------ geometry

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn parse_matrix(text: Option<&str>) -> Matrix {
    let Some(text) = text else { return IDENTITY };
    let v: Vec<f64> = text.split_whitespace().filter_map(num).collect();
    if v.len() == 6 {
        [v[0], v[1], v[2], v[3], v[4], v[5]]
    } else {
        IDENTITY
    }
}

/// `child` applied first, then `parent`.
fn compose(child: &Matrix, parent: &Matrix) -> Matrix {
    let [ca, cb, cc, cd, ctx, cty] = *child;
    let [pa, pb, pc, pd, ptx, pty] = *parent;
    [
        pa * ca + pc * cb,
        pb * ca + pd * cb,
        pa * cc + pc * cd,
        pb * cc + pd * cd,
        pa * ctx + pc * cty + ptx,
        pb * ctx + pd * cty + pty,
    ]
}

fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn axis_aligned(m: &Matrix) -> bool {
    m[1].abs() < 1e-9 && m[2].abs() < 1e-9
}

pub(crate) fn round(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

#[derive(Clone, Copy, Debug)]
struct BBox {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl BBox {
    fn of(points: &[(f64, f64)]) -> Option<BBox> {
        let first = points.first()?;
        let mut b = BBox {
            x0: first.0,
            y0: first.1,
            x1: first.0,
            y1: first.1,
        };
        for &(x, y) in points {
            b.x0 = b.x0.min(x);
            b.y0 = b.y0.min(y);
            b.x1 = b.x1.max(x);
            b.y1 = b.y1.max(y);
        }
        Some(b)
    }
    fn union(self, o: BBox) -> BBox {
        BBox {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
    fn center(self) -> (f64, f64) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
}

/// One bezier anchor: position and its two handles (spread coordinates).
#[derive(Clone, Copy)]
struct Anchor {
    p: (f64, f64),
    l: (f64, f64),
    r: (f64, f64),
}

fn point(text: Option<&str>) -> Option<(f64, f64)> {
    let v: Vec<f64> = text?.split_whitespace().filter_map(num).collect();
    (v.len() == 2).then(|| (v[0], v[1]))
}

/// Subpaths of an item: anchors transformed by `m`, plus an `open` flag.
fn subpaths(item: &Node, m: &Matrix) -> Vec<(Vec<Anchor>, bool)> {
    let mut out = Vec::new();
    for geometry in item.descendants().filter(|n| is(n, "GeometryPathType")) {
        // Stay with this item's own geometry, not a nested child's.
        let owner = geometry.ancestors().find(|a| {
            a.is_element()
                && !matches!(
                    a.tag_name().name(),
                    "Properties" | "PathGeometry" | "GeometryPathType" | "PathPointArray"
                )
        });
        if owner.map(|o| o.id()) != Some(item.id()) {
            continue;
        }
        let open = geometry
            .attribute("PathOpen")
            .and_then(boolean)
            .unwrap_or(false);
        let mut anchors = Vec::new();
        for pt in geometry.descendants().filter(|n| is(n, "PathPointType")) {
            let Some(a) = point(pt.attribute("Anchor")) else {
                continue;
            };
            let l = point(pt.attribute("LeftDirection")).unwrap_or(a);
            let r = point(pt.attribute("RightDirection")).unwrap_or(a);
            anchors.push(Anchor {
                p: apply(m, a.0, a.1),
                l: apply(m, l.0, l.1),
                r: apply(m, r.0, r.1),
            });
        }
        if !anchors.is_empty() {
            out.push((anchors, open));
        }
    }
    out
}

fn path_data(subpaths: &[(Vec<Anchor>, bool)], ox: f64, oy: f64) -> String {
    let mut d = String::new();
    for (anchors, open) in subpaths {
        let at = |p: (f64, f64)| format!("{} {}", round(p.0 - ox), round(p.1 - oy));
        d.push_str(&format!("M {}", at(anchors[0].p)));
        let straight = |a: &Anchor, b: &Anchor| a.r == a.p && b.l == b.p;
        for pair in anchors.windows(2) {
            if straight(&pair[0], &pair[1]) {
                d.push_str(&format!(" L {}", at(pair[1].p)));
            } else {
                d.push_str(&format!(
                    " C {} {} {}",
                    at(pair[0].r),
                    at(pair[1].l),
                    at(pair[1].p)
                ));
            }
        }
        if !*open {
            let (last, first) = (anchors.last().unwrap(), &anchors[0]);
            if !straight(last, first) {
                d.push_str(&format!(
                    " C {} {} {}",
                    at(last.r),
                    at(first.l),
                    at(first.p)
                ));
            }
            d.push_str(" Z");
        }
        d.push(' ');
    }
    d.trim_end().to_string()
}

// ------------------------------------------------------------ colors

fn clamp255(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn hex(r: f64, g: f64, b: f64) -> String {
    format!("#{:02x}{:02x}{:02x}", clamp255(r), clamp255(g), clamp255(b))
}

fn lab_to_hex(l: f64, a: f64, b: f64) -> String {
    let fy = (l + 16.0) / 116.0;
    let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
    let f = |t: f64| {
        if t * t * t > 0.008856 {
            t * t * t
        } else {
            (t - 16.0 / 116.0) / 7.787
        }
    };
    // D50 white, Bradford-adapted sRGB matrix.
    let (x, y, z) = (0.9642 * f(fx), f(fy), 0.8251 * f(fz));
    let lin = [
        3.1338561 * x - 1.6168667 * y - 0.4906146 * z,
        -0.9787684 * x + 1.9161415 * y + 0.0334540 * z,
        0.0719453 * x - 0.2289914 * y + 1.4052427 * z,
    ];
    let gamma = |c: f64| {
        let c = c.clamp(0.0, 1.0);
        255.0
            * if c <= 0.0031308 {
                12.92 * c
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            }
    };
    hex(gamma(lin[0]), gamma(lin[1]), gamma(lin[2]))
}

/// Display value for a swatch; `approximated` when the model is not RGB.
pub(crate) fn swatch_hex(space: &str, value: &str) -> Option<(String, bool)> {
    let v: Vec<f64> = value
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    match (space, v.len()) {
        ("RGB", 3) => Some((hex(v[0], v[1], v[2]), false)),
        ("CMYK", 4) => {
            let k = 1.0 - v[3] / 100.0;
            Some((
                hex(
                    255.0 * (1.0 - v[0] / 100.0) * k,
                    255.0 * (1.0 - v[1] / 100.0) * k,
                    255.0 * (1.0 - v[2] / 100.0) * k,
                ),
                true,
            ))
        }
        ("LAB", 3) => Some((lab_to_hex(v[0], v[1], v[2]), true)),
        _ => None,
    }
}

fn mix_with_white(hex_color: &str, tint: f64) -> String {
    let c = |i: usize| u8::from_str_radix(&hex_color[i..i + 2], 16).unwrap_or(0) as f64;
    let t = (tint / 100.0).clamp(0.0, 1.0);
    let m = |v: f64| 255.0 - (255.0 - v) * t;
    hex(m(c(1)), m(c(3)), m(c(5)))
}

// ------------------------------------------------------------ import

pub struct Imported {
    pub document: Value,
    pub report: Value,
}

const MAX_CARRIED_BYTES: usize = 16 * 1024 * 1024;
const NO_ITEM_CHILDREN: &[&str] = &[
    "Properties",
    "PathGeometry",
    "TextFramePreference",
    "FrameFittingOption",
    "Image",
    "PDF",
    "EPS",
    "Link",
];
const SHAPES: &[&str] = &["Rectangle", "Oval", "Polygon", "GraphicLine", "TextFrame"];
const IGNORED_STYLE_ATTRS: &[&str] = &[
    "Self",
    "Name",
    "Imported",
    "KeyboardShortcut",
    "StyleUniqueId",
    "EmitCss",
    "IncludeClass",
    "BasedOn",
    "NextStyle",
];
const MAPPED_STYLE_ATTRS: &[&str] = &[
    "AppliedFont",
    "FontStyle",
    "PointSize",
    "Leading",
    "Justification",
    "LeftIndent",
    "RightIndent",
    "FirstLineIndent",
    "LastLineIndent",
    "SpaceBefore",
    "SpaceAfter",
    "Hyphenation",
    "Tracking",
    "FillColor",
];

type FrameLink = (String, Option<String>);

#[derive(Default)]
struct Imp {
    rep: Report,
    swatches: HashMap<String, String>,
    swatch_meta: Map<String, Value>,
    used: HashSet<String>,
    carried: Vec<Value>,
    carried_bytes: usize,
    carried_counts: BTreeMap<(String, String), usize>,
    para_styles: HashMap<String, String>,
    char_styles: HashMap<String, String>,
    style_ext: Map<String, Value>,
    items_ext: Map<String, Value>,
}

impl Imp {
    fn unique(&mut self, scope: &str, base: &str) -> String {
        let root = safe_id(base);
        let mut id = root.clone();
        let mut n = 2;
        while !self.used.insert(format!("{scope}:{id}")) {
            id = format!("{root}-{n}");
            n += 1;
        }
        id
    }

    fn carry(&mut self, part: &str, owner: &str, node: Node, reason: &str) {
        let name = node.tag_name().name().to_string();
        *self
            .carried_counts
            .entry((part.to_string(), name.clone()))
            .or_insert(0) += 1;
        let xml = serialize(node);
        if self.carried_bytes + xml.len() > MAX_CARRIED_BYTES {
            self.rep.add(
                "unsupported",
                part,
                &name,
                None,
                owner,
                "carried-data budget exceeded; element not stored",
                "split the document or remove unused content before import",
            );
            return;
        }
        self.carried_bytes += xml.len();
        self.carried.push(json!({
            "part": part, "owner": owner, "element": name, "reason": reason, "xml": xml,
        }));
    }

    fn color(&self, raw: Option<&str>) -> Option<String> {
        let raw = raw?.trim();
        if raw.is_empty() || raw.ends_with("/None") {
            return None;
        }
        self.swatches.get(raw).cloned()
    }

    fn find_part<'a>(
        &mut self,
        parts: &'a BTreeMap<String, String>,
        src: &str,
        from: &str,
    ) -> Option<&'a str> {
        let found = check_entry_name(src)
            .ok()
            .and_then(|_| parts.get(src).map(String::as_str));
        if found.is_none() {
            self.rep.add(
                "unsupported",
                from,
                "src",
                Some("src"),
                src,
                "referenced package part is missing or has an unsafe path",
                "re-export the IDML from InDesign",
            );
        }
        found
    }
}

/// Map an IDML font style name; false when part of it is not understood.
fn font_style(style: &str, out: &mut Map<String, Value>) -> bool {
    let mut s = style.to_ascii_lowercase();
    let italic = s.contains("italic") || s.contains("oblique");
    s = s.replace("italic", "").replace("oblique", "");
    let mut weight = None;
    for (word, value) in [
        ("extralight", 200),
        ("semibold", 600),
        ("extrabold", 800),
        ("thin", 100),
        ("light", 300),
        ("regular", 400),
        ("medium", 500),
        ("bold", 700),
        ("black", 900),
    ] {
        if s.contains(word) {
            weight = Some(value);
            s = s.replace(word, "");
        }
    }
    if let Some(w) = weight.filter(|w| *w != 400) {
        out.insert("weight".into(), json!(w));
    }
    if italic {
        out.insert("italic".into(), json!(true));
    }
    s.trim().is_empty()
}

fn justification(value: &str) -> Option<&'static str> {
    Some(match value {
        "LeftAlign" => "left",
        "CenterAlign" => "center",
        "RightAlign" => "right",
        "FullyJustified" => "justify",
        "LeftJustified" => "justify-last-left",
        "CenterJustified" => "justify-last-center",
        "RightJustified" => "justify-last-right",
        "ToBindingSide" => "towards-spine",
        "AwayFromBindingSide" => "away-from-spine",
        _ => return None,
    })
}

fn map_style(imp: &Imp, node: &Node, kind: &str) -> (Map<String, Value>, Map<String, Value>) {
    let mut out = Map::new();
    let mut extra = Map::new();
    let number = |attr: &str| prop(node, attr).and_then(|t| num(&t));
    if let Some(font) = prop(node, "AppliedFont") {
        out.insert("font".into(), json!(font));
    }
    if let Some(fs) = prop(node, "FontStyle") {
        if !font_style(&fs, &mut out) {
            extra.insert("FontStyle".into(), json!(fs));
        }
    }
    if let Some(v) = number("PointSize") {
        out.insert("size".into(), json!(v));
    }
    if let Some(v) = number("Leading") {
        out.insert("leading".into(), json!(v));
    }
    if kind == "paragraph" {
        if let Some(j) = prop(node, "Justification") {
            match justification(&j) {
                Some(a) => {
                    out.insert("align".into(), json!(a));
                }
                None => {
                    extra.insert("Justification".into(), json!(j));
                }
            }
        }
        for (key, attr) in [
            ("left_indent", "LeftIndent"),
            ("right_indent", "RightIndent"),
            ("first_indent", "FirstLineIndent"),
            ("last_indent", "LastLineIndent"),
            ("space_before", "SpaceBefore"),
            ("space_after", "SpaceAfter"),
        ] {
            if let Some(v) = number(attr) {
                out.insert(key.into(), json!(v));
            }
        }
        if let Some(h) = prop(node, "Hyphenation").and_then(|t| boolean(&t)) {
            out.insert("hyphenate".into(), json!(h));
        }
    } else if let Some(v) = number("Tracking") {
        out.insert("tracking".into(), json!(v));
    }
    if kind == "paragraph" {
        // Pentool paragraph styles have no fill; keep the colour reference so
        // export can return it and nothing is dropped silently.
        if let Some(c) = prop(node, "FillColor") {
            extra.insert("FillColor".into(), json!(c));
        }
    } else if let Some(c) = imp.color(prop(node, "FillColor").as_deref()) {
        out.insert("fill".into(), json!(c));
    }
    for attr in node.attributes() {
        let name = attr.name();
        if !MAPPED_STYLE_ATTRS.contains(&name) && !IGNORED_STYLE_ATTRS.contains(&name) {
            extra.insert(name.into(), json!(attr.value()));
        }
    }
    if let Some(props) = node.children().find(|c| is(c, "Properties")) {
        for c in props.children().filter(Node::is_element) {
            let name = c.tag_name().name();
            if !MAPPED_STYLE_ATTRS.contains(&name) && !IGNORED_STYLE_ATTRS.contains(&name) {
                extra.insert(
                    name.into(),
                    json!(c.text().unwrap_or_default().trim().to_string()),
                );
            }
        }
    }
    (out, extra)
}

fn read_graphic(imp: &mut Imp, doc: &Document, part: &str) {
    for c in doc.descendants().filter(Node::is_element) {
        let name = c.tag_name().name();
        let me = c.attribute("Self").unwrap_or_default().to_string();
        match name {
            "Color" => {
                let space = c.attribute("Space").unwrap_or_default();
                let value = c.attribute("ColorValue").unwrap_or_default();
                match swatch_hex(space, value) {
                    Some((hex_value, approx)) => {
                        if approx {
                            imp.rep.add(
                                "approximated",
                                part,
                                "Color",
                                Some("ColorValue"),
                                &me,
                                &format!("{space} swatch shown as sRGB {hex_value}; original value kept in extension.idml"),
                                "assign output intents in color management before print",
                            );
                        } else {
                            imp.rep.supported();
                        }
                        imp.swatches.insert(me.clone(), hex_value);
                        imp.swatch_meta.insert(
                            me,
                            json!({
                                "name": c.attribute("Name"), "space": space,
                                "value": value, "model": c.attribute("Model"),
                            }),
                        );
                    }
                    None => imp.rep.add(
                        "unsupported",
                        part,
                        "Color",
                        Some("ColorValue"),
                        &me,
                        "unrecognized color space or component count",
                        "re-save the swatch as RGB, CMYK or LAB",
                    ),
                }
            }
            "Tint" => {
                let base = c.attribute("BaseColor").unwrap_or_default();
                let tint = c.attribute("TintValue").and_then(num).unwrap_or(100.0);
                if let Some(b) = imp.swatches.get(base).cloned() {
                    imp.swatches.insert(me.clone(), mix_with_white(&b, tint));
                    imp.rep.add(
                        "approximated",
                        part,
                        "Tint",
                        None,
                        &me,
                        "tint resolved to a flat sRGB color",
                        "none",
                    );
                } else {
                    imp.carry(part, &me, c, "tint of unknown base color");
                }
            }
            "Gradient" => {
                let first = kids(c, "GradientStop")
                    .next()
                    .and_then(|s| s.attribute("StopColor"))
                    .and_then(|sc| imp.swatches.get(sc).cloned());
                if let Some(color) = first {
                    imp.swatches.insert(me.clone(), color);
                }
                imp.rep.add(
                    "approximated",
                    part,
                    "Gradient",
                    None,
                    &me,
                    "gradient shown as its first stop color",
                    "rebuild the gradient as a Pentool gradient fill",
                );
                imp.carry(part, &me, c, "gradient stops");
            }
            "MixedInk" | "MixedInkGroup" => imp.rep.add(
                "approximated",
                part,
                name,
                None,
                &me,
                "mixed ink approximated to a process color",
                "none",
            ),
            _ => {}
        }
    }
}

fn read_styles(imp: &mut Imp, doc: &Document, part: &str) -> Value {
    let mut out = json!({"paragraph": {}, "character": {}, "object": {}});
    let mut links: Vec<(String, String, String, &str)> = Vec::new();
    for c in doc.descendants().filter(Node::is_element) {
        let kind = match c.tag_name().name() {
            "ParagraphStyle" => "paragraph",
            "CharacterStyle" => "character",
            "ObjectStyle" => "object",
            _ => continue,
        };
        let me = c.attribute("Self").unwrap_or_default().to_string();
        let Some(base_name) = style_name(&me) else {
            continue;
        };
        let name = imp.unique(&format!("style-{kind}"), &base_name);
        let (mut entry, extra) = map_style(imp, &c, kind);
        if let Some(b) = prop(&c, "BasedOn") {
            links.push((kind.into(), name.clone(), b, "based_on"));
        }
        if let Some(n) = c.attribute("NextStyle") {
            links.push((kind.into(), name.clone(), n.to_string(), "next"));
        }
        if kind == "object" {
            entry.retain(|k, _| matches!(k.as_str(), "fill" | "opacity"));
        }
        out[kind][&name] = Value::Object(entry);
        match kind {
            "paragraph" => imp.para_styles.insert(me.clone(), name.clone()),
            "character" => imp.char_styles.insert(me.clone(), name.clone()),
            _ => None,
        };
        if extra.is_empty() {
            imp.rep.supported();
        } else {
            imp.rep.add(
                "carried",
                part,
                c.tag_name().name(),
                None,
                &me,
                &format!(
                    "properties kept under extension.idml.style_properties: {}",
                    extra.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
                "none",
            );
            imp.style_ext
                .insert(format!("{kind}:{name}"), Value::Object(extra));
        }
    }
    for (kind, name, target, field) in links {
        let found = match kind.as_str() {
            "paragraph" => imp.para_styles.get(&target),
            "character" => imp.char_styles.get(&target),
            _ => None,
        };
        if let Some(t) = found.filter(|t| **t != name) {
            out[&kind][&name][field] = json!(t);
        }
    }
    for kind in ["paragraph", "character"] {
        let names: Vec<String> = out[kind]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        for name in names {
            let mut seen = HashSet::from([name.clone()]);
            let mut cursor = name;
            while let Some(base) = out[kind][&cursor]
                .get("based_on")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                if !seen.insert(base.clone()) || seen.len() > 14 {
                    if let Some(o) = out[kind][&cursor].as_object_mut() {
                        o.remove("based_on");
                    }
                    break;
                }
                cursor = base;
            }
        }
    }
    out
}

// ------------------------------------------------------------ spreads

struct PageInfo {
    id: String,
    origin: (f64, f64),
    w: f64,
    h: f64,
}

fn list_items(node: &Node) -> Vec<f64> {
    node.children()
        .filter(|c| is(c, "ListItem"))
        .filter_map(|c| c.text().and_then(num))
        .collect()
}

fn page_for(pages: &[PageInfo], b: BBox) -> (usize, bool) {
    let (cx, cy) = b.center();
    for (i, p) in pages.iter().enumerate() {
        if cx >= p.origin.0 && cx <= p.origin.0 + p.w && cy >= p.origin.1 && cy <= p.origin.1 + p.h
        {
            return (i, true);
        }
    }
    let mut best = (0, f64::MAX);
    for (i, p) in pages.iter().enumerate() {
        let dx = cx - (p.origin.0 + p.w / 2.0);
        let dy = cy - (p.origin.1 + p.h / 2.0);
        let d = dx * dx + dy * dy;
        if d < best.1 {
            best = (i, d);
        }
    }
    (best.0, false)
}

fn item_matrix(item: &Node, parent: &Matrix) -> Matrix {
    compose(&parse_matrix(item.attribute("ItemTransform")), parent)
}

fn item_bbox(item: &Node, parent: &Matrix) -> Option<BBox> {
    let own = item_matrix(item, parent);
    if is(item, "Group") {
        return item
            .children()
            .filter(|c| c.is_element() && (SHAPES.contains(&c.tag_name().name()) || is(c, "Group")))
            .filter_map(|c| item_bbox(&c, &own))
            .reduce(BBox::union);
    }
    let pts: Vec<(f64, f64)> = subpaths(item, &own)
        .iter()
        .flat_map(|(a, _)| a.iter().map(|a| a.p))
        .collect();
    BBox::of(&pts)
}

struct SpreadCtx<'a> {
    pages: &'a [PageInfo],
    layer_ids: &'a HashMap<String, String>,
    default_layer: &'a str,
    part: &'a str,
    /// ID of the spread or master that unplaceable items are carried under.
    owner: &'a str,
    placed: Vec<BTreeMap<String, Vec<Value>>>,
    stories: &'a mut HashMap<String, Vec<FrameLink>>,
}

fn shape_style(imp: &Imp, item: &Node, is_line: bool) -> Value {
    let tint = |c: String, t: Option<&str>| match t.and_then(num) {
        Some(t) if (0.0..100.0).contains(&t) => mix_with_white(&c, t),
        _ => c,
    };
    let fill = if is_line {
        None
    } else {
        imp.color(item.attribute("FillColor"))
            .map(|c| tint(c, item.attribute("FillTint")))
    };
    let stroke = imp
        .color(item.attribute("StrokeColor"))
        .map(|c| tint(c, item.attribute("StrokeTint")));
    let width = item
        .attribute("StrokeWeight")
        .and_then(num)
        .unwrap_or(if is_line { 1.0 } else { 0.0 });
    json!({
        "fill": {"fallback": fill.unwrap_or_else(|| "none".into())},
        "stroke": {"fallback": stroke.unwrap_or_else(|| "none".into())},
        "stroke_width": {"fallback": width},
    })
}

fn frame_inset(pref: Option<Node>) -> [f64; 4] {
    let mut inset = [0.0f64; 4];
    let Some(p) = pref else { return inset };
    let values: Vec<f64> = match p
        .children()
        .find(|c| is(c, "Properties"))
        .and_then(|pp| pp.children().find(|c| is(c, "InsetSpacing")))
    {
        Some(list) => list_items(&list),
        None => p
            .attribute("InsetSpacing")
            .map(|t| t.split_whitespace().filter_map(num).collect())
            .unwrap_or_default(),
    };
    if values.len() == 4 {
        // IDML order: top, left, bottom, right. Pentool: top, right, bottom, left.
        inset = [
            values[0].max(0.0),
            values[3].max(0.0),
            values[2].max(0.0),
            values[1].max(0.0),
        ];
    }
    inset
}

fn convert_item(
    imp: &mut Imp,
    ctx: &mut SpreadCtx,
    item: &Node,
    parent: &Matrix,
    forced_page: Option<usize>,
) {
    let name = item.tag_name().name();
    let me = item.attribute("Self").unwrap_or("item").to_string();
    if !(SHAPES.contains(&name) || name == "Group") {
        if !matches!(
            name,
            "Properties"
                | "Page"
                | "FlattenerPreference"
                | "Guide"
                | "Grid"
                | "GridDataInformation"
        ) {
            imp.carry(
                ctx.part,
                ctx.owner,
                *item,
                "page item with no Pentool equivalent",
            );
            imp.rep.add(
                "unsupported",
                ctx.part,
                name,
                None,
                &me,
                "page item kind is not imported; carried unchanged",
                "recreate the object in Pentool or keep the IDML for export",
            );
        }
        return;
    }
    let m = item_matrix(item, parent);
    let Some(bbox) = item_bbox(item, parent) else {
        imp.rep.add(
            "unsupported",
            ctx.part,
            name,
            None,
            &me,
            "item has no geometry",
            "none",
        );
        return;
    };
    let page = match forced_page {
        Some(p) => p,
        None => {
            let (p, inside) = page_for(ctx.pages, bbox);
            if !inside {
                imp.rep.add(
                    "approximated",
                    ctx.part,
                    name,
                    None,
                    &me,
                    &format!(
                        "item lies off every page; placed on page {}",
                        ctx.pages[p].id
                    ),
                    "move the object onto a page",
                );
            }
            p
        }
    };
    let (ox, oy) = ctx.pages[page].origin;
    let layer = item
        .attribute("ItemLayer")
        .and_then(|l| ctx.layer_ids.get(l))
        .cloned()
        .unwrap_or_else(|| ctx.default_layer.to_string());
    let node = if name == "Group" {
        let mut inner = SpreadCtx {
            pages: ctx.pages,
            layer_ids: ctx.layer_ids,
            default_layer: ctx.default_layer,
            part: ctx.part,
            owner: ctx.owner,
            placed: vec![BTreeMap::new(); ctx.pages.len()],
            stories: &mut *ctx.stories,
        };
        for c in item.children().filter(Node::is_element) {
            convert_item(imp, &mut inner, &c, &m, Some(page));
        }
        let children: Vec<Value> = std::mem::take(&mut inner.placed[page])
            .into_values()
            .flatten()
            .collect();
        let id = imp.unique("node", &me);
        json!({"kind": "group", "id": id, "transform": [1, 0, 0, 1, 0, 0], "children": children})
    } else {
        let paths = subpaths(item, &m);
        let id = imp.unique("node", &me);
        let (w, h) = (round(bbox.x1 - bbox.x0), round(bbox.y1 - bbox.y0));
        let (x, y) = (round(bbox.x0 - ox), round(bbox.y0 - oy));
        let first = paths.first();
        let four = paths.len() == 1 && first.is_some_and(|(a, open)| a.len() == 4 && !open);
        let on = |v: f64, a: f64, b: f64| (v - a).abs() < 1e-6 || (v - b).abs() < 1e-6;
        let straight =
            four && first.is_some_and(|(a, _)| a.iter().all(|p| p.p == p.l && p.p == p.r));
        let corners = straight
            && first.is_some_and(|(a, _)| {
                a.iter()
                    .all(|p| on(p.p.0, bbox.x0, bbox.x1) && on(p.p.1, bbox.y0, bbox.y1))
            });
        let (cx, cy) = bbox.center();
        let mids = four
            && first.is_some_and(|(a, _)| {
                a.iter().all(|p| {
                    ((p.p.0 - cx).abs() < 1e-6 && on(p.p.1, bbox.y0, bbox.y1))
                        || ((p.p.1 - cy).abs() < 1e-6 && on(p.p.0, bbox.x0, bbox.x1))
                })
            });
        if name == "TextFrame" {
            let story = item
                .attribute("ParentStory")
                .map(safe_id)
                .unwrap_or_else(|| "story".into());
            let pref = kids(*item, "TextFramePreference").next();
            let columns = pref
                .and_then(|p| p.attribute("TextColumnCount"))
                .and_then(|c| c.parse::<u64>().ok())
                .filter(|c| (1..=40).contains(c))
                .unwrap_or(1);
            let gutter = pref
                .and_then(|p| p.attribute("TextColumnGutter"))
                .and_then(num)
                .filter(|g| *g >= 0.0)
                .unwrap_or(12.0);
            let inset = frame_inset(pref);
            let next = item
                .attribute("NextTextFrame")
                .filter(|n| *n != "n")
                .map(str::to_string);
            ctx.stories
                .entry(story.clone())
                .or_default()
                .push((id.clone(), next));
            imp.items_ext.insert(
                id.clone(),
                json!({"self": me, "applied_object_style": item.attribute("AppliedObjectStyle")}),
            );
            json!({
                "kind": "text-frame", "id": id, "story": story, "next": null,
                "x": x, "y": y, "width": w, "height": h,
                "columns": columns, "gutter": gutter, "inset": inset.map(round),
            })
        } else if kids(*item, "Image")
            .chain(kids(*item, "PDF"))
            .chain(kids(*item, "EPS"))
            .next()
            .is_some()
        {
            let link = item
                .descendants()
                .find(|d| is(d, "Link"))
                .and_then(|l| l.attribute("LinkResourceURI"))
                .unwrap_or("");
            imp.rep.add(
                "approximated",
                ctx.part,
                name,
                None,
                &me,
                &format!("placed graphic {link:?} is not embedded; a graphic frame was created"),
                "relink the graphic after import",
            );
            imp.items_ext
                .insert(id.clone(), json!({"self": me, "link": link}));
            json!({"kind": "graphic-frame", "id": id, "x": x, "y": y, "width": w, "height": h, "fit": "fit"})
        } else if name == "GraphicLine" && first.is_some_and(|(a, _)| a.len() == 2) {
            let a = &paths[0].0;
            json!({
                "kind": "line", "id": id,
                "x1": round(a[0].p.0 - ox), "y1": round(a[0].p.1 - oy),
                "x2": round(a[1].p.0 - ox), "y2": round(a[1].p.1 - oy),
                "style": shape_style(imp, item, true),
            })
        } else if name == "Rectangle" && corners && axis_aligned(&m) {
            let radius = item
                .attribute("TopLeftCornerRadius")
                .and_then(num)
                .unwrap_or(0.0);
            json!({
                "kind": "rect", "id": id, "x": x, "y": y, "width": w, "height": h,
                "radius_x": radius, "radius_y": radius,
                "style": shape_style(imp, item, false),
            })
        } else if name == "Oval" && mids && axis_aligned(&m) {
            json!({
                "kind": "ellipse", "id": id,
                "cx": round(cx - ox), "cy": round(cy - oy),
                "radius_x": round(w / 2.0), "radius_y": round(h / 2.0),
                "style": shape_style(imp, item, false),
            })
        } else {
            let closed = first.is_some_and(|(_, open)| !open);
            json!({
                "kind": "path", "id": id, "d": path_data(&paths, ox, oy), "closed": closed,
                "style": shape_style(imp, item, name == "GraphicLine"),
            })
        }
    };
    for c in item.children().filter(Node::is_element) {
        let cn = c.tag_name().name();
        if !NO_ITEM_CHILDREN.contains(&cn) && !SHAPES.contains(&cn) && cn != "Group" {
            imp.carry(ctx.part, &me, c, "item effect or option not interpreted");
        }
    }
    imp.rep.supported();
    ctx.placed[page].entry(layer).or_default().push(node);
}

// ------------------------------------------------------------ stories

struct StoryState {
    paragraphs: Vec<Value>,
    cur: Option<(Option<String>, Vec<Value>)>,
    para_style: Option<String>,
    char_style: Option<String>,
    notes: BTreeMap<String, usize>,
}

fn flush(st: &mut StoryState) {
    if let Some((style, runs)) = st.cur.take() {
        let mut p = Map::new();
        if let Some(s) = style {
            p.insert("style".into(), json!(s));
        }
        p.insert("runs".into(), Value::Array(runs));
        st.paragraphs.push(Value::Object(p));
    }
}

fn ensure_para(st: &mut StoryState) -> &mut Vec<Value> {
    let style = st.para_style.clone();
    &mut st.cur.get_or_insert_with(|| (style, Vec::new())).1
}

fn push_text(st: &mut StoryState, text: &str) {
    let style = st.char_style.clone();
    let mut buf = String::new();
    let mut pieces: Vec<Value> = Vec::new();
    let mut removed = 0usize;
    let finish = |buf: &mut String, pieces: &mut Vec<Value>| {
        if buf.is_empty() {
            return;
        }
        let mut run = json!({"text": std::mem::take(buf)});
        if let Some(s) = &style {
            run["style"] = json!(s);
        }
        pieces.push(run);
    };
    for c in text.chars() {
        let special = match c {
            '\t' => Some("tab"),
            '\u{2028}' => Some("line-break"),
            '\u{2003}' => Some("em-space"),
            '\u{2002}' => Some("en-space"),
            '\u{2009}' => Some("thin-space"),
            '\u{200A}' => Some("hair-space"),
            '\u{2011}' => Some("nonbreaking-hyphen"),
            _ => None,
        };
        if let Some(s) = special {
            finish(&mut buf, &mut pieces);
            pieces.push(json!({"special": s}));
        } else if c.is_control() {
            removed += 1;
        } else {
            buf.push(c);
        }
    }
    finish(&mut buf, &mut pieces);
    if removed > 0 {
        *st.notes
            .entry("control characters were removed from text".into())
            .or_default() += removed;
    }
    ensure_para(st).extend(pieces);
}

fn walk_story(imp: &mut Imp, node: Node, st: &mut StoryState, part: &str, owner: &str) {
    for child in node.children() {
        match child.node_type() {
            NodeType::PI => {
                let Some(pi) = child.pi() else { continue };
                let special = match (pi.target, pi.value.map(str::trim)) {
                    ("ACE", Some("18")) => Some("page-number"),
                    ("ACE", Some("19")) => Some("next-page-number"),
                    ("ACE", Some("7")) => Some("right-indent-tab"),
                    _ => None,
                };
                match special {
                    Some(s) => ensure_para(st).push(json!({"special": s})),
                    None => {
                        imp.carry(
                            part,
                            owner,
                            child,
                            "special character not in the documented set",
                        );
                        *st.notes
                            .entry("special characters outside the documented set were carried, not rendered".into())
                            .or_default() += 1;
                    }
                }
            }
            NodeType::Element => match child.tag_name().name() {
                "ParagraphStyleRange" => {
                    if st.cur.as_ref().is_some_and(|(_, r)| !r.is_empty()) {
                        flush(st);
                    }
                    let name = child
                        .attribute("AppliedParagraphStyle")
                        .and_then(|s| imp.para_styles.get(s).cloned());
                    let old = std::mem::replace(&mut st.para_style, name.clone());
                    if let Some(cur) = st.cur.as_mut() {
                        cur.0 = name;
                    }
                    walk_story(imp, child, st, part, owner);
                    st.para_style = old;
                }
                "CharacterStyleRange" => {
                    let name = child
                        .attribute("AppliedCharacterStyle")
                        .and_then(|s| imp.char_styles.get(s).cloned());
                    let overrides = child.attributes().any(|a| {
                        matches!(
                            a.name(),
                            "PointSize" | "FontStyle" | "FillColor" | "Tracking" | "AppliedFont"
                        )
                    });
                    if overrides {
                        *st.notes
                            .entry("inline character overrides were not applied".into())
                            .or_default() += 1;
                    }
                    let old = std::mem::replace(&mut st.char_style, name);
                    walk_story(imp, child, st, part, owner);
                    st.char_style = old;
                }
                "Content" => push_text(st, child.text().unwrap_or_default()),
                "Br" => {
                    ensure_para(st);
                    flush(st);
                }
                "Properties" => {}
                "HyperlinkTextSource" | "CrossReferenceSource" | "XMLElement" => {
                    *st.notes
                        .entry(format!(
                            "{} wrapper dropped; its text was kept",
                            child.tag_name().name()
                        ))
                        .or_default() += 1;
                    walk_story(imp, child, st, part, owner);
                }
                other => {
                    imp.carry(part, owner, child, "story element not interpreted");
                    *st.notes
                        .entry(format!("{other} carried, not rendered in Pentool"))
                        .or_default() += 1;
                }
            },
            _ => {}
        }
    }
}

fn read_story(imp: &mut Imp, story: Node, part: &str, id: &str) -> Value {
    let mut st = StoryState {
        paragraphs: Vec::new(),
        cur: None,
        para_style: None,
        char_style: None,
        notes: BTreeMap::new(),
    };
    walk_story(imp, story, &mut st, part, id);
    flush(&mut st);
    for (message, n) in std::mem::take(&mut st.notes) {
        imp.rep.add(
            "approximated",
            part,
            "Story",
            None,
            id,
            &format!("{message} ({n}x)"),
            "review the story text in Pentool",
        );
    }
    if st.paragraphs.is_empty() {
        st.paragraphs.push(json!({"runs": []}));
    }
    imp.rep.supported();
    json!({"id": id, "language": "und", "paragraphs": st.paragraphs})
}

// ------------------------------------------------------------ entry point

struct DesignRefs {
    spreads: Vec<String>,
    masters: Vec<String>,
    stories: Vec<String>,
    graphics: Vec<String>,
    fonts: Vec<String>,
    styles: Vec<String>,
    others: Vec<(String, String)>,
    layers: Vec<(String, String, bool, bool)>,
    sections: Vec<SectionRef>,
}

struct SectionRef {
    page_start: String,
    style: Option<String>,
    number: Option<u64>,
    prefix: Option<String>,
    cont: bool,
}

fn read_designmap(imp: &mut Imp, root: Node) -> DesignRefs {
    let mut r = DesignRefs {
        spreads: vec![],
        masters: vec![],
        stories: vec![],
        graphics: vec![],
        fonts: vec![],
        styles: vec![],
        others: vec![],
        layers: vec![],
        sections: vec![],
    };
    for c in root.children().filter(Node::is_element) {
        let name = c.tag_name().name();
        let src = c.attribute("src").map(str::to_string);
        match (name, src) {
            ("Spread", Some(s)) => r.spreads.push(s),
            ("MasterSpread", Some(s)) => r.masters.push(s),
            ("Story", Some(s)) => r.stories.push(s),
            ("Graphic", Some(s)) => r.graphics.push(s),
            ("Fonts", Some(s)) => r.fonts.push(s),
            ("Styles", Some(s)) => r.styles.push(s),
            ("Layer", _) => r.layers.push((
                c.attribute("Self").unwrap_or("layer").to_string(),
                c.attribute("Name").unwrap_or("Layer").to_string(),
                c.attribute("Visible").and_then(boolean).unwrap_or(true),
                c.attribute("Locked").and_then(boolean).unwrap_or(false),
            )),
            ("Section", _) => r.sections.push(SectionRef {
                page_start: c.attribute("PageStart").unwrap_or_default().to_string(),
                style: c.attribute("PageNumberStyle").map(str::to_string),
                number: c.attribute("PageNumberStart").and_then(|n| n.parse().ok()),
                prefix: c.attribute("SectionPrefix").map(str::to_string),
                cont: c
                    .attribute("ContinueNumbering")
                    .and_then(boolean)
                    .unwrap_or(false),
            }),
            (_, Some(s)) => r.others.push((name.to_string(), s)),
            ("Language" | "DocumentUser" | "ColorGroup", None) => imp.rep.supported(),
            _ => imp.carry(
                "designmap.xml",
                "Document",
                c,
                "designmap element not interpreted",
            ),
        }
    }
    r
}

fn margin_fields(mp: &Node, target: &mut Map<String, Value>) {
    let g = |k: &str| mp.attribute(k).and_then(num).filter(|v| *v >= 0.0);
    if let (Some(t), Some(r), Some(b), Some(l)) = (g("Top"), g("Right"), g("Bottom"), g("Left")) {
        target.insert("margins".into(), json!([t, r, b, l]));
    }
    if let Some(c) = mp
        .attribute("ColumnCount")
        .and_then(|c| c.parse::<u64>().ok())
        .filter(|c| (1..=40).contains(c))
    {
        target.insert("columns".into(), json!(c));
    }
    if let Some(v) = g("ColumnGutter") {
        target.insert("gutter".into(), json!(v));
    }
}

fn page_bounds(pg: &Node) -> Option<[f64; 4]> {
    let b: Vec<f64> = pg
        .attribute("GeometricBounds")?
        .split_whitespace()
        .filter_map(num)
        .collect();
    (b.len() == 4 && b[2] > b[0] && b[3] > b[1]).then(|| [b[0], b[1], b[2], b[3]])
}

/// Import an IDML package into a v9 document and a fidelity report.
pub fn import(bytes: &[u8], source: &str) -> Result<Imported> {
    let parts = read_package(bytes)?;
    let mimetype = parts.get("mimetype").map(|s| s.trim()).unwrap_or_default();
    ensure!(
        mimetype == MIMETYPE,
        "[malformed-resource] mimetype must be {MIMETYPE:?}; this is not an IDML package"
    );
    let design_text = parts
        .get("designmap.xml")
        .context("[missing-resource] designmap.xml is missing; this is not an IDML package")?;
    let design = parse(design_text, "designmap.xml")?;
    let root = design.root_element();
    ensure!(
        root.tag_name().name() == "Document",
        "[malformed-resource] designmap.xml root must be Document"
    );
    let mut imp = Imp::default();
    let refs = read_designmap(&mut imp, root);

    for src in &refs.graphics {
        if let Some(text) = imp.find_part(&parts, src, "designmap.xml") {
            let doc = parse(text, src)?;
            read_graphic(&mut imp, &doc, src);
        }
    }
    let mut styles = json!({"paragraph": {}, "character": {}, "object": {}});
    for src in &refs.styles {
        if let Some(text) = imp.find_part(&parts, src, "designmap.xml") {
            let doc = parse(text, src)?;
            styles = read_styles(&mut imp, &doc, src);
        }
    }
    let mut font_families = Vec::new();
    let mut missing_fonts = Vec::new();
    for src in &refs.fonts {
        let Some(text) = imp.find_part(&parts, src, "designmap.xml") else {
            continue;
        };
        let doc = parse(text, src)?;
        for fam in doc.descendants().filter(|n| is(n, "FontFamily")) {
            let fname = fam.attribute("Name").unwrap_or_default().to_string();
            font_families.push(fname.clone());
            for f in kids(fam, "Font") {
                let status = f.attribute("Status").unwrap_or("Installed");
                if status == "Installed" {
                    imp.rep.supported();
                    continue;
                }
                let style = f.attribute("FontStyleName").unwrap_or_default();
                missing_fonts.push(format!("{fname} {style}").trim().to_string());
                imp.rep.add(
                    "approximated",
                    src,
                    "Font",
                    Some("Status"),
                    f.attribute("Self").unwrap_or_default(),
                    &format!(
                        "font {fname} {style} is {status} in the source; Pentool will substitute"
                    ),
                    "install the font or choose a replacement before export",
                );
            }
        }
    }
    for (name, src) in &refs.others {
        let Some(text) = imp.find_part(&parts, src, "designmap.xml") else {
            continue;
        };
        parse(text, src)?;
        if imp.carried_bytes + text.len() > MAX_CARRIED_BYTES {
            imp.rep.add(
                "unsupported",
                src,
                name,
                None,
                src,
                "part is too large to carry",
                "export without it",
            );
            continue;
        }
        imp.carried_bytes += text.len();
        imp.carried.push(json!({
            "part": src, "owner": "Document", "element": name,
            "reason": "part not interpreted", "xml": text,
        }));
        *imp.carried_counts
            .entry((src.clone(), name.clone()))
            .or_insert(0) += 1;
        imp.rep.add(
            "carried",
            src,
            name,
            None,
            src,
            "part kept unchanged under extension.idml",
            "none",
        );
    }

    // Layers.
    let mut layer_ids: HashMap<String, String> = HashMap::new();
    let mut layer_defs: Vec<(String, String, bool, bool)> = Vec::new();
    for (me, name, visible, locked) in &refs.layers {
        let id = imp.unique("layer", me);
        layer_ids.insert(me.clone(), id.clone());
        layer_defs.push((id, name.clone(), *visible, *locked));
    }
    if layer_defs.is_empty() {
        layer_defs.push(("content".into(), "Content".into(), true, false));
    }
    let default_layer = layer_defs[0].0.clone();

    // Master spreads become parents; shapes render, text frames are carried.
    let mut parents: Vec<Value> = Vec::new();
    let mut master_ids: HashMap<String, String> = HashMap::new();
    let mut master_bases: Vec<(usize, String)> = Vec::new();
    for src in &refs.masters {
        let Some(text) = imp.find_part(&parts, src, "designmap.xml") else {
            continue;
        };
        let doc = parse(text, src)?;
        for ms in doc
            .descendants()
            .filter(|n| is(n, "MasterSpread") && n.has_attribute("Self"))
        {
            let me = ms.attribute("Self").unwrap_or("master").to_string();
            let id = imp.unique("parent", &me);
            master_ids.insert(me.clone(), id.clone());
            let mut parent = Map::new();
            parent.insert("id".into(), json!(id));
            parent.insert("based_on".into(), Value::Null);
            parent.insert("pages".into(), json!([]));
            let first_page = kids(ms, "Page").next();
            if let Some(mp) = first_page.and_then(|p| kids(p, "MarginPreference").next()) {
                margin_fields(&mp, &mut parent);
            }
            if let Some(b) = prop(&ms, "BasedOn").filter(|b| b != "n") {
                master_bases.push((parents.len(), b));
            }
            let origin = first_page
                .and_then(|p| {
                    let b = page_bounds(&p)?;
                    let t = parse_matrix(p.attribute("ItemTransform"));
                    Some(apply(&t, b[1], b[0]))
                })
                .unwrap_or((0.0, 0.0));
            let infos = [PageInfo {
                id: id.clone(),
                origin,
                w: 1e9,
                h: 1e9,
            }];
            let mut unused: HashMap<String, Vec<FrameLink>> = HashMap::new();
            let mut ctx = SpreadCtx {
                pages: &infos,
                layer_ids: &layer_ids,
                default_layer: &default_layer,
                part: src,
                owner: &id,
                placed: vec![BTreeMap::new()],
                stories: &mut unused,
            };
            let spread_m = parse_matrix(ms.attribute("ItemTransform"));
            for c in ms.children().filter(Node::is_element) {
                let name = c.tag_name().name();
                if name == "TextFrame" {
                    imp.carry(src, &me, c, "master text frame");
                    imp.rep.add(
                        "carried",
                        src,
                        "TextFrame",
                        None,
                        &me,
                        "master text frame (running header or folio) is kept under extension.idml, not rendered",
                        "recreate it on the Pentool parent page",
                    );
                } else if SHAPES.contains(&name) || name == "Group" {
                    convert_item(&mut imp, &mut ctx, &c, &spread_m, Some(0));
                }
            }
            let nodes: Vec<Value> = std::mem::take(&mut ctx.placed[0])
                .into_values()
                .flatten()
                .collect();
            if !nodes.is_empty() {
                parent.insert(
                    "layers".into(),
                    json!([{"id": "content", "name": "Content", "visible": true, "locked": false, "nodes": nodes}]),
                );
            }
            parents.push(Value::Object(parent));
            imp.rep.supported();
        }
    }
    for (index, base) in master_bases {
        if let Some(id) = master_ids.get(&base) {
            if parents[index]["id"].as_str() != Some(id) {
                parents[index]["based_on"] = json!(id);
            }
        }
    }

    // Spreads and pages.
    let mut pen_pages: Vec<Value> = Vec::new();
    let mut spreads_out: Vec<Value> = Vec::new();
    let mut page_selfs: HashMap<String, String> = HashMap::new();
    let mut frames_by_story: HashMap<String, Vec<FrameLink>> = HashMap::new();
    for src in &refs.spreads {
        let Some(text) = imp.find_part(&parts, src, "designmap.xml") else {
            continue;
        };
        let doc = parse(text, src)?;
        for sp in doc
            .descendants()
            .filter(|n| is(n, "Spread") && n.has_attribute("Self"))
        {
            let me = sp.attribute("Self").unwrap_or("spread").to_string();
            let spread_id = imp.unique("spread", &me);
            let spread_m = parse_matrix(sp.attribute("ItemTransform"));
            let mut infos: Vec<PageInfo> = Vec::new();
            let mut meta: Vec<(String, Option<String>, Node)> = Vec::new();
            for pg in kids(sp, "Page") {
                let pself = pg.attribute("Self").unwrap_or("page").to_string();
                let b = page_bounds(&pg).with_context(|| {
                    format!("[malformed-resource] {src}: page {pself} has invalid GeometricBounds")
                })?;
                let t = compose(&parse_matrix(pg.attribute("ItemTransform")), &spread_m);
                let id = imp.unique("page", &pself);
                page_selfs.insert(pself, id.clone());
                let master = pg
                    .attribute("AppliedMaster")
                    .and_then(|m| master_ids.get(m).cloned());
                infos.push(PageInfo {
                    id: id.clone(),
                    origin: apply(&t, b[1], b[0]),
                    w: b[3] - b[1],
                    h: b[2] - b[0],
                });
                meta.push((pg.attribute("Name").unwrap_or(&id).to_string(), master, pg));
            }
            if infos.is_empty() {
                imp.rep.add(
                    "unsupported",
                    src,
                    "Spread",
                    None,
                    &me,
                    "spread has no pages and was skipped",
                    "none",
                );
                continue;
            }
            let mut ctx = SpreadCtx {
                pages: &infos,
                layer_ids: &layer_ids,
                default_layer: &default_layer,
                part: src,
                owner: &spread_id,
                placed: vec![BTreeMap::new(); infos.len()],
                stories: &mut frames_by_story,
            };
            for c in sp.children().filter(Node::is_element) {
                if is(&c, "Page") || is(&c, "FlattenerPreference") {
                    continue;
                }
                convert_item(&mut imp, &mut ctx, &c, &spread_m, None);
            }
            let placed = std::mem::take(&mut ctx.placed);
            for (i, info) in infos.iter().enumerate() {
                let mut layers = Vec::new();
                for (lid, lname, visible, locked) in &layer_defs {
                    let nodes = placed[i].get(lid).cloned().unwrap_or_default();
                    if !nodes.is_empty() || (layers.is_empty() && lid == &default_layer) {
                        layers.push(json!({"id": lid, "name": lname, "visible": visible, "locked": locked, "nodes": nodes}));
                    }
                }
                let (name, master, pg) = &meta[i];
                let mut page = json!({
                    "id": info.id, "name": name,
                    "canvas": {
                        "width": info.w.round().max(1.0),
                        "height": info.h.round().max(1.0),
                        "background": "#ffffff",
                    },
                    "layers": layers,
                });
                if (info.w - info.w.round()).abs() > 1e-6 || (info.h - info.h.round()).abs() > 1e-6
                {
                    imp.rep.add(
                        "approximated",
                        src,
                        "Page",
                        Some("GeometricBounds"),
                        &info.id,
                        &format!(
                            "page size {}x{} rounded to whole points",
                            round(info.w),
                            round(info.h)
                        ),
                        "none",
                    );
                }
                let mut layout = Map::new();
                if let Some(m) = master {
                    layout.insert("parent".into(), json!(m));
                }
                if let Some(mp) = kids(*pg, "MarginPreference").next() {
                    margin_fields(&mp, &mut layout);
                }
                if !layout.is_empty() {
                    page["layout"] = Value::Object(layout);
                }
                pen_pages.push(page);
                imp.rep.supported();
            }
            spreads_out.push(json!({
                "id": spread_id,
                "pages": infos.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
                "facing": infos.len() > 1,
            }));
        }
    }
    ensure!(
        !pen_pages.is_empty(),
        "[malformed-resource] the package contains no pages"
    );

    // Stories.
    let mut stories_out: Vec<Value> = Vec::new();
    let mut seen_stories: HashSet<String> = HashSet::new();
    for src in &refs.stories {
        let Some(text) = imp.find_part(&parts, src, "designmap.xml") else {
            continue;
        };
        let doc = parse(text, src)?;
        for story in doc
            .descendants()
            .filter(|n| is(n, "Story") && n.has_attribute("Self"))
        {
            let me = safe_id(story.attribute("Self").unwrap_or("story"));
            if !seen_stories.insert(me.clone()) {
                imp.rep.add(
                    "unsupported",
                    src,
                    "Story",
                    Some("Self"),
                    &me,
                    "duplicate story ID skipped",
                    "re-export the IDML",
                );
                continue;
            }
            stories_out.push(read_story(&mut imp, story, src, &me));
        }
    }
    let mut frame_story_ids: Vec<String> = frames_by_story.keys().cloned().collect();
    frame_story_ids.sort();
    for sid in &frame_story_ids {
        if seen_stories.insert(sid.clone()) {
            imp.rep.add(
                "approximated",
                "designmap.xml",
                "TextFrame",
                Some("ParentStory"),
                sid,
                "frame names a story with no Story part; an empty story was created",
                "re-export the IDML",
            );
            stories_out.push(json!({"id": sid, "language": "und", "paragraphs": [{"runs": []}]}));
        }
    }

    // Thread frames: follow IDML links when they form one chain, else document order.
    let by_self: HashMap<String, String> = imp
        .items_ext
        .iter()
        .filter_map(|(id, v)| {
            v.get("self")
                .and_then(Value::as_str)
                .map(|s| (s.to_string(), id.clone()))
        })
        .collect();
    let mut next_of: HashMap<String, Option<String>> = HashMap::new();
    for sid in &frame_story_ids {
        let frames = &frames_by_story[sid];
        let ids: Vec<&str> = frames.iter().map(|(id, _)| id.as_str()).collect();
        let mut links: HashMap<&str, Option<&str>> = HashMap::new();
        let mut valid = true;
        for (id, next) in frames {
            let target = match next {
                None => None,
                Some(n) => match by_self.get(n) {
                    Some(t) if ids.contains(&t.as_str()) && t != id => Some(t.as_str()),
                    _ => {
                        valid = false;
                        None
                    }
                },
            };
            links.insert(id.as_str(), target);
        }
        let mut preds: HashSet<&str> = HashSet::new();
        for t in links.values().flatten() {
            valid &= preds.insert(t);
        }
        if valid {
            let heads: Vec<&&str> = ids.iter().filter(|i| !preds.contains(**i)).collect();
            if heads.len() == 1 {
                let (mut n, mut at) = (0, Some(*heads[0]));
                while let Some(cur) = at {
                    n += 1;
                    if n > ids.len() {
                        break;
                    }
                    at = links.get(cur).copied().flatten();
                }
                valid = n == ids.len();
            } else {
                valid = false;
            }
        }
        if valid {
            for (id, t) in &links {
                next_of.insert(id.to_string(), t.map(str::to_string));
            }
        } else {
            if ids.len() > 1 {
                imp.rep.add(
                    "approximated",
                    "Spreads",
                    "TextFrame",
                    Some("NextTextFrame"),
                    sid,
                    "frame links do not form one chain; frames were threaded in document order",
                    "check the thread in Pentool",
                );
            }
            for (i, id) in ids.iter().enumerate() {
                next_of.insert(id.to_string(), ids.get(i + 1).map(|s| s.to_string()));
            }
        }
    }
    fn set_next(nodes: &mut [Value], next_of: &HashMap<String, Option<String>>) {
        for node in nodes {
            if node["kind"] == "text-frame" {
                let id = node["id"].as_str().unwrap_or_default().to_string();
                if let Some(Some(n)) = next_of.get(&id) {
                    node["next"] = json!(n);
                }
            }
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                set_next(children, next_of);
            }
        }
    }
    for page in &mut pen_pages {
        for layer in page["layers"].as_array_mut().into_iter().flatten() {
            if let Some(nodes) = layer["nodes"].as_array_mut() {
                set_next(nodes, &next_of);
            }
        }
    }

    // Sections.
    let mut sections_out = Vec::new();
    let mut starts: HashSet<String> = HashSet::new();
    for (i, s) in refs.sections.iter().enumerate() {
        let Some(page) = page_selfs.get(&s.page_start) else {
            imp.rep.add(
                "unsupported",
                "designmap.xml",
                "Section",
                Some("PageStart"),
                &s.page_start,
                "section starts at an unknown page and was skipped",
                "none",
            );
            continue;
        };
        if !starts.insert(page.clone()) {
            continue;
        }
        let style = match s.style.as_deref() {
            Some("UpperRoman") => "upper-roman",
            Some("LowerRoman") => "lower-roman",
            Some("UpperLetters") => "upper-alpha",
            Some("LowerLetters") => "lower-alpha",
            _ => "arabic",
        };
        let mut sec = json!({
            "id": format!("section-{}", i + 1), "start_page": page,
            "style": style, "continue": s.cont,
        });
        if let Some(n) = s.number.filter(|n| *n >= 1) {
            sec["start_number"] = json!(n);
        }
        if let Some(p) = s.prefix.as_ref().filter(|p| {
            !p.is_empty() && p.chars().count() <= 8 && p.chars().all(|c| (' '..='~').contains(&c))
        }) {
            sec["prefix"] = json!(p);
        }
        sections_out.push(sec);
        imp.rep.supported();
    }

    let name = std::path::Path::new(source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported IDML")
        .to_string();
    let mut layout = json!({
        "engine": 1,
        "spreads": spreads_out,
        "parents": parents,
        "sections": sections_out,
        "stories": stories_out,
    });
    let empty = |v: &Value| v.as_object().is_none_or(Map::is_empty);
    if !(empty(&styles["paragraph"]) && empty(&styles["character"]) && empty(&styles["object"])) {
        if empty(&styles["object"]) {
            if let Some(o) = styles.as_object_mut() {
                o.remove("object");
            }
        }
        layout["styles"] = styles;
    }
    let carried_total = imp.carried.len();
    let carried_summary: Vec<Value> = imp
        .carried_counts
        .iter()
        .map(|((part, element), count)| json!({"part": part, "element": element, "count": count}))
        .collect();
    let mut doc = crate::spreads::migrate(crate::scene::new_document(1, 1))
        .context("[internal-error] could not create a version 9 base document")?;
    doc["name"] = json!(name);
    doc["pages"] = json!(pen_pages);
    doc["layout"] = layout;
    doc["extension"] = json!({"idml": {
        "mapping": 1,
        "source": source,
        "swatches": Value::Object(std::mem::take(&mut imp.swatch_meta)),
        "style_properties": Value::Object(std::mem::take(&mut imp.style_ext)),
        "items": Value::Object(std::mem::take(&mut imp.items_ext)),
        "font_families": font_families,
        "carried": std::mem::take(&mut imp.carried),
    }});
    crate::transaction::validate_value(&doc).context(
        "[malformed-resource] the imported document failed validation; nothing was written",
    )?;
    let extra = json!({
        "pages": doc["pages"].as_array().map_or(0, Vec::len),
        "spreads": doc["layout"]["spreads"].as_array().map_or(0, Vec::len),
        "stories": doc["layout"]["stories"].as_array().map_or(0, Vec::len),
        "parents": doc["layout"]["parents"].as_array().map_or(0, Vec::len),
        "carried_blocks": carried_total,
        "carried_summary": carried_summary,
        "missing_fonts": missing_fonts,
    });
    let report = imp.rep.finish(source, extra);
    Ok(Imported {
        document: doc,
        report,
    })
}

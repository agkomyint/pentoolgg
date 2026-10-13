//! Publishing exports (v0.14.0 task 15): tagged PDF with real text and EPUB 3.
//!
//! Composed lines come from `crate::typeset`, the same engine used for
//! rendering and layout preflight. PDF text is embedded as subset fonts
//! (Type 0, Identity-H, ToUnicode) over a raster of the page rendered without
//! its story text, so it stays selectable and searchable. The structure tree,
//! bookmarks, links and alternative text follow the document's stories and
//! `layout` resources. EPUB is reflowable (stories) or fixed-layout
//! (pre-paginated pages with positioned text).
use crate::shaping::{self, Kerning, ShapeParams};
use crate::typeset::{self, PlacedLine};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;

const MAX_PAGES: usize = 2000;
const DEFAULT_SCALE: f32 = 2.0;

#[derive(Debug, Clone)]
pub struct PdfOptions {
    /// Raster scale of the page background (1.0 = 72 ppi).
    pub scale: f32,
    pub title: Option<String>,
    pub language: Option<String>,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            scale: DEFAULT_SCALE,
            title: None,
            language: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EpubOptions {
    /// Pre-paginated pages with positioned text instead of reflowable stories.
    pub fixed: bool,
    pub title: Option<String>,
    pub language: Option<String>,
    /// Background raster scale for fixed layout.
    pub scale: Option<f32>,
}

fn require_v9(raw: &Value) -> Result<()> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] publishing exports need a version 9 document; run `pentool migrate --target 9` first"
    );
    Ok(())
}

fn page_dims(page: &Value) -> (f64, f64) {
    (
        page["canvas"]["width"].as_f64().unwrap_or(612.0),
        page["canvas"]["height"].as_f64().unwrap_or(792.0),
    )
}

fn language_of(raw: &Value, preferred: &Option<String>) -> String {
    preferred
        .clone()
        .or_else(|| {
            raw.pointer("/layout/stories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|s| {
                    s.get("language")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .unwrap_or_else(|| "en".into())
}

/// Heading level (1-6) implied by a paragraph style ID.
pub fn heading_level(style: Option<&str>) -> Option<u8> {
    let id: String = style?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    if let Some(rest) = id.strip_prefix("heading") {
        return Some(
            rest.parse::<u8>()
                .ok()
                .filter(|n| (1..=6).contains(n))
                .unwrap_or(1),
        );
    }
    if id == "title" || id == "chapter" || id == "chaptertitle" {
        return Some(1);
    }
    if let Some(rest) = id.strip_prefix('h') {
        if rest.len() == 1 {
            return rest.parse::<u8>().ok().filter(|n| (1..=6).contains(n));
        }
    }
    None
}

/// Per paragraph: `(start, end, link id)` character ranges of link runs.
type LinkRanges = HashMap<(String, usize), Vec<(usize, usize, String)>>;

/// Per paragraph character ranges of link runs.
fn link_ranges(raw: &Value) -> LinkRanges {
    let mut out = HashMap::new();
    for story in raw
        .pointer("/layout/stories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let sid = story["id"].as_str().unwrap_or_default().to_string();
        for (pi, paragraph) in story["paragraphs"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let mut offset = 0usize;
            for run in paragraph["runs"].as_array().into_iter().flatten() {
                let len = crate::stories::run_text(raw, run).chars().count();
                if let Some(id) = run.get("link").and_then(Value::as_str) {
                    out.entry((sid.clone(), pi)).or_insert_with(Vec::new).push((
                        offset,
                        offset + len,
                        id.to_string(),
                    ));
                }
                offset += len;
            }
        }
    }
    out
}

fn hex_color(value: &str) -> (f64, f64, f64) {
    let hex = value.trim_start_matches('#');
    let number = if hex.len() >= 6 {
        u32::from_str_radix(&hex[..6], 16).unwrap_or(0)
    } else {
        0
    };
    (
        f64::from((number >> 16) & 255) / 255.0,
        f64::from((number >> 8) & 255) / 255.0,
        f64::from(number & 255) / 255.0,
    )
}

fn utf16_hex(text: &str) -> String {
    let mut out = String::from("FEFF");
    for unit in text.encode_utf16() {
        out.push_str(&format!("{unit:04X}"));
    }
    out
}

fn pdf_string(text: &str) -> String {
    format!("<{}>", utf16_hex(text))
}

fn deflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

fn stream(dict: &str, data: &[u8]) -> Result<Vec<u8>> {
    let packed = deflate(data)?;
    let mut out = format!(
        "<< {dict} /Filter /FlateDecode /Length {} >>\nstream\n",
        packed.len()
    )
    .into_bytes();
    out.extend_from_slice(&packed);
    out.extend_from_slice(b"\nendstream");
    Ok(out)
}

struct Objects(Vec<Vec<u8>>);

impl Objects {
    fn reserve(&mut self) -> usize {
        self.0.push(Vec::new());
        self.0.len()
    }
    fn set(&mut self, id: usize, bytes: Vec<u8>) {
        self.0[id - 1] = bytes;
    }
    fn add(&mut self, bytes: Vec<u8>) -> usize {
        self.0.push(bytes);
        self.0.len()
    }
}

struct ShapedLine {
    glyphs: Vec<(u32, i64, usize)>,
    chars: Vec<char>,
}

struct FontUse {
    gids: BTreeSet<u32>,
    unicode: BTreeMap<u32, String>,
}

/// Role, first page, and `(page, mcid)` marks of one structure element.
type StructElem = (String, usize, Vec<(usize, usize)>);

struct Heading {
    level: u8,
    title: String,
    page: usize,
    y: f64,
}

/// Export a version 9 document as a tagged PDF with real text.
pub fn export_pdf(
    raw: &Value,
    document: &Path,
    output: &Path,
    options: &PdfOptions,
) -> Result<Value> {
    require_v9(raw)?;
    ensure!(
        options.scale.is_finite() && (0.25..=8.0).contains(&options.scale),
        "[invalid-input] --scale must be between 0.25 and 8"
    );
    let pages = raw["pages"].as_array().cloned().unwrap_or_default();
    ensure!(
        !pages.is_empty(),
        "[invalid-input] the document has no pages"
    );
    ensure!(
        pages.len() <= MAX_PAGES,
        "[limit-exceeded] at most {MAX_PAGES} pages can be exported"
    );
    let set = typeset::typeset_cached(raw)?;
    let shown = typeset::shown_frames(raw);
    let language = language_of(raw, &options.language);

    // Shape every visible line and collect glyph usage per font.
    let mut shaped: HashMap<usize, ShapedLine> = HashMap::new();
    let mut uses: BTreeMap<usize, FontUse> = BTreeMap::new();
    for (index, line) in set.lines.iter().enumerate() {
        if line.text.trim().is_empty() || !shown.contains(&line.frame) {
            continue;
        }
        let font = &set.fonts[line.font].font;
        let params = ShapeParams {
            size_pt: line.size,
            kerning: Kerning::Metrics,
            ..ShapeParams::default()
        };
        let result = shaping::shape(font, &line.text, &params)
            .with_context(|| format!("[shape-failed] frame {} story {}", line.frame, line.story))?;
        let chars: Vec<char> = line.text.chars().collect();
        let usage = uses.entry(line.font).or_insert_with(|| FontUse {
            gids: BTreeSet::new(),
            unicode: BTreeMap::new(),
        });
        let mut glyphs = Vec::new();
        for run in &result.runs {
            for (g, glyph) in run.glyphs.iter().enumerate() {
                let next = run
                    .glyphs
                    .get(g + 1)
                    .map_or(run.chars.end, |n| n.cluster.max(glyph.cluster + 1));
                let end = next.min(chars.len()).max(glyph.cluster.min(chars.len()));
                let from = glyph.cluster.min(chars.len());
                let text: String = chars[from..end].iter().collect();
                usage.gids.insert(glyph.id);
                usage.unicode.entry(glyph.id).or_insert(text);
                glyphs.push((glyph.id, glyph.x_advance, glyph.cluster));
            }
        }
        shaped.insert(index, ShapedLine { glyphs, chars });
    }

    // Objects: 1 catalog, 2 pages, then fonts, then per-page objects.
    let mut objs = Objects(Vec::new());
    let catalog = objs.reserve();
    let pages_id = objs.reserve();

    // Embedded font subsets.
    let mut font_ref: BTreeMap<usize, (usize, HashMap<u32, u16>)> = BTreeMap::new();
    let mut font_report = Vec::new();
    for (slot, usage) in &uses {
        let font = &set.fonts[*slot].font;
        let gids: Vec<u32> = usage.gids.iter().copied().collect();
        let subset = shaping::subset(font, &gids)?;
        let map: HashMap<u32, u16> = subset.mapping.iter().copied().collect();
        let face = rustybuzz::ttf_parser::Face::parse(&font.bytes, font.index)
            .map_err(|e| anyhow::anyhow!("[font-unavailable] {e:?}"))?;
        let upem = f64::from(face.units_per_em());
        let tag: String = {
            let digest = Sha256::digest(font.sha256.as_bytes());
            digest
                .iter()
                .take(6)
                .map(|b| char::from(b'A' + b % 26))
                .collect()
        };
        let family: String = font
            .family
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let base = format!("{tag}+{}", if family.is_empty() { "Font" } else { &family });
        let cff = font.bytes.starts_with(b"OTTO");
        let file_id = objs.add(if cff {
            stream("/Subtype /OpenType", &subset.bytes)?
        } else {
            stream(&format!("/Length1 {}", subset.bytes.len()), &subset.bytes)?
        });
        let bbox = face.global_bounding_box();
        let to1000 = |v: i16| (f64::from(v) * 1000.0 / upem).round() as i64;
        let ascent = to1000(face.ascender());
        let descent = to1000(face.descender());
        let cap = face.capital_height().map_or(ascent, to1000);
        let descriptor = objs.add(
            format!(
                "<< /Type /FontDescriptor /FontName /{base} /Flags 4 /FontBBox [{} {} {} {}] /ItalicAngle {} /Ascent {ascent} /Descent {descent} /CapHeight {cap} /StemV 80 /{} {file_id} 0 R >>",
                to1000(bbox.x_min),
                to1000(bbox.y_min),
                to1000(bbox.x_max),
                to1000(bbox.y_max),
                if font.italic { -12 } else { 0 },
                if cff { "FontFile3" } else { "FontFile2" },
            )
            .into_bytes(),
        );
        let mut widths = String::new();
        let mut ordered: Vec<(u16, u32)> = subset.mapping.iter().map(|(o, n)| (*n, *o)).collect();
        ordered.sort_unstable();
        for (new, old) in &ordered {
            let advance = face
                .glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(*old as u16))
                .unwrap_or(0);
            widths.push_str(&format!(
                "{new} [{}] ",
                (f64::from(advance) * 1000.0 / upem).round() as i64
            ));
        }
        let cid = objs.add(
            format!(
                "<< /Type /Font /Subtype /{} /BaseFont /{base} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {descriptor} 0 R /W [{widths}]{} >>",
                if cff { "CIDFontType0" } else { "CIDFontType2" },
                if cff { "" } else { " /CIDToGIDMap /Identity" },
            )
            .into_bytes(),
        );
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def /CMapType 2 def\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n",
        );
        let entries: Vec<(u16, &String)> = usage
            .unicode
            .iter()
            .filter_map(|(old, text)| map.get(old).map(|n| (*n, text)))
            .filter(|(_, t)| !t.is_empty())
            .collect();
        for chunk in entries.chunks(100) {
            cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (new, text) in chunk {
                let utf16: String = text.encode_utf16().map(|u| format!("{u:04X}")).collect();
                cmap.push_str(&format!("<{new:04X}> <{utf16}>\n"));
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap CMapName currentdict /CMap defineresource pop end end\n");
        let to_unicode = objs.add(stream("", cmap.as_bytes())?);
        let type0 = objs.add(
            format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{base} /Encoding /Identity-H /DescendantFonts [{cid} 0 R] /ToUnicode {to_unicode} 0 R >>"
            )
            .into_bytes(),
        );
        font_report.push(json!({
            "family": font.family, "weight": font.weight, "italic": font.italic,
            "source": font.source, "glyphs": gids.len(), "subset_bytes": subset.bytes.len(),
            "substituted": font.substituted,
        }));
        font_ref.insert(*slot, (type0, map));
    }

    // Reading order, paragraphs, headings and links.
    let ranges = link_ranges(raw);
    let hyperlinks: HashMap<String, Value> = raw
        .pointer("/layout/hyperlinks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|h| Some((h.get("id")?.as_str()?.to_string(), h.clone())))
        .collect();
    let bookmarks: HashMap<String, Value> = raw
        .pointer("/layout/bookmarks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| Some((b.get("id")?.as_str()?.to_string(), b.clone())))
        .collect();
    let page_index: HashMap<String, usize> = pages
        .iter()
        .enumerate()
        .filter_map(|(i, p)| Some((p.get("id")?.as_str()?.to_string(), i)))
        .collect();
    // First line of each paragraph, for bookmark and heading destinations.
    let mut first_line: HashMap<(String, usize), &PlacedLine> = HashMap::new();
    for line in &set.lines {
        first_line
            .entry((line.story.clone(), line.paragraph))
            .or_insert(line);
    }
    let destination = |target: &Value| -> Option<(usize, f64)> {
        if let Some(p) = target.get("page").and_then(Value::as_str) {
            return page_index.get(p).map(|i| (*i, 0.0));
        }
        let story = target.get("story").and_then(Value::as_str)?;
        let paragraph = target.get("paragraph").and_then(Value::as_u64)? as usize;
        first_line
            .get(&(story.to_string(), paragraph))
            .map(|l| (l.page, l.baseline - l.size))
    };

    // Page objects are reserved first so destinations can reference them.
    let page_ids: Vec<usize> = pages.iter().map(|_| objs.reserve()).collect();
    let mut structure: Vec<StructElem> = Vec::new();
    let mut struct_at: HashMap<(String, usize), usize> = HashMap::new();
    let mut headings: Vec<Heading> = Vec::new();
    let mut link_count = 0usize;
    let mut mcids_per_page: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    let mut page_streams: Vec<Vec<u8>> = Vec::new();
    let mut page_annots: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    let mut page_fonts: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); pages.len()];
    let mut paragraph_text: HashMap<(String, usize), String> = HashMap::new();
    for line in &set.lines {
        if !shown.contains(&line.frame) {
            continue;
        }
        let entry = paragraph_text
            .entry((line.story.clone(), line.paragraph))
            .or_default();
        if !entry.is_empty() {
            entry.push(' ');
        }
        entry.push_str(line.text.trim());
    }

    let stripped = typeset::without_frames(raw);
    let mut raster_ids = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let (width, height) = page_dims(page);
        let id = page["id"].as_str().unwrap_or_default();
        let rgba = crate::composite::render(&stripped, document, Some(id), options.scale)
            .with_context(|| format!("[render-failed] page {id}"))?;
        let (iw, ih) = rgba.dimensions();
        let mut rgb = Vec::with_capacity(iw as usize * ih as usize * 3);
        for pixel in rgba.pixels() {
            let alpha = u16::from(pixel[3]);
            for channel in &pixel.0[..3] {
                rgb.push(((u16::from(*channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8);
            }
        }
        raster_ids.push(objs.add(stream(
            &format!(
                "/Type /XObject /Subtype /Image /Width {iw} /Height {ih} /ColorSpace /DeviceRGB /BitsPerComponent 8"
            ),
            &rgb,
        )?));

        let mut content = format!("q {width} 0 0 {height} 0 0 cm /Im0 Do Q\n");
        for (line_index, line) in set.lines.iter().enumerate() {
            if line.page != index {
                continue;
            }
            let Some(sl) = shaped.get(&line_index) else {
                continue;
            };
            let Some((_, map)) = font_ref.get(&line.font) else {
                continue;
            };
            page_fonts[index].insert(line.font);
            let font = &set.fonts[line.font].font;
            let face = rustybuzz::ttf_parser::Face::parse(&font.bytes, font.index)
                .map_err(|e| anyhow::anyhow!("[font-unavailable] {e:?}"))?;
            let upem = f64::from(face.units_per_em());
            let spaces = sl.chars.iter().filter(|c| **c == ' ').count();
            let extra = if line.justified && spaces > 0 {
                ((line.width - line.natural) / spaces as f64).clamp(-line.size, line.size * 4.0)
            } else {
                0.0
            };
            let mut array = String::new();
            for (gid, advance, cluster) in &sl.glyphs {
                let Some(new) = map.get(gid) else { continue };
                array.push_str(&format!("<{new:04X}>"));
                let natural = f64::from(
                    face.glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(*gid as u16))
                        .unwrap_or(0),
                ) * 1000.0
                    / upem;
                let placed = *advance as f64 * 1000.0 / (line.size * shaping::LU_PER_PT as f64);
                let mut adjust = natural - placed;
                if sl.chars.get(*cluster) == Some(&' ') && extra != 0.0 {
                    adjust -= extra * 1000.0 / line.size;
                }
                if adjust.abs() >= 0.5 {
                    array.push_str(&format!("{adjust:.1}"));
                }
            }
            let mcid = mcids_per_page[index].len();
            let key = (line.story.clone(), line.paragraph);
            let elem = *struct_at.entry(key.clone()).or_insert_with(|| {
                let role = heading_level(line.style.as_deref())
                    .map_or_else(|| "P".to_string(), |n| format!("H{n}"));
                structure.push((role, index, Vec::new()));
                structure.len() - 1
            });
            structure[elem].2.push((index, mcid));
            mcids_per_page[index].push(elem);
            if line.first_in_paragraph {
                if let Some(level) = heading_level(line.style.as_deref()) {
                    headings.push(Heading {
                        level,
                        title: paragraph_text.get(&key).cloned().unwrap_or_default(),
                        page: index,
                        y: line.baseline - line.size,
                    });
                }
            }
            let (r, g, b) = hex_color(&line.fill);
            let y = height - line.baseline;
            content.push_str(&format!(
                "/{} <</MCID {mcid}>> BDC BT /F{} {} Tf {r:.3} {g:.3} {b:.3} rg 1 0 0 1 {:.3} {y:.3} Tm [{array}] TJ ET EMC\n",
                if structure[elem].0 == "P" { "P".to_string() } else { structure[elem].0.clone() },
                line.font,
                line.size,
                line.x,
            ));
            // Link annotations covering the linked part of this paragraph line.
            if let Some(list) = ranges.get(&key) {
                let total = line.end.saturating_sub(line.start).max(1) as f64;
                for (from, to, link_id) in list {
                    let a = (*from).max(line.start);
                    let z = (*to).min(line.end);
                    if a >= z {
                        continue;
                    }
                    let Some(link) = hyperlinks.get(link_id) else {
                        continue;
                    };
                    let x1 = line.x + line.width * (a - line.start) as f64 / total;
                    let x2 = line.x + line.width * (z - line.start) as f64 / total;
                    let rect = format!(
                        "/Rect [{x1:.2} {:.2} {x2:.2} {:.2}]",
                        height - line.baseline - line.size * 0.25,
                        height - line.baseline + line.size * 0.8
                    );
                    let action = if let Some(url) = link.get("url").and_then(Value::as_str) {
                        let safe: String = url
                            .chars()
                            .filter(|c| c.is_ascii() && !c.is_control())
                            .flat_map(|c| match c {
                                '(' | ')' | '\\' => vec!['\\', c],
                                other => vec![other],
                            })
                            .collect();
                        Some(format!("/A << /S /URI /URI ({safe}) >>"))
                    } else {
                        let target = link
                            .get("bookmark")
                            .and_then(Value::as_str)
                            .and_then(|b| bookmarks.get(b))
                            .cloned()
                            .unwrap_or_else(|| link.clone());
                        destination(&target).map(|(p, y)| {
                            format!(
                                "/Dest [{} 0 R /XYZ 0 {:.2} null]",
                                page_ids[p],
                                (pages[p]["canvas"]["height"].as_f64().unwrap_or(792.0) - y)
                                    .max(0.0)
                            )
                        })
                    };
                    if let Some(action) = action {
                        let annot = objs.add(
                            format!(
                                "<< /Type /Annot /Subtype /Link {rect} /Border [0 0 0] {action} >>"
                            )
                            .into_bytes(),
                        );
                        page_annots[index].push(annot);
                        link_count += 1;
                    }
                }
            }
        }
        page_streams.push(stream("", content.as_bytes())?);
    }

    // Figures with alternative text.
    let mut figures: Vec<(usize, String, [f64; 4])> = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let (_, height) = page_dims(page);
        let mut stack: Vec<&Value> = Vec::new();
        for layer in page["layers"].as_array().into_iter().flatten() {
            stack.extend(layer["nodes"].as_array().into_iter().flatten());
        }
        while let Some(node) = stack.pop() {
            if node["kind"] == "group" {
                stack.extend(node["children"].as_array().into_iter().flatten());
                continue;
            }
            let alt = ["alt", "alt_text", "description"]
                .iter()
                .find_map(|k| node.get(*k).and_then(Value::as_str))
                .filter(|s| !s.trim().is_empty());
            if let (Some(alt), Some(x), Some(y), Some(w), Some(h)) = (
                alt,
                node["x"].as_f64(),
                node["y"].as_f64(),
                node["width"].as_f64(),
                node["height"].as_f64(),
            ) {
                figures.push((
                    index,
                    alt.to_string(),
                    [x, height - y - h, x + w, height - y],
                ));
            }
        }
    }

    // Structure tree.
    let root = objs.reserve();
    let doc_elem = objs.reserve();
    let mut elem_ids = Vec::new();
    for (role, first_page, marks) in &structure {
        let id = objs.reserve();
        elem_ids.push(id);
        let kids: String = marks
            .iter()
            .map(|(p, m)| format!("<< /Type /MCR /Pg {} 0 R /MCID {m} >>", page_ids[*p]))
            .collect::<Vec<_>>()
            .join(" ");
        objs.set(
            id,
            format!(
                "<< /Type /StructElem /S /{role} /P {doc_elem} 0 R /Pg {} 0 R /K [{kids}] >>",
                page_ids[*first_page]
            )
            .into_bytes(),
        );
    }
    let mut figure_ids = Vec::new();
    for (page, alt, bbox) in &figures {
        figure_ids.push(objs.add(
            format!(
                "<< /Type /StructElem /S /Figure /P {doc_elem} 0 R /Pg {} 0 R /Alt {} /A << /O /Layout /BBox [{:.2} {:.2} {:.2} {:.2}] >> >>",
                page_ids[*page],
                pdf_string(alt),
                bbox[0], bbox[1], bbox[2], bbox[3]
            )
            .into_bytes(),
        ));
    }
    let all: Vec<String> = elem_ids
        .iter()
        .chain(figure_ids.iter())
        .map(|i| format!("{i} 0 R"))
        .collect();
    objs.set(
        doc_elem,
        format!(
            "<< /Type /StructElem /S /Document /P {root} 0 R /K [{}] >>",
            all.join(" ")
        )
        .into_bytes(),
    );
    let nums: String = mcids_per_page
        .iter()
        .enumerate()
        .map(|(p, list)| {
            let refs: Vec<String> = list
                .iter()
                .map(|e| format!("{} 0 R", elem_ids[*e]))
                .collect();
            format!("{p} [{}]", refs.join(" "))
        })
        .collect::<Vec<_>>()
        .join(" ");
    objs.set(
        root,
        format!("<< /Type /StructTreeRoot /K {doc_elem} 0 R /ParentTree << /Nums [{nums}] >> >>")
            .into_bytes(),
    );

    // Outlines from headings.
    let outlines = if headings.is_empty() {
        None
    } else {
        let outline_root = objs.reserve();
        let base = objs.0.len() + 1;
        let mut parent: Vec<Option<usize>> = Vec::new();
        let mut stack: Vec<(u8, usize)> = Vec::new();
        for (i, h) in headings.iter().enumerate() {
            while stack.last().is_some_and(|(level, _)| *level >= h.level) {
                stack.pop();
            }
            parent.push(stack.last().map(|(_, idx)| *idx));
            stack.push((h.level, i));
        }
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); headings.len()];
        let mut top = Vec::new();
        for (i, p) in parent.iter().enumerate() {
            match p {
                Some(p) => children[*p].push(i),
                None => top.push(i),
            }
        }
        for _ in &headings {
            objs.reserve();
        }
        let sibling = |list: &[usize], at: usize| -> (Option<usize>, Option<usize>) {
            let pos = list.iter().position(|v| *v == at).unwrap_or(0);
            (
                pos.checked_sub(1).map(|p| list[p]),
                list.get(pos + 1).copied(),
            )
        };
        for (i, h) in headings.iter().enumerate() {
            let siblings: &[usize] = match parent[i] {
                Some(p) => &children[p],
                None => &top,
            };
            let (prev, next) = sibling(siblings, i);
            let page_height = page_dims(&pages[h.page]).1;
            let mut dict = format!(
                "<< /Title {} /Parent {} 0 R /Dest [{} 0 R /XYZ 0 {:.2} null]",
                pdf_string(if h.title.is_empty() {
                    "Untitled"
                } else {
                    &h.title
                }),
                parent[i].map_or(outline_root, |p| base + p),
                page_ids[h.page],
                (page_height - h.y).max(0.0)
            );
            if let Some(p) = prev {
                dict.push_str(&format!(" /Prev {} 0 R", base + p));
            }
            if let Some(n) = next {
                dict.push_str(&format!(" /Next {} 0 R", base + n));
            }
            if let (Some(first), Some(last)) = (children[i].first(), children[i].last()) {
                dict.push_str(&format!(
                    " /First {} 0 R /Last {} 0 R /Count {}",
                    base + first,
                    base + last,
                    children[i].len()
                ));
            }
            dict.push_str(" >>");
            objs.set(base + i, dict.into_bytes());
        }
        objs.set(
            outline_root,
            format!(
                "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
                base + top[0],
                base + top[top.len() - 1],
                top.len()
            )
            .into_bytes(),
        );
        Some(outline_root)
    };

    // Pages.
    for (index, page) in pages.iter().enumerate() {
        let (width, height) = page_dims(page);
        let content = objs.add(std::mem::take(&mut page_streams[index]));
        let fonts: String = page_fonts[index]
            .iter()
            .filter_map(|slot| {
                font_ref
                    .get(slot)
                    .map(|(id, _)| format!("/F{slot} {id} 0 R"))
            })
            .collect::<Vec<_>>()
            .join(" ");
        let annots = if page_annots[index].is_empty() {
            String::new()
        } else {
            format!(
                " /Annots [{}]",
                page_annots[index]
                    .iter()
                    .map(|a| format!("{a} 0 R"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        objs.set(
            page_ids[index],
            format!(
                "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {width} {height}] /Resources << /XObject << /Im0 {} 0 R >> /Font << {fonts} >> >> /Contents {content} 0 R /StructParents {index} /Tabs /S{annots} >>",
                raster_ids[index]
            )
            .into_bytes(),
        );
    }
    objs.set(
        pages_id,
        format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            pages.len(),
            page_ids
                .iter()
                .map(|i| format!("{i} 0 R"))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .into_bytes(),
    );
    let title = options
        .title
        .clone()
        .or_else(|| raw["name"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Untitled".into());
    let info =
        objs.add(format!("<< /Title {} /Producer (pentool) >>", pdf_string(&title)).into_bytes());
    objs.set(
        catalog,
        format!(
            "<< /Type /Catalog /Pages {pages_id} 0 R /MarkInfo << /Marked true >> /StructTreeRoot {root} 0 R /Lang ({}) /ViewerPreferences << /DisplayDocTitle true >>{} >>",
            language.replace(['(', ')', '\\'], ""),
            outlines.map_or(String::new(), |o| format!(" /Outlines {o} 0 R /PageMode /UseOutlines")),
        )
        .into_bytes(),
    );
    crate::pdf::write_objects_info(objs.0, Some(info), output)?;
    Ok(json!({
        "operation": "layout-export",
        "format": "pdf",
        "output": output,
        "pages": pages.len(),
        "lines": set.lines.len(),
        "fonts": font_report,
        "tagged": true,
        "language": language,
        "structure_elements": structure.len(),
        "figures": figures.len(),
        "bookmarks": headings.len(),
        "links": link_count,
        "overset_stories": set.overset.len(),
        "substituted_fonts": set.substituted.iter().map(|(f, r)| json!({"font": f, "reason": r})).collect::<Vec<_>>(),
        "limits": [
            "page artwork is a raster of the page without story text",
            "right-to-left and complex-script positioning use shaped advances only",
            "PDF/X conformance is not claimed; use `pentool export-pdfx` for print"
        ],
    }))
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn slug(text: &str) -> String {
    let s: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if s.is_empty() {
        "x".into()
    } else {
        s
    }
}

fn run_html(raw: &Value, run: &Value, links: &HashMap<String, Value>) -> String {
    if let Some(text) = run.get("text").and_then(Value::as_str) {
        let mut style = String::new();
        if let Some(o) = run.get("overrides").and_then(Value::as_object) {
            if o.get("italic").and_then(Value::as_bool) == Some(true) {
                style.push_str("font-style:italic;");
            }
            if o.get("weight")
                .and_then(Value::as_u64)
                .is_some_and(|w| w >= 600)
            {
                style.push_str("font-weight:700;");
            }
            if let Some(fill) = o.get("fill").and_then(Value::as_str) {
                if fill.starts_with('#')
                    && fill.len() <= 9
                    && fill[1..].chars().all(|c| c.is_ascii_hexdigit())
                {
                    style.push_str(&format!("color:{fill};"));
                }
            }
        }
        let body = xml(text);
        return if style.is_empty() {
            body
        } else {
            format!("<span style=\"{style}\">{body}</span>")
        };
    }
    if run.get("special").and_then(Value::as_str) == Some("line-break") {
        return "<br/>".into();
    }
    if let Some(id) = run.get("link").and_then(Value::as_str) {
        let inner: String = run["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| run_html(raw, r, links))
            .collect();
        if let Some(url) = links
            .get(id)
            .and_then(|l| l.get("url"))
            .and_then(Value::as_str)
        {
            return format!("<a href=\"{}\">{inner}</a>", xml(url));
        }
        return inner;
    }
    xml(&crate::stories::run_text(raw, run))
}

fn css_for(raw: &Value, ids: &BTreeSet<String>) -> String {
    let mut css = String::from(
        "body{margin:5%;line-height:1.3}\np{margin:0 0 0.6em 0}\nh1,h2,h3,h4,h5,h6{margin:1.2em 0 0.5em 0}\n",
    );
    for id in ids {
        let Ok(style) = crate::styles::resolve(raw, "paragraph", id) else {
            continue;
        };
        let mut rules = String::new();
        if let Some(f) = style.get("font").and_then(Value::as_str) {
            rules.push_str(&format!("font-family:\"{}\",serif;", f.replace('"', "")));
        }
        if let Some(s) = style.get("size").and_then(Value::as_f64) {
            if s > 0.0 && s < 1000.0 {
                rules.push_str(&format!("font-size:{s}pt;"));
            }
        }
        if let Some(w) = style.get("weight").and_then(Value::as_u64) {
            rules.push_str(&format!("font-weight:{w};"));
        }
        if style.get("italic").and_then(Value::as_bool) == Some(true) {
            rules.push_str("font-style:italic;");
        }
        if let Some(a) = style.get("align").and_then(Value::as_str) {
            if matches!(a, "left" | "right" | "center" | "justify") {
                rules.push_str(&format!("text-align:{a};"));
            }
        }
        if let Some(v) = style.get("space_before").and_then(Value::as_f64) {
            rules.push_str(&format!("margin-top:{v}pt;"));
        }
        if let Some(v) = style.get("space_after").and_then(Value::as_f64) {
            rules.push_str(&format!("margin-bottom:{v}pt;"));
        }
        if !rules.is_empty() {
            css.push_str(&format!(".s-{}{{{rules}}}\n", slug(id)));
        }
    }
    css
}

fn xhtml(title: &str, language: &str, css: &str, head: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{l}\" xml:lang=\"{l}\">\n<head><meta charset=\"utf-8\"/>{head}<title>{t}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"{css}\"/></head>\n<body>\n{body}\n</body>\n</html>\n",
        l = xml(language),
        t = xml(title),
    )
}

/// Export version 9 documents (a book's members, in order) as EPUB 3.
pub fn export_epub(
    docs: &[(&Value, &Path)],
    output: &Path,
    options: &EpubOptions,
) -> Result<Value> {
    ensure!(!docs.is_empty(), "[invalid-input] nothing to export");
    for (raw, _) in docs {
        require_v9(raw)?;
    }
    let language = language_of(docs[0].0, &options.language);
    let title = options
        .title
        .clone()
        .or_else(|| docs[0].0["name"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Untitled".into());
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut manifest: Vec<(String, String, String, String)> = Vec::new(); // id, href, media, props
    let mut spine: Vec<String> = Vec::new();
    let mut nav: Vec<(u8, String, String)> = Vec::new();
    let mut css_ids = BTreeSet::new();
    let mut paragraphs_total = 0usize;
    let mut unplaced = 0usize;
    let mut font_files: BTreeMap<String, (String, Vec<u8>)> = BTreeMap::new();
    let mut overset_total = 0usize;

    for (n, (raw, document)) in docs.iter().enumerate() {
        let links: HashMap<String, Value> = raw
            .pointer("/layout/hyperlinks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|h| Some((h.get("id")?.as_str()?.to_string(), h.clone())))
            .collect();
        let set = typeset::typeset_cached(raw)?;
        overset_total += set.overset.len();
        for f in &set.fonts {
            let name = format!(
                "fonts/{}-{}.ttf",
                slug(&f.font.family),
                &f.font.sha256[..8.min(f.font.sha256.len())]
            );
            font_files
                .entry(name)
                .or_insert_with(|| (f.font.family.clone(), f.font.bytes.clone()));
        }
        if options.fixed {
            let stripped = typeset::without_frames(raw);
            let scale = options.scale.unwrap_or(DEFAULT_SCALE);
            ensure!(
                scale.is_finite() && (0.25..=8.0).contains(&scale),
                "[invalid-input] --scale must be between 0.25 and 8"
            );
            let shown = typeset::shown_frames(raw);
            for (index, page) in raw["pages"].as_array().into_iter().flatten().enumerate() {
                let (w, h) = page_dims(page);
                let id = page["id"].as_str().unwrap_or_default();
                let png = crate::composite::png(&stripped, document, Some(id), scale)?;
                let image_name = format!("images/d{n}-p{index}.png");
                files.push((format!("OEBPS/{image_name}"), png));
                manifest.push((
                    format!("img-{n}-{index}"),
                    image_name.clone(),
                    "image/png".into(),
                    String::new(),
                ));
                let mut body = format!(
                    "<div class=\"page\" style=\"width:{w}px;height:{h}px\"><img src=\"{image_name}\" alt=\"\" style=\"width:{w}px;height:{h}px\"/>"
                );
                for line in set.page_lines(index) {
                    if line.text.trim().is_empty() || !shown.contains(&line.frame) {
                        continue;
                    }
                    let font = &set.fonts[line.font].font;
                    let spacing = if line.justified && line.text.chars().count() > 1 {
                        (line.width - line.natural) / (line.text.chars().count() - 1) as f64
                    } else {
                        0.0
                    };
                    body.push_str(&format!(
                        "<p class=\"l\" style=\"left:{:.2}px;top:{:.2}px;font-family:'{}',serif;font-size:{}px;font-weight:{};font-style:{};color:{};letter-spacing:{:.3}px\">{}</p>",
                        line.x,
                        line.baseline - line.size * 0.85,
                        xml(&font.family),
                        line.size,
                        font.weight,
                        if font.italic { "italic" } else { "normal" },
                        if line.fill.starts_with('#') && line.fill[1..].chars().all(|c| c.is_ascii_hexdigit()) { &line.fill } else { "#000000" },
                        if spacing.is_finite() { spacing } else { 0.0 },
                        xml(&line.text),
                    ));
                }
                body.push_str("</div>");
                let file = format!("page-{n}-{index}.xhtml");
                let head = format!("<meta name=\"viewport\" content=\"width={w}, height={h}\"/>");
                files.push((
                    format!("OEBPS/{file}"),
                    xhtml(
                        &format!("{title} page {}", index + 1),
                        &language,
                        "styles.css",
                        &head,
                        &body,
                    )
                    .into_bytes(),
                ));
                manifest.push((
                    format!("page-{n}-{index}"),
                    file.clone(),
                    "application/xhtml+xml".into(),
                    String::new(),
                ));
                spine.push(format!("page-{n}-{index}"));
                nav.push((1, format!("Page {}", index + 1), file));
            }
        } else {
            let placed: HashMap<String, usize> = crate::stories::story_threads(raw)
                .iter()
                .enumerate()
                .map(|(i, t)| (t.story["id"].as_str().unwrap_or_default().to_string(), i))
                .collect();
            let mut stories: Vec<(&String, &usize)> = placed.iter().collect();
            stories.sort_by_key(|(_, i)| **i);
            unplaced += raw
                .pointer("/layout/stories")
                .and_then(Value::as_array)
                .map_or(0, |s| s.len().saturating_sub(placed.len()));
            for (sid, _) in stories {
                let Some(story) = raw
                    .pointer("/layout/stories")
                    .and_then(Value::as_array)
                    .and_then(|list| list.iter().find(|s| s["id"] == *sid))
                else {
                    continue;
                };
                let mut body = String::new();
                let file = format!("story-{n}-{}.xhtml", slug(sid));
                for (pi, paragraph) in story["paragraphs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let style = paragraph.get("style").and_then(Value::as_str);
                    if let Some(s) = style {
                        css_ids.insert(s.to_string());
                    }
                    let html: String = paragraph["runs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|r| run_html(raw, r, &links))
                        .collect();
                    let class =
                        style.map_or(String::new(), |s| format!(" class=\"s-{}\"", slug(s)));
                    let tag = heading_level(style).map_or("p".to_string(), |l| format!("h{l}"));
                    if let Some(level) = heading_level(style) {
                        let plain: String = paragraph["runs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|r| crate::stories::run_text(raw, r))
                            .collect();
                        nav.push((level, plain, format!("{file}#p{pi}")));
                    }
                    body.push_str(&format!("<{tag} id=\"p{pi}\"{class}>{html}</{tag}>\n"));
                    paragraphs_total += 1;
                }
                files.push((
                    format!("OEBPS/{file}"),
                    xhtml(&title, &language, "styles.css", "", &body).into_bytes(),
                ));
                let id = format!("story-{n}-{}", slug(sid));
                manifest.push((
                    id.clone(),
                    file,
                    "application/xhtml+xml".into(),
                    String::new(),
                ));
                spine.push(id);
            }
        }
        if !options.fixed {
            let _ = document;
        }
        // Style sheet inputs are collected across all documents.
        if n + 1 == docs.len() {
            let mut css = css_for(raw, &css_ids);
            if options.fixed {
                css.push_str(".page{position:relative;margin:0 auto;overflow:hidden}.page img{position:absolute;left:0;top:0}.l{position:absolute;margin:0;white-space:pre;line-height:1.2}\n");
            }
            for (name, (family, _)) in &font_files {
                css.push_str(&format!(
                    "@font-face{{font-family:\"{}\";src:url(\"{name}\")}}\n",
                    family.replace('"', "")
                ));
            }
            files.push(("OEBPS/styles.css".into(), css.into_bytes()));
        }
    }
    ensure!(
        !spine.is_empty(),
        "[invalid-input] no placed stories or pages to export"
    );
    for (name, (_, bytes)) in &font_files {
        files.push((format!("OEBPS/{name}"), bytes.clone()));
        manifest.push((
            format!("font-{}", slug(name)),
            name.clone(),
            "font/ttf".into(),
            String::new(),
        ));
    }
    manifest.push((
        "css".into(),
        "styles.css".into(),
        "text/css".into(),
        String::new(),
    ));

    // Navigation document.
    let mut nav_html = String::from("<nav epub:type=\"toc\" id=\"toc\"><h1>Contents</h1><ol>\n");
    if nav.is_empty() {
        nav_html.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            xml(manifest
                .iter()
                .find(|m| m.0 == spine[0])
                .map_or("", |m| &m.1)),
            xml(&title)
        ));
    }
    for (_, text, href) in &nav {
        nav_html.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            xml(href),
            xml(if text.trim().is_empty() {
                "Untitled"
            } else {
                text.trim()
            })
        ));
    }
    nav_html.push_str("</ol></nav>");
    files.push((
        "OEBPS/nav.xhtml".into(),
        xhtml(&title, &language, "styles.css", "", &nav_html).into_bytes(),
    ));
    manifest.push((
        "nav".into(),
        "nav.xhtml".into(),
        "application/xhtml+xml".into(),
        "nav".into(),
    ));

    // Deterministic identifier from the content.
    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    for (name, bytes) in &files {
        hasher.update(name.as_bytes());
        hasher.update(bytes);
    }
    let digest = hasher.finalize();
    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
        digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14], digest[15]
    );
    let mut opf = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"uid\" xml:lang=\"{l}\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n<dc:identifier id=\"uid\">urn:uuid:{uuid}</dc:identifier>\n<dc:title>{t}</dc:title>\n<dc:language>{l}</dc:language>\n<meta property=\"dcterms:modified\">1970-01-01T00:00:00Z</meta>\n<meta property=\"schema:accessMode\">textual</meta>\n<meta property=\"schema:accessModeSufficient\">textual</meta>\n<meta property=\"schema:accessibilityFeature\">structuralNavigation</meta>\n<meta property=\"schema:accessibilityFeature\">tableOfContents</meta>\n<meta property=\"schema:accessibilityHazard\">none</meta>\n<meta property=\"schema:accessibilitySummary\">Text is real text with a navigation document; page artwork has no alternative text unless set in the document.</meta>\n",
        l = xml(&language),
        t = xml(&title),
    );
    if options.fixed {
        opf.push_str("<meta property=\"rendition:layout\">pre-paginated</meta>\n<meta property=\"rendition:orientation\">auto</meta>\n<meta property=\"rendition:spread\">none</meta>\n");
    }
    opf.push_str("</metadata>\n<manifest>\n");
    for (id, href, media, props) in &manifest {
        opf.push_str(&format!(
            "<item id=\"{}\" href=\"{}\" media-type=\"{media}\"{}/>\n",
            xml(id),
            xml(href),
            if props.is_empty() {
                String::new()
            } else {
                format!(" properties=\"{props}\"")
            }
        ));
    }
    opf.push_str("</manifest>\n<spine>\n");
    for id in &spine {
        opf.push_str(&format!("<itemref idref=\"{}\"/>\n", xml(id)));
    }
    opf.push_str("</spine>\n</package>\n");

    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        zip.start_file("mimetype", stored)?;
        zip.write_all(b"application/epub+zip")?;
        zip.start_file("META-INF/container.xml", deflated)?;
        zip.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>\n")?;
        zip.start_file("OEBPS/content.opf", deflated)?;
        zip.write_all(opf.as_bytes())?;
        for (name, bytes) in &files {
            zip.start_file(name.as_str(), deflated)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }
    let bytes = buffer.into_inner();
    if bytes.len() > 512 * 1024 * 1024 {
        bail!("[limit-exceeded] the EPUB would exceed 512 MiB");
    }
    crate::editing::atomic_write(output, &bytes)?;
    let mut report = Map::new();
    report.insert("operation".into(), json!("layout-export"));
    report.insert("format".into(), json!("epub"));
    report.insert(
        "layout".into(),
        json!(if options.fixed { "fixed" } else { "reflowable" }),
    );
    report.insert("output".into(), json!(output));
    report.insert("documents".into(), json!(docs.len()));
    report.insert("spine_items".into(), json!(spine.len()));
    report.insert("paragraphs".into(), json!(paragraphs_total));
    report.insert("navigation_entries".into(), json!(nav.len()));
    report.insert("embedded_fonts".into(), json!(font_files.len()));
    report.insert("unplaced_stories_skipped".into(), json!(unplaced));
    report.insert("overset_stories".into(), json!(overset_total));
    report.insert("bytes".into(), json!(bytes.len()));
    report.insert(
        "limits".into(),
        json!([
            "reflowable output carries story text and paragraph styles only; images and frames are not exported",
            "internal page and bookmark links are not rewritten in reflowable output",
            "EPUBCheck validation is not run by pentool"
        ]),
    );
    Ok(Value::Object(report))
}

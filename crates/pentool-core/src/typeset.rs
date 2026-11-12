//! Composed frames: flow every placed story through its thread with the
//! paragraph composer and report positioned lines (v0.14.0 tasks 15-16).
//!
//! The result is the single source for rendering text frames, the PDF and EPUB
//! publishing exports and layout preflight, so overset text is measured with
//! the real composer instead of the task 3 estimator. Everything is integer
//! layout units inside the composer; positions leave this module in points.
//!
//! Scope: one uniform paragraph style per paragraph (run-level character
//! styles do not change composition), paragraphs flow column by column and
//! frame by frame, and a paragraph is composed at the width of the column in
//! which it starts.
use crate::compose::{compose_paragraph, Request};
use crate::shaping::{self, Kerning, ResolvedFont, LU_PER_PT};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Upper bound on lines per document; the rest is reported as overset.
pub const MAX_LINES: usize = 400_000;
const DEFAULT_FAMILY: &str = "Atkinson Hyperlegible";
const ASCENT: f64 = 0.8;

#[derive(Debug, Clone)]
pub struct PlacedLine {
    pub story: String,
    pub frame: String,
    pub page: usize,
    pub paragraph: usize,
    /// Characters of the paragraph covered by this line.
    pub start: usize,
    pub end: usize,
    /// Characters of the story before this paragraph (offset of `start`).
    pub story_offset: usize,
    pub text: String,
    pub x: f64,
    pub baseline: f64,
    pub width: f64,
    pub natural: f64,
    pub size: f64,
    pub font: usize,
    pub fill: String,
    pub hyphenated: bool,
    pub justified: bool,
    /// Paragraph style ID, when the paragraph names one.
    pub style: Option<String>,
    pub first_in_paragraph: bool,
}

#[derive(Debug)]
pub struct Overset {
    pub story: String,
    pub frame: String,
    pub page: usize,
    pub paragraph: usize,
    pub offset: usize,
    pub characters_remaining: usize,
}

pub struct FontRef {
    pub font: ResolvedFont,
    pub requested: String,
}

#[derive(Default)]
pub struct Typeset {
    pub lines: Vec<PlacedLine>,
    pub overset: Vec<Overset>,
    pub fonts: Vec<FontRef>,
    /// Fonts that were substituted: (requested family, reason).
    pub substituted: Vec<(String, String)>,
    pub truncated: bool,
}

impl Typeset {
    pub fn page_lines(&self, page: usize) -> impl Iterator<Item = &PlacedLine> {
        self.lines.iter().filter(move |l| l.page == page)
    }
}

pub fn pt(lu: i64) -> f64 {
    lu as f64 / LU_PER_PT as f64
}

fn fill_of(raw: &Value, style: &Map<String, Value>) -> String {
    let direct = style.get("fill").and_then(Value::as_str);
    let via_char = style
        .get("character_style")
        .and_then(Value::as_str)
        .and_then(|id| crate::styles::resolve(raw, "character", id).ok())
        .and_then(|m| m.get("fill").and_then(Value::as_str).map(str::to_string));
    direct
        .map(str::to_string)
        .or(via_char)
        .filter(|f| f.starts_with('#') && f.len() <= 9)
        .unwrap_or_else(|| "#000000".into())
}

/// Compose every placed story of a version 9 document.
pub fn typeset(raw: &Value) -> Result<Typeset> {
    let mut out = Typeset::default();
    let mut font_index: BTreeMap<String, usize> = BTreeMap::new();
    for thread in crate::stories::story_threads(raw) {
        let story_id = thread.story["id"].as_str().unwrap_or_default().to_string();
        let language = thread
            .story
            .get("language")
            .and_then(Value::as_str)
            .map(str::to_string);
        // Frame cursor: frame index, column index, y used inside the column.
        let mut frame_at = 0usize;
        let mut column = 0usize;
        let mut used = 0.0f64;
        let mut story_offset = 0usize;
        let paragraphs = thread
            .story
            .get("paragraphs")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let mut full = false;
        let mut first_overset: Option<(usize, usize, usize)> = None;
        'paragraphs: for (pi, paragraph) in paragraphs.iter().enumerate() {
            let text: String = paragraph
                .get("runs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|r| crate::stories::run_text(raw, r))
                .collect();
            let chars = text.chars().count();
            let style_id = paragraph
                .get("style")
                .and_then(Value::as_str)
                .map(str::to_string);
            let mut style = style_id
                .as_deref()
                .and_then(|id| crate::styles::resolve(raw, "paragraph", id).ok())
                .unwrap_or_default();
            if let Some(over) = paragraph.get("overrides").and_then(Value::as_object) {
                for (k, v) in over {
                    style.insert(k.clone(), v.clone());
                }
            }
            if let Some(lang) = &language {
                style
                    .entry("language")
                    .or_insert_with(|| Value::String(lang.clone()));
            }
            let family = style
                .get("font")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_FAMILY)
                .to_string();
            let weight = style
                .get("weight")
                .and_then(Value::as_u64)
                .map_or(400, |w| w.clamp(1, 1000) as u16);
            let italic = style
                .get("italic")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let size = style
                .get("size")
                .and_then(Value::as_f64)
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(12.0);
            let leading = match style.get("leading") {
                Some(Value::Number(n)) => n.as_f64().unwrap_or(size * 1.2),
                _ => size * 1.2,
            };
            let space_before = style
                .get("space_before")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .max(0.0);
            let space_after = style
                .get("space_after")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .max(0.0);
            let fill = fill_of(raw, &style);
            let align = style
                .get("align")
                .and_then(Value::as_str)
                .unwrap_or("left")
                .to_string();
            if full || thread.frames.is_empty() {
                if first_overset.is_none() {
                    first_overset = Some((pi, story_offset, frame_at));
                }
                story_offset += chars + 1;
                continue;
            }
            let key = format!("{family}|{weight}|{italic}");
            let mut font_slot: Option<usize> = font_index.get(&key).copied();
            let mut piece_offset = 0usize;
            let pieces: Vec<&str> = text.split('\n').collect();
            for (piece_no, piece) in pieces.iter().enumerate() {
                let piece_chars: Vec<char> = piece.chars().collect();
                if piece_chars.is_empty() && pieces.len() > 1 && piece_no + 1 == pieces.len() {
                    break;
                }
                let (cols, col_w, frame_h, gutter) =
                    crate::stories::frame_columns(thread.frames[frame_at].1);
                let width_lu = shaping::to_lu(col_w.max(1.0));
                let request = Request {
                    raw,
                    text: piece,
                    style: style.clone(),
                    family: family.clone(),
                    weight,
                    italic,
                    width_lu,
                    kerning: Kerning::Metrics,
                    features: Vec::new(),
                };
                let outcome = compose_paragraph(&request)
                    .with_context(|| format!("story {story_id} paragraph {pi}"))?;
                let slot = match font_slot {
                    Some(slot) => slot,
                    None => {
                        let slot = out.fonts.len();
                        if let Some(reason) = &outcome.font.substituted {
                            out.substituted.push((family.clone(), reason.clone()));
                        }
                        out.fonts.push(FontRef {
                            font: outcome.font,
                            requested: family.clone(),
                        });
                        font_index.insert(key.clone(), slot);
                        font_slot = Some(slot);
                        slot
                    }
                };
                let _ = (cols, gutter, frame_h);
                let lines = outcome.composed.lines;
                if lines.is_empty() {
                    // An empty paragraph still occupies one line of leading.
                    used += leading;
                }
                let mut first = true;
                let mut line_no = 0usize;
                while line_no < lines.len() {
                    let line = &lines[line_no];
                    let (cols, col_w, frame_h, gutter) =
                        crate::stories::frame_columns(thread.frames[frame_at].1);
                    let before = if first && piece_no == 0 && used > 0.0 {
                        space_before
                    } else {
                        0.0
                    };
                    let fits = used + before + leading <= frame_h + 0.01
                        || used == 0.0 && {
                            // A frame shorter than one line still holds the first line.
                            frame_h >= size * 0.5
                        };
                    if !fits {
                        column += 1;
                        used = 0.0;
                        if column >= cols {
                            column = 0;
                            frame_at += 1;
                            if frame_at >= thread.frames.len() {
                                full = true;
                                first_overset =
                                    Some((pi, story_offset + piece_offset + line.start, frame_at));
                                break;
                            }
                        }
                        continue;
                    }
                    let (page, node) = thread.frames[frame_at];
                    let inset = node
                        .get("inset")
                        .and_then(Value::as_array)
                        .filter(|a| a.len() == 4)
                        .map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
                        .filter(|a| a.len() == 4)
                        .unwrap_or_else(|| vec![0.0; 4]);
                    let fx = node.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                    let fy = node.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                    let col_x = fx + inset[3] + column as f64 * (col_w + gutter);
                    used += before;
                    let baseline = fy + inset[0] + used + size * ASCENT;
                    let chars_text: String = piece_chars
                        .get(line.start..line.end.min(piece_chars.len()))
                        .map(|c| c.iter().collect())
                        .unwrap_or_default();
                    let mut text_out = chars_text.trim_end().to_string();
                    if line.hyphenated {
                        text_out.push('-');
                    }
                    if out.lines.len() >= MAX_LINES {
                        out.truncated = true;
                        full = true;
                        first_overset =
                            Some((pi, story_offset + piece_offset + line.start, frame_at));
                        break;
                    }
                    let justified =
                        align == "justify" && line.ratio_milli != 0 && !text_out.is_empty();
                    out.lines.push(PlacedLine {
                        story: story_id.clone(),
                        frame: node["id"].as_str().unwrap_or_default().to_string(),
                        page,
                        paragraph: pi,
                        start: piece_offset + line.start,
                        end: piece_offset + line.end,
                        story_offset,
                        text: text_out,
                        x: col_x + pt(line.x),
                        baseline,
                        width: pt(line.width),
                        natural: pt(line.natural),
                        size,
                        font: slot,
                        fill: fill.clone(),
                        hyphenated: line.hyphenated,
                        justified,
                        style: style_id.clone(),
                        first_in_paragraph: first && piece_no == 0,
                    });
                    first = false;
                    used += leading;
                    line_no += 1;
                }
                if full {
                    break 'paragraphs;
                }
                piece_offset += piece_chars.len() + 1;
            }
            used += space_after;
            story_offset += chars + 1;
        }
        if let Some((pi, offset, at)) = first_overset {
            let frame_ix = at.min(thread.frames.len().saturating_sub(1));
            let (page, node) = thread.frames[frame_ix];
            let total: usize = paragraphs
                .iter()
                .map(|p| {
                    p.get("runs")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|r| crate::stories::run_text(raw, r).chars().count())
                        .sum::<usize>()
                        + 1
                })
                .sum();
            let last = thread.frames.len() - 1;
            let (page, frame) = if at >= thread.frames.len() {
                let (p, n) = thread.frames[last];
                (p, n)
            } else {
                (page, node)
            };
            out.overset.push(Overset {
                story: story_id.clone(),
                frame: frame["id"].as_str().unwrap_or_default().to_string(),
                page,
                paragraph: pi,
                offset,
                characters_remaining: total.saturating_sub(offset),
            });
        }
    }
    Ok(out)
}

pub fn overset_json(o: &Overset) -> Value {
    json!({
        "story": o.story,
        "frame": o.frame,
        "paragraph": o.paragraph,
        "offset": o.offset,
        "characters_remaining": o.characters_remaining,
        "estimated": false,
    })
}

/// Text frames of a document with their page index, in document order.
pub fn frame_list(raw: &Value) -> Vec<(usize, Value)> {
    let mut out = Vec::new();
    fn walk(nodes: &[Value], page: usize, out: &mut Vec<(usize, Value)>) {
        for node in nodes {
            match node.get("kind").and_then(Value::as_str) {
                Some("text-frame") => out.push((page, node.clone())),
                Some("group") => {
                    if let Some(children) = node.get("children").and_then(Value::as_array) {
                        walk(children, page, out);
                    }
                }
                _ => {}
            }
        }
    }
    for (index, page) in raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        for layer in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
                walk(nodes, index, &mut out);
            }
        }
    }
    out
}

thread_local! {
    static CACHE: std::cell::RefCell<Option<(u64, std::rc::Rc<Typeset>)>> =
        const { std::cell::RefCell::new(None) };
}

/// `typeset` memoized on the document's bytes, so a many-page export composes
/// each story once.
pub fn typeset_cached(raw: &Value) -> Result<std::rc::Rc<Typeset>> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(raw.get("pages").unwrap_or(&Value::Null))
        .unwrap_or_default()
        .hash(&mut hasher);
    serde_json::to_vec(raw.get("layout").unwrap_or(&Value::Null))
        .unwrap_or_default()
        .hash(&mut hasher);
    serde_json::to_vec(raw.get("fonts").unwrap_or(&Value::Null))
        .unwrap_or_default()
        .hash(&mut hasher);
    let key = hasher.finish();
    if let Some(hit) = CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
    }) {
        return Ok(hit);
    }
    let value = std::rc::Rc::new(typeset(raw)?);
    CACHE.with(|c| *c.borrow_mut() = Some((key, value.clone())));
    Ok(value)
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// SVG `<text>` elements for the composed lines of one page.
pub fn page_svg(raw: &Value, page_index: usize) -> Result<String> {
    if !crate::spreads::is_v9(raw) || crate::stories::story_threads(raw).is_empty() {
        return Ok(String::new());
    }
    let set = typeset_cached(raw)?;
    let mut svg = String::new();
    for line in set.page_lines(page_index) {
        if line.text.is_empty() {
            continue;
        }
        let font = &set.fonts[line.font].font;
        let length = if line.justified && line.width > 0.0 {
            format!(r#" textLength="{:.3}" lengthAdjust="spacing""#, line.width)
        } else {
            String::new()
        };
        svg.push_str(&format!(
            r#"<text xml:space="preserve" x="{:.3}" y="{:.3}" font-family="{}" font-size="{}" font-weight="{}" font-style="{}" fill="{}"{}>{}</text>"#,
            line.x,
            line.baseline,
            xml(&font.family),
            line.size,
            font.weight,
            if font.italic { "italic" } else { "normal" },
            xml(&line.fill),
            length,
            xml(&line.text)
        ));
    }
    Ok(svg)
}

fn strip_frames(nodes: &mut Vec<Value>) {
    nodes.retain(|n| {
        !matches!(
            n.get("kind").and_then(Value::as_str),
            Some("text-frame" | "graphic-frame")
        )
    });
    for node in nodes.iter_mut() {
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            strip_frames(children);
        }
    }
}

fn visible_frames(nodes: &[Value], out: &mut std::collections::HashSet<String>) {
    for node in nodes {
        if node.get("visible").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        match node.get("kind").and_then(Value::as_str) {
            Some("text-frame") => {
                if let Some(id) = node.get("id").and_then(Value::as_str) {
                    out.insert(id.to_string());
                }
            }
            Some("group") => {
                if let Some(children) = node.get("children").and_then(Value::as_array) {
                    visible_frames(children, out);
                }
            }
            _ => {}
        }
    }
}

/// A render copy of a version 9 document: frames removed and their composed
/// lines added as ordinary `text` nodes in one top layer per page. Returns
/// `None` when the document has no frames, so callers keep the original.
pub fn materialize(raw: &Value) -> Result<Option<Value>> {
    if !crate::spreads::is_v9(raw) {
        return Ok(None);
    }
    let has_frames = !frame_list(raw).is_empty()
        || raw
            .get("pages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|p| {
                p.get("layers")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|l| {
                        l.get("nodes")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .any(|n| n.get("kind") == Some(&json!("graphic-frame")))
                    })
            });
    let parent_items = parent_item_layers(raw);
    if !has_frames && parent_items.iter().all(Option::is_none) {
        return Ok(None);
    }
    let set = typeset_cached(raw)?;
    let mut shown = std::collections::HashSet::new();
    for page in raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for layer in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if layer.get("visible").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
                visible_frames(nodes, &mut shown);
            }
        }
    }
    let mut out = raw.clone();
    let pages = out
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .context("pages missing")?;
    for (index, page) in pages.iter_mut().enumerate() {
        let layers = page
            .get_mut("layers")
            .and_then(Value::as_array_mut)
            .context("layers missing")?;
        for layer in layers.iter_mut() {
            if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                strip_frames(nodes);
            }
        }
        if let Some(Some(layer)) = parent_items.get(index) {
            layers.insert(0, layer.clone());
        }
        let mut nodes = Vec::new();
        for (n, line) in set.page_lines(index).enumerate() {
            if line.text.is_empty() || !shown.contains(&line.frame) {
                continue;
            }
            let font = &set.fonts[line.font].font;
            let extra = if line.justified && line.text.chars().count() > 1 {
                (line.width - line.natural) / (line.text.chars().count() - 1) as f64
            } else {
                0.0
            };
            nodes.push(json!({
                "kind": "text",
                "id": format!("composed-{}-{}-{n}", line.story, line.frame),
                "content": line.text,
                "x": line.x,
                "y": line.baseline,
                "font_family": font.family,
                "font_size": line.size,
                "font_weight": font.weight,
                "italic": font.italic,
                "align": "left",
                "letter_spacing": if extra.is_finite() { extra } else { 0.0 },
                "line_height": 1.2,
                "style": {"fill": {"fallback": line.fill}},
            }));
        }
        if !nodes.is_empty() {
            layers.push(json!({
                "id": "composed-text",
                "name": "Composed text",
                "visible": true,
                "locked": true,
                "nodes": nodes,
            }));
        }
    }
    Ok(Some(out))
}

/// Per page, one locked bottom layer holding the items the page receives from
/// its parent chain (after overrides and detaches), or `None` when it has none.
/// Parent items render beneath the page's own layers; text frames among them
/// have no story on a page and are dropped like any other frame.
fn parent_item_layers(raw: &Value) -> Vec<Option<Value>> {
    let parents = raw
        .get("layout")
        .and_then(|l| l.get("parents"))
        .and_then(Value::as_array);
    raw.get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|page| {
            let id = page.get("id").and_then(Value::as_str)?;
            let resolved = crate::spreads::resolve_page(raw, id).ok()?;
            let mut nodes: Vec<Value> = resolved["items"]
                .as_array()?
                .iter()
                .filter(|item| {
                    let hidden = parents
                        .into_iter()
                        .flatten()
                        .find(|p| p["id"] == item["parent"])
                        .and_then(|p| p["layers"].as_array())
                        .and_then(|layers| layers.iter().find(|l| l["id"] == item["layer"]))
                        .is_some_and(|l| l["visible"].as_bool() == Some(false));
                    !hidden
                })
                .map(|item| item["node"].clone())
                .collect();
            strip_frames(&mut nodes);
            if nodes.is_empty() {
                return None;
            }
            Some(json!({
                "id": "parent-items",
                "name": "Parent items",
                "visible": true,
                "locked": true,
                "nodes": nodes,
            }))
        })
        .collect()
}

/// IDs of text frames on visible layers (and not hidden individually).
pub fn shown_frames(raw: &Value) -> std::collections::HashSet<String> {
    let mut shown = std::collections::HashSet::new();
    for page in raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for layer in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if layer.get("visible").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
                visible_frames(nodes, &mut shown);
            }
        }
    }
    shown
}

/// A copy of the document with every frame removed, so a page renders
/// without its composed text (exports overlay the text as real text).
pub fn without_frames(raw: &Value) -> Value {
    let mut out = raw.clone();
    if let Some(pages) = out.get_mut("pages").and_then(Value::as_array_mut) {
        for page in pages {
            if let Some(layers) = page.get_mut("layers").and_then(Value::as_array_mut) {
                for layer in layers {
                    if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                        strip_frames(nodes);
                    }
                }
            }
        }
    }
    out
}

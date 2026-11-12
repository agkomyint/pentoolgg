//! v0.14.0 task 4: the shaping and font engine.
//!
//! Text is shaped with the pure-Rust `rustybuzz` OpenType shaper. Bidirectional
//! ordering follows UAX #9 (`unicode-bidi`), break opportunities follow UAX #14
//! (`unicode-linebreak`) and script runs come from `unicode-script`. Every
//! length that leaves this module is an integer number of layout units (LU =
//! 1/1024 pt); no float participates in an advance or an offset.
//!
//! Fonts resolve only from the document's embedded fonts and the bundled
//! default family. The operating system's font service is never queried, so the
//! output depends only on declared fonts (`docs/layout-v1.md` section 6.7).
use crate::fonts;
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rustybuzz::ttf_parser::{self, Tag};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ops::Range;
use unicode_bidi::{BidiInfo, Level};
use unicode_script::{Script as UScript, UnicodeScript};

/// Recorded in `layout.engine`; bump on any behavior change.
pub const ENGINE: u32 = 1;
pub const LU_PER_PT: i64 = 1024;
pub const MAX_TEXT_CHARS: usize = 200_000;
pub const MAX_FEATURES: usize = 64;
pub const MAX_AXES: usize = 16;
/// Optical kerning aims at this side-bearing sum, in 1/1000 em.
const OPTICAL_TARGET_GAP: i64 = 60;
/// Optical kerning never moves a pair by more than this, in 1/1000 em.
const OPTICAL_MAX_ADJUST: i64 = 100;

/// A face found for a request, with its disclosed provenance.
pub struct ResolvedFont {
    pub bytes: Vec<u8>,
    pub index: u32,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub sha256: String,
    /// `document:<font id>` or `bundled`.
    pub source: String,
    /// The requested family or style could not be honored exactly.
    pub substituted: Option<String>,
}

struct Candidate {
    bytes: Vec<u8>,
    source: String,
}

fn candidates(raw: &Value) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    if let Some(list) = raw.get("fonts").and_then(Value::as_array) {
        for font in list {
            let (Some(id), Some(data)) = (
                font.get("id").and_then(Value::as_str),
                font.get("data").and_then(Value::as_str),
            ) else {
                continue;
            };
            if data.len() > 12 * 1024 * 1024 {
                bail!("[limit-exceeded] embedded font {id} is larger than 8 MiB");
            }
            let bytes = STANDARD
                .decode(data)
                .with_context(|| format!("[invalid-input] embedded font {id} is not base64"))?;
            out.push(Candidate {
                bytes,
                source: format!("document:{id}"),
            });
        }
    }
    for (_, _, _, bytes) in fonts::BUNDLED {
        out.push(Candidate {
            bytes: bytes.to_vec(),
            source: "bundled".into(),
        });
    }
    Ok(out)
}

fn face_family(face: &ttf_parser::Face) -> Option<String> {
    let find = |id: u16| {
        face.names()
            .into_iter()
            .filter(|n| n.name_id == id && n.is_unicode())
            .find_map(|n| n.to_string())
    };
    find(16).or_else(|| find(1))
}

/// Resolve `family` at `weight`/`italic` against the document's embedded fonts,
/// then the bundled default. A miss is substituted and the reason disclosed.
pub fn resolve(raw: &Value, family: &str, weight: u16, italic: bool) -> Result<ResolvedFont> {
    let list = candidates(raw)?;
    // (family matches, style distance, weight distance, order)
    let mut best: Option<(u8, u8, u16, usize, String, u16, bool)> = None;
    for (order, candidate) in list.iter().enumerate() {
        let count = ttf_parser::fonts_in_collection(&candidate.bytes).unwrap_or(1);
        for index in 0..count {
            let Ok(face) = ttf_parser::Face::parse(&candidate.bytes, index) else {
                continue;
            };
            let name = face_family(&face).unwrap_or_default();
            let face_italic = face.is_italic() || face.is_oblique();
            let face_weight = face.weight().to_number();
            let key = (
                u8::from(!name.eq_ignore_ascii_case(family)),
                u8::from(face_italic != italic),
                face_weight.abs_diff(weight),
                order * 1024 + index as usize,
                name,
                face_weight,
                face_italic,
            );
            if best
                .as_ref()
                .is_none_or(|b| (b.0, b.1, b.2, b.3) > (key.0, key.1, key.2, key.3))
            {
                best = Some(key);
            }
        }
    }
    let Some((miss, style_miss, weight_distance, slot, name, face_weight, face_italic)) = best
    else {
        bail!("[font-unavailable] no usable font is available");
    };
    let candidate = &list[slot / 1024];
    let reason = if miss == 1 {
        Some(format!("family-unavailable:{family}"))
    } else if style_miss == 1 {
        Some("style-substituted".to_string())
    } else if weight_distance != 0 {
        Some(format!("weight-substituted:{weight}->{face_weight}"))
    } else {
        None
    };
    Ok(ResolvedFont {
        bytes: candidate.bytes.clone(),
        index: (slot % 1024) as u32,
        family: name,
        weight: face_weight,
        italic: face_italic,
        sha256: hex(&Sha256::digest(&candidate.bytes)),
        source: candidate.source.clone(),
        substituted: reason,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn tag(text: &str) -> Result<Tag> {
    let bytes = text.as_bytes();
    if bytes.len() != 4 || !bytes.iter().all(|b| (0x20..0x7f).contains(b)) {
        bail!("[invalid-input] OpenType tag `{text}` must be four ASCII characters");
    }
    Ok(Tag::from_bytes(&[bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn feature_alias(name: &str) -> &str {
    match name {
        "ligatures" => "liga",
        "discretionary-ligatures" => "dlig",
        "kerning" => "kern",
        "small-caps" => "smcp",
        "all-small-caps" => "c2sc",
        "oldstyle-figures" => "onum",
        "lining-figures" => "lnum",
        "tabular-figures" => "tnum",
        "proportional-figures" => "pnum",
        "fractions" => "frac",
        "ordinals" => "ordn",
        "contextual-alternates" => "calt",
        "swashes" => "swsh",
        "titling" => "titl",
        "slashed-zero" => "zero",
        "superscript" => "sups",
        "subscript" => "subs",
        other => other,
    }
}

/// Parse `liga,-kern,smcp,ss03,cv05=2`. Later entries win; `-name` disables.
pub fn parse_features(text: &str) -> Result<Vec<(String, u32)>> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, value) = if let Some(rest) = item.strip_prefix('-') {
            (rest, 0)
        } else if let Some((name, value)) = item.split_once('=') {
            let value: u32 = value.trim().parse().with_context(|| {
                format!("[invalid-input] feature `{item}` needs a non-negative integer value")
            })?;
            (name.trim(), value)
        } else {
            (item, 1)
        };
        let name = feature_alias(name).to_string();
        tag(&name)?;
        out.retain(|(existing, _)| existing != &name);
        out.push((name, value));
    }
    if out.len() > MAX_FEATURES {
        bail!("[limit-exceeded] at most {MAX_FEATURES} OpenType features per run");
    }
    Ok(out)
}

/// Parse `wght=700,wdth=90` variable-font axis settings.
pub fn parse_axes(text: &str) -> Result<Vec<(String, f32)>> {
    let mut out: Vec<(String, f32)> = Vec::new();
    for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, value) = item
            .split_once('=')
            .with_context(|| format!("[invalid-input] axis `{item}` must be `tag=value`"))?;
        tag(name.trim())?;
        let value: f32 = value
            .trim()
            .parse()
            .with_context(|| format!("[invalid-input] axis `{item}` needs a number"))?;
        if !value.is_finite() {
            bail!("[invalid-input] axis `{item}` must be finite");
        }
        out.retain(|(existing, _)| existing != name.trim());
        out.push((name.trim().to_string(), value));
    }
    if out.len() > MAX_AXES {
        bail!("[limit-exceeded] at most {MAX_AXES} variable axes per run");
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kerning {
    Metrics,
    Optical,
    None,
}

impl Kerning {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "metrics" => Self::Metrics,
            "optical" => Self::Optical,
            "none" => Self::None,
            other => bail!("[invalid-input] kerning `{other}` must be metrics, optical or none"),
        })
    }
    fn name(self) -> &'static str {
        match self {
            Self::Metrics => "metrics",
            Self::Optical => "optical",
            Self::None => "none",
        }
    }
}

pub struct ShapeParams {
    pub size_pt: f64,
    pub features: Vec<(String, u32)>,
    pub axes: Vec<(String, f32)>,
    pub language: Option<String>,
    /// `None` = first strong character decides.
    pub rtl: Option<bool>,
    pub kerning: Kerning,
    /// 1/1000 em added after every glyph.
    pub tracking: i32,
    /// Points, positive raises.
    pub baseline_shift: f64,
    /// Percent.
    pub h_scale: f64,
    pub v_scale: f64,
    /// Degrees.
    pub skew: f64,
}

impl Default for ShapeParams {
    fn default() -> Self {
        Self {
            size_pt: 12.0,
            features: Vec::new(),
            axes: Vec::new(),
            language: None,
            rtl: None,
            kerning: Kerning::Metrics,
            tracking: 0,
            baseline_shift: 0.0,
            h_scale: 100.0,
            v_scale: 100.0,
            skew: 0.0,
        }
    }
}

impl ShapeParams {
    pub fn validate(&self) -> Result<()> {
        let in_range = |name: &str, v: f64, lo: f64, hi: f64| -> Result<()> {
            if !v.is_finite() || v < lo || v > hi {
                bail!("[invalid-input] {name} must be between {lo} and {hi}");
            }
            Ok(())
        };
        in_range("--size", self.size_pt, 0.01, 10000.0)?;
        in_range("--h-scale", self.h_scale, 1.0, 1000.0)?;
        in_range("--v-scale", self.v_scale, 1.0, 1000.0)?;
        in_range("--skew", self.skew, -85.0, 85.0)?;
        in_range("--baseline-shift", self.baseline_shift, -10000.0, 10000.0)?;
        in_range("--tracking", f64::from(self.tracking), -1000.0, 10000.0)?;
        Ok(())
    }
}

/// Round-half-away-from-zero `value * numerator / denominator`.
fn scale(value: i64, numerator: i64, denominator: i64) -> i64 {
    let product = value * numerator;
    let half = denominator / 2;
    if product >= 0 {
        (product + half) / denominator
    } else {
        -((-product + half) / denominator)
    }
}

pub fn to_lu(points: f64) -> i64 {
    (points * LU_PER_PT as f64).round() as i64
}

#[derive(Debug, Clone)]
pub struct Glyph {
    pub id: u32,
    /// Character offset of the cluster start within the shaped text.
    pub cluster: usize,
    pub x_advance: i64,
    pub y_advance: i64,
    pub x_offset: i64,
    pub y_offset: i64,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub paragraph: usize,
    pub chars: Range<usize>,
    pub script: &'static str,
    pub rtl: bool,
    pub level: u8,
    pub glyphs: Vec<Glyph>,
    pub missing: usize,
}

pub struct Shaped {
    pub runs: Vec<Run>,
    pub paragraphs: usize,
    pub breaks: Vec<(usize, bool)>,
    pub units_per_em: u16,
    pub axes_applied: Vec<(String, f32)>,
    pub features: Vec<(String, u32)>,
}

fn script_tag(script: UScript) -> Option<Tag> {
    let short = script.short_name();
    if short.len() == 4 && short != "Zyyy" && short != "Zinh" {
        tag(short).ok()
    } else {
        None
    }
}

/// Shape `text` (paragraphs separated by `\n`) with `font` and `params`.
pub fn shape(font: &ResolvedFont, text: &str, params: &ShapeParams) -> Result<Shaped> {
    params.validate()?;
    if text.chars().count() > MAX_TEXT_CHARS {
        bail!("[limit-exceeded] at most {MAX_TEXT_CHARS} characters can be shaped at once");
    }
    let mut face = rustybuzz::Face::from_slice(&font.bytes, font.index)
        .context("[font-unavailable] the font could not be parsed for shaping")?;
    let mut axes_applied = Vec::new();
    if !params.axes.is_empty() {
        let known: Vec<Tag> = face.variation_axes().into_iter().map(|a| a.tag).collect();
        for (name, value) in &params.axes {
            let axis = tag(name)?;
            if !known.contains(&axis) {
                bail!(
                    "[invalid-input] font `{}` has no variable axis `{name}`; its axes are [{}]",
                    font.family,
                    known
                        .iter()
                        .map(|t| t.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            face.set_variation(axis, *value);
            axes_applied.push((name.clone(), *value));
        }
    }
    let upem = i64::from(face.units_per_em()).max(1);
    let size_lu = to_lu(params.size_pt);
    let h_scale = (params.h_scale * 1000.0).round() as i64;
    let mut buzz_features: Vec<rustybuzz::Feature> = Vec::new();
    let mut applied = params.features.clone();
    if params.kerning != Kerning::Metrics && !applied.iter().any(|(n, _)| n == "kern") {
        applied.push(("kern".into(), 0));
    }
    for (name, value) in &applied {
        buzz_features.push(rustybuzz::Feature::new(tag(name)?, *value, ..));
    }
    let language = match &params.language {
        Some(l) => Some(
            l.parse::<rustybuzz::Language>()
                .map_err(|_| anyhow::anyhow!("[invalid-input] language `{l}` is not valid"))?,
        ),
        None => None,
    };
    let baseline_lu = to_lu(params.baseline_shift);
    let tracking_lu = scale(i64::from(params.tracking), size_lu, 1000);

    let mut runs = Vec::new();
    let mut breaks = Vec::new();
    let mut paragraph_count = 0usize;
    let mut char_base = 0usize;
    for (paragraph_index, line) in text.split('\n').enumerate() {
        paragraph_count += 1;
        let byte_to_char: Vec<usize> = {
            let mut map = vec![0usize; line.len() + 1];
            let mut count = 0;
            for (byte, ch) in line.char_indices() {
                for slot in map.iter_mut().skip(byte).take(ch.len_utf8()) {
                    *slot = count;
                }
                count += 1;
            }
            map[line.len()] = count;
            map
        };
        for (byte, opportunity) in unicode_linebreak::linebreaks(line) {
            // The end-of-text mandatory break is not a break inside the text.
            if byte < line.len() {
                breaks.push((
                    char_base + byte_to_char[byte],
                    opportunity == unicode_linebreak::BreakOpportunity::Mandatory,
                ));
            }
        }
        if !line.is_empty() {
            let level = params
                .rtl
                .map(|rtl| if rtl { Level::rtl() } else { Level::ltr() });
            let info = BidiInfo::new(line, level);
            for para in &info.paragraphs {
                let (levels, visual) = info.visual_runs(para, para.range.clone());
                for range in visual {
                    let level = levels[range.start];
                    let rtl = level.is_rtl();
                    let mut subs = script_runs(line, range.clone());
                    if rtl {
                        subs.reverse();
                    }
                    for (sub, script) in subs {
                        let mut buffer = rustybuzz::UnicodeBuffer::new();
                        buffer.push_str(&line[sub.clone()]);
                        buffer.set_direction(if rtl {
                            rustybuzz::Direction::RightToLeft
                        } else {
                            rustybuzz::Direction::LeftToRight
                        });
                        if let Some(script) =
                            script_tag(script).and_then(rustybuzz::Script::from_iso15924_tag)
                        {
                            buffer.set_script(script);
                        }
                        if let Some(language) = &language {
                            buffer.set_language(language.clone());
                        }
                        let output = rustybuzz::shape(&face, &buzz_features, buffer);
                        let infos = output.glyph_infos();
                        let positions = output.glyph_positions();
                        let mut glyphs = Vec::with_capacity(infos.len());
                        let mut missing = 0;
                        for (info, pos) in infos.iter().zip(positions) {
                            if info.glyph_id == 0 {
                                missing += 1;
                            }
                            let cluster =
                                char_base + byte_to_char[sub.start + info.cluster as usize];
                            let advance = scale(i64::from(pos.x_advance), size_lu, upem);
                            glyphs.push(Glyph {
                                id: info.glyph_id,
                                cluster,
                                x_advance: scale(advance, h_scale, 100_000) + tracking_lu,
                                y_advance: scale(i64::from(pos.y_advance), size_lu, upem),
                                x_offset: scale(
                                    scale(i64::from(pos.x_offset), size_lu, upem),
                                    h_scale,
                                    100_000,
                                ),
                                y_offset: scale(i64::from(pos.y_offset), size_lu, upem)
                                    + baseline_lu,
                            });
                        }
                        if params.kerning == Kerning::Optical {
                            optical_kerning(&face, &mut glyphs, size_lu, upem, h_scale);
                        }
                        runs.push(Run {
                            paragraph: paragraph_index,
                            chars: char_base + byte_to_char[sub.start]
                                ..char_base + byte_to_char[sub.end],
                            script: script.short_name(),
                            rtl,
                            level: level.number(),
                            glyphs,
                            missing,
                        });
                    }
                }
            }
        }
        char_base += line.chars().count() + 1;
    }
    Ok(Shaped {
        runs,
        paragraphs: paragraph_count,
        breaks,
        units_per_em: upem as u16,
        axes_applied,
        features: applied,
    })
}

/// Split a bidi level run into script runs in logical order. Common and
/// inherited characters join the preceding run (or the following one at the
/// start), so punctuation never forces a font or shaper change.
fn script_runs(line: &str, range: Range<usize>) -> Vec<(Range<usize>, UScript)> {
    let mut out: Vec<(Range<usize>, UScript)> = Vec::new();
    let neutral = |s: UScript| matches!(s, UScript::Common | UScript::Inherited | UScript::Unknown);
    for (offset, ch) in line[range.clone()].char_indices() {
        let at = range.start + offset;
        let script = ch.script();
        match out.last_mut() {
            Some(last) if neutral(script) || script == last.1 => last.0.end = at + ch.len_utf8(),
            Some(last) if neutral(last.1) => {
                last.1 = script;
                last.0.end = at + ch.len_utf8();
            }
            _ => out.push((at..at + ch.len_utf8(), script)),
        }
    }
    out
}

fn optical_kerning(
    face: &rustybuzz::Face,
    glyphs: &mut [Glyph],
    size_lu: i64,
    upem: i64,
    h_scale: i64,
) {
    let bounds = |id: u32| -> Option<(i64, i64)> {
        let glyph = ttf_parser::GlyphId(u16::try_from(id).ok()?);
        let advance = i64::from(face.glyph_hor_advance(glyph)?);
        let bbox = face.glyph_bounding_box(glyph)?;
        // (left side bearing, right side bearing) in font units
        Some((i64::from(bbox.x_min), advance - i64::from(bbox.x_max)))
    };
    let target = OPTICAL_TARGET_GAP * upem / 1000;
    let limit = OPTICAL_MAX_ADJUST * upem / 1000;
    for i in 0..glyphs.len().saturating_sub(1) {
        let (Some((_, rsb)), Some((lsb, _))) = (bounds(glyphs[i].id), bounds(glyphs[i + 1].id))
        else {
            continue;
        };
        let adjust = ((target - (rsb + lsb)) / 2).clamp(-limit, limit);
        glyphs[i].x_advance += scale(scale(adjust, size_lu, upem), h_scale, 100_000);
    }
}

/// A subset font file and the old-to-new glyph ID mapping (the subset renumbers
/// glyphs densely, so embedded text must use the new IDs).
pub struct Subset {
    pub bytes: Vec<u8>,
    pub mapping: Vec<(u32, u16)>,
}

/// Subset `font` to `glyph_ids` (plus `.notdef`).
pub fn subset(font: &ResolvedFont, glyph_ids: &[u32]) -> Result<Subset> {
    let mut remapper = subsetter::GlyphRemapper::new();
    remapper.remap(0);
    let mut wanted = Vec::new();
    for id in glyph_ids {
        let id16 = u16::try_from(*id).context("[invalid-input] glyph id is out of range")?;
        remapper.remap(id16);
        wanted.push(id16);
    }
    let bytes = subsetter::subset(&font.bytes, font.index, &remapper)
        .map_err(|e| anyhow::anyhow!("[font-subset-failed] {e:?}"))?;
    let mapping = wanted
        .iter()
        .filter_map(|old| remapper.get(*old).map(|new| (u32::from(*old), new)))
        .collect();
    Ok(Subset { bytes, mapping })
}

#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub offset: usize,
    pub limit: usize,
}

/// JSON for `text shape`: compact glyph rows `[id, cluster, x_adv, y_adv, x_off, y_off]`.
pub fn report(font: &ResolvedFont, shaped: &Shaped, params: &ShapeParams, page: Page) -> Value {
    let total: usize = shaped.runs.iter().map(|r| r.glyphs.len()).sum();
    let advance: i64 = shaped
        .runs
        .iter()
        .flat_map(|r| &r.glyphs)
        .map(|g| g.x_advance)
        .sum();
    let missing: usize = shaped.runs.iter().map(|r| r.missing).sum();
    let rows: Vec<Value> = shaped
        .runs
        .iter()
        .flat_map(|r| &r.glyphs)
        .skip(page.offset)
        .take(page.limit)
        .map(|g| {
            json!([
                g.id,
                g.cluster,
                g.x_advance,
                g.y_advance,
                g.x_offset,
                g.y_offset
            ])
        })
        .collect();
    let runs: Vec<Value> = shaped
        .runs
        .iter()
        .map(|r| {
            json!({
                "paragraph": r.paragraph,
                "start": r.chars.start,
                "end": r.chars.end,
                "script": r.script,
                "direction": if r.rtl { "rtl" } else { "ltr" },
                "level": r.level,
                "glyphs": r.glyphs.len(),
                "missing_glyphs": r.missing,
            })
        })
        .collect();
    let features: BTreeMap<&str, u32> = shaped
        .features
        .iter()
        .map(|(n, v)| (n.as_str(), *v))
        .collect();
    json!({
        "engine": ENGINE,
        "units": "lu (1/1024 pt)",
        "font": font_json(font),
        "size_lu": to_lu(params.size_pt),
        "units_per_em": shaped.units_per_em,
        "kerning": params.kerning.name(),
        "tracking": params.tracking,
        "features": features,
        "axes": shaped.axes_applied.iter().map(|(n, v)| (n.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
        "transform": {
            "horizontal_scale": params.h_scale,
            "vertical_scale": params.v_scale,
            "skew": params.skew,
            "baseline_shift_lu": to_lu(params.baseline_shift),
        },
        "paragraphs": shaped.paragraphs,
        "runs": runs,
        "advance_lu": advance,
        "missing_glyphs": missing,
        "breaks": shaped.breaks.iter().map(|(o, m)| json!([o, if *m { "mandatory" } else { "allowed" }])).collect::<Vec<_>>(),
        "matches": total,
        "returned": rows.len(),
        "offset": page.offset,
        "limit": page.limit,
        "has_more": page.offset + rows.len() < total,
        "glyphs": rows,
    })
}

pub fn font_json(font: &ResolvedFont) -> Value {
    json!({
        "family": font.family,
        "weight": font.weight,
        "italic": font.italic,
        "sha256": font.sha256,
        "source": font.source,
        "substituted": font.substituted,
    })
}

/// Families a document requests, with how each resolves and whether text in
/// the document would need substitution. Stable order, bounded output.
pub fn font_report(raw: &Value) -> Result<Value> {
    use std::collections::BTreeSet;
    let mut requests: BTreeSet<(String, u16, bool)> = BTreeSet::new();
    let mut collect_node = |node: &Value| {
        if matches!(
            node.get("kind").and_then(Value::as_str),
            Some("text") | None
        ) {
            if let Some(family) = node.get("font_family").and_then(Value::as_str) {
                requests.insert((
                    family.to_string(),
                    node.get("font_weight")
                        .and_then(Value::as_u64)
                        .map_or(400, |w| w.min(1000) as u16),
                    node.get("italic").and_then(Value::as_bool).unwrap_or(false),
                ));
            }
        }
    };
    fn walk(node: &Value, f: &mut dyn FnMut(&Value)) {
        f(node);
        for key in ["nodes", "children", "texts"] {
            if let Some(list) = node.get(key).and_then(Value::as_array) {
                for child in list {
                    walk(child, f);
                }
            }
        }
    }
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
            walk(layer, &mut collect_node);
        }
    }
    let styles = raw.pointer("/layout/styles");
    for kind in ["paragraph", "character"] {
        if let Some(map) = styles.and_then(|s| s.get(kind)).and_then(Value::as_object) {
            for style in map.values() {
                if let Some(family) = style.get("font").and_then(Value::as_str) {
                    requests.insert((
                        family.to_string(),
                        style
                            .get("weight")
                            .and_then(Value::as_u64)
                            .map_or(400, |w| w.min(1000) as u16),
                        style
                            .get("italic")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    ));
                }
            }
        }
    }
    let mut rows = Vec::new();
    let mut substituted = 0;
    for (family, weight, italic) in &requests {
        let font = resolve(raw, family, *weight, *italic)?;
        if font.substituted.is_some() {
            substituted += 1;
        }
        rows.push(json!({
            "requested": {"family": family, "weight": weight, "italic": italic},
            "resolved": font_json(&font),
            "missing": font.substituted.is_some(),
        }));
    }
    Ok(json!({
        "engine": ENGINE,
        "default_family": fonts::DEFAULT_FAMILY,
        "fallback_chain": ["document fonts", "bundled default"],
        "requested": rows.len(),
        "substituted": substituted,
        "fonts": rows,
    }))
}

pub fn story_text(raw: &Value, id: &str) -> Result<String> {
    let version = raw.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version < 9 {
        bail!("[unsupported-version] --story needs a version 9 document; run `pentool migrate --target 9` first");
    }
    let story = raw
        .pointer("/layout/stories")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find(|s| s["id"] == id))
        .with_context(|| format!("[not-found] story `{id}` does not exist"))?;
    let paragraphs: Vec<String> = story
        .get("paragraphs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|p| {
            p.get("runs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .collect();
    Ok(paragraphs.join("\n"))
}

/// `text shape`: read-only; works on documents of every version.
pub fn run_shape(raw: &Value, args: &crate::text::ShapeArgs) -> Result<Value> {
    let text = match (&args.text, &args.story) {
        (Some(text), None) => text.clone(),
        (None, Some(story)) => story_text(raw, story)?,
        _ => bail!("[invalid-input] give exactly one of --text or --story"),
    };
    if args.limit == 0 || args.limit > 5000 {
        bail!("[invalid-input] --limit must be between 1 and 5000");
    }
    let rtl = match args.direction.as_str() {
        "auto" => None,
        "ltr" => Some(false),
        "rtl" => Some(true),
        other => bail!("[invalid-input] --direction `{other}` must be auto, ltr or rtl"),
    };
    let params = ShapeParams {
        size_pt: args.size,
        features: parse_features(args.features.as_deref().unwrap_or(""))?,
        axes: parse_axes(args.axes.as_deref().unwrap_or(""))?,
        language: args.language.clone(),
        rtl,
        kerning: Kerning::parse(&args.kerning)?,
        tracking: args.tracking,
        baseline_shift: args.baseline_shift,
        h_scale: args.h_scale,
        v_scale: args.v_scale,
        skew: args.skew,
    };
    params.validate()?;
    let font = resolve(raw, &args.font, args.weight, args.italic)?;
    let shaped = shape(&font, &text, &params)?;
    Ok(report(
        &font,
        &shaped,
        &params,
        Page {
            offset: args.offset,
            limit: args.limit,
        },
    ))
}

/// `font add` / `font remove` for scene (version 4+) documents. Fonts live in
/// the top-level `fonts` array exactly as in legacy documents.
pub fn scene_font(raw: &mut Value, action: crate::text::FontAction) -> Result<Value> {
    use crate::text::FontAction;
    const MAX_FONTS: usize = 64;
    let fonts_list = raw
        .as_object_mut()
        .context("[invalid-document] document is not an object")?
        .entry("fonts")
        .or_insert_with(|| json!([]));
    let list = fonts_list
        .as_array_mut()
        .context("[invalid-document] `fonts` must be an array")?;
    match action {
        FontAction::Add { id, file } => {
            if id.is_empty() || id.len() > 128 {
                bail!("[invalid-input] font ID must be 1-128 characters");
            }
            if list.iter().any(|f| f["id"] == id) {
                bail!("[duplicate-id] font ID `{id}` already exists");
            }
            if list.len() >= MAX_FONTS {
                bail!("[limit-exceeded] a document holds at most {MAX_FONTS} embedded fonts");
            }
            let bytes = std::fs::read(&file)
                .with_context(|| format!("could not read font {}", file.display()))?;
            let asset = fonts::asset(id.clone(), bytes)?;
            list.push(json!({"id": asset.id, "data": asset.data}));
            Ok(json!({"font": id, "action": "added", "fonts": list.len()}))
        }
        FontAction::Remove { id } => {
            let index = list
                .iter()
                .position(|f| f["id"] == id)
                .with_context(|| format!("[not-found] font `{id}` is not embedded"))?;
            let before = font_report(raw)?;
            let mut after_raw = raw.clone();
            after_raw["fonts"].as_array_mut().unwrap().remove(index);
            let after = font_report(&after_raw)?;
            if after["substituted"].as_u64() > before["substituted"].as_u64() {
                bail!(
                    "[font-in-use] font `{id}` is used by text that would fall back to a substitute; change the text first"
                );
            }
            raw["fonts"].as_array_mut().unwrap().remove(index);
            Ok(json!({"font": id, "action": "removed"}))
        }
    }
}

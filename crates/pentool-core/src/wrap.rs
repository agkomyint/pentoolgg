//! v0.14.0 task 8: text wrap and anchored objects (`docs/layout-v1.md` section 8).
//!
//! Wrap is a property of an ordinary scene node (`wrap`). Anchored objects are
//! `{"anchor": "<node id>", ...}` runs in a story. Positions that depend on
//! where text lands use the task 3 flow estimator and are reported
//! `estimated: true`; the exclusion geometry itself is exact integer
//! milli-point arithmetic and deterministic on every platform.
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_BANDS_PER_OBJECT: i64 = 256;
pub const MAX_INTERVALS: usize = 10_000;
pub const MAX_LINES: usize = 2000;
pub const MAX_WRAPPERS_PER_PAGE: usize = 500;
const MODES: &[&str] = &[
    "none",
    "bounding-box",
    "shape",
    "alpha",
    "clipping-path",
    "jump-object",
    "jump-next-column",
];
const SIDES: &[&str] = &[
    "both",
    "left",
    "right",
    "towards-spine",
    "away-from-spine",
    "largest",
];
const ANCHOR_MODES: &[&str] = &["inline", "above-line", "custom"];
const POINTS: &[&str] = &[
    "top-left",
    "top",
    "top-right",
    "left",
    "center",
    "right",
    "bottom-left",
    "bottom",
    "bottom-right",
];
const RELATIVE_TO: &[&str] = &["anchor-marker", "column", "frame", "margin", "page"];
const ALIGNS: &[&str] = &["left", "center", "right"];

fn default_limit() -> usize {
    100
}

#[derive(Debug, Clone, clap::Args, Serialize, Deserialize, Default)]
pub struct AnchorOptions {
    /// inline, above-line or custom.
    #[arg(long)]
    pub mode: Option<String>,
    /// Inline: raise the object above the baseline by this much.
    #[arg(long, allow_negative_numbers = true)]
    pub baseline_offset: Option<f64>,
    /// Above-line: space before the object.
    #[arg(long)]
    pub space_before: Option<f64>,
    /// Above-line: space after the object.
    #[arg(long)]
    pub space_after: Option<f64>,
    /// Above-line: left, center or right within the column.
    #[arg(long)]
    pub align: Option<String>,
    /// Custom: the point of the object that is placed (top-left ... bottom-right).
    #[arg(long)]
    pub reference_point: Option<String>,
    /// Custom: the point of the relative-to box it is placed at.
    #[arg(long)]
    pub anchor_point: Option<String>,
    #[arg(long, allow_negative_numbers = true)]
    pub x_offset: Option<f64>,
    #[arg(long, allow_negative_numbers = true)]
    pub y_offset: Option<f64>,
    /// anchor-marker, column, frame, margin or page.
    #[arg(long)]
    pub relative_to: Option<String>,
    #[arg(long)]
    pub keep_within_frame: Option<bool>,
    #[arg(long)]
    pub prevent_manual_positioning: Option<bool>,
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WrapAction {
    /// Set the text wrap of an object.
    Set {
        id: String,
        /// none, bounding-box, shape, alpha, clipping-path, jump-object or jump-next-column.
        mode: String,
        /// top,right,bottom,left offsets in points (default 0,0,0,0).
        #[arg(long)]
        offset: Option<String>,
        /// both, left, right, towards-spine, away-from-spine or largest.
        #[arg(long)]
        side: Option<String>,
        /// Keep text inside the wrap shape instead of outside it.
        #[arg(long)]
        invert: Option<bool>,
        /// Alpha threshold 0-255 for alpha wrap.
        #[arg(long)]
        threshold: Option<u64>,
    },
    /// Remove the wrap of an object.
    Clear { id: String },
    /// Show the wrap of an object.
    Show { id: String },
    /// List wrapped objects (paginated).
    List {
        #[arg(long)]
        page: Option<String>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Free text segments per line of a text frame after wrap (read only).
    Intervals {
        frame: String,
        /// Line height in points (default: the story's first paragraph leading).
        #[arg(long)]
        line_height: Option<f64>,
        /// Number of lines to evaluate per column (default: as many as fit).
        #[arg(long)]
        lines: Option<usize>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Anchor an object into a story at a character offset of a paragraph.
    AnchorAdd {
        object: String,
        #[arg(long)]
        story: String,
        #[arg(long)]
        paragraph: usize,
        /// Character offset inside the paragraph (default: its end).
        #[arg(long)]
        at: Option<usize>,
        #[command(flatten)]
        options: AnchorOptions,
    },
    /// Change the anchoring options of an anchored object.
    AnchorSet {
        object: String,
        #[command(flatten)]
        options: AnchorOptions,
    },
    /// Detach an object from its anchor (the object stays where it is).
    AnchorRemove { object: String },
    /// List anchored objects with their estimated frame and page (paginated).
    AnchorList {
        #[arg(long)]
        story: Option<String>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Move anchored objects to follow their anchors (across frames and pages).
    AnchorApply {
        #[arg(long)]
        story: Option<String>,
        #[arg(long)]
        object: Option<String>,
    },
}

// ------------------------------------------------------------ helpers

fn milli(v: f64) -> i64 {
    (v * 1000.0).round() as i64
}

fn pt(v: i64) -> f64 {
    v as f64 / 1000.0
}

fn pages(raw: &Value) -> &[Value] {
    raw.get("pages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn require_v9(raw: &Value) -> Result<()> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] wrap and anchored objects need a version 9 document; run `pentool migrate --target 9` first"
    );
    Ok(())
}

fn one_of(value: &str, what: &str, allowed: &[&str]) -> Result<()> {
    ensure!(
        allowed.contains(&value),
        "[invalid-input] {what} `{value}` must be one of {}",
        allowed.join(", ")
    );
    Ok(())
}

fn finite(v: f64, what: &str, min: f64, max: f64) -> Result<f64> {
    ensure!(
        v.is_finite() && (min..=max).contains(&v),
        "[invalid-input] {what} must be a number between {min} and {max}"
    );
    Ok(v)
}

/// Visit every node with `(page index, layer index, node)`.
fn walk_nodes<'a>(raw: &'a Value, mut visit: impl FnMut(usize, usize, &'a Value)) {
    fn rec<'a>(
        nodes: &'a [Value],
        p: usize,
        l: usize,
        visit: &mut dyn FnMut(usize, usize, &'a Value),
    ) {
        for node in nodes {
            visit(p, l, node);
            rec(list(node, "children"), p, l, visit);
        }
    }
    for (p, page) in pages(raw).iter().enumerate() {
        for (l, layer) in list(page, "layers").iter().enumerate() {
            rec(list(layer, "nodes"), p, l, &mut visit);
        }
    }
}

fn find_node<'a>(raw: &'a Value, id: &str) -> Option<(usize, usize, &'a Value)> {
    let mut found = None;
    walk_nodes(raw, |p, l, n| {
        if found.is_none() && n.get("id").and_then(Value::as_str) == Some(id) {
            found = Some((p, l, n));
        }
    });
    found
}

fn find_node_mut<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    for node in nodes {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            if let Some(found) = find_node_mut(children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn node_mut<'a>(raw: &'a mut Value, p: usize, l: usize, id: &str) -> Option<&'a mut Value> {
    let nodes = raw
        .pointer_mut(&format!("/pages/{p}/layers/{l}/nodes"))?
        .as_array_mut()?;
    find_node_mut(nodes, id)
}

fn layer_locked(raw: &Value, p: usize, l: usize) -> bool {
    raw["pages"][p]["layers"][l]["locked"]
        .as_bool()
        .unwrap_or(false)
}

/// `(x, y, width, height)` in milli-points, or `None` for geometry-less nodes.
fn node_box(raw: &Value, page: usize, node: &Value) -> Option<(i64, i64, i64, i64)> {
    let n = |k: &str| node.get(k).and_then(Value::as_f64);
    if node.get("transform").is_none() {
        if let (Some(x), Some(y), Some(w), Some(h)) = (n("x"), n("y"), n("width"), n("height")) {
            return Some((milli(x), milli(y), milli(w), milli(h)));
        }
    }
    let id = node.get("id").and_then(Value::as_str)?;
    let page_id = pages(raw)[page].get("id").and_then(Value::as_str)?;
    let b = crate::scene::node_bounds(raw, Some(page_id), id).ok()?;
    let v = |k: &str| b[k].as_f64().unwrap_or(0.0);
    Some((
        milli(v("x")),
        milli(v("y")),
        milli(v("width")),
        milli(v("height")),
    ))
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ------------------------------------------------------------ validation

fn validate_wrap(id: &str, wrap: &Value) -> Result<()> {
    let o = wrap
        .as_object()
        .with_context(|| format!("[invalid-input] node {id}: wrap must be an object"))?;
    let mode = o
        .get("mode")
        .and_then(Value::as_str)
        .with_context(|| format!("[invalid-input] node {id}: wrap.mode is required"))?;
    one_of(mode, &format!("node {id}: wrap.mode"), MODES)?;
    if let Some(side) = o.get("side") {
        one_of(
            side.as_str().unwrap_or_default(),
            &format!("node {id}: wrap.side"),
            SIDES,
        )?;
    }
    if let Some(offset) = o.get("offset") {
        let values: Vec<f64> = offset
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        ensure!(
            values.len() == 4 && values.iter().all(|v| v.is_finite() && (-10_000.0..=10_000.0).contains(v)),
            "[invalid-input] node {id}: wrap.offset must be [top, right, bottom, left] numbers within +/-10000"
        );
    }
    if let Some(invert) = o.get("invert") {
        ensure!(
            invert.is_boolean(),
            "[invalid-input] node {id}: wrap.invert must be a boolean"
        );
    }
    if let Some(threshold) = o.get("threshold") {
        ensure!(
            threshold.as_u64().is_some_and(|t| t <= 255),
            "[invalid-input] node {id}: wrap.threshold must be 0-255"
        );
    }
    Ok(())
}

fn validate_anchor_run(
    story: &str,
    run: &Value,
    ids: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    let object = run
        .get("anchor")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .with_context(|| {
            format!("[malformed-resource] story {story}: an anchor run must name an object ID")
        })?;
    ensure!(
        ids.contains(object),
        "[missing-resource] story {story}: anchored object {object} does not exist"
    );
    ensure!(
        seen.insert(object.to_string()),
        "[invalid-input] object {object} is anchored more than once"
    );
    if let Some(mode) = run.get("mode") {
        one_of(
            mode.as_str().unwrap_or_default(),
            &format!("anchor {object}: mode"),
            ANCHOR_MODES,
        )?;
    }
    for key in ["reference_point", "anchor_point"] {
        if let Some(v) = run.get(key) {
            one_of(
                v.as_str().unwrap_or_default(),
                &format!("anchor {object}: {key}"),
                POINTS,
            )?;
        }
    }
    if let Some(v) = run.get("relative_to") {
        one_of(
            v.as_str().unwrap_or_default(),
            &format!("anchor {object}: relative_to"),
            RELATIVE_TO,
        )?;
    }
    if let Some(v) = run.get("align") {
        one_of(
            v.as_str().unwrap_or_default(),
            &format!("anchor {object}: align"),
            ALIGNS,
        )?;
    }
    for key in [
        "baseline_offset",
        "space_before",
        "space_after",
        "x_offset",
        "y_offset",
    ] {
        if let Some(v) = run.get(key) {
            ensure!(
                v.as_f64()
                    .is_some_and(|n| n.is_finite() && n.abs() <= 1_000_000.0),
                "[invalid-input] anchor {object}: {key} must be a finite number"
            );
        }
    }
    for key in ["keep_within_frame", "prevent_manual_positioning"] {
        if let Some(v) = run.get(key) {
            ensure!(
                v.is_boolean(),
                "[invalid-input] anchor {object}: {key} must be a boolean"
            );
        }
    }
    Ok(())
}

/// Validate node `wrap` objects and story anchor runs; called by the layout validator.
pub fn validate(raw: &Value) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut error: Option<anyhow::Error> = None;
    let mut per_page: BTreeMap<usize, usize> = BTreeMap::new();
    walk_nodes(raw, |p, _, node| {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            ids.insert(id.to_string());
        }
        if let Some(wrap) = node.get("wrap") {
            let id = node.get("id").and_then(Value::as_str).unwrap_or_default();
            if let Err(e) = validate_wrap(id, wrap) {
                error.get_or_insert(e);
            }
            if wrap.get("mode").and_then(Value::as_str) != Some("none") {
                *per_page.entry(p).or_default() += 1;
            }
        }
    });
    if let Some(e) = error {
        return Err(e);
    }
    for (page, count) in per_page {
        ensure!(
            count <= MAX_WRAPPERS_PER_PAGE,
            "[limit-exceeded] page {}: {count} wrapping objects exceeds the limit of {MAX_WRAPPERS_PER_PAGE}",
            pages(raw)[page]["id"].as_str().unwrap_or_default()
        );
    }
    let layout = raw.get("layout").unwrap_or(&Value::Null);
    let mut seen = BTreeSet::new();
    for story in list(layout, "stories") {
        let sid = story["id"].as_str().unwrap_or_default();
        for paragraph in list(story, "paragraphs") {
            for run in list(paragraph, "runs") {
                if run.get("anchor").is_some() {
                    validate_anchor_run(sid, run, &ids, &mut seen)?;
                    let object = run["anchor"].as_str().unwrap_or_default();
                    ensure!(
                        !crate::stories::is_frame_of_story(raw, object, sid),
                        "[invalid-input] story {sid}: a text frame of the story cannot be anchored inside it"
                    );
                }
            }
        }
    }
    Ok(())
}

/// First object carrying a `wrap` (version 8 cannot represent it).
pub fn first_wrapped(raw: &Value) -> Option<String> {
    let mut found = None;
    walk_nodes(raw, |_, _, node| {
        if found.is_none() && node.get("wrap").is_some() {
            found = node.get("id").and_then(Value::as_str).map(str::to_string);
        }
    });
    found
}

// ------------------------------------------------------------ wrap geometry

#[derive(Clone, Debug)]
struct Band {
    y0: i64,
    y1: i64,
    x0: i64,
    x1: i64,
}

struct Wrapper {
    id: String,
    mode: String,
    side: String,
    invert: bool,
    bands: Vec<Band>,
    approximated: Option<&'static str>,
    /// Top of the wrapped object's wrap area (for jump-next-column).
    top: i64,
}

fn offsets(wrap: &Value) -> [i64; 4] {
    let v: Vec<i64> = wrap
        .get("offset")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).map(milli).collect())
        .unwrap_or_default();
    if v.len() == 4 {
        [v[0], v[1], v[2], v[3]]
    } else {
        [0; 4]
    }
}

fn build_wrapper(raw: &Value, page: usize, node: &Value) -> Option<Wrapper> {
    let wrap = node.get("wrap")?;
    let mode = wrap.get("mode").and_then(Value::as_str)?;
    if mode == "none" {
        return None;
    }
    let (x, y, w, h) = node_box(raw, page, node)?;
    let [top, right, bottom, left] = offsets(wrap);
    let kind = node.get("kind").and_then(Value::as_str).unwrap_or_default();
    let mut approximated = None;
    let full = |x: i64, y: i64, w: i64, h: i64| Band {
        y0: y - top,
        y1: y + h + bottom,
        x0: x - left,
        x1: x + w + right,
    };
    let mut bands = Vec::new();
    let shape_like = matches!(mode, "shape" | "alpha" | "clipping-path");
    if shape_like && kind == "ellipse" && w > 0 && h > 0 {
        let n = ((h + 3999) / 4000).clamp(1, MAX_BANDS_PER_OBJECT);
        let (cx, cy) = (x as f64 + w as f64 / 2.0, y as f64 + h as f64 / 2.0);
        for i in 0..n {
            let y0 = y + h * i / n;
            let y1 = y + h * (i + 1) / n;
            let yc = (cy).clamp(y0 as f64, y1 as f64);
            let dy = (yc - cy) / (h as f64 / 2.0);
            let half = (w as f64 / 2.0) * (1.0 - dy * dy).max(0.0).sqrt();
            if half <= 0.0 {
                continue;
            }
            bands.push(Band {
                y0: if i == 0 { y0 - top } else { y0 },
                y1: if i == n - 1 { y1 + bottom } else { y1 },
                x0: (cx - half).round() as i64 - left,
                x1: (cx + half).round() as i64 + right,
            });
        }
        if bands.is_empty() {
            bands.push(full(x, y, w, h));
        }
    } else {
        if shape_like && kind != "rect" && kind != "fill" {
            approximated = Some(match mode {
                "alpha" => "alpha-mask-unavailable-using-bounding-box",
                "clipping-path" => "clipping-path-unavailable-using-bounding-box",
                _ => "shape-unavailable-for-kind-using-bounding-box",
            });
        }
        bands.push(full(x, y, w, h));
    }
    Some(Wrapper {
        id: node.get("id").and_then(Value::as_str)?.to_string(),
        mode: mode.to_string(),
        side: wrap
            .get("side")
            .and_then(Value::as_str)
            .unwrap_or("both")
            .to_string(),
        invert: wrap.get("invert").and_then(Value::as_bool).unwrap_or(false),
        top: y - top,
        bands,
        approximated,
    })
}

fn merge(mut intervals: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    intervals.retain(|(a, b)| b > a);
    intervals.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (a, b) in intervals {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn subtract(column: (i64, i64), blocked: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut at = column.0;
    for &(a, b) in blocked {
        if b <= at {
            continue;
        }
        if a >= column.1 {
            break;
        }
        if a > at {
            out.push((at, a.min(column.1)));
        }
        at = at.max(b);
    }
    if at < column.1 {
        out.push((at, column.1));
    }
    out
}

/// Resolve `side` for the page the frame sits on.
fn effective_side(side: &str, page_side: &str) -> &'static str {
    match (side, page_side) {
        ("left", _) => "left",
        ("right", _) => "right",
        ("largest", _) => "largest",
        ("towards-spine", "right") | ("away-from-spine", "left") => "left",
        ("towards-spine", "left") | ("away-from-spine", "right") => "right",
        _ => "both",
    }
}

fn free_segments(
    column: (i64, i64),
    line: (i64, i64),
    column_bottom: i64,
    wrappers: &[Wrapper],
    page_side: &str,
) -> Vec<(i64, i64)> {
    let mut blocked = Vec::new();
    for w in wrappers {
        let hits: Vec<&Band> = w
            .bands
            .iter()
            .filter(|b| b.y0 < line.1 && b.y1 > line.0 && b.x1 > column.0 && b.x0 < column.1)
            .collect();
        match w.mode.as_str() {
            "jump-object" => {
                if hits.iter().any(|_| true) {
                    return Vec::new();
                }
                continue;
            }
            "jump-next-column" => {
                if !w.bands.is_empty() && line.1 > w.top && w.top < column_bottom {
                    let overlaps_column =
                        w.bands.iter().any(|b| b.x1 > column.0 && b.x0 < column.1);
                    if overlaps_column {
                        return Vec::new();
                    }
                }
                continue;
            }
            _ => {}
        }
        if hits.is_empty() {
            if w.invert {
                // Inverted wrap: text lives only inside the shape.
                blocked.push(column);
            }
            continue;
        }
        let a = hits.iter().map(|b| b.x0).min().unwrap().max(column.0);
        let b = hits.iter().map(|b| b.x1).max().unwrap().min(column.1);
        if w.invert {
            blocked.push((column.0, a));
            blocked.push((b, column.1));
            continue;
        }
        match effective_side(&w.side, page_side) {
            "left" => blocked.push((a, column.1)),
            "right" => blocked.push((column.0, b)),
            "largest" => {
                let (left, right) = (a - column.0, column.1 - b);
                if left >= right {
                    blocked.push((a, column.1));
                } else {
                    blocked.push((column.0, b));
                }
            }
            _ => blocked.push((a, b)),
        }
    }
    subtract(column, &merge(blocked))
}

/// Wrapping objects on a page that affect a frame (never the frame itself, and
/// never objects anchored into the frame's own story).
fn page_wrappers(raw: &Value, page: usize, frame: &str, story: &str) -> Vec<Wrapper> {
    let anchored: BTreeSet<String> = list(raw.get("layout").unwrap_or(&Value::Null), "stories")
        .iter()
        .filter(|s| s["id"] == story)
        .flat_map(|s| list(s, "paragraphs"))
        .flat_map(|p| list(p, "runs"))
        .filter_map(|r| r.get("anchor").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut out = Vec::new();
    for layer in list(&pages(raw)[page], "layers") {
        if layer["visible"].as_bool() == Some(false) {
            continue;
        }
        fn rec(
            raw: &Value,
            page: usize,
            nodes: &[Value],
            frame: &str,
            anchored: &BTreeSet<String>,
            out: &mut Vec<Wrapper>,
        ) {
            for node in nodes {
                let id = node.get("id").and_then(Value::as_str).unwrap_or_default();
                if id != frame
                    && !anchored.contains(id)
                    && node.get("visible") != Some(&json!(false))
                {
                    if let Some(w) = build_wrapper(raw, page, node) {
                        out.push(w);
                    }
                }
                rec(raw, page, list(node, "children"), frame, anchored, out);
            }
        }
        rec(raw, page, list(layer, "nodes"), frame, &anchored, &mut out);
    }
    out
}

// ------------------------------------------------------------ anchors

fn point(name: &str, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    let (fx, fy) = match name {
        "top-left" => (0.0, 0.0),
        "top" => (0.5, 0.0),
        "top-right" => (1.0, 0.0),
        "left" => (0.0, 0.5),
        "right" => (1.0, 0.5),
        "bottom-left" => (0.0, 1.0),
        "bottom" => (0.5, 1.0),
        "bottom-right" => (1.0, 1.0),
        _ => (0.5, 0.5),
    };
    (x + w * fx, y + h * fy)
}

struct AnchorSite {
    story: String,
    paragraph: usize,
    run: usize,
    offset: usize,
    object: String,
    settings: Value,
}

fn anchor_sites(raw: &Value, only_story: Option<&str>) -> Vec<AnchorSite> {
    let mut out = Vec::new();
    for story in list(raw.get("layout").unwrap_or(&Value::Null), "stories") {
        let sid = story["id"].as_str().unwrap_or_default();
        if only_story.is_some_and(|s| s != sid) {
            continue;
        }
        let offsets = crate::stories::run_offsets(raw, sid);
        for (p, paragraph) in list(story, "paragraphs").iter().enumerate() {
            for (r, run) in list(paragraph, "runs").iter().enumerate() {
                if let Some(object) = run.get("anchor").and_then(Value::as_str) {
                    let offset = offsets
                        .iter()
                        .find(|(pp, rr, _)| *pp == p && *rr == r)
                        .map_or(0, |t| t.2);
                    out.push(AnchorSite {
                        story: sid.to_string(),
                        paragraph: p,
                        run: r,
                        offset,
                        object: object.to_string(),
                        settings: run.clone(),
                    });
                }
            }
        }
    }
    out
}

/// Frame (`Placed`) that shows story character `offset`, or the last frame.
fn host_frame(raw: &Value, story: &str, offset: usize) -> Option<(crate::stories::Placed, bool)> {
    let placed = crate::stories::placements(raw, story);
    let hit = placed
        .iter()
        .find(|p| offset >= p.start && offset < p.end)
        .cloned();
    match hit {
        Some(p) => Some((p, false)),
        None => placed.last().cloned().map(|p| (p, true)),
    }
}

/// Estimated top of the line holding `offset` inside the frame (column 0).
fn estimated_line_top(
    raw: &Value,
    story: &str,
    frame_start: usize,
    offset: usize,
    column_width: f64,
) -> (f64, f64, f64) {
    let source = list(raw.get("layout").unwrap_or(&Value::Null), "stories")
        .iter()
        .find(|s| s["id"] == story);
    let Some(source) = source else {
        return (0.0, 12.0, 14.4);
    };
    let mut y = 0.0;
    let mut start = 0usize;
    let mut last = (12.0, 14.4);
    for paragraph in list(source, "paragraphs") {
        let chars: usize = list(paragraph, "runs")
            .iter()
            .map(|r| crate::stories::run_chars(raw, r))
            .sum();
        let (size, leading) = crate::stories::paragraph_metrics(raw, paragraph);
        last = (size, leading);
        let per_line = ((column_width / (0.5 * size)).floor() as usize).max(1);
        let end = start + chars + 1;
        if end <= frame_start {
            start = end;
            continue;
        }
        let from = frame_start.max(start);
        if offset >= end {
            let shown = end - 1 - from.min(end - 1);
            y += (shown.div_ceil(per_line)).max(1) as f64 * leading;
            start = end;
            continue;
        }
        let before = offset.saturating_sub(from);
        y += (before / per_line) as f64 * leading;
        return (y, size, leading);
    }
    (y, last.0, last.1)
}

pub(crate) fn insert_anchor_run(
    paragraph: &mut Value,
    at: Option<usize>,
    run: Value,
) -> Result<()> {
    let runs = paragraph
        .get_mut("runs")
        .and_then(Value::as_array_mut)
        .context("paragraph runs are missing")?;
    let total: usize = runs
        .iter()
        .map(|r| match r.get("text").and_then(Value::as_str) {
            Some(t) => t.chars().count(),
            None => usize::from(r.get("special").is_some()),
        })
        .sum();
    let at = at.unwrap_or(total);
    ensure!(
        at <= total,
        "[invalid-input] --at {at} is beyond the paragraph ({total} characters)"
    );
    let mut seen = 0usize;
    for index in 0..runs.len() {
        let length = match runs[index].get("text").and_then(Value::as_str) {
            Some(t) => t.chars().count(),
            None => usize::from(runs[index].get("special").is_some()),
        };
        if at == seen {
            runs.insert(index, run);
            return Ok(());
        }
        if at < seen + length && runs[index].get("text").is_some() {
            let text = runs[index]["text"].as_str().unwrap_or_default().to_string();
            let cut = at - seen;
            let left: String = text.chars().take(cut).collect();
            let right: String = text.chars().skip(cut).collect();
            let mut a = runs[index].clone();
            a["text"] = json!(left);
            let mut b = runs[index].clone();
            b["text"] = json!(right);
            runs[index] = a;
            runs.insert(index + 1, run);
            runs.insert(index + 2, b);
            return Ok(());
        }
        seen += length;
    }
    runs.push(run);
    Ok(())
}

fn apply_options(run: &mut Map<String, Value>, o: &AnchorOptions) -> Result<()> {
    if let Some(v) = &o.mode {
        one_of(v, "--mode", ANCHOR_MODES)?;
        run.insert("mode".into(), json!(v));
    }
    for (value, key) in [
        (o.baseline_offset, "baseline_offset"),
        (o.space_before, "space_before"),
        (o.space_after, "space_after"),
        (o.x_offset, "x_offset"),
        (o.y_offset, "y_offset"),
    ] {
        if let Some(v) = value {
            finite(v, key, -1_000_000.0, 1_000_000.0)?;
            run.insert(key.into(), json!(v));
        }
    }
    for (value, key, allowed) in [
        (&o.align, "align", ALIGNS),
        (&o.reference_point, "reference_point", POINTS),
        (&o.anchor_point, "anchor_point", POINTS),
        (&o.relative_to, "relative_to", RELATIVE_TO),
    ] {
        if let Some(v) = value {
            one_of(v, key, allowed)?;
            run.insert(key.into(), json!(v));
        }
    }
    for (value, key) in [
        (o.keep_within_frame, "keep_within_frame"),
        (o.prevent_manual_positioning, "prevent_manual_positioning"),
    ] {
        if let Some(v) = value {
            run.insert(key.into(), json!(v));
        }
    }
    Ok(())
}

// ------------------------------------------------------------ run

fn page_label(raw: &Value, page: usize) -> Value {
    pages(raw)[page]["id"].clone()
}

/// Execute one wrap or anchor action. Returns the result and whether it was read only.
pub fn run(raw: &mut Value, action: WrapAction) -> Result<(Value, bool)> {
    require_v9(raw)?;
    match action {
        WrapAction::Set {
            id,
            mode,
            offset,
            side,
            invert,
            threshold,
        } => {
            one_of(&mode, "wrap mode", MODES)?;
            let (p, l, _) = find_node(raw, &id)
                .with_context(|| format!("[missing-resource] object not found: {id}"))?;
            ensure!(
                !layer_locked(raw, p, l),
                "[locked] layer {} is locked",
                raw["pages"][p]["layers"][l]["id"]
            );
            let mut wrap = json!({"mode": mode});
            if let Some(text) = offset {
                let values: Result<Vec<f64>, _> =
                    text.split(',').map(|v| v.trim().parse::<f64>()).collect();
                let values = values.map_err(|_| {
                    anyhow::anyhow!(
                        "[invalid-input] --offset must be top,right,bottom,left numbers"
                    )
                })?;
                ensure!(
                    values.len() == 4,
                    "[invalid-input] --offset must be top,right,bottom,left"
                );
                for v in &values {
                    finite(*v, "--offset", -10_000.0, 10_000.0)?;
                }
                wrap["offset"] = json!(values);
            }
            if let Some(side) = side {
                one_of(&side, "--side", SIDES)?;
                wrap["side"] = json!(side);
            }
            if let Some(invert) = invert {
                wrap["invert"] = json!(invert);
            }
            if let Some(t) = threshold {
                ensure!(t <= 255, "[invalid-input] --threshold must be 0-255");
                wrap["threshold"] = json!(t);
            }
            node_mut(raw, p, l, &id)
                .context("object vanished")?
                .as_object_mut()
                .context("object is not a map")?
                .insert("wrap".into(), wrap.clone());
            Ok((
                json!({"ok": true, "operation": "wrap-set", "object": id, "wrap": wrap}),
                false,
            ))
        }
        WrapAction::Clear { id } => {
            let (p, l, node) = find_node(raw, &id)
                .with_context(|| format!("[missing-resource] object not found: {id}"))?;
            ensure!(
                node.get("wrap").is_some(),
                "[invalid-input] object {id} has no wrap to clear"
            );
            ensure!(
                !layer_locked(raw, p, l),
                "[locked] layer {} is locked",
                raw["pages"][p]["layers"][l]["id"]
            );
            node_mut(raw, p, l, &id)
                .context("object vanished")?
                .as_object_mut()
                .context("object is not a map")?
                .remove("wrap");
            Ok((
                json!({"ok": true, "operation": "wrap-clear", "object": id}),
                false,
            ))
        }
        WrapAction::Show { id } => {
            let (p, _, node) = find_node(raw, &id)
                .with_context(|| format!("[missing-resource] object not found: {id}"))?;
            Ok((
                json!({"object": id, "page": page_label(raw, p), "wrap": node.get("wrap")}),
                true,
            ))
        }
        WrapAction::List {
            page,
            offset,
            limit,
        } => {
            ensure!(
                (1..=1000).contains(&limit),
                "[invalid-input] --limit must be between 1 and 1000"
            );
            let page_filter = match &page {
                Some(id) => Some(
                    pages(raw)
                        .iter()
                        .position(|p| p["id"] == id.as_str())
                        .with_context(|| format!("[missing-resource] page not found: {id}"))?,
                ),
                None => None,
            };
            let mut rows = Vec::new();
            walk_nodes(raw, |p, l, node| {
                if let Some(wrap) = node.get("wrap") {
                    if page_filter.is_none_or(|f| f == p) {
                        rows.push(json!({
                            "id": node["id"], "kind": node["kind"],
                            "page": pages(raw)[p]["id"],
                            "layer": raw["pages"][p]["layers"][l]["id"],
                            "wrap": wrap,
                        }));
                    }
                }
            });
            let slice: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
            Ok((
                json!({
                    "matches": rows.len(), "returned": slice.len(), "offset": offset,
                    "limit": limit, "has_more": offset + slice.len() < rows.len(),
                    "objects": slice,
                }),
                true,
            ))
        }
        WrapAction::Intervals {
            frame,
            line_height,
            lines,
            offset,
            limit,
        } => {
            ensure!(
                (1..=1000).contains(&limit),
                "[invalid-input] --limit must be between 1 and 1000"
            );
            let (p, _, node) = find_node(raw, &frame)
                .with_context(|| format!("[missing-resource] frame not found: {frame}"))?;
            ensure!(
                node["kind"] == "text-frame",
                "[invalid-input] {frame} is not a text frame"
            );
            let story = node["story"].as_str().unwrap_or_default().to_string();
            let fb = crate::stories::frame_box(node);
            let ignore = node
                .get("ignore_wrap")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let leading = line_height.unwrap_or_else(|| {
                list(raw.get("layout").unwrap_or(&Value::Null), "stories")
                    .iter()
                    .find(|s| s["id"] == story.as_str())
                    .and_then(|s| list(s, "paragraphs").first())
                    .map_or(14.4, |para| crate::stories::paragraph_metrics(raw, para).1)
            });
            finite(leading, "--line-height", 0.5, 10_000.0)?;
            let wrappers = if ignore {
                Vec::new()
            } else {
                page_wrappers(raw, p, &frame, &story)
            };
            let total_bands: usize = wrappers.iter().map(|w| w.bands.len()).sum();
            ensure!(
                total_bands <= MAX_INTERVALS,
                "[limit-exceeded] {total_bands} exclusion intervals exceeds the limit of {MAX_INTERVALS}"
            );
            let page_side =
                crate::spreads::resolve_page(raw, pages(raw)[p]["id"].as_str().unwrap_or_default())
                    .ok()
                    .and_then(|r| r["side"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "single".into());
            let inner_top = fb.y + fb.inset[0];
            let inner_height = (fb.height - fb.inset[0] - fb.inset[2]).max(0.0);
            let per_column =
                lines.unwrap_or_else(|| ((inner_height / leading).floor() as usize).max(1));
            ensure!(
                (1..=MAX_LINES).contains(&per_column),
                "[limit-exceeded] --lines must be between 1 and {MAX_LINES}"
            );
            let mut rows = Vec::new();
            let mut columns = Vec::new();
            let lh = milli(leading);
            for c in 0..fb.columns {
                let cx = fb.x + fb.inset[3] + c as f64 * (fb.column_width + fb.gutter);
                let column = (milli(cx), milli(cx + fb.column_width));
                let bottom = milli(inner_top + inner_height);
                columns.push(json!({
                    "index": c, "x": pt(column.0), "y": inner_top,
                    "width": fb.column_width, "height": inner_height,
                }));
                for k in 0..per_column {
                    let y0 = milli(inner_top) + k as i64 * lh;
                    let segs = free_segments(column, (y0, y0 + lh), bottom, &wrappers, &page_side);
                    rows.push(json!({
                        "column": c, "line": k, "y": pt(y0),
                        "segments": segs.iter().map(|(a, b)| json!([pt(*a), pt(*b)])).collect::<Vec<_>>(),
                    }));
                }
            }
            let slice: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
            Ok((
                json!({
                    "frame": frame, "page": page_label(raw, p), "story": story,
                    "ignore_wrap": ignore, "line_height": leading,
                    "columns": columns,
                    "wrappers": wrappers.iter().map(|w| json!({
                        "id": w.id, "mode": w.mode, "bands": w.bands.len(),
                        "approximated": w.approximated,
                    })).collect::<Vec<_>>(),
                    "exclusion_intervals": total_bands,
                    "matches": rows.len(), "returned": slice.len(), "offset": offset,
                    "limit": limit, "has_more": offset + slice.len() < rows.len(),
                    "lines": slice,
                }),
                true,
            ))
        }
        WrapAction::AnchorAdd {
            object,
            story,
            paragraph,
            at,
            options,
        } => {
            let (p, l, node) = find_node(raw, &object)
                .with_context(|| format!("[missing-resource] object not found: {object}"))?;
            ensure!(
                !layer_locked(raw, p, l),
                "[locked] layer {} is locked",
                raw["pages"][p]["layers"][l]["id"]
            );
            ensure!(
                node["kind"] != "text-frame"
                    || !crate::stories::is_frame_of_story(raw, &object, &story),
                "[invalid-input] a text frame of story {story} cannot be anchored inside it"
            );
            ensure!(
                node.get("x").is_some() && node.get("y").is_some(),
                "[invalid-input] object {object} has no x/y position; anchor a rect, ellipse, image or frame (groups are not supported)"
            );
            ensure!(
                anchor_sites(raw, None).iter().all(|s| s.object != object),
                "[invalid-input] object {object} is already anchored; use anchor-set or anchor-remove"
            );
            let mut run = Map::new();
            run.insert("anchor".into(), json!(object));
            apply_options(&mut run, &options)?;
            run.entry("mode").or_insert_with(|| json!("inline"));
            let stories = raw
                .pointer_mut("/layout/stories")
                .and_then(Value::as_array_mut)
                .with_context(|| format!("[missing-resource] story not found: {story}"))?;
            let target = stories
                .iter_mut()
                .find(|s| s["id"] == story.as_str())
                .with_context(|| format!("[missing-resource] story not found: {story}"))?;
            let paragraphs = target
                .get_mut("paragraphs")
                .and_then(Value::as_array_mut)
                .context("story paragraphs are missing")?;
            let count = paragraphs.len();
            let para = paragraphs.get_mut(paragraph).with_context(|| {
                format!("[invalid-input] paragraph {paragraph} is outside the story ({count} paragraphs)")
            })?;
            insert_anchor_run(para, at, Value::Object(run))?;
            Ok((
                json!({"ok": true, "operation": "anchor-add", "object": object, "story": story, "paragraph": paragraph}),
                false,
            ))
        }
        WrapAction::AnchorSet { object, options } => {
            let found = anchor_sites(raw, None)
                .into_iter()
                .find(|s| s.object == object)
                .with_context(|| format!("[missing-resource] object {object} is not anchored"))?;
            let run = raw
                .pointer_mut("/layout/stories")
                .and_then(Value::as_array_mut)
                .and_then(|stories| stories.iter_mut().find(|s| s["id"] == found.story.as_str()))
                .and_then(|s| {
                    s.pointer_mut(&format!(
                        "/paragraphs/{}/runs/{}",
                        found.paragraph, found.run
                    ))
                })
                .and_then(Value::as_object_mut)
                .context("anchor run vanished")?;
            apply_options(run, &options)?;
            Ok((
                json!({"ok": true, "operation": "anchor-set", "object": object}),
                false,
            ))
        }
        WrapAction::AnchorRemove { object } => {
            let found = anchor_sites(raw, None)
                .into_iter()
                .find(|s| s.object == object)
                .with_context(|| format!("[missing-resource] object {object} is not anchored"))?;
            let runs = raw
                .pointer_mut("/layout/stories")
                .and_then(Value::as_array_mut)
                .and_then(|stories| stories.iter_mut().find(|s| s["id"] == found.story.as_str()))
                .and_then(|s| s.pointer_mut(&format!("/paragraphs/{}/runs", found.paragraph)))
                .and_then(Value::as_array_mut)
                .context("anchor run vanished")?;
            runs.remove(found.run);
            Ok((
                json!({"ok": true, "operation": "anchor-remove", "object": object}),
                false,
            ))
        }
        WrapAction::AnchorList {
            story,
            offset,
            limit,
        } => {
            ensure!(
                (1..=1000).contains(&limit),
                "[invalid-input] --limit must be between 1 and 1000"
            );
            let sites = anchor_sites(raw, story.as_deref());
            let rows: Vec<Value> = sites
                .iter()
                .map(|s| {
                    let host = host_frame(raw, &s.story, s.offset);
                    json!({
                        "object": s.object, "story": s.story, "paragraph": s.paragraph,
                        "offset": s.offset,
                        "mode": s.settings.get("mode"),
                        "frame": host.as_ref().map(|(h, _)| h.frame.clone()),
                        "page": host.as_ref().map(|(h, _)| pages(raw)[h.page]["id"].clone()),
                        "overset": host.as_ref().is_some_and(|(_, o)| *o),
                        "estimated": true,
                    })
                })
                .collect();
            let slice: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
            Ok((
                json!({
                    "matches": rows.len(), "returned": slice.len(), "offset": offset,
                    "limit": limit, "has_more": offset + slice.len() < rows.len(),
                    "anchors": slice,
                }),
                true,
            ))
        }
        WrapAction::AnchorApply { story, object } => {
            let mut sites = anchor_sites(raw, story.as_deref());
            if let Some(object) = &object {
                sites.retain(|s| &s.object == object);
                ensure!(
                    !sites.is_empty(),
                    "[missing-resource] object {object} is not anchored"
                );
            }
            let mut moved = Vec::new();
            let mut skipped = Vec::new();
            for site in sites {
                let Some((host, overset)) = host_frame(raw, &site.story, site.offset) else {
                    skipped.push(json!({"object": site.object, "reason": "story-has-no-frames"}));
                    continue;
                };
                if overset {
                    skipped.push(json!({"object": site.object, "reason": "anchor-is-overset"}));
                    continue;
                }
                let frame_node = crate::stories::frame_node(raw, &host.frame)
                    .context("host frame vanished")?
                    .clone();
                let fb = crate::stories::frame_box(&frame_node);
                let (obj_page, obj_layer, obj) =
                    find_node(raw, &site.object).with_context(|| {
                        format!("[missing-resource] object not found: {}", site.object)
                    })?;
                let (w, h) = (
                    obj.get("width").and_then(Value::as_f64).unwrap_or(0.0),
                    obj.get("height").and_then(Value::as_f64).unwrap_or(0.0),
                );
                let (obj_page, obj_layer) = (obj_page, obj_layer);
                let (y_line, size, _) =
                    estimated_line_top(raw, &site.story, host.start, site.offset, fb.column_width);
                let column_x = fb.x + fb.inset[3];
                let top = fb.y + fb.inset[0];
                let marker_y = top + y_line;
                let settings = &site.settings;
                let mode = settings["mode"].as_str().unwrap_or("inline");
                let num = |k: &str| settings.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                let (mut nx, mut ny, mut consumed) = (column_x, marker_y, 0.0_f64);
                let _ = (&mut ny, &mut consumed);
                match mode {
                    "inline" => {
                        ny = marker_y + size - h - num("baseline_offset");
                        consumed = (h + num("baseline_offset") - size).max(0.0);
                    }
                    "above-line" => {
                        ny = marker_y - num("space_before") - h;
                        nx = match settings["align"].as_str().unwrap_or("left") {
                            "center" => column_x + (fb.column_width - w) / 2.0,
                            "right" => column_x + fb.column_width - w,
                            _ => column_x,
                        };
                        consumed = h + num("space_before") + num("space_after");
                    }
                    _ => {
                        let relative = settings["relative_to"].as_str().unwrap_or("anchor-marker");
                        let base = match relative {
                            "frame" => (fb.x, fb.y, fb.width, fb.height),
                            "column" => (
                                column_x,
                                top,
                                fb.column_width,
                                fb.height - fb.inset[0] - fb.inset[2],
                            ),
                            "page" => {
                                let c = &raw["pages"][host.page]["canvas"];
                                (
                                    0.0,
                                    0.0,
                                    c["width"].as_f64().unwrap_or(0.0),
                                    c["height"].as_f64().unwrap_or(0.0),
                                )
                            }
                            "margin" => {
                                let page_id =
                                    pages(raw)[host.page]["id"].as_str().unwrap_or_default();
                                let c = &raw["pages"][host.page]["canvas"];
                                let m: Vec<f64> = crate::spreads::resolve_page(raw, page_id)
                                    .ok()
                                    .and_then(|r| r["margins"].as_array().cloned())
                                    .map(|a| a.iter().filter_map(Value::as_f64).collect())
                                    .unwrap_or_else(|| vec![0.0; 4]);
                                let (cw, ch) = (
                                    c["width"].as_f64().unwrap_or(0.0),
                                    c["height"].as_f64().unwrap_or(0.0),
                                );
                                (m[1], m[0], cw - m[1] - m[3], ch - m[0] - m[2])
                            }
                            _ => (column_x, marker_y, 0.0, 0.0),
                        };
                        let (ax, ay) = point(
                            settings["anchor_point"].as_str().unwrap_or("top-left"),
                            base.0,
                            base.1,
                            base.2,
                            base.3,
                        );
                        let (rx, ry) = point(
                            settings["reference_point"].as_str().unwrap_or("top-left"),
                            0.0,
                            0.0,
                            w,
                            h,
                        );
                        nx = ax + num("x_offset") - rx;
                        ny = ay + num("y_offset") - ry;
                        if settings["keep_within_frame"].as_bool().unwrap_or(false) {
                            nx = nx.clamp(fb.x, (fb.x + fb.width - w).max(fb.x));
                            ny = ny.clamp(fb.y, (fb.y + fb.height - h).max(fb.y));
                        }
                    }
                }
                let (nx, ny) = (r3(nx), r3(ny));
                let target_page = host.page;
                let target_layer = pages(raw)[target_page]["layers"]
                    .as_array()
                    .and_then(|ls| ls.iter().position(|l| l["id"] == host.layer.as_str()))
                    .context("host layer vanished")?;
                ensure!(
                    !layer_locked(raw, obj_page, obj_layer)
                        && !layer_locked(raw, target_page, target_layer),
                    "[locked] the layer of {} or of its anchor frame is locked",
                    site.object
                );
                // Move between layers/pages when the anchor moved.
                let mut node = {
                    let nodes = raw
                        .pointer_mut(&format!("/pages/{obj_page}/layers/{obj_layer}/nodes"))
                        .and_then(Value::as_array_mut)
                        .context("layer nodes missing")?;
                    nodes
                        .iter()
                        .position(|n| n["id"] == site.object.as_str())
                        .map(|index| nodes.remove(index))
                };
                let (final_page, final_layer, in_place) = match &node {
                    Some(_) if (obj_page, obj_layer) != (target_page, target_layer) => {
                        (target_page, target_layer, false)
                    }
                    Some(_) => (obj_page, obj_layer, true),
                    None => (obj_page, obj_layer, true),
                };
                match node.as_mut() {
                    Some(n) => {
                        n["x"] = json!(nx);
                        n["y"] = json!(ny);
                        let nodes = raw
                            .pointer_mut(&format!("/pages/{final_page}/layers/{final_layer}/nodes"))
                            .and_then(Value::as_array_mut)
                            .context("layer nodes missing")?;
                        nodes.push(node.take().unwrap());
                    }
                    None => {
                        // Nested inside a group: update in place.
                        let n = node_mut(raw, obj_page, obj_layer, &site.object)
                            .context("anchored object vanished")?;
                        n["x"] = json!(nx);
                        n["y"] = json!(ny);
                    }
                }
                moved.push(json!({
                    "object": site.object, "mode": mode,
                    "frame": host.frame, "page": pages(raw)[target_page]["id"],
                    "x": nx, "y": ny, "space_consumed": r3(consumed),
                    "changed_page": !in_place, "estimated": true,
                }));
            }
            if moved.is_empty() && skipped.is_empty() {
                bail!("[invalid-input] no anchored objects found");
            }
            Ok((
                json!({"ok": true, "operation": "anchor-apply", "moved": moved, "skipped": skipped}),
                false,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_segments_split_around_an_object_and_respect_side() {
        let wrapper = |side: &str| Wrapper {
            id: "w".into(),
            mode: "bounding-box".into(),
            side: side.into(),
            invert: false,
            bands: vec![Band {
                y0: 0,
                y1: 100_000,
                x0: 40_000,
                x1: 60_000,
            }],
            approximated: None,
            top: 0,
        };
        let column = (0, 100_000);
        let both = free_segments(column, (0, 10_000), 200_000, &[wrapper("both")], "single");
        assert_eq!(both, vec![(0, 40_000), (60_000, 100_000)]);
        let left = free_segments(column, (0, 10_000), 200_000, &[wrapper("left")], "single");
        assert_eq!(left, vec![(0, 40_000)]);
        let spine = free_segments(
            column,
            (0, 10_000),
            200_000,
            &[wrapper("towards-spine")],
            "left",
        );
        assert_eq!(spine, vec![(60_000, 100_000)]);
        // Below the object the line is free again.
        let below = free_segments(
            column,
            (100_000, 110_000),
            200_000,
            &[wrapper("both")],
            "single",
        );
        assert_eq!(below, vec![(0, 100_000)]);
    }

    #[test]
    fn jump_modes_block_whole_lines() {
        let mut w = Wrapper {
            id: "w".into(),
            mode: "jump-object".into(),
            side: "both".into(),
            invert: false,
            bands: vec![Band {
                y0: 20_000,
                y1: 40_000,
                x0: 10_000,
                x1: 20_000,
            }],
            approximated: None,
            top: 20_000,
        };
        let column = (0, 100_000);
        assert!(free_segments(
            column,
            (25_000, 30_000),
            200_000,
            std::slice::from_ref(&w),
            "single"
        )
        .is_empty());
        assert_eq!(
            free_segments(
                column,
                (40_000, 50_000),
                200_000,
                std::slice::from_ref(&w),
                "single"
            ),
            vec![(0, 100_000)]
        );
        w.mode = "jump-next-column".into();
        assert!(free_segments(
            column,
            (150_000, 160_000),
            200_000,
            std::slice::from_ref(&w),
            "single"
        )
        .is_empty());
        assert_eq!(
            free_segments(
                column,
                (0, 10_000),
                200_000,
                std::slice::from_ref(&w),
                "single"
            ),
            vec![(0, 100_000)]
        );
    }
}

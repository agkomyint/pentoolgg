//! v0.14.0 task 7: baseline grids, guides, smart-guide measurement,
//! step-and-repeat, frame grids and graphic-frame fitting
//! (`docs/layout-v1.md` section 7).
//!
//! Everything is an ordinary mutation of `layout.baseline_grid`,
//! `layout.guides` or scene nodes; guides never render in exports.
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

pub const MAX_GUIDES: usize = 5000;
pub const MAX_REPEAT: u64 = 1000;
pub const MAX_GRID_CELLS: u64 = 1000;
pub const MAX_MEASURE: usize = 64;
const MAX_ID_LEN: usize = 128;
const KINDS: &[&str] = &["ruler-h", "ruler-v", "layout-grid"];
const FITS: &[&str] = &[
    "fill",
    "fit",
    "fit-content-to-frame",
    "fit-frame-to-content",
    "center",
    "proportional",
];

fn default_limit() -> usize {
    100
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum GridAction {
    /// Show the effective baseline grid (document, or one frame's).
    BaselineShow {
        #[arg(long)]
        frame: Option<String>,
    },
    /// Set the document baseline grid, or one frame's grid with --frame.
    BaselineSet {
        #[arg(long)]
        frame: Option<String>,
        #[arg(long)]
        increment: f64,
        #[arg(long)]
        start: Option<f64>,
        /// page or margin.
        #[arg(long)]
        relative_to: Option<String>,
        #[arg(long)]
        color: Option<String>,
    },
    /// Remove the document grid, or reset a frame to inherit (--off disables it instead).
    BaselineClear {
        #[arg(long)]
        frame: Option<String>,
        /// With --frame: turn the grid off for that frame.
        #[arg(long)]
        #[serde(default)]
        off: bool,
    },
    /// Snap a vertical position up to the next grid line (read only).
    BaselineSnap {
        y: f64,
        #[arg(long)]
        frame: Option<String>,
        #[arg(long)]
        page: Option<String>,
    },
    /// List guides (paginated), optionally with margin and column guides derived from a page.
    GuideList {
        #[arg(long)]
        page: Option<String>,
        /// Include margin and column guides derived from the page geometry.
        #[arg(long)]
        #[serde(default)]
        derived: bool,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Add a ruler guide (ruler-h is a horizontal line at a y position, ruler-v a vertical line at x).
    GuideAdd {
        id: String,
        #[arg(long)]
        page: String,
        /// ruler-h or ruler-v.
        #[arg(long)]
        kind: String,
        #[arg(long, allow_negative_numbers = true)]
        position: f64,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        #[serde(default)]
        locked: bool,
    },
    /// Add a layout grid (rows x columns inside the margin box or the page).
    GuideGrid {
        id: String,
        #[arg(long)]
        page: String,
        #[arg(long)]
        rows: u64,
        #[arg(long)]
        columns: u64,
        #[arg(long, default_value_t = 0.0)]
        #[serde(default)]
        gutter_x: f64,
        #[arg(long, default_value_t = 0.0)]
        #[serde(default)]
        gutter_y: f64,
        /// margin (default) or page.
        #[arg(long)]
        relative_to: Option<String>,
    },
    /// Move a ruler guide.
    GuideSet {
        id: String,
        #[arg(long, allow_negative_numbers = true)]
        position: f64,
    },
    /// Remove one guide by ID, or all guides of a page with --page (not both).
    GuideRemove {
        /// Guide ID; give this or --page, not both.
        id: Option<String>,
        /// Remove every guide of this page; give this or a guide ID, not both.
        #[arg(long)]
        page: Option<String>,
    },
    /// Measure alignment and spacing between objects (smart guides; read only).
    Measure {
        #[arg(long, value_delimiter = ',')]
        ids: Vec<String>,
        #[arg(long)]
        page: Option<String>,
        /// Alignment tolerance in points.
        #[arg(long, default_value_t = 0.5)]
        #[serde(default = "default_tolerance")]
        tolerance: f64,
    },
    /// Duplicate an object COUNT times at a fixed offset.
    StepRepeat {
        id: String,
        #[arg(long)]
        count: u64,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        #[serde(default)]
        dx: f64,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        #[serde(default)]
        dy: f64,
        /// New IDs are PREFIX1, PREFIX2, ... (default: ID-copy-).
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Create a rows x columns grid of frames (graphic frames, or threaded text frames with --story).
    FrameGrid {
        prefix: String,
        #[arg(long)]
        page: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        rows: u64,
        #[arg(long)]
        columns: u64,
        #[arg(long)]
        x: f64,
        #[arg(long)]
        y: f64,
        /// Total width of the grid.
        #[arg(long)]
        width: f64,
        /// Total height of the grid.
        #[arg(long)]
        height: f64,
        #[arg(long, default_value_t = 12.0)]
        #[serde(default = "default_gutter")]
        gutter_x: f64,
        #[arg(long, default_value_t = 12.0)]
        #[serde(default = "default_gutter")]
        gutter_y: f64,
        /// Thread text frames of this story in reading order instead of making graphic frames.
        #[arg(long)]
        story: Option<String>,
    },
    /// Fit the content of a graphic frame: fill, fit, fit-content-to-frame,
    /// fit-frame-to-content, center or proportional.
    Fit {
        frame: String,
        mode: String,
        /// Content node to fit (default: the frame's `content`).
        #[arg(long)]
        content: Option<String>,
    },
}

fn default_tolerance() -> f64 {
    0.5
}
fn default_gutter() -> f64 {
    12.0
}

// ------------------------------------------------------------ helpers

fn milli(v: f64) -> i64 {
    (v * 1000.0).round() as i64
}

fn from_milli(v: i64) -> f64 {
    v as f64 / 1000.0
}

fn finite(v: f64, what: &str, min: f64, max: f64) -> Result<f64> {
    ensure!(
        v.is_finite() && (min..=max).contains(&v),
        "[invalid-input] {what} must be a number between {min} and {max}"
    );
    Ok(v)
}

fn check_id(what: &str, id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.chars().count() <= MAX_ID_LEN
            && !id.chars().any(|c| c.is_control() || c == '/' || c == '\\'),
        "[invalid-input] {what} ID {id:?} must be 1-{MAX_ID_LEN} characters without control characters, '/' or '\\'"
    );
    Ok(())
}

fn require_v9(raw: &Value) -> Result<()> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] grids and guides need a version 9 document; run `pentool migrate --target 9` first"
    );
    Ok(())
}

fn layout_mut(raw: &mut Value) -> Result<&mut Map<String, Value>> {
    let object = raw.as_object_mut().context("document is not an object")?;
    let layout = object
        .entry("layout")
        .or_insert_with(|| json!({"engine": crate::spreads::ENGINE}));
    layout.as_object_mut().context("layout is not an object")
}

fn pages(raw: &Value) -> &[Value] {
    raw.get("pages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn page_index(raw: &Value, id: &str) -> Result<usize> {
    pages(raw)
        .iter()
        .position(|p| p["id"] == id)
        .with_context(|| format!("[missing-resource] page not found: {id}"))
}

fn guide_list(raw: &Value) -> &[Value] {
    raw.pointer("/layout/guides")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn node_ids(nodes: &[Value], out: &mut BTreeSet<String>) {
    for node in nodes {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            out.insert(id.to_string());
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            node_ids(children, out);
        }
    }
}

fn all_ids(raw: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for page in pages(raw) {
        if let Some(id) = page.get("id").and_then(Value::as_str) {
            out.insert(id.to_string());
        }
        for layer in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = layer.get("id").and_then(Value::as_str) {
                out.insert(id.to_string());
            }
            if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
                node_ids(nodes, &mut out);
            }
        }
    }
    out
}

// ------------------------------------------------------------ baseline grid

fn validate_grid(grid: &Value, what: &str) -> Result<()> {
    let o = grid
        .as_object()
        .with_context(|| format!("[invalid-input] {what} must be an object"))?;
    let inc = o
        .get("increment")
        .and_then(Value::as_f64)
        .with_context(|| format!("[invalid-input] {what}.increment is required"))?;
    ensure!(
        inc.is_finite() && inc > 0.0 && inc <= 10_000.0,
        "[invalid-input] {what}.increment must be greater than 0 and at most 10000"
    );
    if let Some(start) = o.get("start") {
        let s = start
            .as_f64()
            .with_context(|| format!("[invalid-input] {what}.start must be a number"))?;
        ensure!(
            (0.0..=1_000_000.0).contains(&s),
            "[invalid-input] {what}.start must be between 0 and 1000000"
        );
    }
    if let Some(r) = o.get("relative_to") {
        ensure!(
            matches!(r.as_str(), Some("page" | "margin")),
            "[invalid-input] {what}.relative_to must be `page` or `margin`"
        );
    }
    if let Some(c) = o.get("color") {
        ensure!(
            c.as_str().is_some_and(|s| s.len() <= 32),
            "[invalid-input] {what}.color must be a short color string"
        );
    }
    Ok(())
}

/// Validate `layout.guides` and `layout.baseline_grid`; called by the shared layout validator.
pub fn validate(raw: &Value) -> Result<()> {
    if let Some(grid) = raw.pointer("/layout/baseline_grid") {
        validate_grid(grid, "layout.baseline_grid")?;
    }
    let Some(guides) = raw.pointer("/layout/guides") else {
        return Ok(());
    };
    let guides = guides
        .as_array()
        .context("[invalid-input] layout.guides must be an array")?;
    ensure!(
        guides.len() <= MAX_GUIDES,
        "[limit-exceeded] a document is limited to {MAX_GUIDES} guides"
    );
    let page_ids: BTreeSet<&str> = pages(raw).iter().filter_map(|p| p["id"].as_str()).collect();
    let mut seen = BTreeSet::new();
    for guide in guides {
        let id = guide
            .get("id")
            .and_then(Value::as_str)
            .context("[invalid-input] every guide needs an `id`")?;
        ensure!(seen.insert(id), "[invalid-input] duplicate guide `{id}`");
        let kind = guide.get("kind").and_then(Value::as_str);
        // Guides authored by other tools may use other kinds; only known kinds are checked.
        let Some(kind) = kind.filter(|k| KINDS.contains(k)) else {
            continue;
        };
        if let Some(page) = guide.get("page").and_then(Value::as_str) {
            ensure!(
                page_ids.contains(page),
                "[invalid-input] guide `{id}` references missing page `{page}`"
            );
        }
        match kind {
            "ruler-h" | "ruler-v" => {
                ensure!(
                    guide
                        .get("position")
                        .and_then(Value::as_f64)
                        .is_some_and(f64::is_finite),
                    "[invalid-input] guide `{id}` needs a finite `position`"
                );
            }
            _ => {
                for key in ["rows", "columns"] {
                    let n = guide.get(key).and_then(Value::as_u64).unwrap_or(0);
                    ensure!(
                        (1..=200).contains(&n),
                        "[invalid-input] layout grid `{id}` {key} must be between 1 and 200"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Grid in force for a frame (`None` when the frame turns it off or none exists).
fn effective_grid(raw: &Value, frame: Option<&str>) -> Result<(Option<Value>, &'static str)> {
    if let Some(id) = frame {
        let node = crate::stories::frame_node(raw, id)
            .with_context(|| format!("[missing-resource] frame not found: {id}"))?;
        match node.get("baseline_grid") {
            Some(Value::String(s)) if s == "off" => return Ok((None, "frame-off")),
            Some(grid) if grid.is_object() => return Ok((Some(grid.clone()), "frame")),
            _ => {}
        }
    }
    Ok((raw.pointer("/layout/baseline_grid").cloned(), "document"))
}

/// Smallest grid line at or below-after `y`: rounds **up** so text never moves up.
pub fn snap_up(y: f64, origin: f64, start: f64, increment: f64) -> f64 {
    let base = milli(origin + start);
    let inc = milli(increment).max(1);
    let at = milli(y);
    let steps = (at - base).div_euclid(inc) + i64::from((at - base).rem_euclid(inc) != 0);
    from_milli(base + steps * inc)
}

fn grid_origin(raw: &Value, page: Option<&str>, grid: &Value) -> Result<f64> {
    if grid.get("relative_to").and_then(Value::as_str) != Some("margin") {
        return Ok(0.0);
    }
    let page = match page {
        Some(p) => p.to_string(),
        None => pages(raw)
            .first()
            .and_then(|p| p["id"].as_str())
            .unwrap_or_default()
            .to_string(),
    };
    let resolved = crate::spreads::resolve_page(raw, &page)?;
    Ok(resolved["margins"][0].as_f64().unwrap_or(0.0))
}

// ------------------------------------------------------------ guides

fn page_box(raw: &Value, page: usize) -> (f64, f64) {
    let canvas = &pages(raw)[page]["canvas"];
    (
        canvas["width"].as_f64().unwrap_or(0.0),
        canvas["height"].as_f64().unwrap_or(0.0),
    )
}

/// Margin and column guides computed from the resolved page geometry.
fn derived_guides(raw: &Value, page: &str) -> Result<Vec<Value>> {
    let index = page_index(raw, page)?;
    let (w, h) = page_box(raw, index);
    let resolved = crate::spreads::resolve_page(raw, page)?;
    let m: Vec<f64> = resolved["margins"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    ensure!(
        m.len() == 4,
        "[malformed-resource] page margins are invalid"
    );
    let (top, left, bottom, right) = (m[0], m[1], m[2], m[3]);
    let columns = resolved["columns"].as_u64().unwrap_or(1).max(1);
    let gutter = resolved["gutter"].as_f64().unwrap_or(0.0);
    let mut out = vec![
        json!({"kind": "margin", "orientation": "h", "position": top, "page": page}),
        json!({"kind": "margin", "orientation": "h", "position": h - bottom, "page": page}),
        json!({"kind": "margin", "orientation": "v", "position": left, "page": page}),
        json!({"kind": "margin", "orientation": "v", "position": w - right, "page": page}),
    ];
    if columns > 1 {
        let inner = w - left - right;
        let col = (inner - gutter * (columns - 1) as f64) / columns as f64;
        for i in 0..columns {
            let start = left + i as f64 * (col + gutter);
            out.push(json!({"kind": "column", "orientation": "v", "position": start, "page": page, "column": i + 1, "edge": "start"}));
            out.push(json!({"kind": "column", "orientation": "v", "position": start + col, "page": page, "column": i + 1, "edge": "end"}));
        }
    }
    Ok(out)
}

// ------------------------------------------------------------ node access

fn find_mut<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    for node in nodes {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            if let Some(found) = find_mut(children, id) {
                return Some(found);
            }
        }
    }
    None
}

/// `(page, layer, node)` indexes for a top-level or nested node.
fn locate(raw: &Value, id: &str) -> Option<(usize, usize)> {
    fn has(nodes: &[Value], id: &str) -> bool {
        nodes.iter().any(|n| {
            n.get("id").and_then(Value::as_str) == Some(id)
                || n.get("children")
                    .and_then(Value::as_array)
                    .is_some_and(|c| has(c, id))
        })
    }
    for (p, page) in pages(raw).iter().enumerate() {
        for (l, layer) in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            if layer
                .get("nodes")
                .and_then(Value::as_array)
                .is_some_and(|n| has(n, id))
            {
                return Some((p, l));
            }
        }
    }
    None
}

fn node_ref<'a>(raw: &'a Value, id: &str) -> Option<&'a Value> {
    fn find<'a>(nodes: &'a [Value], id: &str) -> Option<&'a Value> {
        for node in nodes {
            if node.get("id").and_then(Value::as_str) == Some(id) {
                return Some(node);
            }
            if let Some(found) = node
                .get("children")
                .and_then(Value::as_array)
                .and_then(|c| find(c, id))
            {
                return Some(found);
            }
        }
        None
    }
    let (p, l) = locate(raw, id)?;
    let nodes = raw["pages"][p]["layers"][l]["nodes"].as_array()?;
    find(nodes, id)
}

fn locked_layer(raw: &Value, page: usize, layer: usize) -> bool {
    raw["pages"][page]["layers"][layer]["locked"]
        .as_bool()
        .unwrap_or(false)
}

fn bounds(raw: &Value, page: usize, id: &str) -> Result<(f64, f64, f64, f64)> {
    let page_id = pages(raw)[page]["id"].as_str().unwrap_or_default();
    let b = crate::scene::node_bounds(raw, Some(page_id), id)?;
    let n = |k: &str| b[k].as_f64().unwrap_or(0.0);
    Ok((n("x"), n("y"), n("width"), n("height")))
}

fn offset_node(node: &mut Value, dx: f64, dy: f64) {
    if let Some(o) = node.as_object_mut() {
        for (key, delta) in [("x", dx), ("y", dy)] {
            if let Some(v) = o.get(key).and_then(Value::as_f64) {
                o.insert(key.into(), json!(v + delta));
            }
        }
    }
}

fn rename_tree(node: &mut Value, suffix: &str, root: &str, new_root: &str) {
    if let Some(o) = node.as_object_mut() {
        if let Some(id) = o.get("id").and_then(Value::as_str) {
            let renamed = if id == root {
                new_root.to_string()
            } else {
                format!("{id}{suffix}")
            };
            o.insert("id".into(), json!(renamed));
        }
        if let Some(children) = o.get_mut("children").and_then(Value::as_array_mut) {
            for child in children {
                rename_tree(child, suffix, root, new_root);
            }
        }
    }
}

// ------------------------------------------------------------ run

/// Execute one grid or guide action. Returns the result and whether it was read only.
pub fn run(raw: &mut Value, action: GridAction) -> Result<(Value, bool)> {
    require_v9(raw)?;
    match action {
        GridAction::BaselineShow { frame } => {
            let (grid, source) = effective_grid(raw, frame.as_deref())?;
            Ok((
                json!({"grid": grid, "source": source, "frame": frame}),
                true,
            ))
        }
        GridAction::BaselineSet {
            frame,
            increment,
            start,
            relative_to,
            color,
        } => {
            finite(increment, "--increment", 0.001, 10_000.0)?;
            let mut grid = json!({"increment": increment});
            if let Some(s) = start {
                grid["start"] = json!(finite(s, "--start", 0.0, 1_000_000.0)?);
            }
            if let Some(r) = relative_to {
                ensure!(
                    matches!(r.as_str(), "page" | "margin"),
                    "[invalid-input] --relative-to must be `page` or `margin`"
                );
                grid["relative_to"] = json!(r);
            }
            if let Some(c) = color {
                grid["color"] = json!(c);
            }
            validate_grid(&grid, "baseline grid")?;
            match &frame {
                Some(id) => {
                    let node = crate::stories::frame_node_mut(raw, id)
                        .with_context(|| format!("[missing-resource] frame not found: {id}"))?;
                    node["baseline_grid"] = grid.clone();
                }
                None => {
                    layout_mut(raw)?.insert("baseline_grid".into(), grid.clone());
                }
            }
            Ok((
                json!({"ok": true, "operation": "baseline-set", "frame": frame, "grid": grid}),
                false,
            ))
        }
        GridAction::BaselineClear { frame, off } => {
            match &frame {
                Some(id) => {
                    let node = crate::stories::frame_node_mut(raw, id)
                        .with_context(|| format!("[missing-resource] frame not found: {id}"))?;
                    let o = node.as_object_mut().expect("frames are objects");
                    if off {
                        o.insert("baseline_grid".into(), json!("off"));
                    } else {
                        o.remove("baseline_grid");
                    }
                }
                None => {
                    ensure!(!off, "[invalid-input] --off needs --frame");
                    layout_mut(raw)?.remove("baseline_grid");
                }
            }
            Ok((
                json!({"ok": true, "operation": "baseline-clear", "frame": frame, "off": off}),
                false,
            ))
        }
        GridAction::BaselineSnap { y, frame, page } => {
            finite(y, "y", -1_000_000.0, 1_000_000.0)?;
            let (grid, source) = effective_grid(raw, frame.as_deref())?;
            let Some(grid) = grid else {
                return Ok((
                    json!({"y": y, "snapped": y, "grid": null, "source": source, "moved": 0}),
                    true,
                ));
            };
            let origin = grid_origin(raw, page.as_deref(), &grid)?;
            let start = grid.get("start").and_then(Value::as_f64).unwrap_or(0.0);
            let inc = grid["increment"].as_f64().unwrap_or(1.0);
            let snapped = snap_up(y, origin, start, inc);
            Ok((
                json!({"y": y, "snapped": snapped, "moved": snapped - y, "grid": grid, "source": source}),
                true,
            ))
        }
        GridAction::GuideList {
            page,
            derived,
            offset,
            limit,
        } => {
            ensure!(
                (1..=1000).contains(&limit),
                "[invalid-input] --limit must be between 1 and 1000"
            );
            if derived {
                ensure!(
                    page.is_some(),
                    "[invalid-input] --derived needs --page to know whose margins to use"
                );
            }
            if let Some(p) = &page {
                page_index(raw, p)?;
            }
            let mut rows: Vec<Value> = guide_list(raw)
                .iter()
                .filter(|g| {
                    page.as_deref()
                        .is_none_or(|p| g.get("page").and_then(Value::as_str) == Some(p))
                })
                .cloned()
                .collect();
            if derived {
                rows.extend(derived_guides(raw, page.as_deref().unwrap())?);
            }
            let slice: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
            Ok((
                json!({
                    "matches": rows.len(),
                    "returned": slice.len(),
                    "offset": offset,
                    "limit": limit,
                    "has_more": offset + slice.len() < rows.len(),
                    "guides": slice,
                }),
                true,
            ))
        }
        GridAction::GuideAdd {
            id,
            page,
            kind,
            position,
            color,
            locked,
        } => {
            check_id("guide", &id)?;
            ensure!(
                matches!(kind.as_str(), "ruler-h" | "ruler-v"),
                "[invalid-input] --kind must be `ruler-h` or `ruler-v`"
            );
            finite(position, "--position", -1_000_000.0, 1_000_000.0)?;
            page_index(raw, &page)?;
            ensure!(
                guide_list(raw).iter().all(|g| g["id"] != id.as_str()),
                "[invalid-input] guide ID already exists: {id}"
            );
            ensure!(
                guide_list(raw).len() < MAX_GUIDES,
                "[limit-exceeded] a document is limited to {MAX_GUIDES} guides"
            );
            let mut guide = json!({"id": id, "kind": kind, "page": page, "position": position});
            if let Some(c) = color {
                guide["color"] = json!(c);
            }
            if locked {
                guide["locked"] = json!(true);
            }
            layout_mut(raw)?
                .entry("guides")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .context("layout.guides must be an array")?
                .push(guide);
            Ok((
                json!({"ok": true, "operation": "guide-add", "guide": id}),
                false,
            ))
        }
        GridAction::GuideGrid {
            id,
            page,
            rows,
            columns,
            gutter_x,
            gutter_y,
            relative_to,
        } => {
            check_id("guide", &id)?;
            ensure!(
                (1..=200).contains(&rows) && (1..=200).contains(&columns),
                "[invalid-input] --rows and --columns must be between 1 and 200"
            );
            finite(gutter_x, "--gutter-x", 0.0, 100_000.0)?;
            finite(gutter_y, "--gutter-y", 0.0, 100_000.0)?;
            let relative = relative_to.unwrap_or_else(|| "margin".into());
            ensure!(
                matches!(relative.as_str(), "margin" | "page"),
                "[invalid-input] --relative-to must be `margin` or `page`"
            );
            page_index(raw, &page)?;
            ensure!(
                guide_list(raw).iter().all(|g| g["id"] != id.as_str()),
                "[invalid-input] guide ID already exists: {id}"
            );
            ensure!(
                guide_list(raw).len() < MAX_GUIDES,
                "[limit-exceeded] a document is limited to {MAX_GUIDES} guides"
            );
            let guide = json!({
                "id": id, "kind": "layout-grid", "page": page, "rows": rows,
                "columns": columns, "gutter_x": gutter_x, "gutter_y": gutter_y,
                "relative_to": relative,
            });
            layout_mut(raw)?
                .entry("guides")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .context("layout.guides must be an array")?
                .push(guide);
            Ok((
                json!({"ok": true, "operation": "guide-grid", "guide": id}),
                false,
            ))
        }
        GridAction::GuideSet { id, position } => {
            finite(position, "--position", -1_000_000.0, 1_000_000.0)?;
            let guides = raw
                .pointer_mut("/layout/guides")
                .and_then(Value::as_array_mut)
                .with_context(|| format!("[missing-resource] guide not found: {id}"))?;
            let guide = guides
                .iter_mut()
                .find(|g| g["id"] == id.as_str())
                .with_context(|| format!("[missing-resource] guide not found: {id}"))?;
            ensure!(
                matches!(guide["kind"].as_str(), Some("ruler-h" | "ruler-v")),
                "[invalid-input] guide `{id}` is not a ruler guide"
            );
            ensure!(
                !guide["locked"].as_bool().unwrap_or(false),
                "[locked] guide `{id}` is locked"
            );
            guide["position"] = json!(position);
            Ok((
                json!({"ok": true, "operation": "guide-set", "guide": id}),
                false,
            ))
        }
        GridAction::GuideRemove { id, page } => {
            ensure!(
                id.is_some() != page.is_some(),
                "[invalid-input] give either a guide ID or --page"
            );
            let Some(guides) = raw
                .pointer_mut("/layout/guides")
                .and_then(Value::as_array_mut)
            else {
                bail!("[missing-resource] the document has no guides");
            };
            let before = guides.len();
            match (&id, &page) {
                (Some(id), _) => {
                    ensure!(
                        guides.iter().any(|g| g["id"] == id.as_str()),
                        "[missing-resource] guide not found: {id}"
                    );
                    guides.retain(|g| g["id"] != id.as_str());
                }
                (_, Some(page)) => {
                    guides.retain(|g| g.get("page").and_then(Value::as_str) != Some(page.as_str()));
                }
                _ => unreachable!(),
            }
            Ok((
                json!({"ok": true, "operation": "guide-remove", "removed": before - guides.len()}),
                false,
            ))
        }
        GridAction::Measure {
            ids,
            page,
            tolerance,
        } => {
            ensure!(
                (2..=MAX_MEASURE).contains(&ids.len()),
                "[invalid-input] --ids needs between 2 and {MAX_MEASURE} object IDs"
            );
            finite(tolerance, "--tolerance", 0.0, 1000.0)?;
            let mut boxes = Vec::new();
            for id in &ids {
                let (p, _) = locate(raw, id)
                    .with_context(|| format!("[missing-resource] object not found: {id}"))?;
                if let Some(want) = &page {
                    ensure!(
                        pages(raw)[p]["id"] == want.as_str(),
                        "[invalid-input] object {id} is not on page {want}"
                    );
                }
                let (x, y, w, h) = bounds(raw, p, id)?;
                boxes.push((id.clone(), p, x, y, w, h));
            }
            ensure!(
                boxes.iter().all(|b| b.1 == boxes[0].1),
                "[invalid-input] measured objects must be on the same page"
            );
            let mut alignments = Vec::new();
            let mut distances = Vec::new();
            for i in 0..boxes.len() {
                for j in i + 1..boxes.len() {
                    let (a, b) = (&boxes[i], &boxes[j]);
                    let edges = |s: &(String, usize, f64, f64, f64, f64)| {
                        [
                            ("left", s.2),
                            ("hcenter", s.2 + s.4 / 2.0),
                            ("right", s.2 + s.4),
                            ("top", s.3),
                            ("vcenter", s.3 + s.5 / 2.0),
                            ("bottom", s.3 + s.5),
                        ]
                    };
                    for (name, va) in edges(a) {
                        for (other, vb) in edges(b) {
                            if name == other && (va - vb).abs() <= tolerance {
                                alignments.push(json!({
                                    "a": a.0, "b": b.0, "edge": name,
                                    "offset": (va - vb).abs(),
                                }));
                            }
                        }
                    }
                    let gap_x = if a.2 + a.4 <= b.2 {
                        b.2 - (a.2 + a.4)
                    } else if b.2 + b.4 <= a.2 {
                        a.2 - (b.2 + b.4)
                    } else {
                        0.0
                    };
                    let gap_y = if a.3 + a.5 <= b.3 {
                        b.3 - (a.3 + a.5)
                    } else if b.3 + b.5 <= a.3 {
                        a.3 - (b.3 + b.5)
                    } else {
                        0.0
                    };
                    distances.push(json!({
                        "a": a.0, "b": b.0, "horizontal_gap": gap_x, "vertical_gap": gap_y,
                        "overlap": gap_x == 0.0 && gap_y == 0.0,
                    }));
                }
            }
            // Equal spacing among objects sorted along each axis.
            let mut equal = Vec::new();
            for (axis, key) in [("horizontal", 0usize), ("vertical", 1)] {
                let mut order: Vec<&(String, usize, f64, f64, f64, f64)> = boxes.iter().collect();
                order.sort_by(|a, b| {
                    let (pa, pb) = if key == 0 { (a.2, b.2) } else { (a.3, b.3) };
                    pa.total_cmp(&pb).then_with(|| a.0.cmp(&b.0))
                });
                let gaps: Vec<f64> = order
                    .windows(2)
                    .map(|w| {
                        if key == 0 {
                            w[1].2 - (w[0].2 + w[0].4)
                        } else {
                            w[1].3 - (w[0].3 + w[0].5)
                        }
                    })
                    .collect();
                if gaps.len() >= 2
                    && gaps
                        .iter()
                        .all(|g| (g - gaps[0]).abs() <= tolerance && *g >= 0.0)
                {
                    equal.push(json!({
                        "axis": axis,
                        "gap": gaps[0],
                        "order": order.iter().map(|b| b.0.clone()).collect::<Vec<_>>(),
                    }));
                }
            }
            // Distance to the page's margin box, when the page has resolvable margins.
            let page_id = pages(raw)[boxes[0].1]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let to_margins: Vec<Value> = match derived_guides(raw, &page_id) {
                Ok(g) => {
                    let pick = |orient: &str, lo: bool| {
                        let mut v: Vec<f64> = g
                            .iter()
                            .filter(|x| x["kind"] == "margin" && x["orientation"] == orient)
                            .filter_map(|x| x["position"].as_f64())
                            .collect();
                        v.sort_by(f64::total_cmp);
                        if lo {
                            v.first().copied()
                        } else {
                            v.last().copied()
                        }
                    };
                    let (top, bottom, left, right) = (
                        pick("h", true),
                        pick("h", false),
                        pick("v", true),
                        pick("v", false),
                    );
                    boxes
                        .iter()
                        .map(|b| {
                            json!({
                                "id": b.0,
                                "left": left.map(|l| b.2 - l),
                                "right": right.map(|r| r - (b.2 + b.4)),
                                "top": top.map(|t| b.3 - t),
                                "bottom": bottom.map(|bt| bt - (b.3 + b.5)),
                            })
                        })
                        .collect()
                }
                Err(_) => Vec::new(),
            };
            Ok((
                json!({
                    "page": page_id,
                    "objects": boxes.iter().map(|b| json!({"id": b.0, "x": b.2, "y": b.3, "width": b.4, "height": b.5})).collect::<Vec<_>>(),
                    "alignments": alignments,
                    "distances": distances,
                    "equal_spacing": equal,
                    "to_margins": to_margins,
                }),
                true,
            ))
        }
        GridAction::StepRepeat {
            id,
            count,
            dx,
            dy,
            prefix,
        } => {
            ensure!(
                (1..=MAX_REPEAT).contains(&count),
                "[limit-exceeded] --count must be between 1 and {MAX_REPEAT}"
            );
            finite(dx, "--dx", -1_000_000.0, 1_000_000.0)?;
            finite(dy, "--dy", -1_000_000.0, 1_000_000.0)?;
            let (p, l) = locate(raw, &id)
                .with_context(|| format!("[missing-resource] object not found: {id}"))?;
            ensure!(
                !locked_layer(raw, p, l),
                "[locked] layer {} is locked",
                raw["pages"][p]["layers"][l]["id"]
            );
            let original = node_ref(raw, &id).expect("located").clone();
            fn has_text_frame(node: &Value) -> bool {
                node.get("kind") == Some(&json!("text-frame"))
                    || node
                        .get("children")
                        .and_then(Value::as_array)
                        .is_some_and(|c| c.iter().any(has_text_frame))
            }
            ensure!(
                !has_text_frame(&original),
                "[invalid-input] step-and-repeat would fork the thread of a text frame; \
                 use `layout-grid frame-grid --story` or `text frame add` instead"
            );
            // Only top-level nodes of a layer can be repeated in place.
            let top = raw["pages"][p]["layers"][l]["nodes"]
                .as_array()
                .and_then(|n| n.iter().position(|x| x["id"] == id.as_str()))
                .with_context(|| {
                    format!(
                        "[invalid-input] object {id} is inside a group; repeat the group instead"
                    )
                })?;
            let prefix = prefix.unwrap_or_else(|| format!("{id}-copy-"));
            check_id("object", &prefix)?;
            let taken = all_ids(raw);
            let mut created = Vec::new();
            let mut copies = Vec::new();
            for n in 1..=count {
                let new_id = format!("{prefix}{n}");
                ensure!(
                    !taken.contains(&new_id),
                    "[invalid-input] object ID already exists: {new_id}; choose another --prefix"
                );
                let mut copy = original.clone();
                rename_tree(&mut copy, &format!("-{prefix}{n}"), &id, &new_id);
                offset_node(&mut copy, dx * n as f64, dy * n as f64);
                created.push(new_id);
                copies.push(copy);
            }
            let nodes = raw["pages"][p]["layers"][l]["nodes"]
                .as_array_mut()
                .expect("checked");
            for (k, copy) in copies.into_iter().enumerate() {
                nodes.insert(top + 1 + k, copy);
            }
            Ok((
                json!({"ok": true, "operation": "step-repeat", "source": id, "count": count, "created": created}),
                false,
            ))
        }
        GridAction::FrameGrid {
            prefix,
            page,
            layer,
            rows,
            columns,
            x,
            y,
            width,
            height,
            gutter_x,
            gutter_y,
            story,
        } => {
            check_id("frame", &prefix)?;
            ensure!(
                rows >= 1 && columns >= 1 && rows * columns <= MAX_GRID_CELLS,
                "[limit-exceeded] a frame grid is limited to {MAX_GRID_CELLS} cells"
            );
            for (v, w) in [(x, "--x"), (y, "--y")] {
                finite(v, w, -1_000_000.0, 1_000_000.0)?;
            }
            finite(width, "--width", 0.001, 1_000_000.0)?;
            finite(height, "--height", 0.001, 1_000_000.0)?;
            finite(gutter_x, "--gutter-x", 0.0, 100_000.0)?;
            finite(gutter_y, "--gutter-y", 0.0, 100_000.0)?;
            let cell_w = (width - gutter_x * (columns - 1) as f64) / columns as f64;
            let cell_h = (height - gutter_y * (rows - 1) as f64) / rows as f64;
            ensure!(
                cell_w > 0.0 && cell_h > 0.0,
                "[invalid-input] the gutters leave no room for cells; reduce --gutter-x/--gutter-y"
            );
            let p = page_index(raw, &page)?;
            let l = raw["pages"][p]["layers"]
                .as_array()
                .and_then(|ls| ls.iter().position(|v| v["id"] == layer.as_str()))
                .with_context(|| format!("[missing-resource] layer not found: {layer}"))?;
            ensure!(!locked_layer(raw, p, l), "[locked] layer {layer} is locked");
            let taken = all_ids(raw);
            let mut ids = Vec::new();
            for r in 0..rows {
                for c in 0..columns {
                    let id = format!("{prefix}-r{}c{}", r + 1, c + 1);
                    ensure!(
                        !taken.contains(&id),
                        "[invalid-input] object ID already exists: {id}; choose another prefix"
                    );
                    ids.push((id, r, c));
                }
            }
            let place = |r: u64, c: u64| {
                (
                    x + c as f64 * (cell_w + gutter_x),
                    y + r as f64 * (cell_h + gutter_y),
                )
            };
            match &story {
                None => {
                    let nodes = raw["pages"][p]["layers"][l]["nodes"]
                        .as_array_mut()
                        .context("layer nodes are missing")?;
                    for (id, r, c) in &ids {
                        let (cx, cy) = place(*r, *c);
                        nodes.push(json!({
                            "kind": "graphic-frame", "id": id, "x": cx, "y": cy,
                            "width": cell_w, "height": cell_h, "fit": "fit",
                        }));
                    }
                }
                Some(story) => {
                    for (id, r, c) in &ids {
                        let (cx, cy) = place(*r, *c);
                        crate::stories::run_frame(
                            raw,
                            crate::stories::FrameAction::Add {
                                id: id.clone(),
                                story: story.clone(),
                                page: page.clone(),
                                layer: layer.clone(),
                                after: None,
                                options: crate::stories::FrameOptions {
                                    x: Some(cx),
                                    y: Some(cy),
                                    width: Some(cell_w),
                                    height: Some(cell_h),
                                    ..Default::default()
                                },
                            },
                        )?;
                    }
                }
            }
            Ok((
                json!({
                    "ok": true, "operation": "frame-grid", "created": ids.len(),
                    "cell": {"width": cell_w, "height": cell_h},
                    "frames": ids.iter().map(|(i, _, _)| i.clone()).collect::<Vec<_>>(),
                    "story": story,
                }),
                false,
            ))
        }
        GridAction::Fit {
            frame,
            mode,
            content,
        } => {
            ensure!(
                FITS.contains(&mode.as_str()),
                "[invalid-input] mode `{mode}` must be one of {}",
                FITS.join(", ")
            );
            let (p, l) = locate(raw, &frame)
                .with_context(|| format!("[missing-resource] frame not found: {frame}"))?;
            ensure!(
                !locked_layer(raw, p, l),
                "[locked] layer {} is locked",
                raw["pages"][p]["layers"][l]["id"]
            );
            let node = node_ref(raw, &frame).expect("located");
            ensure!(
                node["kind"] == "graphic-frame",
                "[invalid-input] `{frame}` is a {}; fit works on graphic frames",
                node["kind"]
            );
            let content_id = content
                .or_else(|| {
                    node.get("content")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .context("[invalid-input] the frame has no `content`; pass --content ID")?;
            ensure!(
                locate(raw, &content_id).is_some_and(|(cp, _)| cp == p),
                "[missing-resource] content `{content_id}` is not on the same page as `{frame}`"
            );
            let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
            let (fx, fy, fw, fh) = (
                num(node, "x").unwrap_or(0.0),
                num(node, "y").unwrap_or(0.0),
                num(node, "width").context("[malformed-resource] frame width is missing")?,
                num(node, "height").context("[malformed-resource] frame height is missing")?,
            );
            // The natural size is remembered so repeated fits are idempotent.
            let content_node = node_ref(raw, &content_id).expect("located");
            let natural = match node.get("natural_size").and_then(Value::as_array) {
                Some(n) if n.len() == 2 => {
                    (n[0].as_f64().unwrap_or(0.0), n[1].as_f64().unwrap_or(0.0))
                }
                _ => (
                    num(content_node, "width")
                        .context("[invalid-input] the content has no width to fit")?,
                    num(content_node, "height")
                        .context("[invalid-input] the content has no height to fit")?,
                ),
            };
            ensure!(
                natural.0 > 0.0 && natural.1 > 0.0,
                "[invalid-input] the content has no area to fit"
            );
            let (cw, ch) = natural;
            let (nx, ny, nw, nh, frame_resize) = match mode.as_str() {
                "fit" => {
                    let s = (fw / cw).min(fh / ch);
                    (
                        fx + (fw - cw * s) / 2.0,
                        fy + (fh - ch * s) / 2.0,
                        cw * s,
                        ch * s,
                        None,
                    )
                }
                "fill" => {
                    let s = (fw / cw).max(fh / ch);
                    (
                        fx + (fw - cw * s) / 2.0,
                        fy + (fh - ch * s) / 2.0,
                        cw * s,
                        ch * s,
                        None,
                    )
                }
                "proportional" => {
                    let s = fw / cw;
                    (fx, fy, fw, ch * s, None)
                }
                "fit-content-to-frame" => (fx, fy, fw, fh, None),
                "center" => (fx + (fw - cw) / 2.0, fy + (fh - ch) / 2.0, cw, ch, None),
                _ => (fx, fy, cw, ch, Some((cw, ch))),
            };
            {
                let page_value = &mut raw["pages"][p];
                let layers = page_value["layers"].as_array_mut().expect("layers");
                let nodes = layers[l]["nodes"].as_array_mut().expect("nodes");
                let content_node = find_mut(nodes, &content_id).expect("located");
                let o = content_node
                    .as_object_mut()
                    .context("content is not an object")?;
                for (key, value) in [("x", nx), ("y", ny), ("width", nw), ("height", nh)] {
                    o.insert(key.into(), json!(value));
                }
                let frame_node = find_mut(nodes, &frame).expect("located");
                let o = frame_node.as_object_mut().expect("object");
                o.insert("fit".into(), json!(mode));
                o.insert("content".into(), json!(content_id));
                o.insert("natural_size".into(), json!([cw, ch]));
                if let Some((w, h)) = frame_resize {
                    o.insert("width".into(), json!(w));
                    o.insert("height".into(), json!(h));
                }
            }
            Ok((
                json!({
                    "ok": true, "operation": "fit", "frame": frame, "mode": mode,
                    "content": content_id,
                    "content_bounds": {"x": nx, "y": ny, "width": nw, "height": nh},
                    "clipped": matches!(mode.as_str(), "fill" | "proportional" | "center")
                        && (nw > fw + 1e-9 || nh > fh + 1e-9),
                }),
                false,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_rounds_up_and_is_exact_in_milli_points() {
        assert_eq!(snap_up(10.0, 0.0, 0.0, 12.0), 12.0);
        assert_eq!(snap_up(12.0, 0.0, 0.0, 12.0), 12.0);
        assert_eq!(snap_up(12.001, 0.0, 0.0, 12.0), 24.0);
        assert_eq!(snap_up(5.0, 36.0, 0.0, 14.0), 8.0);
        assert_eq!(snap_up(-3.0, 0.0, 0.0, 12.0), 0.0);
        assert_eq!(snap_up(0.1 + 0.2, 0.0, 0.0, 0.3), 0.3);
    }
}

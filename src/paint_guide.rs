//! `pentool paint-guide`: reusable composition and form guides (v0.13.0 task 10).
//!
//! Guides are ordinary document data under the top-level `paint_guides` map. They
//! never export unless a proof is requested, never own pixels, and editing one never
//! changes a raster tile. A guide is a kind plus validated parameters; its geometry
//! (lines and points in page pixels) is derived deterministically and rounded to
//! 1/1000. Stroke fields and snapping reference guides by their stable id.
use crate::raster::constraints::atan2_q16;
use crate::raster::math::sin_cos_degrees;
use crate::transaction;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub const ALGORITHM: u32 = 1;
pub const MAX_GUIDES: usize = 256;
const MAX_COORD: f64 = 1.0e6;
const MAX_LINES: usize = 256;
const MAX_LIST: usize = 500;

pub const KINDS: &[&str] = &[
    "thirds",
    "golden",
    "symmetry",
    "horizon",
    "perspective",
    "vanishing-point",
    "gesture",
    "proportion",
    "value-map",
    "custom",
];

#[derive(clap::Subcommand, Debug)]
pub enum GuideAction {
    /// Add a guide (fails if the id exists unless --replace).
    Add {
        id: String,
        /// One of: thirds, golden, symmetry, horizon, perspective, vanishing-point,
        /// gesture, proportion, value-map, custom.
        #[arg(long)]
        kind: String,
        /// Kind parameters as JSON or @file.json.
        #[arg(long, default_value = "{}")]
        params: String,
        #[arg(long)]
        label: Option<String>,
        /// Start hidden in the editor.
        #[arg(long)]
        hidden: bool,
        /// Include in proof exports by default.
        #[arg(long)]
        proof: bool,
        #[arg(long)]
        replace: bool,
    },
    /// Change a guide's parameters, label or visibility. No pixel is touched.
    Set {
        id: String,
        #[arg(long)]
        params: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        visible: Option<bool>,
        #[arg(long)]
        proof: Option<bool>,
    },
    /// Remove a guide.
    Remove { id: String },
    /// List guides, paginated.
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Show one guide with its derived geometry.
    Show { id: String },
    /// Find the nearest snap point (line projection or point) on a guide.
    Snap {
        id: String,
        /// `x,y` in page pixels.
        #[arg(long)]
        point: String,
        /// Largest snap distance in pixels (default 16).
        #[arg(long, default_value_t = 16.0)]
        radius: f64,
    },
    /// Write the visible (or --ids) guides as an SVG proof overlay.
    Proof {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
        /// Comma-separated guide ids; default is every guide marked --proof.
        #[arg(long)]
        ids: Option<String>,
        /// Include hidden guides too.
        #[arg(long)]
        include_hidden: bool,
    },
}

fn num(value: &Value, what: &str) -> Result<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && v.abs() <= MAX_COORD)
        .with_context(|| format!("[invalid-request] guide {what} must be a finite number"))
}

fn pair(value: &Value, what: &str) -> Result<[f64; 2]> {
    let a = value
        .as_array()
        .filter(|a| a.len() == 2)
        .with_context(|| format!("[invalid-request] guide {what} must be [x,y]"))?;
    Ok([num(&a[0], what)?, num(&a[1], what)?])
}

fn rect(value: &Value) -> Result<[f64; 4]> {
    let a = value
        .as_array()
        .filter(|a| a.len() == 4)
        .context("[invalid-request] guide rect must be [x,y,w,h]")?;
    let r = [
        num(&a[0], "rect")?,
        num(&a[1], "rect")?,
        num(&a[2], "rect")?,
        num(&a[3], "rect")?,
    ];
    ensure!(
        r[2] > 0.0 && r[3] > 0.0,
        "[invalid-request] guide rect needs a positive width and height"
    );
    Ok(r)
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[derive(Default, Debug, Clone)]
pub struct Geometry {
    pub lines: Vec<[f64; 4]>,
    pub points: Vec<[f64; 2]>,
    /// value-map cells, row-major, 0-255.
    pub values: Option<Value>,
}

impl Geometry {
    fn line(&mut self, a: [f64; 2], b: [f64; 2]) {
        self.lines.push([r3(a[0]), r3(a[1]), r3(b[0]), r3(b[1])]);
    }
    fn point(&mut self, p: [f64; 2]) {
        self.points.push([r3(p[0]), r3(p[1])]);
    }
    fn to_json(&self) -> Value {
        let mut out = json!({"lines": self.lines, "points": self.points});
        if let Some(values) = &self.values {
            out["value_map"] = values.clone();
        }
        out
    }
}

fn only(params: &Map<String, Value>, allowed: &[&str], kind: &str) -> Result<()> {
    if let Some(key) = params.keys().find(|k| !allowed.contains(&k.as_str())) {
        bail!(
            "[invalid-request] guide kind {kind} has no parameter {key:?}; use {}",
            allowed.join(", ")
        )
    }
    Ok(())
}

fn need<'a>(params: &'a Map<String, Value>, key: &str, kind: &str) -> Result<&'a Value> {
    params
        .get(key)
        .with_context(|| format!("[invalid-request] guide kind {kind} needs parameter {key:?}"))
}

fn count(params: &Map<String, Value>, key: &str, default: u64, lo: u64, hi: u64) -> Result<u64> {
    match params.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| (lo..=hi).contains(n))
            .with_context(|| format!("[invalid-request] guide {key} must be an integer {lo}-{hi}")),
    }
}

/// Validate `params` for `kind` and derive its geometry.
pub fn geometry(kind: &str, params: &Value) -> Result<Geometry> {
    let p = params
        .as_object()
        .context("[invalid-request] guide params must be an object")?;
    let mut g = Geometry::default();
    match kind {
        "thirds" | "golden" => {
            only(p, &["rect"], kind)?;
            let [x, y, w, h] = rect(need(p, "rect", kind)?)?;
            let (a, b) = if kind == "thirds" {
                (1.0 / 3.0, 2.0 / 3.0)
            } else {
                (0.381_966, 0.618_034)
            };
            for f in [a, b] {
                g.line([x + w * f, y], [x + w * f, y + h]);
                g.line([x, y + h * f], [x + w, y + h * f]);
            }
            for fx in [a, b] {
                for fy in [a, b] {
                    g.point([x + w * fx, y + h * fy]);
                }
            }
        }
        "symmetry" => {
            only(p, &["rect", "axis", "at"], kind)?;
            let [x, y, w, h] = rect(need(p, "rect", kind)?)?;
            let at = num(need(p, "at", kind)?, "at")?;
            match p.get("axis").and_then(Value::as_str).unwrap_or("vertical") {
                "vertical" => g.line([at, y], [at, y + h]),
                "horizontal" => g.line([x, at], [x + w, at]),
                other => {
                    bail!("[invalid-request] guide axis {other:?} must be vertical or horizontal")
                }
            }
        }
        "horizon" => {
            only(p, &["rect", "y"], kind)?;
            let [x, _, w, _] = rect(need(p, "rect", kind)?)?;
            let y = num(need(p, "y", kind)?, "y")?;
            g.line([x, y], [x + w, y]);
        }
        "vanishing-point" => {
            only(p, &["rect", "point", "rays"], kind)?;
            let r = rect(need(p, "rect", kind)?)?;
            let origin = pair(need(p, "point", kind)?, "point")?;
            let rays = count(p, "rays", 8, 1, 64)?;
            ray_fan(&mut g, origin, r, rays);
            g.point(origin);
        }
        "perspective" => {
            only(p, &["rect", "horizon_y", "vanishing", "rays"], kind)?;
            let r = rect(need(p, "rect", kind)?)?;
            let horizon_y = num(need(p, "horizon_y", kind)?, "horizon_y")?;
            let vps = need(p, "vanishing", kind)?
                .as_array()
                .filter(|a| (1..=3).contains(&a.len()))
                .context("[invalid-request] guide vanishing must list 1-3 points")?;
            let rays = count(p, "rays", 6, 1, 32)?;
            g.line([r[0], horizon_y], [r[0] + r[2], horizon_y]);
            for vp in vps {
                let origin = pair(vp, "vanishing point")?;
                ray_fan(&mut g, origin, r, rays);
                g.point(origin);
            }
        }
        "gesture" => {
            only(p, &["points"], kind)?;
            let pts = need(p, "points", kind)?
                .as_array()
                .filter(|a| (2..=64).contains(&a.len()))
                .context("[invalid-request] guide points must list 2-64 points")?;
            let pts: Vec<[f64; 2]> = pts
                .iter()
                .map(|v| pair(v, "point"))
                .collect::<Result<_>>()?;
            for w in pts.windows(2) {
                g.line(w[0], w[1]);
            }
            for q in pts {
                g.point(q);
            }
        }
        "proportion" => {
            only(p, &["rect", "preset"], kind)?;
            let [x, y, w, h] = rect(need(p, "rect", kind)?)?;
            let divisions: Vec<f64> = match need(p, "preset", kind)?.as_str() {
                Some("figure-8") => (0..=8).map(|k| f64::from(k) / 8.0).collect(),
                Some("figure-7.5") => (0..=15).map(|k| f64::from(k) / 15.0).collect(),
                Some("face") => vec![0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0],
                _ => bail!("[invalid-request] guide preset must be figure-8, figure-7.5 or face"),
            };
            for f in divisions {
                g.line([x, y + h * f], [x + w, y + h * f]);
            }
            g.line([x + w * 0.5, y], [x + w * 0.5, y + h]);
        }
        "value-map" => {
            only(p, &["rect", "cols", "rows", "values"], kind)?;
            let [x, y, w, h] = rect(need(p, "rect", kind)?)?;
            let cols = count(p, "cols", 0, 1, 64)?;
            let rows = count(p, "rows", 0, 1, 64)?;
            ensure!(
                cols > 0 && rows > 0,
                "[invalid-request] guide value-map needs cols and rows (1-64)"
            );
            let values = need(p, "values", kind)?
                .as_array()
                .filter(|a| a.len() as u64 == cols * rows)
                .context("[invalid-request] guide values must hold cols*rows entries")?;
            ensure!(
                values.iter().all(|v| v.as_u64().is_some_and(|n| n <= 255)),
                "[invalid-request] guide values must be integers 0-255"
            );
            for c in 0..=cols {
                let px = x + w * c as f64 / cols as f64;
                g.line([px, y], [px, y + h]);
            }
            for r in 0..=rows {
                let py = y + h * r as f64 / rows as f64;
                g.line([x, py], [x + w, py]);
            }
            g.values = Some(json!({"cols": cols, "rows": rows, "values": values}));
        }
        "custom" => {
            only(p, &["lines", "points"], kind)?;
            let lines = match p.get("lines") {
                None => &Vec::new(),
                Some(v) => v
                    .as_array()
                    .context("[invalid-request] guide lines must be an array")?,
            };
            let points = match p.get("points") {
                None => &Vec::new(),
                Some(v) => v
                    .as_array()
                    .context("[invalid-request] guide points must be an array")?,
            };
            ensure!(
                !(lines.is_empty() && points.is_empty()),
                "[invalid-request] a custom guide needs lines or points"
            );
            ensure!(
                lines.len() <= MAX_LINES && points.len() <= MAX_LINES,
                "[limit-exceeded] a custom guide holds at most {MAX_LINES} lines and {MAX_LINES} points"
            );
            for l in lines {
                let a = l
                    .as_array()
                    .filter(|a| a.len() == 4)
                    .context("[invalid-request] guide line must be [x1,y1,x2,y2]")?;
                g.line(
                    [num(&a[0], "line")?, num(&a[1], "line")?],
                    [num(&a[2], "line")?, num(&a[3], "line")?],
                );
            }
            for q in points {
                g.point(pair(q, "point")?);
            }
        }
        other => bail!(
            "[invalid-request] unknown guide kind {other:?}; use {}",
            KINDS.join(", ")
        ),
    }
    ensure!(
        g.lines.len() <= 1024,
        "[limit-exceeded] guide derives more than 1024 lines"
    );
    Ok(g)
}

/// Evenly spaced rays from `origin`, long enough to cross `r`.
fn ray_fan(g: &mut Geometry, origin: [f64; 2], r: [f64; 4], rays: u64) {
    let far = ((r[0] - origin[0]).abs() + r[2]).hypot((r[1] - origin[1]).abs() + r[3]);
    for i in 0..rays {
        let degrees = 360.0 * i as f64 / rays as f64;
        let (s, c) = sin_cos_degrees(degrees);
        g.line(origin, [origin[0] + far * c, origin[1] + far * s]);
    }
}

fn guide_hash(kind: &str, params: &Value) -> Result<String> {
    Ok(crate::resource::sha256(&serde_json::to_vec(&json!({
        "algorithm": ALGORITHM,
        "kind": kind,
        "params": params,
    }))?))
}

fn valid_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && text
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn guides(raw: &Value) -> Result<Option<&Map<String, Value>>> {
    match raw.get("paint_guides") {
        None => Ok(None),
        Some(v) => v
            .as_object()
            .map(Some)
            .context("[malformed-resource] paint_guides must be an object"),
    }
}

fn record<'a>(raw: &'a Value, id: &str) -> Result<&'a Value> {
    guides(raw)?
        .and_then(|m| m.get(id))
        .with_context(|| format!("[not-found] guide {id:?} does not exist; list guides with `pentool paint-guide DOCUMENT list`"))
}

/// A stored guide with derived geometry.
pub fn load(raw: &Value, id: &str) -> Result<(Value, Geometry)> {
    let rec = record(raw, id)?;
    let geo = geometry(rec["kind"].as_str().unwrap_or_default(), &rec["params"])?;
    Ok((rec.clone(), geo))
}

/// Rewrite `{"method":"guide","guide":ID, "cell"?, "rect"?}` into the concrete field
/// spec the guide implies, so fields can reference guides by id. Other specs pass
/// through unchanged.
pub fn resolve_field(raw: &Value, field: &Value) -> Result<Value> {
    if field["method"].as_str() != Some("guide") {
        return Ok(field.clone());
    }
    let object = field
        .as_object()
        .context("[invalid-request] field must be an object")?;
    if let Some(key) = object
        .keys()
        .find(|k| !matches!(k.as_str(), "method" | "guide" | "cell" | "rect"))
    {
        bail!("[invalid-request] guide fields take guide, cell and rect only, not {key:?}")
    }
    let id = object
        .get("guide")
        .and_then(Value::as_str)
        .context("[invalid-request] field.guide must name a guide id")?;
    let (rec, geo) = load(raw, id)?;
    let kind = rec["kind"].as_str().unwrap_or_default();
    let mut out = Map::new();
    for key in ["cell", "rect"] {
        if let Some(v) = object.get(key) {
            out.insert(key.to_owned(), v.clone());
        }
    }
    match kind {
        "vanishing-point" | "perspective" => {
            let point = geo
                .points
                .first()
                .context("[invalid-request] the guide has no vanishing point")?;
            out.insert("method".into(), json!("vanishing"));
            out.insert("point".into(), json!(point));
        }
        "horizon" => {
            out.insert("method".into(), json!("angle"));
            out.insert("degrees".into(), json!(0));
        }
        "gesture" | "custom" if !geo.lines.is_empty() => {
            let anchors: Vec<Value> = geo
                .lines
                .iter()
                .take(64)
                .map(|l| {
                    let dy = ((l[3] - l[1]) * 1000.0).round() as i64;
                    let dx = ((l[2] - l[0]) * 1000.0).round() as i64;
                    let degrees = atan2_q16(dy, dx) as f64 / 65536.0;
                    json!([r3((l[0] + l[2]) * 0.5), r3((l[1] + l[3]) * 0.5), r3(degrees)])
                })
                .collect();
            out.insert("method".into(), json!("vectors"));
            out.insert("points".into(), json!(anchors));
        }
        other => bail!(
            "[invalid-request] guide {id:?} (kind {other}) cannot drive a field; use a vanishing-point, perspective, horizon, gesture or custom guide"
        ),
    }
    Ok(Value::Object(out))
}

fn json_arg(text: &str) -> Result<Value> {
    let body = match text.strip_prefix('@') {
        Some(path) => {
            ensure!(
                std::fs::metadata(path)
                    .with_context(|| format!("[missing-resource] cannot read {path}"))?
                    .len()
                    <= 4 << 20,
                "[limit-exceeded] {path} is larger than 4 MiB"
            );
            std::fs::read_to_string(path)?
        }
        None => text.to_owned(),
    };
    serde_json::from_str(&body).context("[invalid-request] argument must be JSON")
}

fn summary(id: &str, rec: &Value) -> Value {
    json!({
        "id": id,
        "kind": rec["kind"],
        "label": rec["label"],
        "visible": rec["visible"],
        "proof": rec["proof"],
        "guide_hash": rec["guide_hash"],
    })
}

fn snap(geo: &Geometry, p: [f64; 2], radius: f64) -> Option<(f64, [f64; 2], &'static str)> {
    // Anchor points (intersections, vanishing points) win over lines in range.
    let mut best: Option<(f64, [f64; 2], &'static str)> = None;
    for q in &geo.points {
        let d = (q[0] - p[0]).hypot(q[1] - p[1]);
        if d <= radius && best.is_none_or(|b| d < b.0) {
            best = Some((d, *q, "point"));
        }
    }
    if best.is_some() {
        return best;
    }
    let mut consider = |d: f64, at: [f64; 2], what: &'static str| {
        if d <= radius && best.is_none_or(|b| d < b.0) {
            best = Some((d, at, what));
        }
    };
    for l in &geo.lines {
        let (dx, dy) = (l[2] - l[0], l[3] - l[1]);
        let len2 = dx * dx + dy * dy;
        let t = if len2 == 0.0 {
            0.0
        } else {
            (((p[0] - l[0]) * dx + (p[1] - l[1]) * dy) / len2).clamp(0.0, 1.0)
        };
        let q = [l[0] + dx * t, l[1] + dy * t];
        consider((q[0] - p[0]).hypot(q[1] - p[1]), q, "line");
    }
    best
}

fn proof_svg(raw: &Value, ids: &[String], width: u32, height: u32) -> Result<String> {
    let mut body = String::new();
    for id in ids {
        let (_, geo) = load(raw, id)?;
        body.push_str(&format!("  <g id=\"guide-{id}\" fill=\"none\" stroke=\"#ff2d95\" stroke-width=\"1\" stroke-dasharray=\"6 4\">\n"));
        for l in &geo.lines {
            body.push_str(&format!(
                "    <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>\n",
                l[0], l[1], l[2], l[3]
            ));
        }
        for q in &geo.points {
            body.push_str(&format!(
                "    <circle cx=\"{}\" cy=\"{}\" r=\"3\"/>\n",
                q[0], q[1]
            ));
        }
        body.push_str("  </g>\n");
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">\n{body}</svg>\n"
    ))
}

pub fn run(
    input: &PathBuf,
    dry_run: bool,
    revision: Option<&str>,
    action: GuideAction,
) -> Result<()> {
    let mut raw: Value = serde_json::from_slice(
        &std::fs::read(input)
            .with_context(|| format!("[missing-resource] cannot read {}", input.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    transaction::validate_value(&raw)?;
    let (result, mutated) = match action {
        GuideAction::Add {
            id,
            kind,
            params,
            label,
            hidden,
            proof,
            replace,
        } => {
            ensure!(
                valid_id(&id),
                "[invalid-request] guide id {id:?} must be 1-128 characters of letters, digits, '.', '_' or '-'"
            );
            let params = json_arg(&params)?;
            let geo = geometry(&kind, &params)?;
            let exists = guides(&raw)?.is_some_and(|m| m.contains_key(&id));
            ensure!(
                replace || !exists,
                "[conflict] guide {id:?} already exists; pass --replace to overwrite it or use `set`"
            );
            ensure!(
                exists || guides(&raw)?.is_none_or(|m| m.len() < MAX_GUIDES),
                "[limit-exceeded] a document holds at most {MAX_GUIDES} guides"
            );
            let rec = json!({
                "algorithm": ALGORITHM,
                "kind": kind,
                "params": params,
                "label": label,
                "visible": !hidden,
                "proof": proof,
                "guide_hash": guide_hash(&kind, &params)?,
            });
            let out = json!({"added": summary(&id, &rec), "geometry": geo.to_json(), "pixels_changed": false});
            raw.as_object_mut()
                .context("[invalid-input] document must be an object")?
                .entry("paint_guides")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("[malformed-resource] paint_guides must be an object")?
                .insert(id, rec);
            (out, true)
        }
        GuideAction::Set {
            id,
            params,
            label,
            visible,
            proof,
        } => {
            ensure!(
                params.is_some() || label.is_some() || visible.is_some() || proof.is_some(),
                "[invalid-request] set needs at least one of --params, --label, --visible, --proof"
            );
            let mut rec = record(&raw, &id)?.clone();
            if let Some(text) = params {
                let params = json_arg(&text)?;
                let kind = rec["kind"].as_str().unwrap_or_default().to_owned();
                geometry(&kind, &params)?;
                rec["guide_hash"] = json!(guide_hash(&kind, &params)?);
                rec["params"] = params;
            }
            if let Some(label) = label {
                rec["label"] = json!(label);
            }
            if let Some(v) = visible {
                rec["visible"] = json!(v);
            }
            if let Some(v) = proof {
                rec["proof"] = json!(v);
            }
            let out = json!({"updated": summary(&id, &rec), "pixels_changed": false});
            raw["paint_guides"][&id] = rec;
            (out, true)
        }
        GuideAction::Remove { id } => {
            record(&raw, &id)?;
            let map = raw["paint_guides"].as_object_mut().expect("checked above");
            map.remove(&id);
            if map.is_empty() {
                raw.as_object_mut().expect("object").remove("paint_guides");
            }
            (json!({"removed": id, "pixels_changed": false}), true)
        }
        GuideAction::List {
            kind,
            limit,
            offset,
        } => {
            ensure!(
                (1..=MAX_LIST).contains(&limit),
                "[invalid-request] --limit must be 1-{MAX_LIST}"
            );
            let rows: Vec<Value> = guides(&raw)?
                .into_iter()
                .flatten()
                .filter(|(_, r)| kind.as_deref().is_none_or(|k| r["kind"] == k))
                .map(|(id, r)| summary(id, r))
                .collect();
            let page: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
            (
                json!({
                    "matches": rows.len(),
                    "returned": page.len(),
                    "offset": offset,
                    "limit": limit,
                    "has_more": offset + page.len() < rows.len(),
                    "guides": page,
                }),
                false,
            )
        }
        GuideAction::Show { id } => {
            let (rec, geo) = load(&raw, &id)?;
            let mut out = rec;
            out["id"] = json!(id);
            out["geometry"] = geo.to_json();
            (out, false)
        }
        GuideAction::Snap { id, point, radius } => {
            ensure!(
                radius.is_finite() && (0.0..=4096.0).contains(&radius),
                "[invalid-request] --radius must be 0-4096"
            );
            let parts: Vec<f64> = point
                .split(',')
                .map(|s| s.trim().parse::<f64>())
                .collect::<Result<_, _>>()
                .context("[invalid-request] --point must be x,y")?;
            ensure!(
                parts.len() == 2 && parts.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD),
                "[invalid-request] --point must be x,y"
            );
            let (_, geo) = load(&raw, &id)?;
            let found = snap(&geo, [parts[0], parts[1]], radius);
            (
                json!({
                    "guide": id,
                    "point": [parts[0], parts[1]],
                    "radius": radius,
                    "snapped": found.is_some(),
                    "to": found.map(|f| json!([r3(f.1[0]), r3(f.1[1])])),
                    "distance": found.map(|f| r3(f.0)),
                    "on": found.map(|f| f.2),
                }),
                false,
            )
        }
        GuideAction::Proof {
            out,
            width,
            height,
            ids,
            include_hidden,
        } => {
            ensure!(
                (1..=16384).contains(&width) && (1..=16384).contains(&height),
                "[invalid-request] --width and --height must be 1-16384"
            );
            let selected: Vec<String> = match ids {
                Some(list) => list.split(',').map(|s| s.trim().to_owned()).collect(),
                None => guides(&raw)?
                    .into_iter()
                    .flatten()
                    .filter(|(_, r)| r["proof"] == true)
                    .map(|(id, _)| id.clone())
                    .collect(),
            };
            let mut used = Vec::new();
            for id in &selected {
                let rec = record(&raw, id)?;
                if include_hidden || rec["visible"] != false {
                    used.push(id.clone());
                }
            }
            ensure!(
                !used.is_empty(),
                "[invalid-request] no visible guide is marked for proof; pass --ids or --include-hidden"
            );
            let svg = proof_svg(&raw, &used, width, height)?;
            std::fs::write(&out, svg.as_bytes())
                .with_context(|| format!("cannot write {}", out.display()))?;
            (
                json!({"proof": out, "guides": used, "bytes": svg.len()}),
                false,
            )
        }
    };
    if mutated {
        let change = transaction::commit_value(input, "paint-guide", dry_run, revision, &raw)?;
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"result": result, "dry_run": dry_run, "change": change})
            )?
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}

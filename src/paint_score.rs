//! Agent diagnostics and before/after revision scoring (v0.13.0 task 11).
//!
//! `diagnose` reports what one edit changed (measured facts) plus warnings that say
//! whether each is measured or a heuristic. `pentool paint-score BEFORE AFTER`
//! scores two revisions of a layer against a declared goal. Nothing here writes a
//! document, accepts a revision, reverts one or continues a plan.
use crate::raster::{self, Surface};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

const MAX_REGION_PIXELS: u64 = 16_777_216;
const MAX_PROTECT: usize = 64;
const MAX_PALETTE: usize = 32;

pub type Rect = [u32; 4];

/// Options shared by plan pass diagnostics and goal scoring.
#[derive(Default, Clone, Debug)]
pub struct Options {
    /// Where the edit was meant to land; changes outside count as edge spill.
    pub region: Option<Rect>,
    /// Rectangles that must not change.
    pub protect: Vec<Rect>,
    /// Intended palette; reported as drift of the changed pixels from it.
    pub palette: Vec<[u8; 3]>,
}

pub fn parse_rect(value: &Value, what: &str) -> Result<Rect> {
    let numbers: Option<Vec<u32>> = value.as_array().filter(|a| a.len() == 4).and_then(|a| {
        a.iter()
            .map(|n| n.as_u64().and_then(|n| u32::try_from(n).ok()))
            .collect()
    });
    let r = numbers.filter(|r| r[2] > 0 && r[3] > 0).with_context(|| {
        format!("[invalid-request] {what} must be [x,y,width,height] integers with a positive size")
    })?;
    ensure!(
        u64::from(r[2]) * u64::from(r[3]) <= MAX_REGION_PIXELS,
        "[limit-exceeded] {what} covers more than {MAX_REGION_PIXELS} pixels"
    );
    Ok([r[0], r[1], r[2], r[3]])
}

pub fn parse_options(object: &Map<String, Value>) -> Result<Options> {
    let mut options = Options::default();
    if let Some(v) = object.get("region").filter(|v| !v.is_null()) {
        options.region = Some(parse_rect(v, "region")?);
    }
    if let Some(v) = object.get("protect").filter(|v| !v.is_null()) {
        let list = v
            .as_array()
            .filter(|a| a.len() <= MAX_PROTECT)
            .with_context(|| {
                format!("[invalid-request] protect must list at most {MAX_PROTECT} rectangles")
            })?;
        options.protect = list
            .iter()
            .map(|r| parse_rect(r, "protect rectangle"))
            .collect::<Result<_>>()?;
    }
    if let Some(v) = object.get("palette").filter(|v| !v.is_null()) {
        let list = v
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= MAX_PALETTE)
            .with_context(|| {
                format!("[invalid-request] palette must list 1-{MAX_PALETTE} colors")
            })?;
        options.palette = list
            .iter()
            .map(|c| {
                raster::parse_color(
                    c.as_str()
                        .context("[invalid-request] palette colors are #rrggbb strings")?,
                )
            })
            .collect::<Result<_>>()?;
    }
    Ok(options)
}

fn luma(p: [u8; 4]) -> u32 {
    (54 * u32::from(p[0]) + 183 * u32::from(p[1]) + 19 * u32::from(p[2]) + 128) >> 8
}

fn inside(r: &Rect, x: u32, y: u32) -> bool {
    x >= r[0] && y >= r[1] && x < r[0].saturating_add(r[2]) && y < r[1].saturating_add(r[3])
}

fn nearest(palette: &[[u8; 3]], p: [u8; 4]) -> f64 {
    palette
        .iter()
        .map(|c| {
            let d = |i: usize| {
                let v = i32::from(c[i]) - i32::from(p[i]);
                f64::from(v * v)
            };
            (d(0) + d(1) + d(2)).sqrt()
        })
        .fold(f64::INFINITY, f64::min)
}

fn r4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn warning(kind: &str, code: &str, message: String) -> Value {
    json!({"basis": kind, "code": code, "message": message})
}

/// Measured facts about the difference between two surfaces of one layer.
pub fn diagnose(before: &Surface, after: &Surface, options: &Options) -> Value {
    let (w, h) = (after.width, after.height);
    let (mut changed, mut spill, mut violations) = (0_u64, 0_u64, 0_u64);
    let mut bounds: Option<[u32; 4]> = None;
    let (mut lo, mut hi) = (255_u32, 0_u32);
    let mut sums = ([0_u64; 3], [0_u64; 3]);
    let mut drift = 0.0_f64;
    let mut bins: BTreeMap<u16, u64> = BTreeMap::new();
    let mut tiles = std::collections::BTreeSet::new();
    let mut seen: Vec<(u32, u32)> = before
        .tiles
        .keys()
        .chain(after.tiles.keys())
        .copied()
        .collect();
    seen.sort_unstable();
    seen.dedup();
    let t = raster::TILE as u32;
    for (tx, ty) in seen {
        let (a, b) = (before.tiles.get(&(tx, ty)), after.tiles.get(&(tx, ty)));
        if a == b {
            continue;
        }
        for dy in 0..t.min(h.saturating_sub(ty * t)) {
            for dx in 0..t.min(w.saturating_sub(tx * t)) {
                let (x, y) = (tx * t + dx, ty * t + dy);
                let (pa, pb) = (before.pixel(x, y), after.pixel(x, y));
                if pa == pb {
                    continue;
                }
                changed += 1;
                tiles.insert((tx, ty));
                let e = bounds.get_or_insert([x, y, x + 1, y + 1]);
                e[0] = e[0].min(x);
                e[1] = e[1].min(y);
                e[2] = e[2].max(x + 1);
                e[3] = e[3].max(y + 1);
                if options.region.as_ref().is_some_and(|r| !inside(r, x, y)) {
                    spill += 1;
                }
                if options.protect.iter().any(|r| inside(r, x, y)) {
                    violations += 1;
                }
                if pb[3] > 0 {
                    let l = luma(pb);
                    lo = lo.min(l);
                    hi = hi.max(l);
                    for (sum, v) in sums.1.iter_mut().zip(pb) {
                        *sum += u64::from(v);
                    }
                    *bins
                        .entry(
                            u16::from(pb[0] >> 4) << 8
                                | u16::from(pb[1] >> 4) << 4
                                | u16::from(pb[2] >> 4),
                        )
                        .or_default() += 1;
                    if !options.palette.is_empty() {
                        drift += nearest(&options.palette, pb);
                    }
                }
                for (sum, v) in sums.0.iter_mut().zip(pa) {
                    *sum += u64::from(v);
                }
            }
        }
    }
    let painted: u64 = bins.values().sum();
    let mean = |s: &[u64; 3], n: u64| -> Value {
        match n {
            0 => Value::Null,
            n => json!([s[0] / n, s[1] / n, s[2] / n]),
        }
    };
    let mut top: Vec<(u16, u64)> = bins.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let dominant: Vec<Value> = top
        .iter()
        .take(5)
        .map(|(bin, n)| {
            json!({
                "color": format!("#{:02x}{:02x}{:02x}", ((bin >> 8) & 15) * 17, ((bin >> 4) & 15) * 17, (bin & 15) * 17),
                "pixels": n,
            })
        })
        .collect();
    let area = u64::from(w) * u64::from(h);
    let coverage = if area == 0 {
        0.0
    } else {
        changed as f64 / area as f64
    };
    let mut warnings = Vec::new();
    if changed == 0 {
        warnings.push(warning(
            "measured",
            "no-change",
            "the edit changed no pixel".into(),
        ));
    }
    if violations > 0 {
        warnings.push(warning(
            "measured",
            "protected-region-violation",
            format!("{violations} changed pixels lie inside protected rectangles"),
        ));
    }
    if spill > 0 {
        warnings.push(warning(
            "measured",
            "edge-spill",
            format!("{spill} changed pixels lie outside the intended region"),
        ));
    }
    if painted > 0 && hi.saturating_sub(lo) < 24 {
        warnings.push(warning(
            "heuristic",
            "flat-values",
            format!(
                "changed pixels span only {} luma levels; the edit may read as flat",
                hi - lo
            ),
        ));
    }
    if coverage > 0.5 {
        warnings.push(warning(
            "heuristic",
            "large-change",
            format!("the edit changed {:.0}% of the layer", coverage * 100.0),
        ));
    }
    let palette_drift = if options.palette.is_empty() || painted == 0 {
        Value::Null
    } else {
        json!(r4(drift / painted as f64))
    };
    json!({
        "measured": {
            "changed_pixels": changed,
            "changed_tiles": tiles.len(),
            "changed_bounds": bounds,
            "coverage": r4(coverage),
            "value_range": if painted == 0 { Value::Null } else { json!([lo, hi]) },
            "mean_color_before": mean(&sums.0, changed),
            "mean_color_after": mean(&sums.1, painted),
            "dominant_colors": dominant,
            "palette_drift": palette_drift,
            "edge_spill_pixels": if options.region.is_some() { json!(spill) } else { Value::Null },
            "protected_violations": if options.protect.is_empty() { Value::Null } else { json!(violations) },
        },
        "warnings": warnings,
    })
}

/// Criteria scored 0-1 over `region` for one surface.
fn criteria(surface: &Surface, region: Rect, goal: &Goal, reference: Option<&Surface>) -> Value {
    let (mut painted, mut in_range, mut diff_sum, mut total) = (0_u64, 0_u64, 0_f64, 0_u64);
    let mut drift = 0.0_f64;
    for y in region[1]..region[1].saturating_add(region[3]).min(surface.height) {
        for x in region[0]..region[0].saturating_add(region[2]).min(surface.width) {
            total += 1;
            let p = surface.pixel(x, y);
            if p[3] > 0 {
                painted += 1;
                if let Some([lo, hi]) = goal.value_range {
                    let l = luma(p);
                    if l >= lo && l <= hi {
                        in_range += 1;
                    }
                }
                if !goal.palette.is_empty() {
                    drift += nearest(&goal.palette, p);
                }
            }
            if let Some(reference) = reference {
                let q = reference.pixel(x, y);
                let d: u32 = (0..4).map(|i| u32::from(p[i].abs_diff(q[i]))).sum();
                diff_sum += f64::from(d) / 1020.0;
            }
        }
    }
    let mut out = Map::new();
    let coverage = if total == 0 {
        0.0
    } else {
        painted as f64 / total as f64
    };
    out.insert("coverage".into(), json!(r4(coverage)));
    if let Some([lo, hi]) = goal.coverage {
        let score = if coverage < lo {
            1.0 - (lo - coverage) / lo.max(f64::EPSILON)
        } else if coverage > hi {
            1.0 - (coverage - hi) / (1.0 - hi).max(f64::EPSILON)
        } else {
            1.0
        };
        out.insert("coverage_score".into(), json!(r4(score.clamp(0.0, 1.0))));
    }
    if goal.value_range.is_some() {
        let s = if painted == 0 {
            0.0
        } else {
            in_range as f64 / painted as f64
        };
        out.insert("value_score".into(), json!(r4(s)));
    }
    if !goal.palette.is_empty() {
        let s = if painted == 0 {
            0.0
        } else {
            1.0 - (drift / painted as f64) / 441.673
        };
        out.insert("palette_score".into(), json!(r4(s.clamp(0.0, 1.0))));
    }
    if reference.is_some() {
        let s = if total == 0 {
            0.0
        } else {
            1.0 - diff_sum / total as f64
        };
        out.insert("reference_score".into(), json!(r4(s.clamp(0.0, 1.0))));
    }
    Value::Object(out)
}

struct Goal {
    region: Option<Rect>,
    palette: Vec<[u8; 3]>,
    value_range: Option<[u32; 2]>,
    coverage: Option<[f64; 2]>,
    reference_layer: Option<String>,
    options: Options,
}

impl Goal {
    fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("[invalid-request] the goal must be a JSON object")?;
        if let Some(key) = object.keys().find(|k| {
            !matches!(
                k.as_str(),
                "region"
                    | "palette"
                    | "value_range"
                    | "coverage"
                    | "reference_layer"
                    | "protect"
                    | "description"
            )
        }) {
            bail!("[invalid-request] unknown goal field {key:?}; use region, palette, value_range, coverage, reference_layer, protect or description")
        }
        let options = parse_options(object)?;
        let value_range = match object.get("value_range").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => {
                let a = v
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .and_then(|a| Some([a[0].as_u64()?, a[1].as_u64()?]))
                    .filter(|r| r[0] <= r[1] && r[1] <= 255)
                    .context("[invalid-request] goal.value_range must be [low, high] luma 0-255")?;
                Some([a[0] as u32, a[1] as u32])
            }
        };
        let coverage = match object.get("coverage").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => {
                let a = v
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .and_then(|a| Some([a[0].as_f64()?, a[1].as_f64()?]))
                    .filter(|r| 0.0 <= r[0] && r[0] <= r[1] && r[1] <= 1.0)
                    .context("[invalid-request] goal.coverage must be [low, high] fractions 0-1")?;
                Some(a)
            }
        };
        let reference_layer = match object.get("reference_layer").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .context("[invalid-request] goal.reference_layer must be a layer id")?
                    .to_owned(),
            ),
        };
        ensure!(
            value_range.is_some()
                || coverage.is_some()
                || reference_layer.is_some()
                || !options.palette.is_empty(),
            "[invalid-request] the goal needs at least one of value_range, coverage, palette or reference_layer"
        );
        Ok(Self {
            region: options.region,
            palette: options.palette.clone(),
            value_range,
            coverage,
            reference_layer,
            options,
        })
    }
}

fn composite(scores: &Value) -> Option<f64> {
    let values: Vec<f64> = scores
        .as_object()?
        .iter()
        .filter(|(k, _)| k.ends_with("_score"))
        .filter_map(|(_, v)| v.as_f64())
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(r4(values.iter().sum::<f64>() / values.len() as f64))
    }
}

#[derive(clap::Args, Debug)]
pub struct ScoreArgs {
    /// Document before the edit.
    pub before: PathBuf,
    /// Document after the edit.
    pub after: PathBuf,
    /// Raster layer id present in both documents.
    #[arg(long)]
    pub layer: String,
    /// Goal JSON or @file.json: region, palette, value_range, coverage,
    /// reference_layer, protect.
    #[arg(long)]
    pub goal: String,
}

fn load_doc(path: &PathBuf) -> Result<Value> {
    let raw: Value = serde_json::from_slice(
        &std::fs::read(path)
            .with_context(|| format!("[missing-resource] cannot read {}", path.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    crate::transaction::validate_value(&raw)?;
    Ok(raw)
}

/// Score `after` against `before` for `goal`; read-only.
pub fn score(before: &Value, after: &Value, layer: &str, goal: &Value) -> Result<Value> {
    let goal = Goal::parse(goal)?;
    let b = raster::layer_surface(before, layer)
        .context("the layer must exist in the before document")?;
    let a = raster::layer_surface(after, layer)
        .context("the layer must exist in the after document")?;
    ensure!(
        (b.width, b.height) == (a.width, a.height),
        "[invalid-request] layer {layer} is {}x{} before and {}x{} after; score needs equal dimensions",
        b.width,
        b.height,
        a.width,
        a.height
    );
    let region = goal
        .region
        .unwrap_or([0, 0, a.width.max(1), a.height.max(1)]);
    let reference = |raw: &Value| -> Result<Option<Surface>> {
        goal.reference_layer
            .as_deref()
            .map(|id| raster::layer_surface(raw, id))
            .transpose()
    };
    let (rb, ra) = (reference(before)?, reference(after)?);
    let sb = criteria(&b, region, &goal, rb.as_ref());
    let sa = criteria(&a, region, &goal, ra.as_ref());
    let (cb, ca) = (composite(&sb), composite(&sa));
    let delta = match (cb, ca) {
        (Some(x), Some(y)) => Some(r4(y - x)),
        _ => None,
    };
    let trend = match delta {
        Some(d) if d > 0.02 => "improved",
        Some(d) if d < -0.02 => "worse",
        _ => "unchanged",
    };
    Ok(json!({
        "layer": layer,
        "region": region,
        "before": {"criteria": sb, "score": cb},
        "after": {"criteria": sa, "score": ca},
        "delta": delta,
        "assessment": {
            "basis": "heuristic",
            "trend": trend,
            "note": "scores are equal-weight means of the declared criteria; accepting, reverting or continuing is the caller's decision",
        },
        "diagnostics": diagnose(&b, &a, &goal.options),
        "applied": false,
    }))
}

pub fn run(args: ScoreArgs) -> Result<()> {
    let goal: Value = match args.goal.strip_prefix('@') {
        Some(path) => {
            ensure!(
                std::fs::metadata(path)
                    .with_context(|| format!("[missing-resource] cannot read {path}"))?
                    .len()
                    <= 1 << 20,
                "[limit-exceeded] {path} is larger than 1 MiB"
            );
            serde_json::from_slice(&std::fs::read(path)?)?
        }
        None => {
            serde_json::from_str(&args.goal).context("[invalid-request] --goal must be JSON")?
        }
    };
    let result = score(
        &load_doc(&args.before)?,
        &load_doc(&args.after)?,
        &args.layer,
        &goal,
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

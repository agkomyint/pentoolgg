//! UV inspection, projection overlays and layout export (v0.15.0 task 6a).
//!
//! The imported source model is never rewritten. A projection is a small,
//! deterministic *overlay* stored beside the model record (keyed by model
//! digest and primitive index) and re-applied each time the model is loaded.

use super::math::{self, PI};
use super::model::{Model, Primitive};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Map, Value};

/// Projections that duplicate vertices stay below this many vertices.
const MAX_SPLIT_VERTICES: usize = 3_000_000;
/// Largest UV layout image edge.
pub const MAX_LAYOUT_SIZE: u32 = 8192;
pub const MAX_TEXTURE_RESOLUTION: u32 = 16384;
const MAX_SEAM_ANALYSIS_TRIANGLES: usize = 250_000;
const MAX_SEAM_REPORT_EDGES: usize = 2048;
const MAX_SEAM_REPORT_SEGMENTS_PER_EDGE: usize = 16;
const MAX_SEAM_RENDER_SEGMENTS: usize = 4096;
const MAX_SEAM_RENDER_STEPS: usize = 16_000_000;
const MAX_ISLAND_OVERLAP_TESTS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Projection {
    Planar,
    Box,
    Cylindrical,
    Spherical,
}

#[derive(Clone, Debug)]
struct Spec {
    projection: Option<Projection>,
    axis: usize,
    scale: [f64; 2],
    offset: [f64; 2],
    rotate: f64,
    islands: Vec<IslandSpec>,
}

#[derive(Clone, Debug, PartialEq)]
struct IslandSpec {
    id: String,
    uv_set: usize,
    triangles: Vec<usize>,
    scale: [f64; 2],
    offset: [f64; 2],
    rotate: f64,
}

#[derive(Clone, Debug)]
struct IslandReport {
    id: String,
    uv_set: usize,
    triangles: usize,
    triangle_ids: Vec<usize>,
    min: [f64; 2],
    max: [f64; 2],
    mirrored: bool,
    texel_density_px_per_model_unit: Option<f64>,
}

fn uv_triangles_overlap(prim: &Primitive, uv_set: usize, a: usize, b: usize) -> bool {
    let points = |triangle: usize| -> [[f64; 2]; 3] {
        std::array::from_fn(|corner| {
            let vertex = prim.indices[triangle * 3 + corner] as usize;
            let uv = prim.uvs[uv_set][vertex];
            [f64::from(uv[0]), f64::from(uv[1])]
        })
    };
    let (a, b) = (points(a), points(b));
    let signed_area = |triangle: &[[f64; 2]; 3]| {
        (triangle[1][0] - triangle[0][0]) * (triangle[2][1] - triangle[0][1])
            - (triangle[1][1] - triangle[0][1]) * (triangle[2][0] - triangle[0][0])
    };
    if signed_area(&a).abs() <= 1e-14 || signed_area(&b).abs() <= 1e-14 {
        return false;
    }
    for triangle in [&a, &b] {
        for edge in 0..3 {
            let start = triangle[edge];
            let end = triangle[(edge + 1) % 3];
            let axis = [-(end[1] - start[1]), end[0] - start[0]];
            let project = |points: &[[f64; 2]; 3]| {
                points.map(|point| point[0] * axis[0] + point[1] * axis[1])
            };
            let pa = project(&a);
            let pb = project(&b);
            let (min_a, max_a) = pa
                .into_iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                    (lo.min(value), hi.max(value))
                });
            let (min_b, max_b) = pb
                .into_iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
                    (lo.min(value), hi.max(value))
                });
            let epsilon = 1e-12
                * 1.0f64
                    .max(min_a.abs())
                    .max(max_a.abs())
                    .max(min_b.abs())
                    .max(max_b.abs());
            if max_a <= min_b + epsilon || max_b <= min_a + epsilon {
                return false;
            }
        }
    }
    true
}

fn triangle_area_3d(prim: &Primitive, triangle: usize) -> f64 {
    let ids = [0, 1, 2].map(|corner| prim.indices[triangle * 3 + corner] as usize);
    let [a, b, c] = ids.map(|index| prim.positions[index]);
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    ((cross[0] as f64).powi(2) + (cross[1] as f64).powi(2) + (cross[2] as f64).powi(2)).sqrt() * 0.5
}

fn triangle_area_uv(prim: &Primitive, uv_set: usize, triangle: usize) -> f64 {
    let ids = [0, 1, 2].map(|corner| prim.indices[triangle * 3 + corner] as usize);
    let [a, b, c] = ids.map(|index| prim.uvs[uv_set][index]);
    (((b[0] - a[0]) as f64 * (c[1] - a[1]) as f64) - ((b[1] - a[1]) as f64 * (c[0] - a[0]) as f64))
        .abs()
        * 0.5
}

fn texel_density(
    prim: &Primitive,
    uv_set: usize,
    triangles: impl Iterator<Item = usize>,
    resolution: u32,
) -> Option<f64> {
    prim.uvs.get(uv_set)?;
    let (mut uv_area, mut model_area) = (0.0, 0.0);
    for triangle in triangles {
        if triangle >= prim.triangle_count() {
            continue;
        }
        uv_area += triangle_area_uv(prim, uv_set, triangle);
        model_area += triangle_area_3d(prim, triangle);
    }
    if uv_area.is_finite() && model_area.is_finite() && uv_area > 0.0 && model_area > 0.0 {
        let density = ((uv_area * (resolution as f64).powi(2)) / model_area).sqrt();
        density.is_finite().then_some(density)
    } else {
        None
    }
}

#[derive(Clone)]
struct UvEdgeRecord {
    key: [u32; 6],
    triangle: u32,
    segment: [[f32; 2]; 2],
}

#[derive(Clone)]
struct UvDiscontinuity {
    triangles: Vec<u32>,
    occurrence_count: usize,
    segments: Vec<[[f32; 2]; 2]>,
}

fn position_bits(position: [f32; 3]) -> [u32; 3] {
    position.map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

/// Find shared geometric edges whose UV coordinates differ between adjacent
/// triangles. Work is capped per primitive; the report says when it is partial.
fn uv_discontinuities(prim: &Primitive, uv_set: usize) -> (Vec<UvDiscontinuity>, bool) {
    let Some(uvs) = prim.uvs.get(uv_set) else {
        return (Vec::new(), false);
    };
    let triangle_count = prim.triangle_count();
    let analyzed = triangle_count.min(MAX_SEAM_ANALYSIS_TRIANGLES);
    let mut records = Vec::with_capacity(analyzed.saturating_mul(3));
    for (triangle, indices) in prim.indices.chunks_exact(3).take(analyzed).enumerate() {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let ia = indices[a] as usize;
            let ib = indices[b] as usize;
            let pa = position_bits(prim.positions[ia]);
            let pb = position_bits(prim.positions[ib]);
            if pa == pb {
                continue;
            }
            let (low, high, segment) = if pa < pb {
                (pa, pb, [uvs[ia], uvs[ib]])
            } else {
                (pb, pa, [uvs[ib], uvs[ia]])
            };
            records.push(UvEdgeRecord {
                key: [low[0], low[1], low[2], high[0], high[1], high[2]],
                triangle: triangle as u32,
                segment,
            });
        }
    }
    records.sort_unstable_by_key(|record| record.key);
    let same_uv = |a: &[[f32; 2]; 2], b: &[[f32; 2]; 2]| {
        (0..2).all(|end| (0..2).all(|axis| (a[end][axis] - b[end][axis]).abs() <= 1e-6))
    };
    let mut discontinuities = Vec::new();
    let mut start = 0usize;
    while start < records.len() {
        let mut end = start + 1;
        while end < records.len() && records[end].key == records[start].key {
            end += 1;
        }
        if end - start > 1
            && records[start + 1..end]
                .iter()
                .any(|record| !same_uv(&records[start].segment, &record.segment))
        {
            let mut triangles = records[start..end]
                .iter()
                .map(|record| record.triangle)
                .collect::<Vec<_>>();
            triangles.sort_unstable();
            triangles.dedup();
            discontinuities.push(UvDiscontinuity {
                triangles,
                occurrence_count: end - start,
                segments: records[start..end]
                    .iter()
                    .take(MAX_SEAM_REPORT_SEGMENTS_PER_EDGE)
                    .map(|record| record.segment)
                    .collect(),
            });
        }
        start = end;
    }
    (discontinuities, triangle_count > analyzed)
}

fn uv_discontinuity_report(prim: &Primitive, uv_set: usize) -> Value {
    let (edges, analysis_truncated) = uv_discontinuities(prim, uv_set);
    let reported_edges = edges.len().min(MAX_SEAM_REPORT_EDGES);
    let reported_segments: usize = edges
        .iter()
        .take(reported_edges)
        .map(|edge| edge.segments.len())
        .sum();
    json!({
        "uv_set":uv_set,
        "count":edges.len(),
        "analyzed_triangles":prim.triangle_count().min(MAX_SEAM_ANALYSIS_TRIANGLES),
        "analysis_truncated":analysis_truncated,
        "details_truncated":edges.len() > MAX_SEAM_REPORT_EDGES || edges.iter().any(|edge| edge.occurrence_count > MAX_SEAM_REPORT_SEGMENTS_PER_EDGE),
        "render_segments_truncated":edges.len() > MAX_SEAM_REPORT_EDGES || reported_segments > MAX_SEAM_RENDER_SEGMENTS,
        "edges":edges.iter().take(MAX_SEAM_REPORT_EDGES).map(|edge| json!({
            "triangles":edge.triangles,
            "occurrence_count":edge.occurrence_count,
            "segments_truncated":edge.occurrence_count > MAX_SEAM_REPORT_SEGMENTS_PER_EDGE,
            "uv_segments":edge.segments,
        })).collect::<Vec<_>>(),
    })
}

fn number(v: &Value, what: &str) -> Result<f64> {
    let n = v
        .as_f64()
        .with_context(|| format!("[invalid-input] {what} must be a number"))?;
    ensure!(n.is_finite(), "[invalid-input] {what} must be finite");
    Ok(n)
}

fn pair(spec: &Map<String, Value>, key: &str, default: [f64; 2]) -> Result<[f64; 2]> {
    let Some(v) = spec.get(key) else {
        return Ok(default);
    };
    let a = v
        .as_array()
        .filter(|a| a.len() == 2)
        .with_context(|| format!("[invalid-input] uv {key} must be [u, v]"))?;
    Ok([
        number(&a[0], &format!("uv {key}[0]"))?,
        number(&a[1], &format!("uv {key}[1]"))?,
    ])
}

fn parse_spec(value: &Value) -> Result<Spec> {
    let o = value
        .as_object()
        .context("[invalid-input] a uv overlay must be an object")?;
    for key in o.keys() {
        ensure!(
            matches!(
                key.as_str(),
                "projection" | "axis" | "scale" | "offset" | "rotate" | "islands"
            ),
            "[invalid-input] unknown uv overlay field `{key}`"
        );
    }
    let projection = match o.get("projection").and_then(Value::as_str) {
        Some("planar") => Some(Projection::Planar),
        Some("box") => Some(Projection::Box),
        Some("cylindrical") => Some(Projection::Cylindrical),
        Some("spherical") => Some(Projection::Spherical),
        None => None,
        other => bail!(
            "[invalid-input] uv projection {other:?} must be planar, box, cylindrical or spherical"
        ),
    };
    ensure!(
        projection.is_some() || !o.contains_key("axis"),
        "[invalid-input] uv axis requires a projection"
    );
    let islands = parse_islands(o.get("islands"))?;
    ensure!(
        projection.is_some() || !islands.is_empty(),
        "[invalid-input] a uv overlay needs a projection, islands, or both"
    );
    let axis = match o.get("axis").and_then(Value::as_str) {
        None => match projection {
            Some(Projection::Planar) => 2,
            _ => 1,
        },
        Some("x") => 0,
        Some("y") => 1,
        Some("z") => 2,
        Some(other) => bail!("[invalid-input] uv axis `{other}` must be x, y or z"),
    };
    let scale = pair(o, "scale", [1.0, 1.0])?;
    ensure!(
        scale.iter().all(|s| s.abs() > 1e-9 && s.abs() <= 1e6),
        "[invalid-input] uv scale must be nonzero and at most 1e6"
    );
    let offset = pair(o, "offset", [0.0, 0.0])?;
    ensure!(
        offset.iter().all(|s| s.abs() <= 1e6),
        "[invalid-input] uv offset is out of range"
    );
    let rotate = match o.get("rotate") {
        Some(v) => number(v, "uv rotate")?,
        None => 0.0,
    };
    ensure!(
        rotate.abs() <= 36000.0,
        "[invalid-input] uv rotate is out of range"
    );
    Ok(Spec {
        projection,
        axis,
        scale,
        offset,
        rotate,
        islands,
    })
}

fn parse_islands(value: Option<&Value>) -> Result<Vec<IslandSpec>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .context("[invalid-input] uv islands must be an array")?;
    ensure!(
        !values.is_empty(),
        "[invalid-input] UV islands cannot be empty; remove the field instead"
    );
    ensure!(
        values.len() <= 4096,
        "[limit-exceeded] a UV overlay supports at most 4096 islands"
    );
    let mut ids = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let island = value
            .as_object()
            .with_context(|| format!("[invalid-input] UV island {index} must be an object"))?;
        ensure!(
            island
                .keys()
                .all(|key| matches!(key.as_str(), "id" | "uv_set" | "triangles" | "transform")),
            "[invalid-input] UV island {index} has an unknown field"
        );
        let id = island
            .get("id")
            .and_then(Value::as_str)
            .context("[invalid-input] UV island ID is missing")?
            .to_owned();
        ensure!(
            !id.trim().is_empty() && id.len() <= 128,
            "[invalid-input] UV island ID must contain 1..=128 characters"
        );
        ensure!(
            ids.insert(id.clone()),
            "[invalid-input] duplicate UV island ID `{id}`"
        );
        let uv_set = island
            .get("uv_set")
            .and_then(Value::as_u64)
            .context("[invalid-input] UV island uv_set must be an integer")?
            as usize;
        ensure!(
            uv_set <= 3,
            "[invalid-input] UV island uv_set must be 0..=3"
        );
        let triangles = island
            .get("triangles")
            .and_then(Value::as_array)
            .context("[invalid-input] UV island triangles must be an array")?;
        ensure!(
            !triangles.is_empty() && triangles.len() <= 1_000_000,
            "[limit-exceeded] UV island triangles must contain 1..=1000000 indices"
        );
        let mut triangle_ids = Vec::with_capacity(triangles.len());
        let mut seen = std::collections::BTreeSet::new();
        for triangle in triangles {
            let triangle = triangle.as_u64().context(
                "[invalid-input] UV island triangle indices must be nonnegative integers",
            )? as usize;
            ensure!(
                seen.insert(triangle),
                "[invalid-input] UV island `{id}` repeats triangle {triangle}"
            );
            triangle_ids.push(triangle);
        }
        let transform = island
            .get("transform")
            .and_then(Value::as_object)
            .context("[invalid-input] UV island transform must be an object")?;
        ensure!(
            transform
                .keys()
                .all(|key| matches!(key.as_str(), "scale" | "offset" | "rotate")),
            "[invalid-input] UV island `{id}` transform has an unknown field"
        );
        for key in ["scale", "offset", "rotate"] {
            ensure!(
                transform.contains_key(key),
                "[invalid-input] UV island `{id}` transform requires {key}"
            );
        }
        let scale = pair(transform, "scale", [1.0, 1.0])?;
        ensure!(
            scale
                .iter()
                .all(|value| value.abs() > 1e-9 && value.abs() <= 1e6),
            "[invalid-input] UV island `{id}` scale must be nonzero and at most 1e6"
        );
        let offset = pair(transform, "offset", [0.0, 0.0])?;
        ensure!(
            offset.iter().all(|value| value.abs() <= 1e6),
            "[invalid-input] UV island `{id}` offset is out of range"
        );
        let rotate = number(&transform["rotate"], &format!("UV island `{id}` rotation"))?;
        ensure!(
            rotate.abs() <= 360.0,
            "[invalid-input] UV island `{id}` rotation must be within -360..=360 degrees"
        );
        out.push(IslandSpec {
            id,
            uv_set,
            triangles: triangle_ids,
            scale,
            offset,
            rotate,
        });
    }
    Ok(out)
}

pub(crate) fn project_primitive(prim: &mut Primitive, projection: &str, axis: &str) -> Result<()> {
    let spec = parse_spec(&serde_json::json!({"projection":projection,"axis":axis}))?;
    project(prim, &spec)
}

/// Validate the `uv` overlay map of a model record.
pub fn validate(overlays: &Value) -> Result<()> {
    let map = overlays
        .as_object()
        .context("[invalid-input] model `uv` must be an object keyed by primitive index")?;
    ensure!(
        map.len() <= 4096,
        "[limit-exceeded] a model supports at most 4096 UV primitive overlays"
    );
    let mut reference_count = 0usize;
    let mut island_count = 0usize;
    for (key, spec) in map {
        ensure!(
            key == "*" || key.parse::<usize>().is_ok(),
            "[invalid-input] uv overlay key `{key}` must be a primitive index or `*`"
        );
        let parsed = parse_spec(spec).with_context(|| format!("uv overlay `{key}`"))?;
        island_count = island_count
            .checked_add(parsed.islands.len())
            .context("[limit-exceeded] UV island count overflow")?;
        ensure!(
            island_count <= 4096,
            "[limit-exceeded] a model supports at most 4096 editable UV islands"
        );
        let mut ownership = std::collections::BTreeSet::new();
        for island in &parsed.islands {
            reference_count = reference_count
                .checked_add(island.triangles.len())
                .context("[limit-exceeded] UV triangle reference count overflow")?;
            ensure!(
                reference_count <= 1_000_000,
                "[limit-exceeded] UV island triangle references exceed 1000000 per model"
            );
            for &triangle in &island.triangles {
                ensure!(ownership.insert((island.uv_set, triangle)), "[invalid-input] triangle {triangle} belongs to more than one UV island in UV set {} of primitive `{key}`", island.uv_set);
            }
        }
    }
    Ok(())
}

/// Apply every overlay to a freshly imported model: `*` first, then
/// per-primitive entries, so a specific entry wins.
pub fn apply(model: &mut Model, overlays: &Value) -> Result<()> {
    validate(overlays)?;
    let map = overlays.as_object().expect("validated");
    let count = model.primitives.len();
    let mut order: Vec<(usize, Spec)> = Vec::new();
    let mut owners = vec![std::collections::BTreeSet::new(); count];
    let mut applied_references = 0usize;
    for (key, spec) in map {
        let parsed = parse_spec(spec)?;
        let indices: Vec<usize> = if key == "*" {
            (0..count).collect()
        } else {
            let index: usize = key.parse().expect("validated");
            ensure!(index < count, "[missing-resource] uv overlay targets primitive {index}, but the model has {count}");
            vec![index]
        };
        for &index in &indices {
            for island in &parsed.islands {
                applied_references = applied_references
                    .checked_add(island.triangles.len())
                    .context("[limit-exceeded] UV triangle reference count overflow")?;
                ensure!(
                    applied_references <= 1_000_000,
                    "[limit-exceeded] applied UV island references exceed 1000000 per model"
                );
                for &triangle in &island.triangles {
                    ensure!(owners[index].insert((island.uv_set, triangle)), "[invalid-input] triangle {triangle} belongs to more than one UV island in set {} of primitive {index}", island.uv_set);
                }
            }
            order.push((index, parsed.clone()));
        }
    }
    for (index, spec) in order {
        let primitive = &mut model.primitives[index];
        if let Some(projection) = spec.projection {
            let mut projection_spec = spec.clone();
            projection_spec.projection = Some(projection);
            project(primitive, &projection_spec)?;
        } else if spec.scale != [1.0, 1.0] || spec.offset != [0.0, 0.0] || spec.rotate != 0.0 {
            transform_existing(primitive, &spec)?;
        }
        apply_islands(primitive, &spec.islands)?;
    }
    model.report.vertices = model.primitives.iter().map(|p| p.positions.len()).sum();
    Ok(())
}

fn transform_existing(prim: &mut Primitive, spec: &Spec) -> Result<()> {
    let set = prim.uvs.first_mut().context(
        "[missing-resource] UV transform requires a source UV set; run `scene3d uv project` first",
    )?;
    let theta = spec.rotate * PI / 180.0;
    let (s, c) = (math::sin(theta), math::cos(theta));
    for uv in set {
        let (x, y) = (
            (uv[0] as f64 - 0.5) * spec.scale[0],
            (uv[1] as f64 - 0.5) * spec.scale[1],
        );
        *uv = [
            (x * c - y * s + 0.5 + spec.offset[0]) as f32,
            (x * s + y * c + 0.5 + spec.offset[1]) as f32,
        ];
    }
    Ok(())
}

fn apply_islands(prim: &mut Primitive, islands: &[IslandSpec]) -> Result<()> {
    if islands.is_empty() {
        return Ok(());
    }
    let triangle_count = prim.triangle_count();
    let mut total_references = 0usize;
    let mut owners = std::collections::BTreeSet::new();
    for island in islands {
        ensure!(
            island.uv_set < prim.uvs.len(),
            "[missing-resource] UV island `{}` targets UV set {}, but the primitive has {} set(s)",
            island.id,
            island.uv_set,
            prim.uvs.len()
        );
        total_references = total_references
            .checked_add(island.triangles.len())
            .context("[limit-exceeded] UV triangle reference count overflow")?;
        ensure!(
            total_references <= 1_000_000,
            "[limit-exceeded] UV island triangle references exceed 1000000 per primitive"
        );
        for &triangle in &island.triangles {
            ensure!(triangle < triangle_count, "[missing-resource] UV island `{}` references triangle {triangle}, but the primitive has {triangle_count}", island.id);
            ensure!(
                owners.insert((island.uv_set, triangle)),
                "[invalid-input] triangle {triangle} belongs to more than one UV island in set {}",
                island.uv_set
            );
        }
    }
    // Split corners once so adjacent islands can move independently without
    // changing their neighbors' UVs. Triangle indices remain stable.
    split(prim)?;
    for island in islands {
        let mut vertices = Vec::with_capacity(island.triangles.len() * 3);
        for &triangle in &island.triangles {
            vertices.extend([triangle * 3, triangle * 3 + 1, triangle * 3 + 2]);
        }
        let mut center = [0.0f64; 2];
        for &vertex in &vertices {
            let uv = prim.uvs[island.uv_set][vertex];
            center[0] += uv[0] as f64;
            center[1] += uv[1] as f64;
        }
        center[0] /= vertices.len() as f64;
        center[1] /= vertices.len() as f64;
        let theta = island.rotate * PI / 180.0;
        let (s, c) = (math::sin(theta), math::cos(theta));
        for vertex in vertices {
            let uv = prim.uvs[island.uv_set][vertex];
            let (x, y) = (
                (uv[0] as f64 - center[0]) * island.scale[0],
                (uv[1] as f64 - center[1]) * island.scale[1],
            );
            prim.uvs[island.uv_set][vertex] = [
                (x * c - y * s + center[0] + island.offset[0]) as f32,
                (x * s + y * c + center[1] + island.offset[1]) as f32,
            ];
        }
    }
    Ok(())
}

/// Pack editable UV islands into a padded unit square while preserving each
/// island's aspect ratio, rotation and mirror state. The mesh supplied here is
/// already loaded with the existing overlay applied.
pub(crate) fn pack_islands(prim: &Primitive, overlay: &Value, padding: f64) -> Result<Value> {
    ensure!(
        padding.is_finite() && (0.0..0.25).contains(&padding),
        "[invalid-input] UV pack padding must be finite and in 0..0.25"
    );
    let mut updated = overlay.clone();
    let spec = updated
        .as_object_mut()
        .context("[invalid-input] primitive UV overlay must be an object")?;
    let islands_value = spec
        .get_mut("islands")
        .and_then(Value::as_array_mut)
        .context("[missing-resource] primitive has no editable UV islands to pack")?;
    let islands = parse_islands(Some(&Value::Array(islands_value.clone())))?;
    ensure!(
        !islands.is_empty(),
        "[missing-resource] no UV islands to pack"
    );

    let mut by_set = std::collections::BTreeMap::<
        usize,
        Vec<(usize, [f64; 2], [f64; 2], [f64; 2], [f64; 2])>,
    >::new();
    for (index, island) in islands.iter().enumerate() {
        ensure!(
            island.uv_set < prim.uvs.len(),
            "[missing-resource] UV island `{}` targets unavailable UV set {}",
            island.id,
            island.uv_set
        );
        let set = by_set.entry(island.uv_set).or_default();
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        let mut center = [0.0; 2];
        let mut count = 0usize;
        for &triangle in &island.triangles {
            ensure!(
                triangle < prim.triangle_count(),
                "[missing-resource] UV island `{}` references missing triangle {triangle}",
                island.id
            );
            for corner in 0..3 {
                let uv = prim.uvs[island.uv_set][triangle * 3 + corner];
                for axis in 0..2 {
                    let value = uv[axis] as f64;
                    min[axis] = min[axis].min(value);
                    max[axis] = max[axis].max(value);
                    center[axis] += value;
                }
                count += 1;
            }
        }
        ensure!(
            min.iter().zip(max).all(|(lo, hi)| hi - lo > 1e-12),
            "[invalid-input] UV island `{}` has a degenerate bounding box and cannot be packed",
            island.id
        );
        center.iter_mut().for_each(|v| *v /= count as f64);
        let bounds_center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
        set.push((index, min, max, center, bounds_center));
    }

    #[derive(Clone)]
    struct Rect {
        index: usize,
        w: f64,
        h: f64,
        x: f64,
        y: f64,
    }
    for (uv_set, entries) in by_set {
        let mut rects: Vec<Rect> = entries
            .iter()
            .map(|(index, min, max, _, _)| Rect {
                index: *index,
                w: max[0] - min[0],
                h: max[1] - min[1],
                x: 0.0,
                y: 0.0,
            })
            .collect();
        rects.sort_by(|a, b| {
            (b.w * b.h)
                .total_cmp(&(a.w * a.h))
                .then_with(|| b.h.total_cmp(&a.h))
                .then_with(|| islands[a.index].id.cmp(&islands[b.index].id))
        });
        let place = |scale: f64, rects: &[Rect]| -> Option<Vec<Rect>> {
            let mut placed = Vec::with_capacity(rects.len());
            let (mut x, mut y, mut row_h) = (padding, padding, 0.0f64);
            for rect in rects {
                let (w, h) = (rect.w * scale, rect.h * scale);
                if x + w > 1.0 - padding + 1e-12 {
                    x = padding;
                    y += row_h + padding;
                    row_h = 0.0;
                }
                if y + h > 1.0 - padding + 1e-12 {
                    return None;
                }
                let mut item = rect.clone();
                item.x = x;
                item.y = y;
                item.w = w;
                item.h = h;
                placed.push(item);
                x += w + padding;
                row_h = row_h.max(h);
            }
            Some(placed)
        };
        let mut low = 0.0;
        let mut high = rects
            .iter()
            .map(|r| (1.0 - 2.0 * padding) / r.w.max(r.h))
            .fold(f64::INFINITY, f64::min);
        for _ in 0..48 {
            let mid = (low + high) * 0.5;
            if place(mid, &rects).is_some() {
                low = mid;
            } else {
                high = mid;
            }
        }
        let placed = place(low, &rects).context(
            "[invalid-input] UV islands cannot fit in the unit square with the requested padding",
        )?;
        for packed in placed {
            let (_, _, _, current_pivot, current_bounds_center) = entries
                .iter()
                .find(|(index, ..)| *index == packed.index)
                .unwrap();
            let island = &mut islands_value[packed.index];
            let transform = island["transform"]
                .as_object_mut()
                .context("[invalid-input] UV island transform must be an object")?;
            let old_scale = pair(transform, "scale", [1.0, 1.0])?;
            let old_offset = pair(transform, "offset", [0.0, 0.0])?;
            let target_center = [packed.x + packed.w * 0.5, packed.y + packed.h * 0.5];
            let multiplier = low;
            transform.insert(
                "scale".into(),
                json!([old_scale[0] * multiplier, old_scale[1] * multiplier]),
            );
            transform.insert(
                "offset".into(),
                json!([
                    target_center[0] - current_pivot[0] + old_offset[0]
                        - multiplier * (current_bounds_center[0] - current_pivot[0]),
                    target_center[1] - current_pivot[1] + old_offset[1]
                        - multiplier * (current_bounds_center[1] - current_pivot[1]),
                ]),
            );
        }
        let _ = uv_set;
    }
    validate(&json!({"0": updated}))?;
    Ok(updated)
}

/// Give every triangle its own vertices so each can take its own UVs.
fn split(prim: &mut Primitive) -> Result<()> {
    let n = prim.indices.len();
    ensure!(
        n <= MAX_SPLIT_VERTICES,
        "[limit-exceeded] projection would create {n} vertices; the limit is {MAX_SPLIT_VERTICES}"
    );
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut uvs: Vec<Vec<[f32; 2]>> = prim.uvs.iter().map(|_| Vec::with_capacity(n)).collect();
    for &i in &prim.indices {
        let i = i as usize;
        positions.push(prim.positions[i]);
        normals.push(prim.normals[i]);
        for (set, out) in prim.uvs.iter().zip(uvs.iter_mut()) {
            out.push(set[i]);
        }
    }
    prim.positions = positions;
    prim.normals = normals;
    prim.uvs = uvs;
    prim.indices = (0..n as u32).collect();
    Ok(())
}

fn project(prim: &mut Primitive, spec: &Spec) -> Result<()> {
    if prim.positions.is_empty() {
        return Ok(());
    }
    let projection = spec
        .projection
        .context("[invalid-input] UV projection is missing")?;
    if projection != Projection::Planar {
        split(prim)?;
    }
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in &prim.positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k] as f64);
            max[k] = max[k].max(p[k] as f64);
        }
    }
    let extent = |k: usize| {
        let e = max[k] - min[k];
        if e > 1e-12 {
            e
        } else {
            1.0
        }
    };
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let axes = |a: usize| ((a + 1) % 3, (a + 2) % 3);
    let at = |prim: &Primitive, i: usize| {
        let p = prim.positions[i];
        [p[0] as f64, p[1] as f64, p[2] as f64]
    };
    let mut raw: Vec<[f64; 2]> = vec![[0.0; 2]; prim.positions.len()];
    match projection {
        Projection::Planar => {
            let (b, c) = axes(spec.axis);
            for (i, uv) in raw.iter_mut().enumerate() {
                let p = at(prim, i);
                *uv = [(p[b] - min[b]) / extent(b), (p[c] - min[c]) / extent(c)];
            }
        }
        Projection::Box => {
            for (t, tri) in prim.indices.chunks_exact(3).enumerate() {
                let p0 = at(prim, tri[0] as usize);
                let p1 = at(prim, tri[1] as usize);
                let p2 = at(prim, tri[2] as usize);
                let n = math::cross(math::sub(p1, p0), math::sub(p2, p0));
                let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
                let a = if ax >= ay && ax >= az {
                    0
                } else if ay >= az {
                    1
                } else {
                    2
                };
                let (b, c) = axes(a);
                for k in 0..3 {
                    let p = at(prim, tri[k] as usize);
                    raw[t * 3 + k] = [(p[b] - min[b]) / extent(b), (p[c] - min[c]) / extent(c)];
                }
            }
        }
        Projection::Cylindrical | Projection::Spherical => {
            let a = spec.axis;
            let (b, c) = axes(a);
            let sphere = projection == Projection::Spherical;
            for (i, uv) in raw.iter_mut().enumerate() {
                let p = at(prim, i);
                let (db, dc) = (p[b] - center[b], p[c] - center[c]);
                let u = if db == 0.0 && dc == 0.0 {
                    0.5
                } else {
                    math::atan2(dc, db) / (2.0 * PI) + 0.5
                };
                let v = if sphere {
                    let d = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
                    let r = math::length(d);
                    if r > 1e-12 {
                        math::acos(d[a] / r) / PI
                    } else {
                        0.5
                    }
                } else {
                    (p[a] - min[a]) / extent(a)
                };
                *uv = [u, v];
            }
            // A triangle that crosses the u seam would smear across the whole
            // map; lift its low-u corners by one turn instead.
            for tri in raw.chunks_exact_mut(3) {
                let lo = tri.iter().map(|t| t[0]).fold(f64::INFINITY, f64::min);
                let hi = tri.iter().map(|t| t[0]).fold(f64::NEG_INFINITY, f64::max);
                if hi - lo > 0.5 {
                    for t in tri.iter_mut() {
                        if t[0] < 0.5 {
                            t[0] += 1.0;
                        }
                    }
                }
            }
        }
    }
    let theta = spec.rotate * PI / 180.0;
    let (s, c) = (math::sin(theta), math::cos(theta));
    let out: Vec<[f32; 2]> = raw
        .into_iter()
        .map(|[u, v]| {
            let (x, y) = (u - 0.5, v - 0.5);
            let (rx, ry) = (x * c - y * s, x * s + y * c);
            [
                (rx * spec.scale[0] + 0.5 + spec.offset[0]) as f32,
                (ry * spec.scale[1] + 0.5 + spec.offset[1]) as f32,
            ]
        })
        .collect();
    if prim.uvs.is_empty() {
        prim.uvs.push(out);
    } else {
        prim.uvs[0] = out;
    }
    Ok(())
}

/// Per-primitive UV report: sets, bounds, out-of-range vertices, degenerate
/// triangles and how much of the unit square the islands cover.
pub fn show(model: &Model) -> Value {
    show_with_resolution(model, 2048).expect("fixed UV inspection resolution is valid")
}

pub fn show_with_resolution(model: &Model, resolution: u32) -> Result<Value> {
    ensure!(
        (1..=MAX_TEXTURE_RESOLUTION).contains(&resolution),
        "[limit-exceeded] UV inspection resolution must be 1..={MAX_TEXTURE_RESOLUTION} pixels"
    );
    const GRID: usize = 64;
    let mut out = Vec::new();
    for (index, prim) in model.primitives.iter().enumerate() {
        let mut entry = json!({
            "primitive": index,
            "name": prim.name,
            "triangles": prim.triangle_count(),
            "vertices": prim.positions.len(),
            "uv_sets": prim.uvs.len(),
        });
        let set_densities: Vec<Value> = prim
            .uvs
            .iter()
            .enumerate()
            .map(|(uv_set, _)| {
                let density = texel_density(prim, uv_set, 0..prim.triangle_count(), resolution);
                json!({
                    "uv_set":uv_set,
                    "texture_resolution_px":[resolution,resolution],
                    "texel_density_px_per_model_unit":density,
                    "model_area_unit":"model_unit"
                })
            })
            .collect();
        entry["texel_density"] = json!(set_densities);
        entry["uv_discontinuities"] = json!(prim
            .uvs
            .iter()
            .enumerate()
            .map(|(uv_set, _)| uv_discontinuity_report(prim, uv_set))
            .collect::<Vec<_>>());
        if let Some(set) = prim.uvs.first() {
            let mut min = [f64::INFINITY; 2];
            let mut max = [f64::NEG_INFINITY; 2];
            let mut outside = 0usize;
            for uv in set {
                for k in 0..2 {
                    min[k] = min[k].min(uv[k] as f64);
                    max[k] = max[k].max(uv[k] as f64);
                }
                if uv.iter().any(|c| !(0.0..=1.0).contains(c)) {
                    outside += 1;
                }
            }
            let mut degenerate = 0usize;
            let mut grid = vec![false; GRID * GRID];
            for tri in prim.indices.chunks_exact(3) {
                let p: Vec<[f64; 2]> = tri
                    .iter()
                    .map(|&i| {
                        let uv = set[i as usize];
                        [uv[0] as f64, uv[1] as f64]
                    })
                    .collect();
                let area = ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                    - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]))
                    * 0.5;
                if area.abs() < 1e-12 {
                    degenerate += 1;
                    continue;
                }
                for gy in 0..GRID {
                    for gx in 0..GRID {
                        let q = [
                            (gx as f64 + 0.5) / GRID as f64,
                            (gy as f64 + 0.5) / GRID as f64,
                        ];
                        let edge = |a: [f64; 2], b: [f64; 2]| {
                            (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])
                        };
                        let (e0, e1, e2) = (edge(p[0], p[1]), edge(p[1], p[2]), edge(p[2], p[0]));
                        let inside = (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0)
                            || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
                        if inside {
                            grid[gy * GRID + gx] = true;
                        }
                    }
                }
            }
            let covered = grid.iter().filter(|c| **c).count();
            entry["uv_bounds"] = json!({"min": min, "max": max});
            entry["vertices_outside_unit"] = json!(outside);
            entry["degenerate_triangles"] = json!(degenerate);
            entry["coverage"] =
                json!((covered as f64 / (GRID * GRID) as f64 * 10000.0).round() / 10000.0);
        }
        out.push(entry);
    }
    Ok(json!({"primitives": out}))
}

/// Add editable island bounds and transform diagnostics to the geometric UV report.
pub fn show_with_overlays(model: &Model, overlays: &Value) -> Result<Value> {
    show_with_overlays_at_resolution(model, overlays, 2048, None)
}

pub fn show_with_overlays_at_resolution(
    model: &Model,
    overlays: &Value,
    resolution: u32,
    minimum_density: Option<f64>,
) -> Result<Value> {
    if let Some(minimum) = minimum_density {
        ensure!(
            minimum.is_finite() && minimum > 0.0,
            "[invalid-input] minimum texel density must be a finite positive number"
        );
    }
    let mut report = show_with_resolution(model, resolution)?;
    if let Some(primitives) = report["primitives"].as_array_mut() {
        for primitive in primitives {
            if let Some(sets) = primitive["texel_density"].as_array_mut() {
                for set in sets {
                    set["below_minimum_texel_density"] = minimum_density
                        .zip(set["texel_density_px_per_model_unit"].as_f64())
                        .map(|(minimum, density)| json!(density < minimum))
                        .unwrap_or(Value::Null);
                }
            }
        }
    }
    let empty_overlays = Map::new();
    let overlay_map = overlays.as_object().unwrap_or(&empty_overlays);
    let mut all_islands: Vec<Vec<IslandReport>> = vec![Vec::new(); model.primitives.len()];
    let mut push_islands = |primitive: usize, spec: &Value| -> Result<()> {
        let Some(entries) = spec.get("islands").and_then(Value::as_array) else {
            return Ok(());
        };
        let prim = model
            .primitives
            .get(primitive)
            .context("UV island primitive missing from loaded model")?;
        for island in entries {
            let id = island
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned();
            let uv_set = island.get("uv_set").and_then(Value::as_u64).unwrap_or(0) as usize;
            let triangles: Vec<usize> = island
                .get("triangles")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .map(|index| index as usize)
                .collect();
            ensure!(
                uv_set < prim.uvs.len(),
                "[missing-resource] UV island `{id}` targets unavailable UV set {uv_set}"
            );
            let mut min = [f64::INFINITY; 2];
            let mut max = [f64::NEG_INFINITY; 2];
            for triangle in &triangles {
                ensure!(
                    *triangle < prim.triangle_count(),
                    "[missing-resource] UV island `{id}` references missing triangle {triangle}"
                );
                for corner in 0..3 {
                    let uv = prim.uvs[uv_set][triangle * 3 + corner];
                    for axis in 0..2 {
                        min[axis] = min[axis].min(uv[axis] as f64);
                        max[axis] = max[axis].max(uv[axis] as f64);
                    }
                }
            }
            let transform = island.get("transform").cloned().unwrap_or_default();
            let scale = transform
                .get("scale")
                .and_then(Value::as_array)
                .map(|v| [v[0].as_f64().unwrap_or(1.0), v[1].as_f64().unwrap_or(1.0)])
                .unwrap_or([1.0, 1.0]);
            let density = texel_density(prim, uv_set, triangles.iter().copied(), resolution);
            all_islands[primitive].push(IslandReport {
                id,
                uv_set,
                triangles: triangles.len(),
                triangle_ids: triangles,
                min,
                max,
                mirrored: scale[0] * scale[1] < 0.0,
                texel_density_px_per_model_unit: density,
            });
        }
        Ok(())
    };
    if let Some(wildcard) = overlay_map.get("*") {
        for index in 0..model.primitives.len() {
            push_islands(index, wildcard)?;
        }
    }
    for (key, spec) in overlay_map {
        if key != "*" {
            if let Ok(index) = key.parse::<usize>() {
                push_islands(index, spec)?;
            }
        }
    }
    for (index, islands) in all_islands.into_iter().enumerate() {
        let mut entries = Vec::with_capacity(islands.len());
        let mut overlap_candidates = Vec::new();
        let mut overlap_tests = 0usize;
        let mut overlap_analysis_truncated = false;
        for (i, island) in islands.iter().enumerate() {
            for other in &islands[..i] {
                if overlap_analysis_truncated {
                    break;
                }
                if island.uv_set != other.uv_set
                    || (0..2).any(|axis| {
                        island.min[axis] >= other.max[axis] || other.min[axis] >= island.max[axis]
                    })
                {
                    continue;
                }
                let primitive = &model.primitives[index];
                let mut overlaps = false;
                'triangles: for &left in &island.triangle_ids {
                    for &right in &other.triangle_ids {
                        if overlap_tests >= MAX_ISLAND_OVERLAP_TESTS {
                            overlap_analysis_truncated = true;
                            break 'triangles;
                        }
                        overlap_tests += 1;
                        if uv_triangles_overlap(primitive, island.uv_set, left, right) {
                            overlaps = true;
                            break 'triangles;
                        }
                    }
                }
                if overlaps {
                    overlap_candidates.push(json!([other.id, island.id]));
                }
                if overlap_analysis_truncated {
                    break;
                }
            }
            let low_resolution = minimum_density
                .zip(island.texel_density_px_per_model_unit)
                .map(|(minimum, density)| density < minimum);
            entries.push(json!({
                "id":island.id,
                "uv_set":island.uv_set,
                "triangle_count":island.triangles,
                "uv_bounds":{"min":island.min,"max":island.max},
                "mirrored":island.mirrored,
                "texture_resolution_px":[resolution,resolution],
                "texel_density_px_per_model_unit":island.texel_density_px_per_model_unit,
                "below_minimum_texel_density":low_resolution
            }));
        }
        report["primitives"][index]["islands"] = json!(entries);
        report["primitives"][index]["island_overlap_candidates"] = json!(overlap_candidates);
        report["primitives"][index]["island_overlap_analysis_truncated"] =
            json!(overlap_analysis_truncated);
    }
    Ok(report)
}

fn edges(prim: &Primitive, size: f64) -> Result<Vec<[f64; 4]>> {
    let set = prim
        .uvs
        .first()
        .context("[missing-resource] primitive has no UV set; run `scene3d uv project` first")?;
    let mut out = Vec::with_capacity(prim.indices.len());
    for tri in prim.indices.chunks_exact(3) {
        for k in 0..3 {
            let a = set[tri[k] as usize];
            let b = set[tri[(k + 1) % 3] as usize];
            out.push([
                a[0] as f64 * size,
                a[1] as f64 * size,
                b[0] as f64 * size,
                b[1] as f64 * size,
            ]);
        }
    }
    Ok(out)
}

/// UV wireframe as SVG (origin top-left, `v` increasing downward).
pub fn layout_svg(prim: &Primitive, size: u32) -> Result<String> {
    ensure!(
        (16..=MAX_LAYOUT_SIZE).contains(&size),
        "[limit-exceeded] layout size must be 16..={MAX_LAYOUT_SIZE}"
    );
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" viewBox=\"0 0 {size} {size}\">\n<rect width=\"{size}\" height=\"{size}\" fill=\"#ffffff\" stroke=\"#9ca3af\"/>\n<path fill=\"none\" stroke=\"#111827\" stroke-width=\"1\" d=\""
    );
    for e in edges(prim, size as f64)? {
        svg.push_str(&format!("M{:.2} {:.2}L{:.2} {:.2}", e[0], e[1], e[2], e[3]));
    }
    svg.push_str("\"/>\n");
    let (seams, _) = uv_discontinuities(prim, 0);
    if !seams.is_empty() {
        let mut path = String::new();
        let mut rendered_segments = 0usize;
        'edge: for edge in seams.iter().take(MAX_SEAM_REPORT_EDGES) {
            for segment in &edge.segments {
                if rendered_segments >= MAX_SEAM_RENDER_SEGMENTS {
                    break 'edge;
                }
                rendered_segments += 1;
                path.push_str(&format!(
                    "M{:.2} {:.2}L{:.2} {:.2}",
                    segment[0][0] as f64 * size as f64,
                    segment[0][1] as f64 * size as f64,
                    segment[1][0] as f64 * size as f64,
                    segment[1][1] as f64 * size as f64,
                ));
            }
        }
        svg.push_str(&format!(
            "<path class=\"uv-discontinuity\" fill=\"none\" stroke=\"#b91c1c\" stroke-width=\"3\" vector-effect=\"non-scaling-stroke\" d=\"{path}\"/>\n"
        ));
    }
    svg.push_str("</svg>\n");
    Ok(svg)
}

/// UV wireframe as an opaque PNG.
pub fn layout_png(prim: &Primitive, size: u32) -> Result<Vec<u8>> {
    ensure!(
        (16..=MAX_LAYOUT_SIZE).contains(&size),
        "[limit-exceeded] layout size must be 16..={MAX_LAYOUT_SIZE}"
    );
    let mut img = image::RgbaImage::from_pixel(size, size, image::Rgba([255, 255, 255, 255]));
    let ink = image::Rgba([17, 24, 39, 255]);
    let seam_ink = image::Rgba([185, 28, 28, 255]);
    let mut plot = |x: i64, y: i64, color| {
        if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
            img.put_pixel(x as u32, y as u32, color);
        }
    };
    for e in edges(prim, size as f64)? {
        // Fixed-step DDA: deterministic and bounded by the image diagonal.
        let (dx, dy) = (e[2] - e[0], e[3] - e[1]);
        let steps = dx.abs().max(dy.abs()).ceil().clamp(1.0, (size * 4) as f64) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            plot(
                (e[0] + dx * t).floor() as i64,
                (e[1] + dy * t).floor() as i64,
                ink,
            );
        }
    }
    let (seams, _) = uv_discontinuities(prim, 0);
    let mut rendered_segments = 0usize;
    let mut rendered_steps = 0usize;
    'edge: for edge in seams.iter().take(MAX_SEAM_REPORT_EDGES) {
        for segment in &edge.segments {
            if rendered_segments >= MAX_SEAM_RENDER_SEGMENTS {
                break 'edge;
            }
            let (x0, y0) = (
                segment[0][0] as f64 * size as f64,
                segment[0][1] as f64 * size as f64,
            );
            let (x1, y1) = (
                segment[1][0] as f64 * size as f64,
                segment[1][1] as f64 * size as f64,
            );
            let (dx, dy) = (x1 - x0, y1 - y0);
            let steps = dx.abs().max(dy.abs()).ceil().clamp(1.0, (size * 4) as f64) as usize;
            if rendered_steps.saturating_add(steps + 1) > MAX_SEAM_RENDER_STEPS {
                break 'edge;
            }
            rendered_segments += 1;
            rendered_steps += steps + 1;
            for step in 0..=steps {
                let t = step as f64 / steps as f64;
                plot(
                    (x0 + dx * t).floor() as i64,
                    (y0 + dy * t).floor() as i64,
                    seam_ink,
                );
            }
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .context("could not encode the UV layout")?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> Primitive {
        Primitive {
            name: "quad".into(),
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 4.0, 0.0],
                [0.0, 4.0, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
            material: None,
        }
    }

    fn spec(projection: &str) -> Spec {
        parse_spec(&json!({"projection": projection})).unwrap()
    }

    #[test]
    fn planar_normalizes_to_the_unit_square() {
        let mut p = quad();
        project(&mut p, &spec("planar")).unwrap();
        let uv = &p.uvs[0];
        assert_eq!(uv[0], [0.0, 0.0]);
        assert_eq!(uv[2], [1.0, 1.0]);
        assert_eq!(p.positions.len(), 4, "planar must not split vertices");
    }

    #[test]
    fn transform_rotates_scales_and_offsets_deterministically() {
        let mut p = quad();
        let s = parse_spec(&json!({"projection": "planar", "scale": [2, 2], "offset": [0.25, 0]}))
            .unwrap();
        project(&mut p, &s).unwrap();
        assert_eq!(p.uvs[0][0], [-0.25, -0.5]);
        let mut a = quad();
        let mut b = quad();
        let r = parse_spec(&json!({"projection": "cylindrical", "rotate": 33.0})).unwrap();
        project(&mut a, &r).unwrap();
        project(&mut b, &r).unwrap();
        assert_eq!(a.uvs, b.uvs);
    }

    #[test]
    fn cylindrical_splits_vertices_and_avoids_seam_smear() {
        let mut p = quad();
        project(&mut p, &spec("cylindrical")).unwrap();
        assert_eq!(p.positions.len(), 6);
        assert_eq!(p.indices, vec![0, 1, 2, 3, 4, 5]);
        for tri in p.uvs[0].chunks_exact(3) {
            let lo = tri.iter().map(|t| t[0]).fold(f32::INFINITY, f32::min);
            let hi = tri.iter().map(|t| t[0]).fold(f32::NEG_INFINITY, f32::max);
            assert!(hi - lo <= 0.5 + 1e-6, "seam smear: {tri:?}");
        }
    }

    #[test]
    fn box_picks_the_dominant_axis_per_face() {
        let mut p = quad();
        project(&mut p, &spec("box")).unwrap();
        // A z-facing quad maps on x/y: corner (2,4,0) is (1,1).
        assert_eq!(p.uvs[0][2], [1.0, 1.0]);
    }

    #[test]
    fn texture_density_uses_uv_area_model_area_and_requested_resolution() {
        let mut primitive = quad();
        project(&mut primitive, &spec("planar")).unwrap();
        let mut model = Model::default();
        model.primitives.push(primitive);
        let report = show_with_resolution(&model, 2048).unwrap();
        let density = report["primitives"][0]["texel_density"][0]
            ["texel_density_px_per_model_unit"]
            .as_f64()
            .unwrap();
        assert!((density - 2048.0 / 8.0_f64.sqrt()).abs() < 1e-9);
        assert_eq!(
            report["primitives"][0]["texel_density"][0]["texture_resolution_px"],
            json!([2048, 2048])
        );

        let overlay = json!({"0":{"islands":[{
            "id":"all","uv_set":0,"triangles":[0,1],
            "transform":{"scale":[1,1],"offset":[0,0],"rotate":0}
        }]}});
        apply(&mut model, &overlay).unwrap();
        let report =
            show_with_overlays_at_resolution(&model, &overlay, 2048, Some(2048.0)).unwrap();
        let island_density = report["primitives"][0]["islands"][0]
            ["texel_density_px_per_model_unit"]
            .as_f64()
            .unwrap();
        assert!((island_density - 2048.0 / 8.0_f64.sqrt()).abs() < 1e-9);
        assert_eq!(
            report["primitives"][0]["islands"][0]["below_minimum_texel_density"],
            json!(true)
        );
        assert!(show_with_resolution(&model, MAX_TEXTURE_RESOLUTION + 1).is_err());
    }

    #[test]
    fn uv_discontinuities_are_reported_and_highlighted_in_the_layout() {
        let primitive = Primitive {
            name: "split-quad".into(),
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 4.0, 0.0],
                [0.0, 0.0, 0.0],
                [2.0, 4.0, 0.0],
                [0.0, 4.0, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 6],
            uvs: vec![vec![
                [0.0, 0.0],
                [1.0, 0.0],
                [1.0, 1.0],
                [0.2, 0.0],
                [1.2, 1.0],
                [0.0, 1.0],
            ]],
            indices: vec![0, 1, 2, 3, 4, 5],
            material: None,
        };
        let report = show_with_resolution(
            &Model {
                primitives: vec![primitive.clone()],
                ..Model::default()
            },
            1024,
        )
        .unwrap();
        let diagnostics = &report["primitives"][0]["uv_discontinuities"][0];
        assert_eq!(diagnostics["count"], json!(1));
        assert_eq!(diagnostics["edges"][0]["triangles"], json!([0, 1]));
        assert_eq!(diagnostics["analysis_truncated"], json!(false));
        let svg = layout_svg(&primitive, 256).unwrap();
        assert!(svg.contains("class=\"uv-discontinuity\""));
        assert!(svg.ends_with("</svg>\n"));
        let png = image::load_from_memory(&layout_png(&primitive, 256).unwrap())
            .unwrap()
            .to_rgba8();
        assert!(png.pixels().any(|pixel| pixel.0 == [185, 28, 28, 255]));

        let mut continuous = primitive;
        continuous.uvs[0][3] = [0.0, 0.0];
        continuous.uvs[0][4] = [1.0, 1.0];
        let report = show_with_resolution(
            &Model {
                primitives: vec![continuous],
                ..Model::default()
            },
            1024,
        )
        .unwrap();
        assert_eq!(
            report["primitives"][0]["uv_discontinuities"][0]["count"],
            json!(0)
        );
    }

    #[test]
    fn spec_validation_names_the_problem() {
        assert!(validate(&json!({"0": {"projection": "weird"}})).is_err());
        assert!(validate(&json!({"x": {"projection": "planar"}})).is_err());
        assert!(validate(&json!({"*": {"projection": "planar", "scale": [0, 1]}})).is_err());
        assert!(validate(&json!({"*": {"projection": "planar", "bogus": 1}})).is_err());
        assert!(
            validate(&json!({"*": {"projection": "planar"}, "3": {"projection": "box"}})).is_ok()
        );
    }

    #[test]
    fn editable_islands_transform_only_owned_triangle_corners() {
        let mut prim = quad();
        project(&mut prim, &spec("planar")).unwrap();
        let original = prim.uvs[0].clone();
        let overlay = json!({"0": {"islands": [{
            "id":"front", "uv_set":0, "triangles":[0],
            "transform":{"scale":[-1.0,1.0],"offset":[0.1,0.0],"rotate":0.0}
        }]}});
        let mut model = Model::default();
        model.primitives.push(prim);
        apply(&mut model, &overlay).unwrap();
        let edited = &model.primitives[0];
        assert_eq!(edited.indices.len(), 6);
        assert_eq!(edited.uvs[0][0][0], (4.0 / 3.0 + 0.1) as f32);
        assert_eq!(edited.uvs[0][1][0], (1.0 / 3.0 + 0.1) as f32);
        assert_eq!(
            edited.uvs[0][3], original[0],
            "the adjacent triangle keeps its own corner UV"
        );
        assert_ne!(
            edited.uvs[0][0], edited.uvs[0][3],
            "shared geometry is split at the editable seam"
        );
    }

    #[test]
    fn editable_islands_reject_duplicate_triangle_ownership_and_bad_indices() {
        let island = json!({"id":"a","uv_set":0,"triangles":[0],"transform":{"scale":[1,1],"offset":[0,0],"rotate":0}});
        let mut second_island = island.clone();
        second_island["id"] = json!("b");
        let mut model = Model::default();
        model.primitives.push(quad());
        let overlapping_overlay =
            json!({"0":{"projection":"planar","islands":[island.clone(), second_island]}});
        let error = apply(&mut model, &overlapping_overlay)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("belongs to more than one UV island"),
            "{error}"
        );
        let mut model = Model::default();
        model.primitives.push(quad());
        let out_of_range = json!({"0":{"projection":"planar","islands":[{"id":"a","uv_set":0,"triangles":[2],"transform":{"scale":[1,1],"offset":[0,0],"rotate":0}}]}});
        assert!(apply(&mut model, &out_of_range)
            .unwrap_err()
            .to_string()
            .contains("references triangle 2"));
        let mut no_uv_model = Model::default();
        no_uv_model.primitives.push(quad());
        let missing_uv = json!({"0":{"islands":[island]}});
        assert!(apply(&mut no_uv_model, &missing_uv)
            .unwrap_err()
            .to_string()
            .contains("UV set 0"));
    }

    #[test]
    fn island_overlap_uses_triangle_interiors_not_only_bounding_boxes() {
        let mut primitive = quad();
        primitive.positions = vec![[0.0, 0.0, 0.0]; 6];
        primitive.normals = vec![[0.0, 0.0, 1.0]; 6];
        primitive.indices = (0..6).collect();
        primitive.uvs = vec![vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
            [1.0, 1.0],
            [1.0, 0.6],
            [0.6, 1.0],
        ]];
        let overlays = json!({"0":{"islands":[
            {"id":"lower-left","uv_set":0,"triangles":[0],"transform":{"scale":[1,1],"offset":[0,0],"rotate":0}},
            {"id":"upper-right","uv_set":0,"triangles":[1],"transform":{"scale":[1,1],"offset":[0,0],"rotate":0}}
        ]}});
        let model = Model {
            primitives: vec![primitive.clone()],
            ..Model::default()
        };
        let report = show_with_overlays_at_resolution(&model, &overlays, 1024, None).unwrap();
        assert!(report["primitives"][0]["island_overlap_candidates"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            report["primitives"][0]["island_overlap_analysis_truncated"],
            json!(false)
        );

        primitive.uvs[0][3] = [0.2, 0.2];
        primitive.uvs[0][4] = [0.4, 0.2];
        primitive.uvs[0][5] = [0.2, 0.4];
        let overlapping = Model {
            primitives: vec![primitive],
            ..Model::default()
        };
        let report = show_with_overlays_at_resolution(&overlapping, &overlays, 1024, None).unwrap();
        assert_eq!(
            report["primitives"][0]["island_overlap_candidates"],
            json!([["lower-left", "upper-right"]])
        );
    }

    #[test]
    fn island_overlap_budget_is_explicit_and_keeps_all_islands_in_report() {
        let per_island = 1_001usize;
        let triangle_count = per_island * 2;
        let mut primitive = quad();
        primitive.positions = vec![[0.0, 0.0, 0.0]; triangle_count * 3];
        primitive.normals = vec![[0.0, 0.0, 1.0]; triangle_count * 3];
        primitive.indices = (0..(triangle_count * 3) as u32).collect();
        let lower = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let upper = [[1.0, 1.0], [1.0, 0.6], [0.6, 1.0]];
        let mut uvs = Vec::with_capacity(triangle_count * 3);
        for _ in 0..per_island {
            uvs.extend(lower);
        }
        for _ in 0..per_island {
            uvs.extend(upper);
        }
        primitive.uvs = vec![uvs];
        let overlays = json!({"0":{"islands":[
            {"id":"lower","uv_set":0,"triangles":(0..per_island).collect::<Vec<_>>(),"transform":{"scale":[1,1],"offset":[0,0],"rotate":0}},
            {"id":"upper","uv_set":0,"triangles":(per_island..triangle_count).collect::<Vec<_>>(),"transform":{"scale":[1,1],"offset":[0,0],"rotate":0}}
        ]}});
        let model = Model {
            primitives: vec![primitive],
            ..Model::default()
        };
        let report = show_with_overlays_at_resolution(&model, &overlays, 512, None).unwrap();
        assert_eq!(
            report["primitives"][0]["island_overlap_analysis_truncated"],
            json!(true)
        );
        assert_eq!(
            report["primitives"][0]["islands"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn wildcard_and_specific_overlays_cannot_own_the_same_triangle_twice() {
        let island = json!({"id":"front","uv_set":0,"triangles":[0],"transform":{"scale":[1,1],"offset":[0,0],"rotate":0}});
        let overlays = json!({
            "*":{"projection":"planar","islands":[island.clone()]},
            "0":{"islands":[{"id":"front-local","uv_set":0,"triangles":[0],"transform":{"scale":[1,1],"offset":[0,0],"rotate":0}}]}
        });
        let mut model = Model::default();
        model.primitives.push(quad());
        let error = apply(&mut model, &overlays).unwrap_err().to_string();
        assert!(
            error.contains("belongs to more than one UV island"),
            "{error}"
        );
    }

    #[test]
    fn show_reports_coverage_and_layout_renders() {
        let mut p = quad();
        project(&mut p, &spec("planar")).unwrap();
        let mut m = Model::default();
        m.primitives.push(p.clone());
        let report = show(&m);
        let coverage = report["primitives"][0]["coverage"].as_f64().unwrap();
        assert!(coverage > 0.99, "{report}");
        assert_eq!(report["primitives"][0]["degenerate_triangles"], 0);
        assert!(layout_svg(&p, 256).unwrap().contains("<path"));
        let png = layout_png(&p, 128).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert!(layout_svg(&p, 8).is_err());
        assert!(layout_svg(&quad(), 128).is_err(), "no UV set yet");
    }
}

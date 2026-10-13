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

#[derive(Clone, Copy, Debug, PartialEq)]
enum Projection {
    Planar,
    Box,
    Cylindrical,
    Spherical,
}

#[derive(Clone, Copy, Debug)]
struct Spec {
    projection: Projection,
    axis: usize,
    scale: [f64; 2],
    offset: [f64; 2],
    rotate: f64,
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
                "projection" | "axis" | "scale" | "offset" | "rotate"
            ),
            "[invalid-input] unknown uv overlay field `{key}`"
        );
    }
    let projection = match o.get("projection").and_then(Value::as_str) {
        Some("planar") => Projection::Planar,
        Some("box") => Projection::Box,
        Some("cylindrical") => Projection::Cylindrical,
        Some("spherical") => Projection::Spherical,
        other => bail!(
            "[invalid-input] uv projection {other:?} must be planar, box, cylindrical or spherical"
        ),
    };
    let axis = match o.get("axis").and_then(Value::as_str) {
        None => match projection {
            Projection::Planar => 2,
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
    })
}

/// Validate the `uv` overlay map of a model record.
pub fn validate(overlays: &Value) -> Result<()> {
    let map = overlays
        .as_object()
        .context("[invalid-input] model `uv` must be an object keyed by primitive index")?;
    for (key, spec) in map {
        ensure!(
            key == "*" || key.parse::<usize>().is_ok(),
            "[invalid-input] uv overlay key `{key}` must be a primitive index or `*`"
        );
        parse_spec(spec).with_context(|| format!("uv overlay `{key}`"))?;
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
    for (key, spec) in map {
        if key == "*" {
            for index in 0..count {
                order.push((index, parse_spec(spec)?));
            }
        }
    }
    for (key, spec) in map {
        if key != "*" {
            let index: usize = key.parse().expect("validated");
            ensure!(
                index < count,
                "[missing-resource] uv overlay targets primitive {index}, but the model has {count}"
            );
            order.push((index, parse_spec(spec)?));
        }
    }
    for (index, spec) in order {
        project(&mut model.primitives[index], &spec)?;
    }
    model.report.vertices = model.primitives.iter().map(|p| p.positions.len()).sum();
    Ok(())
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
    if spec.projection != Projection::Planar {
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
    match spec.projection {
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
            let sphere = spec.projection == Projection::Spherical;
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
    json!({"primitives": out})
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
    svg.push_str("\"/>\n</svg>\n");
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
    let mut plot = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
            img.put_pixel(x as u32, y as u32, ink);
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
            );
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

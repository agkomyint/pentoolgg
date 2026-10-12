//! Deterministic stroke fields (v0.13.0 task 6).
//!
//! A field is a bounded row-major grid of signed 16-bit unit vectors (protocol
//! "Scopes, constraints, palettes, and fields"). It is derived data: built from a
//! raster layer, a guide, or supplied vectors, hashed over method, source, bounds,
//! cell size and parameters, and pinned by the strokes painted through it. Fields
//! describe an undirected orientation; a stroke chooses the sign that continues its
//! heading. Painting seeds strokes on a jittered lattice with SplitMix64, walks
//! them along the field with a curvature limit, and applies collision rules before
//! handing ordinary strokes to the shared `paint` path.
use super::constraints::{atan2_q16, sobel};
use super::math::sin_cos_degrees;
use super::*;

pub const ALGORITHM: u32 = 1;
pub const MAX_FIELD_CELLS: u64 = 1_048_576;
pub const MAX_FIELD_STROKES: usize = 512;
pub const MAX_STROKE_SAMPLES: usize = 512;
const MAX_VECTOR_POINTS: usize = 256;
const MAX_DERIVED_PIXELS: u64 = 16_777_216;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Along {
    Contour,
    Gradient,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Method {
    /// Along the luma contour (perpendicular to the Sobel gradient).
    Contour,
    Gradient,
    /// Chamfer 3-4 distance transform of the opaque region (alpha >= 128).
    Distance(Along),
    /// Smoothed structure tensor over a `radius` window; follows the contour.
    Tensor(u32),
    /// Lines toward a vanishing point.
    Vanishing([f64; 2]),
    /// Tangents of circles around a center.
    Radial([f64; 2]),
    Angle(f64),
    /// `[x, y, degrees]` anchors; the nearest anchor wins, ties by lowest index.
    Vectors(Vec<[f64; 3]>),
}

#[derive(Clone, Debug)]
pub struct FieldSpec {
    pub method: Method,
    pub cell: u32,
    pub rect: Option<[u32; 4]>,
}

fn point(value: &Value, what: &str) -> Result<[f64; 2]> {
    let pair = value
        .as_array()
        .filter(|a| a.len() == 2)
        .and_then(|a| Some([a[0].as_f64()?, a[1].as_f64()?]))
        .filter(|p| p.iter().all(|v| v.is_finite() && v.abs() <= 1.0e6));
    pair.ok_or_else(|| bad("invalid-request", format!("field {what} must be [x,y]")))
}

impl FieldSpec {
    pub fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| bad("invalid-request", "field must be a JSON object"))?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "method"
                    | "cell"
                    | "rect"
                    | "radius"
                    | "along"
                    | "point"
                    | "center"
                    | "degrees"
                    | "points"
            ) {
                return Err(bad("invalid-request", format!("unknown field key {key:?}")));
            }
        }
        let cell = match object.get("cell") {
            None => 8,
            Some(v) => v
                .as_u64()
                .filter(|c| (1..=64).contains(c))
                .ok_or_else(|| bad("invalid-request", "field cell must be 1-64 pixels"))?
                as u32,
        };
        let rect = match object.get("rect") {
            None => None,
            Some(v) => {
                let r = v
                    .as_array()
                    .filter(|a| a.len() == 4)
                    .and_then(|a| {
                        let n: Option<Vec<u32>> = a
                            .iter()
                            .map(|n| n.as_u64().and_then(|n| u32::try_from(n).ok()))
                            .collect();
                        n
                    })
                    .filter(|r| r[2] > 0 && r[3] > 0)
                    .ok_or_else(|| {
                        bad(
                            "invalid-request",
                            "field rect must be [x,y,w,h] with positive size",
                        )
                    })?;
                Some([r[0], r[1], r[2], r[3]])
            }
        };
        let name = object
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("invalid-request", "field.method is required"))?;
        let method = match name {
            "contour" => Method::Contour,
            "gradient" => Method::Gradient,
            "distance" => Method::Distance(match object.get("along").and_then(Value::as_str) {
                None | Some("contour") => Along::Contour,
                Some("gradient") => Along::Gradient,
                Some(_) => return Err(bad("invalid-request", "field.along is contour or gradient")),
            }),
            "tensor" => Method::Tensor(
                match object.get("radius") {
                    None => 3,
                    Some(v) => v
                        .as_u64()
                        .filter(|r| (1..=8).contains(r))
                        .ok_or_else(|| bad("invalid-request", "field.radius must be 1-8"))?
                        as u32,
                },
            ),
            "vanishing" => Method::Vanishing(point(
                object.get("point").unwrap_or(&Value::Null),
                "point",
            )?),
            "radial" => Method::Radial(point(
                object.get("center").unwrap_or(&Value::Null),
                "center",
            )?),
            "angle" => Method::Angle(
                object
                    .get("degrees")
                    .and_then(Value::as_f64)
                    .filter(|d| d.is_finite() && d.abs() <= 360.0)
                    .ok_or_else(|| bad("invalid-request", "field.degrees must be -360..360"))?,
            ),
            "vectors" => {
                let list = object
                    .get("points")
                    .and_then(Value::as_array)
                    .filter(|a| !a.is_empty() && a.len() <= MAX_VECTOR_POINTS)
                    .ok_or_else(|| {
                        bad(
                            "invalid-request",
                            format!("field.points needs 1-{MAX_VECTOR_POINTS} [x,y,degrees] entries"),
                        )
                    })?;
                let mut points = Vec::new();
                for entry in list {
                    let t = entry
                        .as_array()
                        .filter(|a| a.len() == 3)
                        .and_then(|a| Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?]))
                        .filter(|p| p.iter().all(|v| v.is_finite() && v.abs() <= 1.0e6))
                        .ok_or_else(|| bad("invalid-request", "field.points entries are [x,y,degrees]"))?;
                    points.push(t);
                }
                Method::Vectors(points)
            }
            other => {
                return Err(bad(
                    "invalid-request",
                    format!("unknown field method {other:?}; use contour, gradient, distance, tensor, vanishing, radial, angle or vectors"),
                ))
            }
        };
        Ok(Self { method, cell, rect })
    }

    fn describe(&self) -> Value {
        match &self.method {
            Method::Contour => json!({"method": "contour"}),
            Method::Gradient => json!({"method": "gradient"}),
            Method::Distance(a) => {
                json!({"method": "distance", "along": if *a == Along::Contour {"contour"} else {"gradient"}})
            }
            Method::Tensor(r) => json!({"method": "tensor", "radius": r}),
            Method::Vanishing(p) => json!({"method": "vanishing", "point": p}),
            Method::Radial(p) => json!({"method": "radial", "center": p}),
            Method::Angle(d) => json!({"method": "angle", "degrees": d}),
            Method::Vectors(v) => json!({"method": "vectors", "points": v}),
        }
    }

    fn needs_raster(&self) -> bool {
        matches!(
            self.method,
            Method::Contour | Method::Gradient | Method::Distance(_) | Method::Tensor(_)
        )
    }
}

pub struct Field {
    pub cell: u32,
    pub rect: [u32; 4],
    pub cols: u32,
    pub rows: u32,
    pub vectors: Vec<[i16; 2]>,
    pub layer: (u32, u32),
    pub summary: Value,
}

fn milli(q16: i64) -> i64 {
    ((q16 * 1000 + 32768).div_euclid(65536)).rem_euclid(360_000)
}

fn vector(milli_degrees: i64) -> [i16; 2] {
    let (s, c) = sin_cos_degrees(milli_degrees as f64 / 1000.0);
    [(c * 32767.0).round() as i16, (s * 32767.0).round() as i16]
}

fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = (n as f64).sqrt() as u128;
    while x * x > n {
        x -= 1;
    }
    while (x + 1) * (x + 1) <= n {
        x += 1;
    }
    x
}

/// 3-4 chamfer distance (x3) from every opaque pixel to the nearest non-opaque one.
fn chamfer(surface: &Surface) -> Vec<u32> {
    let (w, h) = (surface.width as usize, surface.height as usize);
    let inf = u32::MAX / 2;
    let mut d: Vec<u32> = (0..w * h)
        .map(|i| {
            if surface.pixel((i % w) as u32, (i / w) as u32)[3] >= 128 {
                inf
            } else {
                0
            }
        })
        .collect();
    for y in 0..h {
        for x in 0..w {
            let mut v = d[y * w + x];
            if v == 0 {
                continue;
            }
            if x > 0 {
                v = v.min(d[y * w + x - 1] + 3);
            } else {
                v = v.min(3);
            }
            if y > 0 {
                v = v.min(d[(y - 1) * w + x] + 3);
                if x > 0 {
                    v = v.min(d[(y - 1) * w + x - 1] + 4);
                }
                if x + 1 < w {
                    v = v.min(d[(y - 1) * w + x + 1] + 4);
                }
            } else {
                v = v.min(3);
            }
            d[y * w + x] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let mut v = d[y * w + x];
            if v == 0 {
                continue;
            }
            if x + 1 < w {
                v = v.min(d[y * w + x + 1] + 3);
            } else {
                v = v.min(3);
            }
            if y + 1 < h {
                v = v.min(d[(y + 1) * w + x] + 3);
                if x + 1 < w {
                    v = v.min(d[(y + 1) * w + x + 1] + 4);
                }
                if x > 0 {
                    v = v.min(d[(y + 1) * w + x - 1] + 4);
                }
            } else {
                v = v.min(3);
            }
            d[y * w + x] = v;
        }
    }
    d
}

pub fn build(raw: &Value, id: &str, spec: &FieldSpec) -> Result<Field> {
    let node = all_rasters(raw)
        .into_iter()
        .find(|node| node["id"] == id)
        .ok_or_else(|| bad("not-found", format!("raster layer {id} was not found")))?;
    let surface = Surface::load(raw, node)?;
    let (lw, lh) = (surface.width, surface.height);
    let rect = spec.rect.unwrap_or([0, 0, lw, lh]);
    if u64::from(rect[0]) + u64::from(rect[2]) > u64::from(lw)
        || u64::from(rect[1]) + u64::from(rect[3]) > u64::from(lh)
    {
        return Err(bad(
            "invalid-request",
            format!("field rect {rect:?} (x, y, width, height in layer pixels) exceeds the {lw}x{lh} layer; keep x+width <= {lw} and y+height <= {lh}, or omit the rect to cover the whole layer"),
        ));
    }
    let (cols, rows) = (rect[2].div_ceil(spec.cell), rect[3].div_ceil(spec.cell));
    if u64::from(cols) * u64::from(rows) > MAX_FIELD_CELLS {
        return Err(bad(
            "limit-exceeded",
            format!(
                "the field has {} cells; the maximum is {MAX_FIELD_CELLS}; raise cell",
                u64::from(cols) * u64::from(rows)
            ),
        ));
    }
    let derived = u64::from(lw) * u64::from(lh);
    let needs_maps = matches!(spec.method, Method::Distance(_) | Method::Tensor(_));
    if needs_maps && derived > MAX_DERIVED_PIXELS {
        return Err(bad(
            "limit-exceeded",
            format!("{derived} layer pixels exceed {MAX_DERIVED_PIXELS} for distance/tensor fields; use contour or gradient"),
        ));
    }
    let dist = match spec.method {
        Method::Distance(_) => chamfer(&surface),
        _ => Vec::new(),
    };
    let (gx_map, gy_map): (Vec<i32>, Vec<i32>) = match spec.method {
        Method::Tensor(_) => (0..derived as usize)
            .map(|i| sobel(&surface, (i % lw as usize) as i64, (i / lw as usize) as i64))
            .unzip(),
        _ => (Vec::new(), Vec::new()),
    };
    let at_dist = |x: i64, y: i64| -> i64 {
        let px = x.clamp(0, i64::from(lw) - 1) as usize;
        let py = y.clamp(0, i64::from(lh) - 1) as usize;
        i64::from(dist[py * lw as usize + px])
    };
    let mut vectors = Vec::with_capacity((cols * rows) as usize);
    let mut defined = 0_u64;
    for cy in 0..rows {
        for cx in 0..cols {
            let x =
                i64::from((rect[0] + cx * spec.cell + spec.cell / 2).min(rect[0] + rect[2] - 1));
            let y =
                i64::from((rect[1] + cy * spec.cell + spec.cell / 2).min(rect[1] + rect[3] - 1));
            let angle: Option<i64> = match &spec.method {
                Method::Contour | Method::Gradient => {
                    let (gx, gy) = sobel(&surface, x, y);
                    if gx == 0 && gy == 0 {
                        None
                    } else {
                        let q = atan2_q16(i64::from(gy), i64::from(gx));
                        Some(milli(if spec.method == Method::Contour {
                            q + 90 * 65536
                        } else {
                            q
                        }))
                    }
                }
                Method::Distance(along) => {
                    let gx = at_dist(x + 1, y) - at_dist(x - 1, y);
                    let gy = at_dist(x, y + 1) - at_dist(x, y - 1);
                    if gx == 0 && gy == 0 {
                        None
                    } else {
                        let q = atan2_q16(gy, gx);
                        Some(milli(if *along == Along::Contour {
                            q + 90 * 65536
                        } else {
                            q
                        }))
                    }
                }
                Method::Tensor(r) => {
                    let r = i64::from(*r);
                    let (mut sxx, mut syy, mut sxy) = (0_i64, 0_i64, 0_i64);
                    for yy in (y - r).max(0)..=(y + r).min(i64::from(lh) - 1) {
                        for xx in (x - r).max(0)..=(x + r).min(i64::from(lw) - 1) {
                            let i = (yy * i64::from(lw) + xx) as usize;
                            let (gx, gy) = (i64::from(gx_map[i]), i64::from(gy_map[i]));
                            sxx += gx * gx;
                            syy += gy * gy;
                            sxy += gx * gy;
                        }
                    }
                    if sxx == 0 && syy == 0 {
                        None
                    } else {
                        let twice = atan2_q16(2 * sxy, sxx - syy);
                        Some(milli(twice.div_euclid(2) + 90 * 65536))
                    }
                }
                Method::Vanishing(p) | Method::Radial(p) => {
                    let dx = ((x as f64 - p[0]) * 16.0).round() as i64;
                    let dy = ((y as f64 - p[1]) * 16.0).round() as i64;
                    if dx == 0 && dy == 0 {
                        None
                    } else {
                        let q = atan2_q16(dy, dx);
                        Some(milli(if matches!(spec.method, Method::Radial(_)) {
                            q + 90 * 65536
                        } else {
                            q
                        }))
                    }
                }
                Method::Angle(d) => {
                    Some((d * 1000.0).round() as i64).map(|m| m.rem_euclid(360_000))
                }
                Method::Vectors(points) => {
                    let mut best: Option<(f64, f64)> = None;
                    for p in points {
                        let (dx, dy) = (x as f64 - p[0], y as f64 - p[1]);
                        let d = dx * dx + dy * dy;
                        if best.is_none_or(|(b, _)| d < b) {
                            best = Some((d, p[2]));
                        }
                    }
                    best.map(|(_, deg)| ((deg * 1000.0).round() as i64).rem_euclid(360_000))
                }
            };
            match angle {
                Some(m) => {
                    defined += 1;
                    vectors.push(vector(m));
                }
                None => vectors.push([0, 0]),
            }
        }
    }
    let source_hash = if spec.needs_raster() {
        surface.tile_map_hash()
    } else {
        "none".to_owned()
    };
    let params = spec.describe();
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    let header = format!(
        "pentool-stroke-field-v{ALGORITHM}\n{params}\n{source_hash}\n{rect:?}\n{}\n{cols}x{rows}\n",
        spec.cell
    );
    sha2::Digest::update(&mut hasher, header.as_bytes());
    for v in &vectors {
        sha2::Digest::update(&mut hasher, v[0].to_le_bytes());
        sha2::Digest::update(&mut hasher, v[1].to_le_bytes());
    }
    let field_hash = format!("sha256:{}", hex::encode(sha2::Digest::finalize(hasher)));
    let summary = json!({
        "algorithm": ALGORITHM,
        "field_hash": field_hash,
        "source_hash": source_hash,
        "params": params,
        "cell": spec.cell,
        "rect": rect,
        "columns": cols,
        "rows": rows,
        "defined_cells": defined,
    });
    Ok(Field {
        cell: spec.cell,
        rect,
        cols,
        rows,
        vectors,
        layer: (lw, lh),
        summary,
    })
}

impl Field {
    /// Interpolated unit vector at a layer coordinate, or `None` where undefined or
    /// outside the field rectangle. Neighbours are sign-aligned before blending
    /// because the field is an undirected orientation.
    pub fn at(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let r = self.rect;
        if x < f64::from(r[0])
            || y < f64::from(r[1])
            || x >= f64::from(r[0] + r[2])
            || y >= f64::from(r[1] + r[3])
        {
            return None;
        }
        let cell = f64::from(self.cell);
        let gx = (x - f64::from(r[0]) - cell / 2.0) / cell;
        let gy = (y - f64::from(r[1]) - cell / 2.0) / cell;
        let (x0, y0) = (gx.floor(), gy.floor());
        let fx = ((gx - x0) * 256.0).floor() as i64;
        let fy = ((gy - y0) * 256.0).floor() as i64;
        let (x0, y0) = (x0 as i64, y0 as i64);
        let cell_at = |cx: i64, cy: i64| {
            let cx = cx.clamp(0, i64::from(self.cols) - 1) as usize;
            let cy = cy.clamp(0, i64::from(self.rows) - 1) as usize;
            self.vectors[cy * self.cols as usize + cx]
        };
        let mut reference: Option<[i16; 2]> = None;
        let (mut ax, mut ay) = (0_i128, 0_i128);
        for (dx, dy, w) in [
            (0, 0, (256 - fx) * (256 - fy)),
            (1, 0, fx * (256 - fy)),
            (0, 1, (256 - fx) * fy),
            (1, 1, fx * fy),
        ] {
            let v = cell_at(x0 + dx, y0 + dy);
            if v == [0, 0] || w == 0 {
                continue;
            }
            let base = *reference.get_or_insert(v);
            let dot = i64::from(base[0]) * i64::from(v[0]) + i64::from(base[1]) * i64::from(v[1]);
            let sign = if dot < 0 { -1 } else { 1 };
            ax += i128::from(sign * i64::from(v[0]) * w);
            ay += i128::from(sign * i64::from(v[1]) * w);
        }
        let n = isqrt((ax * ax + ay * ay) as u128);
        if n == 0 {
            return None;
        }
        Some((ax as f64 / n as f64, ay as f64 / n as f64))
    }
}

#[derive(Clone, Debug)]
pub struct Params {
    pub spacing: f64,
    pub jitter: f64,
    pub length: f64,
    pub length_jitter: f64,
    pub step: f64,
    pub curvature: f64,
    pub overlap: f64,
    pub taper: f64,
    pub collision: Collision,
    pub min_length: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collision {
    /// A stroke ends when it would enter space occupied by an earlier stroke.
    Stop,
    /// Seeds in occupied space are skipped; strokes may cross freely afterwards.
    Skip,
    Allow,
}

impl Params {
    pub fn parse(value: &Value) -> Result<Self> {
        let empty = Map::new();
        let object = if value.is_null() {
            &empty
        } else {
            value
                .as_object()
                .ok_or_else(|| bad("invalid-request", "params must be a JSON object"))?
        };
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "spacing"
                    | "jitter"
                    | "length"
                    | "length_jitter"
                    | "step"
                    | "curvature"
                    | "overlap"
                    | "taper"
                    | "collision"
                    | "min_length"
            ) {
                return Err(bad(
                    "invalid-request",
                    format!("unknown params key {key:?}"),
                ));
            }
        }
        let num = |key: &str, default: f64, lo: f64, hi: f64| -> Result<f64> {
            match object.get(key) {
                None => Ok(default),
                Some(v) => v
                    .as_f64()
                    .filter(|n| n.is_finite() && (lo..=hi).contains(n))
                    .ok_or_else(|| {
                        bad("invalid-request", format!("params.{key} must be {lo}-{hi}"))
                    }),
            }
        };
        let step = num("step", 2.0, 0.5, 16.0)?;
        let collision = match object.get("collision").and_then(Value::as_str) {
            None | Some("stop") => Collision::Stop,
            Some("skip") => Collision::Skip,
            Some("allow") => Collision::Allow,
            Some(_) => {
                return Err(bad(
                    "invalid-request",
                    "params.collision is stop, skip or allow",
                ))
            }
        };
        Ok(Self {
            spacing: num("spacing", 8.0, 1.0, 256.0)?,
            jitter: num("jitter", 0.5, 0.0, 1.0)?,
            length: num("length", 48.0, 1.0, 2048.0)?,
            length_jitter: num("length_jitter", 0.25, 0.0, 1.0)?,
            step,
            curvature: num("curvature", 20.0, 0.0, 90.0)?,
            overlap: num("overlap", 0.5, 0.0, 1.0)?,
            taper: num("taper", 0.0, 0.0, 0.5)?,
            collision,
            min_length: num("min_length", 2.0 * step, 0.0, 2048.0)?,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "spacing": self.spacing, "jitter": self.jitter, "length": self.length,
            "length_jitter": self.length_jitter, "step": self.step,
            "curvature": self.curvature, "overlap": self.overlap, "taper": self.taper,
            "collision": match self.collision {
                Collision::Stop => "stop", Collision::Skip => "skip", Collision::Allow => "allow",
            },
            "min_length": self.min_length,
        })
    }
}

/// Journal samples are canonical to 1/1000; round here so replay is exact.
fn canonical(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// One planned stroke: path samples `[x, y, pressure]`.
pub struct Planned {
    pub samples: Vec<[f64; 3]>,
}

/// Seed and trace strokes without touching pixels. `allowed` is an optional
/// layer-sized coverage plane (zero = forbidden).
pub fn plan(
    field: &Field,
    params: &Params,
    brush_size: f64,
    seed: u64,
    allowed: Option<&[u8]>,
) -> Result<Vec<Planned>> {
    let r = field.rect;
    let (lw, _) = field.layer;
    let permitted = |x: f64, y: f64| -> bool {
        if x < f64::from(r[0]) || y < f64::from(r[1]) {
            return false;
        }
        match allowed {
            None => true,
            Some(plane) => {
                let (px, py) = (x.floor() as usize, y.floor() as usize);
                plane.get(py * lw as usize + px).is_some_and(|c| *c > 0)
            }
        }
    };
    let occupancy_cell = (brush_size * (1.0 - params.overlap)).max(1.0);
    let mut grid: HashMap<(i64, i64), u32> = HashMap::new();
    let key = |x: f64, y: f64| {
        (
            (x / occupancy_cell).floor() as i64,
            (y / occupancy_cell).floor() as i64,
        )
    };
    let mut state = seed ^ 0x6669_656C_6473_0001;
    let mut out: Vec<Planned> = Vec::new();
    let lattice_x = (f64::from(r[2]) / params.spacing).ceil() as u64;
    let lattice_y = (f64::from(r[3]) / params.spacing).ceil() as u64;
    if lattice_x.saturating_mul(lattice_y) > 4_194_304 {
        return Err(bad(
            "limit-exceeded",
            format!("seed lattice of {lattice_x}x{lattice_y} exceeds 4194304; raise params.spacing or shrink the field rect"),
        ));
    }
    for ly in 0..lattice_y {
        for lx in 0..lattice_x {
            // Every lattice point consumes the same four values, used or not.
            let (jx, jy) = (signed_unit(&mut state), signed_unit(&mut state));
            let (lj, sign) = (signed_unit(&mut state), splitmix64(&mut state) & 1);
            let sx =
                f64::from(r[0]) + (lx as f64 + 0.5 + 0.5 * params.jitter * jx) * params.spacing;
            let sy =
                f64::from(r[1]) + (ly as f64 + 0.5 + 0.5 * params.jitter * jy) * params.spacing;
            if sx >= f64::from(r[0] + r[2]) || sy >= f64::from(r[1] + r[3]) {
                continue;
            }
            if !permitted(sx, sy) || field.at(sx, sy).is_none() {
                continue;
            }
            if params.collision != Collision::Allow && grid.contains_key(&key(sx, sy)) {
                continue;
            }
            if out.len() >= MAX_FIELD_STROKES {
                return Err(bad(
                    "limit-exceeded",
                    format!("the field would generate more than {MAX_FIELD_STROKES} strokes; raise params.spacing, shrink the rect or constrain the region"),
                ));
            }
            let id = out.len() as u32 + 1;
            let budget = params.length * (1.0 - params.length_jitter * (lj * 0.5 + 0.5));
            let mut pos = (sx, sy);
            let mut heading = field
                .at(sx, sy)
                .map(|(x, y)| if sign == 0 { (x, y) } else { (-x, -y) });
            let mut points = vec![pos];
            let mut travelled = 0.0;
            grid.entry(key(sx, sy)).or_insert(id);
            while travelled + params.step <= budget && points.len() < MAX_STROKE_SAMPLES {
                let Some(dir) = field.at(pos.0, pos.1) else {
                    break;
                };
                let h = heading.unwrap_or(dir);
                let d = if dir.0 * h.0 + dir.1 * h.1 < 0.0 {
                    (-dir.0, -dir.1)
                } else {
                    dir
                };
                let cross = h.0 * d.1 - h.1 * d.0;
                let dot = h.0 * d.0 + h.1 * d.1;
                let turn = atan2_q16((cross * 1.0e6).round() as i64, (dot * 1.0e6).round() as i64)
                    as f64
                    / 65536.0;
                let new = if turn.abs() <= params.curvature {
                    d
                } else {
                    let (s, c) = sin_cos_degrees(params.curvature.copysign(turn));
                    (h.0 * c - h.1 * s, h.0 * s + h.1 * c)
                };
                let next = (pos.0 + new.0 * params.step, pos.1 + new.1 * params.step);
                if !permitted(next.0, next.1) || field.at(next.0, next.1).is_none() {
                    break;
                }
                if params.collision == Collision::Stop {
                    if let Some(owner) = grid.get(&key(next.0, next.1)) {
                        if *owner != id {
                            break;
                        }
                    }
                }
                grid.entry(key(next.0, next.1)).or_insert(id);
                pos = next;
                heading = Some(new);
                travelled += params.step;
                points.push(pos);
            }
            if travelled < params.min_length || points.len() < 2 {
                // Release this stroke's cells so a later seed may use the space.
                grid.retain(|_, owner| *owner != id);
                continue;
            }
            let n = points.len();
            let samples = points
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let t = i as f64 / (n - 1) as f64;
                    let pressure = if params.taper > 0.0 {
                        0.05 + 0.95 * (t.min(1.0 - t) / params.taper).min(1.0)
                    } else {
                        1.0
                    };
                    [canonical(p.0), canonical(p.1), canonical(pressure)]
                })
                .collect();
            out.push(Planned { samples });
        }
    }
    Ok(out)
}

pub struct FieldRequest {
    pub field: Value,
    pub params: Value,
    pub brush: Brush,
    pub color: [u8; 3],
    pub blend: Blend,
    pub seed: u64,
    pub constraints: Value,
}

fn bounds_of(planned: &[Planned]) -> Value {
    let mut b: Option<[f64; 4]> = None;
    for p in planned.iter().flat_map(|p| p.samples.iter()) {
        let e = b.get_or_insert([p[0], p[1], p[0], p[1]]);
        e[0] = e[0].min(p[0]);
        e[1] = e[1].min(p[1]);
        e[2] = e[2].max(p[0]);
        e[3] = e[3].max(p[1]);
    }
    json!(b)
}

/// Resolve the optional constraint mask without installing it.
fn mask_for(raw: &Value, id: &str, constraints: &Value) -> Result<Option<constraints::Resolved>> {
    if constraints.is_null() {
        return Ok(None);
    }
    let spec = constraints::Spec::parse(constraints)?;
    if spec.follow.is_some() {
        return Err(bad(
            "invalid-request",
            "constraints.follow does not apply to field painting; the field sets direction",
        ));
    }
    Ok(Some(constraints::resolve(raw, id, &spec)?))
}

/// Plan only: the generated strokes' count, samples and bounds.
pub fn plan_report(raw: &Value, id: &str, request: &FieldRequest) -> Result<Value> {
    let spec = FieldSpec::parse(&crate::paint_guide::resolve_field(raw, &request.field)?)?;
    let params = Params::parse(&request.params)?;
    let field = build(raw, id, &spec)?;
    let mask = mask_for(raw, id, &request.constraints)?;
    let planned = plan(
        &field,
        &params,
        request.brush.size,
        request.seed,
        mask.as_ref().map(|m| m.plane.as_slice()),
    )?;
    Ok(report(
        &field,
        &params,
        &planned,
        mask.as_ref().map(|m| &m.summary),
    ))
}

fn report(field: &Field, params: &Params, planned: &[Planned], mask: Option<&Value>) -> Value {
    json!({
        "field": field.summary,
        "params": params.to_json(),
        "constraints": mask.cloned().unwrap_or(Value::Null),
        "strokes": planned.len(),
        "samples": planned.iter().map(|p| p.samples.len()).sum::<usize>(),
        "bounds": bounds_of(planned),
    })
}

pub fn paint_field(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    request: FieldRequest,
) -> Result<Value> {
    let spec = FieldSpec::parse(&crate::paint_guide::resolve_field(raw, &request.field)?)?;
    let params = Params::parse(&request.params)?;
    let field = build(raw, id, &spec)?;
    let mask = mask_for(raw, id, &request.constraints)?;
    let planned = plan(
        &field,
        &params,
        request.brush.size,
        request.seed,
        mask.as_ref().map(|m| m.plane.as_slice()),
    )?;
    if planned.is_empty() {
        return Err(bad(
            "empty-field",
            format!(
                "the field generated no stroke (field {}); relax spacing, min_length, collision or constraints",
                field.summary["field_hash"]
            ),
        ));
    }
    let mut next = raw.clone();
    let mut previous = None;
    if let Some(resolved) = &mask {
        if resolved.plane.iter().all(|c| *c == 0) {
            return Err(bad(
                "empty-constraint",
                "the constraints leave no paintable pixel",
            ));
        }
        previous = Some(constraints::install(&mut next, id, resolved)?);
    }
    let pin = json!({
        "algorithm": ALGORITHM,
        "field_hash": field.summary["field_hash"],
        "method": field.summary["params"]["method"],
    });
    let mut brush = request.brush.clone();
    if params.taper > 0.0 {
        brush.pressure_size = true;
    }
    let (mut dabs, mut ids) = (0_u64, Vec::new());
    for (index, stroke) in planned.iter().enumerate() {
        let samples = stroke
            .samples
            .iter()
            .map(|s| Sample {
                x: s[0],
                y: s[1],
                pressure: s[2],
                ..Sample::default()
            })
            .collect();
        let result = paint(
            &mut next,
            page,
            id,
            StrokeRequest {
                brush: brush.clone(),
                samples,
                color: request.color,
                blend: request.blend,
                seed: request.seed.wrapping_add(index as u64),
                clone: None,
            },
        )?;
        dabs += result["dabs"].as_u64().unwrap_or(0);
        if let Some(stroke_id) = result["stroke"].as_str() {
            if let Some(entry) = locate(&mut next, page, id)?["journal"]
                .as_array_mut()
                .and_then(|journal| journal.last_mut())
            {
                if entry["id"] == stroke_id {
                    entry["field"] = pin.clone();
                }
            }
            ids.push(stroke_id.to_owned());
        }
    }
    if let Some(previous) = previous {
        constraints::restore_active(&mut next, id, previous);
    }
    crate::scene::validate(&next)?;
    *raw = next;
    let mut result = report(&field, &params, &planned, mask.as_ref().map(|m| &m.summary));
    result["dabs"] = json!(dabs);
    result["stroke_ids"] = json!(ids);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_and_params_are_strict() {
        assert!(FieldSpec::parse(&json!({"method": "nope"})).is_err());
        assert!(FieldSpec::parse(&json!({"method": "angle"})).is_err());
        assert!(FieldSpec::parse(&json!({"method": "angle", "degrees": 10, "x": 1})).is_err());
        assert!(FieldSpec::parse(&json!({"method": "contour", "cell": 0})).is_err());
        assert!(Params::parse(&json!({"spacing": 0})).is_err());
        assert!(Params::parse(&json!({"collision": "bounce"})).is_err());
        assert!(Params::parse(&json!({})).is_ok());
    }

    #[test]
    fn isqrt_is_exact() {
        for n in [0_u128, 1, 2, 3, 4, 15, 16, 17, 1 << 62, (1 << 62) + 1] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{n}");
        }
    }

    #[test]
    fn sign_aligned_interpolation_keeps_a_line_field_straight() {
        // 89 and 91 degrees are the same orientation up to sign; blending must
        // not collapse to zero.
        let field = Field {
            cell: 4,
            rect: [0, 0, 8, 4],
            cols: 2,
            rows: 1,
            vectors: vec![vector(89_000), vector(271_000)],
            layer: (8, 4),
            summary: Value::Null,
        };
        let (x, y) = field.at(4.0, 2.0).unwrap();
        assert!(x.abs() < 0.05 && y.abs() > 0.99, "{x},{y}");
    }
}

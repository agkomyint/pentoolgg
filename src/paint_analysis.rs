//! `pentool paint-analyze` (v0.13.0 task 8): compact, read-only visual analysis of
//! an explicit bounded scope for agents.
//!
//! The scope is rendered by the authoritative renderer, cropped, then reduced to at
//! most `--max-edge` pixels with the integer box filter. Everything after that is
//! integer arithmetic on that analysis image, so results are identical on every
//! platform. Output is structured summaries (polylines, regions, histograms), never
//! pixels; list features are paginated and every response names the renderer,
//! algorithm, page, scope, document revision and source/analysis hashes.
use crate::paint_review::{self as review, hash_image, ratio};
use crate::raster::constraints::atan2_q16;
use anyhow::{bail, ensure, Context, Result};
use image::RgbaImage;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub const ALGORITHM: u32 = 1;
pub const DEFAULT_MAX_EDGE: u32 = 256;
const MAX_EDGE: u32 = 1024;
pub const MAX_LIMIT: usize = 500;
const FEATURES: [&str; 9] = [
    "contours",
    "silhouettes",
    "values",
    "palette",
    "texture",
    "orientation",
    "focal",
    "gaps",
    "clipping",
];

pub struct Options {
    pub scope: String,
    pub rect: Option<[u32; 4]>,
    pub selection: Option<Value>,
    pub features: Vec<String>,
    pub max_edge: u32,
    pub values: usize,
    pub colors: usize,
    pub edge_threshold: i64,
    pub background: [u8; 3],
    pub limit: usize,
    pub offset: usize,
}

struct Plane {
    w: usize,
    h: usize,
    luma: Vec<i64>,
    rgb: Vec<[u8; 3]>,
    opaque: Vec<bool>,
}

impl Plane {
    fn new(image: &RgbaImage, background: [u8; 3]) -> Self {
        let (w, h) = (image.width() as usize, image.height() as usize);
        let mut plane = Plane {
            w,
            h,
            luma: Vec::with_capacity(w * h),
            rgb: Vec::with_capacity(w * h),
            opaque: Vec::with_capacity(w * h),
        };
        for p in image.pixels() {
            let a = u32::from(p[3]);
            let mix =
                |c: u8, b: u8| ((u32::from(c) * a + u32::from(b) * (255 - a) + 127) / 255) as u8;
            let c = [
                mix(p[0], background[0]),
                mix(p[1], background[1]),
                mix(p[2], background[2]),
            ];
            plane.luma.push(
                (54 * i64::from(c[0]) + 183 * i64::from(c[1]) + 19 * i64::from(c[2]) + 128) >> 8,
            );
            plane.rgb.push(c);
            plane.opaque.push(p[3] > 0);
        }
        plane
    }

    fn l(&self, x: i64, y: i64) -> i64 {
        self.luma[y.clamp(0, self.h as i64 - 1) as usize * self.w
            + x.clamp(0, self.w as i64 - 1) as usize]
    }

    fn sobel(&self, x: i64, y: i64) -> (i64, i64) {
        let gx = (self.l(x + 1, y - 1) + 2 * self.l(x + 1, y) + self.l(x + 1, y + 1))
            - (self.l(x - 1, y - 1) + 2 * self.l(x - 1, y) + self.l(x - 1, y + 1));
        let gy = (self.l(x - 1, y + 1) + 2 * self.l(x, y + 1) + self.l(x + 1, y + 1))
            - (self.l(x - 1, y - 1) + 2 * self.l(x, y - 1) + self.l(x + 1, y - 1));
        (gx, gy)
    }
}

/// Maps analysis coordinates back to scope pixels.
#[derive(Clone, Copy)]
struct Space {
    sw: u64,
    sh: u64,
    aw: u64,
    ah: u64,
    origin: [u32; 2],
}

impl Space {
    fn x(&self, x: usize) -> u64 {
        u64::from(self.origin[0]) + x as u64 * self.sw / self.aw
    }
    fn y(&self, y: usize) -> u64 {
        u64::from(self.origin[1]) + y as u64 * self.sh / self.ah
    }
    /// `[x, y, w, h]` for the analysis box `[x0, y0, x1, y1)`.
    fn rect(&self, b: [usize; 4]) -> [u64; 4] {
        let (x0, y0) = (self.x(b[0]), self.y(b[1]));
        let (x1, y1) = (self.x(b[2]), self.y(b[3]));
        [x0, y0, x1 - x0, y1 - y0]
    }
}

fn page(items: Vec<Value>, offset: usize, limit: usize) -> Value {
    let matches = items.len();
    let shown: Vec<Value> = items.into_iter().skip(offset).take(limit).collect();
    json!({
        "matches": matches,
        "returned": shown.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset + shown.len() < matches,
        "items": shown,
    })
}

/// Corner neighbours sit at even indices.
const NEIGHBORS: [(i64, i64); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Connected components of `mask` in raster order; `diag` selects 8-connectivity.
fn components(mask: &[bool], w: usize, h: usize, diag: bool) -> Vec<Vec<usize>> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    for start in 0..w * h {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let mut cells = Vec::new();
        while let Some(i) = stack.pop() {
            cells.push(i);
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for (k, (dx, dy)) in NEIGHBORS.iter().enumerate() {
                if !diag && k % 2 == 0 {
                    continue;
                }
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let n = ny as usize * w + nx as usize;
                if mask[n] && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        cells.sort_unstable();
        out.push(cells);
    }
    out
}

fn bbox(cells: &[usize], w: usize) -> [usize; 4] {
    let mut b = [usize::MAX, usize::MAX, 0, 0];
    for &i in cells {
        let (x, y) = (i % w, i / w);
        b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
    }
    b
}

/// Douglas-Peucker with squared tolerance `eps2` in analysis pixels.
fn simplify(points: &[(i64, i64)], eps2: i64) -> Vec<(i64, i64)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (ax, ay) = points[a];
        let (dx, dy) = (points[b].0 - ax, points[b].1 - ay);
        let len2 = dx * dx + dy * dy;
        let (mut far, mut far_d) = (0usize, -1i64);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let d2 = if len2 == 0 {
                (p.0 - ax).pow(2) + (p.1 - ay).pow(2)
            } else {
                let cross = dx * (p.1 - ay) - dy * (p.0 - ax);
                cross * cross / len2
            };
            if d2 > far_d {
                far_d = d2;
                far = i;
            }
        }
        if far_d > eps2 {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    points
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect()
}

fn gradient(plane: &Plane) -> Vec<(i64, i64)> {
    let mut g = Vec::with_capacity(plane.w * plane.h);
    for y in 0..plane.h as i64 {
        for x in 0..plane.w as i64 {
            g.push(plane.sobel(x, y));
        }
    }
    g
}

fn contours(plane: &Plane, grad: &[(i64, i64)], threshold: i64, space: Space) -> Vec<Value> {
    let (w, h) = (plane.w, plane.h);
    let mag = |i: usize| grad[i].0.abs() + grad[i].1.abs();
    let mut edge = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let m = mag(i);
            if m < threshold {
                continue;
            }
            // Non-maximum suppression along the quantised gradient direction.
            let (gx, gy) = grad[i];
            let (dx, dy) = if gy.abs() * 2 <= gx.abs() {
                (1, 0)
            } else if gx.abs() * 2 <= gy.abs() {
                (0, 1)
            } else if (gx > 0) == (gy > 0) {
                (1, 1)
            } else {
                (1, -1)
            };
            let at = |sx: i64, sy: i64| -> i64 {
                let (nx, ny) = (x as i64 + sx, y as i64 + sy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    0
                } else {
                    mag(ny as usize * w + nx as usize)
                }
            };
            edge[i] = m >= at(dx, dy) && m > at(-dx, -dy);
        }
    }
    let mut found: Vec<(usize, Vec<usize>)> = components(&edge, w, h, true)
        .into_iter()
        .filter(|c| c.len() >= 6)
        .map(|c| (c.len(), c))
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1[0].cmp(&b.1[0])));
    found
        .into_iter()
        .enumerate()
        .map(|(n, (len, cells))| {
            let in_set: HashSet<usize> = cells.iter().copied().collect();
            let neighbors = |i: usize| -> Vec<usize> {
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                NEIGHBORS
                    .iter()
                    .filter_map(|(dx, dy)| {
                        let (nx, ny) = (x + dx, y + dy);
                        (nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64)
                            .then(|| ny as usize * w + nx as usize)
                    })
                    .filter(|n| in_set.contains(n))
                    .collect()
            };
            // Start at an endpoint (fewest neighbours), ties in raster order.
            let start = cells
                .iter()
                .copied()
                .min_by_key(|&i| (neighbors(i).len(), i))
                .unwrap_or(cells[0]);
            let mut visited = HashSet::new();
            let mut chain = vec![start];
            visited.insert(start);
            let mut current = start;
            while let Some(next) = neighbors(current)
                .into_iter()
                .find(|n| !visited.contains(n))
            {
                visited.insert(next);
                chain.push(next);
                current = next;
            }
            let pts: Vec<(i64, i64)> = chain
                .iter()
                .map(|&i| ((i % w) as i64, (i / w) as i64))
                .collect();
            let simple = simplify(&pts, 2);
            let strength: i64 = cells.iter().map(|&i| mag(i)).sum::<i64>() / len as i64;
            let (a, b) = (pts[0], pts[pts.len() - 1]);
            let closed = pts.len() > 8 && (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2) <= 2;
            json!({
                "id": format!("c{n}"),
                "pixels": len,
                "traced": chain.len(),
                "closed": closed,
                "strength": strength,
                "bounds": space.rect(bbox(&cells, w)),
                "polyline": simple
                    .iter()
                    .map(|(x, y)| json!([space.x(*x as usize), space.y(*y as usize)]))
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn silhouettes(plane: &Plane, space: Space) -> Vec<Value> {
    let mut comps = components(&plane.opaque, plane.w, plane.h, false);
    comps.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    let total = plane.w as u64 * plane.h as u64;
    comps
        .iter()
        .enumerate()
        .map(|(n, c)| {
            let (sx, sy) = c.iter().fold((0u64, 0u64), |(sx, sy), &i| {
                (sx + (i % plane.w) as u64, sy + (i / plane.w) as u64)
            });
            let area = c.len() as u64;
            json!({
                "id": format!("s{n}"),
                "area": ratio(area, total),
                "bounds": space.rect(bbox(c, plane.w)),
                // Pixel centres sit half a pixel past their index; truncating the mean index
                // biased every centroid toward the top-left (#386).
                "centroid": [
                    space.x(((2 * sx + area) / (2 * area)) as usize),
                    space.y(((2 * sy + area) / (2 * area)) as usize),
                ],
            })
        })
        .collect()
}

fn values(plane: &Plane, levels: usize, space: Space) -> Value {
    let mut cells: Vec<Vec<usize>> = vec![Vec::new(); levels];
    let mut hist = [0u64; 16];
    let mut total = 0u64;
    for i in 0..plane.w * plane.h {
        if !plane.opaque[i] {
            continue;
        }
        let l = plane.luma[i].clamp(0, 255) as usize;
        cells[l * levels / 256].push(i);
        hist[l / 16] += 1;
        total += 1;
    }
    let zones: Vec<Value> = cells
        .iter()
        .enumerate()
        .map(|(level, c)| {
            let mean = if c.is_empty() {
                Value::Null
            } else {
                json!(c.iter().map(|&i| plane.luma[i]).sum::<i64>() / c.len() as i64)
            };
            json!({
                "level": level,
                "luma": [level * 256 / levels, ((level + 1) * 256 / levels).saturating_sub(1)],
                "pixels": c.len(),
                "share": ratio(c.len() as u64, total),
                "mean_luma": mean,
                "bounds": if c.is_empty() { Value::Null } else { json!(space.rect(bbox(c, plane.w))) },
            })
        })
        .collect();
    json!({"levels": levels, "zones": zones, "histogram16": hist})
}

fn palette(plane: &Plane, count: usize) -> Value {
    // 4 bits per channel; ties break on the bin index.
    let mut bins: BTreeMap<u16, (u64, [u64; 3])> = BTreeMap::new();
    let mut total = 0u64;
    for (i, c) in plane.rgb.iter().enumerate() {
        if !plane.opaque[i] {
            continue;
        }
        let key = (u16::from(c[0] >> 4) << 8) | (u16::from(c[1] >> 4) << 4) | u16::from(c[2] >> 4);
        let e = bins.entry(key).or_insert((0, [0; 3]));
        e.0 += 1;
        for (sum, v) in e.1.iter_mut().zip(c) {
            *sum += u64::from(*v);
        }
        total += 1;
    }
    let mut list: Vec<_> = bins.into_iter().collect();
    list.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
    let colors: Vec<Value> = list
        .iter()
        .take(count)
        .map(|(_, (n, sum))| {
            let c: Vec<u64> = sum.iter().map(|s| (s + n / 2) / n).collect();
            json!({
                "color": format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
                "pixels": n,
                "share": ratio(*n, total),
            })
        })
        .collect();
    json!({"distinct_bins": list.len(), "colors": colors})
}

fn texture(plane: &Plane) -> Value {
    let bins = review::texture_bins(&plane.luma, plane.w, plane.h);
    let total: u64 = bins.iter().sum();
    let busy: u64 = bins[2..].iter().sum();
    let band = bins
        .iter()
        .enumerate()
        .skip(1)
        .max_by_key(|(i, n)| (**n, usize::MAX - i))
        .map_or(0, |(i, _)| i);
    let density = (busy * 100).checked_div(total).unwrap_or(0);
    json!({
        "laplacian_bins": bins,
        "bin_upper_bounds": [7, 15, 31, 63, 127, 255],
        "busy_share": ratio(busy, total),
        "dominant_band": band,
        "frequency": match density { 0..=4 => "smooth", 5..=24 => "moderate", _ => "busy" },
    })
}

fn orientation(
    plane: &Plane,
    grad: &[(i64, i64)],
    threshold: i64,
    space: Space,
    side: usize,
) -> (Value, Vec<Value>) {
    const BINS: usize = 8;
    let bin_of = |gx: i64, gy: i64| -> usize {
        let a = atan2_q16(gy, gx).rem_euclid(180 * 65536);
        // Nearest 22.5 degree bin; the last wraps onto the first.
        (((a * BINS as i64 * 2 / (180 * 65536)) + 1) / 2) as usize % BINS
    };
    let mut global = [0u64; BINS];
    let (cw, ch) = (plane.w.div_ceil(side).max(1), plane.h.div_ceil(side).max(1));
    let mut grid = vec![[0u64; BINS]; side * side];
    for y in 0..plane.h {
        for x in 0..plane.w {
            let (gx, gy) = grad[y * plane.w + x];
            let m = gx.abs() + gy.abs();
            if m < threshold {
                continue;
            }
            let b = bin_of(gx, gy);
            global[b] += m as u64;
            grid[(y / ch).min(side - 1) * side + (x / cw).min(side - 1)][b] += m as u64;
        }
    }
    let top = |bins: &[u64; BINS]| -> Option<usize> {
        let (i, n) = bins
            .iter()
            .enumerate()
            .max_by_key(|(i, n)| (**n, BINS - i))?;
        (*n > 0).then_some(i)
    };
    let degrees = |b: usize| (b as i64 * 225) as f64 / 10.0;
    let total: u64 = global.iter().sum();
    let summary = match top(&global) {
        None => json!({"state": "flat", "dominant_gradient_degrees": null}),
        Some(b) => json!({
            "state": "ok",
            "dominant_gradient_degrees": degrees(b),
            "suggested_stroke_degrees": (degrees(b) + 90.0) % 180.0,
            "bins": global.iter().enumerate().map(|(i, n)| json!({
                "gradient_degrees": degrees(i),
                "weight": ratio(*n, total),
            })).collect::<Vec<_>>(),
        }),
    };
    let cells = grid
        .iter()
        .enumerate()
        .filter_map(|(i, bins)| {
            let b = top(bins)?;
            let (cx, cy) = (i % side, i / side);
            let area = [
                cx * cw,
                cy * ch,
                ((cx + 1) * cw).min(plane.w),
                ((cy + 1) * ch).min(plane.h),
            ];
            Some(json!({
                "bounds": space.rect(area),
                "suggested_stroke_degrees": (degrees(b) + 90.0) % 180.0,
                "strength": bins[b],
            }))
        })
        .collect();
    (summary, cells)
}

fn focal(plane: &Plane, grad: &[(i64, i64)], space: Space) -> Vec<Value> {
    let side = 8usize;
    let (cw, ch) = (plane.w.div_ceil(side).max(1), plane.h.div_ceil(side).max(1));
    let mut scored = Vec::new();
    for cy in 0..side {
        for cx in 0..side {
            let (x0, y0) = (cx * cw, cy * ch);
            let (x1, y1) = ((x0 + cw).min(plane.w), (y0 + ch).min(plane.h));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let (mut edge, mut lo, mut hi, mut n) = (0i64, 255i64, 0i64, 0i64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = y * plane.w + x;
                    if !plane.opaque[i] {
                        continue;
                    }
                    n += 1;
                    edge += grad[i].0.abs() + grad[i].1.abs();
                    lo = lo.min(plane.luma[i]);
                    hi = hi.max(plane.luma[i]);
                }
            }
            if n == 0 {
                continue;
            }
            scored.push((edge / n + (hi - lo), cy * side + cx, [x0, y0, x1, y1]));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .filter(|s| s.0 > 0)
        .take(8)
        .enumerate()
        .map(|(rank, (score, _, b))| {
            json!({
                "rank": rank + 1,
                "score": score,
                "bounds": space.rect(b),
                "center": [space.x((b[0] + b[2]) / 2), space.y((b[1] + b[3]) / 2)],
            })
        })
        .collect()
}

fn gaps(plane: &Plane, space: Space) -> (Value, Vec<Value>) {
    let total = (plane.w * plane.h) as u64;
    let empty: Vec<bool> = plane.opaque.iter().map(|o| !o).collect();
    let covered = plane.opaque.iter().filter(|o| **o).count() as u64;
    let min_area = (total / 1000).max(4) as usize;
    let mut comps: Vec<Vec<usize>> = components(&empty, plane.w, plane.h, false)
        .into_iter()
        .filter(|c| c.len() >= min_area)
        .collect();
    comps.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    let items = comps
        .iter()
        .enumerate()
        .map(|(n, c)| {
            let b = bbox(c, plane.w);
            let touches = b[0] == 0 || b[1] == 0 || b[2] == plane.w || b[3] == plane.h;
            json!({
                "id": format!("g{n}"),
                "area": ratio(c.len() as u64, total),
                "bounds": space.rect(b),
                "touches_scope_edge": touches,
            })
        })
        .collect();
    (json!({"coverage": ratio(covered, total)}), items)
}

fn clipping(plane: &Plane) -> Value {
    let (mut dark, mut light, mut n) = (0u64, 0u64, 0u64);
    for i in 0..plane.w * plane.h {
        if !plane.opaque[i] {
            continue;
        }
        n += 1;
        match plane.luma[i] {
            0..=2 => dark += 1,
            253..=255 => light += 1,
            _ => {}
        }
    }
    json!({"shadows": ratio(dark, n), "highlights": ratio(light, n), "opaque_pixels": n})
}

pub fn analyze(raw: &Value, document: &Path, page_id: Option<&str>, o: Options) -> Result<Value> {
    ensure!(
        (16..=MAX_EDGE).contains(&o.max_edge),
        "[invalid-request] --max-edge must be between 16 and {MAX_EDGE}"
    );
    ensure!(
        (2..=8).contains(&o.values),
        "[invalid-request] --values must be between 2 and 8"
    );
    ensure!(
        (1..=32).contains(&o.colors),
        "[invalid-request] --colors must be between 1 and 32"
    );
    ensure!(
        (1..=1020).contains(&o.edge_threshold),
        "[invalid-request] --edge-threshold must be between 1 and 1020"
    );
    ensure!(
        (1..=MAX_LIMIT).contains(&o.limit),
        "[invalid-request] --limit must be between 1 and {MAX_LIMIT}"
    );
    let mut wanted: Vec<&str> = Vec::new();
    for f in &o.features {
        if f == "all" {
            wanted.extend(FEATURES);
        } else if let Some(known) = FEATURES.iter().find(|k| *k == f) {
            wanted.push(known);
        } else {
            bail!(
                "[invalid-request] unknown feature {f:?}; use all or {}",
                FEATURES.join(", ")
            )
        }
    }
    wanted.sort_unstable();
    wanted.dedup();
    let bytes = std::fs::read(document)
        .with_context(|| format!("[missing-resource] cannot read {}", document.display()))?;
    let source = review::crop(
        raw,
        document,
        page_id,
        &o.scope,
        o.rect,
        o.selection.as_ref(),
    )?;
    let reduced = review::downscale(&source.image, o.max_edge);
    let plane = Plane::new(&reduced, o.background);
    let space = Space {
        sw: u64::from(source.image.width()),
        sh: u64::from(source.image.height()),
        aw: plane.w as u64,
        ah: plane.h as u64,
        origin: source.origin,
    };
    let grad = gradient(&plane);
    let has = |name: &str| wanted.contains(&name);
    let mut out = json!({
        "algorithm": ALGORITHM,
        "status": "ok",
        "renderer": "pentool-cpu",
        "page": page_id,
        "scope": source.scope,
        "revision": crate::transaction::revision(&bytes),
        "origin": source.origin,
        "source": [source.image.width(), source.image.height()],
        "analysis": [plane.w, plane.h],
        "source_hash": source.hash,
        "analysis_hash": hash_image(&reduced),
        "features": wanted,
    });
    if has("contours") {
        let items = contours(&plane, &grad, o.edge_threshold, space);
        out["contours"] = page(items, o.offset, o.limit);
    }
    if has("silhouettes") {
        out["silhouettes"] = page(silhouettes(&plane, space), o.offset, o.limit);
    }
    if has("values") {
        out["values"] = values(&plane, o.values, space);
    }
    if has("palette") {
        out["palette"] = palette(&plane, o.colors);
    }
    if has("texture") {
        out["texture"] = texture(&plane);
    }
    if has("orientation") {
        let (summary, cells) = orientation(&plane, &grad, o.edge_threshold, space, 4);
        out["orientation"] = summary;
        out["orientation"]["cells"] = page(cells, o.offset, o.limit);
    }
    if has("focal") {
        out["focal"] = page(focal(&plane, &grad, space), o.offset, o.limit);
    }
    if has("gaps") {
        let (summary, items) = gaps(&plane, space);
        out["gaps"] = summary;
        out["gaps"]["regions"] = page(items, o.offset, o.limit);
    }
    if has("clipping") {
        out["clipping"] = clipping(&plane);
    }
    Ok(out)
}

#[derive(clap::Args, Debug)]
pub struct AnalyzeArgs {
    pub input: PathBuf,
    /// page, node:ID, layer:ID or group:ID.
    #[arg(long, default_value = "page")]
    pub scope: String,
    /// Crop as x,y,width,height in scope pixels.
    #[arg(long)]
    pub rect: Option<String>,
    /// Selection query JSON; the crop is its bounds, outside pixels cleared.
    #[arg(long)]
    pub selection: Option<String>,
    /// Comma-separated: all, contours, silhouettes, values, palette, texture,
    /// orientation, focal, gaps, clipping.
    #[arg(long, value_delimiter = ',', default_value = "all")]
    pub features: Vec<String>,
    /// Longest edge of the analysis image (16-1024).
    #[arg(long, default_value_t = DEFAULT_MAX_EDGE)]
    pub max_edge: u32,
    /// Value zones (2-8).
    #[arg(long, default_value_t = 5)]
    pub values: usize,
    /// Dominant colors to report (1-32).
    #[arg(long, default_value_t = 8)]
    pub colors: usize,
    /// Minimum Sobel L1 gradient magnitude counted as an edge.
    #[arg(long, default_value_t = 128)]
    pub edge_threshold: i64,
    /// Background transparent pixels are composited over for value analysis.
    #[arg(long, default_value = "#ffffff")]
    pub background: String,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

pub fn run(args: AnalyzeArgs, page_id: Option<&str>) -> Result<()> {
    let raw: Value = serde_json::from_slice(
        &std::fs::read(&args.input)
            .with_context(|| format!("[missing-resource] cannot read {}", args.input.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    let selection = match args.selection.as_deref() {
        None => None,
        Some(text) => {
            Some(serde_json::from_str(text).context("[invalid-request] --selection must be JSON")?)
        }
    };
    let result = analyze(
        &raw,
        &args.input,
        page_id,
        Options {
            scope: args.scope,
            rect: args.rect.as_deref().map(review::parse_rect).transpose()?,
            selection,
            features: args.features,
            max_edge: args.max_edge,
            values: args.values,
            colors: args.colors,
            edge_threshold: args.edge_threshold,
            background: review::parse_background(&args.background)?,
            limit: args.limit,
            offset: args.offset,
        },
    )?;
    println!("{result}");
    Ok(())
}

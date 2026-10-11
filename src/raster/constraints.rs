//! Region-aware painting constraints (v0.13.0 task 5).
//!
//! A constraint spec resolves, once and before any dab is applied, into a layer
//! sized coverage plane. The plane is stored as the layer's active selection for
//! the duration of the stroke so the existing pinned-selection path applies it
//! exactly once per pixel (no tile seams, no boundary double application) and
//! journals the exact tiles that were used. Direction following uses an integer
//! Sobel gradient and an integer CORDIC `atan2`, so angles are identical on every
//! platform.
use super::*;

pub const ALGORITHM: u32 = 1;
const MAX_SOURCES: usize = 16;
/// Temporary saved-selection slot that pins the replaced active selection.
const PREVIOUS: &str = "__constraint_previous__";
const MAX_BARRIERS: usize = 16;
const MAX_RESOLVED_PIXELS: u64 = 67_108_864;

#[derive(Clone, Debug, PartialEq)]
pub enum Barrier {
    /// Pixels within an L1 RGBA distance of `color` stop the stroke.
    Color { color: [u8; 4], tolerance: u32 },
    /// Pixels whose alpha lies in `min..=max`.
    Alpha { min: u8, max: u8 },
    /// Opaque-ish (alpha > 0) pixels whose luma lies in `min..=max`.
    Luma { min: u8, max: u8 },
    /// Pixels whose `|gx| + |gy|` luma Sobel magnitude is at least `threshold`.
    Edge { threshold: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Follow {
    Contour,
    Gradient,
    /// Fixed brush angle in degrees.
    Angle(f64),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spec {
    pub inside: Vec<String>,
    pub avoid: Vec<String>,
    pub opaque_clip: Option<u8>,
    pub barriers: Vec<Barrier>,
    pub reach_from: Option<[u32; 2]>,
    pub follow: Option<Follow>,
}

fn reject_unknown(
    object: &serde_json::Map<String, Value>,
    allowed: &[&str],
    what: &str,
) -> Result<()> {
    if let Some(key) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        bail!(
            "[invalid-request] unknown {what} field {key:?}; use {}",
            allowed.join(", ")
        )
    }
    Ok(())
}

fn source_list(value: Option<&Value>, name: &str) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let list = value
        .as_array()
        .with_context(|| format!("[invalid-request] constraints.{name} must be an array"))?;
    if list.len() > MAX_SOURCES {
        bail!(
            "[limit-exceeded] constraints.{name} lists {} sources; allowed {MAX_SOURCES}",
            list.len()
        )
    }
    list.iter()
        .map(|entry| {
            let text = entry.as_str().with_context(|| {
                format!("[invalid-request] constraints.{name} entries are \"active\" or \"saved:NAME\"")
            })?;
            if text != "active" && !text.strip_prefix("saved:").is_some_and(|n| !n.is_empty()) {
                bail!("[invalid-request] constraints.{name} entry {text:?} must be \"active\" or \"saved:NAME\"")
            }
            Ok(text.to_owned())
        })
        .collect()
}

fn byte(value: &Value, key: &str, default: u8) -> Result<u8> {
    match value.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= 255)
            .map(|n| n as u8)
            .with_context(|| format!("[invalid-request] barrier {key} must be an integer 0-255")),
    }
}

impl Spec {
    pub fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("[invalid-request] constraints must be a JSON object")?;
        reject_unknown(
            object,
            &[
                "inside",
                "avoid",
                "opaque_clip",
                "barriers",
                "reach_from",
                "follow",
            ],
            "constraints",
        )?;
        let mut spec = Spec {
            inside: source_list(object.get("inside"), "inside")?,
            avoid: source_list(object.get("avoid"), "avoid")?,
            ..Spec::default()
        };
        spec.opaque_clip = match object.get("opaque_clip") {
            None | Some(Value::Bool(false)) => None,
            Some(Value::Bool(true)) => Some(128),
            Some(v) => Some(
                v.as_u64()
                    .filter(|n| (1..=255).contains(n))
                    .map(|n| n as u8)
                    .context(
                        "[invalid-request] opaque_clip must be true or a minimum alpha 1-255",
                    )?,
            ),
        };
        if let Some(list) = object.get("barriers") {
            let list = list
                .as_array()
                .context("[invalid-request] constraints.barriers must be an array")?;
            if list.len() > MAX_BARRIERS {
                bail!(
                    "[limit-exceeded] {} barriers; allowed {MAX_BARRIERS}",
                    list.len()
                )
            }
            for entry in list {
                let kind = entry["kind"].as_str().context(
                    "[invalid-request] each barrier needs kind color, alpha, luma or edge",
                )?;
                let fields = entry
                    .as_object()
                    .context("[invalid-request] barrier must be an object")?;
                spec.barriers.push(match kind {
                    "color" => {
                        reject_unknown(fields, &["kind", "color", "tolerance"], "color barrier")?;
                        let color = super::parse_color(
                            entry["color"].as_str().context("[invalid-request] color barrier needs color")?,
                        )?;
                        let alpha = entry["color"]
                            .as_str()
                            .filter(|t| t.trim_start_matches('#').len() == 8)
                            .map_or(255, |t| u8::from_str_radix(&t[t.len() - 2..], 16).unwrap_or(255));
                        let tolerance = entry
                            .get("tolerance")
                            .map_or(Some(0), Value::as_u64)
                            .filter(|n| *n <= 1020)
                            .context("[invalid-request] color tolerance must be an integer 0-1020")?;
                        Barrier::Color {
                            color: [color[0], color[1], color[2], alpha],
                            tolerance: tolerance as u32,
                        }
                    }
                    "alpha" | "luma" => {
                        reject_unknown(fields, &["kind", "min", "max"], "range barrier")?;
                        let (min, max) = (byte(entry, "min", 0)?, byte(entry, "max", 255)?);
                        if min > max {
                            bail!("[invalid-request] {kind} barrier min {min} exceeds max {max}")
                        }
                        if kind == "alpha" {
                            Barrier::Alpha { min, max }
                        } else {
                            Barrier::Luma { min, max }
                        }
                    }
                    "edge" => {
                        reject_unknown(fields, &["kind", "threshold"], "edge barrier")?;
                        let threshold = entry["threshold"]
                            .as_u64()
                            .filter(|n| (1..=2040).contains(n))
                            .context("[invalid-request] edge threshold must be an integer 1-2040")?;
                        Barrier::Edge { threshold: threshold as u32 }
                    }
                    other => bail!(
                        "[invalid-request] unknown barrier kind {other:?}; use color, alpha, luma or edge"
                    ),
                });
            }
        }
        if let Some(seed) = object.get("reach_from") {
            let pair = seed
                .as_array()
                .filter(|a| a.len() == 2)
                .and_then(|a| Some([a[0].as_u64()?, a[1].as_u64()?]))
                .filter(|p| p.iter().all(|n| *n < MAX_DIMENSION))
                .context("[invalid-request] reach_from must be [x,y] in layer pixels")?;
            spec.reach_from = Some([pair[0] as u32, pair[1] as u32]);
        }
        spec.follow = match object.get("follow") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s == "contour" => Some(Follow::Contour),
            Some(Value::String(s)) if s == "gradient" => Some(Follow::Gradient),
            Some(v) => {
                let angle = v
                    .get("angle")
                    .and_then(Value::as_f64)
                    .filter(|a| a.is_finite() && (-360.0..=360.0).contains(a))
                    .with_context(|| {
                        "[invalid-request] follow must be \"contour\", \"gradient\" or {\"angle\": -360..360}"
                    })?;
                Some(Follow::Angle(angle))
            }
        };
        Ok(spec)
    }

    /// Canonical JSON: the parsed form, stable field order.
    pub fn to_json(&self) -> Value {
        let barriers: Vec<Value> = self
            .barriers
            .iter()
            .map(|b| match b {
                Barrier::Color { color, tolerance } => json!({
                    "kind": "color",
                    "color": format!("#{:02x}{:02x}{:02x}{:02x}", color[0], color[1], color[2], color[3]),
                    "tolerance": tolerance,
                }),
                Barrier::Alpha { min, max } => json!({"kind": "alpha", "min": min, "max": max}),
                Barrier::Luma { min, max } => json!({"kind": "luma", "min": min, "max": max}),
                Barrier::Edge { threshold } => json!({"kind": "edge", "threshold": threshold}),
            })
            .collect();
        json!({
            "algorithm": ALGORITHM,
            "inside": self.inside,
            "avoid": self.avoid,
            "opaque_clip": self.opaque_clip,
            "barriers": barriers,
            "reach_from": self.reach_from,
            "follow": match self.follow {
                None => Value::Null,
                Some(Follow::Contour) => json!("contour"),
                Some(Follow::Gradient) => json!("gradient"),
                Some(Follow::Angle(a)) => json!({"angle": a}),
            },
        })
    }
}

pub struct Resolved {
    /// Coverage plane in row-major layer pixels.
    pub plane: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub summary: Value,
}

impl Resolved {
    pub fn as_selection(&self) -> Surface {
        let mut surface = Surface {
            width: self.width,
            height: self.height,
            tiles: BTreeMap::new(),
        };
        for y in 0..self.height {
            for x in 0..self.width {
                let value = self.plane[(y * self.width + x) as usize];
                if value != 0 {
                    selection::set_coverage(&mut surface, x, y, value);
                }
            }
        }
        surface
    }
}

fn plane_of(selection: &Surface) -> Vec<u8> {
    let mut plane = vec![0u8; selection.width as usize * selection.height as usize];
    for (key, tile) in &selection.tiles {
        for ty in 0..TILE {
            let y = key.1 as usize * TILE + ty;
            if y >= selection.height as usize {
                break;
            }
            for tx in 0..TILE {
                let x = key.0 as usize * TILE + tx;
                if x >= selection.width as usize {
                    break;
                }
                plane[y * selection.width as usize + x] = tile[(ty * TILE + tx) * 4 + 3];
            }
        }
    }
    plane
}

fn luma(p: [u8; 4]) -> i32 {
    (54 * i32::from(p[0]) + 183 * i32::from(p[1]) + 19 * i32::from(p[2]) + 128) >> 8
}

/// Straight-alpha luma weighted by alpha so transparent pixels read as black.
fn weighted_luma(p: [u8; 4]) -> i32 {
    (luma(p) * i32::from(p[3]) + 127) / 255
}

fn sobel(surface: &Surface, x: i64, y: i64) -> (i32, i32) {
    let at = |dx: i64, dy: i64| {
        let px = (x + dx).clamp(0, i64::from(surface.width) - 1) as u32;
        let py = (y + dy).clamp(0, i64::from(surface.height) - 1) as u32;
        weighted_luma(surface.pixel(px, py))
    };
    let gx = (at(1, -1) + 2 * at(1, 0) + at(1, 1)) - (at(-1, -1) + 2 * at(-1, 0) + at(-1, 1));
    let gy = (at(-1, 1) + 2 * at(0, 1) + at(1, 1)) - (at(-1, -1) + 2 * at(0, -1) + at(1, -1));
    (gx, gy)
}

/// `atan2(y, x)` in degrees scaled by 65536, via integer CORDIC (vectoring mode).
pub fn atan2_q16(y: i64, x: i64) -> i64 {
    const TABLE: [i64; 20] = [
        2949120, 1740967, 919879, 466945, 234379, 117304, 58666, 29335, 14668, 7334, 3667, 1833,
        917, 458, 229, 115, 57, 29, 14, 7,
    ];
    if x == 0 && y == 0 {
        return 0;
    }
    let (mut x, mut y, mut z) = if x < 0 {
        (-x << 24, -y << 24, 180 * 65536)
    } else {
        (x << 24, y << 24, 0)
    };
    for (i, step) in TABLE.iter().enumerate() {
        if y > 0 {
            (x, y, z) = (x + (y >> i), y - (x >> i), z + step);
        } else {
            (x, y, z) = (x - (y >> i), y + (x >> i), z - step);
        }
    }
    // Wrap into (-180, 180].
    let full = 360 * 65536;
    let mut z = z.rem_euclid(full);
    if z > full / 2 {
        z -= full;
    }
    z
}

/// Brush angle in milli-degrees for `follow` at a layer pixel, or `None` where the
/// gradient is flat (the brush keeps its own angle there).
pub fn follow_angle_milli(surface: &Surface, follow: Follow, x: f64, y: f64) -> Option<i64> {
    let (px, py) = (x.floor() as i64, y.floor() as i64);
    match follow {
        Follow::Angle(a) => Some((a * 1000.0).round() as i64),
        Follow::Gradient | Follow::Contour => {
            let (gx, gy) = sobel(surface, px, py);
            if gx == 0 && gy == 0 {
                return None;
            }
            let mut q = atan2_q16(i64::from(gy), i64::from(gx));
            if follow == Follow::Contour {
                q += 90 * 65536;
            }
            let milli = (q * 1000 + 32768).div_euclid(65536).rem_euclid(360_000);
            Some(if milli > 180_000 {
                milli - 360_000
            } else {
                milli
            })
        }
    }
}

fn named_selection(raw: &Value, id: &str, name: &str, width: u32, height: u32) -> Result<Vec<u8>> {
    let surface = if name == "active" {
        selection::active(raw, id)?.with_context(|| {
            format!("[invalid-selection] layer {id} has no active selection; select first or use saved:NAME")
        })?
    } else {
        let key = &name["saved:".len()..];
        let entry = raw["raster_selections"]["saved"][id]
            .get(key)
            .filter(|e| e.is_object())
            .with_context(|| {
                format!("[invalid-selection] layer {id} has no saved selection {key:?}; list with select-info")
            })?;
        selection::from_json(raw, entry)?
    };
    if (surface.width, surface.height) != (width, height) {
        bail!(
            "[invalid-selection] selection {name:?} is {}x{} but layer {id} is {width}x{height}",
            surface.width,
            surface.height
        )
    }
    Ok(plane_of(&surface))
}

/// Resolve `spec` for layer `id` of `raw` into a coverage plane.
pub fn resolve(raw: &Value, id: &str, spec: &Spec) -> Result<Resolved> {
    let node = all_rasters(raw)
        .into_iter()
        .find(|node| node["id"] == id)
        .ok_or_else(|| bad("not-found", format!("raster layer {id} was not found")))?;
    let layer = Surface::load(raw, node)?;
    let (width, height) = (layer.width, layer.height);
    let count = u64::from(width) * u64::from(height);
    if count > MAX_RESOLVED_PIXELS {
        bail!("[limit-exceeded] constraints resolve {count} pixels; allowed {MAX_RESOLVED_PIXELS}")
    }
    let mut plane = vec![255u8; count as usize];
    for name in &spec.inside {
        let cover = named_selection(raw, id, name, width, height)?;
        for (out, c) in plane.iter_mut().zip(cover) {
            *out = (*out).min(c);
        }
    }
    for name in &spec.avoid {
        let cover = named_selection(raw, id, name, width, height)?;
        for (out, c) in plane.iter_mut().zip(cover) {
            *out = ((u32::from(*out) * (255 - u32::from(c)) + 127) / 255) as u8;
        }
    }
    let needs_pixels = spec.opaque_clip.is_some() || !spec.barriers.is_empty();
    let mut blocked = 0u64;
    if needs_pixels {
        for y in 0..height {
            for x in 0..width {
                let i = (y * width + x) as usize;
                if plane[i] == 0 {
                    continue;
                }
                let p = layer.pixel(x, y);
                let mut stop = spec.opaque_clip.is_some_and(|min| p[3] < min);
                for barrier in &spec.barriers {
                    stop = stop
                        || match *barrier {
                            Barrier::Color { color, tolerance } => {
                                let d: u32 =
                                    (0..4).map(|c| u32::from(p[c].abs_diff(color[c]))).sum();
                                d <= tolerance
                            }
                            Barrier::Alpha { min, max } => (min..=max).contains(&p[3]),
                            Barrier::Luma { min, max } => {
                                p[3] > 0 && (i32::from(min)..=i32::from(max)).contains(&luma(p))
                            }
                            Barrier::Edge { threshold } => {
                                let (gx, gy) = sobel(&layer, i64::from(x), i64::from(y));
                                (gx.unsigned_abs() + gy.unsigned_abs()) >= threshold
                            }
                        };
                    if stop {
                        break;
                    }
                }
                if stop {
                    plane[i] = 0;
                    blocked += 1;
                }
            }
        }
    }
    if let Some([sx, sy]) = spec.reach_from {
        if sx >= width || sy >= height {
            bail!("[invalid-request] reach_from [{sx},{sy}] is outside the {width}x{height} layer")
        }
        let start = (sy * width + sx) as usize;
        if plane[start] == 0 {
            bail!("[invalid-request] reach_from [{sx},{sy}] is blocked by the constraints; choose an allowed pixel")
        }
        let mut seen = vec![false; plane.len()];
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % width as usize, i / width as usize);
            let mut push = |j: usize| {
                if plane[j] != 0 && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < width as usize {
                push(i + 1);
            }
            if y > 0 {
                push(i - width as usize);
            }
            if y + 1 < height as usize {
                push(i + width as usize);
            }
        }
        for (cover, reached) in plane.iter_mut().zip(&seen) {
            if !reached {
                *cover = 0;
            }
        }
    }
    let allowed = plane.iter().filter(|c| **c != 0).count() as u64;
    let full = plane.iter().filter(|c| **c == 255).count() as u64;
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(
        &mut hasher,
        format!("pentool-constraint-mask-v1\n{width}x{height}\n").as_bytes(),
    );
    sha2::Digest::update(&mut hasher, &plane);
    let mask_hash = format!("sha256:{}", hex::encode(sha2::Digest::finalize(hasher)));
    let canonical = spec.to_json();
    let spec_hash = crate::resource::sha256(canonical.to_string().as_bytes());
    let summary = json!({
        "algorithm": ALGORITHM,
        "width": width,
        "height": height,
        "allowed_pixels": allowed,
        "full_coverage_pixels": full,
        "barrier_blocked_pixels": blocked,
        "mask_hash": mask_hash,
        "spec_hash": spec_hash,
        "spec": canonical,
    });
    Ok(Resolved {
        plane,
        width,
        height,
        summary,
    })
}

/// Intersect `resolved` with a layer's existing active selection, if any, and
/// store the result as the active selection of `id` in `raw`. Returns the previous
/// entry so [`restore_active`] can put it back.
pub fn install(raw: &mut Value, id: &str, resolved: &Resolved) -> Result<Value> {
    let previous = raw["raster_selections"]["active"]
        .get(id)
        .cloned()
        .unwrap_or(Value::Null);
    let mut surface = resolved.as_selection();
    if let Some(existing) = selection::active(raw, id)? {
        if (existing.width, existing.height) != (surface.width, surface.height) {
            bail!("[invalid-selection] the active selection of {id} does not match the layer size")
        }
        let old = plane_of(&existing);
        let mut merged = Surface {
            width: surface.width,
            height: surface.height,
            tiles: BTreeMap::new(),
        };
        for (i, c) in plane_of(&surface).into_iter().enumerate() {
            let v = c.min(old[i]);
            if v != 0 {
                selection::set_coverage(
                    &mut merged,
                    (i % surface.width as usize) as u32,
                    (i / surface.width as usize) as u32,
                    v,
                );
            }
        }
        surface = merged;
    }
    // Keep the replaced selection referenced so the garbage collector run by each
    // stroke cannot release its tiles before it is restored.
    if !previous.is_null() {
        raw["raster_selections"]["saved"][id][PREVIOUS] = previous.clone();
    }
    selection::store_active(raw, id, &surface)?;
    Ok(previous)
}

pub fn restore_active(raw: &mut Value, id: &str, previous: Value) {
    if let Some(all) = raw
        .get_mut("raster_selections")
        .and_then(|s| s.get_mut("saved"))
        .and_then(Value::as_object_mut)
    {
        if let Some(saved) = all.get_mut(id).and_then(Value::as_object_mut) {
            saved.remove(PREVIOUS);
            if saved.is_empty() {
                all.remove(id);
            }
        }
        if all.is_empty() {
            if let Some(root) = raw["raster_selections"].as_object_mut() {
                root.remove("saved");
            }
        }
    }
    if previous.is_null() {
        selection::clear(raw, id);
        if raw["raster_selections"]["active"]
            .as_object()
            .is_some_and(|m| m.is_empty())
        {
            if let Some(root) = raw["raster_selections"].as_object_mut() {
                root.remove("active");
                if root.is_empty() {
                    if let Some(doc) = raw.as_object_mut() {
                        doc.remove("raster_selections");
                    }
                }
            }
        }
    } else {
        raw["raster_selections"]["active"][id] = previous;
    }
}

/// Grayscale PNG of the resolved coverage (255 = fully paintable).
pub fn mask_png(resolved: &Resolved) -> Result<Vec<u8>> {
    let pixels = u64::from(resolved.width) * u64::from(resolved.height);
    if pixels > 4_194_304 {
        bail!("[limit-exceeded] a mask preview of {pixels} pixels exceeds 4194304; preview a smaller layer")
    }
    let image = image::GrayImage::from_raw(resolved.width, resolved.height, resolved.plane.clone())
        .context("mask plane has the wrong size")?;
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .context("could not encode mask preview")?;
    Ok(out.into_inner())
}

/// The direction field `follow` would use, sampled on a `cell`-pixel grid. The
/// digest covers every cell; `samples` lists at most `max_vectors` of them as
/// `[x, y, milli_degrees]` (flat cells are omitted).
pub fn field_preview(
    raw: &Value,
    id: &str,
    follow: Follow,
    cell: u32,
    max_vectors: usize,
) -> Result<Value> {
    if !(1..=512).contains(&cell) {
        bail!("[invalid-request] field cell must be 1-512 pixels")
    }
    let node = all_rasters(raw)
        .into_iter()
        .find(|node| node["id"] == id)
        .ok_or_else(|| bad("not-found", format!("raster layer {id} was not found")))?;
    let surface = Surface::load(raw, node)?;
    let (columns, rows) = (surface.width.div_ceil(cell), surface.height.div_ceil(cell));
    if u64::from(columns) * u64::from(rows) > 4_194_304 {
        bail!("[limit-exceeded] the field has more than 4194304 cells; raise --cell")
    }
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(
        &mut hasher,
        format!("pentool-direction-field-v1\n{columns}x{rows}@{cell}\n").as_bytes(),
    );
    let (mut samples, mut defined) = (Vec::new(), 0u64);
    for cy in 0..rows {
        for cx in 0..columns {
            let x = f64::from(cx * cell) + f64::from(cell) / 2.0;
            let y = f64::from(cy * cell) + f64::from(cell) / 2.0;
            let angle = follow_angle_milli(&surface, follow, x, y);
            sha2::Digest::update(&mut hasher, angle.unwrap_or(i64::MIN).to_le_bytes());
            if let Some(a) = angle {
                defined += 1;
                if samples.len() < max_vectors {
                    samples.push(json!([cx * cell, cy * cell, a]));
                }
            }
        }
    }
    Ok(json!({
        "algorithm": ALGORITHM,
        "cell": cell,
        "columns": columns,
        "rows": rows,
        "defined_cells": defined,
        "returned": samples.len(),
        "has_more": defined > samples.len() as u64,
        "field_hash": format!("sha256:{}", hex::encode(sha2::Digest::finalize(hasher))),
        "samples": samples,
        "unit": "milli-degrees",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cordic_atan2_is_accurate_and_wraps() {
        for (y, x, deg) in [
            (0, 1, 0.0),
            (1, 1, 45.0),
            (1, 0, 90.0),
            (1, -1, 135.0),
            (0, -1, 180.0),
            (-1, -1, -135.0),
            (-1, 0, -90.0),
            (-1, 1, -45.0),
            (1, 2, 26.565_051),
        ] {
            let got = atan2_q16(y, x) as f64 / 65536.0;
            assert!(
                (got - deg + 180.0).rem_euclid(360.0) - 180.0 < 0.01
                    && (got - deg + 180.0).rem_euclid(360.0) - 180.0 > -0.01,
                "atan2({y},{x}) = {got}, want {deg}"
            );
        }
    }

    #[test]
    fn spec_parsing_is_strict() {
        assert!(Spec::parse(&json!({"inside": ["active"], "bogus": 1})).is_err());
        assert!(Spec::parse(&json!({"barriers": [{"kind": "edge"}]})).is_err());
        assert!(Spec::parse(&json!({"inside": ["nope"]})).is_err());
        let spec = Spec::parse(&json!({
            "inside": ["saved:face"],
            "opaque_clip": true,
            "barriers": [{"kind": "luma", "max": 40}],
            "follow": "contour"
        }))
        .unwrap();
        assert_eq!(spec.opaque_clip, Some(128));
        assert_eq!(spec.follow, Some(Follow::Contour));
    }
}

//! v0.13.0 task 3: color sampling, mixing and portable palettes.
//!
//! Pixels come from the authoritative renderer (`composite`). Every algorithm is
//! integer arithmetic with a documented tie-break so results are identical on
//! every platform. Named palettes are stored as readable document data under
//! `paint_palettes` and record scope, color space, algorithm version and the
//! source hash of the pixels they were extracted from.
use crate::raster::splitmix64;
use anyhow::{ensure, Context, Result};
use image::RgbaImage;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

pub const ALGORITHM: u32 = 1;
pub const MAX_COLORS: usize = 256;
pub const MAX_VARIATIONS: usize = 1024;
const MAX_NAME: usize = 100;
const MAX_PALETTES: usize = 256;
const MAX_PIXELS: u64 = 67_108_864;
const ROLES: [&str; 6] = [
    "dominant",
    "median",
    "highlight",
    "midtone",
    "shadow",
    "edge",
];

fn hex(c: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
}

pub fn parse_color(text: &str) -> Result<[u8; 4]> {
    let body = text.strip_prefix('#').with_context(|| {
        format!("[invalid-input] color {text:?} must look like #rrggbb or #rrggbbaa")
    })?;
    ensure!(
        (body.len() == 6 || body.len() == 8) && body.bytes().all(|b| b.is_ascii_hexdigit()),
        "[invalid-input] color {text:?} must look like #rrggbb or #rrggbbaa"
    );
    let byte = |i: usize| u8::from_str_radix(&body[i..i + 2], 16).unwrap();
    Ok([
        byte(0),
        byte(2),
        byte(4),
        if body.len() == 8 { byte(6) } else { 255 },
    ])
}

fn luma(p: [u8; 4]) -> u32 {
    (54 * u32::from(p[0]) + 183 * u32::from(p[1]) + 19 * u32::from(p[2]) + 128) >> 8
}

fn check_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= MAX_NAME
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')),
        "[invalid-input] palette name {name:?} must be 1-{MAX_NAME} letters, digits, space, '-', '_' or '.'"
    );
    Ok(())
}

/// Hash of the colors, in order, with the algorithm version.
pub fn palette_hash(colors: &[[u8; 4]]) -> String {
    let mut h = Sha256::new();
    h.update(b"pentool-palette-v1\n");
    for c in colors {
        h.update(c);
    }
    format!("sha256:{}", hex::encode(h.finalize()))
}

/// Pixels of one scope, optionally cropped and weighted by a selection query.
pub struct Source {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<[u8; 4]>,
    pub coverage: Vec<u8>,
    pub hash: String,
    pub origin: [u32; 2],
}

/// Render `page`, `node:ID` (any node), `group:ID` (a group node) or `layer:ID`
/// (a page layer container; a non-group node id is still accepted for compatibility).
pub(crate) fn render_scope(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    scope: &str,
) -> Result<RgbaImage> {
    if scope == "page" {
        return crate::composite::render(raw, document, page, 1.0);
    }
    if let Some(id) = scope.strip_prefix("node:") {
        return crate::composite::node_pixels(raw, document, page, id, 1.0);
    }
    if let Some(id) = scope.strip_prefix("group:") {
        let (node, _) = crate::composite::node_ref(raw, page, id)?;
        ensure!(
            node["kind"] == "group",
            "[invalid-input] scope group:{id} is not a group; use node:{id}"
        );
        return crate::composite::node_pixels(raw, document, page, id, 1.0);
    }
    let id = scope.strip_prefix("layer:").with_context(|| {
        format!("[invalid-input] scope {scope:?} must be page, node:ID, layer:ID or group:ID")
    })?;
    let page_value = match page {
        Some(p) => raw["pages"]
            .as_array()
            .and_then(|pages| pages.iter().find(|v| v["id"] == p)),
        None => raw["pages"].as_array().and_then(|pages| pages.first()),
    };
    let index = page_value
        .and_then(|p| p["layers"].as_array())
        .and_then(|layers| layers.iter().position(|l| l["id"] == id));
    if let Some(index) = index {
        let mut only = raw.clone();
        let target = match page {
            Some(p) => only["pages"]
                .as_array_mut()
                .and_then(|pages| pages.iter_mut().find(|v| v["id"] == p)),
            None => only["pages"]
                .as_array_mut()
                .and_then(|pages| pages.first_mut()),
        }
        .context("[not-found] page not found")?;
        let layer = target["layers"][index].clone();
        target["layers"] = serde_json::json!([layer]);
        return crate::composite::render(&only, document, page, 1.0);
    }
    let (node, _) = crate::composite::node_ref(raw, page, id)
        .with_context(|| format!("[not-found] layer:{id} is neither a layer nor a node"))?;
    ensure!(
        node["kind"] != "group",
        "[invalid-input] scope layer:{id} is a group; use group:{id}"
    );
    crate::composite::node_pixels(raw, document, page, id, 1.0)
}

pub fn source(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    scope: &str,
    rect: Option<[u32; 4]>,
    query: Option<&Value>,
) -> Result<Source> {
    let full = render_scope(raw, document, page, scope)?;
    let mask = match query {
        Some(q) => Some(crate::selection::coverage(raw, document, page, q, &full)?),
        None => None,
    };
    let (x0, y0, w, h) = match rect {
        Some([x, y, w, h]) => {
            ensure!(
                w > 0
                    && h > 0
                    && u64::from(x) + u64::from(w) <= u64::from(full.width())
                    && u64::from(y) + u64::from(h) <= u64::from(full.height()),
                "[invalid-input] rect {x},{y},{w},{h} must be non-empty and inside the {}x{} scope",
                full.width(),
                full.height()
            );
            (x, y, w, h)
        }
        None => (0, 0, full.width(), full.height()),
    };
    ensure!(
        u64::from(w) * u64::from(h) <= MAX_PIXELS,
        "[limit-exceeded] scope of {w}x{h} pixels exceeds {MAX_PIXELS}"
    );
    let mut rgba = Vec::with_capacity((w * h) as usize);
    let mut coverage = Vec::with_capacity((w * h) as usize);
    let mut hasher = Sha256::new();
    hasher.update(format!("pentool-source-v1\n{w}x{h}\n").as_bytes());
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let p = full.get_pixel(x, y).0;
            let c = mask
                .as_ref()
                .map_or(255, |m| m[(y * full.width() + x) as usize]);
            hasher.update(p);
            hasher.update([c]);
            rgba.push(p);
            coverage.push(c);
        }
    }
    Ok(Source {
        width: w,
        height: h,
        rgba,
        coverage,
        hash: format!("sha256:{}", hex::encode(hasher.finalize())),
        origin: [x0, y0],
    })
}

/// A sampled point, or the mean of a square neighborhood of radius `radius`.
pub fn sample(src: &Source, x: u32, y: u32, radius: u32) -> Result<Value> {
    ensure!(
        radius <= 64,
        "[limit-exceeded] sample radius is limited to 64 pixels"
    );
    ensure!(
        x < src.width && y < src.height,
        "[invalid-input] sample point {x},{y} lies outside the {}x{} scope",
        src.width,
        src.height
    );
    let (mut sum, mut weight) = ([0u64; 3], 0u64);
    let mut alpha = 0u64;
    let mut n = 0u64;
    let x0 = x.saturating_sub(radius);
    let y0 = y.saturating_sub(radius);
    let x1 = (x + radius).min(src.width - 1);
    let y1 = (y + radius).min(src.height - 1);
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            let i = (yy * src.width + xx) as usize;
            let p = src.rgba[i];
            n += 1;
            alpha += u64::from(p[3]);
            let w = (u64::from(src.coverage[i]) * u64::from(p[3]) + 127) / 255;
            weight += w;
            for c in 0..3 {
                sum[c] += u64::from(p[c]) * w;
            }
        }
    }
    let color = if weight == 0 {
        [0, 0, 0, 0]
    } else {
        let div = |s: u64| ((s + weight / 2) / weight) as u8;
        [
            div(sum[0]),
            div(sum[1]),
            div(sum[2]),
            ((alpha + n / 2) / n) as u8,
        ]
    };
    Ok(json!({
        "algorithm": ALGORITHM,
        "color_space": "srgb8",
        "x": x + src.origin[0],
        "y": y + src.origin[1],
        "radius": radius,
        "pixels": n,
        "color": hex(color),
        "rgba": color,
        "empty": weight == 0,
        "source_hash": src.hash,
    }))
}

#[derive(Clone, Copy)]
struct Px {
    c: [u8; 3],
    a: u8,
    w: u64,
}

fn edge_mask(src: &Source) -> Vec<bool> {
    let (w, h) = (src.width as i64, src.height as i64);
    let at = |x: i64, y: i64| {
        let (x, y) = (x.clamp(0, w - 1), y.clamp(0, h - 1));
        i64::from(luma(src.rgba[(y * w + x) as usize]))
    };
    let mut out = vec![false; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let g = (at(x + 1, y) - at(x - 1, y)).abs() + (at(x, y + 1) - at(x, y - 1)).abs();
            out[(y * w + x) as usize] = g >= 96;
        }
    }
    out
}

fn pixels_for_role(src: &Source, role: &str) -> Vec<Px> {
    let edges = (role == "edge").then(|| edge_mask(src));
    let mut out = Vec::new();
    for (i, p) in src.rgba.iter().enumerate() {
        let w = (u64::from(src.coverage[i]) * u64::from(p[3]) + 127) / 255;
        if w == 0 {
            continue;
        }
        let l = luma(*p);
        let keep = match role {
            "shadow" => l <= 85,
            "midtone" => (86..=170).contains(&l),
            "highlight" => l >= 171,
            "edge" => edges.as_ref().is_some_and(|e| e[i]),
            _ => true,
        };
        if keep {
            out.push(Px {
                c: [p[0], p[1], p[2]],
                a: p[3],
                w,
            });
        }
    }
    out
}

fn mean(group: &[Px]) -> [u8; 4] {
    let weight: u64 = group.iter().map(|p| p.w).sum();
    if weight == 0 {
        return [0, 0, 0, 0];
    }
    let mut s = [0u64; 3];
    let mut a = 0u64;
    for p in group {
        for (acc, v) in s.iter_mut().zip(p.c) {
            *acc += u64::from(v) * p.w;
        }
        a += u64::from(p.a) * p.w;
    }
    let d = |v: u64| ((v + weight / 2) / weight) as u8;
    [d(s[0]), d(s[1]), d(s[2]), d(a)]
}

/// Most frequent 5-bit-per-channel bins, each reported as its pixels' mean.
fn dominant(px: &[Px], count: usize) -> Vec<([u8; 4], u64)> {
    let mut bins: BTreeMap<u16, Vec<Px>> = BTreeMap::new();
    for p in px {
        let key =
            (u16::from(p.c[0] >> 3) << 10) | (u16::from(p.c[1] >> 3) << 5) | u16::from(p.c[2] >> 3);
        bins.entry(key).or_default().push(*p);
    }
    let mut ranked: Vec<(u16, u64)> = bins
        .iter()
        .map(|(k, v)| (*k, v.iter().map(|p| p.w).sum()))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(count)
        .map(|(k, w)| (mean(&bins[&k]), w))
        .collect()
}

/// Weighted median cut: repeatedly split the heaviest box on its widest channel.
fn median_cut(px: Vec<Px>, count: usize) -> Vec<([u8; 4], u64)> {
    if px.is_empty() {
        return Vec::new();
    }
    let mut boxes = vec![px];
    while boxes.len() < count {
        // Heaviest splittable box; ties by position.
        let Some((index, _)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .max_by(|(ia, a), (ib, b)| {
                let wa: u64 = a.iter().map(|p| p.w).sum();
                let wb: u64 = b.iter().map(|p| p.w).sum();
                wa.cmp(&wb).then(ib.cmp(ia))
            })
        else {
            break;
        };
        let mut b = boxes.swap_remove(index);
        let range = |ch: usize| {
            let lo = b.iter().map(|p| p.c[ch]).min().unwrap();
            let hi = b.iter().map(|p| p.c[ch]).max().unwrap();
            hi - lo
        };
        let channel = (0..3)
            .max_by(|a, c| range(*a).cmp(&range(*c)).then(c.cmp(a)))
            .unwrap();
        if range(channel) == 0 {
            boxes.insert(index.min(boxes.len()), b);
            // Uniform box: cannot be split further, so stop.
            break;
        }
        b.sort_by_key(|p| (p.c[channel], p.c));
        let total: u64 = b.iter().map(|p| p.w).sum();
        let (mut run, mut at) = (0u64, 0usize);
        for (i, p) in b.iter().enumerate() {
            run += p.w;
            at = i + 1;
            if run * 2 >= total {
                break;
            }
        }
        let at = at.clamp(1, b.len() - 1);
        let tail = b.split_off(at);
        boxes.push(b);
        boxes.push(tail);
    }
    let mut out: Vec<([u8; 4], u64)> = boxes
        .iter()
        .map(|b| (mean(b), b.iter().map(|p| p.w).sum()))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Extract an ordered palette for `role`.
pub fn extract(src: &Source, scope: &str, role: &str, count: usize) -> Result<Value> {
    ensure!(
        ROLES.contains(&role),
        "[invalid-input] role {role:?} must be one of {}",
        ROLES.join(", ")
    );
    ensure!(
        (1..=MAX_COLORS).contains(&count),
        "[invalid-input] count must be between 1 and {MAX_COLORS}"
    );
    let px = pixels_for_role(src, role);
    let scope_total: u64 = px.iter().map(|p| p.w).sum();
    let colors: Vec<([u8; 4], u64)> = if role == "median" {
        median_cut(px, count)
    } else {
        dominant(&px, count)
    };
    let list: Vec<[u8; 4]> = colors.iter().map(|c| c.0).collect();
    let total = scope_total;
    Ok(json!({
        "algorithm": ALGORITHM,
        "role": role,
        "scope": scope,
        "color_space": "srgb8",
        "width": src.width,
        "height": src.height,
        "origin": src.origin,
        "source_hash": src.hash,
        "palette_hash": palette_hash(&list),
        "empty": list.is_empty(),
        "colors": colors.iter().map(|(c, w)| json!({"color": hex(*c), "weight": w, "share": if total == 0 { 0.0 } else { ((*w as f64 / total as f64) * 1e6).round() / 1e6 }})).collect::<Vec<_>>(),
    }))
}

/// Linear per-channel mix in sRGB8. `t` is quantized to thousandths.
pub fn mix(a: [u8; 4], b: [u8; 4], t: f64) -> Result<[u8; 4]> {
    ensure!(
        t.is_finite() && (0.0..=1.0).contains(&t),
        "[invalid-input] mix amount must be between 0 and 1"
    );
    let t = (t * 1000.0).round() as u32;
    let mut out = [0u8; 4];
    for i in 0..4 {
        out[i] = ((u32::from(a[i]) * (1000 - t) + u32::from(b[i]) * t + 500) / 1000) as u8;
    }
    Ok(out)
}

/// Seeded variation: for each color, r, g, b (and optionally alpha) each take one
/// SplitMix64 draw mapped to `[-amount, amount]`.
pub fn vary(base: [u8; 4], seed: u64, amount: u8, count: usize) -> Result<Vec<[u8; 4]>> {
    ensure!(
        (1..=MAX_VARIATIONS).contains(&count),
        "[invalid-input] count must be between 1 and {MAX_VARIATIONS}"
    );
    let mut state = seed;
    let span = i64::from(amount);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut c = base;
        for ch in c.iter_mut().take(3) {
            let draw = (splitmix64(&mut state) % (2 * span as u64 + 1)) as i64 - span;
            *ch = (i64::from(*ch) + draw).clamp(0, 255) as u8;
        }
        out.push(c);
    }
    Ok(out)
}

// -------------------------------------------------------------- stored data

fn store(raw: &Value) -> Option<&Map<String, Value>> {
    raw.get("paint_palettes").and_then(Value::as_object)
}

/// Validate stored palettes: bounded, well-formed and hash-consistent.
pub fn validate(raw: &Value) -> Result<()> {
    let Some(value) = raw.get("paint_palettes") else {
        return Ok(());
    };
    let map = value
        .as_object()
        .context("[invalid-input] paint_palettes must be an object")?;
    ensure!(
        map.len() <= MAX_PALETTES,
        "[limit-exceeded] at most {MAX_PALETTES} palettes"
    );
    for (name, p) in map {
        check_name(name)?;
        let colors = colors_of(p).with_context(|| format!("palette `{name}`"))?;
        if let Some(h) = p.get("palette_hash").and_then(Value::as_str) {
            ensure!(
                h == palette_hash(&colors),
                "[conflict] palette `{name}` palette_hash does not match its colors"
            );
        }
    }
    Ok(())
}

fn colors_of(p: &Value) -> Result<Vec<[u8; 4]>> {
    let list = p
        .get("colors")
        .and_then(Value::as_array)
        .context("[invalid-input] palette needs a colors array")?;
    ensure!(
        !list.is_empty() && list.len() <= MAX_COLORS,
        "[limit-exceeded] a palette holds 1-{MAX_COLORS} colors"
    );
    list.iter()
        .map(|c| {
            let text = c
                .as_str()
                .or_else(|| c.get("color").and_then(Value::as_str))
                .context("[invalid-input] each palette color must be a hex string")?;
            parse_color(text)
        })
        .collect()
}

pub fn list(raw: &Value) -> Value {
    let rows: Vec<Value> = store(raw)
        .into_iter()
        .flatten()
        .map(|(name, p)| {
            json!({
                "name": name,
                "colors": p["colors"].as_array().map_or(0, Vec::len),
                "role": p.get("role").cloned().unwrap_or(Value::Null),
                "scope": p.get("scope").cloned().unwrap_or(Value::Null),
                "palette_hash": p.get("palette_hash").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    json!({"matches": rows.len(), "palettes": rows})
}

pub fn show(raw: &Value, name: &str) -> Result<Value> {
    store(raw)
        .and_then(|m| m.get(name))
        .cloned()
        .with_context(|| format!("[not-found] palette `{name}` does not exist"))
}

/// Store `palette` (an `extract` result or a manual one) under `name`.
pub fn save(raw: &mut Value, name: &str, palette: Value, replace: bool) -> Result<Value> {
    check_name(name)?;
    let colors = colors_of(&palette)?;
    let mut record = palette;
    record["palette_hash"] = json!(palette_hash(&colors));
    record["algorithm"] = json!(ALGORITHM);
    record["color_space"] = json!("srgb8");
    let map = raw
        .as_object_mut()
        .context("[invalid-input] document must be an object")?
        .entry("paint_palettes")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("[invalid-input] paint_palettes must be an object")?;
    ensure!(
        replace || !map.contains_key(name),
        "[conflict] palette `{name}` already exists; pass --replace to overwrite it"
    );
    ensure!(
        map.len() < MAX_PALETTES || map.contains_key(name),
        "[limit-exceeded] at most {MAX_PALETTES} palettes"
    );
    let hash = record["palette_hash"].clone();
    map.insert(name.to_string(), record);
    Ok(json!({"saved": name, "colors": colors.len(), "palette_hash": hash}))
}

pub fn remove(raw: &mut Value, name: &str) -> Result<Value> {
    let map = raw
        .get_mut("paint_palettes")
        .and_then(Value::as_object_mut)
        .with_context(|| format!("[not-found] palette `{name}` does not exist"))?;
    ensure!(
        map.remove(name).is_some(),
        "[not-found] palette `{name}` does not exist"
    );
    if map.is_empty() {
        raw.as_object_mut().unwrap().remove("paint_palettes");
    }
    Ok(json!({"removed": name}))
}

/// A manual palette from hex strings.
pub fn manual(colors: &[String]) -> Result<Value> {
    ensure!(
        !colors.is_empty() && colors.len() <= MAX_COLORS,
        "[invalid-input] give 1-{MAX_COLORS} colors"
    );
    let parsed = colors
        .iter()
        .map(|c| parse_color(c))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"role": "manual", "colors": parsed.iter().map(|c| hex(*c)).collect::<Vec<_>>()}))
}

// --------------------------------------------------------------------- CLI

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum PaletteAction {
    /// Sample a point (or a small neighborhood mean) through the renderer.
    Sample {
        #[arg(long, default_value = "page")]
        scope: String,
        #[arg(long)]
        x: u32,
        #[arg(long)]
        y: u32,
        /// Neighborhood radius in pixels (0 = the pixel itself).
        #[arg(long, default_value_t = 0)]
        radius: u32,
        /// Selection query JSON restricting the scope.
        #[arg(long)]
        query: Option<String>,
    },
    /// Extract an ordered palette for a role: dominant, median, highlight, midtone, shadow or edge.
    Extract {
        #[arg(long, default_value = "page")]
        scope: String,
        /// Crop as x,y,width,height in scope pixels.
        #[arg(long)]
        rect: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, default_value = "dominant")]
        role: String,
        #[arg(long, default_value_t = 5)]
        count: usize,
        /// Store the result as a named palette in the document.
        #[arg(long)]
        save: Option<String>,
        #[arg(long)]
        replace: bool,
    },
    /// Mix two colors linearly per channel (amount in 0..1, quantized to thousandths).
    Mix {
        a: String,
        b: String,
        #[arg(long, default_value_t = 0.5)]
        amount: f64,
    },
    /// Generate seeded variations of a color.
    Vary {
        color: String,
        #[arg(long)]
        seed: u64,
        /// Maximum per-channel deviation (0..255).
        #[arg(long, default_value_t = 8)]
        amount: u8,
        #[arg(long, default_value_t = 8)]
        count: usize,
    },
    /// List stored palettes.
    List,
    /// Show one stored palette.
    Show { name: String },
    /// Store a manual palette of hex colors.
    Add {
        name: String,
        colors: Vec<String>,
        #[arg(long)]
        replace: bool,
    },
    /// Remove a stored palette.
    Remove { name: String },
}

fn json_arg(text: &str, what: &str) -> Result<Value> {
    ensure!(
        text.len() <= 1 << 20,
        "[limit-exceeded] {what} is larger than 1 MiB"
    );
    serde_json::from_str(text).with_context(|| format!("[invalid-input] {what} must be JSON"))
}

pub fn run(
    input: &PathBuf,
    page: Option<&str>,
    dry_run: bool,
    revision: Option<&str>,
    action: PaletteAction,
) -> Result<()> {
    let mut raw: Value = serde_json::from_slice(
        &std::fs::read(input)
            .with_context(|| format!("[missing-resource] cannot read {}", input.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    validate(&raw)?;
    let reading = |query: &Option<String>| -> Result<Option<Value>> {
        query.as_deref().map(|q| json_arg(q, "--query")).transpose()
    };
    let (result, mutated) = match action {
        PaletteAction::Sample {
            scope,
            x,
            y,
            radius,
            query,
        } => {
            let q = reading(&query)?;
            let src = source(&raw, input, page, &scope, None, q.as_ref())?;
            let mut out = sample(&src, x, y, radius)?;
            out["scope"] = json!(scope);
            (out, false)
        }
        PaletteAction::Extract {
            scope,
            rect,
            query,
            role,
            count,
            save,
            replace,
        } => {
            let q = reading(&query)?;
            let rect = rect
                .as_deref()
                .map(|r| -> Result<[u32; 4]> {
                    let parts: Vec<u32> = r
                        .split(',')
                        .map(|p| p.trim().parse::<u32>())
                        .collect::<Result<_, _>>()
                        .context("[invalid-input] --rect must be x,y,width,height")?;
                    <[u32; 4]>::try_from(parts).map_err(|_| {
                        anyhow::anyhow!("[invalid-input] --rect must be x,y,width,height")
                    })
                })
                .transpose()?;
            let src = source(&raw, input, page, &scope, rect, q.as_ref())?;
            let mut out = extract(&src, &scope, &role, count)?;
            if let Some(name) = save {
                ensure!(
                    !out["empty"].as_bool().unwrap_or(true),
                    "[invalid-input] the scope has no visible pixels, so there is no palette to save"
                );
                let stored = save_palette(&mut raw, &name, out.clone(), replace)?;
                out["stored"] = stored;
                (out, true)
            } else {
                (out, false)
            }
        }
        PaletteAction::Mix { a, b, amount } => {
            let (ca, cb) = (parse_color(&a)?, parse_color(&b)?);
            let m = mix(ca, cb, amount)?;
            (
                json!({"algorithm": ALGORITHM, "color_space": "srgb8", "a": hex(ca), "b": hex(cb), "amount": amount, "color": hex(m)}),
                false,
            )
        }
        PaletteAction::Vary {
            color,
            seed,
            amount,
            count,
        } => {
            let base = parse_color(&color)?;
            let list = vary(base, seed, amount, count)?;
            (
                json!({"algorithm": ALGORITHM, "color_space": "srgb8", "base": hex(base), "seed": seed, "amount": amount,
                    "colors": list.iter().map(|c| hex(*c)).collect::<Vec<_>>()}),
                false,
            )
        }
        PaletteAction::List => (list(&raw), false),
        PaletteAction::Show { name } => (show(&raw, &name)?, false),
        PaletteAction::Add {
            name,
            colors,
            replace,
        } => {
            let p = manual(&colors)?;
            (save_palette(&mut raw, &name, p, replace)?, true)
        }
        PaletteAction::Remove { name } => (remove(&mut raw, &name)?, true),
    };
    if !mutated {
        ensure!(
            !dry_run && revision.is_none(),
            "[invalid-input] this palette action is read-only; --dry-run and --if-revision apply to saving or removing palettes"
        );
        println!("{result}");
        return Ok(());
    }
    validate(&raw)?;
    let change = crate::transaction::commit_value(input, "paint-palette", dry_run, revision, &raw)?;
    println!("{}", json!({"result": result, "change": change}));
    Ok(())
}

fn save_palette(raw: &mut Value, name: &str, palette: Value, replace: bool) -> Result<Value> {
    save(raw, name, palette, replace)
}

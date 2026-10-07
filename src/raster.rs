//! Raster-paint layers (v0.10.0): sparse, content-addressed tiles plus a deterministic
//! brush engine. See `docs/raster-paint-v1.md` for the normative contract.
//!
//! Determinism rules: only IEEE `+ - * / sqrt`, `floor`, `round`, comparisons and
//! integer math are used on the pixel path. No `sin`, `cos`, `exp`, `pow` or other
//! libm functions, so tiles are bit-identical on every supported platform.
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Cursor;

pub const ENGINE: u64 = 1;
pub const TILE: usize = 256;
pub const TILE_BYTES: usize = TILE * TILE * 4;
pub const MAX_DIMENSION: u64 = 16_384;
pub const MAX_TILES: usize = 4_096;
pub const MAX_JOURNAL: usize = 256;
pub const MAX_SAMPLES: usize = 8_192;
pub const MAX_DABS: usize = 100_000;
pub const MAX_STROKE_WORK: u64 = 256 * 1024 * 1024;
pub const MAX_BRUSH_SIZE: f64 = 2_048.0;
const FORMAT: &str = "rgba8-straight";

fn bad(code: &str, message: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("[{code}] {message}")
}

// ---------------------------------------------------------------------------
// Deterministic math
// ---------------------------------------------------------------------------

/// Sine and cosine of an angle in degrees using a fixed-length Taylor series, so the
/// result never depends on a platform's libm.
fn sin_cos_degrees(degrees: f64) -> (f64, f64) {
    let mut d = degrees % 360.0;
    if d > 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    let x = d * (std::f64::consts::PI / 180.0);
    let x2 = x * x;
    let (mut sin, mut cos) = (x, 1.0);
    let (mut sin_term, mut cos_term) = (x, 1.0);
    for k in 1..=14 {
        let n = f64::from(2 * k);
        cos_term *= -x2 / ((n - 1.0) * n);
        cos += cos_term;
        sin_term *= -x2 / (n * (n + 1.0));
        sin += sin_term;
    }
    (sin, cos)
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform in [-1, 1) from a 53-bit mantissa; exact in f64.
fn signed_unit(state: &mut u64) -> f64 {
    let bits = splitmix64(state) >> 11;
    (bits as f64) / ((1u64 << 52) as f64) - 1.0
}

// ---------------------------------------------------------------------------
// Tiles and surfaces
// ---------------------------------------------------------------------------

pub type Tile = Box<[u8]>;

/// An in-memory sparse surface: only painted tiles exist.
#[derive(Default, Clone)]
pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub tiles: BTreeMap<(u32, u32), Tile>,
}

pub fn tile_hash(tile: &[u8]) -> String {
    crate::resource::sha256(tile)
}

fn is_blank(tile: &[u8]) -> bool {
    tile.iter().all(|byte| *byte == 0)
}

fn key_string(key: (u32, u32)) -> String {
    format!("{},{}", key.0, key.1)
}

fn parse_key(text: &str) -> Result<(u32, u32)> {
    let (x, y) = text
        .split_once(',')
        .with_context(|| format!("[malformed-raster] tile key {text:?} must be \"x,y\""))?;
    let parse = |part: &str| -> Result<u32> {
        if part.is_empty() || part.len() > 5 || !part.bytes().all(|b| b.is_ascii_digit()) {
            bail!("[malformed-raster] tile key {text:?} must use plain decimal indexes")
        }
        part.parse::<u32>().map_err(Into::into)
    };
    let key = (parse(x)?, parse(y)?);
    if key_string(key) != text {
        bail!("[malformed-raster] tile key {text:?} is not canonical")
    }
    Ok(key)
}

fn tiles_across(extent: u32) -> u32 {
    extent.div_ceil(TILE as u32)
}

fn encode_tile(tile: &[u8]) -> Result<String> {
    let image = image::RgbaImage::from_raw(TILE as u32, TILE as u32, tile.to_vec())
        .context("tile has the wrong size")?;
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .context("could not encode tile")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

fn decode_tile(entry: &Value, digest: &str) -> Result<Tile> {
    if entry["encoding"] != "png-base64" {
        bail!("[malformed-raster] tile {digest} has an unsupported encoding")
    }
    let data = entry["data"]
        .as_str()
        .with_context(|| format!("[malformed-raster] tile {digest} has no data"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| bad("malformed-raster", format!("tile {digest} is not base64")))?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|error| bad("malformed-raster", format!("tile {digest}: {error}")))?
        .to_rgba8();
    if image.width() != TILE as u32 || image.height() != TILE as u32 {
        bail!("[malformed-raster] tile {digest} is not {TILE}x{TILE}")
    }
    let pixels = image.into_raw();
    if tile_hash(&pixels) != digest {
        bail!("[corrupt-raster] tile {digest} does not match its content hash; restore the document from history")
    }
    Ok(pixels.into_boxed_slice())
}

impl Surface {
    pub fn load(raw: &Value, node: &Value) -> Result<Self> {
        let (width, height) = dimensions(node)?;
        let mut surface = Surface {
            width,
            height,
            tiles: BTreeMap::new(),
        };
        let store = raw.get("raster_tiles").and_then(Value::as_object);
        for (key, digest) in node["tiles"].as_object().into_iter().flatten() {
            let digest = digest.as_str().context("tile digest must be a string")?;
            let entry = store.and_then(|s| s.get(digest)).with_context(|| {
                format!("[missing-resource] raster tile {digest} is not in raster_tiles")
            })?;
            surface
                .tiles
                .insert(parse_key(key)?, decode_tile(entry, digest)?);
        }
        Ok(surface)
    }

    /// Write the tile map back, adding new tiles to the store and dropping blanks.
    pub fn store(&self, raw: &mut Value, node: &mut Value) -> Result<()> {
        let mut map = Map::new();
        let store = raw
            .as_object_mut()
            .context("document must be an object")?
            .entry("raster_tiles")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("raster_tiles must be an object")?;
        for (key, tile) in &self.tiles {
            if is_blank(tile) {
                continue;
            }
            let digest = tile_hash(tile);
            if !store.contains_key(&digest) {
                store.insert(
                    digest.clone(),
                    json!({"encoding":"png-base64","data":encode_tile(tile)?}),
                );
            }
            map.insert(key_string(*key), Value::String(digest));
        }
        if map.len() > MAX_TILES {
            bail!("[limit-exceeded] a raster layer may hold at most {MAX_TILES} tiles")
        }
        node["tiles"] = Value::Object(map);
        Ok(())
    }

    pub fn tile_map_hash(&self) -> String {
        let mut text = String::new();
        for (key, tile) in &self.tiles {
            if !is_blank(tile) {
                text.push_str(&format!("{},{}={}\n", key.0, key.1, tile_hash(tile)));
            }
        }
        crate::resource::sha256(text.as_bytes())
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let key = (x / TILE as u32, y / TILE as u32);
        match self.tiles.get(&key) {
            Some(tile) => {
                let at = (((y as usize) % TILE) * TILE + (x as usize) % TILE) * 4;
                [tile[at], tile[at + 1], tile[at + 2], tile[at + 3]]
            }
            None => [0; 4],
        }
    }

    pub fn to_image(&self) -> Result<image::RgbaImage> {
        let mut out = image::RgbaImage::new(self.width, self.height);
        for (key, tile) in &self.tiles {
            for row in 0..TILE {
                let y = key.1 as usize * TILE + row;
                if y >= self.height as usize {
                    break;
                }
                for column in 0..TILE {
                    let x = key.0 as usize * TILE + column;
                    if x >= self.width as usize {
                        break;
                    }
                    let at = (row * TILE + column) * 4;
                    out.put_pixel(
                        x as u32,
                        y as u32,
                        image::Rgba([tile[at], tile[at + 1], tile[at + 2], tile[at + 3]]),
                    );
                }
            }
        }
        Ok(out)
    }
}

fn dimensions(node: &Value) -> Result<(u32, u32)> {
    let read = |key: &str| -> Result<u32> {
        let value = node.get(key).and_then(Value::as_u64).with_context(|| {
            format!("[malformed-raster] raster {key} must be a positive integer")
        })?;
        if value == 0 || value > MAX_DIMENSION {
            bail!("[limit-exceeded] raster {key} must be 1-{MAX_DIMENSION} pixels")
        }
        Ok(value as u32)
    };
    Ok((read("width")?, read("height")?))
}

// ---------------------------------------------------------------------------
// Brush
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Brush {
    pub kind: String,
    pub size: f64,
    pub hardness: f64,
    pub spacing: f64,
    pub opacity: f64,
    pub flow: f64,
    pub angle: f64,
    pub roundness: f64,
    pub scatter: f64,
    pub pressure_size: bool,
    pub pressure_flow: bool,
    pub min_size: f64,
}

pub const BRUSH_KINDS: [&str; 4] = ["hard-round", "soft-round", "pixel", "calligraphic"];

fn number_in(
    object: &Map<String, Value>,
    key: &str,
    default: f64,
    lo: f64,
    hi: f64,
) -> Result<f64> {
    match object.get(key) {
        None => Ok(default),
        Some(value) => {
            let n = value
                .as_f64()
                .filter(|n| n.is_finite())
                .with_context(|| format!("[invalid-brush] {key} must be a finite number"))?;
            if n < lo || n > hi {
                bail!("[invalid-brush] {key} must be between {lo} and {hi}")
            }
            Ok(n)
        }
    }
}

impl Brush {
    pub fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("[invalid-brush] brush must be an object")?;
        const KNOWN: [&str; 12] = [
            "kind",
            "size",
            "hardness",
            "spacing",
            "opacity",
            "flow",
            "angle",
            "roundness",
            "scatter",
            "pressure_size",
            "pressure_flow",
            "min_size",
        ];
        if let Some(key) = object.keys().find(|k| !KNOWN.contains(&k.as_str())) {
            bail!(
                "[invalid-brush] unknown brush property {key:?}; supported: {}",
                KNOWN.join(", ")
            )
        }
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("hard-round")
            .to_owned();
        if !BRUSH_KINDS.contains(&kind.as_str()) {
            bail!(
                "[invalid-brush] unsupported brush kind {kind:?}; use one of {}",
                BRUSH_KINDS.join(", ")
            )
        }
        let calligraphic = kind == "calligraphic";
        let flag = |key: &str| -> Result<bool> {
            match object.get(key) {
                None => Ok(false),
                Some(Value::Bool(b)) => Ok(*b),
                _ => bail!("[invalid-brush] {key} must be true or false"),
            }
        };
        let pixel = kind == "pixel";
        Ok(Brush {
            size: number_in(object, "size", 12.0, 1.0, MAX_BRUSH_SIZE)?,
            hardness: number_in(
                object,
                "hardness",
                if kind == "soft-round" { 0.5 } else { 1.0 },
                0.0,
                1.0,
            )?,
            spacing: number_in(object, "spacing", if pixel { 0.0 } else { 0.1 }, 0.0, 2.0)?,
            opacity: number_in(object, "opacity", 1.0, 0.0, 1.0)?,
            flow: number_in(object, "flow", 1.0, 0.0, 1.0)?,
            angle: number_in(
                object,
                "angle",
                if calligraphic { 45.0 } else { 0.0 },
                -360.0,
                360.0,
            )?,
            roundness: number_in(
                object,
                "roundness",
                if calligraphic { 0.25 } else { 1.0 },
                0.05,
                1.0,
            )?,
            scatter: number_in(object, "scatter", 0.0, 0.0, 5.0)?,
            pressure_size: flag("pressure_size")?,
            pressure_flow: flag("pressure_flow")?,
            min_size: number_in(object, "min_size", 0.1, 0.0, 1.0)?,
            kind,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind, "size": self.size, "hardness": self.hardness,
            "spacing": self.spacing, "opacity": self.opacity, "flow": self.flow,
            "angle": self.angle, "roundness": self.roundness, "scatter": self.scatter,
            "pressure_size": self.pressure_size, "pressure_flow": self.pressure_flow,
            "min_size": self.min_size,
        })
    }

    fn step(&self) -> f64 {
        if self.kind == "pixel" {
            1.0
        } else {
            (self.spacing * self.size).max(0.25)
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub x: f64,
    pub y: f64,
    pub pressure: f64,
}

/// Samples are rounded to 1/1000 pixel so a journal replays exactly.
fn canon(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub fn parse_samples(value: &Value) -> Result<Vec<Sample>> {
    let list = value
        .as_array()
        .context("[invalid-stroke] samples must be an array")?;
    if list.is_empty() || list.len() > MAX_SAMPLES {
        bail!(
            "[limit-exceeded] a stroke needs 1-{MAX_SAMPLES} samples, got {}",
            list.len()
        )
    }
    let mut out = Vec::with_capacity(list.len());
    for (index, item) in list.iter().enumerate() {
        let (x, y, pressure) = match item {
            Value::Array(values) if (2..=3).contains(&values.len()) => (
                values[0].as_f64(),
                values[1].as_f64(),
                values.get(2).map(Value::as_f64).unwrap_or(Some(1.0)),
            ),
            Value::Object(object) => (
                object.get("x").and_then(Value::as_f64),
                object.get("y").and_then(Value::as_f64),
                object
                    .get("pressure")
                    .map(Value::as_f64)
                    .unwrap_or(Some(1.0)),
            ),
            _ => (None, None, None),
        };
        let (Some(x), Some(y), Some(pressure)) = (x, y, pressure) else {
            bail!("[invalid-stroke] sample {index} must be [x, y], [x, y, pressure] or {{x, y, pressure}}")
        };
        if !x.is_finite() || !y.is_finite() || x.abs() > 1.0e6 || y.abs() > 1.0e6 {
            bail!("[invalid-stroke] sample {index} has a non-finite or absurd coordinate")
        }
        if !(0.0..=1.0).contains(&pressure) {
            bail!("[invalid-stroke] sample {index} pressure must be between 0 and 1")
        }
        out.push(Sample {
            x: canon(x),
            y: canon(y),
            pressure: canon(pressure),
        });
    }
    Ok(out)
}

pub fn parse_color(text: &str) -> Result<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("[invalid-stroke] color must be #RRGGBB, got {text:?}")
    }
    let n = u32::from_str_radix(hex, 16)?;
    Ok([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Erase,
}

impl Blend {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "normal" => Ok(Self::Normal),
            "erase" => Ok(Self::Erase),
            other => {
                bail!("[invalid-stroke] blend {other:?} is not supported; use normal or erase")
            }
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Erase => "erase",
        }
    }
}

struct Dab {
    x: f64,
    y: f64,
    size: f64,
    flow: f64,
}

fn dabs(brush: &Brush, samples: &[Sample], seed: u64) -> Result<Vec<Dab>> {
    let step = brush.step();
    let mut points: Vec<(f64, f64, f64)> = Vec::new();
    let mut total = 0.0_f64;
    for pair in samples.windows(2) {
        let (dx, dy) = (pair[1].x - pair[0].x, pair[1].y - pair[0].y);
        total += (dx * dx + dy * dy).sqrt();
    }
    if total / step + 1.0 > MAX_DABS as f64 {
        bail!("[limit-exceeded] this stroke needs more than {MAX_DABS} dabs; use a larger spacing or split the stroke")
    }
    points.push((samples[0].x, samples[0].y, samples[0].pressure));
    let mut traveled = 0.0_f64;
    let mut next = step;
    for pair in samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length = (dx * dx + dy * dy).sqrt();
        if length == 0.0 {
            continue;
        }
        while next <= traveled + length {
            let t = (next - traveled) / length;
            points.push((
                a.x + dx * t,
                a.y + dy * t,
                a.pressure + (b.pressure - a.pressure) * t,
            ));
            next += step;
        }
        traveled += length;
    }
    let mut state = seed;
    let mut out = Vec::with_capacity(points.len());
    for (x, y, pressure) in points {
        let mut size = brush.size;
        if brush.pressure_size {
            size *= brush.min_size + (1.0 - brush.min_size) * pressure;
        }
        let (mut px, mut py) = (x, y);
        if brush.scatter > 0.0 {
            let reach = brush.scatter * brush.size;
            px += signed_unit(&mut state) * reach;
            py += signed_unit(&mut state) * reach;
        }
        let flow = if brush.pressure_flow {
            brush.flow * pressure
        } else {
            brush.flow
        };
        out.push(Dab {
            x: px,
            y: py,
            size: size.max(1.0),
            flow,
        });
    }
    Ok(out)
}

type StrokeBuffer = HashMap<(u32, u32), Vec<u16>>;

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Accumulate one dab into the per-stroke coverage buffer (flow build-up).
fn stamp(buffer: &mut StrokeBuffer, brush: &Brush, dab: &Dab, width: u32, height: u32) {
    let radius = dab.size / 2.0;
    let reach = radius / brush.roundness.clamp(0.05, 1.0) + 1.0;
    let x0 = ((dab.x - reach).floor()).max(0.0) as i64;
    let y0 = ((dab.y - reach).floor()).max(0.0) as i64;
    let x1 = ((dab.x + reach).ceil()).min(f64::from(width)) as i64;
    let y1 = ((dab.y + reach).ceil()).min(f64::from(height)) as i64;
    let (sin, cos) = sin_cos_degrees(brush.angle);
    let flow16 = (dab.flow * 65535.0).round();
    if flow16 <= 0.0 || x1 <= x0 || y1 <= y0 {
        return;
    }
    let pixel_brush = brush.kind == "pixel";
    let size_px = dab.size.round().max(1.0) as i64;
    let (px0, py0) = (
        dab.x.floor() as i64 - size_px / 2,
        dab.y.floor() as i64 - size_px / 2,
    );
    let (range_x, range_y) = if pixel_brush {
        (
            px0.max(0)..(px0 + size_px).min(i64::from(width)),
            py0.max(0)..(py0 + size_px).min(i64::from(height)),
        )
    } else {
        (x0..x1, y0..y1)
    };
    for y in range_y {
        for x in range_x.clone() {
            let coverage = if pixel_brush {
                1.0
            } else {
                let (dx, dy) = (x as f64 + 0.5 - dab.x, y as f64 + 0.5 - dab.y);
                let u = dx * cos + dy * sin;
                let v = (-dx * sin + dy * cos) / brush.roundness;
                let d = (u * u + v * v).sqrt();
                if brush.kind == "soft-round" && brush.hardness < 1.0 {
                    let inner = radius * brush.hardness;
                    if d <= inner {
                        1.0
                    } else if d >= radius {
                        0.0
                    } else {
                        smoothstep((radius - d) / (radius - inner))
                    }
                } else {
                    (radius - d + 0.5).clamp(0.0, 1.0)
                }
            };
            if coverage <= 0.0 {
                continue;
            }
            let dab16 = (coverage * flow16).round() as u32;
            if dab16 == 0 {
                continue;
            }
            let key = ((x as u32) / TILE as u32, (y as u32) / TILE as u32);
            let cell = buffer.entry(key).or_insert_with(|| vec![0u16; TILE * TILE]);
            let at = ((y as usize) % TILE) * TILE + (x as usize) % TILE;
            let current = u32::from(cell[at]);
            let added = (dab16 * (65535 - current) + 32767) / 65535;
            cell[at] = (current + added).min(65535) as u16;
        }
    }
}

fn composite(tile: &mut [u8], at: usize, color: [u8; 3], alpha16: u32, blend: Blend) {
    let da16 = u32::from(tile[at + 3]) * 257;
    match blend {
        Blend::Normal => {
            let t = ((u64::from(da16) * u64::from(65535 - alpha16) + 32767) / 65535) as u32;
            let oa16 = alpha16 + t;
            if oa16 == 0 {
                tile[at..at + 4].fill(0);
                return;
            }
            for channel in 0..3 {
                let src = u64::from(color[channel]) * 257;
                let dst = u64::from(tile[at + channel]) * 257;
                let c16 = (src * u64::from(alpha16) + dst * u64::from(t) + u64::from(oa16) / 2)
                    / u64::from(oa16);
                tile[at + channel] = ((c16 + 128) / 257).min(255) as u8;
            }
            tile[at + 3] = ((oa16 + 128) / 257).min(255) as u8;
        }
        Blend::Erase => {
            let oa16 = ((u64::from(da16) * u64::from(65535 - alpha16) + 32767) / 65535) as u32;
            tile[at + 3] = ((oa16 + 128) / 257).min(255) as u8;
        }
    }
    if tile[at + 3] == 0 {
        tile[at..at + 4].fill(0);
    }
}

pub struct Stroke {
    pub brush: Brush,
    pub samples: Vec<Sample>,
    pub color: [u8; 3],
    pub blend: Blend,
    pub seed: u64,
}

pub struct StrokeResult {
    pub bounds: Option<[u32; 4]>,
    pub tiles_changed: usize,
    pub dabs: usize,
}

/// Apply a stroke to a surface. Fails before any pixel work if limits are exceeded.
pub fn apply_stroke(surface: &mut Surface, stroke: &Stroke) -> Result<StrokeResult> {
    let dab_list = dabs(&stroke.brush, &stroke.samples, stroke.seed)?;
    let footprint =
        (stroke.brush.size / stroke.brush.roundness.max(0.05) + 3.0).min(MAX_BRUSH_SIZE * 20.0);
    let work = (dab_list.len() as f64 * footprint * footprint) as u64;
    if work > MAX_STROKE_WORK {
        bail!("[limit-exceeded] stroke work {work} exceeds {MAX_STROKE_WORK}; reduce size or sample length")
    }
    let mut buffer = StrokeBuffer::new();
    for dab in &dab_list {
        stamp(
            &mut buffer,
            &stroke.brush,
            dab,
            surface.width,
            surface.height,
        );
    }
    let opacity16 = (stroke.brush.opacity * 65535.0).round() as u32;
    let mut touched = 0usize;
    let mut bounds: Option<[u32; 4]> = None;
    let mut keys: Vec<_> = buffer.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        let cell = &buffer[&key];
        let original = surface.tiles.get(&key).cloned();
        let mut tile = original
            .clone()
            .unwrap_or_else(|| vec![0u8; TILE_BYTES].into_boxed_slice());
        for (index, value) in cell.iter().enumerate() {
            if *value == 0 {
                continue;
            }
            let alpha16 = (u32::from(*value) * opacity16 + 32767) / 65535;
            if alpha16 == 0 {
                continue;
            }
            composite(&mut tile, index * 4, stroke.color, alpha16, stroke.blend);
            let x = key.0 * TILE as u32 + (index % TILE) as u32;
            let y = key.1 * TILE as u32 + (index / TILE) as u32;
            let b = bounds.get_or_insert([x, y, x + 1, y + 1]);
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x + 1);
            b[3] = b[3].max(y + 1);
        }
        let changed =
            original.as_deref() != Some(&tile[..]) && !(original.is_none() && is_blank(&tile));
        if changed {
            touched += 1;
        }
        if is_blank(&tile) {
            surface.tiles.remove(&key);
        } else {
            surface.tiles.insert(key, tile);
        }
    }
    Ok(StrokeResult {
        bounds,
        tiles_changed: touched,
        dabs: dab_list.len(),
    })
}

// ---------------------------------------------------------------------------
// Document integration
// ---------------------------------------------------------------------------

pub fn is_raster(node: &Value) -> bool {
    node.get("kind").and_then(Value::as_str) == Some("raster")
}

pub fn validate_node(node: &Value) -> Result<()> {
    let object = node.as_object().context("node must be an object")?;
    let id = object.get("id").and_then(Value::as_str).unwrap_or("raster");
    for key in ["x", "y"] {
        object
            .get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .with_context(|| format!("[malformed-raster] raster {id}: {key} must be a number"))?;
    }
    let (width, height) = dimensions(node)?;
    if object.get("tile_size").and_then(Value::as_u64) != Some(TILE as u64) {
        bail!("[unsupported-capability] raster {id}: tile_size must be {TILE}")
    }
    if object.get("pixel_format").and_then(Value::as_str) != Some(FORMAT) {
        bail!("[unsupported-capability] raster {id}: pixel_format must be {FORMAT}")
    }
    let tiles = object
        .get("tiles")
        .and_then(Value::as_object)
        .with_context(|| format!("[malformed-raster] raster {id}: tiles must be an object"))?;
    if tiles.len() > MAX_TILES {
        bail!("[limit-exceeded] raster {id} has more than {MAX_TILES} tiles")
    }
    let (across, down) = (tiles_across(width), tiles_across(height));
    for (key, digest) in tiles {
        let (tx, ty) = parse_key(key)?;
        if tx >= across || ty >= down {
            bail!("[malformed-raster] raster {id}: tile {key} lies outside the {width}x{height} bounds")
        }
        let digest = digest.as_str().unwrap_or_default();
        if digest.len() != 71 || !digest.starts_with("sha256:") {
            bail!("[malformed-raster] raster {id}: tile {key} digest must be sha256:<64 hex>")
        }
    }
    if let Some(journal) = object.get("journal") {
        let journal = journal
            .as_array()
            .with_context(|| format!("[malformed-raster] raster {id}: journal must be an array"))?;
        if journal.len() > MAX_JOURNAL {
            bail!("[limit-exceeded] raster {id} journal exceeds {MAX_JOURNAL} entries; run `raster checkpoint --compact`")
        }
    }
    Ok(())
}

fn walk_rasters<'a>(nodes: &'a [Value], out: &mut Vec<&'a Value>) {
    for node in nodes {
        if is_raster(node) {
            out.push(node);
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            walk_rasters(children, out);
        }
    }
}

fn all_rasters(raw: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    for page in raw["pages"].as_array().into_iter().flatten() {
        for layer in page["layers"].as_array().into_iter().flatten() {
            if let Some(nodes) = layer["nodes"].as_array() {
                walk_rasters(nodes, &mut out);
            }
        }
    }
    out
}

/// Document-level check: every referenced tile exists with a well-formed entry.
pub fn validate_document(raw: &Value) -> Result<()> {
    let rasters = all_rasters(raw);
    let store = raw.get("raster_tiles");
    if let Some(store) = store {
        if !store.is_object() {
            bail!("[malformed-raster] raster_tiles must be an object")
        }
    }
    for node in rasters {
        for (key, digest) in node["tiles"].as_object().into_iter().flatten() {
            let digest = digest.as_str().unwrap_or_default();
            if store.and_then(|s| s.get(digest)).is_none() {
                bail!(
                    "[missing-resource] raster {} tile {key} references {digest}, which is not in raster_tiles",
                    node["id"].as_str().unwrap_or("?")
                )
            }
        }
    }
    Ok(())
}

/// Remove stored tiles no raster node references.
pub fn collect_garbage(raw: &mut Value) -> usize {
    let referenced: HashSet<String> = all_rasters(raw)
        .iter()
        .flat_map(|node| {
            node["tiles"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(_, d)| d.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut removed = 0;
    if let Some(store) = raw.get_mut("raster_tiles").and_then(Value::as_object_mut) {
        let before = store.len();
        store.retain(|digest, _| referenced.contains(digest));
        removed = before - store.len();
    }
    removed
}

fn find_raster_mut<'a>(raw: &'a mut Value, page: Option<&str>, id: &str) -> Option<&'a mut Value> {
    fn walk<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
        for node in nodes {
            if node["id"] == id {
                return Some(node);
            }
            if let Some(found) = node
                .get_mut("children")
                .and_then(Value::as_array_mut)
                .and_then(|c| walk(c, id))
            {
                return Some(found);
            }
        }
        None
    }
    for p in raw["pages"].as_array_mut()? {
        if page.is_some_and(|wanted| p["id"] != wanted) {
            continue;
        }
        for layer in p["layers"].as_array_mut()? {
            if let Some(found) = layer
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .and_then(|n| walk(n, id))
            {
                return Some(found);
            }
        }
    }
    None
}

fn page_value<'a>(raw: &'a Value, page: Option<&str>) -> Result<&'a Value> {
    let pages = raw["pages"].as_array().context("document has no pages")?;
    match page {
        Some(wanted) => pages.iter().find(|p| p["id"] == wanted),
        None => pages.first(),
    }
    .with_context(|| {
        format!(
            "[not-found] page {} was not found",
            page.unwrap_or("(first)")
        )
    })
}

fn locate<'a>(raw: &'a mut Value, page: Option<&str>, id: &str) -> Result<&'a mut Value> {
    find_raster_mut(raw, page, id).ok_or_else(|| {
        bad(
            "not-found",
            format!("raster layer {id} was not found; list nodes with `pentool tree DOCUMENT --kind raster`"),
        )
    })
}

fn ensure_node_unlocked(raw: &Value, page: Option<&str>, id: &str) -> Result<()> {
    if !all_rasters(raw).iter().any(|node| node["id"] == id) {
        return Err(bad(
            "not-found",
            format!("raster layer {id} was not found; list nodes with `pentool tree DOCUMENT --kind raster`"),
        ));
    }
    crate::composite::ensure_unlocked(page_value(raw, page)?, id)
}

/// Create an empty raster layer in the first (or named) layer of a v6 document.
#[allow(clippy::too_many_arguments)]
pub fn add(
    raw: &mut Value,
    page: Option<&str>,
    layer: Option<&str>,
    id: &str,
    x: f64,
    y: f64,
    width: u32,
    height: u32,
) -> Result<Value> {
    if id.is_empty() {
        bail!("[invalid-input] raster id must not be empty")
    }
    if width == 0
        || height == 0
        || u64::from(width) > MAX_DIMENSION
        || u64::from(height) > MAX_DIMENSION
    {
        bail!("[limit-exceeded] raster width and height must be 1-{MAX_DIMENSION}")
    }
    let mut migrated = crate::composite::migrate(raw.clone())?;
    let pages = migrated["pages"]
        .as_array_mut()
        .context("document has no pages")?;
    let target_page = match page {
        Some(wanted) => pages.iter_mut().find(|p| p["id"] == wanted),
        None => pages.first_mut(),
    }
    .context("[not-found] page was not found")?;
    let layers = target_page["layers"]
        .as_array_mut()
        .context("page has no layers")?;
    let layer_value = match layer {
        Some(wanted) => layers.iter_mut().find(|l| l["id"] == wanted),
        None => layers.first_mut(),
    }
    .context("[not-found] layer was not found")?;
    if layer_value["locked"] == true {
        bail!("[locked-node] layer is locked; unlock it first")
    }
    layer_value["nodes"]
        .as_array_mut()
        .context("layer nodes are missing")?
        .push(json!({
            "kind": "raster",
            "id": id,
            "x": x,
            "y": y,
            "width": width,
            "height": height,
            "tile_size": TILE,
            "pixel_format": FORMAT,
            "tiles": {},
            "journal": [],
            "opacity": 1,
            "blend_mode": "normal",
            "transform": [1, 0, 0, 1, 0, 0]
        }));
    migrated
        .as_object_mut()
        .context("document must be an object")?
        .entry("raster_tiles")
        .or_insert_with(|| json!({}));
    crate::scene::validate(&migrated)?;
    *raw = migrated;
    Ok(json!({"id":id,"width":width,"height":height,"tile_size":TILE,"tiles":0}))
}

pub fn info(raw: &Value, page: Option<&str>, id: &str) -> Result<Value> {
    let mut copy = raw.clone();
    let node = locate(&mut copy, page, id)?.clone();
    let surface = Surface::load(raw, &node)?;
    let painted: usize = surface.tiles.values().filter(|t| !is_blank(t)).count();
    Ok(json!({
        "id": id,
        "width": surface.width,
        "height": surface.height,
        "tile_size": TILE,
        "pixel_format": FORMAT,
        "tiles": painted,
        "tiles_possible": u64::from(tiles_across(surface.width)) * u64::from(tiles_across(surface.height)),
        "tile_map_sha256": surface.tile_map_hash(),
        "journal_entries": node["journal"].as_array().map_or(0, Vec::len),
        "journal_dropped": node["checkpoint"]["journal_dropped"].as_u64().unwrap_or(0),
        "engine": ENGINE,
    }))
}

pub fn clear(raw: &mut Value, page: Option<&str>, id: &str) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let mut next = raw.clone();
    let node = locate(&mut next, page, id)?;
    let cleared = node["tiles"].as_object().map_or(0, Map::len);
    node["tiles"] = json!({});
    node["journal"] = json!([]);
    node.as_object_mut().map(|o| o.remove("checkpoint"));
    let removed = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({"id":id,"tiles_cleared":cleared,"tiles_released":removed}))
}

pub fn checkpoint(raw: &mut Value, page: Option<&str>, id: &str, compact: bool) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let mut next = raw.clone();
    let snapshot = locate(&mut next, page, id)?.clone();
    let surface = Surface::load(&next, &snapshot)?;
    let node = locate(&mut next, page, id)?;
    let entries = node["journal"].as_array().map_or(0, Vec::len);
    let dropped = node["checkpoint"]["journal_dropped"].as_u64().unwrap_or(0);
    let hash = surface.tile_map_hash();
    if compact {
        node["journal"] = json!([]);
    }
    node["checkpoint"] = json!({
        "engine": ENGINE,
        "tile_map_sha256": hash,
        "journal_dropped": dropped + if compact { entries as u64 } else { 0 },
    });
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({"id":id,"tile_map_sha256":hash,"journal_compacted":if compact {entries} else {0}}))
}

pub struct StrokeRequest {
    pub brush: Brush,
    pub samples: Vec<Sample>,
    pub color: [u8; 3],
    pub blend: Blend,
    pub seed: u64,
}

/// Apply one stroke to a raster layer inside `raw` and append its journal entry.
pub fn paint(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    request: StrokeRequest,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let mut next = raw.clone();
    let snapshot = locate(&mut next, page, id)?.clone();
    let mut surface = Surface::load(&next, &snapshot)?;
    let stroke = Stroke {
        brush: request.brush.clone(),
        samples: request.samples.clone(),
        color: request.color,
        blend: request.blend,
        seed: request.seed,
    };
    let before = surface.tile_map_hash();
    let result = apply_stroke(&mut surface, &stroke)?;
    let after = surface.tile_map_hash();
    let canonical = json!({
        "brush": request.brush.to_json(),
        "samples": request.samples.iter().map(|s| json!([s.x, s.y, s.pressure])).collect::<Vec<_>>(),
        "color": format!("#{:02X}{:02X}{:02X}", request.color[0], request.color[1], request.color[2]),
        "blend": request.blend.name(),
        "seed": request.seed,
    });
    let node = locate(&mut next, page, id)?;
    let index = node["journal"].as_array().map_or(0, Vec::len)
        + node["checkpoint"]["journal_dropped"].as_u64().unwrap_or(0) as usize;
    let stroke_id = format!(
        "s{}-{}",
        index,
        crate::resource::sha256(format!("{id}:{index}:{before}:{canonical}").as_bytes())
            .trim_start_matches("sha256:")
            .chars()
            .take(12)
            .collect::<String>()
    );
    let mut entry = canonical;
    entry["id"] = json!(stroke_id);
    entry["engine"] = json!(ENGINE);
    entry["bounds"] = json!(result.bounds);
    entry["tiles_changed"] = json!(result.tiles_changed);
    entry["tile_map_sha256"] = json!(after);
    {
        let journal = node["journal"].as_array_mut().context("journal missing")?;
        journal.push(entry);
        let mut dropped_now = 0u64;
        while journal.len() > MAX_JOURNAL {
            journal.remove(0);
            dropped_now += 1;
        }
        if dropped_now > 0 {
            let prior = node["checkpoint"]["journal_dropped"].as_u64().unwrap_or(0);
            node["checkpoint"] = json!({"engine":ENGINE,"tile_map_sha256":before,"journal_dropped":prior + dropped_now});
        }
    }
    let mut node_copy = node.clone();
    surface.store(&mut next, &mut node_copy)?;
    *locate(&mut next, page, id)? = node_copy;
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "stroke": stroke_id,
        "dabs": result.dabs,
        "bounds": result.bounds,
        "tiles_changed": result.tiles_changed,
        "tiles_total": surface.tiles.len(),
        "tile_map_sha256": after,
        "tiles_released": released,
    }))
}

/// Render a raster node to a PNG data URI for the SVG renderer.
pub fn render_png(raw: &Value, node: &Value) -> Result<Vec<u8>> {
    let surface = Surface::load(raw, node)?;
    let image = surface.to_image()?;
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .context("could not encode raster layer")?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface() -> Surface {
        Surface {
            width: 600,
            height: 300,
            tiles: BTreeMap::new(),
        }
    }

    fn stroke(kind: &str) -> Stroke {
        Stroke {
            brush: Brush::parse(&json!({"kind":kind,"size":24,"hardness":0.4,"flow":0.6,"spacing":0.15,"pressure_size":true,"scatter":0.2})).unwrap(),
            samples: parse_samples(&json!([[20,30,0.2],[300.5,120.25,1.0],[560,260,0.5]])).unwrap(),
            color: [210, 161, 132],
            blend: Blend::Normal,
            seed: 42,
        }
    }

    #[test]
    fn sin_cos_matches_known_values_without_libm() {
        let (s, c) = sin_cos_degrees(30.0);
        assert!((s - 0.5).abs() < 1e-12 && (c - 0.866_025_403_784_438_6).abs() < 1e-12);
        let (s, c) = sin_cos_degrees(-270.0);
        assert!((s - 1.0).abs() < 1e-12 && c.abs() < 1e-12);
    }

    #[test]
    fn replaying_a_stroke_reproduces_identical_tiles() {
        for kind in BRUSH_KINDS {
            let (mut a, mut b) = (surface(), surface());
            apply_stroke(&mut a, &stroke(kind)).unwrap();
            apply_stroke(&mut b, &stroke(kind)).unwrap();
            assert_eq!(a.tile_map_hash(), b.tile_map_hash(), "{kind}");
            assert!(!a.tiles.is_empty(), "{kind}");
        }
    }

    #[test]
    fn stroke_is_clipped_to_the_layer_and_crosses_tile_seams() {
        let mut s = surface();
        let mut request = stroke("hard-round");
        request.samples = parse_samples(&json!([[-50, 150], [650, 150]])).unwrap();
        request.brush = Brush::parse(&json!({"size":20,"flow":1})).unwrap();
        apply_stroke(&mut s, &request).unwrap();
        for x in 0..600 {
            assert_eq!(s.pixel(x, 150)[3], 255, "gap at x={x}");
        }
        assert_eq!(s.tiles.keys().map(|k| k.0).max(), Some(2));
        assert!(s.tiles.keys().all(|k| k.0 <= 2 && k.1 <= 1));
        assert_eq!(
            s.pixel(255, 150),
            s.pixel(256, 150),
            "no seam at the tile edge"
        );
    }

    #[test]
    fn erase_removes_alpha_and_blank_tiles_are_dropped() {
        let mut s = surface();
        let mut paint = stroke("hard-round");
        paint.brush = Brush::parse(&json!({"size":30})).unwrap();
        paint.samples = parse_samples(&json!([[100, 100]])).unwrap();
        apply_stroke(&mut s, &paint).unwrap();
        assert_eq!(s.pixel(100, 100)[3], 255);
        paint.blend = Blend::Erase;
        paint.brush = Brush::parse(&json!({"size":40})).unwrap();
        apply_stroke(&mut s, &paint).unwrap();
        assert!(s.tiles.is_empty());
    }

    #[test]
    fn limits_fail_before_pixel_work() {
        let mut s = surface();
        let mut request = stroke("hard-round");
        request.brush = Brush::parse(&json!({"size":2048,"spacing":0.01})).unwrap();
        request.samples = parse_samples(&json!([[0, 0], [5000, 5000]])).unwrap();
        assert!(apply_stroke(&mut s, &request).is_err());
        assert!(Brush::parse(&json!({"size":0})).is_err());
        assert!(Brush::parse(&json!({"kind":"airbrush"})).is_err());
        assert!(Brush::parse(&json!({"bogus":1})).is_err());
        assert!(parse_samples(&json!([])).is_err());
        assert!(parse_samples(&json!([[1, 2, 3]])).is_err());
    }
}

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

mod api;
mod clone;
mod compose;
mod flood;
mod heal;
mod input;
mod layer;
mod math;
mod pixels;
mod preset;
mod recovery;
mod retouch;
mod selection;
mod shape;
mod tip;
pub use api::{apply, batch, MAX_BATCH_OPS, MAX_BATCH_SAMPLES};
pub use clone::{
    set_source as set_clone_source, stroke as clone_stroke, Options as CloneOptions,
    Request as CloneRequest, Spec as CloneSpec,
};
pub use compose::{flatten, merge_visible, orient, stamp_visible, Orient};
pub use flood::{fill, wand as select_wand, Options as FloodOptions};
pub use heal::spot as heal_spot;
use input::lerp;
pub use input::{
    normalize_input, parse_samples, sample_json, Dynamic, Dynamics, Input, NormalizedInput, Sample,
    FOLD_DISTANCE, MAX_EVENTS, MAX_VELOCITY,
};
pub use layer::{crop, duplicate, merge_down, rasterize, resize, trim, Resample};
use math::{signed_unit, sin_cos_degrees, smoothstep};
pub use pixels::{
    lift as lift_pixels, move_pixels, paste as paste_pixels, transform_pixels,
    Transform as PixelTransform,
};
pub use preset::{
    preset_add, preset_export, preset_import, preset_import_gbr, preset_import_mypaint,
    preset_list, preset_remove, preset_show, resolve_preset,
};
pub use recovery::{repair, replay, verify, RepairStrategy};
pub use retouch::Tool;
pub use selection::{clear_op as select_clear, info_op as select_info, Mode as SelectionMode};
pub use shape::{
    delete as select_delete, lasso as select_lasso, load as select_load, marquee as select_marquee,
    modify as select_modify, quickmask as select_quickmask, save as select_save, Marquee,
    Op as SelectionOp,
};
pub use tip::{load_tip, resolve_tip, tip_add, tip_from_image, tip_list, tip_remove, TipSource};

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
        Self::load_map(raw, width, height, &node["tiles"])
    }

    /// Load a surface from any `"x,y" -> digest` map (live tiles or a checkpoint).
    pub fn load_map(raw: &Value, width: u32, height: u32, tiles: &Value) -> Result<Self> {
        let mut surface = Surface {
            width,
            height,
            tiles: BTreeMap::new(),
        };
        let store = raw.get("raster_tiles").and_then(Value::as_object);
        for (key, digest) in tiles.as_object().into_iter().flatten() {
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
        self.store_with(raw, node, false)
    }

    /// Like [`Surface::store`]; `rewrite` replaces existing store entries, which
    /// repairs a damaged entry whose digest is still correct.
    pub fn store_with(&self, raw: &mut Value, node: &mut Value, rewrite: bool) -> Result<()> {
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
            if rewrite || !store.contains_key(&digest) {
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
    /// EMA weight of the previous smoothed sample (0 = off, max 0.95).
    pub smoothing: f64,
    /// true: overlapping dabs accumulate flow; false: coverage is the per-pixel max.
    pub buildup: bool,
    /// `textured` only: a `sha256:` digest of a stamp tip, or a `brush_tips` name
    /// until [`resolve_tip`] replaces it with the digest.
    pub tip: Option<String>,
    /// Input-driven size, flow, roundness and angle; see [`Dynamics`].
    pub dynamics: Dynamics,
    /// Local-tool parameters, valid only with the blends that use them.
    pub strength: Option<f64>,
    pub tolerance: Option<u32>,
    pub range: Option<String>,
    pub mode: Option<String>,
    /// Heal only: share of source detail and of destination tone, 0-1 (default 1).
    pub texture: Option<f64>,
    pub tone: Option<f64>,
    /// Name of the document preset this brush was resolved from (informational).
    pub preset: Option<String>,
}

pub const BRUSH_KINDS: [&str; 5] = [
    "hard-round",
    "soft-round",
    "pixel",
    "calligraphic",
    "textured",
];
pub const MAX_SMOOTHING: f64 = 0.95;
pub const MIN_ROUNDNESS: f64 = 0.05;
/// A textured tip is one 256x256 coverage plane stored as a raster tile.
pub type TipMask = std::sync::Arc<[u8]>;

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
        const KNOWN: [&str; 23] = [
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
            "smoothing",
            "buildup",
            "tip",
            "dynamics",
            "strength",
            "tolerance",
            "range",
            "mode",
            "texture",
            "tone",
            "preset",
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
        let flag_or = |key: &str, default: bool| -> Result<bool> {
            match object.get(key) {
                None => Ok(default),
                Some(Value::Bool(b)) => Ok(*b),
                _ => bail!("[invalid-brush] {key} must be true or false"),
            }
        };
        let flag = |key: &str| flag_or(key, false);
        let pixel = kind == "pixel";
        let tip = match object.get("tip") {
            None => None,
            Some(Value::String(text)) if !text.is_empty() && text.len() <= 128 => {
                Some(text.clone())
            }
            Some(_) => bail!("[invalid-brush] tip must be a sha256: digest or a brush_tips name"),
        };
        match (kind == "textured", tip.is_some()) {
            (true, false) => bail!("[invalid-brush] a textured brush needs \"tip\"; add one with `pentool raster DOC tip-add NAME --image tip.png`"),
            (false, true) => bail!("[invalid-brush] tip is only valid for kind \"textured\""),
            _ => {}
        }
        let optional_text = |key: &str, check: fn(&str) -> Result<()>| -> Result<Option<String>> {
            match object.get(key) {
                None => Ok(None),
                Some(Value::String(text)) => {
                    check(text)?;
                    Ok(Some(text.clone()))
                }
                Some(_) => bail!("[invalid-brush] {key} must be a string"),
            }
        };
        let strength = match object.get("strength") {
            None => None,
            Some(_) => Some(number_in(object, "strength", 0.5, 0.0, 1.0)?),
        };
        let tolerance = match object.get("tolerance") {
            None => None,
            Some(_) => Some(number_in(object, "tolerance", 32.0, 0.0, 255.0)?),
        };
        if tolerance.is_some_and(|t| t.fract() != 0.0) {
            bail!("[invalid-brush] tolerance must be a whole number between 0 and 255")
        }
        let texture = match object.get("texture") {
            None => None,
            Some(_) => Some(number_in(object, "texture", 1.0, 0.0, 1.0)?),
        };
        let tone = match object.get("tone") {
            None => None,
            Some(_) => Some(number_in(object, "tone", 1.0, 0.0, 1.0)?),
        };
        let preset = optional_text("preset", preset::check_name)?;
        let range = optional_text("range", retouch::parse_range)?;
        let mode = optional_text("mode", retouch::parse_mode)?;
        let dynamics = Dynamics::parse(object.get("dynamics"))?;
        if dynamics.size.is_some() && flag("pressure_size")? {
            bail!("[invalid-brush] use either pressure_size or dynamics.size, not both")
        }
        if dynamics.flow.is_some() && flag("pressure_flow")? {
            bail!("[invalid-brush] use either pressure_flow or dynamics.flow, not both")
        }
        Ok(Brush {
            preset,
            dynamics,
            strength,
            tolerance: tolerance.map(|t| t as u32),
            range,
            mode,
            texture,
            tone,
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
            smoothing: number_in(object, "smoothing", 0.0, 0.0, MAX_SMOOTHING)?,
            buildup: flag_or("buildup", true)?,
            tip,
            kind,
        })
    }

    /// Canonical journal form. Properties added after engine 1 first shipped are
    /// written only when they differ from their defaults, so earlier journals and
    /// stroke IDs stay byte-identical.
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "kind": self.kind, "size": self.size, "hardness": self.hardness,
            "spacing": self.spacing, "opacity": self.opacity, "flow": self.flow,
            "angle": self.angle, "roundness": self.roundness, "scatter": self.scatter,
            "pressure_size": self.pressure_size, "pressure_flow": self.pressure_flow,
            "min_size": self.min_size,
        });
        if self.smoothing != 0.0 {
            out["smoothing"] = json!(self.smoothing);
        }
        if !self.buildup {
            out["buildup"] = json!(false);
        }
        if let Some(tip) = &self.tip {
            out["tip"] = json!(tip);
        }
        if !self.dynamics.is_empty() {
            out["dynamics"] = self.dynamics.to_json();
        }
        if let Some(strength) = self.strength {
            out["strength"] = json!(strength);
        }
        if let Some(tolerance) = self.tolerance {
            out["tolerance"] = json!(tolerance);
        }
        if let Some(range) = &self.range {
            out["range"] = json!(range);
        }
        if let Some(mode) = &self.mode {
            out["mode"] = json!(mode);
        }
        if let Some(texture) = self.texture {
            out["texture"] = json!(texture);
        }
        if let Some(tone) = self.tone {
            out["tone"] = json!(tone);
        }
        if let Some(preset) = &self.preset {
            out["preset"] = json!(preset);
        }
        out
    }

    fn step(&self) -> f64 {
        if self.kind == "pixel" {
            1.0
        } else {
            (self.spacing * self.size).max(0.25)
        }
    }
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
    /// Erasing and local blending tools; see `retouch.rs`.
    Tool(Tool),
}

struct Dab {
    x: f64,
    y: f64,
    size: f64,
    flow: f64,
    angle: f64,
    roundness: f64,
}

/// Exponential moving average over position and pressure:
/// `p[i] = p[i-1] + (1 - smoothing) * (raw[i] - p[i-1])`, starting at the first raw
/// sample. The raw last sample is appended when the average lags behind it, so a
/// smoothed stroke still ends where the pen lifted. Tilt, azimuth, twist and
/// velocity pass through unsmoothed.
fn smooth(samples: &[Sample], smoothing: f64) -> Vec<Sample> {
    if smoothing <= 0.0 || samples.len() < 2 {
        return samples.to_vec();
    }
    let k = 1.0 - smoothing;
    let mut out = Vec::with_capacity(samples.len() + 1);
    let mut p = samples[0];
    out.push(p);
    for s in &samples[1..] {
        p = Sample {
            x: p.x + k * (s.x - p.x),
            y: p.y + k * (s.y - p.y),
            pressure: p.pressure + k * (s.pressure - p.pressure),
            ..*s
        };
        out.push(p);
    }
    let last = samples[samples.len() - 1];
    if p.x != last.x || p.y != last.y || p.pressure != last.pressure {
        out.push(last);
    }
    out
}

fn dabs(brush: &Brush, samples: &[Sample], seed: u64) -> Result<Vec<Dab>> {
    let smoothed = smooth(samples, brush.smoothing);
    let samples = &smoothed[..];
    let step = brush.step();
    let mut points: Vec<Sample> = Vec::new();
    let mut total = 0.0_f64;
    for pair in samples.windows(2) {
        let (dx, dy) = (pair[1].x - pair[0].x, pair[1].y - pair[0].y);
        total += (dx * dx + dy * dy).sqrt();
    }
    if total / step + 1.0 > MAX_DABS as f64 {
        bail!("[limit-exceeded] this stroke needs more than {MAX_DABS} dabs; use a larger spacing or split the stroke")
    }
    points.push(samples[0]);
    let mut traveled = 0.0_f64;
    let mut next = step;
    for pair in samples.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length = (dx * dx + dy * dy).sqrt();
        if length == 0.0 {
            continue;
        }
        while next <= traveled + length {
            points.push(lerp(a, b, (next - traveled) / length));
            next += step;
        }
        traveled += length;
    }
    let dynamics = &brush.dynamics;
    let mut state = seed;
    let mut out = Vec::with_capacity(points.len());
    for point in points {
        let pressure = point.pressure;
        let mut size = brush.size;
        if brush.pressure_size {
            size *= brush.min_size + (1.0 - brush.min_size) * pressure;
        }
        if let Some(dynamic) = &dynamics.size {
            size *= dynamic.eval(&point);
        }
        let (mut px, mut py) = (point.x, point.y);
        if brush.scatter > 0.0 {
            let reach = brush.scatter * brush.size;
            px += signed_unit(&mut state) * reach;
            py += signed_unit(&mut state) * reach;
        }
        let mut flow = if brush.pressure_flow {
            brush.flow * pressure
        } else {
            brush.flow
        };
        if let Some(dynamic) = &dynamics.flow {
            flow *= dynamic.eval(&point);
        }
        let roundness = match &dynamics.roundness {
            Some(dynamic) => (brush.roundness * dynamic.eval(&point)).max(MIN_ROUNDNESS),
            None => brush.roundness,
        };
        let angle = match &dynamics.angle {
            Some(dynamic) => brush.angle + dynamic.eval(&point),
            None => brush.angle,
        };
        out.push(Dab {
            x: px,
            y: py,
            size: size.max(1.0),
            flow,
            angle,
            roundness,
        });
    }
    Ok(out)
}

type StrokeBuffer = HashMap<(u32, u32), Vec<u16>>;

/// Bilinear sample of a 256x256 tip at tip-space `(tu, tv)` in `[-1, 1]`, where the
/// tip square spans the dab's diameter. Outside the tip is zero coverage.
fn tip_coverage(tip: &[u8], tu: f64, tv: f64) -> f64 {
    let side = TILE as f64;
    let fx = (tu + 1.0) * (side / 2.0) - 0.5;
    let fy = (tv + 1.0) * (side / 2.0) - 0.5;
    if fx <= -1.0 || fy <= -1.0 || fx >= side || fy >= side {
        return 0.0;
    }
    let (x0, y0) = (fx.floor(), fy.floor());
    let (ax, ay) = (fx - x0, fy - y0);
    let at = |x: f64, y: f64| -> f64 {
        if x < 0.0 || y < 0.0 || x >= side || y >= side {
            0.0
        } else {
            f64::from(tip[y as usize * TILE + x as usize])
        }
    };
    let top = at(x0, y0) * (1.0 - ax) + at(x0 + 1.0, y0) * ax;
    let bottom = at(x0, y0 + 1.0) * (1.0 - ax) + at(x0 + 1.0, y0 + 1.0) * ax;
    (top * (1.0 - ay) + bottom * ay) / 255.0
}

/// Visit every pixel a dab covers with its coverage in (0, 1].
fn dab_pixels(
    brush: &Brush,
    tip: Option<&[u8]>,
    dab: &Dab,
    width: u32,
    height: u32,
    mut visit: impl FnMut(i64, i64, f64),
) {
    let radius = dab.size / 2.0;
    // A rotated square tip reaches its corners at radius * sqrt(2).
    let corner = if tip.is_some() { 1.4143 } else { 1.0 };
    let reach = radius * corner / dab.roundness.clamp(MIN_ROUNDNESS, 1.0) + 1.0;
    let x0 = ((dab.x - reach).floor()).max(0.0) as i64;
    let y0 = ((dab.y - reach).floor()).max(0.0) as i64;
    let x1 = ((dab.x + reach).ceil()).min(f64::from(width)) as i64;
    let y1 = ((dab.y + reach).ceil()).min(f64::from(height)) as i64;
    let (sin, cos) = sin_cos_degrees(dab.angle);
    if x1 <= x0 || y1 <= y0 {
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
                let v = (-dx * sin + dy * cos) / dab.roundness;
                let d = (u * u + v * v).sqrt();
                if let Some(tip) = tip {
                    tip_coverage(tip, u / radius, v / radius)
                } else if brush.kind == "soft-round" && brush.hardness < 1.0 {
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
            if coverage > 0.0 {
                visit(x, y, coverage);
            }
        }
    }
}

/// Accumulate one dab into the per-stroke coverage buffer: flow build-up, or the
/// per-pixel maximum when `buildup` is false.
fn stamp(
    buffer: &mut StrokeBuffer,
    brush: &Brush,
    tip: Option<&[u8]>,
    dab: &Dab,
    width: u32,
    height: u32,
) {
    let flow16 = (dab.flow * 65535.0).round();
    if flow16 <= 0.0 {
        return;
    }
    dab_pixels(brush, tip, dab, width, height, |x, y, coverage| {
        let dab16 = (coverage * flow16).round() as u32;
        if dab16 == 0 {
            return;
        }
        let key = ((x as u32) / TILE as u32, (y as u32) / TILE as u32);
        let cell = buffer.entry(key).or_insert_with(|| vec![0u16; TILE * TILE]);
        let at = ((y as usize) % TILE) * TILE + (x as usize) % TILE;
        let current = u32::from(cell[at]);
        cell[at] = if brush.buildup {
            let added = (dab16 * (65535 - current) + 32767) / 65535;
            (current + added).min(65535) as u16
        } else {
            current.max(dab16.min(65535)) as u16
        };
    });
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
        Blend::Tool(_) => unreachable!("local tools do not composite a color"),
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
    /// Decoded coverage of `brush.tip`; see [`load_tip`].
    pub tip: Option<TipMask>,
    /// Required by (and only valid with) the `clone` blend.
    pub clone: Option<CloneSpec>,
}

pub struct StrokeResult {
    pub bounds: Option<[u32; 4]>,
    pub tiles_changed: usize,
    pub dabs: usize,
}

/// Apply a stroke to a surface. Fails before any pixel work if limits are exceeded.
/// A smudged tile waiting to be written back once the stroke finishes.
type PendingTile = ((u32, u32), Box<[u8]>);

/// Apply a stroke limited by a selection: each pixel keeps the stroke's result in
/// proportion to its coverage.
pub fn apply_stroke_selected(
    surface: &mut Surface,
    stroke: &Stroke,
    selection: Option<&Surface>,
) -> Result<StrokeResult> {
    let Some(selection) = selection else {
        return apply_stroke(surface, stroke);
    };
    let before = surface.clone();
    let mut result = apply_stroke(surface, stroke)?;
    selection::blend_through(&before, surface, selection);
    result.tiles_changed = surface
        .tiles
        .keys()
        .chain(before.tiles.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|key| surface.tiles.get(key) != before.tiles.get(key))
        .count();
    Ok(result)
}

pub fn apply_stroke(surface: &mut Surface, stroke: &Stroke) -> Result<StrokeResult> {
    let tip = match (&stroke.brush.tip, &stroke.tip) {
        (None, _) => None,
        (Some(_), Some(mask)) if mask.len() == TILE * TILE => Some(&mask[..]),
        (Some(name), _) => bail!("[missing-resource] brush tip {name} is not loaded"),
    };
    retouch::check(&stroke.brush, stroke.blend)?;
    let dab_list = dabs(&stroke.brush, &stroke.samples, stroke.seed)?;
    let corner = if tip.is_some() { 1.4143 } else { 1.0 };
    // Size and roundness dynamics only shrink a dab, so the narrowest possible
    // roundness bounds the work.
    let roundness =
        (stroke.brush.roundness * stroke.brush.dynamics.min_roundness()).max(MIN_ROUNDNESS);
    let footprint =
        (stroke.brush.size * corner / roundness + 3.0).min(MAX_BRUSH_SIZE * 20.0 * corner);
    let work = (dab_list.len() as f64 * footprint * footprint) as u64
        * retouch::work_factor(&stroke.brush, stroke.blend);
    if work > MAX_STROKE_WORK {
        bail!("[limit-exceeded] stroke work {work} exceeds {MAX_STROKE_WORK}; reduce size or sample length")
    }
    if stroke.blend == Blend::Tool(Tool::Smudge) {
        let (bounds, tiles_changed) = retouch::smudge(surface, stroke, tip, &dab_list)?;
        return Ok(StrokeResult {
            bounds,
            tiles_changed,
            dabs: dab_list.len(),
        });
    }
    let cloner = match (stroke.blend, &stroke.clone) {
        (Blend::Tool(Tool::Clone | Tool::Heal), Some(spec)) => Some(spec),
        (Blend::Tool(Tool::Clone), None) => {
            bail!("[invalid-stroke] blend clone needs a clone source; use `raster clone-stroke`")
        }
        (Blend::Tool(Tool::Heal), None) => {
            bail!("[invalid-stroke] blend heal needs a clone source; use `raster heal-stroke`")
        }
        (_, Some(_)) => {
            bail!("[invalid-stroke] a clone source only applies to blends clone and heal")
        }
        _ => None,
    };
    let local = match stroke.blend {
        Blend::Tool(Tool::Clone | Tool::Heal) => None,
        Blend::Tool(tool) => Some(retouch::Context::new(
            tool,
            &stroke.brush,
            stroke.color,
            surface,
            dab_list.first(),
        )?),
        _ => None,
    };
    let mut buffer = StrokeBuffer::new();
    for dab in &dab_list {
        stamp(
            &mut buffer,
            &stroke.brush,
            tip,
            dab,
            surface.width,
            surface.height,
        );
    }
    let opacity16 = (stroke.brush.opacity * 65535.0).round() as u32;
    let field = match (stroke.blend, cloner) {
        (Blend::Tool(Tool::Heal), Some(spec)) => Some(heal::solve(
            surface,
            spec,
            &buffer,
            opacity16,
            &stroke.brush,
        )?),
        _ => None,
    };
    let mut touched = 0usize;
    let mut bounds: Option<[u32; 4]> = None;
    let mut keys: Vec<_> = buffer.keys().copied().collect();
    keys.sort_unstable();
    // Results are applied after the loop so neighbor reads never see a tile this
    // stroke already changed.
    let mut pending: Vec<PendingTile> = Vec::new();
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
            let x = key.0 * TILE as u32 + (index % TILE) as u32;
            let y = key.1 * TILE as u32 + (index / TILE) as u32;
            if let Some(spec) = cloner {
                let p = match &field {
                    Some(field) => field.pixel(x, y),
                    None => spec.pixel(surface, x, y),
                };
                let alpha = (alpha16 * u32::from(p[3]) + 127) / 255;
                if alpha > 0 {
                    composite(
                        &mut tile,
                        index * 4,
                        [p[0], p[1], p[2]],
                        alpha,
                        Blend::Normal,
                    );
                }
            } else {
                match &local {
                    Some(context) => {
                        let out = context.pixel(surface, x, y, i64::from(alpha16));
                        tile[index * 4..index * 4 + 4].copy_from_slice(&out);
                    }
                    None => composite(&mut tile, index * 4, stroke.color, alpha16, stroke.blend),
                }
            }
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
        pending.push((key, tile));
    }
    for (key, tile) in pending {
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
        if !digest.as_str().is_some_and(is_digest) {
            bail!("[malformed-raster] raster {id}: tile {key} digest must be sha256:<64 hex>")
        }
    }
    node_engine(node)?;
    let journal_len = match object.get("journal") {
        None => 0,
        Some(journal) => {
            let journal = journal.as_array().with_context(|| {
                format!("[malformed-raster] raster {id}: journal must be an array")
            })?;
            if journal.len() > MAX_JOURNAL {
                bail!("[limit-exceeded] raster {id} journal exceeds {MAX_JOURNAL} entries; run `raster checkpoint --compact`")
            }
            journal.len()
        }
    };
    if let Some(checkpoint) = object.get("checkpoint") {
        let checkpoint = checkpoint.as_object().with_context(|| {
            format!("[malformed-raster] raster {id}: checkpoint must be an object")
        })?;
        if let Some(tiles) = checkpoint.get("tiles") {
            let tiles = tiles.as_object().with_context(|| {
                format!("[malformed-raster] raster {id}: checkpoint tiles must be an object")
            })?;
            if tiles.len() > MAX_TILES {
                bail!("[limit-exceeded] raster {id} checkpoint has more than {MAX_TILES} tiles")
            }
            for (key, digest) in tiles {
                parse_key(key)?;
                if !digest.as_str().is_some_and(is_digest) {
                    bail!("[malformed-raster] raster {id}: checkpoint tile {key} digest must be sha256:<64 hex>")
                }
            }
        }
        let offset = checkpoint
            .get("journal_offset")
            .map_or(Some(0), Value::as_u64)
            .with_context(|| {
                format!(
                    "[malformed-raster] raster {id}: checkpoint journal_offset must be an integer"
                )
            })?;
        if offset as usize > journal_len {
            bail!("[malformed-raster] raster {id}: checkpoint journal_offset {offset} is past the {journal_len}-entry journal")
        }
    }
    Ok(())
}

fn is_digest(text: &str) -> bool {
    text.len() == 71
        && text.starts_with("sha256:")
        && text[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The raster engine version that last wrote a node (1 when absent).
pub fn node_engine(node: &Value) -> Result<u64> {
    match node.get("engine") {
        None => Ok(1),
        Some(value) => value.as_u64().filter(|n| *n >= 1).with_context(|| {
            format!(
                "[malformed-raster] raster {}: engine must be a positive integer",
                node["id"].as_str().unwrap_or("?")
            )
        }),
    }
}

/// Every content digest a raster node pins: live tiles, checkpoint tiles and any
/// digests recorded in journal entries (for example clone sources).
fn pinned_digests(value: &Value, out: &mut HashSet<String>) {
    match value {
        Value::String(text) if is_digest(text) => {
            out.insert(text.clone());
        }
        Value::Array(items) => items.iter().for_each(|item| pinned_digests(item, out)),
        Value::Object(map) => map.values().for_each(|item| pinned_digests(item, out)),
        _ => {}
    }
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

/// Every raster node anywhere in the document, including copies held by component
/// snapshots and instance fallbacks. Tile retention and validation use this so a
/// raster reachable only through a component never loses its pixels.
fn every_raster(raw: &Value) -> Vec<&Value> {
    fn walk<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
        match value {
            Value::Object(map) => {
                if is_raster(value) {
                    out.push(value);
                }
                map.values().for_each(|item| walk(item, out));
            }
            Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (key, value) in raw.as_object().into_iter().flatten() {
        if key != "raster_tiles" {
            walk(value, &mut out);
        }
    }
    out
}

/// Document-level check: every referenced tile exists with a well-formed entry.
pub fn validate_document(raw: &Value) -> Result<()> {
    let rasters = every_raster(raw);
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
    tip::validate_tips(raw)?;
    preset::validate_presets(raw)
}

/// Digests that must stay in `raster_tiles`: everything pinned by raster nodes plus
/// document-level raster resources (named tips, saved selections, brush presets).
fn retained_digests(raw: &Value) -> HashSet<String> {
    let mut referenced = HashSet::new();
    for node in every_raster(raw) {
        pinned_digests(node, &mut referenced);
    }
    for key in ["brush_tips", "raster_selections", "brush_presets"] {
        if let Some(value) = raw.get(key) {
            pinned_digests(value, &mut referenced);
        }
    }
    referenced
}

/// Remove stored tiles nothing references.
pub fn collect_garbage(raw: &mut Value) -> usize {
    let referenced = retained_digests(raw);
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

/// A raster node may be mutated only when it exists, is unlocked and was written by
/// an engine this build implements. Newer-engine layers stay viewable and
/// exportable from their materialized tiles but are never rewritten or replayed.
fn ensure_node_unlocked(raw: &Value, page: Option<&str>, id: &str) -> Result<()> {
    let Some(node) = all_rasters(raw).into_iter().find(|node| node["id"] == id) else {
        return Err(bad(
            "not-found",
            format!("raster layer {id} was not found; list nodes with `pentool tree DOCUMENT --kind raster`"),
        ));
    };
    let engine = node_engine(node)?;
    if engine > ENGINE {
        bail!("[unsupported-capability] raster {id} was written by raster engine {engine}; this build implements engine {ENGINE}. It can be viewed and exported, but editing needs a newer pentool")
    }
    crate::composite::ensure_unlocked(page_value(raw, page)?, id)
}

/// Replace the checkpoint with the node's current tiles and empty the journal. Used
/// when the journal is full and after operations whose result is not replayable.
pub(crate) fn roll_checkpoint(node: &mut Value, tile_map_sha256: &str) {
    let entries = node["journal"].as_array().map_or(0, Vec::len) as u64;
    let dropped = node["checkpoint"]["journal_dropped"].as_u64().unwrap_or(0);
    node["checkpoint"] = json!({
        "engine": ENGINE,
        "tiles": node["tiles"].clone(),
        "tile_map_sha256": tile_map_sha256,
        "journal_offset": 0,
        "journal_dropped": dropped + entries,
    });
    node["journal"] = json!([]);
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
            "engine": ENGINE,
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
        roll_checkpoint(node, &hash);
    } else {
        node["checkpoint"] = json!({
            "engine": ENGINE,
            "tiles": node["tiles"].clone(),
            "tile_map_sha256": hash,
            "journal_offset": entries,
            "journal_dropped": dropped,
        });
    }
    // Compaction can unpin digests only the dropped journal recorded (brush tips).
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": id,
        "tile_map_sha256": hash,
        "journal_compacted": if compact { entries } else { 0 },
        "tiles_released": released,
    }))
}

pub struct StrokeRequest {
    pub brush: Brush,
    pub samples: Vec<Sample>,
    pub color: [u8; 3],
    pub blend: Blend,
    pub seed: u64,
    pub clone: Option<CloneRequest>,
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
    let mut request = request;
    let tip = resolve_tip(&next, &mut request.brush)?;
    let stroke = Stroke {
        brush: request.brush.clone(),
        samples: request.samples.clone(),
        color: request.color,
        blend: request.blend,
        seed: request.seed,
        tip,
        clone: match request.clone.clone() {
            Some(clone) => {
                let dab_list = dabs(&request.brush, &request.samples, request.seed)?;
                Some(CloneSpec::build(&next, id, clone, &dab_list)?)
            }
            None => None,
        },
    };
    let pinned = selection::pin(&next, id, surface.width, surface.height)?;
    let before = surface.tile_map_hash();
    let result = apply_stroke_selected(
        &mut surface,
        &stroke,
        pinned.as_ref().map(|(selection, _)| selection),
    )?;
    let after = surface.tile_map_hash();
    let canonical = json!({
        "brush": request.brush.to_json(),
        "samples": request.samples.iter().map(sample_json).collect::<Vec<_>>(),
        "color": format!("#{:02X}{:02X}{:02X}", request.color[0], request.color[1], request.color[2]),
        "blend": request.blend.name(),
        "seed": request.seed,
    });
    let mut canonical = canonical;
    if let Some(spec) = &stroke.clone {
        canonical["clone"] = spec.to_json();
        if stroke.blend == Blend::Tool(Tool::Heal) {
            canonical["heal"] = json!({"algorithm": heal::ALGORITHM});
        }
    }
    if let Some((_, entry)) = &pinned {
        canonical["selection"] = entry.clone();
    }
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
    node["journal"]
        .as_array_mut()
        .context("journal missing")?
        .push(entry);
    let mut node_copy = node.clone();
    surface.store(&mut next, &mut node_copy)?;
    let rolled = node_copy["journal"].as_array().map_or(0, Vec::len) >= MAX_JOURNAL;
    if rolled {
        roll_checkpoint(&mut node_copy, &after);
    }
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
        "checkpoint_rolled": rolled,
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
            tip: None,
            clone: None,
        }
    }

    #[test]
    fn replaying_a_stroke_reproduces_identical_tiles() {
        for kind in BRUSH_KINDS.into_iter().filter(|k| *k != "textured") {
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

    /// A tip whose left half (tip x < 128) is fully covered.
    fn left_half_tip() -> TipMask {
        (0..TILE * TILE)
            .map(|i| if i % TILE < 128 { 255 } else { 0 })
            .collect()
    }

    #[test]
    fn textured_tip_shapes_the_dab_and_follows_angle() {
        let mut request = stroke("hard-round");
        request.brush =
            Brush::parse(&json!({"kind":"textured","tip":"sha256:x","size":40})).unwrap();
        request.samples = parse_samples(&json!([[100, 100]])).unwrap();
        let mut s = surface();
        assert!(apply_stroke(&mut s, &request).is_err(), "tip not loaded");
        request.tip = Some(left_half_tip());
        apply_stroke(&mut s, &request).unwrap();
        assert_eq!(s.pixel(90, 100)[3], 255);
        assert_eq!(s.pixel(110, 100)[3], 0);
        // Rotating the tip 180 degrees mirrors the covered half.
        request.brush =
            Brush::parse(&json!({"kind":"textured","tip":"sha256:x","size":40,"angle":180}))
                .unwrap();
        let mut r = surface();
        apply_stroke(&mut r, &request).unwrap();
        assert_eq!(r.pixel(90, 100)[3], 0);
        assert_eq!(r.pixel(110, 100)[3], 255);
        let mut again = surface();
        apply_stroke(&mut again, &request).unwrap();
        assert_eq!(r.tile_map_hash(), again.tile_map_hash());
    }

    #[test]
    fn buildup_false_caps_coverage_at_one_dab() {
        let paint = |buildup: bool| {
            let mut request = stroke("hard-round");
            request.brush =
                Brush::parse(&json!({"size":20,"flow":0.3,"spacing":0.05,"buildup":buildup}))
                    .unwrap();
            request.samples = parse_samples(&json!([[100, 100], [140, 100]])).unwrap();
            let mut s = surface();
            apply_stroke(&mut s, &request).unwrap();
            s.pixel(120, 100)[3]
        };
        assert_eq!(paint(false), 77);
        assert!(paint(true) > 200);
    }

    #[test]
    fn smoothing_damps_jitter_and_still_reaches_the_end() {
        let paint = |smoothing: f64| {
            let mut request = stroke("hard-round");
            request.brush = Brush::parse(&json!({"size":2,"smoothing":smoothing})).unwrap();
            request.samples = parse_samples(&json!([
                [20, 100],
                [40, 140],
                [60, 60],
                [80, 140],
                [100, 60],
                [120, 140],
                [200, 100]
            ]))
            .unwrap();
            let mut s = surface();
            apply_stroke(&mut s, &request).unwrap();
            s
        };
        let (raw, smooth) = (paint(0.0), paint(0.8));
        let extent = |s: &Surface| {
            let rows: Vec<u32> = (0..300)
                .filter(|y| (0..600).any(|x| s.pixel(x, *y)[3] != 0))
                .collect();
            rows.last().unwrap() - rows.first().unwrap()
        };
        assert!(extent(&smooth) * 2 < extent(&raw));
        assert_ne!(smooth.pixel(200, 100)[3], 0, "ends at the last raw sample");
        assert!(Brush::parse(&json!({"smoothing":1.0})).is_err());
    }

    #[test]
    fn default_brush_json_is_unchanged_by_new_properties() {
        let brush = Brush::parse(&json!({})).unwrap();
        let json = brush.to_json();
        for key in ["smoothing", "buildup", "tip"] {
            assert!(json.get(key).is_none(), "{key}");
        }
        assert_eq!(Brush::parse(&json).unwrap(), brush);
        assert!(Brush::parse(&json!({"kind":"textured"})).is_err());
        assert!(Brush::parse(&json!({"tip":"sha256:x"})).is_err());
    }

    fn document_with_layer() -> Value {
        let mut raw: Value =
            serde_json::from_str(include_str!("../../docs/fixtures/v4-scene.pen")).unwrap();
        add(&mut raw, None, None, "p", 0.0, 0.0, 300, 40).unwrap();
        raw
    }

    fn dot(x: f64) -> StrokeRequest {
        StrokeRequest {
            brush: Brush::parse(&json!({"size":3})).unwrap(),
            samples: parse_samples(&json!([[x, 20]])).unwrap(),
            color: [x as u8, 40, 200],
            blend: Blend::Normal,
            seed: 0,
            clone: None,
        }
    }

    fn node(raw: &Value) -> Value {
        all_rasters(raw)[0].clone()
    }

    #[test]
    fn full_journal_rolls_into_a_checkpoint_that_still_replays() {
        let mut raw = document_with_layer();
        let mut rolled_at = None;
        for index in 0..(MAX_JOURNAL + 3) {
            let result = paint(&mut raw, None, "p", dot(index as f64)).unwrap();
            if result["checkpoint_rolled"] == true {
                rolled_at.get_or_insert(index);
            }
        }
        assert_eq!(rolled_at, Some(MAX_JOURNAL - 1));
        let n = node(&raw);
        assert_eq!(n["journal"].as_array().unwrap().len(), 3);
        assert_eq!(n["checkpoint"]["journal_dropped"], MAX_JOURNAL as u64);
        let live = Surface::load(&raw, &n).unwrap().tile_map_hash();
        assert_eq!(replay(&raw, &n).unwrap().tile_map_hash(), live);
        // Stroke ids keep counting across the roll.
        assert!(n["journal"][0]["id"]
            .as_str()
            .unwrap()
            .starts_with(&format!("s{MAX_JOURNAL}-")));
    }

    #[test]
    fn non_compact_checkpoint_replays_only_the_later_entries() {
        let mut raw = document_with_layer();
        paint(&mut raw, None, "p", dot(10.0)).unwrap();
        checkpoint(&mut raw, None, "p", false).unwrap();
        paint(&mut raw, None, "p", dot(30.0)).unwrap();
        let n = node(&raw);
        assert_eq!(n["checkpoint"]["journal_offset"], 1);
        let live = Surface::load(&raw, &n).unwrap().tile_map_hash();
        assert_eq!(replay(&raw, &n).unwrap().tile_map_hash(), live);
        // Clearing the live tiles must not release tiles the checkpoint pins.
        let pinned = n["checkpoint"]["tiles"]["0,0"].as_str().unwrap().to_owned();
        let mut copy = raw.clone();
        for node in copy["pages"][0]["layers"][0]["nodes"]
            .as_array_mut()
            .unwrap()
        {
            if node["id"] == "p" {
                node["tiles"] = json!({});
            }
        }
        collect_garbage(&mut copy);
        assert!(copy["raster_tiles"].get(&pinned).is_some());
    }
}

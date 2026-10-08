//! Local adjustments (`develop.local`): up to 32 adjustments, each a mask of
//! up to 16 components and parameters that act within their own stages,
//! weighted by coverage times `amount`. Specified in "Local adjustments" in
//! `docs/photography-v1.md`.
//!
//! Coverage is computed once, after the global stage 6, from coordinates in
//! the developed, uncropped frame, so a crop never moves a mask. Exposure,
//! white balance, hue, saturation and color are applied per pixel at the
//! weighted strength. The other parameters run the global kernel on a copy and
//! blend toward it by the weight; a negative weight extrapolates away from it.
use super::adjust::{self, Context, Oklab};
use super::develop::{allowed, boolean, digest, group, number};
use super::pixels::Working;
use super::{color, detail, math};
use anyhow::{bail, Context as _, Result};
use serde_json::{json, Map, Value};
use std::path::Path;

pub const MAX_ADJUSTMENTS: usize = 32;
pub const MAX_COMPONENTS: usize = 16;
/// The long edge of a brush coverage plane.
pub const MAX_BRUSH_EDGE: u32 = 4096;
/// A brush plane spans at most this many 256-px tiles.
pub const MAX_BRUSH_TILES: usize = 256;

pub const PARAMS: [&str; 18] = [
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
    "temperature",
    "tint",
    "texture",
    "clarity",
    "dehaze",
    "hue",
    "saturation",
    "sharpness",
    "noise",
    "moire",
    "defringe",
    "color",
];

const KINDS: [&str; 7] = [
    "linear",
    "radial",
    "range-luminance",
    "range-color",
    "depth",
    "brush",
    "mask",
];

const TONE: [&str; 5] = ["contrast", "highlights", "shadows", "whites", "blacks"];

// ---------------------------------------------------------------------------
// Validation.

fn unit_point(value: Option<&Value>) -> Option<[f64; 2]> {
    let v = value?.as_array()?;
    if v.len() != 2 {
        return None;
    }
    let x = v[0].as_f64().filter(|x| (0.0..=1.0).contains(x))?;
    let y = v[1].as_f64().filter(|y| (0.0..=1.0).contains(y))?;
    Some([x, y])
}

fn point(component: &Map<String, Value>, key: &str, what: &str) -> Result<[f64; 2]> {
    match unit_point(component.get(key)) {
        Some(p) => Ok(p),
        None => bail!("[invalid-develop] {what}.{key} must be [x, y] with each in 0–1"),
    }
}

fn required(
    component: &Map<String, Value>,
    key: &str,
    range: (f64, f64),
    what: &str,
) -> Result<f64> {
    match number(component, key, range, what)? {
        Some(v) => Ok(v),
        None => bail!("[invalid-develop] {what}.{key} is required"),
    }
}

/// The brush plane size of a frame: the frame scaled to a long edge of at
/// most 4096 pixels.
pub fn brush_size(frame: (usize, usize)) -> (u32, u32) {
    let long = frame.0.max(frame.1).max(1) as f64;
    let scale = (f64::from(MAX_BRUSH_EDGE) / long).min(1.0);
    let side = |v: usize| ((v as f64 * scale + 0.5) as u32).clamp(1, MAX_BRUSH_EDGE);
    (side(frame.0), side(frame.1))
}

fn validate_component(
    value: &Value,
    index: usize,
    document: Option<&Value>,
    what: &str,
) -> Result<()> {
    let what = format!("{what}.mask.components[{index}]");
    let component = group(value, &what)?;
    let kind = component.get("kind").and_then(Value::as_str).unwrap_or("");
    if !KINDS.contains(&kind) {
        bail!(
            "[invalid-develop] {what}.kind must be one of {}",
            KINDS.join(", ")
        )
    }
    match component.get("mode").and_then(Value::as_str) {
        Some("add") => {}
        Some("subtract" | "intersect") if index > 0 => {}
        Some("subtract" | "intersect") => {
            bail!("[invalid-develop] {what}.mode must be add for the first component")
        }
        _ => bail!("[invalid-develop] {what}.mode must be add, subtract or intersect"),
    }
    boolean(component, "invert", &what)?;
    let common = ["kind", "mode", "invert"];
    let keys = |extra: &[&str]| allowed(component, &[&common[..], extra].concat(), &what);
    match kind {
        "linear" => {
            keys(&["start", "end"])?;
            let (start, end) = (
                point(component, "start", &what)?,
                point(component, "end", &what)?,
            );
            if start == end {
                bail!("[invalid-develop] {what} start and end must differ")
            }
        }
        "radial" => {
            keys(&["center", "radius", "angle", "feather", "inside"])?;
            point(component, "center", &what)?;
            let radius = component.get("radius").and_then(Value::as_array);
            let valid = radius.is_some_and(|r| {
                r.len() == 2
                    && r.iter()
                        .all(|v| v.as_f64().is_some_and(|v| v > 0.0 && v <= 4.0))
            });
            if !valid {
                bail!("[invalid-develop] {what}.radius must be [rx, ry] with each in (0, 4]")
            }
            number(component, "angle", (-360.0, 360.0), &what)?;
            number(component, "feather", (0.0, 100.0), &what)?;
            boolean(component, "inside", &what)?;
        }
        "range-luminance" => {
            keys(&["min", "max", "smoothness"])?;
            let min = required(component, "min", (0.0, 1.0), &what)?;
            let max = required(component, "max", (0.0, 1.0), &what)?;
            number(component, "smoothness", (0.0, 1.0), &what)?;
            if min > max {
                bail!("[invalid-develop] {what}.min must not exceed max")
            }
        }
        "range-color" => {
            keys(&["colors", "amount"])?;
            let colors = component.get("colors").and_then(Value::as_array);
            let valid = colors.is_some_and(|c| {
                (1..=5).contains(&c.len())
                    && c.iter().all(|rgb| {
                        rgb.as_array().is_some_and(|rgb| {
                            rgb.len() == 3
                                && rgb.iter().all(|v| {
                                    v.as_f64().is_some_and(|v| (-1.0e6..=1.0e6).contains(&v))
                                })
                        })
                    })
            });
            if !valid {
                bail!("[invalid-develop] {what}.colors must hold 1–5 working-space [r, g, b] samples")
            }
            number(component, "amount", (0.0, 100.0), &what)?;
        }
        "depth" => bail!("[unsupported-capability] {what} is a depth mask; this build pairs no depth maps with photo sources, so use a range, gradient, brush or mask component"),
        "brush" => {
            keys(&["width", "height", "tiles"])?;
            let side = |key: &str| -> Result<u32> {
                component
                    .get(key)
                    .and_then(Value::as_u64)
                    .filter(|v| (1..=u64::from(MAX_BRUSH_EDGE)).contains(v))
                    .map(|v| v as u32)
                    .with_context(|| {
                        format!("[invalid-develop] {what}.{key} must be an integer in 1–{MAX_BRUSH_EDGE}")
                    })
            };
            let (width, height) = (side("width")?, side("height")?);
            let tiles = component
                .get("tiles")
                .and_then(Value::as_object)
                .with_context(|| format!("[invalid-develop] {what}.tiles must be an object"))?;
            if tiles.len() > MAX_BRUSH_TILES {
                bail!("[limit-exceeded] {what} has more than {MAX_BRUSH_TILES} tiles")
            }
            let columns = width.div_ceil(crate::raster::TILE as u32);
            let rows = height.div_ceil(crate::raster::TILE as u32);
            for (key, value) in tiles {
                let at = key
                    .split_once(',')
                    .and_then(|(x, y)| Some((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?)));
                if !at.is_some_and(|(x, y)| x < columns && y < rows) {
                    bail!("[invalid-develop] {what} tile {key:?} is outside the {width}x{height} plane")
                }
                let tile = value.as_str().filter(|d| digest(d)).with_context(|| {
                    format!("[invalid-develop] {what} tile {key} must be a sha256 digest")
                })?;
                if let Some(raw) = document {
                    if raw
                        .get("raster_tiles")
                        .and_then(|store| store.get(tile))
                        .is_none()
                    {
                        bail!("[missing-resource] {what} tile {key} references {tile}, which is not in raster_tiles")
                    }
                }
            }
        }
        "mask" => {
            keys(&["resource", "provenance"])?;
            let resource = component
                .get("resource")
                .and_then(Value::as_str)
                .filter(|d| digest(d))
                .with_context(|| format!("[invalid-develop] {what}.resource must be a sha256 digest"))?;
            if component.get("provenance").is_some_and(|p| !p.is_object()) {
                bail!("[invalid-develop] {what}.provenance must be an object")
            }
            if let Some(raw) = document {
                let Some(entry) = raw.get("mask_resources").and_then(|m| m.get(resource)) else {
                    bail!("[missing-resource] {what} references mask resource {resource}, which is not in mask_resources")
                };
                if entry["kind"] != "raster" {
                    bail!("[unsupported-capability] {what} references a {} mask resource; photo masks use raster mask resources", entry["kind"])
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn validate_params(value: &Value, what: &str) -> Result<()> {
    let what = format!("{what}.params");
    let params = group(value, &what)?;
    if params.is_empty() {
        bail!("[invalid-develop] {what} must set at least one parameter")
    }
    allowed(params, &PARAMS, &what)?;
    number(params, "exposure", (-5.0, 5.0), &what)?;
    number(params, "hue", (-180.0, 180.0), &what)?;
    for key in PARAMS {
        if !["exposure", "hue", "color"].contains(&key) {
            number(params, key, (-100.0, 100.0), &what)?;
        }
    }
    if let Some(color) = params.get("color") {
        let where_ = format!("{what}.color");
        let color = group(color, &where_)?;
        allowed(color, &["hue", "saturation"], &where_)?;
        required(color, "hue", (0.0, 360.0), &where_)?;
        required(color, "saturation", (0.0, 100.0), &where_)?;
    }
    Ok(())
}

/// Validate `develop.local`. With `document`, brush tiles and mask resources
/// must exist in it.
pub fn validate(local: &Value, document: Option<&Value>, what: &str) -> Result<()> {
    let what = format!("{what} local");
    let list = local
        .as_array()
        .with_context(|| format!("[invalid-develop] {what} must be an array"))?;
    if list.len() > MAX_ADJUSTMENTS {
        bail!("[limit-exceeded] {what} has more than {MAX_ADJUSTMENTS} adjustments")
    }
    let mut seen = std::collections::HashSet::new();
    for adjustment in list {
        let object = group(adjustment, &what)?;
        let id = object.get("id").and_then(Value::as_str).unwrap_or("");
        if !super::catalog::is_id(id) {
            bail!("[invalid-develop] {what} adjustment id {id:?} must be 1–128 letters, digits, '-', '_' or '.'")
        }
        if !seen.insert(id) {
            bail!("[invalid-develop] {what} adjustment id {id} is used more than once")
        }
        let what = format!("{what}[{id}]");
        allowed(
            object,
            &["id", "name", "enabled", "amount", "mask", "params"],
            &what,
        )?;
        if object
            .get("name")
            .is_some_and(|n| n.as_str().is_none_or(|n| n.chars().count() > 256))
        {
            bail!("[invalid-develop] {what}.name must be a string of at most 256 characters")
        }
        boolean(object, "enabled", &what)?;
        number(object, "amount", (0.0, 1.0), &what)?;
        let mask = object
            .get("mask")
            .with_context(|| format!("[invalid-develop] {what}.mask is required"))?;
        let mask = group(mask, &format!("{what}.mask"))?;
        allowed(mask, &["components"], &format!("{what}.mask"))?;
        let components = mask
            .get("components")
            .and_then(Value::as_array)
            .filter(|c| (1..=MAX_COMPONENTS).contains(&c.len()))
            .with_context(|| {
                format!("[invalid-develop] {what}.mask.components must hold 1–{MAX_COMPONENTS} components")
            })?;
        for (index, component) in components.iter().enumerate() {
            validate_component(component, index, document, &what)?;
        }
        let params = object
            .get("params")
            .with_context(|| format!("[invalid-develop] {what}.params is required"))?;
        validate_params(params, &what)?;
    }
    Ok(())
}

/// Whether any enabled local adjustment sets a non-zero `dehaze`, which needs
/// the stored `presence.dehaze_airlight`.
pub fn uses_dehaze(develop: &Value) -> bool {
    develop["local"].as_array().is_some_and(|list| {
        list.iter().any(|a| {
            a["enabled"] != false && a["params"]["dehaze"].as_f64().is_some_and(|v| v != 0.0)
        })
    })
}

// ---------------------------------------------------------------------------
// Rendering.

/// Where brush tiles and mask resources are read from.
pub struct Lookup<'a> {
    pub raw: &'a Value,
    pub document: &'a Path,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Add,
    Subtract,
    Intersect,
}

/// An 8-bit coverage plane stretched over the frame.
struct Plane {
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl Plane {
    /// Bilinear coverage at frame fraction `(u, v)`.
    fn sample(&self, u: f64, v: f64) -> f64 {
        let x = (u * self.width as f64 - 0.5).clamp(0.0, (self.width - 1) as f64);
        let y = (v * self.height as f64 - 0.5).clamp(0.0, (self.height - 1) as f64);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let at = |x: usize, y: usize| f64::from(self.data[y * self.width + x]) / 255.0;
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
        let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
        top + (bottom - top) * fy
    }
}

enum Shape {
    Linear {
        start: [f64; 2],
        end: [f64; 2],
    },
    Radial {
        center: [f64; 2],
        radius: [f64; 2],
        sin_cos: (f64, f64),
        feather: f64,
        inside: bool,
    },
    Luminance {
        min: f64,
        max: f64,
        smoothness: f64,
    },
    Color {
        colors: Vec<[f64; 3]>,
        threshold: f64,
    },
    Plane(Plane),
}

struct Component {
    shape: Shape,
    mode: Mode,
    invert: bool,
}

#[derive(Default)]
struct Params {
    exposure: f64,
    temperature: f64,
    tint: f64,
    tone: [f64; 5],
    texture: f64,
    clarity: f64,
    dehaze: f64,
    hue: f64,
    saturation: f64,
    color: Option<(f64, f64)>,
    sharpness: f64,
    noise: f64,
    moire: f64,
    defringe: f64,
}

struct Adjustment {
    id: String,
    amount: f64,
    components: Vec<Component>,
    params: Params,
    /// Coverage times amount per output pixel, once computed.
    weights: Vec<f32>,
}

/// The enabled local adjustments of a develop object, ready to render.
pub struct Local {
    adjustments: Vec<Adjustment>,
    airlight: Option<[f64; 3]>,
    oklab: Oklab,
}

fn get(object: &Value, key: &str, default: f64) -> f64 {
    object.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn brush_plane(raw: &Value, component: &Value) -> Result<Plane> {
    let (width, height) = (
        component["width"].as_u64().unwrap_or(1) as u32,
        component["height"].as_u64().unwrap_or(1) as u32,
    );
    let surface = crate::raster::Surface::load_map(raw, width, height, &component["tiles"])?;
    let (w, h) = (width as usize, height as usize);
    let mut data = vec![0u8; w * h];
    for (&(tx, ty), _) in surface.tiles.iter() {
        let tile = crate::raster::TILE as u32;
        for y in ty * tile..((ty + 1) * tile).min(height) {
            for x in tx * tile..((tx + 1) * tile).min(width) {
                data[y as usize * w + x as usize] = surface.pixel(x, y)[3];
            }
        }
    }
    Ok(Plane {
        width: w,
        height: h,
        data,
    })
}

fn mask_plane(lookup: &Lookup, resource: &str) -> Result<Plane> {
    let entry = lookup
        .raw
        .get("mask_resources")
        .and_then(|m| m.get(resource))
        .with_context(|| {
            format!("[missing-resource] mask resource {resource} is not in mask_resources")
        })?;
    if entry["kind"] != "raster" {
        bail!("[unsupported-capability] mask resource {resource} is not a raster mask")
    }
    let asset = entry["asset"]
        .as_str()
        .context("[invalid-mask] raster mask asset missing")?;
    let bytes = crate::image::load_asset_bytes_for_document(lookup.raw, lookup.document, asset)?;
    let (_, image) = crate::image::decode_source_pixels(&bytes)?;
    let (width, height) = (image.width() as usize, image.height() as usize);
    if width == 0 || height == 0 {
        bail!("[invalid-mask] mask resource {resource} is empty")
    }
    let data = image
        .pixels()
        .map(|p| {
            let value =
                (u32::from(p[0]) * 2126 + u32::from(p[1]) * 7152 + u32::from(p[2]) * 722 + 5000)
                    / 10000;
            ((value * u32::from(p[3]) + 127) / 255) as u8
        })
        .collect();
    Ok(Plane {
        width,
        height,
        data,
    })
}

impl Local {
    /// The enabled adjustments with a non-zero amount, with brush and mask
    /// planes loaded. `None` when nothing applies.
    pub fn new(develop: &Value, lookup: Option<&Lookup>) -> Result<Option<Self>> {
        let Some(list) = develop.get("local").and_then(Value::as_array) else {
            return Ok(None);
        };
        let oklab = Oklab::new();
        let mut adjustments = Vec::new();
        for item in list {
            let amount = get(item, "amount", 1.0);
            if item["enabled"] == false || amount <= 0.0 {
                continue;
            }
            let id = item["id"].as_str().unwrap_or_default().to_owned();
            let mut components = Vec::new();
            for c in item["mask"]["components"].as_array().into_iter().flatten() {
                let mode = match c["mode"].as_str() {
                    Some("subtract") => Mode::Subtract,
                    Some("intersect") => Mode::Intersect,
                    _ => Mode::Add,
                };
                let pair = |key: &str| -> [f64; 2] {
                    let v = &c[key];
                    [v[0].as_f64().unwrap_or(0.0), v[1].as_f64().unwrap_or(0.0)]
                };
                let shape = match c["kind"].as_str().unwrap_or("") {
                    "linear" => Shape::Linear {
                        start: pair("start"),
                        end: pair("end"),
                    },
                    "radial" => Shape::Radial {
                        center: pair("center"),
                        radius: pair("radius"),
                        sin_cos: math::sin_cos(math::radians(get(c, "angle", 0.0))),
                        feather: get(c, "feather", 50.0) / 100.0,
                        inside: c["inside"] != false,
                    },
                    "range-luminance" => Shape::Luminance {
                        min: get(c, "min", 0.0),
                        max: get(c, "max", 1.0),
                        smoothness: get(c, "smoothness", 0.1),
                    },
                    "range-color" => Shape::Color {
                        colors: c["colors"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|rgb| {
                                oklab.forward(std::array::from_fn(|i| {
                                    rgb[i].as_f64().unwrap_or(0.0)
                                }))
                            })
                            .collect(),
                        threshold: 0.02 + 0.28 * get(c, "amount", 50.0) / 100.0,
                    },
                    "brush" => {
                        let lookup = lookup.with_context(|| {
                            format!("[missing-resource] local adjustment {id} has a brush mask and no document to read its tiles from")
                        })?;
                        Shape::Plane(brush_plane(lookup.raw, c)?)
                    }
                    "mask" => {
                        let lookup = lookup.with_context(|| {
                            format!("[missing-resource] local adjustment {id} has a mask resource and no document to read it from")
                        })?;
                        Shape::Plane(mask_plane(lookup, c["resource"].as_str().unwrap_or(""))?)
                    }
                    other => bail!("[unsupported-capability] local adjustment {id} mask kind {other:?} is not rendered by this build"),
                };
                components.push(Component {
                    shape,
                    mode,
                    invert: c["invert"] == true,
                });
            }
            let p = &item["params"];
            let params = Params {
                exposure: get(p, "exposure", 0.0),
                temperature: get(p, "temperature", 0.0) / 100.0,
                tint: get(p, "tint", 0.0) / 100.0,
                tone: TONE.map(|key| get(p, key, 0.0)),
                texture: get(p, "texture", 0.0),
                clarity: get(p, "clarity", 0.0),
                dehaze: get(p, "dehaze", 0.0),
                hue: get(p, "hue", 0.0),
                saturation: get(p, "saturation", 0.0) / 100.0,
                color: p
                    .get("color")
                    .map(|c| (get(c, "hue", 0.0), get(c, "saturation", 0.0) / 100.0)),
                sharpness: get(p, "sharpness", 0.0) / 100.0,
                noise: get(p, "noise", 0.0) / 100.0,
                moire: get(p, "moire", 0.0) / 100.0,
                defringe: get(p, "defringe", 0.0) / 100.0,
            };
            adjustments.push(Adjustment {
                id,
                amount,
                components,
                params,
                weights: Vec::new(),
            });
        }
        if adjustments.is_empty() {
            return Ok(None);
        }
        let airlight = if adjustments.iter().any(|a| a.params.dehaze != 0.0) {
            let stored = develop["presence"]["dehaze_airlight"].as_array();
            let a: Vec<f64> = stored
                .into_iter()
                .flatten()
                .filter_map(Value::as_f64)
                .collect();
            if a.len() != 3 {
                bail!("[invalid-develop] a local dehaze needs presence.dehaze_airlight; run `raw develop` on the variant to resolve it")
            }
            Some([a[0], a[1], a[2]])
        } else {
            None
        };
        Ok(Some(Self {
            adjustments,
            airlight,
            oklab,
        }))
    }

    /// Compute every adjustment's weights on the image after the global
    /// stage 6.
    pub fn prepare(&mut self, image: &Working, context: &Context) -> Result<()> {
        let width = image.width as usize;
        let n = width * image.height as usize;
        let (fw, fh) = (context.frame.0.max(1) as f64, context.frame.1.max(1) as f64);
        let long = fw.max(fh);
        let oklab = &self.oklab;
        for adjustment in &mut self.adjustments {
            let mut weights = vec![0.0f32; n];
            let components = &adjustment.components;
            let amount = adjustment.amount;
            let ranged = components
                .iter()
                .any(|c| matches!(c.shape, Shape::Luminance { .. } | Shape::Color { .. }));
            adjust::rows(&mut weights, width, &|y, row| {
                let py = (context.origin.1 + y) as f64 + 0.5;
                for (x, out) in row.iter_mut().enumerate() {
                    let px = (context.origin.0 + x) as f64 + 0.5;
                    let i = y * width + x;
                    let lab = if ranged {
                        oklab.forward(adjust::pixel(&image.rgb[i * 3..i * 3 + 3]))
                    } else {
                        [0.0; 3]
                    };
                    let mut total = 0.0;
                    for c in components {
                        let mut v = coverage(&c.shape, px, py, fw, fh, long, lab);
                        if c.invert {
                            v = 1.0 - v;
                        }
                        total = match c.mode {
                            Mode::Add => total + v - total * v,
                            Mode::Subtract => total * (1.0 - v),
                            Mode::Intersect => total * v,
                        };
                    }
                    *out = (total * amount) as f32;
                }
            })?;
            adjustment.weights = weights;
        }
        Ok(())
    }

    /// Stage 6: exposure, temperature and tint per pixel, then the tone curve
    /// sliders.
    pub fn tone(&self, image: &mut Working, context: &Context) -> Result<()> {
        let lw = adjust::luminance_weights();
        for a in &self.adjustments {
            let p = &a.params;
            if p.exposure != 0.0 || p.temperature != 0.0 || p.tint != 0.0 {
                let weights = &a.weights;
                adjust::pixels(image, &|i, px, _| {
                    let w = f64::from(weights[i]);
                    if w == 0.0 {
                        return;
                    }
                    let gains = [
                        math::exp2(w * 0.3 * p.temperature),
                        math::exp2(-w * 0.3 * p.tint),
                        math::exp2(-w * 0.3 * p.temperature),
                    ];
                    let k = math::exp2(w * p.exposure) / adjust::luminance(&lw, gains);
                    let rgb = adjust::pixel(px);
                    adjust::store(px, std::array::from_fn(|c| rgb[c] * gains[c] * k));
                })?;
            }
            if p.tone.iter().any(|v| *v != 0.0) {
                let mut tone = json!({});
                for (key, v) in TONE.iter().zip(p.tone) {
                    tone[*key] = json!(v);
                }
                let neutral = Context {
                    baseline_exposure: 0.0,
                    ..*context
                };
                let mut copy = image.clone();
                adjust::tone(&mut copy, &json!({ "tone": tone }), &neutral)?;
                blend(image, &copy, &a.weights, 1.0)?;
            }
        }
        Ok(())
    }

    /// Stage 7: texture, clarity and dehaze.
    pub fn presence(&self, image: &mut Working, context: &Context) -> Result<()> {
        for a in &self.adjustments {
            let p = &a.params;
            for (amount, kind) in [(p.dehaze, 0), (p.clarity, 1), (p.texture, 2)] {
                if amount == 0.0 {
                    continue;
                }
                let mut copy = image.clone();
                let mut sliders = [0.0; 3];
                sliders[kind] = 1.0f64.copysign(amount);
                adjust::presence_with(&mut copy, sliders, self.airlight, context)?;
                blend(image, &copy, &a.weights, amount.abs() / 100.0)?;
            }
        }
        Ok(())
    }

    /// Stage 9: hue, saturation and color tint in Oklab.
    pub fn color(&self, image: &mut Working) -> Result<()> {
        let oklab = &self.oklab;
        for a in &self.adjustments {
            let p = &a.params;
            if p.hue == 0.0 && p.saturation == 0.0 && p.color.is_none() {
                continue;
            }
            let tint = p.color.map(|(hue, saturation)| {
                let (sin, cos) = math::sin_cos(math::radians(hue));
                (0.06 * saturation * cos, 0.06 * saturation * sin)
            });
            let weights = &a.weights;
            adjust::pixels(image, &|i, px, _| {
                let w = f64::from(weights[i]);
                if w == 0.0 {
                    return;
                }
                let [l, mut ca, mut cb] = oklab.forward(adjust::pixel(px));
                if p.hue != 0.0 {
                    let (sin, cos) = math::sin_cos(math::radians(w * p.hue));
                    (ca, cb) = (ca * cos - cb * sin, ca * sin + cb * cos);
                }
                let scale = (1.0 + w * p.saturation).max(0.0);
                ca *= scale;
                cb *= scale;
                if let Some((ta, tb)) = tint {
                    ca += w * ta;
                    cb += w * tb;
                }
                adjust::store(px, oklab.back([l, ca, cb]));
            })?;
        }
        Ok(())
    }

    /// After capture sharpening: sharpness, then noise, moiré and defringe.
    pub fn detail(&self, image: &mut Working) -> Result<()> {
        let (w, h) = (image.width as usize, image.height as usize);
        for a in &self.adjustments {
            let p = &a.params;
            if p.sharpness > 0.0 {
                let mut copy = image.clone();
                let develop =
                    json!({"detail": {"sharpening": {"amount": 150, "radius": 1, "detail": 25}}});
                detail::sharpen(&mut copy, &develop)?;
                blend(image, &copy, &a.weights, p.sharpness)?;
            } else if p.sharpness < 0.0 {
                let mut copy = image.clone();
                let mut planes: [Vec<f32>; 3] =
                    std::array::from_fn(|c| copy.rgb.iter().skip(c).step_by(3).copied().collect());
                for plane in &mut planes {
                    adjust::gaussian_weighted(plane, copy.alpha.as_deref(), w, h, 1.5)?;
                }
                for (i, px) in copy.rgb.chunks_exact_mut(3).enumerate() {
                    for c in 0..3 {
                        px[c] = planes[c][i];
                    }
                }
                blend(image, &copy, &a.weights, -p.sharpness)?;
            }
            let kernels = [
                (
                    p.noise,
                    json!({"detail": {"noise": {"luminance": 100, "color": 100}}}),
                ),
                (p.moire, json!({"detail": {"moire": 100}})),
                (
                    p.defringe,
                    json!({"lens": {"defringe": {"purple_amount": 20, "green_amount": 20}}}),
                ),
            ];
            for (amount, develop) in kernels {
                if amount == 0.0 {
                    continue;
                }
                if let Some(early) = detail::Early::new(&develop) {
                    let mut copy = image.clone();
                    early.apply(&mut copy.rgb, w, h, &color::identity())?;
                    blend(image, &copy, &a.weights, amount)?;
                }
            }
        }
        Ok(())
    }

    /// The adjustment IDs and their mean weights, for reports.
    pub fn report(&self) -> Value {
        Value::Array(
            self.adjustments
                .iter()
                .map(|a| {
                    let n = a.weights.len().max(1) as f64;
                    let sum: f64 = a.weights.iter().map(|w| f64::from(*w)).sum();
                    json!({"id": a.id, "coverage": adjust::round_to(sum / n, 0.0001)})
                })
                .collect(),
        )
    }
}

/// Coverage of one component at frame pixel `(px, py)`.
fn coverage(shape: &Shape, px: f64, py: f64, fw: f64, fh: f64, long: f64, lab: [f64; 3]) -> f64 {
    match shape {
        Shape::Linear { start, end } => {
            let (sx, sy) = (start[0] * fw, start[1] * fh);
            let (dx, dy) = (end[0] * fw - sx, end[1] * fh - sy);
            let length = dx * dx + dy * dy;
            let t = ((px - sx) * dx + (py - sy) * dy) / length;
            1.0 - adjust::smoothstep(t)
        }
        Shape::Radial {
            center,
            radius,
            sin_cos: (sin, cos),
            feather,
            inside,
        } => {
            let (dx, dy) = (px - center[0] * fw, py - center[1] * fh);
            let x = (dx * cos + dy * sin) / (radius[0] * long);
            let y = (-dx * sin + dy * cos) / (radius[1] * long);
            let d = (x * x + y * y).sqrt();
            let v = if *feather > 0.0 {
                adjust::smoothstep((1.0 - d) / feather)
            } else if d <= 1.0 {
                1.0
            } else {
                0.0
            };
            if *inside {
                v
            } else {
                1.0 - v
            }
        }
        Shape::Luminance {
            min,
            max,
            smoothness,
        } => {
            let l = lab[0];
            if *smoothness > 0.0 {
                adjust::smoothstep((l - (min - smoothness)) / smoothness)
                    * adjust::smoothstep((max + smoothness - l) / smoothness)
            } else if l >= *min && l <= *max {
                1.0
            } else {
                0.0
            }
        }
        Shape::Color { colors, threshold } => {
            let d = colors
                .iter()
                .map(|c| {
                    let (dl, da, db) = (lab[0] - c[0], lab[1] - c[1], lab[2] - c[2]);
                    (0.25 * dl * dl + da * da + db * db).sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            adjust::smoothstep(2.0 * (1.0 - d / threshold))
        }
        Shape::Plane(plane) => plane.sample(px / fw, py / fh),
    }
}

/// `image += weight · strength · (target - image)` on valid pixels.
fn blend(image: &mut Working, target: &Working, weights: &[f32], strength: f64) -> Result<()> {
    adjust::pixels(image, &|i, px, alpha| {
        let w = f64::from(weights[i]) * strength;
        if w == 0.0 || alpha <= 0.0 {
            return;
        }
        let rgb = adjust::pixel(px);
        let t = adjust::pixel(&target.rgb[i * 3..i * 3 + 3]);
        adjust::store(px, std::array::from_fn(|c| rgb[c] + w * (t[c] - rgb[c])));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(frame: (usize, usize)) -> Context {
        Context {
            baseline_exposure: 0.0,
            long_edge: frame.0.max(frame.1) as f64,
            origin: (0, 0),
            frame,
        }
    }

    fn gray(width: u32, height: u32, v: f32) -> Working {
        let mut image = Working::new(width, height, false).unwrap();
        image.rgb.iter_mut().for_each(|p| *p = v);
        image
    }

    fn weights(develop: Value, image: &Working) -> Vec<f32> {
        let mut local = Local::new(&develop, None).unwrap().unwrap();
        local
            .prepare(
                image,
                &context((image.width as usize, image.height as usize)),
            )
            .unwrap();
        local.adjustments.remove(0).weights
    }

    #[test]
    fn linear_ramps_from_start_to_end() {
        let image = gray(100, 10, 0.18);
        let w = weights(
            json!({"local": [{"id": "a", "mask": {"components": [
                {"kind": "linear", "mode": "add", "start": [0.2, 0.5], "end": [0.8, 0.5]}]},
                "params": {"exposure": 1}}]}),
            &image,
        );
        assert_eq!(w[5 * 100 + 10], 1.0);
        assert_eq!(w[5 * 100 + 90], 0.0);
        assert!((w[5 * 100 + 49] - 0.5).abs() < 0.05);
    }

    #[test]
    fn modes_invert_and_amount_combine() {
        let image = gray(64, 64, 0.18);
        let w = weights(
            json!({"local": [{"id": "a", "amount": 0.5, "mask": {"components": [
                {"kind": "radial", "mode": "add", "center": [0.5, 0.5], "radius": [0.25, 0.25], "feather": 0},
                {"kind": "radial", "mode": "subtract", "center": [0.5, 0.5], "radius": [0.1, 0.1], "feather": 0}]},
                "params": {"exposure": 1}}]}),
            &image,
        );
        assert_eq!(w[32 * 64 + 32], 0.0);
        assert_eq!(w[32 * 64 + 32 + 10], 0.5);
        assert_eq!(w[0], 0.0);
        let inverted = weights(
            json!({"local": [{"id": "a", "mask": {"components": [
                {"kind": "radial", "mode": "add", "invert": true, "center": [0.5, 0.5], "radius": [0.25, 0.25], "feather": 0}]},
                "params": {"exposure": 1}}]}),
            &image,
        );
        assert_eq!(inverted[0], 1.0);
        assert_eq!(inverted[32 * 64 + 32], 0.0);
    }

    #[test]
    fn exposure_is_weighted_and_white_balance_keeps_luminance() {
        let mut image = gray(4, 1, 0.18);
        let develop = json!({"local": [{"id": "a", "mask": {"components": [
            {"kind": "linear", "mode": "add", "start": [0.0, 0.5], "end": [1.0, 0.5]}]},
            "params": {"exposure": 1, "temperature": 50}}]});
        let mut local = Local::new(&develop, None).unwrap().unwrap();
        local.prepare(&image, &context((4, 1))).unwrap();
        local.tone(&mut image, &context((4, 1))).unwrap();
        let lw = adjust::luminance_weights();
        let y0 = adjust::luminance(&lw, adjust::pixel(&image.rgb[0..3]));
        let w0 = f64::from(local.adjustments[0].weights[0]);
        assert!((y0 - 0.18 * math::exp2(w0)).abs() < 1.0e-5);
        assert!(image.rgb[0] > image.rgb[2], "warmer");
    }
}

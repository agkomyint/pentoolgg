//! Output recipes and batch delivery (`photo export`): recipe validation and the
//! built-in recipes, Lanczos-3 resizing in linear light, output sharpening,
//! JPEG, PNG and TIFF encoding with metadata, and a planner whose outputs are
//! staged and only renamed into place once every one has succeeded.
use super::jpeg::{self, Chroma};
use super::metadata::{self, Embedded, Field, Policy};
use super::output::{self, Intent, Output};
use super::pixels::{self, Depth, Dither, Raster, Samples, Working};
use super::{catalog, math, png};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Outputs one `photo export` may write.
pub const MAX_OUTPUTS: usize = 10_000;
/// Recipes every document has; a document recipe may not reuse these names.
pub const BUILT_IN: [&str; 4] = ["web-gallery", "social", "archive-master", "photo-lab"];
const KEYS: [&str; 14] = [
    "format",
    "quality",
    "chroma",
    "bit_depth",
    "compression",
    "hdr",
    "color_space",
    "intent",
    "dither",
    "resize",
    "ppi",
    "sharpen",
    "naming",
    "metadata",
];
const MAX_NAMING: usize = 256;
const MAX_FILE_NAME: usize = 200;
const DEFAULT_NAMING: &str = "{photo}-{variant}";
const RESERVED_FILE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The JSON of a built-in recipe.
fn built_in(name: &str) -> Option<Value> {
    Some(match name {
        "web-gallery" => json!({
            "format": "jpeg", "quality": 85, "chroma": "420", "bit_depth": 8,
            "color_space": "srgb", "intent": "perceptual",
            "resize": {"mode": "long-edge", "value": 2048, "enlarge": false}, "ppi": 72,
            "sharpen": {"target": "screen", "amount": "standard"},
            "naming": DEFAULT_NAMING, "metadata": {"policy": "copyright"}
        }),
        "social" => json!({
            "format": "jpeg", "quality": 90, "chroma": "420", "bit_depth": 8,
            "color_space": "srgb", "intent": "perceptual",
            "resize": {"mode": "long-edge", "value": 1080, "enlarge": false}, "ppi": 72,
            "sharpen": {"target": "screen", "amount": "standard"},
            "naming": DEFAULT_NAMING, "metadata": {"policy": "none"}
        }),
        "archive-master" => json!({
            "format": "tiff", "bit_depth": 16, "compression": "deflate",
            "color_space": "prophoto", "intent": "relative-colorimetric",
            "resize": {"mode": "none"}, "ppi": 300,
            "naming": DEFAULT_NAMING, "metadata": {"policy": "public"}
        }),
        "photo-lab" => json!({
            "format": "jpeg", "quality": 95, "chroma": "444", "bit_depth": 8,
            "color_space": "srgb", "intent": "perceptual",
            "resize": {"mode": "print", "ppi": 300, "fit": "error"}, "ppi": 300,
            "sharpen": {"target": "glossy", "amount": "standard"},
            "naming": DEFAULT_NAMING, "metadata": {"policy": "copyright"}
        }),
        _ => return None,
    })
}

/// File format and its codec settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Format {
    Jpeg { quality: u8, chroma: Chroma },
    Png,
    Tiff { deflate: bool },
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Self::Jpeg { .. } => "jpeg",
            Self::Png => "png",
            Self::Tiff { .. } => "tiff",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg { .. } => "jpg",
            Self::Png => "png",
            Self::Tiff { .. } => "tif",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Long,
    Short,
    Width,
    Height,
}

/// What a print does when the photo's aspect differs from the paper's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Error,
    Crop,
    Pad,
}

impl Fit {
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "error" => Ok(Self::Error),
            "crop" => Ok(Self::Crop),
            "pad" => Ok(Self::Pad),
            _ => bail!("[invalid-input] fit {name:?} must be error, crop or pad"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Crop => "crop",
            Self::Pad => "pad",
        }
    }
}

/// A print size; `width` and `height` may come from the run (`--print`).
#[derive(Clone, Debug, PartialEq)]
pub struct Print {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub centimeters: bool,
    pub ppi: f64,
    pub fit: Fit,
    pub enlarge: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Resize {
    None,
    Edge {
        edge: Edge,
        value: u32,
        enlarge: bool,
    },
    Megapixels {
        value: f64,
        enlarge: bool,
    },
    Percent {
        value: f64,
        enlarge: bool,
    },
    Print(Print),
}

/// A validated recipe.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub name: String,
    pub built_in: bool,
    pub format: Format,
    pub depth: u8,
    pub color_space: String,
    pub intent: Intent,
    pub hdr: Option<(String, f64)>,
    pub dither: Dither,
    pub resize: Resize,
    pub ppi: f64,
    /// Target (`screen`, `matte`, `glossy`) and amount (`low`, `standard`, `high`).
    pub sharpen: Option<(&'static str, &'static str)>,
    pub naming: String,
    pub metadata: Policy,
}

fn pick(value: &str, choices: &[&'static str], what: &str) -> Result<&'static str> {
    choices
        .iter()
        .find(|c| **c == value)
        .copied()
        .with_context(|| {
            format!(
                "[invalid-input] {what} {value:?} must be one of {}",
                choices.join(", ")
            )
        })
}

fn only(object: &Map<String, Value>, keys: &[&str], what: &str) -> Result<()> {
    match object.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(key) => bail!(
            "[invalid-input] {what} has unknown field {key:?}; its fields are {}",
            keys.join(", ")
        ),
        None => Ok(()),
    }
}

fn number(value: Option<&Value>, what: &str, min_exclusive: f64, max: f64) -> Result<Option<f64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value.as_f64() {
        Some(v) if v > min_exclusive && v <= max => Ok(Some(v)),
        _ => {
            bail!("[invalid-input] {what} must be a number above {min_exclusive} and at most {max}")
        }
    }
}

fn parse_resize(value: Option<&Value>, what: &str) -> Result<Resize> {
    let Some(value) = value else {
        return Ok(Resize::None);
    };
    let what = format!("{what}.resize");
    let object = value
        .as_object()
        .with_context(|| format!("[invalid-input] {what} must be an object"))?;
    let mode = object["mode"]
        .as_str()
        .with_context(|| format!("[invalid-input] {what} requires mode"))?;
    let modes = [
        "none",
        "long-edge",
        "short-edge",
        "width",
        "height",
        "megapixels",
        "percent",
        "print",
    ];
    let mode = pick(mode, &modes, &format!("{what}.mode"))?;
    let keys: &[&str] = match mode {
        "none" => &["mode"],
        "print" => &["mode", "width", "height", "unit", "ppi", "fit", "enlarge"],
        _ => &["mode", "value", "enlarge"],
    };
    only(object, keys, &what)?;
    let enlarge = match object.get("enlarge") {
        None => None,
        Some(v) => Some(
            v.as_bool()
                .with_context(|| format!("[invalid-input] {what}.enlarge must be true or false"))?,
        ),
    };
    let value = || {
        object
            .get("value")
            .with_context(|| format!("[invalid-input] {what} mode {mode} requires value"))
    };
    Ok(match mode {
        "none" => Resize::None,
        "long-edge" | "short-edge" | "width" | "height" => {
            let edge = match mode {
                "long-edge" => Edge::Long,
                "short-edge" => Edge::Short,
                "width" => Edge::Width,
                _ => Edge::Height,
            };
            let value = value()?
                .as_u64()
                .filter(|v| (1..=u64::from(pixels::MAX_PHOTO_DIMENSION)).contains(v))
                .with_context(|| {
                    format!(
                        "[invalid-input] {what}.value must be a whole number of pixels, 1–{}",
                        pixels::MAX_PHOTO_DIMENSION
                    )
                })? as u32;
            Resize::Edge {
                edge,
                value,
                enlarge: enlarge.unwrap_or(false),
            }
        }
        "megapixels" => Resize::Megapixels {
            value: number(Some(value()?), &format!("{what}.value"), 0.0, 120.0)?.unwrap(),
            enlarge: enlarge.unwrap_or(false),
        },
        "percent" => Resize::Percent {
            value: number(Some(value()?), &format!("{what}.value"), 0.0, 400.0)?.unwrap(),
            enlarge: enlarge.unwrap_or(false),
        },
        _ => {
            let unit = match object.get("unit") {
                None => "in",
                Some(v) => pick(
                    v.as_str().unwrap_or_default(),
                    &["in", "cm"],
                    &format!("{what}.unit"),
                )?,
            };
            let fit = match object.get("fit") {
                None => Fit::Error,
                Some(v) => Fit::parse(v.as_str().unwrap_or_default())?,
            };
            Resize::Print(Print {
                width: number(object.get("width"), &format!("{what}.width"), 0.0, 1000.0)?,
                height: number(object.get("height"), &format!("{what}.height"), 0.0, 1000.0)?,
                centimeters: unit == "cm",
                ppi: number(object.get("ppi"), &format!("{what}.ppi"), 0.0, 4800.0)?.unwrap_or(0.0),
                fit,
                enlarge: enlarge.unwrap_or(true),
            })
        }
    })
}

/// One part of a `naming` template.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Text(String),
    Photo,
    Variant,
    Name,
    Recipe,
    Rating,
    Seq(usize),
    Captured,
}

fn template(naming: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut rest = naming;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            tokens.push(Token::Text(rest[..open].to_string()));
        }
        let close = rest[open..]
            .find('}')
            .map(|c| open + c)
            .with_context(|| format!("[invalid-input] naming {naming:?} has an unclosed {{"))?;
        let name = &rest[open + 1..close];
        tokens.push(match name {
            "photo" => Token::Photo,
            "variant" => Token::Variant,
            "name" => Token::Name,
            "recipe" => Token::Recipe,
            "rating" => Token::Rating,
            "captured:YYYYMMDD" => Token::Captured,
            _ => match name.strip_prefix("seq:").and_then(|n| n.parse::<usize>().ok()) {
                Some(digits) if (1..=6).contains(&digits) => Token::Seq(digits),
                _ => bail!("[invalid-input] naming token {{{name}}} is not one of {{photo}}, {{variant}}, {{name}}, {{recipe}}, {{rating}}, {{seq:N}} (N 1–6), {{captured:YYYYMMDD}}"),
            },
        });
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        tokens.push(Token::Text(rest.to_string()));
    }
    Ok(tokens)
}

impl Recipe {
    /// Validate a recipe's JSON; errors name the recipe.
    pub fn parse(name: &str, value: &Value) -> Result<Self> {
        Self::parse_named(name, value).map_err(|error| {
            let text = format!("{error:#}");
            if text.contains(&format!("recipe {name}")) {
                error
            } else {
                anyhow!("{text} (in recipe {name})")
            }
        })
    }

    fn parse_named(name: &str, value: &Value) -> Result<Self> {
        let what = format!("recipe {name}");
        let object = value
            .as_object()
            .with_context(|| format!("[invalid-input] {what} must be an object"))?;
        only(object, &KEYS, &what)?;
        let text = |key: &str| -> Result<Option<&str>> {
            object
                .get(key)
                .map(|v| {
                    v.as_str()
                        .with_context(|| format!("[invalid-input] {what}.{key} must be a string"))
                })
                .transpose()
        };
        let format = text("format")?
            .with_context(|| format!("[invalid-input] {what} requires format and color_space"))?;
        let color_space = text("color_space")?
            .with_context(|| format!("[invalid-input] {what} requires format and color_space"))?
            .to_string();
        let forbid = |keys: &[&str], format: &str| -> Result<()> {
            match keys.iter().find(|k| object.contains_key(**k)) {
                Some(key) => bail!("[invalid-input] {what}: {key} does not apply to {format}"),
                None => Ok(()),
            }
        };
        let depth = match object.get("bit_depth") {
            None => None,
            Some(v) => match v.as_u64() {
                Some(8) => Some(8),
                Some(16) => Some(16),
                _ => bail!("[invalid-input] {what}.bit_depth must be 8 or 16"),
            },
        };
        let hdr = match object.get("hdr") {
            None => None,
            Some(v) => {
                let hdr = v
                    .as_object()
                    .with_context(|| format!("[invalid-input] {what}.hdr must be an object"))?;
                only(hdr, &["transfer", "headroom"], &format!("{what}.hdr"))?;
                let transfer = pick(
                    hdr.get("transfer")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    &["pq", "hlg"],
                    &format!("{what}.hdr.transfer"),
                )?;
                let headroom = hdr
                    .get("headroom")
                    .and_then(Value::as_f64)
                    .filter(|h| (0.0..=output::MAX_HEADROOM).contains(h))
                    .with_context(|| {
                        format!(
                            "[invalid-input] {what}.hdr.headroom must be 0–{} stops",
                            output::MAX_HEADROOM
                        )
                    })?;
                Some((transfer.to_string(), headroom))
            }
        };
        let quality = || -> Result<u8> {
            match object.get("quality") {
                None => Ok(90),
                Some(v) => v
                    .as_u64()
                    .filter(|q| (1..=100).contains(q))
                    .map(|q| q as u8)
                    .with_context(|| format!("[invalid-input] {what}.quality must be 1–100")),
            }
        };
        let format = match format {
            "jpeg" => {
                forbid(&["compression", "hdr"], "jpeg")?;
                if depth == Some(16) {
                    bail!("[invalid-input] {what}: JPEG is 8-bit; use png or tiff for 16 bits")
                }
                Format::Jpeg {
                    quality: quality()?,
                    chroma: Chroma::parse(text("chroma")?.unwrap_or("420"))?,
                }
            }
            "png" => {
                forbid(&["quality", "chroma", "compression"], "png")?;
                Format::Png
            }
            "tiff" => {
                forbid(&["quality", "chroma", "hdr"], "tiff")?;
                let compression = pick(
                    text("compression")?.unwrap_or("deflate"),
                    &["none", "deflate"],
                    &format!("{what}.compression"),
                )?;
                Format::Tiff {
                    deflate: compression == "deflate",
                }
            }
            other => bail!("[invalid-input] {what}.format {other:?} must be jpeg, png or tiff"),
        };
        let depth = depth.unwrap_or(if hdr.is_some() { 16 } else { 8 });
        let intent = Intent::parse(text("intent")?.unwrap_or("perceptual"))?;
        let dither = match text("dither")?.unwrap_or("none") {
            "none" => Dither::None,
            "ordered4" if depth == 8 => Dither::Ordered4,
            "ordered4" => bail!("[invalid-input] {what}: ordered4 dither applies to 8-bit output"),
            other => bail!("[invalid-input] {what}.dither {other:?} must be none or ordered4"),
        };
        let mut resize = parse_resize(object.get("resize"), &what)?;
        let ppi = number(object.get("ppi"), &format!("{what}.ppi"), 0.0, 4800.0)?;
        if ppi.is_some_and(|p| p < 1.0) {
            bail!("[invalid-input] {what}.ppi must be 1–4800")
        }
        let ppi = match &mut resize {
            Resize::Print(print) => {
                if print.ppi > 0.0 && ppi.is_some_and(|p| p != print.ppi) {
                    bail!(
                        "[invalid-input] {what}: ppi {} and resize.ppi {} disagree; give one",
                        ppi.unwrap(),
                        print.ppi
                    )
                }
                if print.ppi <= 0.0 {
                    print.ppi = ppi.unwrap_or(300.0);
                }
                if print.ppi < 1.0 {
                    bail!("[invalid-input] {what}.resize.ppi must be 1–4800")
                }
                print.ppi
            }
            _ => ppi.unwrap_or(72.0),
        };
        let sharpen = match object.get("sharpen") {
            None => None,
            Some(v) => {
                let s = v
                    .as_object()
                    .with_context(|| format!("[invalid-input] {what}.sharpen must be an object"))?;
                only(s, &["target", "amount"], &format!("{what}.sharpen"))?;
                let field = |key: &str| s.get(key).and_then(Value::as_str).unwrap_or_default();
                Some((
                    pick(
                        field("target"),
                        &["screen", "matte", "glossy"],
                        &format!("{what}.sharpen.target"),
                    )?,
                    pick(
                        field("amount"),
                        &["low", "standard", "high"],
                        &format!("{what}.sharpen.amount"),
                    )?,
                ))
            }
        };
        let naming = text("naming")?.unwrap_or(DEFAULT_NAMING).to_string();
        if naming.is_empty() || naming.len() > MAX_NAMING {
            bail!("[invalid-input] {what}.naming must be 1–{MAX_NAMING} bytes")
        }
        let tokens = template(&naming)?;
        let metadata = match object.get("metadata") {
            None => Policy::new("none", &[], &[])?,
            Some(v) => Policy::from_json(v)?,
        };
        if tokens.contains(&Token::Captured) && !metadata.keeps("timestamps") {
            bail!("[invalid-input] {what}: naming uses {{captured:YYYYMMDD}} but metadata does not export timestamps, so the file name would leak what the metadata strips; keep timestamps or remove the token")
        }
        let recipe = Self {
            name: name.to_string(),
            built_in: false,
            format,
            depth,
            color_space,
            intent,
            hdr,
            dither,
            resize,
            ppi,
            sharpen,
            naming,
            metadata,
        };
        recipe.output()?;
        Ok(recipe)
    }

    /// A built-in recipe or one of `photography.recipes`.
    pub fn named(raw: &Value, name: &str) -> Result<Self> {
        if let Some(value) = built_in(name) {
            let mut recipe = Self::parse(name, &value)?;
            recipe.built_in = true;
            return Ok(recipe);
        }
        let value = raw["photography"]["recipes"].get(name).with_context(|| {
            let mut names: Vec<String> = BUILT_IN.iter().map(|n| n.to_string()).collect();
            names.extend(
                raw["photography"]["recipes"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(n, _)| n.clone()),
            );
            format!(
                "[missing-resource] recipe {name} does not exist; recipes are {}",
                names.join(", ")
            )
        })?;
        Self::parse(name, value)
    }

    /// Every recipe of a document, built-ins first.
    pub fn list(raw: &Value) -> Result<Value> {
        let mut out = Vec::new();
        for name in BUILT_IN {
            out.push(json!({"name": name, "built_in": true, "recipe": built_in(name)}));
        }
        for (name, value) in raw["photography"]["recipes"]
            .as_object()
            .into_iter()
            .flatten()
        {
            Self::parse(name, value)?;
            out.push(json!({"name": name, "built_in": false, "recipe": value}));
        }
        Ok(json!({"recipes": out}))
    }

    /// The stage-11 output this recipe encodes to.
    pub fn output(&self) -> Result<Output> {
        let (transfer, headroom) = match &self.hdr {
            Some((t, h)) => (Some(t.as_str()), Some(*h)),
            None => (None, None),
        };
        if self.hdr.is_some() && self.format != Format::Png {
            bail!("[invalid-input] HDR output is PNG only")
        }
        let mut output = Output::new(
            Some(&self.color_space),
            Some(self.depth),
            self.intent.name(),
            transfer,
            headroom,
        )?;
        output.dither = self.dither;
        Ok(output)
    }

    /// Apply a run's `--print` size and `--fit`; a print recipe needs a size.
    pub fn with_print(mut self, size: Option<(f64, f64, bool)>, fit: Option<Fit>) -> Result<Self> {
        let Resize::Print(print) = &mut self.resize else {
            if size.is_some() || fit.is_some() {
                bail!("[invalid-input] --print and --fit apply to a recipe that resizes for print; recipe {} does not", self.name)
            }
            return Ok(self);
        };
        if let Some((width, height, centimeters)) = size {
            for side in [width, height] {
                if !(side > 0.0 && side <= 1000.0) {
                    bail!("[invalid-input] --print sides must be above 0 and at most 1000")
                }
            }
            print.width = Some(width);
            print.height = Some(height);
            print.centimeters = centimeters;
        }
        if let Some(fit) = fit {
            print.fit = fit;
        }
        if print.width.is_none() || print.height.is_none() {
            bail!("[invalid-input] recipe {} prints at a size given per run; add --print WIDTHxHEIGHTin (or cm), such as --print 6x4in", self.name)
        }
        Ok(self)
    }

    /// The resolved recipe, as reported.
    pub fn report(&self) -> Value {
        let mut out = json!({
            "name": self.name,
            "built_in": self.built_in,
            "format": self.format.name(),
            "bit_depth": self.depth,
            "color_space": self.color_space,
            "intent": self.intent.name(),
            "ppi": self.ppi,
            "naming": self.naming,
            "metadata": self.metadata.report(),
        });
        match self.format {
            Format::Jpeg { quality, chroma } => {
                out["quality"] = json!(quality);
                out["chroma"] = json!(chroma.name());
            }
            Format::Tiff { deflate } => {
                out["compression"] = json!(if deflate { "deflate" } else { "none" });
            }
            Format::Png => {}
        }
        if let Some((transfer, headroom)) = &self.hdr {
            out["hdr"] = json!({"transfer": transfer, "headroom": headroom});
        }
        if let Some((target, amount)) = self.sharpen {
            out["sharpen"] = json!({"target": target, "amount": amount});
        }
        out["resize"] = match &self.resize {
            Resize::None => json!({"mode": "none"}),
            Resize::Edge {
                edge,
                value,
                enlarge,
            } => json!({
                "mode": match edge {
                    Edge::Long => "long-edge",
                    Edge::Short => "short-edge",
                    Edge::Width => "width",
                    Edge::Height => "height",
                },
                "value": value,
                "enlarge": enlarge,
            }),
            Resize::Megapixels { value, enlarge } => {
                json!({"mode": "megapixels", "value": value, "enlarge": enlarge})
            }
            Resize::Percent { value, enlarge } => {
                json!({"mode": "percent", "value": value, "enlarge": enlarge})
            }
            Resize::Print(p) => json!({
                "mode": "print",
                "width": p.width,
                "height": p.height,
                "unit": if p.centimeters { "cm" } else { "in" },
                "ppi": p.ppi,
                "fit": p.fit.name(),
                "enlarge": p.enlarge,
            }),
        };
        out
    }
}

/// `photo recipe set`: add or replace a document recipe.
pub fn set_recipe(raw: &mut Value, name: &str, recipe: Value) -> Result<Value> {
    if BUILT_IN.contains(&name) {
        bail!("[invalid-input] recipe {name} is built in; choose another name")
    }
    let parsed = Recipe::parse(name, &recipe)?;
    if !raw["photography"].is_object() {
        bail!("[missing-resource] the document has no photography catalog; add a photo with raw add first")
    }
    let recipes = &mut raw["photography"]["recipes"];
    if recipes.is_null() {
        *recipes = json!({});
    }
    let replaced = recipes
        .as_object_mut()
        .context("[malformed-resource] photography.recipes must be an object")?
        .insert(name.to_string(), recipe)
        .is_some();
    crate::scene::validate(raw)?;
    Ok(json!({"recipe": parsed.report(), "replaced": replaced}))
}

/// `photo recipe remove`.
pub fn remove_recipe(raw: &mut Value, name: &str) -> Result<Value> {
    if BUILT_IN.contains(&name) {
        bail!("[invalid-input] recipe {name} is built in and cannot be removed")
    }
    let recipes = raw["photography"]["recipes"].as_object_mut();
    if recipes.and_then(|r| r.remove(name)).is_none() {
        bail!("[missing-resource] recipe {name} is not in the document")
    }
    if raw["photography"]["recipes"]
        .as_object()
        .is_some_and(Map::is_empty)
    {
        raw["photography"]
            .as_object_mut()
            .map(|p| p.remove("recipes"));
    }
    Ok(json!({"removed": name}))
}

/// Parse `--print 6x4in` or `--print 15x10cm`.
pub fn parse_print(text: &str) -> Result<(f64, f64, bool)> {
    let fail = || {
        anyhow!("[invalid-input] --print {text:?} must be WIDTHxHEIGHT followed by in or cm, such as 6x4in")
    };
    let (size, centimeters) = if let Some(size) = text.strip_suffix("in") {
        (size, false)
    } else if let Some(size) = text.strip_suffix("cm") {
        (size, true)
    } else {
        return Err(fail());
    };
    let (w, h) = size.split_once('x').ok_or_else(fail)?;
    let side = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    Ok((
        side(w).ok_or_else(fail)?,
        side(h).ok_or_else(fail)?,
        centimeters,
    ))
}

/// How a developed frame becomes an output: scale, then crop (negative
/// offset) or pad (positive offset) to the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub scaled: [u32; 2],
    pub canvas: [u32; 2],
    pub offset: [i64; 2],
}

fn dimension(v: f64) -> u32 {
    let v = v.round();
    if v < 1.0 {
        1
    } else if v > f64::from(u32::MAX) {
        u32::MAX
    } else {
        v as u32
    }
}

/// The output frame of a `width`x`height` photo.
pub fn frame(width: u32, height: u32, resize: &Resize, photo: &str) -> Result<Frame> {
    let (w, h) = (f64::from(width), f64::from(height));
    let plain = |scaled: [u32; 2]| Frame {
        scaled,
        canvas: scaled,
        offset: [0, 0],
    };
    let scale = |s: f64, enlarge: bool| {
        let s = if s > 1.0 && !enlarge { 1.0 } else { s };
        [dimension(w * s), dimension(h * s)]
    };
    let frame = match resize {
        Resize::None => plain([width, height]),
        Resize::Edge {
            edge,
            value,
            enlarge,
        } => {
            let reference = match edge {
                Edge::Long => width.max(height),
                Edge::Short => width.min(height),
                Edge::Width => width,
                Edge::Height => height,
            };
            if *value > reference && !enlarge {
                plain([width, height])
            } else {
                let s = f64::from(*value) / f64::from(reference);
                let mut scaled = [dimension(w * s), dimension(h * s)];
                // The constrained side is exact.
                let landscape = width >= height;
                let side = match edge {
                    Edge::Width => 0,
                    Edge::Height => 1,
                    Edge::Long => usize::from(!landscape),
                    Edge::Short => usize::from(landscape),
                };
                scaled[side] = *value;
                plain(scaled)
            }
        }
        Resize::Megapixels { value, enlarge } => {
            plain(scale((value * 1e6 / (w * h)).sqrt(), *enlarge))
        }
        Resize::Percent { value, enlarge } => plain(scale(value / 100.0, *enlarge)),
        Resize::Print(print) => {
            let (Some(pw), Some(ph)) = (print.width, print.height) else {
                bail!("[invalid-input] the print size is missing; add --print")
            };
            let unit = if print.centimeters { 2.54 } else { 1.0 };
            let mut target = [
                dimension(pw / unit * print.ppi),
                dimension(ph / unit * print.ppi),
            ];
            // A 6x4 print holds a portrait photo as 4x6.
            if (width >= height) != (target[0] >= target[1]) && target[0] != target[1] {
                target.swap(0, 1);
            }
            let (tw, th) = (f64::from(target[0]), f64::from(target[1]));
            let contain = (tw / w).min(th / h);
            let cover = (tw / w).max(th / h);
            let fitted = [dimension(w * contain), dimension(h * contain)];
            let matches = fitted.iter().zip(target).all(|(a, b)| a.abs_diff(b) <= 1);
            let scale = if matches {
                contain
            } else {
                match print.fit {
                    Fit::Error => bail!(
                        "[invalid-input] photo {photo} is {width}x{height} but the print is {}x{} pixels ({pw}x{ph}{} at {} ppi), a different aspect; set fit to crop or pad (pentool never crops silently)",
                        target[0],
                        target[1],
                        if print.centimeters { "cm" } else { "in" },
                        print.ppi
                    ),
                    Fit::Crop => cover,
                    Fit::Pad => contain,
                }
            };
            if scale > 1.0 && !print.enlarge {
                bail!(
                    "[invalid-input] photo {photo} has {:.0} ppi at that print size, below {}; allow enlarge or print smaller",
                    print.ppi / scale,
                    print.ppi
                )
            }
            let scaled = if matches {
                target
            } else {
                [dimension(w * scale), dimension(h * scale)]
            };
            Frame {
                scaled,
                canvas: target,
                offset: [
                    (i64::from(target[0]) - i64::from(scaled[0])) / 2,
                    (i64::from(target[1]) - i64::from(scaled[1])) / 2,
                ],
            }
        }
    };
    for size in [frame.scaled, frame.canvas] {
        pixels::check_surface(size[0], size[1]).with_context(|| format!("photo {photo} output"))?;
    }
    Ok(frame)
}

/// Lanczos-3: `sinc(x) sinc(x / 3)` on `|x| < 3`.
fn lanczos(x: f64) -> f64 {
    let x = x.abs();
    if x < 1e-12 {
        1.0
    } else if x >= 3.0 {
        0.0
    } else {
        let p = std::f64::consts::PI * x;
        3.0 * math::sin(p) * math::sin(p / 3.0) / (p * p)
    }
}

/// Normalized taps of each output sample: (first source index, weights).
fn taps(source: u32, target: u32) -> Vec<(usize, Vec<f64>)> {
    let ratio = f64::from(source) / f64::from(target);
    let scale = ratio.max(1.0);
    let support = 3.0 * scale;
    (0..target)
        .map(|i| {
            let center = (f64::from(i) + 0.5) * ratio;
            let lo = (center - support).floor().max(0.0) as usize;
            let hi = ((center + support).ceil() as usize).min(source as usize);
            let mut weights: Vec<f64> = (lo..hi)
                .map(|j| lanczos((j as f64 + 0.5 - center) / scale))
                .collect();
            let sum: f64 = weights.iter().sum();
            for w in &mut weights {
                *w /= sum;
            }
            (lo, weights)
        })
        .collect()
}

/// One separable pass over `lines` lines of `count` samples of `channels`,
/// with samples `stride` apart. Overshoot is clamped to the range of the
/// samples that contribute, so edges do not ring.
fn pass(
    input: &[f32],
    output: &mut [f32],
    lines: usize,
    taps: &[(usize, Vec<f64>)],
    channels: usize,
    index: impl Fn(usize, usize) -> usize,
    out_index: impl Fn(usize, usize) -> usize,
) -> Result<()> {
    for line in 0..lines {
        if line % 64 == 0 {
            super::check_cancelled()?;
        }
        for (i, (start, weights)) in taps.iter().enumerate() {
            for c in 0..channels {
                let (mut sum, mut lo, mut hi) = (0.0f64, f64::INFINITY, f64::NEG_INFINITY);
                for (k, w) in weights.iter().enumerate() {
                    let v = f64::from(input[index(line, start + k) * channels + c]);
                    sum += w * v;
                    if *w != 0.0 {
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                }
                output[out_index(line, i) * channels + c] = sum.clamp(lo, hi) as f32;
            }
        }
    }
    Ok(())
}

/// Separable Lanczos-3 resampling in linear working light, with alpha
/// premultiplied while filtering.
pub fn resize(image: &Working, width: u32, height: u32) -> Result<Working> {
    let mut out = Working::new(width, height, image.alpha.is_some())?;
    let (sw, sh) = (image.width as usize, image.height as usize);
    let (tw, th) = (width as usize, height as usize);
    let channels = if image.alpha.is_some() { 4 } else { 3 };
    let mut source = Vec::with_capacity(sw * sh * channels);
    for (i, rgb) in image.rgb.chunks_exact(3).enumerate() {
        match &image.alpha {
            Some(alpha) => {
                let a = alpha[i];
                source.extend(rgb.iter().map(|v| v * a));
                source.push(a);
            }
            None => source.extend_from_slice(rgb),
        }
    }
    let horizontal = taps(image.width, width);
    let vertical = taps(image.height, height);
    let mut middle = vec![0.0f32; tw * sh * channels];
    pass(
        &source,
        &mut middle,
        sh,
        &horizontal,
        channels,
        |y, x| y * sw + x,
        |y, x| y * tw + x,
    )?;
    drop(source);
    let mut target = vec![0.0f32; tw * th * channels];
    pass(
        &middle,
        &mut target,
        tw,
        &vertical,
        channels,
        |x, y| y * tw + x,
        |x, y| y * tw + x,
    )?;
    for (i, pixel) in target.chunks_exact(channels).enumerate() {
        let rgb = &mut out.rgb[i * 3..i * 3 + 3];
        match &mut out.alpha {
            Some(alpha) => {
                let a = pixel[3];
                alpha[i] = a.clamp(0.0, 1.0);
                for c in 0..3 {
                    rgb[c] = if a > 0.0 { pixel[c] / a } else { 0.0 };
                }
            }
            None => rgb.copy_from_slice(&pixel[..3]),
        }
    }
    Ok(out)
}

/// Keep the `width`x`height` window of `image` at (`x`, `y`).
fn crop(image: Working, x: u32, y: u32, width: u32, height: u32) -> Result<Working> {
    if (x, y, width, height) == (0, 0, image.width, image.height) {
        return Ok(image);
    }
    let mut out = Working::new(width, height, image.alpha.is_some())?;
    for row in 0..height as usize {
        let from = (y as usize + row) * image.width as usize + x as usize;
        let to = row * width as usize;
        let n = width as usize;
        out.rgb[to * 3..(to + n) * 3].copy_from_slice(&image.rgb[from * 3..(from + n) * 3]);
        if let (Some(a), Some(b)) = (&mut out.alpha, &image.alpha) {
            a[to..to + n].copy_from_slice(&b[from..from + n]);
        }
    }
    Ok(out)
}

/// The capture-sharpening settings of an output target and amount.
pub fn sharpening(target: &str, amount: &str) -> Value {
    let radius = match target {
        "screen" => 0.5,
        "glossy" => 0.8,
        _ => 1.0,
    };
    let amount = match amount {
        "low" => 25,
        "standard" => 50,
        _ => 80,
    };
    json!({"detail": {"sharpening": {"amount": amount, "radius": radius, "detail": 25, "masking": 0}}})
}

/// Place `raster` at `offset` on a white `canvas`.
fn pad(raster: Raster, canvas: [u32; 2], offset: [i64; 2]) -> Result<Raster> {
    if [raster.width, raster.height] == canvas {
        return Ok(raster);
    }
    let channels = raster.channels();
    let (cw, ch) = (canvas[0] as usize, canvas[1] as usize);
    let (w, h) = (raster.width as usize, raster.height as usize);
    let (ox, oy) = (offset[0] as usize, offset[1] as usize);
    let place = |dst: &mut [u32], src: &dyn Fn(usize) -> u32| {
        for y in 0..h {
            for x in 0..w {
                for c in 0..channels {
                    dst[((oy + y) * cw + ox + x) * channels + c] = src((y * w + x) * channels + c);
                }
            }
        }
    };
    let max = raster.depth().max();
    let mut codes = vec![max; cw * ch * channels];
    match &raster.samples {
        Samples::Eight(v) => place(&mut codes, &|i| u32::from(v[i])),
        Samples::Sixteen(v) => place(&mut codes, &|i| u32::from(v[i])),
    }
    let samples = match raster.depth() {
        Depth::Eight => Samples::Eight(codes.into_iter().map(|c| c as u8).collect()),
        Depth::Sixteen => Samples::Sixteen(codes.into_iter().map(|c| c as u16).collect()),
    };
    Raster::new(canvas[0], canvas[1], raster.alpha, samples)
}

/// 8-bit RGB for JPEG: alpha is composited over white in code values.
fn flatten(raster: &Raster) -> Result<(Vec<u8>, u64)> {
    let Samples::Eight(codes) = &raster.samples else {
        bail!("[malformed-resource] JPEG output must be 8-bit")
    };
    if !raster.alpha {
        return Ok((codes.clone(), 0));
    }
    let mut flattened = 0;
    let mut rgb = Vec::with_capacity(codes.len() / 4 * 3);
    for pixel in codes.chunks_exact(4) {
        let a = u32::from(pixel[3]);
        if a < 255 {
            flattened += 1;
        }
        for c in &pixel[..3] {
            rgb.push(((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8);
        }
    }
    Ok((rgb, flattened))
}

/// `pHYs` in pixels per meter.
fn phys(ppi: f64) -> png::Chunk {
    let ppm = dimension(ppi / 0.0254).to_be_bytes();
    let mut data = ppm.to_vec();
    data.extend_from_slice(&ppm);
    data.push(1);
    (*b"pHYs", data)
}

/// A single-strip TIFF of `raster`.
fn tiff(
    raster: &Raster,
    deflate: bool,
    ppi: f64,
    icc: Option<Vec<u8>>,
    embedded: &Embedded,
) -> Result<Vec<u8>> {
    let channels = raster.channels();
    let (w, h) = (raster.width as usize, raster.height as usize);
    let row = w * channels;
    let mut data = match &raster.samples {
        Samples::Eight(v) => {
            let mut v = v.clone();
            if deflate {
                for line in v.chunks_exact_mut(row) {
                    for i in (channels..row).rev() {
                        line[i] = line[i].wrapping_sub(line[i - channels]);
                    }
                }
            }
            v
        }
        Samples::Sixteen(v) => {
            let mut v = v.clone();
            if deflate {
                for line in v.chunks_exact_mut(row) {
                    for i in (channels..row).rev() {
                        line[i] = line[i].wrapping_sub(line[i - channels]);
                    }
                }
            }
            v.iter().flat_map(|s| s.to_le_bytes()).collect()
        }
    };
    if deflate {
        let mut encoder = flate2::write::ZlibEncoder::new(
            Vec::with_capacity(data.len() / 2),
            flate2::Compression::default(),
        );
        encoder.write_all(&data)?;
        data = encoder.finish()?;
    }
    if data.len() > (u32::MAX as usize) - (64 << 20) {
        bail!("[limit-exceeded] the TIFF strip is larger than 4 GiB; resize the recipe")
    }
    let bits = raster.depth().bits();
    let resolution = Field::Rational(vec![[dimension(ppi * 100.0), 100]]);
    let mut image = vec![
        (256, Field::Long(vec![raster.width])),
        (257, Field::Long(vec![raster.height])),
        (258, Field::Short(vec![u16::from(bits); channels])),
        (259, Field::Short(vec![if deflate { 8 } else { 1 }])),
        (262, Field::Short(vec![2])),
        (273, Field::Long(vec![0])),
        (277, Field::Short(vec![channels as u16])),
        (278, Field::Long(vec![raster.height])),
        (279, Field::Long(vec![data.len() as u32])),
        (282, resolution.clone()),
        (283, resolution),
        (284, Field::Short(vec![1])),
        (296, Field::Short(vec![2])),
    ];
    let _ = h;
    if deflate {
        image.push((317, Field::Short(vec![2])));
    }
    if raster.alpha {
        image.push((338, Field::Short(vec![2])));
    }
    if let Some(icc) = icc {
        image.push((34675, Field::Undefined(icc)));
    }
    Ok(metadata::tiff_file(&image, embedded, &data))
}

/// One planned output.
#[derive(Clone, Debug)]
pub struct Planned {
    pub photo: String,
    pub variant: String,
    pub file: String,
    pub source: [u32; 2],
    pub frame: Frame,
    pub estimated_bytes: u64,
    /// A file of that name is already in the output directory.
    pub exists: bool,
}

/// Which variants of each selected photo to export.
pub enum Variants {
    /// Each photo's first variant (its master).
    Master,
    Named(String),
    All,
}

fn estimate(format: Format, width: u32, height: u32, depth: u8) -> u64 {
    let pixels = f64::from(width) * f64::from(height);
    let raw = pixels * 3.0 * f64::from(depth / 8);
    let bytes = match format {
        Format::Jpeg { quality, .. } => {
            let q = f64::from(quality) / 100.0;
            raw * (0.04 + 0.3 * q * q * q)
        }
        Format::Png => raw * 0.6,
        Format::Tiff { deflate: false } => raw + 4096.0,
        Format::Tiff { deflate: true } => raw * 0.7 + 4096.0,
    };
    bytes.ceil() as u64
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

struct Naming<'a> {
    photo: &'a str,
    variant: &'a str,
    name: &'a str,
    recipe: &'a str,
    rating: u64,
    seq: usize,
    captured: Option<&'a str>,
}

fn file_name(tokens: &[Token], naming: &Naming, extension: &str) -> Result<String> {
    let mut stem = String::new();
    for token in tokens {
        match token {
            Token::Text(text) => stem.push_str(text),
            Token::Photo => stem.push_str(naming.photo),
            Token::Variant => stem.push_str(naming.variant),
            Token::Name => stem.push_str(naming.name),
            Token::Recipe => stem.push_str(naming.recipe),
            Token::Rating => stem.push_str(&naming.rating.to_string()),
            Token::Seq(digits) => stem.push_str(&format!("{:0width$}", naming.seq, width = digits)),
            Token::Captured => stem.push_str(
                &naming
                    .captured
                    .map(|c| c.chars().filter(char::is_ascii_digit).take(8).collect())
                    .filter(|d: &String| d.len() == 8)
                    .unwrap_or_else(|| "undated".to_string()),
            ),
        }
    }
    let stem = sanitize(&stem);
    let stem = stem.trim_start_matches('.');
    if stem.is_empty() {
        bail!(
            "[invalid-input] naming gives photo {}/{} an empty file name",
            naming.photo,
            naming.variant
        )
    }
    let stem = if RESERVED_FILE_NAMES.contains(&stem.to_ascii_uppercase().as_str()) {
        format!("{stem}_")
    } else {
        stem.to_string()
    };
    let file = format!("{stem}.{extension}");
    if file.len() > MAX_FILE_NAME {
        bail!(
            "[invalid-input] naming gives photo {}/{} a file name longer than {MAX_FILE_NAME} bytes",
            naming.photo,
            naming.variant
        )
    }
    Ok(file)
}

/// Resolve every output, its name and size, and refuse collisions.
pub fn plan(
    raw: &Value,
    document: &Path,
    recipe: &Recipe,
    photos: &[String],
    variants: &Variants,
    out: &Path,
    overwrite: bool,
) -> Result<Vec<Planned>> {
    crate::scene::validate(raw)?;
    if out.exists() && !out.is_dir() {
        bail!("[invalid-input] --out {} is not a directory", out.display())
    }
    let catalog = &raw["photography"];
    let entries: Vec<&Value> = catalog["photos"]
        .as_array()
        .map(|p| p.iter().collect())
        .unwrap_or_default();
    let mut targets = Vec::new();
    for id in photos {
        let photo = entries
            .iter()
            .find(|p| p["id"] == id.as_str())
            .with_context(|| format!("[missing-resource] photo {id} is not in the catalog"))?;
        let ids: Vec<&str> = photo["variants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["id"].as_str())
            .collect();
        match variants {
            Variants::Master => targets.push((*photo, ids.first().copied().unwrap_or("master"))),
            Variants::Named(name) => {
                let variant = ids.iter().find(|v| **v == name).with_context(|| {
                    format!(
                        "[missing-resource] photo {id} has no variant {name}; its variants are {}",
                        ids.join(", ")
                    )
                })?;
                targets.push((*photo, *variant));
            }
            Variants::All => targets.extend(ids.iter().map(|v| (*photo, *v))),
        }
        if targets.len() > MAX_OUTPUTS {
            bail!("[limit-exceeded] the export has more than {MAX_OUTPUTS} outputs; narrow the selection")
        }
    }
    let tokens = template(&recipe.naming)?;
    let mut planned = Vec::with_capacity(targets.len());
    let mut names: HashMap<String, usize> = HashMap::new();
    for (seq, (photo, variant)) in targets.into_iter().enumerate() {
        super::check_cancelled()?;
        let id = photo["id"].as_str().unwrap_or_default();
        let info = catalog::info_validated(raw, document, id, variant)?;
        let size = |i: usize| -> Result<u32> {
            info["output"][i]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .context("[malformed-resource] the develop plan has no output size")
        };
        let source = [size(0)?, size(1)?];
        let frame = frame(source[0], source[1], &recipe.resize, id)?;
        let asset = &catalog["assets"][photo["source"].as_str().unwrap_or_default()];
        let file = file_name(
            &tokens,
            &Naming {
                photo: id,
                variant,
                name: photo["name"].as_str().unwrap_or(id),
                recipe: &recipe.name,
                rating: photo["rating"].as_u64().unwrap_or(0),
                seq: seq + 1,
                captured: asset["capture"]["captured"].as_str(),
            },
            recipe.format.extension(),
        )?;
        // Case-insensitive, as on Windows and macOS file systems.
        if let Some(other) = names.insert(file.to_ascii_lowercase(), planned.len()) {
            let other: &Planned = &planned[other];
            bail!(
                "[conflict] {}/{} and {id}/{variant} would both be written as {file}; add {{variant}} or {{seq:N}} to the recipe's naming",
                other.photo,
                other.variant
            )
        }
        let target = out.join(&file);
        let exists = target.symlink_metadata().is_ok();
        if exists && target.is_dir() {
            bail!("[conflict] {} is a directory", target.display())
        }
        planned.push(Planned {
            photo: id.to_string(),
            variant: variant.to_string(),
            file,
            source,
            estimated_bytes: estimate(
                recipe.format,
                frame.canvas[0],
                frame.canvas[1],
                recipe.depth,
            ),
            frame,
            exists,
        });
    }
    let existing: Vec<&str> = planned
        .iter()
        .filter(|p| p.exists)
        .map(|p| p.file.as_str())
        .collect();
    if !existing.is_empty() && !overwrite {
        bail!(
            "[policy-denied] {} output(s) already exist in {} ({}{}); choose another --out or pass --overwrite",
            existing.len(),
            out.display(),
            existing.iter().take(5).copied().collect::<Vec<_>>().join(", "),
            if existing.len() > 5 { ", ..." } else { "" }
        )
    }
    Ok(planned)
}

/// The `--dry-run` report.
pub fn plan_report(recipe: &Recipe, out: &Path, planned: &[Planned]) -> Value {
    json!({
        "dry_run": true,
        "recipe": recipe.report(),
        "out": out.display().to_string(),
        "count": planned.len(),
        "estimated_bytes": planned.iter().map(|p| p.estimated_bytes).sum::<u64>(),
        "outputs": planned.iter().map(|p| json!({
            "photo": p.photo,
            "variant": p.variant,
            "path": out.join(&p.file).display().to_string(),
            "width": p.frame.canvas[0],
            "height": p.frame.canvas[1],
            "estimated_bytes": p.estimated_bytes,
            "exists": p.exists,
        })).collect::<Vec<_>>(),
    })
}

/// Develop, resize, sharpen and encode one output.
pub fn encode(
    raw: &Value,
    document: &Path,
    recipe: &Recipe,
    planned: &Planned,
) -> Result<(Vec<u8>, Value)> {
    let output = recipe.output()?;
    let developed = catalog::render_validated(raw, document, &planned.photo, &planned.variant)?;
    let mut image = developed.image;
    if [image.width, image.height] != planned.source {
        bail!(
            "[malformed-resource] photo {}/{} developed to {}x{}, not the planned {}x{}",
            planned.photo,
            planned.variant,
            image.width,
            image.height,
            planned.source[0],
            planned.source[1]
        )
    }
    let frame = planned.frame;
    if [image.width, image.height] != frame.scaled {
        image = resize(&image, frame.scaled[0], frame.scaled[1])?;
    }
    if frame.offset[0] < 0 || frame.offset[1] < 0 {
        let (x, y) = (
            (-frame.offset[0]).max(0) as u32,
            (-frame.offset[1]).max(0) as u32,
        );
        image = crop(image, x, y, frame.canvas[0], frame.canvas[1])?;
    }
    if let Some((target, amount)) = recipe.sharpen {
        super::detail::sharpen(&mut image, &sharpening(target, amount))?;
    }
    let (raster, counts) = output::render(&image, &developed.rendering, true, &output)?;
    drop(image);
    let offset = [frame.offset[0].max(0), frame.offset[1].max(0)];
    let raster = pad(raster, frame.canvas, offset)?;
    let measured = output::measure(&raster, &output, output::DEFAULT_BINS)?;
    let mut report = output::report(&output, &counts, &measured);
    if let Some(histogram) = report.as_object_mut() {
        histogram.remove("histogram");
    }
    let embedded = metadata::prepare(raw, document, &planned.photo, &recipe.metadata)?;
    let bytes = match recipe.format {
        Format::Png => {
            let mut chunks = output::chunks(&output, &measured)?;
            chunks.push(phys(recipe.ppi));
            let bytes = png::write_tagged(&raster, &chunks)?;
            output::verify(&bytes, &raster, &output, &measured)?;
            metadata::embed_png(&bytes, &embedded)?
        }
        Format::Jpeg { quality, chroma } => {
            let (rgb, flattened) = flatten(&raster)?;
            report["flattened_pixels"] = json!(flattened);
            let ppi = u16::try_from(dimension(recipe.ppi)).unwrap_or(u16::MAX);
            let bytes = jpeg::encode(
                raster.width,
                raster.height,
                &rgb,
                quality,
                chroma,
                ppi,
                output.icc().as_deref(),
            )?;
            metadata::embed_jpeg(&bytes, &embedded)?
        }
        Format::Tiff { deflate } => tiff(&raster, deflate, recipe.ppi, output.icc(), &embedded)?,
    };
    report["metadata"] = embedded.report(&recipe.metadata);
    Ok((bytes, report))
}

/// Removes the staging directory, and an output directory the run created,
/// unless the run completed.
struct Staging {
    dir: PathBuf,
    created_out: Option<PathBuf>,
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
        if let Some(out) = &self.created_out {
            let _ = fs::remove_dir(out);
        }
    }
}

/// Write every planned output into `out`: each is rendered into a staging
/// directory, and only after all succeed are they renamed into place.
pub fn run(
    raw: &Value,
    document: &Path,
    recipe: &Recipe,
    planned: &[Planned],
    out: &Path,
) -> Result<Value> {
    let created_out = if out.exists() {
        None
    } else {
        fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
        Some(out.to_path_buf())
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let run = hex::encode(&Sha256::digest(format!("{}-{stamp}", std::process::id()))[..6]);
    let mut staging = Staging {
        dir: out.join(format!(".pentool-export-{run}")),
        created_out,
    };
    fs::create_dir(&staging.dir)
        .with_context(|| format!("create staging directory {}", staging.dir.display()))?;
    let mut outputs = Vec::with_capacity(planned.len());
    let mut total = 0u64;
    for item in planned {
        super::check_cancelled()?;
        let (bytes, delivery) = encode(raw, document, recipe, item)
            .with_context(|| format!("export {}/{}", item.photo, item.variant))?;
        let staged = staging.dir.join(&item.file);
        fs::write(&staged, &bytes).with_context(|| format!("write {}", staged.display()))?;
        total += bytes.len() as u64;
        outputs.push(json!({
            "photo": item.photo,
            "variant": item.variant,
            "path": out.join(&item.file).display().to_string(),
            "width": item.frame.canvas[0],
            "height": item.frame.canvas[1],
            "bytes": bytes.len(),
            "sha256": hex::encode(Sha256::digest(&bytes)),
            "delivery": delivery,
        }));
    }
    super::check_cancelled()?;
    let mut moved: Vec<&Planned> = Vec::new();
    for item in planned {
        let target = out.join(&item.file);
        if let Err(error) = fs::rename(staging.dir.join(&item.file), &target) {
            for done in &moved {
                if !done.exists {
                    let _ = fs::remove_file(out.join(&done.file));
                }
            }
            return Err(anyhow!(error))
                .with_context(|| format!("move {} into place", target.display()));
        }
        moved.push(item);
    }
    staging.created_out = None;
    drop(staging);
    Ok(json!({
        "recipe": recipe.report(),
        "out": out.display().to_string(),
        "count": outputs.len(),
        "bytes": total,
        "outputs": outputs,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(width: u32, height: u32, value: f32) -> Working {
        let mut image = Working::new(width, height, false).unwrap();
        image.rgb.fill(value);
        image
    }

    #[test]
    fn built_in_recipes_and_the_fixture_recipe_parse() {
        for name in BUILT_IN {
            let recipe = Recipe::named(&json!({}), name).unwrap();
            assert!(recipe.built_in);
        }
        let lab = Recipe::named(&json!({}), "photo-lab").unwrap();
        assert!(lab.clone().with_print(None, None).is_err());
        let lab = lab.with_print(Some((6.0, 4.0, false)), None).unwrap();
        assert_eq!(lab.report()["resize"]["width"], 6.0);
        assert!(Recipe::named(&json!({}), "web-gallery")
            .unwrap()
            .with_print(Some((6.0, 4.0, false)), None)
            .is_err());
        let archive = Recipe::named(&json!({}), "archive-master").unwrap();
        assert_eq!(archive.format, Format::Tiff { deflate: true });
        assert_eq!(archive.depth, 16);
    }

    #[test]
    fn recipes_follow_the_schema() {
        let ok = json!({"format": "png", "color_space": "rec2020", "hdr": {"transfer": "pq", "headroom": 2}});
        assert_eq!(Recipe::parse("hdr", &ok).unwrap().depth, 16);
        for bad in [
            json!({"format": "jpeg"}),
            json!({"format": "gif", "color_space": "srgb"}),
            json!({"format": "jpeg", "color_space": "srgb", "bit_depth": 16}),
            json!({"format": "jpeg", "color_space": "srgb", "compression": "deflate"}),
            json!({"format": "png", "color_space": "srgb", "quality": 80}),
            json!({"format": "tiff", "color_space": "srgb", "chroma": "420"}),
            json!({"format": "jpeg", "color_space": "srgb", "quality": 0}),
            json!({"format": "jpeg", "color_space": "cmyk"}),
            json!({"format": "png", "color_space": "srgb", "hdr": {"transfer": "pq", "headroom": 2}}),
            json!({"format": "png", "color_space": "srgb", "extra": 1}),
            json!({"format": "png", "color_space": "srgb", "resize": {"mode": "long-edge"}}),
            json!({"format": "png", "color_space": "srgb", "resize": {"mode": "none", "value": 3}}),
            json!({"format": "png", "color_space": "srgb", "sharpen": {"target": "screen", "amount": "max"}}),
            json!({"format": "png", "color_space": "srgb", "naming": "{photo}-{date}"}),
            json!({"format": "png", "color_space": "srgb", "naming": "{seq:7}"}),
            json!({"format": "png", "color_space": "srgb", "naming": "{captured:YYYYMMDD}"}),
            json!({"format": "png", "color_space": "srgb", "dither": "ordered4", "bit_depth": 16}),
            json!({"format": "png", "color_space": "srgb", "metadata": {"policy": "secret"}}),
        ] {
            assert!(Recipe::parse("bad", &bad).is_err(), "{bad}");
        }
        let dated = json!({"format": "png", "color_space": "srgb", "naming": "{captured:YYYYMMDD}-{seq:3}",
            "metadata": {"policy": "public", "include": ["timestamps"]}});
        assert!(Recipe::parse("dated", &dated).is_ok());
    }

    #[test]
    fn frames_resize_by_every_mode() {
        let edge = |edge, value, enlarge| Resize::Edge {
            edge,
            value,
            enlarge,
        };
        let f = |r: &Resize| frame(3000, 2000, r, "p").unwrap().canvas;
        assert_eq!(f(&Resize::None), [3000, 2000]);
        assert_eq!(f(&edge(Edge::Long, 2048, false)), [2048, 1365]);
        assert_eq!(f(&edge(Edge::Short, 1000, false)), [1500, 1000]);
        assert_eq!(f(&edge(Edge::Height, 4000, false)), [3000, 2000]);
        assert_eq!(f(&edge(Edge::Height, 4000, true)), [6000, 4000]);
        assert_eq!(
            f(&Resize::Percent {
                value: 50.0,
                enlarge: false
            }),
            [1500, 1000]
        );
        assert_eq!(
            f(&Resize::Megapixels {
                value: 1.5,
                enlarge: false
            }),
            [1500, 1000]
        );
        let print = |fit, w, h| {
            Resize::Print(Print {
                width: Some(w),
                height: Some(h),
                centimeters: false,
                ppi: 300.0,
                fit,
                enlarge: true,
            })
        };
        assert_eq!(f(&print(Fit::Error, 6.0, 4.0)), [1800, 1200]);
        // A portrait photo takes the print in portrait.
        assert_eq!(
            frame(2000, 3000, &print(Fit::Error, 6.0, 4.0), "p")
                .unwrap()
                .canvas,
            [1200, 1800]
        );
        assert!(frame(3000, 2000, &print(Fit::Error, 5.0, 5.0), "p").is_err());
        let crop = frame(3000, 2000, &print(Fit::Crop, 5.0, 5.0), "p").unwrap();
        assert_eq!(
            (crop.scaled, crop.canvas, crop.offset),
            ([2250, 1500], [1500, 1500], [-375, 0])
        );
        let pad = frame(3000, 2000, &print(Fit::Pad, 5.0, 5.0), "p").unwrap();
        assert_eq!(
            (pad.scaled, pad.canvas, pad.offset),
            ([1500, 1000], [1500, 1500], [0, 250])
        );
        let mut small = print(Fit::Error, 60.0, 40.0);
        if let Resize::Print(p) = &mut small {
            p.enlarge = false;
        }
        assert!(frame(3000, 2000, &small, "p").is_err());
    }

    #[test]
    fn lanczos_keeps_flat_fields_and_does_not_ring() {
        let flat = resize(&constant(37, 23, 0.25), 11, 7).unwrap();
        assert!(flat.rgb.iter().all(|v| (v - 0.25).abs() < 1e-6));
        let up = resize(&constant(5, 3, 0.5), 17, 9).unwrap();
        assert!(up.rgb.iter().all(|v| (v - 0.5).abs() < 1e-6));
        let mut edge = Working::new(16, 1, true).unwrap();
        for x in 8..16 {
            edge.rgb[x * 3..x * 3 + 3].fill(1.0);
        }
        edge.alpha.as_mut().unwrap()[0] = 0.0;
        let out = resize(&edge, 40, 1).unwrap();
        assert!(
            out.rgb.iter().all(|v| (0.0..=1.0).contains(v)),
            "{:?}",
            out.rgb
        );
        assert_eq!(resize(&edge, 40, 1).unwrap(), out);
    }

    #[test]
    fn names_are_sanitized_and_tokens_filled() {
        let tokens = template("{name} {recipe}/{rating}-{seq:3}-{captured:YYYYMMDD}").unwrap();
        let naming = Naming {
            photo: "a",
            variant: "master",
            name: "Harbor at dusk",
            recipe: "web",
            rating: 4,
            seq: 7,
            captured: Some("2026-05-01T18:30:00"),
        };
        assert_eq!(
            file_name(&tokens, &naming, "jpg").unwrap(),
            "Harbor_at_dusk_web_4-007-20260501.jpg"
        );
        let con = file_name(&template("con").unwrap(), &naming, "png").unwrap();
        assert_eq!(con, "con_.png");
        assert!(file_name(&template("...").unwrap(), &naming, "png").is_err());
        assert_eq!(parse_print("6x4in").unwrap(), (6.0, 4.0, false));
        assert_eq!(parse_print("15x10cm").unwrap(), (15.0, 10.0, true));
        assert!(parse_print("6x4").is_err());
    }
}

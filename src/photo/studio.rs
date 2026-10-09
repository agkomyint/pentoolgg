//! The photographer editor's service (v0.11.0 item 15): previews tagged for
//! the browser (sRGB or Display P3, with matching `iCCP` and `cICP`), clipping,
//! gamut and mask overlays, the photo detail the panels read, and the edit
//! operations the browser sends. Every edit calls the same library function as
//! the CLI command it mirrors, so the browser and the CLI write identical
//! documents.

use super::pixels::{Raster, Samples};
use super::{catalog, export, organize, output, png, variants};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::Path;

/// Default and largest long edge of a preview, in pixels.
pub const DEFAULT_EDGE: u32 = 1600;
pub const MAX_EDGE: u32 = 4096;
/// Histogram bins per channel reported with a preview.
pub const HISTOGRAM_BINS: usize = 64;
/// The color spaces a preview may be tagged with.
pub const PREVIEW_SPACES: [&str; 2] = ["srgb", "display-p3"];
/// Brush samples per browser stroke.
pub const MAX_STROKE_SAMPLES: usize = 4096;

/// Overlay colors in output code values.
const HIGHLIGHT: [u8; 3] = [255, 0, 0];
const SHADOW: [u8; 3] = [0, 64, 255];
const GAMUT: [u8; 3] = [255, 0, 255];
const MASK: [f32; 3] = [255.0, 32.0, 32.0];
/// Opacity of the mask tint at full weight.
const MASK_OPACITY: f32 = 0.6;

/// What a preview paints over the photo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    None,
    /// Pixels with a channel at the top code (red) or, otherwise, at 0 (blue).
    Clipping,
    /// Pixels outside the preview space before gamut mapping (magenta).
    Gamut,
    /// The weights of a local adjustment, as a red tint.
    Mask(String),
}

impl Overlay {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "" | "none" => Self::None,
            "clipping" => Self::Clipping,
            "gamut" => Self::Gamut,
            other => match other.strip_prefix("mask:") {
                Some(id) if catalog::is_id(id) => Self::Mask(id.to_owned()),
                _ => bail!(
                    "[invalid-input] overlay {other:?} must be none, clipping, gamut or mask:ADJUSTMENT"
                ),
            },
        })
    }

    pub fn name(&self) -> String {
        match self {
            Self::None => "none".into(),
            Self::Clipping => "clipping".into(),
            Self::Gamut => "gamut".into(),
            Self::Mask(id) => format!("mask:{id}"),
        }
    }
}

/// A preview of one variant.
pub struct PreviewRequest<'a> {
    pub photo: &'a str,
    pub variant: &'a str,
    /// Long edge in pixels (1–[`MAX_EDGE`]); a smaller develop is not enlarged.
    pub edge: u32,
    /// One of [`PREVIEW_SPACES`].
    pub space: &'a str,
    pub overlay: Overlay,
    /// Develop without the `crop` group: the frame the crop rectangle and
    /// brush masks are drawn in.
    pub uncropped: bool,
}

/// A tagged 8-bit PNG and its report.
pub struct Preview {
    pub png: Vec<u8>,
    pub report: Value,
}

/// The preview size of a `width` x `height` develop with long edge `edge`.
pub fn fit(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= edge {
        return (width, height);
    }
    let scale = f64::from(edge) / f64::from(long);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, edge);
    (side(width), side(height))
}

fn without_crop(raw: &Value, photo: &str, variant: &str) -> Value {
    let mut copy = raw.clone();
    if let Some(develop) = copy["photography"]["photos"]
        .as_array_mut()
        .and_then(|photos| photos.iter_mut().find(|p| p["id"] == photo))
        .and_then(|p| p["variants"].as_array_mut())
        .and_then(|list| list.iter_mut().find(|v| v["id"] == variant))
        .and_then(|v| v.get_mut("develop"))
        .and_then(Value::as_object_mut)
    {
        develop.remove("crop");
    }
    copy
}

/// Develop a variant, scale it to the preview size in linear light, run
/// stage 11 into the preview space and paint the overlay. The histogram and
/// clipping counts are measured before the overlay.
pub fn preview(raw: &Value, document: &Path, request: &PreviewRequest) -> Result<Preview> {
    if !(1..=MAX_EDGE).contains(&request.edge) {
        bail!(
            "[invalid-input] preview edge {} must be 1–{MAX_EDGE}",
            request.edge
        )
    }
    if !PREVIEW_SPACES.contains(&request.space) {
        bail!(
            "[invalid-input] preview space {:?} must be {}",
            request.space,
            PREVIEW_SPACES.join(" or ")
        )
    }
    crate::scene::validate(raw)?;
    let copy;
    let raw = if request.uncropped {
        copy = without_crop(raw, request.photo, request.variant);
        &copy
    } else {
        raw
    };
    let (developed, weights) = match &request.overlay {
        Overlay::Mask(id) => {
            let (developed, weights) =
                catalog::render_mask(raw, document, request.photo, request.variant, id)?;
            (developed, Some(weights))
        }
        _ => (
            catalog::render_validated(raw, document, request.photo, request.variant)?,
            None,
        ),
    };
    let (dw, dh) = (developed.image.width, developed.image.height);
    let (width, height) = fit(dw, dh, request.edge);
    let image = if (width, height) == (dw, dh) {
        developed.image
    } else {
        export::resize(&developed.image, width, height)?
    };
    let out = output::Output::new(Some(request.space), Some(8), "perceptual", None, None)?;
    let (mut raster, counts, marks) =
        output::render_marked(&image, &developed.rendering, true, &out)?;
    drop(image);
    let measured = output::measure(&raster, &out, HISTOGRAM_BINS)?;
    let (highlights, shadows) = paint(&mut raster, |_, codes| clip_class(codes), None)?;
    let marked = match &request.overlay {
        Overlay::None => 0,
        Overlay::Clipping => {
            let (h, s) = paint(
                &mut raster,
                |_, codes| clip_class(codes),
                Some(&|class| if class == 1 { HIGHLIGHT } else { SHADOW }),
            )?;
            h + s
        }
        Overlay::Gamut => paint(&mut raster, |i, _| u8::from(marks[i]), Some(&|_| GAMUT))?.0,
        Overlay::Mask(_) => {
            let weights = weights.unwrap_or_default();
            tint(&mut raster, &weights, (dw, dh))?
        }
    };
    let bytes = png::write_tagged(&raster, &output::chunks(&out, &measured)?)?;
    let report = json!({
        "photo": request.photo,
        "variant": request.variant,
        "width": width,
        "height": height,
        "developed_width": dw,
        "developed_height": dh,
        "uncropped": request.uncropped,
        "overlay": request.overlay.name(),
        "marked_pixels": marked,
        "highlight_clipped_pixels": highlights,
        "shadow_clipped_pixels": shadows,
        "delivery": output::report(&out, &counts, &measured),
    });
    Ok(Preview { png: bytes, report })
}

/// The `srgb8` rendition a page shows for a `photo` node: the developed variant,
/// reduced in linear light when `max_edge` (a version 5 proxy render) is
/// smaller, through stage 11 into sRGB with the perceptual intent. Returns the RGBA pixels and the developed
/// size, which placement uses so that a reduced rendition lands in the same
/// rectangle. `raw` must already be validated.
pub fn node_rendition(
    raw: &Value,
    document: &Path,
    photo: &str,
    variant: &str,
    max_edge: Option<u32>,
) -> Result<(image::RgbaImage, u32, u32)> {
    let developed = catalog::render_validated(raw, document, photo, variant)?;
    let (dw, dh) = (developed.image.width, developed.image.height);
    let (width, height) = max_edge.map_or((dw, dh), |edge| fit(dw, dh, edge));
    let image = if (width, height) == (dw, dh) {
        developed.image
    } else {
        export::resize(&developed.image, width, height)?
    };
    let out = output::Output::new(Some("srgb"), Some(8), "perceptual", None, None)?;
    let (raster, _) = output::render(&image, &developed.rendering, true, &out)?;
    let Samples::Eight(samples) = raster.samples else {
        bail!("photo node rendition must be 8-bit")
    };
    let channels = if raster.alpha { 4 } else { 3 };
    let rgba = samples
        .chunks_exact(channels)
        .flat_map(|p| [p[0], p[1], p[2], if raster.alpha { p[3] } else { u8::MAX }])
        .collect();
    let pixels = image::RgbaImage::from_raw(raster.width, raster.height, rgba)
        .context("photo node rendition has the wrong size")?;
    Ok((pixels, dw, dh))
}

/// [`preview`] through `cache`, adding `cache` (`hit`, `miss` or `off`) to the
/// report. The document is always validated first; a hit returns the same PNG
/// bytes and report as a render, and only successful renders are stored.
pub fn preview_cached(
    raw: &Value,
    document: &Path,
    request: &PreviewRequest,
    cache: Option<&super::cache::Cache>,
) -> Result<Preview> {
    use super::cache::{preview_key, Outcome};
    crate::scene::validate(raw)?;
    let key = cache.and_then(|_| preview_key(raw, request).ok());
    if let (Some(cache), Some(key)) = (cache, &key) {
        if let Some((png, mut report)) = cache.get(key) {
            report["cache"] = json!(Outcome::Hit.name());
            return Ok(Preview { png, report });
        }
    }
    let mut preview = preview(raw, document, request)?;
    let outcome = match (cache, &key) {
        (Some(cache), Some(key)) => {
            cache.put(key, &preview.png, &preview.report);
            Outcome::Miss
        }
        _ => Outcome::Off,
    };
    preview.report["cache"] = json!(outcome.name());
    Ok(preview)
}

/// 1 when a channel is at the top code, 2 when otherwise a channel is 0.
fn clip_class(codes: &[u8]) -> u8 {
    if codes[..3].contains(&u8::MAX) {
        1
    } else if codes[..3].contains(&0) {
        2
    } else {
        0
    }
}

/// Classify every opaque pixel and, with `color`, paint each nonzero class.
/// Returns the pixels of class 1 and of any other nonzero class.
fn paint(
    raster: &mut Raster,
    class: impl Fn(usize, &[u8]) -> u8,
    color: Option<&dyn Fn(u8) -> [u8; 3]>,
) -> Result<(u64, u64)> {
    let channels = if raster.alpha { 4 } else { 3 };
    let Samples::Eight(codes) = &mut raster.samples else {
        bail!("[internal] previews are 8-bit")
    };
    let (mut first, mut other) = (0, 0);
    for (i, pixel) in codes.chunks_exact_mut(channels).enumerate() {
        if channels == 4 && pixel[3] == 0 {
            continue;
        }
        let c = class(i, pixel);
        match c {
            0 => continue,
            1 => first += 1,
            _ => other += 1,
        }
        if let Some(color) = color {
            pixel[..3].copy_from_slice(&color(c));
        }
    }
    Ok((first, other))
}

/// Tint by mask weights sampled (nearest, at pixel centers) from the develop
/// size `source`. Returns the pixels with a weight above 0.
fn tint(raster: &mut Raster, weights: &[f32], source: (u32, u32)) -> Result<u64> {
    let (sw, sh) = (source.0 as usize, source.1 as usize);
    if weights.len() != sw * sh {
        bail!("[internal] mask weights do not match the develop size")
    }
    let (w, h) = (raster.width as usize, raster.height as usize);
    let channels = if raster.alpha { 4 } else { 3 };
    let Samples::Eight(codes) = &mut raster.samples else {
        bail!("[internal] previews are 8-bit")
    };
    let mut marked = 0;
    for y in 0..h {
        let sy = ((y * 2 + 1) * sh / (h * 2)).min(sh - 1);
        for x in 0..w {
            let sx = ((x * 2 + 1) * sw / (w * 2)).min(sw - 1);
            let weight = weights[sy * sw + sx].clamp(0.0, 1.0);
            if weight <= 0.0 {
                continue;
            }
            marked += 1;
            let a = weight * MASK_OPACITY;
            let pixel = &mut codes[(y * w + x) * channels..][..3];
            for (code, tint) in pixel.iter_mut().zip(MASK) {
                *code = (f32::from(*code) * (1.0 - a) + tint * a).round() as u8;
            }
        }
    }
    Ok(marked)
}

/// What the editor's panels show for one photo: its catalog entry (rating,
/// keywords, variants with their develop settings, snapshots) plus the source
/// kind and oriented size.
pub fn detail(raw: &Value, photo_id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let catalog = raw.get("photography").context(
        "[missing-resource] the document has no photography catalog; add a photo with `raw add` first",
    )?;
    let photo = catalog["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"] == photo_id)
        .with_context(|| format!("[missing-resource] photo {photo_id} is not in the catalog"))?;
    let asset = &catalog["assets"][photo["source"].as_str().unwrap_or_default()];
    let swap = asset["orientation"].as_u64().is_some_and(|o| o >= 5);
    let (w, h) = (&asset["pixel_width"], &asset["pixel_height"]);
    let local: Vec<Value> = photo["variants"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            json!({
                "variant": v["id"],
                "adjustments": v["develop"]["local"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| a["id"].clone())
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({
        "photo": photo,
        "kind": asset["kind"],
        "width": if swap { h } else { w },
        "height": if swap { w } else { h },
        "local": local,
    }))
}

fn text<'a>(op: &'a Value, key: &str) -> Result<&'a str> {
    op[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("[invalid-input] edit needs {key:?} as a string"))
}

fn optional_text<'a>(op: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match op.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => bail!("[invalid-input] edit field {key:?} must be a string"),
    }
}

fn strings(op: &Value, key: &str) -> Result<Vec<String>> {
    match op.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .with_context(|| format!("[invalid-input] edit field {key:?} must be strings")),
        Some(_) => bail!("[invalid-input] edit field {key:?} must be an array of strings"),
    }
}

fn flag(op: &Value, key: &str) -> Result<bool> {
    match op.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => bail!("[invalid-input] edit field {key:?} must be true or false"),
    }
}

fn number(value: &Value, key: &str) -> Result<f64> {
    value[key]
        .as_f64()
        .filter(|v| v.is_finite())
        .with_context(|| format!("[invalid-input] white_balance needs {key:?} as a number"))
}

/// Reject keys an operation does not use, so a typo is never a silent no-op.
fn only(op: &Value, keys: &[&str]) -> Result<()> {
    let object = op
        .as_object()
        .context("[invalid-input] an edit must be a JSON object")?;
    for key in object.keys() {
        if key != "op" && !keys.contains(&key.as_str()) {
            bail!(
                "[invalid-input] edit {} has no field {key:?}; it takes {}",
                op["op"],
                keys.join(", ")
            )
        }
    }
    Ok(())
}

fn white_balance(value: &Value) -> Result<catalog::WhiteBalance> {
    use catalog::WhiteBalance;
    Ok(match value["mode"].as_str().unwrap_or_default() {
        "as-shot" => WhiteBalance::AsShot,
        "temperature" => WhiteBalance::Temperature {
            temperature: number(value, "temperature")?,
            tint: value.get("tint").map_or(Ok(0.0), |_| number(value, "tint"))?,
        },
        "neutral" => {
            let n = value["neutral"]
                .as_array()
                .filter(|n| n.len() == 3)
                .and_then(|n| n.iter().map(Value::as_f64).collect::<Option<Vec<_>>>())
                .context("[invalid-input] white_balance neutral needs three numbers")?;
            WhiteBalance::Neutral([n[0], n[1], n[2]])
        }
        "sample" => WhiteBalance::Sample {
            x: number(value, "x")?,
            y: number(value, "y")?,
            radius: value.get("radius").map_or(Ok(2.0), |_| number(value, "radius"))?,
        },
        "suggest" => WhiteBalance::Suggest,
        other => bail!(
            "[invalid-input] white_balance mode {other:?} must be as-shot, temperature, neutral, sample or suggest"
        ),
    })
}

/// Photo targets for a sync: comma-separated `photo[/variant]` entries, or a
/// search query (every match's master).
fn sync_targets(raw: &Value, to: &str) -> Result<Vec<(String, String)>> {
    let entries: Vec<String> = if to.contains([':', '<', '>', '=', ' ', '*', '?']) {
        organize::select(raw, to)?
    } else {
        to.split(',').map(|e| e.trim().to_owned()).collect()
    };
    if entries.len() > variants::MAX_SYNC_TARGETS {
        bail!(
            "[limit-exceeded] {} targets exceed the limit of {} per sync",
            entries.len(),
            variants::MAX_SYNC_TARGETS
        )
    }
    entries.iter().map(|e| variants::split_target(e)).collect()
}

/// Apply one browser edit to `raw` in memory. `op` names the CLI command it
/// mirrors:
///
/// - `develop` (`raw develop`): `photo`, `variant`, `set` `{path: value}`,
///   `unset`, `white_balance`, `upright`, `auto_tone`;
/// - `rate`, `keyword` (`photo rate`, `photo keyword`): `selection`, then
///   `rating`/`pick`/`label` or `add`/`remove`/`clear`;
/// - `variant`, `snapshot`, `restore` (`photo variant add`, `photo snapshot
///   add|restore`);
/// - `sync` (`photo settings sync`): `source`, `to`, `groups`, `except`,
///   `auto_per_photo`;
/// - `paint` (`photo mask paint`): `samples` normalized to the uncropped frame
///   (0–1), `size` as a fraction of its long edge, optional `brush`.
///
/// On error `raw` is unchanged.
pub fn edit(raw: &mut Value, document: &Path, op: &Value) -> Result<Value> {
    let kind = text(op, "op")?;
    match kind {
        "develop" => {
            only(
                op,
                &[
                    "photo",
                    "variant",
                    "set",
                    "unset",
                    "white_balance",
                    "upright",
                    "auto_tone",
                ],
            )?;
            let set = match op.get("set") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Object(map)) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                Some(_) => bail!("[invalid-input] develop set must be an object of path: value"),
            };
            let changes = catalog::DevelopChanges {
                set,
                unset: strings(op, "unset")?,
                camera_profile: None,
                lens_profile: None,
                white_balance: op
                    .get("white_balance")
                    .filter(|v| !v.is_null())
                    .map(white_balance)
                    .transpose()?,
                upright: optional_text(op, "upright")?.map(str::to_owned),
                guides: Vec::new(),
                auto_tone: flag(op, "auto_tone")?,
            };
            if changes.is_empty() {
                bail!("[invalid-input] develop edit needs set, unset, white_balance, upright or auto_tone")
            }
            let variant = optional_text(op, "variant")?.unwrap_or("master");
            catalog::develop_raw(raw, document, text(op, "photo")?, variant, changes)
        }
        "rate" => {
            only(op, &["selection", "rating", "pick", "label"])?;
            let rating = match op.get("rating") {
                None | Some(Value::Null) => None,
                Some(v) => Some(
                    v.as_u64()
                        .context("[invalid-input] rating must be a whole number 0–5")?,
                ),
            };
            organize::rate(
                raw,
                &organize::Selection::parse(text(op, "selection")?),
                &organize::Rating {
                    rating,
                    pick: optional_text(op, "pick")?.map(str::to_owned),
                    label: optional_text(op, "label")?.map(str::to_owned),
                },
            )
        }
        "keyword" => {
            only(op, &["selection", "add", "remove", "clear"])?;
            organize::keywords(
                raw,
                &organize::Selection::parse(text(op, "selection")?),
                &strings(op, "add")?,
                &strings(op, "remove")?,
                flag(op, "clear")?,
            )
        }
        "variant" => {
            only(op, &["photo", "id", "from", "from_snapshot", "name"])?;
            let source = match (
                optional_text(op, "from")?,
                optional_text(op, "from_snapshot")?,
            ) {
                (Some(_), Some(_)) => {
                    bail!("[invalid-input] a variant copies from or from_snapshot, not both")
                }
                (_, Some(snapshot)) => variants::VariantSource::Snapshot(snapshot),
                (from, None) => variants::VariantSource::Variant(from.unwrap_or("master")),
            };
            variants::add_variant(
                raw,
                text(op, "photo")?,
                text(op, "id")?,
                source,
                optional_text(op, "name")?,
            )
        }
        "snapshot" => {
            only(op, &["photo", "variant", "id", "name"])?;
            variants::add_snapshot(
                raw,
                text(op, "photo")?,
                optional_text(op, "variant")?.unwrap_or("master"),
                text(op, "id")?,
                optional_text(op, "name")?,
            )
        }
        "restore" => {
            only(op, &["photo", "snapshot"])?;
            variants::restore_snapshot(raw, text(op, "photo")?, text(op, "snapshot")?)
        }
        "sync" => {
            only(op, &["source", "to", "groups", "except", "auto_per_photo"])?;
            let source = variants::split_target(text(op, "source")?)?;
            let targets = sync_targets(raw, text(op, "to")?)?;
            variants::sync_settings(
                raw,
                document,
                variants::SyncRequest {
                    source: (&source.0, &source.1),
                    targets: &targets,
                    groups: &strings(op, "groups")?,
                    except: &strings(op, "except")?,
                    auto_per_photo: flag(op, "auto_per_photo")?,
                },
            )
        }
        "paint" => {
            only(
                op,
                &[
                    "photo",
                    "variant",
                    "adjustment",
                    "component",
                    "samples",
                    "size",
                    "brush",
                    "erase",
                    "create",
                ],
            )?;
            match op.get("create").filter(|v| !v.is_null()) {
                None => paint_mask(raw, document, op),
                Some(create) => paint_new(raw, document, op, create),
            }
        }
        other => bail!(
            "[invalid-input] unknown edit op {other:?}; use develop, rate, keyword, variant, snapshot, restore, sync or paint"
        ),
    }
}

/// A placeholder first component, so a new adjustment validates before its
/// first brush component exists.
const PLACEHOLDER: &str =
    r#"{"kind": "radial", "mode": "add", "center": [0.5, 0.5], "radius": [0.1, 0.1]}"#;

/// Paint the first stroke of a new brush adjustment `create` (`{id, name?,
/// amount?, params?}`): add it with a placeholder component, paint a new brush
/// component, then drop the placeholder. All on a copy, so `raw` changes only
/// when every step succeeds.
fn paint_new(raw: &mut Value, document: &Path, op: &Value, create: &Value) -> Result<Value> {
    if op.get("adjustment").is_some() || op.get("component").is_some() {
        bail!("[invalid-input] paint with create names the adjustment in create.id; omit adjustment and component")
    }
    let id = text(create, "id")?;
    let photo = text(op, "photo")?;
    let variant = optional_text(op, "variant")?.unwrap_or("master");
    let mut adjustment = json!({
        "id": id,
        "mask": {"components": [serde_json::from_str::<Value>(PLACEHOLDER)?]},
        "params": create.get("params").cloned().unwrap_or(json!({})),
    });
    for key in ["name", "amount"] {
        if let Some(value) = create.get(key) {
            adjustment[key] = value.clone();
        }
    }
    let mut next = raw.clone();
    let develop = develop_mut(&mut next, photo, variant)?;
    let mut local = develop.get("local").cloned().unwrap_or_else(|| json!([]));
    local
        .as_array_mut()
        .context("[invalid-develop] develop.local must be an array")?
        .push(adjustment);
    develop.insert("local".into(), local);
    crate::scene::validate(&next)?;
    let mut stroke = op.clone();
    let fields = stroke
        .as_object_mut()
        .context("[invalid-input] an edit must be a JSON object")?;
    fields.remove("create");
    fields.insert("adjustment".into(), json!(id));
    let mut result = paint_mask(&mut next, document, &stroke)?;
    let develop = develop_mut(&mut next, photo, variant)?;
    let entry = develop["local"]
        .as_array_mut()
        .and_then(|list| list.iter_mut().find(|a| a["id"] == id))
        .context("[internal] new adjustment disappeared")?;
    entry["mask"]["components"]
        .as_array_mut()
        .context("[internal] new adjustment has no components")?
        .remove(0);
    crate::scene::validate(&next)?;
    result["component"] = json!(0);
    result["created"] = json!(true);
    *raw = next;
    Ok(result)
}

fn develop_mut<'a>(
    raw: &'a mut Value,
    photo: &str,
    variant: &str,
) -> Result<&'a mut Map<String, Value>> {
    raw["photography"]["photos"]
        .as_array_mut()
        .and_then(|photos| photos.iter_mut().find(|p| p["id"] == photo))
        .with_context(|| format!("[missing-resource] photo {photo} is not in the catalog"))?
        ["variants"]
        .as_array_mut()
        .and_then(|list| list.iter_mut().find(|v| v["id"] == variant))
        .with_context(|| format!("[missing-resource] variant {photo}/{variant} does not exist"))?
        .get_mut("develop")
        .and_then(Value::as_object_mut)
        .with_context(|| {
            format!("[invalid-develop] variant {photo}/{variant} has no develop object")
        })
}

/// The last brush component of an adjustment painted on `plane`: strokes
/// without an explicit component accumulate there, so erasing works and a
/// long painting session never runs into the component limit. A component
/// from an earlier frame size is left alone and a new one is started.
fn brush_component(
    raw: &Value,
    photo: &str,
    variant: &str,
    adjustment: &str,
    plane: (u32, u32),
) -> Option<usize> {
    let develop = raw["photography"]["photos"]
        .as_array()?
        .iter()
        .find(|p| p["id"] == photo)?["variants"]
        .as_array()?
        .iter()
        .find(|v| v["id"] == variant)?
        .get("develop")?;
    let components = develop["local"]
        .as_array()?
        .iter()
        .find(|a| a["id"] == adjustment)?["mask"]["components"]
        .as_array()?;
    components
        .iter()
        .rposition(|c| c["kind"] == "brush" && c["width"] == plane.0 && c["height"] == plane.1)
}

fn paint_mask(raw: &mut Value, document: &Path, op: &Value) -> Result<Value> {
    use crate::raster;
    let photo = text(op, "photo")?;
    let variant = optional_text(op, "variant")?.unwrap_or("master");
    crate::scene::validate(raw)?;
    let (width, height) = catalog::brush_plane(raw, document, photo, variant)?;
    let samples = op["samples"]
        .as_array()
        .filter(|s| (1..=MAX_STROKE_SAMPLES).contains(&s.len()))
        .with_context(|| format!("[invalid-input] paint needs 1–{MAX_STROKE_SAMPLES} samples"))?;
    let scaled = samples
        .iter()
        .map(|s| {
            let coordinate = |key: &str| {
                s[key]
                    .as_f64()
                    .filter(|v| (0.0..=1.0).contains(v))
                    .with_context(|| {
                        format!(
                            "[invalid-input] paint sample {key} must be 0–1 of the uncropped frame"
                        )
                    })
            };
            let mut point = Map::new();
            point.insert("x".into(), json!(coordinate("x")? * f64::from(width)));
            point.insert("y".into(), json!(coordinate("y")? * f64::from(height)));
            if let Some(pressure) = s.get("pressure") {
                point.insert("pressure".into(), pressure.clone());
            }
            Ok(Value::Object(point))
        })
        .collect::<Result<Vec<_>>>()?;
    let size = op["size"]
        .as_f64()
        .filter(|v| *v > 0.0 && *v <= 1.0)
        .context("[invalid-input] paint size must be a fraction (0–1] of the frame's long edge")?;
    let mut brush = match op.get("brush") {
        None | Some(Value::Null) => json!({}),
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(_) => bail!("[invalid-input] paint brush must be an object"),
    };
    brush["size"] = json!((size * f64::from(width.max(height))).max(1.0));
    let normalized = raster::normalize_input(&Value::Array(scaled))?;
    let summary = normalized.summary();
    let request = raster::StrokeRequest {
        brush: raster::Brush::parse(&raster::resolve_preset(raw, &brush, None)?)?,
        samples: normalized.samples,
        color: [0, 0, 0],
        blend: if flag(op, "erase")? {
            raster::Blend::Erase
        } else {
            raster::Blend::Normal
        },
        seed: 0,
        clone: None,
    };
    let adjustment = text(op, "adjustment")?;
    let component = match op.get("component") {
        None | Some(Value::Null) => {
            let found = brush_component(raw, photo, variant, adjustment, (width, height));
            if found.is_none() && request.blend == raster::Blend::Erase {
                bail!("[invalid-input] local adjustment {adjustment} has no brush component on the current {width}x{height} plane to erase from; paint a stroke first")
            }
            found
        }
        Some(v) => Some(
            v.as_u64()
                .context("[invalid-input] paint component must be an index")? as usize,
        ),
    };
    let mut result = catalog::paint_mask(
        raw,
        document,
        photo,
        variant,
        catalog::MaskStroke {
            adjustment: adjustment.to_owned(),
            component,
            request,
        },
    )?;
    result["input"] = summary;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlays_parse_strictly() {
        assert_eq!(Overlay::parse("").unwrap(), Overlay::None);
        assert_eq!(Overlay::parse("gamut").unwrap(), Overlay::Gamut);
        assert_eq!(
            Overlay::parse("mask:sky").unwrap(),
            Overlay::Mask("sky".into())
        );
        for bad in ["mask:", "mask:-x", "zebra"] {
            assert!(Overlay::parse(bad)
                .unwrap_err()
                .to_string()
                .contains("[invalid-input]"));
        }
    }

    #[test]
    fn previews_fit_without_enlarging() {
        assert_eq!(fit(6000, 4000, 1600), (1600, 1067));
        assert_eq!(fit(4000, 6000, 1600), (1067, 1600));
        assert_eq!(fit(800, 600, 1600), (800, 600));
        assert_eq!(fit(10000, 1, 100), (100, 1));
    }

    #[test]
    fn clipping_classes_and_tint_paint_only_opaque_pixels() {
        let samples = vec![
            255, 10, 10, 255, // highlight
            0, 50, 50, 255, // shadow
            30, 40, 50, 255, // neither
            255, 255, 255, 0, // transparent
        ];
        let mut raster = Raster::new(4, 1, true, Samples::Eight(samples)).unwrap();
        let counts = paint(
            &mut raster,
            |_, c| clip_class(c),
            Some(&|c| if c == 1 { HIGHLIGHT } else { SHADOW }),
        )
        .unwrap();
        assert_eq!(counts, (1, 1));
        let Samples::Eight(codes) = &raster.samples else {
            unreachable!()
        };
        assert_eq!(&codes[..3], &HIGHLIGHT);
        assert_eq!(&codes[4..7], &SHADOW);
        assert_eq!(&codes[8..12], &[30, 40, 50, 255]);
        assert_eq!(&codes[12..], &[255, 255, 255, 0]);
        // A 2x1 weight plane sampled onto 4 pixels: the left half is masked.
        let marked = tint(&mut raster, &[1.0, 0.0], (2, 1)).unwrap();
        assert_eq!(marked, 2);
        assert!(tint(&mut raster, &[1.0], (2, 1)).is_err());
    }
}

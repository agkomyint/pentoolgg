//! Version 6 compositing contract. Source assets and operation engine v1 are immutable.
use anyhow::{bail, Context, Result};
use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
mod adjustment;
mod effect;
thread_local! { static CANCEL: std::cell::RefCell<Option<Arc<AtomicBool>>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
thread_local! { static TRIP: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub(crate) fn check_cancelled() -> Result<()> {
    #[cfg(test)]
    if TRIP.with(|t| {
        let left = t.get();
        t.set(left.saturating_sub(1));
        left == 1
    }) {
        bail!("[cancelled] cancelled at a test checkpoint")
    }
    if CANCEL.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }) {
        bail!("[cancelled] composite preview cancelled")
    }
    Ok(())
}

/// A finished node surface kept between renders, cropped to its nonzero pixels,
/// with the node's pixel-work and memory accounting relative to where it started.
struct CachedSurface {
    x: u32,
    y: u32,
    pixels: RgbaImage,
    work: u64,
    work_peak: u64,
    live: u64,
    live_peak: u64,
    used: u64,
}

#[derive(Default)]
struct NodeCache {
    limit: usize,
    bytes: usize,
    clock: u64,
    entries: HashMap<[u8; 32], CachedSurface>,
}

thread_local! { static NODE_CACHE: std::cell::RefCell<Option<NodeCache>> = const { std::cell::RefCell::new(None) }; }

/// Keep finished node surfaces between renders on this thread, up to `limit_bytes`
/// of pixels (least recently used first out); 0 turns the cache off. Studio's
/// browser core opts in so that editing one layer re-renders only that layer;
/// exports never do. A hit is the same bytes and the same budget accounting as a
/// fresh render, so output and limit errors do not change.
pub fn cache_node_surfaces(limit_bytes: usize) {
    NODE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        match (cache.as_mut(), limit_bytes) {
            (_, 0) => *cache = None,
            (Some(existing), limit) => existing.limit = limit,
            (None, limit) => {
                *cache = Some(NodeCache {
                    limit,
                    ..NodeCache::default()
                })
            }
        }
    });
}

/// Everything outside the node that a cached node surface depends on: the
/// document apart from its pages, the page canvas size, the document path, and
/// the render size. None when the cache is off.
fn node_context(
    raw: &Value,
    page: &Value,
    document: &Path,
    scale: f32,
    size: (u32, u32),
) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    if NODE_CACHE.with(|cache| cache.borrow().is_none()) {
        return None;
    }
    let mut hasher = Sha256::new();
    for (key, value) in raw.as_object()? {
        if key != "pages" {
            serde_json::to_writer(&mut hasher, key).ok()?;
            serde_json::to_writer(&mut hasher, value).ok()?;
        }
    }
    serde_json::to_writer(&mut hasher, &page["canvas"]["width"]).ok()?;
    serde_json::to_writer(&mut hasher, &page["canvas"]["height"]).ok()?;
    hasher.update(document.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(scale.to_bits().to_le_bytes());
    hasher.update(size.0.to_le_bytes());
    hasher.update(size.1.to_le_bytes());
    Some(hasher.finalize().into())
}

impl CachedSurface {
    /// The entry as stored by [`crate::render_cache`]: position, size and
    /// accounting as little-endian integers, then the pixels.
    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(48 + self.pixels.as_raw().len());
        for value in [self.x, self.y, self.pixels.width(), self.pixels.height()] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.work, self.work_peak, self.live, self.live_peak] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(self.pixels.as_raw());
        bytes
    }

    /// An encoded entry that fits a `width` x `height` render.
    fn decode(mut bytes: Vec<u8>, width: u32, height: u32) -> Option<Self> {
        if bytes.len() < 48 {
            return None;
        }
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let long = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        let (x, y, w, h) = (word(0), word(4), word(8), word(12));
        let fits = u64::from(x) + u64::from(w) <= u64::from(width)
            && u64::from(y) + u64::from(h) <= u64::from(height);
        if !fits || (bytes.len() - 48) as u64 != u64::from(w) * u64::from(h) * 4 {
            return None;
        }
        let (work, work_peak, live, live_peak) = (long(16), long(24), long(32), long(40));
        bytes.drain(..48);
        Some(Self {
            x,
            y,
            pixels: RgbaImage::from_raw(w, h, bytes)?,
            work,
            work_peak,
            live,
            live_peak,
            used: 0,
        })
    }
}

/// Store `image` cropped to its nonzero pixels (everything else is [0, 0, 0, 0],
/// so expanding the crop restores it exactly), on disk too when `disk` is
/// given, then evict to the byte limit.
fn remember(key: [u8; 32], image: &RgbaImage, mut entry: CachedSurface, disk: Option<&Path>) {
    let (w, h) = image.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (y, row) in image.as_raw().chunks_exact(w as usize * 4).enumerate() {
        let mut pixels = row.chunks_exact(4).map(|p| p != [0, 0, 0, 0]);
        if let Some(first) = pixels.position(|set| set) {
            let last = row
                .chunks_exact(4)
                .rposition(|p| p != [0, 0, 0, 0])
                .unwrap_or(first);
            x0 = x0.min(first as u32);
            x1 = x1.max(last as u32 + 1);
            y0 = y0.min(y as u32);
            y1 = y1.max(y as u32 + 1);
        }
    }
    if x1 > x0 && y1 > y0 {
        entry.x = x0;
        entry.y = y0;
        entry.pixels = image::imageops::crop_imm(image, x0, y0, x1 - x0, y1 - y0).to_image();
    }
    if let Some(directory) = disk {
        crate::render_cache::write(directory, &key, &entry.encode());
    }
    store(key, entry);
}

/// Keep `entry` in this thread's node cache, then evict to the byte limit.
fn store(key: [u8; 32], mut entry: CachedSurface) {
    NODE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let Some(cache) = cache.as_mut() else { return };
        let size = entry.pixels.as_raw().len() + 128;
        if size > cache.limit {
            return;
        }
        cache.clock += 1;
        entry.used = cache.clock;
        if let Some(old) = cache.entries.insert(key, entry) {
            cache.bytes -= old.pixels.as_raw().len() + 128;
        }
        cache.bytes += size;
        while cache.bytes > cache.limit {
            let Some(oldest) = cache
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k)
            else {
                break;
            };
            let old = cache.entries.remove(&oldest).unwrap();
            cache.bytes -= old.pixels.as_raw().len() + 128;
        }
    });
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn with_cancel<T>(
    flag: Arc<AtomicBool>,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    struct Reset(Option<Arc<AtomicBool>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            CANCEL.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let old = CANCEL.with(|c| c.borrow_mut().replace(flag));
    let _reset = Reset(old);
    check_cancelled()?;
    operation()
}
/// Test hook: run `operation` with cancellation at its `n`th check on this
/// thread (1-based). Returns the result and whether that check was reached.
/// Pixels a plain, unstyled rectangle or ellipse can touch (padded for
/// antialiasing); every other node is charged the full canvas.
fn plain_node_pixels(node: &Value, canvas: u64, scale: f32) -> u64 {
    let plain = matches!(node["kind"].as_str(), Some("rect" | "ellipse"))
        && ["effects", "transforms", "mask", "rotation", "skew"]
            .iter()
            .all(|key| {
                node.get(*key)
                    .is_none_or(|v| v.is_null() || v.as_array().is_some_and(Vec::is_empty))
            });
    let (Some(w), Some(h)) = (node["width"].as_f64(), node["height"].as_f64()) else {
        return canvas;
    };
    if !plain || !w.is_finite() || !h.is_finite() || w < 0.0 || h < 0.0 {
        return canvas;
    }
    // Stroke width and a generous pad cover outside strokes and AA fringes.
    let sw = &node["style"]["stroke_width"];
    let stroke = match sw {
        Value::Null => 0.0,
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        other => match other["fallback"].as_f64() {
            Some(v) => v,
            None => return canvas,
        },
    }
    .max(0.0);
    let pad = 2.0 * stroke + 4.0;
    let scale = f64::from(scale);
    let pixels = (w + pad) * (h + pad) * scale * scale;
    if pixels.is_finite() {
        (pixels.ceil() as u64).min(canvas)
    } else {
        canvas
    }
}

#[cfg(test)]
pub(crate) fn cancel_at_check<T>(n: usize, operation: impl FnOnce() -> T) -> (T, bool) {
    TRIP.with(|t| t.set(n));
    let result = operation();
    let reached = TRIP.with(|t| t.replace(0)) == 0;
    (result, reached)
}

pub const VERSION: u64 = 6;
pub const ENGINE: u64 = 1;
pub const MAX_ADJUSTMENTS: usize = 64;
pub const MAX_TEMP_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_PIXEL_WORK: u64 = 1_073_741_824;

pub(crate) fn settings(settings: &Value, allowed: &[&str]) -> Result<()> {
    let values = settings
        .as_object()
        .context("operation settings must be an object")?;
    for key in values.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("unknown or inapplicable operation setting {key}")
        }
    }
    Ok(())
}

/// Composite operations use exactly the same planners as individual commands.
pub fn batch_operation(
    raw: &mut Value,
    page: Option<&str>,
    document: &Path,
    kind: &str,
    operation: &Value,
    resolve: &dyn Fn(&str) -> Result<String>,
) -> Result<Value> {
    let settings = operation.get("settings").cloned().unwrap_or(json!({}));
    let text = |key: &str| {
        operation
            .get(key)
            .and_then(Value::as_str)
            .with_context(|| format!("{kind} requires {key}"))
    };
    if let Some(action) = kind.strip_prefix("adjustment-") {
        return edit(raw, page, action, &resolve("id")?, &settings);
    }
    if let Some(action) = kind.strip_prefix("effect-") {
        return crate::effects::edit(
            raw,
            page,
            &resolve("id")?,
            action,
            text("op_id")?,
            &settings,
        );
    }
    if let Some(action) = kind.strip_prefix("transform-") {
        if action == "metadata" {
            return crate::transform::metadata(
                raw,
                page,
                &resolve("id")?,
                text("op_id")?,
                &settings,
            );
        }
        return crate::transform::edit(
            raw,
            page,
            &resolve("id")?,
            action,
            text("op_id")?,
            operation["kind"].as_str(),
            &operation.get("params").cloned().unwrap_or(json!({})),
            operation["index"].as_u64().map(|v| v as usize),
        );
    }
    if let Some(action) = kind.strip_prefix("fill-") {
        return crate::fill::edit(raw, page, action, &resolve("id")?, &settings);
    }
    match kind {
        "node-move" => {
            let id = resolve("id")?;
            let index = operation["index"]
                .as_u64()
                .context("node-move requires index")? as usize;
            let mut candidate = raw.clone();
            let selected = crate::scene::page_mut(&mut candidate, page)?;
            ensure_unlocked(selected, &id)?;
            fn reorder(nodes: &mut Vec<Value>, id: &str, index: usize) -> Result<bool> {
                if let Some(old) = nodes.iter().position(|n| n["id"] == id) {
                    if index >= nodes.len() {
                        bail!("node index out of range")
                    };
                    let node = nodes.remove(old);
                    nodes.insert(index, node);
                    return Ok(true);
                }
                for node in nodes {
                    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                        if reorder(children, id, index)? {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            }
            let mut found = false;
            for layer in selected["layers"].as_array_mut().unwrap() {
                if reorder(layer["nodes"].as_array_mut().unwrap(), &id, index)? {
                    found = true;
                    break;
                }
            }
            if !found {
                bail!("node missing")
            }
            crate::scene::validate(&candidate)?;
            *raw = candidate;
            Ok(json!({"id":id,"index":index}))
        }
        "linked-embed" | "linked-relink" => crate::linked::edit(
            raw,
            document,
            kind.strip_prefix("linked-").unwrap(),
            text("asset")?,
            operation["path"].as_str().map(Path::new),
            true,
        ),
        "mask-create" => crate::mask::create(raw, document, page, text("name")?, text("from")?),
        "mask-attach" => crate::mask::attach(raw, page, &resolve("id")?, text("name")?, &settings),
        "mask-detach" => crate::mask::detach(raw, page, &resolve("id")?),
        "mask-delete" => crate::mask::delete(raw, text("name")?),
        "mask-apply" => crate::mask::apply(raw, document, page, &resolve("id")?),
        "clip-add" | "clip-remove" => clip_edit(
            raw,
            page,
            kind.strip_prefix("clip-").unwrap(),
            &resolve("id")?,
            operation["base"].as_str(),
        ),
        "selection-save" => crate::selection::save(
            raw,
            document,
            page,
            text("name")?,
            operation.get("query").context("selection query missing")?,
        ),
        "selection-crop" => crate::selection::crop(
            raw,
            document,
            page,
            operation.get("query").context("selection query missing")?,
        ),
        "appearance-paste" => crate::preset::apply(
            raw,
            page,
            &resolve("id")?,
            operation.get("preset").context("preset missing")?,
        ),
        "composite-set" => {
            let mut candidate = raw.clone();
            let selected = crate::scene::page_mut(&mut candidate, page)?;
            let id = resolve("id")?;
            ensure_unlocked(selected, &id)?;
            let node = crate::scene::find_node_mut(selected, &id).context("node missing")?;
            let settings = settings.as_object().context("settings must be an object")?;
            if settings.is_empty() {
                bail!("composite-set requires settings")
            }
            for (key, value) in settings {
                if ![
                    "opacity",
                    "content_opacity",
                    "blend_mode",
                    "blend_space",
                    "isolation",
                    "effects",
                ]
                .contains(&key.as_str())
                {
                    bail!("unsupported composite property {key}")
                }
                node[key] = value.clone();
            }
            crate::scene::validate(&candidate)?;
            *raw = candidate;
            Ok(json!({"id":id,"settings":settings}))
        }
        _ => bail!("unsupported composite batch operation {kind}"),
    }
}
pub const ADJUSTMENTS: [&str; 13] = [
    "exposure",
    "brightness-contrast",
    "levels",
    "curves",
    "vibrance",
    "hue-saturation",
    "color-balance",
    "black-and-white",
    "channel-mixer",
    "gradient-map",
    "invert",
    "posterize",
    "threshold",
];

/// Scene versions 6 through 8 carry compositing.
pub fn is_document(raw: &Value) -> bool {
    matches!(
        raw.get("version").and_then(Value::as_u64),
        Some(
            VERSION
                | crate::photo::VERSION
                | crate::color::VERSION
                | crate::spreads::VERSION
                | crate::scene3d::node::VERSION
                | crate::scene3d::node::BINDINGS_VERSION
                | crate::scene::PAINT_VERSION
        )
    )
}

pub fn migrate(mut raw: Value) -> Result<Value> {
    if crate::color::has_v8(&raw) {
        raw = crate::color::catalog::downgrade(raw)?;
    }
    if raw.get("version").and_then(Value::as_u64) == Some(crate::photo::VERSION) {
        return crate::photo::catalog::downgrade(raw);
    }
    if is_document(&raw) {
        crate::scene::validate(&raw)?;
        return Ok(raw);
    }
    let mut result = crate::scene::migrate_to_v5(raw)?;
    result["version"] = json!(VERSION);
    result["compositing"] = json!({"engine": ENGINE, "color_space": "srgb8"});
    crate::scene::validate(&result)?;
    Ok(result)
}

/// Downgrade only an unextended v6 scene. Never demote required compositing data.
pub fn downgrade(mut raw: Value) -> Result<Value> {
    if crate::color::has_v8(&raw) {
        raw = crate::color::catalog::downgrade(raw)?;
    }
    if raw.get("version").and_then(Value::as_u64) == Some(crate::photo::VERSION) {
        raw = crate::photo::catalog::downgrade(raw)?;
    }
    crate::scene::validate(&raw)?;
    fn check(value: &Value) -> Result<()> {
        match value {
            Value::Object(object) => {
                if object.get("clipping").is_some()
                    || object
                        .get("transforms")
                        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
                    || object.get("effects").is_some()
                    || object.get("content_opacity").is_some()
                    || object.get("isolation").is_some()
                    || object.get("blend_space").is_some()
                    || object.get("blend_mode").is_some_and(|v| v != "normal")
                    || (!matches!(
                        object.get("kind").and_then(Value::as_str),
                        Some("image" | "raster")
                    ) && object
                        .get("opacity")
                        .is_some_and(|v| v.as_f64() != Some(1.0)))
                {
                    bail!("[unsupported-capability] version 5 cannot represent required compositing properties; detach or explicitly bake them first")
                }
                if object
                    .get("mask")
                    .is_some_and(|m| m.get("resource").is_some())
                {
                    bail!("[unsupported-capability] version 5 cannot represent reusable masks; detach or explicitly apply them first")
                }
                if object.get("kind").and_then(Value::as_str) == Some("adjustment") {
                    bail!("[unsupported-capability] version 5 cannot represent adjustment node {}; remove or explicitly bake it first", object["id"])
                }
                if object.get("kind").and_then(Value::as_str) == Some("fill") {
                    bail!("[unsupported-capability] version 5 cannot represent fill nodes")
                }
                if object.get("kind").and_then(Value::as_str) == Some("raster") {
                    bail!("[unsupported-capability] version 5 cannot represent raster layer {}; export it as an image first", object["id"])
                }
                for child in object.values() {
                    check(child)?;
                }
            }
            Value::Array(values) => {
                for child in values {
                    check(child)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    check(&raw)?;
    raw["version"] = json!(crate::image::VERSION);
    raw.as_object_mut().unwrap().remove("compositing");
    crate::scene::validate(&raw)?;
    Ok(raw)
}

fn number(params: &Value, key: &str, default: f64, min: f64, max: f64) -> Result<f64> {
    let n = match params.get(key) {
        Some(v) => v
            .as_f64()
            .context("adjustment parameter must be a number")?,
        None => default,
    };
    if !n.is_finite() || !(min..=max).contains(&n) {
        bail!("[invalid-adjustment] {key} must be finite and in {min}..={max}")
    }
    Ok(n)
}

pub fn validate_settings(kind: &str, params: &Value) -> Result<()> {
    let object = params
        .as_object()
        .context("[invalid-adjustment] params must be an object")?;
    if !ADJUSTMENTS.contains(&kind) {
        bail!(
            "[unsupported-capability] unknown adjustment {kind}; valid kinds: {}",
            ADJUSTMENTS.join(", ")
        )
    }
    if matches!(
        kind,
        "brightness-contrast" | "levels" | "curves" | "hue-saturation"
    ) {
        return crate::imageops::validate_operation(&json!({
            "id":"adjustment", "kind":kind, "version":1, "enabled":true, "params":params
        }));
    }
    let allowed: &[&str] = match kind {
        "exposure" => &["stops"],
        "vibrance" => &["amount"],
        "color-balance" => &["red", "green", "blue"],
        "black-and-white" => &["red", "green", "blue"],
        "channel-mixer" => &["matrix"],
        "gradient-map" => &["stops"],
        "posterize" => &["levels"],
        "threshold" => &["level"],
        _ => &[],
    };
    let unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| !allowed.contains(key))
        .collect();
    if !unknown.is_empty() {
        bail!(
            "[invalid-adjustment] unknown {kind} parameter {}; valid parameters: {}",
            unknown.join(", "),
            if allowed.is_empty() {
                "(none)".to_string()
            } else {
                allowed.join(", ")
            }
        )
    }
    match kind {
        "exposure" => {
            number(params, "stops", 0.0, -16.0, 16.0)?;
        }
        "vibrance" => {
            number(params, "amount", 0.0, -100.0, 100.0)?;
        }
        "color-balance" => {
            for key in ["red", "green", "blue"] {
                number(params, key, 0.0, -100.0, 100.0)?;
            }
        }
        "black-and-white" => {
            let mut total = 0.0;
            for (key, default) in [("red", 0.2126), ("green", 0.7152), ("blue", 0.0722)] {
                total += number(params, key, default, 0.0, 1.0)?;
            }
            if (total - 1.0).abs() > 1e-9 {
                bail!("[invalid-adjustment] black-and-white weights must sum to 1")
            }
        }
        "channel-mixer" => {
            mixer(params)?;
        }
        "gradient-map" => {
            gradient(params)?;
        }
        "posterize" => {
            if number(params, "levels", 2.0, 2.0, 256.0)?.fract() != 0.0 {
                bail!("[invalid-adjustment] posterize levels must be an integer")
            }
        }
        "threshold" => {
            number(params, "level", 128.0, 0.0, 255.0)?;
        }
        _ => {}
    }
    Ok(())
}

fn mixer(params: &Value) -> Result<[[f64; 4]; 3]> {
    let rows = params["matrix"]
        .as_array()
        .filter(|a| a.len() == 3)
        .context("[invalid-adjustment] channel matrix must have three rows")?;
    let mut result = [[0.0; 4]; 3];
    for (row, values) in rows.iter().enumerate() {
        let values = values
            .as_array()
            .filter(|a| a.len() == 4)
            .context("[invalid-adjustment] each channel row must contain R,G,B,offset")?;
        for (col, value) in values.iter().enumerate() {
            let v = value
                .as_f64()
                .filter(|v| v.is_finite() && (-2.0..=2.0).contains(v))
                .context("[invalid-adjustment] channel coefficients must be in -2..=2")?;
            result[row][col] = v;
        }
    }
    Ok(result)
}

fn gradient(params: &Value) -> Result<Vec<(f64, [f64; 3])>> {
    let stops = params["stops"]
        .as_array()
        .filter(|a| (2..=16).contains(&a.len()))
        .context("[invalid-adjustment] gradient map requires 2..=16 stops")?;
    let mut result = Vec::with_capacity(stops.len());
    for stop in stops {
        if stop.get("position").is_none() && stop.get("offset").is_some() {
            bail!("[invalid-adjustment] gradient stops use \"position\", not \"offset\"");
        }
        let position = stop["position"]
            .as_f64()
            .filter(|v| (0.0..=1.0).contains(v))
            .context("[invalid-adjustment] gradient stop \"position\" must be a number in 0..=1")?;
        let rgb = stop["rgb"]
            .as_array()
            .filter(|a| a.len() == 3)
            .context("[invalid-adjustment] gradient rgb must contain three channels")?;
        let mut color = [0.0; 3];
        for (i, c) in rgb.iter().enumerate() {
            color[i] = c
                .as_f64()
                .filter(|v| (0.0..=255.0).contains(v))
                .context("[invalid-adjustment] gradient channels must be in 0..=255")?;
        }
        result.push((position, color));
    }
    if result[0].0 != 0.0
        || result.last().unwrap().0 != 1.0
        || result.windows(2).any(|w| w[0].0 >= w[1].0)
    {
        bail!("[invalid-adjustment] gradient stops must strictly increase from 0 to 1")
    }
    Ok(result)
}

fn round(value: f64) -> u8 {
    (value + 0.5).floor().clamp(0.0, 255.0) as u8
}
fn luminance(rgb: [f64; 3]) -> f64 {
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

/// Adjustment engine v1: straight sRGB8, immutable alpha, half-up rounding.
pub fn adjust(image: &mut RgbaImage, kind: &str, params: &Value, opacity: f64) -> Result<()> {
    validate_settings(kind, params)?;
    if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
        bail!("[invalid-adjustment] opacity must be in 0..=1")
    }
    if opacity == 0.0 {
        return Ok(());
    }
    if matches!(
        kind,
        "brightness-contrast" | "levels" | "curves" | "hue-saturation"
    ) {
        let adjusted = crate::imageops::apply_stack(
            image.clone(),
            &[json!({
                "id":"adjustment", "kind":kind, "version":1, "enabled":true, "params":params
            })],
        )?;
        for (pixel, result) in image.pixels_mut().zip(adjusted.pixels()) {
            if pixel[3] == 0 {
                continue;
            }
            for i in 0..3 {
                pixel[i] =
                    round(f64::from(pixel[i]) * (1.0 - opacity) + f64::from(result[i]) * opacity);
            }
        }
        return Ok(());
    }
    let get = |key: &str, default: f64| params.get(key).and_then(Value::as_f64).unwrap_or(default);
    let matrix = if kind == "channel-mixer" {
        Some(mixer(params)?)
    } else {
        None
    };
    let stops = if kind == "gradient-map" {
        Some(gradient(params)?)
    } else {
        None
    };
    // Fractional powers use the same deterministic arithmetic as operation engine v1.
    let exposure = crate::imageops::det_pow(0.5, get("stops", 0.0).abs());
    let exposure = if get("stops", 0.0) > 0.0 {
        1.0 / exposure
    } else {
        exposure
    };
    for pixel in image.pixels_mut() {
        if pixel[3] == 0 {
            continue;
        }
        let rgb = [
            f64::from(pixel[0]),
            f64::from(pixel[1]),
            f64::from(pixel[2]),
        ];
        let mut out = rgb;
        match kind {
            "exposure" => {
                for i in 0..3 {
                    out[i] = rgb[i] * exposure;
                }
            }
            "invert" => {
                for i in 0..3 {
                    out[i] = 255.0 - rgb[i];
                }
            }
            "posterize" => {
                let n = get("levels", 2.0) - 1.0;
                for i in 0..3 {
                    out[i] = (rgb[i] * n / 255.0 + 0.5).floor() * 255.0 / n;
                }
            }
            "threshold" => {
                out = [if luminance(rgb) >= get("level", 128.0) {
                    255.0
                } else {
                    0.0
                }; 3];
            }
            "color-balance" => {
                for (i, key) in ["red", "green", "blue"].iter().enumerate() {
                    out[i] = rgb[i] + get(key, 0.0) * 2.55;
                }
            }
            "black-and-white" => {
                out = [rgb[0] * get("red", 0.2126)
                    + rgb[1] * get("green", 0.7152)
                    + rgb[2] * get("blue", 0.0722); 3];
            }
            "channel-mixer" => {
                for (i, row) in matrix.as_ref().unwrap().iter().enumerate() {
                    out[i] = rgb[0] * row[0] + rgb[1] * row[1] + rgb[2] * row[2] + 255.0 * row[3];
                }
            }
            "vibrance" => {
                let saturation =
                    (rgb[0].max(rgb[1]).max(rgb[2]) - rgb[0].min(rgb[1]).min(rgb[2])) / 255.0;
                let factor = 1.0 + get("amount", 0.0) / 100.0 * (1.0 - saturation);
                let y = luminance(rgb);
                for i in 0..3 {
                    out[i] = y + (rgb[i] - y) * factor;
                }
            }
            "gradient-map" => {
                let t = luminance(rgb) / 255.0;
                for pair in stops.as_ref().unwrap().windows(2) {
                    if t <= pair[1].0 {
                        let f = (t - pair[0].0) / (pair[1].0 - pair[0].0);
                        for (i, c) in out.iter_mut().enumerate() {
                            *c = pair[0].1[i] + (pair[1].1[i] - pair[0].1[i]) * f;
                        }
                        break;
                    }
                }
            }
            _ => unreachable!("validated adjustment"),
        }
        for i in 0..3 {
            pixel[i] = round(rgb[i] * (1.0 - opacity) + f64::from(round(out[i])) * opacity);
        }
    }
    Ok(())
}

pub fn validate_node(node: &Value) -> Result<()> {
    if node["enabled"].as_bool().is_none() {
        bail!("[invalid-adjustment] enabled must be a boolean")
    }
    number(node, "opacity", 1.0, 0.0, 1.0)?;
    crate::blend::validate(
        node["blend_mode"]
            .as_str()
            .context("adjustment blend_mode required")?,
        node["blend_space"].as_str().unwrap_or("srgb"),
    )?;
    let kind = node["adjustment"]
        .as_str()
        .context("[invalid-adjustment] adjustment kind is missing")?;
    validate_settings(kind, &node["params"])?;
    let scope = node["scope"]
        .as_object()
        .context("[invalid-adjustment] scope must be an object")?;
    match scope.get("kind").and_then(Value::as_str) {
        Some("below") => {}
        Some("group") => {
            scope
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .context("[invalid-adjustment] scope group id is missing")?;
        }
        Some("targets") => {
            let targets = scope
                .get("ids")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty() && a.len() <= 1000)
                .context("[invalid-adjustment] targets must contain 1..=1000 IDs")?;
            let mut seen = HashSet::new();
            for target in targets {
                let id = target
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .context("[invalid-adjustment] target ID must be nonempty")?;
                if !seen.insert(id) {
                    bail!("[invalid-adjustment] duplicate target {id}")
                }
            }
        }
        _ => bail!("[invalid-adjustment] scope kind must be below, group, or targets"),
    }
    if let Some(mask) = node.get("mask") {
        crate::mask::validate_attachment(mask)?;
    }
    Ok(())
}

pub fn validate(raw: &Value) -> Result<()> {
    crate::mask::validate(raw)?;
    if raw["compositing"]["engine"] != ENGINE || raw["compositing"]["color_space"] != "srgb8" {
        bail!("[unsupported-capability] v6 requires compositing engine 1 and srgb8")
    }
    fn siblings(nodes: &[Value], count: &mut usize, raw: &Value) -> Result<()> {
        let mut prior = HashMap::new();
        let mut clip_base: Option<&str> = None;
        for node in nodes {
            if node["kind"] == "adjustment"
                && (node.get("transforms").is_some()
                    || node.get("effects").is_some()
                    || node.get("content_opacity").is_some())
            {
                bail!("[invalid-composite] adjustments cannot carry content transforms, effects, or content opacity")
            }
            crate::effects::validate(raw, node)?;
            if node["kind"] == "fill" {
                crate::fill::validate(raw, node)?;
            }
            crate::transform::validate(node)?;
            number(node, "opacity", 1.0, 0.0, 1.0)?;
            for key in ["blend_mode", "blend_space"] {
                if node.get(key).is_some_and(|v| !v.is_string()) {
                    bail!("[invalid-composite] {key} must be a string")
                }
            }
            crate::blend::validate(
                node.get("blend_mode")
                    .and_then(Value::as_str)
                    .unwrap_or("normal"),
                node.get("blend_space")
                    .and_then(Value::as_str)
                    .unwrap_or("srgb"),
            )?;
            if let Some(isolation) = node.get("isolation") {
                if node["kind"] != "group"
                    || !matches!(isolation.as_str(), Some("isolated" | "pass-through"))
                {
                    bail!("[invalid-composite] isolation applies to groups and must be isolated or pass-through")
                }
                if isolation == "pass-through"
                    && (node.get("mask").is_some()
                        || node.get("effects").is_some()
                        || node.get("transforms").is_some()
                        || node.get("clipping").is_some()
                        || number(node, "opacity", 1.0, 0.0, 1.0)? != 1.0
                        || node
                            .get("blend_mode")
                            .and_then(Value::as_str)
                            .unwrap_or("normal")
                            != "normal")
                {
                    bail!("[invalid-composite] masked, clipped, translucent or blended groups must use isolated compositing")
                }
            }
            if let Some(clipping) = node.get("clipping") {
                let base = clipping["base"]
                    .as_str()
                    .context("[invalid-clipping] base ID missing")?;
                if node["kind"] == "adjustment" || clip_base != Some(base) {
                    bail!("[invalid-clipping] node {} must be a following sibling of base {base} (immediately after it or its clipping members); detach clipping before reordering",node["id"])
                }
            } else if node["kind"] == "adjustment" {
                // An adjustment scoped to the current base is clipped to it, so the
                // clipping group continues above it; any other adjustment ends it.
                let ids = scope_ids(node);
                if node["scope"]["kind"] != "targets"
                    || ids.is_empty()
                    || ids.iter().any(|id| Some(*id) != clip_base)
                {
                    clip_base = None;
                }
            } else {
                clip_base = node["id"].as_str();
            }
            if node["kind"] == "adjustment" {
                *count += 1;
                validate_node(node)?;
                let targets = scope_ids(node);
                for id in targets {
                    let target = prior
                        .get(id)
                        .context("[invalid-adjustment] scope must reference a preceding sibling")?;
                    if *target == "adjustment"
                        || (node["scope"]["kind"] == "group" && *target != "group")
                    {
                        bail!("[invalid-adjustment] scope {id} does not reference the required content kind")
                    }
                }
            }
            prior.insert(
                node["id"].as_str().unwrap_or_default(),
                node["kind"].as_str().unwrap_or_default(),
            );
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                siblings(children, count, raw)?;
            }
            if let Some(fallback) = node.get("fallback") {
                siblings(std::slice::from_ref(fallback), count, raw)?;
            }
        }
        Ok(())
    }
    for page in raw["pages"]
        .as_array()
        .context("document pages are missing")?
    {
        let mut count = 0;
        for layer in page["layers"]
            .as_array()
            .context("page layers are missing")?
        {
            siblings(
                layer["nodes"]
                    .as_array()
                    .context("layer nodes are missing")?,
                &mut count,
                raw,
            )?;
        }
        if count > MAX_ADJUSTMENTS {
            bail!("[limit-exceeded] page may contain at most {MAX_ADJUSTMENTS} adjustments")
        }
    }
    if let Some(components) = raw.get("components").and_then(Value::as_array) {
        for component in components {
            if let Some(root) = component.get("root") {
                siblings(std::slice::from_ref(root), &mut 0, raw)?;
            }
        }
    }
    Ok(())
}

fn scope_ids(node: &Value) -> Vec<&str> {
    match node["scope"]["kind"].as_str() {
        Some("group") => node["scope"]["id"].as_str().into_iter().collect(),
        Some("targets") => node["scope"]["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn parse_scope(scope: &str) -> Result<Value> {
    if scope == "below" {
        return Ok(json!({"kind":"below"}));
    }
    if let Some(id) = scope.strip_prefix("group:").filter(|id| !id.is_empty()) {
        return Ok(json!({"kind":"group", "id":id}));
    }
    if let Some(ids) = scope.strip_prefix("ids:") {
        return Ok(json!({"kind":"targets", "ids":ids.split(',').collect::<Vec<_>>()}));
    }
    bail!("[invalid-adjustment] scope must be below, group:ID, or ids:ID,ID")
}

/// All mutations plan into a candidate so library callers also receive atomic failures.
pub fn edit(
    raw: &mut Value,
    page: Option<&str>,
    operation: &str,
    id: &str,
    settings: &Value,
) -> Result<Value> {
    let allowed: &[&str] = match operation {
        "add" => &[
            "layer",
            "adjustment",
            "params",
            "scope",
            "enabled",
            "opacity",
            "blend_mode",
            "blend_space",
        ],
        "set" => &[
            "adjustment",
            "params",
            "scope",
            "enabled",
            "opacity",
            "blend_mode",
            "blend_space",
        ],
        _ => &[],
    };
    self::settings(settings, allowed)?;
    if operation == "set" && settings.as_object().unwrap().is_empty() {
        bail!("adjustment set requires settings")
    }
    if !is_document(raw) {
        bail!("[unsupported-capability] adjustment edits require `migrate --target 6`")
    }
    crate::scene::validate(raw)?;
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    let result = match operation {
        "add" => {
            if let Some(existing) = crate::scene::find_node_mut(selected, id) {
                let kind = existing
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("node");
                bail!("[invalid-adjustment] node ID {id} already exists as a {kind} node")
            }
            let layer_id = settings["layer"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    selected
                        .get("layers")
                        .and_then(Value::as_array)
                        .and_then(|layers| layers.first())
                        .and_then(|layer| layer.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .context("[invalid-adjustment] page has no layers")?;
            crate::scene::ensure_layer_unlocked(selected, &layer_id)?;
            let node = json!({"kind":"adjustment", "id":id,
                "adjustment":settings["adjustment"], "params":settings.get("params").cloned().unwrap_or(json!({})),
                "scope":settings.get("scope").cloned().unwrap_or(json!({"kind":"below"})),
                "enabled":settings.get("enabled").cloned().unwrap_or(json!(true)),
                "opacity":settings.get("opacity").cloned().unwrap_or(json!(1.0)), "blend_mode":settings.get("blend_mode").cloned().unwrap_or(json!("normal")),"blend_space":settings.get("blend_space").cloned().unwrap_or(json!("srgb"))});
            crate::scene::layer_nodes_mut(selected, &layer_id)?.push(node.clone());
            node
        }
        "set" | "enable" | "disable" => {
            ensure_unlocked(selected, id)?;
            let node = crate::scene::find_node_mut(selected, id)
                .with_context(|| format!("adjustment not found: {id}"))?;
            if node["kind"] != "adjustment" {
                bail!("node {id} is not an adjustment")
            }
            if operation == "set" {
                for key in [
                    "adjustment",
                    "params",
                    "scope",
                    "enabled",
                    "opacity",
                    "blend_mode",
                    "blend_space",
                ] {
                    if let Some(value) = settings.get(key) {
                        node[key] = value.clone();
                    }
                }
            } else {
                node["enabled"] = json!(operation == "enable");
            }
            node.clone()
        }
        "remove" => {
            ensure_unlocked(selected, id)?;
            fn remove(nodes: &mut Vec<Value>, id: &str) -> Option<Value> {
                if let Some(index) = nodes.iter().position(|n| n["id"] == id) {
                    return Some(nodes.remove(index));
                }
                for node in nodes {
                    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                        if let Some(removed) = remove(children, id) {
                            return Some(removed);
                        }
                    }
                }
                None
            }
            let layers = selected["layers"].as_array_mut().unwrap();
            let removed = layers
                .iter_mut()
                .find_map(|l| remove(l["nodes"].as_array_mut().unwrap(), id))
                .with_context(|| format!("adjustment not found: {id}"))?;
            if removed["kind"] != "adjustment" {
                bail!("node {id} is not an adjustment")
            }
            removed
        }
        _ => bail!("unknown adjustment operation {operation}"),
    };
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(result)
}

pub(crate) fn ensure_unlocked(page: &Value, id: &str) -> Result<()> {
    fn contains(node: &Value, id: &str) -> bool {
        node["id"] == id
            || node
                .get("children")
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|n| contains(n, id)))
    }
    for layer in page["layers"].as_array().context("page layers missing")? {
        if layer["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| contains(n, id))
        {
            if layer["locked"].as_bool().unwrap_or(false) {
                bail!(
                    "[locked-layer] layer {} containing {id} is locked",
                    layer["id"]
                )
            }
            return Ok(());
        }
    }
    bail!("node not found: {id}")
}

pub fn list(raw: &Value, page: Option<&str>) -> Result<Value> {
    crate::scene::validate(raw)?;
    let pages = raw["pages"].as_array().unwrap();
    let page = match page {
        Some(id) => pages
            .iter()
            .find(|p| p["id"] == id)
            .with_context(|| format!("page not found: {id}"))?,
        None => &pages[0],
    };
    fn visit(nodes: &[Value], out: &mut Vec<Value>) {
        for node in nodes {
            if node["kind"] == "adjustment" {
                out.push(node.clone());
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                visit(children, out);
            }
        }
    }
    let mut out = Vec::new();
    for layer in page["layers"].as_array().unwrap() {
        visit(layer["nodes"].as_array().unwrap(), &mut out);
    }
    Ok(json!({"page":page["id"],"adjustments":out,"engine":ENGINE}))
}

pub fn clip_edit(
    raw: &mut Value,
    page: Option<&str>,
    operation: &str,
    id: &str,
    base: Option<&str>,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    if !is_document(raw) {
        bail!("clipping requires `migrate --target 6`")
    }
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    ensure_unlocked(selected, id)?;
    let node = crate::scene::find_node_mut(selected, id).context("clipped node missing")?;
    match operation {
        "add" => {
            node["clipping"] = json!({"base":base.context("clip add requires --base")?});
        }
        "remove" => {
            node.as_object_mut()
                .unwrap()
                .remove("clipping")
                .context("node is not clipped")?;
        }
        _ => bail!("clip operation must be add or remove"),
    }
    let result = node.clone();
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(result)
}

/// Straight-alpha Porter-Duff source-over. Transparent RGB never contributes.
/// Bounding box `(x, y, width, height)` of every nonzero byte in the coverage
/// masks, or `None` when nothing is covered.
fn coverage_bounds<'a>(
    masks: impl Iterator<Item = &'a Vec<u8>>,
    width: u32,
    height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    for mask in masks {
        for (y, row) in mask.chunks_exact(width as usize).enumerate() {
            let Some(first) = row.iter().position(|a| *a != 0) else {
                continue;
            };
            let last = row.iter().rposition(|a| *a != 0).unwrap();
            let y = y as u32;
            x0 = x0.min(first as u32);
            x1 = x1.max(last as u32 + 1);
            y0 = y0.min(y);
            y1 = y1.max(y + 1);
        }
    }
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
}

#[derive(Clone)]
enum SceneSurface {
    Byte(RgbaImage),
    Working(crate::blend::LinearSurface),
}
#[derive(Clone)]
enum Coverage {
    Byte(Vec<u8>),
    Working(Vec<f64>),
}
impl Coverage {
    fn bytes(&self) -> Option<&Vec<u8>> {
        if let Self::Byte(values) = self {
            Some(values)
        } else {
            None
        }
    }
    fn occlude(&mut self, surface: &SceneSurface) {
        match (self, surface) {
            (Self::Byte(mask), SceneSurface::Byte(image)) => {
                for (a, p) in mask.iter_mut().zip(image.pixels()) {
                    *a = ((u32::from(*a) * u32::from(255 - p[3]) + 127) / 255) as u8;
                }
            }
            (Self::Working(mask), SceneSurface::Working(image)) => {
                for (a, p) in mask.iter_mut().zip(image.pixels()) {
                    *a *= 1.0 - p[3];
                }
            }
            _ => unreachable!("scene precision is fixed"),
        }
    }
}
impl SceneSurface {
    fn byte_mut(&mut self, operation: &str) -> Result<&mut RgbaImage> {
        match self {
            Self::Byte(image) => Ok(image),
            Self::Working(_) => {
                bail!("[unsupported-capability] float scene {operation} is not implemented")
            }
        }
    }
    fn into_byte(self) -> Result<RgbaImage> {
        match self {
            Self::Byte(image) => Ok(image),
            Self::Working(_) => bail!("[invalid-composite] expected a byte preview surface"),
        }
    }
    fn alpha(&self) -> Coverage {
        match self {
            Self::Byte(image) => Coverage::Byte(image.pixels().map(|p| p[3]).collect()),
            Self::Working(image) => {
                Coverage::Working(image.pixels().iter().map(|p| p[3]).collect())
            }
        }
    }
    fn opacity(&mut self, opacity: f64) -> Result<()> {
        match self {
            // Full opacity leaves every byte alpha unchanged.
            Self::Byte(_) if opacity == 1.0 => Ok(()),
            Self::Byte(image) => {
                // Zero alpha stays zero, so only covered pixels need the multiply.
                for p in image.pixels_mut().filter(|p| p[3] != 0) {
                    p[3] = round(f64::from(p[3]) * opacity);
                }
                Ok(())
            }
            Self::Working(image) => image.apply_opacity(opacity),
        }
    }
    fn clip(&mut self, coverage: Option<&Coverage>) -> Result<()> {
        match (self, coverage) {
            (Self::Byte(image), Some(Coverage::Byte(mask))) => {
                for (p, a) in image.pixels_mut().zip(mask) {
                    p[3] = ((u32::from(p[3]) * u32::from(*a) + 127) / 255) as u8;
                    if p[3] == 0 {
                        *p = Rgba([0, 0, 0, 0]);
                    }
                }
                Ok(())
            }
            (Self::Working(image), Some(Coverage::Working(mask))) => {
                image.apply_coverage(mask, 1.0)
            }
            (Self::Byte(image), None) => {
                for p in image.pixels_mut() {
                    *p = Rgba([0, 0, 0, 0]);
                }
                Ok(())
            }
            (surface, None) => surface.opacity(0.0),
            _ => bail!("[invalid-composite] clipping precision differs from scene precision"),
        }
    }
    fn over(
        &mut self,
        source: &Self,
        mode: &str,
        space: &str,
        context: Option<&crate::blend::LegacyBlendContext>,
    ) -> Result<()> {
        match (self, source) {
            (Self::Byte(dst), Self::Byte(src)) => crate::blend::over(dst, src, mode, space),
            (Self::Working(dst), Self::Working(src)) => dst.over_declared(
                src,
                mode,
                space,
                context.context("working blend context missing")?,
            ),
            _ => bail!("[invalid-composite] scene surface precisions differ"),
        }
    }
}

struct Renderer<'a> {
    raw: &'a Value,
    page: &'a Value,
    document: &'a Path,
    scale: f32,
    width: u32,
    height: u32,
    live_bytes: u64,
    pixel_work: u64,
    peak_live_bytes: u64,
    working: Option<&'a crate::color::roles::Setting>,
    blend_context: Option<&'a crate::blend::LegacyBlendContext>,
    conversion_clipped: bool,
    gradient_profiles: &'a crate::color::value::ProfileSet,
    gradient_engines: Vec<(
        crate::color::icc::Intent,
        crate::color::value::Evaluator<'a>,
    )>,
    /// Highest pixel work seen at a budget check (it can drop after one).
    work_peak: u64,
    node_context: Option<[u8; 32]>,
    /// Where node surfaces persist between processes, when they do.
    node_disk: Option<PathBuf>,
}

impl Renderer<'_> {
    fn pixel_bytes(&self) -> u64 {
        if self.working.is_some() {
            32
        } else {
            4
        }
    }
    fn coverage_bytes(&self) -> u64 {
        if self.working.is_some() {
            8
        } else {
            1
        }
    }
    fn reserve(&mut self) -> Result<()> {
        self.reserve_pixels(u64::from(self.width) * u64::from(self.height))
    }
    fn plain_node_pixels(&self, node: &Value) -> u64 {
        plain_node_pixels(
            node,
            u64::from(self.width) * u64::from(self.height),
            self.scale,
        )
    }
    /// Reserve a full surface of live memory but only `work` pixels of the
    /// cumulative pixel-work budget.
    fn reserve_surface(&mut self, work: u64) -> Result<()> {
        let canvas = u64::from(self.width) * u64::from(self.height);
        self.reserve_pixels(canvas)?;
        self.pixel_work -= canvas - work.min(canvas);
        Ok(())
    }
    fn reserve_pixels(&mut self, pixels: u64) -> Result<()> {
        check_cancelled()?;
        self.pixel_work = self
            .pixel_work
            .checked_add(pixels)
            .context("pixel work overflow")?;
        self.work_peak = self.work_peak.max(self.pixel_work);
        self.live_bytes = self
            .live_bytes
            .checked_add(pixels * self.pixel_bytes())
            .context("surface allocation overflow")?;
        self.peak_live_bytes = self.peak_live_bytes.max(self.live_bytes);
        if self.live_bytes > MAX_TEMP_BYTES || self.pixel_work > MAX_PIXEL_WORK {
            bail!("[limit-exceeded] compositing exceeds temporary surface or pixel-work budget; reduce canvas size, scale, or stack depth")
        }
        Ok(())
    }
    fn reserve_byte_scratch(&mut self) -> Result<()> {
        let bytes = u64::from(self.width) * u64::from(self.height) * 4;
        self.live_bytes = self
            .live_bytes
            .checked_add(bytes)
            .context("surface allocation overflow")?;
        self.peak_live_bytes = self.peak_live_bytes.max(self.live_bytes);
        if self.live_bytes > MAX_TEMP_BYTES {
            bail!("[limit-exceeded] compositing exceeds temporary surface or pixel-work budget; reduce canvas size, scale, or stack depth")
        }
        Ok(())
    }
    fn release_byte_scratch(&mut self) {
        self.live_bytes -= u64::from(self.width) * u64::from(self.height) * 4;
    }
    fn srgb8_surface(&mut self, bytes: RgbaImage) -> Result<SceneSurface> {
        let working = self.working.context("working scene setting missing")?;
        let destination = working.destination.clone();
        let intent = working.intent;
        let width = bytes.width();
        let mut pixels = Vec::with_capacity(bytes.len() / 4);
        let engine = crate::color::gradient::engine(
            self.gradient_profiles,
            &mut self.gradient_engines,
            intent,
        )?;
        for (index, pixel) in bytes.pixels().enumerate() {
            if index % width as usize == 0 {
                check_cancelled()?;
            }
            let rgba = std::array::from_fn(|channel| f64::from(pixel[channel]) / 255.0);
            pixels.push(engine.srgb_working_rgba(rgba, &destination)?);
        }
        let surface = crate::blend::LinearSurface::from_pixels(
            bytes.width(),
            bytes.height(),
            destination.space(),
            pixels,
        )?;
        self.release_byte_scratch();
        Ok(SceneSurface::Working(surface))
    }
    fn vector_plane(
        &mut self,
        node: &Value,
        parent: kurbo::Affine,
        slot: &str,
    ) -> Result<Option<crate::blend::LinearSurface>> {
        let working = self.working.context("working scene setting missing")?;
        let color = crate::color::gradient::color(&node["style"][slot], &self.raw["styles"], 0)?;
        let destination = working.destination.clone();
        let intent = working.intent;
        let (rgba, clipped) = {
            let engine = crate::color::gradient::engine(
                self.gradient_profiles,
                &mut self.gradient_engines,
                intent,
            )?;
            engine.working_rgba(&color, &destination)?
        };
        self.conversion_clipped |= clipped;
        if rgba[3] == 0.0 {
            return Ok(None);
        }
        self.reserve()?;
        self.reserve_byte_scratch()?;
        let mut content = node.clone();
        content["style"]["fill"] =
            json!({"fallback":if slot == "fill" {"#ffffffff"} else {"none"}});
        content["style"]["stroke"] =
            json!({"fallback":if slot == "stroke" {"#ffffffff"} else {"none"}});
        if content
            .get("mask")
            .is_some_and(|mask| mask.get("resource").is_some())
        {
            content.as_object_mut().unwrap().remove("mask");
        }
        let fragment =
            crate::image::fragment_svg(self.raw, self.page, self.document, &content, parent)?;
        let coverage = crate::render::scene_to_rgba_proxy(&fragment, self.scale)?;
        self.release_byte_scratch();
        let mut surface =
            crate::blend::LinearSurface::new(self.width, self.height, destination.space())?;
        for (index, pixel) in coverage.pixels().enumerate() {
            if index % self.width as usize == 0 {
                check_cancelled()?;
            }
            surface.write_pixel(
                index,
                [
                    rgba[0],
                    rgba[1],
                    rgba[2],
                    rgba[3] * f64::from(pixel[3]) / 255.0,
                ],
            )?;
        }
        Ok(Some(surface))
    }
    fn vector_surface(&mut self, node: &Value, parent: kurbo::Affine) -> Result<SceneSurface> {
        let mut result = self.empty()?;
        for slot in ["fill", "stroke"] {
            if slot == "stroke" && node["kind"] == "text" {
                continue;
            }
            if let Some(plane) = self.vector_plane(node, parent, slot)? {
                result.over(
                    &SceneSurface::Working(plane),
                    "normal",
                    "srgb",
                    self.blend_context,
                )?;
                self.release();
            }
        }
        Ok(result)
    }
    fn working_effects(
        &mut self,
        node: &Value,
        source: &crate::blend::LinearSurface,
    ) -> Result<crate::blend::LinearSurface> {
        let stack = crate::effects::stack(self.raw, node)?;
        effect::validate_supported(stack)?;
        let active: Vec<&Value> = stack.iter().filter(|op| op["enabled"] == true).collect();
        let spatial = active.iter().any(|op| !effect::needs_surface(op));
        self.reserve()?; // atomic stack result
        self.reserve()?; // one prepared overlay paint or spatial effect surface
        if spatial {
            // Blur keeps three float planes alive (input, pass and next) and
            // strokes two f64 distance fields; reserve the larger of the two.
            for _ in 0..3 {
                self.reserve()?;
            }
        }
        let per_pixel: u64 = active
            .iter()
            .map(|op| if effect::needs_surface(op) { 5 } else { 32 })
            .sum();
        self.pixel_work = self
            .pixel_work
            .checked_add(per_pixel * u64::from(self.width) * u64::from(self.height))
            .context("effect pixel work overflow")?;
        if self.pixel_work > MAX_PIXEL_WORK {
            bail!("[limit-exceeded] float effect pixel-work budget exceeded")
        }
        let context = self
            .blend_context
            .context("working blend context missing")?;
        let result = effect::apply(source, node, stack, context, self.scale, |op| {
            if !effect::needs_surface(op) {
                let working = self.working.context("working scene setting missing")?;
                let color = crate::color::gradient::color(
                    op["params"].get("color").unwrap_or(&json!("#000000")),
                    &self.raw["styles"],
                    0,
                )?;
                let engine = crate::color::gradient::engine(
                    self.gradient_profiles,
                    &mut self.gradient_engines,
                    working.intent,
                )?;
                let (rgba, clipped) = engine.working_rgba(&color, &working.destination)?;
                self.conversion_clipped |= clipped;
                return Ok(effect::Paint::Color(rgba));
            }
            let fill = if op["kind"] == "color-overlay" {
                json!({"kind":"solid","color":op["params"].get("color").cloned().unwrap_or(json!("#000000"))})
            } else {
                op["params"]["fill"].clone()
            };
            let ox = node["effect_origin"][0].as_f64().unwrap_or(0.0);
            let oy = node["effect_origin"][1].as_f64().unwrap_or(0.0);
            let fill_node = json!({"kind":"fill","id":"working-effect-paint","x":-ox,"y":-oy,
                "width":f64::from(self.width)/f64::from(self.scale)+ox,
                "height":f64::from(self.height)/f64::from(self.scale)+oy,"fill":fill});
            let definition = crate::fill::resolved(self.raw, &fill_node["fill"])?;
            let working = self.working.context("working scene setting missing")?;
            let intent = crate::color::gradient::setting(self.raw, definition)?
                .map_or(working.intent, |(_, intent)| intent);
            let engine = crate::color::gradient::engine(
                self.gradient_profiles,
                &mut self.gradient_engines,
                intent,
            )?;
            let paint = crate::fill::render_linear_prepared(
                self.raw,
                &fill_node,
                self.document,
                kurbo::Affine::IDENTITY,
                &crate::fill::LinearFillOptions {
                    dimensions: (self.width, self.height),
                    scale: self.scale,
                    working: working.destination.clone(),
                },
                engine,
            )?;
            self.conversion_clipped |= paint.conversion_clipped;
            Ok(effect::Paint::Surface(paint.surface))
        })?;
        if spatial {
            for _ in 0..3 {
                self.release();
            }
        }
        self.release();
        self.release();
        Ok(result)
    }
    fn release(&mut self) {
        self.live_bytes -= u64::from(self.width) * u64::from(self.height) * self.pixel_bytes();
    }
    fn empty(&mut self) -> Result<SceneSurface> {
        self.reserve()?;
        if let Some(working) = self.working {
            Ok(SceneSurface::Working(crate::blend::LinearSurface::new(
                self.width,
                self.height,
                working.destination.space(),
            )?))
        } else {
            Ok(SceneSurface::Byte(RgbaImage::new(self.width, self.height)))
        }
    }
    fn siblings(&mut self, nodes: &[Value], parent: kurbo::Affine) -> Result<SceneSurface> {
        self.siblings_with_backdrop(nodes, parent, None)
    }

    fn siblings_with_backdrop(
        &mut self,
        nodes: &[Value],
        parent: kurbo::Affine,
        backdrop: Option<SceneSurface>,
    ) -> Result<SceneSurface> {
        // Scope references select the visible alpha contribution of earlier siblings.
        // Keep only queried coverage, not a full RGBA surface for every scene node.
        let wanted: HashSet<&str> = nodes
            .iter()
            .filter(|n| n["kind"] == "adjustment")
            .flat_map(scope_ids)
            .collect();
        let mut coverage: HashMap<&str, Coverage> = HashMap::new();
        let mut result = match backdrop {
            Some(surface) => surface,
            None => self.empty()?,
        };
        self.live_bytes += u64::from(self.width) * u64::from(self.height) * self.coverage_bytes();
        self.peak_live_bytes = self.peak_live_bytes.max(self.live_bytes);
        if self.live_bytes > MAX_TEMP_BYTES {
            bail!("[limit-exceeded] clipping coverage budget exceeded")
        }
        let mut clip_alpha: Option<Coverage> = None;
        let any_clipping = nodes.iter().any(|node| node.get("clipping").is_some());
        for node in nodes {
            if node.get("visible").and_then(Value::as_bool) == Some(false) {
                // Adjustments never set the clipping base, so hiding one keeps the group.
                if node.get("clipping").is_none() && node["kind"] != "adjustment" {
                    clip_alpha = None;
                }
                continue;
            }
            if node["kind"] == "adjustment" {
                if !node["enabled"].as_bool().unwrap_or(false) {
                    continue;
                }
                if let SceneSurface::Working(surface) = &mut result {
                    self.reserve()?; // atomic adjustment output
                    let mask = self.mask(node, parent)?;
                    let ids = scope_ids(node);
                    let scopes: Vec<&[f64]> = ids
                        .iter()
                        .filter_map(|id| match coverage.get(id) {
                            Some(Coverage::Working(values)) => Some(values.as_slice()),
                            _ => None,
                        })
                        .collect();
                    self.pixel_work = self
                        .pixel_work
                        .checked_add(
                            scopes.len() as u64 * u64::from(self.width) * u64::from(self.height),
                        )
                        .context("adjustment scope work overflow")?;
                    if self.pixel_work > MAX_PIXEL_WORK {
                        bail!("[limit-exceeded] scoped adjustment pixel-work budget exceeded")
                    }
                    adjustment::apply(
                        surface,
                        node,
                        &scopes,
                        mask.as_deref(),
                        self.blend_context
                            .context("working blend context missing")?,
                    )
                    .with_context(|| {
                        format!("adjustment {} ({})", node["id"], node["adjustment"])
                    })?;
                    if mask.is_some() {
                        self.release();
                    }
                    self.release();
                    continue;
                }
                let result = result.byte_mut("adjustments")?;
                let kind = node["adjustment"].as_str().unwrap();
                let opacity = node.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
                let mode = node["blend_mode"].as_str().unwrap();
                self.reserve()?; // adjustment engines may use one extra scratch surface
                self.reserve()?; // scoped results plus the operation engine's working copy
                if node["scope"]["kind"] == "below"
                    && node.get("mask").is_none()
                    && mode == "normal"
                {
                    adjust(result, kind, &node["params"], opacity)?;
                } else {
                    let ids = scope_ids(node);
                    let mask = self.mask(node, parent)?;
                    let (canvas_w, canvas_h) = (result.width(), result.height());
                    // Adjustments are per-pixel, so only the scoped nodes' bounding
                    // box needs grading: cost follows the target, not the canvas.
                    let below = node["scope"]["kind"] == "below";
                    let region = if below {
                        Some((0, 0, canvas_w, canvas_h))
                    } else {
                        coverage_bounds(
                            ids.iter()
                                .filter_map(|id| coverage.get(id).and_then(Coverage::bytes)),
                            canvas_w,
                            canvas_h,
                        )
                    };
                    if let Some((x0, y0, rw, rh)) = region {
                        let mut adjusted =
                            image::imageops::crop_imm(result, x0, y0, rw, rh).to_image();
                        adjust(&mut adjusted, kind, &node["params"], 1.0)?;
                        let space = node["blend_space"].as_str().unwrap_or("srgb");
                        for ry in 0..rh {
                            for rx in 0..rw {
                                let (x, y) = (x0 + rx, y0 + ry);
                                let index = y as usize * canvas_w as usize + x as usize;
                                let alpha: u32 = if below {
                                    255
                                } else {
                                    ids.iter()
                                        .filter_map(|id| coverage.get(id).and_then(Coverage::bytes))
                                        .map(|mask| u32::from(mask[index]))
                                        .sum()
                                };
                                let amount = opacity * f64::from(alpha.min(255)) / 255.0
                                    * mask.as_ref().map_or(1.0, |m| f64::from(m[index]) / 255.0);
                                if amount == 0.0 {
                                    continue;
                                }
                                let graded = adjusted.get_pixel(rx, ry);
                                let pixel = result.get_pixel_mut(x, y);
                                let mut b = [0.0; 3];
                                let mut s = [0.0; 3];
                                for i in 0..3 {
                                    b[i] = f64::from(pixel[i]) / 255.0;
                                    s[i] = f64::from(graded[i]) / 255.0;
                                    if space == "linear" {
                                        b[i] = crate::blend::to_linear(b[i]);
                                        s[i] = crate::blend::to_linear(s[i]);
                                    }
                                }
                                let mixed = crate::blend::rgb(mode, b, s);
                                for i in 0..3 {
                                    let value = 255.0
                                        * if space == "linear" {
                                            crate::blend::to_srgb(mixed[i].clamp(0.0, 1.0))
                                        } else {
                                            mixed[i]
                                        };
                                    pixel[i] = round(
                                        f64::from(pixel[i]) * (1.0 - amount)
                                            + f64::from(round(value)) * amount,
                                    );
                                }
                            }
                        }
                    }
                    if mask.is_some() {
                        self.release();
                    }
                }
                self.release();
                self.release();
                continue;
            }
            let (mut surface, passed) = self.cached_node_surface(node, parent, &result)?;
            if node.get("clipping").is_some() {
                surface.clip(clip_alpha.as_ref())?;
            } else if any_clipping {
                clip_alpha = Some(surface.alpha());
            }
            for mask in coverage.values_mut() {
                mask.occlude(&surface);
            }
            self.pixel_work +=
                coverage.len() as u64 * u64::from(self.width) * u64::from(self.height);
            if self.pixel_work > MAX_PIXEL_WORK {
                bail!("[limit-exceeded] scoped compositing pixel-work budget exceeded")
            }
            let id = node["id"].as_str().unwrap();
            if wanted.contains(id) {
                let size = u64::from(self.width) * u64::from(self.height) * self.coverage_bytes();
                self.live_bytes += size;
                self.peak_live_bytes = self.peak_live_bytes.max(self.live_bytes);
                if self.live_bytes > MAX_TEMP_BYTES {
                    bail!("[limit-exceeded] adjustment scope coverage exceeds temporary surface budget")
                }
                coverage.insert(id, surface.alpha());
            }
            if let Some(passed) = passed {
                result = passed;
                self.release();
            } else {
                result.over(
                    &surface,
                    node["blend_mode"].as_str().unwrap_or("normal"),
                    node["blend_space"].as_str().unwrap_or("srgb"),
                    self.blend_context,
                )?;
            }
            self.release();
        }
        self.live_bytes -= coverage.len() as u64
            * u64::from(self.width)
            * u64::from(self.height)
            * self.coverage_bytes();
        self.live_bytes -= u64::from(self.width) * u64::from(self.height) * self.coverage_bytes();
        Ok(result)
    }

    /// The cache key of a node whose surface reads only the node, its parent
    /// transform and the render context: byte-precision leaves. Images (masks can
    /// name other page nodes), text (wrap and threads read the page), 3D, groups
    /// and instances are always rendered.
    fn node_key(&self, node: &Value, parent: kurbo::Affine) -> Option<[u8; 32]> {
        use sha2::{Digest, Sha256};
        let context = self.node_context.filter(|_| self.working.is_none())?;
        if !matches!(
            node["kind"].as_str(),
            Some("fill" | "path" | "rect" | "ellipse" | "line")
        ) {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(context);
        for coefficient in parent.as_coeffs() {
            hasher.update(coefficient.to_bits().to_le_bytes());
        }
        serde_json::to_writer(&mut hasher, node).ok()?;
        Some(hasher.finalize().into())
    }

    /// [`Self::node_surface`] through the node cache. A hit replays the node's
    /// pixel-work and memory accounting; when that would cross a limit the node
    /// renders normally so the same error is reported.
    fn cached_node_surface(
        &mut self,
        node: &Value,
        parent: kurbo::Affine,
        result: &SceneSurface,
    ) -> Result<(SceneSurface, Option<SceneSurface>)> {
        let Some(key) = self.node_key(node, parent) else {
            return self.node_surface(node, parent, result);
        };
        let (work0, live0) = (self.pixel_work, self.live_bytes);
        let (width, height) = (self.width, self.height);
        let fits = |entry: &CachedSurface| {
            work0 + entry.work_peak <= MAX_PIXEL_WORK && live0 + entry.live_peak <= MAX_TEMP_BYTES
        };
        let expand = |entry: &CachedSurface| {
            let mut image = RgbaImage::new(width, height);
            image::imageops::replace(
                &mut image,
                &entry.pixels,
                i64::from(entry.x),
                i64::from(entry.y),
            );
            (
                image,
                entry.work,
                entry.work_peak,
                entry.live,
                entry.live_peak,
            )
        };
        let hit = NODE_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let cache = cache.as_mut()?;
            cache.clock += 1;
            let clock = cache.clock;
            let entry = cache.entries.get_mut(&key)?;
            if !fits(entry) {
                return None;
            }
            entry.used = clock;
            Some(expand(entry))
        });
        let hit = hit.or_else(|| {
            let bytes = crate::render_cache::read(self.node_disk.as_deref()?, &key)?;
            let entry = CachedSurface::decode(bytes, width, height).filter(|e| fits(e))?;
            let hit = expand(&entry);
            store(key, entry);
            Some(hit)
        });
        if let Some((image, work, work_peak, live, live_peak)) = hit {
            self.pixel_work += work;
            self.work_peak = self.work_peak.max(work0 + work_peak);
            self.live_bytes += live;
            self.peak_live_bytes = self.peak_live_bytes.max(live0 + live_peak);
            return Ok((SceneSurface::Byte(image), None));
        }
        let (saved_live_peak, saved_work_peak) = (self.peak_live_bytes, self.work_peak);
        self.peak_live_bytes = live0;
        self.work_peak = work0;
        let outcome = self.node_surface(node, parent, result);
        let live_peak = self.peak_live_bytes;
        let work_peak = self.work_peak.max(self.pixel_work);
        self.peak_live_bytes = saved_live_peak.max(live_peak);
        self.work_peak = saved_work_peak.max(work_peak);
        let (surface, passed) = outcome?;
        if let (SceneSurface::Byte(image), None) = (&surface, &passed) {
            if self.pixel_work >= work0 && self.live_bytes >= live0 {
                let entry = CachedSurface {
                    x: 0,
                    y: 0,
                    pixels: RgbaImage::new(0, 0),
                    work: self.pixel_work - work0,
                    work_peak: work_peak - work0,
                    live: self.live_bytes - live0,
                    live_peak: live_peak - live0,
                    used: 0,
                };
                remember(key, image, entry, self.node_disk.as_deref());
            }
        }
        Ok((surface, passed))
    }

    /// One node's finished surface (content, transforms, mask, effects and
    /// opacity) before sibling clipping, plus the pass-through result of a group.
    fn node_surface(
        &mut self,
        node: &Value,
        parent: kurbo::Affine,
        result: &SceneSurface,
    ) -> Result<(SceneSurface, Option<SceneSurface>)> {
        let affine = transform(node)?;
        let direct_affine = if node["kind"] == "group" || node["kind"] == "instance" {
            None
        } else {
            crate::transform::affine(node)?
        };
        let render_parent = direct_affine.unwrap_or(kurbo::Affine::IDENTITY) * parent;
        let mut passed = None;
        let mut surface = if node["kind"] == "group" {
            let surface = self.siblings(node["children"].as_array().unwrap(), parent * affine)?;
            if node["isolation"] == "pass-through" {
                self.reserve()?;
                passed = Some(self.siblings_with_backdrop(
                    node["children"].as_array().unwrap(),
                    parent * affine,
                    Some(result.clone()),
                )?);
            }
            surface
        } else if node["kind"] == "instance" {
            self.siblings(std::slice::from_ref(&node["fallback"]), parent * affine)?
        } else if node["kind"] == "fill" {
            self.reserve()?;
            self.pixel_work += u64::from(self.width) * u64::from(self.height) * 4;
            if self.pixel_work > MAX_PIXEL_WORK {
                bail!("[limit-exceeded] fill pixel-work budget exceeded")
            }
            let fill = crate::fill::resolved(self.raw, &node["fill"])?;
            let intent = crate::color::gradient::setting(self.raw, fill)?
                .map(|(_, intent)| intent)
                .or(self.working.map(|setting| setting.intent));
            let engine = if let Some(intent) = intent {
                Some(crate::color::gradient::engine(
                    self.gradient_profiles,
                    &mut self.gradient_engines,
                    intent,
                )?)
            } else {
                None
            };
            if let Some(working) = self.working {
                let result = crate::fill::render_linear_prepared(
                    self.raw,
                    node,
                    self.document,
                    render_parent,
                    &crate::fill::LinearFillOptions {
                        dimensions: (self.width, self.height),
                        scale: self.scale,
                        working: working.destination.clone(),
                    },
                    engine.unwrap(),
                )?;
                self.conversion_clipped |= result.conversion_clipped;
                SceneSurface::Working(result.surface)
            } else {
                SceneSurface::Byte(crate::fill::render(
                    self.raw,
                    node,
                    self.document,
                    render_parent,
                    (self.width, self.height),
                    self.scale,
                    engine,
                )?)
            }
        } else {
            let raster_source = matches!(
                node["kind"].as_str(),
                Some("image" | "raster" | "photo" | "scene3d")
            );
            let vector_source = matches!(
                node["kind"].as_str(),
                Some("path" | "rect" | "ellipse" | "line" | "text")
            );
            if self.working.is_some() && !raster_source && !vector_source {
                bail!("[unsupported-capability] float scene rendering for node {} ({}) is not implemented",node["id"],node["kind"])
            }
            if self.working.is_some() && vector_source {
                self.vector_surface(node, render_parent)?
            } else {
                // Reuse the image/vector/text serializer and font pipeline; never copy source bytes.
                let work = self.plain_node_pixels(node);
                self.reserve_surface(work)?;
                let mut content = node.clone();
                if matches!(
                    content["kind"].as_str(),
                    Some("image" | "raster" | "photo" | "scene3d")
                ) {
                    content["opacity"] = json!(1.0);
                    content["blend_mode"] = json!("normal");
                }
                if content
                    .get("mask")
                    .is_some_and(|m| m.get("resource").is_some())
                {
                    content.as_object_mut().unwrap().remove("mask");
                }
                if self.working.is_some() {
                    self.reserve_byte_scratch()?;
                }
                let fragment = crate::image::fragment_svg(
                    self.raw,
                    self.page,
                    self.document,
                    &content,
                    render_parent,
                )?;
                let bytes = crate::render::scene_to_rgba_proxy(&fragment, self.scale)?;
                if self.working.is_some() {
                    self.srgb8_surface(bytes)?
                } else {
                    SceneSurface::Byte(bytes)
                }
            }
        };
        if let Some(stack) = node
            .get("transforms")
            .and_then(Value::as_array)
            .filter(|s| !s.is_empty())
            .filter(|_| direct_affine.is_none())
        {
            self.reserve()?; // warped result
                             // Float resampling also holds an encoded sRGB copy of the source.
            let encoded = matches!(surface, SceneSurface::Working(_));
            if encoded {
                self.reserve()?;
            }
            let mut masks = Vec::with_capacity(stack.len());
            for op in stack {
                masks.push(self.mask(op, kurbo::Affine::IDENTITY)?);
            }
            self.pixel_work += u64::from(self.width) * u64::from(self.height) * stack.len() as u64;
            if self.pixel_work > MAX_PIXEL_WORK {
                bail!("[limit-exceeded] transform pixel-work budget exceeded")
            }
            match &mut surface {
                SceneSurface::Byte(pixels) => {
                    *pixels = crate::transform::warp(pixels, stack, &masks, self.scale)?;
                }
                SceneSurface::Working(pixels) => {
                    let context = self
                        .blend_context
                        .context("working blend context missing")?;
                    *pixels =
                        crate::transform::warp_working(pixels, stack, &masks, self.scale, context)
                            .with_context(|| {
                                format!("float scene transform resampling for node {}", node["id"])
                            })?;
                }
            }
            for _ in masks.iter().flatten() {
                self.release();
            }
            if encoded {
                self.release();
            }
            self.release();
        }
        // Layer-style order: the mask defines the silhouette first, so effects
        // (glows, shadows, strokes) follow the masked shape instead of being
        // clipped away by a second mask pass.
        if let Some(mask) = self.mask(node, parent)? {
            let coverage = match &surface {
                SceneSurface::Byte(_) => Coverage::Byte(mask),
                SceneSurface::Working(_) => Coverage::Working(
                    mask.into_iter()
                        .map(|alpha| f64::from(alpha) / 255.0)
                        .collect(),
                ),
            };
            surface.clip(Some(&coverage))?;
            self.release();
        }
        let opacity = node.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
        if node.get("effects").is_some() || node.get("content_opacity").is_some() {
            if self.working.is_some() {
                if crate::effects::stack(self.raw, node)?
                    .iter()
                    .any(|op| op["enabled"] == true)
                {
                    let SceneSurface::Working(source) = &surface else {
                        unreachable!("working effect target")
                    };
                    surface =
                        SceneSurface::Working(self.working_effects(node, source).with_context(
                            || format!("float scene effects for node {}", node["id"]),
                        )?);
                } else {
                    surface.opacity(node["content_opacity"].as_f64().unwrap_or(1.0))?;
                }
            } else {
                let surface = surface.byte_mut("effects")?;
                // Effects run on the content's bounds plus the effect reach, never the
                // whole canvas; the budget below follows that region.
                let (cx, cy, cw, ch) = crate::effects::region(self.raw, node, surface, self.scale)?;
                if cw > 0 && ch > 0 {
                    let pixels = u64::from(cw) * u64::from(ch);
                    // Conservative bound includes the image engine's f64 blur planes.
                    for _ in 0..32 {
                        self.reserve_pixels(pixels)?;
                    }
                    self.pixel_work +=
                        crate::effects::stack(self.raw, node)?.len() as u64 * pixels * 32;
                    if self.pixel_work > MAX_PIXEL_WORK {
                        bail!("[limit-exceeded] effect pixel-work budget exceeded")
                    }
                    let full = (cw, ch) == surface.dimensions();
                    let source = if full {
                        surface.clone()
                    } else {
                        image::imageops::crop_imm(surface, cx, cy, cw, ch).to_image()
                    };
                    let applied = crate::effects::apply(
                        self.raw,
                        node,
                        &source,
                        self.document,
                        self.scale,
                        self.gradient_profiles,
                        &mut self.gradient_engines,
                    )?;
                    if full {
                        *surface = applied;
                    } else {
                        *surface = RgbaImage::new(surface.width(), surface.height());
                        image::imageops::replace(surface, &applied, i64::from(cx), i64::from(cy));
                    }
                    for _ in 0..32 {
                        self.live_bytes -= pixels * 4;
                    }
                }
            }
        }
        surface.opacity(opacity)?;
        Ok((surface, passed))
    }

    fn mask(&mut self, node: &Value, parent: kurbo::Affine) -> Result<Option<Vec<u8>>> {
        let Some(mask) = node.get("mask").filter(|m| m.get("resource").is_some()) else {
            return Ok(None);
        };
        let resource = &self.raw["mask_resources"][mask["resource"].as_str().unwrap()];
        let space = if mask["linked"].as_bool().unwrap() {
            parent
                * transform(node)?
                * if mask.get("anchor").is_some() {
                    transform(&json!({"transform":mask["anchor"]}))?
                } else {
                    kurbo::Affine::IDENTITY
                }
        } else {
            kurbo::Affine::IDENTITY
        };
        let space = space * transform(mask)?;
        let direct_affine = if mask["linked"] == true {
            crate::transform::affine(node)?
        } else {
            None
        };
        let space = direct_affine.unwrap_or(kurbo::Affine::IDENTITY) * space;
        let content = if resource["kind"] == "vector" {
            resource["node"].clone()
        } else {
            json!({"kind":"image","id":"mask-source","asset":resource["asset"],"x":0,"y":0,
                "width":self.raw["image_assets"][resource["asset"].as_str().unwrap()]["pixel_width"],"height":self.raw["image_assets"][resource["asset"].as_str().unwrap()]["pixel_height"],
                "fit":"fill","crop":[0,0,1,1],"position":[0.5,0.5],"opacity":1,"blend_mode":"normal","operations":[]})
        };
        self.reserve()?;
        let fragment =
            crate::image::fragment_svg(self.raw, self.page, self.document, &content, space)?;
        let mut pixels = crate::render::scene_to_rgba_proxy(&fragment, self.scale)?;
        for pixel in pixels.pixels_mut() {
            let alpha = if resource["kind"] == "vector" {
                pixel[3]
            } else {
                let value = (u32::from(pixel[0]) * 2126
                    + u32::from(pixel[1]) * 7152
                    + u32::from(pixel[2]) * 722
                    + 5000)
                    / 10000;
                ((value * u32::from(pixel[3]) + 127) / 255) as u8
            };
            *pixel = Rgba([
                255,
                255,
                255,
                if mask["invert"].as_bool().unwrap() {
                    255 - alpha
                } else {
                    alpha
                },
            ]);
        }
        let radius = mask["feather"].as_f64().unwrap() * f64::from(self.scale);
        if radius > 256.0 {
            bail!("[limit-exceeded] scaled mask feather exceeds 256 pixels")
        }
        if radius > 0.0 {
            for _ in 0..17 {
                self.reserve()?;
            }
            pixels = crate::imageops::apply_stack(
                pixels,
                &[
                    json!({"id":"mask-feather","kind":"blur","version":1,"enabled":true,"params":{"radius":radius}}),
                ],
            )?;
            for _ in 0..17 {
                self.release();
            }
        }
        if mask["linked"] == true && direct_affine.is_none() {
            if let Some(stack) = node
                .get("transforms")
                .and_then(Value::as_array)
                .filter(|s| !s.is_empty())
            {
                self.reserve()?;
                let mut operation_masks = Vec::new();
                for operation in stack {
                    operation_masks.push(self.mask(operation, kurbo::Affine::IDENTITY)?);
                }
                self.pixel_work +=
                    u64::from(self.width) * u64::from(self.height) * stack.len() as u64;
                if self.pixel_work > MAX_PIXEL_WORK {
                    bail!("[limit-exceeded] linked-mask transform pixel-work budget exceeded")
                }
                pixels = crate::transform::warp(&pixels, stack, &operation_masks, self.scale)?;
                for _ in operation_masks.iter().flatten() {
                    self.release();
                }
                self.release();
            }
        }
        let density = mask["density"].as_f64().unwrap();
        self.reserve()?; // retained mask coverage (conservatively one RGBA surface)
        let result = pixels
            .pixels()
            .map(|p| round(255.0 * (1.0 - density) + f64::from(p[3]) * density))
            .collect();
        self.release();
        Ok(Some(result))
    }
}

pub(crate) fn transform(node: &Value) -> Result<kurbo::Affine> {
    let Some(values) = node.get("transform") else {
        return Ok(kurbo::Affine::IDENTITY);
    };
    let values = values
        .as_array()
        .filter(|a| a.len() == 6)
        .context("transform must have six values")?;
    let mut out = [0.0; 6];
    for (i, v) in values.iter().enumerate() {
        out[i] = v
            .as_f64()
            .filter(|n| n.is_finite())
            .context("transform must be finite")?;
    }
    Ok(kurbo::Affine::new(out))
}

pub(crate) fn node_ref<'a>(
    raw: &'a Value,
    page_id: Option<&str>,
    id: &str,
) -> Result<(&'a Value, kurbo::Affine)> {
    let pages = raw["pages"].as_array().context("pages missing")?;
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|p| p["id"] == id)
            .context("page missing")?,
        None => pages.first().context("page missing")?,
    };
    fn walk<'a>(
        nodes: &'a [Value],
        id: &str,
        parent: kurbo::Affine,
    ) -> Result<Option<(&'a Value, kurbo::Affine)>> {
        for node in nodes {
            if node["id"] == id {
                return Ok(Some((node, parent)));
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                if let Some(found) = walk(children, id, parent * transform(node)?)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }
    for layer in page["layers"].as_array().context("layers missing")? {
        if let Some(found) = walk(
            layer["nodes"].as_array().context("nodes missing")?,
            id,
            kurbo::Affine::IDENTITY,
        )? {
            return Ok(found);
        }
    }
    bail!("node not found: {id}")
}

pub fn node_pixels(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    id: &str,
    scale: f32,
) -> Result<RgbaImage> {
    crate::scene::validate(raw)?;
    if !is_document(raw) || !(0.1..=8.0).contains(&scale) {
        bail!("node rendering requires v6 and scale .1..8")
    }
    let scope = crate::color::scene::Scope::Node { page: page_id, id };
    let mut resolved = crate::scene::with_resolved_tokens(raw)?;
    crate::color::scene::preview_scope(&mut resolved, document, scope)?;
    let raw = &resolved;
    let gradient_profiles = crate::color::gradient::profiles(raw, document, scope)?;
    let pages = raw["pages"].as_array().unwrap();
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|p| p["id"] == id)
            .context("page missing")?,
        None => &pages[0],
    };
    fn find<'a>(
        nodes: &'a [Value],
        id: &str,
        parent: kurbo::Affine,
    ) -> Result<Option<(&'a Value, kurbo::Affine)>> {
        for node in nodes {
            if node["id"] == id {
                return Ok(Some((node, parent)));
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                if let Some(found) = find(children, id, parent * transform(node)?)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }
    for layer in page["layers"].as_array().unwrap() {
        if let Some((node, parent)) = find(
            layer["nodes"].as_array().unwrap(),
            id,
            kurbo::Affine::IDENTITY,
        )? {
            if node["kind"] == "adjustment" {
                bail!("adjustment nodes have no independent alpha")
            }
            let width = (page["canvas"]["width"]
                .as_f64()
                .context("canvas width missing")?
                * f64::from(scale))
            .round()
            .max(1.0) as u32;
            let height = (page["canvas"]["height"]
                .as_f64()
                .context("canvas height missing")?
                * f64::from(scale))
            .round()
            .max(1.0) as u32;
            let mut renderer = Renderer {
                raw,
                page,
                document,
                scale,
                width,
                height,
                live_bytes: 0,
                pixel_work: 0,
                peak_live_bytes: 0,
                working: None,
                blend_context: None,
                conversion_clipped: false,
                gradient_profiles: &gradient_profiles,
                gradient_engines: Vec::new(),
                work_peak: 0,
                node_context: None,
                node_disk: None,
            };
            return renderer
                .siblings(std::slice::from_ref(node), parent)?
                .into_byte();
        }
    }
    bail!("node not found: {id}")
}

pub fn mask_coverage(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    name: &str,
) -> Result<Vec<u8>> {
    crate::scene::validate(raw)?;
    if !is_document(raw) {
        bail!("mask analysis requires version 6")
    }
    let scope = crate::color::scene::Scope::Mask {
        page: page_id,
        name,
    };
    let original_digest = raw["masks"][name]
        .as_str()
        .or_else(|| raw["mask_resources"].get(name).map(|_| name))
        .context("mask missing")?;
    let mut resolved = crate::scene::with_resolved_tokens(raw)?;
    let replacements = crate::color::scene::preview_scope(&mut resolved, document, scope)?;
    let identity = crate::color::scene::remap_identity(original_digest, &replacements);
    let name = identity.as_str();
    let scope = crate::color::scene::Scope::Mask {
        page: page_id,
        name,
    };
    let raw = &resolved;
    let gradient_profiles = crate::color::gradient::profiles(raw, document, scope)?;
    let pages = raw["pages"].as_array().unwrap();
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|p| p["id"] == id)
            .context("page missing")?,
        None => &pages[0],
    };
    let digest = raw
        .get("masks")
        .and_then(|m| m.get(name))
        .and_then(Value::as_str)
        .or_else(|| {
            raw.get("mask_resources")
                .filter(|m| m.get(name).is_some())
                .map(|_| name)
        })
        .context("mask missing")?;
    let (width, height) = (
        page["canvas"]["width"]
            .as_f64()
            .context("canvas width missing")?
            .round() as u64,
        page["canvas"]["height"]
            .as_f64()
            .context("canvas height missing")?
            .round() as u64,
    );
    let mut renderer = Renderer {
        raw,
        page,
        document,
        scale: 1.0,
        width: width as u32,
        height: height as u32,
        live_bytes: 0,
        pixel_work: 0,
        peak_live_bytes: 0,
        working: None,
        blend_context: None,
        conversion_clipped: false,
        gradient_profiles: &gradient_profiles,
        gradient_engines: Vec::new(),
        work_peak: 0,
        node_context: None,
        node_disk: None,
    };
    let node = json!({"kind":"rect","id":"mask-analysis","mask":{"resource":digest,"invert":false,"linked":false,"density":1,"feather":0,"transform":[1,0,0,1,0,0]}});
    renderer
        .mask(&node, kurbo::Affine::IDENTITY)?
        .context("mask coverage missing")
}

pub fn render(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    scale: f32,
) -> Result<RgbaImage> {
    render_with(raw, document, page_id, scale, true)
}

/// The visible content of a page over transparency, without the page background.
pub fn render_content(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    scale: f32,
) -> Result<RgbaImage> {
    render_with(raw, document, page_id, scale, false)
}

fn render_with(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    scale: f32,
    with_background: bool,
) -> Result<RgbaImage> {
    render_scene(raw, document, page_id, scale, with_background, false)?
        .surface
        .into_byte()
}

struct SceneRender {
    surface: SceneSurface,
    working: Option<crate::color::roles::Setting>,
    conversion_clipped: bool,
    peak_temporary_bytes: u64,
    pixel_work: u64,
}

/// Developing float scene rendering. Unsupported content fails explicitly;
/// callers receive unencoded working pixels and conversion diagnostics.
pub struct WorkingRender {
    pub surface: crate::blend::LinearSurface,
    pub working_space: crate::color::value::Destination,
    pub intent: crate::color::icc::Intent,
    pub conversion_clipped: bool,
    pub peak_temporary_bytes: u64,
    pub pixel_work: u64,
}
pub fn render_working(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    scale: f32,
    with_background: bool,
) -> Result<WorkingRender> {
    let rendered = render_scene(raw, document, page_id, scale, with_background, true)?;
    let working = rendered.working.context("working scene setting missing")?;
    match rendered.surface {
        SceneSurface::Working(surface) => Ok(WorkingRender {
            surface,
            working_space: working.destination,
            intent: working.intent,
            conversion_clipped: rendered.conversion_clipped,
            peak_temporary_bytes: rendered.peak_temporary_bytes,
            pixel_work: rendered.pixel_work,
        }),
        SceneSurface::Byte(_) => unreachable!("working scene target"),
    }
}

fn render_scene(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    scale: f32,
    with_background: bool,
    working: bool,
) -> Result<SceneRender> {
    crate::scene::validate(raw)?;
    if !is_document(raw) {
        bail!("compositing requires document version 6")
    }
    let materialized = crate::typeset::materialize(raw)?;
    let raw = materialized.as_ref().unwrap_or(raw);
    let working_setting = if working {
        let setting = crate::color::roles::effective(raw, crate::color::roles::Role::Working)?;
        if !setting.destination.is_linear_rgb() {
            bail!("[unsupported-capability] float scene rendering requires a named linear RGB working role")
        }
        Some(setting)
    } else {
        None
    };
    let blend_context = working_setting
        .as_ref()
        .map(|setting| crate::blend::LegacyBlendContext::new(setting.destination.space()))
        .transpose()?;
    let mut resolved = if working {
        None
    } else {
        Some(crate::scene::with_resolved_tokens(raw)?)
    };
    if let Some(resolved) = resolved.as_mut() {
        crate::color::scene::preview(resolved, document, page_id)?;
    }
    let raw = resolved.as_ref().unwrap_or(raw);
    if !(1.0 / 16384.0..=8.0).contains(&scale) {
        bail!("composite scale must be in 1/16384..=8")
    }
    let pages = raw["pages"].as_array().unwrap();
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|p| p["id"] == id)
            .with_context(|| format!("page not found: {id}"))?,
        None => &pages[0],
    };
    let width = page["canvas"]["width"]
        .as_f64()
        .context("canvas width missing")?
        .round() as u64;
    let height = page["canvas"]["height"]
        .as_f64()
        .context("canvas height missing")?
        .round() as u64;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        bail!("canvas dimensions must be in 1..=16384")
    }
    let w = (width as f64 * f64::from(scale)).round().max(1.0) as u32;
    let h = (height as f64 * f64::from(scale)).round().max(1.0) as u32;
    let scope = crate::color::scene::Scope::Page {
        page: page_id,
        visible_only: true,
    };
    let gradient_profiles = if working {
        let colors = crate::color::scene::slots(raw, scope)?;
        let destinations = crate::color::scene::destinations(raw, scope)?;
        crate::color::value::ProfileSet::load(
            raw,
            document,
            colors
                .iter()
                .filter_map(|slot| slot.color.destination().profile())
                .chain(
                    destinations
                        .iter()
                        .filter_map(|slot| slot.destination.profile()),
                ),
        )?
    } else {
        crate::color::gradient::profiles(
            raw,
            document,
            crate::color::scene::Scope::Page {
                page: page_id,
                visible_only: true,
            },
        )?
    };
    let mut renderer = Renderer {
        raw,
        page,
        document,
        scale,
        width: w,
        height: h,
        live_bytes: 0,
        pixel_work: 0,
        peak_live_bytes: 0,
        working: working_setting.as_ref(),
        blend_context: blend_context.as_ref(),
        conversion_clipped: false,
        gradient_profiles: &gradient_profiles,
        gradient_engines: Vec::new(),
        work_peak: 0,
        node_context: node_context(raw, page, document, scale, (w, h)),
        node_disk: None,
    };
    if renderer.node_context.is_some() {
        renderer.node_disk = crate::render_cache::directory(document);
    }
    renderer.reserve()?;
    let background = json!({"kind":"rect","id":"composite-background","x":0,"y":0,"width":width,"height":height,
        "style":{"fill":{"fallback":page["canvas"]["background"]}}});
    let mut result = if let Some(working) = working_setting.as_ref() {
        if with_background {
            let node = json!({"kind":"fill","id":"working-background","x":0,"y":0,"width":width,"height":height,"fill":{"kind":"solid","color":page["canvas"]["background"]}});
            let engine = crate::color::gradient::engine(
                &gradient_profiles,
                &mut renderer.gradient_engines,
                working.intent,
            )?;
            let fill = crate::fill::render_linear_prepared(
                raw,
                &node,
                document,
                kurbo::Affine::IDENTITY,
                &crate::fill::LinearFillOptions {
                    dimensions: (w, h),
                    scale,
                    working: working.destination.clone(),
                },
                engine,
            )?;
            renderer.conversion_clipped |= fill.conversion_clipped;
            SceneSurface::Working(fill.surface)
        } else {
            SceneSurface::Working(crate::blend::LinearSurface::new(
                w,
                h,
                working.destination.space(),
            )?)
        }
    } else if with_background {
        let fragment =
            crate::image::fragment_svg(raw, page, document, &background, kurbo::Affine::IDENTITY)?;
        SceneSurface::Byte(crate::render::scene_to_rgba_proxy(&fragment, scale)?)
    } else {
        SceneSurface::Byte(RgbaImage::new(w, h))
    };
    for layer in page["layers"].as_array().unwrap() {
        if layer.get("visible").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let surface =
            renderer.siblings(layer["nodes"].as_array().unwrap(), kurbo::Affine::IDENTITY)?;
        result.over(&surface, "normal", "srgb", blend_context.as_ref())?;
        renderer.release();
    }
    let conversion_clipped = renderer.conversion_clipped;
    let peak_temporary_bytes = renderer.peak_live_bytes;
    let pixel_work = renderer.pixel_work;
    drop(renderer);
    Ok(SceneRender {
        surface: result,
        working: working_setting,
        conversion_clipped,
        peak_temporary_bytes,
        pixel_work,
    })
}

/// Visible text lines of one page in canvas coordinates (group and node
/// transforms applied), for the searchable PDF text layer of v6 documents.
pub fn text_lines(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
) -> Result<Vec<crate::image::TextLine>> {
    crate::scene::validate(raw)?;
    let materialized = crate::typeset::materialize(raw)?;
    let raw = materialized.as_ref().unwrap_or(raw);
    let mut resolved = crate::scene::with_resolved_tokens(raw)?;
    crate::color::scene::preview(&mut resolved, document, page_id)?;
    let raw = &resolved;
    let pages = raw["pages"].as_array().context("document has no pages")?;
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|p| p["id"] == id)
            .with_context(|| format!("page not found: {id}"))?,
        None => pages.first().context("document has no pages")?,
    };
    fn walk(
        nodes: &[Value],
        parent: kurbo::Affine,
        raw: &Value,
        page: &Value,
        document: &Path,
        out: &mut Vec<crate::image::TextLine>,
    ) -> Result<()> {
        for node in nodes {
            if node.get("visible").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let world = parent * transform(node)?;
            match node["kind"].as_str() {
                Some("group") => walk(
                    node["children"].as_array().unwrap(),
                    world,
                    raw,
                    page,
                    document,
                    out,
                )?,
                Some("instance") => walk(
                    std::slice::from_ref(&node["fallback"]),
                    world,
                    raw,
                    page,
                    document,
                    out,
                )?,
                Some("text") => {
                    let mut local = node.clone();
                    if let Some(object) = local.as_object_mut() {
                        object.remove("transform");
                    }
                    let fragment = crate::image::fragment_svg(
                        raw,
                        page,
                        document,
                        &local,
                        kurbo::Affine::IDENTITY,
                    )?;
                    let [a, b, c, d, _, _] = world.as_coeffs();
                    let factor = (a * d - b * c).abs().sqrt();
                    for line in fragment.texts {
                        let point = world * kurbo::Point::new(line.x, line.y);
                        out.push(crate::image::TextLine {
                            x: point.x,
                            y: point.y,
                            size: line.size * factor,
                            content: line.content,
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for layer in page["layers"].as_array().unwrap() {
        if layer.get("visible").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        walk(
            layer["nodes"].as_array().unwrap(),
            kurbo::Affine::IDENTITY,
            raw,
            page,
            document,
            &mut out,
        )?;
    }
    Ok(out)
}

pub fn png(raw: &Value, document: &Path, page: Option<&str>, scale: f32) -> Result<Vec<u8>> {
    let pixels = render(raw, document, page, scale)?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels).write_to(&mut bytes, image::ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

pub fn svg(raw: &Value, document: &Path, page: Option<&str>, scale: f32) -> Result<String> {
    use base64::Engine;
    let pixels = render(raw, document, page, scale)?;
    let pages = raw["pages"].as_array().unwrap();
    let selected = page.map_or(&pages[0], |id| {
        pages.iter().find(|p| p["id"] == id).unwrap()
    });
    let width = &selected["canvas"]["width"];
    let height = &selected["canvas"]["height"];
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels).write_to(&mut bytes, image::ImageFormat::Png)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><image width="{width}" height="{height}" href="data:image/png;base64,{encoded}"/></svg>"#
    ))
}

#[cfg(test)]
mod plain_node_budget_tests {
    use super::*;

    #[test]
    fn many_small_plain_nodes_fit_the_pixel_work_budget() {
        let canvas = 1920_u64 * 1080;
        let rect = json!({"kind":"rect","id":"r","width":40,"height":40});
        let each = plain_node_pixels(&rect, canvas, 1.0);
        assert!(each < canvas / 100, "{each}");
        // 600 plain nodes previously charged 600 full canvases (> 1 Gpx).
        assert!(600 * each < MAX_PIXEL_WORK);
        assert!(600 * canvas > MAX_PIXEL_WORK);
        let effect =
            json!({"kind":"rect","id":"r","width":40,"height":40,"effects":[{"kind":"blur"}]});
        assert_eq!(plain_node_pixels(&effect, canvas, 1.0), canvas);
        let text = json!({"kind":"text","id":"t","width":40,"height":40});
        assert_eq!(plain_node_pixels(&text, canvas, 1.0), canvas);
    }
}

#[cfg(test)]
mod gradient_stop_tests {
    use super::*;

    #[test]
    fn gradient_map_stops_name_the_position_key() {
        let offset = json!({"stops":[{"offset":0,"rgb":[0,0,0]},{"offset":1,"rgb":[255,255,255]}]});
        let error = gradient(&offset).unwrap_err().to_string();
        assert!(error.contains("\"position\", not \"offset\""), "{error}");
        let position =
            json!({"stops":[{"position":0,"rgb":[0,0,0]},{"position":1,"rgb":[255,255,255]}]});
        assert_eq!(gradient(&position).unwrap().len(), 2);
    }
}

#[cfg(test)]
mod node_cache_tests {
    use super::*;

    fn scene(raw: &Value, page: &str) -> (Vec<u8>, u64, u64) {
        let render = render_scene(raw, Path::new(""), Some(page), 0.75, true, false).unwrap();
        let (peak, work) = (render.peak_temporary_bytes, render.pixel_work);
        (render.surface.into_byte().unwrap().into_raw(), peak, work)
    }

    #[test]
    fn cached_node_surfaces_render_the_same_bytes_and_budget() {
        let mut raw: Value =
            serde_json::from_str(include_str!("../docs/fixtures/v6-composite.pen")).unwrap();
        let blurred = json!({"kind":"ellipse","id":"cloud","cx":3.5,"cy":2.5,"radius_x":2.5,"radius_y":1.5,
            "style":{"fill":{"fallback":"#ffffff"}},
            "effects":[{"id":"soft","kind":"blur","enabled":true,"opacity":1,"blend_mode":"normal","params":{"radius":1.5}}]});
        raw["pages"][0]["layers"][0]["nodes"]
            .as_array_mut()
            .unwrap()
            .insert(3, blurred);
        let pages: Vec<String> = raw["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_string())
            .collect();
        let fresh: Vec<_> = pages.iter().map(|p| scene(&raw, p)).collect();
        cache_node_surfaces(64 << 20);
        for _ in 0..2 {
            for (page, expected) in pages.iter().zip(&fresh) {
                assert_eq!(&scene(&raw, page), expected, "page {page}");
            }
        }
        // Moving one node re-renders it; the others come from the cache.
        raw["pages"][0]["layers"][0]["nodes"][3]["cx"] = json!(5);
        let cached = scene(&raw, &pages[0]);
        cache_node_surfaces(0);
        assert_eq!(cached, scene(&raw, &pages[0]));
        assert_ne!(cached.0, fresh[0].0);
    }

    #[test]
    fn a_hit_that_would_cross_a_limit_renders_and_fails_the_same_way() {
        cache_node_surfaces(64 << 20);
        let mut raw: Value =
            serde_json::from_str(include_str!("../docs/fixtures/v6-composite.pen")).unwrap();
        raw["pages"].as_array_mut().unwrap().truncate(1);
        scene(&raw, "composite");
        let key = NODE_CACHE.with(|c| c.borrow().as_ref().unwrap().entries.len());
        assert!(key > 0);
        // An entry claims a peak beyond the budget: the node must render instead.
        NODE_CACHE.with(|c| {
            for entry in c.borrow_mut().as_mut().unwrap().entries.values_mut() {
                entry.live_peak = MAX_TEMP_BYTES;
                entry.pixels = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 4]));
            }
        });
        let again = scene(&raw, "composite");
        cache_node_surfaces(0);
        assert_eq!(again, scene(&raw, "composite"));
    }
}

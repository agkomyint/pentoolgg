//! The `scene3d` document node: a rectangle whose pixels come from rendering
//! an embedded or external model with the deterministic CPU renderer.
//!
//! Format version 10 adds the `scene3d` node kind and the top-level `models`
//! map (digest -> record). Everything else is unchanged from version 9.

use super::model::{Model, LIMITS};
use super::render::{self, Environment, Light, Options, Projection, Scene};
use super::{gltf, math, obj};
use anyhow::{bail, ensure, Context, Result};
use base64::Engine as _;
use serde_json::{json, Map, Value};
use std::cell::Cell;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

pub const VERSION: u64 = 10;
/// Longest rendered edge in pixels.
pub const MAX_RENDER_EDGE: f64 = 4096.0;
const FORMATS: [&str; 3] = ["glb", "gltf", "obj"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererPreference {
    Reference,
    Accelerated,
}

thread_local! {
    static RENDERER: Cell<RendererPreference> = const { Cell::new(RendererPreference::Reference) };
    static RENDERER_REPORTED: Cell<bool> = const { Cell::new(false) };
}

pub struct RendererGuard {
    preference: RendererPreference,
    reported: bool,
}

impl Drop for RendererGuard {
    fn drop(&mut self) {
        RENDERER.set(self.preference);
        RENDERER_REPORTED.set(self.reported);
    }
}

/// Select the scene3d renderer for the current export operation. The guard
/// restores the previous selection so servers and concurrent exports cannot
/// leak a request into later work.
pub fn select_renderer(preference: RendererPreference) -> RendererGuard {
    let previous = RENDERER.replace(preference);
    let reported = RENDERER_REPORTED.replace(false);
    RendererGuard {
        preference: previous,
        reported,
    }
}

pub fn is_v10(raw: &Value) -> bool {
    raw.get("version")
        .and_then(Value::as_u64)
        .is_some_and(|v| v >= VERSION)
}

fn number(o: &Map<String, Value>, key: &str) -> Result<f64> {
    let v = o
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("[invalid-input] scene3d `{key}` must be a number"))?;
    ensure!(
        v.is_finite(),
        "[invalid-input] scene3d `{key}` must be finite"
    );
    Ok(v)
}

fn optional(o: &Map<String, Value>, key: &str, default: f64, lo: f64, hi: f64) -> Result<f64> {
    match o.get(key) {
        None => Ok(default),
        Some(_) => {
            let v = number(o, key)?;
            ensure!(
                (lo..=hi).contains(&v),
                "[invalid-input] scene3d `{key}` must be within {lo}..={hi}"
            );
            Ok(v)
        }
    }
}

fn vec3(value: &Value, what: &str) -> Result<[f64; 3]> {
    let a = value
        .as_array()
        .filter(|a| a.len() == 3)
        .with_context(|| format!("[invalid-input] scene3d {what} must be [x, y, z]"))?;
    let mut out = [0.0; 3];
    for (slot, v) in out.iter_mut().zip(a) {
        *slot = v
            .as_f64()
            .filter(|v| v.is_finite() && v.abs() <= 1e9)
            .with_context(|| format!("[invalid-input] scene3d {what} must be finite numbers"))?;
    }
    Ok(out)
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa` or `none` -> straight sRGB 0..1 RGBA.
pub fn parse_color(text: &str) -> Result<[f64; 4]> {
    if text == "none" {
        return Ok([0.0; 4]);
    }
    let hex = text.strip_prefix('#').with_context(|| {
        format!("[invalid-input] color `{text}` must be #rgb, #rrggbb or #rrggbbaa")
    })?;
    let expanded: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => hex.to_string(),
        _ => bail!("[invalid-input] color `{text}` must be #rgb, #rrggbb or #rrggbbaa"),
    };
    let byte = |i: usize| -> Result<f64> {
        let part = expanded
            .get(i..i + 2)
            .with_context(|| format!("[invalid-input] color `{text}` is malformed"))?;
        Ok(f64::from(
            u8::from_str_radix(part, 16)
                .with_context(|| format!("[invalid-input] color `{text}` is not hexadecimal"))?,
        ) / 255.0)
    };
    Ok([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if expanded.len() == 8 { byte(6)? } else { 1.0 },
    ])
}

fn color_field(value: &Value, what: &str) -> Result<[f64; 3]> {
    let c = parse_color(
        value
            .as_str()
            .with_context(|| format!("[invalid-input] scene3d {what} must be a color string"))?,
    )?;
    Ok([c[0], c[1], c[2]])
}

/// Validate the top-level `models` map for a version-10 document.
pub fn validate_models(raw: &Value, version: u64) -> Result<()> {
    if version < VERSION {
        return Ok(());
    }
    let Some(models) = raw.get("models") else {
        return Ok(());
    };
    let models = models
        .as_object()
        .context("[invalid-input] `models` must be an object")?;
    ensure!(
        models.len() <= 256,
        "[limit-exceeded] a document holds at most 256 models"
    );
    for (digest, record) in models {
        let record = record
            .as_object()
            .with_context(|| format!("[invalid-input] model {digest} must be an object"))?;
        let format = record.get("format").and_then(Value::as_str).unwrap_or("");
        ensure!(
            FORMATS.contains(&format),
            "[unsupported-capability] model {digest} has unsupported format `{format}` (glb, gltf, obj)"
        );
        let storage = record
            .get("storage")
            .and_then(Value::as_object)
            .with_context(|| format!("[invalid-input] model {digest} has no storage"))?;
        match storage.get("kind").and_then(Value::as_str) {
            Some("embedded") => {
                let data = storage
                    .get("data")
                    .and_then(Value::as_str)
                    .with_context(|| format!("[invalid-input] model {digest} has no data"))?;
                ensure!(
                    data.len() / 4 * 3 <= LIMITS.max_bytes,
                    "[limit-exceeded] model {digest} exceeds the {} byte limit",
                    LIMITS.max_bytes
                );
            }
            Some("external") => {
                let path = storage
                    .get("path")
                    .and_then(Value::as_str)
                    .with_context(|| format!("[invalid-input] model {digest} has no path"))?;
                crate::resource::safe_relative_path(Path::new(path))?;
            }
            other => bail!(
                "[unsupported-capability] model {digest} storage kind {other:?} is unsupported"
            ),
        }
        if let Some(uv) = record.get("uv") {
            super::uv::validate(uv).with_context(|| format!("model {digest}"))?;
        }
    }
    Ok(())
}

/// Structural validation of one node (document-level references included).
pub fn validate_node(node: &Map<String, Value>, raw: &Value) -> Result<()> {
    let id = node.get("id").and_then(Value::as_str).unwrap_or("?");
    let ctx = || format!("scene3d node {id}");
    let width = number(node, "width").with_context(ctx)?;
    let height = number(node, "height").with_context(ctx)?;
    ensure!(
        width > 0.0 && height > 0.0,
        "[invalid-input] {} needs a positive width and height",
        ctx()
    );
    let ratio = optional(node, "pixel_ratio", 1.0, 0.1, 8.0).with_context(ctx)?;
    ensure!(
        width.max(height) * ratio <= MAX_RENDER_EDGE,
        "[limit-exceeded] {} would render {} pixels on its longest side; the limit is {MAX_RENDER_EDGE}",
        ctx(),
        (width.max(height) * ratio).round()
    );
    let digest = node
        .get("model")
        .and_then(Value::as_str)
        .with_context(|| format!("[invalid-input] {} needs a `model` digest", ctx()))?;
    ensure!(
        raw.get("models").and_then(|m| m.get(digest)).is_some(),
        "[missing-resource] {} refers to model {digest}, which is not in `models`",
        ctx()
    );
    scene_for(node, None).with_context(ctx)?;
    validate_material_overrides(node).with_context(ctx)?;
    Ok(())
}

fn validate_material_overrides(node: &Map<String, Value>) -> Result<()> {
    let Some(overrides) = node.get("material_overrides") else {
        return Ok(());
    };
    let overrides = overrides
        .as_object()
        .context("[invalid-input] material_overrides must be an object")?;
    ensure!(
        overrides.len() <= LIMITS.max_materials,
        "[limit-exceeded] too many material overrides"
    );
    for (id, value) in overrides {
        id.parse::<usize>().with_context(|| {
            format!("[invalid-input] material override key `{id}` must be an index")
        })?;
        let o = value
            .as_object()
            .context("[invalid-input] material override must be an object")?;
        for key in o.keys() {
            ensure!(
                matches!(
                    key.as_str(),
                    "base_color" | "metallic" | "roughness" | "emissive" | "double_sided"
                ),
                "[invalid-input] unknown material override field `{key}`"
            );
        }
        if let Some(v) = o.get("base_color") {
            parse_color(
                v.as_str()
                    .context("[invalid-input] base_color must be a color string")?,
            )?;
        }
        if let Some(v) = o.get("emissive") {
            parse_color(
                v.as_str()
                    .context("[invalid-input] emissive must be a color string")?,
            )?;
        }
        for key in ["metallic", "roughness"] {
            if let Some(v) = o.get(key) {
                let v = v
                    .as_f64()
                    .with_context(|| format!("[invalid-input] {key} must be a number"))?;
                ensure!(
                    v.is_finite() && (0.0..=1.0).contains(&v),
                    "[invalid-input] {key} must be within 0..=1"
                );
            }
        }
        if let Some(v) = o.get("double_sided") {
            ensure!(
                v.is_boolean(),
                "[invalid-input] double_sided must be boolean"
            );
        }
    }
    Ok(())
}

fn apply_material_overrides(model: &mut Model, node: &Map<String, Value>) -> Result<()> {
    let Some(overrides) = node.get("material_overrides").and_then(Value::as_object) else {
        return Ok(());
    };
    for (id, value) in overrides {
        let index: usize = id.parse()?;
        let material = model.materials.get_mut(index)
            .with_context(|| format!("[missing-resource] material {index} does not exist; inspect the scene for valid IDs"))?;
        let o = value
            .as_object()
            .context("[invalid-input] material override must be an object")?;
        if let Some(v) = o.get("base_color").and_then(Value::as_str) {
            let c = parse_color(v)?;
            material.base_color = [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32];
        }
        if let Some(v) = o.get("emissive").and_then(Value::as_str) {
            let c = parse_color(v)?;
            material.emissive = [c[0] as f32, c[1] as f32, c[2] as f32];
        }
        if let Some(v) = o.get("metallic").and_then(Value::as_f64) {
            material.metallic = v as f32;
        }
        if let Some(v) = o.get("roughness").and_then(Value::as_f64) {
            material.roughness = v as f32;
        }
        if let Some(v) = o.get("double_sided").and_then(Value::as_bool) {
            material.double_sided = v;
        }
    }
    Ok(())
}

/// Every `scene3d` node in the document (pages and parent pages).
fn collect<'a>(raw: &'a Value, out: &mut Vec<&'a Map<String, Value>>) {
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Map<String, Value>>) {
        if let Some(o) = node.as_object() {
            if o.get("kind").and_then(Value::as_str) == Some("scene3d") {
                out.push(o);
            }
            for child in o
                .get("children")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(child, out);
            }
        }
    }
    fn layers<'a>(container: &'a Value, out: &mut Vec<&'a Map<String, Value>>) {
        for layer in container
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for node in layer
                .get("nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(node, out);
            }
        }
    }
    for page in raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        layers(page, out);
    }
    for parent in raw["layout"]["parents"].as_array().into_iter().flatten() {
        layers(parent, out);
    }
}

/// Document-wide node check: gates the kind by format version and validates
/// every node against the `models` map.
pub fn validate_nodes(raw: &Value, version: u64) -> Result<()> {
    let mut nodes = Vec::new();
    collect(raw, &mut nodes);
    for node in nodes {
        let id = node.get("id").and_then(Value::as_str).unwrap_or("?");
        ensure!(
            version >= VERSION,
            "[unsupported-capability] scene3d node {id} requires document format version 10; run `pentool migrate --target 10` first"
        );
        validate_node(node, raw)?;
    }
    Ok(())
}

/// `migrate --target 10`: version 9 plus the (initially empty) `models` map.
pub fn migrate(raw: Value) -> Result<Value> {
    if is_v10(&raw) {
        crate::scene::validate(&raw)?;
        return Ok(raw);
    }
    ensure!(
        raw.get("models").is_none(),
        "[unsupported-capability] an existing \"models\" extension conflicts with version 10; rename the extension before migration"
    );
    let mut raw = crate::spreads::migrate(raw)?;
    raw["version"] = json!(VERSION);
    crate::scene::validate(&raw)?;
    Ok(raw)
}

/// Bring a version-10 document down to 9. Refused, naming the node or model,
/// while the document uses 3D; other versions pass through untouched.
pub fn to_v9(mut raw: Value) -> Result<Value> {
    if !is_v10(&raw) {
        return Ok(raw);
    }
    let mut nodes = Vec::new();
    collect(&raw, &mut nodes);
    if let Some(node) = nodes.first() {
        bail!(
            "[unsupported-capability] version 9 cannot represent scene3d node {}; remove it first",
            node.get("id").and_then(Value::as_str).unwrap_or("?")
        );
    }
    if let Some(digest) = raw
        .get("models")
        .and_then(Value::as_object)
        .and_then(|m| m.keys().next())
    {
        bail!(
            "[unsupported-capability] version 9 cannot represent model {digest}; remove it first"
        );
    }
    let object = raw.as_object_mut().context("document must be an object")?;
    object.remove("models");
    raw["version"] = json!(crate::spreads::VERSION);
    Ok(raw)
}

/// Build the renderer scene and options from a node. `model` supplies bounds
/// for automatic framing; validation passes `None` and uses a unit stand-in.
pub fn scene_for(node: &Map<String, Value>, model: Option<&Model>) -> Result<(Scene, Options)> {
    let width = number(node, "width")?;
    let height = number(node, "height")?;
    let ratio = optional(node, "pixel_ratio", 1.0, 0.1, 8.0)?;
    let supersample = optional(
        node,
        "supersample",
        2.0,
        1.0,
        render::MAX_SUPERSAMPLE as f64,
    )?;
    ensure!(
        supersample.fract() == 0.0,
        "[invalid-input] scene3d `supersample` must be an integer"
    );
    let px = |v: f64| ((v * ratio).round() as u32).max(1);

    let stand_in;
    let framing: &Model = match model {
        Some(m) => m,
        None => {
            stand_in = Model::default();
            &stand_in
        }
    };
    let cam = node.get("camera").and_then(Value::as_object);
    let empty = Map::new();
    let cam = cam.unwrap_or(&empty);
    let azimuth = optional(cam, "azimuth", 30.0, -360.0, 360.0)?;
    let elevation = optional(cam, "elevation", 15.0, -89.0, 89.0)?;
    let mut fov = optional(cam, "fov", 35.0, 1.0, 170.0)?;
    let physical = node.get("physical_camera").and_then(Value::as_object);
    if let Some(p) = physical {
        let lens = physical_lens(p)?;
        let from_lens = lens.vertical_fov_deg;
        if cam.contains_key("fov") {
            ensure!(
                (from_lens - fov).abs() <= 0.01,
                "[invalid-input] camera fov {fov} and physical_camera ({} mm on a {} mm sensor = {:.3} degrees) disagree; set one or make them match",
                lens.focal_length_mm,
                lens.sensor_height_mm,
                from_lens
            );
        }
        fov = from_lens;
    }
    let orthographic = match cam.get("projection").and_then(Value::as_str) {
        None | Some("perspective") => false,
        Some("orthographic") => true,
        Some(other) => {
            bail!("[invalid-input] camera projection `{other}` must be perspective or orthographic")
        }
    };
    ensure!(
        !(orthographic && physical.is_some()),
        "[invalid-input] physical_camera applies to perspective cameras only"
    );
    let mut camera = render::frame_camera(framing, azimuth, elevation, fov);
    if orthographic {
        let (min, max) = framing.bounds().unwrap_or(([-1.0; 3], [1.0; 3]));
        let diameter = math::length(math::sub(max, min)).max(1e-6);
        camera.projection = Projection::Orthographic {
            height: optional(cam, "height", diameter * 1.1, 1e-6, 1e9)?,
        };
    }
    if let Some(eye) = cam.get("eye") {
        camera.eye = vec3(eye, "camera eye")?;
        camera.target = vec3(
            cam.get("target")
                .context("[invalid-input] camera `eye` needs a `target`")?,
            "camera target",
        )?;
        camera.near = optional(cam, "near", 0.1, 1e-6, 1e9)?;
        camera.far = optional(cam, "far", 1000.0, 1e-6, 1e12)?;
    } else if let Some(target) = cam.get("target") {
        // A target by itself pans an automatically framed/orbit camera while
        // preserving its distance and direction to the subject.
        let target = vec3(target, "camera target")?;
        let offset = math::sub(target, camera.target);
        camera.eye = math::add(camera.eye, offset);
        camera.target = target;
    }
    if let Some(up) = cam.get("up") {
        camera.up = vec3(up, "camera up")?;
    }

    let lights = match node.get("lights") {
        None => render::default_lights(),
        Some(list) => {
            let list = list
                .as_array()
                .context("[invalid-input] scene3d `lights` must be an array")?;
            ensure!(
                list.len() <= render::MAX_LIGHTS,
                "[limit-exceeded] at most {} lights are supported",
                render::MAX_LIGHTS
            );
            let mut out = Vec::new();
            for light in list {
                let o = light
                    .as_object()
                    .context("[invalid-input] a light must be an object")?;
                let color = match o.get("color") {
                    Some(c) => color_field(c, "light color")?,
                    None => [1.0; 3],
                };
                let intensity = optional(o, "intensity", 1.0, 0.0, 1e6)?;
                out.push(match o.get("kind").and_then(Value::as_str) {
                    Some("directional") => Light::Directional {
                        direction: math::normalize(vec3(
                            o.get("direction")
                                .context("[invalid-input] a directional light needs `direction`")?,
                            "light direction",
                        )?),
                        color,
                        intensity,
                    },
                    Some("point") => Light::Point {
                        position: vec3(
                            o.get("position")
                                .context("[invalid-input] a point light needs `position`")?,
                            "light position",
                        )?,
                        color,
                        intensity,
                    },
                    Some("spot") => {
                        let outer = optional(o, "outer_deg", 35.0, 1.0, 89.0)?;
                        let inner = optional(o, "inner_deg", outer * 0.8, 0.0, outer)?;
                        ensure!(
                            inner <= outer,
                            "[invalid-input] spot inner_deg must not exceed outer_deg"
                        );
                        let rad = math::PI / 180.0;
                        Light::Spot {
                            position: vec3(
                                o.get("position")
                                    .context("[invalid-input] a spot light needs `position`")?,
                                "light position",
                            )?,
                            direction: math::normalize(vec3(
                                o.get("direction")
                                    .context("[invalid-input] a spot light needs `direction`")?,
                                "light direction",
                            )?),
                            color,
                            intensity,
                            inner_cos: math::cos(inner * rad),
                            outer_cos: math::cos(outer * rad),
                        }
                    }
                    other => {
                        bail!(
                            "[invalid-input] light kind {other:?} must be directional, point or spot"
                        )
                    }
                });
            }
            out
        }
    };
    let mut environment = Environment::default();
    if let Some(env) = node.get("environment").and_then(Value::as_object) {
        if let Some(c) = env.get("sky") {
            environment.sky = color_field(c, "environment sky")?;
        }
        if let Some(c) = env.get("ground") {
            environment.ground = color_field(c, "environment ground")?;
        }
        environment.intensity = optional(env, "intensity", environment.intensity, 0.0, 100.0)?;
    }
    let background = match node.get("background").and_then(Value::as_str) {
        Some(text) => parse_color(text)?,
        None => [0.0; 4],
    };
    Ok((
        Scene {
            camera,
            lights,
            environment,
            background,
            shadow_size: if node.get("shadows").and_then(Value::as_bool).unwrap_or(true) {
                let size = optional(
                    node,
                    "shadow_map_size",
                    super::shadow::DEFAULT_SHADOW_SIZE as f64,
                    super::shadow::MIN_SHADOW_SIZE as f64,
                    super::shadow::MAX_SHADOW_SIZE as f64,
                )?;
                ensure!(
                    size.fract() == 0.0,
                    "[invalid-input] scene3d `shadow_map_size` must be an integer"
                );
                size as u32
            } else {
                0
            },
            exposure_ev: (optional(node, "exposure_ev", 0.0, -16.0, 16.0)?
                + match physical {
                    Some(p) => optional(p, "exposure_ev", 0.0, -16.0, 16.0)?,
                    None => 0.0,
                })
            .clamp(-16.0, 16.0),
        },
        Options {
            width: px(width),
            height: px(height),
            supersample: supersample as u32,
        },
    ))
}

struct PhysicalLens {
    focal_length_mm: f64,
    sensor_height_mm: f64,
    vertical_fov_deg: f64,
}

/// Parse and validate the `physical_camera` object.
fn physical_lens(p: &Map<String, Value>) -> Result<PhysicalLens> {
    for key in p.keys() {
        ensure!(
            matches!(
                key.as_str(),
                "focal_length_mm"
                    | "sensor_mm"
                    | "aperture_f"
                    | "focus_distance"
                    | "exposure_ev"
                    | "samples"
            ),
            "[invalid-input] unknown physical_camera field `{key}`"
        );
    }
    let focal = number(p, "focal_length_mm")
        .context("[invalid-input] physical_camera needs `focal_length_mm`")?;
    ensure!(
        (1.0..=2000.0).contains(&focal),
        "[invalid-input] focal_length_mm must be 1..=2000"
    );
    let sensor = match p.get("sensor_mm") {
        None => [36.0, 24.0],
        Some(v) => {
            let a = v
                .as_array()
                .filter(|a| a.len() == 2)
                .context("[invalid-input] sensor_mm must be [width, height]")?;
            let mut out = [0.0; 2];
            for (slot, x) in out.iter_mut().zip(a) {
                *slot = x
                    .as_f64()
                    .filter(|x| x.is_finite() && *x > 0.0 && *x <= 1000.0)
                    .context("[invalid-input] sensor_mm values must be in (0, 1000]")?;
            }
            out
        }
    };
    let fov = 2.0 * math::atan(sensor[1] / (2.0 * focal)) * 180.0 / math::PI;
    ensure!(
        (1.0..=170.0).contains(&fov),
        "[invalid-input] {focal} mm on a {} mm sensor gives a {fov:.2} degree field of view; it must be 1..=170",
        sensor[1]
    );
    if let Some(v) = p.get("aperture_f") {
        let f = v
            .as_f64()
            .filter(|f| f.is_finite() && (0.5..=64.0).contains(f));
        ensure!(f.is_some(), "[invalid-input] aperture_f must be 0.5..=64");
    }
    if p.contains_key("focus_distance") {
        optional(p, "focus_distance", 1.0, 1e-6, 1e9)?;
    }
    if p.contains_key("samples") {
        let s = optional(p, "samples", 16.0, 1.0, render::MAX_LENS_SAMPLES as f64)?;
        ensure!(
            s.fract() == 0.0,
            "[invalid-input] physical_camera `samples` must be an integer"
        );
    }
    Ok(PhysicalLens {
        focal_length_mm: focal,
        sensor_height_mm: sensor[1],
        vertical_fov_deg: fov,
    })
}

/// Depth-of-field lens for a node, or `None` when `aperture_f` is unset.
/// World units are meters: the aperture diameter is `focal_length / f-number`.
pub fn lens_for(node: &Map<String, Value>, scene: &Scene) -> Result<Option<render::Lens>> {
    let Some(p) = node.get("physical_camera").and_then(Value::as_object) else {
        return Ok(None);
    };
    let lens = physical_lens(p)?;
    let Some(f) = p.get("aperture_f").and_then(Value::as_f64) else {
        return Ok(None);
    };
    let default_focus = math::length(math::sub(scene.camera.target, scene.camera.eye));
    Ok(Some(render::Lens {
        aperture_radius: lens.focal_length_mm / f / 2.0 / 1000.0,
        focus_distance: optional(p, "focus_distance", default_focus, 1e-6, 1e9)?,
        samples: optional(p, "samples", 16.0, 1.0, render::MAX_LENS_SAMPLES as f64)? as u32,
    }))
}

type ModelCache = Mutex<Vec<(String, Arc<Model>)>>;
type PngCache = Mutex<Vec<(String, Arc<Vec<u8>>)>>;

fn model_cache() -> &'static ModelCache {
    static C: OnceLock<ModelCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

fn png_cache() -> &'static PngCache {
    static C: OnceLock<PngCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

fn remember<T>(cache: &Mutex<Vec<(String, T)>>, key: String, value: T, limit: usize) {
    if let Ok(mut c) = cache.lock() {
        c.retain(|(k, _)| k != &key);
        c.push((key, value));
        while c.len() > limit {
            c.remove(0);
        }
    }
}

/// Cache key: the model digest plus its UV overlay, so editing an overlay
/// never serves a stale model or render.
fn model_key(raw: &Value, digest: &str) -> String {
    match raw["models"][digest].get("uv") {
        Some(uv) if uv.as_object().is_some_and(|m| !m.is_empty()) => format!(
            "{digest}+{}",
            crate::resource::sha256(uv.to_string().as_bytes())
        ),
        _ => digest.to_string(),
    }
}

/// Load, verify and import a model. `document_dir` anchors external paths.
pub fn load_model(raw: &Value, document_dir: &Path, digest: &str) -> Result<Arc<Model>> {
    let cache_key = model_key(raw, digest);
    if let Ok(c) = model_cache().lock() {
        if let Some((_, m)) = c.iter().find(|(k, _)| k == &cache_key) {
            return Ok(Arc::clone(m));
        }
    }
    let record = raw["models"]
        .get(digest)
        .and_then(Value::as_object)
        .with_context(|| format!("[missing-resource] model {digest} is not in `models`"))?;
    let storage = record["storage"]
        .as_object()
        .context("[invalid-input] model storage is missing")?;
    let document = document_dir.join("document.pen");
    let (bytes, base): (Vec<u8>, Option<std::path::PathBuf>) = match storage["kind"].as_str() {
        Some("embedded") => (
            base64::engine::general_purpose::STANDARD
                .decode(storage["data"].as_str().unwrap_or(""))
                .context("[malformed-resource] embedded model is not valid base64")?,
            None,
        ),
        Some("external") => {
            let path = Path::new(storage["path"].as_str().unwrap_or(""));
            (
                crate::resource::read_external_offline_limited(
                    &document,
                    path,
                    digest,
                    LIMITS.max_bytes,
                )?,
                path.parent().map(Path::to_path_buf),
            )
        }
        other => bail!("[unsupported-capability] model storage kind {other:?} is unsupported"),
    };
    crate::resource::verify(&bytes, digest)?;
    let resolver = |uri: &str| -> Result<Vec<u8>> {
        let Some(base) = &base else {
            bail!("[unsafe-path] embedded models cannot reference external file `{uri}`");
        };
        ensure!(
            !uri.contains(':') && !uri.contains('?'),
            "[unsafe-path] model resource `{uri}` must be a relative file path"
        );
        let decoded = uri.replace("%20", " ");
        let relative = base.join(decoded);
        crate::resource::safe_relative_path(&relative)?;
        let full = document_dir.join(&relative);
        let canonical_root = document_dir
            .canonicalize()
            .context("[missing-resource] document directory")?;
        let canonical = full
            .canonicalize()
            .with_context(|| format!("[missing-resource] {}", full.display()))?;
        ensure!(
            canonical.starts_with(&canonical_root),
            "[unsafe-path] model resource `{uri}` resolves outside the document root"
        );
        let meta = std::fs::metadata(&canonical)?;
        ensure!(
            meta.len() as usize <= LIMITS.max_bytes,
            "[limit-exceeded] model resource `{uri}` is too large"
        );
        Ok(std::fs::read(&canonical)?)
    };
    let model = match record["format"].as_str() {
        Some("glb") | Some("gltf") => gltf::import(&bytes, &resolver)?,
        Some("obj") => obj::import(&bytes, &resolver)?,
        other => bail!("[unsupported-capability] model format {other:?} is unsupported"),
    };
    let mut model = model;
    if let Some(uv) = record
        .get("uv")
        .filter(|u| u.as_object().is_some_and(|m| !m.is_empty()))
    {
        super::uv::apply(&mut model, uv)?;
    }
    let model = Arc::new(model);
    remember(model_cache(), cache_key, Arc::clone(&model), 2);
    Ok(model)
}

/// Render a node to PNG bytes (straight alpha sRGB).
pub fn render_png(raw: &Value, document_dir: &Path, node: &Map<String, Value>) -> Result<Vec<u8>> {
    if RENDERER.get() == RendererPreference::Accelerated && !RENDERER_REPORTED.replace(true) {
        let status = super::gpu::availability();
        eprintln!(
            "{}",
            json!({
                "renderer_requested": "accelerated",
                "renderer_used": "reference",
                "fallback": status.reason,
                "adapter": status.adapter,
                "backend": status.backend,
            })
        );
    }
    let digest = node
        .get("model")
        .and_then(Value::as_str)
        .context("[invalid-input] scene3d node needs a `model` digest")?;
    // Everything that affects pixels, and nothing else (not x/y/id/opacity).
    let key = {
        let mut spec = Map::new();
        for k in [
            "model",
            "width",
            "height",
            "pixel_ratio",
            "camera",
            "physical_camera",
            "lights",
            "environment",
            "background",
            "exposure_ev",
            "supersample",
            "shadows",
            "shadow_map_size",
            "material_overrides",
        ] {
            if let Some(v) = node.get(k) {
                spec.insert(k.into(), v.clone());
            }
        }
        spec.insert("uv".into(), json!(model_key(raw, digest)));
        crate::resource::sha256(serde_json::to_string(&Value::Object(spec))?.as_bytes())
    };
    if let Ok(c) = png_cache().lock() {
        if let Some((_, png)) = c.iter().find(|(k, _)| k == &key) {
            return Ok(png.as_ref().clone());
        }
    }
    let loaded = load_model(raw, document_dir, digest)?;
    let mut model = loaded.as_ref().clone();
    apply_material_overrides(&mut model, node)?;
    let (scene, options) = scene_for(node, Some(&model))?;
    let lens = lens_for(node, &scene)?;
    let image = render::render_with_lens(&model, &scene, &options, lens.as_ref())?;
    let buffer = image::RgbaImage::from_raw(image.width, image.height, image.rgba)
        .context("renderer produced a malformed image")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut out, image::ImageFormat::Png)
        .context("could not encode the 3D render")?;
    let png = out.into_inner();
    remember(png_cache(), key, Arc::new(png.clone()), 8);
    Ok(png)
}

/// Compact description for `scene3d inspect`.
pub fn inspect(model: &Model, offset: usize, limit: usize) -> Value {
    let bounds = model.bounds();
    let mut records = Vec::new();
    records.extend(model.primitives.iter().enumerate().map(|(index, p)| json!({
        "id": format!("primitive-{index}"), "kind": "primitive", "index": index,
        "name": p.name, "triangles": p.triangle_count(), "uv_sets": p.uvs.len(), "material": p.material
    })));
    records.extend(model.materials.iter().enumerate().map(|(index, m)| {
        json!({
            "id": format!("material-{index}"), "kind": "material", "index": index, "name": m.name,
            "base_color": m.base_color, "metallic": m.metallic, "roughness": m.roughness,
            "emissive": m.emissive, "double_sided": m.double_sided
        })
    }));
    records.extend(model.instances.iter().map(|i| {
        json!({
            "id": i.id, "kind": "instance", "primitive": i.primitive,
            "triangles": model.primitives[i.primitive].triangle_count()
        })
    }));
    let matches = records.len();
    let page = records
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let returned = page.len();
    json!({
        "format": model.report.format,
        "vertices": model.report.vertices,
        "triangles": model.report.triangles,
        "primitives": model.primitives.len(),
        "materials": model.materials.len(),
        "textures": model.textures.len(),
        "records": page, "matches": matches, "returned": returned,
        "offset": offset, "limit": limit, "has_more": offset.saturating_add(returned) < matches,
        "bounds": bounds.map(|(min, max)| json!({"min": min, "max": max})),
        "unsupported": model.report.unsupported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_cube() -> (Value, String) {
        // Minimal OBJ cube embedded in a v10 document.
        let obj =
            "v -1 -1 -1\nv 1 -1 -1\nv 1 1 -1\nv -1 1 -1\nv -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\n\
                   f 1 3 2\nf 1 4 3\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\nf 2 3 7\nf 2 7 6\n\
                   f 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";
        let digest = crate::resource::sha256(obj.as_bytes());
        let doc = json!({
            "version": 10,
            "models": {digest.clone(): {
                "format": "obj",
                "storage": {"kind": "embedded",
                    "data": base64::engine::general_purpose::STANDARD.encode(obj)}
            }}
        });
        (doc, digest)
    }

    #[test]
    fn colors_parse_and_reject() {
        assert_eq!(parse_color("#fff").unwrap(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(parse_color("none").unwrap()[3], 0.0);
        assert!(parse_color("red").is_err());
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("#gggggg").is_err());
    }

    #[test]
    fn target_only_pans_the_automatic_camera() {
        let (raw, digest) = doc_with_cube();
        let model = load_model(&raw, Path::new("."), &digest).unwrap();
        let base =
            json!({"id":"n","kind":"scene3d","x":0,"y":0,"width":200,"height":200,"model":digest});
        let (before, _) = scene_for(base.as_object().unwrap(), Some(&model)).unwrap();
        let mut panned = base;
        panned["camera"] = json!({"target": [3.0, 4.0, 5.0]});
        let (after, _) = scene_for(panned.as_object().unwrap(), Some(&model)).unwrap();
        assert_eq!(after.camera.target, [3.0, 4.0, 5.0]);
        let before_offset = math::sub(before.camera.eye, before.camera.target);
        let after_offset = math::sub(after.camera.eye, after.camera.target);
        for i in 0..3 {
            assert!((before_offset[i] - after_offset[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn validates_models_and_nodes() {
        let (mut raw, digest) = doc_with_cube();
        validate_models(&raw, 10).unwrap();
        let node =
            json!({"id":"n","kind":"scene3d","x":0,"y":0,"width":200,"height":200,"model":digest});
        validate_node(node.as_object().unwrap(), &raw).unwrap();
        let mut bad = node.clone();
        bad["model"] = json!("sha256:missing");
        assert!(validate_node(bad.as_object().unwrap(), &raw).is_err());
        let mut huge = node.clone();
        huge["width"] = json!(100000);
        assert!(validate_node(huge.as_object().unwrap(), &raw).is_err());
        let mut light = node.clone();
        light["lights"] = json!([{"kind":"laser"}]);
        assert!(validate_node(light.as_object().unwrap(), &raw).is_err());
        raw["models"][&digest]["format"] = json!("fbx");
        assert!(validate_models(&raw, 10).is_err());
    }

    #[test]
    fn renders_a_cube_deterministically_and_caches() {
        let (raw, digest) = doc_with_cube();
        let node = json!({"id":"n","kind":"scene3d","x":0,"y":0,"width":64,"height":64,"model":digest,
                          "background":"#202020"});
        let dir = std::env::temp_dir();
        let a = render_png(&raw, &dir, node.as_object().unwrap()).unwrap();
        let b = render_png(&raw, &dir, node.as_object().unwrap()).unwrap();
        assert_eq!(a, b);
        let img = image::load_from_memory(&a).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (64, 64));
        // The cube is lit, so the center differs from the flat background.
        assert_ne!(img.get_pixel(32, 32).0[..3], [0x20, 0x20, 0x20]);
        assert_eq!(img.get_pixel(0, 0).0, [0x20, 0x20, 0x20, 255]);
    }
}

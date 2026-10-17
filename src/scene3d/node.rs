//! The `scene3d` document node: a rectangle whose pixels come from rendering
//! an embedded or external model with the deterministic CPU renderer.
//!
//! Format version 10 adds the `scene3d` node kind and the top-level `models`
//! map (digest -> record). Everything else is unchanged from version 9.

use super::model::{AlphaMode, Instance, Material, Model, Primitive, LIMITS};
use super::render::{self, Environment, Light, Options, Projection, Scene};
use super::{gltf, math, obj};
use anyhow::{bail, ensure, Context, Result};
use base64::Engine as _;
use serde_json::{json, Map, Value};
use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub const VERSION: u64 = 10;
pub const BINDINGS_VERSION: u64 = 11;
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
                if let Some(resources) = storage.get("resources") {
                    let resources = resources.as_object().with_context(|| {
                        format!("[invalid-input] model {digest} resources must be an object")
                    })?;
                    ensure!(
                        resources.len() <= 1024,
                        "[limit-exceeded] model {digest} has more than 1024 sidecar resources"
                    );
                    let mut encoded_bytes = 0usize;
                    for (path, resource) in resources {
                        crate::resource::safe_relative_path(Path::new(path))?;
                        let resource = resource.as_object().with_context(|| {
                            format!(
                                "[invalid-input] model {digest} resource {path} must be an object"
                            )
                        })?;
                        let data =
                            resource
                                .get("data")
                                .and_then(Value::as_str)
                                .with_context(|| {
                                    format!(
                                    "[invalid-input] model {digest} resource {path} has no data"
                                )
                                })?;
                        let resource_digest = resource
                            .get("digest")
                            .and_then(Value::as_str)
                            .with_context(|| {
                                format!(
                                    "[invalid-input] model {digest} resource {path} has no digest"
                                )
                            })?;
                        ensure!(
                            resource_digest.len() == 71
                                && resource_digest.starts_with("sha256:")
                                && resource_digest[7..].bytes().all(|b| b.is_ascii_hexdigit()),
                            "[invalid-input] model {digest} resource {path} has an invalid digest"
                        );
                        let byte_length = resource
                            .get("byte_length")
                            .and_then(Value::as_u64)
                            .with_context(|| {
                                format!("[invalid-input] model {digest} resource {path} has no byte_length")
                            })?;
                        ensure!(
                            byte_length <= crate::resource::MAX_RESOURCE_BYTES as u64,
                            "[limit-exceeded] model {digest} resource {path} is too large"
                        );
                        encoded_bytes = encoded_bytes
                            .checked_add(data.len() / 4 * 3)
                            .context("model resource size overflow")?;
                    }
                    ensure!(
                        encoded_bytes <= crate::resource::MAX_RESOURCE_BYTES,
                        "[limit-exceeded] model {digest} sidecars exceed the resource byte limit"
                    );
                }
            }
            Some("external") => {
                let path = storage
                    .get("path")
                    .and_then(Value::as_str)
                    .with_context(|| format!("[invalid-input] model {digest} has no path"))?;
                crate::resource::safe_relative_path(Path::new(path))?;
                if let Some(resources) = storage.get("resources") {
                    let resources = resources.as_object().with_context(|| {
                        format!("[invalid-input] model {digest} resources must be an object")
                    })?;
                    ensure!(
                        resources.len() <= 1024,
                        "[limit-exceeded] model {digest} has more than 1024 sidecar resources"
                    );
                    for (path, resource) in resources {
                        crate::resource::safe_relative_path(Path::new(path))?;
                        let resource = resource.as_object().with_context(|| {
                            format!(
                                "[invalid-input] model {digest} resource {path} must be an object"
                            )
                        })?;
                        let sidecar_digest = resource
                            .get("digest")
                            .and_then(Value::as_str)
                            .with_context(|| {
                                format!(
                                    "[invalid-input] model {digest} resource {path} has no digest"
                                )
                            })?;
                        ensure!(
                            sidecar_digest.len() == 71
                                && sidecar_digest.starts_with("sha256:")
                                && sidecar_digest[7..].bytes().all(|b| b.is_ascii_hexdigit()),
                            "[invalid-input] model {digest} resource {path} has an invalid digest"
                        );
                        ensure!(
                            resource
                                .get("byte_length")
                                .and_then(Value::as_u64)
                                .is_some(),
                            "[invalid-input] model {digest} resource {path} has no byte_length"
                        );
                    }
                }
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
    let v11 = raw.get("version").and_then(Value::as_u64).unwrap_or(0) >= BINDINGS_VERSION;
    validate_ground_plane(node, v11).with_context(ctx)?;
    let model_source = node.get("model").and_then(Value::as_str);
    let form_source = v11
        .then(|| node.get("product_form").and_then(Value::as_str))
        .flatten();
    let dieline_source = v11
        .then(|| node.get("dieline").and_then(Value::as_str))
        .flatten();
    ensure!(
        usize::from(model_source.is_some())
            + usize::from(form_source.is_some())
            + usize::from(dieline_source.is_some())
            == 1,
        "[invalid-input] {} needs exactly one model digest, product_form ID, or dieline ID",
        ctx()
    );
    if let Some(digest) = model_source {
        ensure!(
            raw.get("models").and_then(|m| m.get(digest)).is_some(),
            "[missing-resource] {} refers to model {digest}, which is not in `models`",
            ctx()
        );
    } else if let Some(form) = form_source {
        ensure!(
            raw.get("product_forms")
                .and_then(|forms| forms.get(form))
                .is_some(),
            "[missing-resource] {} refers to missing product form `{form}`",
            ctx()
        );
    } else if let Some(dieline) = dieline_source {
        ensure!(
            raw.get("dielines")
                .and_then(|dielines| dielines.get(dieline))
                .is_some(),
            "[missing-resource] {} refers to missing dieline `{dieline}`",
            ctx()
        );
    }
    scene_for(node, None).with_context(ctx)?;
    validate_material_overrides(node).with_context(ctx)?;
    Ok(())
}

fn validate_ground_plane(node: &Map<String, Value>, v11: bool) -> Result<()> {
    let Some(value) = node.get("ground_plane") else {
        return Ok(());
    };
    ensure!(
        v11,
        "[unsupported-capability] explicit ground planes require document format v11"
    );
    let ground = value
        .as_object()
        .context("[invalid-input] ground_plane must be an object")?;
    ensure!(
        ground.keys().all(|key| matches!(
            key.as_str(),
            "enabled" | "color" | "size_multiplier" | "offset" | "mode" | "opacity"
        )),
        "[invalid-input] ground_plane accepts only enabled, color, size_multiplier, offset, mode and opacity"
    );
    ensure!(
        ground.get("enabled").and_then(Value::as_bool).is_some(),
        "[invalid-input] ground_plane.enabled must be a boolean"
    );
    if let Some(color) = ground.get("color") {
        let parsed = parse_color(
            color
                .as_str()
                .context("[invalid-input] ground_plane.color must be a color string")?,
        )?;
        ensure!(
            parsed[3] == 1.0,
            "[invalid-input] ground_plane.color must be opaque (#rgb or #rrggbb)"
        );
    }
    optional(ground, "size_multiplier", 2.5, 1.0, 4.0)?;
    optional(ground, "offset", 0.0, 0.0, 0.25)?;
    if let Some(mode) = ground.get("mode") {
        ensure!(
            matches!(mode.as_str(), Some("opaque" | "shadow-catcher")),
            "[invalid-input] ground_plane.mode must be opaque or shadow-catcher"
        );
    }
    optional(ground, "opacity", 0.65, 0.0, 1.0)?;
    Ok(())
}

fn add_ground_plane(model: &mut Model, node: &Map<String, Value>) -> Result<()> {
    let Some(ground) = node.get("ground_plane").and_then(Value::as_object) else {
        return Ok(());
    };
    if ground.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Ok(());
    }
    let Some((min, max)) = model.bounds() else {
        return Ok(());
    };
    ensure!(
        model.report.vertices <= LIMITS.max_vertices.saturating_sub(4)
            && model.report.triangles <= LIMITS.max_triangles.saturating_sub(2)
            && model.primitives.len() < LIMITS.max_primitives
            && model.instances.len() < LIMITS.max_instances
            && model.materials.len() < LIMITS.max_materials,
        "[limit-exceeded] adding a ground plane would exceed the 3D model resource limits"
    );
    let diagonal = math::length(math::sub(max, min)).max(1e-3);
    let size = diagonal * optional(ground, "size_multiplier", 2.5, 1.0, 4.0)?;
    let offset = diagonal * optional(ground, "offset", 0.0, 0.0, 0.25)? + diagonal * 1e-5;
    let y = min[1] - offset;
    let center_x = (min[0] + max[0]) * 0.5;
    let center_z = (min[2] + max[2]) * 0.5;
    let half = size * 0.5;
    let color = parse_color(
        ground
            .get("color")
            .and_then(Value::as_str)
            .unwrap_or("#d8d5d0"),
    )?;
    let catcher = ground.get("mode").and_then(Value::as_str) == Some("shadow-catcher");
    let opacity = optional(ground, "opacity", 0.65, 0.0, 1.0)?;
    let linear = |component: f64| {
        if component <= 0.04045 {
            component / 12.92
        } else {
            ((component + 0.055) / 1.055).powf(2.4)
        }
    };
    let material = model.materials.len();
    model.materials.push(Material {
        name: if catcher {
            "ground-shadow-catcher"
        } else {
            "ground-plane"
        }
        .into(),
        base_color: [
            linear(color[0]) as f32,
            linear(color[1]) as f32,
            linear(color[2]) as f32,
            if catcher {
                opacity as f32
            } else {
                color[3] as f32
            },
        ],
        metallic: 0.0,
        roughness: 1.0,
        double_sided: true,
        alpha_mode: if catcher {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        },
        ..Material::default()
    });
    let primitive = model.primitives.len();
    model.primitives.push(Primitive {
        name: if catcher {
            "ground-shadow-catcher"
        } else {
            "ground-plane"
        }
        .into(),
        positions: vec![
            [(center_x - half) as f32, y as f32, (center_z + half) as f32],
            [(center_x + half) as f32, y as f32, (center_z + half) as f32],
            [(center_x + half) as f32, y as f32, (center_z - half) as f32],
            [(center_x - half) as f32, y as f32, (center_z - half) as f32],
        ],
        normals: vec![[0.0, 1.0, 0.0]; 4],
        uvs: vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]],
        indices: vec![0, 1, 2, 0, 2, 3],
        material: Some(material),
    });
    model.instances.push(Instance {
        id: "ground-plane".into(),
        primitive,
        transform: math::IDENTITY,
    });
    model.report.vertices += 4;
    model.report.triangles += 2;
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
    super::bindings::validate(raw, version)?;
    Ok(())
}

/// `migrate --target 10`: version 9 plus the (initially empty) `models` map.
pub fn migrate(raw: Value) -> Result<Value> {
    if raw.get("version").and_then(Value::as_u64) == Some(VERSION) {
        crate::scene::validate(&raw)?;
        return Ok(raw);
    }
    ensure!(raw.get("version").and_then(Value::as_u64).unwrap_or(0) < VERSION,
        "[unsupported-version] migrate-to-10 cannot downgrade a newer document; use an explicit downgrade target");
    ensure!(
        raw.get("models").is_none(),
        "[unsupported-capability] an existing \"models\" extension conflicts with version 10; rename the extension before migration"
    );
    let mut raw = crate::spreads::migrate(raw)?;
    raw["version"] = json!(VERSION);
    crate::scene::validate(&raw)?;
    Ok(raw)
}

/// Explicitly migrate a supported document to v11. Existing v10 content is
/// preserved; the v11 catalogs start empty and are populated by later edits.
pub fn migrate_to_v11(raw: Value) -> Result<Value> {
    if raw.get("version").and_then(Value::as_u64) == Some(BINDINGS_VERSION) {
        crate::scene::validate(&raw)?;
        return Ok(raw);
    }
    let mut raw = migrate(raw)?;
    ensure!(
        raw.get("version").and_then(Value::as_u64) == Some(VERSION),
        "[unsupported-capability] migration to v11 requires a v10-compatible document"
    );
    ensure!(raw.get("product_forms").is_none(),
        "[unsupported-capability] existing `product_forms` extension conflicts with v11; rename it before migration");
    ensure!(raw.get("dielines").is_none(),
        "[unsupported-capability] existing `dielines` extension conflicts with v11; rename it before migration");
    raw["product_forms"] = json!({});
    raw["dielines"] = json!({});
    raw["version"] = json!(BINDINGS_VERSION);
    crate::scene::validate(&raw)?;
    Ok(raw)
}

/// Downgrade v11 to v10 only when no v11-only geometry or binding data would
/// be lost. Empty extension containers are removed as migration scaffolding.
pub fn to_v10(mut raw: Value) -> Result<Value> {
    let version = raw.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version <= VERSION {
        return Ok(raw);
    }
    ensure!(
        version == BINDINGS_VERSION,
        "[unsupported-version] cannot downgrade document version {version} to 10"
    );
    for (catalog, label) in [("product_forms", "product form"), ("dielines", "dieline")] {
        if let Some((id, _)) = raw
            .get(catalog)
            .and_then(Value::as_object)
            .and_then(|map| map.iter().next())
        {
            bail!("[unsupported-capability] version 10 cannot represent {label} `{id}`; remove it first");
        }
    }
    let mut nodes = Vec::new();
    collect(&raw, &mut nodes);
    for node in &nodes {
        if node.get("product_form").is_some() {
            bail!("[unsupported-capability] version 10 cannot represent product-form scene3d node {}; remove it first",
                node.get("id").and_then(Value::as_str).unwrap_or("?"));
        }
        if node
            .get("surface_graphics")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
        {
            bail!("[unsupported-capability] version 10 cannot represent surface graphics on scene3d node {}; remove them first",
                node.get("id").and_then(Value::as_str).unwrap_or("?"));
        }
    }
    if let Some(models) = raw.get("models").and_then(Value::as_object) {
        for (digest, record) in models {
            if record
                .get("uv")
                .and_then(Value::as_object)
                .is_some_and(|uv| uv.values().any(|overlay| overlay.get("islands").is_some()))
            {
                bail!("[unsupported-capability] version 10 cannot represent editable UV islands on model {digest}; remove them first");
            }
        }
    }
    raw.as_object_mut()
        .context("document must be an object")?
        .remove("product_forms");
    raw.as_object_mut().unwrap().remove("dielines");
    clear_empty_surface_graphics(&mut raw);
    raw["version"] = json!(VERSION);
    crate::scene::validate(&raw)?;
    Ok(raw)
}

fn clear_empty_surface_graphics(raw: &mut Value) {
    fn walk(node: &mut Value) {
        if let Some(object) = node.as_object_mut() {
            if object.get("kind").and_then(Value::as_str) == Some("scene3d")
                && object
                    .get("surface_graphics")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
            {
                object.remove("surface_graphics");
            }
            if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
                for child in children {
                    walk(child);
                }
            }
        }
    }
    fn layers(container: &mut Value) {
        if let Some(layers) = container.get_mut("layers").and_then(Value::as_array_mut) {
            for layer in layers {
                if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                    for node in nodes {
                        walk(node);
                    }
                }
            }
        }
    }
    if let Some(pages) = raw.get_mut("pages").and_then(Value::as_array_mut) {
        for page in pages {
            layers(page);
        }
    }
    if let Some(parents) = raw
        .get_mut("layout")
        .and_then(|l| l.get_mut("parents"))
        .and_then(Value::as_array_mut)
    {
        for parent in parents {
            layers(parent);
        }
    }
}

/// Bring a version-10 document down to 9. Refused, naming the node or model,
/// while the document uses 3D; other versions pass through untouched.
pub fn to_v9(mut raw: Value) -> Result<Value> {
    if raw.get("version").and_then(Value::as_u64) == Some(BINDINGS_VERSION) {
        raw = to_v10(raw)?;
    }
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
    let (output_width, output_height) = output_pixel_dimensions(node)?;
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
            width: output_width,
            height: output_height,
            supersample: supersample as u32,
        },
    ))
}

/// Exact output raster dimensions used by the scene3d reference renderer.
/// Preflight uses this same calculation rather than assuming document units
/// are pixels, so its resolution estimate matches the rendered PNG dimensions.
pub(crate) fn output_pixel_dimensions(node: &Map<String, Value>) -> Result<(u32, u32)> {
    let width = number(node, "width")?;
    let height = number(node, "height")?;
    let ratio = optional(node, "pixel_ratio", 1.0, 0.1, 8.0)?;
    let px = |value: f64| ((value * ratio).round() as u32).max(1);
    Ok((px(width), px(height)))
}

/// Effective output PPI along each artwork axis after the node's containing
/// affine placement has been applied. Rotation does not reduce resolution;
/// scale and shear do.
pub(crate) fn effective_ppi_for_placement(
    output_width_px: u32,
    output_height_px: u32,
    placed_width: f64,
    placed_height: f64,
    scale_x: f64,
    scale_y: f64,
    units_per_inch: f64,
) -> Option<(f64, f64)> {
    let width = placed_width * scale_x;
    let height = placed_height * scale_y;
    (width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0
        && units_per_inch.is_finite()
        && units_per_inch > 0.0)
        .then(|| {
            (
                f64::from(output_width_px) / width * units_per_inch,
                f64::from(output_height_px) / height * units_per_inch,
            )
        })
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
#[derive(Clone)]
struct CachedPng {
    bytes: Arc<Vec<u8>>,
    renderer: &'static str,
    consumer: Option<(String, String)>,
    sources: Vec<(String, Option<String>)>,
}
type PngCache = Mutex<Vec<(String, CachedPng)>>;
const PNG_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
static PNG_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static PNG_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
static PNG_CACHE_EVICTIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
pub(crate) static PNG_CACHE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn model_cache() -> &'static ModelCache {
    static C: OnceLock<ModelCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

fn png_cache() -> &'static PngCache {
    static C: OnceLock<PngCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

/// Hash every scene3d-node field consumed by model appearance, scene setup,
/// camera/lens setup, and output sizing. Keep this list paired with
/// `scene_for`, `lens_for`, `apply_material_overrides`, and `add_ground_plane`.
fn png_cache_base_key(node: &Map<String, Value>, source_key: &str) -> Result<String> {
    let mut spec = Map::new();
    for key in [
        "model",
        "product_form",
        "dieline",
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
        "ground_plane",
        "material_overrides",
        "surface_graphics",
    ] {
        if let Some(value) = node.get(key) {
            spec.insert(key.into(), value.clone());
        }
    }
    spec.insert("model_source".into(), json!(source_key));
    Ok(crate::resource::sha256(
        serde_json::to_string(&Value::Object(spec))?.as_bytes(),
    ))
}

fn png_cache_key(
    node: &Map<String, Value>,
    source_key: &str,
    resolved_graphics: &str,
    renderer_requested: &str,
) -> Result<String> {
    let base = png_cache_base_key(node, source_key)?;
    Ok(crate::resource::sha256(
        format!("{base}:{resolved_graphics}:{renderer_requested}").as_bytes(),
    ))
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

fn remember_png(
    key: String,
    png: Vec<u8>,
    renderer: &'static str,
    consumer: Option<(String, String)>,
    sources: Vec<(String, Option<String>)>,
) {
    if png.len() > PNG_CACHE_MAX_BYTES {
        return;
    }
    if let Ok(mut cache) = png_cache().lock() {
        insert_png(
            &mut cache,
            key,
            CachedPng {
                bytes: Arc::new(png),
                renderer,
                consumer,
                sources,
            },
            PNG_CACHE_MAX_BYTES,
        );
    }
}

fn cached_png(cache: &PngCache, key: &str) -> Option<CachedPng> {
    cache
        .lock()
        .ok()?
        .iter()
        .find(|(cached_key, _)| cached_key == key)
        .map(|(_, cached)| cached.clone())
}

/// Evict cached renders that consume a changed page/node, or belong to that
/// scene3d node. The cache is only an optimization; document bytes are never
/// changed. A page selector invalidates all bindings from that page, while a
/// node selector invalidates bindings to that exact node.
pub fn evict_png_cache_source(page: &str, node: Option<&str>) -> Value {
    let Ok(mut cache) = png_cache().lock() else {
        return json!({"evicted_entries":0,"evicted_bytes":0,"warning":"cache lock is poisoned"});
    };
    let (evicted_entries, evicted_bytes) = evict_png_cache_entries(&mut cache, page, node);
    json!({"evicted_entries":evicted_entries,"evicted_bytes":evicted_bytes})
}

fn evict_png_cache_entries(
    cache: &mut Vec<(String, CachedPng)>,
    page: &str,
    node: Option<&str>,
) -> (usize, usize) {
    let matches = |reference: &(String, Option<String>)| {
        reference.0 == page
            && match node {
                Some(id) => reference.1.as_deref() == Some(id),
                None => reference.1.is_none(),
            }
    };
    let mut evicted_entries = 0usize;
    let mut evicted_bytes = 0usize;
    cache.retain(|(_, entry)| {
        let consumer_matches = node.is_some_and(|id| {
            entry
                .consumer
                .as_ref()
                .is_some_and(|(owner_page, owner_node)| owner_page == page && owner_node == id)
        });
        let source_matches = entry.sources.iter().any(matches);
        if consumer_matches || source_matches {
            evicted_entries += 1;
            evicted_bytes = evicted_bytes.saturating_add(entry.bytes.len());
            false
        } else {
            true
        }
    });
    (evicted_entries, evicted_bytes)
}

fn binding_cache_sources(node: &Map<String, Value>) -> Vec<(String, Option<String>)> {
    let mut sources = std::collections::BTreeSet::new();
    if let Some(graphics) = node.get("surface_graphics").and_then(Value::as_array) {
        for graphic in graphics {
            if let Some(source) = graphic.get("source") {
                if let Some(page) = source.get("page").and_then(Value::as_str) {
                    let source_node = source.get("node").and_then(Value::as_str);
                    sources.insert((page.to_owned(), source_node.map(str::to_owned)));
                }
            }
        }
    }
    sources.into_iter().collect()
}

/// Invalidate cached render results after a successful shared document
/// transaction. Page-linked artwork depends on the complete page; node-linked
/// artwork is evicted only when its node or an ancestor subtree changed.
pub(crate) fn invalidate_png_cache_for_document_change(before: &Value, after: &Value) {
    let Ok(mut cache) = png_cache().lock() else {
        return;
    };
    invalidate_png_cache_entries_for_document_change(&mut cache, before, after);
}

fn invalidate_png_cache_entries_for_document_change(
    cache: &mut Vec<(String, CachedPng)>,
    before: &Value,
    after: &Value,
) {
    if before.get("version").and_then(Value::as_u64) != Some(BINDINGS_VERSION)
        || after.get("version").and_then(Value::as_u64) != Some(BINDINGS_VERSION)
    {
        return;
    }
    fn pages(document: &Value) -> std::collections::BTreeMap<String, Value> {
        document
            .get("pages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|page| {
                page.get("id")
                    .and_then(Value::as_str)
                    .map(|id| (id.to_owned(), page.clone()))
            })
            .collect()
    }
    fn collect_nodes(node: &Value, nodes: &mut std::collections::BTreeMap<String, Value>) {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            nodes.insert(id.to_owned(), node.clone());
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                collect_nodes(child, nodes);
            }
        }
    }
    fn page_nodes(page: Option<&Value>) -> std::collections::BTreeMap<String, Value> {
        let mut nodes = std::collections::BTreeMap::new();
        if let Some(layers) = page
            .and_then(|page| page.get("layers"))
            .and_then(Value::as_array)
        {
            for layer in layers {
                if let Some(items) = layer.get("nodes").and_then(Value::as_array) {
                    for node in items {
                        collect_nodes(node, &mut nodes);
                    }
                }
            }
        }
        nodes
    }
    fn add_subtree_ids(node: &Value, ids: &mut std::collections::BTreeSet<String>) {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            ids.insert(id.to_owned());
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                add_subtree_ids(child, ids);
            }
        }
    }
    let mut changed_pages = std::collections::BTreeSet::new();
    let mut changed_node_refs = std::collections::BTreeSet::new();
    fn changed_catalog_entries(
        before: &Value,
        after: &Value,
        catalog: &str,
    ) -> std::collections::BTreeSet<String> {
        let old = before.get(catalog).and_then(Value::as_object);
        let new = after.get(catalog).and_then(Value::as_object);
        old.into_iter()
            .flat_map(|map| map.keys())
            .chain(new.into_iter().flat_map(|map| map.keys()))
            .filter(|id| old.and_then(|map| map.get(*id)) != new.and_then(|map| map.get(*id)))
            .cloned()
            .collect()
    }
    fn add_catalog_consumers(
        document: &Value,
        field: &str,
        changed: &std::collections::BTreeSet<String>,
        output: &mut std::collections::BTreeSet<(String, String)>,
    ) {
        if changed.is_empty() {
            return;
        }
        for page in document
            .get("pages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(page_id) = page.get("id").and_then(Value::as_str) else {
                continue;
            };
            for (id, node) in page_nodes(Some(page)) {
                if node.get("kind").and_then(Value::as_str) == Some("scene3d")
                    && node
                        .get(field)
                        .and_then(Value::as_str)
                        .is_some_and(|reference| changed.contains(reference))
                {
                    output.insert((page_id.to_owned(), id));
                }
            }
        }
    }
    for (catalog, field) in [
        ("models", "model"),
        ("product_forms", "product_form"),
        ("dielines", "dieline"),
    ] {
        let changed = changed_catalog_entries(before, after, catalog);
        add_catalog_consumers(before, field, &changed, &mut changed_node_refs);
        add_catalog_consumers(after, field, &changed, &mut changed_node_refs);
    }
    let before_pages = pages(before);
    let after_pages = pages(after);
    let page_ids: std::collections::BTreeSet<_> = before_pages
        .keys()
        .chain(after_pages.keys())
        .cloned()
        .collect();
    for page_id in page_ids {
        let old_page = before_pages.get(&page_id);
        let new_page = after_pages.get(&page_id);
        if old_page == new_page {
            continue;
        }
        // Any change to the page's non-node state affects page bindings.
        let mut old_shell = old_page.cloned().unwrap_or(Value::Null);
        let mut new_shell = new_page.cloned().unwrap_or(Value::Null);
        if let Some(object) = old_shell.as_object_mut() {
            object.remove("layers");
        }
        if let Some(object) = new_shell.as_object_mut() {
            object.remove("layers");
        }
        if old_shell != new_shell {
            changed_pages.insert(page_id.clone());
        }
        let old_layers = old_page
            .and_then(|page| page.get("layers"))
            .and_then(Value::as_array);
        let new_layers = new_page
            .and_then(|page| page.get("layers"))
            .and_then(Value::as_array);
        let layer_count = old_layers
            .map_or(0, Vec::len)
            .max(new_layers.map_or(0, Vec::len));
        let mut changed_nodes = std::collections::BTreeSet::new();
        for index in 0..layer_count {
            let old_layer = old_layers.and_then(|layers| layers.get(index));
            let new_layer = new_layers.and_then(|layers| layers.get(index));
            if old_layer.and_then(|layer| layer.get("visible"))
                != new_layer.and_then(|layer| layer.get("visible"))
                || old_layer.and_then(|layer| layer.get("locked"))
                    != new_layer.and_then(|layer| layer.get("locked"))
            {
                for node in page_nodes(Some(&json!({"layers":[old_layer,new_layer]}))).values() {
                    add_subtree_ids(node, &mut changed_nodes);
                }
            }
        }
        let old_nodes = page_nodes(old_page);
        let new_nodes = page_nodes(new_page);
        let node_ids: std::collections::BTreeSet<_> =
            old_nodes.keys().chain(new_nodes.keys()).cloned().collect();
        for id in node_ids {
            if old_nodes.get(&id) != new_nodes.get(&id) {
                if let Some(node) = old_nodes.get(&id) {
                    add_subtree_ids(node, &mut changed_nodes);
                }
                if let Some(node) = new_nodes.get(&id) {
                    add_subtree_ids(node, &mut changed_nodes);
                }
            }
        }
        for id in changed_nodes {
            changed_node_refs.insert((page_id.clone(), id));
        }
        // A change to visible page artwork affects page-source bindings even
        // when the page's canvas and metadata are byte-identical.
        if old_layers != new_layers {
            changed_pages.insert(page_id);
        }
    }
    if before["color_management"]["roles"]["working"]
        != after["color_management"]["roles"]["working"]
    {
        // A color-role edit can change every linked source pixel while page
        // and node records stay byte-identical. Seed all source selectors in
        // the affected document and their binding owners into the same
        // transitive invalidation walk.
        for document in [before, after] {
            let mut source_pages = std::collections::BTreeSet::new();
            let mut source_nodes = std::collections::BTreeSet::new();
            for page in document
                .get("pages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(page_id) = page.get("id").and_then(Value::as_str) else {
                    continue;
                };
                source_pages.insert(page_id.to_owned());
                source_nodes.extend(
                    page_nodes(Some(page))
                        .into_iter()
                        .filter(|(_, node)| {
                            node.get("kind").and_then(Value::as_str) != Some("scene3d")
                        })
                        .map(|(id, _)| (page_id.to_owned(), id)),
                );
            }
            changed_node_refs.extend(cache_binding_consumers(
                document,
                &source_pages,
                &source_nodes,
            ));
            changed_pages.extend(source_pages);
            changed_node_refs.extend(source_nodes);
        }
    }
    evict_png_cache_dependencies(cache, &[before, after], &changed_pages, &changed_node_refs);
}

fn evict_png_cache_dependencies(
    cache: &mut Vec<(String, CachedPng)>,
    documents: &[&Value],
    changed_pages: &std::collections::BTreeSet<String>,
    changed_nodes: &std::collections::BTreeSet<(String, String)>,
) {
    let mut impacted = changed_nodes.clone();
    let mut removed = std::collections::BTreeSet::<String>::new();
    loop {
        let previous_impacted = impacted.len();
        for document in documents {
            for consumer in cache_binding_consumers(document, changed_pages, &impacted) {
                impacted.insert(consumer);
            }
        }

        for (key, entry) in cache.iter() {
            let consumer_hit = entry
                .consumer
                .as_ref()
                .is_some_and(|owner| impacted.contains(owner));
            let source_hit = entry.sources.iter().any(|(page, node)| match node {
                Some(node) => impacted.contains(&(page.clone(), node.clone())),
                None => changed_pages.contains(page),
            });
            if consumer_hit || source_hit {
                removed.insert(key.clone());
                if let Some(owner) = &entry.consumer {
                    impacted.insert(owner.clone());
                }
            }
        }
        if previous_impacted == impacted.len() {
            break;
        }
    }
    let mut evicted = 0u64;
    cache.retain(|(key, _entry)| {
        if removed.contains(key) {
            evicted += 1;
            false
        } else {
            true
        }
    });
    if evicted > 0 {
        PNG_CACHE_EVICTIONS.fetch_add(evicted, Ordering::Relaxed);
    }
}

fn cache_binding_consumers(
    document: &Value,
    changed_pages: &std::collections::BTreeSet<String>,
    changed_nodes: &std::collections::BTreeSet<(String, String)>,
) -> std::collections::BTreeSet<(String, String)> {
    fn source_matches(
        source: &Value,
        changed_pages: &std::collections::BTreeSet<String>,
        changed_nodes: &std::collections::BTreeSet<(String, String)>,
    ) -> bool {
        let Some(page) = source.get("page").and_then(Value::as_str) else {
            return false;
        };
        match source.get("kind").and_then(Value::as_str) {
            Some("page") => changed_pages.contains(page),
            Some("node") => source
                .get("node")
                .and_then(Value::as_str)
                .is_some_and(|node| changed_nodes.contains(&(page.to_owned(), node.to_owned()))),
            _ => false,
        }
    }
    fn visit(
        node: &Value,
        page: &str,
        changed_pages: &std::collections::BTreeSet<String>,
        changed_nodes: &std::collections::BTreeSet<(String, String)>,
        consumers: &mut std::collections::BTreeSet<(String, String)>,
        owners: &mut std::collections::BTreeMap<(String, String), String>,
    ) {
        if node.get("kind").and_then(Value::as_str) == Some("scene3d") {
            if let Some(id) = node.get("id").and_then(Value::as_str) {
                if let Some(dieline) = node.get("dieline").and_then(Value::as_str) {
                    owners.insert((dieline.to_owned(), id.to_owned()), page.to_owned());
                }
                if node
                    .get("surface_graphics")
                    .and_then(Value::as_array)
                    .is_some_and(|graphics| {
                        graphics.iter().any(|graphic| {
                            graphic.get("source").is_some_and(|source| {
                                source_matches(source, changed_pages, changed_nodes)
                            })
                        })
                    })
                {
                    consumers.insert((page.to_owned(), id.to_owned()));
                }
            }
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                visit(child, page, changed_pages, changed_nodes, consumers, owners);
            }
        }
    }
    let mut consumers = std::collections::BTreeSet::new();
    let mut owners = std::collections::BTreeMap::new();
    for page in document
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(page_id) = page.get("id").and_then(Value::as_str) else {
            continue;
        };
        for node in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|layer| layer.get("nodes").and_then(Value::as_array))
            .flatten()
        {
            visit(
                node,
                page_id,
                changed_pages,
                changed_nodes,
                &mut consumers,
                &mut owners,
            );
        }
    }
    for dieline in document
        .get("dielines")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|dielines| dielines.iter())
    {
        let (dieline_id, dieline) = dieline;
        let Some(owner) = dieline.get("scene3d_node").and_then(Value::as_str) else {
            continue;
        };
        let Some(owner_page) = owners.get(&(dieline_id.to_owned(), owner.to_owned())) else {
            continue;
        };
        if dieline
            .get("panels")
            .and_then(Value::as_array)
            .is_some_and(|panels| {
                panels.iter().any(|panel| {
                    panel
                        .get("artwork")
                        .is_some_and(|source| source_matches(source, changed_pages, changed_nodes))
                })
            })
        {
            consumers.insert((owner_page.clone(), owner.to_owned()));
        }
    }
    consumers
}

fn insert_png(cache: &mut Vec<(String, CachedPng)>, key: String, png: CachedPng, limit: usize) {
    if png.bytes.len() > limit {
        return;
    }
    cache.retain(|(old_key, _)| old_key != &key);
    cache.push((key, png));
    let mut bytes = cache.iter().map(|(_, png)| png.bytes.len()).sum::<usize>();
    while bytes > limit {
        if cache.is_empty() {
            break;
        }
        let (_, removed) = cache.remove(0);
        bytes = bytes.saturating_sub(removed.bytes.len());
        PNG_CACHE_EVICTIONS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Inspect the disposable in-process PNG render cache.
pub fn png_cache_status() -> Value {
    match png_cache().lock() {
        Ok(cache) => json!({
            "entries": cache.len(),
            "bytes": cache.iter().map(|(_, png)| png.bytes.len()).sum::<usize>(),
            "max_bytes": PNG_CACHE_MAX_BYTES,
            "eviction": "fifo",
            "scope": "process",
            "hits": PNG_CACHE_HITS.load(Ordering::Relaxed),
            "misses": PNG_CACHE_MISSES.load(Ordering::Relaxed),
            "evictions": PNG_CACHE_EVICTIONS.load(Ordering::Relaxed),
        }),
        Err(_) => json!({
            "entries": 0,
            "bytes": 0,
            "max_bytes": PNG_CACHE_MAX_BYTES,
            "eviction": "fifo",
            "scope": "process",
            "hits": PNG_CACHE_HITS.load(Ordering::Relaxed),
            "misses": PNG_CACHE_MISSES.load(Ordering::Relaxed),
            "evictions": PNG_CACHE_EVICTIONS.load(Ordering::Relaxed),
            "warning": "cache lock is poisoned",
        }),
    }
}

/// Clear disposable in-process PNG render results without affecting documents.
pub fn clear_png_cache() -> Value {
    match png_cache().lock() {
        Ok(mut cache) => {
            let entries = cache.len();
            let bytes = cache.iter().map(|(_, png)| png.bytes.len()).sum::<usize>();
            cache.clear();
            let hits = PNG_CACHE_HITS.swap(0, Ordering::Relaxed);
            let misses = PNG_CACHE_MISSES.swap(0, Ordering::Relaxed);
            let evictions = PNG_CACHE_EVICTIONS.swap(0, Ordering::Relaxed);
            json!({ "cleared_entries": entries, "cleared_bytes": bytes, "cleared_hits":hits, "cleared_misses":misses, "cleared_evictions":evictions })
        }
        Err(_) => {
            json!({
                "cleared_entries":0,
                "cleared_bytes":0,
                "cleared_hits":PNG_CACHE_HITS.load(Ordering::Relaxed),
                "cleared_misses":PNG_CACHE_MISSES.load(Ordering::Relaxed),
                "cleared_evictions":PNG_CACHE_EVICTIONS.load(Ordering::Relaxed),
                "warning":"cache lock is poisoned"
            })
        }
    }
}

/// Cache key: the model digest plus its UV overlay, so editing an overlay
/// never serves a stale model or render.
fn model_key(raw: &Value, digest: &str) -> String {
    let record = &raw["models"][digest];
    let variant = json!({
        "resources": record["storage"].get("resources"),
        "uv": record.get("uv"),
    });
    format!(
        "{digest}+{}",
        crate::resource::sha256(variant.to_string().as_bytes())
    )
}

/// Read an external model and every sidecar actually consumed by its importer,
/// returning self-contained embedded storage. All paths remain rooted at the
/// document directory and the aggregate resource budget is enforced.
pub fn collect_external_model(raw: &Value, document_dir: &Path, digest: &str) -> Result<Value> {
    use std::cell::{Cell, RefCell};
    let record = raw["models"]
        .get(digest)
        .and_then(Value::as_object)
        .with_context(|| format!("[missing-resource] model {digest} is not in `models`"))?;
    let storage = record["storage"]
        .as_object()
        .context("[invalid-input] model storage is missing")?;
    if storage.get("kind").and_then(Value::as_str) == Some("embedded") {
        return Ok(Value::Object(storage.clone()));
    }
    ensure!(
        storage.get("kind").and_then(Value::as_str) == Some("external"),
        "[unsupported-capability] model storage must be embedded or external"
    );
    let path = Path::new(storage["path"].as_str().unwrap_or(""));
    let document = document_dir.join("document.pen");
    let bytes = crate::resource::read_external_offline_limited(
        &document,
        path,
        digest,
        LIMITS.max_bytes,
    )
    .with_context(|| format!("[missing-resource] collect model source {}", path.display()))?;
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let expected_resources = storage.get("resources").and_then(Value::as_object);
    let resources = RefCell::new(std::collections::BTreeMap::<String, Value>::new());
    let total = Cell::new(bytes.len());
    if let Some(expected_resources) = expected_resources {
        for (uri, expected) in expected_resources {
            crate::resource::safe_relative_path(Path::new(uri))?;
            let relative = base.join(uri);
            crate::resource::safe_relative_path(&relative)?;
            let full = document_dir.join(relative);
            let root = document_dir
                .canonicalize()
                .context("[missing-resource] document directory")?;
            let canonical = full.canonicalize().with_context(|| {
                format!(
                    "[missing-resource] collect model sidecar {}",
                    full.display()
                )
            })?;
            ensure!(
                canonical.starts_with(&root),
                "[unsafe-path] model resource `{uri}` resolves outside the document root"
            );
            let sidecar = std::fs::read(&canonical)?;
            let expected_digest = expected["digest"]
                .as_str()
                .context("[malformed-resource] model resource manifest has no digest")?;
            crate::resource::verify(&sidecar, expected_digest)?;
            ensure!(
                expected.get("byte_length").and_then(Value::as_u64) == Some(sidecar.len() as u64),
                "[hash-mismatch] model resource `{uri}` byte length changed"
            );
            let next = total
                .get()
                .checked_add(sidecar.len())
                .context("model collection size overflow")?;
            ensure!(
                next <= crate::resource::MAX_RESOURCE_BYTES,
                "[limit-exceeded] collected model resources exceed the resource byte limit"
            );
            total.set(next);
            resources.borrow_mut().insert(
                uri.clone(),
                json!({
                    "digest": expected_digest,
                    "byte_length": sidecar.len(),
                    "data": base64::engine::general_purpose::STANDARD.encode(&sidecar),
                }),
            );
        }
    }
    let resolver = |uri: &str| -> Result<Vec<u8>> {
        ensure!(
            !uri.contains(':') && !uri.contains('?'),
            "[unsafe-path] model resource `{uri}` must be a relative file path"
        );
        let decoded = uri.replace("%20", " ");
        crate::resource::safe_relative_path(Path::new(&decoded))?;
        let relative = base.join(&decoded);
        crate::resource::safe_relative_path(&relative)?;
        let full = document_dir.join(&relative);
        let root = document_dir
            .canonicalize()
            .context("[missing-resource] document directory")?;
        let canonical = full
            .canonicalize()
            .with_context(|| format!("[missing-resource] {}", full.display()))?;
        ensure!(
            canonical.starts_with(&root),
            "[unsafe-path] model resource `{uri}` resolves outside the document root"
        );
        let sidecar = std::fs::read(&canonical).with_context(|| {
            format!(
                "[missing-resource] collect model sidecar {}",
                canonical.display()
            )
        })?;
        if resources.borrow().contains_key(&decoded) {
            return Ok(sidecar);
        }
        if let Some(expected_resources) = expected_resources {
            let expected = expected_resources.get(&decoded).with_context(|| {
                format!("[hash-mismatch] model resource `{decoded}` is not in the import manifest")
            })?;
            crate::resource::verify(
                &sidecar,
                expected["digest"]
                    .as_str()
                    .context("[malformed-resource] model resource manifest has no digest")?,
            )?;
            ensure!(
                expected.get("byte_length").and_then(Value::as_u64) == Some(sidecar.len() as u64),
                "[hash-mismatch] model resource `{decoded}` byte length changed"
            );
        }
        let next = total
            .get()
            .checked_add(sidecar.len())
            .context("model collection size overflow")?;
        ensure!(
            next <= crate::resource::MAX_RESOURCE_BYTES,
            "[limit-exceeded] collected model resources exceed the resource byte limit"
        );
        total.set(next);
        let resource_digest = crate::resource::sha256(&sidecar);
        resources.borrow_mut().insert(
            decoded,
            json!({
                "digest": resource_digest,
                "byte_length": sidecar.len(),
                "data": base64::engine::general_purpose::STANDARD.encode(&sidecar),
            }),
        );
        Ok(sidecar)
    };
    match record["format"].as_str() {
        Some("glb") | Some("gltf") => {
            gltf::import(&bytes, &resolver)?;
        }
        Some("obj") => {
            obj::import(&bytes, &resolver)?;
        }
        other => bail!("[unsupported-capability] model format {other:?} is unsupported"),
    }
    let resources = resources.into_inner();
    let mut embedded = json!({
        "kind": "embedded",
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
    });
    if !resources.is_empty() {
        embedded["resources"] = serde_json::to_value(resources)?;
    }
    Ok(embedded)
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
    let mut embedded_resources: Option<&Map<String, Value>> = None;
    let mut external_resources: Option<&Map<String, Value>> = None;
    let (bytes, base): (Vec<u8>, Option<std::path::PathBuf>) = match storage["kind"].as_str() {
        Some("embedded") => {
            embedded_resources = storage.get("resources").and_then(Value::as_object);
            (
                base64::engine::general_purpose::STANDARD
                    .decode(storage["data"].as_str().unwrap_or(""))
                    .context("[malformed-resource] embedded model is not valid base64")?,
                None,
            )
        }
        Some("external") => {
            let path = Path::new(storage["path"].as_str().unwrap_or(""));
            external_resources = storage.get("resources").and_then(Value::as_object);
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
        ensure!(
            !uri.contains(':') && !uri.contains('?'),
            "[unsafe-path] model resource `{uri}` must be a relative file path"
        );
        let decoded = uri.replace("%20", " ");
        crate::resource::safe_relative_path(Path::new(&decoded))?;
        if let Some(resources) = embedded_resources {
            let resource = resources.get(&decoded).with_context(|| {
                format!("[missing-resource] embedded model resource `{decoded}` is missing")
            })?;
            let data = resource["data"].as_str().with_context(|| {
                format!("[malformed-resource] embedded model resource `{decoded}` has no data")
            })?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .with_context(|| {
                    format!("[malformed-resource] model resource `{decoded}` is not valid base64")
                })?;
            if let Some(digest) = resource.get("digest").and_then(Value::as_str) {
                crate::resource::verify(&bytes, digest)?;
            }
            ensure!(
                resource.get("byte_length").and_then(Value::as_u64) == Some(bytes.len() as u64),
                "[malformed-resource] model resource `{decoded}` byte length does not match"
            );
            return Ok(bytes);
        }
        let Some(base) = &base else {
            bail!("[unsafe-path] embedded models cannot reference external file `{uri}`");
        };
        let relative = base.join(&decoded);
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
        let bytes = std::fs::read(&canonical)?;
        if let Some(resources) = external_resources {
            let resource = resources.get(&decoded).with_context(|| {
                format!("[hash-mismatch] model resource `{decoded}` is not in the import manifest")
            })?;
            let expected = resource["digest"]
                .as_str()
                .context("[malformed-resource] external model resource manifest has no digest")?;
            crate::resource::verify(&bytes, expected)?;
            ensure!(
                resource.get("byte_length").and_then(Value::as_u64) == Some(bytes.len() as u64),
                "[hash-mismatch] model resource `{decoded}` byte length changed"
            );
        }
        Ok(bytes)
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

pub fn load_form(raw: &Value, form_id: &str) -> Result<Arc<Model>> {
    let form = raw
        .get("product_forms")
        .and_then(|forms| forms.get(form_id))
        .with_context(|| format!("[missing-resource] product form `{form_id}` is missing"))?;
    let source_key = format!(
        "form:{}",
        crate::resource::sha256(&serde_json::to_vec(form)?)
    );
    if let Ok(cache) = model_cache().lock() {
        if let Some((_, model)) = cache.iter().find(|(key, _)| key == &source_key) {
            return Ok(Arc::clone(model));
        }
    }
    let mut model = super::form::generate(form)?;
    if let Some(uv) = form.get("uv") {
        super::uv::validate(uv).with_context(|| format!("product form {form_id}"))?;
        super::uv::apply(&mut model, uv)?;
    }
    let model = Arc::new(model);
    remember(model_cache(), source_key, Arc::clone(&model), 2);
    Ok(model)
}

fn load_dieline(raw: &Value, dieline_id: &str) -> Result<Arc<Model>> {
    let dieline = super::pack::dieline_from_document(raw, dieline_id)?;
    let source_key = format!(
        "dieline:{}",
        crate::resource::sha256(&serde_json::to_vec(&dieline)?)
    );
    if let Ok(cache) = model_cache().lock() {
        if let Some((_, model)) = cache.iter().find(|(key, _)| key == &source_key) {
            return Ok(Arc::clone(model));
        }
    }
    let model = Arc::new(super::pack::generate(&dieline)?);
    remember(model_cache(), source_key, Arc::clone(&model), 2);
    Ok(model)
}

pub(super) fn panel_artwork_transform(panel: &Value) -> ([f64; 2], [f64; 2], f64) {
    let transform = panel.get("artwork_transform");
    let pair = |field: &str, default: [f64; 2]| {
        transform
            .and_then(|value| value.get(field))
            .and_then(Value::as_array)
            .filter(|values| values.len() == 2)
            .map(|values| {
                [
                    values[0].as_f64().unwrap_or(default[0]),
                    values[1].as_f64().unwrap_or(default[1]),
                ]
            })
            .unwrap_or(default)
    };
    let scale = pair("scale", [1.0, 1.0]);
    let offset = pair("offset", [0.5, 0.5]);
    let rotate = transform
        .and_then(|value| value.get("rotate"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    (scale, offset, rotate)
}

/// Compose the flat panel placement with that panel's UV rectangle for folded
/// surface artwork. Flat export uses panel-local coordinates; the folded
/// binding must map the same artwork into the corresponding face UV region.
pub(super) fn panel_artwork_uv_transform(panel: &Value) -> ([f64; 2], [f64; 2], f64) {
    let (scale, offset, rotate) = panel_artwork_transform(panel);
    let uv = panel
        .get("uv_rect")
        .and_then(Value::as_array)
        .filter(|values| values.len() == 4)
        .map(|values| {
            [
                values[0].as_f64().unwrap_or(0.0),
                values[1].as_f64().unwrap_or(0.0),
                values[2].as_f64().unwrap_or(1.0),
                values[3].as_f64().unwrap_or(1.0),
            ]
        })
        .unwrap_or([0.0, 0.0, 1.0, 1.0]);
    (
        [scale[0] * uv[2], scale[1] * uv[3]],
        [uv[0] + offset[0] * uv[2], uv[1] + offset[1] * uv[3]],
        rotate,
    )
}

fn node_with_dieline_artwork(
    raw: &Value,
    dieline_id: &str,
    node: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let dieline = raw["dielines"]
        .get(dieline_id)
        .with_context(|| format!("[missing-resource] dieline `{dieline_id}` is missing"))?;
    let panels = dieline
        .get("panels")
        .and_then(Value::as_array)
        .context("[invalid-input] dieline panels must be an array")?;
    let mut graphics = Vec::new();
    for panel in panels {
        let Some(source) = panel.get("artwork") else {
            continue;
        };
        let panel_id = panel.get("id").and_then(Value::as_str).unwrap_or("panel");
        let slot = panel.get("face_slot").and_then(Value::as_str).unwrap_or("");
        let (scale, offset, rotate) = panel_artwork_uv_transform(panel);
        graphics.push(json!({
            "id": format!("dieline-{panel_id}"),
            "source": source,
            "slot": slot,
            "mode": "decal",
            "projection": "uv",
            "fit": "stretch",
            "wrap": "clamp",
            "blend": "normal",
            "opacity": 1.0,
            "scale": scale,
            "offset": offset,
            "rotate": rotate
        }));
    }
    if let Some(existing) = node.get("surface_graphics").and_then(Value::as_array) {
        graphics.extend(existing.iter().cloned());
    }
    let mut result = node.clone();
    if !graphics.is_empty() {
        result.insert("surface_graphics".into(), Value::Array(graphics));
    }
    Ok(result)
}

/// Render a node to PNG bytes (straight alpha sRGB).
pub fn render_png(raw: &Value, document_dir: &Path, node: &Map<String, Value>) -> Result<Vec<u8>> {
    let page_id = node
        .get("id")
        .and_then(Value::as_str)
        .and_then(|node_id| unique_node_page(raw, node_id));
    render_png_in_page(raw, document_dir, page_id.as_deref(), node)
}

/// Find the containing page when a caller provides a document node but not its
/// page context. Same-page linked artwork must omit its consumer while the
/// source page is rasterized; otherwise it can feed its own previous preview
/// back into the next render.
fn unique_node_page(raw: &Value, node_id: &str) -> Option<String> {
    fn contains(nodes: &[Value], node_id: &str) -> bool {
        nodes.iter().any(|node| {
            node.get("id").and_then(Value::as_str) == Some(node_id)
                || node
                    .get("children")
                    .and_then(Value::as_array)
                    .is_some_and(|children| contains(children, node_id))
        })
    }

    let mut owners = raw
        .get("pages")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|page| {
            let page_id = page.get("id")?.as_str()?;
            let found = page
                .get("layers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|layer| {
                    layer
                        .get("nodes")
                        .and_then(Value::as_array)
                        .is_some_and(|nodes| contains(nodes, node_id))
                });
            found.then(|| page_id.to_owned())
        });
    let page_id = owners.next()?;
    owners.next().is_none().then_some(page_id)
}

/// Render with page context for same-page artwork bindings.
pub fn render_png_in_page(
    raw: &Value,
    document_dir: &Path,
    page_id: Option<&str>,
    node: &Map<String, Value>,
) -> Result<Vec<u8>> {
    render_png_report_in_page(raw, document_dir, page_id, node).map(|(png, _)| png)
}

/// Render a scene3d node and report the exact path and output used. The report
/// is suitable for previews and batch callers; PNG bytes remain normative.
pub fn render_png_report_in_page(
    raw: &Value,
    document_dir: &Path,
    page_id: Option<&str>,
    node: &Map<String, Value>,
) -> Result<(Vec<u8>, Value)> {
    render_png_report_with_island_selection(raw, document_dir, page_id, node, None)
}

/// Render a scene3d node with one editable UV island visibly highlighted.
/// This is a transient workspace preview and is deliberately not cached.
pub fn render_png_with_island_selection_in_page(
    raw: &Value,
    document_dir: &Path,
    page_id: Option<&str>,
    node: &Map<String, Value>,
    primitive: usize,
    triangles: Vec<usize>,
) -> Result<Vec<u8>> {
    render_png_report_with_island_selection(
        raw,
        document_dir,
        page_id,
        node,
        Some(vec![(primitive, triangles)]),
    )
    .map(|(png, _)| png)
}

/// Render a transient package selection highlight across one or more panel
/// primitives (for example, the segmented faces of a cylindrical label).
pub fn render_png_with_primitive_selections_in_page(
    raw: &Value,
    document_dir: &Path,
    page_id: Option<&str>,
    node: &Map<String, Value>,
    selections: Vec<(usize, Vec<usize>)>,
) -> Result<Vec<u8>> {
    ensure!(
        !selections.is_empty(),
        "[invalid-input] face highlight needs at least one primitive selection"
    );
    render_png_report_with_island_selection(raw, document_dir, page_id, node, Some(selections))
        .map(|(png, _)| png)
}

fn render_png_report_with_island_selection(
    raw: &Value,
    document_dir: &Path,
    page_id: Option<&str>,
    node: &Map<String, Value>,
    selection: Option<Vec<(usize, Vec<usize>)>>,
) -> Result<(Vec<u8>, Value)> {
    let highlighted = selection.is_some();
    validate_ground_plane(
        node,
        raw.get("version").and_then(Value::as_u64).unwrap_or(0) >= BINDINGS_VERSION,
    )?;
    if raw.get("version").and_then(Value::as_u64) == Some(11) {
        // Export uses the same complete-document dependency plan as v11
        // validation, so a direct library render cannot bypass cycle or
        // source-state checks that CLI/server preview already depends on.
        let _dependency_order = super::bindings::plan(raw)?;
    }
    let digest = node.get("model").and_then(Value::as_str);
    let form_id = node.get("product_form").and_then(Value::as_str);
    let dieline_id = node.get("dieline").and_then(Value::as_str);
    ensure!(
        usize::from(digest.is_some())
            + usize::from(form_id.is_some())
            + usize::from(dieline_id.is_some())
            == 1,
        "[invalid-input] scene3d node needs exactly one model digest, product_form ID, or dieline ID"
    );
    let source_key = if let Some(digest) = digest {
        model_key(raw, digest)
    } else if let Some(form_id) = form_id {
        let form = raw
            .get("product_forms")
            .and_then(|forms| forms.get(form_id))
            .with_context(|| format!("[missing-resource] product form `{form_id}` is missing"))?;
        crate::resource::sha256(&serde_json::to_vec(form)?)
    } else {
        let dieline_id = dieline_id.unwrap();
        let dieline = super::pack::dieline_from_document(raw, dieline_id)?;
        crate::resource::sha256(&serde_json::to_vec(&dieline)?)
    };
    // Everything declared on the node that affects pixels, plus the model
    // identity. Linked source pixels are added after their shared raster path
    // has resolved them below.
    let loaded = if let Some(digest) = digest {
        load_model(raw, document_dir, digest)?
    } else if let Some(form_id) = form_id {
        load_form(raw, form_id)?
    } else {
        load_dieline(raw, dieline_id.unwrap())?
    };
    let mut model = loaded.as_ref().clone();
    apply_material_overrides(&mut model, node)?;
    let render_node = if let Some(dieline_id) = dieline_id {
        node_with_dieline_artwork(raw, dieline_id, node)?
    } else {
        node.clone()
    };
    let resolved_graphics =
        super::bindings::apply_graphics(raw, document_dir, page_id, &render_node, &mut model)?;
    let renderer_requested = RENDERER.with(|renderer| match renderer.get() {
        RendererPreference::Reference => "reference",
        RendererPreference::Accelerated => "accelerated",
    });
    let key = png_cache_key(node, &source_key, &resolved_graphics, renderer_requested)?;
    if !highlighted {
        if let Some(cached) = cached_png(png_cache(), &key) {
            PNG_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            let png = cached.bytes.as_ref().clone();
            return Ok((
                png.clone(),
                render_report(
                    raw,
                    page_id,
                    node,
                    &png,
                    renderer_requested,
                    cached.renderer,
                    true,
                ),
            ));
        }
    }
    if !highlighted {
        PNG_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
    }
    let (scene, options) = scene_for(node, Some(&model))?;
    add_ground_plane(&mut model, node)?;
    if let Some(selections) = selection {
        for (primitive, triangles) in selections {
            append_island_highlight(&mut model, primitive, &triangles)?;
        }
    }
    let lens = lens_for(node, &scene)?;
    let reference = render::render_with_lens(&model, &scene, &options, lens.as_ref())?;
    let (image, renderer_used) = if RENDERER.get() == RendererPreference::Accelerated {
        match super::gpu::resolve(reference) {
            Ok((image, status)) => {
                if !RENDERER_REPORTED.replace(true) {
                    eprintln!(
                        "{}",
                        json!({
                            "renderer_requested": "accelerated",
                            "renderer_used": "accelerated",
                            "pipeline": "reference-raster+wgpu-resolve",
                            "adapter": status.adapter,
                            "backend": status.backend,
                        })
                    );
                }
                (image, "accelerated")
            }
            Err(error) => {
                if !RENDERER_REPORTED.replace(true) {
                    let status = super::gpu::availability();
                    eprintln!(
                        "{}",
                        json!({
                            "renderer_requested": "accelerated",
                            "renderer_used": "reference",
                            "fallback": error.to_string(),
                            "adapter": status.adapter,
                            "backend": status.backend,
                        })
                    );
                }
                (
                    render::render_with_lens(&model, &scene, &options, lens.as_ref())?,
                    "reference",
                )
            }
        }
    } else {
        (reference, "reference")
    };
    let buffer = image::RgbaImage::from_raw(image.width, image.height, image.rgba)
        .context("renderer produced a malformed image")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut out, image::ImageFormat::Png)
        .context("could not encode the 3D render")?;
    let png = out.into_inner();
    let consumer = node
        .get("id")
        .and_then(Value::as_str)
        .map(|id| (page_id.unwrap_or_default().to_owned(), id.to_owned()));
    if !highlighted {
        remember_png(
            key,
            png.clone(),
            renderer_used,
            consumer,
            binding_cache_sources(&render_node),
        );
    }
    let report = render_report(
        raw,
        page_id,
        node,
        &png,
        renderer_requested,
        renderer_used,
        false,
    );
    Ok((png, report))
}

fn append_island_highlight(
    model: &mut Model,
    primitive_index: usize,
    triangles: &[usize],
) -> Result<()> {
    ensure!(
        !triangles.is_empty(),
        "[missing-resource] selected UV island has no triangles"
    );
    let source = model
        .primitives
        .get(primitive_index)
        .with_context(|| format!("[missing-resource] primitive {primitive_index} does not exist"))?
        .clone();
    let instance_count = model
        .instances
        .iter()
        .filter(|instance| instance.primitive == primitive_index)
        .count();
    ensure!(
        instance_count > 0,
        "[missing-resource] selected UV primitive has no model instances"
    );
    let extra_triangles = triangles
        .len()
        .checked_mul(instance_count)
        .context("UV highlight triangle count overflow")?;
    ensure!(
        model
            .rendered_triangles()
            .checked_add(extra_triangles)
            .is_some_and(|count| count <= LIMITS.max_triangles),
        "[limit-exceeded] UV island highlight exceeds the {} rendered-triangle limit",
        LIMITS.max_triangles
    );
    let current_vertices = model
        .primitives
        .iter()
        .try_fold(0usize, |count, primitive| {
            count.checked_add(primitive.positions.len())
        })
        .context("model vertex count overflow")?;
    let extra_vertices = triangles
        .len()
        .checked_mul(3)
        .context("UV highlight vertex count overflow")?;
    ensure!(
        current_vertices
            .checked_add(extra_vertices)
            .is_some_and(|count| count <= LIMITS.max_vertices),
        "[limit-exceeded] UV island highlight exceeds the {} vertex limit",
        LIMITS.max_vertices
    );
    ensure!(
        model.primitives.len() < LIMITS.max_primitives
            && model.materials.len() < LIMITS.max_materials
            && model.instances.len().checked_add(instance_count).is_some_and(|count| count <= LIMITS.max_instances),
        "[limit-exceeded] UV island highlight exceeds model primitive, material, or instance limits"
    );
    let (min, max) = model.bounds().context("selected UV model has no bounds")?;
    let epsilon = ((0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(0.0_f64, f64::max)
        * 0.0005)
        .max(0.001);
    let mut highlight = Primitive {
        name: format!("{}-selected-uv-island", source.name),
        positions: Vec::with_capacity(extra_vertices),
        normals: Vec::with_capacity(extra_vertices),
        uvs: source
            .uvs
            .iter()
            .map(|_| Vec::with_capacity(extra_vertices))
            .collect(),
        indices: Vec::with_capacity(extra_vertices),
        material: Some(model.materials.len()),
    };
    for &triangle in triangles {
        ensure!(
            triangle < source.triangle_count(),
            "[invalid-input] selected UV island references missing triangle {triangle}"
        );
        for corner in 0..3 {
            let vertex = source.indices[triangle * 3 + corner] as usize;
            let position = source.positions[vertex];
            let normal = source
                .normals
                .get(vertex)
                .copied()
                .unwrap_or([0.0, 0.0, 1.0]);
            let length = (f64::from(normal[0]).powi(2)
                + f64::from(normal[1]).powi(2)
                + f64::from(normal[2]).powi(2))
            .sqrt()
            .max(1e-9);
            highlight.positions.push(std::array::from_fn(|axis| {
                (f64::from(position[axis]) + f64::from(normal[axis]) / length * epsilon) as f32
            }));
            highlight.normals.push(normal);
            for (set, values) in source.uvs.iter().zip(&mut highlight.uvs) {
                values.push(set[vertex]);
            }
            highlight.indices.push(highlight.indices.len() as u32);
        }
    }
    let material = Material {
        name: "pentool-uv-selection-highlight".into(),
        base_color: [0.04, 0.72, 1.0, 1.0],
        metallic: 0.0,
        roughness: 0.45,
        emissive: [0.01, 0.28, 0.48],
        alpha_mode: AlphaMode::Opaque,
        double_sided: true,
        ..Material::default()
    };
    let highlighted_index = model.primitives.len();
    model.materials.push(material);
    model.primitives.push(highlight);
    let instances: Vec<Instance> = model
        .instances
        .iter()
        .filter(|instance| instance.primitive == primitive_index)
        .map(|instance| Instance {
            id: format!("{}-uv-selection", instance.id),
            primitive: highlighted_index,
            transform: instance.transform,
        })
        .collect();
    model.instances.extend(instances);
    Ok(())
}

fn render_report(
    raw: &Value,
    page_id: Option<&str>,
    node: &Map<String, Value>,
    png: &[u8],
    renderer_requested: &str,
    renderer_used: &str,
    cache_hit: bool,
) -> Value {
    let (width_px, height_px) = if png.len() >= 24 && png.starts_with(b"\x89PNG\r\n\x1a\n") {
        (
            u32::from_be_bytes(png[16..20].try_into().unwrap_or([0; 4])),
            u32::from_be_bytes(png[20..24].try_into().unwrap_or([0; 4])),
        )
    } else {
        (0, 0)
    };
    let page = raw
        .get("pages")
        .and_then(Value::as_array)
        .and_then(|pages| {
            page_id
                .and_then(|id| pages.iter().find(|page| page["id"] == id))
                .or_else(|| pages.first())
        });
    let units_per_inch = page
        .and_then(|page| page["production"]["units_per_inch"].as_f64())
        .unwrap_or(72.0);
    let placed_width = node.get("width").and_then(Value::as_f64).unwrap_or(0.0);
    let placed_height = node.get("height").and_then(Value::as_f64).unwrap_or(0.0);
    let effective_ppi = effective_ppi_for_placement(
        width_px,
        height_px,
        placed_width,
        placed_height,
        1.0,
        1.0,
        units_per_inch,
    )
    .map_or(
        Value::Null,
        |(x, y)| json!({"x":x,"y":y,"units_per_inch":units_per_inch}),
    );
    json!({
        "format":"pentool-scene3d-render-report-v1",
        "output":{"format":"png","width_px":width_px,"height_px":height_px,"bytes":png.len()},
        "effective_ppi":effective_ppi,
        "ground_plane":node.get("ground_plane").cloned().unwrap_or_else(|| json!({"enabled":false})),
        "renderer_requested":renderer_requested,
        "renderer_used":renderer_used,
        "cache_hit":cache_hit,
    })
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

    #[test]
    fn context_free_render_resolves_only_a_unique_node_owner_page() {
        let raw = json!({
            "pages":[
                {"id":"art","layers":[{"nodes":[{"id":"logo"}]}]},
                {"id":"products","layers":[{"nodes":[{
                    "id":"mockup","children":[{"id":"nested-consumer"}]
                }]}]}
            ]
        });
        assert_eq!(
            unique_node_page(&raw, "nested-consumer").as_deref(),
            Some("products")
        );
        assert_eq!(unique_node_page(&raw, "missing"), None);

        let ambiguous = json!({
            "pages":[
                {"id":"one","layers":[{"nodes":[{"id":"duplicate"}]}]},
                {"id":"two","layers":[{"nodes":[{"id":"duplicate"}]}]}
            ]
        });
        assert_eq!(unique_node_page(&ambiguous, "duplicate"), None);
    }

    #[test]
    fn panel_artwork_transform_uses_one_normalized_default_and_explicit_mapping() {
        assert_eq!(
            panel_artwork_transform(&json!({"id":"default"})),
            ([1.0, 1.0], [0.5, 0.5], 0.0)
        );
        assert_eq!(
            panel_artwork_transform(&json!({
                "artwork_transform":{"scale":[0.5,0.75],"offset":[0.6,0.45],"rotate":17.0}
            })),
            ([0.5, 0.75], [0.6, 0.45], 17.0)
        );
    }

    #[test]
    fn folded_panel_binding_uses_the_shared_print_transform_values() {
        let raw = json!({
            "dielines":{"carton":{"panels":[{
                "id":"front",
                "face_slot":"body",
                "artwork":{"kind":"page","page":"art"},
                "artwork_transform":{"scale":[0.5,0.75],"offset":[0.6,0.45],"rotate":17.0}
            }]}}
        });
        let node = Map::new();
        let linked = node_with_dieline_artwork(&raw, "carton", &node).unwrap();
        let graphic = &linked["surface_graphics"][0];
        assert_eq!(graphic["scale"], json!([0.5, 0.75]));
        assert_eq!(graphic["offset"], json!([0.6, 0.45]));
        assert_eq!(graphic["rotate"], json!(17.0));
        assert_eq!(graphic["source"], json!({"kind":"page","page":"art"}));
    }

    #[test]
    fn folded_panel_binding_composes_artwork_placement_with_panel_uv_rect() {
        let raw = json!({
            "dielines":{"carton":{"panels":[{
                "id":"front",
                "face_slot":"body",
                "uv_rect":[0.2,0.25,0.4,0.5],
                "artwork":{"kind":"page","page":"art"},
                "artwork_transform":{"scale":[0.5,0.75],"offset":[0.6,0.45],"rotate":17.0}
            }]}}
        });
        let node = Map::new();
        let linked = node_with_dieline_artwork(&raw, "carton", &node).unwrap();
        let graphic = &linked["surface_graphics"][0];
        assert_eq!(graphic["scale"], json!([0.2, 0.375]));
        assert_eq!(graphic["offset"], json!([0.44, 0.475]));
        assert_eq!(graphic["rotate"], json!(17.0));
    }

    #[test]
    fn png_cache_key_tracks_every_scene_and_output_setting_but_not_page_placement() {
        let base = json!({
            "id": "mockup",
            "kind": "scene3d",
            "x": 20.0,
            "y": 30.0,
            "name": "Bottle",
            "opacity": 0.5,
            "width": 320.0,
            "height": 240.0,
            "model": "model-a",
            "pixel_ratio": 2.0,
            "camera": {"azimuth": 30.0, "projection": "perspective"},
            "physical_camera": {"focal_length_mm": 50.0, "sensor_mm": [36.0, 24.0], "aperture_f": 4.0},
            "lights": [{"kind": "directional", "direction": [0.0, -1.0, 0.0]}],
            "environment": {"sky": "#ffffff", "ground": "#222222", "intensity": 0.4},
            "background": "#fafafa",
            "exposure_ev": 0.5,
            "supersample": 2,
            "shadows": true,
            "shadow_map_size": 512,
            "ground_plane": {"enabled": true, "size_multiplier": 2.5},
            "material_overrides": {"body": {"roughness": 0.4}},
            "surface_graphics": [{"id": "label", "opacity": 1.0}],
        });
        let base_map = base.as_object().unwrap();
        let base_key =
            png_cache_key(base_map, "model-source-a", "linked-art-a", "reference").unwrap();

        for field in [
            "model",
            "product_form",
            "dieline",
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
            "ground_plane",
            "material_overrides",
            "surface_graphics",
        ] {
            let mut changed = base.clone();
            changed[field] = json!({"different": true});
            assert_ne!(
                png_cache_key(
                    changed.as_object().unwrap(),
                    "model-source-a",
                    "linked-art-a",
                    "reference"
                )
                .unwrap(),
                base_key,
                "changing `{field}` must change the cached appearance key"
            );
        }
        assert_ne!(
            png_cache_key(base_map, "model-source-b", "linked-art-a", "reference").unwrap(),
            base_key,
            "changing resolved model/form/dieline identity must change the key"
        );
        assert_ne!(
            png_cache_key(base_map, "model-source-a", "linked-art-b", "reference").unwrap(),
            base_key,
            "changing resolved linked artwork pixels must change the key"
        );
        assert_ne!(
            png_cache_key(base_map, "model-source-a", "linked-art-a", "accelerated").unwrap(),
            base_key,
            "changing requested renderer must change the key"
        );

        let mut moved_on_page = base;
        moved_on_page["x"] = json!(600.0);
        moved_on_page["y"] = json!(-90.0);
        moved_on_page["name"] = json!("Renamed bottle");
        moved_on_page["opacity"] = json!(0.25);
        moved_on_page["locked"] = json!(true);
        assert_eq!(
            png_cache_key(moved_on_page.as_object().unwrap(), "model-source-a", "linked-art-a", "reference").unwrap(),
            base_key,
            "page placement, display metadata, and compositor state do not affect the standalone PNG"
        );
    }

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
        let _cache_guard = PNG_CACHE_TEST_LOCK.blocking_lock();
        let (raw, digest) = doc_with_cube();
        let node = json!({"id":"n","kind":"scene3d","x":0,"y":0,"width":64,"height":64,"model":digest,
                          "background":"#202020"});
        let dir = std::env::temp_dir();
        let _cleared = clear_png_cache();
        let (reported_png, first_report) =
            render_png_report_in_page(&raw, &dir, None, node.as_object().unwrap()).unwrap();
        assert_eq!(first_report["format"], "pentool-scene3d-render-report-v1");
        assert_eq!(first_report["renderer_used"], "reference");
        assert_eq!(first_report["cache_hit"], false);
        assert!(first_report["output"]["width_px"].as_u64().unwrap() > 0);
        assert!(first_report["output"]["height_px"].as_u64().unwrap() > 0);
        let (_, cached_report) =
            render_png_report_in_page(&raw, &dir, None, node.as_object().unwrap()).unwrap();
        assert_eq!(cached_report["cache_hit"], true);
        assert_eq!(reported_png.len(), cached_report["output"]["bytes"]);
        let evicted = evict_png_cache_source("", Some("n"));
        assert_eq!(evicted["evicted_entries"], 1);
        let (_, after_targeted_eviction) =
            render_png_report_in_page(&raw, &dir, None, node.as_object().unwrap()).unwrap();
        assert_eq!(after_targeted_eviction["cache_hit"], false);
        let a = render_png(&raw, &dir, node.as_object().unwrap()).unwrap();
        let b = render_png(&raw, &dir, node.as_object().unwrap()).unwrap();
        assert_eq!(a, b);
        let cleared = clear_png_cache();
        assert!(cleared["cleared_hits"].as_u64().is_some());
        let after_clear = render_png(&raw, &dir, node.as_object().unwrap()).unwrap();
        assert_eq!(
            a, after_clear,
            "cache contents must not affect output semantics"
        );
        let img = image::load_from_memory(&a).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (64, 64));
        // The cube is lit, so the center differs from the flat background.
        assert_ne!(img.get_pixel(32, 32).0[..3], [0x20, 0x20, 0x20]);
        assert_eq!(img.get_pixel(0, 0).0, [0x20, 0x20, 0x20, 255]);
    }

    #[test]
    fn png_cache_evicts_oldest_entries_to_a_byte_budget() {
        let mut cache = Vec::new();
        insert_png(
            &mut cache,
            "old".into(),
            CachedPng {
                bytes: Arc::new(vec![0; 5]),
                renderer: "reference",
                consumer: None,
                sources: Vec::new(),
            },
            8,
        );
        insert_png(
            &mut cache,
            "new".into(),
            CachedPng {
                bytes: Arc::new(vec![1; 5]),
                renderer: "reference",
                consumer: None,
                sources: Vec::new(),
            },
            8,
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].0, "new");
        insert_png(
            &mut cache,
            "too-large".into(),
            CachedPng {
                bytes: Arc::new(vec![2; 9]),
                renderer: "reference",
                consumer: None,
                sources: Vec::new(),
            },
            8,
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].0, "new");
    }

    #[test]
    fn poisoned_png_cache_is_a_miss_not_a_render_error() {
        let cache: PngCache = Mutex::new(Vec::new());
        let poison = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = cache.lock().unwrap();
            panic!("simulate an interrupted cache update");
        }));
        assert!(poison.is_err());
        assert!(cached_png(&cache, "any-key").is_none());
        // The cache is optional: a poisoned lock cannot prevent the caller
        // from taking the regular render path and producing deterministic data.
    }

    #[test]
    fn png_cache_source_eviction_preserves_unrelated_renders() {
        let entry = |size, consumer, sources| CachedPng {
            bytes: Arc::new(vec![0; size]),
            renderer: "reference",
            consumer,
            sources,
        };
        let mut cache = vec![
            (
                "label-render".into(),
                entry(
                    5,
                    Some(("products".into(), "bottle".into())),
                    vec![("artwork".into(), Some("label".into()))],
                ),
            ),
            (
                "other-render".into(),
                entry(
                    7,
                    Some(("other".into(), "mockup".into())),
                    vec![("other-art".into(), None)],
                ),
            ),
        ];
        assert_eq!(
            evict_png_cache_entries(&mut cache, "artwork", Some("label")),
            (1, 5)
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].0, "other-render");
        assert_eq!(
            evict_png_cache_entries(&mut cache, "other", Some("mockup")),
            (1, 7)
        );
        assert!(cache.is_empty());
    }

    #[test]
    fn committed_source_changes_evict_only_page_and_node_consumers() {
        let page_source = "cache-invalidation-test-art";
        let node_source = "cache-invalidation-test-label";
        let entry = |source| CachedPng {
            bytes: Arc::new(vec![0; 4]),
            renderer: "reference",
            consumer: Some(("cache-invalidation-test-products".into(), "mockup".into())),
            sources: vec![source],
        };
        let mut cache = vec![
            (
                "cache-invalidation-test-page".into(),
                entry((page_source.into(), None)),
            ),
            (
                "cache-invalidation-test-node".into(),
                entry((page_source.into(), Some(node_source.into()))),
            ),
            (
                "cache-invalidation-test-unrelated".into(),
                CachedPng {
                    bytes: Arc::new(vec![0; 4]),
                    renderer: "reference",
                    consumer: Some((
                        "cache-invalidation-test-other-products".into(),
                        "other".into(),
                    )),
                    sources: vec![("cache-invalidation-test-other".into(), None)],
                },
            ),
        ];
        let before = json!({"version":11,"pages":[{"id":page_source,"canvas":{"width":32,"height":32},"layers":[{"id":"layer","nodes":[{"id":node_source,"kind":"rect","x":0,"y":0,"width":4,"height":4,"style":{"fill":{"fallback":"#fff"}}}]}]},{"id":"cache-invalidation-test-products","canvas":{"width":32,"height":32},"layers":[{"id":"products","nodes":[]}]}]});
        let mut after = before.clone();
        after["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] = json!("#000");
        invalidate_png_cache_entries_for_document_change(&mut cache, &before, &after);
        assert!(!cache
            .iter()
            .any(|(key, _)| key == "cache-invalidation-test-page"));
        assert!(!cache
            .iter()
            .any(|(key, _)| key == "cache-invalidation-test-node"));
        assert!(cache
            .iter()
            .any(|(key, _)| key == "cache-invalidation-test-unrelated"));
    }

    #[test]
    fn source_changes_evict_transitive_consumers_without_intermediate_cache_entries() {
        let mut cache = vec![(
            "transitive-consumer-c".into(),
            CachedPng {
                bytes: Arc::new(vec![0; 8]),
                renderer: "reference",
                consumer: Some(("products".into(), "c".into())),
                sources: vec![("products".into(), Some("b".into()))],
            },
        )];
        let before = json!({"version":11,"pages":[
            {"id":"art","canvas":{"width":32,"height":32},"layers":[{"id":"art","nodes":[{"id":"a","kind":"rect","x":0,"y":0,"width":4,"height":4,"style":{"fill":{"fallback":"#fff"}}}]}]},
            {"id":"products","canvas":{"width":32,"height":32},"layers":[{"id":"products","nodes":[
                {"kind":"scene3d","id":"b","surface_graphics":[{"id":"from-a","source":{"kind":"node","page":"art","node":"a"},"slot":"label","mode":"decal","projection":"uv"}]},
                {"kind":"scene3d","id":"c","surface_graphics":[{"id":"from-b","source":{"kind":"node","page":"products","node":"b"},"slot":"label","mode":"decal","projection":"uv"}]}
            ]}]}
        ]});
        let mut after = before.clone();
        after["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] = json!("#000");
        let mut changed_nodes = std::collections::BTreeSet::new();
        changed_nodes.insert(("art".into(), "a".into()));
        evict_png_cache_dependencies(
            &mut cache,
            &[&before, &after],
            &std::collections::BTreeSet::new(),
            &changed_nodes,
        );
        assert!(cache.is_empty(), "consumer C must be evicted through B");
    }

    #[test]
    fn product_form_catalog_edits_evict_their_mockup_cache() {
        let mut cache = vec![(
            "form-dependent-render".into(),
            CachedPng {
                bytes: Arc::new(vec![0; 9]),
                renderer: "reference",
                consumer: Some(("products".into(), "bottle".into())),
                sources: Vec::new(),
            },
        )];
        let before = json!({"version":11,"pages":[{"id":"products","canvas":{"width":32,"height":32},"layers":[{"id":"products","nodes":[{"kind":"scene3d","id":"bottle","product_form":"form"}]}]}],"product_forms":{"form":{"kind":"box","parameters":{"length_mm":10}}}});
        let mut after = before.clone();
        after["product_forms"]["form"]["parameters"]["length_mm"] = json!(11);
        invalidate_png_cache_entries_for_document_change(&mut cache, &before, &after);
        assert!(cache.is_empty());
    }

    #[test]
    fn color_role_edits_invalidate_linked_mockups_but_keep_unbound_renders() {
        let mut cache = vec![
            (
                "linked-color-render".into(),
                CachedPng {
                    bytes: Arc::new(vec![0; 5]),
                    renderer: "reference",
                    consumer: Some(("products".into(), "mockup".into())),
                    sources: vec![("artwork".into(), None)],
                },
            ),
            (
                "unbound-color-render".into(),
                CachedPng {
                    bytes: Arc::new(vec![0; 7]),
                    renderer: "reference",
                    consumer: Some(("products".into(), "standalone".into())),
                    sources: Vec::new(),
                },
            ),
        ];
        let before = json!({"version":11,"color_management":{"roles":{"working":{"destination":"srgb:linear","intent":"relative-colorimetric"}}},"pages":[
            {"id":"artwork","canvas":{"width":32,"height":32},"layers":[{"id":"artwork","nodes":[{"id":"label","kind":"rect","x":0,"y":0,"width":4,"height":4}]}]},
            {"id":"products","canvas":{"width":32,"height":32},"layers":[{"id":"products","nodes":[
                {"kind":"scene3d","id":"mockup","surface_graphics":[{"id":"label","source":{"kind":"page","page":"artwork"},"slot":"label","mode":"decal","projection":"uv"}]},
                {"kind":"scene3d","id":"standalone"}
            ]}]}
        ]});
        let mut after = before.clone();
        after["color_management"]["roles"]["working"]["intent"] = json!("perceptual");
        invalidate_png_cache_entries_for_document_change(&mut cache, &before, &after);
        assert!(!cache.iter().any(|(key, _)| key == "linked-color-render"));
        assert!(cache.iter().any(|(key, _)| key == "unbound-color-render"));

        let mut proof_only_cache = vec![(
            "proof-role-does-not-change-preview".into(),
            CachedPng {
                bytes: Arc::new(vec![0; 5]),
                renderer: "reference",
                consumer: Some(("products".into(), "mockup".into())),
                sources: vec![("artwork".into(), None)],
            },
        )];
        let mut proof_only = before.clone();
        proof_only["color_management"]["roles"]["proof"] =
            json!({"destination":"srgb","intent":"perceptual"});
        invalidate_png_cache_entries_for_document_change(
            &mut proof_only_cache,
            &before,
            &proof_only,
        );
        assert_eq!(proof_only_cache.len(), 1);
    }
}

//! `pentool scene3d` commands: import a model, place it, aim the camera, light
//! it, and inspect it. Edits go through the shared transaction path, so
//! `--dry-run` plans without writing and failures leave the file untouched.

use super::node;
use anyhow::{bail, ensure, Context, Result};
use base64::Engine as _;
use clap::Subcommand;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Subcommand)]
pub enum LightAction {
    /// Add a directional or point light.
    Add {
        /// `directional`, `point` or `spot`.
        #[arg(long)]
        kind: String,
        /// Spot cone half angle (degrees) where light reaches zero.
        #[arg(long)]
        outer_deg: Option<f64>,
        /// Spot cone half angle (degrees) at full intensity.
        #[arg(long)]
        inner_deg: Option<f64>,
        /// Direction the light travels, `x,y,z` (directional).
        #[arg(long, allow_hyphen_values = true)]
        direction: Option<String>,
        /// World position, `x,y,z` (point).
        #[arg(long, allow_hyphen_values = true)]
        position: Option<String>,
        #[arg(long, default_value = "#ffffff")]
        color: String,
        #[arg(long, default_value_t = 1.0)]
        intensity: f64,
    },
    /// Remove every light (the environment still lights the model).
    Clear,
    /// Restore the default lighting rig.
    Reset,
}

#[derive(Debug, Clone, Subcommand)]
pub enum UvAction {
    /// Report UV sets, bounds, coverage and degenerate triangles per primitive.
    Show,
    /// Project UVs onto a primitive as a re-applied overlay; the source model
    /// is never rewritten.
    Project {
        /// Primitive index, or `*` for every primitive.
        #[arg(long, default_value = "*")]
        primitive: String,
        /// `planar`, `box`, `cylindrical` or `spherical`.
        #[arg(long)]
        projection: String,
        /// Projection axis: x, y or z.
        #[arg(long)]
        axis: Option<String>,
        /// UV scale `u,v`.
        #[arg(long, allow_hyphen_values = true)]
        scale: Option<String>,
        /// UV offset `u,v`.
        #[arg(long, allow_hyphen_values = true)]
        offset: Option<String>,
        /// Rotation in degrees about the UV center.
        #[arg(long, allow_hyphen_values = true)]
        rotate: Option<f64>,
    },
    /// Remove the overlay for a primitive (or all of them).
    Clear {
        #[arg(long, default_value = "*")]
        primitive: String,
    },
    /// Write the UV wireframe as .svg or .png.
    Layout {
        #[arg(long, default_value_t = 0)]
        primitive: usize,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 1024)]
        size: u32,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum Scene3dAction {
    /// UV inspection, projection and layout export.
    Uv {
        id: String,
        #[command(subcommand)]
        action: UvAction,
    },
    /// Import a .glb, .gltf or .obj model and place it as a scene3d node.
    Add {
        id: String,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        page: Option<String>,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        x: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        y: f64,
        #[arg(long, default_value_t = 400.0)]
        width: f64,
        #[arg(long, default_value_t = 400.0)]
        height: f64,
        /// Keep the model as a document-relative file (needed for models with
        /// sidecar buffers or textures) instead of embedding it.
        #[arg(long)]
        external: bool,
        /// Override the format detected from the extension: glb, gltf, obj.
        #[arg(long)]
        format: Option<String>,
    },
    /// Aim the camera: orbit angles, projection, or an explicit eye/target.
    Camera {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        azimuth: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        elevation: Option<f64>,
        #[arg(long)]
        fov: Option<f64>,
        /// `perspective` or `orthographic`.
        #[arg(long)]
        projection: Option<String>,
        /// Orthographic view height in world units.
        #[arg(long)]
        height: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        eye: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        target: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        up: Option<String>,
        #[arg(long)]
        near: Option<f64>,
        #[arg(long)]
        far: Option<f64>,
        /// Drop every camera setting and frame the model automatically.
        #[arg(long)]
        reset: bool,
        /// Lens focal length in millimeters (derives the field of view and
        /// re-frames the model, so only perspective changes).
        #[arg(long)]
        focal_length: Option<f64>,
        /// Sensor size in millimeters as `WxH` (default 36x24).
        #[arg(long)]
        sensor: Option<String>,
        /// f-number; setting it turns depth of field on.
        #[arg(long)]
        aperture: Option<f64>,
        /// Distance to the sharp plane (default: eye to target).
        #[arg(long)]
        focus_distance: Option<f64>,
        /// Lens exposure compensation in EV.
        #[arg(long, allow_hyphen_values = true)]
        lens_exposure: Option<f64>,
        /// Depth-of-field lens samples (1..=64, default 16).
        #[arg(long)]
        lens_samples: Option<u32>,
        /// Remove the physical camera and return to `--fov`.
        #[arg(long)]
        no_physical: bool,
    },
    /// Edit the light rig.
    Light {
        id: String,
        #[command(subcommand)]
        action: LightAction,
    },
    /// Set the sky/ground gradient environment.
    Environment {
        id: String,
        #[arg(long)]
        sky: Option<String>,
        #[arg(long)]
        ground: Option<String>,
        #[arg(long)]
        intensity: Option<f64>,
    },
    /// Frame, background, exposure and quality settings.
    Set {
        id: String,
        #[arg(long)]
        background: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        exposure_ev: Option<f64>,
        #[arg(long)]
        supersample: Option<u32>,
        #[arg(long)]
        pixel_ratio: Option<f64>,
        /// Turn shadow maps on or off.
        #[arg(long)]
        shadows: Option<bool>,
        /// Shadow map edge in texels (64..=2048).
        #[arg(long)]
        shadow_map_size: Option<u32>,
        #[arg(long, allow_hyphen_values = true)]
        x: Option<f64>,
        #[arg(long, allow_hyphen_values = true)]
        y: Option<f64>,
        #[arg(long)]
        width: Option<f64>,
        #[arg(long)]
        height: Option<f64>,
    },
    /// Report the model (counts, bounds, instances, unsupported features).
    Inspect { id: String },
    /// List scene3d nodes and models.
    List,
}

fn read(input: &Path) -> Result<Value> {
    let bytes = std::fs::read(input)
        .with_context(|| format!("[missing-resource] could not read {}", input.display()))?;
    serde_json::from_slice(&bytes).context("[malformed-resource] invalid .pen document")
}

fn triple(text: &str, what: &str) -> Result<Value> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    ensure!(
        parts.len() == 3,
        "[invalid-input] {what} must be x,y,z (got `{text}`)"
    );
    let mut out = Vec::new();
    for p in parts {
        let v: f64 = p
            .parse()
            .with_context(|| format!("[invalid-input] {what}: `{p}` is not a number"))?;
        ensure!(v.is_finite(), "[invalid-input] {what} must be finite");
        out.push(json!(v));
    }
    Ok(Value::Array(out))
}

fn require_v10(raw: &Value) -> Result<()> {
    ensure!(
        node::is_v10(raw),
        "[unsupported-capability] scene3d needs a version 10 document; run `pentool migrate --target 10` first"
    );
    Ok(())
}

/// Locate a scene3d node anywhere in the pages and edit it in place.
fn find_node<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    for n in nodes.iter_mut() {
        if n.get("id").and_then(Value::as_str) == Some(id) {
            return Some(n);
        }
        if let Some(children) = n.get_mut("children").and_then(Value::as_array_mut) {
            if let Some(found) = find_node(children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn scene3d_node<'a>(raw: &'a mut Value, id: &str) -> Result<&'a mut Map<String, Value>> {
    let pages = raw
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .context("[invalid-input] document has no pages")?;
    for page in pages {
        let Some(layers) = page.get_mut("layers").and_then(Value::as_array_mut) else {
            continue;
        };
        for layer in layers {
            let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) else {
                continue;
            };
            if let Some(n) = find_node(nodes, id) {
                ensure!(
                    n.get("kind").and_then(Value::as_str) == Some("scene3d"),
                    "[invalid-input] node {id} is not a scene3d node"
                );
                return n
                    .as_object_mut()
                    .context("[invalid-input] node must be an object");
            }
        }
    }
    bail!("[missing-resource] no scene3d node `{id}`; list them with `pentool scene3d <doc> list`")
}

fn node_exists(raw: &Value, id: &str) -> bool {
    fn walk(nodes: &[Value], id: &str) -> bool {
        nodes.iter().any(|n| {
            n.get("id").and_then(Value::as_str) == Some(id)
                || n.get("children")
                    .and_then(Value::as_array)
                    .is_some_and(|c| walk(c, id))
        })
    }
    raw["pages"].as_array().into_iter().flatten().any(|p| {
        p["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|l| l["nodes"].as_array().is_some_and(|nodes| walk(nodes, id)))
    })
}

fn detect_format(path: &Path, forced: Option<&str>) -> Result<String> {
    let name = match forced {
        Some(f) => f.to_ascii_lowercase(),
        None => path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default(),
    };
    ensure!(
        matches!(name.as_str(), "glb" | "gltf" | "obj"),
        "[unsupported-capability] model format `{name}` is unsupported; use .glb, .gltf or .obj"
    );
    Ok(name)
}

fn add(
    raw: &mut Value,
    input: &Path,
    args: (
        &str,
        &Path,
        &str,
        Option<&str>,
        [f64; 4],
        bool,
        Option<&str>,
    ),
) -> Result<Value> {
    let (id, model_path, layer, page, [x, y, width, height], external, format) = args;
    require_v10(raw)?;
    ensure!(
        !node_exists(raw, id),
        "[invalid-input] a node with id `{id}` already exists"
    );
    let format = detect_format(model_path, format)?;
    let bytes = std::fs::read(model_path)
        .with_context(|| format!("[missing-resource] could not read {}", model_path.display()))?;
    ensure!(
        bytes.len() <= super::model::LIMITS.max_bytes,
        "[limit-exceeded] model is larger than {} bytes",
        super::model::LIMITS.max_bytes
    );
    let digest = crate::resource::sha256(&bytes);
    let doc_dir = input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let storage = if external {
        let canonical_root = doc_dir
            .canonicalize()
            .context("[missing-resource] document directory")?;
        let canonical = model_path
            .canonicalize()
            .with_context(|| format!("[missing-resource] {}", model_path.display()))?;
        let relative = canonical.strip_prefix(&canonical_root).map_err(|_| {
            anyhow::anyhow!(
                "[unsafe-path] --external models must live inside the document directory ({})",
                canonical_root.display()
            )
        })?;
        let text = relative.to_string_lossy().replace('\\', "/");
        crate::resource::safe_relative_path(Path::new(&text))?;
        json!({"kind": "external", "path": text})
    } else {
        json!({"kind": "embedded", "data": base64::engine::general_purpose::STANDARD.encode(&bytes)})
    };
    // Prove the model imports (and report what it contains) before any write.
    let record = json!({
        "format": format,
        "storage": storage,
        "bytes": bytes.len(),
        "name": model_path.file_name().and_then(|n| n.to_str()).unwrap_or("model"),
    });
    raw["models"] = raw.get("models").cloned().unwrap_or_else(|| json!({}));
    raw["models"][&digest] = record;
    // A model-only document must still validate; load it as the renderer will.
    let probe = json!({"models": raw["models"].clone()});
    let model = node::load_model(&probe, &doc_dir, &digest).with_context(|| {
        if external {
            "model import failed".to_string()
        } else {
            "model import failed (models with sidecar buffers or textures need --external)"
                .to_string()
        }
    })?;
    let report = node::inspect(&model);

    let pages = raw["pages"]
        .as_array_mut()
        .context("[invalid-input] document has no pages")?;
    let page_value = match page {
        Some(p) => pages
            .iter_mut()
            .find(|pg| pg["id"].as_str() == Some(p))
            .with_context(|| format!("[missing-resource] no page `{p}`"))?,
        None => pages
            .first_mut()
            .context("[invalid-input] document has no pages")?,
    };
    let layers = page_value["layers"]
        .as_array_mut()
        .context("[invalid-input] page has no layers")?;
    let target = layers
        .iter_mut()
        .find(|l| l["id"].as_str() == Some(layer))
        .with_context(|| format!("[missing-resource] no layer `{layer}`"))?;
    ensure!(
        target["locked"].as_bool() != Some(true),
        "[locked] layer `{layer}` is locked"
    );
    target["nodes"]
        .as_array_mut()
        .context("[invalid-input] layer has no nodes")?
        .push(json!({
            "id": id, "kind": "scene3d", "x": x, "y": y, "width": width, "height": height,
            "model": digest,
        }));
    Ok(json!({"id": id, "model": digest, "report": report}))
}

pub fn run(
    input: &Path,
    dry_run: bool,
    revision: Option<&str>,
    action: Scene3dAction,
) -> Result<()> {
    let mut raw = read(input)?;
    let doc_dir = input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let (label, mut result) = match action {
        Scene3dAction::List => {
            let mut nodes = Vec::new();
            for page in raw["pages"].as_array().into_iter().flatten() {
                for layer in page["layers"].as_array().into_iter().flatten() {
                    for n in layer["nodes"].as_array().into_iter().flatten() {
                        if n["kind"] == "scene3d" {
                            nodes.push(json!({
                                "id": n["id"], "page": page["id"], "layer": layer["id"],
                                "model": n["model"],
                                "x": n["x"], "y": n["y"], "width": n["width"], "height": n["height"],
                            }));
                        }
                    }
                }
            }
            println!(
                "{}",
                json!({"scene3d": nodes, "models": raw["models"].as_object().map_or(0, |m| m.len())})
            );
            return Ok(());
        }
        Scene3dAction::Uv { id, action } => {
            require_v10(&raw)?;
            let node = scene3d_node(&mut raw, &id)?;
            let digest = node["model"].as_str().unwrap_or_default().to_string();
            match action {
                UvAction::Show => {
                    let model = node::load_model(&raw, &doc_dir, &digest)?;
                    let mut report = super::uv::show(&model);
                    report["id"] = json!(id);
                    report["model"] = json!(digest);
                    println!("{report}");
                    return Ok(());
                }
                UvAction::Layout {
                    primitive,
                    out,
                    size,
                } => {
                    let model = node::load_model(&raw, &doc_dir, &digest)?;
                    let prim = model.primitives.get(primitive).with_context(|| {
                        format!(
                            "[missing-resource] primitive {primitive} does not exist (model has {})",
                            model.primitives.len()
                        )
                    })?;
                    let ext = out
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_ascii_lowercase);
                    let bytes = match ext.as_deref() {
                        Some("svg") => super::uv::layout_svg(prim, size)?.into_bytes(),
                        Some("png") => super::uv::layout_png(prim, size)?,
                        _ => bail!("[invalid-input] --out must end in .svg or .png"),
                    };
                    std::fs::write(&out, &bytes)
                        .with_context(|| format!("could not write {}", out.display()))?;
                    println!(
                        "{}",
                        json!({"id": id, "primitive": primitive, "out": out.display().to_string(),
                               "size": size, "bytes": bytes.len()})
                    );
                    return Ok(());
                }
                UvAction::Project {
                    primitive,
                    projection,
                    axis,
                    scale,
                    offset,
                    rotate,
                } => {
                    ensure!(
                        primitive == "*" || primitive.parse::<usize>().is_ok(),
                        "[invalid-input] --primitive must be an index or `*`"
                    );
                    let mut spec = json!({"projection": projection});
                    if let Some(a) = axis {
                        spec["axis"] = json!(a);
                    }
                    for (key, value) in [("scale", scale), ("offset", offset)] {
                        if let Some(v) = value {
                            let parts: Vec<&str> = v.split(',').map(str::trim).collect();
                            ensure!(
                                parts.len() == 2,
                                "[invalid-input] --{key} must be u,v (got `{v}`)"
                            );
                            let mut pair = Vec::new();
                            for p in parts {
                                pair.push(json!(p.parse::<f64>().with_context(|| format!(
                                    "[invalid-input] --{key}: `{p}` is not a number"
                                ))?));
                            }
                            spec[key] = Value::Array(pair);
                        }
                    }
                    if let Some(r) = rotate {
                        spec["rotate"] = json!(r);
                    }
                    super::uv::validate(&json!({ primitive.clone(): spec.clone() }))?;
                    let record = raw["models"][&digest]
                        .as_object_mut()
                        .context("[missing-resource] model record is missing")?;
                    let overlays = record.entry("uv").or_insert_with(|| json!({}));
                    overlays[&primitive] = spec;
                    // Prove the overlay applies (index range, vertex limits).
                    let model = node::load_model(&raw, &doc_dir, &digest)?;
                    let report = super::uv::show(&model);
                    ("scene3d-uv-project", json!({"id": id, "uv": report}))
                }
                UvAction::Clear { primitive } => {
                    let record = raw["models"][&digest]
                        .as_object_mut()
                        .context("[missing-resource] model record is missing")?;
                    if primitive == "*" {
                        record.remove("uv");
                    } else if let Some(map) = record.get_mut("uv").and_then(Value::as_object_mut) {
                        map.remove(&primitive);
                        if map.is_empty() {
                            record.remove("uv");
                        }
                    }
                    ("scene3d-uv-clear", json!({"id": id}))
                }
            }
        }
        Scene3dAction::Inspect { id } => {
            require_v10(&raw)?;
            let n = scene3d_node(&mut raw, &id)?.clone();
            let digest = n["model"].as_str().unwrap_or_default().to_string();
            let model = node::load_model(&raw, &doc_dir, &digest)?;
            let (scene, options) = node::scene_for(&n, Some(&model))?;
            let mut report = node::inspect(&model);
            report["id"] = json!(id);
            report["model"] = json!(digest);
            report["render"] = json!({
                "width": options.width, "height": options.height,
                "supersample": options.supersample, "lights": scene.lights.len(),
            });
            println!("{report}");
            return Ok(());
        }
        Scene3dAction::Add {
            id,
            model,
            layer,
            page,
            x,
            y,
            width,
            height,
            external,
            format,
        } => (
            "scene3d-add",
            add(
                &mut raw,
                input,
                (
                    &id,
                    &model,
                    &layer,
                    page.as_deref(),
                    [x, y, width, height],
                    external,
                    format.as_deref(),
                ),
            )?,
        ),
        Scene3dAction::Camera {
            id,
            azimuth,
            elevation,
            fov,
            projection,
            height,
            eye,
            target,
            up,
            near,
            far,
            reset,
            focal_length,
            sensor,
            aperture,
            focus_distance,
            lens_exposure,
            lens_samples,
            no_physical,
        } => {
            require_v10(&raw)?;
            let n = scene3d_node(&mut raw, &id)?;
            if reset {
                n.remove("camera");
            }
            let cam = n
                .entry("camera")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("[invalid-input] camera must be an object")?;
            for (key, value) in [
                ("azimuth", azimuth),
                ("elevation", elevation),
                ("fov", fov),
                ("height", height),
                ("near", near),
                ("far", far),
            ] {
                if let Some(v) = value {
                    cam.insert(key.into(), json!(v));
                }
            }
            if let Some(p) = projection {
                cam.insert("projection".into(), json!(p));
            }
            for (key, value, what) in [
                ("eye", eye, "--eye"),
                ("target", target, "--target"),
                ("up", up, "--up"),
            ] {
                if let Some(v) = value {
                    cam.insert(key.into(), triple(&v, what)?);
                }
            }
            if cam.is_empty() {
                n.remove("camera");
            }
            if no_physical || reset {
                n.remove("physical_camera");
            }
            let touched = focal_length.is_some()
                || sensor.is_some()
                || aperture.is_some()
                || focus_distance.is_some()
                || lens_exposure.is_some()
                || lens_samples.is_some();
            if touched {
                let existing = n.get("physical_camera").and_then(Value::as_object).cloned();
                let mut phys = existing.unwrap_or_default();
                if let Some(f) = focal_length {
                    phys.insert("focal_length_mm".into(), json!(f));
                }
                if let Some(s) = sensor {
                    let parts: Vec<&str> = s.split(['x', 'X']).collect();
                    ensure!(
                        parts.len() == 2,
                        "[invalid-input] --sensor must be WxH in millimeters (got `{s}`)"
                    );
                    let mut pair = Vec::new();
                    for p in parts {
                        pair.push(json!(p.trim().parse::<f64>().with_context(|| format!(
                            "[invalid-input] --sensor: `{p}` is not a number"
                        ))?));
                    }
                    phys.insert("sensor_mm".into(), Value::Array(pair));
                }
                for (key, value) in [
                    ("aperture_f", aperture),
                    ("focus_distance", focus_distance),
                    ("exposure_ev", lens_exposure),
                ] {
                    if let Some(v) = value {
                        phys.insert(key.into(), json!(v));
                    }
                }
                if let Some(s) = lens_samples {
                    phys.insert("samples".into(), json!(s));
                }
                ensure!(
                    phys.contains_key("focal_length_mm"),
                    "[invalid-input] a physical camera needs --focal-length"
                );
                // The lens now owns the field of view; drop a stale --fov.
                if let Some(c) = n.get_mut("camera").and_then(Value::as_object_mut) {
                    c.remove("fov");
                }
                n.insert("physical_camera".into(), Value::Object(phys));
            }
            ("scene3d-camera", json!({"id": id}))
        }
        Scene3dAction::Light { id, action } => {
            require_v10(&raw)?;
            let n = scene3d_node(&mut raw, &id)?;
            match action {
                LightAction::Clear => {
                    n.insert("lights".into(), json!([]));
                }
                LightAction::Reset => {
                    n.remove("lights");
                }
                LightAction::Add {
                    kind,
                    outer_deg,
                    inner_deg,
                    direction,
                    position,
                    color,
                    intensity,
                } => {
                    let mut light = json!({"kind": kind, "color": color, "intensity": intensity});
                    match kind.as_str() {
                        "directional" => {
                            light["direction"] = triple(
                                direction.as_deref().context(
                                    "[invalid-input] a directional light needs --direction x,y,z",
                                )?,
                                "--direction",
                            )?;
                        }
                        "point" => {
                            light["position"] = triple(
                                position.as_deref().context(
                                    "[invalid-input] a point light needs --position x,y,z",
                                )?,
                                "--position",
                            )?;
                        }
                        "spot" => {
                            light["position"] = triple(
                                position.as_deref().context(
                                    "[invalid-input] a spot light needs --position x,y,z",
                                )?,
                                "--position",
                            )?;
                            light["direction"] = triple(
                                direction.as_deref().context(
                                    "[invalid-input] a spot light needs --direction x,y,z",
                                )?,
                                "--direction",
                            )?;
                            if let Some(v) = outer_deg {
                                light["outer_deg"] = json!(v);
                            }
                            if let Some(v) = inner_deg {
                                light["inner_deg"] = json!(v);
                            }
                        }
                        other => bail!(
                            "[invalid-input] light kind `{other}` must be directional, point or spot"
                        ),
                    }
                    let lights = n
                        .entry("lights")
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .context("[invalid-input] lights must be an array")?;
                    lights.push(light);
                }
            }
            ("scene3d-light", json!({"id": id}))
        }
        Scene3dAction::Environment {
            id,
            sky,
            ground,
            intensity,
        } => {
            require_v10(&raw)?;
            let n = scene3d_node(&mut raw, &id)?;
            let env = n
                .entry("environment")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("[invalid-input] environment must be an object")?;
            if let Some(s) = sky {
                env.insert("sky".into(), json!(s));
            }
            if let Some(g) = ground {
                env.insert("ground".into(), json!(g));
            }
            if let Some(i) = intensity {
                env.insert("intensity".into(), json!(i));
            }
            ("scene3d-environment", json!({"id": id}))
        }
        Scene3dAction::Set {
            id,
            background,
            exposure_ev,
            supersample,
            pixel_ratio,
            shadows,
            shadow_map_size,
            x,
            y,
            width,
            height,
        } => {
            require_v10(&raw)?;
            let n = scene3d_node(&mut raw, &id)?;
            if let Some(b) = background {
                n.insert("background".into(), json!(b));
            }
            for (key, value) in [
                ("exposure_ev", exposure_ev),
                ("pixel_ratio", pixel_ratio),
                ("x", x),
                ("y", y),
                ("width", width),
                ("height", height),
            ] {
                if let Some(v) = value {
                    n.insert(key.into(), json!(v));
                }
            }
            if let Some(s) = supersample {
                n.insert("supersample".into(), json!(s));
            }
            if let Some(s) = shadows {
                n.insert("shadows".into(), json!(s));
            }
            if let Some(s) = shadow_map_size {
                n.insert("shadow_map_size".into(), json!(s));
            }
            ("scene3d-set", json!({"id": id}))
        }
    };
    crate::scene::validate(&raw)?;
    let change = crate::transaction::commit_value(input, label, dry_run, revision, &raw)?;
    result["change"] = serde_json::to_value(change)?;
    println!("{result}");
    Ok(())
}

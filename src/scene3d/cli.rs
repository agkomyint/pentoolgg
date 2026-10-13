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
        /// `directional` or `point`.
        #[arg(long)]
        kind: String,
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
pub enum Scene3dAction {
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
                        other => bail!(
                            "[invalid-input] light kind `{other}` must be directional or point"
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
            ("scene3d-set", json!({"id": id}))
        }
    };
    crate::scene::validate(&raw)?;
    let change = crate::transaction::commit_value(input, label, dry_run, revision, &raw)?;
    result["change"] = serde_json::to_value(change)?;
    println!("{result}");
    Ok(())
}

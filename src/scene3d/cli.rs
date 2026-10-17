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
    Show {
        /// Square texture width/height used for density estimates.
        #[arg(long, default_value_t = 2048)]
        resolution: u32,
        /// Emit a low-resolution warning for islands below this pixels/model-unit threshold.
        #[arg(long)]
        minimum_density: Option<f64>,
    },
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
    /// Create, transform, or remove an editable triangle UV island (format v11).
    Island {
        #[command(subcommand)]
        action: UvIslandAction,
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
pub enum UvIslandAction {
    /// List the editable islands for one primitive.
    List {
        #[arg(long, default_value_t = 0)]
        primitive: usize,
    },
    /// Assign triangles in a UV set to a named editable island.
    Add {
        #[arg(long, default_value_t = 0)]
        primitive: usize,
        #[arg(long)]
        id: String,
        #[arg(long, default_value_t = 0)]
        uv_set: usize,
        /// Zero-based triangle indices separated by commas.
        #[arg(long)]
        triangles: String,
        #[arg(long, allow_hyphen_values = true)]
        scale: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        offset: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        rotate: Option<f64>,
    },
    /// Change island ownership or its UV transform.
    Set {
        #[arg(long, default_value_t = 0)]
        primitive: usize,
        id: String,
        #[arg(long)]
        uv_set: Option<usize>,
        /// Zero-based triangle indices separated by commas.
        #[arg(long)]
        triangles: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        scale: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        offset: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        rotate: Option<f64>,
    },
    /// Remove an editable UV island.
    Remove {
        #[arg(long, default_value_t = 0)]
        primitive: usize,
        id: String,
    },
    /// Deterministically pack each editable UV set into the unit square.
    Pack {
        #[arg(long, default_value_t = 0)]
        primitive: usize,
        #[arg(long, default_value_t = 0.01)]
        padding: f64,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum MaterialAction {
    /// Override editable PBR values for one imported material.
    Set {
        /// Zero-based material index from `scene3d inspect`.
        material: usize,
        #[arg(long)]
        base_color: Option<String>,
        #[arg(long)]
        metallic: Option<f64>,
        #[arg(long)]
        roughness: Option<f64>,
        #[arg(long)]
        emissive: Option<String>,
        #[arg(long)]
        double_sided: Option<bool>,
    },
    /// Remove the override for one imported material.
    Clear { material: usize },
    /// Remove every material override from this scene.
    Reset,
}

#[derive(Debug, Clone, Subcommand)]
pub enum FormAction {
    /// List bundled editable product-form starters.
    Presets,
    /// Generate a parametric product form and place it on a v11 page.
    Add {
        /// Stable scene node ID.
        id: String,
        /// Stable product form catalog ID.
        #[arg(long)]
        form: String,
        /// Use a bundled starter: perfume-bottle, cosmetic-jar, cosmetic-tube, cylinder-container, shipping-carton, product-card, device-frame, stand-up-pouch.
        #[arg(long, conflicts_with_all = ["kind", "parameters"])]
        preset: Option<String>,
        /// `box`, `cylinder`, `tube` or `lathe-bottle` (optional bottle `cap_mm`).
        #[arg(long, requires = "parameters")]
        kind: Option<String>,
        /// Generator parameters as a JSON object.
        #[arg(long, requires = "kind")]
        parameters: Option<String>,
        /// Optional comma-separated stable material slot IDs.
        #[arg(long)]
        slots: Option<String>,
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
    },
    /// Update a form's dimensions/profile; every node using it updates together.
    Set {
        form: String,
        /// Generator parameters as a JSON object.
        #[arg(long)]
        parameters: String,
        /// Replace the comma-separated stable material slot IDs.
        #[arg(long)]
        slots: Option<String>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum GraphicAction {
    /// Add a linked page/node artwork layer to a named material slot.
    Add {
        graphic: String,
        #[arg(long)]
        source: String,
        #[arg(long)]
        slot: String,
        #[arg(long, default_value = "decal")]
        mode: String,
        #[arg(long, default_value = "uv")]
        projection: String,
        /// Planar projection axis: x, y or z.
        #[arg(long)]
        axis: Option<String>,
        #[arg(long, default_value = "contain")]
        fit: String,
        #[arg(long, default_value = "clamp")]
        wrap: String,
        #[arg(long, default_value = "normal")]
        blend: String,
        #[arg(long, allow_hyphen_values = true)]
        scale: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        offset: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        rotate: Option<f64>,
        #[arg(long)]
        crop: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        opacity: f64,
    },
    /// Update selected properties of an existing linked graphic.
    Set {
        graphic: String,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        slot: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        projection: Option<String>,
        #[arg(long)]
        axis: Option<String>,
        #[arg(long)]
        fit: Option<String>,
        #[arg(long)]
        wrap: Option<String>,
        #[arg(long)]
        blend: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        scale: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        offset: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        rotate: Option<f64>,
        #[arg(long)]
        crop: Option<String>,
        #[arg(long)]
        opacity: Option<f64>,
    },
    /// Remove a linked graphic layer.
    Remove { graphic: String },
    /// List linked graphics on this scene node.
    List,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Scene3dAction {
    /// Add, edit, inspect or remove live source-linked surface graphics.
    Graphic {
        id: String,
        #[command(subcommand)]
        action: GraphicAction,
    },
    /// Create parametric product forms (requires format v11).
    Form {
        #[command(subcommand)]
        action: FormAction,
    },
    /// Create bounded packaging mockups (requires format v11).
    Pack {
        #[command(subcommand)]
        action: PackAction,
    },
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
    /// Frame the complete model and store the resulting explicit camera.
    Frame {
        id: String,
        #[arg(long, default_value_t = 30.0, allow_hyphen_values = true)]
        azimuth: f64,
        #[arg(long, default_value_t = 15.0, allow_hyphen_values = true)]
        elevation: f64,
        /// Optionally frame using a physical focal length in millimeters.
        #[arg(long)]
        focal_length: Option<f64>,
        /// Sensor size in millimeters as `WxH` (default 36x24).
        #[arg(long)]
        sensor: Option<String>,
    },
    /// Edit the light rig.
    Light {
        id: String,
        #[command(subcommand)]
        action: LightAction,
    },
    /// Edit per-node PBR material overrides without changing the source model.
    Material {
        id: String,
        #[command(subcommand)]
        action: MaterialAction,
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
    Inspect {
        id: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// List scene3d nodes and models.
    List,
    /// Inspect resolved binding edges and dependency-first render order.
    Dependencies,
    /// Render one scene3d node and report exact output/cache metadata.
    RenderReport { id: String },
    /// Render selected scene3d nodes as one bounded, atomically published batch.
    Batch {
        /// Scene3d node selectors (ID, or PAGE::ID when IDs repeat across pages).
        #[arg(long = "node", required = true)]
        nodes: Vec<String>,
        /// New output directory; existing directories are never overwritten.
        #[arg(long)]
        out_dir: PathBuf,
        /// Existing file cancels the batch between renders without publishing partial output.
        #[arg(long)]
        cancel_file: Option<PathBuf>,
    },
    /// Render a bounded queue of mockups from multiple documents described by a JSON manifest.
    BatchQueue {
        /// New output directory; existing directories are never overwritten.
        #[arg(long)]
        out_dir: PathBuf,
        /// Existing file cancels the queue between renders without publishing partial output.
        #[arg(long)]
        cancel_file: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum CacheAction {
    /// Report render-cache entries, bytes, and counters from the running server.
    Status,
    /// Clear render results and counters in the running server.
    Clear,
}

/// Inspect or clear the disposable cache of a running local Pentool server.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_cache_command(server: &str, action: CacheAction) -> Result<()> {
    let url = reqwest::Url::parse(server).context("[invalid-input] server URL is invalid")?;
    ensure!(
        url.scheme() == "http"
            && url.host_str().is_some_and(|host| {
                host.eq_ignore_ascii_case("localhost")
                    || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
            })
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "[invalid-input] cache commands only connect to an HTTP loopback server URL, such as http://127.0.0.1:4711"
    );
    let endpoint = format!("{}/api/scene3d/cache", url.origin().ascii_serialization());
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = match action {
        CacheAction::Status => client.get(&endpoint).send(),
        CacheAction::Clear => client.delete(&endpoint).send(),
    }
    .context("[server-unavailable] could not reach the local Pentool server cache endpoint")?;
    ensure!(
        response.status().is_success(),
        "[server-error] local Pentool server returned HTTP {} for the cache command",
        response.status()
    );
    let body = response.text()?;
    let report: Value = serde_json::from_str(&body)
        .context("[malformed-response] local Pentool server returned invalid cache JSON")?;
    println!(
        "{}",
        json!({"server":url.origin().ascii_serialization(), "cache":report})
    );
    Ok(())
}

#[cfg(test)]
mod cache_command_tests {
    use super::*;

    #[test]
    fn cache_cli_refuses_non_loopback_destinations_before_connecting() {
        for server in [
            "https://127.0.0.1:4711",
            "http://example.com:4711",
            "http://127.0.0.1:4711/other",
        ] {
            let error = run_cache_command(server, CacheAction::Status).unwrap_err();
            assert!(error.to_string().contains("HTTP loopback"), "{error}");
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
pub enum PackAction {
    /// List the packaging templates currently supported by the generator.
    Templates,
    /// Import a labeled millimeter SVG dieline and add its folded preview node.
    Import {
        /// Stable dieline catalog ID.
        dieline: String,
        /// Input SVG with named panel rectangles and labeled production paths.
        #[arg(long)]
        svg: PathBuf,
        /// Stable scene3d preview node ID.
        #[arg(long)]
        node: String,
        /// Target layer for the preview node.
        #[arg(long)]
        layer: String,
        /// Target page (defaults to the first page).
        #[arg(long)]
        page: Option<String>,
        /// Preview node origin in document units.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        x: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        y: f64,
        /// Preview node size in document units.
        #[arg(long, default_value_t = 400.0)]
        width: f64,
        #[arg(long, default_value_t = 400.0)]
        height: f64,
    },
    /// Inspect a stored dieline and its panel, crease and path records.
    Inspect { dieline: String },
    /// Set one crease's target fold angle in degrees.
    Crease {
        /// Stable dieline catalog ID.
        dieline: String,
        /// Stable crease ID within the dieline.
        crease: String,
        /// Target angle for a fully folded preview (-180..=180 degrees).
        #[arg(long, allow_hyphen_values = true)]
        angle_deg: Option<f64>,
        /// Optional fold orientation for the crease.
        #[arg(long, value_parser = ["mountain", "valley"])]
        direction: Option<String>,
    },
    /// Update stored board, print bleed, or cylindrical wrap settings.
    Settings {
        /// Stable dieline catalog ID.
        dieline: String,
        /// Board/material thickness (0..=100 mm).
        #[arg(long)]
        board_mm: Option<f64>,
        /// Print bleed (0..=100 mm).
        #[arg(long)]
        bleed_mm: Option<f64>,
        /// Wrap seam angle (-360..=360 degrees; tube/bottle-label only).
        #[arg(long, allow_hyphen_values = true)]
        seam_angle_deg: Option<f64>,
        /// Cylindrical wrap direction.
        #[arg(long, value_parser = ["clockwise", "counterclockwise"])]
        wrap_direction: Option<String>,
    },
    /// Check linked art and stored paths; report mockup-only production limits.
    Preflight {
        /// Stable dieline catalog ID.
        dieline: String,
        /// Warn when rasterized linked artwork resolves below this ppi.
        #[arg(long, default_value_t = 300.0)]
        min_ppi: f64,
    },
    /// Rasterize stored dieline production paths into grayscale spot plates.
    Separate {
        /// Stable dieline catalog ID.
        dieline: String,
        /// New output directory for plates and separations.json.
        #[arg(long)]
        out: PathBuf,
        /// Plate resolution, 36..=1200 ppi.
        #[arg(long, default_value_t = 300.0)]
        ppi: f64,
        /// Plate bit depth (8 or 16).
        #[arg(long, default_value_t = 8)]
        bits: u8,
    },
    /// Export editable vector SVG plates grouped by dieline spot ink.
    VectorSeparate {
        /// Stable dieline catalog ID.
        dieline: String,
        /// New output directory for one SVG per spot ink.
        #[arg(long)]
        out: PathBuf,
    },
    /// Render a static CPU preview at the requested fold state without editing the document.
    Preview {
        /// Stable dieline catalog ID.
        dieline: String,
        /// Fold amount from flat (0) to the stored crease targets (1).
        #[arg(long, default_value_t = 1.0)]
        fold: f64,
        /// Destination PNG path.
        #[arg(long)]
        out: PathBuf,
    },
    /// Export a flat dieline technical SVG with linked artwork where supported.
    Export {
        /// Stable dieline catalog ID.
        dieline: String,
        /// Destination `.svg` path, or `.pdf` with --pdfx.
        #[arg(long)]
        out: PathBuf,
        /// Warn when rasterized linked artwork resolves below this ppi.
        #[arg(long, default_value_t = 300.0)]
        min_ppi: f64,
        /// Write a flattened PDF/X-4 process-CMYK draft (technical paths are rasterized).
        #[arg(long)]
        pdfx: bool,
        /// PDF/X raster resolution, 36..=1200 ppi.
        #[arg(long, default_value_t = 300.0)]
        ppi: f64,
    },
    /// Export a flat SVG, folded preview, PDF/X draft and spot plates as one atomic bundle.
    ExportBundle {
        /// Stable dieline catalog ID.
        dieline: String,
        /// New output directory; existing directories are never replaced.
        #[arg(long)]
        out: PathBuf,
        /// Warn when rasterized linked artwork resolves below this ppi.
        #[arg(long, default_value_t = 300.0)]
        min_ppi: f64,
        /// PDF/X and spot-plate resolution, 36..=1200 ppi.
        #[arg(long, default_value_t = 300.0)]
        ppi: f64,
        /// Spot-plate bit depth (8 or 16).
        #[arg(long, default_value_t = 8)]
        bits: u8,
        /// Fold amount used for the static preview (0 flat, 1 stored target angles).
        #[arg(long, default_value_t = 1.0)]
        fold: f64,
    },
    /// Verify a packaging bundle manifest and every hash-listed artifact.
    VerifyBundle {
        /// Existing bundle directory created by `pack export-bundle`.
        bundle: PathBuf,
    },
    /// Generate a visual four-panel open-ended sleeve and folded preview.
    Sleeve {
        /// Stable dieline catalog ID.
        #[arg(long)]
        dieline: String,
        /// Stable scene3d node ID.
        #[arg(long)]
        node: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        page: Option<String>,
        #[arg(long)]
        length_mm: f64,
        #[arg(long)]
        width_mm: f64,
        #[arg(long)]
        height_mm: f64,
        #[arg(long, default_value_t = 0.5)]
        board_mm: f64,
        #[arg(long, default_value_t = 3.0)]
        bleed_mm: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        x: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        y: f64,
        #[arg(long, default_value_t = 400.0)]
        width: f64,
        #[arg(long, default_value_t = 400.0)]
        height: f64,
    },
    /// Generate a simplified stand-up-pouch mockup with front/back, gussets and bottom.
    Pouch {
        /// Stable dieline catalog ID.
        #[arg(long)]
        dieline: String,
        /// Stable scene3d node ID.
        #[arg(long)]
        node: String,
        #[arg(long)]
        layer: String,
        /// Target page (defaults to the first page).
        #[arg(long)]
        page: Option<String>,
        /// Front panel width in millimeters.
        #[arg(long)]
        front_width_mm: f64,
        /// Side/bottom gusset depth in millimeters.
        #[arg(long)]
        gusset_mm: f64,
        #[arg(long)]
        height_mm: f64,
        #[arg(long, default_value_t = 3.0)]
        bleed_mm: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        x: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        y: f64,
        #[arg(long, default_value_t = 400.0)]
        width: f64,
        #[arg(long, default_value_t = 400.0)]
        height: f64,
    },
    /// Generate a cylindrical label with one shared flat-to-curved surface map.
    Tube {
        /// Stable dieline catalog ID.
        #[arg(long)]
        dieline: String,
        /// Stable scene3d node ID.
        #[arg(long)]
        node: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        page: Option<String>,
        #[arg(long)]
        radius_mm: f64,
        #[arg(long)]
        height_mm: f64,
        #[arg(long, default_value_t = 3.0)]
        bleed_mm: f64,
        /// Extra printable width that wraps over the label start (0..25% circumference).
        #[arg(long, default_value_t = 0.0)]
        overlap_mm: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        x: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        y: f64,
        #[arg(long, default_value_t = 400.0)]
        width: f64,
        #[arg(long, default_value_t = 400.0)]
        height: f64,
    },
    /// Generate a measured-radius bottle-label wrap (cylindrical label zone).
    BottleLabel {
        /// Stable dieline catalog ID.
        #[arg(long)]
        dieline: String,
        /// Stable scene3d node ID.
        #[arg(long)]
        node: String,
        #[arg(long)]
        layer: String,
        /// Target page (defaults to the first page).
        #[arg(long)]
        page: Option<String>,
        /// Radius of the label zone in millimeters.
        #[arg(long)]
        radius_mm: f64,
        /// Label height in millimeters.
        #[arg(long)]
        height_mm: f64,
        #[arg(long, default_value_t = 3.0)]
        bleed_mm: f64,
        /// Printable tail overlap (0..25% of the label circumference).
        #[arg(long, default_value_t = 0.0)]
        overlap_mm: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        x: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        y: f64,
        #[arg(long, default_value_t = 400.0)]
        width: f64,
        #[arg(long, default_value_t = 400.0)]
        height: f64,
    },
    /// Generate a straight- or reverse-tuck carton with closure flaps.
    Carton {
        /// Stable dieline catalog ID.
        #[arg(long)]
        dieline: String,
        /// Stable scene3d node ID.
        #[arg(long)]
        node: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        page: Option<String>,
        #[arg(long, default_value = "straight-tuck", value_parser = ["straight-tuck", "reverse-tuck", "mailer"])]
        template: String,
        #[arg(long)]
        length_mm: f64,
        #[arg(long)]
        width_mm: f64,
        #[arg(long)]
        height_mm: f64,
        #[arg(long, default_value_t = 0.5)]
        board_mm: f64,
        #[arg(long, default_value_t = 3.0)]
        bleed_mm: f64,
        #[arg(long, default_value_t = 15.0)]
        glue_flap_mm: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        x: f64,
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        y: f64,
        #[arg(long, default_value_t = 400.0)]
        width: f64,
        #[arg(long, default_value_t = 400.0)]
        height: f64,
    },
    /// Link a page or node to one packaging panel, or clear it with no source.
    Bind {
        /// Stable dieline catalog ID.
        dieline: String,
        /// Stable panel ID within the dieline.
        #[arg(long)]
        panel: String,
        /// `page:<page-id>` or `node:<page-id>:<node-id>`; omit to clear.
        #[arg(long)]
        source: Option<String>,
    },
}

fn read(input: &Path) -> Result<Value> {
    let bytes = std::fs::read(input)
        .with_context(|| format!("[missing-resource] could not read {}", input.display()))?;
    serde_json::from_slice(&bytes).context("[malformed-resource] invalid .pen document")
}

fn print_pack_templates() {
    println!(
        "{}",
        json!({"templates":[
            {"id":"sleeve","status":"supported","panels":4,"limitations":"open-ended visual mockup"},
            {"id":"straight-tuck","status":"supported","panels":13,"limitations":"visual net with panel interiors, exposed cut-edge walls, and visual folded-hinge bridges; score compression, allowances, and manufacturing certification are not provided"},
            {"id":"reverse-tuck","status":"supported","panels":13,"limitations":"visual net with panel interiors, exposed cut-edge walls, and visual folded-hinge bridges; score compression, allowances, and manufacturing certification are not provided"},
            {"id":"mailer","status":"supported","panels":7,"limitations":"visual net with hinged lid and glue joint, panel interiors, exposed cut-edge walls, and visual folded-hinge bridges; score compression, lock tabs, and manufacturing certification are not provided"},
            {"id":"pouch","status":"supported","panels":5,"limitations":"simplified box-bottom stand-up pouch mockup; no zipper, formed seals, film thickness, or manufacturing certification"},
            {"id":"bottle-label","status":"supported","panels":1,"limitations":"measured cylindrical label zone; does not conform to tapered bottle profiles and is not manufacturing-certified"},
            {"id":"tube-label","status":"supported","panels":1,"limitations":"single cylindrical wrap surface with a measured circumference; no end cap geometry or manufacturing certification"},
            {"id":"svg-import","status":"supported-subset","panels":"1..=256 labeled rectangles","limitations":"absolute millimeter SVG; straight direction-labeled folds; no transforms, curved fold paths, external resources, or certification"}
        ]})
    );
}

/// Run the short packaging command namespace. Template discovery is static;
/// all document operations delegate to the same scene3d planner and transaction.
pub fn run_pack(
    input: Option<&Path>,
    dry_run: bool,
    revision: Option<&str>,
    action: PackAction,
) -> Result<()> {
    if let PackAction::VerifyBundle { bundle } = &action {
        ensure!(
            input.is_none(),
            "[invalid-input] `pack verify-bundle` takes a bundle directory, not a document path"
        );
        let report = verify_pack_bundle(bundle)?;
        println!("{report}");
        return Ok(());
    }
    if input.is_none() && matches!(&action, PackAction::Templates) {
        print_pack_templates();
        return Ok(());
    }
    let input =
        input.context("[invalid-input] `pentool pack` document operations require a DOC path")?;
    run(input, dry_run, revision, Scene3dAction::Pack { action })
}

pub(crate) fn verify_pack_bundle(bundle: &Path) -> Result<Value> {
    let root = bundle.canonicalize().with_context(|| {
        format!(
            "[missing-resource] packaging bundle {} does not exist",
            bundle.display()
        )
    })?;
    ensure!(
        root.is_dir(),
        "[invalid-input] packaging bundle must be a directory"
    );
    let manifest_path = root.join("manifest.json");
    let manifest_bytes = crate::resource::read_bounded(&manifest_path, 4 * 1024 * 1024)?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)
        .context("[malformed-resource] packaging bundle manifest.json is invalid JSON")?;
    ensure!(
        manifest["format"].as_str() == Some("pentool-packaging-bundle-v1"),
        "[unsupported-capability] unsupported packaging bundle manifest format"
    );
    let mut entries: Vec<(String, String, Option<u64>)> = Vec::new();
    fn visit(value: &Value, prefix: &str, out: &mut Vec<(String, String, Option<u64>)>) {
        match value {
            Value::Object(map) => {
                let next_prefix = map
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|path| path.ends_with('/'))
                    .map(|path| format!("{prefix}{path}"))
                    .unwrap_or_else(|| prefix.to_owned());
                if let (Some(path), Some(hash)) = (
                    map.get("path").and_then(Value::as_str),
                    map.get("sha256").and_then(Value::as_str),
                ) {
                    out.push((
                        format!("{prefix}{path}"),
                        hash.to_owned(),
                        map.get("bytes").and_then(Value::as_u64),
                    ));
                }
                for child in map.values() {
                    visit(child, &next_prefix, out);
                }
            }
            Value::Array(items) => {
                for child in items {
                    visit(child, prefix, out);
                }
            }
            _ => {}
        }
    }
    visit(&manifest["files"], "", &mut entries);
    ensure!(
        !entries.is_empty() && entries.len() <= 4096,
        "[invalid-input] packaging bundle manifest must list 1..=4096 hash-verified artifacts"
    );
    let mut verified = Vec::with_capacity(entries.len());
    let mut seen = std::collections::BTreeSet::new();
    for (relative, expected_hash, expected_bytes) in entries {
        let relative_path = Path::new(&relative);
        ensure!(
            !relative_path.is_absolute()
                && relative_path
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_))),
            "[invalid-input] packaging bundle manifest contains unsafe artifact path `{relative}`"
        );
        ensure!(
            seen.insert(relative.clone()),
            "[invalid-input] packaging bundle manifest lists `{relative}` more than once"
        );
        let path = root.join(relative_path);
        let resolved = path.canonicalize().with_context(|| {
            format!("[missing-resource] packaging artifact `{relative}` is missing")
        })?;
        ensure!(
            resolved.starts_with(&root) && resolved.is_file(),
            "[invalid-input] packaging artifact `{relative}` escapes the bundle or is not a file"
        );
        let metadata = std::fs::metadata(&resolved)?;
        ensure!(metadata.len() <= 512 * 1024 * 1024,
            "[limit-exceeded] packaging artifact `{relative}` exceeds the 512 MiB verification limit");
        if let Some(expected) = expected_bytes {
            ensure!(metadata.len() == expected, "[integrity-error] packaging artifact `{relative}` has {} bytes; manifest says {expected}", metadata.len());
        }
        let bytes = crate::resource::read_bounded(&resolved, 512 * 1024 * 1024)?;
        let actual_hash = crate::resource::sha256(&bytes);
        ensure!(
            actual_hash == expected_hash,
            "[integrity-error] packaging artifact `{relative}` SHA-256 mismatch"
        );
        verified.push(serde_json::json!({"path":relative,"bytes":metadata.len(),"sha256":actual_hash,"status":"verified"}));
    }
    // A set of valid hashes can still describe a misleading print handoff.
    // Check the authored SVG semantics as well: the review map must be marked
    // non-production, and each vector plate must contain exactly one known
    // finishing operation under its declared ink.
    ensure!(
        manifest["files"]["cutting_map"]["visual_reference_only"].as_bool() == Some(true),
        "[integrity-error] packaging cutting map must be explicitly marked as visual-reference-only"
    );
    ensure!(
        manifest["files"]["flat_svg"]["path"].as_str() == Some("flat.svg")
            && manifest["files"]["cutting_map"]["path"].as_str() == Some("CUTTING-MAP.svg")
            && manifest["files"]["vector_spot_plates"]["path"].as_str()
                == Some("vector-spot-plates/"),
        "[integrity-error] packaging manifest uses unexpected flat, cutting-map, or vector-plate paths"
    );
    let flat_path = root.join("flat.svg");
    let flat_bytes = crate::resource::read_bounded(&flat_path, 16 * 1024 * 1024)?;
    let flat_text = std::str::from_utf8(&flat_bytes)
        .context("[integrity-error] flat packaging artwork is not UTF-8 SVG")?;
    let flat_doc = roxmltree::Document::parse(flat_text)
        .context("[integrity-error] flat packaging artwork is malformed SVG")?;
    ensure!(
        flat_doc.root_element().tag_name().name() == "svg",
        "[integrity-error] flat packaging artwork root must be SVG"
    );
    let cutting_map_path = root.join("CUTTING-MAP.svg");
    let cutting_map_bytes = crate::resource::read_bounded(&cutting_map_path, 16 * 1024 * 1024)?;
    let cutting_map_text = std::str::from_utf8(&cutting_map_bytes)
        .context("[integrity-error] packaging cutting map is not UTF-8 SVG")?;
    let cutting_map_doc = roxmltree::Document::parse(cutting_map_text)
        .context("[integrity-error] packaging cutting map is malformed SVG")?;
    ensure!(
        cutting_map_doc.root_element().tag_name().name() == "svg",
        "[integrity-error] packaging cutting map root must be SVG"
    );
    let collect_production_paths = |doc: &roxmltree::Document<'_>| {
        doc.descendants()
            .filter(|node| node.tag_name().name() == "path")
            .filter_map(|node| {
                let role = node.attribute("data-role")?;
                if !matches!(
                    role,
                    "cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap"
                ) {
                    return None;
                }
                Some((
                    node.attribute("data-id")?.to_owned(),
                    role.to_owned(),
                    node.attribute("d")?.to_owned(),
                ))
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    let flat_production_paths = collect_production_paths(&flat_doc);
    let map_production_paths = collect_production_paths(&cutting_map_doc);
    ensure!(
        !flat_production_paths.is_empty() && flat_production_paths == map_production_paths,
        "[integrity-error] cutting map production geometry does not match the flat dieline's cut/fold guides"
    );
    let map_has_reference_legend = cutting_map_doc.descendants().any(|node| {
        node.attribute("id") == Some("cutting-map-legend")
            && node.attribute("data-reference-only") == Some("true")
    });
    ensure!(
        map_has_reference_legend,
        "[integrity-error] packaging cutting map is missing its visual-reference-only legend"
    );
    let map_labels = cutting_map_doc
        .descendants()
        .filter(|node| node.is_text())
        .filter_map(|node| node.text())
        .collect::<Vec<_>>()
        .join(" ");
    for label in [
        "CUT · through-cut (red)",
        "CREASE · fold/score, not a cut (blue)",
        "Valley fold · blue solid",
        "Mountain fold · violet dashed",
        "PERFORATION · tear line (purple)",
        "BLEED · artwork extension beyond trim (orange)",
        "SAFE ZONE · keep important artwork inside (green)",
        "GLUE FLAP · adhesive area; keep critical art clear (cyan)",
        "Visual reference only.",
    ] {
        ensure!(
            map_labels.contains(label),
            "[integrity-error] packaging cutting map is missing or changes required production guidance `{label}`"
        );
    }
    let vector_report = &manifest["files"]["vector_spot_plates"]["report"];
    ensure!(
        vector_report["format"].as_str() == Some("svg-vector-spot-plates"),
        "[integrity-error] packaging vector spot plate report has an unsupported format"
    );
    let plates = vector_report["plates"]
        .as_array()
        .context("[integrity-error] packaging vector spot plate report has no plate list")?;
    ensure!(
        !plates.is_empty() && plates.len() <= 32,
        "[integrity-error] packaging vector spot plate report must contain 1..=32 plates"
    );
    let parse_mm = |value: Option<&str>, label: &str| -> Result<f64> {
        value
            .and_then(|value| value.strip_suffix("mm"))
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .with_context(|| {
                format!("[integrity-error] {label} must be a positive millimeter dimension")
            })
    };
    let parse_view_box_value = |value: &str, label: &str| -> Result<f64> {
        let number = value
            .parse::<f64>()
            .with_context(|| format!("[integrity-error] {label} has an invalid viewBox"))?;
        ensure!(
            number.is_finite(),
            "[integrity-error] {label} has a non-finite viewBox"
        );
        Ok(number)
    };
    let flat_width_mm = parse_mm(flat_doc.root_element().attribute("width"), "flat SVG width")?;
    let flat_height_mm = parse_mm(
        flat_doc.root_element().attribute("height"),
        "flat SVG height",
    )?;
    let flat_view_box = flat_doc
        .root_element()
        .attribute("viewBox")
        .context("[integrity-error] flat SVG is missing its viewBox")?
        .split_ascii_whitespace()
        .map(|value| parse_view_box_value(value, "flat SVG"))
        .collect::<Result<Vec<_>>>()?;
    let same_measurement = |left: f64, right: f64| (left - right).abs() <= 1e-7;
    ensure!(
        flat_view_box.len() == 4
            && same_measurement(flat_view_box[0], 0.0)
            && same_measurement(flat_view_box[1], 0.0)
            && same_measurement(flat_view_box[2], flat_width_mm)
            && same_measurement(flat_view_box[3], flat_height_mm),
        "[integrity-error] flat SVG physical dimensions do not match its millimeter viewBox"
    );
    let reported_width_mm = vector_report["width_mm"]
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0)
        .context("[integrity-error] vector spot plate report has invalid width")?;
    let reported_height_mm = vector_report["height_mm"]
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0)
        .context("[integrity-error] vector spot plate report has invalid height")?;
    ensure!(
        same_measurement(reported_width_mm, flat_width_mm)
            && same_measurement(reported_height_mm, flat_height_mm),
        "[integrity-error] vector spot plate report dimensions do not match the flat dieline"
    );
    let plate_bleed_mm = vector_report["bleed_mm"]
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .context("[integrity-error] packaging vector spot plate report has invalid bleed")?;
    let canonical_paths = |data: &str, translate_bleed: bool| -> Result<Vec<String>> {
        let mut path = kurbo::BezPath::from_svg(data)
            .context("[integrity-error] packaging production path is malformed")?;
        if translate_bleed {
            path.apply_affine(kurbo::Affine::translate((plate_bleed_mm, plate_bleed_mm)));
        }
        let mut subpaths = Vec::new();
        let mut current = kurbo::BezPath::new();
        for element in path.elements() {
            if matches!(element, kurbo::PathEl::MoveTo(_)) && !current.elements().is_empty() {
                subpaths.push(current.to_svg());
                current = kurbo::BezPath::new();
            }
            current.push(*element);
        }
        if !current.elements().is_empty() {
            subpaths.push(current.to_svg());
        }
        Ok(subpaths)
    };
    let mut flat_geometry =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for element in flat_doc
        .descendants()
        .filter(|node| matches!(node.tag_name().name(), "path" | "rect"))
    {
        let Some(role) = element.attribute("data-role") else {
            continue;
        };
        if !matches!(
            role,
            "cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap"
        ) {
            continue;
        }
        let data = if element.tag_name().name() == "path" {
            element
                .attribute("d")
                .context("[integrity-error] flat production path is missing its geometry")?
                .to_owned()
        } else {
            let x = element.attribute("x").context(
                "[integrity-error] flat production rectangle is missing its x coordinate",
            )?;
            let y = element.attribute("y").context(
                "[integrity-error] flat production rectangle is missing its y coordinate",
            )?;
            let width = element
                .attribute("width")
                .context("[integrity-error] flat production rectangle is missing its width")?;
            let height = element
                .attribute("height")
                .context("[integrity-error] flat production rectangle is missing its height")?;
            format!("M {x} {y} h {width} v {height} h -{width} Z")
        };
        flat_geometry
            .entry(role.to_owned())
            .or_default()
            .extend(canonical_paths(&data, false)?);
    }
    for (role, paths) in &flat_geometry {
        for data in paths {
            let path = kurbo::BezPath::from_svg(data)
                .context("[integrity-error] normalized flat production path is malformed")?;
            let bounds = kurbo::Shape::bounding_box(&path);
            ensure!(
                [bounds.x0, bounds.y0, bounds.x1, bounds.y1]
                    .iter()
                    .all(|value| value.is_finite())
                    && bounds.x0 >= -1e-7
                    && bounds.y0 >= -1e-7
                    && bounds.x1 <= flat_width_mm + 1e-7
                    && bounds.y1 <= flat_height_mm + 1e-7,
                "[integrity-error] flat production geometry for `{role}` falls outside its declared print page"
            );
        }
    }
    let mut production_roles = std::collections::BTreeSet::new();
    let mut production_inks = std::collections::BTreeSet::new();
    for plate in plates {
        let ink = plate["ink"]
            .as_str()
            .context("[integrity-error] packaging vector spot plate is missing its declared ink")?;
        let relative = plate["path"].as_str().context(
            "[integrity-error] packaging vector spot plate is missing its artifact path",
        )?;
        ensure!(
            Path::new(relative).components().count() == 1
                && Path::new(relative)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_))),
            "[integrity-error] packaging vector spot plate path `{relative}` must be a file name"
        );
        ensure!(
            production_inks.insert(ink),
            "[integrity-error] packaging vector spot plate ink `{ink}` is listed more than once"
        );
        let manifest_artifact = format!("vector-spot-plates/{relative}");
        ensure!(
            seen.contains(&manifest_artifact),
            "[integrity-error] packaging vector spot plate `{relative}` is not checksum-listed in the manifest"
        );
        let relative_path = Path::new("vector-spot-plates").join(relative);
        let plate_bytes =
            crate::resource::read_bounded(&root.join(&relative_path), 16 * 1024 * 1024)
                .with_context(|| {
                    format!("[integrity-error] could not read vector spot plate `{relative}`")
                })?;
        let plate_text = std::str::from_utf8(&plate_bytes)
            .context("[integrity-error] vector spot plate is not UTF-8 SVG")?;
        let plate_doc = roxmltree::Document::parse(plate_text)
            .context("[integrity-error] packaging vector spot plate is malformed SVG")?;
        let plate_root = plate_doc.root_element();
        ensure!(
            plate_root.tag_name().name() == "svg"
                && plate_root.attribute("data-spot-ink") == Some(ink),
            "[integrity-error] vector spot plate `{relative}` does not declare manifest ink `{ink}`"
        );
        let plate_width_mm = parse_mm(plate_root.attribute("width"), "vector spot plate width")?;
        let plate_height_mm = parse_mm(plate_root.attribute("height"), "vector spot plate height")?;
        let plate_view_box = plate_root
            .attribute("viewBox")
            .context("[integrity-error] vector spot plate is missing its viewBox")?
            .split_ascii_whitespace()
            .map(|value| parse_view_box_value(value, "vector spot plate"))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            same_measurement(plate_width_mm, flat_width_mm)
                && same_measurement(plate_height_mm, flat_height_mm)
                && plate_view_box.len() == 4
                && same_measurement(plate_view_box[0], 0.0)
                && same_measurement(plate_view_box[1], 0.0)
                && same_measurement(plate_view_box[2], flat_width_mm)
                && same_measurement(plate_view_box[3], flat_height_mm),
            "[integrity-error] vector spot plate `{relative}` physical page dimensions do not match the flat dieline"
        );
        let roles: std::collections::BTreeSet<String> = plate_doc
            .descendants()
            .filter(|node| node.tag_name().name() == "path")
            .filter_map(|node| node.attribute("data-role"))
            .map(str::to_owned)
            .collect();
        ensure!(
            roles.len() == 1,
            "[integrity-error] vector spot plate `{relative}` must contain paths for exactly one finishing role"
        );
        let role = roles.first().expect("one role was checked above");
        ensure!(
            matches!(
                role.as_str(),
                "cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap"
            ),
            "[integrity-error] vector spot plate `{relative}` has unknown finishing role `{role}`"
        );
        let canonical_role = match ink {
            "CutContour" => Some("cut"),
            "Crease" => Some("crease"),
            "Perforation" => Some("perforation"),
            "Bleed" => Some("bleed"),
            "SafeZone" => Some("safe-zone"),
            "GlueFlap" => Some("glue-flap"),
            _ => None,
        };
        if let Some(expected_role) = canonical_role {
            ensure!(
                role == expected_role,
                "[integrity-error] canonical spot ink `{ink}` is paired with `{role}` paths instead of `{expected_role}`"
            );
        }
        let expected_geometry = flat_geometry.get(role).cloned().unwrap_or_default();
        let plate_geometry = plate_doc
            .descendants()
            .filter(|node| node.tag_name().name() == "path")
            .filter_map(|node| node.attribute("d"))
            .map(|data| canonical_paths(data, true))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<std::collections::BTreeSet<_>>();
        ensure!(
            !expected_geometry.is_empty() && plate_geometry == expected_geometry,
            "[integrity-error] vector spot plate `{relative}` production geometry does not match the flat dieline role `{role}`"
        );
        production_roles.insert(role.clone());
    }
    Ok(serde_json::json!({
        "format":"pentool-packaging-bundle-verification-v1",
        "bundle":root,
        "dieline":manifest["dieline"],
        "document_revision":manifest["document_revision"],
        "artifacts_verified":verified.len(),
        "artifacts":verified,
        "production_roles":production_roles,
        "vector_plates_verified":plates.len(),
        "manufacturing_certified":false,
        "status":"verified"
    }))
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

fn sensor_pair(text: &str) -> Result<Value> {
    let parts: Vec<&str> = text.split(['x', 'X']).collect();
    ensure!(
        parts.len() == 2,
        "[invalid-input] --sensor must be WxH in millimeters (got `{text}`)"
    );
    let values = parts
        .into_iter()
        .map(|part| {
            let value = part
                .trim()
                .parse::<f64>()
                .with_context(|| format!("[invalid-input] --sensor: `{part}` is not a number"))?;
            ensure!(value.is_finite(), "[invalid-input] --sensor must be finite");
            Ok(json!(value))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Value::Array(values))
}

fn require_v10(raw: &Value) -> Result<()> {
    ensure!(
        node::is_v10(raw),
        "[unsupported-capability] scene3d needs document format version 10 or newer; run `pentool migrate --target 10` first"
    );
    Ok(())
}

fn require_v11(raw: &Value) -> Result<()> {
    ensure!(
        raw.get("version")
            .and_then(Value::as_u64)
            .is_some_and(|version| version >= node::BINDINGS_VERSION),
        "[unsupported-capability] product forms require document format version 11; run `pentool migrate --target 11` first"
    );
    Ok(())
}

fn parse_binding_source(text: &str) -> Result<Value> {
    if let Some(page) = text.strip_prefix("page:") {
        ensure!(
            !page.trim().is_empty(),
            "[invalid-input] page source must be page:<page-id>"
        );
        return Ok(json!({"kind":"page", "page":page}));
    }
    if let Some(ids) = text.strip_prefix("node:") {
        let (page, node) = ids
            .split_once(':')
            .context("[invalid-input] node source must be node:<page-id>:<node-id>")?;
        ensure!(
            !page.trim().is_empty() && !node.trim().is_empty(),
            "[invalid-input] node source needs non-empty page and node IDs"
        );
        ensure!(
            !node.contains(':'),
            "[invalid-input] node IDs cannot contain `:` in source selectors"
        );
        return Ok(json!({"kind":"node", "page":page, "node":node}));
    }
    bail!("[invalid-input] --source must be page:<page-id> or node:<page-id>:<node-id>")
}

fn parse_pair(text: &str, name: &str, count: usize) -> Result<Value> {
    let parts: Vec<_> = text.split(',').map(str::trim).collect();
    ensure!(
        parts.len() == count,
        "[invalid-input] {name} must contain {count} comma-separated numbers"
    );
    let values = parts
        .into_iter()
        .map(|part| {
            let value = part
                .parse::<f64>()
                .with_context(|| format!("[invalid-input] {name}: `{part}` is not a number"))?;
            ensure!(
                value.is_finite(),
                "[invalid-input] {name} values must be finite"
            );
            Ok(json!(value))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Value::Array(values))
}

fn parse_triangle_indices(text: &str) -> Result<Value> {
    let parts: Vec<_> = text.split(',').map(str::trim).collect();
    ensure!(
        !parts.is_empty() && parts.len() <= 1_000_000,
        "[limit-exceeded] UV islands need 1..=1000000 triangle indices"
    );
    let mut seen = std::collections::BTreeSet::new();
    let mut values = Vec::with_capacity(parts.len());
    for part in parts {
        let index = part.parse::<usize>().with_context(|| {
            format!("[invalid-input] triangle index `{part}` is not a nonnegative integer")
        })?;
        ensure!(
            seen.insert(index),
            "[invalid-input] triangle index {index} is repeated"
        );
        values.push(json!(index));
    }
    Ok(Value::Array(values))
}

#[allow(clippy::too_many_arguments)]
fn add_form(
    raw: &mut Value,
    id: &str,
    form_id: &str,
    preset: Option<&str>,
    kind: Option<&str>,
    parameters_json: Option<&str>,
    slots: Option<&str>,
    layer_id: &str,
    page_id: Option<&str>,
    placement: [f64; 4],
) -> Result<Value> {
    require_v11(raw)?;
    ensure!(
        !id.trim().is_empty() && !form_id.trim().is_empty(),
        "[invalid-input] node and form IDs cannot be empty"
    );
    ensure!(
        !node_exists(raw, id),
        "[invalid-input] a node with id `{id}` already exists"
    );
    raw["product_forms"] = raw
        .get("product_forms")
        .cloned()
        .unwrap_or_else(|| json!({}));
    ensure!(
        raw["product_forms"].get(form_id).is_none(),
        "[invalid-input] product form `{form_id}` already exists"
    );
    let mut form = if let Some(preset) = preset {
        super::form::starter_preset(preset)?
    } else {
        let kind = kind.context(
            "[invalid-input] choose a bundled --preset or provide both --kind and --parameters",
        )?;
        let parameters_json = parameters_json.context(
            "[invalid-input] choose a bundled --preset or provide both --kind and --parameters",
        )?;
        let parameters: Value = serde_json::from_str(parameters_json)
            .context("[invalid-input] --parameters must be a JSON object")?;
        ensure!(
            parameters.is_object(),
            "[invalid-input] --parameters must be a JSON object"
        );
        json!({"kind":kind,"generator":1,"parameters":parameters})
    };
    if let Some(slots) = slots {
        let slots: Vec<&str> = slots.split(',').map(str::trim).collect();
        ensure!(
            slots.iter().all(|slot| !slot.is_empty()),
            "[invalid-input] --slots cannot contain empty IDs"
        );
        form["material_slots"] = json!(slots);
    }
    let model = super::form::generate(&form).with_context(|| format!("product form {form_id}"))?;
    let kind = form["kind"].clone();
    let pages = raw
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .context("[invalid-input] document has no pages")?;
    let page = match page_id {
        Some(id) => pages
            .iter_mut()
            .find(|page| page.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("[missing-resource] no page `{id}`"))?,
        None => pages
            .first_mut()
            .context("[invalid-input] document has no pages")?,
    };
    let layers = page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("[invalid-input] page has no layers")?;
    let target = layers
        .iter_mut()
        .find(|layer| layer.get("id").and_then(Value::as_str) == Some(layer_id))
        .with_context(|| format!("[missing-resource] no layer `{layer_id}`"))?;
    ensure!(
        target.get("locked").and_then(Value::as_bool) != Some(true),
        "[locked] layer `{layer_id}` is locked"
    );
    target.get_mut("nodes").and_then(Value::as_array_mut).context("[invalid-input] layer has no nodes")?
        .push(json!({"kind":"scene3d","id":id,"x":placement[0],"y":placement[1],"width":placement[2],"height":placement[3],"product_form":form_id}));
    raw["product_forms"][form_id] = form;
    Ok(
        json!({"id":id,"product_form":form_id,"kind":kind,"report":{"vertices":model.report.vertices,"triangles":model.report.triangles,"slots":model.materials.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()}}),
    )
}

fn set_form(
    raw: &mut Value,
    form_id: &str,
    parameters_json: &str,
    slots: Option<&str>,
) -> Result<Value> {
    require_v11(raw)?;
    let existing = raw
        .get("product_forms")
        .and_then(|forms| forms.get(form_id))
        .with_context(|| format!("[missing-resource] no product form `{form_id}`"))?;
    let mut candidate = existing.clone();
    let parameters: Value = serde_json::from_str(parameters_json)
        .context("[invalid-input] --parameters must be a JSON object")?;
    ensure!(
        parameters.is_object(),
        "[invalid-input] --parameters must be a JSON object"
    );
    candidate["parameters"] = parameters;
    if let Some(slots) = slots {
        let slots: Vec<&str> = slots.split(',').map(str::trim).collect();
        ensure!(
            slots.iter().all(|slot| !slot.is_empty()),
            "[invalid-input] --slots cannot contain empty IDs"
        );
        candidate["material_slots"] = json!(slots);
    }
    let model =
        super::form::generate(&candidate).with_context(|| format!("product form {form_id}"))?;
    raw["product_forms"][form_id] = candidate;
    Ok(
        json!({"product_form":form_id,"report":{"vertices":model.report.vertices,"triangles":model.report.triangles,"slots":model.materials.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()}}),
    )
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

fn find_node_readonly<'a>(nodes: &'a [Value], id: &str) -> Option<&'a Value> {
    for node in nodes {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(found) = node
            .get("children")
            .and_then(Value::as_array)
            .and_then(|children| find_node_readonly(children, id))
        {
            return Some(found);
        }
    }
    None
}

fn scene3d_node_with_page(raw: &Value, id: &str) -> Result<(String, Map<String, Value>)> {
    let pages = raw
        .get("pages")
        .and_then(Value::as_array)
        .context("[invalid-input] document has no pages")?;
    for page in pages {
        let Some(page_id) = page.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(layers) = page.get("layers").and_then(Value::as_array) else {
            continue;
        };
        for layer in layers {
            let Some(nodes) = layer.get("nodes").and_then(Value::as_array) else {
                continue;
            };
            if let Some(node) = find_node_readonly(nodes, id) {
                ensure!(
                    node.get("kind").and_then(Value::as_str) == Some("scene3d"),
                    "[invalid-input] node {id} is not a scene3d node"
                );
                return Ok((
                    page_id.to_owned(),
                    node.as_object()
                        .context("[invalid-input] scene3d node must be an object")?
                        .clone(),
                ));
            }
        }
    }
    bail!("[missing-resource] no scene3d node `{id}`; list them with `pentool scene3d <doc> list`")
}

fn ensure_scene3d_layer_unlocked(raw: &Value, id: &str) -> Result<()> {
    scene3d_node_with_page(raw, id)?;
    for page in raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for layer in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if layer
                .get("nodes")
                .and_then(Value::as_array)
                .is_some_and(|nodes| find_node_readonly(nodes, id).is_some())
            {
                ensure!(
                    layer.get("locked").and_then(Value::as_bool) != Some(true),
                    "[locked] layer `{}` is locked",
                    layer
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("<unknown>")
                );
                return Ok(());
            }
        }
    }
    bail!("[missing-resource] no scene3d node `{id}`")
}

fn scene3d_node_on_page(raw: &Value, page_id: &str, id: &str) -> Result<Map<String, Value>> {
    let page = raw
        .get("pages")
        .and_then(Value::as_array)
        .and_then(|pages| pages.iter().find(|page| page["id"] == page_id))
        .with_context(|| format!("[missing-resource] page `{page_id}` does not exist"))?;
    let node = page
        .get("layers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|layer| layer.get("nodes").and_then(Value::as_array))
        .find_map(|nodes| find_node_readonly(nodes, id))
        .with_context(|| {
            format!("[missing-resource] node `{id}` does not exist on page `{page_id}`")
        })?;
    ensure!(
        node.get("kind").and_then(Value::as_str) == Some("scene3d"),
        "[invalid-input] node {id} on page {page_id} is not a scene3d node"
    );
    Ok(node
        .as_object()
        .context("[invalid-input] scene3d node must be an object")?
        .clone())
}

fn ensure_batch_not_cancelled(cancel_file: Option<&Path>) -> Result<()> {
    if let Some(path) = cancel_file {
        ensure!(
            !path.exists(),
            "[cancelled] batch cancelled because marker {} exists; no output directory was published",
            path.display()
        );
    }
    Ok(())
}

fn is_queue_item_failure(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    [
        "[missing-resource]",
        "[malformed-resource]",
        "[unsupported-capability]",
        "[render-failed]",
        "[invalid-input]",
    ]
    .iter()
    .any(|item_failure| message.starts_with(item_failure))
}

fn ensure_output_directory_unused(path: &Path, operation: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            bail!("[invalid-input] {operation} output path already exists; choose a new directory")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "[io-error] could not inspect {operation} output path {}",
                path.display()
            )
        }),
    }
}

fn batch_render(
    raw: &Value,
    input: &Path,
    out_dir: &Path,
    requested_nodes: &[String],
    document_revision: &str,
    dry_run: bool,
    cancel_file: Option<&Path>,
) -> Result<Value> {
    const MAX_NODES: usize = 32;
    const MAX_TOTAL_PIXELS: u64 = 64_000_000;
    const MAX_TOTAL_SAMPLES: u64 = 256 * 1024 * 1024;
    const MAX_TOTAL_PNG_BYTES: usize = 256 * 1024 * 1024;
    ensure!(
        raw.get("version").and_then(Value::as_u64) == Some(11),
        "[unsupported-capability] batch mockup rendering requires document format v11"
    );
    ensure!(
        !requested_nodes.is_empty() && requested_nodes.len() <= MAX_NODES,
        "[limit-exceeded] batch render requires 1..={MAX_NODES} --node values"
    );
    let selectors = requested_nodes
        .iter()
        .map(|selector| {
            if let Some((page, node)) = selector.split_once("::") {
                ensure!(
                    !page.trim().is_empty() && !node.trim().is_empty(),
                    "[invalid-input] qualified batch selector `{selector}` must be PAGE::ID"
                );
                Ok((Some(page.to_owned()), node.to_owned()))
            } else {
                ensure!(
                    !selector.trim().is_empty(),
                    "[invalid-input] batch node selector must be a nonempty ID or PAGE::ID"
                );
                Ok((None, selector.clone()))
            }
        })
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    ensure!(
        selectors.len() == requested_nodes.len(),
        "[invalid-input] batch render selectors must be unique"
    );
    ensure_output_directory_unused(out_dir, "batch render")?;
    let parent = out_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(
        parent.is_dir(),
        "[missing-resource] batch output parent {} is not a directory",
        parent.display()
    );
    out_dir
        .file_name()
        .context("[invalid-input] batch output must name a new directory")?;
    ensure_batch_not_cancelled(cancel_file)?;

    crate::scene::validate(raw)?;
    let plan = super::bindings::plan_report(raw)?;
    let render_order = plan["render_order"]
        .as_array()
        .context("[invalid-input] dependency planner returned no render order")?;
    for (page, node) in &selectors {
        if page.is_none() {
            let matches = render_order
                .iter()
                .filter(|reference| reference.get("node").and_then(Value::as_str) == Some(node))
                .count();
            ensure!(matches <= 1,
                "[invalid-input] batch node ID `{node}` is ambiguous across pages; use PAGE::{node}");
        }
    }
    let document_dir = input
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut rendered = Vec::<(String, Vec<u8>, String, String, Value)>::new();
    let mut found = std::collections::BTreeSet::<(String, String)>::new();
    let mut total_pixels = 0u64;
    let mut total_bytes = 0usize;
    let mut filenames = std::collections::BTreeSet::<String>::new();

    // Predict dimensions from the validated node settings and reject a
    // too-large aggregate before spending time on any 3D render.
    let mut planned_nodes = std::collections::BTreeSet::<(String, String)>::new();
    let mut planned_pixels = 0u64;
    let mut planned_samples = 0u64;
    for reference in render_order {
        let Some(node_id) = reference.get("node").and_then(Value::as_str) else {
            continue;
        };
        let page_id = reference
            .get("page")
            .and_then(Value::as_str)
            .context("[invalid-input] dependency planner returned a node without a page")?;
        if !selectors.iter().any(|(page, node)| {
            node == node_id && page.as_deref().is_none_or(|page| page == page_id)
        }) {
            continue;
        }
        let identity = (page_id.to_owned(), node_id.to_owned());
        ensure!(
            planned_nodes.insert(identity),
            "[invalid-input] dependency planner returned duplicate node {page_id}/{node_id}"
        );
        let node = scene3d_node_on_page(raw, page_id, node_id)?;
        let ratio = node
            .get("pixel_ratio")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let width = (node["width"].as_f64().context("scene3d width missing")? * ratio)
            .round()
            .max(1.0) as u64;
        let height = (node["height"].as_f64().context("scene3d height missing")? * ratio)
            .round()
            .max(1.0) as u64;
        planned_pixels = planned_pixels
            .checked_add(
                width
                    .checked_mul(height)
                    .context("[limit-exceeded] batch pixel count overflow")?,
            )
            .context("[limit-exceeded] batch pixel count overflow")?;
        ensure!(
            planned_pixels <= MAX_TOTAL_PIXELS,
            "[limit-exceeded] batch render exceeds {MAX_TOTAL_PIXELS} total pixels"
        );
        let supersample = node.get("supersample").and_then(Value::as_u64).unwrap_or(2);
        let samples = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(supersample.saturating_mul(supersample)))
            .context("[limit-exceeded] batch sample-work count overflow")?;
        planned_samples = planned_samples
            .checked_add(samples)
            .context("[limit-exceeded] batch sample-work count overflow")?;
        ensure!(
            planned_samples <= MAX_TOTAL_SAMPLES,
            "[limit-exceeded] batch render exceeds {MAX_TOTAL_SAMPLES} supersampled samples"
        );
    }
    ensure!(
        planned_nodes.len() == selectors.len(),
        "[missing-resource] batch node IDs not found: {}",
        requested_nodes
            .iter()
            .filter(|selector| {
                let (page, node) = if let Some((page, node)) = selector.split_once("::") {
                    (Some(page), node)
                } else {
                    (None, selector.as_str())
                };
                !planned_nodes.iter().any(|(found_page, found_node)| {
                    found_node == node && page.is_none_or(|page| found_page == page)
                })
            })
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );

    for (index, reference) in render_order.iter().enumerate() {
        let Some(node_id) = reference.get("node").and_then(Value::as_str) else {
            continue;
        };
        let page_id = reference
            .get("page")
            .and_then(Value::as_str)
            .context("[invalid-input] dependency planner returned a node without a page")?;
        if !selectors.iter().any(|(page, node)| {
            node == node_id && page.as_deref().is_none_or(|page| page == page_id)
        }) {
            continue;
        }
        ensure_batch_not_cancelled(cancel_file)?;
        let identity = (page_id.to_owned(), node_id.to_owned());
        ensure!(
            found.insert(identity),
            "[invalid-input] dependency planner returned duplicate node {page_id}/{node_id}"
        );
        let node = scene3d_node_on_page(raw, page_id, node_id)?;
        let (png, report) =
            super::node::render_png_report_in_page(raw, document_dir, Some(page_id), &node)?;
        let width = report["output"]["width_px"]
            .as_u64()
            .context("[render-failed] renderer report has no output width")?;
        let height = report["output"]["height_px"]
            .as_u64()
            .context("[render-failed] renderer report has no output height")?;
        let pixels = width
            .checked_mul(height)
            .context("[limit-exceeded] batch pixel count overflow")?;
        total_pixels = total_pixels
            .checked_add(pixels)
            .context("[limit-exceeded] batch pixel count overflow")?;
        ensure!(
            total_pixels <= MAX_TOTAL_PIXELS,
            "[limit-exceeded] batch render exceeds {MAX_TOTAL_PIXELS} total pixels"
        );
        total_bytes = total_bytes
            .checked_add(png.len())
            .context("[limit-exceeded] batch byte count overflow")?;
        ensure!(
            total_bytes <= MAX_TOTAL_PNG_BYTES,
            "[limit-exceeded] batch render exceeds {MAX_TOTAL_PNG_BYTES} total PNG bytes"
        );
        let identity = format!("{page_id}/{node_id}");
        let digest = crate::resource::sha256(identity.as_bytes());
        let filename = format!("{:02}-{}.png", index + 1, &digest[7..23]);
        ensure!(
            filenames.insert(filename.clone()),
            "[invalid-input] batch render output filename collision for `{filename}`"
        );
        rendered.push((
            filename,
            png,
            page_id.to_owned(),
            node_id.to_owned(),
            report,
        ));
    }
    ensure!(
        found.len() == selectors.len(),
        "[missing-resource] batch node IDs not found: {}",
        requested_nodes
            .iter()
            .filter(|selector| {
                let (page, node) = if let Some((page, node)) = selector.split_once("::") {
                    (Some(page), node)
                } else {
                    (None, selector.as_str())
                };
                !found.iter().any(|(found_page, found_node)| {
                    found_node == node && page.is_none_or(|page| found_page == page)
                })
            })
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Publish only after all render work succeeds. The destination must be a
    // new directory; a same-parent rename makes the batch visible at once.
    ensure_batch_not_cancelled(cancel_file)?;
    if !dry_run {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_nanos();
        let staging = parent.join(format!(".pentool-batch-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&staging).with_context(|| {
            format!(
                "[io-error] could not create batch staging directory {}",
                staging.display()
            )
        })?;
        let publish = (|| -> Result<()> {
            for (filename, png, _, _, _) in &rendered {
                std::fs::write(staging.join(filename), png)?;
            }
            std::fs::rename(&staging, out_dir).with_context(|| {
                format!(
                    "[io-error] could not atomically publish batch to {}",
                    out_dir.display()
                )
            })?;
            Ok(())
        })();
        if publish.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        publish?;
    }

    let items = rendered
        .iter()
        .map(|(filename, png, page_id, node_id, report)| {
            json!({
                "page":page_id,
                "node":node_id,
                "file":out_dir.join(filename),
                "sha256":crate::resource::sha256(png),
                "bytes":png.len(),
                "render":report,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "format":"pentool-scene3d-batch-report-v1",
        "document_revision":document_revision,
        "output_dir":out_dir,
        "summary":{"requested":requested_nodes.len(),"rendered":items.len(),"total_pixels":total_pixels,"estimated_samples":planned_samples,"total_png_bytes":total_bytes,"complete":!dry_run,"dry_run":dry_run},
        "items":items,
    }))
}

struct BatchQueueStaging {
    path: PathBuf,
}

impl Drop for BatchQueueStaging {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn queue_job_preflight(raw: &Value, requested_nodes: &[String]) -> Result<(usize, u64, u64)> {
    const MAX_NODES: usize = 32;
    const MAX_PIXELS: u64 = 64_000_000;
    const MAX_SAMPLES: u64 = 256 * 1024 * 1024;
    ensure!(
        raw.get("version").and_then(Value::as_u64) == Some(11),
        "[unsupported-capability] batch mockup rendering requires document format v11"
    );
    ensure!(
        !requested_nodes.is_empty() && requested_nodes.len() <= MAX_NODES,
        "[limit-exceeded] each queue job requires 1..={MAX_NODES} --node selectors"
    );
    let selectors = requested_nodes
        .iter()
        .map(|selector| {
            if let Some((page, node)) = selector.split_once("::") {
                ensure!(
                    !page.trim().is_empty() && !node.trim().is_empty(),
                    "[invalid-input] qualified batch selector `{selector}` must be PAGE::ID"
                );
                Ok((Some(page.to_owned()), node.to_owned()))
            } else {
                ensure!(
                    !selector.trim().is_empty(),
                    "[invalid-input] batch node selector must be a nonempty ID or PAGE::ID"
                );
                Ok((None, selector.clone()))
            }
        })
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    ensure!(
        selectors.len() == requested_nodes.len(),
        "[invalid-input] batch render selectors must be unique"
    );
    crate::scene::validate(raw)?;
    let plan = super::bindings::plan_report(raw)?;
    let render_order = plan["render_order"]
        .as_array()
        .context("[invalid-input] dependency planner returned no render order")?;
    for (page, node) in &selectors {
        if page.is_none() {
            let matches = render_order
                .iter()
                .filter(|reference| reference.get("node").and_then(Value::as_str) == Some(node))
                .count();
            ensure!(matches <= 1,
                "[invalid-input] batch node ID `{node}` is ambiguous across pages; use PAGE::{node}");
        }
    }
    let mut found = std::collections::BTreeSet::new();
    let mut pixels = 0u64;
    let mut samples = 0u64;
    for reference in render_order {
        let (Some(page), Some(node)) = (
            reference.get("page").and_then(Value::as_str),
            reference.get("node").and_then(Value::as_str),
        ) else {
            continue;
        };
        if !selectors.iter().any(|(selector_page, selector_node)| {
            selector_node == node && selector_page.as_deref().is_none_or(|p| p == page)
        }) {
            continue;
        }
        ensure!(
            found.insert((page.to_owned(), node.to_owned())),
            "[invalid-input] dependency planner returned duplicate node {page}/{node}"
        );
        let node_value = scene3d_node_on_page(raw, page, node)?;
        let ratio = node_value
            .get("pixel_ratio")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let width = (node_value["width"]
            .as_f64()
            .context("scene3d width missing")?
            * ratio)
            .round()
            .max(1.0) as u64;
        let height = (node_value["height"]
            .as_f64()
            .context("scene3d height missing")?
            * ratio)
            .round()
            .max(1.0) as u64;
        pixels = pixels
            .checked_add(
                width
                    .checked_mul(height)
                    .context("[limit-exceeded] batch pixel count overflow")?,
            )
            .context("[limit-exceeded] batch pixel count overflow")?;
        ensure!(
            pixels <= MAX_PIXELS,
            "[limit-exceeded] batch render exceeds {MAX_PIXELS} total pixels"
        );
        let supersample = node_value
            .get("supersample")
            .and_then(Value::as_u64)
            .unwrap_or(2);
        let node_samples = width
            .checked_mul(height)
            .and_then(|value| value.checked_mul(supersample.saturating_mul(supersample)))
            .context("[limit-exceeded] batch sample-work count overflow")?;
        samples = samples
            .checked_add(node_samples)
            .context("[limit-exceeded] batch sample-work count overflow")?;
        ensure!(
            samples <= MAX_SAMPLES,
            "[limit-exceeded] batch render exceeds {MAX_SAMPLES} supersampled samples"
        );
    }
    ensure!(
        found.len() == selectors.len(),
        "[missing-resource] batch selectors not found: {}",
        requested_nodes
            .iter()
            .filter(|selector| {
                let (page, node) = selector
                    .split_once("::")
                    .map_or((None, selector.as_str()), |(p, n)| (Some(p), n));
                !found.iter().any(|(found_page, found_node)| {
                    found_node == node && page.is_none_or(|p| p == found_page)
                })
            })
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok((found.len(), pixels, samples))
}

fn batch_queue(
    manifest: &Value,
    manifest_path: &Path,
    out_dir: &Path,
    dry_run: bool,
    cancel_file: Option<&Path>,
) -> Result<Value> {
    const MAX_JOBS: usize = 8;
    const MAX_ITEMS: usize = 32;
    const MAX_PIXELS: u64 = 64_000_000;
    const MAX_SAMPLES: u64 = 256 * 1024 * 1024;
    const MAX_PNG_BYTES: u64 = 256 * 1024 * 1024;
    ensure!(
        manifest.is_object(),
        "[invalid-input] batch queue manifest must be an object"
    );
    let object = manifest.as_object().unwrap();
    ensure!(
        object
            .keys()
            .all(|key| matches!(key.as_str(), "format" | "jobs")),
        "[invalid-input] batch queue manifest accepts only format and jobs"
    );
    ensure!(
        manifest["format"].as_str() == Some("pentool-scene3d-batch-queue-v1"),
        "[invalid-input] batch queue format must be pentool-scene3d-batch-queue-v1"
    );
    let jobs = manifest["jobs"]
        .as_array()
        .context("[invalid-input] batch queue jobs must be an array")?;
    ensure!(
        !jobs.is_empty() && jobs.len() <= MAX_JOBS,
        "[limit-exceeded] batch queue requires 1..={MAX_JOBS} jobs"
    );
    ensure_output_directory_unused(out_dir, "batch queue")?;
    let parent = out_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(
        parent.is_dir(),
        "[missing-resource] batch queue output parent {} is not a directory",
        parent.display()
    );
    out_dir
        .file_name()
        .context("[invalid-input] batch queue output must name a new directory")?;
    ensure_batch_not_cancelled(cancel_file)?;

    let manifest_dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut prepared = Vec::with_capacity(jobs.len());
    let mut total_items = 0usize;
    let mut total_pixels = 0u64;
    let mut total_samples = 0u64;
    for (index, job) in jobs.iter().enumerate() {
        let job = job.as_object().with_context(|| {
            format!("[invalid-input] batch queue job {index} must be an object")
        })?;
        ensure!(
            job.keys()
                .all(|key| matches!(key.as_str(), "document" | "nodes")),
            "[invalid-input] batch queue job {index} accepts only document and nodes"
        );
        let document = job
            .get("document")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .with_context(|| {
                format!("[invalid-input] batch queue job {index} needs a document path")
            })?;
        let mut document_path = PathBuf::from(document);
        if document_path.is_relative() {
            document_path = manifest_dir.join(document_path);
        }
        let selectors = job
            .get("nodes")
            .and_then(Value::as_array)
            .with_context(|| {
                format!("[invalid-input] batch queue job {index} nodes must be an array")
            })?
            .iter()
            .map(|node| {
                node.as_str().map(str::to_owned).with_context(|| {
                    format!("[invalid-input] batch queue job {index} selectors must be strings")
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let document_bytes = std::fs::read(&document_path).with_context(|| {
            format!(
                "[missing-resource] could not read {}",
                document_path.display()
            )
        })?;
        let raw: Value = serde_json::from_slice(&document_bytes)
            .context("[malformed-resource] invalid .pen document")?;
        let revision = crate::transaction::revision(&document_bytes);
        let (count, pixels, samples) = queue_job_preflight(&raw, &selectors)
            .with_context(|| format!("batch queue job {index} ({document})"))?;
        total_items = total_items
            .checked_add(count)
            .context("[limit-exceeded] batch queue item count overflow")?;
        total_pixels = total_pixels
            .checked_add(pixels)
            .context("[limit-exceeded] batch queue pixel count overflow")?;
        ensure!(
            total_items <= MAX_ITEMS,
            "[limit-exceeded] batch queue exceeds {MAX_ITEMS} total mockups"
        );
        ensure!(
            total_pixels <= MAX_PIXELS,
            "[limit-exceeded] batch queue exceeds {MAX_PIXELS} total pixels"
        );
        total_samples = total_samples
            .checked_add(samples)
            .context("[limit-exceeded] batch queue sample-work count overflow")?;
        ensure!(
            total_samples <= MAX_SAMPLES,
            "[limit-exceeded] batch queue exceeds {MAX_SAMPLES} supersampled samples"
        );
        // Five bytes per output pixel is a conservative preflight estimate for
        // RGBA plus PNG filter/deflate overhead, preventing oversized queues
        // from rendering before the aggregate byte cap is known.
        let estimated_bytes = total_pixels
            .checked_mul(5)
            .context("[limit-exceeded] batch queue PNG byte estimate overflow")?;
        ensure!(
            estimated_bytes <= MAX_PNG_BYTES,
            "[limit-exceeded] batch queue estimated PNG data exceeds {MAX_PNG_BYTES} bytes"
        );
        prepared.push((document_path, selectors, pixels, samples, revision));
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    let staging_path = parent.join(format!(
        ".pentool-batch-queue-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&staging_path).with_context(|| {
        format!(
            "[io-error] could not create batch queue staging directory {}",
            staging_path.display()
        )
    })?;
    let staging = BatchQueueStaging {
        path: staging_path.clone(),
    };
    let mut total_bytes = 0u64;
    let mut queue_items = Vec::with_capacity(prepared.len());
    for (index, (document_path, selectors, job_pixels, job_samples, preflight_revision)) in
        prepared.into_iter().enumerate()
    {
        ensure_batch_not_cancelled(cancel_file)?;
        let document_bytes = std::fs::read(&document_path)?;
        let revision = crate::transaction::revision(&document_bytes);
        ensure!(
            revision == preflight_revision,
            "[revision-conflict] batch queue document {} changed after preflight; rerun the queue",
            document_path.display()
        );
        let raw: Value = serde_json::from_slice(&document_bytes)
            .context("[malformed-resource] invalid .pen document")?;
        let folder_hash = crate::resource::sha256(format!("{index}:{revision}").as_bytes());
        let folder = format!("{:02}-{}", index + 1, &folder_hash[7..19]);
        let stage_job_dir = staging_path.join(&folder);
        let mut items = Vec::with_capacity(selectors.len());
        std::fs::create_dir(&stage_job_dir).with_context(|| {
            format!(
                "[io-error] could not create queue job staging directory {}",
                stage_job_dir.display()
            )
        })?;
        for (item_index, selector) in selectors.iter().enumerate() {
            ensure_batch_not_cancelled(cancel_file)?;
            let item_dir = stage_job_dir.join(format!("item-{:02}", item_index + 1));
            match batch_render(
                &raw,
                &document_path,
                &item_dir,
                std::slice::from_ref(selector),
                &revision,
                dry_run,
                cancel_file,
            ) {
                Ok(report) => {
                    let bytes = report["summary"]["total_png_bytes"]
                        .as_u64()
                        .context("[render-failed] batch report has no byte total")?;
                    total_bytes = total_bytes
                        .checked_add(bytes)
                        .context("[limit-exceeded] batch queue byte count overflow")?;
                    ensure!(
                        total_bytes <= MAX_PNG_BYTES,
                        "[limit-exceeded] batch queue exceeds {MAX_PNG_BYTES} total PNG bytes"
                    );
                    let item = report["items"]
                        .as_array()
                        .and_then(|items| items.first())
                        .context("[render-failed] batch report has no item")?;
                    let file_name = item["file"]
                        .as_str()
                        .and_then(|path| Path::new(path).file_name())
                        .context("[render-failed] batch item has no output filename")?;
                    items.push(json!({
                        "status":"rendered",
                        "page":item["page"], "node":item["node"],
                        "file":out_dir.join(&folder).join(format!("item-{:02}", item_index + 1)).join(file_name),
                        "sha256":item["sha256"], "bytes":item["bytes"], "render":item["render"]
                    }));
                }
                Err(error) if is_queue_item_failure(&error) => {
                    items.push(json!({
                        "status":"failed",
                        "selector":selector,
                        "error":error.to_string()
                    }));
                }
                Err(error) => return Err(error),
            }
        }
        queue_items.push(json!({
            "document":document_path,
            "document_revision":revision,
            "planned_pixels":job_pixels,
            "estimated_samples":job_samples,
            "items":items,
        }));
    }
    ensure_batch_not_cancelled(cancel_file)?;
    if !dry_run {
        std::fs::rename(&staging_path, out_dir).with_context(|| {
            format!(
                "[io-error] could not atomically publish batch queue to {}",
                out_dir.display()
            )
        })?;
    }
    // `staging` removes the private staging tree on dry run or any failure;
    // after successful rename the old staging path no longer exists.
    drop(staging);
    let rendered_count = queue_items
        .iter()
        .flat_map(|job| job["items"].as_array().into_iter().flatten())
        .filter(|item| item["status"] == "rendered")
        .count();
    let failed_count = queue_items
        .iter()
        .flat_map(|job| job["items"].as_array().into_iter().flatten())
        .filter(|item| item["status"] == "failed")
        .count();
    Ok(json!({
        "format":"pentool-scene3d-batch-queue-report-v1",
        "output_dir":out_dir,
        "summary":{"jobs":queue_items.len(),"requested":total_items,
            "rendered":rendered_count,"failed":failed_count,
            "total_pixels":total_pixels,"estimated_samples":total_samples,
            "total_png_bytes":total_bytes,"complete":!dry_run && failed_count == 0,
            "partial":!dry_run && rendered_count > 0 && failed_count > 0,"dry_run":dry_run},
        "jobs":queue_items,
    }))
}

pub(crate) struct PackBundleOptions<'a> {
    pub(crate) dieline_id: &'a str,
    pub(crate) out: &'a Path,
    pub(crate) min_ppi: f64,
    pub(crate) ppi: f64,
    pub(crate) bits: u8,
    pub(crate) fold: f64,
    pub(crate) source_revision: &'a str,
    pub(crate) dry_run: bool,
}

pub(crate) fn write_pack_bundle(
    raw: &Value,
    input: &Path,
    options: PackBundleOptions<'_>,
) -> Result<Value> {
    let PackBundleOptions {
        dieline_id,
        out,
        min_ppi,
        ppi,
        bits,
        fold,
        source_revision,
        dry_run,
    } = options;
    ensure!(
        bits == 8 || bits == 16,
        "[invalid-input] spot-plate --bits must be 8 or 16"
    );
    ensure!(
        fold.is_finite() && (0.0..=1.0).contains(&fold),
        "[invalid-input] bundle --fold must be finite and within 0..=1"
    );
    ensure!(
        !out.exists(),
        "[output-exists] packaging bundle directory {} already exists",
        out.display()
    );
    let parent = out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(
        parent.is_dir(),
        "[missing-resource] packaging bundle parent {} is not a directory",
        parent.display()
    );
    let name = out
        .file_name()
        .context("[invalid-input] packaging bundle output needs a directory name")?;
    let output_path = parent.canonicalize()?.join(name);
    ensure!(
        !output_path.exists(),
        "[output-exists] packaging bundle directory {} already exists",
        output_path.display()
    );
    crate::scene::validate(raw)?;

    let flat = super::pack::flat_dieline_svg_with_artwork(raw, input, dieline_id, min_ppi)?;
    let (pdf, pdf_flat) =
        super::pack::flat_dieline_pdfx_with_artwork(raw, input, dieline_id, min_ppi, ppi)?;
    ensure!(
        flat.svg == pdf_flat.svg,
        "[render-failed] PDF/X and flat SVG were not derived from the same dieline plan"
    );
    let mut preview_document = raw.clone();
    let dieline = preview_document
        .get_mut("dielines")
        .and_then(|catalog| catalog.get_mut(dieline_id))
        .with_context(|| format!("[missing-resource] dieline `{dieline_id}` does not exist"))?;
    dieline["fold_state"] = json!(fold);
    let node_id = dieline
        .get("scene3d_node")
        .and_then(Value::as_str)
        .context("[missing-resource] dieline has no linked scene3d preview node")?
        .to_owned();
    let (page_id, preview_node) = scene3d_node_with_page(&preview_document, &node_id)?;
    let document_dir = input
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (preview_png, preview_report) = super::node::render_png_report_in_page(
        &preview_document,
        document_dir,
        Some(&page_id),
        &preview_node,
    )?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    let staging = parent.join(format!(
        ".pentool-pack-bundle-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&staging).with_context(|| {
        format!(
            "[io-error] could not create packaging bundle staging directory {}",
            staging.display()
        )
    })?;
    let result = (|| -> Result<Value> {
        crate::editing::atomic_write(&staging.join("flat.svg"), flat.svg.as_bytes())?;
        // Human-facing review sheet only. Machine finishing geometry remains
        // in the unchanged, scale-sensitive vector spot plates.
        let root = flat
            .svg
            .split_once("<svg ")
            .and_then(|(_, rest)| rest.split_once('>').map(|(attrs, _)| attrs))
            .context("[internal] flat packaging SVG has no root")?;
        let dim = |name: &str| -> Result<f64> {
            root.split_once(&format!("{name}=\""))
                .and_then(|(_, tail)| tail.split_once('"').map(|(v, _)| v))
                .and_then(|v| v.strip_suffix("mm"))
                .context("[internal] flat packaging SVG dimensions are invalid")?
                .parse::<f64>()
                .context("[internal] flat packaging SVG dimensions are invalid")
        };
        let map_width_mm = dim("width")?;
        let map_height_mm = dim("height")?;
        let map_height_with_legend = map_height_mm + 48.0;
        let mut cutting_map = flat.svg.clone();
        cutting_map = cutting_map.replacen(
            &format!("height=\"{map_height_mm}mm\""),
            &format!("height=\"{map_height_with_legend}mm\""),
            1,
        );
        cutting_map = cutting_map.replacen(
            &format!("viewBox=\"0 0 {map_width_mm} {map_height_mm}\""),
            &format!("viewBox=\"0 0 {map_width_mm} {map_height_with_legend}\""),
            1,
        );
        let legend = format!("<g id=\"cutting-map-legend\" data-reference-only=\"true\" transform=\"translate(2 {})\" font-family=\"sans-serif\" font-size=\"2.5\"><path d=\"M 0 0 h 8\" stroke=\"#dc2626\" stroke-width=\"0.6\"/><text x=\"10\" y=\"0.9\">CUT · through-cut (red)</text><path d=\"M 0 5 h 8\" stroke=\"#2563eb\" stroke-width=\"0.6\"/><text x=\"10\" y=\"5.9\">CREASE · fold/score, not a cut (blue)</text><path d=\"M 0 10 h 8\" stroke=\"#2563eb\" stroke-width=\"0.6\"/><text x=\"10\" y=\"10.9\">Valley fold · blue solid</text><path d=\"M 0 15 h 8\" stroke=\"#7c3aed\" stroke-width=\"0.6\" stroke-dasharray=\"1 0.7\"/><text x=\"10\" y=\"15.9\">Mountain fold · violet dashed</text><path d=\"M 0 20 h 8\" stroke=\"#7c3aed\" stroke-width=\"0.6\"/><text x=\"10\" y=\"20.9\">PERFORATION · tear line (purple)</text><path d=\"M 0 25 h 8\" stroke=\"#f97316\" stroke-width=\"0.6\" stroke-dasharray=\"1 1\"/><text x=\"10\" y=\"25.9\">BLEED · artwork extension beyond trim (orange)</text><path d=\"M 0 30 h 8\" stroke=\"#16a34a\" stroke-width=\"0.6\" stroke-dasharray=\"1 1\"/><text x=\"10\" y=\"30.9\">SAFE ZONE · keep important artwork inside (green)</text><path d=\"M 0 35 h 8\" stroke=\"#0891b2\" stroke-width=\"0.6\" stroke-dasharray=\"1 1\"/><text x=\"10\" y=\"35.9\">GLUE FLAP · adhesive area; keep critical art clear (cyan)</text><text x=\"0\" y=\"42\" font-size=\"2.2\">Visual reference only. Use vector-spot-plates/ for finishing geometry; check folded-preview.png for assembled appearance.</text></g>", map_height_mm + 3.0);
        let close = cutting_map
            .rfind("</svg>")
            .context("[internal] flat packaging SVG has no closing root")?;
        cutting_map.insert_str(close, &legend);
        crate::editing::atomic_write(&staging.join("CUTTING-MAP.svg"), cutting_map.as_bytes())?;
        crate::editing::atomic_write(&staging.join("print-draft.pdf"), &pdf.bytes)?;
        crate::editing::atomic_write(&staging.join("folded-preview.png"), &preview_png)?;
        let mut print_guide = format!(
            "Pentool packaging print bundle\n\nFlat artwork media size: {map_width_mm:.3} mm × {map_height_mm:.3} mm (includes configured bleed).\nRaster finishing plates: {ppi} PPI. Linked-art resolution warning target: {min_ppi:.0} PPI.\n\n"
        );
        print_guide.push_str(
            "Use these files together; the PDF alone is not a cutting file. Open
CUTTING-MAP.svg for a color-keyed visual map (red cut, solid blue valley fold,
dashed violet mountain fold, purple perforation, orange bleed, green safe zone,
cyan glue flap); use
vector-spot-plates/ for actual finishing geometry.\n\n\
1. Check folded-preview.png first. It shows the package assembled at the\n\
   selected fold amount. Inspect all faces, seams, artwork orientation, and\n\
   glue areas before sending the flat design to print.\n\
 2. Review flat.svg at its physical millimeter size for panel artwork, trim\n\
    outline, bleed, and fold placement. When printing the PDF proof, use 100%\n\
    scale (actual size); turn off fit-to-page or any other scaling. The flat\n\
    SVG and finishing plates are the scale-sensitive handoff files.\n\
3. Use print-draft.pdf for the flattened PDF/X-4 process-CMYK artwork proof. It does\n\
   not contain spot plates or machine cut instructions.\n\
4. Give vector-spot-plates/ to the finishing workflow as separate vector\n\
   operations: CutContour = through-cut paths (where material is cut out),\n\
   Crease = fold/scoring lines (not through-cuts), and Perforation = tear\n\
   lines. The reference map distinguishes blue solid valley folds from violet\n\
   dashed mountain folds; confirm direction with the assembly preview. SafeZone\n\
   and GlueFlap are review guides; do not treat them as cuts.\n\
   The matching grayscale PNG plates in separations/ are raster separations,\n\
   not substitutes for vector cutter geometry.\n\
5. Ask the print/cutting provider to confirm spot-plate mapping, page scale,\n\
   bleed, board/material thickness, glue allowance, cut direction, and cutter\n\
   settings. Do not infer a cut from artwork edges or the PDF proof.\n\n\
This is a design handoff, not a manufacturing-certified package. Structural\n\
folding, material behavior, and cutter conformance are not certified; verify\n\
    the dieline and assembly with your production provider before making copies.\n",
        );
        crate::editing::atomic_write(&staging.join("PRINT-GUIDE.txt"), print_guide.as_bytes())?;
        let mut separations = super::pack::write_dieline_spot_separations(
            raw,
            input,
            dieline_id,
            &staging.join("separations"),
            ppi,
            bits,
            source_revision,
        )?;
        separations["out"] = json!("separations");
        let vector_plates = super::pack::write_dieline_vector_spot_plates(
            raw,
            dieline_id,
            &staging.join("vector-spot-plates"),
        )?;
        let files = json!({
            "flat_svg":{"path":"flat.svg","sha256":crate::resource::sha256(flat.svg.as_bytes()),"bytes":flat.svg.len(),"report":{"artwork_panels":flat.artwork_panels,"rasterized_panels":flat.rasterized_panels,"rasterized_ppi":flat.rasterized_ppi,"bleed_synthesized":flat.bleed_synthesized,"warnings":flat.warnings}},
            "cutting_map":{"path":"CUTTING-MAP.svg","sha256":crate::resource::sha256(cutting_map.as_bytes()),"bytes":cutting_map.len(),"visual_reference_only":true},
            "pdfx_draft":{"path":"print-draft.pdf","sha256":crate::resource::sha256(&pdf.bytes),"bytes":pdf.bytes.len(),"report":pdf.report},
            "folded_preview":{"path":"folded-preview.png","sha256":crate::resource::sha256(&preview_png),"bytes":preview_png.len(),"render":preview_report},
            "print_guide":{"path":"PRINT-GUIDE.txt","sha256":crate::resource::sha256(print_guide.as_bytes()),"bytes":print_guide.len()},
            "spot_separations":{"path":"separations/","report":separations},
            "vector_spot_plates":{"path":"vector-spot-plates/","report":vector_plates},
        });
        let manifest = json!({
            "format":"pentool-packaging-bundle-v1",
            "dieline":dieline_id,
            "document_revision":source_revision,
            "fold_preview":fold,
            "ppi":ppi,
            "minimum_artwork_ppi":min_ppi,
            "spot_plate_bits":bits,
            "files":files,
            "manufacturing_certified":false,
            "limitations":["PDF/X is a flattened process-CMYK draft","vector spot plates and grayscale PNG plates are separate from the PDF/X draft","structural and manufacturing conformance is not certified"],
        });
        crate::editing::atomic_write(
            &staging.join("manifest.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        let report = json!({
            "format":"pentool-packaging-bundle-v1",
            "dieline":dieline_id,
            "out":out,
            "manifest":"manifest.json",
            "document_revision":source_revision,
            "files":files,
            "manufacturing_certified":false,
            "dry_run":dry_run,
            "complete":!dry_run,
        });
        if dry_run {
            std::fs::remove_dir_all(&staging).with_context(|| {
                format!(
                    "[io-error] could not remove dry-run bundle staging directory {}",
                    staging.display()
                )
            })?;
            return Ok(report);
        }
        ensure!(
            !output_path.exists(),
            "[output-exists] packaging bundle directory {} already exists",
            output_path.display()
        );
        std::fs::rename(&staging, &output_path).with_context(|| {
            format!(
                "[io-error] could not atomically publish packaging bundle at {}",
                output_path.display()
            )
        })?;
        Ok(report)
    })();
    if result.is_err() && staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
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

pub(crate) fn add(
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
    if external {
        let collected = node::collect_external_model(raw, &doc_dir, &digest)?;
        if let Some(resources) = collected.get("resources").and_then(Value::as_object) {
            let manifest: Map<String, Value> = resources
                .iter()
                .map(|(path, resource)| {
                    let mut entry = resource.clone();
                    entry.as_object_mut().unwrap().remove("data");
                    (path.clone(), entry)
                })
                .collect();
            raw["models"][&digest]["storage"]["resources"] = Value::Object(manifest);
        }
    }
    let report = node::inspect(&model, 0, 256);

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
    let document_bytes = std::fs::read(input)
        .with_context(|| format!("[missing-resource] could not read {}", input.display()))?;
    let current_revision = crate::transaction::revision(&document_bytes);
    if let Some(expected) = revision {
        ensure!(
            expected == current_revision,
            "revision mismatch: expected {expected:?}, current {current_revision}"
        );
    }
    let mut raw = read(input)?;
    if let Scene3dAction::Camera { id, .. }
    | Scene3dAction::Frame { id, .. }
    | Scene3dAction::Light { id, .. }
    | Scene3dAction::Environment { id, .. }
    | Scene3dAction::Set { id, .. } = &action
    {
        ensure_scene3d_layer_unlocked(&raw, id)?;
    }
    match &action {
        Scene3dAction::Dependencies => {
            require_v11(&raw)?;
            println!("{}", super::bindings::plan_report(&raw)?);
            return Ok(());
        }
        Scene3dAction::RenderReport { id } => {
            crate::scene::validate(&raw)?;
            let (page_id, node) = scene3d_node_with_page(&raw, id)?;
            let doc_dir = input
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let (_, report) =
                super::node::render_png_report_in_page(&raw, doc_dir, Some(&page_id), &node)?;
            println!(
                "{}",
                json!({"node":id,"page":page_id,"document_revision":current_revision,"render":report})
            );
            return Ok(());
        }
        Scene3dAction::Batch {
            nodes,
            out_dir,
            cancel_file,
        } => {
            let report = batch_render(
                &raw,
                input,
                out_dir,
                nodes,
                &current_revision,
                dry_run,
                cancel_file.as_deref(),
            )?;
            println!("{report}");
            return Ok(());
        }
        Scene3dAction::BatchQueue {
            out_dir,
            cancel_file,
        } => {
            let report = batch_queue(&raw, input, out_dir, dry_run, cancel_file.as_deref())?;
            println!("{report}");
            return Ok(());
        }
        Scene3dAction::Form {
            action: FormAction::Presets,
        } => {
            println!(
                "{}",
                json!({
                    "source":"Pentool bounded procedural generators",
                    "asset_license":"No third-party model assets; geometry is generated from Pentool code",
                    "presets":super::form::starter_presets()
                })
            );
            return Ok(());
        }
        Scene3dAction::Pack {
            action: PackAction::Templates,
        } => {
            print_pack_templates();
            return Ok(());
        }
        Scene3dAction::Pack {
            action: PackAction::Inspect { dieline },
        } => {
            crate::scene::validate(&raw)?;
            let value = raw
                .get("dielines")
                .and_then(|catalog| catalog.get(dieline))
                .with_context(|| {
                    format!("[missing-resource] dieline `{dieline}` does not exist")
                })?;
            let panels = value["panels"]
                .as_array()
                .context("[invalid-input] dieline panels must be an array")?;
            let creases = value["creases"].as_array().map_or(&[][..], Vec::as_slice);
            let paths = value["paths"].as_array().map_or(&[][..], Vec::as_slice);
            let panel_report: Vec<Value> = panels
                .iter()
                .map(|panel| {
                    json!({
                        "id":panel["id"],
                        "rect_mm":panel["rect_mm"],
                        "face_slot":panel["face_slot"],
                        "uv_rect":panel["uv_rect"],
                        "artwork":panel["artwork"],
                        "artwork_transform":panel["artwork_transform"],
                        "safe_zone_mm":panel["safe_zone_mm"]
                    })
                })
                .collect();
            let crease_report: Vec<Value> = creases
                .iter()
                .map(|crease| {
                    json!({
                        "id":crease["id"],
                        "line_mm":crease["line_mm"],
                        "angle_deg":crease["angle_deg"],
                        "direction":crease["direction"]
                    })
                })
                .collect();
            let path_report: Vec<Value> = paths
                .iter()
                .map(|path| {
                    json!({
                        "id":path["id"],
                        "role":path["role"],
                        "spot":path["spot"]
                    })
                })
                .collect();
            let panel_art_bound = panels
                .iter()
                .filter(|panel| panel.get("artwork").is_some())
                .count();
            println!(
                "{}",
                json!({
                    "id":dieline,
                    "template":value["template"],
                    "units":value["units"],
                    "dimensions_mm":value["dimensions_mm"],
                    "fold_state":value["fold_state"],
                    "scene3d_node":value["scene3d_node"],
                    "panels":panel_report,
                    "panel_count":panels.len(),
                    "panel_art_bound":panel_art_bound,
                    "creases":crease_report,
                    "crease_count":creases.len(),
                    "paths":path_report,
                    "path_count":paths.len()
                })
            );
            return Ok(());
        }
        Scene3dAction::Pack {
            action: PackAction::VerifyBundle { bundle },
        } => {
            println!("{}", verify_pack_bundle(bundle)?);
            return Ok(());
        }
        Scene3dAction::Pack {
            action: PackAction::Preflight { dieline, min_ppi },
        } => {
            println!(
                "{}",
                super::pack::preflight(&raw, input, dieline, *min_ppi)?
            );
            return Ok(());
        }
        Scene3dAction::Pack {
            action:
                PackAction::Separate {
                    dieline,
                    out,
                    ppi,
                    bits,
                },
        } => {
            ensure!(
                *bits == 8 || *bits == 16,
                "[invalid-input] --bits must be 8 or 16"
            );
            let report = super::pack::write_dieline_spot_separations(
                &raw,
                input,
                dieline,
                out,
                *ppi,
                *bits,
                &current_revision,
            )?;
            println!("{}", report);
            return Ok(());
        }
        Scene3dAction::Pack {
            action: PackAction::VectorSeparate { dieline, out },
        } => {
            let report = if dry_run {
                let parent = out
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                ensure!(
                    parent.is_dir(),
                    "[missing-resource] vector plate output parent must exist"
                );
                ensure!(
                    !out.exists(),
                    "[output-exists] vector plate directory {} already exists",
                    out.display()
                );
                let nonce = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos();
                let stage = parent.join(format!(
                    ".pentool-vector-plates-{}-{nonce}",
                    std::process::id()
                ));
                let mut report =
                    super::pack::write_dieline_vector_spot_plates(&raw, dieline, &stage)?;
                std::fs::remove_dir_all(&stage)?;
                report["dry_run"] = json!(true);
                report["complete"] = json!(false);
                report["out"] = json!(out);
                report
            } else {
                let mut report = super::pack::write_dieline_vector_spot_plates(&raw, dieline, out)?;
                report["dry_run"] = json!(false);
                report["complete"] = json!(true);
                report["out"] = json!(out);
                report
            };
            println!("{report}");
            return Ok(());
        }
        Scene3dAction::Pack {
            action:
                PackAction::ExportBundle {
                    dieline,
                    out,
                    min_ppi,
                    ppi,
                    bits,
                    fold,
                },
        } => {
            let report = write_pack_bundle(
                &raw,
                input,
                PackBundleOptions {
                    dieline_id: dieline,
                    out,
                    min_ppi: *min_ppi,
                    ppi: *ppi,
                    bits: *bits,
                    fold: *fold,
                    source_revision: &current_revision,
                    dry_run,
                },
            )?;
            println!("{report}");
            return Ok(());
        }
        Scene3dAction::Pack {
            action: PackAction::Preview { dieline, fold, out },
        } => {
            ensure!(
                fold.is_finite() && (0.0..=1.0).contains(fold),
                "[invalid-input] --fold must be finite and within 0..=1"
            );
            ensure!(
                out.extension().and_then(|ext| ext.to_str()) == Some("png"),
                "[invalid-input] packaging preview output must end in .png"
            );
            let input_path = std::fs::canonicalize(input).with_context(|| {
                format!("[missing-resource] could not resolve {}", input.display())
            })?;
            let out_path = if out.exists() {
                std::fs::canonicalize(out)?
            } else {
                let parent = out
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                std::fs::canonicalize(parent)?.join(
                    out.file_name()
                        .context("[invalid-input] preview output needs a file name")?,
                )
            };
            ensure!(
                input_path != out_path,
                "[invalid-input] preview output must not overwrite the source document"
            );
            crate::scene::validate(&raw)?;
            let dieline_data = raw
                .get_mut("dielines")
                .and_then(|catalog| catalog.get_mut(dieline))
                .with_context(|| {
                    format!("[missing-resource] dieline `{dieline}` does not exist")
                })?;
            dieline_data["fold_state"] = json!(fold);
            let node_id = dieline_data
                .get("scene3d_node")
                .and_then(Value::as_str)
                .context("[missing-resource] dieline has no linked 3D preview node")?
                .to_owned();
            let node = scene3d_node(&mut raw, &node_id)?.clone();
            let document_dir = input
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let preview = super::node::render_png(&raw, document_dir, &node)?;
            if !dry_run {
                crate::editing::atomic_write(out, &preview)?;
            }
            println!(
                "{}",
                json!({"dieline":dieline,"out":out,"fold":fold,"format":"png","bytes":preview.len(),"renderer":"cpu-reference","document_modified":false,"revision":current_revision,"dry_run":dry_run,"complete":!dry_run})
            );
            return Ok(());
        }
        Scene3dAction::Pack {
            action:
                PackAction::Export {
                    dieline,
                    out,
                    min_ppi,
                    pdfx,
                    ppi,
                },
        } => {
            if *pdfx {
                ensure!(
                    out.extension().and_then(|ext| ext.to_str()) == Some("pdf"),
                    "[invalid-input] PDF/X packaging output must end in .pdf"
                );
                let (export, flat) = super::pack::flat_dieline_pdfx_with_artwork(
                    &raw, input, dieline, *min_ppi, *ppi,
                )?;
                if !dry_run {
                    crate::editing::atomic_write(out, &export.bytes)?;
                }
                println!(
                    "{}",
                    json!({"dieline":dieline,"out":out,"format":"pdfx-4-flattened-draft","bytes":export.bytes.len(),"ppi":ppi,"artwork_panels":flat.artwork_panels,"rasterized_panels":flat.rasterized_panels,"rasterized_ppi":flat.rasterized_ppi,"bleed_synthesized":flat.bleed_synthesized,"warnings":flat.warnings,"technical_paths":"flattened-into-process-cmyk","manufacturing_certified":false,"pdfx":export.report,"revision":current_revision,"dry_run":dry_run,"complete":!dry_run})
                );
                return Ok(());
            }
            ensure!(
                out.extension().and_then(|ext| ext.to_str()) == Some("svg"),
                "[invalid-input] flat dieline output must end in .svg"
            );
            let export =
                super::pack::flat_dieline_svg_with_artwork(&raw, input, dieline, *min_ppi)?;
            if !dry_run {
                crate::editing::atomic_write(out, export.svg.as_bytes())?;
            }
            println!(
                "{}",
                json!({"dieline":dieline,"out":out,"format":"svg","bytes":export.svg.len(),"panel_art_included":!export.artwork_panels.is_empty(),"artwork_panels":export.artwork_panels,"rasterized_panels":export.rasterized_panels,"rasterized_ppi":export.rasterized_ppi,"bleed_synthesized":export.bleed_synthesized,"warnings":export.warnings,"manufacturing_certified":false,"revision":current_revision,"dry_run":dry_run,"complete":!dry_run})
            );
            return Ok(());
        }
        _ => {}
    }
    let doc_dir = input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let (label, mut result) = match action {
        Scene3dAction::Dependencies => {
            unreachable!("dependency inspection returns before mutation planning")
        }
        Scene3dAction::RenderReport { .. } => {
            unreachable!("render report returns before mutation planning")
        }
        Scene3dAction::Batch { .. } | Scene3dAction::BatchQueue { .. } => {
            unreachable!("batch render returns before mutation planning")
        }
        Scene3dAction::Form {
            action: FormAction::Presets,
        } => unreachable!("product-form presets return before mutation planning"),
        Scene3dAction::Graphic { id, action } => {
            require_v11(&raw)?;
            let n = scene3d_node(&mut raw, &id)?;
            match action {
                GraphicAction::List => {
                    println!(
                        "{}",
                        json!({"id":id,"surface_graphics":n.get("surface_graphics").cloned().unwrap_or_else(|| json!([]))})
                    );
                    return Ok(());
                }
                GraphicAction::Add {
                    graphic,
                    source,
                    slot,
                    mode,
                    projection,
                    axis,
                    fit,
                    wrap,
                    blend,
                    scale,
                    offset,
                    rotate,
                    crop,
                    opacity,
                } => {
                    ensure!(
                        !graphic.trim().is_empty(),
                        "[invalid-input] graphic ID cannot be empty"
                    );
                    let graphics = n
                        .entry("surface_graphics")
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .context("[invalid-input] surface_graphics must be an array")?;
                    ensure!(
                        !graphics
                            .iter()
                            .any(|g| g.get("id").and_then(Value::as_str) == Some(&graphic)),
                        "[invalid-input] graphic ID `{graphic}` already exists on node `{id}`"
                    );
                    let mut value = json!({"id":graphic,"source":parse_binding_source(&source)?,"slot":slot,"mode":mode,"projection":projection,"fit":fit,"wrap":wrap,"blend":blend,"opacity":opacity});
                    if let Some(v) = axis {
                        value["axis"] = json!(v);
                    }
                    if let Some(v) = scale {
                        value["scale"] = parse_pair(&v, "--scale", 2)?;
                    }
                    if let Some(v) = offset {
                        value["offset"] = parse_pair(&v, "--offset", 2)?;
                    }
                    if let Some(v) = rotate {
                        value["rotate"] = json!(v);
                    }
                    if let Some(v) = crop {
                        value["crop"] = parse_pair(&v, "--crop", 4)?;
                    }
                    graphics.push(value);
                    ("scene3d-graphic-add", json!({"id":id,"graphic":graphic}))
                }
                GraphicAction::Set {
                    graphic,
                    source,
                    slot,
                    mode,
                    projection,
                    axis,
                    fit,
                    wrap,
                    blend,
                    scale,
                    offset,
                    rotate,
                    crop,
                    opacity,
                } => {
                    ensure!(
                        source.is_some()
                            || slot.is_some()
                            || mode.is_some()
                            || projection.is_some()
                            || axis.is_some()
                            || fit.is_some()
                            || wrap.is_some()
                            || blend.is_some()
                            || scale.is_some()
                            || offset.is_some()
                            || rotate.is_some()
                            || crop.is_some()
                            || opacity.is_some(),
                        "[invalid-input] graphic set needs at least one property"
                    );
                    let item = n.get_mut("surface_graphics").and_then(Value::as_array_mut).and_then(|items| items.iter_mut().find(|g| g.get("id").and_then(Value::as_str) == Some(&graphic))).with_context(|| format!("[missing-resource] graphic `{graphic}` does not exist on node `{id}`"))?;
                    if let Some(v) = source {
                        item["source"] = parse_binding_source(&v)?;
                    }
                    if projection.as_deref().is_some_and(|value| value == "uv") && axis.is_none() {
                        item.as_object_mut().unwrap().remove("axis");
                    }
                    for (key, value) in [
                        ("slot", slot),
                        ("mode", mode),
                        ("projection", projection),
                        ("axis", axis),
                        ("fit", fit),
                        ("wrap", wrap),
                        ("blend", blend),
                    ] {
                        if let Some(value) = value {
                            item[key] = json!(value);
                        }
                    }
                    if let Some(v) = scale {
                        item["scale"] = parse_pair(&v, "--scale", 2)?;
                    }
                    if let Some(v) = offset {
                        item["offset"] = parse_pair(&v, "--offset", 2)?;
                    }
                    if let Some(v) = rotate {
                        item["rotate"] = json!(v);
                    }
                    if let Some(v) = crop {
                        item["crop"] = parse_pair(&v, "--crop", 4)?;
                    }
                    if let Some(v) = opacity {
                        item["opacity"] = json!(v);
                    }
                    ("scene3d-graphic-set", json!({"id":id,"graphic":graphic}))
                }
                GraphicAction::Remove { graphic } => {
                    let graphics = n
                        .get_mut("surface_graphics")
                        .and_then(Value::as_array_mut)
                        .context("[missing-resource] node has no surface graphics")?;
                    let index = graphics.iter().position(|g| g.get("id").and_then(Value::as_str) == Some(&graphic)).with_context(|| format!("[missing-resource] graphic `{graphic}` does not exist on node `{id}`"))?;
                    graphics.remove(index);
                    let is_empty = graphics.is_empty();
                    if is_empty {
                        n.remove("surface_graphics");
                    }
                    ("scene3d-graphic-remove", json!({"id":id,"graphic":graphic}))
                }
            }
        }
        Scene3dAction::Pack {
            action:
                PackAction::Templates
                | PackAction::Inspect { .. }
                | PackAction::Preflight { .. }
                | PackAction::Separate { .. }
                | PackAction::VectorSeparate { .. }
                | PackAction::Preview { .. }
                | PackAction::Export { .. }
                | PackAction::ExportBundle { .. }
                | PackAction::VerifyBundle { .. },
        } => unreachable!("read-only packaging actions return before the mutation planner"),
        Scene3dAction::Pack {
            action:
                PackAction::Import {
                    dieline,
                    svg,
                    node,
                    layer,
                    page,
                    x,
                    y,
                    width,
                    height,
                },
        } => {
            let bytes =
                crate::resource::read_bounded(&svg, super::pack::MAX_IMPORTED_DIELINE_SVG_BYTES)?;
            let svg_text = std::str::from_utf8(&bytes)
                .context("[malformed-resource] dieline SVG must be UTF-8")?;
            (
                "scene3d-pack-svg-import",
                super::pack::add_svg_dieline(
                    &mut raw,
                    svg_text,
                    &dieline,
                    &node,
                    &layer,
                    page.as_deref(),
                    [x, y, width, height],
                )?,
            )
        }
        Scene3dAction::Pack {
            action:
                PackAction::Sleeve {
                    dieline,
                    node,
                    layer,
                    page,
                    length_mm,
                    width_mm,
                    height_mm,
                    board_mm,
                    bleed_mm,
                    x,
                    y,
                    width,
                    height,
                },
        } => (
            "scene3d-pack-sleeve",
            super::pack::add_sleeve(
                &mut raw,
                &dieline,
                &node,
                &layer,
                page.as_deref(),
                length_mm,
                width_mm,
                height_mm,
                board_mm,
                bleed_mm,
                [x, y, width, height],
            )?,
        ),
        Scene3dAction::Pack {
            action:
                PackAction::Crease {
                    dieline,
                    crease,
                    angle_deg,
                    direction,
                },
        } => ("scene3d-pack-crease-angle", {
            ensure!(
                angle_deg.is_some() || direction.is_some(),
                "[invalid-input] provide --angle-deg, --direction, or both"
            );
            let mut result = serde_json::json!({"dieline":dieline,"crease":crease});
            if let Some(angle_deg) = angle_deg {
                result = super::pack::set_crease_angle(&mut raw, &dieline, &crease, angle_deg)?;
            }
            if let Some(direction) = direction {
                let change =
                    super::pack::set_crease_direction(&mut raw, &dieline, &crease, &direction)?;
                result["previous_direction"] = change["previous_direction"].clone();
                result["direction"] = change["direction"].clone();
            }
            result
        }),
        Scene3dAction::Pack {
            action:
                PackAction::Settings {
                    dieline,
                    board_mm,
                    bleed_mm,
                    seam_angle_deg,
                    wrap_direction,
                },
        } => (
            "scene3d-pack-settings",
            super::pack::set_print_settings(
                &mut raw,
                &dieline,
                board_mm,
                bleed_mm,
                seam_angle_deg,
                wrap_direction.as_deref(),
            )?,
        ),
        Scene3dAction::Pack {
            action:
                PackAction::Pouch {
                    dieline,
                    node,
                    layer,
                    page,
                    front_width_mm,
                    gusset_mm,
                    height_mm,
                    bleed_mm,
                    x,
                    y,
                    width,
                    height,
                },
        } => (
            "scene3d-pack-pouch",
            super::pack::add_pouch(
                &mut raw,
                &dieline,
                &node,
                &layer,
                page.as_deref(),
                front_width_mm,
                gusset_mm,
                height_mm,
                bleed_mm,
                [x, y, width, height],
            )?,
        ),
        Scene3dAction::Pack {
            action:
                PackAction::Tube {
                    dieline,
                    node,
                    layer,
                    page,
                    radius_mm,
                    height_mm,
                    bleed_mm,
                    overlap_mm,
                    x,
                    y,
                    width,
                    height,
                },
        } => (
            "scene3d-pack-tube-label",
            super::pack::add_tube_label_with_overlap(
                &mut raw,
                &dieline,
                &node,
                &layer,
                page.as_deref(),
                radius_mm,
                height_mm,
                bleed_mm,
                overlap_mm,
                [x, y, width, height],
            )?,
        ),
        Scene3dAction::Pack {
            action:
                PackAction::BottleLabel {
                    dieline,
                    node,
                    layer,
                    page,
                    radius_mm,
                    height_mm,
                    bleed_mm,
                    overlap_mm,
                    x,
                    y,
                    width,
                    height,
                },
        } => (
            "scene3d-pack-bottle-label",
            super::pack::add_bottle_label(
                &mut raw,
                &dieline,
                &node,
                &layer,
                page.as_deref(),
                radius_mm,
                height_mm,
                bleed_mm,
                overlap_mm,
                [x, y, width, height],
            )?,
        ),
        Scene3dAction::Pack {
            action:
                PackAction::Carton {
                    dieline,
                    node,
                    layer,
                    page,
                    template,
                    length_mm,
                    width_mm,
                    height_mm,
                    board_mm,
                    bleed_mm,
                    glue_flap_mm,
                    x,
                    y,
                    width,
                    height,
                },
        } => (
            "scene3d-pack-carton",
            super::pack::add_tuck_carton(
                &mut raw,
                &dieline,
                &node,
                &layer,
                page.as_deref(),
                &template,
                length_mm,
                width_mm,
                height_mm,
                board_mm,
                bleed_mm,
                glue_flap_mm,
                [x, y, width, height],
            )?,
        ),
        Scene3dAction::Pack {
            action:
                PackAction::Bind {
                    dieline,
                    panel,
                    source,
                },
        } => {
            let binding = source
                .map(|source| parse_binding_source(&source))
                .transpose()?;
            let result =
                super::bindings::set_panel_artwork(&mut raw, &dieline, &panel, binding.as_ref())?;
            ("scene3d-pack-bind", result)
        }
        Scene3dAction::Form {
            action:
                FormAction::Add {
                    id,
                    form,
                    preset,
                    kind,
                    parameters,
                    slots,
                    layer,
                    page,
                    x,
                    y,
                    width,
                    height,
                },
        } => (
            "scene3d-form-add",
            add_form(
                &mut raw,
                &id,
                &form,
                preset.as_deref(),
                kind.as_deref(),
                parameters.as_deref(),
                slots.as_deref(),
                &layer,
                page.as_deref(),
                [x, y, width, height],
            )?,
        ),
        Scene3dAction::Form {
            action:
                FormAction::Set {
                    form,
                    parameters,
                    slots,
                },
        } => (
            "scene3d-form-set",
            set_form(&mut raw, &form, &parameters, slots.as_deref())?,
        ),
        Scene3dAction::List => {
            let mut nodes = Vec::new();
            for page in raw["pages"].as_array().into_iter().flatten() {
                for layer in page["layers"].as_array().into_iter().flatten() {
                    for n in layer["nodes"].as_array().into_iter().flatten() {
                        if n["kind"] == "scene3d" {
                            nodes.push(json!({
                                "id": n["id"], "page": page["id"], "layer": layer["id"],
                                "model": n["model"], "product_form": n["product_form"], "dieline": n["dieline"],
                                "x": n["x"], "y": n["y"], "width": n["width"], "height": n["height"],
                            }));
                        }
                    }
                }
            }
            println!(
                "{}",
                json!({"scene3d": nodes, "models": raw["models"].as_object().map_or(0, |m| m.len()), "product_forms": raw["product_forms"].as_object().map_or(0, |m| m.len()), "dielines": raw["dielines"].as_object().map_or(0, |m| m.len())})
            );
            return Ok(());
        }
        Scene3dAction::Uv { id, action } => {
            require_v10(&raw)?;
            let node = scene3d_node(&mut raw, &id)?;
            let digest = node
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if digest.is_empty() && node.contains_key("dieline") {
                bail!(
                    "[unsupported-capability] node `{id}` is a dieline preview with no UV model; inspect its panel UV rectangles with `pack inspect` and export the flat layout with `pack export`"
                );
            }
            match action {
                UvAction::Show {
                    resolution,
                    minimum_density,
                } => {
                    let model = node::load_model(&raw, &doc_dir, &digest)?;
                    let mut report = super::uv::show_with_overlays_at_resolution(
                        &model,
                        raw["models"][&digest].get("uv").unwrap_or(&Value::Null),
                        resolution,
                        minimum_density,
                    )?;
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
                    let report = super::uv::show_with_overlays(
                        &model,
                        raw["models"][&digest].get("uv").unwrap_or(&Value::Null),
                    )?;
                    ("scene3d-uv-project", json!({"id": id, "uv": report}))
                }
                UvAction::Island { action } => {
                    require_v11(&raw)?;
                    if let UvIslandAction::Pack { primitive, padding } = &action {
                        let key = primitive.to_string();
                        let current = raw["models"][&digest]["uv"][&key].clone();
                        let model = node::load_model(&raw, &doc_dir, &digest)?;
                        let prim = model.primitives.get(*primitive).with_context(|| {
                            format!("[missing-resource] primitive {primitive} does not exist")
                        })?;
                        let packed = super::uv::pack_islands(prim, &current, *padding)?;
                        raw["models"][&digest]["uv"][&key] = packed;
                        let model = node::load_model(&raw, &doc_dir, &digest)?;
                        let report = super::uv::show_with_overlays(
                            &model,
                            raw["models"][&digest].get("uv").unwrap_or(&Value::Null),
                        )?;
                        (
                            "scene3d-uv-island-pack",
                            json!({"id":id,"primitive":key,"padding":padding,"uv":report}),
                        )
                    } else {
                        let key = match &action {
                            UvIslandAction::List { primitive }
                            | UvIslandAction::Add { primitive, .. }
                            | UvIslandAction::Set { primitive, .. }
                            | UvIslandAction::Remove { primitive, .. }
                            | UvIslandAction::Pack { primitive, .. } => primitive.to_string(),
                        };
                        if let UvIslandAction::List { .. } = &action {
                            let islands = raw["models"][&digest]["uv"][&key]["islands"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default();
                            println!(
                                "{}",
                                json!({"id":id,"model":digest,"primitive":key,"islands":islands})
                            );
                            return Ok(());
                        }
                        let record = raw["models"]
                            .get_mut(&digest)
                            .and_then(Value::as_object_mut)
                            .context("[missing-resource] model record is missing")?;
                        let overlays = record
                            .entry("uv")
                            .or_insert_with(|| json!({}))
                            .as_object_mut()
                            .context("[invalid-input] model UV overlay must be an object")?;
                        let overlay = overlays
                            .entry(&key)
                            .or_insert_with(|| json!({}))
                            .as_object_mut()
                            .context("[invalid-input] primitive UV overlay must be an object")?;
                        let islands = overlay
                            .entry("islands")
                            .or_insert_with(|| json!([]))
                            .as_array_mut()
                            .context("[invalid-input] UV islands must be an array")?;
                        match action {
                            UvIslandAction::List { .. } => unreachable!(),
                            UvIslandAction::Pack { .. } => unreachable!(),
                            UvIslandAction::Add {
                                id: island_id,
                                uv_set,
                                triangles,
                                scale,
                                offset,
                                rotate,
                                ..
                            } => {
                                ensure!(
                                    !island_id.trim().is_empty(),
                                    "[invalid-input] UV island ID cannot be empty"
                                );
                                ensure!(!islands.iter().any(|island| island["id"].as_str() == Some(&island_id)), "[invalid-input] UV island `{island_id}` already exists on primitive {key}");
                                let transform = json!({
                                    "scale": scale.map(|v| parse_pair(&v, "--scale", 2)).transpose()?.unwrap_or(json!([1.0,1.0])),
                                    "offset": offset.map(|v| parse_pair(&v, "--offset", 2)).transpose()?.unwrap_or(json!([0.0,0.0])),
                                    "rotate": rotate.unwrap_or(0.0),
                                });
                                islands.push(json!({"id":island_id,"uv_set":uv_set,"triangles":parse_triangle_indices(&triangles)?,"transform":transform}));
                            }
                            UvIslandAction::Set {
                                id: island_id,
                                uv_set,
                                triangles,
                                scale,
                                offset,
                                rotate,
                                ..
                            } => {
                                ensure!(
                                    uv_set.is_some()
                                        || triangles.is_some()
                                        || scale.is_some()
                                        || offset.is_some()
                                        || rotate.is_some(),
                                    "[invalid-input] island set needs at least one property"
                                );
                                let island = islands.iter_mut().find(|island| island["id"].as_str() == Some(&island_id)).with_context(|| format!("[missing-resource] UV island `{island_id}` does not exist on primitive {key}"))?;
                                if let Some(value) = uv_set {
                                    island["uv_set"] = json!(value);
                                }
                                if let Some(value) = triangles {
                                    island["triangles"] = parse_triangle_indices(&value)?;
                                }
                                let transform = island["transform"].as_object_mut().context(
                                    "[invalid-input] UV island transform must be an object",
                                )?;
                                for (key, value) in [("scale", scale), ("offset", offset)] {
                                    if let Some(value) = value {
                                        transform.insert(
                                            key.into(),
                                            parse_pair(&value, &format!("--{key}"), 2)?,
                                        );
                                    }
                                }
                                if let Some(value) = rotate {
                                    transform.insert("rotate".into(), json!(value));
                                }
                            }
                            UvIslandAction::Remove { id: island_id, .. } => {
                                let index = islands.iter().position(|island| island["id"].as_str() == Some(&island_id)).with_context(|| format!("[missing-resource] UV island `{island_id}` does not exist on primitive {key}"))?;
                                islands.remove(index);
                                if islands.is_empty() {
                                    overlay.remove("islands");
                                }
                                if overlay.is_empty() {
                                    overlays.remove(&key);
                                }
                                if overlays.is_empty() {
                                    record.remove("uv");
                                }
                            }
                        }
                        // Load through the shared importer to validate UV set and triangle references before commit.
                        let model = node::load_model(&raw, &doc_dir, &digest)?;
                        let report = super::uv::show_with_overlays(
                            &model,
                            raw["models"][&digest].get("uv").unwrap_or(&Value::Null),
                        )?;
                        (
                            "scene3d-uv-island",
                            json!({"id":id,"primitive":key,"uv":report}),
                        )
                    }
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
        Scene3dAction::Inspect { id, offset, limit } => {
            require_v10(&raw)?;
            ensure!(
                (1..=256).contains(&limit),
                "[invalid-input] inspect limit must be 1..=256"
            );
            let n = scene3d_node(&mut raw, &id)?.clone();
            let digest = n["model"].as_str().unwrap_or_default().to_string();
            let model = node::load_model(&raw, &doc_dir, &digest)?;
            let (scene, options) = node::scene_for(&n, Some(&model))?;
            let mut report = node::inspect(&model, offset, limit);
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
                    phys.insert("sensor_mm".into(), sensor_pair(&s)?);
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
        Scene3dAction::Frame {
            id,
            azimuth,
            elevation,
            focal_length,
            sensor,
        } => {
            require_v10(&raw)?;
            let mut framed = scene3d_node(&mut raw, &id)?.clone();
            let digest = framed["model"].as_str().unwrap_or_default().to_string();
            let model = node::load_model(&raw, &doc_dir, &digest)?;
            let camera = framed
                .entry("camera")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("[invalid-input] camera must be an object")?;
            for key in ["eye", "target", "up", "near", "far", "height"] {
                camera.remove(key);
            }
            camera.insert("projection".into(), json!("perspective"));
            camera.insert("azimuth".into(), json!(azimuth));
            camera.insert("elevation".into(), json!(elevation));
            if let Some(focal) = focal_length {
                camera.remove("fov");
                let physical = framed
                    .entry("physical_camera")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .context("[invalid-input] physical_camera must be an object")?;
                physical.insert("focal_length_mm".into(), json!(focal));
                if let Some(sensor) = sensor {
                    physical.insert("sensor_mm".into(), sensor_pair(&sensor)?);
                }
            } else if sensor.is_some() {
                bail!("[invalid-input] --sensor requires --focal-length");
            }
            let (scene, _) = node::scene_for(&framed, Some(&model))?;
            let camera = framed["camera"].as_object_mut().unwrap();
            camera.insert("eye".into(), json!(scene.camera.eye));
            camera.insert("target".into(), json!(scene.camera.target));
            camera.insert("up".into(), json!(scene.camera.up));
            camera.insert("near".into(), json!(scene.camera.near));
            camera.insert("far".into(), json!(scene.camera.far));
            let stored_camera = framed.remove("camera").unwrap();
            let stored_physical = framed.remove("physical_camera");
            let target = scene3d_node(&mut raw, &id)?;
            target.insert("camera".into(), stored_camera);
            match stored_physical {
                Some(value) => {
                    target.insert("physical_camera".into(), value);
                }
                None => {
                    target.remove("physical_camera");
                }
            }
            (
                "scene3d-frame",
                json!({"id": id, "eye": scene.camera.eye, "target": scene.camera.target}),
            )
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
        Scene3dAction::Material { id, action } => {
            require_v10(&raw)?;
            if let MaterialAction::Set { material, .. } | MaterialAction::Clear { material } =
                &action
            {
                let scene = scene3d_node(&mut raw, &id)?.clone();
                let digest = scene["model"].as_str().unwrap_or_default();
                let model = node::load_model(&raw, &doc_dir, digest)?;
                ensure!(
                    *material < model.materials.len(),
                    "[missing-resource] material {material} does not exist; inspect the scene for valid IDs"
                );
            }
            let n = scene3d_node(&mut raw, &id)?;
            match action {
                MaterialAction::Reset => {
                    n.remove("material_overrides");
                }
                MaterialAction::Clear { material } => {
                    let overrides = n
                        .get_mut("material_overrides")
                        .and_then(Value::as_object_mut)
                        .context(
                            "[missing-resource] this scene has no material overrides to clear",
                        )?;
                    ensure!(
                        overrides.remove(&material.to_string()).is_some(),
                        "[missing-resource] material {material} has no override to clear"
                    );
                    if overrides.is_empty() {
                        n.remove("material_overrides");
                    }
                }
                MaterialAction::Set {
                    material,
                    base_color,
                    metallic,
                    roughness,
                    emissive,
                    double_sided,
                } => {
                    ensure!(
                        base_color.is_some()
                            || metallic.is_some()
                            || roughness.is_some()
                            || emissive.is_some()
                            || double_sided.is_some(),
                        "[invalid-input] material set needs at least one property"
                    );
                    let overrides = n
                        .entry("material_overrides")
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                        .context("[invalid-input] material_overrides must be an object")?;
                    let entry = overrides
                        .entry(material.to_string())
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                        .context("[invalid-input] material override must be an object")?;
                    if let Some(v) = base_color {
                        entry.insert("base_color".into(), json!(v));
                    }
                    if let Some(v) = metallic {
                        entry.insert("metallic".into(), json!(v));
                    }
                    if let Some(v) = roughness {
                        entry.insert("roughness".into(), json!(v));
                    }
                    if let Some(v) = emissive {
                        entry.insert("emissive".into(), json!(v));
                    }
                    if let Some(v) = double_sided {
                        entry.insert("double_sided".into(), json!(v));
                    }
                }
            }
            ("scene3d-material", json!({"id": id}))
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

/// Print the offline procedural mockup catalog without requiring a document.
pub fn run_mockup_list() -> Result<()> {
    println!(
        "{}",
        json!({
            "format":"pentool-mockup-catalog-v1",
            "source":"Pentool bounded procedural generators",
            "asset_license":"No third-party model assets; geometry is generated from Pentool code",
            "presets":super::form::starter_presets()
        })
    );
    Ok(())
}

/// Add a built-in product starter through the shared form transaction path.
#[derive(Debug, Clone)]
pub struct MockupPlacement {
    pub page: Option<String>,
    pub rect: [f64; 4],
}

pub fn run_mockup_apply(
    input: &Path,
    dry_run: bool,
    revision: Option<&str>,
    preset: &str,
    id: &str,
    layer: &str,
    placement: MockupPlacement,
) -> Result<()> {
    let form_id = format!("{id}-form");
    run(
        input,
        dry_run,
        revision,
        Scene3dAction::Form {
            action: FormAction::Add {
                id: id.to_owned(),
                form: form_id,
                preset: Some(preset.to_owned()),
                kind: None,
                parameters: None,
                slots: None,
                layer: layer.to_owned(),
                page: placement.page,
                x: placement.rect[0],
                y: placement.rect[1],
                width: placement.rect[2],
                height: placement.rect[3],
            },
        },
    )
}

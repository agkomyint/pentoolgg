use pentool::{
    agent, ai, asset, benchmark, composite, diff, document, editing, fonts, geometry, history,
    image, import, instance, layout, library, package, page, pdf, raster, render, replace, scene,
    server, style, text, transaction,
};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use document::Document;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(
    name = "pentool",
    version,
    about = "Draw vector paths, automate them, and serve a canvas",
    allow_negative_numbers = true
)]
struct Cli {
    /// Select a page for page-aware commands (v3 documents).
    #[arg(long, global = true)]
    page: Option<String>,
    /// Emit stable machine-readable errors.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Args)]
struct AdjustmentArgs {
    /// add, set, enable, disable, remove, or list.
    operation: String,
    input: PathBuf,
    id: Option<String>,
    #[arg(long)]
    kind: Option<String>,
    /// JSON object with adjustment parameters; set replaces the parameter object.
    #[arg(long)]
    params: Option<String>,
    /// below, group:ID, or ids:ID,ID (preceding siblings in the same stack).
    #[arg(long)]
    scope: Option<String>,
    #[arg(long)]
    layer: Option<String>,
    #[arg(long)]
    opacity: Option<f64>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct SelectionArgs {
    /// query, save, or crop.
    operation: String,
    input: PathBuf,
    #[arg(long)]
    query: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct AnalyzeArgs {
    input: PathBuf,
    #[arg(long, default_value = "page")]
    scope: String,
    #[arg(long)]
    query: Option<String>,
    /// JSON array of pixel coordinates, e.g. [[10,20]].
    #[arg(long, default_value = "[]")]
    samples: String,
    #[arg(long)]
    compare: bool,
    /// Explicitly create color tokens from the reported palette.
    #[arg(long)]
    palette_tokens: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct LinkedArgs {
    /// report, locate, relink, embed, externalize, or collect.
    operation: String,
    input: PathBuf,
    asset: Option<String>,
    #[arg(long)]
    path: Option<PathBuf>,
    /// JSON array of document-relative candidate paths for locate.
    #[arg(long)]
    candidates: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct PresetArgs {
    /// save, apply, copy, or paste. Copy/paste use named document appearance styles.
    operation: String,
    input: PathBuf,
    id: String,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct MaskArgs {
    /// create, attach, detach, delete, apply, or list.
    operation: String,
    input: PathBuf,
    id: Option<String>,
    /// Named mask or resource hash for attach.
    mask: Option<String>,
    /// vector:ID, node-alpha:ID, or asset:sha256:DIGEST.
    #[arg(long)]
    from: Option<String>,
    #[arg(long)]
    invert: bool,
    #[arg(long, default_value_t = 1.0)]
    density: f64,
    #[arg(long, default_value_t = 0.0)]
    feather: f64,
    #[arg(long)]
    unlinked: bool,
    #[arg(long, num_args = 6, allow_hyphen_values = true)]
    transform: Vec<f64>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct ClipArgs {
    operation: String,
    input: PathBuf,
    id: String,
    #[arg(long)]
    base: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct TransformArgs {
    operation: String,
    input: PathBuf,
    id: String,
    kind: Option<String>,
    #[arg(long, default_value = "transform-1")]
    op_id: String,
    #[arg(long)]
    params: Option<String>,
    #[arg(long, conflicts_with = "params")]
    quad: Option<String>,
    #[arg(long)]
    mask: Option<String>,
    #[arg(long, conflicts_with = "mask")]
    no_mask: bool,
    #[arg(long)]
    opacity: Option<f64>,
    #[arg(long)]
    index: Option<usize>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct FillArgs {
    operation: String,
    input: PathBuf,
    id: String,
    #[arg(long)]
    kind: Option<String>,
    /// Hex color, RGBA JSON, or token reference JSON for solid fills.
    #[arg(long)]
    color: Option<String>,
    /// Gradient/pattern settings as a JSON object.
    #[arg(long)]
    params: Option<String>,
    #[arg(long)]
    layer: Option<String>,
    #[arg(long)]
    x: Option<f64>,
    #[arg(long)]
    y: Option<f64>,
    #[arg(long)]
    width: Option<f64>,
    #[arg(long)]
    height: Option<f64>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct EffectArgs {
    operation: String,
    input: PathBuf,
    id: String,
    /// Required for every operation except `list`.
    effect_id: Option<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    params: Option<String>,
    #[arg(long)]
    x: Option<f64>,
    #[arg(long)]
    y: Option<f64>,
    #[arg(long)]
    blur: Option<f64>,
    #[arg(long)]
    color: Option<String>,
    #[arg(long)]
    opacity: Option<f64>,
    #[arg(long)]
    blend: Option<String>,
    #[arg(long)]
    index: Option<usize>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct GroupArgs {
    input: PathBuf,
    #[arg(value_enum)]
    operation: scene::GroupOperation,
    id: String,
    #[arg(long)]
    layer: Option<String>,
    #[arg(long, value_delimiter = ',')]
    children: Vec<String>,
    #[arg(long = "id", visible_alias = "id-new")]
    id_new: Option<String>,
    #[arg(long, default_value_t = 0.0)]
    dx: f64,
    #[arg(long, default_value_t = 0.0)]
    dy: f64,
    #[arg(long, default_value_t = 0.0)]
    degrees: f64,
    #[arg(long, default_value_t = 1.0)]
    scale_x: f64,
    #[arg(long, default_value_t = 1.0)]
    scale_y: f64,
    #[arg(long)]
    index: Option<usize>,
    #[arg(long)]
    child: Option<String>,
    #[arg(long)]
    component: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct StyleArgs {
    input: PathBuf,
    #[arg(value_enum)]
    operation: style::Operation,
    name: Option<String>,
    #[arg(long = "type")]
    token_type: Option<String>,
    #[arg(long)]
    value: Option<String>,
    #[arg(long)]
    to: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct ReplaceArgs {
    input: PathBuf,
    #[arg(long)]
    fill: Option<String>,
    #[arg(long)]
    stroke: Option<String>,
    #[arg(long)]
    to: String,
    #[arg(long)]
    page: Option<String>,
    #[arg(long)]
    all_pages: bool,
    #[arg(long)]
    layer: Option<String>,
    #[arg(long)]
    group: Option<String>,
    #[arg(long)]
    object_type: Option<String>,
    #[arg(long)]
    visible: Option<bool>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct FormatArgs {
    input: PathBuf,
    #[arg(long, required_unless_present = "pretty", conflicts_with = "pretty")]
    compact: bool,
    #[arg(long, required_unless_present = "compact", conflicts_with = "compact")]
    pretty: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct DiffArgs {
    old: PathBuf,
    new: PathBuf,
    #[arg(long)]
    visual: bool,
    /// Isolate the selected node's appearance in v6 visual artifacts.
    #[arg(long, requires = "visual")]
    node: Option<String>,
}

#[derive(Args)]
struct LayoutArgs {
    input: PathBuf,
    #[arg(value_enum)]
    operation: layout::Operation,
    #[arg(long, value_delimiter = ',')]
    ids: Vec<String>,
    #[arg(long)]
    gap: Option<f64>,
    #[arg(long)]
    columns: Option<usize>,
    #[arg(long)]
    key: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct TextBoxArgs {
    input: PathBuf,
    id: String,
    #[arg(long)]
    layer: String,
    #[arg(long)]
    content: String,
    #[arg(long, default_value_t = 100.0)]
    x: f64,
    #[arg(long, default_value_t = 100.0)]
    y: f64,
    #[arg(long)]
    width: Option<f64>,
    #[arg(long)]
    height: Option<f64>,
    #[arg(long, default_value = "Atkinson Hyperlegible")]
    font: String,
    #[arg(long, default_value_t = 48.0)]
    size: f64,
    #[arg(long, default_value_t = 400)]
    weight: u16,
    #[arg(long, default_value = "#111827")]
    fill: String,
    #[arg(long, default_value = "left")]
    align: String,
    #[arg(long, default_value = "top")]
    vertical_align: String,
    #[arg(long, default_value_t = 1.2)]
    line_height: f64,
    #[arg(long, value_enum, default_value = "baseline")]
    anchor: scene::TextAnchor,
    #[arg(long, value_enum, default_value = "visible")]
    overflow: scene::TextOverflow,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Args)]
struct ShapeEditArgs {
    input: PathBuf,
    id: String,
    #[arg(long)]
    x: Option<f64>,
    #[arg(long)]
    y: Option<f64>,
    #[arg(long, visible_alias = "w")]
    width: Option<f64>,
    #[arg(long, visible_alias = "h")]
    height: Option<f64>,
    #[arg(long, visible_alias = "r")]
    radius: Option<f64>,
    #[arg(long)]
    cx: Option<f64>,
    #[arg(long)]
    cy: Option<f64>,
    #[arg(long)]
    radius_x: Option<f64>,
    #[arg(long)]
    radius_y: Option<f64>,
    #[arg(long)]
    x1: Option<f64>,
    #[arg(long)]
    y1: Option<f64>,
    #[arg(long)]
    x2: Option<f64>,
    #[arg(long)]
    y2: Option<f64>,
    #[arg(long)]
    fill: Option<String>,
    #[arg(long)]
    stroke: Option<String>,
    #[arg(long)]
    stroke_width: Option<f64>,
    #[arg(long)]
    to_path: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

/// Object kinds accepted by `tree --kind` and `search --kind`.
#[derive(Clone, Copy, clap::ValueEnum)]
enum SearchKind {
    Path,
    Text,
    /// v5 raster image nodes.
    Image,
    /// v6 raster-paint layers.
    Raster,
}

impl SearchKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Text => "text",
            Self::Image => "image",
            Self::Raster => "raster",
        }
    }
    fn legacy(self) -> Result<agent::ObjectKind> {
        match self {
            Self::Path => Ok(agent::ObjectKind::Path),
            Self::Text => Ok(agent::ObjectKind::Text),
            Self::Image => anyhow::bail!(
                "[unsupported-capability] --kind image requires a v5 document; migrate with `pentool migrate`"
            ),
            Self::Raster => anyhow::bail!(
                "[unsupported-capability] --kind raster requires a v6 document; migrate with `pentool migrate`"
            ),
        }
    }
}

#[derive(clap::Args, Default)]
struct OpFlags {
    /// Blur or sharpen radius in pixels (0-256).
    #[arg(long)]
    radius: Option<f64>,
    /// Sharpen amount in percent (0-500).
    #[arg(long)]
    amount: Option<f64>,
    #[arg(long, allow_hyphen_values = true)]
    brightness: Option<f64>,
    #[arg(long, allow_hyphen_values = true)]
    contrast: Option<f64>,
    #[arg(long)]
    black: Option<f64>,
    #[arg(long)]
    white: Option<f64>,
    #[arg(long)]
    gamma: Option<f64>,
    #[arg(long, allow_hyphen_values = true)]
    hue: Option<f64>,
    #[arg(long, allow_hyphen_values = true)]
    saturation: Option<f64>,
    /// Rotation in degrees: 0, 90, 180, or 270.
    #[arg(long)]
    degrees: Option<f64>,
    /// Resize width in pixels.
    #[arg(long)]
    width: Option<f64>,
    /// Resize height in pixels.
    #[arg(long)]
    height: Option<f64>,
    /// Normalized crop rectangle: x y width height.
    #[arg(long, num_args = 4)]
    rect: Option<Vec<f64>>,
    /// Curve points as input:output pairs, for example 0:0,128:160,255:255.
    #[arg(long)]
    points: Option<String>,
}

impl OpFlags {
    fn into_params(self) -> Result<image::OpParams> {
        let mut map = serde_json::Map::new();
        for (key, value) in [
            ("radius", self.radius),
            ("amount", self.amount),
            ("brightness", self.brightness),
            ("contrast", self.contrast),
            ("black", self.black),
            ("white", self.white),
            ("gamma", self.gamma),
            ("hue", self.hue),
            ("saturation", self.saturation),
            ("degrees", self.degrees),
            ("width", self.width),
            ("height", self.height),
        ] {
            if let Some(value) = value {
                map.insert(key.into(), serde_json::json!(value));
            }
        }
        if let Some(rect) = self.rect {
            for (key, value) in ["x", "y", "width", "height"].into_iter().zip(rect) {
                map.insert(key.into(), serde_json::json!(value));
            }
        }
        if let Some(points) = self.points {
            let parsed = points
                .split(',')
                .map(|pair| {
                    let (input, output) = pair
                        .split_once(':')
                        .context("[invalid-operation] curve points must look like 0:0,255:255")?;
                    Ok(serde_json::json!([
                        input.trim().parse::<f64>()?,
                        output.trim().parse::<f64>()?
                    ]))
                })
                .collect::<Result<Vec<_>>>()?;
            map.insert("points".into(), serde_json::Value::Array(parsed));
        }
        Ok(image::OpParams(map))
    }
}

#[derive(Subcommand)]
enum OpAction {
    /// Append (or insert with --index) a non-destructive operation.
    Add {
        input: PathBuf,
        id: String,
        /// crop, resize, rotate, brightness-contrast, levels, curves,
        /// hue-saturation, blur, sharpen, or grayscale.
        kind: String,
        #[arg(long)]
        op_id: Option<String>,
        #[arg(long)]
        index: Option<usize>,
        #[command(flatten)]
        flags: OpFlags,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// List the ordered operation stack.
    List { input: PathBuf, id: String },
    /// Change parameters of an existing operation.
    Set {
        input: PathBuf,
        id: String,
        op_id: String,
        #[command(flatten)]
        flags: OpFlags,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Move an operation to a new position.
    Move {
        input: PathBuf,
        id: String,
        op_id: String,
        #[arg(long)]
        index: usize,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Enable a disabled operation.
    Enable {
        input: PathBuf,
        id: String,
        op_id: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Disable an operation without deleting it.
    Disable {
        input: PathBuf,
        id: String,
        op_id: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Remove an operation.
    Remove {
        input: PathBuf,
        id: String,
        op_id: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
}

#[derive(Subcommand)]
enum ImageAction {
    /// Add, list, edit, reorder, or toggle non-destructive operations.
    Op {
        #[command(subcommand)]
        action: OpAction,
    },
    /// Flatten crop and enabled operations into a new verified embedded PNG.
    Bake {
        input: PathBuf,
        id: String,
        /// Only PNG is supported; it is lossless and metadata-free.
        #[arg(long, default_value = "png")]
        format: String,
        /// Baked output never retains source metadata (EXIF, GPS, text chunks).
        #[arg(long)]
        strip_metadata: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Copy verified external image sources into the project cache so the
    /// document still renders offline if the original files move.
    Cache { input: PathBuf },
    /// Read-only dimensions, alpha, dominant colours, and focal suggestion.
    Analyze { input: PathBuf, id: String },
    /// Create or update color design tokens from the analyzed dominant colors.
    Palette {
        input: PathBuf,
        id: String,
        /// Tokens are named <prefix>-1, <prefix>-2, ...
        #[arg(long)]
        prefix: String,
        /// Number of dominant colors to promote (1-5).
        #[arg(long, default_value_t = 5)]
        count: usize,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Import and place a verified PNG, JPEG, or WebP source.
    Add {
        input: PathBuf,
        id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        layer: String,
        #[arg(long, default_value_t = 0.0)]
        x: f64,
        #[arg(long, default_value_t = 0.0)]
        y: f64,
        #[arg(long)]
        width: f64,
        #[arg(long)]
        height: f64,
        #[arg(long, value_enum, default_value = "contain")]
        fit: image::Fit,
        #[arg(long, conflicts_with = "external")]
        embed: bool,
        #[arg(long, conflicts_with = "embed")]
        external: bool,
        /// Clip the new image with the vector shape SHAPE_ID.
        #[arg(long)]
        mask: Option<String>,
        #[arg(long, default_value = "nonzero", requires = "mask")]
        mask_fill_rule: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Update image frame, crop, focal position, fit, or opacity.
    Set {
        input: PathBuf,
        id: String,
        #[arg(long)]
        x: Option<f64>,
        #[arg(long)]
        y: Option<f64>,
        #[arg(long)]
        width: Option<f64>,
        #[arg(long)]
        height: Option<f64>,
        #[arg(long, value_enum)]
        fit: Option<image::Fit>,
        #[arg(long, num_args = 2)]
        position: Option<Vec<f64>>,
        #[arg(long, num_args = 4)]
        crop: Option<Vec<f64>>,
        #[arg(long)]
        opacity: Option<f64>,
        #[arg(long, num_args = 6)]
        transform: Option<Vec<f64>>,
        #[arg(long, conflicts_with = "clear_mask")]
        mask: Option<String>,
        #[arg(long, default_value = "nonzero", requires = "mask")]
        mask_fill_rule: String,
        #[arg(long, conflicts_with = "mask")]
        clear_mask: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Inspect an image node and its referenced source record.
    Info { input: PathBuf, id: String },
    /// Remove an image node and prune its source when no nodes reference it.
    Remove {
        input: PathBuf,
        id: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
}

#[derive(Subcommand)]
enum RawAction {
    /// Import a DNG source as a catalog photo with an explicit master variant.
    Add {
        input: PathBuf,
        /// Photo ID: 1-64 letters, digits, '.', '_' or '-', starting with a letter or digit.
        id: String,
        #[arg(long)]
        file: PathBuf,
        /// Embed the source bytes (the default; sources up to 128 MiB).
        #[arg(long, conflicts_with = "external")]
        embed: bool,
        /// Reference the source by a document-relative path instead of embedding it.
        #[arg(long)]
        external: bool,
        /// auto (the profile embedded in the DNG), embedded, matrix-only, or a profile digest.
        #[arg(long, default_value = "auto")]
        camera_profile: String,
        /// Display name; defaults to the file stem.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Print a photo's recorded source facts, storage, and variants.
    Info { input: PathBuf, id: String },
    /// Set a variant's camera profile and white balance.
    Develop {
        input: PathBuf,
        /// Photo ID.
        id: String,
        /// Variant ID.
        #[arg(long, default_value = "master")]
        variant: String,
        /// auto (the profile embedded in the DNG), embedded, matrix-only, or a profile digest.
        #[arg(long)]
        camera_profile: Option<String>,
        /// Use the as-shot white (AsShotNeutral or AsShotWhiteXY).
        #[arg(long, group = "white_balance")]
        as_shot: bool,
        /// Correlated color temperature in kelvin (2000-50000); pair with --tint.
        #[arg(long, group = "white_balance")]
        temperature: Option<f64>,
        /// Tint (-150..150, positive is magenta); used with --temperature.
        #[arg(long, requires = "temperature", allow_hyphen_values = true)]
        tint: Option<f64>,
        /// Camera-RGB neutral as r,g,b.
        #[arg(long, group = "white_balance")]
        neutral: Option<String>,
        /// Sample a neutral at x,y,radius (0-1, oriented frame; radius of the long edge).
        #[arg(long, group = "white_balance")]
        sample: Option<String>,
        /// Store a deterministic gray-world suggestion as temperature and tint.
        #[arg(long, group = "white_balance")]
        suggest: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
}

#[derive(Subcommand)]
enum PhotoAction {
    /// Manage verified camera profiles in photography.profiles.
    Profile {
        #[command(subcommand)]
        action: PhotoProfileAction,
    },
}

#[derive(Subcommand)]
enum PhotoProfileAction {
    /// Verify and store a DNG camera profile (.dcp).
    Add {
        input: PathBuf,
        #[arg(long)]
        file: PathBuf,
        /// Allow the profile for sources whose UniqueCameraModel differs (recorded).
        #[arg(long)]
        force_model: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Import, place, inspect, and edit raster images.
    Image {
        #[command(subcommand)]
        action: ImageAction,
    },
    /// Import and inspect DNG raw photos in the v7 photography catalog.
    Raw {
        #[command(subcommand)]
        action: RawAction,
    },
    /// Photography catalog operations: camera profiles.
    Photo {
        #[command(subcommand)]
        action: PhotoAction,
    },
    /// Create, inspect, or preview reusable assets.
    Asset {
        #[command(subcommand)]
        action: AssetAction,
    },
    /// Manage registered local asset folders.
    Library {
        #[command(subcommand)]
        action: LibraryAction,
    },
    /// Search the local asset index.
    Explore {
        query: Option<String>,
        #[arg(long)]
        library: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Place an indexed asset as a copy or offline-safe instance.
    Add {
        destination: PathBuf,
        asset: String,
        #[arg(long, value_enum, default_value = "copy")]
        mode: AddMode,
        #[arg(long, num_args=2, default_values_t=[0.0,0.0])]
        at: Vec<f64>,
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        #[arg(long, default_value_t = 0.0)]
        rotate: f64,
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Inspect, change, or detach component instances.
    Instance {
        input: PathBuf,
        #[command(subcommand)]
        action: InstanceAction,
    },
    /// Build, verify, publish, or install deterministic packages.
    Package {
        #[command(subcommand)]
        action: PackageAction,
    },
    /// Search a filesystem or static HTTP registry.
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },
    /// Verify project package locks.
    Lock {
        #[command(subcommand)]
        action: LockAction,
    },
    /// Generate and measure a large in-memory document.
    Benchmark {
        #[arg(long, default_value_t = 100)]
        layers: usize,
        #[arg(long, default_value_t = 10_000)]
        objects: usize,
        #[arg(long)]
        png: bool,
        #[arg(long)]
        max_ms: Option<u128>,
        /// Run the stage-level renderer benchmark (implied by any render option).
        #[arg(long)]
        render: bool,
        #[arg(long, default_value_t = 1)]
        warmups: usize,
        #[arg(long, default_value_t = 5)]
        repetitions: usize,
        #[arg(long, default_value_t = 1.0)]
        scale: f32,
        #[arg(long)]
        paths_only: bool,
        /// Benchmark N placements of one reused PNG source (cold then warm runs).
        #[arg(long)]
        images: Option<usize>,
        /// Benchmark compositing stacks, shared masks, clipping and blur.
        #[arg(long, conflicts_with = "images")]
        composite: bool,
        #[arg(long, default_value_t = 16)]
        clipped: usize,
        #[arg(long, default_value_t = 8)]
        stack_depth: usize,
        #[arg(long, default_value_t = 32.0)]
        blur_radius: f64,
        /// Square source size in pixels for --images.
        #[arg(long, default_value_t = 512)]
        source_size: u32,
        /// Operations per image for --images (0-8).
        #[arg(long, default_value_t = 2)]
        operations: usize,
        /// Emit machine-readable JSON (benchmark output is JSON by default).
        #[arg(long)]
        json: bool,
    },
    /// List, add, rename, duplicate, move, or remove pages.
    Page {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: page::PageAction,
    },
    /// Raster-paint layers: sparse tiles, deterministic brush strokes, checkpoints.
    Raster {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long = "if-revision", visible_alias = "revision")]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: RasterAction,
    },
    /// Optional BYOK image models: agent-first setup, generation, review, and local cutouts.
    Ai {
        #[command(subcommand)]
        action: ai::AiAction,
    },
    /// Copy a page from another .pen file into the selected destination page.
    Import {
        destination: PathBuf,
        source: PathBuf,
        #[arg(long)]
        source_page: Option<String>,
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long, conflicts_with = "prefix")]
        no_prefix: bool,
        #[arg(long, num_args = 2, value_names = ["X", "Y"], default_values_t = [0.0, 0.0])]
        at: Vec<f64>,
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        #[arg(long, default_value_t = 0.0)]
        rotate: f64,
        #[arg(long)]
        expand_canvas: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long = "if-revision", visible_alias = "revision")]
        revision: Option<String>,
    },
    /// List or search layers and objects as compact JSON.
    Tree {
        input: PathBuf,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, value_enum)]
        kind: Option<SearchKind>,
        #[arg(long)]
        layer: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Search layers, object IDs, and text content as compact JSON.
    Search {
        input: PathBuf,
        query: String,
        #[arg(long, value_enum)]
        kind: Option<SearchKind>,
        #[arg(long)]
        layer: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Partially edit, rename, duplicate, move, reorder, or remove one object.
    Object {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: agent::ObjectAction,
    },
    /// Apply a JSON array of object operations as one recoverable transaction.
    Batch {
        input: PathBuf,
        operations: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long = "if-revision", visible_alias = "revision")]
        revision: Option<String>,
        /// Replace existing IDs in put-shape/put-path/put-text instead of failing (v4/v5).
        #[arg(long)]
        upsert: bool,
    },
    /// Create, update, or remove editable text.
    Text {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: text::TextAction,
    },
    /// Embed a custom TTF/OTF font or remove an unused font.
    Font {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: text::FontAction,
    },
    /// List available bundled, embedded, and installed font faces as JSON.
    Fonts { input: Option<PathBuf> },
    /// Transform all shapes and fills in one layer together.
    LayerGeometry {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[arg(long)]
        layer: String,
        #[command(subcommand)]
        operation: geometry::Operation,
    },
    /// Shared Rust geometry operations. Coordinates are in document units.
    Geometry {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        id: String,
        #[command(subcommand)]
        operation: geometry::Operation,
    },
    /// Launch the browser editor and JSON API.
    Serve {
        /// Open a shared document for browser viewing and editing.
        input: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(short, long, default_value_t = 4711)]
        port: u16,
    },
    /// Create an empty .pen document.
    New {
        output: PathBuf,
        #[arg(long, default_value_t = 1200)]
        width: u32,
        #[arg(long, default_value_t = 800)]
        height: u32,
        /// Replace an existing document; the previous bytes stay recoverable with `undo`
        #[arg(long)]
        overwrite: bool,
    },
    /// Print document metadata as JSON.
    Info { input: PathBuf },
    /// Explicitly migrate between compatible v3, v4, v5, v6, and v7 document formats.
    Migrate {
        input: PathBuf,
        #[arg(long, default_value_t = 4)]
        target: u32,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Create a semantic v4 rectangle, ellipse, circle, or line.
    #[command(allow_negative_numbers = true)]
    Shape {
        input: PathBuf,
        #[arg(value_enum)]
        kind: scene::ShapeKind,
        id: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        x: Option<f64>,
        #[arg(long)]
        y: Option<f64>,
        #[arg(long, visible_alias = "w")]
        width: Option<f64>,
        #[arg(long, visible_alias = "h")]
        height: Option<f64>,
        #[arg(long, visible_alias = "r")]
        radius: Option<f64>,
        #[arg(long)]
        cx: Option<f64>,
        #[arg(long)]
        cy: Option<f64>,
        #[arg(long)]
        radius_x: Option<f64>,
        #[arg(long)]
        radius_y: Option<f64>,
        #[arg(long)]
        x1: Option<f64>,
        #[arg(long)]
        y1: Option<f64>,
        #[arg(long)]
        x2: Option<f64>,
        #[arg(long)]
        y2: Option<f64>,
        #[arg(long)]
        fill: Option<String>,
        #[arg(long)]
        stroke: Option<String>,
        #[arg(long)]
        stroke_width: Option<f64>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Create, transform, duplicate, or ungroup semantic v4 groups.
    #[command(allow_negative_numbers = true)]
    Group(Box<GroupArgs>),
    /// Edit non-destructive adjustment nodes in an explicitly migrated v6 document.
    Adjustment(Box<AdjustmentArgs>),
    /// Create and attach reusable masks; apply explicitly bakes a content node.
    Mask(Box<MaskArgs>),
    /// Attach or detach explicit consecutive clipping-stack membership.
    Clip(Box<ClipArgs>),
    /// Edit an immutable-source affine/projective transform stack.
    Transform(Box<TransformArgs>),
    /// Add or edit a solid, linear/radial/conic gradient, or pattern fill node.
    Fill(Box<FillArgs>),
    /// Edit an ordered shadow, glow, stroke, overlay, or blur effect stack.
    Effect(Box<EffectArgs>),
    /// Query temporary selections or explicitly save/crop them.
    Selection(Box<SelectionArgs>),
    /// Inspect composite histograms, samples, palettes, and before/after pixels.
    Analyze(Box<AnalyzeArgs>),
    /// Maintain verified linked sources and collect portable projects offline.
    Linked(Box<LinkedArgs>),
    /// Save data-only appearance presets or copy/paste named appearances.
    Preset(Box<PresetArgs>),
    /// List, create, edit, and rebind named document design tokens.
    Style(Box<StyleArgs>),
    /// Replace fills or strokes within an explicit scene scope.
    Replace(Box<ReplaceArgs>),
    /// Serialize deterministically as compact or human-readable JSON.
    Format(Box<FormatArgs>),
    /// Compare document structure and optionally render bounded visual artifacts.
    Diff(Box<DiffArgs>),
    /// Align, distribute, or arrange scene nodes from measured bounds.
    #[command(allow_negative_numbers = true)]
    Layout(Box<LayoutArgs>),
    /// Create wrapped point or bounded text; y is a baseline unless --anchor changes it.
    #[command(allow_negative_numbers = true)]
    TextBox(Box<TextBoxArgs>),
    /// Edit primitive parameters, or explicitly convert a primitive to a path.
    #[command(allow_negative_numbers = true)]
    ShapeEdit(Box<ShapeEditArgs>),
    /// Show history, or prune it with `history prune FILE`.
    History {
        target: String,
        input: Option<PathBuf>,
        #[arg(long, default_value_t = 50)]
        keep: usize,
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore the preceding document revision.
    Undo { input: PathBuf },
    /// Reapply the next undone document revision.
    Redo { input: PathBuf },
    /// Restore an exact content-addressed document revision.
    Restore {
        input: PathBuf,
        #[arg(long)]
        revision: String,
    },
    /// Change canvas size, background, or document name.
    Canvas {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
        #[arg(long)]
        background: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Add, update, remove, or reorder a layer.
    Layer {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: editing::LayerAction,
    },
    /// Add, update, or remove an SVG-compatible vector path.
    Path {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: editing::PathAction,
    },
    /// Render a .pen document to PNG, SVG or PDF (selected by extension).
    Export {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        scale: f32,
        /// Convert text to glyph outlines for a font-independent SVG.
        #[arg(long)]
        outline_text: bool,
        /// Export every page; directories are used for SVG/PNG and one file for PDF.
        #[arg(long)]
        all_pages: bool,
        /// Output format when --all-pages targets a directory: png or svg.
        #[arg(long)]
        format: Option<String>,
        /// SVG only: reference verified external image files by relative href
        /// instead of embedding them. Fails for cropped or processed images.
        #[arg(long)]
        link_images: bool,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum AddMode {
    Copy,
    Instance,
}
#[derive(Clone, clap::ValueEnum)]
enum Scope {
    Project,
    User,
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum AssetAction {
    Create {
        input: PathBuf,
        output: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, default_value = "0.1.0")]
        version: String,
        #[arg(long, default_value = "component")]
        kind: String,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        license: Option<String>,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        source_page: Option<String>,
        #[arg(long)]
        layer: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        objects: Vec<String>,
        #[arg(long,num_args=4,value_names=["X","Y","WIDTH","HEIGHT"])]
        rect: Option<Vec<f64>>,
        /// Use the complete source canvas instead of inferred visible-content bounds.
        #[arg(long, conflicts_with = "rect")]
        canvas_bounds: bool,
        /// Include hidden layers when inferring content bounds.
        #[arg(long)]
        include_hidden: bool,
        /// Expose NAME=TYPE:OBJECT.FIELD; repeat for multiple properties.
        #[arg(long = "property")]
        property: Vec<String>,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        dry_run: bool,
    },
    Inspect {
        asset: String,
    },
    Preview {
        asset: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 1.0)]
        scale: f32,
    },
    /// Author and validate exposed instance properties.
    Property {
        #[command(subcommand)]
        action: AssetPropertyAction,
    },
}

#[derive(Subcommand)]
enum AssetPropertyAction {
    #[command(
        after_help = "Example:\n  pentool asset property add chrome.pen ai/chrome counter --target counter-label --field content --default \"1 / 6\""
    )]
    Add {
        input: PathBuf,
        asset_id: String,
        name: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        field: Option<String>,
        #[arg(long)]
        default: Option<String>,
        #[arg(long)]
        label: Option<String>,
        /// Complete property definition JSON; conflicts with target/field/default/label.
        #[arg(long, conflicts_with_all = ["target", "field", "default", "label"])]
        schema: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    Set {
        input: PathBuf,
        asset_id: String,
        name: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        field: Option<String>,
        #[arg(long)]
        default: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long, conflicts_with_all = ["target", "field", "default", "label"])]
        schema: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    Rename {
        input: PathBuf,
        asset_id: String,
        name: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    Remove {
        input: PathBuf,
        asset_id: String,
        name: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    List {
        input: PathBuf,
        asset_id: String,
    },
    Inspect {
        input: PathBuf,
        asset_id: String,
        name: String,
    },
    Usage {
        input: PathBuf,
        asset_id: String,
        name: String,
    },
    Validate {
        input: PathBuf,
        asset_id: String,
    },
}
#[derive(Subcommand)]
enum LibraryAction {
    Add {
        folder: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
    },
    List {
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
    },
    Refresh {
        name: Option<String>,
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
    },
    Remove {
        name: String,
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
    },
    Enable {
        name: String,
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
    },
    Disable {
        name: String,
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
    },
    Doctor {
        #[arg(long, value_enum, default_value = "project")]
        scope: Scope,
    },
}
#[derive(Subcommand)]
enum InstanceAction {
    List,
    Inspect {
        id: String,
    },
    Updates {
        id: String,
        #[arg(long)]
        source: PathBuf,
    },
    Update {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        id: Option<String>,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        asset: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        current_version: Option<String>,
        #[arg(long)]
        current_hash: Option<String>,
        #[arg(long)]
        package: Option<String>,
        #[arg(long)]
        stale_only: bool,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        continue_on_conflict: bool,
        /// JSON file containing explicit per-conflict resolutions.
        #[arg(long)]
        resolutions: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    Rollback {
        id: String,
        #[arg(long)]
        dry_run: bool,
    },
    Detach {
        id: String,
        #[arg(long)]
        dry_run: bool,
    },
    Set {
        id: String,
        #[arg(long)]
        property: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
}
#[derive(Subcommand)]
enum PackageAction {
    List,
    Remove {
        spec: String,
        #[arg(long)]
        force: bool,
    },
    Keygen {
        prefix: PathBuf,
    },
    Sign {
        file: PathBuf,
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Init {
        dir: PathBuf,
        #[arg(long)]
        name: String,
    },
    Pack {
        dir: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Verify {
        file: PathBuf,
        #[arg(long)]
        signature: Option<PathBuf>,
        #[arg(long)]
        public_key: Option<PathBuf>,
    },
    Inspect {
        file: PathBuf,
    },
    Publish {
        file: PathBuf,
        #[arg(long)]
        registry: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    Install {
        source: String,
        #[arg(long)]
        registry: Option<String>,
    },
}
#[derive(Subcommand)]
enum RegistryAction {
    Search { path: String, query: String },
}
#[derive(Subcommand)]
enum LockAction {
    Verify,
    Sync {
        #[arg(long)]
        offline: bool,
    },
}

fn main() {
    if let Err(error) = cli_main() {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

fn cli_main() -> Result<()> {
    let wants_json = std::env::args_os().any(|arg| arg == "--json");
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !panic_message(info.payload())
            .to_ascii_lowercase()
            .contains("broken pipe")
        {
            default_hook(info);
        }
    }));
    let joined = std::thread::Builder::new()
        .name("pentool-cli".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(run)?
        .join();
    let result = match joined {
        Ok(result) => result,
        Err(payload)
            if panic_message(payload.as_ref())
                .to_ascii_lowercase()
                .contains("broken pipe") =>
        {
            return Ok(())
        }
        Err(_) => return Err(anyhow::anyhow!("pentool CLI thread panicked")),
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if is_broken_pipe(&error) => Ok(()),
        Err(error) if wants_json => {
            eprintln!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "ok":false,
                    "error":ai_error_json(&error)
                }))?
            );
            std::process::exit(2)
        }
        Err(error) => Err(error),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|value| (*value).into()))
        .unwrap_or_default()
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    })
}
/// Region options shared by `fill` and `select-wand`.
#[derive(Args, Clone)]
struct FloodArgs {
    id: String,
    /// Seed pixel, in layer pixels.
    #[arg(long)]
    x: u32,
    #[arg(long)]
    y: u32,
    /// Largest straight-RGBA channel difference from the seed (0-255).
    #[arg(long, default_value_t = 32)]
    tolerance: u8,
    /// Grow through diagonal neighbors too (8-connected).
    #[arg(long)]
    diagonal: bool,
    /// Take every similar pixel in the layer, not only those connected to the seed.
    #[arg(long)]
    global: bool,
    /// Hard edges instead of the 3x3 anti-aliased edge.
    #[arg(long)]
    no_antialias: bool,
    /// Stop at breaks up to 2N pixels wide (0-8, contiguous only).
    #[arg(long, default_value_t = 0)]
    gap: u32,
    /// Never let fully transparent pixels join an opaque seed's region.
    #[arg(long)]
    transparent_barrier: bool,
    /// Measure the region on `layer` (default) or on the visible page `composite`.
    #[arg(long, default_value = "layer", value_parser = ["layer", "composite"])]
    scope: String,
}

impl FloodArgs {
    /// The visible page cropped to the layer when `--scope composite` is used.
    fn sample(
        &self,
        raw: &serde_json::Value,
        document: &std::path::Path,
        page: Option<&str>,
    ) -> Result<Option<raster::Surface>> {
        (self.scope == "composite")
            .then(|| raster::sample_page(raw, document, page, &self.id, raster::Scope::Composite))
            .transpose()
    }

    fn options(&self) -> raster::FloodOptions {
        raster::FloodOptions {
            x: self.x,
            y: self.y,
            tolerance: self.tolerance,
            diagonal: self.diagonal,
            contiguous: !self.global,
            antialias: !self.no_antialias,
            gap: self.gap,
            transparent_barrier: self.transparent_barrier,
        }
    }
}

#[derive(Subcommand)]
enum RasterAction {
    /// Create an empty raster layer (upgrades the document to v6).
    Add {
        id: String,
        #[arg(long, default_value_t = 0.0)]
        x: f64,
        #[arg(long, default_value_t = 0.0)]
        y: f64,
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
        #[arg(long)]
        layer: Option<String>,
    },
    /// Report size, tile count, journal length and the tile-map hash.
    Info { id: String },
    /// Remove all pixels and the stroke journal.
    Clear { id: String },
    /// Record a checkpoint; --compact drops the replayable journal.
    Checkpoint {
        id: String,
        #[arg(long)]
        compact: bool,
    },
    /// Check every tile against its digest; --replay also proves the journal
    /// still reaches the live pixels. Read-only; exits nonzero when damaged.
    Verify {
        /// One raster layer; omit to check every raster on the selected page(s).
        id: Option<String>,
        #[arg(long)]
        replay: bool,
    },
    /// Rebuild damaged tiles: `replay` re-runs the checkpoint and journal exactly;
    /// `transparent` drops unrecoverable tiles and records them in the checkpoint.
    Repair {
        id: String,
        #[arg(long, default_value = "replay")]
        strategy: String,
    },
    /// Scale a layer's pixels to a new size (bilinear or nearest); position is kept.
    Resize {
        id: String,
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
        #[arg(long, default_value = "bilinear")]
        resample: String,
    },
    /// Set a layer's bounds to a layer-local rectangle without resampling. The
    /// rectangle may extend past the old bounds, which grows the layer canvas.
    Crop {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        x: i64,
        #[arg(long, allow_hyphen_values = true)]
        y: i64,
        #[arg(long)]
        width: i64,
        #[arg(long)]
        height: i64,
    },
    /// Shrink a layer to the bounds of its painted pixels.
    Trim { id: String },
    /// Copy a raster layer directly above itself; tiles are shared, not copied.
    Duplicate {
        id: String,
        #[arg(long)]
        new_id: String,
    },
    /// Merge a raster layer into the raster layer directly beneath it.
    MergeDown { id: String },
    /// Bake any visible node into a new raster layer placed above it. The source
    /// is hidden, not deleted, unless --replace is given.
    Rasterize {
        id: String,
        #[arg(long)]
        new_id: String,
        #[arg(long)]
        replace: bool,
    },
    /// Register a textured brush tip from a PNG, JPEG or WebP image. The image is
    /// fitted into 256x256; use the name or returned digest as the brush "tip".
    TipAdd {
        name: String,
        #[arg(long)]
        image: PathBuf,
        /// darkness (black paints) or alpha (opaque paints).
        #[arg(long, default_value = "darkness")]
        source: String,
    },
    /// Remove a tip name; its pixels stay while a journal entry still uses them.
    TipRemove { name: String },
    /// List named brush tips.
    Tips,
    /// Apply a JSON array of raster operations ({action, id, args}) atomically.
    Batch { operations: PathBuf },
    /// Create a brush preset from brush JSON.
    PresetAdd {
        name: String,
        #[arg(long, default_value = "{}")]
        brush: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        replace: bool,
    },
    /// Delete a brush preset.
    PresetRemove { name: String },
    /// List brush presets.
    Presets,
    /// Show one brush preset.
    PresetShow { name: String },
    /// Write a portable preset file (with its tip pixels) to --out.
    PresetExport {
        name: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Import a pentool preset file.
    PresetImport {
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        replace: bool,
    },
    /// Import a GIMP .gbr brush as a textured preset and tip (opt-in conversion).
    PresetImportGbr { file: PathBuf, name: String },
    /// Import a MyPaint .myb brush; unsupported settings are reported, not guessed.
    PresetImportMypaint { file: PathBuf, name: String },
    /// Apply one deterministic brush stroke.
    Stroke {
        id: String,
        /// JSON array of device events ([x, y], [x, y, pressure] or objects with
        /// tilt, azimuth, twist, velocity, t), or @file.json.
        #[arg(long)]
        samples: String,
        /// Brush JSON (kind, size, hardness, spacing, opacity, flow, ...), or @file.json.
        #[arg(long, default_value = "{}")]
        brush: String,
        /// Name of a document brush preset to start from; --brush overrides it.
        #[arg(long)]
        preset: Option<String>,
        #[arg(long, default_value = "#000000")]
        color: String,
        /// normal, erase, background-erase, smudge, blur, sharpen, dodge, burn,
        /// sponge or color-replace.
        #[arg(long, default_value = "normal")]
        blend: String,
        #[arg(long, default_value_t = 0)]
        seed: u64,
    },
    /// Choose where clone strokes on a layer copy from (resets the aligned anchor).
    CloneSource {
        /// Target raster layer that clone strokes paint into.
        id: String,
        /// Raster layer to sample; omit to sample the target before each stroke.
        #[arg(long)]
        layer: Option<String>,
        #[arg(long, allow_negative_numbers = true)]
        x: f64,
        #[arg(long, allow_negative_numbers = true)]
        y: f64,
    },
    /// Clone-stamp from the stored source. Dab, opacity, flow and tip options come
    /// from --brush; source tiles of another layer are pinned in the journal.
    CloneStroke {
        id: String,
        /// Device events, as for `stroke`, or @file.json.
        #[arg(long)]
        samples: String,
        #[arg(long, default_value = "{}")]
        brush: String,
        /// Name of a document brush preset to start from; --brush overrides it.
        #[arg(long)]
        preset: Option<String>,
        /// Keep one source-to-destination offset across strokes instead of
        /// restarting at the source point each stroke.
        #[arg(long)]
        aligned: bool,
        /// Rotate the cloned content clockwise, in degrees (-360 to 360).
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        angle: f64,
        /// Scale the cloned content (0.1 to 10).
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        #[arg(long, default_value_t = 0)]
        seed: u64,
    },
    /// Healing brush: clone-stamp from the stored source, then match tone to the
    /// surroundings. Brush `texture` and `tone` (0-1) blend detail and tone.
    HealStroke {
        id: String,
        #[arg(long)]
        samples: String,
        #[arg(long, default_value = "{}")]
        brush: String,
        /// Name of a document brush preset to start from; --brush overrides it.
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        aligned: bool,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        angle: f64,
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        #[arg(long, default_value_t = 0)]
        seed: u64,
    },
    /// Flood-fill a region with a color; the active selection limits the result.
    Fill {
        #[command(flatten)]
        region: FloodArgs,
        #[arg(long)]
        color: String,
        #[arg(long, default_value_t = 1.0)]
        opacity: f64,
    },
    /// Select a flood region (magic wand) as the layer's selection.
    SelectWand {
        #[command(flatten)]
        region: FloodArgs,
        /// replace, add, subtract or intersect.
        #[arg(long, default_value = "replace")]
        mode: String,
    },
    /// Rectangular or elliptical marquee selection, in layer pixels.
    SelectMarquee {
        id: String,
        /// rect or ellipse.
        #[arg(long, default_value = "rect")]
        shape: String,
        /// X Y WIDTH HEIGHT of the marquee.
        #[arg(long, num_args = 4, allow_negative_numbers = true, value_names = ["X", "Y", "WIDTH", "HEIGHT"])]
        rect: Vec<f64>,
        /// replace, add, subtract or intersect.
        #[arg(long, default_value = "replace")]
        mode: String,
        /// Soften the edge by this many pixels (0-256).
        #[arg(long, default_value_t = 0)]
        feather: u32,
    },
    /// Free-form polygon (lasso) selection from a JSON array of [x, y] points.
    SelectLasso {
        id: String,
        #[arg(long)]
        points: String,
        #[arg(long, default_value = "replace")]
        mode: String,
        #[arg(long, default_value_t = 0)]
        feather: u32,
    },
    /// Change the selection: feather, expand, contract, smooth, border, grow,
    /// similar or invert.
    SelectModify {
        id: String,
        #[arg(long)]
        op: String,
        /// Pixels for feather/expand/contract/smooth/border/grow.
        #[arg(long, default_value_t = 1)]
        amount: u32,
        /// Color tolerance for grow and similar (0-255).
        #[arg(long, default_value_t = 32)]
        tolerance: u8,
    },
    /// Quick-mask painting: a brush stroke adds to (or with --erase, removes from)
    /// the selection.
    SelectQuickmask {
        id: String,
        #[arg(long)]
        samples: String,
        #[arg(long, default_value = "{}")]
        brush: String,
        /// Name of a document brush preset to start from; --brush overrides it.
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        erase: bool,
        #[arg(long, default_value_t = 0)]
        seed: u64,
    },
    /// Save the active selection under a name.
    SelectSave { id: String, name: String },
    /// Load a saved selection, combined with the current one by --mode.
    SelectLoad {
        id: String,
        name: String,
        #[arg(long, default_value = "replace")]
        mode: String,
    },
    /// Delete a saved selection.
    SelectDelete { id: String, name: String },
    /// Copy (or with --cut, cut) the selected pixels into a new raster layer.
    Lift {
        id: String,
        #[arg(long)]
        new_id: String,
        #[arg(long)]
        cut: bool,
    },
    /// Move the selected pixels and their selection by whole pixels.
    MovePixels {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        dx: i64,
        #[arg(long, allow_hyphen_values = true)]
        dy: i64,
        /// Leave the original pixels in place.
        #[arg(long)]
        copy: bool,
    },
    /// Scale and rotate the selected pixels about their center, then shift them.
    TransformPixels {
        id: String,
        #[arg(long, default_value_t = 1.0)]
        scale: f64,
        /// Vertical scale; defaults to --scale.
        #[arg(long)]
        scale_y: Option<f64>,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        rotate: f64,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        dx: f64,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        dy: f64,
        #[arg(long)]
        nearest: bool,
        #[arg(long)]
        copy: bool,
    },
    /// Paste another raster layer's pixels into this layer at (x, y).
    Paste {
        id: String,
        #[arg(long)]
        source: String,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        x: i64,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        y: i64,
        #[arg(long, default_value_t = 1.0)]
        opacity: f64,
    },
    /// Rotate a raster layer by a quarter turn clockwise (90, 180 or 270).
    Rotate {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        degrees: i64,
    },
    /// Mirror a raster layer (horizontal or vertical).
    Flip { id: String, axis: String },
    /// Merge all visible raster siblings of a layer into the bottom-most one.
    MergeVisible { id: String },
    /// Bake everything visible on the page into a new top raster layer.
    StampVisible {
        #[arg(long)]
        new_id: String,
    },
    /// Replace every visible node of the page with one raster of their composite.
    Flatten {
        #[arg(long)]
        new_id: String,
    },
    /// Remove the layer's selection.
    SelectClear { id: String },
    /// Summarize the layer's selection.
    SelectInfo { id: String },
    /// Patch healing: repair the layer's selection with texture from `--dx`,`--dy` away.
    HealPatch {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        dx: f64,
        #[arg(long, allow_hyphen_values = true)]
        dy: f64,
        #[arg(long)]
        texture: Option<f64>,
        #[arg(long)]
        tone: Option<f64>,
        #[arg(long, default_value_t = 0)]
        seed: u64,
    },
    /// Spot healing: repair one round spot, choosing the source automatically.
    HealSpot {
        id: String,
        #[arg(long)]
        x: f64,
        #[arg(long)]
        y: f64,
        /// Spot radius in pixels (1-256).
        #[arg(long, default_value_t = 8.0)]
        radius: f64,
        #[arg(long)]
        texture: Option<f64>,
        #[arg(long)]
        tone: Option<f64>,
        #[arg(long, default_value_t = 0)]
        seed: u64,
    },
}

fn ai_error_json(error: &anyhow::Error) -> serde_json::Value {
    if let Some(ai) = error.chain().find_map(|e| e.downcast_ref::<ai::AiError>()) {
        return serde_json::json!({"code":ai.code,"message":ai.message,"fix":ai.fix});
    }
    serde_json::json!({"code":error_code(error),"message":error.to_string(),"context":format!("{error:#}"),"suggestions":error_suggestions(error),"hint":"Run the command with --help and verify page, layer, group, and object IDs."})
}

fn error_code(error: &anyhow::Error) -> &'static str {
    let message = error.to_string();
    if message.contains("not found") {
        "not_found"
    } else if message.contains("revision mismatch") {
        "revision_conflict"
    } else if message.contains("invalid")
        || message.contains("must")
        || message.contains("required")
    {
        "invalid_input"
    } else {
        "operation_failed"
    }
}
fn error_suggestions(error: &anyhow::Error) -> Vec<String> {
    error
        .to_string()
        .split("nearest ID: ")
        .nth(1)
        .map(|value| vec![value.trim().to_owned()])
        .unwrap_or_default()
}

/// Parse the command line; `PENTOOL_AI=off` also hides the `ai` command from help.
fn parse_cli() -> Result<Cli, clap::Error> {
    use clap::{CommandFactory, FromArgMatches};
    let mut command = Cli::command();
    if ai::disabled() {
        command = command.mut_subcommand("ai", |sub| sub.hide(true));
    }
    let matches = command.try_get_matches()?;
    Cli::from_arg_matches(&matches)
}

#[tokio::main]
async fn run() -> Result<()> {
    let cli = match parse_cli() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            return Ok(());
        }
        Err(error) => return Err(anyhow::anyhow!(error.to_string())),
    };
    let _json_output = cli.json;
    let selected_page = cli.page.as_deref();
    match cli.command.unwrap_or(Command::Serve {
        input: None,
        host: "127.0.0.1".into(),
        port: 4711,
    }) {
        Command::Raw { action } => match action {
            RawAction::Add {
                input,
                id,
                file,
                embed: _,
                external,
                camera_profile,
                name,
                dry_run,
                if_revision,
            } => {
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let source_path = if external {
                    let relative = pentool::resource::safe_relative_path(&file)?;
                    let root = pentool::resource::document_root(&input).canonicalize()?;
                    let source = root.join(&relative).canonicalize().with_context(|| {
                        format!("[missing-resource] {}", root.join(&relative).display())
                    })?;
                    if !source.starts_with(&root) {
                        anyhow::bail!(
                            "[unsafe-path] external raw source resolves outside the document root"
                        )
                    }
                    source
                } else {
                    file.clone()
                };
                let length = fs::metadata(&source_path)
                    .with_context(|| format!("[missing-resource] {}", source_path.display()))?
                    .len();
                if length > pentool::photo::catalog::MAX_PHOTO_SOURCE_BYTES {
                    anyhow::bail!("[limit-exceeded] raw source is larger than 512 MiB")
                }
                if !external && length > image::MAX_SOURCE_BYTES {
                    anyhow::bail!("[limit-exceeded] raw source is larger than the 128 MiB embedding limit; pass --external with a document-relative path")
                }
                let bytes = fs::read(&source_path)
                    .with_context(|| format!("[missing-resource] {}", source_path.display()))?;
                let storage = if external {
                    image::external_storage(&pentool::resource::safe_relative_path(&file)?)?
                } else {
                    image::embedded_storage(&bytes)
                };
                let name = name.unwrap_or_else(|| {
                    file.file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_else(|| id.clone())
                });
                let result = pentool::photo::catalog::add_raw(
                    &mut raw,
                    &id,
                    &name,
                    &bytes,
                    storage,
                    pentool::photo::catalog::camera_profile(&camera_profile)?,
                )?;
                let change = transaction::commit_value(
                    &input,
                    "raw-add",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                Ok(())
            }
            RawAction::Info { input, id } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&pentool::photo::catalog::info(&raw, &id)?)?
                );
                Ok(())
            }
            RawAction::Develop {
                input,
                id,
                variant,
                camera_profile,
                as_shot,
                temperature,
                tint,
                neutral,
                sample,
                suggest,
                dry_run,
                if_revision,
            } => {
                use pentool::photo::catalog::WhiteBalance;
                let triple = |text: &str, flag: &str| -> anyhow::Result<[f64; 3]> {
                    let values = text
                        .split(',')
                        .map(|v| v.trim().parse::<f64>())
                        .collect::<Result<Vec<_>, _>>()
                        .ok()
                        .filter(|v| v.len() == 3 && v.iter().all(|v| v.is_finite()));
                    match values {
                        Some(v) => Ok([v[0], v[1], v[2]]),
                        None => anyhow::bail!(
                            "[invalid-input] {flag} takes three comma-separated numbers"
                        ),
                    }
                };
                let white_balance = if as_shot {
                    Some(WhiteBalance::AsShot)
                } else if let Some(temperature) = temperature {
                    Some(WhiteBalance::Temperature {
                        temperature,
                        tint: tint.unwrap_or(0.0),
                    })
                } else if let Some(neutral) = neutral {
                    Some(WhiteBalance::Neutral(triple(&neutral, "--neutral")?))
                } else if let Some(sample) = sample {
                    let [x, y, radius] = triple(&sample, "--sample")?;
                    Some(WhiteBalance::Sample { x, y, radius })
                } else if suggest {
                    Some(WhiteBalance::Suggest)
                } else {
                    None
                };
                let camera_profile = camera_profile
                    .as_deref()
                    .map(pentool::photo::catalog::camera_profile)
                    .transpose()?;
                if camera_profile.is_none() && white_balance.is_none() {
                    anyhow::bail!("[invalid-input] raw develop needs a setting: --camera-profile, --as-shot, --temperature, --neutral, --sample or --suggest")
                }
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result = pentool::photo::catalog::develop_raw(
                    &mut raw,
                    &input,
                    &id,
                    &variant,
                    camera_profile,
                    white_balance,
                )?;
                let change = transaction::commit_value(
                    &input,
                    "raw-develop",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                Ok(())
            }
        },
        Command::Photo { action } => match action {
            PhotoAction::Profile {
                action:
                    PhotoProfileAction::Add {
                        input,
                        file,
                        force_model,
                        dry_run,
                        if_revision,
                    },
            } => {
                let length = fs::metadata(&file)
                    .with_context(|| format!("[missing-resource] {}", file.display()))?
                    .len();
                if length > pentool::photo::catalog::MAX_PROFILE_BYTES {
                    anyhow::bail!("[limit-exceeded] camera profile is larger than 16 MiB")
                }
                let bytes = fs::read(&file)
                    .with_context(|| format!("[missing-resource] {}", file.display()))?;
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result = pentool::photo::catalog::add_profile(&mut raw, &bytes, force_model)?;
                let change = transaction::commit_value(
                    &input,
                    "photo-profile-add",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                Ok(())
            }
        },
        Command::Image { action } => match action {
            ImageAction::Op { action } => {
                let (input, name, dry_run, if_revision) = match &action {
                    OpAction::List { input, id } => {
                        let raw: serde_json::Value = serde_json::from_slice(&fs::read(input)?)?;
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&image::op_list(
                                &raw,
                                selected_page,
                                id
                            )?)?
                        );
                        return Ok(());
                    }
                    OpAction::Add {
                        input,
                        dry_run,
                        if_revision,
                        ..
                    } => (input.clone(), "image-op-add", *dry_run, if_revision.clone()),
                    OpAction::Set {
                        input,
                        dry_run,
                        if_revision,
                        ..
                    } => (input.clone(), "image-op-set", *dry_run, if_revision.clone()),
                    OpAction::Move {
                        input,
                        dry_run,
                        if_revision,
                        ..
                    } => (
                        input.clone(),
                        "image-op-move",
                        *dry_run,
                        if_revision.clone(),
                    ),
                    OpAction::Enable {
                        input,
                        dry_run,
                        if_revision,
                        ..
                    } => (
                        input.clone(),
                        "image-op-enable",
                        *dry_run,
                        if_revision.clone(),
                    ),
                    OpAction::Disable {
                        input,
                        dry_run,
                        if_revision,
                        ..
                    } => (
                        input.clone(),
                        "image-op-disable",
                        *dry_run,
                        if_revision.clone(),
                    ),
                    OpAction::Remove {
                        input,
                        dry_run,
                        if_revision,
                        ..
                    } => (
                        input.clone(),
                        "image-op-remove",
                        *dry_run,
                        if_revision.clone(),
                    ),
                };
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result = match action {
                    OpAction::Add {
                        id,
                        kind,
                        op_id,
                        index,
                        flags,
                        ..
                    } => image::op_add(
                        &mut raw,
                        selected_page,
                        &id,
                        &kind,
                        op_id.as_deref(),
                        index,
                        flags.into_params()?,
                    )?,
                    OpAction::Set {
                        id, op_id, flags, ..
                    } => image::op_set(&mut raw, selected_page, &id, &op_id, flags.into_params()?)?,
                    OpAction::Move {
                        id, op_id, index, ..
                    } => image::op_move(&mut raw, selected_page, &id, &op_id, index)?,
                    OpAction::Enable { id, op_id, .. } => {
                        image::op_enable(&mut raw, selected_page, &id, &op_id, true)?
                    }
                    OpAction::Disable { id, op_id, .. } => {
                        image::op_enable(&mut raw, selected_page, &id, &op_id, false)?
                    }
                    OpAction::Remove { id, op_id, .. } => {
                        image::op_remove(&mut raw, selected_page, &id, &op_id)?
                    }
                    OpAction::List { .. } => unreachable!(),
                };
                let change =
                    transaction::commit_value(&input, name, dry_run, if_revision.as_deref(), &raw)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                Ok(())
            }
            ImageAction::Bake {
                input,
                id,
                format,
                strip_metadata: _,
                dry_run,
                if_revision,
            } => {
                if format != "png" {
                    anyhow::bail!(
                        "[unsupported-capability] bake format {format} is unsupported; use png"
                    )
                }
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result = image::bake(&mut raw, selected_page, &input, &id, !dry_run)?;
                let change = transaction::commit_value(
                    &input,
                    "image-bake",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                Ok(())
            }
            ImageAction::Cache { input } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let root = pentool::resource::document_root(&input);
                let mut cached = vec![];
                for (digest, asset) in raw["image_assets"].as_object().into_iter().flatten() {
                    if asset["storage"]["kind"] == "external" {
                        let path = asset["storage"]["path"].as_str().unwrap_or_default();
                        let bytes = pentool::resource::read_external_offline(
                            &input,
                            std::path::Path::new(path),
                            digest,
                        )?;
                        pentool::resource::cache_store(root, &bytes)?;
                        cached.push(serde_json::json!({"asset": digest, "path": path, "bytes": bytes.len()}));
                    }
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({"cached": cached}))?
                );
                Ok(())
            }
            ImageAction::Palette {
                input,
                id,
                prefix,
                count,
                dry_run,
                if_revision,
            } => {
                if !(1..=5).contains(&count) || prefix.is_empty() {
                    anyhow::bail!(
                        "[invalid-operation] palette needs a non-empty --prefix and --count 1-5"
                    )
                }
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let analysis = image::analyze(&raw, selected_page, &input, &id)?;
                let colors = analysis["dominant_colors"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let mut tokens = Vec::new();
                for (index, color) in colors.iter().take(count).enumerate() {
                    let hex = color["hex"].as_str().context("analysis color is missing")?;
                    let name = format!("{prefix}-{}", index + 1);
                    style::apply(
                        &mut raw,
                        style::Operation::Set,
                        Some(&name),
                        Some("color"),
                        Some(hex),
                        None,
                    )?;
                    tokens.push(serde_json::json!({"token":name,"value":hex}));
                }
                let change = transaction::commit_value(
                    &input,
                    "image-palette",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":{"image":id,"tokens":tokens}})
                    )?
                );
                Ok(())
            }
            ImageAction::Analyze { input, id } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&image::analyze(
                        &raw,
                        selected_page,
                        &input,
                        &id
                    )?)?
                );
                Ok(())
            }
            ImageAction::Add {
                input,
                id,
                file,
                layer,
                x,
                y,
                width,
                height,
                fit,
                embed: _,
                external,
                mask,
                mask_fill_rule,
                dry_run,
                if_revision,
            } => {
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let (source_path, storage) = if external {
                    let relative = pentool::resource::safe_relative_path(&file)?;
                    let root = pentool::resource::document_root(&input).canonicalize()?;
                    let source = root.join(&relative).canonicalize().with_context(|| {
                        format!("[missing-resource] {}", root.join(&relative).display())
                    })?;
                    if !source.starts_with(&root) {
                        anyhow::bail!(
                            "[unsafe-path] external image resolves outside the document root"
                        )
                    }
                    (source, image::external_storage(&relative)?)
                } else {
                    let bytes = fs::read(&file)
                        .with_context(|| format!("[missing-resource] {}", file.display()))?;
                    (file.clone(), image::embedded_storage(&bytes))
                };
                let bytes = fs::read(&source_path)
                    .with_context(|| format!("[missing-resource] {}", source_path.display()))?;
                let result = image::add(
                    &mut raw,
                    selected_page,
                    &layer,
                    &id,
                    &bytes,
                    storage,
                    x,
                    y,
                    width,
                    height,
                    fit,
                )?;
                if let Some(node) = mask {
                    let update = image::Update {
                        mask: Some((node, mask_fill_rule)),
                        ..Default::default()
                    };
                    image::set(&mut raw, selected_page, &id, &update)?;
                }
                let change = transaction::commit_value(
                    &input,
                    "image-add",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                Ok(())
            }
            ImageAction::Info { input, id } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&image::info(&raw, selected_page, &id)?)?
                );
                Ok(())
            }
            ImageAction::Set {
                input,
                id,
                x,
                y,
                width,
                height,
                fit,
                position,
                crop,
                opacity,
                transform,
                mask,
                mask_fill_rule,
                clear_mask,
                dry_run,
                if_revision,
            } => {
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let update = image::Update {
                    x,
                    y,
                    width,
                    height,
                    fit,
                    position: position.map(|values| [values[0], values[1]]),
                    crop: crop.map(|values| [values[0], values[1], values[2], values[3]]),
                    opacity,
                    transform: transform.map(|values| {
                        [
                            values[0], values[1], values[2], values[3], values[4], values[5],
                        ]
                    }),
                    mask: mask.map(|node| (node, mask_fill_rule)),
                    clear_mask,
                };
                let result = image::set(&mut raw, selected_page, &id, &update)?;
                let change = transaction::commit_value(
                    &input,
                    "image-set",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                Ok(())
            }
            ImageAction::Remove {
                input,
                id,
                dry_run,
                if_revision,
            } => {
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result = image::remove(&mut raw, selected_page, &id)?;
                let change = transaction::commit_value(
                    &input,
                    "image-remove",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                Ok(())
            }
        },
        Command::Asset { action } => {
            let cwd = std::env::current_dir()?;
            match action {
                AssetAction::Create {
                    input,
                    output,
                    id,
                    name,
                    description,
                    version,
                    kind,
                    author,
                    license,
                    tag,
                    category,
                    source_page,
                    layer,
                    objects,
                    rect,
                    canvas_bounds,
                    include_hidden,
                    property,
                    overwrite,
                    dry_run,
                } => {
                    let rect = rect.map(|v| asset::Bounds {
                        x: v[0],
                        y: v[1],
                        width: v[2],
                        height: v[3],
                    });
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&asset::create(&asset::CreateOptions {
                            input,
                            output,
                            page: source_page,
                            layers: layer,
                            objects,
                            rect,
                            canvas_bounds,
                            include_hidden,
                            properties: property,
                            id,
                            name,
                            description: description.unwrap_or_default(),
                            version,
                            kind,
                            author: author.unwrap_or_default(),
                            license: license.unwrap_or_default(),
                            tags: tag,
                            category: category.unwrap_or_default(),
                            overwrite,
                            dry_run
                        })?)?
                    );
                    Ok(())
                }
                AssetAction::Inspect { asset: spec } => {
                    let path = resolve_asset_path(&cwd, &spec)?;
                    println!("{}", serde_json::to_string_pretty(&asset::inspect(&path)?)?);
                    Ok(())
                }
                AssetAction::Preview {
                    asset: spec,
                    output,
                    scale,
                } => {
                    let path = resolve_asset_path(&cwd, &spec)?;
                    let source_bytes = fs::read(&path)?;
                    let hash = asset::hash_bytes(&source_bytes);
                    let extension = output
                        .extension()
                        .and_then(|s| s.to_str())
                        .context("preview output must end in .png or .svg")?;
                    let cache = cwd.join(".pentool").join("previews").join(format!(
                        "{}-{}x.{}",
                        hash.trim_start_matches("sha256:"),
                        scale,
                        extension
                    ));
                    if !cache.exists() {
                        if let Some(parent) = cache.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let doc = read_document(&path, None)?;
                        render::write_export_options(&doc, &cache, scale, false)?;
                    }
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(&cache, &output)?;
                    println!(
                        "{}",
                        json_pretty(
                            serde_json::json!({"ok":true,"asset":spec,"output":output,"cache":cache,"content_hash":hash})
                        )?
                    );
                    Ok(())
                }
                AssetAction::Property { action } => {
                    let result = match action {
                        AssetPropertyAction::Add {
                            input,
                            asset_id,
                            name,
                            target,
                            field,
                            default,
                            label,
                            schema,
                            dry_run,
                            if_revision,
                        } => {
                            let definition = asset::property_definition(
                                schema.as_deref(),
                                target.as_deref(),
                                field.as_deref(),
                                default.as_deref(),
                                label.as_deref(),
                            )?;
                            asset::property_write(
                                &input,
                                &asset_id,
                                &name,
                                definition,
                                false,
                                dry_run,
                                if_revision.as_deref(),
                            )?
                        }
                        AssetPropertyAction::Set {
                            input,
                            asset_id,
                            name,
                            target,
                            field,
                            default,
                            label,
                            schema,
                            dry_run,
                            if_revision,
                        } => {
                            let definition = asset::property_definition(
                                schema.as_deref(),
                                target.as_deref(),
                                field.as_deref(),
                                default.as_deref(),
                                label.as_deref(),
                            )?;
                            asset::property_write(
                                &input,
                                &asset_id,
                                &name,
                                definition,
                                true,
                                dry_run,
                                if_revision.as_deref(),
                            )?
                        }
                        AssetPropertyAction::Rename {
                            input,
                            asset_id,
                            name,
                            to,
                            dry_run,
                            if_revision,
                        } => asset::property_rename(
                            &input,
                            &asset_id,
                            &name,
                            &to,
                            dry_run,
                            if_revision.as_deref(),
                        )?,
                        AssetPropertyAction::Remove {
                            input,
                            asset_id,
                            name,
                            dry_run,
                            if_revision,
                        } => asset::property_remove(
                            &input,
                            &asset_id,
                            &name,
                            dry_run,
                            if_revision.as_deref(),
                        )?,
                        AssetPropertyAction::List { input, asset_id } => {
                            asset::property_list(&input, &asset_id)?
                        }
                        AssetPropertyAction::Inspect {
                            input,
                            asset_id,
                            name,
                        } => asset::property_inspect(&input, &asset_id, &name)?,
                        AssetPropertyAction::Usage {
                            input,
                            asset_id,
                            name,
                        } => asset::property_usage(&input, &asset_id, &name)?,
                        AssetPropertyAction::Validate { input, asset_id } => {
                            asset::property_validate(&input, &asset_id)?
                        }
                    };
                    println!("{}", json_pretty(result)?);
                    Ok(())
                }
            }
        }
        Command::Library { action } => {
            let cwd = std::env::current_dir()?;
            match action {
                LibraryAction::Add {
                    folder,
                    name,
                    scope,
                } => {
                    let (config, index) = scope_paths(&cwd, &scope)?;
                    println!(
                        "{}",
                        json_pretty(library::add(&cwd, &config, &name, &folder)?)?
                    );
                    println!(
                        "{}",
                        json_pretty(library::refresh(&cwd, &config, &index, None)?)?
                    );
                    Ok(())
                }
                LibraryAction::List { scope } => {
                    let (config, _) = scope_paths(&cwd, &scope)?;
                    println!("{}", json_pretty(library::list(&cwd, &config)?)?);
                    Ok(())
                }
                LibraryAction::Refresh { name, scope } => {
                    let (config, index) = scope_paths(&cwd, &scope)?;
                    println!(
                        "{}",
                        json_pretty(library::refresh(&cwd, &config, &index, name.as_deref())?)?
                    );
                    Ok(())
                }
                LibraryAction::Remove { name, scope } => {
                    let (config, index) = scope_paths(&cwd, &scope)?;
                    println!("{}", json_pretty(library::remove(&config, &name)?)?);
                    println!(
                        "{}",
                        json_pretty(library::refresh(&cwd, &config, &index, None)?)?
                    );
                    Ok(())
                }
                LibraryAction::Enable { name, scope } => {
                    let (config, index) = scope_paths(&cwd, &scope)?;
                    println!(
                        "{}",
                        json_pretty(library::set_enabled(&config, &name, true)?)?
                    );
                    println!(
                        "{}",
                        json_pretty(library::refresh(&cwd, &config, &index, None)?)?
                    );
                    Ok(())
                }
                LibraryAction::Disable { name, scope } => {
                    let (config, index) = scope_paths(&cwd, &scope)?;
                    println!(
                        "{}",
                        json_pretty(library::set_enabled(&config, &name, false)?)?
                    );
                    println!(
                        "{}",
                        json_pretty(library::refresh(&cwd, &config, &index, None)?)?
                    );
                    Ok(())
                }
                LibraryAction::Doctor { scope } => {
                    let (_, index) = scope_paths(&cwd, &scope)?;
                    let idx = library::load_index(&index)?;
                    println!(
                        "{}",
                        json_pretty(
                            serde_json::json!({"ok":idx.warnings.is_empty(),"assets":idx.assets.len(),"warnings":idx.warnings})
                        )?
                    );
                    Ok(())
                }
            }
        }
        Command::Explore {
            query,
            library: lib,
            tag,
            category,
            kind,
            offset,
            limit,
        } => {
            let cwd = std::env::current_dir()?;
            let idx = all_indexes(&cwd)?;
            println!(
                "{}",
                json_pretty(library::search(
                    &idx,
                    &library::SearchOptions {
                        query: query.as_deref(),
                        library: lib.as_deref(),
                        tag: tag.as_deref(),
                        category: category.as_deref(),
                        kind: kind.as_deref(),
                        offset,
                        limit
                    }
                ))?
            );
            Ok(())
        }
        Command::Add {
            destination,
            asset: spec,
            mode,
            at,
            scale,
            rotate,
            prefix,
            dry_run,
            if_revision,
        } => {
            let cwd = std::env::current_dir()?;
            let idx = all_indexes(&cwd)?;
            let item = library::resolve(&idx, &spec)?;
            let source_bytes = fs::read(&item.path)?;
            let source_raw: serde_json::Value = serde_json::from_slice(&source_bytes)?;
            let manifest =
                asset::manifest(&source_raw)?.context("indexed asset metadata missing")?;
            let destination_bytes = fs::read(&destination)?;
            let destination_raw: serde_json::Value = serde_json::from_slice(&destination_bytes)?;
            let planned_instance_id =
                matches!(mode, AddMode::Instance).then(|| next_instance_id(&destination_raw));
            let chosen_prefix = prefix.unwrap_or_else(|| {
                planned_instance_id.clone().unwrap_or_else(|| {
                    format!(
                        "{}-{}",
                        item.id.replace('/', "-"),
                        &item.content_hash[7..15]
                    )
                })
            });
            let mut result = import::compose(
                destination_raw,
                source_raw,
                &import::ImportOptions {
                    destination_page: selected_page.map(str::to_owned),
                    source_page: manifest.entry_page.clone(),
                    prefix: Some(chosen_prefix),
                    x: at[0],
                    y: at[1],
                    scale,
                    rotation: rotate,
                    expand_canvas: false,
                },
            )?;
            if matches!(mode, AddMode::Instance) {
                let layer_ids = result.summary["layers"]
                    .as_object()
                    .into_iter()
                    .flat_map(|m| m.values())
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<Vec<_>>();
                let id = planned_instance_id.expect("instance ID planned for instance mode");
                let page_id = result.summary["destination_page"]
                    .as_str()
                    .unwrap_or("page-1")
                    .to_owned();
                let materialized_hash =
                    instance::materialized_hash(&result.document, &page_id, &layer_ids)?;
                let base_layers =
                    instance::selected_layers(&result.document, &page_id, &layer_ids)?;
                let child_ids = result.summary["objects"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let layer_indices =
                    instance::layer_positions_public(&result.document, &page_id, &layer_ids)?;
                let layer_states = instance::layer_states(&result.document, &page_id, &layer_ids)?;
                instance::attach(
                    &mut result.document,
                    instance::InstanceRecord {
                        id: id.clone(),
                        library: item.library.clone(),
                        asset_id: item.id.clone(),
                        asset_version: item.version.clone(),
                        content_hash: item.content_hash.clone(),
                        layer_ids,
                        page_id,
                        parent_id: None,
                        layer_indices,
                        layer_states,
                        transform: {
                            let r = rotate.to_radians();
                            [
                                scale * r.cos(),
                                scale * r.sin(),
                                -scale * r.sin(),
                                scale * r.cos(),
                                at[0],
                                at[1],
                            ]
                        },
                        visible: true,
                        overrides: Default::default(),
                        property_definitions: manifest.properties.clone(),
                        child_ids,
                        local_patches: vec![],
                        materialized_hash: Some(materialized_hash),
                        base_layers,
                        previous: vec![],
                    },
                )?;
                result.summary["instance_id"] = serde_json::Value::String(id);
            }
            let value = serde_json::to_value(&result.document)?;
            let change = transaction::commit_value(
                &destination,
                "add",
                dry_run,
                if_revision.as_deref(),
                &value,
            )?;
            println!(
                "{}",
                json_pretty(serde_json::json!({"change":change,"summary":result.summary}))?
            );
            Ok(())
        }
        Command::Instance { input, action } => match action {
            InstanceAction::List => {
                let raw = serde_json::from_slice(&fs::read(input)?)?;
                println!("{}", json_pretty(instance::list(&raw)?)?);
                Ok(())
            }
            InstanceAction::Inspect { id } => {
                let raw = serde_json::from_slice(&fs::read(input)?)?;
                println!("{}", json_pretty(instance::inspect(&raw, &id)?)?);
                Ok(())
            }
            InstanceAction::Updates { id, source } => {
                println!(
                    "{}",
                    json_pretty(instance::update_plan(&input, &id, &source)?)?
                );
                Ok(())
            }
            InstanceAction::Update {
                id,
                source,
                all,
                asset: asset_filter,
                group,
                current_version,
                current_hash,
                package,
                stale_only,
                to,
                continue_on_conflict,
                resolutions,
                dry_run,
            } => {
                if let Some(expected) = to {
                    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&source)?)?;
                    let actual = asset::manifest(&raw)?
                        .context("source lacks asset metadata")?
                        .asset_version;
                    if actual != expected {
                        anyhow::bail!("source version is {actual}, not requested {expected}")
                    }
                }
                let result = if all {
                    instance::update_bulk(
                        &input,
                        &source,
                        asset_filter.as_deref(),
                        selected_page,
                        group.as_deref(),
                        current_version.as_deref(),
                        current_hash.as_deref(),
                        package.as_deref(),
                        stale_only,
                        continue_on_conflict,
                        resolutions.as_deref(),
                        dry_run,
                    )?
                } else {
                    let id = id.as_deref().context("instance ID is required")?;
                    if let Some(resolutions) = resolutions {
                        instance::update_resolved(&input, id, &source, &resolutions, dry_run)?
                    } else {
                        instance::update(&input, id, &source, dry_run)?
                    }
                };
                println!("{}", json_pretty(result)?);
                Ok(())
            }
            InstanceAction::Rollback { id, dry_run } => {
                println!(
                    "{}",
                    json_pretty(instance::rollback(&input, &id, dry_run)?)?
                );
                Ok(())
            }
            InstanceAction::Detach { id, dry_run } => {
                println!("{}", json_pretty(instance::detach(&input, &id, dry_run)?)?);
                Ok(())
            }
            InstanceAction::Set {
                id,
                property,
                value,
                source,
                dry_run,
            } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(source)?)?;
                let m = asset::manifest(&raw)?.context("source lacks asset metadata")?;
                println!(
                    "{}",
                    json_pretty(instance::set_property(
                        &input, &id, &m, &property, &value, dry_run
                    )?)?
                );
                Ok(())
            }
        },
        Command::Package { action } => {
            let cwd = std::env::current_dir()?;
            let value = match action {
                PackageAction::List => package::list_installed(&cwd)?,
                PackageAction::Remove { spec, force } => {
                    let report = package::remove_installed(&cwd, &spec, force)?;
                    let library_name = format!("pkg-{}", spec.replace(['/', '@'], "-"));
                    let config = library::project_config(&cwd);
                    let _ = library::remove(&config, &library_name);
                    let _ = library::refresh(&cwd, &config, &library::project_index(&cwd), None);
                    report
                }
                PackageAction::Keygen { prefix } => package::keygen(&prefix)?,
                PackageAction::Sign { file, key, output } => {
                    package::sign(&file, &key, output.as_deref())?
                }
                PackageAction::Init { dir, name } => package::init(&dir, &name)?,
                PackageAction::Pack { dir, output } => package::pack(&dir, &output)?,
                PackageAction::Verify {
                    file,
                    signature,
                    public_key,
                } => {
                    let base = package::verify(&file)?;
                    if let Some(sig) = signature {
                        serde_json::json!({"package":base,"signature":package::verify_signature(&file,&sig,public_key.as_deref())?})
                    } else {
                        base
                    }
                }
                PackageAction::Inspect { file } => package::inspect(&file)?,
                PackageAction::Publish {
                    file,
                    registry,
                    dry_run,
                } => package::publish(&file, &registry, dry_run)?,
                PackageAction::Install { source, registry } => {
                    let mut report = if let Some(reg) = registry {
                        package::install_from_registry(&reg, &source, &cwd)?
                    } else {
                        package::install(Path::new(&source), &cwd, &source)?
                    };
                    activate_installed_package(&cwd, &mut report)?;
                    report
                }
            };
            println!("{}", json_pretty(value)?);
            Ok(())
        }
        Command::Registry { action } => match action {
            RegistryAction::Search { path, query } => {
                println!("{}", json_pretty(package::registry_search(&path, &query)?)?);
                Ok(())
            }
        },
        Command::Lock { action } => match action {
            LockAction::Verify => {
                let cwd = std::env::current_dir()?;
                let report = package::lock_verify(&cwd)?;
                println!("{}", json_pretty(report.clone())?);
                if report["ok"] != true {
                    anyhow::bail!("lock verification failed")
                }
                Ok(())
            }
            LockAction::Sync { offline } => {
                let cwd = std::env::current_dir()?;
                println!("{}", json_pretty(package::lock_sync(&cwd, offline)?)?);
                Ok(())
            }
        },
        Command::Benchmark {
            layers,
            objects,
            png,
            max_ms,
            render: render_benchmark,
            warmups,
            repetitions,
            scale,
            paths_only,
            images,
            composite: composite_benchmark,
            clipped,
            stack_depth,
            blur_radius,
            source_size,
            operations,
            json: _,
        } => {
            let value = if composite_benchmark {
                if scale != 1.0 || warmups != 1 {
                    anyhow::bail!("composite benchmark uses scale 1 and one cold run; omit --scale and --warmups")
                }
                benchmark::run_composite(benchmark::CompositeBenchmark {
                    clipped,
                    depth: stack_depth,
                    source_size,
                    blur: blur_radius,
                    repetitions,
                })?
            } else if let Some(images) = images {
                benchmark::run_image(benchmark::ImageBenchmark {
                    images,
                    source_size,
                    operations,
                    scale,
                    repetitions: repetitions.min(20),
                })?
            } else if render_benchmark || paths_only || scale != 1.0 {
                benchmark::run_render(benchmark::RenderBenchmark {
                    layers,
                    objects,
                    paths_only,
                    scale,
                    warmups,
                    repetitions,
                })?
            } else {
                benchmark::run(layers, objects, png, max_ms)?
            };
            if composite_benchmark {
                if let Some(max_ms) = max_ms {
                    let worst = value["runs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|run| run["total_us"].as_u64())
                        .max()
                        .unwrap_or(0);
                    if worst as f64 / 1000.0 > max_ms as f64 {
                        anyhow::bail!(
                            "composite benchmark exceeded --max-ms: {:.3} ms > {max_ms} ms",
                            worst as f64 / 1000.0
                        )
                    }
                }
            }
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        Command::Tree {
            input,
            query,
            kind,
            layer,
            offset,
            limit,
        } => {
            let bytes = fs::read(&input)?;
            let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            if scene::is_scene_document(&raw) {
                let k = kind.map(SearchKind::as_str);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&scene::inspect_paginated_v4(
                        &raw,
                        selected_page,
                        query.as_deref(),
                        k,
                        layer.as_deref(),
                        offset,
                        limit
                    )?)?
                );
                return Ok(());
            }
            let doc = read_document(&input, selected_page)?;
            let mut output = agent::inspect_paginated(
                &doc,
                query.as_deref(),
                kind.map(SearchKind::legacy).transpose()?,
                layer.as_deref(),
                offset,
                limit,
            )?;
            instance::annotate_inspection(&raw, &mut output);
            println!("{}", serde_json::to_string_pretty(&output)?);
            Ok(())
        }
        Command::Search {
            input,
            query,
            kind,
            layer,
            offset,
            limit,
        } => {
            let bytes = fs::read(&input)?;
            let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            if scene::is_scene_document(&raw) {
                let k = kind.map(SearchKind::as_str);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&scene::inspect_paginated_v4(
                        &raw,
                        selected_page,
                        Some(&query),
                        k,
                        layer.as_deref(),
                        offset,
                        limit
                    )?)?
                );
                return Ok(());
            }
            let doc = read_document(&input, selected_page)?;
            let mut output = agent::inspect_paginated(
                &doc,
                Some(&query),
                kind.map(SearchKind::legacy).transpose()?,
                layer.as_deref(),
                offset,
                limit,
            )?;
            instance::annotate_inspection(&raw, &mut output);
            println!("{}", serde_json::to_string_pretty(&output)?);
            Ok(())
        }
        Command::Object {
            input,
            dry_run,
            if_revision,
            action,
        } => {
            let bytes = fs::read(&input)?;
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            let result = if scene::is_scene_document(&raw) {
                scene::apply_object(&mut raw, selected_page, &action)?
            } else {
                let mut doc: Document = serde_json::from_value(raw.clone())?;
                select_page(&mut doc, selected_page)?;
                let result = agent::apply(&mut doc, &action)?;
                agent::merge_document(&mut raw, &doc, std::slice::from_ref(&action))?;
                result
            };
            let change =
                transaction::commit_value(&input, "object", dry_run, if_revision.as_deref(), &raw)?;
            println!("{}", serde_json::json!({"change":change,"result":result}));
            Ok(())
        }
        Command::Batch {
            input,
            operations,
            dry_run,
            revision,
            upsert,
        } => {
            let bytes =
                fs::read(&input).with_context(|| format!("could not read {}", input.display()))?;
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            let operation_bytes = fs::read(&operations)
                .with_context(|| format!("could not read {}", operations.display()))?;
            let changes = if scene::is_scene_document(&raw) {
                let mut actions: Vec<serde_json::Value> = serde_json::from_slice(&operation_bytes)?;
                if upsert {
                    for action in &mut actions {
                        let is_put = matches!(
                            action.get("type").and_then(serde_json::Value::as_str),
                            Some("put-shape" | "put-path" | "put-text" | "put-image")
                        );
                        if let (true, Some(object)) = (is_put, action.as_object_mut()) {
                            object
                                .entry("mode")
                                .or_insert_with(|| serde_json::json!("replace"));
                        }
                    }
                }
                scene::apply_batch_at(&mut raw, selected_page, &actions, &input)?
            } else {
                let mut doc: Document = serde_json::from_value(raw.clone())?;
                select_page(&mut doc, selected_page)?;
                let actions: Vec<agent::ObjectAction> = serde_json::from_slice(&operation_bytes)?;
                let changes = agent::apply_batch(&mut doc, &actions)?;
                agent::merge_document(&mut raw, &doc, &actions)?;
                changes
            };
            let change =
                transaction::commit_value(&input, "batch", dry_run, revision.as_deref(), &raw)?;
            println!("{}", serde_json::json!({"change":change,"changes":changes}));
            Ok(())
        }
        Command::Serve { host, port, input } => server::serve(&host, port, input).await,
        Command::Page {
            input,
            action,
            dry_run,
            if_revision,
        } => {
            let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if scene::is_scene_document(&raw) {
                let mut raw = raw;
                if matches!(action, page::PageAction::List) {
                    println!("{}", serde_json::to_string_pretty(&scene::page_list(&raw))?);
                    return Ok(());
                }
                let result = scene::apply_page(&mut raw, action)?;
                transaction::commit_value(&input, "page", dry_run, if_revision.as_deref(), &raw)?;
                println!("{result}");
                Ok(())
            } else if matches!(action, page::PageAction::List) {
                let doc = read_document(&input, None)?;
                println!("{}", serde_json::to_string_pretty(&page::list(&doc))?);
                Ok(())
            } else {
                editing::edit_page_options(
                    &input,
                    None,
                    "page",
                    dry_run,
                    if_revision.as_deref(),
                    |doc| {
                        println!("{}", page::apply(doc, action)?);
                        Ok(())
                    },
                )
            }
        }
        Command::Raster {
            input,
            dry_run,
            if_revision,
            action,
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if !scene::is_scene_document(&raw) {
                bail!("[unsupported-version] raster layers need a v6 scene document; run `pentool migrate` first")
            }
            let page = selected_page;
            let heal_tool = matches!(action, RasterAction::HealStroke { .. });
            let (operation, result) = match action {
                RasterAction::Add {
                    id,
                    x,
                    y,
                    width,
                    height,
                    layer,
                } => (
                    "raster-add",
                    raster::add(&mut raw, page, layer.as_deref(), &id, x, y, width, height)?,
                ),
                RasterAction::Info { id } => {
                    println!(
                        "{}",
                        serde_json::to_string(&raster::info(&raw, page, &id)?)?
                    );
                    return Ok(());
                }
                RasterAction::Verify { id, replay } => {
                    let report = raster::verify(&raw, page, id.as_deref(), replay)?;
                    println!("{}", serde_json::to_string(&report)?);
                    if report["ok"] != true {
                        bail!("[corrupt-raster] raster verification found damage; see the report for the fix")
                    }
                    return Ok(());
                }
                RasterAction::Repair { id, strategy } => (
                    "raster-repair",
                    raster::repair(
                        &mut raw,
                        page,
                        &id,
                        raster::RepairStrategy::parse(&strategy)?,
                    )?,
                ),
                RasterAction::Clear { id } => ("raster-clear", raster::clear(&mut raw, page, &id)?),
                RasterAction::Resize {
                    id,
                    width,
                    height,
                    resample,
                } => (
                    "raster-resize",
                    raster::resize(
                        &mut raw,
                        page,
                        &id,
                        width,
                        height,
                        raster::Resample::parse(&resample)?,
                    )?,
                ),
                RasterAction::Crop {
                    id,
                    x,
                    y,
                    width,
                    height,
                } => (
                    "raster-crop",
                    raster::crop(&mut raw, page, &id, [x, y, width, height])?,
                ),
                RasterAction::Trim { id } => ("raster-trim", raster::trim(&mut raw, page, &id)?),
                RasterAction::Duplicate { id, new_id } => (
                    "raster-duplicate",
                    raster::duplicate(&mut raw, page, &id, &new_id)?,
                ),
                RasterAction::Rotate { id, degrees } => (
                    "raster-rotate",
                    raster::orient(&mut raw, page, &id, raster::Orient::rotation(degrees)?)?,
                ),
                RasterAction::Flip { id, axis } => (
                    "raster-flip",
                    raster::orient(&mut raw, page, &id, raster::Orient::flip(&axis)?)?,
                ),
                RasterAction::MergeVisible { id } => (
                    "raster-merge-visible",
                    raster::merge_visible(&mut raw, page, &id)?,
                ),
                RasterAction::StampVisible { new_id } => (
                    "raster-stamp-visible",
                    raster::stamp_visible(&mut raw, &input, page, &new_id)?,
                ),
                RasterAction::Flatten { new_id } => (
                    "raster-flatten",
                    raster::flatten(&mut raw, &input, page, &new_id)?,
                ),
                RasterAction::MergeDown { id } => (
                    "raster-merge-down",
                    raster::merge_down(&mut raw, page, &id)?,
                ),
                RasterAction::Rasterize {
                    id,
                    new_id,
                    replace,
                } => (
                    "raster-rasterize",
                    raster::rasterize(&mut raw, &input, page, &id, &new_id, replace)?,
                ),
                RasterAction::TipAdd {
                    name,
                    image,
                    source,
                } => {
                    const MAX_TIP_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
                    if fs::metadata(&image)?.len() > MAX_TIP_SOURCE_BYTES {
                        bail!("[limit-exceeded] tip images are limited to 16 MiB")
                    }
                    let bytes = fs::read(&image)?;
                    (
                        "raster-tip-add",
                        raster::tip_add(
                            &mut raw,
                            &name,
                            &bytes,
                            raster::TipSource::parse(&source)?,
                        )?,
                    )
                }
                RasterAction::TipRemove { name } => {
                    ("raster-tip-remove", raster::tip_remove(&mut raw, &name)?)
                }
                RasterAction::PresetAdd {
                    name,
                    brush,
                    description,
                    replace,
                } => {
                    let brush: serde_json::Value = match brush.strip_prefix('@') {
                        Some(path) => {
                            if fs::metadata(path)?.len() > 16 << 20 {
                                bail!("[limit-exceeded] {path} is larger than 16 MiB")
                            }
                            serde_json::from_str(&fs::read_to_string(path)?)?
                        }
                        None => serde_json::from_str(&brush)?,
                    };
                    (
                        "raster-preset-add",
                        raster::preset_add(
                            &mut raw,
                            &name,
                            &brush,
                            description.as_deref(),
                            replace,
                        )?,
                    )
                }
                RasterAction::PresetRemove { name } => (
                    "raster-preset-remove",
                    raster::preset_remove(&mut raw, &name)?,
                ),
                RasterAction::Presets => {
                    println!("{}", serde_json::to_string(&raster::preset_list(&raw))?);
                    return Ok(());
                }
                RasterAction::PresetShow { name } => {
                    println!(
                        "{}",
                        serde_json::to_string(&raster::preset_show(&raw, &name)?)?
                    );
                    return Ok(());
                }
                RasterAction::PresetExport { name, out } => {
                    let file = raster::preset_export(&raw, &name)?;
                    fs::write(&out, serde_json::to_string_pretty(&file)? + "\n")?;
                    println!(
                        "{}",
                        serde_json::json!({"name": name, "path": out.to_string_lossy()})
                    );
                    return Ok(());
                }
                RasterAction::PresetImport {
                    file,
                    name,
                    replace,
                } => {
                    if fs::metadata(&file)?.len() > 16 << 20 {
                        bail!("[limit-exceeded] preset files are limited to 16 MiB")
                    }
                    let body: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file)?)
                        .map_err(|e| {
                            anyhow::anyhow!("[invalid-input] preset file is not JSON: {e}")
                        })?;
                    (
                        "raster-preset-import",
                        raster::preset_import(&mut raw, &body, name.as_deref(), replace)?,
                    )
                }
                RasterAction::PresetImportGbr { file, name } => {
                    if fs::metadata(&file)?.len() > 16 << 20 {
                        bail!("[limit-exceeded] .gbr files are limited to 16 MiB")
                    }
                    (
                        "raster-preset-import-gbr",
                        raster::preset_import_gbr(&mut raw, &fs::read(&file)?, &name)?,
                    )
                }
                RasterAction::PresetImportMypaint { file, name } => {
                    if fs::metadata(&file)?.len() > 16 << 20 {
                        bail!("[limit-exceeded] .myb files are limited to 16 MiB")
                    }
                    (
                        "raster-preset-import-mypaint",
                        raster::preset_import_mypaint(
                            &mut raw,
                            &fs::read_to_string(&file)?,
                            &name,
                        )?,
                    )
                }
                RasterAction::Batch { operations } => {
                    if fs::metadata(&operations)?.len() > 64 << 20 {
                        bail!("[limit-exceeded] a raster batch file is limited to 64 MiB")
                    }
                    let list: Vec<serde_json::Value> =
                        serde_json::from_slice(&fs::read(&operations)?).map_err(|e| {
                            anyhow::anyhow!("[invalid-input] operations must be a JSON array: {e}")
                        })?;
                    ("raster-batch", raster::batch(&mut raw, page, &list)?)
                }
                RasterAction::Tips => {
                    println!("{}", serde_json::to_string(&raster::tip_list(&raw))?);
                    return Ok(());
                }
                RasterAction::Checkpoint { id, compact } => (
                    "raster-checkpoint",
                    raster::checkpoint(&mut raw, page, &id, compact)?,
                ),
                RasterAction::CloneSource { id, layer, x, y } => {
                    // `below` freezes what lies under the layer into a hidden raster
                    // and samples that, so the stroke pins the composite.
                    let layer = if layer.as_deref() == Some("below") {
                        raster::snapshot_below(&mut raw, &input, page, &id)?;
                        Some(format!("{id}-below"))
                    } else {
                        layer
                    };
                    (
                        "raster-clone-source",
                        raster::set_clone_source(&mut raw, page, &id, layer.as_deref(), x, y)?,
                    )
                }
                RasterAction::Fill {
                    region,
                    color,
                    opacity,
                } => ("raster-fill", {
                    let sample = region.sample(&raw, &input, page)?;
                    raster::fill_sampled(
                        &mut raw,
                        page,
                        &region.id,
                        raster::parse_color(&color)?,
                        opacity,
                        &region.options(),
                        sample.as_ref(),
                    )?
                }),
                RasterAction::SelectWand { region, mode } => ("raster-select-wand", {
                    let sample = region.sample(&raw, &input, page)?;
                    raster::select_wand_sampled(
                        &mut raw,
                        page,
                        &region.id,
                        &region.options(),
                        raster::SelectionMode::parse(&mode)?,
                        sample.as_ref(),
                    )?
                }),
                RasterAction::SelectMarquee {
                    id,
                    shape,
                    rect,
                    mode,
                    feather,
                } => {
                    let rect: [f64; 4] = rect.try_into().map_err(|_| {
                        anyhow::anyhow!("[invalid-selection] --rect needs X Y WIDTH HEIGHT")
                    })?;
                    (
                        "raster-select-marquee",
                        raster::select_marquee(
                            &mut raw,
                            page,
                            &id,
                            raster::Marquee::parse(&shape)?,
                            rect,
                            raster::SelectionMode::parse(&mode)?,
                            feather,
                        )?,
                    )
                }
                RasterAction::SelectLasso {
                    id,
                    points,
                    mode,
                    feather,
                } => {
                    let body = match points.strip_prefix('@') {
                        Some(path) => {
                            if fs::metadata(path)?.len() > 16 << 20 {
                                bail!("[limit-exceeded] {path} is larger than 16 MiB")
                            }
                            fs::read_to_string(path)?
                        }
                        None => points.clone(),
                    };
                    let points: Vec<[f64; 2]> = serde_json::from_str(&body).map_err(|e| {
                        anyhow::anyhow!(
                            "[invalid-selection] --points must be a JSON array of [x, y]: {e}"
                        )
                    })?;
                    (
                        "raster-select-lasso",
                        raster::select_lasso(
                            &mut raw,
                            page,
                            &id,
                            &points,
                            raster::SelectionMode::parse(&mode)?,
                            feather,
                        )?,
                    )
                }
                RasterAction::SelectModify {
                    id,
                    op,
                    amount,
                    tolerance,
                } => (
                    "raster-select-modify",
                    raster::select_modify(
                        &mut raw,
                        page,
                        &id,
                        raster::SelectionOp::parse(&op)?,
                        amount,
                        tolerance,
                    )?,
                ),
                RasterAction::SelectQuickmask {
                    id,
                    samples,
                    brush,
                    preset,
                    erase,
                    seed,
                } => {
                    let read = |text: &str| -> Result<serde_json::Value> {
                        let body = match text.strip_prefix('@') {
                            Some(path) => {
                                if fs::metadata(path)?.len() > 16 << 20 {
                                    bail!("[limit-exceeded] {path} is larger than 16 MiB")
                                }
                                fs::read_to_string(path)?
                            }
                            None => text.to_owned(),
                        };
                        Ok(serde_json::from_str(&body)?)
                    };
                    let normalized = raster::normalize_input(&read(&samples)?)?;
                    let brush = raster::Brush::parse(&raster::resolve_preset(
                        &raw,
                        &read(&brush)?,
                        preset.as_deref(),
                    )?)?;
                    (
                        "raster-select-quickmask",
                        raster::select_quickmask(
                            &mut raw,
                            page,
                            &id,
                            brush,
                            normalized.samples,
                            erase,
                            seed,
                        )?,
                    )
                }
                RasterAction::SelectSave { id, name } => (
                    "raster-select-save",
                    raster::select_save(&mut raw, page, &id, &name)?,
                ),
                RasterAction::SelectLoad { id, name, mode } => (
                    "raster-select-load",
                    raster::select_load(
                        &mut raw,
                        page,
                        &id,
                        &name,
                        raster::SelectionMode::parse(&mode)?,
                    )?,
                ),
                RasterAction::SelectDelete { id, name } => (
                    "raster-select-delete",
                    raster::select_delete(&mut raw, &id, &name)?,
                ),
                RasterAction::Lift { id, new_id, cut } => (
                    "raster-lift",
                    raster::lift_pixels(&mut raw, page, &id, &new_id, cut)?,
                ),
                RasterAction::MovePixels { id, dx, dy, copy } => (
                    "raster-move-pixels",
                    raster::move_pixels(&mut raw, page, &id, dx, dy, copy)?,
                ),
                RasterAction::TransformPixels {
                    id,
                    scale,
                    scale_y,
                    rotate,
                    dx,
                    dy,
                    nearest,
                    copy,
                } => (
                    "raster-transform-pixels",
                    raster::transform_pixels(
                        &mut raw,
                        page,
                        &id,
                        &raster::PixelTransform {
                            scale_x: scale,
                            scale_y: scale_y.unwrap_or(scale),
                            rotate,
                            dx,
                            dy,
                            nearest,
                            copy,
                        },
                    )?,
                ),
                RasterAction::Paste {
                    id,
                    source,
                    x,
                    y,
                    opacity,
                } => (
                    "raster-paste",
                    raster::paste_pixels(&mut raw, page, &id, &source, x, y, opacity)?,
                ),
                RasterAction::SelectClear { id } => {
                    ("raster-select-clear", raster::select_clear(&mut raw, &id)?)
                }
                RasterAction::SelectInfo { id } => {
                    println!(
                        "{}",
                        serde_json::to_string(&raster::select_info(&raw, &id)?)?
                    );
                    return Ok(());
                }
                RasterAction::HealPatch {
                    id,
                    dx,
                    dy,
                    texture,
                    tone,
                    seed,
                } => (
                    "raster-heal-patch",
                    raster::heal_patch(&mut raw, page, &id, (dx, dy), texture, tone, seed)?,
                ),
                RasterAction::HealSpot {
                    id,
                    x,
                    y,
                    radius,
                    texture,
                    tone,
                    seed,
                } => (
                    "raster-heal-spot",
                    raster::heal_spot(&mut raw, page, &id, x, y, radius, texture, tone, seed)?,
                ),
                RasterAction::CloneStroke {
                    id,
                    samples,
                    brush,
                    preset,
                    aligned,
                    angle,
                    scale,
                    seed,
                }
                | RasterAction::HealStroke {
                    id,
                    samples,
                    brush,
                    preset,
                    aligned,
                    angle,
                    scale,
                    seed,
                } => {
                    let read = |text: &str| -> Result<serde_json::Value> {
                        let body = match text.strip_prefix('@') {
                            Some(path) => {
                                if fs::metadata(path)?.len() > 16 << 20 {
                                    bail!("[limit-exceeded] {path} is larger than 16 MiB")
                                }
                                fs::read_to_string(path)?
                            }
                            None => text.to_owned(),
                        };
                        Ok(serde_json::from_str(&body)?)
                    };
                    let normalized = raster::normalize_input(&read(&samples)?)?;
                    let brush = raster::Brush::parse(&raster::resolve_preset(
                        &raw,
                        &read(&brush)?,
                        preset.as_deref(),
                    )?)?;
                    let input_summary = normalized.summary();
                    let mut result = raster::clone_stroke(
                        &mut raw,
                        page,
                        &id,
                        brush,
                        normalized.samples,
                        &raster::CloneOptions {
                            aligned,
                            angle,
                            scale,
                        },
                        seed,
                        if heal_tool {
                            raster::Tool::Heal
                        } else {
                            raster::Tool::Clone
                        },
                    )?;
                    result["input"] = input_summary;
                    (
                        if heal_tool {
                            "raster-heal-stroke"
                        } else {
                            "raster-clone-stroke"
                        },
                        result,
                    )
                }
                RasterAction::Stroke {
                    id,
                    samples,
                    brush,
                    preset,
                    color,
                    blend,
                    seed,
                } => {
                    let read = |text: &str| -> Result<serde_json::Value> {
                        let body = match text.strip_prefix('@') {
                            Some(path) => {
                                const MAX_JSON: u64 = 16 << 20;
                                if fs::metadata(path)?.len() > MAX_JSON {
                                    bail!("[limit-exceeded] {path} is larger than 16 MiB")
                                }
                                fs::read_to_string(path)?
                            }
                            None => text.to_owned(),
                        };
                        Ok(serde_json::from_str(&body)?)
                    };
                    let normalized = raster::normalize_input(&read(&samples)?)?;
                    let input_summary = normalized.summary();
                    let request = raster::StrokeRequest {
                        brush: raster::Brush::parse(&raster::resolve_preset(
                            &raw,
                            &read(&brush)?,
                            preset.as_deref(),
                        )?)?,
                        samples: normalized.samples,
                        color: raster::parse_color(&color)?,
                        blend: raster::Blend::parse(&blend)?,
                        seed,
                        clone: None,
                    };
                    let mut result = raster::paint(&mut raw, page, &id, request)?;
                    result["input"] = input_summary;
                    ("raster-stroke", result)
                }
            };
            let summary = transaction::commit_value(
                &input,
                operation,
                dry_run,
                if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::json!({"result": result, "dry_run": dry_run, "change": summary})
            );
            Ok(())
        }
        Command::Ai { action } => {
            println!(
                "{}",
                serde_json::to_string(&ai::run(action, selected_page)?)?
            );
            Ok(())
        }
        Command::Import {
            destination,
            source,
            source_page,
            prefix,
            no_prefix,
            at,
            scale,
            rotate,
            expand_canvas,
            dry_run,
            revision,
        } => {
            const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;
            for path in [&destination, &source] {
                if fs::metadata(path)?.len() > MAX_IMPORT_BYTES {
                    anyhow::bail!("import files are limited to 64 MiB");
                }
            }
            let destination_bytes = fs::read(&destination)?;
            let prefix = if no_prefix {
                None
            } else {
                Some(prefix.unwrap_or_else(|| {
                    source
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .replace(|c: char| c.is_whitespace(), "-")
                }))
            };
            let source_bytes = fs::read(&source)?;
            for (role, path, bytes) in [
                ("destination", &destination, &destination_bytes),
                ("source", &source, &source_bytes),
            ] {
                let version = serde_json::from_slice::<serde_json::Value>(bytes)
                    .ok()
                    .and_then(|raw| raw.get("version").and_then(serde_json::Value::as_u64));
                if version.is_some_and(|v| v >= 4) {
                    anyhow::bail!(
                        "[unsupported-version] import works on legacy v1-v3 documents; the {role} {} is v{}. Place its contents with `image add`, `batch`, or `asset` commands, or rebuild it from a v3 export (`pentool migrate --target 3`).",
                        path.display(),
                        version.unwrap_or_default()
                    );
                }
            }
            let result = import::compose(
                serde_json::from_slice(&destination_bytes)?,
                serde_json::from_slice(&source_bytes)?,
                &import::ImportOptions {
                    destination_page: selected_page.map(str::to_owned),
                    source_page,
                    prefix,
                    x: at[0],
                    y: at[1],
                    scale,
                    rotation: rotate,
                    expand_canvas,
                },
            )?;
            let value = serde_json::to_value(&result.document)?;
            let change = transaction::commit_value(
                &destination,
                "import",
                dry_run,
                revision.as_deref(),
                &value,
            )?;
            println!(
                "{}",
                serde_json::json!({"change":change,"summary":result.summary})
            );
            Ok(())
        }
        Command::Text {
            input,
            action,
            dry_run,
            if_revision,
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if scene::is_scene_document(&raw) {
                scene::apply_text(&mut raw, selected_page, action)?;
                let summary = transaction::commit_value(
                    &input,
                    "text",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!("{}", serde_json::to_string(&summary)?);
                Ok(())
            } else {
                editing::edit_page_options(
                    &input,
                    selected_page,
                    "text",
                    dry_run,
                    if_revision.as_deref(),
                    |doc| text::apply(doc, action),
                )
            }
        }
        Command::Font {
            input,
            action,
            dry_run,
            if_revision,
        } => editing::edit_page_options(
            &input,
            selected_page,
            "font",
            dry_run,
            if_revision.as_deref(),
            |doc| text::font(doc, action),
        ),
        Command::Fonts { input } => {
            let doc = match input {
                Some(path) => read_document(&path, selected_page)?,
                None => Document::new(1, 1),
            };
            println!("{}", fonts::list(&doc)?);
            Ok(())
        }
        Command::LayerGeometry {
            input,
            dry_run,
            if_revision,
            layer,
            operation,
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if scene::is_scene_document(&raw) {
                scene::apply_layer_geometry(&mut raw, selected_page, &layer, &operation)?;
                let summary = transaction::commit_value(
                    &input,
                    "layer-geometry",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!("{}", serde_json::to_string(&summary)?);
                Ok(())
            } else {
                editing::edit_page_options(
                    &input,
                    selected_page,
                    "layer-geometry",
                    dry_run,
                    if_revision.as_deref(),
                    |doc| {
                        geometry::execute_layer(doc, &layer, &operation)?;
                        Ok(())
                    },
                )
            }
        }
        Command::Geometry {
            input,
            dry_run,
            if_revision,
            layer,
            id,
            operation,
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if scene::is_scene_document(&raw) {
                let result =
                    scene::apply_geometry(&mut raw, selected_page, &layer, &id, &operation)?;
                if operation.is_query() {
                    println!("{result}");
                } else {
                    let summary = transaction::commit_value(
                        &input,
                        "geometry",
                        dry_run,
                        if_revision.as_deref(),
                        &raw,
                    )?;
                    println!("{}", serde_json::to_string(&summary)?);
                }
                Ok(())
            } else if operation.is_query() {
                let mut doc = read_document(&input, selected_page)?;
                println!("{}", geometry::execute(&mut doc, &layer, &id, &operation)?);
                Ok(())
            } else {
                editing::edit_page_options(
                    &input,
                    selected_page,
                    "geometry",
                    dry_run,
                    if_revision.as_deref(),
                    |doc| {
                        geometry::execute(doc, &layer, &id, &operation)?;
                        Ok(())
                    },
                )
            }
        }
        Command::Canvas {
            input,
            dry_run,
            if_revision,
            width,
            height,
            background,
            name,
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if scene::is_scene_document(&raw) {
                scene::edit_canvas(
                    &mut raw,
                    selected_page,
                    width,
                    height,
                    background.as_deref(),
                    name.as_deref(),
                )?;
                let summary = transaction::commit_value(
                    &input,
                    "canvas",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!("{}", serde_json::to_string(&summary)?);
                Ok(())
            } else {
                editing::edit_page_options(
                    &input,
                    selected_page,
                    "canvas",
                    dry_run,
                    if_revision.as_deref(),
                    |doc| {
                        if let Some(v) = width {
                            doc.canvas.width = v;
                        }
                        if let Some(v) = height {
                            doc.canvas.height = v;
                        }
                        if let Some(v) = background {
                            doc.canvas.background = v;
                        }
                        if let Some(v) = name {
                            doc.name = v;
                        }
                        Ok(())
                    },
                )
            }
        }
        Command::Layer {
            input,
            action,
            dry_run,
            if_revision,
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if scene::is_scene_document(&raw) {
                scene::apply_layer(&mut raw, selected_page, action)?;
                let summary = transaction::commit_value(
                    &input,
                    "layer",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!("{}", serde_json::to_string(&summary)?);
                Ok(())
            } else {
                editing::edit_page_options(
                    &input,
                    selected_page,
                    "layer",
                    dry_run,
                    if_revision.as_deref(),
                    |doc| editing::layer(doc, action),
                )
            }
        }
        Command::Path {
            input,
            action,
            dry_run,
            if_revision,
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if scene::is_scene_document(&raw) {
                scene::apply_path(&mut raw, selected_page, action)?;
                let summary = transaction::commit_value(
                    &input,
                    "path",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!("{}", serde_json::to_string(&summary)?);
                Ok(())
            } else {
                editing::edit_page_options(
                    &input,
                    selected_page,
                    "path",
                    dry_run,
                    if_revision.as_deref(),
                    |doc| editing::path(doc, action),
                )
            }
        }
        Command::New {
            output,
            width,
            height,
            overwrite,
        } => {
            if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
                anyhow::bail!("[invalid-input] canvas dimensions must be between 1 and 16384, got {width}x{height}; nothing was written")
            }
            let doc = scene::new_document(width, height);
            scene::validate(&doc)?;
            if output.exists() {
                if !overwrite {
                    anyhow::bail!("[policy-denied] {} already exists; choose another path or pass --overwrite (the previous document stays recoverable with `pentool undo`)", output.display())
                }
                transaction::commit_value(&output, "new", false, None, &doc)
                    .with_context(|| format!("could not replace {}; the existing file is not a valid Pentool document, so move or delete it explicitly", output.display()))?;
            } else {
                editing::atomic_write(&output, &serde_json::to_vec_pretty(&doc)?)
                    .with_context(|| format!("could not write {}", output.display()))?;
            }
            println!("Created {}", output.display());
            Ok(())
        }
        Command::Info { input } => {
            let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            transaction::validate_value(&raw)?;
            println!("{}", serde_json::to_string_pretty(&raw)?);
            Ok(())
        }
        Command::Migrate {
            input,
            target,
            dry_run,
            if_revision,
        } => {
            if !matches!(target, 3..=7) {
                anyhow::bail!("migration target must be 3, 4, 5, 6, or 7")
            }
            let before =
                fs::read(&input).with_context(|| format!("could not read {}", input.display()))?;
            let raw: serde_json::Value =
                serde_json::from_slice(&before).context("invalid .pen document")?;
            let from = raw.get("version").and_then(serde_json::Value::as_u64);
            let migrated = match target {
                7 => pentool::photo::catalog::migrate(raw)?,
                6 => composite::migrate(raw)?,
                5 => scene::migrate_to_v5(raw)?,
                4 => scene::migrate_to_v4(raw)?,
                3 => scene::flatten_to_v3(&raw)?,
                _ => unreachable!(),
            };
            let summary = transaction::commit_value(
                &input,
                "migrate",
                dry_run,
                if_revision.as_deref(),
                &migrated,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":summary,"from_version":from,"to_version":target})
                )?
            );
            Ok(())
        }
        Command::Shape {
            input,
            kind,
            id,
            layer,
            x,
            y,
            width,
            height,
            radius,
            cx,
            cy,
            radius_x,
            radius_y,
            x1,
            y1,
            x2,
            y2,
            fill,
            stroke,
            stroke_width,
            dry_run,
            if_revision,
        } => {
            let bytes =
                fs::read(&input).with_context(|| format!("could not read {}", input.display()))?;
            let mut raw: serde_json::Value =
                serde_json::from_slice(&bytes).context("invalid .pen document")?;
            scene::put_shape(
                &mut raw,
                selected_page,
                &layer,
                kind,
                &id,
                scene::ShapeInput {
                    x,
                    y,
                    width,
                    height,
                    radius,
                    cx,
                    cy,
                    radius_x,
                    radius_y,
                    x1,
                    y1,
                    x2,
                    y2,
                    fill,
                    stroke,
                    stroke_width,
                },
            )?;
            let summary =
                transaction::commit_value(&input, "shape", dry_run, if_revision.as_deref(), &raw)?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
        Command::ShapeEdit(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            let result = if args.to_path {
                scene::shape_to_path(&mut raw, selected_page, &args.id)?
            } else {
                scene::edit_shape(
                    &mut raw,
                    selected_page,
                    &args.id,
                    &scene::ShapeInput {
                        x: args.x,
                        y: args.y,
                        width: args.width,
                        height: args.height,
                        radius: args.radius,
                        cx: args.cx,
                        cy: args.cy,
                        radius_x: args.radius_x,
                        radius_y: args.radius_y,
                        x1: args.x1,
                        y1: args.y1,
                        x2: args.x2,
                        y2: args.y2,
                        fill: args.fill,
                        stroke: args.stroke,
                        stroke_width: args.stroke_width,
                    },
                )?
            };
            let change = transaction::commit_value(
                &args.input,
                "shape-edit",
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Preset(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            let result = match args.operation.as_str() {
                "save" | "copy" => {
                    let preset =
                        pentool::preset::capture(&raw, &args.input, selected_page, &args.id)?;
                    if args.operation == "save" {
                        if args.name.is_some() {
                            anyhow::bail!(
                                "--name only applies to `preset copy` and `preset paste` (named appearances stored in the document); `preset save` writes --file and does not mutate the document"
                            )
                        }
                        if args.if_revision.is_some() {
                            anyhow::bail!(
                                "--if-revision does not apply to `preset save`, which does not mutate the document"
                            )
                        }
                        let file = args
                            .file
                            .as_deref()
                            .context("preset save requires --file")?;
                        if file.exists() {
                            anyhow::bail!("preset output already exists")
                        }
                        if !args.dry_run {
                            pentool::editing::atomic_write(
                                file,
                                &serde_json::to_vec_pretty(&preset)?,
                            )?;
                        }
                        serde_json::json!({"file":file,"dry_run":args.dry_run})
                    } else {
                        if args.file.is_some() {
                            anyhow::bail!("preset copy uses --name, not --file")
                        }
                        let name = args
                            .name
                            .as_deref()
                            .context("preset copy requires --name")?;
                        if name.is_empty() || raw["styles"].get(name).is_some() {
                            anyhow::bail!("appearance style name must be new and nonempty")
                        }
                        if raw.get("styles").is_none() {
                            raw["styles"] = serde_json::json!({});
                        }
                        raw["styles"][name] =
                            serde_json::json!({"type":"appearance","value":preset});
                        let change = transaction::commit_value(
                            &args.input,
                            "appearance-copy",
                            args.dry_run,
                            args.if_revision.as_deref(),
                            &raw,
                        )?;
                        serde_json::json!({"style":name,"change":change})
                    }
                }
                "apply" | "paste" => {
                    let preset = if args.operation == "apply" {
                        if args.name.is_some() {
                            anyhow::bail!("preset apply uses --file, not --name")
                        }
                        let file = args
                            .file
                            .as_deref()
                            .context("preset apply requires --file")?;
                        if fs::metadata(file)?.len() > pentool::resource::MAX_RESOURCE_BYTES as u64
                        {
                            anyhow::bail!("preset exceeds 512 MiB")
                        }
                        serde_json::from_slice(&fs::read(file)?)?
                    } else {
                        if args.file.is_some() {
                            anyhow::bail!("preset paste uses --name, not --file")
                        }
                        let name = args
                            .name
                            .as_deref()
                            .context("preset paste requires --name")?;
                        let token = raw["styles"]
                            .get(name)
                            .context("appearance style missing")?;
                        if token["type"] != "appearance" {
                            anyhow::bail!("style is not an appearance")
                        }
                        token["value"].clone()
                    };
                    let result =
                        pentool::preset::apply(&mut raw, selected_page, &args.id, &preset)?;
                    let change = transaction::commit_value(
                        &args.input,
                        "appearance-paste",
                        args.dry_run,
                        args.if_revision.as_deref(),
                        &raw,
                    )?;
                    serde_json::json!({"result":result,"change":change})
                }
                _ => anyhow::bail!("preset operation must be save, apply, copy, or paste"),
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Command::Linked(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            let result = match args.operation.as_str() {
                "report" => {
                    if args.asset.is_some()
                        || args.path.is_some()
                        || args.candidates.is_some()
                        || args.dry_run
                        || args.if_revision.is_some()
                    {
                        anyhow::bail!("linked report is read-only and accepts no mutation settings")
                    }
                    pentool::linked::report(&raw, &args.input)?
                }
                "locate" => {
                    if args.path.is_some() || args.dry_run || args.if_revision.is_some() {
                        anyhow::bail!("linked locate is read-only")
                    }
                    let paths: Vec<String> = serde_json::from_str(
                        args.candidates
                            .as_deref()
                            .context("locate requires --candidates JSON")?,
                    )?;
                    pentool::linked::locate(
                        &raw,
                        &args.input,
                        args.asset
                            .as_deref()
                            .context("locate requires an asset digest")?,
                        &paths,
                    )?
                }
                "collect" => {
                    if args.asset.is_some()
                        || args.candidates.is_some()
                        || args.if_revision.is_some()
                    {
                        anyhow::bail!("collect does not mutate the source document")
                    }
                    pentool::linked::collect(
                        &raw,
                        &args.input,
                        args.path
                            .as_deref()
                            .context("collect requires --path output-directory")?,
                        args.dry_run,
                    )?
                }
                "embed" | "relink" | "externalize" => {
                    if args.candidates.is_some()
                        || (args.operation == "embed" && args.path.is_some())
                    {
                        anyhow::bail!("inapplicable linked asset flag")
                    }
                    // Check revision before publishing an external source file.
                    transaction::commit_value(
                        &args.input,
                        "linked-preflight",
                        true,
                        args.if_revision.as_deref(),
                        &raw,
                    )?;
                    let resources = if args.operation == "externalize" {
                        let id = args
                            .asset
                            .as_deref()
                            .context("externalize requires an asset")?;
                        raw["image_assets"].get(id).context("asset missing")?;
                        vec![(
                            args.path.clone().context("externalize requires --path")?,
                            image::load_asset_bytes_for_document(&raw, &args.input, id)?,
                        )]
                    } else {
                        Vec::new()
                    };
                    let result = pentool::linked::edit(
                        &mut raw,
                        &args.input,
                        &args.operation,
                        args.asset
                            .as_deref()
                            .context("linked edit requires an asset digest")?,
                        args.path.as_deref(),
                        true,
                    )?;
                    let change = transaction::commit_bundle(
                        &args.input,
                        &format!("linked-{}", args.operation),
                        args.dry_run,
                        args.if_revision.as_deref(),
                        &raw,
                        &resources,
                    )?;
                    serde_json::json!({"result":result,"change":change})
                }
                _ => anyhow::bail!("unknown linked operation"),
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Command::Selection(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            let query = serde_json::from_str(&args.query)?;
            let result = match args.operation.as_str() {
                "query" => {
                    if args.name.is_some() || args.dry_run || args.if_revision.is_some() {
                        anyhow::bail!(
                            "selection query is read-only; mutation flags are not applicable"
                        )
                    }
                    let pixels = composite::render(&raw, &args.input, selected_page, 1.0)?;
                    let coverage = pentool::selection::coverage(
                        &raw,
                        &args.input,
                        selected_page,
                        &query,
                        &pixels,
                    )?;
                    serde_json::json!({"bounds":pentool::selection::bounds(&coverage,pixels.width(),pixels.height()),"selected_pixels":coverage.iter().filter(|v| **v != 0).count()})
                }
                "save" => pentool::selection::save(
                    &mut raw,
                    &args.input,
                    selected_page,
                    args.name
                        .as_deref()
                        .context("selection save requires --name")?,
                    &query,
                )?,
                "crop" => {
                    if args.name.is_some() {
                        anyhow::bail!("selection crop does not accept --name")
                    }
                    pentool::selection::crop(&mut raw, &args.input, selected_page, &query)?
                }
                _ => anyhow::bail!("selection operation must be query, save, or crop"),
            };
            let change = if args.operation == "query" {
                None
            } else {
                Some(transaction::commit_value(
                    &args.input,
                    &format!("selection-{}", args.operation),
                    args.dry_run,
                    args.if_revision.as_deref(),
                    &raw,
                )?)
            };
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"result":result,"change":change})
                )?
            );
            Ok(())
        }
        Command::Analyze(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            let query: Option<serde_json::Value> = args
                .query
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?;
            let samples: Vec<[u32; 2]> = serde_json::from_str(&args.samples)?;
            let analysis = pentool::inspect::analyze(
                &raw,
                &args.input,
                selected_page,
                &args.scope,
                query.as_ref(),
                &samples,
            )?;
            let comparison = if args.compare {
                Some(pentool::inspect::compare(
                    &raw,
                    &args.input,
                    selected_page,
                    &args.scope,
                    query.as_ref(),
                )?)
            } else {
                None
            };
            let change = if let Some(prefix) = args.palette_tokens {
                pentool::inspect::palette_tokens(&mut raw, &analysis, &prefix)?;
                Some(transaction::commit_value(
                    &args.input,
                    "analysis-palette-tokens",
                    args.dry_run,
                    args.if_revision.as_deref(),
                    &raw,
                )?)
            } else {
                if args.dry_run || args.if_revision.is_some() {
                    anyhow::bail!("analysis is read-only unless --palette-tokens is supplied")
                }
                None
            };
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"analysis":analysis,"comparison":comparison,"change":change})
                )?
            );
            Ok(())
        }
        Command::Effect(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            if args.operation == "list" {
                if args.effect_id.is_some() {
                    anyhow::bail!("effect list takes only FILE and NODE_ID")
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&pentool::effects::list(
                        &raw,
                        selected_page,
                        &args.id
                    )?)?
                );
                return Ok(());
            }
            let effect_id = args
                .effect_id
                .clone()
                .context("effect operations other than list require EFFECT_ID")?;
            let mut settings = serde_json::Map::new();
            let params_supplied = args.params.is_some()
                || args.x.is_some()
                || args.y.is_some()
                || args.blur.is_some()
                || args.color.is_some();
            if args.operation != "add"
                && args.operation != "set"
                && (params_supplied
                    || args.kind.is_some()
                    || args.opacity.is_some()
                    || args.blend.is_some())
            {
                anyhow::bail!("effect settings apply only to add/set")
            }
            if args.operation != "add" && args.operation != "move" && args.index.is_some() {
                anyhow::bail!("--index applies only to effect add/move")
            }
            if let Some(kind) = args.kind {
                settings.insert("kind".into(), serde_json::json!(kind));
            } else if args.operation == "add" {
                settings.insert("kind".into(), serde_json::json!("drop-shadow"));
            }
            if params_supplied {
                let mut params: serde_json::Value = args
                    .params
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or(serde_json::json!({}));
                if !params.is_object() {
                    anyhow::bail!("--params must be an object")
                }
                for (key, v) in [("x", args.x), ("y", args.y), ("blur", args.blur)] {
                    if let Some(v) = v {
                        params[key] = serde_json::json!(v);
                    }
                }
                if let Some(color) = args.color {
                    params["color"] =
                        serde_json::from_str(&color).unwrap_or(serde_json::json!(color));
                }
                settings.insert("params".into(), params);
            }
            if let Some(v) = args.opacity {
                settings.insert("opacity".into(), serde_json::json!(v));
            }
            if let Some(v) = args.blend {
                settings.insert("blend_mode".into(), serde_json::json!(v));
            }
            if let Some(v) = args.index {
                settings.insert("index".into(), serde_json::json!(v));
            }
            if args.operation == "set" && settings.is_empty() {
                anyhow::bail!("effect set requires a setting")
            }
            let result = pentool::effects::edit(
                &mut raw,
                selected_page,
                &args.id,
                &args.operation,
                &effect_id,
                &settings.into(),
            )?;
            let change = transaction::commit_value(
                &args.input,
                &format!("effect-{}", args.operation),
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Fill(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            let mut settings = serde_json::Map::new();
            if args.operation != "add" && args.layer.is_some() {
                anyhow::bail!("--layer applies only to fill add")
            }
            if let Some(layer) = args.layer {
                settings.insert("layer".into(), serde_json::json!(layer));
            }
            for (key, value) in [
                ("x", args.x),
                ("y", args.y),
                ("width", args.width),
                ("height", args.height),
            ] {
                if let Some(value) = value {
                    settings.insert(key.into(), serde_json::json!(value));
                }
            }
            if args.kind.is_some() || args.params.is_some() || args.color.is_some() {
                let mut fill: serde_json::Value = args
                    .params
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or(serde_json::json!({}));
                if !fill.is_object() {
                    anyhow::bail!("--params must be an object")
                }
                if let Some(kind) = args.kind {
                    fill["kind"] = serde_json::json!(kind);
                }
                if let Some(color) = args.color {
                    fill["color"] =
                        serde_json::from_str(&color).unwrap_or(serde_json::json!(color));
                }
                settings.insert("fill".into(), fill);
            }
            if settings.is_empty() {
                anyhow::bail!("fill edit requires at least one setting")
            }
            let result = pentool::fill::edit(
                &mut raw,
                selected_page,
                &args.operation,
                &args.id,
                &settings.into(),
            )?;
            let change = transaction::commit_value(
                &args.input,
                &format!("fill-{}", args.operation),
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Transform(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            if args.operation != "add"
                && args.operation != "set"
                && (args.kind.is_some()
                    || args.params.is_some()
                    || args.quad.is_some()
                    || args.mask.is_some()
                    || args.opacity.is_some()
                    || args.no_mask)
            {
                anyhow::bail!("transform settings apply only to add/set")
            }
            if args.operation != "add" && args.operation != "move" && args.index.is_some() {
                anyhow::bail!("--index applies only to transform add/move")
            }
            let geometry_supplied =
                args.quad.is_some() || args.params.is_some() || args.kind.is_some();
            if args.operation == "set"
                && !geometry_supplied
                && args.mask.is_none()
                && args.opacity.is_none()
                && !args.no_mask
            {
                anyhow::bail!("transform set requires geometry, opacity, or a mask setting")
            }
            if args.operation == "add" && args.no_mask {
                anyhow::bail!("--no-mask applies only to existing transforms")
            }
            let params = if let Some(quad) = args.quad {
                serde_json::json!({"quad":serde_json::from_str::<serde_json::Value>(&quad)?})
            } else {
                args.params
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or(serde_json::json!({}))
            };
            let mut result = if args.operation == "set" && !geometry_supplied {
                serde_json::json!({"node":args.id,"transform":args.op_id})
            } else {
                pentool::transform::edit(
                    &mut raw,
                    selected_page,
                    &args.id,
                    &args.operation,
                    &args.op_id,
                    args.kind.as_deref(),
                    &params,
                    args.index,
                )?
            };
            let mut metadata = serde_json::Map::new();
            if let Some(name) = args.mask {
                metadata.insert("mask".into(), serde_json::json!(name));
            }
            if args.no_mask {
                metadata.insert("mask".into(), serde_json::Value::Null);
            }
            if let Some(opacity) = args.opacity {
                metadata.insert("opacity".into(), serde_json::json!(opacity));
            }
            if !metadata.is_empty() {
                result = pentool::transform::metadata(
                    &mut raw,
                    selected_page,
                    &args.id,
                    &args.op_id,
                    &metadata.into(),
                )?;
            }
            let change = transaction::commit_value(
                &args.input,
                &format!("transform-{}", args.operation),
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Clip(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            if args.operation != "add" && args.base.is_some() {
                anyhow::bail!("--base applies only to clip add")
            }
            let result = composite::clip_edit(
                &mut raw,
                selected_page,
                &args.operation,
                &args.id,
                args.base.as_deref(),
            )?;
            let change = transaction::commit_value(
                &args.input,
                &format!("clip-{}", args.operation),
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Mask(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            transaction::validate_value(&raw)?;
            if !composite::is_document(&raw) {
                anyhow::bail!("mask commands require `migrate --target 6`")
            }
            if args.operation == "list" {
                if args.id.is_some()
                    || args.mask.is_some()
                    || args.from.is_some()
                    || args.dry_run
                    || args.if_revision.is_some()
                {
                    anyhow::bail!("mask list accepts only the document")
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"masks":raw.get("masks").cloned().unwrap_or(serde_json::json!({}))})
                    )?
                );
                return Ok(());
            }
            let id = args
                .id
                .as_deref()
                .context("mask name or node ID is required")?;
            if args.operation != "create" && args.from.is_some() {
                anyhow::bail!("--from applies only to mask create")
            }
            if args.operation != "attach"
                && (args.mask.is_some()
                    || args.invert
                    || args.density != 1.0
                    || args.feather != 0.0
                    || args.unlinked
                    || !args.transform.is_empty())
            {
                anyhow::bail!("mask attachment settings apply only to mask attach")
            }
            let settings = serde_json::json!({"invert":args.invert,"density":args.density,"feather":args.feather,"linked":!args.unlinked,
                "transform":if args.transform.is_empty() {vec![1.0,0.0,0.0,1.0,0.0,0.0]} else {args.transform}});
            let result = match args.operation.as_str() {
                "create" => pentool::mask::create(
                    &mut raw,
                    &args.input,
                    selected_page,
                    id,
                    args.from
                        .as_deref()
                        .context("mask create requires --from")?,
                )?,
                "attach" => pentool::mask::attach(
                    &mut raw,
                    selected_page,
                    id,
                    args.mask
                        .as_deref()
                        .context("mask attach requires a mask name")?,
                    &settings,
                )?,
                "detach" => pentool::mask::detach(&mut raw, selected_page, id)?,
                "delete" => pentool::mask::delete(&mut raw, id)?,
                "apply" => pentool::mask::apply(&mut raw, &args.input, selected_page, id)?,
                other => anyhow::bail!("unknown mask operation {other}"),
            };
            let change = transaction::commit_value(
                &args.input,
                &format!("mask-{}", args.operation),
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Adjustment(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            if args.operation == "list" {
                if args.id.is_some()
                    || args.kind.is_some()
                    || args.params.is_some()
                    || args.scope.is_some()
                    || args.layer.is_some()
                    || args.opacity.is_some()
                    || args.dry_run
                    || args.if_revision.is_some()
                {
                    anyhow::bail!("adjustment list accepts only the document and page selection")
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&composite::list(&raw, selected_page)?)?
                );
                return Ok(());
            }
            let id = args
                .id
                .as_deref()
                .context("adjustment node ID is required")?;
            let mut settings = serde_json::Map::new();
            if let Some(kind) = args.kind {
                settings.insert("adjustment".into(), serde_json::json!(kind));
            }
            if let Some(params) = args.params {
                settings.insert(
                    "params".into(),
                    serde_json::from_str(&params).context("--params must be valid JSON")?,
                );
            }
            if let Some(scope) = args.scope {
                settings.insert("scope".into(), composite::parse_scope(&scope)?);
            }
            if let Some(layer) = args.layer {
                settings.insert("layer".into(), serde_json::json!(layer));
            }
            if let Some(opacity) = args.opacity {
                settings.insert("opacity".into(), serde_json::json!(opacity));
            }
            if args.operation != "add" && settings.contains_key("layer") {
                anyhow::bail!("--layer applies only to adjustment add")
            }
            if args.operation != "add" && args.operation != "set" && !settings.is_empty() {
                anyhow::bail!("settings apply only to adjustment add/set")
            }
            if args.operation == "set" && settings.is_empty() {
                anyhow::bail!("adjustment set requires at least one setting")
            }
            let result = composite::edit(
                &mut raw,
                selected_page,
                &args.operation,
                id,
                &settings.into(),
            )?;
            let change = transaction::commit_value(
                &args.input,
                &format!("adjustment-{}", args.operation),
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Group(args) => {
            let GroupArgs {
                input,
                operation,
                id,
                layer,
                children,
                id_new,
                dx,
                dy,
                degrees,
                scale_x,
                scale_y,
                index,
                child,
                component,
                dry_run,
                if_revision,
            } = *args;
            let bytes =
                fs::read(&input).with_context(|| format!("could not read {}", input.display()))?;
            let mut raw: serde_json::Value =
                serde_json::from_slice(&bytes).context("invalid .pen document")?;
            if matches!(
                operation,
                scene::GroupOperation::Promote | scene::GroupOperation::Instantiate
            ) {
                let result = match operation {
                    scene::GroupOperation::Promote => scene::promote_component(
                        &mut raw,
                        selected_page,
                        &id,
                        &component.context("--component is required for group promote")?,
                    )?,
                    scene::GroupOperation::Instantiate => scene::instantiate_component(
                        &mut raw,
                        selected_page,
                        &layer.context("--layer is required for group instantiate")?,
                        &component.context("--component is required for group instantiate")?,
                        &id,
                        dx,
                        dy,
                    )?,
                    _ => unreachable!(),
                };
                let change = transaction::commit_value(
                    &input,
                    "group",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
                return Ok(());
            }
            let query = matches!(operation, scene::GroupOperation::Bounds);
            let action = match operation {
                scene::GroupOperation::Create => scene::GroupAction::Create {
                    id,
                    layer: layer.context("--layer is required for group create")?,
                    children,
                },
                scene::GroupOperation::Move => scene::GroupAction::Move { id, dx, dy },
                scene::GroupOperation::Rotate => scene::GroupAction::Rotate { id, degrees },
                scene::GroupOperation::Scale => scene::GroupAction::Scale {
                    id,
                    scale_x,
                    scale_y,
                },
                scene::GroupOperation::Duplicate => scene::GroupAction::Duplicate {
                    source: id,
                    id: id_new.context("--id-new is required for group duplicate")?,
                    dx,
                    dy,
                },
                scene::GroupOperation::Ungroup => scene::GroupAction::Ungroup { id },
                scene::GroupOperation::Rename => scene::GroupAction::Rename {
                    id,
                    new_id: id_new.context("--id-new is required for group rename")?,
                },
                scene::GroupOperation::Reorder => scene::GroupAction::Reorder {
                    id,
                    index: index.context("--index is required for group reorder")?,
                },
                scene::GroupOperation::Bounds => scene::GroupAction::Bounds { id },
                scene::GroupOperation::AddChild => scene::GroupAction::AddChild {
                    group: id,
                    child: child.context("--child is required for group add-child")?,
                },
                scene::GroupOperation::RemoveChild => scene::GroupAction::RemoveChild {
                    group: id,
                    child: child.context("--child is required for group remove-child")?,
                    layer: layer.context("--layer is required for group remove-child")?,
                },
                scene::GroupOperation::Promote | scene::GroupOperation::Instantiate => {
                    unreachable!()
                }
            };
            let result = scene::apply_group(&mut raw, selected_page, action)?;
            if query {
                println!("{}", serde_json::to_string_pretty(&result)?);
                return Ok(());
            }
            let change =
                transaction::commit_value(&input, "group", dry_run, if_revision.as_deref(), &raw)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Style(args) => {
            let bytes = fs::read(&args.input)
                .with_context(|| format!("could not read {}", args.input.display()))?;
            let mut raw: serde_json::Value =
                serde_json::from_slice(&bytes).context("invalid .pen document")?;
            let result = style::apply(
                &mut raw,
                args.operation,
                args.name.as_deref(),
                args.token_type.as_deref(),
                args.value.as_deref(),
                args.to.as_deref(),
            )?;
            if matches!(
                args.operation,
                style::Operation::List | style::Operation::Usage
            ) {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                let change = transaction::commit_value(
                    &args.input,
                    "style",
                    args.dry_run,
                    args.if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"change":change,"result":result})
                    )?
                );
            }
            Ok(())
        }
        Command::Replace(args) => {
            let (property, from) = match (&args.fill, &args.stroke) {
                (Some(value), None) => ("fill", value.as_str()),
                (None, Some(value)) => ("stroke", value.as_str()),
                _ => anyhow::bail!("specify exactly one of --fill or --stroke"),
            };
            let bytes = fs::read(&args.input)
                .with_context(|| format!("could not read {}", args.input.display()))?;
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            let result = replace::apply(
                &mut raw,
                &replace::Options {
                    page: args.page.as_deref().or(selected_page),
                    all_pages: args.all_pages,
                    layer: args.layer.as_deref(),
                    group: args.group.as_deref(),
                    object_type: args.object_type.as_deref(),
                    visible: args.visible,
                    property,
                    from,
                    to: &args.to,
                },
            )?;
            let change = transaction::commit_value(
                &args.input,
                "replace",
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::Format(args) => {
            let before = fs::read(&args.input)
                .with_context(|| format!("could not read {}", args.input.display()))?;
            let parse_started = std::time::Instant::now();
            let value: serde_json::Value = serde_json::from_slice(&before)?;
            let parse_us = parse_started.elapsed().as_micros();
            transaction::validate_value(&value)?;
            let serialize_started = std::time::Instant::now();
            let after = if args.compact {
                serde_json::to_vec(&value)?
            } else {
                serde_json::to_vec_pretty(&value)?
            };
            let serialize_us = serialize_started.elapsed().as_micros();
            let change = transaction::commit_bytes(
                &args.input,
                "format",
                args.dry_run,
                args.if_revision.as_deref(),
                &after,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "change":change,
                    "mode":if args.compact {"compact"} else {"pretty"},
                    "saved_bytes":before.len().saturating_sub(after.len()),
                    "parse_us":parse_us,
                    "serialize_us":serialize_us
                }))?
            );
            Ok(())
        }
        Command::Diff(args) => {
            let before: serde_json::Value = serde_json::from_slice(&fs::read(&args.old)?)?;
            let after: serde_json::Value = serde_json::from_slice(&fs::read(&args.new)?)?;
            transaction::validate_value(&before)?;
            transaction::validate_value(&after)?;
            let changes = diff::structural(&before, &after);
            let mut artifacts = Vec::new();
            if args.visual {
                let base = args
                    .new
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("document");
                let directory = args.new.parent().unwrap_or_else(|| Path::new("."));
                let old_png = directory.join(format!("{base}.diff-before.png"));
                let new_png = directory.join(format!("{base}.diff-after.png"));
                let raster = |raw: &serde_json::Value, path: &PathBuf| -> Result<Vec<u8>> {
                    if composite::is_document(raw) {
                        if let Some(id) = args.node.as_deref() {
                            let pixels = composite::node_pixels(raw, path, selected_page, id, 1.0)?;
                            let mut bytes = std::io::Cursor::new(Vec::new());
                            ::image::DynamicImage::ImageRgba8(pixels)
                                .write_to(&mut bytes, ::image::ImageFormat::Png)?;
                            Ok(bytes.into_inner())
                        } else {
                            composite::png(raw, path, selected_page, 1.0)
                        }
                    } else {
                        if args.node.is_some() {
                            anyhow::bail!("--node visual isolation requires v6")
                        }
                        if raw["version"] == 5 {
                            render::scene_to_png(&image::to_svg(raw, path, selected_page)?, 1.0)
                        } else {
                            render::to_png(&read_render_document(path, selected_page)?, 1.0)
                        }
                    }
                };
                let before_bytes = raster(&before, &args.old)?;
                let after_bytes = raster(&after, &args.new)?;
                editing::atomic_write(&old_png, &before_bytes)?;
                editing::atomic_write(&new_png, &after_bytes)?;
                artifacts.push(old_png);
                artifacts.push(new_png);
            }
            let result = serde_json::json!({"equal":changes.is_empty(),"change_count":changes.len(),"changes":changes,"visual_artifacts":artifacts});
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Command::Layout(args) => {
            let bytes = fs::read(&args.input)?;
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            let result = layout::apply(
                &mut raw,
                selected_page,
                args.operation,
                &args.ids,
                args.gap,
                args.columns,
                args.key.as_deref(),
            )?;
            let change = transaction::commit_value(
                &args.input,
                "layout",
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::TextBox(args) => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&args.input)?)?;
            let result = scene::put_text_box(
                &mut raw,
                selected_page,
                &args.layer,
                &args.id,
                scene::TextBoxInput {
                    content: args.content,
                    x: args.x,
                    y: args.y,
                    width: args.width,
                    height: args.height,
                    font: args.font,
                    size: args.size,
                    weight: args.weight,
                    fill: args.fill,
                    align: args.align,
                    vertical_align: args.vertical_align,
                    line_height: args.line_height,
                    anchor: args.anchor,
                    overflow: args.overflow,
                },
            )?;
            let change = transaction::commit_value(
                &args.input,
                "text-box",
                args.dry_run,
                args.if_revision.as_deref(),
                &raw,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":result})
                )?
            );
            Ok(())
        }
        Command::History {
            target,
            input,
            keep,
            dry_run,
        } => {
            let result = if target == "prune" {
                history::prune(
                    &input.context("history prune requires a document path")?,
                    keep,
                    dry_run,
                )?
            } else {
                if input.is_some() {
                    anyhow::bail!("unexpected document path after {target}")
                }
                history::list(Path::new(&target))?
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Command::Undo { input } => {
            println!("{}", serde_json::to_string_pretty(&history::undo(&input)?)?);
            Ok(())
        }
        Command::Redo { input } => {
            println!("{}", serde_json::to_string_pretty(&history::redo(&input)?)?);
            Ok(())
        }
        Command::Restore { input, revision } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&history::restore(&input, &revision)?)?
            );
            Ok(())
        }
        Command::Export {
            input,
            output,
            scale,
            outline_text,
            all_pages,
            format,
            link_images,
        } => {
            if !scale.is_finite() || !(0.1..=8.0).contains(&scale) {
                anyhow::bail!("export scale must be finite and between 0.1 and 8")
            }
            if link_images
                && (all_pages || output.extension().and_then(|e| e.to_str()) != Some("svg"))
            {
                anyhow::bail!(
                    "[unsupported-capability] --link-images applies to a single-page .svg output"
                )
            }
            let wants_pdf = output.extension().and_then(|value| value.to_str()) == Some("pdf");
            if all_pages || wants_pdf {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let page_ids: Vec<String> = raw
                    .get("pages")
                    .and_then(serde_json::Value::as_array)
                    .map(|pages| {
                        pages
                            .iter()
                            .filter_map(|page| {
                                page.get("id")
                                    .and_then(serde_json::Value::as_str)
                                    .map(Into::into)
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["page-1".into()]);
                let has_images = composite::is_document(&raw)
                    || raw.get("version").and_then(serde_json::Value::as_u64)
                        == Some(image::VERSION)
                        && raw
                            .get("image_assets")
                            .and_then(serde_json::Value::as_object)
                            .is_some_and(|assets| !assets.is_empty());
                if wants_pdf {
                    if has_images {
                        if outline_text {
                            anyhow::bail!(
                                "[unsupported-capability] outlined text with image scenes is not implemented"
                            )
                        }
                        let pages = page_ids
                            .iter()
                            .map(|id| {
                                let factor = f64::from(scale);
                                if composite::is_document(&raw) {
                                    let texts = composite::text_lines(&raw, &input, Some(id))?
                                        .into_iter()
                                        .map(|line| image::TextLine {
                                            x: line.x * factor,
                                            y: line.y * factor,
                                            size: line.size * factor,
                                            content: line.content,
                                        })
                                        .collect();
                                    return Ok((
                                        composite::png(&raw, &input, Some(id), scale)?,
                                        texts,
                                    ));
                                }
                                let scene = image::to_svg(&raw, &input, Some(id))?;
                                let texts = scene
                                    .texts
                                    .iter()
                                    .map(|line| image::TextLine {
                                        x: line.x * factor,
                                        y: line.y * factor,
                                        size: line.size * factor,
                                        content: line.content.clone(),
                                    })
                                    .collect();
                                Ok((render::scene_to_png(&scene, scale)?, texts))
                            })
                            .collect::<Result<Vec<_>>>()?;
                        pdf::write_png_pages(&pages, &output)?;
                    } else {
                        let docs = page_ids
                            .iter()
                            .map(|id| read_render_document(&input, Some(id)))
                            .collect::<Result<Vec<_>>>()?;
                        pdf::write(&docs, &output)?;
                    }
                } else {
                    let extension = format.as_deref().unwrap_or("png");
                    if !matches!(extension, "png" | "svg") {
                        anyhow::bail!("--format must be png or svg")
                    }
                    let parent = output.parent().unwrap_or_else(|| Path::new("."));
                    let name = output
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("export");
                    let staging =
                        parent.join(format!(".{name}.pentool-stage-{}", std::process::id()));
                    fs::create_dir_all(&staging)?;
                    for (index, id) in page_ids.iter().enumerate() {
                        let safe: String = id
                            .chars()
                            .map(|c| {
                                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                                    c
                                } else {
                                    '-'
                                }
                            })
                            .collect();
                        let destination =
                            staging.join(format!("{:03}-{safe}.{extension}", index + 1));
                        if has_images {
                            write_image_export(
                                &input,
                                &raw,
                                Some(id),
                                &destination,
                                scale,
                                outline_text,
                                false,
                            )?;
                        } else {
                            let doc = read_render_document(&input, Some(id))?;
                            render::write_export_options(&doc, &destination, scale, outline_text)?;
                        }
                    }
                    let previous =
                        parent.join(format!(".{name}.pentool-old-{}", std::process::id()));
                    if output.exists() {
                        fs::rename(&output, &previous)?;
                    }
                    if let Err(error) = fs::rename(&staging, &output) {
                        if previous.exists() {
                            let _ = fs::rename(&previous, &output);
                        }
                        return Err(error.into());
                    }
                    if previous.exists() {
                        fs::remove_dir_all(previous)?;
                    }
                }
            } else {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                if composite::is_document(&raw)
                    || raw.get("version").and_then(serde_json::Value::as_u64)
                        == Some(image::VERSION)
                        && raw
                            .get("image_assets")
                            .and_then(serde_json::Value::as_object)
                            .is_some_and(|assets| !assets.is_empty())
                {
                    write_image_export(
                        &input,
                        &raw,
                        selected_page,
                        &output,
                        scale,
                        outline_text,
                        link_images,
                    )?;
                } else {
                    let doc = read_render_document(&input, selected_page)?;
                    render::write_export_options(&doc, &output, scale, outline_text)?;
                }
            }
            println!("Exported {}", output.display());
            Ok(())
        }
    }
}

fn write_image_export(
    input: &Path,
    raw: &serde_json::Value,
    page: Option<&str>,
    output: &Path,
    scale: f32,
    outline_text: bool,
    link_images: bool,
) -> Result<()> {
    if composite::is_document(raw) {
        if outline_text || link_images {
            anyhow::bail!("[unsupported-capability] v6 composite exports require embedded pixels; omit --outline-text and --link-images")
        }
        let bytes = match output
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => composite::png(raw, input, page, scale)?,
            Some("svg") => composite::svg(raw, input, page, scale)?.into_bytes(),
            _ => anyhow::bail!("output must end in .png, .svg or .pdf"),
        };
        return editing::atomic_write(output, &bytes);
    }
    if outline_text {
        anyhow::bail!("[unsupported-capability] outlined text with image scenes is not implemented")
    }
    let scene = if link_images {
        let directory = pentool::resource::document_root(output);
        fs::create_dir_all(directory)?;
        image::to_svg_linked(raw, input, page, Some(directory))?
    } else {
        image::to_svg(raw, input, page)?
    };
    let bytes = match output
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("svg") => scene.svg.into_bytes(),
        Some("png") => render::scene_to_png(&scene, scale)?,
        _ => anyhow::bail!("output must end in .png, .svg or .pdf"),
    };
    editing::atomic_write(output, &bytes)
}

fn read_document(path: &PathBuf, page: Option<&str>) -> Result<Document> {
    let doc = read_document_unvalidated(path, page)?;
    doc.validate().map_err(anyhow::Error::msg)?;
    Ok(doc)
}

fn read_document_unvalidated(path: &PathBuf, page: Option<&str>) -> Result<Document> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let mut doc: Document = serde_json::from_slice(&bytes).context("invalid .pen document")?;
    select_page(&mut doc, page)?;
    Ok(doc)
}

fn read_render_document(path: &PathBuf, page: Option<&str>) -> Result<Document> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let raw: serde_json::Value = serde_json::from_slice(&bytes).context("invalid .pen document")?;
    let flattened = if scene::is_scene_document(&raw) {
        scene::flatten_to_v3(&raw)?
    } else {
        raw
    };
    let mut doc: Document = serde_json::from_value(flattened).context("invalid .pen document")?;
    select_page(&mut doc, page)?;
    Ok(doc)
}

fn select_page(doc: &mut Document, page: Option<&str>) -> Result<()> {
    if let Some(id) = page {
        doc.select_page(id).map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

fn scope_paths(root: &Path, scope: &Scope) -> Result<(PathBuf, PathBuf)> {
    Ok(match scope {
        Scope::Project => (library::project_config(root), library::project_index(root)),
        Scope::User => (library::user_config()?, library::user_index()?),
    })
}
fn all_indexes(root: &Path) -> Result<library::AssetIndex> {
    library::merged_indexes(
        Some(&library::project_index(root)),
        library::user_index().ok().as_deref(),
    )
}
fn resolve_asset_path(root: &Path, spec: &str) -> Result<PathBuf> {
    let direct = PathBuf::from(spec);
    if direct.is_file() {
        return Ok(direct);
    }
    let idx = all_indexes(root)?;
    Ok(library::resolve(&idx, spec)?.path.clone())
}
fn json_pretty(value: serde_json::Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(&value)?)
}
fn next_instance_id(raw: &serde_json::Value) -> String {
    let used: std::collections::HashSet<_> = raw
        .get("instances")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.get("id").and_then(serde_json::Value::as_str))
        .collect();
    (1..)
        .map(|n| format!("instance-{n}"))
        .find(|id| !used.contains(id.as_str()))
        .unwrap()
}
fn activate_installed_package(root: &Path, report: &mut serde_json::Value) -> Result<()> {
    let Some(folder) = report
        .get("installed")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
    else {
        return Ok(());
    };
    let name = report
        .get("package")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("package");
    let version = report
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("0");
    let library_name = format!("pkg-{}-{version}", name.replace('/', "-"));
    let config = library::project_config(root);
    let index = library::project_index(root);
    match library::add(root, &config, &library_name, &folder) {
        Ok(_) => {}
        Err(e) if e.to_string().contains("already registered") => {}
        Err(e) => return Err(e),
    };
    let refreshed = library::refresh(root, &config, &index, None)?;
    report["library"] = serde_json::Value::String(library_name);
    report["index_refresh"] = refreshed;
    Ok(())
}

use pentool::{
    agent, ai, asset, benchmark, comments, composite, diff, document, editing, extension, fonts,
    geometry, history, image, import, instance, json, layout, library, package, page, pdf, raster,
    render, replace, scene, server, style, text, transaction,
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
    /// Actor kind recorded in the collaboration log: human (default) or agent.
    #[arg(long, global = true, value_name = "KIND")]
    actor_kind: Option<String>,
    /// Stable actor handle recorded in the collaboration log (required for agents).
    #[arg(long, global = true, value_name = "ID")]
    actor_id: Option<String>,
    /// Display name recorded in the collaboration log.
    #[arg(long, global = true, value_name = "NAME")]
    actor_name: Option<String>,
    /// Human the agent acts for (required for agents); recorded in the collaboration log.
    #[arg(long, global = true, value_name = "HUMAN")]
    directed_by: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum CollabAction {
    /// Start recording operations for a v4 document (writes only under .pentool/collab).
    Init { input: PathBuf },
    /// List recorded operations, paginated.
    Log {
        input: PathBuf,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Only operations by this actor id.
        #[arg(long)]
        actor: Option<String>,
    },
    /// Rebuild the document from the log and report conflicts; never modifies DOC.
    Replay {
        input: PathBuf,
        /// Write the replayed document to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Verify the hash chain, operation validity and replay.
    Verify { input: PathBuf },
    /// Issue, list and revoke access tokens (owner only; secrets are shown once).
    Token {
        #[command(subcommand)]
        action: CollabTokenAction,
    },
    /// Register, list and remove sync peers.
    Peer {
        #[command(subcommand)]
        action: CollabPeerAction,
    },
    /// Pull and push missing operations with a registered peer (idempotent, resumable).
    Sync {
        input: PathBuf,
        /// Registered peer name or URL.
        #[arg(long)]
        peer: String,
        /// Access token; defaults to the PENTOOL_COLLAB_TOKEN environment variable.
        #[arg(long)]
        token: Option<String>,
        /// Allow plain HTTP to a non-loopback peer on a trusted network.
        #[arg(long)]
        allow_insecure_lan: bool,
    },
    /// List who is connected to a peer right now (best-effort, in-memory presence).
    Presence {
        input: PathBuf,
        /// Registered peer name or URL.
        #[arg(long)]
        peer: String,
        /// Access token; defaults to the PENTOOL_COLLAB_TOKEN environment variable.
        #[arg(long)]
        token: Option<String>,
        /// Allow plain HTTP to a non-loopback peer on a trusted network.
        #[arg(long)]
        allow_insecure_lan: bool,
        /// Print the full JSON snapshot instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Create a new local copy of a peer's document and pull its history.
    Join {
        /// New document path (must not exist).
        input: PathBuf,
        #[arg(long)]
        peer: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        allow_insecure_lan: bool,
    },
}

#[derive(Subcommand)]
enum CollabTokenAction {
    /// Create a token for the actor named by --actor-kind/--actor-id/--actor-name/--directed-by.
    Create {
        input: PathBuf,
        /// viewer, commenter, editor or owner.
        #[arg(long)]
        role: String,
        /// Label shown in `token list`.
        #[arg(long)]
        name: String,
        /// Lifetime such as 30m, 12h or 7d.
        #[arg(long)]
        expires: Option<String>,
    },
    /// List tokens (never their secrets).
    List { input: PathBuf },
    /// Revoke a token by id; the effect is immediate.
    Revoke { input: PathBuf, id: String },
    /// Change a token's role.
    SetRole {
        input: PathBuf,
        id: String,
        #[arg(long)]
        role: String,
    },
}

#[derive(Subcommand)]
enum CollabPeerAction {
    /// Register a peer URL such as http://192.168.1.20:4711.
    Add {
        input: PathBuf,
        url: String,
        #[arg(long)]
        name: Option<String>,
    },
    List {
        input: PathBuf,
    },
    Remove {
        input: PathBuf,
        peer: String,
    },
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
    /// Also compare the scope against itself with effects disabled and masks
    /// stripped (effects-on vs effects-off, not a diff between files).
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
    /// Gradient/pattern JSON. Native v8 gradient space objects require an explicit intent.
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
    #[arg(long, value_enum, default_value = "left")]
    align: document::TextAlign,
    #[arg(long, value_enum, default_value = "top")]
    vertical_align: scene::TextVerticalAlign,
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
    /// v9 text frames.
    TextFrame,
    /// v9 graphic frames.
    GraphicFrame,
}

impl SearchKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Text => "text",
            Self::Image => "image",
            Self::Raster => "raster",
            Self::TextFrame => "text-frame",
            Self::GraphicFrame => "graphic-frame",
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
            Self::TextFrame | Self::GraphicFrame => anyhow::bail!(
                "[unsupported-capability] frame kinds require a v9 document; migrate with `pentool migrate --target 9`"
            ),
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum DesignFormat {
    Svg,
    Pdf,
    Psd,
    Sketch,
    Figma,
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

#[derive(Args)]
struct CommentActor {
    /// Display name of the commenter.
    #[arg(long, default_value = "local")]
    author: String,
    /// Stable actor id.
    #[arg(long, default_value = "local")]
    actor_id: String,
    /// human or agent.
    #[arg(long, default_value = "human")]
    kind: String,
    /// For agents: the model or tool name.
    #[arg(long)]
    model: Option<String>,
    /// For agents: the human who directed the work.
    #[arg(long)]
    directed_by: Option<String>,
}

impl CommentActor {
    fn actor(self) -> comments::Actor {
        comments::Actor {
            kind: self.kind,
            id: self.actor_id,
            name: self.author,
            model: self.model,
            directed_by: self.directed_by,
        }
    }
}

#[derive(Subcommand)]
enum CommentAction {
    /// Start a thread on the document, a page, an object or a point.
    Add {
        input: PathBuf,
        body: String,
        #[arg(long)]
        page: Option<String>,
        #[arg(long)]
        object: Option<String>,
        #[arg(long, requires = "y", allow_negative_numbers = true)]
        x: Option<f64>,
        #[arg(long, requires = "x", allow_negative_numbers = true)]
        y: Option<f64>,
        #[command(flatten)]
        actor: CommentActor,
        /// Print compact single-line JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reply to a thread.
    Reply {
        input: PathBuf,
        thread: String,
        body: String,
        #[arg(long)]
        if_version: Option<u64>,
        #[command(flatten)]
        actor: CommentActor,
        #[arg(long)]
        json: bool,
    },
    /// Mark a thread resolved.
    Resolve {
        input: PathBuf,
        thread: String,
        #[arg(long)]
        if_version: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// Reopen a resolved thread.
    Reopen {
        input: PathBuf,
        thread: String,
        #[arg(long)]
        if_version: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// Set a review state: none, requested, approved or changes-requested.
    Review {
        input: PathBuf,
        thread: String,
        state: String,
        #[arg(long)]
        if_version: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// List threads (compact, paginated); --thread shows one with its comments.
    List {
        input: PathBuf,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        review_state: Option<String>,
        #[arg(long)]
        page: Option<String>,
        #[arg(long)]
        object: Option<String>,
        #[arg(long)]
        mention: Option<String>,
        #[arg(long)]
        thread: Option<String>,
        /// Include every comment body.
        #[arg(long)]
        comments: bool,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
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
    /// Change a variant's develop settings: profiles, white balance, geometry, tone and the rest.
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
        /// none, embedded-opcodes, or a lens profile digest.
        #[arg(long)]
        lens_profile: Option<String>,
        /// Set a develop value: KEY=JSON with a dotted key, e.g. lens.distortion=12 (repeatable).
        #[arg(long = "set", value_name = "KEY=JSON")]
        set: Vec<String>,
        /// Remove a develop value by dotted key (repeatable).
        #[arg(long = "unset", value_name = "KEY")]
        unset: Vec<String>,
        /// Upright analysis: off, level, vertical, full, or guided.
        #[arg(long)]
        upright: Option<String>,
        /// A guided-upright line x1,y1,x2,y2 (0-1, oriented frame; repeatable, at most 4).
        #[arg(long = "guide", value_name = "X1,Y1,X2,Y2")]
        guide: Vec<String>,
        /// Exposure in EV (-5..5); sets tone.exposure.
        #[arg(long, allow_hyphen_values = true)]
        exposure: Option<f64>,
        /// Resolve tone.exposure, highlights and shadows with auto tone.
        #[arg(long)]
        auto_tone: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
}

#[derive(Subcommand)]
enum PhotoCacheAction {
    /// Remove every cached preview of a document; the document is not changed.
    Clear { input: PathBuf },
}

#[derive(Subcommand)]
enum PhotoAction {
    /// Manage verified camera and lens profiles in photography.profiles.
    Profile {
        #[command(subcommand)]
        action: PhotoProfileAction,
    },
    /// Paint brush masks of local adjustments.
    Mask {
        #[command(subcommand)]
        action: PhotoMaskAction,
    },
    /// Develop a photo variant and write it as a PNG.
    Render {
        input: PathBuf,
        /// Photo ID.
        id: String,
        #[arg(long, default_value = "master")]
        variant: String,
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        output: OutputArgs,
        #[command(flatten)]
        metadata: MetadataArgs,
    },
    /// Develop a variant through output rendering and report its histogram,
    /// gamut and luminance without writing a file.
    Inspect {
        input: PathBuf,
        /// Photo ID.
        id: String,
        #[arg(long, default_value = "master")]
        variant: String,
        #[command(flatten)]
        output: OutputArgs,
        /// Histogram bins per channel (1 to 1024).
        #[arg(long, default_value_t = 64)]
        bins: usize,
    },
    /// Print a variant's resolved frame (sizes, crop, lens steps) without decoding.
    Info {
        input: PathBuf,
        /// Photo ID.
        id: String,
        #[arg(long, default_value = "master")]
        variant: String,
    },
    /// Place a variant on a page layer as a `photo` node. Without --width and
    /// --height the node takes the developed size; with one, the other follows
    /// the developed aspect ratio. Use --page to choose a page.
    Place {
        input: PathBuf,
        /// Photo ID.
        id: String,
        #[arg(long, default_value = "master")]
        variant: String,
        #[arg(long)]
        layer: String,
        /// Node ID (default: PHOTO-VARIANT, numbered when taken).
        #[arg(long = "id")]
        node_id: Option<String>,
        #[arg(long, default_value_t = 0.0)]
        x: f64,
        #[arg(long, default_value_t = 0.0)]
        y: f64,
        #[arg(long)]
        width: Option<f64>,
        #[arg(long)]
        height: Option<f64>,
        #[arg(long, value_enum, default_value = "contain")]
        fit: image::Fit,
        /// Focal position within the frame, as X,Y in 0–1.
        #[arg(long, value_delimiter = ',', default_value = "0.5,0.5")]
        position: Vec<f64>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Write the editor's 8-bit preview of a variant (served from the preview
    /// cache when an identical preview was rendered before).
    Preview {
        input: PathBuf,
        /// Photo ID.
        id: String,
        #[arg(long, default_value = "master")]
        variant: String,
        #[arg(long)]
        out: PathBuf,
        /// Long edge in pixels (1 to 4096); smaller develops are not enlarged.
        #[arg(long, default_value_t = 1600)]
        edge: u32,
        #[arg(long, default_value = "srgb", value_parser = ["srgb", "display-p3"])]
        space: String,
        /// none, clipping, gamut or mask:ADJUSTMENT.
        #[arg(long, default_value = "none")]
        overlay: String,
        /// Ignore the variant's crop.
        #[arg(long)]
        uncropped: bool,
        /// Render instead of reading or writing `.pentool/cache/photo/`.
        #[arg(long)]
        no_cache: bool,
    },
    /// Manage the disposable preview cache in `.pentool/cache/photo/`.
    Cache {
        #[command(subcommand)]
        action: PhotoCacheAction,
    },
    /// Merge exposure brackets into a new photo with a derived scene-linear DNG source.
    MergeHdr {
        input: PathBuf,
        /// Photo IDs of the brackets (2 to 9).
        #[arg(required = true, num_args = 2..)]
        photos: Vec<String>,
        /// ID of the new photo.
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: Option<String>,
        /// Ghost rejection: off, low, medium or high.
        #[arg(long, default_value = "off")]
        deghost: String,
        /// 1-based input that anchors alignment and exposure; default: the middle exposure.
        #[arg(long)]
        reference: Option<usize>,
        #[command(flatten)]
        common: MergeArgs,
    },
    /// Stitch overlapping photos, given left to right, into a new photo with a derived DNG source.
    MergePano {
        input: PathBuf,
        /// Photo IDs of the frames (2 to 64), left to right.
        #[arg(required = true, num_args = 2..)]
        photos: Vec<String>,
        /// ID of the new photo.
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: Option<String>,
        /// spherical, cylindrical or perspective.
        #[arg(long, default_value = "cylindrical")]
        projection: String,
        /// Seed of the deterministic RANSAC sampler.
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Focal length in output pixels; default: EXIF assuming a 36 mm long edge.
        #[arg(long)]
        focal: Option<f64>,
        #[command(flatten)]
        common: MergeArgs,
    },
    /// Add, rename or remove virtual copies (variants) of a photo.
    Variant {
        #[command(subcommand)]
        action: PhotoVariantAction,
    },
    /// Save, restore or remove immutable snapshots of a variant's settings.
    Snapshot {
        #[command(subcommand)]
        action: PhotoSnapshotAction,
    },
    /// Synchronize develop settings between variants.
    Settings {
        #[command(subcommand)]
        action: PhotoSettingsAction,
    },
    /// Search the catalog: rows in catalog order, paginated. An empty query lists every photo.
    Search {
        input: PathBuf,
        /// Terms combined with AND, for example "rating>=4 keyword:\"blue hour\" camera:fuji".
        #[arg(default_value = "")]
        query: String,
        #[arg(long, default_value_t = pentool::photo::organize::DEFAULT_LIMIT)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Set the rating, pick flag or color label of selected photos.
    Rate {
        input: PathBuf,
        /// Comma-separated photo IDs, a search query, or @FILE.json (an array of IDs).
        selection: String,
        /// 0–5; 0 removes the rating.
        #[arg(long)]
        rating: Option<u64>,
        /// pick, reject or none.
        #[arg(long)]
        pick: Option<String>,
        /// none, red, yellow, green, blue or purple.
        #[arg(long)]
        label: Option<String>,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Add or remove keywords of selected photos (compared ignoring case).
    Keyword {
        input: PathBuf,
        /// Comma-separated photo IDs, a search query, or @FILE.json (an array of IDs).
        selection: String,
        #[arg(long, value_delimiter = ',')]
        add: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        remove: Vec<String>,
        /// Remove every keyword before adding.
        #[arg(long)]
        clear: bool,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Group related photos into stacks.
    Stack {
        #[command(subcommand)]
        action: PhotoStackAction,
    },
    /// Manage manual and smart collections.
    Collection {
        #[command(subcommand)]
        action: PhotoCollectionAction,
    },
    /// Write a captioned grid of selected photos to a PNG or PDF file.
    ContactSheet {
        input: PathBuf,
        /// Comma-separated photo IDs, a search query, or @FILE.json (an array of IDs).
        #[arg(long)]
        selection: String,
        #[arg(long, default_value_t = 4)]
        columns: u32,
        #[command(flatten)]
        sheet: SheetArgs,
        /// Variant shown for every photo.
        #[arg(long, default_value = "master")]
        variant: String,
    },
    /// Write photos or variants side by side (one row of up to 4, then a grid).
    Compare {
        input: PathBuf,
        /// 2–16 photo[/variant] items.
        #[arg(num_args = 2..=16, required = true)]
        items: Vec<String>,
        #[command(flatten)]
        sheet: SheetArgs,
    },
    /// Show a photo's metadata by category; gps, serials and identity are redacted unless revealed.
    Metadata {
        input: PathBuf,
        /// Photo ID.
        id: String,
        /// Show a private category: gps, serials or identity (repeatable).
        #[arg(long)]
        reveal: Vec<String>,
    },
    /// List sources whose bytes carry GPS, serial numbers or identity metadata.
    PrivacyReport { input: PathBuf },
    /// Set the title, caption, creator, copyright and location that exports may write.
    /// An empty value removes the field.
    Describe {
        input: PathBuf,
        /// Comma-separated photo IDs, a search query, or @FILE.json (an array of IDs).
        selection: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        caption: Option<String>,
        #[arg(long)]
        creator: Option<String>,
        #[arg(long)]
        copyright: Option<String>,
        #[arg(long)]
        sublocation: Option<String>,
        #[arg(long)]
        city: Option<String>,
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        country: Option<String>,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Export selected photos through a recipe into a directory. Outputs are
    /// staged and moved into place only when all succeed; the document is unchanged.
    Export {
        input: PathBuf,
        /// Comma-separated photo IDs, a search query, or @FILE.json (an array of IDs).
        #[arg(long)]
        selection: String,
        /// Built-in (web-gallery, social, archive-master, photo-lab) or document recipe.
        #[arg(long)]
        recipe: String,
        /// Output directory; created if missing.
        #[arg(long)]
        out: PathBuf,
        /// Export this variant of every photo (default: each master).
        #[arg(long, conflicts_with = "all_variants")]
        variant: Option<String>,
        /// Export every variant of every photo.
        #[arg(long)]
        all_variants: bool,
        /// Print size for a print recipe: WIDTHxHEIGHT in inches or centimeters, such as 6x4in.
        #[arg(long)]
        print: Option<String>,
        /// When the aspect differs from the print: error, crop or pad.
        #[arg(long)]
        fit: Option<String>,
        /// Replace files that already exist in the output directory.
        #[arg(long)]
        overwrite: bool,
        /// Report the planned outputs without writing them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage output recipes.
    Recipe {
        #[command(subcommand)]
        action: PhotoRecipeAction,
    },
}

#[derive(clap::Args)]
struct MetadataArgs {
    /// Metadata written to the file: none, copyright, public or all-including-private.
    #[arg(long, default_value = "none")]
    metadata: String,
    /// Also write these categories (comma-separated).
    #[arg(long, value_delimiter = ',')]
    metadata_include: Vec<String>,
    /// Never write these categories (comma-separated).
    #[arg(long, value_delimiter = ',')]
    metadata_exclude: Vec<String>,
}

impl MetadataArgs {
    fn policy(&self) -> Result<pentool::photo::metadata::Policy> {
        pentool::photo::metadata::Policy::new(
            &self.metadata,
            &self.metadata_include,
            &self.metadata_exclude,
        )
    }
}

#[derive(Subcommand)]
enum PhotoRecipeAction {
    /// List the built-in recipes and the document's recipes.
    List { input: PathBuf },
    /// Add or replace a document recipe.
    Set {
        input: PathBuf,
        name: String,
        /// Recipe JSON, or @FILE.json.
        #[arg(value_name = "JSON")]
        recipe: String,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Remove a document recipe.
    Remove {
        input: PathBuf,
        name: String,
        #[command(flatten)]
        edit: EditArgs,
    },
}

#[derive(clap::Args)]
struct SheetArgs {
    /// Cell size in pixels, 32–1024.
    #[arg(long, default_value_t = 256)]
    cell: u32,
    /// Caption template with {id}, {name}, {rating} and {variant}.
    #[arg(long, default_value = "{id}")]
    caption: String,
    /// Output file: .png or .pdf.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Subcommand)]
enum PhotoStackAction {
    /// Stack two or more photos; the first is the top.
    Add {
        input: PathBuf,
        stack: String,
        /// Comma-separated photo IDs (top first), a search query, or @FILE.json.
        photos: String,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Make PHOTO the top of the stack.
    Top {
        input: PathBuf,
        stack: String,
        photo: String,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Unstack; the photos stay in the catalog.
    Remove {
        input: PathBuf,
        stack: String,
        #[command(flatten)]
        edit: EditArgs,
    },
}

#[derive(Subcommand)]
enum PhotoCollectionAction {
    /// Add a manual collection (--photos) or a smart collection (--query).
    Add {
        input: PathBuf,
        id: String,
        #[arg(long)]
        name: Option<String>,
        /// Photos of a manual collection: IDs, a query (frozen now) or @FILE.json.
        #[arg(long, conflicts_with = "query", required_unless_present = "query")]
        photos: Option<String>,
        /// The stored query of a smart collection.
        #[arg(long)]
        query: Option<String>,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Add or remove photos of a manual collection.
    Update {
        input: PathBuf,
        id: String,
        #[arg(long)]
        add: Option<String>,
        #[arg(long)]
        remove: Option<String>,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Remove a collection; its photos stay in the catalog.
    Remove {
        input: PathBuf,
        id: String,
        #[command(flatten)]
        edit: EditArgs,
    },
}

#[derive(clap::Args)]
struct EditArgs {
    /// Report the change without writing the document.
    #[arg(long)]
    dry_run: bool,
    /// Refuse the change unless the document is at this revision.
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Subcommand)]
enum PhotoVariantAction {
    /// Add a variant with a copy of another variant's or a snapshot's settings.
    /// The source is shared, never duplicated.
    Add {
        input: PathBuf,
        /// Photo ID.
        photo: String,
        /// ID of the new variant.
        id: String,
        /// Copy the settings of this variant (default master).
        #[arg(long, conflicts_with = "from_snapshot")]
        from: Option<String>,
        /// Copy the settings of this snapshot instead.
        #[arg(long)]
        from_snapshot: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Set (or, without --name, clear) a variant's display name.
    Rename {
        input: PathBuf,
        photo: String,
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Remove a variant; master and variants with snapshots or photo nodes are refused.
    Remove {
        input: PathBuf,
        photo: String,
        id: String,
        #[command(flatten)]
        edit: EditArgs,
    },
}

#[derive(Subcommand)]
enum PhotoSnapshotAction {
    /// Save the current settings of PHOTO[/VARIANT] (default master) as a snapshot.
    Add {
        input: PathBuf,
        /// photo or photo/variant.
        target: String,
        /// ID of the new snapshot.
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Copy a snapshot's settings back into its variant.
    Restore {
        input: PathBuf,
        photo: String,
        id: String,
        #[command(flatten)]
        edit: EditArgs,
    },
    /// Remove a snapshot.
    Remove {
        input: PathBuf,
        photo: String,
        id: String,
        #[command(flatten)]
        edit: EditArgs,
    },
}

#[derive(Subcommand)]
enum PhotoSettingsAction {
    /// Copy whole settings groups from SOURCE[/VARIANT] to target variants in one transaction.
    Sync {
        input: PathBuf,
        /// photo or photo/variant (default master).
        source: String,
        /// Comma-separated photo[/variant] targets, or @FILE.json holding an array of them.
        #[arg(long)]
        to: String,
        /// Copy only these groups (comma-separated); default every group except process.
        #[arg(long, value_delimiter = ',')]
        groups: Vec<String>,
        /// Do not copy these groups (comma-separated), for example crop,geometry.
        #[arg(long, value_delimiter = ',')]
        except: Vec<String>,
        /// Run auto white balance, upright and auto tone again for each target.
        #[arg(long)]
        auto_per_photo: bool,
        #[command(flatten)]
        edit: EditArgs,
    },
}

#[derive(clap::Args)]
struct OutputArgs {
    /// Output color space: srgb, display-p3, adobe-rgb-1998, prophoto or rec2020,
    /// optionally with :linear. Default: srgb, or rec2020 with --hdr.
    #[arg(long)]
    space: Option<String>,
    /// Bits per channel: 8 or 16. Default: 8, or 16 with --hdr.
    #[arg(long)]
    depth: Option<u8>,
    /// Gamut mapping: perceptual or relative-colorimetric.
    #[arg(long, default_value = "perceptual")]
    intent: String,
    /// HDR encoding in rec2020: pq or hlg.
    #[arg(long)]
    hdr: Option<String>,
    /// HDR headroom in stops above 203 cd/m² SDR white (0 to 4; HLG at most about 1.92). Default 1.5.
    #[arg(long)]
    headroom: Option<f64>,
}

impl OutputArgs {
    fn resolve(&self) -> Result<pentool::photo::output::Output> {
        pentool::photo::output::Output::new(
            self.space.as_deref(),
            self.depth,
            &self.intent,
            self.hdr.as_deref(),
            self.headroom,
        )
    }
}

#[derive(clap::Args)]
struct MergeArgs {
    /// Scale applied to every input before merging (0 < scale <= 1).
    #[arg(long, default_value_t = 1.0)]
    scale: f64,
    /// Master settings: import (defaults) or first (the first input's look).
    #[arg(long, default_value = "import")]
    settings: String,
    /// Write the derived DNG to this document-relative path instead of embedding it.
    #[arg(long)]
    external: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Subcommand)]
enum PhotoMaskAction {
    /// Paint a brush mask of a local adjustment with the raster brush engine.
    /// Samples are in brush-plane pixels: the uncropped frame scaled to a long
    /// edge of at most 4096.
    Paint {
        input: PathBuf,
        /// Photo ID.
        id: String,
        #[arg(long, default_value = "master")]
        variant: String,
        /// The local adjustment ID (create it with `raw develop --set local=[...]`).
        #[arg(long)]
        adjustment: String,
        /// Index of the brush component to paint into; omit to add a new one.
        #[arg(long)]
        component: Option<usize>,
        /// JSON array of device events, as for `raster stroke`, or @file.json.
        #[arg(long)]
        samples: String,
        /// Brush JSON, or @file.json.
        #[arg(long, default_value = "{}")]
        brush: String,
        /// Name of a document brush preset to start from; --brush overrides it.
        #[arg(long)]
        preset: Option<String>,
        /// Remove coverage instead of adding it.
        #[arg(long)]
        erase: bool,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
}

#[derive(Subcommand)]
enum PhotoProfileAction {
    /// Verify and store a DNG camera profile (.dcp) or a Pentool lens profile (.json).
    Add {
        input: PathBuf,
        /// A DNG camera profile (.dcp).
        #[arg(long, required_unless_present = "lens", conflicts_with = "lens")]
        file: Option<PathBuf>,
        /// A Pentool lens profile (pentool_lens_profile JSON).
        #[arg(long)]
        lens: Option<PathBuf>,
        /// Allow the profile for sources whose UniqueCameraModel differs (recorded).
        #[arg(long, conflicts_with = "lens")]
        force_model: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Convert the supported subset of an Adobe lens profile (.lcp) and store it.
    ImportLcp {
        input: PathBuf,
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
}

#[derive(Subcommand)]
enum ColorAction {
    /// Interpolate two samples in explicit native channels, with separate alpha.
    Interpolate {
        input: PathBuf,
        /// Starting native color JSON.
        #[arg(long)]
        from: String,
        /// Ending native color JSON.
        #[arg(long)]
        to: String,
        /// Interpolation destination JSON with space and optional profile.
        #[arg(long)]
        space: String,
        /// Position in [0,1].
        #[arg(long)]
        position: f64,
        #[arg(long, default_value="relative-colorimetric", value_parser=["perceptual","relative-colorimetric","saturation","absolute-colorimetric"])]
        intent: String,
    },
    /// Configure explicit document color roles or evaluate their target conversions.
    Role {
        #[command(subcommand)]
        action: ColorRoleAction,
    },
    /// Assign or convert stored vector/background/fill/effect paint samples.
    Paints {
        #[command(subcommand)]
        action: ColorPaintAction,
    },
    /// Evaluate native color channels and optionally convert a sample, read only.
    Evaluate {
        input: PathBuf,
        /// Native color JSON with space, channels, optional profile and alpha.
        #[arg(long)]
        color: String,
        /// Destination JSON with space and optional profile.
        #[arg(long)]
        to: Option<String>,
        #[arg(long, default_value="relative-colorimetric", value_parser=["perceptual","relative-colorimetric","saturation","absolute-colorimetric"])]
        intent: String,
    },
    /// Inspect ICC files and manage original profiles in v8 documents.
    Profile {
        #[command(subcommand)]
        action: ColorProfileAction,
    },
    /// Show or edit page production geometry: trim, bleed, slug, safe zone, marks.
    Production {
        #[command(subcommand)]
        action: ProductionAction,
    },
    /// Write deterministic ink plates (C,M,Y,K, then spot IDs) with knockout and
    /// overprint semantics. Read only; refuses unsupported transparency.
    Separate {
        input: PathBuf,
        /// Empty or new directory for plate-NN-*.png and separations.json.
        #[arg(long)]
        out: PathBuf,
        /// CMYK destination JSON; defaults to the document output role.
        #[arg(long)]
        to: Option<String>,
        #[arg(long, value_parser=["perceptual","relative-colorimetric","saturation","absolute-colorimetric"])]
        intent: Option<String>,
        /// Render scale, 0.1 to 8 (default 1 or the recipe's scale).
        #[arg(long)]
        scale: Option<f32>,
        /// Plate PNG bit depth (8 or 16; default 8 or the recipe's plate_bits).
        #[arg(long, value_parser = clap::value_parser!(u8).range(8..=16))]
        bits: Option<u8>,
        /// Print recipe JSON file, or @NAME for a recipe in the document's print_recipes, supplying intent, profile and plate_bits; flags override it.
        #[arg(long)]
        recipe: Option<PathBuf>,
        /// Automatic trap width in document units (spreads lighter ink under
        /// darker neighbors, at most 8 device pixels); overrides the recipe.
        #[arg(long)]
        auto_trap: Option<f64>,
        /// Also write composite-preview.png (plates read through the output
        /// profile to the display role, spot alternates multiplied) and report it.
        #[arg(long)]
        composite: bool,
    },
    /// Build a production package directory: PDF/X draft, plates, preflight
    /// report, verified profiles, source document and digest manifest. Refuses
    /// when preflight finds errors; a failed build leaves no directory.
    Package {
        input: PathBuf,
        /// New package directory.
        #[arg(long)]
        out: PathBuf,
        /// Print recipe JSON file, or @NAME for a recipe in the document's print_recipes, supplying intent, ppi and thresholds.
        #[arg(long)]
        recipe: Option<PathBuf>,
        /// CMYK destination JSON; defaults to the document output role.
        #[arg(long)]
        to: Option<String>,
        #[arg(long, value_parser=["perceptual","relative-colorimetric","saturation","absolute-colorimetric"])]
        intent: Option<String>,
    },
    /// Re-hash every file listed by a package manifest, offline.
    PackageVerify { directory: PathBuf },
    /// Structured print preflight: output profile, geometry, bleed, resolution,
    /// transparency and unsupported-construct issues. Read only.
    Preflight {
        input: PathBuf,
        /// Minimum nominal image resolution in ppi (default 150).
        #[arg(long)]
        min_ppi: Option<f64>,
        /// Minimum bleed on every side in inches (default 0.125).
        #[arg(long)]
        min_bleed_inches: Option<f64>,
        /// Bleed-coverage render scale (0.1-8, default 1); 0 skips the measurement.
        #[arg(long)]
        bleed_scale: Option<f32>,
        /// Maximum total ink (C+M+Y+K) in percent, 100-400; unchecked when absent.
        #[arg(long)]
        max_total_ink: Option<f64>,
        /// Print recipe JSON file, or @NAME for a recipe in the document's print_recipes, supplying thresholds; flags override it.
        #[arg(long)]
        recipe: Option<PathBuf>,
    },
    /// Export the selected page as a flattened CMYK PDF/X-4 draft with an output
    /// intent and TrimBox/BleedBox from the page production geometry. The report
    /// states that independent conformance validation has not been run.
    Pdfx {
        input: PathBuf,
        /// New output PDF path; existing files are never replaced.
        output: PathBuf,
        /// CMYK destination JSON; defaults to the document output role.
        #[arg(long)]
        to: Option<String>,
        #[arg(long, value_parser=["perceptual","relative-colorimetric","saturation","absolute-colorimetric"])]
        intent: Option<String>,
        /// Raster resolution, 36 to 1200 ppi (default 300).
        #[arg(long)]
        ppi: Option<f64>,
        /// Print recipe JSON file, or @NAME for a recipe in the document's print_recipes, supplying intent and resolution; flags override it.
        #[arg(long)]
        recipe: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        /// Creation date YYYY-MM-DDTHH:MM:SSZ (default SOURCE_DATE_EPOCH or the epoch).
        #[arg(long)]
        created: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProductionAction {
    /// Resolve the selected page's boxes, PDF boxes in points and marks.
    Show {
        input: PathBuf,
        /// Also measure painted alpha in each bleed band at this scale (0.1-8).
        #[arg(long)]
        coverage_scale: Option<f32>,
        /// Enforce PDF/X box rules (TrimBox and ArtBox are exclusive).
        #[arg(long)]
        pdfx: bool,
    },
    /// Replace the selected page's production object with the given JSON
    /// (use --merge to patch only the keys you pass; a null value removes a key).
    Set {
        input: PathBuf,
        /// Production object, e.g. '{"trim":{...},"bleed":9}'.
        #[arg(long)]
        geometry: String,
        /// Merge the JSON into the existing production object (RFC 7386
        /// merge-patch) instead of replacing it; keys set to null are removed.
        #[arg(long)]
        merge: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Remove the selected page's production object.
    Clear {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
}

#[derive(Args)]
struct ColorPaintOptions {
    input: PathBuf,
    /// Rendering intent (default: relative-colorimetric, or the selected role's intent).
    #[arg(long, value_parser=["perceptual","relative-colorimetric","saturation","absolute-colorimetric"])]
    intent: Option<String>,
    /// Include shared styles/components; their users on other pages can change.
    #[arg(long)]
    include_shared: bool,
    #[arg(long)]
    all_pages: bool,
    #[arg(long, default_value_t = 0)]
    offset: usize,
    /// Maximum emitted changes; every matching paint is still planned.
    #[arg(long, default_value_t = 100)]
    limit: usize,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
}

#[derive(Subcommand)]
enum ColorPaintAction {
    /// Reinterpret existing channel numbers; channel count must be unchanged.
    Assign {
        #[command(flatten)]
        options: ColorPaintOptions,
        /// Destination JSON with space and optional profile.
        #[arg(long)]
        to: String,
    },
    /// Convert through physical D50 PCS and the selected ICC intent paths.
    Convert {
        #[command(flatten)]
        options: ColorPaintOptions,
        #[arg(
            long,
            required_unless_present_any = ["legacy_srgb", "role"],
            conflicts_with_all = ["legacy_srgb", "role"]
        )]
        to: Option<String>,
        /// Explicitly quantize converted samples to legacy sRGB8 hex paints.
        #[arg(long, conflicts_with_all=["to","role"])]
        legacy_srgb: bool,
        /// Use an effective working/proof/display/output target and its stored intent.
        #[arg(long, conflicts_with_all=["to","legacy_srgb","intent"],value_parser=["working","proof","display","output"])]
        role: Option<String>,
    },
}

#[derive(Subcommand)]
enum ColorRoleAction {
    /// Assign a verified role target without changing stored paint samples.
    Assign {
        input: PathBuf,
        #[arg(value_parser=["working","proof","display","output"])]
        role: String,
        #[arg(long)]
        to: String,
        #[arg(long,default_value="relative-colorimetric",value_parser=["perceptual","relative-colorimetric","saturation","absolute-colorimetric"])]
        intent: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Clear one explicit setting; profiles and stored paints are retained.
    Clear {
        input: PathBuf,
        #[arg(value_parser=["working","proof","display","output"])]
        role: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// List all four roles, including implicit working/display defaults.
    List { input: PathBuf },
    /// Convert a native sample through the selected role and its intent, read only.
    Evaluate {
        input: PathBuf,
        #[arg(value_parser=["working","proof","display","output"])]
        role: String,
        #[arg(long)]
        color: String,
    },
}

#[derive(Subcommand)]
enum ColorProfileAction {
    /// Embed every verified ICC catalog dependency in one transaction.
    Collect {
        input: PathBuf,
        /// Permit restricted/unknown policies for a private copy; packaging still refuses them.
        #[arg(long)]
        local_only: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Remove an unused profile identity and all its aliases, without deleting source files.
    Remove {
        input: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Import original ICC bytes into an explicitly migrated v8 document.
    Import {
        input: PathBuf,
        file: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long,default_value="unknown",value_parser=["allowed","restricted","unknown"])]
        redistribution: String,
        #[arg(long)]
        source: Option<String>,
        /// Store a verified existing document-relative file instead of embedding.
        #[arg(long)]
        external: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// List compact document profile records and aliases.
    List {
        input: PathBuf,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Resolve, hash-verify and inspect a document profile offline.
    Show {
        input: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Embed the verified original bytes of a catalog profile atomically.
    Embed {
        input: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Inspect bounded metadata, paginated tags and executable capabilities.
    Inspect {
        file: PathBuf,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Validate a requested ICC path; this does not certify profile conformance.
    Validate {
        file: PathBuf,
        #[arg(long, default_value = "device-to-pcs", value_parser = ["device-to-pcs", "pcs-to-device", "device-link"])]
        direction: String,
        #[arg(long, default_value = "relative-colorimetric", value_parser = ["perceptual", "relative-colorimetric", "saturation", "absolute-colorimetric"])]
        intent: String,
    },
}

#[derive(Subcommand)]
enum InspectAction {
    /// Read-only CMYK ink analysis of visible paint samples on one page.
    Ink {
        input: PathBuf,
        /// Optional verified output-class CMYK catalog profile; otherwise use the output role.
        #[arg(long)]
        profile: Option<String>,
        /// Intent for --profile (otherwise the assigned output role's intent).
        #[arg(long, requires = "profile", value_parser = ["perceptual", "relative-colorimetric", "saturation", "absolute-colorimetric"])]
        intent: Option<String>,
        /// Maximum total area coverage, C+M+Y+K, in percent.
        #[arg(long)]
        tac_limit: f64,
        /// Text smaller than this point size is checked for multiple plates.
        #[arg(long)]
        small_text_points: f64,
        /// Physical document scale used to convert stored text sizes to points.
        #[arg(long)]
        units_per_inch: f64,
        /// Maximum source Lab chroma for a neutral sample (CIE76).
        #[arg(long, default_value_t = 2.0)]
        neutral_chroma: f64,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Read-only document inspection reports.
    Inspect {
        #[command(subcommand)]
        action: InspectAction,
    },
    /// Professional color management and local ICC profile discovery.
    Color {
        #[command(subcommand)]
        action: ColorAction,
    },
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
    /// Pack, verify, install, list, remove, and run sandboxed wasm extensions.
    Extension {
        #[command(subcommand)]
        action: ExtensionAction,
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
        #[arg(long, conflicts_with_all = ["images", "photo", "scene3d", "scene3d_cache"])]
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
        /// Benchmark photography: ingest, cull, develop, preview and export a
        /// synthetic Bayer shoot.
        #[arg(long, conflicts_with_all = ["images", "composite", "scene3d", "scene3d_cache"])]
        photo: bool,
        /// Benchmark the deterministic static-3D CPU renderer.
        #[arg(long, conflicts_with_all = ["images", "composite", "photo", "scene3d_cache"])]
        scene3d: bool,
        /// Benchmark full-node cold and warm render time through the PNG cache.
        #[arg(long, conflicts_with_all = ["images", "composite", "photo", "scene3d"])]
        scene3d_cache: bool,
        /// Rendered triangle count for --scene3d (1-1,000,000).
        #[arg(long, default_value_t = 100_000)]
        triangles: usize,
        /// Optional square RGBA texture edge for --scene3d; zero disables it.
        #[arg(long, default_value_t = 0)]
        texture_edge: u32,
        /// Photos in the synthetic shoot for --photo (1-500).
        #[arg(long, default_value_t = 24)]
        photos: usize,
        /// Megapixels of each source for --photo (0.01-50).
        #[arg(long, default_value_t = 2.0)]
        megapixels: f64,
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
    /// Version 9 spreads: facing pages, merge, split, arrange.
    Spread {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::layout_cli::SpreadAction,
    },
    /// Static 3D and v11 product forms: import models, generate forms, camera, lights, inspect.
    Scene3d {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::scene3d::cli::Scene3dAction,
    },
    /// Inspect or clear render cache in a running loopback Pentool server.
    Scene3dCache {
        #[arg(long, default_value = "http://127.0.0.1:4711")]
        server: String,
        #[command(subcommand)]
        action: pentool::scene3d::cli::CacheAction,
    },
    /// List procedural product starters or add one as an editable mockup.
    Mockup {
        /// Validate and report without writing the document (apply only).
        #[arg(long, global = true)]
        dry_run: bool,
        /// Reject apply if the document revision no longer matches this value.
        #[arg(long, global = true)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: MockupAction,
    },
    /// Create, inspect, preview, preflight, and export packaging dielines.
    Pack {
        /// Document file; omitted only for the static `templates` listing.
        input: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::scene3d::cli::PackAction,
    },
    /// Version 9 parent pages: create, items, assignment, resolution.
    Parent {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::layout_cli::ParentAction,
    },
    /// Version 9 layout styles: paragraph, character, object, table and cell styles.
    LayoutStyle {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::styles::StyleAction,
    },
    /// Version 9 baseline grids, guides, smart-guide measurement, step-and-repeat and frame grids.
    LayoutGrid {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::grids::GridAction,
    },
    /// Version 9 text wrap and anchored objects.
    LayoutWrap {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::wrap::WrapAction,
    },
    /// Version 9 tables: cells, merging, header rows, CSV/TSV import and planning.
    LayoutTable {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::tables::TableAction,
    },
    /// Import an InDesign Markup (IDML) package as a version 9 document with a
    /// fidelity report. Native .indd/.indb files are refused.
    IdmlImport {
        input: PathBuf,
        output: PathBuf,
        /// Write the full fidelity report here (JSON).
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Export a document as an InDesign Markup (IDML) package. Features with no
    /// exact IDML equivalent fail the export unless --allow-approximation is
    /// given; carried IDML data returns unchanged. OUTPUT must end in .idml.
    IdmlExport {
        input: PathBuf,
        /// IDML package to write (`.idml`; the bytes are always an IDML package).
        output: PathBuf,
        /// Write the full export report here (JSON).
        #[arg(long)]
        report: Option<PathBuf>,
        /// Drop or approximate features IDML cannot express, listing each one.
        #[arg(long)]
        allow_approximation: bool,
        #[arg(long)]
        overwrite: bool,
    },
    /// Version 9 long documents: notes, cross-references, links, variables,
    /// conditional text and generated tables of contents, indexes and captions.
    LayoutDoc {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        /// File name used by the file-name variable (default: the input file name).
        #[arg(long)]
        file_name: Option<String>,
        #[command(subcommand)]
        action: pentool::longdoc::DocAction,
    },
    /// Export composed stories as a tagged PDF with real text, bookmarks and
    /// links, or as EPUB 3 (reflowable, or pre-paginated with `--fixed`).
    LayoutExport {
        input: PathBuf,
        out: PathBuf,
        /// pdf or epub (default: from the output extension).
        #[arg(long)]
        format: Option<String>,
        /// EPUB only: pre-paginated pages instead of reflowable stories.
        #[arg(long)]
        fixed: bool,
        /// Page artwork raster scale (default 2).
        #[arg(long)]
        scale: Option<f32>,
        #[arg(long)]
        title: Option<String>,
        /// BCP-47 document language (default: first story language, else en).
        #[arg(long)]
        language: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
    /// Composer-measured layout preflight of a version 9 document: overset
    /// text, fonts, links, generated stories, geometry and alt text. Read-only.
    LayoutPreflight {
        input: PathBuf,
        /// Allowed text overhang past the page edge, in points.
        #[arg(long, default_value_t = 0.0)]
        bleed: f64,
        /// Percent of paragraphs with local overrides that triggers a warning.
        #[arg(long, default_value_t = 20.0)]
        max_overrides: f64,
        /// Do not report missing alternative text.
        #[arg(long)]
        no_alt: bool,
    },
    /// Merge CSV, TSV or JSON records into `{{field}}` text placeholders and
    /// image frames of a template page, one record per page or several per
    /// page. Writes a new document and reports estimated overflow.
    LayoutMerge {
        /// Template document (never modified).
        input: PathBuf,
        /// CSV, TSV or JSON (array of flat objects) data file.
        #[arg(long)]
        data: PathBuf,
        /// New merged document.
        #[arg(long)]
        out: PathBuf,
        /// csv, tsv or json (default: from the data file extension).
        #[arg(long)]
        format: Option<String>,
        /// Single-character field delimiter for csv/tsv.
        #[arg(long)]
        delimiter: Option<char>,
        /// Template page (default: the first page).
        #[arg(long)]
        template_page: Option<String>,
        #[arg(long, default_value_t = 1)]
        records_per_page: usize,
        /// COLSxROWS cells per page (needs --records-per-page and --cell).
        #[arg(long)]
        grid: Option<String>,
        /// WIDTHxHEIGHT offset between cells.
        #[arg(long)]
        cell: Option<String>,
        /// FRAME=FIELD: fill a graphic frame with the image path in FIELD
        /// (relative to the data file; repeatable).
        #[arg(long = "image-field")]
        image_fields: Vec<String>,
        /// Keep the template page in the output.
        #[arg(long)]
        keep_template: bool,
        /// Plan and report without writing the output.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        overwrite: bool,
    },
    /// Resize one page and re-place its objects: scale, recenter, guide or
    /// object rules.
    LayoutAdapt {
        input: PathBuf,
        #[arg(long)]
        page: String,
        #[arg(long)]
        width: f64,
        #[arg(long)]
        height: f64,
        /// scale, recenter, guide or object.
        #[arg(long, default_value = "scale")]
        rule: String,
        /// With scale: keep the aspect ratio and center the content.
        #[arg(long)]
        uniform: bool,
        /// ID=h:left|right|center|stretch|scale,v:top|bottom|center|stretch|scale (repeatable).
        #[arg(long = "object")]
        objects: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
    },
    /// Alternate layouts (print, tablet, ...) whose stories are linked copies.
    LayoutAlt {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::merge::AltAction,
    },
    /// `.penbook` books: ordered members with continuous numbering, style and
    /// swatch synchronization, book-wide TOC/index, preflight, package, export.
    /// Writes are all-or-none across members.
    Book {
        /// The `.penbook` manifest.
        book: PathBuf,
        #[command(subcommand)]
        action: pentool::book::BookAction,
    },
    /// Version 9 sections: numbering styles, prefixes, start numbers.
    Section {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::layout_cli::SectionAction,
    },
    /// Version 9 parent-item overrides and detach, per page.
    Override {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::layout_cli::OverrideAction,
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
    /// Compose a legacy .pen page or import a static design format into the destination.
    Import {
        destination: PathBuf,
        source: PathBuf,
        /// Explicit design interchange format; normally inferred from the source extension.
        #[arg(long, value_enum)]
        format: Option<DesignFormat>,
        /// Write the full format fidelity report as JSON after a successful commit.
        #[arg(long)]
        report: Option<PathBuf>,
        /// Select a source page when composing legacy .pen documents.
        #[arg(long)]
        source_page: Option<String>,
        #[arg(long)]
        prefix: Option<String>,
        /// Disable ID prefixing for legacy .pen composition (requires collision-free IDs).
        #[arg(long, conflicts_with = "prefix")]
        no_prefix: bool,
        /// Placement options apply to legacy .pen composition; interchange imports retain source coordinates.
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
        /// Render every photo preview instead of using `.pentool/cache/photo/`.
        #[arg(long)]
        no_cache: bool,
        /// Serve the collaboration API (/api/collab/*) with token roles enforced on every route.
        #[arg(long)]
        collab: bool,
        /// Accept a plain-HTTP bind on a non-loopback --host (requires tokens).
        #[arg(long)]
        allow_insecure_lan: bool,
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
    /// Explicitly migrate between compatible v3 through v12 document formats.
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
    /// The clipped node must be a following sibling of its base.
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
    /// Sample colors and manage portable palettes (v0.13.0 agent painting).
    PaintPalette {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::paint_palette::PaletteAction,
    },
    /// Build and preview stroke fields and their stroke plans (v0.13.0 agent painting).
    PaintFields {
        input: PathBuf,
        #[command(subcommand)]
        action: pentool::paint_fields::FieldAction,
    },
    /// List material paint modes and canonicalise a media spec (v0.13.0 agent painting).
    PaintMedia {
        #[command(subcommand)]
        action: pentool::paint_media::MediaAction,
    },
    /// Resolve and preview region painting constraints (v0.13.0 agent painting).
    PaintConstraints {
        input: PathBuf,
        #[command(subcommand)]
        action: pentool::paint_constraints::ConstraintAction,
    },
    /// Score two revisions of a layer against a goal (v0.13.0 agent painting).
    PaintScore(Box<pentool::paint_score::ScoreArgs>),
    /// Reusable composition and form guides (v0.13.0 agent painting).
    PaintGuide {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::paint_guide::GuideAction,
    },
    /// Portable paint bundles and stored plan templates (v0.13.0 agent painting).
    PaintBundle {
        input: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[command(subcommand)]
        action: pentool::paint_bundle::BundleAction,
    },
    /// Structural diff of paint data and layer pixels between two documents or plans.
    PaintDiff(Box<pentool::paint_bundle::DiffArgs>),
    /// Run a declarative, atomic multi-pass paint plan (v0.13.0 agent painting).
    PaintPlan(Box<pentool::paint_plan::PlanArgs>),
    /// Compact, paginated visual analysis of a bounded scope (v0.13.0 agent painting).
    PaintAnalyze(Box<pentool::paint_analysis::AnalyzeArgs>),
    /// Bounded region previews and structural comparison (v0.13.0 agent painting).
    PaintReview {
        input: PathBuf,
        #[command(subcommand)]
        action: pentool::paint_review::ReviewAction,
    },
    /// Maintain verified linked sources and collect portable projects offline.
    Linked(Box<LinkedArgs>),
    /// Save data-only appearance presets or copy/paste named appearances.
    Preset(Box<PresetArgs>),
    /// List, create, edit, and rebind named document design tokens.
    Style(Box<StyleArgs>),
    /// Inspect design tokens and manage versioned, lockable token libraries.
    Design {
        #[command(subcommand)]
        command: pentool::design::cli::DesignCommand,
    },
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
    /// Comment threads, mentions and review states kept beside the document.
    #[command(allow_negative_numbers = true)]
    Comment {
        #[command(subcommand)]
        action: CommentAction,
    },
    /// Show history, or prune it with `history prune FILE`.
    History {
        target: String,
        input: Option<PathBuf>,
        #[arg(long, default_value_t = 50)]
        keep: usize,
        #[arg(long)]
        dry_run: bool,
    },
    /// Collaboration operation log: init, log, replay, verify.
    Collab {
        #[command(subcommand)]
        action: CollabAction,
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
        /// Write the complete bounded fidelity report as JSON; counts are always included in the command result.
        #[arg(long)]
        report: Option<PathBuf>,
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
        /// PSD only: retain each document layer as a raster layer.
        #[arg(long)]
        layers: bool,
        /// 3D renderer: deterministic `reference` or optional `accelerated`.
        #[arg(long, default_value = "reference")]
        renderer: String,
    },
}

#[derive(Subcommand)]
enum MockupAction {
    /// List offline procedural mockup starters as JSON.
    List,
    /// Add a preset as a new editable product form and scene3d node.
    Apply {
        /// Target v11 `.pen` document.
        input: PathBuf,
        /// Offline starter ID from `mockup list`.
        #[arg(long)]
        preset: String,
        /// Stable ID for the new scene3d node; its form ID is `<id>-form`.
        #[arg(long)]
        id: String,
        /// Existing target layer ID.
        #[arg(long)]
        layer: String,
        /// Target page ID (defaults to the first page).
        #[arg(long)]
        page: Option<String>,
        /// Horizontal placement in document units.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        x: f64,
        /// Vertical placement in document units.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        y: f64,
        /// Placed width in document units.
        #[arg(long, default_value_t = 400.0)]
        width: f64,
        /// Placed height in document units.
        #[arg(long, default_value_t = 400.0)]
        height: f64,
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
enum ExtensionAction {
    /// Build a deterministic .penext from a directory holding extension.json.
    Pack { dir: PathBuf, output: PathBuf },
    /// Verify a .penext offline (digests, manifest, module); runs nothing.
    Verify { package: PathBuf },
    /// Verify a .penext and install it. Without --approve only the requested
    /// capabilities are shown and nothing is written.
    Add {
        package: PathBuf,
        /// Grant every capability the manifest declares.
        #[arg(long)]
        approve: bool,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// List installed extensions with capabilities and limits.
    List {
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Remove an installed extension.
    Remove {
        name: String,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Run an installed extension on a document. Edits go through the shared
    /// planner and transaction: --dry-run previews, --if-revision guards.
    Run {
        name: String,
        file: PathBuf,
        /// Command id declared in the manifest.
        #[arg(long)]
        command: Option<String>,
        /// JSON arguments delivered to the extension.
        #[arg(long)]
        args: Option<String>,
        /// Read-only JSON event delivered to the extension.
        #[arg(long)]
        event: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[arg(long)]
        project: Option<PathBuf>,
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

/// Read a camera or lens profile file, checking its size before reading it.
fn read_profile_file(path: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    let length = fs::metadata(path)
        .with_context(|| format!("[missing-resource] {}", path.display()))?
        .len();
    if length > pentool::photo::catalog::MAX_PROFILE_BYTES {
        anyhow::bail!(
            "[limit-exceeded] profile {} is larger than 16 MiB",
            path.display()
        )
    }
    fs::read(path).with_context(|| format!("[missing-resource] {}", path.display()))
}

/// `photo merge-hdr` and `photo merge-pano`: merge, then commit the document
/// and any external DNG together.
/// Apply one photo catalog edit to `input` as a single transaction and print
/// `{change, result}`.
fn run_photo_edit(
    input: &Path,
    operation: &str,
    edit: EditArgs,
    apply: impl FnOnce(&mut serde_json::Value) -> Result<serde_json::Value>,
) -> Result<()> {
    let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(input)?)?;
    let result = apply(&mut raw)?;
    let change = transaction::commit_value(
        input,
        operation,
        edit.dry_run,
        edit.if_revision.as_deref(),
        &raw,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"change":change,"result":result}))?
    );
    Ok(())
}

/// Read `@FILE.json`: a JSON array of strings, at most 16 MiB.
fn id_list_file(path: &str) -> Result<Vec<String>> {
    let size = fs::metadata(path)
        .with_context(|| format!("[missing-resource] cannot read list {path}"))?
        .len();
    if size > 16 * 1024 * 1024 {
        anyhow::bail!("[limit-exceeded] list {path} is larger than 16 MiB")
    }
    serde_json::from_slice(&fs::read(path)?)
        .with_context(|| format!("[invalid-input] list {path} must be a JSON array of strings"))
}

/// A photo selection: `@FILE.json`, a search query or comma-separated IDs.
fn selection_arg(text: &str) -> Result<pentool::photo::organize::Selection> {
    use pentool::photo::organize::Selection;
    Ok(match text.strip_prefix('@') {
        Some(path) => Selection::Ids(id_list_file(path)?),
        None => Selection::parse(text),
    })
}

/// `--to` of `photo settings sync`: comma-separated `photo[/variant]` entries,
/// `@FILE.json` holding an array of them, or a search query (each match's master).
fn sync_targets(to: &str, raw: &serde_json::Value) -> Result<Vec<(String, String)>> {
    use pentool::photo::{organize, variants};
    let entries: Vec<String> = if let Some(path) = to.strip_prefix('@') {
        id_list_file(path)?
    } else if to.contains([':', '<', '>', '=', ' ', '*', '?']) {
        organize::select(raw, to)?
    } else {
        to.split(',')
            .map(|entry| entry.trim().to_string())
            .collect()
    };
    if entries.len() > variants::MAX_SYNC_TARGETS {
        anyhow::bail!(
            "[limit-exceeded] {} targets exceed the limit of {} per sync",
            entries.len(),
            variants::MAX_SYNC_TARGETS
        )
    }
    entries
        .iter()
        .map(|entry| variants::split_target(entry))
        .collect()
}

/// Build and write a contact sheet or compare view, then print its report.
fn run_photo_sheet(
    raw: &serde_json::Value,
    input: &Path,
    items: &[(String, String)],
    columns: u32,
    sheet: SheetArgs,
) -> Result<()> {
    use pentool::photo::organize;
    let (scene, mut report) = organize::sheet(
        raw,
        input,
        items,
        &organize::Sheet {
            columns,
            cell: sheet.cell,
            caption: sheet.caption,
        },
    )?;
    organize::write_sheet(&scene, input, &sheet.out)?;
    report["out"] = serde_json::json!(sheet.out.display().to_string());
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_photo_merge(
    input: PathBuf,
    photos: &[String],
    id: &str,
    name: Option<String>,
    kind: pentool::photo::catalog::MergeKind,
    common: MergeArgs,
    operation: &str,
) -> Result<()> {
    let settings_first = match common.settings.as_str() {
        "import" => false,
        "first" => true,
        other => anyhow::bail!("[invalid-input] --settings {other:?} must be import or first"),
    };
    let external = match &common.external {
        Some(path) => {
            let relative = pentool::resource::safe_relative_path(path)?;
            let target = pentool::resource::document_root(&input).join(&relative);
            if target.exists() {
                anyhow::bail!(
                    "[invalid-input] {} already exists; choose a new --external path",
                    target.display()
                )
            }
            Some(relative)
        }
        None => None,
    };
    let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
    let name = name.unwrap_or_else(|| id.to_owned());
    let (result, resource) = pentool::photo::catalog::merge(
        &mut raw,
        &input,
        pentool::photo::catalog::MergeRequest {
            photo_id: id,
            name: &name,
            inputs: photos,
            kind,
            settings_first,
            external,
        },
    )?;
    let change = transaction::commit_bundle(
        &input,
        operation,
        common.dry_run,
        common.if_revision.as_deref(),
        &raw,
        resource.as_slice(),
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"change":change,"result":result}))?
    );
    Ok(())
}

/// `--token`, or `PENTOOL_COLLAB_TOKEN` so the secret stays out of shell history.
fn collab_token(flag: Option<String>) -> Result<String> {
    flag.or_else(|| std::env::var("PENTOOL_COLLAB_TOKEN").ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "[unauthenticated] pass --token or set PENTOOL_COLLAB_TOKEN to the secret shown by `collab token create`"
            )
        })
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
            return Ok(());
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
    /// Compile an absolute SVG M/L/Q/C/A path into deterministic canonical raster strokes.
    StrokePath {
        id: String,
        /// Absolute SVG path with one subpath and no close command.
        #[arg(long)]
        path: String,
        /// Curve-flattening tolerance in pixels.
        #[arg(long, default_value_t = 0.25)]
        flatness: f64,
        /// JSON object containing size, pressure, flow, opacity, rotation or color curves, or @file.json.
        #[arg(long, default_value = "{}")]
        curves: String,
        #[arg(long, default_value = "{}")]
        brush: String,
        #[arg(long)]
        preset: Option<String>,
        #[arg(long, default_value = "#000000")]
        color: String,
        #[arg(long, default_value = "normal")]
        blend: String,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Region constraints JSON (inside, avoid, opaque_clip, barriers, reach_from,
        /// follow), or @file.json. See docs/agent-painting-protocol-v1.md.
        #[arg(long)]
        constraints: Option<String>,
    },
    /// Paint seeded strokes through a deterministic direction field (hair, grass,
    /// fabric, form-following hatching). The field hash is pinned in the journal.
    StrokeField {
        id: String,
        /// Field JSON: {"method":"contour|gradient|distance|tensor|vanishing|radial|angle|vectors",
        /// "cell":8,"rect":[x,y,w,h],...} or @file.json.
        #[arg(long)]
        field: String,
        /// Stroke rules JSON: spacing, jitter, length, length_jitter, step, curvature,
        /// overlap, taper, collision (stop|skip|allow), min_length; or @file.json.
        #[arg(long, default_value = "{}")]
        params: String,
        #[arg(long, default_value = "{}")]
        brush: String,
        #[arg(long)]
        preset: Option<String>,
        #[arg(long, default_value = "#000000")]
        color: String,
        #[arg(long, default_value = "normal")]
        blend: String,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Region constraints JSON (see stroke-path); `follow` is not allowed here.
        #[arg(long)]
        constraints: Option<String>,
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
    pentool::collab::set_actor_flags(pentool::collab::ActorSpec {
        kind: cli.actor_kind.clone(),
        id: cli.actor_id.clone(),
        name: cli.actor_name.clone(),
        directed_by: cli.directed_by.clone(),
    });
    let selected_page = cli.page.as_deref();
    match cli.command.unwrap_or(Command::Serve {
        input: None,
        host: "127.0.0.1".into(),
        port: 4711,
        no_cache: false,
        collab: false,
        allow_insecure_lan: false,
    }) {
        Command::Inspect {
            action:
                InspectAction::Ink {
                    input,
                    profile,
                    intent,
                    tac_limit,
                    small_text_points,
                    units_per_inch,
                    neutral_chroma,
                    offset,
                    limit,
                },
        } => {
            use pentool::color::{ink, roles};
            if !(1..=256).contains(&limit) {
                bail!("[invalid-input] ink report limit must be in 1..=256");
            }
            if !tac_limit.is_finite() || !(0.0..=400.0).contains(&tac_limit) {
                bail!("[invalid-input] ink TAC limit must be finite and in [0,400] percent");
            }
            if !small_text_points.is_finite()
                || small_text_points <= 0.0
                || small_text_points > 1000.0
            {
                bail!("[invalid-input] small-text threshold must be finite and in (0,1000] points");
            }
            if !units_per_inch.is_finite() || units_per_inch <= 0.0 || units_per_inch > 1_000_000.0
            {
                bail!("[invalid-input] document units per inch must be finite and in (0,1000000]");
            }
            if !neutral_chroma.is_finite() || !(0.0..=150.0).contains(&neutral_chroma) {
                bail!(
                    "[invalid-input] neutral CIE76 chroma threshold must be finite and in [0,150]"
                );
            }
            let (mut raw, _) = read_color_document(&input, None)?;
            if let Some(profile) = profile {
                let intent = intent
                    .as_deref()
                    .unwrap_or("relative-colorimetric")
                    .parse()?;
                roles::assign(
                    &mut raw,
                    &input,
                    roles::Role::Output,
                    &serde_json::json!({"space":"icc-cmyk","profile":profile}),
                    intent,
                )?;
            }
            let report = ink::analyze_page(
                &raw,
                &input,
                selected_page,
                ink::Thresholds {
                    tac_percent: tac_limit,
                    small_text_points,
                    units_per_inch,
                    neutral_chroma,
                },
                offset,
                limit,
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
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
                    anyhow::bail!(
                        "[limit-exceeded] raw source is larger than the 128 MiB embedding limit; pass --external with a document-relative path"
                    )
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
                lens_profile,
                set,
                unset,
                upright,
                guide,
                exposure,
                auto_tone,
                dry_run,
                if_revision,
            } => {
                use pentool::photo::catalog::{DevelopChanges, WhiteBalance};
                if exposure.is_some() && auto_tone {
                    anyhow::bail!(
                        "[invalid-input] --exposure and --auto-tone both set tone.exposure; choose one"
                    )
                }
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
                let lens_profile = lens_profile
                    .as_deref()
                    .map(pentool::photo::catalog::lens_profile)
                    .transpose()?;
                let mut set = set
                    .iter()
                    .map(|entry| {
                        let (key, value) = entry.split_once('=').with_context(|| {
                            format!("[invalid-input] --set {entry:?} must be KEY=JSON")
                        })?;
                        let value: serde_json::Value =
                            serde_json::from_str(value).with_context(|| {
                                format!("[invalid-input] --set {key}: {value:?} is not JSON")
                            })?;
                        Ok((key.to_string(), value))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                if let Some(exposure) = exposure {
                    if !exposure.is_finite() {
                        anyhow::bail!("[invalid-input] --exposure takes a number of EV")
                    }
                    set.push((
                        "tone.exposure".into(),
                        pentool::photo::dng::number(exposure),
                    ));
                }
                if guide.len() > 4 {
                    anyhow::bail!("[invalid-input] at most 4 --guide segments")
                }
                let guides = guide
                    .iter()
                    .map(|text| {
                        let values = text
                            .split(',')
                            .map(|v| v.trim().parse::<f64>())
                            .collect::<Result<Vec<_>, _>>()
                            .ok()
                            .filter(|v| v.len() == 4 && v.iter().all(|v| (0.0..=1.0).contains(v)));
                        match values {
                            Some(v) => Ok([v[0], v[1], v[2], v[3]]),
                            None => anyhow::bail!(
                                "[invalid-input] --guide {text:?} takes x1,y1,x2,y2 between 0 and 1"
                            ),
                        }
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let changes = DevelopChanges {
                    set,
                    unset,
                    camera_profile,
                    lens_profile,
                    white_balance,
                    upright,
                    guides,
                    auto_tone,
                };
                if changes.is_empty() {
                    anyhow::bail!(
                        "[invalid-input] raw develop needs a setting: --set, --unset, --exposure, --auto-tone, --camera-profile, --lens-profile, --upright, --as-shot, --temperature, --neutral, --sample or --suggest"
                    )
                }
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result =
                    pentool::photo::catalog::develop_raw(&mut raw, &input, &id, &variant, changes)?;
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
        Command::Color {
            action: ColorAction::Paints { action },
        } => {
            use pentool::color::{
                editing::{Operation, Options, Target},
                value::Destination,
            };
            let (options, operation, to, role) = match action {
                ColorPaintAction::Assign { options, to } => {
                    (options, Operation::Assign, Some(to), None)
                }
                ColorPaintAction::Convert {
                    options, to, role, ..
                } => (options, Operation::Convert, to, role),
            };
            if !(1..=256).contains(&options.limit) {
                bail!("[invalid-input] paint change limit must be 1..=256");
            }
            let mut intent = options
                .intent
                .as_deref()
                .unwrap_or("relative-colorimetric")
                .parse()?;
            let mut target = if let Some(to) = to {
                if to.len() > 256 {
                    bail!("[limit-exceeded] paint destination JSON exceeds 256 bytes");
                }
                Target::Native(Destination::parse(
                    &serde_json::from_str(&to).context("invalid paint destination JSON")?,
                )?)
            } else {
                Target::LegacySrgb
            };
            let (raw, revision) =
                read_color_document(&options.input, options.if_revision.as_deref())?;
            if let Some(role) = role.as_deref() {
                let setting = pentool::color::roles::effective(&raw, role.parse()?)?;
                target = Target::Native(setting.destination);
                intent = setting.intent;
            }
            let mut plan = pentool::color::editing::plan(
                &raw,
                &options.input,
                &Options {
                    operation,
                    target,
                    intent,
                    page: selected_page,
                    all_pages: options.all_pages,
                    include_shared: options.include_shared,
                    offset: options.offset,
                    limit: options.limit,
                },
            )?;
            if let Some(role) = role {
                plan.report["role"] = serde_json::json!(role);
            }
            let change = transaction::commit_value(
                &options.input,
                match operation {
                    Operation::Assign => "color-paints-assign",
                    Operation::Convert => "color-paints-convert",
                },
                options.dry_run,
                Some(&revision),
                &plan.document,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"change":change,"result":plan.report})
                )?
            );
            Ok(())
        }
        Command::Color {
            action: ColorAction::Role { action },
        } => {
            use pentool::color::roles;
            if selected_page.is_some() {
                bail!(
                    "[invalid-input] document color roles have no page scope; --page is not applicable"
                );
            }
            let result = match action {
                ColorRoleAction::Assign {
                    input,
                    role,
                    to,
                    intent,
                    dry_run,
                    if_revision,
                } => {
                    if to.len() > 256 {
                        bail!("[limit-exceeded] role destination JSON exceeds 256 bytes");
                    }
                    let destination: serde_json::Value =
                        serde_json::from_str(&to).context("invalid role destination JSON")?;
                    let (mut raw, revision) = read_color_document(&input, if_revision.as_deref())?;
                    let result = roles::assign(
                        &mut raw,
                        &input,
                        role.parse()?,
                        &destination,
                        intent.parse()?,
                    )?;
                    let change = transaction::commit_value(
                        &input,
                        "color-role-assign",
                        dry_run,
                        Some(&revision),
                        &raw,
                    )?;
                    serde_json::json!({"change":change,"result":result})
                }
                ColorRoleAction::Clear {
                    input,
                    role,
                    dry_run,
                    if_revision,
                } => {
                    let (mut raw, revision) = read_color_document(&input, if_revision.as_deref())?;
                    let result = roles::clear(&mut raw, role.parse()?)?;
                    let change = transaction::commit_value(
                        &input,
                        "color-role-clear",
                        dry_run,
                        Some(&revision),
                        &raw,
                    )?;
                    serde_json::json!({"change":change,"result":result})
                }
                ColorRoleAction::List { input } => {
                    let (raw, _) = read_color_document(&input, None)?;
                    roles::list(&raw)?
                }
                ColorRoleAction::Evaluate { input, role, color } => {
                    if color.len() > 4096 {
                        bail!("[limit-exceeded] native color JSON exceeds 4096 bytes");
                    }
                    let color: serde_json::Value =
                        serde_json::from_str(&color).context("invalid native color JSON")?;
                    let (raw, _) = read_color_document(&input, None)?;
                    roles::evaluate(&raw, &input, role.parse()?, &color)?
                }
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Command::Color {
            action:
                ColorAction::Interpolate {
                    input,
                    from,
                    to,
                    space,
                    position,
                    intent,
                },
        } => {
            if selected_page.is_some() {
                bail!(
                    "[invalid-input] color interpolation has no page scope; --page is not applicable"
                );
            }
            if from.len() > 4096 || to.len() > 4096 || space.len() > 256 {
                bail!(
                    "[limit-exceeded] color JSON is limited to 4096 bytes and interpolation destination JSON to 256 bytes"
                );
            }
            if !position.is_finite() || !(0.0..=1.0).contains(&position) {
                bail!("[invalid-input] color interpolation position must be finite and in [0,1]");
            }
            let from: serde_json::Value =
                serde_json::from_str(&from).context("invalid starting native color JSON")?;
            let to: serde_json::Value =
                serde_json::from_str(&to).context("invalid ending native color JSON")?;
            let space: serde_json::Value =
                serde_json::from_str(&space).context("invalid interpolation destination JSON")?;
            let (raw, _) = read_color_document(&input, None)?;
            let result = pentool::color::value::interpolate(
                &raw,
                &input,
                &from,
                &to,
                &space,
                position,
                intent.parse()?,
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Command::Color {
            action:
                ColorAction::Evaluate {
                    input,
                    color,
                    to,
                    intent,
                },
        } => {
            if selected_page.is_some() {
                bail!(
                    "[invalid-input] color sample evaluation has no page scope; --page is not applicable"
                );
            }
            if color.len() > 4096 || to.as_ref().is_some_and(|value| value.len() > 256) {
                bail!(
                    "[limit-exceeded] color JSON is limited to 4096 bytes and destination JSON to 256 bytes"
                );
            }
            let color: serde_json::Value =
                serde_json::from_str(&color).context("invalid native color JSON")?;
            let destination: Option<serde_json::Value> = to
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .context("invalid color destination JSON")?;
            let raw: serde_json::Value = serde_json::from_slice(&pentool::resource::read_bounded(
                &input,
                pentool::color::catalog::MAX_CATALOG_DOCUMENT_BYTES,
            )?)?;
            let result = pentool::color::value::evaluate(
                &raw,
                &input,
                &color,
                destination.as_ref(),
                intent.parse()?,
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Command::Color {
            action: ColorAction::Production { action },
        } => {
            use pentool::color::production;
            let page_index = |raw: &serde_json::Value| -> Result<usize> {
                let pages = raw["pages"].as_array().context("pages missing")?;
                match selected_page {
                    Some(id) => pages
                        .iter()
                        .position(|p| p["id"] == id)
                        .with_context(|| format!("[missing-resource] page not found: {id}")),
                    None => Ok(0),
                }
            };
            match action {
                ProductionAction::Show {
                    input,
                    coverage_scale,
                    pdfx,
                } => {
                    let (raw, _) = read_color_document(&input, None)?;
                    pentool::scene::validate(&raw)?;
                    let index = page_index(&raw)?;
                    let page = &raw["pages"][index];
                    let geometry = production::resolve(page, pdfx)?.with_context(|| {
                        format!(
                            "[missing-resource] page {} has no production geometry; use `color production set`",
                            page["id"]
                        )
                    })?;
                    let mut out = serde_json::json!({"page":page["id"],"geometry":geometry.json()});
                    if let Some(scale) = coverage_scale {
                        if !(0.1..=8.0).contains(&scale) {
                            bail!("[invalid-input] coverage scale must be in 0.1..8");
                        }
                        out["bleed_coverage"] =
                            production::bleed_coverage(&raw, &input, page["id"].as_str(), scale)?;
                    }
                    println!("{}", serde_json::to_string_pretty(&out)?);
                    Ok(())
                }
                ProductionAction::Set {
                    input,
                    geometry: json,
                    merge,
                    dry_run,
                    if_revision,
                } => {
                    if json.len() > 4096 {
                        bail!("[limit-exceeded] production JSON is limited to 4096 bytes");
                    }
                    let (mut raw, revision) = read_color_document(&input, if_revision.as_deref())?;
                    ensure_v8_production(&raw)?;
                    let index = page_index(&raw)?;
                    let patch: serde_json::Value =
                        serde_json::from_str(&json).context("invalid production JSON")?;
                    let previous = raw["pages"][index]
                        .get("production")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let next = if merge {
                        if !patch.is_object() {
                            bail!(
                                "[invalid-input] --merge needs a JSON object, e.g. '{{\"bleed\":9}}'"
                            )
                        }
                        let mut base = if previous.is_object() {
                            previous.clone()
                        } else {
                            serde_json::json!({})
                        };
                        merge_patch(&mut base, &patch);
                        base
                    } else {
                        patch
                    };
                    let dropped: Vec<String> = previous
                        .as_object()
                        .map(|old| {
                            old.keys()
                                .filter(|key| next.get(key.as_str()).is_none())
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    raw["pages"][index]["production"] = next;
                    let geometry = production::resolve(&raw["pages"][index], false)?;
                    let change = transaction::commit_value(
                        &input,
                        "production-set",
                        dry_run,
                        Some(&revision),
                        &raw,
                    )?;
                    let mut result = serde_json::json!({
                        "change": change,
                        "geometry": geometry.map(|g| g.json()),
                        "mode": if merge { "merge" } else { "replace" },
                        "dropped_keys": dropped,
                    });
                    if !merge && !result["dropped_keys"].as_array().is_none_or(Vec::is_empty) {
                        result["note"] = serde_json::json!(
                            "the production object was replaced and the listed keys were removed; pass --merge to patch only the keys you give"
                        );
                    }
                    println!("{}", serde_json::to_string_pretty(&result)?);
                    Ok(())
                }
                ProductionAction::Clear {
                    input,
                    dry_run,
                    if_revision,
                } => {
                    let (mut raw, revision) = read_color_document(&input, if_revision.as_deref())?;
                    ensure_v8_production(&raw)?;
                    let index = page_index(&raw)?;
                    let removed = raw["pages"][index]
                        .as_object_mut()
                        .and_then(|page| page.remove("production"))
                        .is_some();
                    if !removed {
                        bail!("[missing-resource] the selected page has no production geometry");
                    }
                    let change = transaction::commit_value(
                        &input,
                        "production-clear",
                        dry_run,
                        Some(&revision),
                        &raw,
                    )?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({"change":change}))?
                    );
                    Ok(())
                }
            }
        }
        Command::Color {
            action:
                ColorAction::Separate {
                    input,
                    out,
                    to,
                    intent,
                    scale,
                    bits,
                    recipe,
                    auto_trap,
                    composite,
                },
        } => {
            use pentool::color::{
                icc::Intent, recipe as print_recipe, separation, value::Destination,
            };
            if to.as_ref().is_some_and(|value| value.len() > 256) {
                bail!("[limit-exceeded] destination JSON is limited to 256 bytes");
            }
            let (mut raw, _) = read_color_document(&input, None)?;
            let recipe = recipe
                .as_deref()
                .map(|spec| print_recipe::resolve(spec, &raw))
                .transpose()?;
            if let Some(recipe) = &recipe {
                recipe.substitute_spots(&mut raw)?;
            }
            let bits = bits
                .or_else(|| recipe.as_ref().and_then(|r| r.plate_bits))
                .unwrap_or(8);
            let destination = to
                .as_deref()
                .map(|value| -> Result<Destination> {
                    Destination::parse(
                        &serde_json::from_str(value).context("invalid color destination JSON")?,
                    )
                })
                .transpose()?;
            let destination = destination.or_else(|| recipe.as_ref().and_then(|r| r.destination()));
            let intent = intent
                .as_deref()
                .map(str::parse::<Intent>)
                .transpose()?
                .or_else(|| recipe.as_ref().and_then(|r| r.intent));
            if destination.is_some() && intent.is_none() {
                bail!("[invalid-input] an explicit destination needs --intent");
            }
            let result = separation::separate(
                &raw,
                &input,
                selected_page,
                &separation::Options {
                    destination,
                    intent,
                    scale: scale
                        .or_else(|| recipe.as_ref().and_then(|r| r.scale))
                        .unwrap_or(1.0),
                    auto_trap: auto_trap.or_else(|| recipe.as_ref().and_then(|r| r.auto_trap)),
                },
            )?;
            let preview = if composite {
                Some(result.composite_preview(&raw, &input)?)
            } else {
                None
            };
            let mut report = result.write_with_depth(&out, bits)?;
            if let Some((png, preview_report)) = preview {
                let path = out.join("composite-preview.png");
                use std::io::Write as _;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .and_then(|mut file| file.write_all(&png))
                    .with_context(|| format!("writing {}", path.display()))?;
                report["composite_preview"] = serde_json::json!({
                    "file":"composite-preview.png","report":preview_report});
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Command::Color {
            action: ColorAction::PackageVerify { directory },
        } => {
            let report = pentool::color::bundle::verify(&directory)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report["ok"] != serde_json::json!(true) {
                bail!("[invalid-input] package verification failed");
            }
            Ok(())
        }
        Command::Color {
            action:
                ColorAction::Package {
                    input,
                    out,
                    recipe,
                    to,
                    intent,
                },
        } => {
            use pentool::color::{
                bundle, icc::Intent, pdfx, preflight, recipe as print_recipe, value::Destination,
            };
            if to.as_ref().is_some_and(|value| value.len() > 256) {
                bail!("[limit-exceeded] destination JSON is limited to 256 bytes");
            }
            let source = pentool::resource::read_bounded(
                &input,
                pentool::color::catalog::MAX_CATALOG_DOCUMENT_BYTES,
            )?;
            let (mut raw, _) = read_color_document(&input, None)?;
            let recipe = recipe
                .as_deref()
                .map(|spec| print_recipe::resolve(spec, &raw))
                .transpose()?;
            if let Some(recipe) = &recipe {
                recipe.substitute_spots(&mut raw)?;
            }
            let destination = to
                .as_deref()
                .map(|value| -> Result<Destination> {
                    Destination::parse(
                        &serde_json::from_str(value).context("invalid color destination JSON")?,
                    )
                })
                .transpose()?;
            let destination = destination.or_else(|| recipe.as_ref().and_then(|r| r.destination()));
            let intent = intent
                .as_deref()
                .map(str::parse::<Intent>)
                .transpose()?
                .or_else(|| recipe.as_ref().and_then(|r| r.intent));
            if destination.is_some() && intent.is_none() {
                bail!("[invalid-input] an explicit destination needs --intent");
            }
            let defaults = preflight::Options::default();
            let manifest = bundle::build(
                &raw,
                &input,
                &source,
                selected_page,
                &out,
                &bundle::Options {
                    pdfx: pdfx::Options {
                        compress: recipe.as_ref().and_then(|r| r.compress).unwrap_or(true),
                        destination,
                        intent,
                        ppi: recipe
                            .as_ref()
                            .and_then(|r| r.ppi)
                            .unwrap_or(pdfx::DEFAULT_PPI),
                        ..Default::default()
                    },
                    preflight: preflight::Options {
                        min_ppi: recipe
                            .as_ref()
                            .and_then(|r| r.min_ppi)
                            .unwrap_or(defaults.min_ppi),
                        min_bleed_inches: recipe
                            .as_ref()
                            .and_then(|r| r.min_bleed_inches)
                            .unwrap_or(defaults.min_bleed_inches),
                        bleed_scale: Some(
                            recipe
                                .as_ref()
                                .and_then(|r| r.bleed_scale)
                                .filter(|s| *s > 0.0)
                                .unwrap_or(1.0),
                        ),
                        max_total_ink: recipe.as_ref().and_then(|r| r.max_total_ink),
                        allowed_spots: recipe.as_ref().and_then(|r| r.allowed_spots.clone()),
                        k_only_text: recipe.as_ref().is_some_and(|r| r.k_only_text),
                        require_marks: recipe.as_ref().is_some_and(|r| r.require_marks),
                        reject_spot_flattening: recipe
                            .as_ref()
                            .is_some_and(|r| r.reject_spot_flattening),
                    },
                    separation_scale: recipe.as_ref().and_then(|r| r.scale).unwrap_or(1.0),
                    auto_trap: recipe.as_ref().and_then(|r| r.auto_trap),
                    plate_bits: recipe.as_ref().and_then(|r| r.plate_bits).unwrap_or(8),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&manifest)?);
            Ok(())
        }
        Command::Color {
            action:
                ColorAction::Preflight {
                    input,
                    min_ppi,
                    min_bleed_inches,
                    bleed_scale,
                    max_total_ink,
                    recipe,
                },
        } => {
            use pentool::color::{preflight, recipe as print_recipe};
            let (mut raw, _) = read_color_document(&input, None)?;
            let recipe = recipe
                .as_deref()
                .map(|spec| print_recipe::resolve(spec, &raw))
                .transpose()?;
            if let Some(recipe) = &recipe {
                recipe.substitute_spots(&mut raw)?;
            }
            let defaults = preflight::Options::default();
            let bleed_scale = bleed_scale
                .or_else(|| recipe.as_ref().and_then(|r| r.bleed_scale))
                .unwrap_or(1.0);
            let report = preflight::run(
                &raw,
                &input,
                selected_page,
                &preflight::Options {
                    min_ppi: min_ppi
                        .or_else(|| recipe.as_ref().and_then(|r| r.min_ppi))
                        .unwrap_or(defaults.min_ppi),
                    min_bleed_inches: min_bleed_inches
                        .or_else(|| recipe.as_ref().and_then(|r| r.min_bleed_inches))
                        .unwrap_or(defaults.min_bleed_inches),
                    bleed_scale: (bleed_scale > 0.0).then_some(bleed_scale),
                    max_total_ink: max_total_ink
                        .or_else(|| recipe.as_ref().and_then(|r| r.max_total_ink)),
                    allowed_spots: recipe.as_ref().and_then(|r| r.allowed_spots.clone()),
                    k_only_text: recipe.as_ref().is_some_and(|r| r.k_only_text),
                    require_marks: recipe.as_ref().is_some_and(|r| r.require_marks),
                    reject_spot_flattening: recipe
                        .as_ref()
                        .is_some_and(|r| r.reject_spot_flattening),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Command::Color {
            action:
                ColorAction::Pdfx {
                    input,
                    output,
                    to,
                    intent,
                    ppi,
                    recipe,
                    title,
                    created,
                },
        } => {
            use pentool::color::{icc::Intent, pdfx, recipe as print_recipe, value::Destination};
            if to.as_ref().is_some_and(|value| value.len() > 256) {
                bail!("[limit-exceeded] destination JSON is limited to 256 bytes");
            }
            let (mut raw, _) = read_color_document(&input, None)?;
            let destination = to
                .as_deref()
                .map(|value| -> Result<Destination> {
                    Destination::parse(
                        &serde_json::from_str(value).context("invalid color destination JSON")?,
                    )
                })
                .transpose()?;
            let recipe = recipe
                .as_deref()
                .map(|spec| print_recipe::resolve(spec, &raw))
                .transpose()?;
            if let Some(recipe) = &recipe {
                recipe.substitute_spots(&mut raw)?;
            }
            let destination = destination.or_else(|| recipe.as_ref().and_then(|r| r.destination()));
            let intent = intent
                .as_deref()
                .map(str::parse::<Intent>)
                .transpose()?
                .or_else(|| recipe.as_ref().and_then(|r| r.intent));
            if destination.is_some() && intent.is_none() {
                bail!("[invalid-input] an explicit destination needs --intent");
            }
            let ppi = ppi
                .or_else(|| recipe.as_ref().and_then(|r| r.ppi))
                .unwrap_or(pdfx::DEFAULT_PPI);
            let report = pdfx::export(
                &raw,
                &input,
                selected_page,
                &output,
                &pdfx::Options {
                    compress: recipe.as_ref().and_then(|r| r.compress).unwrap_or(true),
                    destination,
                    intent,
                    ppi,
                    title,
                    created,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Command::Color {
            action: ColorAction::Profile { action },
        } => {
            use pentool::color::{
                catalog,
                icc::{Direction, Intent},
                profile,
            };
            if selected_page.is_some() {
                bail!(
                    "[invalid-input] profile file/catalog operations have no page scope; --page is not applicable"
                );
            }
            let read = |input: &Path| -> Result<serde_json::Value> {
                serde_json::from_slice(&pentool::resource::read_bounded(
                    input,
                    catalog::MAX_CATALOG_DOCUMENT_BYTES,
                )?)
                .context("invalid .pen document")
            };
            let result = match action {
                ColorProfileAction::Collect {
                    input,
                    local_only,
                    dry_run,
                    if_revision,
                } => {
                    let (mut raw, revision) = read_color_document(&input, if_revision.as_deref())?;
                    let result = catalog::collect(&mut raw, &input, local_only)?;
                    let change = transaction::commit_value(
                        &input,
                        "color-profile-collect",
                        dry_run,
                        Some(&revision),
                        &raw,
                    )?;
                    serde_json::json!({"change":change,"result":result})
                }
                ColorProfileAction::Remove {
                    input,
                    name,
                    dry_run,
                    if_revision,
                } => {
                    let (mut raw, revision) = read_color_document(&input, if_revision.as_deref())?;
                    let result = catalog::remove(&mut raw, &name)?;
                    let change = transaction::commit_value(
                        &input,
                        "color-profile-remove",
                        dry_run,
                        Some(&revision),
                        &raw,
                    )?;
                    serde_json::json!({"change":change,"result":result})
                }
                ColorProfileAction::Import {
                    input,
                    file,
                    name,
                    redistribution,
                    source,
                    external,
                    dry_run,
                    if_revision,
                } => {
                    let (mut raw, revision) = read_color_document(&input, if_revision.as_deref())?;
                    let result = catalog::import(
                        &mut raw,
                        &input,
                        &profile::read_file(&file)?,
                        &name,
                        &redistribution,
                        source.as_deref(),
                        external.as_deref(),
                    )?;
                    let change = transaction::commit_value(
                        &input,
                        "color-profile-import",
                        dry_run,
                        Some(&revision),
                        &raw,
                    )?;
                    serde_json::json!({"change":change,"result":result})
                }
                ColorProfileAction::List {
                    input,
                    offset,
                    limit,
                } => {
                    if !(1..=256).contains(&limit) {
                        bail!("[invalid-input] profile limit must be 1..=256");
                    }
                    catalog::list(&read(&input)?, offset, limit)?
                }
                ColorProfileAction::Show {
                    input,
                    name,
                    offset,
                    limit,
                } => {
                    if !(1..=profile::MAX_TAG_LIMIT).contains(&limit) {
                        bail!(
                            "[invalid-input] ICC tag limit must be 1..={}",
                            profile::MAX_TAG_LIMIT
                        );
                    }
                    let (identity, bytes) = catalog::load(&read(&input)?, &input, &name)?;
                    serde_json::json!({"profile":identity,"inspection":profile::inspect(&bytes,offset,limit)?})
                }
                ColorProfileAction::Embed {
                    input,
                    name,
                    dry_run,
                    if_revision,
                } => {
                    let (mut raw, revision) = read_color_document(&input, if_revision.as_deref())?;
                    let result = catalog::embed(&mut raw, &input, &name)?;
                    let change = transaction::commit_value(
                        &input,
                        "color-profile-embed",
                        dry_run,
                        Some(&revision),
                        &raw,
                    )?;
                    serde_json::json!({"change":change,"result":result})
                }
                ColorProfileAction::Inspect {
                    file,
                    offset,
                    limit,
                } => {
                    // Reject invalid output limits before opening or parsing input.
                    if !(1..=profile::MAX_TAG_LIMIT).contains(&limit) {
                        bail!(
                            "[invalid-input] ICC tag limit must be 1..={}",
                            profile::MAX_TAG_LIMIT
                        );
                    }
                    serde_json::to_value(profile::inspect(
                        &profile::read_file(&file)?,
                        offset,
                        limit,
                    )?)?
                }
                ColorProfileAction::Validate {
                    file,
                    direction,
                    intent,
                } => {
                    let direction = match direction.as_str() {
                        "device-to-pcs" => Some(Direction::DeviceToPcs),
                        "pcs-to-device" => Some(Direction::PcsToDevice),
                        "device-link" => None,
                        _ => unreachable!("clap validates direction"),
                    };
                    let intent = intent.parse::<Intent>()?;
                    profile::validate(&profile::read_file(&file)?, direction, intent)?
                }
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Command::Photo { action } => match action {
            PhotoAction::Profile {
                action:
                    PhotoProfileAction::Add {
                        input,
                        file,
                        lens,
                        force_model,
                        dry_run,
                        if_revision,
                    },
            } => {
                let path = lens.as_ref().or(file.as_ref()).context(
                    "[invalid-input] photo profile add needs --file FILE.dcp or --lens FILE.json",
                )?;
                let bytes = read_profile_file(path)?;
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result = if lens.is_some() {
                    pentool::photo::catalog::add_lens_profile(&mut raw, &bytes)?
                } else {
                    pentool::photo::catalog::add_profile(&mut raw, &bytes, force_model)?
                };
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
            PhotoAction::Profile {
                action:
                    PhotoProfileAction::ImportLcp {
                        input,
                        file,
                        dry_run,
                        if_revision,
                    },
            } => {
                let bytes = read_profile_file(&file)?;
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result = pentool::photo::catalog::import_lcp(&mut raw, &bytes)?;
                let change = transaction::commit_value(
                    &input,
                    "photo-profile-import-lcp",
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
            PhotoAction::Render {
                input,
                id,
                variant,
                out,
                output,
                metadata,
            } => {
                use pentool::photo::{metadata as meta, output};
                let resolved = output.resolve()?;
                let policy = metadata.policy()?;
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let developed = pentool::photo::catalog::render_photo(&raw, &input, &id, &variant)?;
                let embedded = meta::prepare(&raw, &input, &id, &policy)?;
                let encoded = output::encode_png(
                    &developed.image,
                    &developed.rendering,
                    true,
                    &resolved,
                    output::DEFAULT_BINS,
                )?;
                let bytes = meta::embed_png(&encoded.bytes, &embedded)?;
                let temporary = out.with_extension("png.tmp");
                fs::write(&temporary, &bytes)
                    .with_context(|| format!("write {}", temporary.display()))?;
                fs::rename(&temporary, &out).with_context(|| format!("write {}", out.display()))?;
                let mut report = developed.report;
                report["photo"] = serde_json::json!(id);
                report["variant"] = serde_json::json!(variant);
                report["out"] = serde_json::json!(out.display().to_string());
                report["space"] = serde_json::json!(resolved.space.id);
                report["depth"] = serde_json::json!(resolved.depth.bits());
                report["clipped_pixels"] = encoded.report["clipped_pixels"].clone();
                report["bytes"] = serde_json::json!(bytes.len());
                report["delivery"] = encoded.report;
                report["metadata"] = embedded.report(&policy);
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            PhotoAction::Inspect {
                input,
                id,
                variant,
                output,
                bins,
            } => {
                use pentool::photo::output;
                let resolved = output.resolve()?;
                if !(1..=output::MAX_BINS).contains(&bins) {
                    anyhow::bail!(
                        "[invalid-input] --bins {bins} must be 1–{}",
                        output::MAX_BINS
                    )
                }
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let developed = pentool::photo::catalog::render_photo(&raw, &input, &id, &variant)?;
                let measured = output::inspect(
                    &developed.image,
                    &developed.rendering,
                    true,
                    &resolved,
                    bins,
                )?;
                let report = serde_json::json!({
                    "photo": id,
                    "variant": variant,
                    "width": developed.image.width,
                    "height": developed.image.height,
                    "delivery": measured,
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            PhotoAction::Mask {
                action:
                    PhotoMaskAction::Paint {
                        input,
                        id,
                        variant,
                        adjustment,
                        component,
                        samples,
                        brush,
                        preset,
                        erase,
                        seed,
                        dry_run,
                        if_revision,
                    },
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
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let normalized = raster::normalize_input(&read(&samples)?)?;
                let input_summary = normalized.summary();
                let request = raster::StrokeRequest {
                    brush: raster::Brush::parse(&raster::resolve_preset(
                        &raw,
                        &read(&brush)?,
                        preset.as_deref(),
                    )?)?,
                    samples: normalized.samples,
                    color: [0, 0, 0],
                    blend: if erase {
                        raster::Blend::Erase
                    } else {
                        raster::Blend::Normal
                    },
                    seed,
                    clone: None,
                };
                let mut result = pentool::photo::catalog::paint_mask(
                    &mut raw,
                    &input,
                    &id,
                    &variant,
                    pentool::photo::catalog::MaskStroke {
                        adjustment,
                        component,
                        request,
                    },
                )?;
                result["input"] = input_summary;
                let change = transaction::commit_value(
                    &input,
                    "photo-mask-paint",
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
            PhotoAction::Info { input, id, variant } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let report = pentool::photo::catalog::photo_info(&raw, &input, &id, &variant)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            PhotoAction::Place {
                input,
                id,
                variant,
                layer,
                node_id,
                x,
                y,
                width,
                height,
                fit,
                position,
                dry_run,
                if_revision,
            } => {
                let position: [f64; 2] = position.try_into().map_err(|_| {
                    anyhow::anyhow!("[invalid-input] --position takes two numbers such as 0.5,0.5")
                })?;
                let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let placement = pentool::photo::catalog::Placement {
                    page: selected_page.map(str::to_string),
                    layer,
                    id: node_id,
                    x,
                    y,
                    width,
                    height,
                    fit: fit.as_str().to_string(),
                    position,
                };
                let result =
                    pentool::photo::catalog::place(&mut raw, &input, &id, &variant, &placement)?;
                let change = transaction::commit_value(
                    &input,
                    "photo-place",
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
            PhotoAction::MergeHdr {
                input,
                photos,
                id,
                name,
                deghost,
                reference,
                common,
            } => {
                use pentool::photo::merge;
                if reference == Some(0) {
                    anyhow::bail!("[invalid-input] --reference is 1-based")
                }
                let kind = pentool::photo::catalog::MergeKind::Hdr(merge::HdrOptions {
                    reference: reference.map(|r| r - 1),
                    deghost: merge::Deghost::parse(&deghost)?,
                    scale: common.scale,
                });
                run_photo_merge(input, &photos, &id, name, kind, common, "photo-merge-hdr")
            }
            PhotoAction::MergePano {
                input,
                photos,
                id,
                name,
                projection,
                seed,
                focal,
                common,
            } => {
                use pentool::photo::merge;
                let kind = pentool::photo::catalog::MergeKind::Pano(merge::PanoOptions {
                    projection: merge::Projection::parse(&projection)?,
                    seed,
                    focal,
                    scale: common.scale,
                });
                run_photo_merge(input, &photos, &id, name, kind, common, "photo-merge-pano")
            }
            PhotoAction::Variant { action } => {
                use pentool::photo::variants;
                match action {
                    PhotoVariantAction::Add {
                        input,
                        photo,
                        id,
                        from,
                        from_snapshot,
                        name,
                        edit,
                    } => run_photo_edit(&input, "photo-variant-add", edit, |raw| {
                        let from = match &from_snapshot {
                            Some(snapshot) => variants::VariantSource::Snapshot(snapshot),
                            None => variants::VariantSource::Variant(
                                from.as_deref().unwrap_or("master"),
                            ),
                        };
                        variants::add_variant(raw, &photo, &id, from, name.as_deref())
                    }),
                    PhotoVariantAction::Rename {
                        input,
                        photo,
                        id,
                        name,
                        edit,
                    } => run_photo_edit(&input, "photo-variant-rename", edit, |raw| {
                        variants::rename_variant(raw, &photo, &id, name.as_deref())
                    }),
                    PhotoVariantAction::Remove {
                        input,
                        photo,
                        id,
                        edit,
                    } => run_photo_edit(&input, "photo-variant-remove", edit, |raw| {
                        variants::remove_variant(raw, &photo, &id)
                    }),
                }
            }
            PhotoAction::Snapshot { action } => {
                use pentool::photo::variants;
                match action {
                    PhotoSnapshotAction::Add {
                        input,
                        target,
                        id,
                        name,
                        edit,
                    } => {
                        let (photo, variant) = variants::split_target(&target)?;
                        run_photo_edit(&input, "photo-snapshot-add", edit, |raw| {
                            variants::add_snapshot(raw, &photo, &variant, &id, name.as_deref())
                        })
                    }
                    PhotoSnapshotAction::Restore {
                        input,
                        photo,
                        id,
                        edit,
                    } => run_photo_edit(&input, "photo-snapshot-restore", edit, |raw| {
                        variants::restore_snapshot(raw, &photo, &id)
                    }),
                    PhotoSnapshotAction::Remove {
                        input,
                        photo,
                        id,
                        edit,
                    } => run_photo_edit(&input, "photo-snapshot-remove", edit, |raw| {
                        variants::remove_snapshot(raw, &photo, &id)
                    }),
                }
            }
            PhotoAction::Settings {
                action:
                    PhotoSettingsAction::Sync {
                        input,
                        source,
                        to,
                        groups,
                        except,
                        auto_per_photo,
                        edit,
                    },
            } => {
                use pentool::photo::variants;
                let source = variants::split_target(&source)?;
                let document = input.clone();
                run_photo_edit(&input, "photo-settings-sync", edit, |raw| {
                    let targets = sync_targets(&to, raw)?;
                    variants::sync_settings(
                        raw,
                        &document,
                        variants::SyncRequest {
                            source: (&source.0, &source.1),
                            targets: &targets,
                            groups: &groups,
                            except: &except,
                            auto_per_photo,
                        },
                    )
                })
            }
            PhotoAction::Search {
                input,
                query,
                limit,
                offset,
            } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let result = pentool::photo::organize::search(&raw, &query, limit, offset)?;
                println!("{}", serde_json::to_string_pretty(&result)?);
                Ok(())
            }
            PhotoAction::Rate {
                input,
                selection,
                rating,
                pick,
                label,
                edit,
            } => {
                use pentool::photo::organize;
                let selection = selection_arg(&selection)?;
                run_photo_edit(&input, "photo-rate", edit, |raw| {
                    organize::rate(
                        raw,
                        &selection,
                        &organize::Rating {
                            rating,
                            pick,
                            label,
                        },
                    )
                })
            }
            PhotoAction::Keyword {
                input,
                selection,
                add,
                remove,
                clear,
                edit,
            } => {
                let selection = selection_arg(&selection)?;
                run_photo_edit(&input, "photo-keyword", edit, |raw| {
                    pentool::photo::organize::keywords(raw, &selection, &add, &remove, clear)
                })
            }
            PhotoAction::Stack { action } => {
                use pentool::photo::organize;
                match action {
                    PhotoStackAction::Add {
                        input,
                        stack,
                        photos,
                        edit,
                    } => {
                        let selection = selection_arg(&photos)?;
                        run_photo_edit(&input, "photo-stack-add", edit, |raw| {
                            organize::add_stack(raw, &stack, &selection)
                        })
                    }
                    PhotoStackAction::Top {
                        input,
                        stack,
                        photo,
                        edit,
                    } => run_photo_edit(&input, "photo-stack-top", edit, |raw| {
                        organize::stack_top(raw, &stack, &photo)
                    }),
                    PhotoStackAction::Remove { input, stack, edit } => {
                        run_photo_edit(&input, "photo-stack-remove", edit, |raw| {
                            organize::remove_stack(raw, &stack)
                        })
                    }
                }
            }
            PhotoAction::Collection { action } => {
                use pentool::photo::organize;
                match action {
                    PhotoCollectionAction::Add {
                        input,
                        id,
                        name,
                        photos,
                        query,
                        edit,
                    } => {
                        let spec = match (photos, query) {
                            (Some(photos), _) => {
                                organize::CollectionSpec::Manual(selection_arg(&photos)?)
                            }
                            (None, Some(query)) => organize::CollectionSpec::Smart(query),
                            (None, None) => unreachable!("clap requires --photos or --query"),
                        };
                        run_photo_edit(&input, "photo-collection-add", edit, |raw| {
                            organize::add_collection(raw, &id, name.as_deref(), spec)
                        })
                    }
                    PhotoCollectionAction::Update {
                        input,
                        id,
                        add,
                        remove,
                        edit,
                    } => {
                        let add = add.as_deref().map(selection_arg).transpose()?;
                        let remove = remove.as_deref().map(selection_arg).transpose()?;
                        run_photo_edit(&input, "photo-collection-update", edit, |raw| {
                            organize::update_collection(raw, &id, add.as_ref(), remove.as_ref())
                        })
                    }
                    PhotoCollectionAction::Remove { input, id, edit } => {
                        run_photo_edit(&input, "photo-collection-remove", edit, |raw| {
                            organize::remove_collection(raw, &id)
                        })
                    }
                }
            }
            PhotoAction::ContactSheet {
                input,
                selection,
                columns,
                sheet,
                variant,
            } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let ids = selection_arg(&selection)?.resolve(&raw)?;
                let items: Vec<(String, String)> =
                    ids.into_iter().map(|id| (id, variant.clone())).collect();
                run_photo_sheet(&raw, &input, &items, columns, sheet)
            }
            PhotoAction::Compare {
                input,
                items,
                sheet,
            } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let items = items
                    .iter()
                    .map(|item| pentool::photo::variants::split_target(item))
                    .collect::<Result<Vec<_>>>()?;
                let columns = if items.len() <= 4 {
                    items.len() as u32
                } else {
                    4
                };
                run_photo_sheet(&raw, &input, &items, columns, sheet)
            }
            PhotoAction::Metadata { input, id, reveal } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let report = pentool::photo::metadata::show(&raw, &input, &id, &reveal)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            PhotoAction::PrivacyReport { input } => {
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let report = pentool::photo::metadata::privacy_report(&raw, &input)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            PhotoAction::Describe {
                input,
                selection,
                title,
                caption,
                creator,
                copyright,
                sublocation,
                city,
                state,
                country,
                edit,
            } => {
                use pentool::photo::metadata;
                let selection = selection_arg(&selection)?;
                let description = metadata::Description {
                    title,
                    caption,
                    creator,
                    copyright,
                    sublocation,
                    city,
                    state,
                    country,
                };
                run_photo_edit(&input, "photo-describe", edit, |raw| {
                    metadata::describe(raw, &selection, &description)
                })
            }
            PhotoAction::Export {
                input,
                selection,
                recipe,
                out,
                variant,
                all_variants,
                print,
                fit,
                overwrite,
                dry_run,
            } => {
                use pentool::photo::export;
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let print = print.as_deref().map(export::parse_print).transpose()?;
                let fit = fit.as_deref().map(export::Fit::parse).transpose()?;
                let recipe = export::Recipe::named(&raw, &recipe)?.with_print(print, fit)?;
                let photos = selection_arg(&selection)?.resolve(&raw)?;
                if photos.is_empty() {
                    anyhow::bail!("[invalid-input] the selection matches no photos")
                }
                let variants = match (variant, all_variants) {
                    (Some(name), _) => export::Variants::Named(name),
                    (None, true) => export::Variants::All,
                    (None, false) => export::Variants::Master,
                };
                let planned =
                    export::plan(&raw, &input, &recipe, &photos, &variants, &out, overwrite)?;
                let report = if dry_run {
                    export::plan_report(&recipe, &out, &planned)
                } else {
                    export::run(&raw, &input, &recipe, &planned, &out)?
                };
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            PhotoAction::Preview {
                input,
                id,
                variant,
                out,
                edge,
                space,
                overlay,
                uncropped,
                no_cache,
            } => {
                use pentool::photo::{cache::Cache, studio};
                let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                let cache = (!no_cache).then(|| Cache::for_document(&input));
                let preview = studio::preview_cached(
                    &raw,
                    &input,
                    &studio::PreviewRequest {
                        photo: &id,
                        variant: &variant,
                        edge,
                        space: &space,
                        overlay: studio::Overlay::parse(&overlay)?,
                        uncropped,
                    },
                    cache.as_ref(),
                )?;
                let temporary = out.with_extension("png.tmp");
                fs::write(&temporary, &preview.png)
                    .with_context(|| format!("write {}", temporary.display()))?;
                fs::rename(&temporary, &out).with_context(|| format!("write {}", out.display()))?;
                let mut report = preview.report;
                report["out"] = serde_json::json!(out.display().to_string());
                report["bytes"] = serde_json::json!(preview.png.len());
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            PhotoAction::Cache {
                action: PhotoCacheAction::Clear { input },
            } => {
                let report = pentool::photo::cache::clear(&input)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            PhotoAction::Recipe { action } => match action {
                PhotoRecipeAction::List { input } => {
                    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
                    let report = pentool::photo::export::Recipe::list(&raw)?;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                    Ok(())
                }
                PhotoRecipeAction::Set {
                    input,
                    name,
                    recipe,
                    edit,
                } => {
                    let text = match recipe.strip_prefix('@') {
                        Some(path) => fs::read_to_string(path)
                            .with_context(|| format!("[missing-resource] cannot read {path}"))?,
                        None => recipe,
                    };
                    let recipe: serde_json::Value = serde_json::from_str(&text)
                        .context("[invalid-input] the recipe must be a JSON object")?;
                    run_photo_edit(&input, "photo-recipe-set", edit, |raw| {
                        pentool::photo::export::set_recipe(raw, &name, recipe)
                    })
                }
                PhotoRecipeAction::Remove { input, name, edit } => {
                    run_photo_edit(&input, "photo-recipe-remove", edit, |raw| {
                        pentool::photo::export::remove_recipe(raw, &name)
                    })
                }
            },
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
                )?)?
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
        Command::Extension { action } => {
            let project_of = |p: Option<PathBuf>| -> Result<PathBuf> {
                Ok(match p {
                    Some(p) => p,
                    None => std::env::current_dir()?,
                })
            };
            let parse_json =
                |label: &str, raw: Option<String>| -> Result<Option<serde_json::Value>> {
                    raw.map(|r| {
                        serde_json::from_str(&r)
                            .with_context(|| format!("--{label} must be valid JSON"))
                    })
                    .transpose()
                };
            let value = match action {
                ExtensionAction::Pack { dir, output } => extension::store::pack(&dir, &output)?,
                ExtensionAction::Verify { package } => extension::store::verify(&package)?,
                ExtensionAction::Add {
                    package,
                    approve,
                    project,
                } => extension::store::add(&package, &project_of(project)?, approve)?,
                ExtensionAction::List { project } => extension::store::list(&project_of(project)?)?,
                ExtensionAction::Remove { name, project } => {
                    extension::store::remove(&project_of(project)?, &name)?
                }
                ExtensionAction::Run {
                    name,
                    file,
                    command,
                    args,
                    event,
                    dry_run,
                    if_revision,
                    project,
                } => extension::store::run(
                    &project_of(project)?,
                    &name,
                    &file,
                    extension::store::RunOptions {
                        command,
                        args: parse_json("args", args)?,
                        event: parse_json("event", event)?,
                        page: selected_page.map(str::to_string),
                        dry_run,
                        if_revision,
                    },
                )?,
            };
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
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
            photo: photo_benchmark,
            scene3d: scene3d_benchmark,
            scene3d_cache: scene3d_cache_benchmark,
            triangles,
            texture_edge,
            photos,
            megapixels,
            json: _,
        } => {
            if !scene3d_benchmark
                && !scene3d_cache_benchmark
                && (triangles != 100_000 || texture_edge != 0)
            {
                anyhow::bail!("--triangles and --texture-edge require --scene3d")
            }
            let modes = [
                images.is_some(),
                composite_benchmark,
                photo_benchmark,
                scene3d_benchmark,
                scene3d_cache_benchmark,
            ]
            .into_iter()
            .filter(|active| *active)
            .count();
            if modes > 1 {
                anyhow::bail!("choose only one benchmark mode")
            }
            let value = if scene3d_benchmark {
                if render_benchmark || paths_only || scale != 1.0 || warmups != 1 {
                    anyhow::bail!(
                        "scene3d benchmark sets its own render workload; omit --render, --paths-only, --scale and --warmups"
                    )
                }
                benchmark::run_scene3d(benchmark::Scene3dBenchmark {
                    triangles,
                    texture_edge,
                    repetitions,
                })?
            } else if scene3d_cache_benchmark {
                if render_benchmark
                    || paths_only
                    || scale != 1.0
                    || warmups != 1
                    || triangles != 100_000
                    || texture_edge != 0
                {
                    anyhow::bail!(
                        "scene3d cache benchmark sets its own render workload; omit render, paths, scale, warmups, triangle and texture options"
                    )
                }
                benchmark::run_scene3d_cache(repetitions)?
            } else if photo_benchmark {
                if scale != 1.0 || warmups != 1 {
                    anyhow::bail!("photo benchmark sets its own sizes; omit --scale and --warmups")
                }
                benchmark::run_photo(benchmark::PhotoBenchmark {
                    photos,
                    megapixels,
                    repetitions,
                })?
            } else if composite_benchmark {
                if scale != 1.0 || warmups != 1 {
                    anyhow::bail!(
                        "composite benchmark uses scale 1 and one cold run; omit --scale and --warmups"
                    )
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
            if photo_benchmark {
                if let Some(max_ms) = max_ms {
                    let t = &value["timings_us"];
                    let develop = t["develop"].as_array().into_iter().flatten();
                    let total = develop
                        .filter_map(serde_json::Value::as_u64)
                        .max()
                        .unwrap_or(0)
                        + t["ingest"].as_u64().unwrap_or(0)
                        + t["cull"].as_u64().unwrap_or(0)
                        + t["export"].as_u64().unwrap_or(0);
                    if total as f64 / 1000.0 > max_ms as f64 {
                        anyhow::bail!(
                            "photo benchmark exceeded --max-ms: {:.3} ms > {max_ms} ms",
                            total as f64 / 1000.0
                        )
                    }
                }
            }
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
            if scene3d_benchmark {
                if let Some(max_ms) = max_ms {
                    let worst = value["timings_us"]["max"].as_u64().unwrap_or(0);
                    if worst as u128 > max_ms * 1000 {
                        anyhow::bail!(
                            "scene3d benchmark exceeded --max-ms: {:.3} ms > {max_ms} ms",
                            worst as f64 / 1000.0
                        )
                    }
                }
            }
            if scene3d_cache_benchmark {
                if let Some(max_ms) = max_ms {
                    let worst = value["samples"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|sample| sample["cold_us"].as_u64())
                        .max()
                        .unwrap_or(0);
                    if worst as u128 > max_ms * 1000 {
                        anyhow::bail!(
                            "scene3d cache benchmark exceeded --max-ms: {:.3} ms > {max_ms} ms",
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
            json::reject_duplicate_keys(&operation_bytes)?;
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
        Command::Serve {
            host,
            port,
            input,
            no_cache,
            collab,
            allow_insecure_lan,
        } => {
            let options = collab.then(|| server::CollabOptions {
                limits: pentool::collab::sync::Limits::default(),
                allow_insecure_lan,
            });
            if options.is_some() && input.is_none() {
                bail!("[invalid-input] --collab needs a document: pentool serve DOC --collab");
            }
            server::serve_with(&host, port, input, no_cache, options).await
        }
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
        Command::Spread {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_spread(&input, dry_run, if_revision.as_deref(), action),
        Command::Scene3d {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::scene3d::cli::run(&input, dry_run, if_revision.as_deref(), action),
        Command::Scene3dCache { server, action } => {
            pentool::scene3d::cli::run_cache_command(&server, action)
        }
        Command::Mockup {
            dry_run,
            if_revision,
            action,
        } => match action {
            MockupAction::List => {
                anyhow::ensure!(
                    !dry_run && if_revision.is_none(),
                    "--dry-run and --if-revision apply only to `mockup apply`"
                );
                pentool::scene3d::cli::run_mockup_list()
            }
            MockupAction::Apply {
                input,
                preset,
                id,
                layer,
                page,
                x,
                y,
                width,
                height,
            } => pentool::scene3d::cli::run_mockup_apply(
                &input,
                dry_run,
                if_revision.as_deref(),
                &preset,
                &id,
                &layer,
                pentool::scene3d::cli::MockupPlacement {
                    page,
                    rect: [x, y, width, height],
                },
            ),
        },
        Command::Pack {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::scene3d::cli::run_pack(
            input.as_deref(),
            dry_run,
            if_revision.as_deref(),
            action,
        ),
        Command::LayoutStyle {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_layout_style(&input, dry_run, if_revision.as_deref(), action),
        Command::LayoutGrid {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_layout_grid(&input, dry_run, if_revision.as_deref(), action),
        Command::LayoutWrap {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_layout_wrap(&input, dry_run, if_revision.as_deref(), action),
        Command::LayoutTable {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_layout_table(&input, dry_run, if_revision.as_deref(), action),
        Command::LayoutDoc {
            input,
            dry_run,
            if_revision,
            file_name,
            action,
        } => pentool::layout_cli::run_layout_doc(
            &input,
            dry_run,
            if_revision.as_deref(),
            file_name,
            action,
        ),
        Command::LayoutExport {
            input,
            out,
            format,
            fixed,
            scale,
            title,
            language,
            overwrite,
        } => pentool::layout_cli::run_layout_export(
            &input,
            &out,
            format,
            fixed,
            scale,
            title,
            language,
            overwrite,
            selected_page,
        ),
        Command::LayoutPreflight {
            input,
            bleed,
            max_overrides,
            no_alt,
        } => pentool::layout_cli::run_layout_preflight(&input, bleed, max_overrides, no_alt),
        Command::LayoutMerge {
            input,
            data,
            out,
            format,
            delimiter,
            template_page,
            records_per_page,
            grid,
            cell,
            image_fields,
            keep_template,
            dry_run,
            overwrite,
        } => {
            if out.exists() && !overwrite && !dry_run {
                anyhow::bail!(
                    "[policy-denied] {} already exists; choose another path or pass --overwrite; nothing was written",
                    out.display()
                )
            }
            let pair = |text: &str, what: &str| -> Result<(f64, f64)> {
                let (a, b) = text
                    .split_once(['x', 'X'])
                    .with_context(|| format!("[invalid-input] {what} must be AxB"))?;
                Ok((
                    a.trim()
                        .parse()
                        .with_context(|| format!("[invalid-input] {what} {text:?}"))?,
                    b.trim()
                        .parse()
                        .with_context(|| format!("[invalid-input] {what} {text:?}"))?,
                ))
            };
            let grid = match &grid {
                Some(g) => {
                    let (c, r) = pair(g, "--grid")?;
                    anyhow::ensure!(
                        c.fract() == 0.0 && r.fract() == 0.0 && c >= 1.0 && r >= 1.0,
                        "[invalid-input] --grid must be whole numbers COLSxROWS"
                    );
                    Some((c as usize, r as usize))
                }
                None => None,
            };
            let cell = match &cell {
                Some(c) => Some(pair(c, "--cell")?),
                None => None,
            };
            let mut fields = Vec::new();
            for spec in &image_fields {
                let (frame, field) = spec
                    .split_once('=')
                    .context("[invalid-input] --image-field must be FRAME=FIELD")?;
                fields.push((frame.to_string(), field.to_string()));
            }
            let format = pentool::merge::data_format(&data, format.as_deref())?;
            let bytes = pentool::resource::read_bounded(&data, pentool::merge::MAX_DATA_BYTES)
                .with_context(|| format!("[missing-resource] cannot read {}", data.display()))?;
            let records = pentool::merge::parse_records(&bytes, &format, delimiter)?;
            let template: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&input).with_context(|| {
                    format!("[missing-resource] could not read {}", input.display())
                })?)
                .context("[malformed-resource] invalid .pen document")?;
            let dir_of = |path: &std::path::Path| -> PathBuf {
                match path.parent() {
                    Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                    _ => PathBuf::from("."),
                }
            };
            let outcome = pentool::merge::merge(
                &template,
                &records,
                &pentool::merge::MergeOptions {
                    template_page,
                    records_per_page,
                    grid,
                    cell,
                    image_fields: fields,
                    keep_template,
                    data_dir: dir_of(&data),
                    out_dir: dir_of(&out),
                },
            )?;
            let mut report = outcome.report;
            if !dry_run {
                let bytes = serde_json::to_vec_pretty(&outcome.document)?;
                editing::atomic_write(&out, &bytes)
                    .with_context(|| format!("could not write {}", out.display()))?;
                report["output"] = serde_json::json!(out.display().to_string());
                report["revision"] = serde_json::json!(pentool::transaction::revision(&bytes));
            }
            report["dry_run"] = serde_json::json!(dry_run);
            println!("{report}");
            Ok(())
        }
        Command::LayoutAdapt {
            input,
            page,
            width,
            height,
            rule,
            uniform,
            objects,
            dry_run,
            if_revision,
        } => pentool::layout_cli::run_layout_adapt(
            &input,
            dry_run,
            if_revision.as_deref(),
            &page,
            width,
            height,
            &rule,
            uniform,
            &objects,
        ),
        Command::LayoutAlt {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_layout_alt(&input, dry_run, if_revision.as_deref(), action),
        Command::Book { book, action } => {
            let result = pentool::book::run(&book, action)?;
            println!("{result}");
            Ok(())
        }
        Command::Parent {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_parent(&input, dry_run, if_revision.as_deref(), action),
        Command::Section {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_section(&input, dry_run, if_revision.as_deref(), action),
        Command::Override {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::layout_cli::run_override(&input, dry_run, if_revision.as_deref(), action),
        Command::Raster {
            input,
            dry_run,
            if_revision,
            action,
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if !scene::is_scene_document(&raw) {
                bail!(
                    "[unsupported-version] raster layers need a v6 scene document; run `pentool migrate` first"
                )
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
                        bail!(
                            "[corrupt-raster] raster verification found damage; see the report for the fix"
                        )
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
                RasterAction::StrokeField {
                    id,
                    field,
                    params,
                    brush,
                    preset,
                    color,
                    blend,
                    seed,
                    constraints,
                } => {
                    let read = |text: &str| -> Result<serde_json::Value> {
                        let body = match text.strip_prefix('@') {
                            Some(path) => {
                                if fs::metadata(path)?.len() > 4 << 20 {
                                    bail!("[limit-exceeded] {path} is larger than 4 MiB")
                                }
                                fs::read_to_string(path)?
                            }
                            None => text.to_owned(),
                        };
                        Ok(serde_json::from_str(&body)?)
                    };
                    let request = raster::fields::FieldRequest {
                        field: read(&field)?,
                        params: read(&params)?,
                        brush: raster::Brush::parse(&raster::resolve_preset(
                            &raw,
                            &read(&brush)?,
                            preset.as_deref(),
                        )?)?,
                        color: raster::parse_color(&color)?,
                        blend: raster::Blend::parse(&blend)?,
                        seed,
                        constraints: match &constraints {
                            Some(text) => read(text)?,
                            None => serde_json::Value::Null,
                        },
                    };
                    let result = raster::fields::paint_field(&mut raw, page, &id, request)?;
                    ("raster-stroke-field", result)
                }
                RasterAction::StrokePath {
                    id,
                    path,
                    flatness,
                    curves,
                    brush,
                    preset,
                    color,
                    blend,
                    seed,
                    constraints,
                } => {
                    let read = |text: &str| -> Result<serde_json::Value> {
                        let body = match text.strip_prefix('@') {
                            Some(path) => {
                                const MAX_JSON: u64 = 4 << 20;
                                if fs::metadata(path)?.len() > MAX_JSON {
                                    bail!("[limit-exceeded] {path} is larger than 4 MiB")
                                }
                                fs::read_to_string(path)?
                            }
                            None => text.to_owned(),
                        };
                        Ok(serde_json::from_str(&body)?)
                    };
                    let request = raster::PathRequest {
                        path,
                        flatness,
                        brush: raster::Brush::parse(&raster::resolve_preset(
                            &raw,
                            &read(&brush)?,
                            preset.as_deref(),
                        )?)?,
                        color: raster::parse_color(&color)?,
                        blend: raster::Blend::parse(&blend)?,
                        seed,
                        curves: read(&curves)?,
                        constraints: match &constraints {
                            Some(text) => read(text)?,
                            None => serde_json::Value::Null,
                        },
                    };
                    let result = raster::paint_path(&mut raw, page, &id, request)?;
                    ("raster-stroke-path", result)
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
            format,
            report,
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
            let format = format
                .map(|format| match format {
                    DesignFormat::Svg => "svg",
                    DesignFormat::Pdf => "pdf",
                    DesignFormat::Psd => "psd",
                    DesignFormat::Sketch => "sketch",
                    DesignFormat::Figma => "figma",
                })
                .or_else(|| pentool::interchange::detect_format(&source));
            if let Some(format) = format {
                if source_page.is_some()
                    || no_prefix
                    || at != [0.0, 0.0]
                    || scale != 1.0
                    || rotate != 0.0
                    || expand_canvas
                {
                    anyhow::bail!(
                        "[invalid-input] --source-page, --no-prefix, --at, --scale, --rotate, and --expand-canvas apply only to legacy .pen composition; design-format imports use a dedicated namespaced layer"
                    );
                }
                if source == destination
                    || report.as_ref() == Some(&source)
                    || report.as_ref() == Some(&destination)
                {
                    anyhow::bail!(
                        "[invalid-input] source, destination, and fidelity report paths must be distinct"
                    );
                }
                let destination_bytes =
                    pentool::resource::read_bounded(&destination, 512 * 1024 * 1024).with_context(
                        || format!("could not read destination {}", destination.display()),
                    )?;
                let destination_document: serde_json::Value =
                    serde_json::from_slice(&destination_bytes)
                        .context("destination is not a valid Pentool JSON document")?;
                let source_bytes = pentool::resource::read_bounded(&source, 512 * 1024 * 1024)
                    .with_context(|| format!("could not read source {}", source.display()))?;
                let imported = pentool::interchange::import_design(
                    format,
                    &source_bytes,
                    &source.display().to_string(),
                )?;
                let mut fidelity_report = imported.report;
                let prefix = prefix.unwrap_or_else(|| {
                    source
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .replace(
                            |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
                            "-",
                        )
                });
                let (document, import_summary) = pentool::interchange::append_imported_document(
                    &destination_document,
                    &imported.document,
                    selected_page,
                    &prefix,
                    &mut fidelity_report,
                )?;
                let change = transaction::commit_value(
                    &destination,
                    "import",
                    dry_run,
                    revision.as_deref(),
                    &document,
                )?;
                if let Some(report_path) = &report {
                    if !dry_run {
                        editing::atomic_write(
                            report_path,
                            &serde_json::to_vec_pretty(&fidelity_report)?,
                        )
                        .with_context(|| format!("could not write {}", report_path.display()))?;
                    }
                }
                let mut fidelity = serde_json::to_value(&fidelity_report)?;
                if report.is_some() && !dry_run {
                    if let Some(object) = fidelity.as_object_mut() {
                        object.remove("items");
                        object.insert(
                            "report_path".into(),
                            serde_json::json!(report.as_ref().unwrap().display().to_string()),
                        );
                    }
                }
                println!(
                    "{}",
                    serde_json::json!({"change":change,"summary":import_summary,"fidelity":fidelity})
                );
                return Ok(());
            }
            if report.is_some() {
                anyhow::bail!(
                    "[invalid-input] --report is available for design-format imports; choose --format"
                );
            }
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
            match &action {
                text::TextAction::Shape(args) => {
                    println!("{}", pentool::shaping::run_shape(&raw, args)?);
                    return Ok(());
                }
                text::TextAction::Compose(args) => {
                    println!("{}", pentool::compose::run_compose(&raw, args)?);
                    return Ok(());
                }
                text::TextAction::FontReport => {
                    println!("{}", pentool::shaping::font_report(&raw)?);
                    return Ok(());
                }
                _ => {}
            }
            if scene::is_scene_document(&raw) {
                if matches!(
                    action,
                    text::TextAction::Story { .. } | text::TextAction::Frame { .. }
                ) {
                    return pentool::layout_cli::run_text_layout(
                        &input,
                        dry_run,
                        if_revision.as_deref(),
                        raw,
                        action,
                    );
                }
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
        } => {
            let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&input)?)?;
            if scene::is_scene_document(&raw) {
                let result = pentool::shaping::scene_font(&mut raw, action)?;
                let change = transaction::commit_value(
                    &input,
                    "font",
                    dry_run,
                    if_revision.as_deref(),
                    &raw,
                )?;
                println!(
                    "{}",
                    serde_json::json!({"change": change, "result": result})
                );
                Ok(())
            } else {
                editing::edit_page_options(
                    &input,
                    selected_page,
                    "font",
                    dry_run,
                    if_revision.as_deref(),
                    |doc| text::font(doc, action),
                )
            }
        }
        Command::Fonts { input } => {
            let doc = match input {
                Some(path) => read_render_document(&path, selected_page)?,
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
                println!("{}", serde_json::to_string(&summary.to_value())?);
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
                            if v.trim().is_empty() {
                                anyhow::bail!("document name cannot be empty");
                            }
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
        Command::IdmlImport {
            input,
            output,
            report,
            overwrite,
        } => {
            if output.exists() && !overwrite {
                anyhow::bail!(
                    "[policy-denied] {} already exists; choose another path or pass --overwrite; nothing was written",
                    output.display()
                )
            }
            let bytes =
                pentool::resource::read_bounded(&input, pentool::idml::MAX_ARCHIVE_BYTES as usize)
                    .with_context(|| {
                        format!("[missing-resource] cannot read {}", input.display())
                    })?;
            let imported = pentool::idml::import(&bytes, &input.display().to_string())?;
            let pretty = serde_json::to_vec_pretty(&imported.report)?;
            if let Some(path) = &report {
                editing::atomic_write(path, &pretty)
                    .with_context(|| format!("could not write {}", path.display()))?;
            }
            editing::atomic_write(&output, &serde_json::to_vec_pretty(&imported.document)?)
                .with_context(|| format!("could not write {}", output.display()))?;
            let mut summary = imported.report.clone();
            if let Some(map) = summary.as_object_mut() {
                if report.is_some() {
                    map.remove("items");
                }
                map.insert(
                    "output".into(),
                    serde_json::json!(output.display().to_string()),
                );
            }
            println!("{}", serde_json::to_string(&summary)?);
            Ok(())
        }
        Command::IdmlExport {
            input,
            output,
            report,
            allow_approximation,
            overwrite,
        } => {
            // The bytes are always an IDML package; a `.zip`/`.pen` name would
            // mislabel the content, so validate the extension up front.
            anyhow::ensure!(
                output
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("idml")),
                "[invalid-input] the IDML export output must end in .idml, got {}; the file is always an IDML package",
                output.display()
            );
            if output.exists() && !overwrite {
                anyhow::bail!(
                    "[policy-denied] {} already exists; choose another path or pass --overwrite; nothing was written",
                    output.display()
                )
            }
            let raw: serde_json::Value = serde_json::from_slice(
                &pentool::resource::read_bounded(&input, 256 * 1024 * 1024).with_context(|| {
                    format!("[missing-resource] cannot read {}", input.display())
                })?,
            )
            .with_context(|| format!("{} is not a Pentool document", input.display()))?;
            transaction::validate_value(&raw)?;
            let exported = pentool::idml_export::export(
                &raw,
                &input.display().to_string(),
                allow_approximation,
            )?;
            if let Some(path) = &report {
                editing::atomic_write(path, &serde_json::to_vec_pretty(&exported.report)?)
                    .with_context(|| format!("could not write {}", path.display()))?;
            }
            editing::atomic_write(&output, &exported.bytes)
                .with_context(|| format!("could not write {}", output.display()))?;
            let mut summary = exported.report.clone();
            if let Some(map) = summary.as_object_mut() {
                if report.is_some() {
                    map.remove("items");
                }
                map.insert(
                    "output".into(),
                    serde_json::json!(output.display().to_string()),
                );
            }
            println!("{}", serde_json::to_string(&summary)?);
            Ok(())
        }
        Command::New {
            output,
            width,
            height,
            overwrite,
        } => {
            if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
                anyhow::bail!(
                    "[invalid-input] canvas dimensions must be between 1 and 16384, got {width}x{height}; nothing was written"
                )
            }
            let doc = scene::new_document(width, height);
            scene::validate(&doc)?;
            if output.exists() {
                if !overwrite {
                    anyhow::bail!(
                        "[policy-denied] {} already exists; choose another path or pass --overwrite (the previous document stays recoverable with `pentool undo`)",
                        output.display()
                    )
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
            if selected_page.is_some() {
                bail!(
                    "[invalid-input] format migration affects the entire document; --page is not applicable"
                );
            }
            if !matches!(target, 3..=12) {
                anyhow::bail!("migration target must be 3 through 12")
            }
            let before = if u64::from(target) >= pentool::color::VERSION {
                pentool::resource::read_bounded(
                    &input,
                    pentool::color::catalog::MAX_CATALOG_DOCUMENT_BYTES,
                )?
            } else {
                fs::read(&input).with_context(|| format!("could not read {}", input.display()))?
            };
            let raw: serde_json::Value =
                serde_json::from_slice(&before).context("invalid .pen document")?;
            let from = raw.get("version").and_then(serde_json::Value::as_u64);
            let migrated = match target {
                12 => scene::migrate_to_v12(raw)?,
                11 if from == Some(pentool::scene::PAINT_VERSION) => {
                    pentool::svg_paint::downgrade_to_v11(raw)?
                }
                11 => pentool::scene3d::node::migrate_to_v11(raw)?,
                10 => pentool::scene3d::node::migrate(pentool::scene3d::node::to_v10(raw)?)?,
                9 => pentool::spreads::migrate(raw)?,
                8 => pentool::color::catalog::migrate(raw)?,
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
        Command::PaintPalette {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::paint_palette::run(
            &input,
            selected_page,
            dry_run,
            if_revision.as_deref(),
            action,
        ),
        Command::PaintFields { input, action } => pentool::paint_fields::run(&input, action),
        Command::PaintMedia { action } => pentool::paint_media::run(action),
        Command::PaintConstraints { input, action } => {
            pentool::paint_constraints::run(&input, action)
        }
        Command::PaintScore(args) => pentool::paint_score::run(*args),
        Command::PaintGuide {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::paint_guide::run(&input, dry_run, if_revision.as_deref(), action),
        Command::PaintBundle {
            input,
            dry_run,
            if_revision,
            action,
        } => pentool::paint_bundle::run(&input, dry_run, if_revision.as_deref(), action),
        Command::PaintDiff(args) => pentool::paint_bundle::run_diff(*args),
        Command::PaintPlan(args) => pentool::paint_plan::run(*args, selected_page),
        Command::PaintAnalyze(args) => pentool::paint_analysis::run(*args, selected_page),
        Command::PaintReview { input, action } => {
            pentool::paint_review::run(&input, selected_page, action)
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
            if !matches!(
                args.operation.as_str(),
                "add" | "set" | "enable" | "disable" | "remove"
            ) {
                anyhow::bail!("unknown adjustment operation {}", args.operation)
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
        Command::Design { command } => pentool::design::cli::run_and_print(command),
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
                    align: format!("{:?}", args.align).to_lowercase(),
                    vertical_align: format!("{:?}", args.vertical_align).to_lowercase(),
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
        Command::Comment { action } => {
            let (result, compact) = match action {
                CommentAction::Add {
                    input,
                    body,
                    page,
                    object,
                    x,
                    y,
                    actor,
                    json,
                } => {
                    let target = comments::Target {
                        page,
                        object,
                        point: x.zip(y).map(|(x, y)| [x, y]),
                    };
                    (comments::add(&input, target, &actor.actor(), &body)?, json)
                }
                CommentAction::Reply {
                    input,
                    thread,
                    body,
                    if_version,
                    actor,
                    json,
                } => (
                    comments::reply(&input, &thread, &actor.actor(), &body, if_version)?,
                    json,
                ),
                CommentAction::Resolve {
                    input,
                    thread,
                    if_version,
                    json,
                } => (
                    comments::set_resolved(&input, &thread, true, if_version)?,
                    json,
                ),
                CommentAction::Reopen {
                    input,
                    thread,
                    if_version,
                    json,
                } => (
                    comments::set_resolved(&input, &thread, false, if_version)?,
                    json,
                ),
                CommentAction::Review {
                    input,
                    thread,
                    state,
                    if_version,
                    json,
                } => (
                    comments::set_review(&input, &thread, &state, if_version)?,
                    json,
                ),
                CommentAction::List {
                    input,
                    status,
                    review_state,
                    page,
                    object,
                    mention,
                    thread,
                    comments: with_comments,
                    offset,
                    limit,
                    json,
                } => (
                    comments::list(
                        &input,
                        &comments::ListOptions {
                            status,
                            review_state,
                            page,
                            object,
                            mention,
                            thread,
                            comments: with_comments,
                            offset,
                            limit,
                        },
                    )?,
                    json,
                ),
            };
            if compact {
                println!("{}", serde_json::to_string(&result)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
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
        Command::Collab { action } => {
            let result = match action {
                CollabAction::Init { input } => pentool::collab::init(&input)?,
                CollabAction::Log {
                    input,
                    offset,
                    limit,
                    actor,
                } => pentool::collab::log_page(&input, offset, limit, actor.as_deref())?,
                CollabAction::Replay { input, out } => {
                    pentool::collab::replay_report(&input, out.as_deref())?
                }
                CollabAction::Verify { input } => pentool::collab::verify_report(&input)?,
                CollabAction::Token { action } => match action {
                    CollabTokenAction::Create {
                        input,
                        role,
                        name,
                        expires,
                    } => {
                        let (actor, _) = pentool::collab::current_actor()?;
                        pentool::collab::sync::token_create(
                            &input,
                            &role,
                            &name,
                            expires.as_deref(),
                            actor,
                        )?
                    }
                    CollabTokenAction::List { input } => pentool::collab::sync::token_list(&input)?,
                    CollabTokenAction::Revoke { input, id } => {
                        pentool::collab::sync::token_revoke(&input, &id)?
                    }
                    CollabTokenAction::SetRole { input, id, role } => {
                        pentool::collab::sync::token_set_role(&input, &id, &role)?
                    }
                },
                CollabAction::Peer { action } => match action {
                    CollabPeerAction::Add { input, url, name } => pentool::collab::sync::peer_add(
                        &input,
                        &url,
                        name.as_deref(),
                        pentool::collab::sync::Limits::default().max_peers,
                    )?,
                    CollabPeerAction::List { input } => pentool::collab::sync::peer_list(&input)?,
                    CollabPeerAction::Remove { input, peer } => {
                        pentool::collab::sync::peer_remove(&input, &peer)?
                    }
                },
                CollabAction::Sync {
                    input,
                    peer,
                    token,
                    allow_insecure_lan,
                } => {
                    tokio::task::spawn_blocking(move || {
                        pentool::collab::sync::sync_with_peer(
                            &input,
                            &peer,
                            &collab_token(token)?,
                            allow_insecure_lan,
                        )
                    })
                    .await??
                }
                CollabAction::Presence {
                    input,
                    peer,
                    token,
                    allow_insecure_lan,
                    json,
                } => {
                    let snapshot = tokio::task::spawn_blocking(move || {
                        pentool::collab::sync::peer_presence(
                            &input,
                            &peer,
                            &collab_token(token)?,
                            allow_insecure_lan,
                        )
                    })
                    .await??;
                    if !json {
                        let text = |v: &serde_json::Value| v.as_str().unwrap_or("-").to_string();
                        println!("{} connected", snapshot["matches"]);
                        for entry in snapshot["entries"].as_array().into_iter().flatten() {
                            println!(
                                "{}\t{}\t{}\tpage={}\tselected={}",
                                text(&entry["actor"]["id"]),
                                text(&entry["actor"]["kind"]),
                                text(&entry["actor"]["display_name"]),
                                text(&entry["page"]),
                                entry["selection"].as_array().map_or(0, |s| s.len()),
                            );
                        }
                        return Ok(());
                    }
                    snapshot
                }
                CollabAction::Join {
                    input,
                    peer,
                    name,
                    token,
                    allow_insecure_lan,
                } => {
                    tokio::task::spawn_blocking(move || {
                        pentool::collab::sync::join(
                            &input,
                            &peer,
                            name.as_deref(),
                            &collab_token(token)?,
                            allow_insecure_lan,
                        )
                    })
                    .await??
                }
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
            report,
            scale,
            outline_text,
            all_pages,
            format,
            link_images,
            layers,
            renderer,
        } => {
            let renderer = match renderer.as_str() {
                "reference" => pentool::scene3d::node::RendererPreference::Reference,
                "accelerated" => pentool::scene3d::node::RendererPreference::Accelerated,
                other => anyhow::bail!(
                    "[invalid-input] renderer `{other}` must be reference or accelerated"
                ),
            };
            let _renderer = pentool::scene3d::node::select_renderer(renderer);
            if all_pages && selected_page.is_some() {
                anyhow::bail!("[invalid-input] --page cannot be combined with --all-pages")
            }
            if !scale.is_finite() || !(0.1..=8.0).contains(&scale) {
                anyhow::bail!("export scale must be finite and between 0.1 and 8")
            }
            if report
                .as_ref()
                .is_some_and(|path| path == &input || path == &output)
            {
                anyhow::bail!(
                    "[invalid-input] input, export output, and fidelity report paths must be distinct"
                );
            }
            if link_images
                && (all_pages || output.extension().and_then(|e| e.to_str()) != Some("svg"))
            {
                anyhow::bail!(
                    "[unsupported-capability] --link-images applies to a single-page .svg output"
                )
            }
            let wants_pdf = output
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("pdf") || extension.eq_ignore_ascii_case("ai")
                });
            let wants_psd = output
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("psd"));
            if layers && !wants_psd {
                bail!("[unsupported-capability] --layers applies only to .psd output");
            }
            if wants_psd {
                if all_pages {
                    anyhow::bail!(
                        "[unsupported-capability] PSD export writes one selected page; omit --all-pages"
                    )
                }
                if outline_text {
                    anyhow::bail!(
                        "[unsupported-capability] PSD export is flattened; --outline-text applies only to SVG"
                    )
                }
                if link_images {
                    anyhow::bail!(
                        "[unsupported-capability] PSD export is flattened; --link-images applies only to SVG"
                    )
                }
                let raw: serde_json::Value = serde_json::from_slice(
                    &pentool::resource::read_bounded(&input, 512 * 1024 * 1024)?,
                )?;
                if layers
                    && (composite::is_document(&raw)
                        || raw
                            .get("version")
                            .and_then(serde_json::Value::as_u64)
                            .is_some_and(|version| version >= image::VERSION))
                {
                    anyhow::bail!(
                        "[unsupported-capability] --layers currently supports scene and legacy documents; this document uses a composite image pipeline"
                    )
                }
                let png = if composite::is_document(&raw) {
                    composite::png(&raw, &input, selected_page, scale)?
                } else if raw
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|version| version >= image::VERSION)
                    && raw
                        .get("image_assets")
                        .and_then(serde_json::Value::as_object)
                        .is_some_and(|assets| !assets.is_empty())
                {
                    let scene = image::to_svg(&raw, &input, selected_page)?;
                    render::scene_to_png(&scene, scale)?
                } else {
                    let document = read_render_document(&input, selected_page)?;
                    render::to_png(&document, scale)?
                };
                let raster = ::image::load_from_memory(&png)
                    .context("could not decode the rendered PSD composite")?
                    .to_rgba8();
                let psd = if layers {
                    let document = read_render_document(&input, selected_page)?;
                    let pixel_bytes = usize::try_from(raster.width())
                        .ok()
                        .and_then(|width| {
                            usize::try_from(raster.height())
                                .ok()
                                .and_then(|height| width.checked_mul(height))
                        })
                        .and_then(|pixels| pixels.checked_mul(4))
                        .context("[limit-exceeded] PSD layer pixel size overflow")?;
                    let projected = pixel_bytes
                        .checked_mul(
                            document
                                .layers
                                .len()
                                .checked_add(1)
                                .context("[limit-exceeded] PSD layer count overflow")?,
                        )
                        .and_then(|bytes| {
                            bytes.checked_add(document.layers.len().checked_mul(400)?)
                        })
                        .context("[limit-exceeded] PSD layer output size overflow")?;
                    if document.layers.len() > 2_000 || projected > 256 * 1024 * 1024 {
                        bail!(
                            "[limit-exceeded] layered PSD output would exceed the 256 MiB or 2000-layer limit"
                        );
                    }
                    let mut rendered_layers = Vec::with_capacity(document.layers.len());
                    for (index, source_layer) in document.layers.iter().enumerate() {
                        let mut isolated = document.clone();
                        isolated.canvas.background = "none".into();
                        for (layer_index, layer) in isolated.layers.iter_mut().enumerate() {
                            layer.visible = layer_index == index;
                        }
                        let png = render::to_png(&isolated, scale)?;
                        let layer_raster = ::image::load_from_memory(&png)
                            .context("could not decode isolated PSD layer raster")?
                            .to_rgba8();
                        if layer_raster.dimensions() != raster.dimensions() {
                            bail!(
                                "[invalid-state] isolated PSD layer dimensions differ from composite"
                            );
                        }
                        rendered_layers.push((
                            source_layer.name.clone(),
                            source_layer.visible,
                            layer_raster.into_raw(),
                        ));
                    }
                    let layer_refs = rendered_layers
                        .iter()
                        .rev()
                        .map(|(name, visible, rgba)| pentool::psd_export::RasterLayer {
                            name,
                            visible: *visible,
                            rgba,
                        })
                        .collect::<Vec<_>>();
                    pentool::psd_export::encode_layers(
                        raster.width(),
                        raster.height(),
                        &layer_refs,
                        raster.as_raw(),
                    )?
                } else {
                    pentool::psd_export::encode_rgba(
                        raster.width(),
                        raster.height(),
                        raster.as_raw(),
                    )?
                };
                editing::atomic_write(&output, &psd)?;
            } else if all_pages || wants_pdf {
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
                    || raw
                        .get("version")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|version| version >= image::VERSION)
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
                    || raw
                        .get("version")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|version| version >= image::VERSION)
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
            let source_bytes = pentool::resource::read_bounded(&input, 512 * 1024 * 1024)?;
            let source_document: serde_json::Value = serde_json::from_slice(&source_bytes)
                .context("export source is not a valid Pentool document")?;
            let export_format = if wants_pdf {
                "pdf".to_owned()
            } else if all_pages {
                format.as_deref().unwrap_or("png").to_ascii_lowercase()
            } else {
                output
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("unknown")
                    .to_ascii_lowercase()
            };
            let report_page_ids = if all_pages || export_format == "pdf" {
                None
            } else {
                source_document
                    .get("pages")
                    .and_then(serde_json::Value::as_array)
                    .map(|pages| {
                        selected_page
                            .or_else(|| {
                                pages.first()?.get("id").and_then(serde_json::Value::as_str)
                            })
                            .context("export source page has no ID")
                            .map(|page_id| vec![page_id.to_owned()])
                    })
                    .transpose()?
            };
            let rasterized_export = export_format == "png"
                || export_format == "psd"
                || (matches!(export_format.as_str(), "svg" | "pdf")
                    && ((composite::is_document(&source_document)
                        && !(export_format == "svg" && has_shape_gradient(&source_document)))
                        || export_format == "pdf"
                            && source_document
                                .get("version")
                                .and_then(serde_json::Value::as_u64)
                                .is_some_and(|version| version >= image::VERSION)
                            && source_document
                                .get("image_assets")
                                .and_then(serde_json::Value::as_object)
                                .is_some_and(|assets| !assets.is_empty())));
            let fidelity = pentool::interchange::export_fidelity_report_for(
                &source_document,
                &export_format,
                &input.display().to_string(),
                report_page_ids.as_deref(),
                rasterized_export,
            )?;
            if let Some(report_path) = &report {
                editing::atomic_write(report_path, &serde_json::to_vec_pretty(&fidelity)?)
                    .with_context(|| format!("could not write {}", report_path.display()))?;
            }
            let mut fidelity_summary = serde_json::to_value(&fidelity)?;
            if report.is_none() {
                if let Some(object) = fidelity_summary.as_object_mut() {
                    object.insert(
                        "items_omitted".into(),
                        serde_json::json!(fidelity.items.len() + fidelity.items_omitted),
                    );
                    object.insert("items".into(), serde_json::json!([]));
                    object.insert(
                        "report_hint".into(),
                        serde_json::json!("pass --report PATH to write detailed fidelity items"),
                    );
                }
            } else if let Some(object) = fidelity_summary.as_object_mut() {
                object.remove("items");
                object.insert(
                    "report_path".into(),
                    serde_json::json!(report.as_ref().unwrap().display().to_string()),
                );
            }
            println!(
                "{}",
                serde_json::json!({"exported":output.display().to_string(),"fidelity":fidelity_summary})
            );
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
    let vector_gradient_svg = output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
        && has_shape_gradient(raw);
    if composite::is_document(raw) && !vector_gradient_svg {
        if outline_text || link_images {
            anyhow::bail!(
                "[unsupported-capability] v6 composite exports require embedded pixels; omit --outline-text and --link-images"
            )
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

fn has_shape_gradient(raw: &serde_json::Value) -> bool {
    fn paint_is_gradient(node: &serde_json::Value, slot: &str) -> bool {
        node.pointer(&format!("/style/{slot}/fallback"))
            .is_some_and(|paint| {
                paint
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| matches!(kind, "linear" | "radial"))
                    && paint.get("stops").is_some_and(serde_json::Value::is_array)
            })
    }
    fn visit_node(node: &serde_json::Value) -> bool {
        let supported_shape = node
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| matches!(kind, "rect" | "ellipse" | "path" | "line"));
        (supported_shape && (paint_is_gradient(node, "fill") || paint_is_gradient(node, "stroke")))
            || node
                .get("children")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|children| children.iter().any(visit_node))
            || node.get("fallback").is_some_and(visit_node)
    }
    raw.get("pages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|page| page.get("layers").and_then(serde_json::Value::as_array))
        .flatten()
        .flat_map(|layer| layer.get("nodes").and_then(serde_json::Value::as_array))
        .flatten()
        .any(visit_node)
}

fn ensure_v8_production(raw: &serde_json::Value) -> Result<()> {
    if !pentool::color::has_v8(raw) {
        bail!(
            "[unsupported-version] production geometry requires a version {} document; run `migrate` first",
            pentool::color::VERSION
        );
    }
    Ok(())
}

fn read_color_document(path: &Path, expected: Option<&str>) -> Result<(serde_json::Value, String)> {
    let bytes =
        pentool::resource::read_bounded(path, pentool::color::catalog::MAX_CATALOG_DOCUMENT_BYTES)?;
    let revision = transaction::revision(&bytes);
    if expected.is_some_and(|expected| expected != revision) {
        bail!("revision mismatch: expected {expected:?}, current {revision}");
    }
    Ok((
        serde_json::from_slice(&bytes).context("invalid .pen document")?,
        revision,
    ))
}

fn read_document(path: &PathBuf, page: Option<&str>) -> Result<Document> {
    let doc = read_document_unvalidated(path, page)?;
    doc.validate().map_err(anyhow::Error::msg)?;
    Ok(doc)
}

fn read_document_unvalidated(path: &PathBuf, page: Option<&str>) -> Result<Document> {
    editing::recover_interrupted_write(path)?;
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let mut doc: Document = serde_json::from_slice(&bytes).context("invalid .pen document")?;
    select_page(&mut doc, page)?;
    Ok(doc)
}

/// RFC 7386 JSON merge-patch: objects merge recursively, null removes a key.
fn merge_patch(target: &mut serde_json::Value, patch: &serde_json::Value) {
    match patch {
        serde_json::Value::Object(patch) => {
            if !target.is_object() {
                *target = serde_json::json!({});
            }
            let object = target.as_object_mut().unwrap();
            for (key, value) in patch {
                if value.is_null() {
                    object.remove(key);
                } else {
                    merge_patch(
                        object.entry(key.clone()).or_insert(serde_json::Value::Null),
                        value,
                    );
                }
            }
        }
        other => *target = other.clone(),
    }
}

fn read_render_document(path: &PathBuf, page: Option<&str>) -> Result<Document> {
    editing::recover_interrupted_write(path)?;
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

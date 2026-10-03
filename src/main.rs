use pentool::{
    agent, asset, benchmark, diff, document, editing, fonts, geometry, history, import, instance,
    layout, library, package, page, pdf, render, replace, scene, server, style, text, transaction,
};

use anyhow::{Context, Result};
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

#[derive(Subcommand)]
enum Command {
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
        kind: Option<agent::ObjectKind>,
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
        kind: Option<agent::ObjectKind>,
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
    },
    /// Print document metadata as JSON.
    Info { input: PathBuf },
    /// Explicitly migrate a legacy document to the ordered v4 scene graph.
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
    /// Render a .pen document to PNG or SVG (selected by extension).
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
        id: String,
        #[arg(long)]
        source: PathBuf,
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

fn main() -> Result<()> {
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
                    "error":{"code":error_code(&error),"message":error.to_string(),"context":format!("{error:#}"),"suggestions":error_suggestions(&error),"hint":"Run the command with --help and verify page, layer, group, and object IDs."}
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

#[tokio::main]
async fn run() -> Result<()> {
    let cli = match Cli::try_parse() {
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
            let chosen_prefix = prefix.unwrap_or_else(|| {
                format!(
                    "{}-{}",
                    item.id.replace('/', "-"),
                    &item.content_hash[7..15]
                )
            });
            let mut result = import::compose(
                serde_json::from_slice(&destination_bytes)?,
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
                let id = next_instance_id(&result.document);
                instance::attach(
                    &mut result.document,
                    instance::InstanceRecord {
                        id: id.clone(),
                        library: item.library.clone(),
                        asset_id: item.id.clone(),
                        asset_version: item.version.clone(),
                        content_hash: item.content_hash.clone(),
                        layer_ids,
                        page_id: result.summary["destination_page"]
                            .as_str()
                            .unwrap_or("page-1")
                            .to_owned(),
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
                dry_run,
            } => {
                println!(
                    "{}",
                    json_pretty(instance::update(&input, &id, &source, dry_run)?)?
                );
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
            json: _,
        } => {
            let value = if render_benchmark || paths_only || scale != 1.0 {
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
            if let Ok(raw) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if raw.get("version").and_then(serde_json::Value::as_u64) == Some(4) {
                    let k = kind.map(|k| match k {
                        agent::ObjectKind::Path => "path",
                        agent::ObjectKind::Text => "text",
                    });
                    println!("{}", serde_json::to_string_pretty(&scene::inspect_paginated_v4(&raw, selected_page, query.as_deref(), k, layer.as_deref(), offset, limit)?)?);
                    return Ok(());
                }
            }
            let doc = read_document(&input, selected_page)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&agent::inspect_paginated(
                    &doc,
                    query.as_deref(),
                    kind,
                    layer.as_deref(),
                    offset,
                    limit
                )?)?
            );
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
            if let Ok(raw) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if raw.get("version").and_then(serde_json::Value::as_u64) == Some(4) {
                    let k = kind.map(|k| match k {
                        agent::ObjectKind::Path => "path",
                        agent::ObjectKind::Text => "text",
                    });
                    println!("{}", serde_json::to_string_pretty(&scene::inspect_paginated_v4(&raw, selected_page, Some(&query), k, layer.as_deref(), offset, limit)?)?);
                    return Ok(());
                }
            }
            let doc = read_document(&input, selected_page)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&agent::inspect_paginated(
                    &doc,
                    Some(&query),
                    kind,
                    layer.as_deref(),
                    offset,
                    limit
                )?)?
            );
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
            let mut doc: Document = serde_json::from_value(raw.clone())?;
            select_page(&mut doc, selected_page)?;
            let result = agent::apply(&mut doc, &action)?;
            agent::merge_document(&mut raw, &doc, std::slice::from_ref(&action))?;
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
        } => {
            let bytes =
                fs::read(&input).with_context(|| format!("could not read {}", input.display()))?;
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            let operation_bytes = fs::read(&operations)
                .with_context(|| format!("could not read {}", operations.display()))?;
            let changes = if raw.get("version").and_then(serde_json::Value::as_u64)
                == Some(scene::VERSION)
            {
                let actions: Vec<serde_json::Value> = serde_json::from_slice(&operation_bytes)?;
                scene::apply_batch(&mut raw, selected_page, &actions)?
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
            if matches!(action, page::PageAction::List) {
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
            let result = import::compose(
                serde_json::from_slice(&destination_bytes)?,
                serde_json::from_slice(&fs::read(&source)?)?,
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
        } => editing::edit_page_options(
            &input,
            selected_page,
            "text",
            dry_run,
            if_revision.as_deref(),
            |doc| text::apply(doc, action),
        ),
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
        } => editing::edit_page_options(
            &input,
            selected_page,
            "layer-geometry",
            dry_run,
            if_revision.as_deref(),
            |doc| {
                geometry::execute_layer(doc, &layer, &operation)?;
                Ok(())
            },
        ),
        Command::Geometry {
            input,
            dry_run,
            if_revision,
            layer,
            id,
            operation,
        } => {
            if operation.is_query() {
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
        } => editing::edit_page_options(
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
        ),
        Command::Layer {
            input,
            action,
            dry_run,
            if_revision,
        } => editing::edit_page_options(
            &input,
            selected_page,
            "layer",
            dry_run,
            if_revision.as_deref(),
            |doc| editing::layer(doc, action),
        ),
        Command::Path {
            input,
            action,
            dry_run,
            if_revision,
        } => editing::edit_page_options(
            &input,
            selected_page,
            "path",
            dry_run,
            if_revision.as_deref(),
            |doc| editing::path(doc, action),
        ),
        Command::New {
            output,
            width,
            height,
        } => {
            let doc = Document::new(width, height);
            doc.validate().map_err(anyhow::Error::msg)?;
            fs::write(&output, serde_json::to_vec_pretty(&doc)?)
                .with_context(|| format!("could not write {}", output.display()))?;
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
            if !matches!(target, 3 | 4) {
                anyhow::bail!("migration target must be 3 or 4")
            }
            let before =
                fs::read(&input).with_context(|| format!("could not read {}", input.display()))?;
            let raw: serde_json::Value =
                serde_json::from_slice(&before).context("invalid .pen document")?;
            let from = raw.get("version").and_then(serde_json::Value::as_u64);
            let migrated = if target == 4 {
                scene::migrate_to_v4(raw)?
            } else {
                scene::flatten_to_v3(&raw)?
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
                render::write_export_options(
                    &read_render_document(&args.old, selected_page)?,
                    &old_png,
                    1.0,
                    false,
                )?;
                render::write_export_options(
                    &read_render_document(&args.new, selected_page)?,
                    &new_png,
                    1.0,
                    false,
                )?;
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
        } => {
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
                let docs = page_ids
                    .iter()
                    .map(|id| read_render_document(&input, Some(id)))
                    .collect::<Result<Vec<_>>>()?;
                if wants_pdf {
                    pdf::write(&docs, &output)?;
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
                    for (index, (id, doc)) in page_ids.iter().zip(&docs).enumerate() {
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
                        render::write_export_options(doc, &destination, scale, outline_text)?;
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
                let doc = read_render_document(&input, selected_page)?;
                render::write_export_options(&doc, &output, scale, outline_text)?;
            }
            println!("Exported {}", output.display());
            Ok(())
        }
    }
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
    let flattened = if raw.get("version").and_then(serde_json::Value::as_u64) == Some(4) {
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

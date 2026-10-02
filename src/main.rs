use pentool::{
    agent, asset, benchmark, document, editing, fonts, geometry, import, instance, library,
    package, page, render, server, text,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use document::Document;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(
    name = "pentool",
    version,
    about = "Draw vector paths, automate them, and serve a canvas"
)]
struct Cli {
    /// Select a page for page-aware commands (v3 documents).
    #[arg(long, global = true)]
    page: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
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
    },
    /// List, add, rename, duplicate, move, or remove pages.
    Page {
        input: PathBuf,
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
        #[arg(long)]
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
        #[command(subcommand)]
        action: agent::ObjectAction,
    },
    /// Apply a JSON array of object operations as one recoverable transaction.
    Batch {
        input: PathBuf,
        operations: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        revision: Option<String>,
    },
    /// Create, update, or remove editable text.
    Text {
        input: PathBuf,
        #[command(subcommand)]
        action: text::TextAction,
    },
    /// Embed a custom TTF/OTF font or remove an unused font.
    Font {
        input: PathBuf,
        #[command(subcommand)]
        action: text::FontAction,
    },
    /// List available bundled, embedded, and installed font faces as JSON.
    Fonts { input: Option<PathBuf> },
    /// Transform all shapes and fills in one layer together.
    LayerGeometry {
        input: PathBuf,
        #[arg(long)]
        layer: String,
        #[command(subcommand)]
        operation: geometry::Operation,
    },
    /// Shared Rust geometry operations. Coordinates are in document units.
    Geometry {
        input: PathBuf,
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
    /// Change canvas size, background, or document name.
    Canvas {
        input: PathBuf,
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
        #[command(subcommand)]
        action: editing::LayerAction,
    },
    /// Add, update, or remove an SVG-compatible vector path.
    Path {
        input: PathBuf,
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
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
        } => {
            let cwd = std::env::current_dir()?;
            let idx = all_indexes(&cwd)?;
            let item = library::resolve(&idx, &spec)?;
            let source_bytes = fs::read(&item.path)?;
            let source_raw: serde_json::Value = serde_json::from_slice(&source_bytes)?;
            let manifest =
                asset::manifest(&source_raw)?.context("indexed asset metadata missing")?;
            let destination_bytes = fs::read(&destination)?;
            let revision = agent::revision_bytes(&destination_bytes);
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
            let output = serde_json::to_vec_pretty(&result.document)?;
            if !dry_run {
                let backup = editing::transactional_write(&destination, &output)?;
                result.summary["backup"] = serde_json::to_value(backup)?;
            }
            println!(
                "{}",
                json_pretty(
                    serde_json::json!({"ok":true,"dry_run":dry_run,"revision_before":revision,"revision_after":agent::revision_bytes(&output),"summary":result.summary})
                )?
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
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&benchmark::run(layers, objects, png, max_ms)?)?
            );
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
        Command::Object { input, action } => {
            let bytes = fs::read(&input)?;
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            let mut doc: Document = serde_json::from_value(raw.clone())?;
            select_page(&mut doc, selected_page)?;
            let result = agent::apply(&mut doc, &action)?;
            agent::merge_document(&mut raw, &doc, std::slice::from_ref(&action))?;
            let output = serde_json::to_vec_pretty(&raw)?;
            let backup = editing::transactional_write(&input, &output)?;
            println!(
                "{}",
                serde_json::json!({"ok":true,"file":input,"backup":backup,"result":result})
            );
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
            let current_revision = agent::revision_bytes(&bytes);
            if let Some(expected) = revision {
                if expected != current_revision {
                    anyhow::bail!("revision mismatch: document changed on disk");
                }
            }
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            let mut doc: Document = serde_json::from_value(raw.clone())?;
            select_page(&mut doc, selected_page)?;
            let actions: Vec<agent::ObjectAction> = serde_json::from_slice(
                &fs::read(&operations)
                    .with_context(|| format!("could not read {}", operations.display()))?,
            )?;
            let changes = agent::apply_batch(&mut doc, &actions)?;
            agent::merge_document(&mut raw, &doc, &actions)?;
            let output = serde_json::to_vec_pretty(&raw)?;
            if dry_run {
                println!(
                    "{}",
                    serde_json::json!({"ok":true,"dry_run":true,"revision":current_revision,"changes":changes})
                );
            } else {
                let backup = editing::transactional_write(&input, &output)?;
                println!(
                    "{}",
                    serde_json::json!({"ok":true,"revision_before":current_revision,"revision_after":agent::revision_bytes(&output),"backup":backup,"changes":changes})
                );
            }
            Ok(())
        }
        Command::Serve { host, port, input } => server::serve(&host, port, input).await,
        Command::Page { input, action } => {
            if matches!(action, page::PageAction::List) {
                let doc = read_document(&input, None)?;
                println!("{}", serde_json::to_string_pretty(&page::list(&doc))?);
                Ok(())
            } else {
                editing::edit_page(&input, None, |doc| {
                    println!("{}", page::apply(doc, action)?);
                    Ok(())
                })
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
            let current_revision = agent::revision_bytes(&destination_bytes);
            if revision.as_deref().is_some_and(|r| r != current_revision) {
                anyhow::bail!("revision mismatch: destination changed on disk");
            }
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
            let output = serde_json::to_vec_pretty(&result.document)?;
            if dry_run {
                println!(
                    "{}",
                    serde_json::json!({"ok":true,"dry_run":true,"revision":current_revision,"summary":result.summary})
                );
            } else {
                let backup = editing::transactional_write(&destination, &output)?;
                println!(
                    "{}",
                    serde_json::json!({"ok":true,"revision_before":current_revision,"revision_after":agent::revision_bytes(&output),"backup":backup,"summary":result.summary})
                );
            }
            Ok(())
        }
        Command::Text { input, action } => {
            editing::edit_page(&input, selected_page, |doc| text::apply(doc, action))
        }
        Command::Font { input, action } => {
            editing::edit_page(&input, selected_page, |doc| text::font(doc, action))
        }
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
            layer,
            operation,
        } => editing::edit_page(&input, selected_page, |doc| {
            geometry::execute_layer(doc, &layer, &operation)?;
            Ok(())
        }),
        Command::Geometry {
            input,
            layer,
            id,
            operation,
        } => {
            if operation.is_query() {
                let mut doc = read_document(&input, selected_page)?;
                println!("{}", geometry::execute(&mut doc, &layer, &id, &operation)?);
                Ok(())
            } else {
                editing::edit_page(&input, selected_page, |doc| {
                    geometry::execute(doc, &layer, &id, &operation)?;
                    Ok(())
                })
            }
        }
        Command::Canvas {
            input,
            width,
            height,
            background,
            name,
        } => editing::edit_page(&input, selected_page, |doc| {
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
        }),
        Command::Layer { input, action } => {
            editing::edit_page(&input, selected_page, |doc| editing::layer(doc, action))
        }
        Command::Path { input, action } => {
            editing::edit_page(&input, selected_page, |doc| editing::path(doc, action))
        }
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
            let doc = read_document(&input, selected_page)?;
            println!("{}", serde_json::to_string_pretty(&doc)?);
            Ok(())
        }
        Command::Export {
            input,
            output,
            scale,
            outline_text,
        } => {
            let doc = read_document(&input, selected_page)?;
            render::write_export_options(&doc, &output, scale, outline_text)?;
            println!("Exported {}", output.display());
            Ok(())
        }
    }
}

fn read_document(path: &PathBuf, page: Option<&str>) -> Result<Document> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let mut doc: Document = serde_json::from_slice(&bytes).context("invalid .pen document")?;
    select_page(&mut doc, page)?;
    doc.validate().map_err(anyhow::Error::msg)?;
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

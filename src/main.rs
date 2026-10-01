use pentool::{
    agent, benchmark, document, editing, fonts, geometry, import, page, render, server, text,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use document::Document;
use std::{fs, path::PathBuf};

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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let selected_page = cli.page.as_deref();
    match cli.command.unwrap_or(Command::Serve {
        input: None,
        host: "127.0.0.1".into(),
        port: 4711,
    }) {
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

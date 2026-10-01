use pentool::{agent, document, editing, fonts, geometry, render, server, text};

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
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List or search layers and objects as compact JSON.
    Tree {
        input: PathBuf,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, value_enum)]
        kind: Option<agent::ObjectKind>,
        #[arg(long)]
        layer: Option<String>,
    },
    /// Search layers, object IDs, and text content as compact JSON.
    Search {
        input: PathBuf,
        query: String,
        #[arg(long, value_enum)]
        kind: Option<agent::ObjectKind>,
        #[arg(long)]
        layer: Option<String>,
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
    match cli.command.unwrap_or(Command::Serve {
        input: None,
        host: "127.0.0.1".into(),
        port: 4711,
    }) {
        Command::Tree {
            input,
            query,
            kind,
            layer,
        } => {
            let doc = read_document(&input)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&agent::inspect(
                    &doc,
                    query.as_deref(),
                    kind,
                    layer.as_deref()
                )?)?
            );
            Ok(())
        }
        Command::Search {
            input,
            query,
            kind,
            layer,
        } => {
            let doc = read_document(&input)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&agent::inspect(
                    &doc,
                    Some(&query),
                    kind,
                    layer.as_deref()
                )?)?
            );
            Ok(())
        }
        Command::Object { input, action } => {
            let bytes = fs::read(&input)?;
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
            let mut doc: Document = serde_json::from_value(raw.clone())?;
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
        Command::Text { input, action } => editing::edit(&input, |doc| text::apply(doc, action)),
        Command::Font { input, action } => editing::edit(&input, |doc| text::font(doc, action)),
        Command::Fonts { input } => {
            let doc = match input {
                Some(path) => read_document(&path)?,
                None => Document::new(1, 1),
            };
            println!("{}", fonts::list(&doc)?);
            Ok(())
        }
        Command::LayerGeometry {
            input,
            layer,
            operation,
        } => editing::edit(&input, |doc| {
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
                let mut doc = read_document(&input)?;
                println!("{}", geometry::execute(&mut doc, &layer, &id, &operation)?);
                Ok(())
            } else {
                editing::edit(&input, |doc| {
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
        } => editing::edit(&input, |doc| {
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
            editing::edit(&input, |doc| editing::layer(doc, action))
        }
        Command::Path { input, action } => editing::edit(&input, |doc| editing::path(doc, action)),
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
            let doc = read_document(&input)?;
            println!("{}", serde_json::to_string_pretty(&doc)?);
            Ok(())
        }
        Command::Export {
            input,
            output,
            scale,
            outline_text,
        } => {
            let doc = read_document(&input)?;
            render::write_export_options(&doc, &output, scale, outline_text)?;
            println!("Exported {}", output.display());
            Ok(())
        }
    }
}

fn read_document(path: &PathBuf) -> Result<Document> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let doc: Document = serde_json::from_slice(&bytes).context("invalid .pen document")?;
    doc.validate().map_err(anyhow::Error::msg)?;
    Ok(doc)
}

//! CLI definitions and dispatch for version 9 spreads, parents, sections and
//! overrides. The document edits live in `crate::spreads`; this module only
//! parses arguments, reads the document and commits through the shared
//! transaction path (`--dry-run` plans without writing).
use crate::spreads::{self, ParentSettings, SectionSettings};
use crate::transaction;
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde_json::{json, Map, Value};
use std::path::Path;

#[derive(Debug, Clone, Subcommand)]
pub enum SpreadAction {
    /// List spreads with their pages and facing state.
    List,
    /// Turn facing-page margins on or off for a spread.
    Facing {
        spread: String,
        /// true or false.
        facing: bool,
    },
    /// Group adjacent pages (in page order) into one spread.
    Merge {
        pages: Vec<String>,
        /// Mark the new spread as facing.
        #[arg(long)]
        facing: bool,
    },
    /// Split a spread into single-page spreads.
    Split { spread: String },
    /// Rebuild the standard book arrangement: first page alone, then pairs.
    Arrange {
        /// Keep spreads non-facing (single-sided margins).
        #[arg(long)]
        single: bool,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum ParentAction {
    /// List parents with their base, item count and the pages using them.
    List,
    /// Create a parent, optionally based on another.
    Create {
        id: String,
        #[arg(long)]
        based_on: Option<String>,
        #[command(flatten)]
        settings: ParentArgs,
    },
    /// Change a parent's settings or base.
    Set {
        id: String,
        #[arg(long, conflicts_with = "no_base")]
        based_on: Option<String>,
        /// Remove the base so the parent stands alone.
        #[arg(long)]
        no_base: bool,
        #[command(flatten)]
        settings: ParentArgs,
    },
    /// Remove an unused parent.
    Remove { id: String },
    /// Add a scene node (JSON object with an "id") to a parent.
    ItemAdd {
        parent: String,
        #[arg(long)]
        node: String,
        #[arg(long)]
        layer: Option<String>,
        /// Show the item only on this named parent page.
        #[arg(long)]
        parent_page: Option<String>,
    },
    /// Remove an item from a parent; overrides on pages are reported as orphaned.
    ItemRemove { parent: String, item: String },
    /// Assign a parent to a page (or --none).
    Assign {
        page: String,
        #[arg(long, conflicts_with = "none")]
        parent: Option<String>,
        #[arg(long)]
        none: bool,
    },
    /// Show what a page receives: margins, side, items and orphaned overrides.
    Resolve { page: String },
}

#[derive(Debug, Clone, clap::Args)]
pub struct ParentArgs {
    #[arg(long)]
    name: Option<String>,
    /// Margins as top,inner,bottom,outer (facing) or top,left,bottom,right.
    #[arg(long)]
    margins: Option<String>,
    #[arg(long)]
    columns: Option<u64>,
    #[arg(long)]
    gutter: Option<f64>,
    /// Comma-separated parent page names (left,right).
    #[arg(long)]
    pages: Option<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SectionAction {
    /// List sections and the computed number/label of every page (paginated).
    List {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Start a section at a page.
    Add {
        page: String,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        settings: SectionArgs,
    },
    /// Change a section's numbering.
    Set {
        id: String,
        #[command(flatten)]
        settings: SectionArgs,
    },
    /// Remove a section; its pages join the previous one.
    Remove { id: String },
}

#[derive(Debug, Clone, clap::Args)]
pub struct SectionArgs {
    #[arg(long)]
    start_number: Option<u64>,
    /// arabic, upper-roman, lower-roman, upper-alpha or lower-alpha.
    #[arg(long)]
    style: Option<String>,
    #[arg(long)]
    prefix: Option<String>,
    /// Continue numbering from the previous section.
    #[arg(long = "continue")]
    continue_numbering: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum OverrideAction {
    /// Override properties of a parent item on one page.
    Set {
        page: String,
        item: String,
        /// key=value; the value is JSON when it parses, otherwise text. Repeatable.
        #[arg(long = "set", required = true)]
        properties: Vec<String>,
    },
    /// Remove an override (all of it, or one --property).
    Remove {
        page: String,
        item: String,
        #[arg(long)]
        property: Option<String>,
    },
    /// Copy a parent item onto the page as an ordinary node and stop following it.
    Detach { page: String, item: String },
}

fn margins(text: &str) -> Result<[f64; 4]> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("[invalid-input] margins must be four numbers: {text:?}"))?;
    parts.try_into().map_err(|_| {
        anyhow::anyhow!("[invalid-input] margins need exactly four numbers: top,inner,bottom,outer")
    })
}

impl ParentArgs {
    fn settings(self) -> Result<ParentSettings> {
        Ok(ParentSettings {
            name: self.name,
            margins: self.margins.as_deref().map(margins).transpose()?,
            columns: self.columns,
            gutter: self.gutter,
            pages: self.pages.map(|p| {
                p.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }),
        })
    }
}

impl SectionArgs {
    fn settings(self) -> SectionSettings {
        SectionSettings {
            start_number: self.start_number,
            style: self.style,
            prefix: self.prefix,
            continue_numbering: self.continue_numbering.then_some(true),
        }
    }
}

fn read(input: &Path) -> Result<Value> {
    let bytes = std::fs::read(input)
        .with_context(|| format!("[missing-resource] could not read {}", input.display()))?;
    serde_json::from_slice(&bytes).context("[malformed-resource] invalid .pen document")
}

fn finish(
    input: &Path,
    label: &str,
    dry_run: bool,
    revision: Option<&str>,
    raw: &Value,
    mut result: Value,
) -> Result<()> {
    let change = transaction::commit_value(input, label, dry_run, revision, raw)?;
    if let Some(object) = result.as_object_mut() {
        object.insert("change".into(), serde_json::to_value(change)?);
    }
    println!("{result}");
    Ok(())
}

pub fn run_spread(
    input: &Path,
    dry_run: bool,
    revision: Option<&str>,
    action: SpreadAction,
) -> Result<()> {
    let mut raw = read(input)?;
    let result = match action {
        SpreadAction::List => {
            if !spreads::is_v9(&raw) {
                bail!("[unsupported-version] spreads need a version 9 document; run `pentool migrate --target 9` first");
            }
            println!("{}", spreads::spread_list(&raw));
            return Ok(());
        }
        SpreadAction::Facing { spread, facing } => {
            spreads::spread_set_facing(&mut raw, &spread, facing)?
        }
        SpreadAction::Merge { pages, facing } => {
            spreads::spread_merge(&mut raw, &pages, facing.then_some(true))?
        }
        SpreadAction::Split { spread } => spreads::spread_split(&mut raw, &spread)?,
        SpreadAction::Arrange { single } => spreads::spread_arrange(&mut raw, !single)?,
    };
    finish(input, "spread", dry_run, revision, &raw, result)
}

pub fn run_layout_style(
    input: &Path,
    dry_run: bool,
    revision: Option<&str>,
    action: crate::styles::StyleAction,
) -> Result<()> {
    let mut raw = read(input)?;
    let (result, read_only) = crate::styles::run_style(&mut raw, action)?;
    if read_only {
        println!("{result}");
        return Ok(());
    }
    // Reflow bounds and story validity are enforced by the shared validators.
    spreads::validate(&raw, spreads::VERSION)?;
    finish(input, "layout-style", dry_run, revision, &raw, result)
}

pub fn run_parent(
    input: &Path,
    dry_run: bool,
    revision: Option<&str>,
    action: ParentAction,
) -> Result<()> {
    let mut raw = read(input)?;
    let result = match action {
        ParentAction::List => {
            println!("{}", spreads::parent_list(&raw));
            return Ok(());
        }
        ParentAction::Resolve { page } => {
            println!("{}", spreads::resolve_page(&raw, &page)?);
            return Ok(());
        }
        ParentAction::Create {
            id,
            based_on,
            settings,
        } => spreads::parent_create(&mut raw, &id, based_on.as_deref(), settings.settings()?)?,
        ParentAction::Set {
            id,
            based_on,
            no_base,
            settings,
        } => {
            let base = if no_base {
                Some(None)
            } else {
                based_on.as_deref().map(Some)
            };
            spreads::parent_set(&mut raw, &id, base, settings.settings()?)?
        }
        ParentAction::Remove { id } => spreads::parent_remove(&mut raw, &id)?,
        ParentAction::ItemAdd {
            parent,
            node,
            layer,
            parent_page,
        } => {
            let node: Value = serde_json::from_str(&node)
                .context("[invalid-input] --node must be a JSON object")?;
            if !node.is_object() {
                bail!("[invalid-input] --node must be a JSON object");
            }
            spreads::parent_item_add(
                &mut raw,
                &parent,
                node,
                layer.as_deref(),
                parent_page.as_deref(),
            )?
        }
        ParentAction::ItemRemove { parent, item } => {
            spreads::parent_item_remove(&mut raw, &parent, &item)?
        }
        ParentAction::Assign { page, parent, none } => {
            if parent.is_none() && !none {
                bail!("[invalid-input] choose --parent <id> or --none");
            }
            spreads::page_set_parent(&mut raw, &page, parent.as_deref())?
        }
    };
    finish(input, "parent", dry_run, revision, &raw, result)
}

pub fn run_section(
    input: &Path,
    dry_run: bool,
    revision: Option<&str>,
    action: SectionAction,
) -> Result<()> {
    let mut raw = read(input)?;
    let result = match action {
        SectionAction::List { offset, limit } => {
            if !spreads::is_v9(&raw) {
                bail!("[unsupported-version] sections need a version 9 document; run `pentool migrate --target 9` first");
            }
            if limit == 0 || limit > 1000 {
                bail!("[invalid-input] --limit must be 1-1000");
            }
            println!("{}", spreads::section_list(&raw, offset, limit));
            return Ok(());
        }
        SectionAction::Add { page, id, settings } => {
            spreads::section_add(&mut raw, &page, id.as_deref(), settings.settings())?
        }
        SectionAction::Set { id, settings } => {
            spreads::section_set(&mut raw, &id, settings.settings())?
        }
        SectionAction::Remove { id } => spreads::section_remove(&mut raw, &id)?,
    };
    finish(input, "section", dry_run, revision, &raw, result)
}

pub fn run_override(
    input: &Path,
    dry_run: bool,
    revision: Option<&str>,
    action: OverrideAction,
) -> Result<()> {
    let mut raw = read(input)?;
    let result = match action {
        OverrideAction::Set {
            page,
            item,
            properties,
        } => {
            let mut map = Map::new();
            for pair in properties {
                let (key, value) = pair.split_once('=').with_context(|| {
                    format!("[invalid-input] --set expects key=value, got {pair:?}")
                })?;
                let value = serde_json::from_str(value).unwrap_or_else(|_| json!(value));
                map.insert(key.to_string(), value);
            }
            spreads::override_set(&mut raw, &page, &item, map)?
        }
        OverrideAction::Remove {
            page,
            item,
            property,
        } => spreads::override_remove(&mut raw, &page, &item, property.as_deref())?,
        OverrideAction::Detach { page, item } => spreads::detach(&mut raw, &page, &item)?,
    };
    finish(input, "override", dry_run, revision, &raw, result)
}

/// `text story ...` and `text frame ...`: the story/frame commands share the
/// `text` command word. Read-only listings print without touching the file.
pub fn run_text_layout(
    input: &Path,
    dry_run: bool,
    revision: Option<&str>,
    mut raw: Value,
    action: crate::text::TextAction,
) -> Result<()> {
    use crate::stories::{FrameAction, StoryAction};
    use crate::text::TextAction;
    let (result, read_only) = match action {
        TextAction::Story { action } => {
            let read_only = matches!(action, StoryAction::List { .. } | StoryAction::Show { .. });
            (crate::stories::run_story(&mut raw, action)?, read_only)
        }
        TextAction::Frame { action } => {
            let read_only = matches!(action, FrameAction::List { .. });
            (crate::stories::run_frame(&mut raw, action)?, read_only)
        }
        _ => bail!("[invalid-input] not a story or frame command"),
    };
    if read_only {
        println!("{result}");
        return Ok(());
    }
    finish(input, "text", dry_run, revision, &raw, result)
}

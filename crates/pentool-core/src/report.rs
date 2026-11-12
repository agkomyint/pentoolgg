//! `pentool report`: build a readable, cited, charted `.pen` report from a
//! declarative JSON spec and CSV/TSV/JSON data files.
//!
//! The spec lists `sources` (publisher, URL, date, how much to trust it, and an
//! optional data file) and pages of `blocks` (headings, text, stat tiles, bar,
//! column and segmented charts, tables, callouts). Blocks cite sources by id;
//! citations are numbered `[n]` in order of first use and an auto-built Sources
//! page lists them. Layout flows top to bottom and starts a new page when a
//! block does not fit. All nodes are built in memory and validated once.
use crate::merge::{parse_records, Records, MAX_DATA_BYTES};
use crate::scene;
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const MAX_SPEC_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SOURCES: usize = 500;
pub const MAX_BLOCKS: usize = 2000;
pub const MAX_POINTS: usize = 60;
pub const MAX_TABLE_ROWS: usize = 40;
pub const MAX_PAGES: usize = 200;

#[derive(Debug, Clone, Subcommand)]
pub enum ReportAction {
    /// Build a report document from a spec. Writes a new `.pen`.
    Build {
        /// Report spec (JSON). Data files are resolved next to it.
        spec: PathBuf,
        /// Output document.
        #[arg(long)]
        out: PathBuf,
        /// Replace an existing output (recoverable with `pentool undo`).
        #[arg(long)]
        overwrite: bool,
        /// Plan and report without writing.
        #[arg(long)]
        dry_run: bool,
        /// Fail when `check` would warn (uncited numbers, unused sources).
        #[arg(long)]
        strict: bool,
    },
    /// Validate a spec and its data without writing: unknown source ids,
    /// missing columns, uncited numbers, unused sources.
    Check {
        spec: PathBuf,
        /// Exit non-zero on warnings as well as errors.
        #[arg(long)]
        strict: bool,
    },
    /// Inspect a CSV/TSV/JSON data file: fields, numeric columns, first rows.
    Data {
        file: PathBuf,
        /// csv, tsv or json (default: from the extension).
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        delimiter: Option<char>,
        /// Rows to show.
        #[arg(long, default_value_t = 5)]
        rows: usize,
    },
    /// Print a small runnable example spec.
    Example,
}

// ------------------------------------------------------------------ spec

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub page: PageSize,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub sources: Vec<Source>,
    pub pages: Vec<PageSpec>,
    /// Append the Sources page (default true).
    #[serde(default = "yes")]
    pub sources_page: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageSize {
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
}
impl Default for PageSize {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
        }
    }
}
fn default_width() -> u32 {
    1200
}
fn default_height() -> u32 {
    1600
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Theme {
    pub accent: String,
    pub ink: String,
    pub muted: String,
    pub background: String,
    pub card: String,
    pub font: String,
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: "#2563eb".into(),
            ink: "#111827".into(),
            muted: "#6b7280".into(),
            background: "#ffffff".into(),
            card: "#f3f4f6".into(),
            font: "Atkinson Hyperlegible".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Observed,
    Survey,
    Forecast,
    #[default]
    Other,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Observed => "OBSERVED",
            Kind::Survey => "SURVEY",
            Kind::Forecast => "FORECAST",
            Kind::Other => "SOURCE",
        }
    }
    fn color(self) -> &'static str {
        match self {
            Kind::Observed => "#15803d",
            Kind::Survey => "#2563eb",
            Kind::Forecast => "#b45309",
            Kind::Other => "#4b5563",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub note: Option<String>,
    /// Data file (csv, tsv or json), relative to the spec, no `..`.
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub delimiter: Option<char>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageSpec {
    #[serde(default)]
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub label: String,
    pub value: f64,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Stat {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub source: Option<String>,
}

/// Columns, rows and number formatting shared by the data blocks.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Series {
    /// Source id: cited, and its data file is the data unless `values` is set.
    #[serde(default)]
    pub source: Option<String>,
    /// Extra sources to cite.
    #[serde(default)]
    pub cite: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Label column (default: first column).
    #[serde(default)]
    pub label: Option<String>,
    /// Value column (default: first numeric column after the label).
    #[serde(default)]
    pub value: Option<String>,
    /// Inline points instead of a data file.
    #[serde(default)]
    pub values: Option<Vec<Point>>,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub decimals: Option<usize>,
    /// desc, asc or none (default: none = file order).
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Heading {
        text: String,
        #[serde(default)]
        cite: Vec<String>,
    },
    Text {
        text: String,
        #[serde(default)]
        cite: Vec<String>,
        #[serde(default)]
        size: Option<f64>,
    },
    Stats {
        items: Vec<Stat>,
        #[serde(default)]
        cite: Vec<String>,
    },
    Bar(Series),
    Column(Series),
    Segmented(Series),
    Table(TableSeries),
    Callout {
        text: String,
        #[serde(default)]
        cite: Vec<String>,
    },
    Spacer {
        height: f64,
    },
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct TableSeries {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub cite: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Columns to show (default: all, at most 6).
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

// ------------------------------------------------------------------ data

/// Numbers in CSV cells: `$1,234.5`, `12%`, `3.4B` style suffixes are not
/// guessed; only `$`, `,`, `%` and spaces are stripped.
pub fn parse_number(cell: &str) -> Option<f64> {
    let cleaned: String = cell
        .trim()
        .chars()
        .filter(|c| !matches!(c, '$' | ',' | '%' | ' ' | '_'))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn numeric_columns(records: &Records) -> Vec<usize> {
    (0..records.fields.len())
        .filter(|&c| {
            !records.rows.is_empty()
                && records
                    .rows
                    .iter()
                    .all(|row| row.get(c).is_some_and(|cell| parse_number(cell).is_some()))
        })
        .collect()
}

fn safe_relative(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    ensure!(
        !path.is_empty()
            && p.is_relative()
            && p.components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "[invalid-input] data path {path:?} must be relative to the spec and may not use `..`"
    );
    Ok(p.to_path_buf())
}

fn load_records(dir: &Path, source: &Source) -> Result<Option<Records>> {
    let Some(file) = &source.data else {
        return Ok(None);
    };
    let path = dir.join(safe_relative(file)?);
    let format = crate::merge::data_format(&path, source.format.as_deref())?;
    let bytes = crate::resource::read_bounded(&path, MAX_DATA_BYTES).with_context(|| {
        format!(
            "[missing-resource] source {:?}: cannot read {file}",
            source.id
        )
    })?;
    parse_records(&bytes, &format, source.delimiter)
        .with_context(|| format!("source {:?} data {file}", source.id))
        .map(Some)
}

// ------------------------------------------------------------------ planning

const SERIES_COLORS: [&str; 8] = [
    "#2563eb", "#0f766e", "#d97706", "#7c3aed", "#dc2626", "#0891b2", "#65a30d", "#be185d",
];
const MARGIN: f64 = 64.0;
const GAP: f64 = 16.0;

struct Builder<'a> {
    spec: &'a Spec,
    data: BTreeMap<String, Records>,
    /// source id -> citation number, in order of first use.
    numbers: BTreeMap<String, usize>,
    order: Vec<String>,
    warnings: Vec<String>,
    pages: Vec<Vec<Value>>,
    titles: Vec<String>,
    cursor: f64,
    counter: usize,
    sources: BTreeMap<String, &'a Source>,
}

impl<'a> Builder<'a> {
    fn cw(&self) -> f64 {
        f64::from(self.spec.page.width) - 2.0 * MARGIN
    }
    fn bottom(&self) -> f64 {
        f64::from(self.spec.page.height) - MARGIN - 24.0
    }

    fn id(&mut self, kind: &str) -> String {
        self.counter += 1;
        format!("{kind}-{}", self.counter)
    }

    fn cite(&mut self, ids: &[String], context: &str) -> Result<String> {
        let mut marks = String::new();
        let mut seen = BTreeSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            ensure!(
                self.sources.contains_key(id),
                "[missing-resource] {context} cites unknown source {id:?}; declared sources: {}",
                self.sources.keys().cloned().collect::<Vec<_>>().join(", ")
            );
            let next = self.numbers.len() + 1;
            let number = *self.numbers.entry(id.clone()).or_insert_with(|| {
                self.order.push(id.clone());
                next
            });
            marks.push_str(&format!("[{number}]"));
        }
        Ok(marks)
    }

    fn new_page(&mut self, title: Option<&str>) {
        self.pages.push(Vec::new());
        self.titles
            .push(title.unwrap_or(&self.spec.title).to_string());
        self.cursor = MARGIN;
        if let Some(title) = title {
            let (x, size) = (MARGIN, 34.0);
            let id = self.id("page-title");
            let node = self.text_node(
                &id,
                title,
                x,
                self.cursor,
                size,
                700,
                &self.spec.theme.ink.clone(),
            );
            self.push(node);
            self.cursor += size * 1.4;
            let id = self.id("page-rule");
            let width = self.cw();
            let accent = self.spec.theme.accent.clone();
            let node = rect(&id, MARGIN, self.cursor, width, 3.0, 0.0, &accent);
            self.push(node);
            self.cursor += 3.0 + GAP * 1.5;
        }
    }

    fn push(&mut self, node: Value) {
        self.pages.last_mut().expect("page open").push(node);
    }

    #[allow(clippy::too_many_arguments)]
    /// A text node whose top edge is `top`; the stored `y` is the baseline.
    fn text_node(
        &self,
        id: &str,
        content: &str,
        x: f64,
        top: f64,
        size: f64,
        weight: u16,
        fill: &str,
    ) -> Value {
        json!({"kind":"text","id":id,"content":content,"source_content":content,
            "x":x,"y":top + size,"width":null,"height":null,
            "font_family":self.spec.theme.font,"font_size":size,"font_weight":weight,
            "italic":false,"align":"left","vertical_align":"top","line_height":1.35,
            "anchor":"baseline","overflow":"visible","style":{"fill":{"fallback":fill}}})
    }

    /// Reserve `height`, starting a continuation page when it does not fit.
    fn reserve(&mut self, height: f64) -> Result<f64> {
        if self.pages.is_empty() {
            self.new_page(None);
        }
        let room = self.bottom() - MARGIN;
        ensure!(
            height <= room,
            "[limit-exceeded] a block needs {height:.0}px but a page body is {room:.0}px; split it (use `limit`) or make the page taller"
        );
        if self.cursor + height > self.bottom() {
            ensure!(
                self.pages.len() < MAX_PAGES,
                "[limit-exceeded] the report would exceed {MAX_PAGES} pages"
            );
            let title = self.titles.last().cloned();
            let continued = title.map(|t| format!("{t} (cont.)"));
            self.new_page(continued.as_deref());
        }
        let top = self.cursor;
        self.cursor += height;
        Ok(top)
    }

    fn wrap(&self, content: &str, size: f64, width: f64) -> Vec<String> {
        wrap(content, size, width)
    }

    #[allow(clippy::too_many_arguments)]
    fn put_wrapped(
        &mut self,
        kind: &str,
        content: &str,
        x: f64,
        top: f64,
        size: f64,
        weight: u16,
        fill: &str,
        width: f64,
    ) -> f64 {
        let lines = self.wrap(content, size, width);
        let id = self.id(kind);
        let node = self.text_node(&id, &lines.join("\n"), x, top, size, weight, fill);
        self.push(node);
        lines.len() as f64 * size * 1.35
    }

    fn caption_for(&self, ids: &[String]) -> String {
        let mut parts = Vec::new();
        for id in ids {
            if let Some(source) = self.sources.get(id) {
                let who = source.publisher.as_deref().unwrap_or(&source.title);
                let when = source
                    .date
                    .as_deref()
                    .map(|d| format!(", {d}"))
                    .unwrap_or_default();
                parts.push(format!(
                    "{who}{when} ({})",
                    source.kind.label().to_lowercase()
                ));
            }
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("Source: {}", parts.join("; "))
        }
    }

    fn all_series_sources(series_source: &Option<String>, cite: &[String]) -> Vec<String> {
        let mut ids: Vec<String> = series_source.iter().cloned().collect();
        ids.extend(cite.iter().cloned());
        ids
    }

    fn series_points(&mut self, series: &Series, kind: &str) -> Result<Vec<Point>> {
        let mut points = if let Some(values) = &series.values {
            values.clone()
        } else {
            let id = series.source.as_deref().with_context(|| {
                format!("[invalid-input] {kind} needs `values` or a `source` with a data file")
            })?;
            let records = self.data.get(id).with_context(|| {
                format!(
                    "[missing-resource] {kind} reads source {id:?} but it declares no `data` file"
                )
            })?;
            let label_col = match &series.label {
                Some(name) => column(records, name, kind)?,
                None => 0,
            };
            let value_col = match &series.value {
                Some(name) => column(records, name, kind)?,
                None => *numeric_columns(records)
                    .iter()
                    .find(|&&c| c != label_col)
                    .with_context(|| {
                        format!("[invalid-input] {kind}: source {id:?} has no numeric column; name one with `value` (columns: {})", records.fields.join(", "))
                    })?,
            };
            let mut points = Vec::new();
            for (n, row) in records.rows.iter().enumerate() {
                let label = row.get(label_col).cloned().unwrap_or_default();
                let cell = row.get(value_col).map(String::as_str).unwrap_or("");
                let value = parse_number(cell).with_context(|| {
                    format!(
                        "[invalid-input] {kind}: source {id:?} row {} column {:?} is {cell:?}, not a number",
                        n + 2,
                        records.fields[value_col]
                    )
                })?;
                points.push(Point { label, value });
            }
            points
        };
        match series.sort.as_deref() {
            None | Some("none") => {}
            Some("desc") => points.sort_by(|a, b| b.value.total_cmp(&a.value)),
            Some("asc") => points.sort_by(|a, b| a.value.total_cmp(&b.value)),
            Some(other) => bail!("[invalid-input] sort must be desc, asc or none, got {other:?}"),
        }
        if let Some(limit) = series.limit {
            points.truncate(limit);
        }
        ensure!(
            !points.is_empty(),
            "[invalid-input] {kind} has no data points"
        );
        ensure!(
            points.len() <= MAX_POINTS,
            "[limit-exceeded] {kind} has {} points; the limit is {MAX_POINTS} (use `limit` or `sort`)",
            points.len()
        );
        ensure!(
            points.iter().all(|p| p.value >= 0.0),
            "[invalid-input] {kind} charts need nonnegative values; use a table for signed data"
        );
        Ok(points)
    }

    fn format_value(series: &Series, points: &[Point], value: f64) -> String {
        let decimals = series.decimals.unwrap_or_else(|| {
            if points.iter().all(|p| p.value.fract() == 0.0) {
                0
            } else {
                1
            }
        });
        format!(
            "{}{}{}",
            series.prefix,
            with_commas(value, decimals),
            series.suffix
        )
    }

    fn title_and_caption(
        &mut self,
        title: &Option<String>,
        ids: &[String],
        context: &str,
    ) -> Result<(f64, String, String)> {
        let marks = self.cite(ids, context)?;
        let heading = title
            .as_deref()
            .map(|t| format!("{t} {marks}").trim().to_string())
            .unwrap_or_default();
        let caption = self.caption_for(ids);
        let mut head = if heading.is_empty() {
            0.0
        } else {
            16.0 * 1.35 + 10.0
        };
        if !caption.is_empty() {
            head += 11.0 * 1.35 + 18.0;
        }
        Ok((head, heading, caption))
    }

    fn emit_title(&mut self, top: f64, heading: &str) -> f64 {
        if heading.is_empty() {
            return top;
        }
        let ink = self.spec.theme.ink.clone();
        let width = self.cw();
        self.put_wrapped("chart-title", heading, MARGIN, top, 16.0, 700, &ink, width);
        top + 16.0 * 1.35 + 10.0
    }

    fn emit_caption(&mut self, bottom: f64, caption: &str) {
        if caption.is_empty() {
            return;
        }
        let muted = self.spec.theme.muted.clone();
        let width = self.cw();
        self.put_wrapped(
            "caption",
            caption,
            MARGIN,
            bottom + 8.0,
            11.0,
            400,
            &muted,
            width,
        );
    }

    fn block(&mut self, number: usize, block: &Block) -> Result<()> {
        let theme_ink = self.spec.theme.ink.clone();
        let muted = self.spec.theme.muted.clone();
        let accent = self.spec.theme.accent.clone();
        let card = self.spec.theme.card.clone();
        let cw = self.cw();
        match block {
            Block::Heading { text, cite } => {
                let marks = self.cite(cite, &format!("block {number} (heading)"))?;
                let content = format!("{text} {marks}").trim().to_string();
                let height = self.wrap(&content, 24.0, cw).len() as f64 * 24.0 * 1.35 + 12.0;
                let top = self.reserve(height)?;
                self.put_wrapped("heading", &content, MARGIN, top, 24.0, 700, &theme_ink, cw);
            }
            Block::Text { text, cite, size } => {
                let size = size.unwrap_or(15.0).clamp(8.0, 48.0);
                let marks = self.cite(cite, &format!("block {number} (text)"))?;
                let content = format!("{text} {marks}").trim().to_string();
                let height = self.wrap(&content, size, cw).len() as f64 * size * 1.35 + 12.0;
                let top = self.reserve(height)?;
                self.put_wrapped("text", &content, MARGIN, top, size, 400, &theme_ink, cw);
            }
            Block::Callout { text, cite } => {
                let marks = self.cite(cite, &format!("block {number} (callout)"))?;
                let content = format!("{text} {marks}").trim().to_string();
                let inner = cw - 48.0;
                let text_h = self.wrap(&content, 17.0, inner).len() as f64 * 17.0 * 1.35;
                let height = text_h + 32.0;
                let top = self.reserve(height + GAP)?;
                let id = self.id("callout-card");
                self.push(rect(&id, MARGIN, top, cw, height, 10.0, &card));
                let id = self.id("callout-bar");
                self.push(rect(&id, MARGIN, top, 6.0, height, 3.0, &accent));
                self.put_wrapped(
                    "callout",
                    &content,
                    MARGIN + 28.0,
                    top + 16.0,
                    17.0,
                    600,
                    &theme_ink,
                    inner,
                );
            }
            Block::Spacer { height } => {
                ensure!(
                    height.is_finite() && (0.0..=1000.0).contains(height),
                    "[invalid-input] spacer height must be between 0 and 1000"
                );
                self.reserve(*height)?;
            }
            Block::Stats { items, cite } => {
                ensure!(
                    !items.is_empty() && items.len() <= 24,
                    "[invalid-input] stats needs 1 to 24 items, got {}",
                    items.len()
                );
                let context = format!("block {number} (stats)");
                let group = self.cite(cite, &context)?;
                let cols = items.len().min(4);
                let tile_w = (cw - GAP * (cols as f64 - 1.0)) / cols as f64;
                let rows = items.len().div_ceil(cols);
                let tile_h = 118.0;
                let top = self.reserve(rows as f64 * (tile_h + GAP))?;
                for (i, item) in items.iter().enumerate() {
                    let (col, row) = (i % cols, i / cols);
                    let x = MARGIN + col as f64 * (tile_w + GAP);
                    let y = top + row as f64 * (tile_h + GAP);
                    let marks = match &item.source {
                        Some(id) => self.cite(std::slice::from_ref(id), &context)?,
                        None => {
                            if cite.is_empty() {
                                self.warnings.push(format!(
                                    "{context}: stat {:?} ({}) has no source",
                                    item.label, item.value
                                ));
                            }
                            String::new()
                        }
                    };
                    let id = self.id("stat-tile");
                    self.push(rect(&id, x, y, tile_w, tile_h, 12.0, &card));
                    let value = item.value.clone();
                    let id = self.id("stat-value");
                    let big =
                        (tile_w / (value.chars().count().max(1) as f64 * 0.62)).clamp(18.0, 38.0);
                    let node = self.text_node(&id, &value, x + 16.0, y + 14.0, big, 700, &accent);
                    self.push(node);
                    let label = format!("{} {marks}", item.label).trim().to_string();
                    self.put_wrapped(
                        "stat-label",
                        &label,
                        x + 16.0,
                        y + 14.0 + big * 1.35 + 6.0,
                        13.0,
                        400,
                        &muted,
                        tile_w - 32.0,
                    );
                }
                let _ = group;
            }
            Block::Bar(series) => self.bar(number, series)?,
            Block::Column(series) => self.column(number, series)?,
            Block::Segmented(series) => self.segmented(number, series)?,
            Block::Table(series) => self.table(number, series)?,
        }
        Ok(())
    }

    fn data_warning(&mut self, number: usize, kind: &str, series: &Series) {
        if series.source.is_none() && series.cite.is_empty() {
            self.warnings
                .push(format!("block {number} ({kind}): chart data has no source"));
        }
    }

    fn bar(&mut self, number: usize, series: &Series) -> Result<()> {
        let kind = format!("block {number} (bar)");
        self.data_warning(number, "bar", series);
        let points = self.series_points(series, &kind)?;
        let ids = Self::all_series_sources(&series.source, &series.cite);
        let (head, heading, caption) = self.title_and_caption(&series.title, &ids, &kind)?;
        let row_h = 30.0;
        let height = head + points.len() as f64 * row_h + GAP;
        let top = self.reserve(height)?;
        let mut y = self.emit_title(top, &heading);
        let cw = self.cw();
        let (muted, ink, accent) = (
            self.spec.theme.muted.clone(),
            self.spec.theme.ink.clone(),
            self.spec.theme.accent.clone(),
        );
        let label_w = (cw * 0.28).min(280.0);
        let value_w = 120.0;
        let track = cw - label_w - value_w - 16.0;
        let max = points
            .iter()
            .map(|p| p.value)
            .fold(0.0, f64::max)
            .max(f64::MIN_POSITIVE);
        let max_chars = (label_w / (13.0 * 0.55)).floor() as usize;
        for p in &points {
            let id = self.id("bar-label");
            let node = self.text_node(
                &id,
                &truncate(&p.label, max_chars),
                MARGIN,
                y + 4.0,
                13.0,
                400,
                &ink,
            );
            self.push(node);
            let width = (track * p.value / max).max(2.0);
            let id = self.id("bar");
            self.push(rect(
                &id,
                MARGIN + label_w,
                y + 2.0,
                width,
                row_h - 8.0,
                4.0,
                &accent,
            ));
            let id = self.id("bar-value");
            let text = Self::format_value(series, &points, p.value);
            let node = self.text_node(
                &id,
                &text,
                MARGIN + label_w + width + 8.0,
                y + 4.0,
                13.0,
                700,
                &ink,
            );
            self.push(node);
            y += row_h;
        }
        let _ = muted;
        self.emit_caption(y, &caption);
        Ok(())
    }

    fn column(&mut self, number: usize, series: &Series) -> Result<()> {
        let kind = format!("block {number} (column)");
        self.data_warning(number, "column", series);
        let points = self.series_points(series, &kind)?;
        let ids = Self::all_series_sources(&series.source, &series.cite);
        let (head, heading, caption) = self.title_and_caption(&series.title, &ids, &kind)?;
        let plot_h = 220.0;
        let height = head + plot_h + 56.0;
        let top = self.reserve(height)?;
        let y = self.emit_title(top, &heading);
        let cw = self.cw();
        let (ink, accent, muted) = (
            self.spec.theme.ink.clone(),
            self.spec.theme.accent.clone(),
            self.spec.theme.muted.clone(),
        );
        let n = points.len() as f64;
        let slot = cw / n;
        let bar_w = (slot * 0.6).min(80.0);
        let max = points
            .iter()
            .map(|p| p.value)
            .fold(0.0, f64::max)
            .max(f64::MIN_POSITIVE);
        let base = y + 24.0 + plot_h;
        let id = self.id("axis");
        self.push(rect(&id, MARGIN, base, cw, 1.5, 0.0, &muted));
        let max_chars = ((slot - 4.0) / (11.0 * 0.55)).floor().max(1.0) as usize;
        for (i, p) in points.iter().enumerate() {
            let h = (plot_h * p.value / max).max(2.0);
            let x = MARGIN + slot * i as f64 + (slot - bar_w) / 2.0;
            let id = self.id("column");
            self.push(rect(&id, x, base - h, bar_w, h, 4.0, &accent));
            let id = self.id("column-value");
            let text = Self::format_value(series, &points, p.value);
            let node = self.text_node(&id, &text, x, base - h - 20.0, 12.0, 700, &ink);
            self.push(node);
            let id = self.id("column-label");
            let node = self.text_node(
                &id,
                &truncate(&p.label, max_chars),
                x,
                base + 8.0,
                11.0,
                400,
                &ink,
            );
            self.push(node);
        }
        self.emit_caption(base + 28.0, &caption);
        Ok(())
    }

    fn segmented(&mut self, number: usize, series: &Series) -> Result<()> {
        let kind = format!("block {number} (segmented)");
        self.data_warning(number, "segmented", series);
        let points = self.series_points(series, &kind)?;
        let total: f64 = points.iter().map(|p| p.value).sum();
        ensure!(total > 0.0, "[invalid-input] {kind}: values sum to zero");
        let ids = Self::all_series_sources(&series.source, &series.cite);
        let (head, heading, caption) = self.title_and_caption(&series.title, &ids, &kind)?;
        let legend_rows = points.len().div_ceil(2) as f64;
        let height = head + 36.0 + 12.0 + legend_rows * 24.0 + GAP;
        let top = self.reserve(height)?;
        let y = self.emit_title(top, &heading);
        let cw = self.cw();
        let ink = self.spec.theme.ink.clone();
        let mut x = MARGIN;
        for (i, p) in points.iter().enumerate() {
            let w = cw * p.value / total;
            let color = SERIES_COLORS[i % SERIES_COLORS.len()];
            let id = self.id("segment");
            self.push(rect(&id, x, y, w.max(1.0), 36.0, 0.0, color));
            x += w;
        }
        let half = cw / 2.0;
        for (i, p) in points.iter().enumerate() {
            let (col, row) = (i % 2, i / 2);
            let lx = MARGIN + col as f64 * half;
            let ly = y + 36.0 + 14.0 + row as f64 * 24.0;
            let color = SERIES_COLORS[i % SERIES_COLORS.len()];
            let id = self.id("legend-swatch");
            self.push(rect(&id, lx, ly + 3.0, 12.0, 12.0, 2.0, color));
            let text = format!(
                "{}  {} ({:.0}%)",
                truncate(&p.label, 34),
                Self::format_value(series, &points, p.value),
                p.value / total * 100.0
            );
            let id = self.id("legend");
            let node = self.text_node(&id, &text, lx + 20.0, ly, 13.0, 400, &ink);
            self.push(node);
        }
        let bottom = y + 36.0 + 14.0 + legend_rows * 24.0;
        self.emit_caption(bottom, &caption);
        Ok(())
    }

    fn table(&mut self, number: usize, series: &TableSeries) -> Result<()> {
        let kind = format!("block {number} (table)");
        if series.source.is_none() && series.cite.is_empty() {
            self.warnings
                .push(format!("block {number} (table): table has no source"));
        }
        let id = series
            .source
            .as_deref()
            .with_context(|| format!("[invalid-input] {kind} needs a `source` with a data file"))?;
        let records = self.data.get(id).cloned().with_context(|| {
            format!("[missing-resource] {kind} reads source {id:?} but it declares no `data` file")
        })?;
        let columns: Vec<usize> = if series.columns.is_empty() {
            (0..records.fields.len().min(6)).collect()
        } else {
            series
                .columns
                .iter()
                .map(|name| column(&records, name, &kind))
                .collect::<Result<_>>()?
        };
        ensure!(
            (1..=8).contains(&columns.len()),
            "[invalid-input] {kind} shows 1 to 8 columns, got {}",
            columns.len()
        );
        let limit = series.limit.unwrap_or(MAX_TABLE_ROWS).min(MAX_TABLE_ROWS);
        let rows: Vec<&Vec<String>> = records.rows.iter().take(limit).collect();
        ensure!(!rows.is_empty(), "[invalid-input] {kind} has no rows");
        let ids = Self::all_series_sources(&series.source, &series.cite);
        let (head, heading, caption) = self.title_and_caption(&series.title, &ids, &kind)?;
        let row_h = 28.0;
        let height = head + (rows.len() + 1) as f64 * row_h + GAP;
        let top = self.reserve(height)?;
        let mut y = self.emit_title(top, &heading);
        let cw = self.cw();
        let (ink, muted, card, accent) = (
            self.spec.theme.ink.clone(),
            self.spec.theme.muted.clone(),
            self.spec.theme.card.clone(),
            self.spec.theme.accent.clone(),
        );
        let col_w = cw / columns.len() as f64;
        let max_chars = ((col_w - 12.0) / (13.0 * 0.55)).floor().max(2.0) as usize;
        let id = self.id("table-head");
        self.push(rect(&id, MARGIN, y, cw, row_h, 4.0, &accent));
        for (c, &col) in columns.iter().enumerate() {
            let id = self.id("table-th");
            let node = self.text_node(
                &id,
                &truncate(&records.fields[col], max_chars),
                MARGIN + c as f64 * col_w + 8.0,
                y + 6.0,
                13.0,
                700,
                "#ffffff",
            );
            self.push(node);
        }
        y += row_h;
        for (r, row) in rows.iter().enumerate() {
            if r % 2 == 1 {
                let id = self.id("table-zebra");
                self.push(rect(&id, MARGIN, y, cw, row_h, 0.0, &card));
            }
            for (c, &col) in columns.iter().enumerate() {
                let cell = row.get(col).map(String::as_str).unwrap_or("");
                let id = self.id("table-td");
                let node = self.text_node(
                    &id,
                    &truncate(cell, max_chars),
                    MARGIN + c as f64 * col_w + 8.0,
                    y + 6.0,
                    13.0,
                    400,
                    &ink,
                );
                self.push(node);
            }
            y += row_h;
        }
        let _ = muted;
        self.emit_caption(y, &caption);
        Ok(())
    }

    fn sources_pages(&mut self) -> Result<()> {
        if self.order.is_empty() {
            return Ok(());
        }
        self.new_page(Some("Sources"));
        let cw = self.cw();
        let (ink, muted) = (self.spec.theme.ink.clone(), self.spec.theme.muted.clone());
        // Trust legend: one line per kind actually used.
        let used: Vec<Kind> = {
            let mut kinds: Vec<Kind> = self.order.iter().map(|id| self.sources[id].kind).collect();
            kinds.dedup();
            let mut unique = Vec::new();
            for k in kinds {
                if !unique.contains(&k) {
                    unique.push(k);
                }
            }
            unique
        };
        let blurb = |kind: Kind| match kind {
            Kind::Observed => "measured or reported after the fact",
            Kind::Survey => "what a sample of people or firms said",
            Kind::Forecast => "a model's projection, not a measurement",
            Kind::Other => "context; check the original before relying on it",
        };
        let top = self.reserve(used.len() as f64 * 26.0 + GAP)?;
        for (i, kind) in used.iter().enumerate() {
            let y = top + i as f64 * 26.0;
            let id = self.id("trust-chip");
            self.push(rect(&id, MARGIN, y, 96.0, 20.0, 10.0, kind.color()));
            let id = self.id("trust-chip-text");
            let node = self.text_node(
                &id,
                kind.label(),
                MARGIN + 12.0,
                y + 3.0,
                11.0,
                700,
                "#ffffff",
            );
            self.push(node);
            let id = self.id("trust-blurb");
            let node = self.text_node(
                &id,
                blurb(*kind),
                MARGIN + 110.0,
                y + 2.0,
                13.0,
                400,
                &muted,
            );
            self.push(node);
        }
        for (i, sid) in self.order.clone().iter().enumerate() {
            let source = self.sources[sid];
            let mut line = format!("[{}] {}", i + 1, source.title);
            if let Some(publisher) = &source.publisher {
                line.push_str(&format!(" — {publisher}"));
            }
            if let Some(date) = &source.date {
                line.push_str(&format!(", {date}"));
            }
            if let Some(url) = &source.url {
                line.push_str(&format!("\n{url}"));
            }
            if let Some(note) = &source.note {
                line.push_str(&format!("\n{note}"));
            }
            let lines: usize = line.lines().map(|l| wrap(l, 13.0, cw - 110.0).len()).sum();
            let height = lines as f64 * 13.0 * 1.35 + 14.0;
            let top = self.reserve(height)?;
            let id = self.id("source-chip");
            self.push(rect(
                &id,
                MARGIN,
                top + 2.0,
                84.0,
                18.0,
                9.0,
                source.kind.color(),
            ));
            let id = self.id("source-chip-text");
            let node = self.text_node(
                &id,
                source.kind.label(),
                MARGIN + 10.0,
                top + 5.0,
                10.0,
                700,
                "#ffffff",
            );
            self.push(node);
            let wrapped: Vec<String> = line
                .lines()
                .flat_map(|l| wrap(l, 13.0, cw - 110.0))
                .collect();
            let id = self.id("source");
            let node = self.text_node(
                &id,
                &wrapped.join("\n"),
                MARGIN + 100.0,
                top,
                13.0,
                400,
                &ink,
            );
            self.push(node);
        }
        Ok(())
    }
}

fn column(records: &Records, name: &str, context: &str) -> Result<usize> {
    records
        .fields
        .iter()
        .position(|f| f == name)
        .with_context(|| {
            format!(
                "[missing-resource] {context}: no column {name:?}; columns: {}",
                records.fields.join(", ")
            )
        })
}

fn rect(id: &str, x: f64, y: f64, width: f64, height: f64, radius: f64, fill: &str) -> Value {
    json!({"kind":"rect","id":id,"x":x,"y":y,"width":width.max(0.0),"height":height.max(0.0),
        "radius_x":radius,"radius_y":radius,
        "style":{"fill":{"fallback":fill},"stroke":{"fallback":"none"},"stroke_width":{"fallback":0}}})
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(1).max(1);
    format!("{}…", text.chars().take(keep).collect::<String>())
}

/// Greedy word wrap with an average glyph width of `0.55 * size`.
fn wrap(content: &str, size: f64, width: f64) -> Vec<String> {
    let limit = (width / (size * 0.55)).floor().max(4.0) as usize;
    let mut lines = Vec::new();
    for paragraph in content.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let word_len = word.chars().count();
            if !line.is_empty() && line.chars().count() + 1 + word_len > limit {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn with_commas(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    let (int, frac) = match text.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (text, None),
    };
    let negative = int.starts_with('-');
    let digits: Vec<char> = int.trim_start_matches('-').chars().collect();
    let mut out = String::new();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c);
    }
    format!(
        "{}{}{}",
        if negative { "-" } else { "" },
        out,
        frac.map(|f| format!(".{f}")).unwrap_or_default()
    )
}

// ------------------------------------------------------------------ build

pub struct Built {
    pub document: Value,
    pub report: Value,
    pub warnings: Vec<String>,
}

pub fn parse_spec(bytes: &[u8]) -> Result<Spec> {
    ensure!(
        bytes.len() <= MAX_SPEC_BYTES,
        "[limit-exceeded] spec is {} bytes; the limit is {MAX_SPEC_BYTES}",
        bytes.len()
    );
    serde_json::from_slice(bytes).context("[malformed-resource] invalid report spec")
}

/// Build the report document in memory. `dir` is where data files resolve.
pub fn build(spec: &Spec, dir: &Path) -> Result<Built> {
    ensure!(
        !spec.title.trim().is_empty() && spec.title.len() <= 256,
        "[invalid-input] report title must contain 1 to 256 bytes"
    );
    ensure!(
        (200..=16384).contains(&spec.page.width) && (200..=16384).contains(&spec.page.height),
        "[invalid-input] page size must be between 200 and 16384"
    );
    ensure!(
        spec.sources.len() <= MAX_SOURCES,
        "[limit-exceeded] {} sources; the limit is {MAX_SOURCES}",
        spec.sources.len()
    );
    ensure!(
        !spec.pages.is_empty(),
        "[invalid-input] a report needs at least one page"
    );
    let blocks: usize = spec.pages.iter().map(|p| p.blocks.len()).sum();
    ensure!(
        blocks <= MAX_BLOCKS,
        "[limit-exceeded] {blocks} blocks; the limit is {MAX_BLOCKS}"
    );
    for color in [
        &spec.theme.accent,
        &spec.theme.ink,
        &spec.theme.muted,
        &spec.theme.background,
        &spec.theme.card,
    ] {
        ensure!(
            scene::is_valid_color(color),
            "[invalid-input] theme color {color:?} is not a color; use #RRGGBB"
        );
    }
    let mut sources = BTreeMap::new();
    let mut data = BTreeMap::new();
    for source in &spec.sources {
        ensure!(
            !source.id.is_empty() && source.id.len() <= 64,
            "[invalid-input] source ids must contain 1 to 64 bytes"
        );
        ensure!(
            sources.insert(source.id.clone(), source).is_none(),
            "[invalid-input] duplicate source id {:?}",
            source.id
        );
        if let Some(records) = load_records(dir, source)? {
            data.insert(source.id.clone(), records);
        }
    }
    let mut b = Builder {
        spec,
        data,
        numbers: BTreeMap::new(),
        order: Vec::new(),
        warnings: Vec::new(),
        pages: Vec::new(),
        titles: Vec::new(),
        cursor: MARGIN,
        counter: 0,
        sources,
    };
    let mut number = 0;
    for (index, page) in spec.pages.iter().enumerate() {
        b.new_page(page.title.as_deref());
        if index == 0 {
            let (ink, muted) = (spec.theme.ink.clone(), spec.theme.muted.clone());
            if page.title.is_none() {
                let id = b.id("report-title");
                let width = b.cw();
                let lines = wrap(&spec.title, 44.0, width);
                let node = b.text_node(&id, &lines.join("\n"), MARGIN, b.cursor, 44.0, 700, &ink);
                b.push(node);
                b.cursor += lines.len() as f64 * 44.0 * 1.35;
            }
            if let Some(subtitle) = &spec.subtitle {
                let width = b.cw();
                let used = b.put_wrapped(
                    "report-subtitle",
                    subtitle,
                    MARGIN,
                    b.cursor,
                    18.0,
                    400,
                    &muted,
                    width,
                );
                b.cursor += used + GAP * 1.5;
            }
        }
        for block in &page.blocks {
            number += 1;
            b.block(number, block)?;
        }
    }
    if spec.sources_page {
        b.sources_pages()?;
    }
    for id in b.sources.keys() {
        if !b.numbers.contains_key(id) {
            b.warnings.push(format!("source {id:?} is never cited"));
        }
    }
    // Page numbers.
    let total = b.pages.len();
    let muted = spec.theme.muted.clone();
    for index in 0..total {
        let id = format!("footer-{}", index + 1);
        let y = f64::from(spec.page.height) - MARGIN;
        let label = format!("{}   ·   {} / {}", spec.title, index + 1, total);
        let node = b.text_node(&id, &truncate(&label, 120), MARGIN, y, 11.0, 400, &muted);
        b.pages[index].push(node);
    }
    let mut document = scene::new_document(spec.page.width, spec.page.height);
    document["name"] = json!(spec.title.trim());
    let mut pages = Vec::new();
    for (index, nodes) in b.pages.iter().enumerate() {
        pages.push(json!({
            "id": format!("page-{}", index + 1),
            "name": b.titles[index],
            "canvas": {"width": spec.page.width, "height": spec.page.height, "background": spec.theme.background},
            "layers": [{"id": "layer-1", "name": "Report", "visible": true, "locked": false, "nodes": nodes}]
        }));
    }
    document["pages"] = Value::Array(pages);
    scene::validate(&document)?;
    let citations: Vec<Value> = b
        .order
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let s = b.sources[id];
            json!({"number": i + 1, "id": id, "title": s.title, "kind": s.kind.label().to_lowercase(), "url": s.url})
        })
        .collect();
    let report = json!({
        "pages": total,
        "blocks": blocks,
        "nodes": b.pages.iter().map(Vec::len).sum::<usize>(),
        "citations": citations,
        "data_sources": b.data.keys().collect::<Vec<_>>(),
        "warnings": b.warnings,
    });
    let warnings = b.warnings;
    Ok(Built {
        document,
        report,
        warnings,
    })
}

// ------------------------------------------------------------------ commands

fn read_spec(path: &Path) -> Result<(Spec, PathBuf)> {
    let bytes = crate::resource::read_bounded(path, MAX_SPEC_BYTES)
        .with_context(|| format!("[missing-resource] cannot read {}", path.display()))?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    Ok((parse_spec(&bytes)?, dir))
}

pub fn data_summary(
    file: &Path,
    format: Option<&str>,
    delimiter: Option<char>,
    rows: usize,
) -> Result<Value> {
    let format = crate::merge::data_format(file, format)?;
    let bytes = crate::resource::read_bounded(file, MAX_DATA_BYTES)
        .with_context(|| format!("[missing-resource] cannot read {}", file.display()))?;
    let records = parse_records(&bytes, &format, delimiter)?;
    let numeric = numeric_columns(&records);
    let columns: Vec<Value> = records
        .fields
        .iter()
        .enumerate()
        .map(|(c, name)| {
            let mut entry = json!({"name": name, "numeric": numeric.contains(&c)});
            if numeric.contains(&c) {
                let values: Vec<f64> = records
                    .rows
                    .iter()
                    .filter_map(|r| r.get(c).and_then(|cell| parse_number(cell)))
                    .collect();
                entry["min"] = json!(values.iter().cloned().fold(f64::INFINITY, f64::min));
                entry["max"] = json!(values.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
                entry["sum"] = json!(values.iter().sum::<f64>());
            }
            entry
        })
        .collect();
    let sample: Vec<&Vec<String>> = records.rows.iter().take(rows.min(100)).collect();
    Ok(json!({
        "file": file.display().to_string(),
        "format": format,
        "rows": records.rows.len(),
        "columns": columns,
        "sample": sample,
    }))
}

pub const EXAMPLE: &str = r##"{
  "title": "Example market report",
  "subtitle": "Every number cites a source; charts read their data from CSV.",
  "sources": [
    {"id": "regions", "title": "Revenue by region", "publisher": "Example Research",
     "date": "2025", "kind": "observed", "url": "https://example.com/regions",
     "data": "regions.csv"},
    {"id": "outlook", "title": "Five-year outlook", "publisher": "Example Analysts",
     "date": "2025", "kind": "forecast", "url": "https://example.com/outlook"}
  ],
  "pages": [
    {"title": "Overview", "blocks": [
      {"type": "text", "text": "Revenue is concentrated in three regions.", "cite": ["regions"]},
      {"type": "stats", "items": [
        {"value": "$188.8B", "label": "2025 revenue", "source": "regions"},
        {"value": "+3.4%", "label": "growth to 2030", "source": "outlook"}]},
      {"type": "bar", "title": "Revenue by region ($B)", "source": "regions",
       "label": "region", "value": "revenue", "prefix": "$", "suffix": "B", "sort": "desc"},
      {"type": "callout", "text": "Forecasts are projections, not measurements.", "cite": ["outlook"]}
    ]}
  ]
}
"##;

pub const EXAMPLE_CSV: &str =
    "region,revenue\nAsia,84.1\nNorth America,52.6\nEurope,31.0\nLatin America,11.2\nOther,9.9\n";

pub fn run(action: ReportAction) -> Result<Value> {
    match action {
        ReportAction::Example => {
            Ok(json!({"spec": serde_json::from_str::<Value>(EXAMPLE)?, "regions.csv": EXAMPLE_CSV}))
        }
        ReportAction::Data {
            file,
            format,
            delimiter,
            rows,
        } => data_summary(&file, format.as_deref(), delimiter, rows),
        ReportAction::Check { spec, strict } => {
            let (parsed, dir) = read_spec(&spec)?;
            let built = build(&parsed, &dir)?;
            if strict && !built.warnings.is_empty() {
                bail!(
                    "[policy-denied] {} warning(s) in strict mode: {}",
                    built.warnings.len(),
                    built.warnings.join("; ")
                )
            }
            Ok(built.report)
        }
        ReportAction::Build {
            spec,
            out,
            overwrite,
            dry_run,
            strict,
        } => {
            let (parsed, dir) = read_spec(&spec)?;
            let mut built = build(&parsed, &dir)?;
            if strict && !built.warnings.is_empty() {
                bail!(
                    "[policy-denied] {} warning(s) in strict mode: {}; nothing was written",
                    built.warnings.len(),
                    built.warnings.join("; ")
                )
            }
            if out.exists() && !overwrite && !dry_run {
                bail!(
                    "[policy-denied] {} already exists; choose another path or pass --overwrite; nothing was written",
                    out.display()
                )
            }
            if !dry_run {
                if out.exists() {
                    crate::transaction::commit_value(&out, "report", false, None, &built.document)
                        .with_context(|| format!("could not replace {}", out.display()))?;
                } else {
                    crate::editing::atomic_write(
                        &out,
                        &serde_json::to_vec_pretty(&built.document)?,
                    )
                    .with_context(|| format!("could not write {}", out.display()))?;
                }
                built.report["output"] = json!(out.display().to_string());
            }
            built.report["dry_run"] = json!(dry_run);
            Ok(built.report)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_parse_with_currency_and_commas() {
        assert_eq!(parse_number("$1,234.5"), Some(1234.5));
        assert_eq!(parse_number("12%"), Some(12.0));
        assert_eq!(parse_number("n/a"), None);
        assert_eq!(parse_number(""), None);
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(with_commas(1234567.0, 0), "1,234,567");
        assert_eq!(with_commas(-1234.56, 1), "-1,234.6");
        assert_eq!(with_commas(12.0, 0), "12");
    }

    #[test]
    fn data_paths_cannot_escape() {
        assert!(safe_relative("../x.csv").is_err());
        assert!(safe_relative("/etc/x.csv").is_err());
        assert!(safe_relative("data/x.csv").is_ok());
    }

    #[test]
    fn wrap_breaks_long_text() {
        assert!(wrap("one two three four five six seven", 10.0, 60.0).len() > 1);
    }
}

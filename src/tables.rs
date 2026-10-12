//! v0.14.0 task 9: tables (`docs/layout-v1.md` section 9).
//!
//! A table is a definition in `layout.tables` referenced by exactly one
//! `{"table": "<id>"}` run of a story. Cells are sparse: only cells with
//! content, overrides or spans are stored, every other grid position is an
//! empty default cell. Heights and breaks come from the task 3 estimator and
//! are reported `estimated: true`.
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

pub const MAX_TABLES: usize = 1000;
pub const MAX_CELLS: usize = 100_000;
pub const MAX_ROWS: usize = 10_000;
pub const MAX_COLUMNS: usize = 256;
pub const MAX_CELL_BYTES: usize = 64 * 1024;
pub const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;
const ALIGNS: &[&str] = &["left", "center", "right", "justify"];
const VALIGNS: &[&str] = &["top", "center", "bottom"];
const ROTATIONS: &[u64] = &[0, 90, 180, 270];

fn default_limit() -> usize {
    100
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TableAction {
    /// Create an empty table definition (place it in a story with `place`).
    Create {
        id: String,
        #[arg(long)]
        rows: usize,
        #[arg(long)]
        columns: usize,
        /// Width of every column in points.
        #[arg(long, default_value_t = 72.0)]
        #[serde(default)]
        column_width: f64,
        /// Fixed height of every row (omit for text-driven rows).
        #[arg(long)]
        row_height: Option<f64>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        header_rows: usize,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        footer_rows: usize,
        #[arg(long)]
        style: Option<String>,
    },
    /// Show a table and a page of its cells.
    Show {
        id: String,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// List tables (paginated).
    List {
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Delete an unplaced table.
    Delete { id: String },
    /// Set table level properties.
    Set {
        id: String,
        #[arg(long)]
        style: Option<String>,
        #[arg(long)]
        header_rows: Option<usize>,
        #[arg(long)]
        footer_rows: Option<usize>,
        /// Comma separated fills cycled over body rows (alternating pattern).
        #[arg(long)]
        row_pattern: Option<String>,
        /// Comma separated fills cycled over columns.
        #[arg(long)]
        column_pattern: Option<String>,
    },
    /// Place a table into a story at a character offset of a paragraph.
    Place {
        id: String,
        #[arg(long)]
        story: String,
        #[arg(long)]
        paragraph: usize,
        #[arg(long)]
        at: Option<usize>,
    },
    /// Set the content or formatting of one cell.
    CellSet {
        id: String,
        #[arg(long)]
        row: usize,
        #[arg(long)]
        col: usize,
        /// Replace the cell text (paragraphs separated by `\n`).
        #[arg(long)]
        text: Option<String>,
        /// top,right,bottom,left insets in points.
        #[arg(long)]
        inset: Option<String>,
        #[arg(long)]
        align: Option<String>,
        #[arg(long)]
        vertical_align: Option<String>,
        /// 0, 90, 180 or 270.
        #[arg(long)]
        rotation: Option<u64>,
        #[arg(long)]
        fill: Option<String>,
        #[arg(long)]
        stroke_color: Option<String>,
        #[arg(long)]
        stroke_width: Option<f64>,
        /// Node ID shown as the cell content (a graphic cell).
        #[arg(long)]
        graphic: Option<String>,
        /// Cell style ID.
        #[arg(long)]
        style: Option<String>,
    },
    /// Merge a rectangular range of cells; contents concatenate in cell order.
    Merge {
        id: String,
        #[arg(long)]
        row: usize,
        #[arg(long)]
        col: usize,
        #[arg(long)]
        rows: usize,
        #[arg(long)]
        cols: usize,
    },
    /// Split a merged cell back into single cells (content stays top left).
    Split {
        id: String,
        #[arg(long)]
        row: usize,
        #[arg(long)]
        col: usize,
    },
    /// Insert rows before `at` (use the row count to append).
    AddRow {
        id: String,
        #[arg(long)]
        at: usize,
        #[arg(long, default_value_t = 1)]
        #[serde(default)]
        count: usize,
    },
    /// Remove a row; content is never dropped silently.
    RemoveRow {
        id: String,
        #[arg(long)]
        at: usize,
        /// Accept losing the content of the row.
        #[arg(long)]
        discard: bool,
    },
    /// Insert columns before `at` (use the column count to append).
    AddColumn {
        id: String,
        #[arg(long)]
        at: usize,
        #[arg(long, default_value_t = 1)]
        #[serde(default)]
        count: usize,
        #[arg(long, default_value_t = 72.0)]
        #[serde(default)]
        width: f64,
    },
    /// Remove a column; content is never dropped silently.
    RemoveColumn {
        id: String,
        #[arg(long)]
        at: usize,
        #[arg(long)]
        discard: bool,
    },
    /// Convert text to a table: from `--text`, or from `--count` story paragraphs.
    FromText {
        id: String,
        /// Text with the given separators (omit to read story paragraphs).
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        story: Option<String>,
        #[arg(long)]
        paragraph: Option<usize>,
        #[arg(long)]
        count: Option<usize>,
        #[arg(long, default_value = "\\t")]
        #[serde(default = "tab")]
        column_separator: String,
        #[arg(long, default_value = "\\n")]
        #[serde(default = "newline")]
        row_separator: String,
        #[arg(long, default_value_t = 72.0)]
        #[serde(default)]
        column_width: f64,
    },
    /// Convert a table back to text paragraphs in its story.
    ToText {
        id: String,
        #[arg(long, default_value = "\\t")]
        #[serde(default = "tab")]
        column_separator: String,
    },
    /// Create a table from a CSV or TSV file (RFC 4180, UTF-8).
    Import {
        id: String,
        file: std::path::PathBuf,
        /// csv or tsv (default: from the file extension).
        #[arg(long)]
        format: Option<String>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        header_rows: usize,
        #[arg(long, default_value_t = 72.0)]
        #[serde(default)]
        column_width: f64,
    },
    /// Estimate row heights and how the table breaks across frames.
    Plan {
        id: String,
        /// Available heights per frame in points (default: the story's frames).
        #[arg(long, value_delimiter = ',')]
        heights: Vec<f64>,
    },
}

type StorySite = (String, usize, usize);

fn tab() -> String {
    "\\t".into()
}

fn newline() -> String {
    "\\n".into()
}

// ------------------------------------------------------------ helpers

fn tables(raw: &Value) -> &[Value] {
    raw.pointer("/layout/tables")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn unescape(sep: &str) -> Result<String> {
    let out = sep.replace("\\t", "\t").replace("\\n", "\n");
    ensure!(
        !out.is_empty(),
        "[invalid-input] separators must not be empty"
    );
    Ok(out)
}

fn one_of(value: &str, what: &str, allowed: &[&str]) -> Result<()> {
    ensure!(
        allowed.contains(&value),
        "[invalid-input] {what} `{value}` must be one of {}",
        allowed.join(", ")
    );
    Ok(())
}

fn check_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')),
        "[invalid-input] table ID `{id}` must be 1-128 characters of letters, digits, `_ - . :`"
    );
    Ok(())
}

fn index_of(raw: &Value, id: &str) -> Result<usize> {
    tables(raw)
        .iter()
        .position(|t| t["id"] == id)
        .with_context(|| format!("[missing-resource] table not found: {id}"))
}

fn table_mut<'a>(raw: &'a mut Value, id: &str) -> Result<&'a mut Value> {
    let index = index_of(raw, id)?;
    Ok(&mut raw["layout"]["tables"][index])
}

fn dims(table: &Value) -> (usize, usize) {
    (list(table, "rows").len(), list(table, "columns").len())
}

fn style_exists(raw: &Value, kind: &str, id: &str) -> bool {
    raw.pointer(&format!("/layout/styles/{kind}/{id}"))
        .is_some()
}

fn node_exists(raw: &Value, id: &str) -> bool {
    fn rec(nodes: &[Value], id: &str) -> bool {
        nodes
            .iter()
            .any(|n| n["id"] == id || rec(list(n, "children"), id))
    }
    list(raw, "pages")
        .iter()
        .any(|p| list(p, "layers").iter().any(|l| rec(list(l, "nodes"), id)))
}

fn normalized(text: &str, what: &str) -> Result<String> {
    ensure!(
        text.len() <= MAX_CELL_BYTES,
        "[limit-exceeded] {what}: {} bytes exceeds the cell limit of {MAX_CELL_BYTES}",
        text.len()
    );
    ensure!(
        !text.chars().any(|c| c.is_control() && c != '\n'),
        "[invalid-input] {what} contains a control character"
    );
    Ok(text.nfc().collect())
}

fn paragraphs_from(text: &str) -> Vec<Value> {
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                json!({"runs": []})
            } else {
                json!({"runs": [{"text": line}]})
            }
        })
        .collect()
}

fn cell_text(cell: &Value) -> String {
    list(cell, "paragraphs")
        .iter()
        .map(|p| {
            list(p, "runs")
                .iter()
                .filter_map(|r| r.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_content(cell: &Value) -> bool {
    !cell_text(cell).is_empty() || cell.get("graphic").is_some()
}

fn span(cell: &Value, key: &str) -> usize {
    cell.get(key).and_then(Value::as_u64).unwrap_or(1) as usize
}

fn cell_pos(cell: &Value) -> (usize, usize) {
    (
        cell["row"].as_u64().unwrap_or(0) as usize,
        cell["col"].as_u64().unwrap_or(0) as usize,
    )
}

fn find_cell(table: &Value, row: usize, col: usize) -> Option<usize> {
    list(table, "cells")
        .iter()
        .position(|c| cell_pos(c) == (row, col))
}

/// The anchor of the cell covering `(row, col)`, if a span covers it.
fn covering(table: &Value, row: usize, col: usize) -> Option<usize> {
    list(table, "cells").iter().position(|c| {
        let (r, k) = cell_pos(c);
        row >= r && row < r + span(c, "row_span") && col >= k && col < k + span(c, "col_span")
    })
}

fn sort_cells(table: &mut Value) {
    if let Some(cells) = table.get_mut("cells").and_then(Value::as_array_mut) {
        cells.sort_by_key(cell_pos);
    }
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn require_v9(raw: &Value) -> Result<()> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] tables need a version 9 document; run `pentool migrate --target 9` first"
    );
    Ok(())
}

fn table_run_site(raw: &Value, id: &str) -> Option<(usize, usize, usize)> {
    for (s, story) in list(raw.pointer("/layout").unwrap_or(&Value::Null), "stories")
        .iter()
        .enumerate()
    {
        for (p, paragraph) in list(story, "paragraphs").iter().enumerate() {
            for (r, run) in list(paragraph, "runs").iter().enumerate() {
                if run.get("table").and_then(Value::as_str) == Some(id) {
                    return Some((s, p, r));
                }
            }
        }
    }
    None
}

// ------------------------------------------------------------ validation

fn validate_table(raw: &Value, table: &Value) -> Result<()> {
    let id = table["id"].as_str().unwrap_or_default();
    check_id(id)?;
    let columns = table
        .get("columns")
        .and_then(Value::as_array)
        .with_context(|| format!("[malformed-resource] table {id}: columns must be an array"))?;
    let rows = table
        .get("rows")
        .and_then(Value::as_array)
        .with_context(|| format!("[malformed-resource] table {id}: rows must be an array"))?;
    ensure!(
        (1..=MAX_COLUMNS).contains(&columns.len()),
        "[limit-exceeded] table {id}: {} columns must be between 1 and {MAX_COLUMNS}",
        columns.len()
    );
    ensure!(
        (1..=MAX_ROWS).contains(&rows.len()),
        "[limit-exceeded] table {id}: {} rows must be between 1 and {MAX_ROWS}",
        rows.len()
    );
    for (i, column) in columns.iter().enumerate() {
        ensure!(
            column["width"]
                .as_f64()
                .is_some_and(|w| w.is_finite() && w > 0.0 && w <= 100_000.0),
            "[malformed-resource] table {id} column {i}: width must be a positive number"
        );
    }
    for (i, row) in rows.iter().enumerate() {
        for key in ["height", "min_height"] {
            if let Some(v) = row.get(key) {
                ensure!(
                    v.as_f64()
                        .is_some_and(|h| h.is_finite() && (0.0..=100_000.0).contains(&h)),
                    "[malformed-resource] table {id} row {i}: {key} must be a non-negative number"
                );
            }
        }
    }
    let header = table["header_rows"].as_u64().unwrap_or(0) as usize;
    let footer = table["footer_rows"].as_u64().unwrap_or(0) as usize;
    ensure!(
        header + footer < rows.len() || (header + footer == 0),
        "[invalid-input] table {id}: header ({header}) and footer ({footer}) rows leave no body row"
    );
    if let Some(style) = table.get("style").and_then(Value::as_str) {
        ensure!(
            style_exists(raw, "table", style),
            "[missing-resource] table {id}: table style `{style}` does not exist"
        );
    }
    for key in ["row_pattern", "column_pattern"] {
        if let Some(pattern) = table.get(key) {
            let ok = pattern
                .as_array()
                .is_some_and(|a| a.len() <= 64 && a.iter().all(|v| v.is_string()));
            ensure!(
                ok,
                "[malformed-resource] table {id}: {key} must be up to 64 fill strings"
            );
        }
    }
    let cells = table
        .get("cells")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    ensure!(
        cells.len() <= MAX_CELLS,
        "[limit-exceeded] table {id}: {} cells exceeds the limit of {MAX_CELLS}",
        cells.len()
    );
    let (nr, nc) = (rows.len(), columns.len());
    let mut occupied: BTreeMap<(usize, usize), (usize, usize)> = BTreeMap::new();
    for cell in cells {
        let (r, c) = cell_pos(cell);
        let (rs, cs) = (span(cell, "row_span"), span(cell, "col_span"));
        ensure!(
            cell["row"].is_u64() && cell["col"].is_u64() && r < nr && c < nc,
            "[malformed-resource] table {id}: cell ({r}, {c}) is outside the {nr} x {nc} grid"
        );
        ensure!(
            rs >= 1 && cs >= 1 && r + rs <= nr && c + cs <= nc,
            "[malformed-resource] table {id}: cell ({r}, {c}) spans {rs} x {cs}, which leaves the grid"
        );
        if r < header {
            ensure!(
                r + rs <= header,
                "[invalid-input] table {id}: header cell ({r}, {c}) spans into the body"
            );
        } else if r < nr - footer {
            ensure!(
                r + rs <= nr - footer,
                "[invalid-input] table {id}: body cell ({r}, {c}) spans into the footer"
            );
        }
        for rr in r..r + rs {
            for cc in c..c + cs {
                if let Some(other) = occupied.insert((rr, cc), (r, c)) {
                    bail!(
                        "[invalid-input] table {id}: cells ({}, {}) and ({r}, {c}) overlap at ({rr}, {cc})",
                        other.0,
                        other.1
                    );
                }
            }
        }
        {
            if let Some(v) = cell.get("inset") {
                let ok = v.as_array().is_some_and(|a| {
                    a.len() == 4
                        && a.iter()
                            .all(|n| n.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0))
                });
                ensure!(
                    ok,
                    "[malformed-resource] table {id} cell ({r}, {c}): inset must be [top, right, bottom, left]"
                );
            }
        }
        if let Some(v) = cell.get("align") {
            one_of(v.as_str().unwrap_or_default(), "cell align", ALIGNS)?;
        }
        if let Some(v) = cell.get("vertical_align") {
            one_of(
                v.as_str().unwrap_or_default(),
                "cell vertical_align",
                VALIGNS,
            )?;
        }
        if let Some(v) = cell.get("rotation") {
            ensure!(
                v.as_u64().is_some_and(|n| ROTATIONS.contains(&n)),
                "[malformed-resource] table {id} cell ({r}, {c}): rotation must be 0, 90, 180 or 270"
            );
        }
        if let Some(style) = cell.get("style").and_then(Value::as_str) {
            ensure!(
                style_exists(raw, "cell", style),
                "[missing-resource] table {id} cell ({r}, {c}): cell style `{style}` does not exist"
            );
        }
        if let Some(graphic) = cell.get("graphic").and_then(Value::as_str) {
            ensure!(
                node_exists(raw, graphic),
                "[missing-resource] table {id} cell ({r}, {c}): graphic {graphic} does not exist"
            );
        }
        let text = cell_text(cell);
        ensure!(
            text.len() <= MAX_CELL_BYTES,
            "[limit-exceeded] table {id} cell ({r}, {c}): {} bytes exceeds the limit of {MAX_CELL_BYTES}",
            text.len()
        );
    }
    Ok(())
}

/// Validate `layout.tables` and the story runs that place them.
pub fn validate(raw: &Value) -> Result<()> {
    let Some(value) = raw.pointer("/layout/tables") else {
        return placed_tables_exist(raw, &BTreeSet::new());
    };
    let all = value
        .as_array()
        .context("[invalid-input] layout.tables must be an array")?;
    ensure!(
        all.len() <= MAX_TABLES,
        "[limit-exceeded] {} tables exceeds the limit of {MAX_TABLES}",
        all.len()
    );
    let mut ids = BTreeSet::new();
    for table in all {
        validate_table(raw, table)?;
        let id = table["id"].as_str().unwrap_or_default().to_string();
        ensure!(
            ids.insert(id.clone()),
            "[invalid-input] duplicate table ID {id}"
        );
    }
    placed_tables_exist(raw, &ids)
}

fn placed_tables_exist(raw: &Value, ids: &BTreeSet<String>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for story in list(raw.pointer("/layout").unwrap_or(&Value::Null), "stories") {
        for paragraph in list(story, "paragraphs") {
            for run in list(paragraph, "runs") {
                if let Some(table) = run.get("table").and_then(Value::as_str) {
                    ensure!(
                        ids.contains(table),
                        "[missing-resource] story {}: table {table} does not exist",
                        story["id"].as_str().unwrap_or_default()
                    );
                    ensure!(
                        seen.insert(table.to_string()),
                        "[invalid-input] table {table} is placed more than once"
                    );
                }
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------ CSV / TSV

/// Parse RFC 4180 records with a configurable delimiter.
pub fn parse_delimited(text: &str, delimiter: char) -> Result<Vec<Vec<String>>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut in_quotes = false;
    let mut field_started = false;
    let mut line = 1usize;
    let finish_row =
        |rows: &mut Vec<Vec<String>>, row: &mut Vec<String>, line: usize| -> Result<()> {
            ensure!(
                rows.len() < MAX_ROWS,
                "[limit-exceeded] import has more than {MAX_ROWS} rows (line {line})"
            );
            ensure!(
                row.len() <= MAX_COLUMNS,
                "[limit-exceeded] line {line}: more than {MAX_COLUMNS} columns"
            );
            rows.push(std::mem::take(row));
            Ok(())
        };
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
        } else if c == '"' {
            ensure!(
                !field_started,
                "[malformed-resource] line {line}: a quote may only start a field (RFC 4180)"
            );
            in_quotes = true;
            field_started = true;
        } else if c == delimiter {
            row.push(std::mem::take(&mut field));
            field_started = false;
            ensure!(
                row.len() <= MAX_COLUMNS,
                "[limit-exceeded] line {line}: more than {MAX_COLUMNS} columns"
            );
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            finish_row(&mut rows, &mut row, line)?;
            field_started = false;
            line += 1;
        } else {
            field.push(c);
            field_started = true;
        }
        ensure!(
            field.len() <= MAX_CELL_BYTES,
            "[limit-exceeded] line {line}: a cell exceeds {MAX_CELL_BYTES} bytes"
        );
    }
    ensure!(
        !in_quotes,
        "[malformed-resource] the quoted field that opened before line {line} is never closed"
    );
    if field_started || !field.is_empty() || !row.is_empty() {
        row.push(field);
        finish_row(&mut rows, &mut row, line)?;
    }
    Ok(rows)
}

fn build_from_grid(
    id: &str,
    grid: &[Vec<String>],
    header_rows: usize,
    column_width: f64,
) -> Result<(Value, Vec<Value>)> {
    ensure!(!grid.is_empty(), "[invalid-input] the source has no rows");
    ensure!(
        column_width.is_finite() && column_width > 0.0,
        "[invalid-input] --column-width must be positive"
    );
    let width = grid.iter().map(Vec::len).max().unwrap_or(1).max(1);
    ensure!(
        width <= MAX_COLUMNS && grid.len() <= MAX_ROWS,
        "[limit-exceeded] {} rows x {width} columns exceeds {MAX_ROWS} x {MAX_COLUMNS}",
        grid.len()
    );
    ensure!(
        header_rows < grid.len(),
        "[invalid-input] --header-rows {header_rows} leaves no body row"
    );
    let mut cells = Vec::new();
    let mut ragged = Vec::new();
    for (r, row) in grid.iter().enumerate() {
        if row.len() != width {
            ragged.push(json!({"row": r, "columns": row.len(), "expected": width}));
        }
        for (c, text) in row.iter().enumerate() {
            let text = normalized(text, &format!("row {r} column {c}"))?;
            if !text.is_empty() {
                cells.push(json!({"row": r, "col": c, "paragraphs": paragraphs_from(&text)}));
            }
        }
    }
    ensure!(
        cells.len() <= MAX_CELLS,
        "[limit-exceeded] {} cells exceeds the limit of {MAX_CELLS}",
        cells.len()
    );
    let mut table = json!({
        "id": id,
        "columns": (0..width).map(|_| json!({"width": column_width})).collect::<Vec<_>>(),
        "rows": (0..grid.len()).map(|_| json!({})).collect::<Vec<_>>(),
        "cells": cells,
    });
    if header_rows > 0 {
        table["header_rows"] = json!(header_rows);
    }
    Ok((table, ragged))
}

// ------------------------------------------------------------ planning

/// Estimated natural heights of every row.
fn row_heights(raw: &Value, table: &Value) -> Vec<f64> {
    let columns: Vec<f64> = list(table, "columns")
        .iter()
        .map(|c| c["width"].as_f64().unwrap_or(72.0))
        .collect();
    let rows = list(table, "rows");
    let mut heights: Vec<f64> = rows
        .iter()
        .map(|r| {
            r.get("height")
                .and_then(Value::as_f64)
                .or_else(|| r.get("min_height").and_then(Value::as_f64))
                .unwrap_or(0.0)
        })
        .collect();
    let fixed: Vec<bool> = rows.iter().map(|r| r.get("height").is_some()).collect();
    for cell in list(table, "cells") {
        let (r, c) = cell_pos(cell);
        if span(cell, "row_span") != 1 || fixed.get(r).copied().unwrap_or(false) {
            continue;
        }
        let spanned: f64 = columns[c..(c + span(cell, "col_span")).min(columns.len())]
            .iter()
            .sum();
        let inset: Vec<f64> = cell
            .get("inset")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .filter(|a: &Vec<f64>| a.len() == 4)
            .unwrap_or_else(|| vec![0.0; 4]);
        let rotated = matches!(cell["rotation"].as_u64(), Some(90) | Some(270));
        let mut content = 0.0;
        for paragraph in list(cell, "paragraphs") {
            let (size, leading) = crate::stories::paragraph_metrics(raw, paragraph);
            let chars: usize = list(paragraph, "runs")
                .iter()
                .map(|run| {
                    run.get("text")
                        .and_then(Value::as_str)
                        .map_or(0, |t| t.chars().count())
                })
                .sum();
            if rotated {
                content += (chars as f64 * 0.5 * size).max(leading);
            } else {
                let per_line = (((spanned - inset[1] - inset[3]).max(size) / (0.5 * size)).floor()
                    as usize)
                    .max(1);
                content += chars.div_ceil(per_line).max(1) as f64 * leading;
            }
        }
        if content == 0.0 && cell.get("graphic").is_some() {
            content = 14.4;
        }
        let total = round(content + inset[0] + inset[2]);
        if total > heights[r] {
            heights[r] = total;
        }
    }
    heights
}

fn plan(raw: &Value, table: &Value, heights_arg: &[f64]) -> Result<Value> {
    let id = table["id"].as_str().unwrap_or_default();
    let heights = row_heights(raw, table);
    let header = table["header_rows"].as_u64().unwrap_or(0) as usize;
    let footer = table["footer_rows"].as_u64().unwrap_or(0) as usize;
    let rows = heights.len();
    let mut frames: Vec<f64> = heights_arg.to_vec();
    let mut source = "argument";
    if frames.is_empty() {
        if let Some((s, _, _)) = table_run_site(raw, id) {
            let story = list(raw.pointer("/layout").unwrap_or(&Value::Null), "stories")[s]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            frames = crate::stories::placements(raw, &story)
                .iter()
                .filter_map(|p| crate::stories::frame_node(raw, &p.frame))
                .map(|n| {
                    let b = crate::stories::frame_box(n);
                    (b.height - b.inset[0] - b.inset[2]).max(0.0)
                })
                .collect();
            source = "story-frames";
        }
    }
    ensure!(
        !frames.is_empty() && frames.iter().all(|h| h.is_finite() && *h > 0.0),
        "[invalid-input] table {id} is not placed in a story with frames; pass --heights"
    );
    let header_height: f64 = heights[..header].iter().sum();
    let footer_height: f64 = heights[rows - footer..].iter().sum();
    let body: Vec<usize> = (header..rows - footer).collect();
    let spans_of = |r: usize| -> usize {
        list(table, "cells")
            .iter()
            .filter(|c| cell_pos(c).0 == r)
            .map(|c| span(c, "row_span"))
            .max()
            .unwrap_or(1)
    };
    let mut pieces: Vec<Value> = Vec::new();
    let mut notes: Vec<Value> = Vec::new();
    let mut frame = 0usize;
    let mut cursor = 0usize;
    let mut overset = false;
    while cursor < body.len() {
        let Some(&available) = frames.get(frame) else {
            overset = true;
            break;
        };
        let chrome = header_height + footer_height;
        let mut used = chrome;
        let first = cursor;
        let mut fit = 0usize;
        while cursor < body.len() {
            // Keep a row-span group together where it fits a whole frame.
            let group = spans_of(body[cursor]).min(body.len() - cursor);
            let need: f64 = (0..group).map(|k| heights[body[cursor + k]]).sum();
            if used + need <= available + 1e-6 {
                used += need;
                cursor += group;
                fit += group;
                continue;
            }
            if group > 1 && fit > 0 {
                notes.push(json!({
                    "kind": "row-span-moved", "row": body[cursor],
                    "reason": "spanned rows kept together on the next frame",
                }));
                break;
            }
            if fit == 0 {
                // A single row taller than the frame is split at a line boundary.
                let usable = (available - chrome).max(0.0);
                if heights[body[cursor]] > usable && group == 1 {
                    notes.push(json!({
                        "kind": "row-split", "row": body[cursor],
                        "height": heights[body[cursor]], "available": round(usable),
                    }));
                    used += usable.min(heights[body[cursor]]);
                    cursor += 1;
                    fit += 1;
                } else if group > 1 {
                    notes.push(json!({
                        "kind": "row-span-split", "row": body[cursor],
                        "reason": "spanned rows exceed a full frame",
                    }));
                    cursor += group;
                    fit += group;
                    used += need.min(available);
                }
            }
            break;
        }
        pieces.push(json!({
            "frame_index": frame,
            "body_rows": [first, cursor],
            "header_repeated": header > 0 && !pieces.is_empty(),
            "footer_repeated": footer > 0,
            "height": round(used),
            "available": round(available),
        }));
        frame += 1;
        if fit == 0 {
            // No progress possible; avoid looping on an unusable frame.
            if frame >= frames.len() {
                overset = true;
                break;
            }
        }
    }
    Ok(json!({
        "table": id,
        "estimated": true,
        "source": source,
        "row_heights": heights.iter().map(|h| round(*h)).collect::<Vec<_>>(),
        "header_rows": header, "footer_rows": footer,
        "pieces": pieces,
        "notes": notes,
        "overset": overset,
        "frames_used": pieces.len(),
    }))
}

// ------------------------------------------------------------ run

fn pagination(rows: &[Value], offset: usize, limit: usize) -> Result<(Vec<Value>, Value)> {
    ensure!(
        (1..=1000).contains(&limit),
        "[invalid-input] --limit must be between 1 and 1000"
    );
    let slice: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
    let meta = json!({
        "matches": rows.len(), "returned": slice.len(), "offset": offset,
        "limit": limit, "has_more": offset + slice.len() < rows.len(),
    });
    Ok((slice, meta))
}

fn story_paragraph_mut<'a>(
    raw: &'a mut Value,
    story: &str,
    paragraph: usize,
) -> Result<&'a mut Value> {
    let s = raw
        .pointer_mut("/layout/stories")
        .and_then(Value::as_array_mut)
        .and_then(|all| all.iter_mut().find(|s| s["id"] == story))
        .with_context(|| format!("[missing-resource] story not found: {story}"))?;
    let paragraphs = s
        .get_mut("paragraphs")
        .and_then(Value::as_array_mut)
        .context("story paragraphs are missing")?;
    let count = paragraphs.len();
    paragraphs.get_mut(paragraph).with_context(|| {
        format!(
            "[invalid-input] paragraph {paragraph} is outside story {story} ({count} paragraphs)"
        )
    })
}

fn paragraph_plain(paragraph: &Value) -> Result<String> {
    let mut out = String::new();
    for run in list(paragraph, "runs") {
        if let Some(t) = run.get("text").and_then(Value::as_str) {
            out.push_str(t);
        } else if run.get("special").and_then(Value::as_str) == Some("tab") {
            out.push('\t');
        } else {
            bail!(
                "[unsupported-capability] a paragraph with a run other than text or tab cannot be converted without losing it"
            );
        }
    }
    Ok(out)
}

/// Runs for converted text: tabs become tab special runs, since story text
/// carries no control characters.
fn runs_from_plain(line: &str) -> Vec<Value> {
    let mut runs = Vec::new();
    for (i, part) in line.split('\t').enumerate() {
        if i > 0 {
            runs.push(json!({"special": "tab"}));
        }
        if !part.is_empty() {
            runs.push(json!({"text": part}));
        }
    }
    runs
}

fn insert_table(
    raw: &mut Value,
    id: &str,
    story: &str,
    paragraph: usize,
    at: Option<usize>,
) -> Result<()> {
    ensure!(
        table_run_site(raw, id).is_none(),
        "[invalid-input] table {id} is already placed; each table is placed once"
    );
    let target = story_paragraph_mut(raw, story, paragraph)?;
    crate::wrap::insert_anchor_run(target, at, json!({"table": id}))
}

fn pattern(text: &str) -> Vec<String> {
    text.split(',').map(|s| s.trim().to_string()).collect()
}

fn parse4(text: &str, what: &str) -> Result<Vec<f64>> {
    let values: Result<Vec<f64>, _> = text.split(',').map(|v| v.trim().parse::<f64>()).collect();
    let values =
        values.map_err(|_| anyhow::anyhow!("[invalid-input] {what} must be four numbers"))?;
    ensure!(
        values.len() == 4 && values.iter().all(|v| v.is_finite() && *v >= 0.0),
        "[invalid-input] {what} must be top,right,bottom,left non-negative numbers"
    );
    Ok(values)
}

/// Execute one table action. Returns the result and whether it was read only.
pub fn run(raw: &mut Value, action: TableAction) -> Result<(Value, bool)> {
    require_v9(raw)?;
    match action {
        TableAction::Create {
            id,
            rows,
            columns,
            column_width,
            row_height,
            header_rows,
            footer_rows,
            style,
        } => {
            check_id(&id)?;
            ensure!(
                (1..=MAX_ROWS).contains(&rows) && (1..=MAX_COLUMNS).contains(&columns),
                "[limit-exceeded] --rows must be 1-{MAX_ROWS} and --columns 1-{MAX_COLUMNS}"
            );
            ensure!(
                column_width.is_finite() && column_width > 0.0,
                "[invalid-input] --column-width must be positive"
            );
            ensure!(
                tables(raw).len() < MAX_TABLES,
                "[limit-exceeded] a document holds at most {MAX_TABLES} tables"
            );
            ensure!(
                index_of(raw, &id).is_err(),
                "[invalid-input] table {id} already exists"
            );
            let mut table = json!({
                "id": id,
                "columns": (0..columns).map(|_| json!({"width": column_width})).collect::<Vec<_>>(),
                "rows": (0..rows).map(|_| match row_height {
                    Some(h) => json!({"height": h}),
                    None => json!({}),
                }).collect::<Vec<_>>(),
                "cells": [],
            });
            if header_rows > 0 {
                table["header_rows"] = json!(header_rows);
            }
            if footer_rows > 0 {
                table["footer_rows"] = json!(footer_rows);
            }
            if let Some(style) = style {
                table["style"] = json!(style);
            }
            match raw["layout"]
                .get_mut("tables")
                .and_then(Value::as_array_mut)
            {
                Some(all) => all.push(table),
                None => raw["layout"]["tables"] = json!([table]),
            }
            Ok((
                json!({"ok": true, "operation": "table-create", "table": id}),
                false,
            ))
        }
        TableAction::Show { id, offset, limit } => {
            let table = &tables(raw)[index_of(raw, &id)?];
            let (cells, meta) = pagination(list(table, "cells"), offset, limit)?;
            let (r, c) = dims(table);
            let placed = table_run_site(raw, &id).map(|(s, p, _)| {
                json!({
                    "story": raw["layout"]["stories"][s]["id"],
                    "paragraph": p,
                })
            });
            let mut head = table.clone();
            head.as_object_mut().map(|o| o.remove("cells"));
            Ok((
                json!({"table": head, "rows": r, "columns": c, "placed": placed, "cells": cells, "page": meta}),
                true,
            ))
        }
        TableAction::List { offset, limit } => {
            let rows: Vec<Value> = tables(raw)
                .iter()
                .map(|t| {
                    let (r, c) = dims(t);
                    let id = t["id"].as_str().unwrap_or_default();
                    json!({
                        "id": id, "rows": r, "columns": c,
                        "cells": list(t, "cells").len(),
                        "header_rows": t["header_rows"].as_u64().unwrap_or(0),
                        "footer_rows": t["footer_rows"].as_u64().unwrap_or(0),
                        "placed": table_run_site(raw, id).is_some(),
                    })
                })
                .collect();
            let (slice, meta) = pagination(&rows, offset, limit)?;
            let mut out = meta;
            out["tables"] = json!(slice);
            Ok((out, true))
        }
        TableAction::Delete { id } => {
            let index = index_of(raw, &id)?;
            ensure!(
                table_run_site(raw, &id).is_none(),
                "[invalid-input] table {id} is placed in a story; use to-text or remove the run first"
            );
            raw["layout"]["tables"]
                .as_array_mut()
                .context("tables vanished")?
                .remove(index);
            Ok((
                json!({"ok": true, "operation": "table-delete", "table": id}),
                false,
            ))
        }
        TableAction::Set {
            id,
            style,
            header_rows,
            footer_rows,
            row_pattern,
            column_pattern,
        } => {
            let table = table_mut(raw, &id)?;
            let object = table.as_object_mut().context("table is not a map")?;
            if let Some(style) = style {
                object.insert("style".into(), json!(style));
            }
            if let Some(n) = header_rows {
                object.insert("header_rows".into(), json!(n));
            }
            if let Some(n) = footer_rows {
                object.insert("footer_rows".into(), json!(n));
            }
            if let Some(p) = row_pattern {
                object.insert("row_pattern".into(), json!(pattern(&p)));
            }
            if let Some(p) = column_pattern {
                object.insert("column_pattern".into(), json!(pattern(&p)));
            }
            Ok((
                json!({"ok": true, "operation": "table-set", "table": id}),
                false,
            ))
        }
        TableAction::Place {
            id,
            story,
            paragraph,
            at,
        } => {
            index_of(raw, &id)?;
            insert_table(raw, &id, &story, paragraph, at)?;
            Ok((
                json!({"ok": true, "operation": "table-place", "table": id, "story": story, "paragraph": paragraph}),
                false,
            ))
        }
        TableAction::CellSet {
            id,
            row,
            col,
            text,
            inset,
            align,
            vertical_align,
            rotation,
            fill,
            stroke_color,
            stroke_width,
            graphic,
            style,
        } => {
            let snapshot = tables(raw)[index_of(raw, &id)?].clone();
            let (nr, nc) = dims(&snapshot);
            ensure!(
                row < nr && col < nc,
                "[invalid-input] cell ({row}, {col}) is outside the {nr} x {nc} table {id}"
            );
            if let Some(i) = covering(&snapshot, row, col) {
                ensure!(
                    cell_pos(&snapshot["cells"][i]) == (row, col),
                    "[invalid-input] cell ({row}, {col}) is covered by the merged cell at {:?}; address its top-left cell",
                    cell_pos(&snapshot["cells"][i])
                );
            }
            if let Some(g) = &graphic {
                ensure!(
                    node_exists(raw, g),
                    "[missing-resource] graphic {g} does not exist"
                );
            }
            if let Some(s) = &style {
                ensure!(
                    style_exists(raw, "cell", s),
                    "[missing-resource] cell style `{s}` does not exist"
                );
            }
            let paragraphs = match &text {
                Some(t) => Some(paragraphs_from(&normalized(
                    &t.replace("\\n", "\n"),
                    &format!("cell ({row}, {col})"),
                )?)),
                None => None,
            };
            let inset = match inset {
                Some(i) => Some(parse4(&i, "--inset")?),
                None => None,
            };
            if let Some(a) = &align {
                one_of(a, "--align", ALIGNS)?;
            }
            if let Some(a) = &vertical_align {
                one_of(a, "--vertical-align", VALIGNS)?;
            }
            if let Some(r) = rotation {
                ensure!(
                    ROTATIONS.contains(&r),
                    "[invalid-input] --rotation must be 0, 90, 180 or 270"
                );
            }
            if let Some(w) = stroke_width {
                ensure!(
                    w.is_finite() && (0.0..=1000.0).contains(&w),
                    "[invalid-input] --stroke-width must be between 0 and 1000"
                );
            }
            let table = table_mut(raw, &id)?;
            let cells = table
                .get_mut("cells")
                .and_then(Value::as_array_mut)
                .context("table cells are missing")?;
            let index = match cells.iter().position(|c| cell_pos(c) == (row, col)) {
                Some(i) => i,
                None => {
                    ensure!(
                        cells.len() < MAX_CELLS,
                        "[limit-exceeded] table {id} holds at most {MAX_CELLS} cells"
                    );
                    cells.push(json!({"row": row, "col": col}));
                    cells.len() - 1
                }
            };
            let cell = cells[index].as_object_mut().context("cell is not a map")?;
            if let Some(p) = paragraphs {
                cell.insert("paragraphs".into(), json!(p));
            }
            if let Some(i) = inset {
                cell.insert("inset".into(), json!(i));
            }
            if let Some(a) = align {
                cell.insert("align".into(), json!(a));
            }
            if let Some(a) = vertical_align {
                cell.insert("vertical_align".into(), json!(a));
            }
            if let Some(r) = rotation {
                cell.insert("rotation".into(), json!(r));
            }
            if let Some(f) = fill {
                cell.insert("fill".into(), json!(f));
            }
            if stroke_color.is_some() || stroke_width.is_some() {
                let mut stroke = cell
                    .get("stroke")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                if let Some(c) = stroke_color {
                    stroke.insert("color".into(), json!(c));
                }
                if let Some(w) = stroke_width {
                    stroke.insert("width".into(), json!(w));
                }
                cell.insert("stroke".into(), Value::Object(stroke));
            }
            if let Some(g) = graphic {
                cell.insert("graphic".into(), json!(g));
            }
            if let Some(s) = style {
                cell.insert("style".into(), json!(s));
            }
            sort_cells(table);
            Ok((
                json!({"ok": true, "operation": "table-cell-set", "table": id, "row": row, "col": col}),
                false,
            ))
        }
        TableAction::Merge {
            id,
            row,
            col,
            rows,
            cols,
        } => {
            let snapshot = tables(raw)[index_of(raw, &id)?].clone();
            let (nr, nc) = dims(&snapshot);
            ensure!(
                rows >= 1 && cols >= 1 && rows * cols > 1,
                "[invalid-input] a merge needs at least two cells"
            );
            ensure!(
                row + rows <= nr && col + cols <= nc,
                "[invalid-input] merge range leaves the {nr} x {nc} table {id}"
            );
            let header = snapshot["header_rows"].as_u64().unwrap_or(0) as usize;
            let footer = snapshot["footer_rows"].as_u64().unwrap_or(0) as usize;
            let region = |r: usize| {
                if r < header {
                    0
                } else if r >= nr - footer {
                    2
                } else {
                    1
                }
            };
            ensure!(
                region(row) == region(row + rows - 1),
                "[invalid-input] a merge cannot cross the header/body/footer boundary"
            );
            // The range must not cut through an existing merged cell.
            for cell in list(&snapshot, "cells") {
                let (r, c) = cell_pos(cell);
                let (rs, cs) = (span(cell, "row_span"), span(cell, "col_span"));
                let overlaps = r < row + rows && r + rs > row && c < col + cols && c + cs > col;
                let inside = r >= row && r + rs <= row + rows && c >= col && c + cs <= col + cols;
                ensure!(
                    !overlaps || inside,
                    "[invalid-input] the range cuts through the merged cell at ({r}, {c}); split it first"
                );
            }
            let table = table_mut(raw, &id)?;
            let cells = table
                .get_mut("cells")
                .and_then(Value::as_array_mut)
                .context("table cells are missing")?;
            let mut merged_paragraphs: Vec<Value> = Vec::new();
            let mut absorbed = 0usize;
            let mut keep: Vec<Value> = Vec::new();
            let mut top_left: Option<Value> = None;
            for cell in cells.drain(..) {
                let (r, c) = cell_pos(&cell);
                if r >= row && r < row + rows && c >= col && c < col + cols {
                    absorbed += 1;
                    let text = cell_text(&cell);
                    if !text.is_empty() {
                        merged_paragraphs.extend(list(&cell, "paragraphs").iter().cloned());
                    }
                    if (r, c) == (row, col) {
                        top_left = Some(cell);
                    }
                } else {
                    keep.push(cell);
                }
            }
            let mut anchor = top_left.unwrap_or_else(|| json!({"row": row, "col": col}));
            let object = anchor.as_object_mut().context("cell is not a map")?;
            object.insert("row_span".into(), json!(rows));
            object.insert("col_span".into(), json!(cols));
            if merged_paragraphs.is_empty() {
                object.remove("paragraphs");
            } else {
                object.insert("paragraphs".into(), json!(merged_paragraphs));
            }
            keep.push(anchor);
            *cells = keep;
            sort_cells(table);
            Ok((
                json!({"ok": true, "operation": "table-merge", "table": id, "row": row, "col": col,
                       "row_span": rows, "col_span": cols, "cells_merged": absorbed}),
                false,
            ))
        }
        TableAction::Split { id, row, col } => {
            let table = table_mut(raw, &id)?;
            let index = find_cell(table, row, col).with_context(|| {
                format!("[invalid-input] ({row}, {col}) is not the top-left cell of a merged cell")
            })?;
            let cell = table["cells"][index]
                .as_object_mut()
                .context("cell is not a map")?;
            ensure!(
                cell.get("row_span").and_then(Value::as_u64).unwrap_or(1) > 1
                    || cell.get("col_span").and_then(Value::as_u64).unwrap_or(1) > 1,
                "[invalid-input] cell ({row}, {col}) is not merged"
            );
            let (rs, cs) = (
                cell.remove("row_span")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1),
                cell.remove("col_span")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1),
            );
            Ok((
                json!({"ok": true, "operation": "table-split", "table": id, "row": row, "col": col,
                       "row_span": rs, "col_span": cs}),
                false,
            ))
        }
        TableAction::AddRow { id, at, count } => {
            let table = table_mut(raw, &id)?;
            let (nr, _) = dims(table);
            ensure!(
                at <= nr,
                "[invalid-input] --at {at} is beyond the {nr} rows of table {id}"
            );
            ensure!(
                count >= 1 && nr + count <= MAX_ROWS,
                "[limit-exceeded] a table holds at most {MAX_ROWS} rows"
            );
            let header = table["header_rows"].as_u64().unwrap_or(0) as usize;
            let footer = table["footer_rows"].as_u64().unwrap_or(0) as usize;
            let template = table["rows"]
                .get(at.min(nr - 1))
                .cloned()
                .unwrap_or_else(|| json!({}));
            {
                let rows = table["rows"].as_array_mut().context("rows missing")?;
                for _ in 0..count {
                    rows.insert(at, template.clone());
                }
            }
            if let Some(cells) = table.get_mut("cells").and_then(Value::as_array_mut) {
                for cell in cells.iter_mut() {
                    let (r, _) = cell_pos(cell);
                    let rs = span(cell, "row_span");
                    if r >= at {
                        cell["row"] = json!(r + count);
                    } else if r + rs > at {
                        // Inserted inside a merged cell: it grows.
                        cell["row_span"] = json!(rs + count);
                    }
                }
            }
            // A row inserted inside the header or footer belongs to it.
            if at > 0 && at < header {
                table["header_rows"] = json!(header + count);
            } else if footer > 0 && at > nr - footer && at < nr {
                table["footer_rows"] = json!(footer + count);
            }
            sort_cells(table);
            Ok((
                json!({"ok": true, "operation": "table-add-row", "table": id, "at": at, "count": count}),
                false,
            ))
        }
        TableAction::RemoveRow { id, at, discard } => {
            let table = table_mut(raw, &id)?;
            let (nr, _) = dims(table);
            ensure!(
                at < nr,
                "[invalid-input] --at {at} is outside the {nr} rows of table {id}"
            );
            ensure!(nr > 1, "[invalid-input] a table keeps at least one row");
            let mut lost = 0usize;
            for cell in list(table, "cells") {
                let (r, c) = cell_pos(cell);
                let rs = span(cell, "row_span");
                if r == at && rs > 1 {
                    bail!("[invalid-input] row {at} starts the merged cell at ({r}, {c}); split it first");
                }
                if r == at && has_content(cell) {
                    lost += 1;
                }
            }
            ensure!(
                lost == 0 || discard,
                "[invalid-input] row {at} holds {lost} cells with content; pass --discard to remove it"
            );
            let header = table["header_rows"].as_u64().unwrap_or(0) as usize;
            let footer = table["footer_rows"].as_u64().unwrap_or(0) as usize;
            table["rows"]
                .as_array_mut()
                .context("rows missing")?
                .remove(at);
            if let Some(cells) = table.get_mut("cells").and_then(Value::as_array_mut) {
                cells.retain(|c| cell_pos(c).0 != at);
                for cell in cells.iter_mut() {
                    let (r, _) = cell_pos(cell);
                    let rs = span(cell, "row_span");
                    if r > at {
                        cell["row"] = json!(r - 1);
                    } else if r + rs > at {
                        let new = rs - 1;
                        if new <= 1 {
                            cell.as_object_mut().map(|o| o.remove("row_span"));
                        } else {
                            cell["row_span"] = json!(new);
                        }
                    }
                }
            }
            if at < header {
                table["header_rows"] = json!(header - 1);
            } else if at >= nr - footer {
                table["footer_rows"] = json!(footer - 1);
            }
            sort_cells(table);
            Ok((
                json!({"ok": true, "operation": "table-remove-row", "table": id, "at": at, "cells_discarded": lost}),
                false,
            ))
        }
        TableAction::AddColumn {
            id,
            at,
            count,
            width,
        } => {
            ensure!(
                width.is_finite() && width > 0.0,
                "[invalid-input] --width must be positive"
            );
            let table = table_mut(raw, &id)?;
            let (_, nc) = dims(table);
            ensure!(
                at <= nc,
                "[invalid-input] --at {at} is beyond the {nc} columns of table {id}"
            );
            ensure!(
                count >= 1 && nc + count <= MAX_COLUMNS,
                "[limit-exceeded] a table holds at most {MAX_COLUMNS} columns"
            );
            {
                let columns = table["columns"].as_array_mut().context("columns missing")?;
                for _ in 0..count {
                    columns.insert(at, json!({"width": width}));
                }
            }
            if let Some(cells) = table.get_mut("cells").and_then(Value::as_array_mut) {
                for cell in cells.iter_mut() {
                    let (_, c) = cell_pos(cell);
                    let cs = span(cell, "col_span");
                    if c >= at {
                        cell["col"] = json!(c + count);
                    } else if c + cs > at {
                        cell["col_span"] = json!(cs + count);
                    }
                }
            }
            sort_cells(table);
            Ok((
                json!({"ok": true, "operation": "table-add-column", "table": id, "at": at, "count": count}),
                false,
            ))
        }
        TableAction::RemoveColumn { id, at, discard } => {
            let table = table_mut(raw, &id)?;
            let (_, nc) = dims(table);
            ensure!(
                at < nc,
                "[invalid-input] --at {at} is outside the {nc} columns of table {id}"
            );
            ensure!(nc > 1, "[invalid-input] a table keeps at least one column");
            let mut lost = 0usize;
            for cell in list(table, "cells") {
                let (r, c) = cell_pos(cell);
                if c == at && span(cell, "col_span") > 1 {
                    bail!("[invalid-input] column {at} starts the merged cell at ({r}, {c}); split it first");
                }
                if c == at && has_content(cell) {
                    lost += 1;
                }
            }
            ensure!(
                lost == 0 || discard,
                "[invalid-input] column {at} holds {lost} cells with content; pass --discard to remove it"
            );
            table["columns"]
                .as_array_mut()
                .context("columns missing")?
                .remove(at);
            if let Some(cells) = table.get_mut("cells").and_then(Value::as_array_mut) {
                cells.retain(|c| cell_pos(c).1 != at);
                for cell in cells.iter_mut() {
                    let (_, c) = cell_pos(cell);
                    let cs = span(cell, "col_span");
                    if c > at {
                        cell["col"] = json!(c - 1);
                    } else if c + cs > at {
                        let new = cs - 1;
                        if new <= 1 {
                            cell.as_object_mut().map(|o| o.remove("col_span"));
                        } else {
                            cell["col_span"] = json!(new);
                        }
                    }
                }
            }
            sort_cells(table);
            Ok((
                json!({"ok": true, "operation": "table-remove-column", "table": id, "at": at, "cells_discarded": lost}),
                false,
            ))
        }
        TableAction::FromText {
            id,
            text,
            story,
            paragraph,
            count,
            column_separator,
            row_separator,
            column_width,
        } => {
            check_id(&id)?;
            ensure!(
                index_of(raw, &id).is_err(),
                "[invalid-input] table {id} already exists"
            );
            let col_sep = unescape(&column_separator)?;
            let row_sep = unescape(&row_separator)?;
            let (grid, site): (Vec<Vec<String>>, Option<StorySite>) = match (&text, &story) {
                (Some(t), None) => {
                    ensure!(
                        t.len() <= MAX_IMPORT_BYTES,
                        "[limit-exceeded] text exceeds {MAX_IMPORT_BYTES} bytes"
                    );
                    let grid = t
                        .split(row_sep.as_str())
                        .filter(|l| !l.is_empty())
                        .map(|l| l.split(col_sep.as_str()).map(str::to_string).collect())
                        .collect();
                    (grid, None)
                }
                (None, Some(story)) => {
                    let p = paragraph
                        .context("[invalid-input] --paragraph is required with --story")?;
                    let n = count.unwrap_or(1);
                    ensure!(n >= 1, "[invalid-input] --count must be at least 1");
                    let s = raw
                        .pointer("/layout/stories")
                        .and_then(Value::as_array)
                        .and_then(|all| all.iter().find(|s| s["id"] == story.as_str()))
                        .with_context(|| format!("[missing-resource] story not found: {story}"))?;
                    let paragraphs = list(s, "paragraphs");
                    ensure!(
                        p + n <= paragraphs.len(),
                        "[invalid-input] paragraphs {p}..{} are outside story {story} ({} paragraphs)",
                        p + n,
                        paragraphs.len()
                    );
                    let mut grid = Vec::new();
                    for paragraph in &paragraphs[p..p + n] {
                        let plain = paragraph_plain(paragraph)?;
                        grid.push(plain.split(col_sep.as_str()).map(str::to_string).collect());
                    }
                    (grid, Some((story.clone(), p, n)))
                }
                _ => bail!("[invalid-input] pass either --text or --story with --paragraph"),
            };
            let (table, ragged) = build_from_grid(&id, &grid, 0, column_width)?;
            let (rows, columns) = dims(&table);
            ensure!(
                tables(raw).len() < MAX_TABLES,
                "[limit-exceeded] a document holds at most {MAX_TABLES} tables"
            );
            match raw["layout"]
                .get_mut("tables")
                .and_then(Value::as_array_mut)
            {
                Some(all) => all.push(table),
                None => raw["layout"]["tables"] = json!([table]),
            }
            if let Some((story, p, n)) = site {
                let s = raw
                    .pointer_mut("/layout/stories")
                    .and_then(Value::as_array_mut)
                    .and_then(|all| all.iter_mut().find(|s| s["id"] == story.as_str()))
                    .context("story vanished")?;
                let paragraphs = s["paragraphs"]
                    .as_array_mut()
                    .context("paragraphs missing")?;
                let first = paragraphs[p].clone();
                paragraphs.drain(p..p + n);
                let mut host = first;
                host["runs"] = json!([{"table": id}]);
                paragraphs.insert(p, host);
            }
            Ok((
                json!({"ok": true, "operation": "table-from-text", "table": id,
                       "rows": rows, "columns": columns, "ragged_rows": ragged}),
                false,
            ))
        }
        TableAction::ToText {
            id,
            column_separator,
        } => {
            let sep = unescape(&column_separator)?;
            let table = tables(raw)[index_of(raw, &id)?].clone();
            let (nr, nc) = dims(&table);
            let (s, p, r) = table_run_site(raw, &id).with_context(|| {
                format!("[invalid-input] table {id} is not placed in a story; delete it instead")
            })?;
            let mut lines = Vec::new();
            for row in 0..nr {
                let mut fields = Vec::new();
                for col in 0..nc {
                    match covering(&table, row, col) {
                        Some(i) => {
                            let cell = &table["cells"][i];
                            if cell_pos(cell) == (row, col) {
                                ensure!(
                                    cell.get("graphic").is_none(),
                                    "[unsupported-capability] cell ({row}, {col}) holds a graphic that text cannot carry"
                                );
                                let text = cell_text(cell).replace('\n', "\u{2028}");
                                ensure!(
                                    !text.contains(sep.as_str()),
                                    "[invalid-input] cell ({row}, {col}) contains the column separator; choose another"
                                );
                                fields.push(text);
                            } else {
                                fields.push(String::new());
                            }
                        }
                        None => fields.push(String::new()),
                    }
                }
                lines.push(fields.join(sep.as_str()));
            }
            let paragraph = raw["layout"]["stories"][s]["paragraphs"][p].clone();
            let runs = list(&paragraph, "runs");
            ensure!(
                runs.len() == 1 && r == 0,
                "[invalid-input] table {id} shares its paragraph with other runs; move it into its own paragraph first"
            );
            let host = raw["layout"]["stories"][s]["paragraphs"]
                .as_array_mut()
                .context("paragraphs missing")?;
            host.remove(p);
            for (k, line) in lines.iter().enumerate() {
                let mut para = paragraph.clone();
                para["runs"] = json!(runs_from_plain(line));
                host.insert(p + k, para);
            }
            let index = index_of(raw, &id)?;
            raw["layout"]["tables"]
                .as_array_mut()
                .context("tables vanished")?
                .remove(index);
            Ok((
                json!({"ok": true, "operation": "table-to-text", "table": id, "paragraphs": lines.len()}),
                false,
            ))
        }
        TableAction::Import {
            id,
            file,
            format,
            header_rows,
            column_width,
        } => {
            check_id(&id)?;
            ensure!(
                index_of(raw, &id).is_err(),
                "[invalid-input] table {id} already exists"
            );
            let kind = match format {
                Some(f) => f,
                None => file
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default(),
            };
            let delimiter = match kind.as_str() {
                "csv" => ',',
                "tsv" => '\t',
                other => {
                    bail!("[invalid-input] unsupported import format `{other}`; use csv or tsv")
                }
            };
            let meta = std::fs::metadata(&file)
                .with_context(|| format!("[not-found] cannot read {}", file.display()))?;
            ensure!(
                meta.len() as usize <= MAX_IMPORT_BYTES,
                "[limit-exceeded] {} is {} bytes; the import limit is {MAX_IMPORT_BYTES}",
                file.display(),
                meta.len()
            );
            let bytes = std::fs::read(&file)
                .with_context(|| format!("[not-found] cannot read {}", file.display()))?;
            let text = String::from_utf8(bytes).map_err(|_| {
                anyhow::anyhow!("[invalid-input] {} is not valid UTF-8", file.display())
            })?;
            let grid = parse_delimited(&text, delimiter)?;
            let (table, ragged) = build_from_grid(&id, &grid, header_rows, column_width)?;
            let (rows, columns) = dims(&table);
            ensure!(
                tables(raw).len() < MAX_TABLES,
                "[limit-exceeded] a document holds at most {MAX_TABLES} tables"
            );
            match raw["layout"]
                .get_mut("tables")
                .and_then(Value::as_array_mut)
            {
                Some(all) => all.push(table),
                None => raw["layout"]["tables"] = json!([table]),
            }
            Ok((
                json!({"ok": true, "operation": "table-import", "table": id, "rows": rows,
                       "columns": columns, "ragged_rows": ragged}),
                false,
            ))
        }
        TableAction::Plan { id, heights } => {
            let table = &tables(raw)[index_of(raw, &id)?];
            Ok((plan(raw, table, &heights)?, true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4180_quotes_newlines_and_escapes() {
        let rows = parse_delimited("a,\"b,\"\"c\"\"\",d\r\n\"x\ny\",,z\n", ',').unwrap();
        assert_eq!(rows[0], vec!["a", "b,\"c\"", "d"]);
        assert_eq!(rows[1], vec!["x\ny", "", "z"]);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn malformed_and_oversized_input_is_refused() {
        assert!(parse_delimited("\"open", ',').is_err());
        assert!(parse_delimited("ab\"c\"", ',').is_err());
        let wide = vec!["x"; MAX_COLUMNS + 1].join(",");
        assert!(parse_delimited(&wide, ',').is_err());
    }

    #[test]
    fn tsv_uses_tabs_and_keeps_commas() {
        let rows = parse_delimited("a,b\tc\n", '\t').unwrap();
        assert_eq!(rows[0], vec!["a,b", "c"]);
    }
}

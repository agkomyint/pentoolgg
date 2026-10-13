//! v0.14.0 task 12: data merge, adaptive layout and alternate layouts
//! (`docs/layout-v1.md` section 12.1).
//!
//! * **Data merge** fills `{{field}}` placeholders in the stories of a template
//!   page and image placeholders (graphic frames) from bounded CSV, TSV or JSON
//!   records, one record per page or several per page in a grid, into a new
//!   document. The template document is never changed.
//! * **Adaptive layout** resizes one page and re-places its objects by one of
//!   four rules: `scale`, `recenter`, `guide` and `object`.
//! * **Alternate layouts** copy a page to a new size; each copied story is a
//!   *linked* story (`linked_to`) that `sync` refreshes from its source and
//!   that never overwrites local edits silently.
//!
//! Everything is planned in memory and committed through the shared
//! transaction or atomic-write paths. IDs derive from page and record numbers,
//! so the same input always produces the same bytes.
use crate::page::PageAction;
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const MAX_RECORDS: usize = crate::tables::MAX_ROWS;
pub const MAX_FIELDS: usize = crate::tables::MAX_COLUMNS;
pub const MAX_DATA_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_VALUE_BYTES: usize = 64 * 1024;
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CELLS: usize = 1000;
pub const MAX_ALTERNATES: usize = 500;
const MAX_ID_LEN: usize = 128;
const MAX_OVERFLOW_LISTED: usize = 1000;

// ------------------------------------------------------------------ data

/// Parsed records: ordered field names and one value list per record.
#[derive(Debug, Clone)]
pub struct Records {
    pub fields: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

fn field_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.chars().any(|c| c.is_control() || c == '{' || c == '}')
        && name.trim() == name
}

/// Parse CSV/TSV text (first row = field names) or a JSON array of flat
/// objects. Limits are checked before any document work.
pub fn parse_records(bytes: &[u8], format: &str, delimiter: Option<char>) -> Result<Records> {
    ensure!(
        bytes.len() <= MAX_DATA_BYTES,
        "[limit-exceeded] data file is {} bytes; the merge limit is {MAX_DATA_BYTES}",
        bytes.len()
    );
    let text = std::str::from_utf8(bytes).context("[invalid-input] data file is not UTF-8")?;
    let records = match format {
        "csv" | "tsv" => {
            let delimiter = delimiter.unwrap_or(if format == "tsv" { '\t' } else { ',' });
            let mut rows = crate::tables::parse_delimited(text, delimiter)?;
            rows.retain(|row| !(row.len() == 1 && row[0].is_empty()));
            ensure!(
                !rows.is_empty(),
                "[invalid-input] data file has no header row"
            );
            let header: Vec<String> = rows
                .remove(0)
                .into_iter()
                .map(|h| h.trim().into())
                .collect();
            Records {
                fields: header,
                rows,
            }
        }
        "json" => {
            let value: Value = serde_json::from_str(text)
                .context("[malformed-resource] data is not valid JSON")?;
            let items = value
                .as_array()
                .context("[invalid-input] JSON data must be an array of objects")?;
            ensure!(
                items.len() <= MAX_RECORDS,
                "[limit-exceeded] {} records exceed the limit of {MAX_RECORDS}",
                items.len()
            );
            let mut fields: Vec<String> = Vec::new();
            for (n, item) in items.iter().enumerate() {
                let object = item.as_object().with_context(|| {
                    format!("[invalid-input] record {} is not an object", n + 1)
                })?;
                for key in object.keys() {
                    if !fields.contains(key) {
                        fields.push(key.clone());
                    }
                }
            }
            let mut rows = Vec::with_capacity(items.len());
            for (n, item) in items.iter().enumerate() {
                let object = item.as_object().expect("checked");
                let mut row = Vec::with_capacity(fields.len());
                for field in &fields {
                    row.push(match object.get(field) {
                        None | Some(Value::Null) => String::new(),
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Number(n)) => n.to_string(),
                        Some(Value::Bool(b)) => b.to_string(),
                        Some(_) => bail!(
                            "[invalid-input] record {} field {field}: values must be strings, numbers, booleans or null",
                            n + 1
                        ),
                    });
                }
                rows.push(row);
            }
            Records { fields, rows }
        }
        other => bail!("[invalid-input] data format must be csv, tsv or json, not {other}"),
    };
    ensure!(
        (1..=MAX_FIELDS).contains(&records.fields.len()),
        "[limit-exceeded] {} fields must be between 1 and {MAX_FIELDS}",
        records.fields.len()
    );
    let mut seen = HashSet::new();
    for field in &records.fields {
        ensure!(
            field_name_ok(field),
            "[invalid-input] field name {field:?} must be 1-64 characters without braces or control characters"
        );
        ensure!(
            seen.insert(field.as_str()),
            "[invalid-input] duplicate field name {field}"
        );
    }
    ensure!(
        !records.rows.is_empty(),
        "[invalid-input] data file has a header but no records"
    );
    ensure!(
        records.rows.len() <= MAX_RECORDS,
        "[limit-exceeded] {} records exceed the limit of {MAX_RECORDS}",
        records.rows.len()
    );
    for (n, row) in records.rows.iter().enumerate() {
        ensure!(
            row.len() == records.fields.len(),
            "[invalid-input] record {} has {} values but the header has {} fields",
            n + 1,
            row.len(),
            records.fields.len()
        );
        for value in row {
            ensure!(
                value.len() <= MAX_VALUE_BYTES,
                "[limit-exceeded] record {}: a value is {} bytes; the limit is {MAX_VALUE_BYTES}",
                n + 1,
                value.len()
            );
        }
    }
    Ok(records)
}

// ------------------------------------------------------------ placeholders

/// Names of the `{{field}}` placeholders in a text, in order of appearance.
fn placeholders_in(text: &str, out: &mut BTreeSet<String>) {
    scan(text, |name| {
        out.insert(name.to_string());
        None
    });
}

/// Replace each well-formed `{{name}}`; `f` returns the replacement.
fn scan(text: &str, mut f: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) if field_name_ok(after[..end].trim()) => {
                let name = after[..end].trim();
                out.push_str(&rest[..start]);
                match f(name) {
                    Some(value) => out.push_str(&value),
                    None => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            _ => {
                out.push_str(&rest[..start + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn runs_mut(paragraph: &mut Value) -> Option<&mut Vec<Value>> {
    paragraph.get_mut("runs").and_then(Value::as_array_mut)
}

const UNSUPPORTED_RUNS: &[&str] = &["anchor", "note", "xref", "index"];

fn collect_placeholders(runs: &[Value], story: &str, out: &mut BTreeSet<String>) -> Result<()> {
    for run in runs {
        for key in UNSUPPORTED_RUNS {
            ensure!(
                run.get(key).is_none(),
                "[unsupported-capability] story {story} has a `{key}` run; merge templates may contain text, variables, links and conditions but not anchored objects, notes, cross-references or index markers. Move them out of the template story"
            );
        }
        if let Some(text) = run.get("text").and_then(Value::as_str) {
            placeholders_in(text, out);
        }
        if let Some(nested) = run.get("runs").and_then(Value::as_array) {
            collect_placeholders(nested, story, out)?;
        }
    }
    Ok(())
}

struct Sanitized(usize);

fn substitute_runs(
    runs: &mut [Value],
    fields: &HashMap<&str, &str>,
    sanitized: &mut Sanitized,
) -> Result<()> {
    for run in runs {
        if let Some(text) = run.get("text").and_then(Value::as_str) {
            if text.contains("{{") {
                let new = scan(text, |name| {
                    fields.get(name).map(|value| {
                        let cleaned: String = value
                            .chars()
                            .map(|c| {
                                if c.is_control() {
                                    sanitized.0 += 1;
                                    ' '
                                } else {
                                    c
                                }
                            })
                            .collect();
                        cleaned
                    })
                });
                run["text"] = json!(new.nfc().collect::<String>());
            }
        }
        if let Some(nested) = run.get_mut("runs").and_then(Value::as_array_mut) {
            substitute_runs(nested, fields, sanitized)?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ merge

#[derive(Debug, Clone)]
pub struct MergeOptions {
    pub template_page: Option<String>,
    pub records_per_page: usize,
    /// Grid columns and rows for several records per page.
    pub grid: Option<(usize, usize)>,
    /// Cell width and height (the offset between cells).
    pub cell: Option<(f64, f64)>,
    /// Graphic frame ID -> data field holding an image path.
    pub image_fields: Vec<(String, String)>,
    /// Keep the template page in the output (default: it is replaced).
    pub keep_template: bool,
    /// Directory image paths in the data are relative to.
    pub data_dir: PathBuf,
    /// Directory of the output document; external image paths are made
    /// relative to it.
    pub out_dir: PathBuf,
}

pub struct MergeOutcome {
    pub document: Value,
    pub report: Value,
}

fn pages_of(raw: &Value) -> &[Value] {
    raw.get("pages")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn node_id(node: &Value) -> &str {
    node.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn walk_nodes<'a>(nodes: &'a [Value], out: &mut Vec<&'a Value>) {
    for node in nodes {
        out.push(node);
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            walk_nodes(children, out);
        }
    }
}

fn page_nodes(page: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    for layer in page
        .get("layers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
            walk_nodes(nodes, &mut out);
        }
    }
    out
}

fn story_index(raw: &Value) -> HashMap<String, usize> {
    raw.get("layout")
        .and_then(|l| l.get("stories"))
        .and_then(Value::as_array)
        .map(|stories| {
            stories
                .iter()
                .enumerate()
                .map(|(i, s)| (node_id(s).to_string(), i))
                .collect()
        })
        .unwrap_or_default()
}

fn id_with(base: &str, suffix: &str) -> Result<String> {
    let id = format!("{base}-{suffix}");
    ensure!(
        id.chars().count() <= MAX_ID_LEN,
        "[limit-exceeded] merged ID {id:?} is longer than {MAX_ID_LEN} characters; shorten the template ID {base:?}"
    );
    Ok(id)
}

struct Cloner<'a> {
    suffix: String,
    dx: f64,
    dy: f64,
    template_frames: &'a HashSet<String>,
    story_map: &'a HashMap<String, String>,
    images: &'a HashMap<String, (String, Value)>,
    /// Image nodes created for graphic frames, appended after the frame.
    extra: Vec<Value>,
}

impl Cloner<'_> {
    fn clone_node(&mut self, node: &Value) -> Result<Value> {
        let mut copy = node.clone();
        self.rewrite(&mut copy)?;
        Ok(copy)
    }

    fn rewrite(&mut self, node: &mut Value) -> Result<()> {
        let old = node_id(node).to_string();
        if !old.is_empty() {
            node["id"] = json!(id_with(&old, &self.suffix)?);
        }
        let is_top = self.dx != 0.0 || self.dy != 0.0;
        let kind = node
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if kind == "text-frame" {
            if let Some(next) = node.get("next").and_then(Value::as_str).map(str::to_string) {
                ensure!(
                    self.template_frames.contains(&next),
                    "[unsupported-capability] frame {old} threads to {next}, which is not on the template page; keep each merged story on the template page"
                );
                node["next"] = json!(id_with(&next, &self.suffix)?);
            }
            node.as_object_mut().map(|o| o.remove("primary"));
            let story = node
                .get("story")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mapped = self.story_map.get(&story).with_context(|| {
                format!("[missing-resource] frame {old} references missing story {story}")
            })?;
            node["story"] = json!(id_with(mapped, &self.suffix)?);
        }
        if kind == "graphic-frame" {
            if let Some((digest, _)) = self.images.get(&old) {
                let image_id = id_with(&format!("{old}-image"), &self.suffix)?;
                let number = |key: &str| node.get(key).and_then(Value::as_f64).unwrap_or(0.0);
                let image = json!({
                    "kind": "image", "id": image_id, "asset": digest,
                    "x": number("x") + self.dx, "y": number("y") + self.dy,
                    "width": number("width"), "height": number("height"),
                    "fit": "cover", "position": [0.5, 0.5], "crop": [0, 0, 1, 1],
                    "opacity": 1, "blend_mode": "normal",
                    "transform": [1, 0, 0, 1, 0, 0], "operations": [],
                });
                node["content"] = json!(image_id);
                self.extra.push(image);
            } else if let Some(content) = node
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                node["content"] = json!(id_with(&content, &self.suffix)?);
            }
        }
        if is_top {
            shift(node, self.dx, self.dy);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            // Children are relative to the group's transform: they take new IDs
            // but never the cell offset again.
            let (dx, dy) = (self.dx, self.dy);
            self.dx = 0.0;
            self.dy = 0.0;
            for child in children.iter_mut() {
                self.rewrite(child)?;
            }
            self.dx = dx;
            self.dy = dy;
        }
        Ok(())
    }
}

/// Move a node by (dx, dy): its own x/y, or the transform translation for
/// nodes that carry only a transform or a non-identity one.
fn shift(node: &mut Value, dx: f64, dy: f64) {
    let identity = transform_is_identity(node);
    let has_xy = node.get("x").and_then(Value::as_f64).is_some()
        && node.get("y").and_then(Value::as_f64).is_some();
    if has_xy && identity {
        let x = node["x"].as_f64().unwrap_or(0.0);
        let y = node["y"].as_f64().unwrap_or(0.0);
        node["x"] = json!(x + dx);
        node["y"] = json!(y + dy);
        return;
    }
    let mut m = matrix(node);
    m[4] += dx;
    m[5] += dy;
    node["transform"] = json!(m);
}

fn matrix(node: &Value) -> [f64; 6] {
    let mut m = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    if let Some(a) = node.get("transform").and_then(Value::as_array) {
        for (i, v) in a.iter().take(6).enumerate() {
            m[i] = v.as_f64().unwrap_or(m[i]);
        }
    }
    m
}

fn transform_is_identity(node: &Value) -> bool {
    matrix(node) == [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
}

fn data_dir_image(base: &Path, value: &str, out_dir: &Path) -> Result<(PathBuf, String)> {
    let relative = crate::resource::safe_relative_path(Path::new(value))
        .with_context(|| format!("[unsafe-path] image path {value:?} in the data"))?;
    let absolute = base.join(&relative);
    let canonical = absolute.canonicalize().with_context(|| {
        format!(
            "[missing-resource] image {} named in the data does not exist",
            absolute.display()
        )
    })?;
    let out = out_dir
        .canonicalize()
        .unwrap_or_else(|_| out_dir.to_path_buf());
    let inside = canonical.strip_prefix(&out).with_context(|| {
        format!(
            "[unsafe-path] image {} is outside the output document's folder {}; keep merged images beside the output document",
            canonical.display(),
            out.display()
        )
    })?;
    let text = inside
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Ok((canonical, text))
}

/// Merge `records` into a copy of `raw`. The template document is not
/// modified. Nothing is written.
pub fn merge(raw: &Value, records: &Records, options: &MergeOptions) -> Result<MergeOutcome> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] data merge needs a version 9 document; run `pentool migrate --target 9` first"
    );
    let per_page = options.records_per_page;
    ensure!(
        (1..=MAX_CELLS).contains(&per_page),
        "[invalid-input] --records-per-page must be 1..={MAX_CELLS}"
    );
    let (cols, cell_w, cell_h) = if per_page == 1 {
        ensure!(
            options.grid.is_none() && options.cell.is_none(),
            "[invalid-input] --grid and --cell apply only when --records-per-page is greater than 1"
        );
        (1, 0.0, 0.0)
    } else {
        let (c, r) = options.grid.context(
            "[invalid-input] several records per page need --grid COLSxROWS and --cell WIDTHxHEIGHT",
        )?;
        ensure!(
            c >= 1 && r >= 1 && c * r == per_page,
            "[invalid-input] --grid {c}x{r} holds {} cells but --records-per-page is {per_page}",
            c * r
        );
        let (w, h) = options.cell.context(
            "[invalid-input] several records per page need --cell WIDTHxHEIGHT (the offset between cells)",
        )?;
        ensure!(
            w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0,
            "[invalid-input] --cell must be two positive numbers"
        );
        (c, w, h)
    };

    let template_index = match &options.template_page {
        Some(id) => pages_of(raw)
            .iter()
            .position(|p| node_id(p) == id)
            .with_context(|| format!("[missing-resource] template page not found: {id}"))?,
        None => 0,
    };
    let template = pages_of(raw)
        .get(template_index)
        .context("[invalid-input] document has no pages")?
        .clone();
    let template_id = node_id(&template).to_string();

    let output_pages = records.rows.len().div_ceil(per_page);
    ensure!(
        pages_of(raw).len() + output_pages <= crate::spreads::MAX_PAGES,
        "[limit-exceeded] merging {} records at {per_page} per page adds {output_pages} pages; the document limit is {} pages",
        records.rows.len(),
        crate::spreads::MAX_PAGES
    );

    // Template inventory: text frames, their stories, graphic frames.
    let nodes = page_nodes(&template);
    let mut template_frames: HashSet<String> = HashSet::new();
    let mut graphic_frames: HashSet<String> = HashSet::new();
    let mut story_ids: Vec<String> = Vec::new();
    for node in &nodes {
        match node.get("kind").and_then(Value::as_str) {
            Some("text-frame") => {
                template_frames.insert(node_id(node).to_string());
                let story = node
                    .get("story")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !story_ids.contains(&story) {
                    story_ids.push(story);
                }
            }
            Some("graphic-frame") => {
                graphic_frames.insert(node_id(node).to_string());
            }
            _ => {}
        }
    }
    let story_lookup = story_index(raw);
    let stories_array = raw["layout"]["stories"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut used: BTreeSet<String> = BTreeSet::new();
    for story in &story_ids {
        let index = story_lookup
            .get(story)
            .with_context(|| format!("[missing-resource] story not found: {story}"))?;
        let paragraphs = stories_array[*index]
            .get("paragraphs")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for paragraph in paragraphs {
            if let Some(runs) = paragraph.get("runs").and_then(Value::as_array) {
                collect_placeholders(runs, story, &mut used)?;
            }
        }
    }
    let field_set: HashSet<&str> = records.fields.iter().map(String::as_str).collect();
    let missing: Vec<&String> = used
        .iter()
        .filter(|u| !field_set.contains(u.as_str()))
        .collect();
    ensure!(
        missing.is_empty(),
        "[missing-resource] the template uses placeholders with no data field: {}. Data fields: {}",
        missing
            .iter()
            .map(|m| format!("{{{{{m}}}}}"))
            .collect::<Vec<_>>()
            .join(", "),
        records.fields.join(", ")
    );
    for (frame, field) in &options.image_fields {
        ensure!(
            graphic_frames.contains(frame),
            "[missing-resource] image placeholder {frame} is not a graphic frame on template page {template_id}"
        );
        ensure!(
            field_set.contains(field.as_str()),
            "[missing-resource] image placeholder {frame} names field {field}, which the data does not have. Data fields: {}",
            records.fields.join(", ")
        );
    }
    ensure!(
        !used.is_empty() || !options.image_fields.is_empty(),
        "[invalid-input] template page {template_id} has no {{{{field}}}} placeholders and no --image-field; nothing to merge"
    );

    // Images: decode each distinct file once.
    let field_pos: HashMap<&str, usize> = records
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    let mut asset_cache: HashMap<String, (crate::image::SourceInfo, Value)> = HashMap::new();
    let mut record_images: Vec<HashMap<String, (String, Value)>> = Vec::new();
    let mut empty_images = 0usize;
    for row in &records.rows {
        let mut map = HashMap::new();
        for (frame, field) in &options.image_fields {
            let value = row[field_pos[field.as_str()]].trim();
            if value.is_empty() {
                empty_images += 1;
                continue;
            }
            if !asset_cache.contains_key(value) {
                let (absolute, relative) =
                    data_dir_image(&options.data_dir, value, &options.out_dir)?;
                let bytes = crate::resource::read_bounded(&absolute, MAX_IMAGE_BYTES)?;
                let info = crate::image::decode_source(&bytes)?;
                let storage = crate::image::external_storage(Path::new(&relative))?;
                asset_cache.insert(value.to_string(), (info, storage));
            }
            let (info, storage) = &asset_cache[value];
            map.insert(frame.clone(), (info.digest.clone(), storage.clone()));
        }
        record_images.push(map);
    }

    // Build the output.
    let mut out = raw.clone();
    {
        let assets = out
            .as_object_mut()
            .expect("document object")
            .entry("image_assets")
            .or_insert_with(|| json!({}));
        let assets = assets
            .as_object_mut()
            .context("[malformed-resource] image_assets must be an object")?;
        let mut values: Vec<&(crate::image::SourceInfo, Value)> = asset_cache.values().collect();
        values.sort_by(|a, b| a.0.digest.cmp(&b.0.digest));
        for (info, storage) in values {
            assets.entry(info.digest.clone()).or_insert_with(|| {
                json!({
                    "media_type": info.media_type,
                    "byte_length": info.byte_length,
                    "pixel_width": info.pixel_width,
                    "pixel_height": info.pixel_height,
                    "color_space": info.color_space,
                    "orientation": info.orientation,
                    "storage": storage,
                })
            });
        }
    }
    let story_map: HashMap<String, String> =
        story_ids.iter().map(|s| (s.clone(), s.clone())).collect();
    let mut taken_stories: HashSet<String> = story_lookup.keys().cloned().collect();
    let mut new_stories: Vec<Value> = Vec::new();
    let mut story_records: Vec<(String, usize, String)> = Vec::new();
    let mut sanitized = Sanitized(0);
    let mut previous = template_id.clone();
    let mut page_ids: Vec<String> = Vec::new();
    let existing_pages: HashSet<String> = pages_of(raw)
        .iter()
        .map(|p| node_id(p).to_string())
        .collect();
    let layer_count = template
        .get("layers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    for page_number in 0..output_pages {
        let first = page_number * per_page;
        let chunk = &records.rows[first..(first + per_page).min(records.rows.len())];
        let page_id = id_with(&template_id, &format!("m{}", page_number + 1))?;
        ensure!(
            !existing_pages.contains(&page_id),
            "[invalid-input] page ID {page_id} already exists; rename the existing page"
        );
        let mut page = template.clone();
        page["id"] = json!(page_id);
        let name = template
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&template_id);
        page["name"] = json!(format!("{name} {}", page_number + 1));
        for layer_index in 0..layer_count {
            let source: Vec<Value> = template["layers"][layer_index]
                .get("nodes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut produced: Vec<Value> = Vec::new();
            for (cell, _) in chunk.iter().enumerate() {
                let record_number = first + cell;
                let suffix = if per_page == 1 {
                    format!("m{}", page_number + 1)
                } else {
                    format!("m{}c{}", page_number + 1, cell + 1)
                };
                let (col, row) = (cell % cols, cell / cols);
                let mut cloner = Cloner {
                    suffix: suffix.clone(),
                    dx: col as f64 * cell_w,
                    dy: row as f64 * cell_h,
                    template_frames: &template_frames,
                    story_map: &story_map,
                    images: &record_images[record_number],
                    extra: Vec::new(),
                };
                for node in &source {
                    let copy = cloner.clone_node(node)?;
                    produced.push(copy);
                    produced.append(&mut cloner.extra);
                }
                if layer_index == 0 {
                    let fields: HashMap<&str, &str> = records
                        .fields
                        .iter()
                        .map(String::as_str)
                        .zip(chunk[cell].iter().map(String::as_str))
                        .collect();
                    for story in &story_ids {
                        let mut copy = stories_array[story_lookup[story]].clone();
                        let new_id = id_with(story, &suffix)?;
                        ensure!(
                            taken_stories.insert(new_id.clone()),
                            "[invalid-input] story ID {new_id} already exists"
                        );
                        copy["id"] = json!(new_id);
                        if let Some(object) = copy.as_object_mut() {
                            object.remove("linked_to");
                            object.remove("link_digest");
                        }
                        for paragraph in copy["paragraphs"].as_array_mut().into_iter().flatten() {
                            if let Some(runs) = runs_mut(paragraph) {
                                substitute_runs(runs, &fields, &mut sanitized)?;
                            }
                        }
                        story_records.push((new_id, record_number + 1, page_id.clone()));
                        new_stories.push(copy);
                    }
                }
            }
            page["layers"][layer_index]["nodes"] = Value::Array(produced);
        }
        // Stories go in before the next page add, which validates the whole
        // document and would otherwise see dangling story references.
        out["layout"]["stories"]
            .as_array_mut()
            .context("layout.stories must be an array")?
            .append(&mut new_stories);
        let added = crate::spreads::apply_page(
            &mut out,
            PageAction::Add {
                id: page_id.clone(),
                name: None,
                width: None,
                height: None,
                background: None,
                after: Some(previous.clone()),
                before: None,
                parent: None,
                count: None,
            },
        )?;
        let _ = added;
        let index = pages_of(&out)
            .iter()
            .position(|p| node_id(p) == page_id)
            .context("[internal] added page not found")?;
        out["pages"][index] = page;
        previous = page_id.clone();
        page_ids.push(page_id);
    }
    if !options.keep_template {
        crate::spreads::apply_page(
            &mut out,
            PageAction::Remove {
                id: template_id.clone(),
                drop_story: true,
            },
        )?;
    }
    crate::spreads::validate(&out, crate::spreads::VERSION)?;
    crate::transaction::validate_value(&out)?;

    // Overflow report (estimator based).
    let created: HashSet<String> = story_records.iter().map(|(s, _, _)| s.clone()).collect();
    let by_story: HashMap<&str, (usize, &str)> = story_records
        .iter()
        .map(|(s, r, p)| (s.as_str(), (*r, p.as_str())))
        .collect();
    let overset = crate::stories::overset_stories(&out, &created);
    let mut overflow: Vec<Value> = overset
        .iter()
        .map(|entry| {
            let story = entry["story"].as_str().unwrap_or_default();
            let (record, page) = by_story.get(story).copied().unwrap_or((0, ""));
            json!({
                "record": record, "page": page, "story": story,
                "frame": entry["frame"], "characters_remaining": entry["characters_remaining"],
                "estimated": true,
            })
        })
        .collect();
    overflow.sort_by_key(|e| e["record"].as_u64().unwrap_or(0));
    let overflow_count = overflow.len();
    overflow.truncate(MAX_OVERFLOW_LISTED);
    let unplaced_template = options.keep_template;
    let report = json!({
        "operation": "layout-merge",
        "template_page": template_id,
        "records": records.rows.len(),
        "fields": records.fields,
        "records_per_page": per_page,
        "pages_created": page_ids.len(),
        "pages": page_ids,
        "placeholders": used,
        "image_fields": options.image_fields.iter().map(|(f, v)| json!({"frame": f, "field": v})).collect::<Vec<_>>(),
        "images_empty": empty_images,
        "images_distinct": asset_cache.len(),
        "sanitized_control_characters": sanitized.0,
        "template_kept": unplaced_template,
        "overflow_count": overflow_count,
        "overflow_truncated": overflow_count > overflow.len(),
        "overflow": overflow,
        "estimated": true,
    });
    Ok(MergeOutcome {
        document: out,
        report,
    })
}

// --------------------------------------------------------------- adaptive

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    End,
    Center,
    Stretch,
    Scale,
}

impl Anchor {
    fn parse(value: &str, vertical: bool) -> Result<Self> {
        Ok(match (value, vertical) {
            ("left", false) | ("top", true) | ("start", _) => Self::Start,
            ("right", false) | ("bottom", true) | ("end", _) => Self::End,
            ("center", _) => Self::Center,
            ("stretch", _) => Self::Stretch,
            ("scale", _) => Self::Scale,
            _ => bail!(
                "[invalid-input] {} anchor {value:?} must be {}center, stretch or scale",
                if vertical { "vertical" } else { "horizontal" },
                if vertical {
                    "top, bottom, "
                } else {
                    "left, right, "
                }
            ),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Scale,
    Recenter,
    Guide,
    Object,
}

impl Rule {
    pub fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "scale" => Self::Scale,
            "recenter" => Self::Recenter,
            "guide" => Self::Guide,
            "object" => Self::Object,
            _ => bail!("[invalid-input] rule must be scale, recenter, guide or object"),
        })
    }
    fn name(self) -> &'static str {
        match self {
            Self::Scale => "scale",
            Self::Recenter => "recenter",
            Self::Guide => "guide",
            Self::Object => "object",
        }
    }
}

/// `ID=h:left,v:top` -> constraints for one object.
pub fn parse_object_rule(spec: &str) -> Result<(String, Anchor, Anchor)> {
    let (id, rest) = spec
        .split_once('=')
        .context("[invalid-input] --object must be ID=h:ANCHOR,v:ANCHOR")?;
    ensure!(!id.is_empty(), "[invalid-input] --object has an empty ID");
    let mut h = Anchor::Start;
    let mut v = Anchor::Start;
    for part in rest.split(',').filter(|p| !p.is_empty()) {
        let (axis, value) = part
            .split_once(':')
            .context("[invalid-input] --object parts are h:ANCHOR or v:ANCHOR")?;
        match axis {
            "h" => h = Anchor::parse(value, false)?,
            "v" => v = Anchor::parse(value, true)?,
            _ => bail!("[invalid-input] --object axis must be h or v, not {axis}"),
        }
    }
    Ok((id.to_string(), h, v))
}

#[derive(Debug, Clone)]
pub struct AdaptOptions {
    pub rule: Rule,
    /// `scale`: keep the aspect ratio (centered) instead of stretching.
    pub uniform: bool,
    pub objects: Vec<(String, Anchor, Anchor)>,
}

struct Axis {
    old: f64,
    new: f64,
}

fn place_axis(
    anchor: Anchor,
    axis: &Axis,
    lo: f64,
    size: f64,
    guides: &[f64],
    rule: Rule,
    tolerance: f64,
) -> (f64, f64) {
    let ratio = axis.new / axis.old;
    if rule == Rule::Guide {
        let near = |p: f64| guides.iter().copied().find(|g| (g - p).abs() <= tolerance);
        let hi = lo + size;
        return match (near(lo), near(hi)) {
            (Some(a), Some(b)) => (a * ratio, (b - a) * ratio),
            (Some(a), None) => (a * ratio, size),
            (None, Some(b)) => (b * ratio - size, size),
            (None, None) => ((lo + size / 2.0) * ratio - size / 2.0, size),
        };
    }
    let delta = axis.new - axis.old;
    match anchor {
        Anchor::Start => (lo, size),
        Anchor::End => (lo + delta, size),
        Anchor::Center => (lo + delta / 2.0, size),
        Anchor::Stretch => (lo, size + delta),
        Anchor::Scale => (lo * ratio, size * ratio),
    }
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Re-place the top-level nodes of `page` for a new canvas size. Returns the
/// count of nodes moved and the guide offsets applied.
fn adapt_page(
    page: &mut Value,
    old: (f64, f64),
    new: (f64, f64),
    options: &AdaptOptions,
    vguides: &[f64],
    hguides: &[f64],
) -> Result<usize> {
    let constraints: HashMap<&str, (Anchor, Anchor)> = options
        .objects
        .iter()
        .map(|(id, h, v)| (id.as_str(), (*h, *v)))
        .collect();
    let (xa, ya) = (
        Axis {
            old: old.0,
            new: new.0,
        },
        Axis {
            old: old.1,
            new: new.1,
        },
    );
    let (sx, sy) = (new.0 / old.0, new.1 / old.1);
    let uniform = sx.min(sy);
    let mut moved = 0usize;
    for layer in page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if layer.get("locked") == Some(&json!(true)) {
            continue;
        }
        for node in layer
            .get_mut("nodes")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if node.get("locked") == Some(&json!(true)) {
                continue;
            }
            let id = node_id(node).to_string();
            let (h, v) = match options.rule {
                Rule::Object => constraints
                    .get(id.as_str())
                    .copied()
                    .unwrap_or((Anchor::Start, Anchor::Start)),
                _ => (Anchor::Start, Anchor::Start),
            };
            let direct = node.get("x").and_then(Value::as_f64).is_some()
                && node.get("y").and_then(Value::as_f64).is_some()
                && transform_is_identity(node);
            if direct {
                let x = node["x"].as_f64().unwrap_or(0.0);
                let y = node["y"].as_f64().unwrap_or(0.0);
                let w = node.get("width").and_then(Value::as_f64);
                let hh = node.get("height").and_then(Value::as_f64);
                let (nx, nw, ny, nh) = match options.rule {
                    Rule::Scale if options.uniform => {
                        let ox = (new.0 - old.0 * uniform) / 2.0;
                        let oy = (new.1 - old.1 * uniform) / 2.0;
                        (
                            x * uniform + ox,
                            w.map(|w| w * uniform),
                            y * uniform + oy,
                            hh.map(|h| h * uniform),
                        )
                    }
                    Rule::Scale => (x * sx, w.map(|w| w * sx), y * sy, hh.map(|h| h * sy)),
                    Rule::Recenter => (x + (new.0 - old.0) / 2.0, w, y + (new.1 - old.1) / 2.0, hh),
                    Rule::Guide | Rule::Object => {
                        let (px, pw) =
                            place_axis(h, &xa, x, w.unwrap_or(0.0), vguides, options.rule, 0.5);
                        let (py, ph) =
                            place_axis(v, &ya, y, hh.unwrap_or(0.0), hguides, options.rule, 0.5);
                        (px, w.map(|_| pw), py, hh.map(|_| ph))
                    }
                };
                node["x"] = json!(round(nx));
                node["y"] = json!(round(ny));
                if let Some(nw) = nw {
                    node["width"] = json!(round(nw.max(0.0)));
                }
                if let Some(nh) = nh {
                    node["height"] = json!(round(nh.max(0.0)));
                }
            } else {
                // Transform-only (or transformed) nodes: adjust the matrix.
                let mut m = matrix(node);
                match options.rule {
                    Rule::Scale if options.uniform => {
                        let ox = (new.0 - old.0 * uniform) / 2.0;
                        let oy = (new.1 - old.1 * uniform) / 2.0;
                        for value in &mut m[..4] {
                            *value *= uniform;
                        }
                        m[4] = m[4] * uniform + ox;
                        m[5] = m[5] * uniform + oy;
                    }
                    Rule::Scale => {
                        m[0] *= sx;
                        m[2] *= sx;
                        m[4] *= sx;
                        m[1] *= sy;
                        m[3] *= sy;
                        m[5] *= sy;
                    }
                    Rule::Recenter => {
                        m[4] += (new.0 - old.0) / 2.0;
                        m[5] += (new.1 - old.1) / 2.0;
                    }
                    Rule::Guide | Rule::Object => {
                        let (px, _) = place_axis(h, &xa, m[4], 0.0, vguides, options.rule, 0.5);
                        let (py, _) = place_axis(v, &ya, m[5], 0.0, hguides, options.rule, 0.5);
                        m[4] = px;
                        m[5] = py;
                    }
                }
                for value in &mut m {
                    *value = round(*value);
                }
                node["transform"] = json!(m);
            }
            moved += 1;
        }
    }
    Ok(moved)
}

fn page_canvas(page: &Value) -> Result<(f64, f64)> {
    let canvas = page.get("canvas").context("page has no canvas")?;
    let w = canvas.get("width").and_then(Value::as_f64);
    let h = canvas.get("height").and_then(Value::as_f64);
    match (w, h) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Ok((w, h)),
        _ => bail!("[malformed-resource] page canvas has no positive width and height"),
    }
}

fn check_size(width: f64, height: f64) -> Result<()> {
    ensure!(
        width.is_finite()
            && height.is_finite()
            && (1.0..=100_000.0).contains(&width)
            && (1.0..=100_000.0).contains(&height),
        "[invalid-input] page size must be 1..=100000 in each direction"
    );
    Ok(())
}

/// Guide positions of one page, by axis (`ruler-v` -> x, `ruler-h` -> y).
fn page_guides(raw: &Value, page: &str) -> (Vec<f64>, Vec<f64>) {
    let mut vertical = vec![0.0];
    let mut horizontal = vec![0.0];
    for guide in raw
        .get("layout")
        .and_then(|l| l.get("guides"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if guide.get("page").and_then(Value::as_str) != Some(page) {
            continue;
        }
        let Some(position) = guide.get("position").and_then(Value::as_f64) else {
            continue;
        };
        match guide.get("kind").and_then(Value::as_str) {
            Some("ruler-v") => vertical.push(position),
            Some("ruler-h") => horizontal.push(position),
            _ => {}
        }
    }
    (vertical, horizontal)
}

fn move_guides(
    raw: &mut Value,
    page: &str,
    old: (f64, f64),
    new: (f64, f64),
    options: &AdaptOptions,
) -> usize {
    let mut moved = 0;
    let uniform = (new.0 / old.0).min(new.1 / old.1);
    if let Some(guides) = raw
        .get_mut("layout")
        .and_then(|l| l.get_mut("guides"))
        .and_then(Value::as_array_mut)
    {
        for guide in guides {
            if guide.get("page").and_then(Value::as_str) != Some(page)
                || guide.get("locked") == Some(&json!(true))
            {
                continue;
            }
            let Some(position) = guide.get("position").and_then(Value::as_f64) else {
                continue;
            };
            let vertical = guide.get("kind").and_then(Value::as_str) == Some("ruler-v");
            let (o, n) = if vertical {
                (old.0, new.0)
            } else {
                (old.1, new.1)
            };
            let value = match options.rule {
                Rule::Scale if options.uniform => position * uniform + (n - o * uniform) / 2.0,
                Rule::Scale | Rule::Guide => position * (n / o),
                Rule::Recenter => position + (n - o) / 2.0,
                Rule::Object => continue,
            };
            guide["position"] = json!(round(value));
            moved += 1;
        }
    }
    moved
}

fn set_canvas(page: &mut Value, width: f64, height: f64) {
    let w = if width.fract() == 0.0 {
        json!(width as u64)
    } else {
        json!(width)
    };
    let h = if height.fract() == 0.0 {
        json!(height as u64)
    } else {
        json!(height)
    };
    page["canvas"]["width"] = w;
    page["canvas"]["height"] = h;
}

/// Resize one page and re-place its objects (`layout-adapt`).
pub fn adapt(
    raw: &mut Value,
    page_id: &str,
    width: f64,
    height: f64,
    options: &AdaptOptions,
) -> Result<Value> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] adaptive layout needs a version 9 document; run `pentool migrate --target 9` first"
    );
    check_size(width, height)?;
    ensure!(
        options.rule == Rule::Object || options.objects.is_empty(),
        "[invalid-input] --object constraints apply only to --rule object"
    );
    ensure!(
        options.rule == Rule::Scale || !options.uniform,
        "[invalid-input] --uniform applies only to --rule scale"
    );
    let index = pages_of(raw)
        .iter()
        .position(|p| node_id(p) == page_id)
        .with_context(|| format!("[missing-resource] page not found: {page_id}"))?;
    let old = page_canvas(&pages_of(raw)[index])?;
    let mut ids: HashSet<String> = HashSet::new();
    for node in page_nodes(&pages_of(raw)[index]) {
        ids.insert(node_id(node).to_string());
    }
    for (id, _, _) in &options.objects {
        ensure!(
            ids.contains(id),
            "[missing-resource] object {id} is not on page {page_id}"
        );
    }
    let (vguides, hguides) = page_guides(raw, page_id);
    let moved = adapt_page(
        &mut raw["pages"][index],
        old,
        (width, height),
        options,
        &vguides,
        &hguides,
    )?;
    set_canvas(&mut raw["pages"][index], width, height);
    let guides = move_guides(raw, page_id, old, (width, height), options);
    crate::spreads::validate(raw, crate::spreads::VERSION)?;
    Ok(json!({
        "ok": true, "operation": "layout-adapt", "page": page_id,
        "rule": options.rule.name(),
        "from": {"width": old.0, "height": old.1},
        "to": {"width": width, "height": height},
        "objects_adjusted": moved,
        "guides_moved": guides,
        "note": "text size is unchanged; frames reflow in their new geometry",
    }))
}

// -------------------------------------------------------------- alternates

fn story_digest(story: &Value) -> String {
    let mut bytes = serde_json::to_vec(&story["paragraphs"]).unwrap_or_default();
    if let Some(language) = story.get("language") {
        bytes.extend(serde_json::to_vec(language).unwrap_or_default());
    }
    hex::encode(Sha256::digest(&bytes))
}

#[derive(Debug, Clone, Subcommand)]
pub enum AltAction {
    /// List alternate layouts and their linked stories (read only).
    List,
    /// Copy a page to a new size as an alternate layout. Its stories become
    /// linked copies of the source stories.
    Create {
        /// Source page to copy.
        source: String,
        /// New page ID.
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        width: f64,
        #[arg(long)]
        height: f64,
        /// scale, recenter, guide or object.
        #[arg(long, default_value = "scale")]
        rule: String,
        #[arg(long)]
        uniform: bool,
        /// Object constraint ID=h:left|right|center|stretch|scale,v:top|bottom|center|stretch|scale (repeatable).
        #[arg(long = "object")]
        objects: Vec<String>,
    },
    /// Refresh linked stories from their sources. Stories edited locally are
    /// skipped and reported unless --force is given.
    Sync {
        /// Limit to the alternate layout page with this ID.
        #[arg(long)]
        page: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Stop linking an alternate layout's stories to their sources.
    Unlink { page: String },
}

fn alternates(raw: &Value) -> Vec<Value> {
    raw.get("layout")
        .and_then(|l| l.get("alternates"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn text_frame_stories(page: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for node in page_nodes(page) {
        if node.get("kind").and_then(Value::as_str) == Some("text-frame") {
            if let Some(story) = node.get("story").and_then(Value::as_str) {
                if !out.iter().any(|s| s == story) {
                    out.push(story.to_string());
                }
            }
        }
    }
    out
}

fn alt_id(base: &str, page: &str) -> Result<String> {
    id_with(base, page)
}

fn rewrite_ids(
    node: &mut Value,
    page: &str,
    frames: &HashSet<String>,
    stories: &HashMap<String, String>,
) -> Result<()> {
    let old = node_id(node).to_string();
    if !old.is_empty() {
        node["id"] = json!(alt_id(&old, page)?);
    }
    match node.get("kind").and_then(Value::as_str) {
        Some("text-frame") => {
            if let Some(next) = node.get("next").and_then(Value::as_str).map(str::to_string) {
                ensure!(
                    frames.contains(&next),
                    "[unsupported-capability] frame {old} threads to {next}, which is not on the source page; an alternate layout copies one page"
                );
                node["next"] = json!(alt_id(&next, page)?);
            }
            if let Some(story) = node.get("story").and_then(Value::as_str) {
                node["story"] = json!(stories
                    .get(story)
                    .with_context(|| format!("[missing-resource] story not found: {story}"))?);
            }
            node.as_object_mut().map(|o| o.remove("primary"));
        }
        Some("graphic-frame") => {
            if let Some(content) = node
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                node["content"] = json!(alt_id(&content, page)?);
            }
        }
        _ => {}
    }
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            rewrite_ids(child, page, frames, stories)?;
        }
    }
    Ok(())
}

pub fn run_alt(raw: &mut Value, action: AltAction) -> Result<(Value, bool)> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] alternate layouts need a version 9 document; run `pentool migrate --target 9` first"
    );
    match action {
        AltAction::List => {
            let stories = story_index(raw);
            let story_list = raw["layout"]["stories"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut out = Vec::new();
            for entry in alternates(raw) {
                let page = entry["page"].as_str().unwrap_or_default().to_string();
                let page_value = pages_of(raw).iter().find(|p| node_id(p) == page);
                let mut linked = Vec::new();
                if let Some(p) = page_value {
                    for story in text_frame_stories(p) {
                        let Some(i) = stories.get(&story) else {
                            continue;
                        };
                        let s = &story_list[*i];
                        let Some(source) = s.get("linked_to").and_then(Value::as_str) else {
                            continue;
                        };
                        let source_exists = stories.contains_key(source);
                        let edited = s.get("link_digest").and_then(Value::as_str)
                            != Some(story_digest(s).as_str());
                        linked.push(json!({
                            "story": story, "source": source,
                            "source_exists": source_exists, "edited_locally": edited,
                        }));
                    }
                }
                out.push(json!({
                    "page": page,
                    "name": entry["name"],
                    "source_page": entry["source_page"],
                    "page_exists": page_value.is_some(),
                    "linked_stories": linked,
                }));
            }
            Ok((json!({"alternates": out, "count": out.len()}), true))
        }
        AltAction::Create {
            source,
            id,
            name,
            width,
            height,
            rule,
            uniform,
            objects,
        } => {
            check_size(width, height)?;
            ensure!(
                !id.is_empty() && id.chars().count() <= MAX_ID_LEN && !id.contains(['/', '\\']),
                "[invalid-input] page ID must be 1-{MAX_ID_LEN} characters without '/' or '\\'"
            );
            ensure!(
                alternates(raw).len() < MAX_ALTERNATES,
                "[limit-exceeded] a document is limited to {MAX_ALTERNATES} alternate layouts"
            );
            ensure!(
                !pages_of(raw).iter().any(|p| node_id(p) == id),
                "[invalid-input] page ID already exists: {id}"
            );
            let rule = Rule::parse(&rule)?;
            let objects = objects
                .iter()
                .map(|o| parse_object_rule(o))
                .collect::<Result<Vec<_>>>()?;
            let options = AdaptOptions {
                rule,
                uniform,
                objects,
            };
            let index = pages_of(raw)
                .iter()
                .position(|p| node_id(p) == source)
                .with_context(|| format!("[missing-resource] source page not found: {source}"))?;
            let mut page = pages_of(raw)[index].clone();
            let old = page_canvas(&page)?;
            let source_nodes = page_nodes(&pages_of(raw)[index]);
            let frames: HashSet<String> = source_nodes
                .iter()
                .filter(|n| n.get("kind").and_then(Value::as_str) == Some("text-frame"))
                .map(|n| node_id(n).to_string())
                .collect();
            for (object, _, _) in &options.objects {
                ensure!(
                    source_nodes.iter().any(|n| node_id(n) == object),
                    "[missing-resource] object {object} is not on page {source}"
                );
            }
            let story_ids = text_frame_stories(&page);
            let taken: HashSet<String> = story_index(raw).keys().cloned().collect();
            let mut story_map: HashMap<String, String> = HashMap::new();
            let mut new_stories = Vec::new();
            let story_list = raw["layout"]["stories"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let lookup = story_index(raw);
            for story in &story_ids {
                let new_id = alt_id(story, &id)?;
                ensure!(
                    !taken.contains(&new_id),
                    "[invalid-input] story ID {new_id} already exists"
                );
                let source_story = &story_list[*lookup
                    .get(story)
                    .with_context(|| format!("[missing-resource] story not found: {story}"))?];
                let mut copy = source_story.clone();
                copy["id"] = json!(new_id);
                copy["linked_to"] = json!(story);
                copy["link_digest"] = json!(story_digest(&copy));
                story_map.insert(story.clone(), new_id);
                new_stories.push(copy);
            }
            for layer in page
                .get_mut("layers")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                for node in layer
                    .get_mut("nodes")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    rewrite_ids(node, &id, &frames, &story_map)?;
                }
            }
            page["id"] = json!(id);
            let source_name = page
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&source)
                .to_string();
            let display = name.unwrap_or_else(|| format!("{source_name} {id}"));
            page["name"] = json!(display);
            let (vguides, hguides) = page_guides(raw, &source);
            let moved = adapt_page(
                &mut page,
                old,
                (width, height),
                &options,
                &vguides,
                &hguides,
            )?;
            set_canvas(&mut page, width, height);
            crate::spreads::apply_page(
                raw,
                PageAction::Add {
                    id: id.clone(),
                    name: None,
                    width: None,
                    height: None,
                    background: None,
                    after: Some(source.clone()),
                    before: None,
                    parent: None,
                    count: None,
                },
            )?;
            let at = pages_of(raw)
                .iter()
                .position(|p| node_id(p) == id)
                .context("[internal] added page not found")?;
            raw["pages"][at] = page;
            raw["layout"]["stories"]
                .as_array_mut()
                .context("layout.stories must be an array")?
                .extend(new_stories);
            let layout = raw["layout"].as_object_mut().context("layout object")?;
            layout
                .entry("alternates")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .context("layout.alternates must be an array")?
                .push(json!({"page": id, "source_page": source, "name": display}));
            crate::spreads::validate(raw, crate::spreads::VERSION)?;
            Ok((
                json!({
                    "ok": true, "operation": "layout-alt-create", "page": id,
                    "source_page": source, "rule": options.rule.name(),
                    "from": {"width": old.0, "height": old.1},
                    "to": {"width": width, "height": height},
                    "objects_adjusted": moved,
                    "linked_stories": story_map.iter().map(|(s, l)| json!({"source": s, "story": l})).collect::<Vec<_>>(),
                }),
                false,
            ))
        }
        AltAction::Sync { page, force } => {
            let stories = story_index(raw);
            let sources: Vec<(usize, String)> = raw["layout"]["stories"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .enumerate()
                        .filter_map(|(i, s)| {
                            s.get("linked_to")
                                .and_then(Value::as_str)
                                .map(|src| (i, src.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let only: Option<HashSet<String>> = match &page {
                Some(p) => {
                    let target = pages_of(raw)
                        .iter()
                        .find(|x| node_id(x) == p)
                        .with_context(|| format!("[missing-resource] page not found: {p}"))?;
                    Some(text_frame_stories(target).into_iter().collect())
                }
                None => None,
            };
            let mut updated = Vec::new();
            let mut unchanged = 0usize;
            let mut skipped = Vec::new();
            let mut broken = Vec::new();
            for (index, source) in sources {
                let id = node_id(&raw["layout"]["stories"][index]).to_string();
                if only.as_ref().is_some_and(|set| !set.contains(&id)) {
                    continue;
                }
                let Some(&source_index) = stories.get(&source) else {
                    broken.push(json!({"story": id, "source": source}));
                    continue;
                };
                let source_story = raw["layout"]["stories"][source_index].clone();
                let current = raw["layout"]["stories"][index].clone();
                let edited = current.get("link_digest").and_then(Value::as_str)
                    != Some(story_digest(&current).as_str());
                let same = story_digest(&source_story) == story_digest(&current);
                if same {
                    unchanged += 1;
                    continue;
                }
                if edited && !force {
                    skipped
                        .push(json!({"story": id, "source": source, "reason": "edited-locally"}));
                    continue;
                }
                let target = &mut raw["layout"]["stories"][index];
                target["paragraphs"] = source_story["paragraphs"].clone();
                match source_story.get("language") {
                    Some(l) => target["language"] = l.clone(),
                    None => {
                        target.as_object_mut().map(|o| o.remove("language"));
                    }
                }
                let digest = story_digest(target);
                target["link_digest"] = json!(digest);
                updated
                    .push(json!({"story": id, "source": source, "overwrote_local_edits": edited}));
            }
            crate::spreads::validate(raw, crate::spreads::VERSION)?;
            let changed = !updated.is_empty();
            Ok((
                json!({
                    "ok": true, "operation": "layout-alt-sync",
                    "updated": updated, "unchanged": unchanged,
                    "skipped": skipped, "broken_links": broken,
                }),
                !changed,
            ))
        }
        AltAction::Unlink { page } => {
            let target = pages_of(raw)
                .iter()
                .find(|p| node_id(p) == page)
                .with_context(|| format!("[missing-resource] page not found: {page}"))?;
            let owned: HashSet<String> = text_frame_stories(target).into_iter().collect();
            let mut unlinked = Vec::new();
            for story in raw["layout"]["stories"]
                .as_array_mut()
                .into_iter()
                .flatten()
            {
                if owned.contains(node_id(story)) && story.get("linked_to").is_some() {
                    unlinked.push(node_id(story).to_string());
                    if let Some(o) = story.as_object_mut() {
                        o.remove("linked_to");
                        o.remove("link_digest");
                    }
                }
            }
            if let Some(list) = raw["layout"]
                .get_mut("alternates")
                .and_then(Value::as_array_mut)
            {
                list.retain(|e| e["page"] != page.as_str());
            }
            Ok((
                json!({"ok": true, "operation": "layout-alt-unlink", "page": page, "unlinked": unlinked}),
                false,
            ))
        }
    }
}

// -------------------------------------------------------------- validation

/// Structural checks for `layout.alternates` and story links. Stale entries
/// (a removed page or source story) are not errors; `layout-alt list`
/// reports them.
pub fn validate(raw: &Value) -> Result<()> {
    let Some(list) = raw.get("layout").and_then(|l| l.get("alternates")) else {
        return Ok(());
    };
    let list = list
        .as_array()
        .context("[malformed-resource] layout.alternates must be an array")?;
    ensure!(
        list.len() <= MAX_ALTERNATES,
        "[limit-exceeded] alternate layouts: {} exceeds the limit of {MAX_ALTERNATES}",
        list.len()
    );
    let mut seen = HashSet::new();
    for entry in list {
        let page = entry
            .get("page")
            .and_then(Value::as_str)
            .unwrap_or_default();
        ensure!(
            !page.is_empty() && seen.insert(page),
            "[malformed-resource] layout.alternates entries need a unique nonempty page"
        );
        ensure!(
            entry
                .get("source_page")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty()),
            "[malformed-resource] alternate {page}: source_page must be a nonempty page ID"
        );
    }
    for story in raw
        .get("layout")
        .and_then(|l| l.get("stories"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(link) = story.get("linked_to") {
            ensure!(
                link.as_str().is_some_and(|s| !s.is_empty()),
                "[malformed-resource] story {}: linked_to must be a nonempty story ID",
                node_id(story)
            );
        }
    }
    Ok(())
}

/// Format detection for `--data`.
pub fn data_format(path: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(f) = explicit {
        return Ok(f.to_ascii_lowercase());
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("csv") => Ok("csv".into()),
        Some("tsv") => Ok("tsv".into()),
        Some("json") => Ok("json".into()),
        _ => bail!(
            "[invalid-input] cannot tell the data format from {}; pass --format csv|tsv|json",
            path.display()
        ),
    }
}

/// Stable summary map used by tests: story -> record.
pub fn record_story_map(report: &Value) -> BTreeMap<String, u64> {
    report["overflow"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| Some((e["story"].as_str()?.to_string(), e["record"].as_u64()?)))
        .collect()
}

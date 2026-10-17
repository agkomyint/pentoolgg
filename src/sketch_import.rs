//! Bounded, offline importer for a conservative Sketch `.sketch` JSON subset.
//!
//! The reader never extracts archive contents to disk. It imports referenced
//! page JSON and maps static artboards, groups, rectangles, ovals, text, and
//! straight-segment paths into ordinary v4 pages/nodes. Unsupported features
//! are reported instead of silently being treated as preserved.

use crate::interchange::{
    self, FidelityReport, FidelityReportBuilder, FidelityStatus, ImportLimits,
};
use anyhow::{anyhow, ensure, Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read};
use zip::ZipArchive;

const MAX_JSON_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CANVAS_EDGE: f64 = 16_384.0;
const MAX_COLOR: f64 = 1.0;

#[derive(Debug)]
pub struct SketchImport {
    /// Imported Sketch pages as ordinary Pentool v4 page objects.
    pub pages: Vec<Value>,
    /// Reusable component snapshots referenced by imported symbol instances.
    pub components: Vec<Value>,
    /// Bounded fidelity details for this import.
    pub report: FidelityReport,
}

/// Import a `.sketch` archive from memory.
///
/// `source` is diagnostic text only; it is never interpreted as a path. No
/// archive member is extracted and no network or filesystem access occurs.
pub fn import(bytes: &[u8], source: &str) -> Result<SketchImport> {
    import_with_limits(bytes, source, ImportLimits::default())
}

/// Import with caller-supplied ceilings, primarily useful for bounded hosts and
/// tests. A valid Sketch archive must contain `document.json` and its referenced
/// `pages/<id>.json` files.
pub fn import_with_limits(
    bytes: &[u8],
    source: &str,
    limits: ImportLimits,
) -> Result<SketchImport> {
    interchange::validate_limits(limits)?;
    limits.check_input(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .context("[malformed-resource] input is not a valid Sketch ZIP archive")?;
    let entries = interchange::inspect_archive(&mut archive, limits)?;
    let mut by_name = BTreeMap::new();
    for entry in entries {
        ensure!(
            by_name.insert(entry.name.clone(), entry).is_none(),
            "[malformed-resource] Sketch archive contains duplicate member names"
        );
    }
    let document = read_json_member(&mut archive, &by_name, "document.json")?;
    let page_refs = document
        .get("pages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("[malformed-resource] Sketch document.json has no pages array"))?;
    ensure!(
        !page_refs.is_empty() && page_refs.len() <= 1000,
        "[limit-exceeded] Sketch document must contain 1..=1000 pages"
    );

    let mut report = FidelityReportBuilder::new("sketch", source);
    let layer_styles = shared_style_map(&document, "layerStyles", limits.max_nodes, &mut report)?;
    let text_styles =
        shared_style_map(&document, "layerTextStyles", limits.max_nodes, &mut report)?;
    let mut pages = Vec::with_capacity(page_refs.len());
    let mut components = Vec::new();
    // Resolve symbol masters across pages before converting visible page nodes.
    // Only the referenced page JSON is loaded, with a total decompressed budget.
    let mut page_sources = Vec::with_capacity(page_refs.len());
    let mut referenced_pages = HashSet::new();
    let mut total_json_bytes = 0u64;
    let doc_bytes = by_name
        .get("document.json")
        .map(|entry| entry.uncompressed_bytes)
        .unwrap_or(0);
    total_json_bytes = total_json_bytes.saturating_add(doc_bytes);
    for reference in page_refs {
        let ref_path = reference
            .get("_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("[malformed-resource] Sketch page reference is missing _ref"))?;
        let page_path = canonical_page_member(ref_path)?;
        ensure!(
            referenced_pages.insert(page_path.clone()),
            "[malformed-resource] Sketch document references a page more than once"
        );
        let info = by_name
            .get(&page_path)
            .ok_or_else(|| anyhow!("[missing-resource] Sketch archive is missing {page_path}"))?;
        total_json_bytes = total_json_bytes.saturating_add(info.uncompressed_bytes);
        ensure!(
            total_json_bytes <= 256 * 1024 * 1024,
            "[limit-exceeded] referenced Sketch JSON exceeds the 256 MiB aggregate import limit"
        );
        let mut page_json =
            read_json_member(&mut archive, &by_name, &canonical_page_member(ref_path)?)?;
        clear_private_import_markers(&mut page_json);
        page_sources.push((page_path, page_json));
    }
    let mut symbol_components = BTreeMap::<String, String>::new();
    for (_, page_json) in &page_sources {
        let root = page_json
            .get("root")
            .filter(|root| root.is_object())
            .unwrap_or(page_json);
        scan_symbol_masters(root, &mut |master| {
            if let (Some(symbol), Some(object_id)) = (
                string_field(master, "symbolID"),
                string_field(master, "do_objectID"),
            ) {
                symbol_components
                    .entry(symbol)
                    .or_insert_with(|| stable_id("component", &object_id, "master"));
            }
        });
    }
    let mut total_nodes = 0usize;
    let mut symbol_ids = IdContext::default();
    let mut masters = Vec::new();
    for (_, page_json) in &page_sources {
        let root = page_json
            .get("root")
            .filter(|root| root.is_object())
            .unwrap_or(page_json);
        scan_symbol_masters(root, &mut |master| masters.push(master.clone()));
    }
    let symbol_master_layers: BTreeMap<String, Vec<Value>> = masters
        .iter()
        .filter_map(|master| {
            let symbol = string_field(master, "symbolID")?;
            let layers = master.get("layers")?.as_array()?.clone();
            Some((symbol, layers))
        })
        .collect();
    for master in masters {
        let Some(symbol) = string_field(&master, "symbolID") else {
            continue;
        };
        let Some(component_id) = symbol_components.get(&symbol).cloned() else {
            continue;
        };
        if components
            .iter()
            .any(|component: &Value| component["id"] == component_id)
        {
            continue;
        }
        let source_id = string_field(&master, "do_objectID").unwrap_or(symbol);
        let mut max_x = 1.0;
        let mut max_y = 1.0;
        let mut min_x = 0.0;
        let mut min_y = 0.0;
        if let Some(snapshot) = convert_node(
            &master,
            &mut symbol_ids,
            &mut report,
            &limits,
            0,
            &[],
            &mut total_nodes,
            (0.0, 0.0),
            &mut max_x,
            &mut max_y,
            &mut min_x,
            &mut min_y,
            &symbol_components,
            &layer_styles,
            &text_styles,
            true,
        )? {
            let identity = hex::encode(Sha256::digest(serde_json::to_vec(&snapshot)?));
            components
                .push(json!({"id":component_id,"source_identity":identity,"snapshot":snapshot}));
            report.record(
                source_id,
                "symbol_master",
                FidelityStatus::Exact,
                "Sketch symbol master retained as a component snapshot",
                None,
            );
        }
    }
    let mut seen_page_ids = HashSet::new();
    let mut hydrated_nodes = 0usize;
    for (page_index, (page_path, mut page_json)) in page_sources.into_iter().enumerate() {
        ensure!(
            seen_page_ids.insert(page_path.clone()),
            "[malformed-resource] Sketch document references a page more than once"
        );
        let page_object = page_json
            .get("root")
            .filter(|root| root.is_object())
            .unwrap_or(&page_json);
        let source_page_id = string_field(page_object, "do_objectID")
            .unwrap_or_else(|| format!("page-{page_index}"));
        let page_name =
            string_field(page_object, "name").unwrap_or_else(|| format!("Page {}", page_index + 1));
        if page_json.get("root").is_some_and(Value::is_object) {
            hydrate_empty_symbol_instances(
                page_json.get_mut("root").unwrap(),
                &symbol_master_layers,
                &mut hydrated_nodes,
                limits.max_nodes,
                0,
            )?;
        } else {
            hydrate_empty_symbol_instances(
                &mut page_json,
                &symbol_master_layers,
                &mut hydrated_nodes,
                limits.max_nodes,
                0,
            )?;
        }
        let page_object = page_json
            .get("root")
            .filter(|root| root.is_object())
            .unwrap_or(&page_json);
        let source_layers = page_object
            .get("layers")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("[malformed-resource] Sketch page has no layers array"))?;
        ensure!(
            source_layers.len() <= limits.max_nodes,
            "[limit-exceeded] Sketch page layer count exceeds the configured node limit"
        );

        let mut id_context = IdContext::default();
        let mut nodes = Vec::new();
        let mut max_x = 0.0_f64;
        let mut max_y = 0.0_f64;
        let mut min_x = 0.0_f64;
        let mut min_y = 0.0_f64;
        for (node_index, source_node) in source_layers.iter().enumerate() {
            let path = vec![node_index];
            if let Some(node) = convert_node(
                source_node,
                &mut id_context,
                &mut report,
                &limits,
                0,
                &path,
                &mut total_nodes,
                (0.0, 0.0),
                &mut max_x,
                &mut max_y,
                &mut min_x,
                &mut min_y,
                &symbol_components,
                &layer_styles,
                &text_styles,
                false,
            )? {
                nodes.push(node);
            }
        }
        let width = checked_canvas(max_x - min_x)?;
        let height = checked_canvas(max_y - min_y)?;
        if min_x < 0.0 || min_y < 0.0 {
            nodes = vec![json!({
                "kind":"group",
                "id":stable_id("bounds", &source_page_id, "origin"),
                "transform":[1, 0, 0, 1, -min_x, -min_y],
                "children":nodes
            })];
            report.record(
                &source_page_id,
                "canvas_origin",
                FidelityStatus::Exact,
                "negative Sketch page coordinates shifted into the imported canvas",
                None,
            );
        }
        let page_id = stable_id("page", &source_page_id, &page_index.to_string());
        ensure!(
            !pages.iter().any(|page: &Value| page["id"] == page_id),
            "[malformed-resource] Sketch page identifiers collide after normalization"
        );
        pages.push(json!({
            "id": page_id,
            "name": page_name,
            "canvas": {"width": width, "height": height, "background": "#ffffff"},
            "layers": [{
                "id": stable_id("layer", &source_page_id, "root"),
                "name": "Imported Sketch",
                "visible": true,
                "locked": false,
                "nodes": nodes
            }]
        }));
        report.record(
            source_page_id,
            "page",
            FidelityStatus::Exact,
            "Sketch page imported as a Pentool page",
            None,
        );
    }

    // Document-level settings are not interpreted by this focused importer.
    for feature in ["colorSpace", "foreignSymbols", "sharedSwatches"] {
        if document.get(feature).is_some_and(|value| !value.is_null()) {
            report.record(
                "$document",
                feature,
                FidelityStatus::Dropped,
                "document-level Sketch setting is outside the supported import subset",
                Some("open and normalize this setting in Sketch before importing".into()),
            );
        }
    }
    Ok(SketchImport {
        pages,
        components,
        report: report.finish(),
    })
}

fn hydrate_empty_symbol_instances(
    value: &mut Value,
    masters: &BTreeMap<String, Vec<Value>>,
    expanded_nodes: &mut usize,
    max_nodes: usize,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= 256,
        "[limit-exceeded] Sketch symbol fallback nesting exceeds 256"
    );
    if let Some(object) = value.as_object_mut() {
        let empty_instance = object.get("_class").and_then(Value::as_str) == Some("symbolInstance")
            && object
                .get("layers")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty);
        if empty_instance {
            let symbol = object.get("symbolID").and_then(Value::as_str);
            if let Some(layers) = symbol.and_then(|symbol| masters.get(symbol)) {
                let instance_id = object
                    .get("do_objectID")
                    .and_then(Value::as_str)
                    .unwrap_or("symbol-instance");
                let mut cloned = layers.clone();
                for (index, layer) in cloned.iter_mut().enumerate() {
                    rekey_sketch_fallback(
                        layer,
                        instance_id,
                        &index.to_string(),
                        expanded_nodes,
                        max_nodes,
                        0,
                    )?;
                }
                object.insert("layers".into(), Value::Array(cloned));
                object.insert("_pentool_master_fallback".into(), Value::Bool(true));
            }
        }
        for child in object.values_mut() {
            hydrate_empty_symbol_instances(child, masters, expanded_nodes, max_nodes, depth + 1)?;
        }
    } else if let Some(array) = value.as_array_mut() {
        for child in array {
            hydrate_empty_symbol_instances(child, masters, expanded_nodes, max_nodes, depth + 1)?;
        }
    }
    Ok(())
}

fn rekey_sketch_fallback(
    value: &mut Value,
    instance_id: &str,
    path: &str,
    expanded_nodes: &mut usize,
    max_nodes: usize,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= 256,
        "[limit-exceeded] Sketch symbol fallback nesting exceeds 256"
    );
    if value.get("_class").is_some() {
        *expanded_nodes = (*expanded_nodes)
            .checked_add(1)
            .ok_or_else(|| anyhow!("[limit-exceeded] Sketch fallback node count overflow"))?;
    }
    ensure!(
        *expanded_nodes <= max_nodes,
        "[limit-exceeded] expanded Sketch symbol fallbacks exceed the configured node limit"
    );
    if let Some(object) = value.as_object_mut() {
        if let Some(old_id) = object
            .get("do_objectID")
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            object.insert(
                "_pentool_source_object_id".into(),
                Value::String(old_id.clone()),
            );
            let identity = format!("{instance_id}:{old_id}:{path}");
            let id = format!(
                "sketch-{}",
                &hex::encode(Sha256::digest(identity.as_bytes()))[..24]
            );
            object.insert("do_objectID".into(), Value::String(id));
        }
        for (key, child) in object.iter_mut() {
            rekey_sketch_fallback(
                child,
                instance_id,
                &format!("{path}/{key}"),
                expanded_nodes,
                max_nodes,
                depth + 1,
            )?;
        }
    } else if let Some(array) = value.as_array_mut() {
        for (index, child) in array.iter_mut().enumerate() {
            rekey_sketch_fallback(
                child,
                instance_id,
                &format!("{path}/{index}"),
                expanded_nodes,
                max_nodes,
                depth + 1,
            )?;
        }
    }
    Ok(())
}

/// Apply only Sketch's static string override form, and only when its object ID
/// identifies exactly one materialized text layer in the instance subtree.
/// Ambiguous IDs, other override types, and oversized values remain untouched.
fn apply_text_overrides(
    instance: &Value,
    children: &mut [Value],
    allow_rekeyed_ids: bool,
) -> (usize, usize) {
    const MAX_OVERRIDE_TEXT_BYTES: usize = 1_048_576;
    let Some(overrides) = instance.get("overrideValues").and_then(Value::as_array) else {
        return (0, 0);
    };
    let mut applied = 0;
    let mut unsupported = 0;
    for override_value in overrides {
        let Some(name) = override_value.get("overrideName").and_then(Value::as_str) else {
            unsupported += 1;
            continue;
        };
        let Some(object_id) = name.strip_suffix("_stringValue") else {
            unsupported += 1;
            continue;
        };
        let Some(text) = override_value.get("value").and_then(Value::as_str) else {
            unsupported += 1;
            continue;
        };
        if text.len() > MAX_OVERRIDE_TEXT_BYTES {
            unsupported += 1;
            continue;
        }
        let count: usize = children
            .iter()
            .map(|child| count_text_layer_matches(child, object_id, allow_rekeyed_ids))
            .sum();
        if count != 1 {
            unsupported += 1;
            continue;
        }
        if children
            .iter_mut()
            .any(|child| set_text_layer_override(child, object_id, text, allow_rekeyed_ids))
        {
            applied += 1;
        } else {
            unsupported += 1;
        }
    }
    (applied, unsupported)
}

fn count_text_layer_matches(node: &Value, object_id: &str, allow_rekeyed_ids: bool) -> usize {
    if node.get("_class").and_then(Value::as_str) == Some("text")
        && sketch_source_object_id(node, allow_rekeyed_ids) == Some(object_id)
    {
        return 1;
    }
    let Some(object) = node.as_object() else {
        return 0;
    };
    ["layers", "children"]
        .into_iter()
        .filter_map(|key| object.get(key).and_then(Value::as_array))
        .flatten()
        .map(|child| count_text_layer_matches(child, object_id, allow_rekeyed_ids))
        .sum()
}

fn set_text_layer_override(
    node: &mut Value,
    object_id: &str,
    text: &str,
    allow_rekeyed_ids: bool,
) -> bool {
    if node.get("_class").and_then(Value::as_str) == Some("text")
        && sketch_source_object_id(node, allow_rekeyed_ids) == Some(object_id)
    {
        let Some(object) = node.as_object_mut() else {
            return false;
        };
        let attributed = object
            .entry("attributedString")
            .or_insert_with(|| json!({"attributes":[]}));
        if !attributed.is_object() {
            return false;
        }
        attributed["string"] = Value::String(text.to_owned());
        return true;
    }
    let Some(object) = node.as_object_mut() else {
        return false;
    };
    for key in ["layers", "children"] {
        if let Some(items) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in items {
                if set_text_layer_override(child, object_id, text, allow_rekeyed_ids) {
                    return true;
                }
            }
        }
    }
    false
}

fn sketch_source_object_id(node: &Value, allow_rekeyed_ids: bool) -> Option<&str> {
    if allow_rekeyed_ids {
        node.get("_pentool_source_object_id")
            .and_then(Value::as_str)
            .or_else(|| node.get("do_objectID").and_then(Value::as_str))
    } else {
        node.get("do_objectID").and_then(Value::as_str)
    }
}

fn clear_private_import_markers(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.remove("_pentool_source_object_id");
        object.remove("_pentool_master_fallback");
        for child in object.values_mut() {
            clear_private_import_markers(child);
        }
    } else if let Some(array) = value.as_array_mut() {
        for child in array {
            clear_private_import_markers(child);
        }
    }
}

fn canonical_page_member(reference: &str) -> Result<String> {
    let candidate = reference.replace('\\', "/");
    let candidate = candidate.strip_prefix("pages/").unwrap_or(&candidate);
    ensure!(
        !candidate.is_empty()
            && !candidate.starts_with('/')
            && !candidate
                .as_bytes()
                .get(1)
                .is_some_and(|byte| *byte == b':')
            && !candidate.contains('/')
            && candidate
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
        "[unsafe-path] Sketch page reference is not a safe relative page name"
    );
    let leaf = candidate.rsplit('/').next().unwrap_or(candidate);
    let leaf = leaf.strip_suffix(".json").unwrap_or(leaf);
    ensure!(
        !leaf.is_empty()
            && !leaf.contains(':')
            && leaf.len() <= 250
            && leaf
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "[unsafe-path] Sketch page reference must name a JSON page member"
    );
    Ok(format!("pages/{leaf}.json"))
}

fn read_json_member<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    entries: &BTreeMap<String, interchange::ArchiveEntryInfo>,
    name: &str,
) -> Result<Value> {
    let info = entries
        .get(name)
        .ok_or_else(|| anyhow!("[missing-resource] Sketch archive is missing {name}"))?;
    ensure!(!info.is_dir, "[malformed-resource] {name} is a directory");
    ensure!(
        info.uncompressed_bytes <= MAX_JSON_ENTRY_BYTES,
        "[limit-exceeded] Sketch JSON member {name} exceeds {MAX_JSON_ENTRY_BYTES} bytes"
    );
    let member = archive
        .by_name(name)
        .with_context(|| format!("[malformed-resource] cannot open Sketch member {name}"))?;
    let mut limited = member.take(info.uncompressed_bytes.saturating_add(1));
    let mut raw = Vec::with_capacity(info.uncompressed_bytes as usize);
    limited
        .read_to_end(&mut raw)
        .with_context(|| format!("[malformed-resource] cannot decompress Sketch member {name}"))?;
    ensure!(
        raw.len() as u64 == info.uncompressed_bytes,
        "[malformed-resource] Sketch member {name} size does not match its ZIP directory"
    );
    serde_json::from_slice(&raw)
        .with_context(|| format!("[malformed-resource] Sketch member {name} is not valid JSON"))
}

#[derive(Default)]
struct IdContext {
    used: HashSet<String>,
}

impl IdContext {
    fn id(&mut self, kind: &str, source_id: &str, path: &[usize]) -> String {
        let location = path
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join("-");
        let base = stable_id(kind, source_id, &location);
        if self.used.insert(base.clone()) {
            return base;
        }
        let mut suffix = 2usize;
        loop {
            let candidate = format!("{base}-{suffix}");
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
            suffix += 1;
        }
    }
}

fn stable_id(kind: &str, source_id: &str, location: &str) -> String {
    let digest = Sha256::digest(format!("{kind}\0{source_id}\0{location}").as_bytes());
    let suffix = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let slug = source_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .take(24)
        .collect::<String>();
    if slug.is_empty() {
        format!("sketch-{kind}-{suffix}")
    } else {
        format!("sketch-{kind}-{slug}-{suffix}")
    }
}

/// Visit symbol masters without recursive Rust calls. The archive/JSON byte
/// ceilings bound memory; this additionally bounds pre-scan work.
fn scan_symbol_masters<'a>(root: &'a Value, visit: &mut impl FnMut(&'a Value)) {
    let mut pending = vec![(root, 0usize)];
    let mut seen = 0usize;
    while let Some((node, depth)) = pending.pop() {
        seen += 1;
        if seen > 1_000_000 || depth > 128 {
            continue;
        }
        if string_field(node, "_class").as_deref() == Some("symbolMaster") {
            visit(node);
        }
        if let Some(children) = node.get("layers").and_then(Value::as_array) {
            pending.extend(children.iter().map(|child| (child, depth + 1)));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn convert_node(
    source: &Value,
    ids: &mut IdContext,
    report: &mut FidelityReportBuilder,
    limits: &ImportLimits,
    depth: usize,
    path: &[usize],
    total_nodes: &mut usize,
    parent_origin: (f64, f64),
    max_x: &mut f64,
    max_y: &mut f64,
    min_x: &mut f64,
    min_y: &mut f64,
    symbol_components: &BTreeMap<String, String>,
    layer_styles: &BTreeMap<String, Value>,
    text_styles: &BTreeMap<String, Value>,
    snapshot: bool,
) -> Result<Option<Value>> {
    limits.check_nesting(depth)?;
    *total_nodes = total_nodes
        .checked_add(1)
        .ok_or_else(|| anyhow!("[limit-exceeded] Sketch node count overflow"))?;
    limits.check_node_count(*total_nodes)?;
    let class = string_field(source, "_class").unwrap_or_else(|| "unknown".into());
    let source_id = string_field(source, "do_objectID").unwrap_or_else(|| {
        format!(
            "{}-{}",
            class,
            path.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("-")
        )
    });
    let name = string_field(source, "name").unwrap_or_default();
    let report_source_id = source_id.clone();
    let frame = frame(source)?;
    let (x, y, width, height) = frame.unwrap_or((0.0, 0.0, 0.0, 0.0));
    let absolute_x = parent_origin.0 + x;
    let absolute_y = parent_origin.1 + y;
    *max_x = max_x.max(absolute_x + width);
    *max_y = max_y.max(absolute_y + height);
    *min_x = min_x.min(absolute_x);
    *min_y = min_y.min(absolute_y);
    let shared_style_id = ["sharedStyleID", "sharedObjectID"].iter().find_map(|key| {
        string_field(source, key).or_else(|| source.get("style").and_then(|v| string_field(v, key)))
    });
    let resolved_shared_style = shared_style_id.as_ref().and_then(|id| {
        if class == "text" {
            text_styles.get(id)
        } else {
            layer_styles.get(id)
        }
    });
    let layer_opacity = source.get("opacity").and_then(number);
    let effective_style = effective_layer_style(source.get("style"), resolved_shared_style);
    if let Some(shared) = resolved_shared_style {
        report.record(
            &source_id,
            "shared_style",
            FidelityStatus::Approximated,
            "document-level Sketch shared style resolved; supported appearance fields were applied",
            Some("review reported effects and style details after import".into()),
        );
        if class == "text" {
            if let Some(text_style) = shared.get("textStyle") {
                let paragraph = text_style.get("paragraphStyle");
                if let Some(alignment) = paragraph.and_then(sketch_paragraph_alignment) {
                    report.record(
                        &source_id,
                        "shared_text_alignment",
                        FidelityStatus::Approximated,
                        format!("shared Sketch paragraph alignment mapped to `{alignment}`"),
                        None,
                    );
                } else if paragraph.and_then(|value| value.get("alignment")).is_some() {
                    report.record(
                        &source_id,
                        "shared_text_alignment",
                        FidelityStatus::Dropped,
                        "shared Sketch paragraph alignment is outside the supported left/center/right subset",
                        Some("set paragraph alignment to left, center, or right in Sketch".into()),
                    );
                }
                if paragraph
                    .and_then(Value::as_object)
                    .is_some_and(|values| values.keys().any(|key| key != "alignment"))
                {
                    report.record(
                        &source_id,
                        "shared_text_paragraph",
                        FidelityStatus::Dropped,
                        "shared Sketch paragraph spacing, indents, tabs, and line-height details are not mapped",
                        Some("review paragraph spacing and line-height after import".into()),
                    );
                }
                if let Some(attrs) = text_style
                    .get("encodedAttributes")
                    .and_then(Value::as_object)
                {
                    if attrs.keys().any(|key| {
                        key != "MSAttributedStringFontAttribute"
                            && key != "MSAttributedStringColorAttribute"
                    }) {
                        report.record(&source_id, "shared_text_attributes", FidelityStatus::Dropped,
                            "unsupported encoded attributes in the shared Sketch text style were omitted", None);
                    }
                }
            }
        } else if let Some(object) = shared.as_object() {
            for field in [
                "innerShadows",
                "contextSettings",
                "borderOptions",
                "blendingMode",
            ] {
                if object.get(field).is_some_and(|value| !value.is_null()) {
                    report.record(
                        &source_id,
                        format!("shared_{field}"),
                        FidelityStatus::Dropped,
                        format!("shared Sketch {field} is not mapped by this importer"),
                        None,
                    );
                }
            }
        }
    } else if shared_style_id.is_some() {
        report.record(
            &source_id,
            "shared_style",
            FidelityStatus::Dropped,
            "Sketch shared style reference is not resolved; inline appearance is imported when available",
            Some("apply or detach the shared style in Sketch before importing".into()),
        );
    }
    let local_name = name;
    for (field, description) in [
        ("rotation", "layer rotation"),
        ("blendMode", "layer blend mode"),
        ("hasClippingMask", "clipping mask"),
    ] {
        if source.get(field).is_some_and(|value| {
            !value.is_null() && value != &json!(0) && value != &json!(1) && value != &json!(false)
        }) {
            report.record(
                &source_id,
                description,
                FidelityStatus::Dropped,
                format!("Sketch {description} is not interpreted by this importer"),
                Some("flatten or remove this effect in Sketch before importing".into()),
            );
        }
    }
    if source.get("opacity").is_some_and(|value| !value.is_null())
        && !layer_opacity.is_some_and(|opacity| (0.0..=1.0).contains(&opacity))
    {
        report.record(
            &source_id,
            "layer_opacity",
            FidelityStatus::Dropped,
            "invalid Sketch layer opacity was ignored",
            Some("use an opacity between 0 and 1".into()),
        );
    }
    let mut node = match class.as_str() {
        "symbolMaster" if !snapshot => {
            report.record(
                &source_id,
                "symbol_master",
                FidelityStatus::Exact,
                "symbol master is retained as a document component, not painted as a page object",
                None,
            );
            None
        }
        "artboard" | "group" | "layerGroup" | "symbolMaster" => {
            let source_children = source
                .get("layers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut children = Vec::with_capacity(source_children.len());
            for (index, child) in source_children.iter().enumerate() {
                let mut child_path = path.to_vec();
                child_path.push(index);
                if let Some(converted) = convert_node(
                    child,
                    ids,
                    report,
                    limits,
                    depth + 1,
                    &child_path,
                    total_nodes,
                    (absolute_x, absolute_y),
                    max_x,
                    max_y,
                    min_x,
                    min_y,
                    symbol_components,
                    layer_styles,
                    text_styles,
                    snapshot,
                )? {
                    children.push(converted);
                }
            }
            let mut group = json!({
                "kind": "group",
                "id": ids.id(&class, &source_id, path),
                "children": children,
                "transform": [1, 0, 0, 1, absolute_x - parent_origin.0, absolute_y - parent_origin.1]
            });
            if !local_name.is_empty() {
                group["name"] = json!(local_name);
            }
            if class == "artboard" {
                // A background rectangle is supported only when the export has
                // a simple explicit color; otherwise the artboard stays clear.
                if let Some(color) = source
                    .get("hasBackgroundColor")
                    .and_then(Value::as_bool)
                    .filter(|value| *value)
                    .and_then(|_| source.get("backgroundColor"))
                    .and_then(color_string)
                {
                    if let Some(children) = group["children"].as_array_mut() {
                        children.insert(
                            0,
                            json!({
                                "kind":"rect", "id":ids.id("artboard-background", &source_id, path),
                                "x":0, "y":0, "width":width, "height":height,
                                "style":{"fill":{"fallback":color}}
                            }),
                        );
                    }
                }
            }
            report.record(
                source_id,
                if class == "artboard" {
                    "artboard"
                } else {
                    "group"
                },
                FidelityStatus::Exact,
                "container structure imported as an ordered Pentool group",
                None,
            );
            Some(group)
        }
        "rectangle" | "rectangleShape" => {
            let style = style(effective_style.as_ref(), report, &source_id);
            let mut rect = json!({
                "kind":"rect", "id":ids.id("rect", &source_id, path),
                "x":x, "y":y, "width":width, "height":height, "style":style
            });
            if let Some(radius) = source.get("fixedRadius").and_then(number) {
                rect["radius_x"] = json!(radius.max(0.0));
                rect["radius_y"] = json!(radius.max(0.0));
            }
            add_name(&mut rect, local_name);
            report.record(
                source_id,
                "rectangle",
                FidelityStatus::Exact,
                "rectangle geometry mapped",
                None,
            );
            Some(rect)
        }
        "oval" | "ellipse" => {
            let style = style(effective_style.as_ref(), report, &source_id);
            let mut ellipse = json!({
                "kind":"ellipse", "id":ids.id("ellipse", &source_id, path),
                "cx":x + width / 2.0, "cy":y + height / 2.0,
                "radius_x":width / 2.0, "radius_y":height / 2.0, "style":style
            });
            add_name(&mut ellipse, local_name);
            report.record(
                source_id,
                "ellipse",
                FidelityStatus::Exact,
                "ellipse geometry mapped",
                None,
            );
            Some(ellipse)
        }
        "text" => {
            let text = convert_text(
                source,
                resolved_shared_style,
                ids,
                path,
                x,
                y,
                width,
                height,
                &source_id,
                report,
            )?;
            report.record(
                source_id,
                "text",
                FidelityStatus::Approximated,
                "text content and frame imported; Sketch rich text styling is not fully preserved",
                Some("review font, paragraph, and run styling after import".into()),
            );
            Some(text)
        }
        "shapePath" => {
            let path_data = straight_path(source, x, y, width, height, &source_id, report)?;
            let style = style(effective_style.as_ref(), report, &source_id);
            let path_node = json!({
                "kind":"path", "id":ids.id("path", &source_id, path), "d":path_data, "style":style
            });
            report.record(source_id, "path", FidelityStatus::Approximated, "straight-segment path imported; curve controls and winding details are not interpreted", Some("convert curved paths to outlines or simplify them in Sketch".into()));
            Some(path_node)
        }
        "symbolInstance" => {
            let Some(children) = source
                .get("layers")
                .and_then(Value::as_array)
                .filter(|layers| !layers.is_empty())
            else {
                report.record(&source_id, "symbol_instance", FidelityStatus::Dropped,
                    "Sketch symbol instance has no materialized child appearance; no empty node was emitted",
                    Some("provide materialized instance layers in the Sketch archive".into()));
                return Ok(None);
            };
            let mut materialized_children = children.clone();
            let master_fallback = source
                .get("_pentool_master_fallback")
                .and_then(Value::as_bool)
                == Some(true);
            let override_counts =
                apply_text_overrides(source, &mut materialized_children, master_fallback);
            let mut fallback_children = Vec::with_capacity(materialized_children.len());
            for (index, child) in materialized_children.iter().enumerate() {
                let mut child_path = path.to_vec();
                child_path.push(index);
                if let Some(child) = convert_node(
                    child,
                    ids,
                    report,
                    limits,
                    depth + 1,
                    &child_path,
                    total_nodes,
                    (absolute_x, absolute_y),
                    max_x,
                    max_y,
                    min_x,
                    min_y,
                    symbol_components,
                    layer_styles,
                    text_styles,
                    snapshot,
                )? {
                    fallback_children.push(child);
                }
            }
            let fallback = json!({"kind":"group","id":ids.id("instance-fallback", &source_id, path),"children":fallback_children,"transform":[1,0,0,1,0,0]});
            let symbol_id = string_field(source, "symbolID").unwrap_or_default();
            if !snapshot {
                if let Some(component) = symbol_components.get(&symbol_id) {
                    if override_counts.0 > 0 {
                        report.record(&source_id, "symbol_text_overrides", FidelityStatus::Approximated,
                            "uniquely matched Sketch string overrides were applied to materialized text fallback nodes",
                            None);
                    }
                    if override_counts.1 > 0 {
                        report.record(&source_id, "symbol_overrides", FidelityStatus::Dropped,
                        if master_fallback { "unsupported Sketch override metadata was not applied; the known master appearance is retained in the instance fallback" } else { "unsupported Sketch override metadata was not applied; materialized appearance is retained in the instance fallback" },
                        Some("review unsupported text and style overrides after import".into()));
                    }
                    report.record(&source_id, "symbol_instance", FidelityStatus::Approximated,
                    if master_fallback { "instance links to its symbol master and uses a bounded copy of the known master appearance as fallback" } else { "instance links to its symbol master and retains materialized appearance as fallback" },
                    Some("review Sketch instance overrides after import".into()));
                    Some(
                        json!({"kind":"instance","id":ids.id("instance", &source_id, path),"component":component,"fallback":fallback,"overrides":{},"transform":[1,0,0,1,absolute_x-parent_origin.0,absolute_y-parent_origin.1]}),
                    )
                } else {
                    report.record(&source_id, "symbol_instance", FidelityStatus::Approximated,
                    "symbol master is unavailable; materialized supported appearance is retained as a group", None);
                    Some(fallback)
                }
            } else {
                report.record(
                    &source_id,
                    "symbol_instance",
                    FidelityStatus::Approximated,
                    "materialized symbol appearance retained inside component snapshot",
                    None,
                );
                Some(fallback)
            }
        }
        "image" | "slice" | "hotSpot" => {
            report.record(
                source_id,
                class,
                FidelityStatus::Dropped,
                "Sketch object type is outside the supported static vector subset",
                Some("convert the object to supported vector shapes or text in Sketch".into()),
            );
            None
        }
        _ => {
            report.record(
                source_id,
                "object",
                FidelityStatus::Dropped,
                format!("Sketch object class `{class}` is not supported"),
                Some("convert the object to a static vector shape or text in Sketch".into()),
            );
            None
        }
    };
    if let Some(node) = node.as_mut() {
        let effects = shadow_effects(effective_style.as_ref(), &report_source_id, report);
        if !effects.is_empty() {
            node["effects"] = json!(effects);
        }
    }
    if let (Some(node), Some(opacity)) = (node.as_mut(), layer_opacity) {
        if (0.0..=1.0).contains(&opacity) && opacity != 1.0 {
            node["opacity"] = json!(opacity);
        }
    }
    Ok(node)
}

fn frame(source: &Value) -> Result<Option<(f64, f64, f64, f64)>> {
    let Some(frame) = source.get("frame") else {
        return Ok(None);
    };
    let get = |key: &str| -> Result<f64> {
        let value = frame
            .get(key)
            .and_then(number)
            .ok_or_else(|| anyhow!("[malformed-resource] Sketch frame is missing finite {key}"))?;
        ensure!(
            value.abs() <= MAX_CANVAS_EDGE * 16.0,
            "[limit-exceeded] Sketch frame {key} exceeds the coordinate limit"
        );
        Ok(value)
    };
    let x = get("x")?;
    let y = get("y")?;
    let width = get("width")?;
    let height = get("height")?;
    ensure!(
        width >= 0.0 && height >= 0.0,
        "[malformed-resource] Sketch frame has negative dimensions"
    );
    Ok(Some((x, y, width, height)))
}

fn checked_canvas(edge: f64) -> Result<u32> {
    ensure!(
        edge.is_finite() && (1.0..=MAX_CANVAS_EDGE).contains(&edge),
        "[limit-exceeded] Sketch page canvas exceeds {MAX_CANVAS_EDGE} pixels"
    );
    Ok(edge.ceil() as u32)
}

fn shared_style_map(
    document: &Value,
    key: &str,
    max_styles: usize,
    report: &mut FidelityReportBuilder,
) -> Result<BTreeMap<String, Value>> {
    let mut styles = BTreeMap::new();
    let Some(objects) = document
        .get(key)
        .and_then(|v| v.get("objects"))
        .and_then(Value::as_array)
    else {
        return Ok(styles);
    };
    ensure!(
        objects.len() <= max_styles,
        "[limit-exceeded] Sketch shared style count exceeds configured node limit"
    );
    for object in objects {
        let Some(id) = string_field(object, "do_objectID").filter(|id| !id.is_empty()) else {
            continue;
        };
        let value = object.get("value").cloned().unwrap_or(Value::Null);
        ensure!(
            styles.insert(id, value).is_none(),
            "[malformed-resource] Sketch document contains duplicate shared style identifiers"
        );
    }
    if !styles.is_empty() {
        report.record(
            "document",
            key,
            FidelityStatus::Exact,
            format!(
                "{} document-level shared style definitions indexed",
                styles.len()
            ),
            None,
        );
    }
    Ok(styles)
}

fn effective_layer_style<'a>(
    inline: Option<&'a Value>,
    shared: Option<&'a Value>,
) -> Option<Value> {
    let (Some(shared), inline) = (shared, inline) else {
        return inline.cloned();
    };
    let mut merged = shared.clone();
    if let (Some(dst), Some(src)) = (merged.as_object_mut(), inline.and_then(Value::as_object)) {
        for field in ["fills", "borders", "shadows", "innerShadows"] {
            if let Some(value) = src.get(field) {
                dst.insert(field.into(), value.clone());
            }
        }
    }
    Some(merged)
}

fn style(source: Option<&Value>, report: &mut FidelityReportBuilder, source_id: &str) -> Value {
    let mut result = Map::new();
    let Some(source) = source else {
        return Value::Object(result);
    };
    let fills = source.get("fills").and_then(Value::as_array);
    let visible_fills: Vec<_> = fills
        .into_iter()
        .flatten()
        .filter(|fill| fill.get("isEnabled").and_then(Value::as_bool) != Some(false))
        .collect();
    if let Some(fill) = visible_fills.first() {
        if let Some(color) = fill.get("color").and_then(color_string) {
            let opacity = context_opacity(fill, "fill", source_id, report);
            result.insert(
                "fill".into(),
                json!({"fallback":multiply_alpha(&color, opacity)}),
            );
        }
        report_unsupported_context_blend(fill, "fill", source_id, report);
        if visible_fills.len() > 1 {
            report.record(
                source_id,
                "additional-fills",
                FidelityStatus::Dropped,
                "only the first enabled solid fill is imported",
                Some("flatten additional fills in Sketch before importing".into()),
            );
        }
        if fill
            .get("fillType")
            .and_then(Value::as_i64)
            .is_some_and(|kind| kind != 0)
        {
            report.record(
                source_id,
                "fill-type",
                FidelityStatus::Dropped,
                "gradient or pattern fill is not interpreted by the Sketch subset",
                Some("replace with a solid fill or flatten in Sketch".into()),
            );
        }
    }
    let borders = source.get("borders").and_then(Value::as_array);
    if let Some(border) = borders
        .into_iter()
        .flatten()
        .find(|border| border.get("isEnabled").and_then(Value::as_bool) != Some(false))
    {
        if let Some(color) = border.get("color").and_then(color_string) {
            let opacity = context_opacity(border, "border", source_id, report);
            result.insert(
                "stroke".into(),
                json!({"fallback":multiply_alpha(&color, opacity)}),
            );
            if let Some(width) = border
                .get("thickness")
                .and_then(number)
                .filter(|n| *n >= 0.0)
            {
                result.insert("stroke_width".into(), json!({"fallback":width}));
            }
        }
        report_unsupported_context_blend(border, "border", source_id, report);
        if borders
            .into_iter()
            .flatten()
            .filter(|border| border.get("isEnabled").and_then(Value::as_bool) != Some(false))
            .count()
            > 1
        {
            report.record(
                source_id,
                "additional-borders",
                FidelityStatus::Dropped,
                "only the first enabled solid border is imported",
                Some("flatten additional borders in Sketch before importing".into()),
            );
        }
    }
    Value::Object(result)
}

fn shadow_effects(
    source: Option<&Value>,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) -> Vec<Value> {
    let Some(source) = source else {
        return Vec::new();
    };
    let mut effects = Vec::new();
    for (field, effect_kind, feature, label) in [
        ("shadows", "drop-shadow", "shadow", "outer"),
        ("innerShadows", "inner-shadow", "inner_shadow", "inner"),
    ] {
        let Some(shadows) = source.get(field).and_then(Value::as_array) else {
            continue;
        };
        for (index, shadow) in shadows.iter().enumerate() {
            if shadow.get("isEnabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let valid_numeric_fields = ["offsetX", "offsetY", "blur", "spread"]
                .into_iter()
                .all(|key| shadow.get(key).is_none_or(|value| number(value).is_some()));
            let offset_x = shadow.get("offsetX").and_then(number).unwrap_or(0.0);
            let offset_y = shadow.get("offsetY").and_then(number).unwrap_or(0.0);
            let blur = shadow.get("blur").and_then(number).unwrap_or(0.0);
            let spread = shadow.get("spread").and_then(number).unwrap_or(0.0);
            let color = shadow.get("color").and_then(color_string);
            let blend_value = shadow
                .get("contextSettings")
                .and_then(|settings| settings.get("blendMode"));
            let valid_blend = blend_value.is_none_or(|value| number(value) == Some(0.0));
            let supported = valid_numeric_fields
                && offset_x.is_finite()
                && (-16384.0..=16384.0).contains(&offset_x)
                && offset_y.is_finite()
                && (-16384.0..=16384.0).contains(&offset_y)
                && blur.is_finite()
                && (0.0..=256.0).contains(&blur)
                && spread == 0.0
                && color.is_some()
                && valid_blend;
            if !supported || effects.len() >= crate::effects::MAX_EFFECTS {
                report.record(
                source_id,
                feature,
                FidelityStatus::Dropped,
                format!("Sketch {label} shadow {index} uses unsupported or invalid parameters"),
                Some("use a normal-blend shadow with zero spread, valid color, finite offsets, and blur at most 256".into()),
            );
                continue;
            }
            let color = multiply_alpha(
                color.as_deref().unwrap_or("#000000"),
                context_opacity(shadow, feature, source_id, report),
            );
            effects.push(json!({
                "id": format!("sketch-{source_id}-{feature}-{index}"),
                "kind": effect_kind,
                "version": 1,
                "enabled": true,
                "opacity": 1.0,
                "blend_mode": "normal",
                "params": {"x": offset_x, "y": offset_y, "blur": blur, "color": color}
            }));
            report.record(
                source_id,
                feature,
                FidelityStatus::Approximated,
                format!("Sketch {label} shadow {index} imported as a native {effect_kind}"),
                Some("review shadow clipping and appearance in Pentool".into()),
            );
        }
    }
    effects
}

fn context_opacity(
    paint: &Value,
    feature: &str,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) -> f64 {
    let Some(value) = paint
        .get("contextSettings")
        .and_then(|settings| settings.get("opacity"))
    else {
        return 1.0;
    };
    if let Some(opacity) = number(value).filter(|opacity| (0.0..=1.0).contains(opacity)) {
        opacity
    } else {
        report.record(
            source_id,
            format!("{feature}_opacity"),
            FidelityStatus::Dropped,
            format!("invalid Sketch {feature} opacity was ignored"),
            Some("use an opacity between 0 and 1".into()),
        );
        1.0
    }
}

fn report_unsupported_context_blend(
    paint: &Value,
    feature: &str,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) {
    let Some(mode) = paint
        .get("contextSettings")
        .and_then(|settings| settings.get("blendMode"))
        .and_then(number)
    else {
        return;
    };
    if mode != 0.0 {
        report.record(
            source_id,
            format!("{feature}_blend_mode"),
            FidelityStatus::Dropped,
            format!("Sketch {feature} blend mode {mode} is not imported"),
            Some("use the normal blend mode or flatten the appearance in Sketch".into()),
        );
    }
}

fn multiply_alpha(color: &str, opacity: f64) -> String {
    let hex = color.strip_prefix('#').unwrap_or(color);
    let byte = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).unwrap();
    let (r, g, b, alpha) = (
        byte(0..2),
        byte(2..4),
        byte(4..6),
        if hex.len() == 8 { byte(6..8) } else { 255 },
    );
    let alpha = (f64::from(alpha) * opacity).round() as u8;
    if alpha == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{alpha:02x}")
    }
}

fn color_string(color: &Value) -> Option<String> {
    let channel = |key: &str| -> Option<u8> {
        let raw = number(color.get(key)?)?;
        if !(0.0..=MAX_COLOR).contains(&raw) {
            return None;
        }
        Some((raw * 255.0).round() as u8)
    };
    let r = channel("red")?;
    let g = channel("green")?;
    let b = channel("blue")?;
    let a = channel("alpha").unwrap_or(255);
    Some(if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    })
}

#[allow(clippy::too_many_arguments)]
fn convert_text(
    source: &Value,
    shared_style: Option<&Value>,
    ids: &mut IdContext,
    path: &[usize],
    x: f64,
    y: f64,
    _width: f64,
    _height: f64,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) -> Result<Value> {
    let attributed = source
        .get(" attributedString")
        .or_else(|| source.get("attributedString"));
    let content = attributed
        .and_then(|value| value.get("string"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let first_attributes = attributed
        .and_then(|value| value.get("attributes"))
        .and_then(Value::as_array)
        .and_then(|runs| runs.first())
        .and_then(|run| run.get("attributes"));
    let encoded = shared_style
        .and_then(|v| v.get("textStyle"))
        .and_then(|v| v.get("encodedAttributes"));
    let font = first_attributes
        .and_then(|attrs| attrs.get("MSAttributedStringFontAttribute"))
        .or_else(|| encoded.and_then(|attrs| attrs.get("MSAttributedStringFontAttribute")));
    let family = font
        .and_then(|font| {
            font.get("name")
                .or_else(|| font.pointer("/attributes/name"))
        })
        .and_then(Value::as_str)
        .unwrap_or("Arial");
    let font_size = font
        .and_then(|font| {
            font.get("size")
                .or_else(|| font.pointer("/attributes/size"))
        })
        .and_then(number)
        .unwrap_or(16.0);
    ensure!(
        font_size.is_finite() && (0.0..=4096.0).contains(&font_size),
        "[limit-exceeded] Sketch text font size is invalid or too large"
    );
    let fill = first_attributes
        .and_then(|attrs| attrs.get("MSAttributedStringColorAttribute"))
        .or_else(|| encoded.and_then(|attrs| attrs.get("MSAttributedStringColorAttribute")))
        .and_then(color_string)
        .unwrap_or_else(|| "#000000".into());
    let alignment = shared_style.and_then(sketch_paragraph_alignment);
    let mut text = json!({
        "kind":"text", "id":ids.id("text", source_id, path), "content":content,
        "x":x, "y":y + font_size, "font_family":family, "font_size":font_size,
        "style":{"fill":{"fallback":fill}}
    });
    if let Some(alignment) = alignment {
        text["align"] = json!(alignment);
    }
    add_name(&mut text, string_field(source, "name").unwrap_or_default());
    if content.is_empty() {
        report.record(
            source_id,
            "empty-text",
            FidelityStatus::Approximated,
            "empty Sketch text object imported as an empty Pentool text node",
            None,
        );
    }
    Ok(text)
}

fn sketch_paragraph_alignment(paragraph: &Value) -> Option<&'static str> {
    let paragraph = paragraph
        .pointer("/textStyle/paragraphStyle")
        .unwrap_or(paragraph);
    match paragraph.get("alignment")?.as_u64()? {
        0 => Some("left"),
        1 => Some("right"),
        2 => Some("center"),
        _ => None,
    }
}

fn straight_path(
    source: &Value,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) -> Result<String> {
    let paths = source
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("[malformed-resource] Sketch shapePath has no points array"))?;
    ensure!(
        !paths.is_empty() && paths.len() <= 100_000,
        "[limit-exceeded] Sketch path point count is outside supported bounds"
    );
    let mut coords = Vec::with_capacity(paths.len());
    for point in paths {
        let raw = point
            .get("point")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("[malformed-resource] Sketch point has no coordinate pair"))?;
        let (nx, ny) = raw
            .split_once(',')
            .ok_or_else(|| anyhow!("[malformed-resource] Sketch point is not an x,y pair"))?;
        let nx: f64 = nx
            .trim()
            .parse()
            .context("[malformed-resource] invalid Sketch point x")?;
        let ny: f64 = ny
            .trim()
            .parse()
            .context("[malformed-resource] invalid Sketch point y")?;
        ensure!(
            nx.is_finite() && ny.is_finite() && nx.abs() <= 16.0 && ny.abs() <= 16.0,
            "[limit-exceeded] Sketch path point exceeds supported normalized bounds"
        );
        coords.push((x + nx * width, y + ny * height));
        if point
            .get("curveFrom")
            .and_then(Value::as_str)
            .is_some_and(|curve| curve != "-1,-1")
            || point
                .get("curveTo")
                .and_then(Value::as_str)
                .is_some_and(|curve| curve != "-1,-1")
        {
            report.record(
                source_id,
                "bezier-controls",
                FidelityStatus::Dropped,
                "Bezier control points are not imported; the anchor polygon is used",
                Some("simplify the path to straight segments in Sketch".into()),
            );
        }
    }
    let closed = source
        .get("isClosed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut data = String::new();
    for (index, (px, py)) in coords.iter().enumerate() {
        data.push_str(if index == 0 { "M " } else { " L " });
        data.push_str(&format!("{} {}", clean_float(*px), clean_float(*py)));
    }
    if closed {
        data.push_str(" Z");
    }
    Ok(data)
}

fn clean_float(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        value.to_string()
    }
}

fn number(value: &Value) -> Option<f64> {
    let value = value.as_f64()?;
    value.is_finite().then_some(value)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn add_name(node: &mut Value, name: String) {
    if !name.is_empty() {
        node["name"] = json!(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn sketch_zip(document: &Value, pages: &[(&str, Value)]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            let options = SimpleFileOptions::default();
            writer.start_file("document.json", options).unwrap();
            writer
                .write_all(&serde_json::to_vec(document).unwrap())
                .unwrap();
            for (name, page) in pages {
                writer.start_file(*name, options).unwrap();
                writer
                    .write_all(&serde_json::to_vec(page).unwrap())
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn archive_with_member(name: &str, contents: &[u8], method: zip::CompressionMethod) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            let options = SimpleFileOptions::default().compression_method(method);
            writer.start_file(name, options).unwrap();
            writer.write_all(contents).unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn rejects_sketch_archive_member_path_traversal_before_reading_json() {
        let bytes = archive_with_member("..\\outside.json", b"{}", zip::CompressionMethod::Stored);
        let error = import(&bytes, "hostile.sketch").err().unwrap().to_string();
        assert!(error.contains("unsafe-path"), "{error}");
    }

    #[test]
    fn rejects_sketch_archive_compression_ratio_bomb_from_metadata() {
        let repeated = vec![0u8; 16 * 1024];
        let bytes = archive_with_member(
            "assets/repeated.bin",
            &repeated,
            zip::CompressionMethod::Deflated,
        );
        let error = import(&bytes, "hostile.sketch").err().unwrap().to_string();
        assert!(error.contains("compression-ratio"), "{error}");
    }

    #[test]
    fn imports_artboards_rectangles_text_and_stable_ids() {
        let document = json!({"pages":[{"_ref":"pages/page-1"}]});
        let page = json!({"_class":"page","do_objectID":"page-id","name":"Poster","layers":[{
            "_class":"artboard","do_objectID":"board","name":"Cover","frame":{"x":0,"y":0,"width":200,"height":120},
            "layers":[
                {"_class":"rectangle","do_objectID":"rect","name":"Card","frame":{"x":10,"y":12,"width":80,"height":50},"style":{"fills":[{"isEnabled":true,"color":{"red":0.2,"green":0.4,"blue":0.6,"alpha":1}}]}},
                {"_class":"text","do_objectID":"title","name":"Title","frame":{"x":20,"y":20,"width":150,"height":30},"attributedString":{"string":"Hello","attributes":[{"attributes":{"MSAttributedStringFontAttribute":{"name":"Arial","size":18},"MSAttributedStringColorAttribute":{"red":1,"green":0,"blue":0,"alpha":1}}}]}}
            ]
        }]});
        let bytes = sketch_zip(&document, &[("pages/page-1.json", page)]);
        let first = import(&bytes, "poster.sketch").unwrap();
        let second = import(&bytes, "poster.sketch").unwrap();
        assert_eq!(first.pages, second.pages);
        assert_eq!(first.pages[0]["name"], "Poster");
        let board = &first.pages[0]["layers"][0]["nodes"][0];
        assert_eq!(board["kind"], "group");
        assert_eq!(board["children"][0]["kind"], "rect");
        assert_eq!(board["children"][1]["content"], "Hello");
        assert_eq!(first.report.format, "sketch");
        assert!(first.report.counts.approximated > 0);
    }

    #[test]
    fn preserves_supported_layer_opacity_and_reports_invalid_values() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"rectangle","do_objectID":"translucent","opacity":0.35,
             "frame":{"x":0,"y":0,"width":20,"height":10}},
            {"_class":"rectangle","do_objectID":"invalid","opacity":1.5,
             "frame":{"x":30,"y":0,"width":20,"height":10}}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/p"}]}),
            &[("pages/p.json", page)],
        );
        let imported = import(&bytes, "opacity.sketch").unwrap();
        let nodes = &imported.pages[0]["layers"][0]["nodes"];
        assert_eq!(nodes[0]["opacity"], 0.35);
        assert!(nodes[1].get("opacity").is_none());
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "invalid"
                && item.feature == "layer_opacity"
                && item.status == FidelityStatus::Dropped
        }));
        assert!(!imported
            .report
            .items
            .iter()
            .any(|item| { item.source_id == "translucent" && item.feature == "layer_opacity" }));
    }

    #[test]
    fn refuses_traversal_page_refs_before_lookup() {
        let bytes = sketch_zip(&json!({"pages":[{"_ref":"pages/../../evil.json"}]}), &[]);
        let error = import(&bytes, "bad.sketch").err().unwrap().to_string();
        assert!(error.contains("unsafe-path"));
    }

    #[test]
    fn reports_unknown_layer_types_instead_of_silently_keeping_them() {
        let page = json!({"_class":"page","do_objectID":"p","name":"P","layers":[{"_class":"symbolInstance","do_objectID":"sym","frame":{"x":0,"y":0,"width":20,"height":20}}]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/p.json"}]}),
            &[("pages/p.json", page)],
        );
        let imported = import(&bytes, "symbols.sketch").unwrap();
        assert_eq!(imported.pages[0]["layers"][0]["nodes"], json!([]));
        assert!(imported
            .report
            .items
            .iter()
            .any(|item| item.source_id == "sym" && item.status == FidelityStatus::Dropped));
    }

    #[test]
    fn enforces_caller_input_limit() {
        let bytes = sketch_zip(&json!({"pages":[]}), &[]);
        let limits = ImportLimits {
            max_input_bytes: 1,
            ..ImportLimits::default()
        };
        assert!(import_with_limits(&bytes, "large.sketch", limits)
            .err()
            .unwrap()
            .to_string()
            .contains("limit-exceeded"));
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_frames() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[{"_class":"rectangle","do_objectID":"r","frame":{"x":0,"y":0,"width":200000,"height":2}}]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/p.json"}]}),
            &[("pages/p.json", page)],
        );
        assert!(import(&bytes, "oversized.sketch")
            .err()
            .unwrap()
            .to_string()
            .contains("limit-exceeded"));
    }

    #[test]
    fn imports_cross_page_symbol_master_and_materialized_instance() {
        let document = json!({"pages":[{"_ref":"pages/master"},{"_ref":"pages/use"}]});
        let master = json!({"_class":"page","do_objectID":"master-page","name":"Master","layers":[
            {"_class":"symbolMaster","do_objectID":"master-object","symbolID":"symbol-1","name":"Badge","frame":{"x":0,"y":0,"width":20,"height":20},"layers":[
                {"_class":"rectangle","do_objectID":"master-rect","frame":{"x":0,"y":0,"width":20,"height":20}}
            ]}
        ]});
        let instance = json!({"_class":"page","do_objectID":"use-page","name":"Use","layers":[
            {"_class":"symbolInstance","do_objectID":"instance-1","symbolID":"symbol-1","frame":{"x":30,"y":10,"width":20,"height":20},
             "overrideValues":[{"overrideName":"title_stringValue","value":"New"}],"layers":[
                {"_class":"rectangle","do_objectID":"instance-rect","frame":{"x":0,"y":0,"width":20,"height":20}}
             ]}
        ]});
        let bytes = sketch_zip(
            &document,
            &[("pages/master.json", master), ("pages/use.json", instance)],
        );
        let imported = import(&bytes, "symbols.sketch").unwrap();
        assert_eq!(imported.components.len(), 1);
        assert_eq!(imported.components[0]["snapshot"]["kind"], "group");
        let node = &imported.pages[1]["layers"][0]["nodes"][0];
        assert_eq!(node["kind"], "instance");
        assert_eq!(node["component"], imported.components[0]["id"]);
        assert_eq!(node["fallback"]["children"][0]["kind"], "rect");
        assert!(imported.report.items.iter().any(
            |item| item.feature == "symbol_overrides" && item.status == FidelityStatus::Dropped
        ));
    }

    #[test]
    fn applies_unique_string_overrides_to_materialized_text_fallbacks() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"symbolMaster","do_objectID":"master-object","symbolID":"symbol-1","frame":{"x":0,"y":0,"width":40,"height":20},"layers":[
                {"_class":"text","do_objectID":"label","name":"Label","frame":{"x":0,"y":0,"width":40,"height":20},"attributedString":{"string":"Original","attributes":[]}}
            ]},
            {"_class":"symbolInstance","do_objectID":"instance-1","symbolID":"symbol-1","frame":{"x":50,"y":0,"width":40,"height":20},
             "overrideValues":[
                {"overrideName":"label_stringValue","value":"Updated"},
                {"overrideName":"label_color","value":"#ff0000"}
             ],
             "layers":[{"_class":"text","do_objectID":"label","name":"Label","frame":{"x":0,"y":0,"width":40,"height":20},"attributedString":{"string":"Original","attributes":[]}}]}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/overrides"}]}),
            &[("pages/overrides.json", page)],
        );
        let imported = import(&bytes, "overrides.sketch").unwrap();
        let instance = &imported.pages[0]["layers"][0]["nodes"][0];
        assert_eq!(instance["kind"], "instance");
        assert_eq!(instance["fallback"]["children"][0]["content"], "Updated");
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "instance-1"
                && item.feature == "symbol_text_overrides"
                && item.status == FidelityStatus::Approximated
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "instance-1"
                && item.feature == "symbol_overrides"
                && item.status == FidelityStatus::Dropped
        }));
    }

    #[test]
    fn applies_string_override_to_text_in_rekeyed_master_fallback() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"symbolMaster","do_objectID":"master-object","symbolID":"symbol-1","frame":{"x":0,"y":0,"width":40,"height":20},"layers":[
                {"_class":"text","do_objectID":"master-label","name":"Label","frame":{"x":0,"y":0,"width":40,"height":20},"attributedString":{"string":"Original","attributes":[]}}
            ]},
            {"_class":"symbolInstance","do_objectID":"instance-1","symbolID":"symbol-1","frame":{"x":50,"y":0,"width":40,"height":20},
             "overrideValues":[{"overrideName":"master-label_stringValue","value":"Updated"}]}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/master-fallback"}]}),
            &[("pages/master-fallback.json", page)],
        );
        let imported = import(&bytes, "master-fallback.sketch").unwrap();
        let instance = &imported.pages[0]["layers"][0]["nodes"][0];
        let text = &instance["fallback"]["children"][0];
        assert_eq!(instance["kind"], "instance");
        assert_eq!(text["content"], "Updated");
        assert_ne!(text["id"], "master-label");
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "instance-1"
                && item.feature == "symbol_text_overrides"
                && item.status == FidelityStatus::Approximated
        }));
        assert!(!imported
            .report
            .items
            .iter()
            .any(|item| { item.source_id == "instance-1" && item.feature == "symbol_overrides" }));
    }

    #[test]
    fn does_not_apply_ambiguous_override_in_rekeyed_master_fallback() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"symbolMaster","do_objectID":"master-object","symbolID":"symbol-1","frame":{"x":0,"y":0,"width":40,"height":40},"layers":[
                {"_class":"text","do_objectID":"duplicate-label","name":"First","frame":{"x":0,"y":0,"width":40,"height":20},"attributedString":{"string":"First","attributes":[]}},
                {"_class":"text","do_objectID":"duplicate-label","name":"Second","frame":{"x":0,"y":20,"width":40,"height":20},"attributedString":{"string":"Second","attributes":[]}}
            ]},
            {"_class":"symbolInstance","do_objectID":"instance-1","symbolID":"symbol-1","frame":{"x":50,"y":0,"width":40,"height":40},
             "overrideValues":[{"overrideName":"duplicate-label_stringValue","value":"Unsafe"}]}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/ambiguous-fallback"}]}),
            &[("pages/ambiguous-fallback.json", page)],
        );
        let imported = import(&bytes, "ambiguous-fallback.sketch").unwrap();
        let instance = &imported.pages[0]["layers"][0]["nodes"][0];
        let children = instance["fallback"]["children"].as_array().unwrap();
        assert_eq!(children[0]["content"], "First");
        assert_eq!(children[1]["content"], "Second");
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "instance-1"
                && item.feature == "symbol_overrides"
                && item.status == FidelityStatus::Dropped
        }));
        assert!(!imported.report.items.iter().any(|item| {
            item.source_id == "instance-1" && item.feature == "symbol_text_overrides"
        }));
    }

    #[test]
    fn empty_symbol_instance_uses_bounded_rekeyed_master_fallback() {
        let document = json!({"pages":[{"_ref":"pages/master"},{"_ref":"pages/use"}]});
        let master = json!({"_class":"page","do_objectID":"master-page","layers":[
            {"_class":"symbolMaster","do_objectID":"master-object","symbolID":"symbol-1","frame":{"x":0,"y":0,"width":20,"height":20},"layers":[
                {"_class":"rectangle","do_objectID":"master-rect","frame":{"x":0,"y":0,"width":20,"height":20}}
            ]}
        ]});
        let use_page = json!({"_class":"page","do_objectID":"use-page","layers":[
            {"_class":"symbolInstance","do_objectID":"instance-1","symbolID":"symbol-1","frame":{"x":30,"y":10,"width":20,"height":20}}
        ]});
        let bytes = sketch_zip(
            &document,
            &[("pages/master.json", master), ("pages/use.json", use_page)],
        );
        let imported = import(&bytes, "empty-symbol.sketch").unwrap();
        let instance = &imported.pages[1]["layers"][0]["nodes"][0];
        assert_eq!(instance["kind"], "instance");
        assert_eq!(instance["fallback"]["children"][0]["kind"], "rect");
        let snapshot_child_id = imported.components[0]["snapshot"]["children"][0]["id"]
            .as_str()
            .unwrap();
        let fallback_child_id = instance["fallback"]["children"][0]["id"].as_str().unwrap();
        assert_ne!(snapshot_child_id, fallback_child_id);
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "instance-1"
                && item.feature == "symbol_instance"
                && item.status == FidelityStatus::Approximated
        }));
    }

    #[test]
    fn missing_master_keeps_materialized_appearance_but_not_empty_instance() {
        let page = json!({"_class":"page","do_objectID":"p","name":"P","layers":[
            {"_class":"symbolInstance","do_objectID":"materialized","symbolID":"missing","frame":{"x":0,"y":0,"width":10,"height":10},"layers":[
                {"_class":"rectangle","do_objectID":"r","frame":{"x":0,"y":0,"width":10,"height":10}}
            ]},
            {"_class":"symbolInstance","do_objectID":"empty","symbolID":"missing","frame":{"x":20,"y":0,"width":10,"height":10}}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/p"}]}),
            &[("pages/p.json", page)],
        );
        let imported = import(&bytes, "missing.sketch").unwrap();
        let nodes = &imported.pages[0]["layers"][0]["nodes"];
        assert_eq!(nodes.as_array().unwrap().len(), 1);
        assert_eq!(nodes[0]["kind"], "group");
        assert!(imported
            .report
            .items
            .iter()
            .any(|item| item.source_id == "empty" && item.status == FidelityStatus::Dropped));
    }

    #[test]
    fn symbol_snapshot_obeys_hostile_node_limit() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"symbolMaster","do_objectID":"m","symbolID":"s","frame":{"x":0,"y":0,"width":10,"height":10},"layers":[
                {"_class":"rectangle","do_objectID":"r","frame":{"x":0,"y":0,"width":10,"height":10}}
            ]}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/p"}]}),
            &[("pages/p.json", page)],
        );
        let limits = ImportLimits {
            max_nodes: 1,
            ..ImportLimits::default()
        };
        assert!(import_with_limits(&bytes, "hostile.sketch", limits)
            .unwrap_err()
            .to_string()
            .contains("limit-exceeded"));
    }

    #[test]
    fn reports_unresolved_shared_style_references() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"rectangle","do_objectID":"r","sharedStyleID":"shared-style-1","frame":{"x":0,"y":0,"width":10,"height":10}}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/p"}]}),
            &[("pages/p.json", page)],
        );
        let imported = import(&bytes, "shared-style.sketch").unwrap();
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "r"
                && item.feature == "shared_style"
                && item.status == FidelityStatus::Dropped
        }));
    }

    #[test]
    fn resolves_shared_layer_and_text_styles_from_document_json() {
        let document = json!({
            "pages":[{"_ref":"pages/styles"}],
            "layerStyles":{"_class":"sharedStyleContainer","objects":[{
                "_class":"sharedStyle","do_objectID":"layer-style-1","name":"Primary card",
                "value":{"_class":"style",
                    "fills":[{"isEnabled":true,"fillType":0,"color":{"red":0.2,"green":0.4,"blue":0.6,"alpha":1},"contextSettings":{"opacity":0.5,"blendMode":1}}],
                    "borders":[{"isEnabled":true,"thickness":2,"color":{"red":1,"green":0,"blue":0,"alpha":1},"contextSettings":{"opacity":0.25}}],
                    "shadows":[{"isEnabled":true,"offsetX":2,"offsetY":3,"blur":4,"spread":0,"color":{"red":0,"green":0,"blue":0,"alpha":0.3}}],
                    "innerShadows":[{"isEnabled":true,"offsetX":-1,"offsetY":1,"blur":2,"spread":0,"color":{"red":1,"green":0,"blue":0,"alpha":0.5}}]
                }
            }]},
            "layerTextStyles":{"_class":"sharedStyleContainer","objects":[{
                "_class":"sharedStyle","do_objectID":"text-style-1","name":"Heading",
                "value":{"textStyle":{"paragraphStyle":{"alignment":2},"encodedAttributes":{
                    "MSAttributedStringFontAttribute":{"attributes":{"name":"Inter","size":24}},
                    "MSAttributedStringColorAttribute":{"red":0.1,"green":0.2,"blue":0.3,"alpha":1}
                }}}
            }]}
        });
        let page = json!({"_class":"page","do_objectID":"p","name":"Styles","layers":[
            {"_class":"rectangle","do_objectID":"card","sharedStyleID":"layer-style-1","frame":{"x":0,"y":0,"width":100,"height":60}},
            {"_class":"text","do_objectID":"heading","sharedStyleID":"text-style-1","frame":{"x":0,"y":70,"width":100,"height":30},"attributedString":{"string":"Title","attributes":[]}}
        ]});
        let bytes = sketch_zip(&document, &[("pages/styles.json", page)]);
        let imported = import(&bytes, "shared-styles.sketch").unwrap();
        let nodes = &imported.pages[0]["layers"][0]["nodes"];
        assert_eq!(nodes[0]["style"]["fill"]["fallback"], "#33669980");
        assert_eq!(nodes[0]["style"]["stroke"]["fallback"], "#ff000040");
        assert_eq!(nodes[0]["style"]["stroke_width"]["fallback"], 2.0);
        assert_eq!(nodes[0]["effects"][0]["kind"], "drop-shadow");
        assert_eq!(nodes[0]["effects"][0]["params"]["x"], 2.0);
        assert_eq!(nodes[0]["effects"][0]["params"]["y"], 3.0);
        assert_eq!(nodes[0]["effects"][0]["params"]["blur"], 4.0);
        assert_eq!(nodes[0]["effects"][0]["params"]["color"], "#0000004d");
        assert_eq!(nodes[0]["effects"][1]["kind"], "inner-shadow");
        assert_eq!(nodes[0]["effects"][1]["params"]["x"], -1.0);
        assert_eq!(nodes[0]["effects"][1]["params"]["color"], "#ff000080");
        assert_eq!(nodes[1]["font_family"], "Inter");
        assert_eq!(nodes[1]["font_size"], 24.0);
        assert_eq!(nodes[1]["align"], "center");
        assert_eq!(nodes[1]["style"]["fill"]["fallback"], "#1a334d");
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "card"
                && item.feature == "shared_style"
                && item.status == FidelityStatus::Approximated
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "card"
                && item.feature == "shadow"
                && item.status == FidelityStatus::Approximated
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "card"
                && item.feature == "inner_shadow"
                && item.status == FidelityStatus::Approximated
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "card"
                && item.feature == "fill_blend_mode"
                && item.status == FidelityStatus::Dropped
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "heading"
                && item.feature == "shared_text_alignment"
                && item.status == FidelityStatus::Approximated
        }));
    }

    #[test]
    fn imports_supported_local_shadows_and_reports_unsupported_parameters() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"rectangle","do_objectID":"r","frame":{"x":10,"y":10,"width":20,"height":20},"style":{"shadows":[
                {"isEnabled":false,"blur":3,"color":{"red":0,"green":0,"blue":0}},
                {"isEnabled":true,"offsetX":-2,"offsetY":1,"blur":5,"color":{"red":1,"green":0,"blue":0,"alpha":0.5}},
                {"isEnabled":true,"spread":2,"blur":4,"color":{"red":0,"green":0,"blue":0}},
                {"isEnabled":true,"offsetX":"bad","blur":4,"color":{"red":0,"green":0,"blue":0}}
            ]}}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/shadows"}]}),
            &[("pages/shadows.json", page)],
        );
        let imported = import(&bytes, "shadows.sketch").unwrap();
        let node = &imported.pages[0]["layers"][0]["nodes"][0];
        assert_eq!(node["effects"].as_array().unwrap().len(), 1);
        assert_eq!(node["effects"][0]["params"]["color"], "#ff000080");
        assert!(imported
            .report
            .items
            .iter()
            .any(|item| item.source_id == "r"
                && item.feature == "shadow"
                && item.status == FidelityStatus::Dropped));
    }

    #[test]
    fn imports_supported_local_inner_shadows_and_reports_invalid_variants() {
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"rectangle","do_objectID":"r","frame":{"x":0,"y":0,"width":20,"height":20},"style":{"innerShadows":[
                {"isEnabled":false,"blur":2},
                {"isEnabled":true,"offsetX":2,"offsetY":-3,"blur":6,"spread":0,"color":{"red":0.2,"green":0.3,"blue":0.4,"alpha":0.5},"contextSettings":{"opacity":0.5,"blendMode":0}},
                {"isEnabled":true,"spread":1,"blur":2,"color":{"red":0,"green":0,"blue":0}},
                {"isEnabled":true,"blur":2,"color":{"red":2,"green":0,"blue":0}},
                {"isEnabled":true,"blur":2,"color":{"red":0,"green":0,"blue":0},"contextSettings":{"blendMode":1}},
                {"isEnabled":true,"blur":300,"color":{"red":0,"green":0,"blue":0}}
            ]}}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/inner-shadows"}]}),
            &[("pages/inner-shadows.json", page)],
        );
        let imported = import(&bytes, "inner-shadows.sketch").unwrap();
        let node = &imported.pages[0]["layers"][0]["nodes"][0];
        assert_eq!(node["effects"].as_array().unwrap().len(), 1);
        assert_eq!(node["effects"][0]["kind"], "inner-shadow");
        assert_eq!(node["effects"][0]["params"]["x"], 2.0);
        assert_eq!(node["effects"][0]["params"]["y"], -3.0);
        assert_eq!(node["effects"][0]["params"]["blur"], 6.0);
        assert_eq!(node["effects"][0]["params"]["color"], "#334d6640");
        assert_eq!(node["effects"][0]["id"], "sketch-r-inner_shadow-1");
        assert_eq!(
            imported
                .report
                .items
                .iter()
                .filter(|item| item.source_id == "r"
                    && item.feature == "inner_shadow"
                    && item.status == FidelityStatus::Dropped)
                .count(),
            4
        );
    }

    #[test]
    fn reports_unsupported_shared_paragraph_alignment_and_spacing() {
        let document = json!({
            "pages":[{"_ref":"pages/paragraph"}],
            "layerTextStyles":{"_class":"sharedStyleContainer","objects":[{
                "_class":"sharedStyle","do_objectID":"text-style",
                "value":{"textStyle":{"paragraphStyle":{"alignment":3,"paragraphSpacing":4}}}
            }]}
        });
        let page = json!({"_class":"page","do_objectID":"p","layers":[
            {"_class":"text","do_objectID":"copy","sharedStyleID":"text-style","frame":{"x":0,"y":0,"width":100,"height":30},"attributedString":{"string":"Centered?","attributes":[]}}
        ]});
        let bytes = sketch_zip(&document, &[("pages/paragraph.json", page)]);
        let imported = import(&bytes, "paragraph.sketch").unwrap();
        let node = &imported.pages[0]["layers"][0]["nodes"][0];
        assert!(node.get("align").is_none());
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "copy"
                && item.feature == "shared_text_alignment"
                && item.status == FidelityStatus::Dropped
        }));
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "copy"
                && item.feature == "shared_text_paragraph"
                && item.status == FidelityStatus::Dropped
        }));
    }

    #[test]
    fn negative_page_coordinates_expand_and_shift_the_canvas() {
        let page = json!({"_class":"page","do_objectID":"negative-page","layers":[
            {"_class":"rectangle","do_objectID":"negative-rect","frame":{"x":-20,"y":-10,"width":30,"height":20}}
        ]});
        let bytes = sketch_zip(
            &json!({"pages":[{"_ref":"pages/negative"}]}),
            &[("pages/negative.json", page)],
        );
        let imported = import(&bytes, "negative.sketch").unwrap();
        let page = &imported.pages[0];
        assert_eq!(page["canvas"]["width"], 30);
        assert_eq!(page["canvas"]["height"], 20);
        let bounds_group = &page["layers"][0]["nodes"][0];
        assert_eq!(bounds_group["transform"][4], 20.0);
        assert_eq!(bounds_group["transform"][5], 10.0);
        assert_eq!(bounds_group["children"][0]["x"], -20.0);
        assert_eq!(bounds_group["children"][0]["y"], -10.0);
    }
}

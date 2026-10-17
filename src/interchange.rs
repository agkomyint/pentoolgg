//! Shared, bounded design-interchange primitives for v0.17.
//!
//! Format-specific readers map into ordinary Pentool document nodes and use
//! this module for the common fidelity report and hostile-input limits.

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek};
use std::path::Path;
use zip::ZipArchive;

pub const FIDELITY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FidelityStatus {
    Exact,
    Approximated,
    Dropped,
}

#[derive(Clone, Debug, Serialize)]
pub struct FidelityItem {
    /// Stable identifier in the source format. Use `$document` for document-
    /// level facts that do not belong to an individual source object.
    pub source_id: String,
    pub feature: String,
    pub status: FidelityStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FidelityCounts {
    pub exact: usize,
    pub approximated: usize,
    pub dropped: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct FidelityReport {
    pub schema_version: u32,
    pub format: String,
    pub source: String,
    pub counts: FidelityCounts,
    pub items: Vec<FidelityItem>,
    pub items_omitted: usize,
}

impl FidelityReport {
    /// Add a deterministic detail to an existing report, respecting the same
    /// bounded detail policy used during source parsing.
    pub fn record(
        &mut self,
        source_id: impl Into<String>,
        feature: impl Into<String>,
        status: FidelityStatus,
        message: impl Into<String>,
        action: Option<String>,
    ) {
        match status {
            FidelityStatus::Exact => self.counts.exact += 1,
            FidelityStatus::Approximated => self.counts.approximated += 1,
            FidelityStatus::Dropped => self.counts.dropped += 1,
        }
        if self.items.len() >= 5_000 {
            self.items_omitted += 1;
            return;
        }
        let source_id = bounded_report_text(source_id.into(), 1024, true);
        self.items.push(FidelityItem {
            source_id: if source_id.trim().is_empty() {
                "$document".into()
            } else {
                source_id
            },
            feature: bounded_report_text(feature.into(), 128, false),
            status,
            message: bounded_report_text(message.into(), 4096, false),
            action: action.map(|action| bounded_report_text(action, 4096, false)),
        });
    }
}

/// Keeps report memory and JSON output bounded while retaining complete counts.
pub struct FidelityReportBuilder {
    report: FidelityReport,
    max_items: usize,
}

impl FidelityReportBuilder {
    pub fn new(format: impl Into<String>, source: impl Into<String>) -> Self {
        Self::with_item_limit(format, source, 5_000)
    }

    pub fn with_item_limit(
        format: impl Into<String>,
        source: impl Into<String>,
        max_items: usize,
    ) -> Self {
        Self {
            report: FidelityReport {
                schema_version: FIDELITY_SCHEMA_VERSION,
                format: bounded_report_text(format.into(), 64, false),
                source: bounded_report_text(source.into(), 4096, false),
                counts: FidelityCounts::default(),
                items: Vec::new(),
                items_omitted: 0,
            },
            max_items,
        }
    }

    pub fn record(
        &mut self,
        source_id: impl Into<String>,
        feature: impl Into<String>,
        status: FidelityStatus,
        message: impl Into<String>,
        action: Option<String>,
    ) {
        if self.report.items.len() >= self.max_items {
            match status {
                FidelityStatus::Exact => self.report.counts.exact += 1,
                FidelityStatus::Approximated => self.report.counts.approximated += 1,
                FidelityStatus::Dropped => self.report.counts.dropped += 1,
            }
            self.report.items_omitted += 1;
        } else {
            self.report
                .record(source_id, feature, status, message, action);
        }
    }

    pub fn finish(self) -> FidelityReport {
        self.report
    }
}

fn bounded_report_text(value: String, max_bytes: usize, hash_suffix: bool) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let suffix = if hash_suffix {
        format!("…{}", &hex::encode(Sha256::digest(value.as_bytes()))[..16])
    } else {
        "…".to_owned()
    };
    let prefix_budget = max_bytes.saturating_sub(suffix.len());
    let mut end = prefix_budget.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &value[..end])
}

/// Resource ceilings shared by archive-based and XML/vector importers.
#[derive(Clone, Copy, Debug)]
pub struct ImportLimits {
    pub max_input_bytes: u64,
    pub max_archive_entries: usize,
    pub max_uncompressed_bytes: u64,
    pub max_compression_ratio: u64,
    pub max_nodes: usize,
    pub max_nesting: usize,
    pub max_image_bytes: u64,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 512 * 1024 * 1024,
            max_archive_entries: 20_000,
            max_uncompressed_bytes: 2 * 1024 * 1024 * 1024,
            max_compression_ratio: 100,
            max_nodes: 1_000_000,
            max_nesting: 128,
            max_image_bytes: 64 * 1024 * 1024,
        }
    }
}

impl ImportLimits {
    pub fn check_input(&self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() as u64 <= self.max_input_bytes,
            "[limit-exceeded] import input is {} bytes; the limit is {}",
            bytes.len(),
            self.max_input_bytes
        );
        Ok(())
    }

    pub fn check_node_count(&self, count: usize) -> Result<()> {
        ensure!(
            count <= self.max_nodes,
            "[limit-exceeded] import contains {count} nodes; the limit is {}",
            self.max_nodes
        );
        Ok(())
    }

    pub fn check_nesting(&self, depth: usize) -> Result<()> {
        ensure!(
            depth <= self.max_nesting,
            "[limit-exceeded] import nesting is {depth}; the limit is {}",
            self.max_nesting
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveEntryInfo {
    pub name: String,
    pub uncompressed_bytes: u64,
    pub compressed_bytes: u64,
    pub is_dir: bool,
}

/// Validates ZIP metadata before callers read or decompress any entry.
pub fn inspect_archive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    limits: ImportLimits,
) -> Result<Vec<ArchiveEntryInfo>> {
    ensure!(
        archive.len() <= limits.max_archive_entries,
        "[limit-exceeded] archive has {} entries; the limit is {}",
        archive.len(),
        limits.max_archive_entries
    );
    let mut total = 0u64;
    let mut entries = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name().replace('\\', "/");
        let path = std::path::Path::new(&name);
        ensure!(
            !name.starts_with('/')
                && !name.starts_with("//")
                && !name.as_bytes().get(1).is_some_and(|b| *b == b':')
                && path.components().all(|part| {
                    !matches!(
                        part,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                }),
            "[unsafe-path] archive entry {name:?} is not a safe relative path"
        );
        let size = entry.size();
        let compressed = entry.compressed_size();
        total = total
            .checked_add(size)
            .ok_or_else(|| anyhow::anyhow!("[limit-exceeded] archive size overflow"))?;
        ensure!(
            total <= limits.max_uncompressed_bytes,
            "[limit-exceeded] archive expands beyond {} bytes",
            limits.max_uncompressed_bytes
        );
        if size > 0 {
            let maximum_expansion = compressed.saturating_mul(limits.max_compression_ratio);
            ensure!(
                compressed > 0 && size <= maximum_expansion,
                "[limit-exceeded] archive entry {name:?} exceeds the compression-ratio limit"
            );
        }
        entries.push(ArchiveEntryInfo {
            is_dir: entry.is_dir(),
            name,
            uncompressed_bytes: size,
            compressed_bytes: compressed,
        });
    }
    Ok(entries)
}

/// Validate an entry's declared image size before allocating its decoded pixels.
pub fn check_image_bytes(bytes: u64, limits: ImportLimits) -> Result<()> {
    ensure!(
        bytes <= limits.max_image_bytes,
        "[limit-exceeded] imported image is {bytes} bytes; the limit is {}",
        limits.max_image_bytes
    );
    Ok(())
}

/// Avoid accepting a zero-valued limit configuration that would mask mistakes.
pub fn validate_limits(limits: ImportLimits) -> Result<()> {
    if limits.max_input_bytes == 0
        || limits.max_archive_entries == 0
        || limits.max_uncompressed_bytes == 0
        || limits.max_compression_ratio == 0
        || limits.max_nodes == 0
        || limits.max_nesting == 0
        || limits.max_image_bytes == 0
    {
        bail!("[invalid-input] import limits must all be greater than zero");
    }
    Ok(())
}

/// Build a bounded export fidelity report from the source document. Vector
/// primitives map directly to SVG/PDF; text and raster-backed constructs are
/// identified as approximations, and PNG necessarily flattens scene structure.
pub fn export_fidelity_report(
    document: &Value,
    format: &str,
    source: &str,
) -> Result<FidelityReport> {
    export_fidelity_report_for(document, format, source, None, false)
}

/// Build a report for the pages and representation that the exporter will
/// actually write. `page_ids` filters the source scope; `flattened` describes
/// output paths such as PNG or compositing-backed SVG/PDF that rasterize the
/// entire page rather than retaining object-level vector content.
pub fn export_fidelity_report_for(
    document: &Value,
    format: &str,
    source: &str,
    page_ids: Option<&[String]>,
    flattened: bool,
) -> Result<FidelityReport> {
    let mut report = FidelityReportBuilder::new(format, source);
    // Legacy v1/v2 documents keep their single page's layers at the document
    // root. Treat that representation as the implicit page the renderer sees.
    let legacy_layers = document
        .get("layers")
        .and_then(Value::as_array)
        .filter(|_| document.get("pages").and_then(Value::as_array).is_none());
    let pages = document.get("pages").and_then(Value::as_array);
    if pages.is_none() && legacy_layers.is_none() {
        anyhow::bail!("export source has no pages or legacy layers");
    }
    let mut record_page = |page_id: &str, layers: &[Value]| -> Result<()> {
        if page_ids.is_some_and(|ids| !ids.iter().any(|id| id == page_id)) {
            return Ok(());
        }
        report.record(
            page_id,
            "page",
            FidelityStatus::Exact,
            "page dimensions and export order retained",
            None,
        );
        for layer in layers {
            let layer_visible = layer.get("visible").and_then(Value::as_bool) != Some(false);
            let nodes = layer
                .get("nodes")
                .or_else(|| layer.get("paths"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            for node in nodes {
                record_export_node(node, format, flattened, layer_visible, &mut report)?;
            }
            for text in layer
                .get("texts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                record_export_node(text, format, flattened, layer_visible, &mut report)?;
            }
        }
        Ok(())
    };
    if let Some(pages) = pages {
        for page in pages {
            let page_id = page.get("id").and_then(Value::as_str).unwrap_or("page");
            let layers = page
                .get("layers")
                .and_then(Value::as_array)
                .context("export page has no layers")?;
            record_page(page_id, layers)?;
        }
    } else if let Some(layers) = legacy_layers {
        record_page("page-1", layers)?;
    }
    Ok(report.finish())
}

fn record_export_node(
    node: &Value,
    format: &str,
    flattened: bool,
    inherited_visible: bool,
    report: &mut FidelityReportBuilder,
) -> Result<()> {
    let id = node
        .get("id")
        .and_then(Value::as_str)
        .context("export node has no stable ID")?;
    let kind = node
        .get("kind")
        .or_else(|| node.get("type"))
        .and_then(Value::as_str)
        .or_else(|| {
            if node.get("d").is_some() {
                Some("path")
            } else if node.get("content").is_some() {
                Some("text")
            } else if node.get("x1").is_some() {
                Some("line")
            } else {
                None
            }
        })
        .unwrap_or("unknown");
    let id = id.to_owned();
    let visible = inherited_visible && node.get("visible").and_then(Value::as_bool) != Some(false);
    if !visible {
        report.record(
            &id,
            kind,
            FidelityStatus::Dropped,
            "object is hidden and therefore omitted from the rendered export",
            Some("make the object and its parent layers visible before export".into()),
        );
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                record_export_node(child, format, flattened, false, report)?;
            }
        }
        return Ok(());
    }
    if node.get("clip").is_some() {
        report.record(
            &id,
            "clip",
            FidelityStatus::Dropped,
            "general clipping is not represented by the current export path",
            Some("remove or flatten the clip before export".into()),
        );
    }
    if kind == "group" {
        report.record(
            &id,
            "group",
            if format == "png" || flattened {
                FidelityStatus::Approximated
            } else {
                FidelityStatus::Exact
            },
            if format == "png" || flattened {
                "group hierarchy and object editability flattened into page pixels"
            } else {
                "group hierarchy retained as an SVG/PDF container"
            },
            None,
        );
        for child in node
            .get("children")
            .and_then(Value::as_array)
            .context("group children are missing")?
        {
            record_export_node(child, format, flattened, true, report)?;
        }
        return Ok(());
    }
    let (status, message, action) = if format == "png" || flattened {
        (
            FidelityStatus::Approximated,
            "scene object rendered into page pixels; editability and vector geometry are not retained",
            Some("keep the source .pen document for later edits".into()),
        )
    } else {
        match kind {
            "path" | "rect" | "ellipse" | "line" => (
                FidelityStatus::Exact,
                "vector geometry and supported paint exported",
                None,
            ),
            "text" => (
                FidelityStatus::Approximated,
                "text content exported; font metrics or embedding may differ in the target viewer",
                Some("outline text or verify target font availability".into()),
            ),
            "instance" => (
                FidelityStatus::Approximated,
                "component instance exported from its materialized fallback snapshot",
                Some("retain the source .pen document to preserve component links".into()),
            ),
            "image" | "raster" | "photo" | "scene3d" => (
                FidelityStatus::Approximated,
                "raster-backed or 3D content is embedded as rendered pixels",
                Some("retain the source .pen document to preserve editable source data".into()),
            ),
            _ => (
                FidelityStatus::Dropped,
                "node kind is not represented by the selected vector export subset",
                Some("flatten or convert this object before exporting".into()),
            ),
        }
    };
    report.record(&id, kind, status, message, action);
    Ok(())
}

/// Format importer output with the original source context alongside the
/// ordinary editable scene document.
#[derive(Debug)]
pub struct DesignImport {
    pub document: Value,
    pub report: FidelityReport,
}

/// Detect only unambiguous format extensions. Figma JSON remains explicit
/// because ordinary `.json` Pentool documents must keep their current meaning.
pub fn detect_format(path: &Path) -> Option<&'static str> {
    let file_name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if file_name.ends_with(".figma.json") {
        return Some("figma");
    }
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "svg" => Some("svg"),
        "pdf" | "ai" => Some("pdf"),
        "psd" => Some("psd"),
        "fig" => Some("figma"),
        "sketch" => Some("sketch"),
        _ => None,
    }
}

/// Import one supported design source into a standalone `.pen` scene. The
/// caller decides whether to save it as a new document or append it to an
/// existing document transactionally.
pub fn import_design(format: &str, bytes: &[u8], source: &str) -> Result<DesignImport> {
    let limits = ImportLimits::default();
    limits.check_input(bytes)?;
    match format {
        "svg" => {
            let imported = crate::svg_import::import_svg(bytes, source, limits)?;
            Ok(DesignImport {
                document: imported.document,
                report: imported.report,
            })
        }
        "sketch" => {
            let imported = crate::sketch_import::import(bytes, source)?;
            let name = Path::new(source)
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Imported Sketch");
            let mut document = crate::scene::new_document(1, 1);
            document["name"] = json!(name);
            document["pages"] = Value::Array(imported.pages);
            document["components"] = Value::Array(imported.components);
            crate::scene::validate(&document).map_err(anyhow::Error::msg)?;
            Ok(DesignImport {
                document,
                report: imported.report,
            })
        }
        "figma" => {
            let imported = crate::figma_import::import(bytes, source)?;
            let name = Path::new(source)
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or("Imported Figma");
            let mut document = crate::scene::new_document(1, 1);
            document["name"] = json!(name);
            document["pages"] = Value::Array(imported.pages);
            document["components"] = Value::Array(imported.components);
            crate::scene::validate(&document).map_err(anyhow::Error::msg)?;
            Ok(DesignImport {
                document,
                report: imported.report,
            })
        }
        "pdf" => {
            let imported = crate::pdf_import::import(bytes, source, limits)?;
            Ok(DesignImport {
                document: imported.document,
                report: imported.report,
            })
        }
        "psd" => {
            let imported = crate::psd_import::import(bytes, source)?;
            Ok(DesignImport {
                document: imported.document,
                report: imported.report,
            })
        }
        other => bail!(
            "[invalid-input] unsupported design import format {other:?}; choose svg, pdf, psd, figma, or sketch"
        ),
    }
}

/// Add a standalone format-import result to one selected page as a dedicated
/// layer. Unknown node fields survive; imported node IDs are deterministically
/// namespaced and collisions fail before the caller writes the document.
pub fn append_imported_document(
    destination: &Value,
    imported: &Value,
    destination_page: Option<&str>,
    prefix: &str,
    report: &mut FidelityReport,
) -> Result<(Value, Value)> {
    ensure!(
        crate::scene::is_scene_document(destination),
        "[unsupported-version] design-format import requires a v4+ destination; migrate it first"
    );
    crate::scene::validate(destination).map_err(anyhow::Error::msg)?;
    ensure!(
        crate::scene::is_scene_document(imported),
        "[invalid-input] importer did not produce a scene document"
    );
    crate::scene::validate(imported).map_err(anyhow::Error::msg)?;
    let prefix = safe_id_part(prefix);
    ensure!(
        !prefix.is_empty(),
        "[invalid-input] import prefix must contain letters or digits"
    );
    let mut result = destination.clone();
    let imported_assets = imported
        .get("image_assets")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if !imported_assets.is_empty()
        && result.get("version").and_then(Value::as_u64) == Some(crate::scene::VERSION)
    {
        result = crate::scene::migrate_to_v5(result)?;
    }
    if imported.get("version").and_then(Value::as_u64) == Some(crate::scene::PAINT_VERSION)
        && result.get("version").and_then(Value::as_u64).unwrap_or(0) < crate::scene::PAINT_VERSION
    {
        result = crate::scene::migrate_to_v12(result)?;
    }
    let existing_pages = result
        .get("pages")
        .and_then(Value::as_array)
        .context("destination has no pages")?;
    let selected_id = destination_page
        .or_else(|| existing_pages.first()?.get("id")?.as_str())
        .context("destination has no selectable page")?
        .to_owned();
    let target_index = existing_pages
        .iter()
        .position(|page| page.get("id").and_then(Value::as_str) == Some(&selected_id))
        .with_context(|| format!("page not found: {selected_id}"))?;
    let existing_ids = document_node_ids(&result);
    let layer_id = format!("{prefix}-import");
    let layers = result["pages"][target_index]
        .get("layers")
        .and_then(Value::as_array)
        .context("selected destination page has no layers")?;
    ensure!(
        !layers
            .iter()
            .any(|layer| layer.get("id").and_then(Value::as_str) == Some(&layer_id)),
        "[invalid-input] import layer ID {layer_id:?} already exists; choose a distinct --prefix"
    );
    let pages = imported
        .get("pages")
        .and_then(Value::as_array)
        .context("imported document has no pages")?;
    ensure!(
        !pages.is_empty(),
        "[invalid-input] imported document has no pages"
    );

    let mut imported_ids = existing_ids;
    let mut node_count = 0usize;
    let component_sources = imported
        .get("components")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let existing_components = result
        .get("components")
        .and_then(Value::as_array)
        .context("destination components must be an array")?;
    let mut component_ids = std::collections::HashMap::new();
    let mut component_records = Vec::with_capacity(component_sources.len());
    let mut used_component_ids = existing_components
        .iter()
        .filter_map(|component| component.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<std::collections::HashSet<_>>();
    for component in &component_sources {
        let source_id = component
            .get("id")
            .and_then(Value::as_str)
            .context("imported component has no ID")?;
        let id = generated_id(&prefix, &format!("component:{source_id}"));
        ensure!(
            used_component_ids.insert(id.clone()),
            "[invalid-input] imported component ID collides after namespacing; choose a distinct --prefix"
        );
        component_ids.insert(source_id.to_owned(), id.clone());
        let mut record = component.clone();
        record["id"] = json!(id);
        component_records.push(record);
    }
    let mut component_node_ids = imported_ids.clone();
    for record in &mut component_records {
        let snapshot = record
            .get("snapshot")
            .cloned()
            .context("imported component has no snapshot")?;
        let mut snapshot = namespace_node(
            snapshot,
            &format!("{prefix}-component"),
            &mut component_node_ids,
            &mut node_count,
        )?;
        remap_component_refs(&mut snapshot, &component_ids);
        record["snapshot"] = snapshot;
    }
    imported_ids.extend(component_node_ids);
    let mut imported_nodes = Vec::new();
    for (page_index, page) in pages.iter().enumerate() {
        let page_name = page
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Imported page");
        let page_layers = page
            .get("layers")
            .and_then(Value::as_array)
            .context("imported page has no layers")?;
        let source_page_id = page.get("id").and_then(Value::as_str).unwrap_or("page");
        let source_page_key = format!("{source_page_id}:{page_index}");
        let mut imported_layers = Vec::new();
        for (layer_index, layer) in page_layers.iter().enumerate() {
            let source_layer_id = layer.get("id").and_then(Value::as_str).unwrap_or("layer");
            let group_id = generated_id(
                &prefix,
                &format!("{source_page_key}:{source_layer_id}:{layer_index}"),
            );
            ensure!(
                imported_ids.insert(group_id.clone()),
                "[invalid-input] imported layer ID collides after namespacing; choose a distinct --prefix"
            );
            let visible = layer
                .get("visible")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let locked = layer
                .get("locked")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if locked {
                report.record(
                    source_layer_id,
                    "layer_lock",
                    FidelityStatus::Approximated,
                    "source layer lock is retained as metadata but is not enforced after import",
                    Some(
                        "lock the imported layer in Pentool if editing protection is required"
                            .into(),
                    ),
                );
            }
            let mut imported_layer = json!({
                "kind":"group", "id":group_id,
                "name":layer.get("name").and_then(Value::as_str).unwrap_or("Imported layer"),
                "visible":visible,
                "import_source":{"layer_id":source_layer_id,"locked":locked},
                "transform":[1,0,0,1,0,0], "children":[]
            });
            for node in layer
                .get("nodes")
                .and_then(Value::as_array)
                .context("imported layer has no node array")?
            {
                let mut imported_node =
                    namespace_node(node.clone(), &prefix, &mut imported_ids, &mut node_count)?;
                remap_component_refs(&mut imported_node, &component_ids);
                imported_layer["children"]
                    .as_array_mut()
                    .context("imported layer group has no children")?
                    .push(imported_node);
            }
            node_count += 1;
            report.record(
                source_layer_id,
                "layer_structure",
                FidelityStatus::Exact,
                "source layer order and visibility preserved as an ordered group",
                None,
            );
            imported_layers.push(imported_layer);
        }
        if pages.len() == 1 {
            imported_nodes.extend(imported_layers);
        } else {
            let group_id = generated_id(&prefix, &source_page_key);
            ensure!(
                imported_ids.insert(group_id.clone()),
                "[invalid-input] imported page ID collides after namespacing; choose a distinct --prefix"
            );
            imported_nodes.push(json!({
                "kind":"group", "id":group_id, "name":page_name,
                "transform":[1,0,0,1,0,0], "children":imported_layers
            }));
            node_count += 1;
        }
    }

    crate::scene::apply_layer(
        &mut result,
        Some(&selected_id),
        crate::editing::LayerAction::Add {
            id: layer_id.clone(),
            name: Some(format!(
                "Imported {}",
                imported
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("design")
            )),
        },
    )?;
    let target = result["pages"][target_index]
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("selected destination page has no layers")?
        .iter_mut()
        .find(|layer| layer.get("id").and_then(Value::as_str) == Some(&layer_id))
        .context("new import layer was not created")?;
    target["nodes"] = Value::Array(imported_nodes);
    result["components"]
        .as_array_mut()
        .context("destination components must be an array")?
        .extend(component_records);
    if !imported_assets.is_empty() {
        let destination_assets = result
            .get_mut("image_assets")
            .and_then(Value::as_object_mut)
            .context("image-capable destination has no image_assets object")?;
        for (asset_id, asset) in imported_assets {
            if let Some(existing) = destination_assets.get(&asset_id) {
                ensure!(
                    existing == &asset,
                    "[invalid-input] imported image asset {asset_id:?} conflicts with destination content"
                );
            } else {
                destination_assets.insert(asset_id, asset);
            }
        }
    }
    crate::scene::validate(&result).map_err(anyhow::Error::msg)?;
    Ok((
        result,
        json!({"page":selected_id,"layer":layer_id,"prefix":prefix,"nodes_imported":node_count}),
    ))
}

fn safe_id_part(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(32)
        .collect()
}

fn generated_id(prefix: &str, source_id: &str) -> String {
    let slug = safe_id_part(source_id);
    let digest = Sha256::digest(source_id.as_bytes());
    let suffix = digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if slug.is_empty() {
        format!("{prefix}-node-{suffix}")
    } else {
        format!("{prefix}-{slug}-{suffix}")
    }
}

fn document_node_ids(document: &Value) -> std::collections::HashSet<String> {
    fn walk(nodes: &[Value], ids: &mut std::collections::HashSet<String>) {
        for node in nodes {
            if let Some(id) = node.get("id").and_then(Value::as_str) {
                ids.insert(id.to_owned());
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                walk(children, ids);
            }
        }
    }
    let mut ids = std::collections::HashSet::new();
    if let Some(pages) = document.get("pages").and_then(Value::as_array) {
        for page in pages {
            if let Some(layers) = page.get("layers").and_then(Value::as_array) {
                for layer in layers {
                    if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
                        walk(nodes, &mut ids);
                    }
                }
            }
        }
    }
    ids
}

fn namespace_node(
    mut node: Value,
    prefix: &str,
    used: &mut std::collections::HashSet<String>,
    node_count: &mut usize,
) -> Result<Value> {
    let source_id = node
        .get("id")
        .and_then(Value::as_str)
        .context("imported node has no ID")?
        .to_owned();
    let id = generated_id(prefix, &source_id);
    ensure!(
        used.insert(id.clone()),
        "[invalid-input] imported node ID {source_id:?} collides after namespacing; choose a distinct --prefix"
    );
    node["id"] = json!(id);
    *node_count = (*node_count).saturating_add(1);
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children.iter_mut() {
            *child = namespace_node(child.clone(), prefix, used, node_count)?;
        }
    }
    if let Some(fallback) = node.get_mut("fallback") {
        *fallback = namespace_node(
            fallback.clone(),
            &format!("{prefix}-fallback"),
            used,
            node_count,
        )?;
    }
    Ok(node)
}

fn remap_component_refs(node: &mut Value, mapping: &std::collections::HashMap<String, String>) {
    if node.get("kind").and_then(Value::as_str) == Some("instance") {
        if let Some(source_id) = node.get("component").and_then(Value::as_str) {
            if let Some(id) = mapping.get(source_id) {
                node["component"] = json!(id);
            }
        }
    }
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            remap_component_refs(child, mapping);
        }
    }
    if let Some(fallback) = node.get_mut("fallback") {
        remap_component_refs(fallback, mapping);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn fidelity_report_counts_everything_but_caps_details() {
        let mut report = FidelityReportBuilder::with_item_limit("svg", "icons.svg", 1);
        report.record("shape-1", "geometry", FidelityStatus::Exact, "mapped", None);
        report.record(
            "shape-2",
            "filter",
            FidelityStatus::Dropped,
            "filter unsupported",
            Some("rasterize before import".into()),
        );
        let report = report.finish();
        assert_eq!(report.counts.exact, 1);
        assert_eq!(report.counts.dropped, 1);
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items_omitted, 1);
        assert_eq!(report.items[0].source_id, "shape-1");
    }

    #[test]
    fn hostile_archive_paths_and_expansion_are_rejected_from_metadata() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file("../escape", zip::write::SimpleFileOptions::default())
                .unwrap();
            use std::io::Write;
            writer.write_all(b"x").unwrap();
            writer.finish().unwrap();
        }
        let mut archive = ZipArchive::new(Cursor::new(bytes.into_inner())).unwrap();
        assert!(inspect_archive(&mut archive, ImportLimits::default())
            .unwrap_err()
            .to_string()
            .contains("unsafe-path"));
    }

    #[test]
    fn input_and_tree_limits_are_explicit() {
        let limits = ImportLimits {
            max_input_bytes: 1,
            max_nodes: 1,
            max_nesting: 2,
            ..ImportLimits::default()
        };
        assert!(limits.check_input(b"12").is_err());
        assert!(limits.check_node_count(2).is_err());
        assert!(limits.check_nesting(3).is_err());
    }

    #[test]
    fn fidelity_reports_bound_all_source_controlled_text() {
        let mut builder = FidelityReportBuilder::new("f".repeat(100), "s".repeat(10_000));
        builder.record(
            "i".repeat(10_000),
            "feature".repeat(100),
            FidelityStatus::Dropped,
            "message".repeat(1000),
            Some("action".repeat(1000)),
        );
        let report = builder.finish();
        assert!(report.format.len() <= 64);
        assert!(report.source.len() <= 4096);
        assert!(report.items[0].source_id.len() <= 1024);
        assert!(report.items[0].source_id.contains('…'));
        assert!(report.items[0].feature.len() <= 128);
        assert!(report.items[0].message.len() <= 4096);
        assert!(report.items[0].action.as_ref().unwrap().len() <= 4096);
    }

    #[test]
    fn export_reports_classify_vector_text_and_flattened_pixels() {
        let mut document = crate::scene::new_document(64, 48);
        document["pages"][0]["layers"][0]["nodes"] = json!([
            {"kind":"group","id":"group","children":[
                {"kind":"rect","id":"rect","x":0,"y":0,"width":10,"height":8},
                {"kind":"text","id":"label","content":"Press","x":1,"y":12}
            ]}
        ]);
        let svg = export_fidelity_report(&document, "svg", "art.pen").unwrap();
        assert_eq!(svg.counts.exact, 3);
        assert_eq!(svg.counts.approximated, 1);
        assert_eq!(svg.counts.dropped, 0);
        let png = export_fidelity_report(&document, "png", "art.pen").unwrap();
        assert_eq!(png.counts.exact, 1);
        assert_eq!(png.counts.approximated, 3);
        assert!(png
            .items
            .iter()
            .filter(|item| item.feature != "page")
            .all(|item| { item.status == FidelityStatus::Approximated }));
    }

    #[test]
    fn export_fidelity_report_accepts_legacy_root_layers() {
        let document = json!({
            "format": "pentool",
            "version": 2,
            "canvas": {"width": 100, "height": 80, "background": "#fff"},
            "layers": [{
                "id": "layer-1",
                "visible": true,
                "paths": [{"id": "legacy-path", "d": "M 0 0 L 10 10", "fill": "none"}],
                "texts": []
            }]
        });

        let report = export_fidelity_report_for(&document, "png", "legacy.pen", None, true)
            .expect("legacy root-layer documents should have export reports");

        assert_eq!(report.counts.exact, 1);
        assert_eq!(report.counts.approximated, 1);
        assert!(report
            .items
            .iter()
            .any(|item| item.source_id == "legacy-path"));
    }

    #[test]
    fn export_report_matches_selected_pages_visibility_and_rasterized_output() {
        let mut document = crate::scene::new_document(64, 48);
        document["pages"][0]["id"] = json!("page-a");
        document["pages"][0]["layers"][0]["nodes"] = json!([
            {"kind":"rect","id":"visible","x":0,"y":0,"width":10,"height":8},
            {"kind":"group","id":"group","children":[
                {"kind":"ellipse","id":"nested","cx":8,"cy":8,"radius_x":4,"radius_y":4}
            ]}
        ]);
        let hidden_layer = json!({
            "id":"hidden-layer", "name":"Hidden", "visible":false,
            "nodes":[{"kind":"rect","id":"hidden-object","x":0,"y":0,"width":4,"height":4}]
        });
        document["pages"][0]["layers"]
            .as_array_mut()
            .unwrap()
            .push(hidden_layer);
        let mut second = document["pages"][0].clone();
        second["id"] = json!("page-b");
        document["pages"].as_array_mut().unwrap().push(second);

        let report = export_fidelity_report_for(
            &document,
            "pdf",
            "layout.pen",
            Some(&["page-a".to_owned()]),
            true,
        )
        .unwrap();
        assert_eq!(
            report.counts.exact, 1,
            "only the selected page remains exact"
        );
        assert_eq!(
            report.counts.approximated, 3,
            "selected page objects were rasterized"
        );
        assert_eq!(
            report.counts.dropped, 1,
            "hidden layer content is not exported"
        );
        assert!(report.items.iter().all(|item| item.source_id != "page-b"));
        assert!(report.items.iter().any(|item| {
            item.source_id == "hidden-object" && item.status == FidelityStatus::Dropped
        }));
        assert!(report
            .items
            .iter()
            .filter(|item| item.feature != "page")
            .filter(|item| item.source_id != "hidden-object")
            .all(|item| item.status == FidelityStatus::Approximated));
    }
}

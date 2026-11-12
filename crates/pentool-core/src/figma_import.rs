//! Bounded offline importer for Figma REST JSON and the legacy v15 `.fig` subset.
//!
//! Binary container parsing is isolated in `fig_binary`; this module receives
//! only a bounded normalized tree and never calls Figma or a network service.

use crate::interchange::{
    self, FidelityReport, FidelityReportBuilder, FidelityStatus, ImportLimits,
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const MAX_CANVAS_EDGE: f64 = 16_384.0;
const MAX_JSON_DEPTH: usize = 128;

pub struct FigmaImport {
    pub pages: Vec<Value>,
    pub components: Vec<Value>,
    pub report: FidelityReport,
}

pub fn import(bytes: &[u8], source: &str) -> Result<FigmaImport> {
    import_with_limits(bytes, source, ImportLimits::default())
}

pub fn import_with_limits(bytes: &[u8], source: &str, limits: ImportLimits) -> Result<FigmaImport> {
    interchange::validate_limits(limits)?;
    limits.check_input(bytes)?;
    if source.to_ascii_lowercase().ends_with(".fig") {
        let mut root = crate::fig_binary::decode(bytes)?;
        let binary_blobs = root
            .pointer("/document/_figma_binary_blob_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if let Some(object) = root.get_mut("document").and_then(Value::as_object_mut) {
            object.remove("_figma_binary_blob_count");
        }
        return import_document(root, source, limits, "figma_fig", binary_blobs);
    }
    let root: Value = serde_json::from_slice(bytes)
        .context("[malformed-resource] Figma export is not valid JSON")?;
    import_document(root, source, limits, "figma_json", 0)
}

fn import_document(
    root: Value,
    source: &str,
    limits: ImportLimits,
    format: &str,
    binary_blobs: u64,
) -> Result<FigmaImport> {
    ensure!(
        json_depth(&root, 0, limits.max_nesting)? <= MAX_JSON_DEPTH,
        "[limit-exceeded] Figma JSON nesting exceeds {MAX_JSON_DEPTH}"
    );
    let document = root.get("document").unwrap_or(&root);
    ensure!(
        document.get("type").and_then(Value::as_str) == Some("DOCUMENT"),
        "[unsupported-format] expected a Figma DOCUMENT JSON export"
    );
    let canvases = document
        .get("children")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("[malformed-resource] Figma DOCUMENT has no CANVAS children"))?;
    ensure!(
        !canvases.is_empty() && canvases.len() <= 1000,
        "[limit-exceeded] Figma DOCUMENT must contain 1..=1000 canvases"
    );
    let mut report = FidelityReportBuilder::new(format, source);
    let mut component_ids = HashMap::new();
    for canvas in canvases {
        collect_component_ids(canvas, &mut component_ids);
    }
    let mut pages = Vec::with_capacity(canvases.len());
    let mut components = Vec::new();
    let mut total_nodes = 0usize;
    for (page_index, canvas) in canvases.iter().enumerate() {
        ensure!(
            canvas.get("type").and_then(Value::as_str) == Some("CANVAS"),
            "[malformed-resource] Figma DOCUMENT child is not a CANVAS"
        );
        let page_source = source_id(canvas, "canvas", page_index, &[]);
        if canvas.get("visible").and_then(Value::as_bool) == Some(false) {
            report.record(
                &page_source,
                "visibility",
                FidelityStatus::Dropped,
                "hidden Figma canvas omitted",
                Some("make the canvas visible in Figma before import".into()),
            );
            continue;
        }
        let name = canvas
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("Page");
        let children = canvas
            .get("children")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        ensure!(
            children.len() <= limits.max_nodes,
            "[limit-exceeded] Figma canvas layer count exceeds configured node limit"
        );
        let (min_x, min_y, max_x, max_y) = page_bounds(children)?;
        let mut ids = IdContext::default();
        let mut nodes = Vec::with_capacity(children.len());
        for (index, child) in children.iter().enumerate() {
            if let Some(node) = convert_node(
                child,
                &mut ids,
                &mut report,
                limits,
                0,
                &[index],
                &mut total_nodes,
                (min_x, min_y),
                &component_ids,
                &mut components,
            )? {
                nodes.push(node);
            }
        }
        pages.push(json!({"id":stable_id("page",&page_source,&page_index.to_string()),"name":name,
            "canvas":{"width":canvas_edge((max_x-min_x).max(1.0))?,"height":canvas_edge((max_y-min_y).max(1.0))?,"background":"#ffffff"},
            "layers":[{"id":stable_id("layer",&page_source,"root"),"name":"Imported Figma","visible":true,"locked":false,"nodes":nodes}]}));
        report.record(
            page_source,
            "canvas",
            FidelityStatus::Exact,
            "Figma CANVAS imported as a Pentool page",
            None,
        );
    }
    ensure!(
        !pages.is_empty(),
        "[invalid-input] Figma document has no visible canvases to import"
    );
    resolve_component_instances(&mut pages, &components, &mut report);
    if binary_blobs > 0 {
        report.record(
            "$document",
            "embedded_blobs",
            FidelityStatus::Dropped,
            format!(
                "{binary_blobs} embedded Figma blobs were not interpreted by the bounded v15 subset"
            ),
            Some(
                "use a Figma REST JSON export or convert vector/image assets before import".into(),
            ),
        );
    }
    Ok(FigmaImport {
        pages,
        components,
        report: report.finish(),
    })
}

fn collect_component_ids(node: &Value, ids: &mut HashMap<String, String>) {
    if node.get("type").and_then(Value::as_str) == Some("COMPONENT") {
        let source = node
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("component");
        ids.entry(source.to_owned())
            .or_insert_with(|| stable_id("component", source, "definition"));
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_component_ids(child, ids);
        }
    }
}

fn json_depth(value: &Value, depth: usize, limit: usize) -> Result<usize> {
    ensure!(
        depth <= limit,
        "[limit-exceeded] Figma JSON nesting exceeds configured limit {limit}"
    );
    let mut maximum = depth;
    match value {
        Value::Array(a) => {
            for v in a {
                maximum = maximum.max(json_depth(v, depth + 1, limit)?);
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                maximum = maximum.max(json_depth(v, depth + 1, limit)?);
            }
        }
        _ => {}
    }
    Ok(maximum)
}

fn page_bounds(children: &[Value]) -> Result<(f64, f64, f64, f64)> {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64);
    for child in children {
        if let Some((x, y, w, h)) = bounds(child)? {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + w);
            max_y = max_y.max(y + h);
        }
    }
    ensure!(
        (max_x - min_x).is_finite() && (max_y - min_y).is_finite(),
        "[malformed-resource] Figma page bounds are not finite"
    );
    Ok((min_x, min_y, max_x, max_y))
}

fn bounds(node: &Value) -> Result<Option<(f64, f64, f64, f64)>> {
    let Some(b) = node.get("absoluteBoundingBox") else {
        if let Some(s) = node.get("size") {
            return Ok(Some((
                finite(node.get("x")).unwrap_or(0.0),
                finite(node.get("y")).unwrap_or(0.0),
                finite(s.get("x")).unwrap_or(0.0),
                finite(s.get("y")).unwrap_or(0.0),
            )));
        }
        return Ok(None);
    };
    let x = finite(b.get("x"))
        .ok_or_else(|| anyhow!("[malformed-resource] Figma bounds missing finite x"))?;
    let y = finite(b.get("y"))
        .ok_or_else(|| anyhow!("[malformed-resource] Figma bounds missing finite y"))?;
    let w = finite(b.get("width"))
        .ok_or_else(|| anyhow!("[malformed-resource] Figma bounds missing finite width"))?;
    let h = finite(b.get("height"))
        .ok_or_else(|| anyhow!("[malformed-resource] Figma bounds missing finite height"))?;
    ensure!(
        w >= 0.0 && h >= 0.0,
        "[malformed-resource] Figma bounds have negative dimensions"
    );
    for v in [x, y, w, h] {
        ensure!(
            v.abs() <= MAX_CANVAS_EDGE * 16.0,
            "[limit-exceeded] Figma coordinates exceed supported range"
        );
    }
    Ok(Some((x, y, w, h)))
}
fn canvas_edge(v: f64) -> Result<u32> {
    ensure!(
        v.is_finite() && (1.0..=MAX_CANVAS_EDGE).contains(&v),
        "[limit-exceeded] Figma page canvas exceeds {MAX_CANVAS_EDGE} pixels"
    );
    Ok(v.ceil() as u32)
}

#[derive(Default)]
struct IdContext {
    used: HashSet<String>,
}
impl IdContext {
    fn id(&mut self, kind: &str, source: &str, path: &[usize]) -> String {
        let loc = path
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join("-");
        let base = stable_id(kind, source, &loc);
        if self.used.insert(base.clone()) {
            return base;
        }
        let mut n = 2;
        loop {
            let id = format!("{base}-{n}");
            if self.used.insert(id.clone()) {
                return id;
            }
            n += 1
        }
    }
}
fn source_id(n: &Value, kind: &str, index: usize, path: &[usize]) -> String {
    n.get("id")
        .or_else(|| n.get("key"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{kind}-{index}-{}",
                path.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("-")
            )
        })
}
fn stable_id(kind: &str, source: &str, location: &str) -> String {
    let digest = Sha256::digest(format!("{kind}\0{source}\0{location}").as_bytes());
    let suffix = digest[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let slug = source
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(24)
        .collect::<String>();
    if slug.is_empty() {
        format!("figma-{kind}-{suffix}")
    } else {
        format!("figma-{kind}-{slug}-{suffix}")
    }
}

#[allow(clippy::too_many_arguments)]
fn convert_node(
    source: &Value,
    ids: &mut IdContext,
    report: &mut FidelityReportBuilder,
    limits: ImportLimits,
    depth: usize,
    path: &[usize],
    total: &mut usize,
    parent: (f64, f64),
    component_ids: &HashMap<String, String>,
    components: &mut Vec<Value>,
) -> Result<Option<Value>> {
    limits.check_nesting(depth)?;
    *total = total
        .checked_add(1)
        .ok_or_else(|| anyhow!("[limit-exceeded] Figma node count overflow"))?;
    limits.check_node_count(*total)?;
    let kind = source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    let id = source_id(source, kind, 0, path);
    if source.get("visible").and_then(Value::as_bool) == Some(false) {
        report.record(
            &id,
            "visibility",
            FidelityStatus::Dropped,
            "hidden Figma node omitted",
            Some("make it visible or preserve it in Figma".into()),
        );
        return Ok(None);
    }
    let (ax, ay, w, h) = bounds(source)?.unwrap_or((parent.0, parent.1, 0.0, 0.0));
    let x = ax - parent.0;
    let y = ay - parent.1;
    report_container_features(source, &id, report);
    let node = match kind {
        "FRAME" | "GROUP" | "COMPONENT" | "INSTANCE" | "SECTION" => {
            let src_children = source
                .get("children")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let mut children = Vec::with_capacity(src_children.len());
            let mut sizes = Vec::with_capacity(src_children.len());
            let mut participates_in_layout = Vec::with_capacity(src_children.len());
            for (i, c) in src_children.iter().enumerate() {
                let mut p = path.to_vec();
                p.push(i);
                if let Some(mut n) = convert_node(
                    c,
                    ids,
                    report,
                    limits,
                    depth + 1,
                    &p,
                    total,
                    (ax, ay),
                    component_ids,
                    components,
                )? {
                    let size = bounds(c)?.map(|(_, _, w, h)| (w, h)).unwrap_or((0.0, 0.0));
                    let item_metadata = [
                        "layoutPositioning",
                        "layoutSizingHorizontal",
                        "layoutSizingVertical",
                        "layoutAlign",
                    ]
                    .into_iter()
                    .filter_map(|key| c.get(key).map(|value| (key.to_owned(), value.clone())))
                    .collect::<Map<_, _>>();
                    if !item_metadata.is_empty() {
                        n["figma_layout_item"] = Value::Object(item_metadata);
                    }
                    if c.get("layoutPositioning").and_then(Value::as_str) == Some("ABSOLUTE") {
                        report.record(
                            source_id(c, "node", i, &[]),
                            "layout_positioning",
                            FidelityStatus::Approximated,
                            "absolute-positioned child kept at its current coordinates; responsive offset rules are not represented",
                            Some("review placement after resizing the frame".into()),
                        );
                    }
                    children.push(n);
                    sizes.push(size);
                    participates_in_layout.push(
                        c.get("layoutPositioning").and_then(Value::as_str) != Some("ABSOLUTE"),
                    );
                }
            }
            let layout = source
                .get("layoutMode")
                .and_then(Value::as_str)
                .filter(|m| *m == "HORIZONTAL" || *m == "VERTICAL");
            if let Some(mode) = layout {
                let result =
                    materialize_auto_layout(source, &mut children, &sizes, &participates_in_layout);
                report.record(&id,"auto_layout_positions",FidelityStatus::Approximated,if result.applied{format!("{mode} current arrangement materialized using supported fixed-size layout properties; future resizing is not represented")}else{format!("{mode} layout could not be safely materialized; current positions retained")},Some("verify placement and resize behavior".into()));
                if result.approximate {
                    let reason = if source.get("layoutWrap").and_then(Value::as_bool) == Some(true)
                    {
                        "wrapped rows were materialized from fixed child sizes; responsive reflow and overflow are not represented"
                    } else {
                        "FILL/HUG sizing, baseline alignment, or overflowing content is not represented as responsive layout"
                    };
                    report.record(
                        &id,
                        "auto_layout_constraints",
                        FidelityStatus::Approximated,
                        reason,
                        Some("review sizing and wrapping manually".into()),
                    );
                }
            }
            let mut group = json!({"kind":"group","id":ids.id("group",&id,path),"children":children,"transform":[1,0,0,1,x,y]});
            if let Some(name) = source.get("name").and_then(Value::as_str) {
                group["name"] = json!(name)
            }
            if kind == "FRAME" || kind == "COMPONENT" {
                if let Some(color) = source
                    .get("backgroundColor")
                    .and_then(|c| figma_color_with_opacity(c, 1.0))
                {
                    group["children"].as_array_mut().unwrap().insert(0,json!({"kind":"rect","id":ids.id("frame-background",&id,path),"x":0,"y":0,"width":w,"height":h,"style":{"fill":{"fallback":color}}}));
                }
            }
            if layout.is_some() {
                let metadata = [
                    "layoutMode",
                    "itemSpacing",
                    "counterAxisSpacing",
                    "paddingLeft",
                    "paddingRight",
                    "paddingTop",
                    "paddingBottom",
                    "primaryAxisAlignItems",
                    "counterAxisAlignItems",
                    "primaryAxisSizingMode",
                    "counterAxisSizingMode",
                    "layoutWrap",
                    "itemReverseZIndex",
                    "strokesIncludedInLayout",
                    "layoutSizingHorizontal",
                    "layoutSizingVertical",
                ]
                .into_iter()
                .filter_map(|k| source.get(k).map(|v| (k.to_owned(), v.clone())))
                .collect::<Map<_, _>>();
                group["figma_auto_layout"] = Value::Object(metadata);
            }
            if kind == "COMPONENT" {
                if let Some(cid) = component_ids.get(&id) {
                    if !components.iter().any(|c| c["id"] == *cid) {
                        let snap = serde_json::to_vec(&group)?;
                        components.push(json!({"id":cid,"source_identity":hex::encode(Sha256::digest(snap)),"snapshot":group}));
                    }
                    report.record(
                        &id,
                        "component_definition",
                        FidelityStatus::Exact,
                        "Figma component definition retained as a component snapshot",
                        None,
                    );
                }
            }
            if kind == "INSTANCE" {
                if let Some(cid) = source
                    .get("componentId")
                    .and_then(Value::as_str)
                    .and_then(|c| component_ids.get(c))
                {
                    let mut fallback = group.clone();
                    fallback["id"] = json!(ids.id("instance-fallback", &id, path));
                    let applied_text_properties = apply_figma_text_component_properties(
                        source,
                        &mut fallback,
                        &id,
                        report,
                        limits.max_nodes,
                    )?;
                    let overrides = bounded_figma_component_overrides(source);
                    Some(
                        json!({"kind":"instance","id":ids.id("instance",&id,path),"component":cid,"source_identity":Value::Null,"fallback":fallback,"overrides":overrides,"transform":group["transform"],"_figma_source_id":id,"_figma_text_properties_applied":applied_text_properties}),
                    )
                } else {
                    report.record(&id,"component_reference",FidelityStatus::Dropped,"Figma instance master is not present in the imported document; imported current appearance as a group",Some("include the component definition in the export or review the detached appearance".into()));
                    Some(group)
                }
            } else {
                report.record(
                    &id,
                    "container",
                    FidelityStatus::Exact,
                    "Figma hierarchy and current placement imported as a Pentool group",
                    None,
                );
                Some(group)
            }
        }
        "RECTANGLE" => {
            let mut n = json!({"kind":"rect","id":ids.id("rect",&id,path),"x":x,"y":y,"width":w,"height":h,"style":figma_style(source,&id,report)});
            if let Some(r) = finite(source.get("cornerRadius")).filter(|r| *r >= 0.0) {
                n["radius_x"] = json!(r);
                n["radius_y"] = json!(r)
            }
            add_name(&mut n, source);
            report.record(
                &id,
                "rectangle",
                FidelityStatus::Exact,
                "rectangle frame imported",
                None,
            );
            Some(n)
        }
        "ELLIPSE" => {
            let mut n = json!({"kind":"ellipse","id":ids.id("ellipse",&id,path),"cx":x+w/2.0,"cy":y+h/2.0,"radius_x":w/2.0,"radius_y":h/2.0,"style":figma_style(source,&id,report)});
            add_name(&mut n, source);
            report.record(
                &id,
                "ellipse",
                FidelityStatus::Exact,
                "ellipse frame imported",
                None,
            );
            Some(n)
        }
        "LINE" => {
            let mut n = json!({"kind":"line","id":ids.id("line",&id,path),"x1":x,"y1":y,"x2":x+w,"y2":y+h,"style":figma_style(source,&id,report)});
            add_name(&mut n, source);
            report.record(
                &id,
                "line",
                FidelityStatus::Approximated,
                "line bounds and stroke imported; endpoint semantics may differ",
                Some("review line appearance".into()),
            );
            Some(n)
        }
        "TEXT" => {
            let chars = source
                .get("characters")
                .and_then(Value::as_str)
                .unwrap_or("");
            let style = source.get("style");
            let size = style
                .and_then(|v| finite(v.get("fontSize")))
                .unwrap_or(16.0);
            ensure!(
                (0.0..=4096.0).contains(&size),
                "[limit-exceeded] invalid Figma font size"
            );
            let family = style
                .and_then(|v| v.get("fontFamily"))
                .and_then(Value::as_str)
                .unwrap_or("Arial");
            let weight = style
                .and_then(|v| v.get("fontWeight"))
                .and_then(Value::as_u64)
                .unwrap_or(400)
                .min(1000);
            let fills = figma_style(source, &id, report);
            let fill = fills
                .get("fill")
                .and_then(|f| f.get("fallback"))
                .and_then(Value::as_str)
                .unwrap_or("#000000");
            let mut n = json!({"kind":"text","id":ids.id("text",&id,path),"content":chars,"x":x,"y":y+size,"font_family":family,"font_size":size,"font_weight":weight,"style":{"fill":{"fallback":fill}}});
            add_name(&mut n, source);
            report.record(&id,"text",FidelityStatus::Approximated,"text content and primary font style imported; rich text and paragraph layout are not fully represented",Some("review font and text fitting".into()));
            Some(n)
        }
        "STAR" | "REGULAR_POLYGON" => {
            ensure!(
                w > 0.0 && h > 0.0,
                "[malformed-resource] Figma {kind} must have positive bounds"
            );
            let point_count = source
                .get("pointCount")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    anyhow!("[malformed-resource] Figma {kind} is missing an integer pointCount")
                })?;
            ensure!(
                point_count >= 3,
                "[malformed-resource] Figma {kind} pointCount must be at least 3"
            );
            ensure!(
                point_count <= 512,
                "[limit-exceeded] Figma {kind} pointCount exceeds 512"
            );
            let inner_radius = if kind == "STAR" {
                let value = finite(source.get("innerRadius")).ok_or_else(|| {
                    anyhow!("[malformed-resource] Figma STAR is missing a finite innerRadius")
                })?;
                ensure!(
                    (0.0..=1.0).contains(&value),
                    "[malformed-resource] Figma STAR innerRadius must be between 0 and 1"
                );
                Some(value)
            } else {
                None
            };
            let d = figma_regular_path(x, y, w, h, point_count as usize, inner_radius);
            let mut n = json!({"kind":"path","id":ids.id("path",&id,path),"d":d,"closed":true,"style":figma_style(source,&id,report)});
            add_name(&mut n, source);
            let rounded = finite(source.get("cornerRadius")).is_some_and(|radius| radius > 0.0);
            report.record(
                &id,
                kind.to_ascii_lowercase(),
                if rounded {
                    FidelityStatus::Approximated
                } else {
                    FidelityStatus::Exact
                },
                if rounded {
                    "regular geometry imported; Figma corner rounding is not represented"
                } else {
                    "regular geometry imported as an editable closed path"
                },
                rounded.then(|| "review rounded corners after import".into()),
            );
            Some(n)
        }
        "VECTOR" | "BOOLEAN_OPERATION" => {
            report.record(
                &id,
                kind.to_ascii_lowercase(),
                FidelityStatus::Dropped,
                format!("Figma {kind} geometry is outside the static primitive subset"),
                Some("convert to a supported primitive in Figma".into()),
            );
            None
        }
        "CANVAS" | "DOCUMENT" => bail!("[malformed-resource] nested Figma {kind} node is invalid"),
        other => {
            report.record(
                &id,
                "object",
                FidelityStatus::Dropped,
                format!("Figma node type `{other}` is not supported"),
                Some("convert it to a static primitive or text".into()),
            );
            None
        }
    };
    let mut node = node;
    if let Some(node) = node.as_mut() {
        apply_node_opacity(node, source, &id, report);
        if kind == "COMPONENT" {
            if let Some(component_id) = component_ids.get(&id) {
                if let Some(component) = components.iter_mut().find(|c| c["id"] == *component_id) {
                    let snapshot = node.clone();
                    let bytes = serde_json::to_vec(&snapshot)?;
                    component["source_identity"] = json!(hex::encode(Sha256::digest(&bytes)));
                    component["snapshot"] = snapshot;
                }
            }
        }
    }
    Ok(node)
}

fn resolve_component_instances(
    pages: &mut [Value],
    components: &[Value],
    report: &mut FidelityReportBuilder,
) {
    let component_by_id: HashMap<&str, &Value> = components
        .iter()
        .filter_map(|component| Some((component.get("id")?.as_str()?, component)))
        .collect();
    fn walk(
        nodes: &mut [Value],
        component_by_id: &HashMap<&str, &Value>,
        report: &mut FidelityReportBuilder,
    ) {
        for node in nodes {
            if node.get("kind").and_then(Value::as_str) == Some("instance") {
                let source_id = node
                    .get("_figma_source_id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| node.get("id").and_then(Value::as_str).unwrap_or("instance"))
                    .to_owned();
                let component_id = node.get("component").and_then(Value::as_str);
                let definition = component_id.and_then(|id| component_by_id.get(id).copied());
                if let Some(definition) = definition {
                    node["source_identity"] = definition["source_identity"].clone();
                    node.as_object_mut().unwrap().remove("_figma_source_id");
                    let applied_text = node
                        .get("_figma_text_properties_applied")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    node.as_object_mut()
                        .unwrap()
                        .remove("_figma_text_properties_applied");
                    let message = if applied_text > 0 {
                        "Figma instance links to its component; supported TEXT properties were applied to uniquely named fallback nodes, while live override propagation is not represented"
                    } else {
                        "Figma instance links to its imported component and retains current appearance as fallback; live override semantics are not represented"
                    };
                    report.record(
                        &source_id,
                        "component_instance",
                        FidelityStatus::Approximated,
                        message,
                        Some("review instance overrides".into()),
                    );
                    report_component_override_metadata(node, &source_id, report);
                } else {
                    let fallback = node.get("fallback").cloned().unwrap_or(Value::Null);
                    let mut fallback = fallback;
                    if let Some(opacity) = node.get("opacity").cloned() {
                        fallback["opacity"] = opacity;
                    }
                    let overrides = node.get("overrides").cloned().unwrap_or_else(|| json!({}));
                    if overrides.as_object().is_some_and(|map| !map.is_empty()) {
                        fallback["figma_component_overrides"] = overrides;
                    }
                    *node = fallback;
                    report.record(&source_id,"component_reference",FidelityStatus::Dropped,"Figma component master was hidden or omitted by the supported import subset; current instance appearance imported as a group",Some("include the component definition in the export or review the detached appearance".into()));
                    report_component_override_metadata(node, &source_id, report);
                }
            }
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                walk(children, component_by_id, report);
            }
        }
    }
    for page in pages {
        if let Some(layers) = page.get_mut("layers").and_then(Value::as_array_mut) {
            for layer in layers {
                if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                    walk(nodes, &component_by_id, report);
                }
            }
        }
    }
}

fn report_component_override_metadata(
    instance_or_fallback: &Value,
    source_id: &str,
    report: &mut FidelityReportBuilder,
) {
    let overrides = instance_or_fallback
        .get("overrides")
        .or_else(|| instance_or_fallback.get("figma_component_overrides"));
    let Some(overrides) = overrides.and_then(Value::as_object) else {
        return;
    };
    let omitted = overrides
        .get("figma_metadata_omitted")
        .and_then(Value::as_bool)
        == Some(true);
    let retained = overrides.keys().any(|key| key != "figma_metadata_omitted");
    if retained {
        report.record(
            source_id,
            "component_override_metadata",
            FidelityStatus::Approximated,
            "bounded Figma component override metadata retained; where supported, static text properties are applied to matching fallback nodes, but no live Figma update binding is created",
            Some("review the fallback after changing the component".into()),
        );
    }
    if omitted {
        report.record(
            source_id,
            "component_override_metadata",
            FidelityStatus::Dropped,
            "some Figma component property metadata exceeded importer limits or used unsupported values",
            Some("review the original Figma component properties".into()),
        );
    }
}

fn apply_node_opacity(
    node: &mut Value,
    source: &Value,
    id: &str,
    report: &mut FidelityReportBuilder,
) {
    let Some(raw) = source.get("opacity") else {
        return;
    };
    let Some(opacity) = finite(Some(raw)).filter(|v| (0.0..=1.0).contains(v)) else {
        report.record(
            id,
            "node_opacity",
            FidelityStatus::Dropped,
            "invalid Figma node opacity was ignored",
            Some("set opacity between 0 and 1 in Figma".into()),
        );
        return;
    };
    if opacity < 1.0 {
        node["opacity"] = json!(opacity);
        report.record(
            id,
            "node_opacity",
            FidelityStatus::Exact,
            "node opacity preserved",
            None,
        );
    }
}

fn apply_figma_text_component_properties(
    source: &Value,
    fallback: &mut Value,
    instance_id: &str,
    report: &mut FidelityReportBuilder,
    max_nodes: usize,
) -> Result<usize> {
    const MAX_PROPERTIES: usize = 128;
    const MAX_NAME_BYTES: usize = 128;
    const MAX_TEXT_BYTES: usize = 4096;
    let Some(properties) = source.get("componentProperties").and_then(Value::as_object) else {
        return Ok(0);
    };
    let mut applied = 0usize;
    for (name, property) in properties.iter().take(MAX_PROPERTIES) {
        let value = property.get("value");
        let text_target = name
            .strip_suffix("#text")
            .filter(|target| !target.is_empty() && target.len() <= MAX_NAME_BYTES);
        let text_value = value
            .and_then(Value::as_str)
            .filter(|text| text.len() <= MAX_TEXT_BYTES);
        if name.len() > MAX_NAME_BYTES
            || property.get("type").and_then(Value::as_str) != Some("TEXT")
            || text_target.is_none()
            || text_value.is_none()
        {
            report.record(
                instance_id,
                "component_property",
                FidelityStatus::Dropped,
                format!(
                    "Figma component property `{}` is outside the supported text-property subset",
                    bounded_name(name)
                ),
                Some("review this component property in the source design".into()),
            );
            continue;
        }
        let target = text_target.unwrap();
        let mut visited = 0usize;
        let matches = count_named_text_nodes(fallback, target, &mut visited, max_nodes, 0)?;
        if matches != 1 {
            report.record(
                instance_id,
                "component_property",
                FidelityStatus::Dropped,
                format!("Figma text property `{}` matched {} fallback text nodes; exactly one is required", bounded_name(name), matches),
                Some("rename the corresponding instance text layer uniquely before importing".into()),
            );
            continue;
        }
        set_named_text_node(
            fallback,
            target,
            text_value.unwrap(),
            &mut visited,
            max_nodes,
            0,
        )?;
        applied += 1;
        report.record(
            instance_id,
            "component_property_text",
            FidelityStatus::Approximated,
            format!("Figma text component property `{}` applied to its uniquely named materialized fallback node", bounded_name(name)),
            None,
        );
    }
    if properties.len() > MAX_PROPERTIES {
        report.record(
            instance_id,
            "component_property",
            FidelityStatus::Dropped,
            "additional Figma component properties exceeded the 128-property import limit",
            None,
        );
    }
    Ok(applied)
}

fn count_named_text_nodes(
    node: &Value,
    target: &str,
    visited: &mut usize,
    max_nodes: usize,
    depth: usize,
) -> Result<usize> {
    ensure!(
        depth <= 256,
        "[limit-exceeded] Figma component fallback nesting exceeds 256"
    );
    *visited = (*visited)
        .checked_add(1)
        .ok_or_else(|| anyhow!("[limit-exceeded] Figma component override node count overflow"))?;
    ensure!(
        *visited <= max_nodes,
        "[limit-exceeded] Figma component override traversal exceeds the import node limit"
    );
    let mut matches = usize::from(
        node.get("kind").and_then(Value::as_str) == Some("text")
            && node.get("name").and_then(Value::as_str) == Some(target),
    );
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            matches = matches.saturating_add(count_named_text_nodes(
                child,
                target,
                visited,
                max_nodes,
                depth + 1,
            )?);
        }
    }
    Ok(matches)
}

fn set_named_text_node(
    node: &mut Value,
    target: &str,
    value: &str,
    visited: &mut usize,
    max_nodes: usize,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= 256,
        "[limit-exceeded] Figma component fallback nesting exceeds 256"
    );
    *visited = (*visited)
        .checked_add(1)
        .ok_or_else(|| anyhow!("[limit-exceeded] Figma component override node count overflow"))?;
    ensure!(
        *visited <= max_nodes,
        "[limit-exceeded] Figma component override traversal exceeds the import node limit"
    );
    if node.get("kind").and_then(Value::as_str) == Some("text")
        && node.get("name").and_then(Value::as_str) == Some(target)
    {
        node["content"] = json!(value);
        return Ok(());
    }
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            set_named_text_node(child, target, value, visited, max_nodes, depth + 1)?;
        }
    }
    Ok(())
}

fn bounded_name(value: &str) -> String {
    const MAX: usize = 64;
    if value.len() <= MAX {
        value.to_owned()
    } else {
        let end = (0..=MAX)
            .rev()
            .find(|index| value.is_char_boundary(*index))
            .unwrap_or(0);
        format!("{}…", &value[..end])
    }
}

fn bounded_figma_component_overrides(source: &Value) -> Value {
    const MAX_PROPERTIES: usize = 128;
    const MAX_PROPERTY_NAME: usize = 128;
    const MAX_TEXT_VALUE: usize = 4096;

    let mut result = Map::new();
    let mut properties = Map::new();
    let mut omitted = false;
    if let Some(entries) = source.get("componentProperties").and_then(Value::as_object) {
        for (name, property) in entries.iter().take(MAX_PROPERTIES) {
            let value = property.get("value");
            let supported_value = match value {
                Some(Value::Bool(_) | Value::Number(_)) => true,
                Some(Value::String(text)) => text.len() <= MAX_TEXT_VALUE,
                _ => false,
            };
            let property_type = property.get("type").and_then(Value::as_str);
            if name.is_empty()
                || name.len() > MAX_PROPERTY_NAME
                || !supported_value
                || property_type.is_none()
            {
                omitted = true;
                continue;
            }
            properties.insert(
                name.clone(),
                json!({"type":property_type.unwrap(),"value":value.unwrap()}),
            );
        }
        omitted |= entries.len() > MAX_PROPERTIES;
    }
    if !properties.is_empty() {
        result.insert(
            "figma_component_properties".into(),
            Value::Object(properties),
        );
    }

    const MAX_OVERRIDES: usize = 256;
    if let Some(entries) = source.get("overrides").and_then(Value::as_array) {
        let mut preserved = Vec::new();
        for entry in entries.iter().take(MAX_OVERRIDES) {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                omitted = true;
                continue;
            };
            let Some(fields) = entry.get("overriddenFields").and_then(Value::as_array) else {
                omitted = true;
                continue;
            };
            if id.is_empty() || id.len() > MAX_PROPERTY_NAME || fields.len() > 64 {
                omitted = true;
                continue;
            }
            let mut valid_fields = Vec::new();
            for field in fields {
                if field
                    .as_str()
                    .is_some_and(|value| !value.is_empty() && value.len() <= 128)
                {
                    valid_fields.push(field.clone());
                } else {
                    omitted = true;
                }
            }
            preserved.push(json!({"id":id,"overriddenFields":valid_fields}));
        }
        omitted |= entries.len() > MAX_OVERRIDES;
        if !preserved.is_empty() {
            result.insert("figma_overrides".into(), Value::Array(preserved));
        }
    }
    if omitted {
        result.insert("figma_metadata_omitted".into(), Value::Bool(true));
    }
    Value::Object(result)
}

struct LayoutResult {
    applied: bool,
    approximate: bool,
}
fn materialize_auto_layout(
    source: &Value,
    children: &mut [Value],
    sizes: &[(f64, f64)],
    participates_in_layout: &[bool],
) -> LayoutResult {
    if children.len() != sizes.len() || children.len() != participates_in_layout.len() {
        return LayoutResult {
            applied: false,
            approximate: true,
        };
    }
    if source.get("layoutWrap").and_then(Value::as_bool) == Some(true) {
        return materialize_wrapped_auto_layout(source, children, sizes, participates_in_layout);
    }
    let horizontal = source.get("layoutMode").and_then(Value::as_str) == Some("HORIZONTAL");
    let (cw, ch) = bounds(source)
        .ok()
        .flatten()
        .map(|(_, _, w, h)| (w, h))
        .unwrap_or((0.0, 0.0));
    let pad = |k: &str| finite(source.get(k)).unwrap_or(0.0);
    let (l, r, t, b) = (
        pad("paddingLeft"),
        pad("paddingRight"),
        pad("paddingTop"),
        pad("paddingBottom"),
    );
    let gap = finite(source.get("itemSpacing")).unwrap_or(0.0);
    if [l, r, t, b, gap].iter().any(|v| *v < 0.0) {
        return LayoutResult {
            applied: false,
            approximate: true,
        };
    }
    let psize = if horizontal { cw - l - r } else { ch - t - b };
    let csize = if horizontal { ch - t - b } else { cw - l - r };
    let pstart = if horizontal { l } else { t };
    let cstart = if horizontal { t } else { l };
    let justify = source
        .get("primaryAxisAlignItems")
        .and_then(Value::as_str)
        .unwrap_or("MIN");
    let align = source
        .get("counterAxisAlignItems")
        .and_then(Value::as_str)
        .unwrap_or("MIN");
    if !["MIN", "CENTER", "MAX", "SPACE_BETWEEN"].contains(&justify)
        || !["MIN", "CENTER", "MAX", "BASELINE"].contains(&align)
    {
        return LayoutResult {
            applied: false,
            approximate: true,
        };
    }
    let flow_count = participates_in_layout.iter().filter(|flow| **flow).count();
    let total = sizes
        .iter()
        .zip(participates_in_layout)
        .filter(|(_, flow)| **flow)
        .map(|((w, h), _)| if horizontal { *w } else { *h })
        .sum::<f64>()
        + gap * flow_count.saturating_sub(1) as f64;
    let free = psize - total;
    let leading = match justify {
        "CENTER" => free.max(0.0) / 2.0,
        "MAX" => free.max(0.0),
        _ => 0.0,
    };
    let actual_gap = if justify == "SPACE_BETWEEN" && flow_count > 1 {
        gap + free.max(0.0) / (flow_count - 1) as f64
    } else {
        gap
    };
    let mut cursor = pstart + leading;
    for ((child, (w, h)), flow) in children.iter_mut().zip(sizes).zip(participates_in_layout) {
        if !flow {
            continue;
        }
        let (pe, ce) = if horizontal { (*w, *h) } else { (*h, *w) };
        let cf = csize - ce;
        let co = match align {
            "CENTER" => cf.max(0.0) / 2.0,
            "MAX" => cf.max(0.0),
            _ => 0.0,
        };
        let (tx, ty) = if horizontal {
            (cursor, cstart + co)
        } else {
            (cstart + co, cursor)
        };
        let (ox, oy) = node_origin(child, (*w, *h));
        translate_node(child, tx - ox, ty - oy);
        cursor += pe + actual_gap;
    }
    let sizing = ["primaryAxisSizingMode", "counterAxisSizingMode"]
        .iter()
        .any(|k| source.get(k).and_then(Value::as_str) == Some("AUTO"))
        || children.iter().any(|child| {
            ["layoutSizingHorizontal", "layoutSizingVertical"]
                .iter()
                .any(|key| {
                    matches!(
                        child
                            .pointer(&format!("/figma_layout_item/{key}"))
                            .and_then(Value::as_str),
                        Some("FILL" | "HUG")
                    )
                })
        });
    LayoutResult {
        applied: true,
        approximate: sizing || align == "BASELINE" || free < 0.0 || csize < 0.0,
    }
}

/// Materialize Figma's horizontal/vertical wrap using fixed imported child
/// sizes. This records the current arrangement; it does not create responsive
/// reflow rules in the Pentool scene.
fn materialize_wrapped_auto_layout(
    source: &Value,
    children: &mut [Value],
    sizes: &[(f64, f64)],
    participates_in_layout: &[bool],
) -> LayoutResult {
    let horizontal = source.get("layoutMode").and_then(Value::as_str) == Some("HORIZONTAL");
    if !horizontal && source.get("layoutMode").and_then(Value::as_str) != Some("VERTICAL") {
        return LayoutResult {
            applied: false,
            approximate: true,
        };
    }
    let (cw, ch) = match bounds(source).ok().flatten().map(|(_, _, w, h)| (w, h)) {
        Some(size)
            if size.0.is_finite() && size.1.is_finite() && size.0 >= 0.0 && size.1 >= 0.0 =>
        {
            size
        }
        _ => {
            return LayoutResult {
                applied: false,
                approximate: true,
            };
        }
    };
    let pad = |key: &str| finite(source.get(key)).unwrap_or(0.0);
    let (left, right, top, bottom) = (
        pad("paddingLeft"),
        pad("paddingRight"),
        pad("paddingTop"),
        pad("paddingBottom"),
    );
    let gap = finite(source.get("itemSpacing")).unwrap_or(0.0);
    let line_gap = finite(source.get("counterAxisSpacing")).unwrap_or(0.0);
    if [left, right, top, bottom, gap, line_gap]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
    {
        return LayoutResult {
            applied: false,
            approximate: true,
        };
    }
    let (primary_extent, cross_extent, primary_start, cross_start) = if horizontal {
        (cw - left - right, ch - top - bottom, left, top)
    } else {
        (ch - top - bottom, cw - left - right, top, left)
    };
    if primary_extent < 0.0 || cross_extent < 0.0 {
        return LayoutResult {
            applied: false,
            approximate: true,
        };
    }
    let justify = source
        .get("primaryAxisAlignItems")
        .and_then(Value::as_str)
        .unwrap_or("MIN");
    let align = source
        .get("counterAxisAlignItems")
        .and_then(Value::as_str)
        .unwrap_or("MIN");
    if !["MIN", "CENTER", "MAX", "SPACE_BETWEEN"].contains(&justify)
        || !["MIN", "CENTER", "MAX"].contains(&align)
    {
        return LayoutResult {
            applied: false,
            approximate: true,
        };
    }

    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut current = Vec::new();
    let mut used = 0.0;
    for (index, (size, flow)) in sizes.iter().zip(participates_in_layout).enumerate() {
        if !flow {
            continue;
        }
        let primary = if horizontal { size.0 } else { size.1 };
        let cross = if horizontal { size.1 } else { size.0 };
        if !primary.is_finite() || !cross.is_finite() || primary < 0.0 || cross < 0.0 {
            return LayoutResult {
                applied: false,
                approximate: true,
            };
        }
        let next = if current.is_empty() {
            primary
        } else {
            used + gap + primary
        };
        if !current.is_empty() && next > primary_extent {
            lines.push(std::mem::take(&mut current));
            used = primary;
        } else {
            used = next;
        }
        current.push(index);
    }
    if !current.is_empty() {
        lines.push(current);
    }

    let mut cross_cursor = cross_start;
    for line in &lines {
        let line_cross = line
            .iter()
            .map(|index| {
                if horizontal {
                    sizes[*index].1
                } else {
                    sizes[*index].0
                }
            })
            .fold(0.0, f64::max);
        let content_primary = line
            .iter()
            .map(|index| {
                if horizontal {
                    sizes[*index].0
                } else {
                    sizes[*index].1
                }
            })
            .sum::<f64>()
            + gap * line.len().saturating_sub(1) as f64;
        let free = primary_extent - content_primary;
        let leading = match justify {
            "CENTER" => free.max(0.0) / 2.0,
            "MAX" => free.max(0.0),
            _ => 0.0,
        };
        let actual_gap = if justify == "SPACE_BETWEEN" && line.len() > 1 {
            gap + free.max(0.0) / (line.len() - 1) as f64
        } else {
            gap
        };
        let mut primary_cursor = primary_start + leading;
        for index in line {
            let (width, height) = sizes[*index];
            let cross_size = if horizontal { height } else { width };
            let cross_free = line_cross - cross_size;
            let cross_offset = match align {
                "CENTER" => cross_free / 2.0,
                "MAX" => cross_free,
                _ => 0.0,
            };
            let (x, y) = if horizontal {
                (primary_cursor, cross_cursor + cross_offset)
            } else {
                (cross_cursor + cross_offset, primary_cursor)
            };
            let (origin_x, origin_y) = node_origin(&children[*index], (width, height));
            translate_node(&mut children[*index], x - origin_x, y - origin_y);
            primary_cursor += (if horizontal { width } else { height }) + actual_gap;
        }
        cross_cursor += line_cross + line_gap;
    }
    LayoutResult {
        applied: true,
        approximate: true,
    }
}
fn node_origin(n: &Value, size: (f64, f64)) -> (f64, f64) {
    match n.get("kind").and_then(Value::as_str) {
        Some("rect") => (
            finite(n.get("x")).unwrap_or(0.0),
            finite(n.get("y")).unwrap_or(0.0),
        ),
        Some("text") => (
            finite(n.get("x")).unwrap_or(0.0),
            finite(n.get("y")).unwrap_or(0.0) - finite(n.get("font_size")).unwrap_or(size.1),
        ),
        Some("ellipse") => (
            finite(n.get("cx")).unwrap_or(0.0) - size.0 / 2.0,
            finite(n.get("cy")).unwrap_or(0.0) - size.1 / 2.0,
        ),
        Some("line") => (
            finite(n.get("x1"))
                .unwrap_or(0.0)
                .min(finite(n.get("x2")).unwrap_or(0.0)),
            finite(n.get("y1"))
                .unwrap_or(0.0)
                .min(finite(n.get("y2")).unwrap_or(0.0)),
        ),
        _ => {
            let m = n.get("transform").and_then(Value::as_array);
            (
                m.and_then(|a| a.get(4))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
                m.and_then(|a| a.get(5))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            )
        }
    }
}
fn translate_node(n: &mut Value, dx: f64, dy: f64) {
    match n.get("kind").and_then(Value::as_str) {
        Some("rect") | Some("text") => {
            n["x"] = json!(finite(n.get("x")).unwrap_or(0.0) + dx);
            n["y"] = json!(finite(n.get("y")).unwrap_or(0.0) + dy)
        }
        Some("ellipse") => {
            n["cx"] = json!(finite(n.get("cx")).unwrap_or(0.0) + dx);
            n["cy"] = json!(finite(n.get("cy")).unwrap_or(0.0) + dy)
        }
        Some("line") => {
            for k in ["x1", "x2"] {
                n[k] = json!(finite(n.get(k)).unwrap_or(0.0) + dx)
            }
            for k in ["y1", "y2"] {
                n[k] = json!(finite(n.get(k)).unwrap_or(0.0) + dy)
            }
        }
        _ => {
            if let Some(m) = n.get_mut("transform").and_then(Value::as_array_mut) {
                if m.len() == 6 {
                    m[4] = json!(m[4].as_f64().unwrap_or(0.0) + dx);
                    m[5] = json!(m[5].as_f64().unwrap_or(0.0) + dy)
                }
            }
        }
    }
}

fn figma_style(source: &Value, id: &str, report: &mut FidelityReportBuilder) -> Value {
    let mut style = Map::new();
    if let Some(fills) = source.get("fills").and_then(Value::as_array) {
        let visible = fills
            .iter()
            .filter(|p| {
                p.get("visible").and_then(Value::as_bool) != Some(false)
                    && finite(p.get("opacity")).unwrap_or(1.0) != 0.0
            })
            .collect::<Vec<_>>();
        if let Some(paint) = visible.first() {
            let opacity = paint_opacity(paint, id, "fill_opacity", report);
            if let Some(c) = paint
                .get("color")
                .and_then(|color| figma_color_with_opacity(color, opacity))
            {
                style.insert("fill".into(), json!({"fallback":c}));
            }
        }
        if visible.len() > 1 {
            report.record(
                id,
                "additional_fills",
                FidelityStatus::Dropped,
                "only first visible solid fill imported",
                Some("flatten additional fills".into()),
            )
        }
        if visible
            .first()
            .is_some_and(|p| p.get("type").and_then(Value::as_str) != Some("SOLID"))
        {
            report.record(
                id,
                "fill_type",
                FidelityStatus::Dropped,
                "gradient/image fills not interpreted",
                Some("replace with solid fill or flatten".into()),
            )
        }
    }
    if let Some(strokes) = source.get("strokes").and_then(Value::as_array) {
        let visible = strokes
            .iter()
            .filter(|p| {
                p.get("visible").and_then(Value::as_bool) != Some(false)
                    && finite(p.get("opacity")).unwrap_or(1.0) != 0.0
            })
            .collect::<Vec<_>>();
        if let Some(s) = visible.first() {
            let opacity = paint_opacity(s, id, "stroke_opacity", report);
            if let Some(c) = s
                .get("color")
                .and_then(|color| figma_color_with_opacity(color, opacity))
            {
                style.insert("stroke".into(), json!({"fallback":c}));
                if let Some(w) = finite(source.get("strokeWeight")).filter(|w| *w >= 0.0) {
                    style.insert("stroke_width".into(), json!({"fallback":w}));
                }
            }
        }
        if visible.len() > 1 {
            report.record(
                id,
                "additional_strokes",
                FidelityStatus::Dropped,
                "only first visible stroke imported",
                Some("flatten additional strokes".into()),
            )
        }
    }
    Value::Object(style)
}
fn paint_opacity(
    paint: &Value,
    id: &str,
    feature: &str,
    report: &mut FidelityReportBuilder,
) -> f64 {
    let Some(raw) = paint.get("opacity") else {
        return 1.0;
    };
    match finite(Some(raw)).filter(|v| (0.0..=1.0).contains(v)) {
        Some(opacity) => {
            if opacity < 1.0 {
                report.record(
                    id,
                    feature,
                    FidelityStatus::Exact,
                    "paint opacity combined with the solid paint alpha",
                    None,
                );
            }
            opacity
        }
        None => {
            report.record(
                id,
                feature,
                FidelityStatus::Dropped,
                "invalid Figma paint opacity was ignored",
                Some("set paint opacity between 0 and 1 in Figma".into()),
            );
            1.0
        }
    }
}

fn figma_color_with_opacity(c: &Value, opacity: f64) -> Option<String> {
    let ch = |k: &str| -> Option<u8> {
        let n = finite(c.get(k))?;
        (0.0..=1.0)
            .contains(&n)
            .then_some((n * 255.0).round() as u8)
    };
    let (r, g, b) = (ch("r")?, ch("g")?, ch("b")?);
    let alpha = finite(c.get("a")).unwrap_or(1.0) * opacity;
    if !(0.0..=1.0).contains(&alpha) {
        return None;
    }
    let a = (alpha * 255.0).round() as u8;
    Some(if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    })
}
fn report_container_features(n: &Value, id: &str, r: &mut FidelityReportBuilder) {
    if finite(n.get("rotation")).is_some_and(|v| v.abs() > 0.0001) {
        r.record(
            id,
            "rotation",
            FidelityStatus::Approximated,
            "rotation is not applied; axis-aligned bounds used",
            Some("review placement".into()),
        )
    }
    if n.get("effects")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
    {
        r.record(
            id,
            "effects",
            FidelityStatus::Dropped,
            "Figma effects not imported",
            Some("flatten effects".into()),
        )
    }
    if n.get("clipsContent").and_then(Value::as_bool) == Some(true) {
        r.record(
            id,
            "clip_content",
            FidelityStatus::Dropped,
            "frame clipping is not represented",
            Some("flatten clipped content".into()),
        )
    }
}
fn add_name(n: &mut Value, s: &Value) {
    if let Some(name) = s.get("name").and_then(Value::as_str) {
        n["name"] = json!(name)
    }
}
fn finite(v: Option<&Value>) -> Option<f64> {
    let n = v?.as_f64()?;
    n.is_finite().then_some(n)
}

fn figma_regular_path(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    point_count: usize,
    inner_radius: Option<f64>,
) -> String {
    use std::f64::consts::PI;
    let steps = point_count * if inner_radius.is_some() { 2 } else { 1 };
    let mut path = String::new();
    for index in 0..steps {
        let angle = -PI / 2.0 + 2.0 * PI * index as f64 / steps as f64;
        let radius = match inner_radius {
            Some(inner) if index % 2 == 1 => inner,
            _ => 1.0,
        };
        let px = x + width * (0.5 + 0.5 * radius * angle.cos());
        let py = y + height * (0.5 + 0.5 * radius * angle.sin());
        if index == 0 {
            path.push_str(&format!("M {px:.6} {py:.6}"));
        } else {
            path.push_str(&format!(" L {px:.6} {py:.6}"));
        }
    }
    path.push_str(" Z");
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        json!({"document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[{"id":"frame","type":"FRAME","name":"Frame","layoutMode":"HORIZONTAL","itemSpacing":12,"paddingLeft":8,"paddingTop":4,"primaryAxisAlignItems":"MIN","counterAxisAlignItems":"CENTER","absoluteBoundingBox":{"x":100,"y":50,"width":180,"height":80},"children":[{"id":"a","type":"RECTANGLE","absoluteBoundingBox":{"x":110,"y":60,"width":70,"height":30},"fills":[{"type":"SOLID","color":{"r":0.2,"g":0.4,"b":0.8,"a":1}}]},{"id":"b","type":"TEXT","absoluteBoundingBox":{"x":120,"y":65,"width":50,"height":20},"characters":"Go","style":{"fontFamily":"Arial","fontSize":14,"fontWeight":600}}]}]}]}})
    }
    #[test]
    fn imports_and_materializes_auto_layout_deterministically() {
        let b = serde_json::to_vec(&fixture()).unwrap();
        let a = import(&b, "design.json").unwrap();
        let again = import(&b, "design.json").unwrap();
        assert_eq!(a.pages, again.pages);
        let g = &a.pages[0]["layers"][0]["nodes"][0];
        assert_eq!(g["children"][0]["x"], json!(8.0));
        assert_eq!(g["children"][1]["x"], json!(90.0));
        assert_eq!(g["children"][0]["y"], json!(27.0));
        // Pentool text nodes position their origin at the baseline, not the top
        // of the Figma text bounds.
        assert_eq!(g["children"][1]["y"], json!(46.0));
        assert_eq!(g["children"][1]["content"], "Go");
        assert!(a
            .report
            .items
            .iter()
            .any(|i| i.feature == "auto_layout_positions"));
        assert!(g.get("figma_auto_layout").is_some());
    }
    #[test]
    fn components_and_instances_are_retained() {
        let mut f = fixture();
        let c = json!({"id":"master","type":"COMPONENT","absoluteBoundingBox":{"x":0,"y":0,"width":20,"height":10},"children":[]});
        let mut p = c.clone();
        p["type"] = json!("INSTANCE");
        p["id"] = json!("instance");
        p["componentId"] = json!("master");
        f["document"]["children"][0]["children"]
            .as_array_mut()
            .unwrap()
            .push(c);
        f["document"]["children"][0]["children"]
            .as_array_mut()
            .unwrap()
            .push(p);
        let a = import(&serde_json::to_vec(&f).unwrap(), "d.json").unwrap();
        assert_eq!(a.components.len(), 1);
        assert_eq!(a.components[0]["snapshot"]["kind"], "group");
        let nodes = &a.pages[0]["layers"][0]["nodes"];
        assert_eq!(nodes[1]["kind"], "group");
        assert_eq!(nodes[2]["kind"], "instance");
    }
    #[test]
    fn rejects_fig_and_input_over_limit() {
        assert!(import(b"FIG", "file.fig").is_err());
        let b = serde_json::to_vec(&fixture()).unwrap();
        let l = ImportLimits {
            max_input_bytes: 1,
            ..ImportLimits::default()
        };
        assert!(import_with_limits(&b, "x", l)
            .err()
            .unwrap()
            .to_string()
            .contains("limit-exceeded"));
    }
    #[test]
    fn drops_unknown_geometry_with_report() {
        let mut f = fixture();
        f["document"]["children"][0]["children"][0]["children"].as_array_mut().unwrap().push(json!({"id":"v","type":"VECTOR","absoluteBoundingBox":{"x":0,"y":0,"width":1,"height":1}}));
        let a = import(&serde_json::to_vec(&f).unwrap(), "d.json").unwrap();
        assert!(a
            .report
            .items
            .iter()
            .any(|i| i.source_id == "v" && i.status == FidelityStatus::Dropped));
    }

    #[test]
    fn imports_bounded_figma_stars_and_regular_polygons_as_editable_paths() {
        let mut f = fixture();
        let children = f["document"]["children"][0]["children"]
            .as_array_mut()
            .unwrap();
        children.push(json!({"id":"poly","name":"Diamond","type":"REGULAR_POLYGON","pointCount":4,"absoluteBoundingBox":{"x":100,"y":50,"width":20,"height":20},"fills":[{"type":"SOLID","color":{"r":1,"g":0,"b":0,"a":1}}]}));
        children.push(json!({"id":"star","name":"Star","type":"STAR","pointCount":5,"innerRadius":0.5,"absoluteBoundingBox":{"x":130,"y":50,"width":20,"height":20},"fills":[{"type":"SOLID","color":{"r":0,"g":0,"b":1,"a":1}}]}));
        let bytes = serde_json::to_vec(&f).unwrap();
        let imported = import(&bytes, "shapes.json").unwrap();
        let repeated = import(&bytes, "shapes.json").unwrap();
        let nodes = &imported.pages[0]["layers"][0]["nodes"];
        let again = &repeated.pages[0]["layers"][0]["nodes"];
        let polygon = &nodes[1];
        let star = &nodes[2];
        assert_eq!(polygon["kind"], "path");
        assert_eq!(polygon["name"], "Diamond");
        assert_eq!(polygon["d"].as_str().unwrap().matches(" L ").count(), 3);
        assert!(polygon["d"]
            .as_str()
            .unwrap()
            .starts_with("M 110.000000 50.000000"));
        assert_eq!(star["d"].as_str().unwrap().matches(" L ").count(), 9);
        assert!(star["d"]
            .as_str()
            .unwrap()
            .starts_with("M 140.000000 50.000000"));
        assert_eq!(polygon["id"], again[1]["id"]);
        assert_eq!(star["id"], again[2]["id"]);
        assert!(imported
            .report
            .items
            .iter()
            .any(|item| item.source_id == "poly" && item.status == FidelityStatus::Exact));
        assert!(imported
            .report
            .items
            .iter()
            .any(|item| item.source_id == "star" && item.status == FidelityStatus::Exact));

        let mut document = crate::scene::new_document(200, 100);
        document["pages"][0]["layers"][0]["nodes"] = nodes.clone();
        let document = crate::scene::migrate_to_v5(document).unwrap();
        let scene_svg = crate::image::to_svg(
            &document,
            std::path::Path::new("figma-regular-shapes"),
            None,
        )
        .unwrap();
        let png = crate::render::svg_to_png(&scene_svg.svg, scene_svg.width, scene_svg.height, 1.0)
            .unwrap();
        let pixels = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(pixels.get_pixel(110, 60).0, [255, 0, 0, 255]);
        assert_eq!(pixels.get_pixel(140, 60).0, [0, 0, 255, 255]);
        assert_eq!(pixels.get_pixel(102, 52).0, [255, 255, 255, 255]);
    }

    #[test]
    fn rejects_invalid_or_over_limit_figma_regular_geometry() {
        let mut f = fixture();
        f["document"]["children"][0]["children"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"star","type":"STAR","pointCount":513,"innerRadius":0.5,"absoluteBoundingBox":{"x":0,"y":0,"width":20,"height":20}}));
        let error = import(&serde_json::to_vec(&f).unwrap(), "large-star.json")
            .err()
            .unwrap();
        assert!(error.to_string().contains("limit-exceeded"));

        let mut f = fixture();
        f["document"]["children"][0]["children"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"poly","type":"REGULAR_POLYGON","pointCount":2,"absoluteBoundingBox":{"x":0,"y":0,"width":20,"height":20}}));
        let error = import(&serde_json::to_vec(&f).unwrap(), "invalid-polygon.json")
            .err()
            .unwrap();
        assert!(error.to_string().contains("pointCount must be at least 3"));
    }
}

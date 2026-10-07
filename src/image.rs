//! Version 5 raster asset and image-node validation.
use anyhow::{bail, Context, Result};
use base64::Engine;
use image::ImageDecoder;
use serde_json::{Map, Value};
use std::{collections::HashMap, fmt::Write, io::Cursor, path::Path};

pub const VERSION: u64 = 5;
pub const MAX_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_DOCUMENT_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_DIMENSION: u64 = 32_768;
pub const MAX_PIXELS: u64 = 268_435_456;
pub const MAX_SURFACE_BYTES: u64 = 1_073_741_824;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceInfo {
    pub digest: String,
    pub media_type: String,
    pub byte_length: u64,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub color_space: &'static str,
    pub orientation: u8,
    pub has_alpha: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Fit {
    Fill,
    Contain,
    Cover,
    None,
    ScaleDown,
}

impl Fit {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::Contain => "contain",
            Self::Cover => "cover",
            Self::None => "none",
            Self::ScaleDown => "scale-down",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AddResult {
    pub id: String,
    pub asset: String,
    pub deduplicated: bool,
    pub source: SourceInfo,
}

#[derive(Debug, Default)]
pub struct Update {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub fit: Option<Fit>,
    pub position: Option<[f64; 2]>,
    pub crop: Option<[f64; 4]>,
    pub opacity: Option<f64>,
    pub transform: Option<[f64; 6]>,
    pub mask: Option<(String, String)>,
    pub clear_mask: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn add(
    raw: &mut Value,
    page_id: Option<&str>,
    layer_id: &str,
    id: &str,
    bytes: &[u8],
    storage: Value,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    fit: Fit,
) -> Result<AddResult> {
    let source = decode_source(bytes)?;
    let mut migrated = if crate::composite::is_document(raw) {
        crate::scene::validate(raw)?;
        raw.clone()
    } else {
        crate::scene::migrate_to_v5(raw.clone())?
    };
    let assets = migrated["image_assets"]
        .as_object_mut()
        .context("[malformed-resource] v5 image_assets must be an object")?;
    let deduplicated = assets.contains_key(&source.digest);
    assets.entry(source.digest.clone()).or_insert_with(|| {
        serde_json::json!({
            "media_type": source.media_type.clone(),
            "byte_length": source.byte_length,
            "pixel_width": source.pixel_width,
            "pixel_height": source.pixel_height,
            "color_space": source.color_space,
            "orientation": source.orientation,
            "storage": storage
        })
    });
    let pages = migrated
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .context("v5 document has no pages")?;
    let page = if let Some(page_id) = page_id {
        pages
            .iter_mut()
            .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
            .with_context(|| format!("page not found: {page_id}"))?
    } else {
        pages.first_mut().context("document has no pages")?
    };
    let layer = page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .and_then(|layers| {
            layers
                .iter_mut()
                .find(|layer| layer.get("id").and_then(Value::as_str) == Some(layer_id))
        })
        .with_context(|| format!("layer not found: {layer_id}"))?;
    if layer["locked"] == true {
        bail!("[locked-node] image target layer {layer_id} is locked; unlock it first")
    }
    layer
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .context("layer nodes are missing")?
        .push(serde_json::json!({
            "kind": "image",
            "id": id,
            "asset": source.digest.clone(),
            "x": x,
            "y": y,
            "width": width,
            "height": height,
            "fit": fit.as_str(),
            "position": [0.5, 0.5],
            "crop": [0, 0, 1, 1],
            "opacity": 1,
            "blend_mode": "normal",
            "transform": [1, 0, 0, 1, 0, 0],
            "operations": []
        }));
    crate::scene::validate(&migrated)?;
    *raw = migrated;
    Ok(AddResult {
        id: id.into(),
        asset: source.digest.clone(),
        deduplicated,
        source,
    })
}

pub fn embedded_storage(bytes: &[u8]) -> Value {
    serde_json::json!({
        "kind": "embedded",
        "encoding": "base64",
        "data": base64::engine::general_purpose::STANDARD.encode(bytes)
    })
}

pub fn external_storage(path: &Path) -> Result<Value> {
    let path = crate::resource::safe_relative_path(path)?;
    let value = path
        .to_str()
        .context("[unsafe-path] image path must be valid UTF-8")?;
    if value.contains('\\') {
        bail!("[unsafe-path] external image paths must use forward slashes")
    }
    Ok(serde_json::json!({"kind":"external", "path":value}))
}

pub fn set(raw: &mut Value, page_id: Option<&str>, id: &str, update: &Update) -> Result<Value> {
    crate::scene::validate(raw)?;
    let page = page_mut(raw, page_id)?;
    let node = find_node_mut(page, id).with_context(|| format!("image node not found: {id}"))?;
    if node.get("kind").and_then(Value::as_str) != Some("image") {
        bail!("object {id} is not an image")
    }
    let object = node.as_object_mut().unwrap();
    for (key, value) in [
        ("x", update.x),
        ("y", update.y),
        ("width", update.width),
        ("height", update.height),
        ("opacity", update.opacity),
    ] {
        if let Some(value) = value {
            object.insert(key.into(), Value::from(value));
        }
    }
    if let Some(fit) = update.fit {
        object.insert("fit".into(), Value::from(fit.as_str()));
    }
    if let Some(position) = update.position {
        object.insert("position".into(), serde_json::json!(position));
    }
    if let Some(crop) = update.crop {
        object.insert("crop".into(), serde_json::json!(crop));
    }
    if let Some(transform) = update.transform {
        object.insert("transform".into(), serde_json::json!(transform));
    }
    if let Some((node, fill_rule)) = &update.mask {
        object.insert(
            "mask".into(),
            serde_json::json!({"node":node,"space":"parent","fill_rule":fill_rule}),
        );
    } else if update.clear_mask {
        object.remove("mask");
    }
    let result = node.clone();
    crate::scene::validate(raw)?;
    Ok(result)
}

pub fn remove(raw: &mut Value, page_id: Option<&str>, id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    fn remove_from(nodes: &mut Vec<Value>, id: &str) -> Option<Value> {
        if let Some(index) = nodes
            .iter()
            .position(|node| node.get("id").and_then(Value::as_str) == Some(id))
        {
            return Some(nodes.remove(index));
        }
        for node in nodes {
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                if let Some(removed) = remove_from(children, id) {
                    return Some(removed);
                }
            }
        }
        None
    }
    let page = page_mut(raw, page_id)?;
    let layers = page["layers"]
        .as_array_mut()
        .context("page layers are missing")?;
    let removed = layers
        .iter_mut()
        .find_map(|layer| remove_from(layer.get_mut("nodes")?.as_array_mut()?, id))
        .with_context(|| format!("image node not found: {id}"))?;
    if removed.get("kind").and_then(Value::as_str) != Some("image") {
        bail!("object {id} is not an image")
    }
    let digest = removed
        .get("asset")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !document_references_asset(raw, &digest) {
        raw.get_mut("image_assets")
            .and_then(Value::as_object_mut)
            .map(|assets| assets.remove(&digest));
    }
    crate::scene::validate(raw)?;
    Ok(
        serde_json::json!({"removed":id,"asset":digest,"asset_pruned":!document_references_asset(raw, &digest)}),
    )
}

fn page_mut<'a>(raw: &'a mut Value, page_id: Option<&str>) -> Result<&'a mut Value> {
    let pages = raw
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .context("document has no pages")?;
    if let Some(page_id) = page_id {
        pages
            .iter_mut()
            .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
            .with_context(|| format!("page not found: {page_id}"))
    } else {
        pages.first_mut().context("document has no pages")
    }
}

fn find_node_mut<'a>(page: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    fn find<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
        for node in nodes {
            if node.get("id").and_then(Value::as_str) == Some(id) {
                return Some(node);
            }
            if let Some(found) = node
                .get_mut("children")
                .and_then(Value::as_array_mut)
                .and_then(|children| find(children, id))
            {
                return Some(found);
            }
        }
        None
    }
    page.get_mut("layers")?
        .as_array_mut()?
        .iter_mut()
        .find_map(|layer| find(layer.get_mut("nodes")?.as_array_mut()?, id))
}

pub(crate) fn document_references_asset(raw: &Value, digest: &str) -> bool {
    fn references(value: &Value, digest: &str) -> bool {
        match value {
            Value::Object(object) => {
                object.get("kind").and_then(Value::as_str) == Some("image")
                    && object.get("asset").and_then(Value::as_str) == Some(digest)
                    || object.values().any(|value| references(value, digest))
            }
            Value::Array(values) => values.iter().any(|value| references(value, digest)),
            _ => false,
        }
    }
    raw.get("pages")
        .is_some_and(|pages| references(pages, digest))
}

pub fn info(raw: &Value, page_id: Option<&str>, id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    fn find<'a>(nodes: &'a [Value], id: &str) -> Option<&'a Value> {
        for node in nodes {
            if node.get("id").and_then(Value::as_str) == Some(id) {
                return Some(node);
            }
            if let Some(found) = node
                .get("children")
                .and_then(Value::as_array)
                .and_then(|children| find(children, id))
            {
                return Some(found);
            }
        }
        None
    }
    let pages = raw
        .get("pages")
        .and_then(Value::as_array)
        .context("document has no pages")?;
    let page = if let Some(page_id) = page_id {
        pages
            .iter()
            .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
            .with_context(|| format!("page not found: {page_id}"))?
    } else {
        pages.first().context("document has no pages")?
    };
    let node = page
        .get("layers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|layer| find(layer.get("nodes")?.as_array()?, id))
        .with_context(|| format!("image node not found: {id}"))?;
    if node.get("kind").and_then(Value::as_str) != Some("image") {
        bail!("object {id} is not an image")
    }
    let digest = node
        .get("asset")
        .and_then(Value::as_str)
        .context("image asset reference is missing")?;
    let asset = raw
        .get("image_assets")
        .and_then(|assets| assets.get(digest))
        .context("[missing-resource] image asset is missing")?;
    let mut asset = asset.clone();
    if let Some(storage) = asset.get_mut("storage").and_then(Value::as_object_mut) {
        if let Some(data) = storage.remove("data") {
            storage.insert(
                "encoded_length".into(),
                Value::from(data.as_str().map_or(0, str::len)),
            );
        }
    }
    Ok(serde_json::json!({"node":node,"asset_id":digest,"asset":asset}))
}

#[derive(Debug)]
pub struct SceneSvg {
    pub svg: String,
    pub width: u32,
    pub height: u32,
    /// Fonts embedded in the document; rasterizers need them to draw text.
    pub fonts: Vec<crate::document::FontAsset>,
    /// Text lines in canvas coordinates, used for the PDF text layer.
    pub texts: Vec<TextLine>,
}

/// One line of text with its baseline origin and font size.
#[derive(Debug, Clone)]
pub struct TextLine {
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub content: String,
}

pub fn to_svg(raw: &Value, document: &Path, page_id: Option<&str>) -> Result<SceneSvg> {
    to_svg_linked(raw, document, page_id, None)
}

/// A transparent scene fragment for the shared compositing renderer. Source tables
/// are borrowed; referenced mask geometry is resolved against the original page.
pub(crate) fn fragment_svg(
    raw: &Value,
    page: &Value,
    document: &Path,
    node: &Value,
    parent: kurbo::Affine,
) -> Result<SceneSvg> {
    let width = page["canvas"]["width"]
        .as_u64()
        .context("canvas width missing")? as u32;
    let height = page["canvas"]["height"]
        .as_u64()
        .context("canvas height missing")? as u32;
    let context = SvgContext {
        root: crate::resource::document_root(document),
        link_dir: None,
        proxy_edge: None,
        texts: std::cell::RefCell::new(Vec::new()),
    };
    let [a, b, c, d, e, f] = parent.as_coeffs();
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><g transform="matrix({a} {b} {c} {d} {e} {f})">"#
    );
    write_node(&mut svg, node, raw, page, &context, &mut 0)?;
    svg.push_str("</g></svg>");
    Ok(SceneSvg {
        svg,
        width,
        height,
        fonts: raw
            .get("fonts")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default(),
        texts: context.texts.take(),
    })
}

/// Portable SVG by default. With `link_dir` (the directory that will hold the
/// SVG), external sources are referenced by a verified, relative `href` instead
/// of being embedded. Linked mode refuses anything that cannot be expressed as a
/// plain `<image>` of the original bytes, so a viewer never shows other pixels.
pub fn to_svg_linked(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    link_dir: Option<&Path>,
) -> Result<SceneSvg> {
    build_svg(raw, document, page_id, link_dir, None)
}

/// Approximate editor preview: embedded pixels are reduced so that no edge
/// exceeds `max_edge` (16–4096). Placement and geometry are unchanged; only the
/// pixel detail differs, so this must never be used for authoritative export.
pub fn to_svg_proxy(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    max_edge: u32,
) -> Result<SceneSvg> {
    if !(16..=4096).contains(&max_edge) {
        bail!("[invalid-operation] preview max_edge must be between 16 and 4096")
    }
    build_svg(raw, document, page_id, None, Some(max_edge))
}

fn build_svg(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    link_dir: Option<&Path>,
    proxy_edge: Option<u32>,
) -> Result<SceneSvg> {
    crate::scene::validate(raw)?;
    if raw.get("version").and_then(Value::as_u64) != Some(VERSION) {
        bail!("image rendering requires a version 5 document")
    }
    let resolved = crate::scene::with_resolved_tokens(raw)?;
    let raw = &resolved;
    let pages = raw["pages"].as_array().context("document has no pages")?;
    let page = if let Some(page_id) = page_id {
        pages
            .iter()
            .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
            .with_context(|| format!("page not found: {page_id}"))?
    } else {
        pages.first().context("document has no pages")?
    };
    let canvas = page["canvas"]
        .as_object()
        .context("page canvas is missing")?;
    let width = integer(canvas, "width")?;
    let height = integer(canvas, "height")?;
    let width = u32::try_from(width).context("canvas width is too large")?;
    let height = u32::try_from(height).context("canvas height is too large")?;
    let background = string(canvas, "background")?;
    let mut svg = String::new();
    write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="{}"/>"#,
        escape(background)
    )?;
    let root = &SvgContext {
        root: crate::resource::document_root(document),
        link_dir,
        proxy_edge,
        texts: std::cell::RefCell::new(Vec::new()),
    };
    let mut clip_index = 0usize;
    for layer in page["layers"]
        .as_array()
        .context("page layers are missing")?
    {
        if !layer
            .get("visible")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            continue;
        }
        write!(
            svg,
            r#"<g id="{}">"#,
            escape(layer.get("id").and_then(Value::as_str).unwrap_or("layer"))
        )?;
        for node in layer["nodes"]
            .as_array()
            .context("layer nodes are missing")?
        {
            write_node(&mut svg, node, raw, page, root, &mut clip_index)?;
        }
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    let fonts = raw
        .get("fonts")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("[malformed-resource] document fonts are invalid")?
        .unwrap_or_default();
    Ok(SceneSvg {
        svg,
        width,
        height,
        fonts,
        texts: root.texts.take(),
    })
}

struct SvgContext<'a> {
    root: &'a Path,
    link_dir: Option<&'a Path>,
    proxy_edge: Option<u32>,
    texts: std::cell::RefCell<Vec<TextLine>>,
}

fn write_node(
    svg: &mut String,
    node: &Value,
    raw: &Value,
    page: &Value,
    root: &SvgContext,
    clip_index: &mut usize,
) -> Result<()> {
    let object = node.as_object().context("node must be an object")?;
    if object.contains_key("clip") {
        bail!("[unsupported-capability] general scene clipping is not implemented for v5 export")
    }
    let transform = object
        .get("transform")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_f64().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "1 0 0 1 0 0".into());
    let kind = string(object, "kind")?;
    let mask_wrapper = if kind == "image" {
        object
            .get("mask")
            .map(|mask| -> Result<usize> {
                let mask = mask.as_object().context("image mask must be an object")?;
                let target_id = string(mask, "node")?;
                let target = find_page_node(page, target_id).with_context(|| {
                    format!("[missing-mask] mask node {target_id} was not found")
                })?;
                let index = *clip_index;
                *clip_index += 1;
                write!(
                    svg,
                    r#"<defs><clipPath id="vector-mask-{index}" clipPathUnits="userSpaceOnUse">"#
                )?;
                write_mask_geometry(svg, target, string(mask, "fill_rule")?)?;
                write!(
                    svg,
                    r#"</clipPath></defs><g clip-path="url(#vector-mask-{index})">"#
                )?;
                Ok(index)
            })
            .transpose()?
    } else {
        None
    };
    write!(svg, r#"<g transform="matrix({transform})">"#)?;
    match kind {
        "group" => {
            for child in object["children"]
                .as_array()
                .context("group children are missing")?
            {
                write_node(svg, child, raw, page, root, clip_index)?;
            }
        }
        "instance" => write_node(
            svg,
            object
                .get("fallback")
                .context("instance fallback is missing")?,
            raw,
            page,
            root,
            clip_index,
        )?,
        "path" => write!(
            svg,
            r#"<path d="{}" fill="{}" stroke="{}" stroke-width="{}"/>"#,
            escape(string(object, "d")?),
            escape(style_string(object, "fill", "none")),
            escape(style_string(object, "stroke", "none")),
            style_number(object, "stroke_width", 0.0)
        )?,
        "rect" => write!(
            svg,
            r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" ry="{}" fill="{}" stroke="{}" stroke-width="{}"/>"#,
            finite(object, "x")?,
            finite(object, "y")?,
            finite(object, "width")?,
            finite(object, "height")?,
            object
                .get("radius_x")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            object
                .get("radius_y")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            escape(style_string(object, "fill", "none")),
            escape(style_string(object, "stroke", "none")),
            style_number(object, "stroke_width", 0.0)
        )?,
        "ellipse" => write!(
            svg,
            r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="{}" stroke="{}" stroke-width="{}"/>"#,
            finite(object, "cx")?,
            finite(object, "cy")?,
            finite(object, "radius_x")?,
            finite(object, "radius_y")?,
            escape(style_string(object, "fill", "none")),
            escape(style_string(object, "stroke", "none")),
            style_number(object, "stroke_width", 0.0)
        )?,
        "line" => write!(
            svg,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}"/>"#,
            finite(object, "x1")?,
            finite(object, "y1")?,
            finite(object, "x2")?,
            finite(object, "y2")?,
            escape(style_string(object, "stroke", "none")),
            style_number(object, "stroke_width", 0.0)
        )?,
        "text" => write_text(svg, object, root)?,
        "image" => write_image(svg, object, raw, root, clip_index)?,
        kind => bail!("[unsupported-capability] cannot render node kind {kind}"),
    }
    svg.push_str("</g>");
    if mask_wrapper.is_some() {
        svg.push_str("</g>");
    }
    Ok(())
}

fn find_page_node<'a>(page: &'a Value, id: &str) -> Option<&'a Value> {
    fn find<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        node.get("children")
            .and_then(Value::as_array)
            .and_then(|children| children.iter().find_map(|child| find(child, id)))
    }
    page.get("layers")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|layer| layer.get("nodes").and_then(Value::as_array))
        .flat_map(|nodes| nodes.iter())
        .find_map(|node| find(node, id))
}

fn write_mask_geometry(svg: &mut String, node: &Value, fill_rule: &str) -> Result<()> {
    let object = node.as_object().context("mask node must be an object")?;
    let transform = object
        .get("transform")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_f64)
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "1 0 0 1 0 0".into());
    write!(svg, r#"<g transform="matrix({transform})">"#)?;
    match string(object, "kind")? {
        "group" => {
            for child in object["children"]
                .as_array()
                .context("mask group children are missing")?
            {
                write_mask_geometry(svg, child, fill_rule)?;
            }
        }
        "path" => write!(
            svg,
            r#"<path d="{}" fill-rule="{}"/>"#,
            escape(string(object, "d")?),
            fill_rule
        )?,
        "rect" => write!(
            svg,
            r#"<rect x="{}" y="{}" width="{}" height="{}"/>"#,
            finite(object, "x")?,
            finite(object, "y")?,
            finite(object, "width")?,
            finite(object, "height")?
        )?,
        "ellipse" => write!(
            svg,
            r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}"/>"#,
            finite(object, "cx")?,
            finite(object, "cy")?,
            finite(object, "radius_x")?,
            finite(object, "radius_y")?
        )?,
        kind => bail!("[invalid-mask] node kind {kind} cannot be a mask"),
    }
    svg.push_str("</g>");
    Ok(())
}

fn write_image(
    svg: &mut String,
    node: &Map<String, Value>,
    raw: &Value,
    context: &SvgContext,
    clip_index: &mut usize,
) -> Result<()> {
    let digest = string(node, "asset")?;
    let asset = raw["image_assets"]
        .get(digest)
        .and_then(Value::as_object)
        .context("[missing-resource] image asset is missing")?;
    let root = context.root;
    let bytes = load_asset_bytes(raw, root, digest)?;
    let link = linked_href(context, node, asset, digest)?;
    let (source, pixels, processed) = resolve_pixels(node, asset, root, digest, &bytes)?;
    let x = finite(node, "x")?;
    let y = finite(node, "y")?;
    let frame_w = finite(node, "width")?;
    let frame_h = finite(node, "height")?;
    let crop = if processed {
        vec![0.0, 0.0, 1.0, 1.0]
    } else {
        normalized_array(node, "crop", 4, true)?
    };
    let position = normalized_array(node, "position", 2, false)?;
    let source_w = f64::from(source.pixel_width);
    let source_h = f64::from(source.pixel_height);
    let cropped_w = source_w * crop[2];
    let cropped_h = source_h * crop[3];
    let contain = (frame_w / cropped_w).min(frame_h / cropped_h);
    let cover = (frame_w / cropped_w).max(frame_h / cropped_h);
    let (scale_x, scale_y) = match string(node, "fit")? {
        "fill" => (frame_w / cropped_w, frame_h / cropped_h),
        "contain" => (contain, contain),
        "cover" => (cover, cover),
        "none" => (1.0, 1.0),
        "scale-down" => {
            let scale = contain.min(1.0);
            (scale, scale)
        }
        _ => unreachable!(),
    };
    let visible_w = cropped_w * scale_x;
    let visible_h = cropped_h * scale_y;
    let visible_x = x + (frame_w - visible_w) * position[0];
    let visible_y = y + (frame_h - visible_h) * position[1];
    let image_x = visible_x - source_w * crop[0] * scale_x;
    let image_y = visible_y - source_h * crop[1] * scale_y;
    let image_w = source_w * scale_x;
    let image_h = source_h * scale_y;
    let clip = *clip_index;
    *clip_index += 1;
    let href = match link {
        Some(href) => format!("{} data-sha256=\"{digest}\"", escape_href(&href)),
        None => {
            let pixels = match context.proxy_edge {
                Some(edge) if pixels.width().max(pixels.height()) > edge => {
                    let ratio = f64::from(edge) / f64::from(pixels.width().max(pixels.height()));
                    let w = ((f64::from(pixels.width()) * ratio).round() as u32).max(1);
                    let h = ((f64::from(pixels.height()) * ratio).round() as u32).max(1);
                    image::imageops::resize(&pixels, w, h, image::imageops::FilterType::Triangle)
                }
                _ => pixels,
            };
            let mut normalized = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(pixels)
                .write_to(&mut normalized, image::ImageFormat::Png)
                .context("could not encode normalized image pixels")?;
            format!(
                "href=\"data:image/png;base64,{}\"",
                base64::engine::general_purpose::STANDARD.encode(normalized.into_inner())
            )
        }
    };
    write!(
        svg,
        r#"<defs><clipPath id="image-clip-{clip}"><rect x="{x}" y="{y}" width="{frame_w}" height="{frame_h}"/></clipPath></defs><image x="{image_x}" y="{image_y}" width="{image_w}" height="{image_h}" opacity="{}" clip-path="url(#image-clip-{clip})" preserveAspectRatio="none" {href}/>"#,
        finite(node, "opacity")?,
    )?;
    Ok(())
}

/// Emit a text node with the same typography attributes as the non-image renderer.
fn write_text(svg: &mut String, object: &Map<String, Value>, root: &SvgContext) -> Result<()> {
    let x = finite(object, "x")?;
    let y = finite(object, "y")?;
    let size = object
        .get("font_size")
        .and_then(Value::as_f64)
        .unwrap_or(16.0);
    let weight = object
        .get("font_weight")
        .and_then(Value::as_u64)
        .unwrap_or(400);
    let italic = object
        .get("italic")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let anchor = match object.get("align").and_then(Value::as_str) {
        Some("center") => "middle",
        Some("right") => "end",
        _ => "start",
    };
    // A text box's `x` is its left edge; centred and right-aligned lines anchor inside its width.
    let box_width = object
        .get("width")
        .and_then(Value::as_f64)
        .filter(|w| w.is_finite() && *w > 0.0)
        .unwrap_or(0.0);
    let x = match anchor {
        "middle" => x + box_width / 2.0,
        "end" => x + box_width,
        _ => x,
    };
    let spacing = object
        .get("letter_spacing")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let line_height = object
        .get("line_height")
        .and_then(Value::as_f64)
        .unwrap_or(1.2);
    write!(
        svg,
        r#"<text xml:space="preserve" font-family="{}" font-size="{size}" font-weight="{weight}" font-style="{}" text-anchor="{anchor}" letter-spacing="{spacing}" fill="{}">"#,
        escape(
            object
                .get("font_family")
                .and_then(Value::as_str)
                .unwrap_or("sans-serif")
        ),
        if italic { "italic" } else { "normal" },
        escape(style_string(object, "fill", "#111827"))
    )?;
    for (index, line) in string(object, "content")?.lines().enumerate() {
        let line_y = y + index as f64 * size * line_height;
        root.texts.borrow_mut().push(TextLine {
            x,
            y: line_y,
            size,
            content: line.to_string(),
        });
        write!(svg, r#"<tspan x="{x}" y="{line_y}">"#)?;
        if anchor == "start" {
            crate::render::push_text_runs(svg, line, line_y);
        } else {
            svg.push_str(&escape(line));
        }
        svg.push_str("</tspan>");
    }
    svg.push_str("</text>");
    Ok(())
}

fn escape_href(href: &str) -> String {
    format!("href=\"{}\"", escape(href))
}

/// Resolve the relative reference for linked-SVG mode, or `None` for embedded
/// output. Fails closed for any node whose pixels differ from the source file.
fn linked_href(
    context: &SvgContext,
    node: &Map<String, Value>,
    asset: &Map<String, Value>,
    digest: &str,
) -> Result<Option<String>> {
    let Some(link_dir) = context.link_dir else {
        return Ok(None);
    };
    let storage = asset["storage"]
        .as_object()
        .context("image storage is missing")?;
    if string(storage, "kind")? != "external" {
        return Ok(None);
    }
    let id = node.get("id").and_then(Value::as_str).unwrap_or("image");
    let crop = normalized_array(node, "crop", 4, true)?;
    if has_enabled_operation(node)
        || crop != [0.0, 0.0, 1.0, 1.0]
        || integer(asset, "orientation")? != 1
    {
        bail!("[unsupported-capability] linked SVG cannot reference {id}: it has a crop, operations, or EXIF orientation; run `image bake` or export without --link-images")
    }
    let relative = crate::resource::safe_relative_path(Path::new(string(storage, "path")?))?;
    let target = context
        .root
        .join(&relative)
        .canonicalize()
        .with_context(|| format!("[missing-resource] linked image {}", relative.display()))?;
    let from = link_dir
        .canonicalize()
        .with_context(|| format!("[missing-resource] SVG directory {}", link_dir.display()))?;
    let _ = digest;
    let href = relative_link(&from, &target)?;
    Ok(Some(href))
}

fn percent_encode(part: &str) -> String {
    part.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Forward-slash relative path from directory `from` to file `to` (both absolute).
fn relative_link(from: &Path, to: &Path) -> Result<String> {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if common == 0 {
        bail!("[unsafe-path] linked image and SVG share no common root")
    }
    let mut parts: Vec<String> = vec!["..".into(); from.len() - common];
    for component in &to[common..] {
        let part = component
            .as_os_str()
            .to_str()
            .context("[unsafe-path] non-UTF-8 image path")?;
        parts.push(percent_encode(part));
    }
    Ok(parts.join("/"))
}

fn style_string<'a>(object: &'a Map<String, Value>, key: &str, default: &'a str) -> &'a str {
    object
        .get("style")
        .and_then(|style| style.get(key))
        .and_then(|value| value.get("fallback"))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

fn style_number(object: &Map<String, Value>, key: &str, default: f64) -> f64 {
    object
        .get("style")
        .and_then(|style| style.get(key))
        .and_then(|value| value.get("fallback"))
        .and_then(Value::as_f64)
        .unwrap_or(default)
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn decode_source(bytes: &[u8]) -> Result<SourceInfo> {
    decode_source_pixels(bytes).map(|(info, _)| info)
}

pub(crate) fn decode_source_pixels(bytes: &[u8]) -> Result<(SourceInfo, image::RgbaImage)> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_SOURCE_BYTES {
        bail!("[limit-exceeded] image source must contain 1 byte–128 MiB")
    }
    let format =
        image::guess_format(bytes).context("[malformed-resource] unknown image signature")?;
    let (media_type, animation_marker) = match format {
        image::ImageFormat::Png => ("image/png", Some(b"acTL".as_slice())),
        image::ImageFormat::Jpeg => ("image/jpeg", None),
        image::ImageFormat::WebP => ("image/webp", Some(b"ANIM".as_slice())),
        other => bail!("[unsupported-capability] image format {other:?} is unsupported"),
    };
    if animation_marker.is_some_and(|marker| bytes.windows(marker.len()).any(|part| part == marker))
    {
        bail!("[unsupported-capability] animated images require explicit frame selection")
    }
    let reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut decoder = reader
        .into_decoder()
        .context("[malformed-resource] could not initialize image decoder")?;
    let (encoded_width, encoded_height) = decoder.dimensions();
    validate_surface(encoded_width as u64, encoded_height as u64)?;
    let orientation = decoder
        .orientation()
        .context("[malformed-resource] invalid image orientation metadata")?;
    let mut decoded = image::DynamicImage::from_decoder(decoder)
        .context("[malformed-resource] image decode failed")?;
    decoded.apply_orientation(orientation);
    let width = decoded.width();
    let height = decoded.height();
    validate_surface(width as u64, height as u64)?;
    let has_alpha = decoded.color().has_alpha();
    let rgba = decoded.into_rgba8();
    if rgba.width() != width || rgba.height() != height {
        bail!("[malformed-resource] decoded image dimensions changed unexpectedly")
    }
    let info = SourceInfo {
        digest: crate::resource::sha256(bytes),
        media_type: media_type.into(),
        byte_length: bytes.len() as u64,
        pixel_width: width,
        pixel_height: height,
        color_space: "srgb8",
        orientation: orientation.to_exif(),
        has_alpha,
    };
    Ok((info, rgba))
}

pub fn validate_surface(width: u64, height: u64) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        bail!("[limit-exceeded] image dimensions are outside 1–32768")
    }
    let pixels = width
        .checked_mul(height)
        .context("[limit-exceeded] image pixel count overflow")?;
    let bytes = pixels
        .checked_mul(4)
        .context("[limit-exceeded] image surface byte count overflow")?;
    if pixels > MAX_PIXELS || bytes > MAX_SURFACE_BYTES {
        bail!("[limit-exceeded] decoded image surface is too large")
    }
    Ok(())
}

pub fn validate_assets(raw: &Value) -> Result<HashMap<String, (u64, u64)>> {
    let assets = raw
        .get("image_assets")
        .and_then(Value::as_object)
        .context("[malformed-resource] v5 image_assets must be an object")?;
    let mut total = 0u64;
    let mut ids = HashMap::with_capacity(assets.len());
    for (digest, value) in assets {
        validate_digest(digest)?;
        let asset = value.as_object().with_context(|| {
            format!("[malformed-resource] image asset {digest} must be an object")
        })?;
        let byte_length = integer(asset, "byte_length")?;
        if byte_length == 0 || byte_length > MAX_SOURCE_BYTES {
            bail!("[limit-exceeded] image asset {digest} exceeds the 128 MiB source limit")
        }
        let width = integer(asset, "pixel_width")?;
        let height = integer(asset, "pixel_height")?;
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            bail!("[limit-exceeded] image asset {digest} dimensions are outside 1–32768")
        }
        let pixels = width
            .checked_mul(height)
            .context("[limit-exceeded] image pixel count overflow")?;
        let surface = pixels
            .checked_mul(4)
            .context("[limit-exceeded] image surface byte count overflow")?;
        if pixels > MAX_PIXELS || surface > MAX_SURFACE_BYTES {
            bail!("[limit-exceeded] image asset {digest} decoded surface is too large")
        }
        match string(asset, "media_type")? {
            "image/png" | "image/jpeg" | "image/webp" => {}
            other => bail!("[unsupported-capability] unsupported image media type {other}"),
        }
        if string(asset, "color_space")? != "srgb8" {
            bail!("[unsupported-capability] image asset {digest} is not srgb8")
        }
        if !(1..=8).contains(&integer(asset, "orientation")?) {
            bail!("[malformed-resource] image asset {digest} orientation must be 1–8")
        }
        let storage = asset
            .get("storage")
            .and_then(Value::as_object)
            .with_context(|| {
                format!("[malformed-resource] image asset {digest} storage is missing")
            })?;
        match string(storage, "kind")? {
            "embedded" => {
                total = total
                    .checked_add(byte_length)
                    .context("[limit-exceeded] image source byte total overflow")?;
                if total > MAX_DOCUMENT_SOURCE_BYTES {
                    bail!(
                        "[limit-exceeded] embedded image sources exceed the 512 MiB document limit"
                    )
                }
                if string(storage, "encoding")? != "base64" {
                    bail!("[unsupported-capability] image asset {digest} encoding is unsupported")
                }
                let encoded = string(storage, "data")?;
                let maximum_encoded = byte_length
                    .checked_add(2)
                    .and_then(|n| n.checked_div(3))
                    .and_then(|n| n.checked_mul(4))
                    .context("[limit-exceeded] base64 size overflow")?;
                if encoded.len() as u64 > maximum_encoded {
                    bail!("[limit-exceeded] image asset {digest} encoded data exceeds its declared size")
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .with_context(|| {
                        format!("[malformed-resource] image asset {digest} has invalid base64")
                    })?;
                if bytes.len() as u64 != byte_length {
                    bail!("[malformed-resource] image asset {digest} byte_length does not match embedded data")
                }
                crate::resource::verify(&bytes, digest)?;
                let decoded = decode_source(&bytes)?;
                if decoded.media_type != string(asset, "media_type")?
                    || u64::from(decoded.pixel_width) != width
                    || u64::from(decoded.pixel_height) != height
                    || u64::from(decoded.orientation) != integer(asset, "orientation")?
                {
                    bail!("[malformed-resource] image asset {digest} metadata does not match decoded source")
                }
            }
            "external" => {
                crate::resource::safe_relative_path(Path::new(string(storage, "path")?))?;
            }
            other => bail!("[unsupported-capability] image storage kind {other} is unsupported"),
        }
        ids.insert(digest.clone(), (width, height));
    }
    Ok(ids)
}

pub fn validate_node(
    node: &Map<String, Value>,
    assets: &HashMap<String, (u64, u64)>,
) -> Result<()> {
    validate_node_for_version(node, assets, false)
}

pub(crate) fn validate_node_for_version(
    node: &Map<String, Value>,
    assets: &HashMap<String, (u64, u64)>,
    compositing: bool,
) -> Result<()> {
    let id = string(node, "id")?;
    let asset = string(node, "asset")?;
    validate_digest(asset)?;
    let (source_width, source_height) = *assets.get(asset).with_context(|| {
        format!("[missing-resource] image node {id} references missing asset {asset}")
    })?;
    for key in ["x", "y"] {
        finite(node, key)?;
    }
    for key in ["width", "height"] {
        if finite(node, key)? <= 0.0 {
            bail!("[malformed-resource] image node {id} {key} must be positive")
        }
    }
    match string(node, "fit")? {
        "fill" | "contain" | "cover" | "none" | "scale-down" => {}
        other => bail!("[unsupported-capability] image node {id} fit {other} is unsupported"),
    }
    normalized_array(node, "position", 2, false)?;
    let crop = normalized_array(node, "crop", 4, true)?;
    if crop[0] + crop[2] > 1.0 || crop[1] + crop[3] > 1.0 {
        bail!("[malformed-resource] image node {id} crop must remain inside the source")
    }
    let opacity = finite(node, "opacity")?;
    if !(0.0..=1.0).contains(&opacity) {
        bail!("[malformed-resource] image node {id} opacity must be in 0–1")
    }
    if !compositing && string(node, "blend_mode")? != "normal" {
        bail!("[unsupported-capability] image node {id} blend mode is unsupported")
    }
    let operations = node
        .get("operations")
        .and_then(Value::as_array)
        .context("[malformed-resource] image operations must be an array")?;
    crate::imageops::validate_stack(
        operations,
        u32::try_from(source_width).unwrap_or(u32::MAX),
        u32::try_from(source_height).unwrap_or(u32::MAX),
    )
    .with_context(|| format!("image node {id}"))?;
    Ok(())
}

pub fn validate_masks(page: &Value) -> Result<()> {
    validate_masks_for_version(page, false)
}

pub(crate) fn validate_masks_for_version(page: &Value, compositing: bool) -> Result<()> {
    fn collect<'a>(node: &'a Value, nodes: &mut HashMap<&'a str, &'a Value>) {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            nodes.insert(id, node);
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                collect(child, nodes);
            }
        }
    }
    fn check(node: &Value, nodes: &HashMap<&str, &Value>, compositing: bool) -> Result<()> {
        if node.get("kind").and_then(Value::as_str) == Some("image") {
            if let Some(mask) = node.get("mask") {
                if compositing && mask.get("resource").is_some() {
                    return Ok(());
                }
                let mask = mask
                    .as_object()
                    .context("[malformed-resource] image mask must be an object")?;
                let image_id = node.get("id").and_then(Value::as_str).unwrap_or("image");
                let target_id = string(mask, "node")?;
                if target_id == image_id {
                    bail!("[invalid-mask] image {image_id} cannot mask itself")
                }
                if string(mask, "space")? != "parent" {
                    bail!("[unsupported-capability] image mask space must be parent")
                }
                if !matches!(string(mask, "fill_rule")?, "nonzero" | "evenodd") {
                    bail!("[malformed-resource] image mask fill_rule must be nonzero or evenodd")
                }
                let target = nodes.get(target_id).with_context(|| {
                    format!("[missing-mask] image {image_id} references missing mask {target_id}")
                })?;
                if !matches!(
                    target.get("kind").and_then(Value::as_str),
                    Some("path" | "rect" | "ellipse" | "group")
                ) {
                    bail!("[invalid-mask] image {image_id} mask {target_id} is not a vector node")
                }
            }
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                check(child, nodes, compositing)?;
            }
        }
        Ok(())
    }
    let mut nodes = HashMap::new();
    for layer in page["layers"]
        .as_array()
        .context("page layers are missing")?
    {
        for node in layer["nodes"]
            .as_array()
            .context("layer nodes are missing")?
        {
            collect(node, &mut nodes);
        }
    }
    for node in nodes.values() {
        check(node, &nodes, compositing)?;
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    let hex = value
        .strip_prefix("sha256:")
        .context("[malformed-resource] image digest must start with sha256:")?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("[malformed-resource] image digest must use 64 lowercase hexadecimal digits")
    }
    Ok(())
}

fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("[malformed-resource] {key} must be a string"))
}

fn integer(object: &Map<String, Value>, key: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("[malformed-resource] {key} must be a nonnegative integer"))
}

fn finite(object: &Map<String, Value>, key: &str) -> Result<f64> {
    let value = object
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("[malformed-resource] {key} must be a number"))?;
    if !value.is_finite() {
        bail!("[malformed-resource] {key} must be finite")
    }
    Ok(value)
}

fn normalized_array(
    object: &Map<String, Value>,
    key: &str,
    length: usize,
    positive_tail: bool,
) -> Result<Vec<f64>> {
    let values = object
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("[malformed-resource] {key} must be an array"))?;
    if values.len() != length {
        bail!("[malformed-resource] {key} must contain {length} numbers")
    }
    let numbers = values
        .iter()
        .map(|value| value.as_f64().filter(|n| n.is_finite()))
        .collect::<Option<Vec<_>>>()
        .with_context(|| format!("[malformed-resource] {key} values must be finite numbers"))?;
    if numbers.iter().any(|value| !(0.0..=1.0).contains(value))
        || (positive_tail && (numbers[2] <= 0.0 || numbers[3] <= 0.0))
    {
        bail!("[malformed-resource] {key} values are outside their normalized range")
    }
    Ok(numbers)
}

/// Read an asset's exact source bytes offline and verify them against `digest`.
/// Verified source bytes, including offline immutable-cache fallback.
pub fn load_asset_bytes_for_document(
    raw: &Value,
    document: &Path,
    digest: &str,
) -> Result<Vec<u8>> {
    load_asset_bytes(raw, crate::resource::document_root(document), digest)
}

pub(crate) fn load_asset_bytes(raw: &Value, document_dir: &Path, digest: &str) -> Result<Vec<u8>> {
    let asset = raw["image_assets"]
        .get(digest)
        .and_then(Value::as_object)
        .context("[missing-resource] image asset is missing")?;
    let storage = asset["storage"]
        .as_object()
        .context("image storage is missing")?;
    let bytes = match string(storage, "kind")? {
        "embedded" => base64::engine::general_purpose::STANDARD
            .decode(string(storage, "data")?)
            .context("[malformed-resource] invalid embedded image base64")?,
        "external" => crate::resource::read_external_offline(
            &document_dir.join("document.pen"),
            Path::new(string(storage, "path")?),
            digest,
        )?,
        kind => bail!("[unsupported-capability] image storage kind {kind} is unsupported"),
    };
    crate::resource::verify(&bytes, digest)?;
    Ok(bytes)
}

fn node_operations(node: &Map<String, Value>) -> &[Value] {
    node.get("operations")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn has_enabled_operation(node: &Map<String, Value>) -> bool {
    node_operations(node)
        .iter()
        .any(|op| op.get("enabled").and_then(Value::as_bool) == Some(true))
}

const PROCESSED_CACHE_MIN_PIXELS: u64 = 65_536;
const PROCESSED_CACHE_MAX_PIXELS: u64 = 16_777_216;
const PROCESSED_CACHE_MAGIC: &[u8; 8] = b"PTPROC01";

/// Decode and process a node's source. Large processed results are served from a
/// checksummed, content-keyed cache when available. The cache is an accelerator
/// only: any miss, corruption, or write failure recomputes identical pixels.
fn resolve_pixels(
    node: &Map<String, Value>,
    asset: &Map<String, Value>,
    root: &Path,
    digest: &str,
    bytes: &[u8],
) -> Result<(SourceInfo, image::RgbaImage, bool)> {
    let key = if has_enabled_operation(node) {
        Some(processed_cache_key(node, digest)?)
    } else {
        None
    };
    if let Some(key) = &key {
        if let Some(image) = read_processed(root, key) {
            let info = SourceInfo {
                digest: digest.to_owned(),
                media_type: string(asset, "media_type")?.to_owned(),
                byte_length: integer(asset, "byte_length")?,
                pixel_width: image.width(),
                pixel_height: image.height(),
                color_space: "srgb8",
                orientation: 1,
                has_alpha: true,
            };
            return Ok((info, image, true));
        }
    }
    let (source, pixels) = decode_source_pixels(bytes)?;
    if source.media_type != string(asset, "media_type")?
        || u64::from(source.pixel_width) != integer(asset, "pixel_width")?
        || u64::from(source.pixel_height) != integer(asset, "pixel_height")?
    {
        bail!("[malformed-resource] decoded image metadata does not match its asset record")
    }
    match process_pixels(node, &pixels)? {
        Some(image) => {
            if let Some(key) = &key {
                write_processed(root, key, &image);
            }
            let info = SourceInfo {
                pixel_width: image.width(),
                pixel_height: image.height(),
                ..source
            };
            Ok((info, image, true))
        }
        None => Ok((source, pixels, false)),
    }
}

fn processed_cache_key(node: &Map<String, Value>, digest: &str) -> Result<String> {
    let operations: Vec<Value> = node_operations(node)
        .iter()
        .filter(|op| op.get("enabled").and_then(Value::as_bool) == Some(true))
        .map(|op| serde_json::json!({"kind":op["kind"],"version":op["version"],"params":op["params"]}))
        .collect();
    let canonical = serde_json::json!({
        "cache": 1,
        "engine": crate::imageops::ENGINE_VERSION,
        "contract": "rgba8-straight-srgb",
        "source": digest,
        "crop": normalized_array(node, "crop", 4, true)?,
        "operations": operations,
    });
    Ok(crate::resource::sha256(&serde_json::to_vec(&canonical)?)
        .trim_start_matches("sha256:")
        .to_owned())
}

fn processed_cache_path(root: &Path, key: &str) -> std::path::PathBuf {
    root.join(".pentool")
        .join("cache")
        .join("processed")
        .join(format!("{key}.pxl"))
}

fn read_processed(root: &Path, key: &str) -> Option<image::RgbaImage> {
    parse_processed_record(&std::fs::read(processed_cache_path(root, key)).ok()?)
}

/// Decode a processed-cache record; any malformed, truncated, or tampered
/// record yields `None` so the caller recomputes instead of trusting it.
pub fn parse_processed_record(data: &[u8]) -> Option<image::RgbaImage> {
    if data.len() < 8 + 8 + 32 || &data[..8] != PROCESSED_CACHE_MAGIC {
        return None;
    }
    let (body, checksum) = data.split_at(data.len() - 32);
    if <sha2::Sha256 as sha2::Digest>::digest(body).as_slice() != checksum {
        return None;
    }
    let width = u32::from_le_bytes(body[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(body[12..16].try_into().ok()?);
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > PROCESSED_CACHE_MAX_PIXELS || body.len() as u64 != 16 + pixels * 4 {
        return None;
    }
    image::RgbaImage::from_raw(width, height, body[16..].to_vec())
}

fn write_processed(root: &Path, key: &str, image: &image::RgbaImage) {
    let pixels = u64::from(image.width()) * u64::from(image.height());
    if !(PROCESSED_CACHE_MIN_PIXELS..=PROCESSED_CACHE_MAX_PIXELS).contains(&pixels) {
        return;
    }
    let mut data = Vec::with_capacity(16 + image.as_raw().len() + 32);
    data.extend_from_slice(PROCESSED_CACHE_MAGIC);
    data.extend_from_slice(&image.width().to_le_bytes());
    data.extend_from_slice(&image.height().to_le_bytes());
    data.extend_from_slice(image.as_raw());
    let checksum = <sha2::Sha256 as sha2::Digest>::digest(&data);
    data.extend_from_slice(&checksum);
    let path = processed_cache_path(root, key);
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&temp, &data).is_err() || std::fs::rename(&temp, &path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

/// Crop (node `crop`) then run enabled operations. `None` means the source
/// pixels are used unchanged, which keeps pre-operation renders byte-identical.
fn process_pixels(
    node: &Map<String, Value>,
    pixels: &image::RgbaImage,
) -> Result<Option<image::RgbaImage>> {
    if !has_enabled_operation(node) {
        return Ok(None);
    }
    let crop = normalized_array(node, "crop", 4, true)?;
    let mut stack = Vec::with_capacity(node_operations(node).len() + 1);
    if crop != [0.0, 0.0, 1.0, 1.0] {
        let params = serde_json::json!({"x":crop[0],"y":crop[1],"width":crop[2],"height":crop[3]});
        stack.push(crate::imageops::new_operation(
            "node-crop",
            "crop",
            params.as_object().cloned().unwrap_or_default(),
            true,
        )?);
    }
    stack.extend(node_operations(node).iter().cloned());
    crate::imageops::apply_stack(pixels.clone(), &stack).map(Some)
}

/// Operation parameters supplied by the CLI or a batch.
#[derive(Debug, Default, Clone)]
pub struct OpParams(pub Map<String, Value>);

fn image_node<'a>(
    raw: &'a mut Value,
    page_id: Option<&str>,
    id: &str,
) -> Result<&'a mut Map<String, Value>> {
    let page = page_mut(raw, page_id)?;
    let node = find_node_mut(page, id).with_context(|| format!("image node not found: {id}"))?;
    if node.get("kind").and_then(Value::as_str) != Some("image") {
        bail!("object {id} is not an image")
    }
    node.as_object_mut().context("image node must be an object")
}

fn stack_mut(node: &mut Map<String, Value>) -> Result<&mut Vec<Value>> {
    node.get_mut("operations")
        .and_then(Value::as_array_mut)
        .context("[malformed-resource] image operations must be an array")
}

fn next_operation_id(stack: &[Value], kind: &str) -> String {
    let mut n = stack.len() + 1;
    loop {
        let id = format!("{kind}-{n}");
        if !stack.iter().any(|op| op["id"] == id.as_str()) {
            return id;
        }
        n += 1;
    }
}

fn position_of(stack: &[Value], op_id: &str, image: &str) -> Result<usize> {
    stack
        .iter()
        .position(|op| op["id"] == op_id)
        .with_context(|| {
            format!("[invalid-operation] operation {op_id} not found on image {image}")
        })
}

/// Add an operation; `index` defaults to the end of the stack.
pub fn op_add(
    raw: &mut Value,
    page_id: Option<&str>,
    id: &str,
    kind: &str,
    op_id: Option<&str>,
    index: Option<usize>,
    params: OpParams,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let stack = stack_mut(image_node(raw, page_id, id)?)?;
    let op_id = op_id.map_or_else(|| next_operation_id(stack, kind), str::to_owned);
    let operation = crate::imageops::new_operation(&op_id, kind, params.0, true)?;
    let at = index.unwrap_or(stack.len());
    if at > stack.len() {
        bail!(
            "[invalid-operation] index {at} is beyond the {} operations",
            stack.len()
        )
    }
    stack.insert(at, operation.clone());
    crate::scene::validate(raw)?;
    Ok(serde_json::json!({"image":id,"index":at,"operation":operation}))
}

/// Merge parameters into an existing operation.
pub fn op_set(
    raw: &mut Value,
    page_id: Option<&str>,
    id: &str,
    op_id: &str,
    params: OpParams,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let stack = stack_mut(image_node(raw, page_id, id)?)?;
    let at = position_of(stack, op_id, id)?;
    for (key, value) in params.0 {
        stack[at]["params"][key] = value;
    }
    let operation = stack[at].clone();
    crate::scene::validate(raw)?;
    Ok(serde_json::json!({"image":id,"index":at,"operation":operation}))
}

pub fn op_move(
    raw: &mut Value,
    page_id: Option<&str>,
    id: &str,
    op_id: &str,
    index: usize,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let stack = stack_mut(image_node(raw, page_id, id)?)?;
    let from = position_of(stack, op_id, id)?;
    if index >= stack.len() {
        bail!(
            "[invalid-operation] index {index} is outside 0–{}",
            stack.len() - 1
        )
    }
    let operation = stack.remove(from);
    stack.insert(index, operation);
    crate::scene::validate(raw)?;
    Ok(serde_json::json!({"image":id,"operation":op_id,"from":from,"to":index}))
}

pub fn op_enable(
    raw: &mut Value,
    page_id: Option<&str>,
    id: &str,
    op_id: &str,
    enabled: bool,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let stack = stack_mut(image_node(raw, page_id, id)?)?;
    let at = position_of(stack, op_id, id)?;
    stack[at]["enabled"] = Value::Bool(enabled);
    crate::scene::validate(raw)?;
    Ok(serde_json::json!({"image":id,"operation":op_id,"enabled":enabled}))
}

pub fn op_remove(raw: &mut Value, page_id: Option<&str>, id: &str, op_id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let stack = stack_mut(image_node(raw, page_id, id)?)?;
    let at = position_of(stack, op_id, id)?;
    let removed = stack.remove(at);
    crate::scene::validate(raw)?;
    Ok(serde_json::json!({"image":id,"removed":removed,"index":at}))
}

pub fn op_list(raw: &Value, page_id: Option<&str>, id: &str) -> Result<Value> {
    let mut copy = raw.clone();
    crate::scene::validate(&copy)?;
    let node = image_node(&mut copy, page_id, id)?;
    let operations = node_operations(node).to_vec();
    Ok(serde_json::json!({
        "image": id,
        "engine_version": crate::imageops::ENGINE_VERSION,
        "count": operations.len(),
        "operations": operations
    }))
}

/// Deterministically flatten crop + enabled operations into a new embedded PNG
/// source. PNG encoding here carries no EXIF/GPS/text chunks by construction.
/// `commit = false` predicts size and hash without mutating `raw`.
pub fn bake(
    raw: &mut Value,
    page_id: Option<&str>,
    document: &Path,
    id: &str,
    commit: bool,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let root = crate::resource::document_root(document);
    let mut working = raw.clone();
    let node = image_node(&mut working, page_id, id)?.clone();
    let digest = string(&node, "asset")?.to_owned();
    let bytes = load_asset_bytes(raw, root, &digest)?;
    let (_, pixels) = decode_source_pixels(&bytes)?;
    let cropped = normalized_array(&node, "crop", 4, true)? != [0.0, 0.0, 1.0, 1.0];
    let baked = match process_pixels(&node, &pixels)? {
        Some(image) => image,
        None if cropped => {
            let stack = [crate::imageops::new_operation(
                "node-crop",
                "crop",
                serde_json::json!({"x":node["crop"][0],"y":node["crop"][1],"width":node["crop"][2],"height":node["crop"][3]})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                true,
            )?];
            crate::imageops::apply_stack(pixels, &stack)?
        }
        None => bail!("[invalid-operation] image {id} has no crop or enabled operations to bake"),
    };
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(baked)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .context("could not encode baked image")?;
    let encoded = encoded.into_inner();
    let info = decode_source(&encoded)?;
    let operations = node_operations(&node).to_vec();
    let report = serde_json::json!({
        "image": id,
        "source": digest,
        "result": info.digest,
        "media_type": info.media_type,
        "pixel_width": info.pixel_width,
        "pixel_height": info.pixel_height,
        "byte_length": info.byte_length,
        "removed_operations": operations.len(),
        "crop_applied": node["crop"],
        "metadata": "stripped",
        "committed": commit
    });
    if !commit {
        return Ok(report);
    }
    let assets = raw["image_assets"]
        .as_object_mut()
        .context("[malformed-resource] v5 image_assets must be an object")?;
    assets.entry(info.digest.clone()).or_insert_with(|| {
        serde_json::json!({
            "media_type": info.media_type,
            "byte_length": info.byte_length,
            "pixel_width": info.pixel_width,
            "pixel_height": info.pixel_height,
            "color_space": info.color_space,
            "orientation": 1,
            "storage": embedded_storage(&encoded),
            "provenance": {
                "kind": "bake",
                "source": digest,
                "engine_version": crate::imageops::ENGINE_VERSION,
                "operations": operations,
                "crop": node["crop"],
                "metadata": "stripped"
            }
        })
    });
    let target = image_node(raw, page_id, id)?;
    target.insert("asset".into(), Value::from(info.digest.clone()));
    target.insert("crop".into(), serde_json::json!([0, 0, 1, 1]));
    target.insert("operations".into(), serde_json::json!([]));
    if !document_references_asset(raw, &digest) {
        if let Some(assets) = raw["image_assets"].as_object_mut() {
            assets.remove(&digest);
        }
    }
    crate::scene::validate(raw)?;
    Ok(report)
}

pub const ANALYSIS_VERSION: u64 = 1;

/// Read-only deterministic analysis: dimensions, alpha, dominant colours from a
/// 4-bit-per-channel histogram, and a chroma-weighted focal suggestion.
pub fn analyze(raw: &Value, page_id: Option<&str>, document: &Path, id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let mut copy = raw.clone();
    let node = image_node(&mut copy, page_id, id)?.clone();
    let digest = string(&node, "asset")?;
    let bytes = load_asset_bytes(raw, crate::resource::document_root(document), digest)?;
    let (info, pixels) = decode_source_pixels(&bytes)?;
    let mut histogram: HashMap<u16, (u64, [u64; 3])> = HashMap::new();
    let (mut sum, mut total, mut sx, mut sy, mut transparent) = (0u64, 0u64, 0f64, 0f64, 0u64);
    // Bound work: sample on a stride so at most ~1M pixels are visited.
    let stride =
        ((u64::from(info.pixel_width) * u64::from(info.pixel_height)) / 1_000_000 + 1) as u32;
    for (x, y, px) in pixels.enumerate_pixels() {
        if x % stride != 0 || y % stride != 0 {
            continue;
        }
        total += 1;
        if px[3] < 128 {
            transparent += 1;
            continue;
        }
        let key =
            (u16::from(px[0] >> 4) << 8) | (u16::from(px[1] >> 4) << 4) | u16::from(px[2] >> 4);
        let entry = histogram.entry(key).or_insert((0, [0; 3]));
        entry.0 += 1;
        for c in 0..3 {
            entry.1[c] += u64::from(px[c]);
        }
        let chroma = px[0].max(px[1]).max(px[2]) - px[0].min(px[1]).min(px[2]);
        let weight = u64::from(chroma) + 1;
        sum += weight;
        sx += weight as f64 * f64::from(x);
        sy += weight as f64 * f64::from(y);
    }
    let mut buckets: Vec<_> = histogram.into_iter().collect();
    buckets.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
    let visible = (total - transparent).max(1);
    let colors: Vec<Value> = buckets
        .iter()
        .take(5)
        .map(|(_, (count, rgb))| {
            serde_json::json!({
                "hex": format!("#{:02x}{:02x}{:02x}", rgb[0] / count, rgb[1] / count, rgb[2] / count),
                "share": ((*count as f64 / visible as f64) * 10_000.0).round() / 10_000.0
            })
        })
        .collect();
    let focal = if sum == 0 {
        [0.5, 0.5]
    } else {
        [
            (sx / sum as f64 / f64::from(info.pixel_width) * 1000.0).round() / 1000.0,
            (sy / sum as f64 / f64::from(info.pixel_height) * 1000.0).round() / 1000.0,
        ]
    };
    Ok(serde_json::json!({
        "image": id,
        "analysis_version": ANALYSIS_VERSION,
        "source": digest,
        "format": info.media_type,
        "pixel_width": info.pixel_width,
        "pixel_height": info.pixel_height,
        "aspect_ratio": f64::from(info.pixel_width) / f64::from(info.pixel_height),
        "has_alpha": info.has_alpha,
        "color_space": info.color_space,
        "orientation": info.orientation,
        "transparent_share": transparent as f64 / total.max(1) as f64,
        "dominant_colors": colors,
        "focal_suggestion": {
            "position": focal,
            "algorithm": "chroma-weighted-centroid",
            "version": 1,
            "confidence": if sum == 0 { 0.0 } else { 0.5 }
        },
        "read_only": true
    }))
}

/// Batch entry point for image operations: creates an image node (with its
/// complete operation stack) or edits one. Works on the caller's candidate copy,
/// so a failure anywhere leaves neither assets nor nodes behind.
pub fn batch_operation(
    raw: &mut Value,
    page_id: Option<&str>,
    kind: &str,
    operation: &Value,
    resolve: &dyn Fn(&str) -> Result<String>,
) -> Result<Value> {
    let text = |key: &str| operation.get(key).and_then(Value::as_str);
    let number = |key: &str| operation.get(key).and_then(Value::as_f64);
    let params_of = |value: &Value| -> OpParams {
        OpParams(
            value
                .get("params")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        )
    };
    match kind {
        "put-image" => {
            let id = resolve("id")?;
            let layer = resolve("layer")?;
            let bytes = if let Some(data) = text("data") {
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .context("[malformed-resource] put-image data is not valid base64")?
            } else if let Some(asset) = text("asset") {
                let data = raw["image_assets"][asset]["storage"]["data"]
                    .as_str()
                    .with_context(|| {
                        format!("[missing-resource] embedded asset {asset} not found")
                    })?;
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .context("[malformed-resource] invalid embedded image base64")?
            } else {
                bail!("put-image requires data (base64) or asset (embedded digest)")
            };
            let fit = match text("fit") {
                Some(fit) => <Fit as clap::ValueEnum>::from_str(fit, true)
                    .map_err(|_| anyhow::anyhow!("[invalid-operation] unsupported fit {fit}"))?,
                None => Fit::Contain,
            };
            let storage = embedded_storage(&bytes);
            let added = add(
                raw,
                page_id,
                &layer,
                &id,
                &bytes,
                storage,
                number("x").unwrap_or(0.0),
                number("y").unwrap_or(0.0),
                number("width").context("put-image width is missing")?,
                number("height").context("put-image height is missing")?,
                fit,
            )?;
            let position = operation.get("position").and_then(Value::as_array);
            let crop = operation.get("crop").and_then(Value::as_array);
            let floats = |values: &[Value]| -> Vec<f64> {
                values.iter().filter_map(Value::as_f64).collect()
            };
            let update = Update {
                position: position
                    .map(|v| floats(v))
                    .filter(|v| v.len() == 2)
                    .map(|v| [v[0], v[1]]),
                crop: crop
                    .map(|v| floats(v))
                    .filter(|v| v.len() == 4)
                    .map(|v| [v[0], v[1], v[2], v[3]]),
                opacity: number("opacity"),
                ..Default::default()
            };
            set(raw, page_id, &id, &update)?;
            let mut stack = 0;
            for (index, op) in operation
                .get("operations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let op_kind = op
                    .get("kind")
                    .and_then(Value::as_str)
                    .with_context(|| format!("operation index {index}: kind is missing"))?;
                op_add(
                    raw,
                    page_id,
                    &id,
                    op_kind,
                    op.get("id").and_then(Value::as_str),
                    None,
                    params_of(op),
                )
                .with_context(|| format!("operation index {index} ({op_kind}) of image {id}"))?;
                if op.get("enabled").and_then(Value::as_bool) == Some(false) {
                    let op_id = raw_last_operation_id(raw, page_id, &id)?;
                    op_enable(raw, page_id, &id, &op_id, false)?;
                }
                stack += 1;
            }
            Ok(
                serde_json::json!({"type":kind,"id":id,"asset":added.asset,"deduplicated":added.deduplicated,"operations":stack}),
            )
        }
        "set-image" => {
            let id = resolve("id")?;
            let fit = match text("fit") {
                Some(fit) => {
                    Some(<Fit as clap::ValueEnum>::from_str(fit, true).map_err(|_| {
                        anyhow::anyhow!("[invalid-operation] unsupported fit {fit}")
                    })?)
                }
                None => None,
            };
            let update = Update {
                x: number("x"),
                y: number("y"),
                width: number("width"),
                height: number("height"),
                fit,
                opacity: number("opacity"),
                position: fixed_array::<2>(operation, "position")?,
                crop: fixed_array::<4>(operation, "crop")?,
                ..Default::default()
            };
            set(raw, page_id, &id, &update)?;
            Ok(serde_json::json!({"type":kind,"id":id}))
        }
        "image-op-add" => {
            let id = resolve("id")?;
            let op_kind = text("op").context("image-op-add requires op (the operation kind)")?;
            op_add(
                raw,
                page_id,
                &id,
                op_kind,
                text("op_id"),
                operation
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize),
                params_of(operation),
            )
        }
        "image-op-set" => {
            let id = resolve("id")?;
            op_set(
                raw,
                page_id,
                &id,
                text("op_id").context("op_id is missing")?,
                params_of(operation),
            )
        }
        "image-op-move" => {
            let id = resolve("id")?;
            op_move(
                raw,
                page_id,
                &id,
                text("op_id").context("op_id is missing")?,
                operation
                    .get("index")
                    .and_then(Value::as_u64)
                    .context("index is missing")? as usize,
            )
        }
        "image-op-enable" | "image-op-disable" => {
            let id = resolve("id")?;
            op_enable(
                raw,
                page_id,
                &id,
                text("op_id").context("op_id is missing")?,
                kind == "image-op-enable",
            )
        }
        "image-op-remove" => {
            let id = resolve("id")?;
            op_remove(
                raw,
                page_id,
                &id,
                text("op_id").context("op_id is missing")?,
            )
        }
        other => bail!("unsupported image batch operation: {other}"),
    }
}

fn raw_last_operation_id(raw: &mut Value, page_id: Option<&str>, id: &str) -> Result<String> {
    let stack = stack_mut(image_node(raw, page_id, id)?)?;
    stack
        .last()
        .and_then(|op| op["id"].as_str())
        .map(str::to_owned)
        .context("operation stack is empty")
}

/// Optional fixed-length numeric array field; any other shape is an error.
fn fixed_array<const N: usize>(operation: &Value, key: &str) -> Result<Option<[f64; N]>> {
    let Some(value) = operation.get(key) else {
        return Ok(None);
    };
    let values: Option<Vec<f64>> = value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_f64).collect());
    match values {
        Some(values) if values.len() == N && value.as_array().map(Vec::len) == Some(N) => {
            let mut out = [0.0; N];
            out.copy_from_slice(&values);
            Ok(Some(out))
        }
        _ => bail!("[invalid-operation] {key} must be an array of {N} numbers"),
    }
}

//! Version 5 raster asset and image-node validation.
use anyhow::{bail, Context, Result};
use base64::Engine;
use image::ImageDecoder;
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
    io::Cursor,
    path::Path,
};

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
    let mut migrated = crate::scene::migrate_to_v5(raw.clone())?;
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

fn document_references_asset(raw: &Value, digest: &str) -> bool {
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
}

pub fn to_svg(raw: &Value, document: &Path, page_id: Option<&str>) -> Result<SceneSvg> {
    crate::scene::validate(raw)?;
    if raw.get("version").and_then(Value::as_u64) != Some(VERSION) {
        bail!("image rendering requires a version 5 document")
    }
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
    let root = document.parent().unwrap_or_else(|| Path::new("."));
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
    Ok(SceneSvg { svg, width, height })
}

fn write_node(
    svg: &mut String,
    node: &Value,
    raw: &Value,
    page: &Value,
    root: &Path,
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
        "text" => write!(
            svg,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}" fill="{}">{}</text>"#,
            finite(object, "x")?,
            finite(object, "y")?,
            escape(
                object
                    .get("font_family")
                    .and_then(Value::as_str)
                    .unwrap_or("sans-serif")
            ),
            object
                .get("font_size")
                .and_then(Value::as_f64)
                .unwrap_or(16.0),
            escape(style_string(object, "fill", "#111827")),
            escape(string(object, "content")?)
        )?,
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
    root: &Path,
    clip_index: &mut usize,
) -> Result<()> {
    let digest = string(node, "asset")?;
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
            &root.join("document.pen"),
            Path::new(string(storage, "path")?),
            digest,
        )?,
        kind => bail!("[unsupported-capability] image storage kind {kind} is unsupported"),
    };
    crate::resource::verify(&bytes, digest)?;
    let (source, pixels) = decode_source_pixels(&bytes)?;
    if source.media_type != string(asset, "media_type")?
        || source.pixel_width as u64 != integer(asset, "pixel_width")?
        || source.pixel_height as u64 != integer(asset, "pixel_height")?
    {
        bail!("[malformed-resource] decoded image metadata does not match its asset record")
    }
    let x = finite(node, "x")?;
    let y = finite(node, "y")?;
    let frame_w = finite(node, "width")?;
    let frame_h = finite(node, "height")?;
    let crop = normalized_array(node, "crop", 4, true)?;
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
    let mut normalized = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut normalized, image::ImageFormat::Png)
        .context("could not encode normalized image pixels")?;
    write!(
        svg,
        r#"<defs><clipPath id="image-clip-{clip}"><rect x="{x}" y="{y}" width="{frame_w}" height="{frame_h}"/></clipPath></defs><image x="{image_x}" y="{image_y}" width="{image_w}" height="{image_h}" opacity="{}" clip-path="url(#image-clip-{clip})" preserveAspectRatio="none" href="data:image/png;base64,{}"/>"#,
        finite(node, "opacity")?,
        base64::engine::general_purpose::STANDARD.encode(normalized.into_inner())
    )?;
    Ok(())
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

fn decode_source_pixels(bytes: &[u8]) -> Result<(SourceInfo, image::RgbaImage)> {
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

fn validate_surface(width: u64, height: u64) -> Result<()> {
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

pub fn validate_assets(raw: &Value) -> Result<HashSet<String>> {
    let assets = raw
        .get("image_assets")
        .and_then(Value::as_object)
        .context("[malformed-resource] v5 image_assets must be an object")?;
    let mut total = 0u64;
    let mut ids = HashSet::with_capacity(assets.len());
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
        ids.insert(digest.clone());
    }
    Ok(ids)
}

pub fn validate_node(node: &Map<String, Value>, assets: &HashSet<String>) -> Result<()> {
    let id = string(node, "id")?;
    let asset = string(node, "asset")?;
    validate_digest(asset)?;
    if !assets.contains(asset) {
        bail!("[missing-resource] image node {id} references missing asset {asset}")
    }
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
    if string(node, "blend_mode")? != "normal" {
        bail!("[unsupported-capability] image node {id} blend mode is unsupported")
    }
    let operations = node
        .get("operations")
        .and_then(Value::as_array)
        .context("[malformed-resource] image operations must be an array")?;
    if !operations.is_empty() {
        bail!("[unsupported-capability] image operations require the v0.7 operation engine")
    }
    Ok(())
}

pub fn validate_masks(page: &Value) -> Result<()> {
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
    fn check(node: &Value, nodes: &HashMap<&str, &Value>) -> Result<()> {
        if node.get("kind").and_then(Value::as_str) == Some("image") {
            if let Some(mask) = node.get("mask") {
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
                check(child, nodes)?;
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
        check(node, &nodes)?;
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

//! Transactional composition of one `.pen` document into another.
use crate::{
    document::{Document, FontAsset, Layer},
    editing, geometry,
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImportOptions {
    pub destination_page: Option<String>,
    pub source_page: Option<String>,
    pub prefix: Option<String>,
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub rotation: f64,
    pub expand_canvas: bool,
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub document: Value,
    pub summary: Value,
}

pub fn compose(
    mut destination_raw: Value,
    source_raw: Value,
    options: &ImportOptions,
) -> Result<ImportResult> {
    if !options.x.is_finite()
        || !options.y.is_finite()
        || !options.scale.is_finite()
        || options.scale <= 0.0
        || !options.rotation.is_finite()
    {
        bail!("import placement must use finite coordinates, positive scale, and finite rotation");
    }
    let mut destination: Document = serde_json::from_value(destination_raw.clone())?;
    let mut source: Document = serde_json::from_value(source_raw.clone())?;
    destination.validate().map_err(anyhow::Error::msg)?;
    source.validate().map_err(anyhow::Error::msg)?;
    if let Some(page) = &options.destination_page {
        destination.select_page(page).map_err(anyhow::Error::msg)?;
    }
    if let Some(page) = &options.source_page {
        source.select_page(page).map_err(anyhow::Error::msg)?;
    }

    let prefix = options.prefix.as_deref().unwrap_or("");
    if prefix.contains(char::is_whitespace) {
        bail!("import prefix cannot contain whitespace");
    }
    let qualify = |id: &str| {
        if prefix.is_empty() {
            id.to_owned()
        } else {
            format!("{prefix}-{id}")
        }
    };
    let existing_layers: HashSet<_> = destination.layers.iter().map(|l| l.id.clone()).collect();
    let mut layer_map = HashMap::new();
    let mut object_map = Vec::new();
    let mut imported = Vec::with_capacity(source.layers.len());
    for source_layer in &source.layers {
        let mut layer = source_layer.clone();
        let new_layer_id = qualify(&layer.id);
        if existing_layers.contains(&new_layer_id)
            || imported.iter().any(|l: &Layer| l.id == new_layer_id)
        {
            bail!("layer ID collision: {new_layer_id}; use a unique --prefix");
        }
        layer_map.insert(layer.id.clone(), new_layer_id.clone());
        layer.id = new_layer_id;
        let mut ids = HashSet::new();
        for path in &mut layer.paths {
            let old = path.id.clone();
            path.id = qualify(&path.id);
            if !ids.insert(path.id.clone()) {
                bail!("object ID collision after prefixing: {}", path.id);
            }
            object_map.push(json!({"layer":source_layer.id,"id":old,"new_layer":layer.id,"new_id":path.id,"kind":"path"}));
        }
        for text in &mut layer.texts {
            let old = text.id.clone();
            text.id = qualify(&text.id);
            if !ids.insert(text.id.clone()) {
                bail!("object ID collision after prefixing: {}", text.id);
            }
            object_map.push(json!({"layer":source_layer.id,"id":old,"new_layer":layer.id,"new_id":text.id,"kind":"text"}));
        }
        imported.push(layer);
    }

    let mut font_map = Vec::new();
    for font in &source.fonts {
        if let Some(existing) = destination.fonts.iter().find(|f| f.data == font.data) {
            font_map.push(json!({"id":font.id,"new_id":existing.id,"deduplicated":true}));
            continue;
        }
        let mut new_id = qualify(&font.id);
        if destination.fonts.iter().any(|f| f.id == new_id) {
            let base = new_id.clone();
            let mut n = 2;
            while destination.fonts.iter().any(|f| f.id == new_id) {
                new_id = format!("{base}-{n}");
                n += 1;
            }
        }
        destination.fonts.push(FontAsset {
            id: new_id.clone(),
            data: font.data.clone(),
        });
        font_map.push(json!({"id":font.id,"new_id":new_id,"deduplicated":false}));
    }

    let start = destination.layers.len();
    destination.layers.extend(imported);
    for index in start..destination.layers.len() {
        let locked = destination.layers[index].locked;
        destination.layers[index].locked = false;
        let id = destination.layers[index].id.clone();
        if options.scale != 1.0 {
            geometry::execute_layer(
                &mut destination,
                &id,
                &geometry::Operation::Scale {
                    sx: options.scale,
                    sy: options.scale,
                    cx: 0.0,
                    cy: 0.0,
                },
            )?;
        }
        if options.rotation != 0.0 {
            geometry::execute_layer(
                &mut destination,
                &id,
                &geometry::Operation::Rotate {
                    degrees: options.rotation,
                    cx: 0.0,
                    cy: 0.0,
                },
            )?;
        }
        if options.x != 0.0 || options.y != 0.0 {
            geometry::execute_layer(
                &mut destination,
                &id,
                &geometry::Operation::Translate {
                    dx: options.x,
                    dy: options.y,
                },
            )?;
        }
        destination.layers[index].locked = locked;
    }

    if options.expand_canvas {
        let mut width = destination.canvas.width as f64;
        let mut height = destination.canvas.height as f64;
        let objects: Vec<_> = destination
            .layers
            .iter()
            .skip(start)
            .flat_map(|layer| {
                layer
                    .paths
                    .iter()
                    .map(|p| &p.id)
                    .chain(layer.texts.iter().map(|t| &t.id))
                    .map(|id| (layer.id.clone(), id.clone()))
            })
            .collect();
        for (layer, id) in objects {
            let bounds =
                geometry::execute(&mut destination, &layer, &id, &geometry::Operation::Bounds)?;
            width = width
                .max(bounds["x"].as_f64().unwrap_or(0.0) + bounds["width"].as_f64().unwrap_or(0.0));
            height = height.max(
                bounds["y"].as_f64().unwrap_or(0.0) + bounds["height"].as_f64().unwrap_or(0.0),
            );
        }
        destination.canvas.width = width.ceil().clamp(1.0, 16384.0) as u32;
        destination.canvas.height = height.ceil().clamp(1.0, 16384.0) as u32;
    }
    destination.version = 3;
    destination.validate().map_err(anyhow::Error::msg)?;

    // Seed imported layers from the source JSON so unknown extension fields travel
    // with the copied artwork, then merge transformed known fields over them.
    let source_layers = raw_layers(&source_raw, source.active_page_id())?;
    editing::merge(&mut destination_raw, serde_json::to_value(&destination)?);
    if let Some(map) = destination_raw.as_object_mut() {
        map.remove("canvas");
        map.remove("layers");
    }
    let destination_layers = raw_layers_mut(&mut destination_raw, destination.active_page_id())?;
    for (index, raw_layer) in source_layers.into_iter().enumerate() {
        let mut raw_layer = raw_layer;
        rename_raw_layer(&mut raw_layer, &layer_map, &qualify)?;
        editing::merge(
            &mut raw_layer,
            serde_json::to_value(&destination.layers[start + index])?,
        );
        destination_layers[start + index] = raw_layer;
    }
    let verified: Document = serde_json::from_value(destination_raw.clone())?;
    verified.validate().map_err(anyhow::Error::msg)?;
    Ok(ImportResult {
        document: destination_raw,
        summary: json!({
            "destination_page": destination.active_page_id(),
            "source_page": source.active_page_id(),
            "layers": layer_map,
            "objects": object_map,
            "fonts": font_map,
            "placement":{"x":options.x,"y":options.y,"scale":options.scale,"rotation":options.rotation},
            "canvas":{"width":destination.canvas.width,"height":destination.canvas.height}
        }),
    })
}

fn raw_layers(raw: &Value, page: &str) -> Result<Vec<Value>> {
    let root = if raw.get("version").and_then(Value::as_u64) == Some(3) {
        raw.get("pages")
            .and_then(Value::as_array)
            .and_then(|pages| {
                pages
                    .iter()
                    .find(|p| p.get("id").and_then(Value::as_str) == Some(page))
            })
            .context("source page missing from raw document")?
    } else {
        raw
    };
    root.get("layers")
        .and_then(Value::as_array)
        .cloned()
        .context("source layers missing from raw document")
}

fn raw_layers_mut<'a>(raw: &'a mut Value, page: &str) -> Result<&'a mut Vec<Value>> {
    if raw.get("version").and_then(Value::as_u64) == Some(3) {
        raw.get_mut("pages")
            .and_then(Value::as_array_mut)
            .and_then(|pages| {
                pages
                    .iter_mut()
                    .find(|p| p.get("id").and_then(Value::as_str) == Some(page))
            })
            .and_then(|page| page.get_mut("layers"))
            .and_then(Value::as_array_mut)
            .context("destination page layers missing from raw document")
    } else {
        raw.get_mut("layers")
            .and_then(Value::as_array_mut)
            .context("destination layers missing from raw document")
    }
}

fn rename_raw_layer(
    layer: &mut Value,
    layer_map: &HashMap<String, String>,
    qualify: &impl Fn(&str) -> String,
) -> Result<()> {
    let old = layer
        .get("id")
        .and_then(Value::as_str)
        .context("raw layer has no ID")?;
    layer["id"] = Value::String(layer_map.get(old).context("layer mapping missing")?.clone());
    for key in ["paths", "texts"] {
        if let Some(objects) = layer.get_mut(key).and_then(Value::as_array_mut) {
            for object in objects {
                let id = object
                    .get("id")
                    .and_then(Value::as_str)
                    .context("raw object has no ID")?;
                object["id"] = Value::String(qualify(id));
            }
        }
    }
    Ok(())
}

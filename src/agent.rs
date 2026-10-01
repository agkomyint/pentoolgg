//! Agent-oriented discovery and transactional object editing.
use crate::{
    document::{Document, StrokeCap, StrokeJoin, TextAlign},
    geometry::{self, Operation},
};
use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ObjectKind {
    Path,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize, Subcommand)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ObjectAction {
    /// Change only supplied properties, preserving everything else.
    Set {
        id: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        d: Option<String>,
        #[arg(long)]
        stroke: Option<String>,
        #[arg(long)]
        width: Option<f32>,
        #[arg(long)]
        fill: Option<String>,
        #[arg(long, value_enum)]
        cap: Option<StrokeCap>,
        #[arg(long, value_enum)]
        join: Option<StrokeJoin>,
        #[arg(long)]
        miter_limit: Option<f32>,
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        x: Option<f64>,
        #[arg(long)]
        y: Option<f64>,
        #[arg(long)]
        font: Option<String>,
        #[arg(long)]
        size: Option<f64>,
        #[arg(long)]
        weight: Option<u16>,
        #[arg(long)]
        italic: Option<bool>,
        #[arg(long, value_enum)]
        align: Option<TextAlign>,
        #[arg(long)]
        letter_spacing: Option<f64>,
        #[arg(long)]
        line_height: Option<f64>,
    },
    Rename {
        id: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        new_id: String,
    },
    Duplicate {
        id: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        new_id: String,
    },
    MoveToLayer {
        id: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        target_layer: String,
    },
    /// Reorder within its path or text stack; zero is back.
    Reorder {
        id: String,
        #[arg(long)]
        layer: String,
        index: usize,
    },
    Remove {
        id: String,
        #[arg(long)]
        layer: String,
    },
}

fn layer_index(doc: &Document, id: &str) -> Result<usize> {
    doc.layers
        .iter()
        .position(|l| l.id == id)
        .context("layer not found")
}
fn locate(doc: &Document, layer: usize, id: &str) -> Result<(ObjectKind, usize)> {
    let l = &doc.layers[layer];
    if let Some(i) = l.paths.iter().position(|o| o.id == id) {
        return Ok((ObjectKind::Path, i));
    }
    if let Some(i) = l.texts.iter().position(|o| o.id == id) {
        return Ok((ObjectKind::Text, i));
    }
    bail!("object not found")
}
fn id_available(doc: &Document, layer: usize, id: &str) -> bool {
    !doc.layers[layer].paths.iter().any(|o| o.id == id)
        && !doc.layers[layer].texts.iter().any(|o| o.id == id)
}

pub fn apply(doc: &mut Document, action: &ObjectAction) -> Result<Value> {
    let original = doc.clone();
    let result = (|| -> Result<Value> {
        let source_id = match action {
            ObjectAction::Set { id, .. }
            | ObjectAction::Rename { id, .. }
            | ObjectAction::Duplicate { id, .. }
            | ObjectAction::MoveToLayer { id, .. }
            | ObjectAction::Reorder { id, .. }
            | ObjectAction::Remove { id, .. } => id,
        };
        let layer_id = match action {
            ObjectAction::Set { layer, .. }
            | ObjectAction::Rename { layer, .. }
            | ObjectAction::Duplicate { layer, .. }
            | ObjectAction::MoveToLayer { layer, .. }
            | ObjectAction::Reorder { layer, .. }
            | ObjectAction::Remove { layer, .. } => layer,
        };
        let li = layer_index(doc, layer_id)?;
        if doc.layers[li].locked {
            bail!("layer is locked");
        }
        let (kind, index) = locate(doc, li, source_id)?;
        match action {
            ObjectAction::Set {
                d,
                stroke,
                width,
                fill,
                cap,
                join,
                miter_limit,
                content,
                x,
                y,
                font,
                size,
                weight,
                italic,
                align,
                letter_spacing,
                line_height,
                ..
            } => {
                match kind {
                    ObjectKind::Path => {
                        if [
                            content.as_ref().map(|_| ()),
                            x.as_ref().map(|_| ()),
                            y.as_ref().map(|_| ()),
                            font.as_ref().map(|_| ()),
                            size.as_ref().map(|_| ()),
                            weight.as_ref().map(|_| ()),
                            italic.as_ref().map(|_| ()),
                            align.as_ref().map(|_| ()),
                            letter_spacing.as_ref().map(|_| ()),
                            line_height.as_ref().map(|_| ()),
                        ]
                        .iter()
                        .any(Option::is_some)
                        {
                            bail!("text properties cannot be applied to a path");
                        }
                        let p = &mut doc.layers[li].paths[index];
                        if let Some(v) = d {
                            for segment in svgtypes::PathParser::from(v.as_str()) {
                                segment.context("invalid SVG path data")?;
                            }
                            p.d = v.clone();
                            p.closed = v.trim_end().ends_with(['z', 'Z']);
                        }
                        if let Some(v) = stroke {
                            p.stroke = v.clone();
                        }
                        if let Some(v) = width {
                            p.stroke_width = *v;
                        }
                        if let Some(v) = fill {
                            p.fill = v.clone();
                        }
                        if let Some(v) = cap {
                            p.stroke_linecap = *v;
                        }
                        if let Some(v) = join {
                            p.stroke_linejoin = *v;
                        }
                        if let Some(v) = miter_limit {
                            p.stroke_miterlimit = *v;
                        }
                    }
                    ObjectKind::Text => {
                        if d.is_some()
                            || stroke.is_some()
                            || width.is_some()
                            || cap.is_some()
                            || join.is_some()
                            || miter_limit.is_some()
                        {
                            bail!("path properties cannot be applied to text");
                        }
                        let t = &mut doc.layers[li].texts[index];
                        if let Some(v) = content {
                            t.content = v.clone();
                        }
                        if let Some(v) = x {
                            t.x = *v;
                        }
                        if let Some(v) = y {
                            t.y = *v;
                        }
                        if let Some(v) = font {
                            t.font_family = v.clone();
                        }
                        if let Some(v) = size {
                            t.font_size = *v;
                        }
                        if let Some(v) = weight {
                            t.font_weight = *v;
                        }
                        if let Some(v) = italic {
                            t.italic = *v;
                        }
                        if let Some(v) = fill {
                            t.fill = v.clone();
                        }
                        if let Some(v) = align {
                            t.align = *v;
                        }
                        if let Some(v) = letter_spacing {
                            t.letter_spacing = *v;
                        }
                        if let Some(v) = line_height {
                            t.line_height = *v;
                        }
                    }
                }
                Ok(json!({"operation":"set","layer":layer_id,"id":source_id,"kind":kind}))
            }
            ObjectAction::Rename { new_id, .. } => {
                if new_id.is_empty() {
                    bail!("new ID cannot be empty");
                }
                if !id_available(doc, li, new_id) {
                    bail!("new object ID already exists in layer");
                }
                match kind {
                    ObjectKind::Path => doc.layers[li].paths[index].id = new_id.clone(),
                    ObjectKind::Text => doc.layers[li].texts[index].id = new_id.clone(),
                }
                Ok(
                    json!({"operation":"rename","layer":layer_id,"id":source_id,"new_id":new_id,"kind":kind}),
                )
            }
            ObjectAction::Duplicate { new_id, .. } => {
                if new_id.is_empty() {
                    bail!("new ID cannot be empty");
                }
                if !id_available(doc, li, new_id) {
                    bail!("new object ID already exists in layer");
                }
                match kind {
                    ObjectKind::Path => {
                        let mut o = doc.layers[li].paths[index].clone();
                        o.id = new_id.clone();
                        doc.layers[li].paths.insert(index + 1, o)
                    }
                    ObjectKind::Text => {
                        let mut o = doc.layers[li].texts[index].clone();
                        o.id = new_id.clone();
                        doc.layers[li].texts.insert(index + 1, o)
                    }
                }
                Ok(
                    json!({"operation":"duplicate","layer":layer_id,"id":source_id,"new_id":new_id,"kind":kind}),
                )
            }
            ObjectAction::MoveToLayer { target_layer, .. } => {
                let ti = layer_index(doc, target_layer)?;
                if doc.layers[ti].locked {
                    bail!("target layer is locked");
                }
                if !id_available(doc, ti, source_id) {
                    bail!("object ID already exists in target layer");
                }
                match kind {
                    ObjectKind::Path => {
                        let o = doc.layers[li].paths.remove(index);
                        doc.layers[ti].paths.push(o)
                    }
                    ObjectKind::Text => {
                        let o = doc.layers[li].texts.remove(index);
                        doc.layers[ti].texts.push(o)
                    }
                }
                Ok(
                    json!({"operation":"move-to-layer","layer":layer_id,"target_layer":target_layer,"id":source_id,"kind":kind}),
                )
            }
            ObjectAction::Reorder {
                index: new_index, ..
            } => {
                match kind {
                    ObjectKind::Path => {
                        if *new_index >= doc.layers[li].paths.len() {
                            bail!("path index out of range");
                        }
                        let o = doc.layers[li].paths.remove(index);
                        doc.layers[li].paths.insert(*new_index, o)
                    }
                    ObjectKind::Text => {
                        if *new_index >= doc.layers[li].texts.len() {
                            bail!("text index out of range");
                        }
                        let o = doc.layers[li].texts.remove(index);
                        doc.layers[li].texts.insert(*new_index, o)
                    }
                }
                Ok(
                    json!({"operation":"reorder","layer":layer_id,"id":source_id,"index":new_index,"kind":kind}),
                )
            }
            ObjectAction::Remove { .. } => {
                match kind {
                    ObjectKind::Path => {
                        doc.layers[li].paths.remove(index);
                    }
                    ObjectKind::Text => {
                        doc.layers[li].texts.remove(index);
                    }
                }
                Ok(json!({"operation":"remove","layer":layer_id,"id":source_id,"kind":kind}))
            }
        }
    })();
    match result {
        Ok(summary) => {
            doc.version = 2;
            if let Err(e) = doc.validate() {
                *doc = original;
                bail!(e);
            }
            Ok(summary)
        }
        Err(e) => {
            *doc = original;
            Err(e)
        }
    }
}

pub fn apply_batch(doc: &mut Document, actions: &[ObjectAction]) -> Result<Vec<Value>> {
    if actions.is_empty() {
        bail!("batch must contain at least one operation");
    }
    if actions.len() > 10_000 {
        bail!("batch exceeds 10,000 operations");
    }
    let mut candidate = doc.clone();
    let mut changes = Vec::with_capacity(actions.len());
    for (index, action) in actions.iter().enumerate() {
        changes.push(
            apply(&mut candidate, action).with_context(|| format!("operation {index} failed"))?,
        );
    }
    *doc = candidate;
    Ok(changes)
}

/// Mirror topology changes in raw JSON so unknown extension fields travel with
/// renamed, duplicated, moved, and reordered objects.
pub fn prepare_raw(raw: &mut Value, actions: &[ObjectAction]) -> Result<()> {
    fn layer_mut<'a>(raw: &'a mut Value, id: &str) -> Result<&'a mut Value> {
        raw.get_mut("layers")
            .and_then(Value::as_array_mut)
            .and_then(|a| {
                a.iter_mut()
                    .find(|l| l.get("id").and_then(Value::as_str) == Some(id))
            })
            .context("raw layer not found")
    }
    fn find(layer: &Value, id: &str) -> Result<(&'static str, usize)> {
        for key in ["paths", "texts"] {
            if let Some(i) = layer.get(key).and_then(Value::as_array).and_then(|a| {
                a.iter()
                    .position(|o| o.get("id").and_then(Value::as_str) == Some(id))
            }) {
                return Ok((key, i));
            }
        }
        bail!("raw object not found")
    }
    for action in actions {
        match action {
            ObjectAction::Set { .. } => {}
            ObjectAction::Rename { id, layer, new_id } => {
                let l = layer_mut(raw, layer)?;
                let (key, i) = find(l, id)?;
                l[key][i]["id"] = Value::String(new_id.clone());
            }
            ObjectAction::Duplicate { id, layer, new_id } => {
                let l = layer_mut(raw, layer)?;
                let (key, i) = find(l, id)?;
                let mut v = l[key][i].clone();
                v["id"] = Value::String(new_id.clone());
                l[key].as_array_mut().unwrap().insert(i + 1, v);
            }
            ObjectAction::MoveToLayer {
                id,
                layer,
                target_layer,
            } => {
                let v = {
                    let l = layer_mut(raw, layer)?;
                    let (key, i) = find(l, id)?;
                    l[key].as_array_mut().unwrap().remove(i)
                };
                let target = layer_mut(raw, target_layer)?;
                let key = if v.get("d").is_some() {
                    "paths"
                } else {
                    "texts"
                };
                let map = target
                    .as_object_mut()
                    .context("raw layer is not an object")?;
                map.entry(key)
                    .or_insert_with(|| Value::Array(vec![]))
                    .as_array_mut()
                    .context("raw object array is invalid")?
                    .push(v);
            }
            ObjectAction::Reorder { id, layer, index } => {
                let l = layer_mut(raw, layer)?;
                let (key, i) = find(l, id)?;
                let v = l[key].as_array_mut().unwrap().remove(i);
                l[key].as_array_mut().unwrap().insert(*index, v);
            }
            ObjectAction::Remove { id, layer } => {
                let l = layer_mut(raw, layer)?;
                let (key, i) = find(l, id)?;
                l[key].as_array_mut().unwrap().remove(i);
            }
        }
    }
    Ok(())
}

pub fn merge_document(raw: &mut Value, doc: &Document, actions: &[ObjectAction]) -> Result<()> {
    prepare_raw(raw, actions)?;
    crate::editing::merge(raw, serde_json::to_value(doc)?);
    Ok(())
}

pub fn revision_bytes(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub fn inspect(
    doc: &Document,
    query: Option<&str>,
    kind: Option<ObjectKind>,
    layer_filter: Option<&str>,
) -> Result<Value> {
    let needle = query.unwrap_or("").to_lowercase();
    let mut working = doc.clone();
    let mut layers = Vec::new();
    let mut matches = 0usize;
    for l in &doc.layers {
        if let Some(f) = layer_filter {
            if l.id != f && !l.name.to_lowercase().contains(&f.to_lowercase()) {
                continue;
            }
        }
        let layer_match = needle.is_empty()
            || l.id.to_lowercase().contains(&needle)
            || l.name.to_lowercase().contains(&needle);
        let mut objects = Vec::new();
        if kind != Some(ObjectKind::Text) {
            for (i, p) in l.paths.iter().enumerate() {
                if layer_match || p.id.to_lowercase().contains(&needle) {
                    let b = geometry::execute(&mut working, &l.id, &p.id, &Operation::Bounds)?;
                    objects
                        .push(json!({"id":p.id,"kind":"path","index":i,"draw_order":i,"bounds":b}));
                    matches += 1;
                }
            }
        }
        if kind != Some(ObjectKind::Path) {
            for (i, t) in l.texts.iter().enumerate() {
                if layer_match
                    || t.id.to_lowercase().contains(&needle)
                    || t.content.to_lowercase().contains(&needle)
                {
                    let b = geometry::execute(&mut working, &l.id, &t.id, &Operation::Bounds)?;
                    objects.push(json!({"id":t.id,"kind":"text","index":i,"draw_order":l.paths.len()+i,"content":t.content,"bounds":b}));
                    matches += 1;
                }
            }
        }
        if layer_match || !objects.is_empty() {
            layers.push(json!({"id":l.id,"name":l.name,"index":doc.layers.iter().position(|x|x.id==l.id).unwrap(),"visible":l.visible,"locked":l.locked,"objects":objects}));
        }
    }
    Ok(
        json!({"document":doc.name,"version":doc.version,"matches":matches,"stacking":"layers, then paths, then text; zero is back","layers":layers}),
    )
}

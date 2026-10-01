use crate::document::{Document, Layer, Path};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use std::{fs, path::Path as FilePath};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extensions_follow_ids_when_layers_reorder() {
        let mut old = serde_json::json!({"extra":42,"layers":[{"id":"a","custom":true},{"id":"b","custom":false}]});
        merge(
            &mut old,
            serde_json::json!({"layers":[{"id":"b","name":"B"},{"id":"a","name":"A"}]}),
        );
        assert_eq!(old["extra"], 42);
        assert_eq!(old["layers"][0]["custom"], false);
        assert_eq!(old["layers"][1]["custom"], true);
    }
    #[test]
    fn locked_layers_reject_path_changes() {
        let mut doc = Document::new(100, 100);
        doc.layers[0].locked = true;
        assert!(path(
            &mut doc,
            PathAction::Remove {
                id: "p".into(),
                layer: "layer-1".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("locked"));
    }
}

#[derive(Subcommand)]
pub enum LayerAction {
    Add {
        id: String,
        #[arg(long)]
        name: Option<String>,
    },
    Set {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        visible: Option<bool>,
        #[arg(long)]
        locked: Option<bool>,
    },
    Remove {
        id: String,
    },
    /// Move to a zero-based index; zero is the back.
    Move {
        id: String,
        index: usize,
    },
}

#[derive(Subcommand)]
pub enum PathAction {
    /// Create or replace a path by ID in the specified layer.
    Put {
        id: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        d: String,
        #[arg(long, default_value = "#111827")]
        stroke: String,
        #[arg(long, default_value_t = 3.0)]
        width: f32,
        #[arg(long, default_value = "none")]
        fill: String,
        #[arg(long)]
        closed: bool,
    },
    Remove {
        id: String,
        #[arg(long)]
        layer: String,
    },
}

pub fn edit(file: &FilePath, change: impl FnOnce(&mut Document) -> Result<()>) -> Result<()> {
    // Keep extension fields when modifying the known document schema.
    let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(file)?)?;
    let mut doc: Document = serde_json::from_value(raw.clone())?;
    change(&mut doc)?;
    doc.validate().map_err(anyhow::Error::msg)?;
    let updated = serde_json::to_value(&doc)?;
    merge(&mut raw, updated);
    fs::write(file, serde_json::to_vec_pretty(&raw)?)
        .with_context(|| format!("could not write {}", file.display()))?;
    println!("{}", serde_json::json!({"ok":true,"file":file}));
    Ok(())
}

pub(crate) fn merge(old: &mut serde_json::Value, new: serde_json::Value) {
    match (old, new) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                merge(a.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            let previous = std::mem::take(a);
            *a = b
                .into_iter()
                .map(|v| {
                    let mut original = previous
                        .iter()
                        .find(|o| v.get("id").is_some() && o.get("id") == v.get("id"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    merge(&mut original, v);
                    original
                })
                .collect();
        }
        (a, b) => *a = b,
    }
}

pub fn layer(doc: &mut Document, action: LayerAction) -> Result<()> {
    match action {
        LayerAction::Add { id, name } => {
            if doc.layers.iter().any(|l| l.id == id) {
                bail!("layer ID already exists");
            }
            doc.layers.push(Layer {
                id: id.clone(),
                name: name.unwrap_or(id),
                visible: true,
                locked: false,
                paths: vec![],
            });
        }
        LayerAction::Set {
            id,
            name,
            visible,
            locked,
        } => {
            let l = doc
                .layers
                .iter_mut()
                .find(|l| l.id == id)
                .context("layer not found")?;
            if let Some(v) = name {
                l.name = v;
            }
            if let Some(v) = visible {
                l.visible = v;
            }
            if let Some(v) = locked {
                l.locked = v;
            }
        }
        LayerAction::Remove { id } => {
            let i = doc
                .layers
                .iter()
                .position(|l| l.id == id)
                .context("layer not found")?;
            if doc.layers[i].locked {
                bail!("layer is locked");
            }
            if doc.layers.len() == 1 {
                bail!("cannot remove the last layer");
            }
            doc.layers.remove(i);
        }
        LayerAction::Move { id, index } => {
            if index >= doc.layers.len() {
                bail!("layer index out of range");
            }
            let i = doc
                .layers
                .iter()
                .position(|l| l.id == id)
                .context("layer not found")?;
            let l = doc.layers.remove(i);
            doc.layers.insert(index, l);
        }
    }
    Ok(())
}

pub fn path(doc: &mut Document, action: PathAction) -> Result<()> {
    let layer_id = match &action {
        PathAction::Put { layer, .. } | PathAction::Remove { layer, .. } => layer,
    };
    let l = doc
        .layers
        .iter_mut()
        .find(|l| &l.id == layer_id)
        .context("layer not found")?;
    if l.locked {
        bail!("layer is locked");
    }
    match action {
        PathAction::Put {
            id,
            mut d,
            stroke,
            width,
            fill,
            closed,
            ..
        } => {
            if !width.is_finite() || width < 0.0 {
                bail!("width must be finite and nonnegative");
            }
            if closed && !d.trim_end().ends_with(['z', 'Z']) {
                d.push_str(" Z");
            }
            if d.trim().is_empty() {
                bail!("path data cannot be empty");
            }
            for segment in svgtypes::PathParser::from(d.as_str()) {
                segment.context("invalid SVG path data")?;
            }
            let p = Path {
                id: id.clone(),
                d,
                stroke,
                stroke_width: width,
                fill,
                closed,
            };
            if let Some(existing) = l.paths.iter_mut().find(|p| p.id == id) {
                *existing = p;
            } else {
                l.paths.push(p);
            }
        }
        PathAction::Remove { id, .. } => {
            let i = l
                .paths
                .iter()
                .position(|p| p.id == id)
                .context("path not found")?;
            l.paths.remove(i);
        }
    }
    Ok(())
}

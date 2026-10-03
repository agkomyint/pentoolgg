use crate::document::{Document, Layer, Path, StrokeCap, StrokeJoin};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use std::{fs, path::Path as FilePath};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_removes_last_text_and_font_resources() {
        let doc = Document::new(100, 100);
        let mut raw = serde_json::to_value(&doc).unwrap();
        raw["pages"][0]["layers"][0]["texts"] = serde_json::json!([{"id":"title","content":"old"}]);
        raw["fonts"] = serde_json::json!([{"id":"old","data":"unused"}]);
        raw["extension"] = serde_json::json!(true);
        merge(&mut raw, serde_json::to_value(doc).unwrap());
        assert_eq!(raw["pages"][0]["layers"][0]["texts"], serde_json::json!([]));
        assert_eq!(raw["fonts"], serde_json::json!([]));
        assert_eq!(raw["extension"], true);
    }
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
        #[arg(long, value_enum, default_value = "butt")]
        cap: StrokeCap,
        #[arg(long, value_enum, default_value = "miter")]
        join: StrokeJoin,
        #[arg(long, default_value_t = 4.0)]
        miter_limit: f32,
    },
    /// Change stroke edges without replacing path geometry or colors.
    Style {
        id: String,
        #[arg(long)]
        layer: String,
        #[arg(long, value_enum)]
        cap: Option<StrokeCap>,
        #[arg(long, value_enum)]
        join: Option<StrokeJoin>,
        #[arg(long)]
        miter_limit: Option<f32>,
    },
    Remove {
        id: String,
        #[arg(long)]
        layer: String,
    },
}

pub fn edit(file: &FilePath, change: impl FnOnce(&mut Document) -> Result<()>) -> Result<()> {
    edit_page(file, None, change)
}

pub fn edit_page(
    file: &FilePath,
    page: Option<&str>,
    change: impl FnOnce(&mut Document) -> Result<()>,
) -> Result<()> {
    edit_page_options(file, page, "edit", false, None, change)
}

pub fn edit_page_options(
    file: &FilePath,
    page: Option<&str>,
    operation: &str,
    dry_run: bool,
    expected: Option<&str>,
    change: impl FnOnce(&mut Document) -> Result<()>,
) -> Result<()> {
    // Keep extension fields when modifying the known document schema.
    let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(file)?)?;
    let mut doc: Document = serde_json::from_value(raw.clone())?;
    if let Some(id) = page {
        doc.select_page(id).map_err(anyhow::Error::msg)?;
    }
    change(&mut doc)?;
    doc.validate().map_err(anyhow::Error::msg)?;
    let updated = serde_json::to_value(&doc)?;
    merge(&mut raw, updated);
    if doc.version == 3 {
        if let Some(object) = raw.as_object_mut() {
            object.remove("canvas");
            object.remove("layers");
        }
    }
    let summary = crate::transaction::commit_value(file, operation, dry_run, expected, &raw)?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

/// Write through a sibling file and retain the exact prior file as a numbered
/// recovery snapshot. If the final rename fails, the original is restored.
pub fn transactional_write(file: &FilePath, bytes: &[u8]) -> Result<std::path::PathBuf> {
    let parent = file.parent().unwrap_or_else(|| FilePath::new("."));
    let name = file
        .file_name()
        .context("document path has no filename")?
        .to_string_lossy();
    let temp = parent.join(format!(".{name}.pentool-tmp-{}", std::process::id()));
    let backup = (1..=999)
        .map(|n| parent.join(format!("{name}.bak.{n}")))
        .find(|p| !p.exists())
        .context("too many recovery snapshots")?;
    fs::write(&temp, bytes)
        .with_context(|| format!("could not write temporary file {}", temp.display()))?;
    if let Err(e) = fs::rename(file, &backup) {
        let _ = fs::remove_file(&temp);
        return Err(e).context("could not create recovery snapshot");
    }
    if let Err(e) = fs::rename(&temp, file) {
        let _ = fs::rename(&backup, file);
        let _ = fs::remove_file(&temp);
        return Err(e).context("could not replace document; original restored");
    }
    Ok(backup)
}

/// Atomically replace a document without creating a legacy sibling backup.
pub fn atomic_write(file: &FilePath, bytes: &[u8]) -> Result<()> {
    let parent = file.parent().unwrap_or_else(|| FilePath::new("."));
    let name = file
        .file_name()
        .context("document path has no filename")?
        .to_string_lossy();
    let temp = parent.join(format!(".{name}.pentool-tmp-{}", std::process::id()));
    let previous = parent.join(format!(".{name}.pentool-old-{}", std::process::id()));
    fs::write(&temp, bytes)?;
    if file.exists() {
        fs::rename(file, &previous)?;
    }
    if let Err(error) = fs::rename(&temp, file) {
        if previous.exists() {
            let _ = fs::rename(&previous, file);
        }
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    if previous.exists() {
        fs::remove_file(previous)?;
    }
    Ok(())
}

pub fn merge(old: &mut serde_json::Value, new: serde_json::Value) {
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
                texts: vec![],
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
        PathAction::Put { layer, .. }
        | PathAction::Remove { layer, .. }
        | PathAction::Style { layer, .. } => layer,
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
            cap,
            join,
            miter_limit,
            ..
        } => {
            if l.texts.iter().any(|t| t.id == id) {
                bail!("object ID belongs to text");
            }
            if !width.is_finite() || width < 0.0 {
                bail!("width must be finite and nonnegative");
            }
            if !miter_limit.is_finite() || !(1.0..=1000.0).contains(&miter_limit) {
                bail!("miter limit must be finite and between 1 and 1000");
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
                stroke_linecap: cap,
                stroke_linejoin: join,
                stroke_miterlimit: miter_limit,
                fill,
                closed,
            };
            if let Some(existing) = l.paths.iter_mut().find(|p| p.id == id) {
                *existing = p;
            } else {
                l.paths.push(p);
            }
            doc.version = doc.version.max(2);
        }
        PathAction::Style {
            id,
            cap,
            join,
            miter_limit,
            ..
        } => {
            if let Some(v) = miter_limit {
                if !v.is_finite() || !(1.0..=1000.0).contains(&v) {
                    bail!("miter limit must be finite and between 1 and 1000");
                }
            }
            let p = l
                .paths
                .iter_mut()
                .find(|p| p.id == id)
                .context("path not found")?;
            if let Some(v) = cap {
                p.stroke_linecap = v;
            }
            if let Some(v) = join {
                p.stroke_linejoin = v;
            }
            if let Some(v) = miter_limit {
                p.stroke_miterlimit = v;
            }
            doc.version = doc.version.max(2);
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

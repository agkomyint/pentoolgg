use crate::{
    document::{identity, Document, FontAsset, Text, TextAlign},
    fonts,
};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Subcommand, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TextAction {
    /// Create or replace an editable text object.
    Put {
        id: String,
        #[arg(long)]
        layer: String,
        #[arg(long)]
        content: String,
        #[arg(long, default_value_t = 100.0)]
        #[serde(default = "position")]
        x: f64,
        #[arg(long, default_value_t = 100.0)]
        #[serde(default = "position")]
        y: f64,
        #[arg(long, default_value = "Atkinson Hyperlegible")]
        #[serde(default = "crate::document::default_font")]
        font: String,
        #[arg(long, default_value_t = 48.0)]
        #[serde(default = "crate::document::default_size")]
        size: f64,
        #[arg(long, default_value_t = 400)]
        #[serde(default = "crate::document::default_weight")]
        weight: u16,
        #[arg(long)]
        #[serde(default)]
        italic: bool,
        #[arg(long, default_value = "#111827")]
        #[serde(default = "ink")]
        fill: String,
        #[arg(long, value_enum, default_value = "left")]
        #[serde(default)]
        align: TextAlign,
        #[arg(long, default_value_t = 0.0)]
        #[serde(default)]
        letter_spacing: f64,
        #[arg(long, default_value_t = 1.2)]
        #[serde(default = "crate::document::default_line_height")]
        line_height: f64,
    },
    /// Change only the supplied properties of an existing text object.
    Set {
        id: String,
        #[arg(long)]
        layer: String,
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
        #[arg(long)]
        fill: Option<String>,
        #[arg(long, value_enum)]
        align: Option<TextAlign>,
        #[arg(long)]
        letter_spacing: Option<f64>,
        #[arg(long)]
        line_height: Option<f64>,
    },
    Remove {
        id: String,
        #[arg(long)]
        layer: String,
    },
}
fn position() -> f64 {
    100.0
}
fn ink() -> String {
    "#111827".into()
}

pub fn apply(doc: &mut Document, action: TextAction) -> Result<()> {
    let layer_id = match &action {
        TextAction::Put { layer, .. }
        | TextAction::Set { layer, .. }
        | TextAction::Remove { layer, .. } => layer,
    };
    let layer_index = doc
        .layers
        .iter()
        .position(|l| &l.id == layer_id)
        .context("layer not found")?;
    if doc.layers[layer_index].locked {
        bail!("layer is locked");
    }
    let mut updated = doc.clone();
    let layer = &mut updated.layers[layer_index];
    match action {
        TextAction::Put {
            id,
            content,
            x,
            y,
            font,
            size,
            weight,
            italic,
            fill,
            align,
            letter_spacing,
            line_height,
            ..
        } => {
            if layer.paths.iter().any(|p| p.id == id) {
                bail!("object ID belongs to a path");
            }
            let t = Text {
                id: id.clone(),
                content,
                x,
                y,
                font_family: font,
                font_size: size,
                font_weight: weight,
                italic,
                fill,
                align,
                letter_spacing,
                line_height,
                transform: identity(),
            };
            if let Some(existing) = layer.texts.iter_mut().find(|t| t.id == id) {
                *existing = t;
            } else {
                layer.texts.push(t);
            }
        }
        TextAction::Set {
            id,
            content,
            x,
            y,
            font,
            size,
            weight,
            italic,
            fill,
            align,
            letter_spacing,
            line_height,
            ..
        } => {
            let t = layer
                .texts
                .iter_mut()
                .find(|t| t.id == id)
                .context("text object not found")?;
            if let Some(v) = content {
                t.content = v;
            }
            if let Some(v) = x {
                t.x = v;
            }
            if let Some(v) = y {
                t.y = v;
            }
            if let Some(v) = font {
                t.font_family = v;
            }
            if let Some(v) = size {
                t.font_size = v;
            }
            if let Some(v) = weight {
                t.font_weight = v;
            }
            if let Some(v) = italic {
                t.italic = v;
            }
            if let Some(v) = fill {
                t.fill = v;
            }
            if let Some(v) = align {
                t.align = v;
            }
            if let Some(v) = letter_spacing {
                t.letter_spacing = v;
            }
            if let Some(v) = line_height {
                t.line_height = v;
            }
        }
        TextAction::Remove { id, .. } => {
            let i = layer
                .texts
                .iter()
                .position(|t| t.id == id)
                .context("text object not found")?;
            layer.texts.remove(i);
        }
    }
    updated.version = 2;
    updated.validate().map_err(anyhow::Error::msg)?;
    let db = fonts::database(&updated, true)?;
    for t in &updated.layers[layer_index].texts {
        fonts::ensure_family(&db, &t.font_family, t.font_weight, t.italic)?;
    }
    *doc = updated;
    Ok(())
}

#[derive(Subcommand)]
pub enum FontAction {
    Add {
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
    Remove {
        id: String,
    },
}
pub fn add_font(doc: &mut Document, asset: FontAsset) -> Result<()> {
    fonts::decode(&asset)?;
    if doc.fonts.iter().any(|f| f.id == asset.id) {
        bail!("font ID already exists");
    }
    let mut updated = doc.clone();
    updated.version = 2;
    updated.fonts.push(asset);
    updated.validate().map_err(anyhow::Error::msg)?;
    *doc = updated;
    Ok(())
}
pub fn font(doc: &mut Document, action: FontAction) -> Result<()> {
    match action {
        FontAction::Add { id, file } => add_font(
            doc,
            fonts::asset(
                id,
                fs::read(&file)
                    .with_context(|| format!("could not read font {}", file.display()))?,
            )?,
        ),
        FontAction::Remove { id } => {
            let mut updated = doc.clone();
            let i = updated
                .fonts
                .iter()
                .position(|f| f.id == id)
                .context("font not found")?;
            let mut removed = resvg::usvg::fontdb::Database::new();
            removed.load_font_data(fonts::decode(&updated.fonts[i])?);
            let families: Vec<String> = removed
                .faces()
                .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
                .collect();
            updated.fonts.remove(i);
            // Check with no system fonts, so another machine won't silently lose text.
            let db = fonts::database(&updated, false)?;
            for t in updated.layers.iter().flat_map(|l| &l.texts) {
                if !families
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&t.font_family))
                {
                    continue;
                }
                fonts::ensure_family(&db, &t.font_family, t.font_weight, t.italic)
                    .context("font is still used by text; change its family before removing")?;
            }
            *doc = updated;
            Ok(())
        }
    }
}

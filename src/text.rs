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
    /// Create, edit, list and flow stories (version 9 documents).
    Story {
        #[command(subcommand)]
        action: crate::stories::StoryAction,
    },
    /// Create, edit, thread and unthread text frames (version 9 documents).
    Frame {
        #[command(subcommand)]
        action: crate::stories::FrameAction,
    },
    /// Shape text with the OpenType engine and report glyphs, runs and breaks as JSON.
    Shape(ShapeArgs),
    /// Compose paragraphs into lines (Knuth-Plass or single-line) and report them as JSON.
    Compose(ComposeArgs),
    /// Report the fonts the document requests and how each resolves (disclosing substitution).
    FontReport,
}

#[derive(Debug, clap::Args, Serialize, Deserialize)]
pub struct ComposeArgs {
    /// Text to compose; paragraphs are separated by newlines.
    #[arg(long, conflicts_with = "story")]
    pub text: Option<String>,
    /// Compose the paragraphs of this story (version 9 documents).
    #[arg(long)]
    pub story: Option<String>,
    /// Column width in points.
    #[arg(long)]
    pub width: f64,
    /// Paragraph style from `layout.styles.paragraph` (based_on chains are followed).
    #[arg(long)]
    pub style: Option<String>,
    /// Override the style's font family.
    #[arg(long)]
    pub font: Option<String>,
    #[arg(long, default_value_t = 400)]
    pub weight: u16,
    #[arg(long)]
    pub italic: bool,
    /// Override the style's size in points.
    #[arg(long)]
    pub size: Option<f64>,
    /// Override alignment: left, center, right, justify, justify-last-center, justify-last-right, justify-last-all.
    #[arg(long)]
    pub align: Option<String>,
    /// Override hyphenation (true/false).
    #[arg(long)]
    pub hyphenate: Option<bool>,
    /// Override composer: paragraph or single-line.
    #[arg(long)]
    pub composer: Option<String>,
    /// Override the BCP 47 language.
    #[arg(long)]
    pub language: Option<String>,
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
    /// Maximum line rows returned.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,
}

#[derive(Debug, clap::Args, Serialize, Deserialize)]
pub struct ShapeArgs {
    /// Text to shape; paragraphs are separated by newlines.
    #[arg(long, conflicts_with = "story")]
    pub text: Option<String>,
    /// Shape the text of this story instead (version 9 documents).
    #[arg(long)]
    pub story: Option<String>,
    /// Font family; embedded document fonts are tried before the bundled default.
    #[arg(long, default_value = "Atkinson Hyperlegible")]
    pub font: String,
    #[arg(long, default_value_t = 400)]
    pub weight: u16,
    #[arg(long)]
    pub italic: bool,
    /// Size in points.
    #[arg(long, default_value_t = 12.0)]
    pub size: f64,
    /// OpenType features, e.g. `liga,-kern,smcp,ss01,cv05=2` (names such as `small-caps` also work).
    #[arg(long)]
    pub features: Option<String>,
    /// Variable-font axes, e.g. `wght=700,wdth=90`.
    #[arg(long)]
    pub axes: Option<String>,
    /// BCP 47 language tag, e.g. `tr` or `de-CH`.
    #[arg(long)]
    pub language: Option<String>,
    /// Paragraph direction: auto, ltr or rtl.
    #[arg(long, default_value = "auto")]
    pub direction: String,
    /// Kerning: metrics, optical or none.
    #[arg(long, default_value = "metrics")]
    pub kerning: String,
    /// Tracking in 1/1000 em.
    #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
    pub tracking: i32,
    /// Baseline shift in points (positive raises).
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub baseline_shift: f64,
    /// Horizontal scale in percent.
    #[arg(long, default_value_t = 100.0)]
    pub h_scale: f64,
    /// Vertical scale in percent.
    #[arg(long, default_value_t = 100.0)]
    pub v_scale: f64,
    /// Skew in degrees.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub skew: f64,
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
    /// Maximum glyph rows returned.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
}
fn position() -> f64 {
    100.0
}
fn ink() -> String {
    "#111827".into()
}

/// Enforce the typography ranges export requires (size 0.1–4096, weight
/// 100–900, line height 0.1–10) at write time. Each range is checked only
/// when a value is present.
pub fn validate_typography(
    size: Option<f64>,
    weight: Option<f64>,
    line_height: Option<f64>,
) -> Result<()> {
    if size.is_some_and(|v| !v.is_finite() || !(0.1..=4096.0).contains(&v)) {
        bail!("font size must be finite and between 0.1 and 4096")
    }
    if weight.is_some_and(|v| !v.is_finite() || !(100.0..=900.0).contains(&v)) {
        bail!("font weight must be finite and between 100 and 900")
    }
    if line_height.is_some_and(|v| !v.is_finite() || !(0.1..=10.0).contains(&v)) {
        bail!("line height must be finite and between 0.1 and 10")
    }
    Ok(())
}

pub fn apply(doc: &mut Document, action: TextAction) -> Result<()> {
    if matches!(
        action,
        TextAction::Story { .. }
            | TextAction::Frame { .. }
            | TextAction::Shape { .. }
            | TextAction::Compose { .. }
            | TextAction::FontReport
    ) {
        bail!("[unsupported-version] stories and frames need a version 9 document; run `pentool migrate --target 9` first");
    }
    let layer_id = match &action {
        TextAction::Put { layer, .. }
        | TextAction::Set { layer, .. }
        | TextAction::Remove { layer, .. } => layer,
        TextAction::Story { .. }
        | TextAction::Frame { .. }
        | TextAction::Shape { .. }
        | TextAction::Compose { .. }
        | TextAction::FontReport => unreachable!("rejected above"),
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
                width: None,
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
        TextAction::Story { .. }
        | TextAction::Frame { .. }
        | TextAction::Shape { .. }
        | TextAction::Compose { .. }
        | TextAction::FontReport => unreachable!("rejected above"),
    }
    updated.version = updated.version.max(2);
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
    updated.version = updated.version.max(2);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_typography_enforces_export_ranges() {
        validate_typography(Some(48.0), Some(400.0), Some(1.2)).unwrap();
        validate_typography(None, None, None).unwrap();
        let err = validate_typography(Some(48.0), Some(9999.0), Some(1.2)).unwrap_err();
        assert!(err.to_string().contains("font weight"), "unexpected: {err}");
        let err = validate_typography(Some(48.0), Some(400.0), Some(0.0)).unwrap_err();
        assert!(err.to_string().contains("line height"), "unexpected: {err}");
        let err = validate_typography(Some(99999.0), Some(400.0), Some(1.2)).unwrap_err();
        assert!(err.to_string().contains("font size"), "unexpected: {err}");
    }
}

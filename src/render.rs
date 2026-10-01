use crate::document::Document;
use anyhow::{bail, Context, Result};
use std::{fs, path::Path};

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

pub fn to_svg(doc: &Document) -> Result<String> {
    doc.validate().map_err(anyhow::Error::msg)?;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}"><rect width="100%" height="100%" fill="{}"/>"#,
        doc.canvas.width,
        doc.canvas.height,
        doc.canvas.width,
        doc.canvas.height,
        esc(&doc.canvas.background)
    );
    for layer in doc.layers.iter().filter(|layer| layer.visible) {
        svg.push_str(&format!(r#"<g id="{}">"#, esc(&layer.id)));
        for path in &layer.paths {
            svg.push_str(&format!(
                r#"<path d="{}" stroke="{}" stroke-width="{}" fill="{}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                esc(&path.d), esc(&path.stroke), path.stroke_width.max(0.0), esc(&path.fill)
            ));
        }
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    Ok(svg)
}

pub fn to_png(doc: &Document, scale: f32) -> Result<Vec<u8>> {
    if !(0.1..=8.0).contains(&scale) {
        bail!("scale must be between 0.1 and 8");
    }
    let svg = to_svg(doc)?;
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
        .context("could not parse generated SVG")?;
    let width = (doc.canvas.width as f32 * scale).round() as u32;
    let height = (doc.canvas.height as f32 * scale).round() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).context("image is too large")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().context("could not encode PNG")
}

pub fn write_export(doc: &Document, output: &Path, scale: f32) -> Result<()> {
    match output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => fs::write(output, to_png(doc, scale)?)?,
        Some("svg") => fs::write(output, to_svg(doc)?)?,
        _ => bail!("output must end in .png or .svg"),
    }
    Ok(())
}

use crate::{
    document::{Document, Text},
    fonts,
};
use anyhow::{bail, Context, Result};
use std::{fs, path::Path};

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
    svg.push_str(&fonts::svg_faces(doc)?);
    for layer in doc.layers.iter().filter(|layer| layer.visible) {
        svg.push_str(&format!(r#"<g id="{}">"#, esc(&layer.id)));
        for path in &layer.paths {
            svg.push_str(&format!(
                r#"<path d="{}" stroke="{}" stroke-width="{}" fill="{}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                esc(&path.d), esc(&path.stroke), path.stroke_width.max(0.0), esc(&path.fill)
            ));
        }
        for text in &layer.texts {
            svg.push_str(&text_svg(text));
        }
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    Ok(svg)
}

pub fn text_svg(text: &Text) -> String {
    let matrix = text
        .transform
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let mut svg = format!(
        r#"<text xml:space="preserve" transform="matrix({})" font-family="{}" font-size="{}" font-weight="{}" font-style="{}" text-anchor="{}" letter-spacing="{}" fill="{}">"#,
        matrix,
        esc(&text.font_family),
        text.font_size,
        text.font_weight,
        if text.italic { "italic" } else { "normal" },
        text.align.anchor(),
        text.letter_spacing,
        esc(&text.fill)
    );
    for (index, line) in text
        .content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .enumerate()
    {
        svg.push_str(&format!(
            r#"<tspan x="{}" y="{}">{}</tspan>"#,
            text.x,
            text.y + index as f64 * text.font_size * text.line_height,
            esc(line)
        ));
    }
    svg.push_str("</text>");
    svg
}

pub fn options(doc: &Document) -> Result<resvg::usvg::Options<'static>> {
    if doc.layers.iter().all(|l| l.texts.is_empty()) {
        return Ok(resvg::usvg::Options::default());
    }
    let db = fonts::database(doc, true)?;
    for t in doc
        .layers
        .iter()
        .filter(|l| l.visible)
        .flat_map(|l| &l.texts)
    {
        fonts::ensure_family(&db, &t.font_family, t.font_weight, t.italic)?;
    }
    Ok(resvg::usvg::Options {
        font_family: fonts::DEFAULT_FAMILY.into(),
        fontdb: std::sync::Arc::new(db),
        ..Default::default()
    })
}

pub fn text_bounds(doc: &Document, text: &Text) -> Result<kurbo::Rect> {
    text.validate().map_err(anyhow::Error::msg)?;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}"><g id="measure">{}</g></svg>"#,
        doc.canvas.width,
        doc.canvas.height,
        text_svg(text)
    );
    let opt = options(doc)?;
    fonts::ensure_family(
        &opt.fontdb,
        &text.font_family,
        text.font_weight,
        text.italic,
    )?;
    let tree = resvg::usvg::Tree::from_str(&svg, &opt)?;
    if let Some(node) = tree.node_by_id("measure") {
        let r = node.abs_bounding_box();
        Ok(kurbo::Rect::new(
            f64::from(r.x()),
            f64::from(r.y()),
            f64::from(r.right()),
            f64::from(r.bottom()),
        ))
    } else {
        let p = kurbo::Affine::new(text.transform) * kurbo::Point::new(text.x, text.y);
        Ok(kurbo::Rect::new(p.x, p.y, p.x, p.y))
    }
}

pub fn to_svg_outlined(doc: &Document) -> Result<String> {
    let svg = to_svg(doc)?;
    let tree = resvg::usvg::Tree::from_str(&svg, &options(doc)?)?;
    Ok(tree.to_string(&resvg::usvg::WriteOptions::default()))
}

pub fn to_png(doc: &Document, scale: f32) -> Result<Vec<u8>> {
    if !(0.1..=8.0).contains(&scale) {
        bail!("scale must be between 0.1 and 8");
    }
    let svg = to_svg(doc)?;
    let tree = resvg::usvg::Tree::from_str(&svg, &options(doc)?)
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
    write_export_options(doc, output, scale, false)
}
pub fn write_export_options(
    doc: &Document,
    output: &Path,
    scale: f32,
    outline_text: bool,
) -> Result<()> {
    match output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => fs::write(output, to_png(doc, scale)?)?,
        Some("svg") => fs::write(
            output,
            if outline_text {
                to_svg_outlined(doc)?
            } else {
                to_svg(doc)?
            },
        )?,
        _ => bail!("output must end in .png or .svg"),
    }
    Ok(())
}

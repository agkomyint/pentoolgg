use crate::{
    document::{Document, Text},
    fonts,
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{fmt::Write, fs, path::Path, time::Instant};

const MAX_RENDER_PIXELS: u64 = 268_435_456;
const MAX_PIXMAP_BYTES: u64 = 1_073_741_824;

#[derive(Debug, Clone, Serialize)]
pub struct RenderTimings {
    pub svg_construction_us: u128,
    pub font_preparation_us: u128,
    pub usvg_parse_us: u128,
    pub rasterization_us: u128,
    pub png_encoding_us: u128,
}

#[derive(Debug)]
pub struct ProfiledPng {
    pub bytes: Vec<u8>,
    pub timings: RenderTimings,
    pub width: u32,
    pub height: u32,
    pub pixmap_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderLimitError {
    pub width: u32,
    pub height: u32,
    pub pixels: u64,
    pub row_bytes: u64,
    pub pixmap_bytes: u64,
    pub max_pixels: u64,
    pub max_pixmap_bytes: u64,
}
impl std::fmt::Display for RenderLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,"render request is unsafe: {}x{} ({} pixels, {} row bytes, {} RGBA bytes); limits are {} pixels and {} bytes",self.width,self.height,self.pixels,self.row_bytes,self.pixmap_bytes,self.max_pixels,self.max_pixmap_bytes)
    }
}
impl std::error::Error for RenderLimitError {}

/// Symbol blocks that most text fonts lack (arrows, math, dingbats, emoji).
fn is_symbol(c: char) -> bool {
    matches!(c as u32, 0x2190..=0x2BFF | 0x1F000..=0x1FAFF)
}

/// Push escaped text for a start-aligned line, starting a new text chunk (a `y`-only
/// `<tspan>`, which continues at the current x) at every boundary between symbols
/// and other characters. The rasterizer substitutes a fallback font for a whole chunk
/// when a glyph is missing, so this confines the substitution to the symbols.
pub fn push_text_runs(output: &mut String, value: &str, y: f64) {
    let mut start = 0;
    let mut in_symbols = false;
    let mut first = true;
    for (index, c) in value.char_indices().chain([(value.len(), ' ')]) {
        let at_end = index == value.len();
        if at_end || is_symbol(c) != in_symbols {
            let part = &value[start..index];
            if !part.is_empty() {
                if first {
                    push_escaped(output, part);
                } else {
                    output.push_str(&format!(r#"<tspan y="{y}">"#));
                    push_escaped(output, part);
                    output.push_str("</tspan>");
                }
                first = false;
            }
            start = index;
            in_symbols = !in_symbols;
        }
    }
}

fn push_escaped(output: &mut String, value: &str) {
    for part in value.split_inclusive(['&', '<', '>', '"']) {
        let (body, escaped) = match part.as_bytes().last() {
            Some(b'&') => (&part[..part.len() - 1], Some("&amp;")),
            Some(b'<') => (&part[..part.len() - 1], Some("&lt;")),
            Some(b'>') => (&part[..part.len() - 1], Some("&gt;")),
            Some(b'"') => (&part[..part.len() - 1], Some("&quot;")),
            _ => (part, None),
        };
        output.push_str(body);
        if let Some(value) = escaped {
            output.push_str(value);
        }
    }
}

pub fn to_svg(doc: &Document) -> Result<String> {
    doc.validate().map_err(anyhow::Error::msg)?;
    to_svg_validated(doc)
}

fn to_svg_validated(doc: &Document) -> Result<String> {
    assemble_svg(doc, Some(&fonts::svg_faces(doc)?))
}

fn to_render_xml_validated(doc: &Document) -> Result<String> {
    assemble_svg(doc, None)
}

fn assemble_svg(doc: &Document, font_css: Option<&str>) -> Result<String> {
    let content_bytes: usize = doc
        .layers
        .iter()
        .filter(|l| l.visible)
        .flat_map(|l| &l.paths)
        .map(|p| p.d.len())
        .sum::<usize>()
        + doc
            .layers
            .iter()
            .filter(|l| l.visible)
            .flat_map(|l| &l.texts)
            .map(|t| t.content.len())
            .sum::<usize>();
    let objects: usize = doc
        .layers
        .iter()
        .filter(|l| l.visible)
        .map(|l| l.paths.len() + l.texts.len())
        .sum();
    let mut svg = String::with_capacity(
        content_bytes
            .saturating_add(objects.saturating_mul(192))
            .saturating_add(font_css.map_or(0, str::len))
            .saturating_add(256),
    );
    write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}"><rect width="100%" height="100%" fill=""#,
        doc.canvas.width, doc.canvas.height, doc.canvas.width, doc.canvas.height
    )?;
    push_escaped(&mut svg, &doc.canvas.background);
    svg.push_str(r#""/>"#);
    if let Some(css) = font_css {
        svg.push_str(css);
    }
    for layer in doc.layers.iter().filter(|layer| layer.visible) {
        svg.push_str(r#"<g id=""#);
        push_escaped(&mut svg, &layer.id);
        svg.push_str(r#"">"#);
        for path in &layer.paths {
            svg.push_str(r#"<path d=""#);
            push_escaped(&mut svg, &path.d);
            svg.push_str(r#"" stroke=""#);
            push_escaped(&mut svg, &path.stroke);
            write!(
                svg,
                r#"" stroke-width="{}" fill=""#,
                path.stroke_width.max(0.0)
            )?;
            push_escaped(&mut svg, &path.fill);
            write!(
                svg,
                r#"" stroke-linecap="{}" stroke-linejoin="{}" stroke-miterlimit="{}"/>"#,
                path.stroke_linecap.svg(),
                path.stroke_linejoin.svg(),
                path.stroke_miterlimit
            )?;
        }
        for text in &layer.texts {
            write_text_svg(&mut svg, text)?;
        }
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    Ok(svg)
}

pub fn text_svg(text: &Text) -> String {
    let mut svg = String::with_capacity(text.content.len() + 256);
    write_text_svg(&mut svg, text).expect("writing to String cannot fail");
    svg
}

fn write_text_svg(svg: &mut String, text: &Text) -> Result<()> {
    write!(
        svg,
        r#"<text xml:space="preserve" transform="matrix({} {} {} {} {} {})" font-family=""#,
        text.transform[0],
        text.transform[1],
        text.transform[2],
        text.transform[3],
        text.transform[4],
        text.transform[5]
    )?;
    push_escaped(svg, &text.font_family);
    write!(
        svg,
        r#"" font-size="{}" font-weight="{}" font-style="{}" text-anchor="{}" letter-spacing="{}" fill=""#,
        text.font_size,
        text.font_weight,
        if text.italic { "italic" } else { "normal" },
        text.align.anchor(),
        text.letter_spacing
    )?;
    push_escaped(svg, &text.fill);
    svg.push_str(r#"">"#);
    let mut rest = text.content.as_str();
    let mut index = 0usize;
    loop {
        let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
        let line = &rest[..end];
        let line_y = text.y + index as f64 * text.font_size * text.line_height;
        write!(svg, r#"<tspan x="{}" y="{line_y}">"#, text.x)?;
        if text.align.anchor() == "start" {
            push_text_runs(svg, line, line_y);
        } else {
            push_escaped(svg, line);
        }
        svg.push_str("</tspan>");
        if end == rest.len() {
            break;
        }
        let delimiter = rest.as_bytes()[end];
        let mut consumed = end + 1;
        if delimiter == b'\r' && rest.as_bytes().get(consumed) == Some(&b'\n') {
            consumed += 1;
        }
        rest = &rest[consumed..];
        index += 1;
    }
    svg.push_str("</text>");
    Ok(())
}

pub fn options(doc: &Document) -> Result<resvg::usvg::Options<'static>> {
    if doc.layers.iter().all(|l| l.texts.is_empty()) {
        return Ok(resvg::usvg::Options::default());
    }
    let db = fonts::database(doc, true)?;
    for (family, weight, italic) in fonts::requests(doc) {
        fonts::ensure_family(&db, &family, weight, italic)?;
    }
    Ok(resvg::usvg::Options {
        font_family: fonts::DEFAULT_FAMILY.into(),
        fontdb: std::sync::Arc::new(db),
        font_resolver: fonts::resolver(),
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
    doc.validate().map_err(anyhow::Error::msg)?;
    let svg = to_render_xml_validated(doc)?;
    let tree = resvg::usvg::Tree::from_str(&svg, &options(doc)?)?;
    Ok(tree.to_string(&resvg::usvg::WriteOptions::default()))
}

pub fn to_png(doc: &Document, scale: f32) -> Result<Vec<u8>> {
    Ok(to_png_profiled(doc, scale)?.bytes)
}

pub fn svg_to_png(svg: &str, canvas_width: u32, canvas_height: u32, scale: f32) -> Result<Vec<u8>> {
    if !(0.1..=8.0).contains(&scale) {
        bail!("scale must be between 0.1 and 8")
    }
    svg_to_png_with_fonts(svg, canvas_width, canvas_height, scale, &[])
}

/// Rasterizes a scene SVG with the bundled, embedded, and installed fonts so
/// `<text>` nodes render the same as in the SVG export.
pub fn scene_to_png(scene: &crate::image::SceneSvg, scale: f32) -> Result<Vec<u8>> {
    if !(0.1..=8.0).contains(&scale) {
        bail!("scale must be between 0.1 and 8")
    }
    scene_to_png_proxy(scene, scale)
}

/// Compositor proxy rasterization; released export scale checks stay unchanged.
pub(crate) fn scene_to_png_proxy(scene: &crate::image::SceneSvg, scale: f32) -> Result<Vec<u8>> {
    svg_to_png_with_fonts(&scene.svg, scene.width, scene.height, scale, &scene.fonts)
}

fn svg_to_png_with_fonts(
    svg: &str,
    canvas_width: u32,
    canvas_height: u32,
    scale: f32,
    fonts: &[crate::document::FontAsset],
) -> Result<Vec<u8>> {
    if !(1.0 / 16384.0..=8.0).contains(&scale) {
        bail!("proxy scale must be between 1/16384 and 8");
    }
    let mut doc = Document::new(canvas_width, canvas_height);
    doc.fonts = fonts.to_vec();
    let width = (f64::from(canvas_width) * f64::from(scale))
        .round()
        .max(1.0) as u32;
    let height = (f64::from(canvas_height) * f64::from(scale))
        .round()
        .max(1.0) as u32;
    let dimensions = Document::new(width, height);
    render_dimensions(&dimensions, 1.0)?;
    let options = resvg::usvg::Options {
        font_family: fonts::DEFAULT_FAMILY.into(),
        fontdb: std::sync::Arc::new(fonts::database(&doc, true)?),
        font_resolver: fonts::resolver(),
        ..Default::default()
    };
    let tree =
        resvg::usvg::Tree::from_str(svg, &options).context("could not parse generated SVG")?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).context("image is too large")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().context("could not encode PNG")
}

pub fn to_png_profiled(doc: &Document, scale: f32) -> Result<ProfiledPng> {
    if !(0.1..=8.0).contains(&scale) {
        bail!("scale must be between 0.1 and 8");
    }
    doc.validate().map_err(anyhow::Error::msg)?;
    to_png_profiled_validated(doc, scale)
}

pub(crate) fn to_png_profiled_validated(doc: &Document, scale: f32) -> Result<ProfiledPng> {
    let (width, height, pixmap_bytes) = render_dimensions(doc, scale)?;
    let at = Instant::now();
    let svg = to_svg_validated(doc)?;
    let svg_construction_us = at.elapsed().as_micros();
    let at = Instant::now();
    let render_options = options(doc)?;
    let font_preparation_us = at.elapsed().as_micros();
    let at = Instant::now();
    let tree = resvg::usvg::Tree::from_str(&svg, &render_options)
        .context("could not parse generated SVG")?;
    let usvg_parse_us = at.elapsed().as_micros();
    let mut pixmap = tiny_skia::Pixmap::new(width, height).context("image is too large")?;
    let at = Instant::now();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rasterization_us = at.elapsed().as_micros();
    let at = Instant::now();
    let bytes = pixmap.encode_png().context("could not encode PNG")?;
    let png_encoding_us = at.elapsed().as_micros();
    Ok(ProfiledPng {
        bytes,
        timings: RenderTimings {
            svg_construction_us,
            font_preparation_us,
            usvg_parse_us,
            rasterization_us,
            png_encoding_us,
        },
        width,
        height,
        pixmap_bytes,
    })
}

fn render_dimensions(doc: &Document, scale: f32) -> Result<(u32, u32, u64)> {
    let scaled = |value: u32| -> Result<u32> {
        let result = (value as f64 * f64::from(scale)).round();
        if !result.is_finite() || result < 1.0 || result > u32::MAX as f64 {
            bail!("scaled render dimensions are unsafe");
        }
        Ok(result as u32)
    };
    let width = scaled(doc.canvas.width)?;
    let height = scaled(doc.canvas.height)?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .context("render pixel count overflow")?;
    let row_bytes = u64::from(width)
        .checked_mul(4)
        .context("render row byte count overflow")?;
    let pixmap_bytes = row_bytes
        .checked_mul(u64::from(height))
        .context("render pixmap byte count overflow")?;
    if pixels > MAX_RENDER_PIXELS || pixmap_bytes > MAX_PIXMAP_BYTES {
        return Err(RenderLimitError {
            width,
            height,
            pixels,
            row_bytes,
            pixmap_bytes,
            max_pixels: MAX_RENDER_PIXELS,
            max_pixmap_bytes: MAX_PIXMAP_BYTES,
        }
        .into());
    }
    Ok((width, height, pixmap_bytes))
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
    let bytes = match output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => to_png(doc, scale)?,
        Some("svg") => (if outline_text {
            to_svg_outlined(doc)?
        } else {
            to_svg(doc)?
        })
        .into_bytes(),
        _ => bail!("output must end in .png or .svg"),
    };
    atomic_write(output, &bytes)?;
    Ok(())
}

fn atomic_write(output: &Path, bytes: &[u8]) -> Result<()> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let name = output
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("export");
    let temp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    fs::write(&temp, bytes).with_context(|| format!("could not write {}", temp.display()))?;
    let backup = parent.join(format!(".{name}.{}.bak", std::process::id()));
    let had_output = output.exists();
    if had_output {
        fs::rename(output, &backup)
            .with_context(|| format!("could not prepare to replace {}", output.display()))?;
    }
    if let Err(error) = fs::rename(&temp, output) {
        if had_output {
            let _ = fs::rename(&backup, output);
        }
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("could not replace {}", output.display()));
    }
    if had_output {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid_document() -> Document {
        let mut doc = Document::new(10, 10);
        doc.format = "not-pentool".into();
        doc
    }

    #[test]
    fn every_public_export_rejects_invalid_documents() {
        let doc = invalid_document();
        assert!(to_svg(&doc).is_err());
        assert!(to_svg_outlined(&doc).is_err());
        assert!(to_png(&doc, 1.0).is_err());
        assert!(to_png_profiled(&doc, 1.0).is_err());
        let path =
            std::env::temp_dir().join(format!("pentool-invalid-export-{}.png", std::process::id()));
        assert!(write_export(&doc, &path, 1.0).is_err());
        assert!(write_export_options(&doc, &path, 1.0, false).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn unsafe_render_does_not_overwrite_destination() {
        let doc = Document::new(16_384, 16_384);
        let path =
            std::env::temp_dir().join(format!("pentool-safe-export-{}.png", std::process::id()));
        fs::write(&path, b"existing").unwrap();
        let error = write_export(&doc, &path, 8.0).unwrap_err();
        assert!(error.downcast_ref::<RenderLimitError>().is_some());
        assert_eq!(fs::read(&path).unwrap(), b"existing");
        fs::remove_file(path).unwrap();
    }
}

//! Small deterministic vector PDF writer used for multi-page export.
use crate::document::Document;
use anyhow::{Context, Result};
use kurbo::{BezPath, PathEl, Point};

pub fn write(documents: &[Document], output: &std::path::Path) -> Result<()> {
    let page_count = documents.len();
    let font_id = 3 + page_count * 2;
    let mut objects = Vec::<Vec<u8>>::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids = (0..page_count)
        .map(|i| format!("{} 0 R", 3 + i * 2))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push(format!("<< /Type /Pages /Count {page_count} /Kids [{kids}] >>").into_bytes());
    for (index, doc) in documents.iter().enumerate() {
        let page_id = 3 + index * 2;
        let content_id = page_id + 1;
        let stream = content(doc)?;
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 {font_id} 0 R >> >> /Contents {content_id} 0 R >>",doc.canvas.width,doc.canvas.height).into_bytes());
        let mut object = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
        object.extend_from_slice(stream.as_bytes());
        object.extend_from_slice(b"\nendstream");
        objects.push(object)
    }
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    let mut pdf = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n")
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes())
    }
    pdf.extend_from_slice(
        format!(
            "trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    crate::editing::atomic_write(output, &pdf)
}

fn content(doc: &Document) -> Result<String> {
    let mut out = String::new();
    if let Some((r, g, b)) = color(&doc.canvas.background) {
        out.push_str(&format!(
            "{r} {g} {b} rg 0 0 {} {} re f\n",
            doc.canvas.width, doc.canvas.height
        ))
    }
    out.push_str(&format!("q 1 0 0 -1 0 {} cm\n", doc.canvas.height));
    for layer in &doc.layers {
        if !layer.visible {
            continue;
        }
        for path in &layer.paths {
            let bez = BezPath::from_svg(&path.d).context("invalid PDF path")?;
            let mut current = Point::ZERO;
            for element in bez.elements() {
                match *element {
                    PathEl::MoveTo(p) => {
                        current = p;
                        out.push_str(&format!("{} {} m ", p.x, p.y))
                    }
                    PathEl::LineTo(p) => {
                        current = p;
                        out.push_str(&format!("{} {} l ", p.x, p.y))
                    }
                    PathEl::QuadTo(p1, p2) => {
                        let c1 = current + (p1 - current) * (2.0 / 3.0);
                        let c2 = p2 + (p1 - p2) * (2.0 / 3.0);
                        out.push_str(&format!(
                            "{} {} {} {} {} {} c ",
                            c1.x, c1.y, c2.x, c2.y, p2.x, p2.y
                        ));
                        current = p2
                    }
                    PathEl::CurveTo(p1, p2, p3) => {
                        out.push_str(&format!(
                            "{} {} {} {} {} {} c ",
                            p1.x, p1.y, p2.x, p2.y, p3.x, p3.y
                        ));
                        current = p3
                    }
                    PathEl::ClosePath => out.push_str("h "),
                }
            }
            let fill = color(&path.fill);
            let stroke = color(&path.stroke);
            if let Some((r, g, b)) = fill {
                out.push_str(&format!("{r} {g} {b} rg "))
            }
            if let Some((r, g, b)) = stroke {
                out.push_str(&format!("{r} {g} {b} RG {} w ", path.stroke_width))
            }
            out.push_str(match (fill.is_some(), stroke.is_some()) {
                (true, true) => "B\n",
                (true, false) => "f\n",
                (false, true) => "S\n",
                _ => "n\n",
            })
        }
    }
    out.push_str("Q\n");
    for layer in &doc.layers {
        if !layer.visible {
            continue;
        }
        for text in &layer.texts {
            if let Some((r, g, b)) = color(&text.fill) {
                out.push_str(&format!("{r} {g} {b} rg\n"))
            }
            for (index, line) in text.content.lines().enumerate() {
                let y = doc.canvas.height as f64 - text.y
                    + index as f64 * text.font_size * text.line_height;
                out.push_str(&format!(
                    "BT /F1 {} Tf {} {} Td ({}) Tj ET\n",
                    text.font_size,
                    text.x,
                    y,
                    escape(line)
                ))
            }
        }
    }
    Ok(out)
}
fn color(value: &str) -> Option<(f64, f64, f64)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let number = u32::from_str_radix(hex, 16).ok()?;
    Some((
        ((number >> 16) & 255) as f64 / 255.0,
        ((number >> 8) & 255) as f64 / 255.0,
        (number & 255) as f64 / 255.0,
    ))
}
fn escape(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '(' => "\\(".into(),
            ')' => "\\)".into(),
            '\\' => "\\\\".into(),
            c if c.is_ascii() && !c.is_control() => c.to_string(),
            _ => "?".into(),
        })
        .collect()
}

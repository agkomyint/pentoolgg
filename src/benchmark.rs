//! Repeatable large-document smoke benchmark for release qualification.
use crate::{
    agent::{self, ObjectAction},
    document::{Document, Layer, Path, StrokeCap, StrokeJoin, Text},
    render,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::time::Instant;

pub fn run(
    layer_count: usize,
    object_count: usize,
    png: bool,
    max_ms: Option<u128>,
) -> Result<Value> {
    if layer_count == 0 || layer_count > 1000 || object_count > 100_000 {
        bail!("benchmark supports 1–1,000 layers and at most 100,000 objects");
    }
    let generated_at = Instant::now();
    let doc = generate(layer_count, object_count);
    let generate_ms = generated_at.elapsed().as_millis();

    let serialize_at = Instant::now();
    let bytes = serde_json::to_vec(&doc)?;
    let serialize_ms = serialize_at.elapsed().as_millis();
    let parse_at = Instant::now();
    let mut parsed: Document = serde_json::from_slice(&bytes)?;
    let parse_ms = parse_at.elapsed().as_millis();
    let validate_at = Instant::now();
    parsed.validate().map_err(anyhow::Error::msg)?;
    let validate_ms = validate_at.elapsed().as_millis();
    let index_at = Instant::now();
    agent::DocumentIndex::build(&parsed)?;
    let index_ms = index_at.elapsed().as_millis();
    let search_at = Instant::now();
    agent::inspect_paginated(&parsed, Some("object"), None, None, 0, 100)?;
    let search_ms = search_at.elapsed().as_millis();
    let edit_at = Instant::now();
    if object_count > 0 {
        agent::apply(
            &mut parsed,
            &ObjectAction::Set {
                id: "object-0".into(),
                layer: "layer-0".into(),
                d: None,
                stroke: None,
                width: None,
                fill: Some("#ff00ff".into()),
                cap: None,
                join: None,
                miter_limit: None,
                content: None,
                x: None,
                y: None,
                font: None,
                size: None,
                weight: None,
                italic: None,
                align: None,
                letter_spacing: None,
                line_height: None,
            },
        )?;
    }
    let edit_ms = edit_at.elapsed().as_millis();
    let svg_at = Instant::now();
    let svg = render::to_svg(&parsed)?;
    let svg_ms = svg_at.elapsed().as_millis();
    let (png_ms, png_bytes) = if png {
        let at = Instant::now();
        let output = render::to_png(&parsed, 1.0)?;
        (Some(at.elapsed().as_millis()), Some(output.len()))
    } else {
        (None, None)
    };
    let total_ms = generate_ms
        + serialize_ms
        + parse_ms
        + validate_ms
        + index_ms
        + search_ms
        + edit_ms
        + svg_ms
        + png_ms.unwrap_or(0);
    if let Some(budget) = max_ms {
        if total_ms > budget {
            bail!("benchmark exceeded {budget} ms budget: {total_ms} ms");
        }
    }
    Ok(json!({
        "layers":layer_count,"objects":object_count,"json_bytes":bytes.len(),"svg_bytes":svg.len(),"png_bytes":png_bytes,
        "milliseconds":{"generate":generate_ms,"serialize":serialize_ms,"parse":parse_ms,"validate":validate_ms,"index":index_ms,"search_100":search_ms,"edit_one":edit_ms,"svg":svg_ms,"png":png_ms,"total":total_ms}
    }))
}

fn generate(layer_count: usize, object_count: usize) -> Document {
    let mut doc = Document::new(1200, 800);
    doc.layers.clear();
    for index in 0..layer_count {
        doc.layers.push(Layer {
            id: format!("layer-{index}"),
            name: format!("Layer {index}"),
            visible: true,
            locked: false,
            paths: vec![],
            texts: vec![],
        });
    }
    for index in 0..object_count {
        let layer = &mut doc.layers[index % layer_count];
        if index % 10 == 9 {
            layer.texts.push(Text {
                id: format!("object-{index}"),
                content: format!("Label {index}"),
                x: (index % 100) as f64 * 10.0,
                y: (index % 70) as f64 * 10.0 + 20.0,
                font_family: crate::fonts::DEFAULT_FAMILY.into(),
                font_size: 12.0,
                font_weight: 400,
                italic: false,
                fill: "#111827".into(),
                align: Default::default(),
                letter_spacing: 0.0,
                line_height: 1.2,
                transform: crate::document::identity(),
            });
        } else {
            let x = (index % 100) * 10;
            let y = (index % 70) * 10;
            layer.paths.push(Path {
                id: format!("object-{index}"),
                d: format!("M {x} {y} L {} {}", x + 6, y + 6),
                stroke: "#111827".into(),
                stroke_width: 1.0,
                stroke_linecap: StrokeCap::Butt,
                stroke_linejoin: StrokeJoin::Miter,
                stroke_miterlimit: 4.0,
                fill: "none".into(),
                closed: false,
            });
        }
    }
    doc
}

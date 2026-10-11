//! `pentool paint-fields`: build and preview a stroke field and the strokes it would
//! generate without painting (v0.13.0 task 6). Read-only.
use crate::raster::fields::{self, FieldRequest, FieldSpec};
use crate::raster::{Blend, Brush};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(clap::Subcommand, Debug)]
pub enum FieldAction {
    /// Build a field for a raster layer; report its hash, sample vectors, and (with
    /// --params) the strokes painting through it would generate.
    Preview {
        /// Raster layer id.
        id: String,
        /// Field JSON or @file.json.
        #[arg(long)]
        field: String,
        /// Stroke rules JSON or @file.json; adds a stroke plan to the result.
        #[arg(long)]
        params: Option<String>,
        /// Region constraints JSON or @file.json used by the stroke plan.
        #[arg(long)]
        constraints: Option<String>,
        /// Brush JSON (only `size` affects planning).
        #[arg(long, default_value = "{}")]
        brush: String,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Maximum field vectors to list.
        #[arg(long, default_value_t = 100)]
        max_vectors: usize,
    },
}

fn read_json(text: &str) -> Result<Value> {
    let body = match text.strip_prefix('@') {
        Some(path) => {
            ensure!(
                std::fs::metadata(path)
                    .with_context(|| format!("[missing-resource] cannot read {path}"))?
                    .len()
                    <= 4 << 20,
                "[limit-exceeded] {path} is larger than 4 MiB"
            );
            std::fs::read_to_string(path)?
        }
        None => text.to_owned(),
    };
    serde_json::from_str(&body).context("[invalid-request] argument must be JSON")
}

pub fn run(input: &PathBuf, action: FieldAction) -> Result<()> {
    let raw: Value = serde_json::from_slice(
        &std::fs::read(input)
            .with_context(|| format!("[missing-resource] cannot read {}", input.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    let FieldAction::Preview {
        id,
        field,
        params,
        constraints,
        brush,
        seed,
        max_vectors,
    } = action;
    let field_json = crate::paint_guide::resolve_field(&raw, &read_json(&field)?)?;
    let built = fields::build(&raw, &id, &FieldSpec::parse(&field_json)?)?;
    let mut samples = Vec::new();
    let mut defined = 0_u64;
    for (index, v) in built.vectors.iter().enumerate() {
        if *v == [0, 0] {
            continue;
        }
        defined += 1;
        if samples.len() < max_vectors {
            let (cx, cy) = (index as u32 % built.cols, index as u32 / built.cols);
            samples.push(json!([
                built.rect[0] + cx * built.cell,
                built.rect[1] + cy * built.cell,
                v[0],
                v[1]
            ]));
        }
    }
    let mut result = json!({
        "layer": id,
        "field": built.summary,
        "returned": samples.len(),
        "has_more": defined > samples.len() as u64,
        "vector_scale": 32767,
        "samples": samples,
    });
    if let Some(params) = params {
        let request = FieldRequest {
            field: field_json,
            params: read_json(&params)?,
            brush: Brush::parse(&crate::raster::resolve_preset(
                &raw,
                &read_json(&brush)?,
                None,
            )?)?,
            color: [0, 0, 0],
            blend: Blend::parse("normal")?,
            seed,
            constraints: match &constraints {
                Some(text) => read_json(text)?,
                None => Value::Null,
            },
        };
        result["plan"] = fields::plan_report(&raw, &id, &request)?;
    }
    println!("{result}");
    Ok(())
}

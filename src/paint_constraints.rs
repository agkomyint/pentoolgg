//! `pentool paint-constraints`: resolve and preview region constraints without
//! painting (v0.13.0 task 5). Read-only; never mutates the document.
use crate::raster::constraints::{self, Follow, Spec};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(clap::Subcommand, Debug)]
pub enum ConstraintAction {
    /// Resolve a constraint spec for a raster layer and report the mask digest.
    Preview {
        /// Raster layer id.
        id: String,
        /// Constraints JSON (inside, avoid, opaque_clip, barriers, reach_from, follow) or @file.json.
        #[arg(long)]
        spec: String,
        /// Write the resolved coverage mask as a grayscale PNG.
        #[arg(long)]
        out: Option<String>,
        /// Direction-field grid cell in pixels when `follow` is set.
        #[arg(long, default_value_t = 16)]
        cell: u32,
        /// Maximum direction vectors to list.
        #[arg(long, default_value_t = 200)]
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
    serde_json::from_str(&body).context("[invalid-request] --spec must be JSON")
}

pub fn run(input: &PathBuf, action: ConstraintAction) -> Result<()> {
    let raw: Value = serde_json::from_slice(
        &std::fs::read(input)
            .with_context(|| format!("[missing-resource] cannot read {}", input.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    let ConstraintAction::Preview {
        id,
        spec,
        out,
        cell,
        max_vectors,
    } = action;
    let spec = Spec::parse(&read_json(&spec)?)?;
    let resolved = constraints::resolve(&raw, &id, &spec)?;
    ensure!(
        resolved.summary["allowed_pixels"].as_u64() != Some(0),
        "[empty-constraint] the constraints leave no paintable pixel; relax inside/avoid/barriers (mask {})",
        resolved.summary["mask_hash"]
    );
    let mut result = json!({"layer": id, "mask": resolved.summary});
    if let Some(follow) = spec.follow {
        result["field"] = match follow {
            Follow::Angle(a) => json!({"mode": "angle", "degrees": a}),
            other => {
                let mut field = constraints::field_preview(&raw, &id, other, cell, max_vectors)?;
                field["mode"] = json!(if other == Follow::Contour {
                    "contour"
                } else {
                    "gradient"
                });
                field
            }
        };
    }
    if let Some(path) = out {
        let path = PathBuf::from(path);
        ensure!(
            !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("png")),
            "[invalid-request] --out must be a .png path without '..'"
        );
        let png = constraints::mask_png(&resolved)?;
        std::fs::write(&path, &png)?;
        result["artifact"] = json!({"path": path, "bytes": png.len()});
    }
    println!("{result}");
    Ok(())
}

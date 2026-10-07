//! One dispatcher for raster editing actions, shared by `POST /api/raster` and
//! `pentool raster DOC batch`. Each action mirrors the matching CLI command.
use super::info;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// Most operations in one batch.
pub const MAX_BATCH_OPS: usize = 256;
/// Most stroke samples summed over one batch.
pub const MAX_BATCH_SAMPLES: usize = 2_000_000;

/// Run one action on `raw` and return its compact result. `raw` may be partly
/// changed when this fails; callers work on a clone.
pub fn apply(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    action: &str,
    args: &Value,
) -> Result<Value> {
    let args = if args.is_null() { &Value::Null } else { args };
    let num = |k: &str| {
        args[k]
            .as_f64()
            .context(format!("[invalid-input] args.{k} must be a number"))
    };
    let text = |k: &str, d: &'static str| args[k].as_str().unwrap_or(d).to_owned();
    let count = |k: &str| args[k].as_u64().unwrap_or(0) as u32;
    let flood = || super::FloodOptions {
        x: count("x"),
        y: count("y"),
        tolerance: args["tolerance"].as_u64().unwrap_or(32).min(255) as u8,
        diagonal: args["diagonal"].as_bool().unwrap_or(false),
        contiguous: args["contiguous"].as_bool().unwrap_or(true),
        antialias: args["antialias"].as_bool().unwrap_or(true),
        gap: count("gap"),
        transparent_barrier: args["transparent_barrier"].as_bool().unwrap_or(false),
    };
    let result = match action {
        "info" => super::info(raw, page, id)?,
        "fill" => super::fill(
            raw,
            page,
            id,
            super::parse_color(&text("color", "#000000"))?,
            args["opacity"].as_f64().unwrap_or(1.0),
            &flood(),
        )?,
        "clear" => super::clear(raw, page, id)?,
        "select-info" => super::select_info(raw, id)?,
        "select-clear" => super::select_clear(raw, id)?,
        "stroke" | "quickmask" | "clone" | "heal" => {
            let normalized = super::normalize_input(&args["samples"])?;
            let input_summary = normalized.summary();
            let brush = super::Brush::parse(&super::resolve_preset(
                raw,
                &if args["brush"].is_null() {
                    json!({})
                } else {
                    args["brush"].clone()
                },
                args["preset"].as_str(),
            )?)?;
            let seed = args["seed"].as_u64().unwrap_or(0);
            let erase = args["erase"].as_bool().unwrap_or(false);
            let mut result = match action {
                "stroke" => super::paint(
                    raw,
                    page,
                    id,
                    super::StrokeRequest {
                        brush,
                        samples: normalized.samples,
                        color: super::parse_color(&text("color", "#000000"))?,
                        blend: super::Blend::parse(&text("blend", "normal"))?,
                        seed,
                        clone: None,
                    },
                )?,
                "quickmask" => super::select_quickmask(
                    raw,
                    page,
                    id,
                    brush,
                    normalized.samples,
                    erase,
                    seed,
                )?,
                other => {
                    let options = super::CloneOptions {
                        aligned: args["aligned"].as_bool().unwrap_or(true),
                        angle: args["angle"].as_f64().unwrap_or(0.0),
                        scale: args["scale"].as_f64().unwrap_or(1.0)
                    };
                    let tool = if other == "heal" {
                        super::Tool::Heal
                    } else {
                        super::Tool::Clone
                    };
                    super::clone_stroke(
                        raw,
                        page,
                        id,
                        brush,
                        normalized.samples,
                        &options,
                        seed,
                        tool,
                    )?
                }
            };
            result["input"] = input_summary;
            result
        }
        "set-clone-source" => super::set_clone_source(
            raw,
            page,
            id,
            args["layer"].as_str(),
            num("x")?,
            num("y")?,
        )?,
        "select-marquee" => super::select_marquee(
            raw,
            page,
            id,
            super::Marquee::parse(&text("shape", "rect"))?,
            [num("x")?, num("y")?, num("width")?, num("height")?],
            super::SelectionMode::parse(&text("mode", "replace"))?,
            count("feather"),
        )?,
        "select-lasso" => {
            let points: Vec<[f64; 2]> = serde_json::from_value(args["points"].clone())
                .context("[invalid-input] args.points must be [[x,y],...]")?;
            super::select_lasso(
                raw,
                page,
                id,
                &points,
                super::SelectionMode::parse(&text("mode", "replace"))?,
                count("feather"),
            )?
        }
        "select-wand" => super::select_wand(
            raw,
            page,
            id,
            &flood(),
        super::SelectionMode::parse(&text("mode", "replace"))?,
        )?,
        other => bail!(
            "[invalid-input] unknown raster action {other:?}; use info, stroke, quickmask, clone, heal, fill, clear, set-clone-source, select-marquee, select-lasso, select-wand, select-info or select-clear"
        ),
    };
    Ok(result)
}

/// Run `operations` (`{action, id, args?, page?}`) in order on a clone of `raw`.
/// Limits are checked before any pixel work; on success `raw` is replaced and the
/// summary lists, per operation, the engine's compact result and the layer's tile-map
/// hash. A failure names the operation and leaves `raw` untouched.
pub fn batch(raw: &mut Value, page: Option<&str>, operations: &[Value]) -> Result<Value> {
    if operations.is_empty() || operations.len() > MAX_BATCH_OPS {
        bail!(
            "[limit-exceeded] a raster batch needs 1-{MAX_BATCH_OPS} operations, got {}",
            operations.len()
        )
    }
    let mut samples = 0usize;
    for (index, op) in operations.iter().enumerate() {
        let object = op
            .as_object()
            .with_context(|| format!("[invalid-input] operation {index} must be an object"))?;
        if let Some(key) = object
            .keys()
            .find(|k| !matches!(k.as_str(), "action" | "id" | "args" | "page"))
        {
            bail!("[invalid-input] operation {index} has unknown field {key:?}; use action, id, args, page")
        }
        for key in ["action", "id"] {
            if object
                .get(key)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                bail!("[invalid-input] operation {index} needs a non-empty string {key:?}")
            }
        }
        samples += object
            .get("args")
            .and_then(|a| a["samples"].as_array())
            .map_or(0, Vec::len);
        if samples > MAX_BATCH_SAMPLES {
            bail!("[limit-exceeded] the batch carries more than {MAX_BATCH_SAMPLES} stroke samples; split it")
        }
    }
    let mut next = raw.clone();
    let mut report = Vec::with_capacity(operations.len());
    for (index, op) in operations.iter().enumerate() {
        let action = op["action"].as_str().unwrap_or_default();
        let id = op["id"].as_str().unwrap_or_default();
        let op_page = op["page"].as_str().or(page);
        let result = apply(&mut next, op_page, id, action, &op["args"]).with_context(|| {
            format!("raster batch operation {index} ({action} on {id}) failed; nothing was written")
        })?;
        let tile_map = info(&next, op_page, id)
            .ok()
            .map(|i| i["tile_map_sha256"].clone());
        report.push(json!({
            "index": index,
            "action": action,
            "id": id,
            "result": result,
            "tile_map_sha256": tile_map,
        }));
    }
    super::super::transaction::validate_value(&next)?;
    *raw = next;
    Ok(json!({"operations": report.len(), "results": report}))
}

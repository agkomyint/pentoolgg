//! Version 12 vector shape gradient paint contract.
//!
//! These paints use SVG object-bounding-box coordinates. `style.*.fallback`
//! contains either the existing color grammar or a bounded linear/radial
//! gradient definition with 0..1 stop positions and color values.

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

const MAX_COORDINATE: f64 = 1_000_000.0;

pub(crate) fn validate_tree(raw: &Value, node: &Value) -> Result<()> {
    for slot in ["fill", "stroke"] {
        let Some(gradient) = node
            .pointer(&format!("/style/{slot}/fallback"))
            .filter(|value| matches!(value["kind"].as_str(), Some("linear" | "radial")))
        else {
            continue;
        };
        ensure!(
            raw["version"].as_u64() == Some(crate::scene::PAINT_VERSION),
            "[unsupported-capability] shape gradient paints require v12; run `pentool migrate --target 12` first"
        );
        validate_gradient(raw, node, slot, gradient)?;
    }
    if let Some(children) = node["children"].as_array() {
        for child in children {
            validate_tree(raw, child)?;
        }
    }
    if node["kind"] == "instance" {
        if let Some(fallback) = node.get("fallback") {
            validate_tree(raw, fallback)?;
        }
    }
    Ok(())
}

fn validate_gradient(raw: &Value, node: &Value, slot: &str, gradient: &Value) -> Result<()> {
    let kind = gradient["kind"]
        .as_str()
        .context("gradient kind is missing")?;
    ensure!(
        matches!(node["kind"].as_str(), Some("rect" | "ellipse" | "path" | "line"))
            && (node["kind"] != "line" || slot == "stroke"),
        "[unsupported-capability] v12 gradients are supported on rect, ellipse, path, and line strokes"
    );
    let allowed = match kind {
        "linear" => &[
            "kind",
            "stops",
            "x1",
            "y1",
            "x2",
            "y2",
            "space",
            "spread",
            "transform",
        ][..],
        "radial" => &[
            "kind",
            "stops",
            "cx",
            "cy",
            "radius",
            "space",
            "spread",
            "transform",
        ][..],
        _ => unreachable!(),
    };
    let object = gradient
        .as_object()
        .context("[invalid-fill] gradient paint must be an object")?;
    ensure!(
        object.keys().all(|key| allowed.contains(&key.as_str())),
        "[invalid-fill] gradient paint contains an unsupported property"
    );
    ensure!(
        matches!(
            gradient["space"].as_str().unwrap_or("srgb"),
            "srgb" | "linear"
        ),
        "[invalid-fill] shape gradient space must be srgb or linear"
    );
    ensure!(
        matches!(
            gradient["spread"].as_str().unwrap_or("pad"),
            "pad" | "repeat" | "reflect"
        ),
        "[invalid-fill] gradient spread must be pad, repeat, or reflect"
    );
    let stops = gradient["stops"]
        .as_array()
        .filter(|stops| (2..=64).contains(&stops.len()))
        .context("[invalid-fill] gradients require 2..64 stops")?;
    let mut previous = -1.0;
    for stop in stops {
        let position = stop["position"]
            .as_f64()
            .filter(|position| position.is_finite() && (0.0..=1.0).contains(position))
            .context("[invalid-fill] gradient stop position must be finite and in 0..=1")?;
        ensure!(
            position > previous,
            "[invalid-fill] gradient stop positions must increase strictly"
        );
        previous = position;
        crate::fill::validate_color(
            &stop["color"],
            raw.get("styles").unwrap_or(&Value::Null),
            false,
        )?;
    }
    ensure!(
        stops.first().unwrap()["position"] == 0.0 && stops.last().unwrap()["position"] == 1.0,
        "[invalid-fill] gradient stops must start at 0 and end at 1"
    );
    let bounded_number = |key: &str, default: f64| -> Result<f64> {
        let value = gradient.get(key).map_or(Ok(default), |value| {
            value
                .as_f64()
                .filter(|number| number.is_finite() && number.abs() <= MAX_COORDINATE)
                .context("[invalid-fill] gradient coordinate must be finite and bounded")
        })?;
        Ok(value)
    };
    match kind {
        "linear" => {
            let (x1, y1) = (bounded_number("x1", 0.0)?, bounded_number("y1", 0.0)?);
            let (x2, y2) = (bounded_number("x2", 1.0)?, bounded_number("y2", 0.0)?);
            ensure!(
                (x1 - x2).abs() + (y1 - y2).abs() > f64::EPSILON,
                "[invalid-fill] linear gradient endpoints must differ"
            );
        }
        "radial" => {
            bounded_number("cx", 0.5)?;
            bounded_number("cy", 0.5)?;
            ensure!(
                bounded_number("radius", 0.5)? > 0.0,
                "[invalid-fill] radial gradient radius must be positive"
            );
        }
        _ => unreachable!(),
    }
    if let Some(transform) = gradient.get("transform") {
        let values = transform
            .as_array()
            .filter(|values| values.len() == 6)
            .context("[invalid-fill] gradient transform must contain six numbers")?;
        let values = values
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .filter(|number| number.is_finite() && number.abs() <= MAX_COORDINATE)
                    .context("[invalid-fill] gradient transform values must be finite and bounded")
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            (values[0] * values[3] - values[1] * values[2]).abs() > 1e-12,
            "[invalid-fill] gradient transform must be invertible"
        );
    }
    Ok(())
}

/// Downgrade v12 only if no shape gradient would be lost.
pub fn downgrade_to_v11(mut raw: Value) -> Result<Value> {
    ensure!(
        raw["version"].as_u64() == Some(crate::scene::PAINT_VERSION),
        "[invalid-input] paint downgrade requires a v12 document"
    );
    fn find_gradient(nodes: &[Value]) -> Option<&str> {
        for node in nodes {
            if ["fill", "stroke"].iter().any(|slot| {
                matches!(
                    node.pointer(&format!("/style/{slot}/fallback/kind"))
                        .and_then(Value::as_str),
                    Some("linear" | "radial")
                )
            }) {
                return node["id"].as_str();
            }
            if let Some(children) = node["children"].as_array() {
                if let Some(id) = find_gradient(children) {
                    return Some(id);
                }
            }
        }
        None
    }
    for page in raw["pages"].as_array().into_iter().flatten() {
        for layer in page["layers"].as_array().into_iter().flatten() {
            if let Some(id) = layer["nodes"]
                .as_array()
                .and_then(|nodes| find_gradient(nodes))
            {
                bail!("[unsupported-capability] v11 cannot represent gradient paint on object {id}; replace it with a solid fill first")
            }
        }
    }
    raw["version"] = json!(crate::scene3d::node::BINDINGS_VERSION);
    crate::scene::validate(&raw)?;
    Ok(raw)
}

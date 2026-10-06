//! Deterministic solid, gradient, and content-addressed pattern fill nodes.
use anyhow::{bail, Context, Result};
use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
use std::path::Path;

fn n(value: &Value, key: &str, default: f64) -> Result<f64> {
    value.get(key).map_or(Ok(default), |v| {
        v.as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1e6)
            .context("[invalid-fill] number must be finite and bounded")
    })
}
pub(crate) fn color(value: &Value, styles: &Value) -> Result<[f64; 4]> {
    fn resolve(value: &Value, styles: &Value, depth: usize) -> Result<[f64; 4]> {
        if depth > 64 {
            bail!("[invalid-fill] color token depth exceeded")
        }
        if let Some(reference) = value.get("ref").and_then(Value::as_str) {
            if let Some(token) = styles.get(reference).and_then(|v| v.get("value")) {
                return resolve(token, styles, depth + 1);
            }
            return resolve(
                value
                    .get("fallback")
                    .context("missing color token fallback")?,
                styles,
                depth + 1,
            );
        }
        if let Some(fallback) = value.get("fallback") {
            return resolve(fallback, styles, depth + 1);
        }
        if let Some(values) = value.as_array() {
            if ![3, 4].contains(&values.len()) {
                bail!("color must contain RGB or RGBA")
            }
            let mut out = [0.0, 0.0, 0.0, 1.0];
            for (i, v) in values.iter().enumerate() {
                out[i] = v
                    .as_f64()
                    .filter(|n| (0.0..=255.0).contains(n))
                    .context("color channels must be in 0..255")?
                    / 255.0;
            }
            return Ok(out);
        }
        let s = value
            .as_str()
            .context("color must be a hex string, RGBA array, or token reference")?;
        if s == "none" {
            return Ok([0.0; 4]);
        }
        let hex = s
            .strip_prefix('#')
            .filter(|s| [3, 4, 6, 8].contains(&s.len()) && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .context("color must use #RGB, #RGBA, #RRGGBB, or #RRGGBBAA")?;
        let mut out = [0.0, 0.0, 0.0, 1.0];
        if hex.len() <= 4 {
            for (i, c) in hex.as_bytes().iter().enumerate() {
                out[i] = f64::from((*c as char).to_digit(16).unwrap() * 17) / 255.0;
            }
        } else {
            for (i, output) in out.iter_mut().take(hex.len() / 2).enumerate() {
                let start = i * 2;
                *output = f64::from(u8::from_str_radix(&hex[start..start + 2], 16)?) / 255.0;
            }
        }
        Ok(out)
    }
    resolve(value, styles, 0)
}
fn stops(fill: &Value, styles: &Value) -> Result<Vec<(f64, [f64; 4])>> {
    let values = fill["stops"]
        .as_array()
        .filter(|a| (2..=64).contains(&a.len()))
        .context("[invalid-fill] gradients require 2..64 stops")?;
    let mut out = Vec::new();
    for value in values {
        let position = value["position"]
            .as_f64()
            .filter(|v| (0.0..=1.0).contains(v))
            .context("gradient position must be in 0..1")?;
        out.push((position, color(&value["color"], styles)?));
    }
    if out[0].0 != 0.0 || out.last().unwrap().0 != 1.0 || out.windows(2).any(|w| w[0].0 >= w[1].0) {
        bail!("gradient stops must increase strictly from 0 to 1")
    }
    Ok(out)
}
pub(crate) fn resolved<'a>(raw: &'a Value, mut fill: &'a Value) -> Result<&'a Value> {
    for _ in 0..64 {
        let Some(reference) = fill.get("ref") else {
            return Ok(fill);
        };
        let name = reference
            .as_str()
            .filter(|s| !s.is_empty())
            .context("fill style ref must be nonempty")?;
        crate::composite::settings(fill, &["ref", "fallback"])?;
        let fallback = fill
            .get("fallback")
            .context("fill style requires a portable fallback")?;
        fill = if let Some(style) = raw["styles"].get(name) {
            if style["type"] != "fill" {
                bail!("fill style {name} must have type fill")
            }
            &style["value"]
        } else {
            fallback
        };
    }
    bail!("[invalid-fill] fill style reference depth exceeds 64")
}

pub fn validate(raw: &Value, node: &Value) -> Result<()> {
    let fill = resolved(raw, &node["fill"])?;
    let styles = raw.get("styles").unwrap_or(&Value::Null);
    for key in ["x", "y", "width", "height"] {
        n(node, key, 0.0)?;
    }
    if n(node, "width", 0.0)? <= 0.0 || n(node, "height", 0.0)? <= 0.0 {
        bail!("fill dimensions must be positive")
    }
    let kind = fill["kind"].as_str().context("fill kind missing")?;
    let keys: &[&str] = match kind {
        "solid" => &["kind", "color", "transform"],
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
        ],
        "radial" => &[
            "kind",
            "stops",
            "cx",
            "cy",
            "radius",
            "space",
            "spread",
            "transform",
        ],
        "conic" => &[
            "kind",
            "stops",
            "cx",
            "cy",
            "angle",
            "space",
            "spread",
            "transform",
        ],
        "pattern" => &[
            "kind",
            "asset",
            "origin_x",
            "origin_y",
            "scale_x",
            "scale_y",
            "repeat",
            "transform",
        ],
        _ => bail!("unknown fill kind {kind}"),
    };
    for key in fill.as_object().context("fill must be an object")?.keys() {
        if !keys.contains(&key.as_str()) {
            bail!("unknown fill parameter {key}")
        }
    }
    let transform = crate::composite::transform(fill)?;
    if transform.determinant().abs() < 1e-12 {
        bail!("fill transform must be invertible")
    }
    match kind {
        "solid" => {
            color(&fill["color"], styles)?;
        }
        "pattern" => {
            let asset = fill["asset"].as_str().context("pattern asset missing")?;
            if raw["image_assets"].get(asset).is_none() {
                bail!("pattern source {asset} missing")
            }
            for key in ["scale_x", "scale_y"] {
                if n(fill, key, 1.0)? <= 0.0 {
                    bail!("pattern scales must be positive")
                }
            }
            for key in ["origin_x", "origin_y"] {
                n(fill, key, 0.0)?;
            }
            if !matches!(
                fill.get("repeat")
                    .and_then(Value::as_str)
                    .unwrap_or("repeat"),
                "repeat" | "repeat-x" | "repeat-y" | "none"
            ) {
                bail!("unknown pattern repeat mode")
            }
        }
        _ => {
            stops(fill, styles)?;
            if !matches!(
                fill.get("space").and_then(Value::as_str).unwrap_or("srgb"),
                "srgb" | "linear"
            ) {
                bail!("gradient space must be srgb or linear")
            }
            if !matches!(
                fill.get("spread").and_then(Value::as_str).unwrap_or("pad"),
                "pad" | "repeat" | "reflect"
            ) {
                bail!("unknown gradient spread")
            }
            match kind {
                "linear" => {
                    let (x1, y1, x2, y2) = (
                        n(fill, "x1", 0.0)?,
                        n(fill, "y1", 0.0)?,
                        n(fill, "x2", 1.0)?,
                        n(fill, "y2", 0.0)?,
                    );
                    if x1 == x2 && y1 == y2 {
                        bail!("gradient endpoints must differ")
                    }
                }
                "radial" => {
                    n(fill, "cx", 0.0)?;
                    n(fill, "cy", 0.0)?;
                    if n(fill, "radius", 1.0)? <= 0.0 {
                        bail!("radial gradient radius must be positive")
                    }
                }
                _ => {
                    n(fill, "cx", 0.0)?;
                    n(fill, "cy", 0.0)?;
                    n(fill, "angle", 0.0)?;
                }
            }
        }
    }
    Ok(())
}
fn atan2(y: f64, x: f64) -> f64 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let mut t = if x.abs() >= y.abs() {
        y.abs() / x.abs()
    } else {
        x.abs() / y.abs()
    };
    t /= 1.0 + (1.0 + t * t).sqrt();
    let mut term = t;
    let mut sum = 0.0;
    for i in 0..32 {
        sum += term / f64::from(2 * i + 1);
        term *= -t * t;
    }
    let mut angle = 2.0 * sum;
    if y.abs() > x.abs() {
        angle = std::f64::consts::FRAC_PI_2 - angle;
    }
    if x < 0.0 {
        angle = std::f64::consts::PI - angle;
    }
    if y < 0.0 {
        -angle
    } else {
        angle
    }
}
fn gradient(stops: &[(f64, [f64; 4])], mut t: f64, spread: &str, space: &str) -> [f64; 4] {
    t = match spread {
        "repeat" => t.rem_euclid(1.0),
        "reflect" => {
            let v = t.rem_euclid(2.0);
            if v > 1.0 {
                2.0 - v
            } else {
                v
            }
        }
        _ => t.clamp(0.0, 1.0),
    };
    for pair in stops.windows(2) {
        if t <= pair[1].0 {
            let f = (t - pair[0].0) / (pair[1].0 - pair[0].0);
            let a = pair[0].1[3] * (1.0 - f) + pair[1].1[3] * f;
            let mut out = [0.0; 4];
            out[3] = a;
            if a == 0.0 {
                return out;
            }
            for (i, c) in out[..3].iter_mut().enumerate() {
                let (mut l, mut r) = (pair[0].1[i], pair[1].1[i]);
                if space == "linear" {
                    l = crate::blend::to_linear(l);
                    r = crate::blend::to_linear(r);
                }
                *c = (l * pair[0].1[3] * (1.0 - f) + r * pair[1].1[3] * f) / a;
                if space == "linear" {
                    *c = crate::blend::to_srgb(*c);
                }
            }
            return out;
        }
    }
    stops.last().unwrap().1
}
pub(crate) fn render(
    raw: &Value,
    node: &Value,
    document: &Path,
    parent: kurbo::Affine,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<RgbaImage> {
    validate(raw, node)?;
    let fill = resolved(raw, &node["fill"])?;
    let styles = raw.get("styles").unwrap_or(&Value::Null);
    let kind = fill["kind"].as_str().unwrap();
    let bounds = (
        n(node, "x", 0.0)?,
        n(node, "y", 0.0)?,
        n(node, "width", 0.0)?,
        n(node, "height", 0.0)?,
    );
    let world = parent * crate::composite::transform(node)?;
    if world.determinant().abs() < 1e-12 {
        return Ok(RgbaImage::new(width, height));
    }
    let inv = world.inverse();
    let fill_inv = crate::composite::transform(fill)?.inverse();
    let stops = if matches!(kind, "linear" | "radial" | "conic") {
        Some(stops(fill, styles)?)
    } else {
        None
    };
    let solid = if kind == "solid" {
        Some(color(&fill["color"], styles)?)
    } else {
        None
    };
    let pattern = if kind == "pattern" {
        let bytes = crate::image::load_asset_bytes(
            raw,
            crate::resource::document_root(document),
            fill["asset"].as_str().unwrap(),
        )?;
        Some(crate::image::decode_source_pixels(&bytes)?.1)
    } else {
        None
    };
    let get = |key, default| n(fill, key, default).unwrap();
    let mut out = RgbaImage::new(width, height);
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        if x == 0 {
            crate::composite::check_cancelled()?;
        }
        let mut sum = [0.0; 4];
        for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
            let p = inv
                * kurbo::Point::new(
                    (f64::from(x) + sx) / f64::from(scale),
                    (f64::from(y) + sy) / f64::from(scale),
                );
            if p.x < bounds.0
                || p.y < bounds.1
                || p.x >= bounds.0 + bounds.2
                || p.y >= bounds.1 + bounds.3
            {
                continue;
            }
            let p = fill_inv * kurbo::Point::new(p.x - bounds.0, p.y - bounds.1);
            let c = if let Some(solid) = solid {
                solid
            } else if let Some(pattern) = &pattern {
                let px = (p.x - get("origin_x", 0.0)) / get("scale_x", 1.0);
                let py = (p.y - get("origin_y", 0.0)) / get("scale_y", 1.0);
                let repeat = fill["repeat"].as_str().unwrap_or("repeat");
                if ((!matches!(repeat, "repeat" | "repeat-x"))
                    && (px < 0.0 || px >= f64::from(pattern.width())))
                    || ((!matches!(repeat, "repeat" | "repeat-y"))
                        && (py < 0.0 || py >= f64::from(pattern.height())))
                {
                    continue;
                }
                let pixel = pattern.get_pixel(
                    px.rem_euclid(f64::from(pattern.width())).floor() as u32,
                    py.rem_euclid(f64::from(pattern.height())).floor() as u32,
                );
                [
                    f64::from(pixel[0]) / 255.0,
                    f64::from(pixel[1]) / 255.0,
                    f64::from(pixel[2]) / 255.0,
                    f64::from(pixel[3]) / 255.0,
                ]
            } else {
                let t = match kind {
                    "linear" => {
                        let (dx, dy) = (
                            get("x2", 1.0) - get("x1", 0.0),
                            get("y2", 0.0) - get("y1", 0.0),
                        );
                        ((p.x - get("x1", 0.0)) * dx + (p.y - get("y1", 0.0)) * dy)
                            / (dx * dx + dy * dy)
                    }
                    "radial" => {
                        let (dx, dy) = (p.x - get("cx", 0.0), p.y - get("cy", 0.0));
                        (dx * dx + dy * dy).sqrt() / get("radius", 1.0)
                    }
                    _ => {
                        (atan2(p.y - get("cy", 0.0), p.x - get("cx", 0.0))
                            - get("angle", 0.0) * std::f64::consts::PI / 180.0)
                            .rem_euclid(std::f64::consts::TAU)
                            / std::f64::consts::TAU
                    }
                };
                gradient(
                    stops.as_ref().unwrap(),
                    t,
                    fill["spread"].as_str().unwrap_or("pad"),
                    fill["space"].as_str().unwrap_or("srgb"),
                )
            };
            for i in 0..3 {
                sum[i] += c[i] * c[3] * 0.25;
            }
            sum[3] += c[3] * 0.25;
        }
        if sum[3] == 0.0 {
            *pixel = Rgba([0, 0, 0, 0]);
            continue;
        }
        let mut rgb = [0; 4];
        for i in 0..3 {
            rgb[i] = (sum[i] / sum[3] * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
        }
        rgb[3] = (sum[3] * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
        *pixel = Rgba(rgb);
    }
    Ok(out)
}

pub fn edit(
    raw: &mut Value,
    page: Option<&str>,
    operation: &str,
    id: &str,
    settings: &Value,
) -> Result<Value> {
    crate::composite::settings(
        settings,
        if operation == "add" {
            &["layer", "x", "y", "width", "height", "fill"]
        } else {
            &["x", "y", "width", "height", "fill"]
        },
    )?;
    if settings.as_object().unwrap().is_empty() {
        bail!("fill edit requires settings")
    }
    crate::scene::validate(raw)?;
    if !crate::composite::is_document(raw) {
        bail!("fill nodes require `migrate --target 6`")
    }
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    let result = if operation == "add" {
        if crate::scene::find_node_mut(selected, id).is_some() {
            bail!("fill node id already exists")
        }
        let layer = settings["layer"].as_str().unwrap_or("layer-1");
        crate::scene::ensure_layer_unlocked(selected, layer)?;
        let node = json!({"kind":"fill","id":id,"x":settings.get("x").cloned().unwrap_or(json!(0)),"y":settings.get("y").cloned().unwrap_or(json!(0)),
            "width":settings.get("width").cloned().unwrap_or(selected["canvas"]["width"].clone()),"height":settings.get("height").cloned().unwrap_or(selected["canvas"]["height"].clone()),"fill":settings["fill"]});
        crate::scene::layer_nodes_mut(selected, layer)?.push(node.clone());
        node
    } else if operation == "set" {
        crate::composite::ensure_unlocked(selected, id)?;
        let node = crate::scene::find_node_mut(selected, id).context("fill node missing")?;
        if node["kind"] != "fill" {
            bail!("node is not a fill")
        }
        for key in ["x", "y", "width", "height", "fill"] {
            if let Some(value) = settings.get(key) {
                node[key] = value.clone();
            }
        }
        node.clone()
    } else {
        bail!("fill operation must be add or set")
    };
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(result)
}

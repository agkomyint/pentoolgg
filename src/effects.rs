//! Ordered non-destructive appearance effects. Alpha silhouettes remain immutable.
use anyhow::{bail, Context, Result};
use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
use std::path::Path;
pub const MAX_EFFECTS: usize = 32;
pub const KINDS: [&str; 8] = [
    "drop-shadow",
    "inner-shadow",
    "outer-glow",
    "inner-glow",
    "stroke",
    "color-overlay",
    "gradient-overlay",
    "blur",
];

fn num(p: &Value, key: &str, default: f64, min: f64, max: f64) -> Result<f64> {
    let v = p.get(key).map_or(Ok(default), |v| {
        v.as_f64().context("effect parameter must be numeric")
    })?;
    if !v.is_finite() || !(min..=max).contains(&v) {
        bail!("[invalid-effect] {key} must be in {min}..={max}")
    }
    Ok(v)
}
pub fn stack<'a>(raw: &'a Value, node: &'a Value) -> Result<&'a [Value]> {
    let Some(value) = node.get("effects") else {
        return Ok(&[]);
    };
    let mut value = value;
    let mut depth = 0;
    while let Some(reference) = value.get("ref").and_then(Value::as_str) {
        depth += 1;
        if depth > 64 {
            bail!("effect style alias depth exceeded")
        }
        value = raw
            .get("styles")
            .and_then(|s| s.get(reference))
            .and_then(|s| s.get("value"))
            .or_else(|| value.get("fallback"))
            .context("effect style missing and has no fallback")?;
    }
    Ok(value
        .as_array()
        .context("effects must be an array or a style reference")?)
}
pub fn validate(raw: &Value, node: &Value) -> Result<()> {
    num(node, "content_opacity", 1.0, 0.0, 1.0)?;
    if let Some(origin) = node.get("effect_origin") {
        let origin = origin
            .as_array()
            .filter(|v| v.len() == 2)
            .context("effect_origin must contain two coordinates")?;
        if origin.iter().any(|v| {
            v.as_f64()
                .is_none_or(|v| !v.is_finite() || !(0.0..=16384.0).contains(&v))
        }) {
            bail!("effect_origin coordinates must be finite and within 0..16384")
        }
    }
    let stack = stack(raw, node)?;
    if stack.len() > MAX_EFFECTS {
        bail!("[limit-exceeded] at most 32 effects per node")
    }
    let mut ids = std::collections::HashSet::new();
    for op in stack {
        let id = op["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("effect ID missing")?;
        if !ids.insert(id) {
            bail!("duplicate effect ID {id}")
        }
        if op["enabled"].as_bool().is_none() {
            bail!("effect enabled must be boolean")
        }
        num(op, "opacity", 1.0, 0.0, 1.0)?;
        crate::blend::validate(
            op["blend_mode"]
                .as_str()
                .context("effect blend mode missing")?,
            op["blend_space"].as_str().unwrap_or("srgb"),
        )?;
        if op.get("blend_space").is_some_and(|v| !v.is_string()) {
            bail!("effect blend_space must be a string")
        }
        let kind = op["kind"]
            .as_str()
            .filter(|k| KINDS.contains(k))
            .context("unknown effect kind")?;
        let params = op["params"]
            .as_object()
            .context("effect params must be an object")?;
        let keys: &[&str] = match kind {
            "drop-shadow" | "inner-shadow" => &["x", "y", "blur", "color"],
            "outer-glow" | "inner-glow" => &["blur", "color"],
            "stroke" => &["size", "color", "position"],
            "color-overlay" => &["color"],
            "gradient-overlay" => &["fill"],
            _ => &["radius"],
        };
        for key in params.keys() {
            if !keys.contains(&key.as_str()) {
                bail!(
                    "unknown effect parameter {key} for {kind}; valid parameters: {}",
                    keys.join(", ")
                )
            }
        }
        let p = &op["params"];
        if matches!(kind, "drop-shadow" | "inner-shadow") {
            num(p, "x", 0.0, -16384.0, 16384.0)?;
            num(p, "y", 0.0, -16384.0, 16384.0)?;
        }
        if matches!(
            kind,
            "drop-shadow" | "inner-shadow" | "outer-glow" | "inner-glow"
        ) {
            num(p, "blur", 0.0, 0.0, 256.0)?;
        }
        if kind == "blur" {
            num(p, "radius", 0.0, 0.0, 256.0)?;
        }
        if kind == "stroke" {
            num(p, "size", 1.0, 0.0, 128.0)?;
            if !matches!(
                p["position"].as_str().unwrap_or("outer"),
                "outer" | "inner" | "center"
            ) {
                bail!("unknown stroke position")
            }
        }
        if kind == "gradient-overlay" {
            if !p.get("fill").is_some_and(Value::is_object) {
                bail!(
                    "gradient-overlay requires --params '{{\"fill\":{{\"kind\":\"linear\",\"stops\":[{{\"offset\":0,\"color\":\"#000000\"}},{{\"offset\":1,\"color\":\"#ffffff\"}}]}}}}' (a gradient fill object)"
                )
            }
            let fill = json!({"kind":"fill","id":"effect-fill","x":0,"y":0,"width":raw["pages"][0]["canvas"]["width"],"height":raw["pages"][0]["canvas"]["height"],"fill":p["fill"]});
            crate::fill::validate(raw, &fill)?;
            if !matches!(
                crate::fill::resolved(raw, &p["fill"])?["kind"].as_str(),
                Some("linear" | "radial" | "conic")
            ) {
                bail!("gradient overlay requires a gradient fill")
            }
        } else if kind != "blur" {
            crate::fill::color(
                p.get("color").unwrap_or(&json!("#000000")),
                raw.get("styles").unwrap_or(&Value::Null),
            )?;
        }
    }
    Ok(())
}
fn byte(v: f64) -> u8 {
    (v + 0.5).floor().clamp(0.0, 255.0) as u8
}
fn blurred(image: RgbaImage, radius: f64) -> Result<RgbaImage> {
    if radius > 256.0 {
        bail!("[limit-exceeded] scaled effect blur exceeds 256 pixels")
    }
    crate::imageops::apply_stack(
        image,
        &[
            json!({"id":"effect-blur","kind":"blur","version":1,"enabled":true,"params":{"radius":radius}}),
        ],
    )
}
// Two-pass octagonal distance metric (orthogonal=1, diagonal=sqrt(2)), pinned v1.
fn distance(shape: &RgbaImage, inside: bool) -> Vec<f64> {
    let (w, h) = (shape.width() as usize, shape.height() as usize);
    let mut out: Vec<f64> = shape
        .pixels()
        .map(|p| if (p[3] > 0) == inside { 0.0 } else { 1e12 })
        .collect::<Vec<_>>();
    let diagonal = 2.0_f64.sqrt();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            for (dx, dy, step) in [
                (-1, 0, 1.0),
                (-1, -1, diagonal),
                (0, -1, 1.0),
                (1, -1, diagonal),
            ] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx >= 0 && ny >= 0 && (nx as usize) < w {
                    out[i] = out[i].min(out[ny as usize * w + nx as usize] + step);
                }
            }
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            for (dx, dy, step) in [
                (1, 0, 1.0),
                (-1, 1, diagonal),
                (0, 1, 1.0),
                (1, 1, diagonal),
            ] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx >= 0 && (nx as usize) < w && (ny as usize) < h {
                    out[i] = out[i].min(out[ny as usize * w + nx as usize] + step);
                }
            }
        }
    }
    out
}
pub(crate) fn apply(
    raw: &Value,
    node: &Value,
    source: &RgbaImage,
    document: &Path,
    scale: f32,
) -> Result<RgbaImage> {
    let mut result = source.clone();
    let content = num(node, "content_opacity", 1.0, 0.0, 1.0)?;
    for pixel in result.pixels_mut() {
        pixel[3] = byte(f64::from(pixel[3]) * content);
    }
    for op in stack(raw, node)? {
        crate::composite::check_cancelled()?;
        if op["enabled"] != true {
            continue;
        }
        let kind = op["kind"].as_str().unwrap();
        let p = &op["params"];
        let opacity = num(op, "opacity", 1.0, 0.0, 1.0)?;
        let mode = op["blend_mode"].as_str().unwrap();
        let space = op["blend_space"].as_str().unwrap_or("srgb");
        let get = |key, default| p.get(key).and_then(Value::as_f64).unwrap_or(default);
        if kind == "blur" {
            let mut blur = blurred(result.clone(), get("radius", 0.0) * f64::from(scale))?;
            if mode != "normal" || space != "srgb" {
                for (pixel, original) in blur.pixels_mut().zip(result.pixels()) {
                    if original[3] == 0 {
                        continue;
                    }
                    let mut b = [0.0; 3];
                    let mut s = [0.0; 3];
                    for i in 0..3 {
                        b[i] = f64::from(original[i]) / 255.0;
                        s[i] = f64::from(pixel[i]) / 255.0;
                        if space == "linear" {
                            b[i] = crate::blend::to_linear(b[i]);
                            s[i] = crate::blend::to_linear(s[i]);
                        }
                    }
                    let mixed = crate::blend::rgb(mode, b, s);
                    for i in 0..3 {
                        pixel[i] = byte(
                            255.0
                                * if space == "linear" {
                                    crate::blend::to_srgb(mixed[i].clamp(0.0, 1.0))
                                } else {
                                    mixed[i]
                                },
                        );
                    }
                }
            }
            for (pixel, blur) in result.pixels_mut().zip(blur.pixels()) {
                let a = f64::from(pixel[3]) * (1.0 - opacity) + f64::from(blur[3]) * opacity;
                for i in 0..3 {
                    pixel[i] = if a == 0.0 {
                        0
                    } else {
                        byte(
                            (f64::from(pixel[i]) * f64::from(pixel[3]) * (1.0 - opacity)
                                + f64::from(blur[i]) * f64::from(blur[3]) * opacity)
                                / a,
                        )
                    };
                }
                pixel[3] = byte(a);
            }
            continue;
        }
        let default = json!("#000000");
        let color = if kind == "gradient-overlay" {
            None
        } else {
            Some(crate::fill::color(
                p.get("color").unwrap_or(&default),
                raw.get("styles").unwrap_or(&Value::Null),
            )?)
        };
        let mut effect = RgbaImage::new(source.width(), source.height());
        if kind == "gradient-overlay" {
            let ox = node["effect_origin"][0].as_f64().unwrap_or(0.0);
            let oy = node["effect_origin"][1].as_f64().unwrap_or(0.0);
            let fill = json!({"kind":"fill","id":"effect-gradient","x":-ox,"y":-oy,"width":f64::from(source.width())/f64::from(scale)+ox,"height":f64::from(source.height())/f64::from(scale)+oy,"fill":p["fill"]});
            effect = crate::fill::render(
                raw,
                &fill,
                document,
                kurbo::Affine::IDENTITY,
                source.width(),
                source.height(),
                scale,
            )?;
            for (pixel, base) in effect.pixels_mut().zip(source.pixels()) {
                pixel[3] = byte(f64::from(pixel[3]) * f64::from(base[3]) / 255.0 * opacity);
            }
        } else if kind == "color-overlay" {
            let c = color.unwrap();
            for (pixel, base) in effect.pixels_mut().zip(source.pixels()) {
                *pixel = Rgba([
                    byte(c[0] * 255.0),
                    byte(c[1] * 255.0),
                    byte(c[2] * 255.0),
                    byte(f64::from(base[3]) * c[3] * opacity),
                ]);
            }
        } else if kind == "stroke" {
            let c = color.unwrap();
            let position = p["position"].as_str().unwrap_or("outer");
            let outside = distance(source, true);
            let inside = distance(source, false);
            let size = get("size", 1.0) * f64::from(scale);
            for (index, (pixel, base)) in effect.pixels_mut().zip(source.pixels()).enumerate() {
                let visible = match position {
                    "inner" => base[3] > 0 && inside[index] <= size,
                    "center" => {
                        if base[3] > 0 {
                            inside[index] <= size / 2.0
                        } else {
                            outside[index] <= size / 2.0
                        }
                    }
                    _ => base[3] == 0 && outside[index] <= size,
                };
                *pixel = Rgba([
                    byte(c[0] * 255.0),
                    byte(c[1] * 255.0),
                    byte(c[2] * 255.0),
                    if visible {
                        byte(c[3] * 255.0 * opacity)
                    } else {
                        0
                    },
                ]);
            }
        } else {
            let inner = matches!(kind, "inner-shadow" | "inner-glow");
            let c = color.unwrap();
            let (dx, dy) = (
                (get("x", 0.0) * f64::from(scale)).round() as i64,
                (get("y", 0.0) * f64::from(scale)).round() as i64,
            );
            for (x, y, pixel) in effect.enumerate_pixels_mut() {
                let (sx, sy) = (i64::from(x) - dx, i64::from(y) - dy);
                let alpha = if sx >= 0
                    && sy >= 0
                    && sx < i64::from(source.width())
                    && sy < i64::from(source.height())
                {
                    source.get_pixel(sx as u32, sy as u32)[3]
                } else {
                    0
                };
                *pixel = Rgba([255, 255, 255, if inner { 255 - alpha } else { alpha }]);
            }
            effect = blurred(effect, get("blur", 0.0) * f64::from(scale))?;
            for (pixel, base) in effect.pixels_mut().zip(source.pixels()) {
                let alpha = f64::from(pixel[3])
                    * if inner {
                        f64::from(base[3]) / 255.0
                    } else if kind == "outer-glow" {
                        1.0 - f64::from(base[3]) / 255.0
                    } else {
                        1.0
                    };
                *pixel = Rgba([
                    byte(c[0] * 255.0),
                    byte(c[1] * 255.0),
                    byte(c[2] * 255.0),
                    byte(alpha * c[3] * opacity),
                ]);
            }
        }
        if matches!(
            kind,
            "color-overlay" | "gradient-overlay" | "inner-shadow" | "inner-glow"
        ) {
            for ((pixel, effect), base) in result
                .pixels_mut()
                .zip(effect.pixels())
                .zip(source.pixels())
            {
                if base[3] == 0 || effect[3] == 0 {
                    continue;
                }
                let amount = f64::from(effect[3]) / f64::from(base[3]);
                let mut b = [0.0; 3];
                let mut s = [0.0; 3];
                for i in 0..3 {
                    b[i] = f64::from(if pixel[3] == 0 { base[i] } else { pixel[i] }) / 255.0;
                    s[i] = f64::from(effect[i]) / 255.0;
                    if space == "linear" {
                        b[i] = crate::blend::to_linear(b[i]);
                        s[i] = crate::blend::to_linear(s[i]);
                    }
                }
                let mixed = crate::blend::rgb(mode, b, s);
                for i in 0..3 {
                    let c = 255.0
                        * if space == "linear" {
                            crate::blend::to_srgb(mixed[i].clamp(0.0, 1.0))
                        } else {
                            mixed[i]
                        };
                    pixel[i] = if pixel[3] == 0 {
                        byte(c)
                    } else {
                        byte(f64::from(pixel[i]) * (1.0 - amount) + c * amount)
                    };
                }
                pixel[3] = pixel[3].max(effect[3]);
            }
        } else if kind == "drop-shadow" || kind == "outer-glow" {
            crate::blend::over(&mut effect, &result, mode, space)?;
            result = effect;
        } else {
            crate::blend::over(&mut result, &effect, mode, space)?;
        }
    }
    Ok(result)
}

/// The sub-rectangle `(x, y, width, height)` of `surface` an effect stack needs: the
/// visible content grown by the stack's reach. Position-dependent stacks
/// (gradient overlays) use the whole surface; empty content yields zero size.
pub(crate) fn region(
    raw: &Value,
    node: &Value,
    surface: &RgbaImage,
    scale: f32,
) -> Result<(u32, u32, u32, u32)> {
    let (w, h) = surface.dimensions();
    let ops = stack(raw, node)?;
    if ops.iter().any(|op| op["kind"] == "gradient-overlay") {
        return Ok((0, 0, w, h));
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (x, y, p) in surface.enumerate_pixels() {
        if p[3] > 0 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return Ok((0, 0, 0, 0));
    }
    let scale = f64::from(scale);
    let mut reach = 4.0;
    for op in ops.iter().filter(|op| op["enabled"] == true) {
        let p = &op["params"];
        let get = |key: &str| p.get(key).and_then(Value::as_f64).unwrap_or(0.0).abs();
        reach += (get("blur") + get("radius")) * scale * 4.0
            + (get("x") + get("y") + get("size")) * scale
            + 2.0;
    }
    // Inner effects read the complement of the alpha; stay on the canvas edge there.
    let reach = reach.min(f64::from(w.max(h))).ceil() as u32;
    let nx0 = x0.saturating_sub(reach);
    let ny0 = y0.saturating_sub(reach);
    let nx1 = x1.saturating_add(reach).min(w);
    let ny1 = y1.saturating_add(reach).min(h);
    Ok((nx0, ny0, nx1 - nx0, ny1 - ny0))
}

/// A node's effect stack in order, without mutating the document.
pub fn list(raw: &Value, page: Option<&str>, id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    if !crate::composite::is_document(raw) {
        bail!("effects require `migrate --target 6`")
    }
    let mut copy = raw.clone();
    let selected = crate::scene::page_mut(&mut copy, page)?;
    let node = crate::scene::find_node_mut(selected, id).context("effect node missing")?;
    let effects = node.get("effects").cloned().unwrap_or(json!([]));
    Ok(json!({"node":id,"effects":effects}))
}

pub fn edit(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    operation: &str,
    op_id: &str,
    settings: &Value,
) -> Result<Value> {
    let allowed: &[&str] = match operation {
        "add" => &[
            "kind",
            "params",
            "opacity",
            "blend_mode",
            "blend_space",
            "index",
        ],
        "set" => &["kind", "params", "opacity", "blend_mode", "blend_space"],
        "move" => &["index"],
        _ => &[],
    };
    crate::composite::settings(settings, allowed)?;
    if operation == "set" && settings.as_object().unwrap().is_empty() {
        bail!("effect set requires settings")
    }
    crate::scene::validate(raw)?;
    if !crate::composite::is_document(raw) {
        bail!("effects require `migrate --target 6`")
    }
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    crate::composite::ensure_unlocked(selected, id)?;
    let node = crate::scene::find_node_mut(selected, id).context("effect node missing")?;
    if node["kind"] == "adjustment" {
        bail!("adjustments do not have content effects")
    }
    let stack = node
        .as_object_mut()
        .unwrap()
        .entry("effects")
        .or_insert(json!([]))
        .as_array_mut()
        .context("detach the shared effect style before editing its stack")?;
    let at = stack.iter().position(|op| op["id"] == op_id);
    match operation {
        "add" => {
            if at.is_some() {
                bail!("effect id already exists")
            }
            let op = json!({"id":op_id,"kind":settings["kind"],"params":settings.get("params").cloned().unwrap_or(json!({})),"enabled":true,
                "opacity":settings.get("opacity").cloned().unwrap_or(json!(1)),"blend_mode":settings.get("blend_mode").cloned().unwrap_or(json!("normal")),"blend_space":settings.get("blend_space").cloned().unwrap_or(json!("srgb"))});
            let to = settings["index"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(stack.len());
            if to > stack.len() {
                bail!("effect index out of bounds")
            }
            stack.insert(to, op);
        }
        "set" => {
            let op = &mut stack[at.context("effect id missing")?];
            for key in ["kind", "params", "opacity", "blend_mode", "blend_space"] {
                if let Some(value) = settings.get(key) {
                    op[key] = value.clone();
                }
            }
        }
        "enable" | "disable" => {
            stack[at.context("effect id missing")?]["enabled"] = json!(operation == "enable");
        }
        "remove" => {
            stack.remove(at.context("effect id missing")?);
        }
        "move" => {
            let at = at.context("effect id missing")?;
            let to = settings["index"]
                .as_u64()
                .context("effect move requires index")? as usize;
            if to >= stack.len() {
                bail!("effect index out of bounds")
            }
            let op = stack.remove(at);
            stack.insert(to, op);
        }
        _ => bail!("unknown effect operation"),
    }
    let result = json!({"node":id,"effects":stack});
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(result)
}

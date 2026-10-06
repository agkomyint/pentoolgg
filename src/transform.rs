//! Non-destructive coordinate stacks. Every output samples the original surface once.
use anyhow::{bail, Context, Result};
use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
pub const MAX_TRANSFORMS: usize = 64;

fn mul(a: [f64; 9], b: [f64; 9]) -> [f64; 9] {
    let mut out = [0.0; 9];
    for row in 0..3 {
        for col in 0..3 {
            for k in 0..3 {
                out[row * 3 + col] += a[row * 3 + k] * b[k * 3 + col];
            }
        }
    }
    out
}
fn inverse(m: [f64; 9]) -> Result<[f64; 9]> {
    let [a, b, c, d, e, f, g, h, i] = m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if !det.is_finite() || det.abs() < 1e-12 {
        bail!("[invalid-transform] matrix must be invertible")
    }
    Ok([
        (e * i - f * h) / det,
        (c * h - b * i) / det,
        (b * f - c * e) / det,
        (f * g - d * i) / det,
        (a * i - c * g) / det,
        (c * d - a * f) / det,
        (d * h - e * g) / det,
        (b * g - a * h) / det,
        (a * e - b * d) / det,
    ])
}
fn matrix(value: &Value) -> Result<[f64; 9]> {
    let values = value
        .as_array()
        .filter(|a| a.len() == 9)
        .context("[invalid-transform] matrix needs nine numbers")?;
    let mut out = [0.0; 9];
    for (i, v) in values.iter().enumerate() {
        out[i] = v
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1e9)
            .context("[invalid-transform] matrix must be finite and bounded")?;
    }
    inverse(out)?;
    Ok(out)
}
fn num(p: &Value, key: &str, default: f64) -> Result<f64> {
    p.get(key).map_or(Ok(default), |v| {
        v.as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1e6)
            .context("[invalid-transform] parameter must be finite and bounded")
    })
}
// Fixed arithmetic avoids platform-dependent libm trigonometric routines.
fn sin_cos(degrees: f64) -> (f64, f64) {
    let x = ((degrees + 180.0).rem_euclid(360.0) - 180.0) * std::f64::consts::PI / 180.0;
    let (mut s, mut c, mut st, mut ct) = (x, 1.0, x, 1.0);
    for i in 1..=16 {
        let n = f64::from(i);
        st *= -x * x / ((2.0 * n) * (2.0 * n + 1.0));
        ct *= -x * x / ((2.0 * n - 1.0) * (2.0 * n));
        s += st;
        c += ct;
    }
    (s, c)
}
fn homography(rect: [f64; 4], quad: &Value) -> Result<[f64; 9]> {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        bail!("[invalid-transform] perspective source bounds must be nonempty")
    }
    let points = quad
        .as_array()
        .filter(|q| q.len() == 4)
        .context("[invalid-transform] quad must have four [x,y] corners")?;
    let mut dst = [[0.0; 2]; 4];
    for (i, p) in points.iter().enumerate() {
        let pair = p
            .as_array()
            .filter(|a| a.len() == 2)
            .context("quad corners need two values")?;
        for (j, v) in pair.iter().enumerate() {
            dst[i][j] = v
                .as_f64()
                .filter(|n| n.is_finite() && n.abs() <= 1e6)
                .context("quad coordinates must be finite and bounded")?;
        }
    }
    let mut sign = 0.0;
    for i in 0..4 {
        let (a, b, c) = (dst[i], dst[(i + 1) % 4], dst[(i + 2) % 4]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() < 1e-9 || (sign != 0.0 && sign * cross < 0.0) {
            bail!(
                "[invalid-transform] quad must be strictly convex and ordered around its perimeter"
            )
        }
        sign = cross;
    }
    let src = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    let mut equations = [[0.0; 9]; 8];
    for i in 0..4 {
        let [x, y] = src[i];
        let [u, v] = dst[i];
        equations[i * 2] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        equations[i * 2 + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for col in 0..8 {
        let pivot = (col..8)
            .max_by(|a, b| {
                equations[*a][col]
                    .abs()
                    .total_cmp(&equations[*b][col].abs())
            })
            .unwrap();
        equations.swap(col, pivot);
        let divisor = equations[col][col];
        if divisor.abs() < 1e-12 {
            bail!("[invalid-transform] degenerate perspective quad")
        }
        for v in &mut equations[col][col..] {
            *v /= divisor;
        }
        let row = equations[col];
        for (i, other) in equations.iter_mut().enumerate() {
            if i == col {
                continue;
            }
            let factor = other[col];
            for j in col..9 {
                other[j] -= factor * row[j];
            }
        }
    }
    let mut out = [0.0; 9];
    for i in 0..8 {
        out[i] = equations[i][8];
    }
    out[8] = 1.0;
    inverse(out)?;
    Ok(out)
}

pub fn make(kind: &str, params: &Value, bounds: [f64; 4]) -> Result<Value> {
    let object = params
        .as_object()
        .context("transform params must be an object")?;
    let keys: &[&str] = match kind {
        "scale" => &["x", "y"],
        "rotate" => &["degrees"],
        "skew" => &["x", "y"],
        "translate" => &["x", "y"],
        "perspective" | "four-corner" => &["quad"],
        "matrix" => &["matrix"],
        _ => bail!("[unsupported-capability] unknown transform {kind}; mesh warp is not specified"),
    };
    for key in object.keys() {
        if !keys.contains(&key.as_str()) {
            bail!("unknown transform parameter {key}")
        }
    }
    let m = match kind {
        "scale" => [
            num(params, "x", 1.0)?,
            0.0,
            0.0,
            0.0,
            num(params, "y", 1.0)?,
            0.0,
            0.0,
            0.0,
            1.0,
        ],
        "translate" => [
            1.0,
            0.0,
            num(params, "x", 0.0)?,
            0.0,
            1.0,
            num(params, "y", 0.0)?,
            0.0,
            0.0,
            1.0,
        ],
        "rotate" => {
            let (s, c) = sin_cos(num(params, "degrees", 0.0)?);
            [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0]
        }
        "skew" => {
            let (x, y) = (num(params, "x", 0.0)?, num(params, "y", 0.0)?);
            if x.abs() > 80.0 || y.abs() > 80.0 {
                bail!("skew angles must be in -80..80")
            }
            let (sx, cx) = sin_cos(x);
            let (sy, cy) = sin_cos(y);
            [1.0, sx / cx, 0.0, sy / cy, 1.0, 0.0, 0.0, 0.0, 1.0]
        }
        "perspective" | "four-corner" => homography(bounds, &params["quad"])?,
        _ => matrix(&params["matrix"])?,
    };
    matrix(&json!(m))?;
    Ok(json!(m))
}
pub fn validate(node: &Value) -> Result<()> {
    let Some(stack) = node.get("transforms") else {
        return Ok(());
    };
    let stack = stack
        .as_array()
        .filter(|a| a.len() <= MAX_TRANSFORMS)
        .context("[limit-exceeded] transforms must be an array of at most 64 entries")?;
    let mut ids = std::collections::HashSet::new();
    for op in stack {
        let id = op["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("transform id missing")?;
        if !ids.insert(id) {
            bail!("duplicate transform id {id}")
        }
        if !matches!(
            op["kind"].as_str(),
            Some(
                "scale"
                    | "rotate"
                    | "skew"
                    | "translate"
                    | "perspective"
                    | "four-corner"
                    | "matrix"
            )
        ) {
            bail!("unknown transform kind")
        }
        if op["enabled"].as_bool().is_none() {
            bail!("transform enabled must be boolean")
        }
        let opacity = num(op, "opacity", 1.0)?;
        if !(0.0..=1.0).contains(&opacity) {
            bail!("transform opacity must be in 0..1")
        }
        matrix(&op["matrix"])?;
        if let Some(mask) = op.get("mask") {
            crate::mask::validate_attachment(mask)?;
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
pub fn edit(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    operation: &str,
    op_id: &str,
    kind: Option<&str>,
    params: &Value,
    index: Option<usize>,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    if !crate::composite::is_document(raw) {
        bail!("transform stacks require `migrate --target 6`")
    }
    let bounds = crate::scene::node_bounds(raw, page, id)?;
    let rect = [
        bounds["x"].as_f64().unwrap(),
        bounds["y"].as_f64().unwrap(),
        bounds["width"].as_f64().unwrap(),
        bounds["height"].as_f64().unwrap(),
    ];
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    crate::composite::ensure_unlocked(selected, id)?;
    let node = crate::scene::find_node_mut(selected, id).context("transform node missing")?;
    if node["kind"] == "adjustment" {
        bail!("adjustments have no transformable content")
    }
    let stack = node
        .as_object_mut()
        .unwrap()
        .entry("transforms")
        .or_insert(json!([]))
        .as_array_mut()
        .context("transform stack malformed")?;
    let at = stack.iter().position(|op| op["id"] == op_id);
    match operation {
        "add" => {
            if at.is_some() {
                bail!("transform id already exists")
            }
            let kind = kind.context("transform kind required")?;
            let matrix = make(kind, params, rect)?;
            let op = json!({"id":op_id,"kind":kind,"enabled":true,"opacity":1,"matrix":matrix});
            let index = index.unwrap_or(stack.len());
            if index > stack.len() {
                bail!("transform index out of bounds")
            }
            stack.insert(index, op);
        }
        "set" => {
            let at = at.context("transform id missing")?;
            let kind = kind
                .unwrap_or(stack[at]["kind"].as_str().unwrap())
                .to_owned();
            let matrix = make(&kind, params, rect)?;
            stack[at]["matrix"] = matrix;
            stack[at]["kind"] = json!(kind);
        }
        "enable" | "disable" => {
            stack[at.context("transform id missing")?]["enabled"] = json!(operation == "enable");
        }
        "remove" => {
            stack.remove(at.context("transform id missing")?);
        }
        "move" => {
            let at = at.context("transform id missing")?;
            let to = index.context("transform move requires --index")?;
            if to >= stack.len() {
                bail!("transform index out of bounds")
            }
            let op = stack.remove(at);
            stack.insert(to, op);
        }
        _ => bail!("unknown transform stack operation"),
    }
    let result = json!({"node":id,"transforms":stack});
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(result)
}

fn sample(image: &RgbaImage, x: f64, y: f64) -> Rgba<u8> {
    if !x.is_finite()
        || !y.is_finite()
        || x < -1.0
        || y < -1.0
        || x > f64::from(image.width())
        || y > f64::from(image.height())
    {
        return Rgba([0, 0, 0, 0]);
    }
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let mut sum = [0.0; 4];
    for (dx, dy, w) in [
        (0, 0, (1.0 - fx) * (1.0 - fy)),
        (1, 0, fx * (1.0 - fy)),
        (0, 1, (1.0 - fx) * fy),
        (1, 1, fx * fy),
    ] {
        let (px, py) = (ix + dx, iy + dy);
        if px < 0 || py < 0 || px >= i64::from(image.width()) || py >= i64::from(image.height()) {
            continue;
        }
        let p = image.get_pixel(px as u32, py as u32);
        let a = f64::from(p[3]) / 255.0;
        for i in 0..3 {
            sum[i] += w * f64::from(p[i]) * a;
        }
        sum[3] += w * a;
    }
    if sum[3] == 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let mut out = [0; 4];
    for i in 0..3 {
        out[i] = (sum[i] / sum[3] + 0.5).floor().clamp(0.0, 255.0) as u8;
    }
    out[3] = (sum[3] * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
    Rgba(out)
}

pub(crate) fn affine(node: &Value) -> Result<Option<kurbo::Affine>> {
    let Some(stack) = node.get("transforms").and_then(Value::as_array) else {
        return Ok(Some(kurbo::Affine::IDENTITY));
    };
    let mut combined = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    for op in stack {
        if op["enabled"] != true {
            continue;
        }
        if op.get("mask").is_some() || num(op, "opacity", 1.0)? != 1.0 {
            return Ok(None);
        }
        combined = mul(matrix(&op["matrix"])?, combined);
    }
    if combined[6].abs() > 1e-12 || combined[7].abs() > 1e-12 || combined[8].abs() < 1e-12 {
        return Ok(None);
    }
    let k = combined[8];
    Ok(Some(kurbo::Affine::new([
        combined[0] / k,
        combined[3] / k,
        combined[1] / k,
        combined[4] / k,
        combined[2] / k,
        combined[5] / k,
    ])))
}

pub fn attach_mask(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    op_id: &str,
    name: &str,
) -> Result<()> {
    crate::scene::validate(raw)?;
    let digest = raw["masks"][name]
        .as_str()
        .context("transform mask name missing")?
        .to_owned();
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    crate::composite::ensure_unlocked(selected, id)?;
    let node = crate::scene::find_node_mut(selected, id).context("transform node missing")?;
    let op = node["transforms"]
        .as_array_mut()
        .context("transforms missing")?
        .iter_mut()
        .find(|op| op["id"] == op_id)
        .context("transform id missing")?;
    op["mask"] = json!({"resource":digest,"invert":false,"density":1,"feather":0,"linked":false,"transform":[1,0,0,1,0,0]});
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(())
}

/// Edit weighting/mask independently without resetting the stored homography.
pub fn metadata(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    op_id: &str,
    settings: &Value,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    crate::composite::settings(settings, &["opacity", "mask"])?;
    if settings.as_object().unwrap().is_empty() {
        bail!("transform metadata requires opacity or mask")
    }
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    crate::composite::ensure_unlocked(selected, id)?;
    let node = crate::scene::find_node_mut(selected, id).context("transform node missing")?;
    let operation = node["transforms"]
        .as_array_mut()
        .context("transforms missing")?
        .iter_mut()
        .find(|op| op["id"] == op_id)
        .context("transform id missing")?;
    if let Some(opacity) = settings.get("opacity") {
        operation["opacity"] = opacity.clone();
    }
    if settings.get("mask").is_some_and(Value::is_null) {
        operation
            .as_object_mut()
            .unwrap()
            .remove("mask")
            .context("transform has no mask to detach")?;
    }
    if let Some(mask) = settings.get("mask").filter(|m| !m.is_null()) {
        attach_mask(
            &mut candidate,
            page,
            id,
            op_id,
            mask.as_str()
                .context("mask must be a named resource or null")?,
        )?;
    }
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"node":id,"transform":op_id,"settings":settings}))
}
pub(crate) fn warp(
    source: &RgbaImage,
    stack: &[Value],
    masks: &[Option<Vec<u8>>],
    scale: f32,
) -> Result<RgbaImage> {
    let mut inverse_stack = Vec::new();
    // Unmasked fully enabled matrices combine algebraically before sampling.
    let mut combined = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    if stack.iter().all(|op| {
        op.get("mask").is_none() && op.get("opacity").and_then(Value::as_f64).unwrap_or(1.0) == 1.0
    }) {
        for op in stack {
            if op["enabled"] == true {
                combined = mul(matrix(&op["matrix"])?, combined);
            }
        }
        inverse_stack.push((inverse(combined)?, 1.0, None));
    } else {
        for (i, op) in stack.iter().enumerate().rev() {
            if op["enabled"] == true {
                inverse_stack.push((
                    inverse(matrix(&op["matrix"])?)?,
                    num(op, "opacity", 1.0)?,
                    masks[i].as_ref(),
                ));
            }
        }
    }
    let mut out = RgbaImage::new(source.width(), source.height());
    let scale = f64::from(scale);
    for (x, y, pixel) in out.enumerate_pixels_mut() {
        if x == 0 {
            crate::composite::check_cancelled()?;
        }
        let (mut px, mut py) = ((f64::from(x) + 0.5) / scale, (f64::from(y) + 0.5) / scale);
        for (m, opacity, mask) in &inverse_stack {
            let coverage = mask.map_or(1.0, |mask| {
                let (x, y) = ((px * scale).floor() as i64, (py * scale).floor() as i64);
                if x < 0
                    || y < 0
                    || x >= i64::from(source.width())
                    || y >= i64::from(source.height())
                {
                    0.0
                } else {
                    f64::from(mask[(y as u32 * source.width() + x as u32) as usize]) / 255.0
                }
            });
            let factor = opacity * coverage;
            if factor == 0.0 {
                continue;
            }
            let w = m[6] * px + m[7] * py + m[8];
            if w.abs() < 1e-12 {
                px = f64::INFINITY;
                break;
            }
            let nx = (m[0] * px + m[1] * py + m[2]) / w;
            let ny = (m[3] * px + m[4] * py + m[5]) / w;
            px += (nx - px) * factor;
            py += (ny - py) * factor;
        }
        *pixel = sample(source, px * scale - 0.5, py * scale - 0.5);
    }
    Ok(out)
}

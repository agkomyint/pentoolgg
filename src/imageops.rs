//! Versioned, deterministic non-destructive image operation engine (v1).
//!
//! Pinned contract: pixels are 8-bit sRGB RGBA with straight alpha at rest.
//! Filters that mix neighbouring pixels (resize, blur, sharpen) work on
//! premultiplied alpha and un-premultiply with rounding. Colour-channel
//! operations (brightness, contrast, levels, curves, hue, saturation,
//! grayscale) work on straight RGB and never change alpha. All math uses
//! integers or IEEE `+ - * /` and `sqrt` only, so results are identical on
//! every release target. Every stage rounds half-up and clamps to 0–255.
use anyhow::{bail, Context, Result};
use image::RgbaImage;
use serde_json::{json, Map, Value};
use std::collections::HashSet;

pub const ENGINE_VERSION: u64 = 1;
pub const MAX_OPERATIONS: usize = 64;
pub const KINDS: [&str; 10] = [
    "crop",
    "resize",
    "rotate",
    "brightness-contrast",
    "levels",
    "curves",
    "hue-saturation",
    "blur",
    "sharpen",
    "grayscale",
];

fn num(
    params: &Map<String, Value>,
    key: &str,
    min: f64,
    max: f64,
    default: Option<f64>,
) -> Result<f64> {
    let value = match params.get(key) {
        Some(value) => value
            .as_f64()
            .filter(|n| n.is_finite())
            .with_context(|| format!("[invalid-operation] {key} must be a finite number"))?,
        None => default.with_context(|| format!("[invalid-operation] {key} is required"))?,
    };
    if value < min || value > max {
        bail!("[invalid-operation] {key} must be in {min}–{max}")
    }
    Ok(value)
}

fn known(params: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    for key in params.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("[invalid-operation] unknown parameter {key}")
        }
    }
    Ok(())
}

/// Validate one operation object's shape and pinned parameter ranges.
pub fn validate_operation(operation: &Value) -> Result<()> {
    let object = operation
        .as_object()
        .context("[invalid-operation] operation must be an object")?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 128)
        .context("[invalid-operation] operation id must be a nonempty string")?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .with_context(|| format!("[invalid-operation] operation {id} kind is missing"))?;
    if !KINDS.contains(&kind) {
        bail!("[unsupported-capability] operation {id} has unknown kind {kind}")
    }
    if object.get("version").and_then(Value::as_u64) != Some(ENGINE_VERSION) {
        bail!("[unsupported-capability] operation {id} requires engine version {ENGINE_VERSION}")
    }
    if !object.get("enabled").is_some_and(Value::is_boolean) {
        bail!("[invalid-operation] operation {id} enabled must be a boolean")
    }
    let params = object
        .get("params")
        .and_then(Value::as_object)
        .with_context(|| format!("[invalid-operation] operation {id} params must be an object"))?;
    for key in object.keys() {
        if !["id", "kind", "version", "enabled", "params"].contains(&key.as_str()) {
            bail!("[invalid-operation] operation {id} has unknown field {key}")
        }
    }
    (|| -> Result<()> {
        match kind {
            "crop" => {
                known(params, &["x", "y", "width", "height"])?;
                let x = num(params, "x", 0.0, 1.0, None)?;
                let y = num(params, "y", 0.0, 1.0, None)?;
                let w = num(params, "width", 0.0, 1.0, None)?;
                let h = num(params, "height", 0.0, 1.0, None)?;
                if w <= 0.0 || h <= 0.0 || x + w > 1.0 || y + h > 1.0 {
                    bail!(
                        "[invalid-operation] crop rectangle must be nonempty and inside the image"
                    )
                }
            }
            "resize" => {
                known(params, &["width", "height"])?;
                for key in ["width", "height"] {
                    let n = num(params, key, 1.0, crate::image::MAX_DIMENSION as f64, None)?;
                    if n.fract() != 0.0 {
                        bail!("[invalid-operation] {key} must be an integer")
                    }
                }
            }
            "rotate" => {
                known(params, &["degrees"])?;
                let d = num(params, "degrees", 0.0, 270.0, None)?;
                if d % 90.0 != 0.0 {
                    bail!("[invalid-operation] degrees must be 0, 90, 180, or 270")
                }
            }
            "brightness-contrast" => {
                known(params, &["brightness", "contrast"])?;
                num(params, "brightness", -100.0, 100.0, Some(0.0))?;
                num(params, "contrast", -100.0, 100.0, Some(0.0))?;
            }
            "levels" => {
                known(params, &["black", "white", "gamma"])?;
                let b = num(params, "black", 0.0, 254.0, Some(0.0))?;
                let w = num(params, "white", 1.0, 255.0, Some(255.0))?;
                num(params, "gamma", 0.1, 10.0, Some(1.0))?;
                if b >= w {
                    bail!("[invalid-operation] black must be below white")
                }
            }
            "curves" => {
                known(params, &["points"])?;
                curve_points(params)?;
            }
            "hue-saturation" => {
                known(params, &["hue", "saturation"])?;
                num(params, "hue", -180.0, 180.0, Some(0.0))?;
                num(params, "saturation", -100.0, 100.0, Some(0.0))?;
            }
            "blur" => {
                known(params, &["radius"])?;
                num(params, "radius", 0.0, 256.0, None)?;
            }
            "sharpen" => {
                known(params, &["radius", "amount"])?;
                num(params, "radius", 0.0, 256.0, None)?;
                num(params, "amount", 0.0, 500.0, Some(100.0))?;
            }
            _ => known(params, &[])?,
        }
        Ok(())
    })()
    .with_context(|| format!("operation {id} ({kind})"))
}

fn curve_points(params: &Map<String, Value>) -> Result<Vec<(f64, f64)>> {
    let points = params
        .get("points")
        .and_then(Value::as_array)
        .context("[invalid-operation] points must be an array")?;
    if !(2..=16).contains(&points.len()) {
        bail!("[invalid-operation] curves need 2–16 points")
    }
    let mut parsed = Vec::with_capacity(points.len());
    for point in points {
        let pair = point
            .as_array()
            .filter(|p| p.len() == 2)
            .context("[invalid-operation] each curve point must be [input, output]")?;
        let x = pair[0].as_f64().filter(|n| (0.0..=255.0).contains(n));
        let y = pair[1].as_f64().filter(|n| (0.0..=255.0).contains(n));
        parsed.push((
            x.context("[invalid-operation] curve input must be in 0–255")?,
            y.context("[invalid-operation] curve output must be in 0–255")?,
        ));
    }
    if parsed.windows(2).any(|w| w[0].0 >= w[1].0) {
        bail!("[invalid-operation] curve inputs must strictly increase")
    }
    Ok(parsed)
}

/// Validate a node's whole operation stack: bounds, unique ids, each operation,
/// and the cumulative dimension chain against the surface limits.
pub fn validate_stack(operations: &[Value], width: u32, height: u32) -> Result<()> {
    if operations.len() > MAX_OPERATIONS {
        bail!("[limit-exceeded] an image may have at most {MAX_OPERATIONS} operations")
    }
    let mut ids = HashSet::new();
    for (index, operation) in operations.iter().enumerate() {
        validate_operation(operation).with_context(|| format!("operation index {index}"))?;
        let id = operation["id"].as_str().unwrap();
        if !ids.insert(id) {
            bail!("[invalid-operation] duplicate operation id {id} at index {index}")
        }
    }
    let (mut w, mut h) = (u64::from(width), u64::from(height));
    for (index, operation) in operations.iter().enumerate() {
        if !operation["enabled"].as_bool().unwrap_or(false) {
            continue;
        }
        (w, h) = output_size(operation, w, h);
        crate::image::validate_surface(w, h)
            .with_context(|| format!("operation index {index} expands the image beyond limits"))?;
    }
    Ok(())
}

fn output_size(operation: &Value, w: u64, h: u64) -> (u64, u64) {
    let p = &operation["params"];
    match operation["kind"].as_str().unwrap_or_default() {
        "crop" => (
            ((w as f64 * p["width"].as_f64().unwrap_or(1.0)).round() as u64).max(1),
            ((h as f64 * p["height"].as_f64().unwrap_or(1.0)).round() as u64).max(1),
        ),
        "resize" => (
            p["width"].as_f64().map_or(w, |v| v.round() as u64).max(1),
            p["height"].as_f64().map_or(h, |v| v.round() as u64).max(1),
        ),
        "rotate"
            if matches!(
                p["degrees"].as_f64().map(|v| v.round() as u64),
                Some(90 | 270)
            ) =>
        {
            (h, w)
        }
        _ => (w, h),
    }
}

pub fn new_operation(
    id: &str,
    kind: &str,
    params: Map<String, Value>,
    enabled: bool,
) -> Result<Value> {
    let operation = json!({
        "id": id, "kind": kind, "version": ENGINE_VERSION, "enabled": enabled,
        "params": Value::Object(params)
    });
    validate_operation(&operation)?;
    Ok(operation)
}

/// Apply every enabled operation, in array order, to oriented source pixels.
pub fn apply_stack(mut image: RgbaImage, operations: &[Value]) -> Result<RgbaImage> {
    for operation in operations {
        if operation["enabled"].as_bool() != Some(true) {
            continue;
        }
        image = apply(image, operation)?;
    }
    Ok(image)
}

fn p(operation: &Value, key: &str, default: f64) -> f64 {
    operation["params"][key].as_f64().unwrap_or(default)
}

fn apply(image: RgbaImage, operation: &Value) -> Result<RgbaImage> {
    let kind = operation["kind"].as_str().unwrap_or_default();
    Ok(match kind {
        "crop" => {
            let (w, h) = (f64::from(image.width()), f64::from(image.height()));
            let x = (w * p(operation, "x", 0.0)).round() as u32;
            let y = (h * p(operation, "y", 0.0)).round() as u32;
            let cw = ((w * p(operation, "width", 1.0)).round() as u32).max(1);
            let ch = ((h * p(operation, "height", 1.0)).round() as u32).max(1);
            let cw = cw.min(image.width() - x.min(image.width() - 1));
            let ch = ch.min(image.height() - y.min(image.height() - 1));
            image::imageops::crop_imm(
                &image,
                x.min(image.width() - 1),
                y.min(image.height() - 1),
                cw,
                ch,
            )
            .to_image()
        }
        "resize" => resize(
            &image,
            p(operation, "width", 1.0) as u32,
            p(operation, "height", 1.0) as u32,
        ),
        "rotate" => match p(operation, "degrees", 0.0) as u32 {
            90 => image::imageops::rotate90(&image),
            180 => image::imageops::rotate180(&image),
            270 => image::imageops::rotate270(&image),
            _ => image,
        },
        "brightness-contrast" => {
            let brightness = p(operation, "brightness", 0.0) * 255.0 / 100.0;
            let c = p(operation, "contrast", 0.0) * 255.0 / 100.0;
            let factor = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
            let lut = lut(|v| (factor * (v - 128.0) + 128.0 + brightness).max(-1.0e6));
            map_rgb(image, &lut)
        }
        "levels" => {
            let (b, w) = (p(operation, "black", 0.0), p(operation, "white", 255.0));
            let gamma = p(operation, "gamma", 1.0);
            let lut = lut(|v| {
                let t = ((v - b) / (w - b)).clamp(0.0, 1.0);
                255.0 * det_pow(t, 1.0 / gamma)
            });
            map_rgb(image, &lut)
        }
        "curves" => {
            let points = curve_points(operation["params"].as_object().unwrap())?;
            let lut = lut(|v| {
                if v <= points[0].0 {
                    return points[0].1;
                }
                for pair in points.windows(2) {
                    if v <= pair[1].0 {
                        let t = (v - pair[0].0) / (pair[1].0 - pair[0].0);
                        return pair[0].1 + (pair[1].1 - pair[0].1) * t;
                    }
                }
                points[points.len() - 1].1
            });
            map_rgb(image, &lut)
        }
        "hue-saturation" => hue_saturation(
            image,
            p(operation, "hue", 0.0),
            p(operation, "saturation", 0.0),
        ),
        "blur" => blur(&image, p(operation, "radius", 0.0)),
        "sharpen" => sharpen(
            &image,
            p(operation, "radius", 0.0),
            p(operation, "amount", 100.0),
        ),
        "grayscale" => {
            let mut image = image;
            for px in image.pixels_mut() {
                let y = (u32::from(px[0]) * 2126
                    + u32::from(px[1]) * 7152
                    + u32::from(px[2]) * 722
                    + 5000)
                    / 10_000;
                px[0] = y as u8;
                px[1] = y as u8;
                px[2] = y as u8;
            }
            image
        }
        other => bail!("[unsupported-capability] unknown operation kind {other}"),
    })
}

fn round_u8(value: f64) -> u8 {
    (value + 0.5).floor().clamp(0.0, 255.0) as u8
}

fn lut(f: impl Fn(f64) -> f64) -> [u8; 256] {
    let mut table = [0u8; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        *slot = round_u8(f(i as f64));
    }
    table
}

fn map_rgb(mut image: RgbaImage, table: &[u8; 256]) -> RgbaImage {
    for px in image.pixels_mut() {
        for c in 0..3 {
            px[c] = table[px[c] as usize];
        }
    }
    image
}

/// `exp(y * ln(x))` from basic arithmetic so every platform agrees bit for bit.
pub(crate) fn det_pow(x: f64, y: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    // ln(x) = 2 atanh((x-1)/(x+1)); reduce by halving to keep the series short.
    let mut m = x;
    let mut k = 0i32;
    while m < 0.5 {
        m *= 2.0;
        k -= 1;
    }
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let mut term = z;
    let mut sum = 0.0;
    let mut n = 1.0;
    for _ in 0..40 {
        sum += term / n;
        term *= z2;
        n += 2.0;
    }
    const LN2: f64 = std::f64::consts::LN_2;
    let t = y * (2.0 * sum + f64::from(k) * LN2);
    // exp(t), t <= 0: exp(t) = exp(t/2^s)^(2^s)
    let mut r = t / 1024.0;
    let mut e = 1.0;
    let mut term = 1.0;
    for i in 1..20 {
        term *= r / f64::from(i);
        e += term;
    }
    r = e;
    for _ in 0..10 {
        r *= r;
    }
    r
}

fn hue_saturation(mut image: RgbaImage, hue: f64, saturation: f64) -> RgbaImage {
    for px in image.pixels_mut() {
        let (r, g, b) = (
            f64::from(px[0]) / 255.0,
            f64::from(px[1]) / 255.0,
            f64::from(px[2]) / 255.0,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let mut h = if d == 0.0 {
            0.0
        } else if max == r {
            (g - b) / d
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        if h < 0.0 {
            h += 6.0;
        }
        h += hue / 60.0;
        while h < 0.0 {
            h += 6.0;
        }
        while h >= 6.0 {
            h -= 6.0;
        }
        let s = if max == 0.0 { 0.0 } else { d / max };
        let s = if saturation >= 0.0 {
            s + (1.0 - s) * (saturation / 100.0)
        } else {
            s * (1.0 + saturation / 100.0)
        };
        let v = max;
        let i = h.floor();
        let f = h - i;
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
        let (r, g, b) = match i as u32 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        px[0] = round_u8(r * 255.0);
        px[1] = round_u8(g * 255.0);
        px[2] = round_u8(b * 255.0);
    }
    image
}

/// Premultiplied RGBA as f64 planes.
fn premultiply(image: &RgbaImage) -> Vec<[f64; 4]> {
    image
        .pixels()
        .map(|px| {
            let a = f64::from(px[3]);
            [
                f64::from(px[0]) * a / 255.0,
                f64::from(px[1]) * a / 255.0,
                f64::from(px[2]) * a / 255.0,
                a,
            ]
        })
        .collect()
}

fn unpremultiply(width: u32, height: u32, pixels: &[[f64; 4]]) -> RgbaImage {
    let mut out = RgbaImage::new(width, height);
    for (px, v) in out.pixels_mut().zip(pixels) {
        let a = v[3].clamp(0.0, 255.0);
        if a <= 0.0 {
            *px = image::Rgba([0, 0, 0, 0]);
        } else {
            let scale = 255.0 / a;
            *px = image::Rgba([
                round_u8(v[0] * scale),
                round_u8(v[1] * scale),
                round_u8(v[2] * scale),
                round_u8(a),
            ]);
        }
    }
    out
}

/// Separable bilinear resample with pixel-centre mapping and edge clamping.
fn resize(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let src = premultiply(image);
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let (dw, dh) = (width as usize, height as usize);
    let mut out = vec![[0.0; 4]; dw * dh];
    let sx = sw as f64 / dw as f64;
    let sy = sh as f64 / dh as f64;
    for y in 0..dh {
        let fy = ((y as f64 + 0.5) * sy - 0.5).clamp(0.0, (sh - 1) as f64);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f64;
        for x in 0..dw {
            let fx = ((x as f64 + 0.5) * sx - 0.5).clamp(0.0, (sw - 1) as f64);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f64;
            for c in 0..4 {
                let top = src[y0 * sw + x0][c] * (1.0 - tx) + src[y0 * sw + x1][c] * tx;
                let bottom = src[y1 * sw + x0][c] * (1.0 - tx) + src[y1 * sw + x1][c] * tx;
                out[y * dw + x][c] = top * (1.0 - ty) + bottom * ty;
            }
        }
    }
    unpremultiply(width, height, &out)
}

/// Three successive box blurs approximate a Gaussian (standard `sigma = radius`
/// box-size derivation) with only integer-valued window sums.
fn box_sizes(sigma: f64) -> [usize; 3] {
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut lower = ideal.floor() as usize;
    if lower & 1 == 0 {
        lower -= 1;
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let l = lower as f64;
    let m =
        ((12.0 * sigma * sigma - 3.0 * l * l - 12.0 * l - 9.0) / (-4.0 * l - 4.0)).round() as usize;
    let mut sizes = [lower; 3];
    for (i, size) in sizes.iter_mut().enumerate() {
        if i >= m {
            *size = upper;
        }
    }
    sizes
}

fn box_pass(
    data: &[f64],
    width: usize,
    height: usize,
    radius: usize,
    horizontal: bool,
    channel: usize,
) -> Vec<f64> {
    let mut out = vec![0.0; data.len()];
    let (lines, length) = if horizontal {
        (height, width)
    } else {
        (width, height)
    };
    let window = (2 * radius + 1) as f64;
    for line in 0..lines {
        let at = |i: usize| -> usize {
            if horizontal {
                line * width + i
            } else {
                i * width + line
            }
        };
        let get = |i: isize| data[at(i.clamp(0, length as isize - 1) as usize) * 4 + channel];
        let mut sum = 0.0;
        for k in -(radius as isize)..=(radius as isize) {
            sum += get(k);
        }
        for i in 0..length {
            out[at(i) * 4 + channel] = sum / window;
            sum += get(i as isize + radius as isize + 1) - get(i as isize - radius as isize);
        }
    }
    out
}

fn blur_planes(pixels: &[[f64; 4]], width: u32, height: u32, sigma: f64) -> Vec<[f64; 4]> {
    let (w, h) = (width as usize, height as usize);
    let mut data: Vec<f64> = pixels.iter().flatten().copied().collect();
    if sigma > 0.0 {
        for size in box_sizes(sigma) {
            let radius = (size - 1) / 2;
            for horizontal in [true, false] {
                let mut next = data.clone();
                for channel in 0..4 {
                    let pass = box_pass(&data, w, h, radius, horizontal, channel);
                    for i in 0..w * h {
                        next[i * 4 + channel] = pass[i * 4 + channel];
                    }
                }
                data = next;
            }
        }
    }
    (0..data.len() / 4)
        .map(|i| {
            [
                data[4 * i],
                data[4 * i + 1],
                data[4 * i + 2],
                data[4 * i + 3],
            ]
        })
        .collect()
}

fn blur(image: &RgbaImage, radius: f64) -> RgbaImage {
    let blurred = blur_planes(&premultiply(image), image.width(), image.height(), radius);
    unpremultiply(image.width(), image.height(), &blurred)
}

/// Unsharp mask on premultiplied pixels: `v + amount/100 * (v - blur(v))`.
fn sharpen(image: &RgbaImage, radius: f64, amount: f64) -> RgbaImage {
    let source = premultiply(image);
    let blurred = blur_planes(&source, image.width(), image.height(), radius);
    let gain = amount / 100.0;
    let mixed: Vec<[f64; 4]> = source
        .iter()
        .zip(&blurred)
        .map(|(s, b)| {
            let alpha = s[3];
            let mut v = [0.0; 4];
            for c in 0..3 {
                v[c] = (s[c] + gain * (s[c] - b[c])).clamp(0.0, alpha);
            }
            v[3] = alpha;
            v
        })
        .collect();
    unpremultiply(image.width(), image.height(), &mixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(rgba))
    }

    fn op(kind: &str, params: Value) -> Value {
        new_operation(
            kind,
            kind,
            params.as_object().cloned().unwrap_or_default(),
            true,
        )
        .unwrap()
    }

    #[test]
    fn identity_parameters_do_not_change_pixels() {
        let image = solid(3, 3, [10, 200, 30, 255]);
        for operation in [
            op("brightness-contrast", json!({})),
            op("levels", json!({})),
            op("hue-saturation", json!({})),
            op("blur", json!({"radius": 0})),
            op("resize", json!({"width": 3, "height": 3})),
            op("rotate", json!({"degrees": 0})),
        ] {
            assert_eq!(
                apply_stack(image.clone(), std::slice::from_ref(&operation)).unwrap(),
                image,
                "{operation}"
            );
        }
    }

    #[test]
    fn grayscale_rotate_and_crop_are_exact() {
        let gray =
            apply_stack(solid(1, 1, [255, 0, 0, 77]), &[op("grayscale", json!({}))]).unwrap();
        assert_eq!(gray.get_pixel(0, 0).0, [54, 54, 54, 77]);
        let mut image = solid(2, 1, [0, 0, 0, 255]);
        image.put_pixel(1, 0, image::Rgba([9, 9, 9, 255]));
        let rotated = apply_stack(image.clone(), &[op("rotate", json!({"degrees": 90}))]).unwrap();
        assert_eq!((rotated.width(), rotated.height()), (1, 2));
        assert_eq!(rotated.get_pixel(0, 1).0, [9, 9, 9, 255]);
        assert_eq!(rotated.get_pixel(0, 0).0, [0, 0, 0, 255]);
        let cropped = apply_stack(
            image,
            &[op(
                "crop",
                json!({"x": 0.5, "y": 0, "width": 0.5, "height": 1}),
            )],
        )
        .unwrap();
        assert_eq!(cropped.get_pixel(0, 0).0, [9, 9, 9, 255]);
    }

    #[test]
    fn blur_never_leaks_transparent_rgb() {
        let mut image = solid(5, 1, [255, 0, 0, 0]);
        image.put_pixel(2, 0, image::Rgba([0, 0, 255, 255]));
        let out = apply_stack(image, &[op("blur", json!({"radius": 1}))]).unwrap();
        for px in out.pixels().filter(|px| px[3] > 0) {
            assert_eq!(px[0], 0, "red must not leak from fully transparent pixels");
        }
    }

    #[test]
    fn levels_gamma_is_deterministic() {
        assert!((det_pow(0.5, 0.5) - 0.707_106_781_186_547_5).abs() < 1e-12);
        assert!((det_pow(0.25, 2.0) - 0.0625).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_parameters_and_expansion() {
        for (kind, params) in [
            ("blur", json!({"radius": -1})),
            ("rotate", json!({"degrees": 45})),
            ("crop", json!({"x": 0.8, "y": 0, "width": 0.5, "height": 1})),
            ("levels", json!({"black": 200, "white": 100})),
            ("curves", json!({"points": [[0, 0]]})),
            ("blur", json!({"radius": 1, "bogus": 2})),
        ] {
            assert!(
                new_operation("x", kind, params.as_object().cloned().unwrap(), true).is_err(),
                "{kind}"
            );
        }
        let huge = op("resize", json!({"width": 32768, "height": 32768}));
        assert!(validate_stack(&[huge], 10, 10).is_err());
    }
}

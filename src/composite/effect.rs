//! Appearance effects retain the original masked silhouette and commit a whole
//! ordered stack atomically. Spatial kernels mirror the released byte engine in
//! `effects.rs` (box-approximated blur of encoded sRGB, shifted and blurred
//! coverage, octagonal stroke distance) without quantizing color or coverage.
use super::check_cancelled;
use crate::blend::{LegacyBlendContext, LinearSurface};
use anyhow::{bail, Context, Result};
use serde_json::Value;

/// The prepared paint an effect needs: a positioned surface for overlays, or one
/// working RGB color with separate alpha for shadows, glows and strokes.
pub(super) enum Paint {
    Surface(LinearSurface),
    Color([f64; 4]),
}

pub(super) fn validate_supported(stack: &[Value]) -> Result<()> {
    for op in stack.iter().filter(|op| op["enabled"] == true) {
        if !op["kind"]
            .as_str()
            .is_some_and(|kind| crate::effects::KINDS.contains(&kind))
        {
            bail!(
                "[unsupported-capability] float scene effects do not implement {} (effect {})",
                op["kind"],
                op["id"]
            )
        }
    }
    Ok(())
}

/// Whether an effect samples a positioned paint surface rather than one color.
pub(super) fn needs_surface(op: &Value) -> bool {
    matches!(
        op["kind"].as_str(),
        Some("color-overlay" | "gradient-overlay")
    )
}

fn param(op: &Value, key: &str, default: f64) -> f64 {
    op["params"]
        .get(key)
        .and_then(Value::as_f64)
        .unwrap_or(default)
}

fn scaled_blur(radius: f64) -> Result<f64> {
    if !radius.is_finite() || radius > 256.0 {
        bail!("[limit-exceeded] scaled effect blur exceeds 256 pixels")
    }
    Ok(radius.max(0.0))
}

pub(super) fn apply(
    source: &LinearSurface,
    node: &Value,
    stack: &[Value],
    context: &LegacyBlendContext,
    scale: f32,
    mut paint: impl FnMut(&Value) -> Result<Paint>,
) -> Result<LinearSurface> {
    validate_supported(stack)?;
    if source.space() != context.working_space() {
        bail!("[invalid-effect] working primaries differ from the blend context")
    }
    let content = node
        .get("content_opacity")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    if !content.is_finite() || !(0.0..=1.0).contains(&content) {
        bail!("[invalid-effect] content opacity must be in [0,1]")
    }
    let scale = f64::from(scale);
    check_cancelled()?;
    let width = source.dimensions().0 as usize;
    let mut result = source.clone();
    for (index, pixel) in source.pixels().iter().enumerate() {
        if index % width == 0 {
            check_cancelled()?;
        }
        result.write_pixel(index, [pixel[0], pixel[1], pixel[2], pixel[3] * content])?;
    }
    for op in stack {
        check_cancelled()?;
        if op["enabled"] != true {
            continue;
        }
        let opacity = op.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            bail!("[invalid-effect] opacity must be in [0,1]")
        }
        let mode = op["blend_mode"]
            .as_str()
            .context("effect blend mode missing")?;
        let space = op["blend_space"].as_str().unwrap_or("srgb");
        crate::blend::validate(mode, space)?;
        let kind = op["kind"].as_str().unwrap_or_default();
        if kind == "blur" {
            let radius = scaled_blur(param(op, "radius", 0.0) * scale)?;
            blur(&mut result, radius, opacity, mode, space, context)?;
            continue;
        }
        let prepared = paint(op)?;
        if needs_surface(op) {
            let Paint::Surface(overlay) = prepared else {
                bail!("[invalid-effect] overlay paint must be a surface")
            };
            if overlay.dimensions() != source.dimensions() || overlay.space() != source.space() {
                bail!("[invalid-effect] overlay paint dimensions or working primaries differ")
            }
            let pixels = overlay.pixels();
            mix_inside(&mut result, source, mode, space, context, |index| {
                let p = pixels[index];
                ([p[0], p[1], p[2]], p[3] * opacity)
            })?;
            continue;
        }
        let Paint::Color(color) = prepared else {
            bail!("[invalid-effect] {kind} paint must be one color")
        };
        let rgb = [color[0], color[1], color[2]];
        let strength = if kind == "stroke" {
            stroke(source, op, scale)?
        } else {
            shadow(source, kind, op, scale)?
        };
        if matches!(kind, "inner-shadow" | "inner-glow") {
            mix_inside(&mut result, source, mode, space, context, |index| {
                (rgb, strength[index] * color[3] * opacity)
            })?;
            continue;
        }
        let (w, h) = source.dimensions();
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(strength.len())?;
        for (index, (amount, base)) in strength.iter().zip(source.pixels()).enumerate() {
            if index % width == 0 {
                check_cancelled()?;
            }
            let silhouette = if kind == "outer-glow" {
                1.0 - base[3]
            } else {
                1.0
            };
            pixels.push([
                rgb[0],
                rgb[1],
                rgb[2],
                amount * silhouette * color[3] * opacity,
            ]);
        }
        let effect = LinearSurface::from_pixels(w, h, source.space(), pixels)?;
        if kind == "stroke" {
            result.over_declared(&effect, mode, space, context)?;
        } else {
            // Shadows and outer glows sit below the content, which blends onto them.
            let mut below = effect;
            below.over_declared(&result, mode, space, context)?;
            result = below;
        }
    }
    check_cancelled()?;
    Ok(result)
}

/// Overlay-style composition inside the immutable source silhouette. Strength
/// interpolates encoded sRGB as in the released engine; coverage becomes the
/// larger of the current coverage and the effect's source-weighted strength.
fn mix_inside(
    result: &mut LinearSurface,
    source: &LinearSurface,
    mode: &str,
    space: &str,
    context: &LegacyBlendContext,
    sample: impl Fn(usize) -> ([f64; 3], f64),
) -> Result<()> {
    let width = source.dimensions().0 as usize;
    for (index, base) in source.pixels().iter().enumerate() {
        if index % width == 0 {
            check_cancelled()?;
        }
        let (paint, amount) = sample(index);
        if base[3] == 0.0 || amount == 0.0 {
            continue;
        }
        let original = result.pixels()[index];
        let backdrop = if original[3] == 0.0 { *base } else { original };
        let mixed = context.compose(
            mode,
            space,
            [backdrop[0], backdrop[1], backdrop[2], 1.0],
            [paint[0], paint[1], paint[2], 1.0],
        )?;
        let mixed = context.coordinates(mixed, "srgb");
        let encoded = context.coordinates(original, "srgb");
        let rgb = std::array::from_fn::<_, 3, _>(|i| {
            if original[3] == 0.0 {
                mixed[i]
            } else {
                encoded[i] * (1.0 - amount) + mixed[i] * amount
            }
        });
        let rgba = context.working_coordinates(
            [rgb[0], rgb[1], rgb[2], original[3].max(base[3] * amount)],
            "srgb",
        );
        result.write_pixel(index, rgba)?;
    }
    Ok(())
}

/// Blur the current result as premultiplied encoded sRGB, optionally blend the
/// blurred color onto the unblurred content, then mix the two by opacity.
fn blur(
    result: &mut LinearSurface,
    radius: f64,
    opacity: f64,
    mode: &str,
    space: &str,
    context: &LegacyBlendContext,
) -> Result<()> {
    let (w, h) = result.dimensions();
    let width = w as usize;
    let source = result.pixels();
    let mut planes = Vec::new();
    planes.try_reserve_exact(source.len())?;
    planes.resize(source.len(), [0.0; 4]);
    crate::threads::rows(&mut planes, width, |first, band| {
        for (plane, pixel) in band.iter_mut().zip(&source[first * width..]) {
            let e = context.coordinates(*pixel, "srgb");
            *plane = [e[0] * pixel[3], e[1] * pixel[3], e[2] * pixel[3], pixel[3]];
        }
    });
    check_cancelled()?;
    let blurred = crate::imageops::blur_planes(&planes, w, h, radius);
    drop(planes);
    check_cancelled()?;
    let blend = mode != "normal" || space != "srgb";
    // Rows are mixed in parallel; the per-row checkpoints run here first.
    for _ in 0..source.len().checked_div(width).unwrap_or(0) {
        check_cancelled()?;
    }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(blurred.len())?;
    pixels.resize(blurred.len(), [0.0; 4]);
    let failure = std::sync::Mutex::new(None);
    crate::threads::rows(&mut pixels, width, |first, band| {
        let start = first * width;
        for ((slot, original), b) in band.iter_mut().zip(&source[start..]).zip(&blurred[start..]) {
            match mixed(original, b, opacity, blend, mode, space, context) {
                Ok(pixel) => *slot = pixel,
                Err(error) => {
                    failure.lock().unwrap().get_or_insert(error);
                    return;
                }
            }
        }
    });
    if let Some(error) = failure.into_inner().unwrap() {
        return Err(error);
    }
    *result = LinearSurface::from_pixels(w, h, result.space(), pixels)?;
    Ok(())
}

/// One blurred pixel mixed onto its original by `opacity`.
fn mixed(
    original: &[f64; 4],
    b: &[f64; 4],
    opacity: f64,
    blend: bool,
    mode: &str,
    space: &str,
    context: &LegacyBlendContext,
) -> Result<[f64; 4]> {
    let ba = b[3].clamp(0.0, 1.0);
    let mut be = if ba <= 0.0 {
        [0.0; 3]
    } else {
        [b[0] / ba, b[1] / ba, b[2] / ba]
    };
    if blend && original[3] > 0.0 && ba > 0.0 {
        let blurred = context.working_coordinates([be[0], be[1], be[2], 1.0], "srgb");
        let mixed = context.compose(
            mode,
            space,
            [original[0], original[1], original[2], 1.0],
            blurred,
        )?;
        let mixed = context.coordinates(mixed, "srgb");
        be = [mixed[0], mixed[1], mixed[2]];
    }
    let oa = original[3];
    let alpha = oa * (1.0 - opacity) + ba * opacity;
    let encoded = context.coordinates(*original, "srgb");
    let rgb = std::array::from_fn::<_, 3, _>(|i| {
        if alpha == 0.0 {
            0.0
        } else {
            (encoded[i] * oa * (1.0 - opacity) + be[i] * ba * opacity) / alpha
        }
    });
    Ok(context.working_coordinates([rgb[0], rgb[1], rgb[2], alpha.clamp(0.0, 1.0)], "srgb"))
}

/// Blurred, offset coverage of the source (or its complement for inner kinds).
fn shadow(source: &LinearSurface, kind: &str, op: &Value, scale: f64) -> Result<Vec<f64>> {
    let inner = matches!(kind, "inner-shadow" | "inner-glow");
    let (w, h) = source.dimensions();
    let (wi, hi) = (i64::from(w), i64::from(h));
    let (dx, dy) = (
        (param(op, "x", 0.0) * scale).round() as i64,
        (param(op, "y", 0.0) * scale).round() as i64,
    );
    let radius = scaled_blur(param(op, "blur", 0.0) * scale)?;
    let alpha = source.pixels();
    let mut planes = Vec::new();
    planes.try_reserve_exact(alpha.len())?;
    for y in 0..hi {
        check_cancelled()?;
        for x in 0..wi {
            let (sx, sy) = (x - dx, y - dy);
            let a = if (0..wi).contains(&sx) && (0..hi).contains(&sy) {
                alpha[(sy * wi + sx) as usize][3]
            } else {
                0.0
            };
            planes.push([0.0, 0.0, 0.0, if inner { 1.0 - a } else { a }]);
        }
    }
    let blurred = crate::imageops::blur_planes(&planes, w, h, radius);
    check_cancelled()?;
    // Inner kinds gain source coverage in `mix_inside`; outer glow is cut by
    // the caller, matching the byte engine's `effect / base` strength.
    Ok(blurred.iter().map(|b| b[3].clamp(0.0, 1.0)).collect())
}

/// Stroke coverage from the pinned octagonal distance field. Any nonzero float
/// coverage counts as inside, the analogue of the byte engine's alpha above zero.
fn stroke(source: &LinearSurface, op: &Value, scale: f64) -> Result<Vec<f64>> {
    let (w, h) = source.dimensions();
    let filled: Vec<bool> = source.pixels().iter().map(|p| p[3] > 0.0).collect();
    check_cancelled()?;
    let outside = crate::effects::distance_field(&filled, w as usize, h as usize, true);
    check_cancelled()?;
    let inside = crate::effects::distance_field(&filled, w as usize, h as usize, false);
    check_cancelled()?;
    let size = param(op, "size", 1.0) * scale;
    let position = op["params"]["position"].as_str().unwrap_or("outer");
    Ok(filled
        .iter()
        .enumerate()
        .map(|(index, &filled)| {
            let visible = match position {
                "inner" => filled && inside[index] <= size,
                "center" => {
                    if filled {
                        inside[index] <= size / 2.0
                    } else {
                        outside[index] <= size / 2.0
                    }
                }
                _ => !filled && outside[index] <= size,
            };
            if visible {
                1.0
            } else {
                0.0
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn late_overlay_failure_and_cancellation_preserve_the_source() {
        let source =
            LinearSurface::from_pixels(1, 2, "srgb:linear", vec![[0.2, 0.3, 0.4, 0.7]; 2]).unwrap();
        let before = source.clone();
        let node = json!({});
        let stack = vec![
            json!({"kind":"color-overlay","id":"overlay","enabled":true,"blend_mode":"color-dodge"}),
        ];
        let context = LegacyBlendContext::new("srgb:linear").unwrap();
        let paint = || {
            LinearSurface::from_pixels(
                1,
                2,
                "srgb:linear",
                vec![[0.3, 0.4, 0.5, 1.0], [1.5, 0.4, 0.5, 1.0]],
            )
            .map(Paint::Surface)
        };
        assert!(apply(&source, &node, &stack, &context, 1.0, |_| paint()).is_err());
        assert_eq!(source, before);
        let (result, reached) = super::super::cancel_at_check(6, || {
            apply(&source, &node, &stack, &context, 1.0, |_| paint())
        });
        assert!(reached);
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert_eq!(source, before);
    }
    #[test]
    fn spatial_cancellation_at_every_checkpoint_preserves_the_source() {
        let mut pixels = vec![[0.0; 4]; 25];
        pixels[12] = [0.2, 0.3, 0.4, 0.7];
        let source = LinearSurface::from_pixels(5, 5, "srgb:linear", pixels).unwrap();
        let before = source.clone();
        let node = json!({});
        let op = |kind: &str, params: Value| json!({"kind":kind,"id":kind,"enabled":true,"opacity":0.8,"blend_mode":"normal","params":params});
        let stack = vec![
            op("blur", json!({"radius":1})),
            op("drop-shadow", json!({"x":1,"y":1,"blur":1})),
            op("inner-glow", json!({"blur":1})),
            op("stroke", json!({"size":1})),
        ];
        let context = LegacyBlendContext::new("srgb:linear").unwrap();
        let paint = |_: &Value| Ok(Paint::Color([0.1, 0.2, 0.3, 1.0]));
        let complete = apply(&source, &node, &stack, &context, 1.0, paint).unwrap();
        assert!(complete.pixels().iter().filter(|p| p[3] > 0.0).count() > 1);
        let mut cancelled = 0;
        for at in 1.. {
            let (result, reached) = super::super::cancel_at_check(at, || {
                apply(&source, &node, &stack, &context, 1.0, paint)
            });
            assert_eq!(source, before);
            if !reached {
                assert_eq!(result.unwrap(), complete);
                break;
            }
            assert!(result.unwrap_err().to_string().contains("cancelled"));
            cancelled += 1;
        }
        assert!(cancelled > 20, "only {cancelled} checkpoints");
    }
}

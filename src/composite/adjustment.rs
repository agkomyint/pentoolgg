//! Unquantized counterpart of adjustment engine v1. Parameter channels remain
//! encoded sRGB, independent of storage primaries. Affine math retains signed
//! extended channels; bounded-domain operations have explicit policies below.
use super::{check_cancelled, gradient, luminance, mixer, validate_settings};
use crate::blend::{LegacyBlendContext, LinearSurface};
use anyhow::{bail, Context, Result};
use serde_json::Value;

struct Prepared<'a> {
    kind: &'a str,
    params: &'a Value,
    matrix: Option<[[f64; 4]; 3]>,
    stops: Vec<(f64, [f64; 3])>,
    points: Vec<(f64, f64)>,
    exposure: f64,
}
impl<'a> Prepared<'a> {
    fn new(kind: &'a str, params: &'a Value) -> Result<Self> {
        validate_settings(kind, params)?;
        let stops = if kind == "gradient-map" {
            gradient(params)?
        } else {
            Vec::new()
        };
        let matrix = if kind == "channel-mixer" {
            Some(mixer(params)?)
        } else {
            None
        };
        let points = if kind == "curves" {
            params["points"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    (
                        p[0].as_f64().unwrap() / 255.0,
                        p[1].as_f64().unwrap() / 255.0,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let stops_value = params.get("stops").and_then(Value::as_f64).unwrap_or(0.0);
        let exposure = crate::imageops::det_pow(0.5, stops_value.abs());
        Ok(Self {
            kind,
            params,
            matrix,
            stops,
            points,
            exposure: if stops_value > 0.0 {
                1.0 / exposure
            } else {
                exposure
            },
        })
    }
    fn get(&self, key: &str, default: f64) -> f64 {
        self.params
            .get(key)
            .and_then(Value::as_f64)
            .unwrap_or(default)
    }
    fn evaluate(&self, rgb: [f64; 3]) -> Result<[f64; 3]> {
        let mut out = rgb;
        match self.kind {
            "brightness-contrast" => {
                let brightness = self.get("brightness", 0.0) / 100.0;
                let c = self.get("contrast", 0.0) * 255.0 / 100.0;
                let factor = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
                for i in 0..3 {
                    out[i] = factor * (rgb[i] - 128.0 / 255.0) + 128.0 / 255.0 + brightness;
                }
            }
            "levels" => {
                let black = self.get("black", 0.0) / 255.0;
                let white = self.get("white", 255.0) / 255.0;
                let gamma = self.get("gamma", 1.0);
                // Levels explicitly maps its input black/white interval to [0,1].
                for i in 0..3 {
                    out[i] = crate::imageops::det_pow(
                        ((rgb[i] - black) / (white - black)).clamp(0.0, 1.0),
                        1.0 / gamma,
                    );
                }
            }
            "curves" => {
                // Curve endpoint values extend constantly beyond the authored domain.
                for i in 0..3 {
                    out[i] = if rgb[i] <= self.points[0].0 {
                        self.points[0].1
                    } else if let Some(pair) = self.points.windows(2).find(|p| rgb[i] <= p[1].0) {
                        pair[0].1
                            + (pair[1].1 - pair[0].1) * (rgb[i] - pair[0].0)
                                / (pair[1].0 - pair[0].0)
                    } else {
                        self.points.last().unwrap().1
                    };
                }
            }
            "hue-saturation" => {
                let hue = self.get("hue", 0.0);
                let saturation = self.get("saturation", 0.0);
                if hue == 0.0 && saturation == 0.0 {
                    return Ok(rgb);
                }
                // HSV has no specified signed/extended domain. Refuse meaningful
                // excursions instead of silently clipping colors; snap roundoff only.
                if rgb.iter().any(|v| *v < -1e-12 || *v > 1.0 + 1e-12) {
                    bail!("[unsupported-capability] float hue-saturation requires encoded sRGB channels in [0,1]; convert gamut explicitly first")
                }
                let [r, g, b] = rgb.map(|v| v.clamp(0.0, 1.0));
                let max = r.max(g).max(b);
                let min = r.min(g).min(b);
                let delta = max - min;
                let mut h = if delta == 0.0 {
                    0.0
                } else if max == r {
                    (g - b) / delta
                } else if max == g {
                    (b - r) / delta + 2.0
                } else {
                    (r - g) / delta + 4.0
                };
                h = (h + hue / 60.0).rem_euclid(6.0);
                let s = if max == 0.0 { 0.0 } else { delta / max };
                let s = if saturation >= 0.0 {
                    s + (1.0 - s) * saturation / 100.0
                } else {
                    s * (1.0 + saturation / 100.0)
                };
                let f = h - h.floor();
                let (p, q, t) = (
                    max * (1.0 - s),
                    max * (1.0 - s * f),
                    max * (1.0 - s * (1.0 - f)),
                );
                out = match h.floor() as u32 {
                    0 => [max, t, p],
                    1 => [q, max, p],
                    2 => [p, max, t],
                    3 => [p, q, max],
                    4 => [t, p, max],
                    _ => [max, p, q],
                };
            }
            "exposure" => {
                for i in 0..3 {
                    out[i] = rgb[i] * self.exposure;
                }
            }
            "invert" => {
                for i in 0..3 {
                    out[i] = 1.0 - rgb[i];
                }
            }
            "posterize" => {
                // Posterization chooses one of the finite normalized output levels.
                let n = self.get("levels", 2.0) - 1.0;
                for i in 0..3 {
                    out[i] = (rgb[i].clamp(0.0, 1.0) * n + 0.5).floor() / n;
                }
            }
            "threshold" => {
                out = [if luminance(rgb) >= self.get("level", 128.0) / 255.0 {
                    1.0
                } else {
                    0.0
                }; 3];
            }
            "color-balance" => {
                for (i, key) in ["red", "green", "blue"].iter().enumerate() {
                    out[i] = rgb[i] + self.get(key, 0.0) / 100.0;
                }
            }
            "black-and-white" => {
                out = [rgb[0] * self.get("red", 0.2126)
                    + rgb[1] * self.get("green", 0.7152)
                    + rgb[2] * self.get("blue", 0.0722); 3];
            }
            "channel-mixer" => {
                for (i, row) in self.matrix.as_ref().unwrap().iter().enumerate() {
                    out[i] = rgb[0] * row[0] + rgb[1] * row[1] + rgb[2] * row[2] + row[3];
                }
            }
            "vibrance" => {
                // Bound only the modulation weight. RGB and luminance stay extended.
                let saturation = (rgb[0].max(rgb[1]).max(rgb[2]) - rgb[0].min(rgb[1]).min(rgb[2]))
                    .clamp(0.0, 1.0);
                let factor = 1.0 + self.get("amount", 0.0) / 100.0 * (1.0 - saturation);
                let y = luminance(rgb);
                for i in 0..3 {
                    out[i] = y + (rgb[i] - y) * factor;
                }
            }
            "gradient-map" => {
                // A normalized ramp extends by its first/last endpoint colors.
                let t = luminance(rgb).clamp(0.0, 1.0);
                let pair = self.stops.windows(2).find(|p| t <= p[1].0).unwrap();
                let fraction = (t - pair[0].0) / (pair[1].0 - pair[0].0);
                for (i, c) in out.iter_mut().enumerate() {
                    *c = (pair[0].1[i] + (pair[1].1[i] - pair[0].1[i]) * fraction) / 255.0;
                }
            }
            _ => unreachable!("validated adjustment"),
        }
        if out.iter().any(|v| !v.is_finite()) {
            bail!("[invalid-adjustment] float channel arithmetic overflow")
        }
        Ok(out)
    }
}

/// All samples are evaluated into bounded scratch before replacing the caller's
/// surface. Cancellation and late unsupported channels leave it unchanged.
pub(super) fn apply(
    surface: &mut LinearSurface,
    node: &Value,
    scopes: &[&[f64]],
    mask: Option<&[u8]>,
    context: &LegacyBlendContext,
) -> Result<()> {
    let prepared = Prepared::new(
        node["adjustment"]
            .as_str()
            .context("adjustment kind missing")?,
        &node["params"],
    )?;
    let opacity = node.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
    let mode = node["blend_mode"].as_str().unwrap_or("normal");
    let space = node["blend_space"].as_str().unwrap_or("srgb");
    crate::blend::validate(mode, space)?;
    if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
        bail!("[invalid-adjustment] opacity must be in 0..=1")
    }
    let count = surface.pixels().len();
    if surface.space() != context.working_space()
        || mask.is_some_and(|m| m.len() != count)
        || scopes.iter().any(|s| s.len() != count)
    {
        bail!("[invalid-adjustment] mismatched working primaries or invalid scope/mask coverage")
    }
    for scope in scopes {
        for row in scope.chunks(surface.dimensions().0 as usize) {
            check_cancelled()?;
            if row
                .iter()
                .any(|a| !a.is_finite() || !(0.0..=1.0).contains(a))
            {
                bail!("[invalid-adjustment] scope coverage must be finite and in [0,1]")
            }
        }
    }
    check_cancelled()?;
    if opacity == 0.0 {
        return Ok(());
    }
    let below = node["scope"]["kind"] == "below";
    let mut output = surface.clone();
    for (index, original) in surface.pixels().iter().enumerate() {
        if index % surface.dimensions().0 as usize == 0 {
            check_cancelled()?;
        }
        let scope = if below {
            1.0
        } else {
            scopes.iter().map(|s| s[index]).sum::<f64>().min(1.0)
        };
        let amount = opacity * scope * mask.map_or(1.0, |m| f64::from(m[index]) / 255.0);
        if amount == 0.0 || original[3] == 0.0 {
            continue;
        }
        let encoded = context.coordinates(*original, "srgb");
        if encoded.iter().any(|v| !v.is_finite()) {
            bail!("[invalid-adjustment] encoded channel conversion overflow")
        }
        let adjusted = prepared.evaluate([encoded[0], encoded[1], encoded[2]])?;
        let graded =
            context.working_coordinates([adjusted[0], adjusted[1], adjusted[2], 1.0], "srgb");
        let mixed = context.compose(
            mode,
            space,
            [original[0], original[1], original[2], 1.0],
            graded,
        )?;
        let mixed = context.coordinates(mixed, "srgb");
        // Released opacity/masks interpolate the post-blend encoded result.
        let rgb =
            std::array::from_fn::<_, 3, _>(|i| encoded[i] * (1.0 - amount) + mixed[i] * amount);
        let rgba = context.working_coordinates([rgb[0], rgb[1], rgb[2], original[3]], "srgb");
        output.write_pixel(index, rgba)?;
    }
    check_cancelled()?;
    *surface = output;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn node() -> Value {
        json!({"adjustment":"hue-saturation","params":{"hue":60},"opacity":1,
            "scope":{"kind":"below"},"blend_mode":"normal","blend_space":"srgb"})
    }
    #[test]
    fn late_extended_hue_failure_and_row_cancellation_are_atomic() {
        let context = LegacyBlendContext::new("srgb:linear").unwrap();
        let mut surface = LinearSurface::from_pixels(
            1,
            2,
            "srgb:linear",
            vec![[0.2, 0.3, 0.4, 0.7], [1.5, 0.3, 0.4, 0.9]],
        )
        .unwrap();
        let before = surface.clone();
        assert!(apply(&mut surface, &node(), &[], None, &context)
            .unwrap_err()
            .to_string()
            .contains("hue-saturation requires"));
        assert_eq!(surface, before);
        let mut n = node();
        n["adjustment"] = json!("invert");
        n["params"] = json!({});
        // Entry check, first row, then second row after output row one was written.
        let (result, reached) =
            super::super::cancel_at_check(3, || apply(&mut surface, &n, &[], None, &context));
        assert!(reached);
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert_eq!(surface, before);
    }
    #[test]
    fn bounded_operations_define_extended_endpoints_and_affine_math_retains_them() {
        let rgb = [-0.2, 0.5, 1.3];
        for (kind, params, expected) in [
            ("invert", json!({}), [1.2, 0.5, -0.3]),
            ("levels", json!({}), [0.0, 0.5, 1.0]),
            (
                "curves",
                json!({"points":[[0,0],[255,255]]}),
                [0.0, 0.5, 1.0],
            ),
            ("posterize", json!({"levels":3}), [0.0, 0.5, 1.0]),
            (
                "gradient-map",
                json!({"stops":[{"position":0,"rgb":[0,0,0]},{"position":1,"rgb":[255,255,255]}]}),
                [luminance(rgb); 3],
            ),
            ("hue-saturation", json!({}), rgb),
        ] {
            let actual = Prepared::new(kind, &params).unwrap().evaluate(rgb).unwrap();
            for (a, e) in actual.iter().zip(expected) {
                assert!((a - e).abs() < 1e-12, "{kind}: {actual:?}");
            }
        }
    }
}

//! Portable W3C Compositing Level 1 blend equations, engine contract 1.
//! https://www.w3.org/TR/compositing-1/#blending
use anyhow::{bail, Result};
use image::RgbaImage;

pub const MODES: [&str; 16] = [
    "normal",
    "multiply",
    "screen",
    "overlay",
    "darken",
    "lighten",
    "color-dodge",
    "color-burn",
    "hard-light",
    "soft-light",
    "difference",
    "exclusion",
    "hue",
    "saturation",
    "color",
    "luminosity",
];
pub fn validate(mode: &str, space: &str) -> Result<()> {
    if !MODES.contains(&mode) {
        bail!("[unsupported-capability] unknown blend mode {mode}")
    }
    if !["srgb", "linear"].contains(&space) {
        bail!("[unsupported-capability] blend space must be srgb or linear")
    }
    Ok(())
}
fn lum(c: [f64; 3]) -> f64 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}
fn sat(c: [f64; 3]) -> f64 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}
fn set_lum(mut c: [f64; 3], l: f64) -> [f64; 3] {
    let d = l - lum(c);
    for channel in &mut c {
        *channel += d;
    }
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    if n < 0.0 {
        for channel in &mut c {
            *channel = l + (*channel - l) * l / (l - n);
        }
    }
    if x > 1.0 {
        for channel in &mut c {
            *channel = l + (*channel - l) * (1.0 - l) / (x - l);
        }
    }
    c
}
fn set_sat(mut c: [f64; 3], s: f64) -> [f64; 3] {
    let mut order = [0, 1, 2];
    order.sort_by(|a, b| c[*a].total_cmp(&c[*b]).then(a.cmp(b)));
    let [min, mid, max] = order;
    if c[max] > c[min] {
        c[mid] = (c[mid] - c[min]) * s / (c[max] - c[min]);
        c[max] = s;
    } else {
        c[mid] = 0.0;
        c[max] = 0.0;
    }
    c[min] = 0.0;
    c
}
pub(crate) fn rgb(mode: &str, b: [f64; 3], s: [f64; 3]) -> [f64; 3] {
    match mode {
        "hue" => return set_lum(set_sat(s, sat(b)), lum(b)),
        "saturation" => return set_lum(set_sat(b, sat(s)), lum(b)),
        "color" => return set_lum(s, lum(b)),
        "luminosity" => return set_lum(b, lum(s)),
        _ => {}
    }
    let mut out = [0.0; 3];
    for i in 0..3 {
        let (b, s) = (b[i], s[i]);
        let hard = |b: f64, s: f64| {
            if s <= 0.5 {
                2.0 * b * s
            } else {
                1.0 - 2.0 * (1.0 - b) * (1.0 - s)
            }
        };
        out[i] = match mode {
            "normal" => s,
            "multiply" => b * s,
            "screen" => b + s - b * s,
            "overlay" => hard(s, b),
            "darken" => b.min(s),
            "lighten" => b.max(s),
            "color-dodge" => {
                if b == 0.0 {
                    0.0
                } else if s == 1.0 {
                    1.0
                } else {
                    (b / (1.0 - s)).min(1.0)
                }
            }
            "color-burn" => {
                if b == 1.0 {
                    1.0
                } else if s == 0.0 {
                    0.0
                } else {
                    1.0 - ((1.0 - b) / s).min(1.0)
                }
            }
            "hard-light" => hard(b, s),
            "soft-light" => {
                if s <= 0.5 {
                    b - (1.0 - 2.0 * s) * b * (1.0 - b)
                } else {
                    let d = if b <= 0.25 {
                        ((16.0 * b - 12.0) * b + 4.0) * b
                    } else {
                        b.sqrt()
                    };
                    b + (2.0 * s - 1.0) * (d - b)
                }
            }
            "difference" => (b - s).abs(),
            "exclusion" => b + s - 2.0 * b * s,
            _ => unreachable!("validated blend mode"),
        };
    }
    out
}
pub fn to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        crate::imageops::det_pow((v + 0.055) / 1.055, 2.4)
    }
}
pub fn to_srgb(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * crate::imageops::det_pow(v, 1.0 / 2.4) - 0.055
    }
}
fn byte(v: f64) -> u8 {
    (v * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
}
/// Premultiplied source-over with blend applied only in source/backdrop overlap.
pub fn over(dst: &mut RgbaImage, src: &RgbaImage, mode: &str, space: &str) -> Result<()> {
    validate(mode, space)?;
    if dst.dimensions() != src.dimensions() {
        bail!("[invalid-composite] surface dimensions differ")
    }
    let plain = mode == "normal" && space == "srgb";
    for (d, s) in dst.pixels_mut().zip(src.pixels()) {
        if plain {
            // Exact shortcuts for the common cases: nothing to add / fully opaque source.
            if s[3] == 0 {
                if d[3] == 0 {
                    *d = image::Rgba([0, 0, 0, 0]);
                }
                continue;
            }
            if s[3] == 255 {
                *d = *s;
                continue;
            }
        }
        let (sa, da) = (f64::from(s[3]) / 255.0, f64::from(d[3]) / 255.0);
        let a = sa + da * (1.0 - sa);
        if a == 0.0 {
            *d = image::Rgba([0, 0, 0, 0]);
            continue;
        }
        let mut cb = [0.0; 3];
        let mut cs = [0.0; 3];
        for i in 0..3 {
            cb[i] = f64::from(d[i]) / 255.0;
            cs[i] = f64::from(s[i]) / 255.0;
            if space == "linear" {
                cb[i] = to_linear(cb[i]);
                cs[i] = to_linear(cs[i]);
            }
        }
        let mixed = rgb(mode, cb, cs);
        for i in 0..3 {
            let c = ((1.0 - sa) * da * cb[i] + (1.0 - da) * sa * cs[i] + sa * da * mixed[i]) / a;
            d[i] = byte(if space == "linear" {
                to_srgb(c.clamp(0.0, 1.0))
            } else {
                c
            });
        }
        d[3] = byte(a);
    }
    Ok(())
}

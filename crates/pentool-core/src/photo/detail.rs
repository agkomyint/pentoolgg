//! Detail processing of process 1: defective pixels (stage 1), early noise
//! reduction, moiré and defringe on linear camera RGB (stage 2), and capture
//! sharpening at the end of stage 9. The kernels are specified in the "Detail"
//! section of `docs/photography-v1.md`.
//!
//! Detail works at the sensor's pixel scale, so its radii are in pixels of the
//! developed frame rather than fractions of the long edge. An absent or
//! zero-strength control skips its kernel and leaves the pixels bit for bit.
use super::adjust::{self, gaussian, smoothstep, Oklab, FLOOR};
use super::color::{self, Matrix};
use super::dng::Cfa;
use super::math;
use super::opcode::Plane;
use super::pixels::Working;
use anyhow::{bail, Result};
use serde_json::Value;

/// The keys of `detail.noise`, each 0–100.
pub const NOISE: [&str; 6] = [
    "luminance",
    "luminance_detail",
    "luminance_contrast",
    "color",
    "color_detail",
    "color_smoothness",
];

fn get(object: &Value, key: &str, default: f64) -> f64 {
    object.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn positive(v: f64) -> bool {
    v > 0.0
}

// ---------------------------------------------------------------------------
// Stage 1: defective pixels.

/// `raw.defective_pixels`: a list of active-area coordinates and optional
/// automatic detection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Defects {
    pub auto: bool,
    /// 1–100; higher finds fainter defects.
    pub threshold: f64,
    pub list: Vec<(usize, usize)>,
}

impl Defects {
    pub fn new(develop: &Value) -> Self {
        let group = &develop["raw"]["defective_pixels"];
        let list = group["list"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|p| {
                        let x = p.get(0)?.as_u64()?;
                        let y = p.get(1)?.as_u64()?;
                        Some((usize::try_from(x).ok()?, usize::try_from(y).ok()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            auto: group["auto"].as_bool().unwrap_or(false),
            threshold: get(group, "threshold", 50.0),
            list,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.auto && self.list.is_empty()
    }

    /// The gap, in normalized raw units, by which an automatic defect must
    /// exceed the range of its same-color neighbors.
    pub fn gap(&self) -> f32 {
        ((101.0 - self.threshold) / 200.0) as f32
    }
}

/// The same-color neighbors of `(x, y)` in channel `c`: the 5x5 window of a
/// one-channel CFA plane, or the 3x3 window otherwise.
fn neighbors(plane: &Plane, cfa: Option<Cfa>, x: usize, y: usize, c: usize, out: &mut Vec<f32>) {
    out.clear();
    let reach: isize = if cfa.is_some() { 2 } else { 1 };
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (nx, ny) = (x as isize + dx, y as isize + dy);
            if nx < 0 || ny < 0 || nx >= plane.width as isize || ny >= plane.height as isize {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if let Some(cfa) = cfa {
                if cfa.color(nx, ny) != cfa.color(x, y) {
                    continue;
                }
            }
            out.push(plane.data[(ny * plane.width + nx) * plane.channels + c]);
        }
    }
}

fn median(values: &mut [f32]) -> f32 {
    values.sort_by(f32::total_cmp);
    let n = values.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// Replace listed and (with `auto`) detected defective samples of the
/// normalized active plane by the median of their good same-color neighbors.
/// Returns the number of samples replaced.
pub fn fix_defects(plane: &mut Plane, cfa: Option<Cfa>, defects: &Defects) -> Result<usize> {
    if defects.is_empty() || plane.data.is_empty() {
        return Ok(0);
    }
    let cfa = cfa.filter(|_| plane.channels == 1);
    let (w, h, channels) = (plane.width, plane.height, plane.channels);
    let mut flagged = vec![0u8; w * h * channels];
    for &(x, y) in &defects.list {
        if x >= w || y >= h {
            bail!("[invalid-develop] raw.defective_pixels.list point [{x}, {y}] is outside the {w}x{h} active area; coordinates are active-area pixels")
        }
        for c in 0..channels {
            flagged[(y * w + x) * channels + c] = 1;
        }
    }
    if defects.auto {
        let gap = defects.gap();
        let source = &*plane;
        adjust::rows(&mut flagged, w * channels, &|y, row| {
            let mut around = Vec::with_capacity(24);
            for (i, flag) in row.iter_mut().enumerate() {
                let (x, c) = (i / channels, i % channels);
                neighbors(source, cfa, x, y, c, &mut around);
                if around.is_empty() {
                    continue;
                }
                let v = source.data[(y * w + x) * channels + c];
                let (lo, hi) = around
                    .iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), n| {
                        (lo.min(*n), hi.max(*n))
                    });
                if v > hi + gap || v < lo - gap || !v.is_finite() {
                    *flag = 1;
                }
            }
        })?;
    }
    let original = plane.data.clone();
    let source = Plane {
        width: w,
        height: h,
        channels,
        data: original,
    };
    let mut replaced = 0;
    let mut around = Vec::with_capacity(24);
    let mut good = Vec::with_capacity(24);
    for (i, _) in flagged.iter().enumerate().filter(|(_, f)| **f == 1) {
        let (pixel, c) = (i / channels, i % channels);
        let (x, y) = (pixel % w, pixel / w);
        neighbors(&source, cfa, x, y, c, &mut around);
        // Keep only neighbors that are not flagged themselves.
        good.clear();
        let reach: isize = if cfa.is_some() { 2 } else { 1 };
        let mut k = 0;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let (nx, ny) = (nx as usize, ny as usize);
                if cfa.is_some_and(|cfa| cfa.color(nx, ny) != cfa.color(x, y)) {
                    continue;
                }
                if flagged[(ny * w + nx) * channels + c] == 0 {
                    good.push(around[k]);
                }
                k += 1;
            }
        }
        let value = if good.is_empty() {
            median(&mut around)
        } else {
            median(&mut good)
        };
        plane.data[i] = value;
        replaced += 1;
    }
    Ok(replaced)
}

// ---------------------------------------------------------------------------
// Stage 2: noise reduction, moiré and defringe.

/// The stage-2 settings of a develop object; `None` when every control is off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Early {
    luminance: f64,
    luminance_detail: f64,
    luminance_contrast: f64,
    color: f64,
    color_detail: f64,
    color_smoothness: f64,
    moire: f64,
    /// `(amount 0–1, low hue°, high hue°)` for purple then green.
    fringes: [(f64, f64, f64); 2],
}

/// Oklab hue of a defringe hue slider: purple 30–70 maps to 240°–360°,
/// green 40–60 to 110°–170°.
fn fringe_hue(slider: f64, center: f64) -> f64 {
    center + (slider - 50.0) * 3.0
}

impl Early {
    pub fn new(develop: &Value) -> Option<Self> {
        let noise = &develop["detail"]["noise"];
        let defringe = &develop["lens"]["defringe"];
        let hue = |key: &str, default: [f64; 2]| -> [f64; 2] {
            match defringe[key]
                .as_array()
                .map(|h| (h[0].as_f64(), h[1].as_f64()))
            {
                Some((Some(lo), Some(hi))) => [lo, hi],
                _ => default,
            }
        };
        let purple = hue("purple_hue", [30.0, 70.0]);
        let green = hue("green_hue", [40.0, 60.0]);
        let early = Self {
            luminance: get(noise, "luminance", 0.0) / 100.0,
            luminance_detail: get(noise, "luminance_detail", 50.0) / 100.0,
            luminance_contrast: get(noise, "luminance_contrast", 0.0) / 100.0,
            color: get(noise, "color", 0.0) / 100.0,
            color_detail: get(noise, "color_detail", 50.0) / 100.0,
            color_smoothness: get(noise, "color_smoothness", 50.0) / 100.0,
            moire: get(&develop["detail"], "moire", 0.0) / 100.0,
            fringes: [
                (
                    get(defringe, "purple_amount", 0.0) / 20.0,
                    fringe_hue(purple[0], 300.0),
                    fringe_hue(purple[1], 300.0),
                ),
                (
                    get(defringe, "green_amount", 0.0) / 20.0,
                    fringe_hue(green[0], 140.0),
                    fringe_hue(green[1], 140.0),
                ),
            ],
        };
        let active = positive(early.luminance)
            || positive(early.color)
            || positive(early.moire)
            || early.fringes.iter().any(|f| positive(f.0));
        active.then_some(early)
    }

    /// Run stage 2 on interleaved camera RGB. `to_working` takes balanced
    /// camera RGB to linear ProPhoto; colors are judged in Oklab of that.
    pub fn apply(
        &self,
        rgb: &mut [f32],
        width: usize,
        height: usize,
        to_working: &Matrix,
    ) -> Result<()> {
        let n = width * height;
        if n == 0 {
            return Ok(());
        }
        let oklab = Oklab::new();
        let mut planes = [vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]];
        for (i, p) in rgb.chunks_exact(3).enumerate() {
            let lab = oklab.forward(color::apply(to_working, adjust::pixel(p)));
            for (plane, v) in planes.iter_mut().zip(lab) {
                plane[i] = v as f32;
            }
        }
        let [mut l, mut a, mut b] = planes;
        super::check_cancelled()?;

        if positive(self.luminance) {
            let sigma = 1.0 + 2.0 * (1.0 - self.luminance_detail);
            let eps = 0.08 * self.luminance;
            let smooth = guided(&l, width, height, sigma, (eps * eps) as f32)?;
            let keep = (0.5 * self.luminance_contrast) as f32;
            for (v, s) in l.iter_mut().zip(smooth) {
                *v = s + keep * (*v - s);
            }
        }
        if positive(self.color) {
            let sigma = (1.0 + 6.0 * self.color) * (0.5 + self.color_smoothness);
            let strength = (2.0 * self.color).min(1.0) as f32;
            let protect = (50.0 * self.color_detail) as f32;
            let edges = high_pass(&l, width, height, sigma)?;
            for plane in [&mut a, &mut b] {
                let mut blurred = plane.clone();
                gaussian(&mut blurred, width, height, sigma)?;
                for ((v, s), e) in plane.iter_mut().zip(&blurred).zip(&edges) {
                    let w = strength / (1.0 + protect * e.abs());
                    *v += w * (s - *v);
                }
            }
        }
        if positive(self.moire) {
            let sigma = 2.0 + 6.0 * self.moire;
            let mut ba = a.clone();
            let mut bb = b.clone();
            gaussian(&mut ba, width, height, sigma)?;
            gaussian(&mut bb, width, height, sigma)?;
            for i in 0..n {
                let hp = f64::from((a[i] - ba[i]).abs() + (b[i] - bb[i]).abs());
                let w = (self.moire * smoothstep(hp / 0.02)) as f32;
                a[i] += w * (ba[i] - a[i]);
                b[i] += w * (bb[i] - b[i]);
            }
        }
        for &(amount, low, high) in &self.fringes {
            if !positive(amount) {
                continue;
            }
            let sigma = 1.0 + 20.0 * amount / 4.0;
            let mut edges: Vec<f32> = high_pass(&l, width, height, sigma)?
                .into_iter()
                .map(f32::abs)
                .collect();
            gaussian(&mut edges, width, height, sigma)?;
            for i in 0..n {
                let (av, bv) = (f64::from(a[i]), f64::from(b[i]));
                if av == 0.0 && bv == 0.0 {
                    continue;
                }
                let mut hue = math::degrees(math::atan2(bv, av));
                if hue < 0.0 {
                    hue += 360.0;
                }
                let inside = hue_weight(hue, low, high);
                if inside == 0.0 {
                    continue;
                }
                let edge = smoothstep(f64::from(edges[i]) / 0.02);
                let k = (1.0 - amount * inside * edge).max(0.0) as f32;
                a[i] *= k;
                b[i] *= k;
            }
        }
        super::check_cancelled()?;

        let from_working = color::invert(to_working);
        for (i, p) in rgb.chunks_exact_mut(3).enumerate() {
            let lab = [f64::from(l[i]), f64::from(a[i]), f64::from(b[i])];
            adjust::store(p, color::apply(&from_working, oklab.back(lab)));
        }
        Ok(())
    }
}

/// 1 inside `[low, high]` degrees, fading linearly to 0 over 10° outside,
/// with hues compared around the circle.
fn hue_weight(hue: f64, low: f64, high: f64) -> f64 {
    let center = (low + high) / 2.0;
    let half = (high - low) / 2.0;
    let mut d = (hue - center).abs() % 360.0;
    if d > 180.0 {
        d = 360.0 - d;
    }
    if d <= half {
        1.0
    } else {
        (1.0 - (d - half) / 10.0).max(0.0)
    }
}

/// `plane - gaussian(plane)`.
fn high_pass(plane: &[f32], width: usize, height: usize, sigma: f64) -> Result<Vec<f32>> {
    let mut blurred = plane.to_vec();
    gaussian(&mut blurred, width, height, sigma)?;
    Ok(plane.iter().zip(blurred).map(|(v, b)| v - b).collect())
}

/// The self-guided filter (He, Sun and Tang 2013) with Gaussian windows.
fn guided(plane: &[f32], width: usize, height: usize, sigma: f64, eps: f32) -> Result<Vec<f32>> {
    let mut mean = plane.to_vec();
    gaussian(&mut mean, width, height, sigma)?;
    let mut square: Vec<f32> = plane.iter().map(|v| v * v).collect();
    gaussian(&mut square, width, height, sigma)?;
    let mut a = vec![0.0f32; plane.len()];
    let mut b = vec![0.0f32; plane.len()];
    for i in 0..plane.len() {
        let variance = (square[i] - mean[i] * mean[i]).max(0.0);
        a[i] = variance / (variance + eps);
        b[i] = mean[i] - a[i] * mean[i];
    }
    gaussian(&mut a, width, height, sigma)?;
    gaussian(&mut b, width, height, sigma)?;
    Ok(plane
        .iter()
        .zip(a.iter().zip(&b))
        .map(|(v, (a, b))| a * v + b)
        .collect())
}

// ---------------------------------------------------------------------------
// Capture sharpening, at the end of stage 9.

/// Unsharp masking of log2 working luminance. Every pixel's RGB is scaled by
/// the same factor, so hue and saturation are kept and the result does not
/// depend on exposure.
pub fn sharpen(image: &mut Working, develop: &Value) -> Result<()> {
    let group = &develop["detail"]["sharpening"];
    let amount = get(group, "amount", 0.0) / 100.0;
    if !positive(amount) || image.rgb.is_empty() {
        return Ok(());
    }
    let sigma = get(group, "radius", 1.0);
    let threshold = 0.05 + 0.95 * get(group, "detail", 25.0) / 100.0;
    let masking = get(group, "masking", 0.0) / 100.0;
    let (w, h) = (image.width as usize, image.height as usize);
    let weights = adjust::luminance_weights();
    let stops: Vec<f32> = image
        .rgb
        .chunks_exact(3)
        .map(|p| {
            let y = adjust::luminance(&weights, adjust::pixel(p));
            math::log2(if y > FLOOR { y } else { FLOOR }) as f32
        })
        .collect();
    let mut blurred = stops.clone();
    adjust::gaussian_weighted(&mut blurred, image.alpha.as_deref(), w, h, sigma)?;
    let mut detail: Vec<f32> = stops.iter().zip(&blurred).map(|(s, b)| s - b).collect();
    drop(blurred);
    let mask = if positive(masking) {
        let mut edges: Vec<f32> = detail.iter().map(|d| d.abs()).collect();
        adjust::gaussian_weighted(&mut edges, image.alpha.as_deref(), w, h, sigma)?;
        let level = 0.25 * masking;
        Some(
            edges
                .into_iter()
                .map(|e| smoothstep((f64::from(e) - level) / level) as f32)
                .collect::<Vec<f32>>(),
        )
    } else {
        None
    };
    for (i, d) in detail.iter_mut().enumerate() {
        let v = f64::from(*d);
        let damped = v / (1.0 + v.abs() / threshold);
        let m = mask.as_ref().map_or(1.0, |m| f64::from(m[i]));
        *d = (amount * damped * m) as f32;
    }
    adjust::pixels(image, &|i, p, alpha| {
        if alpha <= 0.0 {
            return;
        }
        let rgb = adjust::pixel(p);
        if !positive(adjust::luminance(&weights, rgb)) {
            return;
        }
        let k = math::exp2(f64::from(detail[i]));
        adjust::store(p, rgb.map(|v| v * k));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn listed_and_hot_pixels_take_the_neighbor_median() {
        let mut plane = Plane {
            width: 6,
            height: 6,
            channels: 1,
            data: vec![0.2; 36],
        };
        plane.data[2 * 6 + 2] = 0.95; // hot
        plane.data[3 * 6 + 4] = 0.0; // listed
        let defects = Defects {
            auto: true,
            threshold: 50.0,
            list: vec![(4, 3)],
        };
        let fixed = fix_defects(&mut plane, Some(Cfa::Rggb), &defects).unwrap();
        assert_eq!(fixed, 2);
        assert!(plane.data.iter().all(|v| (*v - 0.2).abs() < 1e-6));
        let outside = Defects {
            list: vec![(6, 0)],
            ..Defects::default()
        };
        let error = fix_defects(&mut plane, None, &outside).unwrap_err();
        assert!(error.to_string().contains("[invalid-develop]"));
    }

    #[test]
    fn inactive_settings_skip_stage_two() {
        assert!(Early::new(&json!({})).is_none());
        assert!(Early::new(&json!({"detail": {"noise": {"color": 0}}})).is_none());
        assert!(Early::new(&json!({"detail": {"noise": {"color": 25}}})).is_some());
        assert!(Early::new(&json!({"lens": {"defringe": {"purple_amount": 5}}})).is_some());
    }

    #[test]
    fn sharpening_does_not_depend_on_exposure() {
        let mut image = Working::new(16, 16, false).unwrap();
        for (i, p) in image.rgb.chunks_exact_mut(3).enumerate() {
            let v = if (i % 16) < 8 { 0.1 } else { 0.4 };
            p.copy_from_slice(&[v, v * 0.9, v * 0.8]);
        }
        let mut brighter = image.clone();
        for v in &mut brighter.rgb {
            *v *= 4.0;
        }
        let develop =
            json!({"detail": {"sharpening": {"amount": 80, "radius": 1.5, "detail": 30}}});
        let before = image.rgb.clone();
        sharpen(&mut image, &develop).unwrap();
        sharpen(&mut brighter, &develop).unwrap();
        assert_ne!(before, image.rgb);
        for (a, b) in image.rgb.iter().zip(&brighter.rgb) {
            assert!((a * 4.0 - b).abs() < 1e-5 * b.abs().max(1.0));
        }
        // The edge is steeper after sharpening.
        assert!(image.rgb[7 * 3] < before[7 * 3]);
        assert!(image.rgb[8 * 3] > before[8 * 3]);
    }

    #[test]
    fn hue_windows_wrap() {
        assert_eq!(hue_weight(355.0, 240.0, 360.0), 1.0);
        assert_eq!(hue_weight(5.0, 240.0, 360.0), 0.5);
        assert_eq!(hue_weight(140.0, 240.0, 360.0), 0.0);
    }
}

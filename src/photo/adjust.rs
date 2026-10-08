//! Development stages 6–10 of process 1 (tone, presence, curves, color and
//! effects) and the stage-3 calibration. The kernels are specified in the
//! "Development stack" section of `docs/photography-v1.md`.
//!
//! An absent group, or one whose parameters are all at identity, skips its
//! kernel, so the pixels pass through bit for bit. Every kernel uses only
//! `+ - * /`, `sqrt` and the in-tree [`math`] functions.
use super::color::{self, Matrix, Transfer, D50, D65};
use super::math;
use super::pixels::Working;
use anyhow::{bail, Result};
use serde_json::Value;
use std::sync::OnceLock;

/// Middle gray in scene-linear working luminance.
pub const GRAY: f64 = 0.18;
/// The luminance floor of the log domain: 20 stops below middle gray.
pub const FLOOR: f64 = GRAY / 1_048_576.0;
/// The long edge of the analysis proxy used by auto tone and dehaze.
pub const ANALYSIS_EDGE: usize = 1024;

/// What the spatial and positional kernels need to know about the frame.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    /// The DNG `BaselineExposure`, in EV, added to `tone.exposure`.
    pub baseline_exposure: f64,
    /// The long edge, in pixels, of the developed, uncropped (oriented) frame.
    /// Radii are fractions of it.
    pub long_edge: f64,
    /// The image's top-left corner in that frame, so grain stays put when the
    /// crop changes.
    pub origin: (usize, usize),
}

fn value(object: &Value, key: &str) -> f64 {
    object.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn percent(object: &Value, key: &str) -> f64 {
    value(object, key) / 100.0
}

fn or(object: &Value, key: &str, default: f64) -> f64 {
    object.get(key).and_then(Value::as_f64).unwrap_or(default)
}

/// The Y row of linear ProPhoto to XYZ D50: working-space luminance.
pub fn luminance_weights() -> [f64; 3] {
    static WEIGHTS: OnceLock<[f64; 3]> = OnceLock::new();
    *WEIGHTS.get_or_init(|| color::ColorSpace::working().to_xyz()[1])
}

fn luminance(w: &[f64; 3], p: [f64; 3]) -> f64 {
    w[0] * p[0] + w[1] * p[1] + w[2] * p[2]
}

fn pixel(p: &[f32]) -> [f64; 3] {
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}

fn store(p: &mut [f32], v: [f64; 3]) {
    p[0] = v[0] as f32;
    p[1] = v[1] as f32;
    p[2] = v[2] as f32;
}

/// `(1 - t²)²` on `t = (s - center) / half_width`, zero outside `|t| < 1`.
fn bump(s: f64, center: f64, half_width: f64) -> f64 {
    let t = (s - center) / half_width;
    if t <= -1.0 || t >= 1.0 || t.is_nan() {
        0.0
    } else {
        let u = 1.0 - t * t;
        u * u
    }
}

/// `3t² - 2t³` on `t` clamped to [0, 1].
fn smoothstep(t: f64) -> f64 {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    t * t * (3.0 - 2.0 * t)
}

/// Round half up to `step` (a power of ten such as 0.01).
pub fn round_to(v: f64, step: f64) -> f64 {
    ((v / step) + 0.5).floor() * step
}

/// `v > 0`, false for NaN.
fn positive(v: f64) -> bool {
    v > 0.0
}

/// Stops of `y` relative to middle gray, floored at [`FLOOR`].
fn stops(y: f64) -> f64 {
    math::log2(if y > FLOOR { y } else { FLOOR } / GRAY)
}

// ---------------------------------------------------------------------------
// Parallel helpers. Rows are split into contiguous blocks, one per thread, and
// every row is computed independently, so the result does not depend on the
// thread count. Cancellation is checked on the calling thread after a pass.

fn rows<T: Send>(data: &mut [T], width: usize, f: &(dyn Fn(usize, &mut [T]) + Sync)) -> Result<()> {
    if data.is_empty() || width == 0 {
        return Ok(());
    }
    let height = data.len() / width;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 16);
    let per = height.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (n, block) in data.chunks_mut(per * width).enumerate() {
            scope.spawn(move || {
                for (k, row) in block.chunks_mut(width).enumerate() {
                    f(n * per + k, row);
                }
            });
        }
    });
    super::check_cancelled()
}

/// Apply `f(index, rgb, alpha)` to every pixel of `image`.
fn pixels(image: &mut Working, f: &(dyn Fn(usize, &mut [f32], f32) + Sync)) -> Result<()> {
    let width = image.width as usize;
    let alpha = image.alpha.as_deref();
    rows(&mut image.rgb, width * 3, &|y, row| {
        for (x, p) in row.chunks_exact_mut(3).enumerate() {
            let i = y * width + x;
            f(i, p, alpha.map_or(1.0, |a| a[i]));
        }
    })
}

fn transpose(plane: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0; plane.len()];
    for y in 0..height {
        for x in 0..width {
            out[x * height + y] = plane[y * width + x];
        }
    }
    out
}

/// Box radii of three passes approximating a Gaussian of `sigma` pixels.
fn box_radii(sigma: f64) -> [usize; 3] {
    let ideal = (4.0 * sigma * sigma + 1.0).sqrt();
    let mut low = ideal.floor() as i64;
    if low % 2 == 0 {
        low -= 1;
    }
    let low = low.max(1);
    let l = low as f64;
    let m = ((12.0 * sigma * sigma - 3.0 * l * l - 12.0 * l - 9.0) / (-4.0 * l - 4.0) + 0.5).floor()
        as i64;
    std::array::from_fn(|i| {
        let size = if (i as i64) < m { low } else { low + 2 };
        ((size - 1) / 2) as usize
    })
}

/// One box pass with clamp-to-edge borders and an `f64` running sum.
fn box_row(src: &[f32], out: &mut [f32], r: usize) {
    let n = src.len() as isize;
    let at = |i: isize| f64::from(src[i.clamp(0, n - 1) as usize]);
    let size = (2 * r + 1) as f64;
    let r = r as isize;
    let mut sum = 0.0;
    for k in -r..=r {
        sum += at(k);
    }
    for (i, o) in out.iter_mut().enumerate() {
        *o = (sum / size) as f32;
        let i = i as isize;
        sum += at(i + r + 1) - at(i - r);
    }
}

fn blur_rows(plane: &mut [f32], width: usize, radii: &[usize; 3]) -> Result<()> {
    rows(plane, width, &|_, row| {
        let mut tmp = vec![0.0; row.len()];
        for &r in radii {
            if r > 0 {
                box_row(row, &mut tmp, r);
                row.copy_from_slice(&tmp);
            }
        }
    })
}

/// Gaussian blur of a `width` x `height` plane: three box passes per axis.
/// A `sigma` below half a pixel leaves the plane unchanged.
pub fn gaussian(plane: &mut Vec<f32>, width: usize, height: usize, sigma: f64) -> Result<()> {
    if sigma.is_nan() || sigma < 0.5 || width == 0 || height == 0 {
        return Ok(());
    }
    let radii = box_radii(sigma);
    blur_rows(plane, width, &radii)?;
    let mut columns = transpose(plane, width, height);
    blur_rows(&mut columns, height, &radii)?;
    *plane = transpose(&columns, height, width);
    Ok(())
}

/// A Gaussian blur that ignores invalid (alpha 0) pixels: the blur of
/// `value · alpha` divided by the blur of `alpha`.
fn gaussian_weighted(
    plane: &mut Vec<f32>,
    alpha: Option<&[f32]>,
    width: usize,
    height: usize,
    sigma: f64,
) -> Result<()> {
    let Some(alpha) = alpha else {
        return gaussian(plane, width, height, sigma);
    };
    let mut weighted: Vec<f32> = plane.iter().zip(alpha).map(|(v, a)| v * a).collect();
    let mut weights = alpha.to_vec();
    gaussian(&mut weighted, width, height, sigma)?;
    gaussian(&mut weights, width, height, sigma)?;
    for ((v, w), out) in weighted.iter().zip(&weights).zip(plane.iter_mut()) {
        if *w > 1.0e-6 {
            *out = v / w;
        }
    }
    Ok(())
}

/// Running minimum over a window of `2r + 1` (van Herk / Gil–Werman), with
/// samples outside the row ignored.
fn min_row(src: &[f32], out: &mut [f32], r: usize) {
    let n = src.len();
    let k = 2 * r + 1;
    let len = (n + 2 * r).div_ceil(k) * k;
    let get = |i: usize| {
        if i < r || i >= r + n {
            f32::INFINITY
        } else {
            src[i - r]
        }
    };
    let mut forward = vec![0.0f32; len];
    let mut backward = vec![0.0f32; len];
    for i in 0..len {
        forward[i] = if i % k == 0 {
            get(i)
        } else {
            forward[i - 1].min(get(i))
        };
    }
    for i in (0..len).rev() {
        backward[i] = if i % k == k - 1 || i == len - 1 {
            get(i)
        } else {
            backward[i + 1].min(get(i))
        };
    }
    for (i, o) in out.iter_mut().enumerate() {
        *o = backward[i].min(forward[i + k - 1]);
    }
}

/// Minimum filter over a `(2r + 1)²` square.
pub fn min_filter(plane: &mut Vec<f32>, width: usize, height: usize, r: usize) -> Result<()> {
    if r == 0 || width == 0 || height == 0 {
        return Ok(());
    }
    let pass = |plane: &mut [f32], width: usize| {
        rows(plane, width, &|_, row| {
            let mut tmp = vec![0.0; row.len()];
            min_row(row, &mut tmp, r);
            row.copy_from_slice(&tmp);
        })
    };
    pass(plane, width)?;
    let mut columns = transpose(plane, width, height);
    pass(&mut columns, height)?;
    *plane = transpose(&columns, height, width);
    Ok(())
}

// ---------------------------------------------------------------------------
// Stage 3: calibration.

/// The calibration matrix (working RGB to working RGB) and shadows tint.
#[derive(Debug, Clone)]
pub struct Calibration {
    pub matrix: Matrix,
    /// `shadows_tint / 100`.
    pub shadows_tint: f64,
}

/// The `calibration` group, or `None` when it is absent or at identity.
pub fn calibration(develop: &Value) -> Option<Calibration> {
    let group = develop.get("calibration")?;
    let primaries = ["red", "green", "blue"].map(|key| {
        let p = &group[key];
        (percent(p, "hue"), percent(p, "saturation"))
    });
    let shadows_tint = percent(group, "shadows_tint");
    let identity = primaries.iter().all(|&(h, s)| h == 0.0 && s == 0.0);
    if identity && shadows_tint == 0.0 {
        return None;
    }
    let matrix = if identity {
        color::identity()
    } else {
        let third = 1.0 / 3.0;
        let axis = 1.0 / 3.0f64.sqrt();
        let mut m = [[0.0; 3]; 3];
        for (c, &(hue, saturation)) in primaries.iter().enumerate() {
            // The primary's offset from gray, rotated about the gray axis and scaled.
            let d: [f64; 3] = std::array::from_fn(|i| if i == c { 1.0 - third } else { -third });
            let cross = [
                axis * (d[2] - d[1]),
                axis * (d[0] - d[2]),
                axis * (d[1] - d[0]),
            ];
            let (sin, cos) = math::sin_cos(math::radians(20.0 * hue));
            let scale = 1.0 + 0.5 * saturation;
            for (i, row) in m.iter_mut().enumerate() {
                row[c] = third + scale * (d[i] * cos + cross[i] * sin);
            }
        }
        // Keep the working white neutral: every row sums to 1.
        for row in &mut m {
            let sum = row[0] + row[1] + row[2];
            for cell in row.iter_mut() {
                *cell /= sum;
            }
        }
        m
    };
    Some(Calibration {
        matrix,
        shadows_tint,
    })
}

/// The calibration shadows tint: green scaled by `2^(-0.25 · amount · w)`,
/// `w = (1 - Y / 0.05)²` below Y = 0.05 (1 at or below 0).
pub fn shadows_tint(rgb: [f64; 3], amount: f64) -> [f64; 3] {
    let y = luminance(&luminance_weights(), rgb);
    let w = if !positive(y) {
        1.0
    } else if y >= 0.05 {
        return rgb;
    } else {
        let t = 1.0 - y / 0.05;
        t * t
    };
    [rgb[0], rgb[1] * math::exp2(-0.25 * amount * w), rgb[2]]
}

// ---------------------------------------------------------------------------
// Stage 6: tone.

#[derive(Debug, Clone, Copy)]
pub struct Tone {
    gain: f64,
    contrast: f64,
    highlights: f64,
    shadows: f64,
    whites: f64,
    blacks: f64,
}

impl Tone {
    pub fn new(develop: &Value, context: &Context) -> Option<Self> {
        let t = &develop["tone"];
        let exposure = value(t, "exposure") + context.baseline_exposure;
        let tone = Self {
            gain: math::exp2(exposure),
            contrast: percent(t, "contrast"),
            highlights: percent(t, "highlights"),
            shadows: percent(t, "shadows"),
            whites: percent(t, "whites"),
            blacks: percent(t, "blacks"),
        };
        (exposure != 0.0 || tone.curved()).then_some(tone)
    }

    fn curved(&self) -> bool {
        self.contrast != 0.0
            || self.highlights != 0.0
            || self.shadows != 0.0
            || self.whites != 0.0
            || self.blacks != 0.0
    }

    /// The tone curve on stops from middle gray, after exposure.
    fn curve(&self, s: f64) -> f64 {
        s * (1.0 + 0.6 * self.contrast)
            + self.shadows * bump(s, -3.0, 3.0)
            + self.highlights * bump(s, 1.2, 2.2)
            + 0.6 * self.whites * bump(s, 2.5, 1.5)
            + 0.6 * self.blacks * bump(s, -6.5, 3.5)
    }

    fn gain(&self, rgb: [f64; 3], weights: &[f64; 3]) -> f64 {
        if !self.curved() {
            return self.gain;
        }
        let y = luminance(weights, rgb) * self.gain;
        if !positive(y) || !y.is_finite() {
            return self.gain;
        }
        let s = stops(y);
        self.gain * math::exp2(self.curve(s) - s)
    }

    pub fn apply(&self, image: &mut Working) -> Result<()> {
        let weights = luminance_weights();
        pixels(image, &|_, p, _| {
            let rgb = pixel(p);
            let k = self.gain(rgb, &weights);
            store(p, rgb.map(|v| v * k));
        })
    }
}

// ---------------------------------------------------------------------------
// Stage 7: presence (dehaze, clarity, texture).

fn dehaze(image: &mut Working, amount: f64, airlight: [f64; 3], context: &Context) -> Result<()> {
    let (width, height) = (image.width as usize, image.height as usize);
    let a = airlight.map(|v| v.max(1.0e-6));
    if amount < 0.0 {
        let t = 1.0 + 0.6 * amount;
        return pixels(image, &|_, p, _| {
            let rgb = pixel(p);
            store(p, std::array::from_fn(|c| rgb[c] * t + a[c] * (1.0 - t)));
        });
    }
    let mut dark: Vec<f32> = image
        .rgb
        .chunks_exact(3)
        .map(|p| {
            let rgb = pixel(p);
            let m = (0..3)
                .map(|c| rgb[c].max(0.0) / a[c])
                .fold(f64::INFINITY, f64::min);
            m as f32
        })
        .collect();
    let radius = patch_radius(context.long_edge);
    min_filter(&mut dark, width, height, radius)?;
    gaussian(&mut dark, width, height, radius as f64)?;
    let strength = 0.95 * amount;
    pixels(image, &|i, p, _| {
        let t = (1.0 - strength * f64::from(dark[i])).max(0.1);
        let rgb = pixel(p);
        store(p, std::array::from_fn(|c| (rgb[c] - a[c]) / t + a[c]));
    })
}

/// The dark-channel patch radius: 0.6% of the long edge, at least 1 pixel.
pub fn patch_radius(long_edge: f64) -> usize {
    (round_to(0.006 * long_edge, 1.0) as usize).max(1)
}

fn local_contrast(
    image: &mut Working,
    clarity: f64,
    texture: f64,
    context: &Context,
) -> Result<()> {
    let (width, height) = (image.width as usize, image.height as usize);
    let weights = luminance_weights();
    let original: Vec<f32> = image
        .rgb
        .chunks_exact(3)
        .map(|p| stops(luminance(&weights, pixel(p))) as f32)
        .collect();
    let alpha = image.alpha.as_deref();
    let mut s = original.clone();
    if clarity != 0.0 {
        let mut base = s.clone();
        gaussian_weighted(&mut base, alpha, width, height, 0.015 * context.long_edge)?;
        for (v, b) in s.iter_mut().zip(&base) {
            let x = f64::from(*v);
            *v = (x + 0.6 * clarity * (x - f64::from(*b)) * bump(x, -1.0, 7.0)) as f32;
        }
    }
    if texture != 0.0 {
        let mut base = s.clone();
        let sigma = (0.002 * context.long_edge).max(0.8);
        gaussian_weighted(&mut base, alpha, width, height, sigma)?;
        for (v, b) in s.iter_mut().zip(&base) {
            let x = f64::from(*v);
            *v = (x + 0.8 * texture * (x - f64::from(*b))) as f32;
        }
    }
    pixels(image, &|i, p, _| {
        let rgb = pixel(p);
        if !positive(luminance(&weights, rgb)) {
            return;
        }
        let k = math::exp2(f64::from(s[i]) - f64::from(original[i]));
        store(p, rgb.map(|v| v * k));
    })
}

/// `presence.dehaze_airlight`, required whenever `presence.dehaze` is not 0.
pub fn airlight(develop: &Value) -> Result<Option<[f64; 3]>> {
    let presence = &develop["presence"];
    if value(presence, "dehaze") == 0.0 {
        return Ok(None);
    }
    let Some(stored) = presence.get("dehaze_airlight").and_then(Value::as_array) else {
        bail!("[invalid-develop] presence.dehaze needs presence.dehaze_airlight; run `raw develop` on the variant to resolve it")
    };
    let a: Vec<f64> = stored.iter().filter_map(Value::as_f64).collect();
    if a.len() != 3 {
        bail!("[invalid-develop] presence.dehaze_airlight must hold three numbers")
    }
    Ok(Some([a[0], a[1], a[2]]))
}

fn presence(image: &mut Working, develop: &Value, context: &Context) -> Result<()> {
    let group = &develop["presence"];
    if let Some(a) = airlight(develop)? {
        dehaze(image, percent(group, "dehaze"), a, context)?;
    }
    let (clarity, texture) = (percent(group, "clarity"), percent(group, "texture"));
    if clarity != 0.0 || texture != 0.0 {
        local_contrast(image, clarity, texture, context)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Stage 8: curves.

const LUT: usize = 4096;

/// Monotone cubic (Fritsch–Carlson) through 2–16 points, flat outside them.
struct Hermite {
    x: Vec<f64>,
    y: Vec<f64>,
    m: Vec<f64>,
}

impl Hermite {
    fn new(points: &[(f64, f64)]) -> Self {
        let n = points.len();
        let x: Vec<f64> = points.iter().map(|p| p.0).collect();
        let y: Vec<f64> = points.iter().map(|p| p.1).collect();
        let d: Vec<f64> = (0..n - 1)
            .map(|k| (y[k + 1] - y[k]) / (x[k + 1] - x[k]))
            .collect();
        let mut m = vec![0.0; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for k in 1..n - 1 {
            m[k] = if d[k - 1] * d[k] <= 0.0 {
                0.0
            } else {
                (d[k - 1] + d[k]) / 2.0
            };
        }
        for k in 0..n - 1 {
            if d[k] == 0.0 {
                m[k] = 0.0;
                m[k + 1] = 0.0;
                continue;
            }
            let (a, b) = (m[k] / d[k], m[k + 1] / d[k]);
            let r = a * a + b * b;
            if r > 9.0 {
                let t = 3.0 / r.sqrt();
                m[k] = t * a * d[k];
                m[k + 1] = t * b * d[k];
            }
        }
        Self { x, y, m }
    }

    fn eval(&self, v: f64) -> f64 {
        let n = self.x.len();
        if v <= self.x[0] {
            return self.y[0];
        }
        if v >= self.x[n - 1] {
            return self.y[n - 1];
        }
        let k = (0..n - 1).find(|&k| v < self.x[k + 1]).unwrap_or(n - 2);
        let h = self.x[k + 1] - self.x[k];
        let t = (v - self.x[k]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.y[k]
            + (t3 - 2.0 * t2 + t) * h * self.m[k]
            + (-2.0 * t3 + 3.0 * t2) * self.y[k + 1]
            + (t3 - t2) * h * self.m[k + 1]
    }
}

fn point_curve(curves: &Value, key: &str) -> Option<Hermite> {
    let points: Vec<(f64, f64)> = curves["point"]
        .get(key)?
        .as_array()?
        .iter()
        .filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
        .collect();
    let identity = points.first() == Some(&(0.0, 0.0))
        && points.last() == Some(&(1.0, 1.0))
        && points.iter().all(|p| p.0 == p.1);
    (points.len() >= 2 && !identity).then(|| Hermite::new(&points))
}

/// The parametric curve on `LUT + 1` samples, made non-decreasing.
fn parametric(curves: &Value) -> Option<Vec<f64>> {
    let p = curves.get("parametric")?;
    let amounts = ["shadows", "darks", "lights", "highlights"].map(|k| percent(p, k));
    if amounts.iter().all(|a| *a == 0.0) {
        return None;
    }
    let splits: Vec<f64> = p
        .get("splits")
        .and_then(Value::as_array)
        .map(|s| s.iter().filter_map(Value::as_f64).collect())
        .filter(|s: &Vec<f64>| s.len() == 3)
        .unwrap_or_else(|| vec![0.25, 0.5, 0.75]);
    let edges = [0.0, splits[0], splits[1], splits[2], 1.0];
    let mut lut = Vec::with_capacity(LUT + 1);
    let mut previous = 0.0f64;
    for i in 0..=LUT {
        let x = i as f64 / LUT as f64;
        let q = 1.0 - 2.0 * x;
        let q2 = q * q;
        let q4 = q2 * q2;
        let envelope = 1.0 - q4 * q4;
        let offset: f64 = (0..4)
            .map(|k| {
                let center = (edges[k] + edges[k + 1]) / 2.0;
                amounts[k] * 0.15 * bump(x, center, edges[k + 1] - edges[k])
            })
            .sum();
        let y = (x + envelope * offset).clamp(0.0, 1.0).max(previous);
        previous = y;
        lut.push(y);
    }
    Some(lut)
}

struct Curves {
    /// Per channel, `LUT + 1` samples of the composed curve in ROMM encoding.
    luts: [Vec<f64>; 3],
}

impl Curves {
    fn new(develop: &Value) -> Option<Self> {
        let curves = develop.get("curves")?;
        let param = parametric(curves);
        let master = point_curve(curves, "rgb");
        let channels = ["red", "green", "blue"].map(|k| point_curve(curves, k));
        if param.is_none() && master.is_none() && channels.iter().all(Option::is_none) {
            return None;
        }
        let luts = std::array::from_fn(|c| {
            (0..=LUT)
                .map(|i| {
                    let mut v = match &param {
                        Some(lut) => lut[i],
                        None => i as f64 / LUT as f64,
                    };
                    if let Some(curve) = &master {
                        v = curve.eval(v).clamp(0.0, 1.0);
                    }
                    if let Some(curve) = &channels[c] {
                        v = curve.eval(v).clamp(0.0, 1.0);
                    }
                    v
                })
                .collect()
        });
        Some(Self { luts })
    }

    fn lookup(lut: &[f64], e: f64) -> f64 {
        let x = e * LUT as f64;
        let i = (x.floor() as usize).min(LUT - 1);
        let f = x - i as f64;
        lut[i] + f * (lut[i + 1] - lut[i])
    }

    /// Linear in, linear out: inside [0, 1] through the ROMM-encoded curve;
    /// above 1 scaled by the curve at 1, below 0 offset from the curve at 0.
    fn channel(&self, c: usize, v: f64) -> f64 {
        let lut = &self.luts[c];
        if v > 1.0 {
            Transfer::Romm.decode(lut[LUT]) * v
        } else if v < 0.0 || v.is_nan() {
            Transfer::Romm.decode(lut[0]) + if v.is_nan() { 0.0 } else { v }
        } else {
            Transfer::Romm.decode(Self::lookup(lut, Transfer::Romm.encode(v)))
        }
    }

    fn apply(&self, image: &mut Working) -> Result<()> {
        pixels(image, &|_, p, _| {
            let rgb = pixel(p);
            store(p, std::array::from_fn(|c| self.channel(c, rgb[c])));
        })
    }
}

// ---------------------------------------------------------------------------
// Stage 9: color (in Oklab built on the working space).

/// The eight hue bands and their centers in Oklab hue degrees.
pub const BANDS: [&str; 8] = [
    "red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta",
];
const CENTERS: [f64; 8] = [20.0, 55.0, 100.0, 140.0, 195.0, 255.0, 295.0, 335.0];

/// The two bands around hue `h` (degrees in [0, 360)) and their weights.
fn band_weights(h: f64) -> [(usize, f64); 2] {
    for i in 0..8 {
        let a = CENTERS[i];
        let b = if i == 7 {
            CENTERS[0] + 360.0
        } else {
            CENTERS[i + 1]
        };
        let hh = if h < a { h + 360.0 } else { h };
        if hh >= a && hh < b {
            let s = smoothstep((hh - a) / (b - a));
            return [(i, 1.0 - s), ((i + 1) % 8, s)];
        }
    }
    [(0, 1.0), (0, 0.0)]
}

fn band_value(values: &[f64; 8], weights: &[(usize, f64); 2]) -> f64 {
    values[weights[0].0] * weights[0].1 + values[weights[1].0] * weights[1].1
}

fn bands(group: &Value) -> [f64; 8] {
    BANDS.map(|band| percent(group, band))
}

/// A cube root by bit-level estimate and five Newton steps; exact zero
/// below 1e-30.
fn cbrt(v: f64) -> f64 {
    let x = v.abs();
    if !x.ge(&1.0e-30) || !x.is_finite() {
        return if x.is_finite() { 0.0 } else { v };
    }
    let mut y = f64::from_bits(x.to_bits() / 3 + 0x2A9F_7893_782D_A1CE);
    for _ in 0..5 {
        y -= (y * y * y - x) / (3.0 * y * y);
    }
    y.copysign(v)
}

/// Oklab (Ottosson 2020) on linear working RGB: Bradford D50→D65, the Oklab
/// M1 with its rows scaled so the D65 white is exactly (1, 1, 1), the cube
/// root and M2.
pub struct Oklab {
    to_lms: Matrix,
    from_lms: Matrix,
    m2_inverse: Matrix,
}

const OKLAB_M1: Matrix = [
    [0.818_933_010_1, 0.361_866_742_4, -0.128_859_713_7],
    [0.032_984_543_6, 0.929_311_871_5, 0.036_145_638_7],
    [0.048_200_301_8, 0.264_366_269_1, 0.633_851_707_0],
];
const OKLAB_M2: Matrix = [
    [0.210_454_255_3, 0.793_617_785_0, -0.004_072_046_8],
    [1.977_998_495_1, -2.428_592_205_0, 0.450_593_709_9],
    [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766_0],
];

impl Oklab {
    pub fn new() -> Self {
        let working = color::ColorSpace::working();
        let xyz = color::multiply(&color::bradford(D50, D65), &working.to_xyz());
        let white = color::apply(&OKLAB_M1, color::xyz(D65));
        let mut m1 = OKLAB_M1;
        for (row, w) in m1.iter_mut().zip(white) {
            for cell in row.iter_mut() {
                *cell /= w;
            }
        }
        let to_lms = color::multiply(&m1, &xyz);
        Self {
            to_lms,
            from_lms: color::invert(&to_lms),
            m2_inverse: color::invert(&OKLAB_M2),
        }
    }

    pub fn forward(&self, rgb: [f64; 3]) -> [f64; 3] {
        color::apply(&OKLAB_M2, color::apply(&self.to_lms, rgb).map(cbrt))
    }

    pub fn back(&self, lab: [f64; 3]) -> [f64; 3] {
        let lms = color::apply(&self.m2_inverse, lab).map(|v| v * v * v);
        color::apply(&self.from_lms, lms)
    }
}

impl Default for Oklab {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
struct Wheel {
    da: f64,
    db: f64,
    dl: f64,
}

struct Grading {
    /// Shadows, midtones, highlights, global.
    wheels: [Wheel; 4],
    middle: f64,
    width: f64,
}

struct ColorStage {
    hue: [f64; 8],
    saturation: [f64; 8],
    luminance: [f64; 8],
    monochrome: Option<[f64; 8]>,
    vibrance: f64,
    global_saturation: f64,
    grading: Option<Grading>,
    oklab: Oklab,
}

impl ColorStage {
    fn new(develop: &Value) -> Option<Self> {
        let hsl = &develop["hsl"];
        let mono = &develop["monochrome"];
        let presence = &develop["presence"];
        let grading = &develop["grading"];
        let wheels = ["shadows", "midtones", "highlights", "global"].map(|key| {
            let w = &grading[key];
            let (sin, cos) = math::sin_cos(math::radians(value(w, "hue")));
            let saturation = 0.06 * percent(w, "saturation");
            Wheel {
                da: saturation * cos,
                db: saturation * sin,
                dl: 0.1 * percent(w, "luminance"),
            }
        });
        let graded = wheels
            .iter()
            .any(|w| w.da != 0.0 || w.db != 0.0 || w.dl != 0.0);
        let stage = Self {
            hue: bands(&hsl["hue"]),
            saturation: bands(&hsl["saturation"]),
            luminance: bands(&hsl["luminance"]),
            monochrome: (mono.get("enabled") == Some(&Value::Bool(true)))
                .then(|| bands(&mono["mix"])),
            vibrance: percent(presence, "vibrance"),
            global_saturation: percent(presence, "saturation"),
            grading: graded.then(|| Grading {
                wheels,
                middle: 0.5 - 0.25 * percent(grading, "balance"),
                width: 0.1 + 0.4 * or(grading, "blending", 50.0) / 100.0,
            }),
            oklab: Oklab::new(),
        };
        let hsl_active = [stage.hue, stage.saturation, stage.luminance]
            .iter()
            .any(|b| b.iter().any(|v| *v != 0.0));
        let active = stage.monochrome.is_some()
            || (hsl_active || stage.vibrance != 0.0 || stage.global_saturation != 0.0)
            || stage.grading.is_some();
        active.then_some(stage)
    }

    fn pixel(&self, rgb: [f64; 3]) -> [f64; 3] {
        let [mut l, mut a, mut b] = self.oklab.forward(rgb);
        let c2 = a * a + b * b;
        let c0 = c2.sqrt();
        let q = c2 / (c2 + 0.0004);
        let hue = if c0 > 0.0 {
            let h = math::degrees(math::atan2(b, a));
            if h < 0.0 {
                h + 360.0
            } else {
                h
            }
        } else {
            0.0
        };
        let weights = band_weights(hue);
        if let Some(mix) = &self.monochrome {
            let f = band_value(mix, &weights);
            if f != 0.0 {
                l *= math::exp2(f * q / 3.0);
            }
            a = 0.0;
            b = 0.0;
        } else {
            let mut c = c0;
            let rotate = 30.0 * band_value(&self.hue, &weights);
            c *= (1.0 + band_value(&self.saturation, &weights)).max(0.0);
            let dl = band_value(&self.luminance, &weights);
            if dl != 0.0 {
                l *= math::exp2(dl * q / 3.0);
            }
            if self.vibrance != 0.0 {
                let low = 1.0 - (c / 0.3).min(1.0);
                let orange = weights
                    .iter()
                    .filter(|w| w.0 == 1)
                    .map(|w| w.1)
                    .sum::<f64>();
                let protect = if self.vibrance > 0.0 {
                    1.0 - 0.5 * orange
                } else {
                    1.0
                };
                c *= (1.0 + self.vibrance * low * protect).max(0.0);
            }
            c *= (1.0 + self.global_saturation).max(0.0);
            if c0 > 0.0 {
                let scale = c / c0;
                let (sin, cos) = if rotate != 0.0 {
                    math::sin_cos(math::radians(rotate))
                } else {
                    (0.0, 1.0)
                };
                let (na, nb) = (a * cos - b * sin, a * sin + b * cos);
                a = na * scale;
                b = nb * scale;
            }
        }
        if let Some(grading) = &self.grading {
            let x = if l.is_nan() { 0.0 } else { l.clamp(0.0, 1.0) };
            let ws = 1.0 - smoothstep((x - (grading.middle - 0.25)) / grading.width + 0.5);
            let wh = smoothstep((x - (grading.middle + 0.25)) / grading.width + 0.5);
            let wm = (1.0 - ws - wh).max(0.0);
            for (w, wheel) in [ws, wm, wh, 1.0].iter().zip(&grading.wheels) {
                a += w * wheel.da;
                b += w * wheel.db;
                l += w * wheel.dl;
            }
        }
        self.oklab.back([l, a, b])
    }

    fn apply(&self, image: &mut Working) -> Result<()> {
        pixels(image, &|_, p, _| {
            let out = self.pixel(pixel(p));
            store(p, out);
        })
    }
}

// ---------------------------------------------------------------------------
// Stage 10: effects (post-crop vignette, then grain).

fn vignette(image: &mut Working, group: &Value) -> Result<()> {
    let amount = percent(group, "amount");
    if amount == 0.0 {
        return Ok(());
    }
    let midpoint = or(group, "midpoint", 50.0) / 100.0;
    let roundness = percent(group, "roundness");
    let feather = or(group, "feather", 50.0) / 100.0;
    let highlights = percent(group, "highlights");
    let (w, h) = (f64::from(image.width), f64::from(image.height));
    let long = w.max(h);
    let (sx, sy) = if roundness > 0.0 {
        (
            1.0 + roundness * (w / long - 1.0),
            1.0 + roundness * (h / long - 1.0),
        )
    } else {
        (1.0, 1.0)
    };
    let power = if roundness < 0.0 {
        2.0 - 6.0 * roundness
    } else {
        2.0
    };
    let start = 0.2 + 0.8 * midpoint;
    let span = 0.05 + 0.95 * feather;
    let width = image.width as usize;
    let weights = luminance_weights();
    pixels(image, &|i, p, _| {
        let (x, y) = ((i % width) as f64 + 0.5, (i / width) as f64 + 0.5);
        let u = ((x - w / 2.0) / (w / 2.0) * sx).abs();
        let v = ((y - h / 2.0) / (h / 2.0) * sy).abs();
        let d = if power == 2.0 {
            (u * u + v * v).sqrt()
        } else {
            math::pow(math::pow(u, power) + math::pow(v, power), 1.0 / power)
        };
        let s = smoothstep((d - start) / span);
        if s == 0.0 {
            return;
        }
        let rgb = pixel(p);
        let mut k = math::exp2(2.0 * amount * s);
        if amount < 0.0 && highlights > 0.0 {
            let protect = highlights * smoothstep(luminance(&weights, rgb) - 0.5);
            k = 1.0 - (1.0 - k) * (1.0 - protect);
        }
        store(p, rgb.map(|v| v * k));
    })
}

fn splitmix(v: u64) -> u64 {
    let mut z = v.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Approximately standard normal: four 16-bit uniforms of one hash, summed.
fn gauss(seed: u64, x: u64, y: u64) -> f64 {
    let h = splitmix(seed ^ splitmix(x ^ splitmix(y)));
    let sum: f64 = (0..4)
        .map(|k| ((h >> (16 * k)) & 0xFFFF) as f64 / 65536.0)
        .sum();
    (sum - 2.0) * 1.732_050_807_568_877_2
}

fn grain(image: &mut Working, group: &Value, context: &Context) -> Result<()> {
    let amount = percent(group, "amount");
    if amount == 0.0 {
        return Ok(());
    }
    let seed = group.get("seed").and_then(Value::as_u64).unwrap_or(0);
    let size = or(group, "size", 25.0) / 100.0;
    let roughness = or(group, "roughness", 50.0) / 100.0;
    let cell = ((0.0005 + 0.002 * size) * context.long_edge).max(1.0);
    let (ox, oy) = context.origin;
    let width = image.width as usize;
    let weights = luminance_weights();
    let pixel_seed = seed ^ 0xA5A5_A5A5_0000_0000;
    pixels(image, &|i, p, _| {
        let fx = (i % width + ox) as u64;
        let fy = (i / width + oy) as u64;
        let gx = (fx as f64 + 0.5) / cell;
        let gy = (fy as f64 + 0.5) / cell;
        let (ix, iy) = (gx.floor(), gy.floor());
        let (tx, ty) = (smoothstep(gx - ix), smoothstep(gy - iy));
        let (ix, iy) = (ix as u64, iy as u64);
        let n00 = gauss(seed, ix, iy);
        let n10 = gauss(seed, ix + 1, iy);
        let n01 = gauss(seed, ix, iy + 1);
        let n11 = gauss(seed, ix + 1, iy + 1);
        let top = n00 + tx * (n10 - n00);
        let bottom = n01 + tx * (n11 - n01);
        let lattice = top + ty * (bottom - top);
        let noise = (1.0 - roughness) * lattice + roughness * gauss(pixel_seed, fx, fy);
        let rgb = pixel(p);
        let y = luminance(&weights, rgb);
        if !positive(y) {
            return;
        }
        let e = y / (y + GRAY);
        let k = (1.0 + 0.3 * amount * noise * 4.0 * e * (1.0 - e)).max(0.0);
        store(p, rgb.map(|v| v * k));
    })
}

// ---------------------------------------------------------------------------

/// Stages 6–10 on the warped, cropped working image.
pub fn apply(image: &mut Working, develop: &Value, context: &Context) -> Result<()> {
    tone(image, develop, context)?;
    presence(image, develop, context)?;
    if let Some(curves) = Curves::new(develop) {
        curves.apply(image)?;
    }
    if let Some(stage) = ColorStage::new(develop) {
        stage.apply(image)?;
    }
    let effects = &develop["effects"];
    if let Some(group) = effects.get("vignette") {
        vignette(image, group)?;
    }
    if let Some(group) = effects.get("grain") {
        grain(image, group, context)?;
    }
    Ok(())
}

/// Stage 6 alone (used by the dehaze analysis).
pub fn tone(image: &mut Working, develop: &Value, context: &Context) -> Result<()> {
    match Tone::new(develop, context) {
        Some(tone) => tone.apply(image),
        None => Ok(()),
    }
}

/// A box-averaged proxy whose long edge is at most `edge`. Invalid pixels are
/// left out of each average; a proxy pixel with no valid source is invalid.
pub fn proxy(image: &Working, edge: usize) -> Result<Working> {
    let (w, h) = (image.width as usize, image.height as usize);
    let step = w.max(h).div_ceil(edge).max(1);
    let (pw, ph) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = Working::new(pw as u32, ph as u32, image.alpha.is_some())?;
    for py in 0..ph {
        for px in 0..pw {
            let mut sum = [0.0f64; 3];
            let mut count = 0.0;
            for y in py * step..((py + 1) * step).min(h) {
                for x in px * step..((px + 1) * step).min(w) {
                    let i = y * w + x;
                    let a = image.alpha.as_ref().map_or(1.0, |a| a[i]);
                    if a > 0.0 {
                        let p = pixel(&image.rgb[i * 3..i * 3 + 3]);
                        for c in 0..3 {
                            sum[c] += p[c];
                        }
                        count += 1.0;
                    }
                }
            }
            let o = py * pw + px;
            if count > 0.0 {
                store(&mut out.rgb[o * 3..o * 3 + 3], sum.map(|s| s / count));
            } else if let Some(alpha) = &mut out.alpha {
                alpha[o] = 0.0;
            }
        }
    }
    super::check_cancelled()?;
    Ok(out)
}

fn valid(image: &Working) -> impl Iterator<Item = (usize, [f64; 3])> + '_ {
    image
        .rgb
        .chunks_exact(3)
        .enumerate()
        .filter(|(i, _)| image.alpha.as_ref().is_none_or(|a| a[*i] > 0.0))
        .map(|(i, p)| (i, pixel(p)))
}

/// Auto tone, algorithm `auto-tone` version 1, on a stage-5 proxy. Returns
/// the resolved `exposure`, `highlights` and `shadows`.
pub fn auto_tone(proxy: &Working, baseline_exposure: f64) -> Result<[f64; 3]> {
    let weights = luminance_weights();
    let gain = math::exp2(baseline_exposure);
    let mut ys: Vec<f64> = valid(proxy)
        .map(|(_, p)| luminance(&weights, p) * gain)
        .filter(|y| y.is_finite())
        .collect();
    if ys.is_empty() {
        bail!("[invalid-input] auto tone found no valid pixels in the developed frame")
    }
    let mean_stops = ys.iter().map(|y| stops(*y)).sum::<f64>() / ys.len() as f64;
    let exposure = round_to((-mean_stops).clamp(-5.0, 5.0), 0.01);
    ys.sort_by(f64::total_cmp);
    let at = |q: f64| ys[(q * (ys.len() - 1) as f64).floor() as usize];
    let scale = math::exp2(exposure);
    let high = at(0.99) * scale;
    let highlights = if high > 1.0 {
        round_to(-(40.0 * math::log2(high)).min(100.0), 1.0)
    } else {
        0.0
    };
    let low = at(0.01) * scale;
    let shadows = round_to((20.0 * (-stops(low) - 5.0)).clamp(0.0, 100.0), 1.0);
    Ok([exposure, highlights, shadows])
}

/// The dehaze airlight, algorithm version 1, on a stage-6 proxy: the mean
/// color of the brightest 0.1% (at least one) of dark-channel values.
pub fn estimate_airlight(proxy: &Working) -> Result<[f64; 3]> {
    let (w, h) = (proxy.width as usize, proxy.height as usize);
    let mut dark: Vec<f32> = proxy
        .rgb
        .chunks_exact(3)
        .map(|p| {
            pixel(p)
                .iter()
                .map(|v| v.max(0.0))
                .fold(f64::INFINITY, f64::min) as f32
        })
        .collect();
    min_filter(&mut dark, w, h, patch_radius(w.max(h) as f64))?;
    let mut candidates: Vec<(usize, f32)> = valid(proxy).map(|(i, _)| (i, dark[i])).collect();
    if candidates.is_empty() {
        bail!("[invalid-input] dehaze found no valid pixels to estimate the airlight from")
    }
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let count = (candidates.len() / 1000).max(1);
    let mut sum = [0.0f64; 3];
    for (i, _) in &candidates[..count] {
        let p = pixel(&proxy.rgb[i * 3..i * 3 + 3]);
        for c in 0..3 {
            sum[c] += p[c];
        }
    }
    Ok(sum.map(|s| round_to((s / count as f64).max(1.0e-6), 1.0e-6)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn image(values: &[[f32; 3]]) -> Working {
        let mut image = Working::new(values.len() as u32, 1, false).unwrap();
        for (p, v) in image.rgb.chunks_exact_mut(3).zip(values) {
            p.copy_from_slice(v);
        }
        image
    }

    fn context() -> Context {
        Context {
            baseline_exposure: 0.0,
            long_edge: 100.0,
            origin: (0, 0),
        }
    }

    #[test]
    fn identity_settings_leave_pixels_untouched() {
        let values = [[0.1, 0.2, 0.3], [-0.01, 0.5, 2.0], [0.0, 0.0, 0.0]];
        let original = image(&values);
        for develop in [
            json!({"process": 1}),
            json!({"process": 1, "tone": {"exposure": 0, "contrast": 0}, "presence": {"clarity": 0},
                   "curves": {"point": {"rgb": [[0, 0], [1, 1]]}}, "hsl": {"hue": {"red": 0}},
                   "grading": {"blending": 80}, "monochrome": {"enabled": false, "mix": {"red": 20}},
                   "effects": {"vignette": {"amount": 0}, "grain": {"amount": 0, "seed": 1}},
                   "calibration": {"red": {"hue": 0}}}),
        ] {
            let mut out = original.clone();
            apply(&mut out, &develop, &context()).unwrap();
            assert_eq!(out, original, "{develop}");
            assert!(calibration(&develop).is_none());
        }
    }

    #[test]
    fn exposure_doubles_and_tone_is_ratio_preserving() {
        let mut out = image(&[[0.1, 0.2, 0.3]]);
        apply(&mut out, &json!({"tone": {"exposure": 1}}), &context()).unwrap();
        assert_eq!(out.rgb, vec![0.2, 0.4, 0.6]);
        let mut out = image(&[[0.1, 0.2, 0.3]]);
        apply(
            &mut out,
            &json!({"tone": {"shadows": 60, "contrast": 30}}),
            &context(),
        )
        .unwrap();
        let ratio = out.rgb[0] / out.rgb[2];
        assert!((ratio - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn oklab_round_trips_and_keeps_neutrals() {
        let ok = Oklab::new();
        let lab = ok.forward([0.18, 0.18, 0.18]);
        assert!(lab[1].abs() < 1e-6 && lab[2].abs() < 1e-6, "{lab:?}");
        for rgb in [[0.3, 0.1, 0.05], [0.01, 0.4, 0.9], [1.5, 1.2, 0.8]] {
            let back = ok.back(ok.forward(rgb));
            for c in 0..3 {
                assert!((back[c] - rgb[c]).abs() < 1e-9, "{rgb:?} {back:?}");
            }
        }
        assert!((cbrt(27.0) - 3.0).abs() < 1e-15 && (cbrt(-8.0) + 2.0).abs() < 1e-15);
    }

    #[test]
    fn monochrome_is_neutral_and_saturation_minus_100_is_gray() {
        let ok = Oklab::new();
        for develop in [
            json!({"monochrome": {"enabled": true, "mix": {"blue": -40}}}),
            json!({"presence": {"saturation": -100}}),
        ] {
            let mut out = image(&[[0.4, 0.1, 0.05], [0.05, 0.2, 0.6]]);
            apply(&mut out, &develop, &context()).unwrap();
            for p in out.rgb.chunks_exact(3) {
                let lab = ok.forward(pixel(p));
                assert!(
                    lab[1].abs() < 1e-5 && lab[2].abs() < 1e-5,
                    "{develop} {lab:?}"
                );
            }
        }
    }

    #[test]
    fn curves_are_monotone_and_map_endpoints() {
        let develop = json!({"curves": {"parametric": {"shadows": -100, "darks": 100, "lights": -100, "highlights": 100},
                                         "point": {"rgb": [[0, 0.05], [0.5, 0.6], [1, 0.95]]}}});
        let curves = Curves::new(&develop).unwrap();
        for lut in &curves.luts {
            assert!(lut.windows(2).all(|w| w[0] <= w[1]));
            assert!((lut[0] - 0.05).abs() < 1e-12 && (lut[LUT] - 0.95).abs() < 1e-12);
        }
    }

    #[test]
    fn calibration_keeps_white_and_tints_shadows() {
        let cal = calibration(
            &json!({"calibration": {"red": {"hue": 40, "saturation": -30},
                                                       "blue": {"hue": -20, "saturation": 50}}}),
        )
        .unwrap();
        let white = color::apply(&cal.matrix, [1.0, 1.0, 1.0]);
        assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-12), "{white:?}");
        let dark = shadows_tint([0.01, 0.01, 0.01], 1.0);
        assert!(dark[1] < 0.01 && dark[0] == 0.01);
        assert_eq!(shadows_tint([0.5, 0.5, 0.5], 1.0), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn blurs_and_filters_are_bounded() {
        let mut plane: Vec<f32> = (0..64).map(|i| (i % 8) as f32).collect();
        gaussian(&mut plane, 8, 8, 2.0).unwrap();
        assert!(plane.iter().all(|v| (0.0..=7.0).contains(v)));
        let mut plane: Vec<f32> = (0..25).map(|i| i as f32).collect();
        min_filter(&mut plane, 5, 5, 1).unwrap();
        assert_eq!(plane[12], 6.0);
        assert_eq!(plane[0], 0.0);
        // Boxes of 3, 3 and 5 pixels: variance 2·(8/12) + 24/12 = 4 = sigma².
        assert_eq!(box_radii(2.0), [1, 1, 2]);
    }

    #[test]
    fn grain_is_seeded_and_anchored_to_the_frame() {
        let develop =
            json!({"effects": {"grain": {"amount": 60, "size": 0, "roughness": 100, "seed": 7}}});
        let base = image(&[[0.2, 0.2, 0.2]; 4]);
        let mut a = base.clone();
        let mut b = base.clone();
        apply(&mut a, &develop, &context()).unwrap();
        apply(&mut b, &develop, &context()).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, base);
        let mut shifted = base.clone();
        let moved = Context {
            origin: (1, 0),
            ..context()
        };
        apply(&mut shifted, &develop, &moved).unwrap();
        assert_eq!(&shifted.rgb[..9], &a.rgb[3..12]);
    }
}

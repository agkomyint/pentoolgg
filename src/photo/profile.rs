//! Stage 3 of process 1 for raw sources: DNG camera profiles (embedded,
//! matrix-only or an imported `.dcp`) and white balance.
//!
//! The color math follows the DNG specification and the DNG SDK's
//! `dng_color_spec`: color matrices are interpolated by inverse correlated color
//! temperature, a neutral converges to a white chromaticity in at most 30 passes,
//! and a forward matrix (when present) maps white-balanced camera RGB to XYZ D50.
//! Temperature and tint use Robertson's isotemperature lines with the SDK's tint
//! scale. Everything is `f64` built from IEEE `+ - * /` and `sqrt`, so every
//! platform produces identical bits.
use super::color::{self, Matrix, Transfer, Xy, D50};
use super::dng::{Dng, Layout};
use super::raw::{self, Decode, Demosaic, Highlights};
use super::tiff::{Ifd, Tiff};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// Tint units per unit of uv distance, negated (DNG SDK `kTintScale`).
pub const TINT_SCALE: f64 = -3000.0;
/// Pass limit and xy tolerance of `neutral_to_xy`.
pub const MAX_PASSES: usize = 30;
pub const TOLERANCE: f64 = 1e-6;
/// Profile LUT entries at most (hue × saturation × value divisions).
pub const MAX_LUT_ENTRIES: usize = 1_048_576;
/// Suggested white balance is analysed on blocks that fit this long edge.
pub const ANALYSIS_EDGE: usize = 1024;
/// A channel at or above this (with unity multipliers, clipped) counts as clipped.
pub const CLIP_LEVEL: f32 = 0.98;
/// Blocks darker than this carry no usable color.
pub const DARK_LEVEL: f32 = 0.002;

/// Robertson's isotemperature lines: mired, u, v and slope t.
const ROBERTSON: [[f64; 4]; 31] = [
    [0.0, 0.18006, 0.26352, -0.24341],
    [10.0, 0.18066, 0.26589, -0.25479],
    [20.0, 0.18133, 0.26846, -0.26876],
    [30.0, 0.18208, 0.27119, -0.28539],
    [40.0, 0.18293, 0.27407, -0.30470],
    [50.0, 0.18388, 0.27709, -0.32675],
    [60.0, 0.18494, 0.28021, -0.35156],
    [70.0, 0.18611, 0.28342, -0.37915],
    [80.0, 0.18740, 0.28668, -0.40955],
    [90.0, 0.18880, 0.28997, -0.44278],
    [100.0, 0.19032, 0.29326, -0.47888],
    [125.0, 0.19462, 0.30141, -0.58204],
    [150.0, 0.19962, 0.30921, -0.70471],
    [175.0, 0.20525, 0.31647, -0.84901],
    [200.0, 0.21142, 0.32312, -1.0182],
    [225.0, 0.21807, 0.32909, -1.2168],
    [250.0, 0.22511, 0.33439, -1.4512],
    [275.0, 0.23247, 0.33904, -1.7298],
    [300.0, 0.24010, 0.34308, -2.0637],
    [325.0, 0.24792, 0.34655, -2.4681],
    [350.0, 0.25591, 0.34951, -2.9641],
    [375.0, 0.26400, 0.35200, -3.5814],
    [400.0, 0.27218, 0.35407, -4.3633],
    [425.0, 0.28039, 0.35577, -5.3762],
    [450.0, 0.28863, 0.35714, -6.7262],
    [475.0, 0.29685, 0.35823, -8.5955],
    [500.0, 0.30505, 0.35907, -11.324],
    [525.0, 0.31320, 0.35968, -15.628],
    [550.0, 0.32129, 0.36011, -23.325],
    [575.0, 0.32931, 0.36038, -40.770],
    [600.0, 0.33724, 0.36051, -116.45],
];

/// The unit direction of an isotemperature line with slope `t`.
fn direction(t: f64) -> (f64, f64) {
    let length = (1.0 + t * t).sqrt();
    (1.0 / length, t / length)
}

/// xy chromaticity to correlated color temperature (K) and tint (DNG SDK
/// `dng_temperature::Set_xy_coord`). Temperatures beyond the table clamp to its
/// ends (1667 K and infinity, reported as 50000 K).
pub fn xy_to_temperature((x, y): Xy) -> (f64, f64) {
    let denominator = 1.5 - x + 6.0 * y;
    let u = 2.0 * x / denominator;
    let v = 3.0 * y / denominator;
    let mut last_dt = 0.0;
    let (mut last_du, mut last_dv) = (0.0, 0.0);
    for index in 1..=30 {
        let (du, dv) = direction(ROBERTSON[index][3]);
        let uu = u - ROBERTSON[index][1];
        let vv = v - ROBERTSON[index][2];
        let mut dt = -uu * dv + vv * du;
        if dt <= 0.0 || index == 30 {
            if dt > 0.0 {
                dt = 0.0;
            }
            dt = -dt;
            let f = if index == 1 { 0.0 } else { dt / (last_dt + dt) };
            let mired = ROBERTSON[index - 1][0] * f + ROBERTSON[index][0] * (1.0 - f);
            let temperature = if mired > 0.0 { 1.0e6 / mired } else { 50_000.0 };
            let uu = u - (ROBERTSON[index - 1][1] * f + ROBERTSON[index][1] * (1.0 - f));
            let vv = v - (ROBERTSON[index - 1][2] * f + ROBERTSON[index][2] * (1.0 - f));
            let mut du = du * (1.0 - f) + last_du * f;
            let mut dv = dv * (1.0 - f) + last_dv * f;
            let length = (du * du + dv * dv).sqrt();
            du /= length;
            dv /= length;
            return (temperature, (uu * du + vv * dv) * TINT_SCALE);
        }
        last_dt = dt;
        last_du = du;
        last_dv = dv;
    }
    unreachable!("the loop returns at index 30")
}

/// Mired (1e6 / K) and tint to xy (DNG SDK `dng_temperature::Get_xy_coord`).
pub fn mired_to_xy(mired: f64, tint: f64) -> Xy {
    let offset = tint / TINT_SCALE;
    let index = (0..30).find(|&i| mired < ROBERTSON[i + 1][0]).unwrap_or(29);
    let f = (ROBERTSON[index + 1][0] - mired) / (ROBERTSON[index + 1][0] - ROBERTSON[index][0]);
    let u = ROBERTSON[index][1] * f + ROBERTSON[index + 1][1] * (1.0 - f);
    let v = ROBERTSON[index][2] * f + ROBERTSON[index + 1][2] * (1.0 - f);
    let (du0, dv0) = direction(ROBERTSON[index][3]);
    let (du1, dv1) = direction(ROBERTSON[index + 1][3]);
    let mut du = du0 * f + du1 * (1.0 - f);
    let mut dv = dv0 * f + dv1 * (1.0 - f);
    let length = (du * du + dv * dv).sqrt();
    du /= length;
    dv /= length;
    let u = u + du * offset;
    let v = v + dv * offset;
    let denominator = u - 4.0 * v + 2.0;
    (1.5 * u / denominator, v / denominator)
}

/// Temperature (K) and tint to xy.
pub fn temperature_to_xy(temperature: f64, tint: f64) -> Xy {
    mired_to_xy(1.0e6 / temperature, tint)
}

/// The temperature of an EXIF `LightSource` code, or 0 when unknown.
pub fn illuminant_temperature(code: u32) -> f64 {
    match code {
        17 | 3 => 2850.0,
        24 => 3200.0,
        23 => 5000.0,
        20 | 1 | 9 | 4 | 18 => 5500.0,
        21 | 19 | 10 => 6500.0,
        22 | 11 => 7500.0,
        12 => 6400.0,
        13 => 5050.0,
        14 | 2 => 4150.0,
        15 => 3525.0,
        16 => 2925.0,
        _ => 0.0,
    }
}

/// One entry of a profile hue/saturation map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HueSat {
    /// Hue shift in degrees.
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
}

/// `ProfileHueSatMapData`: a hue × saturation × value table of adjustments in
/// linear ProPhoto RGB.
#[derive(Debug, Clone, PartialEq)]
pub struct HueSatMap {
    pub hues: usize,
    pub sats: usize,
    pub vals: usize,
    /// `ProfileHueSatMapEncoding` 1: the value axis is looked up sRGB-encoded.
    pub srgb: bool,
    /// Value-major, then hue, then saturation (DNG order).
    pub data: Vec<HueSat>,
}

impl HueSatMap {
    fn is_identity(&self) -> bool {
        self.data
            .iter()
            .all(|e| e.hue == 0.0 && e.saturation == 1.0 && e.value == 1.0)
    }

    /// Entry-wise `weight · a + (1 − weight) · b`; both maps share dimensions.
    fn blend(a: &Self, b: &Self, weight: f64) -> Self {
        let mix = |x: f32, y: f32| (weight * f64::from(x) + (1.0 - weight) * f64::from(y)) as f32;
        Self {
            data: a
                .data
                .iter()
                .zip(&b.data)
                .map(|(x, y)| HueSat {
                    hue: mix(x.hue, y.hue),
                    saturation: mix(x.saturation, y.saturation),
                    value: mix(x.value, y.value),
                })
                .collect(),
            ..a.clone()
        }
    }

    /// Apply the map to one linear ProPhoto pixel. Pixels with a negative channel
    /// or no value pass through unchanged.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [r, g, b] = rgb.map(f64::from);
        if r < 0.0 || g < 0.0 || b < 0.0 || !(r.is_finite() && g.is_finite() && b.is_finite()) {
            return rgb;
        }
        let v = r.max(g).max(b);
        if v <= 0.0 {
            return rgb;
        }
        let gap = v - r.min(g).min(b);
        let (mut h, mut s) = (0.0, 0.0);
        if gap > 0.0 {
            h = if r == v {
                let h = (g - b) / gap;
                if h < 0.0 {
                    h + 6.0
                } else {
                    h
                }
            } else if g == v {
                2.0 + (b - r) / gap
            } else {
                4.0 + (r - g) / gap
            };
            s = gap / v;
        }
        let coordinate = if self.srgb {
            Transfer::Srgb.encode(v.min(1.0))
        } else {
            v
        };
        let entry = self.lookup(h, s, coordinate);
        h += f64::from(entry.hue) * (6.0 / 360.0);
        s = (s * f64::from(entry.saturation)).min(1.0);
        let v = v * f64::from(entry.value);
        hsv_to_rgb(h, s, v).map(|c| c as f32)
    }

    fn lookup(&self, h: f64, s: f64, v: f64) -> HueSat {
        let h_scale = if self.hues < 2 {
            0.0
        } else {
            self.hues as f64 / 6.0
        };
        let h_scaled = h * h_scale;
        let s_scaled = s * (self.sats - 1) as f64;
        let mut h0 = h_scaled as usize;
        let mut h1 = h0 + 1;
        if h0 >= self.hues - 1 {
            h0 = self.hues - 1;
            h1 = 0;
        }
        let s0 = (s_scaled as usize).min(self.sats - 2);
        let hf = h_scaled - h0 as f64;
        let sf = s_scaled - s0 as f64;
        let hue_step = self.sats;
        let val_step = self.hues * hue_step;
        let plane = |base: usize| -> [f64; 3] {
            let at = |index: usize| {
                let e = self.data[index];
                [
                    f64::from(e.hue),
                    f64::from(e.saturation),
                    f64::from(e.value),
                ]
            };
            let e00 = at(base + h0 * hue_step + s0);
            let e01 = at(base + h1 * hue_step + s0);
            let e10 = at(base + h0 * hue_step + s0 + 1);
            let e11 = at(base + h1 * hue_step + s0 + 1);
            std::array::from_fn(|i| {
                let low = (1.0 - hf) * e00[i] + hf * e01[i];
                let high = (1.0 - hf) * e10[i] + hf * e11[i];
                (1.0 - sf) * low + sf * high
            })
        };
        let out = if self.vals < 2 {
            plane(0)
        } else {
            let v_scaled = v * (self.vals - 1) as f64;
            let v0 = (v_scaled.max(0.0) as usize).min(self.vals - 2);
            let vf = v_scaled - v0 as f64;
            let low = plane(v0 * val_step);
            let high = plane((v0 + 1) * val_step);
            std::array::from_fn(|i| (1.0 - vf) * low[i] + vf * high[i])
        };
        HueSat {
            hue: out[0] as f32,
            saturation: out[1] as f32,
            value: out[2] as f32,
        }
    }
}

/// Most points a `ProfileToneCurve` may hold.
pub const MAX_TONE_POINTS: usize = 4096;

/// `ProfileToneCurve`: a monotone curve on linear ProPhoto through points from
/// (0, 0) to (1, 1), interpolated by a monotone cubic (Fritsch–Carlson) and
/// extended linearly with its end slopes outside [0, 1].
#[derive(Debug, Clone, PartialEq)]
pub struct ToneCurve {
    pub points: Vec<(f64, f64)>,
    /// Fritsch–Carlson tangents, one per point.
    slopes: Vec<f64>,
}

impl ToneCurve {
    /// Validate `values` (x, y pairs) and build the curve. The identity
    /// (only the two end points) counts as absent.
    pub fn new(values: &[f64]) -> Result<Option<Self>> {
        if values.len() % 2 != 0 || values.len() < 4 || values.len() / 2 > MAX_TONE_POINTS {
            bail!("[malformed-resource] ProfileToneCurve (50940) must hold 2–{MAX_TONE_POINTS} x, y pairs")
        }
        let points: Vec<(f64, f64)> = values.chunks_exact(2).map(|p| (p[0], p[1])).collect();
        let first = points[0];
        let last = points[points.len() - 1];
        if first != (0.0, 0.0) || last != (1.0, 1.0) {
            bail!("[malformed-resource] ProfileToneCurve (50940) must start at (0, 0) and end at (1, 1)")
        }
        if points
            .windows(2)
            .any(|w| w[1].0.is_nan() || w[1].0 <= w[0].0 || w[1].1 < w[0].1)
            || points.iter().any(|p| !(0.0..=1.0).contains(&p.1))
        {
            bail!("[malformed-resource] ProfileToneCurve (50940) needs increasing x and non-decreasing y within 0–1")
        }
        if points.iter().all(|(x, y)| x == y) {
            return Ok(None);
        }
        // Secant slopes, then Fritsch–Carlson tangents (harmonic-style limits
        // keep every segment monotone).
        let n = points.len();
        let secant: Vec<f64> = points
            .windows(2)
            .map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0))
            .collect();
        let mut slopes = vec![0.0; n];
        slopes[0] = secant[0];
        slopes[n - 1] = secant[n - 2];
        for i in 1..n - 1 {
            slopes[i] = if secant[i - 1] * secant[i] <= 0.0 {
                0.0
            } else {
                (secant[i - 1] + secant[i]) / 2.0
            };
        }
        for (i, d) in secant.iter().enumerate() {
            if *d == 0.0 {
                slopes[i] = 0.0;
                slopes[i + 1] = 0.0;
                continue;
            }
            let (a, b) = (slopes[i] / d, slopes[i + 1] / d);
            let length = a * a + b * b;
            if length > 9.0 {
                let t = 3.0 / length.sqrt();
                slopes[i] = t * a * d;
                slopes[i + 1] = t * b * d;
            }
        }
        Ok(Some(Self { points, slopes }))
    }

    /// The curve at `x`.
    pub fn eval(&self, x: f64) -> f64 {
        let n = self.points.len();
        if x <= 0.0 {
            return x * self.slopes[0];
        }
        if x >= 1.0 {
            return 1.0 + (x - 1.0) * self.slopes[n - 1];
        }
        let i = self.points.partition_point(|p| p.0 <= x).clamp(1, n - 1) - 1;
        let ((x0, y0), (x1, y1)) = (self.points[i], self.points[i + 1]);
        let h = x1 - x0;
        let t = (x - x0) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * h * self.slopes[i]
            + (-2.0 * t3 + 3.0 * t2) * y1
            + (t3 - t2) * h * self.slopes[i + 1]
    }

    /// Apply hue-preservingly as the DNG SDK's `RefBaselineRGBTone` does: the
    /// curve maps the largest and smallest channel, and the middle channel keeps
    /// its relative position between them.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let v = rgb.map(f64::from);
        let (mut hi, mut lo) = (0, 0);
        for i in 1..3 {
            if v[i] > v[hi] {
                hi = i;
            }
            if v[i] < v[lo] {
                lo = i;
            }
        }
        if v[hi] == v[lo] {
            return [self.eval(v[0]) as f32; 3];
        }
        let mid = 3 - hi - lo;
        let (top, bottom) = (self.eval(v[hi]), self.eval(v[lo]));
        let mut out = [0.0; 3];
        out[hi] = top;
        out[lo] = bottom;
        out[mid] = bottom + (top - bottom) * (v[mid] - v[lo]) / (v[hi] - v[lo]);
        out.map(|c| c as f32)
    }
}

/// The profile's stage-11 output rendering: its look table, then its tone
/// curve, both on linear ProPhoto.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendering {
    pub look: Option<HueSatMap>,
    pub tone_curve: Option<ToneCurve>,
}

impl Rendering {
    pub fn is_identity(&self) -> bool {
        self.look.is_none() && self.tone_curve.is_none()
    }

    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let rgb = match &self.look {
            Some(map) => map.apply(rgb),
            None => rgb,
        };
        match &self.tone_curve {
            Some(curve) => curve.apply(rgb),
            None => rgb,
        }
    }
}

fn hsv_to_rgb(mut h: f64, s: f64, v: f64) -> [f64; 3] {
    if s <= 0.0 {
        return [v; 3];
    }
    if h < 0.0 {
        h += 6.0;
    }
    if h >= 6.0 {
        h -= 6.0;
    }
    let i = (h as usize).min(5);
    let f = h - i as f64;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// One calibration illuminant of a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    /// EXIF `LightSource` code.
    pub illuminant: u32,
    /// Its temperature, or 0 when unknown.
    pub temperature: f64,
    /// XYZ to camera (`ColorMatrixN`), normalized.
    pub color: Matrix,
    /// Camera to XYZ D50 for white-balanced camera RGB (`ForwardMatrixN`), normalized.
    pub forward: Option<Matrix>,
    /// `CameraCalibrationN` (only read from a DNG).
    pub camera: Option<Matrix>,
    pub hue_sat: Option<HueSatMap>,
}

/// A camera profile: its calibrations and identity.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraProfile {
    /// 3 for color; 1 for a monochrome DNG, which has no color transform.
    pub planes: usize,
    pub name: Option<String>,
    pub unique_camera_model: Option<String>,
    pub embed_policy: Option<u32>,
    /// `ProfileCalibrationSignature`.
    pub calibration_signature: Option<String>,
    pub calibrations: Vec<Calibration>,
    /// `ProfileLookTableData`, applied in stage 11.
    pub look: Option<HueSatMap>,
    /// `ProfileToneCurve`, applied in stage 11.
    pub tone_curve: Option<ToneCurve>,
}

fn matrix(values: &[f64], tag: u16) -> Result<Matrix> {
    let m = [
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
        [values[6], values[7], values[8]],
    ];
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if values.iter().any(|v| v.abs() > 1.0e4) || det.abs() < 1.0e-9 {
        bail!("[malformed-resource] the 3x3 matrix in tag {tag} is singular or out of range; re-export the profile")
    }
    Ok(m)
}

fn d50_xyz() -> [f64; 3] {
    color::xyz(D50)
}

/// Scale a color matrix so that D50 maps to a camera value with maximum 1, when
/// it is off by more than 1% (DNG SDK `NormalizeColorMatrix`).
fn normalize_color(m: Matrix) -> Matrix {
    let coord = color::apply(&m, d50_xyz());
    let max = coord[0].max(coord[1]).max(coord[2]);
    if max > 0.0 && !(0.99..=1.01).contains(&max) {
        m.map(|row| row.map(|v| v / max))
    } else {
        m
    }
}

/// Scale the rows of a forward matrix so camera white (1, 1, 1) maps to D50
/// (DNG SDK `NormalizeForwardMatrix`).
fn normalize_forward(m: Matrix, tag: u16) -> Result<Matrix> {
    let white = color::apply(&m, [1.0; 3]);
    if white.iter().any(|v| v.abs() < 1.0e-9) {
        bail!("[malformed-resource] ForwardMatrix (tag {tag}) maps camera white to zero; re-export the profile")
    }
    let d50 = d50_xyz();
    let mut out = m;
    for (i, row) in out.iter_mut().enumerate() {
        let scale = d50[i] / white[i];
        for v in row.iter_mut() {
            *v *= scale;
        }
    }
    Ok(out)
}

fn diagonal(values: [f64; 3]) -> Matrix {
    [
        [values[0], 0.0, 0.0],
        [0.0, values[1], 0.0],
        [0.0, 0.0, values[2]],
    ]
}

fn lerp(a: &Matrix, b: &Matrix, weight: f64) -> Matrix {
    std::array::from_fn(|i| std::array::from_fn(|j| weight * a[i][j] + (1.0 - weight) * b[i][j]))
}

const COLOR_TAGS: [u16; 3] = [50721, 50722, 52531];
const CAMERA_TAGS: [u16; 3] = [50723, 50724, 52530];
const FORWARD_TAGS: [u16; 3] = [50964, 50965, 52532];
const ILLUMINANT_TAGS: [u16; 3] = [50778, 50779, 52529];
const HUE_SAT_TAGS: [u16; 3] = [50938, 50939, 52537];

fn text(tiff: &Tiff, ifd: &Ifd, tag: u16) -> Result<Option<String>> {
    Ok(ifd
        .text(tiff, tag, 4096)?
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(256).collect()))
}

/// `ProfileLookTableDims` (50981), `ProfileLookTableData` (50982) and
/// `ProfileLookTableEncoding` (51108), validated like a hue/sat map.
fn read_look(tiff: &Tiff, ifd: &Ifd) -> Result<Option<HueSatMap>> {
    let dims = match ifd.uints(tiff, 50981, 3)? {
        None => {
            if ifd.has(50982) {
                bail!("[malformed-resource] ProfileLookTableData (50982) needs ProfileLookTableDims (50981)")
            }
            return Ok(None);
        }
        Some(d) if d.len() == 3 && d[0] >= 1 && d[1] >= 2 => {
            let vals = d[2].max(1) as usize;
            let entries = d[0] as usize * d[1] as usize * vals;
            if entries > MAX_LUT_ENTRIES {
                bail!("[limit-exceeded] ProfileLookTableDims {d:?} has {entries} entries; the limit is {MAX_LUT_ENTRIES}")
            }
            (d[0] as usize, d[1] as usize, vals)
        }
        Some(d) => bail!("[malformed-resource] ProfileLookTableDims {d:?} needs at least 1 hue and 2 saturation divisions"),
    };
    let srgb = match ifd.uint(tiff, 51108)? {
        None | Some(0) => false,
        Some(1) => true,
        Some(e) => bail!("[unsupported-capability] ProfileLookTableEncoding {e}; expected 0 (linear) or 1 (sRGB)"),
    };
    let (hues, sats, vals) = dims;
    let count = hues * sats * vals;
    let values = ifd.numbers(tiff, 50982, count * 3)?.unwrap_or_default();
    if values.len() != count * 3 {
        bail!(
            "[malformed-resource] ProfileLookTableData (50982) must hold {} values for its dims",
            count * 3
        )
    }
    let data = values
        .chunks_exact(3)
        .map(|e| HueSat {
            hue: e[0] as f32,
            saturation: e[1] as f32,
            value: e[2] as f32,
        })
        .collect::<Vec<_>>();
    if data
        .iter()
        .any(|e| e.hue.abs() > 360.0 || e.saturation < 0.0 || e.value < 0.0)
    {
        bail!("[malformed-resource] ProfileLookTableData holds a hue shift beyond ±360° or a negative scale")
    }
    let map = HueSatMap {
        hues,
        sats,
        vals,
        srgb,
        data,
    };
    Ok((!map.is_identity()).then_some(map))
}

impl CameraProfile {
    /// The stage-11 output rendering this profile carries.
    pub fn rendering(&self) -> Rendering {
        Rendering {
            look: self.look.clone(),
            tone_curve: self.tone_curve.clone(),
        }
    }

    /// Read the calibrations of a profile IFD. `dng` selects DNG-only tags
    /// (`CameraCalibrationN`).
    fn read(tiff: &Tiff, ifd: &Ifd, planes: usize, dng: bool) -> Result<Self> {
        let embed_policy = ifd.uint(tiff, 50941)?;
        if embed_policy.is_some_and(|p| p > 3) {
            bail!("[malformed-resource] ProfileEmbedPolicy (50941) must be 0–3")
        }
        let mut profile = Self {
            planes,
            name: text(tiff, ifd, 50936)?,
            unique_camera_model: text(tiff, ifd, 50708)?,
            embed_policy,
            calibration_signature: text(tiff, ifd, 50932)?,
            calibrations: Vec::new(),
            look: None,
            tone_curve: None,
        };
        if planes != 3 {
            return Ok(profile);
        }
        profile.look = read_look(tiff, ifd)?;
        profile.tone_curve = match ifd.numbers(tiff, 50940, 2 * MAX_TONE_POINTS)? {
            Some(values) => ToneCurve::new(&values)?,
            None => None,
        };
        let dims = match ifd.uints(tiff, 50937, 3)? {
            None => None,
            Some(d) if d.len() == 3 && d[0] >= 1 && d[1] >= 2 => {
                let vals = d[2].max(1) as usize;
                let entries = d[0] as usize * d[1] as usize * vals;
                if entries > MAX_LUT_ENTRIES {
                    bail!("[limit-exceeded] ProfileHueSatMapDims {d:?} has {entries} entries; the limit is {MAX_LUT_ENTRIES}")
                }
                Some((d[0] as usize, d[1] as usize, vals))
            }
            Some(d) => bail!("[malformed-resource] ProfileHueSatMapDims {d:?} needs at least 1 hue and 2 saturation divisions"),
        };
        let srgb = match ifd.uint(tiff, 51107)? {
            None | Some(0) => false,
            Some(1) => true,
            Some(e) => bail!("[unsupported-capability] ProfileHueSatMapEncoding {e}; expected 0 (linear) or 1 (sRGB)"),
        };
        for n in 0..3 {
            let Some(values) = ifd.numbers(tiff, COLOR_TAGS[n], 9)? else {
                if n == 0 {
                    bail!("[malformed-resource] ColorMatrix1 (50721) is missing; every camera profile must carry it")
                }
                break;
            };
            if values.len() != 9 {
                bail!(
                    "[malformed-resource] ColorMatrix{} ({}) must hold 9 values",
                    n + 1,
                    COLOR_TAGS[n]
                )
            }
            let color = normalize_color(matrix(&values, COLOR_TAGS[n])?);
            let illuminant = ifd.uint(tiff, ILLUMINANT_TAGS[n])?.unwrap_or(0);
            let forward = match ifd.numbers(tiff, FORWARD_TAGS[n], 9)? {
                Some(v) if v.len() == 9 => Some(normalize_forward(
                    matrix(&v, FORWARD_TAGS[n])?,
                    FORWARD_TAGS[n],
                )?),
                Some(_) => bail!(
                    "[malformed-resource] ForwardMatrix{} ({}) must hold 9 values",
                    n + 1,
                    FORWARD_TAGS[n]
                ),
                None => None,
            };
            let camera = match dng
                .then(|| ifd.numbers(tiff, CAMERA_TAGS[n], 9))
                .transpose()?
            {
                Some(Some(v)) if v.len() == 9 => Some(matrix(&v, CAMERA_TAGS[n])?),
                Some(Some(_)) => bail!(
                    "[malformed-resource] CameraCalibration{} ({}) must hold 9 values",
                    n + 1,
                    CAMERA_TAGS[n]
                ),
                _ => None,
            };
            let hue_sat = match (dims, ifd.has(HUE_SAT_TAGS[n])) {
                (Some((hues, sats, vals)), true) => {
                    let count = hues * sats * vals;
                    let values = ifd
                        .numbers(tiff, HUE_SAT_TAGS[n], count * 3)?
                        .unwrap_or_default();
                    if values.len() != count * 3 {
                        bail!(
                            "[malformed-resource] ProfileHueSatMapData{} ({}) must hold {} values for its dims",
                            n + 1,
                            HUE_SAT_TAGS[n],
                            count * 3
                        )
                    }
                    let data = values
                        .chunks_exact(3)
                        .map(|e| HueSat {
                            hue: e[0] as f32,
                            saturation: e[1] as f32,
                            value: e[2] as f32,
                        })
                        .collect::<Vec<_>>();
                    if data.iter().any(|e| {
                        e.hue.abs() > 360.0 || e.saturation < 0.0 || e.value < 0.0
                    }) {
                        bail!("[malformed-resource] ProfileHueSatMapData{} holds a hue shift beyond ±360° or a negative scale", n + 1)
                    }
                    let map = HueSatMap {
                        hues,
                        sats,
                        vals,
                        srgb,
                        data,
                    };
                    (!map.is_identity()).then_some(map)
                }
                (None, true) => bail!("[malformed-resource] ProfileHueSatMapData{} needs ProfileHueSatMapDims (50937)", n + 1),
                _ => None,
            };
            profile.calibrations.push(Calibration {
                illuminant,
                temperature: illuminant_temperature(illuminant),
                color,
                forward,
                camera,
                hue_sat,
            });
        }
        let count = profile.calibrations.len();
        let with_forward = profile
            .calibrations
            .iter()
            .filter(|c| c.forward.is_some())
            .count();
        if with_forward != 0 && with_forward != count {
            bail!("[malformed-resource] a profile with {count} calibrations has {with_forward} forward matrices; it needs all or none")
        }
        let with_map = profile
            .calibrations
            .iter()
            .filter(|c| c.hue_sat.is_some())
            .count();
        if with_map != 0 && with_map != count {
            // An identity map counts as absent; give the others one too.
            let identity = |(hues, sats, vals): (usize, usize, usize)| HueSatMap {
                hues,
                sats,
                vals,
                srgb,
                data: vec![
                    HueSat {
                        hue: 0.0,
                        saturation: 1.0,
                        value: 1.0
                    };
                    hues * sats * vals
                ],
            };
            for c in &mut profile.calibrations {
                if c.hue_sat.is_none() {
                    c.hue_sat = dims.map(identity);
                }
            }
        }
        Ok(profile)
    }

    /// The profile embedded in a DNG (IFD0).
    pub fn embedded(dng: &Dng) -> Result<Self> {
        let planes = match dng.layout {
            Layout::Cfa(_) => 3,
            Layout::LinearRaw(samples) => usize::from(samples),
        };
        Self::read(&dng.tiff, dng.tiff.ifd0(), planes, true)
    }

    /// The embedded profile without forward matrices or hue/sat maps.
    pub fn matrix_only(dng: &Dng) -> Result<Self> {
        let mut profile = Self::embedded(dng)?;
        for c in &mut profile.calibrations {
            c.forward = None;
            c.hue_sat = None;
        }
        profile.look = None;
        profile.tone_curve = None;
        Ok(profile)
    }

    /// A DNG camera profile file (`.dcp`): `IIRC`/`MMCR` and a TIFF-like IFD0.
    pub fn from_dcp(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > super::catalog::MAX_PROFILE_BYTES {
            bail!("[limit-exceeded] camera profile is larger than 16 MiB")
        }
        let tiff = Tiff::parse_profile(bytes)?;
        let profile = Self::read(&tiff, tiff.ifd0(), 3, false)?;
        if profile.unique_camera_model.is_none() {
            bail!("[malformed-resource] the camera profile has no UniqueCameraModel (50708); it cannot be verified against a source")
        }
        Ok(profile)
    }

    /// The facts a `photography.profiles` record holds for this profile.
    pub fn record_facts(&self) -> Value {
        let mut facts = json!({
            "kind": "camera",
            "name": self.name.clone().unwrap_or_else(|| "Camera profile".into()),
            "unique_camera_model": self.unique_camera_model.clone().unwrap_or_default(),
            "imported_from": "dcp",
        });
        if let Some(policy) = self.embed_policy {
            facts["embed_policy"] = json!(policy);
        }
        facts
    }
}

/// A profile bound to one source: analog balance and calibrations combined.
#[derive(Debug, Clone)]
pub struct ColorSpec {
    planes: usize,
    calibrations: Vec<Bound>,
    /// The profile's stage-11 look table and tone curve.
    pub rendering: Rendering,
}

#[derive(Debug, Clone)]
struct Bound {
    temperature: f64,
    /// `AB · CC · CM`.
    color: Matrix,
    /// `AB · CC`.
    camera: Matrix,
    forward: Option<Matrix>,
    hue_sat: Option<HueSatMap>,
}

/// The resolved white of a development.
#[derive(Debug, Clone, PartialEq)]
pub struct White {
    pub xy: Xy,
    /// Normalized camera neutral (maximum 1, each at least 0.001).
    pub neutral: [f64; 3],
    pub temperature: f64,
    pub tint: f64,
}

impl White {
    pub fn report(&self) -> Value {
        let round = |v: f64, digits: i32| {
            let scale = 10f64.powi(digits);
            super::dng::number((v * scale).round() / scale)
        };
        json!({
            "xy": [round(self.xy.0, 6), round(self.xy.1, 6)],
            "temperature": round(self.temperature, 0),
            "tint": round(self.tint, 1),
            "neutral": self.neutral.map(|v| round(v, 6)),
        })
    }
}

/// The stage-3 transform for one white.
#[derive(Debug, Clone)]
pub struct Transform {
    /// Balanced camera RGB (decoded with `neutral`) to linear ProPhoto.
    pub matrix: Matrix,
    pub hue_sat: Option<HueSatMap>,
    /// The calibration shadows tint (`calibration.shadows_tint / 100`), applied
    /// after the matrix; see [`super::adjust::shadows_tint`].
    pub shadows_tint: f64,
}

impl Transform {
    /// Compose the develop's `calibration` between the matrix and the
    /// hue/saturation map.
    pub fn calibrate(&mut self, develop: &serde_json::Value) {
        if let Some(calibration) = super::adjust::calibration(develop) {
            self.matrix = color::multiply(&calibration.matrix, &self.matrix);
            self.shadows_tint = calibration.shadows_tint;
        }
    }

    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut linear = color::apply(&self.matrix, rgb.map(f64::from));
        if self.shadows_tint != 0.0 {
            linear = super::adjust::shadows_tint(linear, self.shadows_tint);
        }
        let out = linear.map(|v| v as f32);
        match &self.hue_sat {
            Some(map) => map.apply(out),
            None => out,
        }
    }
}

impl ColorSpec {
    /// Bind `profile` to `dng`. `CameraCalibrationN` comes from the DNG and is used
    /// for its own profiles, and for an imported profile only when the DNG's
    /// `CameraCalibrationSignature` equals the profile's
    /// `ProfileCalibrationSignature`.
    pub fn new(profile: &CameraProfile, dng: &Dng, imported: bool) -> Result<Self> {
        let planes = match dng.layout {
            Layout::Cfa(_) => 3,
            Layout::LinearRaw(samples) => usize::from(samples),
        };
        if planes != profile.planes {
            bail!(
                "[invalid-develop] the camera profile has {} color planes and the source has {planes}",
                profile.planes
            )
        }
        if planes != 3 {
            return Ok(Self {
                planes,
                calibrations: Vec::new(),
                rendering: Rendering::default(),
            });
        }
        let tiff = &dng.tiff;
        let ifd0 = tiff.ifd0();
        let analog = match ifd0.numbers(tiff, 50727, 3)? {
            Some(v) if v.len() == 3 && v.iter().all(|a| *a > 0.0 && *a < 1.0e4) => {
                diagonal([v[0], v[1], v[2]])
            }
            Some(_) => {
                bail!("[malformed-resource] AnalogBalance (50727) must hold 3 positive values")
            }
            None => color::identity(),
        };
        let own = if imported {
            let signature = text(tiff, ifd0, 50931)?;
            let embedded = CameraProfile::read(tiff, ifd0, 3, true)?;
            (signature.is_some() && signature == profile.calibration_signature)
                .then_some(embedded.calibrations)
        } else {
            None
        };
        let calibrations = profile
            .calibrations
            .iter()
            .enumerate()
            .map(|(n, c)| {
                let camera = match &own {
                    Some(own) => own.get(n).and_then(|o| o.camera),
                    None if imported => None,
                    None => c.camera,
                }
                .unwrap_or_else(color::identity);
                let camera = color::multiply(&analog, &camera);
                Bound {
                    temperature: c.temperature,
                    color: color::multiply(&camera, &c.color),
                    camera,
                    forward: c.forward,
                    hue_sat: c.hue_sat.clone(),
                }
            })
            .collect::<Vec<_>>();
        if let Some(maps) = calibrations
            .iter()
            .map(|c| c.hue_sat.as_ref())
            .collect::<Option<Vec<_>>>()
        {
            if maps
                .windows(2)
                .any(|w| (w[0].hues, w[0].sats, w[0].vals) != (w[1].hues, w[1].sats, w[1].vals))
            {
                bail!("[malformed-resource] the profile's hue/sat maps have different dimensions")
            }
        }
        Ok(Self {
            planes,
            calibrations,
            rendering: profile.rendering(),
        })
    }

    pub fn is_monochrome(&self) -> bool {
        self.planes != 3
    }

    /// The pair of calibrations to blend for `white` and the weight of the first.
    fn weights(&self, white: Xy) -> (usize, usize, f64) {
        let c = &self.calibrations;
        let known = |i: usize| c[i].temperature > 0.0;
        if c.len() < 2 || !(0..c.len()).all(known) {
            return (0, 0, 1.0);
        }
        let mut order = (0..c.len()).collect::<Vec<_>>();
        order.sort_by(|a, b| c[*a].temperature.total_cmp(&c[*b].temperature));
        if order
            .windows(2)
            .any(|w| c[w[0]].temperature == c[w[1]].temperature)
        {
            return (0, 0, 1.0);
        }
        let (temperature, _) = xy_to_temperature(white);
        // The bracketing pair: (low, high) with low < high in kelvin.
        let pair = order
            .windows(2)
            .find(|w| temperature <= c[w[1]].temperature)
            .unwrap_or(&order[order.len() - 2..]);
        let (low, high) = (pair[0], pair[1]);
        let (t1, t2) = (c[low].temperature, c[high].temperature);
        let weight = if temperature <= t1 {
            1.0
        } else if temperature >= t2 {
            0.0
        } else {
            (1.0 / temperature - 1.0 / t2) / (1.0 / t1 - 1.0 / t2)
        };
        (low, high, weight)
    }

    /// `FindXYZtoCamera`: `(AB·CC·CM, FM, AB·CC, hue/sat map)` for `white`.
    fn find(&self, white: Xy) -> (Matrix, Option<Matrix>, Matrix, Option<HueSatMap>) {
        let (a, b, g) = self.weights(white);
        let (a, b) = (&self.calibrations[a], &self.calibrations[b]);
        let forward = match (a.forward, b.forward) {
            (Some(x), Some(y)) => Some(lerp(&x, &y, g)),
            _ => None,
        };
        let hue_sat = match (&a.hue_sat, &b.hue_sat) {
            (Some(x), Some(_)) if g >= 1.0 => Some(x.clone()),
            (Some(_), Some(y)) if g <= 0.0 => Some(y.clone()),
            (Some(x), Some(y)) => Some(HueSatMap::blend(x, y, g)),
            _ => None,
        };
        (
            lerp(&a.color, &b.color, g),
            forward,
            lerp(&a.camera, &b.camera, g),
            hue_sat,
        )
    }

    /// `NeutralToXY`: the white whose camera neutral is `neutral`.
    pub fn neutral_to_xy(&self, neutral: [f64; 3]) -> Result<Xy> {
        if neutral.iter().any(|n| !n.is_finite() || *n <= 0.0) {
            bail!("[invalid-develop] white-balance neutral {neutral:?} must hold three positive values")
        }
        if self.is_monochrome() {
            return Ok(D50);
        }
        let mut last = D50;
        for pass in 0..MAX_PASSES {
            let (color, ..) = self.find(last);
            let xyz = color::apply(&color::invert(&color), neutral);
            let sum = xyz[0] + xyz[1] + xyz[2];
            if !(sum.is_finite() && sum > 0.0 && xyz[1] > 0.0) {
                bail!("[invalid-develop] white-balance neutral {neutral:?} is not a plausible camera white for this profile")
            }
            let mut next = (xyz[0] / sum, xyz[1] / sum);
            if (next.0 - last.0).abs() < TOLERANCE && (next.1 - last.1).abs() < TOLERANCE {
                return Ok(next);
            }
            if pass == MAX_PASSES - 1 {
                next = ((last.0 + next.0) * 0.5, (last.1 + next.1) * 0.5);
            }
            last = next;
        }
        Ok(last)
    }

    /// `SetWhiteXY`: the camera neutral and the stage-3 transform for `white`.
    pub fn white(&self, xy: Xy) -> Result<(White, Transform)> {
        if !(xy.0 > 0.0 && xy.1 > 0.0 && xy.0 + xy.1 < 1.0) {
            bail!("[invalid-develop] white point {xy:?} is not a valid chromaticity")
        }
        let (temperature, tint) = xy_to_temperature(xy);
        if self.is_monochrome() {
            let white = White {
                xy,
                neutral: [1.0; 3],
                temperature,
                tint,
            };
            let transform = Transform {
                matrix: color::identity(),
                hue_sat: None,
                shadows_tint: 0.0,
            };
            return Ok((white, transform));
        }
        let (color_matrix, forward, camera, hue_sat) = self.find(xy);
        let mut neutral = color::apply(&color_matrix, color::xyz(xy));
        let max = neutral[0].max(neutral[1]).max(neutral[2]);
        if !(max.is_finite() && max > 0.0) {
            bail!("[invalid-develop] white point {xy:?} has no positive camera neutral for this profile")
        }
        neutral = neutral.map(|v| (v / max).clamp(0.001, 1.0));
        let camera_to_pcs = match forward {
            Some(forward) => {
                let inverse = color::invert(&camera);
                let reference = color::apply(&inverse, neutral);
                let scale = diagonal(reference.map(|r| 1.0 / r));
                color::multiply(&forward, &color::multiply(&scale, &inverse))
            }
            None => {
                let pcs_to_camera = color::multiply(&color_matrix, &color::bradford(D50, xy));
                let d50 = color::apply(&pcs_to_camera, d50_xyz());
                let scale = d50[0].max(d50[1]).max(d50[2]);
                color::invert(&pcs_to_camera.map(|row| row.map(|v| v / scale)))
            }
        };
        let to_working = color::invert(&color::ColorSpace::working().to_xyz());
        let matrix = color::multiply(
            &to_working,
            &color::multiply(&camera_to_pcs, &diagonal(neutral)),
        );
        if matrix.iter().flatten().any(|v| !v.is_finite()) {
            bail!("[invalid-develop] white point {xy:?} produces a singular camera transform")
        }
        let white = White {
            xy,
            neutral,
            temperature,
            tint,
        };
        Ok((
            white,
            Transform {
                matrix,
                hue_sat,
                shadows_tint: 0.0,
            },
        ))
    }

    /// The as-shot white: `AsShotNeutral`, else `AsShotWhiteXY`, else D50.
    pub fn as_shot(&self, dng: &Dng) -> Result<Xy> {
        if let Some(neutral) = dng.as_shot_neutral.as_deref() {
            if let [r, g, b] = neutral {
                return self.neutral_to_xy([*r, *g, *b]);
            }
            return Ok(D50);
        }
        match dng.tiff.ifd0().numbers(&dng.tiff, 50729, 2)? {
            Some(xy) if xy.len() == 2 && xy[0] > 0.0 && xy[1] > 0.0 && xy[0] + xy[1] < 1.0 => {
                Ok((xy[0], xy[1]))
            }
            Some(xy) => bail!(
                "[malformed-resource] AsShotWhiteXY (50729) {xy:?} is not a valid chromaticity"
            ),
            None => Ok(D50),
        }
    }
}

/// The camera profile a `raw.camera_profile` setting selects. `profiles` resolves
/// a `{profile}` digest to `.dcp` bytes.
pub fn select(
    setting: Option<&Value>,
    dng: &Dng,
    profiles: &dyn Fn(&str) -> Result<Vec<u8>>,
) -> Result<ColorSpec> {
    match setting {
        None => ColorSpec::new(&CameraProfile::embedded(dng)?, dng, false),
        Some(Value::String(s)) if s == "embedded" => {
            ColorSpec::new(&CameraProfile::embedded(dng)?, dng, false)
        }
        Some(Value::String(s)) if s == "matrix-only" => {
            ColorSpec::new(&CameraProfile::matrix_only(dng)?, dng, false)
        }
        Some(Value::Object(o)) if o.get("profile").and_then(Value::as_str).is_some() => {
            let digest = o["profile"].as_str().unwrap();
            let bytes = profiles(digest)?;
            let profile = CameraProfile::from_dcp(&bytes)
                .with_context(|| format!("camera profile {digest}"))?;
            ColorSpec::new(&profile, dng, true)
        }
        Some(other) => bail!("[invalid-develop] raw.camera_profile {other} must be embedded, matrix-only or {{\"profile\": digest}}"),
    }
}

/// The stage-1 decode options of a develop object's `raw` group.
pub fn decode_options(develop: &Value, neutral: [f64; 3]) -> Decode {
    let raw = &develop["raw"];
    Decode {
        demosaic: match raw["demosaic"].as_str() {
            Some("bilinear") => Demosaic::Bilinear,
            _ => Demosaic::Mhc,
        },
        highlights: match raw["highlights"].as_str() {
            Some("clip") => Highlights::Clip,
            _ => Highlights::Blend,
        },
        neutral,
    }
}

/// Resolve a develop object's `white_balance` for a raw source.
pub fn resolve(
    spec: &ColorSpec,
    dng: &Dng,
    white_balance: Option<&Value>,
) -> Result<(White, Transform)> {
    let xy = match white_balance {
        None => spec.as_shot(dng)?,
        Some(wb) => match wb["mode"].as_str() {
            Some("as-shot") => spec.as_shot(dng)?,
            Some("temperature") => {
                let temperature = wb["temperature"].as_f64().unwrap_or(5000.0);
                let tint = wb["tint"].as_f64().unwrap_or(0.0);
                temperature_to_xy(temperature, tint)
            }
            Some("neutral") => {
                let n = wb["neutral"]
                    .as_array()
                    .filter(|n| n.len() == 3)
                    .context("[invalid-develop] white_balance.neutral must hold three values")?;
                let n: Vec<f64> = n.iter().filter_map(Value::as_f64).collect();
                if n.len() != 3 {
                    bail!("[invalid-develop] white_balance.neutral must hold three numbers")
                }
                if spec.is_monochrome() {
                    D50
                } else {
                    spec.neutral_to_xy([n[0], n[1], n[2]])?
                }
            }
            mode => bail!(
                "[invalid-develop] white_balance mode {mode:?} does not apply to a raw source"
            ),
        },
    };
    spec.white(xy)
}

/// The relative white balance of a rendered source, as a working-space matrix:
/// Bradford from the white at `200 − temperature` mired and `tint` to the white
/// at 200 mired (5000 K) and tint 0. Zero is exactly the identity.
pub fn relative(temperature: f64, tint: f64) -> Matrix {
    if temperature == 0.0 && tint == 0.0 {
        return color::identity();
    }
    let working = color::ColorSpace::working().to_xyz();
    let adapt = color::bradford(
        mired_to_xy(200.0 - temperature, tint),
        mired_to_xy(200.0, 0.0),
    );
    color::multiply(&color::invert(&working), &color::multiply(&adapt, &working))
}

/// Map a point normalized to the oriented frame into the unoriented pixel grid.
fn unorient(orientation: u8, x: f64, y: f64) -> (f64, f64) {
    match orientation {
        2 => (1.0 - x, y),
        3 => (1.0 - x, 1.0 - y),
        4 => (x, 1.0 - y),
        5 => (y, x),
        6 => (y, 1.0 - x),
        7 => (1.0 - y, 1.0 - x),
        8 => (1.0 - y, x),
        _ => (x, y),
    }
}

fn unbalanced(dng: &Dng, develop: &Value) -> Result<raw::CameraRgb> {
    let mut options = decode_options(develop, [1.0; 3]);
    options.highlights = Highlights::Clip;
    raw::decode(dng, &options)
}

fn normalized(sum: [f64; 3], what: &str) -> Result<[f64; 3]> {
    let max = sum[0].max(sum[1]).max(sum[2]);
    // Also refuses NaN.
    if !sum.iter().all(|v| *v > 0.0) {
        bail!("[invalid-input] {what} has no positive value in every channel; choose a lit, neutral area")
    }
    Ok(sum.map(|v| v / max))
}

/// The mean camera neutral inside a disk: centre `(x, y)` normalized to the
/// oriented frame, `radius` a fraction of the long edge. Clipped pixels are
/// excluded. The result is normalized to a maximum of 1.
pub fn sample(dng: &Dng, develop: &Value, x: f64, y: f64, radius: f64) -> Result<[f64; 3]> {
    if ![x, y, radius].iter().all(|v| (0.0..=1.0).contains(v)) {
        bail!("[invalid-input] --sample x,y,radius must each lie in 0–1")
    }
    let image = unbalanced(dng, develop)?;
    let (w, h) = (image.width, image.height);
    let (u, v) = unorient(dng.orientation, x, y);
    let (cx, cy) = (u * w as f64, v * h as f64);
    let r = (radius * w.max(h) as f64).max(0.5);
    let x0 = ((cx - r).floor().max(0.0)) as usize;
    let x1 = ((cx + r).ceil() as usize).min(w);
    let y0 = ((cy - r).floor().max(0.0)) as usize;
    let y1 = ((cy + r).ceil() as usize).min(h);
    let mut sum = [0.0f64; 3];
    let mut count = 0usize;
    for py in y0..y1 {
        for px in x0..x1 {
            let dx = px as f64 + 0.5 - cx;
            let dy = py as f64 + 0.5 - cy;
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let at = (py * w + px) * 3;
            let p = &image.rgb[at..at + 3];
            if p.iter().any(|c| *c >= CLIP_LEVEL) {
                continue;
            }
            for c in 0..3 {
                sum[c] += f64::from(p[c]);
            }
            count += 1;
        }
    }
    if count == 0 {
        bail!("[invalid-input] every pixel in the sampled area is clipped or outside the image; sample an unclipped neutral area")
    }
    normalized(sum, "the sampled area")
}

/// Gray-world suggestion: the mean of unclipped analysis blocks, as a neutral.
pub fn suggest_neutral(dng: &Dng, develop: &Value) -> Result<[f64; 3]> {
    let image = unbalanced(dng, develop)?;
    let (w, h) = (image.width, image.height);
    let step = w.max(h).div_ceil(ANALYSIS_EDGE);
    let mut sum = [0.0f64; 3];
    let mut blocks = 0usize;
    for by in (0..h).step_by(step) {
        for bx in (0..w).step_by(step) {
            let mut block = [0.0f64; 3];
            let mut clipped = false;
            let mut n = 0usize;
            for py in by..(by + step).min(h) {
                for px in bx..(bx + step).min(w) {
                    let at = (py * w + px) * 3;
                    let p = &image.rgb[at..at + 3];
                    clipped |= p.iter().any(|c| *c >= CLIP_LEVEL);
                    for c in 0..3 {
                        block[c] += f64::from(p[c]);
                    }
                    n += 1;
                }
            }
            let mean = block.map(|v| v / n as f64);
            if clipped || mean[0].max(mean[1]).max(mean[2]) < f64::from(DARK_LEVEL) {
                continue;
            }
            for c in 0..3 {
                sum[c] += mean[c];
            }
            blocks += 1;
        }
    }
    if blocks == 0 {
        bail!("[invalid-input] the image has no unclipped, lit area to estimate white balance from; use --temperature or --sample")
    }
    normalized(sum, "the gray-world estimate")
}

/// The stored `white_balance` of a suggestion: rounded, clamped temperature and
/// tint with provenance.
pub fn suggested(spec: &ColorSpec, neutral: [f64; 3]) -> Result<Value> {
    let xy = spec.neutral_to_xy(neutral)?;
    let (temperature, tint) = xy_to_temperature(xy);
    Ok(json!({
        "mode": "temperature",
        "temperature": temperature.clamp(2000.0, 50_000.0).round() as i64,
        "tint": tint.clamp(-150.0, 150.0).round() as i64,
        "auto": {"algorithm": "gray-world", "version": 1},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_curve_is_monotone_through_its_points_and_hue_preserving() {
        let curve = ToneCurve::new(&[0.0, 0.0, 0.1, 0.05, 0.25, 0.3, 0.5, 0.7, 1.0, 1.0])
            .unwrap()
            .unwrap();
        for (x, y) in &curve.points {
            assert!((curve.eval(*x) - y).abs() < 1e-12);
        }
        let mut previous = curve.eval(-0.1);
        for step in 0..=1200 {
            let y = curve.eval(-0.1 + f64::from(step) * 0.001);
            assert!(y >= previous - 1e-15, "monotone at {step}");
            previous = y;
        }
        // Linear extension above 1 with the last secant slope.
        let slope = (1.0 - 0.7) / 0.5;
        assert!((curve.eval(1.5) - (1.0 + 0.5 * slope)).abs() < 1e-12);
        // The middle channel keeps its relative position.
        let out = curve.apply([0.6, 0.2, 0.4]);
        let t = (out[2] - out[1]) / (out[0] - out[1]);
        assert!((t - 0.5).abs() < 1e-6, "{out:?}");
        assert_eq!(
            curve.apply([0.3; 3]),
            [curve.eval(f64::from(0.3f32)) as f32; 3]
        );
        assert_eq!(
            ToneCurve::new(&[0.0, 0.0, 0.5, 0.5, 1.0, 1.0]).unwrap(),
            None
        );
        for bad in [
            vec![0.0, 0.0, 1.0],
            vec![0.0, 0.1, 1.0, 1.0],
            vec![0.0, 0.0, 0.5, 0.6, 0.4, 0.7, 1.0, 1.0],
            vec![0.0, 0.0, 0.5, 0.6, 0.6, 0.5, 1.0, 1.0],
            vec![0.0, 0.0, 0.5, f64::NAN, 1.0, 1.0],
        ] {
            assert!(ToneCurve::new(&bad).is_err(), "{bad:?}");
        }
        let rendering = Rendering {
            look: None,
            tone_curve: Some(curve.clone()),
        };
        assert!(!rendering.is_identity() && Rendering::default().is_identity());
        assert_eq!(
            rendering.apply([0.6, 0.2, 0.4]),
            curve.apply([0.6, 0.2, 0.4])
        );
    }

    #[test]
    fn robertson_matches_reference_whites() {
        let (t65, tint65) = xy_to_temperature(color::D65);
        assert!((t65 - 6504.0).abs() < 2.0, "{t65}");
        assert!(tint65.abs() < 12.0, "{tint65}");
        let (t50, _) = xy_to_temperature(D50);
        assert!((t50 - 5001.0).abs() < 1.0, "{t50}");
    }

    #[test]
    fn temperature_round_trips() {
        for temperature in [2000.0, 2850.0, 4000.0, 5000.0, 6500.0, 10_000.0, 30_000.0] {
            for tint in [-150.0, -20.0, 0.0, 35.0, 150.0] {
                let xy = temperature_to_xy(temperature, tint);
                let (t, n) = xy_to_temperature(xy);
                assert!(
                    (t - temperature).abs() / temperature < 1e-3,
                    "{temperature} {tint}: {t}"
                );
                assert!((n - tint).abs() < 0.05, "{temperature} {tint}: {n}");
            }
        }
    }

    #[test]
    fn relative_zero_is_identity() {
        assert_eq!(relative(0.0, 0.0), color::identity());
        let warm = relative(50.0, 0.0);
        let out = color::apply(&warm, [1.0; 3]);
        assert!(out[0] > out[2], "{out:?}");
    }

    #[test]
    fn identity_hue_sat_map_is_neutral() {
        let map = HueSatMap {
            hues: 6,
            sats: 2,
            vals: 1,
            srgb: false,
            data: vec![
                HueSat {
                    hue: 0.0,
                    saturation: 1.0,
                    value: 1.0
                };
                12
            ],
        };
        for rgb in [[0.2f32, 0.5, 0.9], [1.5, 0.1, 0.3], [0.4, 0.4, 0.4]] {
            let out = map.apply(rgb);
            for c in 0..3 {
                assert!((out[c] - rgb[c]).abs() < 1e-6, "{rgb:?} {out:?}");
            }
        }
        let mut shifted = map.clone();
        for e in &mut shifted.data {
            e.hue = 60.0;
        }
        // Red shifted by 60° becomes yellow.
        let out = shifted.apply([1.0, 0.0, 0.0]);
        assert!(
            (out[0] - 1.0).abs() < 1e-6 && (out[1] - 1.0).abs() < 1e-6 && out[2].abs() < 1e-6,
            "{out:?}"
        );
    }

    fn bound(color: Matrix, forward: Option<Matrix>, temperature: f64) -> Bound {
        Bound {
            temperature,
            color,
            camera: color::identity(),
            forward,
            hue_sat: None,
        }
    }

    fn sample_matrix() -> Matrix {
        // A plausible camera: XYZ to camera with a strong green channel.
        normalize_color([[0.9, -0.3, -0.1], [-0.4, 1.3, 0.1], [-0.05, 0.15, 0.6]])
    }

    #[test]
    fn neutral_maps_to_working_white() {
        let cm = sample_matrix();
        let forward = normalize_forward(color::invert(&cm), 0).unwrap();
        for fm in [None, Some(forward)] {
            let spec = ColorSpec {
                planes: 3,
                calibrations: vec![bound(cm, fm, 0.0)],
                rendering: Rendering::default(),
            };
            for xy in [D50, color::D65, temperature_to_xy(3200.0, 10.0)] {
                let (white, transform) = spec.white(xy).unwrap();
                let out = color::apply(&transform.matrix, [1.0; 3]);
                for v in out {
                    assert!((v - 1.0).abs() < 1e-9, "{xy:?} {fm:?}: {out:?}");
                }
                let back = spec.neutral_to_xy(white.neutral).unwrap();
                assert!((back.0 - xy.0).abs() < 1e-5 && (back.1 - xy.1).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn calibrations_interpolate_by_inverse_temperature() {
        let a = sample_matrix();
        let b = a.map(|row| row.map(|v| v * 0.5));
        let spec = ColorSpec {
            planes: 3,
            calibrations: vec![bound(a, None, 6500.0), bound(b, None, 2850.0)],
            rendering: Rendering::default(),
        };
        let (low, high, g) = spec.weights(temperature_to_xy(2850.0, 0.0));
        assert_eq!((low, high), (1, 0));
        assert!((g - 1.0).abs() < 1e-3, "{g}");
        let (_, _, g) = spec.weights(temperature_to_xy(10_000.0, 0.0));
        assert_eq!(g, 0.0);
        let middle = 1.0 / ((1.0 / 2850.0 + 1.0 / 6500.0) / 2.0);
        let (_, _, g) = spec.weights(temperature_to_xy(middle, 0.0));
        assert!((g - 0.5).abs() < 2e-3, "{g}");
    }

    #[test]
    fn unorient_maps_corners() {
        // Displayed top-left comes from stored bottom-left for orientation 6.
        assert_eq!(unorient(6, 0.0, 0.0), (0.0, 1.0));
        assert_eq!(unorient(8, 0.0, 0.0), (1.0, 0.0));
        assert_eq!(unorient(1, 0.25, 0.75), (0.25, 0.75));
    }
}

//! HDR and panorama merges (algorithm version 1), specified in "HDR and
//! panorama merges" in `docs/photography-v1.md`.
//!
//! Inputs are developed through stages 1–3 only, oriented upright and
//! optionally scaled, so they are scene-linear working RGB. Each input is
//! decoded once to measure it and once more to merge it, so at most one
//! full-resolution input is held at a time beside the measurements. The result
//! is working RGB that `dngout` writes as a derived DNG.
use super::check_cancelled;
use super::color::ColorSpace;
use super::dng::{number, Dng};
use super::dngout;
use super::math;
use super::pipeline::{self, Profiles};
use super::warp;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// The merge algorithm version recorded in `derived.algorithm`.
pub const ALGORITHM: u64 = 1;
pub const MAX_HDR_INPUTS: usize = 9;
pub const MAX_PANO_INPUTS: usize = 64;
/// Total input pixels a merge may read.
pub const MAX_INPUT_PIXELS: u64 = 1_200_000_000;
/// The specified output limit; `max_output_pixels` can be lower.
pub const MAX_OUTPUT_PIXELS: u64 = 120_000_000;
/// The largest side a photo asset may have.
pub const MAX_SIDE: u64 = 32_768;

/// The largest output a merge writes: 120 megapixels, and no more than a
/// derived DNG can develop within `raw::MAX_DEVELOP_BYTES` (52 bytes per pixel
/// for three samples, plus one strip).
pub fn max_output_pixels() -> u64 {
    let developable = (super::raw::MAX_DEVELOP_BYTES - (4 << 20)) / 52;
    MAX_OUTPUT_PIXELS.min(developable)
}

/// Refuse an output size, suggesting the `--scale` that would fit.
pub fn check_output(width: u64, height: u64, scale: f64) -> Result<()> {
    let pixels = width * height;
    let limit = max_output_pixels();
    if pixels > limit || width > MAX_SIDE || height > MAX_SIDE {
        let fit =
            ((limit as f64 / pixels as f64).sqrt()).min(MAX_SIDE as f64 / width.max(height) as f64);
        let suggested = (scale * fit * 100.0).floor() / 100.0;
        bail!("[limit-exceeded] the merge output would be {width}x{height} ({pixels} pixels); the limit is {limit} pixels and {MAX_SIDE} per side; pass --scale {suggested} or less")
    }
    Ok(())
}

/// One merge input: its source bytes (read on demand) and its stage 1–3
/// develop settings.
pub struct Input<'a> {
    pub load: Box<dyn Fn() -> Result<Vec<u8>> + 'a>,
    pub develop: Value,
}

/// A merge result in the working space, oriented upright.
pub struct Merged {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<f32>,
    pub alpha: Option<Vec<f32>>,
    /// The requested settings recorded in `derived.settings`.
    pub settings: Value,
    /// The resolved alignment recorded in `derived.alignment`.
    pub alignment: Value,
}

/// A decoded, oriented and scaled input.
struct Frame {
    width: usize,
    height: usize,
    rgb: Vec<f32>,
    level: Vec<f32>,
    capture: Value,
    baseline: f64,
}

fn frame(inputs: &[Input], index: usize, profiles: Profiles, scale: f64) -> Result<Frame> {
    let read = || -> Result<Frame> {
        let bytes = (inputs[index].load)()?;
        let dng = Dng::inspect(&bytes)?;
        if dngout::transparency(&dng)?.is_some() {
            bail!("[unsupported-capability] the source has transparent pixels (a panorama); merge its own inputs instead")
        }
        let (rgb, level, _) = pipeline::decode_levels(&dng, &inputs[index].develop, profiles)?;
        let (w, h) = pipeline::decoded_size(&dng);
        let (rgb, ow, oh) = orient(rgb, 3, w, h, dng.orientation);
        let (level, _, _) = orient(level, 1, w, h, dng.orientation);
        let (nw, nh) = scaled(ow, oh, scale);
        let rgb = resize(rgb, 3, ow, oh, nw, nh, false);
        let level = resize(level, 1, ow, oh, nw, nh, true);
        check_cancelled()?;
        Ok(Frame {
            width: nw,
            height: nh,
            rgb,
            level,
            capture: dng.capture(),
            baseline: dng.baseline_exposure.unwrap_or(0.0),
        })
    };
    read().with_context(|| format!("merge input {}", index + 1))
}

/// The size of a `width` x `height` frame at `scale`.
pub fn scaled(width: usize, height: usize, scale: f64) -> (usize, usize) {
    let side = |n: usize| ((n as f64 * scale).round() as usize).max(1);
    (side(width), side(height))
}

/// Rotate an interleaved plane upright by its EXIF orientation.
fn orient(
    data: Vec<f32>,
    channels: usize,
    width: usize,
    height: usize,
    orientation: u8,
) -> (Vec<f32>, usize, usize) {
    if orientation <= 1 {
        return (data, width, height);
    }
    let (ow, oh) = warp::oriented(orientation, width, height);
    let mut out = Vec::with_capacity(data.len());
    for y in 0..oh {
        for x in 0..ow {
            let (u, v) = warp::unorient(
                orientation,
                (x as f64 + 0.5) / ow as f64,
                (y as f64 + 0.5) / oh as f64,
            );
            let sx = ((u * width as f64).floor() as usize).min(width - 1);
            let sy = ((v * height as f64).floor() as usize).min(height - 1);
            let at = (sy * width + sx) * channels;
            out.extend_from_slice(&data[at..at + channels]);
        }
    }
    (out, ow, oh)
}

/// Box-resample a plane to a smaller size: the mean of each output pixel's
/// source area, or its maximum when `peak` (for clip maps).
fn resize(
    data: Vec<f32>,
    channels: usize,
    width: usize,
    height: usize,
    nw: usize,
    nh: usize,
    peak: bool,
) -> Vec<f32> {
    if (nw, nh) == (width, height) {
        return data;
    }
    let span = |o: usize, n: usize, m: usize| (o * m / n, ((o + 1) * m).div_ceil(n).min(m));
    let mut out = Vec::with_capacity(nw * nh * channels);
    for oy in 0..nh {
        let (y0, y1) = span(oy, nh, height);
        for ox in 0..nw {
            let (x0, x1) = span(ox, nw, width);
            for c in 0..channels {
                let mut acc = 0.0f32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let v = data[(y * width + x) * channels + c];
                        acc = if peak { acc.max(v) } else { acc + v };
                    }
                }
                if !peak {
                    acc /= ((x1 - x0) * (y1 - y0)) as f32;
                }
                out.push(acc);
            }
        }
    }
    out
}

fn luminance(rgb: &[f32]) -> Vec<f32> {
    let m = ColorSpace::working().to_xyz();
    let k = [m[1][0] as f32, m[1][1] as f32, m[1][2] as f32];
    rgb.chunks_exact(3)
        .map(|p| k[0] * p[0] + k[1] * p[1] + k[2] * p[2])
        .collect()
}

fn median(mut values: Vec<f32>) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mid = values.len() / 2;
    values.select_nth_unstable_by(mid, f32::total_cmp);
    Some(values[mid])
}

/// The splitmix64 generator that seeds panorama RANSAC.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// HDR

/// HDR deghosting strength: the log2 difference from the reference above
/// which an input pixel is rejected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Deghost {
    Off,
    Low,
    Medium,
    High,
}

impl Deghost {
    pub fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "off" => Self::Off,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            other => bail!("[invalid-input] --deghost {other:?} must be off, low, medium or high"),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    fn stops(self) -> Option<f64> {
        match self {
            Self::Off => None,
            Self::Low => Some(1.0),
            Self::Medium => Some(0.6),
            Self::High => Some(0.35),
        }
    }
}

pub struct HdrOptions {
    /// Zero-based reference input; `None` picks the middle exposure.
    pub reference: Option<usize>,
    pub deghost: Deghost,
    pub scale: f64,
}

/// A pre-balance level at or above which a pixel (or a 3x3 neighbor) is clipped.
const CLIP: f32 = 0.97;
/// A level below which a value is noise.
const FLOOR: f32 = 0.002;
/// Levels used to measure exposure ratios.
const MEASURE: (f32, f32) = (0.01, 0.9);
/// Samples read to measure a ratio or a median.
const SAMPLES: usize = 1_000_000;

/// The measurements of one HDR input, kept between the two passes.
struct Exposed {
    luma: Vec<f32>,
    level: Vec<u16>,
    clipped: Vec<bool>,
    exif: Option<f64>,
    baseline: f64,
}

impl Exposed {
    fn level(&self, p: usize) -> f32 {
        f32::from(self.level[p]) / 65_535.0
    }

    /// Unclipped, above the noise floor, with positive luminance.
    fn usable(&self, p: usize) -> bool {
        !self.clipped[p] && self.level(p) >= FLOOR && self.luma[p] > 0.0
    }
}

/// The sensor exposure `t · ISO / N²` a capture records, if it records a time.
fn exif_exposure(capture: &Value) -> Option<f64> {
    let time = &capture["exposure_time"];
    let (n, d) = (time[0].as_f64()?, time[1].as_f64()?);
    if n <= 0.0 || d <= 0.0 {
        return None;
    }
    let iso = capture["iso"].as_f64().unwrap_or(100.0);
    let aperture = capture["aperture"].as_f64().unwrap_or(1.0);
    Some(n / d * iso / (aperture * aperture))
}

/// The exposure factor of an input relative to the reference: EXIF when it
/// agrees with the measured ratio within half a stop, else the measurement.
fn choose_exposure(exif: Option<f64>, measured: Option<f64>) -> Option<(f64, &'static str)> {
    match (exif, measured) {
        (Some(x), Some(m)) if math::log2(x / m).abs() <= 0.5 => Some((x, "exif")),
        (_, Some(m)) => Some((m, "measured")),
        (Some(x), None) => Some((x, "exif-unverified")),
        (None, None) => None,
    }
}

/// `p + shift` inside a `width` x `height` frame.
fn shifted(x: usize, y: usize, shift: (i64, i64), width: usize, height: usize) -> Option<usize> {
    let sx = x as i64 + shift.0;
    let sy = y as i64 + shift.1;
    (sx >= 0 && sy >= 0 && (sx as usize) < width && (sy as usize) < height)
        .then(|| sy as usize * width + sx as usize)
}

/// A 3x3 dilation of a mask.
fn dilate(mask: &[bool], width: usize, height: usize) -> Vec<bool> {
    let mut out = vec![false; mask.len()];
    for y in 0..height {
        for x in 0..width {
            if mask[y * width + x] {
                for ny in y.saturating_sub(1)..(y + 2).min(height) {
                    for nx in x.saturating_sub(1)..(x + 2).min(width) {
                        out[ny * width + nx] = true;
                    }
                }
            }
        }
    }
    out
}

/// Median-threshold-bitmap pyramids: per level, the threshold bits, the
/// exclusion bits (values away from the median) and the size.
type Bitmaps = Vec<(Vec<bool>, Vec<bool>, usize, usize)>;

fn bitmaps(luma: &[f32], width: usize, height: usize) -> Bitmaps {
    let mut levels = vec![(luma.to_vec(), width, height)];
    while levels.len() < 6 {
        let (data, w, h) = levels.last().unwrap();
        let (nw, nh) = (w / 2, h / 2);
        if nw < 16 || nh < 16 {
            break;
        }
        let mut next = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            for x in 0..nw {
                let at = |dx: usize, dy: usize| data[(2 * y + dy) * w + 2 * x + dx];
                next.push((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) * 0.25);
            }
        }
        levels.push((next, nw, nh));
    }
    levels
        .into_iter()
        .map(|(data, w, h)| {
            let step = (data.len() / SAMPLES).max(1);
            let med = median(data.iter().step_by(step).copied().collect()).unwrap_or(0.0);
            let band = 0.04 * med.abs();
            let threshold = data.iter().map(|v| *v > med).collect();
            let exclusion = data.iter().map(|v| (v - med).abs() > band).collect();
            (threshold, exclusion, w, h)
        })
        .collect()
}

/// The fraction of disagreeing bits over the overlap at `shift`.
fn bitmap_error(
    a: &(Vec<bool>, Vec<bool>, usize, usize),
    b: &Bitmaps,
    level: usize,
    shift: (i64, i64),
) -> f64 {
    let (ta, ea, w, h) = (&a.0, &a.1, a.2, a.3);
    let (tb, eb) = (&b[level].0, &b[level].1);
    let (mut diff, mut overlap) = (0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            if let Some(q) = shifted(x, y, shift, w, h) {
                let p = y * w + x;
                overlap += 1;
                if ea[p] && eb[q] && ta[p] != tb[q] {
                    diff += 1;
                }
            }
        }
    }
    if overlap == 0 {
        f64::MAX
    } else {
        diff as f64 / overlap as f64
    }
}

/// The integer translation aligning `image` to `reference` (`image(p + s)`
/// matches `reference(p)`), searched coarse to fine over up to six levels,
/// so at most ±63 pixels.
fn mtb(reference: &Bitmaps, image: &Bitmaps) -> (i64, i64) {
    let mut shift = (0i64, 0i64);
    for level in (0..reference.len()).rev() {
        shift = (shift.0 * 2, shift.1 * 2);
        let mut best = (f64::MAX, shift);
        for (dx, dy) in [
            (0, 0),
            (-1, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (1, 1),
        ] {
            let candidate = (shift.0 + dx, shift.1 + dy);
            let error = bitmap_error(&reference[level], image, level, candidate);
            if error < best.0 {
                best = (error, candidate);
            }
        }
        shift = best.1;
    }
    shift
}

/// The median of `image(p + shift) / reference(p)` over pixels well exposed in both.
fn measured_ratio(
    reference: &Exposed,
    image: &Exposed,
    shift: (i64, i64),
    width: usize,
    height: usize,
) -> Option<f64> {
    let total = width * height;
    let step = (total / SAMPLES).max(1);
    let good = |e: &Exposed, p: usize| {
        let l = e.level(p);
        !e.clipped[p] && l >= MEASURE.0 && l <= MEASURE.1 && e.luma[p] > 0.0
    };
    let mut ratios = Vec::new();
    for p in (0..total).step_by(step) {
        if let Some(q) = shifted(p % width, p / width, shift, width, height) {
            if good(reference, p) && good(image, q) {
                ratios.push(image.luma[q] / reference.luma[p]);
            }
        }
    }
    if ratios.len() < 64 {
        return None;
    }
    median(ratios).map(f64::from)
}

fn hat(level: f32) -> f32 {
    let t = 2.0 * level - 1.0;
    let t4 = t * t * t * t;
    1.0 - t4 * t4 * t4
}

/// Merge exposure brackets into one scene-linear image.
pub fn hdr(inputs: &[Input], profiles: Profiles, options: &HdrOptions) -> Result<Merged> {
    let n = inputs.len();
    if !(2..=MAX_HDR_INPUTS).contains(&n) {
        bail!("[invalid-input] merge-hdr takes 2 to {MAX_HDR_INPUTS} inputs, not {n}")
    }
    if options.reference.is_some_and(|r| r >= n) {
        bail!("[invalid-input] --reference must name an input from 1 to {n}")
    }
    // Pass 1: measure every input.
    let mut exposed: Vec<Exposed> = Vec::with_capacity(n);
    let (mut width, mut height) = (0, 0);
    for index in 0..n {
        let f = frame(inputs, index, profiles, options.scale)?;
        if index == 0 {
            (width, height) = (f.width, f.height);
        } else if (f.width, f.height) != (width, height) {
            bail!("[invalid-input] HDR input {} develops to {}x{} but input 1 to {width}x{height}; brackets must share one frame size", index + 1, f.width, f.height)
        }
        let level: Vec<f32> = f.level;
        let mut clipped = vec![false; width * height];
        for y in 0..height {
            for x in 0..width {
                let mut peak = 0.0f32;
                for ny in y.saturating_sub(1)..(y + 2).min(height) {
                    for nx in x.saturating_sub(1)..(x + 2).min(width) {
                        peak = peak.max(level[ny * width + nx]);
                    }
                }
                clipped[y * width + x] = peak >= CLIP;
            }
        }
        exposed.push(Exposed {
            luma: luminance(&f.rgb),
            level: level
                .iter()
                .map(|l| (l.clamp(0.0, 1.0) * 65_535.0).round() as u16)
                .collect(),
            clipped,
            exif: exif_exposure(&f.capture),
            baseline: f.baseline,
        });
    }
    let all_exif = exposed.iter().all(|e| e.exif.is_some());
    let reference = match options.reference {
        Some(r) => r,
        None => {
            let step = (width * height / SAMPLES).max(1);
            let key: Vec<f64> = exposed
                .iter()
                .map(|e| match e.exif {
                    Some(x) if all_exif => x,
                    _ => f64::from(
                        median(e.luma.iter().step_by(step).copied().collect()).unwrap_or(0.0),
                    ),
                })
                .collect();
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by(|a, b| key[*a].total_cmp(&key[*b]).then(a.cmp(b)));
            order[(n - 1) / 2]
        }
    };
    // Alignment and exposure normalization against the reference.
    let reference_bits = bitmaps(&exposed[reference].luma, width, height);
    let mut shifts = vec![(0i64, 0i64); n];
    let mut factors = vec![1.0f64; n];
    let mut sources = vec!["reference"; n];
    for index in 0..n {
        if index == reference {
            continue;
        }
        shifts[index] = mtb(
            &reference_bits,
            &bitmaps(&exposed[index].luma, width, height),
        );
        check_cancelled()?;
        let exif = match (exposed[index].exif, exposed[reference].exif) {
            (Some(a), Some(b)) => Some(a / b),
            _ => None,
        };
        let measured = measured_ratio(
            &exposed[reference],
            &exposed[index],
            shifts[index],
            width,
            height,
        );
        let Some((factor, source)) = choose_exposure(exif, measured) else {
            bail!("[invalid-input] HDR input {} shares too few well-exposed pixels with reference input {} and records no exposure time; choose another --reference", index + 1, reference + 1)
        };
        factors[index] = factor;
        sources[index] = source;
    }
    // Deghosting masks, in reference coordinates.
    let mut ghosts: Vec<Option<Vec<bool>>> = vec![None; n];
    if let Some(stops) = options.deghost.stops() {
        let (low, high) = (math::exp2(-stops) as f32, math::exp2(stops) as f32);
        for index in 0..n {
            if index == reference {
                continue;
            }
            let (r, e) = (&exposed[reference], &exposed[index]);
            let inverse = (1.0 / factors[index]) as f32;
            let mut mask = vec![false; width * height];
            for (p, m) in mask.iter_mut().enumerate() {
                if let Some(q) = shifted(p % width, p / width, shifts[index], width, height) {
                    if r.usable(p) && e.usable(q) {
                        let ratio = e.luma[q] * inverse / r.luma[p];
                        *m = ratio < low || ratio > high;
                    }
                }
            }
            ghosts[index] = Some(dilate(&mask, width, height));
        }
        check_cancelled()?;
    }
    let weight = |index: usize, p: usize| -> f32 {
        let Some(q) = shifted(p % width, p / width, shifts[index], width, height) else {
            return 0.0;
        };
        let e = &exposed[index];
        if e.clipped[q] || e.level(q) < FLOOR {
            return 0.0;
        }
        if ghosts[index].as_ref().is_some_and(|g| g[p]) {
            return 0.0;
        }
        hat(e.level(q))
    };
    // Weight sums, and where no input is usable, the darkest (for highlights)
    // or brightest (for shadows) input that covers the pixel.
    let mut by_factor: Vec<usize> = (0..n).collect();
    by_factor.sort_by(|a, b| factors[*a].total_cmp(&factors[*b]).then(a.cmp(b)));
    let mut sum = vec![0.0f32; width * height];
    let mut fallback = vec![u8::MAX; width * height];
    for p in 0..width * height {
        let s: f32 = (0..n).map(|i| weight(i, p)).sum();
        sum[p] = s;
        if s == 0.0 {
            let bright = exposed[reference].level(p) >= 0.5;
            let covers =
                |i: &&usize| shifted(p % width, p / width, shifts[**i], width, height).is_some();
            let pick = if bright {
                by_factor.iter().find(covers)
            } else {
                by_factor.iter().rev().find(covers)
            };
            fallback[p] = *pick.unwrap_or(&reference) as u8;
        }
    }
    check_cancelled()?;
    // Pass 2: accumulate radiance in the reference's scale.
    let ghost_pixels: Vec<u64> = ghosts
        .iter()
        .map(|g| {
            g.as_ref()
                .map_or(0, |g| g.iter().filter(|v| **v).count() as u64)
        })
        .collect();
    let mut rgb = vec![0.0f32; width * height * 3];
    for index in 0..n {
        let f = frame(inputs, index, profiles, options.scale)?;
        if (f.width, f.height) != (width, height) {
            bail!(
                "[invalid-input] HDR input {} changed size between passes",
                index + 1
            )
        }
        let inverse = (1.0 / factors[index]) as f32;
        for p in 0..width * height {
            let w = if sum[p] > 0.0 {
                weight(index, p)
            } else if usize::from(fallback[p]) == index {
                1.0
            } else {
                0.0
            };
            if w > 0.0 {
                let q = shifted(p % width, p / width, shifts[index], width, height).unwrap();
                for c in 0..3 {
                    rgb[p * 3 + c] += w * f.rgb[q * 3 + c] * inverse;
                }
            }
        }
        check_cancelled()?;
    }
    let gain = math::exp2(exposed[reference].baseline) as f32;
    for (p, pixel) in rgb.chunks_exact_mut(3).enumerate() {
        let s = if sum[p] > 0.0 { sum[p] } else { 1.0 };
        for v in pixel {
            *v = (*v / s * gain).max(0.0);
        }
    }
    Ok(Merged {
        width,
        height,
        rgb,
        alpha: None,
        settings: json!({
            "deghost": options.deghost.name(),
            "reference": options.reference.map_or(json!("auto"), |r| json!(r + 1)),
            "scale": number(options.scale),
        }),
        alignment: json!({
            "reference": reference + 1,
            "inputs": (0..n).map(|i| json!({
                "shift": [shifts[i].0, shifts[i].1],
                "exposure": number(factors[i]),
                "exposure_source": sources[i],
                "ghost_pixels": ghost_pixels[i],
            })).collect::<Vec<_>>(),
        }),
    })
}

// ---------------------------------------------------------------------------
// Panorama

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    Spherical,
    Cylindrical,
    Perspective,
}

impl Projection {
    pub fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "spherical" => Self::Spherical,
            "cylindrical" => Self::Cylindrical,
            "perspective" => Self::Perspective,
            other => bail!("[invalid-input] --projection {other:?} must be spherical, cylindrical or perspective"),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Spherical => "spherical",
            Self::Cylindrical => "cylindrical",
            Self::Perspective => "perspective",
        }
    }
}

pub struct PanoOptions {
    pub projection: Projection,
    pub seed: u64,
    /// Focal length in output pixels; `None` uses EXIF, assuming a 36 mm long edge.
    pub focal: Option<f64>,
    pub scale: f64,
}

/// The long edge of the feature proxies.
const PROXY_EDGE: usize = 800;
const MAX_FEATURES: usize = 500;
const PATCH: usize = 4;
const RANSAC_ITERATIONS: usize = 1000;
/// Multiband levels; boxes and the canvas align to `1 << (BANDS - 1)`.
const BANDS: usize = 5;
const ALIGN: usize = 1 << (BANDS - 1);
/// Context kept around each input so band edges stay outside its seam.
const MARGIN: usize = 64;

struct Feature {
    x: f64,
    y: f64,
    descriptor: Vec<f32>,
}

/// Harris corners of a log-luminance proxy with normalized 9x9 patches.
fn features(image: &[f32], width: usize, height: usize) -> Vec<Feature> {
    let border = PATCH + 2;
    if width <= 2 * border || height <= 2 * border {
        return Vec::new();
    }
    let mut tensor = vec![[0.0f32; 3]; width * height];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let at = |x: usize, y: usize| image[y * width + x];
            let gx = (at(x + 1, y) - at(x - 1, y)) * 0.5;
            let gy = (at(x, y + 1) - at(x, y - 1)) * 0.5;
            tensor[y * width + x] = [gx * gx, gy * gy, gx * gy];
        }
    }
    let mut response = vec![0.0f32; width * height];
    for y in border..height - border {
        for x in border..width - border {
            let mut s = [0.0f32; 3];
            for ny in y - 2..=y + 2 {
                for nx in x - 2..=x + 2 {
                    let t = tensor[ny * width + nx];
                    s[0] += t[0];
                    s[1] += t[1];
                    s[2] += t[2];
                }
            }
            let det = s[0] * s[1] - s[2] * s[2];
            let trace = s[0] + s[1];
            response[y * width + x] = det - 0.04 * trace * trace;
        }
    }
    let peak = response.iter().fold(0.0f32, |m, v| m.max(*v));
    if peak <= 0.0 {
        return Vec::new();
    }
    let mut candidates: Vec<usize> = (0..width * height)
        .filter(|p| response[*p] > peak * 1.0e-3)
        .collect();
    candidates.sort_by(|a, b| response[*b].total_cmp(&response[*a]).then(a.cmp(b)));
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut out = Vec::new();
    for p in candidates {
        let (x, y) = (p % width, p / width);
        if taken
            .iter()
            .any(|(tx, ty)| tx.abs_diff(x) <= 4 && ty.abs_diff(y) <= 4)
        {
            continue;
        }
        let mut patch = Vec::with_capacity((2 * PATCH + 1) * (2 * PATCH + 1));
        for ny in y - PATCH..=y + PATCH {
            for nx in x - PATCH..=x + PATCH {
                patch.push(image[ny * width + nx]);
            }
        }
        let mean = patch.iter().sum::<f32>() / patch.len() as f32;
        patch.iter_mut().for_each(|v| *v -= mean);
        let norm = patch.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm < 1.0e-3 {
            continue;
        }
        patch.iter_mut().for_each(|v| *v /= norm);
        taken.push((x, y));
        out.push(Feature {
            x: x as f64 + 0.5,
            y: y as f64 + 0.5,
            descriptor: patch,
        });
        if out.len() == MAX_FEATURES {
            break;
        }
    }
    out
}

/// Mutual best NCC matches that pass the ratio test.
fn match_features(a: &[Feature], b: &[Feature]) -> Vec<(usize, usize)> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let score = |i: usize, j: usize| -> f32 {
        a[i].descriptor
            .iter()
            .zip(&b[j].descriptor)
            .map(|(x, y)| x * y)
            .sum()
    };
    let scores: Vec<Vec<f32>> = (0..a.len())
        .map(|i| (0..b.len()).map(|j| score(i, j)).collect())
        .collect();
    let best_in_a: Vec<usize> = (0..b.len())
        .map(|j| {
            (0..a.len())
                .max_by(|x, y| scores[*x][j].total_cmp(&scores[*y][j]).then(y.cmp(x)))
                .unwrap()
        })
        .collect();
    let mut out = Vec::new();
    for (i, row) in scores.iter().enumerate() {
        let (mut best, mut second, mut at) = (f32::MIN, f32::MIN, 0);
        for (j, s) in row.iter().enumerate() {
            if *s > best {
                second = best;
                best = *s;
                at = j;
            } else if *s > second {
                second = *s;
            }
        }
        let second = second.max(-1.0);
        if best >= 0.8 && (1.0 - best) <= 0.6 * (1.0 - second) && best_in_a[at] == i {
            out.push((i, at));
        }
    }
    out
}

/// Projected coordinates of a continuous source point, centered on the image
/// and in pixels at focal length `f`.
fn project(projection: Projection, f: f64, w: f64, h: f64, x: f64, y: f64) -> (f64, f64) {
    let (dx, dy) = (x - w / 2.0, y - h / 2.0);
    match projection {
        Projection::Cylindrical => {
            let r = (dx * dx + f * f).sqrt();
            (f * math::atan2(dx, f), f * dy / r)
        }
        Projection::Spherical => {
            let r = (dx * dx + f * f).sqrt();
            (f * math::atan2(dx, f), f * math::atan2(dy, r))
        }
        Projection::Perspective => (dx, dy),
    }
}

/// The continuous source point of projected coordinates, if it faces the camera.
fn unproject(projection: Projection, f: f64, w: f64, h: f64, u: f64, v: f64) -> Option<(f64, f64)> {
    let (dx, dy) = match projection {
        Projection::Cylindrical => {
            let (s, c) = math::sin_cos(u / f);
            if c <= 1.0e-6 {
                return None;
            }
            (f * s / c, v / c)
        }
        Projection::Spherical => {
            let (s, c) = math::sin_cos(u / f);
            let (sp, cp) = math::sin_cos(v / f);
            let z = c * cp;
            if z <= 1.0e-6 {
                return None;
            }
            (f * s * cp / z, f * sp / z)
        }
        Projection::Perspective => (u, v),
    };
    Some((dx + w / 2.0, dy + h / 2.0))
}

type Matrix3 = [[f64; 3]; 3];

fn mul(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn inverse(m: &Matrix3) -> Option<Matrix3> {
    let cof =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adj = [
        [cof(1, 2, 1, 2), -cof(0, 2, 1, 2), cof(0, 1, 1, 2)],
        [-cof(1, 2, 0, 2), cof(0, 2, 0, 2), -cof(0, 1, 0, 2)],
        [cof(1, 2, 0, 1), -cof(0, 2, 0, 1), cof(0, 1, 0, 1)],
    ];
    let det = m[0][0] * adj[0][0] + m[0][1] * adj[1][0] + m[0][2] * adj[2][0];
    (det.abs() > 1.0e-15).then(|| adj.map(|row| row.map(|v| v / det)))
}

fn apply(m: &Matrix3, x: f64, y: f64) -> Option<(f64, f64)> {
    let w = m[2][0] * x + m[2][1] * y + m[2][2];
    (w > 1.0e-9).then(|| {
        (
            (m[0][0] * x + m[0][1] * y + m[0][2]) / w,
            (m[1][0] * x + m[1][1] * y + m[1][2]) / w,
        )
    })
}

/// Solve an 8x8 system (augmented rows) by Gaussian elimination with partial pivoting.
fn solve8(mut rows: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|a, b| rows[*a][col].abs().total_cmp(&rows[*b][col].abs()))?;
        if rows[pivot][col].abs() < 1.0e-12 {
            return None;
        }
        rows.swap(col, pivot);
        for r in 0..8 {
            if r != col {
                let k = rows[r][col] / rows[col][col];
                for c in col..9 {
                    rows[r][c] -= k * rows[col][c];
                }
            }
        }
    }
    Some(std::array::from_fn(|i| rows[i][8] / rows[i][i]))
}

/// The least-squares homography mapping `b` points to `a` points, in
/// coordinates divided by `s`.
fn homography(pairs: &[([f64; 2], [f64; 2])], s: f64) -> Option<Matrix3> {
    let mut normal = [[0.0f64; 9]; 8];
    for (a, b) in pairs {
        let (ax, ay, bx, by) = (a[0] / s, a[1] / s, b[0] / s, b[1] / s);
        for (row, target) in [
            ([bx, by, 1.0, 0.0, 0.0, 0.0, -bx * ax, -by * ax], ax),
            ([0.0, 0.0, 0.0, bx, by, 1.0, -bx * ay, -by * ay], ay),
        ] {
            for i in 0..8 {
                for j in 0..8 {
                    normal[i][j] += row[i] * row[j];
                }
                normal[i][8] += row[i] * target;
            }
        }
    }
    let h = solve8(normal)?;
    Some([
        [h[0], h[1], h[2] * s],
        [h[3], h[4], h[5] * s],
        [h[6] / s, h[7] / s, 1.0],
    ])
}

/// One input's placement in the global frame: a translation of its projected
/// coordinates, or a homography of its centered coordinates.
#[derive(Clone)]
enum Placement {
    Shift([f64; 2]),
    Homography(Matrix3),
}

/// RANSAC over adjacent-pair matches (`a` in the left input, `b` in the
/// right): the placement of `b` relative to `a`, and the inlier count.
fn fit(
    projection: Projection,
    pairs: &[([f64; 2], [f64; 2])],
    threshold: f64,
    scale: f64,
    rng: &mut SplitMix,
) -> Option<(Placement, usize)> {
    let n = pairs.len();
    if n == 0 {
        return None;
    }
    let distance = |p: &Placement, (a, b): &([f64; 2], [f64; 2])| -> f64 {
        match p {
            Placement::Shift(d) => (a[0] - b[0] - d[0]).hypot(a[1] - b[1] - d[1]),
            Placement::Homography(m) => {
                apply(m, b[0], b[1]).map_or(f64::MAX, |(x, y)| (x - a[0]).hypot(y - a[1]))
            }
        }
    };
    let inliers = |p: &Placement| -> Vec<usize> {
        (0..n)
            .filter(|i| distance(p, &pairs[*i]) < threshold)
            .collect()
    };
    let mut best: Option<(Placement, Vec<usize>)> = None;
    for _ in 0..RANSAC_ITERATIONS {
        let candidate = if projection == Projection::Perspective {
            if n < 4 {
                return None;
            }
            let mut picks = Vec::with_capacity(4);
            while picks.len() < 4 {
                let i = rng.below(n);
                if !picks.contains(&i) {
                    picks.push(i);
                }
            }
            let sample: Vec<_> = picks.iter().map(|i| pairs[*i]).collect();
            match homography(&sample, scale) {
                Some(m) => Placement::Homography(m),
                None => continue,
            }
        } else {
            let (a, b) = pairs[rng.below(n)];
            Placement::Shift([a[0] - b[0], a[1] - b[1]])
        };
        let found = inliers(&candidate);
        if best.as_ref().is_none_or(|(_, b)| found.len() > b.len()) {
            best = Some((candidate, found));
        }
    }
    let (_, found) = best?;
    let selected: Vec<_> = found.iter().map(|i| pairs[*i]).collect();
    let refined = match projection {
        Projection::Perspective => Placement::Homography(homography(&selected, scale)?),
        _ => {
            let k = selected.len() as f64;
            let sx = selected.iter().map(|(a, b)| a[0] - b[0]).sum::<f64>() / k;
            let sy = selected.iter().map(|(a, b)| a[1] - b[1]).sum::<f64>() / k;
            Placement::Shift([sx, sy])
        }
    };
    let count = inliers(&refined).len();
    Some((refined, count))
}

/// A Gaussian (1 4 6 4 1)/16 reduction to `ceil(n / 2)`, normalized at the edges.
fn reduce(data: &[f32], w: usize, h: usize, ch: usize) -> (Vec<f32>, usize, usize) {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let mut tmp = vec![0.0f32; nw * h * ch];
    for y in 0..h {
        for x in 0..nw {
            for c in 0..ch {
                let (mut s, mut k) = (0.0, 0.0);
                for (t, weight) in K.iter().enumerate() {
                    let sx = (2 * x + t) as isize - 2;
                    if sx >= 0 && (sx as usize) < w {
                        s += weight * data[(y * w + sx as usize) * ch + c];
                        k += weight;
                    }
                }
                tmp[(y * nw + x) * ch + c] = s / k;
            }
        }
    }
    let mut out = vec![0.0f32; nw * nh * ch];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..ch {
                let (mut s, mut k) = (0.0, 0.0);
                for (t, weight) in K.iter().enumerate() {
                    let sy = (2 * y + t) as isize - 2;
                    if sy >= 0 && (sy as usize) < h {
                        s += weight * tmp[(sy as usize * nw + x) * ch + c];
                        k += weight;
                    }
                }
                out[(y * nw + x) * ch + c] = s / k;
            }
        }
    }
    (out, nw, nh)
}

/// The matching expansion of a `w` x `h` level to `nw` x `nh`.
fn expand(data: &[f32], w: usize, h: usize, ch: usize, nw: usize, nh: usize) -> Vec<f32> {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    // Coarse samples j contributing to fine x satisfy |x - 2j| <= 2.
    let taps = |x: usize, n: usize| {
        let lo = (x.saturating_sub(2)).div_ceil(2);
        let hi = ((x + 2) / 2).min(n - 1);
        (lo..=hi).map(move |j| (j, K[(x + 2 - 2 * j).min(4)]))
    };
    let mut tmp = vec![0.0f32; nw * h * ch];
    for y in 0..h {
        for x in 0..nw {
            for c in 0..ch {
                let (mut s, mut k) = (0.0, 0.0);
                for (j, weight) in taps(x, w) {
                    s += weight * data[(y * w + j) * ch + c];
                    k += weight;
                }
                tmp[(y * nw + x) * ch + c] = if k > 0.0 { s / k } else { 0.0 };
            }
        }
    }
    let mut out = vec![0.0f32; nw * nh * ch];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..ch {
                let (mut s, mut k) = (0.0, 0.0);
                for (j, weight) in taps(y, h) {
                    s += weight * tmp[(j * nw + x) * ch + c];
                    k += weight;
                }
                out[(y * nw + x) * ch + c] = if k > 0.0 { s / k } else { 0.0 };
            }
        }
    }
    out
}

/// Fill invalid pixels by push-pull so band filters do not see black.
fn fill(image: &mut [f32], valid: &[f32], w: usize, h: usize) {
    let premultiplied: Vec<f32> = image
        .chunks_exact(3)
        .zip(valid)
        .flat_map(|(p, a)| [p[0] * a, p[1] * a, p[2] * a])
        .collect();
    let mut levels = vec![(premultiplied, valid.to_vec(), w, h)];
    while levels.len() < 16 {
        let (p, a, lw, lh) = levels.last().unwrap();
        if *lw == 1 && *lh == 1 {
            break;
        }
        let (np, nw, nh) = reduce(p, *lw, *lh, 3);
        let (na, _, _) = reduce(a, *lw, *lh, 1);
        levels.push((np, na, nw, nh));
    }
    let (p, a, mut cw, mut chh) = levels.pop().unwrap();
    let mut color: Vec<f32> = p
        .chunks_exact(3)
        .zip(&a)
        .flat_map(|(p, a)| {
            if *a > 1.0e-12 {
                [p[0] / a, p[1] / a, p[2] / a]
            } else {
                [0.0; 3]
            }
        })
        .collect();
    while let Some((p, a, lw, lh)) = levels.pop() {
        let up = expand(&color, cw, chh, 3, lw, lh);
        color = p
            .chunks_exact(3)
            .zip(&a)
            .zip(up.chunks_exact(3))
            .flat_map(|((p, a), e)| {
                let rest = (1.0 - a).max(0.0);
                [p[0] + rest * e[0], p[1] + rest * e[1], p[2] + rest * e[2]]
            })
            .collect();
        (cw, chh) = (lw, lh);
    }
    image.copy_from_slice(&color);
}

/// Bilinear sample of an interleaved RGB frame at a continuous point.
fn bilinear(rgb: &[f32], w: usize, h: usize, x: f64, y: f64) -> [f32; 3] {
    let (sx, sy) = (
        (x - 0.5).clamp(0.0, (w - 1) as f64),
        (y - 0.5).clamp(0.0, (h - 1) as f64),
    );
    let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = ((sx - x0 as f64) as f32, (sy - y0 as f64) as f32);
    std::array::from_fn(|c| {
        let at = |x: usize, y: usize| rgb[(y * w + x) * 3 + c];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
        let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
        top + (bottom - top) * fy
    })
}

/// The geometry of a placed input.
struct Placed {
    width: f64,
    height: f64,
    placement: Placement,
    /// Inverse homography for perspective placements.
    inverse: Option<Matrix3>,
}

impl Placed {
    /// Global coordinates of a continuous source point.
    fn forward(&self, projection: Projection, f: f64, x: f64, y: f64) -> Option<(f64, f64)> {
        let (u, v) = project(projection, f, self.width, self.height, x, y);
        match &self.placement {
            Placement::Shift(t) => Some((u + t[0], v + t[1])),
            Placement::Homography(m) => apply(m, u, v),
        }
    }

    /// The source point of global coordinates, if inside the source.
    fn source(&self, projection: Projection, f: f64, gx: f64, gy: f64) -> Option<(f64, f64)> {
        let (u, v) = match (&self.placement, &self.inverse) {
            (Placement::Shift(t), _) => (gx - t[0], gy - t[1]),
            (Placement::Homography(_), Some(m)) => apply(m, gx, gy)?,
            _ => return None,
        };
        let (x, y) = unproject(projection, f, self.width, self.height, u, v)?;
        (x >= 0.0 && y >= 0.0 && x < self.width && y < self.height).then_some((x, y))
    }
}

/// Stitch overlapping inputs, given left to right, into one panorama.
pub fn pano(inputs: &[Input], profiles: Profiles, options: &PanoOptions) -> Result<Merged> {
    let n = inputs.len();
    if !(2..=MAX_PANO_INPUTS).contains(&n) {
        bail!("[invalid-input] merge-pano takes 2 to {MAX_PANO_INPUTS} inputs, not {n}")
    }
    if options.focal.is_some_and(|f| !f.is_finite() || f <= 0.0) {
        bail!("[invalid-input] --focal must be a positive number of output pixels")
    }
    let projection = options.projection;
    // Pass 1: sizes and feature proxies.
    let mut sizes = Vec::with_capacity(n);
    let mut detected = Vec::with_capacity(n);
    let mut focal = options.focal;
    let mut focal_source = if focal.is_some() { "option" } else { "default" };
    let mut baseline = 0.0;
    let reference = (n - 1) / 2;
    for index in 0..n {
        let f = frame(inputs, index, profiles, options.scale)?;
        let (w, h) = (f.width, f.height);
        if focal.is_none() {
            if let Some(mm) = f.capture["focal_length"].as_f64() {
                focal = Some(mm / 36.0 * w.max(h) as f64);
                focal_source = "exif";
            }
        }
        if index == reference {
            baseline = f.baseline;
        }
        let k = (PROXY_EDGE as f64 / w.max(h) as f64).min(1.0);
        let (pw, ph) = scaled(w, h, k);
        let luma = resize(luminance(&f.rgb), 1, w, h, pw, ph, false);
        drop(f);
        let log: Vec<f32> = luma
            .iter()
            .map(|v| math::ln(f64::from(v.max(1.0e-6))) as f32)
            .collect();
        let found = features(&log, pw, ph);
        sizes.push((w, h));
        detected.push((found, pw as f64 / w as f64, ph as f64 / h as f64));
        check_cancelled()?;
    }
    let f = focal.unwrap_or_else(|| {
        let (w, h) = sizes[0];
        w.max(h) as f64
    });
    // Pairwise placement of each input relative to its left neighbour.
    let mut rng = SplitMix(options.seed);
    let mut chained = vec![Placement::Shift([0.0; 2])];
    let mut global = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut shift = [0.0f64; 2];
    if projection == Projection::Perspective {
        chained[0] = Placement::Homography(global);
    }
    let mut pair_report = Vec::new();
    for index in 0..n - 1 {
        let (fa, kxa, kya) = &detected[index];
        let (fb, kxb, kyb) = &detected[index + 1];
        let (wa, ha) = (sizes[index].0 as f64, sizes[index].1 as f64);
        let (wb, hb) = (sizes[index + 1].0 as f64, sizes[index + 1].1 as f64);
        let pairs: Vec<([f64; 2], [f64; 2])> = match_features(fa, fb)
            .into_iter()
            .map(|(i, j)| {
                let a = project(projection, f, wa, ha, fa[i].x / kxa, fa[i].y / kya);
                let b = project(projection, f, wb, hb, fb[j].x / kxb, fb[j].y / kyb);
                ([a.0, a.1], [b.0, b.1])
            })
            .collect();
        let threshold = 2.0 / kxa.min(*kxb);
        let minimum = if projection == Projection::Perspective {
            8
        } else {
            6
        };
        let fitted = fit(projection, &pairs, threshold, wa.max(ha), &mut rng);
        let Some((placement, inliers)) = fitted.filter(|(_, c)| *c >= minimum) else {
            bail!("[invalid-input] panorama inputs {} and {} share too few matching features ({} matches); give inputs left to right with about 30% overlap", index + 1, index + 2, pairs.len())
        };
        pair_report.push(
            json!({"pair": [index + 1, index + 2], "matches": pairs.len(), "inliers": inliers}),
        );
        chained.push(match placement {
            Placement::Shift(d) => {
                shift = [shift[0] + d[0], shift[1] + d[1]];
                Placement::Shift(shift)
            }
            Placement::Homography(m) => {
                global = mul(&global, &m);
                Placement::Homography(global)
            }
        });
        check_cancelled()?;
    }
    // Re-base on the middle input.
    let placed: Vec<Placed> = match chained[reference].clone() {
        Placement::Shift(base) => chained
            .iter()
            .zip(&sizes)
            .map(|(p, (w, h))| {
                let Placement::Shift(t) = p else {
                    unreachable!()
                };
                Placed {
                    width: *w as f64,
                    height: *h as f64,
                    placement: Placement::Shift([t[0] - base[0], t[1] - base[1]]),
                    inverse: None,
                }
            })
            .collect(),
        Placement::Homography(base) => {
            let back =
                inverse(&base).context("[invalid-input] the panorama alignment is degenerate")?;
            let mut out = Vec::with_capacity(n);
            for (p, (w, h)) in chained.iter().zip(&sizes) {
                let Placement::Homography(m) = p else {
                    unreachable!()
                };
                let m = mul(&back, m);
                let inverse =
                    inverse(&m).context("[invalid-input] the panorama alignment is degenerate")?;
                out.push(Placed {
                    width: *w as f64,
                    height: *h as f64,
                    placement: Placement::Homography(m),
                    inverse: Some(inverse),
                });
            }
            out
        }
    };
    // Canvas bounds from each input's border.
    let mut boxes = Vec::with_capacity(n);
    for (index, p) in placed.iter().enumerate() {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for t in 0..=32 {
            let s = f64::from(t) / 32.0;
            for (x, y) in [
                (s * p.width, 0.0),
                (s * p.width, p.height),
                (0.0, s * p.height),
                (p.width, s * p.height),
            ] {
                let Some((gx, gy)) = p.forward(projection, f, x, y) else {
                    bail!("[invalid-input] panorama input {} lies behind the perspective reference; use --projection cylindrical or spherical", index + 1)
                };
                b = [b[0].min(gx), b[1].min(gy), b[2].max(gx), b[3].max(gy)];
            }
        }
        boxes.push(b);
    }
    let origin = (
        boxes.iter().map(|b| b[0]).fold(f64::MAX, f64::min).floor(),
        boxes.iter().map(|b| b[1]).fold(f64::MAX, f64::min).floor(),
    );
    let extent = (
        boxes.iter().map(|b| b[2]).fold(f64::MIN, f64::max).ceil() - origin.0,
        boxes.iter().map(|b| b[3]).fold(f64::MIN, f64::max).ceil() - origin.1,
    );
    if !(extent.0.is_finite() && extent.1.is_finite()) || extent.0 * extent.1 > 1.0e12 {
        bail!("[limit-exceeded] the panorama alignment diverged; check the input order or use --projection cylindrical")
    }
    let (width, height) = (extent.0.max(1.0) as usize, extent.1.max(1.0) as usize);
    check_output(width as u64, height as u64, options.scale)?;
    // Seam labels: each canvas pixel belongs to the input whose own edge is farthest.
    let global = |u: usize, v: usize| (origin.0 + u as f64 + 0.5, origin.1 + v as f64 + 0.5);
    let pixel_boxes: Vec<[usize; 4]> = boxes
        .iter()
        .map(|b| {
            [
                ((b[0] - origin.0).floor().max(0.0) as usize).min(width),
                ((b[1] - origin.1).floor().max(0.0) as usize).min(height),
                ((b[2] - origin.0).ceil().max(0.0) as usize).min(width),
                ((b[3] - origin.1).ceil().max(0.0) as usize).min(height),
            ]
        })
        .collect();
    let mut labels = vec![u8::MAX; width * height];
    super::adjust::rows(&mut labels, width, &|v, row| {
        for (u, label) in row.iter_mut().enumerate() {
            let (gx, gy) = global(u, v);
            let mut best = 0.0;
            for (index, p) in placed.iter().enumerate() {
                let b = pixel_boxes[index];
                if u < b[0] || u >= b[2] || v < b[1] || v >= b[3] {
                    continue;
                }
                if let Some((x, y)) = p.source(projection, f, gx, gy) {
                    let edge = (x / p.width)
                        .min(1.0 - x / p.width)
                        .min(y / p.height)
                        .min(1.0 - y / p.height);
                    if *label == u8::MAX || edge > best {
                        best = edge;
                        *label = index as u8;
                    }
                }
            }
        }
    })?;
    // Pass 2: multiband accumulation, one input at a time.
    let padded = (
        width.div_ceil(ALIGN) * ALIGN,
        height.div_ceil(ALIGN) * ALIGN,
    );
    let level_size = |k: usize| (padded.0 >> k, padded.1 >> k);
    let mut sums: Vec<Vec<f32>> = (0..BANDS)
        .map(|k| vec![0.0; level_size(k).0 * level_size(k).1 * 3])
        .collect();
    let mut weights: Vec<Vec<f32>> = (0..BANDS)
        .map(|k| vec![0.0; level_size(k).0 * level_size(k).1])
        .collect();
    for (index, p) in placed.iter().enumerate() {
        let b = pixel_boxes[index];
        let x0 = b[0].saturating_sub(MARGIN) / ALIGN * ALIGN;
        let y0 = b[1].saturating_sub(MARGIN) / ALIGN * ALIGN;
        let x1 = ((b[2] + MARGIN).div_ceil(ALIGN) * ALIGN).min(padded.0);
        let y1 = ((b[3] + MARGIN).div_ceil(ALIGN) * ALIGN).min(padded.1);
        if x1 <= x0 || y1 <= y0 || !labels.contains(&(index as u8)) {
            continue;
        }
        let (bw, bh) = (x1 - x0, y1 - y0);
        let source = frame(inputs, index, profiles, options.scale)?;
        let (sw, sh) = (source.width, source.height);
        let mut image = vec![0.0f32; bw * bh * 3];
        let mut valid = vec![0.0f32; bw * bh];
        super::adjust::rows(&mut image, bw * 3, &|j, row| {
            for i in 0..bw {
                let (gx, gy) = global(x0 + i, y0 + j);
                if let Some((x, y)) = p.source(projection, f, gx, gy) {
                    row[i * 3..i * 3 + 3].copy_from_slice(&bilinear(&source.rgb, sw, sh, x, y));
                }
            }
        })?;
        super::adjust::rows(&mut valid, bw, &|j, row| {
            for (i, a) in row.iter_mut().enumerate() {
                let (gx, gy) = global(x0 + i, y0 + j);
                if p.source(projection, f, gx, gy).is_some() {
                    *a = 1.0;
                }
            }
        })?;
        drop(source);
        fill(&mut image, &valid, bw, bh);
        drop(valid);
        let mut mask = vec![0.0f32; bw * bh];
        for j in 0..bh {
            for i in 0..bw {
                let (u, v) = (x0 + i, y0 + j);
                if u < width && v < height && labels[v * width + u] == index as u8 {
                    mask[j * bw + i] = 1.0;
                }
            }
        }
        let mut gauss = vec![(image, bw, bh)];
        let mut masks = vec![mask];
        for _ in 1..BANDS {
            let (g, w, h) = gauss.last().unwrap();
            let (next, nw, nh) = reduce(g, *w, *h, 3);
            let (m, _, _) = reduce(masks.last().unwrap(), *w, *h, 1);
            gauss.push((next, nw, nh));
            masks.push(m);
        }
        for k in 0..BANDS {
            let (g, w, h) = &gauss[k];
            let band: Vec<f32> = if k + 1 < BANDS {
                let (coarse, cw, ch) = &gauss[k + 1];
                let up = expand(coarse, *cw, *ch, 3, *w, *h);
                g.iter().zip(&up).map(|(a, b)| a - b).collect()
            } else {
                g.clone()
            };
            let (lw, _) = level_size(k);
            let (ox, oy) = (x0 >> k, y0 >> k);
            for j in 0..*h {
                for i in 0..*w {
                    let m = masks[k][j * w + i];
                    if m <= 0.0 {
                        continue;
                    }
                    let at = (oy + j) * lw + ox + i;
                    weights[k][at] += m;
                    for c in 0..3 {
                        sums[k][at * 3 + c] += m * band[(j * w + i) * 3 + c];
                    }
                }
            }
        }
        check_cancelled()?;
    }
    // Collapse the normalized bands.
    let normalized = |k: usize| -> Vec<f32> {
        sums[k]
            .chunks_exact(3)
            .zip(&weights[k])
            .flat_map(|(s, w)| {
                if *w > 1.0e-8 {
                    [s[0] / w, s[1] / w, s[2] / w]
                } else {
                    [0.0; 3]
                }
            })
            .collect()
    };
    let mut result = normalized(BANDS - 1);
    for k in (0..BANDS - 1).rev() {
        let (cw, ch) = level_size(k + 1);
        let (w, h) = level_size(k);
        let up = expand(&result, cw, ch, 3, w, h);
        result = up.iter().zip(normalized(k)).map(|(a, b)| a + b).collect();
    }
    drop(sums);
    let gain = math::exp2(baseline) as f32;
    let mut rgb = Vec::with_capacity(width * height * 3);
    let mut alpha = Vec::with_capacity(width * height);
    for v in 0..height {
        for u in 0..width {
            let covered = labels[v * width + u] != u8::MAX;
            let at = (v * padded.0 + u) * 3;
            for c in 0..3 {
                rgb.push(if covered {
                    (result[at + c] * gain).max(0.0)
                } else {
                    0.0
                });
            }
            alpha.push(if covered { 1.0 } else { 0.0 });
        }
    }
    let inputs_report: Vec<Value> = placed
        .iter()
        .map(|p| match &p.placement {
            Placement::Shift(t) => json!({"translate": [number(t[0]), number(t[1])]}),
            Placement::Homography(m) => {
                json!({"homography": m.iter().flatten().map(|v| number(*v)).collect::<Vec<_>>()})
            }
        })
        .collect();
    let mut settings = json!({
        "projection": projection.name(),
        "seed": options.seed,
        "scale": number(options.scale),
    });
    if let Some(focal) = options.focal {
        settings["focal"] = number(focal);
    }
    Ok(Merged {
        width,
        height,
        rgb,
        alpha: alpha.iter().any(|a| *a < 1.0).then_some(alpha),
        settings,
        alignment: json!({
            "projection": projection.name(),
            "reference": reference + 1,
            "seed": options.seed,
            "focal": number(f),
            "focal_source": focal_source,
            "canvas": [width, height],
            "inputs": inputs_report,
            "pairs": pair_report,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_exposure_is_kept_only_when_the_measurement_agrees() {
        assert_eq!(choose_exposure(Some(4.0), Some(3.5)), Some((4.0, "exif")));
        assert_eq!(
            choose_exposure(Some(4.0), Some(2.0)),
            Some((2.0, "measured"))
        );
        assert_eq!(
            choose_exposure(Some(4.0), None),
            Some((4.0, "exif-unverified"))
        );
        assert_eq!(choose_exposure(None, None), None);
        let capture = json!({"exposure_time": [1, 100], "aperture": 4, "iso": 200});
        assert!((exif_exposure(&capture).unwrap() - 0.125).abs() < 1e-12);
        assert_eq!(exif_exposure(&json!({"aperture": 4})), None);
    }

    #[test]
    fn mtb_finds_an_integer_translation() {
        let (w, h) = (96, 80);
        let scene = |x: i64, y: i64| -> f32 {
            let (x, y) = (x as f64, y as f64);
            (0.3 + 0.15 * (x / 6.0).sin() * (y / 5.0).cos() + 0.1 * (x / 13.0 + y / 17.0).sin())
                as f32
        };
        let reference: Vec<f32> = (0..w * h)
            .map(|p| scene((p % w) as i64, (p / w) as i64))
            .collect();
        // image(p + s) = reference(p) with s = (5, -3), at a quarter of the exposure.
        let image: Vec<f32> = (0..w * h)
            .map(|p| scene((p % w) as i64 - 5, (p / w) as i64 + 3) * 0.25)
            .collect();
        let shift = mtb(&bitmaps(&reference, w, h), &bitmaps(&image, w, h));
        assert_eq!(shift, (5, -3));
    }

    #[test]
    fn pyramids_preserve_constants_and_projections_invert() {
        let data = vec![0.5f32; 37 * 23 * 3];
        let (small, w, h) = reduce(&data, 37, 23, 3);
        assert_eq!((w, h), (19, 12));
        assert!(small.iter().all(|v| (v - 0.5).abs() < 1e-6));
        let big = expand(&small, w, h, 3, 37, 23);
        assert!(big.iter().all(|v| (v - 0.5).abs() < 1e-6));
        for projection in [
            Projection::Cylindrical,
            Projection::Spherical,
            Projection::Perspective,
        ] {
            let (u, v) = project(projection, 300.0, 400.0, 300.0, 31.0, 270.0);
            let (x, y) = unproject(projection, 300.0, 400.0, 300.0, u, v).unwrap();
            assert!(
                (x - 31.0).abs() < 1e-6 && (y - 270.0).abs() < 1e-6,
                "{projection:?}"
            );
        }
    }

    #[test]
    fn homographies_fit_exact_correspondences() {
        let truth = [
            [1.02, 0.01, 30.0],
            [-0.02, 0.99, -12.0],
            [1.0e-5, -2.0e-5, 1.0],
        ];
        let pairs: Vec<_> = [
            (0.0, 0.0),
            (100.0, 0.0),
            (0.0, 80.0),
            (100.0, 80.0),
            (50.0, 40.0),
        ]
        .iter()
        .map(|(x, y)| {
            let (ax, ay) = apply(&truth, *x, *y).unwrap();
            ([ax, ay], [*x, *y])
        })
        .collect();
        let m = homography(&pairs, 100.0).unwrap();
        for (a, b) in &pairs {
            let (x, y) = apply(&m, b[0], b[1]).unwrap();
            assert!((x - a[0]).abs() < 1e-6 && (y - a[1]).abs() < 1e-6);
        }
    }
}

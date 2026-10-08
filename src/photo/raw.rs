//! Stage 1 of process 1: decode an inspected DNG into camera RGB.
//!
//! Unpack, opcode list 1, linearize (table, black and white levels), opcode
//! list 2, defective pixels, raw white-balance multipliers, highlight handling, demosaic, opcode
//! list 3 and the final crop. The result is linear camera RGB at the developed
//! size, before orientation, camera profile and lens correction.
use super::detail::{self, Defects};
use super::dng::{row_bytes, Compression, Dng, Format, Layout};
use super::opcode::{self, Plane, Stage};
use super::{check_cancelled, ljpeg};
use anyhow::{bail, Result};
use std::io::Read;

/// Peak memory a development may use; checked before decoding.
pub const MAX_DEVELOP_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Demosaic {
    Bilinear,
    /// Malvar–He–Cutler gradient-corrected linear interpolation.
    Mhc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlights {
    Clip,
    Blend,
}

/// The `raw` and white-balance inputs of stage 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decode {
    pub demosaic: Demosaic,
    pub highlights: Highlights,
    /// Camera-neutral white in camera RGB; multipliers are `max / neutral`.
    pub neutral: [f64; 3],
}

impl Decode {
    /// Import defaults with the as-shot neutral (or unity).
    pub fn as_shot(dng: &Dng) -> Self {
        let neutral = match dng.as_shot_neutral.as_deref() {
            Some([r, g, b]) => [*r, *g, *b],
            _ => [1.0; 3],
        };
        Self {
            demosaic: Demosaic::Mhc,
            highlights: Highlights::Blend,
            neutral,
        }
    }
}

/// Linear camera RGB, interleaved.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraRgb {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<f32>,
}

/// Peak bytes `decode` holds at once: the stored plane, the active plane, the
/// demosaiced copy and its clip map, plus the largest decoded segment.
pub fn develop_bytes(dng: &Dng) -> u64 {
    let stored = (dng.width * dng.height * dng.samples) as u64 * 4;
    let active = (dng.active.width() * dng.active.height()) as u64;
    let segment = (dng.segments.width * dng.segments.height * dng.samples) as u64 * 4;
    stored + active * (dng.samples as u64 + 3 + 3 + 1) * 4 + segment
}

/// Decode stage 1 without defective-pixel correction.
pub fn decode(dng: &Dng, options: &Decode) -> Result<CameraRgb> {
    decode_with(dng, options, &Defects::default())
}

/// Decode stage 1, replacing `defects` after opcode list 2.
pub fn decode_with(dng: &Dng, options: &Decode, defects: &Defects) -> Result<CameraRgb> {
    if let Some(op) = dng.unsupported_opcodes().first() {
        bail!(
            "[unsupported-capability] DNG opcode {} in OpcodeList{} is mandatory and engine 1 cannot apply it; re-export the DNG without it",
            op.id,
            op.list
        );
    }
    if options.neutral.iter().any(|n| !n.is_finite() || *n <= 0.0) {
        bail!(
            "[invalid-develop] white-balance neutral {:?} must hold three positive values",
            options.neutral
        );
    }
    let needed = develop_bytes(dng);
    if needed > MAX_DEVELOP_BYTES {
        bail!(
            "[limit-exceeded] developing this {}x{} raw needs {needed} bytes; the limit is {MAX_DEVELOP_BYTES} (2 GiB)",
            dng.width,
            dng.height
        );
    }
    let ops = |list: u8| -> Vec<opcode::Opcode> {
        dng.opcodes
            .iter()
            .filter(|op| op.list == list)
            .cloned()
            .collect()
    };

    let mut stored = Plane {
        width: dng.width,
        height: dng.height,
        channels: dng.samples,
        data: unpack(dng)?,
    };
    let stage = match dng.format {
        Format::Uint => Stage::Stored,
        Format::Float => Stage::Normalized,
    };
    opcode::apply(&ops(1), &mut stored, stage)?;

    let mut active = linearize(dng, &stored);
    drop(stored);
    opcode::apply(&ops(2), &mut active, Stage::Normalized)?;
    let cfa = match dng.layout {
        Layout::Cfa(cfa) => Some(cfa),
        Layout::LinearRaw(_) => None,
    };
    detail::fix_defects(&mut active, cfa, defects)?;
    check_cancelled()?;

    let (rgb, clip) = match dng.layout {
        Layout::Cfa(cfa) => {
            let colors = |x: usize, y: usize| cfa.color(x, y);
            let (mosaic, clip) = balance(&active, options, |x, y, _| Some(colors(x, y)));
            drop(active);
            let rgb = match options.demosaic {
                Demosaic::Bilinear => bilinear(&mosaic, &colors)?,
                Demosaic::Mhc => mhc(&mosaic, &colors)?,
            };
            (rgb, clip)
        }
        Layout::LinearRaw(_) => {
            let mono = active.channels == 1;
            let (balanced, clip) = balance(&active, options, |_, _, c| (!mono).then_some(c));
            let rgb = if balanced.channels == 1 {
                balanced.data.iter().flat_map(|v| [*v; 3]).collect()
            } else {
                balanced.data
            };
            (
                Plane {
                    width: active.width,
                    height: active.height,
                    channels: 3,
                    data: rgb,
                },
                clip,
            )
        }
    };
    let mut rgb = rgb;
    if options.highlights == Highlights::Blend {
        blend_highlights(&mut rgb, &clip);
    }
    opcode::apply(&ops(3), &mut rgb, Stage::Normalized)?;

    let crop = dng.crop;
    let mut out = Vec::with_capacity(crop.width() * crop.height() * 3);
    for y in crop.top..crop.bottom {
        let row = (y * rgb.width + crop.left) * 3;
        out.extend_from_slice(&rgb.data[row..row + crop.width() * 3]);
    }
    Ok(CameraRgb {
        width: crop.width(),
        height: crop.height(),
        rgb: out,
    })
}

/// Read every strip or tile into the stored plane as `f32` stored values.
fn unpack(dng: &Dng) -> Result<Vec<f32>> {
    let seg = &dng.segments;
    let samples = dng.samples;
    let mut plane = vec![0.0f32; dng.width * dng.height * samples];
    for (index, (offset, count)) in seg.spans.iter().enumerate() {
        check_cancelled()?;
        let x0 = (index % seg.across) * seg.width;
        let y0 = (index / seg.across) * seg.height;
        // Strips stop at the image bottom; tiles are always whole.
        let data_rows = if seg.tiled {
            seg.height
        } else {
            seg.height.min(dng.height - y0)
        };
        let row_values = seg.width * samples;
        let data = &dng.tiff.bytes[*offset..offset + count];
        let values = segment_values(dng, data, data_rows, index)?;
        let rows = data_rows.min(dng.height - y0);
        let cols = seg.width.min(dng.width - x0);
        for r in 0..rows {
            let source = &values[r * row_values..r * row_values + cols * samples];
            let start = ((y0 + r) * dng.width + x0) * samples;
            plane[start..start + cols * samples].copy_from_slice(source);
        }
    }
    Ok(plane)
}

/// The `rows * width * samples` values of one segment.
fn segment_values(dng: &Dng, data: &[u8], rows: usize, index: usize) -> Result<Vec<f32>> {
    let seg = &dng.segments;
    let width = seg.width;
    let per_row = width * dng.samples;
    let kind = if seg.tiled { "tile" } else { "strip" };
    let row_len = row_bytes(width, dng.samples, dng.bits);
    let bytes: std::borrow::Cow<[u8]> = match dng.compression {
        Compression::LosslessJpeg => {
            let frame = ljpeg::decode(data, rows * per_row).map_err(|error| {
                anyhow::anyhow!("{error} (lossless JPEG {kind} {index} of the raw IFD)")
            })?;
            return Ok(frame.samples.into_iter().map(f32::from).collect());
        }
        Compression::None => std::borrow::Cow::Borrowed(&data[..rows * row_len]),
        Compression::Deflate(_) => {
            let expected = rows * row_len;
            let mut out = Vec::with_capacity(expected);
            flate2::read::ZlibDecoder::new(data)
                .take(expected as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|error| {
                    anyhow::anyhow!("[malformed-resource] deflate {kind} {index} of the raw IFD is damaged: {error}")
                })?;
            if out.len() != expected {
                bail!(
                    "[malformed-resource] deflate {kind} {index} of the raw IFD inflates to {}{} bytes; it must hold exactly {expected}",
                    out.len().min(expected),
                    if out.len() > expected { "+" } else { "" }
                );
            }
            std::borrow::Cow::Owned(out)
        }
    };
    let predictor = match dng.compression {
        Compression::Deflate(p) => p,
        _ => 1,
    };
    let big = dng.tiff.big_endian;
    let mut values = Vec::with_capacity(rows * per_row);
    let mut row = vec![0u8; row_len];
    for r in 0..rows {
        row.copy_from_slice(&bytes[r * row_len..(r + 1) * row_len]);
        match dng.format {
            Format::Uint => {
                let start = values.len();
                read_uints(&row, dng.bits, big, per_row, &mut values);
                if predictor == 2 {
                    let modulus = (1u64 << dng.bits) as f32;
                    let line = &mut values[start..];
                    for i in dng.samples..per_row {
                        let v = line[i] + line[i - dng.samples];
                        line[i] = if v >= modulus { v - modulus } else { v };
                    }
                }
            }
            Format::Float => {
                let size = dng.bits as usize / 8;
                let big = if matches!(predictor, 34_894 | 34_895) {
                    let factor = if predictor == 34_894 { 2 } else { 4 };
                    let stride = dng.samples * factor;
                    for i in stride..row.len() {
                        row[i] = row[i].wrapping_add(row[i - stride]);
                    }
                    // Byte planes, most significant first.
                    let shuffled = row.clone();
                    for k in 0..per_row {
                        for j in 0..size {
                            row[k * size + j] = shuffled[j * per_row + k];
                        }
                    }
                    true
                } else {
                    big
                };
                for chunk in row.chunks_exact(size) {
                    let value = read_float(chunk, big);
                    if !value.is_finite() {
                        bail!("[malformed-resource] {kind} {index} of the raw IFD holds a float sample that is not finite");
                    }
                    values.push(value);
                }
            }
        }
    }
    Ok(values)
}

/// Unsigned samples of one row: 8 and 16 bits in file byte order, others
/// packed most-significant bit first.
fn read_uints(row: &[u8], bits: u32, big: bool, count: usize, out: &mut Vec<f32>) {
    match bits {
        8 => out.extend(row[..count].iter().map(|b| f32::from(*b))),
        16 => out.extend(row.chunks_exact(2).take(count).map(|b| {
            f32::from(if big {
                u16::from_be_bytes([b[0], b[1]])
            } else {
                u16::from_le_bytes([b[0], b[1]])
            })
        })),
        _ => {
            let (mut acc, mut have, mut bytes) = (0u32, 0u32, row.iter());
            for _ in 0..count {
                while have < bits {
                    acc = (acc << 8) | u32::from(*bytes.next().unwrap_or(&0));
                    have += 8;
                }
                have -= bits;
                out.push(((acc >> have) & ((1 << bits) - 1)) as f32);
            }
        }
    }
}

fn read_float(bytes: &[u8], big: bool) -> f32 {
    let mut b = [0u8; 4];
    b[..bytes.len()].copy_from_slice(bytes);
    if !big {
        b[..bytes.len()].reverse();
    }
    // `b` now holds the value most-significant byte first.
    match bytes.len() {
        2 => half(u16::from_be_bytes([b[0], b[1]])),
        3 => fp24(u32::from_be_bytes([0, b[0], b[1], b[2]])),
        _ => f32::from_be_bytes(b),
    }
}

/// IEEE binary16.
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * (2.0f32).powi(-24),
        31 => {
            if mantissa == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => sign * (1.0 + mantissa / 1024.0) * (2.0f32).powi(e - 15),
    }
}

/// DNG 24-bit float: sign, 7-bit exponent (bias 63), 16-bit mantissa.
fn fp24(bits: u32) -> f32 {
    let sign = if bits & 0x80_0000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((bits >> 16) & 0x7f) as i32;
    let mantissa = (bits & 0xffff) as f32;
    match exponent {
        0 => sign * mantissa * (2.0f32).powi(-62 - 16),
        127 => {
            if mantissa == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => sign * (1.0 + mantissa / 65_536.0) * (2.0f32).powi(e - 63),
    }
}

/// Crop to the active area and normalize: `(table(v) - black) / (white - black)`.
fn linearize(dng: &Dng, stored: &Plane) -> Plane {
    let area = dng.active;
    let samples = dng.samples;
    let (rows, cols) = dng.black_repeat;
    let mut data = Vec::with_capacity(area.width() * area.height() * samples);
    for y in 0..area.height() {
        let delta_v = dng.black_delta_v.as_ref().map_or(0.0, |d| d[y]);
        for x in 0..area.width() {
            let delta = delta_v + dng.black_delta_h.as_ref().map_or(0.0, |d| d[x]);
            let base = ((area.top + y) * stored.width + area.left + x) * samples;
            for s in 0..samples {
                let mut v = f64::from(stored.data[base + s]);
                if let Some(table) = &dng.linearization {
                    let index = (v.max(0.0) as usize).min(table.len() - 1);
                    v = f64::from(table[index]);
                }
                let black = dng.black[((y % rows) * cols + x % cols) * samples + s] + delta;
                let range = dng.white[s] - black;
                let n = if range > 0.0 {
                    (v - black) / range
                } else {
                    0.0
                };
                data.push(n as f32);
            }
        }
    }
    Plane {
        width: area.width(),
        height: area.height(),
        channels: samples,
        data,
    }
}

/// Apply the white-balance multipliers and the `clip` highlight rule.
/// `color(x, y, sample)` names the camera color of a value; `None` (monochrome)
/// is not balanced. Returns the balanced plane and, per pixel, the pre-balance
/// level `min(n, 1)` that weights `blend`.
fn balance(
    plane: &Plane,
    options: &Decode,
    color: impl Fn(usize, usize, usize) -> Option<usize>,
) -> (Plane, Vec<f32>) {
    let largest = options.neutral.iter().copied().fold(0.0, f64::max);
    let multipliers = options.neutral.map(|n| (largest / n) as f32);
    let channels = plane.channels;
    let mut data = Vec::with_capacity(plane.data.len());
    let mut level = Vec::with_capacity(plane.width * plane.height);
    for y in 0..plane.height {
        for x in 0..plane.width {
            let base = (y * plane.width + x) * channels;
            let mut peak = 0.0f32;
            for c in 0..channels {
                let n = plane.data[base + c];
                peak = peak.max(n.min(1.0));
                let m = color(x, y, c).map_or(1.0, |c| multipliers[c]);
                data.push(match options.highlights {
                    Highlights::Clip => (n * m).min(1.0),
                    Highlights::Blend => n.min(1.0) * m,
                });
            }
            level.push(peak);
        }
    }
    (
        Plane {
            width: plane.width,
            height: plane.height,
            channels,
            data,
        },
        level,
    )
}

/// Blend near-clipped pixels toward their brightest channel so clipped
/// highlights turn neutral instead of magenta. The weight is
/// `clamp((s - 0.95) / 0.05, 0, 1)` where `s` is the largest pre-balance level
/// in the pixel's 3x3 neighborhood.
fn blend_highlights(rgb: &mut Plane, level: &[f32]) {
    let (w, h) = (rgb.width, rgb.height);
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0f32;
            for ny in y.saturating_sub(1)..(y + 2).min(h) {
                for nx in x.saturating_sub(1)..(x + 2).min(w) {
                    s = s.max(level[ny * w + nx]);
                }
            }
            let weight = ((s - 0.95) / 0.05).clamp(0.0, 1.0);
            if weight > 0.0 {
                let px = &mut rgb.data[(y * w + x) * 3..(y * w + x) * 3 + 3];
                let top = px[0].max(px[1]).max(px[2]);
                for v in px {
                    *v += (top - *v) * weight;
                }
            }
        }
    }
}

/// Mirror an index into `0..n` without repeating the edge (reflect-101), which
/// keeps the CFA parity of the mirrored site. `n` is at least 2.
fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    let mut i = i;
    loop {
        if i < 0 {
            i = -i;
        } else if i >= n {
            i = 2 * (n - 1) - i;
        } else {
            return i as usize;
        }
    }
}

/// Bilinear demosaic: each missing color is the mean of the sites of that
/// color in the 3x3 neighborhood.
fn bilinear(mosaic: &Plane, colors: &impl Fn(usize, usize) -> usize) -> Result<Plane> {
    let (w, h) = (mosaic.width, mosaic.height);
    let mut data = vec![0.0f32; w * h * 3];
    for y in 0..h {
        if y % 64 == 0 {
            check_cancelled()?;
        }
        for x in 0..w {
            let site = colors(x, y);
            let (mut sum, mut n) = ([0.0f32; 3], [0u32; 3]);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (sx, sy) = (reflect(x as isize + dx, w), reflect(y as isize + dy, h));
                    let c = colors(sx, sy);
                    if c != site {
                        sum[c] += mosaic.data[sy * w + sx];
                        n[c] += 1;
                    }
                }
            }
            let out = &mut data[(y * w + x) * 3..(y * w + x) * 3 + 3];
            for c in 0..3 {
                out[c] = if c == site {
                    mosaic.data[y * w + x]
                } else if n[c] > 0 {
                    sum[c] / n[c] as f32
                } else {
                    0.0
                };
            }
        }
    }
    Ok(Plane {
        width: w,
        height: h,
        channels: 3,
        data,
    })
}

/// Malvar–He–Cutler kernels in sixteenths, indexed `[dy + 2][dx + 2]`.
const MHC_G_AT_RB: [[i8; 5]; 5] = [
    [0, 0, -2, 0, 0],
    [0, 0, 4, 0, 0],
    [-2, 4, 8, 4, -2],
    [0, 0, 4, 0, 0],
    [0, 0, -2, 0, 0],
];
/// Red or blue at a green site whose horizontal neighbors have that color.
const MHC_RB_AT_G_ROW: [[i8; 5]; 5] = [
    [0, 0, 1, 0, 0],
    [0, -2, 0, -2, 0],
    [-2, 8, 10, 8, -2],
    [0, -2, 0, -2, 0],
    [0, 0, 1, 0, 0],
];
/// Red or blue at a green site whose vertical neighbors have that color.
const MHC_RB_AT_G_COLUMN: [[i8; 5]; 5] = [
    [0, 0, -2, 0, 0],
    [0, -2, 8, -2, 0],
    [1, 0, 10, 0, 1],
    [0, -2, 8, -2, 0],
    [0, 0, -2, 0, 0],
];
/// Red at a blue site, or blue at a red site.
const MHC_RB_AT_BR: [[i8; 5]; 5] = [
    [0, 0, -3, 0, 0],
    [0, 4, 0, 4, 0],
    [-3, 0, 12, 0, -3],
    [0, 4, 0, 4, 0],
    [0, 0, -3, 0, 0],
];

/// Malvar–He–Cutler demosaic with reflect-101 edges; results are clamped at 0.
fn mhc(mosaic: &Plane, colors: &impl Fn(usize, usize) -> usize) -> Result<Plane> {
    let (w, h) = (mosaic.width, mosaic.height);
    let mut data = vec![0.0f32; w * h * 3];
    let mut window = [[0.0f32; 5]; 5];
    for y in 0..h {
        if y % 64 == 0 {
            check_cancelled()?;
        }
        for x in 0..w {
            for (dy, line) in window.iter_mut().enumerate() {
                let sy = reflect(y as isize + dy as isize - 2, h);
                for (dx, value) in line.iter_mut().enumerate() {
                    let sx = reflect(x as isize + dx as isize - 2, w);
                    *value = mosaic.data[sy * w + sx];
                }
            }
            let apply = |kernel: &[[i8; 5]; 5]| -> f32 {
                let mut sum = 0.0f32;
                for (k, v) in kernel.iter().flatten().zip(window.iter().flatten()) {
                    if *k != 0 {
                        sum += f32::from(*k) * v;
                    }
                }
                (sum / 16.0).max(0.0)
            };
            let site = colors(x, y);
            let horizontal = colors(reflect(x as isize + 1, w), y);
            let out = &mut data[(y * w + x) * 3..(y * w + x) * 3 + 3];
            for (c, value) in out.iter_mut().enumerate() {
                *value = if c == site {
                    window[2][2]
                } else if c == 1 {
                    apply(&MHC_G_AT_RB)
                } else if site == 1 {
                    if horizontal == c {
                        apply(&MHC_RB_AT_G_ROW)
                    } else {
                        apply(&MHC_RB_AT_G_COLUMN)
                    }
                } else {
                    apply(&MHC_RB_AT_BR)
                };
            }
        }
    }
    Ok(Plane {
        width: w,
        height: h,
        channels: 3,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo::dng::Cfa;

    fn flat(cfa: Cfa, w: usize, h: usize, rgb: [f32; 3]) -> Plane {
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                data.push(rgb[cfa.color(x, y)]);
            }
        }
        Plane {
            width: w,
            height: h,
            channels: 1,
            data,
        }
    }

    #[test]
    fn demosaic_reproduces_flat_fields_for_every_pattern() {
        for cfa in [Cfa::Rggb, Cfa::Bggr, Cfa::Grbg, Cfa::Gbrg] {
            let colors = |x: usize, y: usize| cfa.color(x, y);
            let mosaic = flat(cfa, 7, 6, [0.25, 0.5, 0.75]);
            for rgb in [
                bilinear(&mosaic, &colors).unwrap(),
                mhc(&mosaic, &colors).unwrap(),
            ] {
                for px in rgb.data.chunks_exact(3) {
                    for (v, e) in px.iter().zip([0.25, 0.5, 0.75]) {
                        assert!((v - e).abs() < 1e-6, "{cfa:?} {px:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn mhc_kernels_sum_to_sixteen() {
        for kernel in [
            MHC_G_AT_RB,
            MHC_RB_AT_G_ROW,
            MHC_RB_AT_G_COLUMN,
            MHC_RB_AT_BR,
        ] {
            assert_eq!(
                kernel.iter().flatten().map(|k| i32::from(*k)).sum::<i32>(),
                16
            );
        }
    }

    #[test]
    fn packed_samples_and_floats_unpack() {
        let mut out = Vec::new();
        // 12-bit 0xABC, 0x123 packed MSB first.
        read_uints(&[0xAB, 0xC1, 0x23], 12, false, 2, &mut out);
        assert_eq!(out, [2748.0, 291.0]);
        out.clear();
        // 10-bit 1023, 0, 512, 1.
        read_uints(&[0xFF, 0xC0, 0x08, 0x00, 0x01], 10, true, 4, &mut out);
        assert_eq!(out, [1023.0, 0.0, 512.0, 1.0]);
        assert_eq!(half(0x3C00), 1.0);
        assert_eq!(half(0x3800), 0.5);
        assert_eq!(fp24(0x3F_0000), 1.0);
        assert_eq!(fp24(0x3E_8000), 0.75);
        assert_eq!(read_float(&[0x00, 0x3C], false), 1.0);
        assert_eq!(reflect(-2, 5), 2);
        assert_eq!(reflect(6, 5), 2);
        assert_eq!(reflect(-2, 2), 0);
    }

    /// A little-endian DNG with one CFA strip per entry of `strips`.
    fn dng(
        width: u32,
        height: u32,
        bits: u16,
        compression: u16,
        strips: &[Vec<u8>],
        extra: Vec<(u16, u16, Vec<u8>)>,
    ) -> Vec<u8> {
        let short = |v: &[u16]| v.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let long = |v: &[u32]| v.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let mut out = b"II*\0\0\0\0\0".to_vec();
        let mut offsets = Vec::new();
        for strip in strips {
            offsets.push(out.len() as u32);
            out.extend(strip);
        }
        let rows = height.div_ceil(strips.len() as u32);
        let mut entries = vec![
            (254u16, 4u16, long(&[0])),
            (256, 4, long(&[width])),
            (257, 4, long(&[height])),
            (258, 3, short(&[bits])),
            (259, 3, short(&[compression])),
            (262, 3, short(&[32803])),
            (273, 4, long(&offsets)),
            (277, 3, short(&[1])),
            (278, 4, long(&[rows])),
            (
                279,
                4,
                long(&strips.iter().map(|s| s.len() as u32).collect::<Vec<_>>()),
            ),
            (33421, 3, short(&[2, 2])),
            (33422, 1, vec![0, 1, 1, 2]),
            (50706, 1, vec![1, 4, 0, 0]),
            (50708, 2, b"Test Body\0".to_vec()),
            (50714, 4, long(&[100])),
            (50717, 4, long(&[1100])),
            (
                50721,
                10,
                [1, 0, 0, 0, 1, 0, 0, 0, 1]
                    .iter()
                    .flat_map(|v: &i32| [v.to_le_bytes(), 1i32.to_le_bytes()].concat())
                    .collect(),
            ),
        ];
        entries.extend(extra);
        entries.sort_by_key(|e| e.0);
        if out.len() % 2 == 1 {
            out.push(0);
        }
        let ifd = out.len();
        out[4..8].copy_from_slice(&(ifd as u32).to_le_bytes());
        let mut data = ifd + 2 + entries.len() * 12 + 4;
        let mut tail: Vec<u8> = Vec::new();
        out.extend((entries.len() as u16).to_le_bytes());
        for (tag, kind, bytes) in &entries {
            let size = match kind {
                1 | 2 | 7 => 1,
                3 => 2,
                4 => 4,
                _ => 8,
            };
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(((bytes.len() / size) as u32).to_le_bytes());
            if bytes.len() <= 4 {
                let mut inline = bytes.clone();
                inline.resize(4, 0);
                out.extend(inline);
            } else {
                out.extend((data as u32).to_le_bytes());
                tail.extend(bytes);
                data += bytes.len();
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(tail);
        out
    }

    /// 4x4 stored values 100..1100.
    fn values() -> Vec<u16> {
        (0..16).map(|i| 100 + i * 61).collect()
    }

    fn reference() -> Decode {
        Decode {
            demosaic: Demosaic::Bilinear,
            highlights: Highlights::Clip,
            neutral: [1.0; 3],
        }
    }

    fn develop(bytes: &[u8]) -> Result<CameraRgb> {
        decode(&Dng::inspect(bytes)?, &reference())
    }

    #[test]
    fn every_compression_decodes_to_the_same_image() {
        let v = values();
        let plain: Vec<u8> = v.iter().flat_map(|v| v.to_le_bytes()).collect();
        let expected = develop(&dng(4, 4, 16, 1, &[plain.clone()], vec![])).unwrap();
        assert_eq!((expected.width, expected.height), (4, 4));
        // The red site at (0, 0) is normalized but otherwise untouched.
        assert_eq!(expected.rgb[0], 0.0);
        assert_eq!(
            expected.rgb[(4 + 1) * 3 + 2],
            (5.0f64 * 61.0 / 1000.0) as f32
        );

        // 12-bit, packed most-significant bit first: two values per three bytes.
        let packed: Vec<u8> = v
            .chunks(2)
            .flat_map(|p| {
                [
                    (p[0] >> 4) as u8,
                    ((p[0] & 15) << 4) as u8 | (p[1] >> 8) as u8,
                    p[1] as u8,
                ]
            })
            .collect();
        assert_eq!(
            develop(&dng(4, 4, 12, 1, &[packed], vec![])).unwrap(),
            expected
        );

        // Deflate with horizontal differencing, in two strips.
        let mut strips = Vec::new();
        for half in v.chunks(8) {
            let mut differenced = Vec::new();
            for row in half.chunks(4) {
                for (i, value) in row.iter().enumerate() {
                    let previous = if i == 0 { 0 } else { row[i - 1] };
                    differenced.extend(value.wrapping_sub(previous).to_le_bytes());
                }
            }
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut encoder, &differenced).unwrap();
            strips.push(encoder.finish().unwrap());
        }
        let predictor = (317, 3, 2u16.to_le_bytes().to_vec());
        assert_eq!(
            develop(&dng(4, 4, 16, 8, &strips, vec![predictor.clone()])).unwrap(),
            expected
        );

        // Lossless JPEG: the usual two-component frame of half the width.
        let jpeg = crate::photo::ljpeg::tests::encode(2, 4, 2, &v);
        assert_eq!(
            develop(&dng(4, 4, 16, 7, &[jpeg], vec![])).unwrap(),
            expected
        );

        // A deflate strip that inflates past its size is refused.
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &vec![0u8; 4096]).unwrap();
        let bomb = encoder.finish().unwrap();
        let error = develop(&dng(4, 4, 16, 8, &[bomb], vec![predictor]))
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("[malformed-resource]") && error.contains("exactly 32"),
            "{error}"
        );
    }

    #[test]
    fn opcodes_apply_and_unknown_mandatory_ones_block_development() {
        let plain: Vec<u8> = values().iter().flat_map(|v| v.to_le_bytes()).collect();
        let list = |id: u32, flags: u32, params: &[u8]| -> Vec<u8> {
            let mut out = 1u32.to_be_bytes().to_vec();
            for v in [id, 0x0103_0000, flags, params.len() as u32] {
                out.extend(v.to_be_bytes());
            }
            out.extend(params);
            out
        };
        // TrimBounds in list 3 shrinks the developed size.
        let trim: Vec<u8> = [1u32, 1, 3, 4]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let bytes = dng(
            4,
            4,
            16,
            1,
            &[plain.clone()],
            vec![(51022, 7, list(6, 0, &trim))],
        );
        let inspected = Dng::inspect(&bytes).unwrap();
        assert_eq!(inspected.pixel_size(), (3, 2));
        let trimmed = decode(&inspected, &reference()).unwrap();
        let full = develop(&dng(4, 4, 16, 1, &[plain.clone()], vec![])).unwrap();
        assert_eq!(&trimmed.rgb[..9], &full.rgb[(4 + 1) * 3..(4 + 4) * 3]);

        // An optional unknown opcode is skipped; a mandatory one is refused.
        let optional = dng(
            4,
            4,
            16,
            1,
            &[plain.clone()],
            vec![(51022, 7, list(99, 1, &[]))],
        );
        assert_eq!(develop(&optional).unwrap(), full);
        let mandatory = dng(4, 4, 16, 1, &[plain], vec![(51022, 7, list(99, 0, &[]))]);
        let inspected = Dng::inspect(&mandatory).unwrap();
        assert_eq!(inspected.raw_facts()["opcodes"][0]["applied"], false);
        let error = decode(&inspected, &reference()).unwrap_err().to_string();
        assert!(error.starts_with("[unsupported-capability]"), "{error}");
    }

    #[test]
    fn white_balance_and_highlight_modes() {
        // Every site at white: clip keeps 1, blend makes the pixel neutral.
        let white: Vec<u8> = [1100u16; 16].iter().flat_map(|v| v.to_le_bytes()).collect();
        let bytes = dng(4, 4, 16, 1, &[white], vec![]);
        let inspected = Dng::inspect(&bytes).unwrap();
        let mut options = reference();
        options.neutral = [0.5, 1.0, 0.8];
        let clip = decode(&inspected, &options).unwrap();
        assert!(clip.rgb.iter().all(|v| *v == 1.0));
        options.highlights = Highlights::Blend;
        let blend = decode(&inspected, &options).unwrap();
        for px in blend.rgb.chunks_exact(3) {
            assert_eq!(px, [2.0, 2.0, 2.0]);
        }
        options.neutral = [0.0, 1.0, 1.0];
        let error = decode(&inspected, &options).unwrap_err().to_string();
        assert!(error.starts_with("[invalid-develop]"), "{error}");
    }
}

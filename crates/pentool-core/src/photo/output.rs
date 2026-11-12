//! Stage 11 (output rendering) and wide-gamut and HDR delivery
//! (`docs/photography-v1.md`, "Wide-gamut and HDR delivery").
//!
//! Scene-referred pixels get the profile's look table and tone curve, the
//! process-1 shoulder and gamut mapping. Display-referred pixels (rendered
//! sources) skip the look and shoulder. The result is quantized with the
//! output transfer, measured from its own codes, and written as a PNG whose
//! color chunks are checked against those codes before the file is accepted.
use super::color::{self, ColorSpace, Matrix, Transfer, D65, NAMED_SPACES};
use super::icc;
use super::math;
use super::pixels::{self, alpha_code, Codes, Depth, Dither, Quantizer, Raster, Samples, Working};
use super::png;
use super::profile::Rendering;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::io::Write;

/// SDR reference white in HDR output (ITU-R BT.2408), cd/m².
pub const SDR_WHITE_NITS: f64 = 203.0;
/// PQ code 1.0, cd/m².
pub const PQ_NITS: f64 = 10_000.0;
/// Nominal HLG display peak for measuring light levels (BT.2100), cd/m².
pub const HLG_DISPLAY_NITS: f64 = 1000.0;
/// HLG system gamma at [`HLG_DISPLAY_NITS`].
pub const HLG_GAMMA: f64 = 1.2;
pub const MAX_HEADROOM: f64 = 4.0;
pub const DEFAULT_HEADROOM: f64 = 1.5;
/// The shoulder is the identity up to this fraction of the peak.
pub const KNEE: f64 = 0.8;
pub const DEFAULT_BINS: usize = 64;
pub const MAX_BINS: usize = 1024;
/// A pixel is outside a gamut when a channel there is below `-tolerance · Y`.
pub const OUTSIDE_TOLERANCE: f64 = 1e-4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Perceptual,
    RelativeColorimetric,
}

impl Intent {
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "perceptual" => Ok(Self::Perceptual),
            "relative-colorimetric" => Ok(Self::RelativeColorimetric),
            other => bail!(
                "[invalid-input] intent {other:?} must be perceptual or relative-colorimetric"
            ),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Perceptual => "perceptual",
            Self::RelativeColorimetric => "relative-colorimetric",
        }
    }

    /// The ICC header rendering intent.
    fn icc(self) -> u32 {
        match self {
            Self::Perceptual => 0,
            Self::RelativeColorimetric => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrTransfer {
    Pq,
    Hlg,
}

impl HdrTransfer {
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "pq" => Ok(Self::Pq),
            "hlg" => Ok(Self::Hlg),
            other => bail!("[invalid-input] HDR transfer {other:?} must be pq or hlg"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Pq => "pq",
            Self::Hlg => "hlg",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hdr {
    pub transfer: HdrTransfer,
    /// Stops above SDR white, 0–4.
    pub headroom: f64,
}

/// HLG scene light of SDR white: the signal 0.75 of BT.2408.
pub fn hlg_reference() -> f64 {
    Transfer::Hlg.decode(0.75)
}

/// The most headroom HLG can carry above SDR white (about 1.92 stops).
pub fn hlg_max_headroom() -> f64 {
    -math::log2(hlg_reference())
}

/// The name a report gives a transfer.
pub fn transfer_name(transfer: Transfer) -> &'static str {
    match transfer {
        Transfer::Linear => "linear",
        Transfer::Srgb => "srgb",
        Transfer::Gamma(_) => "gamma",
        Transfer::Romm => "romm",
        Transfer::Bt709 => "bt709",
        Transfer::Pq => "pq",
        Transfer::Hlg => "hlg",
    }
}

/// An output: color space, depth, intent and optional HDR encoding.
#[derive(Debug, Clone)]
pub struct Output {
    pub space: ColorSpace,
    pub depth: Depth,
    pub intent: Intent,
    pub hdr: Option<Hdr>,
    pub dither: Dither,
}

impl Output {
    /// Resolve command options. HDR implies `rec2020` and 16 bits; SDR
    /// defaults to `srgb` and 8 bits.
    pub fn new(
        space: Option<&str>,
        depth: Option<u8>,
        intent: &str,
        hdr: Option<&str>,
        headroom: Option<f64>,
    ) -> Result<Self> {
        let intent = Intent::parse(intent)?;
        let hdr = match hdr {
            None => {
                if headroom.is_some() {
                    bail!("[invalid-input] --headroom applies to HDR output; add --hdr pq or --hdr hlg")
                }
                None
            }
            Some(name) => {
                let transfer = HdrTransfer::parse(name)?;
                let headroom = headroom.unwrap_or(DEFAULT_HEADROOM);
                if !(0.0..=MAX_HEADROOM).contains(&headroom) {
                    bail!("[invalid-input] --headroom {headroom} must be 0–{MAX_HEADROOM} stops")
                }
                let limit = hlg_max_headroom();
                if transfer == HdrTransfer::Hlg && headroom > limit {
                    bail!("[invalid-input] HLG carries at most {limit:.3} stops above SDR white (signal 0.75); lower --headroom or use --hdr pq")
                }
                Some(Hdr { transfer, headroom })
            }
        };
        let space = match (space, hdr) {
            (None, None) => "srgb",
            (None, Some(_)) | (Some("rec2020"), Some(_)) => "rec2020",
            (Some(other), Some(_)) => bail!(
                "[invalid-input] HDR output is rec2020; --space {other} cannot carry pq or hlg"
            ),
            (Some(name), None) => name,
        };
        let base = space.split_once(':').map_or(space, |(name, _)| name);
        if !NAMED_SPACES.contains(&base) {
            bail!(
                "[invalid-input] --space {space:?} must be one of {} (optionally with :linear)",
                NAMED_SPACES.join(", ")
            )
        }
        let space = ColorSpace::named(space)?;
        let depth = match (depth, hdr) {
            (None, None) | (Some(8), None) => Depth::Eight,
            (None, Some(_)) | (Some(16), _) => Depth::Sixteen,
            (Some(8), Some(_)) => {
                bail!("[invalid-input] HDR output is 16-bit; use --depth 16 or omit it")
            }
            (Some(other), _) => bail!("[invalid-input] --depth {other} must be 8 or 16"),
        };
        Ok(Self {
            space,
            depth,
            intent,
            hdr,
            dither: Dither::None,
        })
    }

    /// Peak output level relative to SDR white.
    pub fn peak(&self) -> f64 {
        self.hdr.map_or(1.0, |h| math::exp2(h.headroom))
    }

    /// Linear signal per unit of SDR-white-relative light.
    pub fn signal_scale(&self) -> f64 {
        match self.hdr.map(|h| h.transfer) {
            None => 1.0,
            Some(HdrTransfer::Pq) => SDR_WHITE_NITS / PQ_NITS,
            Some(HdrTransfer::Hlg) => hlg_reference(),
        }
    }

    /// The encoding transfer.
    pub fn transfer(&self) -> Transfer {
        match self.hdr.map(|h| h.transfer) {
            None => self.space.transfer,
            Some(HdrTransfer::Pq) => Transfer::Pq,
            Some(HdrTransfer::Hlg) => Transfer::Hlg,
        }
    }

    /// The mastering peak written to `mDCV`, cd/m².
    pub fn peak_nits(&self) -> Option<f64> {
        match self.hdr?.transfer {
            HdrTransfer::Pq => Some(SDR_WHITE_NITS * self.peak()),
            HdrTransfer::Hlg => Some(HLG_DISPLAY_NITS),
        }
    }

    /// `cICP` (primaries, transfer, matrix 0 = RGB, full range), when the space
    /// has ITU-T H.273 codes.
    pub fn cicp(&self) -> Option<[u8; 4]> {
        let base = self
            .space
            .id
            .split_once(':')
            .map_or(self.space.id.as_str(), |(n, _)| n);
        let primaries = match base {
            "srgb" => 1,
            "display-p3" => 12,
            "rec2020" => 9,
            _ => return None,
        };
        let transfer = match self.transfer() {
            Transfer::Linear => 8,
            Transfer::Srgb => 13,
            Transfer::Bt709 => 1,
            Transfer::Pq => 16,
            Transfer::Hlg => 18,
            Transfer::Gamma(_) | Transfer::Romm => return None,
        };
        Some([primaries, transfer, 0, 1])
    }

    /// The embedded ICC profile (SDR only; PQ and HLG are described by `cICP`).
    pub fn icc(&self) -> Option<Vec<u8>> {
        match self.hdr {
            Some(_) => None,
            None => icc::profile(&self.space, self.intent.icc()),
        }
    }
}

/// The process-1 shoulder on the max(R, G, B) norm, scaled to `peak`:
/// `f(m) = m` up to `0.8·peak`, then `peak·(0.8 + 0.2 (r − 0.8) / (r − 0.6))`
/// with `r = m / peak`. Every channel is scaled by `f(m) / m`, which keeps the
/// ratios between channels (hue) and approaches `peak` asymptotically.
pub fn shoulder(v: [f64; 3], peak: f64) -> [f64; 3] {
    let m = v[0].max(v[1]).max(v[2]);
    if m.is_nan() || m <= KNEE * peak {
        return v;
    }
    let r = m / peak;
    let f = peak * (KNEE + (1.0 - KNEE) * (r - KNEE) / (r - (2.0 * KNEE - 1.0)));
    let scale = f / m;
    v.map(|c| c * scale)
}

/// Perceptual gamut mapping: move toward the neutral of equal luminance `Y`
/// (a straight line in chromaticity toward the white, so the dominant
/// wavelength is kept) just far enough that every channel lies in
/// `[0, peak]`. `None` when the luminance itself is out of range, which only
/// clipping can resolve.
pub fn desaturate(v: [f64; 3], luminance: [f64; 3], peak: f64) -> Option<[f64; 3]> {
    let y = luminance[0] * v[0] + luminance[1] * v[1] + luminance[2] * v[2];
    if !(y > 0.0 && y < peak) {
        return None;
    }
    let mut s: f64 = 1.0;
    for c in v {
        if c < 0.0 {
            s = s.min(y / (y - c));
        } else if c > peak {
            s = s.min((peak - y) / (c - y));
        }
    }
    Some(v.map(|c| (y + s * (c - y)).clamp(0.0, peak)))
}

/// What stage 11 changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    /// Pixels with a channel outside the output range before mapping.
    pub out_of_gamut: u64,
    /// Out-of-gamut pixels moved into gamut by perceptual mapping.
    pub mapped: u64,
    /// Out-of-gamut pixels clipped per channel.
    pub clipped: u64,
    /// Pixels outside each named space (`NAMED_SPACES` order), before the shoulder.
    pub outside: [u64; 5],
}

/// Stage 11 and quantization of a developed image.
pub fn render(
    image: &Working,
    rendering: &Rendering,
    scene_referred: bool,
    output: &Output,
) -> Result<(Raster, Counts)> {
    render_with(image, rendering, scene_referred, output, None)
}

/// [`render`] that also marks each pixel that was outside the output range
/// before gamut mapping (the editor's gamut overlay).
pub fn render_marked(
    image: &Working,
    rendering: &Rendering,
    scene_referred: bool,
    output: &Output,
) -> Result<(Raster, Counts, Vec<bool>)> {
    let mut marks = vec![false; image.width as usize * image.height as usize];
    let (raster, counts) = render_with(image, rendering, scene_referred, output, Some(&mut marks))?;
    Ok((raster, counts, marks))
}

fn render_with(
    image: &Working,
    rendering: &Rendering,
    scene_referred: bool,
    output: &Output,
    mut marks: Option<&mut Vec<bool>>,
) -> Result<(Raster, Counts)> {
    let pixels = pixels::check_buffer(image)?;
    let quantizer = Quantizer::new(output.transfer(), output.depth, output.dither)?;
    let to_output: Matrix = output.space.from_working();
    let luminance = output.space.to_xyz()[1];
    let working_y = ColorSpace::working().to_xyz()[1];
    let gamuts: Vec<Matrix> = NAMED_SPACES
        .iter()
        .map(|name| ColorSpace::named(name).map(|s| s.from_working()))
        .collect::<Result<_>>()?;
    let (peak, k) = (output.peak(), output.signal_scale());
    let look = scene_referred && !rendering.is_identity();
    let width = image.width as usize;
    let mut codes = Codes::new(pixels, image.alpha.is_some(), output.depth);
    let mut counts = Counts::default();
    for (pixel, rgb) in image.rgb.chunks_exact(3).enumerate() {
        let (x, y) = (pixel % width, pixel / width);
        if x == 0 && y % 64 == 0 {
            super::check_cancelled()?;
        }
        let mut rgb = [rgb[0], rgb[1], rgb[2]];
        if look {
            rgb = rendering.apply(rgb);
        }
        let w = rgb.map(f64::from);
        let yw = working_y[0] * w[0] + working_y[1] * w[1] + working_y[2] * w[2];
        if yw > 0.0 {
            for (count, matrix) in counts.outside.iter_mut().zip(&gamuts) {
                if color::apply(matrix, w)
                    .iter()
                    .any(|c| *c < -OUTSIDE_TOLERANCE * yw)
                {
                    *count += 1;
                }
            }
        }
        let mut v = color::apply(&to_output, w);
        if scene_referred {
            v = shoulder(v, peak);
        }
        if v.iter().any(|c| !quantizer.in_range(c * k)) {
            counts.out_of_gamut += 1;
            if let Some(marks) = marks.as_deref_mut() {
                marks[pixel] = true;
            }
            match output.intent {
                Intent::RelativeColorimetric => counts.clipped += 1,
                Intent::Perceptual => match desaturate(v, luminance, peak) {
                    Some(mapped) => {
                        v = mapped;
                        counts.mapped += 1;
                    }
                    None => counts.clipped += 1,
                },
            }
        }
        for (channel, value) in v.into_iter().enumerate() {
            codes.set(pixel, channel, quantizer.code(value * k, x, y));
        }
        if let Some(alpha) = &image.alpha {
            codes.set(pixel, 3, alpha_code(alpha[pixel], output.depth.max()));
        }
    }
    Ok((codes.finish(image.width, image.height)?, counts))
}

/// Statistics measured from output codes.
#[derive(Debug, Clone, PartialEq)]
pub struct Measure {
    pub bins: usize,
    /// Per channel (R, G, B), counts of codes in `bins` equal ranges.
    pub histogram: [Vec<u64>; 3],
    /// Pixels measured (alpha code above 0).
    pub pixels: u64,
    pub transparent: u64,
    /// Largest luminance relative to SDR white.
    pub max_luminance: f64,
    /// HDR only: MaxCLL and MaxFALL in cd/m² (CTA-861.3, one frame).
    pub max_cll: Option<f64>,
    pub max_fall: Option<f64>,
}

/// Measure an output raster from its codes.
pub fn measure(raster: &Raster, output: &Output, bins: usize) -> Result<Measure> {
    if !(1..=MAX_BINS).contains(&bins) {
        bail!("[invalid-input] --bins {bins} must be 1–{MAX_BINS}")
    }
    let max = raster.depth().max();
    let transfer = output.transfer();
    let table: Vec<f64> = (0..=max)
        .map(|c| transfer.decode(f64::from(c) / f64::from(max)))
        .collect();
    let luminance = output.space.to_xyz()[1];
    let k = output.signal_scale();
    let hdr = output.hdr.map(|h| h.transfer);
    let channels = raster.channels();
    let code = |index: usize| -> u32 {
        match &raster.samples {
            Samples::Eight(v) => u32::from(v[index]),
            Samples::Sixteen(v) => u32::from(v[index]),
        }
    };
    let mut histogram: [Vec<u64>; 3] = std::array::from_fn(|_| vec![0; bins]);
    let (mut pixels, mut transparent) = (0u64, 0u64);
    let (mut max_luminance, mut max_cll, mut fall) = (0.0f64, 0.0f64, 0.0f64);
    let total = raster.width as usize * raster.height as usize;
    for pixel in 0..total {
        let base = pixel * channels;
        if raster.alpha && code(base + 3) == 0 {
            transparent += 1;
            continue;
        }
        pixels += 1;
        let c = [code(base), code(base + 1), code(base + 2)];
        for (channel, value) in c.iter().enumerate() {
            let bin = (*value as usize * bins) / (max as usize + 1);
            histogram[channel][bin] += 1;
        }
        let signal = c.map(|v| table[v as usize]);
        let relative = signal.map(|s| s / k);
        let y =
            luminance[0] * relative[0] + luminance[1] * relative[1] + luminance[2] * relative[2];
        max_luminance = max_luminance.max(y);
        let nits = match hdr {
            None => continue,
            Some(HdrTransfer::Pq) => signal.map(|s| s * PQ_NITS),
            Some(HdrTransfer::Hlg) => {
                // BT.2100 OOTF at the nominal display peak.
                let ys = 0.2627 * signal[0] + 0.6780 * signal[1] + 0.0593 * signal[2];
                let gain = if ys > 0.0 {
                    HLG_DISPLAY_NITS * math::pow(ys, HLG_GAMMA - 1.0)
                } else {
                    0.0
                };
                signal.map(|s| s * gain)
            }
        };
        let brightest = nits[0].max(nits[1]).max(nits[2]);
        max_cll = max_cll.max(brightest);
        fall += brightest;
    }
    let (max_cll, max_fall) = match hdr {
        None => (None, None),
        Some(_) => (
            Some(max_cll),
            Some(if pixels == 0 {
                0.0
            } else {
                fall / pixels as f64
            }),
        ),
    };
    Ok(Measure {
        bins,
        histogram,
        pixels,
        transparent,
        max_luminance,
        max_cll,
        max_fall,
    })
}

/// cd/m² to `mDCV`/`cLLI` units of 0.0001 cd/m², rounded up.
fn units(nits: f64) -> u32 {
    let scaled = (nits * 10_000.0).ceil();
    if scaled >= f64::from(u32::MAX) {
        u32::MAX
    } else if scaled > 0.0 {
        scaled as u32
    } else {
        0
    }
}

/// The color chunks of an output, in file order: `cICP`, `mDCV`, `cLLI`, `iCCP`.
pub fn chunks(output: &Output, measure: &Measure) -> Result<Vec<png::Chunk>> {
    let mut out = Vec::new();
    if let Some(cicp) = output.cicp() {
        out.push((*b"cICP", cicp.to_vec()));
    }
    if let (Some(peak), Some(cll), Some(fall)) =
        (output.peak_nits(), measure.max_cll, measure.max_fall)
    {
        let mut mdcv = Vec::with_capacity(24);
        let chromaticity = |v: f64| ((v * 50_000.0 + 0.5).floor() as u16).to_be_bytes();
        let space = &output.space;
        for (x, y) in [space.red, space.green, space.blue, D65] {
            mdcv.extend_from_slice(&chromaticity(x));
            mdcv.extend_from_slice(&chromaticity(y));
        }
        mdcv.extend_from_slice(&units(peak.max(cll)).to_be_bytes());
        mdcv.extend_from_slice(&1u32.to_be_bytes());
        out.push((*b"mDCV", mdcv));
        let mut clli = units(cll).to_be_bytes().to_vec();
        clli.extend_from_slice(&units(fall).to_be_bytes());
        out.push((*b"cLLI", clli));
    }
    if let Some(profile) = output.icc() {
        let mut iccp = b"pentool\0\0".to_vec();
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&profile)?;
        iccp.extend_from_slice(&encoder.finish()?);
        out.push((*b"iCCP", iccp));
    }
    Ok(out)
}

const COLOR_CHUNKS: [&[u8; 4]; 6] = [b"cICP", b"mDCV", b"cLLI", b"iCCP", b"sRGB", b"gAMA"];

/// Check a written PNG against its pixels: the decoded codes equal `raster`,
/// and every color chunk equals what those codes and `output` produce, so
/// `cLLI` holds the measured MaxCLL and MaxFALL.
pub fn verify(bytes: &[u8], raster: &Raster, output: &Output, measured: &Measure) -> Result<()> {
    let decoded = png::read(bytes)?.raster;
    if decoded != *raster {
        bail!("[malformed-resource] the written PNG does not decode to the rendered codes")
    }
    let remeasured = measure(&decoded, output, measured.bins)?;
    if remeasured != *measured {
        bail!("[malformed-resource] the written PNG measures differently from its rendering")
    }
    let found: Vec<png::Chunk> = png::chunks(bytes)?
        .into_iter()
        .filter(|(kind, _)| COLOR_CHUNKS.contains(&kind))
        .collect();
    if found != chunks(output, &remeasured)? {
        bail!("[malformed-resource] the PNG's color metadata (cICP, mDCV, cLLI, iCCP) does not match its pixels")
    }
    if let (Some(cll), Some(fall)) = (remeasured.max_cll, remeasured.max_fall) {
        if fall > cll {
            bail!("[malformed-resource] MaxFALL {fall} exceeds MaxCLL {cll}")
        }
    }
    Ok(())
}

/// The report of an output.
pub fn report(output: &Output, counts: &Counts, measure: &Measure) -> Value {
    let round = |v: f64, digits: i32| {
        let scale = 10f64.powi(digits);
        super::dng::number((v * scale).round() / scale)
    };
    let outside: serde_json::Map<String, Value> = NAMED_SPACES
        .iter()
        .zip(counts.outside)
        .map(|(name, count)| ((*name).to_string(), json!(count)))
        .collect();
    let mut report = json!({
        "space": output.space.id,
        "transfer": transfer_name(output.transfer()),
        "depth": output.depth.bits(),
        "intent": output.intent.name(),
        "hdr": output.hdr.map(|h| json!({
            "transfer": h.transfer.name(),
            "headroom": round(h.headroom, 6),
            "reference_white_nits": SDR_WHITE_NITS,
            "peak_nits": output.peak_nits().map(|v| round(v, 4)),
        })),
        "cicp": output.cicp(),
        "icc": output.icc().is_some(),
        "pixels": measure.pixels,
        "transparent_pixels": measure.transparent,
        "out_of_gamut_pixels": counts.out_of_gamut,
        "mapped_pixels": counts.mapped,
        "clipped_pixels": counts.clipped,
        "outside_gamut": outside,
        "max_luminance": round(measure.max_luminance, 6),
        "histogram": {
            "bins": measure.bins,
            "r": measure.histogram[0],
            "g": measure.histogram[1],
            "b": measure.histogram[2],
        },
    });
    if let (Some(cll), Some(fall)) = (measure.max_cll, measure.max_fall) {
        report["max_cll"] = round(cll, 4);
        report["max_fall"] = round(fall, 4);
    }
    report
}

/// A rendered output and its report.
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub raster: Raster,
    pub report: Value,
}

/// Stage 11, quantization, measurement and a verified PNG.
pub fn encode_png(
    image: &Working,
    rendering: &Rendering,
    scene_referred: bool,
    output: &Output,
    bins: usize,
) -> Result<Encoded> {
    let (raster, counts) = render(image, rendering, scene_referred, output)?;
    let measured = measure(&raster, output, bins)?;
    let bytes = png::write_tagged(&raster, &chunks(output, &measured)?)?;
    verify(&bytes, &raster, output, &measured)?;
    let report = report(output, &counts, &measured);
    Ok(Encoded {
        bytes,
        raster,
        report,
    })
}

/// Stage 11 and measurement without a file (`photo inspect`).
pub fn inspect(
    image: &Working,
    rendering: &Rendering,
    scene_referred: bool,
    output: &Output,
    bins: usize,
) -> Result<Value> {
    let (raster, counts) = render(image, rendering, scene_referred, output)?;
    let measured = measure(&raster, output, bins)?;
    Ok(report(output, &counts, &measured))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working(pixels: &[[f32; 3]]) -> Working {
        let mut image = Working::new(pixels.len() as u32, 1, false).unwrap();
        for (out, p) in image.rgb.chunks_exact_mut(3).zip(pixels) {
            out.copy_from_slice(p);
        }
        image
    }

    #[test]
    fn shoulder_is_identity_below_the_knee_and_smooth_to_the_asymptote() {
        assert_eq!(shoulder([0.8, 0.4, 0.1], 1.0), [0.8, 0.4, 0.1]);
        let mut previous = 0.8;
        for step in 1..200 {
            let m = 0.8 + f64::from(step) * 0.05;
            let out = shoulder([m, m / 2.0, 0.0], 1.0);
            assert!(out[0] > previous && out[0] < 1.0, "{m}: {out:?}");
            assert!((out[1] / out[0] - 0.5).abs() < 1e-12, "hue kept");
            previous = out[0];
        }
        // Slope 1 at the knee: f(0.8 + e) ≈ 0.8 + e.
        let e = 1e-6;
        assert!((shoulder([0.8 + e; 3], 1.0)[0] - (0.8 + e)).abs() < 1e-10);
        // Scaled to the peak in HDR.
        assert_eq!(shoulder([3.0; 3], 4.0), [3.0; 3]);
        assert!(shoulder([1e6; 3], 4.0)[0] < 4.0);
    }

    #[test]
    fn perceptual_mapping_keeps_luminance_and_direction() {
        let lum = ColorSpace::named("srgb").unwrap().to_xyz()[1];
        let v = [-0.2, 0.6, 0.3];
        let mapped = desaturate(v, lum, 1.0).unwrap();
        let y = |c: [f64; 3]| lum[0] * c[0] + lum[1] * c[1] + lum[2] * c[2];
        assert!((y(mapped) - y(v)).abs() < 1e-12);
        assert!(mapped.iter().all(|c| (0.0..=1.0).contains(c)));
        assert!(
            mapped[0].abs() < 1e-12,
            "the offending channel lands on the boundary"
        );
        // Collinear with the neutral of the same luminance.
        let n = y(v);
        let t0 = (mapped[1] - n) / (v[1] - n);
        let t1 = (mapped[2] - n) / (v[2] - n);
        assert!((t0 - t1).abs() < 1e-12);
        assert!(desaturate([-0.5, -0.5, 0.1], lum, 1.0).is_none());
        assert!(desaturate([2.0, 2.0, 2.0], lum, 1.0).is_none());
    }

    #[test]
    fn rec2020_green_maps_into_srgb_perceptually_and_clips_relatively() {
        let rec = ColorSpace::named("rec2020").unwrap();
        let green = color::apply(&rec.to_working(), [0.0, 0.6, 0.0]).map(|v| v as f32);
        let image = working(&[green, [0.18, 0.18, 0.18]]);
        let rendering = Rendering::default();
        let perceptual = Output::new(None, Some(16), "perceptual", None, None).unwrap();
        let (raster, counts) = render(&image, &rendering, true, &perceptual).unwrap();
        assert_eq!(
            (counts.out_of_gamut, counts.mapped, counts.clipped),
            (1, 1, 0)
        );
        assert_eq!(counts.outside, [1, 1, 1, 0, 0], "outside srgb, p3, adobe");
        let relative = Output::new(None, Some(16), "relative-colorimetric", None, None).unwrap();
        let (clipped, counts) = render(&image, &rendering, true, &relative).unwrap();
        assert_eq!((counts.mapped, counts.clipped), (0, 1));
        // The gray pixel is identical, the green differs.
        let Samples::Sixteen(a) = &raster.samples else {
            panic!()
        };
        let Samples::Sixteen(b) = &clipped.samples else {
            panic!()
        };
        assert_eq!(a[3..], b[3..]);
        assert_ne!(a[..3], b[..3]);
    }

    #[test]
    fn display_referred_in_gamut_values_round_trip_exactly() {
        let space = ColorSpace::named("display-p3").unwrap();
        let samples: Vec<u16> = (0..4096u32)
            .flat_map(|c| [(c * 16) as u16, 65_535 - (c * 16) as u16, (c * 7) as u16])
            .collect();
        let raster = Raster::new(64, 64, false, Samples::Sixteen(samples)).unwrap();
        let image = pixels::to_working(&raster, &space).unwrap();
        let output = Output::new(Some("display-p3"), Some(16), "perceptual", None, None).unwrap();
        let (back, counts) = render(&image, &Rendering::default(), false, &output).unwrap();
        assert_eq!(back, raster);
        assert_eq!(counts.mapped + counts.clipped, 0);
    }

    #[test]
    fn hdr_pq_puts_sdr_white_at_203_nits_and_verifies_metadata() {
        let image = working(&[[1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [100.0, 100.0, 100.0]]);
        let output = Output::new(None, None, "perceptual", Some("pq"), Some(2.0)).unwrap();
        assert_eq!(
            (output.depth, output.space.id.as_str()),
            (Depth::Sixteen, "rec2020")
        );
        let encoded = encode_png(&image, &Rendering::default(), true, &output, 16).unwrap();
        let report = &encoded.report;
        let cll = report["max_cll"].as_f64().unwrap();
        assert!(cll < 812.0 && cll > 790.0, "{report}");
        assert_eq!(report["cicp"], json!([9, 16, 0, 1]));
        assert_eq!(report["icc"], json!(false));
        let chunks = png::chunks(&encoded.bytes).unwrap();
        let clli = &chunks.iter().find(|c| &c.0 == b"cLLI").unwrap().1;
        assert_eq!(
            u32::from_be_bytes(clli[..4].try_into().unwrap()),
            units(cll)
        );
        // The white pixel decodes to 203 cd/m² within a code.
        let Samples::Sixteen(codes) = &encoded.raster.samples else {
            panic!()
        };
        let nits = Transfer::Pq.decode(f64::from(codes[0]) / 65535.0) * PQ_NITS;
        assert!((nits - 203.0).abs() < 0.05, "{nits}");
        // Tampering with cLLI is caught.
        let mut tampered = encoded.bytes.clone();
        let at = tampered.windows(4).position(|w| w == b"cLLI").unwrap();
        tampered[at + 4] ^= 1;
        let crc = png::crc32(&tampered[at..at + 12]);
        tampered[at + 12..at + 16].copy_from_slice(&crc.to_be_bytes());
        let measured = measure(&encoded.raster, &output, 16).unwrap();
        let error = verify(&tampered, &encoded.raster, &output, &measured)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not match its pixels"), "{error}");
    }

    #[test]
    fn hlg_reference_white_is_signal_075_and_headroom_is_bounded() {
        let image = working(&[[1.0, 1.0, 1.0]]);
        let output = Output::new(None, None, "perceptual", Some("hlg"), Some(1.0)).unwrap();
        let (raster, _) = render(&image, &Rendering::default(), true, &output).unwrap();
        let Samples::Sixteen(codes) = &raster.samples else {
            panic!()
        };
        assert_eq!(codes[0], (0.75f64 * 65535.0 + 0.5).floor() as u16);
        let measured = measure(&raster, &output, 4).unwrap();
        // 1000 cd/m² display: SDR white lands near 203 cd/m².
        assert!(
            (measured.max_cll.unwrap() - 203.0).abs() < 1.0,
            "{measured:?}"
        );
        assert!((hlg_max_headroom() - 1.916).abs() < 1e-3);
        let error = Output::new(None, None, "perceptual", Some("hlg"), Some(2.0))
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("[invalid-input]"), "{error}");
        for (space, depth, hdr) in [
            (Some("srgb"), None, Some("pq")),
            (None, Some(8), Some("pq")),
            (None, Some(12), None),
            (Some("cmyk"), None, None),
        ] {
            assert!(Output::new(space, depth, "perceptual", hdr, None).is_err());
        }
        assert!(Output::new(None, None, "perceptual", None, Some(1.0)).is_err());
    }

    #[test]
    fn sdr_png_carries_icc_and_cicp() {
        let image = working(&[[0.5, 0.2, 0.1]]);
        for (space, cicp) in [
            ("srgb", Some([1, 13, 0, 1])),
            ("display-p3", Some([12, 13, 0, 1])),
            ("rec2020", Some([9, 1, 0, 1])),
            ("srgb:linear", Some([1, 8, 0, 1])),
            ("adobe-rgb-1998", None),
            ("prophoto", None),
        ] {
            let output = Output::new(Some(space), Some(16), "perceptual", None, None).unwrap();
            let encoded = encode_png(&image, &Rendering::default(), true, &output, 8).unwrap();
            let chunks = png::chunks(&encoded.bytes).unwrap();
            let kinds: Vec<_> = chunks.iter().map(|c| c.0).collect();
            assert!(kinds.contains(b"iCCP"), "{space}");
            assert_eq!(kinds.contains(b"cICP"), cicp.is_some(), "{space}");
            assert!(!kinds.contains(b"cLLI"));
            assert_eq!(encoded.report["cicp"], json!(cicp));
        }
    }
}

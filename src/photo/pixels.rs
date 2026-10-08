//! 8- and 16-bit RGB storage and the `f32` working representation.
//!
//! Stored rasters are interleaved RGB or RGBA with straight alpha. The working
//! buffer holds linear working-space RGB (`prophoto-linear`) as `f32`, unclamped,
//! plus straight linear alpha. Conversion never passes through 8 bits or sRGB
//! unless the caller asks for that space and depth explicitly.
use super::color::{apply, ColorSpace, Matrix};
use anyhow::{bail, Result};

/// Largest photo surface, in pixels (`docs/photography-v1.md`).
pub const MAX_PHOTO_PIXELS: u64 = 120_000_000;
/// Largest photo width or height.
pub const MAX_PHOTO_DIMENSION: u32 = 32_768;
/// Largest working buffer, in bytes.
pub const MAX_WORKING_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 4x4 Bayer matrix for `ordered4` dithering, indexed `[y % 4][x % 4]`.
const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Eight,
    Sixteen,
}

impl Depth {
    /// Largest code value: 255 or 65535.
    pub fn max(self) -> u32 {
        match self {
            Self::Eight => 255,
            Self::Sixteen => 65_535,
        }
    }

    pub fn bits(self) -> u8 {
        match self {
            Self::Eight => 8,
            Self::Sixteen => 16,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samples {
    Eight(Vec<u8>),
    Sixteen(Vec<u16>),
}

impl Samples {
    pub fn len(&self) -> usize {
        match self {
            Self::Eight(values) => values.len(),
            Self::Sixteen(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn depth(&self) -> Depth {
        match self {
            Self::Eight(_) => Depth::Eight,
            Self::Sixteen(_) => Depth::Sixteen,
        }
    }

    fn get(&self, index: usize) -> u32 {
        match self {
            Self::Eight(values) => u32::from(values[index]),
            Self::Sixteen(values) => u32::from(values[index]),
        }
    }
}

/// Interleaved RGB or RGBA samples (straight alpha) in a named color space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
    pub samples: Samples,
}

impl Raster {
    pub fn new(width: u32, height: u32, alpha: bool, samples: Samples) -> Result<Self> {
        let channels = if alpha { 4 } else { 3 };
        let pixels = check_surface(width, height)?;
        if samples.len() as u64 != pixels * channels {
            bail!(
                "[malformed-resource] {width}x{height} {} raster needs {} samples, found {}",
                if alpha { "RGBA" } else { "RGB" },
                pixels * channels,
                samples.len()
            );
        }
        Ok(Self {
            width,
            height,
            alpha,
            samples,
        })
    }

    pub fn depth(&self) -> Depth {
        self.samples.depth()
    }

    pub fn channels(&self) -> usize {
        if self.alpha {
            4
        } else {
            3
        }
    }
}

/// Linear working-space RGB, `f32`, unclamped, with optional straight alpha.
#[derive(Debug, Clone, PartialEq)]
pub struct Working {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<f32>,
    pub alpha: Option<Vec<f32>>,
}

impl Working {
    /// A black, opaque working buffer, refused before allocation when too large.
    pub fn new(width: u32, height: u32, alpha: bool) -> Result<Self> {
        let pixels = check_working(width, height, alpha)? as usize;
        Ok(Self {
            width,
            height,
            rgb: vec![0.0; pixels * 3],
            alpha: alpha.then(|| vec![1.0; pixels]),
        })
    }
}

/// Output quantization noise shaping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dither {
    None,
    /// 4x4 ordered dither; 8-bit output only.
    Ordered4,
}

/// What an encode had to change to fit the output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncodeReport {
    /// Pixels with at least one channel outside the output gamut or range,
    /// clipped to the nearest code (relative colorimetric clip).
    pub clipped_pixels: u64,
}

/// Check a stored surface and return its pixel count.
pub fn check_surface(width: u32, height: u32) -> Result<u64> {
    if width == 0 || height == 0 {
        bail!("[malformed-resource] photo surface {width}x{height} is empty");
    }
    if width > MAX_PHOTO_DIMENSION || height > MAX_PHOTO_DIMENSION {
        bail!(
            "[limit-exceeded] photo surface {width}x{height} exceeds the {MAX_PHOTO_DIMENSION}-pixel side limit"
        );
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PHOTO_PIXELS {
        bail!(
            "[limit-exceeded] photo surface {width}x{height} has {pixels} pixels; the limit is {MAX_PHOTO_PIXELS}"
        );
    }
    Ok(pixels)
}

/// Check a working buffer before allocating it and return its pixel count.
pub fn check_working(width: u32, height: u32, alpha: bool) -> Result<u64> {
    let pixels = check_surface(width, height)?;
    let bytes = pixels * if alpha { 16 } else { 12 };
    if bytes > MAX_WORKING_BYTES {
        bail!(
            "[limit-exceeded] working buffer for {width}x{height} needs {bytes} bytes; the limit is {MAX_WORKING_BYTES}"
        );
    }
    Ok(pixels)
}

/// Stored samples in `space` to the linear `f32` working representation.
///
/// Each code `c` decodes to `transfer.decode(c / max)` through an `f64` lookup
/// table, then the `f64` matrix to the working space is applied and only the
/// result is rounded to `f32`. Alpha is linear: `c / max`.
pub fn to_working(raster: &Raster, space: &ColorSpace) -> Result<Working> {
    let mut working = Working::new(raster.width, raster.height, raster.alpha)?;
    let max = raster.depth().max();
    let table: Vec<f64> = (0..=max)
        .map(|code| space.transfer.decode(f64::from(code) / f64::from(max)))
        .collect();
    let matrix = space.to_working();
    let channels = raster.channels();
    for (pixel, rgb) in working.rgb.chunks_exact_mut(3).enumerate() {
        let base = pixel * channels;
        let linear = [
            table[raster.samples.get(base) as usize],
            table[raster.samples.get(base + 1) as usize],
            table[raster.samples.get(base + 2) as usize],
        ];
        let out = apply(&matrix, linear);
        for (stored, value) in rgb.iter_mut().zip(out) {
            *stored = value as f32;
        }
        if let Some(alpha) = working.alpha.as_mut() {
            alpha[pixel] = (f64::from(raster.samples.get(base + 3)) / f64::from(max)) as f32;
        }
    }
    Ok(working)
}

/// The working representation to stored samples in `space` at `depth`.
///
/// Values are converted to linear output RGB in `f64`, clipped to [0, 1], and
/// quantized by thresholds on linear light: the code is the number of `c` in
/// `1..=max` with `decode((c - t) / max) <= v`. `t` is 0.5 (round half up in
/// the encoded domain) or, for `ordered4`, `(bayer + 0.5) / 16`.
pub fn from_working(
    working: &Working,
    space: &ColorSpace,
    depth: Depth,
    dither: Dither,
) -> Result<(Raster, EncodeReport)> {
    if dither == Dither::Ordered4 && depth != Depth::Eight {
        bail!("[unsupported-capability] ordered4 dither applies to 8-bit output only; use dither none for 16-bit");
    }
    let pixels = check_working(working.width, working.height, working.alpha.is_some())? as usize;
    if working.rgb.len() != pixels * 3
        || working
            .alpha
            .as_ref()
            .is_some_and(|alpha| alpha.len() != pixels)
    {
        bail!(
            "[malformed-resource] working buffer does not match {}x{}",
            working.width,
            working.height
        );
    }
    let max = depth.max();
    let offsets: Vec<f64> = match dither {
        Dither::None => vec![0.5],
        Dither::Ordered4 => (0..16).map(|b| (f64::from(b) + 0.5) / 16.0).collect(),
    };
    // thresholds[k][c - 1] = decode((c - t_k) / max), increasing in c.
    let thresholds: Vec<Vec<f64>> = offsets
        .iter()
        .map(|t| {
            (1..=max)
                .map(|code| {
                    space
                        .transfer
                        .decode((f64::from(code) - t) / f64::from(max))
                })
                .collect()
        })
        .collect();
    let low = space.transfer.decode(-0.5 / f64::from(max));
    let high = space
        .transfer
        .decode((f64::from(max) + 0.5) / f64::from(max));
    let matrix: Matrix = space.from_working();
    let channels = if working.alpha.is_some() { 4 } else { 3 };
    let mut report = EncodeReport::default();
    let width = working.width as usize;
    let quantize = |table: &[f64], value: f64| -> u32 {
        let value = if value > 0.0 { value.min(1.0) } else { 0.0 };
        table.partition_point(|threshold| *threshold <= value) as u32
    };
    let (mut eight, mut sixteen) = match depth {
        Depth::Eight => (vec![0u8; pixels * channels], Vec::new()),
        Depth::Sixteen => (Vec::new(), vec![0u16; pixels * channels]),
    };
    for (pixel, rgb) in working.rgb.chunks_exact(3).enumerate() {
        let out = apply(
            &matrix,
            [f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2])],
        );
        let table = match dither {
            Dither::None => &thresholds[0],
            Dither::Ordered4 => {
                let (x, y) = (pixel % width, pixel / width);
                &thresholds[usize::from(BAYER4[y % 4][x % 4])]
            }
        };
        if out.iter().any(|value| !(*value >= low && *value <= high)) {
            report.clipped_pixels += 1;
        }
        let base = pixel * channels;
        for (channel, value) in out.into_iter().enumerate() {
            let code = quantize(table, value);
            match depth {
                Depth::Eight => eight[base + channel] = code as u8,
                Depth::Sixteen => sixteen[base + channel] = code as u16,
            }
        }
        if let Some(alpha) = &working.alpha {
            let value = f64::from(alpha[pixel]);
            let value = if value > 0.0 { value.min(1.0) } else { 0.0 };
            let code = ((value * f64::from(max) + 0.5).floor() as u32).min(max);
            match depth {
                Depth::Eight => eight[base + 3] = code as u8,
                Depth::Sixteen => sixteen[base + 3] = code as u16,
            }
        }
    }
    let samples = match depth {
        Depth::Eight => Samples::Eight(eight),
        Depth::Sixteen => Samples::Sixteen(sixteen),
    };
    let raster = Raster::new(
        working.width,
        working.height,
        working.alpha.is_some(),
        samples,
    )?;
    Ok((raster, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every 16-bit code on every channel, as a 256x256 RGB ramp with the
    /// channels offset so neighbours mix bright and dark values.
    fn ramp16() -> Raster {
        let samples = (0..65_536u32)
            .flat_map(|c| [c, (c + 21_845) % 65_536, (c + 43_690) % 65_536])
            .map(|c| c as u16)
            .collect();
        Raster::new(256, 256, false, Samples::Sixteen(samples)).unwrap()
    }

    fn round_trip(source: &Raster, name: &str) -> (Raster, EncodeReport) {
        let space = ColorSpace::named(name).unwrap();
        let working = to_working(source, &space).unwrap();
        from_working(&working, &space, source.depth(), Dither::None).unwrap()
    }

    #[test]
    fn sixteen_bit_round_trips_are_exact_for_curves_with_a_linear_toe() {
        let source = ramp16();
        for name in ["srgb", "display-p3", "prophoto", "rec2020", "srgb:linear"] {
            let (back, report) = round_trip(&source, name);
            assert_eq!(back, source, "{name}");
            assert_eq!(report.clipped_pixels, 0, "{name}");
        }
    }

    #[test]
    fn pure_power_round_trip_error_is_bounded_in_linear_light() {
        // Adobe RGB 1998 has no linear toe: near black, one f32 ulp of a bright
        // neighbouring channel spans many codes, but no linear-light change
        // exceeds 2^-24 (one f32 ulp at 1.0).
        let source = ramp16();
        let (back, _) = round_trip(&source, "adobe-rgb-1998");
        let gamma = super::super::color::Transfer::Gamma(563.0 / 256.0);
        let (Samples::Sixteen(a), Samples::Sixteen(b)) = (&source.samples, &back.samples) else {
            unreachable!()
        };
        let mut worst = 0.0f64;
        for (x, y) in a.iter().zip(b) {
            let linear = |code: u16| gamma.decode(f64::from(code) / 65_535.0);
            worst = worst.max((linear(*x) - linear(*y)).abs());
        }
        assert!(worst <= 1.0 / 16_777_216.0, "worst linear error {worst:e}");
        assert!(a.iter().zip(b).filter(|(x, y)| x != y).count() < 200);
    }

    #[test]
    fn eight_bit_and_alpha_round_trip_exactly() {
        let samples: Vec<u8> = (0..=255u8).flat_map(|c| [c, 255 - c, c / 2, c]).collect();
        let source = Raster::new(16, 16, true, Samples::Eight(samples)).unwrap();
        for name in [
            "srgb",
            "display-p3",
            "adobe-rgb-1998",
            "prophoto",
            "rec2020",
        ] {
            assert_eq!(round_trip(&source, name).0, source, "{name}");
        }
        let alpha: Vec<u16> = (0..65_536u32).flat_map(|c| [0, 0, 0, c as u16]).collect();
        let source = Raster::new(256, 256, true, Samples::Sixteen(alpha)).unwrap();
        assert_eq!(round_trip(&source, "srgb").0, source);
    }

    #[test]
    fn wide_gamut_colors_clip_only_when_leaving_the_gamut() {
        let green = Raster::new(1, 1, false, Samples::Sixteen(vec![0, 65_535, 0])).unwrap();
        let p3 = ColorSpace::named("display-p3").unwrap();
        let working = to_working(&green, &p3).unwrap();
        // P3 green survives the working space and comes back exactly.
        let (back, report) = from_working(&working, &p3, Depth::Sixteen, Dither::None).unwrap();
        assert_eq!((back, report.clipped_pixels), (green, 0));
        // In sRGB it is out of gamut: clipped, reported, never wrapped.
        let srgb = ColorSpace::named("srgb").unwrap();
        let (clipped, report) =
            from_working(&working, &srgb, Depth::Sixteen, Dither::None).unwrap();
        assert_eq!(report.clipped_pixels, 1);
        assert_eq!(clipped.samples, Samples::Sixteen(vec![0, 65_535, 0]));
    }

    #[test]
    fn ordered_dither_averages_between_codes_and_is_eight_bit_only() {
        // A linear value one third of the way from code 100 to code 101.
        let srgb = ColorSpace::named("srgb").unwrap();
        let level = |code: f64| srgb.transfer.decode(code / 255.0);
        let value = (level(100.0) + (level(101.0) - level(100.0)) / 3.0) as f32;
        let mut working = Working::new(4, 4, false).unwrap();
        let to_prophoto = srgb.to_working();
        let converted = apply(&to_prophoto, [f64::from(value); 3]);
        for rgb in working.rgb.chunks_exact_mut(3) {
            for (stored, channel) in rgb.iter_mut().zip(converted) {
                *stored = channel as f32;
            }
        }
        let (plain, _) = from_working(&working, &srgb, Depth::Eight, Dither::None).unwrap();
        let (dithered, _) = from_working(&working, &srgb, Depth::Eight, Dither::Ordered4).unwrap();
        let Samples::Eight(plain) = plain.samples else {
            unreachable!()
        };
        let Samples::Eight(dithered) = dithered.samples else {
            unreachable!()
        };
        assert!(plain.iter().all(|code| *code == 100));
        assert!(dithered.iter().all(|code| *code == 100 || *code == 101));
        let high = dithered.iter().filter(|code| **code == 101).count();
        assert!(
            (3 * 4..=3 * 7).contains(&high),
            "{high} of 48 samples rounded up"
        );
        let error = from_working(&working, &srgb, Depth::Sixteen, Dither::Ordered4).unwrap_err();
        assert!(error.to_string().starts_with("[unsupported-capability]"));
    }

    #[test]
    fn limits_are_refused_before_allocation() {
        for (width, height, code) in [
            (0, 10, "[malformed-resource]"),
            (32_769, 1, "[limit-exceeded]"),
            (32_768, 32_768, "[limit-exceeded]"),
        ] {
            let error = Working::new(width, height, true).unwrap_err().to_string();
            assert!(error.starts_with(code), "{width}x{height}: {error}");
        }
        assert!(check_working(10_000, 12_000, true).is_ok());
        let short = Raster::new(2, 2, false, Samples::Eight(vec![0; 11]));
        assert!(short
            .unwrap_err()
            .to_string()
            .starts_with("[malformed-resource]"));
    }

    #[test]
    fn conversions_are_deterministic() {
        let source = ramp16();
        let space = ColorSpace::named("display-p3").unwrap();
        let first = to_working(&source, &space).unwrap();
        let second = to_working(&source, &space).unwrap();
        let bits = |w: &Working| w.rgb.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&first), bits(&second));
    }
}

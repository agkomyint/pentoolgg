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
/// Matrix inversion and blending can straddle a gamut face by a few ulps.
pub const RGB_ROUNDOFF: f64 = 1e-12;
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

/// Straight-alpha source-over in an already selected RGB coordinate system.
/// Callers validate the mode and channels once before processing a surface.
/// Alpha is coverage and is never transfer encoded or passed through an ICC path.
pub(crate) fn source_over(mode: &str, backdrop: [f64; 4], source: [f64; 4]) -> [f64; 4] {
    let (sa, da) = (source[3], backdrop[3]);
    let alpha = sa + da * (1.0 - sa);
    if alpha == 0.0 {
        return [0.0; 4];
    }
    let cb = [backdrop[0], backdrop[1], backdrop[2]];
    let cs = [source[0], source[1], source[2]];
    let mixed = rgb(mode, cb, cs);
    let mut out = [0.0; 4];
    for i in 0..3 {
        out[i] = ((1.0 - sa) * da * cb[i] + (1.0 - da) * sa * cs[i] + sa * da * mixed[i]) / alpha;
    }
    out[3] = alpha;
    out
}

/// Extended-gamut policy of each blend mode on signed and above-one working RGB.
/// `normal` and the separable modes whose W3C formula is a total, continuous
/// function of real channels (multiply, screen, overlay, darken, lighten,
/// hard-light, soft-light, difference, exclusion) evaluate that formula
/// unclamped. Color-dodge and color-burn divide by a normalized term and clip
/// to the unit interval, and hue/saturation/color/luminosity clip their result
/// into the normalized gamut, so they have no faithful extension and refuse
/// channels outside [0,1] (modulo `RGB_ROUNDOFF`) instead of clipping silently.
pub fn extended_domain(mode: &str) -> bool {
    matches!(
        mode,
        "normal"
            | "multiply"
            | "screen"
            | "overlay"
            | "darken"
            | "lighten"
            | "hard-light"
            | "soft-light"
            | "difference"
            | "exclusion"
    )
}

/// Validate working samples separately from stored normalized device colors.
/// Modes with a total formula (see `extended_domain`) retain signed and
/// above-one RGB; the others require their normalized domain and never clip
/// implicitly.
pub(crate) fn working_source_over(
    mode: &str,
    mut backdrop: [f64; 4],
    mut source: [f64; 4],
) -> Result<[f64; 4]> {
    validate(mode, "linear")?;
    for pixel in [backdrop, source] {
        if pixel.iter().any(|v| !v.is_finite()) || !(0.0..=1.0).contains(&pixel[3]) {
            bail!("[invalid-composite] working RGB must be finite and coverage must be in [0,1]")
        }
    }
    if source[3] == 0.0 {
        return Ok(if backdrop[3] == 0.0 {
            [0.0; 4]
        } else {
            backdrop
        });
    }
    if backdrop[3] == 0.0 {
        return Ok(source);
    }
    if !extended_domain(mode) {
        for pixel in [&mut backdrop, &mut source] {
            for channel in &mut pixel[..3] {
                if !(-RGB_ROUNDOFF..=1.0 + RGB_ROUNDOFF).contains(channel) {
                    bail!("[unsupported-capability] {mode} blending of extended working RGB is undefined: its formula needs normalized channels; use normal source-over or a mode that extends (multiply, screen, overlay, darken, lighten, hard-light, soft-light, difference, exclusion)")
                }
                *channel = channel.clamp(0.0, 1.0);
            }
        }
    }
    let result = source_over(mode, backdrop, source);
    if result.iter().any(|v| !v.is_finite()) {
        bail!("[invalid-composite] working RGB compositing overflow")
    }
    Ok(result)
}

/// Bounded, unquantized straight-alpha linear RGB storage for the shared
/// compositor. Each surface carries its named working primaries so incompatible
/// contexts cannot accidentally blend their channel numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearSurface {
    width: u32,
    height: u32,
    space: String,
    pixels: Vec<[f64; 4]>,
}

/// Prepared bridge from named linear working primaries to the released sRGB
/// blend coordinate systems. This preserves stored blend-space declarations.
pub struct LegacyBlendContext {
    working: String,
    to_srgb: Option<crate::photo::color::Matrix>,
    from_srgb: Option<crate::photo::color::Matrix>,
}
impl LegacyBlendContext {
    pub fn new(working: &str) -> Result<Self> {
        LinearSurface::check_space(working)?;
        use crate::photo::color::{self, ColorSpace};
        let (to_srgb, from_srgb) = if working == "srgb:linear" {
            (None, None)
        } else {
            let source = ColorSpace::named(working)?;
            let srgb = ColorSpace::named("srgb:linear")?;
            let matrix = color::multiply(
                &color::invert(&srgb.to_xyz()),
                &color::multiply(
                    &color::bradford_xyz(color::xyz(source.white), color::xyz(srgb.white)),
                    &source.to_xyz(),
                ),
            );
            (Some(matrix), Some(color::invert(&matrix)))
        };
        Ok(Self {
            working: working.into(),
            to_srgb,
            from_srgb,
        })
    }
    fn transform(matrix: &Option<crate::photo::color::Matrix>, rgba: [f64; 4]) -> [f64; 4] {
        let rgb = matrix
            .as_ref()
            .map_or([rgba[0], rgba[1], rgba[2]], |matrix| {
                crate::photo::color::apply(matrix, [rgba[0], rgba[1], rgba[2]])
            });
        [rgb[0], rgb[1], rgb[2], rgba[3]]
    }
    pub(crate) fn coordinates(&self, rgba: [f64; 4], space: &str) -> [f64; 4] {
        let mut result = Self::transform(&self.to_srgb, rgba);
        if space == "srgb" {
            for channel in &mut result[..3] {
                *channel = crate::photo::color::Transfer::Srgb.encode(*channel);
            }
        }
        result
    }
    pub(crate) fn working_coordinates(&self, mut rgba: [f64; 4], space: &str) -> [f64; 4] {
        if space == "srgb" {
            for channel in &mut rgba[..3] {
                *channel = crate::photo::color::Transfer::Srgb.decode(*channel);
            }
        }
        Self::transform(&self.from_srgb, rgba)
    }
    pub(crate) fn working_space(&self) -> &str {
        &self.working
    }
    pub(crate) fn compose(
        &self,
        mode: &str,
        space: &str,
        backdrop: [f64; 4],
        source: [f64; 4],
    ) -> Result<[f64; 4]> {
        if source[3] == 0.0 {
            return Ok(if backdrop[3] == 0.0 {
                [0.0; 4]
            } else {
                backdrop
            });
        }
        if backdrop[3] == 0.0 || (mode == "normal" && source[3] == 1.0) {
            return Ok(source);
        }
        let mut b = Self::transform(&self.to_srgb, backdrop);
        let mut s = Self::transform(&self.to_srgb, source);
        let transfer = crate::photo::color::Transfer::Srgb;
        if space == "srgb" {
            for c in &mut b[..3] {
                *c = transfer.encode(*c);
            }
            for c in &mut s[..3] {
                *c = transfer.encode(*c);
            }
        }
        let mut result = working_source_over(mode, b, s)?;
        if space == "srgb" {
            for c in &mut result[..3] {
                *c = transfer.decode(*c);
            }
        }
        result = Self::transform(&self.from_srgb, result);
        if result.iter().any(|v| !v.is_finite()) {
            bail!("[invalid-composite] declared blend-space conversion overflow")
        }
        Ok(result)
    }
}

impl LinearSurface {
    /// Per-surface bound; renderers must additionally account for all live
    /// surfaces against their aggregate temporary-memory budget.
    pub const MAX_BYTES: usize = 256 * 1024 * 1024;

    fn count(width: u32, height: u32) -> Result<usize> {
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            bail!("[invalid-composite] linear surface dimensions must be in 1..=16384")
        }
        let count = (width as usize)
            .checked_mul(height as usize)
            .filter(|count| *count <= Self::MAX_BYTES / std::mem::size_of::<[f64; 4]>())
            .ok_or_else(|| anyhow::anyhow!("[limit-exceeded] linear surface exceeds 256 MiB"))?;
        Ok(count)
    }

    fn check_space(space: &str) -> Result<()> {
        if !space.ends_with(":linear") {
            bail!("[unsupported-capability] linear surface requires explicit named linear RGB primaries")
        }
        crate::photo::color::ColorSpace::named(space)?;
        Ok(())
    }

    pub fn new(width: u32, height: u32, space: &str) -> Result<Self> {
        Self::check_space(space)?;
        let count = Self::count(width, height)?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(count)?;
        pixels.resize(count, [0.0; 4]);
        Ok(Self {
            width,
            height,
            space: space.into(),
            pixels,
        })
    }

    /// Inputs are finite signed linear RGB and normalized independent coverage. Hidden RGB
    /// at alpha zero is canonicalized so it cannot leak into later operations.
    pub fn from_pixels(
        width: u32,
        height: u32,
        space: &str,
        mut pixels: Vec<[f64; 4]>,
    ) -> Result<Self> {
        Self::check_space(space)?;
        if pixels.capacity() > Self::MAX_BYTES / std::mem::size_of::<[f64; 4]>() {
            bail!("[limit-exceeded] linear surface backing allocation exceeds 256 MiB")
        }
        if Self::count(width, height)? != pixels.len() {
            bail!("[invalid-composite] linear surface pixel count differs from its dimensions")
        }
        if pixels
            .iter()
            .any(|pixel| pixel.iter().any(|v| !v.is_finite()) || !(0.0..=1.0).contains(&pixel[3]))
        {
            bail!("[invalid-composite] linear RGB must be finite and coverage must be in [0,1]")
        }
        for pixel in &mut pixels {
            if pixel[3] == 0.0 {
                *pixel = [0.0; 4];
            }
        }
        Ok(Self {
            width,
            height,
            space: space.into(),
            pixels,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub fn space(&self) -> &str {
        &self.space
    }
    pub fn pixels(&self) -> &[[f64; 4]] {
        &self.pixels
    }
    pub(crate) fn write_pixel(&mut self, index: usize, mut rgba: [f64; 4]) -> Result<()> {
        if rgba.iter().any(|v| !v.is_finite()) || !(0.0..=1.0).contains(&rgba[3]) {
            bail!("[invalid-composite] working fill produced nonfinite RGB or invalid coverage")
        }
        if rgba[3] == 0.0 {
            rgba = [0.0; 4];
        }
        *self.pixels.get_mut(index).ok_or_else(|| {
            anyhow::anyhow!("[invalid-composite] working fill pixel is outside its surface")
        })? = rgba;
        Ok(())
    }
    pub fn byte_len(&self) -> usize {
        self.pixels.capacity() * std::mem::size_of::<[f64; 4]>()
    }

    /// Blend without byte encoding or per-step quantization. An invalid request
    /// is rejected before changing any destination pixel.
    pub fn over(&mut self, source: &Self, mode: &str) -> Result<()> {
        validate(mode, "linear")?;
        if self.dimensions() != source.dimensions() {
            bail!("[invalid-composite] linear surface dimensions differ")
        }
        if self.space != source.space {
            bail!("[invalid-composite] linear surface working primaries differ; convert before blending")
        }
        // Validate all output samples before mutation, including overflow or an
        // unsupported extended-channel blend in a late pixel. No scratch copy.
        for (index, (backdrop, source)) in self.pixels.iter().zip(&source.pixels).enumerate() {
            if index % self.width as usize == 0 {
                crate::composite::check_cancelled()?;
            }
            working_source_over(mode, *backdrop, *source)?;
        }
        for (backdrop, source) in self.pixels.iter_mut().zip(&source.pixels) {
            *backdrop = working_source_over(mode, *backdrop, *source)?;
        }
        Ok(())
    }

    /// Honor released encoded/linear sRGB blend spaces while retaining float
    /// working storage. Preparation is shared across the whole scene.
    pub fn over_declared(
        &mut self,
        source: &Self,
        mode: &str,
        space: &str,
        context: &LegacyBlendContext,
    ) -> Result<()> {
        validate(mode, space)?;
        if self.dimensions() != source.dimensions()
            || self.space != source.space
            || self.space != context.working
        {
            bail!("[invalid-composite] declared blending requires matching dimensions and working primaries")
        }
        for (index, (b, s)) in self.pixels.iter().zip(&source.pixels).enumerate() {
            if index % self.width as usize == 0 {
                crate::composite::check_cancelled()?;
            }
            context.compose(mode, space, *b, *s)?;
        }
        for (b, s) in self.pixels.iter_mut().zip(&source.pixels) {
            *b = context.compose(mode, space, *b, *s)?;
        }
        Ok(())
    }

    pub fn apply_opacity(&mut self, opacity: f64) -> Result<()> {
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            bail!("[invalid-composite] opacity must be finite and in [0,1]")
        }
        for p in &mut self.pixels {
            p[3] *= opacity;
            if p[3] == 0.0 {
                *p = [0.0; 4];
            }
        }
        Ok(())
    }

    /// Apply masks and opacity to coverage only. Validate the entire mask before
    /// mutation so a malformed late sample cannot leave a partially masked image.
    pub fn apply_coverage(&mut self, coverage: &[f64], opacity: f64) -> Result<()> {
        if coverage.len() != self.pixels.len()
            || !opacity.is_finite()
            || !(0.0..=1.0).contains(&opacity)
            || coverage
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            bail!("[invalid-composite] coverage must match the surface and contain finite [0,1] values")
        }
        for (pixel, coverage) in self.pixels.iter_mut().zip(coverage) {
            pixel[3] *= coverage * opacity;
            if pixel[3] == 0.0 {
                *pixel = [0.0; 4];
            }
        }
        Ok(())
    }
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
        let composed = source_over(mode, [cb[0], cb[1], cb[2], da], [cs[0], cs[1], cs[2], sa]);
        for i in 0..3 {
            let c = composed[i];
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

#[cfg(test)]
mod linear_cancellation_tests {
    use super::*;

    #[test]
    fn cancellation_at_each_validation_row_leaves_destination_unchanged() {
        let original = LinearSurface::from_pixels(2, 3, "srgb:linear", vec![[0.2; 4]; 6]).unwrap();
        let source = LinearSurface::from_pixels(2, 3, "srgb:linear", vec![[0.4; 4]; 6]).unwrap();
        let context = LegacyBlendContext::new("srgb:linear").unwrap();
        for declared in [false, true] {
            for checkpoint in 1..=3 {
                let mut destination = original.clone();
                let (result, reached) = crate::composite::cancel_at_check(checkpoint, || {
                    if declared {
                        destination.over_declared(&source, "multiply", "srgb", &context)
                    } else {
                        destination.over(&source, "multiply")
                    }
                });
                assert!(reached);
                assert!(result.unwrap_err().to_string().contains("[cancelled]"));
                assert_eq!(destination, original);
            }
        }
    }
}

//! Production output transform: the one place a float working render becomes
//! quantized device samples. See `docs/professional-color-v1.md`.
//!
//! Working surfaces stay unquantized `f64` linear RGB. Delivery selects an
//! explicit destination (the document `output` role unless overridden), an
//! intent, an integer depth and an alpha policy, converts each pixel through the
//! D50 PCS and ICC link, and rounds half up with saturation exactly once.
use super::{
    icc::Intent,
    roles::{self, Role},
    value::{Destination, Evaluator, ProfileSet},
};
use crate::composite::WorkingRender;
use anyhow::{bail, ensure, Result};
use serde_json::{json, Value};
use std::path::Path;

/// Integer delivery depth; samples are stored in `u16` either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Eight,
    Sixteen,
}
impl Depth {
    pub fn parse(bits: u64) -> Result<Self> {
        match bits {
            8 => Ok(Self::Eight),
            16 => Ok(Self::Sixteen),
            _ => bail!("[invalid-input] output depth must be 8 or 16 bits"),
        }
    }
    pub fn bits(self) -> u32 {
        match self {
            Self::Eight => 8,
            Self::Sixteen => 16,
        }
    }
    fn max(self) -> f64 {
        match self {
            Self::Eight => 255.0,
            Self::Sixteen => 65535.0,
        }
    }
}

/// How coverage leaves the transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alpha {
    /// Keep straight alpha as a final channel. RGB and gray only; CMYK output
    /// has no alpha plane and refuses non-opaque pixels instead.
    Preserve,
    /// Composite over paper white in linear working light, then drop alpha.
    FlattenOnWhite,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Delivery destination; the document `output` role when absent.
    pub destination: Option<Destination>,
    /// Rendering intent; the `output` role's intent when absent.
    pub intent: Option<Intent>,
    pub depth: Depth,
    pub alpha: Alpha,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub pixels: u64,
    /// Pixels whose transform or destination encoding left the device gamut.
    pub clipped_pixels: u64,
    pub quantized_samples: u64,
    /// Largest normalized rounding error over every quantized sample.
    pub max_quantization_error: f64,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputImage {
    pub width: u32,
    pub height: u32,
    pub destination: Destination,
    pub intent: Intent,
    pub depth: Depth,
    /// Color channels per pixel, excluding alpha.
    pub channels: usize,
    pub has_alpha: bool,
    /// Interleaved samples: color channels, then alpha when present.
    pub samples: Vec<u16>,
    pub report: Report,
}
impl OutputImage {
    /// 8-bit samples as bytes, 16-bit samples as big-endian pairs.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self.depth {
            Depth::Eight => self.samples.iter().map(|v| *v as u8).collect(),
            Depth::Sixteen => self.samples.iter().flat_map(|v| v.to_be_bytes()).collect(),
        }
    }
    pub fn json(&self) -> Value {
        json!({
            "version":1,
            "scope":"production-output-transform",
            "width":self.width,
            "height":self.height,
            "destination":self.destination.json(),
            "intent":self.intent,
            "depth":self.depth.bits(),
            "channels":self.channels,
            "alpha":self.has_alpha,
            "pixels":self.report.pixels,
            "clipped_pixels":self.report.clipped_pixels,
            "quantized_samples":self.report.quantized_samples,
            "max_quantization_error":self.report.max_quantization_error,
            "profile":self.report.profile,
        })
    }
}

fn quantize(value: f64, depth: Depth, error: &mut f64) -> u16 {
    let max = depth.max();
    let clamped = value.clamp(0.0, 1.0);
    let sample = (clamped * max + 0.5).floor().min(max);
    *error = error.max((sample / max - clamped).abs());
    sample as u16
}

/// Convert a float working render to quantized delivery samples. This never
/// changes the document; unsupported destinations and alpha fail before work.
pub fn deliver(
    raw: &Value,
    document: &Path,
    render: &WorkingRender,
    options: &Options,
) -> Result<OutputImage> {
    let (destination, intent) = match (&options.destination, options.intent) {
        (Some(destination), Some(intent)) => (destination.clone(), intent),
        _ => {
            let role = roles::effective(raw, Role::Output)?;
            (
                options
                    .destination
                    .clone()
                    .unwrap_or_else(|| role.destination.clone()),
                options.intent.unwrap_or(role.intent),
            )
        }
    };
    ensure!(
        render.working_space.is_linear_rgb(),
        "[unsupported-capability] output transform requires a named linear RGB working space"
    );
    ensure!(
        destination.space() != "lab-d50",
        "[unsupported-capability] Lab delivery has no unambiguous integer encoding; deliver RGB, gray or CMYK"
    );
    ensure!(
        !destination.is_linear_rgb(),
        "[unsupported-capability] delivery destinations are encoded device values; select an encoded RGB, gray or CMYK destination"
    );
    let channels = destination.channel_count();
    let cmyk = destination.space() == "icc-cmyk";
    let (width, height) = render.surface.dimensions();
    let pixels = render.surface.pixels();
    let has_alpha = options.alpha == Alpha::Preserve && !cmyk;
    let stride = channels + usize::from(has_alpha);
    let total = u64::from(width) * u64::from(height) * stride as u64;
    ensure!(
        total * 2 <= crate::blend::LinearSurface::MAX_BYTES as u64,
        "[limit-exceeded] output transform exceeds the delivery sample budget; reduce canvas size or scale"
    );
    if cmyk && options.alpha == Alpha::Preserve {
        ensure!(
            pixels.iter().all(|p| p[3] == 1.0),
            "[invalid-input] CMYK delivery has no alpha plane; use alpha flatten to composite transparent pixels over paper white"
        );
    }
    let set = ProfileSet::load(raw, document, destination.profile())?;
    let mut engine = Evaluator::new(&set, intent)?;
    let mut samples = Vec::with_capacity(total as usize);
    let mut report = Report {
        pixels: u64::from(width) * u64::from(height),
        clipped_pixels: 0,
        quantized_samples: 0,
        max_quantization_error: 0.0,
        profile: destination.profile().map(str::to_owned),
    };
    let mut last: Option<([f64; 3], [f64; 4], bool)> = None;
    for (index, pixel) in pixels.iter().enumerate() {
        if index % (width as usize).max(1) == 0 {
            crate::composite::check_cancelled()?;
        }
        let alpha = pixel[3];
        let mut rgb = [pixel[0], pixel[1], pixel[2]];
        if options.alpha == Alpha::FlattenOnWhite {
            for channel in &mut rgb {
                *channel = *channel * alpha + (1.0 - alpha);
            }
        }
        let (device, clipped) = match last {
            Some((input, device, clipped)) if input == rgb => (device, clipped),
            _ => {
                let (device, clipped) =
                    engine.working_to_destination(rgb, &render.working_space, &destination)?;
                last = Some((rgb, device, clipped));
                (device, clipped)
            }
        };
        report.clipped_pixels += u64::from(clipped);
        for value in &device[..channels] {
            samples.push(quantize(
                *value,
                options.depth,
                &mut report.max_quantization_error,
            ));
        }
        if has_alpha {
            samples.push(quantize(
                alpha,
                options.depth,
                &mut report.max_quantization_error,
            ));
        }
    }
    report.quantized_samples = samples.len() as u64;
    Ok(OutputImage {
        width,
        height,
        destination,
        intent,
        depth: options.depth,
        channels,
        has_alpha,
        samples,
        report,
    })
}

/// Render a page on float surfaces and deliver it in one explicit step.
pub fn render(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    scale: f32,
    options: &Options,
) -> Result<OutputImage> {
    let rendered = crate::composite::render_working(raw, document, page, scale, true)?;
    deliver(raw, document, &rendered, options)
}

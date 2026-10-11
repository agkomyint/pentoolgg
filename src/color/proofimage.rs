//! Soft-proof image of a page: the delivered output CMYK read back through the
//! output profile to PCS and on to the display role, as a bounded sRGB-class PNG.
//! It is a numeric profile simulation like `color proof`, not a calibrated
//! monitor preview and not a substitute for a contract proof.
use super::{
    icc::Intent,
    output::{self, Alpha, Depth},
    pcs,
    roles::{self, Role},
    value::{Evaluator, NativeColor, ProfileSet},
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path};

/// Proof rasters are bounded; larger pages are rendered at a reduced scale.
pub const MAX_PIXELS: f64 = 1_000_000.0;

#[derive(Debug)]
pub struct Proof {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
    pub report: Value,
}

/// A pixel is flagged when the artwork's display color and its soft proof differ
/// by more than this many 8-bit steps in any channel.
pub const GAMUT_CHANNEL_THRESHOLD: i32 = 12;

pub fn render(raw: &Value, document: &Path, page_id: Option<&str>) -> Result<Proof> {
    render_with(raw, document, page_id, false)
}

/// Gamut warning image: the page in muted display colors with every pixel the
/// output profile cannot reproduce shown in red. It compares the artwork as
/// authored (display role) with its soft proof, so it is a numeric round-trip
/// check, not a contract proof.
pub fn gamut(raw: &Value, document: &Path, page_id: Option<&str>) -> Result<Proof> {
    render_with(raw, document, page_id, true)
}

fn render_with(raw: &Value, document: &Path, page_id: Option<&str>, gamut: bool) -> Result<Proof> {
    ensure!(
        raw["version"] == super::VERSION,
        "[unsupported-version] proof images require a version {} document",
        super::VERSION
    );
    let output_role = roles::effective(raw, Role::Output)?;
    ensure!(
        output_role.destination.space() == "icc-cmyk",
        "[unsupported-capability] proof images simulate an ICC CMYK output role, not {}",
        output_role.destination.space()
    );
    let reference = output_role
        .destination
        .profile()
        .context("[invalid-input] CMYK output role has no profile")?
        .to_owned();
    let display = roles::effective(raw, Role::Display)?;
    ensure!(
        display.destination.channel_count() == 3,
        "[unsupported-capability] the display role must be a three-channel RGB destination for a proof image"
    );
    let pages = raw["pages"].as_array().context("pages missing")?;
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|page| page["id"] == id)
            .with_context(|| format!("[missing-resource] page not found: {id}"))?,
        None => pages.first().context("document has no pages")?,
    };
    let area = page["canvas"]["width"].as_f64().unwrap_or(1.0).max(1.0)
        * page["canvas"]["height"].as_f64().unwrap_or(1.0).max(1.0);
    let scale = (MAX_PIXELS / area).sqrt().clamp(0.1, 1.0) as f32;
    let image = output::render(
        raw,
        document,
        page_id,
        scale,
        &output::Options {
            destination: Some(output_role.destination.clone()),
            intent: Some(output_role.intent),
            depth: Depth::Eight,
            alpha: Alpha::FlattenOnWhite,
        },
    )?;
    ensure!(image.channels == 4, "[internal] proof source is not CMYK");

    let source_set = ProfileSet::load(raw, document, [reference.as_str()])?;
    let mut source_engine = Evaluator::new(&source_set, output_role.intent)?;
    let display_set = ProfileSet::load(raw, document, display.destination.profile())?;
    let mut display_engine = Evaluator::new(&display_set, display.intent)?;

    let mut cache: HashMap<[u16; 4], [u8; 3]> = HashMap::new();
    let mut rgb = Vec::with_capacity(image.samples.len() / 4 * 3);
    let mut clipped = 0u64;
    for pixel in image.samples.chunks_exact(4) {
        let key = [pixel[0], pixel[1], pixel[2], pixel[3]];
        let value = match cache.get(&key) {
            Some(value) => *value,
            None => {
                crate::composite::check_cancelled()?;
                let channels: Vec<f64> = key.iter().map(|v| f64::from(*v) / 255.0).collect();
                let source = NativeColor::parse(&json!({
                    "space":"icc-cmyk","profile":reference,"channels":channels}))?;
                let (xyz, _) = source_engine.to_xyz(&source)?;
                let lab = pcs::xyz_to_lab(xyz)?;
                let appearance =
                    NativeColor::parse(&json!({"space":"lab-d50","channels":lab,"alpha":1.0}))?;
                let converted = display_engine.convert(&appearance, &display.destination)?;
                let device = converted.color.channels();
                if device.len() != 3 {
                    bail!(
                        "[internal] display conversion returned {} channels",
                        device.len()
                    );
                }
                let value = [
                    (device[0].clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8,
                    (device[1].clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8,
                    (device[2].clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8,
                ];
                if converted.clipped {
                    clipped += 1;
                }
                cache.insert(key, value);
                value
            }
        };
        rgb.extend_from_slice(&value);
    }
    let mut flagged = 0u64;
    if gamut {
        let source = output::render(
            raw,
            document,
            page_id,
            scale,
            &output::Options {
                destination: Some(display.destination.clone()),
                intent: Some(display.intent),
                depth: Depth::Eight,
                alpha: Alpha::FlattenOnWhite,
            },
        )?;
        ensure!(
            source.channels == 3 && source.samples.len() == rgb.len(),
            "[internal] gamut source is not an RGB raster of the proof size"
        );
        for (shown, authored) in rgb.chunks_exact_mut(3).zip(source.samples.chunks_exact(3)) {
            let outside = shown
                .iter()
                .zip(authored)
                .any(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() > GAMUT_CHANNEL_THRESHOLD);
            if outside {
                flagged += 1;
                shown.copy_from_slice(&[220, 38, 38]);
            } else {
                for (channel, value) in shown.iter_mut().zip(authored) {
                    *channel = (u32::from(*value) * 2 / 5 + 153) as u8;
                }
            }
        }
    }
    let buffer = image::RgbImage::from_raw(image.width, image.height, rgb)
        .context("[internal] proof buffer size")?;
    let mut png = Vec::new();
    buffer.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    let intent: Intent = output_role.intent;
    Ok(Proof {
        width: image.width,
        height: image.height,
        png,
        report: json!({
            "version":1,
            "scope":"soft-proof-image",
            "width":image.width,
            "height":image.height,
            "scale":scale,
            "unique_colors":cache.len(),
            "clipped_colors":clipped,
            "output_profile":reference,
            "intent":intent,
            "display":display.destination.json(),
            "method":"output-profile-device-to-pcs-to-display",
            "gamut_warning_pixels": gamut.then_some(flagged),
            "gamut_warning_method": gamut.then_some("display-vs-soft-proof-round-trip-difference"),
            "gamut_warning_channel_threshold": gamut.then_some(GAMUT_CHANNEL_THRESHOLD),
            "paper_simulation":false,
            "spectral_paper_or_ink_model":false,
        }),
    })
}

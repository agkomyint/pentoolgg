//! The develop pipeline of a raw source through stage 10: decode (stage 1), the
//! camera profile and calibration (stage 3), lens and geometry (stages 4–5) as
//! one warp, then tone, presence, curves, color and effects (stages 6–10).
//! Later stages are added by later items of the photography milestone.
use super::adjust;
use super::detail;
use super::dng::Dng;
use super::dngout;
use super::lens::LensProfile;
use super::local;
use super::pixels::Working;
use super::warp::{self, Correction, Crop, LensChain, Mapping};
use super::{profile, raw};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// Resolves a profile digest to its stored bytes and catalog record.
pub type Profiles<'a> = &'a dyn Fn(&str) -> Result<(Vec<u8>, Value)>;

/// The lens correction a develop object's `lens.profile` selects.
pub fn lens_chain(
    dng: &Dng,
    develop: &Value,
    width: usize,
    height: usize,
    profiles: Profiles,
) -> Result<LensChain> {
    let lens = &develop["lens"];
    let correction = match lens.get("profile") {
        None => Correction::None,
        Some(Value::String(s)) if s == "none" => Correction::None,
        Some(Value::String(s)) if s == "embedded-opcodes" => {
            let (active, crop) = (&dng.active, &dng.crop);
            Correction::Opcodes {
                opcodes: &dng.opcodes,
                stored: [
                    -((active.left + crop.left) as f64),
                    -((active.top + crop.top) as f64),
                    dng.width as f64,
                    dng.height as f64,
                ],
                active: [
                    -(crop.left as f64),
                    -(crop.top as f64),
                    active.width() as f64,
                    active.height() as f64,
                ],
            }
        }
        Some(Value::Object(o)) => {
            let digest = o.get("profile").and_then(Value::as_str).unwrap_or_default();
            let (bytes, record) = profiles(digest)?;
            if record["kind"] != "lens" {
                bail!("[invalid-develop] lens.profile {digest} is a {} profile; choose a lens profile", record["kind"])
            }
            let profile = LensProfile::parse(&bytes).with_context(|| format!("lens profile {digest}"))?;
            let capture = dng.capture();
            let focal = capture["focal_length"].as_f64();
            let aperture = capture["aperture"].as_f64();
            let (Some(focal), Some(aperture)) = (focal, aperture) else {
                bail!("[invalid-develop] lens profile {digest} needs the source's focal length and aperture, and the DNG records {}; use lens.profile none and manual corrections", if focal.is_none() { "no focal length" } else { "no aperture" })
            };
            Correction::Profile(profile.at(focal, aperture))
        }
        Some(other) => bail!("[invalid-develop] lens.profile {other} must be none, embedded-opcodes or {{\"profile\": digest}}"),
    };
    Ok(LensChain::new(lens, width, height, correction))
}

/// The developed (default-cropped, unoriented) size of a DNG.
pub fn decoded_size(dng: &Dng) -> (usize, usize) {
    (dng.crop.width(), dng.crop.height())
}

/// The mapping, crop and report of a development, computed without decoding.
pub struct Plan {
    pub mapping: Mapping,
    pub crop: Crop,
    pub report: Value,
}

pub fn plan(dng: &Dng, develop: &Value, profiles: Profiles) -> Result<Plan> {
    let (width, height) = decoded_size(dng);
    let lens = lens_chain(dng, develop, width, height, profiles)?;
    let mapping = Mapping::new(width, height, dng.orientation, &develop["geometry"], lens)?;
    let (crop, crop_report) = warp::resolve_crop(&develop["crop"], &mapping)?;
    let report = json!({
        "decoded": [width, height],
        "oriented": [mapping.frame.0, mapping.frame.1],
        "output": [crop[2] - crop[0], crop[3] - crop[1]],
        "orientation": dng.orientation,
        "crop": crop_report,
        "lens": mapping.lens.applied,
        "geometry": develop.get("geometry").cloned().unwrap_or(json!({})),
    });
    Ok(Plan {
        mapping,
        crop,
        report,
    })
}

/// Stages 1–3: decoded camera RGB, early detail, transformed to the working space, at the
/// decoded size, with the resolved white.
pub fn decode_working(
    dng: &Dng,
    develop: &Value,
    profiles: Profiles,
) -> Result<(Vec<f32>, profile::White)> {
    decode_levels(dng, develop, profiles).map(|(rgb, _, white)| (rgb, white))
}

/// `decode_working` plus the clip map: each pixel's pre-balance raw level
/// (`raw::CameraRgb::level`), which merges read.
pub fn decode_levels(
    dng: &Dng,
    develop: &Value,
    profiles: Profiles,
) -> Result<(Vec<f32>, Vec<f32>, profile::White)> {
    let load = |digest: &str| profiles(digest).map(|(bytes, _)| bytes);
    let spec = profile::select(develop["raw"].get("camera_profile"), dng, &load)?;
    let (white, mut transform) = profile::resolve(&spec, dng, develop.get("white_balance"))?;
    transform.calibrate(develop);
    let decoded = raw::decode_with(
        dng,
        &profile::decode_options(develop, white.neutral),
        &detail::Defects::new(develop),
    )?;
    let mut rgb = decoded.rgb;
    if let Some(early) = detail::Early::new(develop) {
        early.apply(&mut rgb, decoded.width, decoded.height, &transform.matrix)?;
    }
    for pixel in rgb.chunks_exact_mut(3) {
        let out = transform.apply([pixel[0], pixel[1], pixel[2]]);
        pixel.copy_from_slice(&out);
    }
    super::check_cancelled()?;
    Ok((rgb, decoded.level, white))
}

/// A development through stage 10.
pub struct Developed {
    pub image: Working,
    pub invalid_pixels: u64,
    pub report: Value,
    /// The camera profile's stage-11 look table and tone curve.
    pub rendering: profile::Rendering,
}

/// The stage-11 rendering of the variant's camera profile.
pub fn rendering(dng: &Dng, develop: &Value, profiles: Profiles) -> Result<profile::Rendering> {
    let load = |digest: &str| profiles(digest).map(|(bytes, _)| bytes);
    Ok(profile::select(develop["raw"].get("camera_profile"), dng, &load)?.rendering)
}

/// The context of stages 6–10 for a planned development.
pub fn context(dng: &Dng, plan: &Plan) -> adjust::Context {
    adjust::Context {
        baseline_exposure: dng.baseline_exposure.unwrap_or(0.0),
        long_edge: plan.mapping.frame.0.max(plan.mapping.frame.1) as f64,
        origin: (plan.crop[0], plan.crop[1]),
        frame: plan.mapping.frame,
    }
}

/// Develop a variant. `masks` reads brush tiles and mask resources; without
/// it, a local adjustment that needs them is refused.
pub fn develop(
    dng: &Dng,
    develop: &Value,
    profiles: Profiles,
    masks: Option<&local::Lookup>,
) -> Result<Developed> {
    let plan = plan(dng, develop, profiles)?;
    let rendering = rendering(dng, develop, profiles)?;
    // Refuse an unresolved dehaze and load masks before any decoding work.
    adjust::airlight(develop)?;
    let mut local = local::Local::new(develop, masks)?;
    let (rgb, white) = decode_working(dng, develop, profiles)?;
    let (mut image, invalid_pixels) = warp::render(&rgb, &plan.mapping, plan.crop)?;
    drop(rgb);
    // A merge output's transparency mask follows the same warp.
    if let Some(mask) = dngout::transparency(dng)? {
        let mask: Vec<f32> = mask.iter().flat_map(|a| [*a; 3]).collect();
        let (warped, _) = warp::render(&mask, &plan.mapping, plan.crop)?;
        let coverage = warped.rgb.chunks_exact(3).map(|p| p[0].clamp(0.0, 1.0));
        image.alpha = Some(match image.alpha.take() {
            Some(alpha) => alpha.iter().zip(coverage).map(|(a, c)| a * c).collect(),
            None => coverage.collect(),
        });
    }
    adjust::apply_with(&mut image, develop, &context(dng, &plan), local.as_mut())?;
    let mut report = plan.report;
    if let Some(local) = &local {
        report["local"] = local.report();
    }
    report["white"] = white.report();
    report["invalid_pixels"] = json!(invalid_pixels);
    Ok(Developed {
        image,
        invalid_pixels,
        report,
        rendering,
    })
}

/// Stages 1–5 of the whole (uncropped) frame, box-averaged to the analysis
/// proxy that auto tone and dehaze read.
pub fn analysis_proxy(dng: &Dng, develop: &Value, profiles: Profiles) -> Result<Working> {
    let mut plan = plan(dng, develop, profiles)?;
    plan.crop = [0, 0, plan.mapping.frame.0, plan.mapping.frame.1];
    let (rgb, _) = decode_working(dng, develop, profiles)?;
    let (image, _) = warp::render(&rgb, &plan.mapping, plan.crop)?;
    drop(rgb);
    adjust::proxy(&image, adjust::ANALYSIS_EDGE)
}

/// Auto tone (`auto-tone` version 1): the `tone` group with `exposure`,
/// `highlights` and `shadows` resolved and `auto` recording the algorithm.
/// The other tone sliders are kept.
pub fn auto_tone(dng: &Dng, develop: &Value, profiles: Profiles) -> Result<Value> {
    let proxy = analysis_proxy(dng, develop, profiles)?;
    let [exposure, highlights, shadows] =
        adjust::auto_tone(&proxy, dng.baseline_exposure.unwrap_or(0.0))?;
    let mut tone = develop.get("tone").cloned().unwrap_or(json!({}));
    tone["exposure"] = number(exposure);
    for (key, v) in [("highlights", highlights), ("shadows", shadows)] {
        match tone.as_object_mut() {
            Some(object) if v == 0.0 => {
                object.remove(key);
            }
            _ => tone[key] = number(v),
        }
    }
    tone["auto"] = json!({"algorithm": "auto-tone", "version": 1});
    Ok(tone)
}

/// The dehaze airlight (algorithm version 1) of the tone-adjusted proxy.
pub fn resolve_airlight(dng: &Dng, develop: &Value, profiles: Profiles) -> Result<Value> {
    let mut proxy = analysis_proxy(dng, develop, profiles)?;
    let context = adjust::Context {
        baseline_exposure: dng.baseline_exposure.unwrap_or(0.0),
        long_edge: proxy.width.max(proxy.height) as f64,
        origin: (0, 0),
        frame: (proxy.width as usize, proxy.height as usize),
    };
    adjust::tone(&mut proxy, develop, &context)?;
    let airlight = adjust::estimate_airlight(&proxy)?;
    Ok(Value::Array(airlight.into_iter().map(number).collect()))
}

fn number(v: f64) -> Value {
    super::dng::number(v)
}

/// Run an upright analysis and return the geometry keys it sets. `mode` is
/// `level`, `vertical`, `full` or `guided`; `guides` are oriented-frame
/// points for `guided`.
pub fn upright(
    dng: &Dng,
    develop: &Value,
    mode: &str,
    guides: Option<&Value>,
    profiles: Profiles,
) -> Result<Value> {
    let (width, height) = decoded_size(dng);
    let frame = warp::oriented(dng.orientation, width, height);
    let geometry = develop.get("geometry").cloned().unwrap_or(json!({}));
    let segments = if mode == "guided" {
        let guides = guides
            .or_else(|| geometry.get("guides"))
            .cloned()
            .unwrap_or(json!([]));
        let segments = warp::guide_segments(&guides, frame);
        if segments.is_empty() {
            bail!("[invalid-input] upright guided needs at least one --guide x1,y1,x2,y2")
        }
        segments
    } else {
        let lens = lens_chain(dng, develop, width, height, profiles)?;
        let mapping = Mapping::new(width, height, dng.orientation, &json!({}), lens)?;
        let (rgb, _) = decode_working(dng, develop, profiles)?;
        let (image, _) = warp::render(&rgb, &mapping, [0, 0, frame.0, frame.1])?;
        drop(rgb);
        let (lum, aw, ah) = warp::analysis_luminance(&image, profile::ANALYSIS_EDGE);
        let segments = warp::detect_lines(&lum, aw, ah, frame);
        if segments.is_empty() {
            bail!("[invalid-input] upright {mode} found no straight lines to correct; use upright guided with --guide segments")
        }
        segments
    };
    let keys = warp::solved_keys(mode, &segments);
    let mut solved = warp::solve(&segments, &keys, &geometry, frame);
    solved["segments"] = json!(segments.len());
    Ok(solved)
}

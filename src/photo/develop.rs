//! Validation of a variant's `develop` settings (`[invalid-develop]`).
//!
//! Each roadmap item validates the groups it implements; `local` is validated by
//! `super::local`.
use anyhow::{bail, Result};
use serde_json::{Map, Value};

/// What a develop object is validated against.
pub struct Source<'a> {
    /// `raw`, `derived` or `rendered`.
    pub kind: &'a str,
    /// The source's `raw.unique_camera_model`, for raw and derived sources.
    pub camera_model: Option<&'a str>,
    /// `photography.profiles`.
    pub profiles: &'a Map<String, Value>,
    /// The document, when local masks must resolve against its resources.
    pub document: Option<&'a Value>,
}

impl Source<'_> {
    fn is_raw(&self) -> bool {
        self.kind != "rendered"
    }
}

const GROUPS: [&str; 16] = [
    "process",
    "raw",
    "white_balance",
    "tone",
    "presence",
    "curves",
    "hsl",
    "grading",
    "monochrome",
    "detail",
    "lens",
    "geometry",
    "crop",
    "effects",
    "calibration",
    "local",
];

pub(super) fn group<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>> {
    match value.as_object() {
        Some(object) => Ok(object),
        None => bail!("[invalid-develop] {what} must be an object"),
    }
}

pub(super) fn allowed(object: &Map<String, Value>, keys: &[&str], what: &str) -> Result<()> {
    if let Some(key) = object.keys().find(|key| !keys.contains(&key.as_str())) {
        bail!(
            "[invalid-develop] {what} has unknown parameter {key:?}; expected one of {}",
            keys.join(", ")
        )
    }
    Ok(())
}

pub(super) fn number(
    object: &Map<String, Value>,
    key: &str,
    (low, high): (f64, f64),
    what: &str,
) -> Result<Option<f64>> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => match value.as_f64() {
            Some(v) if (low..=high).contains(&v) => Ok(Some(v)),
            _ => bail!(
                "[invalid-develop] {what}.{key} must be a number in {low}–{high}; got {value}"
            ),
        },
    }
}

fn required(object: &Map<String, Value>, key: &str, range: (f64, f64), what: &str) -> Result<f64> {
    match number(object, key, range, what)? {
        Some(v) => Ok(v),
        None => bail!("[invalid-develop] {what}.{key} is required"),
    }
}

fn choice(object: &Map<String, Value>, key: &str, choices: &[&str], what: &str) -> Result<()> {
    match object.get(key) {
        None => Ok(()),
        Some(Value::String(s)) if choices.contains(&s.as_str()) => Ok(()),
        Some(value) => bail!(
            "[invalid-develop] {what}.{key} must be one of {}; got {value}",
            choices.join(", ")
        ),
    }
}

pub(super) fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Validate a develop object. `what` names the variant or snapshot.
pub fn validate(develop: &Map<String, Value>, source: &Source, what: &str) -> Result<()> {
    if let Some(key) = develop.keys().find(|key| !GROUPS.contains(&key.as_str())) {
        bail!("[invalid-develop] {what} develop has unknown group {key:?}")
    }
    for (key, value) in develop {
        if key != "process" && key != "local" && !value.is_object() {
            bail!("[invalid-develop] {what} develop.{key} must be an object")
        }
    }
    if let Some(raw) = develop.get("raw") {
        validate_raw(raw, source, &format!("{what} raw"))?;
    }
    if let Some(white_balance) = develop.get("white_balance") {
        validate_white_balance(white_balance, source, &format!("{what} white_balance"))?;
    }
    if let Some(lens) = develop.get("lens") {
        validate_lens(lens, source, &format!("{what} lens"))?;
    }
    if let Some(geometry) = develop.get("geometry") {
        validate_geometry(geometry, &format!("{what} geometry"))?;
    }
    if let Some(crop) = develop.get("crop") {
        validate_crop(crop, &format!("{what} crop"))?;
    }
    validate_development(develop, what)?;
    if let Some(local) = develop.get("local") {
        super::local::validate(local, source.document, what)?;
    }
    Ok(())
}

const SIGNED: (f64, f64) = (-100.0, 100.0);
const PERCENT: (f64, f64) = (0.0, 100.0);

/// Every key of `object` is one of `keys` and a number in `range`.
fn numbers(
    object: &Map<String, Value>,
    keys: &[&str],
    range: (f64, f64),
    what: &str,
) -> Result<()> {
    allowed(object, keys, what)?;
    for key in keys {
        number(object, key, range, what)?;
    }
    Ok(())
}

fn bands(value: &Value, what: &str) -> Result<()> {
    numbers(group(value, what)?, &super::adjust::BANDS, SIGNED, what)
}

/// The development stack groups (stages 6–10 and calibration).
fn validate_development(develop: &Map<String, Value>, what: &str) -> Result<()> {
    if let Some(tone) = develop.get("tone") {
        let what = format!("{what} tone");
        let tone = group(tone, &what)?;
        allowed(
            tone,
            &[
                "exposure",
                "contrast",
                "highlights",
                "shadows",
                "whites",
                "blacks",
                "auto",
            ],
            &what,
        )?;
        number(tone, "exposure", (-5.0, 5.0), &what)?;
        for key in ["contrast", "highlights", "shadows", "whites", "blacks"] {
            number(tone, key, SIGNED, &what)?;
        }
        validate_auto(tone, &what)?;
    }
    if let Some(presence) = develop.get("presence") {
        let what = format!("{what} presence");
        let presence = group(presence, &what)?;
        let sliders = ["texture", "clarity", "dehaze", "vibrance", "saturation"];
        allowed(
            presence,
            &[&sliders[..], &["dehaze_airlight"]].concat(),
            &what,
        )?;
        for key in sliders {
            number(presence, key, SIGNED, &what)?;
        }
        if let Some(airlight) = presence.get("dehaze_airlight") {
            let valid = airlight.as_array().is_some_and(|a| {
                a.len() == 3
                    && a.iter()
                        .all(|v| v.as_f64().is_some_and(|v| (0.0..=1.0e6).contains(&v)))
            });
            if !valid {
                bail!("[invalid-develop] {what}.dehaze_airlight must hold three non-negative numbers; got {airlight}")
            }
        }
    }
    if let Some(curves) = develop.get("curves") {
        validate_curves(curves, &format!("{what} curves"))?;
    }
    if let Some(hsl) = develop.get("hsl") {
        let what = format!("{what} hsl");
        let hsl = group(hsl, &what)?;
        allowed(hsl, &["hue", "saturation", "luminance"], &what)?;
        for (key, value) in hsl {
            bands(value, &format!("{what}.{key}"))?;
        }
    }
    if let Some(grading) = develop.get("grading") {
        let what = format!("{what} grading");
        let grading = group(grading, &what)?;
        let wheels = ["shadows", "midtones", "highlights", "global"];
        allowed(
            grading,
            &[&wheels[..], &["blending", "balance"]].concat(),
            &what,
        )?;
        number(grading, "blending", PERCENT, &what)?;
        number(grading, "balance", SIGNED, &what)?;
        for key in wheels {
            if let Some(wheel) = grading.get(key) {
                let where_ = format!("{what}.{key}");
                let wheel = group(wheel, &where_)?;
                allowed(wheel, &["hue", "saturation", "luminance"], &where_)?;
                number(wheel, "hue", (0.0, 360.0), &where_)?;
                number(wheel, "saturation", PERCENT, &where_)?;
                number(wheel, "luminance", SIGNED, &where_)?;
            }
        }
    }
    if let Some(monochrome) = develop.get("monochrome") {
        let what = format!("{what} monochrome");
        let monochrome = group(monochrome, &what)?;
        allowed(monochrome, &["enabled", "mix"], &what)?;
        boolean(monochrome, "enabled", &what)?;
        if let Some(mix) = monochrome.get("mix") {
            bands(mix, &format!("{what}.mix"))?;
        }
    }
    if let Some(effects) = develop.get("effects") {
        let what = format!("{what} effects");
        let effects = group(effects, &what)?;
        allowed(effects, &["vignette", "grain"], &what)?;
        if let Some(vignette) = effects.get("vignette") {
            let where_ = format!("{what}.vignette");
            let vignette = group(vignette, &where_)?;
            allowed(
                vignette,
                &["amount", "midpoint", "roundness", "feather", "highlights"],
                &where_,
            )?;
            number(vignette, "amount", SIGNED, &where_)?;
            number(vignette, "roundness", SIGNED, &where_)?;
            for key in ["midpoint", "feather", "highlights"] {
                number(vignette, key, PERCENT, &where_)?;
            }
        }
        if let Some(grain) = effects.get("grain") {
            let where_ = format!("{what}.grain");
            let grain = group(grain, &where_)?;
            allowed(grain, &["amount", "size", "roughness", "seed"], &where_)?;
            for key in ["amount", "size", "roughness"] {
                number(grain, key, PERCENT, &where_)?;
            }
            match grain.get("seed") {
                Some(seed) if seed.as_u64().is_some_and(|s| s <= u64::from(u32::MAX)) => {}
                Some(seed) => bail!("[invalid-develop] {where_}.seed must be an integer in 0–4294967295; got {seed}"),
                None => bail!("[invalid-develop] {where_}.seed is required so the grain is reproducible"),
            }
        }
    }
    if let Some(detail) = develop.get("detail") {
        let what = format!("{what} detail");
        let detail = group(detail, &what)?;
        allowed(detail, &["sharpening", "noise", "moire"], &what)?;
        number(detail, "moire", PERCENT, &what)?;
        if let Some(sharpening) = detail.get("sharpening") {
            let where_ = format!("{what}.sharpening");
            let sharpening = group(sharpening, &where_)?;
            allowed(
                sharpening,
                &["amount", "radius", "detail", "masking"],
                &where_,
            )?;
            number(sharpening, "amount", (0.0, 150.0), &where_)?;
            number(sharpening, "radius", (0.5, 3.0), &where_)?;
            number(sharpening, "detail", PERCENT, &where_)?;
            number(sharpening, "masking", PERCENT, &where_)?;
        }
        if let Some(noise) = detail.get("noise") {
            let where_ = format!("{what}.noise");
            numbers(
                group(noise, &where_)?,
                &super::detail::NOISE,
                PERCENT,
                &where_,
            )?;
        }
    }
    if let Some(calibration) = develop.get("calibration") {
        let what = format!("{what} calibration");
        let calibration = group(calibration, &what)?;
        allowed(
            calibration,
            &["shadows_tint", "red", "green", "blue"],
            &what,
        )?;
        number(calibration, "shadows_tint", SIGNED, &what)?;
        for key in ["red", "green", "blue"] {
            if let Some(primary) = calibration.get(key) {
                let where_ = format!("{what}.{key}");
                numbers(
                    group(primary, &where_)?,
                    &["hue", "saturation"],
                    SIGNED,
                    &where_,
                )?;
            }
        }
    }
    Ok(())
}

fn validate_curves(curves: &Value, what: &str) -> Result<()> {
    let curves = group(curves, what)?;
    allowed(curves, &["parametric", "point"], what)?;
    if let Some(parametric) = curves.get("parametric") {
        let where_ = format!("{what}.parametric");
        let parametric = group(parametric, &where_)?;
        let sliders = ["highlights", "lights", "darks", "shadows"];
        allowed(parametric, &[&sliders[..], &["splits"]].concat(), &where_)?;
        for key in sliders {
            number(parametric, key, SIGNED, &where_)?;
        }
        if let Some(splits) = parametric.get("splits") {
            let v: Vec<f64> = splits
                .as_array()
                .map(|s| s.iter().filter_map(Value::as_f64).collect())
                .unwrap_or_default();
            let valid = splits.as_array().is_some_and(|s| s.len() == 3)
                && v.len() == 3
                && v.iter().all(|s| (0.05..=0.95).contains(s))
                && v[0] < v[1]
                && v[1] < v[2];
            if !valid {
                bail!("[invalid-develop] {where_}.splits must be three increasing numbers in 0.05–0.95; got {splits}")
            }
        }
    }
    if let Some(point) = curves.get("point") {
        let where_ = format!("{what}.point");
        let point = group(point, &where_)?;
        allowed(point, &["rgb", "red", "green", "blue"], &where_)?;
        for (key, points) in point {
            let valid = points.as_array().is_some_and(|points| {
                (2..=16).contains(&points.len())
                    && points.iter().all(|p| pair(p, (0.0, 1.0)))
                    && points
                        .windows(2)
                        .all(|w| w[0][0].as_f64() < w[1][0].as_f64())
            });
            if !valid {
                bail!("[invalid-develop] {where_}.{key} must hold 2–16 [input, output] points in 0–1 with strictly increasing inputs; got {points}")
            }
        }
    }
    Ok(())
}

pub(super) fn boolean(object: &Map<String, Value>, key: &str, what: &str) -> Result<()> {
    match object.get(key) {
        None | Some(Value::Bool(_)) => Ok(()),
        Some(value) => bail!("[invalid-develop] {what}.{key} must be true or false; got {value}"),
    }
}

/// A two-number array with each value in `range`.
fn pair(value: &Value, range: (f64, f64)) -> bool {
    value.as_array().is_some_and(|v| {
        v.len() == 2
            && v.iter()
                .all(|v| v.as_f64().is_some_and(|v| (range.0..=range.1).contains(&v)))
    })
}

fn validate_lens(lens: &Value, source: &Source, what: &str) -> Result<()> {
    let lens = group(lens, what)?;
    allowed(
        lens,
        &[
            "profile",
            "distortion",
            "vignetting",
            "chromatic_aberration",
            "defringe",
        ],
        what,
    )?;
    match lens.get("profile") {
        None => {}
        Some(Value::String(s)) if s == "none" => {}
        Some(Value::String(s)) if s == "embedded-opcodes" => {
            if !source.is_raw() {
                bail!("[invalid-develop] {what}.profile embedded-opcodes applies only to raw sources; use none or a lens profile")
            }
        }
        Some(Value::Object(o))
            if o.len() == 1 && o.get("profile").and_then(Value::as_str).is_some_and(digest) =>
        {
            let key = o["profile"].as_str().unwrap();
            let Some(profile) = source.profiles.get(key) else {
                bail!("[missing-resource] {what}.profile references profile {key}, which is not in photography.profiles; add it with `photo profile add --lens` or `photo profile import-lcp`")
            };
            if profile.get("kind").and_then(Value::as_str) != Some("lens") {
                bail!("[invalid-develop] {what}.profile {key} is not a lens profile")
            }
        }
        Some(value) => bail!("[invalid-develop] {what}.profile must be \"none\", \"embedded-opcodes\" or {{\"profile\": digest}}; got {value}"),
    }
    number(lens, "distortion", (-100.0, 100.0), what)?;
    if let Some(vignetting) = lens.get("vignetting") {
        let where_ = format!("{what}.vignetting");
        let vignetting = group(vignetting, &where_)?;
        allowed(vignetting, &["amount", "midpoint"], &where_)?;
        number(vignetting, "amount", (-100.0, 100.0), &where_)?;
        number(vignetting, "midpoint", (0.0, 100.0), &where_)?;
    }
    if let Some(ca) = lens.get("chromatic_aberration") {
        let where_ = format!("{what}.chromatic_aberration");
        let ca = group(ca, &where_)?;
        allowed(ca, &["remove", "red_cyan", "blue_yellow"], &where_)?;
        boolean(ca, "remove", &where_)?;
        number(ca, "red_cyan", (-100.0, 100.0), &where_)?;
        number(ca, "blue_yellow", (-100.0, 100.0), &where_)?;
    }
    if let Some(defringe) = lens.get("defringe") {
        let where_ = format!("{what}.defringe");
        let defringe = group(defringe, &where_)?;
        allowed(
            defringe,
            &["purple_amount", "purple_hue", "green_amount", "green_hue"],
            &where_,
        )?;
        number(defringe, "purple_amount", (0.0, 20.0), &where_)?;
        number(defringe, "green_amount", (0.0, 20.0), &where_)?;
        for (key, range) in [("purple_hue", (30.0, 70.0)), ("green_hue", (40.0, 60.0))] {
            if let Some(hue) = defringe.get(key) {
                let ordered = hue
                    .as_array()
                    .is_some_and(|h| h.len() == 2 && h[0].as_f64() <= h[1].as_f64());
                if !pair(hue, range) || !ordered {
                    bail!(
                        "[invalid-develop] {where_}.{key} must be [low, high] with {}–{} and low ≤ high; got {hue}",
                        range.0,
                        range.1
                    )
                }
            }
        }
    }
    Ok(())
}

fn validate_geometry(geometry: &Value, what: &str) -> Result<()> {
    let geometry = group(geometry, what)?;
    allowed(
        geometry,
        &[
            "upright",
            "guides",
            "vertical",
            "horizontal",
            "rotate",
            "aspect",
            "scale",
            "offset",
            "auto",
        ],
        what,
    )?;
    choice(
        geometry,
        "upright",
        &["off", "level", "vertical", "full", "guided"],
        what,
    )?;
    let upright = geometry
        .get("upright")
        .and_then(Value::as_str)
        .unwrap_or("off");
    if let Some(guides) = geometry.get("guides") {
        if upright != "guided" {
            bail!("[invalid-develop] {what}.guides applies only to upright guided")
        }
        let valid = guides.as_array().is_some_and(|guides| {
            guides.len() <= 4
                && guides.iter().all(|guide| {
                    guide.as_array().is_some_and(|points| {
                        points.len() == 2
                            && points.iter().all(|p| pair(p, (0.0, 1.0)))
                            && points[0] != points[1]
                    })
                })
        });
        if !valid {
            bail!("[invalid-develop] {what}.guides must hold at most 4 segments of two distinct [x, y] points in 0–1")
        }
    }
    if geometry.contains_key("auto") && upright == "off" {
        bail!("[invalid-develop] {what}.auto records an upright analysis and needs upright level, vertical, full or guided")
    }
    validate_auto(geometry, what)?;
    number(geometry, "vertical", (-100.0, 100.0), what)?;
    number(geometry, "horizontal", (-100.0, 100.0), what)?;
    number(geometry, "rotate", (-45.0, 45.0), what)?;
    number(geometry, "aspect", (-100.0, 100.0), what)?;
    number(geometry, "scale", (50.0, 150.0), what)?;
    if let Some(offset) = geometry.get("offset") {
        if !pair(offset, (-100.0, 100.0)) {
            bail!("[invalid-develop] {what}.offset must be [x, y] with each in -100–100; got {offset}")
        }
    }
    Ok(())
}

/// A crop aspect: `free`, `original` or `W:H` with 1–9999 on each side.
pub fn crop_aspect(value: &str) -> Option<Option<(u32, u32)>> {
    match value {
        "free" | "original" => Some(None),
        _ => {
            let (w, h) = value.split_once(':')?;
            let side = |s: &str| {
                (!s.is_empty() && s.len() <= 4 && !s.starts_with('0'))
                    .then(|| s.parse::<u32>().ok())
                    .flatten()
            };
            Some(Some((side(w)?, side(h)?)))
        }
    }
}

fn validate_crop(crop: &Value, what: &str) -> Result<()> {
    let crop = group(crop, what)?;
    allowed(crop, &["rect", "aspect", "constrain"], what)?;
    boolean(crop, "constrain", what)?;
    if let Some(rect) = crop.get("rect") {
        let valid = rect.as_array().is_some_and(|r| {
            let v: Vec<f64> = r.iter().filter_map(Value::as_f64).collect();
            r.len() == 4
                && v.len() == 4
                && (0.0..=1.0).contains(&v[0])
                && (0.0..=1.0).contains(&v[1])
                && v[2] > 0.0
                && v[3] > 0.0
                && v[0] + v[2] <= 1.0 + 1e-9
                && v[1] + v[3] <= 1.0 + 1e-9
        });
        if !valid {
            bail!("[invalid-develop] {what}.rect must be [x, y, w, h] in 0–1 with w, h > 0 and the rectangle inside the frame; got {rect}")
        }
    }
    if let Some(aspect) = crop.get("aspect") {
        if aspect.as_str().and_then(crop_aspect).is_none() {
            bail!("[invalid-develop] {what}.aspect must be free, original or W:H (1–9999 each); got {aspect}")
        }
    }
    Ok(())
}

fn validate_raw(raw: &Value, source: &Source, what: &str) -> Result<()> {
    let raw = group(raw, what)?;
    if !source.is_raw() {
        bail!("[invalid-develop] {what} applies only to raw sources; remove it from this rendered source")
    }
    allowed(
        raw,
        &[
            "demosaic",
            "highlights",
            "camera_profile",
            "defective_pixels",
        ],
        what,
    )?;
    choice(raw, "demosaic", &["bilinear", "mhc"], what)?;
    choice(raw, "highlights", &["clip", "blend"], what)?;
    match raw.get("camera_profile") {
        None => {}
        Some(Value::String(s)) if s == "embedded" || s == "matrix-only" => {}
        Some(Value::Object(o))
            if o.len() == 1 && o.get("profile").and_then(Value::as_str).is_some_and(digest) =>
        {
            let key = o["profile"].as_str().unwrap();
            let Some(profile) = source.profiles.get(key) else {
                bail!("[missing-resource] {what}.camera_profile references profile {key}, which is not in photography.profiles; add it with `photo profile add`")
            };
            if profile.get("kind").and_then(Value::as_str) != Some("camera") {
                bail!("[invalid-develop] {what}.camera_profile {key} is not a camera profile")
            }
            let model = profile.get("unique_camera_model").and_then(Value::as_str);
            let forced = profile.get("force_model") == Some(&Value::Bool(true));
            if !forced && model != source.camera_model {
                bail!(
                    "[invalid-develop] {what}.camera_profile {key} is for camera {:?} but the source is {:?}; use a matching profile or re-add it with `photo profile add --force-model`",
                    model.unwrap_or_default(),
                    source.camera_model.unwrap_or_default()
                )
            }
        }
        Some(value) => bail!("[invalid-develop] {what}.camera_profile must be \"embedded\", \"matrix-only\" or {{\"profile\": digest}}; got {value}"),
    }
    if let Some(defective) = raw.get("defective_pixels") {
        let where_ = format!("{what}.defective_pixels");
        let defective = group(defective, &where_)?;
        allowed(defective, &["auto", "threshold", "list"], &where_)?;
        if defective.get("auto").is_some_and(|v| !v.is_boolean()) {
            bail!("[invalid-develop] {where_}.auto must be true or false")
        }
        number(defective, "threshold", (1.0, 100.0), &where_)?;
        if let Some(list) = defective.get("list") {
            let valid = list.as_array().is_some_and(|list| {
                list.len() <= 4096
                    && list.iter().all(|point| {
                        point
                            .as_array()
                            .is_some_and(|p| p.len() == 2 && p.iter().all(|c| c.as_u64().is_some()))
                    })
            });
            if !valid {
                bail!("[invalid-develop] {where_}.list must hold at most 4096 [x, y] pairs of non-negative integers")
            }
        }
    }
    Ok(())
}

fn validate_auto(object: &Map<String, Value>, what: &str) -> Result<()> {
    let Some(auto) = object.get("auto") else {
        return Ok(());
    };
    let valid = auto.as_object().is_some_and(|auto| {
        auto.len() == 2
            && auto
                .get("algorithm")
                .and_then(Value::as_str)
                .is_some_and(|a| !a.is_empty() && a.chars().count() <= 64)
            && auto
                .get("version")
                .and_then(Value::as_u64)
                .is_some_and(|v| v >= 1)
    });
    if !valid {
        bail!("[invalid-develop] {what}.auto must be {{\"algorithm\": name, \"version\": n ≥ 1}}")
    }
    Ok(())
}

fn validate_white_balance(white_balance: &Value, source: &Source, what: &str) -> Result<()> {
    let wb = group(white_balance, what)?;
    match wb.get("mode").and_then(Value::as_str) {
        Some("as-shot") => allowed(wb, &["mode"], what),
        Some("temperature") => {
            if !source.is_raw() {
                bail!("[invalid-develop] {what} mode temperature (kelvin) applies only to raw sources; use mode relative for a rendered source")
            }
            allowed(wb, &["mode", "temperature", "tint", "auto"], what)?;
            required(wb, "temperature", (2000.0, 50_000.0), what)?;
            required(wb, "tint", (-150.0, 150.0), what)?;
            validate_auto(wb, what)
        }
        Some("relative") => {
            if source.is_raw() {
                bail!("[invalid-develop] {what} mode relative applies only to rendered sources; use mode temperature for a raw source")
            }
            allowed(wb, &["mode", "temperature", "tint", "auto"], what)?;
            required(wb, "temperature", (-100.0, 100.0), what)?;
            required(wb, "tint", (-100.0, 100.0), what)?;
            validate_auto(wb, what)
        }
        Some("neutral") => {
            allowed(wb, &["mode", "neutral", "sampled", "auto"], what)?;
            let valid = wb
                .get("neutral")
                .and_then(Value::as_array)
                .is_some_and(|n| {
                    n.len() == 3
                        && n.iter()
                            .all(|v| v.as_f64().is_some_and(|v| v > 0.0 && v <= 1.0e6))
                });
            if !valid {
                bail!("[invalid-develop] {what}.neutral must hold three positive numbers")
            }
            if let Some(sampled) = wb.get("sampled") {
                let where_ = format!("{what}.sampled");
                let sampled = group(sampled, &where_)?;
                allowed(sampled, &["x", "y", "radius"], &where_)?;
                for key in ["x", "y", "radius"] {
                    required(sampled, key, (0.0, 1.0), &where_)?;
                }
            }
            validate_auto(wb, what)
        }
        mode => bail!(
            "[invalid-develop] {what}.mode {} must be as-shot, temperature, relative or neutral",
            mode.map_or_else(|| "(missing)".to_string(), |m| format!("{m:?}"))
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(develop: Value, kind: &str) -> Result<()> {
        let profiles = Map::new();
        let source = Source {
            kind,
            camera_model: Some("Body"),
            profiles: &profiles,
            document: None,
        };
        validate(develop.as_object().unwrap(), &source, "variant p/master")
    }

    #[test]
    fn raw_and_white_balance_are_checked() {
        assert!(check(json!({"process": 1, "raw": {"demosaic": "mhc"}, "white_balance": {"mode": "temperature", "temperature": 5400, "tint": -3}}), "raw").is_ok());
        for (develop, kind) in [
            (json!({"process": 1, "raw": {"demosaic": "ahd"}}), "raw"),
            (json!({"process": 1, "raw": {}}), "rendered"),
            (
                json!({"process": 1, "white_balance": {"mode": "temperature", "temperature": 1999, "tint": 0}}),
                "raw",
            ),
            (
                json!({"process": 1, "white_balance": {"mode": "temperature", "temperature": 5000, "tint": 0}}),
                "rendered",
            ),
            (
                json!({"process": 1, "white_balance": {"mode": "relative", "temperature": 5, "tint": 0}}),
                "raw",
            ),
            (
                json!({"process": 1, "white_balance": {"mode": "neutral", "neutral": [1, 0, 1]}}),
                "raw",
            ),
            (
                json!({"process": 1, "white_balance": {"mode": "as-shot", "tint": 0}}),
                "raw",
            ),
            (
                json!({"process": 1, "raw": {"camera_profile": {"profile": format!("sha256:{}", "0".repeat(64))}}}),
                "raw",
            ),
            (json!({"process": 1, "sparkle": {}}), "raw"),
        ] {
            let error = check(develop.clone(), kind).unwrap_err().to_string();
            assert!(
                error.starts_with("[invalid-develop]") || error.starts_with("[missing-resource]"),
                "{develop}: {error}"
            );
        }
    }

    #[test]
    fn lens_geometry_and_crop_are_checked() {
        let auto = json!({"algorithm": "upright-hough", "version": 1});
        for develop in [
            json!({"process": 1, "lens": {"profile": "embedded-opcodes", "distortion": -12,
                   "vignetting": {"amount": 20, "midpoint": 50},
                   "chromatic_aberration": {"remove": true}}}),
            json!({"process": 1, "geometry": {"upright": "vertical", "auto": auto, "vertical": 4.5,
                   "rotate": -1.25, "scale": 110, "offset": [5, -5]}}),
            json!({"process": 1, "geometry": {"upright": "guided",
                   "guides": [[[0.1, 0.0], [0.12, 1.0]]]}}),
            json!({"process": 1, "crop": {"rect": [0.1, 0.1, 0.8, 0.9], "aspect": "3:2", "constrain": true}}),
        ] {
            assert!(check(develop.clone(), "raw").is_ok(), "{develop}");
        }
        for (develop, kind) in [
            (
                json!({"process": 1, "lens": {"profile": "embedded-opcodes"}}),
                "rendered",
            ),
            (json!({"process": 1, "lens": {"profile": "adobe"}}), "raw"),
            (json!({"process": 1, "lens": {"distortion": 101}}), "raw"),
            (
                json!({"process": 1, "lens": {"vignetting": {"amount": 5, "feather": 1}}}),
                "raw",
            ),
            (
                json!({"process": 1, "geometry": {"upright": "tilt"}}),
                "raw",
            ),
            (json!({"process": 1, "geometry": {"rotate": 46}}), "raw"),
            (json!({"process": 1, "geometry": {"scale": 40}}), "raw"),
            (json!({"process": 1, "geometry": {"auto": auto}}), "raw"),
            (
                json!({"process": 1, "geometry": {"upright": "level", "guides": [[[0, 0], [1, 1]]]}}),
                "raw",
            ),
            (
                json!({"process": 1, "geometry": {"upright": "guided", "guides": [[[0.5, 0.5], [0.5, 0.5]]]}}),
                "raw",
            ),
            (
                json!({"process": 1, "geometry": {"upright": "vertical", "auto": {"algorithm": "upright-hough"}}}),
                "raw",
            ),
            (
                json!({"process": 1, "crop": {"rect": [0.5, 0, 0.6, 1]}}),
                "raw",
            ),
            (json!({"process": 1, "crop": {"rect": [0, 0, 0, 1]}}), "raw"),
            (json!({"process": 1, "crop": {"aspect": "16:0"}}), "raw"),
            (json!({"process": 1, "crop": {"constrain": "yes"}}), "raw"),
        ] {
            let error = check(develop.clone(), kind).unwrap_err().to_string();
            assert!(error.starts_with("[invalid-develop]"), "{develop}: {error}");
        }
        assert_eq!(crop_aspect("original"), Some(None));
        assert_eq!(crop_aspect("3:2"), Some(Some((3, 2))));
        assert_eq!(crop_aspect("03:2"), None);
        assert_eq!(crop_aspect("10000:1"), None);
    }

    #[test]
    fn development_groups_are_checked() {
        let develop = json!({"process": 1,
            "tone": {"exposure": -1.5, "contrast": 20, "highlights": -60, "shadows": 40, "whites": 5, "blacks": -5,
                     "auto": {"algorithm": "auto-tone", "version": 1}},
            "presence": {"texture": 10, "clarity": 20, "dehaze": 30, "vibrance": 15, "saturation": -5,
                         "dehaze_airlight": [0.8, 0.85, 0.9]},
            "curves": {"parametric": {"shadows": -10, "highlights": 10, "splits": [0.2, 0.5, 0.8]},
                       "point": {"rgb": [[0, 0], [0.5, 0.55], [1, 1]], "blue": [[0, 0.05], [1, 1]]}},
            "hsl": {"hue": {"orange": -10}, "saturation": {"blue": 30}, "luminance": {"aqua": -20}},
            "grading": {"shadows": {"hue": 220, "saturation": 20}, "highlights": {"hue": 40, "saturation": 15, "luminance": 5},
                        "blending": 60, "balance": -10},
            "monochrome": {"enabled": false, "mix": {"red": 20}},
            "effects": {"vignette": {"amount": -30, "midpoint": 40, "roundness": 10, "feather": 60, "highlights": 20},
                        "grain": {"amount": 25, "size": 30, "roughness": 50, "seed": 4294967295u64}},
            "calibration": {"shadows_tint": 5, "red": {"hue": 10, "saturation": -5}}});
        check(develop, "raw").unwrap();
        for develop in [
            json!({"tone": {"exposure": 5.5}}),
            json!({"tone": {"clarity": 5}}),
            json!({"tone": {"auto": {"algorithm": "auto-tone"}}}),
            json!({"presence": {"dehaze": 101}}),
            json!({"presence": {"dehaze_airlight": [1, 1]}}),
            json!({"presence": {"dehaze_airlight": [1, -1, 1]}}),
            json!({"curves": {"parametric": {"splits": [0.5, 0.4, 0.8]}}}),
            json!({"curves": {"parametric": {"splits": [0.01, 0.4, 0.8]}}}),
            json!({"curves": {"point": {"rgb": [[0, 0]]}}}),
            json!({"curves": {"point": {"rgb": [[0.5, 0], [0.5, 1]]}}}),
            json!({"curves": {"point": {"luma": [[0, 0], [1, 1]]}}}),
            json!({"hsl": {"hue": {"teal": 5}}}),
            json!({"hsl": {"vibrance": {}}}),
            json!({"grading": {"shadows": {"hue": 361}}}),
            json!({"grading": {"midtones": {"saturation": -5}}}),
            json!({"monochrome": {"enabled": "yes"}}),
            json!({"effects": {"grain": {"amount": 20}}}),
            json!({"effects": {"grain": {"amount": 20, "seed": 4294967296u64}}}),
            json!({"effects": {"vignette": {"feather": -1}}}),
            json!({"effects": {"sharpen": {}}}),
            json!({"calibration": {"red": {"hue": 101}}}),
            json!({"calibration": {"cyan": {}}}),
        ] {
            let error = check(develop.clone(), "raw").unwrap_err().to_string();
            assert!(error.starts_with("[invalid-develop]"), "{develop}: {error}");
        }
    }
}

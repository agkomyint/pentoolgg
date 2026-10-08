//! Validation of a variant's `develop` settings (`[invalid-develop]`).
//!
//! Each roadmap item validates the groups it implements. Groups that later items
//! own are accepted as opaque objects until then.
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

fn group<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>> {
    match value.as_object() {
        Some(object) => Ok(object),
        None => bail!("[invalid-develop] {what} must be an object"),
    }
}

fn allowed(object: &Map<String, Value>, keys: &[&str], what: &str) -> Result<()> {
    if let Some(key) = object.keys().find(|key| !keys.contains(&key.as_str())) {
        bail!(
            "[invalid-develop] {what} has unknown parameter {key:?}; expected one of {}",
            keys.join(", ")
        )
    }
    Ok(())
}

fn number(
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

fn digest(value: &str) -> bool {
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
}

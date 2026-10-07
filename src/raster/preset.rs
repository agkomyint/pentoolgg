//! Brush presets: named, readable brush definitions stored in the document's
//! `brush_presets`, with portable export/import and opt-in conversion of GIMP
//! `.gbr` and MyPaint `.myb` brushes.
//!
//! A preset is data only: an engine requirement plus a canonical brush object whose
//! keys are exactly the ones [`Brush::parse`] accepts, so no preset can carry a
//! script or an unknown property. A stroke made from a preset records the fully
//! resolved brush plus the preset name, so changing or deleting a preset never
//! reinterprets an old stroke. Textured presets reference a `brush_tips` name; their
//! pixels are content-addressed in `raster_tiles` like any other tile.
use super::tip::{tip_register, TipSource};
use super::*;

pub const MAX_PRESETS: usize = 256;
const FILE_VERSION: u64 = 1;
const MAX_IMPORT_BYTES: usize = 16 << 20;

pub(super) fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        bail!("[invalid-input] preset name {name:?} must be 1-64 characters of A-Z, a-z, 0-9, '.', '_' or '-'")
    }
    Ok(())
}

fn presets_of(raw: &Value) -> Option<&Map<String, Value>> {
    raw.get("brush_presets").and_then(Value::as_object)
}

fn entry(brush: &Brush, description: Option<&str>) -> Result<Value> {
    let mut brush = brush.clone();
    // A preset is the base, not a record of where it came from.
    brush.preset = None;
    let mut out = json!({"engine": ENGINE, "brush": brush.to_json()});
    if let Some(text) = description {
        if text.len() > 280 || text.chars().any(char::is_control) {
            bail!("[invalid-input] a preset description is at most 280 characters of plain text")
        }
        out["description"] = json!(text);
    }
    Ok(out)
}

fn store(raw: &mut Value, name: &str, value: Value, replace: bool) -> Result<Value> {
    check_name(name)?;
    let existing = presets_of(raw).and_then(|p| p.get(name)).is_some();
    if existing && !replace {
        bail!("[conflict] brush preset {name:?} already exists; pass --replace to overwrite it")
    }
    if !existing && presets_of(raw).map_or(0, Map::len) >= MAX_PRESETS {
        bail!("[limit-exceeded] a document may hold at most {MAX_PRESETS} brush presets")
    }
    let mut next = raw.clone();
    next.as_object_mut()
        .context("document must be an object")?
        .entry("brush_presets")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("brush_presets must be an object")?
        .insert(name.to_owned(), value.clone());
    validate_presets(&next)?;
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(value)
}

/// Create (or with `replace`, overwrite) a preset from brush JSON.
pub fn preset_add(
    raw: &mut Value,
    name: &str,
    brush: &Value,
    description: Option<&str>,
    replace: bool,
) -> Result<Value> {
    let parsed = Brush::parse(brush)?;
    check_tip(raw, &parsed)?;
    let value = store(raw, name, entry(&parsed, description)?, replace)?;
    Ok(json!({"name": name, "preset": value, "replaced": replace}))
}

fn check_tip(raw: &Value, brush: &Brush) -> Result<()> {
    let Some(tip) = &brush.tip else {
        return Ok(());
    };
    let known = if is_digest(tip) {
        raw.get("raster_tiles").and_then(|s| s.get(tip)).is_some()
    } else {
        raw.get("brush_tips").and_then(|t| t.get(tip)).is_some()
    };
    if !known {
        bail!("[not-found] brush tip {tip:?} is not in this document; add it first with `tip-add`")
    }
    Ok(())
}

pub fn preset_remove(raw: &mut Value, name: &str) -> Result<Value> {
    let mut next = raw.clone();
    let removed = next
        .get_mut("brush_presets")
        .and_then(Value::as_object_mut)
        .and_then(|p| p.remove(name));
    if removed.is_none() {
        bail!("[not-found] brush preset {name:?} was not found; list presets with `raster DOC presets`")
    }
    if next["brush_presets"].as_object().is_some_and(Map::is_empty) {
        next.as_object_mut().unwrap().remove("brush_presets");
    }
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({"name": name}))
}

pub fn preset_list(raw: &Value) -> Value {
    let presets: Vec<Value> = presets_of(raw)
        .into_iter()
        .flatten()
        .map(|(name, value)| {
            json!({
                "name": name,
                "kind": value["brush"]["kind"],
                "size": value["brush"]["size"],
                "engine": value["engine"],
                "description": value.get("description"),
            })
        })
        .collect();
    json!({"presets": presets, "returned": presets.len()})
}

pub fn preset_show(raw: &Value, name: &str) -> Result<Value> {
    let value = presets_of(raw)
        .and_then(|p| p.get(name))
        .with_context(|| {
            format!("[not-found] brush preset {name:?} was not found; list presets with `raster DOC presets`")
        })?;
    Ok(json!({"name": name, "preset": value}))
}

/// Resolve a brush request that may name a preset. The preset supplies the base,
/// the request's own properties override it, and the result keeps `preset` so the
/// journal records where it came from. Without `preset` the request is unchanged.
pub fn resolve_preset(raw: &Value, request: &Value, preset: Option<&str>) -> Result<Value> {
    let name = match (preset, request.get("preset")) {
        (Some(name), _) => Some(name.to_owned()),
        (None, Some(Value::String(name))) => Some(name.clone()),
        (None, Some(_)) => bail!("[invalid-brush] preset must be a preset name"),
        (None, None) => None,
    };
    let Some(name) = name else {
        return Ok(request.clone());
    };
    let found = presets_of(raw).and_then(|p| p.get(&name)).with_context(|| {
        format!("[not-found] brush preset {name:?} was not found; list presets with `raster DOC presets`")
    })?;
    let mut merged = found["brush"].as_object().cloned().unwrap_or_default();
    let overrides = request
        .as_object()
        .context("[invalid-brush] brush must be an object")?;
    for (key, value) in overrides {
        merged.insert(key.clone(), value.clone());
    }
    merged.insert("preset".to_owned(), json!(name));
    Ok(Value::Object(merged))
}

/// A portable, readable preset file, including its tip pixels when it has one.
pub fn preset_export(raw: &Value, name: &str) -> Result<Value> {
    let value = presets_of(raw)
        .and_then(|p| p.get(name))
        .with_context(|| format!("[not-found] brush preset {name:?} was not found"))?;
    let mut out = json!({
        "pentool_brush_preset": FILE_VERSION,
        "name": name,
        "engine": value["engine"],
        "brush": value["brush"],
    });
    if let Some(text) = value.get("description") {
        out["description"] = text.clone();
    }
    if let Some(tip) = value["brush"]["tip"].as_str() {
        let digest = if is_digest(tip) {
            tip.to_owned()
        } else {
            raw["brush_tips"][tip]
                .as_str()
                .with_context(|| format!("[not-found] brush tip {tip:?} is missing"))?
                .to_owned()
        };
        let tile = raw["raster_tiles"]
            .get(&digest)
            .with_context(|| format!("[missing-resource] tip {digest} is not in raster_tiles"))?;
        out["tip"] = json!({"digest": digest, "tile": tile});
    }
    Ok(out)
}

/// Import a file written by [`preset_export`]. Anything it does not recognise is an
/// error rather than being ignored.
pub fn preset_import(
    raw: &mut Value,
    file: &Value,
    name: Option<&str>,
    replace: bool,
) -> Result<Value> {
    let object = file
        .as_object()
        .context("[invalid-input] a preset file must be a JSON object")?;
    const ALLOWED: [&str; 6] = [
        "pentool_brush_preset",
        "name",
        "engine",
        "brush",
        "description",
        "tip",
    ];
    if let Some(key) = object.keys().find(|k| !ALLOWED.contains(&k.as_str())) {
        bail!(
            "[invalid-input] unknown preset file property {key:?}; a preset file holds only {}",
            ALLOWED.join(", ")
        )
    }
    if object.get("pentool_brush_preset").and_then(Value::as_u64) != Some(FILE_VERSION) {
        bail!("[unsupported-version] not a pentool brush preset file version {FILE_VERSION}")
    }
    let engine = object.get("engine").and_then(Value::as_u64).unwrap_or(0);
    if engine == 0 || engine > ENGINE {
        bail!("[unsupported-capability] the preset needs raster engine {engine}; this build implements engine {ENGINE}")
    }
    let brush = Brush::parse(
        object
            .get("brush")
            .context("[invalid-input] preset has no brush")?,
    )?;
    let name = name
        .map(str::to_owned)
        .or_else(|| {
            object
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .context("[invalid-input] the preset file has no name; pass --name")?;
    let mut next = raw.clone();
    if let Some(tip) = object.get("tip") {
        let digest = tip["digest"]
            .as_str()
            .filter(|d| is_digest(d))
            .context("[invalid-input] preset tip needs a sha256: digest")?;
        // Verifies the pixels against the digest before anything is stored.
        decode_tile(&tip["tile"], digest)?;
        let tip_name = brush
            .tip
            .clone()
            .filter(|t| !is_digest(t))
            .context("[invalid-input] a preset tip must be referenced by name")?;
        let existing = next["brush_tips"].get(&tip_name).and_then(Value::as_str);
        if existing.is_some_and(|d| d != digest) {
            bail!("[conflict] this document already has a different brush tip named {tip_name:?}; rename it or pass a different tip name")
        }
        let o = next.as_object_mut().context("document must be an object")?;
        o.entry("raster_tiles")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("raster_tiles must be an object")?
            .entry(digest.to_owned())
            .or_insert_with(|| tip["tile"].clone());
        o.entry("brush_tips")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("brush_tips must be an object")?
            .insert(tip_name, json!(digest));
    }
    check_tip(&next, &brush)?;
    let description = object.get("description").and_then(Value::as_str);
    let value = store(&mut next, &name, entry(&brush, description)?, replace)?;
    *raw = next;
    Ok(json!({"name": name, "preset": value, "imported": "pentool"}))
}

fn be32(bytes: &[u8], at: usize) -> Result<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .context("[invalid-input] the .gbr file is truncated")
}

/// Import a GIMP `.gbr` brush (version 1 or 2, grayscale or RGBA) as a textured
/// preset plus a tip of the same name. Grayscale brushes paint where they are dark,
/// as in GIMP; RGBA brushes paint where they are opaque (their color is dropped,
/// because pentool picks the color per stroke).
pub fn preset_import_gbr(raw: &mut Value, bytes: &[u8], name: &str) -> Result<Value> {
    check_name(name)?;
    if bytes.len() > MAX_IMPORT_BYTES {
        bail!("[limit-exceeded] .gbr files are limited to 16 MiB")
    }
    let header = be32(bytes, 0)? as usize;
    let version = be32(bytes, 4)?;
    let (width, height, depth) = (be32(bytes, 8)?, be32(bytes, 12)?, be32(bytes, 16)?);
    if !(1..=2).contains(&version) {
        bail!("[unsupported-version] .gbr version {version} is not supported; versions 1 and 2 are")
    }
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        bail!("[limit-exceeded] .gbr brushes must be 1-4096 pixels on a side")
    }
    if depth != 1 && depth != 4 {
        bail!("[unsupported-capability] .gbr brushes with {depth} bytes per pixel are not supported; use 1 (gray) or 4 (RGBA)")
    }
    let spacing = if version == 2 {
        if bytes.get(20..24) != Some(b"GIMP") {
            bail!("[invalid-input] the .gbr file has no GIMP magic")
        }
        f64::from(be32(bytes, 24)?) / 100.0
    } else {
        0.1
    };
    let count = width as usize * height as usize * depth as usize;
    let data = bytes
        .get(header..header + count)
        .context("[invalid-input] the .gbr pixel data is truncated")?;
    let mut image = image::RgbaImage::new(width, height);
    for (i, pixel) in image.pixels_mut().enumerate() {
        *pixel = if depth == 1 {
            let v = data[i];
            image::Rgba([v, v, v, 255])
        } else {
            image::Rgba([
                data[i * 4],
                data[i * 4 + 1],
                data[i * 4 + 2],
                data[i * 4 + 3],
            ])
        };
    }
    let source = if depth == 1 {
        TipSource::Darkness
    } else {
        TipSource::Alpha
    };
    let mut next = raw.clone();
    let tip = tip_register(&mut next, name, &image, source)?;
    let spacing = spacing.clamp(0.01, 2.0);
    let brush = json!({
        "kind": "textured", "tip": name, "spacing": spacing,
        "size": f64::from(width.max(height)).min(MAX_BRUSH_SIZE),
    });
    let parsed = Brush::parse(&brush)?;
    let value = store(
        &mut next,
        name,
        entry(&parsed, Some("Imported from .gbr"))?,
        false,
    )?;
    *raw = next;
    Ok(json!({
        "name": name, "preset": value, "tip": tip, "imported": "gbr",
        "converted": ["tip pixels", "spacing", "size"],
        "unsupported": [],
    }))
}

/// Settings that only choose a paint color; pentool takes the color per stroke.
const COLOR_SETTINGS: [&str; 6] = [
    "color_h",
    "color_s",
    "color_v",
    "change_color_h",
    "change_color_hsv_s",
    "change_color_v",
];

/// Import a MyPaint `.myb` brush (version 3). Radius, opacity, hardness, dab
/// spacing and pressure on radius/opacity are converted; every other setting that
/// differs from "off" is listed under `unsupported` and not approximated.
pub fn preset_import_mypaint(raw: &mut Value, text: &str, name: &str) -> Result<Value> {
    check_name(name)?;
    if text.len() > MAX_IMPORT_BYTES {
        bail!("[limit-exceeded] .myb files are limited to 16 MiB")
    }
    let file: Value = serde_json::from_str(text)
        .map_err(|e| anyhow::anyhow!("[invalid-input] the .myb file is not valid JSON: {e}"))?;
    if file["version"].as_u64() != Some(3) {
        bail!("[unsupported-version] only MyPaint brush files with \"version\": 3 are supported")
    }
    let settings = file["settings"]
        .as_object()
        .context("[invalid-input] the .myb file has no settings object")?;
    let base = |key: &str| settings.get(key).and_then(|s| s["base_value"].as_f64());
    let inputs = |key: &str| -> Vec<String> {
        settings
            .get(key)
            .and_then(|s| s["inputs"].as_object())
            .into_iter()
            .flatten()
            .filter(|(_, points)| points.as_array().is_some_and(|p| !p.is_empty()))
            .map(|(input, _)| input.clone())
            .collect()
    };
    let mut brush = Map::new();
    let mut converted: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    let mut handled: Vec<&str> = Vec::new();

    if let Some(v) = base("radius_logarithmic") {
        // MyPaint radius is e^v pixels; pentool size is the diameter.
        let size = (2.0 * v.exp()).clamp(1.0, MAX_BRUSH_SIZE);
        brush.insert("size".into(), json!((size * 100.0).round() / 100.0));
        converted.push("radius_logarithmic -> size".into());
    }
    handled.push("radius_logarithmic");
    if let Some(v) = base("hardness") {
        let hardness = v.clamp(0.0, 1.0);
        brush.insert("hardness".into(), json!(hardness));
        brush.insert(
            "kind".into(),
            json!(if hardness < 1.0 {
                "soft-round"
            } else {
                "hard-round"
            }),
        );
        converted.push("hardness -> hardness".into());
    }
    handled.push("hardness");
    if let Some(v) = base("opaque") {
        brush.insert("flow".into(), json!(v.clamp(0.0, 1.0)));
        converted.push("opaque -> flow".into());
    }
    handled.push("opaque");
    if let Some(d) = base("dabs_per_basic_radius").filter(|d| *d > 0.0) {
        // d dabs per radius is a spacing of 1/(2d) diameters.
        let spacing = (1.0 / (2.0 * d)).clamp(0.01, 2.0);
        brush.insert("spacing".into(), json!((spacing * 1000.0).round() / 1000.0));
        converted.push("dabs_per_basic_radius -> spacing".into());
    }
    handled.push("dabs_per_basic_radius");
    if inputs("radius_logarithmic").iter().any(|i| i == "pressure") {
        brush.insert("pressure_size".into(), json!(true));
        converted.push("radius_logarithmic.pressure -> pressure_size".into());
    }
    let flow_inputs: Vec<String> = inputs("opaque")
        .into_iter()
        .chain(inputs("opaque_multiply"))
        .collect();
    if flow_inputs.iter().any(|i| i == "pressure") {
        brush.insert("pressure_flow".into(), json!(true));
        converted.push("opaque.pressure -> pressure_flow".into());
    }
    handled.push("opaque_multiply");
    for key in ["radius_logarithmic", "opaque", "opaque_multiply"] {
        for input in inputs(key) {
            if input != "pressure" {
                unsupported.push(format!("{key}.{input}"));
            }
        }
    }
    let mut ignored: Vec<String> = Vec::new();
    for (key, setting) in settings {
        let active =
            setting["base_value"].as_f64().is_some_and(|v| v != 0.0) || !inputs(key).is_empty();
        if !active || handled.contains(&key.as_str()) {
            continue;
        }
        if COLOR_SETTINGS.contains(&key.as_str()) {
            ignored.push(key.clone());
        } else {
            unsupported.push(key.clone());
        }
    }
    unsupported.sort();
    unsupported.dedup();
    ignored.sort();
    let parsed = Brush::parse(&Value::Object(brush))?;
    let mut next = raw.clone();
    let value = store(
        &mut next,
        name,
        entry(&parsed, Some("Imported from MyPaint"))?,
        false,
    )?;
    *raw = next;
    Ok(json!({
        "name": name, "preset": value, "imported": "mypaint",
        "converted": converted, "unsupported": unsupported,
        "ignored_color_settings": ignored,
    }))
}

/// Document-level `brush_presets` check used by [`validate_document`].
pub(super) fn validate_presets(raw: &Value) -> Result<()> {
    let Some(presets) = raw.get("brush_presets") else {
        return Ok(());
    };
    let presets = presets
        .as_object()
        .context("[malformed-raster] brush_presets must be an object of name -> preset")?;
    if presets.len() > MAX_PRESETS {
        bail!("[limit-exceeded] brush_presets holds more than {MAX_PRESETS} presets")
    }
    for (name, value) in presets {
        check_name(name)?;
        let object = value
            .as_object()
            .with_context(|| format!("[malformed-raster] brush preset {name} must be an object"))?;
        if let Some(key) = object
            .keys()
            .find(|k| !["engine", "brush", "description"].contains(&k.as_str()))
        {
            bail!("[malformed-raster] brush preset {name} has unknown property {key:?}")
        }
        let engine = object.get("engine").and_then(Value::as_u64).unwrap_or(0);
        if engine == 0 || engine > ENGINE {
            bail!("[unsupported-capability] brush preset {name} needs raster engine {engine}; this build implements engine {ENGINE}")
        }
        let brush =
            Brush::parse(object.get("brush").with_context(|| {
                format!("[malformed-raster] brush preset {name} has no brush")
            })?)?;
        check_tip(raw, &brush)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Value {
        serde_json::from_str(include_str!("../../docs/fixtures/v4-scene.pen")).unwrap()
    }

    #[test]
    fn presets_resolve_with_overrides_and_reject_unknown_properties() {
        let mut raw = base();
        preset_add(
            &mut raw,
            "soft",
            &json!({"kind":"soft-round","size":36,"flow":0.12}),
            Some("Soft retouch"),
            false,
        )
        .unwrap();
        assert!(preset_add(&mut raw, "soft", &json!({}), None, false).is_err());
        assert!(preset_add(&mut raw, "bad", &json!({"script":"x"}), None, false).is_err());
        let merged = resolve_preset(&raw, &json!({"size":10}), Some("soft")).unwrap();
        let brush = Brush::parse(&merged).unwrap();
        assert_eq!((brush.size, brush.flow), (10.0, 0.12));
        assert_eq!(brush.preset.as_deref(), Some("soft"));
        assert!(resolve_preset(&raw, &json!({}), Some("nope")).is_err());
        preset_remove(&mut raw, "soft").unwrap();
        assert!(raw.get("brush_presets").is_none());
    }

    #[test]
    fn mypaint_import_converts_what_it_knows_and_reports_the_rest() {
        let mut raw = base();
        let myb = r#"{"version":3,"settings":{
            "radius_logarithmic":{"base_value":2.0,"inputs":{"pressure":[[0,0],[1,1]]}},
            "hardness":{"base_value":0.4,"inputs":{}},
            "opaque":{"base_value":0.5,"inputs":{}},
            "dabs_per_basic_radius":{"base_value":4.0,"inputs":{}},
            "smudge":{"base_value":0.3,"inputs":{}},
            "color_h":{"base_value":0.1,"inputs":{}},
            "slow_tracking":{"base_value":0.0,"inputs":{}}}}"#;
        let out = preset_import_mypaint(&mut raw, myb, "pencil").unwrap();
        assert_eq!(out["unsupported"], json!(["smudge"]));
        assert_eq!(out["ignored_color_settings"], json!(["color_h"]));
        let brush = &raw["brush_presets"]["pencil"]["brush"];
        assert_eq!(brush["kind"], "soft-round");
        assert_eq!(brush["flow"], 0.5);
        assert_eq!(brush["pressure_size"], true);
        assert_eq!(brush["spacing"], 0.125);
        assert!(preset_import_mypaint(&mut raw, "{\"version\":2}", "old").is_err());
    }

    #[test]
    fn gbr_import_builds_a_tip_and_export_round_trips() {
        let mut gbr = Vec::new();
        let name = b"dot\0";
        for v in [28 + name.len() as u32, 2, 4, 4, 1] {
            gbr.extend(v.to_be_bytes());
        }
        gbr.extend(b"GIMP");
        gbr.extend(25u32.to_be_bytes());
        gbr.extend(name);
        gbr.extend([0u8; 16]);
        let mut raw = base();
        let out = preset_import_gbr(&mut raw, &gbr, "dot").unwrap();
        assert_eq!(out["preset"]["brush"]["kind"], "textured");
        assert_eq!(out["preset"]["brush"]["spacing"], 0.25);
        let file = preset_export(&raw, "dot").unwrap();
        let mut other = base();
        preset_import(&mut other, &file, None, false).unwrap();
        assert_eq!(other["brush_presets"]["dot"], raw["brush_presets"]["dot"]);
        assert_eq!(other["brush_tips"], raw["brush_tips"]);
        let mut tampered = file.clone();
        tampered["extra"] = json!(1);
        assert!(preset_import(&mut base(), &tampered, None, false).is_err());
        assert!(preset_import_gbr(&mut base(), &gbr[..30], "cut").is_err());
    }
}

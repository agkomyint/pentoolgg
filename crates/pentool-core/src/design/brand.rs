//! Brand kit: colors, type, logos, ink/profile references and usage rules
//! (`docs/design-systems-v1.md` section 6). Entries refer to tokens by ID; colors
//! are stated once as tokens. Rules are advisory data for `design audit`.
use crate::{
    asset,
    tokens::{self, DesignError},
};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

type Result<T> = std::result::Result<T, DesignError>;

pub const MAX_ENTRIES: usize = 256;
pub const MAX_LIST: usize = 64;
pub const MAX_TEXT_BYTES: usize = 512;

type KindOf<'a> = &'a dyn Fn(&str) -> Option<String>;

fn bad(path: &str, message: impl Into<String>) -> DesignError {
    DesignError::new("invalid-brand", message)
        .path(json!(path))
        .action("fix the brand block (see docs/design-systems-v1.md section 6)")
}

fn object<'a>(value: &'a Value, path: &str, allowed: &[&str]) -> Result<&'a Map<String, Value>> {
    let map = value
        .as_object()
        .ok_or_else(|| bad(path, format!("{path} must be an object")))?;
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(bad(path, format!("{path} has unknown key {key:?}")));
        }
    }
    Ok(map)
}

fn text(map: &Map<String, Value>, key: &str, path: &str) -> Result<()> {
    match map.get(key) {
        None => Ok(()),
        Some(Value::String(s)) if s.len() <= MAX_TEXT_BYTES => Ok(()),
        Some(_) => Err(bad(
            path,
            format!("{path}/{key} must be a string of at most {MAX_TEXT_BYTES} bytes"),
        )),
    }
}

fn entry_name(name: &str, path: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(bad(
            path,
            format!("name {name:?} must be 1-64 characters of a-z 0-9 - _"),
        ))
    }
}

fn token_ref(value: Option<&Value>, path: &str, want: &str, kind_of: KindOf) -> Result<String> {
    let id = value
        .and_then(Value::as_str)
        .ok_or_else(|| bad(path, format!("{path} must name a {want} token")))?;
    match kind_of(id) {
        None => Err(DesignError::new(
            "unknown-token",
            format!("{path} refers to token {id}, which does not exist"),
        )
        .token(id)
        .path(json!(path))
        .action("add the token or fix the reference")),
        Some(kind) if kind != want => Err(DesignError::new(
            "token-type-mismatch",
            format!("{path} needs a {want} token, but {id} is {kind}"),
        )
        .token(id)
        .path(json!(path))
        .action(format!("reference a {want} token"))),
        Some(_) => Ok(id.to_owned()),
    }
}

fn token_list(value: Option<&Value>, path: &str, want: &str, kind_of: KindOf) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    let list = value
        .as_array()
        .filter(|a| a.len() <= MAX_LIST)
        .ok_or_else(|| {
            bad(
                path,
                format!("{path} must be an array of at most {MAX_LIST}"),
            )
        })?;
    for (i, item) in list.iter().enumerate() {
        token_ref(Some(item), &format!("{path}/{i}"), want, kind_of)?;
    }
    Ok(())
}

fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn array<'a>(value: &'a Value, path: &str, max: usize) -> Result<&'a Vec<Value>> {
    value
        .as_array()
        .filter(|a| a.len() <= max)
        .ok_or_else(|| bad(path, format!("{path} must be an array of at most {max}")))
}

fn validate_logos(value: &Value, kind_of: KindOf) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (i, logo) in array(value, "/brand/logos", MAX_LIST)?.iter().enumerate() {
        let path = format!("/brand/logos/{i}");
        let logo = object(
            logo,
            &path,
            &[
                "id",
                "asset",
                "variants",
                "min_width",
                "clear_space",
                "forbidden_backgrounds",
            ],
        )?;
        for key in ["id", "asset"] {
            let id = logo.get(key).and_then(Value::as_str).unwrap_or("");
            asset::validate_id(id)
                .map_err(|e| bad(&format!("{path}/{key}"), format!("logo {key}: {e}")))?;
        }
        if !seen.insert(logo["id"].as_str().unwrap_or("").to_owned()) {
            return Err(bad(&path, "duplicate logo id"));
        }
        if let Some(variants) = logo.get("variants") {
            let ok = variants.as_array().is_some_and(|v| {
                v.len() <= MAX_LIST
                    && v.iter()
                        .all(|s| s.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 64))
            });
            if !ok {
                return Err(bad(&path, "variants must be an array of short strings"));
            }
        }
        if let Some(v) = logo.get("min_width") {
            if !v.as_f64().is_some_and(|n| n.is_finite() && n > 0.0) {
                return Err(bad(&path, "min_width must be a finite number > 0"));
            }
        }
        if let Some(v) = logo.get("clear_space") {
            if !v.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0) {
                return Err(bad(&path, "clear_space must be a finite number >= 0"));
            }
        }
        token_list(
            logo.get("forbidden_backgrounds"),
            &format!("{path}/forbidden_backgrounds"),
            "color",
            kind_of,
        )?;
    }
    Ok(())
}

fn validate_inks(value: &Value, kind_of: KindOf) -> Result<()> {
    let inks = object(value, "/brand/inks", &["spot", "profiles"])?;
    if let Some(spot) = inks.get("spot") {
        let mut seen = BTreeSet::new();
        for (i, ink) in array(spot, "/brand/inks/spot", MAX_ENTRIES)?
            .iter()
            .enumerate()
        {
            let path = format!("/brand/inks/spot/{i}");
            let ink = object(ink, &path, &["id", "token"])?;
            let id = ink.get("id").and_then(Value::as_str).unwrap_or("");
            asset::validate_id(id)
                .map_err(|e| bad(&format!("{path}/id"), format!("ink id: {e}")))?;
            if !seen.insert(id.to_owned()) {
                return Err(bad(&path, "duplicate ink id"));
            }
            token_ref(ink.get("token"), &format!("{path}/token"), "color", kind_of)?;
        }
    }
    if let Some(profiles) = inks.get("profiles") {
        let mut seen = BTreeSet::new();
        for (i, profile) in array(profiles, "/brand/inks/profiles", MAX_LIST)?
            .iter()
            .enumerate()
        {
            let path = format!("/brand/inks/profiles/{i}");
            let profile = object(profile, &path, &["role", "profile", "redistribution"])?;
            let role = profile.get("role").and_then(Value::as_str).unwrap_or("");
            if !matches!(role, "output" | "proof" | "display" | "working") {
                return Err(bad(&path, "role must be output, proof, display or working"));
            }
            let id = profile.get("profile").and_then(Value::as_str).unwrap_or("");
            if !digest(id) {
                return Err(bad(
                    &path,
                    "profile must be a sha256:<64 hex> ICC identity from the color catalog",
                ));
            }
            if !matches!(
                profile.get("redistribution").and_then(Value::as_str),
                Some("allowed" | "restricted" | "unknown")
            ) {
                return Err(bad(
                    &path,
                    "redistribution must be allowed, restricted or unknown",
                ));
            }
            if !seen.insert((role.to_owned(), id.to_owned())) {
                return Err(bad(&path, "duplicate profile role and identity"));
            }
        }
    }
    Ok(())
}

fn validate_rules(value: &Value, kind_of: KindOf) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (i, rule) in array(value, "/brand/rules", MAX_LIST)?.iter().enumerate() {
        let path = format!("/brand/rules/{i}");
        let kind = rule.get("kind").and_then(Value::as_str).unwrap_or("");
        let allowed: &[&str] = match kind {
            "contrast" => &["id", "kind", "min"],
            "forbidden_pairs" => &["id", "kind", "pairs"],
            "logo" => &["id", "kind"],
            "fonts" => &["id", "kind", "allowed"],
            _ => {
                return Err(bad(
                    &path,
                    "rule kind must be contrast, forbidden_pairs, logo or fonts",
                ))
            }
        };
        let map = object(rule, &path, allowed)?;
        let id = map.get("id").and_then(Value::as_str).unwrap_or("");
        entry_name(id, &format!("{path}/id"))?;
        if !seen.insert(id.to_owned()) {
            return Err(bad(&path, "duplicate rule id"));
        }
        match kind {
            "contrast" => {
                if !map
                    .get("min")
                    .and_then(Value::as_f64)
                    .is_some_and(|m| (1.0..=21.0).contains(&m))
                {
                    return Err(bad(&path, "contrast min must be a ratio from 1 to 21"));
                }
            }
            "forbidden_pairs" => {
                let pairs = map
                    .get("pairs")
                    .ok_or_else(|| bad(&path, "forbidden_pairs needs pairs"))?;
                for (j, pair) in array(pairs, &format!("{path}/pairs"), MAX_ENTRIES)?
                    .iter()
                    .enumerate()
                {
                    let pair = pair
                        .as_array()
                        .filter(|p| p.len() == 2)
                        .ok_or_else(|| bad(&path, "each pair needs exactly two tokens"))?;
                    for (k, token) in pair.iter().enumerate() {
                        token_ref(
                            Some(token),
                            &format!("{path}/pairs/{j}/{k}"),
                            "color",
                            kind_of,
                        )?;
                    }
                }
            }
            "fonts" => token_list(
                map.get("allowed"),
                &format!("{path}/allowed"),
                "typography",
                kind_of,
            )?,
            _ => {}
        }
    }
    Ok(())
}

/// Strictly validate a brand block. `kind_of` maps a token ID to its type.
pub fn validate_block(brand: &Value, kind_of: KindOf) -> Result<()> {
    let root = object(
        brand,
        "/brand",
        &["colors", "type", "logos", "inks", "rules"],
    )?;
    for (section, want) in [("colors", "color"), ("type", "typography")] {
        let Some(value) = root.get(section) else {
            continue;
        };
        let path = format!("/brand/{section}");
        let map = value
            .as_object()
            .ok_or_else(|| bad(&path, format!("{path} must be an object")))?;
        if map.len() > MAX_ENTRIES {
            return Err(bad(
                &path,
                format!("{path} has more than {MAX_ENTRIES} entries"),
            ));
        }
        for (name, entry) in map {
            let epath = format!("{path}/{name}");
            entry_name(name, &epath)?;
            let allowed: &[&str] = if section == "colors" {
                &["token", "usage", "forbidden_on"]
            } else {
                &["token", "usage"]
            };
            let entry = object(entry, &epath, allowed)?;
            token_ref(entry.get("token"), &format!("{epath}/token"), want, kind_of)?;
            text(entry, "usage", &epath)?;
            token_list(
                entry.get("forbidden_on"),
                &format!("{epath}/forbidden_on"),
                "color",
                kind_of,
            )?;
        }
    }
    if let Some(value) = root.get("logos") {
        validate_logos(value, kind_of)?;
    }
    if let Some(value) = root.get("inks") {
        validate_inks(value, kind_of)?;
    }
    if let Some(value) = root.get("rules") {
        validate_rules(value, kind_of)?;
    }
    Ok(())
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// Every token ID the brand block names, sorted and de-duplicated.
pub fn referenced_tokens(brand: &Value) -> Vec<String> {
    let mut out = BTreeSet::new();
    for section in ["colors", "type"] {
        for entry in brand
            .get(section)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.values())
        {
            if let Some(t) = entry["token"].as_str() {
                out.insert(t.to_owned());
            }
            out.extend(strings(entry.get("forbidden_on")));
        }
    }
    for logo in brand
        .get("logos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        out.extend(strings(logo.get("forbidden_backgrounds")));
    }
    for ink in brand
        .pointer("/inks/spot")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(t) = ink["token"].as_str() {
            out.insert(t.to_owned());
        }
    }
    for rule in brand
        .get("rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for pair in rule
            .get("pairs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.extend(strings(Some(pair)));
        }
        out.extend(strings(rule.get("allowed")));
    }
    out.into_iter().collect()
}

/// Profiles whose redistribution policy blocks packing.
pub fn restricted_profiles(brand: &Value) -> Vec<String> {
    brand
        .pointer("/inks/profiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| p["redistribution"] != "allowed")
        .filter_map(|p| p["profile"].as_str().map(str::to_owned))
        .collect()
}

/// Token-type lookup over a document's `styles`.
pub fn styles_kind(styles: &Value) -> impl Fn(&str) -> Option<String> + '_ {
    move |id| {
        styles
            .get(id)
            .and_then(|r| r.get("type"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
}

fn resolved(styles: &Value, token: &str) -> Value {
    let r = tokens::resolve(styles, token);
    json!({"status": r.status.as_str(), "value": r.value})
}

fn profile_rows(raw: &Value, brand: &Value) -> Vec<Value> {
    brand
        .pointer("/inks/profiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|p| {
            let id = p["profile"].as_str().unwrap_or("");
            let mut row = p.clone();
            let present = raw
                .pointer("/color_management/profiles")
                .and_then(|m| m.get(id))
                .is_some();
            row["state"] = json!(if present { "present" } else { "missing" });
            row
        })
        .collect()
}

/// `design brand show`: the document's brand block with resolved values and the
/// state of referenced ICC profiles in the document's color catalog.
pub fn show(raw: &Value) -> Result<Value> {
    let Some(brand) = raw.pointer("/design/brand") else {
        return Ok(json!({"present": false, "counts": {
            "colors":0,"type":0,"logos":0,"spot_inks":0,"profiles":0,"rules":0}}));
    };
    let styles = raw.get("styles").cloned().unwrap_or_else(|| json!({}));
    validate_block(brand, &styles_kind(&styles))?;
    let named = |section: &str| -> Vec<Value> {
        brand
            .get(section)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(name, entry)| {
                let mut row = entry.clone();
                row["name"] = json!(name);
                row["resolved"] = resolved(&styles, entry["token"].as_str().unwrap_or(""));
                row
            })
            .collect()
    };
    let colors = named("colors");
    let types = named("type");
    let logos = brand.get("logos").cloned().unwrap_or_else(|| json!([]));
    let spot: Vec<Value> = brand
        .pointer("/inks/spot")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|ink| {
            let mut row = ink.clone();
            row["resolved"] = resolved(&styles, ink["token"].as_str().unwrap_or(""));
            row
        })
        .collect();
    let profiles = profile_rows(raw, brand);
    let rules = brand.get("rules").cloned().unwrap_or_else(|| json!([]));
    Ok(json!({
        "present": true,
        "counts": {"colors": colors.len(), "type": types.len(),
                   "logos": logos.as_array().map_or(0, Vec::len), "spot_inks": spot.len(),
                   "profiles": profiles.len(), "rules": rules.as_array().map_or(0, Vec::len)},
        "colors": colors, "type": types, "logos": logos,
        "inks": {"spot": spot, "profiles": profiles}, "rules": rules,
        "libraries": raw.pointer("/design/libraries").cloned().unwrap_or_else(|| json!({})),
    }))
}

/// Reference state of a library brand's ICC profiles in a target document.
pub fn profile_report(raw: &Value, brand: &Value) -> Vec<Value> {
    profile_rows(raw, brand)
}

/// Set `design.brand` from a library's block. A different brand that did not
/// come from this library is a conflict, never overwritten silently.
pub fn install_into(doc: &mut Value, library: &str, brand: &Value) -> Result<()> {
    let ours = doc
        .pointer(&format!(
            "/design/libraries/{}/brand",
            library.replace('/', "~1")
        ))
        .and_then(Value::as_bool)
        == Some(true);
    if let Some(existing) = doc.pointer("/design/brand") {
        if existing != brand && !ours {
            return Err(DesignError::new(
                "conflict",
                "the document already has a local brand kit that differs from the library's",
            )
            .action("remove the local design.brand first; nothing was changed"));
        }
    }
    doc["design"]["brand"] = brand.clone();
    doc["design"]["libraries"][library]["brand"] = json!(true);
    Ok(())
}

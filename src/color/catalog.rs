//! Version 8 professional profile catalog, shared by document transactions.
use super::{
    icc::{Profile, MAX_PROFILE_BYTES},
    VERSION,
};
use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use serde_json::{json, Map, Value};
use std::path::Path;

pub const MAX_PROFILES: usize = 64;
pub const MAX_TOTAL_PROFILE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_CATALOG_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;

struct SizeGate {
    bytes: usize,
    maximum: usize,
}
impl std::io::Write for SizeGate {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes) {
            return Err(std::io::Error::other(
                "[limit-exceeded] professional color mutation exceeds the 64 MiB document limit",
            ));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn size_gate(raw: &Value) -> Result<()> {
    serde_json::to_writer_pretty(
        &mut SizeGate {
            bytes: 0,
            maximum: MAX_CATALOG_DOCUMENT_BYTES,
        },
        raw,
    )?;
    Ok(())
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .with_context(|| format!("[malformed-resource] {label} must be an object"))
}
fn keys(value: &Map<String, Value>, allowed: &[&str], label: &str) -> Result<()> {
    ensure!(
        value.keys().all(|key| allowed.contains(&key.as_str())),
        "[malformed-resource] {label} has an unsupported property"
    );
    Ok(())
}
fn text<'a>(value: &'a Value, label: &str, maximum: usize) -> Result<&'a str> {
    let text = value
        .as_str()
        .with_context(|| format!("[malformed-resource] {label} must be text"))?;
    ensure!(
        text.len() <= maximum,
        "[limit-exceeded] {label} exceeds {maximum} UTF-8 bytes"
    );
    Ok(text)
}
fn digest(value: &str) -> Result<()> {
    ensure!(
        value.strip_prefix("sha256:").is_some_and(|v| v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
        "[malformed-resource] ICC identity must be sha256:<64 lowercase hex>"
    );
    Ok(())
}
fn name(value: &str) -> Result<()> {
    ensure!(
        crate::photo::catalog::is_id(value),
        "[invalid-input] profile alias must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$"
    );
    Ok(())
}

pub(crate) fn validate_reference(value: &str) -> Result<()> {
    let requested = value.strip_prefix("profile:").unwrap_or(value);
    if requested.starts_with("sha256:") {
        digest(requested)
    } else {
        name(requested)
    }
}
fn policy(value: &str) -> Result<()> {
    ensure!(
        matches!(value, "allowed" | "restricted" | "unknown"),
        "[invalid-input] profile redistribution must be allowed, restricted or unknown"
    );
    Ok(())
}

pub fn empty() -> Value {
    json!({"engine":1,"legacy_rgb":"srgb","working_space":"srgb:linear","profiles":{},"aliases":{}})
}

fn metadata_structure(value: &Value) -> Result<()> {
    let fields = object(value, "ICC metadata")?;
    keys(
        fields,
        &["description", "copyright", "manufacturer", "model"],
        "ICC metadata",
    )?;
    for key in ["description", "copyright", "manufacturer", "model"] {
        let value = fields
            .get(key)
            .context("[malformed-resource] ICC metadata field is missing")?;
        if value.is_null() {
            continue;
        }
        let field = object(value, "ICC metadata text")?;
        keys(field, &["text", "language", "country"], "ICC metadata text")?;
        text(
            field
                .get("text")
                .context("[malformed-resource] ICC text is missing")?,
            "ICC metadata text",
            32768,
        )?;
        for key in ["language", "country"] {
            let v = field
                .get(key)
                .context("[malformed-resource] ICC metadata locale is missing")?;
            if !v.is_null() {
                let v = text(v, "ICC metadata locale", 2)?;
                ensure!(
                    v.len() == 2
                        && v.bytes().all(|b| if key == "language" {
                            b.is_ascii_lowercase()
                        } else {
                            b.is_ascii_uppercase()
                        }),
                    "[malformed-resource] invalid ICC metadata locale"
                );
            }
        }
    }
    Ok(())
}

fn record_structure(record: &Value) -> Result<u64> {
    let record = object(record, "ICC profile record")?;
    keys(
        record,
        &[
            "byte_length",
            "icc_version",
            "class",
            "color_space",
            "connection_space",
            "metadata",
            "storage",
            "redistribution",
            "source",
        ],
        "ICC profile record",
    )?;
    let length = record
        .get("byte_length")
        .and_then(Value::as_u64)
        .context("[malformed-resource] ICC byte_length must be an integer")?;
    ensure!(
        (132..=MAX_PROFILE_BYTES as u64).contains(&length),
        "[limit-exceeded] ICC byte_length must be 132..={MAX_PROFILE_BYTES}"
    );
    let version = text(
        record
            .get("icc_version")
            .context("[malformed-resource] ICC version is missing")?,
        "ICC version",
        5,
    )?;
    let v = version.as_bytes();
    ensure!(
        v.len() == 5
            && matches!(v[0], b'2' | b'4')
            && v[1] == b'.'
            && v[2].is_ascii_digit()
            && v[3] == b'.'
            && v[4].is_ascii_digit(),
        "[malformed-resource] invalid ICC version"
    );
    for (key, allowed) in [
        (
            "class",
            &["scnr", "mntr", "prtr", "link", "spac", "abst", "nmcl"][..],
        ),
        ("color_space", &["RGB ", "GRAY", "CMYK", "XYZ ", "Lab "][..]),
        (
            "connection_space",
            &["RGB ", "GRAY", "CMYK", "XYZ ", "Lab "][..],
        ),
    ] {
        let value = record
            .get(key)
            .and_then(Value::as_str)
            .context("[malformed-resource] ICC header declaration is missing")?;
        ensure!(
            allowed.contains(&value),
            "[malformed-resource] invalid ICC {key} declaration"
        );
    }
    policy(
        record
            .get("redistribution")
            .and_then(Value::as_str)
            .context("[malformed-resource] ICC redistribution policy is missing")?,
    )?;
    if let Some(source) = record.get("source") {
        text(source, "ICC source", 4096)?;
    }
    metadata_structure(
        record
            .get("metadata")
            .context("[malformed-resource] ICC metadata is missing")?,
    )?;
    let storage = object(
        record
            .get("storage")
            .context("[malformed-resource] ICC storage is missing")?,
        "ICC storage",
    )?;
    match storage.get("kind").and_then(Value::as_str) {
        Some("embedded") => {
            keys(
                storage,
                &["kind", "encoding", "data"],
                "ICC embedded storage",
            )?;
            ensure!(
                storage.get("encoding").and_then(Value::as_str) == Some("base64"),
                "[malformed-resource] ICC embedded encoding must be base64"
            );
            let data = storage
                .get("data")
                .and_then(Value::as_str)
                .context("[malformed-resource] ICC embedded data must be text")?;
            ensure!(
                data.len() == (length as usize).div_ceil(3) * 4,
                "[malformed-resource] ICC embedded length disagrees with byte_length"
            );
        }
        Some("external") => {
            keys(storage, &["kind", "path"], "ICC external storage")?;
            let path = text(
                storage
                    .get("path")
                    .context("[malformed-resource] ICC path is missing")?,
                "ICC external path",
                4096,
            )?;
            crate::resource::safe_relative_path(Path::new(path))?;
        }
        _ => bail!("[malformed-resource] ICC storage kind must be embedded or external"),
    }
    Ok(length)
}

fn check_bytes(record: &Value, identity: &str, bytes: &[u8]) -> Result<()> {
    ensure!(
        record["byte_length"].as_u64() == Some(bytes.len() as u64),
        "[malformed-resource] ICC {identity} byte length mismatch"
    );
    crate::resource::verify(bytes, identity)?;
    let parsed = Profile::parse(bytes)?;
    let summary = parsed.summary();
    for (key, actual) in [
        ("icc_version", summary.version),
        ("class", summary.class),
        ("color_space", summary.color_space),
        ("connection_space", summary.connection_space),
    ] {
        ensure!(
            record[key].as_str() == Some(actual.as_str()),
            "[malformed-resource] ICC {identity} {key} disagrees with original bytes"
        );
    }
    ensure!(
        record["metadata"] == serde_json::to_value(parsed.metadata()?)?,
        "[malformed-resource] ICC {identity} metadata disagrees with original bytes"
    );
    Ok(())
}

pub(crate) fn embedded(record: &Value) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(
            record["storage"]["data"]
                .as_str()
                .context("[malformed-resource] ICC base64 data is missing")?,
        )
        .context("[malformed-resource] ICC embedded data is invalid base64")
}

pub fn validate(raw: &Value, version: u64) -> Result<()> {
    // Before v8 this name remains an uninterpreted extension, as released.
    if version < VERSION {
        return Ok(());
    }
    let catalog = object(
        raw.get("color_management")
            .context("[malformed-resource] version 8 needs color_management")?,
        "color_management",
    )?;
    keys(
        catalog,
        &[
            "engine",
            "legacy_rgb",
            "working_space",
            "profiles",
            "aliases",
            "roles",
            "inks",
        ],
        "color_management",
    )?;
    ensure!(
        catalog.get("engine").and_then(Value::as_u64) == Some(1),
        "[unsupported-capability] professional color engine must be 1"
    );
    ensure!(catalog.get("legacy_rgb").and_then(Value::as_str)==Some("srgb") && catalog.get("working_space").and_then(Value::as_str)==Some("srgb:linear"),"[unsupported-capability] current scene interpretation requires legacy sRGB and linear-sRGB working space");
    let profiles = object(
        catalog
            .get("profiles")
            .context("[malformed-resource] ICC profiles are missing")?,
        "ICC profiles",
    )?;
    ensure!(
        profiles.len() <= MAX_PROFILES,
        "[limit-exceeded] ICC catalog exceeds {MAX_PROFILES} profiles"
    );
    let mut total = 0u64;
    for (identity, record) in profiles {
        digest(identity)?;
        total += record_structure(record).with_context(|| format!("ICC {identity}"))?;
        ensure!(
            total <= MAX_TOTAL_PROFILE_BYTES,
            "[limit-exceeded] ICC catalog exceeds {MAX_TOTAL_PROFILE_BYTES} decoded bytes"
        );
    }
    // Check all declared lengths before decoding any payload.
    for (identity, record) in profiles {
        if record["storage"]["kind"] == "embedded" {
            check_bytes(record, identity, &embedded(record)?)?;
        }
    }
    let aliases = object(
        catalog
            .get("aliases")
            .context("[malformed-resource] ICC aliases are missing")?,
        "ICC aliases",
    )?;
    ensure!(
        aliases.len() <= 256,
        "[limit-exceeded] ICC catalog exceeds 256 aliases"
    );
    for (alias, identity) in aliases {
        name(alias)?;
        let identity = identity
            .as_str()
            .context("[malformed-resource] ICC alias target must be a digest")?;
        digest(identity)?;
        ensure!(
            profiles.contains_key(identity),
            "[missing-resource] ICC alias {alias} references missing {identity}"
        );
    }
    if let Some(inks) = catalog.get("inks") {
        let inks = super::spot::Library::from_value(inks)
            .context("[malformed-resource] invalid v8 spot ink library")?;
        for ink in inks.inks.values() {
            let alternate = super::value::NativeColor::parse(&ink.alternate)
                .with_context(|| format!("spot ink {} alternate", ink.id))?;
            if let Some(reference) = alternate.destination().profile() {
                let requested = reference.strip_prefix("profile:").unwrap_or(reference);
                let identity = if requested.starts_with("sha256:") {
                    requested
                } else {
                    aliases
                        .get(requested)
                        .and_then(Value::as_str)
                        .with_context(|| format!("[missing-resource] spot ink {} alternate references missing ICC profile {requested}", ink.id))?
                };
                let record = profiles.get(identity).with_context(|| {
                    format!("[missing-resource] spot ink {} alternate references missing ICC profile {identity}", ink.id)
                })?;
                let expected = alternate
                    .destination()
                    .signature()
                    .context("[unsupported-capability] spot alternate ICC space is unsupported")?;
                ensure!(record["color_space"].as_str() == Some(expected), "[malformed-resource] spot ink {} alternate ICC color space disagrees with profile {identity}", ink.id);
            }
        }
    }
    super::roles::validate_in(&raw["color_management"])?;
    Ok(())
}

pub fn migrate(raw: Value) -> Result<Value> {
    let raw = crate::spreads::to_v8(raw)?;
    if raw["version"].as_u64() == Some(VERSION) {
        crate::scene::validate(&raw)?;
        size_gate(&raw)?;
        return Ok(raw);
    }
    ensure!(raw.get("color_management").is_none(),"[unsupported-capability] existing color_management extension conflicts with v8; rename the extension before migration");
    let mut raw = crate::photo::catalog::migrate(raw)?;
    raw["version"] = json!(VERSION);
    raw["color_management"] = empty();
    crate::scene::validate(&raw)?;
    size_gate(&raw)?;
    Ok(raw)
}

pub fn downgrade(raw: Value) -> Result<Value> {
    let mut raw = crate::spreads::to_v8(raw)?;
    crate::scene::validate(&raw)?;
    ensure!(
        raw["version"].as_u64() == Some(VERSION),
        "[invalid-input] color downgrade requires version 8"
    );
    ensure!(raw["color_management"]["roles"].as_object().is_none_or(Map::is_empty),"[unsupported-capability] version 7 cannot represent explicit color roles; clear their settings before downgrading");
    ensure!(raw["color_management"]["profiles"].as_object().is_some_and(Map::is_empty) && raw["color_management"]["aliases"].as_object().is_some_and(Map::is_empty),"[unsupported-capability] version 7 cannot represent the ICC catalog; collect/remove its profiles explicitly before downgrading");
    ensure!(super::scene::slots(&raw, super::scene::Scope::Document)?.is_empty(), "[unsupported-capability] version 7 cannot represent native color declarations; explicitly convert them to legacy sRGB before downgrading");
    ensure!(super::scene::destinations(&raw, super::scene::Scope::Document)?.is_empty(), "[unsupported-capability] version 7 cannot represent native gradient interpolation; explicitly change gradient spaces to legacy srgb or linear before downgrading");
    raw.as_object_mut().unwrap().remove("color_management");
    raw["version"] = json!(crate::photo::VERSION);
    crate::scene::validate(&raw)?;
    Ok(raw)
}

fn catalog(raw: &Value) -> Result<&Value> {
    crate::scene::validate(raw)?;
    ensure!(crate::color::has_v8(raw),"[unsupported-capability] professional profiles require version 8; run `pentool migrate --target 8` first");
    Ok(&raw["color_management"])
}

pub fn resolve<'a>(raw: &'a Value, identifier: &str) -> Result<(&'a str, &'a Value)> {
    let catalog = catalog(raw)?;
    resolve_in(catalog, identifier)
}

/// Resolve within an already structurally validated catalog. Scene validation
/// uses this path without recursively validating its own document.
pub(crate) fn resolve_in<'a>(catalog: &'a Value, identifier: &str) -> Result<(&'a str, &'a Value)> {
    validate_reference(identifier)?;
    let requested = identifier.strip_prefix("profile:").unwrap_or(identifier);
    let identity = if requested.starts_with("sha256:") {
        requested
    } else {
        catalog["aliases"][requested].as_str().with_context(|| {
            format!(
                "[missing-resource] ICC alias {requested} is missing; import or relink the profile"
            )
        })?
    };
    digest(identity)?;
    let (identity, record) = catalog["profiles"]
        .as_object()
        .context("[malformed-resource] ICC profiles must be an object")?
        .get_key_value(identity)
        .with_context(|| {
            format!("[missing-resource] ICC {identity} is missing; import the original profile")
        })?;
    Ok((identity, record))
}

pub fn load(raw: &Value, document: &Path, identifier: &str) -> Result<(String, Vec<u8>)> {
    let (identity, record) = resolve(raw, identifier)?;
    load_record(document, identity, record)
}

fn load_record(document: &Path, identity: &str, record: &Value) -> Result<(String, Vec<u8>)> {
    let bytes = if record["storage"]["kind"] == "embedded" {
        embedded(record)?
    } else {
        crate::resource::read_external_offline_limited(
            document,
            Path::new(record["storage"]["path"].as_str().unwrap()),
            identity,
            MAX_PROFILE_BYTES,
        )?
    };
    check_bytes(record, identity, &bytes)?;
    Ok((identity.into(), bytes))
}

/// A validated borrowed catalog for a batch of dependency resolutions. Its
/// immutable borrow prevents edits invalidating validation during the batch.
pub struct Catalog<'a> {
    value: &'a Value,
}
impl<'a> Catalog<'a> {
    pub fn new(raw: &'a Value) -> Result<Self> {
        Ok(Self {
            value: catalog(raw)?,
        })
    }
    pub fn resolve(&self, identifier: &str) -> Result<(&'a str, &'a Value)> {
        resolve_in(self.value, identifier)
    }
    pub fn load(&self, document: &Path, identifier: &str) -> Result<(String, Vec<u8>)> {
        let (identity, record) = self.resolve(identifier)?;
        load_record(document, identity, record)
    }
}

pub fn import(
    raw: &mut Value,
    document: &Path,
    bytes: &[u8],
    alias: &str,
    redistribution: &str,
    source: Option<&str>,
    external: Option<&str>,
) -> Result<Value> {
    catalog(raw)?;
    name(alias)?;
    policy(redistribution)?;
    if let Some(source) = source {
        ensure!(
            source.len() <= 4096,
            "[limit-exceeded] ICC source exceeds 4096 bytes"
        );
    }
    let parsed = Profile::parse(bytes)?;
    let summary = parsed.summary();
    let identity = summary.sha256;
    let storage = if let Some(relative) = external {
        let relative_path = crate::resource::safe_relative_path(Path::new(relative))?;
        // Import establishes a real external dependency. Cache fallback is only
        // for subsequently loading an already recorded dependency offline.
        crate::resource::document_root(document)
            .join(&relative_path)
            .canonicalize()
            .with_context(|| {
                format!("[missing-resource] external ICC {relative} must exist before import")
            })?;
        let checked = crate::resource::read_external_offline_limited(
            document,
            Path::new(relative),
            &identity,
            MAX_PROFILE_BYTES,
        )?;
        ensure!(
            checked == bytes,
            "[hash-mismatch] ICC external source differs from imported original bytes"
        );
        json!({"kind":"external","path":relative})
    } else {
        json!({"kind":"embedded","encoding":"base64","data":base64::engine::general_purpose::STANDARD.encode(bytes)})
    };
    let mut record = json!({"byte_length":bytes.len(),"icc_version":summary.version,"class":summary.class,"color_space":summary.color_space,"connection_space":summary.connection_space,"metadata":parsed.metadata()?,"storage":storage,"redistribution":redistribution});
    if let Some(source) = source {
        record["source"] = json!(source);
    }
    let mut planned = raw.clone();
    let catalog = &mut planned["color_management"];
    if let Some(existing) = catalog["aliases"].get(alias) {
        ensure!(existing==&identity,"[invalid-input] ICC alias {alias} already identifies another profile; choose a new alias");
    }
    let reused = catalog["profiles"].get(&identity).is_some();
    if reused {
        ensure!(catalog["profiles"][&identity]["redistribution"]==record["redistribution"] && catalog["profiles"][&identity].get("source")==record.get("source"),"[invalid-input] existing ICC provenance differs; importing cannot silently replace licensing/source declarations");
    } else {
        catalog["profiles"][&identity] = record;
    }
    catalog["aliases"][alias] = json!(identity);
    crate::scene::validate(&planned)?;
    size_gate(&planned)?;
    *raw = planned;
    Ok(json!({"profile":identity,"alias":alias,"reused":reused,"assigned":false}))
}

pub fn embed(raw: &mut Value, document: &Path, identifier: &str) -> Result<Value> {
    let (identity, bytes) = load(raw, document, identifier)?;
    let already_embedded =
        raw["color_management"]["profiles"][&identity]["storage"]["kind"] == "embedded";
    let mut planned = raw.clone();
    planned["color_management"]["profiles"][&identity]["storage"] = json!({"kind":"embedded","encoding":"base64","data":base64::engine::general_purpose::STANDARD.encode(bytes)});
    crate::scene::validate(&planned)?;
    size_gate(&planned)?;
    *raw = planned;
    Ok(json!({"profile":identity,"embedded":true,"already_embedded":already_embedded}))
}

/// Collect every catalog entry in one private plan. Licensing is checked for
/// the entire set before reading external bytes; no dependency is omitted.
pub fn collect(raw: &mut Value, document: &Path, local_only: bool) -> Result<Value> {
    let context = Catalog::new(raw)?;
    let records = raw["color_management"]["profiles"].as_object().unwrap();
    for (identity, record) in records {
        ensure!(local_only || record["redistribution"]=="allowed", "[redistribution-restricted] ICC {identity} has policy {}; collection for sharing requires allowed permission, or use --local-only for a private copy",record["redistribution"]);
    }
    let mut planned = raw.clone();
    let mut embedded_count = 0;
    let mut total = 0u64;
    for (identity, record) in records {
        let (_, bytes) = context.load(document, identity)?;
        total += bytes.len() as u64;
        embedded_count += usize::from(record["storage"]["kind"] != "embedded");
        planned["color_management"]["profiles"][identity]["storage"] = json!({"kind":"embedded","encoding":"base64","data":base64::engine::general_purpose::STANDARD.encode(bytes)});
    }
    crate::scene::validate(&planned)?;
    size_gate(&planned)?;
    let report = json!({"version":1,"scope":"icc-catalog","local_only":local_only,"matched":records.len(),"embedded":embedded_count,"already_embedded":records.len()-embedded_count,"byte_length":total,"profiles":profile_table(&planned)});
    *raw = planned;
    Ok(report)
}

fn profile_table(raw: &Value) -> Vec<Value> {
    let catalog = &raw["color_management"];
    catalog["profiles"].as_object().unwrap().iter().map(|(identity,record)|json!({"digest":identity,"byte_length":record["byte_length"],"icc_version":record["icc_version"],"class":record["class"],"color_space":record["color_space"],"connection_space":record["connection_space"],"redistribution":record["redistribution"],"source":record.get("source"),"aliases":catalog["aliases"].as_object().unwrap().iter().filter(|(_,value)|value.as_str()==Some(identity)).map(|(name,_)|name).collect::<Vec<_>>()})).collect()
}

/// A deterministic audit table for the existing package manifest. The document
/// carries original embedded bytes, and the shared validator verifies them.
pub fn package_table(raw: &Value) -> Result<Vec<Value>> {
    crate::transaction::validate_value(raw)?;
    package_table_validated(raw)
}

/// Package callers have already run the shared document validator. Reuse that
/// validation rather than decoding original image/ICC bytes a second time.
pub(crate) fn package_table_validated(raw: &Value) -> Result<Vec<Value>> {
    if !crate::color::has_v8(raw) {
        return Ok(Vec::new());
    }
    for (identity, record) in raw["color_management"]["profiles"].as_object().unwrap() {
        ensure!(record["storage"]["kind"]=="embedded","[unsupported-capability] packaged ICC {identity} must be embedded; run `color profile collect` first");
        ensure!(record["redistribution"]=="allowed","[redistribution-restricted] packaged ICC {identity} requires a declared allowed redistribution policy");
    }
    super::roles::validate_embedded_paths(raw)?;
    super::gradient::validate_embedded_paths(raw)?;
    Ok(profile_table(raw))
}

/// Explicitly remove an unused identity and all its aliases. Missing external
/// bytes do not prevent cleanup; any native reference rejects the whole plan.
pub fn remove(raw: &mut Value, identifier: &str) -> Result<Value> {
    let catalog = catalog(raw)?;
    let (identity, _) = resolve_in(catalog, identifier)?;
    super::roles::reject_removal(raw, identity)?;
    for slot in super::scene::destinations(raw, super::scene::Scope::Document)? {
        if let Some(reference) = slot.destination.profile() {
            let (used, _) = resolve_in(catalog, reference)?;
            ensure!(used!=identity,"[resource-in-use] ICC {identity} is used by {}, gradient {}; change its interpolation space before removing it",slot.owner,slot.pointer);
        }
    }
    for slot in super::scene::slots(raw, super::scene::Scope::Document)? {
        if let Some(reference) = slot.color.destination().profile() {
            let (used, _) = resolve_in(catalog, reference)?;
            ensure!(used!=identity,"[resource-in-use] ICC {identity} is used by {}, color {}; assign or convert its paints before removing it",slot.owner,slot.pointer);
        }
    }
    let aliases: Vec<String> = catalog["aliases"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, value)| value.as_str() == Some(identity))
        .map(|(name, _)| name.clone())
        .collect();
    let identity = identity.to_owned();
    let mut planned = raw.clone();
    planned["color_management"]["profiles"]
        .as_object_mut()
        .unwrap()
        .remove(&identity);
    for alias in &aliases {
        planned["color_management"]["aliases"]
            .as_object_mut()
            .unwrap()
            .remove(alias);
    }
    crate::scene::validate(&planned)?;
    size_gate(&planned)?;
    *raw = planned;
    Ok(json!({"profile":identity,"removed":true,"aliases":aliases}))
}

pub fn list(raw: &Value, offset: usize, limit: usize) -> Result<Value> {
    ensure!(
        (1..=256).contains(&limit),
        "[invalid-input] profile limit must be 1..=256"
    );
    let catalog = catalog(raw)?;
    let profiles = catalog["profiles"].as_object().unwrap();
    let items: Vec<Value> = profiles.iter().skip(offset).take(limit).map(|(identity,record)|json!({"profile":identity,"byte_length":record["byte_length"],"icc_version":record["icc_version"],"class":record["class"],"color_space":record["color_space"],"storage":record["storage"]["kind"],"redistribution":record["redistribution"],"aliases":catalog["aliases"].as_object().unwrap().iter().filter(|(_,v)|v.as_str()==Some(identity.as_str())).map(|(k,_)|k.as_str()).collect::<Vec<_>>()})).collect();
    Ok(
        json!({"matches":profiles.len(),"returned":items.len(),"offset":offset,"limit":limit,"has_more":offset.saturating_add(items.len())<profiles.len(),"profiles":items}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_size_gate_counts_serialized_bytes_without_collecting_output() {
        let value = json!({"text":"a\\b\n🖨","nested":[1,2,3]});
        let expected = serde_json::to_vec_pretty(&value).unwrap().len();
        let mut writer = SizeGate {
            bytes: 0,
            maximum: expected,
        };
        serde_json::to_writer_pretty(&mut writer, &value).unwrap();
        assert_eq!(writer.bytes, expected);
        let mut writer = SizeGate {
            bytes: 0,
            maximum: expected - 1,
        };
        assert!(serde_json::to_writer_pretty(&mut writer, &value)
            .unwrap_err()
            .to_string()
            .contains("limit-exceeded"));
    }
}

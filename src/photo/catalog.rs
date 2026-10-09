//! The version 7 photography catalog (`docs/photography-v1.md`): structural
//! validation, photo node references, migration to and from version 6, and the
//! `raw add` / `raw info` operations.
use super::dng::Dng;
use super::VERSION;
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const ENGINE: u64 = 1;
pub const WORKING_SPACE: &str = "prophoto-linear";
/// The largest photo source, embedded or external.
pub const MAX_PHOTO_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_PROFILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ASSETS: usize = 10_000;
const MAX_PROFILES: usize = 256;
const MAX_PHOTOS: usize = 10_000;
pub(super) const MAX_VARIANTS: usize = 64;
pub(super) const MAX_SNAPSHOTS: usize = 64;
const MAX_STACKS: usize = 1024;
const MAX_COLLECTIONS: usize = 256;
const MAX_RECIPES: usize = 64;
const MAX_KEYWORDS: usize = 64;
const RESERVED_RECIPES: [&str; 4] = ["web-gallery", "social", "archive-master", "photo-lab"];
const RGB_SPACES: [&str; 5] = [
    "srgb",
    "display-p3",
    "adobe-rgb-1998",
    "prophoto",
    "rec2020",
];

/// Photo and variant IDs: `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`.
pub fn is_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn id<'a>(value: Option<&'a Value>, what: &str) -> Result<&'a str> {
    let value = value
        .and_then(Value::as_str)
        .with_context(|| format!("[malformed-resource] {what} must be a string ID"))?;
    if !is_id(value) {
        bail!(
            "[malformed-resource] {what} {value:?} must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$"
        )
    }
    Ok(value)
}

fn digest(value: &str, what: &str) -> Result<()> {
    let valid = value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    if !valid {
        bail!("[malformed-resource] {what} {value:?} must be sha256: and 64 lowercase hexadecimal digits")
    }
    Ok(())
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .with_context(|| format!("[malformed-resource] {what} must be an object"))
}

fn keys(object: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<()> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        bail!("[malformed-resource] {what} has unknown property {key:?}")
    }
    Ok(())
}

fn integer(object: &Map<String, Value>, key: &str, range: (u64, u64), what: &str) -> Result<u64> {
    let value = object
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("[malformed-resource] {what} {key} must be an integer"))?;
    if value < range.0 || value > range.1 {
        bail!(
            "[malformed-resource] {what} {key} {value} is outside {}–{}",
            range.0,
            range.1
        )
    }
    Ok(value)
}

fn text(object: &Map<String, Value>, key: &str, max: usize, what: &str) -> Result<()> {
    if let Some(value) = object.get(key) {
        let value = value
            .as_str()
            .with_context(|| format!("[malformed-resource] {what} {key} must be a string"))?;
        if value.chars().count() > max {
            bail!("[malformed-resource] {what} {key} exceeds {max} characters")
        }
    }
    Ok(())
}

fn one_of(object: &Map<String, Value>, key: &str, allowed: &[&str], what: &str) -> Result<()> {
    if let Some(value) = object.get(key) {
        if !value.as_str().is_some_and(|value| allowed.contains(&value)) {
            bail!("[malformed-resource] {what} {key} must be one of {allowed:?}")
        }
    }
    Ok(())
}

/// Validate a v7 photo node's own properties. References are checked against the
/// catalog by [`validate`], which sees the whole document.
pub fn validate_node(node: &Map<String, Value>) -> Result<()> {
    let node_id = node.get("id").and_then(Value::as_str).unwrap_or("unknown");
    let what = format!("photo node {node_id}");
    id(node.get("photo"), &format!("{what} photo"))?;
    id(node.get("variant"), &format!("{what} variant"))?;
    for key in ["x", "y", "width", "height"] {
        let value = node
            .get(key)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .with_context(|| {
                format!("[malformed-resource] {what} {key} must be a finite number")
            })?;
        if matches!(key, "width" | "height") && value <= 0.0 {
            bail!("[malformed-resource] {what} {key} must be positive")
        }
    }
    match node.get("fit").and_then(Value::as_str) {
        Some("fill" | "contain" | "cover" | "none" | "scale-down") => {}
        _ => bail!(
            "[malformed-resource] {what} fit must be fill, contain, cover, none or scale-down"
        ),
    }
    let position = node
        .get("position")
        .and_then(Value::as_array)
        .filter(|values| values.len() == 2)
        .with_context(|| format!("[malformed-resource] {what} position must hold 2 numbers"))?;
    if !position
        .iter()
        .all(|value| value.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n)))
    {
        bail!("[malformed-resource] {what} position values must be in 0–1")
    }
    if node.contains_key("crop") {
        bail!("[malformed-resource] {what} cannot have crop; cropping belongs to the variant")
    }
    if let Some(opacity) = node.get("opacity") {
        if !opacity.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n)) {
            bail!("[malformed-resource] {what} opacity must be in 0–1")
        }
    }
    Ok(())
}

/// Every photo node in page order, including group children and instance fallbacks.
pub(super) fn photo_nodes(raw: &Value) -> Vec<&Map<String, Value>> {
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Map<String, Value>>) {
        let Some(object) = node.as_object() else {
            return;
        };
        if object.get("kind").and_then(Value::as_str) == Some("photo") {
            out.push(object);
        }
        for child in object
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            walk(child, out);
        }
        if let Some(fallback) = object.get("fallback") {
            walk(fallback, out);
        }
    }
    let mut out = Vec::new();
    for page in raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for layer in page
            .get("layers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for node in layer
                .get("nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(node, &mut out);
            }
        }
    }
    match raw.get("mask_resources") {
        Some(Value::Object(resources)) => {
            for resource in resources.values() {
                walk(&resource["node"], &mut out)
            }
        }
        Some(Value::Array(resources)) => {
            for resource in resources {
                walk(&resource["node"], &mut out)
            }
        }
        _ => {}
    }
    out
}

fn node_id(node: &Map<String, Value>) -> &str {
    node.get("id").and_then(Value::as_str).unwrap_or("unknown")
}

/// Validate the photography catalog and photo node references of a scene document.
/// Documents before version 7 must contain neither.
pub fn validate(raw: &Value, version: u64) -> Result<()> {
    let nodes = photo_nodes(raw);
    if version < VERSION {
        if raw.get("photography").is_some() {
            bail!("[unsupported-capability] the photography catalog requires document format version 7; run `pentool migrate --target 7` first")
        }
        if let Some(node) = nodes.first() {
            bail!(
                "[unsupported-capability] photo node {} requires document format version 7",
                node_id(node)
            )
        }
        return Ok(());
    }
    let catalog = match raw.get("photography") {
        Some(catalog) => validate_catalog(raw, catalog)?,
        None => HashMap::new(),
    };
    for node in nodes {
        let photo = node["photo"].as_str().unwrap_or_default();
        let variant = node["variant"].as_str().unwrap_or_default();
        let variants = catalog.get(photo).with_context(|| {
            format!(
                "[missing-resource] photo node {} references missing photo {photo}",
                node_id(node)
            )
        })?;
        if !variants.contains(variant) {
            bail!(
                "[missing-resource] photo node {} references missing variant {photo}/{variant}",
                node_id(node)
            )
        }
    }
    Ok(())
}

/// Photo IDs mapped to their variant IDs.
fn validate_catalog<'a>(
    raw: &Value,
    catalog: &'a Value,
) -> Result<HashMap<&'a str, HashSet<&'a str>>> {
    let catalog = object(catalog, "photography")?;
    keys(
        catalog,
        &[
            "engine",
            "working_space",
            "assets",
            "profiles",
            "photos",
            "stacks",
            "collections",
            "recipes",
        ],
        "photography",
    )?;
    if catalog.get("engine").and_then(Value::as_u64) != Some(ENGINE) {
        bail!("[unsupported-capability] photography engine must be {ENGINE}")
    }
    if catalog.get("working_space").and_then(Value::as_str) != Some(WORKING_SPACE) {
        bail!("[unsupported-capability] photography working_space must be {WORKING_SPACE}")
    }
    let profiles = match catalog.get("profiles") {
        Some(profiles) => object(profiles, "photography.profiles")?.clone(),
        None => Map::new(),
    };
    if profiles.len() > MAX_PROFILES {
        bail!("[limit-exceeded] photography has more than {MAX_PROFILES} profiles")
    }
    let mut embedded = embedded_image_bytes(raw);
    for (key, profile) in &profiles {
        digest(key, "photography profile key")?;
        let what = format!("profile {key}");
        let profile = object(profile, &what)?;
        keys(
            profile,
            &[
                "kind",
                "name",
                "byte_length",
                "storage",
                "unique_camera_model",
                "embed_policy",
                "force_model",
                "imported_from",
                "unsupported",
            ],
            &what,
        )?;
        one_of(profile, "kind", &["camera", "lens", "icc"], &what)?;
        if !profile
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty() && name.chars().count() <= 256)
        {
            bail!("[malformed-resource] {what} name must be 1–256 characters")
        }
        let length = integer(profile, "byte_length", (1, MAX_PROFILE_BYTES), &what)?;
        if profile.contains_key("force_model") && !profile["force_model"].is_boolean() {
            bail!("[malformed-resource] {what} force_model must be true or false")
        }
        let bytes = validate_storage(profile.get("storage"), key, length, &what, &mut embedded)?;
        // A camera profile's recorded facts must be the facts its bytes produce.
        if let (Some(bytes), Some("camera")) = (&bytes, profile.get("kind").and_then(Value::as_str))
        {
            let facts = super::profile::CameraProfile::from_dcp(bytes)
                .with_context(|| what.clone())?
                .record_facts();
            let facts = facts.as_object().unwrap();
            if facts
                .iter()
                .any(|(key, value)| profile.get(key) != Some(value))
                || (profile.contains_key("embed_policy") && !facts.contains_key("embed_policy"))
            {
                bail!("[malformed-resource] {what} recorded facts do not match its camera profile bytes")
            }
        }
        if let Some(unsupported) = profile.get("unsupported") {
            let valid = unsupported.as_array().is_some_and(|list| {
                list.len() <= super::lens::MAX_UNSUPPORTED
                    && list.iter().all(|item| {
                        item.as_str()
                            .is_some_and(|s| !s.is_empty() && s.chars().count() <= 256)
                    })
            });
            if !valid {
                bail!("[malformed-resource] {what} unsupported must hold at most 256 strings of 1–256 characters")
            }
        }
        if profile.get("kind").and_then(Value::as_str) == Some("lens") {
            one_of(profile, "imported_from", &["pentool-lens", "lcp"], &what)?;
            if !profile.contains_key("imported_from") {
                bail!("[malformed-resource] {what} must record imported_from pentool-lens or lcp")
            }
            if let Some(bytes) = bytes {
                let lens = super::lens::LensProfile::parse(&bytes).with_context(|| what.clone())?;
                if profile.get("name").and_then(Value::as_str) != Some(lens.name().as_str()) {
                    bail!("[malformed-resource] {what} name does not match its lens profile bytes")
                }
            }
        }
    }
    let assets = object(
        catalog
            .get("assets")
            .context("[malformed-resource] photography assets are missing")?,
        "photography.assets",
    )?;
    if assets.len() > MAX_ASSETS {
        bail!("[limit-exceeded] photography has more than {MAX_ASSETS} assets")
    }
    for (key, asset) in assets {
        validate_asset(key, asset, &profiles, &mut embedded)?;
    }
    let photos = catalog
        .get("photos")
        .and_then(Value::as_array)
        .context("[malformed-resource] photography photos must be an array")?;
    if photos.len() > MAX_PHOTOS {
        bail!("[limit-exceeded] photography has more than {MAX_PHOTOS} photos")
    }
    let mut ids: HashMap<&str, HashSet<&str>> = HashMap::with_capacity(photos.len());
    for photo in photos {
        let (photo_id, variants) = validate_photo(raw, photo, assets, &profiles)?;
        if ids.insert(photo_id, variants).is_some() {
            bail!("[malformed-resource] photo ID {photo_id} is used more than once")
        }
    }
    let known = |list: &Value, what: &str, min: usize| -> Result<()> {
        let list = list
            .as_array()
            .with_context(|| format!("[malformed-resource] {what} photos must be an array"))?;
        if list.len() < min || list.len() > MAX_PHOTOS {
            bail!("[malformed-resource] {what} must list {min}–{MAX_PHOTOS} photos")
        }
        let mut seen = HashSet::new();
        for entry in list {
            let photo = id(Some(entry), &format!("{what} photo"))?;
            if !seen.insert(photo) {
                bail!("[malformed-resource] {what} lists photo {photo} more than once")
            }
            if !ids.contains_key(photo) {
                bail!("[missing-resource] {what} references missing photo {photo}")
            }
        }
        Ok(())
    };
    if let Some(stacks) = catalog.get("stacks") {
        let stacks = object(stacks, "photography.stacks")?;
        if stacks.len() > MAX_STACKS {
            bail!("[limit-exceeded] photography has more than {MAX_STACKS} stacks")
        }
        let mut stacked: HashMap<&str, &str> = HashMap::new();
        for (name, stack) in stacks {
            let what = format!("stack {name}");
            id(Some(&json!(name)), "stack ID")?;
            let stack = object(stack, &what)?;
            keys(stack, &["photos"], &what)?;
            known(&stack["photos"], &what, 2)?;
            for photo in stack["photos"].as_array().into_iter().flatten() {
                let photo = photo.as_str().unwrap_or_default();
                if let Some(other) = stacked.insert(photo, name) {
                    bail!("[malformed-resource] photo {photo} is in stacks {other} and {name}; a photo belongs to at most one stack")
                }
            }
        }
    }
    if let Some(collections) = catalog.get("collections") {
        let collections = object(collections, "photography.collections")?;
        if collections.len() > MAX_COLLECTIONS {
            bail!("[limit-exceeded] photography has more than {MAX_COLLECTIONS} collections")
        }
        for (name, collection) in collections {
            let what = format!("collection {name}");
            id(Some(&json!(name)), "collection ID")?;
            let collection = object(collection, &what)?;
            text(collection, "name", 256, &what)?;
            match collection.get("kind").and_then(Value::as_str) {
                Some("manual") => {
                    keys(collection, &["kind", "name", "photos"], &what)?;
                    known(&collection["photos"], &what, 0)?;
                }
                Some("smart") => {
                    keys(collection, &["kind", "name", "query"], &what)?;
                    if !collection
                        .get("query")
                        .and_then(Value::as_str)
                        .is_some_and(|query| !query.is_empty() && query.chars().count() <= 4096)
                    {
                        bail!("[malformed-resource] {what} query must be 1–4096 characters")
                    }
                    super::organize::parse(collection["query"].as_str().unwrap_or_default())
                        .with_context(|| format!("[malformed-resource] {what} query"))?;
                }
                _ => bail!("[malformed-resource] {what} kind must be manual or smart"),
            }
        }
    }
    if let Some(recipes) = catalog.get("recipes") {
        let recipes = object(recipes, "photography.recipes")?;
        if recipes.len() > MAX_RECIPES {
            bail!("[limit-exceeded] photography has more than {MAX_RECIPES} recipes")
        }
        for (name, recipe) in recipes {
            id(Some(&json!(name)), "recipe ID")?;
            if RESERVED_RECIPES.contains(&name.as_str()) {
                bail!("[malformed-resource] recipe {name} reuses a built-in recipe name; choose another")
            }
            let what = format!("recipe {name}");
            let recipe = object(recipe, &what)?;
            one_of(recipe, "format", &["jpeg", "png", "tiff"], &what)?;
            if !recipe.contains_key("format") || !recipe.contains_key("color_space") {
                bail!("[malformed-resource] {what} requires format and color_space")
            }
        }
    }
    Ok(ids)
}

/// Embedded `image_assets` bytes, which share the 512 MiB document limit with photos.
fn embedded_image_bytes(raw: &Value) -> u64 {
    raw.get("image_assets")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|assets| assets.values())
        .filter(|asset| asset["storage"]["kind"] == "embedded")
        .filter_map(|asset| asset["byte_length"].as_u64())
        .sum()
}

/// Check storage and, for embedded bytes, their length and digest. Returns the
/// decoded embedded bytes.
fn validate_storage(
    storage: Option<&Value>,
    key: &str,
    length: u64,
    what: &str,
    embedded: &mut u64,
) -> Result<Option<Vec<u8>>> {
    let storage = object(
        storage.with_context(|| format!("[malformed-resource] {what} storage is missing"))?,
        &format!("{what} storage"),
    )?;
    match storage.get("kind").and_then(Value::as_str) {
        Some("embedded") => {
            if length > crate::image::MAX_SOURCE_BYTES {
                bail!("[limit-exceeded] {what} is larger than the 128 MiB embedding limit; store it with --external")
            }
            *embedded = embedded
                .checked_add(length)
                .context("[limit-exceeded] embedded byte total overflow")?;
            if *embedded > crate::image::MAX_DOCUMENT_SOURCE_BYTES {
                bail!("[limit-exceeded] embedded photo and image sources exceed the 512 MiB document limit; store large sources with --external")
            }
            if storage.get("encoding").and_then(Value::as_str) != Some("base64") {
                bail!("[unsupported-capability] {what} storage encoding must be base64")
            }
            let encoded = storage
                .get("data")
                .and_then(Value::as_str)
                .with_context(|| format!("[malformed-resource] {what} storage data is missing"))?;
            if encoded.len() as u64 > length.div_ceil(3) * 4 {
                bail!("[limit-exceeded] {what} encoded data exceeds its declared size")
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .with_context(|| format!("[malformed-resource] {what} has invalid base64"))?;
            if bytes.len() as u64 != length {
                bail!("[malformed-resource] {what} byte_length does not match its embedded data")
            }
            crate::resource::verify(&bytes, key)?;
            Ok(Some(bytes))
        }
        Some("external") => {
            let path = storage
                .get("path")
                .and_then(Value::as_str)
                .with_context(|| format!("[malformed-resource] {what} storage path is missing"))?;
            crate::resource::safe_relative_path(Path::new(path))?;
            if path.contains("//") || path.contains('\0') {
                bail!("[unsafe-path] {what} storage path must be a normalized relative path")
            }
            Ok(None)
        }
        _ => bail!("[unsupported-capability] {what} storage kind must be embedded or external"),
    }
}

fn validate_asset(
    key: &str,
    asset: &Value,
    profiles: &Map<String, Value>,
    embedded: &mut u64,
) -> Result<()> {
    digest(key, "photography asset key")?;
    let what = format!("photo asset {key}");
    let record = object(asset, &what)?;
    keys(
        record,
        &[
            "media_type",
            "kind",
            "byte_length",
            "pixel_width",
            "pixel_height",
            "orientation",
            "bit_depth",
            "input_profile",
            "storage",
            "capture",
            "raw",
            "derived",
        ],
        &what,
    )?;
    let media_type = record.get("media_type").and_then(Value::as_str);
    let kind = record.get("kind").and_then(Value::as_str);
    match (kind, media_type) {
        (Some("raw" | "derived"), Some(super::dng::MEDIA_TYPE)) => {
            if record.get("input_profile") != Some(&json!("camera")) || !record.contains_key("raw")
            {
                bail!("[malformed-resource] {what} is a raw source and needs input_profile camera and raw facts")
            }
            if (kind == Some("derived")) != record.contains_key("derived") {
                bail!("[malformed-resource] {what}: only derived sources carry derived")
            }
            if let Some(derived) = record.get("derived") {
                let what = format!("{what} derived");
                let derived = object(derived, &what)?;
                keys(
                    derived,
                    &["operation", "algorithm", "inputs", "settings", "alignment"],
                    &what,
                )?;
                if !matches!(
                    derived.get("operation").and_then(Value::as_str),
                    Some("merge-hdr" | "merge-pano")
                ) {
                    bail!("[malformed-resource] {what} operation must be merge-hdr or merge-pano")
                }
                integer(derived, "algorithm", (1, u64::from(u32::MAX)), &what)?;
                let inputs = derived
                    .get("inputs")
                    .and_then(Value::as_array)
                    .filter(|inputs| (2..=64).contains(&inputs.len()))
                    .with_context(|| {
                        format!("[malformed-resource] {what} inputs must list 2 to 64 sources")
                    })?;
                for input in inputs {
                    digest(input.as_str().unwrap_or_default(), &format!("{what} input"))?;
                }
                for key in ["settings", "alignment"] {
                    object(
                        derived.get(key).unwrap_or(&Value::Null),
                        &format!("{what} {key}"),
                    )?;
                }
            }
        }
        (Some("rendered"), Some("image/tiff" | "image/png" | "image/jpeg" | "image/webp")) => {
            if record.contains_key("raw") || record.contains_key("derived") {
                bail!("[malformed-resource] {what} is rendered and cannot carry raw or derived facts")
            }
        }
        _ => bail!("[unsupported-capability] {what} kind {kind:?} with media type {media_type:?} is unsupported"),
    }
    let length = integer(record, "byte_length", (1, MAX_PHOTO_SOURCE_BYTES), &what)?;
    let width = integer(record, "pixel_width", (1, 32_768), &what)?;
    let height = integer(record, "pixel_height", (1, 32_768), &what)?;
    if width * height > super::pixels::MAX_PHOTO_PIXELS {
        bail!(
            "[limit-exceeded] {what} has more than {} pixels",
            super::pixels::MAX_PHOTO_PIXELS
        )
    }
    integer(record, "orientation", (1, 8), &what)?;
    if !matches!(
        record.get("bit_depth").and_then(Value::as_u64),
        Some(8 | 10 | 12 | 14 | 16 | 24 | 32)
    ) {
        bail!("[malformed-resource] {what} bit_depth must be 8, 10, 12, 14, 16, 24 or 32")
    }
    match record.get("input_profile") {
        Some(Value::String(name)) if name == "camera" || RGB_SPACES.contains(&name.as_str()) => {}
        Some(Value::Object(icc)) if icc.len() == 1 && icc.contains_key("icc") => {
            let profile = icc["icc"].as_str().unwrap_or_default();
            digest(profile, &format!("{what} input_profile"))?;
            if !profiles.contains_key(profile) {
                bail!("[missing-resource] {what} references missing profile {profile}")
            }
        }
        _ => bail!("[malformed-resource] {what} input_profile must be camera, an RGB space name or {{\"icc\": digest}}"),
    }
    let bytes = validate_storage(record.get("storage"), key, length, &what, embedded)?;
    // A raw or derived source's recorded facts must be the facts its bytes
    // produce; a derived source also records `derived` and its own kind.
    if let (Some(bytes), Some(kind @ ("raw" | "derived"))) = (bytes, kind) {
        let facts = Dng::inspect(&bytes)
            .with_context(|| what.clone())?
            .asset_facts();
        let facts = facts.as_object().unwrap();
        let extra = if kind == "derived" { 2 } else { 1 };
        if facts.len() + extra != record.len()
            || facts
                .iter()
                .any(|(key, value)| key != "kind" && record.get(key) != Some(value))
        {
            bail!("[malformed-resource] {what} recorded facts do not match its DNG bytes")
        }
    }
    Ok(())
}

fn validate_photo<'a>(
    raw: &Value,
    photo: &'a Value,
    assets: &Map<String, Value>,
    profiles: &Map<String, Value>,
) -> Result<(&'a str, HashSet<&'a str>)> {
    let record = object(photo, "photo")?;
    let photo_id = id(record.get("id"), "photo ID")?;
    let what = format!("photo {photo_id}");
    keys(
        record,
        &[
            "id",
            "source",
            "name",
            "rating",
            "pick",
            "label",
            "keywords",
            "title",
            "caption",
            "creator",
            "copyright",
            "location",
            "variants",
            "snapshots",
        ],
        &what,
    )?;
    let source = record
        .get("source")
        .and_then(Value::as_str)
        .with_context(|| format!("[malformed-resource] {what} source must be a digest"))?;
    digest(source, &format!("{what} source"))?;
    let Some(asset) = assets.get(source) else {
        bail!("[missing-resource] {what} references missing asset {source}")
    };
    let source = super::develop::Source {
        kind: asset["kind"].as_str().unwrap_or("rendered"),
        camera_model: asset["raw"]["unique_camera_model"].as_str(),
        profiles,
        document: Some(raw),
    };
    text(record, "name", 256, &what)?;
    text(record, "title", 1024, &what)?;
    text(record, "caption", 8192, &what)?;
    text(record, "creator", 256, &what)?;
    text(record, "copyright", 1024, &what)?;
    if record.contains_key("rating") {
        integer(record, "rating", (0, 5), &what)?;
    }
    one_of(record, "pick", &["none", "pick", "reject"], &what)?;
    one_of(
        record,
        "label",
        &["none", "red", "yellow", "green", "blue", "purple"],
        &what,
    )?;
    if let Some(keywords) = record.get("keywords") {
        let keywords = keywords
            .as_array()
            .with_context(|| format!("[malformed-resource] {what} keywords must be an array"))?;
        let mut seen = HashSet::new();
        if keywords.len() > MAX_KEYWORDS
            || !keywords.iter().all(|keyword| {
                keyword.as_str().is_some_and(|keyword| {
                    !keyword.is_empty()
                        && keyword.chars().count() <= 64
                        && seen.insert(keyword.to_lowercase())
                })
            })
        {
            bail!("[malformed-resource] {what} keywords must be at most {MAX_KEYWORDS} strings of 1–64 characters, unique ignoring case")
        }
    }
    if let Some(location) = record.get("location") {
        let location = object(location, &format!("{what} location"))?;
        let fields = ["sublocation", "city", "state", "country"];
        keys(location, &fields, &format!("{what} location"))?;
        for field in fields {
            text(location, field, 256, &format!("{what} location"))?;
        }
    }
    let variants = record
        .get("variants")
        .and_then(Value::as_array)
        .with_context(|| format!("[malformed-resource] {what} variants must be an array"))?;
    if variants.is_empty() || variants.len() > MAX_VARIANTS {
        bail!("[malformed-resource] {what} must have 1–{MAX_VARIANTS} variants")
    }
    let mut ids = HashSet::with_capacity(variants.len());
    for (index, variant) in variants.iter().enumerate() {
        let variant = object(variant, &format!("{what} variant"))?;
        let variant_id = id(variant.get("id"), &format!("{what} variant ID"))?;
        let where_ = format!("variant {photo_id}/{variant_id}");
        if index == 0 && variant_id != "master" {
            bail!("[malformed-resource] {what} variants[0] must be master")
        }
        if !ids.insert(variant_id) {
            bail!("[malformed-resource] {where_} is used more than once")
        }
        keys(variant, &["id", "name", "develop"], &where_)?;
        text(variant, "name", 256, &where_)?;
        validate_develop(variant.get("develop"), &source, &where_)?;
    }
    if let Some(snapshots) = record.get("snapshots") {
        let snapshots = snapshots
            .as_array()
            .with_context(|| format!("[malformed-resource] {what} snapshots must be an array"))?;
        if snapshots.len() > MAX_SNAPSHOTS {
            bail!("[limit-exceeded] {what} has more than {MAX_SNAPSHOTS} snapshots")
        }
        let mut seen = HashSet::with_capacity(snapshots.len());
        for snapshot in snapshots {
            let snapshot = object(snapshot, &format!("{what} snapshot"))?;
            let snapshot_id = id(snapshot.get("id"), &format!("{what} snapshot ID"))?;
            let where_ = format!("snapshot {photo_id}/{snapshot_id}");
            if !seen.insert(snapshot_id) {
                bail!("[malformed-resource] {where_} is used more than once")
            }
            keys(snapshot, &["id", "name", "variant", "develop"], &where_)?;
            text(snapshot, "name", 256, &where_)?;
            let variant = id(snapshot.get("variant"), &format!("{where_} variant"))?;
            if !ids.contains(variant) {
                bail!("[missing-resource] {where_} references missing variant {photo_id}/{variant}")
            }
            validate_develop(snapshot.get("develop"), &source, &where_)?;
        }
    }
    Ok((photo_id, ids))
}

/// The develop envelope and the groups implemented so far (`[invalid-develop]`).
fn validate_develop(
    develop: Option<&Value>,
    source: &super::develop::Source,
    what: &str,
) -> Result<()> {
    let develop = develop
        .and_then(Value::as_object)
        .with_context(|| format!("[invalid-develop] {what} develop must be an object"))?;
    match develop.get("process").and_then(Value::as_u64) {
        Some(1) => super::develop::validate(develop, source, what),
        Some(process) if process > 1 => bail!("[unsupported-capability] {what} uses develop process {process}; this build implements process 1"),
        _ => bail!("[invalid-develop] {what} develop process must be a positive integer"),
    }
}

fn is_empty_catalog(catalog: &Value) -> bool {
    catalog.as_object().is_none_or(|catalog| {
        [
            "assets",
            "profiles",
            "photos",
            "stacks",
            "collections",
            "recipes",
        ]
        .iter()
        .all(|key| match catalog.get(*key) {
            None => true,
            Some(Value::Array(values)) => values.is_empty(),
            Some(Value::Object(values)) => values.is_empty(),
            Some(_) => false,
        })
    })
}

/// `migrate --target 7`: the v6 migration followed by `version: 7`. No catalog is
/// added; the first photo command creates it.
pub fn migrate(raw: Value) -> Result<Value> {
    if raw.get("version").and_then(Value::as_u64) == Some(VERSION) {
        crate::scene::validate(&raw)?;
        return Ok(raw);
    }
    let mut result = crate::composite::migrate(raw)?;
    result["version"] = json!(VERSION);
    crate::scene::validate(&result)?;
    Ok(result)
}

/// `migrate --target 6`: only a document without photo nodes and with no (or an
/// empty) catalog can be represented by version 6.
pub fn downgrade(mut raw: Value) -> Result<Value> {
    crate::scene::validate(&raw)?;
    if let Some(node) = photo_nodes(&raw).first() {
        bail!(
            "[unsupported-capability] version 6 cannot represent photo {}; export it as an image first",
            node_id(node)
        )
    }
    if let Some(catalog) = raw.get("photography") {
        if !is_empty_catalog(catalog) {
            match catalog["photos"].get(0).and_then(|photo| photo["id"].as_str()) {
                Some(photo) => bail!("[unsupported-capability] version 6 cannot represent photo {photo}; export it as an image first"),
                None => bail!("[unsupported-capability] version 6 cannot represent the photography catalog; remove its assets, profiles and recipes first"),
            }
        }
    }
    let object = raw.as_object_mut().context("document must be an object")?;
    object.remove("photography");
    object.insert("version".into(), json!(crate::composite::VERSION));
    crate::scene::validate(&raw)?;
    Ok(raw)
}

/// How `raw add` chooses the camera profile of the master variant.
pub fn camera_profile(value: &str) -> Result<Value> {
    match value {
        "auto" | "embedded" => Ok(json!("embedded")),
        "matrix-only" => Ok(json!("matrix-only")),
        profile if profile.starts_with("sha256:") => {
            digest(profile, "camera profile")?;
            Ok(json!({"profile": profile}))
        }
        other => bail!("[invalid-develop] camera profile {other:?} must be auto, embedded, matrix-only or a profile digest"),
    }
}

/// The settings `raw add` writes explicitly into a raw source's master variant.
pub fn raw_import_defaults(camera_profile: Value) -> Value {
    json!({
        "process": 1,
        "raw": {"demosaic": "mhc", "highlights": "blend", "camera_profile": camera_profile},
        "white_balance": {"mode": "as-shot"},
        "lens": {"profile": "embedded-opcodes"},
        "detail": {
            "sharpening": {"amount": 40, "radius": 1, "detail": 25},
            "noise": {"color": 25}
        }
    })
}

/// The document at version 7 with a photography catalog (created empty when
/// absent), and whether it was upgraded.
fn with_catalog(raw: &Value) -> Result<(Value, bool)> {
    let version = raw.get("version").and_then(Value::as_u64);
    let mut document = if version == Some(VERSION) {
        crate::scene::validate(raw)?;
        raw.clone()
    } else {
        migrate(raw.clone())?
    };
    let catalog = document
        .as_object_mut()
        .context("document must be an object")?
        .entry("photography")
        .or_insert_with(|| {
            json!({
                "engine": ENGINE,
                "working_space": WORKING_SPACE,
                "assets": {},
                "profiles": {},
                "photos": []
            })
        });
    catalog
        .as_object_mut()
        .context("[malformed-resource] photography must be an object")?
        .entry("profiles")
        .or_insert_with(|| json!({}));
    Ok((document, version != Some(VERSION)))
}

/// `photo profile add`: verify and store a DNG camera profile (`.dcp`) in
/// `photography.profiles`. `force_model` records that the profile may be used
/// for sources whose `UniqueCameraModel` differs.
pub fn add_profile(raw: &mut Value, bytes: &[u8], force_model: bool) -> Result<Value> {
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        bail!("[limit-exceeded] camera profile is larger than 16 MiB")
    }
    let profile = super::profile::CameraProfile::from_dcp(bytes)?;
    let digest = crate::resource::sha256(bytes);
    let (mut document, upgraded) = with_catalog(raw)?;
    let profiles = document["photography"]["profiles"]
        .as_object_mut()
        .context("[malformed-resource] photography.profiles must be an object")?;
    let deduplicated = profiles.contains_key(&digest);
    if let Some(record) = profiles.get_mut(&digest) {
        if record["kind"] != "camera" {
            bail!(
                "[invalid-input] these bytes are already stored as a {} profile",
                record["kind"]
            )
        }
        if force_model {
            record["force_model"] = json!(true);
        }
    } else {
        if profiles.len() >= MAX_PROFILES {
            bail!("[limit-exceeded] photography already has {MAX_PROFILES} profiles")
        }
        let mut record = profile.record_facts();
        record["byte_length"] = json!(bytes.len());
        record["storage"] = crate::image::embedded_storage(bytes);
        if force_model {
            record["force_model"] = json!(true);
        }
        profiles.insert(digest.clone(), record);
    }
    let record = profiles[&digest].clone();
    crate::scene::validate(&document)?;
    *raw = document;
    Ok(json!({
        "profile": digest,
        "name": record["name"],
        "unique_camera_model": record["unique_camera_model"],
        "embed_policy": record.get("embed_policy"),
        "force_model": record.get("force_model").cloned().unwrap_or(json!(false)),
        "calibrations": profile.calibrations.len(),
        "hue_sat_map": profile.calibrations.iter().any(|c| c.hue_sat.is_some()),
        "look_table": profile.look.is_some(),
        "tone_curve": profile.tone_curve.as_ref().map_or(0, |c| c.points.len()),
        "deduplicated": deduplicated,
        "upgraded": upgraded,
    }))
}

/// The bytes behind a validated storage record: embedded data, or an external
/// document-relative file verified against `digest` (offline).
pub fn stored_bytes(document: &Path, storage: &Value, digest: &str) -> Result<Vec<u8>> {
    match storage["kind"].as_str() {
        Some("embedded") => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(storage["data"].as_str().unwrap_or_default())
                .context("[malformed-resource] embedded data has invalid base64")?;
            crate::resource::verify(&bytes, digest)?;
            Ok(bytes)
        }
        Some("external") => crate::resource::read_external_offline(
            document,
            Path::new(storage["path"].as_str().unwrap_or_default()),
            digest,
        ),
        _ => bail!("[unsupported-capability] storage kind must be embedded or external"),
    }
}

/// White balance requested by `raw develop`.
#[derive(Debug, Clone, PartialEq)]
pub enum WhiteBalance {
    AsShot,
    Temperature {
        temperature: f64,
        tint: f64,
    },
    Neutral([f64; 3]),
    /// Sample a disk: centre x, y in the oriented frame and radius, all 0–1.
    Sample {
        x: f64,
        y: f64,
        radius: f64,
    },
    /// Gray-world suggestion.
    Suggest,
}

/// How `raw develop --lens-profile` chooses lens correction.
pub fn lens_profile(value: &str) -> Result<Value> {
    match value {
        "none" | "embedded-opcodes" => Ok(json!(value)),
        profile if profile.starts_with("sha256:") => {
            digest(profile, "lens profile")?;
            Ok(json!({"profile": profile}))
        }
        other => bail!("[invalid-develop] lens profile {other:?} must be none, embedded-opcodes or a profile digest"),
    }
}

/// Changes requested by `raw develop`, applied in this order: `set` and
/// `unset`, camera and lens profile, white balance, then the upright analysis
/// (which sees every other change).
#[derive(Debug, Clone, Default)]
pub struct DevelopChanges {
    /// Dotted develop paths such as `lens.distortion` and their JSON values.
    pub set: Vec<(String, Value)>,
    pub unset: Vec<String>,
    pub camera_profile: Option<Value>,
    pub lens_profile: Option<Value>,
    pub white_balance: Option<WhiteBalance>,
    /// `off`, `level`, `vertical`, `full` or `guided`.
    pub upright: Option<String>,
    /// Guided upright segments `x1, y1, x2, y2` in the oriented frame (0–1).
    pub guides: Vec<[f64; 4]>,
    /// Resolve `tone.exposure`, `highlights` and `shadows` with auto tone.
    pub auto_tone: bool,
}

/// The groups the dehaze airlight is measured through (stages 1–6). A change
/// to any of them re-resolves a stored airlight.
pub(super) const AIRLIGHT_INPUTS: [&str; 6] = [
    "raw",
    "white_balance",
    "calibration",
    "lens",
    "geometry",
    "tone",
];

impl DevelopChanges {
    pub fn is_empty(&self) -> bool {
        !self.auto_tone
            && self.set.is_empty()
            && self.unset.is_empty()
            && self.camera_profile.is_none()
            && self.lens_profile.is_none()
            && self.white_balance.is_none()
            && self.upright.is_none()
            && self.guides.is_empty()
    }
}

const MAX_PATH_DEPTH: usize = 4;

fn develop_path(path: &str) -> Result<Vec<&str>> {
    let parts: Vec<&str> = path.split('.').collect();
    let valid = parts.len() <= MAX_PATH_DEPTH
        && parts.iter().all(|p| {
            !p.is_empty() && p.len() <= 32 && p.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        });
    if !valid {
        bail!("[invalid-input] develop path {path:?} must be 1–{MAX_PATH_DEPTH} dot-separated lowercase keys such as lens.distortion")
    }
    if parts[0] == "process" {
        bail!("[invalid-input] develop path {path:?}: process is fixed by the engine")
    }
    Ok(parts)
}

fn set_path(develop: &mut Value, path: &str, value: Value) -> Result<()> {
    let parts = develop_path(path)?;
    let mut node = develop;
    for part in &parts[..parts.len() - 1] {
        if !node.get(*part).is_some_and(Value::is_object) {
            node[*part] = json!({});
        }
        node = node.get_mut(*part).unwrap();
    }
    node[parts[parts.len() - 1]] = value;
    Ok(())
}

fn unset_path(develop: &mut Value, path: &str) -> Result<()> {
    let parts = develop_path(path)?;
    fn remove(node: &mut Value, parts: &[&str]) -> bool {
        let Some(object) = node.as_object_mut() else {
            return false;
        };
        if parts.len() == 1 {
            return object.remove(parts[0]).is_some();
        }
        let Some(child) = object.get_mut(parts[0]) else {
            return false;
        };
        let removed = remove(child, &parts[1..]);
        if removed && child.as_object().is_some_and(Map::is_empty) {
            object.remove(parts[0]);
        }
        removed
    }
    if !remove(develop, &parts) {
        bail!("[invalid-input] develop path {path} is not set; nothing to unset")
    }
    Ok(())
}

/// Apply an upright mode to `develop["geometry"]`: `off` clears upright, its
/// provenance, guides, rotation and perspective; another mode stores the
/// solved values and clears the keys it does not solve.
pub(super) fn apply_upright(develop: &mut Value, mode: &str, solved: &Value) {
    if !develop.get("geometry").is_some_and(Value::is_object) {
        develop["geometry"] = json!({});
    }
    let geometry = develop["geometry"].as_object_mut().unwrap();
    for key in ["upright", "auto", "rotate", "vertical", "horizontal"] {
        geometry.remove(key);
    }
    if mode != "guided" {
        geometry.remove("guides");
    }
    if mode != "off" {
        geometry.insert("upright".into(), json!(mode));
        for key in ["rotate", "vertical", "horizontal"] {
            if let Some(v) = solved.get(key) {
                geometry.insert(key.into(), v.clone());
            }
        }
        let algorithm = if mode == "guided" {
            "upright-guided"
        } else {
            "upright-hough"
        };
        geometry.insert("auto".into(), json!({"algorithm": algorithm, "version": 1}));
    }
    if geometry.is_empty() {
        develop.as_object_mut().unwrap().remove("geometry");
    }
}

/// `raw develop`: change a variant's develop settings. The result reports the
/// resolved white (xy, temperature, tint and camera neutral), the upright
/// analysis when one ran, and the resolved frame (sizes, crop, lens steps).
pub fn develop_raw(
    raw: &mut Value,
    document: &Path,
    photo_id: &str,
    variant_id: &str,
    changes: DevelopChanges,
) -> Result<Value> {
    use super::profile;
    let DevelopChanges {
        set,
        unset,
        camera_profile,
        lens_profile,
        white_balance,
        upright,
        guides,
        auto_tone,
    } = changes;
    crate::scene::validate(raw)?;
    let catalog = raw.get("photography").context(
        "[missing-resource] the document has no photography catalog; add a photo with `raw add` first",
    )?;
    let photo_index = catalog["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .position(|photo| photo["id"] == photo_id)
        .with_context(|| format!("[missing-resource] photo {photo_id} is not in the catalog"))?;
    let photo = &catalog["photos"][photo_index];
    let variant_index = photo["variants"]
        .as_array()
        .into_iter()
        .flatten()
        .position(|variant| variant["id"] == variant_id)
        .with_context(|| {
            format!("[missing-resource] variant {photo_id}/{variant_id} does not exist")
        })?;
    let source = photo["source"].as_str().unwrap_or_default();
    let asset = &catalog["assets"][source];
    if asset["kind"] == "rendered" {
        bail!("[invalid-develop] photo {photo_id} has a rendered source; `raw develop` applies to raw sources")
    }
    if !guides.is_empty() && upright.as_deref() != Some("guided") {
        bail!("[invalid-input] --guide segments need --upright guided")
    }
    if let Some(mode) = upright.as_deref() {
        if !["off", "level", "vertical", "full", "guided"].contains(&mode) {
            bail!("[invalid-input] upright {mode:?} must be off, level, vertical, full or guided")
        }
    }
    let bytes = stored_bytes(document, &asset["storage"], source)
        .with_context(|| format!("photo {photo_id} source"))?;
    let dng = Dng::inspect(&bytes)?;
    let mut develop = photo["variants"][variant_index]["develop"].clone();
    let before = develop.clone();
    let touches = |group: &str| {
        let under = |path: &String| path == group || path.starts_with(&format!("{group}."));
        unset.iter().any(under) || set.iter().any(|(path, _)| under(path))
    };
    // A hand-edited tone value is no longer the auto tone result.
    let tone_edited = touches("tone")
        && !unset
            .iter()
            .chain(set.iter().map(|(p, _)| p))
            .all(|p| p == "tone.auto");
    let airlight_unset = unset.iter().any(|p| p == "presence.dehaze_airlight");
    for path in &unset {
        unset_path(&mut develop, path)?;
    }
    for (path, value) in set {
        set_path(&mut develop, &path, value)?;
    }
    if let Some(camera_profile) = camera_profile {
        develop["raw"]["camera_profile"] = camera_profile;
    }
    if let Some(lens_profile) = lens_profile {
        if !develop.get("lens").is_some_and(Value::is_object) {
            develop["lens"] = json!({});
        }
        develop["lens"]["profile"] = lens_profile;
    }
    if !guides.is_empty() {
        if !develop.get("geometry").is_some_and(Value::is_object) {
            develop["geometry"] = json!({});
        }
        develop["geometry"]["upright"] = json!("guided");
        develop["geometry"]["guides"] = guides
            .iter()
            .map(|g| {
                json!([
                    [super::dng::number(g[0]), super::dng::number(g[1])],
                    [super::dng::number(g[2]), super::dng::number(g[3])]
                ])
            })
            .collect();
    }
    let loader = profile_loader(document, &catalog["profiles"]);
    let load = |digest: &str| loader(digest).map(|(bytes, _)| bytes);
    // Validate the requested settings before reading any profile bytes.
    let mut document_value = raw.clone();
    document_value["photography"]["photos"][photo_index]["variants"][variant_index]["develop"] =
        develop.clone();
    crate::scene::validate(&document_value)?;
    let spec = profile::select(develop["raw"].get("camera_profile"), &dng, &load)?;
    let monochrome = spec.is_monochrome();
    let measured = matches!(
        white_balance,
        Some(WhiteBalance::Sample { .. } | WhiteBalance::Suggest | WhiteBalance::Neutral(_))
    );
    if monochrome && measured {
        bail!("[invalid-develop] photo {photo_id} is a monochrome DNG and has no white balance to sample or suggest; use --as-shot")
    }
    let neutral_value = |n: [f64; 3]| -> Value {
        Value::Array(
            n.iter()
                .map(|v| super::dng::number(((v * 1.0e6).round() / 1.0e6).max(1.0e-6)))
                .collect(),
        )
    };
    let stored = match white_balance {
        None => None,
        Some(WhiteBalance::AsShot) => Some(json!({"mode": "as-shot"})),
        Some(WhiteBalance::Temperature { temperature, tint }) => Some(json!({
            "mode": "temperature",
            "temperature": super::dng::number(temperature),
            "tint": super::dng::number(tint),
        })),
        Some(WhiteBalance::Neutral(n)) => {
            if n.iter().any(|v| !(*v > 0.0 && *v <= 1.0e6)) {
                bail!("[invalid-develop] variant {photo_id}/{variant_id} white_balance.neutral {n:?} must hold three positive numbers up to 1e6")
            }
            Some(json!({"mode": "neutral", "neutral": neutral_value(n)}))
        }
        Some(WhiteBalance::Sample { x, y, radius }) => {
            let n = profile::sample(&dng, &develop, x, y, radius)?;
            Some(json!({
                "mode": "neutral",
                "neutral": neutral_value(n),
                "sampled": {
                    "x": super::dng::number(x),
                    "y": super::dng::number(y),
                    "radius": super::dng::number(radius),
                },
            }))
        }
        Some(WhiteBalance::Suggest) => {
            let n = profile::suggest_neutral(&dng, &develop)?;
            Some(profile::suggested(&spec, n)?)
        }
    };
    if let Some(stored) = stored {
        develop["white_balance"] = stored;
    }
    let mut analysis = Value::Null;
    if let Some(mode) = upright.as_deref() {
        let solved = if mode == "off" {
            json!({})
        } else {
            super::pipeline::upright(
                &dng,
                &develop,
                mode,
                develop["geometry"].get("guides"),
                &loader,
            )?
        };
        apply_upright(&mut develop, mode, &solved);
        analysis = solved;
    }
    if tone_edited && !auto_tone {
        if let Some(tone) = develop.get_mut("tone").and_then(Value::as_object_mut) {
            tone.remove("auto");
        }
    }
    let mut resolved_tone = Value::Null;
    if auto_tone {
        document_value["photography"]["photos"][photo_index]["variants"][variant_index]
            ["develop"] = develop.clone();
        crate::scene::validate(&document_value)?;
        develop["tone"] = super::pipeline::auto_tone(&dng, &develop, &loader)?;
        resolved_tone = develop["tone"].clone();
    }
    let mut resolved_airlight = Value::Null;
    let dehaze = develop["presence"]
        .get("dehaze")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if dehaze != 0.0 || super::local::uses_dehaze(&develop) {
        let stored = develop["presence"].get("dehaze_airlight").is_some();
        let changed = AIRLIGHT_INPUTS
            .iter()
            .any(|group| before.get(*group) != develop.get(*group));
        if !stored || changed || airlight_unset {
            document_value["photography"]["photos"][photo_index]["variants"][variant_index]
                ["develop"] = develop.clone();
            if let Some(presence) = document_value["photography"]["photos"][photo_index]["variants"]
                [variant_index]["develop"]
                .get_mut("presence")
                .and_then(Value::as_object_mut)
            {
                presence.remove("dehaze_airlight");
            }
            crate::scene::validate(&document_value)?;
            let airlight = super::pipeline::resolve_airlight(&dng, &develop, &loader)?;
            develop["presence"]["dehaze_airlight"] = airlight.clone();
            resolved_airlight = airlight;
        }
    }
    document_value["photography"]["photos"][photo_index]["variants"][variant_index]["develop"] =
        develop.clone();
    crate::scene::validate(&document_value)?;
    let (white, _) = profile::resolve(&spec, &dng, develop.get("white_balance"))?;
    let plan = super::pipeline::plan(&dng, &develop, &loader)?;
    let mut result = json!({
        "photo": photo_id,
        "variant": variant_id,
        "camera_profile": develop["raw"].get("camera_profile").cloned().unwrap_or(json!("embedded")),
        "lens_profile": develop["lens"].get("profile").cloned().unwrap_or(json!("none")),
        "white_balance": develop.get("white_balance").cloned().unwrap_or(json!({"mode": "as-shot"})),
        "monochrome": monochrome,
        "resolved": white.report(),
        "frame": plan.report,
    });
    if !analysis.is_null() {
        result["upright"] = analysis;
    }
    if !resolved_tone.is_null() {
        result["tone"] = resolved_tone;
    }
    if !resolved_airlight.is_null() {
        result["dehaze_airlight"] = resolved_airlight;
    }
    drop(loader);
    *raw = document_value;
    Ok(result)
}

/// Resolves a profile digest of `photography.profiles` to its verified bytes
/// and record.
pub(super) fn profile_loader<'a>(
    document: &'a Path,
    profiles: &'a Value,
) -> impl Fn(&str) -> Result<(Vec<u8>, Value)> + 'a {
    move |digest: &str| {
        let record = profiles.get(digest).with_context(|| {
            format!("[missing-resource] profile {digest} is not in photography.profiles; add it with `photo profile add` first")
        })?;
        let bytes = stored_bytes(document, &record["storage"], digest)?;
        Ok((bytes, record.clone()))
    }
}

/// A photo variant and its decoded source, located for rendering or inspection.
fn locate<'a>(
    raw: &'a Value,
    document: &Path,
    photo_id: &str,
    variant_id: &str,
) -> Result<(&'a Value, Vec<u8>, &'a Value)> {
    crate::scene::validate(raw)?;
    let catalog = raw.get("photography").context(
        "[missing-resource] the document has no photography catalog; add a photo with `raw add` first",
    )?;
    let photo = catalog["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|photo| photo["id"] == photo_id)
        .with_context(|| format!("[missing-resource] photo {photo_id} is not in the catalog"))?;
    let variant = photo["variants"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|variant| variant["id"] == variant_id)
        .with_context(|| {
            format!("[missing-resource] variant {photo_id}/{variant_id} does not exist")
        })?;
    let source = photo["source"].as_str().unwrap_or_default();
    let asset = &catalog["assets"][source];
    if asset["kind"] == "rendered" {
        bail!("[unsupported-capability] photo {photo_id} has a rendered source; photo development currently renders raw sources only")
    }
    let bytes = stored_bytes(document, &asset["storage"], source)
        .with_context(|| format!("photo {photo_id} source"))?;
    Ok((&variant["develop"], bytes, &catalog["profiles"]))
}

/// `photo render`: develop a variant into the working space.
pub fn render_photo(
    raw: &Value,
    document: &Path,
    photo_id: &str,
    variant_id: &str,
) -> Result<super::pipeline::Developed> {
    let (develop, bytes, profiles) = locate(raw, document, photo_id, variant_id)?;
    let dng = Dng::inspect(&bytes)?;
    let loader = profile_loader(document, profiles);
    let masks = super::local::Lookup { raw, document };
    super::pipeline::develop(&dng, develop, &loader, Some(&masks))
}

/// `photo info`: the resolved frame of a variant without decoding pixels,
/// plus the number of output pixels whose source lies outside the image.
pub fn photo_info(raw: &Value, document: &Path, photo_id: &str, variant_id: &str) -> Result<Value> {
    let (develop, bytes, profiles) = locate(raw, document, photo_id, variant_id)?;
    let dng = Dng::inspect(&bytes)?;
    let loader = profile_loader(document, profiles);
    let plan = super::pipeline::plan(&dng, develop, &loader)?;
    let invalid = super::warp::count_invalid(&plan.mapping, plan.crop)?;
    let mut report = plan.report;
    report["photo"] = json!(photo_id);
    report["variant"] = json!(variant_id);
    report["invalid_pixels"] = json!(invalid);
    Ok(report)
}

/// A brush stroke for `photo mask paint`. Samples are in brush-plane pixels.
pub struct MaskStroke {
    pub adjustment: String,
    /// The brush component to paint into; `None` appends a new one.
    pub component: Option<usize>,
    pub request: crate::raster::StrokeRequest,
}

/// `photo mask paint`: paint a brush mask component of a local adjustment with
/// the raster brush engine. The plane is the uncropped frame scaled to a long
/// edge of at most 4096 pixels; its tiles live in `raster_tiles`.
pub fn paint_mask(
    raw: &mut Value,
    document: &Path,
    photo_id: &str,
    variant_id: &str,
    stroke: MaskStroke,
) -> Result<Value> {
    use crate::raster;
    let frame = {
        let (develop, bytes, profiles) = locate(raw, document, photo_id, variant_id)?;
        let dng = Dng::inspect(&bytes)?;
        let loader = profile_loader(document, profiles);
        super::pipeline::plan(&dng, develop, &loader)?.mapping.frame
    };
    let (width, height) = super::local::brush_size(frame);
    let mut next = raw.clone();
    let MaskStroke {
        adjustment,
        component,
        mut request,
    } = stroke;
    let photo = next["photography"]["photos"]
        .as_array_mut()
        .and_then(|photos| photos.iter_mut().find(|p| p["id"] == photo_id))
        .context("[missing-resource] photo is not in the catalog")?;
    let develop = photo["variants"]
        .as_array_mut()
        .and_then(|variants| variants.iter_mut().find(|v| v["id"] == variant_id))
        .map(|variant| &mut variant["develop"])
        .context("[missing-resource] variant does not exist")?;
    let entry = develop["local"]
        .as_array()
        .into_iter()
        .flatten()
        .position(|a| a["id"] == adjustment.as_str())
        .with_context(|| {
            format!("[missing-resource] variant {photo_id}/{variant_id} has no local adjustment {adjustment}; add it with `raw develop --set local=[...]` first")
        })?;
    let components_len = develop["local"][entry]["mask"]["components"]
        .as_array()
        .map_or(0, Vec::len);
    let (index, existing) = match component {
        Some(index) => {
            let current = develop["local"][entry]["mask"]["components"]
                .get(index)
                .with_context(|| {
                    format!(
                        "[invalid-input] local adjustment {adjustment} has no component {index}"
                    )
                })?;
            if current["kind"] != "brush" {
                bail!("[invalid-input] component {index} of local adjustment {adjustment} is a {} component; paint into a brush component or omit --component", current["kind"])
            }
            if current["width"] != width || current["height"] != height {
                bail!("[invalid-input] brush component {index} is {}x{} but the frame's brush plane is {width}x{height}; the frame changed, so paint a new component", current["width"], current["height"])
            }
            (index, Some(current.clone()))
        }
        None => {
            if components_len >= super::local::MAX_COMPONENTS {
                bail!(
                    "[limit-exceeded] local adjustment {adjustment} already has {} mask components",
                    super::local::MAX_COMPONENTS
                )
            }
            (components_len, None)
        }
    };
    let tiles = existing.as_ref().map_or(json!({}), |c| c["tiles"].clone());
    let mut surface = raster::Surface::load_map(raw, width, height, &tiles)?;
    let tip = raster::resolve_tip(raw, &mut request.brush)?;
    let painted = raster::Stroke {
        brush: request.brush.clone(),
        samples: request.samples.clone(),
        color: [0, 0, 0],
        blend: request.blend,
        seed: request.seed,
        tip,
        clone: None,
    };
    let result = raster::apply_stroke(&mut surface, &painted)?;
    let mut holder = json!({});
    surface.store(&mut next, &mut holder)?;
    let develop = next["photography"]["photos"]
        .as_array_mut()
        .and_then(|photos| photos.iter_mut().find(|p| p["id"] == photo_id))
        .and_then(|photo| {
            photo["variants"]
                .as_array_mut()?
                .iter_mut()
                .find(|v| v["id"] == variant_id)
        })
        .map(|variant| &mut variant["develop"])
        .context("[missing-resource] variant does not exist")?;
    let components = develop["local"][entry]["mask"]["components"]
        .as_array_mut()
        .context("[invalid-develop] local adjustment mask components must be an array")?;
    let mut value = existing.unwrap_or_else(
        || json!({"kind": "brush", "mode": "add", "width": width, "height": height}),
    );
    value["tiles"] = holder.get("tiles").cloned().unwrap_or(json!({}));
    if index == components.len() {
        components.push(value);
    } else {
        components[index] = value;
    }
    let released = raster::collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "photo": photo_id,
        "variant": variant_id,
        "adjustment": adjustment,
        "component": index,
        "plane": [width, height],
        "frame": [frame.0, frame.1],
        "dabs": result.dabs,
        "bounds": result.bounds,
        "tiles_changed": result.tiles_changed,
        "tiles": surface.tiles.len(),
        "tiles_released": released,
    }))
}

/// `photo profile add --lens`: verify and store a Pentool lens profile.
pub fn add_lens_profile(raw: &mut Value, bytes: &[u8]) -> Result<Value> {
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        bail!("[limit-exceeded] lens profile is larger than 16 MiB")
    }
    let lens = super::lens::LensProfile::parse(bytes)?;
    store_lens(raw, bytes, &lens, "pentool-lens", Vec::new())
}

/// `photo profile import-lcp`: convert the supported subset of an Adobe lens
/// correction profile and store the canonical Pentool lens profile, listing
/// every model that was not converted.
pub fn import_lcp(raw: &mut Value, bytes: &[u8]) -> Result<Value> {
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        bail!("[limit-exceeded] lens correction profile is larger than 16 MiB")
    }
    let converted = super::lens::import_lcp(bytes)?;
    let canonical = converted.profile.to_bytes();
    store_lens(
        raw,
        &canonical,
        &converted.profile,
        "lcp",
        converted.unsupported,
    )
}

fn store_lens(
    raw: &mut Value,
    bytes: &[u8],
    lens: &super::lens::LensProfile,
    imported_from: &str,
    unsupported: Vec<String>,
) -> Result<Value> {
    let digest = crate::resource::sha256(bytes);
    let (mut document, upgraded) = with_catalog(raw)?;
    let profiles = document["photography"]["profiles"]
        .as_object_mut()
        .context("[malformed-resource] photography.profiles must be an object")?;
    let deduplicated = profiles.contains_key(&digest);
    if let Some(record) = profiles.get(&digest) {
        if record["kind"] != "lens" {
            bail!(
                "[invalid-input] these bytes are already stored as a {} profile",
                record["kind"]
            )
        }
    } else {
        if profiles.len() >= MAX_PROFILES {
            bail!("[limit-exceeded] photography already has {MAX_PROFILES} profiles")
        }
        let mut record = json!({
            "kind": "lens",
            "name": lens.name(),
            "imported_from": imported_from,
            "byte_length": bytes.len(),
            "storage": crate::image::embedded_storage(bytes),
        });
        if !unsupported.is_empty() {
            record["unsupported"] = json!(unsupported);
        }
        profiles.insert(digest.clone(), record);
    }
    let record = profiles[&digest].clone();
    crate::scene::validate(&document)?;
    *raw = document;
    Ok(json!({
        "profile": digest,
        "kind": "lens",
        "name": record["name"],
        "imported_from": record["imported_from"],
        "samples": lens.samples.len(),
        "unsupported": record.get("unsupported").cloned().unwrap_or(json!([])),
        "deduplicated": deduplicated,
        "upgraded": upgraded,
    }))
}

/// Which merge `photo merge-hdr` or `photo merge-pano` runs.
pub enum MergeKind {
    Hdr(super::merge::HdrOptions),
    Pano(super::merge::PanoOptions),
}

/// A merge request: the new photo, its inputs (photo IDs, in order) and options.
pub struct MergeRequest<'a> {
    pub photo_id: &'a str,
    pub name: &'a str,
    pub inputs: &'a [String],
    pub kind: MergeKind,
    /// `--settings first`: start the master from the first input's look.
    pub settings_first: bool,
    /// A normalized document-relative path for `--external`; `None` embeds.
    pub external: Option<PathBuf>,
}

/// The develop sections a merge reads from each input: stages 1–3.
const MERGE_INPUT_KEYS: &[&str] = &["process", "raw", "white_balance", "detail", "calibration"];
/// The sections `--settings first` copies from the first input's master. Raw,
/// white balance, calibration and stage-2 detail were applied to the inputs;
/// lens corrections need capture facts a derived DNG does not carry; geometry
/// and local masks are framed on one input. Panoramas also drop the crop.
const MERGE_FIRST_KEYS: &[&str] = &[
    "tone",
    "presence",
    "curves",
    "hsl",
    "grading",
    "monochrome",
    "effects",
];

/// An external output: its document-relative path and bytes.
pub type ExternalFile = (PathBuf, Vec<u8>);

/// `photo merge-hdr` and `photo merge-pano`: merge photos into a new photo
/// whose source is a derived DNG. Returns the result and, for `--external`,
/// the file to write with the document commit.
pub fn merge(
    raw: &mut Value,
    document: &Path,
    request: MergeRequest,
) -> Result<(Value, Option<ExternalFile>)> {
    use super::merge as m;
    let MergeRequest {
        photo_id,
        name,
        inputs,
        kind,
        settings_first,
        external,
    } = request;
    if !is_id(photo_id) {
        bail!("[malformed-resource] photo ID {photo_id:?} must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$")
    }
    let (operation, scale, limit) = match &kind {
        MergeKind::Hdr(o) => ("merge-hdr", o.scale, m::MAX_HDR_INPUTS),
        MergeKind::Pano(o) => ("merge-pano", o.scale, m::MAX_PANO_INPUTS),
    };
    if !(scale.is_finite() && scale > 0.0 && scale <= 1.0) {
        bail!("[invalid-input] --scale must be greater than 0 and at most 1")
    }
    if !(2..=limit).contains(&inputs.len()) {
        bail!(
            "[invalid-input] {operation} takes 2 to {limit} photos, not {}",
            inputs.len()
        )
    }
    let (mut next, upgraded) = with_catalog(raw)?;
    let catalog = &next["photography"];
    let photos = catalog["photos"].as_array().cloned().unwrap_or_default();
    if photos.iter().any(|photo| photo["id"] == photo_id) {
        bail!("[invalid-input] photo ID {photo_id} is already used; choose a unique ID")
    }
    let mut sources = Vec::with_capacity(inputs.len());
    let mut total_pixels = 0u64;
    for (index, input) in inputs.iter().enumerate() {
        if inputs[..index].contains(input) {
            bail!("[invalid-input] photo {input} is given twice; each merge input must be a different photo")
        }
        let photo = photos
            .iter()
            .find(|photo| photo["id"] == input.as_str())
            .with_context(|| format!("[missing-resource] photo {input} is not in the catalog"))?;
        let digest = photo["source"].as_str().unwrap_or_default().to_owned();
        let asset = &catalog["assets"][&digest];
        if asset["kind"] == "rendered" {
            bail!("[unsupported-capability] photo {input} has a rendered source; merges read raw and derived sources only")
        }
        total_pixels += asset["pixel_width"].as_u64().unwrap_or(0)
            * asset["pixel_height"].as_u64().unwrap_or(0);
        let master = photo["variants"]
            .as_array()
            .and_then(|variants| variants.iter().find(|v| v["id"] == "master"))
            .or_else(|| photo["variants"].get(0))
            .with_context(|| format!("[malformed-resource] photo {input} has no variants"))?;
        let mut develop = json!({});
        for key in MERGE_INPUT_KEYS {
            if let Some(value) = master["develop"].get(*key) {
                develop[*key] = value.clone();
            }
        }
        sources.push((
            digest,
            asset["storage"].clone(),
            develop,
            master["develop"].clone(),
        ));
    }
    if total_pixels > m::MAX_INPUT_PIXELS {
        bail!(
            "[limit-exceeded] the merge inputs hold {total_pixels} pixels; the limit is {}",
            m::MAX_INPUT_PIXELS
        )
    }
    if let MergeKind::Hdr(_) = &kind {
        // Brackets share the first input's size, so the output size is known.
        let asset = &catalog["assets"][&sources[0].0];
        let (w, h) = m::scaled(
            asset["pixel_width"].as_u64().unwrap_or(0) as usize,
            asset["pixel_height"].as_u64().unwrap_or(0) as usize,
            scale,
        );
        m::check_output(w as u64, h as u64, scale)?;
    }
    let loader = profile_loader(document, &catalog["profiles"]);
    let merge_inputs: Vec<m::Input> = sources
        .iter()
        .map(|(digest, storage, develop, _)| m::Input {
            load: Box::new(move || stored_bytes(document, storage, digest)),
            develop: develop.clone(),
        })
        .collect();
    let merged = match &kind {
        MergeKind::Hdr(options) => m::hdr(&merge_inputs, &loader, options)?,
        MergeKind::Pano(options) => m::pano(&merge_inputs, &loader, options)?,
    };
    drop(merge_inputs);
    drop(loader);
    let bytes = super::dngout::LinearDng {
        width: merged.width,
        height: merged.height,
        rgb: &merged.rgb,
        alpha: merged.alpha.as_deref(),
        model: &format!("pentool {operation}"),
    }
    .write()?;
    if bytes.len() as u64 > MAX_PHOTO_SOURCE_BYTES {
        bail!("[limit-exceeded] the merged DNG is larger than 512 MiB; pass a smaller --scale")
    }
    if external.is_none() && bytes.len() as u64 > crate::image::MAX_SOURCE_BYTES {
        bail!("[limit-exceeded] the merged DNG is {} bytes, above the 128 MiB embedding limit; pass --external with a document-relative path", bytes.len())
    }
    let digest = crate::resource::sha256(&bytes);
    let dng = Dng::inspect(&bytes)?;
    let mut record = dng.asset_facts();
    record["kind"] = json!("derived");
    record["derived"] = json!({
        "operation": operation,
        "algorithm": m::ALGORITHM,
        "inputs": sources.iter().map(|s| s.0.clone()).collect::<Vec<_>>(),
        "settings": merged.settings,
        "alignment": merged.alignment,
    });
    record["storage"] = match &external {
        Some(path) => crate::image::external_storage(path)?,
        None => crate::image::embedded_storage(&bytes),
    };
    let mut develop = raw_import_defaults(json!("embedded"));
    develop["detail"] = json!({"sharpening": develop["detail"]["sharpening"]});
    let mut copied = Vec::new();
    if settings_first {
        let first = &sources[0].3;
        let mut keys = MERGE_FIRST_KEYS.to_vec();
        if matches!(kind, MergeKind::Hdr(_)) {
            keys.push("crop");
        }
        for key in keys {
            if let Some(value) = first.get(key) {
                develop[key] = value.clone();
                copied.push(key);
            }
        }
        if let Some(sharpening) = first["detail"].get("sharpening") {
            develop["detail"]["sharpening"] = sharpening.clone();
            copied.push("detail.sharpening");
        }
    }
    let catalog = &mut next["photography"];
    let assets = catalog["assets"]
        .as_object_mut()
        .context("[malformed-resource] photography assets must be an object")?;
    let deduplicated = assets.contains_key(&digest);
    let resource = match external {
        Some(path) if !deduplicated => Some((path, bytes.clone())),
        _ => None,
    };
    if !deduplicated {
        assets.insert(digest.clone(), record.clone());
    }
    catalog["photos"]
        .as_array_mut()
        .context("[malformed-resource] photography photos must be an array")?
        .push(json!({
            "id": photo_id,
            "source": digest,
            "name": name,
            "variants": [{"id": "master", "name": "Master", "develop": develop}]
        }));
    crate::scene::validate(&next)?;
    *raw = next;
    Ok((
        json!({
            "photo": photo_id,
            "asset": digest,
            "operation": operation,
            "inputs": inputs,
            "size": [merged.width, merged.height],
            "transparent": merged.alpha.is_some(),
            "bytes": bytes.len(),
            "storage": record["storage"]["kind"],
            "settings": record["derived"]["settings"],
            "alignment": record["derived"]["alignment"],
            "settings_copied": copied,
            "deduplicated": deduplicated,
            "upgraded": upgraded,
            "variant": "master",
        }),
        resource,
    ))
}

/// Add a DNG source as photo `id`, upgrading the document to version 7 when needed.
pub fn add_raw(
    raw: &mut Value,
    photo_id: &str,
    name: &str,
    bytes: &[u8],
    storage: Value,
    camera_profile: Value,
) -> Result<Value> {
    if !is_id(photo_id) {
        bail!("[malformed-resource] photo ID {photo_id:?} must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,63}}$")
    }
    if bytes.len() as u64 > MAX_PHOTO_SOURCE_BYTES {
        bail!("[limit-exceeded] photo source exceeds the 512 MiB limit")
    }
    let dng = Dng::inspect(bytes)?;
    let digest = crate::resource::sha256(bytes);
    let (mut document, upgraded) = with_catalog(raw)?;
    let catalog = &mut document["photography"];
    if let Some(profile) = camera_profile.get("profile").and_then(Value::as_str) {
        if catalog["profiles"].get(profile).is_none() {
            bail!("[missing-resource] camera profile {profile} is not in photography.profiles; add it with `photo profile add` first")
        }
    }
    let photos = catalog["photos"]
        .as_array()
        .context("[malformed-resource] photography photos must be an array")?;
    if photos.iter().any(|photo| photo["id"] == photo_id) {
        bail!("[invalid-input] photo ID {photo_id} is already used; choose a unique ID")
    }
    let assets = catalog["assets"]
        .as_object_mut()
        .context("[malformed-resource] photography assets must be an object")?;
    let deduplicated = assets.contains_key(&digest);
    if !deduplicated {
        let mut record = dng.asset_facts();
        record["storage"] = storage;
        assets.insert(digest.clone(), record);
    }
    let photo = json!({
        "id": photo_id,
        "source": digest,
        "name": name,
        "variants": [{
            "id": "master",
            "name": "Master",
            "develop": raw_import_defaults(camera_profile)
        }]
    });
    catalog["photos"]
        .as_array_mut()
        .unwrap()
        .push(photo.clone());
    crate::scene::validate(&document)?;
    let unsupported = dng
        .unsupported_opcodes()
        .iter()
        .map(|op| json!({"list": op.list, "id": op.id}))
        .collect::<Vec<_>>();
    *raw = document;
    Ok(json!({
        "photo": photo_id,
        "asset": digest,
        "deduplicated": deduplicated,
        "upgraded": upgraded,
        "variant": "master",
        "unsupported_opcodes": unsupported,
    }))
}

/// `raw info`: a photo's recorded source facts, storage and variants, without
/// embedded bytes or develop settings.
pub fn info(raw: &Value, photo_id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    let catalog = raw
        .get("photography")
        .context("[missing-resource] the document has no photography catalog")?;
    let photo = catalog["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|photo| photo["id"] == photo_id)
        .with_context(|| format!("[missing-resource] photo {photo_id} is not in the catalog"))?;
    let source = photo["source"].as_str().unwrap();
    let mut asset = catalog["assets"][source].clone();
    let storage = asset
        .as_object_mut()
        .unwrap()
        .remove("storage")
        .unwrap_or_default();
    let storage = match storage["kind"].as_str() {
        Some("external") => json!({"kind": "external", "path": storage["path"]}),
        kind => json!({"kind": kind}),
    };
    let variants = photo["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variant| json!({"id": variant["id"], "name": variant.get("name")}))
        .collect::<Vec<_>>();
    let snapshots = photo
        .get("snapshots")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    Ok(json!({
        "photo": photo_id,
        "name": photo.get("name"),
        "asset": source,
        "storage": storage,
        "source": asset,
        "variants": variants,
        "snapshots": snapshots,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_the_catalog_pattern() {
        for valid in ["master", "a", "Hero_1.v-2", &"x".repeat(64)] {
            assert!(is_id(valid), "{valid}");
        }
        for invalid in ["", "-a", ".a", "a b", "a/b", &"x".repeat(65), "é"] {
            assert!(!is_id(invalid), "{invalid}");
        }
    }

    #[test]
    fn empty_catalogs_downgrade() {
        assert!(is_empty_catalog(
            &json!({"engine": 1, "working_space": WORKING_SPACE, "assets": {}, "photos": []})
        ));
        assert!(!is_empty_catalog(&json!({"recipes": {"a": {}}})));
    }
}

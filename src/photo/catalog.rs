//! The version 7 photography catalog (`docs/photography-v1.md`): structural
//! validation, photo node references, migration to and from version 6, and the
//! `raw add` / `raw info` operations.
use super::dng::Dng;
use super::VERSION;
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const ENGINE: u64 = 1;
pub const WORKING_SPACE: &str = "prophoto-linear";
/// The largest photo source, embedded or external.
pub const MAX_PHOTO_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_PROFILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ASSETS: usize = 10_000;
const MAX_PROFILES: usize = 256;
const MAX_PHOTOS: usize = 10_000;
const MAX_VARIANTS: usize = 64;
const MAX_SNAPSHOTS: usize = 64;
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
fn photo_nodes(raw: &Value) -> Vec<&Map<String, Value>> {
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
        validate_storage(profile.get("storage"), key, length, &what, &mut embedded)?;
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
        let (photo_id, variants) = validate_photo(photo, assets)?;
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
        for (name, stack) in stacks {
            let what = format!("stack {name}");
            id(Some(&json!(name)), "stack ID")?;
            let stack = object(stack, &what)?;
            keys(stack, &["photos"], &what)?;
            known(&stack["photos"], &what, 2)?;
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
    // A raw source's recorded facts must be the facts its bytes produce.
    if let (Some(bytes), Some("raw")) = (bytes, kind) {
        let facts = Dng::inspect(&bytes)
            .with_context(|| what.clone())?
            .asset_facts();
        let facts = facts.as_object().unwrap();
        if facts.len() + 1 != record.len()
            || facts
                .iter()
                .any(|(key, value)| record.get(key) != Some(value))
        {
            bail!("[malformed-resource] {what} recorded facts do not match its DNG bytes")
        }
    }
    Ok(())
}

fn validate_photo<'a>(
    photo: &'a Value,
    assets: &Map<String, Value>,
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
    if !assets.contains_key(source) {
        bail!("[missing-resource] {what} references missing asset {source}")
    }
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
                    !keyword.is_empty() && keyword.chars().count() <= 64 && seen.insert(keyword)
                })
            })
        {
            bail!("[malformed-resource] {what} keywords must be at most {MAX_KEYWORDS} unique strings of 1–64 characters")
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
        validate_develop(variant.get("develop"), &where_)?;
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
            validate_develop(snapshot.get("develop"), &where_)?;
        }
    }
    Ok((photo_id, ids))
}

/// The develop envelope. Group and value ranges are validated by the develop
/// pipeline (`[invalid-develop]`).
fn validate_develop(develop: Option<&Value>, what: &str) -> Result<()> {
    let develop = develop
        .and_then(Value::as_object)
        .with_context(|| format!("[invalid-develop] {what} develop must be an object"))?;
    match develop.get("process").and_then(Value::as_u64) {
        Some(1) => Ok(()),
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
        "upgraded": version != Some(VERSION),
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

//! Reusable `.pen` asset metadata and extraction.
use crate::{document::Document, editing, geometry};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetManifest {
    #[serde(default = "schema_one")]
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_version")]
    pub asset_version: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub entry_page: Option<String>,
    #[serde(default)]
    pub bounds: Option<Bounds>,
    #[serde(default)]
    pub properties: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
fn schema_one() -> u32 {
    1
}
fn default_version() -> String {
    "0.1.0".into()
}
fn default_kind() -> String {
    "component".into()
}

pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 200
        || id.starts_with('/')
        || id.ends_with('/')
        || id.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || !p
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    {
        bail!("asset ID must be slash-separated portable identifiers");
    }
    Ok(())
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn manifest(raw: &Value) -> Result<Option<AssetManifest>> {
    let Some(value) = raw.get("asset") else {
        return Ok(None);
    };
    let m: AssetManifest =
        serde_json::from_value(value.clone()).context("invalid asset metadata")?;
    validate_id(&m.id)?;
    if m.schema != 1 {
        bail!("unsupported asset metadata schema {}", m.schema);
    }
    semver::Version::parse(&m.asset_version)
        .context("asset_version must be semantic versioning")?;
    if m.name.trim().is_empty() {
        bail!("asset name cannot be empty");
    }
    Ok(Some(m))
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub page: Option<String>,
    pub layers: Vec<String>,
    pub objects: Vec<String>,
    pub rect: Option<Bounds>,
    pub canvas_bounds: bool,
    pub include_hidden: bool,
    pub properties: Vec<String>,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub kind: String,
    pub author: String,
    pub license: String,
    pub tags: Vec<String>,
    pub category: String,
    pub overwrite: bool,
    pub dry_run: bool,
}

pub fn create(options: &CreateOptions) -> Result<Value> {
    validate_id(&options.id)?;
    semver::Version::parse(&options.version).context("--version must be semantic versioning")?;
    if options.output.exists() && !options.overwrite {
        bail!("output exists; pass --overwrite");
    }
    let bytes = fs::read(&options.input)?;
    let mut raw: Value = serde_json::from_slice(&bytes).context("invalid .pen document")?;
    if raw.get("version").and_then(Value::as_u64) == Some(crate::scene::VERSION) {
        raw = crate::scene::flatten_to_v3(&raw)
            .context("could not flatten v4 scene for reusable asset extraction")?;
    }
    let mut doc: Document = serde_json::from_value(raw.clone())?;
    if let Some(page) = &options.page {
        doc.select_page(page).map_err(anyhow::Error::msg)?;
    }
    doc.validate().map_err(anyhow::Error::msg)?;

    // Serialize only the selected page. This also upgrades legacy inputs safely.
    doc.version = 3;
    raw = serde_json::to_value(&doc)?;
    if let Some(map) = raw.as_object_mut() {
        map.remove("canvas");
        map.remove("layers");
    }
    let layer_count;
    {
        let pages = raw
            .get_mut("pages")
            .and_then(Value::as_array_mut)
            .context("selected page missing")?;
        let selected = doc.active_page_id().to_owned();
        pages.retain(|p| p.get("id").and_then(Value::as_str) == Some(&selected));
        let page = pages.first_mut().context("selected page missing")?;
        page["id"] = Value::String("asset".into());
        page["name"] = Value::String(options.name.clone());
        let layers = page
            .get_mut("layers")
            .and_then(Value::as_array_mut)
            .context("asset layers missing")?;

        if !options.layers.is_empty() {
            let wanted: HashSet<&str> = options.layers.iter().map(String::as_str).collect();
            layers.retain(|l| {
                l.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| wanted.contains(id))
            });
            if layers.len() != wanted.len() {
                bail!("one or more selected layers do not exist");
            }
        }
        let mut rect_objects = HashSet::new();
        if let Some(rect) = options.rect {
            if rect.width <= 0.0
                || rect.height <= 0.0
                || ![rect.x, rect.y, rect.width, rect.height]
                    .iter()
                    .all(|n| n.is_finite())
            {
                bail!("selection rectangle must be finite with positive size");
            }
            let mut measured = doc.clone();
            let pairs = measured
                .layers
                .iter()
                .flat_map(|l| {
                    l.paths
                        .iter()
                        .map(|o| (l.id.clone(), o.id.clone()))
                        .chain(l.texts.iter().map(|o| (l.id.clone(), o.id.clone())))
                })
                .collect::<Vec<_>>();
            for (layer, id) in pairs {
                let b =
                    geometry::execute(&mut measured, &layer, &id, &geometry::Operation::Bounds)?;
                let bx = b["x"].as_f64().unwrap_or(0.0);
                let by = b["y"].as_f64().unwrap_or(0.0);
                let bw = b["width"].as_f64().unwrap_or(0.0);
                let bh = b["height"].as_f64().unwrap_or(0.0);
                if bx < rect.x + rect.width
                    && bx + bw > rect.x
                    && by < rect.y + rect.height
                    && by + bh > rect.y
                {
                    rect_objects.insert(format!("{layer}/{id}"));
                }
            }
        }
        if !options.objects.is_empty() || options.rect.is_some() {
            let wanted: HashSet<&str> = options.objects.iter().map(String::as_str).collect();
            let mut found = HashSet::new();
            for layer in layers.iter_mut() {
                let layer_id = layer
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                for key in ["paths", "texts"] {
                    if let Some(items) = layer.get_mut(key).and_then(Value::as_array_mut) {
                        items.retain(|item| {
                            let id = item.get("id").and_then(Value::as_str).unwrap_or("");
                            let qualified = format!("{layer_id}/{id}");
                            let keep = wanted.contains(id)
                                || wanted.contains(qualified.as_str())
                                || rect_objects.contains(&qualified);
                            if keep {
                                found.insert(id.to_owned());
                                found.insert(qualified);
                            }
                            keep
                        });
                    }
                }
            }
            if options.objects.iter().any(|id| !found.contains(id)) {
                bail!("one or more selected objects do not exist");
            }
            layers.retain(|l| {
                ["paths", "texts"].iter().any(|k| {
                    l.get(k)
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty())
                })
            });
        }
        if layers.is_empty() {
            bail!("asset selection is empty");
        }
        layer_count = layers.len();
    }

    let inferred_bounds = if let Some(rect) = options.rect {
        rect
    } else if options.canvas_bounds {
        Bounds {
            x: 0.0,
            y: 0.0,
            width: doc.canvas.width as f64,
            height: doc.canvas.height as f64,
        }
    } else {
        tight_visible_bounds(&raw, options.include_hidden)?
    };
    let mut properties = serde_json::Map::new();
    for shorthand in &options.properties {
        let (name, definition) = property_shorthand(shorthand)?;
        if properties
            .insert(name.clone(), definition.clone())
            .is_some()
        {
            bail!("duplicate exposed property: {name}")
        }
        validate_property_definition(&raw, &name, &definition)?;
    }
    raw["name"] = Value::String(options.name.clone());
    raw["asset"] = serde_json::to_value(AssetManifest {
        schema: 1,
        id: options.id.clone(),
        name: options.name.clone(),
        description: options.description.clone(),
        asset_version: options.version.clone(),
        kind: options.kind.clone(),
        author: options.author.clone(),
        license: options.license.clone(),
        tags: options.tags.clone(),
        category: options.category.clone(),
        entry_page: Some("asset".into()),
        bounds: Some(inferred_bounds),
        properties,
    })?;
    let output = serde_json::to_vec_pretty(&raw)?;
    let verified: Document = serde_json::from_slice(&output)?;
    verified.validate().map_err(anyhow::Error::msg)?;
    let summary = json!({"ok":true,"dry_run":options.dry_run,"output":options.output,"asset_id":options.id,"asset_version":options.version,"content_hash":hash_bytes(&output),"layers":layer_count,"bounds":inferred_bounds});
    if !options.dry_run {
        if let Some(parent) = options.output.parent() {
            fs::create_dir_all(parent)?;
        }
        if options.output.exists() {
            editing::transactional_write(&options.output, &output)?;
        } else {
            atomic_new(&options.output, &output)?;
        }
    }
    Ok(summary)
}

fn tight_visible_bounds(raw: &Value, include_hidden: bool) -> Result<Bounds> {
    let mut doc: Document = serde_json::from_value(raw.clone())?;
    let objects = doc
        .layers
        .iter()
        .filter(|layer| include_hidden || layer.visible)
        .flat_map(|layer| {
            layer
                .paths
                .iter()
                .map(|item| (layer.id.clone(), item.id.clone()))
                .chain(
                    layer
                        .texts
                        .iter()
                        .map(|item| (layer.id.clone(), item.id.clone())),
                )
        })
        .collect::<Vec<_>>();
    let mut union: Option<Bounds> = None;
    for (layer, object) in objects {
        let value = geometry::execute(&mut doc, &layer, &object, &geometry::Operation::Bounds)?;
        let bounds = Bounds {
            x: value["x"].as_f64().context("bounds x missing")?,
            y: value["y"].as_f64().context("bounds y missing")?,
            width: value["width"].as_f64().context("bounds width missing")?,
            height: value["height"].as_f64().context("bounds height missing")?,
        };
        union = Some(match union {
            None => bounds,
            Some(old) => {
                let x = old.x.min(bounds.x);
                let y = old.y.min(bounds.y);
                Bounds {
                    x,
                    y,
                    width: (old.x + old.width).max(bounds.x + bounds.width) - x,
                    height: (old.y + old.height).max(bounds.y + bounds.height) - y,
                }
            }
        });
    }
    union.context(
        "asset has no measurable visible content; select content, pass --include-hidden, or use --canvas-bounds",
    )
}

pub fn atomic_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.pentool-tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path).with_context(|| format!("could not create {}", path.display()))?;
    Ok(())
}

pub fn atomic_new_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        editing::transactional_write(path, bytes).map(|_| ())
    } else {
        atomic_new(path, bytes)
    }
}

pub fn inspect(path: &Path) -> Result<Value> {
    let bytes = fs::read(path)?;
    let raw: Value = serde_json::from_slice(&bytes)?;
    let doc: Document = serde_json::from_value(raw.clone())?;
    doc.validate().map_err(anyhow::Error::msg)?;
    Ok(
        json!({"file":path,"content_hash":hash_bytes(&bytes),"asset":manifest(&raw)?,"document":doc.name,"page":doc.active_page_id(),"canvas":doc.canvas,"layers":doc.layers.len()}),
    )
}

pub fn property_list(path: &Path, asset_id: &str) -> Result<Value> {
    let raw: Value = serde_json::from_slice(&fs::read(path)?)?;
    let manifest = checked_manifest(&raw, asset_id)?;
    Ok(json!({"asset_id":asset_id,"properties":manifest.properties}))
}

pub fn property_inspect(path: &Path, asset_id: &str, name: &str) -> Result<Value> {
    let raw: Value = serde_json::from_slice(&fs::read(path)?)?;
    let manifest = checked_manifest(&raw, asset_id)?;
    let definition = manifest
        .properties
        .get(name)
        .with_context(|| format!("exposed property not found: {name}"))?;
    Ok(json!({"asset_id":asset_id,"name":name,"definition":definition}))
}

pub fn property_usage(path: &Path, asset_id: &str, name: &str) -> Result<Value> {
    let inspected = property_inspect(path, asset_id, name)?;
    Ok(json!({"asset_id":asset_id,"property":name,"targets":inspected["definition"]["targets"]}))
}

pub fn property_validate(path: &Path, asset_id: &str) -> Result<Value> {
    let raw: Value = serde_json::from_slice(&fs::read(path)?)?;
    let manifest = checked_manifest(&raw, asset_id)?;
    for (name, definition) in &manifest.properties {
        validate_property_definition(&raw, name, definition)?;
    }
    Ok(json!({"ok":true,"asset_id":asset_id,"properties":manifest.properties.len()}))
}

pub fn validate_properties(raw: &Value) -> Result<()> {
    let manifest = manifest(raw)?.context("file has no asset metadata")?;
    for (name, definition) in &manifest.properties {
        validate_property_definition(raw, name, definition)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn property_write(
    path: &Path,
    asset_id: &str,
    name: &str,
    definition: Value,
    replace: bool,
    dry_run: bool,
    expected: Option<&str>,
) -> Result<Value> {
    validate_id(name).context("property name must be a portable identifier")?;
    let mut raw: Value = serde_json::from_slice(&fs::read(path)?)?;
    checked_manifest(&raw, asset_id)?;
    validate_property_definition(&raw, name, &definition)?;
    let properties = raw["asset"]["properties"]
        .as_object_mut()
        .context("asset properties must be an object")?;
    if properties.contains_key(name) != replace {
        if replace {
            bail!("exposed property not found: {name}")
        } else {
            bail!("exposed property already exists: {name}")
        }
    }
    properties.insert(name.to_owned(), definition.clone());
    let change = crate::transaction::commit_value(
        path,
        if replace {
            "asset-property-set"
        } else {
            "asset-property-add"
        },
        dry_run,
        expected,
        &raw,
    )?;
    Ok(
        json!({"ok":true,"dry_run":dry_run,"asset_id":asset_id,"property":name,"definition":definition,"change":change}),
    )
}

pub fn property_rename(
    path: &Path,
    asset_id: &str,
    name: &str,
    to: &str,
    dry_run: bool,
    expected: Option<&str>,
) -> Result<Value> {
    validate_id(to).context("new property name must be a portable identifier")?;
    let mut raw: Value = serde_json::from_slice(&fs::read(path)?)?;
    checked_manifest(&raw, asset_id)?;
    let properties = raw["asset"]["properties"]
        .as_object_mut()
        .context("asset properties must be an object")?;
    if properties.contains_key(to) {
        bail!("exposed property already exists: {to}")
    }
    let definition = properties
        .remove(name)
        .with_context(|| format!("exposed property not found: {name}"))?;
    properties.insert(to.to_owned(), definition);
    let change =
        crate::transaction::commit_value(path, "asset-property-rename", dry_run, expected, &raw)?;
    Ok(json!({"ok":true,"dry_run":dry_run,"asset_id":asset_id,"from":name,"to":to,"change":change}))
}

pub fn property_remove(
    path: &Path,
    asset_id: &str,
    name: &str,
    dry_run: bool,
    expected: Option<&str>,
) -> Result<Value> {
    let mut raw: Value = serde_json::from_slice(&fs::read(path)?)?;
    checked_manifest(&raw, asset_id)?;
    raw["asset"]["properties"]
        .as_object_mut()
        .context("asset properties must be an object")?
        .remove(name)
        .with_context(|| format!("exposed property not found: {name}"))?;
    let change =
        crate::transaction::commit_value(path, "asset-property-remove", dry_run, expected, &raw)?;
    Ok(json!({"ok":true,"dry_run":dry_run,"asset_id":asset_id,"removed":name,"change":change}))
}

pub fn property_definition(
    raw_schema: Option<&Path>,
    target: Option<&str>,
    field: Option<&str>,
    default: Option<&str>,
    label: Option<&str>,
) -> Result<Value> {
    if let Some(path) = raw_schema {
        return serde_json::from_slice(&fs::read(path)?)
            .context("property schema must be valid JSON");
    }
    let target = target.context("--target is required without --schema")?;
    let field = field.context("--field is required without --schema")?;
    let property_type = property_type(field)?;
    let mut definition =
        json!({"type":property_type,"targets":[{"object":target,"property":field}]});
    if let Some(label) = label {
        definition["label"] = Value::String(label.to_owned());
    }
    if let Some(default) = default {
        definition["default"] = parse_property_value(field, default)?;
    }
    Ok(definition)
}

pub fn property_shorthand(value: &str) -> Result<(String, Value)> {
    let (name, remainder) = value
        .split_once('=')
        .context("property must use NAME=TYPE:OBJECT.FIELD")?;
    validate_id(name).context("property name must be a portable identifier")?;
    let (declared_type, target) = remainder
        .split_once(':')
        .context("property must use NAME=TYPE:OBJECT.FIELD")?;
    let (object, field) = target
        .rsplit_once('.')
        .context("property target must use OBJECT.FIELD")?;
    let actual_type = property_type(field)?;
    if declared_type != actual_type {
        bail!("property {name} declares {declared_type} but {field} requires {actual_type}")
    }
    Ok((
        name.to_owned(),
        json!({"type":declared_type,"targets":[{"object":object,"property":field}]}),
    ))
}

fn checked_manifest(raw: &Value, asset_id: &str) -> Result<AssetManifest> {
    let manifest = manifest(raw)?.context("file has no asset metadata")?;
    if manifest.id != asset_id {
        bail!(
            "asset ID mismatch: file contains {}, requested {asset_id}",
            manifest.id
        )
    }
    Ok(manifest)
}

fn property_type(field: &str) -> Result<&'static str> {
    match field {
        "content" => Ok("text"),
        "fill" | "stroke" => Ok("color"),
        "visible" => Ok("visibility"),
        "stroke_width" | "opacity" | "width" | "height" => Ok("number"),
        "style_ref" => Ok("style"),
        _ => bail!("unsupported exposed property field: {field}"),
    }
}

fn parse_property_value(field: &str, value: &str) -> Result<Value> {
    match property_type(field)? {
        "visibility" => Ok(Value::Bool(
            value.parse().context("expected true or false")?,
        )),
        "number" => serde_json::Number::from_f64(value.parse().context("expected a number")?)
            .map(Value::Number)
            .context("number must be finite"),
        _ => Ok(Value::String(value.to_owned())),
    }
}

fn validate_property_definition(raw: &Value, name: &str, definition: &Value) -> Result<()> {
    let targets = definition
        .get("targets")
        .and_then(Value::as_array)
        .with_context(|| format!("property {name} must contain a targets array"))?;
    if targets.is_empty() {
        bail!("property {name} must contain at least one target")
    }
    let declared = definition.get("type").and_then(Value::as_str);
    let mut target_field = None;
    for target in targets {
        let object = target
            .get("object")
            .and_then(Value::as_str)
            .with_context(|| format!("property {name} target lacks object"))?;
        let field = target
            .get("property")
            .and_then(Value::as_str)
            .with_context(|| format!("property {name} target lacks property"))?;
        let actual = property_type(field)?;
        if declared.is_some_and(|declared| actual != declared) {
            bail!(
                "property {name} declares {} but {field} requires {actual}",
                declared.unwrap_or_default()
            )
        }
        if target_field
            .replace(field)
            .is_some_and(|old| property_type(old).ok() != Some(actual))
        {
            bail!("property {name} mixes incompatible target types")
        }
        if !contains_object_id(raw, object) {
            let suggestion = object_ids(raw)
                .into_iter()
                .min_by_key(|candidate| edit_distance(candidate, object));
            if let Some(suggestion) = suggestion {
                bail!(
                    "property {name} target object not found: {object}; did you mean {suggestion}?"
                )
            }
            bail!("property {name} target object not found: {object}")
        }
    }
    if let (Some(default), Some(field)) = (definition.get("default"), target_field) {
        validate_property_value(name, field, default, definition.get("constraints"))?;
    }
    Ok(())
}

fn validate_property_value(
    name: &str,
    field: &str,
    value: &Value,
    constraints: Option<&Value>,
) -> Result<()> {
    match property_type(field)? {
        "visibility" if !value.is_boolean() => bail!("property {name} expects a boolean"),
        "number" if !value.is_number() => bail!("property {name} expects a number"),
        "text" | "color" | "style" if !value.is_string() => {
            bail!("property {name} expects a string")
        }
        _ => {}
    }
    if let Some(constraints) = constraints {
        let constraints = constraints
            .as_object()
            .with_context(|| format!("property {name} constraints must be an object"))?;
        if let Some(max_length) = constraints.get("max_length").and_then(Value::as_u64) {
            if value
                .as_str()
                .is_some_and(|text| text.chars().count() > max_length as usize)
            {
                bail!("property {name} exceeds max_length {max_length}")
            }
        }
        if let Some(number) = value.as_f64() {
            if constraints
                .get("min")
                .and_then(Value::as_f64)
                .is_some_and(|min| number < min)
            {
                bail!("property {name} is below its minimum")
            }
            if constraints
                .get("max")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max)
            {
                bail!("property {name} is above its maximum")
            }
        }
        if let Some(allowed) = constraints.get("enum").and_then(Value::as_array) {
            if !allowed.contains(value) {
                bail!("property {name} is not one of its allowed values")
            }
        }
    }
    Ok(())
}

fn contains_object_id(value: &Value, id: &str) -> bool {
    match value {
        Value::Object(map) => {
            map.get("id").and_then(Value::as_str) == Some(id)
                || map.values().any(|child| contains_object_id(child, id))
        }
        Value::Array(values) => values.iter().any(|child| contains_object_id(child, id)),
        _ => false,
    }
}

fn object_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    fn visit(value: &Value, ids: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(id) = map.get("id").and_then(Value::as_str) {
                    ids.push(id.to_owned());
                }
                map.values().for_each(|value| visit(value, ids));
            }
            Value::Array(values) => values.iter().for_each(|value| visit(value, ids)),
            _ => {}
        }
    }
    visit(value, &mut ids);
    ids
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.chars().count()).collect::<Vec<_>>();
    for (i, a) in left.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, b) in right.chars().enumerate() {
            current.push(
                (current[j] + 1)
                    .min(previous[j + 1] + 1)
                    .min(previous[j] + usize::from(a != b)),
            );
        }
        previous = current;
    }
    previous[right.chars().count()]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_validation() {
        assert!(validate_id("open/icons/arrow-right").is_ok());
        assert!(validate_id("../escape").is_err());
        assert!(validate_id("bad name").is_err());
    }

    #[test]
    fn inferred_bounds_are_tight_and_ignore_hidden_layers() {
        let raw = json!({
            "format":"pentool","version":3,"name":"Bounds","fonts":[],
            "pages":[{"id":"asset","name":"Asset","canvas":{"width":920,"height":400,"background":"none"},"layers":[
                {"id":"visible","name":"Visible","visible":true,"locked":false,"paths":[{"id":"body","d":"M10 10 L90 10 L90 90 L10 90 Z","fill":"#000","stroke":"none","stroke_width":0,"closed":true}],"texts":[]},
                {"id":"hidden","name":"Hidden","visible":false,"locked":false,"paths":[{"id":"far","d":"M500 20 L600 20 L600 80 Z","fill":"#000","stroke":"none","stroke_width":0,"closed":true}],"texts":[]}
            ]}]
        });
        let visible = tight_visible_bounds(&raw, false).unwrap();
        assert_eq!(
            (visible.x, visible.y, visible.width, visible.height),
            (10.0, 10.0, 80.0, 80.0)
        );
        let all = tight_visible_bounds(&raw, true).unwrap();
        assert!(all.width > 500.0);
    }
}

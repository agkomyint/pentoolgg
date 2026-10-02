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
        bounds: None,
        properties: Default::default(),
    })?;
    let output = serde_json::to_vec_pretty(&raw)?;
    let verified: Document = serde_json::from_slice(&output)?;
    verified.validate().map_err(anyhow::Error::msg)?;
    let summary = json!({"ok":true,"dry_run":options.dry_run,"output":options.output,"asset_id":options.id,"asset_version":options.version,"content_hash":hash_bytes(&output),"layers":layer_count});
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_validation() {
        assert!(validate_id("open/icons/arrow-right").is_ok());
        assert!(validate_id("../escape").is_err());
        assert!(validate_id("bad name").is_err());
    }
}

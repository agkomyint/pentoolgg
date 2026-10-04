//! Offline-safe component instance metadata over materialized imported layers.
use crate::asset::AssetManifest;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceRecord {
    pub id: String,
    pub library: String,
    pub asset_id: String,
    pub asset_version: String,
    pub content_hash: String,
    pub layer_ids: Vec<String>,
    #[serde(default = "default_page")]
    pub page_id: String,
    pub transform: [f64; 6],
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub overrides: serde_json::Map<String, Value>,
    #[serde(default)]
    pub previous: Vec<Value>,
}
fn yes() -> bool {
    true
}
fn default_page() -> String {
    "page-1".into()
}

pub fn attach(raw: &mut Value, record: InstanceRecord) -> Result<()> {
    let root = raw
        .as_object_mut()
        .context("document root must be an object")?;
    let values = root
        .entry("instances")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("instances must be an array")?;
    if values
        .iter()
        .any(|v| v.get("id").and_then(Value::as_str) == Some(&record.id))
    {
        bail!("instance ID already exists")
    }
    values.push(serde_json::to_value(record)?);
    Ok(())
}
pub fn list(raw: &Value) -> Result<Value> {
    Ok(
        json!({"instances":raw.get("instances").and_then(Value::as_array).cloned().unwrap_or_default()}),
    )
}
pub fn inspect(raw: &Value, id: &str) -> Result<Value> {
    raw.get("instances")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        })
        .cloned()
        .context("instance not found")
}
pub fn detach(file: &Path, id: &str, dry_run: bool) -> Result<Value> {
    let bytes = fs::read(file)?;
    let mut raw: Value = serde_json::from_slice(&bytes)?;
    let arr = raw
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .context("document has no instances")?;
    let before = arr.len();
    arr.retain(|v| v.get("id").and_then(Value::as_str) != Some(id));
    if arr.len() == before {
        bail!("instance not found")
    };
    crate::transaction::commit_value(file, "instance-detach", dry_run, None, &raw)?;
    Ok(json!({"ok":true,"dry_run":dry_run,"instance":id,"detached":true}))
}

pub fn set_property(
    file: &Path,
    id: &str,
    manifest: &AssetManifest,
    name: &str,
    value: &str,
    dry_run: bool,
) -> Result<Value> {
    let definition = manifest
        .properties
        .get(name)
        .context("property is not exposed by this asset")?;
    let targets = definition
        .get("targets")
        .and_then(Value::as_array)
        .context("property has no targets")?;
    let bytes = fs::read(file)?;
    let mut raw: Value = serde_json::from_slice(&bytes)?;
    let record = raw
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .and_then(|a| {
            a.iter_mut()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        })
        .context("instance not found")?;
    let layer_ids: Vec<String> = record
        .get("layer_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    for target in targets {
        let object = target
            .get("object")
            .and_then(Value::as_str)
            .context("property target lacks object")?;
        let property = target
            .get("property")
            .and_then(Value::as_str)
            .context("property target lacks property")?;
        if !matches!(property, "fill" | "stroke" | "content" | "visible") {
            bail!("unsupported exposed property target")
        };
        set_object_property(&mut raw, &layer_ids, object, property, value)?;
    }
    let record = raw
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .and_then(|a| {
            a.iter_mut()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        })
        .unwrap();
    record["overrides"][name] = Value::String(value.into());
    crate::transaction::commit_value(file, "instance-set", dry_run, None, &raw)?;
    Ok(json!({"ok":true,"dry_run":dry_run,"instance":id,"property":name,"value":value}))
}
fn set_object_property(
    raw: &mut Value,
    layers: &[String],
    object: &str,
    property: &str,
    value: &str,
) -> Result<()> {
    let pages = raw
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .context("instance document must use pages")?;
    for page in pages {
        for layer in page
            .get_mut("layers")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if !layers
                .iter()
                .any(|id| layer.get("id").and_then(Value::as_str) == Some(id))
            {
                continue;
            }
            for key in ["paths", "texts"] {
                for item in layer
                    .get_mut(key)
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if item
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id == object || id.ends_with(&format!("-{object}")))
                    {
                        item[property] = if property == "visible" {
                            Value::Bool(value.parse()?)
                        } else {
                            Value::String(value.into())
                        };
                        return Ok(());
                    }
                }
            }
        }
    }
    bail!("property target object not found")
}

pub fn update_plan(file: &Path, id: &str, source: &Path) -> Result<Value> {
    let document: Value = serde_json::from_slice(&fs::read(file)?)?;
    let record: InstanceRecord = serde_json::from_value(inspect(&document, id)?)?;
    let bytes = fs::read(source)?;
    let raw: Value = serde_json::from_slice(&bytes)?;
    let manifest = crate::asset::manifest(&raw)?.context("source lacks asset metadata")?;
    if manifest.id != record.asset_id {
        bail!("source asset ID does not match instance")
    }
    let next_hash = crate::asset::hash_bytes(&bytes);
    Ok(
        json!({"instance":id,"asset_id":record.asset_id,"from_version":record.asset_version,"to_version":manifest.asset_version,"from_hash":record.content_hash,"to_hash":next_hash,"changed":record.content_hash!=next_hash,"overrides":record.overrides,"conflicts":if record.overrides.is_empty(){vec![]}else{vec!["instance has overrides; detach or clear them before this update"]}}),
    )
}

pub fn update(file: &Path, id: &str, source: &Path, dry_run: bool) -> Result<Value> {
    let plan = update_plan(file, id, source)?;
    if !plan["changed"].as_bool().unwrap_or(false) {
        return Ok(json!({"ok":true,"changed":false,"instance":id}));
    }
    if plan["conflicts"].as_array().is_some_and(|a| !a.is_empty()) {
        bail!("instance update has conflicts; inspect the update plan")
    }
    if dry_run {
        return Ok(json!({"ok":true,"dry_run":true,"plan":plan}));
    }
    let destination_bytes = fs::read(file)?;
    let mut destination: Value = serde_json::from_slice(&destination_bytes)?;
    let old_value = inspect(&destination, id)?;
    let old: InstanceRecord = serde_json::from_value(old_value)?;
    let old_positions = layer_positions(&destination, &old.page_id, &old.layer_ids)?;
    let old_layers = take_layers(&mut destination, &old.page_id, &old.layer_ids)?;
    let source_bytes = fs::read(source)?;
    let source_raw: Value = serde_json::from_slice(&source_bytes)?;
    let manifest = crate::asset::manifest(&source_raw)?.context("source lacks asset metadata")?;
    let new_hash = crate::asset::hash_bytes(&source_bytes);
    let sx = (old.transform[0] * old.transform[0] + old.transform[1] * old.transform[1]).sqrt();
    let rotation = old.transform[1].atan2(old.transform[0]).to_degrees();
    let result = crate::import::compose(
        destination,
        source_raw,
        &crate::import::ImportOptions {
            destination_page: Some(old.page_id.clone()),
            source_page: manifest.entry_page.clone(),
            prefix: Some(format!("{}-{}", id, &new_hash[7..15])),
            x: old.transform[4],
            y: old.transform[5],
            scale: sx,
            rotation,
            expand_canvas: false,
        },
    )?;
    let mut updated = result.document;
    let new_ids = result.summary["layers"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let imported_layers = take_layers(&mut updated, &old.page_id, &new_ids)?;
    insert_layers_at(&mut updated, &old.page_id, imported_layers, &old_positions)?;
    let record = updated
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .and_then(|a| {
            a.iter_mut()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        })
        .context("instance record lost during update")?;
    let history = json!({"asset_version":old.asset_version,"content_hash":old.content_hash,"layer_ids":old.layer_ids,"layers":old_layers,"positions":old_positions});
    record["asset_version"] = Value::String(manifest.asset_version.clone());
    record["content_hash"] = Value::String(new_hash.clone());
    record["layer_ids"] = serde_json::to_value(&new_ids)?;
    record["previous"]
        .as_array_mut()
        .context("invalid instance history")?
        .push(history);
    crate::transaction::commit_value(file, "instance-update", false, None, &updated)?;
    Ok(
        json!({"ok":true,"instance":id,"from":plan["from_version"],"to":manifest.asset_version,"content_hash":new_hash,"layers":new_ids}),
    )
}

pub fn rollback(file: &Path, id: &str, dry_run: bool) -> Result<Value> {
    let bytes = fs::read(file)?;
    let mut raw: Value = serde_json::from_slice(&bytes)?;
    let record_value = inspect(&raw, id)?;
    let current: InstanceRecord = serde_json::from_value(record_value)?;
    let previous = current
        .previous
        .last()
        .cloned()
        .context("instance has no update to roll back")?;
    if dry_run {
        return Ok(
            json!({"ok":true,"dry_run":true,"instance":id,"to_version":previous["asset_version"]}),
        );
    }
    take_layers(&mut raw, &current.page_id, &current.layer_ids)?;
    let page = page_mut(&mut raw, &current.page_id)?;
    let layers = page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers missing")?;
    let restored = previous["layers"]
        .as_array()
        .context("rollback layers missing")?
        .clone();
    let positions = previous
        .get("positions")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_u64)
                .map(|v| v as usize)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![layers.len(); restored.len()]);
    for (layer, position) in restored.into_iter().zip(positions) {
        layers.insert(position.min(layers.len()), layer);
    }
    let record = raw
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .and_then(|a| {
            a.iter_mut()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        })
        .unwrap();
    record["asset_version"] = previous["asset_version"].clone();
    record["content_hash"] = previous["content_hash"].clone();
    record["layer_ids"] = previous["layer_ids"].clone();
    record["previous"].as_array_mut().unwrap().pop();
    let rolled_back_to = record["asset_version"].clone();
    crate::transaction::commit_value(file, "instance-rollback", false, None, &raw)?;
    Ok(json!({"ok":true,"instance":id,"rolled_back_to":rolled_back_to}))
}
fn page_mut<'a>(raw: &'a mut Value, page_id: &str) -> Result<&'a mut Value> {
    raw.get_mut("pages")
        .and_then(Value::as_array_mut)
        .and_then(|p| {
            p.iter_mut()
                .find(|p| p.get("id").and_then(Value::as_str) == Some(page_id))
        })
        .context("instance page not found")
}
fn take_layers(raw: &mut Value, page_id: &str, ids: &[String]) -> Result<Vec<Value>> {
    let layers = page_mut(raw, page_id)?
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers missing")?;
    let mut removed = vec![];
    let mut kept = vec![];
    for layer in std::mem::take(layers) {
        if ids
            .iter()
            .any(|id| layer.get("id").and_then(Value::as_str) == Some(id))
        {
            removed.push(layer)
        } else {
            kept.push(layer)
        }
    }
    *layers = kept;
    if removed.len() != ids.len() {
        bail!("instance materialized layers are missing")
    };
    Ok(removed)
}

fn layer_positions(raw: &Value, page_id: &str, ids: &[String]) -> Result<Vec<usize>> {
    let layers = raw
        .get("pages")
        .and_then(Value::as_array)
        .and_then(|pages| {
            pages
                .iter()
                .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
        })
        .and_then(|page| page.get("layers"))
        .and_then(Value::as_array)
        .context("instance page layers missing")?;
    ids.iter()
        .map(|id| {
            layers
                .iter()
                .position(|layer| layer.get("id").and_then(Value::as_str) == Some(id))
                .with_context(|| format!("instance layer missing: {id}"))
        })
        .collect()
}

fn insert_layers_at(
    raw: &mut Value,
    page_id: &str,
    layers_to_insert: Vec<Value>,
    positions: &[usize],
) -> Result<()> {
    let layers = page_mut(raw, page_id)?
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers missing")?;
    for (layer, position) in layers_to_insert.into_iter().zip(positions.iter().copied()) {
        layers.insert(position.min(layers.len()), layer);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_instance_layers_preserves_their_stack_positions() {
        let mut raw = json!({"pages":[{"id":"page-1","layers":[
            {"id":"back"},{"id":"old-a"},{"id":"old-b"},{"id":"front"}
        ]}]});
        let old_ids = vec!["old-a".to_owned(), "old-b".to_owned()];
        let positions = layer_positions(&raw, "page-1", &old_ids).unwrap();
        assert_eq!(positions, vec![1, 2]);
        take_layers(&mut raw, "page-1", &old_ids).unwrap();
        insert_layers_at(
            &mut raw,
            "page-1",
            vec![json!({"id":"new-a"}), json!({"id":"new-b"})],
            &positions,
        )
        .unwrap();
        let ids = raw["pages"][0]["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|layer| layer["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["back", "new-a", "new-b", "front"]);
    }
}

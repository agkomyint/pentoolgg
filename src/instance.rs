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
    /// Exposed-property definitions accepted for the current source revision.
    /// These make semantic target changes detectable even when a key is reused.
    #[serde(default)]
    pub property_definitions: serde_json::Map<String, Value>,
    /// Hash of the last materialized state accepted by Pentool. Generic edits
    /// that change this hash are treated as undeclared local changes.
    #[serde(default)]
    pub materialized_hash: Option<String>,
    /// Exact materialized base used for three-way update planning.
    #[serde(default)]
    pub base_layers: Vec<Value>,
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
    let page_id = record
        .get("page_id")
        .and_then(Value::as_str)
        .unwrap_or("page-1")
        .to_owned();
    let override_value = parse_override_value(definition, value)?;
    for target in targets {
        let object = target
            .get("object")
            .and_then(Value::as_str)
            .context("property target lacks object")?;
        let property = target
            .get("property")
            .and_then(Value::as_str)
            .context("property target lacks property")?;
        if !matches!(
            property,
            "fill"
                | "stroke"
                | "content"
                | "visible"
                | "stroke_width"
                | "opacity"
                | "width"
                | "height"
                | "style_ref"
        ) {
            bail!("unsupported exposed property target")
        };
        set_object_property(&mut raw, &layer_ids, object, property, &override_value)?;
    }
    let accepted_hash = materialized_hash(&raw, &page_id, &layer_ids)?;
    let record = raw
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .and_then(|a| {
            a.iter_mut()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        })
        .unwrap();
    record["overrides"][name] = override_value.clone();
    record["property_definitions"][name] = definition.clone();
    record["materialized_hash"] = Value::String(accepted_hash);
    crate::transaction::commit_value(file, "instance-set", dry_run, None, &raw)?;
    Ok(json!({"ok":true,"dry_run":dry_run,"instance":id,"property":name,"value":value}))
}
fn set_object_property(
    raw: &mut Value,
    layers: &[String],
    object: &str,
    property: &str,
    value: &Value,
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
                        item[property] = value.clone();
                        return Ok(());
                    }
                }
            }
        }
    }
    bail!("property target object not found")
}

fn parse_override_value(definition: &Value, value: &str) -> Result<Value> {
    let targets = definition
        .get("targets")
        .and_then(Value::as_array)
        .context("property has no targets")?;
    let kinds = targets
        .iter()
        .filter_map(|target| target.get("property").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if kinds.is_empty() {
        bail!("property has no valid targets")
    }
    if kinds.iter().all(|kind| *kind == "visible") {
        return Ok(Value::Bool(
            value.parse().context("expected true or false")?,
        ));
    }
    if kinds
        .iter()
        .all(|kind| matches!(*kind, "stroke_width" | "opacity" | "width" | "height"))
    {
        let number = value.parse::<f64>().context("expected a number")?;
        return serde_json::Number::from_f64(number)
            .map(Value::Number)
            .context("number must be finite");
    }
    if kinds
        .iter()
        .all(|kind| matches!(*kind, "fill" | "stroke" | "content" | "style_ref"))
    {
        return Ok(Value::String(value.to_owned()));
    }
    bail!("property mixes incompatible target types")
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
    let current_materialized_hash =
        materialized_hash(&document, &record.page_id, &record.layer_ids)?;
    let local_changed = record
        .materialized_hash
        .as_deref()
        .is_none_or(|accepted| accepted != current_materialized_hash);
    let mut conflicts = Vec::new();
    if local_changed {
        conflicts.push(json!({
            "kind":"untracked-local-edit",
            "instance":id,
            "page":record.page_id,
            "base":record.materialized_hash,
            "local":current_materialized_hash,
            "incoming":next_hash,
            "resolutions":["keep-local","take-source","detach"]
        }));
    }
    for (name, value) in &record.overrides {
        match manifest.properties.get(name) {
            Some(definition)
                if override_compatible(definition, value)
                    && record
                        .property_definitions
                        .get(name)
                        .is_some_and(|accepted| property_identity(accepted) == property_identity(definition))
                    && override_targets_exist(&raw, definition) => {}
            Some(definition)
                if record.property_definitions.contains_key(name)
                    && property_identity(&record.property_definitions[name])
                        != property_identity(definition) => conflicts.push(json!({"kind":"retargeted-override","instance":id,"property":name,"base":property_identity(&record.property_definitions[name]),"incoming":property_identity(definition),"resolutions":["take-source","detach","map-target"]})),
            Some(definition) if override_compatible(definition, value) => conflicts.push(json!({"kind":"removed-override-target","instance":id,"property":name,"resolutions":["take-source","detach","map-target"]})),
            Some(_) => conflicts.push(json!({"kind":"incompatible-override","instance":id,"property":name,"resolutions":["keep-local","take-source","detach"]})),
            None => conflicts.push(json!({"kind":"removed-override-target","instance":id,"property":name,"resolutions":["take-source","detach","map-target"]})),
        }
    }
    Ok(
        json!({"instance":id,"asset_id":record.asset_id,"from_version":record.asset_version,"to_version":manifest.asset_version,"from_hash":record.content_hash,"to_hash":next_hash,"changed":record.content_hash!=next_hash,"local_changed":local_changed,"materialized":{"accepted":record.materialized_hash,"current":current_materialized_hash},"overrides":record.overrides,"conflicts":conflicts}),
    )
}

fn property_identity(definition: &Value) -> Value {
    Value::Array(
        definition
            .get("targets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|target| {
                json!({
                    "object": target.get("object").cloned().unwrap_or(Value::Null),
                    "property": target.get("property").cloned().unwrap_or(Value::Null)
                })
            })
            .collect(),
    )
}

fn override_targets_exist(source: &Value, definition: &Value) -> bool {
    definition
        .get("targets")
        .and_then(Value::as_array)
        .is_some_and(|targets| {
            !targets.is_empty()
                && targets.iter().all(|target| {
                    target
                        .get("object")
                        .and_then(Value::as_str)
                        .is_some_and(|id| contains_object_id(source, id))
                })
        })
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

fn override_compatible(definition: &Value, value: &Value) -> bool {
    let Some(targets) = definition.get("targets").and_then(Value::as_array) else {
        return false;
    };
    !targets.is_empty()
        && targets.iter().all(
            |target| match target.get("property").and_then(Value::as_str) {
                Some("visible") => {
                    value.as_bool().is_some()
                        || value.as_str().is_some_and(|v| v.parse::<bool>().is_ok())
                }
                Some("stroke_width" | "opacity" | "width" | "height") => {
                    value.as_f64().is_some()
                        || value.as_str().is_some_and(|v| v.parse::<f64>().is_ok())
                }
                Some("fill" | "stroke" | "content" | "style_ref") => value.is_string(),
                _ => false,
            },
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
    let next_base_layers = selected_layers(&updated, &old.page_id, &new_ids)?;
    for (name, value) in &old.overrides {
        let definition = manifest
            .properties
            .get(name)
            .with_context(|| format!("override target removed during update: {name}"))?;
        apply_override(&mut updated, &new_ids, definition, value)?;
    }
    let next_materialized_hash = materialized_hash(&updated, &old.page_id, &new_ids)?;
    let record = updated
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .and_then(|a| {
            a.iter_mut()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        })
        .context("instance record lost during update")?;
    let history = json!({"asset_version":old.asset_version,"content_hash":old.content_hash,"layer_ids":old.layer_ids,"layers":old_layers,"positions":old_positions,"materialized_hash":old.materialized_hash,"base_layers":old.base_layers,"property_definitions":old.property_definitions});
    record["asset_version"] = Value::String(manifest.asset_version.clone());
    record["content_hash"] = Value::String(new_hash.clone());
    record["layer_ids"] = serde_json::to_value(&new_ids)?;
    record["materialized_hash"] = Value::String(next_materialized_hash);
    record["base_layers"] = Value::Array(next_base_layers);
    record["property_definitions"] = serde_json::to_value(&manifest.properties)?;
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
    record["materialized_hash"] = previous
        .get("materialized_hash")
        .cloned()
        .unwrap_or(Value::Null);
    record["base_layers"] = previous
        .get("base_layers")
        .cloned()
        .unwrap_or_else(|| previous["layers"].clone());
    record["property_definitions"] = previous
        .get("property_definitions")
        .cloned()
        .unwrap_or_else(|| json!({}));
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

pub fn selected_layers(raw: &Value, page_id: &str, ids: &[String]) -> Result<Vec<Value>> {
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
                .find(|layer| layer.get("id").and_then(Value::as_str) == Some(id))
                .cloned()
                .with_context(|| format!("instance layer missing: {id}"))
        })
        .collect()
}

pub fn materialized_hash(raw: &Value, page_id: &str, ids: &[String]) -> Result<String> {
    Ok(crate::asset::hash_bytes(&serde_json::to_vec(
        &selected_layers(raw, page_id, ids)?,
    )?))
}

fn apply_override(
    raw: &mut Value,
    layers: &[String],
    definition: &Value,
    value: &Value,
) -> Result<()> {
    let targets = definition
        .get("targets")
        .and_then(Value::as_array)
        .context("property has no targets")?;
    for target in targets {
        let object = target
            .get("object")
            .and_then(Value::as_str)
            .context("property target lacks object")?;
        let property = target
            .get("property")
            .and_then(Value::as_str)
            .context("property target lacks property")?;
        set_object_property(raw, layers, object, property, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pentool-v063-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

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

    #[test]
    fn update_plan_reports_untracked_local_edits_and_accepts_compatible_overrides() {
        let document_path = temp_file("document.pen");
        let source_path = temp_file("source.pen");
        let layer_id = "instance-card-content".to_owned();
        let mut document = json!({
            "format":"pentool","version":3,"name":"Deck","fonts":[],
            "pages":[{"id":"page-1","name":"Page 1","canvas":{"width":400,"height":300,"background":"#fff"},"layers":[
                {"id":layer_id,"name":"Card","visible":true,"locked":false,"paths":[],"texts":[{"id":"instance-card-label","content":"Local","x":10,"y":20,"fill":"#000"}]}
            ]}],"instances":[]
        });
        let ids = vec![layer_id];
        let accepted = materialized_hash(&document, "page-1", &ids).unwrap();
        attach(
            &mut document,
            InstanceRecord {
                id: "card".into(),
                library: "test".into(),
                asset_id: "ui/card".into(),
                asset_version: "1.0.0".into(),
                content_hash: "sha256:old".into(),
                layer_ids: ids,
                page_id: "page-1".into(),
                transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                visible: true,
                overrides: serde_json::Map::from_iter([(
                    "label".into(),
                    Value::String("Local".into()),
                )]),
                property_definitions: serde_json::Map::from_iter([(
                    "label".into(),
                    json!({"targets":[{"object":"label","property":"content"}]}),
                )]),
                materialized_hash: Some(accepted),
                base_layers: vec![],
                previous: vec![],
            },
        )
        .unwrap();
        let source = json!({
            "format":"pentool","version":3,"name":"Card","fonts":[],
            "asset":{"id":"ui/card","name":"Card","asset_version":"1.1.0","entry_page":"page-1","properties":{"label":{"targets":[{"object":"label","property":"content"}]}}},
            "pages":[{"id":"page-1","name":"Page 1","canvas":{"width":100,"height":100,"background":"#fff"},"layers":[
                {"id":"content","name":"Card","visible":true,"locked":false,"paths":[],"texts":[{"id":"label","content":"Source","x":10,"y":20,"fill":"#000"}]}
            ]}]
        });
        fs::write(&document_path, serde_json::to_vec(&document).unwrap()).unwrap();
        fs::write(&source_path, serde_json::to_vec(&source).unwrap()).unwrap();
        let clean = update_plan(&document_path, "card", &source_path).unwrap();
        assert_eq!(clean["local_changed"], false);
        assert_eq!(clean["conflicts"], json!([]));

        let mut missing_target = source.clone();
        missing_target["pages"][0]["layers"][0]["texts"] = json!([]);
        fs::write(&source_path, serde_json::to_vec(&missing_target).unwrap()).unwrap();
        let missing = update_plan(&document_path, "card", &source_path).unwrap();
        assert_eq!(missing["conflicts"][0]["kind"], "removed-override-target");

        let mut retargeted = source.clone();
        retargeted["asset"]["properties"]["label"]["targets"][0]["property"] = json!("fill");
        fs::write(&source_path, serde_json::to_vec(&retargeted).unwrap()).unwrap();
        let retargeted_plan = update_plan(&document_path, "card", &source_path).unwrap();
        assert_eq!(
            retargeted_plan["conflicts"][0]["kind"],
            "retargeted-override"
        );
        fs::write(&source_path, serde_json::to_vec(&source).unwrap()).unwrap();

        document["pages"][0]["layers"][0]["texts"][0]["content"] = json!("Untracked");
        fs::write(&document_path, serde_json::to_vec(&document).unwrap()).unwrap();
        let changed = update_plan(&document_path, "card", &source_path).unwrap();
        assert_eq!(changed["local_changed"], true);
        assert_eq!(changed["conflicts"][0]["kind"], "untracked-local-edit");
        let _ = fs::remove_file(document_path);
        let _ = fs::remove_file(source_path);
    }

    #[test]
    fn update_preserves_stack_and_reapplies_compatible_override() {
        let document_path = temp_file("update-document.pen");
        let source_path = temp_file("update-source.pen");
        let old_layer_id = "card-old-content".to_owned();
        let old_layer = json!({"id":old_layer_id,"name":"Card","visible":true,"locked":false,"paths":[],"texts":[{"id":"card-old-label","content":"Local label","x":10,"y":20,"fill":"#000"}]});
        let mut document = json!({
            "format":"pentool","version":3,"name":"Deck","fonts":[],
            "pages":[{"id":"page-1","name":"Page 1","canvas":{"width":400,"height":300,"background":"#fff"},"layers":[
                {"id":"back","name":"Back","visible":true,"locked":false,"paths":[],"texts":[]},
                old_layer,
                {"id":"check","name":"Check","visible":true,"locked":true,"paths":[],"texts":[]}
            ]}],"instances":[]
        });
        let ids = vec![old_layer_id];
        let accepted = materialized_hash(&document, "page-1", &ids).unwrap();
        let base = selected_layers(&document, "page-1", &ids).unwrap();
        attach(
            &mut document,
            InstanceRecord {
                id: "card".into(),
                library: "test".into(),
                asset_id: "ui/card".into(),
                asset_version: "1.0.0".into(),
                content_hash: "sha256:old".into(),
                layer_ids: ids,
                page_id: "page-1".into(),
                transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                visible: true,
                overrides: serde_json::Map::from_iter([(
                    "label".into(),
                    Value::String("Local label".into()),
                )]),
                property_definitions: serde_json::Map::from_iter([(
                    "label".into(),
                    json!({"targets":[{"object":"label","property":"content"}]}),
                )]),
                materialized_hash: Some(accepted),
                base_layers: base,
                previous: vec![],
            },
        )
        .unwrap();
        let source = json!({
            "format":"pentool","version":3,"name":"Card","fonts":[],
            "pages":[{"id":"asset","name":"Asset","canvas":{"width":920,"height":400,"background":"none"},"layers":[
                {"id":"content","name":"Content","visible":true,"locked":false,"paths":[{"id":"body","d":"M0 0 L80 0 L80 80 Z","stroke":"none","stroke_width":0,"fill":"#eee","closed":true}],"texts":[{"id":"label","content":"Source 1.1","x":10,"y":20,"fill":"#000"}]}
            ]}],
            "asset":{"id":"ui/card","name":"Card","asset_version":"1.1.0","entry_page":"asset","properties":{"label":{"type":"text","targets":[{"object":"label","property":"content"}]}}}
        });
        fs::write(&document_path, serde_json::to_vec(&document).unwrap()).unwrap();
        fs::write(&source_path, serde_json::to_vec(&source).unwrap()).unwrap();
        update(&document_path, "card", &source_path, false).unwrap();
        let updated: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
        let layers = updated["pages"][0]["layers"].as_array().unwrap();
        assert_eq!(layers[0]["id"], "back");
        assert_eq!(layers[2]["id"], "check");
        assert_eq!(layers[1]["texts"][0]["content"], "Local label");
        assert_eq!(updated["instances"][0]["asset_version"], "1.1.0");
        let _ = fs::remove_file(document_path);
        let _ = fs::remove_file(source_path);
    }
}

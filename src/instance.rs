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
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub layer_indices: Vec<usize>,
    #[serde(default)]
    pub layer_states: Vec<Value>,
    pub transform: [f64; 6],
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub overrides: serde_json::Map<String, Value>,
    /// Exposed-property definitions accepted for the current source revision.
    /// These make semantic target changes detectable even when a key is reused.
    #[serde(default)]
    pub property_definitions: serde_json::Map<String, Value>,
    /// Stable source-to-materialized child ID mappings for discovery APIs.
    #[serde(default)]
    pub child_ids: Vec<Value>,
    /// Explicitly accepted property-level local edits reapplied after updates.
    #[serde(default)]
    pub local_patches: Vec<Value>,
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
    if !override_compatible(definition, &override_value) {
        bail!("override value violates the exposed property's type or constraints")
    }
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
    let bytes = fs::read(source)?;
    let raw: Value = serde_json::from_slice(&bytes)?;
    update_plan_value(&document, id, &raw, &bytes)
}

fn update_plan_value(document: &Value, id: &str, raw: &Value, bytes: &[u8]) -> Result<Value> {
    let record: InstanceRecord = serde_json::from_value(inspect(document, id)?)?;
    let manifest = crate::asset::manifest(raw)?.context("source lacks asset metadata")?;
    if manifest.id != record.asset_id {
        bail!("source asset ID does not match instance")
    }
    let next_hash = crate::asset::hash_bytes(bytes);
    let current_materialized_hash =
        materialized_hash(document, &record.page_id, &record.layer_ids)?;
    let local_changed = record
        .materialized_hash
        .as_deref()
        .is_none_or(|accepted| accepted != current_materialized_hash);
    let mut conflicts = Vec::new();
    let changes = classify_changes(&record, document, raw);
    let classification_counts = classification_counts(&changes);
    if local_changed {
        conflicts.push(json!({
            "kind":"untracked-local-edit",
            "instance":id,
            "page":record.page_id,
            "base":record.materialized_hash,
            "local":current_materialized_hash,
            "incoming":next_hash,
            "changes":changes.iter().filter(|change| change["local_changed"] == true).cloned().collect::<Vec<_>>(),
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
                    && override_targets_exist(raw, definition) => {}
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
        json!({"instance":id,"asset_id":record.asset_id,"page":record.page_id,"parent":null,"from_version":record.asset_version,"to_version":manifest.asset_version,"from_hash":record.content_hash,"to_hash":next_hash,"changed":record.content_hash!=next_hash,"local_changed":local_changed,"materialized":{"accepted":record.materialized_hash,"current":current_materialized_hash},"placement":{"page":record.page_id,"transform":record.transform,"visible":record.visible},"overrides":record.overrides,"changes":changes,"classification_counts":classification_counts,"conflicts":conflicts}),
    )
}

fn classification_counts(changes: &[Value]) -> Value {
    let kinds = [
        "added",
        "removed",
        "moved",
        "renamed",
        "type-changed",
        "geometry-changed",
        "style-changed",
        "text-changed",
        "locally-changed",
        "overridden",
        "unchanged",
    ];
    let mut counts = serde_json::Map::new();
    for kind in kinds {
        let count = if kind == "overridden" {
            changes
                .iter()
                .filter(|change| {
                    change["overrides"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty())
                })
                .count()
        } else {
            changes
                .iter()
                .filter(|change| change["kind"].as_str() == Some(kind))
                .count()
        };
        counts.insert(kind.into(), json!(count));
    }
    Value::Object(counts)
}

fn classify_changes(record: &InstanceRecord, document: &Value, incoming: &Value) -> Vec<Value> {
    let local_layers =
        selected_layers(document, &record.page_id, &record.layer_ids).unwrap_or_default();
    let overridden = record
        .property_definitions
        .iter()
        .filter(|(name, _)| record.overrides.contains_key(*name))
        .flat_map(|(name, definition)| {
            definition
                .get("targets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(move |target| {
                    Some((
                        target.get("object")?.as_str()?.to_owned(),
                        target.get("property")?.as_str()?.to_owned(),
                        name.clone(),
                    ))
                })
        })
        .collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut incoming_objects = Vec::new();
    collect_content_objects(incoming, &mut incoming_objects);
    let incoming_order = incoming_objects
        .iter()
        .filter_map(|object| object.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    for (base_index, mapping) in record.child_ids.iter().enumerate() {
        let Some(source_id) = mapping.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(materialized_id) = mapping.get("new_id").and_then(Value::as_str) else {
            continue;
        };
        let base = find_id_in_values(&record.base_layers, materialized_id);
        let local = find_id_in_values(&local_layers, materialized_id);
        let next = find_id(incoming, source_id);
        let kind = match (base, local, next) {
            (_, _, None) => "removed",
            (None, _, Some(_)) => "added",
            (Some(base), Some(_local), Some(next)) if object_kind(base) != object_kind(next) => {
                "type-changed"
            }
            (Some(base), Some(local), Some(next)) => {
                if base.get("content") != next.get("content") {
                    "text-changed"
                } else if ["d", "x", "y", "width", "height", "transform"]
                    .iter()
                    .any(|key| base.get(*key) != next.get(*key))
                {
                    "geometry-changed"
                } else if ["fill", "stroke", "stroke_width", "opacity", "style_ref"]
                    .iter()
                    .any(|key| base.get(*key) != next.get(*key))
                {
                    "style-changed"
                } else if incoming_order.iter().position(|id| *id == source_id) != Some(base_index)
                {
                    "moved"
                } else if base != local {
                    "locally-changed"
                } else {
                    "unchanged"
                }
            }
            _ => "locally-changed",
        };
        let local_changed = base != local;
        let override_paths = overridden
            .iter()
            .filter(|(object, _, _)| object == source_id)
            .map(|(_, property, name)| json!({"property":name,"path":property}))
            .collect::<Vec<_>>();
        changes.push(json!({
            "source_object_id":source_id,
            "materialized_id":materialized_id,
            "kind":kind,
            "local_changed":local_changed,
            "overrides":override_paths,
            "base":object_summary(base),
            "local":object_summary(local),
            "incoming":object_summary(next)
        }));
    }
    let mapped = record
        .child_ids
        .iter()
        .filter_map(|mapping| mapping.get("id").and_then(Value::as_str))
        .collect::<std::collections::HashSet<_>>();
    for object in incoming_objects {
        let Some(source_id) = object.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !mapped.contains(source_id) {
            changes.push(json!({"source_object_id":source_id,"materialized_id":null,"kind":"added","local_changed":false,"overrides":[],"base":null,"local":null,"incoming":object_summary(Some(object))}));
        }
    }
    let removed = changes
        .iter()
        .enumerate()
        .filter(|(_, change)| change["kind"] == "removed")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let added = changes
        .iter()
        .enumerate()
        .filter(|(_, change)| change["kind"] == "added")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut consumed = std::collections::HashSet::new();
    for removed_index in removed {
        if let Some(added_index) = added.iter().copied().find(|added_index| {
            !consumed.contains(added_index)
                && changes[removed_index]["base"] == changes[*added_index]["incoming"]
        }) {
            let new_id = changes[added_index]["source_object_id"].clone();
            changes[removed_index]["kind"] = json!("renamed");
            changes[removed_index]["incoming_source_object_id"] = new_id;
            consumed.insert(added_index);
        }
    }
    for index in consumed.into_iter().collect::<Vec<_>>().into_iter().rev() {
        changes.remove(index);
    }
    changes
}

fn collect_content_objects<'a>(value: &'a Value, output: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            if map.get("content").is_some() || map.get("d").is_some() {
                output.push(value);
            }
            map.values()
                .for_each(|child| collect_content_objects(child, output));
        }
        Value::Array(values) => values
            .iter()
            .for_each(|child| collect_content_objects(child, output)),
        _ => {}
    }
}

fn find_id_in_values<'a>(values: &'a [Value], id: &str) -> Option<&'a Value> {
    values.iter().find_map(|value| find_id(value, id))
}

fn find_id<'a>(value: &'a Value, id: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            if map.get("id").and_then(Value::as_str) == Some(id) {
                Some(value)
            } else {
                map.values().find_map(|child| find_id(child, id))
            }
        }
        Value::Array(values) => values.iter().find_map(|child| find_id(child, id)),
        _ => None,
    }
}

fn object_kind(value: &Value) -> &'static str {
    if value.get("content").is_some() {
        "text"
    } else if value.get("d").is_some() {
        "path"
    } else {
        "object"
    }
}

fn object_summary(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    json!({
        "kind":object_kind(value),
        "content":value.get("content"),
        "d":value.get("d"),
        "fill":value.get("fill"),
        "stroke":value.get("stroke"),
        "stroke_width":value.get("stroke_width"),
        "x":value.get("x"),"y":value.get("y"),
        "width":value.get("width"),"height":value.get("height")
    })
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
        && constraints_accept(definition.get("constraints"), value)
}

fn constraints_accept(constraints: Option<&Value>, value: &Value) -> bool {
    let Some(constraints) = constraints else {
        return true;
    };
    let Some(constraints) = constraints.as_object() else {
        return false;
    };
    if constraints
        .get("max_length")
        .and_then(Value::as_u64)
        .is_some_and(|max| {
            value
                .as_str()
                .is_none_or(|text| text.chars().count() > max as usize)
        })
    {
        return false;
    }
    if let Some(number) = value.as_f64() {
        if constraints
            .get("min")
            .and_then(Value::as_f64)
            .is_some_and(|min| number < min)
            || constraints
                .get("max")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max)
        {
            return false;
        }
    }
    constraints
        .get("enum")
        .and_then(Value::as_array)
        .is_none_or(|allowed| allowed.contains(value))
}

pub fn update(file: &Path, id: &str, source: &Path, dry_run: bool) -> Result<Value> {
    let destination_bytes = fs::read(file)?;
    let destination: Value = serde_json::from_slice(&destination_bytes)?;
    let source_bytes = fs::read(source)?;
    let source_raw: Value = serde_json::from_slice(&source_bytes)?;
    let plan = update_plan_value(&destination, id, &source_raw, &source_bytes)?;
    if !plan["changed"].as_bool().unwrap_or(false) {
        return Ok(json!({"ok":true,"changed":false,"instance":id}));
    }
    if plan["conflicts"].as_array().is_some_and(|a| !a.is_empty()) {
        bail!("instance update has conflicts; inspect the update plan")
    }
    if dry_run {
        return Ok(json!({"ok":true,"dry_run":true,"plan":plan}));
    }
    let (updated, result) = apply_update(destination, id, source_raw, &source_bytes)?;
    crate::transaction::commit_value(file, "instance-update", false, None, &updated)?;
    Ok(result)
}

pub fn update_resolved(
    file: &Path,
    id: &str,
    source: &Path,
    resolutions_file: &Path,
    dry_run: bool,
) -> Result<Value> {
    let destination_bytes = fs::read(file)?;
    let mut destination: Value = serde_json::from_slice(&destination_bytes)?;
    let source_bytes = fs::read(source)?;
    let mut source_raw: Value = serde_json::from_slice(&source_bytes)?;
    let resolutions: Value = serde_json::from_slice(&fs::read(resolutions_file)?)
        .context("resolution file must be valid JSON")?;
    let detached = apply_resolutions(&mut destination, id, &mut source_raw, &resolutions)?;
    if detached {
        if !dry_run {
            crate::transaction::commit_value(
                file,
                "instance-resolution-detach",
                false,
                None,
                &destination,
            )?;
        }
        return Ok(json!({"ok":true,"dry_run":dry_run,"instance":id,"resolution":"detach"}));
    }
    let plan = update_plan_value(&destination, id, &source_raw, &source_bytes)?;
    if plan["conflicts"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        bail!("resolution file does not resolve every conflict")
    }
    if dry_run {
        return Ok(json!({"ok":true,"dry_run":true,"plan":plan}));
    }
    let (updated, mut summary) = apply_update(destination, id, source_raw, &source_bytes)?;
    let change =
        crate::transaction::commit_value(file, "instance-update-resolved", false, None, &updated)?;
    summary["change"] = serde_json::to_value(change)?;
    summary["resolutions"] = resolutions;
    Ok(summary)
}

fn apply_resolutions(
    document: &mut Value,
    id: &str,
    source: &mut Value,
    resolutions: &Value,
) -> Result<bool> {
    let actions = resolutions
        .get("resolutions")
        .unwrap_or(resolutions)
        .as_array()
        .context("resolution file must be an array or contain a resolutions array")?;
    for resolution in actions.iter().filter(|resolution| {
        resolution
            .get("instance")
            .and_then(Value::as_str)
            .is_none_or(|instance| instance == id)
    }) {
        let action = resolution
            .get("action")
            .and_then(Value::as_str)
            .context("resolution lacks action")?;
        let property = resolution.get("property").and_then(Value::as_str);
        match action {
            "detach" => {
                document
                    .get_mut("instances")
                    .and_then(Value::as_array_mut)
                    .context("document has no instances")?
                    .retain(|instance| instance.get("id").and_then(Value::as_str) != Some(id));
                return Ok(true);
            }
            "take-source" => {
                let current: InstanceRecord = serde_json::from_value(inspect(document, id)?)?;
                let current_hash =
                    materialized_hash(document, &current.page_id, &current.layer_ids)?;
                let record = record_mut(document, id)?;
                record["materialized_hash"] = Value::String(current_hash);
                if let Some(property) = property {
                    record["overrides"]
                        .as_object_mut()
                        .context("invalid overrides")?
                        .remove(property);
                } else {
                    record["overrides"] = json!({});
                    record["local_patches"] = json!([]);
                }
            }
            "keep-local" => {
                let current: InstanceRecord = serde_json::from_value(inspect(document, id)?)?;
                let patches = local_patches(document, &current)?;
                let current_hash =
                    materialized_hash(document, &current.page_id, &current.layer_ids)?;
                let record = record_mut(document, id)?;
                record["local_patches"] = Value::Array(patches);
                record["materialized_hash"] = Value::String(current_hash);
                if let Some(property) = property {
                    record["overrides"]
                        .as_object_mut()
                        .context("invalid overrides")?
                        .remove(property);
                }
            }
            "map-target" => {
                let property = property.context("map-target requires property")?;
                let target = resolution
                    .get("target")
                    .and_then(Value::as_str)
                    .context("map-target requires target")?;
                let field = resolution
                    .get("field")
                    .and_then(Value::as_str)
                    .context("map-target requires field")?;
                if !contains_object_id(source, target) {
                    bail!("mapped target object not found: {target}")
                }
                let definition = source
                    .get_mut("asset")
                    .and_then(|asset| asset.get_mut("properties"))
                    .and_then(|properties| properties.get_mut(property))
                    .with_context(|| format!("incoming property not found: {property}"))?;
                definition["targets"] = json!([{"object":target,"property":field}]);
                let record = record_mut(document, id)?;
                record["property_definitions"][property] = definition.clone();
            }
            other => bail!("unsupported conflict resolution: {other}"),
        }
    }
    Ok(false)
}

fn record_mut<'a>(document: &'a mut Value, id: &str) -> Result<&'a mut Value> {
    document
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .and_then(|instances| {
            instances
                .iter_mut()
                .find(|instance| instance.get("id").and_then(Value::as_str) == Some(id))
        })
        .context("instance not found")
}

fn local_patches(document: &Value, record: &InstanceRecord) -> Result<Vec<Value>> {
    let local = selected_layers(document, &record.page_id, &record.layer_ids)?;
    let mut patches = Vec::new();
    for mapping in &record.child_ids {
        let Some(source_id) = mapping.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(materialized_id) = mapping.get("new_id").and_then(Value::as_str) else {
            continue;
        };
        let (Some(base), Some(current)) = (
            find_id_in_values(&record.base_layers, materialized_id),
            find_id_in_values(&local, materialized_id),
        ) else {
            continue;
        };
        let Some(base) = base.as_object() else {
            continue;
        };
        let Some(current) = current.as_object() else {
            continue;
        };
        for (property, value) in current {
            if property != "id" && base.get(property) != Some(value) {
                patches.push(json!({"object":source_id,"property":property,"value":value}));
            }
        }
    }
    Ok(patches)
}

fn apply_update(
    mut destination: Value,
    id: &str,
    source_raw: Value,
    source_bytes: &[u8],
) -> Result<(Value, Value)> {
    let old_value = inspect(&destination, id)?;
    let old: InstanceRecord = serde_json::from_value(old_value)?;
    let old_positions = layer_positions(&destination, &old.page_id, &old.layer_ids)?;
    let preserved_states = if old.layer_states.is_empty() {
        layer_states(&destination, &old.page_id, &old.layer_ids)?
    } else {
        old.layer_states.clone()
    };
    let old_layers = take_layers(&mut destination, &old.page_id, &old.layer_ids)?;
    let manifest = crate::asset::manifest(&source_raw)?.context("source lacks asset metadata")?;
    let new_hash = crate::asset::hash_bytes(source_bytes);
    let sx = (old.transform[0] * old.transform[0] + old.transform[1] * old.transform[1]).sqrt();
    let rotation = old.transform[1].atan2(old.transform[0]).to_degrees();
    let result = crate::import::compose(
        destination,
        source_raw,
        &crate::import::ImportOptions {
            destination_page: Some(old.page_id.clone()),
            source_page: manifest.entry_page.clone(),
            // Instance-derived IDs stay readable and stable across revisions.
            // The old materialization has already been removed in memory, so the
            // stable prefix cannot collide with itself during replacement.
            prefix: Some(id.to_owned()),
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
    restore_layer_states(&mut updated, &old.page_id, &new_ids, &preserved_states)?;
    let next_base_layers = selected_layers(&updated, &old.page_id, &new_ids)?;
    for (name, value) in &old.overrides {
        let definition = manifest
            .properties
            .get(name)
            .with_context(|| format!("override target removed during update: {name}"))?;
        apply_override(&mut updated, &new_ids, definition, value)?;
    }
    for patch in &old.local_patches {
        let object = patch
            .get("object")
            .and_then(Value::as_str)
            .context("local patch lacks object")?;
        let property = patch
            .get("property")
            .and_then(Value::as_str)
            .context("local patch lacks property")?;
        let value = patch.get("value").context("local patch lacks value")?;
        set_object_property(&mut updated, &new_ids, object, property, value)?;
    }
    let next_materialized_hash = materialized_hash(&updated, &old.page_id, &new_ids)?;
    let next_layer_states = layer_states(&updated, &old.page_id, &new_ids)?;
    let record = updated
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .and_then(|a| {
            a.iter_mut()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(id))
        })
        .context("instance record lost during update")?;
    let history = json!({"asset_version":old.asset_version,"content_hash":old.content_hash,"layer_ids":old.layer_ids,"layers":old_layers,"positions":old_positions,"materialized_hash":old.materialized_hash,"base_layers":old.base_layers,"property_definitions":old.property_definitions,"child_ids":old.child_ids,"local_patches":old.local_patches,"parent_id":old.parent_id,"layer_indices":old.layer_indices,"layer_states":old.layer_states});
    record["asset_version"] = Value::String(manifest.asset_version.clone());
    record["content_hash"] = Value::String(new_hash.clone());
    record["layer_ids"] = serde_json::to_value(&new_ids)?;
    record["layer_indices"] = serde_json::to_value(&old_positions)?;
    record["layer_states"] = serde_json::to_value(next_layer_states)?;
    record["materialized_hash"] = Value::String(next_materialized_hash);
    record["base_layers"] = Value::Array(next_base_layers);
    record["property_definitions"] = serde_json::to_value(&manifest.properties)?;
    record["child_ids"] = result.summary["objects"].clone();
    record["local_patches"] = serde_json::to_value(&old.local_patches)?;
    record["previous"]
        .as_array_mut()
        .context("invalid instance history")?
        .push(history);
    let summary = json!({"ok":true,"instance":id,"from":old.asset_version,"to":manifest.asset_version,"content_hash":new_hash,"layers":new_ids,"page":old.page_id,"positions":old_positions});
    Ok((updated, summary))
}

#[allow(clippy::too_many_arguments)]
pub fn update_bulk(
    file: &Path,
    source: &Path,
    asset_filter: Option<&str>,
    page_filter: Option<&str>,
    group_filter: Option<&str>,
    version_filter: Option<&str>,
    hash_filter: Option<&str>,
    package_filter: Option<&str>,
    stale_only: bool,
    continue_on_conflict: bool,
    resolutions_file: Option<&Path>,
    dry_run: bool,
) -> Result<Value> {
    let before = fs::read(file)?;
    let mut document: Value = serde_json::from_slice(&before)?;
    let source_bytes = fs::read(source)?;
    let source_raw: Value = serde_json::from_slice(&source_bytes)?;
    let source_manifest =
        crate::asset::manifest(&source_raw)?.context("source lacks asset metadata")?;
    let source_hash = crate::asset::hash_bytes(&source_bytes);
    let ids =
        document
            .get("instances")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|instance| {
                asset_filter.is_none_or(|asset| {
                    instance.get("asset_id").and_then(Value::as_str) == Some(asset)
                }) && page_filter.is_none_or(|page| {
                    instance.get("page_id").and_then(Value::as_str) == Some(page)
                }) && group_filter.is_none_or(|group| {
                    instance.get("parent_id").and_then(Value::as_str) == Some(group)
                }) && version_filter.is_none_or(|version| {
                    instance.get("asset_version").and_then(Value::as_str) == Some(version)
                }) && hash_filter.is_none_or(|hash| {
                    instance.get("content_hash").and_then(Value::as_str) == Some(hash)
                }) && package_filter.is_none_or(|package| {
                    instance.get("library").and_then(Value::as_str) == Some(package)
                }) && (!stale_only
                    || instance.get("content_hash").and_then(Value::as_str)
                        != Some(source_hash.as_str()))
                    && instance.get("asset_id").and_then(Value::as_str)
                        == Some(source_manifest.id.as_str())
            })
            .filter_map(|instance| {
                instance
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
    if ids.is_empty() {
        bail!("no instances match the requested update scope")
    }
    let mut plans = Vec::with_capacity(ids.len());
    let mut pending = Vec::with_capacity(ids.len());
    let mut skipped = Vec::new();
    let resolutions = resolutions_file
        .map(|path| {
            serde_json::from_slice::<Value>(&fs::read(path)?)
                .context("resolution file must be valid JSON")
        })
        .transpose()?;
    for id in ids {
        let mut instance_source = source_raw.clone();
        let mut candidate = document.clone();
        let mut detached = false;
        if let Some(resolutions) = &resolutions {
            if apply_resolutions(&mut candidate, &id, &mut instance_source, resolutions)? {
                plans.push(json!({"instance":id,"resolution":"detach","conflicts":[]}));
                detached = true;
            }
        }
        if detached {
            pending.push((id, instance_source, true, false));
            continue;
        }
        let plan = update_plan_value(&candidate, &id, &instance_source, &source_bytes)?;
        let conflicts = plan["conflicts"]
            .as_array()
            .is_some_and(|items| !items.is_empty());
        if conflicts {
            skipped.push(id.clone());
        }
        let changed = plan["changed"].as_bool() == Some(true);
        pending.push((id, instance_source, false, conflicts || !changed));
        plans.push(plan);
    }
    if !skipped.is_empty() && !continue_on_conflict && !dry_run {
        bail!("bulk instance update has conflicts; no changes were committed")
    }
    let mut updated_ids = Vec::new();
    if !dry_run {
        for (id, instance_source, detached, skip) in &mut pending {
            if *skip {
                continue;
            }
            if let Some(resolutions) = &resolutions {
                let resolved_detach =
                    apply_resolutions(&mut document, id, instance_source, resolutions)?;
                if resolved_detach {
                    updated_ids.push(id.clone());
                    continue;
                }
            }
            if *detached {
                updated_ids.push(id.clone());
                continue;
            }
            let (next, _) = apply_update(document, id, instance_source.clone(), &source_bytes)?;
            document = next;
            updated_ids.push(id.clone());
        }
    } else {
        updated_ids.extend(
            pending
                .iter()
                .filter(|(_, _, _, skip)| !skip)
                .map(|(id, _, _, _)| id.clone()),
        );
    }
    let change = if dry_run || updated_ids.is_empty() {
        Value::Null
    } else {
        serde_json::to_value(crate::transaction::commit_value(
            file,
            "instance-update-bulk",
            false,
            None,
            &document,
        )?)?
    };
    Ok(json!({
        "ok": skipped.is_empty() || continue_on_conflict,
        "dry_run":dry_run,
        "asset_id":source_manifest.id,
        "plans":plans,
        "counts":{"matched":plans.len(),"updated":updated_ids.len(),"skipped":skipped.len()},
        "updated":updated_ids,
        "skipped":skipped,
        "all_or_nothing":!continue_on_conflict,
        "change":change
    }))
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
    record["child_ids"] = previous
        .get("child_ids")
        .cloned()
        .unwrap_or_else(|| json!([]));
    record["local_patches"] = previous
        .get("local_patches")
        .cloned()
        .unwrap_or_else(|| json!([]));
    record["parent_id"] = previous.get("parent_id").cloned().unwrap_or(Value::Null);
    record["layer_indices"] = previous
        .get("layer_indices")
        .cloned()
        .unwrap_or_else(|| previous["positions"].clone());
    record["layer_states"] = previous
        .get("layer_states")
        .cloned()
        .unwrap_or_else(|| json!([]));
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

pub fn layer_positions_public(raw: &Value, page_id: &str, ids: &[String]) -> Result<Vec<usize>> {
    layer_positions(raw, page_id, ids)
}

pub fn layer_states(raw: &Value, page_id: &str, ids: &[String]) -> Result<Vec<Value>> {
    Ok(selected_layers(raw, page_id, ids)?
        .into_iter()
        .map(|layer| json!({"visible":layer.get("visible").cloned().unwrap_or(json!(true)),"locked":layer.get("locked").cloned().unwrap_or(json!(false))}))
        .collect())
}

fn restore_layer_states(
    raw: &mut Value,
    page_id: &str,
    ids: &[String],
    states: &[Value],
) -> Result<()> {
    if states.is_empty() {
        return Ok(());
    }
    let layers = page_mut(raw, page_id)?
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers missing")?;
    for (id, state) in ids.iter().zip(states) {
        let layer = layers
            .iter_mut()
            .find(|layer| layer.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("instance layer missing: {id}"))?;
        layer["visible"] = state.get("visible").cloned().unwrap_or(json!(true));
        layer["locked"] = state.get("locked").cloned().unwrap_or(json!(false));
    }
    Ok(())
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
    let mut layers = Value::Array(selected_layers(raw, page_id, ids)?);
    canonicalize_numbers(&mut layers);
    Ok(crate::asset::hash_bytes(&serde_json::to_vec(&layers)?))
}

fn canonicalize_numbers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(value) = number.as_f64() {
                if value.is_finite()
                    && value.fract() == 0.0
                    && value >= i64::MIN as f64
                    && value <= i64::MAX as f64
                {
                    *number = serde_json::Number::from(value as i64);
                } else if let Some(value) = serde_json::Number::from_f64(value) {
                    *number = value;
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(canonicalize_numbers),
        Value::Object(map) => map.values_mut().for_each(canonicalize_numbers),
        _ => {}
    }
}

pub fn annotate_inspection(document: &Value, output: &mut Value) {
    let mappings = document
        .get("instances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|instance| {
            let instance_id = instance.get("id").and_then(Value::as_str).unwrap_or("");
            instance
                .get("child_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(move |mapping| {
                    Some((
                        mapping.get("new_id")?.as_str()?.to_owned(),
                        instance_id.to_owned(),
                        mapping.get("id")?.as_str()?.to_owned(),
                    ))
                })
        })
        .collect::<Vec<_>>();
    fn visit(value: &mut Value, mappings: &[(String, String, String)]) {
        match value {
            Value::Object(map) => {
                if let Some(id) = map.get("id").and_then(Value::as_str) {
                    if let Some((_, instance, source)) =
                        mappings.iter().find(|(new, _, _)| new == id)
                    {
                        map.insert("instance_id".into(), Value::String(instance.clone()));
                        map.insert("source_id".into(), Value::String(source.clone()));
                    }
                }
                map.values_mut().for_each(|value| visit(value, mappings));
            }
            Value::Array(values) => values.iter_mut().for_each(|value| visit(value, mappings)),
            _ => {}
        }
    }
    visit(output, &mappings);
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
    fn inspection_exposes_source_and_stable_materialized_ids() {
        let document = json!({"instances":[{"id":"card-2","child_ids":[{"id":"label","new_id":"card-2-label"}]}]});
        let mut output = json!({"layers":[{"objects":[{"id":"card-2-label","kind":"text"}]}]});
        annotate_inspection(&document, &mut output);
        assert_eq!(output["layers"][0]["objects"][0]["source_id"], "label");
        assert_eq!(output["layers"][0]["objects"][0]["instance_id"], "card-2");
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
                parent_id: None,
                layer_indices: vec![0],
                layer_states: vec![],
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
                child_ids: vec![],
                local_patches: vec![],
                materialized_hash: Some(accepted),
                base_layers: vec![],
                previous: vec![],
            },
        )
        .unwrap();
        let source = json!({
            "format":"pentool","version":3,"name":"Card","fonts":[],
            "asset":{"id":"ui/card","name":"Card","asset_version":"1.1.0","entry_page":"page-1","properties":{"label":{"type":"text","targets":[{"object":"label","property":"content"}]}}},
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
                parent_id: None,
                layer_indices: vec![1],
                layer_states: vec![],
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
                child_ids: vec![],
                local_patches: vec![],
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
        assert_eq!(layers[1]["id"], "card-content");
        assert_eq!(layers[1]["texts"][0]["id"], "card-label");
        assert_eq!(layers[1]["texts"][0]["content"], "Local label");
        assert_eq!(updated["instances"][0]["asset_version"], "1.1.0");
        rollback(&document_path, "card", false).unwrap();
        let rolled_back: Value =
            serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
        let rolled_back_layers = rolled_back["pages"][0]["layers"].as_array().unwrap();
        assert_eq!(rolled_back_layers[0]["id"], "back");
        assert_eq!(rolled_back_layers[1]["id"], "card-old-content");
        assert_eq!(rolled_back_layers[2]["id"], "check");
        assert_eq!(rolled_back["instances"][0]["asset_version"], "1.0.0");
        let mut locally_edited = rolled_back;
        locally_edited["pages"][0]["layers"][1]["texts"][0]["content"] = json!("Undeclared");
        fs::write(&document_path, serde_json::to_vec(&locally_edited).unwrap()).unwrap();
        let resolutions_path = temp_file("resolutions.json");
        fs::write(
            &resolutions_path,
            br#"{"resolutions":[{"instance":"card","action":"take-source"}]}"#,
        )
        .unwrap();
        update_resolved(
            &document_path,
            "card",
            &source_path,
            &resolutions_path,
            false,
        )
        .unwrap();
        let resolved: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
        assert_eq!(
            resolved["pages"][0]["layers"][1]["texts"][0]["content"],
            "Source 1.1"
        );
        let _ = fs::remove_file(document_path);
        let _ = fs::remove_file(source_path);
        let _ = fs::remove_file(resolutions_path);
    }
}

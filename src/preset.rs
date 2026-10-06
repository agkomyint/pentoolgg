//! Deterministic data-only appearances with explicit engine compatibility.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;
const KEYS: &[&str] = &[
    "style",
    "opacity",
    "content_opacity",
    "blend_mode",
    "blend_space",
    "isolation",
    "effects",
    "effect_origin",
    "transforms",
    "mask",
    "operations",
    "fill",
    "adjustment",
    "params",
    "enabled",
];

pub fn capture(raw: &Value, document: &Path, page: Option<&str>, id: &str) -> Result<Value> {
    crate::scene::validate(raw)?;
    if !crate::composite::is_document(raw) {
        bail!("appearance presets require explicit migration to v6")
    }
    let (node, _) = crate::composite::node_ref(raw, page, id)?;
    let appearance = KEYS
        .iter()
        .filter_map(|key| {
            node.get(*key)
                .map(|value| ((*key).to_string(), value.clone()))
        })
        .collect::<serde_json::Map<_, _>>();
    let mut needed = std::collections::BTreeSet::<(String, String)>::new();
    fn scan(value: &Value, needed: &mut std::collections::BTreeSet<(String, String)>) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    if let Some(name) = value.as_str() {
                        let table = match key.as_str() {
                            "ref" => Some("styles"),
                            "asset" => Some("image_assets"),
                            "resource" => Some("mask_resources"),
                            _ => None,
                        };
                        if let Some(table) = table {
                            needed.insert((table.into(), name.into()));
                        }
                    }
                    scan(value, needed);
                }
            }
            Value::Array(values) => {
                for value in values {
                    scan(value, needed);
                }
            }
            _ => {}
        }
    }
    scan(&Value::Object(appearance.clone()), &mut needed);
    let mut visited = std::collections::BTreeSet::new();
    loop {
        let pending = needed.difference(&visited).cloned().collect::<Vec<_>>();
        if pending.is_empty() {
            break;
        }
        for (table, name) in pending {
            visited.insert((table.clone(), name.clone()));
            if let Some(value) = raw[&table].get(&name) {
                scan(value, &mut needed);
            }
        }
        if needed.len() > 4096 {
            bail!("[limit-exceeded] preset dependency closure exceeds 4096 entries")
        }
    }
    let table = |key: &str| {
        needed
            .iter()
            .filter(|(table, _)| table == key)
            .filter_map(|(_, name)| {
                raw[key]
                    .get(name)
                    .map(|value| (name.clone(), value.clone()))
            })
            .collect::<serde_json::Map<_, _>>()
    };
    let mut assets = Value::Object(table("image_assets"));
    let masks = table("mask_resources");
    let styles = table("styles");
    let mut bytes_total = 0usize;
    for (digest, asset) in assets.as_object_mut().unwrap() {
        let bytes =
            crate::image::load_asset_bytes(raw, crate::resource::document_root(document), digest)?;
        bytes_total = bytes_total
            .checked_add(bytes.len())
            .context("preset size overflow")?;
        if bytes_total > crate::resource::MAX_RESOURCE_BYTES {
            bail!("[limit-exceeded] preset sources exceed 512 MiB")
        }
        asset["storage"] = crate::image::embedded_storage(&bytes);
    }
    let package = json!({"format":"penpreset","version":1,"engine":1,"color_space":"srgb8","node_kind":node["kind"],"appearance":appearance,"image_assets":assets,"mask_resources":masks,"styles":styles});
    if serde_json::to_vec(&package)?.len() > crate::resource::MAX_RESOURCE_BYTES {
        bail!("[limit-exceeded] encoded preset exceeds 512 MiB")
    }
    Ok(package)
}

pub fn apply(raw: &mut Value, page: Option<&str>, id: &str, preset: &Value) -> Result<Value> {
    crate::scene::validate(raw)?;
    if !crate::composite::is_document(raw) {
        bail!("appearance presets require v6")
    }
    let object = preset.as_object().context("preset must be an object")?;
    for key in object.keys() {
        if ![
            "format",
            "version",
            "engine",
            "color_space",
            "node_kind",
            "appearance",
            "image_assets",
            "mask_resources",
            "styles",
        ]
        .contains(&key.as_str())
        {
            bail!("[invalid-preset] unknown package field {key}")
        }
    }
    if preset["format"] != "penpreset"
        || preset["version"] != 1
        || preset["engine"] != 1
        || preset["color_space"] != "srgb8"
    {
        bail!("[incompatible-preset] requires penpreset v1, engine 1, srgb8")
    }
    let (node, _) = crate::composite::node_ref(raw, page, id)?;
    if node["kind"] != preset["node_kind"] {
        bail!("[incompatible-preset] node kind must match; geometry and IDs are never replaced")
    }
    let appearance = preset["appearance"]
        .as_object()
        .context("appearance must be an object")?;
    for key in appearance.keys() {
        if !KEYS.contains(&key.as_str()) {
            bail!("[invalid-preset] unknown appearance property {key}")
        }
    }
    let mut candidate = raw.clone();
    crate::image::validate_assets(&json!({"image_assets":preset["image_assets"]}))?;
    for asset in preset["image_assets"].as_object().unwrap().values() {
        if asset["storage"]["kind"] != "embedded" {
            bail!("[invalid-preset] preset sources must be embedded")
        }
    }
    for key in ["image_assets", "mask_resources", "styles"] {
        let incoming = preset[key]
            .as_object()
            .with_context(|| format!("preset {key} must be an object"))?;
        if candidate.get(key).is_none() {
            candidate[key] = json!({});
        }
        let destination = candidate[key]
            .as_object_mut()
            .context("resource table malformed")?;
        for (name, value) in incoming {
            if let Some(existing) = destination.get(name) {
                if key == "image_assets" {
                    // Storage is transport metadata; verified same-hash assets may remain linked.
                    let mut a = existing.clone();
                    let mut b = value.clone();
                    a.as_object_mut().unwrap().remove("storage");
                    b.as_object_mut()
                        .context("asset must be an object")?
                        .remove("storage");
                    if a != b {
                        bail!("[preset-conflict] asset metadata conflicts at {name}")
                    }
                } else if existing != value {
                    bail!("[preset-conflict] {key} {name} differs; rename it before applying")
                }
            } else {
                destination.insert(name.clone(), value.clone());
            }
        }
    }
    let page = crate::scene::page_mut(&mut candidate, page)?;
    let layer_id = page["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|layer| {
            let mut found = false;
            fn contains(node: &Value, id: &str) -> bool {
                node["id"] == id
                    || node["children"]
                        .as_array()
                        .is_some_and(|children| children.iter().any(|n| contains(n, id)))
            }
            if let Some(nodes) = layer["nodes"].as_array() {
                found = nodes.iter().any(|n| contains(n, id));
            }
            found.then(|| layer["id"].as_str().unwrap().to_owned())
        })
        .context("target layer missing")?;
    crate::scene::ensure_layer_unlocked(page, &layer_id)?;
    crate::composite::ensure_unlocked(page, id)?;
    let node = crate::scene::find_node_mut(page, id).context("target node missing")?;
    let node = node.as_object_mut().unwrap();
    for key in KEYS {
        node.remove(*key);
    }
    node.extend(appearance.clone());
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(json!({"node":id,"properties":appearance.keys().collect::<Vec<_>>()}))
}

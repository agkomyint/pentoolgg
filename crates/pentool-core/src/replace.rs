//! Scoped bulk value replacement for v4 scene nodes.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

#[derive(Debug)]
pub struct Options<'a> {
    pub page: Option<&'a str>,
    pub all_pages: bool,
    pub layer: Option<&'a str>,
    pub group: Option<&'a str>,
    pub object_type: Option<&'a str>,
    pub visible: Option<bool>,
    pub property: &'a str,
    pub from: &'a str,
    pub to: &'a str,
}

pub fn apply(raw: &mut Value, options: &Options<'_>) -> Result<Value> {
    crate::scene::validate(raw)?;
    if options.page.is_none() && !options.all_pages {
        bail!("choose --page or explicitly use --all-pages")
    }
    let pages = raw.get_mut("pages").and_then(Value::as_array_mut).unwrap();
    if let Some(page) = options.page {
        let found = pages
            .iter()
            .any(|p| p.get("id").and_then(Value::as_str) == Some(page));
        if !found {
            bail!("page not found: {page}")
        }
    }
    if let Some(layer) = options.layer {
        let mut found = false;
        for page in pages.iter() {
            let id = page.get("id").and_then(Value::as_str).unwrap_or_default();
            if !options.all_pages && options.page != Some(id) {
                continue;
            }
            if let Some(layers) = page.get("layers").and_then(Value::as_array) {
                if layers
                    .iter()
                    .any(|l| l.get("id").and_then(Value::as_str) == Some(layer))
                {
                    found = true;
                    break;
                }
            }
        }
        if !found {
            bail!("layer not found: {layer}")
        }
    }
    if let Some(kind) = options.object_type {
        fn has_kind(node: &Value, kind: &str) -> bool {
            if node.get("kind").and_then(Value::as_str) == Some(kind) {
                return true;
            }
            if node.get("kind").and_then(Value::as_str) == Some("instance") {
                return false;
            }
            node.get("children")
                .and_then(Value::as_array)
                .is_some_and(|children| children.iter().any(|child| has_kind(child, kind)))
        }
        let mut found = false;
        for page in pages.iter() {
            let id = page.get("id").and_then(Value::as_str).unwrap_or_default();
            if !options.all_pages && options.page != Some(id) {
                continue;
            }
            if let Some(layers) = page.get("layers").and_then(Value::as_array) {
                for layer in layers {
                    if options.layer.is_some()
                        && options.layer != layer.get("id").and_then(Value::as_str)
                    {
                        continue;
                    }
                    if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
                        if nodes.iter().any(|node| has_kind(node, kind)) {
                            found = true;
                            break;
                        }
                    }
                }
            }
            if found {
                break;
            }
        }
        if !found {
            bail!("object type \"{kind}\" matches no nodes in scope")
        }
    }
    let mut ids = Vec::new();
    for page in pages {
        let page_id = page.get("id").and_then(Value::as_str).unwrap_or_default();
        if !options.all_pages && options.page != Some(page_id) {
            continue;
        }
        let layers = page
            .get_mut("layers")
            .and_then(Value::as_array_mut)
            .context("page layers are missing")?;
        for layer in layers {
            if options.layer.is_some() && options.layer != layer.get("id").and_then(Value::as_str) {
                continue;
            }
            let nodes = layer
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .context("layer nodes are missing")?;
            if let Some(group) = options.group {
                let target =
                    find(nodes, group).with_context(|| format!("group not found: {group}"))?;
                if target.get("kind").and_then(Value::as_str) != Some("group") {
                    bail!("scope is not a group: {group}")
                }
                visit(target, options, &mut ids);
            } else {
                for node in nodes {
                    visit(node, options, &mut ids);
                }
            }
        }
    }
    Ok(
        json!({"matched_ids":ids,"before_count":ids.len(),"after_count":ids.len(),"property":options.property,"from":options.from,"to":options.to}),
    )
}

fn find<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    for node in nodes {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            if let Some(found) = find(children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn visit(node: &mut Value, options: &Options<'_>, ids: &mut Vec<String>) {
    let kind = node
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let node_id = node
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let visible = node.get("visible").and_then(Value::as_bool).unwrap_or(true);
    let eligible = options.object_type.is_none_or(|wanted| wanted == kind)
        && options.visible.is_none_or(|wanted| wanted == visible);
    if eligible {
        let target = node
            .get_mut("style")
            .and_then(|v| v.get_mut(options.property));
        if let Some(target) = target {
            let matched = if let Some(object) = target.as_object() {
                object.get("fallback").and_then(Value::as_str) == Some(options.from)
                    || object.get("ref").and_then(Value::as_str) == Some(options.from)
            } else {
                target.as_str() == Some(options.from)
            };
            if matched {
                if let Some(object) = target.as_object_mut() {
                    if object.get("ref").and_then(Value::as_str) == Some(options.from) {
                        object.insert("ref".into(), json!(options.to));
                    } else {
                        object.insert("fallback".into(), json!(options.to));
                    }
                } else {
                    *target = json!(options.to)
                }
                ids.push(node_id);
            }
        }
    }
    if kind != "instance" {
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            for child in children {
                visit(child, options, ids)
            }
        }
    }
}

//! Version 4 ordered scene graph migration and validation.
use anyhow::{bail, Context, Result};
use kurbo::{Affine, BezPath, Rect, Shape};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::collections::HashSet;

pub const VERSION: u64 = 4;
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ShapeKind {
    Rect,
    Rrect,
    Ellipse,
    Circle,
    Line,
}

#[derive(Debug)]
pub enum GroupAction {
    Create {
        id: String,
        layer: String,
        children: Vec<String>,
    },
    Move {
        id: String,
        dx: f64,
        dy: f64,
    },
    Rotate {
        id: String,
        degrees: f64,
    },
    Scale {
        id: String,
        scale_x: f64,
        scale_y: f64,
    },
    Duplicate {
        source: String,
        id: String,
        dx: f64,
        dy: f64,
    },
    Ungroup {
        id: String,
    },
    Rename {
        id: String,
        new_id: String,
    },
    Reorder {
        id: String,
        index: usize,
    },
    AddChild {
        group: String,
        child: String,
    },
    RemoveChild {
        group: String,
        child: String,
        layer: String,
    },
    Bounds {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum GroupOperation {
    Create,
    Move,
    Rotate,
    Scale,
    Duplicate,
    Ungroup,
    Rename,
    Reorder,
    Bounds,
    AddChild,
    RemoveChild,
    Promote,
    Instantiate,
}

pub fn apply_group(raw: &mut Value, page_id: Option<&str>, action: GroupAction) -> Result<Value> {
    validate(raw)?;
    let page = page_mut(raw, page_id)?;
    let result = match action {
        GroupAction::Create {
            id,
            layer,
            children,
        } => {
            if children.is_empty() {
                bail!("group requires at least one child")
            };
            if pages_node_ids(page).contains(id.as_str()) {
                bail!("node ID already exists on page: {id}")
            };
            let layer = layer_nodes_mut(page, &layer)?;
            let wanted: HashSet<&str> = children.iter().map(String::as_str).collect();
            let mut found = Vec::new();
            let mut kept = Vec::new();
            let mut insert_at = None;
            for node in std::mem::take(layer) {
                if node
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|node_id| wanted.contains(node_id))
                {
                    insert_at.get_or_insert(kept.len());
                    found.push(node)
                } else {
                    kept.push(node)
                }
            }
            if found.len() != wanted.len() {
                bail!("one or more group children were not found as top-level nodes")
            };
            let ordered: Vec<Value> = children
                .iter()
                .map(|id| {
                    found
                        .iter()
                        .find(|n| n.get("id").and_then(Value::as_str) == Some(id))
                        .unwrap()
                        .clone()
                })
                .collect();
            kept.insert(
                insert_at.unwrap_or(kept.len()),
                json!({"kind":"group","id":id,"transform":[1,0,0,1,0,0],"children":ordered}),
            );
            *layer = kept;
            json!({"group":id,"children":children})
        }
        GroupAction::Move { id, dx, dy } => {
            if !dx.is_finite() || !dy.is_finite() {
                bail!("group movement must be finite")
            };
            let missing = missing_id(page, &id, "group");
            let node = find_node_mut(page, &id).context(missing)?;
            if node.get("kind").and_then(Value::as_str) != Some("group") {
                bail!("node is not a group")
            };
            let next = Affine::translate((dx, dy)) * matrix(node.get("transform"))?;
            node["transform"] = json!(next.as_coeffs());
            json!({"group":id,"dx":dx,"dy":dy})
        }
        GroupAction::Rotate { id, degrees } => {
            if !degrees.is_finite() {
                bail!("group rotation must be finite")
            };
            let missing = missing_id(page, &id, "group");
            let node = find_node_mut(page, &id).context(missing)?;
            require_group(node)?;
            let next = Affine::rotate(degrees.to_radians()) * matrix(node.get("transform"))?;
            node["transform"] = json!(next.as_coeffs());
            json!({"group":id,"degrees":degrees})
        }
        GroupAction::Scale {
            id,
            scale_x,
            scale_y,
        } => {
            if !scale_x.is_finite() || !scale_y.is_finite() || scale_x == 0.0 || scale_y == 0.0 {
                bail!("group scale values must be finite and nonzero")
            };
            let missing = missing_id(page, &id, "group");
            let node = find_node_mut(page, &id).context(missing)?;
            require_group(node)?;
            let next = Affine::scale_non_uniform(scale_x, scale_y) * matrix(node.get("transform"))?;
            node["transform"] = json!(next.as_coeffs());
            json!({"group":id,"scale_x":scale_x,"scale_y":scale_y})
        }
        GroupAction::Duplicate { source, id, dx, dy } => {
            if pages_node_ids(page).contains(id.as_str()) {
                bail!("node ID already exists on page: {id}")
            };
            let mut copy = find_node(page, &source)
                .with_context(|| missing_id(page, &source, "group"))?
                .clone();
            if copy.get("kind").and_then(Value::as_str) != Some("group") {
                bail!("node is not a group")
            };
            rename_tree(&mut copy, &id, true);
            let next = Affine::translate((dx, dy)) * matrix(copy.get("transform"))?;
            copy["transform"] = json!(next.as_coeffs());
            insert_after(page, &source, copy)?;
            json!({"group":source,"duplicate":id,"dx":dx,"dy":dy})
        }
        GroupAction::Ungroup { id } => {
            let count = ungroup(page, &id)?;
            json!({"group":id,"ungrouped_children":count})
        }
        GroupAction::Rename { id, new_id } => {
            if new_id.is_empty() || pages_node_ids(page).contains(new_id.as_str()) {
                bail!("new group ID must be nonempty and unique: {new_id}")
            }
            let missing = missing_id(page, &id, "group");
            let node = find_node_mut(page, &id).context(missing)?;
            require_group(node)?;
            node["id"] = json!(new_id);
            json!({"group":id,"renamed":new_id})
        }
        GroupAction::Reorder { id, index } => {
            let applied = reorder_node(page, &id, index)?;
            json!({"group":id,"index":applied})
        }
        GroupAction::Bounds { id } => node_bounds_on_page(page, &id)?,
        GroupAction::AddChild { group, child } => {
            if group == child {
                bail!("a group cannot contain itself")
            }
            let node = take_node(page, &child)?;
            let target = find_node_mut(page, &group).context("group not found")?;
            require_group(target)?;
            if find_node_in(
                node.get("children")
                    .and_then(Value::as_array)
                    .unwrap_or(&Vec::new()),
                &group,
            )
            .is_some()
            {
                bail!("reparenting would create a cycle")
            }
            target
                .get_mut("children")
                .and_then(Value::as_array_mut)
                .unwrap()
                .push(node);
            json!({"group":group,"added":child})
        }
        GroupAction::RemoveChild {
            group,
            child,
            layer,
        } => {
            let target = find_node_mut(page, &group).context("group not found")?;
            require_group(target)?;
            let children = target
                .get_mut("children")
                .and_then(Value::as_array_mut)
                .unwrap();
            let index = children
                .iter()
                .position(|node| node.get("id").and_then(Value::as_str) == Some(&child))
                .with_context(|| format!("child not found in group: {child}"))?;
            let node = children.remove(index);
            layer_nodes_mut(page, &layer)?.push(node);
            json!({"group":group,"removed":child,"layer":layer})
        }
    };
    validate(raw)?;
    Ok(result)
}

fn find_node_in<'a>(nodes: &'a [Value], id: &str) -> Option<&'a Value> {
    for node in nodes {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(found) = node
            .get("children")
            .and_then(Value::as_array)
            .and_then(|children| find_node_in(children, id))
        {
            return Some(found);
        }
    }
    None
}
fn take_node(page: &mut Value, id: &str) -> Result<Value> {
    fn take(nodes: &mut Vec<Value>, id: &str) -> Option<Value> {
        if let Some(index) = nodes
            .iter()
            .position(|node| node.get("id").and_then(Value::as_str) == Some(id))
        {
            return Some(nodes.remove(index));
        }
        for node in nodes {
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                if let Some(found) = take(children, id) {
                    return Some(found);
                }
            }
        }
        None
    }
    for layer in page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers missing")?
    {
        if let Some(node) = take(
            layer
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .context("layer nodes missing")?,
            id,
        ) {
            return Ok(node);
        }
    }
    bail!("node not found: {id}")
}

pub fn node_bounds(raw: &Value, page_id: Option<&str>, id: &str) -> Result<Value> {
    validate(raw)?;
    let pages = raw.get("pages").and_then(Value::as_array).unwrap();
    let page = match page_id {
        Some(wanted) => pages
            .iter()
            .find(|page| page.get("id").and_then(Value::as_str) == Some(wanted))
            .with_context(|| format!("page not found: {wanted}"))?,
        None => pages.first().unwrap(),
    };
    node_bounds_on_page(page, id)
}

fn node_bounds_on_page(page: &Value, id: &str) -> Result<Value> {
    fn local(node: &Value, world: Affine) -> Result<Option<Rect>> {
        let transform = world * matrix(node.get("transform"))?;
        let kind = node
            .get("kind")
            .and_then(Value::as_str)
            .context("node kind is missing")?;
        let rect = match kind {
            "group" => {
                let mut bounds: Option<Rect> = None;
                for child in node
                    .get("children")
                    .and_then(Value::as_array)
                    .context("group children are missing")?
                {
                    if let Some(child) = local(child, transform)? {
                        bounds = Some(bounds.map_or(child, |current| current.union(child)))
                    }
                }
                return Ok(bounds);
            }
            "rect" => Rect::new(
                node["x"].as_f64().context("x missing")?,
                node["y"].as_f64().context("y missing")?,
                node["x"].as_f64().unwrap() + node["width"].as_f64().context("width missing")?,
                node["y"].as_f64().unwrap() + node["height"].as_f64().context("height missing")?,
            ),
            "ellipse" => {
                let (cx, cy, rx, ry) = (
                    node["cx"].as_f64().context("cx missing")?,
                    node["cy"].as_f64().context("cy missing")?,
                    node["radius_x"].as_f64().context("radius_x missing")?,
                    node["radius_y"].as_f64().context("radius_y missing")?,
                );
                Rect::new(cx - rx, cy - ry, cx + rx, cy + ry)
            }
            "line" => Rect::new(
                node["x1"]
                    .as_f64()
                    .context("x1 missing")?
                    .min(node["x2"].as_f64().context("x2 missing")?),
                node["y1"]
                    .as_f64()
                    .context("y1 missing")?
                    .min(node["y2"].as_f64().context("y2 missing")?),
                node["x1"]
                    .as_f64()
                    .unwrap()
                    .max(node["x2"].as_f64().unwrap()),
                node["y1"]
                    .as_f64()
                    .unwrap()
                    .max(node["y2"].as_f64().unwrap()),
            ),
            "path" => BezPath::from_svg(
                node.get("d")
                    .and_then(Value::as_str)
                    .context("path d missing")?,
            )?
            .bounding_box(),
            "text" => {
                let (x, y, size) = (
                    node["x"].as_f64().context("text x missing")?,
                    node["y"].as_f64().context("text y missing")?,
                    node.get("font_size")
                        .and_then(Value::as_f64)
                        .unwrap_or(16.0),
                );
                let width = node
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .chars()
                    .count() as f64
                    * size
                    * 0.6;
                Rect::new(x, y - size, x + width, y)
            }
            "instance" => {
                return local(
                    node.get("fallback")
                        .context("instance fallback is missing")?,
                    transform,
                )
            }
            _ => return Ok(None),
        };
        Ok(Some(transform.transform_rect_bbox(rect)))
    }
    fn search(nodes: &[Value], id: &str, parent: Affine) -> Result<Option<Rect>> {
        for node in nodes {
            if node.get("id").and_then(Value::as_str) == Some(id) {
                return local(node, parent);
            }
            let world = parent * matrix(node.get("transform"))?;
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                if let Some(found) = search(children, id, world)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }
    for layer in page
        .get("layers")
        .and_then(Value::as_array)
        .context("page layers missing")?
    {
        if let Some(rect) = search(
            layer
                .get("nodes")
                .and_then(Value::as_array)
                .context("layer nodes missing")?,
            id,
            Affine::IDENTITY,
        )? {
            return Ok(
                json!({"id":id,"x":rect.x0,"y":rect.y0,"width":rect.width(),"height":rect.height(),"right":rect.x1,"bottom":rect.y1}),
            );
        }
    }
    bail!("node not found: {id}")
}

pub fn translate_node(
    raw: &mut Value,
    page_id: Option<&str>,
    id: &str,
    dx: f64,
    dy: f64,
) -> Result<()> {
    let page = page_mut(raw, page_id)?;
    let node = find_node_mut(page, id).with_context(|| format!("node not found: {id}"))?;
    node["transform"] =
        json!((Affine::translate((dx, dy)) * matrix(node.get("transform"))?).as_coeffs());
    Ok(())
}

pub fn promote_component(
    raw: &mut Value,
    page_id: Option<&str>,
    group_id: &str,
    component_id: &str,
) -> Result<Value> {
    validate(raw)?;
    if component_id.is_empty() {
        bail!("component ID must be nonempty")
    }
    let pages = raw.get("pages").and_then(Value::as_array).unwrap();
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("page not found: {id}"))?,
        None => pages.first().unwrap(),
    };
    let group = find_node(page, group_id).context("group not found")?;
    require_group(group)?;
    let snapshot = group.clone();
    let identity = crate::transaction::revision(&serde_json::to_vec(&snapshot)?);
    let components = raw
        .as_object_mut()
        .unwrap()
        .entry("components")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("components must be an array")?;
    if components
        .iter()
        .any(|item| item.get("id").and_then(Value::as_str) == Some(component_id))
    {
        bail!("component already exists: {component_id}")
    }
    components.push(json!({"id":component_id,"source_identity":identity,"snapshot":snapshot}));
    validate(raw)?;
    Ok(json!({"component":component_id,"source_group":group_id,"source_identity":identity}))
}

pub fn instantiate_component(
    raw: &mut Value,
    page_id: Option<&str>,
    layer: &str,
    component_id: &str,
    id: &str,
    dx: f64,
    dy: f64,
) -> Result<Value> {
    validate(raw)?;
    let component = raw
        .get("components")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(component_id))
        })
        .with_context(|| format!("component not found: {component_id}"))?;
    let identity = component
        .get("source_identity")
        .cloned()
        .unwrap_or(Value::Null);
    let mut fallback = component
        .get("snapshot")
        .cloned()
        .context("component snapshot is missing")?;
    rename_tree(&mut fallback, &format!("{id}-fallback"), true);
    let page = page_mut(raw, page_id)?;
    if pages_node_ids(page).contains(id) {
        bail!("node ID already exists: {id}")
    }
    layer_nodes_mut(page,layer)?.push(json!({"kind":"instance","id":id,"component":component_id,"source_identity":identity,"fallback":fallback,"overrides":{},"transform":[1,0,0,1,dx,dy]}));
    validate(raw)?;
    Ok(json!({"instance":id,"component":component_id,"dx":dx,"dy":dy}))
}

pub fn reparent_node(
    raw: &mut Value,
    page_id: Option<&str>,
    id: &str,
    target_group: Option<&str>,
    target_layer: Option<&str>,
) -> Result<Value> {
    validate(raw)?;
    if target_group.is_some() == target_layer.is_some() {
        bail!("choose exactly one target_group or target_layer")
    }
    let page = page_mut(raw, page_id)?;
    let node = take_node(page, id)?;
    if let Some(group) = target_group {
        if group == id
            || node
                .get("children")
                .and_then(Value::as_array)
                .is_some_and(|children| find_node_in(children, group).is_some())
        {
            bail!("reparenting would create a cycle")
        }
        let target =
            find_node_mut(page, group).with_context(|| format!("group not found: {group}"))?;
        require_group(target)?;
        target
            .get_mut("children")
            .and_then(Value::as_array_mut)
            .unwrap()
            .push(node);
        validate(raw)?;
        Ok(json!({"id":id,"group":group}))
    } else {
        let layer = target_layer.unwrap();
        layer_nodes_mut(page, layer)?.push(node);
        validate(raw)?;
        Ok(json!({"id":id,"layer":layer}))
    }
}

fn require_group(node: &Value) -> Result<()> {
    if node.get("kind").and_then(Value::as_str) != Some("group") {
        bail!("node is not a group")
    }
    Ok(())
}

fn missing_id(page: &Value, wanted: &str, kind: &str) -> String {
    let nearest = pages_node_ids(page)
        .into_iter()
        .min_by_key(|candidate| edit_distance(candidate, wanted));
    nearest.map_or_else(
        || format!("{kind} not found: {wanted}"),
        |id| format!("{kind} not found: {wanted}; nearest ID: {id}"),
    )
}
fn edit_distance(a: &str, b: &str) -> usize {
    let mut row: Vec<usize> = (0..=b.chars().count()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut next = vec![i + 1];
        for (j, cb) in b.chars().enumerate() {
            next.push(
                (next[j] + 1)
                    .min(row[j + 1] + 1)
                    .min(row[j] + usize::from(ca != cb)),
            )
        }
        row = next
    }
    row[b.chars().count()]
}

fn reorder_node(page: &mut Value, id: &str, requested: usize) -> Result<usize> {
    fn apply(nodes: &mut Vec<Value>, id: &str, requested: usize) -> Option<usize> {
        if let Some(from) = nodes
            .iter()
            .position(|node| node.get("id").and_then(Value::as_str) == Some(id))
        {
            let node = nodes.remove(from);
            let to = requested.min(nodes.len());
            nodes.insert(to, node);
            return Some(to);
        }
        for node in nodes {
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                if let Some(index) = apply(children, id, requested) {
                    return Some(index);
                }
            }
        }
        None
    }
    for layer in page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers are missing")?
    {
        if let Some(index) = apply(
            layer
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .context("layer nodes are missing")?,
            id,
            requested,
        ) {
            return Ok(index);
        }
    }
    bail!("group not found")
}

fn page_mut<'a>(raw: &'a mut Value, id: Option<&str>) -> Result<&'a mut Value> {
    let pages = raw.get_mut("pages").and_then(Value::as_array_mut).unwrap();
    match id {
        Some(id) => pages
            .iter_mut()
            .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("page not found: {id}")),
        None => Ok(pages.first_mut().unwrap()),
    }
}
fn layer_nodes_mut<'a>(page: &'a mut Value, id: &str) -> Result<&'a mut Vec<Value>> {
    page.get_mut("layers")
        .and_then(Value::as_array_mut)
        .unwrap()
        .iter_mut()
        .find(|l| l.get("id").and_then(Value::as_str) == Some(id))
        .with_context(|| format!("layer not found: {id}"))?
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .context("layer nodes are missing")
}
fn find_node<'a>(page: &'a Value, id: &str) -> Option<&'a Value> {
    fn find<'a>(nodes: &'a [Value], id: &str) -> Option<&'a Value> {
        for node in nodes {
            if node.get("id").and_then(Value::as_str) == Some(id) {
                return Some(node);
            }
            if let Some(found) = node
                .get("children")
                .and_then(Value::as_array)
                .and_then(|c| find(c, id))
            {
                return Some(found);
            }
        }
        None
    }
    page.get("layers")?
        .as_array()?
        .iter()
        .find_map(|l| find(l.get("nodes")?.as_array()?, id))
}
fn find_node_mut<'a>(page: &'a mut Value, id: &str) -> Option<&'a mut Value> {
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
    page.get_mut("layers")?
        .as_array_mut()?
        .iter_mut()
        .find_map(|l| find(l.get_mut("nodes")?.as_array_mut()?, id))
}
fn rename_tree(node: &mut Value, new_root: &str, root: bool) {
    let old = node
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("node")
        .to_owned();
    node["id"] = json!(if root {
        new_root.to_owned()
    } else {
        format!("{new_root}-{old}")
    });
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            rename_tree(child, new_root, false)
        }
    }
}
fn insert_after(page: &mut Value, id: &str, node: Value) -> Result<()> {
    fn insert(nodes: &mut Vec<Value>, id: &str, node: &mut Option<Value>) -> bool {
        if let Some(index) = nodes
            .iter()
            .position(|n| n.get("id").and_then(Value::as_str) == Some(id))
        {
            nodes.insert(index + 1, node.take().unwrap());
            return true;
        }
        for current in nodes {
            if let Some(children) = current.get_mut("children").and_then(Value::as_array_mut) {
                if insert(children, id, node) {
                    return true;
                }
            }
        }
        false
    }
    let mut node = Some(node);
    for layer in page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .unwrap()
    {
        if insert(
            layer
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .unwrap(),
            id,
            &mut node,
        ) {
            return Ok(());
        }
    }
    bail!("group not found")
}
fn ungroup(page: &mut Value, id: &str) -> Result<usize> {
    fn apply(nodes: &mut Vec<Value>, id: &str) -> Option<usize> {
        if let Some(index) = nodes
            .iter()
            .position(|n| n.get("id").and_then(Value::as_str) == Some(id))
        {
            let mut group = nodes.remove(index);
            let transform = group
                .get("transform")
                .cloned()
                .unwrap_or_else(|| json!([1, 0, 0, 1, 0, 0]));
            let mut children = group
                .get_mut("children")?
                .as_array_mut()
                .map(std::mem::take)?;
            let parent = matrix(Some(&transform)).ok()?;
            for child in &mut children {
                let combined = parent * matrix(child.get("transform")).ok()?;
                child["transform"] = json!(combined.as_coeffs())
            }
            let count = children.len();
            nodes.splice(index..index, children);
            return Some(count);
        }
        for node in nodes {
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                if let Some(count) = apply(children, id) {
                    return Some(count);
                }
            }
        }
        None
    }
    for layer in page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .unwrap()
    {
        if let Some(count) = apply(
            layer
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .unwrap(),
            id,
        ) {
            return Ok(count);
        }
    }
    bail!("group not found")
}

#[derive(Debug, Default)]
pub struct ShapeInput {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub radius: Option<f64>,
    pub cx: Option<f64>,
    pub cy: Option<f64>,
    pub radius_x: Option<f64>,
    pub radius_y: Option<f64>,
    pub x1: Option<f64>,
    pub y1: Option<f64>,
    pub x2: Option<f64>,
    pub y2: Option<f64>,
    pub fill: Option<String>,
    pub stroke: Option<String>,
    pub stroke_width: Option<f64>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum TextAnchor {
    Baseline,
    Top,
    Center,
    Bottom,
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum TextOverflow {
    Visible,
    Clip,
    Ellipsis,
}

#[derive(Debug)]
pub struct TextBoxInput {
    pub content: String,
    pub x: f64,
    pub y: f64,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub font: String,
    pub size: f64,
    pub weight: u16,
    pub fill: String,
    pub align: String,
    pub vertical_align: String,
    pub line_height: f64,
    pub anchor: TextAnchor,
    pub overflow: TextOverflow,
}

pub fn put_text_box(
    raw: &mut Value,
    page: Option<&str>,
    layer_id: &str,
    id: &str,
    input: TextBoxInput,
) -> Result<Value> {
    validate(raw)?;
    if input.size <= 0.0
        || !input.size.is_finite()
        || input.width.is_some_and(|v| v <= 0.0 || !v.is_finite())
        || input.height.is_some_and(|v| v <= 0.0 || !v.is_finite())
    {
        bail!("text box dimensions and font size must be finite and positive")
    }
    let max_chars = input
        .width
        .map(|width| (width / (input.size * 0.6)).floor().max(1.0) as usize);
    let mut lines = Vec::new();
    for paragraph in input.content.lines() {
        if let Some(limit) = max_chars {
            let mut line = String::new();
            for word in paragraph.split_whitespace() {
                if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > limit {
                    lines.push(std::mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ')
                }
                line.push_str(word)
            }
            lines.push(line)
        } else {
            lines.push(paragraph.into())
        }
    }
    let line_px = input.size * input.line_height;
    let max_lines = input
        .height
        .map(|height| (height / line_px).floor().max(1.0) as usize)
        .unwrap_or(usize::MAX);
    let overflowed = lines.len() > max_lines;
    if overflowed && !matches!(input.overflow, TextOverflow::Visible) {
        lines.truncate(max_lines);
        if matches!(input.overflow, TextOverflow::Ellipsis) {
            if let Some(last) = lines.last_mut() {
                last.push('…')
            }
        }
    }
    let y = match input.anchor {
        TextAnchor::Baseline => input.y,
        TextAnchor::Top => input.y + input.size,
        TextAnchor::Center => input.y - (lines.len() as f64 * line_px) / 2.0 + input.size,
        TextAnchor::Bottom => input.y - lines.len().saturating_sub(1) as f64 * line_px,
    };
    let page_value = page_mut(raw, page)?;
    if pages_node_ids(page_value).contains(id) {
        bail!("node ID already exists on page: {id}")
    }
    let node = json!({"kind":"text","id":id,"content":lines.join("\n"),"source_content":input.content,"x":input.x,"y":y,"width":input.width,"height":input.height,"font_family":input.font,"font_size":input.size,"font_weight":input.weight,"italic":false,"align":input.align,"vertical_align":input.vertical_align,"line_height":input.line_height,"anchor":format!("{:?}",input.anchor).to_lowercase(),"overflow":format!("{:?}",input.overflow).to_lowercase(),"style":{"fill":{"fallback":input.fill}}});
    layer_nodes_mut(page_value, layer_id)?.push(node);
    validate(raw)?;
    Ok(json!({"id":id,"lines":lines.len(),"overflowed":overflowed}))
}

pub fn put_shape(
    raw: &mut Value,
    page: Option<&str>,
    layer_id: &str,
    kind: ShapeKind,
    id: &str,
    input: ShapeInput,
) -> Result<()> {
    validate(raw)?;
    if id.is_empty() {
        bail!("shape ID must be nonempty")
    }
    let pages = raw.get_mut("pages").and_then(Value::as_array_mut).unwrap();
    let page = match page {
        Some(id) => pages
            .iter_mut()
            .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("page not found: {id}"))?,
        None => pages.first_mut().unwrap(),
    };
    if pages_node_ids(page).contains(id) {
        bail!("node ID already exists on page: {id}")
    }
    let layer = page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .unwrap()
        .iter_mut()
        .find(|l| l.get("id").and_then(Value::as_str) == Some(layer_id))
        .with_context(|| format!("layer not found: {layer_id}"))?;
    let nodes = layer
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .unwrap();
    let style = json!({"fill":{"fallback":input.fill.unwrap_or_else(||"none".into())},"stroke":{"fallback":input.stroke.unwrap_or_else(||"none".into())},"stroke_width":{"fallback":input.stroke_width.unwrap_or(0.0)}});
    let required = |v: Option<f64>, name: &str| {
        let value = v.with_context(|| format!("--{name} is required for this shape"))?;
        if !value.is_finite() {
            bail!("--{name} must be finite")
        }
        Ok(value)
    };
    let nonnegative = |v, name| {
        let value = required(v, name)?;
        if value < 0.0 {
            bail!("--{name} must be nonnegative")
        }
        Ok(value)
    };
    let positive = |v, name| {
        let value = required(v, name)?;
        if value <= 0.0 {
            bail!("--{name} must be positive")
        }
        Ok(value)
    };
    let node = match kind {
        ShapeKind::Rect | ShapeKind::Rrect => {
            let radius = if matches!(kind, ShapeKind::Rrect) {
                nonnegative(input.radius, "radius")?
            } else {
                0.0
            };
            json!({"kind":"rect","id":id,"x":required(input.x,"x")?,"y":required(input.y,"y")?,"width":nonnegative(input.width,"width")?,"height":nonnegative(input.height,"height")?,"radius_x":radius,"radius_y":radius,"style":style})
        }
        ShapeKind::Ellipse => {
            json!({"kind":"ellipse","id":id,"cx":required(input.cx,"cx")?,"cy":required(input.cy,"cy")?,"radius_x":positive(input.radius_x,"radius-x")?,"radius_y":positive(input.radius_y,"radius-y")?,"style":style})
        }
        ShapeKind::Circle => {
            let r = positive(input.radius, "radius")?;
            json!({"kind":"ellipse","id":id,"cx":required(input.cx,"cx")?,"cy":required(input.cy,"cy")?,"radius_x":r,"radius_y":r,"style":style})
        }
        ShapeKind::Line => {
            json!({"kind":"line","id":id,"x1":required(input.x1,"x1")?,"y1":required(input.y1,"y1")?,"x2":required(input.x2,"x2")?,"y2":required(input.y2,"y2")?,"style":style})
        }
    };
    nodes.push(node);
    validate(raw)
}

pub fn edit_shape(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    updates: &ShapeInput,
) -> Result<Value> {
    validate(raw)?;
    let node = find_node_mut(page_mut(raw, page)?, id)
        .with_context(|| format!("shape not found: {id}"))?;
    let kind = node
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    {
        let mut set = |key: &str, value: Option<f64>| -> Result<()> {
            if let Some(value) = value {
                if !value.is_finite() {
                    bail!("{key} must be finite")
                }
                node[key] = json!(value)
            }
            Ok(())
        };
        match kind.as_str() {
            "rect" => {
                set("x", updates.x)?;
                set("y", updates.y)?;
                set("width", updates.width)?;
                set("height", updates.height)?;
                if let Some(radius) = updates.radius {
                    set("radius_x", Some(radius))?;
                    set("radius_y", Some(radius))?;
                }
            }
            "ellipse" => {
                set("cx", updates.cx)?;
                set("cy", updates.cy)?;
                if let Some(radius) = updates.radius {
                    set("radius_x", Some(radius))?;
                    set("radius_y", Some(radius))?;
                }
                set("radius_x", updates.radius_x)?;
                set("radius_y", updates.radius_y)?;
            }
            "line" => {
                set("x1", updates.x1)?;
                set("y1", updates.y1)?;
                set("x2", updates.x2)?;
                set("y2", updates.y2)?;
            }
            _ => bail!("node is not an editable primitive"),
        }
    }
    if let Some(fill) = &updates.fill {
        node["style"]["fill"]["fallback"] = json!(fill)
    }
    if let Some(stroke) = &updates.stroke {
        node["style"]["stroke"]["fallback"] = json!(stroke)
    }
    if let Some(width) = updates.stroke_width {
        node["style"]["stroke_width"]["fallback"] = json!(width)
    }
    validate(raw)?;
    Ok(json!({"id":id,"kind":kind}))
}

pub fn shape_to_path(raw: &mut Value, page: Option<&str>, id: &str) -> Result<Value> {
    validate(raw)?;
    let node = find_node_mut(page_mut(raw, page)?, id)
        .with_context(|| format!("shape not found: {id}"))?;
    let object = node.as_object().context("node must be an object")?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let d = match kind.as_str() {
        "rect" => rect_path(object)?,
        "ellipse" => ellipse_path(object)?,
        "line" => format!(
            "M {} {} L {} {}",
            number(object, "x1")?,
            number(object, "y1")?,
            number(object, "x2")?,
            number(object, "y2")?
        ),
        _ => bail!("only rect, ellipse, and line primitives can be converted"),
    };
    let mut replacement = Map::new();
    for key in ["id", "style", "transform", "visible", "locked"] {
        if let Some(value) = object.get(key) {
            replacement.insert(key.into(), value.clone());
        }
    }
    replacement.insert("kind".into(), json!("path"));
    replacement.insert("d".into(), json!(d));
    replacement.insert("closed".into(), json!(kind != "line"));
    *node = Value::Object(replacement);
    validate(raw)?;
    Ok(json!({"id":id,"converted_from":kind}))
}

/// Apply high-level v4 operations atomically to an in-memory scene.
pub fn apply_batch(
    raw: &mut Value,
    page: Option<&str>,
    operations: &[Value],
) -> Result<Vec<Value>> {
    if operations.is_empty() || operations.len() > 10_000 {
        bail!("batch must contain 1 to 10,000 operations")
    }
    validate(raw)?;
    let mut candidate = raw.clone();
    let mut aliases = HashMap::<String, String>::new();
    let mut changes = Vec::new();
    for (index, operation) in operations.iter().enumerate() {
        let result = (|| -> Result<Value> {
            let kind = operation
                .get("type")
                .and_then(Value::as_str)
                .context("operation type is missing")?;
            let resolve = |key: &str| -> Result<String> {
                let value = operation
                    .get(key)
                    .and_then(Value::as_str)
                    .with_context(|| format!("{key} is missing"))?;
                Ok(value
                    .strip_prefix('$')
                    .and_then(|a| aliases.get(a))
                    .cloned()
                    .unwrap_or_else(|| value.into()))
            };
            let result = match kind {
                "put-shape" => {
                    let id = resolve("id")?;
                    let layer = resolve("layer")?;
                    let shape = match operation
                        .get("shape")
                        .and_then(Value::as_str)
                        .context("shape is missing")?
                    {
                        "rect" => ShapeKind::Rect,
                        "rrect" => ShapeKind::Rrect,
                        "ellipse" => ShapeKind::Ellipse,
                        "circle" => ShapeKind::Circle,
                        "line" => ShapeKind::Line,
                        other => bail!("unsupported shape: {other}"),
                    };
                    let n = |key| operation.get(key).and_then(Value::as_f64);
                    put_shape(
                        &mut candidate,
                        page,
                        &layer,
                        shape,
                        &id,
                        ShapeInput {
                            x: n("x"),
                            y: n("y"),
                            width: n("width").or_else(|| n("w")),
                            height: n("height").or_else(|| n("h")),
                            radius: n("radius").or_else(|| n("r")),
                            cx: n("cx"),
                            cy: n("cy"),
                            radius_x: n("radius_x"),
                            radius_y: n("radius_y"),
                            x1: n("x1"),
                            y1: n("y1"),
                            x2: n("x2"),
                            y2: n("y2"),
                            fill: operation
                                .get("fill")
                                .and_then(Value::as_str)
                                .map(Into::into),
                            stroke: operation
                                .get("stroke")
                                .and_then(Value::as_str)
                                .map(Into::into),
                            stroke_width: n("stroke_width"),
                        },
                    )?;
                    if let Some(reference)=operation.get("fill_ref").and_then(Value::as_str){let page_value=page_mut(&mut candidate,page)?;let node=find_node_mut(page_value,&id).unwrap();node["style"]["fill"]["ref"]=json!(reference);}
                    json!({"type":kind,"id":id})
                }
                "put-path" | "put-text" => {
                    let id = resolve("id")?;
                    let layer = resolve("layer")?;
                    let page_value = page_mut(&mut candidate, page)?;
                    if pages_node_ids(page_value).contains(id.as_str()) {
                        bail!("node ID already exists: {id}")
                    }
                    let mut node = operation.clone();
                    let object = node.as_object_mut().unwrap();
                    object.remove("type");
                    object.remove("alias");
                    object.remove("layer");
                    object.insert("id".into(), json!(id));
                    object.insert(
                        "kind".into(),
                        json!(if kind == "put-path" { "path" } else { "text" }),
                    );
                    layer_nodes_mut(page_value, &layer)?.push(node);
                    json!({"type":kind,"id":id})
                }
                "create-group" => {
                    let id = resolve("id")?;
                    let layer = resolve("layer")?;
                    let children = operation
                        .get("children")
                        .and_then(Value::as_array)
                        .context("children are missing")?
                        .iter()
                        .map(|v| {
                            let raw = v.as_str().context("child ID must be a string")?;
                            Ok(raw
                                .strip_prefix('$')
                                .and_then(|a| aliases.get(a))
                                .cloned()
                                .unwrap_or_else(|| raw.into()))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    apply_group(
                        &mut candidate,
                        page,
                        GroupAction::Create {
                            id: id.clone(),
                            layer,
                            children,
                        },
                    )?;
                    json!({"type":kind,"id":id})
                }
                "set-style" => {
                    let name = resolve("name")?;
                    crate::style::apply(
                        &mut candidate,
                        crate::style::Operation::Set,
                        Some(&name),
                        operation
                            .get("style_type")
                            .or_else(|| operation.get("token_type"))
                            .and_then(Value::as_str),
                        operation.get("value").and_then(Value::as_str),
                        None,
                    )?
                }
                "reparent" => {
                    let id = resolve("id")?;
                    reparent_node(
                        &mut candidate,
                        page,
                        &id,
                        operation.get("target_group").and_then(Value::as_str),
                        operation.get("target_layer").and_then(Value::as_str),
                    )?
                }
                "promote-component"=>{let id=resolve("id")?;let component=resolve("component")?;promote_component(&mut candidate,page,&id,&component)?}
                "instantiate-component"=>{let id=resolve("id")?;let component=resolve("component")?;let layer=resolve("layer")?;instantiate_component(&mut candidate,page,&layer,&component,&id,operation.get("dx").and_then(Value::as_f64).unwrap_or(0.0),operation.get("dy").and_then(Value::as_f64).unwrap_or(0.0))?}
                "add-page"=>{let id=resolve("id")?;let name=operation.get("name").and_then(Value::as_str).unwrap_or(&id);let width=operation.get("width").and_then(Value::as_u64).unwrap_or(1200);let height=operation.get("height").and_then(Value::as_u64).unwrap_or(800);let layer=operation.get("layer").and_then(Value::as_str).unwrap_or("content");let pages=candidate.get_mut("pages").and_then(Value::as_array_mut).unwrap();if pages.iter().any(|page|page.get("id").and_then(Value::as_str)==Some(&id)){bail!("page already exists: {id}")}pages.push(json!({"id":id,"name":name,"canvas":{"width":width,"height":height,"background":operation.get("background").and_then(Value::as_str).unwrap_or("#ffffff")},"layers":[{"id":layer,"name":"Content","visible":true,"locked":false,"nodes":[]}]}));json!({"type":kind,"id":id})}
                other => bail!("unsupported v4 batch operation: {other}"),
            };
            if let Some(alias) = operation.get("alias").and_then(Value::as_str) {
                let id = operation
                    .get("id")
                    .and_then(Value::as_str)
                    .context("aliased operation requires id")?;
                aliases.insert(alias.into(), id.into());
            }
            Ok(result)
        })()
        .with_context(|| {
            format!(
                "operation {index} failed (page {}, layer {}, group {}, object {})",
                page.unwrap_or("default"),
                operation
                    .get("layer")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
                operation
                    .get("group")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
                operation.get("id").and_then(Value::as_str).unwrap_or("-")
            )
        })?;
        changes.push(result);
    }
    validate(&candidate)?;
    *raw = candidate;
    Ok(changes)
}

fn pages_node_ids(page: &Value) -> HashSet<&str> {
    fn visit<'a>(node: &'a Value, out: &mut HashSet<&'a str>) {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            out.insert(id);
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                visit(child, out)
            }
        }
    }
    let mut out = HashSet::new();
    if let Some(layers) = page.get("layers").and_then(Value::as_array) {
        for layer in layers {
            if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
                for node in nodes {
                    visit(node, &mut out)
                }
            }
        }
    }
    out
}

pub fn migrate_to_v4(mut raw: Value) -> Result<Value> {
    let version = raw
        .get("version")
        .and_then(Value::as_u64)
        .context("document version is missing")?;
    if version == VERSION {
        validate(&raw)?;
        return Ok(raw);
    }
    if !(1..=3).contains(&version) {
        bail!("only .pen versions 1–4 are supported");
    }
    let object = raw
        .as_object_mut()
        .context("document must be a JSON object")?;
    let pages = if version == 3 {
        object
            .get_mut("pages")
            .and_then(Value::as_array_mut)
            .context("v3 document has no pages")?
    } else {
        let canvas = object
            .remove("canvas")
            .context("legacy document has no canvas")?;
        let layers = object
            .remove("layers")
            .context("legacy document has no layers")?;
        object.insert(
            "pages".into(),
            json!([{"id":"page-1","name":"Page 1","canvas":canvas,"layers":layers}]),
        );
        object
            .get_mut("pages")
            .and_then(Value::as_array_mut)
            .unwrap()
    };
    for page in pages {
        for layer in page
            .get_mut("layers")
            .and_then(Value::as_array_mut)
            .context("page has no layers")?
        {
            let layer = layer.as_object_mut().context("layer must be an object")?;
            let paths = layer.remove("paths").unwrap_or_else(|| json!([]));
            let texts = layer.remove("texts").unwrap_or_else(|| json!([]));
            let mut nodes = Vec::new();
            for value in paths.as_array().context("layer paths must be an array")? {
                nodes.push(path_node(value.clone())?);
            }
            for value in texts.as_array().context("layer texts must be an array")? {
                nodes.push(text_node(value.clone())?);
            }
            layer.insert("nodes".into(), Value::Array(nodes));
        }
    }
    object.insert("version".into(), Value::from(VERSION));
    object.entry("styles").or_insert_with(|| json!({}));
    object.entry("components").or_insert_with(|| json!([]));
    validate(&raw)?;
    Ok(raw)
}

fn fallback(value: Value) -> Value {
    json!({"fallback":value})
}
fn path_node(value: Value) -> Result<Value> {
    let mut o = value.as_object().context("path must be an object")?.clone();
    o.insert("kind".into(), json!("path"));
    let mut style = Map::new();
    for key in [
        "fill",
        "stroke",
        "stroke_width",
        "stroke_linecap",
        "stroke_linejoin",
        "stroke_miterlimit",
    ] {
        if let Some(v) = o.remove(key) {
            style.insert(key.into(), fallback(v));
        }
    }
    o.insert("style".into(), Value::Object(style));
    o.entry("transform")
        .or_insert_with(|| json!([1, 0, 0, 1, 0, 0]));
    Ok(Value::Object(o))
}
fn text_node(value: Value) -> Result<Value> {
    let mut o = value.as_object().context("text must be an object")?.clone();
    o.insert("kind".into(), json!("text"));
    let mut style = Map::new();
    if let Some(v) = o.remove("fill") {
        style.insert("fill".into(), fallback(v));
    }
    o.insert("style".into(), Value::Object(style));
    Ok(Value::Object(o))
}

pub fn validate(raw: &Value) -> Result<()> {
    if raw.get("format").and_then(Value::as_str) != Some("pentool")
        || raw.get("version").and_then(Value::as_u64) != Some(VERSION)
    {
        bail!("not a Pentool v4 document")
    }
    if let Some(styles) = raw.get("styles") {
        crate::style::validate_aliases(styles)?;
    }
    let mut component_ids = HashSet::new();
    if let Some(components) = raw.get("components").and_then(Value::as_array) {
        for component in components {
            let id = component
                .get("id")
                .and_then(Value::as_str)
                .context("component ID is missing")?;
            if id.is_empty() || !component_ids.insert(id) {
                bail!("component IDs must be unique and nonempty")
            }
        }
    }
    let pages = raw
        .get("pages")
        .and_then(Value::as_array)
        .context("v4 document has no pages")?;
    if pages.is_empty() || pages.len() > 1000 {
        bail!("document must contain 1–1000 pages")
    }
    let mut page_ids = HashSet::new();
    for page in pages {
        let id = page
            .get("id")
            .and_then(Value::as_str)
            .context("page ID is missing")?;
        if id.is_empty() || !page_ids.insert(id) {
            bail!("page IDs must be unique and nonempty")
        };
        let layers = page
            .get("layers")
            .and_then(Value::as_array)
            .context("page layers are missing")?;
        if layers.is_empty() || layers.len() > 1000 {
            bail!("page must contain 1–1000 layers")
        };
        let mut ids = HashSet::new();
        for layer in layers {
            for node in layer
                .get("nodes")
                .and_then(Value::as_array)
                .context("v4 layer has no nodes")?
            {
                validate_node(node, 0, &mut ids)?;
            }
        }
        fn check_instances(node: &Value, components: &HashSet<&str>) -> Result<()> {
            if node.get("kind").and_then(Value::as_str) == Some("instance") {
                let component = node
                    .get("component")
                    .and_then(Value::as_str)
                    .context("instance component is missing")?;
                if !components.contains(component) {
                    bail!("instance references missing component: {component}")
                }
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                for child in children {
                    check_instances(child, components)?;
                }
            }
            Ok(())
        }
        for layer in layers {
            for node in layer.get("nodes").and_then(Value::as_array).unwrap() {
                check_instances(node, &component_ids)?;
            }
        }
    }
    Ok(())
}

pub fn flatten_to_v3(raw: &Value) -> Result<Value> {
    validate(raw)?;
    let mut output = raw.clone();
    let styles = output.get("styles").cloned().unwrap_or_else(|| json!({}));
    resolve_style_fallbacks(&mut output, &styles)?;
    let object = output.as_object_mut().unwrap();
    object.insert("version".into(), json!(3));
    object.remove("styles");
    object.remove("components");
    for page in object["pages"].as_array_mut().unwrap() {
        for layer in page["layers"].as_array_mut().unwrap() {
            let layer_object = layer.as_object_mut().unwrap();
            let nodes = layer_object.remove("nodes").unwrap_or_else(|| json!([]));
            let mut flattened = Vec::new();
            for node in nodes.as_array().unwrap() {
                flatten_node(node, Affine::IDENTITY, &mut flattened)?;
            }
            // v3 stores paths and text in separate stacks. Keep each stack stable;
            // the v4 renderer uses this same deterministic paths-then-text lowering.
            layer_object.insert(
                "paths".into(),
                Value::Array(
                    flattened
                        .iter()
                        .filter_map(|item| match item {
                            Flat::Path(v) => Some(v.clone()),
                            _ => None,
                        })
                        .collect(),
                ),
            );
            layer_object.insert(
                "texts".into(),
                Value::Array(
                    flattened
                        .into_iter()
                        .filter_map(|item| match item {
                            Flat::Text(v) => Some(v),
                            _ => None,
                        })
                        .collect(),
                ),
            );
        }
    }
    // A flattened file contains only v3 fields at the scene-bearing levels.
    Ok(output)
}

fn resolve_style_fallbacks(value: &mut Value, styles: &Value) -> Result<()> {
    fn token(styles: &Value, name: &str, stack: &mut Vec<String>) -> Result<Value> {
        if stack.iter().any(|item| item == name) {
            bail!("style alias cycle: {} -> {name}", stack.join(" -> "))
        }
        let value = styles
            .get(name)
            .with_context(|| format!("missing style token: {name}"))?
            .get("value")
            .cloned()
            .context("style token value is missing")?;
        if let Some(next) = value.get("ref").and_then(Value::as_str) {
            stack.push(name.into());
            let resolved = token(styles, next, stack);
            stack.pop();
            resolved
        } else {
            Ok(value)
        }
    }
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("ref").and_then(Value::as_str).map(str::to_owned) {
                if let Ok(resolved) = token(styles, &reference, &mut Vec::new()) {
                    object.insert("fallback".into(), resolved);
                }
            }
            for (key, child) in object {
                if key != "styles" {
                    resolve_style_fallbacks(child, styles)?;
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                resolve_style_fallbacks(child, styles)?;
            }
        }
        _ => {}
    }
    Ok(())
}

enum Flat {
    Path(Value),
    Text(Value),
}

fn flatten_node(node: &Value, parent: Affine, out: &mut Vec<Flat>) -> Result<()> {
    let object = node.as_object().context("node must be an object")?;
    if object.contains_key("clip") {
        bail!("v3 flattening does not support clipping")
    }
    let local = matrix(object.get("transform"))?;
    let world = parent * local;
    match object.get("kind").and_then(Value::as_str).unwrap() {
        "group" => {
            for child in object["children"].as_array().unwrap() {
                flatten_node(child, world, out)?
            }
        }
        "instance" => flatten_node(
            object
                .get("fallback")
                .context("instance fallback is missing")?,
            world,
            out,
        )?,
        "text" => {
            let mut value = object.clone();
            value.remove("kind");
            value.remove("style");
            value.remove("clip");
            value.insert("fill".into(), style(object, "fill", json!("#111827")));
            value.insert("transform".into(), json!(world.as_coeffs()));
            out.push(Flat::Text(Value::Object(value)));
        }
        kind => {
            let d = match kind {
                "path" => string(object, "d")?.to_owned(),
                "line" => format!(
                    "M {} {} L {} {}",
                    number(object, "x1")?,
                    number(object, "y1")?,
                    number(object, "x2")?,
                    number(object, "y2")?
                ),
                "rect" => rect_path(object)?,
                "ellipse" => ellipse_path(object)?,
                _ => bail!("unsupported node kind: {kind}"),
            };
            let mut path = BezPath::from_svg(&d).context("invalid node geometry")?;
            path.apply_affine(world);
            let mut value = Map::new();
            value.insert("id".into(), object["id"].clone());
            value.insert("d".into(), json!(path.to_svg()));
            value.insert("stroke".into(), style(object, "stroke", json!("none")));
            value.insert(
                "stroke_width".into(),
                style(object, "stroke_width", json!(0)),
            );
            value.insert(
                "stroke_linecap".into(),
                style(object, "stroke_linecap", json!("butt")),
            );
            value.insert(
                "stroke_linejoin".into(),
                style(object, "stroke_linejoin", json!("miter")),
            );
            value.insert(
                "stroke_miterlimit".into(),
                style(object, "stroke_miterlimit", json!(4)),
            );
            value.insert("fill".into(), style(object, "fill", json!("none")));
            value.insert(
                "closed".into(),
                json!(
                    matches!(kind, "rect" | "ellipse")
                        || object
                            .get("closed")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                ),
            );
            out.push(Flat::Path(Value::Object(value)));
        }
    }
    Ok(())
}

fn matrix(value: Option<&Value>) -> Result<Affine> {
    let Some(value) = value else {
        return Ok(Affine::IDENTITY);
    };
    let a = value.as_array().context("transform must be an array")?;
    if a.len() != 6 {
        bail!("transform must contain six numbers")
    };
    let mut c = [0.0; 6];
    for (i, v) in a.iter().enumerate() {
        c[i] = v.as_f64().context("transform values must be numbers")?;
    }
    Ok(Affine::new(c))
}
fn number(o: &Map<String, Value>, key: &str) -> Result<f64> {
    o.get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("{key} must be a number"))
}
fn string<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    o.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} must be a string"))
}
fn style(o: &Map<String, Value>, key: &str, default: Value) -> Value {
    o.get("style")
        .and_then(|v| v.get(key))
        .and_then(|v| v.get("fallback"))
        .cloned()
        .unwrap_or(default)
}
fn rect_path(o: &Map<String, Value>) -> Result<String> {
    let (x, y, w, h) = (
        number(o, "x")?,
        number(o, "y")?,
        number(o, "width")?,
        number(o, "height")?,
    );
    let rx = o
        .get("radius_x")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .min(w / 2.0);
    let ry = o
        .get("radius_y")
        .and_then(Value::as_f64)
        .unwrap_or(rx)
        .min(h / 2.0);
    Ok(if rx == 0.0 || ry == 0.0 {
        format!("M{x} {y} H{} V{} H{x} Z", x + w, y + h)
    } else {
        let left = x;
        let right = x + w;
        let top = y;
        let bottom = y + h;
        let inner_left = left + rx;
        let inner_right = right - rx;
        let inner_top = top + ry;
        let inner_bottom = bottom - ry;
        format!("M{inner_left} {top} H{inner_right} A{rx} {ry} 0 0 1 {right} {inner_top} V{inner_bottom} A{rx} {ry} 0 0 1 {inner_right} {bottom} H{inner_left} A{rx} {ry} 0 0 1 {left} {inner_bottom} V{inner_top} A{rx} {ry} 0 0 1 {inner_left} {top} Z")
    })
}
fn ellipse_path(o: &Map<String, Value>) -> Result<String> {
    let (cx, cy, rx, ry) = (
        number(o, "cx")?,
        number(o, "cy")?,
        number(o, "radius_x")?,
        number(o, "radius_y")?,
    );
    Ok(format!(
        "M{} {cy} A{rx} {ry} 0 1 0 {} {cy} A{rx} {ry} 0 1 0 {} {cy} Z",
        cx - rx,
        cx + rx,
        cx - rx
    ))
}
fn validate_node(node: &Value, depth: usize, ids: &mut HashSet<String>) -> Result<()> {
    if depth > MAX_DEPTH {
        bail!("scene graph exceeds maximum nesting depth {MAX_DEPTH}")
    };
    let o = node.as_object().context("node must be an object")?;
    let id = o
        .get("id")
        .and_then(Value::as_str)
        .context("node ID is missing")?;
    if id.is_empty() || !ids.insert(id.into()) {
        bail!("node IDs must be unique and nonempty within a page: {id}")
    };
    let kind = o
        .get("kind")
        .and_then(Value::as_str)
        .context("node kind is missing")?;
    matrix(o.get("transform"))?;
    if kind == "group" {
        for child in o
            .get("children")
            .and_then(Value::as_array)
            .context("group children are missing")?
        {
            validate_node(child, depth + 1, ids)?
        }
    } else {
        match kind {
            "rect" => {
                number(o, "x")?;
                number(o, "y")?;
                if number(o, "width")? < 0.0 || number(o, "height")? < 0.0 {
                    bail!("rectangle dimensions must be nonnegative")
                }
            }
            "ellipse" => {
                number(o, "cx")?;
                number(o, "cy")?;
                if number(o, "radius_x")? <= 0.0 || number(o, "radius_y")? <= 0.0 {
                    bail!("ellipse radii must be positive")
                }
            }
            "line" => {
                for key in ["x1", "y1", "x2", "y2"] {
                    number(o, key)?;
                }
            }
            "path" => {
                BezPath::from_svg(string(o, "d")?).context("invalid path geometry")?;
            }
            "text" => {
                string(o, "content")?;
                number(o, "x")?;
                number(o, "y")?;
                if o.get("font_size")
                    .and_then(Value::as_f64)
                    .is_some_and(|size| size <= 0.0)
                {
                    bail!("font_size must be positive")
                }
            }
            "instance" => {
                string(o, "component")?;
                let fallback = o.get("fallback").context("instance fallback is missing")?;
                let mut fallback_ids = HashSet::new();
                validate_node(fallback, depth + 1, &mut fallback_ids)?;
            }
            _ => bail!("unsupported node kind: {kind}"),
        }
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_migration_preserves_order_and_extensions() {
        let raw:Value=serde_json::from_str(r##"{"format":"pentool","version":1,"name":"x","canvas":{"width":10,"height":10,"background":"none"},"layers":[{"id":"l","name":"L","custom":7,"paths":[{"id":"p","d":"M0 0","stroke":"#000","stroke_width":1,"fill":"none","closed":false}],"texts":[]}],"extra":{"x":1}}"##).unwrap();
        let v4 = migrate_to_v4(raw).unwrap();
        assert_eq!(v4["version"], 4);
        assert_eq!(v4["pages"][0]["layers"][0]["nodes"][0]["id"], "p");
        assert_eq!(v4["pages"][0]["layers"][0]["custom"], 7);
        assert_eq!(v4["extra"]["x"], 1);
    }

    #[test]
    fn fixture_flattens_to_a_valid_renderable_v3_document() {
        let raw: Value =
            serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
        let flattened = flatten_to_v3(&raw).unwrap();
        let doc: crate::document::Document = serde_json::from_value(flattened).unwrap();
        doc.validate().unwrap();
        assert_eq!(doc.layers[0].paths.len(), 1);
        assert_eq!(doc.layers[0].texts.len(), 1);
        assert!(crate::render::to_png(&doc, 1.0).is_ok());
    }

    #[test]
    fn semantic_shapes_are_canonical_and_flattenable() {
        let mut raw: Value =
            serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
        raw["pages"][0]["layers"][0]["nodes"] = json!([]);
        put_shape(
            &mut raw,
            None,
            "content",
            ShapeKind::Circle,
            "badge",
            ShapeInput {
                cx: Some(250.0),
                cy: Some(40.0),
                radius: Some(12.0),
                fill: Some("#22d3ee".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(raw["pages"][0]["layers"][0]["nodes"][0]["kind"], "ellipse");
        assert!(raw["pages"][0]["layers"][0]["nodes"][0].get("r").is_none());
        let flattened = flatten_to_v3(&raw).unwrap();
        let doc: crate::document::Document = serde_json::from_value(flattened).unwrap();
        assert_eq!(doc.layers[0].paths.len(), 1);
    }

    #[test]
    fn groups_move_duplicate_and_ungroup_without_losing_children() {
        let mut raw: Value =
            serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
        apply_group(
            &mut raw,
            None,
            GroupAction::Move {
                id: "card".into(),
                dx: 40.0,
                dy: -8.0,
            },
        )
        .unwrap();
        assert_eq!(
            find_node(&raw["pages"][0], "card").unwrap()["transform"][4],
            60.0
        );

        apply_group(
            &mut raw,
            None,
            GroupAction::Duplicate {
                source: "card".into(),
                id: "card-2".into(),
                dx: 200.0,
                dy: 0.0,
            },
        )
        .unwrap();
        let duplicate = find_node(&raw["pages"][0], "card-2").unwrap();
        assert_eq!(duplicate["children"].as_array().unwrap().len(), 2);
        assert!(find_node(&raw["pages"][0], "card-2-card-bg").is_some());

        let result = apply_group(
            &mut raw,
            None,
            GroupAction::Ungroup {
                id: "card-2".into(),
            },
        )
        .unwrap();
        assert_eq!(result["ungrouped_children"], 2);
        assert!(find_node(&raw["pages"][0], "card-2").is_none());
        validate(&raw).unwrap();
    }
}


pub fn inspect_paginated_v4(
    raw: &Value,
    page_id: Option<&str>,
    query: Option<&str>,
    kind_filter: Option<&str>,
    layer_filter: Option<&str>,
    offset: usize,
    limit: usize,
) -> Result<Value> {
    if limit == 0 || limit > 10_000 {
        bail!("search limit must be between 1 and 10,000");
    }
    let needle = query.unwrap_or("").to_lowercase();
    let mut doc = raw.clone();
    let page = page_mut(&mut doc, page_id)?;
    
    let mut matches = 0usize;
    let mut returned = 0usize;
    let mut layers = Vec::new();

    let doc_name = raw.get("name").and_then(Value::as_str).unwrap_or("Untitled");
    let version = raw.get("version").and_then(Value::as_u64).unwrap_or(4);
    let active_page = page.get("id").and_then(Value::as_str).unwrap_or("page-1");

    let page_layers = page.get("layers").and_then(Value::as_array).cloned().unwrap_or_default();
    for (l_idx, l) in page_layers.iter().enumerate() {
        let lid = l.get("id").and_then(Value::as_str).unwrap_or("");
        let lname = l.get("name").and_then(Value::as_str).unwrap_or("");
        
        if let Some(f) = layer_filter {
            if lid != f && !lname.to_lowercase().contains(&f.to_lowercase()) {
                continue;
            }
        }

        let mut out_nodes = Vec::new();
        let nodes = l.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
        
        for node in nodes {
            if let Some(aug) = augment_node(page, &node, &needle, kind_filter, &mut matches, &mut returned, offset, limit)? {
                out_nodes.push(aug);
            }
        }
        
        let layer_match = needle.is_empty() || lid.to_lowercase().contains(&needle) || lname.to_lowercase().contains(&needle);
        
        if layer_match || !out_nodes.is_empty() {
            let mut out_layer = l.clone();
            out_layer["nodes"] = json!(out_nodes);
            layers.push(out_layer);
        }
    }

    Ok(json!({
        "document": doc_name,
        "version": version,
        "page": active_page,
        "matches": matches,
        "returned": returned,
        "offset": offset,
        "limit": limit,
        "has_more": offset.saturating_add(returned) < matches,
        "stacking": "layers, then nodes; zero is back",
        "layers": layers
    }))
}

fn augment_node(
    page: &Value,
    node: &Value,
    needle: &str,
    kind_filter: Option<&str>,
    matches: &mut usize,
    returned: &mut usize,
    offset: usize,
    limit: usize,
) -> Result<Option<Value>> {
    let mut out = node.clone();
    let id = node.get("id").and_then(Value::as_str).unwrap_or("");
    let name = node.get("name").and_then(Value::as_str).unwrap_or("");
    let kind = node.get("kind").and_then(Value::as_str).unwrap_or("");
    
    let mut is_match = needle.is_empty() || id.to_lowercase().contains(needle) || name.to_lowercase().contains(needle);
    if kind == "text" {
        if let Some(content) = node.get("content").and_then(Value::as_str) {
            if content.to_lowercase().contains(needle) {
                is_match = true;
            }
        }
    }
    
    let kind_matches = kind_filter.map_or(true, |k| k == kind);

    let mut augmented_children = Vec::new();
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            if let Some(aug) = augment_node(page, child, needle, kind_filter, matches, returned, offset, limit)? {
                augmented_children.push(aug);
            }
        }
    }
    
    if (is_match && kind_matches) || !augmented_children.is_empty() {
        if is_match && kind_matches {
            if *matches >= offset && *returned < limit {
                if let Ok(b) = node_bounds_on_page(page, id) {
                    if !b.is_null() {
                        out["bounds"] = b;
                    }
                }
                *returned += 1;
            }
            *matches += 1;
        } else {
            // Add bounds for parent groups even if they aren't the direct match
            if let Ok(b) = node_bounds_on_page(page, id) {
                if !b.is_null() {
                    out["bounds"] = b;
                }
            }
        }
        
        if !augmented_children.is_empty() {
            out["children"] = json!(augmented_children);
        }
        
        return Ok(Some(out));
    }
    
    Ok(None)
}

//! Version 4 ordered scene graph migration and validation.
use anyhow::{bail, Context, Result};
use kurbo::{Affine, BezPath, Rect, Shape};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::collections::HashSet;

pub const VERSION: u64 = 4;
pub const LATEST_VERSION: u64 = crate::composite::VERSION;
pub const MAX_DEPTH: usize = 64;

pub fn is_scene_document(raw: &Value) -> bool {
    matches!(
        raw.get("version").and_then(Value::as_u64),
        Some(VERSION) | Some(crate::image::VERSION) | Some(crate::composite::VERSION)
    )
}

pub fn new_document(width: u32, height: u32) -> Value {
    json!({
        "format": "pentool",
        "version": VERSION,
        "name": "Untitled",
        "pages": [{
            "id": "page-1",
            "name": "Page 1",
            "canvas": {"width": width, "height": height, "background": "#ffffff"},
            "layers": [{"id": "layer-1", "name": "Layer 1", "visible": true, "locked": false, "nodes": []}]
        }],
        "styles": {},
        "components": [],
        "fonts": []
    })
}

pub fn edit_canvas(
    raw: &mut Value,
    page_id: Option<&str>,
    width: Option<u32>,
    height: Option<u32>,
    background: Option<&str>,
    name: Option<&str>,
) -> Result<()> {
    validate(raw)?;
    if let Some(name) = name {
        raw["name"] = json!(name);
    }
    let canvas = page_mut(raw, page_id)?
        .get_mut("canvas")
        .and_then(Value::as_object_mut)
        .context("page has no canvas")?;
    if let Some(width) = width {
        canvas.insert("width".into(), json!(width));
    }
    if let Some(height) = height {
        canvas.insert("height".into(), json!(height));
    }
    if let Some(background) = background {
        canvas.insert("background".into(), json!(background));
    }
    validate(raw)
}

pub fn apply_path(
    raw: &mut Value,
    page_id: Option<&str>,
    action: crate::editing::PathAction,
) -> Result<()> {
    use crate::editing::PathAction;
    validate(raw)?;
    let page = page_mut(raw, page_id)?;
    match action {
        PathAction::Put {
            id,
            layer,
            mut d,
            stroke,
            width,
            fill,
            closed,
            cap,
            join,
            miter_limit,
        } => {
            if !width.is_finite() || width < 0.0 {
                bail!("width must be finite and nonnegative")
            }
            if !miter_limit.is_finite() || !(1.0..=1000.0).contains(&miter_limit) {
                bail!("miter limit must be finite and between 1 and 1000")
            }
            if closed && !d.trim_end().ends_with(['z', 'Z']) {
                d.push_str(" Z")
            }
            if d.trim().is_empty() {
                bail!("path data cannot be empty")
            }
            for segment in svgtypes::PathParser::from(d.as_str()) {
                segment.context("invalid SVG path data")?;
            }
            let replacement = json!({"kind":"path","id":id,"d":d,"closed":closed,"style":{"fill":{"fallback":fill},"stroke":{"fallback":stroke},"stroke_width":{"fallback":width},"stroke_linecap":{"fallback":cap.svg()},"stroke_linejoin":{"fallback":join.svg()},"stroke_miterlimit":{"fallback":miter_limit}}});
            let nodes = layer_nodes_mut(page, &layer)?;
            if let Some(existing) = find_node_in_mut(nodes, &id) {
                *existing = replacement;
            } else {
                if pages_node_ids(page).contains(id.as_str()) {
                    bail!("node ID already exists on page: {id}")
                }
                layer_nodes_mut(page, &layer)?.push(replacement);
            }
        }
        PathAction::Style {
            id,
            layer,
            cap,
            join,
            miter_limit,
        } => {
            if let Some(value) = miter_limit {
                if !value.is_finite() || !(1.0..=1000.0).contains(&value) {
                    bail!("miter limit must be finite and between 1 and 1000")
                }
            }
            let node =
                find_node_in_mut(layer_nodes_mut(page, &layer)?, &id).context("path not found")?;
            if node.get("kind").and_then(Value::as_str) != Some("path") {
                bail!("node is not a path")
            }
            if let Some(value) = cap {
                node["style"]["stroke_linecap"]["fallback"] = json!(value.svg());
            }
            if let Some(value) = join {
                node["style"]["stroke_linejoin"]["fallback"] = json!(value.svg());
            }
            if let Some(value) = miter_limit {
                node["style"]["stroke_miterlimit"]["fallback"] = json!(value);
            }
        }
        PathAction::Remove { id, layer } => {
            let nodes = layer_nodes_mut(page, &layer)?;
            let index = nodes
                .iter()
                .position(|node| node.get("id").and_then(Value::as_str) == Some(&id))
                .context("path not found")?;
            if nodes[index].get("kind").and_then(Value::as_str) != Some("path") {
                bail!("node is not a path")
            }
            nodes.remove(index);
        }
    }
    validate(raw)
}

fn find_node_in_mut<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    for node in nodes {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            if let Some(found) = find_node_in_mut(children, id) {
                return Some(found);
            }
        }
    }
    None
}

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
    if crate::composite::is_document(raw) {
        let mut candidate = raw.clone();
        let selected = page_mut(&mut candidate, page_id)?;
        match &action {
            GroupAction::Bounds { .. } => {}
            GroupAction::Create {
                children, layer, ..
            } => {
                ensure_layer_unlocked(selected, layer)?;
                for id in children {
                    crate::composite::ensure_unlocked(selected, id)?;
                }
            }
            GroupAction::AddChild { group, child } => {
                crate::composite::ensure_unlocked(selected, group)?;
                crate::composite::ensure_unlocked(selected, child)?;
            }
            GroupAction::RemoveChild {
                group,
                child,
                layer,
            } => {
                crate::composite::ensure_unlocked(selected, group)?;
                crate::composite::ensure_unlocked(selected, child)?;
                ensure_layer_unlocked(selected, layer)?;
            }
            GroupAction::Duplicate { source, .. } => {
                crate::composite::ensure_unlocked(selected, source)?
            }
            GroupAction::Move { id, .. }
            | GroupAction::Rotate { id, .. }
            | GroupAction::Scale { id, .. }
            | GroupAction::Ungroup { id }
            | GroupAction::Rename { id, .. }
            | GroupAction::Reorder { id, .. } => crate::composite::ensure_unlocked(selected, id)?,
        }
        if let GroupAction::Ungroup { id } = &action {
            let node = find_node(selected, id).context("group missing")?;
            let styled = node["visible"] == false
                || node.get("opacity").is_some_and(|v| v.as_f64() != Some(1.0))
                || node
                    .get("content_opacity")
                    .is_some_and(|v| v.as_f64() != Some(1.0))
                || node.get("blend_mode").is_some_and(|v| v != "normal")
                || ["effects", "transforms", "mask", "clip", "clipping"]
                    .iter()
                    .any(|key| node.get(*key).is_some());
            let backdrop_sensitive = node["children"].as_array().is_some_and(|children| {
                children.iter().any(|n| {
                    n["kind"] == "adjustment"
                        || n["isolation"] == "pass-through"
                        || n.get("blend_mode").is_some_and(|v| v != "normal")
                })
            });
            if styled || backdrop_sensitive {
                bail!("[unsupported-capability] ungroup would change compositing; retain the group or explicitly remove its appearance and backdrop-sensitive children first")
            }
        }
        let result = apply_group_inner(&mut candidate, page_id, action)?;
        validate(&candidate)?;
        *raw = candidate;
        return Ok(result);
    }
    apply_group_inner(raw, page_id, action)
}

fn apply_group_inner(raw: &mut Value, page_id: Option<&str>, action: GroupAction) -> Result<Value> {
    validate(raw)?;
    let compositing = crate::composite::is_document(raw);
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
            let mut mapping = HashMap::new();
            fn collect(node: &Value, prefix: &str, root: bool, map: &mut HashMap<String, String>) {
                if let Some(old) = node["id"].as_str() {
                    map.insert(
                        old.into(),
                        if root {
                            prefix.into()
                        } else {
                            format!("{prefix}-{old}")
                        },
                    );
                }
                if let Some(children) = node["children"].as_array() {
                    for child in children {
                        collect(child, prefix, false, map);
                    }
                }
            }
            collect(&copy, &id, true, &mut mapping);
            rename_tree(&mut copy, &id, true);
            fn refs(node: &mut Value, map: &HashMap<String, String>) {
                for pointer in ["/clipping/base", "/mask/node", "/clip/node", "/scope/id"] {
                    if let Some(value) = node.pointer_mut(pointer) {
                        if let Some(new) = value.as_str().and_then(|old| map.get(old)) {
                            *value = json!(new);
                        }
                    }
                }
                if let Some(ids) = node.pointer_mut("/scope/ids").and_then(Value::as_array_mut) {
                    for id in ids {
                        if let Some(new) = id.as_str().and_then(|old| map.get(old)) {
                            *id = json!(new);
                        }
                    }
                }
                if let Some(children) = node["children"].as_array_mut() {
                    for child in children {
                        refs(child, map);
                    }
                }
            }
            if compositing {
                refs(&mut copy, &mapping);
            }
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
    if let Some(node) = find_node(page, id) {
        if node["kind"] == "adjustment" {
            // Adjustments have no geometry. Report a conservative affected region,
            // preserving the bounds field of the compact discovery API.
            let width = page["canvas"]["width"]
                .as_f64()
                .context("canvas width missing")?;
            let height = page["canvas"]["height"]
                .as_f64()
                .context("canvas height missing")?;
            return Ok(
                json!({"id":id,"x":0,"y":0,"width":width,"height":height,"right":width,"bottom":height}),
            );
        }
    }
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
            "rect" | "fill" => Rect::new(
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
                let content = node
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let width = node
                    .get("width")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| {
                        content
                            .lines()
                            .map(|line| line.chars().count())
                            .max()
                            .unwrap_or(0) as f64
                            * size
                            * 0.6
                    });
                let height = node
                    .get("height")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| {
                        content.lines().count().max(1) as f64
                            * size
                            * node
                                .get("line_height")
                                .and_then(Value::as_f64)
                                .unwrap_or(1.2)
                    });
                Rect::new(x, y - size, x + width, y - size + height)
            }
            "image" | "raster" => Rect::new(
                node["x"].as_f64().context("image x missing")?,
                node["y"].as_f64().context("image y missing")?,
                node["x"].as_f64().unwrap()
                    + node["width"].as_f64().context("image width missing")?,
                node["y"].as_f64().unwrap()
                    + node["height"].as_f64().context("image height missing")?,
            ),
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

pub(crate) fn page_mut<'a>(raw: &'a mut Value, id: Option<&str>) -> Result<&'a mut Value> {
    let pages = raw.get_mut("pages").and_then(Value::as_array_mut).unwrap();
    match id {
        Some(id) => pages
            .iter_mut()
            .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("page not found: {id}")),
        None => Ok(pages.first_mut().unwrap()),
    }
}
pub(crate) fn layer_nodes_mut<'a>(page: &'a mut Value, id: &str) -> Result<&'a mut Vec<Value>> {
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
pub(crate) fn find_node_mut<'a>(page: &'a mut Value, id: &str) -> Option<&'a mut Value> {
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

/// Remove an existing leaf node so a `put-*` operation can replace it.
/// Returns the layer and top-level index it occupied, when it was top-level.
fn take_for_replace(page: &mut Value, id: &str) -> Result<Option<(String, usize)>> {
    fn remove(nodes: &mut Vec<Value>, id: &str) -> Result<Option<usize>> {
        if let Some(index) = nodes
            .iter()
            .position(|n| n.get("id").and_then(Value::as_str) == Some(id))
        {
            if nodes[index].get("children").is_some() {
                bail!("cannot replace group {id}; remove it or edit its children")
            }
            nodes.remove(index);
            return Ok(Some(index));
        }
        for node in nodes.iter_mut() {
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                if remove(children, id)?.is_some() {
                    return Ok(Some(usize::MAX));
                }
            }
        }
        Ok(None)
    }
    let layers = page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers are missing")?;
    for layer in layers.iter_mut() {
        let layer_id = layer
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let locked = layer
            .get("locked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) else {
            continue;
        };
        if nodes_contain_id(nodes, id) {
            if locked {
                bail!("layer is locked: {layer_id}")
            }
            return Ok(match remove(nodes, id)? {
                Some(usize::MAX) | None => None,
                Some(index) => Some((layer_id, index)),
            });
        }
    }
    Ok(None)
}

fn nodes_contain_id(nodes: &[Value], id: &str) -> bool {
    nodes.iter().any(|node| {
        node.get("id").and_then(Value::as_str) == Some(id)
            || node
                .get("children")
                .and_then(Value::as_array)
                .is_some_and(|children| nodes_contain_id(children, id))
    })
}

/// Put a replaced node back at its previous top-level position.
fn restore_position(page: &mut Value, previous: Option<(String, usize)>, layer: &str, id: &str) {
    let Some((old_layer, index)) = previous else {
        return;
    };
    if old_layer != layer {
        return;
    }
    if let Ok(nodes) = layer_nodes_mut(page, layer) {
        if let Some(last) = nodes
            .iter()
            .position(|n| n.get("id").and_then(Value::as_str) == Some(id))
        {
            let node = nodes.remove(last);
            nodes.insert(index.min(nodes.len()), node);
        }
    }
}

fn is_replace(operation: &Value) -> Result<bool> {
    match operation.get("mode").and_then(Value::as_str) {
        None | Some("create") => Ok(false),
        Some("replace") => Ok(true),
        Some(other) => bail!("unsupported mode: {other} (use create or replace)"),
    }
}

/// Apply high-level v4 operations atomically to an in-memory scene.
pub fn apply_batch(
    raw: &mut Value,
    page: Option<&str>,
    operations: &[Value],
) -> Result<Vec<Value>> {
    apply_batch_at(raw, page, operations, std::path::Path::new("document.pen"))
}

/// Batch planner with the document root needed for verified linked sources.
pub fn apply_batch_at(
    raw: &mut Value,
    page: Option<&str>,
    operations: &[Value],
    document: &std::path::Path,
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
                    let previous = if is_replace(operation)? {
                        take_for_replace(page_mut(&mut candidate, page)?, &id)?
                    } else {
                        None
                    };
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
                    restore_position(page_mut(&mut candidate, page)?, previous, &layer, &id);
                    if let Some(reference) = operation.get("fill_ref").and_then(Value::as_str) {
                        let node = find_node_mut(page_mut(&mut candidate, page)?, &id).unwrap();
                        node["style"]["fill"]["ref"] = json!(reference);
                    }
                    if let Some(reference) = operation.get("stroke_ref").and_then(Value::as_str) {
                        let node = find_node_mut(page_mut(&mut candidate, page)?, &id).unwrap();
                        node["style"]["stroke"]["ref"] = json!(reference);
                    }
                    json!({"type":kind,"id":id})
                }
                "put-path" | "put-text" => {
                    let id = resolve("id")?;
                    let layer = resolve("layer")?;
                    let page_value = page_mut(&mut candidate, page)?;
                    let previous = if is_replace(operation)? {
                        take_for_replace(page_value, &id)?
                    } else {
                        None
                    };
                    if pages_node_ids(page_value).contains(id.as_str()) {
                        bail!("node ID already exists: {id} (set mode replace or pass --upsert to replace it)")
                    }
                    let mut node = operation.clone();
                    let object = node.as_object_mut().unwrap();
                    object.remove("type");
                    object.remove("mode");
                    object.remove("alias");
                    object.remove("layer");
                    object.insert("id".into(), json!(id));
                    object.insert(
                        "kind".into(),
                        json!(if kind == "put-path" { "path" } else { "text" }),
                    );
                    normalize_batch_style(object, kind)?;
                    layer_nodes_mut(page_value, &layer)?.push(node);
                    restore_position(page_value, previous, &layer, &id);
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
                "put-image" | "set-image" | "image-op-add" | "image-op-set" | "image-op-move"
                | "image-op-enable" | "image-op-disable" | "image-op-remove" => {
                    let replacing = kind == "put-image" && is_replace(operation)?;
                    let target = resolve("id").ok().filter(|_| replacing);
                    let previous = match (&target, operation.get("layer").and_then(Value::as_str)) {
                        (Some(id), Some(_)) => take_for_replace(page_mut(&mut candidate, page)?, id)?,
                        _ => None,
                    };
                    let result =
                        crate::image::batch_operation(&mut candidate, page, kind, operation, &resolve)?;
                    if let (Some(id), Some(layer)) =
                        (&target, operation.get("layer").and_then(Value::as_str))
                    {
                        let layer = resolve("layer").unwrap_or_else(|_| layer.to_owned());
                        restore_position(page_mut(&mut candidate, page)?, previous, &layer, id);
                    }
                    result
                }
                other if crate::composite::is_document(&candidate) => crate::composite::batch_operation(&mut candidate,page,document,other,operation,&resolve)?,
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
        .map_err(|error| {
            if operation
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("put-"))
                && error.to_string().contains("already exists on page") {
                anyhow::anyhow!(
                    "{error}; pass `batch --upsert` or set \"mode\":\"replace\" on the operation to replace it"
                )
            } else {
                error
            }
        })
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

fn normalize_batch_style(object: &mut Map<String, Value>, operation: &str) -> Result<()> {
    let mut style = object
        .remove("style")
        .unwrap_or_else(|| json!({}))
        .as_object()
        .cloned()
        .context("style must be an object")?;
    let keys: &[&str] = if operation == "put-path" {
        &[
            "fill",
            "stroke",
            "stroke_width",
            "stroke_linecap",
            "stroke_linejoin",
            "stroke_miterlimit",
        ]
    } else {
        &["fill"]
    };
    for key in keys {
        if let Some(value) = object.remove(*key) {
            style.insert(
                (*key).into(),
                if value.is_object() {
                    value
                } else {
                    json!({"fallback": value})
                },
            );
        }
        let reference_key = format!("{key}_ref");
        if let Some(reference) = object.remove(&reference_key) {
            if !reference.is_string() {
                bail!("{reference_key} must be a string")
            }
            let entry = style.entry(*key).or_insert_with(|| json!({"fallback": if *key == "stroke_width" { json!(0) } else { json!("none") }}));
            entry["ref"] = reference;
        }
    }
    if operation == "put-path" {
        style
            .entry("fill")
            .or_insert_with(|| json!({"fallback":"none"}));
        style
            .entry("stroke")
            .or_insert_with(|| json!({"fallback":"none"}));
        style
            .entry("stroke_width")
            .or_insert_with(|| json!({"fallback":0}));
    } else {
        style
            .entry("fill")
            .or_insert_with(|| json!({"fallback":"#111827"}));
    }
    object.insert("style".into(), Value::Object(style));
    Ok(())
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
    if crate::composite::is_document(&raw) {
        raw = crate::composite::downgrade(raw)?;
    }
    let version = raw
        .get("version")
        .and_then(Value::as_u64)
        .context("document version is missing")?;
    if version == VERSION {
        validate(&raw)?;
        return Ok(raw);
    }
    if version == crate::image::VERSION {
        validate(&raw)?;
        if raw
            .get("image_assets")
            .and_then(Value::as_object)
            .is_some_and(|assets| !assets.is_empty())
        {
            bail!("[unsupported-capability] version 4 cannot represent image assets")
        }
        let object = raw
            .as_object_mut()
            .context("document must be a JSON object")?;
        object.insert("version".into(), Value::from(VERSION));
        object.remove("image_assets");
        validate(&raw)?;
        return Ok(raw);
    }
    if !(1..=3).contains(&version) {
        bail!("only .pen versions 1–5 are supported");
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

pub fn migrate_to_v5(raw: Value) -> Result<Value> {
    if crate::composite::is_document(&raw) {
        return crate::composite::downgrade(raw);
    }
    let mut raw = match raw.get("version").and_then(Value::as_u64) {
        Some(crate::image::VERSION) => {
            validate(&raw)?;
            return Ok(raw);
        }
        Some(VERSION) => raw,
        Some(1..=3) => migrate_to_v4(raw)?,
        Some(version) => {
            bail!("[unsupported-capability] cannot migrate document version {version} to version 5")
        }
        None => bail!("[malformed-resource] document version is missing or invalid"),
    };
    let object = raw
        .as_object_mut()
        .context("document must be a JSON object")?;
    object.insert("version".into(), Value::from(crate::image::VERSION));
    object.entry("image_assets").or_insert_with(|| json!({}));
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

fn layer_mut<'a>(page: &'a mut Value, id: &str) -> Result<&'a mut Value> {
    page.get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers are missing")?
        .iter_mut()
        .find(|l| l.get("id").and_then(Value::as_str) == Some(id))
        .with_context(|| format!("layer not found: {id}"))
}

pub(crate) fn ensure_layer_unlocked(page: &mut Value, id: &str) -> Result<()> {
    if layer_mut(page, id)?
        .get("locked")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!("layer is locked: {id}")
    }
    Ok(())
}

/// `layer add|set|remove|move` for scene (v4/v5) documents.
pub fn apply_layer(
    raw: &mut Value,
    page_id: Option<&str>,
    action: crate::editing::LayerAction,
) -> Result<()> {
    use crate::editing::LayerAction;
    validate(raw)?;
    let page = page_mut(raw, page_id)?;
    let layers = page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .context("page layers are missing")?;
    let position = |layers: &[Value], id: &str| {
        layers
            .iter()
            .position(|l| l.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("layer not found: {id}"))
    };
    match action {
        LayerAction::Add { id, name } => {
            if id.trim().is_empty() {
                bail!("layer ID cannot be empty")
            }
            if layers
                .iter()
                .any(|l| l.get("id").and_then(Value::as_str) == Some(id.as_str()))
            {
                bail!("layer ID already exists: {id}")
            }
            layers.push(json!({
                "id": id,
                "name": name.unwrap_or_else(|| id.clone()),
                "visible": true,
                "locked": false,
                "nodes": []
            }));
        }
        LayerAction::Set {
            id,
            name,
            visible,
            locked,
        } => {
            let index = position(layers, &id)?;
            let layer = layers[index].as_object_mut().unwrap();
            if let Some(name) = name {
                layer.insert("name".into(), json!(name));
            }
            if let Some(visible) = visible {
                layer.insert("visible".into(), json!(visible));
            }
            if let Some(locked) = locked {
                layer.insert("locked".into(), json!(locked));
            }
        }
        LayerAction::Remove { id } => {
            let index = position(layers, &id)?;
            if layers[index]
                .get("locked")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                bail!("layer is locked: {id}")
            }
            if layers.len() == 1 {
                bail!("cannot remove the last layer")
            }
            layers.remove(index);
        }
        LayerAction::Move { id, index } => {
            if index >= layers.len() {
                bail!("layer index out of range")
            }
            let old = position(layers, &id)?;
            let layer = layers.remove(old);
            layers.insert(index, layer);
        }
    }
    validate(raw)
}

/// `text put|set|remove` for scene (v4/v5) documents.
pub fn apply_text(
    raw: &mut Value,
    page_id: Option<&str>,
    action: crate::text::TextAction,
) -> Result<()> {
    use crate::text::TextAction;
    validate(raw)?;
    let page = page_mut(raw, page_id)?;
    let layer_id = match &action {
        TextAction::Put { layer, .. }
        | TextAction::Set { layer, .. }
        | TextAction::Remove { layer, .. } => layer.clone(),
    };
    ensure_layer_unlocked(page, &layer_id)?;
    match action {
        TextAction::Put {
            id,
            content,
            x,
            y,
            font,
            size,
            weight,
            italic,
            fill,
            align,
            letter_spacing,
            line_height,
            ..
        } => {
            if !size.is_finite() || size <= 0.0 {
                bail!("font size must be finite and positive")
            }
            let align = serde_json::to_value(align)?;
            let replacement = json!({"kind":"text","id":id,"content":content,"x":x,"y":y,"font_family":font,"font_size":size,"font_weight":weight,"italic":italic,"align":align,"letter_spacing":letter_spacing,"line_height":line_height,"style":{"fill":{"fallback":fill}}});
            let nodes = layer_nodes_mut(page, &layer_id)?;
            if let Some(existing) = find_node_in_mut(nodes, &id) {
                if existing.get("kind").and_then(Value::as_str) != Some("text") {
                    bail!("object ID belongs to a non-text node: {id}")
                }
                *existing = replacement;
            } else {
                if pages_node_ids(page).contains(id.as_str()) {
                    bail!("node ID already exists on page: {id}")
                }
                layer_nodes_mut(page, &layer_id)?.push(replacement);
            }
        }
        TextAction::Set {
            id,
            content,
            x,
            y,
            font,
            size,
            weight,
            italic,
            fill,
            align,
            letter_spacing,
            line_height,
            ..
        } => {
            if size.is_some_and(|v| !v.is_finite() || v <= 0.0) {
                bail!("font size must be finite and positive")
            }
            let nodes = layer_nodes_mut(page, &layer_id)?;
            let node = find_node_in_mut(nodes, &id)
                .with_context(|| format!("text not found in layer {layer_id}: {id}"))?;
            if node.get("kind").and_then(Value::as_str) != Some("text") {
                bail!("object is not text: {id}")
            }
            let object = node.as_object_mut().unwrap();
            let mut set = |key: &str, value: Option<Value>| {
                if let Some(value) = value {
                    object.insert(key.into(), value);
                }
            };
            set("content", content.map(Value::from));
            set("x", x.map(Value::from));
            set("y", y.map(Value::from));
            set("font_family", font.map(Value::from));
            set("font_size", size.map(Value::from));
            set("font_weight", weight.map(Value::from));
            set("italic", italic.map(Value::from));
            set("align", align.map(|a| serde_json::to_value(a).unwrap()));
            set("letter_spacing", letter_spacing.map(Value::from));
            set("line_height", line_height.map(Value::from));
            if let Some(fill) = fill {
                node["style"]["fill"] = json!({"fallback": fill});
            }
        }
        TextAction::Remove { id, .. } => {
            let nodes = layer_nodes_mut(page, &layer_id)?;
            let before = nodes.len();
            nodes.retain(|n| {
                !(n.get("id").and_then(Value::as_str) == Some(id.as_str())
                    && n.get("kind").and_then(Value::as_str) == Some("text"))
            });
            if nodes.len() == before {
                bail!("text not found in layer {layer_id}: {id}")
            }
        }
    }
    validate(raw)
}

/// `page add|rename|duplicate|move|remove` for scene (v4/v5) documents.
pub fn apply_page(raw: &mut Value, action: crate::page::PageAction) -> Result<Value> {
    use crate::page::PageAction;
    validate(raw)?;
    if let PageAction::List = action {
        return Ok(page_list(raw));
    }
    let pages = raw
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .context("document has no pages")?;
    let index_of = |pages: &[Value], id: &str| {
        pages
            .iter()
            .position(|p| p.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("page not found: {id}"))
    };
    let ensure_new = |pages: &[Value], id: &str| -> Result<()> {
        if id.trim().is_empty() {
            bail!("page ID cannot be empty")
        }
        if pages
            .iter()
            .any(|p| p.get("id").and_then(Value::as_str) == Some(id))
        {
            bail!("page ID already exists: {id}")
        }
        Ok(())
    };
    let result = match action {
        PageAction::List => unreachable!(),
        PageAction::Add {
            id,
            name,
            width,
            height,
            background,
        } => {
            ensure_new(pages, &id)?;
            pages.push(json!({
                "id": id,
                "name": name.unwrap_or_else(|| id.clone()),
                "canvas": {"width": width, "height": height, "background": background},
                "layers": [{"id": "layer-1", "name": "Layer 1", "visible": true, "locked": false, "nodes": []}]
            }));
            json!({"operation":"add","page":id})
        }
        PageAction::Rename { id, name } => {
            let index = index_of(pages, &id)?;
            pages[index]["name"] = json!(name);
            json!({"operation":"rename","page":id})
        }
        PageAction::Duplicate { id, new_id } => {
            ensure_new(pages, &new_id)?;
            let index = index_of(pages, &id)?;
            let mut copy = pages[index].clone();
            let name = copy
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            copy["id"] = json!(new_id);
            copy["name"] = json!(format!("{name} copy"));
            pages.insert(index + 1, copy);
            json!({"operation":"duplicate","page":id,"new_id":new_id})
        }
        PageAction::Move { id, index } => {
            if index >= pages.len() {
                bail!("page index out of range")
            }
            let old = index_of(pages, &id)?;
            let page = pages.remove(old);
            pages.insert(index, page);
            json!({"operation":"move","page":id,"index":index})
        }
        PageAction::Remove { id } => {
            if pages.len() == 1 {
                bail!("cannot remove the last page")
            }
            let index = index_of(pages, &id)?;
            pages.remove(index);
            let active = pages[index.min(pages.len() - 1)]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            json!({"operation":"remove","page":id,"active_page":active})
        }
    };
    validate(raw)?;
    Ok(result)
}

pub fn page_list(raw: &Value) -> Value {
    fn count(nodes: &[Value]) -> usize {
        nodes
            .iter()
            .map(|n| {
                1 + n
                    .get("children")
                    .and_then(Value::as_array)
                    .map_or(0, |c| count(c))
            })
            .sum()
    }
    let pages = raw
        .get("pages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    json!({"pages": pages.iter().enumerate().map(|(index, page)| {
        let layers = page.get("layers").and_then(Value::as_array).cloned().unwrap_or_default();
        json!({
            "id": page.get("id"),
            "name": page.get("name"),
            "index": index,
            "width": page["canvas"]["width"],
            "height": page["canvas"]["height"],
            "layers": layers.len(),
            "objects": layers.iter().map(|l| count(l.get("nodes").and_then(Value::as_array).map_or(&[][..], |n| n.as_slice()))).sum::<usize>()
        })
    }).collect::<Vec<_>>()})
}

/// `object set|rename|duplicate|move-to-layer|reorder|remove` for scene documents.
pub fn apply_object(
    raw: &mut Value,
    page_id: Option<&str>,
    action: &crate::agent::ObjectAction,
) -> Result<Value> {
    let compositing = crate::composite::is_document(raw);
    use crate::agent::ObjectAction;
    validate(raw)?;
    let page = page_mut(raw, page_id)?;
    let (id, layer_id) = match action {
        ObjectAction::Set { id, layer, .. }
        | ObjectAction::Rename { id, layer, .. }
        | ObjectAction::Duplicate { id, layer, .. }
        | ObjectAction::MoveToLayer { id, layer, .. }
        | ObjectAction::Reorder { id, layer, .. }
        | ObjectAction::Remove { id, layer } => (id.clone(), layer.clone()),
    };
    ensure_layer_unlocked(page, &layer_id)?;
    if compositing {
        crate::composite::ensure_unlocked(page, &id)?;
    }
    let position = |page: &mut Value| -> Result<usize> {
        layer_nodes_mut(page, &layer_id)?
            .iter()
            .position(|n| n.get("id").and_then(Value::as_str) == Some(id.as_str()))
            .with_context(|| format!("object not found in layer {layer_id}: {id}"))
    };
    let result = match action {
        ObjectAction::Set {
            d,
            stroke,
            width,
            fill,
            cap,
            join,
            miter_limit,
            content,
            x,
            y,
            font,
            size,
            weight,
            italic,
            align,
            letter_spacing,
            line_height,
            blend,
            blend_space,
            opacity,
            content_opacity,
            isolation,
            ..
        } => {
            let composite_change = blend.is_some()
                || blend_space.is_some()
                || opacity.is_some()
                || content_opacity.is_some()
                || isolation.is_some();
            if composite_change && !compositing {
                bail!(
                    "[unsupported-capability] compositing properties require `migrate --target 6`"
                )
            }
            let index = position(page)?;
            let node = &mut layer_nodes_mut(page, &layer_id)?[index];
            let kind = node.get("kind").and_then(Value::as_str).unwrap_or_default();
            if composite_change {
                let path_settings = d.is_some()
                    || stroke.is_some()
                    || width.is_some()
                    || cap.is_some()
                    || join.is_some()
                    || miter_limit.is_some();
                let text_settings = content.is_some()
                    || x.is_some()
                    || y.is_some()
                    || font.is_some()
                    || size.is_some()
                    || weight.is_some()
                    || italic.is_some()
                    || align.is_some()
                    || letter_spacing.is_some()
                    || line_height.is_some();
                if (kind != "path" && path_settings)
                    || (kind != "text" && text_settings)
                    || (!matches!(kind, "path" | "text") && fill.is_some())
                {
                    bail!("object {id}: geometry/text flags do not apply to {kind}; use its matching command")
                }
            }
            if kind == "path" {
                if let Some(d) = d {
                    BezPath::from_svg(d).context("invalid path data")?;
                    node["d"] = json!(d);
                }
                if let Some(v) = stroke {
                    node["style"]["stroke"] = json!({"fallback": v});
                }
                if let Some(v) = width {
                    node["style"]["stroke_width"] = json!({"fallback": v});
                }
                if let Some(v) = fill {
                    node["style"]["fill"] = json!({"fallback": v});
                }
                if let Some(v) = cap {
                    node["style"]["stroke_linecap"] = json!({"fallback": v.svg()});
                }
                if let Some(v) = join {
                    node["style"]["stroke_linejoin"] = json!({"fallback": v.svg()});
                }
                if let Some(v) = miter_limit {
                    node["style"]["stroke_miterlimit"] = json!({"fallback": v});
                }
            } else if kind == "text" {
                if size.is_some_and(|v| !v.is_finite() || v <= 0.0) {
                    bail!("font size must be finite and positive")
                }
                let object = node.as_object_mut().unwrap();
                let mut set = |key: &str, value: Option<Value>| {
                    if let Some(value) = value {
                        object.insert(key.into(), value);
                    }
                };
                set("content", content.clone().map(Value::from));
                set("x", x.map(Value::from));
                set("y", y.map(Value::from));
                set("font_family", font.clone().map(Value::from));
                set("font_size", size.map(Value::from));
                set("font_weight", weight.map(Value::from));
                set("italic", italic.map(Value::from));
                set("align", align.and_then(|a| serde_json::to_value(a).ok()));
                set("letter_spacing", letter_spacing.map(Value::from));
                set("line_height", line_height.map(Value::from));
                if let Some(v) = fill {
                    node["style"]["fill"] = json!({"fallback": v});
                }
            } else if !composite_change {
                bail!("object {id} is a {kind} node; use the matching command to edit it")
            }
            if let Some(v) = blend {
                node["blend_mode"] = json!(v);
            }
            if let Some(v) = blend_space {
                node["blend_space"] = json!(v);
            }
            if let Some(v) = opacity {
                node["opacity"] = json!(v);
            }
            if let Some(v) = content_opacity {
                node["content_opacity"] = json!(v);
            }
            if let Some(v) = isolation {
                node["isolation"] = json!(v);
            }
            json!({"operation":"set","id":id})
        }
        ObjectAction::Rename { new_id, .. } | ObjectAction::Duplicate { new_id, .. } => {
            if new_id.trim().is_empty() {
                bail!("object ID cannot be empty")
            }
            if pages_node_ids(page).contains(new_id.as_str()) {
                bail!("node ID already exists on page: {new_id}")
            }
            let index = position(page)?;
            let nodes = layer_nodes_mut(page, &layer_id)?;
            if matches!(action, ObjectAction::Rename { .. }) {
                nodes[index]["id"] = json!(new_id);
                json!({"operation":"rename","id":id,"new_id":new_id})
            } else {
                if nodes[index].get("children").is_some() {
                    bail!("duplicating a group would duplicate child IDs; duplicate its children instead")
                }
                let mut copy = nodes[index].clone();
                copy["id"] = json!(new_id);
                nodes.insert(index + 1, copy);
                json!({"operation":"duplicate","id":id,"new_id":new_id})
            }
        }
        ObjectAction::MoveToLayer { target_layer, .. } => {
            ensure_layer_unlocked(page, target_layer)?;
            let index = position(page)?;
            let node = layer_nodes_mut(page, &layer_id)?.remove(index);
            layer_nodes_mut(page, target_layer)?.push(node);
            json!({"operation":"move-to-layer","id":id,"layer":target_layer})
        }
        ObjectAction::Reorder { index, .. } => {
            let old = position(page)?;
            let nodes = layer_nodes_mut(page, &layer_id)?;
            if *index >= nodes.len() {
                bail!("object index out of range")
            }
            let node = nodes.remove(old);
            nodes.insert(*index, node);
            json!({"operation":"reorder","id":id,"index":index})
        }
        ObjectAction::Remove { .. } => {
            let index = position(page)?;
            layer_nodes_mut(page, &layer_id)?.remove(index);
            json!({"operation":"remove","id":id})
        }
    };
    validate(raw)?;
    Ok(result)
}

fn operation_affine(op: &crate::geometry::Operation) -> Result<Affine> {
    use crate::geometry::Operation;
    let about = |c: (f64, f64), m: Affine| {
        Affine::translate((c.0, c.1)) * m * Affine::translate((-c.0, -c.1))
    };
    let affine = match *op {
        Operation::Translate { dx, dy } => Affine::translate((dx, dy)),
        Operation::Transform { a, b, c, d, e, f } => Affine::new([a, b, c, d, e, f]),
        Operation::Rotate { degrees, cx, cy } => {
            about((cx, cy), Affine::rotate(degrees.to_radians()))
        }
        Operation::Scale { sx, sy, cx, cy } => about((cx, cy), Affine::scale_non_uniform(sx, sy)),
        _ => bail!("scene geometry supports bounds, translate, rotate, scale, and transform"),
    };
    if !affine.as_coeffs().iter().all(|v| v.is_finite()) {
        bail!("coordinates must be finite")
    }
    Ok(affine)
}

/// `geometry` for scene documents: bounds query or a transform applied to the node.
pub fn apply_geometry(
    raw: &mut Value,
    page_id: Option<&str>,
    layer_id: &str,
    id: &str,
    op: &crate::geometry::Operation,
) -> Result<Value> {
    if matches!(op, crate::geometry::Operation::Bounds) {
        return node_bounds(raw, page_id, id);
    }
    let affine = operation_affine(op)?;
    validate(raw)?;
    let page = page_mut(raw, page_id)?;
    ensure_layer_unlocked(page, layer_id)?;
    let nodes = layer_nodes_mut(page, layer_id)?;
    let node = find_node_in_mut(nodes, id)
        .with_context(|| format!("object not found in layer {layer_id}: {id}"))?;
    node["transform"] = json!((affine * matrix(node.get("transform"))?).as_coeffs());
    validate(raw)?;
    Ok(json!({"ok":true,"id":id}))
}

/// `layer-geometry` for scene documents: transforms every top-level node of a layer.
pub fn apply_layer_geometry(
    raw: &mut Value,
    page_id: Option<&str>,
    layer_id: &str,
    op: &crate::geometry::Operation,
) -> Result<Value> {
    if !matches!(
        op,
        crate::geometry::Operation::Transform { .. }
            | crate::geometry::Operation::Translate { .. }
            | crate::geometry::Operation::Rotate { .. }
            | crate::geometry::Operation::Scale { .. }
    ) {
        bail!("layer operations support translate, rotate, scale, and transform");
    }
    let affine = operation_affine(op)?;
    validate(raw)?;
    let page = page_mut(raw, page_id)?;
    ensure_layer_unlocked(page, layer_id)?;
    let nodes = layer_nodes_mut(page, layer_id)?;
    for node in nodes.iter_mut() {
        node["transform"] = json!((affine * matrix(node.get("transform"))?).as_coeffs());
    }
    let changed = nodes.len();
    validate(raw)?;
    Ok(json!({"ok":true,"nodes_changed":changed}))
}

/// Whether `text` is the documented color grammar: `none`, `#RGB`, `#RGBA`, `#RRGGBB`
/// or `#RRGGBBAA`.
pub fn is_valid_color(text: &str) -> bool {
    text == "none"
        || text.strip_prefix('#').is_some_and(|hex| {
            [3, 4, 6, 8].contains(&hex.len()) && hex.bytes().all(|c| c.is_ascii_hexdigit())
        })
}

/// Content problems a mutation must not introduce: canvas sizes the renderer
/// rejects, colors outside the grammar and blank identifiers. Documents that
/// already contain them stay editable; the transaction layer only refuses *new*
/// violations (see `transaction::reject_new_violations`).
pub fn content_violations(raw: &Value) -> Vec<String> {
    fn nodes(layer_path: &str, list: &[Value], out: &mut Vec<String>) {
        for node in list {
            let id = node.get("id").and_then(Value::as_str).unwrap_or_default();
            if id.trim().is_empty() && !id.is_empty() {
                out.push(format!(
                    "[invalid-input] {layer_path}: object ID cannot be blank"
                ));
            }
            for slot in ["fill", "stroke"] {
                if let Some(color) = node
                    .pointer(&format!("/style/{slot}/fallback"))
                    .and_then(Value::as_str)
                {
                    if !is_valid_color(color) {
                        out.push(format!(
                            "[invalid-input] {layer_path}: object {id} has invalid {slot} color {color:?}; use none, #RGB, #RGBA, #RRGGBB or #RRGGBBAA"
                        ));
                    }
                }
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                nodes(layer_path, children, out);
            }
        }
    }
    let mut out = Vec::new();
    for page in raw
        .get("pages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let page_id = page.get("id").and_then(Value::as_str).unwrap_or_default();
        let canvas = &page["canvas"];
        for side in ["width", "height"] {
            if !canvas
                .get(side)
                .and_then(Value::as_u64)
                .is_some_and(|v| (1..=16384).contains(&v))
            {
                out.push(format!(
                    "[invalid-input] page {page_id}: canvas {side} must be an integer between 1 and 16384"
                ));
            }
        }
        if let Some(background) = canvas.get("background").and_then(Value::as_str) {
            if !is_valid_color(background) {
                out.push(format!(
                    "[invalid-input] page {page_id}: canvas background {background:?} is not a color; use none, #RGB, #RGBA, #RRGGBB or #RRGGBBAA"
                ));
            }
        }
        for layer in page
            .get("layers")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            let layer_id = layer.get("id").and_then(Value::as_str).unwrap_or_default();
            if layer_id.trim().is_empty() && !layer_id.is_empty() {
                out.push(format!(
                    "[invalid-input] page {page_id}: layer ID cannot be blank"
                ));
            }
            if let Some(list) = layer.get("nodes").and_then(Value::as_array) {
                nodes(&format!("page {page_id}, layer {layer_id}"), list, &mut out);
            }
        }
    }
    out
}

pub fn validate(raw: &Value) -> Result<()> {
    let version = raw
        .get("version")
        .and_then(Value::as_u64)
        .context("[malformed-resource] document version is missing or invalid")?;
    if raw.get("format").and_then(Value::as_str) != Some("pentool")
        || !matches!(
            version,
            VERSION | crate::image::VERSION | crate::composite::VERSION
        )
    {
        bail!("not a supported Pentool scene document")
    }
    let image_assets = if version >= crate::image::VERSION {
        Some(crate::image::validate_assets(raw)?)
    } else {
        None
    };
    if let Some(styles) = raw.get("styles") {
        crate::style::validate_aliases(styles)?;
    }
    if version >= crate::composite::VERSION {
        crate::raster::validate_document(raw)?;
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
                validate_node(
                    node,
                    0,
                    &mut ids,
                    image_assets.as_ref(),
                    version == crate::composite::VERSION,
                )?;
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
        if version >= crate::image::VERSION {
            crate::image::validate_masks_for_version(page, version == crate::composite::VERSION)?;
        }
    }
    if version == crate::composite::VERSION {
        crate::composite::validate(raw)?;
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

/// A copy of `raw` whose `ref` bindings carry their token's current value as
/// `fallback`, so every render path (plain, image, composite) follows tokens.
pub(crate) fn with_resolved_tokens(raw: &Value) -> Result<Value> {
    let mut output = raw.clone();
    if let Some(styles) = raw.get("styles").filter(|styles| !styles.is_null()) {
        resolve_style_fallbacks(&mut output, styles)?;
    }
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
        "raster" => bail!(
            "[unsupported-capability] v3 flattening cannot represent raster node {}",
            object
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        "image" => bail!(
            "[unsupported-capability] v3 flattening cannot represent image node {}",
            object
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
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
pub(crate) fn validate_node(
    node: &Value,
    depth: usize,
    ids: &mut HashSet<String>,
    image_assets: Option<&HashMap<String, (u64, u64)>>,
    compositing: bool,
) -> Result<()> {
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
            validate_node(child, depth + 1, ids, image_assets, compositing)?
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
                validate_node(
                    fallback,
                    depth + 1,
                    &mut fallback_ids,
                    image_assets,
                    compositing,
                )?;
            }
            "fill" if compositing => {
                for key in ["x", "y", "width", "height"] {
                    number(o, key)?;
                }
                o.get("fill")
                    .and_then(Value::as_object)
                    .context("fill descriptor missing")?;
            }
            "adjustment" if compositing => crate::composite::validate_node(node)?,
            "raster" if compositing => crate::raster::validate_node(node)?,
            "image" => crate::image::validate_node_for_version(
                o,
                image_assets.context(
                    "[unsupported-capability] image nodes require document format version 5",
                )?,
                compositing,
            )?,
            _ => bail!("unsupported node kind: {kind}"),
        }
    };
    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
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

    #[test]
    fn v4_inspection_is_compact_and_paginates_nested_nodes() {
        let raw: Value =
            serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
        let first = inspect_paginated_v4(&raw, None, None, None, None, 0, 1).unwrap();
        assert_eq!(first["matches"], 3);
        assert_eq!(first["returned"], 1);
        assert_eq!(first["has_more"], true);
        assert_eq!(first["layers"][0]["objects"].as_array().unwrap().len(), 1);
        assert_eq!(first["layers"][0]["objects"][0]["id"], "card");
        // Groups list direct child IDs only, never the child nodes themselves.
        let children = first["layers"][0]["objects"][0]["children"]
            .as_array()
            .unwrap();
        assert!(children.iter().all(Value::is_string));
        assert!(children.contains(&json!("card-bg")));

        let second = inspect_paginated_v4(&raw, None, None, None, None, 1, 1).unwrap();
        assert_eq!(second["layers"][0]["objects"][0]["id"], "card-bg");
        assert_eq!(second["layers"][0]["objects"][0]["parent"], "card");
        assert_eq!(second["layers"][0]["objects"][0]["depth"], 1);
    }

    #[test]
    fn v4_search_matches_text_and_maps_path_filter_to_shapes() {
        let raw: Value =
            serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
        let text =
            inspect_paginated_v4(&raw, None, Some("semantic"), Some("text"), None, 0, 10).unwrap();
        assert_eq!(text["matches"], 1);
        assert_eq!(text["layers"][0]["objects"][0]["id"], "title");

        let shapes = inspect_paginated_v4(&raw, None, None, Some("path"), None, 0, 10).unwrap();
        assert_eq!(shapes["matches"], 1);
        assert_eq!(shapes["layers"][0]["objects"][0]["kind"], "rect");
    }

    #[test]
    fn new_canvas_and_path_commands_are_native_v4() {
        let mut raw = new_document(320, 240);
        assert_eq!(raw["version"], 4);
        edit_canvas(&mut raw, None, Some(640), None, Some("#abc"), Some("Demo")).unwrap();
        assert_eq!(raw["pages"][0]["canvas"]["width"], 640);
        assert_eq!(raw["name"], "Demo");
        apply_path(
            &mut raw,
            None,
            crate::editing::PathAction::Put {
                id: "outline".into(),
                layer: "layer-1".into(),
                d: "M0 0 L10 10".into(),
                stroke: "#123456".into(),
                width: 2.0,
                fill: "none".into(),
                closed: false,
                cap: crate::document::StrokeCap::Round,
                join: crate::document::StrokeJoin::Bevel,
                miter_limit: 4.0,
            },
        )
        .unwrap();
        assert_eq!(raw["pages"][0]["layers"][0]["nodes"][0]["kind"], "path");
        assert_eq!(
            raw["pages"][0]["layers"][0]["nodes"][0]["style"]["stroke"]["fallback"],
            "#123456"
        );
    }

    #[test]
    fn batch_flat_styles_and_stroke_refs_are_normalized() {
        let mut raw = new_document(100, 100);
        raw["styles"]["outline"] = json!({"type":"color","value":"#112233"});
        let operations = json!([{
            "type":"put-path", "id":"p", "layer":"layer-1", "d":"M0 0 L20 20",
            "fill":"none", "stroke":"#000000", "stroke_ref":"outline", "stroke_width":3
        }]);
        apply_batch(&mut raw, None, operations.as_array().unwrap()).unwrap();
        let node = &raw["pages"][0]["layers"][0]["nodes"][0];
        assert_eq!(node["style"]["stroke"]["ref"], "outline");
        assert_eq!(node["style"]["stroke_width"]["fallback"], 3);
        assert!(node.get("stroke").is_none());
        let usage = crate::style::apply(
            &mut raw,
            crate::style::Operation::Usage,
            Some("outline"),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(usage["ids"], json!(["p"]));
        let tree = inspect_paginated_v4(&raw, None, None, None, None, 0, 10).unwrap();
        assert_eq!(
            tree["layers"][0]["objects"][0]["style_refs"]["stroke"],
            "outline"
        );
        let flat = flatten_to_v3(&raw).unwrap();
        assert_eq!(
            flat["pages"][0]["layers"][0]["paths"][0]["stroke"],
            "#112233"
        );
    }

    #[test]
    fn bounded_text_uses_box_bounds_inside_groups() {
        let mut raw = new_document(800, 600);
        raw["pages"][0]["layers"][0]["nodes"] = json!([
            {"kind":"rect","id":"card","x":0,"y":0,"width":260,"height":100,"radius_x":0,"radius_y":0,"style":{}},
            {"kind":"text","id":"copy","content":"a very long line that would otherwise measure wider","x":0,"y":16,"width":200,"height":40,"font_size":16,"line_height":1.2,"style":{"fill":{"fallback":"#000"}}}
        ]);
        apply_group(
            &mut raw,
            None,
            GroupAction::Create {
                id: "group".into(),
                layer: "layer-1".into(),
                children: vec!["card".into(), "copy".into()],
            },
        )
        .unwrap();
        let bounds = node_bounds(&raw, None, "group").unwrap();
        assert_eq!(bounds["width"], 260.0);
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

    let doc_name = raw
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Untitled");
    let version = raw.get("version").and_then(Value::as_u64).unwrap_or(4);
    let active_page = page.get("id").and_then(Value::as_str).unwrap_or("page-1");

    let page_layers = page
        .get("layers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (layer_index, layer) in page_layers.iter().enumerate() {
        let lid = layer.get("id").and_then(Value::as_str).unwrap_or("");
        let lname = layer.get("name").and_then(Value::as_str).unwrap_or("");

        if let Some(f) = layer_filter {
            if lid != f && !lname.to_lowercase().contains(&f.to_lowercase()) {
                continue;
            }
        }

        let layer_match = needle.is_empty()
            || lid.to_lowercase().contains(&needle)
            || lname.to_lowercase().contains(&needle);
        let mut candidates = Vec::new();
        if let Some(nodes) = layer.get("nodes").and_then(Value::as_array) {
            collect_v4_matches(
                nodes,
                None,
                0,
                &needle,
                layer_match,
                kind_filter,
                &mut candidates,
            );
        }

        let layer_matches = candidates.len();
        let start = offset.saturating_sub(matches).min(layer_matches);
        let remaining = limit.saturating_sub(returned);
        let take = layer_matches.saturating_sub(start).min(remaining);
        if take > 0 {
            let mut objects = Vec::with_capacity(take);
            for mut object in candidates.into_iter().skip(start).take(take) {
                let id = object["id"].as_str().unwrap_or_default();
                object["bounds"] = node_bounds_on_page(page, id)?;
                objects.push(object);
            }
            returned += objects.len();
            layers.push(json!({
                "id": lid,
                "name": lname,
                "index": layer_index,
                "visible": layer.get("visible").and_then(Value::as_bool).unwrap_or(true),
                "locked": layer.get("locked").and_then(Value::as_bool).unwrap_or(false),
                "objects": objects
            }));
        }
        matches += layer_matches;
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
        "stacking": "layers, then depth-first nodes; zero is back",
        "layers": layers
    }))
}

fn collect_v4_matches(
    nodes: &[Value],
    parent: Option<&str>,
    depth: usize,
    needle: &str,
    ancestor_match: bool,
    kind_filter: Option<&str>,
    output: &mut Vec<Value>,
) {
    for (index, node) in nodes.iter().enumerate() {
        let id = node.get("id").and_then(Value::as_str).unwrap_or("");
        let name = node.get("name").and_then(Value::as_str).unwrap_or("");
        let kind = node.get("kind").and_then(Value::as_str).unwrap_or("");
        let content = node.get("content").and_then(Value::as_str);
        let query_matches = ancestor_match
            || id.to_lowercase().contains(needle)
            || name.to_lowercase().contains(needle)
            || content.is_some_and(|value| value.to_lowercase().contains(needle));
        let kind_matches = match kind_filter {
            Some("text") => kind == "text",
            Some("path") => matches!(kind, "path" | "rect" | "ellipse" | "line"),
            Some(expected) => kind == expected,
            None => true,
        };
        if query_matches && kind_matches {
            let mut object = json!({
                "id": id,
                "kind": kind,
                "index": index,
                "depth": depth,
                "draw_order": output.len()
            });
            if let Some(parent) = parent {
                object["parent"] = json!(parent);
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                object["children"] = Value::Array(
                    children
                        .iter()
                        .filter_map(|child| child.get("id").and_then(Value::as_str))
                        .map(|id| json!(id))
                        .collect(),
                );
            }
            if let Some(content) = content {
                object["content"] = json!(content);
            }
            if let Some(asset) = node.get("asset").and_then(Value::as_str) {
                object["asset"] = json!(asset);
            }
            if let Some(style) = node.get("style").and_then(Value::as_object) {
                let refs = style
                    .iter()
                    .filter_map(|(property, value)| {
                        value
                            .get("ref")
                            .and_then(Value::as_str)
                            .map(|reference| (property.clone(), json!(reference)))
                    })
                    .collect::<Map<String, Value>>();
                if !refs.is_empty() {
                    object["style_refs"] = Value::Object(refs);
                }
            }
            output.push(object);
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            collect_v4_matches(
                children,
                Some(id),
                depth + 1,
                needle,
                ancestor_match,
                kind_filter,
                output,
            );
        }
    }
}

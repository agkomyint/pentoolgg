//! Version-11 source bindings and deterministic dependency planning.

use anyhow::{bail, ensure, Context, Result};
use kurbo::Shape;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

type Ref = (String, String);

struct PlanGraph {
    order: Vec<Ref>,
    edges: BTreeMap<Ref, BTreeSet<Ref>>,
    reasons: BTreeMap<(Ref, Ref), BTreeSet<String>>,
}

/// Set or clear artwork linked to one stable dieline panel ID.
pub fn set_panel_artwork(
    raw: &mut Value,
    dieline_id: &str,
    panel_id: &str,
    source: Option<&Value>,
) -> Result<Value> {
    ensure!(
        raw.get("version").and_then(Value::as_u64) == Some(11),
        "[unsupported-capability] panel artwork requires document format v11"
    );
    let panel = raw
        .get_mut("dielines")
        .and_then(|dielines| dielines.get_mut(dieline_id))
        .and_then(|dieline| dieline.get_mut("panels"))
        .and_then(Value::as_array_mut)
        .and_then(|panels| {
            panels
                .iter_mut()
                .find(|panel| panel.get("id").and_then(Value::as_str) == Some(panel_id))
        })
        .with_context(|| {
            format!("[missing-resource] dieline `{dieline_id}` has no panel `{panel_id}`")
        })?;
    if let Some(source) = source {
        ensure!(
            source.is_object(),
            "[invalid-input] panel artwork source must be an object"
        );
        panel["artwork"] = source.clone();
        Ok(serde_json::json!({"dieline":dieline_id,"panel":panel_id,"artwork":source}))
    } else {
        if let Some(object) = panel.as_object_mut() {
            object.remove("artwork");
        }
        Ok(serde_json::json!({"dieline":dieline_id,"panel":panel_id,"artwork":null}))
    }
}

/// Assign a production operation to an ordinary path on a linked dieline page.
/// The path remains a normal editable `.pen` node; its stable metadata ID makes
/// the shared dieline exporters include it in the matching production plate.
/// A crease role is a scoring path only; structural fold edges still require
/// the separate validated shared-panel-edge operation.
pub fn set_layout_path_role(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    role: &str,
) -> Result<Value> {
    ensure!(
        raw.get("version").and_then(Value::as_u64) == Some(11),
        "[unsupported-capability] packaging production roles require document format v11"
    );
    ensure!(
        matches!(
            role,
            "cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap"
        ),
        "[invalid-input] new packaging paths support cut, crease, perforation, bleed, safe-zone, or glue-flap"
    );
    let layout_page_id = raw
        .get("dielines")
        .and_then(|items| items.get(dieline_id))
        .and_then(|dieline| dieline.get("layout_page"))
        .and_then(Value::as_str)
        .with_context(|| {
            format!("[missing-resource] dieline `{dieline_id}` has no editable layout page")
        })?
        .to_owned();
    let existing_path_ids = raw["dielines"][dieline_id]["paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|path| path.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let page = raw
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .and_then(|pages| pages.iter_mut().find(|page| page["id"] == layout_page_id))
        .with_context(|| {
            format!("[missing-resource] editable dieline page `{layout_page_id}` is missing")
        })?;
    ensure!(
        page["dieline_layout"]["dieline"].as_str() == Some(dieline_id),
        "[invalid-input] page `{layout_page_id}` does not declare dieline `{dieline_id}`"
    );

    fn find_path<'a>(
        nodes: &'a mut [Value],
        id: &str,
        locked: bool,
    ) -> Result<Option<&'a mut Value>> {
        for node in nodes {
            let is_locked = locked || node.get("locked").and_then(Value::as_bool) == Some(true);
            if node.get("id").and_then(Value::as_str) == Some(id) {
                ensure!(
                    !is_locked,
                    "[locked] packaging path `{id}` is on a locked node or layer"
                );
                ensure!(
                    node.get("kind").and_then(Value::as_str) == Some("path"),
                    "[invalid-input] selected dieline object `{id}` is not a path"
                );
                return Ok(Some(node));
            }
            if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                if let Some(found) = find_path(children, id, is_locked)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }

    let mut selected = None;
    for layer in page
        .get_mut("layers")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let locked = layer.get("locked").and_then(Value::as_bool) == Some(true);
        if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
            selected = find_path(nodes, node_id, locked)?;
            if selected.is_some() {
                break;
            }
        }
    }
    let node = selected.with_context(|| {
        format!("[missing-resource] editable dieline page has no path `{node_id}`")
    })?;
    ensure!(
        node.get("dieline_crease_id").is_none(),
        "[invalid-input] structural fold edge `{node_id}` cannot be reassigned as a production path"
    );
    let data = node
        .get("d")
        .and_then(Value::as_str)
        .context("[invalid-input] selected path has no SVG path data")?;
    let path = kurbo::BezPath::from_svg(data)
        .context("[invalid-input] selected path has malformed SVG path data")?;
    ensure!(
        !path.elements().is_empty() && path.is_finite() && path.elements().len() <= 20_000,
        "[invalid-input] selected path must contain finite, bounded geometry"
    );
    let existing_path_id = node.get("dieline_path_id").and_then(Value::as_str);
    let path_id = existing_path_id
        .map(str::to_owned)
        .unwrap_or_else(|| format!("layout-{layout_page_id}-{node_id}"));
    if existing_path_id.is_none() {
        ensure!(
            !existing_path_ids.contains(&path_id),
            "[conflict] generated path ID `{path_id}` already belongs to another production path"
        );
    }
    let spot = match role {
        "cut" => "CutContour",
        "crease" => "Crease",
        "perforation" => "Perforation",
        "bleed" => "Bleed",
        "safe-zone" => "SafeZone",
        "glue-flap" => "GlueFlap",
        _ => unreachable!(),
    };
    // Keep these canvas swatches aligned with pack::role_color and Studio's
    // packaging production legend; they distinguish technical marks visually.
    let display_color = match role {
        "cut" => "#dc2626",
        "crease" => "#2563eb",
        "perforation" => "#7c3aed",
        "bleed" => "#f97316",
        "safe-zone" => "#16a34a",
        "glue-flap" => "#0891b2",
        _ => unreachable!(),
    };
    node["dieline_path_id"] = serde_json::json!(path_id);
    node["dieline_path_role"] = serde_json::json!(role);
    node["dieline_path_spot"] = serde_json::json!(spot);
    node["style"]["fill"]["fallback"] = serde_json::json!("none");
    node["style"]["stroke"]["fallback"] = serde_json::json!(display_color);
    if node["style"]["stroke_width"]["fallback"].as_f64().is_none() {
        node["style"]["stroke_width"]["fallback"] = serde_json::json!(0.2);
    }

    Ok(
        serde_json::json!({"dieline":dieline_id,"path":path_id,"node":node_id,"role":role,"spot":spot}),
    )
}

/// Promote a straight path on the editable packaging page into a structural
/// hinge. Unlike a production mark assignment, this is accepted only when the
/// selected segment is exactly the shared edge of two known panels.
pub fn set_layout_path_crease(
    raw: &mut Value,
    dieline_id: &str,
    node_id: &str,
    crease_id: &str,
    angle_deg: f64,
    direction: &str,
) -> Result<Value> {
    ensure!(
        raw.get("version").and_then(Value::as_u64) == Some(11),
        "[unsupported-capability] structural packaging creases require document format v11"
    );
    ensure!(
        !crease_id.trim().is_empty() && crease_id.len() <= 128,
        "[invalid-input] crease ID must contain 1..=128 characters"
    );
    ensure!(
        angle_deg.is_finite() && (-180.0..=180.0).contains(&angle_deg),
        "[invalid-input] crease target angle must be within -180..=180 degrees"
    );
    ensure!(
        matches!(direction, "mountain" | "valley"),
        "[invalid-input] crease direction must be `mountain` or `valley`"
    );
    let dieline = raw
        .get("dielines")
        .and_then(|items| items.get(dieline_id))
        .with_context(|| format!("[missing-resource] dieline `{dieline_id}` does not exist"))?
        .clone();
    ensure!(
        !dieline["creases"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["id"] == crease_id),
        "[conflict] crease ID `{crease_id}` already exists on dieline `{dieline_id}`"
    );
    let layout_page_id = dieline["layout_page"]
        .as_str()
        .context(
            "[unsupported-capability] structural crease creation requires an editable layout page",
        )?
        .to_owned();
    let bleed = raw["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|page| page["id"] == layout_page_id)
        .and_then(|page| page["production"]["bleed"].as_f64())
        .unwrap_or(0.0);
    ensure!(
        bleed.is_finite() && bleed >= 0.0,
        "[invalid-input] layout bleed is invalid"
    );

    fn find<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
        nodes
            .iter_mut()
            .find(|node| node["id"].as_str() == Some(id))
    }
    let line = {
        let pages = raw["pages"]
            .as_array_mut()
            .context("[invalid-input] pages must be an array")?;
        let page = pages
            .iter_mut()
            .find(|page| page["id"] == layout_page_id)
            .with_context(|| {
                format!("[missing-resource] editable dieline page `{layout_page_id}` is missing")
            })?;
        ensure!(
            page["dieline_layout"]["dieline"].as_str() == Some(dieline_id),
            "[invalid-input] page `{layout_page_id}` is not linked to dieline `{dieline_id}`"
        );
        let mut selected = None;
        for layer in page["layers"].as_array_mut().into_iter().flatten() {
            ensure!(
                layer["locked"].as_bool() != Some(true),
                "[locked] packaging layer is locked"
            );
            if let Some(nodes) = layer["nodes"].as_array_mut() {
                selected = find(nodes, node_id);
                if selected.is_some() {
                    break;
                }
            }
        }
        let node = selected.with_context(|| {
            format!("[missing-resource] editable dieline page has no top-level path `{node_id}`")
        })?;
        ensure!(
            node["kind"] == "path",
            "[invalid-input] selected dieline object `{node_id}` is not a path"
        );
        ensure!(
            node["locked"].as_bool() != Some(true),
            "[locked] packaging path `{node_id}` is locked"
        );
        ensure!(
        node.get("dieline_crease_id").is_none() && node.get("dieline_path_role").is_none()
            && node.get("x").is_none() && node.get("y").is_none() && node.get("transform").is_none(),
        "[unsupported-capability] select an untransformed, unassigned path directly on the dieline layer"
    );
        let path = kurbo::BezPath::from_svg(
            node["d"]
                .as_str()
                .context("[invalid-input] selected path has no SVG path data")?,
        )
        .context("[invalid-input] selected path has malformed SVG path data")?;
        let [kurbo::PathEl::MoveTo(a), kurbo::PathEl::LineTo(b)] = path.elements() else {
            bail!(
                "[unsupported-capability] a structural crease must be one straight two-point path"
            )
        };
        ensure!(
            path.is_finite(),
            "[invalid-input] selected crease path contains non-finite coordinates"
        );
        let line = [[a.x - bleed, a.y - bleed], [b.x - bleed, b.y - bleed]];
        let panels = dieline["panels"]
            .as_array()
            .context("[invalid-input] dieline panels must be an array")?;
        let tol = line.iter().flatten().fold(1.0_f64, |s, v| s.max(v.abs())) * 1.0e-8;
        let close = |x: f64, y: f64| (x - y).abs() <= tol;
        let mut shared_edge = false;
        for i in 0..panels.len() {
            for j in i + 1..panels.len() {
                let rect = |p: &Value| -> Option<[f64; 4]> {
                    let r = p["rect_mm"].as_array()?;
                    Some([
                        r[0].as_f64()?,
                        r[1].as_f64()?,
                        r[2].as_f64()?,
                        r[3].as_f64()?,
                    ])
                };
                let (Some([x1, y1, w1, h1]), Some([x2, y2, w2, h2])) =
                    (rect(&panels[i]), rect(&panels[j]))
                else {
                    continue;
                };
                let (y0, y1_overlap) = (y1.max(y2), (y1 + h1).min(y2 + h2));
                let (x0, x1_overlap) = (x1.max(x2), (x1 + w1).min(x2 + w2));
                let endpoints_match = |edge: [[f64; 2]; 2]| {
                    (close(line[0][0], edge[0][0])
                        && close(line[0][1], edge[0][1])
                        && close(line[1][0], edge[1][0])
                        && close(line[1][1], edge[1][1]))
                        || (close(line[1][0], edge[0][0])
                            && close(line[1][1], edge[0][1])
                            && close(line[0][0], edge[1][0])
                            && close(line[0][1], edge[1][1]))
                };
                if (close(x1 + w1, x2) || close(x2 + w2, x1)) && y1_overlap > y0 + tol {
                    let x = if close(x1 + w1, x2) { x2 } else { x1 };
                    shared_edge |= endpoints_match([[x, y0], [x, y1_overlap]]);
                }
                if (close(y1 + h1, y2) || close(y2 + h2, y1)) && x1_overlap > x0 + tol {
                    let y = if close(y1 + h1, y2) { y2 } else { y1 };
                    shared_edge |= endpoints_match([[x0, y], [x1_overlap, y]]);
                }
            }
        }
        ensure!(shared_edge, "[invalid-input] selected path is not exactly a shared edge between two dieline panels; choose the hinge edge or draw a straight path along it");
        node["dieline_crease_id"] = serde_json::json!(crease_id);
        node["style"]["fill"]["fallback"] = serde_json::json!("none");
        node["style"]["stroke"]["fallback"] = serde_json::json!("#0891b2");
        if node["style"]["stroke_width"]["fallback"].as_f64().is_none() {
            node["style"]["stroke_width"]["fallback"] = serde_json::json!(0.2);
        }
        line
    };
    let source_dieline = raw["dielines"].get_mut(dieline_id).unwrap();
    source_dieline["creases"].as_array_mut().context("[invalid-input] dieline creases must be an array")?
        .push(serde_json::json!({"id":crease_id,"line_mm":line,"angle_deg":angle_deg,"direction":direction}));
    Ok(
        serde_json::json!({"dieline":dieline_id,"crease":crease_id,"node":node_id,"line_mm":line,"angle_deg":angle_deg,"direction":direction}),
    )
}

/// Set the normalized placement of a linked panel graphic. The same transform
/// is consumed by the flat SVG exporter and the folded surface binding path.
pub fn set_panel_artwork_transform(
    raw: &mut Value,
    dieline_id: &str,
    panel_id: &str,
    scale: [f64; 2],
    offset: [f64; 2],
    rotate: f64,
) -> Result<Value> {
    ensure!(
        raw.get("version").and_then(Value::as_u64) == Some(11),
        "[unsupported-capability] panel artwork placement requires document format v11"
    );
    ensure!(
        scale
            .iter()
            .all(|value| value.is_finite() && *value > 0.0 && *value <= 1.0),
        "[invalid-input] panel artwork scale must be within (0,1] on each axis"
    );
    ensure!(
        offset.iter().all(|value| value.is_finite())
            && offset[0] >= scale[0] / 2.0
            && offset[0] <= 1.0 - scale[0] / 2.0
            && offset[1] >= scale[1] / 2.0
            && offset[1] <= 1.0 - scale[1] / 2.0,
        "[invalid-input] panel artwork center must keep the unrotated graphic inside its panel"
    );
    ensure!(
        rotate.is_finite() && rotate.abs() <= 360.0,
        "[invalid-input] panel artwork rotation must be within -360..=360 degrees"
    );
    let panel = raw
        .get_mut("dielines")
        .and_then(|catalog| catalog.get_mut(dieline_id))
        .and_then(|dieline| dieline.get_mut("panels"))
        .and_then(Value::as_array_mut)
        .and_then(|panels| {
            panels
                .iter_mut()
                .find(|panel| panel.get("id").and_then(Value::as_str) == Some(panel_id))
        })
        .with_context(|| {
            format!("[missing-resource] dieline `{dieline_id}` has no panel `{panel_id}`")
        })?;
    let transform = serde_json::json!({"scale":scale,"offset":offset,"rotate":rotate});
    panel["artwork_transform"] = transform.clone();
    Ok(serde_json::json!({"dieline":dieline_id,"panel":panel_id,"artwork_transform":transform}))
}

/// Set the panel's printable safe-area inset in millimeters. This is a guide,
/// not synthesized bleed or manufacturing validation.
pub fn set_panel_safe_zone(
    raw: &mut Value,
    dieline_id: &str,
    panel_id: &str,
    inset_mm: f64,
) -> Result<Value> {
    ensure!(
        raw.get("version").and_then(Value::as_u64) == Some(11),
        "[unsupported-capability] panel safe-area guides require document format v11"
    );
    ensure!(
        inset_mm.is_finite() && inset_mm >= 0.0,
        "[invalid-input] panel safe-area inset must be a finite non-negative number"
    );
    let mut candidate = raw.clone();
    let resolved = super::pack::dieline_from_document(&candidate, dieline_id)?;
    let panel_rect = resolved["panels"]
        .as_array()
        .and_then(|panels| panels.iter().find(|panel| panel["id"] == panel_id))
        .and_then(|panel| panel["rect_mm"].as_array())
        .filter(|rect| rect.len() == 4)
        .context("[missing-resource] resolved dieline panel geometry is missing")?;
    let panel_x = panel_rect[0]
        .as_f64()
        .context("[invalid-input] panel x is missing")?;
    let panel_y = panel_rect[1]
        .as_f64()
        .context("[invalid-input] panel y is missing")?;
    let panel_width = panel_rect[2]
        .as_f64()
        .context("[invalid-input] panel width is missing")?;
    let panel_height = panel_rect[3]
        .as_f64()
        .context("[invalid-input] panel height is missing")?;
    let bleed = resolved["dimensions_mm"]["bleed"].as_f64().unwrap_or(0.0);
    let panel = candidate
        .get_mut("dielines")
        .and_then(|catalog| catalog.get_mut(dieline_id))
        .and_then(|dieline| dieline.get_mut("panels"))
        .and_then(Value::as_array_mut)
        .and_then(|panels| {
            panels
                .iter_mut()
                .find(|panel| panel.get("id").and_then(Value::as_str) == Some(panel_id))
        })
        .with_context(|| {
            format!("[missing-resource] dieline `{dieline_id}` has no panel `{panel_id}`")
        })?;
    let rect = panel
        .get("rect_mm")
        .and_then(Value::as_array)
        .filter(|rect| rect.len() == 4)
        .context("[invalid-input] panel rect_mm must be [x, y, width, height]")?;
    let width = bounded_number(&rect[2], "panel width_mm", f64::MIN_POSITIVE, 1_000_000.0)?;
    let height = bounded_number(&rect[3], "panel height_mm", f64::MIN_POSITIVE, 1_000_000.0)?;
    ensure!(
        inset_mm * 2.0 < width.min(height),
        "[invalid-input] panel safe-area inset must be less than half the panel's shortest side"
    );
    panel["safe_zone_mm"] = serde_json::json!(inset_mm);
    if let Some(layout_page_id) = candidate["dielines"][dieline_id]["layout_page"].as_str() {
        let layout_page_id = layout_page_id.to_owned();
        let page = candidate["pages"]
            .as_array_mut()
            .and_then(|pages| pages.iter_mut().find(|page| page["id"] == layout_page_id))
            .context("[missing-resource] editable dieline page is missing")?;
        let guide_rect = [
            panel_x + bleed + inset_mm,
            panel_y + bleed + inset_mm,
            panel_width - 2.0 * inset_mm,
            panel_height - 2.0 * inset_mm,
        ];
        fn remove_guide(nodes: &mut Vec<Value>, panel_id: &str) -> Option<Value> {
            if let Some(index) = nodes
                .iter()
                .position(|node| node["dieline_safe_zone_panel"] == panel_id)
            {
                return Some(nodes.remove(index));
            }
            for node in nodes {
                if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
                    if let Some(guide) = remove_guide(children, panel_id) {
                        return Some(guide);
                    }
                }
            }
            None
        }
        let mut guide = None;
        for layer in page["layers"].as_array_mut().into_iter().flatten() {
            if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                guide = remove_guide(nodes, panel_id);
                if guide.is_some() {
                    break;
                }
            }
        }
        if inset_mm > 0.0 {
            let mut guide = guide.unwrap_or_else(|| {
                serde_json::json!({
                    "kind":"rect", "id":format!("{layout_page_id}-safe-{panel_id}"),
                    "name":format!("Safe area · {panel_id}"), "dieline_safe_zone_panel":panel_id,
                    "style":{"fill":{"fallback":"none"},"stroke":{"fallback":"#a855f7"},"stroke_width":{"fallback":0.2},"stroke_dasharray":{"fallback":"1.2 0.8"}}
                })
            });
            guide["x"] = serde_json::json!(guide_rect[0]);
            guide["y"] = serde_json::json!(guide_rect[1]);
            guide["width"] = serde_json::json!(guide_rect[2]);
            guide["height"] = serde_json::json!(guide_rect[3]);
            guide.as_object_mut().unwrap().remove("transform");
            let nodes = page["layers"]
                .as_array_mut()
                .and_then(|layers| layers.first_mut())
                .and_then(|layer| layer.get_mut("nodes"))
                .and_then(Value::as_array_mut)
                .context("[invalid-input] editable dieline guide layer has no node array")?;
            nodes.push(guide);
        }
    }
    *raw = candidate;
    Ok(serde_json::json!({"dieline":dieline_id,"panel":panel_id,"safe_zone_mm":inset_mm}))
}

#[derive(Clone, Copy, Default)]
struct NodeState {
    visible: bool,
    locked: bool,
}

fn walk(
    node: &Value,
    page: &str,
    ancestors_visible: bool,
    ancestors_locked: bool,
    nodes: &mut BTreeMap<Ref, Value>,
    all: &mut BTreeMap<Ref, NodeState>,
) -> Result<()> {
    if let Some(object) = node.as_object() {
        let visible =
            ancestors_visible && object.get("visible").and_then(Value::as_bool) != Some(false);
        let locked =
            ancestors_locked || object.get("locked").and_then(Value::as_bool) == Some(true);
        if let Some(id) = object.get("id").and_then(Value::as_str) {
            all.insert(
                (page.to_owned(), id.to_owned()),
                NodeState { visible, locked },
            );
        }
        if object.get("kind").and_then(Value::as_str) == Some("scene3d") {
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .context("[invalid-input] scene3d node has no ID")?;
            nodes.insert((page.to_owned(), id.to_owned()), node.clone());
        }
        if let Some(children) = object.get("children").and_then(Value::as_array) {
            for child in children {
                walk(child, page, visible, locked, nodes, all)?;
            }
        }
    }
    Ok(())
}

fn node_map(raw: &Value) -> Result<(BTreeMap<Ref, Value>, BTreeMap<Ref, NodeState>)> {
    let mut nodes = BTreeMap::new();
    let mut all = BTreeMap::new();
    for page in raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let page_id = page
            .get("id")
            .and_then(Value::as_str)
            .context("[invalid-input] page has no ID")?;
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
                let layer_visible = layer.get("visible").and_then(Value::as_bool) != Some(false);
                let layer_locked = layer.get("locked").and_then(Value::as_bool) == Some(true);
                walk(
                    node,
                    page_id,
                    layer_visible,
                    layer_locked,
                    &mut nodes,
                    &mut all,
                )?;
            }
        }
    }
    Ok((nodes, all))
}

/// Return scene3d render dependencies in stable dependency-first order and
/// reject unresolved sources or cycles before any export/mutation occurs.
fn build_plan(raw: &Value) -> Result<PlanGraph> {
    let (nodes, all_nodes) = node_map(raw)?;
    let mut form_vertices = 0usize;
    if let Some(forms) = raw.get("product_forms") {
        let forms = forms
            .as_object()
            .context("[invalid-input] v11 product_forms must be an object")?;
        ensure!(
            forms.len() <= 256,
            "[limit-exceeded] a document holds at most 256 product forms"
        );
        for (id, form) in forms {
            let generated =
                super::form::generate(form).with_context(|| format!("product form {id}"))?;
            form_vertices = form_vertices
                .checked_add(generated.report.vertices)
                .context("generated product form vertex count overflow")?;
            ensure!(
                form_vertices <= super::model::LIMITS.max_vertices,
                "[limit-exceeded] document product forms exceed the {} generated-vertex limit",
                super::model::LIMITS.max_vertices
            );
        }
    }
    let mut page_ids = BTreeSet::new();
    for page in raw
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(id) = page.get("id").and_then(Value::as_str) {
            page_ids.insert(id.to_owned());
        }
    }
    let mut edges: BTreeMap<Ref, BTreeSet<Ref>> = nodes
        .keys()
        .cloned()
        .map(|key| (key, BTreeSet::new()))
        .collect();
    let mut edge_reasons = BTreeMap::<(Ref, Ref), BTreeSet<String>>::new();
    for (consumer, value) in &nodes {
        let graphics = value.get("surface_graphics").and_then(Value::as_array);
        let Some(graphics) = graphics else { continue };
        ensure!(
            graphics.len() <= 256,
            "[limit-exceeded] scene3d node {} has more than 256 surface graphics",
            consumer.1
        );
        let mut ids = BTreeSet::new();
        for graphic in graphics {
            let graphic_id = graphic
                .get("id")
                .and_then(Value::as_str)
                .context("[invalid-input] surface graphic has no ID")?;
            ensure!(
                !graphic_id.is_empty() && ids.insert(graphic_id),
                "[invalid-input] scene3d node {} has duplicate or empty surface graphic ID `{graphic_id}`",
                consumer.1
            );
            let slot = graphic
                .get("slot")
                .and_then(Value::as_str)
                .context("[invalid-input] surface graphic slot is missing")?;
            ensure!(
                !slot.trim().is_empty(),
                "[invalid-input] surface graphic {graphic_id} slot cannot be empty"
            );
            if let Some(form_id) = value.get("product_form").and_then(Value::as_str) {
                let form = raw
                    .get("product_forms")
                    .and_then(|forms| forms.get(form_id))
                    .with_context(|| format!("[missing-resource] scene3d node {} refers to missing product form `{form_id}`", consumer.1))?;
                let slots = form
                    .get("material_slots")
                    .and_then(Value::as_array)
                    .context("[invalid-input] product form material_slots must be an array")?;
                ensure!(
                    slots
                        .iter()
                        .any(|candidate| candidate.as_str() == Some(slot)),
                    "[missing-resource] surface graphic {graphic_id} targets missing material slot `{slot}` on product form `{form_id}` (consumer {}/{})",
                    consumer.0,
                    consumer.1
                );
            }
            ensure!(
                matches!(
                    graphic.get("mode").and_then(Value::as_str),
                    Some("decal" | "fill")
                ),
                "[invalid-input] surface graphic {graphic_id} mode must be decal or fill"
            );
            let projection = graphic
                .get("projection")
                .and_then(Value::as_str)
                .unwrap_or("uv");
            ensure!(
                matches!(projection, "uv" | "planar" | "box" | "cylindrical" | "spherical" | "triplanar"),
                "[unsupported-capability] surface graphic {graphic_id} projection `{projection}` is not implemented by the CPU renderer yet"
            );
            let axis = graphic.get("axis").and_then(Value::as_str).unwrap_or("z");
            ensure!(
                projection == "uv" || matches!(axis, "x" | "y" | "z"),
                "[invalid-input] surface graphic {graphic_id} projection axis must be x, y or z"
            );
            ensure!(
                projection != "uv" || graphic.get("axis").is_none(),
                "[invalid-input] surface graphic {graphic_id} axis is only valid for a procedural projection"
            );
            ensure!(
                projection != "triplanar" || graphic.get("axis").is_none(),
                "[invalid-input] surface graphic {graphic_id} triplanar projection does not use an axis"
            );
            if let Some(island) = graphic.get("island") {
                ensure!(
                    island.as_str().is_some_and(|id| !id.is_empty()),
                    "[invalid-input] surface graphic {graphic_id} island must be a non-empty stable UV island ID"
                );
            }
            if let Some(material) = graphic.get("material") {
                let material = material.as_object().with_context(|| {
                    format!("[invalid-input] surface graphic {graphic_id} material response must be an object")
                })?;
                for (property, value) in material {
                    ensure!(
                        matches!(property.as_str(), "metallic" | "roughness" | "opacity"),
                        "[invalid-input] surface graphic {graphic_id} material response has unsupported property `{property}`"
                    );
                    let value = value.as_f64().with_context(|| {
                        format!("[invalid-input] surface graphic {graphic_id} material.{property} must be a number")
                    })?;
                    ensure!(
                        value.is_finite() && (0.0..=1.0).contains(&value),
                        "[invalid-input] surface graphic {graphic_id} material.{property} must be within 0..=1"
                    );
                }
            }
            let blend = graphic
                .get("blend")
                .and_then(Value::as_str)
                .unwrap_or("normal");
            ensure!(
                matches!(
                    blend,
                    "normal" | "multiply" | "screen" | "overlay" | "darken" | "lighten"
                ),
                "[invalid-input] surface graphic {graphic_id} has unknown blend mode `{blend}`"
            );
            let wrap = graphic
                .get("wrap")
                .and_then(Value::as_str)
                .unwrap_or("clamp");
            ensure!(
                matches!(wrap, "clamp" | "repeat" | "mirror"),
                "[invalid-input] surface graphic {graphic_id} wrap must be clamp, repeat or mirror"
            );
            let fit = graphic
                .get("fit")
                .and_then(Value::as_str)
                .unwrap_or("contain");
            ensure!(
                matches!(fit, "contain" | "cover" | "stretch"),
                "[invalid-input] surface graphic {graphic_id} fit must be contain, cover or stretch"
            );
            let mode = graphic
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("decal");
            let pair = |field: &str, default: [f64; 2]| -> Result<[f64; 2]> {
                let Some(value) = graphic.get(field) else {
                    return Ok(default);
                };
                let values = value.as_array().filter(|values| values.len() == 2).with_context(|| format!("[invalid-input] surface graphic {graphic_id} {field} must be a numeric pair"))?;
                let x = values[0]
                    .as_f64()
                    .context("surface graphic pair values must be numbers")?;
                let y = values[1]
                    .as_f64()
                    .context("surface graphic pair values must be numbers")?;
                ensure!(
                    x.is_finite() && y.is_finite(),
                    "[invalid-input] surface graphic {graphic_id} {field} values must be finite"
                );
                Ok([x, y])
            };
            let scale = pair("scale", [1.0, 1.0])?;
            let offset = pair(
                "offset",
                if mode == "fill" {
                    [0.0, 0.0]
                } else {
                    [0.5, 0.5]
                },
            )?;
            if mode == "decal" {
                ensure!(
                    scale.iter().all(|v| *v > 0.0 && *v <= 1.0)
                        && offset[0] >= scale[0] / 2.0
                        && offset[0] <= 1.0 - scale[0] / 2.0
                        && offset[1] >= scale[1] / 2.0
                        && offset[1] <= 1.0 - scale[1] / 2.0,
                    "[invalid-input] surface graphic {graphic_id} decal scale/offset must fit within normalized UV bounds"
                );
            } else {
                ensure!(
                    scale[0] >= 1.0
                        && scale[1] >= 1.0
                        && scale[0] <= 16.0
                        && scale[1] <= 16.0
                        && scale[0] * scale[1] <= 256.0
                        && offset.iter().all(|v| (0.0..=1.0).contains(v)),
                    "[invalid-input] surface graphic {graphic_id} fill supports 1..=16 repeats per axis, at most 256 tiles, and a normalized tile phase"
                );
            }
            if let Some(rotate) = graphic.get("rotate") {
                let rotate = rotate
                    .as_f64()
                    .context("[invalid-input] surface graphic rotation must be a number")?;
                ensure!(
                    rotate.is_finite() && rotate.abs() <= 360.0,
                    "[invalid-input] surface graphic rotation must be within -360..=360 degrees"
                );
            }
            if let Some(crop) = graphic.get("crop") {
                let values = crop
                    .as_array()
                    .filter(|values| values.len() == 4)
                    .context("[invalid-input] surface graphic crop must be [x,y,width,height]")?;
                let values: Vec<f64> = values
                    .iter()
                    .map(|value| {
                        value
                            .as_f64()
                            .context("[invalid-input] surface crop values must be numbers")
                    })
                    .collect::<Result<_>>()?;
                ensure!(
                    values.iter().all(|value| value.is_finite())
                        && values[0] >= 0.0
                        && values[1] >= 0.0
                        && values[2] > 0.0
                        && values[3] > 0.0
                        && values[0] + values[2] <= 1.0
                        && values[1] + values[3] <= 1.0,
                    "[invalid-input] surface graphic {graphic_id} crop must stay inside normalized 0..=1 bounds"
                );
            }
            let source = graphic
                .get("source")
                .and_then(Value::as_object)
                .context("[invalid-input] surface graphic source must be an object")?;
            let kind = source
                .get("kind")
                .and_then(Value::as_str)
                .context("[invalid-input] surface graphic source kind is missing")?;
            let page = source
                .get("page")
                .and_then(Value::as_str)
                .context("[invalid-input] surface graphic source page is missing")?;
            ensure!(
                page_ids.contains(page),
                "[missing-resource] surface graphic {graphic_id} on consumer {}/{} in slot `{slot}` refers to missing source page `{page}`; create that page or update/clear the binding",
                consumer.0,
                consumer.1
            );
            match kind {
                "page" => ensure!(
                    source.len() == 2,
                    "[invalid-input] page source for graphic {graphic_id} has unexpected fields"
                ),
                "node" => {
                    ensure!(
                        source.len() == 3,
                        "[invalid-input] node source for graphic {graphic_id} must include page and node"
                    );
                    let source_id = source
                        .get("node")
                        .and_then(Value::as_str)
                        .context("[invalid-input] node source ID is missing")?;
                    let reference = (page.to_owned(), source_id.to_owned());
                    let state = all_nodes.get(&reference).with_context(|| format!("[missing-resource] surface graphic {graphic_id} on consumer {}/{} in slot `{slot}` refers to missing source node {page}/{source_id}; choose an existing node or update/clear the binding", consumer.0, consumer.1))?;
                    ensure!(
                        state.visible,
                        "[invalid-input] surface graphic {graphic_id} source {page}/{source_id} is hidden; show it or bind a visible source (consumer {}/{}, slot {slot})",
                        consumer.0,
                        consumer.1
                    );
                    ensure!(
                        !state.locked,
                        "[invalid-input] surface graphic {graphic_id} source {page}/{source_id} is locked; unlock it or bind an editable source (consumer {}/{}, slot {slot})",
                        consumer.0,
                        consumer.1
                    );
                    ensure!(
                        &reference != consumer,
                        "[dependency-cycle] graphic {graphic_id} in slot `{slot}` on consumer {}/{} directly references source {}/{}; choose a different source or remove the binding",
                        consumer.0,
                        consumer.1,
                        reference.0,
                        reference.1
                    );
                    if nodes.contains_key(&reference) {
                        edges.get_mut(consumer).unwrap().insert(reference.clone());
                        edge_reasons
                            .entry((consumer.clone(), reference.clone()))
                            .or_default()
                            .insert(format!(
                                "graphic {graphic_id} slot `{slot}` source node {page}/{source_id}"
                            ));
                    }
                }
                other => bail!(
                    "[invalid-input] surface graphic {graphic_id} on consumer {}/{} in slot `{slot}` has unsupported source kind `{other}`; use a page or node source, or remove the binding",
                    consumer.0,
                    consumer.1
                ),
            }
            if kind == "page" {
                for reference in nodes
                    .keys()
                    .filter(|reference| reference.0 == page && *reference != consumer)
                {
                    edges.get_mut(consumer).unwrap().insert(reference.clone());
                    edge_reasons
                        .entry((consumer.clone(), reference.clone()))
                        .or_default()
                        .insert(format!(
                            "graphic {graphic_id} slot `{slot}` source page {page}"
                        ));
                }
            }
            if let Some(opacity) = graphic.get("opacity") {
                let opacity = opacity
                    .as_f64()
                    .context("[invalid-input] surface graphic opacity must be a number")?;
                ensure!(
                    opacity.is_finite() && (0.0..=1.0).contains(&opacity),
                    "[invalid-input] surface graphic opacity must be within 0..=1"
                );
            }
        }
    }
    validate_dielines(
        raw,
        &nodes,
        &all_nodes,
        &page_ids,
        &mut edges,
        &mut edge_reasons,
    )?;
    fn visit(
        key: &Ref,
        edges: &BTreeMap<Ref, BTreeSet<Ref>>,
        edge_reasons: &BTreeMap<(Ref, Ref), BTreeSet<String>>,
        marks: &mut BTreeMap<Ref, u8>,
        out: &mut Vec<Ref>,
    ) -> Result<()> {
        match marks.get(key).copied().unwrap_or(0) {
            2 => return Ok(()),
            1 => bail!("[dependency-cycle] dependency cycle returns to {}/{}; remove or rebind one of the listed scene3d source edges", key.0, key.1),
            _ => {}
        }
        marks.insert(key.clone(), 1);
        if let Some(dependencies) = edges.get(key) {
            for dependency in dependencies {
                if marks.get(dependency).copied() == Some(1) {
                    let reason = edge_reasons
                        .get(&(key.clone(), dependency.clone()))
                        .and_then(BTreeSet::first)
                        .map(String::as_str)
                        .unwrap_or("scene3d dependency");
                    bail!("[dependency-cycle] consumer {}/{} depends on source {}/{} through {reason}, closing a cycle; remove or rebind this graphic edge",
                        key.0, key.1, dependency.0, dependency.1);
                }
                visit(dependency, edges, edge_reasons, marks, out)?;
            }
        }
        marks.insert(key.clone(), 2);
        out.push(key.clone());
        Ok(())
    }
    let mut marks = BTreeMap::new();
    let mut out = Vec::<Ref>::new();
    for key in edges.keys() {
        visit(key, &edges, &edge_reasons, &mut marks, &mut out)?;
    }
    Ok(PlanGraph {
        order: out,
        edges,
        reasons: edge_reasons,
    })
}

/// Return the dependency-first sequence as stable `page/node` identifiers.
pub fn plan(raw: &Value) -> Result<Vec<String>> {
    Ok(build_plan(raw)?
        .order
        .into_iter()
        .map(|(page, node)| format!("{page}/{node}"))
        .collect())
}

/// Compact deterministic graph and render order for CLI and agent inspection.
pub fn plan_report(raw: &Value) -> Result<Value> {
    let plan = build_plan(raw)?;
    let render_order = plan
        .order
        .iter()
        .map(|(page, node)| serde_json::json!({"page":page,"node":node}))
        .collect::<Vec<_>>();
    let dependencies = plan
        .reasons
        .iter()
        .map(|((consumer, source), reasons)| {
            serde_json::json!({
                "consumer":{"page":consumer.0,"node":consumer.1},
                "source":{"page":source.0,"node":source.1},
                "bindings":reasons,
            })
        })
        .collect::<Vec<_>>();
    let nodes = plan
        .edges
        .iter()
        .map(|(key, sources)| {
            serde_json::json!({
                "page":key.0,
                "node":key.1,
                "dependencies":sources.iter().map(|(page,node)| serde_json::json!({"page":page,"node":node})).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_value(serde_json::json!({
        "format":"pentool-scene3d-dependency-plan-v1",
        "render_order":render_order,
        "nodes":nodes,
        "dependencies":dependencies,
        "summary":{"nodes":plan.edges.len(),"edges":dependencies.len()},
    }))
    .context("could not serialize scene3d dependency plan")
}

fn bounded_number(value: &Value, label: &str, minimum: f64, maximum: f64) -> Result<f64> {
    let number = value
        .as_f64()
        .with_context(|| format!("[invalid-input] {label} must be a number"))?;
    ensure!(
        number.is_finite() && number >= minimum && number <= maximum,
        "[invalid-input] {label} must be within {minimum}..={maximum}"
    );
    Ok(number)
}

fn validate_dielines(
    raw: &Value,
    nodes: &BTreeMap<Ref, Value>,
    all_nodes: &BTreeMap<Ref, NodeState>,
    page_ids: &BTreeSet<String>,
    edges: &mut BTreeMap<Ref, BTreeSet<Ref>>,
    edge_reasons: &mut BTreeMap<(Ref, Ref), BTreeSet<String>>,
) -> Result<()> {
    let Some(catalog) = raw.get("dielines") else {
        return Ok(());
    };
    let catalog = catalog
        .as_object()
        .context("[invalid-input] v11 dielines must be an object")?;
    ensure!(
        catalog.len() <= 256,
        "[limit-exceeded] a document holds at most 256 packaging dielines"
    );
    for (dieline_id, dieline) in catalog {
        if let Some(layout_page_value) = dieline.get("layout_page") {
            let layout_page_id = layout_page_value.as_str().with_context(|| {
                format!("[invalid-input] dieline `{dieline_id}` layout_page must be a page ID")
            })?;
            ensure!(page_ids.contains(layout_page_id), "[missing-resource] dieline `{dieline_id}` layout page `{layout_page_id}` does not exist");
            let page = raw["pages"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|page| page["id"].as_str() == Some(layout_page_id))
                .context("[missing-resource] editable dieline page is missing")?;
            ensure!(page["dieline_layout"]["dieline"].as_str()==Some(dieline_id.as_str()) && page["dieline_layout"]["coordinates"].as_str()==Some("mm"), "[invalid-input] page `{layout_page_id}` does not declare the expected dieline ID and millimeter coordinates");
        }
        let resolved_dieline = super::pack::dieline_from_document(raw, dieline_id)?;
        let dieline = &resolved_dieline;
        ensure!(
            !dieline_id.trim().is_empty() && dieline_id.len() <= 128,
            "[invalid-input] dieline IDs must contain 1..=128 characters"
        );
        let template = dieline
            .get("template")
            .and_then(Value::as_str)
            .context("[invalid-input] dieline template is missing")?;
        ensure!(
            matches!(
                template,
                "straight-tuck"
                    | "reverse-tuck"
                    | "sleeve"
                    | "mailer"
                    | "cylinder"
                    | "tube"
                    | "pouch"
                    | "bottle-label"
                    | "svg-import"
            ),
            "[unsupported-capability] dieline `{dieline_id}` template `{template}` is unsupported"
        );
        ensure!(
            dieline.get("units").and_then(Value::as_str) == Some("mm"),
            "[invalid-input] dieline `{dieline_id}` units must be mm"
        );
        let dimensions = dieline
            .get("dimensions_mm")
            .and_then(Value::as_object)
            .context("[invalid-input] dieline dimensions_mm must be an object")?;
        let length = bounded_number(
            dimensions.get("length").unwrap_or(&Value::Null),
            "dieline length_mm",
            f64::MIN_POSITIVE,
            100_000.0,
        )?;
        let width = bounded_number(
            dimensions.get("width").unwrap_or(&Value::Null),
            "dieline width_mm",
            f64::MIN_POSITIVE,
            100_000.0,
        )?;
        let height = bounded_number(
            dimensions.get("height").unwrap_or(&Value::Null),
            "dieline height_mm",
            f64::MIN_POSITIVE,
            100_000.0,
        )?;
        if matches!(template, "tube" | "bottle-label") {
            let wrap = dieline
                .get("wrap")
                .context("[invalid-input] tube dielines require an explicit wrap mapping")?;
            let radius = bounded_number(
                wrap.get("radius_mm").unwrap_or(&Value::Null),
                "tube wrap radius_mm",
                1.0,
                100_000.0,
            )?;
            let overlap = bounded_number(
                wrap.get("overlap_mm").unwrap_or(&Value::from(0.0)),
                "tube overlap_mm",
                0.0,
                std::f64::consts::TAU * radius * 0.25,
            )?;
            let panels = dieline
                .get("panels")
                .and_then(Value::as_array)
                .context("[invalid-input] tube dieline panels must be an array")?;
            ensure!(
                panels.len() == 1,
                "[invalid-input] tube dielines require exactly one wrap panel"
            );
            let panel_width = panels[0]["rect_mm"][2]
                .as_f64()
                .context("[invalid-input] tube label panel width must be numeric")?;
            let expected_width = std::f64::consts::TAU * radius + overlap;
            ensure!(
                (panel_width - expected_width).abs() <= expected_width * 1e-6,
                "[invalid-input] tube label panel width must equal 2π × wrap radius plus overlap_mm"
            );
            ensure!(
                (length - 2.0 * radius).abs() <= 2.0 * radius * 1e-6
                    && (width - 2.0 * radius).abs() <= 2.0 * radius * 1e-6,
                "[invalid-input] tube dieline length and width must equal twice its wrap radius"
            );
            bounded_number(
                wrap.get("seam_angle_deg").unwrap_or(&Value::Null),
                "tube seam_angle_deg",
                -360.0,
                360.0,
            )?;
            ensure!(
                matches!(
                    wrap.get("direction").and_then(Value::as_str),
                    Some("clockwise" | "counterclockwise")
                ),
                "[invalid-input] tube wrap direction must be clockwise or counterclockwise"
            );
        } else {
            ensure!(
                dieline.get("wrap").is_none(),
                "[invalid-input] wrap mapping is only supported for tube and bottle-label dielines"
            );
        }
        let board = bounded_number(
            dimensions.get("board").unwrap_or(&Value::Null),
            "dieline board thickness_mm",
            0.0,
            100.0,
        )?;
        ensure!(
            board < length.min(width).min(height) / 2.0,
            "[invalid-input] dieline `{dieline_id}` board thickness must be less than half the smallest package dimension"
        );
        bounded_number(
            dimensions.get("bleed").unwrap_or(&Value::Null),
            "dieline bleed_mm",
            0.0,
            100.0,
        )?;
        if let Some(glue) = dimensions.get("glue_flap") {
            bounded_number(glue, "dieline glue_flap_mm", 0.0, 100_000.0)?;
        }
        let fold_state = dieline.get("fold_state").map_or(Ok(1.0), |state| {
            bounded_number(state, "dieline fold_state", 0.0, 1.0)
        })?;
        let _ = fold_state;
        let owner_id = dieline
            .get("scene3d_node")
            .and_then(Value::as_str)
            .with_context(|| {
                format!("[invalid-input] dieline `{dieline_id}` must identify its scene3d_node")
            })?;
        let owner = nodes
            .keys()
            .find(|(_, node_id)| node_id == owner_id)
            .cloned()
            .with_context(|| format!("[missing-resource] dieline `{dieline_id}` scene3d node `{owner_id}` does not exist"))?;
        ensure!(
            nodes[&owner].get("dieline").and_then(Value::as_str) == Some(dieline_id.as_str()),
            "[invalid-input] scene3d node `{owner_id}` must refer back to dieline `{dieline_id}`"
        );

        let panels = dieline
            .get("panels")
            .and_then(Value::as_array)
            .context("[invalid-input] dieline panels must be an array")?;
        ensure!(
            !panels.is_empty() && panels.len() <= 256,
            "[limit-exceeded] dieline `{dieline_id}` must have 1..=256 panels"
        );
        let mut panel_ids = BTreeSet::new();
        let mut rects = Vec::with_capacity(panels.len());
        for panel in panels {
            let panel_id = panel
                .get("id")
                .and_then(Value::as_str)
                .context("[invalid-input] dieline panel has no stable ID")?;
            ensure!(
                !panel_id.trim().is_empty() && panel_ids.insert(panel_id),
                "[invalid-input] dieline `{dieline_id}` has duplicate or empty panel ID `{panel_id}`"
            );
            let slot = panel
                .get("face_slot")
                .and_then(Value::as_str)
                .with_context(|| {
                    format!("[invalid-input] dieline panel `{panel_id}` has no face_slot")
                })?;
            ensure!(
                !slot.trim().is_empty(),
                "[invalid-input] dieline panel `{panel_id}` face_slot cannot be empty"
            );
            let rect = panel.get("rect_mm").and_then(Value::as_array).filter(|r| r.len() == 4)
                .with_context(|| format!("[invalid-input] dieline panel `{panel_id}` rect_mm must be [x,y,width,height]"))?;
            let x = bounded_number(&rect[0], "panel x_mm", 0.0, 1_000_000.0)?;
            let y = bounded_number(&rect[1], "panel y_mm", 0.0, 1_000_000.0)?;
            let w = bounded_number(&rect[2], "panel width_mm", f64::MIN_POSITIVE, 1_000_000.0)?;
            let h = bounded_number(&rect[3], "panel height_mm", f64::MIN_POSITIVE, 1_000_000.0)?;
            rects.push((panel_id, [x, y, w, h]));
            if let Some(safe_zone) = panel.get("safe_zone_mm") {
                let safe_zone = bounded_number(safe_zone, "panel safe_zone_mm", 0.0, 500_000.0)?;
                ensure!(
                    safe_zone * 2.0 < w.min(h),
                    "[invalid-input] panel `{panel_id}` safe_zone_mm must be less than half its shortest side"
                );
            }
            let uv = panel.get("uv_rect").and_then(Value::as_array).filter(|r| r.len() == 4)
                .with_context(|| format!("[invalid-input] dieline panel `{panel_id}` uv_rect must be [u,v,width,height]"))?;
            let u = bounded_number(&uv[0], "panel u", 0.0, 1.0)?;
            let v = bounded_number(&uv[1], "panel v", 0.0, 1.0)?;
            let uw = bounded_number(&uv[2], "panel UV width", f64::MIN_POSITIVE, 1.0)?;
            let vh = bounded_number(&uv[3], "panel UV height", f64::MIN_POSITIVE, 1.0)?;
            ensure!(
                u + uw <= 1.0 && v + vh <= 1.0,
                "[invalid-input] dieline panel `{panel_id}` uv_rect must stay within 0..=1"
            );
            if let Some(transform) = panel.get("artwork_transform") {
                let transform_object = transform.as_object().with_context(|| {
                    format!(
                        "[invalid-input] panel `{panel_id}` artwork_transform must be an object"
                    )
                })?;
                ensure!(
                    transform_object.len() == 3
                        && transform_object.contains_key("scale")
                        && transform_object.contains_key("offset")
                        && transform_object.contains_key("rotate"),
                    "[invalid-input] panel `{panel_id}` artwork_transform requires only scale, offset, and rotate"
                );
                let read_pair = |field: &str, min: f64, max: f64| -> Result<[f64; 2]> {
                    let values = transform[field].as_array().filter(|values| values.len() == 2)
                        .with_context(|| format!("[invalid-input] panel `{panel_id}` artwork_transform.{field} must be a pair"))?;
                    Ok([
                        bounded_number(
                            &values[0],
                            &format!("panel {panel_id} artwork {field} x"),
                            min,
                            max,
                        )?,
                        bounded_number(
                            &values[1],
                            &format!("panel {panel_id} artwork {field} y"),
                            min,
                            max,
                        )?,
                    ])
                };
                let scale = read_pair("scale", f64::MIN_POSITIVE, 1.0)?;
                let offset = read_pair("offset", 0.0, 1.0)?;
                ensure!(
                    offset[0] >= scale[0] / 2.0
                        && offset[0] <= 1.0 - scale[0] / 2.0
                        && offset[1] >= scale[1] / 2.0
                        && offset[1] <= 1.0 - scale[1] / 2.0,
                    "[invalid-input] panel `{panel_id}` artwork center must keep the unrotated graphic inside its panel"
                );
                bounded_number(
                    &transform["rotate"],
                    &format!("panel {panel_id} artwork rotation"),
                    -360.0,
                    360.0,
                )?;
            }
            if let Some(source) = panel.get("artwork") {
                let kind = source
                    .get("kind")
                    .and_then(Value::as_str)
                    .with_context(|| {
                        format!(
                            "[invalid-input] dieline panel `{panel_id}` artwork kind is missing"
                        )
                    })?;
                let page = source
                    .get("page")
                    .and_then(Value::as_str)
                    .with_context(|| {
                        format!(
                            "[invalid-input] dieline panel `{panel_id}` artwork page is missing"
                        )
                    })?;
                ensure!(
                    page_ids.contains(page),
                    "[missing-resource] dieline panel `{panel_id}` in slot `{slot}` on consumer {}/{} refers to missing source page `{page}`; create that page or update/clear the panel artwork",
                    owner.0,
                    owner.1
                );
                match kind {
                    "page" => {
                        ensure!(
                            source.as_object().is_some_and(|o| o.len() == 2),
                            "[invalid-input] dieline panel `{panel_id}` page artwork has unexpected fields"
                        );
                        for dependency in nodes.keys().filter(|key| key.0 == page && *key != &owner) {
                            edges.get_mut(&owner).unwrap().insert(dependency.clone());
                            edge_reasons
                                .entry((owner.clone(), dependency.clone()))
                                .or_default()
                                .insert(format!("dieline `{dieline_id}` panel `{panel_id}` slot `{slot}` artwork source page {page}"));
                        }
                    }
                    "node" => {
                        let source_id = source.get("node").and_then(Value::as_str)
                            .with_context(|| format!("[invalid-input] dieline panel `{panel_id}` node artwork has no node ID"))?;
                        let reference = (page.to_owned(), source_id.to_owned());
                        let state = all_nodes.get(&reference).with_context(|| format!("[missing-resource] dieline panel `{panel_id}` in slot `{slot}` on consumer {}/{} refers to missing source node {page}/{source_id}; choose an existing node or update/clear the panel artwork", owner.0, owner.1))?;
                        ensure!(
                            state.visible,
                            "[invalid-input] dieline panel `{panel_id}` source {page}/{source_id} is hidden"
                        );
                        ensure!(
                            !state.locked,
                            "[invalid-input] dieline panel `{panel_id}` source {page}/{source_id} is locked"
                        );
                        ensure!(
                            reference != owner,
                            "[dependency-cycle] dieline `{dieline_id}` panel `{panel_id}` slot `{slot}` on consumer {}/{} refers to source {}/{}; choose a different source or clear the panel artwork",
                            owner.0,
                            owner.1,
                            reference.0,
                            reference.1
                        );
                        if nodes.contains_key(&reference) {
                            edges.get_mut(&owner).unwrap().insert(reference.clone());
                            edge_reasons
                                .entry((owner.clone(), reference.clone()))
                                .or_default()
                                .insert(format!("dieline `{dieline_id}` panel `{panel_id}` slot `{slot}` artwork source node {page}/{source_id}"));
                        }
                    }
                    other => bail!(
                        "[invalid-input] dieline panel `{panel_id}` artwork kind `{other}` must be page or node"
                    ),
                }
            }
        }
        for (index, (left_id, left)) in rects.iter().enumerate() {
            for (right_id, right) in rects.iter().skip(index + 1) {
                let overlap_x =
                    left[0].max(right[0]) < (left[0] + left[2]).min(right[0] + right[2]);
                let overlap_y =
                    left[1].max(right[1]) < (left[1] + left[3]).min(right[1] + right[3]);
                ensure!(
                    !(overlap_x && overlap_y),
                    "[invalid-input] dieline `{dieline_id}` panels `{left_id}` and `{right_id}` overlap in the flat layout"
                );
            }
        }

        let creases = dieline
            .get("creases")
            .and_then(Value::as_array)
            .context("[invalid-input] dieline creases must be an array")?;
        ensure!(
            creases.len() <= 512,
            "[limit-exceeded] dieline `{dieline_id}` has more than 512 creases"
        );
        let mut crease_ids = BTreeSet::new();
        for crease in creases {
            let id = crease
                .get("id")
                .and_then(Value::as_str)
                .context("[invalid-input] dieline crease has no ID")?;
            ensure!(
                !id.trim().is_empty() && crease_ids.insert(id),
                "[invalid-input] dieline `{dieline_id}` has duplicate or empty crease ID `{id}`"
            );
            bounded_number(
                crease.get("angle_deg").unwrap_or(&Value::Null),
                "crease angle_deg",
                -180.0,
                180.0,
            )?;
            ensure!(
                matches!(
                    crease.get("direction").and_then(Value::as_str),
                    Some("mountain" | "valley")
                ),
                "[invalid-input] crease `{id}` direction must be mountain or valley"
            );
            let line = crease
                .get("line_mm")
                .and_then(Value::as_array)
                .filter(|v| v.len() == 2)
                .with_context(|| {
                    format!("[invalid-input] crease `{id}` line_mm must contain two points")
                })?;
            let mut points = [[0.0; 2]; 2];
            for (point_index, point) in line.iter().enumerate() {
                let point = point.as_array().filter(|p| p.len() == 2).with_context(|| {
                    format!("[invalid-input] crease `{id}` points must be [x,y] pairs")
                })?;
                points[point_index] = [
                    bounded_number(&point[0], "crease x_mm", 0.0, 1_000_000.0)?,
                    bounded_number(&point[1], "crease y_mm", 0.0, 1_000_000.0)?,
                ];
            }
            ensure!(
                points[0] != points[1],
                "[invalid-input] crease `{id}` has zero length"
            );
        }

        let paths = dieline
            .get("paths")
            .and_then(Value::as_array)
            .context("[invalid-input] dieline paths must be an array")?;
        ensure!(
            paths.len() <= 4096,
            "[limit-exceeded] dieline `{dieline_id}` has more than 4096 production paths"
        );
        let mut path_ids = BTreeSet::new();
        let mut contour_panels = BTreeSet::new();
        for path in paths {
            let id = path
                .get("id")
                .and_then(Value::as_str)
                .context("[invalid-input] production path has no ID")?;
            ensure!(
                !id.trim().is_empty() && path_ids.insert(id),
                "[invalid-input] dieline `{dieline_id}` has duplicate or empty path ID `{id}`"
            );
            let role = path.get("role").and_then(Value::as_str);
            ensure!(
                matches!(
                    role,
                    Some("cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap")
                ),
                "[invalid-input] production path `{id}` has an unknown role"
            );
            ensure!(
                path.get("d")
                    .and_then(Value::as_str)
                    .is_some_and(|d| !d.is_empty() && d.len() <= 1_000_000),
                "[invalid-input] production path `{id}` must have a nonempty path no longer than 1,000,000 bytes"
            );
            if let Some(panel_id) = path.get("panel_id") {
                let panel_id = panel_id.as_str().with_context(|| {
                    format!("[invalid-input] production path `{id}` panel_id must be a panel ID")
                })?;
                ensure!(
                    role == Some("cut"),
                    "[invalid-input] only cut production path `{id}` may target a panel"
                );
                ensure!(
                    contour_panels.insert(panel_id),
                    "[invalid-input] panel `{panel_id}` has more than one associated cut contour; combine its closed subpaths into one production path"
                );
                let (_, panel_rect) = rects
                    .iter()
                    .find(|(known_id, _)| *known_id == panel_id)
                    .with_context(|| {
                        format!("[missing-resource] cut path `{id}` refers to missing panel `{panel_id}`")
                    })?;
                let data = path["d"].as_str().unwrap();
                let contour = kurbo::BezPath::from_svg(data).with_context(|| {
                    format!("[invalid-input] associated cut path `{id}` has invalid SVG path data")
                })?;
                ensure!(
                    contour.is_finite()
                        && !contour.elements().is_empty()
                        && contour.elements().iter().any(|el| matches!(el, kurbo::PathEl::ClosePath)),
                    "[invalid-input] associated cut path `{id}` for panel `{panel_id}` must contain a closed contour"
                );
                let bounds = contour.bounding_box();
                let [x, y, width, height] = *panel_rect;
                let tolerance = x.max(y).max(width).max(height).max(1.0) * 1.0e-8;
                ensure!(
                    bounds.x0 >= x - tolerance
                        && bounds.y0 >= y - tolerance
                        && bounds.x1 <= x + width + tolerance
                        && bounds.y1 <= y + height + tolerance,
                    "[invalid-input] cut contour `{id}` bounds must stay within its associated panel `{panel_id}`"
                );
            }
        }
        super::pack::generate(dieline).with_context(|| format!("folded dieline `{dieline_id}`"))?;
    }
    for (reference, node) in nodes {
        if let Some(dieline_id) = node.get("dieline") {
            let dieline_id = dieline_id.as_str().with_context(|| {
                format!(
                    "[invalid-input] scene3d node `{}` dieline must be an ID",
                    reference.1
                )
            })?;
            let dieline = catalog.get(dieline_id).with_context(|| {
                format!(
                    "[missing-resource] scene3d node `{}` refers to missing dieline `{dieline_id}`",
                    reference.1
                )
            })?;
            ensure!(
                dieline.get("scene3d_node").and_then(Value::as_str) == Some(reference.1.as_str()),
                "[invalid-input] dieline `{dieline_id}` does not declare scene3d node `{}` as its owner",
                reference.1
            );
        }
    }
    Ok(())
}

pub fn validate(raw: &Value, version: u64) -> Result<()> {
    if version >= 11 {
        let _ = plan(raw)?;
    }
    Ok(())
}

const GRAPHIC_TEXTURE_EDGE: u32 = 1024;

struct GraphicOverlay {
    slot_index: usize,
    projection: String,
    axis: String,
    wrap: super::model::WrapMode,
    blend_mode: Option<super::model::GraphicBlend>,
    metallic: Option<f32>,
    roughness: Option<f32>,
    island_triangles: Option<BTreeMap<usize, BTreeSet<usize>>>,
    pixels: image::RgbaImage,
}

pub(crate) fn filtered_source_document(
    raw: &Value,
    page_id: &str,
    consumer_page: Option<&str>,
    consumer_id: &str,
    source: &Value,
) -> Result<Value> {
    let kind = source
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    fn has_node(nodes: &[Value], id: &str) -> bool {
        nodes.iter().any(|node| {
            node.get("id").and_then(Value::as_str) == Some(id)
                || node
                    .get("children")
                    .and_then(Value::as_array)
                    .is_some_and(|children| has_node(children, id))
        })
    }
    let consumer_page = consumer_page.or_else(|| {
        raw.get("pages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|page| {
                page.get("layers")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|layer| layer.get("nodes").and_then(Value::as_array))
                    .any(|nodes| has_node(nodes, consumer_id))
            })
            .and_then(|page| page.get("id"))
            .and_then(Value::as_str)
    });
    fn owner_dieline(node: &Value, id: &str) -> Option<String> {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return node
                .get("dieline")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        node.get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|child| owner_dieline(child, id))
    }
    let consumer_dieline = if consumer_page == Some(page_id) {
        raw.get("pages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
            .and_then(|page| page.get("layers"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|layer| layer.get("nodes"))
            .filter_map(Value::as_array)
            .flatten()
            .find_map(|node| owner_dieline(node, consumer_id))
    } else {
        None
    };
    let mut filtered = raw.clone();
    if let Some(dieline_id) = consumer_dieline.as_deref() {
        if let Some(dielines) = filtered.get_mut("dielines").and_then(Value::as_object_mut) {
            dielines.remove(dieline_id);
        }
    }
    let pages = filtered
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .context("[invalid-input] document pages are missing")?;
    let page = pages
        .iter_mut()
        .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
        .with_context(|| {
            format!("[missing-resource] graphic source page `{page_id}` is missing")
        })?;
    let mut found;
    fn contains(node: &Value, id: &str) -> bool {
        node.get("id").and_then(Value::as_str) == Some(id)
            || node
                .get("children")
                .and_then(Value::as_array)
                .is_some_and(|children| children.iter().any(|child| contains(child, id)))
    }
    fn trim_node(node: &mut Value, id: &str) -> bool {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return true;
        }
        node.get_mut("children")
            .and_then(Value::as_array_mut)
            .is_some_and(|children| trim_siblings(children, id))
    }
    fn trim_siblings(nodes: &mut Vec<Value>, id: &str) -> bool {
        let selected: Vec<bool> = nodes.iter().map(|node| contains(node, id)).collect();
        if !selected.iter().any(|keep| *keep) {
            return false;
        }
        for (node, keep) in nodes.iter_mut().zip(&selected) {
            if *keep {
                trim_node(node, id);
            }
        }

        // A selected clipped node depends on the alpha of its sibling base.
        // Keep that base (and any base it itself depends on) in original order
        // so the shared renderer sees the same clipping stack as the source.
        let mut required = std::collections::BTreeSet::<String>::new();
        for (node, keep) in nodes.iter().zip(&selected) {
            if *keep {
                if let Some(base) = node
                    .get("clipping")
                    .and_then(|clipping| clipping.get("base"))
                    .and_then(Value::as_str)
                {
                    required.insert(base.to_owned());
                }
            }
        }
        loop {
            let prior_len = required.len();
            let dependencies: Vec<String> = nodes
                .iter()
                .filter(|node| {
                    node.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|node_id| required.contains(node_id))
                })
                .filter_map(|node| {
                    node.get("clipping")
                        .and_then(|clipping| clipping.get("base"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect();
            required.extend(dependencies);
            if required.len() == prior_len {
                break;
            }
        }
        let retained = nodes
            .drain(..)
            .enumerate()
            .filter_map(|(index, node)| {
                (selected[index]
                    || node
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|node_id| required.contains(node_id)))
                .then_some(node)
            })
            .collect();
        *nodes = retained;
        true
    }
    fn remove_from(node: &mut Value, id: &str) -> bool {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return false;
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            children.retain_mut(|child| remove_from(child, id));
        }
        true
    }
    if kind == "node" {
        let id = source
            .get("node")
            .and_then(Value::as_str)
            .context("[invalid-input] node source has no node ID")?;
        found = false;
        for layer in page
            .get_mut("layers")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                found |= trim_siblings(nodes, id);
            }
        }
        ensure!(
            found,
            "[missing-resource] node source {page_id}/{id} was not found while preparing its texture"
        );
    }
    if consumer_page == Some(page_id) {
        for layer in page
            .get_mut("layers")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                nodes.retain_mut(|node| remove_from(node, consumer_id));
            }
        }
    }
    ensure!(
        page.get("canvas").is_some(),
        "[malformed-resource] graphic source page has no canvas"
    );
    Ok(filtered)
}

fn source_page(
    raw: &Value,
    document_dir: &Path,
    page_id: &str,
    consumer_page: Option<&str>,
    consumer_id: &str,
    source: &Value,
) -> Result<image::RgbaImage> {
    let filtered = filtered_source_document(raw, page_id, consumer_page, consumer_id, source)?;
    let canvas = filtered
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))
        .and_then(|page| page.get("canvas"))
        .context("[malformed-resource] graphic source page has no canvas")?;
    let width = canvas
        .get("width")
        .and_then(Value::as_u64)
        .context("[malformed-resource] source page width is invalid")?;
    let height = canvas
        .get("height")
        .and_then(Value::as_u64)
        .context("[malformed-resource] source page height is invalid")?;
    let longest = width.max(height).max(1) as f32;
    let scale = (4096.0 / longest).min(1.0);
    crate::composite::render_content(&filtered, document_dir, Some(page_id), scale)
        .context("[unsupported-capability] failed to rasterize the linked artwork source")
}

fn crop_source(image: &image::RgbaImage, crop: Option<&Value>) -> Result<image::RgbaImage> {
    let Some(crop) = crop else {
        return Ok(image.clone());
    };
    let rect = crop
        .as_array()
        .filter(|v| v.len() == 4)
        .context("[invalid-input] graphic crop must be [x,y,width,height]")?;
    let values: Vec<f64> = rect
        .iter()
        .map(|v| {
            v.as_f64()
                .context("[invalid-input] graphic crop values must be numbers")
        })
        .collect::<Result<_>>()?;
    ensure!(
        values.iter().all(|v| v.is_finite())
            && values[0] >= 0.0
            && values[1] >= 0.0
            && values[2] > 0.0
            && values[3] > 0.0
            && values[0] + values[2] <= 1.0
            && values[1] + values[3] <= 1.0,
        "[invalid-input] graphic crop must stay within normalized 0..=1 bounds"
    );
    let x = (values[0] * image.width() as f64).round() as u32;
    let y = (values[1] * image.height() as f64).round() as u32;
    ensure!(
        x < image.width() && y < image.height(),
        "[invalid-input] graphic crop rounds outside the source image"
    );
    let w = ((values[2] * image.width() as f64).round() as u32)
        .max(1)
        .min(image.width() - x);
    let h = ((values[3] * image.height() as f64).round() as u32)
        .max(1)
        .min(image.height() - y);
    Ok(image::imageops::crop_imm(image, x, y, w, h).to_image())
}

fn fit_source(
    source: &image::RgbaImage,
    width: u32,
    height: u32,
    fit: &str,
) -> Result<(image::RgbaImage, u32, u32)> {
    use image::imageops::FilterType;
    let (sw, sh) = (source.width().max(1), source.height().max(1));
    if fit == "stretch" {
        return Ok((
            image::imageops::resize(source, width, height, FilterType::Lanczos3),
            0,
            0,
        ));
    }
    ensure!(
        matches!(fit, "contain" | "cover"),
        "[unsupported-capability] graphic fit `{fit}` is not implemented"
    );
    let ratio = if fit == "contain" {
        (width as f64 / sw as f64).min(height as f64 / sh as f64)
    } else {
        (width as f64 / sw as f64).max(height as f64 / sh as f64)
    };
    let rw = ((sw as f64 * ratio).round() as u32).max(1);
    let rh = ((sh as f64 * ratio).round() as u32).max(1);
    let resized = image::imageops::resize(source, rw, rh, FilterType::Lanczos3);
    if fit == "contain" {
        return Ok((resized, (width - rw) / 2, (height - rh) / 2));
    }
    let crop_x = (rw - width) / 2;
    let crop_y = (rh - height) / 2;
    Ok((
        image::imageops::crop_imm(&resized, crop_x, crop_y, width, height).to_image(),
        0,
        0,
    ))
}

fn rotate_image(source: &image::RgbaImage, degrees: f64) -> Result<image::RgbaImage> {
    use image::Rgba;
    ensure!(
        degrees.is_finite() && degrees.abs() <= 360.0,
        "[invalid-input] graphic rotation must be within -360..=360 degrees"
    );
    if degrees.rem_euclid(360.0) == 0.0 {
        return Ok(source.clone());
    }
    let mut out = image::RgbaImage::from_pixel(source.width(), source.height(), Rgba([0, 0, 0, 0]));
    let radians = degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    let (cx, cy) = (
        (f64::from(source.width()) - 1.0) / 2.0,
        (f64::from(source.height()) - 1.0) / 2.0,
    );
    for y in 0..out.height() {
        for x in 0..out.width() {
            let dx = f64::from(x) - cx;
            let dy = f64::from(y) - cy;
            let sx = cos * dx + sin * dy + cx;
            let sy = -sin * dx + cos * dy + cy;
            if sx >= 0.0
                && sy >= 0.0
                && sx < f64::from(source.width())
                && sy < f64::from(source.height())
            {
                let sample_x = (sx.round() as u32).min(source.width() - 1);
                let sample_y = (sy.round() as u32).min(source.height() - 1);
                *out.get_pixel_mut(x, y) = *source.get_pixel(sample_x, sample_y);
            }
        }
    }
    Ok(out)
}

/// Rasterize linked artwork and append transparent overlay geometry for each
/// target slot. Source documents remain untouched; textures are render-time data.
pub fn apply_graphics(
    raw: &Value,
    document_dir: &Path,
    consumer_page: Option<&str>,
    node: &serde_json::Map<String, Value>,
    model: &mut super::model::Model,
) -> Result<String> {
    use super::model::{
        AlphaMode, GraphicBlend, Instance, TexRef, Texture, TextureProjection, WrapMode,
    };
    use image::{imageops, Rgba, RgbaImage};
    let Some(graphics) = node.get("surface_graphics").and_then(Value::as_array) else {
        return Ok(crate::resource::sha256(b"no-surface-graphics"));
    };
    if graphics.is_empty() {
        return Ok(crate::resource::sha256(b"no-surface-graphics"));
    }
    let mut resolved_content = Sha256::new();
    let mut overlays: Vec<GraphicOverlay> = Vec::new();
    let base_texture_bytes: usize = model.textures.iter().map(|t| t.rgba.len()).sum();
    let overlay_texture_bytes = (GRAPHIC_TEXTURE_EDGE as usize)
        .checked_mul(GRAPHIC_TEXTURE_EDGE as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("surface texture byte count overflow")?;
    for graphic in graphics {
        let graphic_id = graphic.get("id").and_then(Value::as_str).unwrap_or("?");
        let projection = graphic
            .get("projection")
            .and_then(Value::as_str)
            .unwrap_or("uv")
            .to_owned();
        let axis = graphic
            .get("axis")
            .and_then(Value::as_str)
            .unwrap_or("z")
            .to_owned();
        ensure!(
            matches!(projection.as_str(), "uv" | "planar" | "box" | "cylindrical" | "spherical" | "triplanar"),
            "[unsupported-capability] surface graphic {graphic_id}: projection `{projection}` is not currently rendered"
        );
        ensure!(
            graphic.get("axis").is_none() || matches!(axis.as_str(), "x" | "y" | "z"),
            "[invalid-input] surface graphic {graphic_id} projection axis must be x, y or z"
        );
        ensure!(
            projection != "triplanar" || graphic.get("axis").is_none(),
            "[invalid-input] surface graphic {graphic_id} triplanar projection does not use an axis"
        );
        let wrap = match graphic
            .get("wrap")
            .and_then(Value::as_str)
            .unwrap_or("clamp")
        {
            "clamp" => WrapMode::Clamp,
            "repeat" => WrapMode::Repeat,
            "mirror" => WrapMode::Mirror,
            other => bail!(
                "[invalid-input] surface graphic {graphic_id} wrap `{other}` must be clamp, repeat or mirror"
            ),
        };
        let blend_mode = match graphic
            .get("blend")
            .and_then(Value::as_str)
            .unwrap_or("normal")
        {
            "normal" => None,
            "multiply" => Some(GraphicBlend::Multiply),
            "screen" => Some(GraphicBlend::Screen),
            "overlay" => Some(GraphicBlend::Overlay),
            "darken" => Some(GraphicBlend::Darken),
            "lighten" => Some(GraphicBlend::Lighten),
            other => bail!(
                "[invalid-input] surface graphic {graphic_id} has unknown blend mode `{other}`"
            ),
        };
        let source = graphic
            .get("source")
            .context("[invalid-input] surface graphic source is missing")?;
        let source_page_id = source
            .get("page")
            .and_then(Value::as_str)
            .context("[invalid-input] surface graphic source page is missing")?;
        let input = source_page(
            raw,
            document_dir,
            source_page_id,
            consumer_page,
            node.get("id").and_then(Value::as_str).unwrap_or("?"),
            source,
        )?;
        let input = crop_source(&input, graphic.get("crop"))?;
        let mode = graphic
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("decal");
        let scale = graphic
            .get("scale")
            .and_then(Value::as_array)
            .filter(|v| v.len() == 2)
            .map(|v| (v[0].as_f64().unwrap_or(1.0), v[1].as_f64().unwrap_or(1.0)))
            .unwrap_or((1.0, 1.0));
        let (ox, oy, rw, rh) = if mode == "fill" {
            ensure!(
                scale.0.is_finite()
                    && scale.1.is_finite()
                    && scale.0 >= 1.0
                    && scale.1 >= 1.0
                    && scale.0 <= 16.0
                    && scale.1 <= 16.0
                    && scale.0 * scale.1 <= 256.0,
                "[invalid-input] fill scale for graphic {graphic_id} is a bounded repeat count (1..=16 per axis, at most 256 tiles)"
            );
            (
                0.0,
                0.0,
                (f64::from(GRAPHIC_TEXTURE_EDGE) / scale.0).round().max(1.0) as u32,
                (f64::from(GRAPHIC_TEXTURE_EDGE) / scale.1).round().max(1.0) as u32,
            )
        } else {
            let offset = graphic
                .get("offset")
                .and_then(Value::as_array)
                .filter(|v| v.len() == 2)
                .map(|v| (v[0].as_f64().unwrap_or(0.5), v[1].as_f64().unwrap_or(0.5)))
                .unwrap_or((0.5, 0.5));
            ensure!(
                scale.0.is_finite()
                    && scale.1.is_finite()
                    && scale.0 > 0.0
                    && scale.1 > 0.0
                    && scale.0 <= 1.0
                    && scale.1 <= 1.0
                    && offset.0.is_finite()
                    && offset.1.is_finite(),
                "[invalid-input] surface graphic {graphic_id} scale/offset must define a bounded normalized UV region"
            );
            let w = ((scale.0 * f64::from(GRAPHIC_TEXTURE_EDGE)).round() as u32).max(1);
            let h = ((scale.1 * f64::from(GRAPHIC_TEXTURE_EDGE)).round() as u32).max(1);
            let x = ((offset.0 - scale.0 / 2.0) * f64::from(GRAPHIC_TEXTURE_EDGE)).round();
            let y = ((offset.1 - scale.1 / 2.0) * f64::from(GRAPHIC_TEXTURE_EDGE)).round();
            ensure!(
                x >= 0.0
                    && y >= 0.0
                    && x + f64::from(w) <= f64::from(GRAPHIC_TEXTURE_EDGE)
                    && y + f64::from(h) <= f64::from(GRAPHIC_TEXTURE_EDGE),
                "[invalid-input] surface graphic {graphic_id} UV region extends beyond 0..=1"
            );
            (x, y, w, h)
        };
        let fit = graphic
            .get("fit")
            .and_then(Value::as_str)
            .unwrap_or("contain");
        let (mut placed, dx, dy) = fit_source(&input, rw, rh, fit)?;
        placed = rotate_image(
            &placed,
            graphic.get("rotate").and_then(Value::as_f64).unwrap_or(0.0),
        )?;
        let material = graphic.get("material");
        let material_opacity = material
            .and_then(|material| material.get("opacity"))
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let opacity = graphic
            .get("opacity")
            .and_then(Value::as_f64)
            .unwrap_or(1.0)
            * material_opacity;
        ensure!(
            opacity.is_finite() && (0.0..=1.0).contains(&opacity),
            "[invalid-input] surface graphic opacity must be within 0..=1"
        );
        if opacity < 1.0 {
            for pixel in placed.pixels_mut() {
                pixel[3] = (f64::from(pixel[3]) * opacity).round() as u8;
            }
        }
        let slot_name = graphic
            .get("slot")
            .and_then(Value::as_str)
            .context("[invalid-input] surface graphic slot is missing")?;
        let slot_index = model.materials.iter().position(|material| material.name == slot_name)
            .with_context(|| format!("[missing-resource] surface graphic {graphic_id} targets unknown material slot `{slot_name}`"))?;
        let island_triangles = if let Some(island_id) =
            graphic.get("island").and_then(Value::as_str)
        {
            let source = if let Some(digest) = node.get("model").and_then(Value::as_str) {
                raw.get("models").and_then(|models| models.get(digest))
            } else if let Some(form_id) = node.get("product_form").and_then(Value::as_str) {
                raw.get("product_forms")
                    .and_then(|forms| forms.get(form_id))
            } else {
                None
            };
            let overlay_map = source
                .and_then(|source| source.get("uv"))
                .and_then(Value::as_object)
                .with_context(|| format!("[missing-resource] surface graphic {graphic_id} on consumer `{}` in slot `{slot_name}` targets UV island `{island_id}`, but its model has no editable UV island data; create the island in the UV workspace or clear the island target", node.get("id").and_then(Value::as_str).unwrap_or("?")))?;
            let mut matches = BTreeMap::<usize, BTreeSet<usize>>::new();
            for (key, spec) in overlay_map {
                let applicable: Vec<usize> = if key == "*" {
                    (0..model.primitives.len()).collect()
                } else if let Ok(index) = key.parse::<usize>() {
                    vec![index]
                } else {
                    Vec::new()
                };
                for primitive in applicable
                    .into_iter()
                    .filter(|index| *index < model.primitives.len())
                {
                    for island in spec
                        .get("islands")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|island| {
                            island.get("id").and_then(Value::as_str) == Some(island_id)
                                && island.get("uv_set").and_then(Value::as_u64) == Some(0)
                        })
                    {
                        let triangles = island
                            .get("triangles")
                            .and_then(Value::as_array)
                            .context("[invalid-input] UV island has no triangle list")?;
                        let selected = matches.entry(primitive).or_default();
                        for triangle in triangles {
                            let triangle = triangle.as_u64().context(
                                "[invalid-input] UV island triangle must be a non-negative index",
                            )? as usize;
                            ensure!(triangle < model.primitives[primitive].triangle_count(), "[invalid-input] UV island `{island_id}` references missing triangle {triangle} in primitive {primitive}");
                            selected.insert(triangle);
                        }
                    }
                }
            }
            ensure!(!matches.is_empty(), "[missing-resource] surface graphic {graphic_id} on consumer `{}` in slot `{slot_name}` targets unknown UV-set-0 island `{island_id}`; choose an existing island from the UV workspace or clear the island target", node.get("id").and_then(Value::as_str).unwrap_or("?"));
            Some(matches)
        } else {
            None
        };
        let total_overlay_bytes = (overlays.len() + 1)
            .checked_mul(overlay_texture_bytes)
            .context("surface texture byte count overflow")?;
        ensure!(
            base_texture_bytes
                .checked_add(total_overlay_bytes)
                .is_some_and(|n| n <= super::model::LIMITS.max_texture_bytes),
            "[limit-exceeded] bound surface textures exceed the {} byte limit",
            super::model::LIMITS.max_texture_bytes
        );
        ensure!(
            model.textures.len() + overlays.len() < super::model::LIMITS.max_textures,
            "[limit-exceeded] rendered model exceeds the texture count limit"
        );
        let mut canvas = RgbaImage::from_pixel(
            GRAPHIC_TEXTURE_EDGE,
            GRAPHIC_TEXTURE_EDGE,
            Rgba([0, 0, 0, 0]),
        );
        if mode == "fill" {
            let offset = graphic
                .get("offset")
                .and_then(Value::as_array)
                .filter(|v| v.len() == 2)
                .map(|v| (v[0].as_f64().unwrap_or(0.0), v[1].as_f64().unwrap_or(0.0)))
                .unwrap_or((0.0, 0.0));
            ensure!(
                offset.0.is_finite()
                    && offset.1.is_finite()
                    && (0.0..=1.0).contains(&offset.0)
                    && (0.0..=1.0).contains(&offset.1),
                "[invalid-input] fill offset for graphic {graphic_id} is a normalized tile phase within 0..=1"
            );
            let start_x = -((offset.0 * f64::from(rw)).round() as i64);
            let start_y = -((offset.1 * f64::from(rh)).round() as i64);
            let mut row = 0usize;
            let mut y = start_y;
            while y < i64::from(GRAPHIC_TEXTURE_EDGE) {
                let mut column = 0usize;
                let mut x = start_x;
                while x < i64::from(GRAPHIC_TEXTURE_EDGE) {
                    let tile = if wrap == WrapMode::Mirror && (row + column) % 2 == 1 {
                        imageops::flip_horizontal(&placed)
                    } else {
                        placed.clone()
                    };
                    imageops::overlay(&mut canvas, &tile, x + dx as i64, y + dy as i64);
                    x += i64::from(rw);
                    column += 1;
                }
                y += i64::from(rh);
                row += 1;
            }
        } else {
            imageops::overlay(
                &mut canvas,
                &placed,
                ox as i64 + dx as i64,
                oy as i64 + dy as i64,
            );
        }
        resolved_content.update(canvas.as_raw());
        let metallic = material
            .and_then(|material| material.get("metallic"))
            .and_then(Value::as_f64)
            .map(|value| value as f32);
        let roughness = material
            .and_then(|material| material.get("roughness"))
            .and_then(Value::as_f64)
            .map(|value| value as f32);
        overlays.push(GraphicOverlay {
            slot_index,
            projection,
            axis,
            wrap,
            blend_mode,
            metallic,
            roughness,
            island_triangles,
            pixels: canvas,
        });
    }
    let (min, max) = model.bounds().unwrap_or(([-1.0; 3], [1.0; 3]));
    let epsilon = ((0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(0.0_f64, f64::max)
        * 0.0005)
        .max(0.001);
    for GraphicOverlay {
        slot_index,
        projection,
        axis,
        wrap,
        blend_mode,
        metallic,
        roughness,
        island_triangles,
        pixels,
    } in overlays
    {
        let texture_index = model.textures.len();
        model.textures.push(Texture {
            name: format!("surface-{}", model.materials[slot_index].name),
            width: GRAPHIC_TEXTURE_EDGE,
            height: GRAPHIC_TEXTURE_EDGE,
            rgba: pixels.into_raw(),
        });
        let mut overlay_material = model.materials[slot_index].clone();
        overlay_material.name = format!("{}-surface-overlay", overlay_material.name);
        overlay_material.base_color = [1.0; 4];
        if let Some(metallic) = metallic {
            overlay_material.metallic = metallic;
        }
        if let Some(roughness) = roughness {
            overlay_material.roughness = roughness;
        }
        overlay_material.base_color_texture = Some(TexRef {
            texture: texture_index,
            uv_set: 0,
            wrap,
            projection: if projection == "triplanar" {
                TextureProjection::Triplanar
            } else {
                TextureProjection::Uv
            },
        });
        overlay_material.alpha_mode = blend_mode.map_or(AlphaMode::Blend, AlphaMode::GraphicBlend);
        let overlay_material_index = model.materials.len();
        model.materials.push(overlay_material);
        let has_slot_geometry = model
            .primitives
            .iter()
            .any(|primitive| primitive.material == Some(slot_index));
        let targets: Vec<usize> = model
            .primitives
            .iter()
            .enumerate()
            .filter_map(|(index, primitive)| {
                if island_triangles
                    .as_ref()
                    .is_none_or(|islands| islands.contains_key(&index))
                    && (primitive.material == Some(slot_index)
                        || (!has_slot_geometry
                            && primitive.name == "lathed-body"
                            && primitive.material.is_some()))
                {
                    Some(index)
                } else {
                    None
                }
            })
            .collect();
        ensure!(
            !targets.is_empty(),
            "[missing-resource] material slot `{}` has no UV-mapped surface",
            model.materials[slot_index].name
        );
        ensure!(
            model.materials.len() < super::model::LIMITS.max_materials,
            "[limit-exceeded] bound surface overlays exceed the material count limit"
        );
        let extra_instances = targets
            .iter()
            .map(|primitive| {
                model
                    .instances
                    .iter()
                    .filter(|instance| instance.primitive == *primitive)
                    .count()
            })
            .sum::<usize>();
        let extra_triangles = targets
            .iter()
            .map(|primitive| {
                island_triangles
                    .as_ref()
                    .and_then(|islands| islands.get(primitive))
                    .map_or(model.primitives[*primitive].triangle_count(), BTreeSet::len)
                    .saturating_mul(
                        model
                            .instances
                            .iter()
                            .filter(|instance| instance.primitive == *primitive)
                            .count(),
                    )
            })
            .sum::<usize>();
        ensure!(
            model
                .instances
                .len()
                .checked_add(extra_instances)
                .is_some_and(|n| n <= super::model::LIMITS.max_instances),
            "[limit-exceeded] surface overlays exceed the {} instance limit",
            super::model::LIMITS.max_instances
        );
        ensure!(
            model
                .rendered_triangles()
                .checked_add(extra_triangles)
                .is_some_and(|n| n <= super::model::LIMITS.max_triangles),
            "[limit-exceeded] surface overlays exceed the {} rendered-triangle limit",
            super::model::LIMITS.max_triangles
        );
        ensure!(
            model
                .primitives
                .len()
                .checked_add(targets.len())
                .is_some_and(|n| n <= super::model::LIMITS.max_primitives),
            "[limit-exceeded] surface overlays exceed the {} primitive limit",
            super::model::LIMITS.max_primitives
        );
        for primitive_index in targets {
            let mut primitive = model.primitives[primitive_index].clone();
            if let Some(island_triangles) = &island_triangles {
                let selected = island_triangles.get(&primitive_index).with_context(|| {
                    format!("[missing-resource] surface graphic targets island triangles outside material slot `{}`", model.materials[slot_index].name)
                })?;
                primitive.indices = primitive
                    .indices
                    .chunks_exact(3)
                    .enumerate()
                    .filter(|(triangle, _)| selected.contains(triangle))
                    .flat_map(|(_, vertices)| vertices.iter().copied())
                    .collect();
                if primitive.indices.is_empty() {
                    continue;
                }
            }
            if projection != "uv" && projection != "triplanar" {
                super::uv::project_primitive(&mut primitive, &projection, &axis)
                    .with_context(|| format!("surface graphic {projection} projection"))?;
            }
            ensure!(
                projection == "triplanar"
                    || primitive
                        .uvs
                        .first()
                        .is_some_and(|uv| uv.len() == primitive.positions.len()),
                "[missing-resource] surface graphic material `{}` requires UV coordinates on every vertex",
                model.materials[slot_index].name
            );
            for (position, normal) in primitive.positions.iter_mut().zip(&primitive.normals) {
                let length = (f64::from(normal[0]).powi(2)
                    + f64::from(normal[1]).powi(2)
                    + f64::from(normal[2]).powi(2))
                .sqrt()
                .max(1e-9);
                for axis in 0..3 {
                    position[axis] += (f64::from(normal[axis]) / length * epsilon) as f32;
                }
            }
            primitive.name = format!("{}-surface-overlay", primitive.name);
            primitive.material = Some(overlay_material_index);
            let overlay_primitive = model.primitives.len();
            model.primitives.push(primitive);
            let instances: Vec<Instance> = model
                .instances
                .iter()
                .filter(|instance| instance.primitive == primitive_index)
                .map(|instance| Instance {
                    id: format!("{}-surface-overlay", instance.id),
                    primitive: overlay_primitive,
                    transform: instance.transform,
                })
                .collect();
            model.instances.extend(instances);
        }
    }
    Ok(format!(
        "sha256:{}",
        hex::encode(resolved_content.finalize())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document(nodes: Value) -> Value {
        json!({"version":11,"pages":[
            {"id":"art","layers":[{"nodes":[{"id":"logo","kind":"rect"}]}]},
            {"id":"mockups","layers":[{"nodes":nodes}]}
        ]})
    }

    fn scene(id: &str, graphics: Value) -> Value {
        json!({"kind":"scene3d","id":id,"surface_graphics":graphics})
    }

    #[test]
    fn triplanar_surface_binding_renders_without_source_uvs() {
        let raw: Value = serde_json::from_str(include_str!(
            "../../docs/fixtures/v11-product-packaging.pen"
        ))
        .unwrap();
        let node = json!({"id":"triplanar-preview","width":96,"height":96,
        "product_form":"perfume-bottle","surface_graphics":[{
            "id":"label","source":{"kind":"page","page":"artwork"},
            "slot":"label-band","mode":"decal","projection":"triplanar",
            "fit":"stretch","scale":[1,1],"offset":[0.5,0.5],
            "material":{"metallic":0.35,"roughness":0.22,"opacity":0.6}
        }]});
        let form = raw["product_forms"]["perfume-bottle"].clone();
        let mut model = super::super::form::generate(&form).unwrap();
        for primitive in &mut model.primitives {
            primitive.uvs.clear();
        }
        apply_graphics(
            &raw,
            std::path::Path::new("."),
            Some("products"),
            node.as_object().unwrap(),
            &mut model,
        )
        .unwrap();
        let overlay = model.materials.last().unwrap();
        assert_eq!(
            overlay.base_color_texture.as_ref().unwrap().projection,
            super::super::model::TextureProjection::Triplanar
        );
        assert!((overlay.metallic - 0.35).abs() < 1e-6);
        assert!((overlay.roughness - 0.22).abs() < 1e-6);
        assert!(model
            .textures
            .last()
            .unwrap()
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel[3] > 0 && pixel[3] < 255));
        assert!(model
            .primitives
            .iter()
            .any(|primitive| primitive.uvs.is_empty()));
        let (render_scene, options) =
            super::super::node::scene_for(node.as_object().unwrap(), Some(&model)).unwrap();
        let image = super::super::render::render(&model, &render_scene, &options).unwrap();
        assert!(image.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));
        let invalid_axis = document(json!([scene(
            "consumer",
            json!([{"id":"g","source":{"kind":"page","page":"art"},
                "slot":"label","mode":"decal","projection":"triplanar","axis":"y"}])
        )]));
        assert!(plan(&invalid_axis)
            .unwrap_err()
            .to_string()
            .contains("does not use an axis"));
    }

    #[test]
    fn uv_island_surface_graphic_duplicates_only_owned_triangles() {
        let mut raw: Value = serde_json::from_str(include_str!(
            "../../docs/fixtures/v11-product-packaging.pen"
        ))
        .unwrap();
        let form_id = "perfume-bottle";
        let generated = super::super::form::generate(&raw["product_forms"][form_id]).unwrap();
        let primitive_index = generated
            .primitives
            .iter()
            .position(|primitive| primitive.material.is_some())
            .unwrap();
        let slot = generated.primitives[primitive_index].material.unwrap();
        let slot_name = generated.materials[slot].name.clone();
        let total_triangles = generated.primitives[primitive_index].triangle_count();
        raw["product_forms"][form_id]["uv"][primitive_index.to_string()] = json!({
            "islands":[{"id":"front-label","uv_set":0,"triangles":[0],
                "transform":{"scale":[1,1],"offset":[0,0],"rotate":0}}]
        });
        let node = json!({"id":"island-target","width":96,"height":96,
        "product_form":form_id,"surface_graphics":[{
            "id":"front-art","source":{"kind":"page","page":"artwork"},
                "slot":slot_name,"mode":"decal","projection":"uv",
            "island":"front-label"
        }]});
        let mut model = super::super::node::load_form(&raw, form_id)
            .unwrap()
            .as_ref()
            .clone();
        apply_graphics(
            &raw,
            std::path::Path::new("."),
            Some("products"),
            node.as_object().unwrap(),
            &mut model,
        )
        .unwrap();
        assert_eq!(
            model.primitives[primitive_index].triangle_count(),
            total_triangles
        );
        let overlay_material = model.materials.len() - 1;
        let island_overlay = model
            .primitives
            .iter()
            .find(|primitive| primitive.material == Some(overlay_material))
            .unwrap();
        assert_eq!(island_overlay.triangle_count(), 1);

        let mut invalid_node = node.as_object().unwrap().clone();
        invalid_node["surface_graphics"][0]["island"] = json!("missing-island");
        let mut unchanged_model = super::super::node::load_form(&raw, form_id)
            .unwrap()
            .as_ref()
            .clone();
        let error = apply_graphics(
            &raw,
            std::path::Path::new("."),
            Some("products"),
            &invalid_node,
            &mut unchanged_model,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("unknown UV-set-0 island `missing-island`"));
    }

    #[test]
    fn plans_node_sources_in_stable_dependency_first_order() {
        let raw = document(json!([
            scene(
                "consumer",
                json!([{"id":"label","source":{"kind":"node","page":"art","node":"logo"},"slot":"label","mode":"decal","projection":"uv"}])
            ),
            scene("other", json!([]))
        ]));
        assert_eq!(plan(&raw).unwrap(), ["mockups/consumer", "mockups/other"]);
    }

    #[test]
    fn missing_page_and_node_sources_are_rejected() {
        let missing_page = document(json!([scene(
            "consumer",
            json!([{"id":"g","source":{"kind":"page","page":"ghost"},"slot":"label","mode":"decal","projection":"uv"}])
        )]));
        assert!(plan(&missing_page)
            .unwrap_err()
            .to_string()
            .contains("missing-resource"));
        let missing_node = document(json!([scene(
            "consumer",
            json!([{"id":"g","source":{"kind":"node","page":"art","node":"ghost"},"slot":"label","mode":"decal","projection":"uv"}])
        )]));
        assert!(plan(&missing_node)
            .unwrap_err()
            .to_string()
            .contains("missing-resource"));
    }

    #[test]
    fn direct_node_sources_must_be_visible_and_unlocked_through_ancestors() {
        let hidden = document(json!([scene(
            "consumer",
            json!([{"id":"g","source":{"kind":"node","page":"art","node":"logo"},"slot":"label","mode":"decal","projection":"uv"}])
        )]));
        let mut hidden = hidden;
        hidden["pages"][0]["layers"][0]["nodes"][0]["visible"] = json!(false);
        assert!(plan(&hidden).unwrap_err().to_string().contains("is hidden"));

        let mut locked = document(json!([scene(
            "consumer",
            json!([{"id":"g","source":{"kind":"node","page":"art","node":"logo"},"slot":"label","mode":"decal","projection":"uv"}])
        )]));
        locked["pages"][0]["layers"][0]["locked"] = json!(true);
        assert!(plan(&locked).unwrap_err().to_string().contains("is locked"));

        let mut hidden_ancestor = document(json!([scene(
            "consumer",
            json!([{"id":"g","source":{"kind":"node","page":"art","node":"logo"},"slot":"label","mode":"decal","projection":"uv"}])
        )]));
        hidden_ancestor["pages"][0]["layers"][0]["nodes"] = json!([{
            "id":"group", "kind":"group", "visible":false,
            "children":[{"id":"logo", "kind":"rect"}]
        }]);
        assert!(plan(&hidden_ancestor)
            .unwrap_err()
            .to_string()
            .contains("is hidden"));
    }

    #[test]
    fn rejects_node_dependency_cycles() {
        let raw = document(json!([
            scene(
                "a",
                json!([{"id":"ab","source":{"kind":"node","page":"mockups","node":"b"},"slot":"label","mode":"decal","projection":"uv"}])
            ),
            scene(
                "b",
                json!([{"id":"ba","source":{"kind":"node","page":"mockups","node":"a"},"slot":"label","mode":"decal","projection":"uv"}])
            )
        ]));
        assert!(plan(&raw)
            .unwrap_err()
            .to_string()
            .contains("dependency-cycle"));
    }

    #[test]
    fn v10_binding_validation_is_unchanged() {
        let mut raw = document(json!([]));
        raw["version"] = json!(10);
        assert!(validate(&raw, 10).is_ok());
    }

    #[test]
    fn nested_clipped_node_sources_keep_their_ancestor_transform_and_clip_base() {
        let mut raw: Value = serde_json::from_str(include_str!(
            "../../docs/fixtures/v11-product-packaging.pen"
        ))
        .unwrap();
        raw["pages"][0]["layers"][0]["nodes"] = json!([{
            "kind":"group","id":"label-group","transform":[1,0,0,1,20,10],
            "children":[
                {"kind":"rect","id":"clip-base","x":0,"y":0,"width":30,"height":30,
                    "style":{"fill":{"fallback":"#0000ff"}}},
                {"kind":"rect","id":"label-ink","x":0,"y":0,"width":60,"height":60,
                    "clipping":{"base":"clip-base"},"style":{"fill":{"fallback":"#ff0000"}}},
                {"kind":"rect","id":"unrelated","x":100,"y":0,"width":20,"height":20,
                    "style":{"fill":{"fallback":"#00ff00"}}}
            ]
        }]);
        let filtered = filtered_source_document(
            &raw,
            "artwork",
            Some("products"),
            "perfume-preview",
            &json!({"kind":"node","page":"artwork","node":"label-ink"}),
        )
        .unwrap();
        let group = &filtered["pages"][0]["layers"][0]["nodes"][0];
        assert_eq!(group["transform"], json!([1, 0, 0, 1, 20, 10]));
        let children = group["children"].as_array().unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0]["id"], "clip-base");
        assert_eq!(children[1]["id"], "label-ink");
        assert_eq!(children[1]["clipping"]["base"], "clip-base");

        let rendered = crate::composite::render_content(
            &filtered,
            std::path::Path::new("."),
            Some("artwork"),
            1.0,
        )
        .unwrap();
        assert_eq!(rendered.get_pixel(30, 20).0, [255, 0, 0, 255]);
        assert_eq!(rendered.get_pixel(55, 45).0[3], 0);
    }
}

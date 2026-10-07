//! Orientation and page-wide composition: rotate, flip, merge visible, stamp
//! visible and flatten.
//!
//! Like the other whole-layer operations these roll the checkpoint of every layer
//! they rewrite (reason in `checkpoint.reason`), run on a clone of the document and
//! replace it only on success. Rotation is limited to quarter turns and flips are
//! mirror images, so both are lossless; the layer keeps its top-left position.
use super::layer::{commit_surface, find_path, merge_down, siblings_mut};
use super::shape::ensure_plane;
use super::*;
use std::path::Path;

pub const ALGORITHM: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orient {
    Rotate90,
    Rotate180,
    Rotate270,
    FlipHorizontal,
    FlipVertical,
}

impl Orient {
    pub fn rotation(degrees: i64) -> Result<Self> {
        match degrees.rem_euclid(360) {
            90 => Ok(Self::Rotate90),
            180 => Ok(Self::Rotate180),
            270 => Ok(Self::Rotate270),
            _ => bail!("[invalid-input] rotation must be 90, 180 or 270 degrees clockwise"),
        }
    }

    pub fn flip(axis: &str) -> Result<Self> {
        match axis {
            "horizontal" => Ok(Self::FlipHorizontal),
            "vertical" => Ok(Self::FlipVertical),
            other => bail!("[invalid-input] axis {other:?} must be horizontal or vertical"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Rotate90 => "rotate-90",
            Self::Rotate180 => "rotate-180",
            Self::Rotate270 => "rotate-270",
            Self::FlipHorizontal => "flip-horizontal",
            Self::FlipVertical => "flip-vertical",
        }
    }
}

/// The surface turned or mirrored; every pixel keeps its exact value.
pub(super) fn reorient(source: &Surface, how: Orient) -> Surface {
    let (w, h) = (source.width, source.height);
    let mut out = match how {
        Orient::Rotate90 | Orient::Rotate270 => Surface::empty(h, w),
        _ => Surface::empty(w, h),
    };
    source.for_each_painted(|x, y, p| {
        let (nx, ny) = match how {
            Orient::Rotate90 => (h - 1 - y, x),
            Orient::Rotate180 => (w - 1 - x, h - 1 - y),
            Orient::Rotate270 => (y, w - 1 - x),
            Orient::FlipHorizontal => (w - 1 - x, y),
            Orient::FlipVertical => (x, h - 1 - y),
        };
        out.set_pixel(nx, ny, p);
    });
    out.tiles.retain(|_, tile| !is_blank(tile));
    out
}

/// Rotate or flip a raster layer's pixels, and its active and saved selections.
pub fn orient(raw: &mut Value, page: Option<&str>, id: &str, how: Orient) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let mut next = raw.clone();
    let node = locate(&mut next, page, id)?.clone();
    if !is_raster(&node) {
        bail!("[invalid-input] {id} is not a raster layer")
    }
    let surface = Surface::load(&next, &node)?;
    ensure_plane(surface.width, surface.height)?;
    let turned = reorient(&surface, how);
    // Selections follow the pixels so they keep describing the same content.
    let mut entries = Vec::new();
    if let Some(entry) = next["raster_selections"]["active"].get(id) {
        entries.push((None, entry.clone()));
    }
    if let Some(saved) = next["raster_selections"]["saved"][id].as_object() {
        entries.extend(saved.iter().map(|(k, v)| (Some(k.clone()), v.clone())));
    }
    for (name, entry) in entries {
        let plane = selection::from_json(&next, &entry)?;
        let rebuilt = selection::to_json(&mut next, &reorient(&plane, how))?;
        match name {
            None => next["raster_selections"]["active"][id] = rebuilt,
            Some(name) => next["raster_selections"]["saved"][id][name] = rebuilt,
        }
    }
    let mut node = node;
    let hash = commit_surface(&mut next, &mut node, &turned, how.name())?;
    *locate(&mut next, page, id)? = node;
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": id, "operation": how.name(), "algorithm": ALGORITHM,
        "width": turned.width, "height": turned.height,
        "tile_map_sha256": hash, "tiles_released": released,
    }))
}

/// Merge every visible raster layer that shares `id`'s sibling list into the
/// bottom-most of them. Hidden nodes between them move above the merged layer; a
/// visible node of another kind between them is refused because moving it would
/// change the picture.
pub fn merge_visible(raw: &mut Value, page: Option<&str>, id: &str) -> Result<Value> {
    let mut next = raw.clone();
    let path = find_path(&next, page, id)?;
    let list = siblings_mut(&mut next, &path)?;
    let visible = |n: &Value| n["visible"] != false;
    let rasters: Vec<usize> = (0..list.len())
        .filter(|i| is_raster(&list[*i]) && visible(&list[*i]))
        .collect();
    if !rasters.contains(path.chain.last().unwrap()) {
        bail!("[invalid-input] {id} is not a visible raster layer; merge-visible works on its visible raster siblings")
    }
    if rasters.len() < 2 {
        bail!("[invalid-input] {id} has no other visible raster layer in its group or layer to merge with")
    }
    let (first, last) = (rasters[0], *rasters.last().unwrap());
    if let Some(blocker) = (first..=last).find(|i| visible(&list[*i]) && !is_raster(&list[*i])) {
        bail!(
            "[invalid-input] visible {} {} sits between the raster layers; hide or move it, or merge with merge-down",
            list[blocker]["kind"].as_str().unwrap_or("node"),
            list[blocker]["id"].as_str().unwrap_or("?")
        )
    }
    // Gather the visible rasters contiguously; hidden nodes in between go above.
    let ids: Vec<String> = rasters
        .iter()
        .map(|i| list[*i]["id"].as_str().unwrap_or("").to_owned())
        .collect();
    let mut gathered: Vec<Value> = Vec::new();
    let mut hidden: Vec<Value> = Vec::new();
    let tail: Vec<Value> = list.drain(first..=last).collect();
    for node in tail {
        if is_raster(&node) && visible(&node) {
            gathered.push(node);
        } else {
            hidden.push(node);
        }
    }
    for (offset, node) in gathered.into_iter().chain(hidden).enumerate() {
        list.insert(first + offset, node);
    }
    let target = ids[0].clone();
    for upper in ids[1..].iter().rev() {
        merge_down(&mut next, page, upper)?;
    }
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": target, "merged": &ids[1..], "layers": ids.len(), "algorithm": ALGORITHM,
        "tiles_released": released,
    }))
}

fn page_index(raw: &Value, page: Option<&str>) -> Result<usize> {
    let pages = raw["pages"].as_array().context("document has no pages")?;
    match page {
        Some(wanted) => pages.iter().position(|p| p["id"] == wanted),
        None => (!pages.is_empty()).then_some(0),
    }
    .with_context(|| {
        format!(
            "[not-found] page {} was not found",
            page.unwrap_or("(first)")
        )
    })
}

/// Composite the visible content of a page (without the page background) into a
/// surface trimmed to its painted bounds, plus its page position.
fn stamp(raw: &Value, document: &Path, page: Option<&str>) -> Result<Option<(Surface, [u32; 4])>> {
    let index = page_index(raw, page)?;
    let canvas = &raw["pages"][index]["canvas"];
    let (w, h) = (
        canvas["width"].as_u64().unwrap_or(0) as u32,
        canvas["height"].as_u64().unwrap_or(0) as u32,
    );
    ensure_plane(w, h)?;
    let image = crate::composite::render_content(raw, document, page, 1.0)?;
    let full = Surface::from_image(&image);
    let Some([x0, y0, x1, y1]) = full.painted_bounds() else {
        return Ok(None);
    };
    let mut surface = Surface::empty(x1 - x0, y1 - y0);
    surface.blit(&full, -i64::from(x0), -i64::from(y0));
    Ok(Some((surface, [x0, y0, x1, y1])))
}

/// How a layer's flood scope or clone source samples the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Everything visible on the page, including the layer itself.
    Composite,
    /// Only content stacked beneath the layer.
    Below,
}

impl Scope {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "composite" => Ok(Self::Composite),
            "below" => Ok(Self::Below),
            other => bail!("[invalid-input] sample scope {other:?} must be composite or below"),
        }
    }
}

/// Hide the node `chain` leads to and everything stacked above it. Later siblings
/// and later layers are above earlier ones.
fn hide_from(raw: &mut Value, path: &layer::NodePath) {
    let layers = raw["pages"][path.page]["layers"].as_array_mut().unwrap();
    for upper in layers.iter_mut().skip(path.layer + 1) {
        upper["visible"] = json!(false);
    }
    let mut list = layers[path.layer]["nodes"].as_array_mut().unwrap();
    for (depth, index) in path.chain.iter().enumerate() {
        for upper in list.iter_mut().skip(index + 1) {
            upper["visible"] = json!(false);
        }
        if depth + 1 == path.chain.len() {
            list[*index]["visible"] = json!(false);
            return;
        }
        list = list[*index]["children"].as_array_mut().unwrap();
    }
}

/// The page as seen through the rectangle of raster layer `id`, in layer pixels.
/// The layer must be unrotated and unscaled so that the two grids coincide.
pub fn sample_page(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    id: &str,
    scope: Scope,
) -> Result<Surface> {
    let path = find_path(raw, page, id)?;
    let node = all_rasters(raw)
        .into_iter()
        .find(|n| n["id"] == id)
        .with_context(|| format!("[not-found] raster layer {id} was not found"))?;
    if let Some(transform) = node.get("transform") {
        if *transform != json!([1, 0, 0, 1, 0, 0])
            && *transform != json!([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        {
            bail!("[unsupported-capability] sampling the page through {id} needs an untransformed layer; its transform is {transform}")
        }
    }
    let (w, h) = (
        node["width"].as_u64().unwrap_or(0) as u32,
        node["height"].as_u64().unwrap_or(0) as u32,
    );
    ensure_plane(w, h)?;
    let (x, y) = (
        node["x"].as_f64().unwrap_or(0.0).round() as i64,
        node["y"].as_f64().unwrap_or(0.0).round() as i64,
    );
    let mut view = raw.clone();
    if scope == Scope::Below {
        hide_from(&mut view, &path);
    }
    let image = crate::composite::render_content(&view, document, page, 1.0)?;
    let full = Surface::from_image(&image);
    let mut out = Surface::empty(w, h);
    out.blit(&full, -x, -y);
    Ok(out)
}

/// Freeze what lies beneath raster layer `id` as a hidden raster `ID-below` placed
/// just under it, replacing an earlier snapshot. Use that layer as a clone source:
/// the stroke pins its tiles, so later edits cannot reinterpret the stroke.
pub fn snapshot_below(
    raw: &mut Value,
    document: &Path,
    page: Option<&str>,
    id: &str,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let sample = sample_page(raw, document, page, id, Scope::Below)?;
    let snapshot_id = format!("{id}-below");
    let mut next = raw.clone();
    let path = find_path(&next, page, id)?;
    let existing = all_rasters(&next)
        .into_iter()
        .any(|n| n["id"] == snapshot_id.as_str());
    if !existing {
        layer::ensure_new_id(&next, &snapshot_id)?;
    }
    let target = all_rasters(&next)
        .into_iter()
        .find(|n| n["id"] == id)
        .context("raster layer vanished")?
        .clone();
    let mut node = raster_node(&snapshot_id, [0, 0, sample.width, sample.height]);
    node["x"] = target["x"].clone();
    node["y"] = target["y"].clone();
    node["visible"] = json!(false);
    let hash = commit_surface(&mut next, &mut node, &sample, "snapshot-below")?;
    let list = siblings_mut(&mut next, &path)?;
    if let Some(at) = list.iter().position(|n| n["id"] == snapshot_id.as_str()) {
        list[at] = node;
    } else {
        let at = list.iter().position(|n| n["id"] == id).unwrap_or(0);
        list.insert(at, node);
    }
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": snapshot_id, "source_for": id, "algorithm": ALGORITHM,
        "tiles": sample.tiles.len(), "tile_map_sha256": hash,
    }))
}

fn raster_node(new_id: &str, bounds: [u32; 4]) -> Value {
    json!({
        "kind": "raster", "id": new_id, "engine": ENGINE,
        "x": bounds[0], "y": bounds[1],
        "width": bounds[2] - bounds[0], "height": bounds[3] - bounds[1],
        "tile_size": TILE, "pixel_format": FORMAT,
        "tiles": {}, "journal": [], "opacity": 1, "blend_mode": "normal",
        "transform": [1, 0, 0, 1, 0, 0]
    })
}

/// The unlocked layer that receives a stamped result: the topmost one.
fn top_layer(raw: &Value, index: usize) -> Result<usize> {
    let layers = raw["pages"][index]["layers"]
        .as_array()
        .context("page has no layers")?;
    let last = layers.len().checked_sub(1).context("page has no layers")?;
    if layers[last]["locked"] == true {
        bail!("[locked-node] the top layer is locked; unlock it first")
    }
    Ok(last)
}

/// Bake everything visible on the page into one new raster layer on top of the
/// stack, leaving every original untouched.
pub fn stamp_visible(
    raw: &mut Value,
    document: &Path,
    page: Option<&str>,
    new_id: &str,
) -> Result<Value> {
    layer::ensure_new_id(raw, new_id)?;
    let index = page_index(raw, page)?;
    let layer = top_layer(raw, index)?;
    let Some((surface, bounds)) = stamp(raw, document, page)? else {
        bail!("[invalid-input] nothing visible on this page renders any pixels; nothing to stamp")
    };
    let mut next = raw.clone();
    let mut node = raster_node(new_id, bounds);
    let hash = commit_surface(&mut next, &mut node, &surface, "stamp-visible")?;
    next["pages"][index]["layers"][layer]["nodes"]
        .as_array_mut()
        .context("layer nodes are missing")?
        .push(node);
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": new_id, "algorithm": ALGORITHM,
        "bounds": [bounds[0], bounds[1], bounds[2] - bounds[0], bounds[3] - bounds[1]],
        "tiles": surface.tiles.len(), "tile_map_sha256": hash,
    }))
}

/// Replace every visible node of the page with one raster of their composite.
/// Hidden nodes and hidden layers are kept; locked content blocks the operation.
pub fn flatten(
    raw: &mut Value,
    document: &Path,
    page: Option<&str>,
    new_id: &str,
) -> Result<Value> {
    layer::ensure_new_id(raw, new_id)?;
    let index = page_index(raw, page)?;
    {
        let layers = raw["pages"][index]["layers"]
            .as_array()
            .context("page has no layers")?;
        for layer in layers.iter().filter(|l| l["visible"] != false) {
            for node in layer["nodes"].as_array().into_iter().flatten() {
                if node["visible"] != false && (layer["locked"] == true || node["locked"] == true) {
                    bail!(
                        "[locked-node] {} is locked; unlock it before flattening the page",
                        node["id"].as_str().unwrap_or("?")
                    )
                }
            }
        }
    }
    let Some((surface, bounds)) = stamp(raw, document, page)? else {
        bail!("[invalid-input] nothing visible on this page renders any pixels; nothing to flatten")
    };
    let mut next = raw.clone();
    let mut node = raster_node(new_id, bounds);
    let hash = commit_surface(&mut next, &mut node, &surface, "flatten")?;
    let mut removed = 0usize;
    let mut home: Option<usize> = None;
    let layers = next["pages"][index]["layers"]
        .as_array_mut()
        .context("page has no layers")?;
    for (at, layer) in layers.iter_mut().enumerate() {
        if layer["visible"] == false {
            continue;
        }
        let nodes = layer["nodes"]
            .as_array_mut()
            .context("layer nodes are missing")?;
        let before = nodes.len();
        nodes.retain(|n| n["visible"] == false);
        if nodes.len() != before {
            removed += before - nodes.len();
            home.get_or_insert(at);
        }
    }
    let home = home.context("[invalid-input] there are no visible nodes to flatten")?;
    layers[home]["nodes"]
        .as_array_mut()
        .context("layer nodes are missing")?
        .insert(0, node);
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": new_id, "algorithm": ALGORITHM, "nodes_replaced": removed,
        "bounds": [bounds[0], bounds[1], bounds[2] - bounds[0], bounds[3] - bounds[1]],
        "tiles": surface.tiles.len(), "tile_map_sha256": hash, "tiles_released": released,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Surface {
        let mut s = Surface::empty(4, 2);
        s.set_pixel(0, 0, [1, 0, 0, 255]);
        s.set_pixel(3, 0, [0, 2, 0, 128]);
        s.set_pixel(1, 1, [0, 0, 3, 255]);
        s
    }

    #[test]
    fn four_quarter_turns_and_double_flips_are_the_identity() {
        let s = sample();
        let mut t = s.clone();
        for _ in 0..4 {
            t = reorient(&t, Orient::Rotate90);
        }
        assert_eq!(t.tile_map_hash(), s.tile_map_hash());
        let back = reorient(
            &reorient(&s, Orient::FlipHorizontal),
            Orient::FlipHorizontal,
        );
        assert_eq!(back.tile_map_hash(), s.tile_map_hash());
        let r = reorient(&s, Orient::Rotate90);
        assert_eq!((r.width, r.height), (2, 4));
        assert_eq!(r.pixel(1, 0), [1, 0, 0, 255], "top-left goes to top-right");
        let half = reorient(&reorient(&s, Orient::Rotate90), Orient::Rotate90);
        assert_eq!(
            half.tile_map_hash(),
            reorient(&s, Orient::Rotate180).tile_map_hash()
        );
        let ccw = reorient(&s, Orient::Rotate270);
        assert_eq!(
            ccw.pixel(0, 3),
            [1, 0, 0, 255],
            "top-left goes to bottom-left"
        );
    }

    #[test]
    fn rotation_arguments_are_validated() {
        assert_eq!(Orient::rotation(-90).unwrap(), Orient::Rotate270);
        assert!(Orient::rotation(45).is_err());
        assert!(Orient::flip("diagonal").is_err());
    }
}

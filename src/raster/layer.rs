//! Raster layer operations: resize, crop, duplicate, merge-down and rasterize.
//!
//! None of these is replayable stroke math, so each rolls the layer's checkpoint
//! (recording the operation in `checkpoint.reason`) and empties its journal. Every
//! operation works on a clone of the document and only replaces `raw` on success.
use super::*;
use std::path::Path;

/// Largest surface a dense operation may materialize (two RGBA buffers stay
/// within the compositor's temporary-surface budget).
pub const MAX_DENSE_PIXELS: u64 = crate::composite::MAX_TEMP_BYTES / 8;

impl Surface {
    pub fn empty(width: u32, height: u32) -> Self {
        Surface {
            width,
            height,
            tiles: BTreeMap::new(),
        }
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, value: [u8; 4]) {
        let key = (x / TILE as u32, y / TILE as u32);
        let at = (((y as usize) % TILE) * TILE + (x as usize) % TILE) * 4;
        if value[3] == 0 {
            if let Some(tile) = self.tiles.get_mut(&key) {
                tile[at..at + 4].fill(0);
            }
            return;
        }
        let tile = self
            .tiles
            .entry(key)
            .or_insert_with(|| vec![0u8; TILE_BYTES].into_boxed_slice());
        tile[at..at + 4].copy_from_slice(&value);
    }

    /// Visit every non-transparent pixel in tile order.
    pub fn for_each_painted(&self, mut visit: impl FnMut(u32, u32, [u8; 4])) {
        for (key, tile) in &self.tiles {
            for (index, pixel) in tile.chunks(4).enumerate() {
                if pixel[3] == 0 {
                    continue;
                }
                let x = key.0 * TILE as u32 + (index % TILE) as u32;
                let y = key.1 * TILE as u32 + (index / TILE) as u32;
                if x < self.width && y < self.height {
                    visit(x, y, [pixel[0], pixel[1], pixel[2], pixel[3]]);
                }
            }
        }
    }

    /// Copy `source` into this surface offset by `(dx, dy)`, clipping to bounds.
    pub fn blit(&mut self, source: &Surface, dx: i64, dy: i64) {
        let (width, height) = (i64::from(self.width), i64::from(self.height));
        source.for_each_painted(|x, y, value| {
            let (tx, ty) = (i64::from(x) + dx, i64::from(y) + dy);
            if (0..width).contains(&tx) && (0..height).contains(&ty) {
                self.set_pixel(tx as u32, ty as u32, value);
            }
        });
        self.tiles.retain(|_, tile| !is_blank(tile));
    }

    pub fn from_image(image: &image::RgbaImage) -> Self {
        let mut surface = Surface::empty(image.width(), image.height());
        for (x, y, pixel) in image.enumerate_pixels() {
            if pixel[3] != 0 {
                surface.set_pixel(x, y, pixel.0);
            }
        }
        surface
    }

    /// Painted bounds `[x0, y0, x1, y1)` or `None` for an empty surface.
    pub fn painted_bounds(&self) -> Option<[u32; 4]> {
        let mut bounds: Option<[u32; 4]> = None;
        self.for_each_painted(|x, y, _| {
            let b = bounds.get_or_insert([x, y, x + 1, y + 1]);
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x + 1);
            b[3] = b[3].max(y + 1);
        });
        bounds
    }
}

fn ensure_dense(width: u32, height: u32, operation: &str) -> Result<()> {
    if u64::from(width) * u64::from(height) > MAX_DENSE_PIXELS {
        bail!("[limit-exceeded] raster {operation} needs a {width}x{height} working surface; the limit is {MAX_DENSE_PIXELS} pixels")
    }
    Ok(())
}

fn check_size(width: i64, height: i64) -> Result<(u32, u32)> {
    if width < 1 || height < 1 || width as u64 > MAX_DIMENSION || height as u64 > MAX_DIMENSION {
        bail!("[limit-exceeded] raster width and height must be 1-{MAX_DIMENSION}, got {width}x{height}")
    }
    Ok((width as u32, height as u32))
}

/// Store a rebuilt surface into a node and roll its checkpoint.
pub(super) fn commit_surface(
    raw: &mut Value,
    node: &mut Value,
    surface: &Surface,
    reason: &str,
) -> Result<String> {
    node["width"] = json!(surface.width);
    node["height"] = json!(surface.height);
    surface.store(raw, node)?;
    let hash = surface.tile_map_hash();
    roll_checkpoint(node, &hash);
    node["checkpoint"]["reason"] = json!(reason);
    node["engine"] = json!(ENGINE);
    Ok(hash)
}

/// Location of a node: page index, layer index and the index chain through
/// `nodes`/`children`. The last index is the node within its sibling list.
pub(super) struct NodePath {
    pub(super) page: usize,
    pub(super) layer: usize,
    pub(super) chain: Vec<usize>,
}

pub(super) fn find_path(raw: &Value, page: Option<&str>, id: &str) -> Result<NodePath> {
    fn walk(nodes: &[Value], id: &str, chain: &mut Vec<usize>) -> bool {
        for (index, node) in nodes.iter().enumerate() {
            chain.push(index);
            if node["id"] == id {
                return true;
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                if walk(children, id, chain) {
                    return true;
                }
            }
            chain.pop();
        }
        false
    }
    for (page_index, p) in raw["pages"].as_array().into_iter().flatten().enumerate() {
        let selected = match page {
            Some(wanted) => p["id"] == wanted,
            None => page_index == 0,
        };
        if !selected {
            continue;
        }
        for (layer_index, layer) in p["layers"].as_array().into_iter().flatten().enumerate() {
            let mut chain = Vec::new();
            if walk(
                layer["nodes"].as_array().map_or(&[][..], Vec::as_slice),
                id,
                &mut chain,
            ) {
                return Ok(NodePath {
                    page: page_index,
                    layer: layer_index,
                    chain,
                });
            }
        }
    }
    Err(bad(
        "not-found",
        format!(
            "node {id} was not found on page {}; list nodes with `pentool tree DOCUMENT`",
            page.unwrap_or("(first)")
        ),
    ))
}

pub(super) fn siblings_mut<'a>(raw: &'a mut Value, path: &NodePath) -> Result<&'a mut Vec<Value>> {
    let mut list = raw["pages"][path.page]["layers"][path.layer]["nodes"]
        .as_array_mut()
        .context("layer nodes are missing")?;
    for index in &path.chain[..path.chain.len() - 1] {
        list = list[*index]["children"]
            .as_array_mut()
            .context("group children are missing")?;
    }
    Ok(list)
}

pub(super) fn ensure_new_id(raw: &Value, id: &str) -> Result<()> {
    if id.is_empty() {
        bail!("[invalid-input] the new id must not be empty")
    }
    fn contains(value: &Value, id: &str) -> bool {
        match value {
            Value::Object(map) => {
                map.get("id").and_then(Value::as_str) == Some(id)
                    || map.values().any(|v| contains(v, id))
            }
            Value::Array(items) => items.iter().any(|v| contains(v, id)),
            _ => false,
        }
    }
    if contains(&raw["pages"], id) {
        bail!("[invalid-input] id {id} is already used; choose a unique id")
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resample {
    Nearest,
    Bilinear,
}

impl Resample {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "nearest" => Ok(Self::Nearest),
            "bilinear" => Ok(Self::Bilinear),
            other => bail!(
                "[invalid-input] resample {other:?} is not supported; use bilinear or nearest"
            ),
        }
    }
}

/// Deterministic resample with pixel-center mapping. Bilinear filtering runs in
/// premultiplied alpha so transparent pixels never bleed color.
pub(super) fn resample(
    source: &image::RgbaImage,
    width: u32,
    height: u32,
    mode: Resample,
) -> image::RgbaImage {
    let (sw, sh) = (source.width(), source.height());
    let mut out = image::RgbaImage::new(width, height);
    let fx = f64::from(sw) / f64::from(width);
    let fy = f64::from(sh) / f64::from(height);
    for y in 0..height {
        let sy = (f64::from(y) + 0.5) * fy - 0.5;
        for x in 0..width {
            let sx = (f64::from(x) + 0.5) * fx - 0.5;
            let value = match mode {
                Resample::Nearest => {
                    let px = (sx + 0.5).floor().clamp(0.0, f64::from(sw - 1)) as u32;
                    let py = (sy + 0.5).floor().clamp(0.0, f64::from(sh - 1)) as u32;
                    source.get_pixel(px, py).0
                }
                Resample::Bilinear => {
                    let (x0, y0) = (sx.floor(), sy.floor());
                    let (tx, ty) = (sx - x0, sy - y0);
                    let mut acc = [0.0f64; 4];
                    for (ox, oy, weight) in [
                        (0.0, 0.0, (1.0 - tx) * (1.0 - ty)),
                        (1.0, 0.0, tx * (1.0 - ty)),
                        (0.0, 1.0, (1.0 - tx) * ty),
                        (1.0, 1.0, tx * ty),
                    ] {
                        let px = (x0 + ox).clamp(0.0, f64::from(sw - 1)) as u32;
                        let py = (y0 + oy).clamp(0.0, f64::from(sh - 1)) as u32;
                        let p = source.get_pixel(px, py).0;
                        let alpha = f64::from(p[3]);
                        for channel in 0..3 {
                            acc[channel] += f64::from(p[channel]) * alpha * weight;
                        }
                        acc[3] += alpha * weight;
                    }
                    let alpha = acc[3].round().clamp(0.0, 255.0);
                    if alpha == 0.0 {
                        [0; 4]
                    } else {
                        let mut value = [0u8; 4];
                        for channel in 0..3 {
                            value[channel] =
                                (acc[channel] / acc[3]).round().clamp(0.0, 255.0) as u8;
                        }
                        value[3] = alpha as u8;
                        value
                    }
                }
            };
            out.put_pixel(x, y, image::Rgba(value));
        }
    }
    out
}

/// Scale a layer's pixels to a new size; its position is unchanged.
pub fn resize(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    width: u32,
    height: u32,
    mode: Resample,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let (width, height) = check_size(i64::from(width), i64::from(height))?;
    let mut next = raw.clone();
    let mut node = locate(&mut next, page, id)?.clone();
    let source = Surface::load(&next, &node)?;
    ensure_dense(source.width, source.height, "resize")?;
    ensure_dense(width, height, "resize")?;
    let scaled = resample(&source.to_image()?, width, height, mode);
    let surface = Surface::from_image(&scaled);
    let hash = commit_surface(&mut next, &mut node, &surface, "resize")?;
    *locate(&mut next, page, id)? = node;
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(
        json!({"id":id,"width":width,"height":height,"from":[source.width,source.height],
        "tiles":surface.tiles.len(),"tile_map_sha256":hash,"tiles_released":released}),
    )
}

/// Change a layer's bounds to a layer-local rectangle without resampling. The
/// rectangle may extend past the old bounds (new area is transparent), so this is
/// also the layer canvas-resize operation. Pixels keep their place on the page.
pub fn crop(raw: &mut Value, page: Option<&str>, id: &str, rect: [i64; 4]) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let (width, height) = check_size(rect[2], rect[3])?;
    let mut next = raw.clone();
    let mut node = locate(&mut next, page, id)?.clone();
    let source = Surface::load(&next, &node)?;
    let mut surface = Surface::empty(width, height);
    surface.blit(&source, -rect[0], -rect[1]);
    let lost = {
        let mut count = 0u64;
        source.for_each_painted(|x, y, _| {
            let (tx, ty) = (i64::from(x) - rect[0], i64::from(y) - rect[1]);
            if tx < 0 || ty < 0 || tx >= rect[2] || ty >= rect[3] {
                count += 1;
            }
        });
        count
    };
    node["x"] = json!(node["x"].as_f64().unwrap_or(0.0) + rect[0] as f64);
    node["y"] = json!(node["y"].as_f64().unwrap_or(0.0) + rect[1] as f64);
    let hash = commit_surface(&mut next, &mut node, &surface, "crop")?;
    *locate(&mut next, page, id)? = node;
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(
        json!({"id":id,"rect":rect,"width":width,"height":height,"pixels_removed":lost,
        "tiles":surface.tiles.len(),"tile_map_sha256":hash,"tiles_released":released}),
    )
}

/// Trim a layer to its painted pixels (item 10). Empty layers are refused.
pub fn trim(raw: &mut Value, page: Option<&str>, id: &str) -> Result<Value> {
    let node = all_rasters(raw)
        .into_iter()
        .find(|n| n["id"] == id)
        .cloned()
        .ok_or_else(|| bad("not-found", format!("raster layer {id} was not found")))?;
    let surface = Surface::load(raw, &node)?;
    let Some([x0, y0, x1, y1]) = surface.painted_bounds() else {
        bail!("[invalid-input] raster {id} has no painted pixels to trim to; use `raster clear` or remove the node")
    };
    crop(
        raw,
        page,
        id,
        [
            i64::from(x0),
            i64::from(y0),
            i64::from(x1 - x0),
            i64::from(y1 - y0),
        ],
    )
}

/// Copy a raster layer directly above itself under a new id. Tiles are shared.
pub fn duplicate(raw: &mut Value, page: Option<&str>, id: &str, new_id: &str) -> Result<Value> {
    // Duplicating reads the source only; the destination layer must be unlocked.
    ensure_node_unlocked(raw, page, id)?;
    ensure_new_id(raw, new_id)?;
    let mut next = raw.clone();
    let path = find_path(&next, page, id)?;
    let index = *path.chain.last().unwrap();
    let list = siblings_mut(&mut next, &path)?;
    let mut copy = list[index].clone();
    copy["id"] = json!(new_id);
    let hash = Surface::load(raw, &copy)?.tile_map_hash();
    roll_checkpoint(&mut copy, &hash);
    copy["checkpoint"]["reason"] = json!("duplicate");
    copy["checkpoint"]["journal_dropped"] = json!(0);
    copy["engine"] = json!(ENGINE);
    list.insert(index + 1, copy);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({"id":new_id,"source":id,"tile_map_sha256":hash}))
}

pub(super) fn integer_offset(node: &Value, what: &str) -> Result<(i64, i64)> {
    let id = node["id"].as_str().unwrap_or("?");
    let t = crate::composite::transform(node)?;
    let c = t.as_coeffs();
    if c[0] != 1.0 || c[1] != 0.0 || c[2] != 0.0 || c[3] != 1.0 {
        bail!("[unsupported-capability] {what} {id} has a scale, rotation or skew transform; rasterize it first")
    }
    let x = node["x"].as_f64().unwrap_or(0.0) + c[4];
    let y = node["y"].as_f64().unwrap_or(0.0) + c[5];
    if x.fract() != 0.0 || y.fract() != 0.0 {
        bail!("[unsupported-capability] {what} {id} sits at a fractional position ({x}, {y}); move it to whole pixels or rasterize it first")
    }
    if node
        .get("transforms")
        .and_then(Value::as_array)
        .is_some_and(|s| !s.is_empty())
    {
        bail!("[unsupported-capability] {what} {id} has a warp transform stack; rasterize it first")
    }
    Ok((x as i64, y as i64))
}

fn refuse_live_styling(node: &Value, what: &str) -> Result<()> {
    let id = node["id"].as_str().unwrap_or("?");
    for key in ["effects", "mask", "clipping", "content_opacity"] {
        if node.get(key).is_some_and(|v| !v.is_null()) {
            bail!("[unsupported-capability] {what} {id} has {key}; rasterize it first so the merge cannot drop live styling")
        }
    }
    Ok(())
}

/// Merge a raster layer into the raster layer directly beneath it in the same
/// sibling list, applying the upper layer's opacity and blend mode. The lower
/// layer grows to the union of both bounds, so no pixels are discarded.
pub fn merge_down(raw: &mut Value, page: Option<&str>, id: &str) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    let mut next = raw.clone();
    let path = find_path(&next, page, id)?;
    let index = *path.chain.last().unwrap();
    let list = siblings_mut(&mut next, &path)?;
    if index == 0 {
        bail!("[invalid-input] {id} is the bottom node of its group or layer; there is nothing to merge into")
    }
    let upper = list[index].clone();
    let lower = list[index - 1].clone();
    let lower_id = lower["id"].as_str().unwrap_or("?").to_owned();
    if !is_raster(&lower) {
        bail!("[invalid-input] the node below {id} is {} {lower_id}, not a raster layer; rasterize it first", lower["kind"].as_str().unwrap_or("?"))
    }
    ensure_node_unlocked(raw, page, &lower_id)?;
    if upper["visible"] == false || lower["visible"] == false {
        bail!("[invalid-input] merge-down needs both {id} and {lower_id} visible")
    }
    refuse_live_styling(&upper, "raster")?;
    refuse_live_styling(&lower, "raster")?;
    if lower.get("opacity").and_then(Value::as_f64).unwrap_or(1.0) != 1.0
        || lower["blend_mode"].as_str().unwrap_or("normal") != "normal"
    {
        bail!("[unsupported-capability] {lower_id} has its own opacity or blend mode, which would also apply to the merged pixels; reset it or rasterize both first")
    }
    let (ux, uy) = integer_offset(&upper, "raster")?;
    let (lx, ly) = integer_offset(&lower, "raster")?;
    let top = Surface::load(&next, &upper)?;
    let bottom = Surface::load(&next, &lower)?;
    let opacity = upper.get("opacity").and_then(Value::as_f64).unwrap_or(1.0);
    let mode = upper["blend_mode"].as_str().unwrap_or("normal").to_owned();
    let space = upper["blend_space"].as_str().unwrap_or("srgb").to_owned();
    crate::blend::validate(&mode, &space)?;

    // Union of both layers in page pixels; only the upper's painted area grows it.
    let mut x0 = lx;
    let mut y0 = ly;
    let mut x1 = lx + i64::from(bottom.width);
    let mut y1 = ly + i64::from(bottom.height);
    if let Some(b) = top.painted_bounds() {
        x0 = x0.min(ux + i64::from(b[0]));
        y0 = y0.min(uy + i64::from(b[1]));
        x1 = x1.max(ux + i64::from(b[2]));
        y1 = y1.max(uy + i64::from(b[3]));
    }
    let (width, height) = check_size(x1 - x0, y1 - y0)?;
    let mut merged = Surface::empty(width, height);
    merged.blit(&bottom, lx - x0, ly - y0);

    // Blend tile by tile over the destination tiles the upper layer touches.
    let mut touched: std::collections::BTreeSet<(u32, u32)> = Default::default();
    top.for_each_painted(|x, y, _| {
        let (mx, my) = (i64::from(x) + ux - x0, i64::from(y) + uy - y0);
        touched.insert((mx as u32 / TILE as u32, my as u32 / TILE as u32));
    });
    for key in touched {
        let (ox, oy) = (key.0 * TILE as u32, key.1 * TILE as u32);
        let w = (width - ox).min(TILE as u32);
        let h = (height - oy).min(TILE as u32);
        let mut dst = image::RgbaImage::new(w, h);
        let mut src = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                dst.put_pixel(x, y, image::Rgba(merged.pixel(ox + x, oy + y)));
                let (tx, ty) = (i64::from(ox + x) + x0 - ux, i64::from(oy + y) + y0 - uy);
                if tx >= 0 && ty >= 0 && tx < i64::from(top.width) && ty < i64::from(top.height) {
                    let mut p = top.pixel(tx as u32, ty as u32);
                    // Same rounding as the compositor's layer opacity.
                    p[3] = (f64::from(p[3]) * opacity).round() as u8;
                    if p[3] == 0 {
                        p = [0; 4];
                    }
                    src.put_pixel(x, y, image::Rgba(p));
                }
            }
        }
        crate::blend::over(&mut dst, &src, &mode, &space)?;
        for (x, y, p) in dst.enumerate_pixels() {
            merged.set_pixel(ox + x, oy + y, p.0);
        }
    }
    merged.tiles.retain(|_, tile| !is_blank(tile));

    let list = siblings_mut(&mut next, &path)?;
    let mut target = list[index - 1].clone();
    // Keep the lower layer's transform translation; express growth through x/y.
    let c = crate::composite::transform(&target)?.as_coeffs();
    target["x"] = json!(x0 as f64 - c[4]);
    target["y"] = json!(y0 as f64 - c[5]);
    let hash = commit_surface(&mut next, &mut target, &merged, "merge-down")?;
    let list = siblings_mut(&mut next, &path)?;
    list[index - 1] = target;
    list.remove(index);
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(
        json!({"id":lower_id,"merged":id,"blend_mode":mode,"opacity":opacity,
        "bounds":[x0,y0,width,height],"tiles":merged.tiles.len(),"tile_map_sha256":hash,
        "tiles_released":released}),
    )
}

/// Explicitly bake any visible node (vector, text, image, group, raster with
/// effects...) into a new raster layer placed directly above it. The source is
/// hidden, not deleted, unless `replace` is set.
pub fn rasterize(
    raw: &mut Value,
    document: &Path,
    page: Option<&str>,
    id: &str,
    new_id: &str,
    replace: bool,
) -> Result<Value> {
    ensure_new_id(raw, new_id)?;
    let mut next = raw.clone();
    let path = find_path(&next, page, id)?;
    crate::composite::ensure_unlocked(&next["pages"][path.page], id)?;
    let (_, parent) = crate::composite::node_ref(&next, page, id)?;
    if parent != kurbo::Affine::IDENTITY {
        bail!("[unsupported-capability] {id} is inside a transformed group; rasterize the group instead")
    }
    let mut source = siblings_mut(&mut next, &path)?[*path.chain.last().unwrap()].clone();
    // Render the node exactly as the compositor shows it, but ignore its hidden state.
    source.as_object_mut().map(|o| o.remove("visible"));
    let mut probe = next.clone();
    siblings_mut(&mut probe, &path)?[*path.chain.last().unwrap()] = source.clone();
    let canvas = crate::composite::node_pixels(&probe, document, page, id, 1.0)?;
    ensure_dense(canvas.width(), canvas.height(), "rasterize")?;
    let full = Surface::from_image(&canvas);
    let Some([x0, y0, x1, y1]) = full.painted_bounds() else {
        bail!("[invalid-input] {id} renders no visible pixels on this page; nothing to rasterize")
    };
    let mut surface = Surface::empty(x1 - x0, y1 - y0);
    surface.blit(&full, -i64::from(x0), -i64::from(y0));
    let mut node = json!({
        "kind": "raster", "id": new_id, "engine": ENGINE, "x": x0, "y": y0,
        "width": x1 - x0, "height": y1 - y0, "tile_size": TILE, "pixel_format": FORMAT,
        "tiles": {}, "journal": [], "opacity": 1, "blend_mode": "normal",
        "transform": [1, 0, 0, 1, 0, 0]
    });
    let hash = commit_surface(&mut next, &mut node, &surface, "rasterize")?;
    node["checkpoint"]["source"] = json!(id);
    let index = *path.chain.last().unwrap();
    let list = siblings_mut(&mut next, &path)?;
    if replace {
        list[index] = node;
    } else {
        list[index]["visible"] = json!(false);
        list.insert(index + 1, node);
    }
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(
        json!({"id":new_id,"source":id,"source_kept":!replace,"bounds":[x0,y0,x1-x0,y1-y0],
        "tiles":surface.tiles.len(),"tile_map_sha256":hash}),
    )
}

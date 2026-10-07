//! Working with selected pixels: lift to a new layer, move, transform, paste.
//!
//! These are not replayable stroke math, so each one that rewrites a layer rolls
//! its checkpoint (recording the operation in `checkpoint.reason`) and empties its
//! journal, like resize and merge-down. Alpha is never discarded: lifted pixels
//! keep their color and carry `alpha * coverage`, and compositing back is a
//! straight-alpha "over". Selection coverage is also moved or transformed with the
//! pixels, so the selection keeps following what the user is working on.
use super::layer::{commit_surface, ensure_new_id, find_path, integer_offset, siblings_mut};
use super::shape::ensure_plane;
use super::*;

pub const ALGORITHM: u64 = 1;
pub const MIN_SCALE: f64 = 0.05;
pub const MAX_SCALE: f64 = 20.0;

/// Straight-alpha "source over destination", rounded to 8 bits.
fn over(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let (sa, da) = (u64::from(src[3]), u64::from(dst[3]));
    if sa == 255 || da == 0 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let out_a = sa * 255 + da * (255 - sa);
    let mut out = [0u8; 4];
    for ch in 0..3 {
        let num = u64::from(src[ch]) * sa * 255 + u64::from(dst[ch]) * da * (255 - sa);
        out[ch] = ((num + out_a / 2) / out_a).min(255) as u8;
    }
    out[3] = ((out_a + 127) / 255).min(255) as u8;
    out
}

struct Target {
    surface: Surface,
    selection: Surface,
}

fn load_target(raw: &Value, page: Option<&str>, id: &str) -> Result<(Target, Value)> {
    let node = locate(&mut raw.clone(), page, id)?.clone();
    if !is_raster(&node) {
        bail!("[invalid-input] {id} is not a raster layer")
    }
    let surface = Surface::load(raw, &node)?;
    ensure_plane(surface.width, surface.height)?;
    let Some((selection, _)) = selection::pin(raw, id, surface.width, surface.height)? else {
        bail!("[invalid-selection] {id} has no selection; make one with select-marquee, select-lasso or select-wand")
    };
    if selection.tiles.is_empty() {
        bail!("[invalid-selection] the selection of {id} selects nothing")
    }
    Ok((Target { surface, selection }, node))
}

/// The selected pixels: each keeps its color and carries `alpha * coverage`.
fn lifted(target: &Target) -> Surface {
    let mut out = Surface::empty(target.surface.width, target.surface.height);
    target.selection.for_each_painted(|x, y, cover| {
        let p = target.surface.pixel(x, y);
        let alpha = ((u32::from(p[3]) * u32::from(cover[3]) + 127) / 255) as u8;
        if alpha != 0 {
            out.set_pixel(x, y, [p[0], p[1], p[2], alpha]);
        }
    });
    out
}

/// The layer with the selected share of each pixel removed.
fn without_selected(target: &Target) -> Surface {
    let mut out = target.surface.clone();
    target.selection.for_each_painted(|x, y, cover| {
        let p = out.pixel(x, y);
        let alpha = ((u32::from(p[3]) * (255 - u32::from(cover[3])) + 127) / 255) as u8;
        out.set_pixel(
            x,
            y,
            if alpha == 0 {
                [0; 4]
            } else {
                [p[0], p[1], p[2], alpha]
            },
        );
    });
    out.tiles.retain(|_, tile| !is_blank(tile));
    out
}

fn composite_over(base: &mut Surface, top: &Surface, dx: i64, dy: i64) {
    let (w, h) = (i64::from(base.width), i64::from(base.height));
    top.for_each_painted(|x, y, p| {
        let (tx, ty) = (i64::from(x) + dx, i64::from(y) + dy);
        if (0..w).contains(&tx) && (0..h).contains(&ty) {
            let under = base.pixel(tx as u32, ty as u32);
            base.set_pixel(tx as u32, ty as u32, over(under, p));
        }
    });
    base.tiles.retain(|_, tile| !is_blank(tile));
}

fn translated(source: &Surface, dx: i64, dy: i64) -> Surface {
    let mut out = Surface::empty(source.width, source.height);
    out.blit(source, dx, dy);
    out
}

fn commit(
    next: &mut Value,
    page: Option<&str>,
    id: &str,
    mut node: Value,
    surface: &Surface,
    reason: &str,
) -> Result<String> {
    let hash = commit_surface(next, &mut node, surface, reason)?;
    *locate(next, page, id)? = node;
    Ok(hash)
}

/// Copy (or cut) the selected pixels into a new raster layer directly above the
/// source, positioned over the selection's bounds.
pub fn lift(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    new_id: &str,
    cut: bool,
) -> Result<Value> {
    if cut {
        ensure_node_unlocked(raw, page, id)?;
    }
    ensure_new_id(raw, new_id)?;
    let (target, node) = load_target(raw, page, id)?;
    let (ox, oy) = integer_offset(&node, "raster")?;
    let pixels = lifted(&target);
    let Some([x0, y0, x1, y1]) = pixels.painted_bounds() else {
        bail!("[invalid-selection] the selection of {id} covers only transparent pixels; there is nothing to lift")
    };
    let mut surface = Surface::empty(x1 - x0, y1 - y0);
    surface.blit(&pixels, -i64::from(x0), -i64::from(y0));
    let mut next = raw.clone();
    let mut layer = json!({
        "kind": "raster", "id": new_id, "engine": ENGINE,
        "x": ox + i64::from(x0), "y": oy + i64::from(y0),
        "width": x1 - x0, "height": y1 - y0, "tile_size": TILE, "pixel_format": FORMAT,
        "tiles": {}, "journal": [], "opacity": 1, "blend_mode": "normal",
        "transform": [1, 0, 0, 1, 0, 0]
    });
    let lifted_hash = commit_surface(
        &mut next,
        &mut layer,
        &surface,
        if cut { "cut" } else { "copy" },
    )?;
    layer["checkpoint"]["source"] = json!(id);
    let path = find_path(&next, page, id)?;
    let index = *path.chain.last().unwrap();
    siblings_mut(&mut next, &path)?.insert(index + 1, layer);
    if cut {
        let rest = without_selected(&target);
        commit(&mut next, page, id, node, &rest, "cut")?;
    }
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": new_id, "source": id, "cut": cut, "algorithm": ALGORITHM,
        "bounds": [ox + i64::from(x0), oy + i64::from(y0), x1 - x0, y1 - y0],
        "tiles": surface.tiles.len(), "tile_map_sha256": lifted_hash,
        "tiles_released": released,
    }))
}

/// Move the selected pixels (and the selection) by whole pixels. `copy` leaves the
/// original in place.
pub fn move_pixels(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    dx: i64,
    dy: i64,
    copy: bool,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    if dx.unsigned_abs() > MAX_DIMENSION || dy.unsigned_abs() > MAX_DIMENSION {
        bail!("[limit-exceeded] a move is limited to {MAX_DIMENSION} pixels in each direction")
    }
    let (target, node) = load_target(raw, page, id)?;
    let floating = lifted(&target);
    let mut surface = if copy {
        target.surface.clone()
    } else {
        without_selected(&target)
    };
    composite_over(&mut surface, &floating, dx, dy);
    let moved = translated(&target.selection, dx, dy);
    let mut next = raw.clone();
    let hash = commit(
        &mut next,
        page,
        id,
        node,
        &surface,
        if copy { "copy-move" } else { "move" },
    )?;
    selection::store_active(&mut next, id, &moved)?;
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": id, "dx": dx, "dy": dy, "copy": copy, "algorithm": ALGORITHM,
        "tile_map_sha256": hash, "tiles_released": released,
        "selection": selection::summary(&moved),
    }))
}

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotate: f64,
    pub dx: f64,
    pub dy: f64,
    pub nearest: bool,
    pub copy: bool,
}

/// Bilinear (or nearest) resampling of a straight-RGBA surface through the inverse
/// of `forward`, restricted to destinations inside the canvas.
fn warp(source: &Surface, inverse: [f64; 6], bounds: [i64; 4], nearest: bool) -> Surface {
    let mut out = Surface::empty(source.width, source.height);
    let at = |x: i64, y: i64| -> [f64; 4] {
        if x < 0 || y < 0 || x >= i64::from(source.width) || y >= i64::from(source.height) {
            return [0.0; 4];
        }
        let p = source.pixel(x as u32, y as u32);
        [
            f64::from(p[0]),
            f64::from(p[1]),
            f64::from(p[2]),
            f64::from(p[3]),
        ]
    };
    for y in bounds[1]..bounds[3] {
        for x in bounds[0]..bounds[2] {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let u = inverse[0] * px + inverse[2] * py + inverse[4];
            let v = inverse[1] * px + inverse[3] * py + inverse[5];
            let pixel = if nearest {
                let p = at(u.floor() as i64, v.floor() as i64);
                [p[0] as u8, p[1] as u8, p[2] as u8, p[3] as u8]
            } else {
                let (u, v) = (u - 0.5, v - 0.5);
                let (x0, y0) = (u.floor(), v.floor());
                let (fx, fy) = (u - x0, v - y0);
                let mut alpha = 0.0;
                let mut color = [0.0; 3];
                for (ox, oy, weight) in [
                    (0, 0, (1.0 - fx) * (1.0 - fy)),
                    (1, 0, fx * (1.0 - fy)),
                    (0, 1, (1.0 - fx) * fy),
                    (1, 1, fx * fy),
                ] {
                    let p = at(x0 as i64 + ox, y0 as i64 + oy);
                    let w = weight * p[3];
                    alpha += w;
                    for ch in 0..3 {
                        color[ch] += w * p[ch];
                    }
                }
                let a = alpha.round().clamp(0.0, 255.0) as u8;
                if a == 0 {
                    [0; 4]
                } else {
                    let c = |i: usize| (color[i] / alpha).round().clamp(0.0, 255.0) as u8;
                    [c(0), c(1), c(2), a]
                }
            };
            if pixel[3] != 0 {
                out.set_pixel(x as u32, y as u32, pixel);
            }
        }
    }
    out
}

/// Scale and rotate the selected pixels (and the selection) about the center of
/// the selection's bounds, then shift them by `(dx, dy)`.
pub fn transform_pixels(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    t: &Transform,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    for s in [t.scale_x, t.scale_y] {
        if !s.is_finite() || !(MIN_SCALE..=MAX_SCALE).contains(&s) {
            bail!("[invalid-transform] scale must be {MIN_SCALE}-{MAX_SCALE}")
        }
    }
    if !t.rotate.is_finite() || t.rotate.abs() > 360.0 || !t.dx.is_finite() || !t.dy.is_finite() {
        bail!("[invalid-transform] rotate must be -360 to 360 degrees and the shift finite")
    }
    let (target, node) = load_target(raw, page, id)?;
    let floating = lifted(&target);
    let Some([bx0, by0, bx1, by1]) = target.selection.painted_bounds() else {
        bail!("[invalid-selection] the selection of {id} selects nothing")
    };
    let (cx, cy) = (
        f64::from(bx0 + bx1) / 2.0 + t.dx,
        f64::from(by0 + by1) / 2.0 + t.dy,
    );
    let (ox, oy) = (f64::from(bx0 + bx1) / 2.0, f64::from(by0 + by1) / 2.0);
    let (sin, cos) = sin_cos_degrees(t.rotate);
    // forward: p' = center' + R * S * (p - center)
    let forward = |x: f64, y: f64| -> (f64, f64) {
        let (sx, sy) = ((x - ox) * t.scale_x, (y - oy) * t.scale_y);
        (cx + cos * sx - sin * sy, cy + sin * sx + cos * sy)
    };
    // inverse: p = center + S^-1 * R^-1 * (p' - center')
    let inverse = {
        let (a, b, c, d) = (
            cos / t.scale_x,
            -sin / t.scale_y,
            sin / t.scale_x,
            cos / t.scale_y,
        );
        // u = a * (x - cx) + c * (y - cy) + ox ; v = b * (x - cx) + d * (y - cy) + oy
        [a, b, c, d, ox - a * cx - c * cy, oy - b * cx - d * cy]
    };
    let corners = [
        forward(f64::from(bx0), f64::from(by0)),
        forward(f64::from(bx1), f64::from(by0)),
        forward(f64::from(bx0), f64::from(by1)),
        forward(f64::from(bx1), f64::from(by1)),
    ];
    let lo = |f: fn(&(f64, f64)) -> f64, w: u32| -> i64 {
        (corners.iter().map(f).fold(f64::INFINITY, f64::min).floor() as i64 - 1)
            .clamp(0, i64::from(w))
    };
    let hi = |f: fn(&(f64, f64)) -> f64, w: u32| -> i64 {
        (corners
            .iter()
            .map(f)
            .fold(f64::NEG_INFINITY, f64::max)
            .floor() as i64
            + 2)
        .clamp(0, i64::from(w))
    };
    let (w, h) = (target.surface.width, target.surface.height);
    let bounds = [
        lo(|c| c.0, w),
        lo(|c| c.1, h),
        hi(|c| c.0, w),
        hi(|c| c.1, h),
    ];
    let moved = warp(&floating, inverse, bounds, t.nearest);
    let moved_selection = warp(&target.selection, inverse, bounds, t.nearest);
    let mut surface = if t.copy {
        target.surface.clone()
    } else {
        without_selected(&target)
    };
    composite_over(&mut surface, &moved, 0, 0);
    let mut next = raw.clone();
    let hash = commit(&mut next, page, id, node, &surface, "transform")?;
    selection::store_active(&mut next, id, &moved_selection)?;
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": id, "algorithm": ALGORITHM, "copy": t.copy,
        "scale": [t.scale_x, t.scale_y], "rotate": t.rotate, "shift": [t.dx, t.dy],
        "resample": if t.nearest { "nearest" } else { "bilinear" },
        "tile_map_sha256": hash, "tiles_released": released,
        "selection": selection::summary(&moved_selection),
    }))
}

/// Paste another raster layer's pixels into `id` at layer-local `(x, y)`, over the
/// existing pixels. An active selection on `id` limits the paste.
pub fn paste(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    source: &str,
    x: i64,
    y: i64,
    opacity: f64,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    if id == source {
        bail!("[invalid-input] paste needs a different source layer than {id}; use move-pixels within one layer")
    }
    if !(0.0..=1.0).contains(&opacity) || !opacity.is_finite() {
        bail!("[invalid-input] opacity must be between 0 and 1")
    }
    if x.unsigned_abs() > MAX_DIMENSION * 2 || y.unsigned_abs() > MAX_DIMENSION * 2 {
        bail!("[limit-exceeded] paste position is out of range")
    }
    let destination = locate(&mut raw.clone(), page, id)?.clone();
    let origin = locate(&mut raw.clone(), page, source)?.clone();
    if !is_raster(&destination) || !is_raster(&origin) {
        bail!("[invalid-input] paste works between raster layers")
    }
    let base = Surface::load(raw, &destination)?;
    let top = Surface::load(raw, &origin)?;
    let pinned = selection::pin(raw, id, base.width, base.height)?;
    let mut surface = base.clone();
    let mut faded = Surface::empty(top.width, top.height);
    top.for_each_painted(|px, py, p| {
        let alpha = (f64::from(p[3]) * opacity).round() as u8;
        if alpha != 0 {
            faded.set_pixel(px, py, [p[0], p[1], p[2], alpha]);
        }
    });
    composite_over(&mut surface, &faded, x, y);
    if let Some((selection, _)) = &pinned {
        selection::blend_through(&base, &mut surface, selection);
    }
    let mut next = raw.clone();
    let hash = commit(&mut next, page, id, destination, &surface, "paste")?;
    next_source(&mut next, id, source);
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": id, "source": source, "at": [x, y], "opacity": opacity,
        "selection_applied": pinned.is_some(), "algorithm": ALGORITHM,
        "tile_map_sha256": hash, "tiles_released": released,
    }))
}

/// Record the source layer in the destination checkpoint for traceability.
fn next_source(next: &mut Value, id: &str, source: &str) {
    fn walk(value: &mut Value, id: &str, source: &str) -> bool {
        match value {
            Value::Object(map) => {
                if map.get("id").and_then(Value::as_str) == Some(id)
                    && map.contains_key("checkpoint")
                {
                    map["checkpoint"]["source"] = json!(source);
                    return true;
                }
                map.values_mut().any(|v| walk(v, id, source))
            }
            Value::Array(items) => items.iter_mut().any(|v| walk(v, id, source)),
            _ => false,
        }
    }
    walk(next, id, source);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_keeps_alpha_and_color() {
        assert_eq!(over([0, 0, 255, 255], [255, 0, 0, 255]), [255, 0, 0, 255]);
        assert_eq!(over([0, 0, 255, 255], [255, 0, 0, 0]), [0, 0, 255, 255]);
        assert_eq!(over([0; 4], [10, 20, 30, 40]), [10, 20, 30, 40]);
        let mixed = over([0, 0, 255, 255], [255, 0, 0, 128]);
        assert_eq!(mixed[3], 255);
        assert!(mixed[0] > 120 && mixed[2] > 120);
    }

    #[test]
    fn an_identity_warp_reproduces_the_pixels() {
        let mut s = Surface::empty(8, 8);
        s.set_pixel(3, 3, [10, 200, 30, 255]);
        s.set_pixel(4, 3, [10, 200, 30, 90]);
        let out = warp(&s, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, 0, 8, 8], false);
        assert_eq!(out.pixel(3, 3), [10, 200, 30, 255]);
        assert_eq!(out.pixel(4, 3), [10, 200, 30, 90]);
        let shifted = warp(&s, [1.0, 0.0, 0.0, 1.0, -2.0, 0.0], [0, 0, 8, 8], true);
        assert_eq!(shifted.pixel(5, 3), [10, 200, 30, 255]);
    }
}

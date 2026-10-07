//! Pixel selections: an 8-bit coverage plane per raster layer.
//!
//! A selection is stored like a layer: 256-px tiles in `raster_tiles` holding
//! `[0, 0, 0, coverage]`, referenced from `raster_selections.active.<layer id>`
//! (`{width, height, tiles}`) so garbage collection keeps them. A missing tile is
//! coverage 0. Having no entry means "no selection": every pixel is editable. An
//! entry with no tiles selects nothing.
//!
//! While a selection is active, every paint operation on its layer (strokes, clone,
//! heal, fill) changes only what the selection covers, in proportion to coverage.
//! The selection is pinned into the journal entry of each stroke so replay never
//! reads the live selection.
use super::*;

/// A stroke journal pins at most this many selection tiles.
pub const MAX_PINNED_TILES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl Mode {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "replace" => Self::Replace,
            "add" => Self::Add,
            "subtract" => Self::Subtract,
            "intersect" => Self::Intersect,
            other => bail!(
                "[invalid-selection] mode {other:?} must be replace, add, subtract or intersect"
            ),
        })
    }
}

#[cfg(test)]
pub(super) fn coverage(selection: &Surface, x: u32, y: u32) -> u8 {
    selection.pixel(x, y)[3]
}

pub(super) fn set_coverage(selection: &mut Surface, x: u32, y: u32, value: u8) {
    selection.set_pixel(x, y, if value == 0 { [0; 4] } else { [0, 0, 0, value] });
}

/// A selection stored for `id`, or `None` when nothing was ever selected.
pub fn active(raw: &Value, id: &str) -> Result<Option<Surface>> {
    let Some(entry) = raw
        .get("raster_selections")
        .and_then(|s| s.get("active"))
        .and_then(|a| a.get(id))
    else {
        return Ok(None);
    };
    Ok(Some(from_json(raw, entry)?))
}

pub(super) fn from_json(raw: &Value, entry: &Value) -> Result<Surface> {
    let dimension = |key: &str| -> Result<u32> {
        entry[key]
            .as_u64()
            .filter(|n| (1..=MAX_DIMENSION).contains(n))
            .map(|n| n as u32)
            .with_context(|| {
                format!("[malformed-raster] selection {key} must be 1-{MAX_DIMENSION}")
            })
    };
    let tiles = entry
        .get("tiles")
        .filter(|t| t.is_object())
        .context("[malformed-raster] selection tiles must be an object")?;
    Surface::load_map(raw, dimension("width")?, dimension("height")?, tiles)
}

pub(super) fn to_json(raw: &mut Value, selection: &Surface) -> Result<Value> {
    let mut holder = json!({});
    selection.store(raw, &mut holder)?;
    Ok(json!({
        "width": selection.width,
        "height": selection.height,
        "tiles": holder["tiles"].clone(),
    }))
}

pub fn store_active(raw: &mut Value, id: &str, selection: &Surface) -> Result<()> {
    let entry = to_json(raw, selection)?;
    let root = raw
        .as_object_mut()
        .context("document must be an object")?
        .entry("raster_selections")
        .or_insert_with(|| json!({}));
    root["active"][id] = entry;
    Ok(())
}

pub fn clear(raw: &mut Value, id: &str) -> bool {
    raw.get_mut("raster_selections")
        .and_then(|s| s.get_mut("active"))
        .and_then(Value::as_object_mut)
        .is_some_and(|map| map.remove(id).is_some())
}

/// The active selection of a layer with its dimensions checked, plus the journal
/// form that pins its tiles.
pub(super) fn pin(
    raw: &Value,
    id: &str,
    width: u32,
    height: u32,
) -> Result<Option<(Surface, Value)>> {
    let Some(selection) = active(raw, id)? else {
        return Ok(None);
    };
    if (selection.width, selection.height) != (width, height) {
        bail!(
            "[invalid-selection] the selection of {id} is {}x{} but the layer is {width}x{height}; run `pentool raster DOCUMENT select-clear {id}`",
            selection.width,
            selection.height
        )
    }
    let entry = raw["raster_selections"]["active"][id].clone();
    let count = entry["tiles"].as_object().map_or(0, |t| t.len());
    if count > MAX_PINNED_TILES {
        bail!("[limit-exceeded] the selection of {id} spans {count} tiles; a stroke can pin at most {MAX_PINNED_TILES}. Shrink or split the selection")
    }
    Ok(Some((selection, entry)))
}

/// Combine a new coverage plane with the existing selection.
pub fn combine(mode: Mode, old: Option<&Surface>, new: &Surface) -> Result<Surface> {
    let Some(old) = old else {
        return match mode {
            Mode::Subtract => bail!("[invalid-selection] nothing is selected to subtract from"),
            _ => Ok(new.clone()),
        };
    };
    if mode == Mode::Replace {
        return Ok(new.clone());
    }
    if (old.width, old.height) != (new.width, new.height) {
        bail!("[invalid-selection] cannot combine selections of different sizes")
    }
    let mut out = Surface::empty(old.width, old.height);
    let mut keys: Vec<_> = old.tiles.keys().chain(new.tiles.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    let zero = vec![0u8; TILE_BYTES];
    for key in keys {
        let a = old.tiles.get(&key).map_or(&zero[..], |t| &t[..]);
        let b = new.tiles.get(&key).map_or(&zero[..], |t| &t[..]);
        let mut tile = vec![0u8; TILE_BYTES].into_boxed_slice();
        for i in 0..TILE * TILE {
            let (a, b) = (u32::from(a[i * 4 + 3]), u32::from(b[i * 4 + 3]));
            let value = match mode {
                Mode::Add => a + b - (a * b + 127) / 255,
                Mode::Subtract => (a * (255 - b) + 127) / 255,
                Mode::Intersect => (a * b + 127) / 255,
                Mode::Replace => b,
            };
            tile[i * 4 + 3] = value as u8;
        }
        if !is_blank(&tile) {
            out.tiles.insert(key, tile);
        }
    }
    Ok(out)
}

/// Selected-area summary for `select-info` and command results.
pub fn summary(selection: &Surface) -> Value {
    let (mut pixels, mut full) = (0u64, 0u64);
    selection.for_each_painted(|_, _, p| {
        pixels += 1;
        if p[3] == 255 {
            full += 1;
        }
    });
    let bounds = selection.painted_bounds();
    json!({
        "width": selection.width,
        "height": selection.height,
        "selected_pixels": pixels,
        "fully_selected_pixels": full,
        "tiles": selection.tiles.len(),
        "bounds": bounds,
        "tile_map_sha256": selection.tile_map_hash(),
    })
}

/// Keep `post` only where `selection` covers: each pixel becomes the
/// coverage-weighted mix of its `pre` and `post` values (premultiplied).
pub(super) fn blend_through(pre: &Surface, post: &mut Surface, selection: &Surface) {
    let mut keys: Vec<_> = pre.tiles.keys().chain(post.tiles.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    let zero = vec![0u8; TILE_BYTES].into_boxed_slice();
    for key in keys {
        let a = pre.tiles.get(&key).unwrap_or(&zero);
        let b = post.tiles.get(&key).unwrap_or(&zero);
        if a == b {
            continue;
        }
        let mut out = a.clone();
        if let Some(cover) = selection.tiles.get(&key) {
            for i in 0..TILE * TILE {
                let c = u32::from(cover[i * 4 + 3]);
                if c == 0 {
                    continue;
                }
                let (pa, pb) = (&a[i * 4..i * 4 + 4], &b[i * 4..i * 4 + 4]);
                if c == 255 {
                    out[i * 4..i * 4 + 4].copy_from_slice(pb);
                    continue;
                }
                let (a0, a1) = (u32::from(pa[3]), u32::from(pb[3]));
                let alpha = (a0 * (255 - c) + a1 * c + 127) / 255;
                if alpha == 0 {
                    out[i * 4..i * 4 + 4].fill(0);
                    continue;
                }
                let den = u64::from(alpha) * 255;
                for ch in 0..3 {
                    let num = u64::from(pa[ch]) * u64::from(a0) * u64::from(255 - c)
                        + u64::from(pb[ch]) * u64::from(a1) * u64::from(c);
                    out[i * 4 + ch] = ((num + den / 2) / den).min(255) as u8;
                }
                out[i * 4 + 3] = alpha as u8;
            }
        }
        if is_blank(&out) {
            post.tiles.remove(&key);
        } else {
            post.tiles.insert(key, out);
        }
    }
}

/// `select-clear`: remove the layer's selection so every pixel is editable again.
pub fn clear_op(raw: &mut Value, id: &str) -> Result<Value> {
    if !all_rasters(raw).iter().any(|node| node["id"] == id) {
        bail!("[not-found] raster layer {id} was not found; list nodes with `pentool tree DOCUMENT --kind raster`")
    }
    let mut next = raw.clone();
    let had = clear(&mut next, id);
    collect_garbage(&mut next);
    *raw = next;
    Ok(json!({"id": id, "cleared": had}))
}

/// `select-info`: summary of the layer's selection.
pub fn info_op(raw: &Value, id: &str) -> Result<Value> {
    let Some(selection) = active(raw, id)? else {
        return Ok(json!({"id": id, "selection": null}));
    };
    let mut summary = summary(&selection);
    summary["id"] = json!(id);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(w: u32, h: u32, cells: &[(u32, u32, u8)]) -> Surface {
        let mut s = Surface::empty(w, h);
        for &(x, y, v) in cells {
            set_coverage(&mut s, x, y, v);
        }
        s
    }

    #[test]
    fn modes_combine_coverage() {
        let a = plane(8, 8, &[(1, 1, 255), (2, 2, 128)]);
        let b = plane(8, 8, &[(1, 1, 255), (2, 2, 128), (3, 3, 255)]);
        let add = combine(Mode::Add, Some(&a), &b).unwrap();
        assert_eq!(coverage(&add, 3, 3), 255);
        assert_eq!(coverage(&add, 2, 2), 192);
        let sub = combine(Mode::Subtract, Some(&a), &b).unwrap();
        assert_eq!(coverage(&sub, 1, 1), 0);
        let int = combine(Mode::Intersect, Some(&a), &b).unwrap();
        assert_eq!(coverage(&int, 2, 2), 64);
        assert!(combine(Mode::Subtract, None, &b).is_err());
    }

    #[test]
    fn blending_through_a_selection_is_proportional_and_exact_at_the_ends() {
        let mut pre = Surface::empty(8, 8);
        pre.set_pixel(1, 1, [0, 0, 255, 255]);
        let mut post = pre.clone();
        for x in 0..4 {
            post.set_pixel(x, 1, [255, 0, 0, 255]);
        }
        let selection = plane(8, 8, &[(0, 1, 255), (1, 1, 128), (2, 1, 0)]);
        blend_through(&pre, &mut post, &selection);
        assert_eq!(post.pixel(0, 1), [255, 0, 0, 255]);
        assert_eq!(post.pixel(1, 1), [128, 0, 127, 255]);
        assert_eq!(post.pixel(2, 1), [0; 4], "unselected stays as before");
        assert_eq!(post.pixel(3, 1), [0; 4]);
    }
}

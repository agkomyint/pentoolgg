//! Flood fill and magic-wand regions (flood algorithm 1).
//!
//! A region is computed on the layer's own pixels (`scope: layer`). A pixel is a
//! candidate when its largest straight-RGBA channel difference from the seed pixel
//! is at most `tolerance`; all fully transparent pixels count as one color, and
//! with `transparency: barrier` they never join the region of an opaque seed.
//! Contiguous mode grows from the seed over 4 or 8 neighbors; global mode takes
//! every candidate. A `gap` of N pixels first erodes the candidates by an N-pixel
//! square so thin breaks of up to 2N pixels stop the fill, then grows the result
//! back inside the original candidates. Anti-aliasing turns the one-pixel edge
//! into partial coverage with a 3x3 box average. Work is bounded before anything
//! is allocated, and nothing is committed until the whole region is known.
use super::selection::set_coverage;
use super::*;

pub const ALGORITHM: u64 = 1;
/// Largest layer area the fill allocates working planes for.
pub const MAX_PIXELS: u64 = 32 * 1024 * 1024;
pub const MAX_GAP: u32 = 8;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub x: u32,
    pub y: u32,
    pub tolerance: u8,
    pub diagonal: bool,
    pub contiguous: bool,
    pub antialias: bool,
    pub gap: u32,
    pub transparent_barrier: bool,
}

pub(super) fn candidate(p: [u8; 4], seed: [u8; 4], tolerance: u8, barrier: bool) -> bool {
    if p[3] == 0 && seed[3] == 0 {
        return true;
    }
    if barrier && (p[3] == 0) != (seed[3] == 0) {
        return false;
    }
    let da = p[3].abs_diff(seed[3]);
    let diff = if p[3] == 0 || seed[3] == 0 {
        da
    } else {
        da.max(p[0].abs_diff(seed[0]))
            .max(p[1].abs_diff(seed[1]))
            .max(p[2].abs_diff(seed[2]))
    };
    diff <= tolerance
}

/// Square erosion (`erode`) or dilation of a boolean plane; cells outside the
/// plane count as `outside`.
fn square(plane: &[u8], w: usize, h: usize, radius: usize, erode: bool) -> Vec<u8> {
    let outside = u8::from(erode);
    let pass = |input: &[u8], horizontal: bool| -> Vec<u8> {
        let mut out = vec![0u8; input.len()];
        for y in 0..h {
            for x in 0..w {
                let mut keep = true;
                let mut any = false;
                for d in 0..=2 * radius {
                    let at = if horizontal {
                        (x + d)
                            .checked_sub(radius)
                            .filter(|v| *v < w)
                            .map(|v| y * w + v)
                    } else {
                        (y + d)
                            .checked_sub(radius)
                            .filter(|v| *v < h)
                            .map(|v| v * w + x)
                    };
                    let v = at.map_or(outside, |i| input[i]);
                    keep &= v != 0;
                    any |= v != 0;
                }
                out[y * w + x] = u8::from(if erode { keep } else { any });
            }
        }
        out
    };
    pass(&pass(plane, true), false)
}

/// Compute the coverage plane (`[0,0,0,coverage]` tiles) of a region.
pub fn region(surface: &Surface, options: &Options) -> Result<Surface> {
    let (w, h) = (surface.width as usize, surface.height as usize);
    if u64::from(surface.width) * u64::from(surface.height) > MAX_PIXELS {
        bail!("[limit-exceeded] a {}x{} layer exceeds the {MAX_PIXELS}-pixel fill limit; crop it or fill a smaller layer", surface.width, surface.height)
    }
    if options.x >= surface.width || options.y >= surface.height {
        bail!(
            "[invalid-fill] seed ({}, {}) is outside the {}x{} layer",
            options.x,
            options.y,
            surface.width,
            surface.height
        )
    }
    if options.gap > MAX_GAP {
        bail!("[invalid-fill] gap must be 0-{MAX_GAP} pixels")
    }
    if options.gap > 0 && !options.contiguous {
        bail!("[invalid-fill] gap only applies to contiguous fills")
    }
    let seed = surface.pixel(options.x, options.y);
    let mut cand = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = surface.pixel(x as u32, y as u32);
            cand[y * w + x] = u8::from(candidate(
                p,
                seed,
                options.tolerance,
                options.transparent_barrier,
            ));
        }
    }
    let gap = options.gap as usize;
    let walls = if gap > 0 {
        square(&cand, w, h, gap, true)
    } else {
        cand.clone()
    };
    let start = options.y as usize * w + options.x as usize;
    let mut inside = vec![0u8; w * h];
    if options.contiguous {
        if walls[start] == 0 {
            bail!("[invalid-fill] the seed lies within a gap of {gap} pixels; lower gap or seed inside the area")
        }
        let mut stack = vec![start];
        inside[start] = 1;
        while let Some(at) = stack.pop() {
            let (x, y) = ((at % w) as i64, (at / w) as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if (dx, dy) == (0, 0) || (!options.diagonal && dx != 0 && dy != 0) {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if walls[n] != 0 && inside[n] == 0 {
                        inside[n] = 1;
                        stack.push(n);
                    }
                }
            }
        }
        if gap > 0 {
            let grown = square(&inside, w, h, gap, false);
            for i in 0..w * h {
                inside[i] = grown[i] & cand[i];
            }
        }
    } else {
        inside = walls;
    }
    let mut out = Surface::empty(surface.width, surface.height);
    let count = |x: i64, y: i64| -> u32 {
        let mut n = 0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64 {
                    n += u32::from(inside[ny as usize * w + nx as usize]);
                } else {
                    n += u32::from(
                        inside[(y.clamp(0, h as i64 - 1) as usize) * w
                            + x.clamp(0, w as i64 - 1) as usize],
                    );
                }
            }
        }
        n
    };
    for y in 0..h {
        for x in 0..w {
            let value = if options.antialias {
                let n = count(x as i64, y as i64);
                ((n * 255 + 4) / 9) as u8
            } else {
                if inside[y * w + x] == 0 {
                    continue;
                }
                255
            };
            if value > 0 {
                set_coverage(&mut out, x as u32, y as u32, value);
            }
        }
    }
    Ok(out)
}

/// Fill the region with a color, limited by the active selection. Like the other
/// non-stroke layer operations it rolls the layer's checkpoint.
pub fn fill(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    color: [u8; 3],
    opacity: f64,
    options: &Options,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    if !(0.0..=1.0).contains(&opacity) || !opacity.is_finite() {
        bail!("[invalid-fill] opacity must be between 0 and 1")
    }
    let mut next = raw.clone();
    let snapshot = locate(&mut next, page, id)?.clone();
    let mut surface = Surface::load(&next, &snapshot)?;
    let coverage = region(&surface, options)?;
    let selection = selection::pin(&next, id, surface.width, surface.height)?;
    let opacity16 = (opacity * 65535.0).round() as u32;
    let mut changed = 0u64;
    for (key, plane) in &coverage.tiles {
        let limit = selection.as_ref().map(|(s, _)| s.tiles.get(key));
        let tile = surface
            .tiles
            .entry(*key)
            .or_insert_with(|| vec![0u8; TILE_BYTES].into_boxed_slice());
        for index in 0..TILE * TILE {
            let mut cover = u32::from(plane[index * 4 + 3]);
            if cover == 0 {
                continue;
            }
            if let Some(limit) = limit {
                cover = (cover * limit.map_or(0, |t| u32::from(t[index * 4 + 3])) + 127) / 255;
            }
            let alpha16 = (cover * opacity16 + 127) / 255;
            if alpha16 == 0 {
                continue;
            }
            composite(tile, index * 4, color, alpha16.min(65535), Blend::Normal);
            changed += 1;
        }
    }
    surface.tiles.retain(|_, tile| !is_blank(tile));
    let mut node = snapshot;
    let hash = layer::commit_surface(&mut next, &mut node, &surface, "fill")?;
    *locate(&mut next, page, id)? = node;
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": id,
        "algorithm": ALGORITHM,
        "pixels_filled": changed,
        "tile_map_sha256": hash,
        "tiles_released": released,
        "selection_applied": selection.is_some(),
    }))
}

/// Magic wand: turn a flood region into the layer's selection.
pub fn wand(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    options: &Options,
    mode: selection::Mode,
) -> Result<Value> {
    let node = locate(&mut raw.clone(), page, id)?.clone();
    let surface = Surface::load(raw, &node)?;
    let coverage = region(&surface, options)?;
    let mut next = raw.clone();
    let old = selection::active(&next, id)?;
    if let Some(old) = &old {
        if (old.width, old.height) != (surface.width, surface.height) {
            bail!("[invalid-selection] the selection of {id} does not match the layer; run `select-clear {id}` first")
        }
    }
    let merged = selection::combine(mode, old.as_ref(), &coverage)?;
    selection::store_active(&mut next, id, &merged)?;
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    let mut result = selection::summary(&merged);
    result["id"] = json!(id);
    result["algorithm"] = json!(ALGORITHM);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white 20x10 layer split by a black vertical wall at x = 10, with a hole.
    fn layer(hole: Option<u32>) -> Surface {
        let mut s = Surface::empty(20, 10);
        for y in 0..10 {
            for x in 0..20 {
                s.set_pixel(x, y, [255, 255, 255, 255]);
            }
            if hole != Some(y) {
                s.set_pixel(10, y, [0, 0, 0, 255]);
            }
        }
        s
    }

    fn options(x: u32, y: u32) -> Options {
        Options {
            x,
            y,
            tolerance: 10,
            diagonal: false,
            contiguous: true,
            antialias: false,
            gap: 0,
            transparent_barrier: false,
        }
    }

    fn selected(s: &Surface) -> u32 {
        let mut n = 0;
        s.for_each_painted(|_, _, _| n += 1);
        n
    }

    #[test]
    fn contiguous_stops_at_the_wall_and_global_does_not() {
        let s = layer(None);
        assert_eq!(selected(&region(&s, &options(2, 2)).unwrap()), 10 * 10);
        let mut global = options(2, 2);
        global.contiguous = false;
        assert_eq!(selected(&region(&s, &global).unwrap()), 19 * 10);
    }

    #[test]
    fn a_gap_policy_blocks_leaks_through_small_holes() {
        let s = layer(Some(5));
        assert_eq!(selected(&region(&s, &options(2, 2)).unwrap()), 191);
        let mut gapped = options(2, 2);
        gapped.gap = 1;
        let left = region(&s, &gapped).unwrap();
        assert_eq!(
            coverage_at(&left, 15, 5),
            0,
            "did not leak through the hole"
        );
        assert_eq!(coverage_at(&left, 9, 5), 255, "still fills up to the wall");
        assert!(region(&s, &Options { gap: 9, ..gapped }).is_err());
    }

    fn coverage_at(s: &Surface, x: u32, y: u32) -> u8 {
        s.pixel(x, y)[3]
    }

    #[test]
    fn tolerance_antialias_and_bounds() {
        let mut s = Surface::empty(10, 10);
        for y in 0..10 {
            for x in 0..10 {
                s.set_pixel(x, y, [100 + x as u8, 100, 100, 255]);
            }
        }
        let mut o = options(0, 0);
        o.tolerance = 3;
        assert_eq!(selected(&region(&s, &o).unwrap()), 4 * 10);
        o.antialias = true;
        let soft = region(&s, &o).unwrap();
        assert_eq!(coverage_at(&soft, 0, 5), 255);
        assert!(coverage_at(&soft, 4, 5) > 0 && coverage_at(&soft, 4, 5) < 255);
        assert!(region(&s, &options(10, 0)).is_err());
    }

    #[test]
    fn transparent_pixels_match_each_other_and_can_be_barriers() {
        let mut s = Surface::empty(6, 1);
        s.set_pixel(0, 0, [10, 10, 10, 255]);
        s.set_pixel(1, 0, [10, 10, 10, 200]);
        let mut o = options(0, 0);
        o.tolerance = 255;
        assert_eq!(selected(&region(&s, &o).unwrap()), 6);
        o.transparent_barrier = true;
        assert_eq!(selected(&region(&s, &o).unwrap()), 2);
        assert_eq!(selected(&region(&s, &options(5, 0)).unwrap()), 4);
    }
}

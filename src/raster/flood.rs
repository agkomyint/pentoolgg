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
//!
//! Two options make the wand usable on photographs. `sample` averages an odd
//! square of pixels around the seed so one noisy pixel does not decide the region.
//! `gradient` switches contiguous growth from "close to the seed" to "close to the
//! neighbor it grew from" (a step tolerance), so a sky, studio sweep or shaded
//! surface selects as one region; `leash` still caps the distance from the seed.
use super::selection::set_coverage;
use super::*;

pub const ALGORITHM: u64 = 1;
/// Largest layer area the fill allocates working planes for.
pub const MAX_PIXELS: u64 = 32 * 1024 * 1024;
pub const MAX_GAP: u32 = 8;
/// Largest seed-averaging window edge in pixels (odd).
pub const MAX_SAMPLE: u32 = 51;

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
    /// Edge of the square averaged around the seed (odd, 1 = the seed pixel only).
    pub sample: u32,
    /// Step tolerance between neighbors; 0 keeps the classic seed-relative test.
    pub gradient: u8,
    /// With `gradient`, the largest difference from the seed any pixel may have.
    pub leash: u8,
}

impl Options {
    /// Classic wand/fill options: seed pixel, no gradient following.
    pub fn at(x: u32, y: u32, tolerance: u8) -> Self {
        Options {
            x,
            y,
            tolerance,
            diagonal: false,
            contiguous: true,
            antialias: true,
            gap: 0,
            transparent_barrier: false,
            sample: 1,
            gradient: 0,
            leash: 255,
        }
    }
}

/// The reference color of a region: the seed pixel, or the mean of the odd
/// `sample` x `sample` window around it (color over visible pixels, alpha over all).
fn seed_color(surface: &Surface, x: u32, y: u32, sample: u32) -> [u8; 4] {
    if sample <= 1 {
        return surface.pixel(x, y);
    }
    let r = i64::from(sample / 2);
    let (mut sum, mut seen, mut alpha, mut total) = ([0u64; 3], 0u64, 0u64, 0u64);
    for dy in -r..=r {
        for dx in -r..=r {
            let (nx, ny) = (i64::from(x) + dx, i64::from(y) + dy);
            if nx < 0 || ny < 0 || nx >= i64::from(surface.width) || ny >= i64::from(surface.height)
            {
                continue;
            }
            let p = surface.pixel(nx as u32, ny as u32);
            total += 1;
            alpha += u64::from(p[3]);
            if p[3] > 0 {
                seen += 1;
                for c in 0..3 {
                    sum[c] += u64::from(p[c]);
                }
            }
        }
    }
    if seen == 0 {
        return [0, 0, 0, 0];
    }
    let mean = |v: u64, n: u64| ((v + n / 2) / n) as u8;
    [
        mean(sum[0], seen),
        mean(sum[1], seen),
        mean(sum[2], seen),
        mean(alpha, total),
    ]
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
    if options.sample == 0 || options.sample % 2 == 0 || options.sample > MAX_SAMPLE {
        bail!("[invalid-fill] sample must be an odd window edge of 1-{MAX_SAMPLE} pixels")
    }
    if options.gradient > 0 && (!options.contiguous || options.gap > 0) {
        bail!("[invalid-fill] gradient only applies to contiguous fills without a gap")
    }
    let seed = seed_color(surface, options.x, options.y, options.sample);
    let mut cand = vec![0u8; w * h];
    if options.gradient == 0 {
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
    }
    let gap = options.gap as usize;
    let walls = if gap > 0 {
        square(&cand, w, h, gap, true)
    } else {
        cand.clone()
    };
    let start = options.y as usize * w + options.x as usize;
    let mut inside = vec![0u8; w * h];
    if options.gradient > 0 {
        let step = |from: usize, to: usize| {
            let at = |i: usize| surface.pixel((i % w) as u32, (i / w) as u32);
            let p = at(to);
            candidate(p, at(from), options.gradient, options.transparent_barrier)
                && candidate(p, seed, options.leash, options.transparent_barrier)
        };
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
                    if inside[n] == 0 && step(at, n) {
                        inside[n] = 1;
                        stack.push(n);
                    }
                }
            }
        }
    } else if options.contiguous {
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
    fill_sampled(raw, page, id, color, opacity, options, None)
}

/// [`fill`] with the region measured on `sample` (a same-size view such as the
/// composite page, `scope: composite`) instead of the layer's own pixels.
pub fn fill_sampled(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    color: [u8; 3],
    opacity: f64,
    options: &Options,
    sample: Option<&Surface>,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    if !(0.0..=1.0).contains(&opacity) || !opacity.is_finite() {
        bail!("[invalid-fill] opacity must be between 0 and 1")
    }
    let mut next = raw.clone();
    let snapshot = locate(&mut next, page, id)?.clone();
    let mut surface = Surface::load(&next, &snapshot)?;
    let coverage = region(sample.unwrap_or(&surface), options)?;
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
    wand_sampled(raw, page, id, options, mode, None)
}

/// [`wand`] with the region measured on `sample`; see [`fill_sampled`].
pub fn wand_sampled(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    options: &Options,
    mode: selection::Mode,
    sample: Option<&Surface>,
) -> Result<Value> {
    let node = locate(&mut raw.clone(), page, id)?.clone();
    let surface = Surface::load(raw, &node)?;
    let coverage = region(sample.unwrap_or(&surface), options)?;
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
    result["scope"] = json!(if sample.is_some() {
        "composite"
    } else {
        "layer"
    });
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
            antialias: false,
            ..Options::at(x, y, 10)
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

    /// A 60x40 sky that darkens by 60 levels top to bottom, with +-3 noise and a
    /// red 20x12 car; returns the surface and the car's pixel count.
    fn sky_and_car() -> (Surface, u32) {
        let mut s = Surface::empty(60, 40);
        let mut noise = 7u64;
        let mut car = 0;
        for y in 0..40u32 {
            for x in 0..60u32 {
                noise = splitmix64(&mut noise);
                let n = (noise % 7) as i32 - 3;
                let sky = [
                    200 - 60 * y as i32 / 40 + n,
                    215 - 50 * y as i32 / 40 + n,
                    235 + n,
                ];
                let in_car = (20..40).contains(&x) && (22..34).contains(&y);
                car += u32::from(in_car);
                let c = if in_car {
                    [190, 30, 35]
                } else {
                    sky.map(|v| v.clamp(0, 255) as u8)
                };
                s.set_pixel(x, y, [c[0], c[1], c[2], 255]);
            }
        }
        (s, car)
    }

    #[test]
    fn gradient_flood_selects_a_noisy_gradient_sky_without_entering_the_car() {
        let (s, car) = sky_and_car();
        let sky = 60 * 40 - car;
        // Seed-relative tolerance cannot do it: tight stops early, loose leaks.
        let tight = selected(&region(&s, &options(2, 2)).unwrap());
        assert!(
            tight < sky,
            "tight tolerance stops partway down the gradient"
        );
        let mut loose = options(2, 2);
        loose.tolerance = 220;
        assert!(
            selected(&region(&s, &loose).unwrap()) > sky,
            "loose floods the car"
        );
        let mut o = options(2, 2);
        o.gradient = 20;
        o.sample = 3;
        let grown = region(&s, &o).unwrap();
        assert_eq!(selected(&grown), sky);
        assert_eq!(coverage_at(&grown, 30, 28), 0, "the car stays unselected");
    }

    #[test]
    fn gradient_and_sample_are_validated() {
        let (s, _) = sky_and_car();
        let mut o = options(2, 2);
        o.gradient = 10;
        o.contiguous = false;
        assert!(region(&s, &o).is_err());
        o.contiguous = true;
        o.gap = 1;
        assert!(region(&s, &o).is_err());
        let mut o = options(2, 2);
        o.sample = 4;
        assert!(region(&s, &o).is_err());
        o.sample = MAX_SAMPLE + 2;
        assert!(region(&s, &o).is_err());
    }

    #[test]
    fn sample_averaging_ignores_a_noisy_seed_pixel() {
        let mut s = Surface::empty(9, 9);
        for y in 0..9 {
            for x in 0..9 {
                s.set_pixel(x, y, [100, 100, 100, 255]);
            }
        }
        s.set_pixel(4, 4, [130, 100, 100, 255]);
        let mut o = options(4, 4);
        assert_eq!(
            selected(&region(&s, &o).unwrap()),
            1,
            "the odd seed stands alone"
        );
        o.sample = 5;
        o.tolerance = 30;
        assert_eq!(selected(&region(&s, &o).unwrap()), 81);
    }
}

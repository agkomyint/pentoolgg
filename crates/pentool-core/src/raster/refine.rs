//! Edge-aware selection refinement (selection algorithm 2).
//!
//! A rough selection (a loose marquee, lasso or wand result) is snapped to image
//! edges. The rough shape defines a trimap: pixels deeper than `band` inside it are
//! certainly subject, pixels farther than `band` outside it are certainly
//! background, and the ring between them is undecided. Optional `--fg` / `--bg`
//! hint strokes pin extra pixels. Every undecided pixel then goes to whichever
//! side reaches it with the cheaper path, where a step costs more across strong
//! color edges and when the pixel looks like the other side's color histogram, so
//! the boundary settles on the nearest edge. Only integer arithmetic, a
//! tie-broken priority queue and 4-connected steps are used, so the result is
//! identical on every platform.
//!
//! A loose box is a poor trimap because its corners are background: `core` pulls
//! the certain-subject area further in than `band`, and two cleansing rounds then
//! demote core pixels whose color is far more typical of the certain background
//! than of the rest of the core.
use super::flood::MAX_PIXELS;
use super::selection;
use super::shape::{box_blur, ensure_plane, from_plane, layer_size, morph, to_plane};
use super::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub const ALGORITHM: u64 = 2;
pub const MAX_BAND: u32 = 64;
pub const MAX_HINT_POINTS: usize = 4_096;
const EDGE_WEIGHT: u32 = 4;
const MODEL_PENALTY: u32 = 60;

const FOREGROUND: u8 = 1;
const BACKGROUND: u8 = 2;

fn diff(a: [u8; 4], b: [u8; 4]) -> u32 {
    (0..4)
        .map(|c| u32::from(a[c].abs_diff(b[c])))
        .max()
        .unwrap_or(0)
}

fn bin(p: [u8; 4]) -> usize {
    (usize::from(p[0] >> 4) << 8) | (usize::from(p[1] >> 4) << 4) | usize::from(p[2] >> 4)
}

/// Pixels along the polyline through `points`, clamped to the layer; a single
/// point becomes a 3x3 dab.
pub fn stroke_pixels(points: &[[f64; 2]], w: u32, h: u32) -> Result<Vec<usize>> {
    if points.len() > MAX_HINT_POINTS {
        bail!("[limit-exceeded] a hint takes at most {MAX_HINT_POINTS} points")
    }
    let mut out = Vec::new();
    let mut put = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && x < i64::from(w) && y < i64::from(h) {
            out.push(y as usize * w as usize + x as usize);
        }
    };
    let snapped: Vec<(i64, i64)> = points
        .iter()
        .map(|p| {
            if !p[0].is_finite() || !p[1].is_finite() {
                bail!("[invalid-input] hint points must be finite numbers")
            }
            Ok((p[0].floor() as i64, p[1].floor() as i64))
        })
        .collect::<Result<_>>()?;
    for (i, &(x, y)) in snapped.iter().enumerate() {
        if snapped.len() == 1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    put(x + dx, y + dy);
                }
            }
            break;
        }
        let Some(&(nx, ny)) = snapped.get(i + 1) else {
            break;
        };
        let steps = (nx - x).abs().max((ny - y).abs()).max(1);
        for s in 0..=steps {
            put(x + (nx - x) * s / steps, y + (ny - y) * s / steps);
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Refine `plane` (coverage 0-255, `w` x `h`) against the pixels of `surface`.
/// Returns the refined plane and the number of undecided pixels that were resolved.
#[allow(clippy::too_many_arguments)]
pub fn refine_plane(
    surface: &Surface,
    plane: &[u8],
    w: usize,
    h: usize,
    band: u32,
    core: u32,
    fg_hint: &[usize],
    bg_hint: &[usize],
) -> Result<(Vec<u8>, u64)> {
    if band == 0 || band > MAX_BAND {
        bail!("[invalid-selection] band must be 1-{MAX_BAND} pixels")
    }
    if core > MAX_BAND {
        bail!("[invalid-selection] core must be 0-{MAX_BAND} pixels (0 uses the band)")
    }
    let core = if core == 0 { band } else { core.max(band) };
    let solid: Vec<u8> = plane
        .iter()
        .map(|v| if *v >= 128 { 255 } else { 0 })
        .collect();
    if solid.iter().all(|v| *v == 0) {
        bail!("[invalid-selection] the selection is empty; there is nothing to refine")
    }
    let b = band as usize;
    let inner = morph(&solid, w, h, core as usize, false);
    let reach = morph(&solid, w, h, b, true);
    let mut label = vec![0u8; w * h];
    for i in 0..w * h {
        if inner[i] == 255 {
            label[i] = FOREGROUND;
        } else if reach[i] == 0 {
            label[i] = BACKGROUND;
        }
    }
    for &i in fg_hint {
        label[i] = FOREGROUND;
    }
    for &i in bg_hint {
        if fg_hint.binary_search(&i).is_ok() {
            bail!("[invalid-selection] a foreground and a background hint cover the same pixel")
        }
        label[i] = BACKGROUND;
    }
    let count = |l: u8| label.iter().filter(|v| **v == l).count();
    if count(FOREGROUND) == 0 {
        bail!("[invalid-selection] no certain foreground remains {band} pixels inside the selection; lower --band or add --fg hints")
    }
    if count(BACKGROUND) == 0 {
        bail!("[invalid-selection] no certain background remains {band} pixels outside the selection; lower --band or add --bg hints")
    }
    let pixels: Vec<[u8; 4]> = (0..w * h)
        .map(|i| surface.pixel((i % w) as u32, (i / w) as u32))
        .collect();
    let build = |label: &[u8]| {
        let mut histogram = [vec![0u64; 4096], vec![0u64; 4096]];
        let mut totals = [0u64; 2];
        for (i, l) in label.iter().enumerate() {
            if *l != 0 {
                let side = usize::from(*l - 1);
                histogram[side][bin(pixels[i])] += 1;
                totals[side] += 1;
            }
        }
        (histogram, totals)
    };
    let (mut histogram, mut totals) = build(&label);
    if core > band {
        for _ in 0..2 {
            let mut demoted = false;
            for i in 0..w * h {
                if label[i] != FOREGROUND || fg_hint.binary_search(&i).is_ok() {
                    continue;
                }
                let k = bin(pixels[i]);
                if u128::from(histogram[1][k]) * u128::from(totals[0])
                    >= 4 * u128::from(histogram[0][k]) * u128::from(totals[1])
                {
                    label[i] = 0;
                    demoted = true;
                }
            }
            if !demoted {
                break;
            }
            (histogram, totals) = build(&label);
        }
        if !label.contains(&FOREGROUND) {
            bail!("[invalid-selection] the subject core is indistinguishable from the background; add --fg hints")
        }
    }
    // Pixels that look like the other side's colors pay a penalty to join.
    let looks_like = |i: usize, side: usize| {
        let k = bin(pixels[i]);
        let (mine, other) = (histogram[side][k], histogram[1 - side][k]);
        u128::from(other) * u128::from(totals[side])
            > u128::from(mine) * u128::from(totals[1 - side])
    };

    // Sensor noise or texture must not look like an edge: measure how much
    // neighboring pixels differ inside the certain regions and charge only for
    // differences above that floor (the 90th percentile).
    let floor = {
        let mut counts = [0u64; 256];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if label[i] == 0 {
                    continue;
                }
                for n in [i + 1, i + w] {
                    let inside = if n == i + 1 { x + 1 < w } else { y + 1 < h };
                    if inside && label[n] == label[i] {
                        counts[diff(pixels[i], pixels[n]) as usize] += 1;
                    }
                }
            }
        }
        let total: u64 = counts.iter().sum();
        let mut seen = 0;
        counts
            .iter()
            .position(|c| {
                seen += c;
                seen * 10 >= total * 9
            })
            .unwrap_or(0) as u32
    };
    let undecided = label.iter().filter(|v| **v == 0).count() as u64;
    let race = |side: usize| -> Vec<u32> {
        let own = side as u8 + 1;
        let mut dist = vec![u32::MAX; w * h];
        let mut heap = BinaryHeap::new();
        for (i, l) in label.iter().enumerate() {
            if *l == own {
                dist[i] = 0;
                heap.push(Reverse((0u32, i as u32)));
            }
        }
        while let Some(Reverse((d, at))) = heap.pop() {
            let at = at as usize;
            if d > dist[at] {
                continue;
            }
            let (x, y) = (at % w, at / w);
            let mut visit = |n: usize| {
                let l = label[n];
                if l != 0 && l != own {
                    return;
                }
                let mut step = 1 + EDGE_WEIGHT * diff(pixels[at], pixels[n]).saturating_sub(floor);
                if l == 0 && looks_like(n, side) {
                    step += MODEL_PENALTY;
                }
                let next = d.saturating_add(step);
                if next < dist[n] {
                    dist[n] = next;
                    heap.push(Reverse((next, n as u32)));
                }
            };
            if y > 0 {
                visit(at - w);
            }
            if x > 0 {
                visit(at - 1);
            }
            if x + 1 < w {
                visit(at + 1);
            }
            if y + 1 < h {
                visit(at + w);
            }
        }
        dist
    };
    let (fg, bg) = (race(0), race(1));
    let hard: Vec<u8> = (0..w * h)
        .map(|i| match label[i] {
            FOREGROUND => 255,
            BACKGROUND => 0,
            _ if fg[i] < bg[i] => 255,
            _ if fg[i] > bg[i] => 0,
            _ => solid[i],
        })
        .collect();
    // One pixel of anti-aliasing on the settled edge.
    Ok((box_blur(&hard, w, h, 1), undecided))
}

/// Refine the active selection of raster layer `id` to image edges.
pub fn refine(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    band: u32,
    core: u32,
    fg_points: &[[f64; 2]],
    bg_points: &[[f64; 2]],
) -> Result<Value> {
    let (w, h) = layer_size(raw, page, id)?;
    ensure_plane(w, h)?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS / 2 {
        bail!("[limit-exceeded] refining a {w}x{h} layer needs too much work; crop it first")
    }
    let Some(old) = selection::active(raw, id)? else {
        bail!("[invalid-selection] {id} has no selection to refine; make a rough one with select-marquee, select-lasso or select-wand")
    };
    if (old.width, old.height) != (w, h) {
        bail!("[invalid-selection] the selection of {id} does not match the layer; run `select-clear {id}` first")
    }
    let node = locate(&mut raw.clone(), page, id)?.clone();
    let surface = Surface::load(raw, &node)?;
    let fg = stroke_pixels(fg_points, w, h)?;
    let bg = stroke_pixels(bg_points, w, h)?;
    let (plane, undecided) = refine_plane(
        &surface,
        &to_plane(&old),
        w as usize,
        h as usize,
        band,
        core,
        &fg,
        &bg,
    )?;
    let updated = from_plane(w, h, &plane);
    let mut next = raw.clone();
    selection::store_active(&mut next, id, &updated)?;
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    let mut result = selection::summary(&updated);
    result["id"] = json!(id);
    result["algorithm"] = json!(ALGORITHM);
    result["band"] = json!(band);
    result["core"] = json!(core.max(band));
    result["undecided_pixels"] = json!(undecided);
    result["foreground_hint_pixels"] = json!(fg.len());
    result["background_hint_pixels"] = json!(bg.len());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noisy gradient sky with an elliptical red car; returns the surface and the
    /// true car mask.
    fn scene() -> (Surface, Vec<u8>) {
        let (w, h) = (96usize, 64usize);
        let mut s = Surface::empty(w as u32, h as u32);
        let mut truth = vec![0u8; w * h];
        let mut noise = 11u64;
        for y in 0..h {
            for x in 0..w {
                let n = (math::splitmix64(&mut noise) % 9) as i32 - 4;
                let (dx, dy) = (x as i32 - 48, y as i32 - 34);
                let car = (dx * dx * 15 * 15 + dy * dy * 24 * 24) <= 24 * 24 * 15 * 15;
                truth[y * w + x] = if car { 255 } else { 0 };
                let c = if car {
                    [190 + n, 40 + n, 45 + n]
                } else {
                    [
                        120 + 60 * y as i32 / h as i32 + n,
                        170 + n,
                        230 - 40 * y as i32 / h as i32 + n,
                    ]
                };
                let c = c.map(|v| v.clamp(0, 255) as u8);
                s.set_pixel(x as u32, y as u32, [c[0], c[1], c[2], 255]);
            }
        }
        (s, truth)
    }

    fn iou(a: &[u8], b: &[u8]) -> f64 {
        let (mut both, mut either) = (0u64, 0u64);
        for (x, y) in a.iter().zip(b) {
            let (x, y) = (*x >= 128, *y >= 128);
            both += u64::from(x && y);
            either += u64::from(x || y);
        }
        both as f64 / either as f64
    }

    #[test]
    fn a_loose_selection_snaps_to_the_car_edge() {
        let (s, truth) = scene();
        let (w, h) = (96, 64);
        let loose = morph(&truth, w, h, 5, true);
        assert!(
            iou(&loose, &truth) < 0.9,
            "the rough shape is visibly loose"
        );
        let (refined, undecided) = refine_plane(&s, &loose, w, h, 9, 0, &[], &[]).unwrap();
        assert!(undecided > 0);
        let score = iou(&refined, &truth);
        assert!(score >= 0.95, "iou {score}");
        // Deterministic: a second run is identical.
        let again = refine_plane(&s, &loose, w, h, 9, 0, &[], &[]).unwrap().0;
        assert_eq!(refined, again);
    }

    #[test]
    fn hints_correct_the_result_and_conflicts_are_refused() {
        let (s, truth) = scene();
        let (w, h) = (96, 64);
        let loose = morph(&truth, w, h, 5, true);
        let spot = stroke_pixels(&[[48.0, 34.0]], 96, 64).unwrap();
        assert_eq!(spot.len(), 9);
        assert!(refine_plane(&s, &loose, w, h, 9, 0, &spot, &spot).is_err());
        let line = stroke_pixels(&[[10.0, 10.0], [14.0, 10.0]], 96, 64).unwrap();
        assert_eq!(line.len(), 5);
        let bg = stroke_pixels(&[[48.0, 34.0]], 96, 64).unwrap();
        let (forced, _) = refine_plane(&s, &loose, w, h, 9, 0, &[], &bg).unwrap();
        assert_eq!(forced[34 * w + 48], 0, "a background hint wins");
    }

    #[test]
    fn empty_selections_and_oversized_bands_are_refused() {
        let (s, truth) = scene();
        let (w, h) = (96, 64);
        assert!(refine_plane(&s, &vec![0; w * h], w, h, 5, 0, &[], &[]).is_err());
        assert!(refine_plane(&s, &truth, w, h, 0, 0, &[], &[]).is_err());
        assert!(refine_plane(&s, &truth, w, h, MAX_BAND + 1, 0, &[], &[]).is_err());
        // Everything selected: no certain background exists.
        assert!(refine_plane(&s, &vec![255; w * h], w, h, 5, 0, &[], &[]).is_err());
    }
}

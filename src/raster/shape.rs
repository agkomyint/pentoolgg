//! Selection shapes and modifiers (selection algorithm 1).
//!
//! Marquee and lasso shapes are rasterized with 4x4 sub-pixel sampling, so edges
//! carry 8-bit partial coverage. Modifiers work on a dense 8-bit coverage plane:
//! `feather` and `smooth` use three or one box blur(s), `expand`, `contract` and
//! `border` use a square structuring element, `grow` and `similar` read the
//! layer's own pixels. Everything uses integer math or IEEE `+ - * /`, `floor`
//! and `sqrt`-free arithmetic, so results are identical on every platform.
use super::flood::{candidate, MAX_PIXELS};
use super::selection::{self, Mode};
use super::*;

pub const ALGORITHM: u64 = 1;
pub const MAX_POINTS: usize = 4_096;
pub const MAX_MORPH: u32 = 64;
pub const MAX_BLUR: u32 = 256;
pub const MAX_SAVED: usize = 32;
const MAX_SIMILAR_COLORS: usize = 64;
const MAX_SIMILAR_WORK: u64 = 1 << 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Feather,
    Expand,
    Contract,
    Smooth,
    Border,
    Grow,
    Similar,
    Invert,
}

impl Op {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "feather" => Self::Feather,
            "expand" => Self::Expand,
            "contract" => Self::Contract,
            "smooth" => Self::Smooth,
            "border" => Self::Border,
            "grow" => Self::Grow,
            "similar" => Self::Similar,
            "invert" => Self::Invert,
            other => bail!("[invalid-selection] op {other:?} must be feather, expand, contract, smooth, border, grow, similar or invert"),
        })
    }
}

pub(super) fn ensure_plane(width: u32, height: u32) -> Result<()> {
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        bail!("[limit-exceeded] a {width}x{height} layer exceeds the {MAX_PIXELS}-pixel selection limit; crop it first")
    }
    Ok(())
}

pub(super) fn to_plane(selection: &Surface) -> Vec<u8> {
    let w = selection.width as usize;
    let mut plane = vec![0u8; w * selection.height as usize];
    selection.for_each_painted(|x, y, p| plane[y as usize * w + x as usize] = p[3]);
    plane
}

pub(super) fn from_plane(width: u32, height: u32, plane: &[u8]) -> Surface {
    let mut out = Surface::empty(width, height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let value = plane[y * width as usize + x];
            if value != 0 {
                selection::set_coverage(&mut out, x as u32, y as u32, value);
            }
        }
    }
    out
}

fn layer_size(raw: &Value, page: Option<&str>, id: &str) -> Result<(u32, u32)> {
    let node = locate(&mut raw.clone(), page, id)?.clone();
    if !is_raster(&node) {
        bail!("[invalid-input] {id} is not a raster layer")
    }
    let dim = |key: &str| node[key].as_u64().unwrap_or(0) as u32;
    Ok((dim("width"), dim("height")))
}

/// Combine a new plane with the layer's selection and store it.
fn install(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    mode: Mode,
    build: impl FnOnce(u32, u32) -> Result<Surface>,
) -> Result<Value> {
    let (w, h) = layer_size(raw, page, id)?;
    ensure_plane(w, h)?;
    let new = build(w, h)?;
    let mut next = raw.clone();
    let old = selection::active(&next, id)?;
    if let Some(old) = &old {
        if (old.width, old.height) != (w, h) {
            bail!("[invalid-selection] the selection of {id} does not match the layer; run `select-clear {id}` first")
        }
    }
    let merged = selection::combine(mode, old.as_ref(), &new)?;
    selection::store_active(&mut next, id, &merged)?;
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    let mut result = selection::summary(&merged);
    result["id"] = json!(id);
    result["algorithm"] = json!(ALGORITHM);
    Ok(result)
}

fn box_blur(plane: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let n = (2 * r + 1) as u32;
    let clamp = |v: i64, max: usize| v.clamp(0, max as i64 - 1) as usize;
    let pass = |input: &[u8], horizontal: bool| -> Vec<u8> {
        let mut out = vec![0u8; input.len()];
        let (lines, len) = if horizontal { (h, w) } else { (w, h) };
        let at = |line: usize, i: usize| {
            if horizontal {
                line * w + i
            } else {
                i * w + line
            }
        };
        for line in 0..lines {
            let mut sum: u32 = (-(r as i64)..=r as i64)
                .map(|d| u32::from(input[at(line, clamp(d, len))]))
                .sum();
            for i in 0..len {
                out[at(line, i)] = ((sum + n / 2) / n) as u8;
                let add = clamp(i as i64 + r as i64 + 1, len);
                let drop = clamp(i as i64 - r as i64, len);
                sum = sum + u32::from(input[at(line, add)]) - u32::from(input[at(line, drop)]);
            }
        }
        out
    };
    pass(&pass(plane, true), false)
}

fn morph(plane: &[u8], w: usize, h: usize, r: usize, grow: bool) -> Vec<u8> {
    let pass = |input: &[u8], horizontal: bool| -> Vec<u8> {
        let mut out = vec![0u8; input.len()];
        let (lines, len) = if horizontal { (h, w) } else { (w, h) };
        for line in 0..lines {
            for i in 0..len {
                let mut best = if grow { 0u8 } else { 255u8 };
                for d in i.saturating_sub(r)..=(i + r).min(len - 1) {
                    let v = if horizontal {
                        input[line * w + d]
                    } else {
                        input[d * w + line]
                    };
                    best = if grow { best.max(v) } else { best.min(v) };
                }
                // Cells beyond the plane edge count as empty, so a contraction
                // eats in from the canvas border; an expansion is unaffected.
                if !grow && (i < r || i + r >= len) {
                    best = 0;
                }
                let at = if horizontal {
                    line * w + i
                } else {
                    i * w + line
                };
                out[at] = best;
            }
        }
        out
    };
    pass(&pass(plane, true), false)
}

fn check_amount(op: Op, amount: u32) -> Result<()> {
    let max = match op {
        Op::Expand | Op::Contract | Op::Border => MAX_MORPH,
        Op::Feather | Op::Smooth | Op::Grow => MAX_BLUR,
        Op::Similar | Op::Invert => return Ok(()),
    };
    if amount == 0 || amount > max {
        bail!("[invalid-selection] amount must be 1-{max} pixels for this operation")
    }
    Ok(())
}

/// Apply a modifier to a coverage plane. `surface` is required for grow/similar.
pub fn modify_plane(
    plane: &[u8],
    w: usize,
    h: usize,
    op: Op,
    amount: u32,
    tolerance: u8,
    surface: Option<&Surface>,
) -> Result<Vec<u8>> {
    check_amount(op, amount)?;
    let r = amount as usize;
    if matches!(op, Op::Expand | Op::Contract | Op::Border)
        && (w * h) as u64 * u64::from(amount) > MAX_SIMILAR_WORK
    {
        bail!("[limit-exceeded] this selection change is too large for the layer; use a smaller amount")
    }
    Ok(match op {
        Op::Invert => plane.iter().map(|v| 255 - v).collect(),
        Op::Feather => {
            let radius = r.div_ceil(2).max(1);
            let mut out = plane.to_vec();
            for _ in 0..3 {
                out = box_blur(&out, w, h, radius);
            }
            out
        }
        Op::Expand => morph(plane, w, h, r, true),
        Op::Contract => morph(plane, w, h, r, false),
        Op::Smooth => box_blur(plane, w, h, r)
            .into_iter()
            .map(|v| if v >= 128 { 255 } else { 0 })
            .collect(),
        Op::Border => {
            let solid: Vec<u8> = plane
                .iter()
                .map(|v| if *v >= 128 { 255 } else { 0 })
                .collect();
            let outer = morph(&solid, w, h, r.div_ceil(2), true);
            let inner = morph(&solid, w, h, r / 2, false);
            outer
                .iter()
                .zip(&inner)
                .map(|(o, i)| if *i == 255 { 0 } else { *o })
                .collect()
        }
        Op::Grow | Op::Similar => {
            let surface = surface.context("grow and similar read the layer pixels")?;
            if op == Op::Grow {
                grow(plane, w, h, r, tolerance, surface)
            } else {
                similar(plane, w, h, tolerance, surface)?
            }
        }
    })
}

/// Add neighbors whose color is within tolerance of the selected pixel they were
/// reached from, for at most `rounds` pixels of distance (4-connected).
fn grow(
    plane: &[u8],
    w: usize,
    h: usize,
    rounds: usize,
    tolerance: u8,
    surface: &Surface,
) -> Vec<u8> {
    let mut out = plane.to_vec();
    let mut frontier: Vec<(usize, [u8; 4])> = (0..w * h)
        .filter(|i| plane[*i] >= 128)
        .map(|i| (i, surface.pixel((i % w) as u32, (i / w) as u32)))
        .collect();
    for _ in 0..rounds {
        let mut next = Vec::new();
        for (at, origin) in frontier {
            let (x, y) = ((at % w) as i64, (at / w) as i64);
            for (dx, dy) in [(0i64, -1i64), (-1, 0), (1, 0), (0, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let n = ny as usize * w + nx as usize;
                if out[n] < 128
                    && candidate(
                        surface.pixel(nx as u32, ny as u32),
                        origin,
                        tolerance,
                        false,
                    )
                {
                    out[n] = 255;
                    next.push((n, origin));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    out
}

/// Select every pixel of the layer within tolerance of any selected color.
fn similar(plane: &[u8], w: usize, h: usize, tolerance: u8, surface: &Surface) -> Result<Vec<u8>> {
    let mut colors: Vec<[u8; 4]> = Vec::new();
    for (i, cover) in plane.iter().enumerate() {
        if *cover < 128 {
            continue;
        }
        let p = surface.pixel((i % w) as u32, (i / w) as u32);
        if !colors.contains(&p) {
            if colors.len() == MAX_SIMILAR_COLORS {
                bail!("[limit-exceeded] the selection spans more than {MAX_SIMILAR_COLORS} distinct colors; use select-wand --global on one color instead")
            }
            colors.push(p);
        }
    }
    if (w * h) as u64 * colors.len() as u64 > MAX_SIMILAR_WORK {
        bail!("[limit-exceeded] select similar needs too much work for this layer; crop it first")
    }
    let mut out = plane.to_vec();
    for (i, cover) in out.iter_mut().enumerate() {
        if *cover >= 128 {
            continue;
        }
        let p = surface.pixel((i % w) as u32, (i / w) as u32);
        if colors.iter().any(|c| candidate(p, *c, tolerance, false)) {
            *cover = 255;
        }
    }
    Ok(out)
}

pub fn modify(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    op: Op,
    amount: u32,
    tolerance: u8,
) -> Result<Value> {
    let (w, h) = layer_size(raw, page, id)?;
    ensure_plane(w, h)?;
    let Some(old) = selection::active(raw, id)? else {
        bail!("[invalid-selection] {id} has no selection to change; make one with select-marquee, select-lasso or select-wand")
    };
    if (old.width, old.height) != (w, h) {
        bail!("[invalid-selection] the selection of {id} does not match the layer; run `select-clear {id}` first")
    }
    let surface = if matches!(op, Op::Grow | Op::Similar) {
        let node = locate(&mut raw.clone(), page, id)?.clone();
        Some(Surface::load(raw, &node)?)
    } else {
        None
    };
    let plane = modify_plane(
        &to_plane(&old),
        w as usize,
        h as usize,
        op,
        amount,
        tolerance,
        surface.as_ref(),
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
    Ok(result)
}

fn feathered(plane: Vec<u8>, w: u32, h: u32, feather: u32) -> Result<Vec<u8>> {
    if feather == 0 {
        return Ok(plane);
    }
    modify_plane(
        &plane,
        w as usize,
        h as usize,
        Op::Feather,
        feather,
        0,
        None,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marquee {
    Rect,
    Ellipse,
}

impl Marquee {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "rect" => Ok(Self::Rect),
            "ellipse" => Ok(Self::Ellipse),
            other => bail!("[invalid-selection] shape {other:?} must be rect or ellipse"),
        }
    }
}

/// Rectangular or elliptical marquee in layer pixels (`rect` = x, y, width, height).
pub fn marquee(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    shape: Marquee,
    rect: [f64; 4],
    mode: Mode,
    feather: u32,
) -> Result<Value> {
    if rect.iter().any(|v| !v.is_finite()) || rect[2] <= 0.0 || rect[3] <= 0.0 {
        bail!("[invalid-selection] the marquee rectangle needs finite x, y and a positive width and height")
    }
    install(raw, page, id, mode, |w, h| {
        let [rx, ry, rw, rh] = rect;
        if rx >= f64::from(w) || ry >= f64::from(h) || rx + rw <= 0.0 || ry + rh <= 0.0 {
            bail!("[invalid-selection] the marquee lies outside the {w}x{h} layer")
        }
        let mut plane = vec![0u8; w as usize * h as usize];
        let x0 = rx.floor().max(0.0) as usize;
        let y0 = ry.floor().max(0.0) as usize;
        let x1 = ((rx + rw).floor().min(f64::from(w)) as usize + 1).min(w as usize);
        let y1 = ((ry + rh).floor().min(f64::from(h)) as usize + 1).min(h as usize);
        let (cx, cy) = (rx + rw / 2.0, ry + rh / 2.0);
        let (ax, ay) = (rw / 2.0, rh / 2.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let mut count = 0u32;
                for j in 0..4 {
                    for i in 0..4 {
                        let sx = x as f64 + (f64::from(i) + 0.5) / 4.0;
                        let sy = y as f64 + (f64::from(j) + 0.5) / 4.0;
                        let inside = match shape {
                            Marquee::Rect => sx >= rx && sx < rx + rw && sy >= ry && sy < ry + rh,
                            Marquee::Ellipse => {
                                let (dx, dy) = ((sx - cx) / ax, (sy - cy) / ay);
                                dx * dx + dy * dy <= 1.0
                            }
                        };
                        count += u32::from(inside);
                    }
                }
                plane[y * w as usize + x] = ((count * 255 + 8) / 16) as u8;
            }
        }
        Ok(from_plane(w, h, &feathered(plane, w, h, feather)?))
    })
}

/// Polygon (lasso) selection by the even-odd rule, in layer pixels.
pub fn lasso(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    points: &[[f64; 2]],
    mode: Mode,
    feather: u32,
) -> Result<Value> {
    if points.len() < 3 || points.len() > MAX_POINTS {
        bail!(
            "[invalid-selection] a lasso needs 3-{MAX_POINTS} points, got {}",
            points.len()
        )
    }
    if points.iter().flatten().any(|v| !v.is_finite()) {
        bail!("[invalid-selection] lasso points must be finite numbers")
    }
    install(raw, page, id, mode, |w, h| {
        let (wu, hu) = (w as usize, h as usize);
        let mut counts = vec![0u8; wu * hu];
        let top = points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
        let bottom = points
            .iter()
            .map(|p| p[1])
            .fold(f64::NEG_INFINITY, f64::max);
        if bottom <= 0.0 || top >= f64::from(h) {
            bail!("[invalid-selection] the lasso lies outside the {w}x{h} layer")
        }
        let first = (top.floor().max(0.0) as usize) * 4;
        let last = (((bottom.floor().min(f64::from(h)) as i64 + 1).min(i64::from(h)) as usize) * 4)
            .min(hu * 4);
        let mut crossings: Vec<f64> = Vec::new();
        for sub in first..last {
            let ys = (sub as f64 + 0.5) / 4.0;
            crossings.clear();
            for k in 0..points.len() {
                let (a, b) = (points[k], points[(k + 1) % points.len()]);
                if (a[1] <= ys && ys < b[1]) || (b[1] <= ys && ys < a[1]) {
                    crossings.push(a[0] + (ys - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                }
            }
            crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let row = &mut counts[(sub / 4) * wu..(sub / 4 + 1) * wu];
            for pair in crossings.chunks_exact(2) {
                // Sub-sample k has its center at (k + 0.5) / 4.
                let start = (-(-(pair[0] * 4.0 - 0.5)).floor()).max(0.0);
                let end = (-(-(pair[1] * 4.0 - 0.5)).floor()).min((wu * 4) as f64);
                let (start, end) = (start as usize, end.max(0.0) as usize);
                for k in start..end {
                    row[k / 4] = row[k / 4].saturating_add(1);
                }
            }
        }
        let plane: Vec<u8> = counts
            .iter()
            .map(|c| ((u32::from(*c).min(16) * 255 + 8) / 16) as u8)
            .collect();
        Ok(from_plane(w, h, &feathered(plane, w, h, feather)?))
    })
}

/// Quick-mask painting: a brush stroke adds to (or with `erase`, removes from) the
/// layer's selection. Coverage follows the brush exactly as paint would.
#[allow(clippy::too_many_arguments)]
pub fn quickmask(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    brush: Brush,
    samples: Vec<Sample>,
    erase: bool,
    seed: u64,
) -> Result<Value> {
    if brush.tip.is_some() {
        bail!("[unsupported-capability] quick-mask strokes do not use textured tips; remove the tip from the brush")
    }
    let (w, h) = layer_size(raw, page, id)?;
    ensure_plane(w, h)?;
    let mut mask = match selection::active(raw, id)? {
        Some(old) if (old.width, old.height) == (w, h) => old,
        Some(_) => bail!("[invalid-selection] the selection of {id} does not match the layer; run `select-clear {id}` first"),
        None => Surface::empty(w, h),
    };
    let stroke = Stroke {
        brush,
        samples,
        color: [0, 0, 0],
        blend: if erase { Blend::Erase } else { Blend::Normal },
        seed,
        tip: None,
        clone: None,
    };
    let result = apply_stroke(&mut mask, &stroke)?;
    // Only alpha is coverage; keep the stored color canonical.
    let plane = to_plane(&mask);
    let mask = from_plane(w, h, &plane);
    let mut next = raw.clone();
    selection::store_active(&mut next, id, &mask)?;
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    let mut summary = selection::summary(&mask);
    summary["id"] = json!(id);
    summary["dabs"] = json!(result.dabs);
    summary["algorithm"] = json!(ALGORITHM);
    Ok(summary)
}

fn valid_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("[invalid-selection] a saved selection name is 1-64 letters, digits, '-', '_' or '.'")
    }
    Ok(())
}

/// Store the active selection under a name.
pub fn save(raw: &mut Value, page: Option<&str>, id: &str, name: &str) -> Result<Value> {
    valid_name(name)?;
    layer_size(raw, page, id)?;
    let Some(active) = selection::active(raw, id)? else {
        bail!("[invalid-selection] {id} has no selection to save")
    };
    let mut next = raw.clone();
    let existing = next["raster_selections"]["saved"][id]
        .as_object()
        .map_or(0, |m| m.len() - usize::from(m.contains_key(name)));
    if existing >= MAX_SAVED {
        bail!("[limit-exceeded] a layer keeps at most {MAX_SAVED} saved selections; delete one with select-delete")
    }
    let entry = selection::to_json(&mut next, &active)?;
    next["raster_selections"]["saved"][id][name] = entry;
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({"id": id, "name": name, "saved": true}))
}

/// Make a saved selection active, combined with the current one by `mode`.
pub fn load(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    name: &str,
    mode: Mode,
) -> Result<Value> {
    valid_name(name)?;
    let (w, h) = layer_size(raw, page, id)?;
    let entry = raw["raster_selections"]["saved"][id]
        .get(name)
        .cloned()
        .with_context(|| {
            format!("[not-found] {id} has no saved selection {name:?}; see `select-info {id}`")
        })?;
    let saved = selection::from_json(raw, &entry)?;
    if (saved.width, saved.height) != (w, h) {
        bail!(
            "[invalid-selection] saved selection {name:?} is {}x{} but {id} is {w}x{h}",
            saved.width,
            saved.height
        )
    }
    install(raw, page, id, mode, |_, _| Ok(saved))
}

pub fn delete(raw: &mut Value, id: &str, name: &str) -> Result<Value> {
    valid_name(name)?;
    let mut next = raw.clone();
    let removed = next["raster_selections"]["saved"][id]
        .as_object_mut()
        .and_then(|m| m.remove(name))
        .is_some();
    if !removed {
        bail!("[not-found] {id} has no saved selection {name:?}")
    }
    if next["raster_selections"]["saved"][id]
        .as_object()
        .is_some_and(|m| m.is_empty())
    {
        next["raster_selections"]["saved"]
            .as_object_mut()
            .map(|m| m.remove(id));
    }
    collect_garbage(&mut next);
    *raw = next;
    Ok(json!({"id": id, "name": name, "deleted": true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(plane: &[u8]) -> usize {
        plane.iter().filter(|v| **v > 0).count()
    }

    #[test]
    fn expand_contract_and_border_use_a_square() {
        let (w, h) = (20, 20);
        let mut plane = vec![0u8; w * h];
        for y in 8..12 {
            for x in 8..12 {
                plane[y * w + x] = 255;
            }
        }
        let big = modify_plane(&plane, w, h, Op::Expand, 2, 0, None).unwrap();
        assert_eq!(count(&big), 8 * 8);
        let back = modify_plane(&big, w, h, Op::Contract, 2, 0, None).unwrap();
        assert_eq!(back, plane);
        let ring = modify_plane(&plane, w, h, Op::Border, 2, 0, None).unwrap();
        assert!(ring[10 * w + 10] == 0 && ring[7 * w + 7] > 0);
        let inverse = modify_plane(&plane, w, h, Op::Invert, 1, 0, None).unwrap();
        assert_eq!(count(&inverse), w * h - 16);
    }

    #[test]
    fn feather_is_symmetric_and_smooth_is_binary() {
        let (w, h) = (16, 16);
        let mut plane = vec![0u8; w * h];
        for y in 4..12 {
            for x in 4..12 {
                plane[y * w + x] = 255;
            }
        }
        let soft = modify_plane(&plane, w, h, Op::Feather, 4, 0, None).unwrap();
        assert_eq!(soft[8 * w + 3], soft[8 * w + 12]);
        assert!(soft[8 * w + 3] > 0 && soft[8 * w + 3] < 255);
        let smooth = modify_plane(&plane, w, h, Op::Smooth, 2, 0, None).unwrap();
        assert!(smooth.iter().all(|v| *v == 0 || *v == 255));
        assert!(modify_plane(&plane, w, h, Op::Feather, 0, 0, None).is_err());
    }

    #[test]
    fn grow_and_similar_follow_color() {
        let mut s = Surface::empty(10, 1);
        for x in 0..10 {
            s.set_pixel(
                x,
                0,
                if x < 6 {
                    [200, 0, 0, 255]
                } else {
                    [0, 0, 200, 255]
                },
            );
        }
        let mut plane = vec![0u8; 10];
        plane[0] = 255;
        let grown = modify_plane(&plane, 10, 1, Op::Grow, 3, 5, Some(&s)).unwrap();
        assert_eq!(count(&grown), 4);
        let all = modify_plane(&plane, 10, 1, Op::Grow, 100, 5, Some(&s)).unwrap();
        assert_eq!(count(&all), 6, "growth stops at the color change");
        let like = modify_plane(&plane, 10, 1, Op::Similar, 1, 5, Some(&s)).unwrap();
        assert_eq!(count(&like), 6);
    }
}

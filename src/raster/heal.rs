//! Healing brush and spot healing (heal algorithm 1, engine 1).
//!
//! A heal stroke reads a clone source (see `clone.rs`) and separates two things:
//!
//! * **Texture** is the source detail. `texture` (0-1, default 1) scales the
//!   source's deviation from its own `(2*2+1)^2` alpha-weighted box mean, so 0
//!   transfers only the source's local tone and 1 transfers every detail.
//! * **Tone and color** come from the destination around the repair. The pixels
//!   the stroke covers form a region; the difference between destination and
//!   textured source is known on the one-pixel ring around that region and is
//!   filled across it by harmonic (Laplace) interpolation with a fixed number of
//!   row-major Gauss-Seidel sweeps in 1/16 fixed point. `tone` (0-1, default 1)
//!   scales that correction, so 0 pastes the source tone unchanged.
//!
//! The corrected source is composited with the stroke coverage and the source
//! alpha, as for clone. The region is bounded (1024 px per side and a limit on
//! pixels times sweeps), and everything is integer math. The algorithm number is
//! recorded in the journal; a newer number makes the entry unreplayable here.
//!
//! Spot healing picks the source itself: it compares the ring of pixels around the
//! spot with 64 candidate placements in a fixed order and takes the lowest score
//! (the first on ties), then records the chosen offset like any clone source.
use super::clone::Spec;
use super::math::sin_cos_degrees;
use super::*;

pub const ALGORITHM: u64 = 1;
const MAX_REGION: i64 = 1024;
const MAX_SOLVE_WORK: i64 = 64 * 1024 * 1024;
const TEXTURE_RADIUS: i64 = 2;
pub const MAX_SPOT_RADIUS: f64 = 256.0;

/// Corrected source pixels for every covered destination pixel.
pub(super) struct Field {
    x0: i64,
    y0: i64,
    w: i64,
    pixels: Vec<[u8; 4]>,
}

impl Field {
    pub(super) fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let (dx, dy) = (i64::from(x) - self.x0, i64::from(y) - self.y0);
        let rows = (self.pixels.len() as i64) / self.w.max(1);
        if dx < 0 || dy < 0 || dx >= self.w || dy >= rows {
            return [0; 4];
        }
        self.pixels[(dy * self.w + dx) as usize]
    }
}

fn div_round(n: i64, d: i64) -> i64 {
    (n + if n >= 0 { d / 2 } else { -d / 2 }) / d
}

/// Source pixel with `texture` applied.
fn textured(spec: &Spec, current: &Surface, x: i64, y: i64, texture16: i64) -> [u8; 4] {
    let center = spec.pixel_at(current, x, y);
    if texture16 == 65535 || center[3] == 0 {
        return center;
    }
    let mut weight = 0i64;
    let mut sum = [0i64; 3];
    for j in -TEXTURE_RADIUS..=TEXTURE_RADIUS {
        for i in -TEXTURE_RADIUS..=TEXTURE_RADIUS {
            let p = spec.pixel_at(current, x + i, y + j);
            let a = i64::from(p[3]);
            weight += a;
            for (s, v) in sum.iter_mut().zip(p) {
                *s += a * i64::from(v);
            }
        }
    }
    let mut out = center;
    for c in 0..3 {
        let mean = div_round(sum[c], weight.max(1));
        let detail = i64::from(center[c]) - mean;
        out[c] = (mean + div_round(detail * texture16, 65535)).clamp(0, 255) as u8;
    }
    out
}

/// Solve the repair for every pixel the stroke covers (coverage times opacity > 0).
pub(super) fn solve(
    surface: &Surface,
    spec: &Spec,
    buffer: &HashMap<(u32, u32), Vec<u16>>,
    opacity16: u32,
    brush: &Brush,
) -> Result<Field> {
    let texture16 = (brush.texture.unwrap_or(1.0) * 65535.0).round() as i64;
    let tone16 = (brush.tone.unwrap_or(1.0) * 65535.0).round() as i64;
    let mut covered: Vec<(i64, i64)> = Vec::new();
    let (mut lo, mut hi) = ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
    let mut keys: Vec<_> = buffer.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        for (index, value) in buffer[&key].iter().enumerate() {
            if *value == 0 || (u32::from(*value) * opacity16 + 32767) / 65535 == 0 {
                continue;
            }
            let x = i64::from(key.0) * TILE as i64 + (index % TILE) as i64;
            let y = i64::from(key.1) * TILE as i64 + (index / TILE) as i64;
            covered.push((x, y));
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        }
    }
    if covered.is_empty() {
        return Ok(Field {
            x0: 0,
            y0: 0,
            w: 1,
            pixels: Vec::new(),
        });
    }
    let (x0, y0) = (lo.0 - 1, lo.1 - 1);
    let (w, h) = (hi.0 - lo.0 + 3, hi.1 - lo.1 + 3);
    if w > MAX_REGION || h > MAX_REGION {
        bail!("[limit-exceeded] heal region is {w}x{h}; the limit is {MAX_REGION} pixels per side. Heal in smaller strokes")
    }
    let sweeps = (2 * w.max(h)).clamp(16, 512);
    if w * h * sweeps > MAX_SOLVE_WORK {
        bail!("[limit-exceeded] heal region {w}x{h} needs more than the solver budget; heal in smaller strokes")
    }
    let cells = (w * h) as usize;
    let at = |x: i64, y: i64| ((y - y0) * w + (x - x0)) as usize;
    let mut inside = vec![false; cells];
    for &(x, y) in &covered {
        inside[at(x, y)] = true;
    }
    let mut source = vec![[0u8; 4]; cells];
    for y in 0..h {
        for x in 0..w {
            source[(y * w + x) as usize] = textured(spec, surface, x0 + x, y0 + y, texture16);
        }
    }
    // Known destination-minus-source differences (1/16 units) on the ring, and the
    // running estimate inside the region.
    let mut diff = vec![[0i64; 3]; cells];
    let mut state = vec![0u8; cells]; // 0 unusable, 1 known, 2 unknown
    let (mut boundary_sum, mut boundary_count) = ([0i64; 3], 0i64);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let (px, py) = (x0 + x, y0 + y);
            if source[i][3] == 0 {
                continue;
            }
            if inside[i] {
                state[i] = 2;
            } else if px >= 0
                && py >= 0
                && px < i64::from(surface.width)
                && py < i64::from(surface.height)
            {
                let d = surface.pixel(px as u32, py as u32);
                if d[3] > 0 {
                    state[i] = 1;
                    for c in 0..3 {
                        diff[i][c] = (i64::from(d[c]) - i64::from(source[i][c])) * 16;
                        boundary_sum[c] += diff[i][c];
                    }
                    boundary_count += 1;
                }
            }
        }
    }
    if boundary_count > 0 {
        let mean = boundary_sum.map(|s| div_round(s, boundary_count));
        for i in 0..cells {
            if state[i] == 2 {
                diff[i] = mean;
            }
        }
        for _ in 0..sweeps {
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let i = (y * w + x) as usize;
                    if state[i] != 2 {
                        continue;
                    }
                    let mut sum = [0i64; 3];
                    let mut count = 0i64;
                    for n in [i - 1, i + 1, i - w as usize, i + w as usize] {
                        if state[n] != 0 {
                            count += 1;
                            for (s, d) in sum.iter_mut().zip(diff[n]) {
                                *s += d;
                            }
                        }
                    }
                    if count > 0 {
                        for (d, s) in diff[i].iter_mut().zip(sum) {
                            *d = div_round(s, count);
                        }
                    }
                }
            }
        }
    }
    let mut pixels = vec![[0u8; 4]; cells];
    for &(x, y) in &covered {
        let i = at(x, y);
        let s = source[i];
        if s[3] == 0 {
            continue;
        }
        let mut out = s;
        if boundary_count > 0 {
            for c in 0..3 {
                let correction = div_round(diff[i][c] * tone16, 16 * 65535);
                out[c] = (i64::from(s[c]) + correction).clamp(0, 255) as u8;
            }
        }
        pixels[i] = out;
    }
    Ok(Field { x0, y0, w, pixels })
}

fn sample_luma(surface: &Surface, x: i64, y: i64) -> Option<i64> {
    if x < 0 || y < 0 || x >= i64::from(surface.width) || y >= i64::from(surface.height) {
        return None;
    }
    let p = surface.pixel(x as u32, y as u32);
    (p[3] > 0).then(|| retouch::luma(p))
}

/// Choose the offset whose surroundings best match the spot's surroundings.
fn choose_offset(surface: &Surface, x: f64, y: f64, radius: f64) -> Result<(i64, i64, i64)> {
    let (cx, cy) = (x.floor() as i64, y.floor() as i64);
    let ring_radii = [radius * 1.35 + 1.0, radius * 1.7 + 2.0];
    let mut ring: Vec<(i64, i64, i64)> = Vec::new();
    for k in 0..16 {
        let (sin, cos) = sin_cos_degrees(f64::from(k) * 22.5);
        for r in ring_radii {
            let (px, py) = ((x + r * cos).floor() as i64, (y + r * sin).floor() as i64);
            if let Some(l) = sample_luma(surface, px, py) {
                ring.push((px, py, l));
            }
        }
    }
    if ring.len() < 4 {
        bail!("[invalid-heal] fewer than 4 painted pixels surround ({x}, {y}) within the radius; use `heal-stroke` with an explicit source")
    }
    let mut best: Option<(i64, i64, i64)> = None;
    for mult in [3.2, 4.5, 6.0, 8.0] {
        let distance = radius * mult + 4.0;
        for k in 0..16 {
            let (sin, cos) = sin_cos_degrees(f64::from(k) * 22.5 + 11.25);
            let (ox, oy) = (
                (distance * cos).round() as i64,
                (distance * sin).round() as i64,
            );
            let mut score = 0i64;
            let mut usable = true;
            for &(px, py, l) in &ring {
                match sample_luma(surface, px + ox, py + oy) {
                    Some(c) => score += (l - c).abs(),
                    None => {
                        usable = false;
                        break;
                    }
                }
            }
            if !usable {
                continue;
            }
            // Prefer a source whose own interior is smooth.
            let mut inner = vec![sample_luma(surface, cx + ox, cy + oy)];
            for j in 0..8 {
                let (sin, cos) = sin_cos_degrees(f64::from(j) * 45.0);
                inner.push(sample_luma(
                    surface,
                    cx + ox + (radius * 0.5 * cos).round() as i64,
                    cy + oy + (radius * 0.5 * sin).round() as i64,
                ));
            }
            let Some(values) = inner.into_iter().collect::<Option<Vec<_>>>() else {
                continue;
            };
            let mean = div_round(values.iter().sum::<i64>(), values.len() as i64);
            score += values.iter().map(|v| (v - mean).abs()).sum::<i64>();
            if best.is_none_or(|(_, _, s)| score < s) {
                best = Some((ox, oy, score));
            }
        }
    }
    best.context("[invalid-heal] no candidate source around the spot is fully painted; use `heal-stroke` with an explicit source")
}

/// Heal one round spot, choosing the source automatically.
#[allow(clippy::too_many_arguments)]
pub fn spot(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    x: f64,
    y: f64,
    radius: f64,
    texture: Option<f64>,
    tone: Option<f64>,
    seed: u64,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    if !x.is_finite() || !y.is_finite() || !(1.0..=MAX_SPOT_RADIUS).contains(&radius) {
        bail!("[invalid-heal] spot needs finite x and y and a radius of 1-{MAX_SPOT_RADIUS}")
    }
    let node = all_rasters(raw)
        .into_iter()
        .find(|node| node["id"] == id)
        .context("[not-found] raster layer was not found")?;
    let surface = Surface::load(raw, node)?;
    if x < 0.0 || y < 0.0 || x >= f64::from(surface.width) || y >= f64::from(surface.height) {
        bail!("[invalid-heal] ({x}, {y}) is outside the layer")
    }
    let (ox, oy, score) = choose_offset(&surface, x, y, radius)?;
    let mut brush = json!({
        "kind": "soft-round",
        "size": radius * 2.5,
        "hardness": 0.8,
    });
    if let Some(texture) = texture {
        brush["texture"] = json!(texture);
    }
    if let Some(tone) = tone {
        brush["tone"] = json!(tone);
    }
    let mut result = paint(
        raw,
        page,
        id,
        StrokeRequest {
            brush: Brush::parse(&brush)?,
            samples: parse_samples(&json!([[x, y]]))?,
            color: [0, 0, 0],
            blend: Blend::Tool(Tool::Heal),
            seed,
            clone: Some(CloneRequest {
                source: None,
                sx: x + ox as f64,
                sy: y + oy as f64,
                ax: x,
                ay: y,
                angle: 0.0,
                scale: 1.0,
            }),
        },
    )?;
    result["heal"] = json!({
        "algorithm": ALGORITHM,
        "source_offset": [ox, oy],
        "score": score,
    });
    Ok(result)
}

/// Patch healing: repair the active selection with texture from the area `dx`,
/// `dy` pixels away. The selection edge is the tone ring, so only selected pixels
/// change; this is a heal stroke that covers the selection's bounds.
pub fn patch(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    offset: (f64, f64),
    texture: Option<f64>,
    tone: Option<f64>,
    seed: u64,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    if !offset.0.is_finite() || !offset.1.is_finite() || offset == (0.0, 0.0) {
        bail!("[invalid-heal] patch needs a finite, nonzero source offset (dx, dy)")
    }
    let node = all_rasters(raw)
        .into_iter()
        .find(|node| node["id"] == id)
        .context("[not-found] raster layer was not found")?;
    let surface = Surface::load(raw, node)?;
    let Some((selection, _)) = selection::pin(raw, id, surface.width, surface.height)? else {
        bail!("[invalid-heal] patch repairs the selection of {id}, but it has none; run `select-marquee`, `select-lasso` or `select-wand` first")
    };
    let Some([x0, y0, x1, y1]) = selection.painted_bounds() else {
        bail!("[invalid-heal] the selection of {id} is empty")
    };
    let (cx, cy) = (f64::from(x0 + x1) / 2.0, f64::from(y0 + y1) / 2.0);
    let diagonal = f64::from(x1 - x0).hypot(f64::from(y1 - y0));
    let mut brush = json!({
        "kind": "hard-round",
        "size": (diagonal + 4.0).min(MAX_BRUSH_SIZE),
        "hardness": 1.0,
    });
    if let Some(texture) = texture {
        brush["texture"] = json!(texture);
    }
    if let Some(tone) = tone {
        brush["tone"] = json!(tone);
    }
    let mut result = paint(
        raw,
        page,
        id,
        StrokeRequest {
            brush: Brush::parse(&brush)?,
            samples: parse_samples(&json!([[cx, cy]]))?,
            color: [0, 0, 0],
            blend: Blend::Tool(Tool::Heal),
            seed,
            clone: Some(CloneRequest {
                source: None,
                sx: cx + offset.0,
                sy: cy + offset.1,
                ax: cx,
                ay: cy,
                angle: 0.0,
                scale: 1.0,
            }),
        },
    )?;
    result["heal"] = json!({
        "algorithm": ALGORITHM,
        "patch": {"source_offset": [offset.0, offset.1], "bounds": [x0, y0, x1, y1]},
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: u32, height: u32, color: [u8; 4]) -> Surface {
        let mut s = Surface {
            width,
            height,
            tiles: BTreeMap::new(),
        };
        for y in 0..height {
            for x in 0..width {
                put(&mut s, x, y, color);
            }
        }
        s
    }

    fn put(s: &mut Surface, x: u32, y: u32, c: [u8; 4]) {
        let tile = s
            .tiles
            .entry((x / TILE as u32, y / TILE as u32))
            .or_insert_with(|| vec![0u8; TILE_BYTES].into_boxed_slice());
        let at = ((y as usize % TILE) * TILE + x as usize % TILE) * 4;
        tile[at..at + 4].copy_from_slice(&c);
    }

    fn heal(s: &mut Surface, x: f64, y: f64, source: (f64, f64), brush: Value) -> Stroke {
        let stroke = Stroke {
            brush: Brush::parse(&brush).unwrap(),
            samples: parse_samples(&json!([[x, y]])).unwrap(),
            color: [0; 3],
            blend: Blend::Tool(Tool::Heal),
            seed: 0,
            tip: None,
            clone: Some(
                Spec::build(
                    &json!({}),
                    "t",
                    CloneRequest {
                        source: None,
                        sx: source.0,
                        sy: source.1,
                        ax: x,
                        ay: y,
                        angle: 0.0,
                        scale: 1.0,
                    },
                    &[],
                )
                .unwrap(),
            ),
        };
        apply_stroke(s, &stroke).unwrap();
        stroke
    }

    #[test]
    fn tone_comes_from_the_destination_and_texture_from_the_source() {
        // A mid-gray field with a dark blemish; the source patch is brighter but
        // carries a vertical stripe of detail.
        let mut s = flat(200, 100, [120, 120, 120, 255]);
        for y in 40..60 {
            for x in 40..60 {
                put(&mut s, x, y, [20, 20, 20, 255]);
            }
        }
        for y in 0..100 {
            for x in 130..170 {
                let stripe = if x % 4 < 2 { 40 } else { 0 };
                put(
                    &mut s,
                    x,
                    y,
                    [180 + stripe, 180 + stripe, 180 + stripe, 255],
                );
            }
        }
        heal(
            &mut s,
            50.0,
            50.0,
            (150.0, 50.0),
            json!({"size":40,"hardness":1}),
        );
        let healed = s.pixel(50, 50);
        assert!(
            (100..=140).contains(&healed[0]),
            "tone follows the surroundings, not the bright source: {healed:?}"
        );
        let (a, b) = (s.pixel(48, 50)[0], s.pixel(50, 50)[0]);
        assert!(a != b, "source stripe detail survives: {a} vs {b}");
        assert_eq!(s.pixel(10, 10), [120, 120, 120, 255], "outside the stroke");
    }

    #[test]
    fn texture_zero_removes_detail_and_tone_zero_pastes_source_tone() {
        let build = || {
            let mut s = flat(200, 100, [120, 120, 120, 255]);
            for y in 0..100 {
                for x in 130..170 {
                    let stripe = if x % 4 < 2 { 40 } else { 0 };
                    put(
                        &mut s,
                        x,
                        y,
                        [180 + stripe, 180 + stripe, 180 + stripe, 255],
                    );
                }
            }
            s
        };
        let mut flat_texture = build();
        heal(
            &mut flat_texture,
            50.0,
            50.0,
            (150.0, 50.0),
            json!({"size":40,"hardness":1,"texture":0}),
        );
        let spread = |s: &Surface| {
            let v: Vec<u8> = (44..56).map(|x| s.pixel(x, 50)[0]).collect();
            i32::from(*v.iter().max().unwrap()) - i32::from(*v.iter().min().unwrap())
        };
        // A 5-pixel box cannot flatten a 4-pixel stripe exactly; it leaves a
        // residual far below the stripe's 40 levels.
        assert!(spread(&flat_texture) <= 10, "{}", spread(&flat_texture));
        let mut source_tone = build();
        heal(
            &mut source_tone,
            50.0,
            50.0,
            (150.0, 50.0),
            json!({"size":40,"hardness":1,"tone":0}),
        );
        assert!(source_tone.pixel(50, 50)[0] >= 180);
    }

    #[test]
    fn healing_is_deterministic_and_bounded() {
        let build = || {
            let mut s = flat(120, 80, [90, 100, 110, 255]);
            put(&mut s, 60, 40, [255, 0, 0, 255]);
            s
        };
        let (mut a, mut b) = (build(), build());
        heal(&mut a, 60.0, 40.0, (30.0, 40.0), json!({"size":20}));
        heal(&mut b, 60.0, 40.0, (30.0, 40.0), json!({"size":20}));
        assert_eq!(a.tile_map_hash(), b.tile_map_hash());
        assert_eq!(a.pixel(60, 40), [90, 100, 110, 255], "blemish gone");
        let mut big = flat(2000, 2000, [1, 2, 3, 255]);
        let stroke = Stroke {
            brush: Brush::parse(&json!({"size":1500})).unwrap(),
            samples: parse_samples(&json!([[1000, 1000]])).unwrap(),
            color: [0; 3],
            blend: Blend::Tool(Tool::Heal),
            seed: 0,
            tip: None,
            clone: Some(
                Spec::build(
                    &json!({}),
                    "t",
                    CloneRequest {
                        source: None,
                        sx: 300.0,
                        sy: 300.0,
                        ax: 1000.0,
                        ay: 1000.0,
                        angle: 0.0,
                        scale: 1.0,
                    },
                    &[],
                )
                .unwrap(),
            ),
        };
        let error = apply_stroke(&mut big, &stroke).err().unwrap().to_string();
        assert!(error.contains("limit-exceeded"), "{error}");
    }

    #[test]
    fn spot_search_picks_a_clean_source_and_refuses_empty_surroundings() {
        let mut s = flat(200, 120, [100, 100, 100, 255]);
        for y in 56..64 {
            for x in 96..104 {
                put(&mut s, x, y, [0, 0, 0, 255]);
            }
        }
        let (ox, oy, _) = choose_offset(&s, 100.0, 60.0, 6.0).unwrap();
        assert!(ox != 0 || oy != 0);
        let empty = Surface {
            width: 100,
            height: 100,
            tiles: BTreeMap::new(),
        };
        assert!(choose_offset(&empty, 50.0, 50.0, 6.0).is_err());
    }
}

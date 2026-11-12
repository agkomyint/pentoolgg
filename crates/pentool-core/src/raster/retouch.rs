//! Erasing and local blending tools (engine 1).
//!
//! Every tool reads the pixels as they were before the stroke, except `smudge`,
//! which is defined sequentially along the dab path. Reads past the layer edge
//! clamp to the nearest edge pixel. All math is integer, so results are identical
//! on every platform.
use super::*;
use anyhow::Context as _;

/// Blur and sharpen average a `(2r+1)^2` box with `r = clamp(round(size / 16), 1, 4)`.
pub const MAX_KERNEL_RADIUS: i64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    BackgroundErase,
    Smudge,
    Blur,
    Sharpen,
    Dodge,
    Burn,
    Sponge,
    ColorReplace,
    /// Copies pixels from a pinned source; see `clone.rs`.
    Clone,
    /// Clone with tone matched to the surroundings; see `heal.rs`.
    Heal,
}

impl Blend {
    pub const NAMES: [&'static str; 12] = [
        "normal",
        "erase",
        "background-erase",
        "smudge",
        "blur",
        "sharpen",
        "dodge",
        "burn",
        "sponge",
        "color-replace",
        "clone",
        "heal",
    ];

    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "normal" => Self::Normal,
            "erase" => Self::Erase,
            "background-erase" => Self::Tool(Tool::BackgroundErase),
            "smudge" => Self::Tool(Tool::Smudge),
            "blur" => Self::Tool(Tool::Blur),
            "sharpen" => Self::Tool(Tool::Sharpen),
            "dodge" => Self::Tool(Tool::Dodge),
            "burn" => Self::Tool(Tool::Burn),
            "sponge" => Self::Tool(Tool::Sponge),
            "color-replace" => Self::Tool(Tool::ColorReplace),
            "clone" => Self::Tool(Tool::Clone),
            "heal" => Self::Tool(Tool::Heal),
            other => bail!(
                "[invalid-stroke] blend {other:?} is not supported; use one of {}",
                Self::NAMES.join(", ")
            ),
        })
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Erase => "erase",
            Self::Tool(Tool::BackgroundErase) => "background-erase",
            Self::Tool(Tool::Smudge) => "smudge",
            Self::Tool(Tool::Blur) => "blur",
            Self::Tool(Tool::Sharpen) => "sharpen",
            Self::Tool(Tool::Dodge) => "dodge",
            Self::Tool(Tool::Burn) => "burn",
            Self::Tool(Tool::Sponge) => "sponge",
            Self::Tool(Tool::ColorReplace) => "color-replace",
            Self::Tool(Tool::Clone) => "clone",
            Self::Tool(Tool::Heal) => "heal",
        }
    }
}

/// Tool parameters are accepted only by the tools that use them; a parameter that
/// would be ignored is an error rather than a silent no-op.
pub(super) fn check(brush: &Brush, blend: Blend) -> Result<()> {
    let tool = match blend {
        Blend::Tool(tool) => Some(tool),
        _ => None,
    };
    let uses_strength = matches!(
        tool,
        Some(Tool::Smudge | Tool::Blur | Tool::Sharpen | Tool::Dodge | Tool::Burn | Tool::Sponge)
    );
    let uses_tolerance = matches!(tool, Some(Tool::BackgroundErase | Tool::ColorReplace));
    let uses_range = matches!(tool, Some(Tool::Dodge | Tool::Burn));
    let uses_mode = matches!(tool, Some(Tool::Sponge));
    let uses_heal = matches!(tool, Some(Tool::Heal));
    for (given, used, key) in [
        (brush.strength.is_some(), uses_strength, "strength"),
        (brush.tolerance.is_some(), uses_tolerance, "tolerance"),
        (brush.range.is_some(), uses_range, "range"),
        (brush.mode.is_some(), uses_mode, "mode"),
        (brush.texture.is_some(), uses_heal, "texture"),
        (brush.tone.is_some(), uses_heal, "tone"),
    ] {
        if given && !used {
            bail!(
                "[invalid-brush] {key} does not apply to blend {}; it is used by {}",
                blend.name(),
                match key {
                    "strength" => "smudge, blur, sharpen, dodge, burn and sponge",
                    "tolerance" => "background-erase and color-replace",
                    "range" => "dodge and burn",
                    "texture" | "tone" => "heal",
                    _ => "sponge",
                }
            )
        }
    }
    Ok(())
}

pub(super) fn parse_range(text: &str) -> Result<()> {
    match text {
        "shadows" | "midtones" | "highlights" => Ok(()),
        other => bail!("[invalid-brush] range {other:?} must be shadows, midtones or highlights"),
    }
}

pub(super) fn parse_mode(text: &str) -> Result<()> {
    match text {
        "saturate" | "desaturate" => Ok(()),
        other => bail!("[invalid-brush] mode {other:?} must be saturate or desaturate"),
    }
}

fn div_round(n: i64, d: i64) -> i64 {
    (n + if n >= 0 { d / 2 } else { -d / 2 }) / d
}

pub(super) fn luma(p: [u8; 4]) -> i64 {
    (54 * i64::from(p[0]) + 183 * i64::from(p[1]) + 19 * i64::from(p[2]) + 128) >> 8
}

impl Surface {
    /// Pixel with coordinates clamped to the layer, which is the edge mode of every
    /// local tool.
    pub(super) fn pixel_clamped(&self, x: i64, y: i64) -> [u8; 4] {
        self.pixel(
            x.clamp(0, i64::from(self.width) - 1) as u32,
            y.clamp(0, i64::from(self.height) - 1) as u32,
        )
    }
}

pub(super) fn kernel_radius(brush: &Brush) -> i64 {
    ((brush.size / 16.0).round() as i64).clamp(1, MAX_KERNEL_RADIUS)
}

/// Extra per-pixel cost a tool adds to the stroke-work estimate.
pub(super) fn work_factor(brush: &Brush, blend: Blend) -> u64 {
    match blend {
        Blend::Tool(Tool::Blur | Tool::Sharpen) => {
            let side = (2 * kernel_radius(brush) + 1) as u64;
            side * side
        }
        _ => 1,
    }
}

/// Per-stroke constants for the pointwise tools.
pub(super) struct Context {
    tool: Tool,
    strength16: i64,
    tolerance: i64,
    range: &'static str,
    saturate: bool,
    radius: i64,
    color: [u8; 3],
    /// Color under the first dab, for the tools that match against it.
    sample: [u8; 4],
}

impl Context {
    pub(super) fn new(
        tool: Tool,
        brush: &Brush,
        color: [u8; 3],
        surface: &Surface,
        first: Option<&Dab>,
    ) -> Result<Self> {
        let sample = if matches!(tool, Tool::BackgroundErase | Tool::ColorReplace) {
            let dab = first.context("[invalid-stroke] the stroke has no dabs")?;
            let (x, y) = (dab.x.floor() as i64, dab.y.floor() as i64);
            if x < 0 || y < 0 || x >= i64::from(surface.width) || y >= i64::from(surface.height) {
                bail!(
                    "[invalid-stroke] {} samples the color under the first dab, which is outside the layer",
                    Blend::Tool(tool).name()
                )
            }
            let pixel = surface.pixel(x as u32, y as u32);
            if pixel[3] == 0 {
                bail!(
                    "[invalid-stroke] {} needs painted pixels under the first dab; the pixel at {x},{y} is transparent",
                    Blend::Tool(tool).name()
                )
            }
            pixel
        } else {
            [0; 4]
        };
        Ok(Self {
            tool,
            strength16: ((brush.strength.unwrap_or(0.5)) * 65535.0).round() as i64,
            tolerance: i64::from(brush.tolerance.unwrap_or(32)),
            range: match brush.range.as_deref() {
                Some("shadows") => "shadows",
                Some("highlights") => "highlights",
                _ => "midtones",
            },
            saturate: brush.mode.as_deref() == Some("saturate"),
            radius: kernel_radius(brush),
            color,
            sample,
        })
    }

    /// Weight 0..=65535 of how closely `pixel` matches the sampled color.
    fn match_weight(&self, pixel: [u8; 4]) -> i64 {
        let distance = (0..3)
            .map(|i| (i64::from(pixel[i]) - i64::from(self.sample[i])).abs())
            .max()
            .unwrap_or(0);
        if distance > self.tolerance {
            0
        } else {
            (self.tolerance + 1 - distance) * 65535 / (self.tolerance + 1)
        }
    }

    /// New value of the pixel at `x, y` given stroke coverage `amount` (0..=65535,
    /// flow and opacity included), reading neighbors from the untouched surface.
    pub(super) fn pixel(&self, surface: &Surface, x: u32, y: u32, amount: i64) -> [u8; 4] {
        let px = surface.pixel(x, y);
        let eff = amount * self.strength16 / 65535;
        match self.tool {
            Tool::Smudge | Tool::Clone | Tool::Heal => px,
            Tool::BackgroundErase => {
                if px[3] == 0 {
                    return px;
                }
                let amt = amount * self.match_weight(px) / 65535;
                let a = (i64::from(px[3]) * (65535 - amt) + 32767) / 65535;
                if a == 0 {
                    [0; 4]
                } else {
                    [px[0], px[1], px[2], a as u8]
                }
            }
            Tool::ColorReplace => {
                if px[3] == 0 {
                    return px;
                }
                let amt = amount * self.match_weight(px) / 65535;
                let mut out = px;
                for i in 0..3 {
                    let shift = i64::from(self.color[i]) - i64::from(self.sample[i]);
                    out[i] = (i64::from(px[i]) + div_round(shift * amt, 65535)).clamp(0, 255) as u8;
                }
                out
            }
            Tool::Blur | Tool::Sharpen => self.convolve(surface, x, y, px, eff),
            Tool::Dodge | Tool::Burn => {
                if px[3] == 0 {
                    return px;
                }
                let l = luma(px);
                let weight = match self.range {
                    "shadows" => (255 - l) * (255 - l) / 255,
                    "highlights" => l * l / 255,
                    _ => 255 - (2 * l - 255).abs(),
                };
                let e = eff * weight / 255;
                let mut out = px;
                for i in 0..3 {
                    let c = i64::from(px[i]);
                    out[i] = if self.tool == Tool::Dodge {
                        c + (((255 - c) * e + 32767) / 65535)
                    } else {
                        c - ((c * e + 32767) / 65535)
                    }
                    .clamp(0, 255) as u8;
                }
                out
            }
            Tool::Sponge => {
                if px[3] == 0 {
                    return px;
                }
                let g = luma(px);
                let mut out = px;
                for i in 0..3 {
                    let c = i64::from(px[i]);
                    let delta = div_round((c - g) * eff, 65535);
                    out[i] = if self.saturate { c + delta } else { c - delta }.clamp(0, 255) as u8;
                }
                out
            }
        }
    }

    fn convolve(&self, surface: &Surface, x: u32, y: u32, px: [u8; 4], eff: i64) -> [u8; 4] {
        let r = self.radius;
        let n = (2 * r + 1) * (2 * r + 1);
        let (mut sum_a, mut sum_p) = (0i64, [0i64; 3]);
        for dy in -r..=r {
            for dx in -r..=r {
                let q = surface.pixel_clamped(i64::from(x) + dx, i64::from(y) + dy);
                let a = i64::from(q[3]);
                sum_a += a;
                for i in 0..3 {
                    sum_p[i] += i64::from(q[i]) * a;
                }
            }
        }
        let mean_a = (sum_a + n / 2) / n;
        let a0 = i64::from(px[3]);
        if self.tool == Tool::Sharpen {
            // Unsharp mask on premultiplied color with gain 2; alpha is preserved.
            if a0 == 0 {
                return px;
            }
            let mut out = px;
            for i in 0..3 {
                let p0 = i64::from(px[i]) * a0;
                let mean = (sum_p[i] + n / 2) / n;
                let sharp = p0 + div_round((p0 - mean) * 2 * eff, 65535);
                out[i] = div_round(sharp.clamp(0, 255 * a0), a0).clamp(0, 255) as u8;
            }
            return out;
        }
        let a = (a0 * (65535 - eff) + mean_a * eff + 32767) / 65535;
        if a == 0 {
            return [0; 4];
        }
        let mut out = [0, 0, 0, a as u8];
        for i in 0..3 {
            let p0 = i64::from(px[i]) * a0;
            let mean = (sum_p[i] + n / 2) / n;
            let p = (p0 * (65535 - eff) + mean * eff + 32767) / 65535;
            out[i] = ((p + a / 2) / a).clamp(0, 255) as u8;
        }
        out
    }
}

/// Smudge drags color along the dab path. It runs on a copy-on-write view of the
/// touched tiles, because each dab must see what earlier dabs left behind.
///
/// The carried color starts as the pixel under the first dab. For each dab, every
/// covered pixel moves toward the carried color by `coverage * flow * opacity *
/// strength`; then the carry moves toward the pixel under the dab center, read before
/// that dab changed it, by `1 - strength`. Strength 1 therefore never refreshes the carry.
pub(super) fn smudge(
    surface: &mut Surface,
    stroke: &Stroke,
    tip: Option<&[u8]>,
    dab_list: &[Dab],
) -> Result<(Option<[u32; 4]>, usize)> {
    let strength16 = (stroke.brush.strength.unwrap_or(0.5) * 65535.0).round() as i64;
    let opacity16 = (stroke.brush.opacity * 65535.0).round() as i64;
    let mut work: BTreeMap<(u32, u32), Box<[u8]>> = BTreeMap::new();
    let mut original: BTreeMap<(u32, u32), Option<Box<[u8]>>> = BTreeMap::new();
    let (width, height) = (surface.width, surface.height);
    let read = |work: &BTreeMap<(u32, u32), Box<[u8]>>, surface: &Surface, x: i64, y: i64| {
        let (x, y) = (
            x.clamp(0, i64::from(width) - 1) as u32,
            y.clamp(0, i64::from(height) - 1) as u32,
        );
        match work.get(&(x / TILE as u32, y / TILE as u32)) {
            Some(tile) => {
                let at = ((y as usize % TILE) * TILE + x as usize % TILE) * 4;
                [tile[at], tile[at + 1], tile[at + 2], tile[at + 3]]
            }
            None => surface.pixel(x, y),
        }
    };
    // Carry is premultiplied: color channels scaled by alpha, alpha 0..=255.
    let premultiplied = |p: [u8; 4]| -> [i64; 4] {
        let a = i64::from(p[3]);
        [
            i64::from(p[0]) * a,
            i64::from(p[1]) * a,
            i64::from(p[2]) * a,
            a,
        ]
    };
    let lerp_carry = |from: [i64; 4], to: [i64; 4], t: i64| -> [i64; 4] {
        let mut out = [0; 4];
        for i in 0..4 {
            out[i] = (from[i] * (65535 - t) + to[i] * t + 32767) / 65535;
        }
        out
    };
    let first = &dab_list[0];
    let mut carry = premultiplied(read(
        &work,
        surface,
        first.x.floor() as i64,
        first.y.floor() as i64,
    ));
    let mut bounds: Option<[u32; 4]> = None;
    for dab in dab_list {
        let center = premultiplied(read(
            &work,
            surface,
            dab.x.floor() as i64,
            dab.y.floor() as i64,
        ));
        let flow16 = (dab.flow * 65535.0).round() as i64;
        let mut writes: Vec<(u32, u32, [u8; 4])> = Vec::new();
        dab_pixels(&stroke.brush, tip, dab, width, height, |x, y, coverage| {
            let amount = (coverage * flow16 as f64).round() as i64 * opacity16 / 65535;
            let eff = amount * strength16 / 65535;
            if eff == 0 {
                return;
            }
            let p = read(&work, surface, x, y);
            let current = premultiplied(p);
            let next = lerp_carry(current, carry, eff);
            let out = if next[3] == 0 {
                [0; 4]
            } else {
                let a = next[3];
                [
                    ((next[0] + a / 2) / a).clamp(0, 255) as u8,
                    ((next[1] + a / 2) / a).clamp(0, 255) as u8,
                    ((next[2] + a / 2) / a).clamp(0, 255) as u8,
                    a as u8,
                ]
            };
            writes.push((x as u32, y as u32, out));
        });
        for (x, y, out) in writes {
            let key = (x / TILE as u32, y / TILE as u32);
            original
                .entry(key)
                .or_insert_with(|| surface.tiles.get(&key).cloned());
            let tile = work.entry(key).or_insert_with(|| {
                surface
                    .tiles
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| vec![0u8; TILE_BYTES].into_boxed_slice())
            });
            let at = ((y as usize % TILE) * TILE + x as usize % TILE) * 4;
            tile[at..at + 4].copy_from_slice(&out);
            let b = bounds.get_or_insert([x, y, x + 1, y + 1]);
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x + 1);
            b[3] = b[3].max(y + 1);
        }
        carry = lerp_carry(carry, center, 65535 - strength16);
    }
    let mut touched = 0;
    for (key, tile) in work {
        let before = original.remove(&key).flatten();
        let blank = is_blank(&tile);
        if before.as_deref() != Some(&tile[..]) && !(before.is_none() && blank) {
            touched += 1;
        }
        if blank {
            surface.tiles.remove(&key);
        } else {
            surface.tiles.insert(key, tile);
        }
    }
    Ok((bounds, touched))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint_rect(s: &mut Surface, x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                let tile = s
                    .tiles
                    .entry((x / TILE as u32, y / TILE as u32))
                    .or_insert_with(|| vec![0u8; TILE_BYTES].into_boxed_slice());
                let at = ((y as usize % TILE) * TILE + x as usize % TILE) * 4;
                tile[at..at + 4].copy_from_slice(&c);
            }
        }
    }

    fn blank(width: u32, height: u32) -> Surface {
        Surface {
            width,
            height,
            tiles: BTreeMap::new(),
        }
    }

    /// Left half red, right half blue, 300x100, opaque.
    fn halves() -> Surface {
        let mut s = blank(300, 100);
        paint_rect(&mut s, 0, 0, 150, 100, [200, 40, 40, 255]);
        paint_rect(&mut s, 150, 0, 300, 100, [40, 40, 200, 255]);
        s
    }

    fn run(s: &mut Surface, blend: &str, brush: Value, samples: Value) -> Result<StrokeResult> {
        let stroke = Stroke {
            brush: Brush::parse(&brush)?,
            samples: parse_samples(&samples)?,
            color: [0, 200, 0],
            blend: Blend::parse(blend)?,
            clone: None,
            seed: 1,
            tip: None,
        };
        apply_stroke(s, &stroke)
    }

    fn red_step(s: &Surface) -> i32 {
        i32::from(s.pixel(149, 50)[0]) - i32::from(s.pixel(151, 50)[0])
    }

    #[test]
    fn blend_names_round_trip_and_unknown_is_refused() {
        for name in Blend::NAMES {
            assert_eq!(Blend::parse(name).unwrap().name(), name);
        }
        assert!(Blend::parse("multiply").is_err());
    }

    #[test]
    fn blur_softens_a_hard_edge_and_sharpen_steepens_it() {
        let brush = json!({"size":40,"strength":1});
        let line = json!([[100, 50], [200, 50]]);
        let mut s = halves();
        let hard = red_step(&s);
        let result = run(&mut s, "blur", brush.clone(), line.clone()).unwrap();
        assert!(result.tiles_changed > 0);
        let soft = red_step(&s);
        assert!(soft < hard && soft > 0, "{soft} vs {hard}");
        assert_eq!(s.pixel(5, 5), [200, 40, 40, 255], "far from the stroke");
        run(&mut s, "sharpen", brush, line).unwrap();
        assert!(red_step(&s) > soft, "sharpen steepens the gradient");
        assert_eq!(s.pixel(149, 50)[3], 255, "sharpen never touches alpha");
    }

    #[test]
    fn local_tools_do_not_depend_on_tile_order_across_seams() {
        let build = || {
            let mut s = blank(512, 64);
            paint_rect(&mut s, 0, 0, 256, 64, [255, 0, 0, 255]);
            paint_rect(&mut s, 256, 0, 512, 64, [0, 0, 255, 255]);
            s
        };
        let (mut a, mut b) = (build(), build());
        let brush = json!({"size":48,"strength":1});
        let line = json!([[200, 30], [320, 30]]);
        run(&mut a, "blur", brush.clone(), line.clone()).unwrap();
        run(&mut b, "blur", brush, line).unwrap();
        assert_eq!(a.tile_map_hash(), b.tile_map_hash());
        let (left, right) = (a.pixel(255, 30), a.pixel(256, 30));
        assert!(left[0] > 0 && left[2] > 0 && right[0] > 0 && right[2] > 0);
        assert!(
            left[2] <= right[2] && left[0] >= right[0],
            "monotone across the seam"
        );
    }

    #[test]
    fn dodge_lightens_burn_darkens_and_range_selects_tones() {
        let mut base = blank(200, 60);
        paint_rect(&mut base, 0, 0, 100, 60, [30, 30, 30, 255]);
        paint_rect(&mut base, 100, 0, 200, 60, [220, 220, 220, 255]);
        let paint = |blend: &str, range: &str| {
            let mut copy = blank(200, 60);
            copy.tiles = base.tiles.clone();
            run(
                &mut copy,
                blend,
                json!({"size":180,"strength":1,"range":range}),
                json!([[100, 30]]),
            )
            .unwrap();
            (copy.pixel(50, 30)[0], copy.pixel(150, 30)[0])
        };
        let (dark, light) = paint("dodge", "shadows");
        assert!(dark > 30, "shadows dodge lifts the dark side");
        assert!(light <= 222, "shadows barely move highlights: {light}");
        let (dark, light) = paint("burn", "highlights");
        assert!(dark <= 30 && light < 220, "{dark} {light}");
        let (dark, light) = paint("burn", "shadows");
        assert!(dark < 30, "shadow burn darkens the dark side: {dark}");
        assert!(light >= 214, "highlights barely move: {light}");
    }

    #[test]
    fn sponge_changes_saturation_and_keeps_alpha() {
        let chroma = |p: [u8; 4]| {
            let hi = p[..3].iter().max().copied().unwrap();
            let lo = p[..3].iter().min().copied().unwrap();
            i32::from(hi) - i32::from(lo)
        };
        let before = chroma(halves().pixel(50, 50));
        for (mode, more) in [("desaturate", false), ("saturate", true)] {
            let mut s = halves();
            run(
                &mut s,
                "sponge",
                json!({"size":60,"strength":1,"mode":mode}),
                json!([[50, 50]]),
            )
            .unwrap();
            let after = chroma(s.pixel(50, 50));
            assert_eq!(after > before, more, "{mode}: {before} -> {after}");
            assert_eq!(s.pixel(50, 50)[3], 255);
        }
    }

    #[test]
    fn color_replace_shifts_only_matching_pixels() {
        let mut s = halves();
        run(
            &mut s,
            "color-replace",
            json!({"size":300,"tolerance":20}),
            json!([[100, 50]]),
        )
        .unwrap();
        // Red moves by (0,200,0) - (200,40,40); blue is outside the tolerance.
        assert_eq!(s.pixel(100, 50), [0, 200, 0, 255]);
        assert_eq!(s.pixel(250, 50), [40, 40, 200, 255]);
        let mut empty = blank(50, 50);
        let error = run(&mut empty, "color-replace", json!({}), json!([[10, 10]]))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("transparent"), "{error}");
    }

    #[test]
    fn background_erase_removes_the_sampled_color_but_not_the_subject() {
        let mut s = blank(200, 100);
        paint_rect(&mut s, 0, 0, 200, 100, [250, 250, 250, 255]);
        paint_rect(&mut s, 80, 30, 120, 70, [200, 30, 30, 255]);
        run(
            &mut s,
            "background-erase",
            json!({"size":60,"tolerance":16}),
            json!([[60, 50], [140, 50]]),
        )
        .unwrap();
        assert_eq!(s.pixel(60, 50)[3], 0, "background gone");
        assert_eq!(s.pixel(100, 50), [200, 30, 30, 255], "subject protected");
        assert_eq!(s.pixel(190, 95)[3], 255, "outside the stroke");
    }

    #[test]
    fn smudge_drags_color_across_an_edge_and_is_deterministic() {
        let drag = || {
            let mut s = halves();
            run(
                &mut s,
                "smudge",
                json!({"size":30,"strength":0.8,"spacing":0.1}),
                json!([[110, 50], [190, 50]]),
            )
            .unwrap();
            s
        };
        let (a, b) = (drag(), drag());
        assert_eq!(a.tile_map_hash(), b.tile_map_hash());
        let p = a.pixel(165, 50);
        assert!(p[0] > 100 && p[2] < 200, "red dragged into blue: {p:?}");
        assert_eq!(a.pixel(280, 50), [40, 40, 200, 255]);
    }

    #[test]
    fn tool_parameters_that_would_be_ignored_are_errors() {
        let mut s = halves();
        for (blend, brush) in [
            ("normal", json!({"strength": 0.5})),
            ("erase", json!({"tolerance": 5})),
            ("blur", json!({"tolerance": 5})),
            ("dodge", json!({"mode": "saturate"})),
            ("sponge", json!({"range": "shadows"})),
        ] {
            let error = run(&mut s, blend, brush, json!([[10, 10]]))
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("does not apply"), "{blend}: {error}");
        }
        for bad in [
            json!({"strength": 1.5}),
            json!({"tolerance": 3.5}),
            json!({"tolerance": 300}),
            json!({"range": "darks"}),
            json!({"mode": "boost"}),
        ] {
            assert!(Brush::parse(&bad).is_err(), "{bad}");
        }
        let json = Brush::parse(&json!({})).unwrap().to_json();
        for key in ["strength", "tolerance", "range", "mode"] {
            assert!(json.get(key).is_none(), "{key}");
        }
    }
}

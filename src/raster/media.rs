//! Lighting- and material-aware paint modes (v0.13.0 task 7).
//!
//! A `media` object on a brush selects one documented, bounded pixel recipe. The
//! recipes are integer-only after a per-stroke setup, use canvas-fixed procedural
//! grain (no external texture file), and never simulate unbounded physical media.
//! Every recipe is a function of: the dab coverage, the pixel position, the
//! destination pixel and the parameters, so a journaled stroke replays exactly.
//!
//! Pipeline per touched pixel (alpha and coverage are 16-bit):
//!
//! 1. coverage `c = min(65535, alpha * 65535 / opacity)`; grain `g` in 0..65535;
//! 2. the kind adjusts alpha (grain, bristle threshold, knife edge, wet pooling);
//! 3. zones scale alpha by the destination luma weight (shadow/midtone/highlight);
//! 4. deposit scales alpha; the kind transforms colour (multiply, relief light);
//! 5. pickup mixes destination colour into the deposit;
//! 6. preserve-luminance / preserve-chroma pull the result toward the destination.
use super::math::sin_cos_degrees;
use super::*;
use anyhow::Context as _;

pub const ALGORITHM: u32 = 1;
const MAX_WET_LENGTH: f64 = 4096.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Glaze,
    DryBrush,
    Bristle,
    Impasto,
    PaletteKnife,
    WatercolorWash,
    Charcoal,
    Pencil,
    CanvasGrain,
    PaperGrain,
}

pub const KINDS: [(&str, Kind); 10] = [
    ("glaze", Kind::Glaze),
    ("dry-brush", Kind::DryBrush),
    ("bristle", Kind::Bristle),
    ("impasto", Kind::Impasto),
    ("palette-knife", Kind::PaletteKnife),
    ("watercolor-wash", Kind::WatercolorWash),
    ("charcoal", Kind::Charcoal),
    ("pencil", Kind::Pencil),
    ("canvas-grain", Kind::CanvasGrain),
    ("paper-grain", Kind::PaperGrain),
];

impl Kind {
    pub fn name(self) -> &'static str {
        KINDS
            .iter()
            .find(|(_, k)| *k == self)
            .map_or("", |(n, _)| n)
    }

    /// Parameters this kind reads beyond the common set.
    fn extras(self) -> &'static [&'static str] {
        match self {
            Kind::Glaze => &[],
            Kind::DryBrush | Kind::Bristle => &["scale", "direction"],
            Kind::Impasto => &["scale", "direction", "light"],
            Kind::PaletteKnife => &["scale", "light", "edge"],
            Kind::WatercolorWash => &["scale", "wetness", "wet_length", "edge"],
            Kind::Charcoal | Kind::Pencil | Kind::CanvasGrain | Kind::PaperGrain => &["scale"],
        }
    }

    fn default_strength(self) -> f64 {
        match self {
            Kind::Glaze => 0.6,
            Kind::WatercolorWash => 0.5,
            _ => 0.8,
        }
    }

    fn default_scale(self) -> u32 {
        match self {
            Kind::Pencil => 1,
            Kind::Charcoal | Kind::CanvasGrain => 3,
            Kind::Bristle | Kind::DryBrush => 2,
            _ => 4,
        }
    }
}

const COMMON: [&str; 9] = [
    "kind",
    "strength",
    "zones",
    "preserve_luminance",
    "preserve_chroma",
    "pickup",
    "deposit",
    "seed",
    "vary",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub kind: Kind,
    pub strength: f64,
    pub scale: u32,
    pub direction: f64,
    pub light: f64,
    /// Bit 0 shadows, bit 1 midtones, bit 2 highlights.
    pub zones: u8,
    pub preserve_luminance: f64,
    pub preserve_chroma: f64,
    pub pickup: f64,
    pub deposit: f64,
    pub wetness: f64,
    /// Pixels along the stroke over which wetness decays to zero (0 = constant).
    pub wet_length: f64,
    pub edge: f64,
    pub seed: u32,
    /// Mix the stroke seed into the grain so repeated strokes differ.
    pub vary: bool,
}

impl Media {
    pub fn parse(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("[invalid-brush] media must be an object")?;
        let name = object
            .get("kind")
            .and_then(Value::as_str)
            .context("[invalid-brush] media.kind is required")?;
        let kind = KINDS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, k)| *k)
            .with_context(|| {
                format!(
                    "[invalid-brush] unknown media kind {name:?}; use {}",
                    KINDS.map(|(n, _)| n).join(", ")
                )
            })?;
        match object.get("algorithm") {
            None => {}
            Some(v) if v.as_u64() == Some(u64::from(ALGORITHM)) => {}
            Some(v) => bail!(
                "[unsupported-capability] media algorithm {v} is not supported; this build implements {ALGORITHM}"
            ),
        }
        for key in object.keys().filter(|k| *k != "algorithm") {
            if !COMMON.contains(&key.as_str()) && !kind.extras().contains(&key.as_str()) {
                bail!(
                    "[invalid-brush] media.{key} does not apply to {name}; it reads {}",
                    COMMON[1..]
                        .iter()
                        .chain(kind.extras())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        let num =
            |key: &str, default: f64, lo: f64, hi: f64| number_in(object, key, default, lo, hi);
        let scale = num("scale", f64::from(kind.default_scale()), 1.0, 64.0)?;
        if scale.fract() != 0.0 {
            bail!("[invalid-brush] media.scale must be a whole number of pixels")
        }
        let zones = match object.get("zones") {
            None => 7,
            Some(Value::Array(list)) if !list.is_empty() && list.len() <= 3 => {
                let mut bits = 0;
                for item in list {
                    bits |= match item.as_str() {
                        Some("shadow") => 1,
                        Some("midtone") => 2,
                        Some("highlight") => 4,
                        _ => bail!(
                            "[invalid-brush] media.zones entries are shadow, midtone or highlight"
                        ),
                    };
                }
                bits
            }
            Some(_) => {
                bail!("[invalid-brush] media.zones must list 1-3 of shadow, midtone, highlight")
            }
        };
        let seed = num("seed", 0.0, 0.0, 4_294_967_295.0)?;
        if seed.fract() != 0.0 {
            bail!("[invalid-brush] media.seed must be a whole number")
        }
        let vary = match object.get("vary") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => bail!("[invalid-brush] media.vary must be true or false"),
        };
        Ok(Self {
            kind,
            strength: num("strength", kind.default_strength(), 0.0, 1.0)?,
            scale: scale as u32,
            direction: num("direction", 0.0, -360.0, 360.0)?,
            light: num("light", 135.0, -360.0, 360.0)?,
            zones,
            preserve_luminance: num("preserve_luminance", 0.0, 0.0, 1.0)?,
            preserve_chroma: num("preserve_chroma", 0.0, 0.0, 1.0)?,
            pickup: num(
                "pickup",
                if kind == Kind::PaletteKnife {
                    0.35
                } else {
                    0.0
                },
                0.0,
                1.0,
            )?,
            deposit: num("deposit", 1.0, 0.0, 1.0)?,
            wetness: num("wetness", 0.5, 0.0, 1.0)?,
            wet_length: num("wet_length", 0.0, 0.0, MAX_WET_LENGTH)?,
            edge: num("edge", 0.5, 0.0, 1.0)?,
            seed: seed as u32,
            vary,
        })
    }

    /// Canonical form: every parameter this kind reads, plus the algorithm version.
    pub fn to_json(&self) -> Value {
        let zones: Vec<&str> = ["shadow", "midtone", "highlight"]
            .iter()
            .enumerate()
            .filter(|(i, _)| self.zones & (1 << i) != 0)
            .map(|(_, n)| *n)
            .collect();
        let mut out = json!({
            "algorithm": ALGORITHM,
            "kind": self.kind.name(),
            "strength": self.strength,
            "zones": zones,
            "preserve_luminance": self.preserve_luminance,
            "preserve_chroma": self.preserve_chroma,
            "pickup": self.pickup,
            "deposit": self.deposit,
            "seed": self.seed,
            "vary": self.vary,
        });
        for key in self.kind.extras() {
            out[*key] = match *key {
                "scale" => json!(self.scale),
                "direction" => json!(self.direction),
                "light" => json!(self.light),
                "wetness" => json!(self.wetness),
                "wet_length" => json!(self.wet_length),
                _ => json!(self.edge),
            };
        }
        out
    }

    /// Content address of the recipe: algorithm version, parameters and the grain
    /// field they generate (a 64x64 sample), so a changed formula changes the hash.
    pub fn digest(&self) -> String {
        let context = Context::new(self, 0, 65535);
        let mut bytes = format!("pentool-media-v{ALGORITHM}\n{}\n", self.to_json()).into_bytes();
        for y in 0..64 {
            for x in 0..64 {
                bytes.extend_from_slice(&(context.grain(x, y) as u16).to_le_bytes());
            }
        }
        crate::resource::sha256(&bytes)
    }

    pub fn wet_length(&self) -> Option<f64> {
        (self.kind == Kind::WatercolorWash && self.wet_length > 0.0).then_some(self.wet_length)
    }
}

// --- integer noise ---------------------------------------------------------

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash16(x: i64, y: i64, seed: u64, salt: u64) -> i64 {
    let z = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9)
        ^ salt;
    (mix(z) >> 48) as i64
}

/// Smooth value noise, 0..65535, lattice spacing `cell` px.
fn value_noise(x: i64, y: i64, cell: i64, seed: u64, salt: u64) -> i64 {
    let (cx, cy) = (x.div_euclid(cell), y.div_euclid(cell));
    let ease = |t: i64| {
        // t in 0..65536 -> smoothstep in 0..65536
        (t * t / 65536) * (3 * 65536 - 2 * t) / 65536
    };
    let fx = ease((x.rem_euclid(cell) * 65536) / cell);
    let fy = ease((y.rem_euclid(cell) * 65536) / cell);
    let h = |dx, dy| hash16(cx + dx, cy + dy, seed, salt);
    let top = h(0, 0) + (h(1, 0) - h(0, 0)) * fx / 65536;
    let bottom = h(0, 1) + (h(1, 1) - h(0, 1)) * fx / 65536;
    top + (bottom - top) * fy / 65536
}

// --- per-stroke context -----------------------------------------------------

pub struct Context {
    kind: Kind,
    strength: i64,
    scale: i64,
    dir: (i64, i64),
    light: (i64, i64),
    zones: u8,
    preserve_luminance: i64,
    preserve_chroma: i64,
    pickup: i64,
    deposit: i64,
    wetness: i64,
    wet_length: Option<f64>,
    edge: i64,
    seed: u64,
    opacity16: i64,
}

fn q8(v: f64) -> i64 {
    (v * 256.0).round() as i64
}

fn unit(degrees: f64) -> (i64, i64) {
    let (s, c) = sin_cos_degrees(degrees);
    ((c * 32768.0).round() as i64, (s * 32768.0).round() as i64)
}

fn luma(c: [u8; 3]) -> i64 {
    (54 * i64::from(c[0]) + 183 * i64::from(c[1]) + 19 * i64::from(c[2]) + 128) >> 8
}

impl Context {
    pub fn new(media: &Media, stroke_seed: u64, opacity16: u32) -> Self {
        let seed = u64::from(media.seed)
            ^ if media.vary {
                mix(stroke_seed ^ 0x6D65_6469_6131)
            } else {
                0
            };
        Self {
            kind: media.kind,
            strength: q8(media.strength),
            scale: i64::from(media.scale),
            dir: unit(media.direction),
            light: unit(media.light),
            zones: media.zones,
            preserve_luminance: q8(media.preserve_luminance),
            preserve_chroma: q8(media.preserve_chroma),
            pickup: q8(media.pickup),
            deposit: q8(media.deposit),
            wetness: q8(media.wetness),
            wet_length: media.wet_length(),
            edge: q8(media.edge),
            seed,
            opacity16: i64::from(opacity16.max(1)),
        }
    }

    /// Stripe index perpendicular to `direction` (bristle lines run along it).
    fn stripe(&self, x: i64, y: i64) -> i64 {
        ((y * self.dir.0 - x * self.dir.1) >> 15).div_euclid(self.scale)
    }

    fn bristle_height(&self, x: i64, y: i64) -> i64 {
        let stripe = hash16(self.stripe(x, y), 0, self.seed, 0xB157);
        let fine = hash16(x, y, self.seed, 0xF1_4E);
        (stripe * 7 + fine) / 8
    }

    /// Grain value 0..65535 for the kind.
    pub fn grain(&self, x: i64, y: i64) -> i64 {
        let s = self.seed;
        match self.kind {
            Kind::Glaze => 0,
            Kind::PaperGrain => {
                let coarse = value_noise(x, y, self.scale, s, 0x9A9E);
                let fine = value_noise(x, y, (self.scale / 2).max(1), s, 0x9A9F);
                (coarse * 2 + fine) / 3
            }
            Kind::CanvasGrain => {
                let (tx, ty) = (x.div_euclid(self.scale), y.div_euclid(self.scale));
                let over = (tx + ty) & 1 == 0;
                let thread = if over {
                    hash16(tx, 0, s, 0xC0)
                } else {
                    hash16(0, ty, s, 0xC1)
                };
                let along = if over { y } else { x }.rem_euclid(self.scale);
                let t = (along * 65536) / self.scale;
                let ridge = 4 * t * (65536 - t) / 65536; // 0 at thread edges, 65536 mid
                let weave = 65535 - ridge * (65535 - thread / 2) / 65536;
                (weave * 3 + hash16(x, y, s, 0xC2)) / 4
            }
            Kind::Pencil => (hash16(x, y, s, 0x9E_C1) * 3 + value_noise(x, y, 2, s, 0x9E_C2)) / 4,
            Kind::Charcoal => {
                let n =
                    (value_noise(x, y, self.scale * 2, s, 0xC4A) * 2 + hash16(x, y, s, 0xC4B)) / 3;
                ((n - 16384) * 2).clamp(0, 65535)
            }
            Kind::DryBrush | Kind::Bristle | Kind::Impasto => self.bristle_height(x, y),
            Kind::PaletteKnife => value_noise(x, y, self.scale * 4, s, 0x4B1F),
            Kind::WatercolorWash => {
                let pool = value_noise(x, y, self.scale * 4, s, 0x3A7);
                let paper = value_noise(x, y, self.scale, s, 0x3A8);
                (pool * 3 + paper) / 4
            }
        }
    }

    /// Relief height for kinds that are lit.
    fn height(&self, x: i64, y: i64) -> i64 {
        match self.kind {
            Kind::Impasto => {
                (self.bristle_height(x, y) * 3
                    + value_noise(x, y, self.scale * 2, self.seed, 0x1A9))
                    / 4
            }
            _ => self.grain(x, y),
        }
    }

    fn zone_weight(&self, dst: [u8; 4]) -> i64 {
        if self.zones == 7 || dst[3] == 0 {
            return 256;
        }
        let y = luma([dst[0], dst[1], dst[2]]);
        let shadow = ((128 - y) * 4).clamp(0, 256);
        let highlight = ((y - 128) * 4).clamp(0, 256);
        let mid = 256 - ((y - 128).abs() * 2).clamp(0, 256);
        let mut w = 0;
        if self.zones & 1 != 0 {
            w = w.max(shadow);
        }
        if self.zones & 2 != 0 {
            w = w.max(mid);
        }
        if self.zones & 4 != 0 {
            w = w.max(highlight);
        }
        w
    }

    /// New `(alpha16, colour)` for one touched pixel. `travelled` is the stroke
    /// distance (millipixels) at which this pixel was first reached.
    pub fn pixel(
        &self,
        x: i64,
        y: i64,
        alpha16: u32,
        color: [u8; 3],
        dst: [u8; 4],
        travelled: Option<u32>,
    ) -> (u32, [u8; 3]) {
        let a = i64::from(alpha16);
        let coverage = (a * 65535 / self.opacity16).min(65535);
        let g = self.grain(x, y);
        let wet = match (self.wet_length, travelled) {
            (Some(len), Some(t)) => {
                let used = (f64::from(t) / 1000.0 / len).min(1.0);
                (self.wetness as f64 * (1.0 - used)).round() as i64
            }
            _ => self.wetness,
        };
        let mut alpha = match self.kind {
            Kind::Glaze => a * self.strength / 256,
            Kind::CanvasGrain | Kind::PaperGrain | Kind::Pencil | Kind::Charcoal => {
                a * (65535 * 256 - self.strength * g) / (65535 * 256)
            }
            Kind::DryBrush | Kind::Bristle => {
                let strength = if self.kind == Kind::Bristle {
                    self.strength / 2
                } else {
                    self.strength
                };
                // Bare bristle gaps open everywhere and widen toward the soft edge.
                let gate = strength * (32768 + (65535 - coverage) / 2) / 256;
                let k = ((g - gate) * 16).clamp(0, 65535);
                a * k / 65535
            }
            Kind::Impasto => a,
            Kind::PaletteKnife => {
                let k = ((coverage - 16384) * 4).clamp(0, 65535);
                a * k / 65535
            }
            Kind::WatercolorWash => {
                // Pigment pools at soft edges (low coverage) and in paper hollows.
                let pool = self.edge * wet / 256 * (65535 - coverage) / 65535;
                let grain = self.strength * g / 512;
                (a * (256 + pool) / 256 * (65535 - grain) / 65535).min(65535)
            }
        };
        alpha = alpha * self.zone_weight(dst) / 256;
        alpha = (alpha * self.deposit / 256).clamp(0, 65535);
        let dst_a = i64::from(dst[3]);
        let mut c = [
            i64::from(color[0]),
            i64::from(color[1]),
            i64::from(color[2]),
        ];
        match self.kind {
            Kind::Glaze | Kind::WatercolorWash => {
                for (i, v) in c.iter_mut().enumerate() {
                    let multiplied = i64::from(dst[i]) * *v / 255;
                    *v = (dst_a * multiplied + (255 - dst_a) * *v) / 255;
                }
            }
            Kind::Impasto | Kind::PaletteKnife => {
                let here = self.height(x, y);
                let (lx, ly) = (
                    (self.light.0 * 2 + 32768).div_euclid(65536),
                    (self.light.1 * 2 + 32768).div_euclid(65536),
                );
                let behind = self.height(x - lx, y - ly);
                let slope = here - behind; // lit when the surface rises toward the light
                let mut shade = slope * 96 / 65535 * self.strength / 256;
                if self.kind == Kind::PaletteKnife {
                    let ridge = (8192 - (coverage - 24576).abs()).max(0);
                    shade += ridge * 48 / 8192 * self.edge / 256;
                }
                for v in &mut c {
                    *v = (*v + shade).clamp(0, 255);
                }
            }
            _ => {}
        }
        let pickup = if self.kind == Kind::WatercolorWash {
            self.pickup + (256 - self.pickup) * wet / 512
        } else {
            self.pickup
        };
        if pickup > 0 && dst_a > 0 {
            let k = pickup * dst_a / 255;
            for (i, v) in c.iter_mut().enumerate() {
                *v += (i64::from(dst[i]) - *v) * k / 256;
            }
        }
        if dst_a > 0 && (self.preserve_luminance > 0 || self.preserve_chroma > 0) {
            let d = [i64::from(dst[0]), i64::from(dst[1]), i64::from(dst[2])];
            let dl = luma([dst[0], dst[1], dst[2]]);
            let cl = luma([c[0] as u8, c[1] as u8, c[2] as u8]);
            let w = dst_a;
            let delta = (dl - cl) * self.preserve_luminance / 256 * w / 255;
            let target = cl + delta;
            let pc = self.preserve_chroma * w / 255;
            for (i, v) in c.iter_mut().enumerate() {
                let chroma = *v - cl;
                let want = d[i] - dl;
                *v = (target + chroma + (want - chroma) * pc / 256).clamp(0, 255);
            }
        }
        (
            alpha.clamp(0, 65535) as u32,
            [
                c[0].clamp(0, 255) as u8,
                c[1].clamp(0, 255) as u8,
                c[2].clamp(0, 255) as u8,
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(value: Value) -> Media {
        Media::parse(&value).unwrap()
    }

    #[test]
    fn parsing_rejects_ignored_and_out_of_range_parameters() {
        assert!(Media::parse(&json!({})).is_err());
        assert!(Media::parse(&json!({"kind": "oil"})).is_err());
        assert!(Media::parse(&json!({"kind": "glaze", "scale": 3})).is_err());
        assert!(Media::parse(&json!({"kind": "pencil", "light": 10})).is_err());
        assert!(Media::parse(&json!({"kind": "pencil", "strength": 2})).is_err());
        assert!(Media::parse(&json!({"kind": "pencil", "zones": ["dusk"]})).is_err());
        assert!(Media::parse(&json!({"kind": "impasto", "light": 45, "scale": 6})).is_ok());
    }

    #[test]
    fn canonical_form_round_trips_and_digest_tracks_parameters() {
        let a = media(json!({"kind": "paper-grain", "scale": 6, "seed": 3}));
        let b = Media::parse(&a.to_json()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.digest(), b.digest());
        let c = media(json!({"kind": "paper-grain", "scale": 7, "seed": 3}));
        assert_ne!(a.digest(), c.digest());
    }

    #[test]
    fn zones_target_by_destination_luma() {
        let m = media(json!({"kind": "glaze", "strength": 1, "zones": ["shadow"]}));
        let ctx = Context::new(&m, 0, 65535);
        let (dark, _) = ctx.pixel(0, 0, 65535, [200, 0, 0], [10, 10, 10, 255], None);
        let (light, _) = ctx.pixel(0, 0, 65535, [200, 0, 0], [240, 240, 240, 255], None);
        assert!(dark > 60000 && light < 1000, "{dark} {light}");
    }

    #[test]
    fn preserve_luminance_keeps_destination_value() {
        let m = media(json!({"kind": "glaze", "strength": 1, "preserve_luminance": 1}));
        let ctx = Context::new(&m, 0, 65535);
        let dst = [100, 100, 100, 255];
        let (_, c) = ctx.pixel(0, 0, 65535, [250, 20, 20], dst, None);
        let want = luma([100, 100, 100]);
        assert!((luma(c) - want).abs() <= 2, "{c:?}");
    }

    #[test]
    fn grain_is_canvas_fixed_and_bounded() {
        let m = media(json!({"kind": "canvas-grain", "seed": 9}));
        let a = Context::new(&m, 1, 65535);
        let b = Context::new(&m, 2, 65535);
        for y in -5..40 {
            for x in -5..40 {
                let g = a.grain(x, y);
                assert!((0..=65535).contains(&g));
                assert_eq!(g, b.grain(x, y));
            }
        }
    }
}

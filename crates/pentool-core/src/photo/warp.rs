//! Stages 4–5 of process 1: lens correction and geometry composed into one
//! inverse mapping and sampled once with Catmull–Rom bicubic interpolation.
//!
//! Coordinates are continuous pixels with pixel centers at `i + 0.5`. The
//! decoded frame is the developed (default-cropped) raw before orientation; the
//! oriented frame is the same image after the DNG orientation. An output pixel
//! walks back through crop, geometry, orientation and the lens steps (last step
//! first) to a source position per channel.
use super::lens::{Sample, VignetteModel};
use super::math;
use super::opcode::{Op, Opcode};
use super::pixels::Working;
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// Projective `w` at or below this is behind the viewer: the pixel is invalid.
const MIN_W: f64 = 1.0e-12;
const BAND_ROWS: usize = 32;

#[derive(Debug, Clone, PartialEq)]
enum Falloff {
    /// `1 + k0 r^2 + ... + k4 r^10`.
    Gain([f64; 5]),
    /// `1 / (1 + a1 r^2 + a2 r^4 + a3 r^6)`.
    Inverse([f64; 3]),
    /// `2^(amount r^power)`.
    Exp { amount: f64, power: f64 },
}

#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// `WarpRectilinear` per plane `[kr0, kr1, kr2, kr3, kt0, kt1]`, mapping an
    /// output position to the position it is read from.
    Warp {
        center: [f64; 2],
        unit: f64,
        planes: [[f64; 6]; 3],
    },
    /// A gain applied at the position the step sees.
    Vignette {
        center: [f64; 2],
        unit: f64,
        falloff: Falloff,
    },
}

/// The lens steps in forward order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LensChain {
    steps: Vec<Step>,
    /// What was applied, for reports.
    pub applied: Vec<Value>,
}

/// Where lens correction comes from.
pub enum Correction<'a> {
    None,
    /// The DNG's `WarpRectilinear` and `FixVignetteRadial` opcodes with their
    /// list frames in decoded-frame pixels: `[x0, y0, width, height]` of the
    /// stored image (list 1) and of the active area (lists 2 and 3).
    Opcodes {
        opcodes: &'a [Opcode],
        stored: [f64; 4],
        active: [f64; 4],
    },
    /// A lens profile sample interpolated at the capture's focal length and aperture.
    Profile(Sample),
}

fn corner_distance(center: [f64; 2], frame: [f64; 4]) -> f64 {
    let [x0, y0, w, h] = frame;
    let mut best: f64 = 0.0;
    for (x, y) in [(x0, y0), (x0 + w, y0), (x0, y0 + h), (x0 + w, y0 + h)] {
        let (dx, dy) = (x - center[0], y - center[1]);
        best = best.max((dx * dx + dy * dy).sqrt());
    }
    best.max(1.0)
}

fn num(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

impl LensChain {
    /// Build the chain for the decoded `width` x `height` frame from a develop
    /// object's `lens` group. Forward order: manual chromatic aberration, the
    /// profile or opcode steps, manual distortion, manual vignetting.
    pub fn new(lens: &Value, width: usize, height: usize, correction: Correction) -> Self {
        let frame = [0.0, 0.0, width as f64, height as f64];
        let middle = [width as f64 / 2.0, height as f64 / 2.0];
        let corner = corner_distance(middle, frame);
        let ca = &lens["chromatic_aberration"];
        let remove = ca.get("remove").and_then(Value::as_bool).unwrap_or(false);
        let mut chain = Self::default();

        let (red, blue) = (num(ca, "red_cyan"), num(ca, "blue_yellow"));
        if red != 0.0 || blue != 0.0 {
            let scale = |s: f64| [s, 0.0, 0.0, 0.0, 0.0, 0.0];
            chain.steps.push(Step::Warp {
                center: middle,
                unit: corner,
                planes: [
                    scale(1.0 + red / 20_000.0),
                    scale(1.0),
                    scale(1.0 + blue / 20_000.0),
                ],
            });
            chain
                .applied
                .push(json!({"step": "manual-chromatic-aberration"}));
        }

        match correction {
            Correction::None => {}
            Correction::Opcodes {
                opcodes,
                stored,
                active,
            } => {
                for op in opcodes.iter().filter(|op| op.applied()) {
                    let frame = if op.list == 1 { stored } else { active };
                    let at = |c: [f64; 2]| [frame[0] + c[0] * frame[2], frame[1] + c[1] * frame[3]];
                    match &op.op {
                        Op::WarpRectilinear { planes, center } => {
                            let center = at(*center);
                            let plane = |c: usize| {
                                let index = if remove {
                                    c.min(planes.len() - 1)
                                } else {
                                    1.min(planes.len() - 1)
                                };
                                planes[index]
                            };
                            chain.steps.push(Step::Warp {
                                center,
                                unit: corner_distance(center, frame),
                                planes: [plane(0), plane(1), plane(2)],
                            });
                            chain
                                .applied
                                .push(json!({"step": "opcode", "list": op.list, "id": 1}));
                        }
                        Op::FixVignetteRadial { k, center } => {
                            let center = at(*center);
                            chain.steps.push(Step::Vignette {
                                center,
                                unit: corner_distance(center, frame),
                                falloff: Falloff::Gain(*k),
                            });
                            chain
                                .applied
                                .push(json!({"step": "opcode", "list": op.list, "id": 3}));
                        }
                        _ => {}
                    }
                }
            }
            Correction::Profile(sample) => {
                let long = width.max(height) as f64;
                let place = |center: Option<[f64; 2]>, scale: Option<f64>| {
                    let c = center.map_or(middle, |c| [c[0] * long, c[1] * long]);
                    (
                        c,
                        scale.map_or_else(|| corner_distance(c, frame), |s| s * long),
                    )
                };
                if let Some(v) = &sample.vignette {
                    let (center, unit) =
                        place(v.center.or(sample.center), v.scale.or(sample.scale));
                    chain.steps.push(Step::Vignette {
                        center,
                        unit,
                        falloff: match v.model {
                            VignetteModel::Gain(k) => Falloff::Gain(k),
                            VignetteModel::Falloff(a) => Falloff::Inverse(a),
                        },
                    });
                }
                if sample.radial.is_some() || sample.chromatic_aberration.is_some() {
                    let (center, unit) = place(sample.center, sample.scale);
                    let [k1, k2, k3] = sample.radial.unwrap_or([0.0; 3]);
                    let [t0, t1] = sample.tangential;
                    let ca = match sample.chromatic_aberration {
                        Some([r, b]) if remove => [r, 1.0, b],
                        _ => [1.0; 3],
                    };
                    let plane = |s: f64| [s, k1 * s, k2 * s, k3 * s, t0 * s, t1 * s];
                    chain.steps.push(Step::Warp {
                        center,
                        unit,
                        planes: [plane(ca[0]), plane(ca[1]), plane(ca[2])],
                    });
                }
                chain.applied.push(json!({
                    "step": "profile",
                    "focal": super::dng::number(sample.focal),
                    "aperture": super::dng::number(sample.aperture),
                }));
            }
        }

        let distortion = num(lens, "distortion");
        if distortion != 0.0 {
            let k = -distortion / 400.0;
            let plane = [1.0, k, 0.0, 0.0, 0.0, 0.0];
            chain.steps.push(Step::Warp {
                center: middle,
                unit: corner,
                planes: [plane; 3],
            });
            chain.applied.push(json!({"step": "manual-distortion"}));
        }
        let vignetting = &lens["vignetting"];
        let amount = num(vignetting, "amount");
        if amount != 0.0 {
            let midpoint = vignetting
                .get("midpoint")
                .and_then(Value::as_f64)
                .unwrap_or(50.0);
            chain.steps.push(Step::Vignette {
                center: middle,
                unit: corner,
                falloff: Falloff::Exp {
                    amount: amount / 100.0,
                    power: 1.0 + 4.0 * midpoint / 100.0,
                },
            });
            chain.applied.push(json!({"step": "manual-vignetting"}));
        }
        chain
    }

    pub fn is_identity(&self) -> bool {
        self.steps.is_empty()
    }

    /// Whether every channel maps identically.
    fn uniform(&self) -> bool {
        self.steps.iter().all(|step| match step {
            Step::Warp { planes, .. } => planes[0] == planes[1] && planes[1] == planes[2],
            Step::Vignette { .. } => true,
        })
    }

    /// The source position and gain for channel `c` of decoded-frame position `p`.
    fn invert(&self, mut p: [f64; 2], c: usize) -> ([f64; 2], f64) {
        let mut gain = 1.0;
        for step in self.steps.iter().rev() {
            match step {
                Step::Warp {
                    center,
                    unit,
                    planes,
                } => {
                    let k = &planes[c];
                    let x = (p[0] - center[0]) / unit;
                    let y = (p[1] - center[1]) / unit;
                    let r2 = x * x + y * y;
                    let f = k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3]));
                    let dx = f * x + k[4] * 2.0 * x * y + k[5] * (r2 + 2.0 * x * x);
                    let dy = f * y + k[5] * 2.0 * x * y + k[4] * (r2 + 2.0 * y * y);
                    p = [center[0] + unit * dx, center[1] + unit * dy];
                }
                Step::Vignette {
                    center,
                    unit,
                    falloff,
                } => {
                    let x = (p[0] - center[0]) / unit;
                    let y = (p[1] - center[1]) / unit;
                    let r2 = x * x + y * y;
                    gain *= match falloff {
                        Falloff::Gain(k) => {
                            1.0 + r2 * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * (k[3] + r2 * k[4]))))
                        }
                        Falloff::Inverse(a) => {
                            1.0 / (1.0 + r2 * (a[0] + r2 * (a[1] + r2 * a[2]))).max(1.0e-3)
                        }
                        Falloff::Exp { amount, power } => {
                            math::exp2(amount * math::pow(r2, power / 2.0))
                        }
                    };
                }
            }
        }
        (p, gain)
    }
}

/// The oriented frame size of a `width` x `height` image.
pub fn oriented(orientation: u8, width: usize, height: usize) -> (usize, usize) {
    if orientation >= 5 {
        (height, width)
    } else {
        (width, height)
    }
}

type Matrix3 = [[f64; 3]; 3];

fn multiply(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn invert(m: &Matrix3) -> Option<Matrix3> {
    let cof =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adj = [
        [cof(1, 2, 1, 2), -cof(0, 2, 1, 2), cof(0, 1, 1, 2)],
        [-cof(1, 2, 0, 2), cof(0, 2, 0, 2), -cof(0, 1, 0, 2)],
        [cof(1, 2, 0, 1), -cof(0, 2, 0, 1), cof(0, 1, 0, 1)],
    ];
    let det = m[0][0] * adj[0][0] + m[0][1] * adj[1][0] + m[0][2] * adj[2][0];
    (det.abs() > 1.0e-15).then(|| adj.map(|row| row.map(|v| v / det)))
}

const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The forward geometry `Offset · Scale · Aspect · Rotate · Perspective` in
/// coordinates centered on the oriented frame and divided by half its long edge.
pub fn forward(geometry: &Value, width: f64, height: f64) -> Matrix3 {
    let half = width.max(height) / 2.0;
    let v = num(geometry, "vertical") / 100.0;
    let h = num(geometry, "horizontal") / 100.0;
    let (sin, cos) = math::sin_cos(math::radians(num(geometry, "rotate")));
    let aspect = math::exp2(num(geometry, "aspect") / 200.0);
    let scale = geometry
        .get("scale")
        .and_then(Value::as_f64)
        .unwrap_or(100.0)
        / 100.0;
    let offset = geometry
        .get("offset")
        .and_then(Value::as_array)
        .map(|o| [o[0].as_f64().unwrap_or(0.0), o[1].as_f64().unwrap_or(0.0)])
        .unwrap_or([0.0; 2]);
    let perspective = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-0.5 * h, 0.5 * v, 1.0]];
    let rotate = [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]];
    let stretch = [
        [scale * aspect, 0.0, 0.0],
        [0.0, scale / aspect, 0.0],
        [0.0, 0.0, 1.0],
    ];
    let shift = [
        [1.0, 0.0, offset[0] / 100.0 * width / (2.0 * half)],
        [0.0, 1.0, offset[1] / 100.0 * height / (2.0 * half)],
        [0.0, 0.0, 1.0],
    ];
    multiply(
        &shift,
        &multiply(&stretch, &multiply(&rotate, &perspective)),
    )
}

/// Map a point normalized to the oriented frame into the unoriented frame.
pub fn unorient(orientation: u8, x: f64, y: f64) -> (f64, f64) {
    match orientation {
        2 => (1.0 - x, y),
        3 => (1.0 - x, 1.0 - y),
        4 => (x, 1.0 - y),
        5 => (y, x),
        6 => (y, 1.0 - x),
        7 => (1.0 - y, 1.0 - x),
        8 => (1.0 - y, x),
        _ => (x, y),
    }
}

/// The composed inverse mapping of one development, without the crop.
#[derive(Debug, Clone)]
pub struct Mapping {
    /// Decoded frame.
    pub width: usize,
    pub height: usize,
    pub orientation: u8,
    /// Oriented frame.
    pub frame: (usize, usize),
    inverse: Option<Matrix3>,
    pub lens: LensChain,
}

impl Mapping {
    pub fn new(
        width: usize,
        height: usize,
        orientation: u8,
        geometry: &Value,
        lens: LensChain,
    ) -> Result<Self> {
        let frame = oriented(orientation, width, height);
        let forward = forward(geometry, frame.0 as f64, frame.1 as f64);
        let inverse = if forward == IDENTITY {
            None
        } else {
            Some(invert(&forward).ok_or_else(|| {
                anyhow::anyhow!("[invalid-develop] the geometry is singular; reduce vertical or horizontal perspective")
            })?)
        };
        Ok(Self {
            width,
            height,
            orientation,
            frame,
            inverse,
            lens,
        })
    }

    /// Whether the mapping is orientation (and crop) only.
    pub fn is_exact(&self) -> bool {
        self.inverse.is_none() && self.lens.is_identity()
    }

    /// The decoded-frame position of oriented-frame position `(x, y)` before lens
    /// correction, or `None` when the geometry maps it behind the viewer.
    fn geometric(&self, x: f64, y: f64) -> Option<[f64; 2]> {
        let (fw, fh) = (self.frame.0 as f64, self.frame.1 as f64);
        let (mut x, mut y) = (x, y);
        if let Some(m) = &self.inverse {
            let half = fw.max(fh) / 2.0;
            let gx = (x - fw / 2.0) / half;
            let gy = (y - fh / 2.0) / half;
            let w = m[2][0] * gx + m[2][1] * gy + m[2][2];
            if w <= MIN_W || w.is_nan() {
                return None;
            }
            x = fw / 2.0 + half * (m[0][0] * gx + m[0][1] * gy + m[0][2]) / w;
            y = fh / 2.0 + half * (m[1][0] * gx + m[1][1] * gy + m[1][2]) / w;
        }
        let (u, v) = unorient(self.orientation, x / fw, y / fh);
        Some([u * self.width as f64, v * self.height as f64])
    }

    fn inside(&self, p: [f64; 2]) -> bool {
        p[0] >= 0.0 && p[0] <= self.width as f64 && p[1] >= 0.0 && p[1] <= self.height as f64
    }

    /// Whether every channel of oriented-frame position `(x, y)` reads inside the source.
    pub fn valid(&self, x: f64, y: f64) -> bool {
        let Some(p) = self.geometric(x, y) else {
            return false;
        };
        if self.lens.uniform() {
            return self.inside(self.lens.invert(p, 1).0);
        }
        (0..3).all(|c| self.inside(self.lens.invert(p, c).0))
    }

    /// Source positions and gains of the three channels, or `None` when invalid.
    fn sources(&self, x: f64, y: f64) -> Option<[([f64; 2], f64); 3]> {
        let p = self.geometric(x, y)?;
        let out = if self.lens.uniform() {
            let s = self.lens.invert(p, 1);
            [s, s, s]
        } else {
            [0, 1, 2].map(|c| self.lens.invert(p, c))
        };
        out.iter().all(|(q, _)| self.inside(*q)).then_some(out)
    }
}

/// Catmull–Rom weights for taps -1, 0, 1, 2 at fraction `t`.
fn weights(t: f64) -> [f64; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        (-t3 + 2.0 * t2 - t) / 2.0,
        (3.0 * t3 - 5.0 * t2 + 2.0) / 2.0,
        (-3.0 * t3 + 4.0 * t2 + t) / 2.0,
        (t3 - t2) / 2.0,
    ]
}

fn bicubic(image: &[f32], width: usize, height: usize, p: [f64; 2], c: usize) -> f64 {
    let fx = p[0] - 0.5;
    let fy = p[1] - 0.5;
    let (ix, iy) = (fx.floor(), fy.floor());
    let (wx, wy) = (weights(fx - ix), weights(fy - iy));
    let clamp = |v: f64, n: usize| (v.max(0.0) as usize).min(n - 1);
    let mut sum = 0.0;
    for (j, wy) in wy.iter().enumerate() {
        let row = clamp(iy - 1.0 + j as f64, height) * width;
        let mut line = 0.0;
        for (i, wx) in wx.iter().enumerate() {
            let col = clamp(ix - 1.0 + i as f64, width);
            line += wx * f64::from(image[(row + col) * 3 + c]);
        }
        sum += wy * line;
    }
    sum
}

/// An integer crop `[x0, y0, x1, y1)` of the oriented, post-geometry frame.
pub type Crop = [usize; 4];

/// Fills output row `y` (RGB, alpha) and returns its count of invalid pixels.
type RowFn<'a> = dyn Fn(usize, &mut [f32], &mut [f32]) -> u64 + Sync + 'a;

/// Run `row` for every output row in parallel bands, checking cancellation
/// between bands on the calling thread. Returns the sum of the row results.
fn parallel_rows(
    width: usize,
    height: usize,
    rgb: &mut [f32],
    alpha: &mut [f32],
    row: &RowFn<'_>,
) -> Result<u64> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 16);
    let mut total = 0;
    let band = BAND_ROWS * threads;
    let mut y0 = 0;
    for (rgb_band, alpha_band) in rgb
        .chunks_mut(band * width * 3)
        .zip(alpha.chunks_mut(band * width))
    {
        let rows = rgb_band.len() / (width * 3);
        let per = rows.div_ceil(threads);
        let parts: Vec<u64> = std::thread::scope(|scope| {
            let handles: Vec<_> = rgb_band
                .chunks_mut(per * width * 3)
                .zip(alpha_band.chunks_mut(per * width))
                .enumerate()
                .map(|(n, (rgb, alpha))| {
                    let start = y0 + n * per;
                    scope.spawn(move || {
                        let mut count = 0;
                        for (k, (rgb, alpha)) in rgb
                            .chunks_mut(width * 3)
                            .zip(alpha.chunks_mut(width))
                            .enumerate()
                        {
                            count += row(start + k, rgb, alpha);
                        }
                        count
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("row worker"))
                .collect()
        });
        total += parts.iter().sum::<u64>();
        y0 += rows;
        super::check_cancelled()?;
    }
    debug_assert_eq!(y0, height);
    Ok(total)
}

/// Resample `image` (interleaved RGB of the decoded frame) through `mapping`
/// into `crop`. Invalid pixels are black and transparent; alpha is present only
/// when some pixel is invalid. Returns the image and the invalid pixel count.
pub fn render(image: &[f32], mapping: &Mapping, crop: Crop) -> Result<(Working, u64)> {
    let [x0, y0, x1, y1] = crop;
    let (w, h) = (x1 - x0, y1 - y0);
    let mut out = Working::new(w as u32, h as u32, true)?;
    let mut alpha = out.alpha.take().unwrap();
    let (sw, sh) = (mapping.width, mapping.height);
    let (fw, fh) = (mapping.frame.0 as f64, mapping.frame.1 as f64);
    let exact = mapping.is_exact();
    let row = |j: usize, rgb: &mut [f32], alpha: &mut [f32]| -> u64 {
        let y = (y0 + j) as f64 + 0.5;
        let mut invalid = 0;
        for i in 0..w {
            let x = (x0 + i) as f64 + 0.5;
            let px = &mut rgb[i * 3..i * 3 + 3];
            if exact {
                let (u, v) = unorient(mapping.orientation, x / fw, y / fh);
                let sx = ((u * sw as f64).floor() as usize).min(sw - 1);
                let sy = ((v * sh as f64).floor() as usize).min(sh - 1);
                px.copy_from_slice(&image[(sy * sw + sx) * 3..(sy * sw + sx) * 3 + 3]);
                continue;
            }
            match mapping.sources(x, y) {
                Some(sources) => {
                    for (c, (p, gain)) in sources.iter().enumerate() {
                        px[c] = (bicubic(image, sw, sh, *p, c) * gain) as f32;
                    }
                }
                None => {
                    px.fill(0.0);
                    alpha[i] = 0.0;
                    invalid += 1;
                }
            }
        }
        invalid
    };
    let invalid = parallel_rows(w, h, &mut out.rgb, &mut alpha, &row)?;
    if invalid > 0 {
        out.alpha = Some(alpha);
    }
    Ok((out, invalid))
}

/// Count the invalid output pixels of `crop` without sampling.
pub fn count_invalid(mapping: &Mapping, crop: Crop) -> Result<u64> {
    if mapping.is_exact() {
        return Ok(0);
    }
    let [x0, y0, x1, y1] = crop;
    let w = x1 - x0;
    let rows = y1 - y0;
    // One scratch row per worker is enough; the buffers only drive the bands.
    let mut rgb = vec![0.0f32; w * 3 * rows.min(BAND_ROWS * 16)];
    let mut alpha = vec![0.0f32; w * rows.min(BAND_ROWS * 16)];
    let mut total = 0;
    let mut start = 0;
    while start < rows {
        let n = (rows - start).min(BAND_ROWS * 16);
        let first = start;
        let row = |j: usize, _: &mut [f32], _: &mut [f32]| -> u64 {
            let y = (y0 + first + j) as f64 + 0.5;
            (0..w)
                .filter(|i| !mapping.valid((x0 + i) as f64 + 0.5, y))
                .count() as u64
        };
        total += parallel_rows(w, n, &mut rgb[..w * 3 * n], &mut alpha[..w * n], &row)?;
        start += n;
    }
    Ok(total)
}

fn snap(v: f64) -> usize {
    v.round().max(0.0) as usize
}

/// Resolve `crop` against the oriented frame: the stored rectangle snapped to
/// pixels, then, with `constrain`, the largest rectangle of the crop's aspect
/// centered in it that contains only valid pixels.
pub fn resolve_crop(crop: &Value, mapping: &Mapping) -> Result<(Crop, Value)> {
    let (fw, fh) = (mapping.frame.0 as f64, mapping.frame.1 as f64);
    let rect = crop
        .get("rect")
        .and_then(Value::as_array)
        .map(|r| [0, 1, 2, 3].map(|i| r[i].as_f64().unwrap_or(0.0)))
        .unwrap_or([0.0, 0.0, 1.0, 1.0]);
    let mut x0 = snap(rect[0] * fw).min(mapping.frame.0 - 1);
    let mut y0 = snap(rect[1] * fh).min(mapping.frame.1 - 1);
    let mut x1 = snap((rect[0] + rect[2]) * fw).clamp(x0 + 1, mapping.frame.0);
    let mut y1 = snap((rect[1] + rect[3]) * fh).clamp(y0 + 1, mapping.frame.1);
    let constrain = crop
        .get("constrain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut report = json!({"constrained": false});
    if constrain {
        let aspect = match crop.get("aspect").and_then(Value::as_str).unwrap_or("free") {
            "original" => fw / fh,
            "free" => (x1 - x0) as f64 / (y1 - y0) as f64,
            other => match super::develop::crop_aspect(other) {
                Some(Some((a, b))) => f64::from(a) / f64::from(b),
                _ => bail!("[invalid-develop] crop.aspect {other:?} must be free, original or W:H"),
            },
        };
        let (cx, cy) = ((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0);
        let (rw, rh) = ((x1 - x0) as f64, (y1 - y0) as f64);
        let (mw, mh) = if rw / rh > aspect {
            (rh * aspect, rh)
        } else {
            (rw, rw / aspect)
        };
        if !mapping.valid(cx, cy) {
            bail!("[invalid-develop] crop.constrain cannot keep the crop's center inside the image; reduce the geometry correction or move the crop")
        }
        let fits = |t: f64| {
            let (hw, hh) = (mw * t / 2.0, mh * t / 2.0);
            (0..=256).all(|k| {
                let s = f64::from(k) / 256.0;
                let x = cx - hw + 2.0 * hw * s;
                let y = cy - hh + 2.0 * hh * s;
                mapping.valid(x, cy - hh)
                    && mapping.valid(x, cy + hh)
                    && mapping.valid(cx - hw, y)
                    && mapping.valid(cx + hw, y)
            })
        };
        let t = if fits(1.0) {
            1.0
        } else {
            let (mut low, mut high) = (0.0, 1.0);
            for _ in 0..30 {
                let mid = (low + high) / 2.0;
                if fits(mid) {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            low
        };
        let (hw, hh) = (mw * t / 2.0, mh * t / 2.0);
        let nx0 = (cx - hw).ceil().max(x0 as f64) as usize;
        let ny0 = (cy - hh).ceil().max(y0 as f64) as usize;
        let nx1 = ((cx + hw).floor() as usize).min(x1);
        let ny1 = ((cy + hh).floor() as usize).min(y1);
        if nx1 <= nx0 || ny1 <= ny0 {
            bail!("[invalid-develop] crop.constrain leaves no valid pixels; reduce the geometry correction")
        }
        (x0, y0, x1, y1) = (nx0, ny0, nx1, ny1);
        report = json!({"constrained": true});
    }
    report["rect"] = json!([x0, y0, x1 - x0, y1 - y0]);
    Ok(([x0, y0, x1, y1], report))
}

// ---------------------------------------------------------------------------
// Upright.

/// A line segment in coordinates centered on the oriented frame and divided by
/// half its long edge, with its family.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub vertical: bool,
}

impl Segment {
    fn length(&self) -> f64 {
        let (dx, dy) = (self.b[0] - self.a[0], self.b[1] - self.a[1]);
        (dx * dx + dy * dy).sqrt()
    }
}

/// Box-downsample a working image to at most `edge` pixels on its long side
/// as `sqrt` of luminance.
pub fn analysis_luminance(image: &Working, edge: usize) -> (Vec<f64>, usize, usize) {
    let (w, h) = (image.width as usize, image.height as usize);
    let step = w.max(h).div_ceil(edge).max(1);
    let (aw, ah) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = vec![0.0; aw * ah];
    for (j, line) in out.chunks_mut(aw).enumerate() {
        for (i, cell) in line.iter_mut().enumerate() {
            let mut sum = 0.0;
            let mut n = 0.0;
            for y in j * step..((j + 1) * step).min(h) {
                for x in i * step..((i + 1) * step).min(w) {
                    let p = &image.rgb[(y * w + x) * 3..(y * w + x) * 3 + 3];
                    let valid = image.alpha.as_ref().is_none_or(|a| a[y * w + x] > 0.0);
                    if valid {
                        sum += 0.288 * f64::from(p[0])
                            + 0.7119 * f64::from(p[1])
                            + 0.0001 * f64::from(p[2]);
                        n += 1.0;
                    }
                }
            }
            *cell = if n > 0.0 {
                (sum / n).max(0.0).sqrt()
            } else {
                0.0
            };
        }
    }
    (out, aw, ah)
}

const THETA_STEP: f64 = 0.25;
const THETA_SPAN: f64 = 25.0;
const AGREE: f64 = 10.0;
const MAX_PEAKS: usize = 16;

/// Detect near-vertical and near-horizontal line segments with a Hough
/// transform over gradient-agreeing edge pixels. `frame` is the oriented frame
/// the `aw` x `ah` analysis image covers.
pub fn detect_lines(lum: &[f64], aw: usize, ah: usize, frame: (usize, usize)) -> Vec<Segment> {
    if aw < 8 || ah < 8 {
        return Vec::new();
    }
    // Sobel gradients.
    let mut edges: Vec<(f64, f64, f64)> = Vec::new(); // (x, y, gradient angle in degrees 0–180)
    let mut magnitudes = vec![0.0; aw * ah];
    let at = |x: usize, y: usize| lum[y * aw + x];
    let mut gradients = vec![(0.0, 0.0); aw * ah];
    for y in 1..ah - 1 {
        for x in 1..aw - 1 {
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x - 1, y)
                - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x, y - 1)
                - at(x + 1, y - 1);
            magnitudes[y * aw + x] = (gx * gx + gy * gy).sqrt();
            gradients[y * aw + x] = (gx, gy);
        }
    }
    let mean = magnitudes.iter().sum::<f64>() / magnitudes.len() as f64;
    let threshold = (4.0 * mean).max(0.02);
    for y in 1..ah - 1 {
        for x in 1..aw - 1 {
            let m = magnitudes[y * aw + x];
            if m < threshold {
                continue;
            }
            let (gx, gy) = gradients[y * aw + x];
            let mut angle = math::degrees(math::atan2(gy, gx));
            if angle < 0.0 {
                angle += 180.0;
            }
            if angle >= 180.0 {
                angle -= 180.0;
            }
            edges.push((x as f64 + 0.5, y as f64 + 0.5, angle));
        }
    }
    let diagonal = ((aw * aw + ah * ah) as f64).sqrt().ceil() as i64;
    let rhos = (2 * diagonal + 1) as usize;
    let thetas = (2.0 * THETA_SPAN / THETA_STEP) as usize + 1;
    let min_votes = 30.max(aw.max(ah) / 20) as u32;
    let mut segments = Vec::new();
    let half = frame.0.max(frame.1) as f64 / 2.0;
    let scale_x = frame.0 as f64 / aw as f64;
    let scale_y = frame.1 as f64 / ah as f64;
    let to_frame = |x: f64, y: f64| {
        [
            (x * scale_x - frame.0 as f64 / 2.0) / half,
            (y * scale_y - frame.1 as f64 / 2.0) / half,
        ]
    };
    // Normal angle family centers: 0° (vertical lines), 90° (horizontal lines).
    for (vertical, base) in [(true, 0.0), (false, 90.0)] {
        let table: Vec<(f64, f64, f64)> = (0..thetas)
            .map(|t| {
                let theta = base - THETA_SPAN + t as f64 * THETA_STEP;
                let (s, c) = math::sin_cos(math::radians(theta));
                (theta, c, s)
            })
            .collect();
        let differ = |a: f64, b: f64| {
            let d = (a - b).rem_euclid(180.0);
            d.min(180.0 - d)
        };
        let mut votes = vec![0u32; thetas * rhos];
        for &(x, y, angle) in &edges {
            for (t, &(theta, c, s)) in table.iter().enumerate() {
                if differ(angle, theta) > AGREE {
                    continue;
                }
                let rho = (x * c + y * s).round() as i64 + diagonal;
                votes[t * rhos + rho as usize] += 1;
            }
        }
        // Peaks with non-maximum suppression, strongest first.
        let mut order: Vec<usize> = (0..votes.len())
            .filter(|&i| votes[i] >= min_votes)
            .collect();
        order.sort_by(|a, b| votes[*b].cmp(&votes[*a]).then(a.cmp(b)));
        let mut peaks: Vec<(usize, usize)> = Vec::new();
        for index in order {
            if peaks.len() >= MAX_PEAKS {
                break;
            }
            let (t, r) = (index / rhos, index % rhos);
            if peaks
                .iter()
                .any(|&(pt, pr)| pt.abs_diff(t) <= 8 && pr.abs_diff(r) <= 10)
            {
                continue;
            }
            peaks.push((t, r));
        }
        for (t, r) in peaks {
            let (theta, c, s) = table[t];
            let rho = r as f64 - diagonal as f64;
            let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
            for &(x, y, angle) in &edges {
                if differ(angle, theta) > AGREE || ((x * c + y * s) - rho).abs() > 1.0 {
                    continue;
                }
                // Position along the line direction (-s, c).
                let along = -x * s + y * c;
                low = low.min(along);
                high = high.max(along);
            }
            if high <= low || high.is_nan() || low.is_nan() {
                continue;
            }
            let point = |u: f64| (rho * c - u * s, rho * s + u * c);
            let (ax, ay) = point(low);
            let (bx, by) = point(high);
            segments.push(Segment {
                a: to_frame(ax, ay),
                b: to_frame(bx, by),
                vertical,
            });
        }
    }
    segments
}

/// Segments from `geometry.guides` (points normalized to the oriented frame).
pub fn guide_segments(guides: &Value, frame: (usize, usize)) -> Vec<Segment> {
    let (fw, fh) = (frame.0 as f64, frame.1 as f64);
    let half = fw.max(fh) / 2.0;
    let point = |p: &Value| {
        [
            (p[0].as_f64().unwrap_or(0.0) * fw - fw / 2.0) / half,
            (p[1].as_f64().unwrap_or(0.0) * fh - fh / 2.0) / half,
        ]
    };
    guides
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| {
            let (a, b) = (point(&g[0]), point(&g[1]));
            Segment {
                a,
                b,
                vertical: (b[1] - a[1]).abs() > (b[0] - a[0]).abs(),
            }
        })
        .collect()
}

fn cost(segments: &[Segment], geometry: &Value, frame: (usize, usize)) -> f64 {
    let m = forward(geometry, frame.0 as f64, frame.1 as f64);
    let map = |p: [f64; 2]| {
        let w = m[2][0] * p[0] + m[2][1] * p[1] + m[2][2];
        (w > 1.0e-6).then(|| {
            [
                (m[0][0] * p[0] + m[0][1] * p[1] + m[0][2]) / w,
                (m[1][0] * p[0] + m[1][1] * p[1] + m[1][2]) / w,
            ]
        })
    };
    let mut total = 0.0;
    for s in segments {
        let length = s.length();
        let deviation = match (map(s.a), map(s.b)) {
            (Some(a), Some(b)) => {
                let angle = math::degrees(math::atan2(b[1] - a[1], b[0] - a[0]));
                let target = if s.vertical { 90.0 } else { 0.0 };
                let d = (angle - target).rem_euclid(180.0);
                d.min(180.0 - d).min(10.0)
            }
            _ => 10.0,
        };
        total += length * deviation * deviation;
    }
    total
}

/// Which geometry keys an upright mode solves.
pub fn solved_keys(mode: &str, segments: &[Segment]) -> Vec<&'static str> {
    match mode {
        "level" => vec!["rotate"],
        "vertical" => vec!["rotate", "vertical"],
        "full" => vec!["rotate", "vertical", "horizontal"],
        "guided" => {
            let verticals = segments.iter().filter(|s| s.vertical).count();
            let horizontals = segments.len() - verticals;
            let mut keys = vec!["rotate"];
            if verticals >= 2 {
                keys.push("vertical");
            }
            if horizontals >= 2 {
                keys.push("horizontal");
            }
            keys
        }
        _ => Vec::new(),
    }
}

/// Coordinate descent over the solved keys: a coarse grid in order of
/// increasing magnitude, then refinement at a tenth and a hundredth of the step,
/// three cycles. Other geometry values are held fixed.
pub fn solve(
    segments: &[Segment],
    keys: &[&str],
    geometry: &Value,
    frame: (usize, usize),
) -> Value {
    let mut current = geometry.clone();
    if !current.is_object() {
        current = json!({});
    }
    for key in keys {
        current[*key] = json!(0.0);
    }
    let bounds = |key: &str| match key {
        "rotate" => (0.5, 25.0),
        _ => (2.0, 100.0),
    };
    let mut best = cost(segments, &current, frame);
    for _ in 0..3 {
        for key in keys {
            let (step, limit) = bounds(key);
            let center = current[*key].as_f64().unwrap_or(0.0);
            let mut candidates = vec![0.0];
            let mut k = 1.0;
            while k * step <= limit + 1e-9 {
                candidates.push(k * step);
                candidates.push(-k * step);
                k += 1.0;
            }
            let mut value = center;
            for scale in [1.0, 0.1, 0.01] {
                let around = if scale == 1.0 { 0.0 } else { value };
                let list: Vec<f64> = if scale == 1.0 {
                    candidates.clone()
                } else {
                    (1..=10)
                        .flat_map(|n| {
                            let d = step * scale * f64::from(n);
                            [around + d, around - d]
                        })
                        .collect()
                };
                for candidate in list {
                    if candidate.abs() > limit {
                        continue;
                    }
                    current[*key] = json!(candidate);
                    let c = cost(segments, &current, frame);
                    if c < best {
                        best = c;
                        value = candidate;
                    }
                }
                current[*key] = json!(value);
            }
        }
    }
    let round = |v: f64, digits: f64| (v * digits).round() / digits;
    let mut out = serde_json::Map::new();
    for key in keys {
        let v = current[*key].as_f64().unwrap_or(0.0);
        let v = if *key == "rotate" {
            round(v, 100.0)
        } else {
            round(v, 10.0)
        };
        out.insert(
            (*key).to_string(),
            super::dng::number(if v == 0.0 { 0.0 } else { v }),
        );
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: usize, height: usize) -> Vec<f32> {
        (0..width * height)
            .flat_map(|i| {
                let (x, y) = ((i % width) as f32, (i / width) as f32);
                [x, y, x + y]
            })
            .collect()
    }

    #[test]
    fn identity_and_orientation_are_exact_remaps() {
        let image = gradient(5, 3);
        for orientation in 1..=8u8 {
            let mapping =
                Mapping::new(5, 3, orientation, &json!({}), LensChain::default()).unwrap();
            let (fw, fh) = mapping.frame;
            let (out, invalid) = render(&image, &mapping, [0, 0, fw, fh]).unwrap();
            assert_eq!(invalid, 0);
            assert!(out.alpha.is_none());
            let mut seen: Vec<[u32; 2]> = out
                .rgb
                .chunks(3)
                .map(|p| [p[0] as u32, p[1] as u32])
                .collect();
            seen.sort();
            seen.dedup();
            assert_eq!(seen.len(), 15, "orientation {orientation} is a permutation");
        }
        let mapping = Mapping::new(5, 3, 6, &json!({}), LensChain::default()).unwrap();
        let (out, _) = render(&image, &mapping, [0, 0, 3, 5]).unwrap();
        // Orientation 6 (90° clockwise): the top-left output is the bottom-left source.
        assert_eq!(&out.rgb[0..2], &[0.0, 2.0]);
    }

    #[test]
    fn catmull_rom_interpolates_linear_ramps_exactly() {
        let image = gradient(8, 8);
        for (x, y) in [(3.5, 3.5), (3.75, 4.25), (2.1, 5.9)] {
            let v = bicubic(&image, 8, 8, [x, y], 0);
            assert!((v - (x - 0.5)).abs() < 1e-9, "{x} {y} {v}");
        }
        let w = weights(0.3);
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-15);
    }

    #[test]
    fn rotation_invalidates_corners_and_constrain_removes_them() {
        let mapping =
            Mapping::new(400, 300, 1, &json!({"rotate": 10}), LensChain::default()).unwrap();
        assert!(!mapping.valid(0.5, 0.5));
        assert!(mapping.valid(200.0, 150.0));
        let invalid = count_invalid(&mapping, [0, 0, 400, 300]).unwrap();
        assert!(invalid > 0);
        let (crop, report) =
            resolve_crop(&json!({"constrain": true, "aspect": "original"}), &mapping).unwrap();
        assert_eq!(report["constrained"], true);
        assert_eq!(count_invalid(&mapping, crop).unwrap(), 0);
        let (w, h) = ((crop[2] - crop[0]) as f64, (crop[3] - crop[1]) as f64);
        assert!((w / h - 4.0 / 3.0).abs() < 0.02, "{crop:?}");
        assert!(w > 250.0, "{crop:?}");
    }

    #[test]
    fn manual_distortion_and_vignetting_follow_their_signs() {
        let lens = LensChain::new(&json!({"distortion": 40}), 100, 100, Correction::None);
        // Positive distortion reads corners from closer to the center (barrel removal).
        let (p, _) = lens.invert([0.0, 0.0], 1);
        assert!(p[0] > 0.0 && p[1] > 0.0, "{p:?}");
        let lens = LensChain::new(
            &json!({"vignetting": {"amount": 100}}),
            100,
            100,
            Correction::None,
        );
        let (_, corner) = lens.invert([0.0, 0.0], 1);
        let (_, middle) = lens.invert([50.0, 50.0], 1);
        assert!((corner - 2.0).abs() < 1e-12 && middle == 1.0);
        let lens = LensChain::new(
            &json!({"chromatic_aberration": {"red_cyan": 100}}),
            100,
            100,
            Correction::None,
        );
        let (red, _) = lens.invert([0.0, 0.0], 0);
        let (green, _) = lens.invert([0.0, 0.0], 1);
        assert!(red[0] < green[0]);
    }

    #[test]
    fn upright_levels_a_tilted_line_set() {
        // Synthetic segments tilted by 3°: level should rotate by -3°... back to level.
        let (s, c) = math::sin_cos(math::radians(3.0));
        let segments: Vec<Segment> = [-0.5, 0.0, 0.5]
            .iter()
            .map(|&x| Segment {
                a: [x - 0.5 * s, -0.5 * c],
                b: [x + 0.5 * s, 0.5 * c],
                vertical: true,
            })
            .collect();
        let solved = solve(&segments, &["rotate"], &json!({}), (300, 200));
        let rotate = solved["rotate"].as_f64().unwrap();
        assert!(
            (rotate - 3.0).abs() < 0.02 || (rotate + 3.0).abs() < 0.02,
            "{solved}"
        );
        assert!(cost(&segments, &solved, (300, 200)) < 1e-3);
    }

    #[test]
    fn hough_finds_lines_in_an_image() {
        let (w, h) = (200, 160);
        let mut lum = vec![0.2; w * h];
        for y in 0..h {
            for x in 0..w {
                // A bright vertical bar tilted by about 2°.
                let edge = 100.0 + (y as f64 - 80.0) * 0.035;
                if (x as f64) > edge {
                    lum[y * w + x] = 0.8;
                }
            }
        }
        let segments = detect_lines(&lum, w, h, (w, h));
        assert!(segments.iter().any(|s| s.vertical), "{segments:?}");
        let solved = solve(&segments, &["rotate"], &json!({}), (w, h));
        let rotate = solved["rotate"].as_f64().unwrap();
        assert!(rotate.abs() > 1.0 && rotate.abs() < 3.0, "{solved}");
    }
}

//! ICC legacy lut8Type/lut16Type execution in normalized table coordinates.
//! PCS encoding and intent selection are deliberately handled by the caller.
use super::{fixed, range, rgb, signature, u16_at, Profile};
use anyhow::{bail, ensure, Result};

mod modern;
use modern::ModernTransform;

pub const MAX_GRID_POINTS: usize = 65;
pub const MAX_CLUT_SAMPLES: usize = 4_194_304;

/// Fixed-capacity sample avoids per-pixel allocations. Only `channels()` is
/// meaningful; unused elements are zero. `clipped` reports matrix clipping.
#[derive(Debug, Clone, Copy)]
pub struct LutSample {
    values: [f64; 4],
    count: usize,
    pub clipped: bool,
}

impl LutSample {
    pub fn channels(&self) -> &[f64] {
        &self.values[..self.count]
    }
}

#[derive(Debug)]
struct LegacyTransform<'a> {
    inputs: usize,
    outputs: usize,
    grid: usize,
    precision: usize,
    matrix: rgb::Matrix,
    input_tables: Vec<Vec<f64>>,
    output_tables: Vec<Vec<f64>>,
    clut: &'a [u8],
    forward: bool,
    pcs: Option<PcsEncoding>,
}

#[derive(Debug, Clone, Copy)]
enum PcsEncoding {
    Xyz16,
    Lab8,
    Lab16Legacy,
}

impl PcsEncoding {
    fn decode(self, encoded: [f64; 3]) -> Result<[f64; 3]> {
        match self {
            Self::Xyz16 => Ok(encoded.map(|v| v * (65535.0 / 32768.0))),
            Self::Lab8 | Self::Lab16Legacy => {
                let scale = if matches!(self, Self::Lab16Legacy) {
                    65535.0 / 65280.0
                } else {
                    1.0
                };
                let lab = [
                    encoded[0] * scale * 100.0,
                    encoded[1] * scale * 255.0 - 128.0,
                    encoded[2] * scale * 255.0 - 128.0,
                ];
                ensure!(
                    lab[0] <= 100.0 + 1e-10,
                    "[malformed-resource] ICC LUT emits Lab lightness above 100"
                );
                crate::color::pcs::lab_to_xyz(lab)
            }
        }
    }

    fn encode(self, xyz: [f64; 3]) -> Result<([f64; 3], bool)> {
        ensure!(
            xyz.iter().all(|v| v.is_finite()),
            "[invalid-input] ICC PCS values must be finite"
        );
        let (values, maximum) = match self {
            Self::Xyz16 => (xyz.map(|v| v * (32768.0 / 65535.0)), [1.0; 3]),
            Self::Lab8 | Self::Lab16Legacy => {
                let lab = crate::color::pcs::xyz_to_lab(xyz)?;
                let scale = if matches!(self, Self::Lab16Legacy) {
                    65280.0 / 65535.0
                } else {
                    1.0
                };
                (
                    [
                        lab[0] / 100.0 * scale,
                        (lab[1] + 128.0) / 255.0 * scale,
                        (lab[2] + 128.0) / 255.0 * scale,
                    ],
                    [scale, 1.0, 1.0],
                )
            }
        };
        let clipped = values
            .iter()
            .enumerate()
            .any(|(i, v)| !(0.0..=maximum[i]).contains(v));
        Ok((
            std::array::from_fn(|i| values[i].clamp(0.0, maximum[i])),
            clipped,
        ))
    }
}

fn channels(space: &[u8]) -> Result<usize> {
    match space {
        b"GRAY" => Ok(1),
        b"RGB " | b"XYZ " | b"Lab " => Ok(3),
        b"CMYK" => Ok(4),
        _ => bail!(
            "[unsupported-capability] ICC LUT channel space {}",
            signature(space)
        ),
    }
}

fn scalar(bytes: &[u8], index: usize, precision: usize) -> f64 {
    if precision == 1 {
        f64::from(bytes[index]) / 255.0
    } else {
        f64::from(u16::from_be_bytes([bytes[index * 2], bytes[index * 2 + 1]])) / 65535.0
    }
}

/// LittleCMS convention: tetrahedral for 3D, linear between tetrahedral slices
/// for 4D, and linear/bilinear for 1D/2D. Fixed capacity, no pixel allocations.
fn evaluate_clut(
    bytes: &[u8],
    grid: &[usize],
    outputs: usize,
    precision: usize,
    input: &[f64; 4],
) -> [f64; 4] {
    let mut lo = [0usize; 4];
    let mut fraction = [0.0; 4];
    for axis in 0..grid.len() {
        let value = input[axis].clamp(0.0, 1.0) * (grid[axis] - 1) as f64;
        lo[axis] = (value.floor() as usize).min(grid[axis] - 2);
        fraction[axis] = value - lo[axis] as f64;
    }
    let mut output = [0.0; 4];
    let mut add = |corner: &[usize; 4], weight: f64| {
        let mut index = 0;
        for axis in 0..grid.len() {
            index = index * grid[axis] + lo[axis] + corner[axis];
        }
        for (channel, value) in output.iter_mut().enumerate().take(outputs) {
            *value += weight * scalar(bytes, index * outputs + channel, precision);
        }
    };
    if grid.len() < 3 {
        for bits in 0..1usize << grid.len() {
            let mut corner = [0; 4];
            let mut weight = 1.0;
            for axis in 0..grid.len() {
                corner[axis] = (bits >> axis) & 1;
                weight *= if corner[axis] == 1 {
                    fraction[axis]
                } else {
                    1.0 - fraction[axis]
                };
            }
            add(&corner, weight);
        }
    } else {
        let start = grid.len() - 3;
        let mut axes = [start, start + 1, start + 2];
        axes.sort_by(|a, b| fraction[*b].total_cmp(&fraction[*a]));
        let f = axes.map(|i| fraction[i]);
        let weights = [1.0 - f[0], f[0] - f[1], f[1] - f[2], f[2]];
        for plane in 0..if grid.len() == 4 { 2 } else { 1 } {
            let scale = if grid.len() == 4 {
                if plane == 0 {
                    1.0 - fraction[0]
                } else {
                    fraction[0]
                }
            } else {
                1.0
            };
            let mut corner = [0; 4];
            if grid.len() == 4 {
                corner[0] = plane;
            }
            add(&corner, weights[0] * scale);
            for i in 0..3 {
                corner[axes[i]] = 1;
                add(&corner, weights[i + 1] * scale);
            }
        }
    }
    output
}

fn tables(bytes: &[u8], count: usize, entries: usize, precision: usize) -> Vec<Vec<f64>> {
    (0..count)
        .map(|channel| {
            (0..entries)
                .map(|entry| scalar(bytes, channel * entries + entry, precision))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod interpolation_tests {
    use super::evaluate_clut;

    #[test]
    fn tetrahedra_cover_every_fraction_order_and_shared_faces() {
        // Only vertex 111 is nonzero: tetrahedral interpolation yields the
        // smallest coordinate, whereas trilinear gives their product.
        let bytes = [0, 0, 0, 0, 0, 0, 0, 255];
        for xyz in [
            [0.2, 0.4, 0.7],
            [0.2, 0.7, 0.4],
            [0.4, 0.2, 0.7],
            [0.4, 0.7, 0.2],
            [0.7, 0.2, 0.4],
            [0.7, 0.4, 0.2],
            [0.4, 0.4, 0.7],
            [0.7, 0.4, 0.4],
            [0.4, 0.7, 0.4],
            [0.4, 0.4, 0.4],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ] {
            let expected = xyz.into_iter().fold(1.0_f64, f64::min);
            let actual = evaluate_clut(&bytes, &[2; 3], 1, 1, &[xyz[0], xyz[1], xyz[2], 0.0]);
            assert!((actual[0] - expected).abs() < 1e-14, "{xyz:?}");
        }
    }

    #[test]
    fn fourth_dimension_interpolates_first_axis_planes() {
        let mut bytes = [0; 16];
        bytes[15] = 255;
        for first in [0.0, 0.25, 0.5, 1.0] {
            let actual = evaluate_clut(&bytes, &[2; 4], 1, 1, &[first, 0.7, 0.2, 0.4]);
            assert!((actual[0] - first * 0.2).abs() < 1e-14);
        }
    }
}

fn interpolate(table: &[f64], value: f64) -> f64 {
    let position = value.clamp(0.0, 1.0) * (table.len() - 1) as f64;
    let lo = position.floor() as usize;
    let hi = (lo + 1).min(table.len() - 1);
    table[lo] + (table[hi] - table[lo]) * (position - lo as f64)
}

/// Compiled legacy or ICC v4 staged LUT. The public channel and PCS APIs are
/// shared so intent selection cannot accidentally bypass a supported tag form.
#[derive(Debug)]
pub struct LutTransform<'a> {
    engine: LutEngine<'a>,
}

#[derive(Debug)]
enum LutEngine<'a> {
    Legacy(LegacyTransform<'a>),
    Modern(ModernTransform<'a>),
}

impl<'a> LutTransform<'a> {
    pub fn compile(profile: &Profile<'a>, tag: &[u8; 4]) -> Result<Self> {
        let body = profile.required(tag)?;
        let engine = match range(body, 0, 4)? {
            b"mAB " | b"mBA " => LutEngine::Modern(ModernTransform::compile(profile, tag)?),
            _ => LutEngine::Legacy(LegacyTransform::compile(profile, tag)?),
        };
        Ok(Self { engine })
    }

    pub fn input_channels(&self) -> usize {
        match &self.engine {
            LutEngine::Legacy(e) => e.input_channels(),
            LutEngine::Modern(e) => e.inputs,
        }
    }
    pub fn output_channels(&self) -> usize {
        match &self.engine {
            LutEngine::Legacy(e) => e.output_channels(),
            LutEngine::Modern(e) => e.outputs,
        }
    }
    pub fn evaluate(&self, input: &[f64]) -> Result<LutSample> {
        match &self.engine {
            LutEngine::Legacy(e) => e.evaluate(input),
            LutEngine::Modern(e) => e.evaluate(input),
        }
    }
    pub fn to_xyz(&self, input: &[f64]) -> Result<([f64; 3], bool)> {
        match &self.engine {
            LutEngine::Legacy(e) => e.to_xyz(input),
            LutEngine::Modern(e) => e.to_xyz(input),
        }
    }
    pub fn from_xyz(&self, pcs: [f64; 3]) -> Result<LutSample> {
        match &self.engine {
            LutEngine::Legacy(e) => e.evaluate_xyz(pcs),
            LutEngine::Modern(e) => e.evaluate_xyz(pcs),
        }
    }
}

impl<'a> LegacyTransform<'a> {
    /// Compile one explicit A2B/B2A tag, without fallback or inverse guessing.
    /// The profile owns the borrowed CLUT bytes, which are never expanded.
    pub fn compile(profile: &Profile<'a>, tag: &[u8; 4]) -> Result<Self> {
        let class = &profile.bytes[12..16];
        ensure!(
            matches!(class, b"scnr" | b"mntr" | b"prtr" | b"spac" | b"link"),
            "[unsupported-capability] ICC LUT class {}",
            signature(class)
        );
        let forward = matches!(tag, b"A2B0" | b"A2B1" | b"A2B2");
        let reverse = matches!(tag, b"B2A0" | b"B2A1" | b"B2A2");
        ensure!(
            forward || reverse,
            "[invalid-input] ICC LUT tag must be an explicit A2B0..2 or B2A0..2"
        );
        if class == b"link" {
            ensure!(
                tag == b"A2B0",
                "[unsupported-capability] ICC device link requires its A2B0 path"
            );
        }
        let (input_space, output_space) = if forward {
            (&profile.bytes[16..20], &profile.bytes[20..24])
        } else {
            (&profile.bytes[20..24], &profile.bytes[16..20])
        };
        let body = profile.required(tag)?;
        let (precision, header, input_entries, output_entries) = match range(body, 0, 4)? {
            b"mft1" => (1, 48, 256, 256),
            b"mft2" => {
                range(body, 0, 52)?;
                (
                    2,
                    52,
                    usize::from(u16_at(body, 48)?),
                    usize::from(u16_at(body, 50)?),
                )
            }
            kind => bail!(
                "[unsupported-capability] ICC {} element {}; legacy LUT executor supports mft1/mft2",
                signature(tag),
                signature(kind)
            ),
        };
        ensure!(
            precision != 1 || (input_space != b"XYZ " && output_space != b"XYZ "),
            "[unsupported-capability] ICC mft1 has no defined 8-bit PCS XYZ encoding; use a 16-bit LUT"
        );
        range(body, 0, header)?;
        ensure!(
            body[4..8] == [0; 4] && body[11] == 0,
            "[malformed-resource] ICC LUT reserved bytes are nonzero"
        );
        let inputs = usize::from(body[8]);
        let outputs = usize::from(body[9]);
        ensure!(
            inputs == channels(input_space)? && outputs == channels(output_space)?,
            "[malformed-resource] ICC {} LUT channels {inputs}->{outputs} disagree with profile spaces",
            signature(tag)
        );
        let grid = usize::from(body[10]);
        ensure!(
            grid >= 2,
            "[malformed-resource] ICC LUT grid needs at least two points"
        );
        ensure!(
            grid <= MAX_GRID_POINTS,
            "[limit-exceeded] ICC LUT grid {grid} exceeds {MAX_GRID_POINTS}"
        );
        ensure!(
            (2..=4096).contains(&input_entries) && (2..=4096).contains(&output_entries),
            "[malformed-resource] ICC LUT table lengths must be 2..4096"
        );
        let matrix = [
            [fixed(body, 12)?, fixed(body, 16)?, fixed(body, 20)?],
            [fixed(body, 24)?, fixed(body, 28)?, fixed(body, 32)?],
            [fixed(body, 36)?, fixed(body, 40)?, fixed(body, 44)?],
        ];
        ensure!(
            input_space == b"XYZ " || matrix == rgb::identity(),
            "[malformed-resource] ICC LUT matrix must be identity unless input is PCS XYZ"
        );
        let mut samples = outputs;
        for _ in 0..inputs {
            samples = samples
                .checked_mul(grid)
                .filter(|v| *v <= MAX_CLUT_SAMPLES)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "[limit-exceeded] ICC CLUT exceeds {MAX_CLUT_SAMPLES} scalar samples"
                    )
                })?;
        }
        let input_size = inputs * input_entries * precision;
        let clut_size = samples * precision;
        let output_size = outputs * output_entries * precision;
        let expected = header + input_size + clut_size + output_size;
        ensure!(
            body.len() == expected,
            "[malformed-resource] ICC {} LUT payload has {} bytes; expected {expected}",
            signature(tag),
            body.len()
        );
        // Every count and range is now bounded, before table allocation.
        let input_tables = tables(
            &body[header..header + input_size],
            inputs,
            input_entries,
            precision,
        );
        let clut = &body[header + input_size..header + input_size + clut_size];
        let output_tables = tables(
            &body[header + input_size + clut_size..],
            outputs,
            output_entries,
            precision,
        );
        let pcs = if class == b"link" {
            None
        } else {
            Some(match (&profile.bytes[20..24], precision) {
                (b"XYZ ", 2) => PcsEncoding::Xyz16,
                (b"Lab ", 1) => PcsEncoding::Lab8,
                (b"Lab ", 2) => PcsEncoding::Lab16Legacy,
                _ => bail!("[unsupported-capability] ICC LUT PCS encoding"),
            })
        };
        Ok(Self {
            inputs,
            outputs,
            grid,
            precision,
            matrix,
            input_tables,
            output_tables,
            clut,
            forward,
            pcs,
        })
    }

    pub fn input_channels(&self) -> usize {
        self.inputs
    }
    pub fn output_channels(&self) -> usize {
        self.outputs
    }

    /// Device channels to physical D50 XYZ through the compiled forward path.
    pub fn to_xyz(&self, device: &[f64]) -> Result<([f64; 3], bool)> {
        ensure!(
            self.forward,
            "[invalid-input] ICC to_xyz requires an A2B path"
        );
        let pcs = self.pcs.ok_or_else(|| {
            anyhow::anyhow!("[unsupported-capability] device links do not have a PCS")
        })?;
        let result = self.evaluate(device)?;
        Ok((
            pcs.decode([result.values[0], result.values[1], result.values[2]])?,
            result.clipped,
        ))
    }

    /// Physical D50 XYZ to device channels; out-of-range PCS is reported.
    pub fn evaluate_xyz(&self, xyz: [f64; 3]) -> Result<LutSample> {
        ensure!(
            !self.forward,
            "[invalid-input] ICC from_xyz requires a B2A path"
        );
        let pcs = self.pcs.ok_or_else(|| {
            anyhow::anyhow!("[unsupported-capability] device links do not have a PCS")
        })?;
        let (encoded, clipped) = pcs.encode(xyz)?;
        let mut result = self.evaluate(&encoded)?;
        result.clipped |= clipped;
        Ok(result)
    }

    /// Evaluate normalized LUT coordinates, including ICC table/matrix ordering.
    /// This method does not label normalized PCS coordinates as physical XYZ/Lab.
    pub fn evaluate(&self, input: &[f64]) -> Result<LutSample> {
        ensure!(
            input.len() == self.inputs
                && input
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "[invalid-input] ICC LUT requires {} finite normalized input channels",
            self.inputs
        );
        let mut work = [0.0; 4];
        work[..self.inputs].copy_from_slice(input);
        let mut clipped = false;
        if self.inputs == 3 {
            let transformed = rgb::apply(&self.matrix, [work[0], work[1], work[2]]);
            for (i, value) in transformed.iter().enumerate() {
                clipped |= !(0.0..=1.0).contains(value);
                work[i] = value.clamp(0.0, 1.0);
            }
        }
        for (axis, value) in work.iter_mut().enumerate().take(self.inputs) {
            *value = interpolate(&self.input_tables[axis], *value);
        }
        let mut output = evaluate_clut(
            self.clut,
            &[self.grid; 4][..self.inputs],
            self.outputs,
            self.precision,
            &work,
        );
        for (channel, value) in output.iter_mut().enumerate().take(self.outputs) {
            *value = interpolate(&self.output_tables[channel], *value);
        }
        Ok(LutSample {
            values: output,
            count: self.outputs,
            clipped,
        })
    }
}

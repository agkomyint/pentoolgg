//! Bounded ICC v4 lutAToBType/lutBToAType, with borrowed variable-axis CLUTs.
use super::super::{fixed, range, rgb, signature, u16_at, u32_at, Curve, MAX_CURVE_SAMPLES};
use super::{
    channels, evaluate_clut, LutSample, PcsEncoding, Profile, MAX_CLUT_SAMPLES, MAX_GRID_POINTS,
};
use anyhow::{bail, ensure, Result};

#[derive(Debug)]
pub(super) struct ModernTransform<'a> {
    pub inputs: usize,
    pub outputs: usize,
    forward: bool,
    pcs: Option<PcsEncoding>,
    a: Vec<Curve>,
    b: Vec<Curve>,
    m: Vec<Curve>,
    matrix: Option<(rgb::Matrix, [f64; 3])>,
    clut: Option<Clut<'a>>,
}

#[derive(Debug)]
struct Clut<'a> {
    bytes: &'a [u8],
    grid: Vec<usize>,
    outputs: usize,
    precision: usize,
}

type Span = (usize, usize, u8);

fn curves(body: &[u8], offset: usize, count: usize, spans: &mut Vec<Span>) -> Result<Vec<Curve>> {
    if offset == 0 {
        return Ok(Vec::new());
    }
    let mut at = offset;
    let mut result = Vec::with_capacity(count);
    for channel in 0..count {
        let header = range(body, at, 12)?;
        ensure!(
            header[4..8] == [0; 4],
            "[malformed-resource] ICC embedded curve at {at} has nonzero reserved bytes"
        );
        let length = match &header[..4] {
            b"curv" => {
                let n = u32_at(header, 8)? as usize;
                ensure!(
                    n <= MAX_CURVE_SAMPLES,
                    "[limit-exceeded] ICC embedded curve exceeds {MAX_CURVE_SAMPLES} samples"
                );
                12 + 2 * n
            }
            b"para" => {
                12 + 4 * match u16_at(header, 8)? {
                    0 => 1,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    4 => 7,
                    kind => bail!("[unsupported-capability] ICC embedded curve function {kind}"),
                }
            }
            kind => bail!(
                "[unsupported-capability] ICC embedded curve {} at {at}",
                signature(kind)
            ),
        };
        let payload = range(body, at, length)?;
        result.push(
            Curve::parse_forward(payload)
                .map_err(|e| anyhow::anyhow!("ICC embedded curve {channel} at {at}: {e}"))?,
        );
        let end = at + length; // range() checked the sum above.
        spans.push((at, end, 0));
        let aligned = (end + 3) & !3;
        ensure!(
            body[end..aligned.min(body.len())].iter().all(|v| *v == 0),
            "[malformed-resource] ICC curve padding at {end} is not zero"
        );
        at = aligned;
    }
    Ok(result)
}

impl<'a> Clut<'a> {
    fn parse(
        body: &'a [u8],
        offset: usize,
        inputs: usize,
        outputs: usize,
        spans: &mut Vec<Span>,
    ) -> Result<Self> {
        let header = range(body, offset, 20)?;
        ensure!(
            header[inputs..16].iter().all(|v| *v == 0) && header[17..20] == [0; 3],
            "[malformed-resource] ICC CLUT unused dimensions/reserved bytes are nonzero"
        );
        let precision = usize::from(header[16]);
        ensure!(
            matches!(precision, 1 | 2),
            "[unsupported-capability] ICC CLUT precision must be one or two bytes"
        );
        let grid: Vec<usize> = header[..inputs].iter().map(|v| usize::from(*v)).collect();
        let mut samples = outputs;
        for n in &grid {
            ensure!(
                *n >= 2,
                "[malformed-resource] ICC CLUT grid axis needs at least two points"
            );
            ensure!(
                *n <= MAX_GRID_POINTS,
                "[limit-exceeded] ICC CLUT axis exceeds {MAX_GRID_POINTS} points"
            );
            samples = samples
                .checked_mul(*n)
                .filter(|v| *v <= MAX_CLUT_SAMPLES)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "[limit-exceeded] ICC CLUT exceeds {MAX_CLUT_SAMPLES} scalar samples"
                    )
                })?;
        }
        let bytes = range(body, offset + 20, samples * precision)?;
        spans.push((offset, offset + 20 + bytes.len(), 2));
        Ok(Self {
            bytes,
            grid,
            outputs,
            precision,
        })
    }

    fn evaluate(&self, work: &[f64; 4]) -> [f64; 4] {
        evaluate_clut(self.bytes, &self.grid, self.outputs, self.precision, work)
    }
}

impl<'a> ModernTransform<'a> {
    pub fn compile(profile: &Profile<'a>, tag: &[u8; 4]) -> Result<Self> {
        ensure!(
            profile.bytes[8] == 4,
            "[unsupported-capability] ICC mAB/mBA requires profile version 4"
        );
        let class = &profile.bytes[12..16];
        ensure!(
            matches!(class, b"scnr" | b"mntr" | b"prtr" | b"spac" | b"link"),
            "[unsupported-capability] ICC staged LUT class {}",
            signature(class)
        );
        let forward = matches!(tag, b"A2B0" | b"A2B1" | b"A2B2");
        ensure!(
            forward || matches!(tag, b"B2A0" | b"B2A1" | b"B2A2"),
            "[invalid-input] ICC staged LUT requires an explicit A2B/B2A tag"
        );
        ensure!(
            class != b"link" || tag == b"A2B0",
            "[unsupported-capability] ICC device link requires A2B0"
        );
        let body = profile.required(tag)?;
        range(body, 0, 32)?;
        ensure!(
            &body[..4] == if forward { b"mAB " } else { b"mBA " },
            "[malformed-resource] ICC staged LUT type disagrees with A2B/B2A direction"
        );
        ensure!(
            body[4..8] == [0; 4] && body[10..12] == [0; 2],
            "[malformed-resource] ICC staged LUT reserved bytes are nonzero"
        );
        let inputs = usize::from(body[8]);
        let outputs = usize::from(body[9]);
        let (input_space, output_space) = if forward {
            (&profile.bytes[16..20], &profile.bytes[20..24])
        } else {
            (&profile.bytes[20..24], &profile.bytes[16..20])
        };
        ensure!(
            inputs == channels(input_space)? && outputs == channels(output_space)?,
            "[malformed-resource] ICC staged LUT channels disagree with profile spaces"
        );
        let offsets: Vec<usize> = (0..5)
            .map(|i| u32_at(body, 12 + 4 * i).map(|v| v as usize))
            .collect::<Result<_>>()?;
        for offset in &offsets {
            ensure!(
                *offset == 0 || (*offset >= 32 && *offset % 4 == 0 && *offset < body.len()),
                "[malformed-resource] ICC staged LUT element offset {offset} is outside data or unaligned"
            );
        }
        let [b_at, matrix_at, m_at, clut_at, a_at]: [usize; 5] =
            offsets.try_into().expect("five offsets");
        ensure!(
            b_at != 0 && (matrix_at == 0) == (m_at == 0) && (clut_at == 0) == (a_at == 0),
            "[malformed-resource] ICC staged LUT requires B, paired matrix/M, and paired CLUT/A elements"
        );
        ensure!(
            clut_at != 0 || inputs == outputs,
            "[malformed-resource] ICC staged LUT without CLUT cannot change channel count"
        );
        let side = if forward { outputs } else { inputs };
        ensure!(
            matrix_at == 0 || side == 3,
            "[malformed-resource] ICC staged LUT matrix requires three channels"
        );
        let mut spans = Vec::new();
        let b = curves(body, b_at, side, &mut spans)?;
        let a = curves(
            body,
            a_at,
            if forward { inputs } else { outputs },
            &mut spans,
        )?;
        let m = curves(body, m_at, side, &mut spans)?;
        let matrix = if matrix_at == 0 {
            None
        } else {
            let data = range(body, matrix_at, 48)?;
            let mut matrix = [[0.0; 3]; 3];
            for (row, values) in matrix.iter_mut().enumerate() {
                for (col, value) in values.iter_mut().enumerate() {
                    *value = fixed(data, 4 * (row * 3 + col))?;
                }
            }
            spans.push((matrix_at, matrix_at + 48, 1));
            Some((
                matrix,
                [fixed(data, 36)?, fixed(data, 40)?, fixed(data, 44)?],
            ))
        };
        let clut = if clut_at == 0 {
            None
        } else {
            Some(Clut::parse(body, clut_at, inputs, outputs, &mut spans)?)
        };
        spans.sort_unstable();
        for pair in spans.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            ensure!(
                b.0 >= a.1 || (a == b && a.2 == 0),
                "[malformed-resource] ICC staged LUT elements overlap at {}",
                b.0
            );
        }
        let pcs = if class == b"link" {
            None
        } else {
            Some(if &profile.bytes[20..24] == b"XYZ " {
                PcsEncoding::Xyz16
            } else {
                PcsEncoding::Lab8
            })
        };
        Ok(Self {
            inputs,
            outputs,
            forward,
            pcs,
            a,
            b,
            m,
            matrix,
            clut,
        })
    }

    pub fn evaluate(&self, input: &[f64]) -> Result<LutSample> {
        ensure!(
            input.len() == self.inputs
                && input
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "[invalid-input] ICC staged LUT requires {} finite normalized channels",
            self.inputs
        );
        let mut work = [0.0; 4];
        work[..self.inputs].copy_from_slice(input);
        let mut clipped = false;
        let curves = |work: &mut [f64; 4], curves: &[Curve]| {
            for (i, curve) in curves.iter().enumerate() {
                work[i] = curve.eval(work[i]);
            }
        };
        let matrix = |work: &mut [f64; 4], clipped: &mut bool| {
            if let Some((matrix, offset)) = &self.matrix {
                let out = rgb::apply(matrix, [work[0], work[1], work[2]]);
                for i in 0..3 {
                    let value = out[i] + offset[i];
                    *clipped |= !(0.0..=1.0).contains(&value);
                    work[i] = value.clamp(0.0, 1.0);
                }
            }
        };
        if self.forward {
            curves(&mut work, &self.a);
            if let Some(clut) = &self.clut {
                work = clut.evaluate(&work);
            }
            curves(&mut work, &self.m);
            matrix(&mut work, &mut clipped);
            curves(&mut work, &self.b);
        } else {
            curves(&mut work, &self.b);
            matrix(&mut work, &mut clipped);
            curves(&mut work, &self.m);
            if let Some(clut) = &self.clut {
                work = clut.evaluate(&work);
            }
            curves(&mut work, &self.a);
        }
        Ok(LutSample {
            values: work,
            count: self.outputs,
            clipped,
        })
    }

    pub fn to_xyz(&self, input: &[f64]) -> Result<([f64; 3], bool)> {
        ensure!(
            self.forward,
            "[invalid-input] ICC to_xyz requires an A2B path"
        );
        let pcs = self.pcs.ok_or_else(|| {
            anyhow::anyhow!("[unsupported-capability] device links do not have a PCS")
        })?;
        let result = self.evaluate(input)?;
        Ok((
            pcs.decode([result.values[0], result.values[1], result.values[2]])?,
            result.clipped,
        ))
    }

    pub fn evaluate_xyz(&self, xyz: [f64; 3]) -> Result<LutSample> {
        ensure!(
            !self.forward,
            "[invalid-input] ICC from_xyz requires a B2A path"
        );
        let pcs = self.pcs.ok_or_else(|| {
            anyhow::anyhow!("[unsupported-capability] device links do not have a PCS")
        })?;
        let (encoded, clipped) = pcs.encode(xyz)?;
        let mut sample = self.evaluate(&encoded)?;
        sample.clipped |= clipped;
        Ok(sample)
    }
}

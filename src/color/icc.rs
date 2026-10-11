//! Bounded, borrowing ICC.1 reader and RGB matrix/TRC execution.
//!
//! Structural readability and executable transform support are separate: a LUT
//! profile can be inspected without claiming that the matrix executor supports it.
use crate::photo::{color as rgb, math};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

mod lut;
pub use lut::{LutSample, LutTransform};
mod metadata;
mod transform;
pub use metadata::{Metadata, ProfileText};
mod link;
pub use link::{BlackMapping, Link, LinkDescription, LinkSample};
pub use transform::{
    DeviceLinkTransform, Direction, Intent, Transform, TransformDescription, TransformSample,
};

pub const MAX_PROFILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TAGS: usize = 256;
pub const MAX_CURVE_SAMPLES: usize = 65_536;

/// Compact metadata; never includes original profile or tag payloads.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub sha256: String,
    pub byte_length: usize,
    #[serde(rename = "icc_version")]
    pub version: String,
    pub class: String,
    pub color_space: String,
    pub connection_space: String,
    pub rendering_intent: u32,
    pub tags: Vec<TagSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagSummary {
    pub signature: String,
    pub kind: String,
    pub offset: usize,
    pub byte_length: usize,
}

#[derive(Debug)]
pub struct Profile<'a> {
    bytes: &'a [u8],
    tags: BTreeMap<[u8; 4], (usize, usize)>,
}

fn range(bytes: &[u8], offset: usize, count: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(count)
        .context("[malformed-resource] ICC byte range overflow")?;
    bytes
        .get(offset..end)
        .with_context(|| format!("[malformed-resource] ICC truncated byte range {offset}..{end}"))
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(range(bytes, offset, 2)?.try_into()?))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(range(bytes, offset, 4)?.try_into()?))
}

fn fixed(bytes: &[u8], offset: usize) -> Result<f64> {
    Ok(f64::from(i32::from_be_bytes(range(bytes, offset, 4)?.try_into()?)) / 65536.0)
}

fn signature(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl<'a> Profile<'a> {
    /// Validate container structure without copying the profile or trusting any
    /// data offset. Original bytes remain the authoritative hash identity.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_PROFILE_BYTES,
            "[limit-exceeded] ICC profile exceeds {MAX_PROFILE_BYTES} bytes"
        );
        ensure!(
            bytes.len() >= 132,
            "[malformed-resource] ICC header/tag table is truncated"
        );
        ensure!(
            u32_at(bytes, 0)? as usize == bytes.len(),
            "[malformed-resource] ICC declared size differs from actual byte length"
        );
        ensure!(
            &bytes[36..40] == b"acsp",
            "[malformed-resource] ICC signature at byte 36 is not acsp"
        );
        ensure!(
            matches!(bytes[8], 2 | 4),
            "[unsupported-capability] ICC major version {}; support requires ICC.1 v2 or v4",
            bytes[8]
        );
        ensure!(
            bytes[10..12] == [0, 0] && bytes[9] >> 4 <= 9 && bytes[9] & 15 <= 9,
            "[malformed-resource] invalid ICC version encoding"
        );
        ensure!(
            bytes[100..128].iter().all(|b| *b == 0),
            "[malformed-resource] ICC reserved header bytes are nonzero"
        );
        ensure!(
            u32_at(bytes, 64)? <= 3,
            "[malformed-resource] ICC rendering intent must be 0..3"
        );
        let class = &bytes[12..16];
        ensure!(
            [b"scnr", b"mntr", b"prtr", b"link", b"spac", b"abst", b"nmcl"]
                .iter()
                .any(|v| class == *v),
            "[unsupported-capability] ICC class {}",
            signature(class)
        );
        ensure!(
            [b"RGB ", b"GRAY", b"CMYK", b"XYZ ", b"Lab "]
                .iter()
                .any(|v| &bytes[16..20] == *v),
            "[unsupported-capability] ICC channel space {}; supported maximum is four device channels",
            signature(&bytes[16..20])
        );
        if class == b"link" {
            ensure!(
                [b"RGB ", b"GRAY", b"CMYK"]
                    .iter()
                    .any(|v| &bytes[20..24] == *v),
                "[unsupported-capability] ICC device-link output space {}",
                signature(&bytes[20..24])
            );
        } else {
            ensure!(
                matches!(&bytes[20..24], b"XYZ " | b"Lab "),
                "[unsupported-capability] ICC PCS must be XYZ or Lab"
            );
        }
        for (axis, expected) in [0x0000_f6d6u32, 0x0001_0000, 0x0000_d32d]
            .iter()
            .enumerate()
        {
            ensure!(
                u32_at(bytes, 68 + 4 * axis)? == *expected,
                "[malformed-resource] ICC header PCS illuminant must be D50"
            );
        }
        let count = u32_at(bytes, 128)? as usize;
        ensure!(
            count <= MAX_TAGS,
            "[limit-exceeded] ICC tag count {count} exceeds {MAX_TAGS}"
        );
        let table_end = 132 + count * 12; // count was bounded above.
        range(bytes, 132, count * 12)?;
        let mut tags = BTreeMap::new();
        let mut spans = Vec::with_capacity(count);
        for index in 0..count {
            let at = 132 + index * 12;
            let name: [u8; 4] = bytes[at..at + 4].try_into()?;
            let label = signature(&name);
            let offset = u32_at(bytes, at + 4)? as usize;
            let size = u32_at(bytes, at + 8)? as usize;
            ensure!(
                offset >= table_end && offset % 4 == 0,
                "[malformed-resource] ICC tag {label} at {offset} overlaps header/table or is not aligned"
            );
            ensure!(
                size >= 8,
                "[malformed-resource] ICC tag {label} at {offset} lacks a type header"
            );
            let body = range(bytes, offset, size)
                .map_err(|error| anyhow::anyhow!("ICC tag {label} at {offset}: {error}"))?;
            ensure!(
                body[4..8] == [0; 4],
                "[malformed-resource] ICC tag {label} at {offset} has nonzero reserved bytes"
            );
            ensure!(
                tags.insert(name, (offset, size)).is_none(),
                "[malformed-resource] duplicate ICC tag {label}"
            );
            spans.push((offset, offset + size, label));
        }
        spans.sort_by_key(|span| (span.0, span.1));
        for pair in spans.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            ensure!(
                b.0 >= a.1 || (a.0 == b.0 && a.1 == b.1),
                "[malformed-resource] ICC tags {} and {} partially overlap at {}",
                a.2,
                b.2,
                b.0
            );
        }
        Ok(Self { bytes, tags })
    }

    /// Return the ICC media white point in physical D50-relative XYZ when the
    /// profile declares a valid `wtpt` tag.
    pub fn media_white_point(&self) -> Result<[f64; 3]> {
        xyz(self.required(b"wtpt")?)
    }

    pub fn summary(&self) -> Summary {
        Summary {
            sha256: format!("sha256:{}", hex::encode(Sha256::digest(self.bytes))),
            byte_length: self.bytes.len(),
            version: format!(
                "{}.{}.{}",
                self.bytes[8],
                self.bytes[9] >> 4,
                self.bytes[9] & 15
            ),
            class: signature(&self.bytes[12..16]),
            color_space: signature(&self.bytes[16..20]),
            connection_space: signature(&self.bytes[20..24]),
            rendering_intent: u32::from_be_bytes(
                self.bytes[64..68].try_into().expect("validated header"),
            ),
            tags: self
                .tags
                .iter()
                .map(|(name, (offset, size))| TagSummary {
                    signature: signature(name),
                    kind: signature(&self.bytes[*offset..*offset + 4]),
                    offset: *offset,
                    byte_length: *size,
                })
                .collect(),
        }
    }

    pub fn tag(&self, name: &[u8; 4]) -> Option<&'a [u8]> {
        self.tags
            .get(name)
            .map(|(offset, size)| &self.bytes[*offset..*offset + *size])
    }

    fn required(&self, name: &[u8; 4]) -> Result<&'a [u8]> {
        self.tag(name).with_context(|| {
            format!(
                "[malformed-resource] ICC required tag {} is missing",
                signature(name)
            )
        })
    }

    fn reject_lut_substitution(&self) -> Result<()> {
        for name in [
            b"A2B0", b"A2B1", b"A2B2", b"B2A0", b"B2A1", b"B2A2", b"D2B0", b"D2B1", b"D2B2",
            b"D2B3", b"B2D0", b"B2D1", b"B2D2", b"B2D3",
        ] {
            ensure!(
                self.tag(name).is_none(),
                "[unsupported-capability] ICC {} requires the LUT executor; TRC substitution is refused",
                signature(name)
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum Curve {
    Identity,
    Gamma(f64),
    Table(Vec<f64>),
    Parametric { kind: u16, p: Vec<f64> },
}

impl Curve {
    fn parse(bytes: &[u8]) -> Result<Self> {
        Self::parse_for(bytes, true)
    }

    fn parse_forward(bytes: &[u8]) -> Result<Self> {
        Self::parse_for(bytes, false)
    }

    fn parse_for(bytes: &[u8], invertible: bool) -> Result<Self> {
        match range(bytes, 0, 4)? {
            b"curv" => {
                let count = u32_at(bytes, 8)? as usize;
                ensure!(
                    count <= MAX_CURVE_SAMPLES,
                    "[limit-exceeded] ICC curve has {count} samples; maximum is {MAX_CURVE_SAMPLES}"
                );
                ensure!(
                    bytes.len() == 12 + 2 * count,
                    "[malformed-resource] ICC curve payload size disagrees with sample count"
                );
                match count {
                    0 => Ok(Self::Identity),
                    1 => {
                        let gamma = f64::from(u16_at(bytes, 12)?) / 256.0;
                        ensure!(
                            gamma > 0.0,
                            "[malformed-resource] ICC curve gamma must be positive"
                        );
                        Ok(Self::Gamma(gamma))
                    }
                    _ => {
                        let mut values = Vec::with_capacity(count);
                        for i in 0..count {
                            values.push(f64::from(u16_at(bytes, 12 + 2 * i)?) / 65535.0);
                        }
                        ensure!(
                            !invertible || values.windows(2).all(|p| p[0] <= p[1]),
                            "[unsupported-capability] ICC matrix curve must be monotonic for inverse execution"
                        );
                        ensure!(
                            !invertible || values[0] < values[count - 1],
                            "[unsupported-capability] ICC matrix curve is constant"
                        );
                        Ok(Self::Table(values))
                    }
                }
            }
            b"para" => {
                let kind = u16_at(bytes, 8)?;
                ensure!(
                    u16_at(bytes, 10)? == 0,
                    "[malformed-resource] ICC parametric curve reserved bytes are nonzero"
                );
                let count = match kind {
                    0 => 1,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    4 => 7,
                    _ => bail!("[unsupported-capability] ICC parametric curve type {kind}"),
                };
                ensure!(
                    bytes.len() == 12 + 4 * count,
                    "[malformed-resource] ICC parametric curve payload length"
                );
                let p = (0..count)
                    .map(|i| fixed(bytes, 12 + i * 4))
                    .collect::<Result<Vec<_>>>()?;
                ensure!(
                    p[0] > 0.0,
                    "[malformed-resource] ICC parametric gamma must be positive"
                );
                if kind != 0 {
                    ensure!(
                        p[1] > 0.0,
                        "[malformed-resource] ICC parametric scale must be positive"
                    );
                }
                if kind >= 3 {
                    ensure!(
                        (0.0..=1.0).contains(&p[4])
                            && (!invertible || p[3] >= 0.0)
                            && p[1] * p[4] + p[2] >= 0.0,
                        "[malformed-resource] ICC parametric branch is undefined or decreasing"
                    );
                    let low = p[3] * p[4] + if kind == 4 { p[6] } else { 0.0 };
                    let high =
                        math::pow(p[1] * p[4] + p[2], p[0]) + if kind == 4 { p[5] } else { 0.0 };
                    // Fixed-point coefficients can move a nominally continuous
                    // breakpoint by several code values. Reject real reversals.
                    ensure!(
                        !invertible || high + 4.0 / 65535.0 >= low,
                        "[unsupported-capability] ICC parametric curve decreases at its branch"
                    );
                }
                let curve = Self::Parametric { kind, p };
                ensure!(
                    curve.eval(0.0).is_finite()
                        && curve.eval(1.0).is_finite()
                        && (!invertible || curve.eval(0.0) < curve.eval(1.0)),
                    "[malformed-resource] ICC parametric curve is undefined or constant"
                );
                Ok(curve)
            }
            other => bail!(
                "[unsupported-capability] ICC curve element {}",
                signature(other)
            ),
        }
    }

    fn eval(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        let value = match self {
            Self::Identity => x,
            Self::Gamma(g) => math::pow(x, *g),
            Self::Table(v) => {
                let at = x * (v.len() - 1) as f64;
                let lo = at.floor() as usize;
                let hi = (lo + 1).min(v.len() - 1);
                v[lo] + (v[hi] - v[lo]) * (at - lo as f64)
            }
            Self::Parametric { kind, p } => match kind {
                0 => math::pow(x, p[0]),
                1 | 2 => {
                    let c = if *kind == 2 { p[3] } else { 0.0 };
                    if x >= -p[2] / p[1] {
                        math::pow((p[1] * x + p[2]).max(0.0), p[0]) + c
                    } else {
                        c
                    }
                }
                3 | 4 => {
                    if x >= p[4] {
                        math::pow((p[1] * x + p[2]).max(0.0), p[0])
                            + if *kind == 4 { p[5] } else { 0.0 }
                    } else {
                        p[3] * x + if *kind == 4 { p[6] } else { 0.0 }
                    }
                }
                _ => unreachable!("validated curve kind"),
            },
        };
        value.clamp(0.0, 1.0)
    }

    fn inverse(&self, value: f64) -> f64 {
        // ICC.1 Annex F.1: interior plateaus select their highest input;
        // a plateau reaching the final input selects its lowest input.
        let value = value.clamp(self.eval(0.0), self.eval(1.0));
        let terminal = value == self.eval(1.0);
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..56 {
            let mid = (lo + hi) * 0.5;
            if self.eval(mid) < value || (!terminal && self.eval(mid) == value) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if terminal || (self.eval(hi) - value).abs() < (self.eval(lo) - value).abs() {
            hi
        } else {
            lo
        }
    }
}

fn xyz(bytes: &[u8]) -> Result<[f64; 3]> {
    ensure!(
        bytes.len() == 20 && &bytes[..4] == b"XYZ ",
        "[malformed-resource] ICC matrix column must contain exactly one XYZ value"
    );
    Ok([fixed(bytes, 8)?, fixed(bytes, 12)?, fixed(bytes, 16)?])
}

/// Compiled colorimetric RGB matrix/TRC path. LUTs and intent selection belong
/// to the full transform compiler; this API deliberately exposes only XYZ PCS.
#[derive(Debug, Clone)]
pub struct MatrixTransform {
    matrix: rgb::Matrix,
    inverse: rgb::Matrix,
    curves: [Curve; 3],
}

impl MatrixTransform {
    pub fn compile(profile: &Profile<'_>) -> Result<Self> {
        ensure!(
            matches!(&profile.bytes[12..16], b"mntr" | b"scnr"),
            "[unsupported-capability] RGB matrix executor requires display or input class"
        );
        ensure!(
            &profile.bytes[16..24] == b"RGB XYZ ",
            "[unsupported-capability] RGB matrix executor requires RGB and XYZ PCS"
        );
        profile.reject_lut_substitution()?;
        let columns = [
            xyz(profile.required(b"rXYZ")?)?,
            xyz(profile.required(b"gXYZ")?)?,
            xyz(profile.required(b"bXYZ")?)?,
        ];
        let matrix = std::array::from_fn(|row| std::array::from_fn(|col| columns[col][row]));
        let det = matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
            - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
            + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]);
        ensure!(
            det.abs() > 1e-10,
            "[malformed-resource] ICC RGB colorant matrix is singular"
        );
        let inverse = rgb::invert(&matrix);
        let norm = |m: &rgb::Matrix| {
            m.iter()
                .map(|row| row.iter().map(|v| v.abs()).sum::<f64>())
                .fold(0.0, f64::max)
        };
        ensure!(
            norm(&matrix) * norm(&inverse) <= 1e8,
            "[malformed-resource] ICC RGB colorant matrix is ill-conditioned"
        );
        let curve = |name| {
            Curve::parse(profile.required(name)?)
                .map_err(|error| anyhow::anyhow!("ICC tag {}: {error}", signature(name)))
        };
        let curves = [curve(b"rTRC")?, curve(b"gTRC")?, curve(b"bTRC")?];
        // Colorant columns already describe D50 PCS. Do not apply chad again.
        Ok(Self {
            matrix,
            inverse,
            curves,
        })
    }

    pub fn to_xyz(&self, device: [f64; 3]) -> Result<[f64; 3]> {
        ensure!(
            device
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
            "[invalid-input] ICC RGB samples must be finite and in 0..1"
        );
        Ok(rgb::apply(
            &self.matrix,
            std::array::from_fn(|i| self.curves[i].eval(device[i])),
        ))
    }

    /// Returns normalized destination samples and whether PCS lies outside the
    /// matrix/TRC range. Clipping is explicit in the returned report.
    pub fn from_xyz(&self, pcs: [f64; 3]) -> Result<([f64; 3], bool)> {
        ensure!(
            pcs.iter().all(|v| v.is_finite()),
            "[invalid-input] ICC XYZ samples must be finite"
        );
        let linear = rgb::apply(&self.inverse, pcs);
        ensure!(
            linear.iter().all(|v| v.is_finite()),
            "[invalid-input] ICC XYZ samples overflow the destination matrix"
        );
        let clipped = linear
            .iter()
            .enumerate()
            .any(|(i, v)| *v < self.curves[i].eval(0.0) || *v > self.curves[i].eval(1.0));
        Ok((
            std::array::from_fn(|i| self.curves[i].inverse(linear[i])),
            clipped,
        ))
    }
}

/// Monochrome TRC path with physical XYZ PCS at the API boundary.
#[derive(Debug, Clone)]
pub struct GrayTransform {
    curve: Curve,
    lab: bool,
}

impl GrayTransform {
    pub fn compile(profile: &Profile<'_>) -> Result<Self> {
        ensure!(
            matches!(&profile.bytes[12..16], b"scnr" | b"mntr" | b"prtr")
                && &profile.bytes[16..20] == b"GRAY",
            "[unsupported-capability] gray TRC executor requires a gray input, display or output profile"
        );
        profile.reject_lut_substitution()?;
        let curve = Curve::parse(profile.required(b"kTRC")?)
            .map_err(|e| anyhow::anyhow!("ICC tag kTRC: {e}"))?;
        Ok(Self {
            curve,
            lab: &profile.bytes[20..24] == b"Lab ",
        })
    }

    pub fn to_xyz(&self, gray: f64) -> Result<[f64; 3]> {
        ensure!(
            gray.is_finite() && (0.0..=1.0).contains(&gray),
            "[invalid-input] ICC gray sample must be finite and in 0..1"
        );
        let connection = self.curve.eval(gray);
        if self.lab {
            crate::color::pcs::lab_to_xyz([connection * 100.0, 0.0, 0.0])
        } else {
            Ok(crate::color::pcs::D50.map(|v| v * connection))
        }
    }

    /// A chromatic PCS value projects onto the gray axis; clipping reports both
    /// range clipping and loss of chromatic information.
    pub fn from_xyz(&self, pcs: [f64; 3]) -> Result<(f64, bool)> {
        ensure!(
            pcs.iter().all(|v| v.is_finite()),
            "[invalid-input] ICC XYZ samples must be finite"
        );
        let connection = if self.lab {
            crate::color::pcs::xyz_to_lab(pcs)?[0] / 100.0
        } else {
            pcs[1]
        };
        let gray = self.curve.inverse(connection);
        let reproduced = self.to_xyz(gray)?;
        let clipped = reproduced
            .iter()
            .zip(pcs)
            .any(|(a, b)| (a - b).abs() > 1e-10);
        Ok((gray, clipped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(kind: u16, params: &[f64]) -> Vec<u8> {
        let mut bytes = b"para\0\0\0\0".to_vec();
        bytes.extend_from_slice(&kind.to_be_bytes());
        bytes.extend_from_slice(&[0; 2]);
        for p in params {
            bytes.extend_from_slice(&((*p * 65536.0).round() as i32).to_be_bytes());
        }
        bytes
    }

    fn table(values: &[u16]) -> Vec<u8> {
        let mut bytes = b"curv\0\0\0\0".to_vec();
        bytes.extend_from_slice(&(values.len() as u32).to_be_bytes());
        for value in values {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes
    }

    #[test]
    fn all_parametric_functions_match_explicit_equations() {
        // Binary-exact parameters isolate formula behavior from coefficient
        // quantization. These are numerical probes, not profile characterizations.
        let probes = [
            (0, vec![2.0], vec![(0.25, 0.0625), (0.5, 0.25)]),
            (1, vec![2.0, 1.0, -0.25], vec![(0.125, 0.0), (0.5, 0.0625)]),
            (
                2,
                vec![2.0, 1.0, -0.25, 0.125],
                vec![(0.125, 0.125), (0.5, 0.1875)],
            ),
            (
                3,
                vec![2.0, 1.0, 0.0, 0.5, 0.5],
                vec![(0.25, 0.125), (0.75, 0.5625)],
            ),
            (
                4,
                vec![2.0, 1.0, 0.0, 0.5, 0.5, 0.125, 0.125],
                vec![(0.25, 0.25), (0.75, 0.6875)],
            ),
        ];
        for (kind, params, samples) in probes {
            let curve = Curve::parse(&para(kind, &params)).unwrap();
            for (input, expected) in samples {
                assert!(
                    (curve.eval(input) - expected).abs() < 1e-12,
                    "type {kind}, x={input}"
                );
            }
        }
    }

    #[test]
    fn sampled_curves_interpolate_and_invert_plateaus_by_icc_annex_f() {
        let curve = Curve::parse(&table(&[0, 16384, 16384, 65535])).unwrap();
        assert!((curve.eval(1.0 / 6.0) - 8192.0 / 65535.0).abs() < 1e-12);
        assert!((curve.inverse(16384.0 / 65535.0) - 2.0 / 3.0).abs() < 1e-12);
        let terminal = Curve::parse(&table(&[0, 16384, 65535, 65535])).unwrap();
        assert!((terminal.inverse(1.0) - 2.0 / 3.0).abs() < 1e-12);
        let initial = Curve::parse(&table(&[0, 0, 16384, 65535])).unwrap();
        assert!((initial.inverse(0.0) - 1.0 / 3.0).abs() < 1e-12);
        assert!((curve.inverse(curve.eval(0.8)) - 0.8).abs() < 1e-12);
        assert!(Curve::parse(&table(&[0, 65535, 32768])).is_err());
        assert!(Curve::parse(&table(&[42, 42])).is_err());
        let identity = Curve::parse(&table(&[])).unwrap();
        assert_eq!(identity.eval(0.25), 0.25);
        let gamma = Curve::parse(&table(&[512])).unwrap();
        assert!((gamma.eval(0.5) - 0.25).abs() < 1e-12);
        assert!(Curve::parse(&table(&[0])).is_err());
    }

    #[test]
    fn undefined_truncated_and_oversized_curves_fail() {
        for bytes in [
            para(0, &[0.0]),
            para(1, &[2.0, 0.0, 0.0]),
            para(3, &[2.0, 1.0, -1.0, 0.5, 0.5]),
            para(3, &[2.0, 1.0, 0.0, 1.0, 0.5]),
            para(5, &[1.0]),
            para(4, &[2.0]),
            b"curv\0\0\0\0\xff\xff\xff\xff".to_vec(),
        ] {
            assert!(Curve::parse(&bytes).is_err(), "{bytes:?}");
        }
        let valid = para(4, &[2.0, 1.0, 0.0, 0.5, 0.5, 0.0, 0.0]);
        for end in 0..valid.len() {
            assert!(Curve::parse(&valid[..end]).is_err());
        }
    }
}

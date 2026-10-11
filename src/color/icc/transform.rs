//! Intent selection and physical XYZ boundaries shared by managed workflows.
use super::{signature, xyz, GrayTransform, LutSample, LutTransform, MatrixTransform, Profile};
use crate::color::pcs::D50;
use anyhow::{bail, ensure, Result};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Intent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl Intent {
    pub fn icc_number(self) -> u32 {
        match self {
            Self::Perceptual => 0,
            Self::RelativeColorimetric => 1,
            Self::Saturation => 2,
            Self::AbsoluteColorimetric => 3,
        }
    }

    fn lut_index(self) -> u8 {
        match self {
            Self::Perceptual => b'0',
            Self::Saturation => b'2',
            _ => b'1',
        }
    }
}

impl std::str::FromStr for Intent {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "perceptual" => Ok(Self::Perceptual),
            "relative-colorimetric" => Ok(Self::RelativeColorimetric),
            "saturation" => Ok(Self::Saturation),
            "absolute-colorimetric" => Ok(Self::AbsoluteColorimetric),
            _ => bail!(
                "[invalid-input] intent must be perceptual, relative-colorimetric, saturation or absolute-colorimetric"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    DeviceToPcs,
    PcsToDevice,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransformDescription {
    pub intent: Intent,
    pub direction: Direction,
    pub path: String,
    /// A matrix/TRC model has no separate perceptual/saturation gamut mapping.
    pub colorimetric_equivalent: bool,
    pub absolute_media_white: Option<[f64; 3]>,
}

#[derive(Debug, Clone, Copy)]
pub struct TransformSample {
    values: [f64; 4],
    count: usize,
    pub clipped: bool,
}

impl TransformSample {
    pub fn channels(&self) -> &[f64] {
        &self.values[..self.count]
    }
}

#[derive(Debug)]
enum Engine<'a> {
    Rgb(MatrixTransform),
    Gray(GrayTransform),
    Lut(LutTransform<'a>),
}

#[derive(Debug)]
pub struct Transform<'a> {
    engine: Engine<'a>,
    description: TransformDescription,
}

impl<'a> Transform<'a> {
    /// Select a single intent path before rendering. Missing/unsupported LUTs
    /// never fall back to a different intent or a matrix approximation.
    pub fn compile(profile: &Profile<'a>, direction: Direction, intent: Intent) -> Result<Self> {
        ensure!(
            &profile.bytes[12..16] != b"link",
            "[unsupported-capability] compile a device link explicitly with DeviceLinkTransform"
        );
        let mut extended = if direction == Direction::DeviceToPcs {
            *b"D2B0"
        } else {
            *b"B2D0"
        };
        extended[3] = b'0' + intent.icc_number() as u8;
        ensure!(
            profile.tag(&extended).is_none(),
            "[unsupported-capability] ICC {} has precedence and requires unsupported multi-process execution",
            signature(&extended)
        );
        let mut tag = if direction == Direction::DeviceToPcs {
            *b"A2B0"
        } else {
            *b"B2A0"
        };
        tag[3] = intent.lut_index();
        let (engine, path, colorimetric_equivalent) = if profile.tag(&tag).is_some() {
            (
                Engine::Lut(LutTransform::compile(profile, &tag)?),
                signature(&tag),
                false,
            )
        } else {
            // Presence of another LUT is not authorization to substitute it.
            if let Err(error) = profile.reject_lut_substitution() {
                bail!(
                    "[unsupported-capability] ICC requested {} is missing; cannot substitute another path: {error}",
                    signature(&tag)
                );
            }
            if &profile.bytes[16..20] == b"GRAY" {
                (
                    Engine::Gray(GrayTransform::compile(profile)?),
                    "gray-trc".to_owned(),
                    true,
                )
            } else {
                (
                    Engine::Rgb(MatrixTransform::compile(profile)?),
                    "rgb-matrix-trc".to_owned(),
                    true,
                )
            }
        };
        let absolute_media_white = if intent == Intent::AbsoluteColorimetric {
            let white = xyz(profile.required(b"wtpt")?)?;
            ensure!(
                white.iter().all(|v| *v > 0.0),
                "[malformed-resource] ICC absolute transform requires a positive media white point"
            );
            Some(white)
        } else {
            None
        };
        Ok(Self {
            engine,
            description: TransformDescription {
                intent,
                direction,
                path,
                colorimetric_equivalent,
                absolute_media_white,
            },
        })
    }

    pub fn description(&self) -> &TransformDescription {
        &self.description
    }

    pub fn to_xyz(&self, input: &[f64]) -> Result<([f64; 3], bool)> {
        ensure!(
            self.description.direction == Direction::DeviceToPcs,
            "[invalid-input] transform was compiled for PCS-to-device"
        );
        let (mut pcs, clipped) = match &self.engine {
            Engine::Rgb(engine) => {
                let input: [f64; 3] = input.try_into().map_err(|_| {
                    anyhow::anyhow!("[invalid-input] RGB transform requires three channels")
                })?;
                (engine.to_xyz(input)?, false)
            }
            Engine::Gray(engine) => {
                ensure!(
                    input.len() == 1,
                    "[invalid-input] gray transform requires one channel"
                );
                (engine.to_xyz(input[0])?, false)
            }
            Engine::Lut(engine) => engine.to_xyz(input)?,
        };
        if let Some(white) = self.description.absolute_media_white {
            pcs = std::array::from_fn(|i| pcs[i] * white[i] / D50[i]);
            ensure!(
                pcs.iter().all(|v| v.is_finite()),
                "[invalid-input] absolute ICC scaling overflow"
            );
        }
        Ok((pcs, clipped))
    }

    pub fn from_xyz(&self, mut pcs: [f64; 3]) -> Result<TransformSample> {
        ensure!(
            self.description.direction == Direction::PcsToDevice,
            "[invalid-input] transform was compiled for device-to-PCS"
        );
        if let Some(white) = self.description.absolute_media_white {
            pcs = std::array::from_fn(|i| pcs[i] * D50[i] / white[i]);
        }
        match &self.engine {
            Engine::Rgb(engine) => {
                let (v, clipped) = engine.from_xyz(pcs)?;
                Ok(TransformSample {
                    values: [v[0], v[1], v[2], 0.0],
                    count: 3,
                    clipped,
                })
            }
            Engine::Gray(engine) => {
                let (v, clipped) = engine.from_xyz(pcs)?;
                Ok(TransformSample {
                    values: [v, 0.0, 0.0, 0.0],
                    count: 1,
                    clipped,
                })
            }
            Engine::Lut(engine) => {
                let sample = engine.from_xyz(pcs)?;
                let mut values = [0.0; 4];
                values[..sample.channels().len()].copy_from_slice(sample.channels());
                Ok(TransformSample {
                    values,
                    count: sample.channels().len(),
                    clipped: sample.clipped,
                })
            }
        }
    }
}

#[derive(Debug)]
pub struct DeviceLinkTransform<'a> {
    engine: LutTransform<'a>,
    intent: Intent,
}

impl<'a> DeviceLinkTransform<'a> {
    pub fn intent(&self) -> Intent {
        self.intent
    }

    pub fn compile(profile: &Profile<'a>, intent: Intent) -> Result<Self> {
        ensure!(
            &profile.bytes[12..16] == b"link",
            "[invalid-input] explicit device-link execution requires link class"
        );
        let recorded = super::u32_at(profile.bytes, 64)?;
        ensure!(
            recorded == intent.icc_number(),
            "[invalid-input] device-link intent {recorded} differs from requested {}",
            intent.icc_number()
        );
        for tag in [b"D2B0", b"D2B1", b"D2B2", b"D2B3"] {
            if profile.tag(tag).is_some() {
                bail!(
                    "[unsupported-capability] device-link {} requires multi-process execution",
                    signature(tag)
                );
            }
        }
        Ok(Self {
            engine: LutTransform::compile(profile, b"A2B0")?,
            intent,
        })
    }

    pub fn evaluate(&self, channels: &[f64]) -> Result<LutSample> {
        self.engine.evaluate(channels)
    }
}

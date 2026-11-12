//! Compose profile paths through physical D50 XYZ with explicit black mapping.
use super::{Direction, Intent, Profile, Transform, TransformDescription};
use crate::color::pcs::{self, D50};
use anyhow::{ensure, Context, Result};
use serde::Serialize;

/// ICC v4 perceptual reference-medium black, also used for saturation linking.
pub const PERCEPTUAL_BLACK: [f64; 3] = [0.00336, 0.0034731, 0.00287];

#[derive(Debug, Clone, Serialize)]
pub struct BlackMapping {
    pub source_black: [f64; 3],
    pub destination_black: [f64; 3],
    pub scale: [f64; 3],
    pub offset: [f64; 3],
    pub algorithm: &'static str,
}

impl BlackMapping {
    fn new(source: [f64; 3], destination: [f64; 3]) -> Result<Self> {
        ensure!(
            source.iter().zip(D50).all(|(b, w)| b.is_finite() && *b < w)
                && destination
                    .iter()
                    .zip(D50)
                    .all(|(b, w)| b.is_finite() && *b < w),
            "[malformed-resource] ICC black mapping requires finite black points below D50"
        );
        let scale = std::array::from_fn(|i| (D50[i] - destination[i]) / (D50[i] - source[i]));
        let offset = std::array::from_fn(|i| destination[i] - scale[i] * source[i]);
        Ok(Self {
            source_black: source,
            destination_black: destination,
            scale,
            offset,
            algorithm: "xyz-affine-black-white-v1",
        })
    }
    fn apply(&self, xyz: [f64; 3]) -> Result<[f64; 3]> {
        let result = std::array::from_fn(|i| self.scale[i] * xyz[i] + self.offset[i]);
        ensure!(
            result.iter().all(|v| v.is_finite()),
            "[invalid-input] ICC black mapping overflow"
        );
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkDescription {
    pub intent: Intent,
    /// None means a physical D50 XYZ endpoint, not an untagged device space.
    pub source: Option<TransformDescription>,
    pub destination: Option<TransformDescription>,
    pub black_mapping: Option<BlackMapping>,
    pub optional_colorimetric_bpc: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct LinkSample {
    values: [f64; 4],
    count: usize,
    pub clipped: bool,
}
impl LinkSample {
    pub fn channels(&self) -> &[f64] {
        &self.values[..self.count]
    }
}

#[derive(Debug)]
pub struct Link<'a> {
    source: Option<Transform<'a>>,
    destination: Option<Transform<'a>>,
    description: LinkDescription,
}

fn black_point(profile: &Profile<'_>, selected: &Transform<'_>) -> Result<[f64; 3]> {
    if selected.description().colorimetric_equivalent || profile.bytes[8] == 2 {
        // v2 source black is the selected intent's darkest device colorant.
        // A matrix/TRC model shares its relative path for every intent.
        let intent = if selected.description().colorimetric_equivalent {
            Intent::RelativeColorimetric
        } else {
            selected.description().intent
        };
        let forward = Transform::compile(profile, Direction::DeviceToPcs, intent)?;
        let (sample, count) = match &profile.bytes[16..20] {
            b"RGB " => ([0.0; 4], 3),
            b"GRAY" => ([0.0; 4], 1),
            b"CMYK" => ([1.0; 4], 4),
            _ => unreachable!("link planner validated device channel space"),
        };
        let (xyz, _) = forward.to_xyz(&sample[..count])?;
        let mut lab = pcs::xyz_to_lab(xyz)?;
        // Bound inverted synthetic black while retaining chromaticity.
        lab[0] = if lab[0] > 95.0 {
            0.0
        } else {
            lab[0].clamp(0.0, 50.0)
        };
        return pcs::lab_to_xyz(lab);
    }
    Ok(PERCEPTUAL_BLACK)
}

impl<'a> Link<'a> {
    pub fn compile(
        source: &Profile<'a>,
        destination: &Profile<'a>,
        intent: Intent,
    ) -> Result<Self> {
        Self::plan(Some(source), Some(destination), intent)
    }
    /// Input is physical XYZ; Lab callers use pcs::lab_to_xyz first.
    pub fn from_pcs(destination: &Profile<'a>, intent: Intent) -> Result<Self> {
        Self::plan(None, Some(destination), intent)
    }
    /// Output is physical XYZ, retaining extended finite values.
    pub fn to_pcs(source: &Profile<'a>, intent: Intent) -> Result<Self> {
        Self::plan(Some(source), None, intent)
    }
    fn plan(
        source: Option<&Profile<'a>>,
        destination: Option<&Profile<'a>>,
        intent: Intent,
    ) -> Result<Self> {
        for profile in source.into_iter().chain(destination) {
            ensure!(
                matches!(&profile.bytes[16..20], b"RGB " | b"GRAY" | b"CMYK"),
                "[unsupported-capability] ICC profile linking requires RGB, gray or CMYK device channels; use a physical PCS endpoint for Lab/XYZ"
            );
        }
        let source_path = source
            .map(|p| {
                Transform::compile(p, Direction::DeviceToPcs, intent).context("ICC link source")
            })
            .transpose()?;
        let destination_path = destination
            .map(|p| {
                Transform::compile(p, Direction::PcsToDevice, intent)
                    .context("ICC link destination")
            })
            .transpose()?;
        // Linking behavior is determined at the destination boundary. A
        // physical PCS endpoint does not request additional black mapping.
        let implicit = matches!(intent, Intent::Perceptual | Intent::Saturation)
            && destination.is_some_and(|p| p.bytes[8] == 4);
        let black_mapping = if implicit {
            let from = match (source, source_path.as_ref()) {
                (Some(p), Some(t)) => black_point(p, t).context("ICC link source black")?,
                _ => [0.0; 3],
            };
            let to = match (destination, destination_path.as_ref()) {
                (Some(p), Some(t)) => black_point(p, t).context("ICC link destination black")?,
                _ => [0.0; 3],
            };
            if from == to {
                None
            } else {
                Some(BlackMapping::new(from, to)?)
            }
        } else {
            None
        };
        let description = LinkDescription {
            intent,
            source: source_path.as_ref().map(|p| p.description().clone()),
            destination: destination_path.as_ref().map(|p| p.description().clone()),
            black_mapping,
            optional_colorimetric_bpc: false,
        };
        Ok(Self {
            source: source_path,
            destination: destination_path,
            description,
        })
    }
    pub fn description(&self) -> &LinkDescription {
        &self.description
    }
    pub fn evaluate(&self, channels: &[f64]) -> Result<LinkSample> {
        let (mut xyz, mut clipped) = match &self.source {
            Some(source) => source.to_xyz(channels)?,
            None => {
                let xyz: [f64; 3] = channels
                    .try_into()
                    .context("[invalid-input] ICC PCS link requires three physical XYZ channels")?;
                ensure!(
                    xyz.iter().all(|v| v.is_finite()),
                    "[invalid-input] ICC PCS link requires finite XYZ"
                );
                (xyz, false)
            }
        };
        if let Some(mapping) = &self.description.black_mapping {
            xyz = mapping.apply(xyz)?;
        }
        if let Some(destination) = &self.destination {
            let result = destination.from_xyz(xyz)?;
            clipped |= result.clipped;
            let mut values = [0.0; 4];
            values[..result.channels().len()].copy_from_slice(result.channels());
            Ok(LinkSample {
                values,
                count: result.channels().len(),
                clipped,
            })
        } else {
            Ok(LinkSample {
                values: [xyz[0], xyz[1], xyz[2], 0.0],
                count: 3,
                clipped,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn affine_mapping_preserves_white_maps_black_and_inverts_extended_values() {
        let mapping = BlackMapping::new([0.0; 3], PERCEPTUAL_BLACK).unwrap();
        let inverse = BlackMapping::new(PERCEPTUAL_BLACK, [0.0; 3]).unwrap();
        assert_eq!(mapping.apply([0.0; 3]).unwrap(), PERCEPTUAL_BLACK);
        for xyz in [D50, [0.1, 0.2, 0.3], [-0.1, 1.2, 0.5]] {
            let actual = inverse.apply(mapping.apply(xyz).unwrap()).unwrap();
            for (a, b) in actual.iter().zip(xyz) {
                assert!((a - b).abs() < 1e-14);
            }
        }
        for (a, b) in mapping.apply(D50).unwrap().iter().zip(D50) {
            assert!((a - b).abs() < 1e-14);
        }
        assert!(BlackMapping::new(D50, [0.0; 3]).is_err());
        assert!(BlackMapping::new([f64::NAN; 3], [0.0; 3]).is_err());
        assert!(mapping.apply([f64::INFINITY; 3]).is_err());
    }
}

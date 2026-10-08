//! RGB color spaces, transfer functions and matrices for photo engine 1.
//!
//! The working space is linear ProPhoto (ROMM primaries, D50 white). Every
//! matrix is derived in `f64` from the chromaticities in
//! `docs/photography-v1.md`, and adaptation between whites uses Bradford.
use super::math;
use anyhow::{bail, Result};

pub type Matrix = [[f64; 3]; 3];
pub type Xy = (f64, f64);

pub const D65: Xy = (0.3127, 0.3290);
pub const D50: Xy = (0.3457, 0.3585);

/// Id of the photo engine 1 working space.
pub const WORKING_SPACE: &str = "prophoto-linear";

/// An encoding curve between linear light and stored values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transfer {
    Linear,
    /// IEC 61966-2-1 piecewise sRGB curve (sRGB and Display P3).
    Srgb,
    /// Pure power `v = L^(1/gamma)` (Adobe RGB 1998 uses 563/256).
    Gamma(f64),
    /// ROMM RGB: linear segment `16 L` below `1/512`, gamma 1.8 above.
    Romm,
    /// ITU-R BT.709/BT.2020 OETF with the BT.2020 high-precision constants.
    Bt709,
    /// SMPTE ST 2084 perceptual quantizer; linear 1.0 is 10,000 cd/m².
    Pq,
    /// ITU-R BT.2100 hybrid log-gamma OETF on scene light in [0, 1].
    Hlg,
}

const BT_ALPHA: f64 = 1.099_296_826_809_44;
const BT_BETA: f64 = 0.018_053_968_510_807;
const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;
const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 1.0 - 4.0 * HLG_A;
const HLG_C: f64 = 0.559_910_729_529_562; // 0.5 - a ln(4a)

impl Transfer {
    /// Encoded value to linear light. Negative inputs mirror (`-f(-v)`), so
    /// signed values survive a round trip.
    pub fn decode(self, value: f64) -> f64 {
        if value < 0.0 {
            return -self.decode(-value);
        }
        match self {
            Self::Linear => value,
            Self::Srgb => {
                if value <= 0.040_45 {
                    value / 12.92
                } else {
                    math::pow((value + 0.055) / 1.055, 2.4)
                }
            }
            Self::Gamma(gamma) => math::pow(value, gamma),
            Self::Romm => {
                if value < 16.0 / 512.0 {
                    value / 16.0
                } else {
                    math::pow(value, 1.8)
                }
            }
            Self::Bt709 => {
                if value < 4.5 * BT_BETA {
                    value / 4.5
                } else {
                    math::pow((value + (BT_ALPHA - 1.0)) / BT_ALPHA, 1.0 / 0.45)
                }
            }
            Self::Pq => {
                let p = math::pow(value, 1.0 / PQ_M2);
                let numerator = (p - PQ_C1).max(0.0);
                math::pow(numerator / (PQ_C2 - PQ_C3 * p), 1.0 / PQ_M1)
            }
            Self::Hlg => {
                if value <= 0.5 {
                    value * value / 3.0
                } else {
                    (math::exp((value - HLG_C) / HLG_A) + HLG_B) / 12.0
                }
            }
        }
    }

    /// Linear light to encoded value; the inverse of [`Transfer::decode`].
    pub fn encode(self, linear: f64) -> f64 {
        if linear < 0.0 {
            return -self.encode(-linear);
        }
        match self {
            Self::Linear => linear,
            Self::Srgb => {
                if linear <= 0.003_130_8 {
                    linear * 12.92
                } else {
                    1.055 * math::pow(linear, 1.0 / 2.4) - 0.055
                }
            }
            Self::Gamma(gamma) => math::pow(linear, 1.0 / gamma),
            Self::Romm => {
                if linear < 1.0 / 512.0 {
                    linear * 16.0
                } else {
                    math::pow(linear, 1.0 / 1.8)
                }
            }
            Self::Bt709 => {
                if linear < BT_BETA {
                    linear * 4.5
                } else {
                    BT_ALPHA * math::pow(linear, 0.45) - (BT_ALPHA - 1.0)
                }
            }
            // ST 2084 maps 0 to c1^m2 (about 7.3e-7); black stays exactly 0.
            Self::Pq if linear == 0.0 => 0.0,
            Self::Pq => {
                let y = math::pow(linear, PQ_M1);
                math::pow((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2)
            }
            Self::Hlg => {
                if linear <= 1.0 / 12.0 {
                    (3.0 * linear).sqrt()
                } else {
                    HLG_A * math::ln(12.0 * linear - HLG_B) + HLG_C
                }
            }
        }
    }
}

/// A named RGB space: primaries, white point and transfer.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorSpace {
    pub id: String,
    pub red: Xy,
    pub green: Xy,
    pub blue: Xy,
    pub white: Xy,
    pub transfer: Transfer,
}

/// The named spaces of `docs/photography-v1.md`, in table order.
pub const NAMED_SPACES: [&str; 5] = [
    "srgb",
    "display-p3",
    "adobe-rgb-1998",
    "prophoto",
    "rec2020",
];

impl ColorSpace {
    /// Parse `NAME`, `NAME:linear`, or the working space id `prophoto-linear`.
    /// PQ and HLG are not ids: a recipe's `hdr.transfer` sets them on `rec2020`.
    pub fn named(id: &str) -> Result<Self> {
        if id == WORKING_SPACE {
            return Self::named("prophoto:linear").map(|space| Self {
                id: id.into(),
                ..space
            });
        }
        let (name, transfer) = match id.split_once(':') {
            Some((name, transfer)) => (name, Some(transfer)),
            None => (id, None),
        };
        let (red, green, blue, white, natural) = match name {
            "srgb" => (
                (0.64, 0.33),
                (0.30, 0.60),
                (0.15, 0.06),
                D65,
                Transfer::Srgb,
            ),
            "display-p3" => (
                (0.680, 0.320),
                (0.265, 0.690),
                (0.150, 0.060),
                D65,
                Transfer::Srgb,
            ),
            "adobe-rgb-1998" => (
                (0.64, 0.33),
                (0.21, 0.71),
                (0.15, 0.06),
                D65,
                Transfer::Gamma(563.0 / 256.0),
            ),
            "prophoto" => (
                (0.7347, 0.2653),
                (0.1596, 0.8404),
                (0.0366, 0.0001),
                D50,
                Transfer::Romm,
            ),
            "rec2020" => (
                (0.708, 0.292),
                (0.170, 0.797),
                (0.131, 0.046),
                D65,
                Transfer::Bt709,
            ),
            other => bail!(
                "[unsupported-capability] unknown color space {other}; use one of {}",
                NAMED_SPACES.join(", ")
            ),
        };
        let transfer = match transfer {
            None => natural,
            Some("linear") => Transfer::Linear,
            Some(other) => bail!(
                "[unsupported-capability] transfer {other} is not available as {name}:{other}; use {name} or {name}:linear (HDR transfers are set by a recipe's hdr.transfer)"
            ),
        };
        Ok(Self {
            id: id.into(),
            red,
            green,
            blue,
            white,
            transfer,
        })
    }

    /// The photo engine 1 working space (linear ProPhoto, D50).
    pub fn working() -> Self {
        Self::named(WORKING_SPACE).expect("working space is built in")
    }

    /// Linear RGB to CIE XYZ relative to this space's own white (Y of white = 1).
    pub fn to_xyz(&self) -> Matrix {
        let primaries = [xyz(self.red), xyz(self.green), xyz(self.blue)];
        let p = [
            [primaries[0][0], primaries[1][0], primaries[2][0]],
            [primaries[0][1], primaries[1][1], primaries[2][1]],
            [primaries[0][2], primaries[1][2], primaries[2][2]],
        ];
        let s = apply(&invert(&p), xyz(self.white));
        let mut m = p;
        for row in &mut m {
            for (column, scale) in s.iter().enumerate() {
                row[column] *= scale;
            }
        }
        m
    }

    /// Linear RGB in this space to linear working-space RGB.
    pub fn to_working(&self) -> Matrix {
        let working = Self::working();
        multiply(
            &invert(&working.to_xyz()),
            &multiply(&bradford(self.white, working.white), &self.to_xyz()),
        )
    }

    /// Linear working-space RGB to linear RGB in this space.
    pub fn from_working(&self) -> Matrix {
        invert(&self.to_working())
    }
}

/// xy chromaticity to XYZ with Y = 1.
pub fn xyz((x, y): Xy) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Bradford chromatic adaptation from one white to another, in XYZ.
pub fn bradford(from: Xy, to: Xy) -> Matrix {
    const B: Matrix = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    if from == to {
        return identity();
    }
    let source = apply(&B, xyz(from));
    let target = apply(&B, xyz(to));
    let mut scale = identity();
    for i in 0..3 {
        scale[i][i] = target[i] / source[i];
    }
    multiply(&invert(&B), &multiply(&scale, &B))
}

pub fn identity() -> Matrix {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

pub fn apply(m: &Matrix, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

/// Inverse by cofactors. The matrices here are well-conditioned color matrices.
pub fn invert(m: &Matrix) -> Matrix {
    let c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
    let c01 = m[1][2] * m[2][0] - m[1][0] * m[2][2];
    let c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
    let det = m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02;
    [
        [
            c00 / det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / det,
        ],
        [
            c01 / det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / det,
        ],
        [
            c02 / det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / det,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: &Matrix, b: &Matrix, tolerance: f64) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() <= tolerance))
    }

    #[test]
    fn derived_matrices_match_published_values() {
        // IEC 61966-2-1 sRGB to XYZ (D65), as published to four decimals.
        let srgb = [
            [0.4124, 0.3576, 0.1805],
            [0.2126, 0.7152, 0.0722],
            [0.0193, 0.1192, 0.9505],
        ];
        assert!(near(
            &ColorSpace::named("srgb").unwrap().to_xyz(),
            &srgb,
            1e-4
        ));
        // ROMM RGB to XYZ. ISO 22028-2 publishes Z = 0.8249 for the ICC D50
        // white; the frozen xy white (0.3457, 0.3585) gives 0.8251.
        let romm = [
            [0.7977, 0.1352, 0.0313],
            [0.2880, 0.7119, 0.0001],
            [0.0000, 0.0000, 0.8251],
        ];
        assert!(near(&ColorSpace::working().to_xyz(), &romm, 2e-4));
        // Bradford D65 to D50 as published by Lindbloom, who uses XYZ whites
        // rather than the frozen xy whites, so agreement is only to 1e-3.
        let adapt = [
            [1.0478112, 0.0228866, -0.0501270],
            [0.0295424, 0.9904844, -0.0170491],
            [-0.0092345, 0.0150436, 0.7521316],
        ];
        let derived = bradford(D65, D50);
        assert!(near(&derived, &adapt, 1e-3));
        // The defining property is exact: the source white lands on the target.
        let white = apply(&derived, xyz(D65));
        for (channel, target) in white.iter().zip(xyz(D50)) {
            assert!((channel - target).abs() < 1e-12, "{white:?}");
        }
    }

    #[test]
    fn white_maps_to_white_and_inverses_compose_to_identity() {
        for name in NAMED_SPACES {
            let space = ColorSpace::named(name).unwrap();
            let white = apply(&space.to_working(), [1.0, 1.0, 1.0]);
            for channel in white {
                assert!((channel - 1.0).abs() < 1e-12, "{name}: {white:?}");
            }
            let round = multiply(&space.from_working(), &space.to_working());
            assert!(near(&round, &identity(), 1e-12), "{name}");
        }
    }

    #[test]
    fn transfers_are_monotonic_inverses() {
        let transfers = [
            Transfer::Linear,
            Transfer::Srgb,
            Transfer::Gamma(563.0 / 256.0),
            Transfer::Romm,
            Transfer::Bt709,
            Transfer::Pq,
            Transfer::Hlg,
        ];
        for transfer in transfers {
            let mut previous = -1.0;
            for step in 0..=4096 {
                let encoded = f64::from(step) / 4096.0;
                let linear = transfer.decode(encoded);
                assert!(linear > previous || step == 0, "{transfer:?} at {encoded}");
                previous = linear;
                let back = transfer.encode(linear);
                assert!(
                    (back - encoded).abs() < 1e-12,
                    "{transfer:?}: {encoded} -> {back}"
                );
            }
            assert!((transfer.decode(-0.25) + transfer.decode(0.25)).abs() == 0.0);
        }
        // Reference points: sRGB mid gray, PQ at 100 cd/m², HLG reference white.
        assert!((Transfer::Srgb.decode(0.5) - 0.214_041_140_5).abs() < 1e-9);
        assert!((Transfer::Pq.encode(0.01) - 0.508_078_421_5).abs() < 1e-9);
        assert!((Transfer::Hlg.encode(1.0) - 1.0).abs() < 1e-7);
    }

    #[test]
    fn names_parse_and_unknown_names_fail_clearly() {
        assert_eq!(
            ColorSpace::named("srgb:linear").unwrap().transfer,
            Transfer::Linear
        );
        assert!(ColorSpace::named("rec2020:pq").is_err());
        assert_eq!(ColorSpace::working().transfer, Transfer::Linear);
        assert_eq!(ColorSpace::working().white, D50);
        let error = ColorSpace::named("cmyk").unwrap_err().to_string();
        assert!(error.starts_with("[unsupported-capability]") && error.contains("display-p3"));
        assert!(ColorSpace::named("srgb:pq").is_err());
    }
}

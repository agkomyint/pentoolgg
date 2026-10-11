//! Physical D50 XYZ and CIELAB values, independent of ICC table encodings.
use crate::photo::math;
use anyhow::{ensure, Result};

pub const D50: [f64; 3] = [0xF6D6 as f64 / 65536.0, 1.0, 0xD32D as f64 / 65536.0];

/// Extended finite PCS values are retained until explicit output clipping.
pub fn xyz_to_lab(xyz: [f64; 3]) -> Result<[f64; 3]> {
    ensure!(
        xyz.iter().all(|v| v.is_finite()),
        "[invalid-input] XYZ values must be finite"
    );
    let delta = 6.0 / 29.0;
    let f = |t: f64| {
        if t > delta * delta * delta {
            math::pow(t, 1.0 / 3.0)
        } else {
            t / (3.0 * delta * delta) + 4.0 / 29.0
        }
    };
    let [x, y, z] = std::array::from_fn(|i| f(xyz[i] / D50[i]));
    let lab = [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)];
    ensure!(
        lab.iter().all(|v| v.is_finite()),
        "[invalid-input] XYZ to Lab conversion overflow"
    );
    Ok(lab)
}

pub fn lab_to_xyz(lab: [f64; 3]) -> Result<[f64; 3]> {
    ensure!(
        lab.iter().all(|v| v.is_finite()),
        "[invalid-input] Lab values must be finite"
    );
    let y = (lab[0] + 16.0) / 116.0;
    let f = [y + lab[1] / 500.0, y, y - lab[2] / 200.0];
    let delta = 6.0 / 29.0;
    let xyz: [f64; 3] = std::array::from_fn(|i| {
        D50[i]
            * if f[i] > delta {
                f[i] * f[i] * f[i]
            } else {
                3.0 * delta * delta * (f[i] - 4.0 / 29.0)
            }
    });
    ensure!(
        xyz.iter().all(|v| v.is_finite()),
        "[invalid-input] Lab to XYZ conversion overflow"
    );
    Ok(xyz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_black_and_extended_pcs_round_trip() {
        for (xyz, lab) in [([0.0; 3], [0.0; 3]), (D50, [100.0, 0.0, 0.0])] {
            for (a, b) in xyz_to_lab(xyz).unwrap().iter().zip(lab) {
                assert!((a - b).abs() < 1e-12);
            }
        }
        for lab in [
            [0.0; 3],
            [100.0, 0.0, 0.0],
            [50.0, -128.0, 127.0],
            [120.0, 20.0, -20.0],
            [-2.0, 5.0, -10.0],
        ] {
            let back = xyz_to_lab(lab_to_xyz(lab).unwrap()).unwrap();
            for (a, b) in back.iter().zip(lab) {
                assert!((a - b).abs() < 1e-10);
            }
        }
        for invalid in [[f64::NAN; 3], [f64::INFINITY; 3], [f64::MAX; 3]] {
            assert!(xyz_to_lab(invalid).is_err());
            assert!(lab_to_xyz(invalid).is_err());
        }
    }
}

//! Deterministic arithmetic shared by every raster tool.
//!
//! Only IEEE-754 `+ - * /`, `sqrt`, `floor`, `round`, `%`, comparisons and integer
//! operations are used. Those are correctly rounded on every supported target, so
//! results are bit-identical on Linux, Windows and both macOS architectures. Never
//! call `sin`, `cos`, `exp`, `ln`, `powf` or other libm functions on the pixel path.

/// Sine and cosine of an angle in degrees with a fixed 14-term Taylor series.
pub fn sin_cos_degrees(degrees: f64) -> (f64, f64) {
    let mut d = degrees % 360.0;
    if d > 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    let x = d * (std::f64::consts::PI / 180.0);
    let x2 = x * x;
    let (mut sin, mut cos) = (x, 1.0);
    let (mut sin_term, mut cos_term) = (x, 1.0);
    for k in 1..=14 {
        let n = f64::from(2 * k);
        cos_term *= -x2 / ((n - 1.0) * n);
        cos += cos_term;
        sin_term *= -x2 / (n * (n + 1.0));
        sin += sin_term;
    }
    (sin, cos)
}

pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform in [-1, 1) from a 53-bit mantissa; exact in f64.
pub fn signed_unit(state: &mut u64) -> f64 {
    let bits = splitmix64(state) >> 11;
    (bits as f64) / ((1u64 << 52) as f64) - 1.0
}

pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trig_matches_known_values() {
        let (s, c) = sin_cos_degrees(30.0);
        assert!((s - 0.5).abs() < 1e-12 && (c - 0.866_025_403_784_438_6).abs() < 1e-12);
        let (s, c) = sin_cos_degrees(-270.0);
        assert!((s - 1.0).abs() < 1e-12 && c.abs() < 1e-12);
    }
}

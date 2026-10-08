//! Deterministic transcendental functions for photo engine 1.
//!
//! Only IEEE-754 `+ - * /`, `sqrt`, comparisons, `floor` and bit manipulation are used, so
//! every platform produces identical bits. Platform `libm` (`powf`, `ln`, `exp`)
//! is never called on a rendering path. Accuracy is about 1 ulp of `f64`, far
//! below one 16-bit code value.

const LN_2: f64 = std::f64::consts::LN_2;

/// Base-2 logarithm for finite `x > 0`. Returns `-inf` for `x <= 0`, `+inf`
/// for `+inf`, and NaN for NaN.
pub fn log2(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    let (mut mantissa, mut exponent) = split(x);
    // Center the mantissa on 1 so |z| <= 0.1716 and the series converges fast.
    if mantissa > std::f64::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }
    // ln(m) = 2 atanh(z), z = (m - 1) / (m + 1)
    let z = (mantissa - 1.0) / (mantissa + 1.0);
    let z2 = z * z;
    let mut term = z;
    let mut sum = 0.0;
    let mut denominator = 1.0;
    for _ in 0..16 {
        sum += term / denominator;
        term *= z2;
        denominator += 2.0;
    }
    f64::from(exponent) + 2.0 * sum / LN_2
}

/// Natural logarithm, defined through [`log2`].
pub fn ln(x: f64) -> f64 {
    log2(x) * LN_2
}

/// `2^t`. Saturates to `+inf` above 1024 and to `0` below -1075.
pub fn exp2(t: f64) -> f64 {
    if t.is_nan() {
        return f64::NAN;
    }
    if t >= 1024.0 {
        return f64::INFINITY;
    }
    if t < -1075.0 {
        return 0.0;
    }
    let whole = t.floor();
    let fraction = (t - whole) * LN_2; // in [0, ln 2)
    let mut term = 1.0;
    let mut sum = 1.0;
    for i in 1..20 {
        term *= fraction / f64::from(i);
        sum += term;
    }
    scale(sum, whole as i32)
}

/// `exp(t)`, defined through [`exp2`].
pub fn exp(t: f64) -> f64 {
    exp2(t / LN_2)
}

/// `x^y` for `x >= 0`. `0^y` is 0 for `y > 0`, `x^0` is 1, and negative `x`
/// returns NaN; callers mirror signed values explicitly.
pub fn pow(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x == 0.0 {
        return if y > 0.0 { 0.0 } else { f64::INFINITY };
    }
    exp2(y * log2(x))
}

const PI: f64 = std::f64::consts::PI;
const FRAC_PI_2: f64 = std::f64::consts::FRAC_PI_2;
// pi/2 split so `k * PI_2_HIGH` is exact for |k| < 2^20 (Cody–Waite).
const PI_2_HIGH: f64 = 1.570_796_326_734_125_6;
const PI_2_LOW: f64 = 6.077_100_506_506_192e-11;

/// `sin` and `cos` of `r` in [-pi/4, pi/4] by their Taylor series.
fn sin_cos_reduced(r: f64) -> (f64, f64) {
    let r2 = r * r;
    let mut sin = 0.0;
    let mut cos = 0.0;
    let mut s_term = r;
    let mut c_term = 1.0;
    for n in 0..12 {
        sin += s_term;
        cos += c_term;
        let k = f64::from(2 * n + 2);
        c_term = -c_term * r2 / (k * (k - 1.0));
        s_term = -s_term * r2 / (k * (k + 1.0));
    }
    (sin, cos)
}

/// `(sin x, cos x)` for radians with |x| up to about 1e6; NaN otherwise.
pub fn sin_cos(x: f64) -> (f64, f64) {
    if !x.is_finite() || x.abs() > 1.0e6 {
        return (f64::NAN, f64::NAN);
    }
    let k = (x / FRAC_PI_2 + 0.5).floor();
    let r = (x - k * PI_2_HIGH) - k * PI_2_LOW;
    let (s, c) = sin_cos_reduced(r);
    match (k as i64).rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

pub fn sin(x: f64) -> f64 {
    sin_cos(x).0
}

pub fn cos(x: f64) -> f64 {
    sin_cos(x).1
}

/// `atan(t)` for finite `t`, by two half-angle reductions and a series.
pub fn atan(t: f64) -> f64 {
    if t.is_nan() {
        return f64::NAN;
    }
    if t.is_infinite() {
        return FRAC_PI_2.copysign(t);
    }
    let (t, flip) = if t.abs() > 1.0 {
        (1.0 / t, true)
    } else {
        (t, false)
    };
    // atan(t) = 2 atan(t / (1 + sqrt(1 + t^2))); twice leaves |z| <= 0.2.
    let mut z = t;
    for _ in 0..2 {
        z /= 1.0 + (1.0 + z * z).sqrt();
    }
    let z2 = z * z;
    let mut term = z;
    let mut sum = 0.0;
    for n in 0..20 {
        sum += term / f64::from(2 * n + 1);
        term = -term * z2;
    }
    let angle = 4.0 * sum;
    if flip {
        FRAC_PI_2.copysign(t) - angle
    } else {
        angle
    }
}

/// `atan2(y, x)` in (-pi, pi]. `atan2(0, 0)` is 0.
pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 0.0 {
        return if y > 0.0 {
            FRAC_PI_2
        } else if y < 0.0 {
            -FRAC_PI_2
        } else {
            0.0
        };
    }
    let base = atan(y / x);
    if x > 0.0 {
        base
    } else if y >= 0.0 {
        base + PI
    } else {
        base - PI
    }
}

/// Degrees to radians.
pub fn radians(degrees: f64) -> f64 {
    degrees * (PI / 180.0)
}

/// Radians to degrees.
pub fn degrees(radians: f64) -> f64 {
    radians * (180.0 / PI)
}

/// Split a positive finite `x` into a mantissa in [1, 2) and an exponent.
fn split(x: f64) -> (f64, i32) {
    let mut bits = x.to_bits();
    let mut exponent = ((bits >> 52) & 0x7ff) as i32;
    let mut bias = 1023;
    if exponent == 0 {
        // Subnormal: scale by 2^54 to normalize.
        bits = (x * f64::from_bits(0x4350_0000_0000_0000)).to_bits();
        exponent = ((bits >> 52) & 0x7ff) as i32;
        bias += 54;
    }
    let mantissa = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    (mantissa, exponent - bias)
}

/// `value * 2^power` without a platform `ldexp`, including subnormal results.
fn scale(value: f64, power: i32) -> f64 {
    let mut value = value;
    let mut power = power;
    while power > 1000 {
        value *= f64::from_bits(((1023 + 1000) as u64) << 52);
        power -= 1000;
    }
    while power < -1000 {
        value *= f64::from_bits(((1023 - 1000) as u64) << 52);
        power += 1000;
    }
    value * f64::from_bits(((1023 + power) as u64) << 52)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, relative: f64) -> bool {
        (a - b).abs() <= relative * b.abs().max(f64::MIN_POSITIVE)
    }

    #[test]
    fn functions_match_reference_values_closely() {
        for &x in &[
            1e-300, 1e-12, 3e-5, 0.018, 0.5, 0.75, 1.0, 1.5, 2.0, 10.0, 1e6, 1e300,
        ] {
            assert!(
                close(log2(x), x.log2(), 1e-15) || (x == 1.0 && log2(x) == 0.0),
                "{x}"
            );
            assert!(close(ln(x), x.ln(), 1e-15) || x == 1.0, "{x}");
        }
        for &t in &[-1074.0, -60.5, -1.0, -0.3, 0.0, 0.3, 1.0, 12.25, 1023.5] {
            assert!(close(exp2(t), t.exp2(), 4e-16), "{t}");
        }
        for &(x, y) in &[
            (0.5, 2.4),
            (0.04, 1.0 / 2.4),
            (7.0, 0.45),
            (1e-11, 2.2),
            (3.0, -1.5),
        ] {
            assert!(close(pow(x, y), x.powf(y), 2e-15), "{x}^{y}");
        }
        assert_eq!(pow(0.0, 2.0), 0.0);
        assert_eq!(pow(5.0, 0.0), 1.0);
        assert!(pow(-1.0, 2.0).is_nan());
        assert_eq!(exp2(-2000.0), 0.0);
        assert_eq!(exp2(2000.0), f64::INFINITY);
        assert_eq!(log2(0.0), f64::NEG_INFINITY);
        assert!(close(log2(f64::from_bits(1)), -1074.0, 1e-15));
    }

    #[test]
    fn trigonometry_matches_reference_values_closely() {
        let mut x = -20.0;
        while x <= 20.0 {
            let (s, c) = sin_cos(x);
            assert!((s - x.sin()).abs() < 2e-15, "sin {x}");
            assert!((c - x.cos()).abs() < 2e-15, "cos {x}");
            x += 0.0137;
        }
        assert_eq!(sin_cos(0.0), (0.0, 1.0));
        for &t in &[
            -1e9, -30.0, -1.0, -0.4, -1e-9, 0.0, 0.2, 0.9, 1.0, 3.0, 1e12,
        ] {
            assert!((atan(t) - t.atan()).abs() < 2e-15, "atan {t}");
        }
        for &(y, x) in &[
            (1.0, 1.0),
            (1.0, -1.0),
            (-1.0, -1.0),
            (-1.0, 1.0),
            (0.0, -2.0),
            (3.0, 0.0),
            (-3.0, 0.0),
            (1e-8, 5.0),
            (2.0, -1e-9),
        ] {
            assert!((atan2(y, x) - f64::atan2(y, x)).abs() < 4e-15, "{y} {x}");
        }
        assert_eq!(atan2(0.0, 0.0), 0.0);
        assert!((degrees(radians(37.5)) - 37.5).abs() < 1e-13);
    }

    #[test]
    fn exact_powers_of_two_round_trip() {
        for power in -1070..1020 {
            let x = scale(1.0, power);
            assert_eq!(log2(x), f64::from(power));
            assert_eq!(exp2(f64::from(power)), x);
        }
    }
}

//! Deterministic math for the 3D engine.
//!
//! Everything here uses only IEEE-754 basic operations (`+ - * /` and `sqrt`,
//! which are correctly rounded) in a fixed order. There are no calls into the
//! platform `libm` (`sin`, `cos`, `tan`, `powf`, `exp`) and no `mul_add`, so the
//! same inputs produce the same bits on every release target.

pub type Vec3 = [f64; 3];
pub type Mat4 = [f64; 16];

pub const PI: f64 = std::f64::consts::PI;
pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
pub fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l > 1e-300 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Column-major product `a * b` (column vectors, glTF convention).
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

pub fn transform_point(m: &Mat4, p: Vec3) -> Vec3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

pub fn transform_dir(m: &Mat4, p: Vec3) -> Vec3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2],
    ]
}

/// Compose translation, rotation quaternion `[x, y, z, w]` and scale.
pub fn from_trs(t: Vec3, q: [f64; 4], s: Vec3) -> Mat4 {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let (x, y, z, w) = if n > 1e-300 {
        (q[0] / n, q[1] / n, q[2] / n, q[3] / n)
    } else {
        (0.0, 0.0, 0.0, 1.0)
    };
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        (1.0 - 2.0 * (yy + zz)) * s[0],
        2.0 * (xy + wz) * s[0],
        2.0 * (xz - wy) * s[0],
        0.0,
        2.0 * (xy - wz) * s[1],
        (1.0 - 2.0 * (xx + zz)) * s[1],
        2.0 * (yz + wx) * s[1],
        0.0,
        2.0 * (xz + wy) * s[2],
        2.0 * (yz - wx) * s[2],
        (1.0 - 2.0 * (xx + yy)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

/// Reduce an angle to `[-pi, pi]`.
fn reduce(x: f64) -> f64 {
    let two_pi = 2.0 * PI;
    let mut r = x - two_pi * ((x / two_pi) + 0.5).floor();
    if r > PI {
        r -= two_pi;
    } else if r < -PI {
        r += two_pi;
    }
    r
}

/// Sine by range reduction to `[-pi/2, pi/2]` and a fixed Taylor series.
pub fn sin(x: f64) -> f64 {
    let mut r = reduce(x);
    if r > PI / 2.0 {
        r = PI - r;
    } else if r < -PI / 2.0 {
        r = -PI - r;
    }
    let r2 = r * r;
    let mut term = r;
    let mut sum = r;
    let mut n = 1.0;
    for _ in 0..9 {
        term = -term * r2 / ((n + 1.0) * (n + 2.0));
        sum += term;
        n += 2.0;
    }
    sum
}

pub fn cos(x: f64) -> f64 {
    sin(x + PI / 2.0)
}

pub fn tan(x: f64) -> f64 {
    sin(x) / cos(x)
}

/// Arctangent: reduce to `|x| <= 1`, halve the angle twice, then a fixed series.
pub fn atan(x: f64) -> f64 {
    if x.is_nan() {
        return 0.0;
    }
    let negative = x < 0.0;
    let mut a = x.abs();
    let inverted = a > 1.0;
    if inverted {
        a = 1.0 / a;
    }
    // atan(a) = 2 atan(a / (1 + sqrt(1 + a^2))); two halvings bound a below 0.2.
    for _ in 0..2 {
        a /= 1.0 + (1.0 + a * a).sqrt();
    }
    let a2 = a * a;
    let mut term = a;
    let mut sum = a;
    for k in 1..12 {
        term = -term * a2;
        sum += term / (2 * k + 1) as f64;
    }
    let mut r = sum * 4.0;
    if inverted {
        r = PI / 2.0 - r;
    }
    if negative {
        -r
    } else {
        r
    }
}

/// Four-quadrant arctangent of `y / x`, in `(-pi, pi]`.
pub fn atan2(y: f64, x: f64) -> f64 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    if x > 0.0 {
        atan(y / x)
    } else if x < 0.0 {
        if y >= 0.0 {
            atan(y / x) + PI
        } else {
            atan(y / x) - PI
        }
    } else if y > 0.0 {
        PI / 2.0
    } else {
        -PI / 2.0
    }
}

/// Arccosine of a value clamped to `[-1, 1]`, in `[0, pi]`.
pub fn acos(x: f64) -> f64 {
    let x = x.clamp(-1.0, 1.0);
    atan2((1.0 - x * x).max(0.0).sqrt(), x)
}

/// `base^exponent` for a small non-negative integer exponent (repeated squaring).
pub fn powi(mut base: f64, mut exponent: u32) -> f64 {
    let mut result = 1.0;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result *= base;
        }
        base *= base;
        exponent >>= 1;
    }
    result
}

/// Natural exponential. Range-reduced Taylor series; no platform libm.
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let x = x.clamp(-700.0, 700.0);
    const LN2: f64 = std::f64::consts::LN_2;
    let k = (x / LN2).round();
    let r = x - k * LN2;
    let mut term = 1.0;
    let mut sum = 1.0;
    for n in 1..=18 {
        term *= r / n as f64;
        sum += term;
    }
    // Scale by 2^k through the exponent bits (exact).
    let k = k as i64;
    sum * f64::from_bits(((1023 + k) as u64) << 52)
}

/// Natural logarithm for positive finite `x` (non-positive returns -inf/NaN).
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    let bits = x.to_bits();
    let mut exponent = ((bits >> 52) & 0x7ff) as i64;
    if exponent == 0 {
        // Subnormal: scale into the normal range exactly.
        return ln(x * f64::from_bits(0x4330_0000_0000_0000)) - 52.0 * std::f64::consts::LN_2;
    }
    exponent -= 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | (1023u64 << 52));
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        exponent += 1;
    }
    // ln(m) = 2 atanh((m-1)/(m+1))
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let mut term = z;
    let mut sum = 0.0;
    let mut n = 1.0;
    for _ in 0..24 {
        sum += term / n;
        term *= z2;
        n += 2.0;
    }
    2.0 * sum + exponent as f64 * std::f64::consts::LN_2
}

/// `base^exponent` for `base >= 0`.
pub fn pow(base: f64, exponent: f64) -> f64 {
    if base <= 0.0 {
        return if exponent == 0.0 { 1.0 } else { 0.0 };
    }
    exp(exponent * ln(base))
}

pub fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

pub fn perspective(fov_y_radians: f64, aspect: f64, near: f64, far: f64) -> Mat4 {
    let f = 1.0 / tan(fov_y_radians / 2.0);
    let mut m = [0.0; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = (far + near) / (near - far);
    m[11] = -1.0;
    m[14] = 2.0 * far * near / (near - far);
    m
}

pub fn orthographic(half_width: f64, half_height: f64, near: f64, far: f64) -> Mat4 {
    let mut m = IDENTITY;
    m[0] = 1.0 / half_width;
    m[5] = 1.0 / half_height;
    m[10] = -2.0 / (far - near);
    m[14] = -(far + near) / (far - near);
    m
}

pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let f = normalize(sub(target, eye));
    let mut s = cross(f, up);
    if length(s) < 1e-9 {
        s = cross(f, [1.0, 0.0, 0.0]);
    }
    let s = normalize(s);
    let u = cross(s, f);
    [
        s[0],
        u[0],
        -f[0],
        0.0,
        s[1],
        u[1],
        -f[1],
        0.0,
        s[2],
        u[2],
        -f[2],
        0.0,
        -dot(s, eye),
        -dot(u, eye),
        dot(f, eye),
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trig_matches_known_values_without_libm() {
        assert!((sin(0.0)).abs() < 1e-15);
        assert!((sin(PI / 2.0) - 1.0).abs() < 1e-12);
        assert!((cos(PI) + 1.0).abs() < 1e-12);
        assert!((sin(100.0) - (-0.506_365_641_109_758_8)).abs() < 1e-10);
        assert!((tan(PI / 4.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn inverse_trig_matches_known_values() {
        assert!((atan(1.0) - PI / 4.0).abs() < 1e-12);
        assert!((atan(-3.0) + 1.249_045_772_398_254_4).abs() < 1e-10);
        assert!((atan2(1.0, -1.0) - 3.0 * PI / 4.0).abs() < 1e-12);
        assert!((atan2(-1.0, -1.0) + 3.0 * PI / 4.0).abs() < 1e-12);
        assert!((acos(0.5) - PI / 3.0).abs() < 1e-11);
        assert!((acos(-1.0) - PI).abs() < 1e-12);
    }

    #[test]
    fn identity_trs_is_identity() {
        let m = from_trs([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        assert_eq!(m, IDENTITY);
        let p = transform_point(&mul(&m, &IDENTITY), [1.0, 2.0, 3.0]);
        assert_eq!(p, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn exp_ln_pow_match_known_values() {
        assert!((exp(1.0) - std::f64::consts::E).abs() < 1e-12);
        assert!((ln(std::f64::consts::E) - 1.0).abs() < 1e-12);
        assert!((pow(2.0, 10.0) - 1024.0).abs() < 1e-9);
        assert!((pow(0.5, 1.0 / 2.4) - 0.749_153_538).abs() < 1e-6);
        assert_eq!(exp(0.5).to_bits(), exp(0.5).to_bits());
    }
}

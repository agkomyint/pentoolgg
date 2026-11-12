//! The DNG that pentool writes for merge outputs (`docs/photography-v1.md`,
//! "HDR and panorama merges"): LinearRaw, three `f16` samples, deflate with
//! floating-point predictor 34894, `ColorMatrix1` describing the working
//! primaries and a neutral `AsShotNeutral`, so the decoder develops it as
//! working-space RGB. Pixels outside the image are recorded in a DNG
//! transparency mask (`NewSubfileType` 4) in a SubIFD, which the reader below
//! turns into alpha.
use super::color::{self, ColorSpace};
use super::dng::Dng;
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};

/// Bytes per stored strip, before compression.
const STRIP_BYTES: usize = 1 << 20;

/// `f32` to IEEE half precision, rounding to nearest even and saturating at
/// the largest finite half.
pub fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;
    if exponent == 0xff {
        return sign | if mantissa == 0 { 0x7bff } else { 0 };
    }
    let e = exponent - 112;
    if e >= 0x1f {
        return sign | 0x7bff;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mantissa | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let mut r = m >> shift;
        let rest = m & ((1 << shift) - 1);
        if rest > half || (rest == half && r & 1 == 1) {
            r += 1;
        }
        return sign | r as u16;
    }
    let mut r = ((e as u32) << 10) | (mantissa >> 13);
    let rest = mantissa & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && r & 1 == 1) {
        r += 1;
    }
    sign | (r.min(0x7bff) as u16)
}

/// A merge output ready to write.
pub struct LinearDng<'a> {
    pub width: usize,
    pub height: usize,
    /// Interleaved working-space RGB; the writer scales it into the stored
    /// range and records the scale as an integer `BaselineExposure`.
    pub rgb: &'a [f32],
    /// Coverage in 0–1; written as an 8-bit transparency mask when present.
    pub alpha: Option<&'a [f32]>,
    pub model: &'a str,
}

/// The stored scale: the smallest power of two that keeps every sample at or
/// below 0.9, below the decoder's highlight blending.
pub fn baseline_exposure(rgb: &[f32]) -> i32 {
    let peak = rgb.iter().fold(0.0f32, |m, v| m.max(*v));
    let mut stops = 0i32;
    while stops < 10 && f64::from(peak) > 0.9 * f64::from(2.0f32.powi(stops)) {
        stops += 1;
    }
    stops
}

enum Value {
    Byte(Vec<u8>),
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    Srational(Vec<(i32, i32)>),
}

impl Value {
    fn encode(&self) -> (u16, u32, Vec<u8>) {
        match self {
            Value::Byte(v) => (1, v.len() as u32, v.clone()),
            Value::Ascii(s) => {
                let mut bytes = s.as_bytes().to_vec();
                bytes.push(0);
                (2, bytes.len() as u32, bytes)
            }
            Value::Short(v) => (
                3,
                v.len() as u32,
                v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            Value::Long(v) => (
                4,
                v.len() as u32,
                v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            ),
            Value::Rational(v) => (
                5,
                v.len() as u32,
                v.iter()
                    .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
                    .collect(),
            ),
            Value::Srational(v) => (
                10,
                v.len() as u32,
                v.iter()
                    .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
                    .collect(),
            ),
        }
    }
}

/// Append an IFD (and its out-of-line values) at the end of `out`, returning
/// its offset.
fn write_ifd(out: &mut Vec<u8>, mut entries: Vec<(u16, Value)>) -> Result<u32> {
    entries.sort_by_key(|(tag, _)| *tag);
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let at = out.len();
    let mut data_at = at + 2 + 12 * entries.len() + 4;
    let mut data = Vec::new();
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, value) in &entries {
        let (kind, count, bytes) = value.encode();
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        if bytes.len() <= 4 {
            let mut inline = bytes.clone();
            inline.resize(4, 0);
            out.extend(inline);
        } else {
            out.extend(offset(data_at)?.to_le_bytes());
            data_at += bytes.len() + bytes.len() % 2;
            data.extend(&bytes);
            if bytes.len() % 2 == 1 {
                data.push(0);
            }
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(data);
    offset(at)
}

fn offset(at: usize) -> Result<u32> {
    u32::try_from(at).context("[limit-exceeded] the merge output DNG exceeds 4 GiB")
}

fn deflate(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}

/// Append compressed strips of `rows_per_strip` rows, returning their offsets
/// and byte counts.
fn write_strips(
    out: &mut Vec<u8>,
    height: usize,
    rows_per_strip: usize,
    row: impl Fn(usize, &mut Vec<u8>),
) -> Result<(Vec<u32>, Vec<u32>)> {
    let (mut offsets, mut counts) = (Vec::new(), Vec::new());
    let mut y = 0;
    while y < height {
        super::check_cancelled()?;
        let mut raw = Vec::new();
        for r in y..(y + rows_per_strip).min(height) {
            row(r, &mut raw);
        }
        let packed = deflate(&raw)?;
        if out.len() % 2 == 1 {
            out.push(0);
        }
        offsets.push(offset(out.len())?);
        counts.push(packed.len() as u32);
        out.extend(packed);
        y += rows_per_strip;
    }
    Ok((offsets, counts))
}

/// A fraction as a signed rational with a fixed denominator.
fn srational(v: f64) -> (i32, i32) {
    const DENOMINATOR: i32 = 1 << 24;
    ((v * f64::from(DENOMINATOR)).round() as i32, DENOMINATOR)
}

impl LinearDng<'_> {
    pub fn write(&self) -> Result<Vec<u8>> {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || self.rgb.len() != w * h * 3 {
            bail!("[invalid-input] merge output buffer does not match {w}x{h}")
        }
        if self.alpha.is_some_and(|a| a.len() != w * h) {
            bail!("[invalid-input] merge output alpha does not match {w}x{h}")
        }
        let stops = baseline_exposure(self.rgb);
        let scale = 1.0 / 2.0f32.powi(stops);
        let mut out = b"II*\0\0\0\0\0".to_vec();

        let row_len = w * 6;
        let rows_per_strip = (STRIP_BYTES / row_len).clamp(1, h);
        let (offsets, counts) = write_strips(&mut out, h, rows_per_strip, |y, raw| {
            let start = raw.len();
            raw.resize(start + row_len, 0);
            let per_row = w * 3;
            let row = &mut raw[start..];
            for k in 0..per_row {
                let bits = f16_bits(self.rgb[y * per_row + k] * scale).to_be_bytes();
                row[k] = bits[0];
                row[per_row + k] = bits[1];
            }
            let stride = 3 * 2;
            for i in (stride..row_len).rev() {
                row[i] = row[i].wrapping_sub(row[i - stride]);
            }
        })?;

        let mask = match self.alpha {
            Some(alpha) => {
                let rows = (STRIP_BYTES / w).clamp(1, h);
                let (mask_offsets, mask_counts) = write_strips(&mut out, h, rows, |y, raw| {
                    raw.extend(
                        alpha[y * w..(y + 1) * w]
                            .iter()
                            .map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8),
                    );
                })?;
                Some(write_ifd(
                    &mut out,
                    vec![
                        (254, Value::Long(vec![4])),
                        (256, Value::Long(vec![w as u32])),
                        (257, Value::Long(vec![h as u32])),
                        (258, Value::Short(vec![8])),
                        (259, Value::Short(vec![8])),
                        (262, Value::Short(vec![4])),
                        (273, Value::Long(mask_offsets)),
                        (277, Value::Short(vec![1])),
                        (278, Value::Long(vec![rows as u32])),
                        (279, Value::Long(mask_counts)),
                        (284, Value::Short(vec![1])),
                    ],
                )?)
            }
            None => None,
        };

        let to_camera = color::invert(&ColorSpace::working().to_xyz());
        let matrix = to_camera.iter().flatten().map(|v| srational(*v)).collect();
        let mut entries = vec![
            (254, Value::Long(vec![0])),
            (256, Value::Long(vec![w as u32])),
            (257, Value::Long(vec![h as u32])),
            (258, Value::Short(vec![16; 3])),
            (259, Value::Short(vec![8])),
            (262, Value::Short(vec![34892])),
            (273, Value::Long(offsets)),
            (274, Value::Short(vec![1])),
            (277, Value::Short(vec![3])),
            (278, Value::Long(vec![rows_per_strip as u32])),
            (279, Value::Long(counts)),
            (284, Value::Short(vec![1])),
            (305, Value::Ascii("pentool".into())),
            (317, Value::Short(vec![34894])),
            (339, Value::Short(vec![3; 3])),
            (50706, Value::Byte(vec![1, 4, 0, 0])),
            (50707, Value::Byte(vec![1, 4, 0, 0])),
            (50708, Value::Ascii(self.model.into())),
            (50721, Value::Srational(matrix)),
            (50728, Value::Rational(vec![(1, 1); 3])),
            (50730, Value::Srational(vec![(stops, 1)])),
            (50778, Value::Short(vec![23])),
        ];
        if let Some(mask) = mask {
            entries.push((330, Value::Long(vec![mask])));
        }
        let ifd0 = write_ifd(&mut out, entries)?;
        out[4..8].copy_from_slice(&ifd0.to_le_bytes());
        Ok(out)
    }
}

/// The transparency mask of a DNG pentool wrote, cropped like the decoded
/// image (`dng.active` then `dng.crop`), as coverage in 0–1. `None` when the
/// DNG has no mask.
pub fn transparency(dng: &Dng) -> Result<Option<Vec<f32>>> {
    let tiff = &dng.tiff;
    let Some(ifd) = tiff
        .ifds
        .iter()
        .find(|ifd| matches!(ifd.uint(tiff, 254), Ok(Some(4))))
    else {
        return Ok(None);
    };
    let what = "[malformed-resource] the DNG transparency mask";
    let width = ifd.uint(tiff, 256)?.unwrap_or(0) as usize;
    let height = ifd.uint(tiff, 257)?.unwrap_or(0) as usize;
    if (width, height) != (dng.width, dng.height) {
        bail!(
            "{what} is {width}x{height}; the raw image is {}x{}",
            dng.width,
            dng.height
        )
    }
    if ifd.uint(tiff, 258)?.unwrap_or(1) != 8 || ifd.uint(tiff, 277)?.unwrap_or(1) != 1 {
        bail!("[unsupported-capability] the DNG transparency mask must hold one 8-bit sample")
    }
    let compression = ifd.uint(tiff, 259)?.unwrap_or(1);
    if !matches!(compression, 1 | 8) || ifd.uint(tiff, 317)?.unwrap_or(1) != 1 {
        bail!("[unsupported-capability] the DNG transparency mask must be uncompressed or deflate without a predictor")
    }
    let rows = (ifd.uint(tiff, 278)?.unwrap_or(u32::MAX) as usize).clamp(1, height);
    let offsets = ifd
        .uints(tiff, 273, super::dng::MAX_SEGMENTS)?
        .unwrap_or_default();
    let counts = ifd
        .uints(tiff, 279, super::dng::MAX_SEGMENTS)?
        .unwrap_or_default();
    let strips = height.div_ceil(rows);
    if offsets.len() != strips || counts.len() != strips {
        bail!("{what} needs {strips} strips")
    }
    let mut plane = Vec::with_capacity(width * height);
    for (index, (at, count)) in offsets.iter().zip(&counts).enumerate() {
        let (at, count) = (*at as usize, *count as usize);
        let data = tiff
            .bytes
            .get(at..at.saturating_add(count))
            .with_context(|| format!("{what} strip {index} lies outside the file"))?;
        let expected = width * rows.min(height - index * rows);
        let bytes = if compression == 8 {
            let mut inflated = Vec::with_capacity(expected);
            flate2::read::ZlibDecoder::new(data)
                .take(expected as u64 + 1)
                .read_to_end(&mut inflated)
                .map_err(|e| anyhow::anyhow!("{what} strip {index} is damaged: {e}"))?;
            inflated
        } else {
            data.to_vec()
        };
        if bytes.len() < expected {
            bail!(
                "{what} strip {index} holds {} of {expected} bytes",
                bytes.len()
            )
        }
        plane.extend(bytes[..expected].iter().map(|v| f32::from(*v) / 255.0));
    }
    let (active, crop) = (dng.active, dng.crop);
    let mut out = Vec::with_capacity(crop.width() * crop.height());
    for y in crop.top..crop.bottom {
        let row = (active.top + y) * width + active.left + crop.left;
        out.extend_from_slice(&plane[row..row + crop.width()]);
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_precision_rounds_to_nearest_even() {
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1.0e6), 0x7bff);
        assert_eq!(f16_bits(0.0), 0);
        // The smallest subnormal half.
        assert_eq!(f16_bits(5.960_464_5e-8), 1);
        // Halfway between 1 and the next half rounds to even (1).
        assert_eq!(f16_bits(1.0 + 1.0 / 2048.0), 0x3c00);
        assert_eq!(f16_bits(1.0 + 3.0 / 2048.0), 0x3c02);
    }

    #[test]
    fn written_dngs_decode_to_the_same_working_rgb() {
        let (w, h) = (7, 5);
        let rgb: Vec<f32> = (0..w * h * 3)
            .map(|i| 0.01 + (i % 17) as f32 * 0.37)
            .collect();
        let alpha: Vec<f32> = (0..w * h)
            .map(|i| if i % 4 == 0 { 0.0 } else { 1.0 })
            .collect();
        let bytes = LinearDng {
            width: w,
            height: h,
            rgb: &rgb,
            alpha: Some(&alpha),
            model: "Pentool Test Merge",
        }
        .write()
        .unwrap();
        let dng = Dng::inspect(&bytes).unwrap();
        assert_eq!((dng.width, dng.height), (w, h));
        let stops = baseline_exposure(&rgb);
        assert_eq!(dng.baseline_exposure, Some(f64::from(stops)));
        let develop = serde_json::json!({});
        let none = |_: &str| -> Result<(Vec<u8>, serde_json::Value)> { bail!("no profiles") };
        let (decoded, _) = super::super::pipeline::decode_working(&dng, &develop, &none).unwrap();
        let gain = 2.0f32.powi(stops);
        for (a, b) in decoded.iter().zip(&rgb) {
            assert!((a * gain - b).abs() <= 2e-3 * b.max(0.05), "{a} vs {b}");
        }
        let mask = transparency(&dng).unwrap().unwrap();
        assert_eq!(mask, alpha);
    }
}

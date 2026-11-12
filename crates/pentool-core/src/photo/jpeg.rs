//! A deterministic baseline JPEG encoder (ITU-T T.81) with 4:2:0 or 4:4:4
//! chroma, the Annex K tables scaled by quality, a JFIF density and ICC_PROFILE
//! segments. The bundled encoder cannot subsample chroma.
use anyhow::{bail, Context, Result};

/// Table K.1, natural order.
#[rustfmt::skip]
const LUMA_Q: [u8; 64] = [16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,];

/// Table K.2, natural order.
#[rustfmt::skip]
const CHROMA_Q: [u8; 64] = [17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,];

/// Table K.3 code lengths.
#[rustfmt::skip]
const LUMA_DC_LENGTHS: [u8; 16] = [0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,];

/// DC symbols of tables K.3 and K.4.
#[rustfmt::skip]
const DC_VALUES: [u8; 12] = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,];

/// Table K.4 code lengths.
#[rustfmt::skip]
const CHROMA_DC_LENGTHS: [u8; 16] = [0x00, 0x03, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,];

/// Table K.5 code lengths.
#[rustfmt::skip]
const LUMA_AC_LENGTHS: [u8; 16] = [0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7D,];

/// Table K.5 symbols.
#[rustfmt::skip]
const LUMA_AC_VALUES: [u8; 162] = [0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,];

/// Table K.6 code lengths.
#[rustfmt::skip]
const CHROMA_AC_LENGTHS: [u8; 16] = [0x00, 0x02, 0x01, 0x02, 0x04, 0x04, 0x03, 0x04, 0x07, 0x05, 0x04, 0x04, 0x00, 0x01, 0x02, 0x77,];

/// Table K.6 symbols.
#[rustfmt::skip]
const CHROMA_AC_VALUES: [u8; 162] = [0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0, 0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,];

/// Natural index of each zigzag position.
#[rustfmt::skip]
const ZIGZAG: [u8; 64] = [0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,];

/// Chroma subsampling of a JPEG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chroma {
    /// Cb and Cr at half resolution in both directions.
    Sub420,
    /// Full-resolution chroma.
    Full444,
}

impl Chroma {
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "420" => Ok(Self::Sub420),
            "444" => Ok(Self::Full444),
            _ => bail!("[invalid-input] JPEG chroma {name:?} must be 420 or 444"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sub420 => "420",
            Self::Full444 => "444",
        }
    }
}

/// `cos(k π / 16)` for k = 0..8, written out so every platform agrees.
const COS: [f64; 9] = [
    1.0,
    0.980_785_280_403_230_4,
    0.923_879_532_511_286_7,
    0.831_469_612_302_545_2,
    std::f64::consts::FRAC_1_SQRT_2,
    0.555_570_233_019_602_2,
    0.382_683_432_365_089_8,
    0.195_090_322_016_128_25,
    0.0,
];

/// `cos(k π / 16)` for any k.
fn cos16(k: usize) -> f64 {
    let k = k % 32;
    match k {
        0..=8 => COS[k],
        9..=16 => -COS[16 - k],
        17..=24 => -COS[k - 16],
        _ => COS[32 - k],
    }
}

/// `basis[u][x] = C(u) / 2 · cos((2x + 1) u π / 16)`.
fn basis() -> [[f64; 8]; 8] {
    let mut table = [[0.0; 8]; 8];
    for (u, row) in table.iter_mut().enumerate() {
        let c = if u == 0 { COS[4] } else { 1.0 };
        for (x, value) in row.iter_mut().enumerate() {
            *value = c / 2.0 * cos16((2 * x + 1) * u);
        }
    }
    table
}

/// Quantization tables scaled for `quality` (1–100) as libjpeg does.
fn tables(quality: u8) -> [[u16; 64]; 2] {
    let q = u32::from(quality.clamp(1, 100));
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    [LUMA_Q, CHROMA_Q]
        .map(|table| table.map(|v| ((u32::from(v) * scale + 50) / 100).clamp(1, 255) as u16))
}

/// A Huffman table as (code, length) by symbol.
struct Huffman([(u16, u8); 256]);

impl Huffman {
    fn new(lengths: &[u8; 16], values: &[u8]) -> Self {
        let mut table = [(0u16, 0u8); 256];
        let mut code = 0u16;
        let mut k = 0;
        for (i, count) in lengths.iter().enumerate() {
            for _ in 0..*count {
                table[usize::from(values[k])] = (code, i as u8 + 1);
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        Self(table)
    }
}

struct Bits {
    out: Vec<u8>,
    buffer: u32,
    count: u32,
}

impl Bits {
    fn put(&mut self, code: u16, length: u8) {
        self.buffer = (self.buffer << length) | u32::from(code);
        self.count += u32::from(length);
        while self.count >= 8 {
            let byte = (self.buffer >> (self.count - 8)) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0);
            }
            self.count -= 8;
            self.buffer &= (1 << self.count) - 1;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            let pad = 8 - self.count as u8;
            self.put((1 << pad) - 1, pad);
        }
        self.out
    }
}

/// Bit category and the low bits of a coefficient.
fn category(value: i32) -> (u8, u16) {
    let magnitude = value.unsigned_abs();
    let size = (32 - magnitude.leading_zeros()) as u8;
    let bits = if value < 0 {
        (value - 1) as u32 & ((1 << size) - 1)
    } else {
        value as u32
    };
    (size, bits as u16)
}

struct Encoder {
    basis: [[f64; 8]; 8],
    quant: [[u16; 64]; 2],
    dc: [Huffman; 2],
    ac: [Huffman; 2],
    bits: Bits,
    previous: [i32; 3],
}

impl Encoder {
    /// Transform, quantize and entropy-code one 8x8 block of level-shifted samples.
    fn block(&mut self, samples: &[f64; 64], component: usize) {
        let table = usize::from(component > 0);
        let mut rows = [0.0f64; 64];
        for y in 0..8 {
            for u in 0..8 {
                rows[y * 8 + u] = (0..8).map(|x| self.basis[u][x] * samples[y * 8 + x]).sum();
            }
        }
        let mut quantized = [0i32; 64];
        for (zigzag, &index) in ZIGZAG.iter().enumerate() {
            let (v, u) = (usize::from(index) / 8, usize::from(index) % 8);
            let coefficient: f64 = (0..8).map(|y| self.basis[v][y] * rows[y * 8 + u]).sum();
            let q = f64::from(self.quant[table][usize::from(index)]);
            quantized[zigzag] = (coefficient / q).round() as i32;
        }
        let diff = quantized[0] - self.previous[component];
        self.previous[component] = quantized[0];
        let (size, bits) = category(diff);
        let (code, length) = self.dc[table].0[usize::from(size)];
        self.bits.put(code, length);
        if size > 0 {
            self.bits.put(bits, size);
        }
        let mut run = 0;
        for &value in &quantized[1..] {
            if value == 0 {
                run += 1;
                continue;
            }
            while run > 15 {
                let (code, length) = self.ac[table].0[0xF0];
                self.bits.put(code, length);
                run -= 16;
            }
            let (size, bits) = category(value);
            let (code, length) = self.ac[table].0[(run << 4) | usize::from(size)];
            self.bits.put(code, length);
            self.bits.put(bits, size);
            run = 0;
        }
        if run > 0 {
            let (code, length) = self.ac[table].0[0x00];
            self.bits.put(code, length);
        }
    }
}

fn segment(out: &mut Vec<u8>, marker: u8, body: &[u8]) -> Result<()> {
    let length = u16::try_from(body.len() + 2)
        .ok()
        .context("[limit-exceeded] JPEG segment is longer than 65,533 bytes")?;
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(body);
    Ok(())
}

fn huffman_segment(
    out: &mut Vec<u8>,
    class_id: u8,
    lengths: &[u8; 16],
    values: &[u8],
) -> Result<()> {
    let mut body = vec![class_id];
    body.extend_from_slice(lengths);
    body.extend_from_slice(values);
    segment(out, 0xC4, &body)
}

/// A baseline JPEG of 8-bit RGB samples, with an ICC profile and a JFIF density.
pub fn encode(
    width: u32,
    height: u32,
    rgb: &[u8],
    quality: u8,
    chroma: Chroma,
    ppi: u16,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
        bail!("[limit-exceeded] JPEG output is {width}x{height}; each side must be 1–65,535 pixels")
    }
    if rgb.len() != width as usize * height as usize * 3 {
        bail!("[malformed-resource] JPEG input needs {width}x{height} RGB samples")
    }
    if !(1..=100).contains(&quality) {
        bail!("[invalid-input] JPEG quality {quality} must be 1–100")
    }
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0xFF, 0xD8];
    let mut jfif = b"JFIF\0\x01\x01\x01".to_vec();
    jfif.extend_from_slice(&ppi.max(1).to_be_bytes());
    jfif.extend_from_slice(&ppi.max(1).to_be_bytes());
    jfif.extend_from_slice(&[0, 0]);
    segment(&mut out, 0xE0, &jfif)?;
    if let Some(icc) = icc {
        let chunks: Vec<&[u8]> = icc.chunks(65_519).collect();
        if chunks.len() > 255 {
            bail!("[limit-exceeded] ICC profile is too large for JPEG")
        }
        for (i, chunk) in chunks.iter().enumerate() {
            let mut body = b"ICC_PROFILE\0".to_vec();
            body.push(i as u8 + 1);
            body.push(chunks.len() as u8);
            body.extend_from_slice(chunk);
            segment(&mut out, 0xE2, &body)?;
        }
    }
    let quant = tables(quality);
    for (id, table) in quant.iter().enumerate() {
        let mut body = vec![id as u8];
        body.extend(ZIGZAG.iter().map(|&i| table[usize::from(i)] as u8));
        segment(&mut out, 0xDB, &body)?;
    }
    let sampling = match chroma {
        Chroma::Sub420 => 0x22,
        Chroma::Full444 => 0x11,
    };
    let mut frame = vec![8];
    frame.extend_from_slice(&(height as u16).to_be_bytes());
    frame.extend_from_slice(&(width as u16).to_be_bytes());
    frame.extend_from_slice(&[3, 1, sampling, 0, 2, 0x11, 1, 3, 0x11, 1]);
    segment(&mut out, 0xC0, &frame)?;
    huffman_segment(&mut out, 0x00, &LUMA_DC_LENGTHS, &DC_VALUES)?;
    huffman_segment(&mut out, 0x10, &LUMA_AC_LENGTHS, &LUMA_AC_VALUES)?;
    huffman_segment(&mut out, 0x01, &CHROMA_DC_LENGTHS, &DC_VALUES)?;
    huffman_segment(&mut out, 0x11, &CHROMA_AC_LENGTHS, &CHROMA_AC_VALUES)?;
    segment(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0])?;

    // YCbCr (BT.601 full range, level-shifted), edges replicated.
    let ycc = |x: usize, y: usize| -> [f64; 3] {
        let i = (y.min(h - 1) * w + x.min(w - 1)) * 3;
        let [r, g, b] = [rgb[i], rgb[i + 1], rgb[i + 2]].map(f64::from);
        [
            0.299 * r + 0.587 * g + 0.114 * b - 128.0,
            -0.168_736 * r - 0.331_264 * g + 0.5 * b,
            0.5 * r - 0.418_688 * g - 0.081_312 * b,
        ]
    };
    let mut encoder = Encoder {
        basis: basis(),
        quant,
        dc: [
            Huffman::new(&LUMA_DC_LENGTHS, &DC_VALUES),
            Huffman::new(&CHROMA_DC_LENGTHS, &DC_VALUES),
        ],
        ac: [
            Huffman::new(&LUMA_AC_LENGTHS, &LUMA_AC_VALUES),
            Huffman::new(&CHROMA_AC_LENGTHS, &CHROMA_AC_VALUES),
        ],
        bits: Bits {
            out: Vec::new(),
            buffer: 0,
            count: 0,
        },
        previous: [0; 3],
    };
    let mcu = if chroma == Chroma::Sub420 { 16 } else { 8 };
    let mut block = [0.0f64; 64];
    for my in (0..h).step_by(mcu) {
        if my % 64 == 0 {
            super::check_cancelled()?;
        }
        for mx in (0..w).step_by(mcu) {
            for by in (0..mcu).step_by(8) {
                for bx in (0..mcu).step_by(8) {
                    for (i, sample) in block.iter_mut().enumerate() {
                        *sample = ycc(mx + bx + i % 8, my + by + i / 8)[0];
                    }
                    encoder.block(&block, 0);
                }
            }
            for component in 1..3 {
                for (i, sample) in block.iter_mut().enumerate() {
                    let (x, y) = (i % 8, i / 8);
                    *sample = if mcu == 16 {
                        let (sx, sy) = (mx + 2 * x, my + 2 * y);
                        (ycc(sx, sy)[component]
                            + ycc(sx + 1, sy)[component]
                            + ycc(sx, sy + 1)[component]
                            + ycc(sx + 1, sy + 1)[component])
                            / 4.0
                    } else {
                        ycc(mx + x, my + y)[component]
                    };
                }
                encoder.block(&block, component);
            }
        }
    }
    out.extend(encoder.bits.finish());
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend([
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x + y) % 256) as u8,
                ]);
            }
        }
        rgb
    }

    #[test]
    fn baseline_jpegs_decode_close_to_the_input() {
        for (w, h) in [(1, 1), (17, 9), (64, 40)] {
            let rgb = gradient(w, h);
            for chroma in [Chroma::Sub420, Chroma::Full444] {
                let bytes = encode(w, h, &rgb, 95, chroma, 300, None).unwrap();
                assert_eq!(bytes, encode(w, h, &rgb, 95, chroma, 300, None).unwrap());
                let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
                assert_eq!(decoded.dimensions(), (w, h));
                let error: f64 = decoded
                    .as_raw()
                    .iter()
                    .zip(&rgb)
                    .map(|(a, b)| (f64::from(*a) - f64::from(*b)).abs())
                    .sum::<f64>()
                    / rgb.len() as f64;
                assert!(error < 6.0, "{w}x{h} {chroma:?}: mean error {error}");
            }
        }
    }

    #[test]
    fn subsampling_quality_and_icc_are_written() {
        let rgb = gradient(32, 32);
        let sub = encode(32, 32, &rgb, 85, Chroma::Sub420, 72, Some(b"profile")).unwrap();
        let full = encode(32, 32, &rgb, 85, Chroma::Full444, 72, None).unwrap();
        let sof = |b: &[u8]| {
            let at = b.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
            b[at + 11]
        };
        assert_eq!((sof(&sub), sof(&full)), (0x22, 0x11));
        assert!(sub.windows(12).any(|w| w == b"ICC_PROFILE\0"));
        let low = encode(32, 32, &rgb, 10, Chroma::Sub420, 72, None).unwrap();
        assert!(low.len() < sub.len());
        assert!(encode(32, 32, &rgb, 0, Chroma::Sub420, 72, None).is_err());
        assert!(encode(0, 32, &[], 50, Chroma::Sub420, 72, None).is_err());
    }
}

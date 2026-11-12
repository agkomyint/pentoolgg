//! Lossless JPEG (ITU T.81 process 14, SOF3) for DNG compression 7.
//!
//! Supports 2–16 bit precision, 1–4 components without subsampling, Huffman
//! tables, predictors 1–7, point transform and restart markers. The caller
//! states the exact number of samples it expects; any other size is refused.
use anyhow::{bail, Result};

#[derive(Default, Clone)]
struct Table {
    /// `(code length, code) -> symbol`, as canonical first-code tables.
    max_code: [i32; 18],
    val_offset: [i32; 18],
    values: Vec<u8>,
    present: bool,
}

impl Table {
    fn build(counts: &[u8; 16], values: Vec<u8>) -> Self {
        let mut table = Self {
            max_code: [-1; 18],
            val_offset: [0; 18],
            values,
            present: true,
        };
        let mut code = 0i32;
        let mut k = 0i32;
        for length in 1..=16 {
            let n = i32::from(counts[length - 1]);
            if n > 0 {
                table.val_offset[length] = k - code;
                code += n;
                k += n;
                table.max_code[length] = code - 1;
            }
            code <<= 1;
        }
        table.max_code[17] = i32::MAX;
        table
    }
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    buffer: u32,
    count: u32,
    /// A marker was reached; further reads yield zero bits.
    marker: bool,
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32> {
        if self.count == 0 {
            let byte = if self.marker || self.at >= self.data.len() {
                if self.at >= self.data.len() && !self.marker {
                    bail!("[malformed-resource] lossless JPEG scan ends early");
                }
                0
            } else {
                let byte = self.data[self.at];
                self.at += 1;
                if byte == 0xff {
                    match self.data.get(self.at) {
                        Some(0) => self.at += 1,
                        Some(_) => {
                            self.marker = true;
                            self.at -= 1;
                            return Ok(0);
                        }
                        None => bail!("[malformed-resource] lossless JPEG scan ends early"),
                    }
                }
                byte
            };
            self.buffer = u32::from(byte);
            self.count = 8;
        }
        self.count -= 1;
        Ok((self.buffer >> self.count) & 1)
    }

    fn bits(&mut self, n: u32) -> Result<u32> {
        let mut value = 0;
        for _ in 0..n {
            value = (value << 1) | self.bit()?;
        }
        Ok(value)
    }

    fn symbol(&mut self, table: &Table) -> Result<u8> {
        let mut code = 0i32;
        for length in 1..=16 {
            code = (code << 1) | self.bit()? as i32;
            if code <= table.max_code[length] {
                let index = (code + table.val_offset[length]) as usize;
                return table.values.get(index).copied().ok_or_else(|| {
                    anyhow::anyhow!("[malformed-resource] lossless JPEG Huffman code has no value")
                });
            }
        }
        bail!("[malformed-resource] lossless JPEG Huffman code longer than 16 bits")
    }

    /// Skip to the next RSTn marker after a restart interval.
    fn restart(&mut self) -> Result<()> {
        self.count = 0;
        self.marker = false;
        while self.at + 1 < self.data.len() {
            if self.data[self.at] == 0xff && (0xd0..=0xd7).contains(&self.data[self.at + 1]) {
                self.at += 2;
                return Ok(());
            }
            self.at += 1;
        }
        bail!("[malformed-resource] lossless JPEG restart marker is missing")
    }
}

/// A decoded frame: `width * components` samples per row, `height` rows.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub components: usize,
    pub precision: u32,
    pub samples: Vec<u16>,
}

/// Decode a lossless JPEG stream that must hold exactly `expected` samples.
pub fn decode(data: &[u8], expected: usize) -> Result<Frame> {
    if !data.starts_with(&[0xff, 0xd8]) {
        bail!("[malformed-resource] lossless JPEG tile does not start with SOI");
    }
    let mut at = 2;
    let mut tables: [Table; 4] = Default::default();
    let mut frame: Option<(u32, usize, usize, Vec<u8>)> = None;
    let mut restart_interval = 0usize;
    loop {
        if at + 4 > data.len() || data[at] != 0xff {
            bail!("[malformed-resource] lossless JPEG marker expected at byte {at}");
        }
        let marker = data[at + 1];
        if marker == 0xff {
            at += 1;
            continue;
        }
        let length = usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
        let body = data.get(at + 4..at + 2 + length).ok_or_else(|| {
            anyhow::anyhow!("[malformed-resource] lossless JPEG segment overruns the tile")
        })?;
        if length < 2 {
            bail!("[malformed-resource] lossless JPEG segment length {length} is invalid");
        }
        match marker {
            0xc3 => {
                if body.len() < 6 {
                    bail!("[malformed-resource] lossless JPEG SOF3 is truncated");
                }
                let precision = u32::from(body[0]);
                let height = usize::from(u16::from_be_bytes([body[1], body[2]]));
                let width = usize::from(u16::from_be_bytes([body[3], body[4]]));
                let components = usize::from(body[5]);
                if !(2..=16).contains(&precision) || !(1..=4).contains(&components) {
                    bail!("[unsupported-capability] lossless JPEG with {precision}-bit precision and {components} components");
                }
                if body.len() < 6 + 3 * components {
                    bail!("[malformed-resource] lossless JPEG SOF3 is truncated");
                }
                let ids = (0..components).map(|c| body[6 + 3 * c]).collect();
                for c in 0..components {
                    if body[7 + 3 * c] != 0x11 {
                        bail!("[unsupported-capability] subsampled lossless JPEG components");
                    }
                }
                if width == 0
                    || height == 0
                    || width.saturating_mul(height).saturating_mul(components) != expected
                {
                    bail!(
                        "[malformed-resource] lossless JPEG frame {width}x{height}x{components} does not match the {expected} samples the tile needs"
                    );
                }
                frame = Some((precision, width, height, ids));
            }
            0xc0..=0xc2 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                bail!("[unsupported-capability] JPEG process SOF{} in a DNG tile; only lossless SOF3 is supported", marker - 0xc0)
            }
            0xc4 => {
                let mut rest = body;
                while !rest.is_empty() {
                    if rest.len() < 17 {
                        bail!("[malformed-resource] lossless JPEG DHT is truncated");
                    }
                    let (class, slot) = (rest[0] >> 4, usize::from(rest[0] & 15));
                    if class != 0 || slot > 3 {
                        bail!("[malformed-resource] lossless JPEG DHT names table {class}/{slot}");
                    }
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&rest[1..17]);
                    let total: usize = counts.iter().map(|n| usize::from(*n)).sum();
                    let values = rest.get(17..17 + total).ok_or_else(|| {
                        anyhow::anyhow!("[malformed-resource] lossless JPEG DHT is truncated")
                    })?;
                    tables[slot] = Table::build(&counts, values.to_vec());
                    rest = &rest[17 + total..];
                }
            }
            0xdd => {
                if body.len() < 2 {
                    bail!("[malformed-resource] lossless JPEG DRI is truncated");
                }
                restart_interval = usize::from(u16::from_be_bytes([body[0], body[1]]));
            }
            0xda => {
                let Some((precision, width, height, ids)) = frame.take() else {
                    bail!("[malformed-resource] lossless JPEG SOS before SOF3");
                };
                let count = usize::from(*body.first().unwrap_or(&0));
                if count != ids.len() || body.len() < 1 + 2 * count + 3 {
                    bail!("[unsupported-capability] lossless JPEG scans must hold every component");
                }
                let mut selected = Vec::with_capacity(count);
                for c in 0..count {
                    if body[1 + 2 * c] != ids[c] {
                        bail!("[unsupported-capability] lossless JPEG scan component order differs from the frame");
                    }
                    let slot = usize::from(body[2 + 2 * c] >> 4);
                    if slot > 3 || !tables[slot].present {
                        bail!(
                            "[malformed-resource] lossless JPEG scan uses undefined table {slot}"
                        );
                    }
                    selected.push(slot);
                }
                let predictor = u32::from(body[1 + 2 * count]);
                let transform = u32::from(body[3 + 2 * count] & 15);
                if !(1..=7).contains(&predictor) || transform >= precision {
                    bail!("[malformed-resource] lossless JPEG predictor {predictor} or point transform {transform} is invalid");
                }
                let scan = &data[at + 2 + length..];
                let samples = scan_samples(
                    scan,
                    &tables,
                    &selected,
                    width,
                    height,
                    precision,
                    predictor,
                    transform,
                    restart_interval,
                )?;
                return Ok(Frame {
                    width,
                    height,
                    components: count,
                    precision,
                    samples,
                });
            }
            0xd9 => bail!("[malformed-resource] lossless JPEG ends before a scan"),
            _ => {} // APPn, COM, DQT and others carry nothing a DNG tile needs.
        }
        at += 2 + length;
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_samples(
    scan: &[u8],
    tables: &[Table; 4],
    selected: &[usize],
    width: usize,
    height: usize,
    precision: u32,
    predictor: u32,
    transform: u32,
    restart_interval: usize,
) -> Result<Vec<u16>> {
    let components = selected.len();
    let row_len = width * components;
    let mut out = vec![0u16; row_len * height];
    let mut bits = Bits {
        data: scan,
        at: 0,
        buffer: 0,
        count: 0,
        marker: false,
    };
    let mask = (1u32 << precision) - 1;
    let initial = 1i32 << (precision - transform - 1);
    let mut until_restart = restart_interval;
    // A restart interval begins like the frame: its first sample is predicted
    // from `initial` and the rest of that row from the left neighbor.
    let (mut fresh_row, mut fresh_x) = (0usize, 0usize);
    for y in 0..height {
        for x in 0..width {
            if restart_interval > 0 && y * width + x > 0 && until_restart == 0 {
                bits.restart()?;
                until_restart = restart_interval;
                (fresh_row, fresh_x) = (y, x);
            }
            for (c, slot) in selected.iter().enumerate() {
                let length = u32::from(bits.symbol(&tables[*slot])?);
                let diff = match length {
                    0 => 0,
                    16 => 32_768,
                    1..=15 => {
                        let v = bits.bits(length)? as i32;
                        if v < 1 << (length - 1) {
                            v - (1 << length) + 1
                        } else {
                            v
                        }
                    }
                    _ => bail!("[malformed-resource] lossless JPEG difference category {length}"),
                };
                let at = y * row_len + x * components + c;
                let left = || i32::from(out[at - components]);
                let above = || i32::from(out[at - row_len]);
                let prediction = if (y, x) == (fresh_row, fresh_x) {
                    initial
                } else if y == fresh_row {
                    left()
                } else if x == 0 {
                    above()
                } else {
                    let (a, b, d) = (left(), above(), i32::from(out[at - row_len - components]));
                    match predictor {
                        1 => a,
                        2 => b,
                        3 => d,
                        4 => a + b - d,
                        5 => a + ((b - d) >> 1),
                        6 => b + ((a - d) >> 1),
                        _ => (a + b) / 2,
                    }
                };
                out[at] = ((prediction + diff) as u32 & mask) as u16;
            }
            until_restart = until_restart.saturating_sub(1);
        }
        crate::photo::check_cancelled()?;
    }
    if transform > 0 {
        for value in &mut out {
            *value <<= transform;
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Encode samples as lossless JPEG with predictor 1 and one fixed table
    /// (difference categories 0–16, 5-bit codes) for round-trip tests.
    pub(crate) fn encode(
        width: usize,
        height: usize,
        components: usize,
        samples: &[u16],
    ) -> Vec<u8> {
        let mut out = vec![0xff, 0xd8];
        // DHT: 17 symbols, all with 5-bit codes.
        let mut counts = [0u8; 16];
        counts[4] = 17;
        out.extend([0xff, 0xc4]);
        out.extend(((2 + 1 + 16 + 17) as u16).to_be_bytes());
        out.push(0x00);
        out.extend(counts);
        out.extend(0u8..17);
        // SOF3: 16-bit precision.
        out.extend([0xff, 0xc3]);
        out.extend(((8 + 3 * components) as u16).to_be_bytes());
        out.push(16);
        out.extend((height as u16).to_be_bytes());
        out.extend((width as u16).to_be_bytes());
        out.push(components as u8);
        for c in 0..components {
            out.extend([c as u8 + 1, 0x11, 0]);
        }
        // SOS: all components, table 0, predictor 1, no point transform.
        out.extend([0xff, 0xda]);
        out.extend(((6 + 2 * components) as u16).to_be_bytes());
        out.push(components as u8);
        for c in 0..components {
            out.extend([c as u8 + 1, 0x00]);
        }
        out.extend([1, 0, 0]);
        let mut writer = (0u64, 0u32, Vec::new());
        let put = |value: u32, n: u32, w: &mut (u64, u32, Vec<u8>)| {
            for i in (0..n).rev() {
                w.0 = (w.0 << 1) | u64::from((value >> i) & 1);
                w.1 += 1;
                if w.1 == 8 {
                    let byte = w.0 as u8;
                    w.2.push(byte);
                    if byte == 0xff {
                        w.2.push(0);
                    }
                    w.0 = 0;
                    w.1 = 0;
                }
            }
        };
        let row_len = width * components;
        for y in 0..height {
            for x in 0..width {
                for c in 0..components {
                    let at = y * row_len + x * components + c;
                    let prediction = if y == 0 && x == 0 {
                        1 << 15
                    } else if x == 0 {
                        i32::from(samples[at - row_len])
                    } else {
                        i32::from(samples[at - components])
                    };
                    let diff = (i32::from(samples[at]) - prediction) as i16 as i32;
                    let (category, extra) = if diff == 0 {
                        (0, 0)
                    } else if diff == -32_768 {
                        (16, 0)
                    } else {
                        let category = 32 - diff.unsigned_abs().leading_zeros();
                        let extra = if diff < 0 {
                            (diff + (1 << category) - 1) as u32
                        } else {
                            diff as u32
                        };
                        (category, extra)
                    };
                    put(category, 5, &mut writer);
                    if (1..16).contains(&category) {
                        put(extra, category, &mut writer);
                    }
                }
            }
        }
        while writer.1 != 0 {
            put(1, 1, &mut writer);
        }
        out.extend(writer.2);
        out.extend([0xff, 0xd9]);
        out
    }

    #[test]
    fn lossless_jpeg_round_trips_sixteen_bit_samples() {
        // Extremes force category-16 and wrapped differences.
        let samples: Vec<u16> = (0..6 * 5 * 2)
            .map(|i| match i % 5 {
                0 => 0,
                1 => 65_535,
                _ => ((i * 7919) % 65_536) as u16,
            })
            .collect();
        let encoded = encode(6, 5, 2, &samples);
        let frame = decode(&encoded, 60).unwrap();
        assert_eq!((frame.width, frame.height, frame.components), (6, 5, 2));
        assert_eq!(frame.samples, samples);
        let error = decode(&encoded, 61).err().unwrap().to_string();
        assert!(error.starts_with("[malformed-resource]"), "{error}");
        let truncated = &encoded[..encoded.len() / 2];
        assert!(decode(truncated, 60).is_err());
    }
}

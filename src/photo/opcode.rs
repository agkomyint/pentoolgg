//! DNG opcode lists (`OpcodeList1–3`): bounded parsing and the decode-stage
//! opcodes. `WarpRectilinear` and `FixVignetteRadial` are parsed here and applied
//! by the lens stage (stage 4), not during decode.
use anyhow::{bail, Result};

pub const MAX_LIST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_OPCODES: usize = 256;
pub const MAX_BAD_PIXELS: usize = 1_000_000;
pub const MAX_GAIN_POINTS: usize = 1_048_576;
pub const MAX_MAP_TABLE: usize = 65_536;
pub const MAX_POLYNOMIAL_DEGREE: u32 = 8;

/// The region an opcode touches. Coordinates are in the pixels of the image
/// the list applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
    pub plane: u32,
    pub planes: u32,
    pub row_pitch: u32,
    pub col_pitch: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GainMap {
    pub area: Area,
    pub points_v: usize,
    pub points_h: usize,
    pub spacing_v: f64,
    pub spacing_h: f64,
    pub origin_v: f64,
    pub origin_h: f64,
    pub map_planes: usize,
    /// `[v][h][plane]`.
    pub gains: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Per plane `[kr0, kr1, kr2, kr3, kt0, kt1]`, then the normalized center.
    WarpRectilinear {
        planes: Vec<[f64; 6]>,
        center: [f64; 2],
    },
    FixVignetteRadial {
        k: [f64; 5],
        center: [f64; 2],
    },
    FixBadPixelsConstant {
        constant: u32,
        phase: u32,
    },
    FixBadPixelsList {
        phase: u32,
        /// `(row, column)`.
        points: Vec<(u32, u32)>,
        /// `[top, left, bottom, right)`.
        rects: Vec<[u32; 4]>,
    },
    TrimBounds(Trim),
    MapTable {
        area: Area,
        table: Vec<u16>,
    },
    MapPolynomial {
        area: Area,
        coefficients: Vec<f64>,
    },
    GainMap(GainMap),
    /// An opcode engine 1 does not implement.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Opcode {
    pub list: u8,
    pub id: u32,
    pub optional: bool,
    pub preview: bool,
    pub op: Op,
}

impl Opcode {
    /// Whether engine 1 applies the opcode when developing. Preview-only
    /// opcodes are skipped for full-quality output.
    pub fn applied(&self) -> bool {
        !self.preview && !matches!(self.op, Op::Unknown)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    list: u8,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let slice = self
            .at
            .checked_add(n)
            .and_then(|end| self.bytes.get(self.at..end))
            .ok_or_else(|| {
                anyhow::anyhow!("[malformed-resource] OpcodeList{} is truncated", self.list)
            })?;
        self.at += n;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f64(&mut self) -> Result<f64> {
        let b = self.take(8)?;
        let value = f64::from_be_bytes(b.try_into().unwrap());
        if !value.is_finite() {
            bail!(
                "[malformed-resource] OpcodeList{} holds a value that is not finite",
                self.list
            );
        }
        Ok(value)
    }

    fn f32(&mut self) -> Result<f32> {
        let b = self.take(4)?;
        let value = f32::from_be_bytes(b.try_into().unwrap());
        if !value.is_finite() {
            bail!(
                "[malformed-resource] OpcodeList{} holds a value that is not finite",
                self.list
            );
        }
        Ok(value)
    }

    fn area(&mut self) -> Result<Area> {
        let area = Area {
            top: self.u32()?,
            left: self.u32()?,
            bottom: self.u32()?,
            right: self.u32()?,
            plane: self.u32()?,
            planes: self.u32()?,
            row_pitch: self.u32()?,
            col_pitch: self.u32()?,
        };
        if area.bottom < area.top
            || area.right < area.left
            || area.planes == 0
            || area.row_pitch == 0
            || area.col_pitch == 0
        {
            bail!(
                "[malformed-resource] OpcodeList{} has an invalid area {area:?}",
                self.list
            );
        }
        Ok(area)
    }
}

/// Parse one opcode list. `bytes` is the tag value (big-endian per DNG).
pub fn parse(list: u8, bytes: &[u8]) -> Result<Vec<Opcode>> {
    let mut reader = Reader { bytes, at: 0, list };
    let count = reader.u32()? as usize;
    if count > MAX_OPCODES {
        bail!("[limit-exceeded] OpcodeList{list} has {count} opcodes; the limit is {MAX_OPCODES}");
    }
    let mut opcodes = Vec::with_capacity(count);
    for _ in 0..count {
        let id = reader.u32()?;
        let _version = reader.u32()?;
        let flags = reader.u32()?;
        let size = reader.u32()? as usize;
        let params = reader.take(size)?;
        let mut p = Reader {
            bytes: params,
            at: 0,
            list,
        };
        let op = match id {
            1 => {
                let planes = p.u32()? as usize;
                if !(1..=3).contains(&planes) {
                    bail!("[malformed-resource] WarpRectilinear in OpcodeList{list} has {planes} planes");
                }
                let mut coefficients = Vec::with_capacity(planes);
                for _ in 0..planes {
                    let mut k = [0.0; 6];
                    for value in &mut k {
                        *value = p.f64()?;
                    }
                    coefficients.push(k);
                }
                Op::WarpRectilinear {
                    planes: coefficients,
                    center: [p.f64()?, p.f64()?],
                }
            }
            3 => {
                let mut k = [0.0; 5];
                for value in &mut k {
                    *value = p.f64()?;
                }
                Op::FixVignetteRadial {
                    k,
                    center: [p.f64()?, p.f64()?],
                }
            }
            4 => Op::FixBadPixelsConstant {
                constant: p.u32()?,
                phase: phase(p.u32()?, list)?,
            },
            5 => {
                let phase = phase(p.u32()?, list)?;
                let (points, rects) = (p.u32()? as usize, p.u32()? as usize);
                if points + rects > MAX_BAD_PIXELS {
                    bail!("[limit-exceeded] FixBadPixelsList in OpcodeList{list} has {} entries; the limit is {MAX_BAD_PIXELS}", points + rects);
                }
                if params.len() != 12 + points * 8 + rects * 16 {
                    bail!("[malformed-resource] FixBadPixelsList in OpcodeList{list} has {} parameter bytes for {points} points and {rects} rectangles", params.len());
                }
                let points = (0..points)
                    .map(|_| Ok((p.u32()?, p.u32()?)))
                    .collect::<Result<_>>()?;
                let rects = (0..rects)
                    .map(|_| Ok([p.u32()?, p.u32()?, p.u32()?, p.u32()?]))
                    .collect::<Result<_>>()?;
                Op::FixBadPixelsList {
                    phase,
                    points,
                    rects,
                }
            }
            6 => {
                let trim = Trim {
                    top: p.u32()?,
                    left: p.u32()?,
                    bottom: p.u32()?,
                    right: p.u32()?,
                };
                if trim.bottom <= trim.top || trim.right <= trim.left {
                    bail!("[malformed-resource] TrimBounds in OpcodeList{list} is empty");
                }
                Op::TrimBounds(trim)
            }
            7 => {
                let area = p.area()?;
                let size = p.u32()? as usize;
                if size == 0 || size > MAX_MAP_TABLE {
                    bail!("[limit-exceeded] MapTable in OpcodeList{list} has {size} entries; the limit is 1–{MAX_MAP_TABLE}");
                }
                let table = p
                    .take(size * 2)?
                    .chunks_exact(2)
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect();
                Op::MapTable { area, table }
            }
            8 => {
                let area = p.area()?;
                let degree = p.u32()?;
                if degree > MAX_POLYNOMIAL_DEGREE {
                    bail!("[malformed-resource] MapPolynomial in OpcodeList{list} has degree {degree}; the limit is {MAX_POLYNOMIAL_DEGREE}");
                }
                let coefficients = (0..=degree).map(|_| p.f64()).collect::<Result<_>>()?;
                Op::MapPolynomial { area, coefficients }
            }
            9 => {
                let area = p.area()?;
                let (points_v, points_h) = (p.u32()? as usize, p.u32()? as usize);
                let (spacing_v, spacing_h) = (p.f64()?, p.f64()?);
                let (origin_v, origin_h) = (p.f64()?, p.f64()?);
                let map_planes = p.u32()? as usize;
                let total = points_v
                    .checked_mul(points_h)
                    .and_then(|n| n.checked_mul(map_planes))
                    .unwrap_or(usize::MAX);
                if points_v == 0 || points_h == 0 || map_planes == 0 {
                    bail!("[malformed-resource] GainMap in OpcodeList{list} is empty");
                }
                if total > MAX_GAIN_POINTS {
                    bail!("[limit-exceeded] GainMap in OpcodeList{list} has {total} points; the limit is {MAX_GAIN_POINTS}");
                }
                if (points_v > 1 && spacing_v <= 0.0) || (points_h > 1 && spacing_h <= 0.0) {
                    bail!("[malformed-resource] GainMap in OpcodeList{list} has a spacing that is not positive");
                }
                let gains = (0..total).map(|_| p.f32()).collect::<Result<_>>()?;
                Op::GainMap(GainMap {
                    area,
                    points_v,
                    points_h,
                    spacing_v,
                    spacing_h,
                    origin_v,
                    origin_h,
                    map_planes,
                    gains,
                })
            }
            _ => Op::Unknown,
        };
        if !matches!(op, Op::Unknown) && p.at != params.len() {
            bail!(
                "[malformed-resource] opcode {id} in OpcodeList{list} has {} parameter bytes; it uses {}",
                params.len(),
                p.at
            );
        }
        opcodes.push(Opcode {
            list,
            id,
            optional: flags & 1 == 1,
            preview: flags & 2 == 2,
            op,
        });
    }
    if reader.at != bytes.len() {
        bail!("[malformed-resource] OpcodeList{list} has bytes after its last opcode");
    }
    Ok(opcodes)
}

fn phase(value: u32, list: u8) -> Result<u32> {
    if value > 3 {
        bail!("[malformed-resource] BayerPhase {value} in OpcodeList{list}; expected 0–3");
    }
    Ok(value)
}

/// An interleaved `f32` image an opcode list operates on.
#[derive(Debug, Clone, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub data: Vec<f32>,
}

/// How values are scaled at the point a list runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// List 1 on stored integer values: 0–65535 is the 16-bit range.
    Stored,
    /// Lists 2 and 3 on black/white-normalized values: 0–1.
    Normalized,
}

/// Apply the decode-stage opcodes of one list. Lens opcodes and `TrimBounds`
/// are left to later stages; unknown opcodes must have been rejected or be
/// optional.
pub fn apply(opcodes: &[Opcode], image: &mut Plane, stage: Stage) -> Result<()> {
    for opcode in opcodes.iter().filter(|op| op.applied()) {
        match &opcode.op {
            Op::MapTable { area, table } => each(image, area, |v| {
                let index = match stage {
                    Stage::Stored => v.round().clamp(0.0, 65_535.0) as usize,
                    Stage::Normalized => (v.clamp(0.0, 1.0) * 65_535.0).round() as usize,
                };
                let mapped = f32::from(table[index.min(table.len() - 1)]);
                match stage {
                    Stage::Stored => mapped,
                    Stage::Normalized => mapped / 65_535.0,
                }
            }),
            Op::MapPolynomial { area, coefficients } => each(image, area, |v| {
                let x = match stage {
                    Stage::Stored => f64::from(v) / 65_535.0,
                    Stage::Normalized => f64::from(v),
                };
                let y = coefficients.iter().rev().fold(0.0, |y, c| y * x + c);
                match stage {
                    Stage::Stored => (y.clamp(0.0, 1.0) * 65_535.0) as f32,
                    Stage::Normalized => y as f32,
                }
            }),
            Op::GainMap(map) => gain_map(image, map, stage),
            Op::FixBadPixelsConstant { constant, phase } => {
                let constant = *constant as f32;
                let bad: Vec<bool> = image
                    .data
                    .chunks_exact(image.channels)
                    .map(|p| p[0] == constant)
                    .collect();
                fix_bad(image, &bad, *phase);
            }
            Op::FixBadPixelsList {
                phase,
                points,
                rects,
            } => {
                let (w, h) = (image.width, image.height);
                let mut bad = vec![false; w * h];
                for (row, col) in points {
                    if (*row as usize) < h && (*col as usize) < w {
                        bad[*row as usize * w + *col as usize] = true;
                    }
                }
                for [top, left, bottom, right] in rects {
                    for row in (*top as usize)..(*bottom as usize).min(h) {
                        for col in (*left as usize)..(*right as usize).min(w) {
                            bad[row * w + col] = true;
                        }
                    }
                }
                fix_bad(image, &bad, *phase);
            }
            Op::TrimBounds(_)
            | Op::WarpRectilinear { .. }
            | Op::FixVignetteRadial { .. }
            | Op::Unknown => {}
        }
    }
    Ok(())
}

/// Visit every value an area selects.
fn each(image: &mut Plane, area: &Area, f: impl Fn(f32) -> f32) {
    let channels = image.channels;
    let bottom = (area.bottom as usize).min(image.height);
    let right = (area.right as usize).min(image.width);
    let last = (area.plane as usize + area.planes as usize).min(channels);
    for row in (area.top as usize..bottom).step_by(area.row_pitch as usize) {
        for col in (area.left as usize..right).step_by(area.col_pitch as usize) {
            let base = (row * image.width + col) * channels;
            for plane in area.plane as usize..last {
                let value = &mut image.data[base + plane];
                *value = f(*value);
            }
        }
    }
}

fn gain_map(image: &mut Plane, map: &GainMap, stage: Stage) {
    let area = map.area;
    let channels = image.channels;
    let (width, height) = (image.width as f64, image.height as f64);
    let bottom = (area.bottom as usize).min(image.height);
    let right = (area.right as usize).min(image.width);
    let last = (area.plane as usize + area.planes as usize).min(channels);
    // Map coordinates: pixel centers relative to the whole image, in map cells.
    let cell = |position: f64, origin: f64, spacing: f64, points: usize| -> (usize, usize, f64) {
        if points == 1 {
            return (0, 0, 0.0);
        }
        let t = ((position - origin) / spacing).clamp(0.0, (points - 1) as f64);
        let low = (t as usize).min(points - 2);
        (low, low + 1, t - low as f64)
    };
    let gain = |v: (usize, usize, f64), h: (usize, usize, f64), plane: usize| -> f64 {
        let at = |iv: usize, ih: usize| {
            f64::from(map.gains[(iv * map.points_h + ih) * map.map_planes + plane])
        };
        let top = at(v.0, h.0) * (1.0 - h.2) + at(v.0, h.1) * h.2;
        let bottom = at(v.1, h.0) * (1.0 - h.2) + at(v.1, h.1) * h.2;
        top * (1.0 - v.2) + bottom * v.2
    };
    for row in (area.top as usize..bottom).step_by(area.row_pitch as usize) {
        let v = cell(
            (row as f64 + 0.5) / height,
            map.origin_v,
            map.spacing_v,
            map.points_v,
        );
        for col in (area.left as usize..right).step_by(area.col_pitch as usize) {
            let h = cell(
                (col as f64 + 0.5) / width,
                map.origin_h,
                map.spacing_h,
                map.points_h,
            );
            let base = (row * image.width + col) * channels;
            for plane in area.plane as usize..last {
                let map_plane = (plane - area.plane as usize).min(map.map_planes - 1);
                let value = f64::from(image.data[base + plane]) * gain(v, h, map_plane);
                image.data[base + plane] = match stage {
                    Stage::Stored => value.clamp(0.0, 65_535.0) as f32,
                    Stage::Normalized => value as f32,
                };
            }
        }
    }
}

/// Replace each bad pixel by the mean of its good neighbors of the same color.
///
/// For one-channel (CFA) images the color comes from the DNG `BayerPhase`
/// (0 RGGB, 1 GRBG, 2 GBRG, 3 BGGR at the image origin): same-color sites at
/// offsets of 2 along rows, columns and diagonals, plus the four diagonal
/// neighbors for green. Multi-channel images use the eight adjacent pixels.
/// A pixel without good neighbors is left unchanged.
fn fix_bad(image: &mut Plane, bad: &[bool], phase: u32) {
    let (w, h, channels) = (image.width as isize, image.height as isize, image.channels);
    let green = |x: isize, y: isize| -> bool {
        let (x, y) = ((x & 1) as u32, (y & 1) as u32);
        // Green sites are where the cell position differs from the red/blue diagonal.
        let red_or_blue_diagonal = matches!(phase, 0 | 3);
        (x ^ y == 1) == red_or_blue_diagonal
    };
    let source = image.data.clone();
    for y in 0..h {
        for x in 0..w {
            let index = (y * w + x) as usize;
            if !bad[index] {
                continue;
            }
            let mut offsets: Vec<(isize, isize)> = Vec::with_capacity(12);
            if channels == 1 {
                for (dx, dy) in [
                    (2, 0),
                    (-2, 0),
                    (0, 2),
                    (0, -2),
                    (2, 2),
                    (2, -2),
                    (-2, 2),
                    (-2, -2),
                ] {
                    offsets.push((dx, dy));
                }
                if green(x, y) {
                    offsets.extend([(1, 1), (1, -1), (-1, 1), (-1, -1)]);
                }
            } else {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if (dx, dy) != (0, 0) {
                            offsets.push((dx, dy));
                        }
                    }
                }
            }
            for c in 0..channels {
                let (mut sum, mut n) = (0.0f64, 0u32);
                for (dx, dy) in &offsets {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let neighbor = (ny * w + nx) as usize;
                    if !bad[neighbor] {
                        sum += f64::from(source[neighbor * channels + c]);
                        n += 1;
                    }
                }
                if n > 0 {
                    image.data[index * channels + c] = (sum / f64::from(n)) as f32;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(ops: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = (ops.len() as u32).to_be_bytes().to_vec();
        for (id, flags, params) in ops {
            out.extend(id.to_be_bytes());
            out.extend(0x0103_0000u32.to_be_bytes());
            out.extend(flags.to_be_bytes());
            out.extend((params.len() as u32).to_be_bytes());
            out.extend(params);
        }
        out
    }

    fn area(width: u32, height: u32) -> Vec<u8> {
        [0, 0, height, width, 0, 1, 1, 1]
            .iter()
            .flat_map(|v: &u32| v.to_be_bytes())
            .collect()
    }

    #[test]
    fn map_table_gain_map_and_bad_pixels_apply() {
        let mut table = area(4, 4);
        table.extend(4u32.to_be_bytes());
        for v in [0u16, 100, 200, 300] {
            table.extend(v.to_be_bytes());
        }
        let mut gain = area(4, 4);
        gain.extend(1u32.to_be_bytes());
        gain.extend(1u32.to_be_bytes());
        for v in [1.0f64, 1.0, 0.0, 0.0] {
            gain.extend(v.to_be_bytes());
        }
        gain.extend(1u32.to_be_bytes());
        gain.extend(2.0f32.to_be_bytes());
        let mut bad = Vec::new();
        for v in [0u32, 1, 0, 2, 2] {
            bad.extend(v.to_be_bytes());
        }
        let bytes = list(&[(7, 0, table), (9, 0, gain), (5, 0, bad)]);
        let ops = parse(1, &bytes).unwrap();
        assert_eq!(ops.len(), 3);
        assert!(ops.iter().all(Opcode::applied));
        let mut image = Plane {
            width: 4,
            height: 4,
            channels: 1,
            data: [1.0, 2.0, 3.0, 9.0].repeat(4),
        };
        apply(&ops, &mut image, Stage::Stored).unwrap();
        // 1 -> 100 -> 200; 3 -> 300 -> 600; index 9 clamps to the last entry.
        assert_eq!(&image.data[..4], &[200.0, 400.0, 600.0, 600.0]);
        // (2, 2) is a bad red site: the mean of the red sites (0, 0), (0, 2), (2, 0).
        assert_eq!(image.data[2 * 4 + 2], (1000.0f64 / 3.0) as f32);
    }

    #[test]
    fn unknown_opcodes_and_limits() {
        let ops = parse(3, &list(&[(42, 1, vec![1, 2, 3]), (43, 0, vec![])])).unwrap();
        assert!(ops[0].optional && !ops[0].applied());
        assert!(!ops[1].optional && !ops[1].applied());
        let mut too_many = list(&[]);
        too_many[..4].copy_from_slice(&257u32.to_be_bytes());
        let error = parse(1, &too_many).unwrap_err().to_string();
        assert!(error.starts_with("[limit-exceeded]"), "{error}");
        let truncated = &list(&[(6, 0, vec![0; 16])])[..20];
        let error = parse(1, truncated).unwrap_err().to_string();
        assert!(error.starts_with("[malformed-resource]"), "{error}");
        let short_trim = list(&[(6, 0, vec![0; 12])]);
        assert!(parse(1, &short_trim).is_err());
    }
}

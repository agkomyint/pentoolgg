//! Bounded classic TIFF reader shared by DNG sources and DNG camera profiles.
//!
//! The reader walks IFD0's chain, SubIFDs (depth up to 2), and the EXIF and GPS
//! IFDs of IFD0. Every entry's value must lie inside the file, the number of IFDs
//! and entries is capped, and any IFD reached twice is a cycle. Values are read
//! lazily and every array read takes an explicit maximum count.
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_IFDS: usize = 32;
pub const MAX_ENTRIES: usize = 1024;
pub const MAX_SUBIFD_DEPTH: u8 = 2;

pub const TAG_SUBIFDS: u16 = 330;
pub const TAG_EXIF_IFD: u16 = 34665;
pub const TAG_GPS_IFD: u16 = 34853;

/// Where an IFD was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Position in IFD0's next-IFD chain (0 is IFD0).
    Chain(usize),
    /// A SubIFD: index of the parent IFD in [`Tiff::ifds`] and the depth (1–2).
    Sub {
        parent: usize,
        depth: u8,
    },
    Exif,
    Gps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub kind: u16,
    pub count: u32,
    /// Absolute offset of the value bytes, already checked to lie in the file.
    pub at: usize,
}

#[derive(Debug, Clone)]
pub struct Ifd {
    pub offset: u32,
    pub role: Role,
    pub entries: BTreeMap<u16, Entry>,
}

#[derive(Debug, Clone)]
pub struct Tiff<'a> {
    pub bytes: &'a [u8],
    pub big_endian: bool,
    pub ifds: Vec<Ifd>,
}

/// Byte size of one value of a TIFF field type; `None` for unknown types.
fn type_size(kind: u16) -> Option<u64> {
    Some(match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

/// True for a classic TIFF header (`II*\0` or `MM\0*`).
pub fn is_tiff(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && (bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*"))
}

/// True for a DNG camera profile header (`IIRC` or `MMCR`).
pub fn is_profile(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && (bytes.starts_with(b"IIRC") || bytes.starts_with(b"MMCR"))
}

impl<'a> Tiff<'a> {
    /// Parse the IFD structure, enforcing the IFD, entry and bounds limits.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if !is_tiff(bytes) {
            if bytes.len() >= 4 && (bytes.starts_with(b"II+\0") || bytes.starts_with(b"MM\0+")) {
                bail!("[unsupported-capability] BigTIFF is not supported; save the DNG as classic TIFF");
            }
            bail!("[malformed-resource] not a TIFF file: the header is not II*\\0 or MM\\0*");
        }
        Self::parse_body(bytes)
    }

    /// Parse a DNG camera profile (`.dcp`): a TIFF structure whose header magic is
    /// `IIRC` or `MMCR` instead of 42.
    pub fn parse_profile(bytes: &'a [u8]) -> Result<Self> {
        if !is_profile(bytes) {
            bail!("[malformed-resource] not a DNG camera profile: the header is not IIRC or MMCR");
        }
        Self::parse_body(bytes)
    }

    fn parse_body(bytes: &'a [u8]) -> Result<Self> {
        let mut tiff = Self {
            bytes,
            big_endian: bytes[0] == b'M',
            ifds: Vec::new(),
        };
        let mut seen = BTreeSet::new();
        // IFD0 chain.
        let mut next = tiff.u32_at(4)?;
        let mut position = 0;
        while next != 0 {
            let following = tiff.read_ifd(next, Role::Chain(position), &mut seen)?;
            position += 1;
            next = following;
        }
        if tiff.ifds.is_empty() {
            bail!("[malformed-resource] TIFF has no IFD0");
        }
        // SubIFDs, breadth first, up to the depth limit.
        let mut index = 0;
        while index < tiff.ifds.len() {
            let depth = match tiff.ifds[index].role {
                Role::Chain(0) => 0,
                Role::Sub { depth, .. } => depth,
                _ => {
                    index += 1;
                    continue;
                }
            };
            if let Some(entry) = tiff.ifds[index].entries.get(&TAG_SUBIFDS).copied() {
                if depth >= MAX_SUBIFD_DEPTH {
                    bail!(
                        "[limit-exceeded] SubIFDs nest deeper than {MAX_SUBIFD_DEPTH} levels below IFD0"
                    );
                }
                for offset in tiff.uints_of(entry, TAG_SUBIFDS, MAX_IFDS)? {
                    tiff.read_ifd(
                        offset,
                        Role::Sub {
                            parent: index,
                            depth: depth + 1,
                        },
                        &mut seen,
                    )?;
                }
            }
            index += 1;
        }
        for (tag, role) in [(TAG_EXIF_IFD, Role::Exif), (TAG_GPS_IFD, Role::Gps)] {
            if let Some(offset) = tiff.ifd0().uint(&tiff, tag)? {
                tiff.read_ifd(offset, role, &mut seen)?;
            }
        }
        Ok(tiff)
    }

    pub fn ifd0(&self) -> &Ifd {
        &self.ifds[0]
    }

    pub fn find(&self, role: Role) -> Option<&Ifd> {
        self.ifds.iter().find(|ifd| ifd.role == role)
    }

    /// Read one IFD and return its next-IFD offset.
    fn read_ifd(&mut self, offset: u32, role: Role, seen: &mut BTreeSet<u32>) -> Result<u32> {
        if !seen.insert(offset) {
            bail!("[malformed-resource] IFD at offset {offset} is reached twice (an IFD cycle)");
        }
        if self.ifds.len() >= MAX_IFDS {
            bail!("[limit-exceeded] TIFF has more than {MAX_IFDS} IFDs");
        }
        let start = offset as usize;
        let count = self.u16_at(start)? as usize;
        if count > MAX_ENTRIES {
            bail!("[limit-exceeded] IFD at offset {offset} has {count} entries; the limit is {MAX_ENTRIES}");
        }
        let table_end = start + 2 + count * 12;
        let next = self.u32_at(table_end)?;
        let mut entries = BTreeMap::new();
        for row in 0..count {
            let at = start + 2 + row * 12;
            let tag = self.u16_at(at)?;
            let kind = self.u16_at(at + 2)?;
            let value_count = self.u32_at(at + 4)?;
            let Some(size) = type_size(kind) else {
                continue; // TIFF readers ignore fields of unknown type.
            };
            let length = size * u64::from(value_count);
            let value_at = if length <= 4 {
                (at + 8) as u64
            } else {
                u64::from(self.u32_at(at + 8)?)
            };
            if value_at + length > self.bytes.len() as u64 {
                bail!(
                    "[malformed-resource] tag {tag} in the IFD at offset {offset} points outside the file"
                );
            }
            entries.insert(
                tag,
                Entry {
                    kind,
                    count: value_count,
                    at: value_at as usize,
                },
            );
        }
        self.ifds.push(Ifd {
            offset,
            role,
            entries,
        });
        Ok(next)
    }

    fn slice(&self, at: usize, length: usize) -> Result<&'a [u8]> {
        at.checked_add(length)
            .and_then(|end| self.bytes.get(at..end))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "[malformed-resource] TIFF structure at offset {at} lies outside the file"
                )
            })
    }

    pub fn u16_at(&self, at: usize) -> Result<u16> {
        let b = self.slice(at, 2)?;
        Ok(if self.big_endian {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        })
    }

    pub fn u32_at(&self, at: usize) -> Result<u32> {
        let b = self.slice(at, 4)?;
        let b = [b[0], b[1], b[2], b[3]];
        Ok(if self.big_endian {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }

    /// Unsigned integers (BYTE, SHORT, LONG, IFD) of an entry, at most `max`.
    pub fn uints_of(&self, entry: Entry, tag: u16, max: usize) -> Result<Vec<u32>> {
        let count = entry.count as usize;
        if count > max {
            bail!("[limit-exceeded] tag {tag} has {count} values; the limit is {max}");
        }
        (0..count).map(|i| self.uint_value(entry, i, tag)).collect()
    }

    fn uint_value(&self, entry: Entry, index: usize, tag: u16) -> Result<u32> {
        match entry.kind {
            1 | 7 => Ok(u32::from(self.bytes[entry.at + index])),
            3 => self.u16_at(entry.at + 2 * index).map(u32::from),
            4 | 13 => self.u32_at(entry.at + 4 * index),
            kind => bail!(
                "[malformed-resource] tag {tag} has type {kind}; expected an unsigned integer"
            ),
        }
    }

    /// Numeric values of an entry as `f64` (integers, rationals, floats), at most `max`.
    pub fn numbers_of(&self, entry: Entry, tag: u16, max: usize) -> Result<Vec<f64>> {
        let count = entry.count as usize;
        if count > max {
            bail!("[limit-exceeded] tag {tag} has {count} values; the limit is {max}");
        }
        (0..count)
            .map(|i| {
                Ok(match entry.kind {
                    1 | 3 | 4 | 7 | 13 => f64::from(self.uint_value(entry, i, tag)?),
                    6 => f64::from(self.bytes[entry.at + i] as i8),
                    8 => f64::from(self.u16_at(entry.at + 2 * i)? as i16),
                    9 => f64::from(self.u32_at(entry.at + 4 * i)? as i32),
                    5 => {
                        let n = self.u32_at(entry.at + 8 * i)?;
                        let d = self.u32_at(entry.at + 8 * i + 4)?;
                        rational(f64::from(n), f64::from(d), tag)?
                    }
                    10 => {
                        let n = self.u32_at(entry.at + 8 * i)? as i32;
                        let d = self.u32_at(entry.at + 8 * i + 4)? as i32;
                        rational(f64::from(n), f64::from(d), tag)?
                    }
                    11 => f64::from(f32::from_bits(self.u32_at(entry.at + 4 * i)?)),
                    12 => {
                        let high = u64::from(self.u32_at(entry.at + 8 * i)?);
                        let low = u64::from(self.u32_at(entry.at + 8 * i + 4)?);
                        let bits = if self.big_endian {
                            (high << 32) | low
                        } else {
                            (low << 32) | high
                        };
                        f64::from_bits(bits)
                    }
                    kind => {
                        bail!("[malformed-resource] tag {tag} has type {kind}; expected a number")
                    }
                })
            })
            .collect::<Result<Vec<f64>>>()
            .and_then(|values| {
                if values.iter().all(|v| v.is_finite()) {
                    Ok(values)
                } else {
                    bail!("[malformed-resource] tag {tag} holds a value that is not finite")
                }
            })
    }

    /// Raw bytes of an entry, at most `max` bytes.
    pub fn bytes_of(&self, entry: Entry, tag: u16, max: usize) -> Result<&'a [u8]> {
        let length = entry.count as usize * type_size(entry.kind).unwrap_or(1) as usize;
        if length > max {
            bail!("[limit-exceeded] tag {tag} holds {length} bytes; the limit is {max}");
        }
        self.slice(entry.at, length)
    }

    /// An ASCII entry as text, trimmed of NULs and trailing spaces, at most `max` bytes.
    pub fn text_of(&self, entry: Entry, tag: u16, max: usize) -> Result<String> {
        let bytes = self.bytes_of(entry, tag, max)?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        let text = String::from_utf8_lossy(&bytes[..end]);
        Ok(text.trim_end().to_string())
    }
}

fn rational(numerator: f64, denominator: f64, tag: u16) -> Result<f64> {
    if denominator == 0.0 {
        bail!("[malformed-resource] tag {tag} has a rational with a zero denominator");
    }
    Ok(numerator / denominator)
}

impl Ifd {
    pub fn has(&self, tag: u16) -> bool {
        self.entries.contains_key(&tag)
    }

    /// The single unsigned integer value of `tag`, if present.
    pub fn uint(&self, tiff: &Tiff, tag: u16) -> Result<Option<u32>> {
        let Some(entry) = self.entries.get(&tag) else {
            return Ok(None);
        };
        let values = tiff.uints_of(*entry, tag, 1)?;
        values
            .first()
            .copied()
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("[malformed-resource] tag {tag} is empty"))
    }

    pub fn uints(&self, tiff: &Tiff, tag: u16, max: usize) -> Result<Option<Vec<u32>>> {
        self.entries
            .get(&tag)
            .map(|entry| tiff.uints_of(*entry, tag, max))
            .transpose()
    }

    pub fn numbers(&self, tiff: &Tiff, tag: u16, max: usize) -> Result<Option<Vec<f64>>> {
        self.entries
            .get(&tag)
            .map(|entry| tiff.numbers_of(*entry, tag, max))
            .transpose()
    }

    pub fn text(&self, tiff: &Tiff, tag: u16, max: usize) -> Result<Option<String>> {
        self.entries
            .get(&tag)
            .map(|entry| tiff.text_of(*entry, tag, max))
            .transpose()
    }

    pub fn raw<'a>(&self, tiff: &Tiff<'a>, tag: u16, max: usize) -> Result<Option<&'a [u8]>> {
        self.entries
            .get(&tag)
            .map(|entry| tiff.bytes_of(*entry, tag, max))
            .transpose()
    }
}

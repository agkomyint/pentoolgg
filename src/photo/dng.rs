//! DNG inspection: container detection, the raw IFD, decode limits and the facts
//! recorded in `photography.assets` (`raw` and `capture`).
//!
//! Inspection reads tags and validates every structure the decoder will touch,
//! in the order `docs/photography-v1.md` lists, without decoding pixels.
use super::opcode::{self, Opcode};
use super::pixels::check_surface;
use super::tiff::{Ifd, Role, Tiff};
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

/// Largest RAW source file.
pub const MAX_RAW_BYTES: u64 = 512 * 1024 * 1024;
/// Largest number of strips or tiles in the raw IFD.
pub const MAX_SEGMENTS: usize = 65_536;
/// Largest `LinearizationTable`.
pub const MAX_LINEARIZATION: usize = 65_536;
/// Largest `BlackLevelRepeatDim` side.
pub const MAX_BLACK_REPEAT: u32 = 64;
/// Newest DNG (and DNG backward) version engine 1 reads.
pub const NEWEST_DNG: [u8; 4] = [1, 7, 1, 0];

pub const MEDIA_TYPE: &str = "image/x-adobe-dng";

/// A 2x2 Bayer pattern, named by its first row then second row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cfa {
    Rggb,
    Bggr,
    Grbg,
    Gbrg,
}

impl Cfa {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rggb => "RGGB",
            Self::Bggr => "BGGR",
            Self::Grbg => "GRBG",
            Self::Gbrg => "GBRG",
        }
    }

    /// The colors (0 red, 1 green, 2 blue) of the 2x2 cell, row-major.
    pub fn cell(self) -> [usize; 4] {
        match self {
            Self::Rggb => [0, 1, 1, 2],
            Self::Bggr => [2, 1, 1, 0],
            Self::Grbg => [1, 0, 2, 1],
            Self::Gbrg => [1, 2, 0, 1],
        }
    }

    /// Color of the photosite at `(x, y)` relative to the active area.
    pub fn color(self, x: usize, y: usize) -> usize {
        self.cell()[(y & 1) * 2 + (x & 1)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Cfa(Cfa),
    /// LinearRaw with 1 (monochrome) or 3 samples per pixel.
    LinearRaw(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Unsigned integers of 8, 10, 12, 14 or 16 bits.
    Uint,
    /// IEEE floats of 16, 24 or 32 bits (LinearRaw only).
    Float,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    LosslessJpeg,
    /// Deflate with its TIFF `Predictor`.
    Deflate(u16),
}

/// Strip or tile layout of the raw IFD. Strips are tiles as wide as the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segments {
    pub width: usize,
    pub height: usize,
    pub across: usize,
    pub tiled: bool,
    /// `(offset, byte count)`, each already checked to lie inside the file.
    pub spans: Vec<(usize, usize)>,
}

/// A rectangle `[top, left, bottom, right)` in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub top: usize,
    pub left: usize,
    pub bottom: usize,
    pub right: usize,
}

impl Rect {
    pub fn width(&self) -> usize {
        self.right - self.left
    }

    pub fn height(&self) -> usize {
        self.bottom - self.top
    }
}

/// An inspected DNG: everything decoding needs, validated against the limits.
#[derive(Debug, Clone)]
pub struct Dng<'a> {
    pub tiff: Tiff<'a>,
    /// Index of the raw IFD in `tiff.ifds`.
    pub raw: usize,
    pub version: [u8; 4],
    pub width: usize,
    pub height: usize,
    pub layout: Layout,
    pub samples: usize,
    pub bits: u32,
    pub format: Format,
    pub compression: Compression,
    pub segments: Segments,
    pub linearization: Option<Vec<u16>>,
    /// `BlackLevelRepeatDim` as `(rows, columns)`.
    pub black_repeat: (usize, usize),
    /// `rows * columns * samples` values, row-major then by sample.
    pub black: Vec<f64>,
    /// Per active-area column and row.
    pub black_delta_h: Option<Vec<f64>>,
    pub black_delta_v: Option<Vec<f64>>,
    /// One per sample.
    pub white: Vec<f64>,
    /// Active area in stored-image pixels.
    pub active: Rect,
    /// Final crop relative to the active area: the default crop intersected
    /// with every `TrimBounds` opcode.
    pub crop: Rect,
    pub orientation: u8,
    pub baseline_exposure: Option<f64>,
    pub as_shot_neutral: Option<Vec<f64>>,
    pub unique_camera_model: String,
    pub opcodes: Vec<Opcode>,
}

/// Name a non-DNG RAW container from its signature.
pub fn foreign_signature(bytes: &[u8]) -> Option<&'static str> {
    let at = |offset: usize, magic: &[u8]| bytes.get(offset..offset + magic.len()) == Some(magic);
    if at(0, b"II*\0") && at(8, b"CR") {
        return Some("Canon CR2");
    }
    if at(4, b"ftypcrx ") {
        return Some("Canon CR3");
    }
    if at(0, b"IIRO") || at(0, b"IIRS") || at(0, b"MMOR") {
        return Some("Olympus ORF");
    }
    if at(0, b"IIU\0") {
        return Some("Panasonic RW2");
    }
    if at(0, b"FUJIFILMCCD-RAW") {
        return Some("Fujifilm RAF");
    }
    if at(0, b"FOVb") {
        return Some("Sigma X3F");
    }
    if at(0, b"IIII") {
        return Some("Phase One IIQ");
    }
    None
}

/// Name a TIFF-structured RAW container (no DNGVersion) by its camera make.
fn foreign_make(tiff: &Tiff) -> Option<&'static str> {
    let make = tiff.ifd0().text(tiff, 271, 256).ok()??.to_ascii_uppercase();
    [
        ("NIKON", "Nikon NEF/NRW"),
        ("SONY", "Sony ARW/SR2"),
        ("PENTAX", "Pentax PEF"),
        ("RICOH", "Pentax PEF"),
        ("HASSELBLAD", "Hasselblad 3FR"),
        ("PHASE ONE", "Phase One IIQ"),
        ("CANON", "Canon CR2"),
    ]
    .into_iter()
    .find(|(vendor, _)| make.starts_with(vendor))
    .map(|(_, name)| name)
}

const TAG_DNG_VERSION: u16 = 50706;

fn convert_hint(container: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "[unsupported-capability] {container} raw files are not supported; convert to DNG first (for example with Adobe DNG Converter)"
    )
}

fn version_text(version: [u8; 4]) -> String {
    format!(
        "{}.{}.{}.{}",
        version[0], version[1], version[2], version[3]
    )
}

impl<'a> Dng<'a> {
    /// Inspect a DNG, checking the limits in the specified order.
    pub fn inspect(bytes: &'a [u8]) -> Result<Self> {
        if bytes.is_empty() {
            bail!("[malformed-resource] RAW source is empty");
        }
        if bytes.len() as u64 > MAX_RAW_BYTES {
            bail!(
                "[limit-exceeded] RAW source is {} bytes; the limit is {MAX_RAW_BYTES} (512 MiB)",
                bytes.len()
            );
        }
        if let Some(container) = foreign_signature(bytes) {
            return Err(convert_hint(container));
        }
        let tiff = Tiff::parse(bytes)?;
        let ifd0 = tiff.ifd0();
        let Some(version) = ifd0.raw(&tiff, TAG_DNG_VERSION, 4)? else {
            if let Some(container) = foreign_make(&tiff) {
                return Err(convert_hint(container));
            }
            bail!("[unsupported-capability] TIFF without DNGVersion is a rendered image, not a RAW source; import it as a photo instead of with raw add");
        };
        let version: [u8; 4] = version.try_into().map_err(|_| {
            anyhow::anyhow!("[malformed-resource] DNGVersion (50706) must hold 4 bytes")
        })?;
        if version < [1, 0, 0, 0] || version > NEWEST_DNG {
            bail!(
                "[unsupported-capability] DNG version {} is outside 1.0.0.0–1.7.1.0; re-save it with a DNG 1.7 or older writer",
                version_text(version)
            );
        }
        if let Some(backward) = ifd0.raw(&tiff, 50707, 4)? {
            let backward: [u8; 4] = backward.try_into().map_err(|_| {
                anyhow::anyhow!("[malformed-resource] DNGBackwardVersion (50707) must hold 4 bytes")
            })?;
            if backward > NEWEST_DNG {
                bail!(
                    "[unsupported-capability] DNG needs a reader for version {}; engine 1 reads up to 1.7.1.0",
                    version_text(backward)
                );
            }
        }
        let raw = find_raw_ifd(&tiff)?;
        let ifd = &tiff.ifds[raw];
        let (width, height) = (required(ifd, &tiff, 256)?, required(ifd, &tiff, 257)?);
        // Dimensions are checked before anything is sized from them.
        check_surface(width, height)?;
        let (width, height) = (width as usize, height as usize);

        let samples = ifd.uint(&tiff, 277)?.unwrap_or(1) as usize;
        let photometric = required(ifd, &tiff, 262)?;
        let layout = match photometric {
            32803 => Layout::Cfa(read_cfa(ifd, &tiff)?),
            34892 if samples == 1 || samples == 3 => Layout::LinearRaw(samples as u8),
            34892 => bail!("[unsupported-capability] LinearRaw with {samples} samples per pixel; engine 1 reads 1 or 3"),
            other => bail!("[unsupported-capability] PhotometricInterpretation {other} in the raw IFD; engine 1 reads CFA (32803) and LinearRaw (34892)"),
        };
        if matches!(layout, Layout::Cfa(_)) && samples != 1 {
            bail!("[unsupported-capability] CFA raw with {samples} samples per pixel");
        }
        if ifd.uint(&tiff, 284)?.unwrap_or(1) != 1 {
            bail!("[unsupported-capability] PlanarConfiguration 2 (separate planes); engine 1 reads chunky (1) raw data");
        }
        let bits_all = ifd.uints(&tiff, 258, 4)?.unwrap_or_else(|| vec![1]);
        let bits = bits_all[0];
        if bits_all.len() != samples || bits_all.iter().any(|b| *b != bits) {
            bail!("[unsupported-capability] BitsPerSample {bits_all:?} must hold one equal value per sample");
        }
        let format = match ifd.uint(&tiff, 339)?.unwrap_or(1) {
            1 => Format::Uint,
            3 => Format::Float,
            other => bail!("[unsupported-capability] SampleFormat {other} in the raw IFD"),
        };
        match (format, layout) {
            (Format::Uint, _) if matches!(bits, 8 | 10 | 12 | 14 | 16) => {}
            (Format::Float, Layout::LinearRaw(_)) if matches!(bits, 16 | 24 | 32) => {}
            _ => bail!(
                "[unsupported-capability] {bits}-bit {} samples in a {} raw; engine 1 reads integer 8, 10, 12, 14 or 16 bits, and float 16, 24 or 32 bits for LinearRaw",
                if format == Format::Uint { "integer" } else { "float" },
                if matches!(layout, Layout::Cfa(_)) { "CFA" } else { "LinearRaw" }
            ),
        }
        let compression = match ifd.uint(&tiff, 259)?.unwrap_or(1) {
            1 => Compression::None,
            7 if format == Format::Uint => Compression::LosslessJpeg,
            8 => {
                let predictor = ifd.uint(&tiff, 317)?.unwrap_or(1);
                let valid = match (predictor, format) {
                    (1, _) => true,
                    (2, Format::Uint) => matches!(bits, 8 | 16),
                    (34_894 | 34_895, Format::Float) => true,
                    _ => false,
                };
                if !valid {
                    bail!("[unsupported-capability] deflate Predictor {predictor} with {bits}-bit samples; engine 1 reads 1, 2 (8/16-bit integers), 34894 and 34895 (floats)");
                }
                Compression::Deflate(predictor as u16)
            }
            34_892 => bail!("[unsupported-capability] lossy JPEG DNG (compression 34892) is not supported; export an uncompressed or lossless DNG"),
            52_546 => bail!("[unsupported-capability] JPEG XL DNG (compression 52546) is not supported; export an uncompressed or lossless DNG"),
            other => bail!("[unsupported-capability] DNG compression {other} is not supported; engine 1 reads 1 (none), 7 (lossless JPEG) and 8 (deflate)"),
        };
        let segments = read_segments(ifd, &tiff, width, height)?;
        check_segment_sizes(&segments, compression, samples, bits, height)?;

        let linearization = match ifd.entries.get(&50712) {
            Some(entry) => {
                if format == Format::Float {
                    bail!("[unsupported-capability] LinearizationTable with float samples");
                }
                let table = tiff.uints_of(*entry, 50712, MAX_LINEARIZATION)?;
                if table.is_empty() {
                    bail!("[malformed-resource] LinearizationTable (50712) is empty");
                }
                Some(table.into_iter().map(|v| v.min(65_535) as u16).collect())
            }
            None => None,
        };

        let active = match ifd.uints(&tiff, 50829, 4)? {
            Some(v) if v.len() == 4 => Rect {
                top: v[0] as usize,
                left: v[1] as usize,
                bottom: v[2] as usize,
                right: v[3] as usize,
            },
            Some(_) => bail!("[malformed-resource] ActiveArea (50829) must hold 4 values"),
            None => Rect {
                top: 0,
                left: 0,
                bottom: height,
                right: width,
            },
        };
        if active.top >= active.bottom
            || active.left >= active.right
            || active.bottom > height
            || active.right > width
        {
            bail!("[malformed-resource] ActiveArea {active:?} is empty or outside the {width}x{height} raw image");
        }
        if let Layout::Cfa(_) = layout {
            if active.width() < 2 || active.height() < 2 {
                bail!("[malformed-resource] the CFA active area must be at least 2x2");
            }
        }

        let black_repeat = match ifd.uints(&tiff, 50713, 2)? {
            Some(v) if v.len() == 2 && (1..=MAX_BLACK_REPEAT).contains(&v[0]) && (1..=MAX_BLACK_REPEAT).contains(&v[1]) => {
                (v[0] as usize, v[1] as usize)
            }
            Some(v) => bail!("[malformed-resource] BlackLevelRepeatDim {v:?} must be two values in 1–{MAX_BLACK_REPEAT}"),
            None => (1, 1),
        };
        let black_count = black_repeat.0 * black_repeat.1 * samples;
        let black = match ifd.numbers(&tiff, 50714, black_count)? {
            Some(v) if v.len() == black_count => v,
            Some(v) if v.len() == 1 => vec![v[0]; black_count],
            Some(v) => bail!("[malformed-resource] BlackLevel holds {} values; BlackLevelRepeatDim and SamplesPerPixel need {black_count}", v.len()),
            None => vec![0.0; black_count],
        };
        let black_delta_h = delta(ifd, &tiff, 50715, active.width())?;
        let black_delta_v = delta(ifd, &tiff, 50716, active.height())?;
        let default_white = match format {
            Format::Uint => f64::from((1u32 << bits) - 1),
            Format::Float => 1.0,
        };
        let white = match ifd.numbers(&tiff, 50717, samples)? {
            Some(v) if v.len() == samples => v,
            Some(v) if v.len() == 1 => vec![v[0]; samples],
            Some(_) => {
                bail!("[malformed-resource] WhiteLevel (50717) must hold one value per sample")
            }
            None => vec![default_white; samples],
        };
        for (s, w) in white.iter().enumerate() {
            let darkest = (0..black_repeat.0 * black_repeat.1)
                .map(|cell| black[cell * samples + s])
                .fold(f64::NEG_INFINITY, f64::max);
            if *w <= darkest {
                bail!("[malformed-resource] WhiteLevel {w} of sample {s} is not above BlackLevel {darkest}");
            }
        }

        if let Some(scale) = ifd.numbers(&tiff, 50718, 2)? {
            if scale != [1.0, 1.0] {
                bail!("[unsupported-capability] DefaultScale {scale:?}; engine 1 reads square-pixel DNGs (DefaultScale 1/1)");
            }
        }
        let mut crop = default_crop(ifd, &tiff, &active)?;

        let mut opcodes = Vec::new();
        for (list, tag) in [(1u8, 51008u16), (2, 51009), (3, 51022)] {
            if let Some(bytes) = ifd.raw(&tiff, tag, opcode::MAX_LIST_BYTES)? {
                opcodes.extend(opcode::parse(list, bytes)?);
            }
        }
        for op in &opcodes {
            if let opcode::Op::TrimBounds(trim) = &op.op {
                // List 1 is in stored-image pixels; lists 2 and 3 in active-area pixels.
                let (dy, dx) = if op.list == 1 {
                    (active.top, active.left)
                } else {
                    (0, 0)
                };
                let shift = |v: u32, d: usize| (v as usize).saturating_sub(d);
                crop = Rect {
                    top: crop.top.max(shift(trim.top, dy)),
                    left: crop.left.max(shift(trim.left, dx)),
                    bottom: crop.bottom.min(shift(trim.bottom, dy)),
                    right: crop.right.min(shift(trim.right, dx)),
                };
                if crop.top >= crop.bottom || crop.left >= crop.right {
                    bail!("[malformed-resource] TrimBounds in opcode list {} leaves no pixels inside the default crop", op.list);
                }
            }
        }

        let ifd0 = tiff.ifd0();
        let orientation = match ifd0.uint(&tiff, 274)? {
            None => 1,
            Some(v @ 1..=8) => v as u8,
            Some(v) => bail!("[malformed-resource] Orientation (274) is {v}; expected 1–8"),
        };
        let baseline_exposure = ifd0.numbers(&tiff, 50730, 1)?.map(|v| v[0]);
        if baseline_exposure.is_some_and(|v| !(-10.0..=10.0).contains(&v)) {
            bail!("[malformed-resource] BaselineExposure must lie in -10..10 EV");
        }
        let planes = samples.max(if matches!(layout, Layout::Cfa(_)) {
            3
        } else {
            1
        });
        let as_shot_neutral = match ifd0.numbers(&tiff, 50728, 4)? {
            Some(v) if v.len() == planes && v.iter().all(|n| *n > 0.0) => Some(v),
            Some(v) => {
                bail!("[malformed-resource] AsShotNeutral {v:?} must hold {planes} positive values")
            }
            None => None,
        };
        let color_matrix = ifd0.numbers(&tiff, 50721, 9)?;
        if color_matrix.as_ref().is_none_or(|m| m.len() != 3 * planes) {
            bail!("[malformed-resource] ColorMatrix1 (50721) is missing or does not hold {} values; every DNG must carry it", 3 * planes);
        }
        let unique_camera_model = ifd0
            .text(&tiff, 50708, 4096)?
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("[malformed-resource] UniqueCameraModel (50708) is missing; every DNG must carry it"))?;
        let unique_camera_model = truncate(&unique_camera_model, 256);

        Ok(Self {
            raw,
            version,
            width,
            height,
            layout,
            samples,
            bits,
            format,
            compression,
            segments,
            linearization,
            black_repeat,
            black,
            black_delta_h,
            black_delta_v,
            white,
            active,
            crop,
            orientation,
            baseline_exposure,
            as_shot_neutral,
            unique_camera_model,
            opcodes,
            tiff,
        })
    }

    pub fn ifd(&self) -> &Ifd {
        &self.tiff.ifds[self.raw]
    }

    /// Developed size before orientation: the final crop.
    pub fn pixel_size(&self) -> (u32, u32) {
        (self.crop.width() as u32, self.crop.height() as u32)
    }

    /// The `raw` facts of the asset record.
    pub fn raw_facts(&self) -> Value {
        let mut raw = Map::new();
        raw.insert("dng_version".into(), json!(version_text(self.version)));
        match self.layout {
            Layout::Cfa(cfa) => {
                raw.insert("layout".into(), json!("cfa"));
                raw.insert("cfa_pattern".into(), json!(cfa.name()));
            }
            Layout::LinearRaw(samples) => {
                raw.insert("layout".into(), json!("linear-raw"));
                raw.insert("cfa_pattern".into(), Value::Null);
                raw.insert("samples".into(), json!(samples));
            }
        }
        raw.insert(
            "unique_camera_model".into(),
            json!(self.unique_camera_model),
        );
        if let Some(neutral) = self.as_shot_neutral.as_ref().filter(|n| n.len() == 3) {
            raw.insert(
                "as_shot_neutral".into(),
                Value::Array(neutral.iter().map(|v| number(*v)).collect()),
            );
        }
        if let Some(exposure) = self.baseline_exposure {
            raw.insert("baseline_exposure".into(), number(exposure));
        }
        raw.insert(
            "opcodes".into(),
            Value::Array(
                self.opcodes
                    .iter()
                    .map(|op| {
                        json!({"list": op.list, "id": op.id, "applied": op.applied(), "optional": op.optional})
                    })
                    .collect(),
            ),
        );
        Value::Object(raw)
    }

    /// The searchable `capture` summary. GPS, serials and owner are never read.
    pub fn capture(&self) -> Value {
        capture(&self.tiff)
    }

    /// The complete asset record facts (everything except `storage`).
    pub fn asset_facts(&self) -> Value {
        let (width, height) = self.pixel_size();
        json!({
            "media_type": MEDIA_TYPE,
            "kind": "raw",
            "byte_length": self.tiff.bytes.len(),
            "pixel_width": width,
            "pixel_height": height,
            "orientation": self.orientation,
            "bit_depth": self.bits,
            "input_profile": "camera",
            "capture": self.capture(),
            "raw": self.raw_facts(),
        })
    }

    /// Mandatory opcodes this engine cannot apply; developing fails on them.
    pub fn unsupported_opcodes(&self) -> Vec<&Opcode> {
        self.opcodes
            .iter()
            .filter(|op| matches!(op.op, opcode::Op::Unknown) && !op.optional && !op.preview)
            .collect()
    }
}

fn required(ifd: &Ifd, tiff: &Tiff, tag: u16) -> Result<u32> {
    ifd.uint(tiff, tag)?
        .ok_or_else(|| anyhow::anyhow!("[malformed-resource] the raw IFD lacks required tag {tag}"))
}

fn find_raw_ifd(tiff: &Tiff) -> Result<usize> {
    let mut candidates = Vec::new();
    for (index, ifd) in tiff.ifds.iter().enumerate() {
        if !matches!(ifd.role, Role::Chain(0) | Role::Sub { .. }) {
            continue;
        }
        if ifd.uint(tiff, 254)?.unwrap_or(0) == 0 {
            candidates.push(index);
        }
    }
    if candidates.len() > 1 {
        let mut raw = Vec::new();
        for index in candidates {
            if matches!(tiff.ifds[index].uint(tiff, 262)?, Some(32803 | 34892)) {
                raw.push(index);
            }
        }
        candidates = raw;
    }
    match candidates.as_slice() {
        [index] => Ok(*index),
        [] => bail!("[malformed-resource] DNG has no main raw image (NewSubfileType 0) in IFD0 or its SubIFDs"),
        _ => bail!("[malformed-resource] DNG has {} main raw images; expected one", candidates.len()),
    }
}

fn read_cfa(ifd: &Ifd, tiff: &Tiff) -> Result<Cfa> {
    let dim = ifd.uints(tiff, 33421, 2)?.unwrap_or_default();
    if dim != [2, 2] {
        bail!("[unsupported-capability] CFARepeatPatternDim {dim:?}; engine 1 reads 2x2 Bayer patterns (X-Trans and other layouts are not supported)");
    }
    if ifd.uint(tiff, 50711)?.unwrap_or(1) != 1 {
        bail!("[unsupported-capability] CFALayout other than 1 (rectangular)");
    }
    let planes = ifd.uints(tiff, 50710, 4)?.unwrap_or_else(|| vec![0, 1, 2]);
    if planes != [0, 1, 2] {
        bail!("[unsupported-capability] CFAPlaneColor {planes:?}; engine 1 reads RGB (0, 1, 2) sensors");
    }
    let pattern = ifd
        .uints(tiff, 33422, 4)?
        .ok_or_else(|| anyhow::anyhow!("[malformed-resource] CFA raw lacks CFAPattern (33422)"))?;
    [Cfa::Rggb, Cfa::Bggr, Cfa::Grbg, Cfa::Gbrg]
        .into_iter()
        .find(|cfa| {
            cfa.cell()
                .iter()
                .map(|c| *c as u32)
                .eq(pattern.iter().copied())
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "[unsupported-capability] CFAPattern {pattern:?} is not RGGB, BGGR, GRBG or GBRG"
            )
        })
}

fn read_segments(ifd: &Ifd, tiff: &Tiff, width: usize, height: usize) -> Result<Segments> {
    let tiled = ifd.has(324) || ifd.has(322);
    let (offsets_tag, counts_tag) = if tiled { (324, 325) } else { (273, 279) };
    let (seg_width, seg_height) = if tiled {
        let tw = required(ifd, tiff, 322)? as usize;
        let th = required(ifd, tiff, 323)? as usize;
        if tw == 0 || th == 0 || tw > 65_536 || th > 65_536 {
            bail!("[malformed-resource] TileWidth/TileLength {tw}x{th} must be 1–65536");
        }
        (tw, th)
    } else {
        let rows = ifd.uint(tiff, 278)?.unwrap_or(u32::MAX) as usize;
        if rows == 0 {
            bail!("[malformed-resource] RowsPerStrip (278) is 0");
        }
        (width, rows.min(height))
    };
    let across = width.div_ceil(seg_width);
    let expected = across * height.div_ceil(seg_height);
    let entry = |tag: u16| {
        ifd.entries.get(&tag).copied().ok_or_else(|| {
            anyhow::anyhow!(
                "[malformed-resource] the raw IFD lacks {} ({tag})",
                if tag == 273 || tag == 324 {
                    "segment offsets"
                } else {
                    "segment byte counts"
                }
            )
        })
    };
    let (offsets, counts) = (entry(offsets_tag)?, entry(counts_tag)?);
    for e in [offsets, counts] {
        if e.count as usize > MAX_SEGMENTS {
            bail!(
                "[limit-exceeded] the raw IFD has {} strips or tiles; the limit is {MAX_SEGMENTS}",
                e.count
            );
        }
    }
    if expected > MAX_SEGMENTS {
        bail!("[limit-exceeded] the raw image needs {expected} strips or tiles; the limit is {MAX_SEGMENTS}");
    }
    let offsets = tiff.uints_of(offsets, offsets_tag, MAX_SEGMENTS)?;
    let counts = tiff.uints_of(counts, counts_tag, MAX_SEGMENTS)?;
    if offsets.len() != expected || counts.len() != expected {
        bail!(
            "[malformed-resource] the raw IFD lists {} offsets and {} byte counts; its layout needs {expected}",
            offsets.len(),
            counts.len()
        );
    }
    let size = tiff.bytes.len() as u64;
    let mut spans = Vec::with_capacity(expected);
    for (index, (offset, count)) in offsets.into_iter().zip(counts).enumerate() {
        if u64::from(offset) + u64::from(count) > size || count == 0 {
            bail!(
                "[malformed-resource] {} {index} (offset {offset}, {count} bytes) lies outside the {size}-byte file",
                if tiled { "tile" } else { "strip" }
            );
        }
        spans.push((offset as usize, count as usize));
    }
    Ok(Segments {
        width: seg_width,
        height: seg_height,
        across,
        tiled,
        spans,
    })
}

/// Bytes one row of a segment occupies when uncompressed.
pub fn row_bytes(width: usize, samples: usize, bits: u32) -> usize {
    (width * samples * bits as usize).div_ceil(8)
}

fn check_segment_sizes(
    segments: &Segments,
    compression: Compression,
    samples: usize,
    bits: u32,
    height: usize,
) -> Result<()> {
    if compression != Compression::None {
        return Ok(()); // Compressed sizes are checked when each segment decodes.
    }
    for (index, (_, count)) in segments.spans.iter().enumerate() {
        let row = index / segments.across;
        let rows = if segments.tiled {
            segments.height
        } else {
            segments.height.min(height - row * segments.height)
        };
        let needed = rows * row_bytes(segments.width, samples, bits);
        if *count < needed {
            bail!(
                "[malformed-resource] {} {index} holds {count} bytes; uncompressed it needs {needed}",
                if segments.tiled { "tile" } else { "strip" }
            );
        }
    }
    Ok(())
}

fn delta(ifd: &Ifd, tiff: &Tiff, tag: u16, length: usize) -> Result<Option<Vec<f64>>> {
    match ifd.numbers(tiff, tag, length)? {
        Some(v) if v.len() == length => Ok(Some(v)),
        Some(v) => bail!(
            "[malformed-resource] tag {tag} holds {} values; the active area needs {length}",
            v.len()
        ),
        None => Ok(None),
    }
}

fn default_crop(ifd: &Ifd, tiff: &Tiff, active: &Rect) -> Result<Rect> {
    let pair = |tag: u16| -> Result<Option<(f64, f64)>> {
        match ifd.numbers(tiff, tag, 2)? {
            Some(v) if v.len() == 2 && v.iter().all(|n| *n >= 0.0) => Ok(Some((v[0], v[1]))),
            Some(_) => bail!("[malformed-resource] tag {tag} must hold two non-negative values"),
            None => Ok(None),
        }
    };
    // DefaultCrop values are rounded to the nearest whole pixel (half away from zero).
    let origin = pair(50719)?.unwrap_or((0.0, 0.0));
    let size = pair(50720)?.unwrap_or((active.width() as f64, active.height() as f64));
    let (left, top) = (origin.0.round() as usize, origin.1.round() as usize);
    let (width, height) = (size.0.round() as usize, size.1.round() as usize);
    if width == 0 || height == 0 || left + width > active.width() || top + height > active.height()
    {
        bail!(
            "[malformed-resource] DefaultCrop {width}x{height} at {left},{top} lies outside the {}x{} active area",
            active.width(),
            active.height()
        );
    }
    Ok(Rect {
        top,
        left,
        bottom: top + height,
        right: left + width,
    })
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// A JSON number, written as an integer when the value is integral.
pub fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn capture(tiff: &Tiff) -> Value {
    let mut capture = Map::new();
    let ifd0 = tiff.ifd0();
    let text = |ifd: &Ifd, tag| {
        ifd.text(tiff, tag, 4096)
            .ok()
            .flatten()
            .map(|t| truncate(t.trim(), 128))
            .filter(|t| !t.is_empty())
    };
    if let Some(make) = text(ifd0, 271) {
        capture.insert("make".into(), json!(make));
    }
    if let Some(model) = text(ifd0, 272) {
        capture.insert("model".into(), json!(model));
    }
    if let Some(exif) = tiff.find(Role::Exif) {
        if let Some(lens) = text(exif, 42036) {
            capture.insert("lens".into(), json!(lens));
        }
        let first = |tag| {
            exif.numbers(tiff, tag, 16)
                .ok()
                .flatten()
                .and_then(|v| v.first().copied())
        };
        if let Some(focal) = first(37386).filter(|v| *v > 0.0 && *v <= 10_000.0) {
            capture.insert("focal_length".into(), number(focal));
        }
        if let Some(aperture) = first(33437).filter(|v| *v > 0.0 && *v <= 1000.0) {
            capture.insert("aperture".into(), number(aperture));
        }
        if let Some(entry) = exif
            .entries
            .get(&33434)
            .filter(|e| e.kind == 5 && e.count >= 1)
        {
            if let (Ok(n), Ok(d)) = (tiff.u32_at(entry.at), tiff.u32_at(entry.at + 4)) {
                if n >= 1 && d >= 1 {
                    capture.insert("exposure_time".into(), json!([n, d]));
                }
            }
        }
        if let Some(iso) = first(34855).filter(|v| *v >= 1.0 && *v <= 10_000_000.0) {
            capture.insert("iso".into(), json!(iso as u64));
        }
        if let Some(flash) = first(37385) {
            capture.insert("flash".into(), json!(flash as u32 & 1 == 1));
        }
        if let Some(captured) = text(exif, 36867).and_then(|t| iso_datetime(&t)) {
            let offset = text(exif, 36881).filter(|o| valid_offset(o));
            capture.insert(
                "captured".into(),
                json!(format!("{captured}{}", offset.unwrap_or_default())),
            );
        }
    }
    Value::Object(capture)
}

/// EXIF `YYYY:MM:DD HH:MM:SS` as ISO 8601 local time.
fn iso_datetime(text: &str) -> Option<String> {
    let b = text.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| b[range].iter().all(u8::is_ascii_digit);
    let shape = digits(0..4)
        && b[4] == b':'
        && digits(5..7)
        && b[7] == b':'
        && digits(8..10)
        && b[10] == b' '
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19);
    if !shape || &text[0..4] == "0000" {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}",
        &text[0..4],
        &text[5..7],
        &text[8..10],
        &text[11..19]
    ))
}

fn valid_offset(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 6
        && (b[0] == b'+' || b[0] == b'-')
        && b[1..3].iter().all(u8::is_ascii_digit)
        && b[3] == b':'
        && b[4..6].iter().all(u8::is_ascii_digit)
}

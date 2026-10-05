//! Version 5 raster asset and image-node validation.
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{Map, Value};
use std::{collections::HashSet, io::Cursor, path::Path};

pub const VERSION: u64 = 5;
pub const MAX_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_DOCUMENT_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_DIMENSION: u64 = 32_768;
pub const MAX_PIXELS: u64 = 268_435_456;
pub const MAX_SURFACE_BYTES: u64 = 1_073_741_824;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceInfo {
    pub digest: String,
    pub media_type: String,
    pub byte_length: u64,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub color_space: &'static str,
    pub orientation: u8,
    pub has_alpha: bool,
}

pub fn decode_source(bytes: &[u8]) -> Result<SourceInfo> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_SOURCE_BYTES {
        bail!("[limit-exceeded] image source must contain 1 byte–128 MiB")
    }
    let format =
        image::guess_format(bytes).context("[malformed-resource] unknown image signature")?;
    let (media_type, animation_marker) = match format {
        image::ImageFormat::Png => ("image/png", Some(b"acTL".as_slice())),
        image::ImageFormat::Jpeg => ("image/jpeg", None),
        image::ImageFormat::WebP => ("image/webp", Some(b"ANIM".as_slice())),
        other => bail!("[unsupported-capability] image format {other:?} is unsupported"),
    };
    if animation_marker.is_some_and(|marker| bytes.windows(marker.len()).any(|part| part == marker))
    {
        bail!("[unsupported-capability] animated images require explicit frame selection")
    }
    let reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let (width, height) = reader
        .into_dimensions()
        .context("[malformed-resource] could not read image dimensions")?;
    validate_surface(width as u64, height as u64)?;
    let decoded = image::load_from_memory_with_format(bytes, format)
        .context("[malformed-resource] image decode failed")?;
    let has_alpha = decoded.color().has_alpha();
    let rgba = decoded.into_rgba8();
    if rgba.width() != width || rgba.height() != height {
        bail!("[malformed-resource] decoded image dimensions changed unexpectedly")
    }
    Ok(SourceInfo {
        digest: crate::resource::sha256(bytes),
        media_type: media_type.into(),
        byte_length: bytes.len() as u64,
        pixel_width: width,
        pixel_height: height,
        color_space: "srgb8",
        orientation: 1,
        has_alpha,
    })
}

fn validate_surface(width: u64, height: u64) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        bail!("[limit-exceeded] image dimensions are outside 1–32768")
    }
    let pixels = width
        .checked_mul(height)
        .context("[limit-exceeded] image pixel count overflow")?;
    let bytes = pixels
        .checked_mul(4)
        .context("[limit-exceeded] image surface byte count overflow")?;
    if pixels > MAX_PIXELS || bytes > MAX_SURFACE_BYTES {
        bail!("[limit-exceeded] decoded image surface is too large")
    }
    Ok(())
}

pub fn validate_assets(raw: &Value) -> Result<HashSet<String>> {
    let assets = raw
        .get("image_assets")
        .and_then(Value::as_object)
        .context("[malformed-resource] v5 image_assets must be an object")?;
    let mut total = 0u64;
    let mut ids = HashSet::with_capacity(assets.len());
    for (digest, value) in assets {
        validate_digest(digest)?;
        let asset = value.as_object().with_context(|| {
            format!("[malformed-resource] image asset {digest} must be an object")
        })?;
        let byte_length = integer(asset, "byte_length")?;
        if byte_length == 0 || byte_length > MAX_SOURCE_BYTES {
            bail!("[limit-exceeded] image asset {digest} exceeds the 128 MiB source limit")
        }
        let width = integer(asset, "pixel_width")?;
        let height = integer(asset, "pixel_height")?;
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            bail!("[limit-exceeded] image asset {digest} dimensions are outside 1–32768")
        }
        let pixels = width
            .checked_mul(height)
            .context("[limit-exceeded] image pixel count overflow")?;
        let surface = pixels
            .checked_mul(4)
            .context("[limit-exceeded] image surface byte count overflow")?;
        if pixels > MAX_PIXELS || surface > MAX_SURFACE_BYTES {
            bail!("[limit-exceeded] image asset {digest} decoded surface is too large")
        }
        match string(asset, "media_type")? {
            "image/png" | "image/jpeg" | "image/webp" => {}
            other => bail!("[unsupported-capability] unsupported image media type {other}"),
        }
        if string(asset, "color_space")? != "srgb8" {
            bail!("[unsupported-capability] image asset {digest} is not srgb8")
        }
        if !(1..=8).contains(&integer(asset, "orientation")?) {
            bail!("[malformed-resource] image asset {digest} orientation must be 1–8")
        }
        let storage = asset
            .get("storage")
            .and_then(Value::as_object)
            .with_context(|| {
                format!("[malformed-resource] image asset {digest} storage is missing")
            })?;
        match string(storage, "kind")? {
            "embedded" => {
                total = total
                    .checked_add(byte_length)
                    .context("[limit-exceeded] image source byte total overflow")?;
                if total > MAX_DOCUMENT_SOURCE_BYTES {
                    bail!(
                        "[limit-exceeded] embedded image sources exceed the 512 MiB document limit"
                    )
                }
                if string(storage, "encoding")? != "base64" {
                    bail!("[unsupported-capability] image asset {digest} encoding is unsupported")
                }
                let encoded = string(storage, "data")?;
                let maximum_encoded = byte_length
                    .checked_add(2)
                    .and_then(|n| n.checked_div(3))
                    .and_then(|n| n.checked_mul(4))
                    .context("[limit-exceeded] base64 size overflow")?;
                if encoded.len() as u64 > maximum_encoded {
                    bail!("[limit-exceeded] image asset {digest} encoded data exceeds its declared size")
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .with_context(|| {
                        format!("[malformed-resource] image asset {digest} has invalid base64")
                    })?;
                if bytes.len() as u64 != byte_length {
                    bail!("[malformed-resource] image asset {digest} byte_length does not match embedded data")
                }
                crate::resource::verify(&bytes, digest)?;
            }
            "external" => {
                crate::resource::safe_relative_path(Path::new(string(storage, "path")?))?;
            }
            other => bail!("[unsupported-capability] image storage kind {other} is unsupported"),
        }
        ids.insert(digest.clone());
    }
    Ok(ids)
}

pub fn validate_node(node: &Map<String, Value>, assets: &HashSet<String>) -> Result<()> {
    let id = string(node, "id")?;
    let asset = string(node, "asset")?;
    validate_digest(asset)?;
    if !assets.contains(asset) {
        bail!("[missing-resource] image node {id} references missing asset {asset}")
    }
    for key in ["x", "y"] {
        finite(node, key)?;
    }
    for key in ["width", "height"] {
        if finite(node, key)? <= 0.0 {
            bail!("[malformed-resource] image node {id} {key} must be positive")
        }
    }
    match string(node, "fit")? {
        "fill" | "contain" | "cover" | "none" | "scale-down" => {}
        other => bail!("[unsupported-capability] image node {id} fit {other} is unsupported"),
    }
    normalized_array(node, "position", 2, false)?;
    let crop = normalized_array(node, "crop", 4, true)?;
    if crop[0] + crop[2] > 1.0 || crop[1] + crop[3] > 1.0 {
        bail!("[malformed-resource] image node {id} crop must remain inside the source")
    }
    let opacity = finite(node, "opacity")?;
    if !(0.0..=1.0).contains(&opacity) {
        bail!("[malformed-resource] image node {id} opacity must be in 0–1")
    }
    if string(node, "blend_mode")? != "normal" {
        bail!("[unsupported-capability] image node {id} blend mode is unsupported")
    }
    let operations = node
        .get("operations")
        .and_then(Value::as_array)
        .context("[malformed-resource] image operations must be an array")?;
    if !operations.is_empty() {
        bail!("[unsupported-capability] image operations require the v0.7 operation engine")
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    let hex = value
        .strip_prefix("sha256:")
        .context("[malformed-resource] image digest must start with sha256:")?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("[malformed-resource] image digest must use 64 lowercase hexadecimal digits")
    }
    Ok(())
}

fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("[malformed-resource] {key} must be a string"))
}

fn integer(object: &Map<String, Value>, key: &str) -> Result<u64> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("[malformed-resource] {key} must be a nonnegative integer"))
}

fn finite(object: &Map<String, Value>, key: &str) -> Result<f64> {
    let value = object
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("[malformed-resource] {key} must be a number"))?;
    if !value.is_finite() {
        bail!("[malformed-resource] {key} must be finite")
    }
    Ok(value)
}

fn normalized_array(
    object: &Map<String, Value>,
    key: &str,
    length: usize,
    positive_tail: bool,
) -> Result<Vec<f64>> {
    let values = object
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("[malformed-resource] {key} must be an array"))?;
    if values.len() != length {
        bail!("[malformed-resource] {key} must contain {length} numbers")
    }
    let numbers = values
        .iter()
        .map(|value| value.as_f64().filter(|n| n.is_finite()))
        .collect::<Option<Vec<_>>>()
        .with_context(|| format!("[malformed-resource] {key} values must be finite numbers"))?;
    if numbers.iter().any(|value| !(0.0..=1.0).contains(value))
        || (positive_tail && (numbers[2] <= 0.0 || numbers[3] <= 0.0))
    {
        bail!("[malformed-resource] {key} values are outside their normalized range")
    }
    Ok(numbers)
}

//! Bounded Photoshop document export.
//!
//! This writes an interoperable PSD v1 subset: 8-bit RGBA, uncompressed
//! channels and optional full-canvas raster layers, with no profiles.

use anyhow::{ensure, Context, Result};

const MAX_PSD_BYTES: usize = 256 * 1024 * 1024;
const PSD_DIMENSION_LIMIT: u32 = 30_000;
const PSD_LAYER_LIMIT: usize = 2_000;

/// A full-canvas RGBA raster layer. PSD order is front-to-back.
pub struct RasterLayer<'a> {
    pub name: &'a str,
    pub visible: bool,
    pub rgba: &'a [u8],
}

/// Encode a tightly packed RGBA raster as a flattened, uncompressed PSD v1.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    encode(width, height, &[], rgba)
}

/// Encode a composite and optional full-canvas layers as uncompressed PSD v1.
pub fn encode_layers(
    width: u32,
    height: u32,
    layers: &[RasterLayer<'_>],
    composite_rgba: &[u8],
) -> Result<Vec<u8>> {
    encode(width, height, layers, composite_rgba)
}

fn encode(width: u32, height: u32, layers: &[RasterLayer<'_>], rgba: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        (1..=PSD_DIMENSION_LIMIT).contains(&width) && (1..=PSD_DIMENSION_LIMIT).contains(&height),
        "[limit-exceeded] PSD dimensions must be between 1 and {PSD_DIMENSION_LIMIT}"
    );
    let pixels = usize::try_from(width)
        .context("PSD width does not fit memory address space")?
        .checked_mul(
            usize::try_from(height).context("PSD height does not fit memory address space")?,
        )
        .context("[limit-exceeded] PSD pixel count overflow")?;
    let channel_bytes = pixels
        .checked_mul(4)
        .context("[limit-exceeded] PSD channel size overflow")?;
    ensure!(
        layers.len() <= PSD_LAYER_LIMIT,
        "[limit-exceeded] PSD export supports at most {PSD_LAYER_LIMIT} layers"
    );
    ensure!(
        rgba.len() == channel_bytes,
        "[invalid-input] RGBA buffer has {} bytes; expected {channel_bytes}",
        rgba.len()
    );
    for layer in layers {
        ensure!(
            layer.rgba.len() == channel_bytes,
            "[invalid-input] PSD layer `{}` has {} bytes; expected {channel_bytes}",
            layer.name,
            layer.rgba.len()
        );
        ensure!(
            layer.name.len() <= 255,
            "[invalid-input] PSD layer names must be at most 255 UTF-8 bytes"
        );
    }

    // Preflight the exact serialized size before constructing either the layer
    // section or final output. Layer records have variable name padding, so a
    // per-layer guess can undercount and bypass the output cap.
    let layer_info_len = if layers.is_empty() {
        0
    } else {
        let record_bytes = layers.iter().try_fold(2usize, |sum, layer| {
            let pascal_padded = (layer.name.len() + 1 + 3) & !3;
            let extra_bytes = 8 + pascal_padded;
            sum.checked_add(62 + extra_bytes)
                .context("[limit-exceeded] PSD layer record size overflow")
        })?;
        let channel_bytes = pixels
            .checked_add(2)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_mul(layers.len()))
            .context("[limit-exceeded] PSD layer channel size overflow")?;
        record_bytes
            .checked_add(channel_bytes)
            .context("[limit-exceeded] PSD layer info size overflow")?
    };
    ensure!(
        layer_info_len <= u32::MAX as usize,
        "[limit-exceeded] PSD layer info exceeds format limit"
    );
    let expected = 40usize
        .checked_add(channel_bytes)
        .and_then(|n| {
            n.checked_add(if layers.is_empty() {
                0
            } else {
                8 + layer_info_len
            })
        })
        .context("[limit-exceeded] PSD output size overflow")?;
    ensure!(
        expected <= MAX_PSD_BYTES,
        "[limit-exceeded] PSD output is {expected} bytes; the limit is {MAX_PSD_BYTES}"
    );

    let layer_section = encode_layer_section(width, height, layers, pixels)?;
    let mut output = Vec::with_capacity(expected);
    output.extend_from_slice(b"8BPS");
    output.extend_from_slice(&1u16.to_be_bytes());
    output.extend_from_slice(&[0; 6]);
    output.extend_from_slice(&4u16.to_be_bytes());
    output.extend_from_slice(&height.to_be_bytes());
    output.extend_from_slice(&width.to_be_bytes());
    output.extend_from_slice(&8u16.to_be_bytes());
    output.extend_from_slice(&3u16.to_be_bytes()); // RGB color mode
    output.extend_from_slice(&0u32.to_be_bytes()); // color mode data
    output.extend_from_slice(&0u32.to_be_bytes()); // image resources
    output.extend_from_slice(&(layer_section.len() as u32).to_be_bytes());
    output.extend_from_slice(&layer_section);
    write_planar_rgba(&mut output, rgba);
    debug_assert!(output.len() <= expected);
    Ok(output)
}

fn encode_layer_section(
    width: u32,
    height: u32,
    layers: &[RasterLayer<'_>],
    pixels: usize,
) -> Result<Vec<u8>> {
    if layers.is_empty() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for layer in layers {
        records.extend_from_slice(&0i32.to_be_bytes());
        records.extend_from_slice(&0i32.to_be_bytes());
        records.extend_from_slice(&(height as i32).to_be_bytes());
        records.extend_from_slice(&(width as i32).to_be_bytes());
        records.extend_from_slice(&4u16.to_be_bytes());
        for id in [0i16, 1, 2, -1] {
            records.extend_from_slice(&id.to_be_bytes());
            records.extend_from_slice(&((pixels + 2) as u32).to_be_bytes());
        }
        records.extend_from_slice(b"8BIMnorm");
        records.extend_from_slice(&[255, 0, if layer.visible { 0 } else { 2 }, 0]);
        let mut extra = Vec::new();
        extra.extend_from_slice(&0u32.to_be_bytes());
        extra.extend_from_slice(&0u32.to_be_bytes());
        extra.push(layer.name.len() as u8);
        extra.extend_from_slice(layer.name.as_bytes());
        while extra.len() % 4 != 0 {
            extra.push(0);
        }
        records.extend_from_slice(&(extra.len() as u32).to_be_bytes());
        records.extend_from_slice(&extra);
    }
    let channel_data_len = layers
        .len()
        .checked_mul(4)
        .and_then(|n| n.checked_mul(pixels + 2))
        .context("[limit-exceeded] PSD channel data length overflow")?;
    let info_len = 2usize
        .checked_add(records.len())
        .and_then(|n| n.checked_add(channel_data_len))
        .context("[limit-exceeded] PSD layer info length overflow")?;
    ensure!(
        info_len <= u32::MAX as usize,
        "[limit-exceeded] PSD layer info exceeds format limit"
    );
    let mut section = Vec::new();
    section.extend_from_slice(&(info_len as u32).to_be_bytes());
    section.extend_from_slice(&(layers.len() as i16).to_be_bytes());
    section.extend_from_slice(&records);
    for layer in layers {
        for channel in 0..4 {
            section.extend_from_slice(&0u16.to_be_bytes());
            section.extend(layer.rgba.chunks_exact(4).map(|pixel| pixel[channel]));
        }
    }
    section.extend_from_slice(&0u32.to_be_bytes());
    Ok(section)
}

fn write_planar_rgba(output: &mut Vec<u8>, rgba: &[u8]) {
    output.extend_from_slice(&0u16.to_be_bytes());
    for channel in 0..4 {
        output.extend(rgba.chunks_exact(4).map(|pixel| pixel[channel]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattened_psd_round_trips_composite_pixels_through_importer() {
        let bytes = encode_rgba(2, 1, &[255, 0, 0, 255, 0, 40, 255, 128]).unwrap();
        let imported = crate::psd_import::import(&bytes, "roundtrip.psd").unwrap();
        crate::scene::validate(&imported.document).unwrap();
        assert!(imported
            .document
            .get("image_assets")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|assets| !assets.is_empty()));
        assert!(imported.report.counts.approximated > 0);
    }

    #[test]
    fn flattened_psd_checks_dimensions_and_buffer_length_before_allocating() {
        assert!(encode_rgba(0, 1, &[]).is_err());
        assert!(encode_rgba(1, 1, &[0; 3]).is_err());
        assert!(encode_rgba(30_001, 1, &[]).is_err());
    }

    #[test]
    fn layered_psd_round_trips_names_visibility_and_pixels_through_importer() {
        let back = [255, 0, 0, 255];
        let front = [0, 0, 255, 128];
        let composite = [127, 0, 128, 255];
        let layers = [
            RasterLayer {
                name: "Front",
                visible: true,
                rgba: &front,
            },
            RasterLayer {
                name: "Back",
                visible: false,
                rgba: &back,
            },
        ];
        let bytes = encode_layers(1, 1, &layers, &composite).unwrap();
        let imported = crate::psd_import::import(&bytes, "layered.psd").unwrap();
        crate::scene::validate(&imported.document).unwrap();
        let json = serde_json::to_string(&imported.document).unwrap();
        assert!(json.contains("Front"));
        assert!(json.contains("Back"));
        assert!(bytes.len() <= MAX_PSD_BYTES);
    }

    #[test]
    fn layered_psd_output_cap_counts_full_records_and_name_padding() {
        let pixels = vec![0; 30_000 * 2 * 4];
        let layers = (0..PSD_LAYER_LIMIT)
            .map(|i| RasterLayer {
                name: if i % 2 == 0 { "x" } else { "longer layer name" },
                visible: true,
                rgba: &pixels,
            })
            .collect::<Vec<_>>();
        let error = encode_layers(30_000, 2, &layers, &pixels).unwrap_err();
        assert!(error.to_string().contains("PSD output is"));
    }
}

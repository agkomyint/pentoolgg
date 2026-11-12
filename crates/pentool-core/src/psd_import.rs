//! Bounded import of the deliberately small, deterministic PSD v1 subset.
//!
//! Supported inputs are 8-bit RGB or grayscale PSDs using raw or PackBits
//! channel compression. The importer materializes each ordinary raster layer
//! as a normal embedded Pentool image asset. Basic folder hierarchy and
//! supported blend modes, a bounded raster-mask subset, and legacy RGB
//! Brightness/Contrast and Invert adjustment layers are retained. Other adjustment layers,
//! unsupported masks and blend modes, and color-profile semantics are disclosed,
//! never executed or fetched.

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::interchange::{FidelityReport, FidelityReportBuilder, FidelityStatus};

const MAX_INPUT_BYTES: usize = 512 * 1024 * 1024;
const MAX_DIMENSION: u32 = 16_384;
const MAX_PIXELS: u64 = 100_000_000;
const MAX_LAYERS: usize = 2_000;
const MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;
const MAX_RGBA_BYTES: usize = MAX_DECODED_BYTES / 4;

#[derive(Debug)]
pub struct PsdImport {
    pub document: Value,
    pub report: FidelityReport,
}

#[derive(Debug)]
struct LayerRecord {
    id: usize,
    name: String,
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    opacity: u8,
    visible: bool,
    is_clipped: bool,
    blend: [u8; 4],
    channels: Vec<(i16, Vec<u8>)>,
    has_mask: bool,
    mask_supported: bool,
    mask_bounds: Option<MaskBounds>,
    has_unsupported_effects: bool,
    section_type: Option<u32>,
    adjustment: Option<PsdAdjustment>,
    unsupported_adjustment_data: bool,
}

#[derive(Debug, Clone, Copy)]
enum PsdAdjustment {
    BrightnessContrast { brightness: i16, contrast: i16 },
    Invert,
}

#[derive(Debug, Clone, Copy)]
struct MaskBounds {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    default: u8,
    inverted: bool,
    relative: bool,
}

#[derive(Debug)]
struct ClipBase {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    alpha: Vec<u8>,
}

#[derive(Debug)]
struct LayerExtra {
    name: String,
    has_mask: bool,
    mask_bounds: Option<MaskBounds>,
    has_unsupported_effects: bool,
    section_type: Option<u32>,
    adjustment: Option<PsdAdjustment>,
    unsupported_adjustment_data: bool,
}

#[derive(Debug)]
struct GroupContext {
    marker_id: usize,
    children: Vec<Value>,
}

/// Parse a PSD v1 into a standalone Pentool scene with embedded raster assets.
pub fn import(bytes: &[u8], source: &str) -> Result<PsdImport> {
    ensure!(
        bytes.len() <= MAX_INPUT_BYTES,
        "[limit-exceeded] PSD input exceeds 512 MiB"
    );
    let mut input = Reader::new(bytes);
    ensure!(
        input.take(4)? == b"8BPS",
        "[unsupported-format] not a Photoshop PSD file"
    );
    ensure!(
        input.u16()? == 1,
        "[unsupported-format] only PSD version 1 is supported; PSB is not supported"
    );
    ensure!(
        input.take(6)?.iter().all(|byte| *byte == 0),
        "[invalid-input] PSD reserved header bytes are nonzero"
    );
    let channel_count = input.u16()? as usize;
    let height = input.u32()?;
    let width = input.u32()?;
    let depth = input.u16()?;
    let color_mode = input.u16()?;
    ensure!(
        depth == 8,
        "[unsupported-format] PSD import supports only 8-bit channels"
    );
    ensure!(
        matches!(color_mode, 1 | 3),
        "[unsupported-format] PSD import supports grayscale and RGB modes only"
    );
    let expected_channels = if color_mode == 1 { 1 } else { 3 };
    ensure!(
        channel_count >= expected_channels && channel_count <= 56,
        "[invalid-input] invalid PSD channel count {channel_count}"
    );
    check_dimensions(width, height)?;
    checked_rgba_len(width as usize, height as usize)?;
    let pixels = (width as u64)
        .checked_mul(height as u64)
        .context("[limit-exceeded] PSD pixel count overflow")?;
    ensure!(
        pixels
            .checked_mul(channel_count as u64)
            .is_some_and(|n| n <= (MAX_DECODED_BYTES / 2) as u64),
        "[limit-exceeded] PSD composite channel allocation exceeds limit"
    );

    let color_data_len = input.u32()? as usize;
    input.skip(color_data_len)?;
    let resources_len = input.u32()? as usize;
    input.skip(resources_len)?;
    let layer_mask_len = input.u32()? as usize;
    let layer_mask = input.take(layer_mask_len)?;
    let mut decoded_channel_total = 0usize;
    let layers = parse_layer_mask(layer_mask, &mut decoded_channel_total)?;

    let mut report = FidelityReportBuilder::new("psd", source);
    report.record(
        "$document",
        "color_mode",
        FidelityStatus::Exact,
        if color_mode == 1 {
            "8-bit grayscale samples are expanded to sRGB grayscale"
        } else {
            "8-bit RGB samples are preserved as sRGB"
        },
        None,
    );
    if channel_count > expected_channels + 1 {
        report.record(
            "$document",
            "spot_channels",
            FidelityStatus::Dropped,
            "additional spot or auxiliary channels are not mapped into the Pentool scene",
            Some("convert spot channels to the document color mode before import".into()),
        );
    }
    report.record(
        "$document",
        "resolution",
        FidelityStatus::Approximated,
        "pixel dimensions are retained; PSD resolution metadata and print units are not mapped",
        Some("set document print dimensions after import if required".into()),
    );
    report.record(
        "$document",
        "color_profile",
        FidelityStatus::Approximated,
        "embedded ICC profiles are not applied; samples are interpreted as sRGB",
        Some("convert the source to sRGB before import for predictable color".into()),
    );

    let mut document = crate::scene::migrate_to_v5(crate::scene::new_document(width, height))?;
    let doc_name = std::path::Path::new(source)
        .file_stem()
        .and_then(|v| v.to_str())
        .filter(|v| !v.is_empty())
        .unwrap_or("Imported PSD");
    document["name"] = json!(doc_name);
    document["pages"][0]["canvas"]["background"] = json!("none");
    let mut image_assets = BTreeMap::<String, Value>::new();
    let mut nodes = Vec::new();
    let mut emitted = 0usize;
    let mut has_adjustment = false;
    let mut decoded_total = 0usize;
    let mut encoded_asset_total = 0usize;
    let mut group_stack = Vec::<GroupContext>::new();
    // The current root-level clipping base is retained only while processing
    // its immediately following clipped layers. It is bounded below the same
    // decoded-data ceiling as the layer pixels.
    let mut clip_base: Option<ClipBase> = None;

    // PSD stores layer records in front-to-back order; Pentool paints in array order.
    for layer in layers.into_iter().rev() {
        match layer.section_type {
            Some(3) => {
                clip_base = None;
                ensure!(
                    group_stack.len() < 128,
                    "[limit-exceeded] PSD group nesting exceeds 128"
                );
                group_stack.push(GroupContext {
                    marker_id: layer.id,
                    children: Vec::new(),
                });
                continue;
            }
            Some(1 | 2) => {
                clip_base = None;
                if let Some(context) = group_stack.pop() {
                    close_group(&mut nodes, &mut group_stack, &layer, context, &mut report)?;
                } else {
                    report.record(
                        format!("layer:{}", layer.id),
                        "group_hierarchy",
                        FidelityStatus::Dropped,
                        "PSD folder record had no matching bounding divider",
                        None,
                    );
                }
                continue;
            }
            Some(_) => {
                report.record(
                    format!("layer:{}", layer.id),
                    "group_hierarchy",
                    FidelityStatus::Dropped,
                    "unsupported PSD section-divider type was dropped",
                    None,
                );
                continue;
            }
            None => {}
        }
        if !layer.is_clipped {
            clip_base = None;
        }
        if let Some(adjustment) = layer.adjustment {
            let source_id = format!("layer:{}", layer.id);
            if layer.has_mask || layer.is_clipped || layer.unsupported_adjustment_data {
                report.record(
                    &source_id,
                    "adjustment_layer",
                    FidelityStatus::Dropped,
                    "adjustment settings were recognized, but this subset cannot preserve the layer mask, clipping relationship, or another visual adjustment/effect",
                    Some("rasterize the adjustment with its affected layers in the source".into()),
                );
                continue;
            }
            emitted += 1;
            ensure!(
                emitted <= MAX_LAYERS,
                "[limit-exceeded] PSD layer count exceeds {MAX_LAYERS}"
            );
            has_adjustment = true;
            append_node(
                &mut nodes,
                &mut group_stack,
                adjustment_node(&layer, adjustment),
            );
            let (description, settings) = match adjustment {
                PsdAdjustment::BrightnessContrast {
                    brightness,
                    contrast,
                } => (
                    "legacy PSD brightness/contrast settings",
                    json!({"brightness":brightness,"contrast":contrast}),
                ),
                PsdAdjustment::Invert => ("PSD Invert settings", json!({})),
            };
            report.record(
                &source_id,
                "adjustment_layer",
                FidelityStatus::Approximated,
                format!("{description} mapped to the native v6 scoped adjustment with settings {settings}"),
                Some("compare rendered colors with Photoshop before editing".into()),
            );
            report.record(
                &source_id,
                "visibility_opacity",
                FidelityStatus::Exact,
                "adjustment visibility and opacity are retained",
                None,
            );
            if layer.has_unsupported_effects {
                report.record(
                    &source_id,
                    "unmapped_layer_metadata",
                    FidelityStatus::Dropped,
                    "additional PSD layer metadata was not preserved alongside the mapped adjustment",
                    Some("review the fidelity report and rasterize the layer if the metadata affects its appearance".into()),
                );
            }
            continue;
        }
        if layer.width == 0 || layer.height == 0 {
            report.record(
                format!("layer:{}", layer.id),
                "empty_layer",
                FidelityStatus::Dropped,
                "empty PSD layer has no standalone raster content",
                None,
            );
            continue;
        }
        let required = if color_mode == 1 {
            &[0][..]
        } else {
            &[0, 1, 2][..]
        };
        if required
            .iter()
            .any(|id| !layer.channels.iter().any(|(channel, _)| channel == id))
        {
            report.record(
                format!("layer:{}", layer.id),
                "adjustment_or_nonpixel_layer",
                FidelityStatus::Dropped,
                "layer has no complete supported color-channel set and was not rasterized",
                Some("rasterize the layer or merge its adjustment in the source".into()),
            );
            continue;
        }
        emitted += 1;
        ensure!(
            emitted <= MAX_LAYERS,
            "[limit-exceeded] PSD layer count exceeds {MAX_LAYERS}"
        );
        let rgba_bytes = checked_rgba_len(layer.width, layer.height)?;
        decoded_total = decoded_total
            .checked_add(rgba_bytes)
            .context("[limit-exceeded] decoded byte count overflow")?;
        ensure!(
            decoded_total <= MAX_DECODED_BYTES,
            "[limit-exceeded] decoded PSD layer data exceeds {} bytes",
            MAX_DECODED_BYTES
        );
        let channel_bytes = layer
            .channels
            .iter()
            .try_fold(0usize, |total, (_, channel)| {
                total
                    .checked_add(channel.len())
                    .context("[limit-exceeded] PSD layer channel allocation overflow")
            })?;
        ensure!(
            channel_bytes <= MAX_DECODED_BYTES,
            "[limit-exceeded] decoded PSD layer channels exceed {} bytes",
            MAX_DECODED_BYTES
        );
        let mut rgba = layer_rgba(&layer, color_mode)?;
        let clipping_applied = if layer.is_clipped && group_stack.is_empty() {
            if let Some(base) = clip_base.as_ref() {
                apply_clipping_alpha(
                    &mut rgba,
                    layer.width,
                    layer.height,
                    layer.left,
                    layer.top,
                    base,
                )?;
                true
            } else {
                false
            }
        } else {
            false
        };
        if !layer.is_clipped && group_stack.is_empty() {
            let alpha_bytes = layer
                .width
                .checked_mul(layer.height)
                .context("PSD clipping-base alpha size overflow")?;
            let existing_cache = clip_base.as_ref().map_or(0, |current| current.alpha.len());
            if decoded_total
                .checked_add(existing_cache)
                .and_then(|size| size.checked_add(alpha_bytes))
                .is_some_and(|size| size <= MAX_DECODED_BYTES)
            {
                let opacity = if layer.visible { layer.opacity } else { 0 };
                let alpha = rgba
                    .chunks_exact(4)
                    .map(|pixel| ((u16::from(pixel[3]) * u16::from(opacity) + 127) / 255) as u8)
                    .collect();
                clip_base = Some(ClipBase {
                    left: layer.left,
                    top: layer.top,
                    width: layer.width,
                    height: layer.height,
                    alpha,
                });
            }
        }
        let png = encode_png(layer.width as u32, layer.height as u32, &rgba)?;
        let image_info = crate::image::decode_source(&png)
            .context("encoded PSD layer image failed Pentool image validation")?;
        let asset_id = image_info.digest.clone();
        if !image_assets.contains_key(&asset_id) {
            encoded_asset_total = encoded_asset_total
                .checked_add(png.len())
                .context("[limit-exceeded] embedded image byte count overflow")?;
            ensure!(
                encoded_asset_total <= MAX_DECODED_BYTES,
                "[limit-exceeded] encoded PSD layer assets exceed {} bytes",
                MAX_DECODED_BYTES
            );
        }
        image_assets.entry(asset_id.clone()).or_insert_with(|| {
            json!({
                "media_type": image_info.media_type,
                "byte_length": image_info.byte_length,
                "pixel_width": image_info.pixel_width,
                "pixel_height": image_info.pixel_height,
                "color_space": image_info.color_space,
                "orientation": image_info.orientation,
                "storage": crate::image::embedded_storage(&png)
            })
        });
        let node_id = format!("psd-image-{}", layer.id);
        let mapped_blend = map_blend(&layer.blend);
        let blend_mode = mapped_blend.unwrap_or("normal");
        let image_node = json!({
            "kind":"image", "id":node_id, "asset":asset_id,
            "x":layer.left, "y":layer.top, "width":layer.width, "height":layer.height,
            "fit":"fill", "position":[0.5,0.5], "crop":[0,0,1,1],
            "opacity":layer.opacity as f64 / 255.0, "blend_mode":blend_mode,
            "transform":[1,0,0,1,0,0], "operations":[], "name":layer.name,
            "visible":layer.visible
        });
        append_node(&mut nodes, &mut group_stack, image_node);
        let source_id = format!("layer:{}", layer.id);
        report.record(
            &source_id,
            "raster_pixels",
            FidelityStatus::Exact,
            "8-bit layer pixels and alpha were materialized as an embedded image",
            None,
        );
        report.record(
            &source_id,
            "visibility",
            FidelityStatus::Exact,
            if layer.visible {
                "PSD layer visibility retained"
            } else {
                "hidden PSD layer remains hidden"
            },
            None,
        );
        report.record(
            &source_id,
            "name_bounds_opacity",
            FidelityStatus::Exact,
            "layer name, pixel bounds, and opacity are retained",
            None,
        );
        if layer.has_mask && layer.mask_supported {
            report.record(
                &source_id,
                "layer_mask",
                FidelityStatus::Exact,
            "8-bit raster layer mask was sampled at layer coordinates and multiplied into alpha",
                None,
            );
        } else if layer.has_mask {
            report.record(
                &source_id,
                "layer_mask",
                FidelityStatus::Dropped,
                "layer or vector mask pixels are not applied by this PSD subset",
                Some("apply the mask in the source before import".into()),
            );
        }
        if layer.has_unsupported_effects {
            report.record(
                &source_id,
                "unmapped_layer_metadata",
                FidelityStatus::Dropped,
                "unrecognized PSD layer additional-info blocks were not preserved or evaluated",
                Some("rasterize the affected layer in the source if the metadata changes its appearance".into()),
            );
        }
        if layer.is_clipped && clipping_applied {
            report.record(
                &source_id,
                "clipping_relationship",
                FidelityStatus::Exact,
                "root-level clipped layer alpha was constrained by its preceding base layer",
                None,
            );
        } else if layer.is_clipped {
            report.record(
                &source_id,
                "clipping_relationship",
                FidelityStatus::Dropped,
                "PSD clipping-base relationship is not represented",
                Some("rasterize the clipped layer with its base in the source".into()),
            );
        }
        if let Some(mapped) = mapped_blend {
            if mapped != "normal" {
                report.record(
                    &source_id,
                    "blend_mode",
                    FidelityStatus::Approximated,
                    format!(
                        "PSD blend mode {:?} mapped to Pentool {mapped}",
                        String::from_utf8_lossy(&layer.blend)
                    ),
                    Some("verify the rendered result against the source application".into()),
                );
            } else {
                report.record(
                    &source_id,
                    "blend_mode",
                    FidelityStatus::Exact,
                    "PSD normal blend mode retained",
                    None,
                );
            }
        } else {
            report.record(
                &source_id,
                "blend_mode",
                FidelityStatus::Dropped,
                format!(
                    "PSD blend mode {:?} is not supported and is represented as normal",
                    String::from_utf8_lossy(&layer.blend)
                ),
                Some("flatten this layer in the source to preserve its appearance".into()),
            );
        }
    }

    // Unbalanced folders are recovered by promoting their already-imported children.
    while let Some(context) = group_stack.pop() {
        report.record(
            format!("layer:{}", context.marker_id),
            "group_hierarchy",
            FidelityStatus::Dropped,
            "PSD group had no matching folder record; its imported children were promoted",
            None,
        );
        for child in context.children {
            append_node(&mut nodes, &mut group_stack, child);
        }
    }

    if nodes.is_empty() {
        // A flattened composite is useful when the PSD has no layer records.
        // It remains a single editable image, and its vector structure is reported lost.
        let compression_offset = input.position();
        ensure!(
            compression_offset < bytes.len(),
            "[invalid-input] PSD has no image data"
        );
        let composite_channel_bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(channel_count))
            .context("[limit-exceeded] PSD composite channel allocation overflow")?;
        reserve_decoded_bytes(&mut decoded_channel_total, composite_channel_bytes)?;
        let composite = decode_composite(
            &mut input,
            width as usize,
            height as usize,
            channel_count,
            color_mode,
        )?;
        let png = encode_png(width, height, &composite)?;
        let info = crate::image::decode_source(&png)?;
        let asset_id = info.digest.clone();
        encoded_asset_total = encoded_asset_total
            .checked_add(png.len())
            .context("[limit-exceeded] embedded image byte count overflow")?;
        ensure!(
            encoded_asset_total <= MAX_DECODED_BYTES,
            "[limit-exceeded] encoded PSD image assets exceed {} bytes",
            MAX_DECODED_BYTES
        );
        image_assets.insert(
            asset_id.clone(),
            json!({
                "media_type":info.media_type, "byte_length":info.byte_length,
                "pixel_width":info.pixel_width, "pixel_height":info.pixel_height,
                "color_space":info.color_space, "orientation":info.orientation,
                "storage":crate::image::embedded_storage(&png)
            }),
        );
        nodes.push(json!({"kind":"image","id":"psd-composite","asset":asset_id,
            "x":0,"y":0,"width":width,"height":height,"fit":"fill",
            "position":[0.5,0.5],"crop":[0,0,1,1],"opacity":1,"blend_mode":"normal",
            "transform":[1,0,0,1,0,0],"operations":[],"name":"Flattened composite"}));
        report.record(
            "$document",
            "flattened_composite",
            FidelityStatus::Approximated,
            "document imported as one raster image; editable layer structure is unavailable",
            None,
        );
    }
    if has_adjustment {
        document = crate::composite::migrate(document)?;
    }
    document["image_assets"] = Value::Object(image_assets.into_iter().collect());
    document["pages"][0]["layers"][0]["id"] = json!("psd-layers");
    document["pages"][0]["layers"][0]["name"] = json!("PSD Layers");
    document["pages"][0]["layers"][0]["nodes"] = Value::Array(nodes);
    crate::scene::validate(&document).context("PSD import produced an invalid Pentool scene")?;
    Ok(PsdImport {
        document,
        report: report.finish(),
    })
}

fn append_node(root: &mut Vec<Value>, groups: &mut [GroupContext], node: Value) {
    if let Some(parent) = groups.last_mut() {
        parent.children.push(node);
    } else {
        root.push(node);
    }
}

fn close_group(
    root: &mut Vec<Value>,
    groups: &mut [GroupContext],
    folder: &LayerRecord,
    context: GroupContext,
    report: &mut FidelityReportBuilder,
) -> Result<()> {
    let pass_through = folder.blend == *b"pass";
    let mapped_blend = if pass_through {
        Some("normal")
    } else {
        map_blend(&folder.blend)
    };
    let node = json!({
        "kind":"group",
        "id":format!("psd-group-{}", folder.id),
        "name":folder.name,
        "children":context.children,
        "visible":folder.visible,
        "opacity":folder.opacity as f64 / 255.0,
        "blend_mode":mapped_blend.unwrap_or("normal"),
        "isolation":if pass_through { "pass-through" } else { "isolated" }
    });
    append_node(root, groups, node);
    report.record(
        format!("layer:{}", folder.id),
        "group_hierarchy",
        FidelityStatus::Exact,
        format!(
            "PSD group children, name, visibility, and opacity retained (divider layer {})",
            context.marker_id
        ),
        None,
    );
    if pass_through {
        report.record(
            format!("layer:{}", folder.id),
            "group_isolation",
            FidelityStatus::Approximated,
            "PSD pass-through group mapped to Pentool pass-through isolation",
            None,
        );
    }
    if let Some(mode) = mapped_blend {
        report.record(
            format!("layer:{}", folder.id),
            "blend_mode",
            if mode == "normal" {
                FidelityStatus::Exact
            } else {
                FidelityStatus::Approximated
            },
            format!("PSD group blend mode mapped to Pentool {mode}"),
            None,
        );
    } else {
        report.record(
            format!("layer:{}", folder.id),
            "blend_mode",
            FidelityStatus::Dropped,
            format!(
                "PSD group blend mode {:?} is represented as normal",
                String::from_utf8_lossy(&folder.blend)
            ),
            Some("flatten the group in the source to preserve its appearance".into()),
        );
    }
    Ok(())
}

fn map_blend(code: &[u8; 4]) -> Option<&'static str> {
    Some(match code {
        b"norm" => "normal",
        b"mul " => "multiply",
        b"scrn" => "screen",
        b"over" => "overlay",
        b"dark" => "darken",
        b"lite" => "lighten",
        b"div " => "color-dodge",
        b"idiv" => "color-burn",
        b"hLit" => "hard-light",
        b"sLit" => "soft-light",
        b"diff" => "difference",
        b"smud" => "exclusion",
        b"hue " => "hue",
        b"sat " => "saturation",
        b"colr" => "color",
        b"lum " => "luminosity",
        _ => return None,
    })
}

fn check_dimensions(width: u32, height: u32) -> Result<()> {
    ensure!(
        width > 0 && height > 0 && width <= MAX_DIMENSION && height <= MAX_DIMENSION,
        "[limit-exceeded] PSD dimensions must be 1..={MAX_DIMENSION} per axis"
    );
    ensure!(
        (width as u64) * (height as u64) <= MAX_PIXELS,
        "[limit-exceeded] PSD exceeds {MAX_PIXELS} pixels"
    );
    Ok(())
}

fn checked_rgba_len(width: usize, height: usize) -> Result<usize> {
    let bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("[limit-exceeded] PSD RGBA allocation overflow")?;
    ensure!(
        bytes <= MAX_RGBA_BYTES,
        "[limit-exceeded] PSD RGBA allocation exceeds {} bytes",
        MAX_RGBA_BYTES
    );
    Ok(bytes)
}

fn parse_layer_mask(bytes: &[u8], decoded_channel_total: &mut usize) -> Result<Vec<LayerRecord>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(bytes);
    let info_len = r.u32()? as usize;
    if info_len == 0 {
        return Ok(Vec::new());
    }
    let block = r.take(info_len)?;
    let global_mask_len = if r.remaining() >= 4 {
        r.u32()? as usize
    } else {
        0
    };
    r.skip(global_mask_len)?;
    // Additional layer-info blocks may contain profiles, patterns, or unknown data.
    // They are intentionally ignored after strict bounds checks.
    let mut block_reader = Reader::new(block);
    let signed_count = block_reader.i16()?;
    let count = signed_count.unsigned_abs() as usize;
    ensure!(
        count <= MAX_LAYERS,
        "[limit-exceeded] PSD declares {count} layers; limit is {MAX_LAYERS}"
    );
    let mut records = Vec::with_capacity(count);
    let mut channel_metadata = Vec::with_capacity(count);
    for index in 0..count {
        let top = block_reader.i32()?;
        let left = block_reader.i32()?;
        let bottom = block_reader.i32()?;
        let right = block_reader.i32()?;
        let width = (right as i64 - left as i64).max(0) as usize;
        let height = (bottom as i64 - top as i64).max(0) as usize;
        ensure!(
            width <= MAX_DIMENSION as usize && height <= MAX_DIMENSION as usize,
            "[limit-exceeded] PSD layer {index} bounds exceed dimension limit"
        );
        let channel_count = block_reader.u16()? as usize;
        ensure!(
            channel_count <= 56,
            "[invalid-input] PSD layer has too many channels"
        );
        let mut channel_meta = Vec::with_capacity(channel_count);
        for _ in 0..channel_count {
            channel_meta.push((block_reader.i16()?, block_reader.u32()? as usize));
        }
        let decoded_channels = channel_meta
            .iter()
            .filter(|(id, _)| matches!(*id, -2..=2))
            .count();
        ensure!(
            width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(decoded_channels))
                .is_some_and(|n| n <= MAX_DECODED_BYTES / 2),
            "[limit-exceeded] PSD layer channel allocation exceeds budget"
        );
        ensure!(
            block_reader.take(4)? == b"8BIM",
            "[invalid-input] invalid layer blend signature"
        );
        let blend = block_reader.take(4)?.try_into().unwrap();
        let opacity = block_reader.u8()?;
        let clipping = block_reader.u8()?;
        let flags = block_reader.u8()?;
        block_reader.skip(1)?;
        let extra_len = block_reader.u32()? as usize;
        let extra = block_reader.take(extra_len)?;
        let extra = parse_layer_extra(extra)?;
        let mask_supported = extra.mask_bounds.is_some();
        channel_metadata.push(channel_meta);
        records.push(LayerRecord {
            id: index,
            name: extra.name,
            left,
            top,
            width,
            height,
            opacity,
            visible: flags & 0x02 == 0,
            is_clipped: clipping != 0,
            blend,
            channels: Vec::new(),
            has_mask: extra.has_mask,
            mask_supported,
            mask_bounds: extra.mask_bounds,
            has_unsupported_effects: extra.has_unsupported_effects,
            section_type: extra.section_type,
            adjustment: extra.adjustment,
            unsupported_adjustment_data: extra.unsupported_adjustment_data,
        });
    }
    // PSD stores all layer records first, followed by each record's channel
    // image data in the same order. Do not consume channel bytes while parsing
    // the records, or a second layer is mistaken for channel data.
    for (record, channel_meta) in records.iter_mut().zip(channel_metadata) {
        for (channel_id, data_len) in channel_meta {
            let channel_data = block_reader.take(data_len)?;
            if record.width == 0 || record.height == 0 {
                continue;
            }
            if matches!(channel_id, -2..=2) {
                if channel_id == -2 && !record.mask_supported {
                    continue;
                }
                let (channel_width, channel_height) = if channel_id == -2 {
                    let bounds = record.mask_bounds.expect("supported mask has bounds");
                    (bounds.width, bounds.height)
                } else {
                    (record.width, record.height)
                };
                let decoded = decode_channel_with_budget(
                    channel_data,
                    channel_width,
                    channel_height,
                    decoded_channel_total,
                )?;
                record.channels.push((channel_id, decoded));
            }
        }
    }
    Ok(records)
}

fn parse_layer_extra(bytes: &[u8]) -> Result<LayerExtra> {
    let mut r = Reader::new(bytes);
    let mask_len = r.u32()? as usize;
    let mask_data = r.take(mask_len)?;
    let mask_bounds = if mask_data.len() == 18 {
        let top = i32::from_be_bytes(mask_data[0..4].try_into().unwrap());
        let left = i32::from_be_bytes(mask_data[4..8].try_into().unwrap());
        let bottom = i32::from_be_bytes(mask_data[8..12].try_into().unwrap());
        let right = i32::from_be_bytes(mask_data[12..16].try_into().unwrap());
        let flags = mask_data[17];
        let width = right as i64 - left as i64;
        let height = bottom as i64 - top as i64;
        // Bit 0 makes bounds relative to the layer origin; bit 2 inverts the
        // sampled mask. Disabled masks and extended parameter data remain out
        // of this subset because they alter which channel data is authoritative.
        if flags & !0x05 == 0
            && width > 0
            && height > 0
            && width <= i64::from(MAX_DIMENSION)
            && height <= i64::from(MAX_DIMENSION)
            && (width as u64) * (height as u64) <= MAX_PIXELS
        {
            Some(MaskBounds {
                left,
                top,
                width: width as usize,
                height: height as usize,
                default: mask_data[16],
                inverted: flags & 0x04 != 0,
                relative: flags & 0x01 != 0,
            })
        } else {
            None
        }
    } else {
        None
    };
    let ranges_len = r.u32()? as usize;
    r.skip(ranges_len)?;
    let pascal_len = r.u8()? as usize;
    let raw_name = r.take(pascal_len)?;
    let padded = (pascal_len + 1 + 3) & !3;
    r.skip(padded - pascal_len - 1)?;
    let mut name = String::from_utf8_lossy(raw_name).into_owned();
    let mut section = None;
    let mut adjustment = None;
    let mut unsupported_adjustment_data = false;
    let mut has_unsupported_effects = ranges_len > 0;
    while r.remaining() >= 12 {
        let signature = r.take(4)?;
        if signature != b"8BIM" && signature != b"8B64" {
            break;
        }
        let key: [u8; 4] = r.take(4)?.try_into().unwrap();
        let len = r.u32()? as usize;
        let payload = r.take(len)?;
        if key == *b"luni" && payload.len() >= 4 {
            let chars = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
            if chars
                .checked_mul(2)
                .is_some_and(|n| n <= payload.len().saturating_sub(4))
            {
                let words = payload[4..4 + chars * 2]
                    .chunks_exact(2)
                    .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
                name = char::decode_utf16(words)
                    .map(|v| v.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect();
            }
        } else if key == *b"brit" {
            if let Some(parsed) = parse_brightness_contrast(payload) {
                if adjustment.replace(parsed).is_some() {
                    has_unsupported_effects = true;
                    unsupported_adjustment_data = true;
                }
            } else {
                has_unsupported_effects = true;
                unsupported_adjustment_data = true;
            }
        } else if key == *b"nvrt" {
            if payload.is_empty() {
                if adjustment.replace(PsdAdjustment::Invert).is_some() {
                    has_unsupported_effects = true;
                    unsupported_adjustment_data = true;
                }
            } else {
                has_unsupported_effects = true;
                unsupported_adjustment_data = true;
            }
        } else if is_unsupported_visual_adjustment_or_effect(&key) {
            has_unsupported_effects = true;
            unsupported_adjustment_data = true;
        } else if key == *b"lsct" || key == *b"lsdk" {
            if payload.len() >= 4 {
                section = Some(u32::from_be_bytes(payload[..4].try_into().unwrap()));
            }
        } else {
            has_unsupported_effects = true;
        }
        if len & 1 == 1 {
            r.skip(1)?;
        }
    }
    Ok(LayerExtra {
        name: if name.is_empty() {
            "Layer".into()
        } else {
            name
        },
        has_mask: mask_len > 0,
        mask_bounds,
        has_unsupported_effects,
        section_type: section,
        adjustment,
        unsupported_adjustment_data,
    })
}

fn is_unsupported_visual_adjustment_or_effect(key: &[u8; 4]) -> bool {
    matches!(
        key,
        b"SoCo"
            | b"GdFl"
            | b"PtFl"
            | b"levl"
            | b"curv"
            | b"expA"
            | b"vibA"
            | b"hue "
            | b"hue2"
            | b"blnc"
            | b"blwh"
            | b"phfl"
            | b"mixr"
            | b"clrL"
            | b"nvrt"
            | b"post"
            | b"thrs"
            | b"grdm"
            | b"selc"
            | b"lrFX"
            | b"vmsk"
            | b"vsms"
    )
}

fn adjustment_node(layer: &LayerRecord, adjustment: PsdAdjustment) -> Value {
    let (kind, params) = match adjustment {
        PsdAdjustment::BrightnessContrast {
            brightness,
            contrast,
        } => (
            "brightness-contrast",
            json!({"brightness":brightness,"contrast":contrast}),
        ),
        PsdAdjustment::Invert => ("invert", json!({})),
    };
    json!({
        "kind":"adjustment", "id":format!("psd-adjustment-{}", layer.id), "name":layer.name,
        "adjustment":kind, "params":params,
        "scope":{"kind":"below"}, "enabled":layer.visible,
        "opacity":layer.opacity as f64 / 255.0, "blend_mode":"normal"
    })
}

/// Parse the seven-byte legacy RGB Brightness/Contrast block from Adobe's PSD
/// specification. Non-neutral mean and Lab-only modes use different semantics
/// and are intentionally left to the fidelity report as unsupported metadata.
fn parse_brightness_contrast(payload: &[u8]) -> Option<PsdAdjustment> {
    if payload.len() != 7 {
        return None;
    }
    let brightness = i16::from_be_bytes([payload[0], payload[1]]);
    let contrast = i16::from_be_bytes([payload[2], payload[3]]);
    let mean = u16::from_be_bytes([payload[4], payload[5]]);
    let lab_only = payload[6];
    ((-100..=100).contains(&brightness)
        && (-100..=100).contains(&contrast)
        && mean == 128
        && lab_only == 0)
        .then_some(PsdAdjustment::BrightnessContrast {
            brightness,
            contrast,
        })
}

fn layer_rgba(layer: &LayerRecord, color_mode: u16) -> Result<Vec<u8>> {
    let pixel_count = layer
        .width
        .checked_mul(layer.height)
        .context("PSD layer pixel overflow")?;
    let rgba_bytes = checked_rgba_len(layer.width, layer.height)?;
    let get = |id| {
        layer
            .channels
            .iter()
            .find(|(channel, _)| *channel == id)
            .map(|(_, data)| data.as_slice())
    };
    let alpha = get(-1);
    let mask = if layer.mask_supported {
        Some(get(-2).context(
            "[invalid-input] PSD layer declares supported mask metadata without mask pixels",
        )?)
    } else {
        None
    };
    let red = get(0).context("[invalid-input] PSD layer lacks its first color channel")?;
    let green = if color_mode == 3 {
        Some(get(1).context("[invalid-input] RGB PSD layer lacks green channel")?)
    } else {
        None
    };
    let blue = if color_mode == 3 {
        Some(get(2).context("[invalid-input] RGB PSD layer lacks blue channel")?)
    } else {
        None
    };
    let mut rgba = Vec::with_capacity(rgba_bytes);
    for i in 0..pixel_count {
        let r = red[i];
        let (g, b) = if color_mode == 1 {
            (r, r)
        } else {
            (green.unwrap()[i], blue.unwrap()[i])
        };
        let source_alpha = alpha.map_or(255, |channel| channel[i]);
        let mask_alpha = mask
            .zip(layer.mask_bounds)
            .map_or(255, |(channel, bounds)| {
                let x = i % layer.width;
                let y = i / layer.width;
                let document_x = i64::from(layer.left) + x as i64;
                let document_y = i64::from(layer.top) + y as i64;
                let mask_origin_x = i64::from(bounds.left)
                    + if bounds.relative {
                        i64::from(layer.left)
                    } else {
                        0
                    };
                let mask_origin_y = i64::from(bounds.top)
                    + if bounds.relative {
                        i64::from(layer.top)
                    } else {
                        0
                    };
                let mask_x = document_x - mask_origin_x;
                let mask_y = document_y - mask_origin_y;
                if mask_x >= 0
                    && mask_y >= 0
                    && mask_x < bounds.width as i64
                    && mask_y < bounds.height as i64
                {
                    channel[mask_y as usize * bounds.width + mask_x as usize]
                } else {
                    bounds.default
                }
            });
        let mask_alpha = if layer.mask_bounds.is_some_and(|bounds| bounds.inverted) {
            255 - mask_alpha
        } else {
            mask_alpha
        };
        let alpha = ((u16::from(source_alpha) * u16::from(mask_alpha) + 127) / 255) as u8;
        rgba.extend_from_slice(&[r, g, b, alpha]);
    }
    Ok(rgba)
}

fn apply_clipping_alpha(
    rgba: &mut [u8],
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    base: &ClipBase,
) -> Result<()> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("[limit-exceeded] clipped PSD layer dimensions overflow")?;
    ensure!(
        rgba.len() == expected,
        "[invalid-input] clipped PSD layer RGBA size mismatch"
    );
    let base_pixels = base
        .width
        .checked_mul(base.height)
        .context("[limit-exceeded] PSD clipping base dimensions overflow")?;
    ensure!(
        base.alpha.len() == base_pixels,
        "[invalid-input] PSD clipping base alpha size mismatch"
    );
    for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        let x = index % width;
        let y = index / width;
        let base_x = i64::from(left) + x as i64 - i64::from(base.left);
        let base_y = i64::from(top) + y as i64 - i64::from(base.top);
        let base_alpha = if base_x >= 0
            && base_y >= 0
            && base_x < base.width as i64
            && base_y < base.height as i64
        {
            base.alpha[base_y as usize * base.width + base_x as usize]
        } else {
            0
        };
        pixel[3] = ((u16::from(pixel[3]) * u16::from(base_alpha) + 127) / 255) as u8;
    }
    Ok(())
}

#[cfg(test)]
fn decode_channel(encoded: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    let mut decoded_total = 0;
    decode_channel_with_budget(encoded, width, height, &mut decoded_total)
}

fn decode_channel_with_budget(
    encoded: &[u8],
    width: usize,
    height: usize,
    decoded_total: &mut usize,
) -> Result<Vec<u8>> {
    let mut r = Reader::new(encoded);
    let compression = r.u16()?;
    let expected = width
        .checked_mul(height)
        .context("PSD channel allocation overflow")?;
    // Charge before allocating or expanding the channel. Parsing fails atomically,
    // so keeping the charge on a later malformed-channel error is harmless.
    reserve_decoded_bytes(decoded_total, expected)?;
    match compression {
        0 => {
            let data = r.take(expected)?;
            ensure!(
                r.remaining() == 0,
                "[invalid-input] PSD raw channel has trailing bytes"
            );
            Ok(data.to_vec())
        }
        1 => {
            let mut row_lengths = Vec::with_capacity(height);
            for _ in 0..height {
                row_lengths.push(r.u16()? as usize);
            }
            let mut output = Vec::with_capacity(expected);
            for row_len in row_lengths {
                let row = r.take(row_len)?;
                let decoded = unpackbits(row, width)?;
                output.extend_from_slice(&decoded);
            }
            ensure!(
                output.len() == expected,
                "[invalid-input] PSD PackBits channel length mismatch"
            );
            ensure!(
                r.remaining() == 0,
                "[invalid-input] PSD PackBits channel has trailing bytes"
            );
            Ok(output)
        }
        other => bail!(
            "[unsupported-format] PSD channel compression {other} is unsupported; use raw or PackBits"
        ),
    }
}

fn reserve_decoded_bytes(total: &mut usize, amount: usize) -> Result<()> {
    let next_total = total
        .checked_add(amount)
        .context("[limit-exceeded] PSD decoded channel byte count overflow")?;
    ensure!(
        next_total <= MAX_DECODED_BYTES,
        "[limit-exceeded] cumulative decoded PSD channel data exceeds {} bytes",
        MAX_DECODED_BYTES
    );
    *total = next_total;
    Ok(())
}

fn decode_composite(
    r: &mut Reader<'_>,
    width: usize,
    height: usize,
    channels: usize,
    color_mode: u16,
) -> Result<Vec<u8>> {
    let compression = r.u16()?;
    let pixels = width
        .checked_mul(height)
        .context("PSD composite pixel overflow")?;
    checked_rgba_len(width, height)?;
    let mut planes = Vec::with_capacity(channels);
    match compression {
        0 => {
            for _ in 0..channels {
                planes.push(r.take(pixels)?.to_vec());
            }
        }
        1 => {
            let mut lengths = Vec::with_capacity(channels * height);
            for _ in 0..channels * height {
                lengths.push(r.u16()? as usize);
            }
            for _ in 0..channels {
                let mut plane = Vec::with_capacity(pixels);
                for _ in 0..height {
                    let bytes =
                        r.take(lengths[planes.len() * height + plane.len() / width.max(1)])?;
                    plane.extend_from_slice(&unpackbits(bytes, width)?);
                }
                planes.push(plane);
            }
        }
        other => bail!(
            "[unsupported-format] PSD composite compression {other} is unsupported; use raw or PackBits"
        ),
    }
    let mut rgba = Vec::with_capacity(
        pixels
            .checked_mul(4)
            .context("PSD composite allocation overflow")?,
    );
    for i in 0..pixels {
        let (red, green, blue) = if color_mode == 1 {
            let v = planes[0][i];
            (v, v, v)
        } else {
            (planes[0][i], planes[1][i], planes[2][i])
        };
        let alpha = planes
            .get(if color_mode == 1 { 1 } else { 3 })
            .map_or(255, |plane| plane[i]);
        rgba.extend_from_slice(&[red, green, blue, alpha]);
    }
    Ok(rgba)
}

fn unpackbits(encoded: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(expected);
    let mut i = 0;
    while i < encoded.len() {
        let n = encoded[i] as i8;
        i += 1;
        if n >= 0 {
            let count = n as usize + 1;
            ensure!(
                count <= encoded.len().saturating_sub(i)
                    && count <= expected.saturating_sub(output.len()),
                "[invalid-input] malformed PSD PackBits literal run"
            );
            output.extend_from_slice(&encoded[i..i + count]);
            i += count;
        } else if n != -128 {
            ensure!(
                i < encoded.len(),
                "[invalid-input] truncated PSD PackBits repeat run"
            );
            let count = (1i16 - n as i16) as usize;
            ensure!(
                count <= expected.saturating_sub(output.len()),
                "[invalid-input] PSD PackBits row expands beyond width"
            );
            output.resize(output.len() + count, encoded[i]);
            i += 1;
        }
    }
    ensure!(
        output.len() == expected,
        "[invalid-input] PSD PackBits row decoded to {} bytes, expected {expected}",
        output.len()
    );
    Ok(output)
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let expected = checked_rgba_len(width as usize, height as usize)?;
    ensure!(rgba.len() == expected, "internal PSD RGBA length mismatch");
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .context("failed to construct PSD layer image")?;
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut output, image::ImageFormat::Png)?;
    Ok(output.into_inner())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn position(&self) -> usize {
        self.offset
    }
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .context("[invalid-input] PSD offset overflow")?;
        ensure!(
            end <= self.bytes.len(),
            "[invalid-input] truncated PSD at byte {}",
            self.offset
        );
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }
    fn skip(&mut self, count: usize) -> Result<()> {
        self.take(count).map(|_| ())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_size_helper_checks_budget_and_arithmetic_without_allocating() {
        assert_eq!(checked_rgba_len(4096, 4096).unwrap(), MAX_RGBA_BYTES);
        assert!(checked_rgba_len(4097, 4096).is_err());
        assert!(checked_rgba_len(usize::MAX, 2).is_err());
    }

    #[test]
    fn packbits_decodes_literals_repeats_and_noop() {
        assert_eq!(
            unpackbits(&[2, 4, 5, 6, 0xfe, 9, 0x80], 6).unwrap(),
            [4, 5, 6, 9, 9, 9]
        );
    }

    #[test]
    fn packbits_rejects_rows_that_overrun_or_underfill() {
        assert!(unpackbits(&[3, 1], 3).is_err());
        assert!(unpackbits(&[1, 1, 2], 3).is_err());
    }

    #[test]
    fn channel_decoder_handles_raw_and_packbits_rows() {
        assert_eq!(decode_channel(&[0, 0, 7, 8], 2, 1).unwrap(), [7, 8]);
        assert_eq!(
            decode_channel(&[0, 1, 0, 3, 1, 4, 5], 2, 1).unwrap(),
            [4, 5]
        );
    }

    #[test]
    fn channel_decoder_rejects_cumulative_expansion_before_allocating() {
        let mut decoded_total = MAX_DECODED_BYTES - 1;
        let error =
            decode_channel_with_budget(&[0, 0, 7, 8], 2, 1, &mut decoded_total).unwrap_err();
        assert!(error
            .to_string()
            .contains("cumulative decoded PSD channel data"));
        assert_eq!(decoded_total, MAX_DECODED_BYTES - 1);
    }

    #[test]
    fn blend_codes_map_only_to_supported_pentool_modes() {
        assert_eq!(map_blend(b"mul "), Some("multiply"));
        assert_eq!(map_blend(b"hLit"), Some("hard-light"));
        assert_eq!(map_blend(b"norm"), Some("normal"));
        assert_eq!(map_blend(b"diss"), None);
    }

    #[test]
    fn offset_raster_mask_uses_default_outside_its_rectangle() {
        let layer = LayerRecord {
            id: 0,
            name: "masked".into(),
            left: 0,
            top: 0,
            width: 2,
            height: 1,
            opacity: 255,
            visible: true,
            is_clipped: false,
            blend: *b"norm",
            channels: vec![(-1, vec![255, 255]), (0, vec![40, 60]), (-2, vec![128, 0])],
            has_mask: true,
            mask_supported: true,
            mask_bounds: Some(MaskBounds {
                left: 1,
                top: 0,
                width: 1,
                height: 1,
                default: 0,
                inverted: false,
                relative: false,
            }),
            has_unsupported_effects: false,
            section_type: None,
            adjustment: None,
            unsupported_adjustment_data: false,
        };

        assert_eq!(
            layer_rgba(&layer, 1).unwrap(),
            [40, 40, 40, 0, 60, 60, 60, 128]
        );
    }

    #[test]
    fn inverted_offset_raster_mask_inverts_channel_and_default() {
        let layer = LayerRecord {
            id: 0,
            name: "inverted-mask".into(),
            left: 0,
            top: 0,
            width: 2,
            height: 1,
            opacity: 255,
            visible: true,
            is_clipped: false,
            blend: *b"norm",
            channels: vec![(-1, vec![255, 255]), (0, vec![40, 60]), (-2, vec![64, 192])],
            has_mask: true,
            mask_supported: true,
            mask_bounds: Some(MaskBounds {
                left: 1,
                top: 0,
                width: 1,
                height: 1,
                default: 0,
                inverted: true,
                relative: false,
            }),
            has_unsupported_effects: false,
            section_type: None,
            adjustment: None,
            unsupported_adjustment_data: false,
        };

        // The first pixel is outside the mask rectangle, so the default 0 is
        // inverted to full opacity. The second samples 64 and becomes 191.
        assert_eq!(
            layer_rgba(&layer, 1).unwrap(),
            [40, 40, 40, 255, 60, 60, 60, 191]
        );
    }

    #[test]
    fn relative_offset_raster_mask_is_sampled_from_layer_origin() {
        let layer = LayerRecord {
            id: 0,
            name: "relative-mask".into(),
            left: 10,
            top: 20,
            width: 2,
            height: 1,
            opacity: 255,
            visible: true,
            is_clipped: false,
            blend: *b"norm",
            channels: vec![(0, vec![40, 60]), (-2, vec![128])],
            has_mask: true,
            mask_supported: true,
            mask_bounds: Some(MaskBounds {
                left: 0,
                top: 0,
                width: 1,
                height: 1,
                default: 0,
                inverted: false,
                relative: true,
            }),
            has_unsupported_effects: false,
            section_type: None,
            adjustment: None,
            unsupported_adjustment_data: false,
        };

        assert_eq!(
            layer_rgba(&layer, 1).unwrap(),
            [40, 40, 40, 128, 60, 60, 60, 0]
        );
    }

    #[test]
    fn standard_and_inverted_offset_mask_metadata_are_supported() {
        let mut extra = Vec::new();
        extra.extend_from_slice(&18u32.to_be_bytes());
        extra.extend_from_slice(&0i32.to_be_bytes()); // top
        extra.extend_from_slice(&1i32.to_be_bytes()); // left
        extra.extend_from_slice(&1i32.to_be_bytes()); // bottom
        extra.extend_from_slice(&2i32.to_be_bytes()); // right
        extra.push(0); // default outside mask bounds
        extra.push(0); // flags
        extra.extend_from_slice(&0u32.to_be_bytes()); // blending ranges
        extra.push(0); // empty Pascal name
        extra.extend_from_slice(&[0; 3]); // Pascal padding

        let parsed = parse_layer_extra(&extra).unwrap();
        assert!(parsed.has_mask);
        let bounds = parsed.mask_bounds.unwrap();
        assert_eq!(bounds.left, 1);
        assert!(!bounds.inverted);

        extra[21] = 0x04; // invert mask flag
        let parsed = parse_layer_extra(&extra).unwrap();
        assert!(parsed.mask_bounds.unwrap().inverted);

        extra[21] = 0x01; // bounds are relative to layer origin
        let parsed = parse_layer_extra(&extra).unwrap();
        assert!(parsed.mask_bounds.unwrap().relative);

        extra[21] = 0x05; // relative and inverted
        let parsed = parse_layer_extra(&extra).unwrap();
        let bounds = parsed.mask_bounds.unwrap();
        assert!(bounds.relative);
        assert!(bounds.inverted);

        extra[21] = 0x02; // disabled mask flag
        let parsed = parse_layer_extra(&extra).unwrap();
        assert!(parsed.mask_bounds.is_none());
    }

    #[test]
    fn hostile_raster_mask_bounds_and_unknown_flags_stay_unsupported() {
        // PSD bounds are signed 32-bit document coordinates. Extreme but
        // well-formed coordinates must not wrap when width/height are formed,
        // and unknown mask flags must never be treated as an ordinary mask.
        for (top, left, bottom, right, flags) in [
            (i32::MIN, 0, i32::MAX, 1, 0),
            (0, i32::MIN, 1, i32::MAX, 0),
            (0, 0, 1, 1, 0x08),
        ] {
            let mut extra = Vec::new();
            extra.extend_from_slice(&18u32.to_be_bytes());
            extra.extend_from_slice(&top.to_be_bytes());
            extra.extend_from_slice(&left.to_be_bytes());
            extra.extend_from_slice(&bottom.to_be_bytes());
            extra.extend_from_slice(&right.to_be_bytes());
            extra.push(255); // default outside bounds
            extra.push(flags);
            extra.extend_from_slice(&0u32.to_be_bytes()); // no blending ranges
            extra.extend_from_slice(&[0, 0, 0, 0]); // empty Pascal name and padding

            let parsed = parse_layer_extra(&extra).unwrap();
            assert!(parsed.has_mask);
            assert!(parsed.mask_bounds.is_none());
        }
    }

    #[test]
    fn legacy_brightness_contrast_block_requires_supported_rgb_defaults() {
        let mut extra = Vec::new();
        extra.extend_from_slice(&0u32.to_be_bytes()); // no user mask
        extra.extend_from_slice(&0u32.to_be_bytes()); // no blending ranges
        extra.extend_from_slice(&[0, 0, 0, 0]); // empty Pascal name and padding
        extra.extend_from_slice(b"8BIMbrit");
        extra.extend_from_slice(&7u32.to_be_bytes());
        extra.extend_from_slice(&(-25i16).to_be_bytes());
        extra.extend_from_slice(&40i16.to_be_bytes());
        extra.extend_from_slice(&128u16.to_be_bytes());
        extra.push(0); // RGB, not Lab-only
        extra.push(0); // even-size block padding

        let parsed = parse_layer_extra(&extra).unwrap();
        let adjustment = parsed.adjustment.unwrap();
        assert!(matches!(
            adjustment,
            PsdAdjustment::BrightnessContrast {
                brightness: -25,
                contrast: 40
            }
        ));
        assert!(!parsed.has_unsupported_effects);
        assert!(!parsed.unsupported_adjustment_data);

        // A non-neutral mean changes Photoshop's adjustment semantics; do not
        // silently coerce that block to Pentool's standard RGB adjustment.
        extra[28..30].copy_from_slice(&129u16.to_be_bytes());
        let parsed = parse_layer_extra(&extra).unwrap();
        assert!(parsed.adjustment.is_none());
        assert!(parsed.has_unsupported_effects);
        assert!(parsed.unsupported_adjustment_data);
    }

    #[test]
    fn empty_psd_invert_block_maps_to_native_invert_adjustment() {
        let mut extra = Vec::new();
        extra.extend_from_slice(&0u32.to_be_bytes()); // no user mask
        extra.extend_from_slice(&0u32.to_be_bytes()); // no blending ranges
        extra.extend_from_slice(&[0, 0, 0, 0]); // empty Pascal name and padding
        extra.extend_from_slice(b"8BIMnvrt");
        extra.extend_from_slice(&0u32.to_be_bytes());

        let parsed = parse_layer_extra(&extra).unwrap();
        assert!(matches!(parsed.adjustment, Some(PsdAdjustment::Invert)));
        assert!(!parsed.unsupported_adjustment_data);

        let layer = LayerRecord {
            id: 5,
            name: "Invert".into(),
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            opacity: 255,
            visible: true,
            is_clipped: false,
            blend: *b"norm",
            channels: Vec::new(),
            has_mask: false,
            mask_supported: false,
            mask_bounds: None,
            has_unsupported_effects: false,
            section_type: None,
            adjustment: parsed.adjustment,
            unsupported_adjustment_data: parsed.unsupported_adjustment_data,
        };
        let node = adjustment_node(&layer, layer.adjustment.unwrap());
        assert_eq!(node["adjustment"], "invert");
        assert_eq!(node["params"], json!({}));
        let mut document = crate::composite::migrate(
            crate::scene::migrate_to_v5(crate::scene::new_document(2, 2)).unwrap(),
        )
        .unwrap();
        document["pages"][0]["layers"][0]["nodes"] = json!([node]);
        crate::scene::validate(&document).unwrap();

        // The payload is defined as empty; do not accept arbitrary bytes that
        // could describe a future or incompatible adjustment variant.
        extra.extend_from_slice(b"\0\0");
        let payload_len_offset = 20usize;
        extra[payload_len_offset..payload_len_offset + 4].copy_from_slice(&2u32.to_be_bytes());
        let parsed = parse_layer_extra(&extra).unwrap();
        assert!(parsed.adjustment.is_none());
        assert!(parsed.unsupported_adjustment_data);
    }

    #[test]
    fn legacy_brightness_contrast_maps_to_valid_scoped_scene_adjustment() {
        let layer = LayerRecord {
            id: 4,
            name: "Tone".into(),
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            opacity: 192,
            visible: true,
            is_clipped: false,
            blend: *b"norm",
            channels: Vec::new(),
            has_mask: false,
            mask_supported: false,
            mask_bounds: None,
            has_unsupported_effects: false,
            section_type: None,
            adjustment: None,
            unsupported_adjustment_data: false,
        };
        let adjustment = PsdAdjustment::BrightnessContrast {
            brightness: -25,
            contrast: 40,
        };
        let node = adjustment_node(&layer, adjustment);
        assert_eq!(node["adjustment"], "brightness-contrast");
        assert_eq!(node["params"]["brightness"], -25);
        assert_eq!(node["params"]["contrast"], 40);
        assert_eq!(node["scope"]["kind"], "below");
        assert_eq!(node["enabled"], true);
        assert!((node["opacity"].as_f64().unwrap() - 192.0 / 255.0).abs() < 1e-12);

        let document = crate::composite::migrate(
            crate::scene::migrate_to_v5(crate::scene::new_document(2, 2)).unwrap(),
        )
        .unwrap();
        let mut document = document;
        document["pages"][0]["layers"][0]["nodes"] = json!([node]);
        crate::scene::validate(&document).unwrap();
    }

    #[test]
    fn root_clipped_layer_alpha_uses_base_alpha_at_document_coordinates() {
        let mut clipped = [255, 0, 0, 255, 0, 255, 0, 255];
        let base = ClipBase {
            left: 1,
            top: 0,
            width: 1,
            height: 1,
            alpha: vec![128],
        };

        apply_clipping_alpha(&mut clipped, 2, 1, 0, 0, &base).unwrap();

        assert_eq!(clipped, [255, 0, 0, 0, 0, 255, 0, 128]);
    }

    #[test]
    fn group_close_retains_children_attributes_and_pass_through() {
        let folder = LayerRecord {
            id: 7,
            name: "Bottle group".into(),
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            opacity: 128,
            visible: false,
            is_clipped: false,
            blend: *b"pass",
            channels: Vec::new(),
            has_mask: false,
            mask_supported: false,
            mask_bounds: None,
            has_unsupported_effects: false,
            section_type: Some(2),
            adjustment: None,
            unsupported_adjustment_data: false,
        };
        let context = GroupContext {
            marker_id: 9,
            children: vec![json!({"kind":"image","id":"psd-image-4"})],
        };
        let mut root = Vec::new();
        let mut stack = Vec::new();
        let mut report = FidelityReportBuilder::new("psd", "group.psd");

        close_group(&mut root, &mut stack, &folder, context, &mut report).unwrap();

        assert_eq!(root.len(), 1);
        assert_eq!(root[0]["kind"], "group");
        assert_eq!(root[0]["id"], "psd-group-7");
        assert_eq!(root[0]["children"][0]["id"], "psd-image-4");
        assert_eq!(root[0]["visible"], false);
        assert_eq!(root[0]["isolation"], "pass-through");
        assert!((root[0]["opacity"].as_f64().unwrap() - 128.0 / 255.0).abs() < f64::EPSILON);
        let report = report.finish();
        assert_eq!(report.counts.exact, 2);
        assert_eq!(report.counts.approximated, 1);
    }

    #[test]
    fn header_rejects_psb_and_unsupported_color_modes() {
        let mut psb = vec![0; 26];
        psb[..4].copy_from_slice(b"8BPS");
        psb[4..6].copy_from_slice(&2u16.to_be_bytes());
        assert!(import(&psb, "psb.psb").is_err());

        let mut indexed = vec![0; 26];
        indexed[..4].copy_from_slice(b"8BPS");
        indexed[4..6].copy_from_slice(&1u16.to_be_bytes());
        indexed[12..14].copy_from_slice(&1u16.to_be_bytes());
        indexed[14..18].copy_from_slice(&1u32.to_be_bytes());
        indexed[18..22].copy_from_slice(&1u32.to_be_bytes());
        indexed[22..24].copy_from_slice(&8u16.to_be_bytes());
        indexed[24..26].copy_from_slice(&2u16.to_be_bytes());
        assert!(import(&indexed, "indexed.psd").is_err());
    }
}

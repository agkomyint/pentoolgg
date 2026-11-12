//! v0.13.0 task 4: bounded region previews and structural comparison.
//!
//! Both operations are read-only. Pixels come from the authoritative renderer;
//! every metric is integer arithmetic reported as an exact ratio so results are
//! identical on every platform (see `docs/agent-painting-protocol-v1.md`).
use anyhow::{bail, ensure, Context, Result};
use image::{GrayImage, RgbaImage};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const ALGORITHM: u32 = 1;
pub const MAX_PREVIEW_PIXELS: u64 = 4_194_304;
pub const MAX_CROP_PIXELS: u64 = 67_108_864;
pub const MAX_ARTIFACT_BYTES: u64 = 64 << 20;
pub const DEFAULT_MAX_EDGE: u32 = 1024;

/// A cropped RGBA8 region and where it came from.
pub struct Crop {
    pub image: RgbaImage,
    pub origin: [u32; 2],
    pub scope: String,
    pub hash: String,
}

pub fn parse_rect(text: &str) -> Result<[u32; 4]> {
    let parts: Vec<u32> = text
        .split(',')
        .map(|p| p.trim().parse::<u32>())
        .collect::<Result<_, _>>()
        .context("[invalid-request] rect must be x,y,width,height in whole pixels")?;
    <[u32; 4]>::try_from(parts).map_err(|_| {
        anyhow::anyhow!("[invalid-request] rect must be x,y,width,height in whole pixels")
    })
}

pub fn parse_background(text: &str) -> Result<[u8; 3]> {
    let c = crate::paint_palette::parse_color(text)?;
    Ok([c[0], c[1], c[2]])
}

pub(crate) fn hash_image(image: &RgbaImage) -> String {
    let mut h = Sha256::new();
    h.update(format!("pentool-crop-v1\n{}x{}\n", image.width(), image.height()).as_bytes());
    h.update(image.as_raw());
    format!("sha256:{}", hex::encode(h.finalize()))
}

use crate::paint_palette::render_scope;

/// Render a scope and crop it to `rect` (default: the opaque bounds of a node,
/// or the whole page).
pub fn crop(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    scope: &str,
    rect: Option<[u32; 4]>,
    selection: Option<&Value>,
) -> Result<Crop> {
    let mut full = render_scope(raw, document, page, scope)?;
    let mut rect = rect;
    if let Some(query) = selection {
        ensure!(
            rect.is_none(),
            "[invalid-request] give either --rect or --selection, not both"
        );
        let cov = crate::selection::coverage(raw, document, page, query, &full)?;
        rect = Some(
            crate::selection::bounds(&cov, full.width(), full.height())
                .context("[invalid-request] the selection is empty")?,
        );
        // Pixels outside the selection do not participate.
        for (i, p) in full.pixels_mut().enumerate() {
            if cov[i] == 0 {
                *p = image::Rgba([0, 0, 0, 0]);
            }
        }
    }
    let [x, y, w, h] = rect.unwrap_or([0, 0, full.width(), full.height()]);
    ensure!(
        w > 0
            && h > 0
            && u64::from(x) + u64::from(w) <= u64::from(full.width())
            && u64::from(y) + u64::from(h) <= u64::from(full.height()),
        "[invalid-request] rect {x},{y},{w},{h} must be non-empty and inside the {}x{} scope",
        full.width(),
        full.height()
    );
    ensure!(
        u64::from(w) * u64::from(h) <= MAX_CROP_PIXELS,
        "[limit-exceeded] a crop of {w}x{h} pixels exceeds {MAX_CROP_PIXELS}"
    );
    let image = image::imageops::crop_imm(&full, x, y, w, h).to_image();
    let hash = hash_image(&image);
    Ok(Crop {
        image,
        origin: [x, y],
        scope: scope.to_string(),
        hash,
    })
}

/// Integer box-filter downscale so that the long edge is at most `max_edge`.
pub fn downscale(image: &RgbaImage, max_edge: u32) -> RgbaImage {
    let (w, h) = (image.width(), image.height());
    let long = w.max(h);
    if long <= max_edge {
        return image.clone();
    }
    let nw = ((u64::from(w) * u64::from(max_edge)) / u64::from(long)).max(1) as u32;
    let nh = ((u64::from(h) * u64::from(max_edge)) / u64::from(long)).max(1) as u32;
    let mut out = RgbaImage::new(nw, nh);
    for oy in 0..nh {
        let y0 = (u64::from(oy) * u64::from(h) / u64::from(nh)) as u32;
        let y1 = (((u64::from(oy) + 1) * u64::from(h) / u64::from(nh)) as u32).max(y0 + 1);
        for ox in 0..nw {
            let x0 = (u64::from(ox) * u64::from(w) / u64::from(nw)) as u32;
            let x1 = (((u64::from(ox) + 1) * u64::from(w) / u64::from(nw)) as u32).max(x0 + 1);
            // Premultiplied average so transparent pixels do not darken edges.
            let (mut sum, mut a, mut n) = ([0u64; 3], 0u64, 0u64);
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let p = image.get_pixel(x, y).0;
                    n += 1;
                    a += u64::from(p[3]);
                    for c in 0..3 {
                        sum[c] += u64::from(p[c]) * u64::from(p[3]);
                    }
                }
            }
            let px = if n == 0 || a == 0 {
                [0, 0, 0, 0]
            } else {
                [
                    ((sum[0] + a / 2) / a) as u8,
                    ((sum[1] + a / 2) / a) as u8,
                    ((sum[2] + a / 2) / a) as u8,
                    ((a + n / 2) / n) as u8,
                ]
            };
            out.put_pixel(ox, oy, image::Rgba(px));
        }
    }
    out
}

fn contained_output(path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path);
    ensure!(
        !p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir)),
        "[invalid-request] output path {path:?} must not contain '..'"
    );
    ensure!(
        p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")),
        "[invalid-request] output path {path:?} must end in .png"
    );
    Ok(p)
}

fn write_png(path: &Path, image: &image::DynamicImage) -> Result<(u64, String)> {
    let mut bytes = Vec::new();
    image.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )?;
    ensure!(
        bytes.len() as u64 <= MAX_ARTIFACT_BYTES,
        "[limit-exceeded] artifact of {} bytes exceeds {MAX_ARTIFACT_BYTES}",
        bytes.len()
    );
    std::fs::write(path, &bytes)
        .with_context(|| format!("[io-error] cannot write {}", path.display()))?;
    Ok((
        bytes.len() as u64,
        format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
    ))
}

pub struct PreviewOptions {
    pub scope: String,
    pub rect: Option<[u32; 4]>,
    pub selection: Option<Value>,
    pub max_edge: u32,
    pub max_pixels: u64,
    pub out: Option<String>,
    pub background: Option<[u8; 3]>,
}

pub fn preview(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    options: PreviewOptions,
) -> Result<Value> {
    preview_image(raw, document, page, options).map(|(report, _)| report)
}

/// `preview` plus the bounded preview image itself (for Studio's crop view).
pub fn preview_image(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    options: PreviewOptions,
) -> Result<(Value, RgbaImage)> {
    ensure!(
        (1..=8192).contains(&options.max_edge),
        "[invalid-request] --max-edge must be between 1 and 8192"
    );
    ensure!(
        (1..=MAX_PREVIEW_PIXELS).contains(&options.max_pixels),
        "[limit-exceeded] --max-pixels must be between 1 and {MAX_PREVIEW_PIXELS}"
    );
    let source = crop(
        raw,
        document,
        page,
        &options.scope,
        options.rect,
        options.selection.as_ref(),
    )?;
    let mut shown = downscale(&source.image, options.max_edge);
    // The pixel budget may be tighter than the edge budget: shrink further.
    let mut edge = options.max_edge;
    while u64::from(shown.width()) * u64::from(shown.height()) > options.max_pixels {
        edge = (edge * 3 / 4).max(1);
        shown = downscale(&source.image, edge);
        if edge == 1 {
            break;
        }
    }
    if let Some(bg) = options.background {
        for p in shown.pixels_mut() {
            *p = image::Rgba(over(p.0, bg));
        }
    }
    let artifact = match &options.out {
        Some(path) => {
            let path = contained_output(path)?;
            let (bytes, hash) = write_png(&path, &image::DynamicImage::ImageRgba8(shown.clone()))?;
            Some(json!({"path": path, "bytes": bytes, "sha256": hash}))
        }
        None => None,
    };
    let report = json!({
        "algorithm": ALGORITHM,
        "status": "ok",
        "scope": source.scope,
        "origin": source.origin,
        "source": [source.image.width(), source.image.height()],
        "preview": [shown.width(), shown.height()],
        "downscaled": shown.width() != source.image.width() || shown.height() != source.image.height(),
        "source_hash": source.hash,
        "artifact": artifact,
    });
    Ok((report, shown))
}

fn over(p: [u8; 4], bg: [u8; 3]) -> [u8; 4] {
    let a = u32::from(p[3]);
    let mix = |c: u8, b: u8| ((u32::from(c) * a + u32::from(b) * (255 - a) + 127) / 255) as u8;
    [mix(p[0], bg[0]), mix(p[1], bg[1]), mix(p[2], bg[2]), 255]
}

fn luma(p: [u8; 4]) -> i64 {
    (54 * i64::from(p[0]) + 183 * i64::from(p[1]) + 19 * i64::from(p[2]) + 128) >> 8
}

pub(crate) fn ratio(numerator: u64, denominator: u64) -> Value {
    if denominator == 0 {
        return json!({"numerator": numerator, "denominator": 0, "value": Value::Null});
    }
    // Six decimals, ties away from zero, in exact integer arithmetic.
    let scaled = (numerator as u128 * 2_000_000 + denominator as u128) / (2 * denominator as u128);
    json!({
        "numerator": numerator,
        "denominator": denominator,
        "value": scaled as f64 / 1_000_000.0,
    })
}

fn edge_pixels(l: &[i64], w: usize, h: usize) -> Vec<bool> {
    let at = |x: i64, y: i64| {
        l[(y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize]
    };
    let mut out = vec![false; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let g = (at(x + 1, y) - at(x - 1, y)).abs() + (at(x, y + 1) - at(x, y - 1)).abs();
            out[y as usize * w + x as usize] = g >= 96;
        }
    }
    out
}

pub(crate) fn texture_bins(l: &[i64], w: usize, h: usize) -> [u64; 6] {
    let at = |x: i64, y: i64| {
        l[(y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize]
    };
    let mut bins = [0u64; 6];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let lap =
                (4 * at(x, y) - at(x - 1, y) - at(x + 1, y) - at(x, y - 1) - at(x, y + 1)).abs();
            let b = match lap {
                0..=7 => 0,
                8..=15 => 1,
                16..=31 => 2,
                32..=63 => 3,
                64..=127 => 4,
                _ => 5,
            };
            bins[b] += 1;
        }
    }
    bins
}

pub struct Measurements {
    pub value: Value,
    pub heat: GrayImage,
}

/// Compare two equally sized crops over the same background.
pub fn measure(a: &RgbaImage, b: &RgbaImage, background: [u8; 3]) -> Result<Measurements> {
    ensure!(
        a.dimensions() == b.dimensions(),
        "[invalid-request] crops differ in size: {}x{} versus {}x{}; no implicit resizing is performed",
        a.width(),
        a.height(),
        b.width(),
        b.height()
    );
    let (w, h) = (a.width() as usize, a.height() as usize);
    let n = (w * h) as u64;
    if n == 0 {
        return Ok(Measurements {
            value: json!({"state": "empty"}),
            heat: GrayImage::new(1, 1),
        });
    }
    let pa: Vec<[u8; 4]> = a.pixels().map(|p| over(p.0, background)).collect();
    let pb: Vec<[u8; 4]> = b.pixels().map(|p| over(p.0, background)).collect();
    let la: Vec<i64> = pa.iter().map(|p| luma(*p)).collect();
    let lb: Vec<i64> = pb.iter().map(|p| luma(*p)).collect();

    let (mut color, mut value) = (0u64, 0u64);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0usize, 0usize);
    let mut heat = GrayImage::new(w as u32, h as u32);
    for i in 0..w * h {
        for c in 0..3 {
            color += (i64::from(pa[i][c]) - i64::from(pb[i][c])).unsigned_abs();
        }
        if pa[i] != pb[i] {
            let (x, y) = (i % w, i / w);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        let d = (la[i] - lb[i]).unsigned_abs();
        value += d;
        heat.put_pixel(
            (i % w) as u32,
            (i / w) as u32,
            image::Luma([d.min(255) as u8]),
        );
    }

    let ea = edge_pixels(&la, w, h);
    let eb = edge_pixels(&lb, w, h);
    let edge_diff = ea.iter().zip(&eb).filter(|(x, y)| x != y).count() as u64;

    let (mut inter, mut union) = (0u64, 0u64);
    for (x, y) in a.pixels().zip(b.pixels()) {
        let (sa, sb) = (x.0[3] >= 128, y.0[3] >= 128);
        inter += u64::from(sa && sb);
        union += u64::from(sa || sb);
    }

    let (mut structure, mut blocks) = (0u64, 0u64);
    for by in (0..h).step_by(8) {
        for bx in (0..w).step_by(8) {
            let (x1, y1) = ((bx + 8).min(w), (by + 8).min(h));
            let count = ((x1 - bx) * (y1 - by)) as i64;
            let (mut sa, mut sb, mut xa, mut xb) = (0i64, 0i64, 0i64, 0i64);
            for y in by..y1 {
                for x in bx..x1 {
                    let i = y * w + x;
                    sa += la[i];
                    sb += lb[i];
                    xa += i64::from(ea[i]);
                    xb += i64::from(eb[i]);
                }
            }
            let mean = |s: i64| (s + count / 2) / count;
            let cover = |s: i64| (s * 255 + count / 2) / count;
            structure +=
                (mean(sa) - mean(sb)).unsigned_abs() + (cover(xa) - cover(xb)).unsigned_abs();
            blocks += 1;
        }
    }

    let (ta, tb) = (texture_bins(&la, w, h), texture_bins(&lb, w, h));
    let texture: u64 = ta.iter().zip(&tb).map(|(x, y)| x.abs_diff(*y)).sum();

    let identical =
        color == 0 && edge_diff == 0 && union == inter && structure == 0 && texture == 0;
    let silhouette = if union == 0 {
        json!({"numerator": 0, "denominator": 0, "value": Value::Null, "state": "empty"})
    } else {
        ratio(inter, union)
    };
    Ok(Measurements {
        value: json!({
            "state": if identical { "identical" } else { "different" },
            "pixels": n,
            "color_mae": ratio(color, 3 * 255 * n),
            "value_mae": ratio(value, 255 * n),
            "edge_disagreement": ratio(edge_diff, n),
            "silhouette_iou": silhouette,
            "structure_mae": ratio(structure, 2 * 255 * blocks),
            "texture_frequency_mae": ratio(texture, 2 * n),
            "changed_bounds": if min_x == usize::MAX {
                Value::Null
            } else {
                json!([min_x, min_y, max_x - min_x + 1, max_y - min_y + 1])
            },
            "interpretation": "guidance about these formulas only; not a perceptual or artistic quality score",
        }),
        heat,
    })
}

pub struct CompareOptions {
    pub scope: String,
    pub rect: Option<[u32; 4]>,
    pub selection: Option<Value>,
    pub reference_image: Option<String>,
    pub reference_document: Option<PathBuf>,
    pub reference_scope: Option<String>,
    pub background: [u8; 3],
    pub heatmap: Option<String>,
}

fn load_reference(path: &str) -> Result<RgbaImage> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("[missing-resource] cannot read reference {path}"))?;
    ensure!(
        meta.len() <= MAX_ARTIFACT_BYTES,
        "[limit-exceeded] reference {path} is larger than {MAX_ARTIFACT_BYTES} bytes"
    );
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let (w, h) = reader
        .into_dimensions()
        .context("[malformed-resource] unreadable reference image")?;
    ensure!(
        u64::from(w) * u64::from(h) <= MAX_CROP_PIXELS,
        "[limit-exceeded] reference of {w}x{h} pixels exceeds {MAX_CROP_PIXELS}"
    );
    Ok(image::open(path)
        .context("[malformed-resource] unreadable reference image")?
        .to_rgba8())
}

pub fn compare(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    options: CompareOptions,
) -> Result<Value> {
    compare_with_heat(raw, document, page, options).map(|(report, _)| report)
}

/// `compare` plus the grayscale value-difference heat map (for Studio's overlay).
pub fn compare_with_heat(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    options: CompareOptions,
) -> Result<(Value, GrayImage)> {
    let current = crop(
        raw,
        document,
        page,
        &options.scope,
        options.rect,
        options.selection.as_ref(),
    )?;
    let (reference, ref_label, ref_hash) =
        match (&options.reference_image, &options.reference_document) {
            (Some(path), None) => {
                let image = load_reference(path)?;
                let h = hash_image(&image);
                (image, json!({"image": path}), h)
            }
            (None, Some(other)) => {
                let bytes = std::fs::read(other).with_context(|| {
                    format!("[missing-resource] cannot read {}", other.display())
                })?;
                let doc: Value = serde_json::from_slice(&bytes)
                    .context("[malformed-resource] reference document is not a .pen file")?;
                let scope = options
                    .reference_scope
                    .clone()
                    .unwrap_or_else(|| options.scope.clone());
                let c = crop(
                    &doc,
                    other,
                    page,
                    &scope,
                    options.rect,
                    options.selection.as_ref(),
                )?;
                (c.image, json!({"document": other, "scope": scope}), c.hash)
            }
            _ => bail!(
                "[invalid-request] give exactly one of --reference-image or --reference-document"
            ),
        };
    let m = measure(&current.image, &reference, options.background)?;
    let artifact = match &options.heatmap {
        Some(path) => {
            ensure!(
                u64::from(m.heat.width()) * u64::from(m.heat.height()) <= MAX_PREVIEW_PIXELS,
                "[limit-exceeded] heat map exceeds {MAX_PREVIEW_PIXELS} pixels; narrow --rect"
            );
            let path = contained_output(path)?;
            let (bytes, hash) = write_png(&path, &image::DynamicImage::ImageLuma8(m.heat.clone()))?;
            Some(json!({"path": path, "bytes": bytes, "sha256": hash}))
        }
        None => None,
    };
    let report = json!({
        "algorithm": ALGORITHM,
        "scope": current.scope,
        "origin": current.origin,
        "size": [current.image.width(), current.image.height()],
        "background": format!("#{:02x}{:02x}{:02x}", options.background[0], options.background[1], options.background[2]),
        "current_hash": current.hash,
        "reference": ref_label,
        "reference_hash": ref_hash,
        "metrics": m.value,
        "heatmap": artifact,
    });
    Ok((report, m.heat))
}

#[derive(clap::Subcommand, Debug)]
pub enum ReviewAction {
    /// Export a bounded preview of a page, node, layer or group region.
    Preview {
        #[arg(long, default_value = "page")]
        scope: String,
        /// Crop as x,y,width,height in scope pixels.
        #[arg(long)]
        rect: Option<String>,
        /// Selection query JSON; the crop is its bounds, outside pixels cleared.
        #[arg(long)]
        selection: Option<String>,
        #[arg(long, default_value_t = DEFAULT_MAX_EDGE)]
        max_edge: u32,
        #[arg(long, default_value_t = MAX_PREVIEW_PIXELS)]
        max_pixels: u64,
        /// Composite over this background (default: keep alpha).
        #[arg(long)]
        background: Option<String>,
        /// Write the preview PNG here; omitted means measure only.
        #[arg(long)]
        out: Option<String>,
    },
    /// Compare a region with a reference image or another document.
    Compare {
        #[arg(long, default_value = "page")]
        scope: String,
        #[arg(long)]
        rect: Option<String>,
        /// Selection query JSON; the crop is its bounds, outside pixels cleared.
        #[arg(long)]
        selection: Option<String>,
        #[arg(long)]
        reference_image: Option<String>,
        #[arg(long)]
        reference_document: Option<PathBuf>,
        #[arg(long)]
        reference_scope: Option<String>,
        /// Explicit background both sides are composited over.
        #[arg(long, default_value = "#ffffff")]
        background: String,
        /// Write a grayscale value-difference heat map PNG.
        #[arg(long)]
        heatmap: Option<String>,
    },
}

pub fn run(input: &PathBuf, page: Option<&str>, action: ReviewAction) -> Result<()> {
    let raw: Value = serde_json::from_slice(
        &std::fs::read(input)
            .with_context(|| format!("[missing-resource] cannot read {}", input.display()))?,
    )
    .context("[malformed-resource] invalid .pen document")?;
    let query = |q: &Option<String>| -> Result<Option<Value>> {
        q.as_deref()
            .map(|t| serde_json::from_str(t).context("[invalid-request] --selection must be JSON"))
            .transpose()
    };
    let rect = |r: &Option<String>| r.as_deref().map(parse_rect).transpose();
    let result = match action {
        ReviewAction::Preview {
            scope,
            rect: r,
            selection,
            max_edge,
            max_pixels,
            background,
            out,
        } => preview(
            &raw,
            input,
            page,
            PreviewOptions {
                scope,
                rect: rect(&r)?,
                selection: query(&selection)?,
                max_edge,
                max_pixels,
                out,
                background: background.as_deref().map(parse_background).transpose()?,
            },
        )?,
        ReviewAction::Compare {
            scope,
            rect: r,
            selection,
            reference_image,
            reference_document,
            reference_scope,
            background,
            heatmap,
        } => compare(
            &raw,
            input,
            page,
            CompareOptions {
                scope,
                rect: rect(&r)?,
                selection: query(&selection)?,
                reference_image,
                reference_document,
                reference_scope,
                background: parse_background(&background)?,
                heatmap,
            },
        )?,
    };
    println!("{result}");
    Ok(())
}

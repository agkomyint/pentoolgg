//! Textured stamp tips. A tip is one 256x256 coverage plane stored as an ordinary
//! raster tile, so it is content-addressed, deduplicated, digest-checked on load and
//! carried by packages exactly like layer pixels. Coverage is the tile's alpha
//! channel. `brush_tips` names tips; a stroke journal records the digest itself.
use super::*;

pub const MAX_TIPS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipSource {
    /// Coverage = (255 - luma) * alpha: black paints, white and transparent do not.
    Darkness,
    /// Coverage = alpha.
    Alpha,
}

impl TipSource {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "darkness" => Ok(Self::Darkness),
            "alpha" => Ok(Self::Alpha),
            other => bail!(
                "[invalid-input] tip source {other:?} is not supported; use darkness or alpha"
            ),
        }
    }
}

fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || is_digest(name)
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        bail!("[invalid-input] tip name {name:?} must be 1-64 characters of A-Z, a-z, 0-9, '.', '_' or '-'")
    }
    Ok(())
}

/// Decode a stored tip to its coverage plane.
pub fn load_tip(raw: &Value, digest: &str) -> Result<TipMask> {
    let entry = raw
        .get("raster_tiles")
        .and_then(|store| store.get(digest))
        .with_context(|| format!("[missing-resource] brush tip {digest} is not in raster_tiles"))?;
    let tile = decode_tile(entry, digest)?;
    Ok(tile.chunks_exact(4).map(|pixel| pixel[3]).collect())
}

/// Replace a `brush_tips` name in `brush.tip` with its digest and load the tip.
pub fn resolve_tip(raw: &Value, brush: &mut Brush) -> Result<Option<TipMask>> {
    let Some(tip) = brush.tip.clone() else {
        return Ok(None);
    };
    let digest = if is_digest(&tip) {
        tip
    } else {
        let tips = raw.get("brush_tips").and_then(Value::as_object);
        let found = tips.and_then(|t| t.get(&tip)).and_then(Value::as_str);
        let Some(found) = found else {
            let known: Vec<&str> = tips
                .map(|t| t.keys().map(String::as_str).collect())
                .unwrap_or_default();
            bail!(
                "[not-found] brush tip {tip:?} was not found; known tips: [{}]. Add one with `pentool raster DOC tip-add NAME --image tip.png`",
                known.join(", ")
            )
        };
        found.to_owned()
    };
    let mask = load_tip(raw, &digest)?;
    brush.tip = Some(digest);
    Ok(Some(mask))
}

/// Convert an image to a tip plane: coverage per [`TipSource`], scaled with the
/// deterministic premultiplied bilinear resampler so the longer side is 256 and
/// centered on the square.
pub fn tip_from_image(image: &image::RgbaImage, source: TipSource) -> Result<Tile> {
    let mut coverage = image::RgbaImage::new(image.width(), image.height());
    for (x, y, pixel) in image.enumerate_pixels() {
        let [r, g, b, a] = pixel.0;
        let value = match source {
            TipSource::Alpha => a,
            TipSource::Darkness => {
                let luma = (54 * u32::from(r) + 183 * u32::from(g) + 19 * u32::from(b) + 128) >> 8;
                (((255 - luma) * u32::from(a) + 127) / 255) as u8
            }
        };
        coverage.put_pixel(x, y, image::Rgba([0, 0, 0, value]));
    }
    let side = TILE as u32;
    let (w, h) = (coverage.width(), coverage.height());
    let (fit_w, fit_h) = if w >= h {
        (
            side,
            ((u64::from(h) * u64::from(side) + u64::from(w) / 2) / u64::from(w)).max(1) as u32,
        )
    } else {
        (
            ((u64::from(w) * u64::from(side) + u64::from(h) / 2) / u64::from(h)).max(1) as u32,
            side,
        )
    };
    let scaled = if (fit_w, fit_h) == (w, h) {
        coverage
    } else {
        layer::resample(&coverage, fit_w, fit_h, Resample::Bilinear)
    };
    let (ox, oy) = ((side - fit_w) / 2, (side - fit_h) / 2);
    let mut tile = vec![0u8; TILE_BYTES];
    for (x, y, pixel) in scaled.enumerate_pixels() {
        let at = (((y + oy) as usize) * TILE + (x + ox) as usize) * 4;
        // Coverage only: color channels stay zero so equal tips share one digest.
        tile[at + 3] = pixel[3];
    }
    if is_blank(&tile) {
        bail!("[invalid-input] the tip image has no coverage; use --source alpha for a white-on-transparent tip")
    }
    Ok(tile.into_boxed_slice())
}

/// Register a named tip from encoded image bytes (PNG, JPEG or WebP).
pub fn tip_add(raw: &mut Value, name: &str, bytes: &[u8], source: TipSource) -> Result<Value> {
    check_name(name)?;
    let tips = raw.get("brush_tips").and_then(Value::as_object);
    if tips.is_some_and(|t| t.contains_key(name)) {
        bail!("[conflict] brush tip {name:?} already exists; remove it with `tip-remove` first")
    }
    if tips.map_or(0, Map::len) >= MAX_TIPS {
        bail!("[limit-exceeded] a document may name at most {MAX_TIPS} brush tips")
    }
    let (_, image) = crate::image::decode_source_pixels(bytes)?;
    let tile = tip_from_image(&image, source)?;
    let digest = tile_hash(&tile);
    let mut next = raw.clone();
    let object = next.as_object_mut().context("document must be an object")?;
    let store = object
        .entry("raster_tiles")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("raster_tiles must be an object")?;
    if !store.contains_key(&digest) {
        store.insert(
            digest.clone(),
            json!({"encoding":"png-base64","data":encode_tile(&tile)?}),
        );
    }
    object
        .entry("brush_tips")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("brush_tips must be an object")?
        .insert(name.to_owned(), json!(digest));
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    let covered = tile.chunks_exact(4).filter(|p| p[3] != 0).count();
    Ok(json!({
        "name": name,
        "tip": digest,
        "source_size": [image.width(), image.height()],
        "covered_pixels": covered,
    }))
}

/// Unname a tip. Its tile stays only while a journal entry still records it.
pub fn tip_remove(raw: &mut Value, name: &str) -> Result<Value> {
    let mut next = raw.clone();
    let Some(digest) = next
        .get_mut("brush_tips")
        .and_then(Value::as_object_mut)
        .and_then(|t| t.remove(name))
    else {
        bail!("[not-found] brush tip {name:?} was not found; list tips with `raster DOC tips`")
    };
    let released = collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({"name": name, "tip": digest, "tiles_released": released}))
}

pub fn tip_list(raw: &Value) -> Value {
    let tips: Vec<Value> = raw
        .get("brush_tips")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, digest)| json!({"name": name, "tip": digest}))
        .collect();
    json!({"tips": tips, "returned": tips.len()})
}

/// Document-level `brush_tips` check used by [`validate_document`].
pub(super) fn validate_tips(raw: &Value) -> Result<()> {
    let Some(tips) = raw.get("brush_tips") else {
        return Ok(());
    };
    let tips = tips
        .as_object()
        .context("[malformed-raster] brush_tips must be an object of name -> sha256 digest")?;
    if tips.len() > MAX_TIPS {
        bail!("[limit-exceeded] brush_tips holds more than {MAX_TIPS} tips")
    }
    let store = raw.get("raster_tiles");
    for (name, digest) in tips {
        check_name(name)?;
        let digest = digest.as_str().filter(|d| is_digest(d)).with_context(|| {
            format!("[malformed-raster] brush tip {name} must be a sha256: digest")
        })?;
        if store.and_then(|s| s.get(digest)).is_none() {
            bail!("[missing-resource] brush tip {name} references {digest}, which is not in raster_tiles")
        }
    }
    Ok(())
}

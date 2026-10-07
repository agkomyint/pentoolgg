//! Clone stamp (engine 1).
//!
//! A clone stroke copies pixels from a source point to the dabs of an ordinary
//! brush stroke: destination pixel `p` reads the source at
//! `S + R(-angle) * (p - A) / scale`, where `A` is the anchor (the first sample of
//! the stroke, or the persisted anchor of an aligned session) and `S` the source
//! point. Sampling is bilinear in premultiplied color with 8-bit fixed-point
//! weights; source pixels outside the source layer are transparent. The sampled
//! pixel is composited over the destination with the stroke coverage, so dab
//! shape, hardness, flow, opacity, tips and dynamics behave as for painting.
//!
//! A source on the target layer reads the layer as it was before the stroke. A
//! source on another layer is pinned: the journal entry records the digests of the
//! source tiles the stroke can read, so later edits to that layer never
//! reinterpret an old clone. Replay and recovery read only those digests.
use super::math::sin_cos_degrees;
use super::*;

/// A clone stroke may read at most this many source tiles.
pub const MAX_PINNED_TILES: usize = 256;
pub const MIN_SCALE: f64 = 0.1;
pub const MAX_SCALE: f64 = 10.0;
const STATE_KEY: &str = "raster_clone";

/// Clone parameters as requested, before the source tiles are pinned.
#[derive(Clone, Debug)]
pub struct Request {
    /// Source raster layer; `None` samples the target layer before the stroke.
    pub source: Option<String>,
    pub sx: f64,
    pub sy: f64,
    pub ax: f64,
    pub ay: f64,
    pub angle: f64,
    pub scale: f64,
}

/// A resolved clone: the request plus the pinned source tiles.
pub struct Spec {
    request: Request,
    /// Pinned source layer, or `None` to read the target layer.
    source: Option<Pinned>,
}

struct Pinned {
    surface: Surface,
    tiles: Map<String, Value>,
}

fn finite(value: f64, name: &str) -> Result<()> {
    if value.is_finite() && value.abs() <= 1.0e7 {
        Ok(())
    } else {
        bail!("[invalid-clone] {name} must be a finite number within 10 million")
    }
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        for (value, name) in [
            (self.sx, "source x"),
            (self.sy, "source y"),
            (self.ax, "anchor x"),
            (self.ay, "anchor y"),
        ] {
            finite(value, name)?;
        }
        if !(-360.0..=360.0).contains(&self.angle) {
            bail!("[invalid-clone] angle must be within -360 and 360 degrees")
        }
        if !(MIN_SCALE..=MAX_SCALE).contains(&self.scale) {
            bail!("[invalid-clone] scale must be within {MIN_SCALE} and {MAX_SCALE}")
        }
        Ok(())
    }

    /// Source-layer position (in pixel-center coordinates) read for the destination
    /// point `x, y`.
    fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let (sin, cos) = sin_cos_degrees(self.angle);
        let (dx, dy) = (x - self.ax, y - self.ay);
        (
            self.sx + (cos * dx + sin * dy) / self.scale,
            self.sy + (-sin * dx + cos * dy) / self.scale,
        )
    }
}

impl Spec {
    pub fn request(&self) -> &Request {
        &self.request
    }

    pub fn to_json(&self) -> Value {
        let r = &self.request;
        let source = match &self.source {
            None => json!("current"),
            Some(pinned) => json!({
                "layer": r.source,
                "width": pinned.surface.width,
                "height": pinned.surface.height,
                "tiles": pinned.tiles,
            }),
        };
        json!({
            "source": source,
            "sx": r.sx, "sy": r.sy, "ax": r.ax, "ay": r.ay,
            "angle": r.angle, "scale": r.scale,
        })
    }

    /// Rebuild a spec from a journal entry, loading only the pinned tiles.
    pub fn from_json(raw: &Value, value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .context("[malformed-raster] clone must be an object")?;
        let number = |key: &str| -> Result<f64> {
            object
                .get(key)
                .and_then(Value::as_f64)
                .with_context(|| format!("[malformed-raster] clone.{key} must be a number"))
        };
        let mut request = Request {
            source: None,
            sx: number("sx")?,
            sy: number("sy")?,
            ax: number("ax")?,
            ay: number("ay")?,
            angle: number("angle")?,
            scale: number("scale")?,
        };
        request.validate()?;
        let source = match object.get("source") {
            Some(Value::String(text)) if text == "current" => None,
            Some(Value::Object(source)) => {
                let layer = source
                    .get("layer")
                    .and_then(Value::as_str)
                    .context("[malformed-raster] clone.source.layer must be a string")?;
                let dimension = |key: &str| -> Result<u32> {
                    source
                        .get(key)
                        .and_then(Value::as_u64)
                        .filter(|n| (1..=MAX_DIMENSION).contains(n))
                        .map(|n| n as u32)
                        .with_context(|| {
                            format!(
                                "[malformed-raster] clone.source.{key} must be 1-{MAX_DIMENSION}"
                            )
                        })
                };
                let tiles = source
                    .get("tiles")
                    .and_then(Value::as_object)
                    .context("[malformed-raster] clone.source.tiles must be an object")?;
                if tiles.len() > MAX_PINNED_TILES {
                    bail!("[limit-exceeded] clone pins more than {MAX_PINNED_TILES} source tiles")
                }
                request.source = Some(layer.to_owned());
                Some(Pinned {
                    surface: Surface::load_map(
                        raw,
                        dimension("width")?,
                        dimension("height")?,
                        &Value::Object(tiles.clone()),
                    )?,
                    tiles: tiles.clone(),
                })
            }
            _ => bail!("[malformed-raster] clone.source must be \"current\" or an object"),
        };
        Ok(Self { request, source })
    }

    /// Resolve a request against the document: validate it and pin the source tiles
    /// the stroke can read.
    pub(super) fn build(
        raw: &Value,
        target: &str,
        mut request: Request,
        dabs: &[Dab],
    ) -> Result<Self> {
        request.validate()?;
        if request.source.as_deref() == Some(target) {
            request.source = None;
        }
        let Some(layer) = request.source.clone() else {
            return Ok(Self {
                request,
                source: None,
            });
        };
        let node = all_rasters(raw)
            .into_iter()
            .find(|node| node["id"] == layer.as_str())
            .ok_or_else(|| {
                bad(
                    "not-found",
                    format!("clone source layer {layer} was not found; list nodes with `pentool tree DOCUMENT --kind raster`"),
                )
            })?;
        let engine = node_engine(node)?;
        if engine > ENGINE {
            bail!("[unsupported-capability] clone source {layer} was written by raster engine {engine}; this build implements {ENGINE}")
        }
        let (width, height) = dimensions(node)?;
        // Every destination pixel a dab can touch, mapped into the source.
        let mut low = (f64::MAX, f64::MAX);
        let mut high = (f64::MIN, f64::MIN);
        for dab in dabs {
            let reach = dab.size / dab.roundness.max(MIN_ROUNDNESS) + 2.0;
            for (cx, cy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let (u, v) = request.map(dab.x + cx * reach, dab.y + cy * reach);
                low = (low.0.min(u), low.1.min(v));
                high = (high.0.max(u), high.1.max(v));
            }
        }
        let tile = TILE as f64;
        let range = |lo: f64, hi: f64| {
            ((lo - 1.0).floor() / tile).floor()..=((hi + 1.0).floor() / tile).floor()
        };
        let (xs, ys) = (range(low.0, high.0), range(low.1, high.1));
        let mut tiles = Map::new();
        for (key, digest) in node["tiles"].as_object().into_iter().flatten() {
            let (tx, ty) = parse_key(key)?;
            if xs.contains(&f64::from(tx)) && ys.contains(&f64::from(ty)) {
                tiles.insert(key.clone(), digest.clone());
            }
        }
        if tiles.len() > MAX_PINNED_TILES {
            bail!("[limit-exceeded] clone would read {} source tiles; the limit is {MAX_PINNED_TILES}. Split the stroke into shorter strokes", tiles.len())
        }
        let surface = Surface::load_map(raw, width, height, &Value::Object(tiles.clone()))?;
        Ok(Self {
            request,
            source: Some(Pinned { surface, tiles }),
        })
    }

    /// Straight-alpha RGBA read for destination pixel `x, y`; `current` is the
    /// target layer before the stroke.
    pub(super) fn pixel(&self, current: &Surface, x: u32, y: u32) -> [u8; 4] {
        let source = self.source.as_ref().map_or(current, |p| &p.surface);
        let (u, v) = self.request.map(f64::from(x) + 0.5, f64::from(y) + 0.5);
        let (u, v) = (u - 0.5, v - 0.5);
        let (fx, fy) = (u.floor(), v.floor());
        let wx = (((u - fx) * 256.0).round() as i64).clamp(0, 256);
        let wy = (((v - fy) * 256.0).round() as i64).clamp(0, 256);
        let (x0, y0) = (fx as i64, fy as i64);
        let mut alpha = 0i64;
        let mut color = [0i64; 3];
        for (px, py, weight) in [
            (x0, y0, (256 - wx) * (256 - wy)),
            (x0 + 1, y0, wx * (256 - wy)),
            (x0, y0 + 1, (256 - wx) * wy),
            (x0 + 1, y0 + 1, wx * wy),
        ] {
            if weight == 0
                || px < 0
                || py < 0
                || px >= i64::from(source.width)
                || py >= i64::from(source.height)
            {
                continue;
            }
            let p = source.pixel(px as u32, py as u32);
            let w = weight * i64::from(p[3]);
            alpha += w;
            for (c, value) in color.iter_mut().zip(p) {
                *c += w * i64::from(value);
            }
        }
        if alpha == 0 {
            return [0; 4];
        }
        let out = |c: i64| ((c + alpha / 2) / alpha) as u8;
        [
            out(color[0]),
            out(color[1]),
            out(color[2]),
            ((alpha + 32768) >> 16) as u8,
        ]
    }
}

fn state_for<'a>(raw: &'a Value, id: &str) -> Option<&'a Value> {
    raw.get(STATE_KEY)?.get(id)
}

/// Remember the clone source for a target raster layer. Resets the aligned anchor.
pub fn set_source(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    layer: Option<&str>,
    x: f64,
    y: f64,
) -> Result<Value> {
    ensure_node_unlocked(raw, page, id)?;
    finite(x, "source x")?;
    finite(y, "source y")?;
    let layer = layer.filter(|l| *l != "current" && *l != id);
    if let Some(layer) = layer {
        if !all_rasters(raw).iter().any(|node| node["id"] == layer) {
            return Err(bad(
                "not-found",
                format!("clone source layer {layer} was not found; list nodes with `pentool tree DOCUMENT --kind raster`"),
            ));
        }
    }
    let mut next = raw.clone();
    next.as_object_mut()
        .context("document must be an object")?
        .entry(STATE_KEY)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("[malformed-raster] raster_clone must be an object")?
        .insert(
            id.to_owned(),
            json!({"layer": layer, "x": x, "y": y, "anchor": null}),
        );
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({"id": id, "source": layer.unwrap_or("current"), "x": x, "y": y}))
}

pub struct Options {
    pub aligned: bool,
    pub angle: f64,
    pub scale: f64,
}

/// Run a clone stroke from the source stored by [`set_source`].
pub fn stroke(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    brush: Brush,
    samples: Vec<Sample>,
    options: &Options,
    seed: u64,
) -> Result<Value> {
    let state = state_for(raw, id).cloned().ok_or_else(|| {
        bad(
            "invalid-clone",
            format!("raster layer {id} has no clone source; run `pentool raster DOCUMENT clone-source {id} --x X --y Y` first"),
        )
    })?;
    let first = samples
        .first()
        .context("[invalid-stroke] a clone stroke needs at least one sample")?;
    let anchor = match (options.aligned, state["anchor"].as_array()) {
        (true, Some(pair)) if pair.len() == 2 => (
            pair[0].as_f64().unwrap_or(first.x),
            pair[1].as_f64().unwrap_or(first.y),
        ),
        _ => (first.x, first.y),
    };
    let number = |key: &str| -> Result<f64> {
        state[key]
            .as_f64()
            .with_context(|| format!("[malformed-raster] raster_clone.{id}.{key} must be a number"))
    };
    let request = Request {
        source: state["layer"].as_str().map(str::to_owned),
        sx: number("x")?,
        sy: number("y")?,
        ax: anchor.0,
        ay: anchor.1,
        angle: options.angle,
        scale: options.scale,
    };
    let source = request.source.clone();
    let mut result = paint(
        raw,
        page,
        id,
        StrokeRequest {
            brush,
            samples,
            color: [0, 0, 0],
            blend: Blend::Tool(Tool::Clone),
            seed,
            clone: Some(request),
        },
    )?;
    if options.aligned {
        raw[STATE_KEY][id]["anchor"] = json!([anchor.0, anchor.1]);
    }
    result["clone"] = json!({
        "source": source.as_deref().unwrap_or("current"),
        "aligned": options.aligned,
        "anchor": [anchor.0, anchor.1],
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint_rect(s: &mut Surface, x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                let tile = s
                    .tiles
                    .entry((x / TILE as u32, y / TILE as u32))
                    .or_insert_with(|| vec![0u8; TILE_BYTES].into_boxed_slice());
                let at = ((y as usize % TILE) * TILE + x as usize % TILE) * 4;
                tile[at..at + 4].copy_from_slice(&c);
            }
        }
    }

    fn spec(sx: f64, sy: f64, angle: f64, scale: f64) -> Spec {
        Spec {
            request: Request {
                source: None,
                sx,
                sy,
                ax: 100.0,
                ay: 100.0,
                angle,
                scale,
            },
            source: None,
        }
    }

    fn base() -> Surface {
        let mut s = Surface {
            width: 300,
            height: 300,
            tiles: BTreeMap::new(),
        };
        paint_rect(&mut s, 20, 20, 40, 40, [200, 10, 10, 255]);
        paint_rect(&mut s, 40, 20, 60, 40, [10, 10, 200, 255]);
        s
    }

    #[test]
    fn integer_offsets_copy_pixels_exactly() {
        let s = base();
        // Anchor (100,100) reads source (30.5, 30.5): destination 100,100 is the
        // pixel at 30,30 of the source.
        let clone = spec(30.5, 30.5, 0.0, 1.0);
        assert_eq!(clone.pixel(&s, 100, 100), [200, 10, 10, 255]);
        assert_eq!(clone.pixel(&s, 110, 100), [10, 10, 200, 255]);
        assert_eq!(clone.pixel(&s, 100, 130), [0; 4], "empty source");
    }

    #[test]
    fn reads_outside_the_source_are_transparent_not_clamped() {
        let s = base();
        let clone = spec(0.5, 0.5, 0.0, 1.0);
        assert_eq!(clone.pixel(&s, 90, 100), [0; 4]);
        assert_eq!(clone.pixel(&s, 100, 100), [0; 4]);
    }

    #[test]
    fn rotation_turns_the_cloned_content_clockwise() {
        let s = base();
        // The red square is left of the blue one in the source. Rotated 90 degrees
        // clockwise about the anchor, red ends up above blue.
        let clone = spec(40.0, 30.0, 90.0, 1.0);
        assert_eq!(clone.pixel(&s, 100, 90), [200, 10, 10, 255]);
        assert_eq!(clone.pixel(&s, 100, 110), [10, 10, 200, 255]);
    }

    #[test]
    fn fractional_offsets_blend_in_premultiplied_color() {
        let s = base();
        let clone = spec(39.5, 30.0, 0.0, 1.0);
        let p = clone.pixel(&s, 100, 100);
        assert_eq!(p[3], 255);
        assert!(p[0] > 90 && p[0] < 120 && p[2] > 90 && p[2] < 120, "{p:?}");
    }

    #[test]
    fn requests_are_validated() {
        let mut request = spec(1.0, 1.0, 0.0, 1.0).request;
        assert!(request.validate().is_ok());
        request.scale = 0.0;
        assert!(request.validate().is_err());
        request.scale = 1.0;
        request.angle = 400.0;
        assert!(request.validate().is_err());
        request.angle = 0.0;
        request.sx = f64::NAN;
        assert!(request.validate().is_err());
    }

    #[test]
    fn json_round_trips() {
        let clone = spec(12.25, 3.5, 15.0, 1.5);
        let json = clone.to_json();
        let back = Spec::from_json(&json!({}), &json).unwrap();
        assert_eq!(back.to_json(), json);
        assert!(Spec::from_json(&json!({}), &json!({"source": 3})).is_err());
    }
}

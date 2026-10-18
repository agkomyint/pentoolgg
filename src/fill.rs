//! Deterministic solid, gradient, and content-addressed pattern fill nodes.
use anyhow::{bail, Context, Result};
use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
use std::path::Path;

fn n(value: &Value, key: &str, default: f64) -> Result<f64> {
    value.get(key).map_or(Ok(default), |v| {
        v.as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1e6)
            .context("[invalid-fill] number must be finite and bounded")
    })
}
pub(crate) fn color(value: &Value, styles: &Value) -> Result<[f64; 4]> {
    fn resolve(value: &Value, styles: &Value, depth: usize) -> Result<[f64; 4]> {
        if depth > 64 {
            bail!("[invalid-fill] color token depth exceeded")
        }
        if let Some(reference) = value.get("ref").and_then(Value::as_str) {
            if let Some(token) = styles.get(reference).and_then(|v| v.get("value")) {
                return resolve(token, styles, depth + 1);
            }
            return resolve(
                value
                    .get("fallback")
                    .context("missing color token fallback")?,
                styles,
                depth + 1,
            );
        }
        if let Some(fallback) = value.get("fallback") {
            return resolve(fallback, styles, depth + 1);
        }
        if let Some(values) = value.as_array() {
            if ![3, 4].contains(&values.len()) {
                bail!("color must contain RGB or RGBA")
            }
            let mut out = [0.0, 0.0, 0.0, 1.0];
            for (i, v) in values.iter().enumerate() {
                out[i] = v
                    .as_f64()
                    .filter(|n| (0.0..=255.0).contains(n))
                    .context("color channels must be in 0..255")?
                    / 255.0;
            }
            return Ok(out);
        }
        let s = value
            .as_str()
            .context("color must be a hex string, RGBA array, or token reference")?;
        if s == "none" {
            return Ok([0.0; 4]);
        }
        let hex = s
            .strip_prefix('#')
            .filter(|s| [3, 4, 6, 8].contains(&s.len()) && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .context("color must use #RGB, #RGBA, #RRGGBB, or #RRGGBBAA")?;
        let mut out = [0.0, 0.0, 0.0, 1.0];
        if hex.len() <= 4 {
            for (i, c) in hex.as_bytes().iter().enumerate() {
                out[i] = f64::from((*c as char).to_digit(16).unwrap() * 17) / 255.0;
            }
        } else {
            for (i, output) in out.iter_mut().take(hex.len() / 2).enumerate() {
                let start = i * 2;
                *output = f64::from(u8::from_str_radix(&hex[start..start + 2], 16)?) / 255.0;
            }
        }
        Ok(out)
    }
    resolve(value, styles, 0)
}
pub(crate) fn validate_color(value: &Value, styles: &Value, native: bool) -> Result<()> {
    fn check(value: &Value, styles: &Value, native: bool, depth: usize) -> Result<()> {
        if depth > 64 {
            bail!("[invalid-fill] color token depth exceeded")
        }
        if native && value.get("space").is_some() {
            crate::color::value::NativeColor::parse(value)?;
            return Ok(());
        }
        if let Some(reference) = value.get("ref").and_then(Value::as_str) {
            if let Some(token) = styles.get(reference).and_then(|v| v.get("value")) {
                return check(token, styles, native, depth + 1);
            }
            return check(
                value
                    .get("fallback")
                    .context("missing color token fallback")?,
                styles,
                native,
                depth + 1,
            );
        }
        if let Some(fallback) = value.get("fallback") {
            return check(fallback, styles, native, depth + 1);
        }
        color(value, styles)?;
        Ok(())
    }
    check(value, styles, native, 0)
}
fn stops(fill: &Value, styles: &Value, validation: Option<bool>) -> Result<Vec<(f64, [f64; 4])>> {
    let values = fill["stops"]
        .as_array()
        .filter(|a| (2..=64).contains(&a.len()))
        .context("[invalid-fill] gradients require 2..64 stops")?;
    let mut out = Vec::new();
    for value in values {
        if value.get("position").is_none() && value.get("offset").is_some() {
            bail!("[invalid-fill] gradient stops use \"position\", not \"offset\"");
        }
        let position = value["position"]
            .as_f64()
            .filter(|v| (0.0..=1.0).contains(v))
            .context("gradient stop \"position\" must be a number in 0..1")?;
        let color = if let Some(native) = validation {
            validate_color(&value["color"], styles, native)?;
            [0.0; 4]
        } else {
            color(&value["color"], styles)?
        };
        out.push((position, color));
    }
    if out[0].0 != 0.0 || out.last().unwrap().0 != 1.0 || out.windows(2).any(|w| w[0].0 >= w[1].0) {
        bail!("gradient stops must increase strictly from 0 to 1")
    }
    Ok(out)
}
/// Gradient geometry defaults span the node: left to right, centred, and
/// reaching the corners. Geometry is node-local pixels.
fn geometry_default(kind: &str, key: &str, width: f64, height: f64) -> f64 {
    match (kind, key) {
        ("linear", "x2") => width,
        ("radial", "radius") => width.hypot(height) / 2.0,
        (_, "cx") => width / 2.0,
        (_, "cy") => height / 2.0,
        _ => 0.0,
    }
}

pub(crate) fn resolved<'a>(raw: &'a Value, mut fill: &'a Value) -> Result<&'a Value> {
    for _ in 0..64 {
        let Some(reference) = fill.get("ref") else {
            return Ok(fill);
        };
        let name = reference
            .as_str()
            .filter(|s| !s.is_empty())
            .context("fill style ref must be nonempty")?;
        crate::composite::settings(fill, &["ref", "fallback"])?;
        let fallback = fill
            .get("fallback")
            .context("fill style requires a portable fallback")?;
        fill = if let Some(style) = raw["styles"].get(name) {
            if style["type"] != "fill" {
                bail!("fill style {name} must have type fill")
            }
            &style["value"]
        } else {
            fallback
        };
    }
    bail!("[invalid-fill] fill style reference depth exceeds 64")
}

pub fn validate(raw: &Value, node: &Value) -> Result<()> {
    let fill = resolved(raw, &node["fill"])?;
    let styles = raw.get("styles").unwrap_or(&Value::Null);
    for key in ["x", "y", "width", "height"] {
        n(node, key, 0.0)?;
    }
    if n(node, "width", 0.0)? <= 0.0 || n(node, "height", 0.0)? <= 0.0 {
        bail!("fill dimensions must be positive")
    }
    let kind = fill["kind"].as_str().context("fill kind missing")?;
    let (w, h) = (n(node, "width", 0.0)?, n(node, "height", 0.0)?);
    let keys: &[&str] = match kind {
        "solid" => &["kind", "color", "transform"],
        "linear" => &[
            "kind",
            "stops",
            "x1",
            "y1",
            "x2",
            "y2",
            "space",
            "intent",
            "spread",
            "transform",
        ],
        "radial" => &[
            "kind",
            "stops",
            "cx",
            "cy",
            "radius",
            "space",
            "intent",
            "spread",
            "transform",
        ],
        "conic" => &[
            "kind",
            "stops",
            "cx",
            "cy",
            "angle",
            "space",
            "intent",
            "spread",
            "transform",
        ],
        "pattern" => &[
            "kind",
            "asset",
            "origin_x",
            "origin_y",
            "scale_x",
            "scale_y",
            "repeat",
            "transform",
        ],
        _ => bail!("unknown fill kind {kind}"),
    };
    for key in fill.as_object().context("fill must be an object")?.keys() {
        if !keys.contains(&key.as_str()) || (key == "intent" && !fill["space"].is_object()) {
            bail!("unknown fill parameter {key}")
        }
    }
    let transform = crate::composite::transform(fill)?;
    if transform.determinant().abs() < 1e-12 {
        bail!("fill transform must be invertible")
    }
    match kind {
        "solid" => {
            validate_color(&fill["color"], styles, crate::color::has_v8(raw))?;
        }
        "pattern" => {
            let asset = fill["asset"].as_str().context("pattern asset missing")?;
            if raw["image_assets"].get(asset).is_none() {
                bail!("pattern source {asset} missing")
            }
            for key in ["scale_x", "scale_y"] {
                if n(fill, key, 1.0)? <= 0.0 {
                    bail!("pattern scales must be positive")
                }
            }
            for key in ["origin_x", "origin_y"] {
                n(fill, key, 0.0)?;
            }
            if !matches!(
                fill.get("repeat")
                    .and_then(Value::as_str)
                    .unwrap_or("repeat"),
                "repeat" | "repeat-x" | "repeat-y" | "none"
            ) {
                bail!("unknown pattern repeat mode")
            }
        }
        _ => {
            stops(fill, styles, Some(crate::color::has_v8(raw)))?;
            if crate::color::has_v8(raw)
                && fill
                    .get("space")
                    .is_some_and(|space| !space.is_string() && !space.is_object())
            {
                bail!("gradient space must be srgb, linear, or a native destination object")
            }
            if crate::color::gradient::setting(raw, fill)?.is_none()
                && !matches!(
                    fill.get("space").and_then(Value::as_str).unwrap_or("srgb"),
                    "srgb" | "linear"
                )
            {
                bail!("gradient space must be srgb or linear")
            }
            if !matches!(
                fill.get("spread").and_then(Value::as_str).unwrap_or("pad"),
                "pad" | "repeat" | "reflect"
            ) {
                bail!("unknown gradient spread")
            }
            match kind {
                "linear" => {
                    let (x1, y1, x2, y2) = (
                        n(fill, "x1", geometry_default(kind, "x1", w, h))?,
                        n(fill, "y1", geometry_default(kind, "y1", w, h))?,
                        n(fill, "x2", geometry_default(kind, "x2", w, h))?,
                        n(fill, "y2", geometry_default(kind, "y2", w, h))?,
                    );
                    if x1 == x2 && y1 == y2 {
                        bail!("gradient endpoints must differ")
                    }
                    let (dx, dy) = (x2 - x1, y2 - y1);
                    let t = |x: f64, y: f64| ((x - x1) * dx + (y - y1) * dy) / (dx * dx + dy * dy);
                    let corners = [t(0.0, 0.0), t(w, 0.0), t(0.0, h), t(w, h)];
                    let spread = fill.get("spread").and_then(Value::as_str).unwrap_or("pad");
                    if fill.get("transform").is_none_or(|t| t.is_null())
                        && spread == "pad"
                        && (corners.iter().all(|t| *t <= 0.0) || corners.iter().all(|t| *t >= 1.0))
                    {
                        bail!(
                            "[invalid-fill] linear gradient endpoints ({x1},{y1})-({x2},{y2}) leave the {w}x{h} node flat; gradient geometry is node-local pixels (0,0 is the node's top-left), not page coordinates"
                        )
                    }
                }
                "radial" => {
                    let cx = n(fill, "cx", geometry_default(kind, "cx", w, h))?;
                    let cy = n(fill, "cy", geometry_default(kind, "cy", w, h))?;
                    let radius = n(fill, "radius", geometry_default(kind, "radius", w, h))?;
                    if radius <= 0.0 {
                        bail!("radial gradient radius must be positive")
                    }
                    let nearest = (cx - cx.clamp(0.0, w)).hypot(cy - cy.clamp(0.0, h));
                    let spread = fill.get("spread").and_then(Value::as_str).unwrap_or("pad");
                    if fill.get("transform").is_none_or(|t| t.is_null())
                        && spread == "pad"
                        && nearest >= radius
                    {
                        bail!(
                            "[invalid-fill] radial gradient centre ({cx},{cy}) radius {radius} misses the {w}x{h} node; gradient geometry is node-local pixels (0,0 is the node's top-left), not page coordinates"
                        )
                    }
                }
                _ => {
                    n(fill, "cx", geometry_default(kind, "cx", w, h))?;
                    n(fill, "cy", geometry_default(kind, "cy", w, h))?;
                    n(fill, "angle", 0.0)?;
                }
            }
        }
    }
    Ok(())
}
fn atan2(y: f64, x: f64) -> f64 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let mut t = if x.abs() >= y.abs() {
        y.abs() / x.abs()
    } else {
        x.abs() / y.abs()
    };
    t /= 1.0 + (1.0 + t * t).sqrt();
    let mut term = t;
    let mut sum = 0.0;
    for i in 0..32 {
        sum += term / f64::from(2 * i + 1);
        term *= -t * t;
    }
    let mut angle = 2.0 * sum;
    if y.abs() > x.abs() {
        angle = std::f64::consts::FRAC_PI_2 - angle;
    }
    if x < 0.0 {
        angle = std::f64::consts::PI - angle;
    }
    if y < 0.0 {
        -angle
    } else {
        angle
    }
}
pub(crate) fn spread_position(t: f64, spread: &str) -> f64 {
    match spread {
        "repeat" => t.rem_euclid(1.0),
        "reflect" => {
            let v = t.rem_euclid(2.0);
            if v > 1.0 {
                2.0 - v
            } else {
                v
            }
        }
        _ => t.clamp(0.0, 1.0),
    }
}
fn gradient(stops: &[(f64, [f64; 4])], t: f64, spread: &str, space: &str) -> [f64; 4] {
    let t = spread_position(t, spread);
    for pair in stops.windows(2) {
        if t <= pair[1].0 {
            let f = (t - pair[0].0) / (pair[1].0 - pair[0].0);
            let a = pair[0].1[3] * (1.0 - f) + pair[1].1[3] * f;
            let mut out = [0.0; 4];
            out[3] = a;
            if a == 0.0 {
                return out;
            }
            for (i, c) in out[..3].iter_mut().enumerate() {
                let (mut l, mut r) = (pair[0].1[i], pair[1].1[i]);
                if space == "linear" {
                    l = crate::blend::to_linear(l);
                    r = crate::blend::to_linear(r);
                }
                *c = (l * pair[0].1[3] * (1.0 - f) + r * pair[1].1[3] * f) / a;
                if space == "linear" {
                    *c = crate::blend::to_srgb(*c);
                }
            }
            return out;
        }
    }
    stops.last().unwrap().1
}
/// Settings for uncomposited fill coverage in explicit working RGB. Node opacity,
/// masks, effects and sibling blending belong to the shared scene compositor.
pub struct LinearFillOptions {
    pub dimensions: (u32, u32),
    pub scale: f32,
    pub working: crate::color::value::Destination,
}

pub struct LinearFillResult {
    pub surface: crate::blend::LinearSurface,
    /// Includes native endpoint conversions, selected ICC paths and conversion
    /// into a legacy gradient's explicitly bounded sRGB interpolation domain.
    pub conversion_clipped: bool,
}

struct FillTarget<'a> {
    dimensions: (u32, u32),
    scale: f32,
    working: Option<&'a crate::color::value::Destination>,
}
enum FillOutput {
    Preview(RgbaImage),
    Working(LinearFillResult),
}
impl FillOutput {
    fn new(target: &FillTarget<'_>) -> Result<Self> {
        let (w, h) = target.dimensions;
        if w == 0
            || h == 0
            || w > 16384
            || h > 16384
            || !(1.0 / 16384.0..=8.0).contains(&target.scale)
        {
            bail!("[invalid-fill] dimensions must be in 1..=16384 and scale in 1/16384..=8")
        }
        if let Some(working) = target.working {
            if !working.is_linear_rgb() {
                bail!("[unsupported-capability] working fill requires named linear RGB")
            }
            Ok(Self::Working(LinearFillResult {
                surface: crate::blend::LinearSurface::new(w, h, working.space())?,
                conversion_clipped: false,
            }))
        } else {
            if u64::from(w) * u64::from(h) * 4 > crate::composite::MAX_TEMP_BYTES {
                bail!("[limit-exceeded] fill surface exceeds temporary memory budget")
            }
            Ok(Self::Preview(RgbaImage::new(w, h)))
        }
    }
    fn clipped(&mut self, clipped: bool) {
        if let Self::Working(result) = self {
            result.conversion_clipped |= clipped;
        }
    }
    fn write(&mut self, x: u32, y: u32, rgba: [f64; 4]) -> Result<()> {
        match self {
            Self::Preview(image) => {
                let pixel = std::array::from_fn(|i| {
                    (rgba[i] * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
                });
                *image.get_pixel_mut(x, y) = Rgba(pixel);
                Ok(())
            }
            Self::Working(result) => result.surface.write_pixel(
                y as usize * result.surface.dimensions().0 as usize + x as usize,
                rgba,
            ),
        }
    }
}

/// Render a fill's coverage into a bounded float surface using a caller-owned
/// verified color context. Its intent must match an explicit native gradient.
/// Source document, bindings and channel declarations are never changed.
pub fn render_linear(
    raw: &Value,
    node: &Value,
    document: &Path,
    parent: kurbo::Affine,
    options: &LinearFillOptions,
    engine: &mut crate::color::value::Evaluator<'_>,
) -> Result<LinearFillResult> {
    crate::scene::validate(raw)?;
    render_linear_prepared(raw, node, document, parent, options, engine)
}

/// Scene renderers validate once before walking their nodes and reuse the same
/// verified profile snapshot. Keep fill-local validation in the shared loop.
pub(crate) fn render_linear_prepared(
    raw: &Value,
    node: &Value,
    document: &Path,
    parent: kurbo::Affine,
    options: &LinearFillOptions,
    engine: &mut crate::color::value::Evaluator<'_>,
) -> Result<LinearFillResult> {
    if !crate::color::has_v8(raw) {
        bail!("[unsupported-capability] working fill rendering requires an explicitly migrated v8 document")
    }
    if node["kind"] != "fill" {
        bail!("[invalid-fill] working fill rendering requires a fill node")
    }
    match render_target(
        raw,
        node,
        document,
        parent,
        FillTarget {
            dimensions: options.dimensions,
            scale: options.scale,
            working: Some(&options.working),
        },
        Some(engine),
    )? {
        FillOutput::Working(result) => Ok(result),
        FillOutput::Preview(_) => unreachable!("working fill target"),
    }
}

pub(crate) fn render(
    raw: &Value,
    node: &Value,
    document: &Path,
    parent: kurbo::Affine,
    dimensions: (u32, u32),
    scale: f32,
    engine: Option<&mut crate::color::value::Evaluator<'_>>,
) -> Result<RgbaImage> {
    match render_target(
        raw,
        node,
        document,
        parent,
        FillTarget {
            dimensions,
            scale,
            working: None,
        },
        engine,
    )? {
        FillOutput::Preview(image) => Ok(image),
        FillOutput::Working(_) => unreachable!("preview fill target"),
    }
}

fn render_target(
    raw: &Value,
    node: &Value,
    document: &Path,
    parent: kurbo::Affine,
    target: FillTarget<'_>,
    mut engine: Option<&mut crate::color::value::Evaluator<'_>>,
) -> Result<FillOutput> {
    let dimensions = target.dimensions;
    let scale = target.scale;
    let (width, height) = dimensions;
    validate(raw, node)?;
    let fill = resolved(raw, &node["fill"])?;
    let styles = raw.get("styles").unwrap_or(&Value::Null);
    let kind = fill["kind"].as_str().unwrap();
    let bounds = (
        n(node, "x", 0.0)?,
        n(node, "y", 0.0)?,
        n(node, "width", 0.0)?,
        n(node, "height", 0.0)?,
    );
    let world = parent * crate::composite::transform(node)?;
    if world.as_coeffs().iter().any(|v| !v.is_finite()) || !world.determinant().is_finite() {
        bail!(
            "[invalid-fill] accumulated fill transform overflow; reduce transform scale or nesting"
        )
    }
    let mut out = FillOutput::new(&target)?;
    if world.determinant().abs() < 1e-12 {
        return Ok(out);
    }
    let inv = world.inverse();
    let fill_inv = crate::composite::transform(fill)?.inverse();
    let native = crate::color::gradient::setting(raw, fill)?;
    if let (Some(_), Some((_, intent))) = (target.working, native.as_ref()) {
        if engine
            .as_deref()
            .context("working fill color context missing")?
            .intent()
            != *intent
        {
            bail!("[invalid-fill] working color context intent differs from the native gradient intent")
        }
    }
    let sources = native
        .as_ref()
        .map(|_| crate::color::gradient::sources(fill, styles))
        .transpose()?;
    let mut native_gradient = sources
        .map(|sources| {
            crate::color::gradient::Gradient::prepare(
                sources,
                &native.as_ref().unwrap().0,
                engine
                    .as_deref_mut()
                    .context("native gradient color context missing")?,
                target.working,
            )
        })
        .transpose()?;
    if let Some(native) = &native_gradient {
        out.clipped(native.conversion_clipped());
    }
    let stops = if native.is_none() && matches!(kind, "linear" | "radial" | "conic") {
        if target.working.is_some() {
            let destination = crate::color::value::Destination::parse(&json!({"space":"srgb"}))?;
            let context = engine
                .as_deref_mut()
                .context("working fill color context missing")?;
            Some(
                crate::color::gradient::sources(fill, styles)?
                    .into_iter()
                    .map(|(position, color)| {
                        let evaluated = context.convert(&color, &destination)?;
                        out.clipped(evaluated.clipped);
                        let c = evaluated.color;
                        Ok((
                            position,
                            [c.channels()[0], c.channels()[1], c.channels()[2], c.alpha()],
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?,
            )
        } else {
            Some(stops(fill, styles, None)?)
        }
    } else {
        None
    };
    let solid = if kind == "solid" {
        if let Some(working) = target.working {
            let color = crate::color::gradient::color(&fill["color"], styles, 0)?;
            let (rgba, clipped) = engine
                .as_deref_mut()
                .context("working fill color context missing")?
                .working_rgba(&color, working)?;
            out.clipped(clipped);
            Some(rgba)
        } else {
            Some(color(&fill["color"], styles)?)
        }
    } else {
        None
    };
    let pattern = if kind == "pattern" {
        let bytes = crate::image::load_asset_bytes(
            raw,
            crate::resource::document_root(document),
            fill["asset"].as_str().unwrap(),
        )?;
        Some(crate::image::decode_source_pixels(&bytes)?.1)
    } else {
        None
    };
    let (node_w, node_h) = (bounds.2, bounds.3);
    let get = |key, default| n(fill, key, default).unwrap();
    let geo = |key: &str| n(fill, key, geometry_default(kind, key, node_w, node_h)).unwrap();
    for y in 0..height {
        crate::composite::check_cancelled()?;
        for x in 0..width {
            let mut sum = [0.0; 4];
            for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let p = inv
                    * kurbo::Point::new(
                        (f64::from(x) + sx) / f64::from(scale),
                        (f64::from(y) + sy) / f64::from(scale),
                    );
                if p.x < bounds.0
                    || p.y < bounds.1
                    || p.x >= bounds.0 + bounds.2
                    || p.y >= bounds.1 + bounds.3
                {
                    continue;
                }
                let p = fill_inv * kurbo::Point::new(p.x - bounds.0, p.y - bounds.1);
                let c = if let Some(solid) = solid {
                    solid
                } else if let Some(pattern) = &pattern {
                    let px = (p.x - get("origin_x", 0.0)) / get("scale_x", 1.0);
                    let py = (p.y - get("origin_y", 0.0)) / get("scale_y", 1.0);
                    let repeat = fill["repeat"].as_str().unwrap_or("repeat");
                    if ((!matches!(repeat, "repeat" | "repeat-x"))
                        && (px < 0.0 || px >= f64::from(pattern.width())))
                        || ((!matches!(repeat, "repeat" | "repeat-y"))
                            && (py < 0.0 || py >= f64::from(pattern.height())))
                    {
                        continue;
                    }
                    let pixel = pattern.get_pixel(
                        px.rem_euclid(f64::from(pattern.width())).floor() as u32,
                        py.rem_euclid(f64::from(pattern.height())).floor() as u32,
                    );
                    // Pattern assets are sRGB8 rasters, interpreted like images.
                    let rgba = [
                        f64::from(pixel[0]) / 255.0,
                        f64::from(pixel[1]) / 255.0,
                        f64::from(pixel[2]) / 255.0,
                        f64::from(pixel[3]) / 255.0,
                    ];
                    if let Some(working) = target.working {
                        engine
                            .as_deref_mut()
                            .context("working fill color context missing")?
                            .srgb_working_rgba(rgba, working)?
                    } else {
                        rgba
                    }
                } else {
                    let t = match kind {
                        "linear" => {
                            let (dx, dy) = (geo("x2") - geo("x1"), geo("y2") - geo("y1"));
                            ((p.x - geo("x1")) * dx + (p.y - geo("y1")) * dy) / (dx * dx + dy * dy)
                        }
                        "radial" => {
                            let (dx, dy) = (p.x - geo("cx"), p.y - geo("cy"));
                            (dx * dx + dy * dy).sqrt() / geo("radius")
                        }
                        _ => {
                            (atan2(p.y - geo("cy"), p.x - geo("cx"))
                                - get("angle", 0.0) * std::f64::consts::PI / 180.0)
                                .rem_euclid(std::f64::consts::TAU)
                                / std::f64::consts::TAU
                        }
                    };
                    if let Some(native) = &mut native_gradient {
                        if let Some(working) = target.working {
                            let (rgba, clipped) = native.sample_working(
                                engine.as_deref_mut().unwrap(),
                                working,
                                t,
                                fill["spread"].as_str().unwrap_or("pad"),
                            )?;
                            out.clipped(clipped);
                            rgba
                        } else {
                            native.sample(
                                engine.as_deref_mut().unwrap(),
                                t,
                                fill["spread"].as_str().unwrap_or("pad"),
                            )?
                        }
                    } else {
                        let rgba = gradient(
                            stops.as_ref().unwrap(),
                            t,
                            fill["spread"].as_str().unwrap_or("pad"),
                            fill["space"].as_str().unwrap_or("srgb"),
                        );
                        if let Some(working) = target.working {
                            engine
                                .as_deref_mut()
                                .context("working fill color context missing")?
                                .srgb_working_rgba(rgba, working)?
                        } else {
                            rgba
                        }
                    }
                };
                for i in 0..3 {
                    sum[i] += c[i] * c[3] * 0.25;
                }
                sum[3] += c[3] * 0.25;
            }
            if sum[3] == 0.0 {
                continue;
            }
            let mut rgba = [0.0; 4];
            for i in 0..3 {
                rgba[i] = sum[i] / sum[3];
            }
            rgba[3] = sum[3];
            out.write(x, y, rgba)?;
        }
    }
    Ok(out)
}

pub fn edit(
    raw: &mut Value,
    page: Option<&str>,
    operation: &str,
    id: &str,
    settings: &Value,
) -> Result<Value> {
    crate::composite::settings(
        settings,
        if operation == "add" {
            &["layer", "x", "y", "width", "height", "fill"]
        } else {
            &["x", "y", "width", "height", "fill"]
        },
    )?;
    if settings.as_object().unwrap().is_empty() {
        bail!("fill edit requires settings")
    }
    crate::scene::validate(raw)?;
    if !crate::composite::is_document(raw) {
        bail!("fill nodes require `migrate --target 6`")
    }
    let mut candidate = raw.clone();
    let selected = crate::scene::page_mut(&mut candidate, page)?;
    let result = if operation == "add" {
        if crate::scene::find_node_mut(selected, id).is_some() {
            bail!("fill node id already exists")
        }
        let layer = settings["layer"].as_str().unwrap_or("layer-1");
        crate::scene::ensure_layer_unlocked(selected, layer)?;
        let node = json!({"kind":"fill","id":id,"x":settings.get("x").cloned().unwrap_or(json!(0)),"y":settings.get("y").cloned().unwrap_or(json!(0)),
            "width":settings.get("width").cloned().unwrap_or(selected["canvas"]["width"].clone()),"height":settings.get("height").cloned().unwrap_or(selected["canvas"]["height"].clone()),"fill":settings["fill"]});
        crate::scene::layer_nodes_mut(selected, layer)?.push(node.clone());
        node
    } else if operation == "set" {
        crate::composite::ensure_unlocked(selected, id)?;
        let node = crate::scene::find_node_mut(selected, id).context("fill node missing")?;
        if node["kind"] != "fill" {
            bail!("node is not a fill")
        }
        for key in ["x", "y", "width", "height", "fill"] {
            if let Some(value) = settings.get(key) {
                node[key] = value.clone();
            }
        }
        node.clone()
    } else {
        bail!("fill operation must be add or set")
    };
    crate::scene::validate(&candidate)?;
    *raw = candidate;
    Ok(result)
}

#[cfg(test)]
mod linear_fill_tests {
    use super::*;
    use crate::color::{
        icc::Intent,
        value::{Destination, Evaluator, ProfileSet},
    };

    #[test]
    fn cancellation_at_each_working_fill_row_discards_private_output() {
        let mut raw: Value =
            serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
        raw["pages"][0]["canvas"] = json!({"width":1,"height":3,"background":"#00000000"});
        raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"paint","x":0,"y":0,"width":1,"height":3,
            "fill":{"kind":"solid","color":{"space":"srgb:linear","channels":[0.2,0.4,0.6],"alpha":0.123456789}}}]);
        let before = raw.clone();
        let document = Path::new("target/cancel-working-fill.pen");
        let set = ProfileSet::load(&raw, document, std::iter::empty()).unwrap();
        let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
        let options = LinearFillOptions {
            dimensions: (1, 3),
            scale: 1.0,
            working: Destination::parse(&json!({"space":"srgb:linear"})).unwrap(),
        };
        let node = &raw["pages"][0]["layers"][0]["nodes"][0];
        for checkpoint in 1..=3 {
            let (result, reached) = crate::composite::cancel_at_check(checkpoint, || {
                render_linear(
                    &raw,
                    node,
                    document,
                    kurbo::Affine::IDENTITY,
                    &options,
                    &mut engine,
                )
            });
            assert!(reached);
            assert!(result.err().unwrap().to_string().contains("[cancelled]"));
            assert_eq!(raw, before);
        }
        let complete = render_linear(
            &raw,
            node,
            document,
            kurbo::Affine::IDENTITY,
            &options,
            &mut engine,
        )
        .unwrap();
        for pixel in complete.surface.pixels() {
            for (actual, expected) in pixel.iter().zip([0.2, 0.4, 0.6, 0.123456789]) {
                assert!((actual - expected).abs() < 1e-15);
            }
        }
        assert_eq!(raw, before);
    }
}

#[cfg(test)]
mod gradient_stop_tests {
    use super::*;

    #[test]
    fn fill_stops_name_the_position_key() {
        let offset = json!({"kind":"linear","stops":[
            {"offset":0,"color":"#000000"},{"offset":1,"color":"#ffffff"}]});
        let error = stops(&offset, &json!({}), None).unwrap_err().to_string();
        assert!(error.contains("\"position\", not \"offset\""), "{error}");
        let position = json!({"kind":"linear","stops":[
            {"position":0,"color":"#000000"},{"position":1,"color":"#ffffff"}]});
        assert_eq!(stops(&position, &json!({}), None).unwrap().len(), 2);
        let color = crate::color::gradient::sources(&offset, &json!({}))
            .unwrap_err()
            .to_string();
        assert!(color.contains("\"position\", not \"offset\""), "{color}");
    }
}

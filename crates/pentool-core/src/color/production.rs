//! Page production geometry: trim, bleed, slug, safe zone, art box and marks.
//! See "Production geometry and delivery" in `docs/professional-color-v1.md`.
//!
//! A page may carry a `production` object. All lengths are document units;
//! `units_per_inch` (default 72) defines the physical size, and PDF boxes are
//! converted to points with 72 points per inch and a bottom-left origin.
//!
//! ```json
//! "production": {
//!   "units_per_inch": 72,
//!   "trim": {"x": 18, "y": 18, "width": 612, "height": 792},
//!   "bleed": 9, "slug": {"top": 36, "right": 36, "bottom": 36, "left": 36},
//!   "safe": 18,
//!   "marks": {"crop": true, "registration": true, "offset": 9, "length": 18}
//! }
//! ```
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub const MAX_UNITS_PER_INCH: f64 = 2400.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Rect {
    fn right(&self) -> f64 {
        self.x + self.width
    }
    fn bottom(&self) -> f64 {
        self.y + self.height
    }
    fn contains(&self, inner: &Rect) -> bool {
        const EPS: f64 = 1e-9;
        inner.x >= self.x - EPS
            && inner.y >= self.y - EPS
            && inner.right() <= self.right() + EPS
            && inner.bottom() <= self.bottom() + EPS
    }
    fn grow(&self, e: &Edges) -> Rect {
        Rect {
            x: self.x - e.left,
            y: self.y - e.top,
            width: self.width + e.left + e.right,
            height: self.height + e.top + e.bottom,
        }
    }
    fn json(&self) -> Value {
        json!({"x":self.x,"y":self.y,"width":self.width,"height":self.height})
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

/// Color-bar patches (C, M, Y, K ink amounts): C, M, Y, K, R, G, B, 50% K.
pub const COLOR_BAR: [[f64; 4]; 8] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
    [0.0, 1.0, 1.0, 0.0],
    [1.0, 0.0, 1.0, 0.0],
    [1.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.5],
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marks {
    pub crop: bool,
    pub registration: bool,
    /// Eight process-color control patches in the bottom slug.
    pub color_bars: bool,
    pub offset: f64,
    pub length: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub units_per_inch: f64,
    pub canvas: (f64, f64),
    pub trim: Rect,
    pub bleed: Rect,
    pub bleed_edges: Edges,
    pub media: Rect,
    pub art: Option<Rect>,
    pub safe: Rect,
    pub marks: Option<Marks>,
    /// Job-information line printed in registration color in the top slug.
    pub slug_text: Option<String>,
}

/// Slug text is set at 6 pt and limited to printable ASCII so the same line
/// prints identically with any available font.
pub const MAX_SLUG_TEXT: usize = 200;
const SLUG_TEXT_POINTS: f64 = 6.0;

fn number(value: &Value, what: &str, min: f64, max: f64) -> Result<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (min..=max).contains(v))
        .with_context(|| {
            format!("[invalid-input] production {what} must be a number in {min}..{max}")
        })
}

fn edges(value: Option<&Value>, what: &str) -> Result<Edges> {
    let Some(value) = value else {
        return Ok(Edges::default());
    };
    if value.is_number() {
        let v = number(value, what, 0.0, 65536.0)?;
        return Ok(Edges {
            top: v,
            right: v,
            bottom: v,
            left: v,
        });
    }
    let fields = value.as_object().with_context(|| {
        format!(
            "[invalid-input] production {what} must be a number or top/right/bottom/left object"
        )
    })?;
    ensure!(
        fields
            .keys()
            .all(|k| matches!(k.as_str(), "top" | "right" | "bottom" | "left")),
        "[invalid-input] production {what} only accepts top, right, bottom and left"
    );
    let side = |key: &str| -> Result<f64> {
        match fields.get(key) {
            None => Ok(0.0),
            Some(v) => number(v, &format!("{what}.{key}"), 0.0, 65536.0),
        }
    };
    Ok(Edges {
        top: side("top")?,
        right: side("right")?,
        bottom: side("bottom")?,
        left: side("left")?,
    })
}

fn rect(value: &Value, what: &str) -> Result<Rect> {
    let fields = value
        .as_object()
        .with_context(|| format!("[invalid-input] production {what} must be an object"))?;
    ensure!(
        fields
            .keys()
            .all(|k| matches!(k.as_str(), "x" | "y" | "width" | "height")),
        "[invalid-input] production {what} only accepts x, y, width and height"
    );
    let field = |key: &str, min: f64| -> Result<f64> {
        number(
            fields
                .get(key)
                .with_context(|| format!("[invalid-input] production {what}.{key} is required"))?,
            &format!("{what}.{key}"),
            min,
            1.0e7,
        )
    };
    let out = Rect {
        x: field("x", -1.0e7)?,
        y: field("y", -1.0e7)?,
        width: field("width", 0.0)?,
        height: field("height", 0.0)?,
    };
    ensure!(
        out.width > 0.0 && out.height > 0.0,
        "[invalid-input] production {what} must have positive width and height"
    );
    Ok(out)
}

/// Resolve and validate one page's production declaration. `pdfx` additionally
/// enforces that the art box and trim box are not both used for PDF/X.
pub fn resolve(page: &Value, pdfx: bool) -> Result<Option<Geometry>> {
    let Some(value) = page.get("production") else {
        return Ok(None);
    };
    let id = page["id"].as_str().unwrap_or("<page>");
    let fields = value
        .as_object()
        .with_context(|| format!("[invalid-input] page {id} production must be an object"))?;
    if let Some(key) = fields.keys().find(|k| {
        !matches!(
            k.as_str(),
            "units_per_inch" | "trim" | "bleed" | "slug" | "art" | "safe" | "marks" | "slug_text"
        )
    }) {
        bail!("[invalid-input] page {id} production has unknown field {key}");
    }
    let wrap = |e: anyhow::Error| e.context(format!("page {id} production"));
    let upi = match fields.get("units_per_inch") {
        None => 72.0,
        Some(v) => number(v, "units_per_inch", 1.0, MAX_UNITS_PER_INCH).map_err(wrap)?,
    };
    let canvas = (
        page["canvas"]["width"]
            .as_f64()
            .context("canvas width missing")?,
        page["canvas"]["height"]
            .as_f64()
            .context("canvas height missing")?,
    );
    let full = Rect {
        x: 0.0,
        y: 0.0,
        width: canvas.0,
        height: canvas.1,
    };
    let trim = match fields.get("trim") {
        None => full,
        Some(v) => rect(v, "trim").map_err(wrap)?,
    };
    ensure!(
        full.contains(&trim),
        "[invalid-input] page {id} trim box lies outside the {}x{} canvas; resize the canvas or trim",
        canvas.0,
        canvas.1
    );
    let bleed_edges = edges(fields.get("bleed"), "bleed").map_err(wrap)?;
    let slug = edges(fields.get("slug"), "slug").map_err(wrap)?;
    let bleed = trim.grow(&bleed_edges);
    let media = bleed.grow(&slug);
    ensure!(
        full.contains(&media),
        "[invalid-input] page {id} bleed and slug extend outside the canvas; the media box must fit the canvas (increase the canvas or reduce bleed/slug)"
    );
    let art = match fields.get("art") {
        None => None,
        Some(v) => Some(rect(v, "art").map_err(wrap)?),
    };
    if let Some(art) = &art {
        ensure!(
            bleed.contains(art),
            "[invalid-input] page {id} art box must lie within the bleed box"
        );
        ensure!(
            !pdfx,
            "[unsupported-capability] page {id} declares both trim and art boxes; PDF/X allows one of TrimBox or ArtBox, remove production.art"
        );
    }
    let safe_edges = edges(fields.get("safe"), "safe").map_err(wrap)?;
    let safe = Rect {
        x: trim.x + safe_edges.left,
        y: trim.y + safe_edges.top,
        width: trim.width - safe_edges.left - safe_edges.right,
        height: trim.height - safe_edges.top - safe_edges.bottom,
    };
    ensure!(
        safe.width > 0.0 && safe.height > 0.0,
        "[invalid-input] page {id} safe-zone insets leave no area inside the trim box"
    );
    let marks = match fields.get("marks") {
        None => None,
        Some(v) => {
            let m = v.as_object().with_context(|| {
                format!("[invalid-input] page {id} production marks must be an object")
            })?;
            ensure!(
                m.keys()
                    .all(|k| matches!(
                        k.as_str(),
                        "crop" | "registration" | "color_bars" | "offset" | "length"
                    )),
                "[invalid-input] page {id} marks only accept crop, registration, color_bars, offset and length"
            );
            let flag = |key: &str| {
                m.get(key).map_or(Ok(false), |v| {
                    v.as_bool()
                        .with_context(|| format!("[invalid-input] marks.{key} must be a boolean"))
                })
            };
            let marks = Marks {
                crop: flag("crop")?,
                registration: flag("registration")?,
                color_bars: flag("color_bars")?,
                offset: m
                    .get("offset")
                    .map_or(Ok(0.0), |v| number(v, "marks.offset", 0.0, 65536.0))
                    .map_err(wrap)?,
                length: m
                    .get("length")
                    .map_or(Ok(0.0), |v| number(v, "marks.length", 0.0, 65536.0))
                    .map_err(wrap)?,
            };
            if marks.crop || marks.registration || marks.color_bars {
                ensure!(
                    marks.length > 0.0,
                    "[invalid-input] page {id} marks need a positive length"
                );
                let widest = bleed_edges
                    .top
                    .max(bleed_edges.right)
                    .max(bleed_edges.bottom)
                    .max(bleed_edges.left);
                ensure!(
                    marks.offset >= widest,
                    "[invalid-input] page {id} marks must start outside the bleed: offset {} is below the largest bleed {widest}",
                    marks.offset
                );
                let reach = marks.offset + marks.length;
                let free = Edges {
                    top: trim.y - media.y,
                    right: media.right() - trim.right(),
                    bottom: media.bottom() - trim.bottom(),
                    left: trim.x - media.x,
                };
                ensure!(
                    [free.top, free.right, free.bottom, free.left].iter().all(|f| *f + 1e-9 >= reach),
                    "[invalid-input] page {id} marks need {reach} units outside the trim on every side but the media box allows less; increase slug"
                );
            }
            if marks.color_bars {
                // Eight patches of length/2 sit left of the center registration mark.
                ensure!(
                    marks.length * 4.5 <= trim.width / 2.0 + 1e-9,
                    "[invalid-input] page {id} color bar needs trim width of at least {}; shorten marks.length or widen the trim",
                    marks.length * 9.0
                );
            }
            Some(marks)
        }
    };
    let slug_text = match fields.get("slug_text") {
        None => None,
        Some(v) => {
            let text = v
                .as_str()
                .with_context(|| format!("[invalid-input] page {id} slug_text must be a string"))?;
            ensure!(
                !text.is_empty()
                    && text.len() <= MAX_SLUG_TEXT
                    && text.bytes().all(|b| (0x20..=0x7e).contains(&b)),
                "[invalid-input] page {id} slug_text must be 1..={MAX_SLUG_TEXT} printable ASCII characters"
            );
            let size = SLUG_TEXT_POINTS / 72.0 * upi;
            let reach = marks.map_or(0.0, |m| {
                if m.crop || m.registration || m.color_bars {
                    m.offset + m.length
                } else {
                    0.0
                }
            });
            let margin = marks.map_or(0.0, |m| m.length);
            ensure!(
                trim.y - media.y - reach + 1e-9 >= size * 1.4,
                "[invalid-input] page {id} slug_text needs {} units of top slug above the marks; increase slug.top",
                size * 1.4
            );
            ensure!(
                text.len() as f64 * size * 0.62 <= trim.width - 2.0 * margin + 1e-9,
                "[invalid-input] page {id} slug_text is too long for the trim width; shorten it or widen the trim"
            );
            Some(text.to_owned())
        }
    };
    Ok(Some(Geometry {
        units_per_inch: upi,
        canvas,
        trim,
        bleed,
        bleed_edges,
        media,
        art,
        safe,
        marks,
        slug_text,
    }))
}

impl Geometry {
    fn points(&self, r: &Rect, origin: &Rect) -> [f64; 4] {
        // Top-left document coordinates to bottom-left PDF points, relative to media.
        let k = 72.0 / self.units_per_inch;
        let llx = (r.x - origin.x) * k;
        let urx = (r.right() - origin.x) * k;
        let lly = (origin.bottom() - r.bottom()) * k;
        let ury = (origin.bottom() - r.y) * k;
        [llx, lly, urx, ury]
    }

    /// PDF page boxes in points with the media box at the origin.
    pub fn pdf_boxes(&self) -> Value {
        let mut out = json!({
            "media": self.points(&self.media, &self.media),
            "bleed": self.points(&self.bleed, &self.media),
            "trim": self.points(&self.trim, &self.media),
        });
        if let Some(art) = &self.art {
            out["art"] = json!(self.points(art, &self.media));
        }
        out
    }

    /// Crop and registration mark geometry in document units, outside the bleed.
    pub fn marks(&self) -> Vec<Value> {
        let Some(m) = self.marks else {
            return Vec::new();
        };
        let t = &self.trim;
        let mut out = Vec::new();
        if m.crop {
            for (cx, sx) in [(t.x, -1.0), (t.right(), 1.0)] {
                for (cy, sy) in [(t.y, -1.0), (t.bottom(), 1.0)] {
                    out.push(json!({"kind":"crop",
                        "x1":cx + sx * m.offset,"y1":cy,"x2":cx + sx * (m.offset + m.length),"y2":cy}));
                    out.push(json!({"kind":"crop",
                        "x1":cx,"y1":cy + sy * m.offset,"x2":cx,"y2":cy + sy * (m.offset + m.length)}));
                }
            }
        }
        if m.color_bars {
            for (i, cmyk) in COLOR_BAR.iter().enumerate() {
                out.push(json!({"kind":"color-bar","index":i,
                    "x":t.x + i as f64 * m.length / 2.0,"y":t.bottom() + m.offset,
                    "width":m.length / 2.0,"height":m.length,"cmyk":cmyk}));
            }
        }
        if m.registration {
            let r = m.length / 2.0;
            let d = m.offset + r;
            let (mx, my) = (t.x + t.width / 2.0, t.y + t.height / 2.0);
            for (x, y) in [
                (mx, t.y - d),
                (mx, t.bottom() + d),
                (t.x - d, my),
                (t.right() + d, my),
            ] {
                out.push(json!({"kind":"registration","cx":x,"cy":y,"r":r}));
            }
        }
        out
    }

    /// Rasterize the color-bar patches: `paint(x, y, coverage, [c, m, y, k])`
    /// with exact area coverage. Returns the patch count.
    pub fn paint_color_bars(
        &self,
        width: u32,
        height: u32,
        mut paint: impl FnMut(u32, u32, f64, [f64; 4]),
    ) -> usize {
        let patches: Vec<Value> = self
            .marks()
            .into_iter()
            .filter(|mark| mark["kind"] == "color-bar")
            .collect();
        if width == 0 || height == 0 {
            return 0;
        }
        let sx = f64::from(width) / self.canvas.0;
        let sy = f64::from(height) / self.canvas.1;
        for patch in &patches {
            let num = |k: &str| patch[k].as_f64().unwrap_or(0.0);
            let cmyk = COLOR_BAR[patch["index"].as_u64().unwrap_or(0) as usize % COLOR_BAR.len()];
            let (x0, y0) = (num("x") * sx, num("y") * sy);
            let (x1, y1) = (x0 + num("width") * sx, y0 + num("height") * sy);
            for y in (y0.floor().max(0.0) as u32)..(y1.ceil().max(0.0) as u32).min(height) {
                let oy = (f64::from(y) + 1.0).min(y1) - f64::from(y).max(y0);
                for x in (x0.floor().max(0.0) as u32)..(x1.ceil().max(0.0) as u32).min(width) {
                    let ox = (f64::from(x) + 1.0).min(x1) - f64::from(x).max(x0);
                    let c = (ox * oy).clamp(0.0, 1.0);
                    if c > 0.0 {
                        paint(x, y, c, cmyk);
                    }
                }
            }
        }
        patches.len()
    }

    /// Rasterize the crop and registration marks onto a `width` x `height`
    /// pixel grid covering the canvas. `paint(x, y, coverage)` is called with
    /// coverage in (0,1] for each touched pixel. Marks are 0.25 pt hairlines (at
    /// least one device pixel) and are meant to print in registration color,
    /// that is, full ink on every process plate. Returns the mark count.
    pub fn paint_marks(
        &self,
        width: u32,
        height: u32,
        mut paint: impl FnMut(u32, u32, f64),
    ) -> usize {
        let marks: Vec<Value> = self
            .marks()
            .into_iter()
            .filter(|mark| mark["kind"] != "color-bar")
            .collect();
        if marks.is_empty() || width == 0 || height == 0 {
            return 0;
        }
        let sx = f64::from(width) / self.canvas.0;
        let sy = f64::from(height) / self.canvas.1;
        let weight = (0.25 / 72.0 * self.units_per_inch).max(1.0 / sx.min(sy));
        let rect = |paint: &mut dyn FnMut(u32, u32, f64), x0: f64, y0: f64, x1: f64, y1: f64| {
            let (px0, px1) = (x0 * sx, x1 * sx);
            let (py0, py1) = (y0 * sy, y1 * sy);
            let lo_x = px0.floor().max(0.0) as u32;
            let hi_x = (px1.ceil().max(0.0) as u32).min(width);
            let lo_y = py0.floor().max(0.0) as u32;
            let hi_y = (py1.ceil().max(0.0) as u32).min(height);
            for y in lo_y..hi_y {
                let oy = (f64::from(y) + 1.0).min(py1) - f64::from(y).max(py0);
                for x in lo_x..hi_x {
                    let ox = (f64::from(x) + 1.0).min(px1) - f64::from(x).max(px0);
                    let c = ox.max(0.0) * oy.max(0.0);
                    if c > 0.0 {
                        paint(x, y, c.min(1.0));
                    }
                }
            }
        };
        let h = weight / 2.0;
        for mark in &marks {
            let num = |k: &str| mark[k].as_f64().unwrap_or(0.0);
            if mark["kind"] == "crop" {
                let (x1, y1, x2, y2) = (num("x1"), num("y1"), num("x2"), num("y2"));
                rect(
                    &mut paint,
                    x1.min(x2) - h,
                    y1.min(y2) - h,
                    x1.max(x2) + h,
                    y1.max(y2) + h,
                );
            } else {
                let (cx, cy, r) = (num("cx"), num("cy"), num("r"));
                rect(&mut paint, cx - r, cy - h, cx + r, cy + h);
                rect(&mut paint, cx - h, cy - r, cx + h, cy + r);
                // Ring at 0.6r: 4x4 supersampling keeps the edge deterministic.
                let ring = r * 0.6;
                let lo_x = ((cx - ring - h) * sx).floor().max(0.0) as u32;
                let hi_x = (((cx + ring + h) * sx).ceil().max(0.0) as u32).min(width);
                let lo_y = ((cy - ring - h) * sy).floor().max(0.0) as u32;
                let hi_y = (((cy + ring + h) * sy).ceil().max(0.0) as u32).min(height);
                for y in lo_y..hi_y {
                    for x in lo_x..hi_x {
                        let mut hit = 0;
                        for j in 0..4 {
                            for i in 0..4 {
                                let ux = (f64::from(x) + (f64::from(i) + 0.5) / 4.0) / sx;
                                let uy = (f64::from(y) + (f64::from(j) + 0.5) / 4.0) / sy;
                                let d = ((ux - cx).powi(2) + (uy - cy).powi(2)).sqrt();
                                if (d - ring).abs() <= h {
                                    hit += 1;
                                }
                            }
                        }
                        if hit > 0 {
                            paint(x, y, f64::from(hit) / 16.0);
                        }
                    }
                }
            }
        }
        marks.len()
    }

    /// Rasterize the slug text line (if declared) with the scene's own text
    /// pipeline and call `paint(x, y, coverage)` for every inked pixel. The
    /// text prints in registration color like the other marks. Returns 1 when a
    /// line was painted.
    pub fn paint_slug_text(
        &self,
        raw: &Value,
        page: &Value,
        document: &Path,
        scale: f32,
        mut paint: impl FnMut(u32, u32, f64),
    ) -> Result<usize> {
        let Some(text) = &self.slug_text else {
            return Ok(0);
        };
        let size = SLUG_TEXT_POINTS / 72.0 * self.units_per_inch;
        let margin = self.marks.map_or(0.0, |m| m.length);
        let node = json!({
            "kind":"text","id":"__slug_text","content":text,
            "x":self.trim.x + margin,"y":self.media.y + size * 1.05,
            "font_family":"Atkinson Hyperlegible","font_size":size,"font_weight":400,
            "style":{"fill":{"fallback":"#000000ff"}}
        });
        let fragment =
            crate::image::fragment_svg(raw, page, document, &node, kurbo::Affine::IDENTITY)?;
        let coverage = crate::render::scene_to_rgba_proxy(&fragment, scale)?;
        for (x, y, pixel) in coverage.enumerate_pixels() {
            if pixel[3] > 0 {
                paint(x, y, f64::from(pixel[3]) / 255.0);
            }
        }
        Ok(1)
    }

    pub fn json(&self) -> Value {
        json!({
            "slug_text":self.slug_text,
            "units_per_inch":self.units_per_inch,
            "canvas":{"width":self.canvas.0,"height":self.canvas.1},
            "trim":self.trim.json(),
            "bleed":self.bleed.json(),
            "media":self.media.json(),
            "art":self.art.as_ref().map(Rect::json),
            "safe":self.safe.json(),
            "pdf_boxes_pt":self.pdf_boxes(),
            "marks":self.marks(),
        })
    }
}

/// Measure painted alpha in each bleed band, not object bounds. A side is
/// `complete` when every pixel in its band is fully opaque.
pub fn bleed_coverage(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    scale: f32,
) -> Result<Value> {
    let pages = raw["pages"].as_array().context("pages missing")?;
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|p| p["id"] == id)
            .with_context(|| format!("[missing-resource] page not found: {id}"))?,
        None => pages.first().context("document has no pages")?,
    };
    let geometry = resolve(page, false)?.context(
        "[missing-resource] page has no production geometry; run `production set` first",
    )?;
    let image = crate::composite::render(raw, document, page_id, scale)?;
    let s = f64::from(scale);
    let (w, h) = image.dimensions();
    let band = |x0: f64, y0: f64, x1: f64, y1: f64| -> Value {
        let (px0, py0) = (
            (x0 * s).round().max(0.0) as u32,
            (y0 * s).round().max(0.0) as u32,
        );
        let (px1, py1) = (
            ((x1 * s).round().max(0.0) as u32).min(w),
            ((y1 * s).round().max(0.0) as u32).min(h),
        );
        let (mut total, mut opaque, mut min) = (0u64, 0u64, 255u8);
        for y in py0..py1 {
            for x in px0..px1 {
                let a = image.get_pixel(x, y).0[3];
                total += 1;
                opaque += u64::from(a == 255);
                min = min.min(a);
            }
        }
        if total == 0 {
            json!({"pixels":0,"complete":null,"painted_fraction":null,"min_alpha":null})
        } else {
            json!({"pixels":total,"complete":opaque == total,
                "painted_fraction":opaque as f64 / total as f64,"min_alpha":f64::from(min) / 255.0})
        }
    };
    let (t, b) = (&geometry.trim, &geometry.bleed);
    Ok(json!({
        "version":1,
        "scope":"bleed-coverage",
        "scale":scale,
        "top":band(b.x, b.y, b.right(), t.y),
        "bottom":band(b.x, t.bottom(), b.right(), b.bottom()),
        "left":band(b.x, t.y, t.x, t.bottom()),
        "right":band(t.right(), t.y, b.right(), t.bottom()),
    }))
}

/// Validate every page declaration (document validation hook).
pub fn validate(raw: &Value) -> Result<()> {
    for page in raw["pages"].as_array().into_iter().flatten() {
        resolve(page, false)?;
    }
    Ok(())
}

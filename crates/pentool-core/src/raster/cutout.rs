//! One-step cutout: select a subject, optionally snap it to image edges, and lift
//! it into a new transparent raster layer.
//!
//! The source is never changed. An image (or any non-raster node) is baked into a
//! temporary raster layer first, the selection is built and refined on that layer,
//! the selected pixels are copied into the new layer, and the temporary layer is
//! removed again, all inside one in-memory transaction. Coordinates are page
//! units, the same ones a rendered preview shows, so an agent can read them off
//! an image it has looked at.
use super::flood::Options as FloodOptions;
use super::layer::{ensure_new_id, find_path, integer_offset, siblings_mut};
use super::selection::{self, Mode};
use super::shape::{Marquee, Op};
use super::*;
use std::path::Path;

pub enum Source {
    /// X, Y, width, height in page units.
    Box([f64; 4]),
    Polygon(Vec<[f64; 2]>),
    /// Magic wand; `x` and `y` are page units and must lie on the layer.
    Wand(FloodOptions),
    /// The selection already stored on the (raster) source layer.
    Selection,
}

impl Source {
    fn name(&self) -> &'static str {
        match self {
            Source::Box(_) => "box",
            Source::Polygon(_) => "polygon",
            Source::Wand(_) => "wand",
            Source::Selection => "selection",
        }
    }
}

pub struct Request {
    pub source: Source,
    pub new_id: String,
    /// Snap the shape to image edges (see `select-refine`); the band in pixels.
    pub refine: Option<u32>,
    /// Certain-subject depth for the refinement; 0 derives it from a box source.
    pub core: u32,
    /// Select the complement of the chosen shape (for example the wand on a backdrop).
    pub invert: bool,
    pub fg: Vec<[f64; 2]>,
    pub bg: Vec<[f64; 2]>,
    /// Pull the edge in by this many pixels to drop a background fringe.
    pub shrink: u32,
    pub feather: u32,
    /// Recolor the soft edge from the opaque interior (removes a background fringe).
    pub decontaminate: bool,
}

pub fn cutout(
    raw: &mut Value,
    document: &Path,
    page: Option<&str>,
    id: &str,
    request: &Request,
) -> Result<Value> {
    let mut next = raw.clone();
    let path = find_path(&next, page, id)?;
    let original = siblings_mut(&mut next, &path)?[*path.chain.last().unwrap()].clone();
    let is_raster = original["kind"] == "raster";
    ensure_new_id(&next, &request.new_id)?;
    let work = if is_raster {
        id.to_owned()
    } else {
        if matches!(request.source, Source::Selection) {
            bail!("[invalid-input] {id} is not a raster layer, so it has no stored selection; use --from box, polygon or wand, or rasterize it first")
        }
        let temp = format!("{}-cutout-source", request.new_id);
        rasterize(&mut next, document, page, id, &temp, false)?;
        temp
    };
    let node = locate(&mut next.clone(), page, &work)?.clone();
    let (ox, oy) = integer_offset(&node, "raster")?;
    let (w, h) = (
        node["width"].as_u64().unwrap_or(0) as u32,
        node["height"].as_u64().unwrap_or(0) as u32,
    );

    let (ox_f, oy_f) = (ox as f64, oy as f64);
    match &request.source {
        Source::Box([x, y, bw, bh]) => {
            super::select_marquee(
                &mut next,
                page,
                &work,
                Marquee::Rect,
                [x - ox_f, y - oy_f, *bw, *bh],
                Mode::Replace,
                0,
            )?;
        }
        Source::Polygon(points) => {
            let local: Vec<[f64; 2]> = points.iter().map(|p| [p[0] - ox_f, p[1] - oy_f]).collect();
            super::select_lasso(&mut next, page, &work, &local, Mode::Replace, 0)?;
        }
        Source::Wand(options) => {
            let (lx, ly) = (i64::from(options.x) - ox, i64::from(options.y) - oy);
            if lx < 0 || ly < 0 || lx >= i64::from(w) || ly >= i64::from(h) {
                bail!(
                    "[invalid-input] the wand point ({}, {}) lies outside {id}, which covers x {ox}..{} and y {oy}..{} on the page",
                    options.x,
                    options.y,
                    ox + i64::from(w),
                    oy + i64::from(h)
                )
            }
            let local = FloodOptions {
                x: lx as u32,
                y: ly as u32,
                ..*options
            };
            super::select_wand(&mut next, page, &work, &local, Mode::Replace)?;
        }
        Source::Selection => {
            if selection::active(&next, &work)?.is_none() {
                bail!("[invalid-selection] {id} has no selection; make one first or choose --from box, polygon or wand")
            }
        }
    }
    if request.invert {
        super::select_modify(&mut next, page, &work, Op::Invert, 1, 0)?;
    }
    if let Some(band) = request.refine {
        let core = match (&request.source, request.core) {
            (Source::Box([_, _, bw, bh]), 0) => {
                ((bw.min(*bh) / 4.0) as u32).min(super::refine::MAX_BAND)
            }
            (_, core) => core,
        };
        super::select_refine(&mut next, page, &work, band, core, &request.fg, &request.bg)?;
    } else if !request.fg.is_empty() || !request.bg.is_empty() {
        bail!("[invalid-input] --fg and --bg hints need --refine")
    }
    if request.shrink > 0 {
        super::select_modify(&mut next, page, &work, Op::Contract, request.shrink, 0)?;
    }
    if request.feather > 0 {
        super::select_modify(&mut next, page, &work, Op::Feather, request.feather, 0)?;
    }
    let chosen = selection::active(&next, &work)?.context("the selection came out empty")?;
    let summary = selection::summary(&chosen);
    if summary["selected_pixels"] == 0 {
        bail!("[invalid-selection] nothing was selected; check the coordinates, or raise the wand tolerance")
    }
    let mut lifted = lift_pixels(&mut next, page, &work, &request.new_id, false)?;
    let recolored = if request.decontaminate {
        let n = super::pixels::decontaminate(&mut next, page, &request.new_id, 3)?;
        let node = locate(&mut next.clone(), page, &request.new_id)?.clone();
        lifted["tile_map_sha256"] = json!(Surface::load(&next, &node)?.tile_map_hash());
        n
    } else {
        0
    };

    let mut warnings = Vec::new();
    if let Some(b) = summary["bounds"].as_array() {
        let n = |i: usize| b[i].as_u64().unwrap_or(0) as u32;
        if n(0) == 0 || n(1) == 0 || n(2) >= w || n(3) >= h {
            warnings.push("the selection reaches the edge of the image, so the subject may be cut off or the background may be included".to_owned());
        }
    }
    if summary["fully_selected_pixels"].as_u64().unwrap_or(0) < 16 {
        warnings.push("fewer than 16 pixels are fully selected; the cutout is tiny".to_owned());
    }

    if is_raster {
        if !matches!(request.source, Source::Selection) {
            selection::clear(&mut next, &work);
        }
    } else {
        let temp_path = find_path(&next, page, &work)?;
        let at = *temp_path.chain.last().unwrap();
        siblings_mut(&mut next, &temp_path)?.remove(at);
        let source_path = find_path(&next, page, id)?;
        let at = *source_path.chain.last().unwrap();
        siblings_mut(&mut next, &source_path)?[at] = original;
    }
    collect_garbage(&mut next);
    crate::scene::validate(&next)?;
    *raw = next;
    Ok(json!({
        "id": request.new_id,
        "source": id,
        "from": request.source.name(),
        "bounds": lifted["bounds"],
        "selected_pixels": summary["selected_pixels"],
        "fully_selected_pixels": summary["fully_selected_pixels"],
        "selection_tile_map_sha256": summary["tile_map_sha256"],
        "tile_map_sha256": lifted["tile_map_sha256"],
        "refined": request.refine.is_some(),
        "decontaminated_pixels": recolored,
        "warnings": warnings,
    }))
}

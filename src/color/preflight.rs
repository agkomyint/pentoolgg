//! Structured print preflight over one page of a v8 document.
//! See "Preflight (implemented subset)" in `docs/professional-color-v1.md`.
//!
//! Every issue has a stable `code`, a `severity` (`error` blocks production,
//! `warning` needs review), the node it concerns, and a corrective action. The
//! check is read only and bounded by the document's node limits.
use super::{catalog, production, roles, roles::Role};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

pub const MAX_ISSUES: usize = 500;
const MAX_LINKED_SPOT_SCAN_NODES: usize = 100_000;

#[derive(Debug, Clone)]
pub struct Options {
    /// Minimum effective image resolution in pixels per inch.
    pub min_ppi: f64,
    /// Minimum bleed on every side, in inches.
    pub min_bleed_inches: f64,
    /// Measure painted bleed coverage at this scale (0.1..=8); skipped when absent.
    pub bleed_scale: Option<f32>,
    /// Maximum total ink coverage (sum of C+M+Y+K) in percent, 100..=400.
    pub max_total_ink: Option<f64>,
    /// Spot ink IDs a job may use; any other spot paint is an error. `None`
    /// allows every ink.
    pub allowed_spots: Option<Vec<String>>,
    /// When true, text painted in an ICC CMYK color must use only the K
    /// channel (a `text-not-k-only` error otherwise).
    pub k_only_text: bool,
    /// When true, the page must declare printer's marks (`marks-missing`).
    pub require_marks: bool,
    /// When true, spot paints that the flattened PDF/X export would convert to
    /// process color are an error instead of a warning.
    pub reject_spot_flattening: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            min_ppi: 150.0,
            min_bleed_inches: 0.125,
            bleed_scale: Some(1.0),
            max_total_ink: None,
            allowed_spots: None,
            k_only_text: false,
            require_marks: false,
            reject_spot_flattening: false,
        }
    }
}

struct Issues {
    list: Vec<Value>,
    dropped: usize,
    page_id: String,
    /// Nodes that declare overprint, for the print view overlay.
    overprint: Vec<Value>,
    /// Reference-rendered scene3d rasters and their final placed resolution.
    scene3d_ppi: Vec<Value>,
    scene3d_ppi_omitted: usize,
    linked_spot_nodes_scanned: usize,
    linked_spot_scan_limit_reported: bool,
}

struct LinkedSpotScan {
    visited_nodes: usize,
    max_nodes: usize,
    found_spot: bool,
    truncated: bool,
    visited_scene3d: BTreeSet<(String, String)>,
}

impl LinkedSpotScan {
    fn new(max_nodes: usize) -> Self {
        Self {
            max_nodes,
            ..Self::default()
        }
    }
}

impl Default for LinkedSpotScan {
    fn default() -> Self {
        Self {
            visited_nodes: 0,
            max_nodes: MAX_LINKED_SPOT_SCAN_NODES,
            found_spot: false,
            truncated: false,
            visited_scene3d: BTreeSet::new(),
        }
    }
}

fn node_has_spot_paint(node: &Value) -> bool {
    ["fill", "stroke"].iter().any(|slot| {
        node["style"][slot]["spot"]["ink"]
            .as_str()
            .is_some_and(|ink| !ink.is_empty())
    })
}

impl LinkedSpotScan {
    fn find_node<'a>(&mut self, node: &'a Value, id: &str, depth: usize) -> Option<&'a Value> {
        if self.truncated || depth > crate::scene::MAX_DEPTH {
            self.truncated = true;
            return None;
        }
        self.visited_nodes += 1;
        if self.visited_nodes > self.max_nodes {
            self.truncated = true;
            return None;
        }
        if node["id"].as_str() == Some(id) {
            return Some(node);
        }
        node["children"].as_array().and_then(|children| {
            children
                .iter()
                .find_map(|child| self.find_node(child, id, depth + 1))
        })
    }

    fn scan_node(&mut self, raw: &Value, page_id: &str, node: &Value, depth: usize) -> Result<()> {
        if self.found_spot || self.truncated {
            return Ok(());
        }
        if depth > crate::scene::MAX_DEPTH {
            self.truncated = true;
            return Ok(());
        }
        self.visited_nodes += 1;
        if self.visited_nodes > self.max_nodes {
            self.truncated = true;
            return Ok(());
        }
        if node["visible"].as_bool() == Some(false) {
            return Ok(());
        }
        if node_has_spot_paint(node) {
            self.found_spot = true;
            return Ok(());
        }
        if node["kind"].as_str() == Some("scene3d") {
            if let Some(id) = node["id"].as_str() {
                self.scan_scene3d(raw, page_id, node, id, depth + 1)?;
            }
        }
        for child in node["children"].as_array().into_iter().flatten() {
            self.scan_node(raw, page_id, child, depth + 1)?;
            if self.found_spot || self.truncated {
                break;
            }
        }
        Ok(())
    }

    fn scan_scene3d(
        &mut self,
        raw: &Value,
        page_id: &str,
        node: &Value,
        node_id: &str,
        depth: usize,
    ) -> Result<()> {
        if !self
            .visited_scene3d
            .insert((page_id.to_owned(), node_id.to_owned()))
        {
            return Ok(());
        }
        let mut sources = node["surface_graphics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|graphic| graphic.get("source"))
            .collect::<Vec<_>>();
        if let Some(dieline_id) = node["dieline"].as_str() {
            if let Some(panels) = raw["dielines"][dieline_id]["panels"].as_array() {
                sources.extend(panels.iter().filter_map(|panel| panel.get("artwork")));
            }
        }
        for source in sources {
            self.scan_source(raw, source, page_id, node_id, depth + 1)?;
            if self.found_spot || self.truncated {
                break;
            }
        }
        Ok(())
    }

    fn scan_source(
        &mut self,
        raw: &Value,
        source: &Value,
        consumer_page: &str,
        consumer_id: &str,
        depth: usize,
    ) -> Result<()> {
        let Some(source_page) = source["page"].as_str() else {
            return Ok(());
        };
        let Some(page) = raw["pages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|page| page["id"].as_str() == Some(source_page))
        else {
            return Ok(());
        };
        let layers = page["layers"].as_array().into_iter().flatten();
        match source["kind"].as_str() {
            Some("node") => {
                if let Some(source_id) = source["node"].as_str() {
                    let mut found = None;
                    for layer in layers {
                        if layer["visible"].as_bool() == Some(false) {
                            continue;
                        }
                        for node in layer["nodes"].as_array().into_iter().flatten() {
                            found = self.find_node(node, source_id, depth);
                            if found.is_some() || self.truncated {
                                break;
                            }
                        }
                        if found.is_some() || self.truncated {
                            break;
                        }
                    }
                    if let Some(node) = found {
                        self.scan_node(raw, source_page, node, depth)?;
                    }
                }
            }
            Some("page") => {
                for layer in layers {
                    if layer["visible"].as_bool() == Some(false) {
                        continue;
                    }
                    for node in layer["nodes"].as_array().into_iter().flatten() {
                        if source_page == consumer_page && node["id"].as_str() == Some(consumer_id)
                        {
                            continue;
                        }
                        self.scan_node(raw, source_page, node, depth)?;
                        if self.found_spot || self.truncated {
                            break;
                        }
                    }
                    if self.found_spot || self.truncated {
                        break;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl Issues {
    fn add(&mut self, severity: &str, code: &str, node: Option<&str>, message: String, fix: &str) {
        if self.list.len() >= MAX_ISSUES {
            self.dropped += 1;
            return;
        }
        self.list
            .push(json!({"severity":severity,"code":code,"node":node,
            "message":message,"fix":fix}));
    }
}

fn walk(
    nodes: &[Value],
    raw: &Value,
    upi: f64,
    options: &Options,
    out: &mut Issues,
    parent_transform: kurbo::Affine,
    depth: usize,
) -> Result<()> {
    if depth > 64 {
        return Ok(());
    }
    for node in nodes {
        if node["visible"].as_bool() == Some(false) {
            continue;
        }
        let transform = parent_transform * crate::composite::transform(node)?;
        let id = node["id"].as_str();
        let kind = node["kind"].as_str().unwrap_or_default();
        if node["style"]["overprint"].is_object() && out.overprint.len() < MAX_ISSUES {
            // Child coordinates are group relative, so only top-level nodes carry bounds.
            let bounds = |key: &str| node[key].as_f64().filter(|_| depth == 0);
            out.overprint
                .push(json!({"id":id,"kind":kind,"x":bounds("x"),"y":bounds("y"),
                "width":bounds("width"),"height":bounds("height")}));
        }
        if kind == "text" {
            out.add(
                "warning",
                "text-rasterized-in-pdfx",
                id,
                "text is rasterized by the flattened PDF/X subset; no font program is embedded"
                    .into(),
                "accept the rasterized text or wait for vector PDF/X output",
            );
        }
        if options.k_only_text && kind == "text" {
            let fill = &node["style"]["fill"];
            if fill["space"] == "icc-cmyk" {
                if let Some(channels) = fill["channels"].as_array() {
                    let ink = |i: usize| channels.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                    if ink(0) > 0.0 || ink(1) > 0.0 || ink(2) > 0.0 {
                        out.add(
                            "error",
                            "text-not-k-only",
                            id,
                            "text fill carries C, M or Y ink, which the print recipe's black policy forbids".into(),
                            "set the text fill to a K-only color or relax the recipe's black_policy",
                        );
                    }
                }
            }
        }
        if let Some(allowed) = &options.allowed_spots {
            for slot in ["fill", "stroke"] {
                let Some(ink) = node["style"][slot]["spot"]["ink"].as_str() else {
                    continue;
                };
                if !allowed.iter().any(|a| a == ink) {
                    out.add(
                        "error",
                        "spot-ink-not-allowed",
                        id,
                        format!(
                            "{slot} uses spot ink {ink}, which the print recipe does not allow"
                        ),
                        "use an allowed spot ink or extend the recipe's allowed_spots",
                    );
                }
            }
        }
        if kind == "image" {
            let digest = node["asset"].as_str().unwrap_or_default();
            let asset = &raw["image_assets"][digest];
            match (
                asset["pixel_width"].as_f64(),
                node["width"].as_f64().filter(|w| *w > 0.0),
            ) {
                (Some(pixels), Some(width)) => {
                    let ppi = pixels / (width / upi);
                    if ppi < options.min_ppi {
                        out.add(
                            "warning",
                            "image-low-resolution",
                            id,
                            format!(
                                "nominal resolution is {ppi:.0} ppi, below {:.0}",
                                options.min_ppi
                            ),
                            "replace the image with a higher-resolution source or scale it down",
                        );
                    }
                }
                _ => out.add(
                    "error",
                    "image-unresolved",
                    id,
                    format!("image source {digest} has no recorded pixel size"),
                    "re-import the image so its asset record exists",
                ),
            }
        }
        if kind == "scene3d" {
            let node_map = node
                .as_object()
                .context("validated scene3d node must be an object")?;
            let (width_px, height_px) = crate::scene3d::node::output_pixel_dimensions(node_map)?;
            if let Some(id) = id {
                let remaining =
                    MAX_LINKED_SPOT_SCAN_NODES.saturating_sub(out.linked_spot_nodes_scanned);
                let mut scan = LinkedSpotScan::new(remaining);
                if remaining > 0 {
                    scan.scan_scene3d(raw, &out.page_id, node, id, depth + 1)?;
                } else {
                    scan.truncated = true;
                }
                out.linked_spot_nodes_scanned = out
                    .linked_spot_nodes_scanned
                    .saturating_add(scan.visited_nodes.min(remaining));
                if scan.found_spot {
                    let strict = options.reject_spot_flattening;
                    out.add(
                        if strict { "error" } else { "warning" },
                        "scene3d-spot-flattened",
                        Some(id),
                        "linked artwork uses spot ink, but 3D mockups are flattened to process-color pixels".into(),
                        "use the flat/vector source for spot-critical printing, or allow spot conversion in the recipe",
                    );
                } else if scan.truncated && !out.linked_spot_scan_limit_reported {
                    out.linked_spot_scan_limit_reported = true;
                    out.add(
                        if options.reject_spot_flattening { "error" } else { "warning" },
                        "scene3d-spot-audit-incomplete",
                        Some(id),
                        format!("linked spot-ink audit reached its {MAX_LINKED_SPOT_SCAN_NODES}-node limit"),
                        "reduce linked-source complexity or inspect its spot plates directly",
                    );
                }
            }
            // Scene3d is composited as pixels in both the PDF/X subset and
            // separation pipeline. Spot paints are audited above for the
            // configured rejection policy, but overprint/traps and source
            // vector plates are not retained; report that known limitation.
            out.add(
                "warning",
                "scene3d-print-flattened",
                id,
                "3D mockup artwork is flattened to process-color pixels; linked spot inks, overprint, traps, and source vector plates are not retained".into(),
                "use the flat dieline/vector artwork for plate-critical printing, or accept the process-color mockup",
            );
            let placed_width = node["width"].as_f64().unwrap_or_default();
            let placed_height = node["height"].as_f64().unwrap_or_default();
            let [a, b, c, d, _, _] = transform.as_coeffs();
            let scale_x = a.hypot(b);
            let scale_y = c.hypot(d);
            if let Some((ppi_x, ppi_y)) = crate::scene3d::node::effective_ppi_for_placement(
                width_px,
                height_px,
                placed_width,
                placed_height,
                scale_x,
                scale_y,
                upi,
            ) {
                if out.scene3d_ppi.len() < MAX_ISSUES {
                    out.scene3d_ppi.push(json!({
                        "node":id,
                        "width_px":width_px,
                        "height_px":height_px,
                        "placed_width":placed_width * scale_x,
                        "placed_height":placed_height * scale_y,
                        "effective_ppi":{"x":ppi_x,"y":ppi_y,"units_per_inch":upi}
                    }));
                } else {
                    out.scene3d_ppi_omitted += 1;
                }
                if ppi_x < options.min_ppi || ppi_y < options.min_ppi {
                    out.add(
                        "warning",
                        "scene3d-low-resolution",
                        id,
                        format!(
                            "rendered mockup resolves to {:.1} × {:.1} ppi after placement, below {:.0} ppi",
                            ppi_x, ppi_y, options.min_ppi
                        ),
                        "increase the mockup pixel_ratio or reduce its final placed scale",
                    );
                }
            } else {
                out.add(
                    "error",
                    "scene3d-resolution-unavailable",
                    id,
                    "rendered mockup dimensions or final placement cannot be converted to effective ppi".into(),
                    "set positive mockup dimensions and valid page units, then rerun preflight",
                );
            }
        }
        if node["opacity"].as_f64().is_some_and(|o| o < 1.0)
            || node
                .get("blend_mode")
                .and_then(Value::as_str)
                .is_some_and(|m| m != "normal")
            || node["effects"].as_array().is_some_and(|e| !e.is_empty())
        {
            out.add(
                "warning",
                "transparency-flattened",
                id,
                "live transparency, blend modes or effects are flattened to raster in PDF/X export"
                    .into(),
                "accept the flattened result or remove the transparency",
            );
        }
        if let Some(children) = node["children"].as_array() {
            walk(children, raw, upi, options, out, transform, depth + 1)?;
        }
    }
    Ok(())
}

/// Render the page through the output role and report pixels whose C+M+Y+K sum
/// exceeds `limit` percent. The measurement raster is bounded to ~4 million
/// pixels; it is the delivered 8-bit CMYK, so it reflects the profile's own
/// conversion rather than the nominal paint values.
fn measure_total_ink(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    page: &Value,
    limit: f64,
    out: &mut Issues,
) {
    let Ok(role) = roles::effective(raw, Role::Output) else {
        return; // already reported as output-role-missing
    };
    if role.destination.space() != "icc-cmyk" {
        return;
    }
    let area = page["canvas"]["width"].as_f64().unwrap_or(1.0).max(1.0)
        * page["canvas"]["height"].as_f64().unwrap_or(1.0).max(1.0);
    let scale = (4.0e6 / area).sqrt().clamp(0.1, 1.0) as f32;
    let image = super::output::render(
        raw,
        document,
        page_id,
        scale,
        &super::output::Options {
            destination: Some(role.destination.clone()),
            intent: Some(role.intent),
            depth: super::output::Depth::Eight,
            alpha: super::output::Alpha::FlattenOnWhite,
        },
    );
    let image = match image {
        Ok(image) if image.channels == 4 => image,
        Ok(_) | Err(_) => {
            out.add(
                "warning",
                "ink-not-measured",
                None,
                "total ink could not be measured because the page did not render to CMYK".into(),
                "fix the output profile and run preflight again",
            );
            return;
        }
    };
    let mut worst = 0.0_f64;
    let mut over = 0usize;
    for pixel in image.samples.chunks_exact(4) {
        let total: f64 = pixel.iter().map(|s| f64::from(*s)).sum::<f64>() / 255.0 * 100.0;
        worst = worst.max(total);
        if total > limit + 1e-9 {
            over += 1;
        }
    }
    if over > 0 {
        out.add(
            "error",
            "ink-total-exceeded",
            None,
            format!(
                "{over} of {} sampled pixels exceed {limit}% total ink (worst {worst:.1}%)",
                image.samples.len() / 4
            ),
            "lower the separation's ink limit or use a profile with a total-area-coverage limit",
        );
    }
}

pub fn run(
    raw: &Value,
    document: &Path,
    page_id: Option<&str>,
    options: &Options,
) -> Result<Value> {
    ensure!(
        crate::color::has_v8(raw),
        "[unsupported-version] preflight requires a version {} document",
        super::VERSION
    );
    ensure!(
        options.min_ppi.is_finite() && (1.0..=2400.0).contains(&options.min_ppi),
        "[invalid-input] minimum resolution must be 1..2400 ppi"
    );
    ensure!(
        options.min_bleed_inches.is_finite() && (0.0..=4.0).contains(&options.min_bleed_inches),
        "[invalid-input] minimum bleed must be 0..4 inches"
    );
    if let Some(scale) = options.bleed_scale {
        ensure!(
            (0.1..=8.0).contains(&scale),
            "[invalid-input] bleed scale must be in 0.1..8"
        );
    }
    if let Some(limit) = options.max_total_ink {
        ensure!(
            limit.is_finite() && (100.0..=400.0).contains(&limit),
            "[invalid-input] total ink limit must be 100..400 percent"
        );
    }
    crate::scene::validate(raw)?;
    let pages = raw["pages"].as_array().context("pages missing")?;
    let page = match page_id {
        Some(id) => pages
            .iter()
            .find(|page| page["id"] == id)
            .with_context(|| format!("[missing-resource] page not found: {id}"))?,
        None => pages.first().context("document has no pages")?,
    };
    let id = page["id"].as_str().unwrap_or("page").to_owned();
    let mut out = Issues {
        list: Vec::new(),
        dropped: 0,
        page_id: id.clone(),
        overprint: Vec::new(),
        scene3d_ppi: Vec::new(),
        scene3d_ppi_omitted: 0,
        linked_spot_nodes_scanned: 0,
        linked_spot_scan_limit_reported: false,
    };

    match roles::effective(raw, Role::Output) {
        Err(_) => out.add(
            "error",
            "output-role-missing",
            None,
            "no output color role is configured".into(),
            "run `pentool color role assign` with a verified CMYK press profile",
        ),
        Ok(role) => {
            if role.destination.space() != "icc-cmyk" {
                out.add(
                    "error",
                    "output-not-cmyk",
                    None,
                    format!("output role is {}, not ICC CMYK", role.destination.space()),
                    "assign a CMYK press profile as the output role",
                );
            } else if let Some(reference) = role.destination.profile() {
                match catalog::load(raw, document, reference) {
                    Err(error) => out.add(
                        "error",
                        "output-profile-unavailable",
                        None,
                        format!("output profile {reference}: {error}"),
                        "import or embed the profile in the document catalog",
                    ),
                    Ok((_, bytes)) => {
                        if bytes.len() < 128 || &bytes[12..16] != b"prtr" {
                            out.add(
                                "error",
                                "output-profile-class",
                                None,
                                format!("profile {reference} is not an output (prtr) profile"),
                                "use a press characterization profile",
                            );
                        } else if bytes[8] > 4 {
                            out.add(
                                "error",
                                "output-profile-version",
                                None,
                                format!(
                                    "profile {reference} is ICC v{}; PDF/X-4 allows v4 or older",
                                    bytes[8]
                                ),
                                "use an ICC v2 or v4 press profile",
                            );
                        }
                    }
                }
            }
        }
    }

    let geometry = match production::resolve(page, true) {
        Err(error) => {
            out.add(
                "error",
                "production-invalid",
                None,
                error.to_string(),
                "correct the page production declaration",
            );
            None
        }
        Ok(None) => {
            out.add(
                "error",
                "production-missing",
                None,
                format!("page {id} declares no trim/bleed geometry"),
                "run `pentool color production set`",
            );
            None
        }
        Ok(Some(geometry)) => Some(geometry),
    };
    let mut upi = 72.0;
    if let Some(geometry) = &geometry {
        upi = geometry.units_per_inch;
        if options.require_marks && geometry.marks.is_none() {
            out.add(
                "error",
                "marks-missing",
                None,
                format!("page {id} declares no printer's marks but the recipe requires them"),
                "run `pentool color production set` with marks, or drop `require_marks`",
            );
        }
        let edges = [
            geometry.bleed_edges.top,
            geometry.bleed_edges.right,
            geometry.bleed_edges.bottom,
            geometry.bleed_edges.left,
        ];
        if edges
            .iter()
            .any(|b| *b < upi * options.min_bleed_inches - 1e-9)
        {
            out.add(
                "warning",
                "bleed-below-minimum",
                None,
                format!(
                    "bleed is under {} inch on at least one side",
                    options.min_bleed_inches
                ),
                "increase the bleed to the printer's requirement",
            );
        }
        if let Some(scale) = options.bleed_scale {
            if edges.iter().any(|b| *b > 0.0) {
                let coverage = production::bleed_coverage(raw, document, page_id, scale)?;
                for side in ["top", "right", "bottom", "left"] {
                    if coverage[side]["complete"] == json!(false) {
                        out.add(
                            "error",
                            "bleed-incomplete",
                            None,
                            format!(
                                "the {side} bleed band is not fully painted (min alpha {})",
                                coverage[side]["min_alpha"]
                            ),
                            "extend background or artwork through the bleed band",
                        );
                    }
                }
            }
        }
    }

    if let Some(limit) = options.max_total_ink {
        measure_total_ink(raw, document, page_id, page, limit, &mut out);
    }

    for layer in page["layers"].as_array().into_iter().flatten() {
        if layer["visible"].as_bool() == Some(false) {
            continue;
        }
        walk(
            layer["nodes"].as_array().map_or(&[][..], Vec::as_slice),
            raw,
            upi,
            options,
            &mut out,
            kurbo::Affine::IDENTITY,
            0,
        )?;
    }
    let text = page.to_string();
    for (key, code, what) in [
        ("\"spot\"", "spot-not-in-pdfx", "spot-ink paints"),
        (
            "\"overprint\"",
            "overprint-not-in-pdfx",
            "overprint declarations",
        ),
    ] {
        if text.contains(key) {
            let severity = if code == "spot-not-in-pdfx" && options.reject_spot_flattening {
                "error"
            } else {
                "warning"
            };
            out.add(
                severity,
                code,
                None,
                format!("page {id} uses {what}, which the flattened PDF/X export cannot carry"),
                "use plate separations for spot and overprint production",
            );
        }
    }

    let errors = out.list.iter().filter(|i| i["severity"] == "error").count();
    Ok(json!({
        "version": 1,
        "scope": "print-preflight",
        "page": id,
        "ok": errors == 0,
        "errors": errors,
        "warnings": out.list.len() - errors,
        "issues": out.list,
        "issues_omitted": out.dropped,
        "overprint_nodes": out.overprint,
        "scene3d_ppi": out.scene3d_ppi,
        "scene3d_ppi_omitted": out.scene3d_ppi_omitted,
        "min_ppi": options.min_ppi,
        "max_total_ink": options.max_total_ink,
        "allowed_spots": options.allowed_spots,
        "k_only_text": options.k_only_text,
        "require_marks": options.require_marks,
        "reject_spot_flattening": options.reject_spot_flattening,
        "mutated": false,
    }))
}

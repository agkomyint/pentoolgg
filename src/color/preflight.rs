//! Structured print preflight over one page of a v8 document.
//! See "Preflight (implemented subset)" in `docs/professional-color-v1.md`.
//!
//! Every issue has a stable `code`, a `severity` (`error` blocks production,
//! `warning` needs review), the node it concerns, and a corrective action. The
//! check is read only and bounded by the document's node limits.
use super::{catalog, production, roles, roles::Role};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub const MAX_ISSUES: usize = 500;

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
    /// Nodes that declare overprint, for the print view overlay.
    overprint: Vec<Value>,
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

fn walk(nodes: &[Value], raw: &Value, upi: f64, options: &Options, out: &mut Issues, depth: usize) {
    if depth > 64 {
        return;
    }
    for node in nodes {
        if node["visible"].as_bool() == Some(false) {
            continue;
        }
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
            walk(children, raw, upi, options, out, depth + 1);
        }
    }
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
        raw["version"] == super::VERSION,
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
        overprint: Vec::new(),
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
            0,
        );
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
        "min_ppi": options.min_ppi,
        "max_total_ink": options.max_total_ink,
        "allowed_spots": options.allowed_spots,
        "k_only_text": options.k_only_text,
        "require_marks": options.require_marks,
        "reject_spot_flattening": options.reject_spot_flattening,
        "mutated": false,
    }))
}

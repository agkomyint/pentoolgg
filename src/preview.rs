//! Size-bounded preview rendering shared by the server and the CLI.
use anyhow::{Context, Result};

/// Composite PNG. A sized preview that exceeds the compositing budget is
/// retried at a smaller scale instead of failing; full-size exports never are.
pub fn composite_png(
    value: &serde_json::Value,
    document: &std::path::Path,
    page: Option<&str>,
    mut scale: f32,
    preview: bool,
) -> Result<Vec<u8>> {
    let mut attempts = 0;
    loop {
        match crate::composite::png(value, document, page, scale) {
            Err(error)
                if preview
                    && attempts < PREVIEW_BUDGET_RETRIES
                    && error.to_string().starts_with("[limit-exceeded]") =>
            {
                attempts += 1;
                scale *= 0.5;
            }
            result => return result,
        }
    }
}

pub fn composite_preview_scale(
    value: &serde_json::Value,
    page: Option<&str>,
    max_edge: Option<u32>,
) -> Result<f32> {
    if max_edge.is_none() {
        return Ok(1.0);
    }
    crate::scene::validate(value)?;
    let pages = value["pages"].as_array().unwrap();
    let selected = match page {
        Some(id) => pages
            .iter()
            .find(|p| p["id"] == id)
            .context("preview page missing")?,
        None => &pages[0],
    };
    preview_scale(
        selected["canvas"]["width"].as_f64().unwrap_or(1.0),
        selected["canvas"]["height"].as_f64().unwrap_or(1.0),
        max_edge,
        1.0 / 16384.0,
    )
}

/// Largest preview a client may request, in pixels on the longest edge and in
/// total pixels. Zoomed Studio views ask for more than the canvas size so
/// vector edges stay sharp; the budget keeps one preview bounded.
pub const PREVIEW_MAX_EDGE: u32 = 8192;
/// Halvings tried when a sized composite preview exceeds the compositing budget.
pub const PREVIEW_BUDGET_RETRIES: u32 = 3;
pub const PREVIEW_MAX_PIXELS: f64 = 48.0 * 1024.0 * 1024.0;

/// Scale that makes the longest canvas edge `max_edge` pixels, within the
/// renderer's 8x limit and the preview pixel budget. `None` is full resolution.
pub fn preview_scale(width: f64, height: f64, max_edge: Option<u32>, min: f32) -> Result<f32> {
    let Some(edge) = max_edge else { return Ok(1.0) };
    if !(1..=PREVIEW_MAX_EDGE).contains(&edge) {
        anyhow::bail!("preview max_edge must be in 1..={PREVIEW_MAX_EDGE}")
    }
    let (width, height) = (width.max(1.0), height.max(1.0));
    let mut scale = (f64::from(edge) / width.max(height)).min(8.0);
    if width * height * scale * scale > PREVIEW_MAX_PIXELS {
        scale = (PREVIEW_MAX_PIXELS / (width * height)).sqrt();
    }
    Ok((scale as f32).max(min))
}

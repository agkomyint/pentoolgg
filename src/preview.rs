//! Size-bounded preview rendering shared by the server and the CLI.
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Composite PNG. A sized preview that exceeds the compositing budget is
/// retried at a smaller scale instead of failing; full-size exports never are.
pub fn composite_png(
    value: &serde_json::Value,
    document: &std::path::Path,
    page: Option<&str>,
    scale: f32,
    preview: bool,
) -> Result<Vec<u8>> {
    composite_png_scaled(value, document, page, scale, preview).map(|(bytes, _)| bytes)
}

/// [`composite_png`] that also returns the scale it finally rendered at.
fn composite_png_scaled(
    value: &serde_json::Value,
    document: &std::path::Path,
    page: Option<&str>,
    mut scale: f32,
    preview: bool,
) -> Result<(Vec<u8>, f32)> {
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
            result => return result.map(|bytes| (bytes, scale)),
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

/// Longest canvas edge, in document units, of the selected page (the first
/// page when none is selected) or of every page. Legacy documents without
/// pages use their top-level canvas.
pub fn page_extent(value: &Value, page: Option<&str>, all_pages: bool) -> Result<f64> {
    let edge = |canvas: &Value| -> Option<f64> {
        let width = canvas.get("width")?.as_f64()?;
        let height = canvas.get("height")?.as_f64()?;
        Some(width.max(height))
    };
    let pages = value
        .get("pages")
        .and_then(Value::as_array)
        .filter(|pages| !pages.is_empty());
    let extent = match (pages, page) {
        (Some(pages), _) if all_pages => pages
            .iter()
            .filter_map(|p| p.get("canvas").and_then(edge))
            .reduce(f64::max),
        (Some(pages), Some(id)) => pages
            .iter()
            .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
            .with_context(|| format!("[not-found] page not found: {id}"))?
            .get("canvas")
            .and_then(edge),
        (Some(pages), None) => pages[0].get("canvas").and_then(edge),
        (None, _) => None,
    };
    extent
        .or_else(|| value.get("canvas").and_then(edge))
        .filter(|extent| extent.is_finite() && *extent > 0.0)
        .context("[invalid-input] document has no canvas size")
}

/// Largest export scale (within the renderer's 0.1 to 8 range) at which a
/// canvas whose longest edge is `extent` renders no more than `max_size`
/// pixels long, whichever way the renderer rounds.
pub fn fit_scale(extent: f64, max_size: u32) -> Result<f32> {
    if !(1..=PREVIEW_MAX_EDGE).contains(&max_size) {
        anyhow::bail!("[invalid-input] --max-size must be in 1..={PREVIEW_MAX_EDGE}")
    }
    let mut scale = ((f64::from(max_size) / extent).min(8.0)) as f32;
    while scale > 0.0 && (extent * f64::from(scale)).ceil() > f64::from(max_size) {
        scale = f32::from_bits(scale.to_bits() - 1);
    }
    if scale < 0.1 {
        anyhow::bail!(
            "[limit-exceeded] --max-size {max_size} needs scale {scale:.4} for a {extent} unit canvas; the smallest export scale is 0.1, so use --max-size {} or more",
            (extent * 0.1).ceil()
        )
    }
    Ok(scale)
}

/// Long edge of a CLI preview when `--preview-size` is not given: sized for
/// model input, not for print.
pub const DEFAULT_PREVIEW_EDGE: u32 = 1024;
/// Previews kept in `.pentool/preview/`; older ones are removed.
pub const MAX_KEPT_PREVIEWS: usize = 32;

/// Process-wide preview request from the CLI's global flags.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub enabled: bool,
    pub out: Option<PathBuf>,
    pub max_edge: Option<u32>,
    pub page: Option<String>,
}

/// The last commit of this process, kept until its preview is rendered.
struct Pending {
    path: PathBuf,
    value: Value,
    page: Option<String>,
    revision: String,
    dry_run: bool,
}

#[derive(Default)]
struct State {
    settings: Settings,
    pending: Option<Pending>,
    output: Vec<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Set the preview request for this process. Rejects an out-of-range size
/// before any command runs, so a bad flag never follows a write.
pub fn configure(settings: Settings) -> Result<()> {
    if let Some(edge) = settings.max_edge {
        if !(1..=PREVIEW_MAX_EDGE).contains(&edge) {
            anyhow::bail!("[invalid-input] --preview-size must be in 1..={PREVIEW_MAX_EDGE}")
        }
    }
    *state() = settings.enabled.then(|| State {
        settings,
        ..State::default()
    });
    Ok(())
}

/// Print one line of command output. With previews on, output is held until
/// [`finish`] so the preview can join the command's JSON result; otherwise
/// it is printed at once, unchanged.
pub fn print(line: String) {
    match state().as_mut() {
        Some(state) => state.output.push(line),
        None => std::println!("{line}"),
    }
}

/// Note a commit (or dry run) whose page the preview will show. A command
/// that commits more than once is previewed at its final revision.
pub fn record(path: &Path, before: &[u8], after: &Value, revision: &str, dry_run: bool) {
    let mut guard = state();
    let Some(state) = guard.as_mut() else {
        return;
    };
    let page = state
        .settings
        .page
        .clone()
        .or_else(|| changed_page(before, after));
    state.pending = Some(Pending {
        path: path.to_owned(),
        value: after.clone(),
        page,
        revision: revision.into(),
        dry_run,
    });
}

/// Render the recorded commit and print the held output with a top-level
/// `"preview"` key added to its final JSON object (or on its own line when
/// the command printed no JSON object). A failed render is reported as
/// `{"error"}` and never fails a command that already committed.
pub fn finish() {
    let Some(State {
        settings,
        pending,
        mut output,
    }) = state().take()
    else {
        return;
    };
    if let Some(pending) = pending {
        let preview = write(&pending, &settings).unwrap_or_else(|error| {
            json!({
                "error": format!("{error:#}"),
                "revision": pending.revision,
                "page": pending.page,
            })
        });
        match output.last().and_then(|last| with_preview(last, &preview)) {
            Some(joined) => *output.last_mut().unwrap() = joined,
            None => output.push(json!({ "preview": preview }).to_string()),
        }
    }
    for line in output {
        std::println!("{line}");
    }
}

/// `text` with `"preview"` appended to its JSON object, keeping the
/// command's own key order and layout; `None` when it is not an object.
fn with_preview(text: &str, preview: &Value) -> Option<String> {
    let trimmed = text.trim_end();
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    let object = parsed.as_object()?;
    if object.contains_key("preview") {
        return None;
    }
    let body = trimmed.strip_suffix('}')?.trim_end();
    let separator = if object.is_empty() { "" } else { "," };
    Some(if trimmed.contains('\n') {
        let value = serde_json::to_string_pretty(preview)
            .ok()?
            .replace('\n', "\n  ");
        format!("{body}{separator}\n  \"preview\": {value}\n}}")
    } else {
        format!("{body}{separator}\"preview\":{preview}}}")
    })
}

/// Render one page of any document version at a long edge of `max_edge`
/// pixels, through the same renderer as `export`. Returns the PNG and scale.
pub fn png(
    value: &Value,
    document: &Path,
    page: Option<&str>,
    max_edge: u32,
) -> Result<(Vec<u8>, f32)> {
    let max_edge = Some(max_edge);
    if crate::composite::is_document(value) {
        let scale = composite_preview_scale(value, page, max_edge)?;
        return composite_png_scaled(value, document, page, scale, true);
    }
    let image_scene = value
        .get("version")
        .and_then(Value::as_u64)
        .is_some_and(|version| version >= crate::image::VERSION)
        && value
            .get("image_assets")
            .and_then(Value::as_object)
            .is_some_and(|assets| !assets.is_empty());
    if image_scene {
        let scene = crate::image::to_svg(value, document, page)?;
        let scale = preview_scale(
            f64::from(scene.width),
            f64::from(scene.height),
            max_edge,
            0.1,
        )?;
        return Ok((crate::render::scene_to_png(&scene, scale)?, scale));
    }
    let flattened = if crate::scene::is_scene_document(value) {
        crate::scene::flatten_to_v3(value)?
    } else {
        value.clone()
    };
    let mut doc: crate::document::Document =
        serde_json::from_value(flattened).context("invalid .pen document")?;
    if let Some(page) = page {
        doc.select_page(page).map_err(anyhow::Error::msg)?;
    }
    let scale = preview_scale(
        f64::from(doc.canvas.width),
        f64::from(doc.canvas.height),
        max_edge,
        0.1,
    )?;
    Ok((crate::render::to_png(&doc, scale)?, scale))
}

fn write(pending: &Pending, settings: &Settings) -> Result<Value> {
    let Pending {
        path,
        value,
        revision,
        dry_run,
        ..
    } = pending;
    let page = pending.page.as_deref();
    let (bytes, scale) = png(
        value,
        path,
        page,
        settings.max_edge.unwrap_or(DEFAULT_PREVIEW_EDGE),
    )?;
    let (width, height) = png_size(&bytes).context("renderer returned an invalid PNG")?;
    let target = match &settings.out {
        Some(out) => out.clone(),
        None => {
            let directory = crate::resource::document_root(path)
                .join(".pentool")
                .join("preview");
            let stem = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "document".into());
            let suffix = if *dry_run { "-dry" } else { "" };
            directory.join(format!("{stem}-{}{suffix}.png", &revision[..12]))
        }
    };
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    crate::editing::atomic_write(&target, &bytes)
        .with_context(|| format!("could not write preview {}", target.display()))?;
    if settings.out.is_none() {
        prune(&target);
    }
    let page = page
        .map(str::to_owned)
        .or_else(|| value["pages"][0]["id"].as_str().map(str::to_owned));
    Ok(json!({
        "path": target.display().to_string(),
        "page": page,
        "width": width,
        "height": height,
        "scale": scale,
        "revision": revision,
        "dry_run": dry_run,
    }))
}

/// First page whose content differs between the two documents, matched by
/// page id; `None` (the first page) when no page changed.
fn changed_page(before: &[u8], after: &Value) -> Option<String> {
    let before: Value = serde_json::from_slice(before).ok()?;
    let old = before["pages"].as_array();
    after["pages"].as_array()?.iter().find_map(|page| {
        let id = page["id"].as_str()?;
        let same = old
            .and_then(|pages| pages.iter().find(|old| old["id"] == id))
            .is_some_and(|old| old == page);
        (!same).then(|| id.to_owned())
    })
}

/// Keep the newest previews in the shared directory, never the one just written.
fn prune(written: &Path) {
    let Some(directory) = written.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut others: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let is_png = path.extension().is_some_and(|ext| ext == "png");
            if !is_png || path == written {
                return None;
            }
            Some((entry.metadata().ok()?.modified().ok()?, path))
        })
        .collect();
    if others.len() < MAX_KEPT_PREVIEWS {
        return;
    }
    others.sort();
    let excess = others.len() + 1 - MAX_KEPT_PREVIEWS;
    for (_, path) in others.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let read = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    Some((read(16), read(20)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_extent_selects_pages_and_falls_back_to_the_canvas() {
        let doc = json!({"pages": [
            {"id": "a", "canvas": {"width": 300, "height": 200}},
            {"id": "b", "canvas": {"width": 100, "height": 900}},
        ]});
        assert_eq!(page_extent(&doc, None, false).unwrap(), 300.0);
        assert_eq!(page_extent(&doc, Some("b"), false).unwrap(), 900.0);
        assert_eq!(page_extent(&doc, None, true).unwrap(), 900.0);
        assert!(page_extent(&doc, Some("c"), false).is_err());
        let legacy = json!({"canvas": {"width": 640, "height": 480}});
        assert_eq!(page_extent(&legacy, None, false).unwrap(), 640.0);
        assert!(page_extent(&json!({}), None, false).is_err());
    }

    #[test]
    fn fit_scale_never_rounds_past_the_bound() {
        for extent in [1.0, 3.0, 7.0, 320.0, 333.3, 1199.0, 1200.0, 4096.0] {
            for max in [1, 7, 100, 333, 1024, 8192] {
                if let Ok(scale) = fit_scale(extent, max) {
                    assert!((extent * f64::from(scale)).ceil() <= f64::from(max));
                    assert!((0.1..=8.0).contains(&scale));
                }
            }
        }
        assert_eq!(fit_scale(320.0, 200).unwrap(), 0.625);
        assert_eq!(fit_scale(100.0, 8192).unwrap(), 8.0);
        assert!(fit_scale(1000.0, 50).is_err());
        assert!(fit_scale(1000.0, 0).is_err());
    }

    #[test]
    fn preview_joins_json_objects_keeping_their_layout() {
        let preview = json!({"path": "p.png", "width": 2});
        assert_eq!(
            with_preview(r#"{"b":1,"a":2}"#, &preview).unwrap(),
            r#"{"b":1,"a":2,"preview":{"path":"p.png","width":2}}"#
        );
        let pretty = with_preview("{\n  \"b\": 1\n}", &preview).unwrap();
        assert_eq!(
            pretty,
            "{\n  \"b\": 1,\n  \"preview\": {\n    \"path\": \"p.png\",\n    \"width\": 2\n  }\n}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&pretty).unwrap()["preview"],
            preview
        );
        assert_eq!(
            with_preview("{}", &preview).unwrap(),
            r#"{"preview":{"path":"p.png","width":2}}"#
        );
        assert!(with_preview("[1]", &preview).is_none());
        assert!(with_preview("plain text", &preview).is_none());
        assert!(with_preview(r#"{"preview":1}"#, &preview).is_none());
    }

    #[test]
    fn changed_page_is_found_by_id() {
        let before = json!({"pages": [{"id": "a", "x": 1}, {"id": "b", "x": 1}]});
        let bytes = serde_json::to_vec(&before).unwrap();
        let after = json!({"pages": [{"id": "a", "x": 1}, {"id": "b", "x": 2}]});
        assert_eq!(changed_page(&bytes, &after).as_deref(), Some("b"));
        assert_eq!(changed_page(&bytes, &before), None);
        let added = json!({"pages": [{"id": "a", "x": 1}, {"id": "b", "x": 1}, {"id": "c"}]});
        assert_eq!(changed_page(&bytes, &added).as_deref(), Some("c"));
    }
}

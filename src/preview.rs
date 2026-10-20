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

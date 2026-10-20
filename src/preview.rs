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
    /// Objects to focus on; empty means the objects the command changed.
    pub focus: Vec<String>,
    /// Always show the whole page.
    pub no_focus: bool,
    /// Draw object IDs and bounds on the preview image.
    pub annotate: bool,
    /// Count the preview pixels that differ from the same view of an earlier
    /// revision, a file, or `before` (the state before this change).
    pub compare: Option<String>,
}

/// The last commit of this process, kept until its preview is rendered.
struct Pending {
    path: PathBuf,
    before: Vec<u8>,
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
        before: before.to_vec(),
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
    render_page(value, document, page, |width, height| {
        preview_scale(width, height, max_edge, 0.1)
    })
}

/// Render one page through the same renderer as `export`, at the scale
/// `scale` picks from the page size in document units.
fn render_page(
    value: &Value,
    document: &Path,
    page: Option<&str>,
    scale: impl FnOnce(f64, f64) -> Result<f32>,
) -> Result<(Vec<u8>, f32)> {
    if crate::composite::is_document(value) {
        let (width, height) = canvas_size(value, page)?;
        return composite_png_scaled(value, document, page, scale(width, height)?, true);
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
        let scale = scale(f64::from(scene.width), f64::from(scene.height))?;
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
    let scale = scale(f64::from(doc.canvas.width), f64::from(doc.canvas.height))?;
    Ok((crate::render::to_png(&doc, scale)?, scale))
}

/// Render the document as it is on disk, for `pentool preview`: the whole
/// page, or the `--focus` objects with a page thumbnail. Never writes the
/// document.
pub fn view(path: &Path, settings: Settings) -> Result<Value> {
    if let Some(edge) = settings.max_edge {
        if !(1..=PREVIEW_MAX_EDGE).contains(&edge) {
            anyhow::bail!("[invalid-input] --max-size must be in 1..={PREVIEW_MAX_EDGE}")
        }
    }
    crate::editing::recover_interrupted_write(path)?;
    let bytes = crate::resource::read_bounded(path, 512 * 1024 * 1024)
        .with_context(|| format!("could not read {}", path.display()))?;
    let value: Value = serde_json::from_slice(&bytes).context("invalid .pen document")?;
    let pending = Pending {
        path: path.to_owned(),
        before: Vec::new(),
        page: settings.page.clone(),
        revision: crate::transaction::revision(&bytes),
        value,
        dry_run: false,
    };
    let mut pending = pending;
    if settings.compare.as_deref() == Some("before") {
        // The revision the last recorded change started from.
        if let Some(entry) = crate::history::change_for(path, &bytes) {
            pending.before = crate::history::version(path, &entry.before)?;
        }
    }
    let mut preview = write(&pending, &settings)?;
    if let Some(object) = preview.as_object_mut() {
        object.remove("dry_run");
    }
    Ok(preview)
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
    let max_edge = settings.max_edge.unwrap_or(DEFAULT_PREVIEW_EDGE);
    let focus = if settings.no_focus {
        None
    } else if !settings.focus.is_empty() {
        Some(focus_region(value, page, &settings.focus)?)
    } else {
        changed_ids(&pending.before, value, page)
            .filter(|ids| ids.len() <= MAX_FOCUS_IDS)
            .and_then(|ids| focus_region(value, page, &ids).ok())
            .filter(|focus| focus.is_small(value, page))
    };
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
    let (mut bytes, scale, origin) = match &focus {
        Some(focus) => png_region(value, path, page, max_edge, focus)?,
        None => {
            let (bytes, scale) = png(value, path, page, max_edge)?;
            (bytes, scale, (0, 0))
        }
    };
    let comparison = match &settings.compare {
        Some(against) => Some(compare(pending, against, &bytes, scale, origin, &target)?),
        None => None,
    };
    let annotations = if settings.annotate {
        let ids = match &focus {
            Some(focus) => focus.ids.clone(),
            None => objects(value, page).into_iter().map(|(id, _)| id).collect(),
        };
        let (annotated, labels) = annotate(&bytes, value, page, scale, origin, &ids)?;
        bytes = annotated;
        Some(labels)
    } else {
        None
    };
    let (width, height) = png_size(&bytes).context("renderer returned an invalid PNG")?;
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    crate::editing::atomic_write(&target, &bytes)
        .with_context(|| format!("could not write preview {}", target.display()))?;
    let thumbnail = match &focus {
        Some(_) => {
            let (bytes, scale) = png(value, path, page, THUMBNAIL_EDGE)?;
            let (width, height) = png_size(&bytes).context("renderer returned an invalid PNG")?;
            let thumbnail = sibling(&target, "page");
            crate::editing::atomic_write(&thumbnail, &bytes)
                .with_context(|| format!("could not write preview {}", thumbnail.display()))?;
            Some(json!({
                "path": thumbnail.display().to_string(),
                "width": width,
                "height": height,
                "scale": scale,
            }))
        }
        None => None,
    };
    if settings.out.is_none() {
        prune(&target);
    }
    let page = page
        .map(str::to_owned)
        .or_else(|| value["pages"][0]["id"].as_str().map(str::to_owned));
    let mut result = json!({
        "path": target.display().to_string(),
        "page": page,
        "width": width,
        "height": height,
        "scale": scale,
        "revision": revision,
        "dry_run": dry_run,
    });
    if let (Some(focus), Some(thumbnail)) = (focus, thumbnail) {
        result["focus"] = focus.to_json();
        result["thumbnail"] = thumbnail;
    }
    if let Some(comparison) = comparison {
        result["compare"] = comparison;
    }
    if let Some(annotations) = annotations {
        result["annotations"] = annotations;
    }
    Ok(result)
}

/// Long edge of the whole-page thumbnail that accompanies a focused preview.
pub const THUMBNAIL_EDGE: u32 = 256;
/// More changed objects than this are previewed as the whole page.
pub const MAX_FOCUS_IDS: usize = 64;
/// A focused preview crops a page render of at most this many pixels; the
/// composite pipeline (effects, blur) costs more per pixel, so gets less.
const FOCUS_PAGE_PIXELS: f64 = 16.0 * 1024.0 * 1024.0;
const FOCUS_COMPOSITE_PIXELS: f64 = 4.0 * 1024.0 * 1024.0;

/// The page area a focused preview shows, in document units: the union of
/// the objects' bounds plus padding, clamped to the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct Focus {
    pub ids: Vec<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Focus {
    /// A change worth zooming into covers less than half of the page.
    fn is_small(&self, value: &Value, page: Option<&str>) -> bool {
        canvas_size(value, page)
            .is_ok_and(|(width, height)| self.width * self.height < 0.5 * width * height)
    }

    pub fn to_json(&self) -> Value {
        let r = crate::json::round3;
        json!({
            "ids": self.ids,
            "x": r(self.x),
            "y": r(self.y),
            "width": r(self.width),
            "height": r(self.height),
        })
    }

    /// `(x, y, width, height)` of the output pixels covered at `scale`, within
    /// a page `limit` pixels in size.
    fn pixels(&self, scale: f32, limit: (u32, u32)) -> (u32, u32, u32, u32) {
        let scale = f64::from(scale);
        let (max_x, max_y) = (limit.0.max(1), limit.1.max(1));
        let x = ((self.x * scale).floor() as u32).min(max_x - 1);
        let y = ((self.y * scale).floor() as u32).min(max_y - 1);
        let right = (((self.x + self.width) * scale).ceil() as u32).clamp(x + 1, max_x);
        let bottom = (((self.y + self.height) * scale).ceil() as u32).clamp(y + 1, max_y);
        (x, y, right - x, bottom - y)
    }
}

/// Width and height of the selected page's canvas (the first page, or the
/// top-level canvas of a legacy document).
fn canvas_size(value: &Value, page: Option<&str>) -> Result<(f64, f64)> {
    let canvas = match value.get("pages").and_then(Value::as_array) {
        Some(pages) if !pages.is_empty() => match page {
            Some(id) => pages
                .iter()
                .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
                .with_context(|| format!("[not-found] page not found: {id}"))?,
            None => &pages[0],
        }
        .get("canvas")
        .or_else(|| value.get("canvas")),
        _ => value.get("canvas"),
    }
    .context("[invalid-input] document has no canvas size")?;
    let size = |key: &str| {
        canvas
            .get(key)
            .and_then(Value::as_f64)
            .filter(|v| *v > 0.0)
            .with_context(|| format!("canvas {key} missing"))
    };
    Ok((size("width")?, size("height")?))
}

/// Bounds of one object in document units, for any document version.
fn object_bounds(value: &Value, page: Option<&str>, id: &str) -> Result<kurbo::Rect> {
    let bounds = if crate::scene::is_scene_document(value) {
        crate::scene::node_bounds(value, page, id)?
    } else {
        let mut doc: crate::document::Document =
            serde_json::from_value(value.clone()).context("invalid .pen document")?;
        if let Some(page) = page {
            doc.select_page(page).map_err(anyhow::Error::msg)?;
        }
        let layer = doc
            .layers
            .iter()
            .find(|layer| {
                layer.paths.iter().any(|p| p.id == id) || layer.texts.iter().any(|t| t.id == id)
            })
            .map(|layer| layer.id.clone())
            .with_context(|| format!("object not found: {id}"))?;
        crate::geometry::execute(&mut doc, &layer, id, &crate::geometry::Operation::Bounds)?
    };
    let number = |key: &str| {
        bounds[key]
            .as_f64()
            .with_context(|| format!("bounds of {id} have no {key}"))
    };
    let (x, y) = (number("x")?, number("y")?);
    Ok(kurbo::Rect::new(
        x,
        y,
        x + number("width")?,
        y + number("height")?,
    ))
}

/// Union of the objects' bounds plus padding, clamped to the page canvas.
pub fn focus_region(value: &Value, page: Option<&str>, ids: &[String]) -> Result<Focus> {
    let mut union: Option<kurbo::Rect> = None;
    for id in ids {
        let bounds = object_bounds(value, page, id)
            .with_context(|| format!("[not-found] cannot focus on `{id}`"))?;
        union = Some(union.map_or(bounds, |u| u.union(bounds)));
    }
    let union = union.context("[invalid-input] focus needs at least one object id")?;
    let (width, height) = canvas_size(value, page)?;
    let pad = (0.1 * union.width().max(union.height())).max(8.0);
    let padded = union
        .inflate(pad, pad)
        .intersect(kurbo::Rect::new(0.0, 0.0, width, height));
    if !(padded.width() > 0.0 && padded.height() > 0.0) {
        anyhow::bail!("[invalid-input] the focused objects are outside the page")
    }
    Ok(Focus {
        ids: ids.to_vec(),
        x: padded.x0,
        y: padded.y0,
        width: padded.width(),
        height: padded.height(),
    })
}

/// Render the focused region with its long edge at most `max_edge` pixels.
/// The page is rendered by the same renderer as `export` (within a pixel
/// budget, so a tiny object on a huge page gets less zoom) and cropped, so
/// every pixel equals the export at the reported scale. Returns the PNG and
/// scale, and the region's top-left corner in page pixels.
pub fn png_region(
    value: &Value,
    document: &Path,
    page: Option<&str>,
    max_edge: u32,
    focus: &Focus,
) -> Result<(Vec<u8>, f32, (u32, u32))> {
    // One pixel of slack: the region's edges are rounded outwards.
    let edge = Some(max_edge.saturating_sub(1).max(1));
    let wanted = preview_scale(focus.width, focus.height, edge, 0.1)?;
    let budget = if crate::composite::is_document(value) {
        FOCUS_COMPOSITE_PIXELS
    } else {
        FOCUS_PAGE_PIXELS
    };
    let (bytes, scale) = render_page(value, document, page, |width, height| {
        let fit = (budget / (width * height)).sqrt() as f32;
        Ok(wanted.min(fit).max(0.1))
    })?;
    let image = ::image::load_from_memory(&bytes)
        .context("could not decode the page render")?
        .to_rgba8();
    let (x, y, width, height) = focus.pixels(scale, image.dimensions());
    let cropped = ::image::imageops::crop_imm(&image, x, y, width, height).to_image();
    let mut out = std::io::Cursor::new(Vec::new());
    cropped
        .write_to(&mut out, ::image::ImageFormat::Png)
        .context("could not encode the focused preview")?;
    Ok((out.into_inner(), scale, (x, y)))
}

/// `<stem>-<suffix>.png` beside a preview image.
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let stem = target
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "preview".into());
    target.with_file_name(format!("{stem}-{suffix}.png"))
}

/// The document a preview is compared with: `before` (the state this change
/// started from), a 64-digit history revision, or a `.pen` file. Returns its
/// bytes and the path its relative resources resolve against.
fn comparison_document(pending: &Pending, against: &str) -> Result<(Vec<u8>, PathBuf)> {
    if against == "before" {
        if pending.before.is_empty() {
            anyhow::bail!(
                "[not-found] no recorded change produced this revision; compare with a history revision or a file"
            )
        }
        return Ok((pending.before.clone(), pending.path.clone()));
    }
    if against.len() == 64 && against.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok((
            crate::history::version(&pending.path, against)?,
            pending.path.clone(),
        ));
    }
    let file = PathBuf::from(against);
    let bytes = crate::resource::read_bounded(&file, 512 * 1024 * 1024)
        .with_context(|| format!("[missing-resource] cannot read {}", file.display()))?;
    Ok((bytes, file))
}

/// Compare the clean preview `image` (at `scale`, cropped at `origin`) with
/// the same view of another document. Pixels outside the other render count
/// as transparent. When any pixel differs, writes `<stem>-diff.png`: the
/// preview faded to gray with the changed pixels in red.
fn compare(
    pending: &Pending,
    against: &str,
    image: &[u8],
    scale: f32,
    origin: (u32, u32),
    target: &Path,
) -> Result<Value> {
    let (bytes, document) = comparison_document(pending, against)?;
    let other: Value = serde_json::from_slice(&bytes)
        .context("[malformed-resource] comparison document is not a .pen file")?;
    let (rendered, _) = render_page(&other, &document, pending.page.as_deref(), |_, _| Ok(scale))
        .context("could not render the comparison document")?;
    let other = ::image::load_from_memory(&rendered)
        .context("could not decode the comparison render")?
        .to_rgba8();
    let image = ::image::load_from_memory(image)
        .context("could not decode the preview")?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let mut diff = ::image::RgbaImage::new(width, height);
    let mut changed = 0u64;
    let mut bbox: Option<(u32, u32, u32, u32)> = None;
    for (x, y, pixel) in image.enumerate_pixels() {
        let then = other
            .get_pixel_checked(x + origin.0, y + origin.1)
            .copied()
            .unwrap_or(::image::Rgba([0, 0, 0, 0]));
        let out = if *pixel == then {
            let [r, g, b, a] = pixel.0.map(u32::from);
            // Luma over white, faded towards white.
            let luma = (299 * r + 587 * g + 114 * b) / 1000;
            let over_white = (luma * a + 255 * (255 - a)) / 255;
            let faded = (255 - (255 - over_white) / 4) as u8;
            ::image::Rgba([faded, faded, faded, 255])
        } else {
            changed += 1;
            bbox = Some(match bbox {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
            ::image::Rgba([255, 0, 0, 255])
        };
        diff.put_pixel(x, y, out);
    }
    let total = u64::from(width) * u64::from(height);
    let mut result = json!({
        "against": against,
        "revision": crate::transaction::revision(&bytes),
        "changed_pixels": changed,
        "changed_pct": crate::json::round3(100.0 * changed as f64 / total.max(1) as f64),
        "changed_bbox": bbox.map(|(x0, y0, x1, y1)| json!({
            "x": x0,
            "y": y0,
            "width": x1 - x0 + 1,
            "height": y1 - y0 + 1,
        })),
    });
    if changed > 0 {
        let path = sibling(target, "diff");
        let mut out = std::io::Cursor::new(Vec::new());
        diff.write_to(&mut out, ::image::ImageFormat::Png)
            .context("could not encode the diff image")?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        crate::editing::atomic_write(&path, &out.into_inner())
            .with_context(|| format!("could not write preview {}", path.display()))?;
        result["diff"] = json!({ "path": path.display().to_string() });
    }
    Ok(result)
}

/// Labels an annotated preview draws at most.
pub const MAX_ANNOTATIONS: usize = 50;
/// Objects whose bounds are looked up for labels at most, so a huge page with
/// few visible objects stays bounded.
const MAX_ANNOTATION_LOOKUPS: usize = 4 * MAX_ANNOTATIONS;
const ANNOTATION_COLORS: [&str; 6] = [
    "#e6194b", "#2f9e44", "#4363d8", "#e8590c", "#911eb4", "#0b7285",
];

/// Draw the bounds and ID of each object in `ids` that shows in the preview
/// (`scale` page pixels per unit, cropped at `origin`). The overlay is drawn
/// on the preview image only, never on the document or an export. Returns the
/// annotated PNG and `{"labels": [{id, color, x, y, width, height}],
/// "truncated"}` in preview pixels.
fn annotate(
    bytes: &[u8],
    value: &Value,
    page: Option<&str>,
    scale: f32,
    origin: (u32, u32),
    ids: &[String],
) -> Result<(Vec<u8>, Value)> {
    let mut image = ::image::load_from_memory(bytes)
        .context("could not decode the preview")?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let frame = kurbo::Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
    let scale = f64::from(scale);
    let (ox, oy) = (f64::from(origin.0), f64::from(origin.1));
    let mut labels = Vec::new();
    let mut truncated = false;
    for (looked_up, id) in ids.iter().enumerate() {
        if labels.len() == MAX_ANNOTATIONS || looked_up == MAX_ANNOTATION_LOOKUPS {
            truncated = true;
            break;
        }
        let Ok(bounds) = object_bounds(value, page, id) else {
            continue;
        };
        let rect = kurbo::Rect::new(
            bounds.x0 * scale - ox,
            bounds.y0 * scale - oy,
            bounds.x1 * scale - ox,
            bounds.y1 * scale - oy,
        )
        .intersect(frame);
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            continue;
        }
        labels.push((id.as_str(), rect.round()));
    }
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">"#
    );
    let mut list = Vec::new();
    for (index, (id, rect)) in labels.iter().enumerate() {
        let color = ANNOTATION_COLORS[index % ANNOTATION_COLORS.len()];
        let text: String = if id.chars().count() > 32 {
            id.chars().take(31).chain(std::iter::once('…')).collect()
        } else {
            (*id).to_owned()
        };
        let tag_width = 6.0 + 6.5 * text.chars().count() as f64;
        let tag_y = if rect.y0 >= 14.0 {
            rect.y0 - 14.0
        } else {
            rect.y0
        };
        let tag_x = rect.x0.min(f64::from(width) - tag_width).max(0.0);
        svg.push_str(&format!(
            r##"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="{color}" stroke-width="2"/><rect x="{tag_x}" y="{tag_y}" width="{tag_width}" height="14" fill="{color}"/><text x="{}" y="{}" font-family="{}" font-size="11" font-weight="700" fill="#ffffff">{}</text>"##,
            rect.x0 + 1.0,
            rect.y0 + 1.0,
            (rect.width() - 2.0).max(1.0),
            (rect.height() - 2.0).max(1.0),
            tag_x + 3.0,
            tag_y + 11.0,
            crate::fonts::DEFAULT_FAMILY,
            xml_escape(&text),
        ));
        list.push(json!({
            "id": id,
            "color": color,
            "x": rect.x0,
            "y": rect.y0,
            "width": rect.width(),
            "height": rect.height(),
        }));
    }
    svg.push_str("</svg>");
    let overlay = crate::render::svg_to_png(&svg, width, height, 1.0)?;
    let overlay = ::image::load_from_memory(&overlay)
        .context("could not decode the annotation overlay")?
        .to_rgba8();
    ::image::imageops::overlay(&mut image, &overlay, 0, 0);
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, ::image::ImageFormat::Png)
        .context("could not encode the annotated preview")?;
    Ok((
        out.into_inner(),
        json!({ "labels": list, "truncated": truncated }),
    ))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// IDs of the objects on `page` that the change added or modified, in
/// document order. A group counts as changed only for its own fields, not
/// for changes inside it. `None` when nothing identifiable changed.
fn changed_ids(before: &[u8], after: &Value, page: Option<&str>) -> Option<Vec<String>> {
    let before: Value = serde_json::from_slice(before).ok()?;
    let old: std::collections::HashMap<String, Value> =
        objects(&before, page).into_iter().collect();
    let changed: Vec<String> = objects(after, page)
        .into_iter()
        .filter(|(id, node)| old.get(id) != Some(node))
        .map(|(id, _)| id)
        .collect();
    (!changed.is_empty()).then_some(changed)
}

/// `(id, fields without children)` for every object on the page.
fn objects(value: &Value, page: Option<&str>) -> Vec<(String, Value)> {
    fn walk(nodes: &Value, out: &mut Vec<(String, Value)>) {
        for node in nodes.as_array().into_iter().flatten() {
            if let Some(id) = node.get("id").and_then(Value::as_str) {
                let mut own = node.clone();
                if let Some(object) = own.as_object_mut() {
                    object.remove("children");
                }
                out.push((id.to_owned(), own));
            }
            if let Some(children) = node.get("children") {
                walk(children, out);
            }
        }
    }
    let layers = match value.get("pages").and_then(Value::as_array) {
        Some(pages) if crate::scene::is_scene_document(value) => match page {
            Some(id) => pages.iter().find(|p| p["id"] == id),
            None => pages.first(),
        }
        .map(|page| &page["layers"]),
        _ => value.get("layers"),
    };
    let mut out = Vec::new();
    for layer in layers.and_then(Value::as_array).into_iter().flatten() {
        for key in ["nodes", "paths", "texts"] {
            if let Some(nodes) = layer.get(key) {
                walk(nodes, &mut out);
            }
        }
    }
    out
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
    fn changed_ids_name_edited_objects_not_their_groups() {
        let before = json!({"version": 4, "pages": [{"id": "p", "layers": [{"id": "l", "nodes": [
            {"id": "g", "kind": "group", "children": [{"id": "a", "kind": "rect", "x": 1}]},
            {"id": "b", "kind": "rect", "x": 2},
        ]}]}]});
        let mut after = before.clone();
        after["pages"][0]["layers"][0]["nodes"][0]["children"][0]["x"] = json!(5);
        after["pages"][0]["layers"][0]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "c", "kind": "rect"}));
        let bytes = serde_json::to_vec(&before).unwrap();
        assert_eq!(
            changed_ids(&bytes, &after, None),
            Some(vec!["a".to_owned(), "c".to_owned()])
        );
        assert_eq!(changed_ids(&bytes, &before, None), None);
        let legacy = json!({"version": 2, "layers": [{"id": "l", "paths": [{"id": "p1", "d": "M0 0"}], "texts": []}]});
        let mut edited = legacy.clone();
        edited["layers"][0]["paths"][0]["d"] = json!("M1 1");
        assert_eq!(
            changed_ids(&serde_json::to_vec(&legacy).unwrap(), &edited, None),
            Some(vec!["p1".to_owned()])
        );
    }

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

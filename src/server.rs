use crate::{document::Document, image, render};
use anyhow::{bail, Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::{
    hash::{Hash, Hasher},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
struct Shared {
    file: Option<PathBuf>,
    project_root: PathBuf,
    gate: Arc<Mutex<()>>,
    previews: Arc<Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
    /// Bypass the photo preview cache (`serve --no-cache`).
    no_cache: bool,
}

/// Built from `web-ui/` (`bun run build`); committed so cargo needs no Node or Bun.
const STUDIO: &str = include_str!("../web/studio.html");

pub async fn serve(host: &str, port: u16, file: Option<PathBuf>, no_cache: bool) -> Result<()> {
    if let Some(path) = &file {
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        crate::transaction::validate_value(&value)?;
    }
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .context("invalid host or port")?;
    let app = Router::new()
        .route("/", get(|| async { Html(STUDIO) }))
        .route("/studio", get(|| async { Html(STUDIO) }))
        .route("/api/raster", post(raster_command))
        .route("/api/paint/plan", post(paint_plan_command))
        .route("/api/paint/score", post(paint_score_command))
        .route("/api/paint/palette", post(paint_palette_command))
        .route("/api/paint/compare", post(paint_compare_command))
        .route("/api/paint/preview", post(paint_preview_command))
        .route("/api/paint/field", post(paint_field_command))
        .route("/api/paint/media", post(paint_media_command))
        .route("/api/image/bake", post(image_bake))
        .route("/api/migrate", post(migrate_document))
        .route("/api/image/add", post(image_add))
        .route("/api/color/preflight", get(color_preflight))
        .route("/api/color/plates", get(color_plates))
        .route("/api/color/proof", get(color_proof))
        .route("/api/color/gamut", get(color_gamut))
        .route("/api/color/pdfx", get(color_pdfx))
        .route("/api/photo/catalog", get(photo_catalog))
        .route("/api/photo/detail", get(photo_detail))
        .route("/api/photo/preview", post(photo_preview))
        .route("/api/photo/edit", post(photo_edit))
        .route("/api/composite/analyze", post(composite_analyze))
        .route("/api/composite/dependencies", get(composite_dependencies))
        .route("/api/composite/cancel", post(composite_cancel))
        .route(
            "/api/health",
            get(|| async {
                Json(serde_json::json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}))
            }),
        )
        .route("/api/render/png", post(render_png))
        .route("/api/render/svg", post(render_svg))
        .route("/api/document", get(get_document).put(put_document))
        .route("/api/scene", post(scene_command))
        .route("/api/history", get(history_list))
        .route("/api/undo", post(history_undo))
        .route("/api/redo", post(history_redo))
        .route("/api/geometry", post(geometry_command))
        .route("/api/text", post(text_command))
        .route("/api/edit", post(object_command))
        .route("/api/import", post(import_command))
        .route("/api/fonts", get(font_list))
        .route("/api/font", post(embed_font))
        .route("/api/font-info", post(font_info))
        .route("/api/assets", get(asset_search))
        .route("/api/asset", get(asset_document))
        .route("/api/registry/search", get(registry_search))
        .route("/api/registry/install", post(registry_install))
        .route("/fonts/:name", get(bundled_font))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(Shared {
            file,
            project_root: std::env::current_dir()?,
            gate: Arc::new(Mutex::new(())),
            previews: Arc::new(Mutex::new(std::collections::HashMap::new())),
            no_cache,
        });
    println!("Pentool listening on http://{address}");
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(serde::Deserialize, Default)]
struct AssetQuery {
    query: Option<String>,
    library: Option<String>,
    tag: Option<String>,
    category: Option<String>,
    kind: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}
async fn asset_search(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<AssetQuery>,
) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let project = crate::library::project_index(&state.project_root);
        let user = crate::library::user_index().ok();
        let index = crate::library::merged_indexes(Some(&project), user.as_deref())?;
        crate::library::search(
            &index,
            &crate::library::SearchOptions {
                query: q.query.as_deref(),
                library: q.library.as_deref(),
                tag: q.tag.as_deref(),
                category: q.category.as_deref(),
                kind: q.kind.as_deref(),
                offset: q.offset.unwrap_or(0),
                limit: q.limit.unwrap_or(50),
            },
        )
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
#[derive(serde::Deserialize)]
struct AssetDocumentQuery {
    spec: String,
}
async fn asset_document(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<AssetDocumentQuery>,
) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let project = crate::library::project_index(&state.project_root);
        let user = crate::library::user_index().ok();
        let index = crate::library::merged_indexes(Some(&project), user.as_deref())?;
        let item = crate::library::resolve(&index, &q.spec)?;
        let document: serde_json::Value = serde_json::from_slice(&std::fs::read(&item.path)?)?;
        Ok(serde_json::json!({"asset":item,"document":document}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
#[derive(serde::Deserialize)]
struct RegistryQuery {
    source: String,
    query: String,
}
async fn registry_search(axum::extract::Query(q): axum::extract::Query<RegistryQuery>) -> Response {
    match crate::package::registry_search(&q.source, &q.query) {
        Ok(v) => Json(v).into_response(),
        Err(e) => problem(e),
    }
}
#[derive(serde::Deserialize)]
struct RegistryInstall {
    source: String,
    spec: String,
}
async fn registry_install(
    State(state): State<Shared>,
    Json(body): Json<RegistryInstall>,
) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let mut report =
            crate::package::install_from_registry(&body.source, &body.spec, &state.project_root)?;
        let folder = report
            .get("installed")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
            .context("install path missing")?;
        let name = report
            .get("package")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("package");
        let version = report
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("0");
        let library_name = format!("pkg-{}-{version}", name.replace('/', "-"));
        let config = crate::library::project_config(&state.project_root);
        match crate::library::add(&state.project_root, &config, &library_name, &folder) {
            Ok(_) => {}
            Err(e) if e.to_string().contains("already registered") => {}
            Err(e) => return Err(e),
        };
        report["index_refresh"] = crate::library::refresh(
            &state.project_root,
            &config,
            &crate::library::project_index(&state.project_root),
            None,
        )?;
        Ok(report)
    })();
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => problem(e),
    }
}

#[derive(serde::Deserialize)]
struct TextRequest {
    document: serde_json::Value,
    page: Option<String>,
    action: crate::text::TextAction,
}
#[derive(serde::Deserialize)]
struct ObjectRequest {
    document: serde_json::Value,
    page: Option<String>,
    actions: Vec<crate::agent::ObjectAction>,
}
#[derive(serde::Deserialize)]
struct ImportRequest {
    document: serde_json::Value,
    source: serde_json::Value,
    options: crate::import::ImportOptions,
}
async fn import_command(Json(body): Json<ImportRequest>) -> Response {
    match crate::import::compose(body.document, body.source, &body.options) {
        Ok(result) => {
            Json(serde_json::json!({"document":result.document,"summary":result.summary}))
                .into_response()
        }
        Err(error) => problem(error),
    }
}
async fn object_command(Json(body): Json<ObjectRequest>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let mut doc: Document = serde_json::from_value(body.document.clone())?;
        if let Some(page) = &body.page {
            doc.select_page(page).map_err(anyhow::Error::msg)?;
        }
        doc.validate().map_err(anyhow::Error::msg)?;
        let changes = crate::agent::apply_batch(&mut doc, &body.actions)?;
        let mut raw = body.document;
        crate::agent::merge_document(&mut raw, &doc, &body.actions)?;
        Ok(serde_json::json!({"document":raw,"changes":changes}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}
async fn text_command(Json(body): Json<TextRequest>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let mut doc: Document = serde_json::from_value(body.document.clone())?;
        if let Some(page) = &body.page {
            doc.select_page(page).map_err(anyhow::Error::msg)?;
        }
        doc.validate().map_err(anyhow::Error::msg)?;
        crate::text::apply(&mut doc, body.action)?;
        let mut raw = body.document;
        crate::editing::merge(&mut raw, serde_json::to_value(doc)?);
        Ok(serde_json::json!({"document":raw}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}
async fn font_list() -> Response {
    match crate::fonts::list(&Document::new(1, 1)) {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}
#[derive(serde::Deserialize)]
struct FontRequest {
    document: serde_json::Value,
    id: String,
    data: String,
}
async fn font_info(Json(asset): Json<crate::document::FontAsset>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let bytes = crate::fonts::decode(&asset)?;
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_font_data(bytes);
        let faces:Vec<_>=db.faces().map(|face|serde_json::json!({"family":face.families.first().map(|(name,_)|name),"weight":face.weight.0,"italic":face.style!=resvg::usvg::fontdb::Style::Normal})).collect();
        Ok(serde_json::json!({"faces":faces}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}
async fn embed_font(Json(body): Json<FontRequest>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let mut doc: Document = serde_json::from_value(body.document.clone())?;
        doc.validate().map_err(anyhow::Error::msg)?;
        crate::text::add_font(
            &mut doc,
            crate::document::FontAsset {
                id: body.id,
                data: body.data,
            },
        )?;
        let mut raw = body.document;
        crate::editing::merge(&mut raw, serde_json::to_value(&doc)?);
        Ok(serde_json::json!({"document":raw,"fonts":crate::fonts::list(&doc)?}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}
async fn bundled_font(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    match crate::fonts::BUNDLED
        .iter()
        .find(|(id, _, _, _)| format!("{id}.ttf") == name)
    {
        Some((_, _, _, bytes)) => (
            [
                (header::CONTENT_TYPE, "font/ttf"),
                (header::CACHE_CONTROL, "public, max-age=3600"),
            ],
            bytes.to_vec(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(serde::Deserialize)]
struct GeometryRequest {
    document: serde_json::Value,
    page: Option<String>,
    layer: String,
    id: Option<String>,
    operation: crate::geometry::Operation,
}
async fn geometry_command(Json(body): Json<GeometryRequest>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let mut doc: Document = serde_json::from_value(body.document.clone())?;
        if let Some(page) = &body.page {
            doc.select_page(page).map_err(anyhow::Error::msg)?;
        }
        doc.validate().map_err(anyhow::Error::msg)?;
        let result = match &body.id {
            Some(id) => crate::geometry::execute(&mut doc, &body.layer, id, &body.operation)?,
            None => crate::geometry::execute_layer(&mut doc, &body.layer, &body.operation)?,
        };
        let mut document = body.document;
        crate::editing::merge(&mut document, serde_json::to_value(doc)?);
        Ok(serde_json::json!({"document":document,"result":result}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

#[derive(serde::Deserialize)]
struct RasterRequest {
    document: serde_json::Value,
    page: Option<String>,
    id: String,
    action: String,
    #[serde(default)]
    args: serde_json::Value,
}

/// Stateless raster editing for the canvas: the whole document in, the changed
/// document and a compact result out. Nothing is written unless it validates.
async fn raster_command(Json(body): Json<RasterRequest>) -> Response {
    match raster_apply(body) {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

#[derive(serde::Deserialize)]
struct PaintPlanRequest {
    document: serde_json::Value,
    page: Option<String>,
    plan: serde_json::Value,
    /// Preview only: the report is identical to a commit but no document is returned.
    #[serde(default)]
    dry_run: bool,
    /// Refuse unless the canonical plan hashes to this value (`[plan-changed]`).
    expect_plan_hash: Option<String>,
    /// Cancellable job ID for `/api/composite/cancel`; a cancelled plan writes nothing.
    job: Option<String>,
}

/// Stateless plan execution. A preview and a commit share one planner, so the
/// report a reviewer sees is exactly what the commit returns.
async fn paint_plan_command(
    State(state): State<Shared>,
    Json(body): Json<PaintPlanRequest>,
) -> Response {
    let job = body.job.clone();
    match run_job(&state, job, "plan", move || paint_plan_apply(body)).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

fn paint_plan_apply(body: PaintPlanRequest) -> Result<serde_json::Value> {
    if let Some(expected) = &body.expect_plan_hash {
        let actual = crate::paint_plan::validate_template(&body.plan)?;
        anyhow::ensure!(
            &actual == expected,
            "[plan-changed] the plan hashes to {actual}, not the expected {expected}; review the plan again. The document was not changed"
        );
    }
    let mut raw = body.document;
    crate::transaction::validate_value(&raw)?;
    let result = crate::paint_plan::execute(&mut raw, body.page.as_deref(), &body.plan)?;
    crate::transaction::validate_value(&raw)?;
    if body.dry_run {
        Ok(serde_json::json!({"result": result, "dry_run": true}))
    } else {
        Ok(serde_json::json!({"result": result, "dry_run": false, "document": raw}))
    }
}

#[derive(serde::Deserialize)]
struct PaintScoreRequest {
    before: serde_json::Value,
    after: serde_json::Value,
    layer: String,
    goal: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct PaintPaletteRequest {
    document: serde_json::Value,
    page: Option<String>,
    /// `page`, `node:ID`, `layer:ID` or `group:ID`.
    #[serde(default = "page_scope")]
    scope: String,
    rect: Option<[u32; 4]>,
    /// `sample` (a point or neighborhood) or `extract` (a role palette).
    action: String,
    x: Option<u32>,
    y: Option<u32>,
    #[serde(default)]
    radius: u32,
    #[serde(default = "dominant_role")]
    role: String,
    #[serde(default = "default_palette_count")]
    count: usize,
}
fn page_scope() -> String {
    "page".into()
}
fn dominant_role() -> String {
    "dominant".into()
}
fn default_palette_count() -> usize {
    6
}

/// Read-only palette sampling from the rendered canvas, so a pass palette can be
/// built from what is actually on the page.
async fn paint_palette_command(
    State(state): State<Shared>,
    Json(body): Json<PaintPaletteRequest>,
) -> Response {
    let document = state
        .file
        .clone()
        .unwrap_or_else(|| state.project_root.join("browser.pen"));
    match run_job(&state, None, "palette", move || {
        paint_palette_apply(&document, body)
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

fn paint_palette_apply(
    document: &std::path::Path,
    body: PaintPaletteRequest,
) -> Result<serde_json::Value> {
    {
        let source = crate::paint_palette::source(
            &body.document,
            document,
            body.page.as_deref(),
            &body.scope,
            body.rect,
            None,
        )?;
        match body.action.as_str() {
            "sample" => {
                let (Some(x), Some(y)) = (body.x, body.y) else {
                    anyhow::bail!("[invalid-request] sample needs x and y");
                };
                crate::paint_palette::sample(&source, x, y, body.radius)
            }
            "extract" => {
                crate::paint_palette::extract(&source, &body.scope, &body.role, body.count)
            }
            other => anyhow::bail!("[invalid-request] action {other:?} must be sample or extract"),
        }
    }
}

#[derive(serde::Deserialize)]
struct PaintCompareRequest {
    document: serde_json::Value,
    page: Option<String>,
    #[serde(default = "page_scope")]
    scope: String,
    rect: Option<[u32; 4]>,
    /// The reference image as base64 PNG/JPEG (at most 8 MiB decoded).
    reference_image_base64: String,
    #[serde(default = "white")]
    background: String,
    /// Also return the value-difference heat map as a base64 PNG overlay.
    #[serde(default)]
    heatmap: bool,
}
fn white() -> String {
    "#ffffff".into()
}

/// Read-only comparison of the current canvas region with a reference image.
async fn paint_compare_command(
    State(state): State<Shared>,
    Json(body): Json<PaintCompareRequest>,
) -> Response {
    let document = state
        .file
        .clone()
        .unwrap_or_else(|| state.project_root.join("browser.pen"));
    match run_job(&state, None, "compare", move || {
        paint_compare_apply(&document, body)
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

fn paint_compare_apply(
    document: &std::path::Path,
    body: PaintCompareRequest,
) -> Result<serde_json::Value> {
    {
        use base64::Engine;
        use rand_core::RngCore;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body.reference_image_base64.trim())
            .context("[invalid-request] reference image must be base64")?;
        anyhow::ensure!(
            bytes.len() <= 8 * 1024 * 1024,
            "[limit-exceeded] reference image is larger than 8 MiB"
        );
        // The comparer reads a file; stage it under a unique temp name that is
        // removed on every path.
        let format = ::image::guess_format(&bytes)
            .context("[malformed-resource] reference image is not a recognized format")?;
        let path = std::env::temp_dir().join(format!(
            "pentool-reference-{:016x}.{}",
            rand_core::OsRng.next_u64(),
            format.extensions_str().first().copied().unwrap_or("img")
        ));
        std::fs::write(&path, &bytes).context("cannot stage the reference image")?;
        struct Remove(std::path::PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _remove = Remove(path.clone());
        let (mut report, heat) = crate::paint_review::compare_with_heat(
            &body.document,
            document,
            body.page.as_deref(),
            crate::paint_review::CompareOptions {
                scope: body.scope,
                rect: body.rect,
                selection: None,
                reference_image: Some(path.to_string_lossy().into_owned()),
                reference_document: None,
                reference_scope: None,
                background: crate::paint_review::parse_background(&body.background)?,
                heatmap: None,
            },
        )?;
        if body.heatmap {
            anyhow::ensure!(
                u64::from(heat.width()) * u64::from(heat.height()) <= 4 << 20,
                "[limit-exceeded] heat map exceeds 4194304 pixels; narrow rect"
            );
            report["heatmap_png_base64"] =
                serde_json::Value::String(png_base64(&::image::DynamicImage::ImageLuma8(heat))?);
        }
        Ok(report)
    }
}

fn png_base64(image: &::image::DynamicImage) -> Result<String> {
    use base64::Engine;
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, ::image::ImageFormat::Png)
        .context("cannot encode the preview PNG")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

#[derive(serde::Deserialize)]
struct PaintPreviewRequest {
    document: serde_json::Value,
    page: Option<String>,
    #[serde(default = "page_scope")]
    scope: String,
    rect: Option<[u32; 4]>,
    max_edge: Option<u32>,
}

/// Bounded crop preview of a canvas region as `{report, png_base64}`.
async fn paint_preview_command(
    State(state): State<Shared>,
    Json(body): Json<PaintPreviewRequest>,
) -> Response {
    let document = state
        .file
        .clone()
        .unwrap_or_else(|| state.project_root.join("browser.pen"));
    match run_job(&state, None, "preview", move || {
        paint_preview_apply(&document, body)
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

fn paint_preview_apply(
    document: &std::path::Path,
    body: PaintPreviewRequest,
) -> Result<serde_json::Value> {
    let (report, image) = crate::paint_review::preview_image(
        &body.document,
        document,
        body.page.as_deref(),
        crate::paint_review::PreviewOptions {
            scope: body.scope,
            rect: body.rect,
            selection: None,
            max_edge: body.max_edge.unwrap_or(512).min(1024),
            max_pixels: 1 << 20,
            out: None,
            background: None,
        },
    )?;
    Ok(serde_json::json!({
        "report": report,
        "png_base64": png_base64(&::image::DynamicImage::ImageRgba8(image))?,
    }))
}

#[derive(serde::Deserialize)]
struct PaintFieldRequest {
    document: serde_json::Value,
    layer: String,
    field: serde_json::Value,
    params: Option<serde_json::Value>,
    constraints: Option<serde_json::Value>,
    #[serde(default)]
    brush: Option<serde_json::Value>,
    #[serde(default)]
    seed: u64,
    #[serde(default = "default_max_vectors")]
    max_vectors: usize,
}
fn default_max_vectors() -> usize {
    200
}

/// Field visualization: the built field's summary and sample vectors, plus the
/// strokes painting through it would generate when `params` is given.
async fn paint_field_command(
    State(state): State<Shared>,
    Json(body): Json<PaintFieldRequest>,
) -> Response {
    let work = move || {
        crate::paint_fields::preview_value(
            &body.document,
            &body.layer,
            &body.field,
            body.params,
            body.constraints,
            &body.brush.unwrap_or_else(|| serde_json::json!({})),
            body.seed,
            body.max_vectors.min(1000),
        )
    };
    match run_job(&state, None, "field", work).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

#[derive(serde::Deserialize)]
struct PaintMediaRequest {
    /// Omit to list the material kinds; give a media object to canonicalise it.
    media: Option<serde_json::Value>,
}

/// Material controls: the media kinds, or the canonical form of one media spec.
async fn paint_media_command(Json(body): Json<PaintMediaRequest>) -> Response {
    let result = match &body.media {
        None => Ok(crate::paint_media::list_value()),
        Some(media) => crate::paint_media::describe_value(media),
    };
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

/// Read-only before/after scoring of one layer against a goal.
async fn paint_score_command(Json(body): Json<PaintScoreRequest>) -> Response {
    match crate::paint_score::score(&body.before, &body.after, &body.layer, &body.goal) {
        Ok(value) => Json(value).into_response(),
        Err(e) => problem(e),
    }
}

fn raster_apply(body: RasterRequest) -> Result<serde_json::Value> {
    let mut raw = body.document;
    if body.action == "add" {
        // Creating the layer is not a pixel operation, so it has no existing id to apply to.
        let canvas_side = |side: &str| -> u32 {
            raw["pages"]
                .as_array()
                .and_then(|pages| match body.page.as_deref() {
                    Some(wanted) => pages.iter().find(|p| p["id"] == wanted),
                    None => pages.first(),
                })
                .and_then(|page| page["canvas"][side].as_u64())
                .unwrap_or(1200) as u32
        };
        let arg = |name: &str, fallback: u32| -> u32 {
            body.args
                .get(name)
                .and_then(serde_json::Value::as_u64)
                .map_or(fallback, |value| value as u32)
        };
        let width = arg("width", canvas_side("width"));
        let height = arg("height", canvas_side("height"));
        let x = body
            .args
            .get("x")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let y = body
            .args
            .get("y")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let layer = body.args.get("layer").and_then(serde_json::Value::as_str);
        let result = crate::raster::add(
            &mut raw,
            body.page.as_deref(),
            layer,
            &body.id,
            x,
            y,
            width,
            height,
        )?;
        crate::transaction::validate_value(&raw)?;
        return Ok(serde_json::json!({"document": raw, "result": result}));
    }
    let result = crate::raster::apply(
        &mut raw,
        body.page.as_deref(),
        &body.id,
        &body.action,
        &body.args,
    )?;
    crate::transaction::validate_value(&raw)?;
    Ok(serde_json::json!({"document": raw, "result": result}))
}

async fn get_document(State(state): State<Shared>) -> Response {
    let Some(file) = state.file else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"No shared document; use pentool serve artwork.pen"})),
        )
            .into_response();
    };
    match std::fs::read(file).and_then(|bytes| {
        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(std::io::Error::other)
    }) {
        Ok(document) => {
            Json(serde_json::json!({"revision":revision(&document),"document":document}))
                .into_response()
        }
        Err(e) => problem(e.into()),
    }
}

#[derive(serde::Deserialize)]
struct SceneRequest {
    document: Option<serde_json::Value>,
    operations: Vec<serde_json::Value>,
    page: Option<String>,
    revision: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(serde::Deserialize)]
struct AnalysisRequest {
    document: serde_json::Value,
    page: Option<String>,
    #[serde(default = "analysis_scope")]
    scope: String,
    query: Option<serde_json::Value>,
    #[serde(default)]
    samples: Vec<[u32; 2]>,
    #[serde(default)]
    compare: bool,
}
fn analysis_scope() -> String {
    "page".into()
}
async fn composite_analyze(
    State(state): State<Shared>,
    Json(body): Json<AnalysisRequest>,
) -> Response {
    let document = state
        .file
        .unwrap_or_else(|| state.project_root.join("browser.pen"));
    // CPU-heavy analysis runs off the async request executor.
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
        let analysis = crate::inspect::analyze(
            &body.document,
            &document,
            body.page.as_deref(),
            &body.scope,
            body.query.as_ref(),
            &body.samples,
        )?;
        let comparison = if body.compare {
            Some(crate::inspect::compare(
                &body.document,
                &document,
                body.page.as_deref(),
                &body.scope,
                body.query.as_ref(),
            )?)
        } else {
            None
        };
        Ok(serde_json::json!({"analysis":analysis,"comparison":comparison}))
    })
    .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => problem(error),
        Err(error) => problem(error.into()),
    }
}
#[derive(serde::Deserialize)]
struct PreflightQuery {
    page: Option<String>,
    min_ppi: Option<f64>,
    min_bleed_inches: Option<f64>,
    max_total_ink: Option<f64>,
}

/// Read-only print preflight and resolved production geometry of the shared
/// document; nothing is written.
#[derive(serde::Deserialize)]
struct PlateQuery {
    page: Option<String>,
    plate: Option<String>,
    scale: Option<f32>,
    /// Proof-setup rendering intent for `/api/color/proof` and `/api/color/gamut`.
    intent: Option<String>,
    /// Black point compensation for the soft proof and gamut views.
    bpc: Option<bool>,
}

/// Parse a proof-setup intent name; absent means the output role's own intent.
fn proof_intent(name: Option<&str>) -> Result<Option<crate::color::icc::Intent>> {
    use crate::color::icc::Intent;
    Ok(match name {
        None => None,
        Some("perceptual") => Some(Intent::Perceptual),
        Some("relative-colorimetric") => Some(Intent::RelativeColorimetric),
        Some("saturation") => Some(Intent::Saturation),
        Some("absolute-colorimetric") => Some(Intent::AbsoluteColorimetric),
        Some(other) => anyhow::bail!(
            "[invalid-input] unknown intent {other}; use perceptual, relative-colorimetric, saturation or absolute-colorimetric"
        ),
    })
}

/// Bounded working raster for print views: at most ~1M pixels unless a smaller
/// scale is requested.
fn print_view_scale(raw: &serde_json::Value, page: Option<&str>, requested: Option<f32>) -> f32 {
    let pages = raw["pages"].as_array().cloned().unwrap_or_default();
    let page = match page {
        Some(id) => pages.iter().find(|p| p["id"] == id),
        None => pages.first(),
    };
    let area = page.map_or(1.0, |p| {
        p["canvas"]["width"].as_f64().unwrap_or(1.0).max(1.0)
            * p["canvas"]["height"].as_f64().unwrap_or(1.0).max(1.0)
    });
    let bounded = (1_000_000.0 / area).sqrt().clamp(0.05, 1.0) as f32;
    requested.map_or(bounded, |s| s.clamp(0.05, bounded))
}

fn shared_document(state: &Shared) -> Result<(std::path::PathBuf, serde_json::Value)> {
    let file = state
        .file
        .clone()
        .context("[not-found] no shared document")?;
    let raw = serde_json::from_slice(&crate::resource::read_bounded(
        &file,
        crate::color::catalog::MAX_CATALOG_DOCUMENT_BYTES,
    )?)?;
    Ok((file, raw))
}

fn png_response(bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response()
}

/// Read-only plate viewer: without `plate`, per-plate coverage; with `plate`, that
/// plate as a grayscale PNG (white is no ink). Nothing is written.
async fn color_plates(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PlateQuery>,
) -> Response {
    let result = (|| -> Result<Response> {
        let (file, raw) = shared_document(&state)?;
        let scale = print_view_scale(&raw, q.page.as_deref(), q.scale);
        let separations = crate::color::separation::separate(
            &raw,
            &file,
            q.page.as_deref(),
            &crate::color::separation::Options {
                destination: None,
                intent: None,
                scale,
                auto_trap: None,
            },
        )?;
        let pixels = (separations.width as usize * separations.height as usize).max(1);
        match q.plate {
            None => {
                let plates: Vec<serde_json::Value> = separations
                    .plates
                    .iter()
                    .map(|plate| {
                        let sum: f64 = plate.samples.iter().sum();
                        serde_json::json!({"plate":plate.id,"spot":plate.spot,
                            "average_coverage_percent":sum / pixels as f64 * 100.0,
                            "max_percent":plate.samples.iter().fold(0.0_f64, |a, b| a.max(*b)) * 100.0})
                    })
                    .collect();
                Ok(Json(serde_json::json!({"width":separations.width,
                    "height":separations.height,"scale":scale,"plates":plates}))
                .into_response())
            }
            Some(id) => {
                let plate = separations
                    .plates
                    .iter()
                    .find(|plate| plate.id == id)
                    .with_context(|| format!("[missing-resource] plate not found: {id}"))?;
                let gray: Vec<u8> = plate
                    .samples
                    .iter()
                    .map(|v| ((1.0 - v.clamp(0.0, 1.0)) * 255.0 + 0.5).floor() as u8)
                    .collect();
                let buffer =
                    ::image::GrayImage::from_raw(separations.width, separations.height, gray)
                        .context("[internal] plate buffer size")?;
                let mut png = Vec::new();
                buffer.write_to(
                    &mut std::io::Cursor::new(&mut png),
                    ::image::ImageFormat::Png,
                )?;
                Ok(png_response(png))
            }
        }
    })();
    result.unwrap_or_else(problem)
}

/// Read-only soft-proof image of the output role; nothing is written.
async fn color_proof(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PlateQuery>,
) -> Response {
    let result = (|| -> Result<Response> {
        let (file, raw) = shared_document(&state)?;
        let intent = proof_intent(q.intent.as_deref())?;
        let proof = crate::color::proofimage::render_setup(
            &raw,
            &file,
            q.page.as_deref(),
            false,
            crate::color::proofimage::Setup {
                intent,
                black_point_compensation: q.bpc.unwrap_or(false),
            },
        )?;
        Ok(png_response(proof.png))
    })();
    result.unwrap_or_else(problem)
}

#[derive(serde::Deserialize)]
struct PdfxQuery {
    page: Option<String>,
    ppi: Option<f64>,
    intent: Option<String>,
    compress: Option<bool>,
    title: Option<String>,
}

/// Flattened CMYK PDF/X-4 draft as a download. Read only: the document is not
/// touched and nothing is written next to it. Not a conformance claim.
async fn color_pdfx(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PdfxQuery>,
) -> Response {
    let result = (|| -> Result<Response> {
        let (file, raw) = shared_document(&state)?;
        let defaults = crate::color::pdfx::Options::default();
        let built = crate::color::pdfx::build(
            &raw,
            &file,
            q.page.as_deref(),
            &crate::color::pdfx::Options {
                intent: proof_intent(q.intent.as_deref())?,
                ppi: q.ppi.unwrap_or(defaults.ppi),
                title: q.title,
                compress: q.compress.unwrap_or(true),
                ..defaults
            },
        )?;
        Ok((
            [
                (header::CONTENT_TYPE, "application/pdf"),
                (header::CACHE_CONTROL, "no-cache"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"pentool-pdfx-draft.pdf\"",
                ),
            ],
            built.bytes,
        )
            .into_response())
    })();
    result.unwrap_or_else(problem)
}

async fn color_gamut(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PlateQuery>,
) -> Response {
    let result = (|| -> Result<Response> {
        let (file, raw) = shared_document(&state)?;
        let intent = proof_intent(q.intent.as_deref())?;
        let proof = crate::color::proofimage::render_setup(
            &raw,
            &file,
            q.page.as_deref(),
            true,
            crate::color::proofimage::Setup {
                intent,
                black_point_compensation: q.bpc.unwrap_or(false),
            },
        )?;
        Ok(png_response(proof.png))
    })();
    result.unwrap_or_else(problem)
}

async fn color_preflight(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PreflightQuery>,
) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let result = (|| -> Result<serde_json::Value> {
        let raw: serde_json::Value = serde_json::from_slice(&crate::resource::read_bounded(
            &file,
            crate::color::catalog::MAX_CATALOG_DOCUMENT_BYTES,
        )?)?;
        let defaults = crate::color::preflight::Options::default();
        let mut report = crate::color::preflight::run(
            &raw,
            &file,
            q.page.as_deref(),
            &crate::color::preflight::Options {
                min_ppi: q.min_ppi.unwrap_or(defaults.min_ppi),
                min_bleed_inches: q.min_bleed_inches.unwrap_or(defaults.min_bleed_inches),
                bleed_scale: defaults.bleed_scale,
                max_total_ink: q.max_total_ink,
                allowed_spots: None,
                k_only_text: false,
                require_marks: false,
                reject_spot_flattening: false,
            },
        )?;
        let pages = raw["pages"].as_array().cloned().unwrap_or_default();
        let page = match q.page.as_deref() {
            Some(id) => pages.iter().find(|p| p["id"] == id),
            None => pages.first(),
        };
        report["production"] = match page {
            Some(page) => crate::color::production::resolve(page, false)?
                .map_or(serde_json::Value::Null, |g| g.json()),
            None => serde_json::Value::Null,
        };
        Ok(report)
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

async fn composite_dependencies(State(state): State<Shared>) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let result = (|| -> Result<serde_json::Value> {
        let raw = serde_json::from_slice(&std::fs::read(&file)?)?;
        crate::linked::report(&raw, &file)
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
async fn scene_command(State(state): State<Shared>, Json(body): Json<SceneRequest>) -> Response {
    if let Some(mut raw) = body.document {
        let result = crate::scene::apply_batch_at(
            &mut raw,
            body.page.as_deref(),
            &body.operations,
            &state.project_root.join("browser.pen"),
        );
        return match result {
            Ok(changes) => Json(serde_json::json!({"document":raw,"changes":changes,"local":true}))
                .into_response(),
            Err(error) => problem(error),
        };
    }
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let result = (|| -> Result<serde_json::Value> {
        let _guard = state
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("document lock failed"))?;
        let bytes = std::fs::read(&file)?;
        let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
        require_revision(body.revision.as_deref(), &raw)?;
        let changes =
            crate::scene::apply_batch_at(&mut raw, body.page.as_deref(), &body.operations, &file)?;
        let expected = crate::transaction::revision(&bytes);
        let change = crate::transaction::commit_value(
            &file,
            "browser-scene",
            body.dry_run,
            Some(&expected),
            &raw,
        )?;
        Ok(serde_json::json!({"change":change,"changes":changes}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AddImageRequest {
    document: Option<serde_json::Value>,
    id: String,
    page: Option<String>,
    layer: String,
    data: String,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    migrate: bool,
    #[serde(default)]
    dry_run: bool,
}

fn plan_image_add(
    raw: &mut serde_json::Value,
    body: &AddImageRequest,
) -> Result<serde_json::Value> {
    use base64::Engine;
    crate::transaction::validate_value(raw)?;
    if body.revision != revision(raw) {
        anyhow::bail!("document revision changed; reload before adding the image")
    }
    if body.data.len() > 44 * 1024 * 1024 {
        anyhow::bail!("[limit-exceeded] browser imports accept images up to 32 MiB")
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body.data)
        .context("image data must be base64")?;
    if bytes.len() > 32 * 1024 * 1024 {
        anyhow::bail!("[limit-exceeded] browser imports accept images up to 32 MiB")
    }
    let mut candidate = raw.clone();
    if !crate::composite::is_document(raw) && raw["version"] != 5 {
        if !body.migrate {
            anyhow::bail!("adding an image requires v5; explicitly allow format migration")
        }
        candidate = crate::scene::migrate_to_v5(candidate)?;
    }
    let page = crate::scene::page_mut(&mut candidate, body.page.as_deref())?;
    let layer = page["layers"]
        .as_array()
        .context("page layers missing")?
        .iter()
        .find(|layer| layer["id"] == body.layer)
        .context("image target layer not found")?;
    if layer["locked"] == true {
        anyhow::bail!("[locked-node] image target layer is locked; unlock it first")
    }
    let width = page["canvas"]["width"]
        .as_f64()
        .context("canvas width missing")?;
    let height = page["canvas"]["height"]
        .as_f64()
        .context("canvas height missing")?;
    let source = image::decode_source(&bytes)?;
    let factor = (width * 0.8 / f64::from(source.pixel_width))
        .min(height * 0.8 / f64::from(source.pixel_height))
        .min(1.0);
    let w = f64::from(source.pixel_width) * factor;
    let h = f64::from(source.pixel_height) * factor;
    let added = image::add(
        &mut candidate,
        body.page.as_deref(),
        &body.layer,
        &body.id,
        &bytes,
        image::embedded_storage(&bytes),
        (width - w) / 2.0,
        (height - h) / 2.0,
        w,
        h,
        image::Fit::Contain,
    )?;
    *raw = candidate;
    Ok(serde_json::to_value(added)?)
}

#[derive(serde::Deserialize)]
struct MigrateRequest {
    document: serde_json::Value,
    target: u64,
}

/// Upgrades an in-memory document to a newer format version with the same
/// migrations as `pentool migrate`; nothing is written to disk.
async fn migrate_document(Json(body): Json<MigrateRequest>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let from = body
            .document
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .context("[invalid-input] the document has no version")?;
        if !(4..=crate::color::VERSION).contains(&body.target) {
            bail!(
                "[invalid-input] migration target must be between 4 and {}",
                crate::color::VERSION
            );
        }
        if body.target <= from {
            bail!(
                "[invalid-input] the document is already format v{from}; choose a newer target than v{from}"
            );
        }
        let migrated = match body.target {
            8 => crate::color::catalog::migrate(body.document)?,
            7 => crate::photo::catalog::migrate(body.document)?,
            6 => crate::composite::migrate(body.document)?,
            5 => crate::scene::migrate_to_v5(body.document)?,
            _ => crate::scene::migrate_to_v4(body.document)?,
        };
        Ok(serde_json::json!({
            "document": migrated,
            "from_version": from,
            "to_version": body.target,
        }))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

async fn image_add(State(state): State<Shared>, Json(mut body): Json<AddImageRequest>) -> Response {
    if let Some(mut raw) = body.document.take() {
        body.revision = revision(&raw);
        return match plan_image_add(&mut raw, &body) {
            Ok(result) => Json(serde_json::json!({"document":raw,"result":result,"local":true}))
                .into_response(),
            Err(error) => problem(error),
        };
    }
    let Some(file) = state.file else {
        return (
            StatusCode::NOT_FOUND,
            "No shared document; serve a .pen file first",
        )
            .into_response();
    };
    let result = (|| -> Result<serde_json::Value> {
        let _guard = state
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("document lock failed"))?;
        let bytes = std::fs::read(&file)?;
        let mut raw = serde_json::from_slice(&bytes)?;
        let result = plan_image_add(&mut raw, &body)?;
        let expected = crate::transaction::revision(&bytes);
        let change = crate::transaction::commit_value(
            &file,
            "browser-image-add",
            body.dry_run,
            Some(&expected),
            &raw,
        )?;
        Ok(serde_json::json!({"change":change,"result":result}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
struct BakeRequest {
    id: String,
    page: Option<String>,
    revision: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

/// Bake through the same shared service and transaction as `image bake`.
async fn image_bake(State(state): State<Shared>, Json(body): Json<BakeRequest>) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let result = (|| -> Result<serde_json::Value> {
        let _guard = state
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("document lock failed"))?;
        let mut raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
        if body
            .revision
            .as_deref()
            .is_some_and(|wanted| wanted != revision(&raw))
        {
            anyhow::bail!("document revision changed")
        }
        let result = crate::image::bake(
            &mut raw,
            body.page.as_deref(),
            &file,
            &body.id,
            !body.dry_run,
        )?;
        let change = crate::transaction::commit_value(
            &file,
            "browser-image-bake",
            body.dry_run,
            None,
            &raw,
        )?;
        Ok(serde_json::json!({"change":change,"result":result}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
#[derive(serde::Deserialize, Default)]
struct PhotoCatalogQuery {
    #[serde(default)]
    query: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

/// The shared document's bytes and their revision, for the photo endpoints.
fn shared_photo_document(state: &Shared) -> Result<(PathBuf, serde_json::Value, String)> {
    let file = state
        .file
        .clone()
        .context("[missing-resource] no shared document; run `pentool serve CATALOG.pen`")?;
    let bytes = std::fs::read(&file)?;
    let raw = serde_json::from_slice(&bytes)?;
    Ok((file, raw, crate::transaction::revision(&bytes)))
}

/// The grid and filmstrip: `photo search` rows of the shared document.
async fn photo_catalog(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PhotoCatalogQuery>,
) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let (_, raw, revision) = shared_photo_document(&state)?;
        let mut value = crate::photo::organize::search(
            &raw,
            &q.query,
            q.limit.unwrap_or(200),
            q.offset.unwrap_or(0),
        )?;
        value["revision"] = serde_json::json!(revision);
        Ok(value)
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
struct PhotoDetailQuery {
    photo: String,
}

async fn photo_detail(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<PhotoDetailQuery>,
) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let (_, raw, revision) = shared_photo_document(&state)?;
        let mut value = crate::photo::studio::detail(&raw, &q.photo)?;
        value["revision"] = serde_json::json!(revision);
        Ok(value)
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
struct PhotoPreviewRequest {
    photo: String,
    #[serde(default = "master")]
    variant: String,
    edge: Option<u32>,
    #[serde(default = "srgb")]
    space: String,
    #[serde(default)]
    overlay: String,
    #[serde(default)]
    uncropped: bool,
}

fn master() -> String {
    "master".into()
}

fn srgb() -> String {
    "srgb".into()
}

/// Develop a preview: `{report, png}` with the PNG base64-encoded, tagged with
/// the preview space's `iCCP` and `cICP`.
fn photo_preview_value(
    raw: &serde_json::Value,
    file: &std::path::Path,
    body: &PhotoPreviewRequest,
    no_cache: bool,
) -> Result<serde_json::Value> {
    use crate::photo::{cache::Cache, studio};
    use base64::Engine;
    let cache = (!no_cache).then(|| Cache::for_document(file));
    let preview = studio::preview_cached(
        raw,
        file,
        &studio::PreviewRequest {
            photo: &body.photo,
            variant: &body.variant,
            edge: body.edge.unwrap_or(studio::DEFAULT_EDGE),
            space: &body.space,
            overlay: studio::Overlay::parse(&body.overlay)?,
            uncropped: body.uncropped,
        },
        cache.as_ref(),
    )?;
    Ok(serde_json::json!({
        "report": preview.report,
        "png": base64::engine::general_purpose::STANDARD.encode(preview.png),
    }))
}

async fn photo_preview(
    State(state): State<Shared>,
    Json(body): Json<PhotoPreviewRequest>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
        let (file, raw, revision) = shared_photo_document(&state)?;
        let mut value = photo_preview_value(&raw, &file, &body, state.no_cache)?;
        value["revision"] = serde_json::json!(revision);
        Ok(value)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|r| r);
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
struct PhotoEditRequest {
    edit: serde_json::Value,
    /// The revision the browser last read; the edit is refused if the
    /// document changed since.
    revision: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

/// Apply one photo edit under the document gate as a guarded transaction.
fn photo_edit_apply(
    file: &std::path::Path,
    gate: &Mutex<()>,
    body: &PhotoEditRequest,
) -> Result<serde_json::Value> {
    let _guard = gate
        .lock()
        .map_err(|_| anyhow::anyhow!("document lock failed"))?;
    let bytes = std::fs::read(file)?;
    let current = crate::transaction::revision(&bytes);
    let expected = body.revision.clone().unwrap_or(current.clone());
    if expected != current {
        anyhow::bail!(
            "[conflict] the document changed since revision {expected} (now {current}); reload the photo and edit again"
        )
    }
    let mut raw = serde_json::from_slice(&bytes)?;
    let result = crate::photo::studio::edit(&mut raw, file, &body.edit)?;
    let operation = format!(
        "browser-photo-{}",
        body.edit["op"].as_str().unwrap_or("edit")
    );
    let change =
        crate::transaction::commit_value(file, &operation, body.dry_run, Some(&expected), &raw)?;
    let revision = crate::transaction::revision(&std::fs::read(file)?);
    Ok(serde_json::json!({"change":change,"result":result,"revision":revision}))
}

async fn photo_edit(State(state): State<Shared>, Json(body): Json<PhotoEditRequest>) -> Response {
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
        let file = state
            .file
            .clone()
            .context("[missing-resource] no shared document; run `pentool serve CATALOG.pen`")?;
        photo_edit_apply(&file, &state.gate, &body)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|r| r);
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

async fn history_list(State(state): State<Shared>) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    match crate::history::list(&file) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
async fn history_undo(State(state): State<Shared>) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    match crate::history::undo(&file) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
async fn history_redo(State(state): State<Shared>) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    match crate::history::redo(&file) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

async fn put_document(
    State(state): State<Shared>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let result = (|| -> Result<String> {
        let _guard = state
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("document lock failed"))?;
        let current: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
        let unchanged = match body.get("revision").and_then(|v| v.as_str()) {
            Some(token) => token == revision(&current),
            None => body.get("base") == Some(&current),
        };
        if !unchanged {
            anyhow::bail!(
                "Document changed on disk. Reload before saving to avoid overwriting agent edits."
            );
        }
        let value = body.get("document").context("missing document")?;
        crate::transaction::commit_value(&file, "browser-save", false, None, value)?;
        Ok(revision(value))
    })();
    match result {
        Ok(revision) => Json(serde_json::json!({"ok":true,"revision":revision})).into_response(),
        Err(e) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error":e.to_string()})),
        )
            .into_response(),
    }
}

// Opaque optimistic-concurrency token: clients must retain it without rebuilding
// the loaded JSON (JavaScript changes integer/float number representations).
fn revision(value: &serde_json::Value) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Optimistic-concurrency guard for shared-document commits: a missing
/// revision must fail rather than commit unguarded against concurrent writers.
fn require_revision(revision: Option<&str>, raw: &serde_json::Value) -> Result<()> {
    let wanted = revision
        .with_context(|| "scene batch requires the document revision from GET /api/document")?;
    if wanted != self::revision(raw) {
        anyhow::bail!("document revision changed")
    }
    Ok(())
}

#[derive(serde::Deserialize, Default)]
struct PageQuery {
    page: Option<String>,
    max_edge: Option<u32>,
    job: Option<String>,
}

#[derive(serde::Deserialize)]
struct CancelRequest {
    job: String,
}
async fn composite_cancel(
    State(state): State<Shared>,
    Json(body): Json<CancelRequest>,
) -> Response {
    let jobs = state.previews.lock();
    match jobs {
        Ok(jobs) => {
            let found = jobs.get(&body.job);
            if let Some(flag) = found {
                flag.store(true, std::sync::atomic::Ordering::Relaxed)
            }
            Json(serde_json::json!({"cancelled":found.is_some()})).into_response()
        }
        Err(_) => problem(anyhow::anyhow!("preview registry lock failed")),
    }
}

async fn composite_job(
    state: &Shared,
    value: serde_json::Value,
    query: PageQuery,
    svg: bool,
) -> Result<Vec<u8>> {
    let scale = composite_preview_scale(&value, query.page.as_deref(), query.max_edge)?;
    let document = state
        .file
        .clone()
        .unwrap_or_else(|| state.project_root.join("browser.pen"));
    run_job(state, query.job, "export", move || {
        if svg {
            Ok(
                crate::composite::svg(&value, &document, query.page.as_deref(), scale)?
                    .into_bytes(),
            )
        } else {
            crate::composite::png(&value, &document, query.page.as_deref(), scale)
        }
    })
    .await
}

/// Run cancellable blocking work under a registered job ID. `/api/composite/cancel`
/// raises the flag; dropping the request future cancels the work too.
async fn run_job<T: Send + 'static>(
    state: &Shared,
    job: Option<String>,
    prefix: &str,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    use rand_core::RngCore;
    let job = job.unwrap_or_else(|| format!("{prefix}-{:016x}", rand_core::OsRng.next_u64()));
    if job.is_empty()
        || job.len() > 128
        || !job
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        anyhow::bail!("invalid preview job ID")
    }
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut jobs = state
            .previews
            .lock()
            .map_err(|_| anyhow::anyhow!("preview registry lock failed"))?;
        if jobs.len() >= 4 || jobs.contains_key(&job) {
            anyhow::bail!("[limit-exceeded] preview workers busy; cancel or retry")
        }
        jobs.insert(job.clone(), flag.clone());
    }
    struct JobGuard {
        jobs: Arc<Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
        job: String,
        flag: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Drop for JobGuard {
        fn drop(&mut self) {
            self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut jobs) = self.jobs.lock() {
                jobs.remove(&self.job);
            }
        }
    }
    let guard = JobGuard {
        jobs: state.previews.clone(),
        job,
        flag: flag.clone(),
    };
    struct CancelOnDrop(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let cancel_on_drop = CancelOnDrop(flag.clone());
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        crate::composite::with_cancel(flag, work)
    })
    .await
    .context("preview worker failed")?;
    drop(cancel_on_drop);
    result
}

fn composite_preview_scale(
    value: &serde_json::Value,
    page: Option<&str>,
    max_edge: Option<u32>,
) -> Result<f32> {
    let Some(edge) = max_edge else { return Ok(1.0) };
    if !(1..=4096).contains(&edge) {
        anyhow::bail!("preview max_edge must be in 1..=4096")
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
    let longest = selected["canvas"]["width"]
        .as_u64()
        .unwrap()
        .max(selected["canvas"]["height"].as_u64().unwrap());
    Ok((edge as f32 / longest as f32).min(1.0))
}

async fn render_png(
    State(state): State<Shared>,
    axum::extract::Query(query): axum::extract::Query<PageQuery>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    if crate::composite::is_document(&value) {
        return match composite_job(&state, value, query, false).await {
            Ok(bytes) => binary(bytes, "image/png", "artwork.png"),
            Err(error) => problem(error),
        };
    }
    let result = (|| -> Result<Vec<u8>> {
        if crate::composite::is_document(&value) {
            let document = state
                .file
                .clone()
                .unwrap_or_else(|| state.project_root.join("browser.pen"));
            let scale = composite_preview_scale(&value, query.page.as_deref(), query.max_edge)?;
            return crate::composite::png(&value, &document, query.page.as_deref(), scale);
        }
        if value.get("version").and_then(serde_json::Value::as_u64) == Some(image::VERSION) {
            let document = state
                .file
                .clone()
                .unwrap_or_else(|| state.project_root.join("browser.pen"));
            let scene = image::to_svg(&value, &document, query.page.as_deref())?;
            return render::scene_to_png(&scene, 1.0);
        }
        let value = if crate::scene::is_scene_document(&value) {
            crate::scene::flatten_to_v3(&value)?
        } else {
            value
        };
        let mut doc: Document = serde_json::from_value(value)?;
        if let Some(page) = query.page {
            if let Err(error) = doc.select_page(&page) {
                return Err(anyhow::Error::msg(error));
            }
        }
        render::to_png_cached(&doc, 1.0).map(|(bytes, _)| bytes)
    })();
    match result {
        Ok(bytes) => binary(bytes, "image/png", "artwork.png"),
        Err(error) => problem(error),
    }
}

async fn render_svg(
    State(state): State<Shared>,
    axum::extract::Query(query): axum::extract::Query<PageQuery>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    if crate::composite::is_document(&value) {
        let proxy = query.max_edge.is_some();
        return match composite_job(&state, value, query, true).await {
            Ok(bytes) => {
                let mut response = binary(bytes, "image/svg+xml", "artwork.svg");
                response.headers_mut().insert(
                    "x-pentool-preview",
                    HeaderValue::from_static(if proxy {
                        "approximate"
                    } else {
                        "authoritative"
                    }),
                );
                response
            }
            Err(error) => problem(error),
        };
    }
    let max_edge = query.max_edge;
    let result = (|| -> Result<String> {
        if crate::composite::is_document(&value) {
            let document = state
                .file
                .clone()
                .unwrap_or_else(|| state.project_root.join("browser.pen"));
            let scale = composite_preview_scale(&value, query.page.as_deref(), query.max_edge)?;
            return crate::composite::svg(&value, &document, query.page.as_deref(), scale);
        }
        if value.get("version").and_then(serde_json::Value::as_u64) == Some(image::VERSION) {
            let document = state
                .file
                .clone()
                .unwrap_or_else(|| state.project_root.join("browser.pen"));
            return Ok(match query.max_edge {
                Some(edge) => image::to_svg_proxy(&value, &document, query.page.as_deref(), edge)?,
                None => image::to_svg(&value, &document, query.page.as_deref())?,
            }
            .svg);
        }
        let value = if crate::scene::is_scene_document(&value) {
            crate::scene::flatten_to_v3(&value)?
        } else {
            value
        };
        let mut doc: Document = serde_json::from_value(value)?;
        if let Some(page) = query.page {
            if let Err(error) = doc.select_page(&page) {
                return Err(anyhow::Error::msg(error));
            }
        }
        render::to_svg(&doc)
    })();
    match result {
        Ok(svg) => {
            let mut response = binary(svg.into_bytes(), "image/svg+xml", "artwork.svg");
            response.headers_mut().insert(
                "x-pentool-preview",
                HeaderValue::from_static(if max_edge.is_some() {
                    "approximate"
                } else {
                    "authoritative"
                }),
            );
            response
        }
        Err(error) => problem(error),
    }
}

fn binary(bytes: Vec<u8>, kind: &'static str, name: &'static str) -> Response {
    let mut response = Body::from(bytes).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).unwrap(),
    );
    response
}

fn problem(error: anyhow::Error) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": error.to_string()})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn paint_plan_endpoint_previews_and_commits_the_same_report() {
        use serde_json::json;
        let mut raw = crate::scene::new_document(64, 64);
        crate::raster::add(&mut raw, None, None, "p", 0.0, 0.0, 64, 64).unwrap();
        let plan = json!({
            "schema": 1, "engine": 1, "id": "ui",
            "passes": [{"id": "a", "target": "p", "action": "stroke-path",
                        "args": {"path": "M4 10 L60 10", "color": "#336699",
                                 "brush": {"size": 8, "hardness": 1}}}]
        });
        let request = |dry_run: bool, hash: Option<String>| super::PaintPlanRequest {
            document: raw.clone(),
            page: None,
            plan: plan.clone(),
            dry_run,
            expect_plan_hash: hash,
            job: None,
        };
        let preview = super::paint_plan_apply(request(true, None)).unwrap();
        assert!(preview.get("document").is_none());
        let hash = preview["result"]["plan_hash"].as_str().unwrap().to_owned();
        let commit = super::paint_plan_apply(request(false, Some(hash))).unwrap();
        assert_eq!(preview["result"], commit["result"]);
        assert_ne!(commit["document"], raw);
        let stale = super::paint_plan_apply(request(false, Some("sha256:0".into())));
        assert!(stale.unwrap_err().to_string().contains("plan-changed"));
    }

    #[test]
    fn palette_and_compare_endpoints_read_the_canvas_without_mutation() {
        use base64::Engine;
        use serde_json::json;
        let mut raw = crate::scene::new_document(32, 32);
        crate::raster::add(&mut raw, None, None, "p", 0.0, 0.0, 32, 32).unwrap();
        let plan = json!({
            "schema": 1, "engine": 1, "id": "pal",
            "passes": [{"id": "a", "target": "p", "action": "fill",
                        "args": {"color": "#cc3322"}}]
        });
        crate::paint_plan::execute(&mut raw, None, &plan).unwrap();
        let before = raw.clone();
        let here = std::path::Path::new("browser.pen");
        let request = |action: &str| super::PaintPaletteRequest {
            document: raw.clone(),
            page: None,
            scope: "page".into(),
            rect: None,
            action: action.into(),
            x: Some(4),
            y: Some(4),
            radius: 1,
            role: "dominant".into(),
            count: 2,
        };
        let sampled = super::paint_palette_apply(here, request("sample")).unwrap();
        assert!(sampled.to_string().contains("#cc3322"), "{sampled}");
        let extracted = super::paint_palette_apply(here, request("extract")).unwrap();
        assert!(extracted.to_string().contains("#cc3322"), "{extracted}");
        assert!(super::paint_palette_apply(here, request("bogus")).is_err());

        // A reference image that is solid blue differs from the red canvas.
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(32, 32, image::Rgba([0, 0, 255, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let compared = super::paint_compare_apply(
            here,
            super::PaintCompareRequest {
                document: raw.clone(),
                page: None,
                scope: "page".into(),
                rect: None,
                reference_image_base64: base64::engine::general_purpose::STANDARD
                    .encode(png.into_inner()),
                background: "#ffffff".into(),
                heatmap: true,
            },
        )
        .unwrap();
        assert!(compared["metrics"].is_object(), "{compared}");
        assert!(compared["heatmap_png_base64"].as_str().unwrap().len() > 20);
        let shown = super::paint_preview_apply(
            here,
            super::PaintPreviewRequest {
                document: raw.clone(),
                page: None,
                scope: "page".into(),
                rect: Some([0, 0, 16, 16]),
                max_edge: Some(64),
            },
        )
        .unwrap();
        assert_eq!(shown["report"]["source"], serde_json::json!([16, 16]));
        assert!(shown["png_base64"].as_str().unwrap().len() > 20);
        let bad = super::paint_compare_apply(
            here,
            super::PaintCompareRequest {
                document: raw.clone(),
                page: None,
                scope: "page".into(),
                rect: None,
                reference_image_base64: "!!!".into(),
                background: "#ffffff".into(),
                heatmap: false,
            },
        );
        assert!(bad.is_err());
        assert_eq!(raw, before);
    }

    #[test]
    fn raster_endpoint_adds_a_page_sized_paint_layer_to_a_new_document() {
        let raw = crate::scene::new_document(64, 48);
        let value = super::raster_apply(super::RasterRequest {
            document: raw,
            page: None,
            id: "paint-1".into(),
            action: "add".into(),
            args: serde_json::Value::Null,
        })
        .unwrap();
        let document = &value["document"];
        assert!(document["version"].as_u64().unwrap() >= 6);
        let info = crate::raster::info(document, None, "paint-1").unwrap();
        assert_eq!(info["width"], 64);
        assert_eq!(info["height"], 48);
    }

    #[test]
    fn raster_endpoint_paints_selects_and_rejects_without_mutation() {
        use serde_json::json;
        let mut raw = crate::scene::new_document(40, 40);
        crate::raster::add(&mut raw, None, None, "p", 0.0, 0.0, 40, 40).unwrap();
        let request = |document: &serde_json::Value, action: &str, args: serde_json::Value| {
            super::raster_apply(super::RasterRequest {
                document: document.clone(),
                page: None,
                id: "p".into(),
                action: action.into(),
                args,
            })
        };
        let painted = request(
            &raw,
            "stroke",
            json!({"samples":[{"x":5,"y":5},{"x":30,"y":30}],"brush":{"size":6},"color":"#ff0000"}),
        )
        .unwrap();
        assert_ne!(painted["document"], raw);
        assert_eq!(painted["result"]["input"]["samples"], 2);
        let selected = request(
            &painted["document"],
            "select-marquee",
            json!({"x":2,"y":2,"width":10,"height":10}),
        )
        .unwrap();
        let info = request(&selected["document"], "select-info", json!({})).unwrap();
        assert!(
            info["result"]["selected_pixels"].as_u64().unwrap() == 100,
            "{info}"
        );
        // A bad action or malformed input is an error and the caller's document is untouched.
        let before = painted["document"].clone();
        assert!(request(&before, "nope", json!({})).is_err());
        assert!(request(&before, "stroke", json!({"samples":[]})).is_err());
        assert_eq!(before, painted["document"]);
    }

    #[test]
    fn browser_image_add_is_page_aware_atomic_and_requires_explicit_migration() {
        use base64::Engine;
        use serde_json::json;
        let mut encoded = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgba8(::image::RgbaImage::from_pixel(
            2,
            1,
            ::image::Rgba([10, 20, 30, 255]),
        ))
        .write_to(&mut encoded, ::image::ImageFormat::Png)
        .unwrap();
        let mut raw = crate::scene::new_document(20, 20);
        let mut second = raw["pages"][0].clone();
        second["id"] = json!("second");
        raw["pages"].as_array_mut().unwrap().push(second);
        let mut request = super::AddImageRequest {
            document: None,
            id: "photo".into(),
            page: Some("second".into()),
            layer: "layer-1".into(),
            data: base64::engine::general_purpose::STANDARD.encode(encoded.into_inner()),
            revision: super::revision(&raw),
            migrate: false,
            dry_run: false,
        };
        let before = raw.clone();
        assert!(super::plan_image_add(&mut raw, &request).is_err());
        assert_eq!(raw, before);
        request.migrate = true;
        super::plan_image_add(&mut raw, &request).unwrap();
        assert_eq!(raw["version"], 5);
        assert!(raw["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(raw["pages"][1]["layers"][0]["nodes"][0]["id"], "photo");
        let before = raw.clone();
        assert!(super::plan_image_add(&mut raw, &request).is_err());
        assert_eq!(raw, before);
        request.revision = super::revision(&raw);
        request.id = "other".into();
        request.data = "invalid".into();
        assert!(super::plan_image_add(&mut raw, &request).is_err());
        assert_eq!(raw, before);
        let mut raw = crate::composite::migrate(before).unwrap();
        raw["pages"][1]["layers"][0]["locked"] = json!(true);
        request.revision = super::revision(&raw);
        request.data = base64::engine::general_purpose::STANDARD.encode(b"not a PNG");
        let before = raw.clone();
        assert!(super::plan_image_add(&mut raw, &request)
            .unwrap_err()
            .to_string()
            .contains("locked"));
        assert_eq!(raw, before);
    }
    #[test]
    fn composite_proxy_bounds_and_cancellation_are_explicit() {
        let raw = crate::composite::migrate(crate::scene::new_document(16384, 8)).unwrap();
        let scale = super::composite_preview_scale(&raw, None, Some(1024)).unwrap();
        assert_eq!(scale, 0.0625);
        let pixels =
            crate::composite::render(&raw, std::path::Path::new("target/proxy.pen"), None, scale)
                .unwrap();
        assert_eq!(pixels.dimensions(), (1024, 1));
        assert!(super::composite_preview_scale(&raw, None, Some(0)).is_err());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        assert!(crate::composite::with_cancel(flag, crate::composite::check_cancelled).is_err());
        crate::composite::check_cancelled().unwrap();
    }
    #[test]
    fn revision_tracks_disk_values_without_a_browser_round_trip() {
        let original: serde_json::Value = serde_json::from_str(r#"{"x":100.0}"#).unwrap();
        let browser: serde_json::Value = serde_json::from_str(r#"{"x":100}"#).unwrap();
        assert_ne!(original, browser);
        let loaded_token = super::revision(&original);
        assert_eq!(loaded_token, super::revision(&original));
        assert_ne!(loaded_token, super::revision(&browser));
        assert_ne!(loaded_token, super::revision(&serde_json::json!({"x":101})));
    }

    #[test]
    fn scene_commit_requires_a_revision() {
        let raw = serde_json::json!({"format": "pentool", "version": 4});
        let err = super::require_revision(None, &raw).unwrap_err();
        assert!(
            err.to_string().contains("requires the document revision"),
            "unexpected error: {err}"
        );
        let err = super::require_revision(Some("stale"), &raw).unwrap_err();
        assert!(
            err.to_string().contains("document revision changed"),
            "unexpected error: {err}"
        );
        super::require_revision(Some(&super::revision(&raw)), &raw).unwrap();
    }
}

use crate::preview::{composite_png, composite_preview_scale, preview_scale};
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
    /// Open `/api/document/events` streams, bounded by [`MAX_DOCUMENT_STREAMS`].
    document_streams: Arc<std::sync::atomic::AtomicUsize>,
}

/// Built from `web-ui/` (`bun run build`); committed so cargo needs no Node or Bun.
const STUDIO: &str = include_str!("../web/studio.html");

/// The browser renderer (`wasm/`), embedded when present at build time (see `build.rs`).
#[cfg(has_wasm_core)]
const CORE_WASM: Option<&[u8]> = Some(include_bytes!("../web/pentool_core.wasm"));
#[cfg(not(has_wasm_core))]
const CORE_WASM: Option<&[u8]> = None;

async fn core_wasm() -> Response {
    match CORE_WASM {
        Some(bytes) => {
            let mut response = Body::from(bytes).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/wasm"),
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            response
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn build_app(
    file: Option<PathBuf>,
    no_cache: bool,
    collab_ctx: Option<Arc<crate::collab::sync::Ctx>>,
) -> Result<Router> {
    if let Some(path) = &file {
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        crate::transaction::validate_value(&value)?;
    }
    let mut app = Router::new()
        .route("/", get(|| async { Html(STUDIO) }))
        .route("/studio", get(|| async { Html(STUDIO) }))
        .route("/core/pentool_core.wasm", get(core_wasm))
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
        .route("/api/scene3d/import", post(scene3d_import))
        .route("/api/scene3d/render-report", post(scene3d_render_report))
        .route("/api/scene3d/form", post(scene3d_form))
        .route("/api/scene3d/fold", post(scene3d_fold))
        .route("/api/scene3d/pack/settings", post(scene3d_pack_settings))
        .route("/api/scene3d/crease-angle", post(scene3d_crease_angle))
        .route(
            "/api/scene3d/crease-direction",
            post(scene3d_crease_direction),
        )
        .route("/api/scene3d/pack/preflight", post(scene3d_pack_preflight))
        .route(
            "/api/scene3d/pack/flat-preview",
            post(scene3d_pack_flat_preview),
        )
        .route(
            "/api/scene3d/pack/export-bundle",
            post(scene3d_pack_export_bundle),
        )
        .route(
            "/api/scene3d/pack/face-preview",
            post(scene3d_pack_face_preview),
        )
        .route("/api/scene3d/sleeve", post(scene3d_sleeve))
        .route("/api/scene3d/carton", post(scene3d_carton))
        .route("/api/scene3d/tube-label", post(scene3d_tube_label))
        .route("/api/scene3d/pouch", post(scene3d_pouch))
        .route("/api/scene3d/bottle-label", post(scene3d_bottle_label))
        .route("/api/scene3d/panel-art", post(scene3d_panel_art))
        .route(
            "/api/scene3d/panel-art-transform",
            post(scene3d_panel_art_transform),
        )
        .route(
            "/api/scene3d/panel-safe-zone",
            post(scene3d_panel_safe_zone),
        )
        .route("/api/scene3d/path-role", post(scene3d_path_role))
        .route("/api/scene3d/path-crease", post(scene3d_path_crease))
        .route("/api/scene3d/uv/preview", post(scene3d_uv_preview))
        .route(
            "/api/scene3d/uv/model-preview",
            post(scene3d_uv_model_preview),
        )
        .route("/api/scene3d/uv/face-pick", post(scene3d_uv_face_pick))
        .route("/api/scene3d/uv/pack", post(scene3d_uv_pack))
        .route("/api/scene3d/status", get(scene3d_status))
        .route(
            "/api/scene3d/cache",
            get(scene3d_cache_status).delete(scene3d_cache_clear),
        )
        .route("/api/color/preflight", get(color_preflight))
        .route("/api/layout", post(layout_command))
        .route("/api/layout/preflight", get(layout_preflight))
        .route("/api/layout/overview", get(layout_overview))
        .route(
            "/api/layout/export",
            get(layout_export).post(layout_export_document),
        )
        .route("/api/color/plates", get(color_plates))
        .route("/api/color/proof", get(color_proof))
        .route("/api/color/gamut", get(color_gamut))
        .route("/api/color/pdfx", get(color_pdfx))
        .route("/api/photo/catalog", get(photo_catalog))
        .route("/api/photo/detail", get(photo_detail))
        .route("/api/photo/import", post(photo_import))
        .route("/api/photo/place", post(photo_place))
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
        .route("/api/document/events", get(document_events))
        .route("/api/scene", post(scene_command))
        .route("/api/design/tokens", get(design_tokens))
        .route("/api/design/library/status", get(design_library_status))
        .route("/api/design/audit", get(design_audit))
        .route("/api/design/templates", get(design_templates))
        .route("/api/design/template/check", get(design_template_check))
        .route("/api/design/template/fill", post(design_template_fill))
        .route("/api/ai/status", get(ai_status))
        .route("/api/ai/connect", post(ai_connect))
        .route("/api/ai/doctor", post(ai_doctor))
        .route("/api/ai/remove", post(ai_remove))
        .route("/api/extensions", get(extensions_list))
        .route("/api/extensions/run", post(extensions_run))
        .route("/api/comments", get(comments_list).post(comments_apply))
        .route("/api/history", get(history_list))
        .route("/api/collab/stats", get(collab_stats))
        .route("/api/collab/conflicts", get(collab_conflicts))
        .route("/api/history/version", get(history_version))
        .route("/api/history/restore", post(history_restore))
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
        .route("/fonts/:name", get(bundled_font));
    if let Some(ctx) = collab_ctx {
        app = app
            .merge(crate::collab::sync::routes(&ctx.limits))
            .layer(axum::middleware::from_fn_with_state(
                ctx.clone(),
                crate::collab::perm::guard,
            ))
            .layer(axum::Extension(ctx));
    }
    let app = app
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(Shared {
            file,
            project_root: std::env::current_dir()?,
            gate: Arc::new(Mutex::new(())),
            previews: Arc::new(Mutex::new(std::collections::HashMap::new())),
            no_cache,
            document_streams: Arc::default(),
        });
    Ok(app)
}

/// Options for `serve --collab`.
pub struct CollabOptions {
    pub limits: crate::collab::sync::Limits,
    pub allow_insecure_lan: bool,
}

pub async fn serve(host: &str, port: u16, file: Option<PathBuf>, no_cache: bool) -> Result<()> {
    serve_with(host, port, file, no_cache, None).await
}

pub async fn serve_with(
    host: &str,
    port: u16,
    file: Option<PathBuf>,
    no_cache: bool,
    collab: Option<CollabOptions>,
) -> Result<()> {
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .context("invalid host or port")?;
    if let (Some(options), Some(path)) = (&collab, &file) {
        crate::collab::sync::check_bind(path, host, options.allow_insecure_lan)?;
    }
    if let Some(path) = &file {
        if !path.is_file() {
            bail!(
                "[missing-resource] document not found: {}; create it with `pentool new` or check the path",
                path.display()
            );
        }
    }
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| {
            format!(
                "[conflict] could not listen on {address}; the port may already be in use, try another with --port"
            )
        })?;
    println!("Pentool listening on http://{address}");
    serve_listener(listener, file, no_cache, collab).await
}

/// Serve on an already-bound listener (used by tests that need port 0).
pub async fn serve_listener(
    listener: tokio::net::TcpListener,
    file: Option<PathBuf>,
    no_cache: bool,
    collab: Option<CollabOptions>,
) -> Result<()> {
    let loopback = listener.local_addr()?.ip().is_loopback();
    let ctx = match collab {
        Some(options) => {
            let Some(path) = &file else {
                bail!("[invalid-input] --collab needs a document: pentool serve DOC --collab");
            };
            let ctx = crate::collab::sync::Ctx::new(path, options.limits, loopback)?;
            Some(Arc::new(ctx))
        }
        None => None,
    };
    let app = build_app(file, no_cache, ctx)?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
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
    format: Option<String>,
    source_name: Option<String>,
}
async fn import_command(Json(body): Json<ImportRequest>) -> Response {
    if let Some(format) = body.format.clone() {
        return match plan_design_import(body, &format) {
            Ok(result) => Json(result).into_response(),
            Err(error) => problem(error),
        };
    }
    match crate::import::compose(body.document, body.source, &body.options) {
        Ok(result) => {
            Json(serde_json::json!({"document":result.document,"summary":result.summary}))
                .into_response()
        }
        Err(error) => problem(error),
    }
}

fn plan_design_import(body: ImportRequest, format: &str) -> Result<serde_json::Value> {
    use base64::Engine;
    let encoded = body
        .source
        .as_str()
        .context("[invalid-input] design source must be base64-encoded bytes")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("[malformed-resource] design source is not valid base64")?;
    const MAX_STUDIO_IMPORT_BYTES: usize = 32 * 1024 * 1024;
    anyhow::ensure!(
        bytes.len() <= MAX_STUDIO_IMPORT_BYTES,
        "[limit-exceeded] Studio design imports are limited to {MAX_STUDIO_IMPORT_BYTES} bytes"
    );
    let source = body.source_name.as_deref().unwrap_or("imported-design");
    let imported = crate::interchange::import_design(format, &bytes, source)?;
    let mut report = imported.report;
    let prefix = body.options.prefix.as_deref().unwrap_or_else(|| {
        std::path::Path::new(source)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("import")
    });
    let (document, summary) = crate::interchange::append_imported_document(
        &body.document,
        &imported.document,
        body.options.destination_page.as_deref(),
        prefix,
        &mut report,
    )?;
    Ok(serde_json::json!({
        "document": document,
        "summary": summary,
        "report": report,
        "local": true
    }))
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
                .and_then(|page| page["canvas"][side].as_f64())
                .map_or(1200, |v| v.round() as u32)
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

/// Open-tab limit for `/api/document/events`; each Studio tab holds one stream.
const MAX_DOCUMENT_STREAMS: usize = 32;
/// How often a stream checks the document file's size and modification time.
const DOCUMENT_POLL: std::time::Duration = std::time::Duration::from_millis(250);
/// Re-hash the file at least this often, in case a write kept size and mtime.
const DOCUMENT_RECHECK: std::time::Duration = std::time::Duration::from_secs(5);
/// Streams end after this long; clients reconnect and get the current revision.
const DOCUMENT_STREAM_LIFETIME: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Releases a `/api/document/events` slot when its stream ends.
struct DocumentStreamSlot(Arc<std::sync::atomic::AtomicUsize>);

impl DocumentStreamSlot {
    fn acquire(open: &Arc<std::sync::atomic::AtomicUsize>) -> Option<Self> {
        use std::sync::atomic::Ordering;
        // Construct the guard first so a refused slot is released on return.
        let slot = Self(open.clone());
        (open.fetch_add(1, Ordering::SeqCst) < MAX_DOCUMENT_STREAMS).then_some(slot)
    }
}

impl Drop for DocumentStreamSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Revision of the document on disk, plus the history entry that produced
/// it (operation, actor, time) when there is one; `None` while unreadable.
fn disk_revision(file: &std::path::Path) -> Option<(String, Option<crate::history::Entry>)> {
    let bytes = std::fs::read(file).ok()?;
    let document = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    Some((
        revision(&document),
        crate::history::change_for(file, &bytes),
    ))
}

/// Server-sent `revision` events: the current revision of the shared document
/// on connect, then each time any writer (Studio, the CLI, an agent or a
/// collaboration peer) changes the file. The revision matches
/// `GET /api/document`, so a client reloads only when it differs from its own.
async fn document_events(State(state): State<Shared>) -> Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let Some(file) = state.file.clone() else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let Some(slot) = DocumentStreamSlot::acquire(&state.document_streams) else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "too many open document streams; close a Studio tab and retry"
            })),
        )
            .into_response();
    };
    type Stamp = Option<(Option<std::time::SystemTime>, u64)>;
    let stamp = |file: &std::path::Path| -> Stamp {
        std::fs::metadata(file)
            .ok()
            .map(|meta| (meta.modified().ok(), meta.len()))
    };
    let started = std::time::Instant::now();
    let initial: (Stamp, Option<String>, std::time::Instant) = (None, None, started);
    let stream = futures_util::stream::unfold(
        (file, slot, initial),
        move |(file, slot, (mut seen, last, mut hashed))| async move {
            loop {
                if started.elapsed() > DOCUMENT_STREAM_LIFETIME {
                    return None;
                }
                let now = stamp(&file);
                if last.is_none() || now != seen || hashed.elapsed() >= DOCUMENT_RECHECK {
                    let path = file.clone();
                    let current = tokio::task::spawn_blocking(move || disk_revision(&path))
                        .await
                        .ok()
                        .flatten();
                    hashed = std::time::Instant::now();
                    // An unreadable file (mid-write elsewhere) is retried next tick.
                    if let Some((current, change)) = current {
                        seen = now;
                        if last.as_deref() != Some(current.as_str()) {
                            let mut data = serde_json::json!({ "revision": current });
                            if let Some(change) = change {
                                data["change"] = serde_json::json!(change);
                            }
                            let event = Event::default().event("revision").data(data.to_string());
                            return Some((
                                Ok::<Event, std::convert::Infallible>(event),
                                (file, slot, (seen, Some(current), hashed)),
                            ));
                        }
                    }
                }
                tokio::time::sleep(DOCUMENT_POLL).await;
            }
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
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

#[derive(Clone, serde::Deserialize)]
struct Scene3dImportRequest {
    document: serde_json::Value,
    data: String,
    format: String,
    id: String,
    layer: String,
    page: Option<String>,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default = "scene3d_default_extent")]
    width: f64,
    #[serde(default = "scene3d_default_extent")]
    height: f64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dFormRequest {
    document: serde_json::Value,
    action: String,
    form_id: String,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    parameters: Option<serde_json::Value>,
    #[serde(default)]
    material_slots: Option<Vec<String>>,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    page: Option<String>,
    #[serde(default)]
    placement: Option<[f64; 4]>,
}

fn plan_scene3d_form(request: &Scene3dFormRequest) -> Result<serde_json::Value> {
    use serde_json::json;

    anyhow::ensure!(
        request
            .document
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|v| v >= 11),
        "[unsupported-version] product forms need document format v11 or newer; upgrade the document first"
    );
    let mut candidate = request.document.clone();
    let form = match request.action.as_str() {
        "add" => {
            anyhow::ensure!(
                !request.form_id.trim().is_empty(),
                "[invalid-input] product form ID cannot be empty"
            );
            anyhow::ensure!(
                candidate
                    .get("product_forms")
                    .and_then(|forms| forms.get(&request.form_id))
                    .is_none(),
                "[invalid-input] product form `{}` already exists",
                request.form_id
            );
            let mut form = if let Some(preset) = &request.preset {
                anyhow::ensure!(
                    request.kind.is_none() && request.parameters.is_none(),
                    "[invalid-input] a product-form preset cannot be combined with custom kind or parameters"
                );
                crate::scene3d::form::starter_preset(preset)?
            } else {
                let kind = request
                    .kind
                    .as_deref()
                    .context("[invalid-input] new product form kind or preset is required")?;
                let parameters = request
                    .parameters
                    .as_ref()
                    .context("[invalid-input] new product form parameters are required")?;
                anyhow::ensure!(
                    parameters.is_object(),
                    "[invalid-input] product form parameters must be an object"
                );
                json!({"kind":kind,"generator":1,"parameters":parameters})
            };
            if let Some(slots) = &request.material_slots {
                form["material_slots"] = json!(slots);
            }
            form
        }
        "set" => {
            anyhow::ensure!(
                request.preset.is_none() && request.kind.is_none(),
                "[invalid-input] product-form presets and kinds apply only when adding a form"
            );
            let current = candidate
                .get("product_forms")
                .and_then(|forms| forms.get(&request.form_id))
                .with_context(|| {
                    format!(
                        "[missing-resource] product form `{}` does not exist",
                        request.form_id
                    )
                })?;
            let mut form = current.clone();
            let parameters = request
                .parameters
                .as_ref()
                .context("[invalid-input] updated product form parameters are required")?;
            anyhow::ensure!(
                parameters.is_object(),
                "[invalid-input] product form parameters must be an object"
            );
            form["parameters"] = parameters.clone();
            if let Some(slots) = &request.material_slots {
                form["material_slots"] = json!(slots);
            }
            form
        }
        other => anyhow::bail!("[invalid-input] product form action `{other}` must be add or set"),
    };
    let model = crate::scene3d::form::generate(&form)
        .with_context(|| format!("product form `{}`", request.form_id))?;
    candidate["product_forms"][&request.form_id] = form;
    let result = if request.action == "add" {
        let node_id = request
            .node_id
            .as_deref()
            .context("[invalid-input] new product form node ID is required")?;
        let layer_id = request
            .layer
            .as_deref()
            .context("[invalid-input] product form target layer is required")?;
        let page_id = request
            .page
            .clone()
            .or_else(|| {
                candidate["pages"]
                    .as_array()
                    .and_then(|pages| pages.first())
                    .and_then(|page| page["id"].as_str())
                    .map(str::to_owned)
            })
            .context("[invalid-input] product form target page is required")?;
        let placement = request.placement.unwrap_or([80.0, 80.0, 480.0, 480.0]);
        anyhow::ensure!(
            placement
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0),
            "[invalid-input] product form placement values must be finite and nonnegative"
        );
        anyhow::ensure!(
            placement[2] > 0.0 && placement[3] > 0.0,
            "[invalid-input] product form node width and height must be positive"
        );
        let page = candidate["pages"]
            .as_array_mut()
            .context("[invalid-input] document pages are missing")?
            .iter_mut()
            .find(|page| page["id"].as_str() == Some(page_id.as_str()))
            .with_context(|| format!("[missing-resource] page `{page_id}` does not exist"))?;
        let layer = page["layers"]
            .as_array_mut()
            .context("[invalid-input] target page has no layers")?
            .iter_mut()
            .find(|layer| layer["id"].as_str() == Some(layer_id))
            .with_context(|| {
                format!("[missing-resource] layer `{layer_id}` does not exist on page `{page_id}`")
            })?;
        anyhow::ensure!(
            layer["locked"].as_bool() != Some(true),
            "[locked] target layer `{layer_id}` is locked"
        );
        layer["nodes"].as_array_mut().context("[invalid-input] target layer has no node array")?
            .push(json!({"kind":"scene3d","id":node_id,"x":placement[0],"y":placement[1],"width":placement[2],"height":placement[3],"product_form":request.form_id}));
        json!({"node":node_id,"form":request.form_id,"vertices":model.report.vertices,"triangles":model.report.triangles})
    } else {
        json!({"form":request.form_id,"vertices":model.report.vertices,"triangles":model.report.triangles})
    };
    crate::transaction::validate_value(&candidate)?;
    Ok(json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_form(Json(request): Json<Scene3dFormRequest>) -> Response {
    match plan_scene3d_form(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dFoldRequest {
    document: serde_json::Value,
    dieline: String,
    fold_state: f64,
}

fn plan_scene3d_fold(request: &Scene3dFoldRequest) -> Result<serde_json::Value> {
    use serde_json::json;
    anyhow::ensure!(
        request
            .document
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|v| v >= 11),
        "[unsupported-version] package folding needs document format v11 or newer; upgrade the document first"
    );
    anyhow::ensure!(
        request.fold_state.is_finite() && (0.0..=1.0).contains(&request.fold_state),
        "[invalid-input] fold state must be within 0..=1"
    );
    let mut candidate = request.document.clone();
    let dieline = candidate
        .get_mut("dielines")
        .and_then(|dielines| dielines.get_mut(&request.dieline))
        .with_context(|| {
            format!(
                "[missing-resource] dieline `{}` does not exist",
                request.dieline
            )
        })?;
    dieline["fold_state"] = json!(request.fold_state);
    crate::transaction::validate_value(&candidate)?;
    Ok(
        json!({"document":candidate,"result":{"dieline":request.dieline,"fold_state":request.fold_state},"local":true}),
    )
}

async fn scene3d_fold(Json(request): Json<Scene3dFoldRequest>) -> Response {
    match plan_scene3d_fold(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dRenderReportRequest {
    document: serde_json::Value,
    page: String,
    node: String,
}

fn find_scene3d_node<'a>(
    nodes: &'a [serde_json::Value],
    id: &str,
) -> Option<&'a serde_json::Value> {
    for node in nodes {
        if node.get("id").and_then(serde_json::Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(found) = node
            .get("children")
            .and_then(serde_json::Value::as_array)
            .and_then(|children| find_scene3d_node(children, id))
        {
            return Some(found);
        }
    }
    None
}

fn plan_scene3d_render_report(
    request: &Scene3dRenderReportRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    crate::transaction::validate_value(&request.document)?;
    let page = request
        .document
        .get("pages")
        .and_then(serde_json::Value::as_array)
        .and_then(|pages| {
            pages.iter().find(|page| {
                page.get("id").and_then(serde_json::Value::as_str) == Some(&request.page)
            })
        })
        .with_context(|| format!("[missing-resource] page `{}` does not exist", request.page))?;
    let node = page
        .get("layers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|layer| layer.get("nodes").and_then(serde_json::Value::as_array))
        .find_map(|nodes| find_scene3d_node(nodes, &request.node))
        .with_context(|| {
            format!(
                "[missing-resource] node `{}` does not exist on page `{}`",
                request.node, request.page
            )
        })?;
    anyhow::ensure!(
        node.get("kind").and_then(serde_json::Value::as_str) == Some("scene3d"),
        "[invalid-input] node `{}` is not a scene3d node",
        request.node
    );
    let node = node
        .as_object()
        .context("[invalid-input] scene3d node must be an object")?;
    let (_, report) = crate::scene3d::node::render_png_report_in_page(
        &request.document,
        project_root,
        Some(&request.page),
        node,
    )?;
    let revision = crate::transaction::revision(&serde_json::to_vec(&request.document)?);
    Ok(serde_json::json!({
        "node": request.node,
        "page": request.page,
        "document_revision": revision,
        "render": report,
        "local": true
    }))
}

async fn scene3d_render_report(
    State(state): State<Shared>,
    Json(request): Json<Scene3dRenderReportRequest>,
) -> Response {
    match plan_scene3d_render_report(&request, &state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dCreaseAngleRequest {
    document: serde_json::Value,
    dieline: String,
    crease: String,
    angle_deg: f64,
}

fn plan_scene3d_crease_angle(request: &Scene3dCreaseAngleRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::pack::set_crease_angle(
        &mut candidate,
        &request.dieline,
        &request.crease,
        request.angle_deg,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_crease_angle(Json(request): Json<Scene3dCreaseAngleRequest>) -> Response {
    match plan_scene3d_crease_angle(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dCreaseDirectionRequest {
    document: serde_json::Value,
    dieline: String,
    crease: String,
    direction: String,
}

fn plan_scene3d_crease_direction(
    request: &Scene3dCreaseDirectionRequest,
) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::pack::set_crease_direction(
        &mut candidate,
        &request.dieline,
        &request.crease,
        &request.direction,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_crease_direction(Json(request): Json<Scene3dCreaseDirectionRequest>) -> Response {
    match plan_scene3d_crease_direction(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPackSettingsRequest {
    document: serde_json::Value,
    dieline: String,
    #[serde(default)]
    board_mm: Option<f64>,
    #[serde(default)]
    bleed_mm: Option<f64>,
    #[serde(default)]
    seam_angle_deg: Option<f64>,
    #[serde(default)]
    wrap_direction: Option<String>,
}

fn plan_scene3d_pack_settings(request: &Scene3dPackSettingsRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::pack::set_print_settings(
        &mut candidate,
        &request.dieline,
        request.board_mm,
        request.bleed_mm,
        request.seam_angle_deg,
        request.wrap_direction.as_deref(),
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_pack_settings(Json(request): Json<Scene3dPackSettingsRequest>) -> Response {
    match plan_scene3d_pack_settings(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPackPreflightRequest {
    document: serde_json::Value,
    dieline: String,
    #[serde(default = "scene3d_default_min_ppi")]
    min_ppi: f64,
}

fn scene3d_default_min_ppi() -> f64 {
    300.0
}

fn plan_scene3d_pack_preflight(
    request: &Scene3dPackPreflightRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    let report = crate::scene3d::pack::preflight(
        &request.document,
        project_root,
        &request.dieline,
        request.min_ppi,
    )?;
    Ok(serde_json::json!({"result": report, "local": true}))
}

async fn scene3d_pack_preflight(
    State(state): State<Shared>,
    Json(request): Json<Scene3dPackPreflightRequest>,
) -> Response {
    match plan_scene3d_pack_preflight(&request, &state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPackFlatPreviewRequest {
    document: serde_json::Value,
    dieline: String,
    #[serde(default = "scene3d_default_min_ppi")]
    min_ppi: f64,
}

fn plan_scene3d_pack_flat_preview(
    request: &Scene3dPackFlatPreviewRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    const MAX_BROWSER_PREVIEW_BYTES: usize = 16 * 1024 * 1024;
    let preview = crate::scene3d::pack::flat_dieline_svg_with_artwork(
        &request.document,
        project_root,
        &request.dieline,
        request.min_ppi,
    )?;
    anyhow::ensure!(
        preview.svg.len() <= MAX_BROWSER_PREVIEW_BYTES,
        "[limit-exceeded] flat dieline browser preview exceeds 16 MiB; export the SVG with the CLI instead"
    );
    Ok(serde_json::json!({
        "svg": preview.svg,
        "report": {
            "dieline": request.dieline,
            "artwork_panels": preview.artwork_panels,
            "rasterized_panels": preview.rasterized_panels,
            "rasterized_ppi": preview.rasterized_ppi,
            "bleed_synthesized": preview.bleed_synthesized,
            "minimum_ppi": request.min_ppi,
            "warnings": preview.warnings,
            "manufacturing_certified": false
        }
    }))
}

async fn scene3d_pack_flat_preview(
    State(state): State<Shared>,
    Json(request): Json<Scene3dPackFlatPreviewRequest>,
) -> Response {
    match plan_scene3d_pack_flat_preview(&request, &state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPackExportBundleRequest {
    document: serde_json::Value,
    dieline: String,
    #[serde(default = "scene3d_default_min_ppi")]
    min_ppi: f64,
    #[serde(default = "scene3d_default_min_ppi")]
    ppi: f64,
    #[serde(default = "scene3d_default_bundle_fold")]
    fold: f64,
}

fn scene3d_default_bundle_fold() -> f64 {
    1.0
}

fn zip_pack_directory(
    root: &std::path::Path,
    directory: &std::path::Path,
    writer: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>,
    total_bytes: &mut usize,
) -> Result<()> {
    let mut entries = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort();
    for path in entries {
        let metadata = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "[invalid-input] packaging export encountered a symlink"
        );
        if metadata.is_dir() {
            zip_pack_directory(root, &path, writer, total_bytes)?;
            continue;
        }
        anyhow::ensure!(
            metadata.is_file(),
            "[invalid-input] packaging export encountered a non-file artifact"
        );
        *total_bytes = total_bytes
            .checked_add(metadata.len() as usize)
            .context("packaging ZIP size overflow")?;
        anyhow::ensure!(
            *total_bytes <= 512 * 1024 * 1024,
            "[limit-exceeded] packaging print bundle exceeds the 512 MiB download limit"
        );
        let relative = path.strip_prefix(root)?;
        let name = relative.to_string_lossy().replace('\\', "/");
        writer.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )?;
        use std::io::Write as _;
        writer.write_all(&crate::resource::read_bounded(&path, 512 * 1024 * 1024)?)?;
    }
    Ok(())
}

async fn scene3d_pack_export_bundle(
    State(state): State<Shared>,
    Json(request): Json<Scene3dPackExportBundleRequest>,
) -> Response {
    let result = (|| -> Result<Response> {
        crate::scene::validate(&request.document)?;
        let revision = crate::transaction::revision(&serde_json::to_vec(&request.document)?);
        let source = state
            .file
            .clone()
            .unwrap_or_else(|| state.project_root.join("untitled.pen"));
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "pentool-pack-download-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&temp_root)?;
        let bundle_dir = temp_root.join("bundle");
        if let Err(error) = crate::scene3d::cli::write_pack_bundle(
            &request.document,
            &source,
            crate::scene3d::cli::PackBundleOptions {
                dieline_id: &request.dieline,
                out: &bundle_dir,
                min_ppi: request.min_ppi,
                ppi: request.ppi,
                bits: 8,
                fold: request.fold,
                source_revision: &revision,
                dry_run: false,
            },
        ) {
            let _ = std::fs::remove_dir_all(&temp_root);
            return Err(error);
        }
        let mut uncompressed_bytes = 0usize;
        let zipped = (|| -> Result<std::io::Cursor<Vec<u8>>> {
            let verification = crate::scene3d::cli::verify_pack_bundle(&bundle_dir)?;
            anyhow::ensure!(
                verification["status"] == "verified",
                "[integrity-error] generated packaging bundle did not pass verification"
            );
            let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            zip_pack_directory(
                &bundle_dir,
                &bundle_dir,
                &mut archive,
                &mut uncompressed_bytes,
            )?;
            Ok(archive.finish()?)
        })();
        let _ = std::fs::remove_dir_all(&temp_root);
        let bytes = zipped?.into_inner();
        anyhow::ensure!(
            bytes.len() <= 512 * 1024 * 1024,
            "[limit-exceeded] compressed packaging bundle exceeds the 512 MiB download limit"
        );
        let mut response = bytes.into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/zip"),
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"pentool-packaging-bundle.zip\""),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        Ok(response)
    })();
    result.unwrap_or_else(problem)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPackFacePreviewRequest {
    document: serde_json::Value,
    page: String,
    node: String,
    panel: String,
}

fn plan_scene3d_pack_face_preview(
    request: &Scene3dPackFacePreviewRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    use base64::Engine as _;
    const MAX_FACE_PREVIEW_EDGE: f64 = 768.0;
    crate::scene::validate(&request.document)?;
    let page = request.document["pages"]
        .as_array()
        .context("[invalid-input] document pages are missing")?
        .iter()
        .find(|page| page["id"].as_str() == Some(request.page.as_str()))
        .with_context(|| format!("[missing-resource] page `{}` does not exist", request.page))?;
    let source_node = find_scene3d_value(page, &request.node).with_context(|| {
        format!(
            "[missing-resource] scene3d node `{}` does not exist on page `{}`",
            request.node, request.page
        )
    })?;
    let dieline_id = source_node["dieline"].as_str().context(
        "[unsupported-capability] selected-face preview requires a dieline-backed scene3d node",
    )?;
    let dieline = request.document["dielines"]
        .get(dieline_id)
        .with_context(|| format!("[missing-resource] dieline `{dieline_id}` does not exist"))?;
    let panels = dieline["panels"]
        .as_array()
        .context("[invalid-input] dieline panels are missing")?;
    anyhow::ensure!(
        panels
            .iter()
            .any(|panel| panel["id"].as_str() == Some(request.panel.as_str())),
        "[missing-resource] panel `{}` does not exist in dieline `{dieline_id}`",
        request.panel
    );
    let model = crate::scene3d::pack::generate(dieline)?;
    let selections = model
        .primitives
        .iter()
        .enumerate()
        .filter_map(|(index, primitive)| {
            let name = primitive.name.as_str();
            (name == request.panel || name.starts_with(&format!("{}-overlap-", request.panel)))
                .then(|| (index, (0..primitive.triangle_count()).collect::<Vec<_>>()))
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !selections.is_empty(),
        "[missing-resource] panel `{}` has no folded faces to highlight",
        request.panel
    );
    let mut preview_node = source_node
        .as_object()
        .context("[invalid-input] scene3d node must be an object")?
        .clone();
    let width = preview_node["width"]
        .as_f64()
        .context("[invalid-input] scene3d node width is missing")?;
    let height = preview_node["height"]
        .as_f64()
        .context("[invalid-input] scene3d node height is missing")?;
    anyhow::ensure!(
        width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0,
        "[invalid-input] scene3d node dimensions must be finite and positive"
    );
    let scale = MAX_FACE_PREVIEW_EDGE / width.max(height);
    preview_node.insert("width".into(), serde_json::json!(width * scale));
    preview_node.insert("height".into(), serde_json::json!(height * scale));
    preview_node.insert("pixel_ratio".into(), serde_json::json!(1.0));
    preview_node.insert("supersample".into(), serde_json::json!(1));
    let png = crate::scene3d::node::render_png_with_primitive_selections_in_page(
        &request.document,
        project_root,
        Some(&request.page),
        &preview_node,
        selections,
    )?;
    anyhow::ensure!(
        png.len() <= 16 * 1024 * 1024,
        "[limit-exceeded] selected-face preview exceeds 16 MiB"
    );
    Ok(serde_json::json!({
        "node":request.node,"page":request.page,"panel":request.panel,
        "png_base64":base64::engine::general_purpose::STANDARD.encode(png),
        "max_edge_px":MAX_FACE_PREVIEW_EDGE as u32
    }))
}
async fn scene3d_pack_face_preview(
    State(state): State<Shared>,
    Json(request): Json<Scene3dPackFacePreviewRequest>,
) -> Response {
    match plan_scene3d_pack_face_preview(&request, &state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dSleeveRequest {
    document: serde_json::Value,
    dieline_id: String,
    node_id: String,
    layer: String,
    #[serde(default)]
    page: Option<String>,
    length_mm: f64,
    width_mm: f64,
    height_mm: f64,
    #[serde(default)]
    board_mm: f64,
    #[serde(default)]
    bleed_mm: f64,
    placement: [f64; 4],
}

fn plan_scene3d_sleeve(request: &Scene3dSleeveRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::pack::add_sleeve(
        &mut candidate,
        &request.dieline_id,
        &request.node_id,
        &request.layer,
        request.page.as_deref(),
        request.length_mm,
        request.width_mm,
        request.height_mm,
        request.board_mm,
        request.bleed_mm,
        request.placement,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_sleeve(Json(request): Json<Scene3dSleeveRequest>) -> Response {
    match plan_scene3d_sleeve(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dCartonRequest {
    document: serde_json::Value,
    dieline_id: String,
    node_id: String,
    layer: String,
    #[serde(default)]
    page: Option<String>,
    template: String,
    length_mm: f64,
    width_mm: f64,
    height_mm: f64,
    #[serde(default = "scene3d_default_board")]
    board_mm: f64,
    #[serde(default = "scene3d_default_bleed")]
    bleed_mm: f64,
    #[serde(default = "scene3d_default_glue_flap")]
    glue_flap_mm: f64,
    placement: [f64; 4],
}

fn plan_scene3d_carton(request: &Scene3dCartonRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::pack::add_tuck_carton(
        &mut candidate,
        &request.dieline_id,
        &request.node_id,
        &request.layer,
        request.page.as_deref(),
        &request.template,
        request.length_mm,
        request.width_mm,
        request.height_mm,
        request.board_mm,
        request.bleed_mm,
        request.glue_flap_mm,
        request.placement,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_carton(Json(request): Json<Scene3dCartonRequest>) -> Response {
    match plan_scene3d_carton(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dTubeLabelRequest {
    document: serde_json::Value,
    dieline_id: String,
    node_id: String,
    layer: String,
    #[serde(default)]
    page: Option<String>,
    radius_mm: f64,
    height_mm: f64,
    #[serde(default = "scene3d_default_bleed")]
    bleed_mm: f64,
    #[serde(default)]
    overlap_mm: f64,
    placement: [f64; 4],
}

fn plan_scene3d_tube_label(request: &Scene3dTubeLabelRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::pack::add_tube_label_with_overlap(
        &mut candidate,
        &request.dieline_id,
        &request.node_id,
        &request.layer,
        request.page.as_deref(),
        request.radius_mm,
        request.height_mm,
        request.bleed_mm,
        request.overlap_mm,
        request.placement,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_tube_label(Json(request): Json<Scene3dTubeLabelRequest>) -> Response {
    match plan_scene3d_tube_label(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPouchRequest {
    document: serde_json::Value,
    dieline_id: String,
    node_id: String,
    layer: String,
    #[serde(default)]
    page: Option<String>,
    front_width_mm: f64,
    gusset_mm: f64,
    height_mm: f64,
    #[serde(default = "scene3d_default_bleed")]
    bleed_mm: f64,
    placement: [f64; 4],
}

fn plan_scene3d_pouch(request: &Scene3dPouchRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::pack::add_pouch(
        &mut candidate,
        &request.dieline_id,
        &request.node_id,
        &request.layer,
        request.page.as_deref(),
        request.front_width_mm,
        request.gusset_mm,
        request.height_mm,
        request.bleed_mm,
        request.placement,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_pouch(Json(request): Json<Scene3dPouchRequest>) -> Response {
    match plan_scene3d_pouch(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dBottleLabelRequest {
    document: serde_json::Value,
    dieline_id: String,
    node_id: String,
    layer: String,
    #[serde(default)]
    page: Option<String>,
    radius_mm: f64,
    height_mm: f64,
    #[serde(default = "scene3d_default_bleed")]
    bleed_mm: f64,
    #[serde(default)]
    overlap_mm: f64,
    placement: [f64; 4],
}

fn plan_scene3d_bottle_label(request: &Scene3dBottleLabelRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::pack::add_bottle_label(
        &mut candidate,
        &request.dieline_id,
        &request.node_id,
        &request.layer,
        request.page.as_deref(),
        request.radius_mm,
        request.height_mm,
        request.bleed_mm,
        request.overlap_mm,
        request.placement,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_bottle_label(Json(request): Json<Scene3dBottleLabelRequest>) -> Response {
    match plan_scene3d_bottle_label(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

fn scene3d_default_board() -> f64 {
    0.5
}

fn scene3d_default_bleed() -> f64 {
    3.0
}

fn scene3d_default_glue_flap() -> f64 {
    15.0
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPanelArtRequest {
    document: serde_json::Value,
    dieline: String,
    panel: String,
    #[serde(default)]
    source: Option<serde_json::Value>,
}

fn plan_scene3d_panel_art(request: &Scene3dPanelArtRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::bindings::set_panel_artwork(
        &mut candidate,
        &request.dieline,
        &request.panel,
        request.source.as_ref(),
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_panel_art(Json(request): Json<Scene3dPanelArtRequest>) -> Response {
    match plan_scene3d_panel_art(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPanelArtTransformRequest {
    document: serde_json::Value,
    dieline: String,
    panel: String,
    scale: [f64; 2],
    offset: [f64; 2],
    rotate: f64,
}

fn plan_scene3d_panel_art_transform(
    request: &Scene3dPanelArtTransformRequest,
) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::bindings::set_panel_artwork_transform(
        &mut candidate,
        &request.dieline,
        &request.panel,
        request.scale,
        request.offset,
        request.rotate,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_panel_art_transform(
    Json(request): Json<Scene3dPanelArtTransformRequest>,
) -> Response {
    match plan_scene3d_panel_art_transform(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPanelSafeZoneRequest {
    document: serde_json::Value,
    dieline: String,
    panel: String,
    inset_mm: f64,
}

fn plan_scene3d_panel_safe_zone(
    request: &Scene3dPanelSafeZoneRequest,
) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::bindings::set_panel_safe_zone(
        &mut candidate,
        &request.dieline,
        &request.panel,
        request.inset_mm,
    )?;
    crate::transaction::validate_value(&candidate)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_panel_safe_zone(Json(request): Json<Scene3dPanelSafeZoneRequest>) -> Response {
    match plan_scene3d_panel_safe_zone(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPathRoleRequest {
    document: serde_json::Value,
    dieline: String,
    node: String,
    role: String,
}

fn plan_scene3d_path_role(request: &Scene3dPathRoleRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::bindings::set_layout_path_role(
        &mut candidate,
        &request.dieline,
        &request.node,
        &request.role,
    )?;
    crate::transaction::validate_value(&candidate)?;
    let resolved = crate::scene3d::pack::dieline_from_document(&candidate, &request.dieline)?;
    crate::scene3d::pack::flat_dieline_svg(&resolved)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_path_role(Json(request): Json<Scene3dPathRoleRequest>) -> Response {
    match plan_scene3d_path_role(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene3dPathCreaseRequest {
    document: serde_json::Value,
    dieline: String,
    node: String,
    crease: String,
    angle_deg: f64,
    direction: String,
}

fn plan_scene3d_path_crease(request: &Scene3dPathCreaseRequest) -> Result<serde_json::Value> {
    let mut candidate = request.document.clone();
    let result = crate::scene3d::bindings::set_layout_path_crease(
        &mut candidate,
        &request.dieline,
        &request.node,
        &request.crease,
        request.angle_deg,
        &request.direction,
    )?;
    crate::transaction::validate_value(&candidate)?;
    let resolved = crate::scene3d::pack::dieline_from_document(&candidate, &request.dieline)?;
    crate::scene3d::pack::flat_dieline_svg(&resolved)?;
    crate::scene3d::pack::generate(&resolved)?;
    Ok(serde_json::json!({"document":candidate,"result":result,"local":true}))
}

async fn scene3d_path_crease(Json(request): Json<Scene3dPathCreaseRequest>) -> Response {
    match plan_scene3d_path_crease(&request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

fn scene3d_default_extent() -> f64 {
    400.0
}

fn plan_scene3d_import(
    raw: &mut serde_json::Value,
    body: &Scene3dImportRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    use base64::Engine as _;
    let format = body.format.to_ascii_lowercase();
    if !matches!(format.as_str(), "glb" | "gltf" | "obj") {
        bail!(
            "[unsupported-capability] model format `{format}` is unsupported; use .glb, .gltf or .obj"
        );
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body.data)
        .context("[malformed-resource] model data is not valid base64")?;
    if bytes.len() > crate::scene3d::LIMITS.max_bytes {
        bail!(
            "[limit-exceeded] model is larger than {} bytes",
            crate::scene3d::LIMITS.max_bytes
        );
    }
    let temp_dir = std::env::temp_dir().join(format!(
        "pentool-scene3d-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&temp_dir)?;
    let model_path = temp_dir.join(format!("model.{format}"));
    let mut candidate = raw.clone();
    let result = (|| {
        std::fs::write(&model_path, &bytes)?;
        crate::scene3d::cli::add(
            &mut candidate,
            &project_root.join("browser.pen"),
            (
                &body.id,
                &model_path,
                &body.layer,
                body.page.as_deref(),
                [body.x, body.y, body.width, body.height],
                false,
                Some(&format),
            ),
        )
    })();
    let _ = std::fs::remove_file(&model_path);
    let _ = std::fs::remove_dir(&temp_dir);
    let value = result?;
    crate::transaction::validate_value(&candidate)?;
    *raw = candidate;
    Ok(value)
}

async fn scene3d_import(
    State(state): State<Shared>,
    Json(mut body): Json<Scene3dImportRequest>,
) -> Response {
    let request = body.clone();
    let result = plan_scene3d_import(&mut body.document, &request, &state.project_root);
    match result {
        Ok(value) => Json(serde_json::json!({
            "document": body.document,
            "result": value,
            "local": true
        }))
        .into_response(),
        Err(error) => problem(error),
    }
}

async fn scene3d_status() -> Response {
    let status = crate::scene3d::gpu::availability();
    Json(serde_json::json!({
        "active": "reference",
        "accelerated_available": status.available,
        "adapter": status.adapter,
        "backend": status.backend,
        "fallback_reason": status.reason,
        "render_cache": crate::scene3d::node::png_cache_status(),
    }))
    .into_response()
}

async fn scene3d_cache_status() -> Response {
    Json(crate::scene3d::node::png_cache_status()).into_response()
}

async fn scene3d_cache_clear() -> Response {
    Json(crate::scene3d::node::clear_png_cache()).into_response()
}

#[derive(serde::Deserialize)]
struct Scene3dUvPreviewRequest {
    document: serde_json::Value,
    node: String,
    primitive: usize,
    #[serde(default = "scene3d_uv_default_size")]
    size: u32,
    #[serde(default = "scene3d_uv_default_texture_resolution")]
    texture_resolution: u32,
    minimum_density: Option<f64>,
}

#[derive(serde::Deserialize)]
struct Scene3dUvModelPreviewRequest {
    document: serde_json::Value,
    page: String,
    node: String,
    primitive: usize,
    island: Option<String>,
}

#[derive(serde::Deserialize)]
struct Scene3dUvFacePickRequest {
    document: serde_json::Value,
    page: String,
    node: String,
    #[serde(default)]
    primitive: Option<usize>,
    /// Normalized coordinates in the rendered preview, each in [0, 1].
    x: f64,
    y: f64,
}

#[derive(serde::Deserialize)]
struct Scene3dUvPackRequest {
    document: serde_json::Value,
    page: String,
    node: String,
    primitive: usize,
    #[serde(default = "scene3d_uv_default_padding")]
    padding: f64,
}

fn scene3d_uv_default_padding() -> f64 {
    0.01
}

fn scene3d_uv_default_size() -> u32 {
    1024
}

fn scene3d_uv_default_texture_resolution() -> u32 {
    2048
}

fn find_scene3d_value<'a>(value: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("scene3d")
        && value.get("id").and_then(serde_json::Value::as_str) == Some(id)
    {
        return Some(value);
    }
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| find_scene3d_value(value, id)),
        serde_json::Value::Object(values) => values
            .values()
            .find_map(|value| find_scene3d_value(value, id)),
        _ => None,
    }
}

fn plan_scene3d_uv_preview(
    request: &Scene3dUvPreviewRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    use base64::Engine as _;
    crate::scene::validate(&request.document)?;
    let node = find_scene3d_value(&request.document, &request.node).with_context(|| {
        format!(
            "[missing-resource] scene3d node `{}` does not exist",
            request.node
        )
    })?;
    let (model, overlays, source) = if let Some(digest) =
        node.get("model").and_then(serde_json::Value::as_str)
    {
        (
            crate::scene3d::node::load_model(&request.document, project_root, digest)?,
            request.document["models"][digest]
                .get("uv")
                .unwrap_or(&serde_json::Value::Null),
            serde_json::json!({"kind":"model","id":digest}),
        )
    } else if let Some(form_id) = node.get("product_form").and_then(serde_json::Value::as_str) {
        (
            crate::scene3d::node::load_form(&request.document, form_id)?,
            request.document["product_forms"][form_id]
                .get("uv")
                .unwrap_or(&serde_json::Value::Null),
            serde_json::json!({"kind":"product_form","id":form_id}),
        )
    } else {
        anyhow::bail!(
            "[unsupported-capability] UV workspace supports imported models and parametric product forms"
        )
    };
    let primitive = model.primitives.get(request.primitive).with_context(|| {
        format!(
            "[missing-resource] primitive {} does not exist (model has {})",
            request.primitive,
            model.primitives.len()
        )
    })?;
    let material_slot = primitive
        .material
        .and_then(|index| model.materials.get(index))
        .map(|material| material.name.as_str());
    let uv = crate::scene3d::uv::show_with_overlays_at_resolution(
        &model,
        overlays,
        request.texture_resolution,
        request.minimum_density,
    )?;
    let svg = crate::scene3d::uv::layout_svg(primitive, request.size)?;
    let png = crate::scene3d::uv::layout_png(primitive, request.size)?;
    Ok(serde_json::json!({
        "node": request.node,
        "source": source,
        "primitive": request.primitive,
        "material_slot": material_slot,
        "svg": svg,
        "png_base64": base64::engine::general_purpose::STANDARD.encode(png),
        "report": uv,
    }))
}

async fn scene3d_uv_preview(
    State(state): State<Shared>,
    Json(request): Json<Scene3dUvPreviewRequest>,
) -> Response {
    match plan_scene3d_uv_preview(&request, &state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

fn plan_scene3d_uv_model_preview(
    request: &Scene3dUvModelPreviewRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    use base64::Engine as _;
    anyhow::ensure!(
        request
            .document
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|v| v >= 11),
        "[unsupported-version] UV model preview requires document format v11 or newer"
    );
    crate::scene::validate(&request.document)?;
    let page = request.document["pages"]
        .as_array()
        .context("[invalid-input] document pages are missing")?
        .iter()
        .find(|page| page["id"].as_str() == Some(request.page.as_str()))
        .with_context(|| format!("[missing-resource] page `{}` does not exist", request.page))?;
    let source_node = find_scene3d_value(page, &request.node).with_context(|| {
        format!(
            "[missing-resource] scene3d node `{}` does not exist on page `{}`",
            request.node, request.page
        )
    })?;
    let (model, source_record) = if let Some(digest) =
        source_node.get("model").and_then(serde_json::Value::as_str)
    {
        (
            crate::scene3d::node::load_model(&request.document, project_root, digest)?,
            request.document["models"].get(digest).with_context(|| {
                format!("[missing-resource] model record `{digest}` does not exist")
            })?,
        )
    } else if let Some(form_id) = source_node
        .get("product_form")
        .and_then(serde_json::Value::as_str)
    {
        (
            crate::scene3d::node::load_form(&request.document, form_id)?,
            request.document["product_forms"]
                .get(form_id)
                .with_context(|| {
                    format!("[missing-resource] product form `{form_id}` does not exist")
                })?,
        )
    } else {
        anyhow::bail!(
            "[unsupported-capability] UV model preview supports imported models and parametric product forms"
        )
    };
    let primitive = model.primitives.get(request.primitive).with_context(|| {
        format!(
            "[missing-resource] primitive {} does not exist (model has {})",
            request.primitive,
            model.primitives.len()
        )
    })?;
    let triangles = if let Some(island_id) = request.island.as_deref() {
        let key = request.primitive.to_string();
        let empty_uv = serde_json::Value::Null;
        let uv_overlays = source_record.get("uv").unwrap_or(&empty_uv);
        let overlay = uv_overlays
            .get(&key)
            .or_else(|| uv_overlays.get("*"))
            .with_context(|| {
                format!(
                    "[missing-resource] primitive {} has no UV overlay for island `{island_id}`",
                    request.primitive
                )
            })?;
        let island = overlay["islands"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|island| island["id"].as_str() == Some(island_id))
            .with_context(|| {
                format!(
                    "[missing-resource] UV island `{island_id}` does not exist on primitive {}",
                    request.primitive
                )
            })?;
        Some(
            island["triangles"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|triangle| {
                    triangle.as_u64().map(|triangle| triangle as usize).context(
                        "[invalid-input] UV island triangle indices must be non-negative integers",
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )
    } else {
        None
    };
    if let Some(triangles) = &triangles {
        anyhow::ensure!(
            !triangles.is_empty(),
            "[missing-resource] selected UV island has no triangles"
        );
        anyhow::ensure!(
            triangles
                .iter()
                .all(|triangle| *triangle < primitive.triangle_count()),
            "[invalid-input] selected UV island references a missing primitive triangle"
        );
    }
    let mut preview_node = source_node
        .as_object()
        .context("[invalid-input] scene3d node must be an object")?
        .clone();
    let width = preview_node["width"]
        .as_f64()
        .context("[invalid-input] scene3d node width is missing")?;
    let height = preview_node["height"]
        .as_f64()
        .context("[invalid-input] scene3d node height is missing")?;
    let scale = 512.0 / width.max(height);
    preview_node.insert("width".into(), serde_json::json!(width * scale));
    preview_node.insert("height".into(), serde_json::json!(height * scale));
    preview_node.insert("pixel_ratio".into(), serde_json::json!(1.0));
    preview_node.insert("supersample".into(), serde_json::json!(1));
    let png = match triangles {
        Some(triangles) => crate::scene3d::node::render_png_with_island_selection_in_page(
            &request.document,
            project_root,
            Some(&request.page),
            &preview_node,
            request.primitive,
            triangles,
        )?,
        None => crate::scene3d::node::render_png_in_page(
            &request.document,
            project_root,
            Some(&request.page),
            &preview_node,
        )?,
    };
    Ok(serde_json::json!({
        "node": request.node,
        "page": request.page,
        "primitive": request.primitive,
        "island": request.island,
        "png_base64": base64::engine::general_purpose::STANDARD.encode(png),
        "max_edge_px": 512,
    }))
}

fn plan_scene3d_uv_face_pick(
    request: &Scene3dUvFacePickRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    anyhow::ensure!(
        request.x.is_finite()
            && request.y.is_finite()
            && (0.0..=1.0).contains(&request.x)
            && (0.0..=1.0).contains(&request.y),
        "[invalid-argument] face-pick coordinates must be normalized to [0, 1]"
    );
    crate::scene::validate(&request.document)?;
    let page = request.document["pages"]
        .as_array()
        .context("[invalid-input] document pages are missing")?
        .iter()
        .find(|page| page["id"].as_str() == Some(request.page.as_str()))
        .with_context(|| format!("[missing-resource] page `{}` does not exist", request.page))?;
    let node = find_scene3d_value(page, &request.node).with_context(|| {
        format!(
            "[missing-resource] scene3d node `{}` does not exist on page `{}`",
            request.node, request.page
        )
    })?;
    let digest = node.get("model").and_then(serde_json::Value::as_str);
    let model = if let Some(digest) = digest {
        let loaded = crate::scene3d::node::load_model(&request.document, project_root, digest)?;
        let mut model = loaded.as_ref().clone();
        crate::scene3d::uv::apply(
            &mut model,
            request.document["models"][digest]
                .get("uv")
                .unwrap_or(&serde_json::Value::Null),
        )?;
        model
    } else if let Some(form_id) = node.get("product_form").and_then(serde_json::Value::as_str) {
        crate::scene3d::node::load_form(&request.document, form_id)?
            .as_ref()
            .clone()
    } else if let Some(dieline_id) = node.get("dieline").and_then(serde_json::Value::as_str) {
        let dieline = request.document["dielines"]
            .get(dieline_id)
            .with_context(|| format!("[missing-resource] dieline `{dieline_id}` is missing"))?;
        crate::scene3d::pack::generate(dieline)?
    } else {
        anyhow::bail!(
            "[unsupported-capability] face picking supports imported models, product forms, and packaging dielines"
        )
    };
    if let Some(primitive) = request.primitive {
        model
            .primitives
            .get(primitive)
            .with_context(|| format!("[missing-resource] primitive {primitive} does not exist"))?;
    }
    let node_map = node
        .as_object()
        .context("[invalid-input] scene3d node must be an object")?;
    let (scene, _) = crate::scene3d::node::scene_for(node_map, Some(&model))?;
    let aspect = node["width"]
        .as_f64()
        .context("[invalid-input] scene3d node width is missing")?
        / node["height"]
            .as_f64()
            .context("[invalid-input] scene3d node height is missing")?;
    let camera = &scene.camera;
    let forward =
        crate::scene3d::math::normalize(crate::scene3d::math::sub(camera.target, camera.eye));
    let right = crate::scene3d::math::normalize(crate::scene3d::math::cross(forward, camera.up));
    let up = crate::scene3d::math::cross(right, forward);
    let nx = request.x * 2.0 - 1.0;
    let ny = 1.0 - request.y * 2.0;
    let (origin, direction) = match camera.projection {
        crate::scene3d::render::Projection::Perspective { fov_y_deg } => {
            let t = crate::scene3d::math::tan(fov_y_deg * crate::scene3d::math::PI / 360.0);
            (
                camera.eye,
                crate::scene3d::math::normalize(crate::scene3d::math::add(
                    forward,
                    crate::scene3d::math::add(
                        crate::scene3d::math::scale(right, nx * t * aspect),
                        crate::scene3d::math::scale(up, ny * t),
                    ),
                )),
            )
        }
        crate::scene3d::render::Projection::Orthographic { height } => (
            crate::scene3d::math::add(
                camera.eye,
                crate::scene3d::math::add(
                    crate::scene3d::math::scale(right, nx * height * aspect * 0.5),
                    crate::scene3d::math::scale(up, ny * height * 0.5),
                ),
            ),
            forward,
        ),
    };
    type FaceHit = (f64, usize, usize, Option<String>, Option<[f64; 2]>);
    let mut nearest: Option<FaceHit> = None;
    for instance in &model.instances {
        let primitive = &model.primitives[instance.primitive];
        for (triangle, indices) in primitive.indices.chunks_exact(3).enumerate() {
            let vertices = [indices[0], indices[1], indices[2]].map(|index| {
                crate::scene3d::math::transform_point(&instance.transform, {
                    let p = primitive.positions[index as usize];
                    [p[0] as f64, p[1] as f64, p[2] as f64]
                })
            });
            let edge1 = crate::scene3d::math::sub(vertices[1], vertices[0]);
            let edge2 = crate::scene3d::math::sub(vertices[2], vertices[0]);
            let p = crate::scene3d::math::cross(direction, edge2);
            let det = crate::scene3d::math::dot(edge1, p);
            if det.abs() < 1e-12 {
                continue;
            }
            let material = primitive
                .material
                .and_then(|index| model.materials.get(index));
            if det < 0.0 && !material.is_some_and(|material| material.double_sided) {
                continue;
            }
            let inv = 1.0 / det;
            let tvec = crate::scene3d::math::sub(origin, vertices[0]);
            let u = crate::scene3d::math::dot(tvec, p) * inv;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q = crate::scene3d::math::cross(tvec, edge1);
            let v = crate::scene3d::math::dot(direction, q) * inv;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let distance = crate::scene3d::math::dot(edge2, q) * inv;
            if distance > 0.0
                && nearest
                    .as_ref()
                    .is_none_or(|(best, _, _, _, _)| distance < *best)
            {
                let w = 1.0 - u - v;
                let uv = primitive.uvs.first().map(|set| {
                    let uv0 = set[indices[0] as usize];
                    let uv1 = set[indices[1] as usize];
                    let uv2 = set[indices[2] as usize];
                    [
                        f64::from(uv0[0]) * w + f64::from(uv1[0]) * u + f64::from(uv2[0]) * v,
                        f64::from(uv0[1]) * w + f64::from(uv1[1]) * u + f64::from(uv2[1]) * v,
                    ]
                });
                let slot = material.map(|material| material.name.clone());
                nearest = Some((distance, instance.primitive, triangle, slot, uv));
            }
        }
    }
    // A face on another primitive in front of the selected one must occlude it.
    let hit = nearest.filter(|(_, primitive, _, _, _)| {
        request
            .primitive
            .is_none_or(|expected| *primitive == expected)
    });
    let picked_primitive = hit.as_ref().map(|(_, primitive, _, _, _)| *primitive);
    let triangle = hit.as_ref().map(|(_, _, triangle, _, _)| *triangle);
    let material_slot = hit.as_ref().and_then(|(_, _, _, slot, _)| slot.clone());
    let uv = hit.as_ref().and_then(|(_, _, _, _, uv)| *uv);
    let island = digest.and_then(|digest| {
        let triangle = triangle?;
        let primitive = picked_primitive?;
        request.document["models"][digest]["uv"]
            .as_object()?
            .iter()
            .filter(|(key, _)| key.as_str() == "*" || key.parse::<usize>().ok() == Some(primitive))
            .flat_map(|(_, overlay)| overlay["islands"].as_array().into_iter().flatten())
            .find(|island| {
                island["triangles"].as_array().is_some_and(|triangles| {
                    triangles
                        .iter()
                        .any(|value| value.as_u64() == Some(triangle as u64))
                })
            })
            .and_then(|island| island["id"].as_str())
            .map(str::to_owned)
    });
    let panel = node
        .get("dieline")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| request.document["dielines"].get(id))
        .and_then(|dieline| dieline["panels"].as_array())
        .and_then(|panels| {
            picked_primitive.and_then(|primitive| {
                let name = model.primitives[primitive].name.as_str();
                panels.iter().find_map(|panel| {
                    let id = panel["id"].as_str()?;
                    (name == id || name.starts_with(&format!("{id}-overlap-")))
                        .then(|| id.to_owned())
                })
            })
        });
    Ok(serde_json::json!({
        "node": request.node,
        "page": request.page,
        "primitive": picked_primitive,
        "panel": panel,
        "triangle": triangle,
        "island": island,
        "material_slot": material_slot,
        "uv": uv
    }))
}

async fn scene3d_uv_face_pick(
    State(state): State<Shared>,
    Json(request): Json<Scene3dUvFacePickRequest>,
) -> Response {
    match plan_scene3d_uv_face_pick(&request, &state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

async fn scene3d_uv_model_preview(
    State(state): State<Shared>,
    Json(request): Json<Scene3dUvModelPreviewRequest>,
) -> Response {
    match plan_scene3d_uv_model_preview(&request, &state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

fn plan_scene3d_uv_pack(
    request: &Scene3dUvPackRequest,
    project_root: &std::path::Path,
) -> Result<serde_json::Value> {
    anyhow::ensure!(
        request
            .document
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|v| v >= 11),
        "[unsupported-version] editable UV island packing needs document format v11 or newer"
    );
    crate::scene::validate(&request.document)?;
    let page = request.document["pages"]
        .as_array()
        .context("[invalid-input] document pages are missing")?
        .iter()
        .find(|page| page["id"].as_str() == Some(request.page.as_str()))
        .with_context(|| format!("[missing-resource] page `{}` does not exist", request.page))?;
    let node = find_scene3d_value(page, &request.node).with_context(|| {
        format!(
            "[missing-resource] scene3d node `{}` does not exist on page `{}`",
            request.node, request.page
        )
    })?;
    let (model, source_kind, source_id) = if let Some(digest) =
        node.get("model").and_then(serde_json::Value::as_str)
    {
        (
            crate::scene3d::node::load_model(&request.document, project_root, digest)?,
            "models",
            digest.to_owned(),
        )
    } else if let Some(form_id) = node.get("product_form").and_then(serde_json::Value::as_str) {
        (
            crate::scene3d::node::load_form(&request.document, form_id)?,
            "product_forms",
            form_id.to_owned(),
        )
    } else {
        anyhow::bail!(
            "[unsupported-capability] UV packing supports imported models and parametric product forms"
        )
    };
    let primitive = model.primitives.get(request.primitive).with_context(|| {
        format!(
            "[missing-resource] primitive {} does not exist (model has {})",
            request.primitive,
            model.primitives.len()
        )
    })?;
    let key = request.primitive.to_string();
    let overlays = &request.document[source_kind][&source_id]["uv"];
    let current_overlay = overlays
        .get(&key)
        .or_else(|| overlays.get("*"))
        .cloned()
        .context("[missing-resource] primitive has no editable UV islands to pack")?;
    let packed_overlay =
        crate::scene3d::uv::pack_islands(primitive, &current_overlay, request.padding)?;
    let mut candidate = request.document.clone();
    candidate[source_kind][&source_id]["uv"][&key] = packed_overlay;
    crate::scene::validate(&candidate)?;
    let packed_model = if source_kind == "models" {
        crate::scene3d::node::load_model(&candidate, project_root, &source_id)?
    } else {
        crate::scene3d::node::load_form(&candidate, &source_id)?
    };
    let report = crate::scene3d::uv::show_with_overlays_at_resolution(
        &packed_model,
        &candidate[source_kind][&source_id]["uv"],
        2048,
        None,
    )?;
    Ok(serde_json::json!({
        "document":candidate,
        "result":{"node":request.node,"page":request.page,"primitive":request.primitive,
            "padding":request.padding,"islands":candidate[source_kind][&source_id]["uv"][&key]["islands"].as_array().map_or(0, Vec::len),
            "report":report["primitives"][request.primitive]},
    }))
}

async fn scene3d_uv_pack(
    State(state): State<Shared>,
    Json(request): Json<Scene3dUvPackRequest>,
) -> Response {
    match plan_scene3d_uv_pack(&request, &state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
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

#[derive(serde::Deserialize)]
struct LayoutQuery {
    bleed: Option<f64>,
    max_overrides: Option<f64>,
    no_alt: Option<bool>,
    format: Option<String>,
    fixed: Option<bool>,
}

fn layout_document(state: &Shared) -> Result<(serde_json::Value, std::path::PathBuf)> {
    let file = state
        .file
        .clone()
        .ok_or_else(|| anyhow::anyhow!("[missing-resource] no shared document"))?;
    let raw = serde_json::from_slice(&crate::resource::read_bounded(
        &file,
        crate::color::catalog::MAX_CATALOG_DOCUMENT_BYTES,
    )?)?;
    Ok((raw, file))
}

/// Read-only composer-measured layout preflight of the shared document.
async fn layout_preflight(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<LayoutQuery>,
) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let (raw, file) = layout_document(&state)?;
        let defaults = crate::layout_preflight::Options::default();
        crate::layout_preflight::run(
            &raw,
            &file,
            &crate::layout_preflight::Options {
                bleed: q.bleed.unwrap_or(defaults.bleed),
                max_override_pct: q.max_overrides.unwrap_or(defaults.max_override_pct),
                require_alt: !q.no_alt.unwrap_or(false),
            },
        )
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

/// Stories and frames with composed overset state.
async fn layout_overview(State(state): State<Shared>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let (raw, _) = layout_document(&state)?;
        crate::layout_preflight::overview(&raw)
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

/// Tagged PDF or EPUB 3 of the shared document, returned as a download.
async fn layout_export(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<LayoutQuery>,
) -> Response {
    let document = layout_document(&state);
    layout_export_bytes(document, q)
}

#[derive(serde::Deserialize)]
struct LayoutExportRequest {
    document: serde_json::Value,
    format: Option<String>,
    fixed: Option<bool>,
}

/// Same export, but of the document open in the browser (unsaved edits included).
async fn layout_export_document(
    State(state): State<Shared>,
    Json(body): Json<LayoutExportRequest>,
) -> Response {
    let file = state
        .file
        .clone()
        .unwrap_or_else(|| state.project_root.join("browser.pen"));
    layout_export_bytes(
        Ok((body.document, file)),
        LayoutQuery {
            bleed: None,
            max_overrides: None,
            no_alt: None,
            format: body.format,
            fixed: body.fixed,
        },
    )
}

fn layout_export_bytes(
    document: Result<(serde_json::Value, std::path::PathBuf)>,
    q: LayoutQuery,
) -> Response {
    let result = (|| -> Result<(Vec<u8>, &'static str, &'static str)> {
        let (raw, file) = document?;
        let format = q.format.as_deref().unwrap_or("pdf");
        let dir = std::env::temp_dir().join(format!("pentool-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let out = dir.join(format!("export-{}.{format}", rand_suffix()));
        let built = match format {
            "pdf" => crate::publish::export_pdf(&raw, &file, &out, &Default::default())
                .map(|_| ("application/pdf", "pdf")),
            "epub" => crate::publish::export_epub(
                &[(&raw, file.as_path())],
                &out,
                &crate::publish::EpubOptions {
                    fixed: q.fixed.unwrap_or(false),
                    ..Default::default()
                },
            )
            .map(|_| ("application/epub+zip", "epub")),
            other => Err(anyhow::anyhow!(
                "[invalid-input] format must be pdf or epub, not {other}"
            )),
        };
        let bytes = built.and_then(|meta| Ok((std::fs::read(&out)?, meta)));
        let _ = std::fs::remove_file(&out);
        let (bytes, (mime, ext)) = bytes?;
        Ok((bytes, mime, ext))
    })();
    match result {
        Ok((bytes, mime, ext)) => (
            [
                (axum::http::header::CONTENT_TYPE, mime.to_string()),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"layout.{ext}\""),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(error) => problem(error),
    }
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::SeqCst)
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
// ---- Design systems (`/api/design/*`) -------------------------------------
// Every route builds the same `DesignCommand` the CLI parses and runs it through
// `design::cli::run`, so Studio and the CLI share one implementation.

// DesignError is a structured error built only on failure paths (see src/design/mod.rs).
type DesignResult<T> = std::result::Result<T, crate::tokens::DesignError>;

fn design_problem(error: crate::tokens::DesignError) -> Response {
    (StatusCode::BAD_REQUEST, Json(error.to_json())).into_response()
}

#[allow(clippy::result_large_err)]
fn design_invalid(message: impl Into<String>) -> crate::tokens::DesignError {
    crate::tokens::DesignError::new("invalid-input", message)
}

/// A project-relative path: no absolute paths, drive prefixes or `..`.
#[allow(clippy::result_large_err)]
fn project_relative(root: &std::path::Path, rel: &str) -> DesignResult<PathBuf> {
    use std::path::Component;
    let path = std::path::Path::new(rel);
    let safe = !rel.is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !safe {
        return Err(design_invalid(format!(
            "path {rel:?} must be relative to the served project and stay inside it"
        ))
        .action("use a plain relative path without `..`"));
    }
    Ok(root.join(path))
}

#[allow(clippy::result_large_err)]
async fn design_read(
    state: Shared,
    build: impl FnOnce(PathBuf, &std::path::Path) -> DesignResult<crate::design::cli::DesignCommand>
        + Send
        + 'static,
) -> Response {
    let Some(file) = state.file.clone() else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let root = state.project_root.clone();
    let gate = state.gate.clone();
    let work = move || -> DesignResult<serde_json::Value> {
        let _guard = gate
            .lock()
            .map_err(|_| crate::tokens::DesignError::new("internal", "document lock failed"))?;
        crate::design::cli::run(build(file, &root)?)
    };
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => design_problem(error),
        Err(error) => problem(anyhow::anyhow!("design task failed: {error}")),
    }
}

#[derive(serde::Deserialize, Default)]
struct DesignTokenQuery {
    #[serde(rename = "type")]
    token_type: Option<String>,
    source: Option<String>,
    #[serde(default)]
    resolve: bool,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[allow(clippy::result_large_err)]
async fn design_tokens(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<DesignTokenQuery>,
) -> Response {
    use crate::design::cli::{DesignCommand, TokenAction};
    design_read(state, move |file, _| {
        Ok(DesignCommand::Token {
            action: TokenAction::List {
                input: file,
                token_type: q.token_type,
                source: q.source,
                resolve: q.resolve,
                offset: q.offset,
                limit: q.limit,
            },
        })
    })
    .await
}

#[allow(clippy::result_large_err)]
async fn design_library_status(State(state): State<Shared>) -> Response {
    use crate::design::cli::{DesignCommand, LibraryAction};
    design_read(state, |file, root| {
        Ok(DesignCommand::Library {
            action: LibraryAction::Status {
                input: file,
                project: Some(root.to_path_buf()),
            },
        })
    })
    .await
}

#[derive(serde::Deserialize, Default)]
struct DesignAuditQuery {
    page: Option<String>,
    #[serde(default)]
    all_pages: bool,
    rules: Option<String>,
    library: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
    #[serde(default)]
    fix_plan: bool,
    #[serde(default)]
    fix_nearest: bool,
}

/// Read-only: the audit never writes; applying a fix plan goes through `/api/scene`.
#[allow(clippy::result_large_err)]
async fn design_audit(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<DesignAuditQuery>,
) -> Response {
    use crate::design::cli::DesignCommand;
    design_read(state, move |file, _| {
        Ok(DesignCommand::Audit {
            input: file,
            page: q.page,
            all_pages: q.all_pages,
            rules: q.rules,
            library: q.library,
            offset: q.offset,
            limit: q.limit,
            fix_plan: q.fix_plan,
            fix_nearest: q.fix_nearest,
            fail_on: None,
        })
    })
    .await
}

#[derive(serde::Deserialize, Default)]
struct DesignTemplateQuery {
    /// Project-relative directory (templates) or file (check); default `.` / the shared document.
    path: Option<String>,
    page: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[allow(clippy::result_large_err)]
async fn design_templates(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<DesignTemplateQuery>,
) -> Response {
    use crate::design::cli::{DesignCommand, TemplateAction};
    design_read(state, move |_, root| {
        let dir = match q.path.as_deref() {
            None | Some("") | Some(".") => root.to_path_buf(),
            Some(rel) => project_relative(root, rel)?,
        };
        Ok(DesignCommand::Template {
            action: TemplateAction::List {
                dir,
                offset: q.offset,
                limit: q.limit,
            },
        })
    })
    .await
}

#[allow(clippy::result_large_err)]
async fn design_template_check(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<DesignTemplateQuery>,
) -> Response {
    use crate::design::cli::{DesignCommand, TemplateAction};
    design_read(state, move |file, root| {
        let input = match q.path.as_deref() {
            None | Some("") => file,
            Some(rel) => project_relative(root, rel)?,
        };
        Ok(DesignCommand::Template {
            action: TemplateAction::Check {
                input,
                page: q.page,
            },
        })
    })
    .await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DesignFillRequest {
    /// Project-relative template file; default: the shared document.
    template: Option<String>,
    values: serde_json::Map<String, serde_json::Value>,
    /// Project-relative `.pen` file to create; never the template itself.
    output: String,
    page: Option<String>,
    /// Studio revision (GET /api/document) for the shared document, or the
    /// file's byte revision for another template. Required.
    revision: Option<String>,
    #[serde(default)]
    dry_run: bool,
    offset: Option<usize>,
    limit: Option<usize>,
}

/// Fills a template into a new document with `design template fill`. The
/// template is only read; the revision guard ties the plan to the bytes the
/// user saw, and dry run and commit share one planner.
#[allow(clippy::result_large_err)]
async fn design_template_fill(
    State(state): State<Shared>,
    Json(body): Json<DesignFillRequest>,
) -> Response {
    use crate::design::cli::{DesignCommand, TemplateAction};
    let Some(shared) = state.file.clone() else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let root = state.project_root.clone();
    let gate = state.gate.clone();
    let work = move || -> DesignResult<serde_json::Value> {
        let _guard = gate
            .lock()
            .map_err(|_| crate::tokens::DesignError::new("internal", "document lock failed"))?;
        let output = project_relative(&root, &body.output)?;
        if output.extension().is_none_or(|e| e != "pen") {
            return Err(design_invalid("output must be a .pen file"));
        }
        let input = match body.template.as_deref() {
            None | Some("") => shared.clone(),
            Some(rel) => project_relative(&root, rel)?,
        };
        let pinned = if input == shared {
            let bytes = std::fs::read(&input)
                .map_err(|e| design_invalid(format!("could not read {}: {e}", input.display())))?;
            let raw: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| design_invalid(format!("template is not valid JSON: {e}")))?;
            require_revision(body.revision.as_deref(), &raw).map_err(|e| {
                crate::tokens::DesignError::new("conflict", format!("{e:#}"))
                    .action("reload the document and plan again")
            })?;
            crate::transaction::revision(&bytes)
        } else {
            body.revision.clone().ok_or_else(|| {
                crate::tokens::DesignError::new("conflict", "revision is required")
                    .action("send the template's revision from the template listing")
            })?
        };
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let values_path = std::env::temp_dir().join(format!(
            "pentool-design-values-{}-{nonce}.json",
            std::process::id()
        ));
        std::fs::write(
            &values_path,
            serde_json::to_vec(&body.values).unwrap_or_default(),
        )
        .map_err(|e| design_invalid(format!("could not stage values: {e}")))?;
        let result = crate::design::cli::run(DesignCommand::Template {
            action: TemplateAction::Fill {
                input,
                values: Some(values_path.clone()),
                output: Some(output),
                data: None,
                output_dir: None,
                page: body.page,
                dry_run: body.dry_run,
                if_revision: Some(pinned),
                offset: body.offset,
                limit: body.limit,
            },
        });
        let _ = std::fs::remove_file(&values_path);
        result
    };
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => design_problem(error),
        Err(error) => problem(anyhow::anyhow!("design task failed: {error}")),
    }
}

/// Installed extensions of the served project, with the capabilities each was approved for.
async fn extensions_list(State(state): State<Shared>) -> Response {
    match crate::extension::store::list(&state.project_root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionRunRequest {
    name: String,
    command: Option<String>,
    args: Option<serde_json::Value>,
    page: Option<String>,
    revision: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

/// Runs an installed extension against the shared document. The run goes through
/// the same host and transaction as `pentool extension run`; installing and
/// approving capabilities stays a CLI action.
async fn extensions_run(
    State(state): State<Shared>,
    Json(body): Json<ExtensionRunRequest>,
) -> Response {
    let Some(file) = state.file.clone() else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let root = state.project_root.clone();
    let gate = state.gate.clone();
    let work = move || -> Result<serde_json::Value> {
        let _guard = gate
            .lock()
            .map_err(|_| anyhow::anyhow!("document lock failed"))?;
        // Studio revisions are value hashes (GET /api/document); the transaction
        // guard uses the file's byte hash. Check the former, then pin the latter
        // under the same lock so nothing can change in between.
        let bytes = std::fs::read(&file)?;
        let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
        require_revision(body.revision.as_deref(), &raw)?;
        let pinned = crate::transaction::revision(&bytes);
        crate::extension::store::run(
            &root,
            &body.name,
            &file,
            crate::extension::store::RunOptions {
                command: body.command,
                args: body.args,
                event: None,
                page: body.page,
                dry_run: body.dry_run,
                if_revision: Some(pinned),
            },
        )
    };
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => problem(error),
        Err(error) => problem(anyhow::anyhow!("extension task failed: {error}")),
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
        if !(4..=11).contains(&body.target) {
            bail!("[invalid-input] migration target must be between 4 and 11");
        }
        if body.target <= from {
            bail!(
                "[invalid-input] the document is already format v{from}; choose a newer target than v{from}"
            );
        }
        let migrated = match body.target {
            11 => crate::scene3d::node::migrate_to_v11(body.document)?,
            10 => crate::scene3d::node::migrate(crate::scene3d::node::to_v10(body.document)?)?,
            9 => crate::spreads::migrate(body.document)?,
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
        let (limit, offset) = (q.limit.unwrap_or(200), q.offset.unwrap_or(0));
        // A project gets its catalog with its first photo. Until then the grid is
        // empty, and Studio needs this revision to import that first photo.
        if raw.get("photography").is_none() {
            return Ok(serde_json::json!({
                "query": q.query, "matches": 0, "returned": 0, "offset": offset,
                "limit": limit, "has_more": false, "photos": [], "revision": revision,
            }));
        }
        let mut value = crate::photo::organize::search(&raw, &q.query, limit, offset)?;
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
#[serde(deny_unknown_fields)]
struct PhotoImportQuery {
    id: String,
    name: String,
}

/// Import a DNG, JPEG or PNG from Studio into the shared project.
fn photo_import_apply(
    file: &std::path::Path,
    gate: &Mutex<()>,
    query: &PhotoImportQuery,
    expected: &str,
    bytes: &[u8],
) -> Result<serde_json::Value> {
    const MAX_STUDIO_DNG_BYTES: usize = 64 * 1024 * 1024;
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_STUDIO_DNG_BYTES,
        "[limit-exceeded] Studio accepts photo files up to 64 MiB"
    );
    anyhow::ensure!(
        !query.name.trim().is_empty() && query.name.len() <= 512,
        "[invalid-input] photo name must contain 1 to 512 bytes"
    );
    let _guard = gate
        .lock()
        .map_err(|_| anyhow::anyhow!("document lock failed"))?;
    let original = std::fs::read(file)?;
    let current = crate::transaction::revision(&original);
    anyhow::ensure!(
        expected == current,
        "[conflict] the document changed since revision {expected} (now {current}); reload the photo catalog and retry"
    );
    let mut raw: serde_json::Value = serde_json::from_slice(&original)?;
    let is_dng = crate::photo::dng::Dng::inspect(bytes).is_ok();
    let result = if is_dng {
        crate::photo::catalog::add_raw(
            &mut raw,
            &query.id,
            query.name.trim(),
            bytes,
            crate::image::embedded_storage(bytes),
            crate::photo::catalog::camera_profile("auto")?,
        )?
    } else {
        crate::photo::catalog::add_rendered(
            &mut raw,
            &query.id,
            query.name.trim(),
            bytes,
            crate::image::embedded_storage(bytes),
        )?
    };
    let change = crate::transaction::commit_value(
        file,
        "browser-photo-raw-add",
        false,
        Some(expected),
        &raw,
    )?;
    let revision = crate::transaction::revision(&std::fs::read(file)?);
    Ok(serde_json::json!({"change":change,"result":result,"revision":revision}))
}

async fn photo_import(
    State(state): State<Shared>,
    axum::extract::Query(query): axum::extract::Query<PhotoImportQuery>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(expected) = headers
        .get("x-pentool-revision")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return problem(anyhow::anyhow!(
            "[invalid-input] photo import requires the latest x-pentool-revision header"
        ));
    };
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
        let file = state
            .file
            .clone()
            .context("[missing-resource] no shared document; run `pentool serve PROJECT.pen`")?;
        photo_import_apply(&file, &state.gate, &query, &expected, &body)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|result| result);
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PhotoPlaceRequest {
    photo: String,
    #[serde(default = "master")]
    variant: String,
    page: String,
    layer: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    #[serde(default = "photo_fit_contain")]
    fit: String,
    #[serde(default = "photo_center_position")]
    position: [f64; 2],
    revision: String,
}

fn photo_fit_contain() -> String {
    "contain".into()
}

fn photo_center_position() -> [f64; 2] {
    [0.5, 0.5]
}

fn photo_place_apply(
    file: &std::path::Path,
    gate: &Mutex<()>,
    body: &PhotoPlaceRequest,
) -> Result<serde_json::Value> {
    let _guard = gate
        .lock()
        .map_err(|_| anyhow::anyhow!("document lock failed"))?;
    let bytes = std::fs::read(file)?;
    let current = crate::transaction::revision(&bytes);
    anyhow::ensure!(
        body.revision == current,
        "[conflict] the document changed since revision {} (now {current}); reload the photo and retry",
        body.revision
    );
    let mut raw: serde_json::Value = serde_json::from_slice(&bytes)?;
    let result = crate::photo::catalog::place(
        &mut raw,
        file,
        &body.photo,
        &body.variant,
        &crate::photo::catalog::Placement {
            page: Some(body.page.clone()),
            layer: body.layer.clone(),
            id: None,
            x: body.x,
            y: body.y,
            width: Some(body.width),
            height: Some(body.height),
            fit: body.fit.clone(),
            position: body.position,
        },
    )?;
    let change = crate::transaction::commit_value(
        file,
        "browser-photo-place",
        false,
        Some(&body.revision),
        &raw,
    )?;
    let revision = crate::transaction::revision(&std::fs::read(file)?);
    Ok(serde_json::json!({"change":change,"result":result,"revision":revision}))
}

async fn photo_place(State(state): State<Shared>, Json(body): Json<PhotoPlaceRequest>) -> Response {
    let result = tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
        let file = state
            .file
            .clone()
            .context("[missing-resource] no shared document; run `pentool serve PROJECT.pen`")?;
        photo_place_apply(&file, &state.gate, &body)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|result| result);
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

#[derive(serde::Deserialize)]
struct CommentsQuery {
    status: Option<String>,
    review_state: Option<String>,
    page: Option<String>,
    object: Option<String>,
    mention: Option<String>,
    thread: Option<String>,
    #[serde(default)]
    comments: bool,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

async fn comments_list(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<CommentsQuery>,
) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let options = crate::comments::ListOptions {
        status: q.status,
        review_state: q.review_state,
        page: q.page,
        object: q.object,
        mention: q.mention,
        thread: q.thread,
        comments: q.comments,
        offset: q.offset,
        limit: q.limit,
    };
    match crate::comments::list(&file, &options) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

async fn comments_apply(
    State(state): State<Shared>,
    principal: Option<axum::Extension<crate::collab::token::Principal>>,
    Json(mut request): Json<serde_json::Value>,
) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    // Under a collaboration token the author is the token's actor, as for
    // operations; a body `actor` cannot attribute a comment to someone else.
    if let Some(axum::Extension(principal)) = principal.filter(|p| p.token.is_some()) {
        if let Some(object) = request.as_object_mut() {
            object.insert("actor".into(), comment_actor(&principal.actor));
        }
    }
    match crate::comments::apply(&file, &request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

fn comment_actor(actor: &crate::collab::op::Actor) -> serde_json::Value {
    let mut value = serde_json::json!({
        "kind": actor.kind,
        "id": actor.id,
        "name": actor.display_name.as_deref().unwrap_or(&actor.id),
    });
    if let Some(model) = &actor.model {
        value["model"] = serde_json::json!(model);
    }
    if let Some(director) = &actor.directed_by {
        value["directed_by"] = serde_json::json!(director);
    }
    value
}

#[derive(serde::Deserialize)]
struct CollabStatsQuery {
    by: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

/// Read-only contribution statistics derived from the verified operation log.
async fn collab_stats(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<CollabStatsQuery>,
) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let result =
        crate::collab::stats::By::parse(q.by.as_deref().unwrap_or("actor")).and_then(|by| {
            crate::collab::stats_report(
                &file,
                by,
                q.offset.unwrap_or(0),
                q.limit.unwrap_or(crate::collab::stats::DEFAULT_LIMIT),
            )
        });
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

/// Named merge conflicts from replaying the verified operation log (read only).
async fn collab_conflicts(State(state): State<Shared>) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    match crate::collab::verify_report(&file) {
        Ok(report) => Json(serde_json::json!({
            "chain": report["chain"],
            "head": report["head"],
            "ops": report["ops"],
            "conflicts_total": report["replay"]["conflicts_total"],
            "conflicts": report["replay"]["conflicts"],
            "rejected": report["replay"]["rejected"],
            "pending": report["replay"]["pending"],
        }))
        .into_response(),
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
#[derive(serde::Deserialize)]
struct HistoryHash {
    hash: String,
}

/// One stored version, for previewing it in Studio's version history.
async fn history_version(
    State(state): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<HistoryHash>,
) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let result = (|| -> Result<serde_json::Value> {
        let bytes = crate::history::version(&file, &q.hash)?;
        let document: serde_json::Value = serde_json::from_slice(&bytes)?;
        Ok(serde_json::json!({"hash":q.hash,"document":document}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

/// Restore a stored version as a new, undoable history step.
async fn history_restore(State(state): State<Shared>, Json(body): Json<HistoryHash>) -> Response {
    let Some(file) = state.file else {
        return (StatusCode::NOT_FOUND, "No shared document").into_response();
    };
    let result = (|| -> Result<serde_json::Value> {
        let _guard = state
            .gate
            .lock()
            .map_err(|_| anyhow::anyhow!("document lock failed"))?;
        crate::history::restore_as_step(&file, &body.hash)
    })();
    match result {
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
    let preview = query.max_edge.is_some();
    run_job(state, query.job, "export", move || {
        if svg {
            Ok(
                crate::composite::svg(&value, &document, query.page.as_deref(), scale)?
                    .into_bytes(),
            )
        } else {
            composite_png(&value, &document, query.page.as_deref(), scale, preview)
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
            let scale = preview_scale(
                f64::from(scene.width),
                f64::from(scene.height),
                query.max_edge,
                0.1,
            )?;
            return render::scene_to_png(&scene, scale);
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
        let scale = preview_scale(
            f64::from(doc.canvas.width),
            f64::from(doc.canvas.height),
            query.max_edge,
            0.1,
        )?;
        render::to_png_cached(&doc, scale).map(|(bytes, _)| bytes)
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

/// A provider-setup failure keeps the CLI's stable `code` and `fix` next to the
/// message so the settings panel can show them without parsing text.
fn ai_problem(error: anyhow::Error) -> Response {
    let (code, fix) = error
        .downcast_ref::<crate::ai::AiError>()
        .map(|e| (Some(e.code), Some(e.fix.clone())))
        .unwrap_or((None, None));
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": format!("{error:#}"), "code": code, "fix": fix})),
    )
        .into_response()
}

async fn ai_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| anyhow::anyhow!("provider task failed: {e}"))?
}

async fn ai_status() -> Response {
    match ai_blocking(crate::ai::studio_status).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => ai_problem(e),
    }
}

async fn ai_connect(Json(body): Json<crate::ai::ConnectRequest>) -> Response {
    match ai_blocking(move || crate::ai::studio_connect(body)).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => ai_problem(e),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AiDoctorRequest {
    provider: Option<String>,
    #[serde(default)]
    check: bool,
}

async fn ai_doctor(Json(body): Json<AiDoctorRequest>) -> Response {
    match ai_blocking(move || crate::ai::studio_doctor(body.provider, body.check)).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => ai_problem(e),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AiRemoveRequest {
    name: String,
}

async fn ai_remove(Json(body): Json<AiRemoveRequest>) -> Response {
    match ai_blocking(move || crate::ai::studio_remove(body.name)).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => ai_problem(e),
    }
}

fn problem(error: anyhow::Error) -> Response {
    (
        StatusCode::BAD_REQUEST,
        // `{:#}` keeps the cause chain, so batch errors name the failing check.
        Json(serde_json::json!({"error": format!("{error:#}")})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn studio_tracks_the_newest_format() {
        // Studio creates documents at LATEST_FORMAT and renders them locally
        // only inside localVersion; both went stale when v12 shipped, so new
        // Studio documents missed v12 and upgraded ones lost the fast path.
        let latest = crate::scene::LATEST_VERSION;
        let store = include_str!("../web-ui/src/store.ts");
        assert!(
            store.contains(&format!("export const LATEST_FORMAT = {latest};")),
            "web-ui/src/store.ts LATEST_FORMAT must be {latest}"
        );
        let wasm = include_str!("../web-ui/src/lib/wasmCore.ts");
        assert!(
            wasm.contains(&format!("version >= 6 && version <= {latest};")),
            "web-ui/src/lib/wasmCore.ts localVersion must include v{latest}"
        );
    }

    #[test]
    fn studio_photo_import_embeds_dng_and_rejects_stale_or_invalid_input_atomically() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pentool-photo-import-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("photos.pen");
        let original = serde_json::to_vec(&crate::scene::new_document(80, 60)).unwrap();
        std::fs::write(&file, &original).unwrap();
        let gate = std::sync::Mutex::new(());
        let query = super::PhotoImportQuery {
            id: "studio-shot".into(),
            name: "Studio Shot.dng".into(),
        };
        let expected = crate::transaction::revision(&original);
        let dng = include_bytes!("../docs/fixtures/photo-dng-rggb16.dng");
        let imported = super::photo_import_apply(&file, &gate, &query, &expected, dng).unwrap();
        let saved = std::fs::read(&file).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(imported["result"]["photo"], "studio-shot");
        assert!(document["photography"]["photos"]
            .as_array()
            .unwrap()
            .iter()
            .any(|photo| photo["id"] == "studio-shot"));

        let after_import = saved.clone();
        let stale = super::photo_import_apply(&file, &gate, &query, &expected, dng).unwrap_err();
        assert!(stale.to_string().contains("[conflict]"));
        assert_eq!(std::fs::read(&file).unwrap(), after_import);

        let latest = crate::transaction::revision(&after_import);
        let invalid = super::photo_import_apply(&file, &gate, &query, &latest, b"not a DNG");
        assert!(invalid.is_err());
        assert_eq!(std::fs::read(&file).unwrap(), after_import);

        let place = super::PhotoPlaceRequest {
            photo: "studio-shot".into(),
            variant: "master".into(),
            page: "page-1".into(),
            layer: "layer-1".into(),
            x: 20.0,
            y: 10.0,
            width: 40.0,
            height: 30.0,
            fit: "contain".into(),
            position: [0.5, 0.5],
            revision: latest,
        };
        let placed = super::photo_place_apply(&file, &gate, &place).unwrap();
        assert_eq!(placed["result"]["photo"], "studio-shot");
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert!(document["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["kind"] == "photo" && node["photo"] == "studio-shot"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn browser_design_import_is_local_namespaced_and_reports_fidelity() {
        use base64::Engine;
        let document = crate::scene::new_document(100, 80);
        let request = super::ImportRequest {
            document: document.clone(),
            source: serde_json::Value::String(
                base64::engine::general_purpose::STANDARD.encode(
                    br#"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="20"><rect id="box" width="10" height="8"/></svg>"#,
                ),
            ),
            options: crate::import::ImportOptions {
                destination_page: Some("page-1".into()),
                source_page: None,
                prefix: Some("local".into()),
                x: 0.0,
                y: 0.0,
                scale: 1.0,
                rotation: 0.0,
                expand_canvas: false,
            },
            format: Some("svg".into()),
            source_name: Some("local.svg".into()),
        };
        let result = super::plan_design_import(request, "svg").unwrap();
        assert_eq!(result["local"], true);
        assert_eq!(result["report"]["format"], "svg");
        assert_eq!(result["summary"]["layer"], "local-import");
        crate::scene::validate(&result["document"]).unwrap();
        assert_eq!(document["pages"][0]["layers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn browser_design_import_refuses_active_svg_without_mutating_document() {
        use base64::Engine;
        let document = crate::scene::new_document(100, 80);
        let request = super::ImportRequest {
            document: document.clone(),
            source: serde_json::Value::String(
                base64::engine::general_purpose::STANDARD
                    .encode(br#"<svg xmlns="http://www.w3.org/2000/svg"><script>x</script></svg>"#),
            ),
            options: crate::import::ImportOptions {
                destination_page: Some("page-1".into()),
                source_page: None,
                prefix: Some("local".into()),
                x: 0.0,
                y: 0.0,
                scale: 1.0,
                rotation: 0.0,
                expand_canvas: false,
            },
            format: Some("svg".into()),
            source_name: Some("active.svg".into()),
        };
        assert!(super::plan_design_import(request, "svg")
            .unwrap_err()
            .to_string()
            .contains("active SVG element"));
        assert_eq!(document["pages"][0]["layers"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn packaging_bundle_api_returns_the_complete_print_zip() {
        use std::io::Read as _;
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pentool-pack-api-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.pen");
        crate::color::catalog::import(
            &mut document,
            &source,
            include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
            "press",
            "allowed",
            None,
            None,
        )
        .unwrap();
        document["color_management"]["roles"] = serde_json::json!({"output":{
            "destination":{"space":"icc-cmyk","profile":"press"},
            "intent":"relative-colorimetric"
        }});
        std::fs::write(&source, serde_json::to_vec(&document).unwrap()).unwrap();
        let state = super::Shared {
            file: Some(source.clone()),
            project_root: root.clone(),
            gate: std::sync::Arc::new(std::sync::Mutex::new(())),
            previews: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            no_cache: false,
            document_streams: std::sync::Arc::default(),
        };
        let response = super::scene3d_pack_export_bundle(
            axum::extract::State(state),
            axum::Json(super::Scene3dPackExportBundleRequest {
                document: document.clone(),
                dieline: "shirt-carton".into(),
                min_ppi: 36.0,
                ppi: 36.0,
                fold: 1.0,
            }),
        )
        .await;
        let status = response.status();
        let content_type = response.headers()[axum::http::header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_owned();
        let bytes = axum::body::to_bytes(response.into_body(), 512 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(content_type, "application/zip");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_owned())
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "PRINT-GUIDE.txt"));
        assert!(names
            .iter()
            .any(|name| name.starts_with("vector-spot-plates/") && name.ends_with(".svg")));
        let mut guide = archive.by_name("PRINT-GUIDE.txt").unwrap();
        let mut guide_text = String::new();
        guide.read_to_string(&mut guide_text).unwrap();
        assert!(guide_text.contains("CutContour = through-cut paths"));
        assert!(guide_text.contains("Check folded-preview.png first"));
        assert_eq!(
            std::fs::read(&source).unwrap(),
            serde_json::to_vec(&document).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packaging_bundle_zip_contains_relative_paths_and_bytes() {
        use std::io::Read as _;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pentool-pack-zip-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("plates")).unwrap();
        std::fs::write(root.join("manifest.json"), b"manifest").unwrap();
        std::fs::write(root.join("plates/CutContour.svg"), b"cut path").unwrap();

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let mut total = 0;
        super::zip_pack_directory(&root, &root, &mut writer, &mut total).unwrap();
        assert_eq!(total, b"manifest".len() + b"cut path".len());
        let bytes = writer.finish().unwrap().into_inner();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["manifest.json", "plates/CutContour.svg"]);
        let mut cut = archive.by_name("plates/CutContour.svg").unwrap();
        let mut content = String::new();
        cut.read_to_string(&mut content).unwrap();
        assert_eq!(content, "cut path");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scene3d_server_render_report_uses_the_shared_renderer() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let request = super::Scene3dRenderReportRequest {
            document: document.clone(),
            page: "products".into(),
            node: "perfume-preview".into(),
        };
        let report = super::plan_scene3d_render_report(&request, &std::env::temp_dir()).unwrap();
        assert_eq!(report["node"], "perfume-preview");
        assert_eq!(report["page"], "products");
        assert_eq!(
            report["render"]["format"],
            "pentool-scene3d-render-report-v1"
        );
        assert!(report["render"]["output"]["width_px"].as_u64().unwrap() > 0);
        assert!(report["render"]["output"]["height_px"].as_u64().unwrap() > 0);
        assert!(report["render"]["effective_ppi"]["x"].as_f64().unwrap() > 0.0);
        assert_eq!(report["render"]["effective_ppi"]["units_per_inch"], 72.0);
        assert_eq!(request.document, document);
    }

    #[test]
    fn browser_scene3d_import_is_bounded_validated_and_atomic() {
        use base64::Engine as _;
        let raw = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let layer = raw["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        let request = super::Scene3dImportRequest {
            document: raw.clone(),
            data: base64::engine::general_purpose::STANDARD.encode(obj),
            format: "obj".into(),
            id: "model-1".into(),
            layer,
            page: None,
            x: 10.0,
            y: 20.0,
            width: 200.0,
            height: 180.0,
        };
        let mut imported = raw.clone();
        let result =
            super::plan_scene3d_import(&mut imported, &request, &std::env::temp_dir()).unwrap();
        assert_eq!(result["id"], "model-1");
        assert_eq!(result["report"]["triangles"], 1);
        assert_eq!(
            imported["pages"][0]["layers"][0]["nodes"][0]["kind"],
            "scene3d"
        );
        assert_eq!(imported["models"].as_object().unwrap().len(), 1);

        let mut unchanged = raw.clone();
        let bad = super::Scene3dImportRequest {
            format: "fbx".into(),
            ..request
        };
        assert!(super::plan_scene3d_import(&mut unchanged, &bad, &std::env::temp_dir()).is_err());
        assert_eq!(unchanged, raw);

        let malformed = super::Scene3dImportRequest {
            format: "obj".into(),
            data: base64::engine::general_purpose::STANDARD.encode(b"not an obj"),
            ..bad
        };
        assert!(
            super::plan_scene3d_import(&mut unchanged, &malformed, &std::env::temp_dir()).is_err()
        );
        assert_eq!(unchanged, raw);
    }

    #[test]
    fn browser_uv_preview_uses_the_loaded_model_and_validates_primitive_bounds() {
        use base64::Engine as _;
        let raw = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let layer = raw["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n";
        let mut document = raw;
        let import = super::Scene3dImportRequest {
            document: document.clone(),
            data: base64::engine::general_purpose::STANDARD.encode(obj),
            format: "obj".into(),
            id: "uv-model".into(),
            layer,
            page: None,
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        super::plan_scene3d_import(&mut document, &import, &std::env::temp_dir()).unwrap();
        let request = super::Scene3dUvPreviewRequest {
            document: document.clone(),
            node: "uv-model".into(),
            primitive: 0,
            size: 128,
            texture_resolution: 4096,
            minimum_density: Some(5000.0),
        };
        let preview = super::plan_scene3d_uv_preview(&request, &std::env::temp_dir()).unwrap();
        assert_eq!(preview["material_slot"], serde_json::Value::Null);
        assert!(preview["svg"].as_str().unwrap().contains("<path"));
        let png = base64::engine::general_purpose::STANDARD
            .decode(preview["png_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(preview["report"]["primitives"][0]["uv_sets"], 1);
        assert_eq!(
            preview["report"]["primitives"][0]["texel_density"][0]["texture_resolution_px"],
            serde_json::json!([4096, 4096])
        );
        assert_eq!(
            preview["report"]["primitives"][0]["texel_density"][0]["below_minimum_texel_density"],
            true
        );

        let invalid = super::Scene3dUvPreviewRequest {
            primitive: 9,
            ..request
        };
        let error = super::plan_scene3d_uv_preview(&invalid, &std::env::temp_dir()).unwrap_err();
        assert!(error.to_string().contains("primitive 9"));
    }

    #[test]
    fn browser_uv_face_pick_selects_the_hit_island_without_mutating_document() {
        use base64::Engine as _;
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let mut document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        let layer = document["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n";
        let import = super::Scene3dImportRequest {
            document: document.clone(),
            data: base64::engine::general_purpose::STANDARD.encode(obj),
            format: "obj".into(),
            id: "uv-face-pick-model".into(),
            layer,
            page: Some(page.clone()),
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        super::plan_scene3d_import(&mut document, &import, &std::env::temp_dir()).unwrap();
        let digest = document["pages"][0]["layers"][0]["nodes"][0]["model"]
            .as_str()
            .unwrap()
            .to_owned();
        document["models"][&digest]["uv"]["0"] = serde_json::json!({
            "islands":[{"id":"label","uv_set":0,"triangles":[0],"transform":{"scale":[1,1],"offset":[0,0],"rotate":0.0}}]
        });
        let before = document.clone();
        let request = |x, y| super::Scene3dUvFacePickRequest {
            document: document.clone(),
            page: page.clone(),
            node: "uv-face-pick-model".into(),
            primitive: Some(0),
            x,
            y,
        };
        let hit =
            super::plan_scene3d_uv_face_pick(&request(0.5, 0.5), &std::env::temp_dir()).unwrap();
        assert_eq!(hit["triangle"], 0);
        assert_eq!(hit["island"], "label");
        assert_eq!(hit["uv"].as_array().unwrap().len(), 2);
        assert_eq!(document, before);
        assert!(
            super::plan_scene3d_uv_face_pick(&request(f64::NAN, 0.5), &std::env::temp_dir())
                .is_err()
        );
    }

    #[test]
    fn browser_uv_model_preview_highlights_selected_island_without_mutating_document() {
        use base64::Engine as _;
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let mut document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        let layer = document["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n";
        let import = super::Scene3dImportRequest {
            document: document.clone(),
            data: base64::engine::general_purpose::STANDARD.encode(obj),
            format: "obj".into(),
            id: "uv-highlight-model".into(),
            layer,
            page: Some(page.clone()),
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        super::plan_scene3d_import(&mut document, &import, &std::env::temp_dir()).unwrap();
        let digest = document["pages"][0]["layers"][0]["nodes"][0]["model"]
            .as_str()
            .unwrap()
            .to_owned();
        document["models"][&digest]["uv"]["0"] = serde_json::json!({
            "islands":[{"id":"label","uv_set":0,"triangles":[0],"transform":{"scale":[1,1],"offset":[0,0],"rotate":0.0}}]
        });
        let before = document.clone();
        let request = super::Scene3dUvModelPreviewRequest {
            document: document.clone(),
            page: page.clone(),
            node: "uv-highlight-model".into(),
            primitive: 0,
            island: None,
        };
        let plain = super::plan_scene3d_uv_model_preview(&request, &std::env::temp_dir()).unwrap();
        let highlighted = super::plan_scene3d_uv_model_preview(
            &super::Scene3dUvModelPreviewRequest {
                island: Some("label".into()),
                ..request
            },
            &std::env::temp_dir(),
        )
        .unwrap();
        let plain_png = base64::engine::general_purpose::STANDARD
            .decode(plain["png_base64"].as_str().unwrap())
            .unwrap();
        let highlighted_png = base64::engine::general_purpose::STANDARD
            .decode(highlighted["png_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            u32::from_be_bytes(plain_png[16..20].try_into().unwrap()),
            512
        );
        assert_eq!(
            u32::from_be_bytes(plain_png[20..24].try_into().unwrap()),
            512
        );
        assert_ne!(plain_png, highlighted_png);
        assert_eq!(document, before);
    }

    #[test]
    fn browser_uv_model_preview_renders_and_highlights_product_forms() {
        use base64::Engine as _;
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let mut document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        document["product_forms"]["perfume-bottle"] =
            crate::scene3d::form::starter_preset("perfume-bottle").unwrap();
        document["pages"][0]["layers"][0]["nodes"] = serde_json::json!([{
            "kind":"scene3d", "id":"uv-product-form", "x":0, "y":0,
            "width":160, "height":200, "product_form":"perfume-bottle"
        }]);
        document["product_forms"]["perfume-bottle"]["uv"]["0"] = serde_json::json!({
            "islands":[{"id":"bottle-body-island","uv_set":0,"triangles":[0],
                "transform":{"scale":[0.8,0.8],"offset":[0.1,0.1],"rotate":0.0}}]
        });
        crate::scene::validate(&document).unwrap();
        let before = document.clone();
        let request = super::Scene3dUvModelPreviewRequest {
            document: document.clone(),
            page,
            node: "uv-product-form".into(),
            primitive: 0,
            island: None,
        };
        let plain = super::plan_scene3d_uv_model_preview(&request, &std::env::temp_dir()).unwrap();
        assert_eq!(plain["primitive"], 0);
        assert_eq!(plain["island"], serde_json::Value::Null);
        let highlighted = super::plan_scene3d_uv_model_preview(
            &super::Scene3dUvModelPreviewRequest {
                island: Some("bottle-body-island".into()),
                ..request
            },
            &std::env::temp_dir(),
        )
        .unwrap();
        let plain_png = base64::engine::general_purpose::STANDARD
            .decode(plain["png_base64"].as_str().unwrap())
            .unwrap();
        let highlighted_png = base64::engine::general_purpose::STANDARD
            .decode(highlighted["png_base64"].as_str().unwrap())
            .unwrap();
        assert_ne!(plain_png, highlighted_png);
        assert_eq!(document, before);
    }

    #[test]
    fn browser_uv_pack_returns_a_valid_undoable_document_without_mutating_input() {
        use base64::Engine as _;
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let mut document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        let layer = document["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n";
        let import = super::Scene3dImportRequest {
            document: document.clone(),
            data: base64::engine::general_purpose::STANDARD.encode(obj),
            format: "obj".into(),
            id: "pack-model".into(),
            layer,
            page: Some(page.clone()),
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        super::plan_scene3d_import(&mut document, &import, &std::env::temp_dir()).unwrap();
        let digest = document["pages"][0]["layers"][0]["nodes"][0]["model"]
            .as_str()
            .unwrap()
            .to_owned();
        document["models"][&digest]["uv"]["0"] = serde_json::json!({
            "islands":[{"id":"face","uv_set":0,"triangles":[0],"transform":{"scale":[0.4,0.4],"offset":[0.1,0.1],"rotate":0.0}}]
        });
        let before = document.clone();
        let request = super::Scene3dUvPackRequest {
            document: document.clone(),
            page,
            node: "pack-model".into(),
            primitive: 0,
            padding: 0.02,
        };
        let packed = super::plan_scene3d_uv_pack(&request, &std::env::temp_dir()).unwrap();
        assert_eq!(document, before);
        assert_eq!(packed["result"]["islands"], 1);
        let bounds = &packed["result"]["report"]["islands"][0]["uv_bounds"];
        assert!(bounds["min"][0].as_f64().unwrap() >= 0.02 - 1e-6);
        assert!(bounds["min"][1].as_f64().unwrap() >= 0.02 - 1e-6);
        assert!(bounds["max"][0].as_f64().unwrap() <= 0.98 + 1e-6);
        assert!(bounds["max"][1].as_f64().unwrap() <= 0.98 + 1e-6);
        crate::scene::validate(&packed["document"]).unwrap();

        let invalid = super::Scene3dUvPackRequest {
            primitive: 9,
            ..request
        };
        let error = super::plan_scene3d_uv_pack(&invalid, &std::env::temp_dir()).unwrap_err();
        assert!(error.to_string().contains("primitive 9"));
        assert_eq!(document, before);
    }

    #[test]
    fn browser_product_form_add_and_edit_validate_before_returning_document() {
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        let layer = document["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let request = super::Scene3dFormRequest {
            document: document.clone(),
            action: "add".into(),
            form_id: "perfume-form".into(),
            preset: None,
            kind: Some("lathe-bottle".into()),
            parameters: Some(
                serde_json::json!({"height_mm":100,"radial_segments":32,"profile_mm":[[18,0],[20,20],[18,70],[8,82],[8,100]]}),
            ),
            material_slots: Some(vec!["body".into(), "label".into()]),
            node_id: Some("perfume-node".into()),
            layer: Some(layer),
            page: Some(page),
            placement: Some([10.0, 10.0, 120.0, 180.0]),
        };
        let added = super::plan_scene3d_form(&request).unwrap();
        assert_eq!(
            added["document"]["product_forms"]["perfume-form"]["kind"],
            "lathe-bottle"
        );
        assert_eq!(added["result"]["triangles"], 1024);
        let mut form_uv_document = added["document"].clone();
        form_uv_document["product_forms"]["perfume-form"]["uv"] = serde_json::json!({
            "0": {
                "projection": "planar",
                "islands": [{
                    "id": "profile-front",
                    "uv_set": 0,
                    "triangles": [0],
                    "transform": {"scale": [1, 1], "offset": [0, 0], "rotate": 0}
                }]
            }
        });
        let uv_preview = super::plan_scene3d_uv_preview(
            &super::Scene3dUvPreviewRequest {
                document: form_uv_document,
                node: "perfume-node".into(),
                primitive: 0,
                size: 256,
                texture_resolution: 512,
                minimum_density: None,
            },
            &std::env::temp_dir(),
        )
        .unwrap();
        assert_eq!(uv_preview["source"]["kind"], "product_form");
        assert_eq!(uv_preview["source"]["id"], "perfume-form");
        assert_eq!(uv_preview["report"]["primitives"][0]["primitive"], 0);
        assert_eq!(
            uv_preview["report"]["primitives"][0]["islands"][0]["id"],
            "profile-front"
        );
        let picked = super::plan_scene3d_uv_face_pick(
            &super::Scene3dUvFacePickRequest {
                document: added["document"].clone(),
                page: request.page.clone().unwrap(),
                node: "perfume-node".into(),
                primitive: Some(0),
                x: 0.5,
                y: 0.5,
            },
            &std::env::temp_dir(),
        )
        .unwrap();
        assert!(picked["triangle"].is_u64(), "{picked}");
        assert_eq!(picked["material_slot"], "body");
        assert_eq!(picked["uv"].as_array().unwrap().len(), 2);

        let starter = super::Scene3dFormRequest {
            document: document.clone(),
            action: "add".into(),
            form_id: "starter-jar-form".into(),
            preset: Some("cosmetic-jar".into()),
            kind: None,
            parameters: None,
            material_slots: None,
            node_id: Some("starter-jar-node".into()),
            layer: request.layer.clone(),
            page: request.page.clone(),
            placement: Some([20.0, 20.0, 120.0, 180.0]),
        };
        let started = super::plan_scene3d_form(&starter).unwrap();
        assert_eq!(
            started["document"]["product_forms"]["starter-jar-form"]["kind"],
            "lathe-bottle"
        );
        assert_eq!(
            started["document"]["product_forms"]["starter-jar-form"]["material_slots"][2],
            "cap"
        );

        let edit = super::Scene3dFormRequest {
            document: added["document"].clone(),
            action: "set".into(),
            form_id: "perfume-form".into(),
            preset: None,
            kind: None,
            parameters: Some(
                serde_json::json!({"height_mm":120,"radial_segments":32,"profile_mm":[[18,0],[20,20],[18,70],[8,82],[8,120]],"profile_tangents":[0.0,0.2,0.0,-0.2,0.0],"cap_mm":{"radius_mm":9,"height_mm":18}}),
            ),
            material_slots: Some(vec!["body".into(), "label".into(), "cap".into()]),
            node_id: None,
            layer: None,
            page: None,
            placement: None,
        };
        let updated = super::plan_scene3d_form(&edit).unwrap();
        assert_eq!(
            updated["document"]["product_forms"]["perfume-form"]["parameters"]["height_mm"],
            120
        );
        assert_eq!(
            updated["document"]["product_forms"]["perfume-form"]["material_slots"][2],
            "cap"
        );
        assert_eq!(
            updated["document"]["product_forms"]["perfume-form"]["parameters"]["profile_tangents"]
                [1],
            0.2
        );

        let invalid = super::Scene3dFormRequest {
            parameters: Some(serde_json::json!({"height_mm":-1})),
            ..edit
        };
        assert!(super::plan_scene3d_form(&invalid).is_err());
        assert_eq!(
            invalid.document["product_forms"]["perfume-form"]["parameters"]["height_mm"],
            100
        );
    }

    #[test]
    fn browser_package_fold_validates_candidate_and_returns_undoable_document() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let request = super::Scene3dFoldRequest {
            document: document.clone(),
            dieline: "shirt-carton".into(),
            fold_state: 0.4,
        };
        let folded = super::plan_scene3d_fold(&request).unwrap();
        assert_eq!(
            folded["document"]["dielines"]["shirt-carton"]["fold_state"],
            0.4
        );
        assert_eq!(folded["document"]["pages"], document["pages"]);

        let invalid = super::Scene3dFoldRequest {
            fold_state: 1.01,
            ..request
        };
        assert!(super::plan_scene3d_fold(&invalid).is_err());
        assert_eq!(invalid.document, document);
    }

    #[test]
    fn browser_crease_angle_edit_updates_one_target_without_mutating_input() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let before = document.clone();
        let request = super::Scene3dCreaseAngleRequest {
            document,
            dieline: "shirt-carton".into(),
            crease: "front-right-fold".into(),
            angle_deg: 45.0,
        };
        let planned = super::plan_scene3d_crease_angle(&request).unwrap();
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["creases"][1]["angle_deg"],
            45.0
        );
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["fold_state"],
            1.0
        );
        assert_eq!(planned["result"]["previous_angle_deg"], 90.0);
        assert_eq!(request.document, before);

        let invalid = super::Scene3dCreaseAngleRequest {
            angle_deg: 181.0,
            ..request
        };
        assert!(super::plan_scene3d_crease_angle(&invalid).is_err());
        assert_eq!(invalid.document, before);
    }

    #[test]
    fn browser_crease_direction_edit_is_validated_and_failure_atomic() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let before = document.clone();
        let request = super::Scene3dCreaseDirectionRequest {
            document,
            dieline: "shirt-carton".into(),
            crease: "front-right-fold".into(),
            direction: "valley".into(),
        };
        let planned = super::plan_scene3d_crease_direction(&request).unwrap();
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["creases"][1]["direction"],
            "valley"
        );
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["creases"][1]["angle_deg"],
            90.0
        );
        assert_eq!(planned["result"]["previous_direction"], "mountain");
        assert_eq!(request.document, before);

        let invalid = super::Scene3dCreaseDirectionRequest {
            direction: "sideways".into(),
            ..request
        };
        assert!(super::plan_scene3d_crease_direction(&invalid).is_err());
        assert_eq!(invalid.document, before);
    }

    #[test]
    fn browser_pack_print_settings_preserve_layout_and_reject_out_of_range_values() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let before = document.clone();
        let request = super::Scene3dPackSettingsRequest {
            document,
            dieline: "shirt-carton".into(),
            board_mm: Some(1.2),
            bleed_mm: Some(5.0),
            seam_angle_deg: None,
            wrap_direction: None,
        };
        let planned = super::plan_scene3d_pack_settings(&request).unwrap();
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["dimensions_mm"]["board"],
            1.2
        );
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["dimensions_mm"]["bleed"],
            5.0
        );
        assert_eq!(planned["document"]["pages"], before["pages"]);
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["panels"],
            before["dielines"]["shirt-carton"]["panels"]
        );
        assert_eq!(request.document, before);

        let invalid = super::Scene3dPackSettingsRequest {
            board_mm: None,
            bleed_mm: Some(100.1),
            ..request
        };
        assert!(super::plan_scene3d_pack_settings(&invalid).is_err());
        assert_eq!(invalid.document, before);
    }

    #[test]
    fn browser_pack_wrap_settings_share_the_validated_cylindrical_mapping() {
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        crate::scene3d::pack::add_tube_label_with_overlap(
            &mut document,
            "tube-label",
            "tube-label-preview",
            "mockups",
            Some("products"),
            28.0,
            96.0,
            3.0,
            2.0,
            [25.0, 30.0, 260.0, 500.0],
        )
        .unwrap();
        let request = super::Scene3dPackSettingsRequest {
            document: document.clone(),
            dieline: "tube-label".into(),
            board_mm: None,
            bleed_mm: None,
            seam_angle_deg: Some(42.0),
            wrap_direction: Some("counterclockwise".into()),
        };
        let planned = super::plan_scene3d_pack_settings(&request).unwrap();
        assert_eq!(
            planned["document"]["dielines"]["tube-label"]["wrap"]["seam_angle_deg"],
            42.0
        );
        assert_eq!(
            planned["document"]["dielines"]["tube-label"]["wrap"]["direction"],
            "counterclockwise"
        );
        assert_eq!(request.document, document);

        let invalid = super::Scene3dPackSettingsRequest {
            seam_angle_deg: Some(361.0),
            ..request
        };
        assert!(super::plan_scene3d_pack_settings(&invalid).is_err());
        assert_eq!(invalid.document, document);
    }

    #[test]
    fn browser_package_preflight_is_read_only_and_reports_limits() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let before = document.clone();
        let request = super::Scene3dPackPreflightRequest {
            document,
            dieline: "shirt-carton".into(),
            min_ppi: 300.0,
        };
        let planned = super::plan_scene3d_pack_preflight(&request, &std::env::temp_dir()).unwrap();
        assert_eq!(planned["result"]["preflight_status"], "needs-review");
        assert_eq!(planned["result"]["manufacturing_certified"], false);
        assert_eq!(planned["result"]["minimum_ppi"], 300.0);
        assert_eq!(planned["result"]["pdfx_output_role"]["configured"], false);
        assert!(planned["result"]["pdfx_output_role"]["message"]
            .as_str()
            .unwrap()
            .contains("output color role is not configured"));
        assert_eq!(request.document, before);
        assert!(planned.get("document").is_none());
    }

    #[test]
    fn browser_face_pick_resolves_folded_package_panel_from_cpu_camera_ray() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let before = document.clone();
        let request = super::Scene3dUvFacePickRequest {
            document,
            page: "products".into(),
            node: "carton-preview".into(),
            primitive: None,
            x: 0.5,
            y: 0.5,
        };
        let picked = super::plan_scene3d_uv_face_pick(&request, &std::env::temp_dir()).unwrap();
        assert!(picked["primitive"].is_number(), "{picked}");
        assert!(picked["panel"].is_string(), "{picked}");
        assert!(request.document["dielines"]["shirt-carton"]["panels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|panel| panel["id"] == picked["panel"]));
        assert_eq!(request.document, before);
    }

    #[test]
    fn browser_package_face_preview_highlights_selected_panel_without_mutation() {
        use base64::Engine as _;
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let before = document.clone();
        let request = super::Scene3dPackFacePreviewRequest {
            document,
            page: "products".into(),
            node: "carton-preview".into(),
            panel: "front".into(),
        };
        let preview =
            super::plan_scene3d_pack_face_preview(&request, &std::env::temp_dir()).unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .decode(preview["png_base64"].as_str().unwrap())
            .unwrap();
        let page = request.document["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|page| page["id"] == request.page)
            .unwrap();
        let source_node = super::find_scene3d_value(page, &request.node).unwrap();
        let mut baseline_node = source_node.as_object().unwrap().clone();
        let width = baseline_node["width"].as_f64().unwrap();
        let height = baseline_node["height"].as_f64().unwrap();
        let scale = 768.0 / width.max(height);
        baseline_node.insert("width".into(), serde_json::json!(width * scale));
        baseline_node.insert("height".into(), serde_json::json!(height * scale));
        baseline_node.insert("pixel_ratio".into(), serde_json::json!(1.0));
        baseline_node.insert("supersample".into(), serde_json::json!(1));
        let baseline = crate::scene3d::node::render_png_in_page(
            &request.document,
            &std::env::temp_dir(),
            Some(&request.page),
            &baseline_node,
        )
        .unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_ne!(png, baseline, "selected panel should change preview pixels");
        assert_eq!(preview["panel"], "front");
        assert_eq!(preview["max_edge_px"], 768);
        assert_eq!(request.document, before);
    }

    #[test]
    fn browser_flat_pack_preview_uses_shared_export_and_does_not_return_a_document() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let before = document.clone();
        let request = super::Scene3dPackFlatPreviewRequest {
            document,
            dieline: "shirt-carton".into(),
            min_ppi: 300.0,
        };
        let planned =
            super::plan_scene3d_pack_flat_preview(&request, &std::env::temp_dir()).unwrap();
        assert!(planned["svg"]
            .as_str()
            .unwrap()
            .contains("data-artwork-panel=\"front\""));
        assert_eq!(planned["report"]["artwork_panels"][0], "front");
        assert_eq!(planned["report"]["manufacturing_certified"], false);
        assert_eq!(request.document, before);
        assert!(planned.get("document").is_none());

        let missing = super::Scene3dPackFlatPreviewRequest {
            dieline: "missing".into(),
            ..request
        };
        assert!(super::plan_scene3d_pack_flat_preview(&missing, &std::env::temp_dir()).is_err());
    }

    #[test]
    fn browser_sleeve_creation_builds_a_valid_linked_four_panel_preview() {
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        let layer = document["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let request = super::Scene3dSleeveRequest {
            document: document.clone(),
            dieline_id: "sleeve-1".into(),
            node_id: "sleeve-preview".into(),
            layer,
            page: Some(page),
            length_mm: 120.0,
            width_mm: 70.0,
            height_mm: 180.0,
            board_mm: 0.5,
            bleed_mm: 3.0,
            placement: [10.0, 10.0, 200.0, 240.0],
        };
        let planned = super::plan_scene3d_sleeve(&request).unwrap();
        let dieline = &planned["document"]["dielines"]["sleeve-1"];
        assert_eq!(dieline["template"], "sleeve");
        assert_eq!(dieline["panels"].as_array().unwrap().len(), 4);
        assert_eq!(dieline["creases"].as_array().unwrap().len(), 3);
        assert_eq!(dieline["scene3d_node"], "sleeve-preview");
        assert_eq!(
            planned["document"]["pages"][0]["layers"][0]["nodes"][0]["dieline"],
            "sleeve-1"
        );
        crate::transaction::validate_value(&planned["document"]).unwrap();

        let invalid = super::Scene3dSleeveRequest {
            length_mm: 0.0,
            ..request
        };
        assert!(super::plan_scene3d_sleeve(&invalid).is_err());
        assert_eq!(invalid.document, document);
    }

    #[test]
    fn browser_carton_creation_generates_valid_tuck_and_mailer_nets() {
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        let layer = document["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        for template in ["straight-tuck", "reverse-tuck", "mailer"] {
            let request = super::Scene3dCartonRequest {
                document: document.clone(),
                dieline_id: format!("{template}-die"),
                node_id: format!("{template}-preview"),
                layer: layer.clone(),
                page: Some(page.clone()),
                template: template.into(),
                length_mm: 120.0,
                width_mm: 70.0,
                height_mm: 180.0,
                board_mm: 0.5,
                bleed_mm: 3.0,
                glue_flap_mm: 15.0,
                placement: [10.0, 10.0, 200.0, 240.0],
            };
            let planned = super::plan_scene3d_carton(&request).unwrap();
            let dieline = &planned["document"]["dielines"][request.dieline_id.as_str()];
            assert_eq!(dieline["template"], template);
            let (panels, creases) = if template == "mailer" {
                (7, 6)
            } else {
                (13, 12)
            };
            assert_eq!(dieline["panels"].as_array().unwrap().len(), panels);
            assert_eq!(dieline["creases"].as_array().unwrap().len(), creases);
            assert_eq!(dieline["scene3d_node"], request.node_id);
            crate::transaction::validate_value(&planned["document"]).unwrap();
        }
    }

    #[test]
    fn packaging_path_role_planner_exports_drawn_cut_and_score_marks_without_folding_them() {
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        let layer = document["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let created = super::plan_scene3d_carton(&super::Scene3dCartonRequest {
            document,
            dieline_id: "role-test-die".into(),
            node_id: "role-test-preview".into(),
            layer,
            page: Some(page),
            template: "straight-tuck".into(),
            length_mm: 120.0,
            width_mm: 70.0,
            height_mm: 180.0,
            board_mm: 0.5,
            bleed_mm: 3.0,
            glue_flap_mm: 15.0,
            placement: [10.0, 10.0, 200.0, 240.0],
        })
        .unwrap();
        let mut source = created["document"].clone();
        let layout_page = source["dielines"]["role-test-die"]["layout_page"]
            .as_str()
            .unwrap()
            .to_owned();
        let page = source["pages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|page| page["id"] == layout_page)
            .unwrap();
        page["layers"][1]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "kind":"path","id":"user-cut-line","name":"New cut line",
                "d":"M 10 10 L 20 10 L 20 20 L 10 20 Z",
                "style":{"fill":{"fallback":"none"},"stroke":{"fallback":"#111111"}}
            }));

        let request = super::Scene3dPathRoleRequest {
            document: source.clone(),
            dieline: "role-test-die".into(),
            node: "user-cut-line".into(),
            role: "cut".into(),
        };
        let planned = super::plan_scene3d_path_role(&request).unwrap();
        assert_eq!(planned["result"]["spot"], "CutContour");
        let resolved =
            crate::scene3d::pack::dieline_from_document(&planned["document"], "role-test-die")
                .unwrap();
        assert!(resolved["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| { path["id"] == planned["result"]["path"] && path["role"] == "cut" }));
        let root = std::env::temp_dir().join(format!(
            "pentool-path-role-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let plates_dir = root.join("vector-plates");
        let plates = crate::scene3d::pack::write_dieline_vector_spot_plates(
            &planned["document"],
            "role-test-die",
            &plates_dir,
        )
        .unwrap();
        let cut_plate = plates["plates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|plate| plate["ink"] == "CutContour")
            .unwrap();
        let cut_svg =
            std::fs::read_to_string(plates_dir.join(cut_plate["path"].as_str().unwrap())).unwrap();
        assert!(cut_svg.contains("data-id=\"layout-"));
        assert!(cut_svg.contains("data-role=\"cut\""));

        let scored_document = planned["document"].clone();
        let layout_page = scored_document["dielines"]["role-test-die"]["layout_page"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut scored_source = scored_document;
        let score_page = scored_source["pages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|page| page["id"] == layout_page)
            .unwrap();
        score_page["layers"][1]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "kind":"path","id":"user-score-line","name":"Curved score",
                "d":"M 10 30 Q 20 20 30 30",
                "style":{"fill":{"fallback":"none"},"stroke":{"fallback":"#111111"}}
            }));
        let score_request = super::Scene3dPathRoleRequest {
            document: scored_source,
            dieline: "role-test-die".into(),
            node: "user-score-line".into(),
            role: "crease".into(),
        };
        let scored = super::plan_scene3d_path_role(&score_request).unwrap();
        assert_eq!(scored["result"]["spot"], "Crease");
        let scored_dieline =
            crate::scene3d::pack::dieline_from_document(&scored["document"], "role-test-die")
                .unwrap();
        assert_eq!(scored_dieline["creases"].as_array().unwrap().len(), 12);
        assert!(scored_dieline["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| { path["id"] == scored["result"]["path"] && path["role"] == "crease" }));
        let score_plates_dir = root.join("score-plates");
        let score_plates = crate::scene3d::pack::write_dieline_vector_spot_plates(
            &scored["document"],
            "role-test-die",
            &score_plates_dir,
        )
        .unwrap();
        let crease_plate = score_plates["plates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|plate| plate["ink"] == "Crease")
            .unwrap();
        let crease_svg =
            std::fs::read_to_string(score_plates_dir.join(crease_plate["path"].as_str().unwrap()))
                .unwrap();
        assert!(crease_svg.contains("data-id=\"layout-"));
        assert!(crease_svg.contains("data-role=\"crease\""));

        std::fs::remove_dir_all(&root).unwrap();

        let invalid = super::Scene3dPathRoleRequest {
            role: "hinge".into(),
            ..request
        };
        assert!(super::plan_scene3d_path_role(&invalid)
            .unwrap_err()
            .to_string()
            .contains("new packaging paths support"));
        assert_eq!(invalid.document, source);
    }

    #[test]
    fn packaging_path_crease_planner_promotes_only_exact_shared_panel_edges() {
        let v10 = crate::scene3d::node::migrate(crate::scene::new_document(320, 240)).unwrap();
        let document = crate::scene3d::node::migrate_to_v11(v10).unwrap();
        let page = document["pages"][0]["id"].as_str().unwrap().to_owned();
        let layer = document["pages"][0]["layers"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let created = super::plan_scene3d_carton(&super::Scene3dCartonRequest {
            document,
            dieline_id: "crease-test-die".into(),
            node_id: "crease-test-preview".into(),
            layer,
            page: Some(page),
            template: "straight-tuck".into(),
            length_mm: 120.0,
            width_mm: 70.0,
            height_mm: 180.0,
            board_mm: 0.5,
            bleed_mm: 3.0,
            glue_flap_mm: 15.0,
            placement: [10.0, 10.0, 200.0, 240.0],
        })
        .unwrap();
        let mut source = created["document"].clone();
        let dieline = &source["dielines"]["crease-test-die"];
        let old = dieline["creases"][0].clone();
        let layout_id = dieline["layout_page"].as_str().unwrap().to_owned();
        let old_node_id = format!("{layout_id}-crease-{}", old["id"].as_str().unwrap());
        let line = &old["line_mm"];
        let bleed = 3.0;
        let d = format!(
            "M {} {} L {} {}",
            line[0][0].as_f64().unwrap() + bleed,
            line[0][1].as_f64().unwrap() + bleed,
            line[1][0].as_f64().unwrap() + bleed,
            line[1][1].as_f64().unwrap() + bleed
        );
        source["dielines"]["crease-test-die"]["creases"]
            .as_array_mut()
            .unwrap()
            .remove(0);
        source["dielines"]["crease-test-die"]["paths"]
            .as_array_mut()
            .unwrap()
            .retain(|path| path["role"] != "crease");
        let layout = source["pages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|page| page["id"] == layout_id)
            .unwrap();
        let nodes = layout["layers"][1]["nodes"].as_array_mut().unwrap();
        nodes.retain(|node| node["id"] != old_node_id && node["dieline_path_role"] != "crease");
        nodes.push(serde_json::json!({
            "kind":"path","id":"authored-fold","name":"Authored fold",
            "d":d,"style":{"fill":{"fallback":"none"},"stroke":{"fallback":"#111"}}
        }));
        let request = super::Scene3dPathCreaseRequest {
            document: source.clone(),
            dieline: "crease-test-die".into(),
            node: "authored-fold".into(),
            crease: "authored-hinge".into(),
            angle_deg: 90.0,
            direction: "valley".into(),
        };
        let planned = super::plan_scene3d_path_crease(&request).unwrap();
        assert_eq!(planned["result"]["crease"], "authored-hinge");
        assert_eq!(
            planned["document"]["pages"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|p| p["layers"].as_array().unwrap())
                .flat_map(|l| l["nodes"].as_array().unwrap())
                .find(|n| n["id"] == "authored-fold")
                .unwrap()["dieline_crease_id"],
            "authored-hinge"
        );
        let resolved =
            crate::scene3d::pack::dieline_from_document(&planned["document"], "crease-test-die")
                .unwrap();
        assert!(resolved["creases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "authored-hinge"));
        let flat_svg = crate::scene3d::pack::flat_dieline_svg(&resolved).unwrap();
        assert!(flat_svg.contains("data-role=\"crease\""));
        assert!(flat_svg.contains("data-direction=\"valley\""));
        assert!(flat_svg.contains("data-angle-deg=\"90\""));
        let spot_dir = std::env::temp_dir().join(format!(
            "pentool-path-crease-plates-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let plates = crate::scene3d::pack::write_dieline_vector_spot_plates(
            &planned["document"],
            "crease-test-die",
            &spot_dir,
        )
        .unwrap();
        let crease_plate = plates["plates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|plate| plate["ink"] == "Crease")
            .unwrap();
        let crease_svg =
            std::fs::read_to_string(spot_dir.join(crease_plate["path"].as_str().unwrap())).unwrap();
        assert!(crease_svg.contains("data-id=\"structural-crease-authored-hinge\""));
        std::fs::remove_dir_all(spot_dir).unwrap();

        let mut raster_doc: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        raster_doc["dielines"]["shirt-carton"]["paths"]
            .as_array_mut()
            .unwrap()
            .retain(|path| path["role"] != "crease");
        let (separations, _) = crate::scene3d::pack::dieline_spot_separations(
            &raster_doc,
            std::path::Path::new("packaging.pen"),
            "shirt-carton",
            36.0,
        )
        .unwrap();
        let crease_separation = separations
            .plates
            .iter()
            .find(|plate| plate.spot && plate.id == "Crease")
            .unwrap();
        assert!(crease_separation.samples.iter().any(|sample| *sample > 0.0));
        crate::scene3d::pack::generate(&resolved).unwrap();

        let mut invalid = source;
        let layout = invalid["pages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|page| page["id"] == layout_id)
            .unwrap();
        layout["layers"][1]["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|n| n["id"] == "authored-fold")
            .unwrap()["d"] = "M 5 5 L 15 5".into();
        let failed = super::Scene3dPathCreaseRequest {
            document: invalid.clone(),
            ..request
        };
        assert!(super::plan_scene3d_path_crease(&failed)
            .unwrap_err()
            .to_string()
            .contains("not exactly a shared edge"));
        assert_eq!(failed.document, invalid);
    }

    #[test]
    fn tube_label_planner_uses_the_measured_circumference_and_preserves_input() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let request = super::Scene3dTubeLabelRequest {
            document: document.clone(),
            dieline_id: "tube-label".into(),
            node_id: "tube-preview".into(),
            layer: "mockups".into(),
            page: Some("products".into()),
            radius_mm: 32.0,
            height_mm: 95.0,
            bleed_mm: 3.0,
            overlap_mm: 4.0,
            placement: [10.0, 10.0, 200.0, 240.0],
        };
        let planned = super::plan_scene3d_tube_label(&request).unwrap();
        let dieline = &planned["document"]["dielines"]["tube-label"];
        assert_eq!(dieline["template"], "tube");
        assert_eq!(dieline["wrap"]["radius_mm"], 32.0);
        assert_eq!(dieline["panels"][0]["face_slot"], "label");
        let flat_width = dieline["panels"][0]["rect_mm"][2].as_f64().unwrap();
        assert!((flat_width - (std::f64::consts::TAU * 32.0 + 4.0)).abs() < 1e-4);
        assert_eq!(dieline["wrap"]["overlap_mm"], 4.0);
        crate::transaction::validate_value(&planned["document"]).unwrap();

        let invalid = super::Scene3dTubeLabelRequest {
            radius_mm: 0.0,
            ..request
        };
        assert!(super::plan_scene3d_tube_label(&invalid).is_err());
        assert_eq!(invalid.document, document);
    }

    #[test]
    fn browser_pouch_and_bottle_label_creation_use_validated_planners() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let pouch = super::Scene3dPouchRequest {
            document: document.clone(),
            dieline_id: "pouch-1".into(),
            node_id: "pouch-preview".into(),
            layer: "mockups".into(),
            page: Some("products".into()),
            front_width_mm: 120.0,
            gusset_mm: 35.0,
            height_mm: 180.0,
            bleed_mm: 3.0,
            placement: [10.0, 10.0, 200.0, 240.0],
        };
        let planned = super::plan_scene3d_pouch(&pouch).unwrap();
        assert_eq!(
            planned["document"]["dielines"]["pouch-1"]["template"],
            "pouch"
        );
        assert_eq!(
            planned["document"]["dielines"]["pouch-1"]["panels"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        crate::transaction::validate_value(&planned["document"]).unwrap();
        let invalid = super::Scene3dPouchRequest {
            gusset_mm: 0.0,
            ..pouch
        };
        assert!(super::plan_scene3d_pouch(&invalid).is_err());
        assert_eq!(invalid.document, document);

        let bottle_label = super::Scene3dBottleLabelRequest {
            document: document.clone(),
            dieline_id: "bottle-label-1".into(),
            node_id: "bottle-label-preview".into(),
            layer: "mockups".into(),
            page: Some("products".into()),
            radius_mm: 30.0,
            height_mm: 90.0,
            bleed_mm: 3.0,
            overlap_mm: 2.0,
            placement: [10.0, 10.0, 200.0, 240.0],
        };
        let planned = super::plan_scene3d_bottle_label(&bottle_label).unwrap();
        assert_eq!(
            planned["document"]["dielines"]["bottle-label-1"]["template"],
            "bottle-label"
        );
        assert_eq!(
            planned["document"]["dielines"]["bottle-label-1"]["panels"][0]["id"],
            "bottle-label"
        );
        crate::transaction::validate_value(&planned["document"]).unwrap();
        let invalid = super::Scene3dBottleLabelRequest {
            overlap_mm: 1000.0,
            ..bottle_label
        };
        assert!(super::plan_scene3d_bottle_label(&invalid).is_err());
        assert_eq!(invalid.document, document);
    }

    #[test]
    fn panel_art_binding_is_validated_and_clears_without_mutating_the_source() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let request = super::Scene3dPanelArtRequest {
            document: document.clone(),
            dieline: "shirt-carton".into(),
            panel: "back".into(),
            source: Some(serde_json::json!({"kind":"page","page":"artwork"})),
        };
        let planned = super::plan_scene3d_panel_art(&request).unwrap();
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["panels"][1]["artwork"]["page"],
            "artwork"
        );

        let invalid = super::Scene3dPanelArtRequest {
            source: Some(serde_json::json!({"kind":"page","page":"missing-page"})),
            ..request
        };
        assert!(super::plan_scene3d_panel_art(&invalid).is_err());
        assert_eq!(invalid.document, document);

        let mut cleared_doc = planned["document"].clone();
        let cleared = crate::scene3d::bindings::set_panel_artwork(
            &mut cleared_doc,
            "shirt-carton",
            "back",
            None,
        )
        .unwrap();
        assert_eq!(cleared["artwork"], serde_json::Value::Null);
        assert!(cleared_doc["dielines"]["shirt-carton"]["panels"][1]
            .get("artwork")
            .is_none());
    }

    #[test]
    fn panel_safe_area_planner_is_validated_and_failure_atomic() {
        let document: serde_json::Value =
            serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        let request = super::Scene3dPanelSafeZoneRequest {
            document: document.clone(),
            dieline: "shirt-carton".into(),
            panel: "front".into(),
            inset_mm: 12.0,
        };
        let planned = super::plan_scene3d_panel_safe_zone(&request).unwrap();
        assert_eq!(
            planned["document"]["dielines"]["shirt-carton"]["panels"][0]["safe_zone_mm"],
            12.0
        );
        assert_eq!(planned["result"]["safe_zone_mm"], 12.0);
        assert!(request.document["dielines"]["shirt-carton"]["panels"][0]
            .get("safe_zone_mm")
            .is_none());

        let invalid = super::Scene3dPanelSafeZoneRequest {
            inset_mm: 131.0,
            ..request
        };
        assert!(super::plan_scene3d_panel_safe_zone(&invalid).is_err());
        assert!(invalid.document["dielines"]["shirt-carton"]["panels"][0]
            .get("safe_zone_mm")
            .is_none());
    }

    #[tokio::test]
    async fn scene3d_cache_endpoint_reports_and_clears_process_local_entries() {
        let _cache_guard = crate::scene3d::node::PNG_CACHE_TEST_LOCK.lock().await;
        let status = super::scene3d_cache_status().await;
        let body = axum::body::to_bytes(status.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status["max_bytes"], 64 * 1024 * 1024);
        assert_eq!(status["scope"], "process");
        assert!(status["bytes"].as_u64().unwrap() <= status["max_bytes"].as_u64().unwrap());
        assert!(status["hits"].as_u64().is_some());
        assert!(status["misses"].as_u64().is_some());
        assert!(status["evictions"].as_u64().is_some());

        let cleared = super::scene3d_cache_clear().await;
        let body = axum::body::to_bytes(cleared.into_body(), usize::MAX)
            .await
            .unwrap();
        let cleared: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(cleared["cleared_entries"].as_u64().is_some());
        assert!(cleared["cleared_bytes"].as_u64().is_some());
        assert!(cleared["cleared_hits"].as_u64().is_some());
        assert!(cleared["cleared_misses"].as_u64().is_some());
        assert!(cleared["cleared_evictions"].as_u64().is_some());
    }

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
        assert!(super::composite_preview_scale(&raw, None, Some(8193)).is_err());
        // Zoomed previews may exceed the canvas size, within 8x and the pixel budget.
        assert_eq!(
            super::preview_scale(1200.0, 800.0, Some(2400), 0.1).unwrap(),
            2.0
        );
        assert_eq!(
            super::preview_scale(100.0, 100.0, Some(8192), 0.1).unwrap(),
            8.0
        );
        let budget = super::preview_scale(8000.0, 8000.0, Some(8192), 0.1).unwrap();
        assert!(
            8000.0 * 8000.0 * f64::from(budget).powi(2)
                <= crate::preview::PREVIEW_MAX_PIXELS * 1.0001
        );
        assert_eq!(super::preview_scale(1200.0, 800.0, None, 0.1).unwrap(), 1.0);
        // The largest zoomed preview of an ordinary page exceeds the compositing
        // budget; the preview steps down instead of failing, exports do not.
        let page = crate::composite::migrate(crate::scene::new_document(1200, 800)).unwrap();
        let path = std::path::Path::new("target/proxy.pen");
        let big = super::composite_preview_scale(&page, None, Some(8192)).unwrap();
        assert!(crate::composite::png(&page, path, None, big).is_err());
        let png = super::composite_png(&page, path, None, big, true).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert!(decoded.width() >= 2048 && decoded.width() < 8192);
        assert!(super::composite_png(&page, path, None, big, false).is_err());
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

#[derive(serde::Deserialize)]
struct LayoutRequest {
    document: serde_json::Value,
    op: LayoutOp,
}

/// One layout read or edit applied to the in-browser document. Edits run the
/// same planners as the CLI (`story`, `frame`, `page`, `spread`) so the editor
/// never keeps a second layout model.
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum LayoutOp {
    Overview,
    Preflight {
        bleed: Option<f64>,
        max_overrides: Option<f64>,
        #[serde(default)]
        no_alt: bool,
    },
    Story {
        action: crate::stories::StoryAction,
    },
    Frame {
        action: crate::stories::FrameAction,
    },
    PageAdd {
        id: String,
        after: Option<String>,
        parent: Option<String>,
    },
    PageDuplicate {
        id: String,
        new_id: String,
    },
    PageRemove {
        id: String,
        #[serde(default)]
        drop_story: bool,
    },
    PageMove {
        id: String,
        index: usize,
    },
    PageRename {
        id: String,
        name: String,
    },
    PageParent {
        page: String,
        parent: Option<String>,
    },
    SpreadFacing {
        spread: String,
        facing: bool,
    },
    Table {
        action: crate::tables::TableAction,
    },
    Wrap {
        action: crate::wrap::WrapAction,
    },
    Style {
        action: crate::styles::StyleAction,
    },
    Grid {
        action: crate::grids::GridAction,
    },
    Doc {
        action: crate::longdoc::DocAction,
    },
}

/// `POST /api/layout`: `{document, op}` -> `{document, result}`. Read-only ops
/// omit `document`. Nothing is written to disk; the client commits with Save.
async fn layout_command(State(state): State<Shared>, Json(body): Json<LayoutRequest>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        use crate::page::PageAction;
        let mut raw = body.document;
        let base = state
            .file
            .clone()
            .unwrap_or_else(|| state.project_root.join("browser.pen"));
        let page_edit =
            |raw: &mut serde_json::Value, action| crate::spreads::apply_page(raw, action);
        let result = match body.op {
            LayoutOp::Overview => {
                return crate::layout_preflight::overview(&raw)
                    .map(|r| serde_json::json!({"result": r}));
            }
            LayoutOp::Preflight {
                bleed,
                max_overrides,
                no_alt,
            } => {
                let defaults = crate::layout_preflight::Options::default();
                let report = crate::layout_preflight::run(
                    &raw,
                    &base,
                    &crate::layout_preflight::Options {
                        bleed: bleed.unwrap_or(defaults.bleed),
                        max_override_pct: max_overrides.unwrap_or(defaults.max_override_pct),
                        require_alt: !no_alt,
                    },
                )?;
                return Ok(serde_json::json!({"result": report}));
            }
            LayoutOp::Story { action } => crate::stories::run_story(&mut raw, action)?,
            LayoutOp::Frame { action } => crate::stories::run_frame(&mut raw, action)?,
            LayoutOp::PageAdd { id, after, parent } => page_edit(
                &mut raw,
                PageAction::Add {
                    id,
                    name: None,
                    width: None,
                    height: None,
                    background: None,
                    after,
                    before: None,
                    parent,
                    count: None,
                },
            )?,
            LayoutOp::PageDuplicate { id, new_id } => {
                page_edit(&mut raw, PageAction::Duplicate { id, new_id })?
            }
            LayoutOp::PageRemove { id, drop_story } => {
                page_edit(&mut raw, PageAction::Remove { id, drop_story })?
            }
            LayoutOp::PageMove { id, index } => {
                page_edit(&mut raw, PageAction::Move { id, index })?
            }
            LayoutOp::PageRename { id, name } => {
                page_edit(&mut raw, PageAction::Rename { id, name })?
            }
            LayoutOp::PageParent { page, parent } => {
                crate::spreads::page_set_parent(&mut raw, &page, parent.as_deref())?
            }
            LayoutOp::SpreadFacing { spread, facing } => {
                crate::spreads::spread_set_facing(&mut raw, &spread, facing)?
            }
            LayoutOp::Table { action } => crate::tables::run(&mut raw, action)?.0,
            LayoutOp::Wrap { action } => crate::wrap::run(&mut raw, action)?.0,
            LayoutOp::Style { action } => crate::styles::run_style(&mut raw, action)?.0,
            LayoutOp::Grid { action } => crate::grids::run(&mut raw, action)?.0,
            LayoutOp::Doc { action } => {
                let ctx = crate::longdoc::Context {
                    file_name: base
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned()),
                };
                crate::longdoc::run(&mut raw, action, &ctx)?.0
            }
        };
        crate::transaction::validate_value(&raw)?;
        Ok(serde_json::json!({"document": raw, "result": result}))
    })();
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}

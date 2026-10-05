use crate::{document::Document, image, render};
use anyhow::{Context, Result};
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
}

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE: &str = include_str!("../web/style.css");
const IMAGE_PANEL_JS: &str = include_str!("../web/image-panel.js");

pub async fn serve(host: &str, port: u16, file: Option<PathBuf>) -> Result<()> {
    if let Some(path) = &file {
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        crate::transaction::validate_value(&value)?;
    }
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .context("invalid host or port")?;
    let app = Router::new()
        .route("/", get(|| async { Html(INDEX) }))
        .route(
            "/app.js",
            get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }),
        )
        .route(
            "/image-panel.js",
            get(|| async { asset(IMAGE_PANEL_JS, "text/javascript; charset=utf-8") }),
        )
        .route("/api/image/bake", post(image_bake))
        .route(
            "/style.css",
            get(|| async { asset(STYLE, "text/css; charset=utf-8") }),
        )
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
        Ok(crate::library::search(
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
        ))
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
    operations: Vec<serde_json::Value>,
    page: Option<String>,
    revision: Option<String>,
    #[serde(default)]
    dry_run: bool,
}
async fn scene_command(State(state): State<Shared>, Json(body): Json<SceneRequest>) -> Response {
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
        if body
            .revision
            .as_deref()
            .is_some_and(|wanted| wanted != revision(&raw))
        {
            anyhow::bail!("document revision changed")
        };
        let changes = crate::scene::apply_batch(&mut raw, body.page.as_deref(), &body.operations)?;
        let change =
            crate::transaction::commit_value(&file, "browser-scene", body.dry_run, None, &raw)?;
        Ok(serde_json::json!({"change":change,"changes":changes}))
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

fn asset(body: &'static str, kind: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, kind),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

#[derive(serde::Deserialize, Default)]
struct PageQuery {
    page: Option<String>,
    max_edge: Option<u32>,
}

async fn render_png(
    State(state): State<Shared>,
    axum::extract::Query(query): axum::extract::Query<PageQuery>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    let result = (|| -> Result<Vec<u8>> {
        if value.get("version").and_then(serde_json::Value::as_u64) == Some(image::VERSION) {
            let document = state
                .file
                .clone()
                .unwrap_or_else(|| state.project_root.join("browser.pen"));
            let scene = image::to_svg(&value, &document, query.page.as_deref())?;
            return render::svg_to_png(&scene.svg, scene.width, scene.height, 1.0);
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
        render::to_png(&doc, 1.0)
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
    let max_edge = query.max_edge;
    let result = (|| -> Result<String> {
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
    fn revision_tracks_disk_values_without_a_browser_round_trip() {
        let original: serde_json::Value = serde_json::from_str(r#"{"x":100.0}"#).unwrap();
        let browser: serde_json::Value = serde_json::from_str(r#"{"x":100}"#).unwrap();
        assert_ne!(original, browser);
        let loaded_token = super::revision(&original);
        assert_eq!(loaded_token, super::revision(&original));
        assert_ne!(loaded_token, super::revision(&browser));
        assert_ne!(loaded_token, super::revision(&serde_json::json!({"x":101})));
    }
}

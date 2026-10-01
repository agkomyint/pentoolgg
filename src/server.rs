use crate::{document::Document, render};
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
    gate: Arc<Mutex<()>>,
}

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE: &str = include_str!("../web/style.css");

pub async fn serve(host: &str, port: u16, file: Option<PathBuf>) -> Result<()> {
    if let Some(path) = &file {
        let doc: Document = serde_json::from_slice(&std::fs::read(path)?)?;
        doc.validate().map_err(anyhow::Error::msg)?;
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
        .route("/api/geometry", post(geometry_command))
        .route("/api/text", post(text_command))
        .route("/api/fonts", get(font_list))
        .route("/api/font", post(embed_font))
        .route("/api/font-info", post(font_info))
        .route("/fonts/:name", get(bundled_font))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .with_state(Shared {
            file,
            gate: Arc::new(Mutex::new(())),
        });
    println!("Pentool listening on http://{address}");
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct TextRequest {
    document: serde_json::Value,
    action: crate::text::TextAction,
}
async fn text_command(Json(body): Json<TextRequest>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let mut doc: Document = serde_json::from_value(body.document.clone())?;
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
    layer: String,
    id: Option<String>,
    operation: crate::geometry::Operation,
}
async fn geometry_command(Json(body): Json<GeometryRequest>) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        let mut doc: Document = serde_json::from_value(body.document.clone())?;
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
        let doc: Document = serde_json::from_value(value.clone())?;
        doc.validate().map_err(anyhow::Error::msg)?;
        std::fs::write(&file, serde_json::to_vec_pretty(value)?)?;
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

async fn render_png(Json(doc): Json<Document>) -> Response {
    match render::to_png(&doc, 1.0) {
        Ok(bytes) => binary(bytes, "image/png", "artwork.png"),
        Err(error) => problem(error),
    }
}

async fn render_svg(Json(doc): Json<Document>) -> Response {
    match render::to_svg(&doc) {
        Ok(svg) => binary(svg.into_bytes(), "image/svg+xml", "artwork.svg"),
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

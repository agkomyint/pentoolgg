//! Server-enforced roles (spec section 9.1): route classification and the
//! request guard.
//!
//! Every route the server registers must be listed in [`ROUTES`]. A route that
//! is not listed is denied with `permission-denied` (deny by default), and a
//! unit test fails when `server.rs` registers a route or method that this table
//! does not classify.
use super::{
    sync::Ctx,
    token::{self, AuthError, Principal, Role},
};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use std::{net::SocketAddr, sync::Arc};

/// What a request needs before any handler runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Static UI assets and the health probe: no token.
    Public,
    Role(Role),
}

struct Entry {
    path: &'static str,
    /// Needed for GET and HEAD.
    read: Option<Need>,
    /// Needed for POST, PUT, PATCH and DELETE.
    write: Option<Need>,
}

const PUB: Option<Need> = Some(Need::Public);
const VIEW: Option<Need> = Some(Need::Role(Role::Viewer));
const COMMENT: Option<Need> = Some(Need::Role(Role::Commenter));
const EDIT: Option<Need> = Some(Need::Role(Role::Editor));
const OWNER: Option<Need> = Some(Need::Role(Role::Owner));
const NONE: Option<Need> = None;

const fn e(path: &'static str, read: Option<Need>, write: Option<Need>) -> Entry {
    Entry { path, read, write }
}

/// The classification of every server route. Read-only POST routes (renders,
/// analysis, previews) are viewer-level; everything else that can change the
/// document, the project or the filesystem needs `editor` or `owner`.
static ROUTES: &[Entry] = &[
    e("/", PUB, NONE),
    e("/studio", PUB, NONE),
    e("/core/pentool_core.wasm", PUB, NONE),
    e("/api/health", PUB, NONE),
    e("/fonts/:name", PUB, NONE),
    // reads that take a POST body but never touch the document or project
    e("/api/render/png", NONE, VIEW),
    e("/api/render/svg", NONE, VIEW),
    e("/api/font-info", NONE, VIEW),
    e("/api/composite/analyze", NONE, VIEW),
    e("/api/composite/cancel", NONE, VIEW),
    e("/api/photo/preview", NONE, VIEW),
    // reads
    e("/api/composite/dependencies", VIEW, NONE),
    e("/api/color/preflight", VIEW, NONE),
    e("/api/color/plates", VIEW, NONE),
    e("/api/color/proof", VIEW, NONE),
    e("/api/color/gamut", VIEW, NONE),
    e("/api/color/pdfx", VIEW, NONE),
    e("/api/layout/preflight", VIEW, NONE),
    e("/api/layout/overview", VIEW, NONE),
    e("/api/layout/export", VIEW, VIEW),
    e("/api/photo/catalog", VIEW, NONE),
    e("/api/photo/detail", VIEW, NONE),
    e("/api/design/tokens", VIEW, NONE),
    e("/api/design/library/status", VIEW, NONE),
    e("/api/design/audit", VIEW, NONE),
    e("/api/design/templates", VIEW, NONE),
    e("/api/design/template/check", VIEW, NONE),
    e("/api/extensions", VIEW, NONE),
    e("/api/history", VIEW, NONE),
    e("/api/fonts", VIEW, NONE),
    e("/api/assets", VIEW, NONE),
    e("/api/asset", VIEW, NONE),
    e("/api/registry/search", VIEW, NONE),
    e("/api/scene3d/status", VIEW, NONE),
    // read, and comment
    e("/api/comments", VIEW, COMMENT),
    // read, and mutate the document
    e("/api/document", VIEW, EDIT),
    e("/api/scene3d/cache", VIEW, EDIT),
    // mutate the document or project assets
    e("/api/raster", NONE, EDIT),
    e("/api/paint/plan", NONE, EDIT),
    e("/api/paint/score", NONE, EDIT),
    e("/api/paint/palette", NONE, EDIT),
    e("/api/paint/compare", NONE, EDIT),
    e("/api/paint/preview", NONE, EDIT),
    e("/api/paint/field", NONE, EDIT),
    e("/api/paint/media", NONE, EDIT),
    e("/api/image/bake", NONE, EDIT),
    e("/api/image/add", NONE, EDIT),
    e("/api/migrate", NONE, EDIT),
    e("/api/scene3d/import", NONE, EDIT),
    e("/api/scene3d/render-report", NONE, EDIT),
    e("/api/scene3d/form", NONE, EDIT),
    e("/api/scene3d/fold", NONE, EDIT),
    e("/api/scene3d/pack/settings", NONE, EDIT),
    e("/api/scene3d/crease-angle", NONE, EDIT),
    e("/api/scene3d/crease-direction", NONE, EDIT),
    e("/api/scene3d/pack/preflight", NONE, EDIT),
    e("/api/scene3d/pack/flat-preview", NONE, EDIT),
    e("/api/scene3d/pack/export-bundle", NONE, EDIT),
    e("/api/scene3d/pack/face-preview", NONE, EDIT),
    e("/api/scene3d/sleeve", NONE, EDIT),
    e("/api/scene3d/carton", NONE, EDIT),
    e("/api/scene3d/tube-label", NONE, EDIT),
    e("/api/scene3d/pouch", NONE, EDIT),
    e("/api/scene3d/bottle-label", NONE, EDIT),
    e("/api/scene3d/panel-art", NONE, EDIT),
    e("/api/scene3d/panel-art-transform", NONE, EDIT),
    e("/api/scene3d/panel-safe-zone", NONE, EDIT),
    e("/api/scene3d/path-role", NONE, EDIT),
    e("/api/scene3d/path-crease", NONE, EDIT),
    e("/api/scene3d/uv/preview", NONE, EDIT),
    e("/api/scene3d/uv/model-preview", NONE, EDIT),
    e("/api/scene3d/uv/face-pick", NONE, EDIT),
    e("/api/scene3d/uv/pack", NONE, EDIT),
    e("/api/layout", NONE, EDIT),
    e("/api/photo/import", NONE, EDIT),
    e("/api/photo/place", NONE, EDIT),
    e("/api/photo/edit", NONE, EDIT),
    e("/api/scene", NONE, EDIT),
    e("/api/design/template/fill", NONE, EDIT),
    e("/api/undo", NONE, EDIT),
    e("/api/redo", NONE, EDIT),
    e("/api/geometry", NONE, EDIT),
    e("/api/text", NONE, EDIT),
    e("/api/edit", NONE, EDIT),
    e("/api/import", NONE, EDIT),
    e("/api/font", NONE, EDIT),
    // run code or install into the project: owner only
    e("/api/extensions/run", NONE, OWNER),
    e("/api/registry/install", NONE, OWNER),
    // collaboration
    e("/api/collab/session", VIEW, NONE),
    e("/api/collab/genesis", VIEW, NONE),
    e("/api/collab/events", VIEW, NONE),
    e("/api/collab/ops", VIEW, EDIT),
    e("/api/collab/claim", NONE, EDIT),
    e("/api/collab/release", NONE, EDIT),
    e("/api/collab/tokens", OWNER, OWNER),
    e("/api/collab/tokens/revoke", NONE, OWNER),
    e("/api/collab/tokens/role", NONE, OWNER),
];

/// Classify a request. `None` means unclassified, which is denied.
pub fn classify(method: &str, path: &str) -> Option<Need> {
    let entry = ROUTES.iter().find(|entry| {
        entry.path == path
            || entry
                .path
                .strip_suffix(":name")
                .is_some_and(|prefix| path.starts_with(prefix) && path.len() > prefix.len())
    })?;
    match method {
        "GET" | "HEAD" => entry.read,
        "POST" | "PUT" | "PATCH" | "DELETE" => entry.write,
        _ => None,
    }
}

/// Route registrations found in a `server.rs`-shaped source: `(path, methods)`.
/// Used by the "no unclassified route" test.
pub fn scan_routes(source: &str) -> Vec<(String, Vec<String>)> {
    let source = source.split("#[cfg(test)]").next().unwrap_or(source);
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = source[at..].find(".route(") {
        let mut i = at + found + ".route(".len();
        at = i;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'"') {
            continue;
        }
        let start = i + 1;
        i = start;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        let path = source[start..i].to_string();
        i += 1;
        let mut depth = 1usize;
        let mut methods = Vec::new();
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'"' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                b'(' => depth += 1,
                b')' => depth -= 1,
                c if depth == 1 && c.is_ascii_lowercase() => {
                    let word_start = i;
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    let word = &source[word_start..i];
                    let before = word_start.checked_sub(1).map(|p| bytes[p]);
                    let boundary = !before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_');
                    if boundary
                        && bytes.get(i) == Some(&b'(')
                        && matches!(word, "get" | "post" | "put" | "patch" | "delete")
                    {
                        methods.push(word.to_uppercase());
                    }
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
        out.push((path, methods));
    }
    out
}

/// Routes (and methods) in `source` that [`classify`] does not cover.
pub fn unclassified_routes(source: &str) -> Vec<String> {
    let mut missing = Vec::new();
    for (path, methods) in scan_routes(source) {
        if methods.is_empty() {
            missing.push(format!("{path} (no method found)"));
        }
        for method in methods {
            if classify(&method, &path.replace(":name", "x")).is_none() {
                missing.push(format!("{method} {path}"));
            }
        }
    }
    missing
}

pub fn problem(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    let mut response = (
        status,
        Json(serde_json::json!({"error": format!("[{code}] {}", message.into()), "code": code})),
    )
        .into_response();
    if status == StatusCode::UNAUTHORIZED {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    response
}

fn loopback_host(host: &str) -> bool {
    let name = host
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_else(|| host.split(':').next().unwrap_or(host));
    matches!(name, "localhost" | "127.0.0.1" | "::1")
}

/// Clears the per-request actor override even if the request future is dropped.
struct ActorScope;
impl Drop for ActorScope {
    fn drop(&mut self) {
        super::set_request_actor(None);
    }
}

fn bearer(req: &Request) -> Option<Result<&str, ()>> {
    let value = req.headers().get(header::AUTHORIZATION)?;
    Some(
        value
            .to_str()
            .ok()
            .and_then(|text| text.strip_prefix("Bearer "))
            .map(str::trim)
            .ok_or(()),
    )
}

/// Authenticate and authorize every request before any handler runs.
pub async fn guard(State(ctx): State<Arc<Ctx>>, mut req: Request, next: Next) -> Response {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let Some(need) = classify(&method, &path) else {
        return problem(
            StatusCode::FORBIDDEN,
            "permission-denied",
            format!("{method} {path} is not a classified route; it is denied while --collab is on"),
        );
    };
    let Need::Role(required) = need else {
        return next.run(req).await;
    };
    let principal = match bearer(&req) {
        Some(Ok(secret)) => match token::authenticate(&ctx.dir, secret) {
            Ok(principal) => principal,
            Err(error) => {
                return problem(
                    StatusCode::UNAUTHORIZED,
                    error.code(),
                    match error {
                        AuthError::Expired => "the token has expired; ask the owner for a new one",
                        AuthError::Revoked => "the token was revoked",
                        _ => "the token is not valid",
                    },
                )
            }
        },
        Some(Err(())) => {
            return problem(
                StatusCode::UNAUTHORIZED,
                "invalid-token",
                "send the token as `Authorization: Bearer TOKEN`",
            )
        }
        None => match local_owner(&ctx, &req) {
            Some(principal) => principal,
            None => {
                return problem(
                    StatusCode::UNAUTHORIZED,
                    AuthError::Missing.code(),
                    "this server requires a token: send `Authorization: Bearer TOKEN`",
                )
            }
        },
    };
    if principal.role < required {
        return problem(
            StatusCode::FORBIDDEN,
            "permission-denied",
            format!(
                "{method} {path} needs the {} role; this token is {}",
                required.name(),
                principal.role.name()
            ),
        );
    }
    let writes = !matches!(method.as_str(), "GET" | "HEAD");
    let attributed = writes && principal.token.is_some();
    req.extensions_mut().insert(principal.clone());
    if !attributed {
        return next.run(req).await;
    }
    // Mutations run one at a time so the document's operation log attributes
    // them to the token's actor, never to the server's own identity.
    let _turn = ctx.actor_gate.lock().await;
    let director = principal
        .actor
        .directed_by
        .as_ref()
        .map(|id| super::op::Actor {
            id: id.clone(),
            kind: "human".into(),
            display_name: None,
            tool: None,
            model: None,
            directed_by: None,
        });
    super::set_request_actor(Some((principal.actor.clone(), director)));
    let _scope = ActorScope;
    next.run(req).await
}

/// The host's own browser on a loopback server, when tokens are not required.
fn local_owner(ctx: &Ctx, req: &Request) -> Option<Principal> {
    if !ctx.local_owner {
        return None;
    }
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>()?.0;
    let host = req.headers().get(header::HOST)?.to_str().ok()?;
    let proxied = req.headers().contains_key("x-forwarded-for")
        || req.headers().contains_key("x-forwarded-host")
        || req.headers().contains_key(header::FORWARDED);
    if !peer.ip().is_loopback() || !loopback_host(host) || proxied {
        return None;
    }
    let (actor, _) = super::current_actor().ok()?;
    Some(Principal {
        token: None,
        role: Role::Owner,
        actor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_server_route_is_classified() {
        let source = include_str!("../server.rs");
        let routes = scan_routes(source);
        assert!(routes.len() > 80, "route scan found {}", routes.len());
        assert_eq!(unclassified_routes(source), Vec::<String>::new());
    }

    #[test]
    fn unknown_routes_and_methods_are_denied() {
        assert_eq!(classify("GET", "/api/never-heard-of-it"), None);
        assert_eq!(classify("POST", "/api/health"), None);
        assert_eq!(
            classify("DELETE", "/api/document"),
            Some(Need::Role(Role::Editor))
        );
        assert_eq!(classify("TRACE", "/api/document"), None);
        assert_eq!(classify("GET", "/fonts/"), None);
    }

    #[test]
    fn scanner_sees_chained_methods() {
        let found = scan_routes(
            "x.route(\"/a\", get(h).put(h2))\n.route(\n \"/b\",\n post(|| async { (\"ok\") }),\n)",
        );
        assert_eq!(found[0], ("/a".into(), vec!["GET".into(), "PUT".into()]));
        assert_eq!(found[1], ("/b".into(), vec!["POST".into()]));
    }
}

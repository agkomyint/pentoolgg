//! Collaboration sync: two instances converging over HTTP, hostile peers,
//! server-enforced roles on every route, crash-safe resume and the guarantee
//! that a server without `--collab` is unchanged.
use pentool::collab::perm::{classify, unclassified_routes, Need};
use pentool::collab::sync::{inject_fault, sync_with_peer, Fault};
use pentool::collab::token::Role;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

// ------------------------------------------------------------ test helpers

struct Workspace(PathBuf);
impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pentool-sync-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .env_remove("PENTOOL_COLLAB_TOKEN")
        .output()
        .unwrap()
}

fn ok_json(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?} printed non-JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn put_shape(id: &str) -> Value {
    json!([{"type":"put-shape","shape":"rect","id":id,"layer":"content","x":1,"y":1,"width":2,"height":2}])
}

fn edit(ws: &Workspace, doc: &str, id: &str, actor: &[&str]) {
    let file = ws.path(&format!("{id}.json"));
    fs::write(&file, put_shape(id).to_string()).unwrap();
    let mut args = vec!["batch", doc, file.as_str()];
    args.extend_from_slice(actor);
    let out = run(&args);
    assert!(
        out.status.success(),
        "batch {id}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn fixture(ws: &Workspace, name: &str) -> String {
    let doc = ws.path(name);
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    doc
}

/// Issue a token for `handle` and return its secret.
fn token(doc: &str, role: &str, handle: &str, extra: &[&str]) -> String {
    let mut args = vec![
        "--actor-id",
        handle,
        "collab",
        "token",
        "create",
        doc,
        "--role",
        role,
        "--name",
        handle,
    ];
    args.extend_from_slice(extra);
    ok_json(&args)["token"].as_str().unwrap().to_string()
}

struct Server {
    url: String,
}

fn start(doc: &str, collab: bool) -> Server {
    let (tx, rx) = std::sync::mpsc::channel();
    let doc = PathBuf::from(doc);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let options = collab.then(|| pentool::server::CollabOptions {
                limits: pentool::collab::sync::Limits::default(),
                allow_insecure_lan: false,
            });
            let _ = pentool::server::serve_listener(listener, Some(doc), false, options).await;
        });
    });
    Server {
        url: format!("http://{}", rx.recv().unwrap()),
    }
}

fn http(method: &str, url: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let client = reqwest::blocking::Client::new();
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let mut request = client.request(method, url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(body.to_string());
    }
    let response = request.send().unwrap();
    let status = response.status().as_u16();
    let bytes = response.bytes().unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn ops_of(server: &Server, token: &str) -> Vec<Value> {
    let (status, page) = http(
        "GET",
        &format!("{}/api/collab/ops?since=0&limit=500", server.url),
        Some(token),
        None,
    );
    assert_eq!(status, 200, "{page}");
    page["ops"].as_array().unwrap().clone()
}

// ------------------------------------------------------------ convergence

/// A peer's document without its chain position. `collab.head` and `collab.seq`
/// depend on the order this peer appended operations, so two converged peers
/// agree on everything else, including the derived `stats_summary`.
fn converged_form(path: &str) -> Value {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    if let Some(block) = value.get_mut("collab").and_then(Value::as_object_mut) {
        block.remove("head");
        block.remove("seq");
    }
    value
}

#[test]
fn two_instances_converge_after_offline_divergence() {
    let ws = Workspace::new("converge");
    let a = fixture(&ws, "a.pen");
    let b = ws.path("b.pen");
    ok_json(&["collab", "init", &a]);
    let secret = token(&a, "editor", "bob", &[]);
    let server = start(&a, true);

    let joined = ok_json(&[
        "collab",
        "join",
        &b,
        "--peer",
        &server.url,
        "--token",
        &secret,
    ]);
    assert_eq!(joined["role"], json!("editor"));
    assert_eq!(converged_form(&a), converged_form(&b));

    // Both sides edit while disconnected.
    edit(&ws, &a, "from-a", &[]);
    edit(&ws, &b, "from-b", &["--actor-id", "bob"]);
    assert_ne!(fs::read(&a).unwrap(), fs::read(&b).unwrap());

    let synced = ok_json(&[
        "collab",
        "sync",
        &b,
        "--peer",
        &server.url,
        "--token",
        &secret,
    ]);
    assert_eq!(synced["pulled"]["accepted"], json!(1));
    assert_eq!(synced["pushed"]["accepted"], json!(1));
    assert_eq!(converged_form(&a), converged_form(&b));
    let text = fs::read_to_string(&a).unwrap();
    assert!(text.contains("from-a") && text.contains("from-b"));
    assert_eq!(ok_json(&["collab", "verify", &a])["chain"], json!("valid"));
    assert_eq!(ok_json(&["collab", "verify", &b])["chain"], json!("valid"));

    // Repeating the sync changes nothing.
    let before = fs::read(&b).unwrap();
    let again = ok_json(&[
        "collab",
        "sync",
        &b,
        "--peer",
        &server.url,
        "--token",
        &secret,
    ]);
    assert_eq!(again["pulled"]["accepted"], json!(0));
    assert_eq!(again["pushed"]["accepted"].as_u64().unwrap_or(0), 0);
    assert_eq!(fs::read(&b).unwrap(), before);
    assert_eq!(converged_form(&a), converged_form(&b));
}

#[test]
fn viewer_pulls_but_cannot_push() {
    let ws = Workspace::new("viewer");
    let a = fixture(&ws, "a.pen");
    let b = ws.path("b.pen");
    ok_json(&["collab", "init", &a]);
    let secret = token(&a, "viewer", "vera", &[]);
    let server = start(&a, true);
    ok_json(&[
        "collab",
        "join",
        &b,
        "--peer",
        &server.url,
        "--token",
        &secret,
    ]);
    edit(&ws, &a, "only-a", &[]);
    edit(&ws, &b, "local-v", &["--actor-id", "vera"]);
    let synced = ok_json(&[
        "collab",
        "sync",
        &b,
        "--peer",
        &server.url,
        "--token",
        &secret,
    ]);
    assert!(synced["pushed"].is_string());
    assert!(!fs::read_to_string(&a).unwrap().contains("local-v"));
    assert!(fs::read_to_string(&b).unwrap().contains("only-a"));
}

// ------------------------------------------------------------ hostile peer

#[test]
fn hostile_peer_is_contained() {
    let ws = Workspace::new("hostile");
    let a = fixture(&ws, "a.pen");
    let b = ws.path("b.pen");
    ok_json(&["collab", "init", &a]);
    let bob = token(&a, "editor", "bob", &[]);
    let server = start(&a, true);
    ok_json(&["collab", "join", &b, "--peer", &server.url, "--token", &bob]);
    edit(&ws, &a, "owner-edit", &[]);
    edit(&ws, &b, "bob-edit", &["--actor-id", "bob"]);
    ok_json(&["collab", "sync", &b, "--peer", &server.url, "--token", &bob]);

    let all = ops_of(&server, &bob);
    let bob_actor = pentool::collab::op::Actor::derive_id("human", "bob");
    let theirs = all.iter().find(|o| o["actor"] != json!(bob_actor)).unwrap();
    let mine = all.iter().find(|o| o["actor"] == json!(bob_actor)).unwrap();
    let ops_url = format!("{}/api/collab/ops", server.url);
    let doc_before = fs::read(&a).unwrap();

    // Forged actor: another actor's operation under bob's token.
    let (_, reply) = http("POST", &ops_url, Some(&bob), Some(json!({"ops": [theirs]})));
    assert_eq!(
        reply["rejected"][0]["code"],
        json!("actor-mismatch"),
        "{reply}"
    );

    // Replay of an operation the server already holds.
    let (status, reply) = http("POST", &ops_url, Some(&bob), Some(json!({"ops": [mine]})));
    assert_eq!(status, 200);
    assert_eq!(reply["duplicate"].as_array().unwrap().len(), 1, "{reply}");
    assert!(reply["accepted"].as_array().unwrap().is_empty());

    // Tampered content breaks the operation id.
    let mut tampered = mine.clone();
    tampered["payload"]["value"] = json!("tampered");
    tampered["id"] = json!("op_00000000000000000000000000000000");
    let (_, reply) = http(
        "POST",
        &ops_url,
        Some(&bob),
        Some(json!({"ops": [tampered]})),
    );
    assert!(reply["accepted"].as_array().unwrap().is_empty(), "{reply}");
    assert_eq!(reply["rejected"].as_array().unwrap().len(), 1, "{reply}");

    // Oversized operation.
    let mut huge = mine.clone();
    huge["payload"] = json!({"pad": "x".repeat(300 * 1024)});
    let (_, reply) = http("POST", &ops_url, Some(&bob), Some(json!({"ops": [huge]})));
    assert!(reply["accepted"].as_array().unwrap().is_empty(), "{reply}");
    assert_eq!(reply["rejected"].as_array().unwrap().len(), 1, "{reply}");

    // Too many operations in one batch.
    let many: Vec<Value> = (0..501).map(|_| mine.clone()).collect();
    let (status, reply) = http("POST", &ops_url, Some(&bob), Some(json!({"ops": many})));
    assert_eq!(status, 413, "{reply}");
    assert_eq!(reply["code"], json!("limit-exceeded"));

    // Malformed batch.
    let (status, _) = http("POST", &ops_url, Some(&bob), Some(json!({"ops": "nope"})));
    assert_eq!(status, 400);

    assert_eq!(
        fs::read(&a).unwrap(),
        doc_before,
        "document must be unchanged"
    );
    assert_eq!(ok_json(&["collab", "verify", &a])["chain"], json!("valid"));

    // Credentials.
    let session = format!("{}/api/collab/session", server.url);
    let (status, reply) = http("GET", &session, Some("ptc_deadbeef_00"), None);
    assert_eq!(
        (status, reply["code"].clone()),
        (401, json!("invalid-token"))
    );
    let client = reqwest::blocking::Client::new();
    let forwarded = client
        .get(&session)
        .header("x-forwarded-for", "10.0.0.9")
        .send()
        .unwrap();
    assert_eq!(
        forwarded.status().as_u16(),
        401,
        "proxied requests need a token"
    );
    let (status, _) = http("GET", &session, None, None);
    assert_eq!(status, 200, "the host's own loopback browser is the owner");

    // Expired and revoked tokens.
    let short = token(&a, "editor", "shorty", &["--expires", "1s"]);
    assert_eq!(http("GET", &session, Some(&short), None).0, 200);
    std::thread::sleep(std::time::Duration::from_millis(1600));
    let (status, reply) = http("GET", &session, Some(&short), None);
    assert_eq!(
        (status, reply["code"].clone()),
        (401, json!("token-expired"))
    );
    let listing = ok_json(&["collab", "token", "list", &a]);
    let id = listing["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == json!("bob"))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        !listing.to_string().contains(&bob),
        "secrets are never listed"
    );
    ok_json(&["collab", "token", "revoke", &a, &id]);
    let (status, reply) = http("GET", &session, Some(&bob), None);
    assert_eq!(
        (status, reply["code"].clone()),
        (401, json!("token-revoked"))
    );

    // Secrets are not stored in the sidecar or the document.
    let stored = fs::read_dir(ws.0.join(".pentool").join("collab"))
        .unwrap()
        .flatten()
        .flat_map(|d| fs::read_dir(d.path()).into_iter().flatten().flatten())
        .map(|f| fs::read_to_string(f.path()).unwrap_or_default())
        .collect::<String>();
    assert!(!stored.contains(&bob) && !stored.contains(&short));
    assert!(!fs::read_to_string(&a).unwrap().contains(&bob));
}

#[test]
fn peer_urls_and_documents_cannot_escape() {
    let ws = Workspace::new("paths");
    let a = fixture(&ws, "a.pen");
    ok_json(&["collab", "init", &a]);
    for bad in [
        "http://host/../../etc",
        "http://u:p@host:1",
        "http://host/?token=abc",
        "file:///etc/passwd",
        "http://host/%2e%2e/x",
    ] {
        let out = run(&["collab", "peer", "add", &a, bad]);
        assert!(!out.status.success(), "{bad} must be refused");
    }
    let ok = ok_json(&[
        "collab",
        "peer",
        "add",
        &a,
        "http://127.0.0.1:1",
        "--name",
        "one",
    ]);
    assert_eq!(ok["peers"].as_array().unwrap().len(), 1);
    assert!(!run(&["collab", "peer", "add", &a, "http://127.0.0.1:1"])
        .status
        .success());
    let listed = ok_json(&["collab", "peer", "list", &a]);
    assert_eq!(listed["matches"], json!(1));
    ok_json(&["collab", "peer", "remove", &a, "one"]);
    assert_eq!(
        ok_json(&["collab", "peer", "list", &a])["matches"],
        json!(0)
    );
    // Syncing with an unregistered or unreachable peer fails without touching the document.
    let before = fs::read(&a).unwrap();
    let out = run(&[
        "collab",
        "sync",
        &a,
        "--peer",
        "http://127.0.0.1:1",
        "--token",
        "t",
    ]);
    assert!(!out.status.success());
    assert_eq!(fs::read(&a).unwrap(), before);
    // A document outside the project cannot be reached through a route name.
    let server = start(&a, true);
    let (status, _) = http(
        "GET",
        &format!("{}/fonts/..%2f..%2fCargo.toml", server.url),
        None,
        None,
    );
    assert_ne!(status, 200);
}

#[test]
fn event_streams_are_bounded() {
    let ws = Workspace::new("sse");
    let a = fixture(&ws, "a.pen");
    ok_json(&["collab", "init", &a]);
    let secret = token(&a, "viewer", "vera", &[]);
    let server = start(&a, true);
    let client = reqwest::blocking::Client::new();
    let mut open = Vec::new();
    let mut refused = 0;
    for _ in 0..8 {
        let response = client
            .get(format!("{}/api/collab/events", server.url))
            .bearer_auth(&secret)
            .send()
            .unwrap();
        if response.status().as_u16() == 429 {
            refused += 1;
        } else {
            assert_eq!(response.status().as_u16(), 200);
            open.push(response);
        }
    }
    assert_eq!(open.len(), 4, "per-token stream limit");
    assert_eq!(refused, 4);
}

// ------------------------------------------------------------ permissions

fn all_routes() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for source in [
        include_str!("../src/server.rs"),
        include_str!("../src/collab/sync.rs"),
    ] {
        for (path, methods) in pentool::collab::perm::scan_routes(source) {
            for method in methods {
                out.push((method, path.replace(":name", "x")));
            }
        }
    }
    out
}

#[test]
fn every_route_is_classified() {
    assert_eq!(
        unclassified_routes(include_str!("../src/server.rs")),
        Vec::<String>::new()
    );
    assert_eq!(
        unclassified_routes(include_str!("../src/collab/sync.rs")),
        Vec::<String>::new()
    );
    assert!(all_routes().len() > 80);
    assert_eq!(classify("DELETE", "/api/unknown"), None);
}

#[test]
fn permission_matrix_is_enforced_on_every_route() {
    let ws = Workspace::new("matrix");
    let a = fixture(&ws, "a.pen");
    ok_json(&["collab", "init", &a]);
    let before = fs::read(&a).unwrap();
    let server = start(&a, true);
    let roles = [
        ("viewer", Role::Viewer),
        ("commenter", Role::Commenter),
        ("editor", Role::Editor),
    ];
    let mut checked = 0;
    for (name, role) in roles {
        let secret = token(&a, name, &format!("user-{name}"), &[]);
        for (method, path) in all_routes() {
            if path == "/api/collab/events" {
                continue;
            }
            let Some(Need::Role(need)) = classify(&method, &path) else {
                continue;
            };
            if role >= need {
                continue;
            }
            let (status, reply) = http(
                &method,
                &format!("{}{path}", server.url),
                Some(&secret),
                Some(json!({})),
            );
            assert_eq!(status, 403, "{name} {method} {path}: {reply}");
            assert_eq!(reply["code"], json!("permission-denied"));
            checked += 1;
        }
    }
    assert!(checked > 100, "matrix covered only {checked} denials");

    // Allowed requests still work.
    let viewer = token(&a, "viewer", "vera", &[]);
    let commenter = token(&a, "commenter", "carl", &[]);
    let owner = token(&a, "owner", "olga", &[]);
    assert_eq!(
        http(
            "GET",
            &format!("{}/api/collab/session", server.url),
            Some(&viewer),
            None
        )
        .0,
        200
    );
    let (_, session) = http(
        "GET",
        &format!("{}/api/collab/session", server.url),
        Some(&commenter),
        None,
    );
    assert_eq!(
        session["may"],
        json!({"read": true, "comment": true, "edit": false, "manage": false})
    );
    let (status, tokens) = http(
        "GET",
        &format!("{}/api/collab/tokens", server.url),
        Some(&owner),
        None,
    );
    assert_eq!(status, 200);
    assert!(!tokens.to_string().contains(&owner));
    // Unauthenticated proxied requests never reach a handler.
    let client = reqwest::blocking::Client::new();
    let denied = client
        .post(format!("{}/api/edit", server.url))
        .header("x-forwarded-for", "10.1.1.1")
        .body("{}")
        .send()
        .unwrap();
    assert_eq!(denied.status().as_u16(), 401);
    assert_eq!(
        fs::read(&a).unwrap(),
        before,
        "denied requests must not touch the document"
    );
}

#[test]
fn only_the_owner_manages_tokens_and_roles() {
    let ws = Workspace::new("escalate");
    let a = fixture(&ws, "a.pen");
    ok_json(&["collab", "init", &a]);
    let editor = token(&a, "editor", "eddie", &[]);
    let server = start(&a, true);
    let base = &server.url;
    let body = json!({"role": "owner", "name": "evil", "actor": {"kind": "human", "id": "eddie"}});
    assert_eq!(
        http(
            "POST",
            &format!("{base}/api/collab/tokens"),
            Some(&editor),
            Some(body)
        )
        .0,
        403
    );
    let listing = ok_json(&["collab", "token", "list", &a]);
    let id = listing["tokens"][0]["id"].as_str().unwrap().to_string();
    let body = json!({"id": id, "role": "owner"});
    assert_eq!(
        http(
            "POST",
            &format!("{base}/api/collab/tokens/role"),
            Some(&editor),
            Some(body)
        )
        .0,
        403
    );
    assert_eq!(
        ok_json(&["collab", "token", "list", &a])["tokens"][0]["role"],
        json!("editor")
    );
}

// ----------------------------------------------------------------- claims

#[test]
fn claims_block_other_actors_until_released() {
    let ws = Workspace::new("claims");
    let a = fixture(&ws, "a.pen");
    ok_json(&["collab", "init", &a]);
    let bob = token(&a, "editor", "bob", &[]);
    let server = start(&a, true);
    let claim_url = format!("{}/api/collab/claim", server.url);
    let (status, reply) = http(
        "POST",
        &claim_url,
        Some(&bob),
        Some(json!({"targets": ["rect-1"], "ttl_ms": 60000})),
    );
    assert_eq!(status, 200, "{reply}");
    let (status, reply) = http(
        "POST",
        &claim_url,
        Some(&bob),
        Some(json!({"targets": ["rect-1"], "ttl_ms": 99999999})),
    );
    assert_eq!(status, 413, "{reply}");
    let (_, session) = http(
        "GET",
        &format!("{}/api/collab/session", server.url),
        Some(&bob),
        None,
    );
    assert_eq!(session["claims"].as_array().unwrap().len(), 1);
    let (status, reply) = http(
        "POST",
        &format!("{}/api/collab/release", server.url),
        Some(&bob),
        Some(json!({"targets": ["rect-1"]})),
    );
    assert_eq!((status, reply["released"].clone()), (200, json!(1)));
}

// ------------------------------------------------------------------ crash

#[test]
fn crash_during_sync_resumes_to_the_same_bytes() {
    for fault in [Fault::AfterPrepare, Fault::AfterDocument] {
        let ws = Workspace::new(&format!("crash-{fault:?}"));
        let a = fixture(&ws, "a.pen");
        let b = ws.path("b.pen");
        ok_json(&["collab", "init", &a]);
        let secret = token(&a, "editor", "bob", &[]);
        let server = start(&a, true);
        ok_json(&[
            "collab",
            "join",
            &b,
            "--peer",
            &server.url,
            "--token",
            &secret,
        ]);
        edit(&ws, &a, "one", &[]);
        edit(&ws, &a, "two", &[]);
        let before = fs::read(&b).unwrap();

        inject_fault(&PathBuf::from(&b), fault);
        let failed = sync_with_peer(&PathBuf::from(&b), &server.url, &secret, false);
        let error = failed.unwrap_err().to_string();
        assert!(error.contains("fault-injected"), "{error}");
        if fault == Fault::AfterPrepare {
            assert_eq!(
                fs::read(&b).unwrap(),
                before,
                "document untouched before commit"
            );
        }

        // "Restart": the next run recovers the interrupted unit and finishes.
        let resumed = sync_with_peer(&PathBuf::from(&b), &server.url, &secret, false).unwrap();
        assert!(resumed["head"]["seq"].as_u64().unwrap() >= 2);
        // A fresh, uninterrupted join of the same history is the reference.
        let reference = ws.path("reference.pen");
        ok_json(&[
            "collab",
            "join",
            &reference,
            "--peer",
            &server.url,
            "--token",
            &secret,
        ]);
        assert!(
            converged_form(&b) == converged_form(&reference),
            "{fault:?}: resumed bytes differ from an uninterrupted sync"
        );
        assert_eq!(converged_form(&a), converged_form(&b), "{fault:?}");
        assert_eq!(ok_json(&["collab", "verify", &b])["chain"], json!("valid"));
        assert_eq!(
            ok_json(&["collab", "verify", &b])["replay"]["matches_document"],
            json!(true)
        );
    }
}

// ------------------------------------------------------------ no --collab

#[test]
fn server_without_collab_is_unchanged() {
    let ws = Workspace::new("plain");
    let a = fixture(&ws, "a.pen");
    let server = start(&a, false);
    let (status, _) = http(
        "GET",
        &format!("{}/api/collab/session", server.url),
        None,
        None,
    );
    assert_eq!(status, 404);
    // No token is needed or inspected.
    let (status, _) = http("GET", &format!("{}/api/fonts", server.url), None, None);
    assert_eq!(status, 200);
    let (status, _) = http(
        "GET",
        &format!("{}/api/fonts", server.url),
        Some("garbage"),
        None,
    );
    assert_eq!(status, 200);
    let (status, _) = http("GET", &format!("{}/api/document", server.url), None, None);
    assert_eq!(status, 200);
}

#[test]
fn non_loopback_bind_needs_tokens() {
    let ws = Workspace::new("bind");
    let a = fixture(&ws, "a.pen");
    ok_json(&["collab", "init", &a]);
    let out = run(&["serve", &a, "--collab", "--host", "0.0.0.0", "--port", "0"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("without tokens"));
    token(&a, "editor", "bob", &[]);
    let out = run(&["serve", &a, "--collab", "--host", "0.0.0.0", "--port", "0"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("allow-insecure-lan"));
}

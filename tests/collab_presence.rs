//! Best-effort presence: in-memory only, bounded, rate-limited, derived from
//! the token and never part of the document, history or operation log.
use pentool::collab::op::Actor;
use pentool::collab::presence::{Flags, PresenceLimits, Store, Update};
use pentool::collab::sync::Limits;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

struct Workspace(PathBuf);
impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-presence-{name}-{}", std::process::id()));
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

fn ok_json(args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .env_remove("PENTOOL_COLLAB_TOKEN")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn setup(ws: &Workspace) -> String {
    let doc = ws.path("a.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    ok_json(&["collab", "init", &doc]);
    doc
}

fn token(doc: &str, role: &str, handle: &str) -> String {
    ok_json(&[
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
    ])["token"]
        .as_str()
        .unwrap()
        .to_string()
}

fn start(doc: &str) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    let doc = PathBuf::from(doc);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let options = pentool::server::CollabOptions {
                limits: Limits::default(),
                allow_insecure_lan: false,
            };
            let _ =
                pentool::server::serve_listener(listener, Some(doc), false, Some(options)).await;
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

fn http(method: &str, url: &str, token: Option<&str>, body: Option<String>) -> (u16, Value) {
    let client = reqwest::blocking::Client::new();
    let mut request = client.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(body);
    }
    let response = request.send().unwrap();
    let status = response.status().as_u16();
    let bytes = response.bytes().unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn publish(url: &str, token: &str, body: Value) -> (u16, Value) {
    http(
        "POST",
        &format!("{url}/api/collab/presence"),
        Some(token),
        Some(body.to_string()),
    )
}

fn actor_id(url: &str, token: &str) -> String {
    let (status, session) = http(
        "GET",
        &format!("{url}/api/collab/session"),
        Some(token),
        None,
    );
    assert_eq!(status, 200, "{session}");
    session["actor"]["id"].as_str().unwrap().to_string()
}

fn snapshot(url: &str, token: &str) -> Value {
    let (status, body) = http(
        "GET",
        &format!("{url}/api/collab/presence"),
        Some(token),
        None,
    );
    assert_eq!(status, 200, "{body}");
    body
}

// ------------------------------------------------------------------ tests

#[test]
fn presence_never_touches_the_document_or_disk() {
    let ws = Workspace::new("noop");
    let doc = setup(&ws);
    let editor = token(&doc, "editor", "eve");
    let url = start(&doc);
    let files = |dir: &PathBuf| {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let before_bytes = fs::read(&doc).unwrap();
    let before_files = files(&ws.0);
    let before_ops = http(
        "GET",
        &format!("{url}/api/collab/ops?since=0&limit=500"),
        Some(&editor),
        None,
    )
    .1;

    for i in 0..5 {
        let (status, reply) = publish(
            &url,
            &editor,
            json!({"page": "p1", "selection": ["a", "b"], "cursor": {"x": i, "y": 2.5},
                   "viewport": {"x": 0, "y": 0, "width": 800, "height": 600}}),
        );
        assert_eq!(status, 200, "{reply}");
    }
    assert_eq!(publish(&url, &editor, json!({})).0, 200);
    let seen = snapshot(&url, &editor);
    assert_eq!(seen["matches"], json!(1));
    assert_eq!(
        seen["entries"][0]["actor"]["id"],
        json!(actor_id(&url, &editor))
    );
    assert_eq!(seen["entries"][0]["cursor"], json!({"x": 4.0, "y": 2.5}));

    assert_eq!(fs::read(&doc).unwrap(), before_bytes);
    assert_eq!(files(&ws.0), before_files);
    let after_ops = http(
        "GET",
        &format!("{url}/api/collab/ops?since=0&limit=500"),
        Some(&editor),
        None,
    )
    .1;
    assert_eq!(before_ops, after_ops);
    // Nothing in the document or its sidecars mentions presence.
    for name in files(&ws.0) {
        let path = ws.0.join(&name);
        if path.is_file() {
            assert!(!String::from_utf8_lossy(&fs::read(&path).unwrap()).contains("\"cursor\""));
        }
    }
}

#[test]
fn identity_comes_from_the_token_and_forgery_is_refused() {
    let ws = Workspace::new("forge");
    let doc = setup(&ws);
    let editor = token(&doc, "editor", "eve");
    let url = start(&doc);
    for forged in [
        json!({"actor": "mallory"}),
        json!({"actor": {"id": "mallory", "kind": "human"}}),
        json!({"actor": 7}),
    ] {
        let (status, reply) = publish(&url, &editor, forged);
        assert_eq!(status, 403, "{reply}");
        assert_eq!(reply["code"], json!("permission-denied"));
    }
    // Naming itself is fine; unknown fields are not.
    let own = actor_id(&url, &editor);
    assert_eq!(publish(&url, &editor, json!({"actor": own})).0, 200);
    assert_eq!(publish(&url, &editor, json!({"colour": "red"})).0, 400);
    let seen = snapshot(&url, &editor);
    assert_eq!(seen["matches"], json!(1));
    assert_eq!(seen["entries"][0]["actor"]["id"], json!(own));
}

#[test]
fn viewers_read_but_cannot_publish_and_commenters_can() {
    let ws = Workspace::new("roles");
    let doc = setup(&ws);
    let viewer = token(&doc, "viewer", "vera");
    let commenter = token(&doc, "commenter", "carl");
    let url = start(&doc);
    let (status, reply) = publish(&url, &viewer, json!({}));
    assert_eq!(status, 403, "{reply}");
    assert_eq!(reply["code"], json!("permission-denied"));
    assert_eq!(
        publish(&url, &commenter, json!({"cursor": {"x": 1, "y": 1}})).0,
        200
    );
    assert_eq!(snapshot(&url, &viewer)["matches"], json!(1));
    // Leaving removes the entry.
    let (status, reply) = publish(&url, &commenter, json!({"leave": true}));
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["left"], json!(true));
    assert_eq!(snapshot(&url, &viewer)["matches"], json!(0));
}

#[test]
fn bad_input_and_size_caps_are_refused_before_work() {
    let ws = Workspace::new("caps");
    let doc = setup(&ws);
    let editor = token(&doc, "editor", "eve");
    let url = start(&doc);
    let ids: Vec<String> = (0..65).map(|i| format!("o{i}")).collect();
    let cases = vec![
        json!({"selection": ids}),
        json!({"selection": [""]}),
        json!({"selection": ["x".repeat(300)]}),
        json!({"page": ""}),
        json!({"cursor": {"x": 1e12, "y": 0}}),
        json!({"cursor": {"x": 1}}),
        json!({"viewport": {"x": 0, "y": 0, "width": -1, "height": 5}}),
        json!({"leave": true, "page": "p1"}),
    ];
    for case in cases {
        let (status, reply) = publish(&url, &editor, case.clone());
        assert!(status == 400 || status == 413, "{case}: {status} {reply}");
        // Spread over rate windows so only validation is under test.
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    let (status, reply) = http(
        "POST",
        &format!("{url}/api/collab/presence"),
        Some(&editor),
        Some(format!("{{\"page\":\"{}\"}}", "p".repeat(10_000))),
    );
    assert!(status == 413 || status == 400, "{status} {reply}");
    assert_eq!(snapshot(&url, &editor)["matches"], json!(0));
    assert!(Update::parse(b"{\"page\": nope").is_err());
    assert!(Update::parse(&vec![b' '; 5000]).is_err());
}

#[test]
fn updates_are_rate_limited_per_token() {
    let ws = Workspace::new("rate");
    let doc = setup(&ws);
    let busy = token(&doc, "editor", "busy");
    let calm = token(&doc, "editor", "calm");
    let url = start(&doc);
    let refused = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for t in 0..8 {
            let (url, busy, refused) = (&url, &busy, &refused);
            scope.spawn(move || {
                for i in 0..10 {
                    let (status, reply) =
                        publish(url, busy, json!({"cursor": {"x": i + t, "y": 0}}));
                    match status {
                        200 => {}
                        429 => {
                            assert_eq!(reply["code"], json!("rate-limited"));
                            refused.fetch_add(1, Ordering::SeqCst);
                        }
                        other => panic!("unexpected {other}: {reply}"),
                    }
                }
            });
        }
    });
    let refused = refused.load(Ordering::SeqCst);
    assert!(refused > 0, "burst was never limited");
    // Another token is unaffected.
    assert_eq!(publish(&url, &calm, json!({})).0, 200);
}

#[test]
fn event_stream_shares_the_bounded_stream_slots() {
    let ws = Workspace::new("sse");
    let doc = setup(&ws);
    let viewer = token(&doc, "viewer", "vera");
    let url = start(&doc);
    let client = reqwest::blocking::Client::new();
    let mut open = Vec::new();
    let mut refused = 0;
    for _ in 0..8 {
        let response = client
            .get(format!("{url}/api/collab/presence/events"))
            .bearer_auth(&viewer)
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

#[test]
fn unknown_ids_are_flagged_not_trusted() {
    let ws = Workspace::new("flags");
    let doc = setup(&ws);
    let editor = token(&doc, "editor", "eve");
    let url = start(&doc);
    let (status, reply) = publish(
        &url,
        &editor,
        json!({"page": "no-such-page", "selection": ["no-such-object"]}),
    );
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["unknown_page"], json!(true));
    let seen = snapshot(&url, &editor);
    assert_eq!(seen["entries"][0]["unknown_page"], json!(true));
}

#[test]
fn session_reports_presence_limits() {
    let ws = Workspace::new("session");
    let doc = setup(&ws);
    let viewer = token(&doc, "viewer", "vera");
    let url = start(&doc);
    let (status, session) = http(
        "GET",
        &format!("{url}/api/collab/session"),
        Some(&viewer),
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(session["presence"]["updates_per_second"], json!(10));
    assert_eq!(session["presence"]["ttl_ms"], json!(15000));
}

// ----------------------------------------------- store with a manual clock

fn actor(id: &str) -> Actor {
    Actor {
        id: id.into(),
        kind: "human".into(),
        display_name: Some(id.into()),
        tool: None,
        model: None,
        directed_by: None,
    }
}

fn manual() -> (Arc<AtomicU64>, Store) {
    let now = Arc::new(AtomicU64::new(1_000));
    let clock = now.clone();
    let store = Store::with_clock(
        PresenceLimits {
            updates_per_second: 3,
            max_entries: 2,
            ttl_ms: 5_000,
        },
        Arc::new(move || clock.load(Ordering::SeqCst)),
    );
    (now, store)
}

#[test]
fn entries_expire_after_the_ttl_and_heartbeats_extend_it() {
    let (now, store) = manual();
    let cursor = Update::parse(br#"{"cursor":{"x":1,"y":2}}"#).unwrap();
    store
        .publish(&actor("a"), &cursor, Flags::default())
        .unwrap();
    store
        .publish(&actor("b"), &cursor, Flags::default())
        .unwrap();
    assert_eq!(store.snapshot(0, 10)["matches"], json!(2));

    now.fetch_add(4_000, Ordering::SeqCst);
    let beat = Update::parse(b"{}").unwrap();
    assert!(beat.is_heartbeat());
    store.publish(&actor("a"), &beat, Flags::default()).unwrap();
    now.fetch_add(2_000, Ordering::SeqCst);
    let seen = store.snapshot(0, 10);
    assert_eq!(seen["matches"], json!(1), "b expired, a was kept alive");
    assert_eq!(seen["entries"][0]["actor"]["id"], json!("a"));
    // A heartbeat keeps the state it already had.
    assert_eq!(seen["entries"][0]["cursor"], json!({"x": 1.0, "y": 2.0}));

    now.fetch_add(5_000, Ordering::SeqCst);
    assert_eq!(store.snapshot(0, 10)["matches"], json!(0));
}

#[test]
fn entry_cap_drops_new_participants_until_one_expires() {
    let (now, store) = manual();
    let beat = Update::parse(b"{}").unwrap();
    store.publish(&actor("a"), &beat, Flags::default()).unwrap();
    store.publish(&actor("b"), &beat, Flags::default()).unwrap();
    let error = store
        .publish(&actor("c"), &beat, Flags::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("[limit-exceeded]"), "{error}");
    // An existing participant can still update.
    store.publish(&actor("a"), &beat, Flags::default()).unwrap();
    now.fetch_add(6_000, Ordering::SeqCst);
    store.publish(&actor("c"), &beat, Flags::default()).unwrap();
    assert_eq!(store.snapshot(0, 10)["matches"], json!(1));
}

#[test]
fn rate_window_resets_each_second_and_snapshots_are_ordered_and_paged() {
    let (now, store) = manual();
    assert!(store.allow("t") && store.allow("t") && store.allow("t"));
    assert!(!store.allow("t"));
    assert!(store.allow("other"));
    now.fetch_add(1_000, Ordering::SeqCst);
    assert!(store.allow("t"));

    let beat = Update::parse(b"{}").unwrap();
    store
        .publish(&actor("zed"), &beat, Flags::default())
        .unwrap();
    store
        .publish(&actor("amy"), &beat, Flags::default())
        .unwrap();
    let first = store.snapshot(0, 1);
    assert_eq!(first["entries"][0]["actor"]["id"], json!("amy"));
    assert_eq!(
        (
            first["matches"].as_u64(),
            first["returned"].as_u64(),
            first["has_more"].as_bool()
        ),
        (Some(2), Some(1), Some(true))
    );
    let second = store.snapshot(1, 1);
    assert_eq!(second["entries"][0]["actor"]["id"], json!("zed"));
    assert_eq!(second["has_more"], json!(false));
}

#[test]
fn version_only_moves_when_viewers_would_see_a_change() {
    let (now, store) = manual();
    let spot = Update::parse(br#"{"cursor":{"x":1,"y":1}}"#).unwrap();
    store.publish(&actor("a"), &spot, Flags::default()).unwrap();
    let v = store.version();
    store.publish(&actor("a"), &spot, Flags::default()).unwrap();
    store
        .publish(
            &actor("a"),
            &Update::parse(b"{}").unwrap(),
            Flags::default(),
        )
        .unwrap();
    assert_eq!(
        store.version(),
        v,
        "identical state and heartbeats are silent"
    );
    assert!(store.view_if_changed(Some(v)).is_none());
    now.fetch_add(9_000, Ordering::SeqCst);
    let (version, view) = store.view_if_changed(Some(v)).unwrap();
    assert!(version > v);
    assert_eq!(view["matches"], json!(0));
}

/// EXT-002: under a token, a comment's author is the token's actor; a body
/// `actor` cannot attribute the comment to someone else.
#[test]
fn comment_author_comes_from_the_token() {
    let ws = Workspace::new("comment-author");
    let doc = setup(&ws);
    let commenter = token(&doc, "commenter", "carl");
    let url = start(&doc);
    let own = actor_id(&url, &commenter);
    let forged = json!({
        "action": "add",
        "body": "looks good",
        "actor": {"kind": "human", "id": "mallory", "name": "Mallory"}
    });
    let (status, reply) = http(
        "POST",
        &format!("{url}/api/comments"),
        Some(&commenter),
        Some(forged.to_string()),
    );
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["thread"]["opened_by"]["id"], json!(own), "{reply}");
    assert_ne!(reply["thread"]["opened_by"]["name"], json!("Mallory"));
}

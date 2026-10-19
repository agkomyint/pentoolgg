//! `GET /api/document/events` tells an open Studio tab when any writer changes
//! the shared document, with the same revision token as `GET /api/document`.
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct Workspace(PathBuf);
impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pentool-live-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn start(doc: PathBuf) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let _ = pentool::server::serve_listener(listener, Some(doc), false, None).await;
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

fn disk_revision(url: &str) -> String {
    let bytes = reqwest::blocking::get(format!("{url}/api/document"))
        .unwrap()
        .bytes()
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    body["revision"].as_str().unwrap().to_string()
}

/// The next `revision` event's token; keep-alive comments are skipped.
fn next_revision(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> String {
    let mut event = String::new();
    for line in lines {
        let line = line.expect("event stream ended before a revision event");
        if let Some(name) = line.strip_prefix("event:") {
            event = name.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data:") {
            assert_eq!(event, "revision");
            let data: Value = serde_json::from_str(data.trim()).unwrap();
            return data["revision"].as_str().unwrap().to_string();
        }
    }
    panic!("event stream ended before a revision event")
}

#[test]
fn an_open_stream_reports_a_cli_edit_with_the_document_revision() {
    let ws = Workspace::new("cli-edit");
    let doc = ws.0.join("a.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let url = start(doc.clone());

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let response = client
        .get(format!("{url}/api/document/events"))
        .send()
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let mut lines = BufReader::new(response).lines();

    let first = next_revision(&mut lines);
    assert_eq!(
        first,
        disk_revision(&url),
        "connect reports the current revision"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .arg("text")
        .arg(&doc)
        .args([
            "put",
            "live-note",
            "--layer",
            "content",
            "--content",
            "hello",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let second = next_revision(&mut lines);
    assert_ne!(second, first, "an edit outside the server is reported");
    assert_eq!(
        second,
        disk_revision(&url),
        "the event names the new disk revision"
    );
}

#[test]
fn the_stream_needs_a_shared_document() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let _ = pentool::server::serve_listener(listener, None, false, None).await;
        });
    });
    let url = format!("http://{}", rx.recv().unwrap());
    let status = reqwest::blocking::get(format!("{url}/api/document/events"))
        .unwrap()
        .status();
    assert_eq!(status.as_u16(), 404);
}

/// The next `revision` event's whole payload.
fn next_event(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> Value {
    let mut event = String::new();
    for line in lines {
        let line = line.expect("event stream ended before a revision event");
        if let Some(name) = line.strip_prefix("event:") {
            event = name.trim().to_string();
        } else if let Some(data) = line.strip_prefix("data:") {
            assert_eq!(event, "revision");
            return serde_json::from_str(data.trim()).unwrap();
        }
    }
    panic!("event stream ended before a revision event")
}

#[test]
fn an_agent_edit_is_attributed_and_its_versions_can_be_previewed_and_restored() {
    let ws = Workspace::new("agent-edit");
    let doc = ws.0.join("a.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let original = fs::read(&doc).unwrap();
    let url = start(doc.clone());
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let response = client
        .get(format!("{url}/api/document/events"))
        .send()
        .unwrap();
    let mut lines = BufReader::new(response).lines();
    let first = next_event(&mut lines);
    assert!(first.get("change").is_none(), "no history yet: {first}");

    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .arg("text")
        .arg(&doc)
        .args(["put", "agent-note", "--layer", "content", "--content", "hi"])
        .args(["--actor-kind", "agent", "--actor-id", "claude"])
        .args(["--actor-name", "Claude", "--directed-by", "aung"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let event = next_event(&mut lines);
    let change = &event["change"];
    assert_eq!(change["actor"]["kind"], "agent", "{event}");
    assert_eq!(change["actor"]["name"], "Claude", "{event}");
    let before = change["before"].as_str().unwrap().to_string();

    let version: Value = serde_json::from_slice(
        &client
            .get(format!("{url}/api/history/version?hash={before}"))
            .send()
            .unwrap()
            .bytes()
            .unwrap(),
    )
    .unwrap();
    let previewed: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(version["document"], previewed, "preview is the old bytes");

    let bad = client
        .get(format!("{url}/api/history/version?hash=..%2F..%2Fsecret"))
        .send()
        .unwrap();
    assert!(!bad.status().is_success(), "a non-digest hash is refused");

    let restored = client
        .post(format!("{url}/api/history/restore"))
        .header("content-type", "application/json")
        .body(serde_json::json!({ "hash": before }).to_string())
        .send()
        .unwrap();
    assert!(restored.status().is_success());
    assert_eq!(
        fs::read(&doc).unwrap(),
        original,
        "restore writes the version"
    );

    let history: Value = serde_json::from_slice(
        &client
            .get(format!("{url}/api/history"))
            .send()
            .unwrap()
            .bytes()
            .unwrap(),
    )
    .unwrap();
    let entries = history["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{history}");
    assert_eq!(entries[1]["operation"], "restore", "restore is undoable");
}

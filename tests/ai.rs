//! v0.9.0 optional BYOK image-model tooling, exercised against a local mock provider.
use base64::Engine;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::{fs, thread};

const KEY: &str = "TESTKEY-abcdef0123456789";

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-ai-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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

fn pentool(ws: &Workspace, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .env("PENTOOL_AI_CONFIG", ws.path("ai.json"))
        .env("TEST_PROVIDER_KEY", KEY)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("PENTOOL_AI")
        .output()
        .unwrap()
}

fn ok(ws: &Workspace, args: &[&str]) -> Value {
    let out = pentool(ws, args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn fail(ws: &Workspace, args: &[&str]) -> Value {
    let out = pentool(ws, args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    let text = String::from_utf8_lossy(&out.stderr);
    let line = text.lines().find(|l| l.starts_with('{')).unwrap_or("{}");
    serde_json::from_str::<Value>(line).unwrap()["error"].clone()
}

fn png(color: [u8; 4], size: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(size, size, image::Rgba(color));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// Serve `responses` in order (one per connection); returns the base URL and the
/// captured requests.
fn mock(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!(
        "http://127.0.0.1:{}/v1beta",
        listener.local_addr().unwrap().port()
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    thread::spawn(move || {
        for (status, body) in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut data = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                let n = stream.read(&mut buffer).unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&data).into_owned();
                if let Some(split) = text.find("\r\n\r\n") {
                    let length = text[..split]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if data.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            captured
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&data).into_owned());
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (base, seen)
}

fn gemini_image(bytes: &[u8]) -> String {
    json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(bytes)}}]}}],"usageMetadata":{"totalTokenCount":7}}).to_string()
}

fn connect(ws: &Workspace, endpoint: &str) {
    ok(
        ws,
        &[
            "ai",
            "connect",
            "--name",
            "gemini",
            "--endpoint",
            endpoint,
            "--credential-env",
            "TEST_PROVIDER_KEY",
        ],
    );
}

fn new_doc(ws: &Workspace) -> String {
    let doc = ws.path("doc.pen");
    let out = pentool(ws, &["new", &doc, "--width", "400", "--height", "300"]);
    assert!(out.status.success());
    doc
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(read) = fs::read_dir(dir) {
        for entry in read.flatten() {
            if entry.path().is_dir() {
                out.extend(walk(&entry.path()));
            } else {
                out.push(entry.path());
            }
        }
    }
    out
}

#[test]
fn setup_from_env_is_idempotent_and_stores_no_secret() {
    let ws = Workspace::new("setup");
    let run = |ws: &Workspace| {
        let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["ai", "setup", "--from-env"])
            .env("PENTOOL_AI_CONFIG", ws.path("ai.json"))
            .env("GEMINI_API_KEY", KEY)
            .env_remove("OPENAI_API_KEY")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let first = run(&ws);
    assert_eq!(first["changed"], true);
    assert_eq!(first["providers"][0]["status"], "added");
    assert_eq!(
        first["defaults"]["generate"],
        "gemini/gemini-2.5-flash-image"
    );
    let second = run(&ws);
    assert_eq!(second["changed"], false);
    assert_eq!(second["providers"][0]["status"], "unchanged");
    let config = fs::read_to_string(ws.path("ai.json")).unwrap();
    assert!(!config.contains(KEY));
    assert!(config.contains("GEMINI_API_KEY"));
}

#[test]
fn ephemeral_setup_writes_nothing() {
    let ws = Workspace::new("ephemeral");
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["ai", "setup", "--from-env", "--ephemeral"])
        .env("PENTOOL_AI_CONFIG", ws.path("ai.json"))
        .env("OPENAI_API_KEY", KEY)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!std::path::Path::new(&ws.path("ai.json")).exists());
}

#[test]
fn raw_keys_on_the_command_line_are_rejected() {
    let ws = Workspace::new("rawkey");
    let error = fail(
        &ws,
        &[
            "ai",
            "connect",
            "--name",
            "gemini",
            "--api-key",
            KEY,
            "--credential-env",
            "X",
            "--json",
        ],
    );
    assert_eq!(error["code"], "raw-secret-rejected");
    assert!(error["fix"].as_str().unwrap().contains("--credential-env"));
    assert!(!std::path::Path::new(&ws.path("ai.json")).exists());
}

#[test]
fn model_calls_are_gated_and_dry_run_never_connects() {
    let ws = Workspace::new("gate");
    let (base, seen) = mock(vec![]);
    connect(&ws, &base);
    let doc = new_doc(&ws);
    let denied = fail(
        &ws,
        &["ai", "generate", &doc, "art", "--prompt", "a fox", "--json"],
    );
    assert_eq!(denied["code"], "model-call-not-allowed");
    let dry = ok(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "art",
            "--prompt",
            "a fox",
            "--dry-run",
        ],
    );
    assert_eq!(dry["contacted_provider"], false);
    assert_eq!(dry["disclosure"]["endpoint_host"], "127.0.0.1");
    assert!(!dry.to_string().contains(KEY));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn generate_accept_and_replace_keep_provenance_without_secrets() {
    let ws = Workspace::new("flow");
    let first = png([200, 40, 40, 255], 16);
    let second = png([40, 40, 200, 255], 16);
    let (base, seen) = mock(vec![
        (200, gemini_image(&first)),
        (200, gemini_image(&second)),
    ]);
    connect(&ws, &base);
    let doc = new_doc(&ws);

    let run = ok(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "art",
            "--prompt",
            "a fox",
            "--aspect",
            "1:1",
            "--allow-model-call",
        ],
    );
    assert_eq!(run["document_changed"], false);
    let id = run["run"].as_str().unwrap().to_owned();
    {
        let requests = seen.lock().unwrap();
        assert!(
            requests[0].contains(&format!("key={KEY}")),
            "key goes in the query only"
        );
        assert!(requests[0].contains("\"aspectRatio\":\"1:1\""));
    }
    let before = fs::read(&doc).unwrap();
    let dry = ok(
        &ws,
        &[
            "ai",
            "run",
            "accept",
            &doc,
            &id,
            "--candidate",
            "1",
            "--dry-run",
        ],
    );
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fs::read(&doc).unwrap(), before);

    ok(
        &ws,
        &[
            "ai",
            "run",
            "accept",
            &doc,
            &id,
            "--candidate",
            "1",
            "--id",
            "hero",
        ],
    );
    let text = fs::read_to_string(&doc).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    let node = &value["pages"][0]["layers"][0]["nodes"][0];
    assert_eq!(node["id"], "hero");
    assert_eq!(node["ai"]["provider"], "gemini");
    assert!(node["ai"]["prompt_sha256"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert!(!text.contains(KEY));
    assert!(
        !text.contains("a fox"),
        "prompt text is not stored in the document"
    );

    let second_run = ok(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "art2",
            "--prompt",
            "a bluer fox",
            "--allow-model-call",
        ],
    );
    let id2 = second_run["run"].as_str().unwrap().to_owned();
    let old_asset = node["asset"].as_str().unwrap().to_owned();
    ok(
        &ws,
        &[
            "ai",
            "run",
            "accept",
            &doc,
            &id2,
            "--candidate",
            "1",
            "--replace",
            "hero",
        ],
    );
    let replaced: Value = serde_json::from_str(&fs::read_to_string(&doc).unwrap()).unwrap();
    let nodes = replaced["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["id"], "hero");
    assert_ne!(nodes[0]["asset"].as_str().unwrap(), old_asset);
    assert!(
        replaced["image_assets"].get(&old_asset).is_none(),
        "orphan asset removed"
    );
    for entry in walk(&ws.0) {
        if entry.ends_with("ai.json") {
            continue;
        }
        if let Ok(bytes) = fs::read(&entry) {
            assert!(!String::from_utf8_lossy(&bytes).contains(KEY), "{entry:?}");
        }
    }
}

#[test]
fn auth_failure_and_refusal_map_to_distinct_codes_and_redact_the_key() {
    let ws = Workspace::new("errors");
    let body = json!({"error":{"message":format!("API key {KEY} is invalid")}}).to_string();
    let refusal = json!({"candidates":[{"finishReason":"IMAGE_SAFETY","content":{"parts":[{"text":"cannot"}]}}]}).to_string();
    let (base, _) = mock(vec![(403, body), (200, refusal)]);
    connect(&ws, &base);
    let doc = new_doc(&ws);
    let before = fs::read(&doc).unwrap();
    let out = pentool(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "a",
            "--prompt",
            "x",
            "--allow-model-call",
            "--json",
        ],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains(KEY), "{stderr}");
    assert!(stderr.contains("provider-auth-failed"));
    let error = fail(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "b",
            "--prompt",
            "x",
            "--allow-model-call",
            "--json",
        ],
    );
    assert_eq!(error["code"], "provider-refusal");
    assert_eq!(fs::read(&doc).unwrap(), before);
}

#[test]
fn missing_credential_names_the_variable() {
    let ws = Workspace::new("missing");
    let (base, _) = mock(vec![]);
    ok(
        &ws,
        &[
            "ai",
            "connect",
            "--name",
            "gemini",
            "--endpoint",
            &base,
            "--credential-env",
            "NOT_SET_ANYWHERE",
        ],
    );
    let doc = new_doc(&ws);
    let error = fail(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "a",
            "--prompt",
            "x",
            "--allow-model-call",
            "--json",
        ],
    );
    assert_eq!(error["code"], "missing-credential");
    assert!(error["fix"].as_str().unwrap().contains("NOT_SET_ANYWHERE"));
    let doctor = ok(&ws, &["ai", "doctor"]);
    assert_eq!(doctor["ok"], false);
}

#[test]
fn keyout_makes_the_key_colour_transparent_without_a_model() {
    let ws = Workspace::new("keyout");
    let mut image = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 255, 255]));
    for x in 4..12 {
        for y in 4..12 {
            image.put_pixel(x, y, image::Rgba([30, 160, 60, 255]));
        }
    }
    let source = ws.path("magenta.png");
    image.save(&source).unwrap();
    let doc = new_doc(&ws);
    let add = pentool(
        &ws,
        &[
            "image", "add", &doc, "src", "--file", &source, "--layer", "layer-1", "--x", "0",
            "--y", "0", "--width", "100", "--height", "100", "--embed",
        ],
    );
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let result = ok(&ws, &["ai", "keyout", &doc, "cut", "--source", "src"]);
    assert_eq!(result["model_call"], false);
    assert_eq!(result["transparent_pixels"], 16 * 16 - 8 * 8);
}

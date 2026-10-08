//! v0.10.1 regression tests for the v0.9.0 audit (PEN-090-001..018).
use base64::Engine;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::{fs, thread};

const KEY: &str = "TESTKEY-abcdef0123456789";

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-v0101-{label}-{}-{}",
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

fn command(ws: &Workspace, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pentool"));
    cmd.args(args)
        .env("PENTOOL_AI_CONFIG", ws.path("ai.json"))
        .env("TEST_PROVIDER_KEY", KEY)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("PENTOOL_AI")
        .env_remove("PENTOOL_AI_ALLOW_HOSTS")
        .env_remove("PENTOOL_AI_MAX_CALLS")
        .env_remove("PENTOOL_AI_MAX_PIXELS")
        .env_remove("PENTOOL_AI_TIMEOUT_SECS");
    cmd
}

fn pentool(ws: &Workspace, args: &[&str]) -> Output {
    command(ws, args).output().unwrap()
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

fn text_of(out: &Output, args: &[&str]) -> String {
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A failing command's stderr text.
fn fail_text(ws: &Workspace, args: &[&str]) -> String {
    text_of(&pentool(ws, args), args)
}

/// A failing `--json` command's structured error.
fn fail(ws: &Workspace, args: &[&str]) -> Value {
    let mut with_json = args.to_vec();
    with_json.push("--json");
    let text = fail_text(ws, &with_json);
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

fn gemini_image(bytes: &[u8]) -> String {
    json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":base64::engine::general_purpose::STANDARD.encode(bytes)}}]}}],"usageMetadata":{"totalTokenCount":7}}).to_string()
}

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

fn new_doc(ws: &Workspace, name: &str) -> String {
    let doc = ws.path(name);
    let out = pentool(ws, &["new", &doc, "--width", "400", "--height", "300"]);
    assert!(out.status.success());
    doc
}

/// Generate one candidate (consumes one mock response) and accept it as node `node`.
fn image_node(ws: &Workspace, doc: &str, node: &str) {
    let run = ok(
        ws,
        &[
            "ai",
            "generate",
            doc,
            node,
            "--prompt",
            "x",
            "--allow-model-call",
        ],
    );
    let id = run["run"].as_str().unwrap().to_owned();
    ok(
        ws,
        &[
            "ai",
            "run",
            "accept",
            doc,
            &id,
            "--candidate",
            "1",
            "--id",
            node,
        ],
    );
}

// ---- PEN-090-014 .. 018: document-level fixes --------------------------------

#[test]
fn new_refuses_to_overwrite_and_overwrite_is_recoverable() {
    let ws = Workspace::new("new");
    let doc = new_doc(&ws, "a.pen");
    assert!(pentool(&ws, &["canvas", &doc, "--name", "Precious"])
        .status
        .success());
    let before = fs::read(&doc).unwrap();
    let text = fail_text(&ws, &["new", &doc, "--width", "10", "--height", "10"]);
    assert!(
        text.contains("already exists") && text.contains("--overwrite"),
        "{text}"
    );
    assert_eq!(before, fs::read(&doc).unwrap());

    let replaced = pentool(
        &ws,
        &[
            "new",
            &doc,
            "--width",
            "10",
            "--height",
            "10",
            "--overwrite",
        ],
    );
    assert!(replaced.status.success());
    assert_ne!(before, fs::read(&doc).unwrap());
    assert!(pentool(&ws, &["undo", &doc]).status.success());
    assert_eq!(
        before,
        fs::read(&doc).unwrap(),
        "undo restores the document"
    );
}

#[test]
fn invalid_canvas_dimensions_are_rejected_without_writing() {
    let ws = Workspace::new("canvas");
    let doc = new_doc(&ws, "a.pen");
    let before = fs::read(&doc).unwrap();
    for args in [
        vec!["canvas", &doc, "--width", "0"],
        vec!["canvas", &doc, "--height", "99999"],
        vec!["page", &doc, "add", "p2", "--width", "0"],
        vec!["page", &doc, "add", "p2", "--height", "20000"],
    ] {
        let text = fail_text(&ws, &args);
        assert!(text.contains("between 1 and 16384"), "{args:?}: {text}");
        assert_eq!(before, fs::read(&doc).unwrap(), "{args:?}");
    }
    for (w, h) in [("0", "5"), ("5", "0"), ("16385", "5")] {
        let fresh = ws.path("fresh.pen");
        let text = fail_text(&ws, &["new", &fresh, "--width", w, "--height", h]);
        assert!(text.contains("16384"), "{text}");
        assert!(!std::path::Path::new(&fresh).exists());
    }
}

#[test]
fn invalid_colors_and_blank_ids_are_rejected_without_writing() {
    let ws = Workspace::new("content");
    let doc = new_doc(&ws, "a.pen");
    let before = fs::read(&doc).unwrap();
    let text = fail_text(&ws, &["canvas", &doc, "--background", "not-a-color"]);
    assert!(
        text.contains("not a color") && text.contains("nothing was written"),
        "{text}"
    );
    let text = fail_text(
        &ws,
        &[
            "shape", &doc, "rect", "  ", "--layer", "layer-1", "--x", "1", "--y", "1", "--width",
            "5", "--height", "5",
        ],
    );
    assert!(text.contains("blank"), "{text}");
    let text = fail_text(&ws, &["layer", &doc, "add", "   "]);
    assert!(text.contains("empty") || text.contains("blank"), "{text}");
    assert_eq!(before, fs::read(&doc).unwrap());
    ok(&ws, &["canvas", &doc, "--background", "#abc"]);
    ok(&ws, &["canvas", &doc, "--background", "none"]);
}

#[test]
fn export_errors_and_help_name_pdf() {
    let ws = Workspace::new("export");
    let doc = new_doc(&ws, "a.pen");
    let text = fail_text(&ws, &["export", &doc, &ws.path("out.bmp")]);
    assert!(text.contains("pdf"), "{text}");
    let help = pentool(&ws, &["export", "--help"]);
    assert!(String::from_utf8_lossy(&help.stdout).contains("PDF"));
}

// ---- AI endpoint, policy and limit fixes -------------------------------------

#[test]
fn endpoint_secrets_in_query_or_fragment_are_rejected_and_never_echoed() {
    let ws = Workspace::new("endpoint");
    for endpoint in [
        "http://127.0.0.1:9/v1?api_key=SUPERSECRET123",
        "http://127.0.0.1:9/v1?Access-Token=SUPERSECRET123",
        "http://127.0.0.1:9/v1?x=1&key=SUPERSECRET123",
        "http://127.0.0.1:9/v1#SUPERSECRET123",
    ] {
        let out = pentool(
            &ws,
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
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "{endpoint}");
        assert!(!text.contains("SUPERSECRET123"), "{endpoint}: {text}");
    }
    assert!(!fs::read_to_string(ws.path("ai.json"))
        .unwrap_or_default()
        .contains("SUPERSECRET123"));
    ok(
        &ws,
        &[
            "ai",
            "connect",
            "--name",
            "gemini",
            "--endpoint",
            "http://127.0.0.1:9/v1?api-version=2024-01",
            "--credential-env",
            "TEST_PROVIDER_KEY",
        ],
    );
}

#[test]
fn ai_is_hidden_from_help_when_disabled() {
    let ws = Workspace::new("hidden");
    let listing = |off: bool| {
        let mut cmd = command(&ws, &["--help"]);
        if off {
            cmd.env("PENTOOL_AI", "off");
        }
        String::from_utf8_lossy(&cmd.output().unwrap().stdout).into_owned()
    };
    assert!(listing(false)
        .lines()
        .any(|l| l.trim_start().starts_with("ai ")));
    assert!(!listing(true)
        .lines()
        .any(|l| l.trim_start().starts_with("ai ")));
    let mut cmd = command(&ws, &["ai", "doctor"]);
    cmd.env("PENTOOL_AI", "off");
    assert!(!cmd.output().unwrap().status.success());
}

#[test]
fn unsupported_capability_does_not_recommend_gemini() {
    let ws = Workspace::new("capability");
    ok(
        &ws,
        &[
            "ai",
            "connect",
            "--name",
            "local",
            "--adapter",
            "openai-compatible",
            "--endpoint",
            "http://127.0.0.1:9/v1",
            "--credential-env",
            "TEST_PROVIDER_KEY",
            "--default-model",
            "m1",
        ],
    );
    let error = fail(&ws, &["ai", "resolve", "--capability", "inpaint"]);
    let fix = error["fix"].as_str().unwrap().to_ascii_lowercase();
    assert!(!fix.contains("gemini"), "{error}");
    let error = fail(
        &ws,
        &[
            "ai",
            "resolve",
            "--capability",
            "edit",
            "--provider",
            "local",
        ],
    );
    assert_eq!(error["code"], "unsupported-capability");
}

#[test]
fn dry_run_enforces_the_host_allow_list() {
    let ws = Workspace::new("hosts");
    let (base, seen) = mock(vec![]);
    connect(&ws, &base);
    let doc = new_doc(&ws, "a.pen");
    let args = [
        "ai",
        "generate",
        doc.as_str(),
        "art",
        "--prompt",
        "x",
        "--dry-run",
    ];
    let mut cmd = command(&ws, &args);
    cmd.env("PENTOOL_AI_ALLOW_HOSTS", "api.example.com");
    let text = text_of(&cmd.output().unwrap(), &args);
    assert!(
        text.contains("policy-denied") && text.contains("PENTOOL_AI_ALLOW_HOSTS"),
        "{text}"
    );
    let mut cmd = command(&ws, &args);
    cmd.env("PENTOOL_AI_ALLOW_HOSTS", "127.0.0.1");
    assert!(cmd.output().unwrap().status.success());
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn malformed_limit_variables_fail_closed() {
    let ws = Workspace::new("limits");
    let (base, seen) = mock(vec![]);
    connect(&ws, &base);
    let doc = new_doc(&ws, "a.pen");
    for (name, value) in [
        ("PENTOOL_AI_MAX_CALLS", "many"),
        ("PENTOOL_AI_MAX_CALLS", "-1"),
        ("PENTOOL_AI_MAX_PIXELS", "0"),
        ("PENTOOL_AI_MAX_PIXELS", "big"),
        ("PENTOOL_AI_TIMEOUT_SECS", "0"),
        ("PENTOOL_AI_TIMEOUT_SECS", "soon"),
    ] {
        let args = [
            "ai",
            "generate",
            doc.as_str(),
            "art",
            "--prompt",
            "x",
            "--allow-model-call",
        ];
        let mut cmd = command(&ws, &args);
        cmd.env(name, value);
        let text = text_of(&cmd.output().unwrap(), &args);
        assert!(text.contains(name), "{name}={value}: {text}");
        let mut doctor = command(&ws, &["ai", "doctor"]);
        doctor.env(name, value);
        let out = doctor.output().unwrap();
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["ok"], false, "{name}={value}");
        assert!(!report["limits"]["problems"].as_array().unwrap().is_empty());
    }
    assert!(seen.lock().unwrap().is_empty(), "no request was made");
}

#[test]
fn oversized_credential_file_keeps_its_code_and_a_fix() {
    let ws = Workspace::new("bigcred");
    let (base, _) = mock(vec![]);
    let file = ws.path("key.txt");
    fs::write(&file, "k".repeat(5000)).unwrap();
    ok(
        &ws,
        &[
            "ai",
            "connect",
            "--name",
            "gemini",
            "--endpoint",
            &base,
            "--credential-file",
            &file,
        ],
    );
    let doc = new_doc(&ws, "a.pen");
    let error = fail(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "art",
            "--prompt",
            "x",
            "--allow-model-call",
        ],
    );
    assert_eq!(error["code"], "limit-exceeded", "{error}");
    assert!(!error["fix"].as_str().unwrap_or_default().is_empty());
}

#[cfg(windows)]
#[test]
fn project_local_credential_files_are_refused_on_windows() {
    let ws = Workspace::new("wincred");
    let (base, seen) = mock(vec![]);
    fs::write(ws.path("key.txt"), KEY).unwrap();
    ok(
        &ws,
        &[
            "ai",
            "connect",
            "--name",
            "gemini",
            "--endpoint",
            &base,
            "--credential-file",
            &ws.path("key.txt"),
        ],
    );
    let doc = new_doc(&ws, "a.pen");
    let args = [
        "ai",
        "generate",
        doc.as_str(),
        "art",
        "--prompt",
        "x",
        "--allow-model-call",
    ];
    let mut cmd = command(&ws, &args);
    cmd.current_dir(&ws.0);
    let text = text_of(&cmd.output().unwrap(), &args);
    assert!(text.contains("unsafe-credential-file"), "{text}");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn default_model_accepts_a_provider_prefix_that_matches_the_name() {
    let ws = Workspace::new("model");
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
            "TEST_PROVIDER_KEY",
            "--default-model",
            "gemini/gemini-2.5-flash-image",
        ],
    );
    let config: Value =
        serde_json::from_str(&fs::read_to_string(ws.path("ai.json")).unwrap()).unwrap();
    assert_eq!(
        config["providers"]["gemini"]["default_model"],
        "gemini-2.5-flash-image"
    );
    let error = fail(
        &ws,
        &[
            "ai",
            "connect",
            "--name",
            "gemini",
            "--endpoint",
            &base,
            "--credential-env",
            "TEST_PROVIDER_KEY",
            "--default-model",
            "openai/gpt-image-1",
        ],
    );
    assert_eq!(error["code"], "invalid-model", "{error}");
}

// ---- AI run store ------------------------------------------------------------

#[test]
fn runs_are_scoped_to_their_document() {
    let ws = Workspace::new("scope");
    let (base, _) = mock(vec![(200, gemini_image(&png([200, 40, 40, 255], 8)))]);
    connect(&ws, &base);
    let a = new_doc(&ws, "a.pen");
    let b = new_doc(&ws, "b.pen");
    let run = ok(
        &ws,
        &[
            "ai",
            "generate",
            &a,
            "art",
            "--prompt",
            "x",
            "--allow-model-call",
        ],
    );
    let id = run["run"].as_str().unwrap();
    assert_eq!(
        ok(&ws, &["ai", "run", "list", &a])["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(ok(&ws, &["ai", "run", "list", &b])["runs"]
        .as_array()
        .unwrap()
        .is_empty());
    let before = fs::read(&b).unwrap();
    for args in [
        vec!["ai", "run", "accept", &b, id, "--candidate", "1"],
        vec!["ai", "run", "show", &b, id],
        vec!["ai", "run", "discard", &b, id],
    ] {
        fail(&ws, &args);
    }
    assert_eq!(before, fs::read(&b).unwrap());
    ok(&ws, &["ai", "run", "show", &a, id]);
}

#[test]
fn stale_edit_runs_cannot_silently_replace_a_newer_image() {
    let ws = Workspace::new("stale");
    let (base, _) = mock(vec![
        (200, gemini_image(&png([200, 40, 40, 255], 8))),
        (200, gemini_image(&png([40, 200, 40, 255], 8))),
        (200, gemini_image(&png([40, 40, 200, 255], 8))),
    ]);
    connect(&ws, &base);
    let doc = new_doc(&ws, "a.pen");
    image_node(&ws, &doc, "hero");
    // Start an edit from the current hero image.
    let edit = ok(
        &ws,
        &[
            "ai",
            "edit",
            &doc,
            "edit1",
            "--source",
            "hero",
            "--prompt",
            "greener",
            "--allow-model-call",
        ],
    );
    let run = edit["run"].as_str().unwrap().to_owned();
    // Meanwhile hero is replaced by a different image.
    let newer = ok(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "newer",
            "--prompt",
            "y",
            "--allow-model-call",
        ],
    )["run"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        &ws,
        &[
            "ai",
            "run",
            "accept",
            &doc,
            &newer,
            "--candidate",
            "1",
            "--replace",
            "hero",
        ],
    );
    let before = fs::read(&doc).unwrap();
    let error = fail(
        &ws,
        &[
            "ai",
            "run",
            "accept",
            &doc,
            &run,
            "--candidate",
            "1",
            "--replace",
            "hero",
        ],
    );
    assert_eq!(error["code"], "stale-source", "{error}");
    assert_eq!(before, fs::read(&doc).unwrap());
    ok(
        &ws,
        &[
            "ai",
            "run",
            "accept",
            &doc,
            &run,
            "--candidate",
            "1",
            "--replace",
            "hero",
            "--allow-stale-source",
        ],
    );
}

#[test]
fn replace_respects_locked_layers_atomically() {
    let ws = Workspace::new("lock");
    let (base, _) = mock(vec![
        (200, gemini_image(&png([200, 40, 40, 255], 8))),
        (200, gemini_image(&png([40, 40, 200, 255], 8))),
    ]);
    connect(&ws, &base);
    let doc = new_doc(&ws, "a.pen");
    image_node(&ws, &doc, "hero");
    let run = ok(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "b",
            "--prompt",
            "y",
            "--allow-model-call",
        ],
    );
    let id = run["run"].as_str().unwrap().to_owned();
    ok(&ws, &["layer", &doc, "set", "layer-1", "--locked", "true"]);
    let before = fs::read(&doc).unwrap();
    let error = fail(
        &ws,
        &[
            "ai",
            "run",
            "accept",
            &doc,
            &id,
            "--candidate",
            "1",
            "--replace",
            "hero",
        ],
    );
    assert_eq!(error["code"], "locked-layer", "{error}");
    assert_eq!(before, fs::read(&doc).unwrap());
}

#[test]
fn duplicate_candidates_are_reported_and_all_usage_is_kept() {
    let ws = Workspace::new("dupes");
    let same = gemini_image(&png([200, 40, 40, 255], 8));
    let (base, _) = mock(vec![(200, same.clone()), (200, same)]);
    connect(&ws, &base);
    let doc = new_doc(&ws, "a.pen");
    let run = ok(
        &ws,
        &[
            "ai",
            "generate",
            &doc,
            "art",
            "--prompt",
            "x",
            "--candidates",
            "2",
            "--allow-model-call",
        ],
    );
    assert_eq!(run["requested"], 2);
    assert_eq!(run["unique"], 1);
    assert_eq!(run["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(run["usage"].as_array().unwrap().len(), 2);
    assert!(run["warnings"][0].as_str().unwrap().contains("duplicate"));
    let shown = ok(
        &ws,
        &["ai", "run", "show", &doc, run["run"].as_str().unwrap()],
    );
    assert_eq!(shown["run"]["requested"], 2);
}

#[test]
fn background_removal_rejects_an_image_that_stayed_opaque() {
    let ws = Workspace::new("opaque");
    let (base, _) = mock(vec![
        (200, gemini_image(&png([200, 40, 40, 255], 8))),
        // The model answers with a flat opaque image: no key background to remove.
        (200, gemini_image(&png([40, 200, 40, 255], 8))),
    ]);
    connect(&ws, &base);
    let doc = new_doc(&ws, "a.pen");
    image_node(&ws, &doc, "hero");
    let error = fail(
        &ws,
        &[
            "ai",
            "edit",
            &doc,
            "cut",
            "--source",
            "hero",
            "--kind",
            "remove-background",
            "--allow-model-call",
        ],
    );
    assert_eq!(error["code"], "keyout-failed", "{error}");
}

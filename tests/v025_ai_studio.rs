//! Studio provider setup: connect, validate and remove through the server routes.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

const SECRET: &str = "sk-sentinel-never-echoed-4f9a";

struct Guard(Child, PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

/// A loopback provider that lists one image model and records auth headers.
fn mock_provider() -> (u16, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let body = r#"{"data":[{"id":"mock-image-1"},{"id":"mock-chat"}]}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, rx)
}

fn post(client: &reqwest::blocking::Client, base: &str, path: &str, body: Value) -> (u16, Value) {
    let response = client
        .post(format!("{base}{path}"))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .unwrap();
    let status = response.status().as_u16();
    (
        status,
        serde_json::from_str(&response.text().unwrap()).unwrap_or(Value::Null),
    )
}

#[test]
fn studio_connects_validates_and_removes_a_provider_without_exposing_the_key() {
    let dir = std::env::temp_dir().join(format!("pentool-ai-studio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let doc = dir.join("d.pen");
    assert!(Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["new", doc.to_str().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (mock_port, requests) = mock_provider();
    let _server = Guard(
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["serve", doc.to_str().unwrap(), "--port", &port.to_string()])
            .env("PENTOOL_AI_CONFIG", dir.join("ai.json"))
            .env("PENTOOL_AI_STUDIO_KEY", SECRET)
            .env_remove("PENTOOL_AI")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
        dir.clone(),
    );
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();
    for _ in 0..100 {
        if client.get(format!("{base}/api/health")).send().is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let status: Value = client
        .get(format!("{base}/api/ai/status"))
        .send()
        .unwrap()
        .text()
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap();
    assert_eq!(status["enabled"], true);
    assert_eq!(status["configured"], json!([]));
    assert!(status["catalog"].as_array().is_some_and(|c| !c.is_empty()));

    // An insecure endpoint and a missing credential are refused with stable codes.
    let (code, bad) = post(
        &client,
        &base,
        "/api/ai/connect",
        json!({"name":"mock","adapter":"openai-compatible","endpoint":"http://example.com/v1","credential_env":"PENTOOL_AI_STUDIO_KEY"}),
    );
    assert_eq!(code, 400);
    assert!(bad["code"].is_string() && bad["fix"].is_string(), "{bad}");
    let (code, bad) = post(
        &client,
        &base,
        "/api/ai/connect",
        json!({"name":"mock","adapter":"openai-compatible","endpoint":format!("http://127.0.0.1:{mock_port}/v1")}),
    );
    assert_eq!(code, 400);
    assert_eq!(bad["code"], "missing-credential");
    // A key pasted into the form is not a field the route accepts.
    let (code, _) = post(
        &client,
        &base,
        "/api/ai/connect",
        json!({"name":"mock","adapter":"openai-compatible","api_key":SECRET}),
    );
    assert_ne!(code, 200);

    let endpoint = format!("http://127.0.0.1:{mock_port}/v1");
    let (code, added) = post(
        &client,
        &base,
        "/api/ai/connect",
        json!({"name":"mock","adapter":"openai-compatible","endpoint":endpoint,"credential_env":"PENTOOL_AI_STUDIO_KEY"}),
    );
    assert_eq!(code, 200, "{added}");
    assert_eq!(added["status"], "added");
    let (_, ghost) = post(
        &client,
        &base,
        "/api/ai/connect",
        json!({"name":"ghost","adapter":"openai-compatible","endpoint":endpoint,"credential_env":"PENTOOL_AI_NOT_SET"}),
    );
    assert_eq!(ghost["status"], "added");

    // Offline validation reports the unset variable; the network is not touched.
    let (_, offline) = post(
        &client,
        &base,
        "/api/ai/doctor",
        json!({"provider":"ghost"}),
    );
    assert_eq!(offline["providers"][0]["ready"], false);
    assert_eq!(
        offline["providers"][0]["problems"][0]["code"],
        "missing-credential"
    );
    assert!(
        requests.try_recv().is_err(),
        "doctor without check contacted the provider"
    );

    // The explicit check makes one model-list call with the key.
    let (_, checked) = post(
        &client,
        &base,
        "/api/ai/doctor",
        json!({"provider":"mock","check":true}),
    );
    assert_eq!(checked["providers"][0]["check"]["ok"], true, "{checked}");
    assert_eq!(checked["providers"][0]["check"]["image_models"], 1);
    let seen = requests
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(seen.starts_with("GET /v1/models"), "{seen}");

    let listed: Value = client
        .get(format!("{base}/api/ai/status"))
        .send()
        .unwrap()
        .text()
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap();
    assert_eq!(listed["configured"].as_array().unwrap().len(), 2);
    for body in [added.to_string(), checked.to_string(), listed.to_string()] {
        assert!(!body.contains(SECRET), "a response exposed the key");
    }
    assert!(!std::fs::read_to_string(dir.join("ai.json"))
        .unwrap()
        .contains(SECRET));

    let (code, removed) = post(&client, &base, "/api/ai/remove", json!({"name":"ghost"}));
    assert_eq!(code, 200, "{removed}");
    let (code, missing) = post(&client, &base, "/api/ai/remove", json!({"name":"ghost"}));
    assert_eq!(code, 400);
    assert_eq!(missing["code"], "unknown-provider");
}

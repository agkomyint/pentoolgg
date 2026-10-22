//! Studio provider setup: connect, validate and remove through the server routes.
use base64::Engine;
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
fn mock_provider() -> (
    u16,
    std::sync::mpsc::Receiver<String>,
    std::sync::mpsc::Sender<()>,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buf = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&buf[..end]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + length {
                        break;
                    }
                }
                assert!(buf.len() < 100_000);
            }
            let request = String::from_utf8_lossy(&buf);
            let _ = tx.send(request.to_string());
            if request.contains("slow image") {
                let _ = release_rx.recv_timeout(std::time::Duration::from_secs(30));
            }
            let body = if request.starts_with("POST /v1/images/generations")
                || request.contains(":generateContent")
            {
                let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([32, 140, 200, 255]));
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(image)
                    .write_to(&mut png, image::ImageFormat::Png)
                    .unwrap();
                let data = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
                if request.contains(":generateContent") {
                    json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":data}}]}}]}).to_string()
                } else {
                    json!({"data":[{"b64_json":data}]}).to_string()
                }
            } else {
                json!({"data":[{"id":"mock-image-1"},{"id":"mock-chat"}]}).to_string()
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, rx, release_tx)
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
    let (mock_port, requests, release_provider) = mock_provider();
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

    // Review is offline and an unconfirmed live request never contacts the model.
    let before = std::fs::read(&doc).unwrap();
    let request =
        json!({"name":"hero","provider":"mock","model":"mock-image-1","prompt":"A blue square"});
    let mut review = request.clone();
    review["dry_run"] = json!(true);
    let (code, plan) = post(&client, &base, "/api/ai/generate", review);
    assert_eq!(code, 200, "{plan}");
    assert_eq!(plan["contacted_provider"], false);
    assert!(requests.try_recv().is_err());
    let (code, refused) = post(&client, &base, "/api/ai/generate", request.clone());
    assert_eq!(code, 400);
    assert_eq!(refused["code"], "model-call-not-allowed");
    assert!(requests.try_recv().is_err());
    let mut unreviewed = request.clone();
    unreviewed["allow_model_call"] = json!(true);
    let (code, refused) = post(&client, &base, "/api/ai/generate", unreviewed);
    assert_eq!(code, 400, "{refused}");
    assert!(
        requests.try_recv().is_err(),
        "unreviewed request contacted a provider"
    );
    let mut confirmed = request;
    confirmed["allow_model_call"] = json!(true);
    confirmed["reviewed_disclosure"] = plan["disclosure"].clone();
    let (code, generated) = post(&client, &base, "/api/ai/generate", confirmed);
    assert_eq!(code, 200, "{generated}");
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    assert!(requests
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .starts_with("POST /v1/images/generations"));
    let run = generated["run"].as_str().unwrap();
    let listing: Value = client
        .get(format!("{base}/api/ai/runs"))
        .send()
        .unwrap()
        .text()
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap();
    assert_eq!(listing["runs"][0]["run"], run);
    let candidate = client
        .get(format!("{base}/api/ai/runs/{run}/candidates/1"))
        .send()
        .unwrap();
    assert!(candidate.status().is_success());
    assert_eq!(
        image::load_from_memory(&candidate.bytes().unwrap())
            .unwrap()
            .width(),
        4
    );
    let loaded: Value = serde_json::from_str(
        &client
            .get(format!("{base}/api/document"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
    )
    .unwrap();
    let revision = loaded["revision"].clone();
    let path = format!("/api/ai/runs/{run}/accept");
    let (_, stale) = post(
        &client,
        &base,
        &path,
        json!({"candidate":1,"id":"hero","if_revision":"stale"}),
    );
    assert!(stale["error"].is_string());
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    let (code, accepted) = post(
        &client,
        &base,
        &path,
        json!({"candidate":1,"id":"hero","if_revision":revision}),
    );
    assert_eq!(code, 200, "{accepted}");
    let saved = std::fs::read(&doc).unwrap();
    assert_ne!(saved, before);
    let saved_value: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(
        saved_value["pages"][0]["layers"][0]["nodes"][0]["ai"]["provider"],
        "mock"
    );
    assert!(!String::from_utf8_lossy(&saved).contains("A blue square"));
    // An edited-image run records the source hash. Another actor replacing
    // that source must be rejected even with the newest document revision.
    let (code, connected) = post(
        &client,
        &base,
        "/api/ai/connect",
        json!({"name":"gemini-mock","adapter":"gemini","endpoint":endpoint,"credential_env":"PENTOOL_AI_STUDIO_KEY","default_model":"mock-image"}),
    );
    assert_eq!(code, 200, "{connected}");
    let edit =
        json!({"name":"variation","provider":"gemini-mock","source":"hero","prompt":"variation"});
    let mut review = edit.clone();
    review["dry_run"] = json!(true);
    let (code, edit_plan) = post(&client, &base, "/api/ai/generate", review);
    assert_eq!(code, 200, "{edit_plan}");
    let mut edit = edit;
    edit["allow_model_call"] = json!(true);
    edit["reviewed_disclosure"] = edit_plan["disclosure"].clone();
    let (code, edited) = post(&client, &base, "/api/ai/generate", edit);
    assert_eq!(code, 200, "{edited}");
    requests
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 40, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let bytes = png.into_inner();
    let mut changed = saved_value.clone();
    pentool::image::add(
        &mut changed,
        None,
        "layer-1",
        "other-source",
        &bytes,
        pentool::image::embedded_storage(&bytes),
        0.0,
        0.0,
        4.0,
        4.0,
        pentool::image::Fit::Fill,
    )
    .unwrap();
    let asset = changed["pages"][0]["layers"][0]["nodes"][1]["asset"].clone();
    changed["pages"][0]["layers"][0]["nodes"][0]["asset"] = asset;
    // Provenance belongs to the earlier pixels; the external actor removes it.
    changed["pages"][0]["layers"][0]["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("ai");
    let changed = serde_json::to_vec_pretty(&changed).unwrap();
    std::fs::write(&doc, &changed).unwrap();
    let loaded: Value = serde_json::from_str(
        &client
            .get(format!("{base}/api/document"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
    )
    .unwrap();
    let (code, stale_source) = post(
        &client,
        &base,
        &format!("/api/ai/runs/{}/accept", edited["run"].as_str().unwrap()),
        json!({"candidate":1,"replace":"hero","if_revision":loaded["revision"]}),
    );
    assert_eq!(code, 400);
    assert_eq!(stale_source["code"], "stale-source", "{stale_source}");
    assert_eq!(std::fs::read(&doc).unwrap(), changed);
    std::fs::write(&doc, &saved).unwrap();
    post(
        &client,
        &base,
        &format!("/api/ai/runs/{}/discard", edited["run"].as_str().unwrap()),
        json!({}),
    );

    let (code, undone) = post(&client, &base, "/api/undo", json!({}));
    assert_eq!(code, 200, "{undone}");
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    let (code, discarded) = post(
        &client,
        &base,
        &format!("/api/ai/runs/{run}/discard"),
        json!({}),
    );
    assert_eq!(code, 200, "{discarded}");
    assert_eq!(std::fs::read(&doc).unwrap(), before);

    // Cancellation stops a pending generation before it can save a run.
    let (_, slow_plan) = post(
        &client,
        &base,
        "/api/ai/generate",
        json!({"name":"cancelled","provider":"mock","model":"mock-image-1","prompt":"slow image","dry_run":true}),
    );
    let pending_client = client.clone();
    let pending_base = base.clone();
    let pending = std::thread::spawn(move || {
        post(
            &pending_client,
            &pending_base,
            "/api/ai/generate",
            json!({"name":"cancelled","provider":"mock","model":"mock-image-1","prompt":"slow image",
            "job":"cancel-test","allow_model_call":true,"reviewed_disclosure":slow_plan["disclosure"]}),
        )
    });
    requests
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let (_, preview_cancel) = post(
        &client,
        &base,
        "/api/composite/cancel",
        json!({"job":"cancel-test"}),
    );
    assert_eq!(
        preview_cancel["cancelled"], false,
        "preview cancellation reached an AI job"
    );
    let (_, cancelled) = post(
        &client,
        &base,
        "/api/ai/cancel",
        json!({"job":"cancel-test"}),
    );
    assert_eq!(cancelled["cancelled"], true);
    release_provider.send(()).unwrap();
    let (code, failure) = pending.join().unwrap();
    assert_eq!(code, 400, "{failure}");
    assert!(failure["error"].as_str().unwrap().contains("cancelled"));
    let listing: Value = serde_json::from_str(
        &client
            .get(format!("{base}/api/ai/runs"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(listing["runs"], json!([]));
    assert_eq!(std::fs::read(&doc).unwrap(), before);

    let (code, removed) = post(&client, &base, "/api/ai/remove", json!({"name":"ghost"}));
    assert_eq!(code, 200, "{removed}");
    let (code, missing) = post(&client, &base, "/api/ai/remove", json!({"name":"ghost"}));
    assert_eq!(code, 400);
    assert_eq!(missing["code"], "unknown-provider");
}

//! Hostile-input, concurrency, and benchmark-bound coverage for v0.7.0.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);

impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-v070h-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let out = run(args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn png(size: u32) -> Vec<u8> {
    let mut image = ::image::RgbaImage::new(size, size);
    for (x, y, px) in image.enumerate_pixels_mut() {
        *px = ::image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 199) as u8, 255]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, ::image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn add_args<'a>(doc: &'a str, id: &'a str, file: &'a str) -> Vec<&'a str> {
    vec![
        "image", "add", doc, id, "--file", file, "--layer", "content", "--width", "200",
        "--height", "200", "--embed",
    ]
}

fn new_document(ws: &Workspace, file: &str, image: &[u8]) -> PathBuf {
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    fs::write(ws.path(file), image).unwrap();
    ok(&add_args(
        doc.to_str().unwrap(),
        "hero",
        ws.path(file).to_str().unwrap(),
    ));
    doc
}

#[test]
fn image_benchmark_is_bounded_and_cache_state_never_changes_output() {
    let report = ok(&[
        "benchmark",
        "--images",
        "4",
        "--source-size",
        "32",
        "--operations",
        "2",
        "--repetitions",
        "2",
    ]);
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0]["cache"], "cold");
    assert_eq!(runs[1]["cache"], "warm");
    assert!(runs
        .iter()
        .all(|r| r["output_bytes"] == runs[0]["output_bytes"]));
    assert_eq!(report["fixture"]["reuse_count"], 4);
    for bad in [["0", "8"], ["5000", "8"], ["2", "99999"]] {
        let message = fails(&["benchmark", "--images", bad[0], "--source-size", bad[1]]);
        assert!(message.contains("image benchmark supports"), "{message}");
    }
    assert!(fails(&["benchmark", "--images", "1", "--operations", "99"]).contains("operations"));
}

#[test]
fn hostile_inputs_fail_without_touching_the_document() {
    let ws = Workspace::new("hostile");
    let good = png(16);
    let doc = new_document(&ws, "ok.png", &good);
    let before = fs::read(&doc).unwrap();
    let d = doc.to_str().unwrap();

    // Truncated, garbage, and zero-length sources.
    let mut bomb = good.clone();
    bomb[16..20].copy_from_slice(&0x7fff_ffffu32.to_be_bytes());
    bomb[20..24].copy_from_slice(&0x7fff_ffffu32.to_be_bytes());
    for (name, bytes) in [
        ("truncated.png", good[..good.len() / 2].to_vec()),
        ("garbage.png", vec![0x42; 4096]),
        ("empty.png", Vec::new()),
        ("bomb.png", bomb),
    ] {
        fs::write(ws.path(name), bytes).unwrap();
        fails(&add_args(d, "bad", ws.path(name).to_str().unwrap()));
        assert_eq!(
            fs::read(&doc).unwrap(),
            before,
            "{name} mutated the document"
        );
    }

    // A malicious operation stack of 65 entries is rejected as a whole.
    let mut value: Value = serde_json::from_slice(&before).unwrap();
    let operations: Vec<Value> = (0..65)
        .map(|i| serde_json::json!({"id":format!("o{i}"),"kind":"grayscale","enabled":true,"params":{}}))
        .collect();
    fn set(nodes: &mut [Value], ops: &[Value]) {
        for node in nodes {
            if node["id"] == "hero" {
                node["operations"] = Value::Array(ops.to_vec());
            }
            if let Some(children) = node["children"].as_array_mut() {
                set(children, ops);
            }
        }
    }
    set(
        value["pages"][0]["layers"][0]["nodes"]
            .as_array_mut()
            .unwrap(),
        &operations,
    );
    let hostile = ws.path("hostile.pen");
    fs::write(&hostile, serde_json::to_vec(&value).unwrap()).unwrap();
    let hostile_before = fs::read(&hostile).unwrap();
    let h = hostile.to_str().unwrap();
    fails(&["image", "op", "add", h, "hero", "grayscale"]);
    fails(&["export", h, ws.path("o.png").to_str().unwrap()]);
    assert_eq!(fs::read(&hostile).unwrap(), hostile_before);
    assert!(
        !ws.path("o.png").exists(),
        "failed export must not leave output"
    );

    // Path traversal in external storage is rejected.
    let mut value: Value = serde_json::from_slice(&before).unwrap();
    let digest = value["image_assets"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    value["image_assets"][&digest]["storage"] =
        serde_json::json!({"kind":"external","path":"../../etc/passwd"});
    let traversal = ws.path("traversal.pen");
    fs::write(&traversal, serde_json::to_vec(&value).unwrap()).unwrap();
    fails(&[
        "export",
        traversal.to_str().unwrap(),
        ws.path("t.png").to_str().unwrap(),
    ]);
    assert!(!ws.path("t.png").exists());
}

#[test]
fn concurrent_exports_share_the_processed_cache_safely() {
    let ws = Workspace::new("concurrent");
    let doc = new_document(&ws, "big.png", &png(320));
    let d = doc.to_str().unwrap().to_owned();
    ok(&["image", "op", "add", &d, "hero", "blur", "--radius", "2"]);
    let outputs: Vec<PathBuf> = (0..4).map(|i| ws.path(&format!("out{i}.png"))).collect();
    let handles: Vec<_> = outputs
        .iter()
        .map(|output| {
            let (d, output) = (d.clone(), output.to_str().unwrap().to_owned());
            std::thread::spawn(move || {
                let out = run(&["export", &d, &output]);
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let first = fs::read(&outputs[0]).unwrap();
    assert!(outputs.iter().all(|o| fs::read(o).unwrap() == first));

    // Junk records and leftover partial files never change output.
    let cache = ws.0.join(".pentool/cache/processed");
    if cache.exists() {
        for entry in fs::read_dir(&cache).unwrap() {
            fs::write(entry.unwrap().path(), b"junk").unwrap();
        }
        fs::write(cache.join("deadbeef.tmp"), b"partial").unwrap();
    }
    let again = ws.path("again.png");
    ok(&["export", &d, again.to_str().unwrap()]);
    assert_eq!(fs::read(&again).unwrap(), first);
}

#[test]
fn proxy_preview_is_smaller_approximate_and_never_alters_authoritative_output() {
    let ws = Workspace::new("proxy");
    let doc = new_document(&ws, "big.png", &png(320));
    let raw: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let full = pentool::image::to_svg(&raw, &doc, None).unwrap();
    let proxy = pentool::image::to_svg_proxy(&raw, &doc, None, 32).unwrap();
    assert_eq!((full.width, full.height), (proxy.width, proxy.height));
    assert!(proxy.svg.len() < full.svg.len() / 2);
    // Geometry attributes are identical; only embedded pixel detail differs.
    let geometry = |svg: &str| svg.split("href=").next().unwrap().to_owned();
    assert_eq!(geometry(&full.svg), geometry(&proxy.svg));
    assert_eq!(
        pentool::image::to_svg(&raw, &doc, None).unwrap().svg,
        full.svg
    );
    for edge in [0, 15, 4097] {
        assert!(pentool::image::to_svg_proxy(&raw, &doc, None, edge).is_err());
    }
}

struct Server(std::process::Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn browser_endpoints_use_shared_services_and_label_preview_status() {
    let ws = Workspace::new("browser");
    let doc = new_document(&ws, "big.png", &png(96));
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["serve", doc.to_str().unwrap(), "--port", &port.to_string()])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();
    let mut ready = false;
    for _ in 0..100 {
        if client.get(format!("{base}/api/health")).send().is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(ready, "server did not start");
    let script = client.get(format!("{base}/image-panel.js")).send().unwrap();
    assert!(script.status().is_success());

    let shared: Value = client
        .get(format!("{base}/api/document"))
        .send()
        .unwrap()
        .text()
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap();
    let revision = shared["revision"].as_str().unwrap().to_owned();
    let document = shared["document"].clone();

    // Preview status: proxy is labelled approximate, default is authoritative.
    let proxy = client
        .post(format!("{base}/api/render/svg?max_edge=32"))
        .header("content-type", "application/json")
        .body(document.to_string())
        .send()
        .unwrap();
    assert_eq!(proxy.headers()["x-pentool-preview"], "approximate");
    let exact = client
        .post(format!("{base}/api/render/svg"))
        .header("content-type", "application/json")
        .body(document.to_string())
        .send()
        .unwrap();
    assert_eq!(exact.headers()["x-pentool-preview"], "authoritative");

    // Operation edits commit through the shared batch engine as one history entry.
    let added = client
        .post(format!("{base}/api/scene"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({"revision":revision,"operations":[
            {"type":"image-op-add","id":"hero","op":"grayscale","op_id":"g"}]})
            .to_string(),
        )
        .send()
        .unwrap();
    assert!(added.status().is_success(), "{:?}", added.text());
    let stale = client
        .post(format!("{base}/api/scene"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({"revision":revision,"operations":[
            {"type":"image-op-remove","id":"hero","op_id":"g"}]})
            .to_string(),
        )
        .send()
        .unwrap();
    assert!(
        !stale.status().is_success(),
        "stale revision must be rejected"
    );

    // Bake: dry run leaves the file alone; the real run flattens and prunes the stack.
    let before = fs::read(&doc).unwrap();
    let shared: Value = client
        .get(format!("{base}/api/document"))
        .send()
        .unwrap()
        .text()
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap();
    let revision = shared["revision"].as_str().unwrap().to_owned();
    let dry = client
        .post(format!("{base}/api/image/bake"))
        .header("content-type", "application/json")
        .body(serde_json::json!({"id":"hero","revision":revision,"dry_run":true}).to_string())
        .send()
        .unwrap();
    assert!(dry.status().is_success());
    assert_eq!(fs::read(&doc).unwrap(), before);
    let done = client
        .post(format!("{base}/api/image/bake"))
        .header("content-type", "application/json")
        .body(serde_json::json!({"id":"hero","revision":revision}).to_string())
        .send()
        .unwrap();
    assert!(done.status().is_success());
    let baked: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let hero = serde_json::to_string(&baked).unwrap();
    assert!(hero.contains("\"operations\":[]"));
}

#[test]
fn palette_handoff_is_explicit_transactional_and_analysis_stays_read_only() {
    let ws = Workspace::new("palette");
    let doc = new_document(&ws, "p.png", &png(48));
    let d = doc.to_str().unwrap();
    let before = fs::read(&doc).unwrap();
    ok(&["image", "analyze", d, "hero"]);
    assert_eq!(fs::read(&doc).unwrap(), before, "analyze must not write");
    ok(&[
        "image",
        "palette",
        d,
        "hero",
        "--prefix",
        "brand",
        "--dry-run",
    ]);
    assert_eq!(fs::read(&doc).unwrap(), before, "dry run must not write");
    let report = ok(&[
        "image", "palette", d, "hero", "--prefix", "brand", "--count", "2",
    ]);
    assert_eq!(report["result"]["tokens"].as_array().unwrap().len(), 2);
    let value: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    assert_eq!(value["styles"]["brand-1"]["type"], "color");
    assert!(value["styles"]["brand-2"]["value"]
        .as_str()
        .unwrap()
        .starts_with('#'));
    fails(&[
        "image", "palette", d, "hero", "--prefix", "", "--count", "9",
    ]);
    ok(&["undo", d]);
    assert_eq!(
        fs::read(&doc).unwrap(),
        before,
        "one undo restores the document"
    );
}

/// Deterministic mutation sweep: decoders and validators must return errors, never panic.
#[test]
fn mutated_sources_and_operation_stacks_never_panic() {
    let good = png(24);
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut accepted = 0;
    for round in 0..600 {
        let mut bytes = good.clone();
        match round % 3 {
            0 => bytes.truncate((next() % bytes.len() as u64) as usize),
            1 => {
                for _ in 0..1 + next() % 4 {
                    let at = (next() % bytes.len() as u64) as usize;
                    bytes[at] ^= 1 << (next() % 8);
                }
            }
            _ => {
                let at = (next() % bytes.len() as u64) as usize;
                bytes[at..].fill((next() & 0xff) as u8);
            }
        }
        if pentool::image::decode_source(&bytes).is_ok() {
            accepted += 1;
        }
    }
    assert!(accepted < 600);

    let kinds = [
        "crop",
        "resize",
        "rotate",
        "brightness-contrast",
        "levels",
        "curves",
        "hue-saturation",
        "blur",
        "sharpen",
        "grayscale",
        "bogus",
    ];
    let wild = [
        serde_json::json!(-1e300),
        serde_json::json!(1e300),
        serde_json::json!(0),
        serde_json::json!(f64::MAX),
        serde_json::json!("x"),
        serde_json::json!(null),
        serde_json::json!(4294967296u64),
    ];
    for round in 0..500 {
        let stack: Vec<Value> = (0..1 + next() % 70)
            .map(|i| {
                let mut params = serde_json::Map::new();
                for key in ["x", "y", "width", "height", "radius", "amount", "degrees", "gamma", "black", "white", "points"] {
                    if next() % 3 == 0 {
                        params.insert(key.into(), wild[(next() % wild.len() as u64) as usize].clone());
                    }
                }
                serde_json::json!({"id":format!("o{round}-{i}"),"kind":kinds[(next() % kinds.len() as u64) as usize],
                    "enabled":true,"params":params})
            })
            .collect();
        let _ = pentool::imageops::validate_stack(&stack, 64, 64);
    }
}

#[test]
fn browser_frame_controls_commit_through_set_image_with_validation() {
    let ws = Workspace::new("frame");
    let doc = new_document(&ws, "f.png", &png(32));
    let d = doc.to_str().unwrap();
    let ops = ws.path("ops.json");
    fs::write(
        &ops,
        r#"[{"type":"set-image","id":"hero","fit":"cover","opacity":0.5,"position":[0.25,0.75],"crop":[0.1,0.1,0.5,0.5]}]"#,
    )
    .unwrap();
    ok(&["batch", d, ops.to_str().unwrap()]);
    let value: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let hero = &value["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "hero")
        .unwrap()
        .clone();
    assert_eq!(hero["fit"], "cover");
    assert_eq!(hero["position"], serde_json::json!([0.25, 0.75]));
    assert_eq!(hero["crop"], serde_json::json!([0.1, 0.1, 0.5, 0.5]));
    let before = fs::read(&doc).unwrap();
    for bad in [
        r#"[{"type":"set-image","id":"hero","crop":[0.1,0.1]}]"#,
        r#"[{"type":"set-image","id":"hero","crop":[0.9,0.9,0.5,0.5]}]"#,
        r#"[{"type":"set-image","id":"hero","position":[2,0]}]"#,
    ] {
        fs::write(&ops, bad).unwrap();
        fails(&["batch", d, ops.to_str().unwrap()]);
        assert_eq!(fs::read(&doc).unwrap(), before);
    }
}

#[test]
fn cache_records_and_packages_reject_every_mutation() {
    // Build a valid record through the public cache path, then mutate it.
    let ws = Workspace::new("records");
    let doc = new_document(&ws, "big.png", &png(320));
    let d = doc.to_str().unwrap();
    ok(&["image", "op", "add", d, "hero", "blur", "--radius", "1"]);
    ok(&["export", d, ws.path("o.png").to_str().unwrap()]);
    let record = fs::read_dir(ws.0.join(".pentool/cache/processed"))
        .unwrap()
        .next()
        .expect("a processed record was written")
        .unwrap()
        .path();
    let good = fs::read(&record).unwrap();
    assert!(pentool::image::parse_processed_record(&good).is_some());
    for at in (0..good.len()).step_by(good.len() / 97 + 1) {
        let mut bad = good.clone();
        bad[at] ^= 0x40;
        assert!(
            pentool::image::parse_processed_record(&bad).is_none(),
            "flip at {at}"
        );
    }
    for cut in [0, 7, 16, 40, good.len() - 1] {
        assert!(pentool::image::parse_processed_record(&good[..cut]).is_none());
    }

    // A packed archive survives verify; truncated or flipped copies fail without panics.
    let kit = ws.path("kit");
    ok(&[
        "package",
        "init",
        kit.to_str().unwrap(),
        "--name",
        "photo-kit",
    ]);
    let mut raw: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    raw["asset"] = serde_json::json!({"schema":1,"id":"hero-card","name":"Hero card","asset_version":"0.1.0","kind":"component"});
    fs::write(
        kit.join("assets/hero.pen"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let package = ws.path("a.penpkg");
    ok(&[
        "package",
        "pack",
        kit.to_str().unwrap(),
        "--output",
        package.to_str().unwrap(),
    ]);
    let bytes = fs::read(&package).unwrap();
    ok(&["package", "verify", package.to_str().unwrap()]);
    let mutated = ws.path("m.penpkg");
    let mut rejected = 0;
    for cut in [10, bytes.len() / 2, bytes.len() - 5] {
        fs::write(&mutated, &bytes[..cut]).unwrap();
        assert!(!run(&["package", "verify", mutated.to_str().unwrap()])
            .status
            .success());
        rejected += 1;
    }
    for at in (0..bytes.len()).step_by(bytes.len() / 23 + 1) {
        let mut bad = bytes.clone();
        bad[at] ^= 0x55;
        fs::write(&mutated, &bad).unwrap();
        let out = run(&["package", "verify", mutated.to_str().unwrap()]);
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("panicked"),
            "panic when flipping byte {at}"
        );
        if !out.status.success() {
            rejected += 1;
        }
    }
    assert!(rejected > 3);
}

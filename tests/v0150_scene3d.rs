//! v0.15.0: version 10 scene3d nodes, the models map, migration, and the CLI.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

const TETRA: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\nf 1 2 4\nf 1 4 3\nf 2 3 4\n";

fn scratch() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pentool-scene3d-{}-{nonce}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir(&dir).unwrap();
    dir
}

fn pentool(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn base(dir: &Path) -> PathBuf {
    let doc = json!({
        "format": "pentool", "version": 4, "name": "t",
        "pages": [{"id": "p", "name": "P",
            "canvas": {"width": 120, "height": 120, "background": "#ffffff"},
            "layers": [{"id": "l", "name": "L", "visible": true, "locked": false, "nodes": []}]}],
        "styles": {}
    });
    let path = dir.join("d.pen");
    std::fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    let out = pentool(&["migrate", path.to_str().unwrap(), "--target", "10"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    path
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

struct TestServer(std::process::Child);

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_server(dir: &Path, port: u16) -> TestServer {
    let child = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["serve", "doc.pen", "--port", &port.to_string()])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let server = TestServer(child);
    let client = reqwest::blocking::Client::new();
    let health = format!("http://127.0.0.1:{port}/api/health");
    assert!(
        (0..100).any(|_| {
            let up = client.get(&health).send().is_ok();
            if !up {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            up
        }),
        "server did not start"
    );
    server
}

#[test]
fn scene3d_render_cache_is_discarded_when_server_process_restarts() {
    let dir = scratch();
    let mut document: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let product = document["pages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|page| page["id"] == "products")
        .unwrap();
    let bottle = product["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "perfume-preview")
        .unwrap();
    // Keep the valid product form/model, but use an unbound render so it is
    // eligible for the PNG cache rather than the linked-art bypass path.
    bottle.as_object_mut().unwrap().remove("surface_graphics");
    std::fs::write(dir.join("doc.pen"), serde_json::to_vec(&document).unwrap()).unwrap();

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();
    let request = serde_json::json!({
        "document": document,
        "page": "products",
        "node": "perfume-preview"
    });
    let request_body = serde_json::to_vec(&request).unwrap();

    let first_output;
    {
        let _server = start_server(&dir, port);
        let first = client
            .post(format!("{base}/api/scene3d/render-report"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_body.clone())
            .send()
            .unwrap();
        let first_text = first.text().unwrap();
        let first: Value = serde_json::from_str(&first_text)
            .unwrap_or_else(|_| panic!("render report failed: {first_text}"));
        assert_eq!(first["render"]["cache_hit"], false);
        first_output = first["render"]["output"].clone();

        let cached = client
            .post(format!("{base}/api/scene3d/render-report"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_body.clone())
            .send()
            .unwrap();
        let cached: Value = serde_json::from_str(&cached.text().unwrap()).unwrap();
        assert_eq!(cached["render"]["cache_hit"], true);
        let status: Value = serde_json::from_str(
            &client
                .get(format!("{base}/api/scene3d/cache"))
                .send()
                .unwrap()
                .text()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(status["entries"], 1);
        assert!(status["bytes"].as_u64().unwrap() > 0);
        pentool::scene3d::cli::run_cache_command(&base, pentool::scene3d::cli::CacheAction::Status)
            .unwrap();
        pentool::scene3d::cli::run_cache_command(&base, pentool::scene3d::cli::CacheAction::Clear)
            .unwrap();
        let cleared: Value = serde_json::from_str(
            &client
                .get(format!("{base}/api/scene3d/cache"))
                .send()
                .unwrap()
                .text()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(cleared["entries"], 0);
        assert_eq!(cleared["bytes"], 0);
    }

    {
        let _server = start_server(&dir, port);
        let status: Value = serde_json::from_str(
            &client
                .get(format!("{base}/api/scene3d/cache"))
                .send()
                .unwrap()
                .text()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(status["entries"], 0);
        assert_eq!(status["bytes"], 0);

        let after_restart = client
            .post(format!("{base}/api/scene3d/render-report"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_body)
            .send()
            .unwrap();
        let after_restart: Value = serde_json::from_str(&after_restart.text().unwrap()).unwrap();
        assert_eq!(after_restart["render"]["cache_hit"], false);
        assert_eq!(after_restart["render"]["output"], first_output);
    }
}

#[test]
fn add_camera_light_and_export_render_the_model() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    let out = pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--width",
        "100",
        "--height",
        "100",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["report"]["triangles"], 4);
    assert_eq!(read(&doc)["version"], 10);

    assert!(pentool(&["scene3d", d, "camera", "m", "--azimuth", "30"])
        .status
        .success());
    assert!(pentool(&[
        "scene3d",
        d,
        "light",
        "m",
        "add",
        "--kind",
        "point",
        "--position",
        "1,2,3"
    ])
    .status
    .success());
    let png = dir.join("o.png");
    let out = pentool(&["export", d, png.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first = std::fs::read(&png).unwrap();
    assert!(pentool(&["export", d, png.to_str().unwrap()])
        .status
        .success());
    assert_eq!(
        first,
        std::fs::read(&png).unwrap(),
        "export must be deterministic"
    );
}

#[test]
fn per_node_scene3d_edits_respect_locked_host_layer() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    let added = pentool(&[
        "scene3d",
        d,
        "add",
        "hero",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
    ]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let locked = pentool(&["layer", d, "set", "l", "--locked", "true"]);
    assert!(
        locked.status.success(),
        "{}",
        String::from_utf8_lossy(&locked.stderr)
    );
    let before = std::fs::read(&doc).unwrap();

    for args in [
        vec!["camera", "hero", "--azimuth", "30"],
        vec!["frame", "hero"],
        vec!["set", "hero", "--exposure-ev", "1.5"],
        vec!["environment", "hero", "--sky", "#ffffff"],
        vec![
            "light",
            "hero",
            "add",
            "--kind",
            "directional",
            "--direction",
            "1,2,3",
        ],
    ] {
        let mut command = vec!["scene3d", d];
        command.extend(args);
        let out = pentool(&command);
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{command:?} unexpectedly succeeded");
        assert!(error.contains("[locked]"), "{command:?}: {error}");
        assert_eq!(
            std::fs::read(&doc).unwrap(),
            before,
            "{command:?} changed the file"
        );
    }
}

#[test]
fn dry_run_and_bad_input_leave_the_file_untouched() {
    let dir = scratch();
    let doc = base(&dir);
    let before = std::fs::read(&doc).unwrap();
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    let out = pentool(&[
        "scene3d",
        d,
        "--dry-run",
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
    ]);
    assert!(out.status.success());
    assert_eq!(before, std::fs::read(&doc).unwrap());
    let bad = dir.join("bad.obj");
    std::fs::write(&bad, "f 1 2 3\n").unwrap();
    let out = pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        bad.to_str().unwrap(),
        "--layer",
        "l",
    ]);
    assert!(!out.status.success());
    assert_eq!(before, std::fs::read(&doc).unwrap());
}

#[test]
fn material_overrides_are_atomic_and_inspection_is_paginated() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("painted.obj");
    let mtl = dir.join("painted.mtl");
    std::fs::write(&mtl, "newmtl clay\nKd 0.8 0.2 0.1\nNs 20\n").unwrap();
    std::fs::write(
        &model,
        "mtllib painted.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl clay\nf 1 3 2\n",
    )
    .unwrap();
    let d = doc.to_str().unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--external"
    ])
    .status
    .success());

    let inspected = pentool(&[
        "scene3d", d, "inspect", "m", "--offset", "1", "--limit", "1",
    ]);
    assert!(
        inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    let report: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(report["matches"], 3);
    assert_eq!(report["returned"], 1);
    assert_eq!(report["offset"], 1);
    assert_eq!(report["limit"], 1);
    assert_eq!(report["has_more"], true);
    assert_eq!(report["records"][0]["id"], "material-0");

    assert!(pentool(&[
        "scene3d",
        d,
        "material",
        "m",
        "set",
        "0",
        "--base-color",
        "#2080ffff",
        "--metallic",
        "0.25",
        "--roughness",
        "0.4",
        "--double-sided",
        "true"
    ])
    .status
    .success());
    let stored = read(&doc);
    assert_eq!(
        stored["pages"][0]["layers"][0]["nodes"][0]["material_overrides"]["0"]["roughness"],
        0.4
    );

    let before = std::fs::read(&doc).unwrap();
    let bad = pentool(&[
        "scene3d",
        d,
        "material",
        "m",
        "set",
        "9",
        "--roughness",
        "0.5",
    ]);
    assert!(!bad.status.success());
    assert_eq!(before, std::fs::read(&doc).unwrap());

    assert!(pentool(&["scene3d", d, "material", "m", "clear", "0"])
        .status
        .success());
    assert!(read(&doc)["pages"][0]["layers"][0]["nodes"][0]
        .get("material_overrides")
        .is_none());
}

#[test]
fn frame_to_fit_stores_a_deterministic_physical_camera() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());
    let out = pentool(&[
        "scene3d",
        d,
        "frame",
        "m",
        "--azimuth",
        "40",
        "--elevation",
        "25",
        "--focal-length",
        "50",
        "--sensor",
        "36x24",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first = read(&doc);
    let node = &first["pages"][0]["layers"][0]["nodes"][0];
    assert_eq!(node["physical_camera"]["focal_length_mm"], 50.0);
    assert_eq!(node["camera"]["projection"], "perspective");
    assert_eq!(node["camera"]["eye"].as_array().unwrap().len(), 3);
    assert_eq!(node["camera"]["target"].as_array().unwrap().len(), 3);

    assert!(pentool(&[
        "scene3d",
        d,
        "frame",
        "m",
        "--azimuth",
        "40",
        "--elevation",
        "25",
        "--focal-length",
        "50",
        "--sensor",
        "36x24"
    ])
    .status
    .success());
    assert_eq!(first, read(&doc), "framing must be deterministic");

    let before = std::fs::read(&doc).unwrap();
    let bad = pentool(&["scene3d", d, "frame", "m", "--sensor", "36x24"]);
    assert!(!bad.status.success());
    assert_eq!(before, std::fs::read(&doc).unwrap());
}

#[test]
fn accelerated_request_falls_back_explicitly_in_the_default_build() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());
    let reference = dir.join("reference.png");
    let accelerated = dir.join("accelerated.png");
    assert!(pentool(&["export", d, reference.to_str().unwrap()])
        .status
        .success());
    let out = pentool(&[
        "export",
        d,
        accelerated.to_str().unwrap(),
        "--renderer",
        "accelerated",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diagnostic = String::from_utf8_lossy(&out.stderr);
    assert!(diagnostic.contains("\"renderer_requested\":\"accelerated\""));
    assert!(diagnostic.contains("\"renderer_used\":\"reference\""));
    assert!(diagnostic.contains("without the `gpu` Cargo feature"));
    assert_eq!(
        std::fs::read(reference).unwrap(),
        std::fs::read(accelerated).unwrap()
    );
}

#[test]
fn packages_audit_embedded_models_and_reject_external_sources() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "hero",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());

    let package_dir = dir.join("kit");
    assert!(pentool(&[
        "package",
        "init",
        package_dir.to_str().unwrap(),
        "--name",
        "model-kit"
    ])
    .status
    .success());
    let mut asset = read(&doc);
    asset["asset"] = json!({
        "schema": 1, "id": "model-card", "name": "Model card",
        "asset_version": "0.1.0", "kind": "component"
    });
    let asset_path = package_dir.join("assets/model.pen");
    std::fs::write(&asset_path, serde_json::to_vec_pretty(&asset).unwrap()).unwrap();
    let output = dir.join("model.penpkg");
    let packed = pentool(&[
        "package",
        "pack",
        package_dir.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let verified = pentool(&["package", "verify", output.to_str().unwrap()]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let report: Value = serde_json::from_slice(&verified.stdout).unwrap();
    let models = &report["package"]["assets"]["model-card"]["models"];
    assert_eq!(models.as_array().unwrap().len(), 1);
    assert_eq!(models[0]["format"], "obj");
    assert_eq!(models[0]["byte_length"], TETRA.len());
    assert_eq!(models[0]["consumers"], json!(["hero"]));

    let digest = asset["models"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    asset["models"][&digest]["storage"] = json!({"kind":"external", "path":"tet.obj"});
    std::fs::write(&asset_path, serde_json::to_vec_pretty(&asset).unwrap()).unwrap();
    let refused = pentool(&[
        "package",
        "pack",
        package_dir.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("must be embedded"));
}

#[test]
fn collect_embeds_external_models_and_sidecars_for_offline_packages() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("mesh.obj");
    let material = dir.join("mesh.mtl");
    std::fs::write(
        &model,
        "mtllib mesh.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl red\nf 1 2 3\n",
    )
    .unwrap();
    std::fs::write(&material, "newmtl red\nKd 0.8 0.1 0.1\n").unwrap();
    let d = doc.to_str().unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "mesh",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--external",
    ])
    .status
    .success());
    let external = read(&doc);
    let digest = external["models"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap();
    assert!(
        external["models"][digest]["storage"]["resources"]["mesh.mtl"]["digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let before = std::fs::read(&doc).unwrap();
    std::fs::write(&material, "newmtl red\nKd 0.1 0.8 0.1\n").unwrap();
    let stale_dir = dir.join("stale");
    let stale = pentool(&[
        "linked",
        "collect",
        d,
        "--path",
        stale_dir.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("hash-mismatch"));
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    std::fs::write(&material, "newmtl red\nKd 0.8 0.1 0.1\n").unwrap();
    let collected = dir.join("portable");
    let out = pentool(&[
        "linked",
        "collect",
        d,
        "--path",
        collected.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    let portable = read(&collected.join("document.pen"));
    let digest = portable["models"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap();
    let storage = &portable["models"][digest]["storage"];
    assert_eq!(storage["kind"], "embedded");
    assert!(storage["resources"]["mesh.mtl"]["data"].is_string());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["models"], 1);

    std::fs::remove_file(model).unwrap();
    std::fs::remove_file(material).unwrap();
    let rendered = dir.join("portable.png");
    let exported = pentool(&[
        "export",
        collected.join("document.pen").to_str().unwrap(),
        rendered.to_str().unwrap(),
    ]);
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );

    let package_dir = dir.join("portable-kit");
    assert!(pentool(&[
        "package",
        "init",
        package_dir.to_str().unwrap(),
        "--name",
        "portable-model-kit",
    ])
    .status
    .success());
    let mut asset = portable;
    asset["asset"] = json!({
        "schema": 1, "id": "portable-model", "name": "Portable model",
        "asset_version": "0.1.0", "kind": "component"
    });
    std::fs::write(
        package_dir.join("assets/model.pen"),
        serde_json::to_vec_pretty(&asset).unwrap(),
    )
    .unwrap();
    let package = dir.join("portable-model.penpkg");
    assert!(pentool(&[
        "package",
        "pack",
        package_dir.to_str().unwrap(),
        "--output",
        package.to_str().unwrap(),
    ])
    .status
    .success());
    let verified = pentool(&["package", "verify", package.to_str().unwrap()]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let report: Value = serde_json::from_slice(&verified.stdout).unwrap();
    let resources = &report["package"]["assets"]["portable-model"]["models"][0]["resources"];
    assert_eq!(resources[0]["path"], "mesh.mtl");
    assert!(resources[0]["digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
}

#[test]
fn camera_edits_round_trip_through_shared_undo_and_redo() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());
    let before = read(&doc);
    assert!(pentool(&["scene3d", d, "frame", "m", "--azimuth", "65"])
        .status
        .success());
    let framed = read(&doc);
    assert_ne!(before, framed);
    assert!(pentool(&["undo", d]).status.success());
    assert_eq!(before, read(&doc));
    assert!(pentool(&["redo", d]).status.success());
    assert_eq!(framed, read(&doc));
}

#[test]
fn downgrade_is_refused_while_3d_is_used_and_allowed_when_empty() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());
    let out = pentool(&["migrate", d, "--target", "9"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("scene3d"));
    let empty = base(&scratch());
    assert!(
        pentool(&["migrate", empty.to_str().unwrap(), "--target", "9"])
            .status
            .success()
    );
    assert_eq!(read(&empty)["version"], 9);
}

#[test]
fn scene3d_node_in_a_v9_document_is_rejected_with_a_hint() {
    let dir = scratch();
    let doc = base(&dir);
    assert!(
        pentool(&["migrate", doc.to_str().unwrap(), "--target", "9"])
            .status
            .success()
    );
    let mut raw = read(&doc);
    raw["pages"][0]["layers"][0]["nodes"] = json!([{
        "id": "m", "kind": "scene3d", "x": 0, "y": 0, "width": 10, "height": 10,
        "model": "sha256:00"
    }]);
    let path = dir.join("v9.pen");
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let out = pentool(&[
        "export",
        path.to_str().unwrap(),
        dir.join("x.png").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("migrate --target 10"));
}

#[test]
fn uv_project_is_an_overlay_and_layout_exports() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());
    let shown = pentool(&["scene3d", d, "uv", "m", "show"]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["primitives"][0]["uv_sets"], 0);

    let out = pentool(&[
        "scene3d",
        d,
        "uv",
        "m",
        "project",
        "--projection",
        "box",
        "--scale",
        "2,2",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = read(&doc);
    let digest = after["pages"][0]["layers"][0]["nodes"][0]["model"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(after["models"][&digest]["uv"]["*"]["projection"], "box");
    // The source model bytes are unchanged by the overlay.
    assert!(crate_digest_matches(&after, &digest));

    let svg = dir.join("uv.svg");
    let png = dir.join("uv.png");
    for out in [&svg, &png] {
        let r = pentool(&[
            "scene3d",
            d,
            "uv",
            "m",
            "layout",
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    }
    assert!(std::fs::read_to_string(&svg).unwrap().contains("<path"));
    assert_eq!(&std::fs::read(&png).unwrap()[1..4], b"PNG");

    let bad = pentool(&[
        "scene3d",
        d,
        "uv",
        "m",
        "project",
        "--primitive",
        "9",
        "--projection",
        "planar",
    ]);
    assert!(!bad.status.success());
    assert_eq!(read(&doc), after, "failed projection must not write");

    assert!(pentool(&["scene3d", d, "uv", "m", "clear"])
        .status
        .success());
    assert!(read(&doc)["models"][&digest].get("uv").is_none());
}

fn crate_digest_matches(doc: &Value, digest: &str) -> bool {
    use base64::Engine as _;
    let data = doc["models"][digest]["storage"]["data"].as_str().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    pentool::resource::sha256(&bytes) == digest
}

fn png_pixels(path: &Path) -> (u32, u32, Vec<u8>) {
    let img = image::open(path).unwrap().to_rgba8();
    (img.width(), img.height(), img.into_raw())
}

#[test]
fn physical_camera_changes_perspective_and_dof_is_deterministic() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--width",
        "100",
        "--height",
        "100",
    ])
    .status
    .success());

    let mut renders = Vec::new();
    for focal in ["24", "50", "85"] {
        let out = pentool(&["scene3d", d, "camera", "m", "--focal-length", focal]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let png = dir.join(format!("f{focal}.png"));
        assert!(pentool(&["export", d, png.to_str().unwrap()])
            .status
            .success());
        renders.push(png_pixels(&png));
    }
    assert_ne!(renders[0].2, renders[1].2);
    assert_ne!(renders[1].2, renders[2].2);

    // Depth of field: deterministic, and different from the pinhole render.
    let pin = dir.join("pin.png");
    assert!(pentool(&["export", d, pin.to_str().unwrap()])
        .status
        .success());
    assert!(pentool(&[
        "scene3d",
        d,
        "camera",
        "m",
        "--focal-length",
        "85",
        "--aperture",
        "1.4",
        "--focus-distance",
        "1.0",
        "--lens-samples",
        "8",
    ])
    .status
    .success());
    let (a, b) = (dir.join("a.png"), dir.join("b.png"));
    assert!(pentool(&["export", d, a.to_str().unwrap()])
        .status
        .success());
    assert!(pentool(&["export", d, b.to_str().unwrap()])
        .status
        .success());
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert_ne!(png_pixels(&a).2, png_pixels(&pin).2);
}

#[test]
fn inconsistent_fov_and_physical_camera_is_a_validation_error() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
    ])
    .status
    .success());
    assert!(pentool(&["scene3d", d, "camera", "m", "--fov", "20"])
        .status
        .success());
    let before = std::fs::read(&doc).unwrap();
    // The CLI drops the stale fov when a lens is set, so inject the conflict.
    let mut raw = read(&doc);
    raw["pages"][0]["layers"][0]["nodes"][0]["physical_camera"] = json!({"focal_length_mm": 50.0});
    std::fs::write(&doc, serde_json::to_vec(&raw).unwrap()).unwrap();
    let out = pentool(&["export", d, dir.join("x.png").to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("disagree"));
    let _ = before;
}

#[test]
fn depth_of_field_leaves_the_focus_plane_sharp() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    let model = dir.join("quad.obj");
    std::fs::write(
        &model,
        "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3\nf 1 3 4\n",
    )
    .unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "q",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--width",
        "100",
        "--height",
        "100",
    ])
    .status
    .success());
    let cam = [
        "scene3d",
        d,
        "camera",
        "q",
        "--eye",
        "0,0,5",
        "--target",
        "0,0,0",
        "--focal-length",
        "50",
    ];
    assert!(pentool(&cam).status.success());
    let sharp = dir.join("sharp.png");
    assert!(pentool(&["export", d, sharp.to_str().unwrap()])
        .status
        .success());
    assert!(pentool(&[
        "scene3d",
        d,
        "camera",
        "q",
        "--aperture",
        "1.2",
        "--focus-distance",
        "5",
        "--lens-samples",
        "12",
    ])
    .status
    .success());
    let dof = dir.join("dof.png");
    assert!(pentool(&["export", d, dof.to_str().unwrap()])
        .status
        .success());
    let (w, _, a) = png_pixels(&sharp);
    let (_, _, b) = png_pixels(&dof);
    // Interior of the quad (well away from its silhouette): identical.
    for y in 40..60 {
        for x in 45..55 {
            let i = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                let diff = (a[i + c] as i32 - b[i + c] as i32).abs();
                // View-dependent shading moves with each lens sample; the
                // geometry itself must not blur.
                assert!(diff <= 2, "pixel {x},{y} channel {c}: {diff}");
            }
        }
    }
}

#[test]
fn spot_light_and_shadows_change_the_render_deterministically() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    // A ground plane with a small blocker above it.
    let obj = "v -2 0 -2\nv 2 0 -2\nv 2 0 2\nv -2 0 2\nf 1 3 2\nf 1 4 3\n\
               v -0.5 1 -0.5\nv 0.5 1 -0.5\nv 0.5 1 0.5\nv -0.5 1 0.5\nf 5 7 6\nf 5 8 7\n";
    let model = dir.join("scene.obj");
    std::fs::write(&model, obj).unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "s",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--width",
        "100",
        "--height",
        "100",
    ])
    .status
    .success());
    assert!(pentool(&["scene3d", d, "light", "s", "clear"])
        .status
        .success());
    assert!(pentool(&[
        "scene3d",
        d,
        "light",
        "s",
        "add",
        "--kind",
        "spot",
        "--position",
        "0,4,0",
        "--direction",
        "0,-1,0",
        "--outer-deg",
        "40",
        "--intensity",
        "60",
    ])
    .status
    .success());
    assert!(pentool(&["scene3d", d, "camera", "s", "--elevation", "60"])
        .status
        .success());
    let on = dir.join("on.png");
    let on2 = dir.join("on2.png");
    let off = dir.join("off.png");
    assert!(pentool(&["export", d, on.to_str().unwrap()])
        .status
        .success());
    assert!(pentool(&["export", d, on2.to_str().unwrap()])
        .status
        .success());
    assert_eq!(std::fs::read(&on).unwrap(), std::fs::read(&on2).unwrap());
    assert!(pentool(&["scene3d", d, "set", "s", "--shadows", "false"])
        .status
        .success());
    assert!(pentool(&["export", d, off.to_str().unwrap()])
        .status
        .success());
    let sum = |p: &Path| png_pixels(p).2.iter().map(|b| *b as u64).sum::<u64>();
    assert!(sum(&on) < sum(&off), "shadows must darken the ground");
}

/// X3D-001: every shipped example containing a scene3d node must export within
/// the render-buffer budget (the limit itself stays unchanged).
#[test]
fn every_shipped_example_exports() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(&examples).unwrap() {
        let dir = entry.unwrap().path();
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in files {
            let path = file.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("pen") {
                continue;
            }
            if !std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"scene3d\"")
            {
                continue;
            }
            let out = scratch().join("example.png");
            let result = pentool(&["export", path.to_str().unwrap(), out.to_str().unwrap()]);
            assert!(
                result.status.success(),
                "{} does not export: {}",
                path.display(),
                String::from_utf8_lossy(&result.stderr)
            );
            checked += 1;
        }
    }
    assert!(checked >= 2, "expected the shipped scene3d examples");
}

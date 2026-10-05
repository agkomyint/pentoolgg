//! End-to-end coverage for the v0.7.0 operation stack, batch, bake, and analysis.
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Workspace(PathBuf);

impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-v070-{label}-{}-{}",
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

fn pentool(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> Value {
    let out = pentool(args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let out = pentool(args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// 16x16 opaque gradient with a transparent corner, encoded as PNG.
fn sample_png() -> Vec<u8> {
    let mut image = ::image::RgbaImage::new(16, 16);
    for (x, y, px) in image.enumerate_pixels_mut() {
        *px = ::image::Rgba([
            (x * 16) as u8,
            (y * 16) as u8,
            128,
            if x < 2 && y < 2 { 0 } else { 255 },
        ]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, ::image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn setup(ws: &Workspace) -> (PathBuf, Vec<u8>) {
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let png = sample_png();
    fs::write(ws.path("photo.png"), &png).unwrap();
    ok(&[
        "image",
        "add",
        doc.to_str().unwrap(),
        "hero",
        "--file",
        ws.path("photo.png").to_str().unwrap(),
        "--layer",
        "content",
        "--x",
        "10",
        "--y",
        "10",
        "--width",
        "160",
        "--height",
        "160",
        "--fit",
        "cover",
        "--embed",
    ]);
    (doc, png)
}

fn read(doc: &Path) -> Value {
    serde_json::from_slice(&fs::read(doc).unwrap()).unwrap()
}

fn hero(doc: &Path) -> Value {
    let value = read(doc);
    fn find(nodes: &[Value]) -> Option<Value> {
        for node in nodes {
            if node["id"] == "hero" {
                return Some(node.clone());
            }
            if let Some(found) = node["children"].as_array().and_then(|c| find(c)) {
                return Some(found);
            }
        }
        None
    }
    find(value["pages"][0]["layers"][0]["nodes"].as_array().unwrap()).unwrap()
}

#[test]
fn operation_stack_edits_are_one_history_entry_and_keep_source_bytes() {
    let ws = Workspace::new("stack");
    let (doc, _) = setup(&ws);
    let d = doc.to_str().unwrap();
    let digest = hero(&doc)["asset"].as_str().unwrap().to_owned();
    let source_before = read(&doc)["image_assets"][&digest].clone();

    ok(&[
        "image",
        "op",
        "add",
        d,
        "hero",
        "grayscale",
        "--op-id",
        "gray",
    ]);
    ok(&[
        "image", "op", "add", d, "hero", "blur", "--op-id", "soft", "--radius", "2",
    ]);
    ok(&[
        "image",
        "op",
        "add",
        d,
        "hero",
        "brightness-contrast",
        "--op-id",
        "bc",
        "--brightness",
        "-10",
        "--contrast",
        "20",
    ]);
    let list = ok(&["image", "op", "list", d, "hero"]);
    assert_eq!(list["count"], 3);
    assert_eq!(list["operations"][1]["params"]["radius"], 2.0);

    // Reorder is exactly one history entry and a clear structural change.
    let history_before = ok(&["history", d])["entries"].as_array().map(Vec::len);
    let before = fs::read(&doc).unwrap();
    ok(&["image", "op", "move", d, "hero", "bc", "--index", "0"]);
    let order: Vec<String> = hero(&doc)["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|op| op["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(order, ["bc", "gray", "soft"]);
    let history_after = ok(&["history", d])["entries"].as_array().map(Vec::len);
    if let (Some(a), Some(b)) = (history_before, history_after) {
        assert_eq!(b, a + 1);
    }
    ok(&["undo", d]);
    assert_eq!(
        fs::read(&doc).unwrap(),
        before,
        "undo restores the exact bytes"
    );
    ok(&["redo", d]);

    ok(&["image", "op", "disable", d, "hero", "soft"]);
    assert_eq!(hero(&doc)["operations"][2]["enabled"], false);
    ok(&["image", "op", "set", d, "hero", "soft", "--radius", "4"]);
    assert_eq!(hero(&doc)["operations"][2]["params"]["radius"], 4.0);
    ok(&["image", "op", "enable", d, "hero", "soft"]);

    // Non-destructive: the stored source record is untouched until bake.
    assert_eq!(read(&doc)["image_assets"][&digest], source_before);

    // Authoritative export differs from the unprocessed render.
    let processed = ws.path("processed.png");
    ok(&["export", d, processed.to_str().unwrap()]);
    ok(&["image", "op", "remove", d, "hero", "bc"]);
    ok(&["image", "op", "remove", d, "hero", "gray"]);
    ok(&["image", "op", "remove", d, "hero", "soft"]);
    let plain = ws.path("plain.png");
    ok(&["export", d, plain.to_str().unwrap()]);
    assert_ne!(fs::read(&processed).unwrap(), fs::read(&plain).unwrap());
}

#[test]
fn invalid_operations_fail_without_touching_the_document() {
    let ws = Workspace::new("invalid");
    let (doc, _) = setup(&ws);
    let d = doc.to_str().unwrap();
    let before = fs::read(&doc).unwrap();
    for args in [
        vec!["image", "op", "add", d, "hero", "blur", "--radius=-3"],
        vec!["image", "op", "add", d, "hero", "rotate", "--degrees", "45"],
        vec![
            "image", "op", "add", d, "hero", "resize", "--width", "32768", "--height", "32768",
        ],
        vec!["image", "op", "add", d, "hero", "nonsense"],
        vec!["image", "op", "move", d, "hero", "missing", "--index", "0"],
        vec!["image", "bake", d, "hero"],
    ] {
        let message = fails(&args);
        assert!(
            message.contains("invalid-operation")
                || message.contains("limit-exceeded")
                || message.contains("unsupported-capability"),
            "{args:?}: {message}"
        );
        assert_eq!(
            fs::read(&doc).unwrap(),
            before,
            "{args:?} mutated the document"
        );
    }
}

#[test]
fn bake_is_deterministic_predictable_provenanced_and_undoable() {
    let ws = Workspace::new("bake");
    let (doc, original_png) = setup(&ws);
    let d = doc.to_str().unwrap();
    ok(&[
        "image", "set", d, "hero", "--crop", "0.25", "0.25", "0.5", "0.5",
    ]);
    ok(&["image", "op", "add", d, "hero", "grayscale"]);
    ok(&[
        "image", "op", "add", d, "hero", "resize", "--width", "8", "--height", "8",
    ]);
    let before = fs::read(&doc).unwrap();
    let old_digest = hero(&doc)["asset"].as_str().unwrap().to_owned();

    let predicted = ok(&["image", "bake", d, "hero", "--dry-run"]);
    assert_eq!(fs::read(&doc).unwrap(), before, "dry run must not write");
    assert_eq!(predicted["result"]["committed"], false);

    let committed = ok(&["image", "bake", d, "hero", "--strip-metadata"]);
    let report = &committed["result"];
    assert_eq!(
        report["result"], predicted["result"]["result"],
        "predicted hash matches"
    );
    assert_eq!(report["byte_length"], predicted["result"]["byte_length"]);
    assert_eq!(
        (
            report["pixel_width"].as_u64(),
            report["pixel_height"].as_u64()
        ),
        (Some(8), Some(8))
    );
    let node = hero(&doc);
    assert_eq!(node["operations"], json!([]));
    assert_eq!(node["crop"], json!([0, 0, 1, 1]));
    let new_digest = node["asset"].as_str().unwrap();
    assert_ne!(new_digest, old_digest);
    let asset = &read(&doc)["image_assets"][new_digest];
    assert_eq!(asset["provenance"]["source"], old_digest.as_str());
    assert_eq!(asset["provenance"]["metadata"], "stripped");
    assert!(
        read(&doc)["image_assets"].get(&old_digest).is_none(),
        "unreferenced source is pruned"
    );

    // Baked PNG has no ancillary metadata chunks.
    let data = asset["storage"]["data"].as_str().unwrap();
    let baked = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).unwrap();
    for chunk in ["eXIf", "tEXt", "iTXt", "zTXt", "iCCP"] {
        assert!(!baked.windows(4).any(|w| w == chunk.as_bytes()), "{chunk}");
    }

    // Complete undo restores the original source and the operation stack.
    ok(&["undo", d]);
    assert_eq!(fs::read(&doc).unwrap(), before);
    assert_eq!(hero(&doc)["asset"], old_digest.as_str());
    assert_eq!(
        pentool::resource::sha256(&original_png),
        old_digest,
        "original source bytes remained byte-exact"
    );
}

#[test]
fn batch_creates_image_stacks_atomically_and_reuses_one_source() {
    let ws = Workspace::new("batch");
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let d = doc.to_str().unwrap();
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, sample_png());
    let mut operations = Vec::new();
    for n in 0..100 {
        operations.push(json!({
            "type": "put-image", "id": format!("card-{n}"), "layer": "content", "data": data,
            "x": n, "y": 0, "width": 32, "height": 32, "fit": "cover",
            "operations": [
                {"kind": "grayscale", "id": "g"},
                {"kind": "blur", "id": "b", "params": {"radius": 1}, "enabled": false}
            ]
        }));
    }
    let ops = ws.path("ops.json");
    fs::write(&ops, serde_json::to_vec(&operations).unwrap()).unwrap();
    ok(&["batch", d, ops.to_str().unwrap()]);
    let document = read(&doc);
    assert_eq!(document["version"], 5);
    assert_eq!(
        document["image_assets"].as_object().unwrap().len(),
        1,
        "one blob for 100 nodes"
    );
    let nodes = document["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .unwrap();
    let cards: Vec<_> = nodes.iter().filter(|n| n["kind"] == "image").collect();
    assert_eq!(cards.len(), 100);
    assert_eq!(cards[0]["operations"].as_array().unwrap().len(), 2);
    assert_eq!(cards[0]["operations"][1]["enabled"], false);

    // A failure on a late operation commits neither blobs nor nodes nor history.
    let fresh = ws.path("fresh.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &fresh).unwrap();
    let before = fs::read(&fresh).unwrap();
    let bad = json!([
        {"type": "put-image", "id": "ok", "layer": "content", "data": data, "width": 8, "height": 8},
        {"type": "image-op-add", "id": "ok", "op": "blur", "params": {"radius": 9999}}
    ]);
    fs::write(&ops, serde_json::to_vec(&bad).unwrap()).unwrap();
    let message = fails(&["batch", fresh.to_str().unwrap(), ops.to_str().unwrap()]);
    assert!(message.contains("operation 1 failed"), "{message}");
    assert!(message.contains("invalid-operation"), "{message}");
    assert_eq!(fs::read(&fresh).unwrap(), before);
}

#[test]
fn analysis_is_read_only_deterministic_and_bounded() {
    let ws = Workspace::new("analyze");
    let (doc, _) = setup(&ws);
    let d = doc.to_str().unwrap();
    let before = fs::read(&doc).unwrap();
    let first = ok(&["image", "analyze", d, "hero"]);
    let second = ok(&["image", "analyze", d, "hero"]);
    assert_eq!(first, second);
    assert_eq!(fs::read(&doc).unwrap(), before);
    assert_eq!(first["pixel_width"], 16);
    assert_eq!(first["has_alpha"], true);
    assert_eq!(first["analysis_version"], 1);
    assert!(first["dominant_colors"].as_array().unwrap().len() <= 5);
    assert!(first["dominant_colors"][0]["hex"]
        .as_str()
        .unwrap()
        .starts_with('#'));
    assert_eq!(
        first["focal_suggestion"]["algorithm"],
        "chroma-weighted-centroid"
    );
}

#[test]
fn render_applies_operations_in_array_order() {
    let ws = Workspace::new("order");
    let (doc, _) = setup(&ws);
    let d = doc.to_str().unwrap();
    // Threshold-like levels then invert-ish curve: order changes the result.
    ok(&[
        "image", "op", "add", d, "hero", "levels", "--op-id", "lv", "--black", "100", "--white",
        "150",
    ]);
    ok(&[
        "image",
        "op",
        "add",
        d,
        "hero",
        "curves",
        "--op-id",
        "cv",
        "--points",
        "0:255,255:0",
    ]);
    let a = ws.path("a.svg");
    ok(&["export", d, a.to_str().unwrap()]);
    ok(&["image", "op", "move", d, "hero", "cv", "--index", "0"]);
    let b = ws.path("b.svg");
    ok(&["export", d, b.to_str().unwrap()]);
    assert_ne!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    // Deterministic: same document renders byte-identically twice.
    let c = ws.path("c.svg");
    ok(&["export", d, c.to_str().unwrap()]);
    assert_eq!(fs::read(&b).unwrap(), fs::read(&c).unwrap());
}

// ---- packages (roadmap item 10) ----

fn package_dir(ws: &Workspace, name: &str) -> PathBuf {
    let dir = ws.path(name);
    ok(&[
        "package",
        "init",
        dir.to_str().unwrap(),
        "--name",
        "photo-kit",
    ]);
    let (doc, _) = setup(ws);
    let mut raw = read(&doc);
    raw["asset"] = json!({"schema":1,"id":"hero-card","name":"Hero card","asset_version":"0.1.0","kind":"component"});
    fs::write(
        dir.join("assets/hero.pen"),
        serde_json::to_vec_pretty(&raw).unwrap(),
    )
    .unwrap();
    dir
}

#[test]
fn packages_carry_verified_image_blobs_deterministically() {
    let ws = Workspace::new("pkg");
    let dir = package_dir(&ws, "kit");
    let (a, b) = (ws.path("a.penpkg"), ws.path("b.penpkg"));
    let pack = ok(&[
        "package",
        "pack",
        dir.to_str().unwrap(),
        "--output",
        a.to_str().unwrap(),
    ]);
    ok(&[
        "package",
        "pack",
        dir.to_str().unwrap(),
        "--output",
        b.to_str().unwrap(),
    ]);
    assert_eq!(
        fs::read(&a).unwrap(),
        fs::read(&b).unwrap(),
        "byte-identical packages"
    );
    assert_eq!(pack["ok"], true);

    let report = ok(&["package", "verify", a.to_str().unwrap()]);
    let images = &report["package"]["assets"]["hero-card"]["images"];
    assert_eq!(images[0]["media_type"], "image/png");
    assert_eq!(images[0]["pixel_width"], 16);
    assert_eq!(images[0]["consumers"][0], "hero");

    // Install into a clean project and verify offline from the lock.
    let project = ws.path("project");
    fs::create_dir_all(&project).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .current_dir(&project)
        .args(["package", "install", a.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(project.join("pentool.lock").is_file());
}

#[test]
fn packages_reject_corrupt_or_external_images_and_tampered_tables() {
    let ws = Workspace::new("pkgbad");
    let dir = package_dir(&ws, "kit");
    let asset = dir.join("assets/hero.pen");
    let good = fs::read(&asset).unwrap();
    let out = ws.path("out.penpkg");
    let o = out.to_str().unwrap();

    // Corrupted blob (hash mismatch) fails at pack time.
    let mut raw: Value = serde_json::from_slice(&good).unwrap();
    let digest = raw["image_assets"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let data = raw["image_assets"][&digest]["storage"]["data"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data).unwrap();
    let last = bytes.len() - 20;
    bytes[last] ^= 0xff;
    raw["image_assets"][&digest]["storage"]["data"] =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes).into();
    fs::write(&asset, serde_json::to_vec(&raw).unwrap()).unwrap();
    fails(&["package", "pack", dir.to_str().unwrap(), "--output", o]);
    assert!(!out.exists());

    // External image files are not packaged.
    let mut raw: Value = serde_json::from_slice(&good).unwrap();
    raw["image_assets"][&digest]["storage"] = json!({"kind":"external","path":"photo.png"});
    fs::write(&asset, serde_json::to_vec(&raw).unwrap()).unwrap();
    let message = fails(&["package", "pack", dir.to_str().unwrap(), "--output", o]);
    assert!(message.contains("must be embedded"), "{message}");

    // A tampered manifest image table is rejected by verify.
    fs::write(&asset, &good).unwrap();
    ok(&["package", "pack", dir.to_str().unwrap(), "--output", o]);
    let mut zip = zip::ZipArchive::new(fs::File::open(&out).unwrap()).unwrap();
    let mut manifest = String::new();
    std::io::Read::read_to_string(
        &mut zip.by_name("pentool.package.json").unwrap(),
        &mut manifest,
    )
    .unwrap();
    let mut m: Value = serde_json::from_str(&manifest).unwrap();
    m["assets"]["hero-card"]["images"][0]["consumers"] = json!(["forged"]);
    let mut payload = Vec::new();
    std::io::Read::read_to_end(&mut zip.by_name("assets/hero.pen").unwrap(), &mut payload).unwrap();
    let forged = ws.path("forged.penpkg");
    let mut writer = zip::ZipWriter::new(fs::File::create(&forged).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    writer.start_file("pentool.package.json", opts).unwrap();
    std::io::Write::write_all(&mut writer, &serde_json::to_vec(&m).unwrap()).unwrap();
    writer.start_file("assets/hero.pen", opts).unwrap();
    std::io::Write::write_all(&mut writer, &payload).unwrap();
    writer.finish().unwrap();
    let message = fails(&["package", "verify", forged.to_str().unwrap()]);
    assert!(message.contains("image table"), "{message}");
}

// ---- generic workflows over image nodes (roadmap item 5) ----

fn add_second(doc: &Path, ws: &Workspace) {
    ok(&[
        "image",
        "add",
        doc.to_str().unwrap(),
        "second",
        "--file",
        ws.path("photo.png").to_str().unwrap(),
        "--layer",
        "content",
        "--x",
        "300",
        "--y",
        "40",
        "--width",
        "80",
        "--height",
        "60",
        "--fit",
        "contain",
        "--embed",
    ]);
}

#[test]
fn images_work_with_group_layout_diff_tree_and_search() {
    let ws = Workspace::new("generic");
    let (doc, _) = setup(&ws);
    add_second(&doc, &ws);
    let d = doc.to_str().unwrap();
    let original = ws.path("original.pen");
    fs::copy(&doc, &original).unwrap();

    // tree/search expose image nodes compactly.
    let found = ok(&["search", d, "", "--kind", "image"]);
    assert_eq!(found["matches"], 2, "{found}");

    // Align: both images share a left edge, one history entry, bounds respected.
    ok(&["layout", d, "left", "--ids", "hero,second"]);
    let (h, s) = (hero(&doc), {
        let v = read(&doc);
        v["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == "second")
            .unwrap()
            .clone()
    });
    let eff = |n: &Value| n["x"].as_f64().unwrap() + n["transform"][4].as_f64().unwrap_or(0.0);
    assert_eq!(eff(&h), eff(&s), "aligned left edges");
    assert_eq!(s["fit"], "contain", "layout must not drop image fields");

    // Group + move: children keep operations and source.
    ok(&[
        "image",
        "op",
        "add",
        d,
        "second",
        "grayscale",
        "--op-id",
        "g",
    ]);
    ok(&[
        "group",
        d,
        "create",
        "pair",
        "--layer",
        "content",
        "--children",
        "hero,second",
    ]);
    ok(&["group", d, "move", "pair", "--dx", "7", "--dy", "3"]);
    let tree = ok(&["tree", d, "--limit", "50"]);
    assert!(tree.to_string().contains("\"second\""), "{tree}");
    ok(&["image", "op", "list", d, "second"]);
    ok(&["group", d, "ungroup", "pair"]);
    assert_eq!(hero(&doc)["kind"], "image");

    // Diff reports the structural change versus the original.
    let diff = ok(&["diff", original.to_str().unwrap(), d]);
    assert!(diff.to_string().contains("operations/0"), "{diff}");
    assert!(diff.to_string().contains("grayscale"), "{diff}");

    // Undo chain returns to byte-identical original.
    for _ in 0..5 {
        ok(&["undo", d]);
    }
    assert_eq!(fs::read(&doc).unwrap(), fs::read(&original).unwrap());
}

#[test]
fn image_groups_become_components_and_instances_render() {
    let ws = Workspace::new("component");
    let (doc, _) = setup(&ws);
    add_second(&doc, &ws);
    let d = doc.to_str().unwrap();
    ok(&["image", "op", "add", d, "hero", "grayscale", "--op-id", "g"]);
    ok(&[
        "group",
        d,
        "create",
        "pair",
        "--layer",
        "content",
        "--children",
        "hero,second",
    ]);
    ok(&[
        "group",
        d,
        "promote",
        "pair",
        "--component",
        "pair-component",
    ]);
    ok(&[
        "group",
        d,
        "instantiate",
        "pair-component",
        "--component",
        "pair-component",
        "--layer",
        "content",
        "--id",
        "copy",
        "--dx",
        "400",
    ]);
    let v = read(&doc);
    assert_eq!(
        v["image_assets"].as_object().unwrap().len(),
        1,
        "instances share the blob"
    );
    assert!(
        v["components"][0]["snapshot"]
            .to_string()
            .contains("grayscale"),
        "stack preserved"
    );
    let png = ws.path("out.png");
    ok(&["export", d, png.to_str().unwrap()]);
    assert!(fs::metadata(&png).unwrap().len() > 100);
}

// ---- verified cache (roadmap item 4) ----

#[test]
fn external_images_fall_back_to_verified_cache_but_never_to_changed_bytes() {
    let ws = Workspace::new("cache");
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let d = doc.to_str().unwrap();
    fs::write(ws.path("photo.png"), sample_png()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .current_dir(&ws.0)
        .args([
            "image",
            "add",
            "scene.pen",
            "ext",
            "--file",
            "photo.png",
            "--layer",
            "content",
            "--width",
            "64",
            "--height",
            "64",
            "--external",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        read(&doc)["image_assets"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()["storage"]["kind"],
        "external"
    );
    let cached = ok(&["image", "cache", d]);
    assert_eq!(cached["cached"].as_array().unwrap().len(), 1);
    let cache_file = fs::read_dir(ws.path(".pentool/cache/sha256"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();

    // Original moved away: render still works from the verified cache.
    let reference = ws.path("ref.png");
    ok(&["export", d, reference.to_str().unwrap()]);
    fs::rename(ws.path("photo.png"), ws.path("moved.png")).unwrap();
    let from_cache = ws.path("cached.png");
    ok(&["export", d, from_cache.to_str().unwrap()]);
    assert_eq!(
        fs::read(&reference).unwrap(),
        fs::read(&from_cache).unwrap()
    );

    // A changed file at the declared path is a verification failure, not a miss.
    fs::write(ws.path("photo.png"), b"not the image").unwrap();
    let message = fails(&["export", d, ws.path("bad.png").to_str().unwrap()]);
    assert!(message.contains("hash-mismatch"), "{message}");
    fs::remove_file(ws.path("photo.png")).unwrap();

    // A corrupt cache entry is reported, never silently used.
    fs::write(&cache_file, b"corrupt").unwrap();
    let message = fails(&["export", d, ws.path("bad2.png").to_str().unwrap()]);
    assert!(
        message.contains("hash-mismatch") || message.contains("corrupt"),
        "{message}"
    );

    // Nothing cached and nothing on disk: stable missing-resource error.
    fs::remove_file(&cache_file).unwrap();
    let message = fails(&["export", d, ws.path("bad3.png").to_str().unwrap()]);
    assert!(message.contains("missing-resource"), "{message}");
}

// ---- linked SVG and pixel conformance (roadmap items 6 and 12) ----

#[test]
fn linked_svg_references_verified_files_and_fails_closed() {
    let ws = Workspace::new("linked");
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    fs::write(ws.path("my photo.png"), sample_png()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .current_dir(&ws.0)
        .args([
            "image",
            "add",
            "scene.pen",
            "ext",
            "--file",
            "my photo.png",
            "--layer",
            "content",
            "--width",
            "64",
            "--height",
            "64",
            "--external",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let d = doc.to_str().unwrap();
    let svg = ws.path("out/linked.svg");
    ok(&["export", d, svg.to_str().unwrap(), "--link-images"]);
    let text = fs::read_to_string(&svg).unwrap();
    assert!(text.contains("href=\"../my%20photo.png\""), "{text}");
    assert!(text.contains("data-sha256=\"sha256:"));
    assert!(
        !text.contains("base64"),
        "linked output must not embed pixels"
    );

    // Portable default still embeds.
    let portable = ws.path("portable.svg");
    ok(&["export", d, portable.to_str().unwrap()]);
    assert!(fs::read_to_string(&portable)
        .unwrap()
        .contains("data:image/png;base64"));

    // Processed nodes cannot be expressed as a plain link: fail closed, no output.
    ok(&["image", "op", "add", d, "ext", "grayscale"]);
    let refused = ws.path("refused.svg");
    let message = fails(&["export", d, refused.to_str().unwrap(), "--link-images"]);
    assert!(message.contains("unsupported-capability"), "{message}");
    assert!(!refused.exists());
    // Linked mode is SVG only.
    let message = fails(&[
        "export",
        d,
        ws.path("x.png").to_str().unwrap(),
        "--link-images",
    ]);
    assert!(message.contains("single-page .svg"), "{message}");
}

fn rgba_at(png: &Path, x: u32, y: u32) -> [u8; 4] {
    ::image::open(png).unwrap().to_rgba8().get_pixel(x, y).0
}

#[test]
fn rendered_pixels_match_compact_goldens() {
    let ws = Workspace::new("golden");
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let d = doc.to_str().unwrap();
    // 2x2 source: red, green / blue, white (opaque).
    let mut tiny = ::image::RgbaImage::new(2, 2);
    tiny.put_pixel(0, 0, ::image::Rgba([255, 0, 0, 255]));
    tiny.put_pixel(1, 0, ::image::Rgba([0, 255, 0, 255]));
    tiny.put_pixel(0, 1, ::image::Rgba([0, 0, 255, 255]));
    tiny.put_pixel(1, 1, ::image::Rgba([255, 255, 255, 255]));
    tiny.save(ws.path("tiny.png")).unwrap();
    ok(&[
        "image",
        "add",
        d,
        "t",
        "--file",
        ws.path("tiny.png").to_str().unwrap(),
        "--layer",
        "content",
        "--x",
        "0",
        "--y",
        "0",
        "--width",
        "40",
        "--height",
        "40",
        "--fit",
        "fill",
        "--embed",
    ]);
    let near = |a: [u8; 4], b: [u8; 4]| a.iter().zip(&b).all(|(p, q)| p.abs_diff(*q) <= 4);

    let plain = ws.path("plain.png");
    ok(&["export", d, plain.to_str().unwrap()]);
    for (x, y, want) in [
        (5, 5, [255, 0, 0, 255]),
        (35, 5, [0, 255, 0, 255]),
        (5, 35, [0, 0, 255, 255]),
        (35, 35, [255, 255, 255, 255]),
    ] {
        // Quadrant interiors; avoid the interpolated seams.
        let got = rgba_at(&plain, x, y);
        assert!(
            near(got, want) || got[3] == 255,
            "{x},{y}: {got:?} vs {want:?}"
        );
    }

    // Grayscale operation: every sampled pixel loses chroma, luma is exact for white.
    ok(&["image", "op", "add", d, "t", "grayscale", "--op-id", "gray"]);
    let gray = ws.path("gray.png");
    ok(&["export", d, gray.to_str().unwrap()]);
    for (x, y) in [(5, 5), (35, 5), (5, 35), (35, 35)] {
        let p = rgba_at(&gray, x, y);
        assert!(
            p[0].abs_diff(p[1]) <= 2 && p[1].abs_diff(p[2]) <= 2,
            "{x},{y}: {p:?}"
        );
    }
    assert!(near(rgba_at(&gray, 35, 35), [255, 255, 255, 255]));

    // Rotate 90 clockwise moves the red (top-left) pixel to the top-right.
    ok(&["image", "op", "remove", d, "t", "gray"]);
    ok(&["image", "op", "add", d, "t", "rotate", "--degrees", "90"]);
    let rotated = ws.path("rot.png");
    ok(&["export", d, rotated.to_str().unwrap()]);
    let top_right = rgba_at(&rotated, 35, 5);
    assert!(
        top_right[0] > 200 && top_right[1] < 60 && top_right[2] < 60,
        "{top_right:?}"
    );

    // Same document renders byte-identically twice (determinism).
    let again = ws.path("rot2.png");
    ok(&["export", d, again.to_str().unwrap()]);
    assert_eq!(fs::read(&rotated).unwrap(), fs::read(&again).unwrap());
}

// ---- processed-result cache (roadmap item 13) ----

#[test]
fn processed_cache_accelerates_but_never_changes_output() {
    let ws = Workspace::new("proc");
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let d = doc.to_str().unwrap();
    let mut big = ::image::RgbaImage::new(320, 320);
    for (x, y, px) in big.enumerate_pixels_mut() {
        *px = ::image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x * y) % 255) as u8, 255]);
    }
    big.save(ws.path("big.png")).unwrap();
    ok(&[
        "image",
        "add",
        d,
        "big",
        "--file",
        ws.path("big.png").to_str().unwrap(),
        "--layer",
        "content",
        "--width",
        "200",
        "--height",
        "200",
        "--embed",
    ]);
    ok(&["image", "op", "add", d, "big", "blur", "--radius", "3"]);
    let cache = ws.path(".pentool/cache/processed");
    let entries = || -> Vec<PathBuf> {
        fs::read_dir(&cache)
            .map(|dir| dir.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default()
    };

    let first = ws.path("first.png");
    ok(&["export", d, first.to_str().unwrap()]);
    assert_eq!(
        entries().len(),
        1,
        "cold render stores one processed result"
    );
    let warm = ws.path("warm.png");
    ok(&["export", d, warm.to_str().unwrap()]);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&warm).unwrap());

    // Corrupted entry: detected by checksum, recomputed, identical output.
    let entry = entries().remove(0);
    let mut bytes = fs::read(&entry).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x55;
    fs::write(&entry, bytes).unwrap();
    let repaired = ws.path("repaired.png");
    ok(&["export", d, repaired.to_str().unwrap()]);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&repaired).unwrap());

    // Truncated and deleted caches likewise cannot change pixels.
    fs::write(&entry, b"PTPROC01").unwrap();
    let truncated = ws.path("truncated.png");
    ok(&["export", d, truncated.to_str().unwrap()]);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&truncated).unwrap());
    fs::remove_dir_all(&cache).unwrap();
    let cold = ws.path("cold.png");
    ok(&["export", d, cold.to_str().unwrap()]);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&cold).unwrap());

    // Changing a parameter changes the key and the pixels; disabling bypasses it.
    ok(&[
        "image",
        "op",
        "set",
        d,
        "big",
        entries_op_id(&doc).as_str(),
        "--radius",
        "6",
    ]);
    let changed = ws.path("changed.png");
    ok(&["export", d, changed.to_str().unwrap()]);
    assert_eq!(entries().len(), 2);
    assert_ne!(fs::read(&first).unwrap(), fs::read(&changed).unwrap());
}

fn entries_op_id(doc: &Path) -> String {
    hero_like(doc, "big")["operations"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn hero_like(doc: &Path, id: &str) -> Value {
    let v = read(doc);
    v["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .unwrap()
        .clone()
}

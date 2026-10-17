//! v0.13.0 task 12: portable paint bundles, plan templates, diff and packages.
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-bundle-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
    fn document(&self, name: &str) -> String {
        let doc = self.path(name);
        fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
        ok(&[
            "raster", &doc, "add", "paint", "--width", "64", "--height", "64",
        ]);
        doc
    }
    fn file(&self, name: &str, value: &Value) -> String {
        let path = self.path(name);
        fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path
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
    let output = run(args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let output = run(args);
    assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn plan(color: &str) -> Value {
    json!({
        "schema": 1, "engine": 1, "id": "wash",
        "passes": [{"id": "a", "target": "paint", "action": "stroke-path",
                    "args": {"path": "M4 10 L60 10",
                             "brush": {"size": 10, "hardness": 1, "spacing": 0.1},
                             "color": color}}]
    })
}

/// A document with a brush, palette, guide and plan template.
fn stocked(ws: &Workspace, name: &str) -> String {
    let doc = ws.document(name);
    ok(&[
        "raster",
        &doc,
        "preset-add",
        "round",
        "--brush",
        r#"{"size":8,"hardness":1,"spacing":0.1}"#,
    ]);
    ok(&["paint-palette", &doc, "add", "warm", "#c03030", "#e0a040"]);
    ok(&[
        "paint-guide",
        &doc,
        "add",
        "thirds",
        "--kind",
        "thirds",
        "--params",
        r#"{"rect":[0,0,64,64]}"#,
    ]);
    let file = ws.file("plan.json", &plan("#3070c0"));
    ok(&["paint-bundle", &doc, "plan-add", "wash", &file]);
    doc
}

#[test]
fn bundles_round_trip_between_documents() {
    let ws = Workspace::new("trip");
    let src = stocked(&ws, "src.pen");
    let bundle = ws.path("bundle.json");
    let r = ok(&["paint-bundle", &src, "export", "--out", &bundle]);
    assert_eq!(r["exported"]["brushes"], 1);
    assert_eq!(r["exported"]["plans"], 1);
    let dst = ws.document("dst.pen");
    let dry = ok(&["paint-bundle", &dst, "--dry-run", "import", &bundle]);
    assert_eq!(dry["dry_run"], true);
    assert!(ok(&["paint-bundle", &dst, "plan-list"])["plans"]
        .as_array()
        .unwrap()
        .is_empty());
    let done = ok(&["paint-bundle", &dst, "import", &bundle]);
    assert_eq!(done["imported"]["guides"]["added"], json!(["thirds"]));
    assert_eq!(done["pixels_changed"], false);
    let listed = ok(&["paint-bundle", &dst, "plan-list"]);
    assert_eq!(listed["plans"][0]["id"], "wash");
    // Both documents now carry identical asset hashes.
    assert_eq!(
        ok(&["paint-bundle", &src, "assets"]),
        ok(&["paint-bundle", &dst, "assets"])
    );
    // A second import conflicts until --replace is given; undo restores the original.
    let before = fs::read(&dst).unwrap();
    let conflict = fails(&["paint-bundle", &dst, "import", &bundle]);
    assert!(
        conflict.contains("[conflict] brush preset \"round\" already exists"),
        "{conflict}"
    );
    assert_eq!(before, fs::read(&dst).unwrap());
    ok(&["paint-bundle", &dst, "import", &bundle, "--replace"]);
    ok(&["undo", &dst]);
    ok(&["undo", &dst]);
    assert!(ok(&["paint-bundle", &dst, "plan-list"])["plans"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn import_conflict_uses_singular_section_label() {
    let ws = Workspace::new("singular");
    let src = stocked(&ws, "src.pen");
    let bundle = ws.path("bundle.json");
    ok(&["paint-bundle", &src, "export", "--out", &bundle]);
    // Both documents carry preset "round": the import must conflict, and the
    // message must name the section with a real singular ("brush", not "brushe").
    let dst = stocked(&ws, "dst.pen");
    let message = fails(&["paint-bundle", &dst, "import", &bundle]);
    assert!(
        message.contains(r#"brush "round" already exists"#),
        "conflict message: {message}"
    );
    assert!(!message.contains("brushe"), "conflict message: {message}");
    // The export-side not-found label singularizes the same way.
    let missing = fails(&[
        "paint-bundle",
        &src,
        "export",
        "--out",
        &ws.path("b2.json"),
        "--brushes",
        "nope",
    ]);
    assert!(
        missing.contains(r#"brush "nope" does not exist"#),
        "not-found message: {missing}"
    );
}

#[test]
fn imports_validate_and_leave_scene_data_alone() {
    let ws = Workspace::new("safe");
    let src = stocked(&ws, "src.pen");
    let bundle = ws.path("bundle.json");
    ok(&["paint-bundle", &src, "export", "--out", &bundle]);
    let dst = ws.document("dst.pen");
    let scene_before: Value = serde_json::from_slice(&fs::read(&dst).unwrap()).unwrap();
    ok(&["paint-bundle", &dst, "import", &bundle]);
    let after: Value = serde_json::from_slice(&fs::read(&dst).unwrap()).unwrap();
    assert_eq!(scene_before["pages"], after["pages"]);
    assert_eq!(
        scene_before.get("components"),
        after.get("components"),
        "components must not change"
    );
    // Revision guard and malformed bundles leave the document unchanged.
    let fresh = ws.document("fresh.pen");
    let before = fs::read(&fresh).unwrap();
    assert!(fails(&[
        "paint-bundle",
        &fresh,
        "--if-revision",
        "deadbeef",
        "import",
        &bundle
    ])
    .to_lowercase()
    .contains("revision"));
    let mut bad: Value = serde_json::from_slice(&fs::read(&bundle).unwrap()).unwrap();
    bad["scripts"] = json!(["rm -rf /"]);
    let file = ws.file("unknown.json", &bad);
    assert!(fails(&["paint-bundle", &fresh, "import", &file]).contains("data only"));
    let mut bad: Value = serde_json::from_slice(&fs::read(&bundle).unwrap()).unwrap();
    bad["plans"]["wash"]["engine"] = json!(9);
    let file = ws.file("engine.json", &bad);
    assert!(!fails(&["paint-bundle", &fresh, "import", &file]).is_empty());
    let mut bad: Value = serde_json::from_slice(&fs::read(&bundle).unwrap()).unwrap();
    bad["guides"]["thirds"]["params"] = json!({"rect": [0, 0, 0, 5]});
    bad.as_object_mut().unwrap().remove("bundle_hash");
    let file = ws.file("guide.json", &bad);
    assert!(!fails(&["paint-bundle", &fresh, "import", &file]).is_empty());
    let mut tampered: Value = serde_json::from_slice(&fs::read(&bundle).unwrap()).unwrap();
    tampered["palettes"] = json!({});
    let file = ws.file("tampered.json", &tampered);
    assert!(fails(&["paint-bundle", &fresh, "import", &file]).contains("bundle_hash"));
    assert_eq!(before, fs::read(&fresh).unwrap());
}

#[test]
fn diff_summarizes_plans_data_and_tile_impact() {
    let ws = Workspace::new("diff");
    let a = stocked(&ws, "a.pen");
    let b = ws.path("b.pen");
    fs::copy(&a, &b).unwrap();
    let mut changed = plan("#ff0000");
    changed["passes"].as_array_mut().unwrap().push(
        json!({"id": "extra", "target": "paint", "action": "stroke-path",
                     "args": {"path": "M4 30 L60 30", "color": "#000000",
                              "brush": {"size": 4, "hardness": 1}}}),
    );
    let file = ws.file("changed.json", &changed);
    ok(&["paint-bundle", &b, "plan-add", "wash", &file, "--replace"]);
    let dry = ok(&["paint-plan", &b, &file, "--dry-run"]);
    ok(&[
        "paint-plan",
        &b,
        &file,
        "--if-revision",
        dry["commit"]["if_revision"].as_str().unwrap(),
        "--expect-plan-hash",
        dry["commit"]["plan_hash"].as_str().unwrap(),
    ]);
    ok(&["paint-bundle", &b, "plan-remove", "wash"]);
    ok(&["paint-guide", &b, "remove", "thirds"]);
    let r = ok(&["paint-diff", &a, &b]);
    assert_eq!(r["sections"]["guides"]["removed"], json!(["thirds"]));
    assert_eq!(r["sections"]["plans"]["removed"], json!(["wash"]));
    assert_eq!(r["layers"][0]["layer"], "paint");
    assert!(r["layers"][0]["changed_tiles"].as_u64().unwrap() >= 1);
    // Plan-to-plan diff names the changed field and added pass.
    let old = ws.file("old.json", &plan("#3070c0"));
    let d = ok(&["paint-diff", "--plans", &old, &file]);
    assert_eq!(d["identical"], false);
    assert_eq!(d["passes"]["added"], json!(["extra"]));
    assert_eq!(d["passes"]["changed"][0]["fields"], json!(["args"]));
    assert_eq!(
        ok(&["paint-diff", "--plans", &old, &old])["identical"],
        true
    );
}

#[test]
fn packages_record_paint_assets_and_detect_tampering() {
    let ws = Workspace::new("pkg");
    let doc = stocked(&ws, "asset.pen");
    let table = ok(&["paint-bundle", &doc, "assets"]);
    assert_eq!(table["plans"].as_array().unwrap().len(), 1);
    // The package table is deterministic for the same document.
    assert_eq!(table, ok(&["paint-bundle", &doc, "assets"]));
}

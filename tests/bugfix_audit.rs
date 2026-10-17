//! Regression tests for the post-v0.13 audit: version preservation, no-op
//! feedback, scene-document font listing and production merge-patch.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-audit-{label}-{}-{}",
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
    fn copy_fixture(&self, fixture: &str, name: &str) -> String {
        let from = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("docs/fixtures")
            .join(fixture);
        let to = self.path(name);
        fs::copy(from, &to).unwrap();
        to
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let out = run(args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

fn version(path: &str) -> u64 {
    let doc: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    doc["version"].as_u64().unwrap()
}

#[test]
fn raster_add_keeps_the_document_version() {
    let ws = Workspace::new("raster");
    let doc = ws.copy_fixture("print-v8-native.pen", "v8.pen");
    assert_eq!(version(&doc), 8);
    ok(&[
        "raster", &doc, "add", "paint", "--width", "16", "--height", "16",
    ]);
    assert_eq!(version(&doc), 8, "raster add must not demote a v8 document");
}

#[test]
fn canvas_reports_unchanged_documents() {
    let ws = Workspace::new("canvas");
    let doc = ws.path("c.pen");
    ok(&["new", &doc, "--width", "400", "--height", "300"]);
    let first = ok(&["canvas", &doc, "--width", "500"]);
    assert_eq!(first["changed"], true);
    let again = ok(&["canvas", &doc, "--width", "500"]);
    assert_eq!(again["changed"], false);
    assert!(again["note"].as_str().unwrap().contains("identical"));
}

#[test]
fn fonts_lists_faces_for_scene_documents() {
    let ws = Workspace::new("fonts");
    let doc = ws.copy_fixture("v4-scene.pen", "v4.pen");
    let out = run(&["fonts", &doc]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn production_set_merge_keeps_unlisted_keys() {
    let ws = Workspace::new("production");
    let doc = ws.copy_fixture("print-v8-native.pen", "p.pen");
    ok(&[
        "color",
        "production",
        "set",
        &doc,
        "--geometry",
        r#"{"trim":{"x":10,"y":10,"width":40,"height":28},"bleed":2,"slug":1}"#,
    ]);
    let replaced = ok(&[
        "color",
        "production",
        "set",
        &doc,
        "--geometry",
        r#"{"trim":{"x":10,"y":10,"width":40,"height":28},"bleed":3}"#,
    ]);
    assert_eq!(replaced["dropped_keys"], serde_json::json!(["slug"]));
    ok(&[
        "color",
        "production",
        "set",
        &doc,
        "--geometry",
        r#"{"slug":2}"#,
        "--merge",
    ]);
    let doc_value: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let production = &doc_value["pages"][0]["production"];
    assert_eq!(production["bleed"], 3);
    assert_eq!(production["slug"], 2);
}

#[test]
fn unreadable_inputs_use_one_missing_resource_envelope() {
    let ws = Workspace::new("missing-input");
    let doc = ws.path("doc.pen");
    ok(&["new", &doc, "--overwrite"]);
    ok(&["migrate", &doc, "--target", "9"]);
    let missing = ws.path("NOPE.csv");
    // The same unreadable-input condition must surface the same envelope
    // across layout/book/idml commands (issue #167).
    for args in [
        vec![
            "layout-merge",
            &doc,
            "--data",
            &missing,
            "--out",
            &ws.path("o.pen"),
        ],
        vec!["idml-import", &missing, &ws.path("o.pen")],
        vec!["idml-export", &missing, &ws.path("o.idml")],
        vec!["layout-style", &doc, "import", &missing],
        vec!["layout-table", &doc, "import", "t1", &missing],
    ] {
        let error = fails(&args);
        assert!(
            error.contains("[missing-resource] cannot read"),
            "{}: {error}",
            args.join(" ")
        );
        // The code must lead the message, not trail a raw cause.
        assert!(
            error.trim_start().starts_with("Error: [missing-resource]"),
            "{}: {error}",
            args.join(" ")
        );
    }
}

#[test]
fn tree_and_search_bounds_are_rounded_to_three_decimals() {
    let ws = Workspace::new("bounds");
    let doc = ws.copy_fixture("v4-scene.pen", "a.pen");
    let mut checked = 0;
    for args in [vec!["tree", &doc], vec!["search", &doc, "title"]] {
        let listed = ok(&args);
        for layer in listed["layers"].as_array().unwrap() {
            for object in layer["objects"].as_array().unwrap() {
                let bounds = &object["bounds"];
                for key in ["x", "y", "width", "height", "right", "bottom"] {
                    let value = bounds[key].as_f64().unwrap();
                    assert!(
                        (value * 1000.0).round() / 1000.0 == value,
                        "{}: {}={} carries float artifacts",
                        args.join(" "),
                        key,
                        value
                    );
                    checked += 1;
                }
                if object["id"].as_str() == Some("title") {
                    // The fixture's 48pt text measured 28.799999999999997
                    // before output rounding (issue #45).
                    assert_eq!(bounds["height"].as_f64().unwrap(), 28.8);
                }
            }
        }
    }
    assert!(checked > 0, "the fixture must emit bounds");
}

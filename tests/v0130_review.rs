//! v0.13.0 task 4: region previews and structural comparison.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-review-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
    /// Document with a 64x64 raster layer holding one red stroke.
    fn document(&self, name: &str, color: &str) -> String {
        let doc = self.file(name);
        fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
        ok(&[
            "raster", &doc, "add", "paint", "--width", "64", "--height", "64",
        ]);
        ok(&[
            "raster",
            &doc,
            "stroke-path",
            "paint",
            "--path",
            "M8 16 L56 16",
            "--brush",
            r#"{"size":12,"hardness":1,"spacing":0.1}"#,
            "--color",
            color,
        ]);
        doc
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

#[test]
fn preview_respects_edge_and_pixel_budgets() {
    let ws = Workspace::new("preview");
    let doc = ws.document("a.pen", "#ff0000");
    let out = ws.file("p.png");
    let full = ok(&[
        "paint-review",
        &doc,
        "preview",
        "--scope",
        "node:paint",
        "--out",
        &out,
    ]);
    assert_eq!(full["source"], serde_json::json!([320, 240]));
    assert_eq!(full["downscaled"], false);
    assert!(fs::metadata(&out).unwrap().len() > 0);
    let small = ok(&[
        "paint-review",
        &doc,
        "preview",
        "--scope",
        "node:paint",
        "--max-edge",
        "16",
    ]);
    assert_eq!(small["preview"], serde_json::json!([16, 12]));
    assert_eq!(small["downscaled"], true);
    assert_eq!(small["source_hash"], full["source_hash"]);
    let budget = ok(&[
        "paint-review",
        &doc,
        "preview",
        "--scope",
        "node:paint",
        "--max-pixels",
        "100",
    ]);
    let p = budget["preview"].as_array().unwrap();
    assert!(p[0].as_u64().unwrap() * p[1].as_u64().unwrap() <= 100);
    let crop = ok(&[
        "paint-review",
        &doc,
        "preview",
        "--scope",
        "node:paint",
        "--rect",
        "0,0,32,32",
    ]);
    assert_eq!(crop["source"], serde_json::json!([32, 32]));
    assert!(fails(&[
        "paint-review",
        &doc,
        "preview",
        "--scope",
        "node:paint",
        "--rect",
        "300,200,64,64"
    ])
    .contains("invalid-request"));
    assert!(fails(&["paint-review", &doc, "preview", "--out", "../escape.png"]).contains(".."));
}

#[test]
fn identical_regions_have_zero_metrics() {
    let ws = Workspace::new("same");
    let a = ws.document("a.pen", "#ff0000");
    let b = ws.document("b.pen", "#ff0000");
    let r = ok(&[
        "paint-review",
        &a,
        "compare",
        "--scope",
        "node:paint",
        "--reference-document",
        &b,
    ]);
    assert_eq!(r["metrics"]["state"], "identical");
    assert_eq!(r["metrics"]["color_mae"]["numerator"], 0);
    assert_eq!(r["metrics"]["silhouette_iou"]["value"], 1.0);
    assert!(r["metrics"]["changed_bounds"].is_null());
    assert_eq!(r["current_hash"], r["reference_hash"]);
}

#[test]
fn different_regions_report_exact_ratios_and_bounds() {
    let ws = Workspace::new("diff");
    let a = ws.document("a.pen", "#ff0000");
    let b = ws.document("b.pen", "#0000ff");
    let heat = ws.file("heat.png");
    let r = ok(&[
        "paint-review",
        &a,
        "compare",
        "--scope",
        "node:paint",
        "--reference-document",
        &b,
        "--heatmap",
        &heat,
    ]);
    let m = &r["metrics"];
    assert_eq!(m["state"], "different");
    assert!(m["color_mae"]["numerator"].as_u64().unwrap() > 0);
    assert_eq!(m["color_mae"]["denominator"], 3 * 255 * 320 * 240);
    assert_eq!(m["silhouette_iou"]["value"], 1.0);
    let bounds = m["changed_bounds"].as_array().unwrap();
    assert!(bounds[2].as_u64().unwrap() > 0 && bounds[3].as_u64().unwrap() <= 240);
    assert!(fs::metadata(&heat).unwrap().len() > 0);
    assert!(r["heatmap"]["sha256"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    // Determinism, and the reference document is never mutated.
    let before = fs::read(&b).unwrap();
    let again = ok(&[
        "paint-review",
        &a,
        "compare",
        "--scope",
        "node:paint",
        "--reference-document",
        &b,
    ]);
    assert_eq!(again["metrics"], r["metrics"]);
    assert_eq!(before, fs::read(&b).unwrap());
}

#[test]
fn compare_against_an_exported_image_and_size_mismatch() {
    let ws = Workspace::new("image");
    let a = ws.document("a.pen", "#ff0000");
    let png = ws.file("ref.png");
    ok(&[
        "paint-review",
        &a,
        "preview",
        "--scope",
        "node:paint",
        "--out",
        &png,
    ]);
    let r = ok(&[
        "paint-review",
        &a,
        "compare",
        "--scope",
        "node:paint",
        "--reference-image",
        &png,
    ]);
    assert_eq!(r["metrics"]["state"], "identical");
    let err = fails(&[
        "paint-review",
        &a,
        "compare",
        "--scope",
        "node:paint",
        "--rect",
        "0,0,32,32",
        "--reference-image",
        &png,
    ]);
    assert!(err.contains("differ in size"), "{err}");
    assert!(
        fails(&["paint-review", &a, "compare", "--scope", "node:paint"]).contains("exactly one")
    );
}

#[test]
fn selection_and_rect_are_exclusive() {
    let ws = Workspace::new("selection");
    let doc = ws.document("a.pen", "#ff0000");
    let err = fails(&[
        "paint-review",
        &doc,
        "preview",
        "--selection",
        "{}",
        "--rect",
        "0,0,8,8",
    ]);
    assert!(err.contains("not both"), "{err}");
}

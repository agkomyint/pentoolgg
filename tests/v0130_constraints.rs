//! v0.13.0 task 5: region-aware painting constraints.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-constraints-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    /// Document with an empty 64x64 raster layer `paint`.
    fn document(&self) -> String {
        let doc = self.0.join("doc.pen").to_string_lossy().into_owned();
        fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
        ok(&[
            "raster", &doc, "add", "paint", "--width", "64", "--height", "64",
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

const BRUSH: &str = r#"{"size":10,"hardness":1,"spacing":0.1}"#;

fn stroke(doc: &str, path: &str, color: &str, constraints: Option<&str>) -> Value {
    let mut args = vec![
        "raster",
        doc,
        "stroke-path",
        "paint",
        "--path",
        path,
        "--brush",
        BRUSH,
        "--color",
        color,
    ];
    if let Some(c) = constraints {
        args.extend(["--constraints", c]);
    }
    ok(&args)
}

fn sample(doc: &str, x: u32, y: u32) -> String {
    let v = ok(&[
        "paint-palette",
        doc,
        "sample",
        "--scope",
        "node:paint",
        "--x",
        &x.to_string(),
        "--y",
        &y.to_string(),
    ]);
    if v["empty"] == true {
        "empty".into()
    } else {
        v["color"].as_str().unwrap().to_owned()
    }
}

fn save_left_half(doc: &str) {
    ok(&[
        "raster",
        doc,
        "select-marquee",
        "paint",
        "--rect",
        "0",
        "0",
        "32",
        "64",
    ]);
    ok(&["raster", doc, "select-save", "paint", "left"]);
    ok(&["raster", doc, "select-clear", "paint"]);
}

#[test]
fn saved_selection_confines_the_stroke_and_is_journaled() {
    let ws = Workspace::new("inside");
    let doc = ws.document();
    save_left_half(&doc);
    let result = stroke(
        &doc,
        "M4 32 L60 32",
        "#ff0000",
        Some(r#"{"inside":["saved:left"]}"#),
    );
    assert!(sample(&doc, 16, 32).starts_with("#ff0000"));
    assert_eq!(sample(&doc, 48, 32), "empty");
    let result = &result["result"];
    assert_eq!(result["constraints"]["algorithm"], 1);
    assert!(result["constraints"]["mask_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    // The temporary active selection does not leak.
    let info = ok(&["raster", &doc, "select-info", "paint"]);
    assert!(info["selection"].is_null(), "{info}");
}

#[test]
fn avoid_and_opaque_clip_and_barriers() {
    let ws = Workspace::new("avoid");
    let doc = ws.document();
    save_left_half(&doc);
    stroke(
        &doc,
        "M4 32 L60 32",
        "#ff0000",
        Some(r#"{"avoid":["saved:left"]}"#),
    );
    assert_eq!(sample(&doc, 16, 32), "empty");
    assert!(sample(&doc, 48, 32).starts_with("#ff0000"));

    // Opaque clip: only existing pixels may change.
    stroke(&doc, "M4 16 L28 16", "#0000ff", None);
    stroke(
        &doc,
        "M4 16 L60 16",
        "#00ff00",
        Some(r#"{"opaque_clip":true}"#),
    );
    assert!(sample(&doc, 16, 16).starts_with("#00ff00"));
    assert_eq!(sample(&doc, 50, 16), "empty");

    // Alpha barrier: opaque pixels stop the stroke, empty ones accept it.
    stroke(
        &doc,
        "M4 16 L60 16",
        "#ffffff",
        Some(r#"{"barriers":[{"kind":"alpha","min":200,"max":255}]}"#),
    );
    assert!(sample(&doc, 16, 16).starts_with("#00ff00"));
    assert!(sample(&doc, 50, 16).starts_with("#ffffff"));
}

#[test]
fn reach_from_stops_at_barriers_and_unreachable_regions_stay_clean() {
    let ws = Workspace::new("reach");
    let doc = ws.document();
    // A vertical wall at x=32.
    stroke(&doc, "M32 0 L32 63", "#000000", None);
    stroke(
        &doc,
        "M4 40 L60 40",
        "#ff0000",
        Some(r#"{"barriers":[{"kind":"alpha","min":1,"max":255}],"reach_from":[4,40]}"#),
    );
    assert!(sample(&doc, 10, 40).starts_with("#ff0000"));
    assert_eq!(sample(&doc, 54, 40), "empty");
    assert!(fails(&[
        "raster",
        &doc,
        "stroke-path",
        "paint",
        "--path",
        "M4 40 L10 40",
        "--constraints",
        r#"{"barriers":[{"kind":"alpha","min":1,"max":255}],"reach_from":[32,10]}"#,
    ])
    .contains("blocked"));
}

#[test]
fn invalid_or_empty_constraints_leave_the_document_unchanged() {
    let ws = Workspace::new("invalid");
    let doc = ws.document();
    let before = fs::read(&doc).unwrap();
    for (spec, needle) in [
        (r#"{"inside":["saved:missing"]}"#, "saved selection"),
        (r#"{"bogus":1}"#, "unknown constraints field"),
        (r#"{"inside":["active"]}"#, "no active selection"),
        (r#"{"barriers":[{"kind":"edge"}]}"#, "edge threshold"),
        (
            r#"{"barriers":[{"kind":"alpha","min":0,"max":255}]}"#,
            "no paintable pixel",
        ),
    ] {
        let err = fails(&[
            "raster",
            &doc,
            "stroke-path",
            "paint",
            "--path",
            "M4 4 L40 4",
            "--constraints",
            spec,
        ]);
        assert!(err.contains(needle), "{spec}: {err}");
        assert_eq!(before, fs::read(&doc).unwrap());
    }
}

#[test]
fn active_selection_is_intersected_and_restored() {
    let ws = Workspace::new("active");
    let doc = ws.document();
    save_left_half(&doc);
    ok(&[
        "raster",
        &doc,
        "select-marquee",
        "paint",
        "--rect",
        "0",
        "0",
        "64",
        "16",
    ]);
    let before = ok(&["raster", &doc, "select-info", "paint"]);
    stroke(
        &doc,
        "M4 8 L60 8",
        "#ff0000",
        Some(r#"{"inside":["saved:left"]}"#),
    );
    stroke(
        &doc,
        "M4 8 L60 40",
        "#ff0000",
        Some(r#"{"inside":["saved:left"]}"#),
    );
    assert!(sample(&doc, 10, 8).starts_with("#ff0000"));
    assert_eq!(sample(&doc, 50, 8), "empty");
    let after = ok(&["raster", &doc, "select-info", "paint"]);
    assert_eq!(before["selection"], after["selection"]);
}

#[test]
fn preview_is_deterministic_and_reports_follow_fields() {
    let ws = Workspace::new("preview");
    let doc = ws.document();
    stroke(&doc, "M10 32 L54 32", "#ffffff", None);
    let spec = r#"{"opaque_clip":true,"follow":"contour"}"#;
    let out = ws.0.join("mask.png").to_string_lossy().into_owned();
    let cell = "2";
    let a = ok(&[
        "paint-constraints",
        &doc,
        "preview",
        "paint",
        "--spec",
        spec,
        "--out",
        &out,
        "--cell",
        cell,
    ]);
    let b = ok(&[
        "paint-constraints",
        &doc,
        "preview",
        "paint",
        "--spec",
        spec,
        "--cell",
        cell,
    ]);
    assert_eq!(a["mask"]["mask_hash"], b["mask"]["mask_hash"]);
    assert_eq!(a["field"]["field_hash"], b["field"]["field_hash"]);
    assert_eq!(a["field"]["mode"], "contour");
    assert!(fs::metadata(&out).unwrap().len() > 0);
    // The horizontal bar's gradient is vertical, so the contour is horizontal.
    let samples = a["field"]["samples"].as_array().unwrap();
    let middle = samples
        .iter()
        .find(|s| (24..40).contains(&s[0].as_i64().unwrap()))
        .expect("a defined cell above or below the middle of the bar");
    let angle = middle[2].as_i64().unwrap();
    assert!(
        angle.rem_euclid(180_000) < 2_000 || angle.rem_euclid(180_000) > 178_000,
        "{angle}"
    );
    // Paint following the contour of the same bar.
    let painted = stroke(&doc, "M10 32 L54 32", "#ff0000", Some(spec));
    assert_eq!(
        painted["result"]["constraints"]["spec"]["follow"],
        "contour"
    );
    assert!(
        fails(&["paint-constraints", &doc, "preview", "nope", "--spec", spec]).contains("nope")
    );
}

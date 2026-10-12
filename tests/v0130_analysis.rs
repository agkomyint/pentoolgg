//! v0.13.0 task 8: compact visual analysis for agents.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-analysis-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    /// 64x64 raster `paint` with a black horizontal bar and a red blob.
    fn document(&self) -> String {
        let doc = self.0.join("doc.pen").to_string_lossy().into_owned();
        fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
        ok(&[
            "raster", &doc, "add", "paint", "--width", "64", "--height", "64",
        ]);
        for (path, size, color) in [
            ("M6 20 L58 20", "10", "#000000"),
            ("M40 48 L44 48", "12", "#ff0000"),
        ] {
            ok(&[
                "raster",
                &doc,
                "stroke-path",
                "paint",
                "--path",
                path,
                "--brush",
                &format!(r#"{{"size":{size},"hardness":1,"spacing":0.1}}"#),
                "--color",
                color,
            ]);
        }
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

fn analyze(doc: &str, extra: &[&str]) -> Value {
    let mut args = vec!["paint-analyze", doc, "--scope", "node:paint"];
    args.extend(extra);
    ok(&args)
}

#[test]
fn analysis_is_deterministic_read_only_and_tagged() {
    let ws = Workspace::new("tags");
    let doc = ws.document();
    let before = fs::read(&doc).unwrap();
    let a = analyze(&doc, &[]);
    let b = analyze(&doc, &[]);
    assert_eq!(a, b);
    assert_eq!(before, fs::read(&doc).unwrap());
    assert_eq!(a["algorithm"], 1);
    assert_eq!(a["renderer"], "pentool-cpu");
    assert_eq!(a["scope"], "node:paint");
    assert!(a["revision"].as_str().unwrap().len() >= 16);
    assert!(a["source_hash"].as_str().unwrap().starts_with("sha256:"));
    assert!(a["analysis_hash"].as_str().unwrap().starts_with("sha256:"));
    // Painting changes the revision and the source hash.
    ok(&[
        "raster",
        &doc,
        "stroke-path",
        "paint",
        "--path",
        "M2 2 L10 2",
        "--color",
        "#00ff00",
    ]);
    let c = analyze(&doc, &[]);
    assert_ne!(a["revision"], c["revision"]);
    assert_ne!(a["source_hash"], c["source_hash"]);
}

#[test]
fn reports_contours_values_palette_and_orientation() {
    let ws = Workspace::new("features");
    let doc = ws.document();
    let r = analyze(&doc, &["--max-edge", "64"]);
    assert!(r["contours"]["matches"].as_u64().unwrap() >= 2, "{r}");
    let first = &r["contours"]["items"][0];
    let b = first["bounds"].as_array().unwrap();
    assert!(b[0].as_u64().unwrap() <= 64 && b[2].as_u64().unwrap() <= 64);
    assert!(first["polyline"].as_array().unwrap().len() >= 2);

    let zones = r["values"]["zones"].as_array().unwrap();
    assert_eq!(zones.len(), 5);
    assert!(zones[0]["pixels"].as_u64().unwrap() > 0, "dark bar: {r}");

    let colors = r["palette"]["colors"].as_array().unwrap();
    assert_eq!(colors[0]["color"], "#000000");
    assert!(colors.iter().any(|c| c["color"] == "#ff0000"));

    // A horizontal bar has a vertical gradient, so strokes run horizontally.
    assert_eq!(r["orientation"]["state"], "ok");
    assert_eq!(r["orientation"]["suggested_stroke_degrees"], 0.0);
    assert!(r["focal"]["items"].as_array().unwrap().len() <= 8);
    assert!(r["gaps"]["coverage"]["value"].as_f64().unwrap() > 0.0);
}

#[test]
fn lists_are_paginated_exactly() {
    let ws = Workspace::new("pages");
    let doc = ws.document();
    let all = analyze(&doc, &["--features", "contours,palette"]);
    assert!(all.get("values").is_none());
    let matches = all["contours"]["matches"].as_u64().unwrap();
    assert!(matches >= 2);
    let one = analyze(&doc, &["--features", "contours", "--limit", "1"]);
    assert_eq!(one["contours"]["returned"], 1);
    assert_eq!(one["contours"]["has_more"], true);
    assert_eq!(one["contours"]["matches"], matches);
    let tail = analyze(
        &doc,
        &[
            "--features",
            "contours",
            "--limit",
            "1",
            "--offset",
            &matches.to_string(),
        ],
    );
    assert_eq!(tail["contours"]["returned"], 0);
    assert_eq!(tail["contours"]["has_more"], false);
}

#[test]
fn rect_scope_reports_scope_coordinates_and_bad_input_fails() {
    let ws = Workspace::new("rect");
    let doc = ws.document();
    let r = analyze(
        &doc,
        &[
            "--rect",
            "32,32,32,32",
            "--features",
            "silhouettes,contours",
        ],
    );
    assert_eq!(r["origin"], serde_json::json!([32, 32]));
    assert_eq!(r["source"], serde_json::json!([32, 32]));
    for item in r["contours"]["items"].as_array().unwrap() {
        let b = item["bounds"].as_array().unwrap();
        assert!(
            b[0].as_u64().unwrap() >= 32 && b[1].as_u64().unwrap() >= 32,
            "{item}"
        );
    }
    for (args, needle) in [
        (vec!["--features", "bokeh"], "unknown feature"),
        (vec!["--limit", "0"], "--limit"),
        (vec!["--values", "1"], "--values"),
        (vec!["--max-edge", "4"], "--max-edge"),
        (vec!["--rect", "0,0,500,500"], "inside"),
    ] {
        let mut full = vec!["paint-analyze", doc.as_str(), "--scope", "node:paint"];
        full.extend(args.iter().copied());
        let err = fails(&full);
        assert!(err.contains(needle), "{args:?}: {err}");
    }
}

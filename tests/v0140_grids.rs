//! v0.14.0 task 7: baseline grids, guides, smart guides, step-and-repeat,
//! frame grids and fitting.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pentool-grids-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Workspace(dir)
    }
    fn doc(&self) -> String {
        self.0.join("doc.pen").to_string_lossy().into_owned()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.0.join("doc.pen")).unwrap()).unwrap()
    }
    fn write(&self, raw: &Value) {
        std::fs::write(
            self.0.join("doc.pen"),
            serde_json::to_vec_pretty(raw).unwrap(),
        )
        .unwrap();
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.0.join("doc.pen")).unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let out = run(args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn rect(id: &str, x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({"kind": "rect", "id": id, "x": x, "y": y, "width": w, "height": h, "fill": "#336699"})
}

fn book(ws: &Workspace) -> String {
    let doc = ws.doc();
    ok(&["new", &doc]);
    ok(&["migrate", &doc, "--target", "9"]);
    doc
}

#[test]
fn baseline_grid_snaps_up_per_document_and_per_frame() {
    let ws = Workspace::new("baseline");
    let doc = book(&ws);
    let r = ok(&["layout-grid", &doc, "baseline-show"]);
    assert!(r["grid"].is_null());
    ok(&[
        "layout-grid",
        &doc,
        "baseline-set",
        "--increment",
        "12",
        "--start",
        "6",
    ]);
    let r = ok(&["layout-grid", &doc, "baseline-snap", "20"]);
    assert_eq!(r["snapped"], 30.0);
    let r = ok(&["layout-grid", &doc, "baseline-snap", "30"]);
    assert_eq!(r["snapped"], 30.0);

    // A frame may carry its own grid, or turn the document's off.
    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": [{"runs": [{"text": "x"}]}]}]);
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "text-frame", "id": "f", "story": "s", "next": null,
         "x": 10, "y": 10, "width": 100, "height": 100}
    ]);
    ws.write(&raw);
    ok(&[
        "layout-grid",
        &doc,
        "baseline-set",
        "--frame",
        "f",
        "--increment",
        "10",
    ]);
    let r = ok(&["layout-grid", &doc, "baseline-snap", "21", "--frame", "f"]);
    assert_eq!(
        (r["snapped"].as_f64(), r["source"].as_str()),
        (Some(30.0), Some("frame"))
    );
    ok(&[
        "layout-grid",
        &doc,
        "baseline-clear",
        "--frame",
        "f",
        "--off",
    ]);
    let r = ok(&["layout-grid", &doc, "baseline-show", "--frame", "f"]);
    assert_eq!(r["source"], "frame-off");

    let before = ws.bytes();
    let err = fails(&["layout-grid", &doc, "baseline-set", "--increment", "0"]);
    assert!(err.contains("increment"), "{err}");
    assert_eq!(before, ws.bytes());
}

#[test]
fn guides_are_paginated_validated_and_derive_margins() {
    let ws = Workspace::new("guides");
    let doc = book(&ws);
    ok(&["parent", &doc, "create", "A", "--margins", "36,36,36,36"]);
    ok(&["parent", &doc, "assign", "page-1", "--parent", "A"]);
    for (id, pos) in [("g1", "100"), ("g2", "200"), ("g3", "300")] {
        ok(&[
            "layout-grid",
            &doc,
            "guide-add",
            id,
            "--page",
            "page-1",
            "--kind",
            "ruler-v",
            "--position",
            pos,
        ]);
    }
    let r = ok(&["layout-grid", &doc, "guide-list", "--limit", "2"]);
    assert_eq!(
        (
            r["matches"].as_u64(),
            r["returned"].as_u64(),
            r["has_more"].as_bool()
        ),
        (Some(3), Some(2), Some(true))
    );
    // Duplicate and missing-page guides are refused.
    assert!(fails(&[
        "layout-grid",
        &doc,
        "guide-add",
        "g1",
        "--page",
        "page-1",
        "--kind",
        "ruler-h",
        "--position",
        "1"
    ])
    .contains("already exists"));
    assert!(fails(&[
        "layout-grid",
        &doc,
        "guide-add",
        "g9",
        "--page",
        "nope",
        "--kind",
        "ruler-h",
        "--position",
        "1"
    ])
    .contains("page"));
    ok(&["layout-grid", &doc, "guide-set", "g1", "--position", "150"]);
    let r = ok(&[
        "layout-grid",
        &doc,
        "guide-list",
        "--page",
        "page-1",
        "--limit",
        "1",
    ]);
    assert_eq!(r["guides"][0]["position"], 150.0);
    // The ID and --page forms are exclusive, and --help says so.
    let both = run(&[
        "layout-grid",
        &doc,
        "guide-remove",
        "g2",
        "--page",
        "page-1",
    ]);
    assert!(!both.status.success());
    assert!(String::from_utf8_lossy(&both.stderr).contains("either a guide ID or --page"));
    let help = run(&["layout-grid", &doc, "guide-remove", "--help"]);
    assert!(String::from_utf8_lossy(&help.stdout).contains("not both"));
    let r = ok(&["layout-grid", &doc, "guide-remove", "g2"]);
    assert_eq!(r["removed"], 1);
    ok(&["layout-grid", &doc, "guide-remove", "--page", "page-1"]);
    let r = ok(&["layout-grid", &doc, "guide-list"]);
    assert_eq!(r["matches"], 0);
    // Derived margin guides use the resolved page geometry (no parent -> zero margins here).
    let r = ok(&[
        "layout-grid",
        &doc,
        "guide-list",
        "--page",
        "page-1",
        "--derived",
    ]);
    assert_eq!(r["matches"], 4);
    let positions: Vec<f64> = r["guides"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["position"].as_f64().unwrap())
        .collect();
    assert_eq!(positions, [36.0, 764.0, 36.0, 1164.0]);
}

#[test]
fn smart_guides_report_alignment_gaps_and_equal_spacing() {
    let ws = Workspace::new("measure");
    let doc = book(&ws);
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        rect("a", 0.0, 100.0, 50.0, 50.0),
        rect("b", 100.0, 100.0, 50.0, 50.0),
        rect("c", 200.0, 100.0, 50.0, 50.0)
    ]);
    ws.write(&raw);
    let r = ok(&["layout-grid", &doc, "measure", "--ids", "a,b,c"]);
    let aligned: Vec<&str> = r["alignments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["edge"].as_str().unwrap())
        .collect();
    assert!(aligned.contains(&"top") && aligned.contains(&"bottom"));
    assert_eq!(r["equal_spacing"][0]["gap"], 50.0);
    assert_eq!(r["equal_spacing"][0]["axis"], "horizontal");
    assert!(fails(&["layout-grid", &doc, "measure", "--ids", "a"]).contains("between 2 and"));
    assert!(fails(&["layout-grid", &doc, "measure", "--ids", "a,zzz"]).contains("not found"));
}

#[test]
fn step_repeat_makes_unique_ids_and_refuses_text_frames_and_collisions() {
    let ws = Workspace::new("repeat");
    let doc = book(&ws);
    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": [{"runs": [{"text": "x"}]}]}]);
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        rect("r", 10.0, 10.0, 20.0, 20.0),
        {"kind": "text-frame", "id": "tf", "story": "s", "next": null,
         "x": 10, "y": 100, "width": 100, "height": 100}
    ]);
    ws.write(&raw);
    let r = ok(&[
        "layout-grid",
        &doc,
        "step-repeat",
        "r",
        "--count",
        "3",
        "--dx",
        "30",
        "--prefix",
        "c",
    ]);
    assert_eq!(r["created"], json!(["c1", "c2", "c3"]));
    let nodes = ws.json()["pages"][0]["layers"][0]["nodes"].clone();
    let xs: Vec<f64> = nodes
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "rect")
        .map(|n| n["x"].as_f64().unwrap())
        .collect();
    assert_eq!(xs, [10.0, 40.0, 70.0, 100.0]);

    let before = ws.bytes();
    assert!(fails(&["layout-grid", &doc, "step-repeat", "tf", "--count", "2"]).contains("thread"));
    assert!(fails(&[
        "layout-grid",
        &doc,
        "step-repeat",
        "r",
        "--count",
        "1",
        "--prefix",
        "c"
    ])
    .contains("already exists"));
    assert!(fails(&["layout-grid", &doc, "step-repeat", "r", "--count", "5000"]).contains("limit"));
    assert_eq!(before, ws.bytes());

    // Dry run reports the same plan and leaves the file untouched.
    let r = ok(&[
        "layout-grid",
        &doc,
        "--dry-run",
        "step-repeat",
        "r",
        "--count",
        "2",
        "--dy",
        "5",
        "--prefix",
        "d",
    ]);
    assert_eq!(r["created"], json!(["d1", "d2"]));
    assert_eq!(before, ws.bytes());
}

#[test]
fn frame_grids_make_graphic_or_threaded_text_frames() {
    let ws = Workspace::new("framegrid");
    let doc = book(&ws);
    let r = ok(&[
        "layout-grid",
        &doc,
        "frame-grid",
        "ph",
        "--page",
        "page-1",
        "--layer",
        "layer-1",
        "--rows",
        "2",
        "--columns",
        "3",
        "--x",
        "0",
        "--y",
        "0",
        "--width",
        "300",
        "--height",
        "100",
        "--gutter-x",
        "0",
        "--gutter-y",
        "0",
    ]);
    assert_eq!(r["created"], 6);
    assert_eq!(r["cell"]["width"], 100.0);
    assert_eq!(r["cell"]["height"], 50.0);

    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": [{"runs": [{"text": "x"}]}]}]);
    ws.write(&raw);
    ok(&[
        "layout-grid",
        &doc,
        "frame-grid",
        "t",
        "--page",
        "page-1",
        "--layer",
        "layer-1",
        "--rows",
        "1",
        "--columns",
        "2",
        "--x",
        "0",
        "--y",
        "200",
        "--width",
        "200",
        "--height",
        "100",
        "--gutter-x",
        "0",
        "--story",
        "s",
    ]);
    let raw = ws.json();
    let nodes = raw["pages"][0]["layers"][0]["nodes"].as_array().unwrap();
    let t1 = nodes.iter().find(|n| n["id"] == "t-r1c1").unwrap();
    assert_eq!(t1["next"], "t-r1c2");

    // Gutters that leave no room are refused without writing.
    let before = ws.bytes();
    let err = fails(&[
        "layout-grid",
        &doc,
        "frame-grid",
        "z",
        "--page",
        "page-1",
        "--layer",
        "layer-1",
        "--rows",
        "1",
        "--columns",
        "4",
        "--x",
        "0",
        "--y",
        "0",
        "--width",
        "30",
        "--height",
        "30",
        "--gutter-x",
        "20",
    ]);
    assert!(err.contains("gutter"), "{err}");
    assert_eq!(before, ws.bytes());
}

#[test]
fn fit_modes_are_idempotent_and_explicit() {
    let ws = Workspace::new("fit");
    let doc = book(&ws);
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "graphic-frame", "id": "fr", "x": 0, "y": 0, "width": 200, "height": 100,
         "fit": "fit", "content": "pic"},
        rect("pic", 0.0, 0.0, 100.0, 100.0)
    ]);
    ws.write(&raw);
    let r = ok(&["layout-grid", &doc, "fit", "fr", "fill"]);
    assert_eq!(r["content_bounds"]["width"], 200.0);
    assert_eq!(r["content_bounds"]["height"], 200.0);
    assert_eq!(r["clipped"], true);
    // Switching modes starts from the remembered natural size, not the filled size.
    let r = ok(&["layout-grid", &doc, "fit", "fr", "fit"]);
    assert_eq!(r["content_bounds"]["width"], 100.0);
    assert_eq!(r["content_bounds"]["x"], 50.0);
    let r = ok(&["layout-grid", &doc, "fit", "fr", "fit-frame-to-content"]);
    assert_eq!(
        ws.json()["pages"][0]["layers"][0]["nodes"][0]["width"],
        100.0
    );
    assert_eq!(r["content_bounds"]["height"], 100.0);
    assert!(fails(&["layout-grid", &doc, "fit", "fr", "stretch"]).contains("mode"));
    assert!(fails(&["layout-grid", &doc, "fit", "pic", "fit"]).contains("graphic frames"));
}

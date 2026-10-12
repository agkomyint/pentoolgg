//! v0.14.0 task 8: text wrap and anchored objects.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pentool-wrap-{name}-{}", std::process::id()));
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
    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": [
        {"runs": [{"text": "alpha beta gamma delta"}]},
        {"runs": [{"text": "second paragraph"}]}
    ]}]);
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "text-frame", "id": "f", "story": "s", "next": null,
         "x": 0, "y": 0, "width": 200, "height": 100},
        rect("r", 80.0, 0.0, 40.0, 50.0)
    ]);
    ws.write(&raw);
    doc
}

fn segs(r: &Value, line: usize) -> Vec<(f64, f64)> {
    r["lines"][line]["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s[0].as_f64().unwrap(), s[1].as_f64().unwrap()))
        .collect()
}

#[test]
fn bounding_box_wrap_splits_lines_and_honors_offsets_and_side() {
    let ws = Workspace::new("bbox");
    let doc = book(&ws);
    ok(&[
        "layout-wrap",
        &doc,
        "set",
        "r",
        "bounding-box",
        "--offset",
        "0,5,0,5",
    ]);
    let r = ok(&["layout-wrap", &doc, "intervals", "f", "--line-height", "10"]);
    assert_eq!(segs(&r, 0), vec![(0.0, 75.0), (125.0, 200.0)]);
    assert_eq!(segs(&r, 5), vec![(0.0, 200.0)]);
    ok(&[
        "layout-wrap",
        &doc,
        "set",
        "r",
        "bounding-box",
        "--side",
        "left",
    ]);
    let r = ok(&["layout-wrap", &doc, "intervals", "f", "--line-height", "10"]);
    assert_eq!(segs(&r, 0), vec![(0.0, 80.0)]);
    ok(&[
        "layout-wrap",
        &doc,
        "set",
        "r",
        "bounding-box",
        "--side",
        "right",
    ]);
    let r = ok(&["layout-wrap", &doc, "intervals", "f", "--line-height", "10"]);
    assert_eq!(segs(&r, 0), vec![(120.0, 200.0)]);
}

#[test]
fn jump_modes_ignore_wrap_and_clear() {
    let ws = Workspace::new("jump");
    let doc = book(&ws);
    ok(&["layout-wrap", &doc, "set", "r", "jump-object"]);
    let r = ok(&["layout-wrap", &doc, "intervals", "f", "--line-height", "10"]);
    assert!(segs(&r, 0).is_empty());
    assert_eq!(segs(&r, 5), vec![(0.0, 200.0)]);
    ok(&["layout-wrap", &doc, "set", "r", "jump-next-column"]);
    let r = ok(&["layout-wrap", &doc, "intervals", "f", "--line-height", "10"]);
    assert!(segs(&r, 9).is_empty());
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["nodes"][0]["ignore_wrap"] = json!(true);
    ws.write(&raw);
    let r = ok(&["layout-wrap", &doc, "intervals", "f", "--line-height", "10"]);
    assert_eq!(segs(&r, 0), vec![(0.0, 200.0)]);
    ok(&["layout-wrap", &doc, "clear", "r"]);
    assert!(ok(&["layout-wrap", &doc, "show", "r"])["wrap"].is_null());
    assert!(fails(&["layout-wrap", &doc, "clear", "r"]).contains("no wrap"));
}

#[test]
fn ellipse_shape_wrap_uses_bands_and_other_shapes_report_approximation() {
    let ws = Workspace::new("shape");
    let doc = book(&ws);
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "text-frame", "id": "f", "story": "s", "next": null,
         "x": 0, "y": 0, "width": 200, "height": 100},
        {"kind": "ellipse", "id": "e", "cx": 100.0, "cy": 40.0, "radius_x": 40.0, "radius_y": 40.0,
         "style": {"fill": "#336699"}}
    ]);
    ws.write(&raw);
    ok(&["layout-wrap", &doc, "set", "e", "shape"]);
    let r = ok(&["layout-wrap", &doc, "intervals", "f", "--line-height", "10"]);
    let top = segs(&r, 0);
    let mid = segs(&r, 4);
    // The ellipse leaves more room near its top than at its middle.
    assert!(top[0].1 > mid[0].1, "{top:?} vs {mid:?}");
    assert!(r["wrappers"][0]["bands"].as_u64().unwrap() > 1);
    assert!(r["wrappers"][0]["approximated"].is_null());
}

#[test]
fn wrap_list_paginates_and_bad_input_is_refused_unchanged() {
    let ws = Workspace::new("list");
    let doc = book(&ws);
    ok(&["layout-wrap", &doc, "set", "r", "bounding-box"]);
    let r = ok(&["layout-wrap", &doc, "list", "--limit", "1"]);
    assert_eq!(r["matches"], 1);
    assert_eq!(r["has_more"], false);
    let before = ws.bytes();
    assert!(fails(&["layout-wrap", &doc, "set", "r", "swirl"]).contains("wrap mode"));
    assert!(fails(&["layout-wrap", &doc, "set", "nope", "shape"]).contains("not found"));
    assert!(
        fails(&["layout-wrap", &doc, "set", "r", "shape", "--offset", "1,2"]).contains("--offset")
    );
    assert!(fails(&["layout-wrap", &doc, "intervals", "r"]).contains("not a text frame"));
    assert_eq!(before, ws.bytes());
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["nodes"][1]["wrap"] = json!({"mode": "weird"});
    ws.write(&raw);
    assert!(fails(&["layout-wrap", &doc, "set", "f", "none"]).contains("wrap.mode"));
}

#[test]
fn anchored_objects_follow_their_story_and_are_validated() {
    let ws = Workspace::new("anchor");
    let doc = book(&ws);
    ok(&[
        "layout-wrap",
        &doc,
        "anchor-add",
        "r",
        "--story",
        "s",
        "--paragraph",
        "0",
        "--at",
        "5",
        "--mode",
        "above-line",
        "--space-before",
        "2",
        "--align",
        "right",
    ]);
    let raw = ws.json();
    let runs = raw["layout"]["stories"][0]["paragraphs"][0]["runs"]
        .as_array()
        .unwrap();
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[1]["anchor"], "r");
    assert_eq!(runs[0]["text"], "alpha");
    assert!(fails(&[
        "layout-wrap",
        &doc,
        "anchor-add",
        "r",
        "--story",
        "s",
        "--paragraph",
        "1"
    ])
    .contains("already anchored"));
    let r = ok(&["layout-wrap", &doc, "anchor-list"]);
    assert_eq!(r["matches"], 1);
    assert_eq!(r["anchors"][0]["estimated"], true);
    assert_eq!(r["anchors"][0]["frame"], "f");
    let r = ok(&["layout-wrap", &doc, "anchor-apply", "--object", "r"]);
    assert_eq!(r["moved"][0]["estimated"], true);
    let moved = ws.json();
    let x = moved["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "r")
        .unwrap()["x"]
        .as_f64()
        .unwrap();
    assert_eq!(x, 160.0);
    ok(&[
        "layout-wrap",
        &doc,
        "anchor-set",
        "r",
        "--mode",
        "inline",
        "--baseline-offset",
        "3",
    ]);
    ok(&["layout-wrap", &doc, "anchor-remove", "r"]);
    assert!(fails(&["layout-wrap", &doc, "anchor-remove", "r"]).contains("not anchored"));
    let mut raw = ws.json();
    raw["layout"]["stories"][0]["paragraphs"][0]["runs"] =
        json!([{"text": "x"}, {"anchor": "ghost"}]);
    ws.write(&raw);
    assert!(fails(&["layout-wrap", &doc, "set", "r", "bounding-box"]).contains("ghost"));
}

#[test]
fn locked_layers_refuse_wrap_changes() {
    let ws = Workspace::new("lock");
    let doc = book(&ws);
    ok(&["layout-wrap", &doc, "set", "r", "bounding-box"]);
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["locked"] = json!(true);
    ws.write(&raw);
    let before = ws.bytes();
    assert!(fails(&["layout-wrap", &doc, "clear", "r"]).contains("locked"));
    assert_eq!(before, ws.bytes());
}

#[test]
fn downgrade_refuses_wrapped_objects() {
    let ws = Workspace::new("downgrade");
    let doc = ws.doc();
    ok(&["new", &doc]);
    ok(&["migrate", &doc, "--target", "9"]);
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["nodes"] = json!([rect("r", 0.0, 0.0, 10.0, 10.0)]);
    ws.write(&raw);
    ok(&["layout-wrap", &doc, "set", "r", "bounding-box"]);
    let before = ws.bytes();
    let error = fails(&["migrate", &doc, "--target", "8"]);
    assert!(error.contains("text wrap on object r"), "{error}");
    assert_eq!(before, ws.bytes());
}

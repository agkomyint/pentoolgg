//! v0.14.0 task 12: data merge, adaptive layout and alternate layouts.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pentool-merge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Workspace(dir)
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn s(&self, rel: &str) -> String {
        self.p(rel).to_string_lossy().into_owned()
    }
    fn bytes(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.p(rel)).unwrap()
    }
    fn json(&self, rel: &str) -> Value {
        serde_json::from_slice(&self.bytes(rel)).unwrap()
    }
    fn write(&self, rel: &str, raw: &Value) {
        std::fs::write(self.p(rel), serde_json::to_vec_pretty(raw).unwrap()).unwrap();
    }
    fn text(&self, rel: &str, text: &str) {
        std::fs::write(self.p(rel), text).unwrap();
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

/// A version 9 template: one 400x300 page with a name frame (story `card`),
/// a photo graphic frame and a plain rectangle.
fn template(ws: &Workspace, rel: &str) {
    let path = ws.s(rel);
    ok(&["new", &path, "--overwrite"]);
    ok(&["migrate", &path, "--target", "9"]);
    let mut raw = ws.json(rel);
    raw["layout"]["stories"] = json!([{
        "id": "card",
        "paragraphs": [{"runs": [{"text": "Hello {{name}}, you owe {{amount}}."}]}]
    }]);
    raw["layout"]["styles"] = json!({"paragraph": {"body": {"size": 10, "leading": 12}}});
    raw["pages"][0]["canvas"]["width"] = json!(400);
    raw["pages"][0]["canvas"]["height"] = json!(300);
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "text-frame", "id": "greeting", "story": "card", "next": null,
         "x": 20, "y": 20, "width": 300, "height": 40},
        {"kind": "graphic-frame", "id": "photo", "x": 20, "y": 80, "width": 100,
         "height": 100, "fit": "fill"},
    ]);
    ws.write(rel, &raw);
}

fn story_text(raw: &Value, id: &str) -> String {
    let story = raw["layout"]["stories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("story {id} missing"));
    story["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["runs"].as_array().unwrap().iter())
        .filter_map(|r| r["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn csv_merge_makes_one_page_per_record_and_leaves_the_template_alone() {
    let ws = Workspace::new("csv");
    template(&ws, "tpl.pen");
    ws.text(
        "people.csv",
        "name,amount\nAda,10\n\"Grace, H.\",20\n\"Zoë\",30\n",
    );
    let before = ws.bytes("tpl.pen");
    let report = ok(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("people.csv"),
        "--out",
        &ws.s("out.pen"),
    ]);
    assert_eq!(report["records"], 3);
    assert_eq!(report["pages_created"], 3);
    assert_eq!(report["overflow_count"], 0);
    assert_eq!(before, ws.bytes("tpl.pen"), "template changed");
    let out = ws.json("out.pen");
    let pages = out["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 3, "the template page is replaced");
    assert_eq!(story_text(&out, "card-m1"), "Hello Ada, you owe 10.");
    assert_eq!(story_text(&out, "card-m2"), "Hello Grace, H., you owe 20.");
    assert_eq!(story_text(&out, "card-m3"), "Hello Zoë, you owe 30.");
    assert!(out["layout"]["stories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["id"] != "card"));
    assert_eq!(pages[1]["layers"][0]["nodes"][0]["story"], "card-m2");
    assert_eq!(pages[1]["layers"][0]["nodes"][0]["id"], "greeting-m2");
    ok(&["tree", &ws.s("out.pen"), "--limit", "1"]);

    // Deterministic.
    ok(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("people.csv"),
        "--out",
        &ws.s("again.pen"),
    ]);
    assert_eq!(ws.bytes("out.pen"), ws.bytes("again.pen"));

    // Refuses to overwrite without the flag.
    let err = fails(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("people.csv"),
        "--out",
        &ws.s("out.pen"),
    ]);
    assert!(err.contains("policy-denied"), "{err}");
}

#[test]
fn keep_template_and_json_data() {
    let ws = Workspace::new("json");
    template(&ws, "tpl.pen");
    ws.text(
        "d.json",
        r#"[{"name":"Ada","amount":10},{"name":"Bo","amount":null,"extra":true}]"#,
    );
    let report = ok(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("d.json"),
        "--out",
        &ws.s("out.pen"),
        "--keep-template",
    ]);
    assert_eq!(report["template_kept"], true);
    let out = ws.json("out.pen");
    assert_eq!(out["pages"].as_array().unwrap().len(), 3);
    assert_eq!(story_text(&out, "card-m2"), "Hello Bo, you owe .");
    assert_eq!(
        story_text(&out, "card"),
        "Hello {{name}}, you owe {{amount}}."
    );
}

#[test]
fn bad_data_and_unknown_fields_write_nothing() {
    let ws = Workspace::new("bad");
    template(&ws, "tpl.pen");
    ws.text("short.csv", "name\nAda\n");
    let err = fails(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("short.csv"),
        "--out",
        &ws.s("out.pen"),
    ]);
    assert!(err.contains("{{amount}}"), "{err}");
    assert!(err.contains("name"), "lists data fields: {err}");
    assert!(!ws.p("out.pen").exists());

    ws.text("ragged.csv", "name,amount\nAda\n");
    let err = fails(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("ragged.csv"),
        "--out",
        &ws.s("out.pen"),
    ]);
    assert!(err.contains("record 1 has 1 values"), "{err}");

    ws.text("dup.csv", "name,name\nA,B\n");
    let err = fails(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("dup.csv"),
        "--out",
        &ws.s("out.pen"),
    ]);
    assert!(err.contains("duplicate field"), "{err}");
    assert!(!ws.p("out.pen").exists());
}

#[test]
fn several_records_per_page_use_a_grid_of_cells() {
    let ws = Workspace::new("grid");
    template(&ws, "tpl.pen");
    // Shrink the template to a 200x100 card on a 400x300 sheet.
    let mut raw = ws.json("tpl.pen");
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "text-frame", "id": "greeting", "story": "card", "next": null,
         "x": 5, "y": 5, "width": 190, "height": 40}
    ]);
    ws.write("tpl.pen", &raw);
    ws.text("d.csv", "name,amount\nA,1\nB,2\nC,3\nD,4\nE,5\n");
    let report = ok(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("d.csv"),
        "--out",
        &ws.s("out.pen"),
        "--records-per-page",
        "4",
        "--grid",
        "2x2",
        "--cell",
        "200x100",
    ]);
    assert_eq!(report["pages_created"], 2);
    let out = ws.json("out.pen");
    let first = out["pages"][0]["layers"][0]["nodes"].as_array().unwrap();
    assert_eq!(first.len(), 4);
    let xs: Vec<(f64, f64)> = first
        .iter()
        .map(|n| (n["x"].as_f64().unwrap(), n["y"].as_f64().unwrap()))
        .collect();
    assert_eq!(
        xs,
        vec![(5.0, 5.0), (205.0, 5.0), (5.0, 105.0), (205.0, 105.0)]
    );
    let second = out["pages"][1]["layers"][0]["nodes"].as_array().unwrap();
    assert_eq!(second.len(), 1, "the last page holds the remainder only");
    assert_eq!(story_text(&out, "card-m2c1"), "Hello E, you owe 5.");

    // A grid that does not match the record count per page is refused.
    let err = fails(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("d.csv"),
        "--out",
        &ws.s("bad.pen"),
        "--records-per-page",
        "3",
        "--grid",
        "2x2",
        "--cell",
        "200x100",
    ]);
    assert!(err.contains("holds 4 cells"), "{err}");
}

#[test]
fn overflow_is_reported_per_record() {
    let ws = Workspace::new("overflow");
    template(&ws, "tpl.pen");
    let long = "word ".repeat(400);
    ws.text(
        "d.csv",
        &format!("name,amount\nShort,1\n\"{long}\",2\nOk,3\n"),
    );
    let report = ok(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("d.csv"),
        "--out",
        &ws.s("out.pen"),
    ]);
    assert_eq!(report["overflow_count"], 1, "{report}");
    assert_eq!(report["overflow"][0]["record"], 2);
    assert_eq!(report["overflow"][0]["estimated"], true);
}

#[test]
fn image_placeholders_use_files_beside_the_output() {
    let ws = Workspace::new("image");
    template(&ws, "tpl.pen");
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255]));
    img.save(ws.p("a.png")).unwrap();
    ws.text("d.csv", "name,amount,pic\nAda,1,a.png\nBo,2,\n");
    let report = ok(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("d.csv"),
        "--out",
        &ws.s("out.pen"),
        "--image-field",
        "photo=pic",
    ]);
    assert_eq!(report["images_distinct"], 1);
    assert_eq!(report["images_empty"], 1);
    let out = ws.json("out.pen");
    let nodes = out["pages"][0]["layers"][0]["nodes"].as_array().unwrap();
    let frame = nodes.iter().find(|n| n["id"] == "photo-m1").unwrap();
    let image_id = frame["content"].as_str().unwrap();
    let node = nodes.iter().find(|n| n["id"] == image_id).unwrap();
    assert_eq!(node["kind"], "image");
    let asset = node["asset"].as_str().unwrap();
    assert_eq!(out["image_assets"][asset]["storage"]["path"], "a.png");
    // The second record has no image: the frame stays empty.
    let second = out["pages"][1]["layers"][0]["nodes"].as_array().unwrap();
    assert!(second
        .iter()
        .find(|n| n["id"] == "photo-m2")
        .unwrap()
        .get("content")
        .is_none());

    // An image outside the output folder is refused.
    ws.text("d2.csv", "name,amount,pic\nAda,1,a.png\n");
    std::fs::create_dir_all(ws.p("sub")).unwrap();
    let err = fails(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("d2.csv"),
        "--out",
        &ws.s("sub/out.pen"),
        "--image-field",
        "photo=pic",
    ]);
    assert!(err.contains("unsafe-path"), "{err}");
    let err = fails(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("d.csv"),
        "--out",
        &ws.s("x.pen"),
        "--image-field",
        "nope=pic",
    ]);
    assert!(err.contains("not a graphic frame"), "{err}");
}

#[test]
fn anchored_objects_in_template_stories_are_refused() {
    let ws = Workspace::new("anchor");
    template(&ws, "tpl.pen");
    let mut raw = ws.json("tpl.pen");
    raw["layout"]["stories"][0]["paragraphs"][0]["runs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"anchor": "pull"}));
    ws.write("tpl.pen", &raw);
    ws.text("d.csv", "name,amount\nA,1\n");
    let err = fails(&[
        "layout-merge",
        &ws.s("tpl.pen"),
        "--data",
        &ws.s("d.csv"),
        "--out",
        &ws.s("out.pen"),
    ]);
    assert!(err.contains("unsupported-capability"), "{err}");
}

fn node<'a>(raw: &'a Value, id: &str) -> &'a Value {
    raw["pages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["layers"][0]["nodes"].as_array().unwrap().iter())
        .find(|n| n["id"] == id)
        .unwrap_or_else(|| panic!("node {id} missing"))
}

#[test]
fn adaptive_rules_replace_objects_for_a_new_page_size() {
    let ws = Workspace::new("adapt");
    template(&ws, "tpl.pen");
    // 400x300 -> 800x600.
    let scale = ws.s("scale.pen");
    std::fs::copy(ws.p("tpl.pen"), &scale).unwrap();
    let r = ok(&[
        "layout-adapt",
        &scale,
        "--page",
        "page-1",
        "--width",
        "800",
        "--height",
        "600",
        "--rule",
        "scale",
    ]);
    assert_eq!(r["objects_adjusted"], 2);
    let doc = ws.json("scale.pen");
    assert_eq!(doc["pages"][0]["canvas"]["width"], 800);
    let f = node(&doc, "greeting");
    assert_eq!(f["x"], 40.0);
    assert_eq!(f["width"], 600.0);
    assert_eq!(f["height"], 80.0);

    let recenter = ws.s("recenter.pen");
    std::fs::copy(ws.p("tpl.pen"), &recenter).unwrap();
    ok(&[
        "layout-adapt",
        &recenter,
        "--page",
        "page-1",
        "--width",
        "800",
        "--height",
        "600",
        "--rule",
        "recenter",
    ]);
    let doc = ws.json("recenter.pen");
    let f = node(&doc, "greeting");
    assert_eq!(f["x"], 220.0);
    assert_eq!(f["y"], 170.0);
    assert_eq!(f["width"], 300.0);

    let object = ws.s("object.pen");
    std::fs::copy(ws.p("tpl.pen"), &object).unwrap();
    ok(&[
        "layout-adapt",
        &object,
        "--page",
        "page-1",
        "--width",
        "800",
        "--height",
        "600",
        "--rule",
        "object",
        "--object",
        "greeting=h:stretch,v:top",
        "--object",
        "photo=h:right,v:bottom",
    ]);
    let doc = ws.json("object.pen");
    let f = node(&doc, "greeting");
    assert_eq!(
        (f["x"].as_f64(), f["width"].as_f64()),
        (Some(20.0), Some(700.0))
    );
    let p = node(&doc, "photo");
    assert_eq!(
        (p["x"].as_f64(), p["y"].as_f64()),
        (Some(420.0), Some(380.0))
    );
    assert_eq!(p["width"], 100.0);

    // Unknown object and a rule mismatch are refused without a write.
    let before = ws.bytes("object.pen");
    let err = fails(&[
        "layout-adapt",
        &object,
        "--page",
        "page-1",
        "--width",
        "10",
        "--height",
        "10",
        "--rule",
        "object",
        "--object",
        "ghost=h:left",
    ]);
    assert!(err.contains("not on page"), "{err}");
    assert_eq!(before, ws.bytes("object.pen"));
    let err = fails(&[
        "layout-adapt",
        &object,
        "--page",
        "page-1",
        "--width",
        "10",
        "--height",
        "10",
        "--rule",
        "scale",
        "--object",
        "photo=h:left",
    ]);
    assert!(err.contains("--rule object"), "{err}");
}

#[test]
fn guide_rule_follows_guides() {
    let ws = Workspace::new("guide");
    template(&ws, "tpl.pen");
    let path = ws.s("tpl.pen");
    // photo spans x 20..120; a guide at 20 and one at 120 pin both edges.
    ok(&[
        "layout-grid",
        &path,
        "guide-add",
        "g1",
        "--page",
        "page-1",
        "--kind",
        "ruler-v",
        "--position",
        "20",
    ]);
    ok(&[
        "layout-grid",
        &path,
        "guide-add",
        "g2",
        "--page",
        "page-1",
        "--kind",
        "ruler-v",
        "--position",
        "120",
    ]);
    ok(&[
        "layout-adapt",
        &path,
        "--page",
        "page-1",
        "--width",
        "800",
        "--height",
        "300",
        "--rule",
        "guide",
    ]);
    let doc = ws.json("tpl.pen");
    let photo = node(&doc, "photo");
    // Both edges sit on guides, so the frame stretches with them: 40..240.
    assert_eq!(photo["x"], 40.0);
    assert_eq!(photo["width"], 200.0);
    assert_eq!(doc["layout"]["guides"][0]["position"], 40.0);
    assert_eq!(doc["layout"]["guides"][1]["position"], 240.0);
    // Unpinned: the frame keeps its size and keeps its proportional center.
    assert_eq!(node(&doc, "greeting")["width"], 300.0);
}

#[test]
fn alternate_layouts_share_linked_stories_and_protect_local_edits() {
    let ws = Workspace::new("alt");
    template(&ws, "tpl.pen");
    let path = ws.s("tpl.pen");
    let r = ok(&[
        "layout-alt",
        &path,
        "create",
        "page-1",
        "tablet",
        "--width",
        "800",
        "--height",
        "600",
        "--rule",
        "scale",
    ]);
    assert_eq!(r["linked_stories"][0]["story"], "card-tablet");
    let doc = ws.json("tpl.pen");
    assert_eq!(doc["pages"].as_array().unwrap().len(), 2);
    assert_eq!(doc["pages"][1]["canvas"]["width"], 800);
    assert_eq!(node(&doc, "greeting-tablet")["width"], 600.0);
    assert_eq!(node(&doc, "greeting-tablet")["story"], "card-tablet");
    assert_eq!(story_text(&doc, "card-tablet"), story_text(&doc, "card"));
    ok(&["tree", &path, "--limit", "1"]);

    // Source edit propagates.
    let mut raw = ws.json("tpl.pen");
    raw["layout"]["stories"][0]["paragraphs"][0]["runs"][0]["text"] = json!("Updated body");
    ws.write("tpl.pen", &raw);
    let r = ok(&["layout-alt", &path, "sync"]);
    assert_eq!(r["updated"].as_array().unwrap().len(), 1);
    assert_eq!(
        story_text(&ws.json("tpl.pen"), "card-tablet"),
        "Updated body"
    );

    // Local edit on the tablet story blocks the next sync.
    let mut raw = ws.json("tpl.pen");
    raw["layout"]["stories"][1]["paragraphs"][0]["runs"][0]["text"] = json!("Tablet only");
    raw["layout"]["stories"][0]["paragraphs"][0]["runs"][0]["text"] = json!("Source v3");
    ws.write("tpl.pen", &raw);
    let r = ok(&["layout-alt", &path, "sync"]);
    assert_eq!(r["skipped"][0]["reason"], "edited-locally");
    assert_eq!(
        story_text(&ws.json("tpl.pen"), "card-tablet"),
        "Tablet only"
    );
    let list = ok(&["layout-alt", &path, "list"]);
    assert_eq!(
        list["alternates"][0]["linked_stories"][0]["edited_locally"],
        true
    );
    let r = ok(&["layout-alt", &path, "sync", "--force"]);
    assert_eq!(r["updated"][0]["overwrote_local_edits"], true);
    assert_eq!(story_text(&ws.json("tpl.pen"), "card-tablet"), "Source v3");

    // Unlink stops syncing.
    ok(&["layout-alt", &path, "unlink", "tablet"]);
    let doc = ws.json("tpl.pen");
    assert!(doc["layout"]["stories"][1].get("linked_to").is_none());
    assert_eq!(doc["layout"]["alternates"].as_array().unwrap().len(), 0);

    // Duplicate IDs are refused.
    let err = fails(&[
        "layout-alt",
        &path,
        "create",
        "page-1",
        "tablet",
        "--width",
        "10",
        "--height",
        "10",
    ]);
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn storyless_documents_support_alt_create_and_image_only_merge() {
    let ws = Workspace::new("storyless");
    // A freshly migrated v9 document has layout {engine, spreads} with no
    // stories key, which the schema allows. Both write paths must tolerate
    // the absent array instead of failing "layout.stories must be an array".
    let alt = ws.s("alt.pen");
    ok(&["new", &alt, "--overwrite"]);
    ok(&["migrate", &alt, "--target", "9"]);
    let mut raw = ws.json("alt.pen");
    assert!(
        raw["layout"].get("stories").is_none(),
        "fixture premise: no stories key"
    );
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "graphic-frame", "id": "g1", "x": 20, "y": 80, "width": 100,
         "height": 100, "fit": "fill"},
    ]);
    ws.write("alt.pen", &raw);

    let r = ok(&[
        "layout-alt",
        &alt,
        "create",
        "page-1",
        "tablet",
        "--width",
        "600",
        "--height",
        "800",
    ]);
    assert_eq!(r["operation"], "layout-alt-create");
    assert_eq!(r["page"], "tablet");
    assert_eq!(r["linked_stories"].as_array().unwrap().len(), 0);
    assert_eq!(ws.json("alt.pen")["pages"].as_array().unwrap().len(), 2);
    ok(&["tree", &alt, "--limit", "1"]);

    // Image-only merge on the same story-less template shape.
    let merge = ws.s("merge.pen");
    ok(&["new", &merge, "--overwrite"]);
    ok(&["migrate", &merge, "--target", "9"]);
    let mut raw = ws.json("merge.pen");
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "graphic-frame", "id": "g1", "x": 20, "y": 80, "width": 100,
         "height": 100, "fit": "fill"},
    ]);
    ws.write("merge.pen", &raw);
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255]));
    img.save(ws.p("a.png")).unwrap();
    ws.text("d.csv", "pic\na.png\n");
    let report = ok(&[
        "layout-merge",
        &merge,
        "--data",
        &ws.s("d.csv"),
        "--out",
        &ws.s("out.pen"),
        "--image-field",
        "g1=pic",
    ]);
    assert_eq!(report["records"], 1);
    assert_eq!(report["images_distinct"], 1);
    let out = ws.json("out.pen");
    assert_eq!(
        out["layout"]["stories"].as_array().unwrap().len(),
        0,
        "merge output carries an empty stories array"
    );
    ok(&["tree", &ws.s("out.pen"), "--limit", "1"]);
}

//! Regression coverage for the v0.7.0 bug report (docs/roadmap/v0.7.0/bug.md).
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);

impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-v071-{label}-{}-{}",
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
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let out = run(args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn tiny_png() -> Vec<u8> {
    let image = ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, ::image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn white_pixels(path: &str) -> usize {
    ::image::open(path)
        .unwrap()
        .to_rgba8()
        .pixels()
        .filter(|p| p.0[0] > 200 && p.0[1] > 200 && p.0[2] > 200)
        .count()
}

fn new_doc(ws: &Workspace, name: &str) -> String {
    let doc = ws.path(name);
    ok(&["new", &doc, "--width", "400", "--height", "160"]);
    doc
}

#[test]
fn text_survives_png_and_pdf_export_when_an_image_is_present() {
    let ws = Workspace::new("text-image");
    let doc = new_doc(&ws, "bug.pen");
    ok(&["canvas", &doc, "--background", "#222222"]);
    ok(&[
        "text",
        &doc,
        "put",
        "t",
        "--layer",
        "layer-1",
        "--content",
        "HELLO",
        "--x",
        "10",
        "--y",
        "90",
        "--size",
        "72",
        "--fill",
        "#ffffff",
    ]);
    let before = ws.path("before.png");
    ok(&["export", &doc, &before]);
    let baseline = white_pixels(&before);
    assert!(baseline > 500, "text should render without an image");

    fs::write(ws.path("tiny.png"), tiny_png()).unwrap();
    ok(&[
        "image",
        "add",
        &doc,
        "pic",
        "--file",
        &ws.path("tiny.png"),
        "--layer",
        "layer-1",
        "--x",
        "320",
        "--y",
        "20",
        "--width",
        "60",
        "--height",
        "60",
        "--embed",
    ]);
    let after = ws.path("after.png");
    ok(&["export", &doc, &after]);
    assert!(
        white_pixels(&after) > baseline / 2,
        "text vanished from the PNG once an image was present"
    );
    let pdf = ws.path("after.pdf");
    ok(&["export", &doc, &pdf]);
    // Image scenes export as raster PDF pages; the rasterized text must be in them.
    assert!(fs::read(&pdf).unwrap().starts_with(b"%PDF"));
    let without_text = ws.path("notext.pen");
    ok(&["new", &without_text, "--width", "400", "--height", "160"]);
    ok(&["canvas", &without_text, "--background", "#222222"]);
    ok(&[
        "image",
        "add",
        &without_text,
        "pic",
        "--file",
        &ws.path("tiny.png"),
        "--layer",
        "layer-1",
        "--x",
        "320",
        "--y",
        "20",
        "--width",
        "60",
        "--height",
        "60",
        "--embed",
    ]);
    let bare = ws.path("notext.pdf");
    ok(&["export", &without_text, &bare]);
    assert_ne!(
        fs::read(&bare).unwrap(),
        fs::read(&pdf).unwrap(),
        "PDF with text should differ from PDF without text"
    );
}

#[test]
fn layer_page_text_object_and_geometry_work_on_scene_documents() {
    let ws = Workspace::new("canvas");
    let doc = new_doc(&ws, "n.pen");
    ok(&["canvas", &doc, "--background", "#222"]);
    ok(&["layer", &doc, "add", "L2"]);
    ok(&["page", &doc, "add", "p2"]);
    let pages = ok(&["page", &doc, "list"]);
    assert_eq!(pages["pages"].as_array().unwrap().len(), 2);
    ok(&[
        "text",
        &doc,
        "put",
        "t1",
        "--layer",
        "layer-1",
        "--content",
        "hi",
        "--x",
        "10",
        "--y",
        "10",
    ]);
    ok(&[
        "object", &doc, "set", "t1", "--layer", "layer-1", "--x", "5",
    ]);
    ok(&[
        "object",
        &doc,
        "duplicate",
        "t1",
        "--layer",
        "layer-1",
        "--new-id",
        "t2",
    ]);
    ok(&[
        "object",
        &doc,
        "move-to-layer",
        "t2",
        "--layer",
        "layer-1",
        "--target-layer",
        "L2",
    ]);
    let before = ok(&[
        "geometry", &doc, "--layer", "layer-1", "--id", "t1", "bounds",
    ]);
    ok(&[
        "geometry",
        &doc,
        "--layer",
        "layer-1",
        "--id",
        "t1",
        "translate",
        "3",
        "4",
    ]);
    let after = ok(&[
        "geometry", &doc, "--layer", "layer-1", "--id", "t1", "bounds",
    ]);
    assert_eq!(
        after["x"].as_f64().unwrap(),
        before["x"].as_f64().unwrap() + 3.0
    );
    ok(&[
        "layer-geometry",
        &doc,
        "--layer",
        "L2",
        "translate",
        "1",
        "1",
    ]);
    ok(&["object", &doc, "remove", "t2", "--layer", "L2"]);
    let message = fails(&["object", &doc, "remove", "t2", "--layer", "L2"]);
    assert!(message.contains("t2"), "{message}");
    ok(&["layer", &doc, "set", "L2", "--locked", "true"]);
    fails(&[
        "text",
        &doc,
        "put",
        "t3",
        "--layer",
        "L2",
        "--content",
        "x",
        "--x",
        "0",
        "--y",
        "0",
    ]);
}

#[test]
fn batch_put_replaces_only_when_asked() {
    let ws = Workspace::new("upsert");
    let doc = new_doc(&ws, "u.pen");
    let ops = ws.path("u.json");
    let write = |fill: &str, mode: &str| {
        fs::write(
            &ops,
            format!(
                r##"[{{"type":"put-shape","shape":"rect","id":"u","layer":"layer-1","x":1,"y":1,"width":5,"height":5,"fill":"{fill}"{mode}}},
{{"type":"put-text","id":"tt","layer":"layer-1","content":"{fill}","x":1,"y":1{mode}}}]"##
            ),
        )
        .unwrap();
    };
    write("#fff", "");
    ok(&["batch", &doc, &ops]);
    let message = fails(&["batch", &doc, &ops]);
    assert!(message.contains("already exists"), "{message}");
    let original = fs::read(&doc).unwrap();
    write("#f00", "");
    fails(&["batch", &doc, &ops]);
    assert_eq!(
        fs::read(&doc).unwrap(),
        original,
        "failed batch must not mutate"
    );

    write("#f00", r#","mode":"replace""#);
    ok(&["batch", &doc, &ops]);
    write("#00f", "");
    ok(&["batch", &doc, &ops, "--upsert"]);
    let text = fs::read_to_string(&doc).unwrap();
    assert!(text.contains("#00f"), "{text}");
    assert_eq!(text.matches("\"id\": \"u\"").count(), 1, "{text}");
    assert_eq!(text.matches("\"id\": \"tt\"").count(), 1, "{text}");
}

#[test]
fn tree_exposes_group_children() {
    let ws = Workspace::new("tree");
    let doc = new_doc(&ws, "g.pen");
    let ops = ws.path("g.json");
    fs::write(
        &ops,
        r##"[{"type":"put-shape","shape":"rect","id":"u","layer":"layer-1","x":1,"y":1,"width":5,"height":5,"fill":"#fff"},
{"type":"create-group","id":"gg","layer":"layer-1","children":["u"]}]"##,
    )
    .unwrap();
    ok(&["batch", &doc, &ops]);
    let tree = ok(&["tree", &doc]);
    let objects = tree["layers"][0]["objects"].as_array().unwrap();
    let group = objects.iter().find(|o| o["id"] == "gg").unwrap();
    assert_eq!(group["children"], serde_json::json!(["u"]));
    let child = objects.iter().find(|o| o["id"] == "u").unwrap();
    assert_eq!(child["parent"], "gg");
    assert_eq!(tree["matches"], 2);
    assert_eq!(tree["returned"], 2);
}

#[test]
fn missing_image_file_reports_code_and_path() {
    let ws = Workspace::new("missing");
    let doc = new_doc(&ws, "m.pen");
    let missing = ws.path("nope.png");
    let message = fails(&[
        "image", "add", &doc, "missing", "--file", &missing, "--layer", "layer-1", "--x", "0",
        "--y", "0", "--width", "10", "--height", "10",
    ]);
    assert!(message.contains("[missing-resource]"), "{message}");
    assert!(message.contains("nope.png"), "{message}");
}

#[test]
fn image_add_and_set_both_document_mask() {
    let add = String::from_utf8_lossy(&run(&["image", "add", "--help"]).stdout).into_owned();
    assert!(add.contains("--mask"));
    let set = String::from_utf8_lossy(&run(&["image", "set", "--help"]).stdout).into_owned();
    assert!(set.contains("--mask"));
    let docs = fs::read_to_string("docs/image-workflow.md").unwrap();
    assert!(docs.contains("`image set`"));
}

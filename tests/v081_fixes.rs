//! Regression coverage for the v0.8.0 bug report (`docs/roadmap/v0.8.0/bug.md`).
use pentool::{composite, effects, mask, preset, scene};
use serde_json::{json, Value};
use std::path::Path;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);

impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-v081-{label}-{}-{}",
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

fn ok(args: &[&str]) -> String {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn tiny_png() -> Vec<u8> {
    let image = ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, ::image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn pixel(path: &str, x: u32, y: u32) -> [u8; 3] {
    let p = ::image::open(path).unwrap().to_rgba8().get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

fn near(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 2)
}

fn token_doc(ws: &Workspace, name: &str, image: bool, v6: bool) -> String {
    let doc = ws.path(name);
    ok(&["new", &doc, "--width", "300", "--height", "150"]);
    ok(&["canvas", &doc, "--background", "#101010"]);
    let batch = ws.path("tok.json");
    fs::write(
        &batch,
        r##"[{"type":"set-style","name":"color.a","style_type":"color","value":"#FF0000"},
{"type":"put-shape","shape":"rect","id":"r","layer":"layer-1","x":10,"y":10,"width":100,"height":100,"fill":"#FF0000","fill_ref":"color.a"}]"##,
    )
    .unwrap();
    ok(&["batch", &doc, &batch]);
    if image {
        let png = ws.path("tiny.png");
        fs::write(&png, tiny_png()).unwrap();
        ok(&[
            "image", "add", &doc, "im", "--file", &png, "--layer", "layer-1", "--x", "150", "--y",
            "10", "--width", "100", "--height", "60", "--embed",
        ]);
    }
    if v6 {
        ok(&["migrate", &doc, "--target", "6"]);
        ok(&[
            "adjustment",
            "add",
            &doc,
            "adj",
            "--kind",
            "invert",
            "--scope",
            "ids:im",
        ]);
    }
    let change = ws.path("chg.json");
    fs::write(
        &change,
        r##"[{"type":"set-style","name":"color.a","style_type":"color","value":"#0000FF"}]"##,
    )
    .unwrap();
    ok(&["batch", &doc, &change]);
    doc
}

#[test]
fn token_changes_reach_every_render_path() {
    let ws = Workspace::new("token");
    for (image, v6) in [(false, false), (true, false), (true, true)] {
        let doc = token_doc(&ws, &format!("t{image}{v6}.pen"), image, v6);
        let png = ws.path("t.png");
        ok(&["export", &doc, &png]);
        assert!(
            near(pixel(&png, 50, 50), [0, 0, 255]),
            "png stale (image={image}, v6={v6}): {:?}",
            pixel(&png, 50, 50)
        );
        if !v6 {
            let svg = ws.path("t.svg");
            ok(&["export", &doc, &svg]);
            let text = fs::read_to_string(&svg).unwrap();
            assert!(!text.contains("#FF0000"), "svg stale (image={image})");
        }
    }
}

fn masked_doc() -> Value {
    // 24x24 canvas, a 6x6 red photo in the middle masked to its left half.
    let mut raw = composite::migrate(scene::new_document(24, 24)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"rect","id":"shape","x":9,"y":9,"width":3,"height":6,"style":{"fill":{"fallback":"#000000"}}},
        {"kind":"rect","id":"photo","x":9,"y":9,"width":6,"height":6,"style":{"fill":{"fallback":"#ff0000"}}}
    ]);
    mask::create(
        &mut raw,
        Path::new("target/v081.pen"),
        None,
        "half",
        "vector:shape",
    )
    .unwrap();
    mask::attach(&mut raw, None, "photo", "half", &json!({})).unwrap();
    raw
}

fn alpha_outside_mask(raw: &Value) -> u32 {
    let image = composite::render(raw, Path::new("target/v081.pen"), None, 1.0).unwrap();
    // Pixels right of the masked half (x >= 12), where only a glow can paint.
    (12..24)
        .flat_map(|x| (0..24).map(move |y| (x, y)))
        .map(|(x, y)| u32::from(image.get_pixel(x, y).0[3]))
        .sum()
}

#[test]
fn effects_follow_the_masked_silhouette() {
    let mut raw = masked_doc();
    assert_eq!(alpha_outside_mask(&raw), 0);
    effects::edit(
        &mut raw,
        None,
        "photo",
        "add",
        "glow",
        &json!({"kind":"outer-glow","params":{"blur":3,"color":"#00ffff"}}),
    )
    .unwrap();
    assert!(
        alpha_outside_mask(&raw) > 0,
        "glow vanished on a masked node"
    );
}

#[test]
fn preset_paste_keeps_the_targets_mask() {
    let mut raw = masked_doc();
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"rect","id":"other","x":0,"y":0,"width":6,"height":6,"style":{"fill":{"fallback":"#00ff00"}}}));
    effects::edit(
        &mut raw,
        None,
        "other",
        "add",
        "glow",
        &json!({"kind":"outer-glow","params":{"blur":3,"color":"#00ffff"}}),
    )
    .unwrap();
    let appearance = preset::capture(&raw, Path::new("target/v081.pen"), None, "other").unwrap();
    preset::apply(&mut raw, None, "photo", &appearance).unwrap();
    let node = &raw["pages"][0]["layers"][0]["nodes"][1];
    assert!(node.get("mask").is_some(), "mask dropped by preset paste");
    assert!(node["effects"].as_array().is_some_and(|e| !e.is_empty()));
}

fn fill_node(fill: Value) -> Value {
    json!({"kind":"fill","id":"g","x":0,"y":0,"width":20,"height":10,"fill":fill})
}

fn stops() -> Value {
    json!([{"position":0,"color":"#000000"},{"position":1,"color":"#ffffff"}])
}

fn gradient_doc(fill: Value) -> Value {
    let mut raw = composite::migrate(scene::new_document(20, 10)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] = json!([fill_node(fill)]);
    raw
}

#[test]
fn gradients_default_to_the_node_bounds_and_reject_page_coordinates() {
    let render = |fill: Value| {
        let raw = gradient_doc(fill);
        scene::validate(&raw)?;
        composite::render(&raw, Path::new("target/v081.pen"), None, 1.0)
    };
    let linear = render(json!({"kind":"linear","stops":stops()})).unwrap();
    assert!(linear.get_pixel(1, 5).0[0] < 40, "left should be dark");
    assert!(linear.get_pixel(18, 5).0[0] > 215, "right should be light");
    let radial = render(json!({"kind":"radial","stops":stops()})).unwrap();
    assert!(radial.get_pixel(10, 5).0[0] < 40, "centre should be dark");
    assert!(radial.get_pixel(0, 0).0[0] > 200, "corner should be light");
    let error =
        render(json!({"kind":"linear","stops":stops(),"x1":400,"y1":300,"x2":900,"y2":300}))
            .unwrap_err()
            .to_string();
    assert!(error.contains("node-local"), "{error}");
}

#[test]
fn small_cli_fixes() {
    let ws = Workspace::new("cli");
    let doc = token_doc(&ws, "c.pen", true, true);
    // B12: effect list needs only the node.
    ok(&[
        "effect",
        "add",
        &doc,
        "im",
        "glow",
        "--kind",
        "outer-glow",
        "--blur",
        "4",
    ]);
    let listed: Value = serde_json::from_str(&ok(&["effect", "list", &doc, "im"])).unwrap();
    assert_eq!(listed["effects"][0]["id"], "glow");
    // B10: a gradient-overlay without params names the required key.
    let failure = run(&[
        "effect",
        "add",
        &doc,
        "im",
        "ov",
        "--kind",
        "gradient-overlay",
    ]);
    let message = String::from_utf8_lossy(&failure.stderr).into_owned();
    assert!(
        !failure.status.success() && message.contains("fill"),
        "{message}"
    );
    // B11: --name belongs to copy/paste.
    let out = ws.path("p.penpreset");
    let failure = run(&["preset", "save", &doc, "im", "--file", &out, "--name", "x"]);
    let message = String::from_utf8_lossy(&failure.stderr).into_owned();
    assert!(message.contains("copy"), "{message}");
    // B09: no embedded payload in the linked report.
    let report = ok(&["linked", "report", &doc]);
    assert!(
        report.len() < 2000,
        "report dumps pixels: {} bytes",
        report.len()
    );
    assert!(report.contains("\"im\""), "{report}");
    // B03: image add accepts --mask.
    let png = ws.path("tiny.png");
    let batch = ws.path("m.json");
    fs::write(
        &batch,
        r##"[{"type":"put-shape","shape":"circle","id":"m","layer":"layer-1","cx":20,"cy":20,"radius":8,"fill":"#000000"}]"##,
    )
    .unwrap();
    ok(&["batch", &doc, &batch]);
    ok(&[
        "image", "add", &doc, "im2", "--file", &png, "--layer", "layer-1", "--x", "0", "--y", "0",
        "--width", "40", "--height", "40", "--mask", "m",
    ]);
    assert!(ok(&["image", "info", &doc, "im2"]).contains("\"m\""));
}

#[test]
fn v6_pdf_keeps_a_text_layer() {
    let ws = Workspace::new("pdf");
    let doc = ws.path("p.pen");
    ok(&["new", &doc, "--width", "300", "--height", "100"]);
    ok(&["canvas", &doc, "--background", "#222222"]);
    ok(&[
        "text",
        &doc,
        "put",
        "t1",
        "--layer",
        "layer-1",
        "--content",
        "Searchable words",
        "--x",
        "10",
        "--y",
        "50",
        "--size",
        "24",
        "--fill",
        "#ffffff",
    ]);
    ok(&["migrate", &doc, "--target", "6"]);
    let pdf = ws.path("p.pdf");
    ok(&["export", &doc, &pdf]);
    let text = String::from_utf8_lossy(&fs::read(&pdf).unwrap()).into_owned();
    assert!(text.contains("(Searchable words) Tj"), "no v6 text layer");
}

#[test]
fn batch_upsert_is_idempotent_and_the_error_names_it() {
    use base64::Engine;
    let ws = Workspace::new("upsert");
    let doc = ws.path("u.pen");
    ok(&["new", &doc, "--width", "100", "--height", "100"]);
    let data = base64::engine::general_purpose::STANDARD.encode(tiny_png());
    let batch = ws.path("u.json");
    fs::write(
        &batch,
        format!(
            r##"[{{"type":"put-shape","shape":"rect","id":"u","layer":"layer-1","x":1,"y":1,"width":5,"height":5,"fill":"#ff0000"}},
{{"type":"put-image","id":"pic","layer":"layer-1","data":"{data}","x":10,"y":10,"width":20,"height":20}}]"##
        ),
    )
    .unwrap();
    ok(&["batch", &doc, &batch]);
    let again = run(&["batch", &doc, &batch]);
    let message = String::from_utf8_lossy(&again.stderr).into_owned();
    assert!(
        !again.status.success() && message.contains("--upsert"),
        "{message}"
    );
    ok(&["batch", &doc, &batch, "--upsert"]);
    ok(&["batch", &doc, &batch, "--upsert"]);
    let tree = ok(&["tree", &doc]);
    assert_eq!(tree.matches("\"kind\": \"image\"").count(), 1, "{tree}");
}

#[test]
fn effects_budget_follows_the_content_not_the_canvas() {
    // B16: a glow on a small circle must export on a large canvas.
    let ws = Workspace::new("glow-large");
    let doc = ws.path("v.pen");
    ok(&["new", &doc, "--width", "1600", "--height", "1600"]);
    ok(&["migrate", &doc, "--target", "6"]);
    let batch = ws.path("v.json");
    fs::write(
        &batch,
        r##"[{"type":"put-shape","shape":"circle","id":"m","layer":"layer-1","cx":800,"cy":800,"radius":70,"fill":"#ff0000"}]"##,
    )
    .unwrap();
    ok(&["batch", &doc, &batch]);
    ok(&[
        "effect",
        "add",
        &doc,
        "m",
        "g",
        "--kind",
        "outer-glow",
        "--color",
        "#22D3EE",
        "--blur",
        "40",
        "--opacity",
        "0.9",
    ]);
    let png = ws.path("v.png");
    ok(&["export", &doc, &png]);
    // Glow is visible just outside the circle, absent far away, circle intact.
    assert!(near(pixel(&png, 800, 800), [255, 0, 0]));
    assert_ne!(pixel(&png, 800 + 85, 800), [255, 255, 255]);
    assert_eq!(pixel(&png, 10, 10), [255, 255, 255]);
}

fn ink_center(png: &str) -> f64 {
    let image = ::image::open(png).unwrap().to_rgb8();
    let (mut min, mut max) = (u32::MAX, 0u32);
    for (x, _, p) in image.enumerate_pixels() {
        if p[0] < 128 {
            min = min.min(x);
            max = max.max(x);
        }
    }
    assert!(min <= max, "no ink found");
    f64::from(min + max) / 2.0
}

#[test]
fn centered_text_box_anchors_inside_its_width() {
    // The renderer used to centre around the box's left edge and clip the text.
    for target in [None, Some("6")] {
        let ws = Workspace::new("textbox-center");
        let doc = ws.path("t.pen");
        ok(&["new", &doc, "--width", "1000", "--height", "200"]);
        if let Some(version) = target {
            ok(&["migrate", &doc, "--target", version]);
        }
        for (id, align, x) in [("c", "center", "100"), ("r", "right", "100")] {
            ok(&[
                "text-box",
                &doc,
                id,
                "--layer",
                "layer-1",
                "--content",
                "MMMM",
                "--x",
                x,
                "--y",
                "20",
                "--width",
                "600",
                "--size",
                "60",
                "--weight",
                "700",
                "--fill",
                "#000000",
                "--align",
                align,
                "--anchor",
                "top",
            ]);
            let png = ws.path("t.png");
            ok(&["export", &doc, &png]);
            let center = ink_center(&png);
            let expected = if align == "center" { 400.0 } else { 700.0 };
            if align == "center" {
                assert!(
                    (center - expected).abs() < 6.0,
                    "{target:?} {align}: {center}"
                );
            } else {
                // Right alignment: the ink hugs the box's right edge (x + width).
                let image = ::image::open(&png).unwrap().to_rgb8();
                let max = image
                    .enumerate_pixels()
                    .filter(|(_, _, p)| p[0] < 128)
                    .map(|(x, _, _)| x)
                    .max()
                    .unwrap();
                assert!(
                    (f64::from(max) - expected).abs() < 8.0,
                    "{target:?} right: {max}"
                );
            }
            ok(&["text", &doc, "remove", "--layer", "layer-1", id]);
        }
    }
}

#[test]
fn import_names_the_unsupported_version_instead_of_a_canvas_error() {
    let ws = Workspace::new("import-version");
    let (a, b) = (ws.path("a.pen"), ws.path("b.pen"));
    ok(&["new", &a, "--width", "200", "--height", "200"]);
    ok(&["new", &b, "--width", "100", "--height", "100"]);
    let out = run(&["import", &a, &b]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[unsupported-version]"), "{stderr}");
    assert!(!stderr.contains("no canvas"), "{stderr}");
}

//! Page production geometry: boxes, PDF point conversion, marks, bleed coverage.
use pentool::color::production;
use serde_json::{json, Value};
use std::path::Path;

fn doc(production: Value, nodes: Value) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = nodes;
    if !production.is_null() {
        raw["pages"][0]["production"] = production;
    }
    raw
}

fn page(raw: &Value) -> &Value {
    &raw["pages"][0]
}

fn near(a: &Value, expected: [f64; 4]) {
    for (got, want) in a.as_array().unwrap().iter().zip(expected) {
        assert!(
            (got.as_f64().unwrap() - want).abs() < 1e-9,
            "{a} != {expected:?}"
        );
    }
}

fn spec() -> Value {
    json!({"units_per_inch":144,"trim":{"x":40,"y":20,"width":120,"height":60},
        "bleed":10,"slug":{"top":10,"right":10,"bottom":10,"left":10},"safe":6,
        "marks":{"crop":true,"registration":true,"offset":10,"length":10}})
}

#[test]
fn boxes_convert_to_pdf_points_with_a_bottom_left_origin() {
    let raw = doc(spec(), json!([]));
    pentool::scene::validate(&raw).unwrap();
    let g = production::resolve(page(&raw), true).unwrap().unwrap();
    assert_eq!((g.media.width, g.media.height), (160.0, 100.0));
    // 144 units/inch is 0.5 point per unit; media origin is (20, 0) in units.
    let boxes = g.pdf_boxes();
    near(&boxes["media"], [0.0, 0.0, 80.0, 50.0]);
    near(&boxes["bleed"], [5.0, 5.0, 75.0, 45.0]);
    near(&boxes["trim"], [10.0, 10.0, 70.0, 40.0]);
    assert!(boxes.get("art").is_none());
    assert_eq!(
        (g.safe.x, g.safe.y, g.safe.width, g.safe.height),
        (46.0, 26.0, 108.0, 48.0)
    );
    let marks = g.marks();
    assert_eq!(marks.len(), 8 + 4);
    assert_eq!(marks[0]["kind"], "crop");
    assert_eq!(marks[0]["x1"], 30.0); // trim.x - offset
    assert_eq!(marks[0]["x2"], 20.0); // trim.x - offset - length
}

#[test]
fn invalid_geometry_names_the_page_and_the_correction() {
    let mut cases: Vec<(Value, &str)> = Vec::new();
    let mut outside = spec();
    outside["trim"]["x"] = json!(150);
    cases.push((outside, "outside"));
    let mut huge = spec();
    huge["bleed"] = json!(30);
    cases.push((huge, "media box must fit the canvas"));
    let mut inside = spec();
    inside["marks"]["offset"] = json!(4);
    cases.push((inside, "start outside the bleed"));
    let mut short = spec();
    short["marks"]["length"] = json!(15);
    cases.push((short, "increase slug"));
    let mut unknown = spec();
    unknown["trap"] = json!(1);
    cases.push((unknown, "unknown field trap"));
    let mut negative = spec();
    negative["bleed"] = json!(-1);
    cases.push((negative, "bleed"));
    let mut unsafe_zone = spec();
    unsafe_zone["safe"] = json!(40);
    cases.push((unsafe_zone, "safe-zone"));
    for (production, needle) in cases {
        let raw = doc(production, json!([]));
        let error = pentool::scene::validate(&raw).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains(needle), "{needle}: {text}");
    }
}

#[test]
fn art_box_conflicts_with_trim_only_for_pdfx() {
    let mut with_art = spec();
    with_art["art"] = json!({"x":50,"y":30,"width":100,"height":40});
    let raw = doc(with_art, json!([]));
    pentool::scene::validate(&raw).unwrap();
    let g = production::resolve(page(&raw), false).unwrap().unwrap();
    near(&g.pdf_boxes()["art"], [15.0, 15.0, 65.0, 35.0]);
    let error = production::resolve(page(&raw), true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("TrimBox or ArtBox"), "{error}");
}

fn rect(x: u32, y: u32, w: u32, h: u32) -> Value {
    json!([{"kind":"rect","id":"paint","x":x,"y":y,"width":w,"height":h,
        "style":{"fill":{"fallback":"#cc0000"}}}])
}

#[test]
fn bleed_coverage_measures_painted_pixels_not_object_bounds() {
    let document = Path::new("target/production.pen");
    let full = doc(spec(), rect(30, 10, 140, 80));
    let report = production::bleed_coverage(&full, document, None, 1.0).unwrap();
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(report[side]["complete"], true, "{side}: {report}");
    }
    // Paint stops at the trim: every bleed band is empty.
    let trimmed = doc(spec(), rect(40, 20, 120, 60));
    let report = production::bleed_coverage(&trimmed, document, None, 1.0).unwrap();
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(report[side]["complete"], false);
        assert_eq!(report[side]["painted_fraction"], 0.0);
    }
    // Bleed only on the left edge.
    let partial = doc(spec(), rect(30, 20, 130, 60));
    let report = production::bleed_coverage(&partial, document, None, 1.0).unwrap();
    assert_eq!(report["left"]["complete"], true);
    assert_eq!(report["right"]["complete"], false);
}

#[test]
fn cli_show_set_and_clear_are_transactional() {
    let dir = std::env::temp_dir().join(format!("pentool-production-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("doc.pen");
    std::fs::write(
        &input,
        serde_json::to_vec(&doc(Value::Null, json!([]))).unwrap(),
    )
    .unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["color", "production"])
            .args(args)
            .arg(&input)
            .output()
            .unwrap()
    };
    let shown = run(&["show"]);
    assert!(!shown.status.success());
    let bad_before = std::fs::read(&input).unwrap();
    let bad = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["color", "production", "set"])
        .arg(&input)
        .args(["--geometry", r#"{"bleed":500}"#])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert_eq!(std::fs::read(&input).unwrap(), bad_before);
    let set = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["color", "production", "set"])
        .arg(&input)
        .args(["--geometry", &spec().to_string()])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );
    let shown = run(&["show"]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let report: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(report["geometry"]["media"]["width"], 160.0);
    let cleared = run(&["clear"]);
    assert!(
        cleared.status.success(),
        "{}",
        String::from_utf8_lossy(&cleared.stderr)
    );
    assert!(!run(&["show"]).status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

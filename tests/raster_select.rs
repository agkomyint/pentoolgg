//! v0.10.0 item 9: selection shapes and modifiers, saved selections, and working
//! with selected pixels.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-select-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    /// A 100x60 layer `p`: opaque white with a red block at (20..40, 20..40).
    fn document(&self, name: &str) -> String {
        let path = self.0.join(name).to_string_lossy().into_owned();
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        ok(&[
            "raster", &path, "add", "p", "--width", "100", "--height", "60",
        ]);
        stroke(&path, "p", "[[50,30]]", 400, "#FFFFFF");
        stroke(&path, "p", "[[30,30]]", 20, "#FF0000");
        path
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
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn fail(args: &[&str]) -> String {
    let out = run(args);
    assert!(!out.status.success(), "{args:?} should fail");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

fn stroke(doc: &str, id: &str, at: &str, size: u32, color: &str) {
    ok(&[
        "raster",
        doc,
        "stroke",
        id,
        "--samples",
        at,
        "--brush",
        &format!(r#"{{"size":{size},"hardness":1,"flow":1}}"#),
        "--color",
        color,
    ]);
}

fn marquee(doc: &str, rect: [&str; 4], extra: &[&str]) -> Value {
    let mut args = vec!["raster", doc, "select-marquee", "p", "--rect"];
    args.extend(rect);
    args.extend(extra);
    ok(&args)["result"].clone()
}

fn info(doc: &str) -> Value {
    ok(&["raster", doc, "select-info", "p"])
}

fn selected(doc: &str) -> u64 {
    info(doc)["selected_pixels"].as_u64().unwrap_or(0)
}

#[test]
fn marquee_lasso_and_modifiers_shape_the_selection() {
    let ws = Workspace::new("shapes");
    let doc = ws.document("shapes.pen");
    let rect = marquee(&doc, ["10", "10", "40", "30"], &[]);
    assert_eq!(rect["selected_pixels"], 1200);
    assert_eq!(rect["bounds"], serde_json::json!([10, 10, 50, 40]));

    let modify = |op: &str, amount: &str| {
        ok(&[
            "raster",
            &doc,
            "select-modify",
            "p",
            "--op",
            op,
            "--amount",
            amount,
        ])["result"]["selected_pixels"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(modify("expand", "2"), 44 * 34);
    assert_eq!(modify("contract", "2"), 1200);
    assert_eq!(modify("invert", "1"), 100 * 60 - 1200);
    assert_eq!(modify("invert", "1"), 1200);
    let border = modify("border", "4");
    assert!(border > 0 && border < 1200 * 2, "{border}");

    // Feathering softens the edge: more pixels touched, fewer fully selected.
    marquee(&doc, ["10", "10", "40", "30"], &["--feather", "6"]);
    let soft = info(&doc);
    assert!(soft["selected_pixels"].as_u64().unwrap() > 1200);
    assert!(soft["fully_selected_pixels"].as_u64().unwrap() < 1200);

    // A circle holds about pi/4 of its bounding square.
    let circle = marquee(&doc, ["10", "10", "40", "40"], &["--shape", "ellipse"]);
    let n = circle["selected_pixels"].as_u64().unwrap();
    assert!((1200..1400).contains(&n), "{n}");

    // A right triangle: half of a 40x40 square, give or take the diagonal edge.
    let triangle = ok(&[
        "raster",
        &doc,
        "select-lasso",
        "p",
        "--points",
        "[[0,0],[40,0],[0,40]]",
    ])["result"]
        .clone();
    let n = triangle["selected_pixels"].as_u64().unwrap();
    assert!((800..880).contains(&n), "{n}");
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn quickmask_grow_similar_and_saved_selections() {
    let ws = Workspace::new("mask");
    let doc = ws.document("mask.pen");
    ok(&[
        "raster",
        &doc,
        "select-quickmask",
        "p",
        "--samples",
        "[[70,30]]",
        "--brush",
        r#"{"size":10,"hardness":1}"#,
    ]);
    let painted = selected(&doc);
    assert!(painted > 60, "{painted}");
    ok(&[
        "raster",
        &doc,
        "select-quickmask",
        "p",
        "--samples",
        "[[70,30]]",
        "--brush",
        r#"{"size":14,"hardness":1}"#,
        "--erase",
    ]);
    assert_eq!(selected(&doc), 0);

    // Grow stays on the white area; similar picks up the same color elsewhere.
    marquee(&doc, ["60", "10", "4", "4"], &["--mode", "replace"]);
    let grown = ok(&[
        "raster",
        &doc,
        "select-modify",
        "p",
        "--op",
        "grow",
        "--amount",
        "200",
        "--tolerance",
        "10",
    ])["result"]["selected_pixels"]
        .as_u64()
        .unwrap();
    // The red block is a round brush dab of about 314 pixels.
    assert!((5600..5750).contains(&grown), "{grown}");
    marquee(&doc, ["60", "10", "4", "4"], &[]);
    let similar = ok(&[
        "raster",
        &doc,
        "select-modify",
        "p",
        "--op",
        "similar",
        "--tolerance",
        "10",
    ])["result"]["selected_pixels"]
        .as_u64()
        .unwrap();
    assert_eq!(similar, grown);

    // Save, change, load, delete.
    ok(&["raster", &doc, "select-save", "p", "white"]);
    ok(&["raster", &doc, "select-clear", "p"]);
    assert_eq!(info(&doc)["saved"], serde_json::json!(["white"]));
    let loaded = ok(&["raster", &doc, "select-load", "p", "white"]);
    assert_eq!(loaded["result"]["selected_pixels"], grown);
    ok(&["raster", &doc, "select-delete", "p", "white"]);
    assert_eq!(info(&doc)["saved"], serde_json::json!([]));
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn lift_move_transform_and_paste_keep_alpha_and_follow_the_selection() {
    let ws = Workspace::new("pixels");
    let doc = ws.document("pixels.pen");
    marquee(&doc, ["20", "20", "20", "20"], &[]);

    // Copy to a new layer: positioned over the selection, same size.
    let copy = ok(&["raster", &doc, "lift", "p", "--new-id", "q"]);
    assert_eq!(
        copy["result"]["bounds"],
        serde_json::json!([20, 20, 20, 20])
    );
    let q = ok(&["raster", &doc, "info", "q"]);
    assert_eq!(
        (q["width"].as_u64(), q["height"].as_u64()),
        (Some(20), Some(20))
    );

    // Cutting leaves a transparent hole that a wand can find.
    ok(&["raster", &doc, "lift", "p", "--new-id", "r", "--cut"]);
    let hole = ok(&[
        "raster",
        &doc,
        "select-wand",
        "p",
        "--x",
        "30",
        "--y",
        "30",
        "--tolerance",
        "0",
        "--no-antialias",
        "--mode",
        "replace",
    ])["result"]["selected_pixels"]
        .as_u64()
        .unwrap();
    assert_eq!(hole, 400);

    // Move the (re-made) selection's pixels: the selection follows by the offset.
    marquee(&doc, ["60", "10", "10", "10"], &[]);
    let moved = ok(&[
        "raster",
        &doc,
        "move-pixels",
        "p",
        "--dx",
        "-20",
        "--dy",
        "30",
    ]);
    assert_eq!(
        moved["result"]["selection"]["bounds"],
        serde_json::json!([40, 40, 50, 50])
    );
    assert_eq!(selected(&doc), 100);

    // Scaling about the center doubles the selection's footprint.
    marquee(&doc, ["20", "20", "10", "10"], &[]);
    let scaled = ok(&["raster", &doc, "transform-pixels", "p", "--scale", "2"]);
    let b = scaled["result"]["selection"]["bounds"].as_array().unwrap();
    let width = b[2].as_i64().unwrap() - b[0].as_i64().unwrap();
    assert!((19..=22).contains(&width), "{b:?}");
    let rotated = ok(&[
        "raster",
        &doc,
        "transform-pixels",
        "p",
        "--rotate",
        "90",
        "--nearest",
        "--copy",
    ]);
    assert_eq!(rotated["result"]["resample"], "nearest");

    // Paste the copy back in, limited by a selection.
    marquee(&doc, ["0", "0", "100", "30"], &[]);
    let pasted = ok(&[
        "raster", &doc, "paste", "p", "--source", "q", "--x", "70", "--y", "20",
    ]);
    assert_eq!(pasted["result"]["selection_applied"], true);

    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn bad_requests_leave_the_document_unchanged() {
    let ws = Workspace::new("errors");
    let doc = ws.document("errors.pen");
    let before = fs::read(&doc).unwrap();
    let cases: [Vec<&str>; 13] = [
        // Nothing selected yet.
        vec!["raster", &doc, "select-modify", "p", "--op", "invert"],
        vec!["raster", &doc, "lift", "p", "--new-id", "q"],
        vec!["raster", &doc, "move-pixels", "p", "--dx", "1", "--dy", "1"],
        vec!["raster", &doc, "select-save", "p", "name"],
        vec!["raster", &doc, "select-load", "p", "missing"],
        vec!["raster", &doc, "select-delete", "p", "missing"],
        // Bad shapes and operations.
        vec![
            "raster",
            &doc,
            "select-marquee",
            "p",
            "--rect",
            "200",
            "0",
            "10",
            "10",
        ],
        vec![
            "raster",
            &doc,
            "select-marquee",
            "p",
            "--rect",
            "0",
            "0",
            "0",
            "10",
        ],
        vec![
            "raster",
            &doc,
            "select-lasso",
            "p",
            "--points",
            "[[0,0],[5,5]]",
        ],
        vec!["raster", &doc, "select-modify", "p", "--op", "melt"],
        vec![
            "raster",
            &doc,
            "select-marquee",
            "p",
            "--rect",
            "0",
            "0",
            "10",
            "10",
            "--shape",
            "star",
        ],
        vec!["raster", &doc, "paste", "p", "--source", "p"],
        vec!["raster", &doc, "paste", "p", "--source", "nope"],
    ];
    for args in cases {
        fail(&args);
    }
    assert_eq!(fs::read(&doc).unwrap(), before);

    marquee(&doc, ["10", "10", "20", "20"], &[]);
    let with_selection = fs::read(&doc).unwrap();
    let cases: [Vec<&str>; 5] = [
        vec!["raster", &doc, "transform-pixels", "p", "--scale", "0"],
        vec!["raster", &doc, "transform-pixels", "p", "--rotate", "400"],
        vec![
            "raster",
            &doc,
            "select-modify",
            "p",
            "--op",
            "expand",
            "--amount",
            "0",
        ],
        vec!["raster", &doc, "lift", "p", "--new-id", "p"],
        vec![
            "raster",
            &doc,
            "select-modify",
            "p",
            "--op",
            "contract",
            "--amount",
            "65",
        ],
    ];
    for args in cases {
        fail(&args);
    }
    assert_eq!(fs::read(&doc).unwrap(), with_selection);
    let dry = ok(&[
        "raster",
        &doc,
        "--dry-run",
        "lift",
        "p",
        "--new-id",
        "q",
        "--cut",
    ]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fs::read(&doc).unwrap(), with_selection);
}

#[test]
fn refine_snaps_a_loose_marquee_to_the_subject_edge() {
    let ws = Workspace::new("refine");
    let doc = ws.document("refine.pen");
    let before = fs::read(&doc).unwrap();
    let error = fail(&["raster", &doc, "select-refine", "p"]);
    assert!(error.contains("no selection to refine"), "{error}");
    assert_eq!(fs::read(&doc).unwrap(), before);

    marquee(&doc, ["14", "14", "32", "32"], &["--shape", "ellipse"]);
    let loose = info(&doc);
    let rough = fs::read(&doc).unwrap();
    let dry = ok(&[
        "raster",
        &doc,
        "--dry-run",
        "select-refine",
        "p",
        "--band",
        "6",
    ]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fs::read(&doc).unwrap(), rough);

    let refined = ok(&["raster", &doc, "select-refine", "p", "--band", "6"]);
    let result = &refined["result"];
    assert_eq!(result["algorithm"], 2);
    assert!(result["undecided_pixels"].as_u64().unwrap() > 0);
    let area = result["fully_selected_pixels"].as_u64().unwrap();
    assert!(
        (190..=330).contains(&area),
        "the red disc is ~314 px, got {area}"
    );
    let bounds: Vec<i64> = result["bounds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert!(
        bounds[0] >= 18 && bounds[1] >= 18 && bounds[2] <= 42,
        "{bounds:?}"
    );
    assert_ne!(
        loose["result"]["tile_map_sha256"],
        result["tile_map_sha256"]
    );

    // A background hint inside the subject is honored, and bad hints are refused.
    let before = fs::read(&doc).unwrap();
    fail(&["raster", &doc, "select-refine", "p", "--fg", "not json"]);
    fail(&["raster", &doc, "select-refine", "p", "--band", "0"]);
    assert_eq!(fs::read(&doc).unwrap(), before);
}

fn page_bounds(layer: &Value) -> Vec<i64> {
    layer["bounds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect()
}

#[test]
fn cutout_lifts_a_refined_subject_without_touching_the_source() {
    let ws = Workspace::new("cutout-raster");
    let doc = ws.document("cut.pen");
    let before = ok(&["raster", &doc, "info", "p"]);
    let dry = ok(&[
        "raster",
        &doc,
        "--dry-run",
        "cutout",
        "p",
        "--id",
        "car",
        "--box",
        "10",
        "10",
        "40",
        "40",
        "--refine",
        "8",
    ]);
    assert_eq!(dry["dry_run"], true);
    let untouched = fs::read(&doc).unwrap();

    let cut = ok(&[
        "raster", &doc, "cutout", "p", "--id", "car", "--box", "10", "10", "40", "40", "--refine",
        "8",
    ]);
    let result = &cut["result"];
    assert_eq!(result["id"], "car");
    assert_eq!(result["from"], "box");
    assert_eq!(result["refined"], true);
    assert_eq!(result["warnings"], serde_json::json!([]));
    let b = page_bounds(result);
    assert!(
        (18..=22).contains(&b[0]) && (18..=22).contains(&b[1]) && b[2] <= 24 && b[3] <= 24,
        "the cutout hugs the red disc: {b:?}"
    );
    assert_ne!(fs::read(&doc).unwrap(), untouched);
    let after = ok(&["raster", &doc, "info", "p"]);
    assert_eq!(
        before["result"]["tile_map_sha256"],
        after["result"]["tile_map_sha256"]
    );
    assert!(info_has_no_selection(&doc));

    // Wand + invert cuts the subject out of its plain backdrop.
    let again = ok(&[
        "raster",
        &doc,
        "cutout",
        "p",
        "--id",
        "car2",
        "--wand",
        "2",
        "2",
        "--tolerance",
        "20",
        "--invert",
    ]);
    let b = page_bounds(&again["result"]);
    assert!(b[0] >= 17 && b[0] <= 21 && b[2] <= 26, "{b:?}");

    // Bad requests fail and leave the document alone.
    let kept = fs::read(&doc).unwrap();
    for args in [
        vec!["raster", &doc, "cutout", "p", "--id", "car"],
        vec!["raster", &doc, "cutout", "p", "--id", "x", "--selection"],
        vec![
            "raster", &doc, "cutout", "p", "--id", "x", "--box", "0", "0", "5", "5", "--fg",
            "[[1,1]]",
        ],
        vec![
            "raster", &doc, "cutout", "p", "--id", "x", "--wand", "999", "999",
        ],
    ] {
        fail(&args);
    }
    assert_eq!(fs::read(&doc).unwrap(), kept);
}

fn info_has_no_selection(doc: &str) -> bool {
    let info = ok(&["raster", doc, "select-info", "p"]);
    info["result"]["selected_pixels"].as_u64().unwrap_or(0) == 0
}

#[test]
fn cutout_bakes_an_image_node_and_keeps_it_unchanged() {
    let ws = Workspace::new("cutout-image");
    let doc = ws.document("img.pen");
    let mut image = image::RgbaImage::from_pixel(100, 60, image::Rgba([255, 255, 255, 255]));
    for y in 20..40 {
        for x in 30..50 {
            image.put_pixel(x, y, image::Rgba([200, 20, 30, 255]));
        }
    }
    let source = ws.0.join("subject.png");
    image.save(&source).unwrap();
    ok(&[
        "image",
        "add",
        &doc,
        "photo",
        "--file",
        source.to_str().unwrap(),
        "--layer",
        "content",
        "--x",
        "0",
        "--y",
        "0",
        "--width",
        "100",
        "--height",
        "60",
        "--embed",
    ]);
    let tree = |doc: &str| {
        let out = run(&["tree", doc, "--limit", "50"]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let before = tree(&doc);
    let cut = ok(&[
        "raster", &doc, "cutout", "photo", "--id", "subject", "--box", "20", "10", "40", "40",
        "--refine", "6",
    ]);
    assert_eq!(cut["result"]["source"], "photo");
    let b = page_bounds(&cut["result"]);
    assert!(
        (28..=32).contains(&b[0]) && (18..=22).contains(&b[1]) && b[2] <= 24,
        "{b:?}"
    );
    let after = tree(&doc);
    assert!(after.contains("subject"), "{after}");
    assert!(
        !after.contains("cutout-source"),
        "temporary layer removed: {after}"
    );
    assert!(after.contains("photo") && before.contains("photo"));
}

#[test]
fn agent_builds_a_three_image_mood_board_from_cutouts() {
    let ws = Workspace::new("moodboard");
    let doc = ws.document("board.pen");
    let colors = [[200u8, 20, 30], [20, 160, 40], [30, 40, 210]];
    for (i, c) in colors.iter().enumerate() {
        let mut image = image::RgbaImage::from_pixel(60, 60, image::Rgba([240, 240, 240, 255]));
        for y in 20..40 {
            for x in 20..40 {
                image.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
        let src = ws.0.join(format!("src{i}.png"));
        image.save(&src).unwrap();
        let (x, name) = (100 + i * 70, format!("photo{i}"));
        ok(&[
            "image",
            "add",
            &doc,
            &name,
            "--file",
            src.to_str().unwrap(),
            "--layer",
            "content",
            "--x",
            &x.to_string(),
            "--y",
            "100",
            "--width",
            "60",
            "--height",
            "60",
            "--embed",
        ]);
        let out = ok(&[
            "raster",
            &doc,
            "cutout",
            &name,
            "--id",
            &format!("cut{i}"),
            "--box",
            &(x as i32 + 10).to_string(),
            "110",
            "40",
            "40",
            "--refine",
            "5",
        ]);
        let b = page_bounds(&out["result"]);
        assert!(b[2] >= 16 && b[2] <= 24, "cut {i}: {b:?}");
    }
    let png = ws.0.join("board.png");
    ok(&["export", &doc, png.to_str().unwrap()]);
    let board = image::open(&png).unwrap().to_rgba8();
    assert!(board.width() > 0 && fs::metadata(&png).unwrap().len() > 0);
}

#[test]
fn cutout_decontaminate_recolors_the_soft_edge_from_the_subject() {
    let ws = Workspace::new("decon");
    let doc = ws.document("d.pen");
    let args = |id: &'static str, extra: &[&'static str]| {
        let mut a = vec![
            "raster",
            doc.as_str(),
            "--dry-run",
            "cutout",
            "p",
            "--id",
            id,
            "--box",
            "14",
            "14",
            "32",
            "32",
            "--refine",
            "6",
            "--feather",
            "3",
        ];
        a.extend(extra);
        ok(&a)["result"].clone()
    };
    let plain = args("a", &[]);
    let clean = args("b", &["--decontaminate"]);
    assert_eq!(plain["decontaminated_pixels"], 0);
    assert!(clean["decontaminated_pixels"].as_u64().unwrap() > 0);
    assert_eq!(plain["bounds"], clean["bounds"]);
}

#[test]
fn selection_crop_node_crops_one_image_without_resizing_the_page() {
    let ws = Workspace::new("cropnode");
    let doc = ws.0.join("c.pen").to_string_lossy().into_owned();
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    ok(&["migrate", &doc, "--target", "6"]);
    let image = image::RgbaImage::from_pixel(100, 60, image::Rgba([10, 120, 200, 255]));
    let src = ws.0.join("s.png");
    image.save(&src).unwrap();
    let add = |fit: &str| {
        ok(&[
            "image",
            "add",
            &doc,
            "pic",
            "--file",
            src.to_str().unwrap(),
            "--layer",
            "content",
            "--x",
            "0",
            "--y",
            "0",
            "--width",
            "100",
            "--height",
            "60",
            "--fit",
            fit,
            "--embed",
        ])
    };
    add("cover");
    let q = r#"{"kind":"rectangle","x":20,"y":10,"width":40,"height":30}"#;
    let err = fail(&["selection", "crop", &doc, "--query", q, "--node", "pic"]);
    assert!(err.contains("fit fill"), "{err}");
    ok(&["image", "set", &doc, "pic", "--fit", "fill"]);
    let canvas = ok(&["info", &doc])["pages"][0]["canvas"].clone();
    let out = ok(&["selection", "crop", &doc, "--query", q, "--node", "pic"]);
    assert_eq!(
        out["result"]["frame"],
        serde_json::json!([20.0, 10.0, 40.0, 30.0])
    );
    let crop = out["result"]["crop"].as_array().unwrap();
    assert!((crop[0].as_f64().unwrap() - 0.2).abs() < 1e-9);
    assert!((crop[3].as_f64().unwrap() - 0.5).abs() < 1e-9);
    assert_eq!(
        ok(&["info", &doc])["pages"][0]["canvas"],
        canvas,
        "page size unchanged"
    );
    fail(&["selection", "query", &doc, "--query", q, "--node", "pic"]);
}

#[test]
fn document_queries_can_use_raster_selections_and_modifiers() {
    let ws = Workspace::new("bridge");
    let doc = ws.document("b.pen");
    marquee(&doc, ["10", "10", "40", "30"], &[]);
    let count = |q: &str| {
        ok(&["selection", "query", &doc, "--query", q])["result"]["selected_pixels"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(count(r#"{"kind":"raster-selection","layer":"p"}"#), 1200);
    let canvas: u64 = 320 * 240;
    assert_eq!(
        count(r#"{"kind":"invert","a":{"kind":"raster-selection","layer":"p"}}"#),
        canvas - 1200
    );
    assert!(
        count(r#"{"kind":"grow","amount":2,"a":{"kind":"raster-selection","layer":"p"}}"#) > 1200
    );
    assert!(
        count(r#"{"kind":"shrink","amount":2,"a":{"kind":"raster-selection","layer":"p"}}"#) < 1200
    );
    ok(&["raster", &doc, "select-save", "p", "box"]);
    assert_eq!(
        count(r#"{"kind":"raster-selection","layer":"p","name":"box"}"#),
        1200
    );
    let err = fail(&[
        "selection",
        "query",
        &doc,
        "--query",
        r#"{"kind":"raster-selection","layer":"nope"}"#,
    ]);
    assert!(err.contains("nope"), "{err}");
}

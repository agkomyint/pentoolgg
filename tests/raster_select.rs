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

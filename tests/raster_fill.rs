//! v0.10.0 item 8: flood fill, magic wand and selections limiting paint.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-fill-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    /// A 100x60 layer `p`: white everywhere, a black vertical wall at x = 50.
    fn document(&self, name: &str) -> String {
        let path = self.0.join(name).to_string_lossy().into_owned();
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        ok(&[
            "raster", &path, "add", "p", "--width", "100", "--height", "60",
        ]);
        stroke(&path, "[[50,30]]", 400, "#FFFFFF");
        stroke(&path, "[[50,-5],[50,65]]", 4, "#000000");
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

fn stroke(doc: &str, at: &str, size: u32, color: &str) -> Value {
    ok(&[
        "raster",
        doc,
        "stroke",
        "p",
        "--samples",
        at,
        "--brush",
        &format!(r#"{{"size":{size},"hardness":1,"flow":1}}"#),
        "--color",
        color,
    ])
}

fn hash(doc: &str) -> String {
    ok(&["raster", doc, "info", "p"])["tile_map_sha256"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn selected(doc: &str) -> u64 {
    ok(&["raster", doc, "select-info", "p"])["selected_pixels"]
        .as_u64()
        .unwrap_or(0)
}

fn journal(doc: &str) -> Vec<Value> {
    fn find(value: &Value) -> Option<&Value> {
        match value {
            Value::Object(map) => {
                if map.get("id").and_then(Value::as_str) == Some("p") && map.contains_key("journal")
                {
                    Some(value)
                } else {
                    map.values().find_map(find)
                }
            }
            Value::Array(items) => items.iter().find_map(find),
            _ => None,
        }
    }
    let raw: Value = serde_json::from_str(&fs::read_to_string(doc).unwrap()).unwrap();
    find(&raw).unwrap()["journal"].as_array().unwrap().clone()
}

#[test]
fn fill_stops_at_the_wall_and_is_deterministic() {
    let ws = Workspace::new("fill");
    let (a, b) = (ws.document("a.pen"), ws.document("b.pen"));
    let fill = |doc: &str| {
        ok(&[
            "raster",
            doc,
            "fill",
            "p",
            "--x",
            "10",
            "--y",
            "10",
            "--color",
            "#FF0000",
            "--no-antialias",
        ])
    };
    let result = fill(&a);
    fill(&b);
    assert_eq!(hash(&a), hash(&b));
    let filled = result["result"]["pixels_filled"].as_u64().unwrap();
    assert!(
        (48 * 60 - 120..=48 * 60 + 120).contains(&filled),
        "{filled}"
    );
    // The far side is untouched: filling it from there changes it too.
    let other = ok(&[
        "raster",
        &a,
        "fill",
        "p",
        "--x",
        "90",
        "--y",
        "10",
        "--color",
        "#00FF00",
        "--no-antialias",
    ]);
    assert!(other["result"]["pixels_filled"].as_u64().unwrap() > 1000);
    let verify = ok(&["raster", &a, "verify", "p"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn selections_limit_strokes_and_replay_with_the_pinned_selection() {
    let ws = Workspace::new("select");
    let doc = ws.document("select.pen");
    let wand = ok(&[
        "raster",
        &doc,
        "select-wand",
        "p",
        "--x",
        "10",
        "--y",
        "10",
        "--no-antialias",
    ]);
    assert!(wand["result"]["selected_pixels"].as_u64().unwrap() > 2000);
    assert_eq!(selected(&doc), wand["result"]["selected_pixels"]);

    // A stroke across the whole layer only changes the left half.
    stroke(&doc, "[[0,30],[99,30]]", 20, "#0000FF");
    let entry = journal(&doc).pop().unwrap();
    assert!(entry["selection"]["tiles"].is_object());
    let verify = ok(&["raster", &doc, "verify", "p", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
    assert_eq!(verify["layers"][0]["replay"]["status"], "match");

    // The right half stayed white: filling it later sees one white region.
    ok(&["raster", &doc, "select-clear", "p"]);
    let right = ok(&[
        "raster",
        &doc,
        "fill",
        "p",
        "--x",
        "95",
        "--y",
        "5",
        "--color",
        "#00FF00",
        "--no-antialias",
        "--tolerance",
        "0",
    ]);
    let pixels = right["result"]["pixels_filled"].as_u64().unwrap();
    assert!(pixels > 48 * 60 - 120, "right half was untouched: {pixels}");

    // Replay still works after the live selection was cleared.
    let verify = ok(&["raster", &doc, "verify", "p", "--replay"]);
    assert_eq!(verify["layers"][0]["replay"]["status"], "match", "{verify}");
}

#[test]
fn wand_modes_refine_the_selection_and_clear_removes_it() {
    let ws = Workspace::new("modes");
    let doc = ws.document("modes.pen");
    let wand = |x: &str, mode: &str| {
        ok(&[
            "raster",
            &doc,
            "select-wand",
            "p",
            "--x",
            x,
            "--y",
            "10",
            "--mode",
            mode,
            "--no-antialias",
        ])["result"]["selected_pixels"]
            .as_u64()
            .unwrap()
    };
    let left = wand("10", "replace");
    let both = wand("90", "add");
    assert!(both > left);
    let only_right = wand("10", "subtract");
    assert_eq!(only_right, both - left);
    assert_eq!(wand("10", "intersect"), 0);
    ok(&["raster", &doc, "select-clear", "p"]);
    assert_eq!(
        ok(&["raster", &doc, "select-info", "p"])["selection"],
        Value::Null
    );
}

#[test]
fn bad_requests_leave_the_document_unchanged() {
    let ws = Workspace::new("errors");
    let doc = ws.document("errors.pen");
    let before = fs::read(&doc).unwrap();
    let cases: [Vec<&str>; 5] = [
        vec![
            "raster", &doc, "fill", "p", "--x", "100", "--y", "0", "--color", "#FF0000",
        ],
        vec![
            "raster", &doc, "fill", "p", "--x", "1", "--y", "1", "--color", "red",
        ],
        vec![
            "raster", &doc, "fill", "p", "--x", "1", "--y", "1", "--color", "#FF0000", "--gap", "9",
        ],
        vec![
            "raster", &doc, "fill", "p", "--x", "1", "--y", "1", "--color", "#FF0000", "--global",
            "--gap", "2",
        ],
        vec![
            "raster",
            &doc,
            "select-wand",
            "p",
            "--x",
            "1",
            "--y",
            "1",
            "--mode",
            "subtract",
        ],
    ];
    for args in cases {
        fail(&args);
    }
    assert_eq!(fs::read(&doc).unwrap(), before);
    let dry = ok(&[
        "raster",
        &doc,
        "--dry-run",
        "fill",
        "p",
        "--x",
        "1",
        "--y",
        "1",
        "--color",
        "#FF0000",
    ]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fs::read(&doc).unwrap(), before);

    // A selection that no longer matches the layer blocks painting with advice.
    ok(&["raster", &doc, "select-wand", "p", "--x", "1", "--y", "1"]);
    ok(&[
        "raster", &doc, "resize", "p", "--width", "120", "--height", "60",
    ]);
    let message = fail(&[
        "raster",
        &doc,
        "stroke",
        "p",
        "--samples",
        "[[5,5]]",
        "--color",
        "#FF0000",
    ]);
    assert!(message.contains("select-clear"), "{message}");
}

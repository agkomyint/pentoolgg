//! v0.10.0 item 6: clone stamp with pinned sources, through the CLI.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-clone-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    /// A document with raster layers `src` and `p`, both 300x200.
    fn document(&self, name: &str) -> String {
        let path = self.0.join(name).to_string_lossy().into_owned();
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        for id in ["src", "p"] {
            ok(&[
                "raster", &path, "add", id, "--width", "300", "--height", "200",
            ]);
        }
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

fn hash(doc: &str, id: &str) -> String {
    ok(&["raster", doc, "info", id])["tile_map_sha256"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn find_node<'a>(value: &'a Value, id: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            if map.get("id").and_then(Value::as_str) == Some(id) && map.contains_key("journal") {
                Some(value)
            } else {
                map.values().find_map(|v| find_node(v, id))
            }
        }
        Value::Array(items) => items.iter().find_map(|v| find_node(v, id)),
        _ => None,
    }
}

fn load(doc: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(doc).unwrap()).unwrap()
}

fn paint(doc: &str, id: &str, at: &str, size: u32, color: &str) {
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

fn source(doc: &str, layer: Option<&str>) {
    let mut args = vec!["raster", doc, "clone-source", "p"];
    if let Some(layer) = layer {
        args.extend(["--layer", layer]);
    }
    args.extend(["--x", "50", "--y", "50"]);
    ok(&args);
}

#[test]
fn clone_copies_pixels_from_a_pinned_layer_and_survives_source_edits() {
    let ws = Workspace::new("pinned");
    let doc = ws.document("pinned.pen");
    paint(&doc, "src", "[[50,50]]", 40, "#C82828");
    source(&doc, Some("src"));
    let result = ok(&[
        "raster",
        &doc,
        "clone-stroke",
        "p",
        "--samples",
        "[[150,100]]",
        "--brush",
        r#"{"size":12,"hardness":1,"flow":1}"#,
    ]);
    assert!(result["result"]["tiles_changed"].as_u64().unwrap() > 0);
    assert_eq!(result["result"]["clone"]["source"], "src");

    // Fully inside the opaque source disc, so it equals painting that color there.
    let reference = ws.document("reference.pen");
    paint(&reference, "p", "[[150,100]]", 12, "#C82828");
    assert_eq!(hash(&doc, "p"), hash(&reference, "p"));

    let raw = load(&doc);
    let entry = &find_node(&raw, "p").unwrap()["journal"][0];
    assert_eq!(entry["blend"], "clone");
    assert_eq!(entry["clone"]["source"]["layer"], "src");
    let pinned = entry["clone"]["source"]["tiles"]["0,0"].as_str().unwrap();
    assert!(raw["raster_tiles"].get(pinned).is_some());

    // Wiping and repainting the source must not reinterpret the old clone.
    let before = hash(&doc, "p");
    ok(&["raster", &doc, "clear", "src"]);
    paint(&doc, "src", "[[50,50]]", 40, "#2028C8");
    assert!(
        load(&doc)["raster_tiles"].get(pinned).is_some(),
        "journal pin keeps the source tile"
    );
    assert_eq!(hash(&doc, "p"), before);
    let verify = ok(&["raster", &doc, "verify", "p", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
    assert_eq!(verify["layers"][0]["replay"]["status"], "match");
}

#[test]
fn clone_from_the_same_layer_reads_the_pre_stroke_pixels() {
    let ws = Workspace::new("current");
    let doc = ws.document("current.pen");
    paint(&doc, "p", "[[50,50]]", 40, "#C82828");
    source(&doc, None);
    ok(&[
        "raster",
        &doc,
        "clone-stroke",
        "p",
        "--samples",
        "[[150,100],[200,100]]",
        "--brush",
        r#"{"size":12,"hardness":1,"flow":1}"#,
    ]);
    let raw = load(&doc);
    let entry = &find_node(&raw, "p").unwrap()["journal"][1];
    assert_eq!(entry["clone"]["source"], "current");
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn aligned_sessions_keep_one_offset_and_plain_strokes_restart() {
    let ws = Workspace::new("aligned");
    let doc = ws.document("aligned.pen");
    paint(&doc, "src", "[[50,50]]", 40, "#C82828");
    source(&doc, Some("src"));
    let stroke = |at: &str, aligned: bool| {
        let mut args = vec![
            "raster",
            &doc,
            "clone-stroke",
            "p",
            "--samples",
            at,
            "--brush",
            r#"{"size":10,"hardness":1,"flow":1}"#,
        ];
        if aligned {
            args.push("--aligned");
        }
        ok(&args)
    };
    stroke("[[150,100]]", true);
    stroke("[[160,100]]", true);
    stroke("[[170,120]]", false);
    let raw = load(&doc);
    let journal = find_node(&raw, "p").unwrap()["journal"].as_array().unwrap();
    assert_eq!(journal[1]["clone"]["ax"], 150.0, "aligned anchor persists");
    assert_eq!(journal[2]["clone"]["ax"], 170.0, "plain stroke restarts");
    source(&doc, Some("src"));
    assert!(load(&doc)["raster_clone"]["p"]["anchor"].is_null());
}

#[test]
fn bad_clone_requests_leave_the_document_unchanged() {
    let ws = Workspace::new("errors");
    let doc = ws.document("errors.pen");
    let at = "[[150,100]]";
    let message = fail(&["raster", &doc, "clone-stroke", "p", "--samples", at]);
    assert!(message.contains("clone source"), "{message}");
    source(&doc, Some("src"));
    let before = fs::read(&doc).unwrap();
    let cases: [Vec<&str>; 4] = [
        vec![
            "raster",
            &doc,
            "clone-source",
            "p",
            "--layer",
            "nope",
            "--x",
            "1",
            "--y",
            "1",
        ],
        vec![
            "raster",
            &doc,
            "clone-stroke",
            "p",
            "--samples",
            at,
            "--scale",
            "0",
        ],
        vec![
            "raster",
            &doc,
            "clone-stroke",
            "p",
            "--samples",
            at,
            "--angle",
            "500",
        ],
        vec![
            "raster",
            &doc,
            "stroke",
            "p",
            "--samples",
            at,
            "--blend",
            "clone",
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
        "clone-stroke",
        "p",
        "--samples",
        at,
    ]);
    assert_eq!(dry["dry_run"], json!(true));
    assert_eq!(fs::read(&doc).unwrap(), before);
}

//! v0.10.0 item 10: rotate, flip, merge visible, stamp visible and flatten.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-compose-{label}-{}-{}",
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

fn result(args: &[&str]) -> Value {
    ok(args)["result"].clone()
}

#[test]
fn rotate_and_flip_are_lossless_and_move_the_selection() {
    let ws = Workspace::new("orient");
    let doc = ws.document("orient.pen");
    ok(&[
        "raster",
        &doc,
        "select-marquee",
        "p",
        "--rect",
        "0",
        "0",
        "30",
        "30",
    ]);
    let before = fs::read(&doc).unwrap();
    assert!(fail(&["raster", &doc, "rotate", "p", "--degrees", "45"]).contains("90, 180 or 270"));
    assert!(fail(&["raster", &doc, "flip", "p", "diagonal"]).contains("horizontal or vertical"));
    assert_eq!(
        before,
        fs::read(&doc).unwrap(),
        "failures leave the file unchanged"
    );

    let turned = result(&["raster", &doc, "rotate", "p", "--degrees", "90"]);
    assert_eq!(
        (turned["width"].as_u64(), turned["height"].as_u64()),
        (Some(60), Some(100))
    );
    // The selected top-left corner is now the top-right corner.
    let sel = ok(&["raster", &doc, "select-info", "p"]);
    assert_eq!(sel["bounds"], serde_json::json!([30, 0, 60, 30]), "{sel}");
    let hash = turned["tile_map_sha256"].clone();
    for _ in 0..3 {
        result(&["raster", &doc, "rotate", "p", "--degrees", "90"]);
    }
    let a = result(&["raster", &doc, "flip", "p", "horizontal"]);
    let b = result(&["raster", &doc, "flip", "p", "horizontal"]);
    assert_ne!(a["tile_map_sha256"], b["tile_map_sha256"]);
    let full = result(&["raster", &doc, "rotate", "p", "--degrees", "270"]);
    let again = result(&["raster", &doc, "rotate", "p", "--degrees", "90"]);
    assert_eq!(again["tile_map_sha256"], hash);
    assert_ne!(full["tile_map_sha256"], hash);
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn merge_stamp_and_flatten_composite_the_visible_page() {
    let ws = Workspace::new("flatten");
    let doc = ws.document("flatten.pen");
    ok(&[
        "raster", &doc, "add", "q", "--width", "100", "--height", "60",
    ]);
    stroke(&doc, "q", "[[70,30]]", 20, "#0000FF");

    let stamped = result(&["raster", &doc, "stamp-visible", "--new-id", "stamp"]);
    assert!(stamped["tiles"].as_u64().unwrap() > 0, "{stamped}");
    // The stamp covers at least the red dab and the fixture's own content.
    let b = stamped["bounds"].as_array().unwrap();
    assert!(
        b[2].as_u64().unwrap() >= 20 && b[3].as_u64().unwrap() >= 20,
        "{stamped}"
    );
    assert!(fail(&["raster", &doc, "stamp-visible", "--new-id", "stamp"]).contains("stamp"));

    // Merge the visible rasters p, q and stamp into the bottom layer.
    let merged = result(&["raster", &doc, "merge-visible", "q"]);
    assert_eq!(merged["layers"], 3, "{merged}");
    assert!(fail(&["raster", &doc, "merge-visible", "p"]).contains("no other visible raster"));

    let flat = result(&["raster", &doc, "flatten", "--new-id", "flat"]);
    assert!(flat["nodes_replaced"].as_u64().unwrap() >= 1, "{flat}");
    assert!(fail(&["raster", &doc, "flatten", "--new-id", "flat"]).contains("flat"));
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

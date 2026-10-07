//! v0.10.0 raster-paint layers: creation, deterministic strokes, atomicity, history.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-raster-{label}-{}-{}",
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
    fn document(&self, name: &str) -> String {
        let path = self.path(name);
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
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

const SAMPLES: &str = "[[20,30,0.2],[300.5,120.25,1],[560,260,0.5]]";
const BRUSH: &str = r#"{"kind":"soft-round","size":30,"hardness":0.4,"flow":0.7,"pressure_size":true,"scatter":0.1}"#;

fn paint(doc: &str, id: &str) -> Value {
    ok(&[
        "raster",
        doc,
        "stroke",
        id,
        "--samples",
        SAMPLES,
        "--brush",
        BRUSH,
        "--color",
        "#D2A184",
        "--seed",
        "7",
    ])
}

#[test]
fn same_stroke_gives_identical_tile_hashes_in_independent_documents() {
    let ws = Workspace::new("determinism");
    let (a, b) = (ws.document("a.pen"), ws.document("b.pen"));
    for doc in [&a, &b] {
        ok(&[
            "raster", doc, "add", "p", "--width", "600", "--height", "300",
        ]);
        paint(doc, "p");
    }
    let (ia, ib) = (
        ok(&["raster", &a, "info", "p"]),
        ok(&["raster", &b, "info", "p"]),
    );
    assert_eq!(ia["tile_map_sha256"], ib["tile_map_sha256"]);
    assert_eq!(ia["tiles"], 4);
    assert_eq!(ia["journal_entries"], 1);
}

#[test]
fn dry_run_matches_commit_and_leaves_the_file_untouched() {
    let ws = Workspace::new("dry");
    let doc = ws.document("a.pen");
    ok(&[
        "raster", &doc, "add", "p", "--width", "600", "--height", "300",
    ]);
    let before = fs::read(&doc).unwrap();
    let dry = ok(&[
        "raster",
        &doc,
        "--dry-run",
        "stroke",
        "p",
        "--samples",
        SAMPLES,
        "--brush",
        BRUSH,
        "--seed",
        "7",
    ]);
    assert_eq!(fs::read(&doc).unwrap(), before);
    let real = ok(&[
        "raster",
        &doc,
        "stroke",
        "p",
        "--samples",
        SAMPLES,
        "--brush",
        BRUSH,
        "--seed",
        "7",
    ]);
    assert_eq!(dry["result"], real["result"]);
}

#[test]
fn failed_stroke_leaves_the_document_byte_for_byte_unchanged() {
    let ws = Workspace::new("atomic");
    let doc = ws.document("a.pen");
    ok(&[
        "raster", &doc, "add", "p", "--width", "600", "--height", "300",
    ]);
    let before = fs::read(&doc).unwrap();
    for bad in [
        vec!["--samples", "[]"],
        vec!["--samples", "[[1,2,9]]"],
        vec!["--samples", "[[1,2]]", "--brush", r#"{"kind":"airbrush"}"#],
        vec!["--samples", "[[1,2]]", "--color", "red"],
        vec!["--samples", "[[1,2]]", "--blend", "multiply"],
    ] {
        let mut args = vec!["raster", doc.as_str(), "stroke", "p"];
        args.extend(bad);
        fail(&args);
        assert_eq!(fs::read(&doc).unwrap(), before);
    }
    assert!(
        fail(&["raster", &doc, "stroke", "nope", "--samples", "[[1,2]]"]).contains("not-found")
    );
}

#[test]
fn undo_restores_the_pre_stroke_pixels() {
    let ws = Workspace::new("undo");
    let doc = ws.document("a.pen");
    ok(&[
        "raster", &doc, "add", "p", "--width", "600", "--height", "300",
    ]);
    let empty = ok(&["raster", &doc, "info", "p"]);
    paint(&doc, "p");
    assert_ne!(
        ok(&["raster", &doc, "info", "p"])["tile_map_sha256"],
        empty["tile_map_sha256"]
    );
    ok(&["undo", &doc]);
    assert_eq!(
        ok(&["raster", &doc, "info", "p"])["tile_map_sha256"],
        empty["tile_map_sha256"]
    );
}

#[test]
fn raster_layers_are_discoverable_and_render() {
    let ws = Workspace::new("render");
    let doc = ws.document("a.pen");
    ok(&[
        "raster", &doc, "add", "p", "--width", "600", "--height", "300",
    ]);
    paint(&doc, "p");
    let tree = ok(&["tree", &doc, "--kind", "raster"]);
    assert_eq!(tree["layers"][0]["objects"][0]["kind"], "raster");
    assert_eq!(tree["layers"][0]["objects"][0]["id"], "p");
    let png = ws.path("out.png");
    assert!(run(&["export", &doc, &png]).status.success());
    let image = image::open(&png).unwrap().to_rgba8();
    assert!(image.pixels().any(|p| p.0[..3] == [210, 161, 132]));
}

#[test]
fn checkpoint_compact_drops_journal_and_keeps_pixels() {
    let ws = Workspace::new("checkpoint");
    let doc = ws.document("a.pen");
    ok(&[
        "raster", &doc, "add", "p", "--width", "600", "--height", "300",
    ]);
    paint(&doc, "p");
    paint(&doc, "p");
    let before = ok(&["raster", &doc, "info", "p"]);
    assert_eq!(before["journal_entries"], 2);
    ok(&["raster", &doc, "checkpoint", "p", "--compact"]);
    let after = ok(&["raster", &doc, "info", "p"]);
    assert_eq!(after["journal_entries"], 0);
    assert_eq!(after["journal_dropped"], 2);
    assert_eq!(after["tile_map_sha256"], before["tile_map_sha256"]);
}

#[test]
fn clear_releases_tiles_and_downgrade_to_v5_is_refused() {
    let ws = Workspace::new("clear");
    let doc = ws.document("a.pen");
    ok(&[
        "raster", &doc, "add", "p", "--width", "600", "--height", "300",
    ]);
    paint(&doc, "p");
    let refused = fail(&["migrate", &doc, "--target", "5"]);
    assert!(refused.contains("raster"), "{refused}");
    ok(&["raster", &doc, "clear", "p"]);
    let raw: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    assert_eq!(raw["raster_tiles"], json!({}));
    assert_eq!(ok(&["raster", &doc, "info", "p"])["tiles"], 0);
}

#[test]
fn tampered_or_missing_tiles_are_rejected_with_a_fix() {
    let ws = Workspace::new("tamper");
    let doc = ws.document("a.pen");
    ok(&[
        "raster", &doc, "add", "p", "--width", "600", "--height", "300",
    ]);
    paint(&doc, "p");
    let mut raw: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let digest = raw["raster_tiles"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let store = raw["raster_tiles"].as_object_mut().unwrap();
    let entry = store.remove(&digest).unwrap();
    let mut damaged = raw.clone();
    fs::write(&doc, serde_json::to_vec(&damaged).unwrap()).unwrap();
    assert!(fail(&["raster", &doc, "info", "p"]).contains("missing-resource"));
    damaged["raster_tiles"]
        .as_object_mut()
        .unwrap()
        .insert(digest.replace(&digest[7..9], "ff"), entry);
    fs::write(&doc, serde_json::to_vec(&damaged).unwrap()).unwrap();
    assert!(!run(&["raster", &doc, "info", "p"]).status.success());
}

#[test]
fn oversized_requests_fail_before_work() {
    let ws = Workspace::new("limits");
    let doc = ws.document("a.pen");
    assert!(
        fail(&["raster", &doc, "add", "p", "--width", "20000", "--height", "10"])
            .contains("limit-exceeded")
    );
    ok(&[
        "raster", &doc, "add", "p", "--width", "100", "--height", "100",
    ]);
    let huge = fail(&[
        "raster",
        &doc,
        "stroke",
        "p",
        "--samples",
        "[[0,0],[100000,100000]]",
        "--brush",
        r#"{"size":2048,"spacing":0.01}"#,
    ]);
    assert!(huge.contains("limit-exceeded"), "{huge}");
}

#[test]
fn normative_fixture_replays_to_its_recorded_hash() {
    let ws = Workspace::new("fixture");
    let doc = ws.document("a.pen");
    ok(&[
        "raster", &doc, "add", "paint", "--width", "300", "--height", "120",
    ]);
    ok(&[
        "raster",
        &doc,
        "stroke",
        "paint",
        "--samples",
        "[[10,60,0.3],[150,30,1],[290,90,0.5]]",
        "--brush",
        r#"{"kind":"soft-round","size":16,"hardness":0.5,"pressure_size":true}"#,
        "--color",
        "#D2A184",
        "--seed",
        "1",
    ]);
    let fixture = ok(&["raster", "docs/fixtures/raster-v6.pen", "info", "paint"]);
    let replay = ok(&["raster", &doc, "info", "paint"]);
    assert_eq!(fixture["tile_map_sha256"], replay["tile_map_sha256"]);
    assert_eq!(
        fixture["tile_map_sha256"],
        "sha256:1c6569e6066dc9a572836335460b7becd147b4a113f5a1ecc23d25f710036139"
    );
    assert!(fail(&[
        "raster",
        "docs/fixtures/raster-v6-missing-tile.pen",
        "info",
        "paint"
    ])
    .contains("missing-resource"));
}

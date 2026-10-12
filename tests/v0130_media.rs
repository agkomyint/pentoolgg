//! v0.13.0 task 7: material paint modes.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-media-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    /// Document with an empty 64x64 raster layer `paint`.
    fn document(&self) -> String {
        self.named("doc")
    }
    fn named(&self, name: &str) -> String {
        let doc = self
            .0
            .join(format!("{name}.pen"))
            .to_string_lossy()
            .into_owned();
        fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
        ok(&[
            "raster", &doc, "add", "paint", "--width", "64", "--height", "64",
        ]);
        doc
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
    let output = run(args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let output = run(args);
    assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn tile_map(doc: &str) -> Value {
    ok(&["raster", doc, "info", "paint"])["tile_map_sha256"].clone()
}

fn brush(media: &str) -> String {
    format!(r#"{{"size":12,"hardness":1,"spacing":0.1,"media":{media}}}"#)
}

fn stroke(doc: &str, path: &str, color: &str, brush: &str, seed: &str) -> Value {
    ok(&[
        "raster",
        doc,
        "stroke-path",
        "paint",
        "--path",
        path,
        "--brush",
        brush,
        "--color",
        color,
        "--seed",
        seed,
    ])
}

fn sample(doc: &str, x: u32, y: u32) -> [i32; 3] {
    let v = ok(&[
        "paint-palette",
        doc,
        "sample",
        "--scope",
        "node:paint",
        "--x",
        &x.to_string(),
        "--y",
        &y.to_string(),
    ]);
    let hex = v["color"].as_str().unwrap_or("#000000");
    let n = i32::from_str_radix(&hex[1..7], 16).unwrap();
    [n >> 16 & 255, n >> 8 & 255, n & 255]
}

const KINDS: [&str; 10] = [
    "glaze",
    "dry-brush",
    "bristle",
    "impasto",
    "palette-knife",
    "watercolor-wash",
    "charcoal",
    "pencil",
    "canvas-grain",
    "paper-grain",
];

#[test]
fn every_kind_is_deterministic_replays_and_changes_the_result() {
    let ws = Workspace::new("kinds");
    let plain = ws.named("plain");
    stroke(&plain, "M4 32 L60 32", "#3070c0", r#"{"size":12}"#, "5");
    for kind in KINDS {
        let media = format!(r#"{{"kind":"{kind}","strength":0.9}}"#);
        let a = ws.named(&format!("{kind}-a"));
        let b = ws.named(&format!("{kind}-b"));
        for doc in [&a, &b] {
            stroke(doc, "M4 32 L60 32", "#3070c0", &brush(&media), "5");
        }
        assert_eq!(tile_map(&a), tile_map(&b), "{kind} is not deterministic");
        let verify = ok(&["raster", &a, "verify", "--replay"]);
        assert_eq!(verify["ok"], true, "{kind}: {verify}");
        if kind != "glaze" {
            assert_ne!(tile_map(&a), tile_map(&plain), "{kind} left no mark");
        }
    }
}

#[test]
fn glaze_darkens_and_preserve_luminance_holds_value() {
    let ws = Workspace::new("glaze");
    let doc = ws.document();
    stroke(&doc, "M4 32 L60 32", "#c0c0c0", r#"{"size":20}"#, "1");
    let before = sample(&doc, 30, 32);
    stroke(
        &doc,
        "M4 32 L60 32",
        "#c03030",
        &brush(r#"{"kind":"glaze","strength":1}"#),
        "1",
    );
    let glazed = sample(&doc, 30, 32);
    assert!(glazed[1] < before[1] - 20, "{before:?} -> {glazed:?}");

    let held = ws.named("held");
    stroke(&held, "M4 32 L60 32", "#808080", r#"{"size":20}"#, "1");
    stroke(
        &held,
        "M4 32 L60 32",
        "#ff2020",
        &brush(r#"{"kind":"glaze","strength":1,"preserve_luminance":1}"#),
        "1",
    );
    let c = sample(&held, 30, 32);
    let luma = |c: [i32; 3]| (54 * c[0] + 183 * c[1] + 19 * c[2]) / 256;
    assert!((luma(c) - 128).abs() <= 6, "{c:?}");
}

#[test]
fn zones_restrict_paint_to_tonal_ranges() {
    let ws = Workspace::new("zones");
    let doc = ws.document();
    stroke(&doc, "M4 20 L60 20", "#101010", r#"{"size":10}"#, "1");
    stroke(&doc, "M4 44 L60 44", "#f0f0f0", r#"{"size":10}"#, "1");
    stroke(
        &doc,
        "M4 20 L60 44",
        "#ff0000",
        &brush(r#"{"kind":"glaze","strength":1,"zones":["highlight"],"pickup":0}"#),
        "1",
    );
    let dark = sample(&doc, 8, 20);
    assert!(dark[0] < 40, "shadows must stay untouched: {dark:?}");
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn wet_wash_dries_along_the_stroke_and_replays() {
    let ws = Workspace::new("wet");
    let doc = ws.document();
    let media = r#"{"kind":"watercolor-wash","wetness":1,"wet_length":40,"scale":3,"vary":true}"#;
    let result = stroke(&doc, "M2 32 L62 32", "#2050a0", &brush(media), "9");
    assert_eq!(result["result"]["strokes"].as_array().unwrap().len(), 1);
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn invalid_media_and_misapplied_media_leave_the_document_unchanged() {
    let ws = Workspace::new("invalid");
    let doc = ws.document();
    let before = fs::read(&doc).unwrap();
    for (media, blend, needle) in [
        (r#"{"kind":"oil"}"#, "normal", "unknown media kind"),
        (r#"{"kind":"glaze","light":10}"#, "normal", "does not apply"),
        (r#"{"kind":"pencil","strength":3}"#, "normal", "strength"),
        (r#"{"kind":"pencil"}"#, "erase", "media only applies"),
    ] {
        let err = fails(&[
            "raster",
            &doc,
            "stroke-path",
            "paint",
            "--path",
            "M4 4 L40 4",
            "--brush",
            &brush(media),
            "--blend",
            blend,
        ]);
        assert!(err.contains(needle), "{media}: {err}");
        assert_eq!(before, fs::read(&doc).unwrap());
    }
}

#[test]
fn media_catalogue_and_describe() {
    let listing = ok(&["paint-media", "list"]);
    assert_eq!(listing["kinds"].as_array().unwrap().len(), 10);
    let a = ok(&[
        "paint-media",
        "describe",
        r#"{"kind":"impasto","light":45}"#,
    ]);
    let b = ok(&[
        "paint-media",
        "describe",
        r#"{"light":45,"kind":"impasto"}"#,
    ]);
    assert_eq!(a["digest"], b["digest"]);
    assert_eq!(a["media"]["algorithm"], 1);
    let c = ok(&[
        "paint-media",
        "describe",
        r#"{"kind":"impasto","light":90}"#,
    ]);
    assert_ne!(a["digest"], c["digest"]);
    assert!(
        fails(&["paint-media", "describe", r#"{"kind":"glaze","edge":1}"#])
            .contains("does not apply")
    );
}

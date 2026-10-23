//! v0.13.0 task 3: color sampling, mixing and portable palettes.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-palette-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    /// A document with a 64x64 raster layer `paint` holding one red and one blue stroke.
    fn document(&self) -> String {
        let path = self.0.join("doc.pen");
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        let doc = path.to_string_lossy().into_owned();
        ok(&[
            "raster", &doc, "add", "paint", "--width", "64", "--height", "64",
        ]);
        for (path, color) in [("M8 16 L56 16", "#ff0000"), ("M8 48 L56 48", "#0000ff")] {
            ok(&[
                "raster",
                &doc,
                "stroke-path",
                "paint",
                "--path",
                path,
                "--brush",
                r#"{"size":12,"hardness":1,"spacing":0.1}"#,
                "--color",
                color,
            ]);
        }
        doc
    }
    fn bytes(doc: &str) -> Vec<u8> {
        fs::read(doc).unwrap()
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

#[test]
fn sampling_goes_through_the_renderer_and_is_deterministic() {
    let ws = Workspace::new("sample");
    let doc = ws.document();
    let red = ok(&[
        "paint-palette",
        &doc,
        "sample",
        "--scope",
        "node:paint",
        "--x",
        "32",
        "--y",
        "16",
    ]);
    assert!(
        red["color"].as_str().unwrap().starts_with("#ff0000"),
        "{red}"
    );
    assert_eq!(red["algorithm"], 1);
    assert_eq!(red["color_space"], "srgb8");
    let again = ok(&[
        "paint-palette",
        &doc,
        "sample",
        "--scope",
        "node:paint",
        "--x",
        "32",
        "--y",
        "16",
    ]);
    assert_eq!(red, again);
    let empty = ok(&[
        "paint-palette",
        &doc,
        "sample",
        "--scope",
        "node:paint",
        "--x",
        "32",
        "--y",
        "32",
    ]);
    assert_eq!(empty["empty"], true);
    let err = fails(&[
        "paint-palette",
        &doc,
        "sample",
        "--scope",
        "node:paint",
        "--x",
        "99999",
        "--y",
        "0",
    ]);
    assert!(err.contains("outside"), "{err}");
}

#[test]
fn extraction_orders_roles_and_hashes_sources() {
    let ws = Workspace::new("extract");
    let doc = ws.document();
    let out = ok(&[
        "paint-palette",
        &doc,
        "extract",
        "--scope",
        "node:paint",
        "--count",
        "2",
    ]);
    let colors: Vec<&str> = out["colors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["color"].as_str().unwrap())
        .collect();
    assert_eq!(colors.len(), 2);
    assert!(
        colors.iter().any(|c| c.starts_with("#ff0000"))
            && colors.iter().any(|c| c.starts_with("#0000ff")),
        "{colors:?}"
    );
    // A color's share is of the scope, so it does not change with --count.
    let one = ok(&[
        "paint-palette",
        &doc,
        "extract",
        "--scope",
        "node:paint",
        "--count",
        "1",
    ]);
    assert!(one["colors"][0]["share"].as_f64().unwrap() < 1.0, "{one}");
    assert!(out["source_hash"].as_str().unwrap().starts_with("sha256:"));
    assert!(out["palette_hash"].as_str().unwrap().starts_with("sha256:"));
    // Blue is darker than red in the protocol luma, so it is the shadow color.
    let shadow = ok(&[
        "paint-palette",
        &doc,
        "extract",
        "--scope",
        "node:paint",
        "--role",
        "shadow",
        "--count",
        "1",
    ]);
    assert!(shadow["colors"][0]["color"]
        .as_str()
        .unwrap()
        .starts_with("#0000ff"));
    let median = ok(&[
        "paint-palette",
        &doc,
        "extract",
        "--scope",
        "node:paint",
        "--role",
        "median",
        "--count",
        "2",
    ]);
    assert_eq!(median["colors"].as_array().unwrap().len(), 2);
    let edge = ok(&[
        "paint-palette",
        &doc,
        "extract",
        "--scope",
        "node:paint",
        "--role",
        "edge",
        "--count",
        "3",
    ]);
    assert_eq!(edge["role"], "edge");
    let cropped = ok(&[
        "paint-palette",
        &doc,
        "extract",
        "--scope",
        "node:paint",
        "--rect",
        "0,0,64,32",
        "--count",
        "3",
    ]);
    assert_eq!(cropped["colors"].as_array().unwrap().len(), 1);
    assert!(cropped["colors"][0]["color"]
        .as_str()
        .unwrap()
        .starts_with("#ff0000"));
    assert_ne!(cropped["source_hash"], out["source_hash"]);
}

#[test]
fn mixing_and_variation_are_exact_and_seeded() {
    let ws = Workspace::new("mix");
    let doc = ws.document();
    let m = ok(&[
        "paint-palette",
        &doc,
        "mix",
        "#000000",
        "#ffffff",
        "--amount",
        "0.5",
    ]);
    assert_eq!(m["color"], "#808080ff");
    let m = ok(&[
        "paint-palette",
        &doc,
        "mix",
        "#ff0000",
        "#0000ff",
        "--amount",
        "0",
    ]);
    assert_eq!(m["color"], "#ff0000ff");
    assert!(fails(&[
        "paint-palette",
        &doc,
        "mix",
        "#ff0000",
        "#0000ff",
        "--amount",
        "1.5"
    ])
    .contains("between 0 and 1"));
    let a = ok(&[
        "paint-palette",
        &doc,
        "vary",
        "#808080",
        "--seed",
        "7",
        "--amount",
        "10",
        "--count",
        "5",
    ]);
    let b = ok(&[
        "paint-palette",
        &doc,
        "vary",
        "#808080",
        "--seed",
        "7",
        "--amount",
        "10",
        "--count",
        "5",
    ]);
    let c = ok(&[
        "paint-palette",
        &doc,
        "vary",
        "#808080",
        "--seed",
        "8",
        "--amount",
        "10",
        "--count",
        "5",
    ]);
    assert_eq!(a, b);
    assert_ne!(a["colors"], c["colors"]);
    for hex in a["colors"].as_array().unwrap() {
        let v = u8::from_str_radix(&hex.as_str().unwrap()[1..3], 16).unwrap();
        assert!((0x80 - 10..=0x80 + 10).contains(&v));
    }
    assert!(fails(&[
        "paint-palette",
        &doc,
        "vary",
        "#808080",
        "--seed",
        "1",
        "--count",
        "0"
    ])
    .contains("count"));
}

#[test]
fn named_palettes_are_stored_validated_and_atomic() {
    let ws = Workspace::new("store");
    let doc = ws.document();
    let saved = ok(&[
        "paint-palette",
        &doc,
        "extract",
        "--scope",
        "node:paint",
        "--count",
        "2",
        "--save",
        "warm",
    ]);
    assert_eq!(saved["result"]["stored"]["saved"], "warm");
    let listed = ok(&["paint-palette", &doc, "list"]);
    assert_eq!(listed["matches"], 1);
    let shown = ok(&["paint-palette", &doc, "show", "warm"]);
    assert_eq!(shown["algorithm"], 1);
    assert_eq!(shown["scope"], "node:paint");
    assert_eq!(shown["palette_hash"], saved["result"]["palette_hash"]);
    // A second save under the same name needs --replace and leaves bytes untouched.
    let before = Workspace::bytes(&doc);
    let err = fails(&["paint-palette", &doc, "add", "warm", "#112233"]);
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(before, Workspace::bytes(&doc));
    ok(&["paint-palette", &doc, "add", "warm", "#112233", "--replace"]);
    // Dry run does not write.
    let before = Workspace::bytes(&doc);
    ok(&["paint-palette", &doc, "--dry-run", "remove", "warm"]);
    assert_eq!(before, Workspace::bytes(&doc));
    ok(&["paint-palette", &doc, "remove", "warm"]);
    assert_eq!(ok(&["paint-palette", &doc, "list"])["matches"], 0);
    assert!(fails(&["paint-palette", &doc, "show", "warm"]).contains("not-found"));
    assert!(fails(&["paint-palette", &doc, "add", "bad/name", "#000000"]).contains("palette name"));
    assert!(fails(&["paint-palette", &doc, "add", "x", "red"]).contains("color"));
}

#[test]
fn tampered_palette_hashes_are_rejected() {
    let ws = Workspace::new("tamper");
    let doc = ws.document();
    ok(&["paint-palette", &doc, "add", "p", "#112233", "#445566"]);
    let mut raw: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    raw["paint_palettes"]["p"]["colors"][0] = Value::String("#ffffff".into());
    fs::write(&doc, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    assert!(fails(&["paint-palette", &doc, "list"]).contains("palette_hash"));
}

#[test]
fn read_only_actions_reject_write_flags() {
    let ws = Workspace::new("ro");
    let doc = ws.document();
    let err = fails(&[
        "paint-palette",
        &doc,
        "--dry-run",
        "mix",
        "#000000",
        "#ffffff",
    ]);
    assert!(err.contains("read-only"), "{err}");
}

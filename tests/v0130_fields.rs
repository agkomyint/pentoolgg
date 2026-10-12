//! v0.13.0 task 6: deterministic stroke fields.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-fields-{tag}-{}-{}",
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
        let doc = self.0.join("doc.pen").to_string_lossy().into_owned();
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

const BRUSH: &str = r#"{"size":4,"hardness":1,"spacing":0.2}"#;

fn paint_field(doc: &str, field: &str, params: &str, extra: &[&str]) -> Value {
    let mut args = vec![
        "raster",
        doc,
        "stroke-field",
        "paint",
        "--field",
        field,
        "--params",
        params,
        "--brush",
        BRUSH,
        "--color",
        "#ff0000",
    ];
    args.extend_from_slice(extra);
    ok(&args)
}

fn tile_map(doc: &str) -> Value {
    ok(&["raster", doc, "info", "paint"])["tile_map_sha256"].clone()
}

const FIELD: &str = r#"{"method":"angle","degrees":30,"cell":8}"#;
const PARAMS: &str = r#"{"spacing":10,"length":24,"step":2,"jitter":0.6}"#;

#[test]
fn painting_is_deterministic_and_seed_sensitive() {
    let ws = Workspace::new("determinism");
    let a = ws.document();
    let b = ws.0.join("b.pen").to_string_lossy().into_owned();
    fs::copy(&a, &b).unwrap();
    let c = ws.0.join("c.pen").to_string_lossy().into_owned();
    fs::copy(&a, &c).unwrap();
    let ra = paint_field(&a, FIELD, PARAMS, &["--seed", "7"]);
    let rb = paint_field(&b, FIELD, PARAMS, &["--seed", "7"]);
    paint_field(&c, FIELD, PARAMS, &["--seed", "8"]);
    assert_eq!(
        ra["result"]["field"]["field_hash"],
        rb["result"]["field"]["field_hash"]
    );
    assert_eq!(tile_map(&a), tile_map(&b));
    assert_ne!(tile_map(&a), tile_map(&c));
    assert!(ra["result"]["strokes"].as_u64().unwrap() > 3, "{ra}");
    assert!(ra["result"]["dabs"].as_u64().unwrap() > 0);
}

#[test]
fn committed_strokes_pin_the_field_hash_and_replay() {
    let ws = Workspace::new("pin");
    let doc = ws.document();
    let result = paint_field(&doc, FIELD, PARAMS, &[]);
    let hash = result["result"]["field"]["field_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(hash.starts_with("sha256:"));
    let document: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let text = document.to_string();
    assert!(text.contains(&hash), "journal must pin the field hash");
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert!(!verify.to_string().contains("\"ok\":false"), "{verify}");
}

#[test]
fn contour_follows_a_bar_and_preview_matches_the_plan() {
    let ws = Workspace::new("contour");
    let doc = ws.document();
    ok(&[
        "raster",
        &doc,
        "stroke-path",
        "paint",
        "--path",
        "M8 32 L56 32",
        "--brush",
        r#"{"size":12,"hardness":1}"#,
        "--color",
        "#ffffff",
    ]);
    let field = r#"{"method":"contour","cell":2}"#;
    let preview = ok(&[
        "paint-fields",
        &doc,
        "preview",
        "paint",
        "--field",
        field,
        "--params",
        r#"{"spacing":6,"length":16,"step":1}"#,
        "--constraints",
        r#"{"opaque_clip":true}"#,
        "--brush",
        BRUSH,
    ]);
    assert!(preview["field"]["defined_cells"].as_u64().unwrap() > 0);
    let planned = preview["plan"]["strokes"].as_u64().unwrap();
    assert!(planned > 0, "{preview}");
    let painted = paint_field(
        &doc,
        field,
        r#"{"spacing":6,"length":16,"step":1}"#,
        &["--constraints", r#"{"opaque_clip":true}"#],
    );
    assert_eq!(painted["result"]["strokes"].as_u64().unwrap(), planned);
    assert_eq!(
        painted["result"]["field"]["field_hash"],
        preview["plan"]["field"]["field_hash"]
    );
}

#[test]
fn collision_rules_and_limits() {
    let ws = Workspace::new("collision");
    let doc = ws.document();
    let plan = |params: &str| {
        let v = ok(&[
            "paint-fields",
            &doc,
            "preview",
            "paint",
            "--field",
            FIELD,
            "--params",
            params,
            "--brush",
            r#"{"size":8}"#,
        ]);
        v["plan"]["strokes"].as_u64().unwrap()
    };
    let stop = plan(r#"{"spacing":3,"length":30,"overlap":0,"collision":"stop"}"#);
    let allow = plan(r#"{"spacing":3,"length":30,"overlap":0,"collision":"allow"}"#);
    assert!(stop < allow, "stop {stop} allow {allow}");
    // Too many strokes is a limit error and leaves the document untouched.
    let before = fs::read(&doc).unwrap();
    let err = fails(&[
        "raster",
        &doc,
        "stroke-field",
        "paint",
        "--field",
        FIELD,
        "--params",
        r#"{"spacing":1,"collision":"allow"}"#,
    ]);
    assert!(err.contains("limit-exceeded"), "{err}");
    assert_eq!(before, fs::read(&doc).unwrap());
    // Bad params are rejected up front.
    let err = fails(&[
        "raster",
        &doc,
        "stroke-field",
        "paint",
        "--field",
        FIELD,
        "--params",
        r#"{"bogus":1}"#,
    ]);
    assert!(err.contains("unknown params key"), "{err}");
    assert_eq!(before, fs::read(&doc).unwrap());
}

#[test]
fn guide_and_vector_methods_build_fields() {
    let ws = Workspace::new("methods");
    let doc = ws.document();
    for field in [
        r#"{"method":"vanishing","point":[32,-40],"cell":8}"#,
        r#"{"method":"radial","center":[32,32],"cell":8}"#,
        r#"{"method":"vectors","points":[[8,8,0],[56,56,90]],"cell":8}"#,
    ] {
        let v = ok(&["paint-fields", &doc, "preview", "paint", "--field", field]);
        assert_eq!(v["field"]["defined_cells"], 64, "{field}: {v}");
    }
    ok(&[
        "raster",
        &doc,
        "stroke-path",
        "paint",
        "--path",
        "M10 32 L54 32",
        "--brush",
        r#"{"size":14,"hardness":1}"#,
        "--color",
        "#ffffff",
    ]);
    for method in ["distance", "tensor", "gradient"] {
        let field = format!(r#"{{"method":"{method}","cell":4}}"#);
        let a = ok(&["paint-fields", &doc, "preview", "paint", "--field", &field]);
        let b = ok(&["paint-fields", &doc, "preview", "paint", "--field", &field]);
        assert_eq!(a["field"]["field_hash"], b["field"]["field_hash"]);
        assert!(
            a["field"]["defined_cells"].as_u64().unwrap() > 0,
            "{method}"
        );
    }
}

#[test]
fn taper_and_curvature_replay_exactly() {
    let ws = Workspace::new("taper");
    let doc = ws.document();
    paint_field(
        &doc,
        r#"{"method":"radial","center":[32,32],"cell":4}"#,
        r#"{"spacing":12,"length":40,"taper":0.3,"curvature":10,"step":1.5}"#,
        &["--seed", "3"],
    );
    let verify = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

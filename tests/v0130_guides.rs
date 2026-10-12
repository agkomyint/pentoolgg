//! v0.13.0 task 10: reusable composition and form guides.
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-guides-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
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

fn document_without_guides(path: &str) -> Value {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("paint_guides");
    value
}

#[test]
fn every_kind_derives_geometry_and_editing_never_touches_pixels() {
    let ws = Workspace::new("kinds");
    let doc = ws.document();
    ok(&[
        "raster",
        &doc,
        "stroke-path",
        "paint",
        "--path",
        "M4 4 L50 50",
        "--color",
        "#336699",
    ]);
    let pixels_before = document_without_guides(&doc);
    let rect = "[0,0,64,64]";
    let cases: Vec<(&str, String)> = vec![
        ("thirds", format!(r#"{{"rect":{rect}}}"#)),
        ("golden", format!(r#"{{"rect":{rect}}}"#)),
        ("symmetry", format!(r#"{{"rect":{rect},"at":32}}"#)),
        ("horizon", format!(r#"{{"rect":{rect},"y":20}}"#)),
        (
            "vanishing-point",
            format!(r#"{{"rect":{rect},"point":[32,16],"rays":6}}"#),
        ),
        (
            "perspective",
            format!(r#"{{"rect":{rect},"horizon_y":20,"vanishing":[[0,20],[64,20]]}}"#),
        ),
        ("gesture", r#"{"points":[[4,60],[30,30],[58,6]]}"#.into()),
        (
            "proportion",
            format!(r#"{{"rect":{rect},"preset":"figure-8"}}"#),
        ),
        (
            "value-map",
            format!(r#"{{"rect":{rect},"cols":2,"rows":1,"values":[10,240]}}"#),
        ),
        (
            "custom",
            r#"{"lines":[[0,0,64,64]],"points":[[32,32]]}"#.into(),
        ),
    ];
    for (kind, params) in &cases {
        let id = format!("g-{kind}");
        let r = ok(&[
            "paint-guide",
            &doc,
            "add",
            &id,
            "--kind",
            kind,
            "--params",
            params,
        ]);
        assert_eq!(r["result"]["pixels_changed"], false, "{kind}");
        let shown = ok(&["paint-guide", &doc, "show", &id]);
        assert!(
            !shown["geometry"]["lines"].as_array().unwrap().is_empty()
                || !shown["geometry"]["points"].as_array().unwrap().is_empty(),
            "{kind}"
        );
        assert!(shown["guide_hash"].as_str().unwrap().starts_with("sha256:"));
    }
    // Editing and hiding change guide data only.
    ok(&["paint-guide", &doc, "set", "g-thirds", "--visible", "false"]);
    ok(&[
        "paint-guide",
        &doc,
        "set",
        "g-custom",
        "--params",
        r#"{"lines":[[0,64,64,0]]}"#,
    ]);
    ok(&["paint-guide", &doc, "remove", "g-gesture"]);
    assert_eq!(pixels_before, document_without_guides(&doc));
    let list = ok(&["paint-guide", &doc, "list", "--limit", "3"]);
    assert_eq!(list["returned"], 3);
    assert_eq!(list["matches"], 9);
    assert_eq!(list["has_more"], true);
}

#[test]
fn snap_reports_nearest_line_point_or_nothing() {
    let ws = Workspace::new("snap");
    let doc = ws.document();
    ok(&[
        "paint-guide",
        &doc,
        "add",
        "t",
        "--kind",
        "thirds",
        "--params",
        r#"{"rect":[0,0,60,60]}"#,
    ]);
    let near = ok(&[
        "paint-guide",
        &doc,
        "snap",
        "t",
        "--point",
        "21,35",
        "--radius",
        "5",
    ]);
    assert_eq!(near["snapped"], true);
    assert_eq!(near["to"], serde_json::json!([20.0, 35.0]));
    let power = ok(&[
        "paint-guide",
        &doc,
        "snap",
        "t",
        "--point",
        "19,21",
        "--radius",
        "5",
    ]);
    assert_eq!(power["on"], "point");
    assert_eq!(power["to"], serde_json::json!([20.0, 20.0]));
    let far = ok(&[
        "paint-guide",
        &doc,
        "snap",
        "t",
        "--point",
        "30,30",
        "--radius",
        "2",
    ]);
    assert_eq!(far["snapped"], false);
}

#[test]
fn guides_drive_fields_by_id() {
    let ws = Workspace::new("field");
    let doc = ws.document();
    ok(&[
        "paint-guide",
        &doc,
        "add",
        "vp",
        "--kind",
        "vanishing-point",
        "--params",
        r#"{"rect":[0,0,64,64],"point":[32,0]}"#,
    ]);
    let preview = ok(&[
        "paint-fields",
        &doc,
        "preview",
        "paint",
        "--field",
        r#"{"method":"guide","guide":"vp","cell":16}"#,
    ]);
    assert!(
        preview["field"]["field_hash"].as_str().is_some(),
        "{preview}"
    );
    let r = ok(&[
        "raster",
        &doc,
        "stroke-field",
        "paint",
        "--field",
        r#"{"method":"guide","guide":"vp","cell":16}"#,
        "--params",
        r#"{"spacing":12,"length":20}"#,
        "--brush",
        r#"{"size":3,"hardness":1}"#,
        "--color",
        "#202020",
    ]);
    assert!(r["result"]["strokes"].as_u64().unwrap() >= 1, "{r}");
    let err = fails(&[
        "paint-fields",
        &doc,
        "preview",
        "paint",
        "--field",
        r#"{"method":"guide","guide":"missing"}"#,
    ]);
    assert!(err.contains("not-found"), "{err}");
}

#[test]
fn proof_export_is_explicit_and_bad_input_fails() {
    let ws = Workspace::new("proof");
    let doc = ws.document();
    ok(&[
        "paint-guide",
        &doc,
        "add",
        "a",
        "--kind",
        "horizon",
        "--params",
        r#"{"rect":[0,0,64,64],"y":10}"#,
        "--proof",
    ]);
    ok(&[
        "paint-guide",
        &doc,
        "add",
        "b",
        "--kind",
        "horizon",
        "--params",
        r#"{"rect":[0,0,64,64],"y":50}"#,
    ]);
    let out = ws.0.join("proof.svg").to_string_lossy().into_owned();
    let r = ok(&[
        "paint-guide",
        &doc,
        "proof",
        "--out",
        &out,
        "--width",
        "64",
        "--height",
        "64",
    ]);
    assert_eq!(r["guides"], serde_json::json!(["a"]));
    let svg = fs::read_to_string(&out).unwrap();
    assert!(svg.contains("guide-a") && !svg.contains("guide-b"));
    let before = fs::read(&doc).unwrap();
    for (args, needle) in [
        (vec!["add", "x", "--kind", "bokeh"], "unknown guide kind"),
        (
            vec![
                "add",
                "a",
                "--kind",
                "horizon",
                "--params",
                r#"{"rect":[0,0,64,64],"y":1}"#,
            ],
            "already exists",
        ),
        (
            vec![
                "add",
                "x",
                "--kind",
                "thirds",
                "--params",
                r#"{"rect":[0,0,0,5]}"#,
            ],
            "positive",
        ),
        (
            vec![
                "add",
                "x",
                "--kind",
                "thirds",
                "--params",
                r#"{"rect":[0,0,5,5],"zzz":1}"#,
            ],
            "no parameter",
        ),
        (
            vec![
                "add",
                "bad id",
                "--kind",
                "horizon",
                "--params",
                r#"{"rect":[0,0,5,5],"y":1}"#,
            ],
            "guide id",
        ),
        (vec!["remove", "nope"], "not-found"),
    ] {
        let mut full = vec!["paint-guide", doc.as_str()];
        full.extend(args.iter().copied());
        let err = fails(&full);
        assert!(err.contains(needle), "{args:?}: {err}");
    }
    assert_eq!(before, fs::read(&doc).unwrap());
}

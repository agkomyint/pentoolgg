//! batch operation file validation at the CLI level.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-batch-{label}-{}-{}",
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

#[test]
fn batch_rejects_duplicate_json_keys_without_mutation() {
    let ws = Workspace::new("duplicate-keys");
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let ops = ws.path("ops.json");
    fs::write(
        &ops,
        r#"[{"type":"put-shape","shape":"rect","id":"x1","layer":"content","x":1,"y":1,"width":2,"height":2,"x":5}]"#,
    )
    .unwrap();
    let before = fs::read(&doc).unwrap();
    let out = run(&["batch", &doc, &ops]);
    assert!(!out.status.success(), "duplicate keys should fail");
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        message.contains("duplicate JSON object key"),
        "unexpected error: {message}"
    );
    assert_eq!(fs::read(&doc).unwrap(), before);
}

fn batch_error(ws: &Workspace, ops: &str) -> String {
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    let ops_path = ws.path("ops.json");
    fs::write(&ops_path, ops).unwrap();
    let before = fs::read(&doc).unwrap();
    let out = run(&["batch", &doc, &ops_path]);
    assert!(!out.status.success(), "should fail: {ops}");
    assert_eq!(fs::read(&doc).unwrap(), before, "document changed");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

#[test]
fn put_shape_rejects_a_non_string_fill_instead_of_dropping_it() {
    let ws = Workspace::new("shape-fill");
    let message = batch_error(
        &ws,
        r#"[{"type":"put-shape","shape":"rect","id":"x1","layer":"content","x":1,"y":1,"width":2,"height":2,"fill":{"kind":"radial","stops":[]}}]"#,
    );
    assert!(message.contains("fill must be a color string"), "{message}");
    let message = batch_error(
        &ws,
        r#"[{"type":"put-shape","shape":"line","id":"x2","layer":"content","x1":0,"y1":0,"x2":5,"y2":5,"stroke":7}]"#,
    );
    assert!(
        message.contains("stroke must be a color string"),
        "{message}"
    );
}

#[test]
fn put_path_rejects_a_paint_object_without_fallback_or_ref() {
    let ws = Workspace::new("path-paint");
    let message = batch_error(
        &ws,
        r##"[{"type":"put-path","id":"p1","layer":"content","d":"M 0 0 L 5 5","fill":{"kind":"radial","stops":[]}}]"##,
    );
    assert!(
        message.contains("fill needs a \"fallback\" or \"ref\""),
        "{message}"
    );
}

#[test]
fn hidden_nodes_are_left_out_of_exports_and_groups_hide_their_children() {
    let ws = Workspace::new("hidden-node");
    let doc = ws.path("hidden.pen");
    assert!(run(&["new", &doc]).status.success());
    let ops = ws.path("ops.json");
    fs::write(
        &ops,
        r##"[{"type":"put-shape","shape":"rect","id":"r","layer":"layer-1","x":0,"y":0,"width":100,"height":100,"fill":"#ff0000"},
             {"type":"put-shape","shape":"circle","id":"c","layer":"layer-1","cx":300,"cy":300,"radius":40,"fill":"#0000ff"},
             {"type":"create-group","id":"g","layer":"layer-1","children":["c"]}]"##,
    )
    .unwrap();
    assert!(run(&["batch", &doc, &ops]).status.success());
    let empty = ws.path("empty.pen");
    assert!(run(&["new", &empty]).status.success());
    let shown = ws.path("shown.png");
    assert!(run(&["export", &doc, &shown]).status.success());
    let blank = ws.path("blank.png");
    assert!(run(&["export", &empty, &blank]).status.success());
    assert_ne!(fs::read(&shown).unwrap(), fs::read(&blank).unwrap());

    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    for node in value["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
    {
        node["visible"] = serde_json::json!(false);
    }
    fs::write(&doc, serde_json::to_vec(&value).unwrap()).unwrap();
    let hidden = ws.path("hidden.png");
    let out = run(&["export", &doc, &hidden]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read(&hidden).unwrap(),
        fs::read(&blank).unwrap(),
        "a node with visible:false was still rendered"
    );
}

#[test]
fn help_states_the_step_repeat_count_range_and_select_load_modes() {
    let help = |args: &[&str]| {
        let out = run(args);
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    let repeat = help(&["layout-grid", "doc.pen", "step-repeat", "--help"]);
    assert!(repeat.contains("from 1 to 1000"), "{repeat}");
    let load = help(&["raster", "doc.pen", "select-load", "--help"]);
    assert!(
        load.contains("replace, add, subtract or intersect"),
        "{load}"
    );
}

#[test]
fn layer_geometry_accepts_negative_positional_numbers() {
    let ws = Workspace::new("neg-geometry");
    let doc = ws.path("a.pen");
    assert!(run(&["new", &doc]).status.success());
    let ops = ws.path("ops.json");
    fs::write(
        &ops,
        r##"[{"type":"put-shape","id":"r","layer":"layer-1","shape":"rect","x":10,"y":10,"width":5,"height":5,"fill":"#ff0000"}]"##,
    )
    .unwrap();
    assert!(run(&["batch", &doc, &ops]).status.success());
    for args in [
        vec!["translate", "3", "-2"],
        vec!["rotate", "-15"],
        vec!["scale", "-1", "1"],
    ] {
        let mut cmd = vec!["layer-geometry", doc.as_str(), "--layer", "layer-1"];
        cmd.extend(args.iter().copied());
        let out = run(&cmd);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn shape_and_benchmark_help_state_their_requirements() {
    let shape = run(&["shape", "--help"]);
    let shape = String::from_utf8_lossy(&shape.stdout).into_owned();
    for needle in [
        "rect: --x --y --width --height",
        "ellipse: --cx --cy --radius-x --radius-y",
        "circle: --cx --cy --radius",
        "line: --x1 --y1 --x2 --y2",
    ] {
        assert!(
            shape.contains(needle),
            "shape --help lacks {needle:?}:
{shape}"
        );
    }
    let bench = run(&["benchmark", "--help"]);
    let bench = String::from_utf8_lossy(&bench.stdout).into_owned();
    assert!(bench.contains("1 to 1000"), "{bench}");
    assert!(bench.contains("0 to 100000"), "{bench}");
}

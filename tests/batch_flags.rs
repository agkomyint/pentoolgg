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

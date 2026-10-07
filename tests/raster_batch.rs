//! v0.10.0 item 13: raster batch parity with the single-operation commands.
use serde_json::Value;
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
    /// A 100x60 layer `p`: opaque white with a red block at (20..40, 20..40).
    fn document(&self, name: &str) -> String {
        let path = self.0.join(name).to_string_lossy().into_owned();
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        ok(&[
            "raster", &path, "add", "p", "--width", "100", "--height", "60",
        ]);
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

fn layer_hash(doc: &str) -> String {
    ok(&["raster", doc, "info", "p"])["tile_map_sha256"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn write(ws: &Workspace, name: &str, value: &Value) -> String {
    let path = ws.0.join(name).to_string_lossy().into_owned();
    fs::write(&path, serde_json::to_string(value).unwrap()).unwrap();
    path
}

const SAMPLES: &str = r#"[{"x":5,"y":5},{"x":60,"y":30},{"x":90,"y":10}]"#;

#[test]
fn batch_matches_single_commands_and_reports_hashes_without_pixels() {
    let ws = Workspace::new("match");
    let single = ws.document("single.pen");
    let batched = ws.document("batched.pen");
    ok(&[
        "raster",
        &single,
        "stroke",
        "p",
        "--samples",
        SAMPLES,
        "--brush",
        r#"{"size":8}"#,
        "--color",
        "#ff0000",
    ]);
    ok(&[
        "raster",
        &single,
        "select-marquee",
        "p",
        "--rect",
        "10",
        "10",
        "30",
        "20",
    ]);
    let ops = serde_json::json!([
        {"action":"stroke","id":"p","args":{"samples":serde_json::from_str::<Value>(SAMPLES).unwrap(),"brush":{"size":8},"color":"#ff0000"}},
        {"action":"select-marquee","id":"p","args":{"x":10,"y":10,"width":30,"height":20}}
    ]);
    let file = write(&ws, "ops.json", &ops);
    let out = ok(&["raster", &batched, "batch", &file]);
    assert_eq!(out["result"]["operations"], 2, "{out}");
    let first = &out["result"]["results"][0];
    assert!(first["result"]["bounds"].is_array(), "{first}");
    assert!(first["result"]["tiles_changed"].as_u64().unwrap() >= 1);
    assert_eq!(
        first["tile_map_sha256"].as_str().unwrap(),
        layer_hash(&single)
    );
    assert_eq!(layer_hash(&single), layer_hash(&batched));
    assert!(!out.to_string().contains("\"data\""), "no pixel dumps");
}

#[test]
fn failed_batch_changes_nothing_and_names_the_operation() {
    let ws = Workspace::new("atomic");
    let doc = ws.document("a.pen");
    let before = fs::read(&doc).unwrap();
    let ops = serde_json::json!([
        {"action":"stroke","id":"p","args":{"samples":serde_json::from_str::<Value>(SAMPLES).unwrap()}},
        {"action":"stroke","id":"missing","args":{"samples":serde_json::from_str::<Value>(SAMPLES).unwrap()}}
    ]);
    let file = write(&ws, "bad.json", &ops);
    let message = fail(&["raster", &doc, "batch", &file]);
    assert!(message.contains("operation 1"), "{message}");
    assert_eq!(before, fs::read(&doc).unwrap());
}

#[test]
fn batch_enforces_limits_before_work() {
    let ws = Workspace::new("limits");
    let doc = ws.document("l.pen");
    let before = fs::read(&doc).unwrap();
    let many: Vec<Value> = (0..257)
        .map(|_| serde_json::json!({"action":"select-info","id":"p"}))
        .collect();
    let file = write(&ws, "many.json", &Value::Array(many));
    assert!(fail(&["raster", &doc, "batch", &file]).contains("limit-exceeded"));
    let empty = write(&ws, "empty.json", &serde_json::json!([]));
    assert!(fail(&["raster", &doc, "batch", &empty]).contains("limit-exceeded"));
    let unknown = write(
        &ws,
        "unknown.json",
        &serde_json::json!([{"action":"select-info","id":"p","bogus":1}]),
    );
    assert!(fail(&["raster", &doc, "batch", &unknown]).contains("bogus"));
    let action = write(
        &ws,
        "action.json",
        &serde_json::json!([{"action":"explode","id":"p"}]),
    );
    assert!(fail(&["raster", &doc, "batch", &action]).contains("unknown raster action"));
    assert_eq!(before, fs::read(&doc).unwrap());
}

#[test]
fn dry_run_batch_reports_but_does_not_write() {
    let ws = Workspace::new("dry");
    let doc = ws.document("d.pen");
    let before = fs::read(&doc).unwrap();
    let file = write(
        &ws,
        "ops.json",
        &serde_json::json!([{"action":"fill","id":"p","args":{"x":1,"y":1,"color":"#00ff00"}}]),
    );
    let out = ok(&["raster", &doc, "--dry-run", "batch", &file]);
    assert_eq!(out["result"]["operations"], 1, "{out}");
    assert_eq!(before, fs::read(&doc).unwrap());
}

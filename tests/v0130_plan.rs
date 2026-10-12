//! v0.13.0 task 9: declarative multi-pass paint plans.
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-plan-{tag}-{}-{}",
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
    fn plan(&self, name: &str, value: &Value) -> String {
        let path = self.0.join(name).to_string_lossy().into_owned();
        fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        path
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

fn plan() -> Value {
    json!({
        "schema": 1, "engine": 1, "id": "sky-and-bar",
        "passes": [
            {"id": "wash", "target": "paint", "action": "stroke-path",
             "args": {"path": "M4 10 L60 10",
                      "brush": {"size": 14, "hardness": 0.6, "spacing": 0.1},
                      "color": "#3070c0", "seed": 3}},
            {"id": "bar", "target": "paint", "action": "stroke-path",
             "args": {"path": "M8 40 L56 40",
                      "brush": {"size": 6, "hardness": 1, "spacing": 0.1},
                      "color": "#101010", "seed": 4}}
        ]
    })
}

#[test]
fn dry_run_and_commit_report_the_same_plan() {
    let ws = Workspace::new("parity");
    let doc = ws.document();
    let file = ws.plan("plan.json", &plan());
    let before = fs::read(&doc).unwrap();
    let dry = ok(&["paint-plan", &doc, &file, "--dry-run"]);
    assert_eq!(before, fs::read(&doc).unwrap());
    let done = ok(&["paint-plan", &doc, &file]);
    assert_ne!(before, fs::read(&doc).unwrap());
    assert_eq!(dry["result"], done["result"]);
    assert_eq!(done["dry_run"], false);
    let r = &done["result"];
    assert!(r["plan_hash"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(r["totals"]["passes"], 2);
    let passes = r["passes"].as_array().unwrap();
    assert!(passes[0]["changed_pixels"].as_u64().unwrap() > 0);
    let b = passes[1]["changed_bounds"].as_array().unwrap();
    assert!(
        b[1].as_u64().unwrap() >= 36 && b[3].as_u64().unwrap() <= 44,
        "{b:?}"
    );
    assert!(r["totals"]["dabs"].as_u64().unwrap() > 0);
}

#[test]
fn plans_are_deterministic_and_undo_is_one_step() {
    let ws = Workspace::new("undo");
    let a = ws.document();
    let original = fs::read(&a).unwrap();
    let file = ws.plan("plan.json", &plan());
    let first = ok(&["paint-plan", &a, &file]);
    // Identical plan on an identical document yields identical pixels.
    let b = ws.0.join("b.pen").to_string_lossy().into_owned();
    fs::write(&b, &original).unwrap();
    let second = ok(&["paint-plan", &b, &file]);
    assert_eq!(first["result"], second["result"]);
    // One undo restores the pre-plan document for both passes.
    ok(&["undo", &a]);
    assert_eq!(original, fs::read(&a).unwrap());
}

#[test]
fn stale_revision_and_plan_hash_leave_the_document_unchanged() {
    let ws = Workspace::new("guards");
    let doc = ws.document();
    let file = ws.plan("plan.json", &plan());
    let before = fs::read(&doc).unwrap();
    let err = fails(&["paint-plan", &doc, &file, "--if-revision", "deadbeef"]);
    assert!(err.to_lowercase().contains("revision"), "{err}");
    let err = fails(&[
        "paint-plan",
        &doc,
        &file,
        "--expect-plan-hash",
        &format!("sha256:{}", "0".repeat(64)),
    ]);
    assert!(err.contains("plan-changed"), "{err}");
    assert_eq!(before, fs::read(&doc).unwrap());
    let dry = ok(&["paint-plan", &doc, &file, "--dry-run"]);
    let hash = dry["result"]["plan_hash"].as_str().unwrap();
    ok(&["paint-plan", &doc, &file, "--expect-plan-hash", hash]);
}

#[test]
fn a_failing_pass_is_atomic_and_named() {
    let ws = Workspace::new("atomic");
    let doc = ws.document();
    let mut bad = plan();
    bad["passes"][1]["target"] = json!("no-such-layer");
    let file = ws.plan("bad.json", &bad);
    let before = fs::read(&doc).unwrap();
    let err = fails(&["paint-plan", &doc, &file]);
    assert!(
        err.contains("bar") && err.contains("nothing was written"),
        "{err}"
    );
    assert_eq!(before, fs::read(&doc).unwrap());
}

#[test]
fn limits_and_malformed_plans_are_rejected() {
    let ws = Workspace::new("limits");
    let doc = ws.document();
    let before = fs::read(&doc).unwrap();
    let mut tight = plan();
    tight["limits"] = json!({"max_changed_pixels": 10});
    let mut unknown = plan();
    unknown["passes"][0]["extra"] = json!(1);
    let mut action = plan();
    action["passes"][0]["action"] = json!("info");
    let mut dup = plan();
    dup["passes"][1]["id"] = json!("wash");
    let mut engine = plan();
    engine["engine"] = json!(2);
    let mut empty = plan();
    empty["passes"] = json!([]);
    for (name, value, needle) in [
        ("tight", tight, "limit-exceeded"),
        ("unknown", unknown, "unknown pass 0 field"),
        ("action", action, "not allowed"),
        ("dup", dup, "duplicate pass id"),
        ("engine", engine, "plan.engine"),
        ("empty", empty, "needs 1-"),
    ] {
        let file = ws.plan(&format!("{name}.json"), &value);
        let err = fails(&["paint-plan", &doc, &file]);
        assert!(err.contains(needle), "{name}: {err}");
        assert_eq!(before, fs::read(&doc).unwrap(), "{name}");
    }
}

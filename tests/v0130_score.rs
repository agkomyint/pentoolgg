//! v0.13.0 task 11: diagnostics and before/after revision scoring.
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-score-{tag}-{}-{}",
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
    fn file(&self, name: &str, value: &Value) -> String {
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

fn stroke(path: &str, color: &str) -> Value {
    json!({"path": path, "brush": {"size": 8, "hardness": 1, "spacing": 0.1}, "color": color})
}

#[test]
fn plan_passes_report_measured_diagnostics_and_warnings() {
    let ws = Workspace::new("diag");
    let doc = ws.document();
    let plan = json!({
        "schema": 1, "engine": 1, "id": "d",
        "passes": [{
            "id": "bar", "target": "paint", "action": "stroke-path",
            "args": stroke("M4 20 L60 20", "#c03030"),
            "region": [0, 0, 64, 32],
            "protect": [[0, 14, 20, 12]],
            "palette": ["#c03030"],
        }, {
            "id": "out", "target": "paint", "action": "stroke-path",
            "args": stroke("M8 52 L56 52", "#c03030"),
            "region": [0, 0, 64, 32],
        }]
    });
    let file = ws.file("plan.json", &plan);
    let r = ok(&["paint-plan", &doc, &file, "--dry-run"]);
    let passes = r["result"]["passes"].as_array().unwrap();
    let m = &passes[0]["diagnostics"]["measured"];
    assert!(m["changed_pixels"].as_u64().unwrap() > 0);
    assert!(m["protected_violations"].as_u64().unwrap() > 0, "{m}");
    assert_eq!(m["edge_spill_pixels"], 0);
    assert_eq!(m["palette_drift"], 0.0);
    let codes = |p: &Value| -> Vec<String> {
        p["diagnostics"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| format!("{}:{}", w["basis"].as_str().unwrap(), w["code"].as_str().unwrap()))
            .collect()
    };
    assert!(codes(&passes[0]).contains(&"measured:protected-region-violation".to_string()));
    let spill = &passes[1]["diagnostics"]["measured"];
    assert!(spill["edge_spill_pixels"].as_u64().unwrap() > 0);
    assert!(codes(&passes[1]).contains(&"measured:edge-spill".to_string()));
    assert!(codes(&passes[1]).contains(&"heuristic:flat-values".to_string()));
}

#[test]
fn scoring_compares_revisions_without_changing_anything() {
    let ws = Workspace::new("score");
    let before = ws.document();
    let after = ws.0.join("after.pen").to_string_lossy().into_owned();
    fs::copy(&before, &after).unwrap();
    ok(&[
        "raster", &after, "stroke-path", "paint", "--path", "M2 32 L62 32", "--brush",
        r#"{"size":20,"hardness":1,"spacing":0.1}"#, "--color", "#204080",
    ]);
    let (b, a) = (fs::read(&before).unwrap(), fs::read(&after).unwrap());
    let goal = ws.file(
        "goal.json",
        &json!({
            "region": [0, 0, 64, 64],
            "coverage": [0.2, 0.5],
            "palette": ["#204080"],
            "value_range": [20, 90],
        }),
    );
    let goal_arg = format!("@{goal}");
    let r = ok(&[
        "paint-score", &before, &after, "--layer", "paint", "--goal", &goal_arg,
    ]);
    assert_eq!(r["applied"], false);
    assert_eq!(r["assessment"]["basis"], "heuristic");
    assert_eq!(r["assessment"]["trend"], "improved", "{r}");
    assert!(r["after"]["score"].as_f64().unwrap() > r["before"]["score"].as_f64().unwrap());
    assert!(r["diagnostics"]["measured"]["changed_pixels"].as_u64().unwrap() > 0);
    // Reverse direction is worse; scoring is deterministic and read-only.
    let back = ok(&[
        "paint-score", &after, &before, "--layer", "paint", "--goal", &goal_arg,
    ]);
    assert_eq!(back["assessment"]["trend"], "worse");
    assert_eq!(
        r,
        ok(&["paint-score", &before, &after, "--layer", "paint", "--goal", &goal_arg])
    );
    assert_eq!(b, fs::read(&before).unwrap());
    assert_eq!(a, fs::read(&after).unwrap());
}

#[test]
fn bad_goals_and_regions_are_rejected() {
    let ws = Workspace::new("bad");
    let doc = ws.document();
    for (goal, needle) in [
        (r#"{}"#, "needs at least one"),
        (r#"{"coverage":[0.8,0.2]}"#, "coverage"),
        (r#"{"value_range":[0,300]}"#, "value_range"),
        (r#"{"coverage":[0,1],"zzz":1}"#, "unknown goal field"),
        (r#"{"coverage":[0,1],"region":[0,0,0,4]}"#, "region"),
    ] {
        let err = fails(&["paint-score", &doc, &doc, "--layer", "paint", "--goal", goal]);
        assert!(err.contains(needle), "{goal}: {err}");
    }
    let plan = json!({
        "schema": 1, "engine": 1, "id": "d",
        "passes": [{"id": "p", "target": "paint", "action": "stroke-path",
                    "args": stroke("M4 4 L20 4", "#000000"), "region": [0, 0, 0, 3]}]
    });
    let file = ws.file("plan.json", &plan);
    let err = fails(&["paint-plan", &doc, &file, "--dry-run"]);
    assert!(err.contains("region"), "{err}");
}

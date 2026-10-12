//! v0.14.0 task 6: nested, line and GREP style evaluation, and lists.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-style-eval-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Workspace(dir)
    }
    fn doc(&self) -> String {
        self.0.join("doc.pen").to_string_lossy().into_owned()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.0.join("doc.pen")).unwrap()).unwrap()
    }
    fn write(&self, raw: &Value) {
        std::fs::write(
            self.0.join("doc.pen"),
            serde_json::to_vec_pretty(raw).unwrap(),
        )
        .unwrap();
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.0.join("doc.pen")).unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let out = run(args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn book(ws: &Workspace) -> String {
    let doc = ws.doc();
    ok(&["new", &doc]);
    ok(&["migrate", &doc, "--target", "9"]);
    doc
}

#[test]
fn nested_and_grep_styles_become_ordered_spans() {
    let ws = Workspace::new("spans");
    let doc = book(&ws);
    ok(&["layout-style", &doc, "create", "character", "lead"]);
    ok(&["layout-style", &doc, "create", "character", "num"]);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "body",
        "--set",
        r#"nested=[{"style":"lead","through":2,"of":"word"}]"#,
        "--set",
        r#"grep=[{"style":"num","pattern":"\\d+"}]"#,
    ]);
    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": [
        {"style": "body", "runs": [{"text": "The 12 rivers carry 300 stories"}]}
    ]}]);
    ws.write(&raw);
    let r = ok(&["layout-style", &doc, "evaluate", "s"]);
    let spans = r["evaluated"][0]["spans"].as_array().unwrap();
    // Nested: "The 12 " through the second word; GREP applies afterwards.
    assert_eq!(
        spans[0],
        json!({"start": 0, "end": 7, "style": "lead", "source": "nested"})
    );
    assert_eq!(spans[1]["source"], "grep");
    assert_eq!(
        (spans[1]["start"].as_u64(), spans[1]["end"].as_u64()),
        (Some(4), Some(6))
    );
    assert_eq!(
        (spans[2]["start"].as_u64(), spans[2]["end"].as_u64()),
        (Some(20), Some(23))
    );
    // Line styles need composition and are disclosed, never dropped silently.
    ok(&[
        "layout-style",
        &doc,
        "set",
        "paragraph",
        "body",
        "--set",
        r#"line_styles=[{"style":"lead","lines":1}]"#,
    ]);
    let r = ok(&["layout-style", &doc, "evaluate", "s"]);
    assert_eq!(
        r["evaluated"][0]["skipped"][0]["reason"],
        "needs-composition"
    );
}

#[test]
fn bad_patterns_are_refused_at_write_time() {
    let ws = Workspace::new("bad");
    let doc = book(&ws);
    ok(&["layout-style", &doc, "create", "character", "c"]);
    ok(&["layout-style", &doc, "create", "paragraph", "p"]);
    let before = ws.bytes();
    for pattern in [r"(a)\\1", "(?<=a)b", "(a*)*", "["] {
        let rules = format!(r#"grep=[{{"style":"c","pattern":"{pattern}"}}]"#);
        let err = fails(&[
            "layout-style",
            &doc,
            "set",
            "paragraph",
            "p",
            "--set",
            &rules,
        ]);
        assert!(err.contains("GREP"), "{pattern}: {err}");
        assert_eq!(before, ws.bytes(), "{pattern} mutated the document");
    }
}

#[test]
fn lists_number_across_stories_and_restart_by_level() {
    let ws = Workspace::new("lists");
    let doc = book(&ws);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "item",
        "--set",
        r#"list={"list":"steps","level":1}"#,
    ]);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "sub",
        "--set",
        r#"list={"list":"steps","level":2}"#,
    ]);
    let mut raw = ws.json();
    raw["layout"]["lists"] = json!([{"id": "steps", "levels": [
        {"format": "^#.^t"},
        {"format": "^1.^#)", "number_style": "a"}
    ]}]);
    let p = |style: &str| json!({"style": style, "runs": [{"text": "x"}]});
    raw["layout"]["stories"] = json!([
        {"id": "a", "paragraphs": [p("item"), p("sub"), p("sub"), p("item"), p("sub")]},
        {"id": "b", "paragraphs": [p("item")]}
    ]);
    ws.write(&raw);
    let r = ok(&["layout-style", &doc, "lists"]);
    let labels: Vec<String> = r["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(labels, ["1.\t", "1.a)", "1.b)", "2.\t", "2.a)", "3.\t"]);
    // Continuation: story b picks up at 3 because the list ID is shared.
    let only = ok(&["layout-style", &doc, "lists", "--story", "b"]);
    assert_eq!(only["matches"], 1);
    assert_eq!(only["labels"][0]["number"], 3);
}

#[test]
fn undefined_lists_are_reported_not_hidden() {
    let ws = Workspace::new("undefined");
    let doc = book(&ws);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "item",
        "--set",
        r#"list={"list":"nope"}"#,
    ]);
    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{"id": "a", "paragraphs": [
        {"style": "item", "runs": [{"text": "x"}]}]}]);
    ws.write(&raw);
    let r = ok(&["layout-style", &doc, "lists"]);
    assert_eq!(r["problems"][0]["problem"], "undefined-list");
}

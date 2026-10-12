//! v0.14.0 task 6: layout styles.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-styles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Workspace(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
    fn doc(&self) -> String {
        self.path("doc.pen")
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.0.join("doc.pen")).unwrap()).unwrap()
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

fn with_story(ws: &Workspace) {
    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{
        "id": "body-story",
        "paragraphs": [
            {"style": "body", "runs": [{"text": "One", "style": "em"}], "overrides": {"size": 14}},
            {"style": "lead", "runs": [{"text": "Two"}]}
        ]
    }]);
    std::fs::write(
        ws.0.join("doc.pen"),
        serde_json::to_vec_pretty(&raw).unwrap(),
    )
    .unwrap();
}

#[test]
fn create_resolve_and_inherit() {
    let ws = Workspace::new("resolve");
    let doc = book(&ws);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "base",
        "--set",
        "size=11",
        "--set",
        "align=\"left\"",
    ]);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "lead",
        "--based-on",
        "base",
        "--set",
        "size=14",
    ]);
    let shown = ok(&["layout-style", &doc, "show", "paragraph", "lead"]);
    assert_eq!(shown["chain"], json!(["lead", "base"]));
    assert_eq!(shown["resolved"]["size"], 14.0);
    assert_eq!(shown["resolved"]["align"], "left");
    let list = ok(&[
        "layout-style",
        &doc,
        "list",
        "--kind",
        "paragraph",
        "--limit",
        "1",
    ]);
    assert_eq!(list["matches"], 2);
    assert_eq!(list["returned"], 1);
    assert_eq!(list["has_more"], true);
}

#[test]
fn invalid_styles_are_rejected_without_mutation() {
    let ws = Workspace::new("invalid");
    let doc = book(&ws);
    ok(&["layout-style", &doc, "create", "paragraph", "a"]);
    let before = ws.bytes();
    assert!(fails(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "b",
        "--based-on",
        "nope"
    ])
    .contains("nope"));
    assert!(fails(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "c",
        "--set",
        "size=-3"
    ])
    .contains("size"));
    assert!(fails(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "d",
        "--set",
        "bogus=1"
    ])
    .contains("bogus"));
    assert!(fails(&[
        "layout-style",
        &doc,
        "create",
        "character",
        "e",
        "--next",
        "a"
    ])
    .contains("next"));
    assert!(fails(&["layout-style", &doc, "create", "paragraph", "a"]).contains("already exists"));
    assert!(fails(&[
        "layout-style",
        &doc,
        "set",
        "paragraph",
        "a",
        "--based-on",
        "a"
    ])
    .contains("cycle"));
    assert_eq!(before, ws.bytes());
}

#[test]
fn cycles_and_depth_are_detected() {
    let ws = Workspace::new("cycle");
    let doc = book(&ws);
    ok(&["layout-style", &doc, "create", "paragraph", "s0"]);
    for i in 1..=15 {
        ok(&[
            "layout-style",
            &doc,
            "create",
            "paragraph",
            &format!("s{i}"),
            "--based-on",
            &format!("s{}", i - 1),
        ]);
    }
    let err = fails(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "s16",
        "--based-on",
        "s15",
    ]);
    assert!(err.contains("deeper"), "{err}");
    let err = fails(&[
        "layout-style",
        &doc,
        "set",
        "paragraph",
        "s0",
        "--based-on",
        "s5",
    ]);
    assert!(err.contains("cycle"), "{err}");
}

#[test]
fn usage_reports_derived_styles_and_stories() {
    let ws = Workspace::new("usage");
    let doc = book(&ws);
    ok(&["layout-style", &doc, "create", "paragraph", "body"]);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "lead",
        "--based-on",
        "body",
        "--next",
        "body",
    ]);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "character",
        "em",
        "--set",
        "italic=true",
    ]);
    with_story(&ws);
    let u = ok(&["layout-style", &doc, "usage", "paragraph", "body"]);
    assert_eq!(u["derived"], json!(["lead"]));
    assert_eq!(u["next_of"], json!(["lead"]));
    assert_eq!(u["paragraph_references"], 2);
    assert_eq!(u["affected_stories"], json!(["body-story"]));
    let c = ok(&["layout-style", &doc, "usage", "character", "em"]);
    assert_eq!(c["paragraph_references"], 1);
}

#[test]
fn set_reports_affected_stories_and_dry_run_is_pure() {
    let ws = Workspace::new("set");
    let doc = book(&ws);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "body",
        "--set",
        "size=11",
    ]);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "lead",
        "--based-on",
        "body",
    ]);
    with_story(&ws);
    let before = ws.bytes();
    let dry = ok(&[
        "layout-style",
        &doc,
        "--dry-run",
        "set",
        "paragraph",
        "body",
        "--set",
        "size=13",
    ]);
    assert_eq!(dry["affected_styles"], 1);
    assert_eq!(dry["affected_stories"], 1);
    assert_eq!(before, ws.bytes());
    ok(&[
        "layout-style",
        &doc,
        "set",
        "paragraph",
        "body",
        "--set",
        "size=13",
    ]);
    assert_eq!(
        ws.json()["layout"]["styles"]["paragraph"]["body"]["size"],
        13.0
    );
}

#[test]
fn remove_requires_reassign_when_in_use() {
    let ws = Workspace::new("remove");
    let doc = book(&ws);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "body",
        "--set",
        "size=11",
    ]);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "lead",
        "--based-on",
        "body",
    ]);
    ok(&["layout-style", &doc, "create", "paragraph", "other"]);
    with_story(&ws);
    let before = ws.bytes();
    assert!(fails(&["layout-style", &doc, "remove", "paragraph", "body"]).contains("in-use"));
    assert_eq!(before, ws.bytes());
    ok(&[
        "layout-style",
        &doc,
        "remove",
        "paragraph",
        "body",
        "--reassign",
        "other",
    ]);
    let raw = ws.json();
    assert!(raw["layout"]["styles"]["paragraph"]["body"].is_null());
    assert_eq!(
        raw["layout"]["styles"]["paragraph"]["lead"]["based_on"],
        "other"
    );
    assert_eq!(
        raw["layout"]["stories"][0]["paragraphs"][0]["style"],
        "other"
    );
}

#[test]
fn redefine_and_clear_overrides() {
    let ws = Workspace::new("redefine");
    let doc = book(&ws);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "body",
        "--set",
        "size=11",
    ]);
    ok(&["layout-style", &doc, "create", "paragraph", "lead"]);
    ok(&["layout-style", &doc, "create", "character", "em"]);
    with_story(&ws);
    ok(&["layout-style", &doc, "redefine", "body-story", "0"]);
    let raw = ws.json();
    assert_eq!(raw["layout"]["styles"]["paragraph"]["body"]["size"], 14);
    assert!(raw["layout"]["stories"][0]["paragraphs"][0]["overrides"].is_null());
    assert!(fails(&["layout-style", &doc, "redefine", "body-story", "0"]).contains("no overrides"));
    assert!(fails(&["layout-style", &doc, "redefine", "body-story", "9"]).contains("out of range"));
    ok(&["layout-style", &doc, "clear-overrides", "body-story"]);
}

#[test]
fn import_policies() {
    let ws = Workspace::new("import");
    let doc = book(&ws);
    ok(&[
        "layout-style",
        &doc,
        "create",
        "paragraph",
        "body",
        "--set",
        "size=11",
    ]);
    let other = ws.path("other.pen");
    ok(&["new", &other]);
    ok(&["migrate", &other, "--target", "9"]);
    ok(&[
        "layout-style",
        &other,
        "create",
        "paragraph",
        "body",
        "--set",
        "size=20",
    ]);
    ok(&[
        "layout-style",
        &other,
        "create",
        "paragraph",
        "h1",
        "--based-on",
        "body",
    ]);
    let r = ok(&["layout-style", &doc, "import", &other]);
    assert_eq!(r["imported"], 2);
    let raw = ws.json();
    assert_eq!(raw["layout"]["styles"]["paragraph"]["body"]["size"], 11.0);
    assert_eq!(raw["layout"]["styles"]["paragraph"]["body-2"]["size"], 20.0);
    assert_eq!(
        raw["layout"]["styles"]["paragraph"]["h1"]["based_on"],
        "body-2"
    );
    ok(&[
        "layout-style",
        &doc,
        "import",
        &other,
        "--policy",
        "replace",
    ]);
    assert_eq!(
        ws.json()["layout"]["styles"]["paragraph"]["body"]["size"],
        20.0
    );
    assert!(
        fails(&["layout-style", &doc, "import", &other, "--policy", "nope"]).contains("policy")
    );
}

#[test]
fn check_reports_undefined_references() {
    let ws = Workspace::new("check");
    let doc = book(&ws);
    ok(&["layout-style", &doc, "create", "paragraph", "body"]);
    with_story(&ws);
    let c = ok(&["layout-style", &doc, "check"]);
    assert_eq!(c["valid"], true);
    assert_eq!(c["undefined_references"]["paragraph:lead"], 1);
    assert_eq!(c["undefined_references"]["character:em"], 1);
}

#[test]
fn legacy_documents_are_refused_clearly() {
    let ws = Workspace::new("legacy");
    let doc = ws.doc();
    ok(&["new", &doc]);
    let err = fails(&["layout-style", &doc, "list"]);
    assert!(err.contains("unsupported-version"), "{err}");
}

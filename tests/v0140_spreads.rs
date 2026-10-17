//! v0.14.0 task 2: spreads, parents, sections and page numbering.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-spreads-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Workspace(dir)
    }
    fn doc(&self) -> String {
        self.0.join("doc.pen").to_string_lossy().into_owned()
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.0.join("doc.pen")).unwrap()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes()).unwrap()
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

/// A v9 document with pages page-1..p`count`, each a letter-sized single page.
fn book(ws: &Workspace, count: usize) -> String {
    let doc = ws.doc();
    ok(&["new", &doc, "--width", "612", "--height", "792"]);
    for n in 2..=count {
        ok(&["page", &doc, "add", &format!("p{n}")]);
    }
    ok(&["migrate", &doc, "--target", "9"]);
    doc
}

#[test]
fn migration_to_v9_is_lossless_and_each_page_becomes_a_single_spread() {
    let ws = Workspace::new("migrate");
    let doc = ws.doc();
    ok(&["new", &doc]);
    ok(&["page", &doc, "add", "second"]);
    let before = ws.json();
    ok(&["migrate", &doc, "--target", "9"]);
    let after = ws.json();
    assert_eq!(after["version"], 9);
    assert_eq!(after["pages"], before["pages"]);
    let spreads = after["layout"]["spreads"].as_array().unwrap();
    assert_eq!(spreads.len(), 2);
    assert!(spreads
        .iter()
        .all(|s| s["facing"] == false && s["pages"].as_array().unwrap().len() == 1));
    assert!(after["pages"][0].get("layout").is_none());
    // Back down again: nothing but the empty layout is dropped.
    ok(&["migrate", &doc, "--target", "8"]);
    let down = ws.json();
    assert_eq!(down["version"], 8);
    assert!(down.get("layout").is_none());
    assert_eq!(down["pages"], before["pages"]);
}

#[test]
fn downgrade_refuses_layout_that_version_8_cannot_hold_and_writes_nothing() {
    let ws = Workspace::new("downgrade");
    let doc = book(&ws, 2);
    ok(&["parent", &doc, "create", "A-Parent"]);
    let before = ws.bytes();
    let error = fails(&["migrate", &doc, "--target", "8"]);
    assert!(
        error.contains("unsupported-capability") && error.contains("A-Parent"),
        "{error}"
    );
    assert_eq!(ws.bytes(), before);
}

#[test]
fn sections_number_pages_and_page_operations_report_renumbering() {
    let ws = Workspace::new("sections");
    let doc = book(&ws, 6);
    ok(&[
        "section",
        &doc,
        "add",
        "page-1",
        "--id",
        "front",
        "--style",
        "lower-roman",
    ]);
    ok(&[
        "section",
        &doc,
        "add",
        "p3",
        "--id",
        "body",
        "--start-number",
        "1",
    ]);
    let list = ok(&["section", &doc, "list"]);
    let labels: Vec<&str> = list["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["i", "ii", "1", "2", "3", "4"]);
    assert_eq!(list["pages"][3]["previous"], "1");
    assert_eq!(list["pages"][3]["next"], "3");

    // Every emitted array obeys the pagination window: a window past the end
    // returns no pages AND no sections (issue #163).
    let past = ok(&["section", &doc, "list", "--offset", "10", "--limit", "5"]);
    assert_eq!(past["pages"].as_array().unwrap().len(), 0);
    assert!(past["sections"].as_array().unwrap().is_empty());
    assert_eq!(past["returned"], 0);
    assert_eq!(past["has_more"], false);
    let narrow = ok(&["section", &doc, "list", "--limit", "1"]);
    assert_eq!(narrow["sections"].as_array().unwrap().len(), 1);
    assert_eq!(
        narrow["has_more"], true,
        "one page at a time must report more"
    );

    // A page inserted into the body shifts every later label by one.
    let added = ok(&["page", &doc, "add", "extra", "--after", "p3"]);
    assert_eq!(added["reflow"]["pages_added"], json!(["extra"]));
    assert_eq!(added["reflow"]["pages_renumbered"], 3);
    assert_eq!(added["reflow"]["stories_reflowed"], 0);
    let ids: Vec<Value> = ws.json()["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].clone())
        .collect();
    assert_eq!(ids, ["page-1", "p2", "p3", "extra", "p4", "p5", "p6"]);

    // Removing the page a section starts on moves the section to the next page.
    let removed = ok(&["page", &doc, "remove", "p3"]);
    assert_eq!(removed["reflow"]["sections_changed"], json!(["body"]));
    let doc_json = ws.json();
    let body = doc_json["layout"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "body")
        .unwrap();
    assert_eq!(body["start_page"], "extra");
}

#[test]
fn facing_spreads_resolve_inner_and_outer_margins_per_side() {
    let ws = Workspace::new("facing");
    let doc = book(&ws, 3);
    ok(&[
        "parent",
        &doc,
        "create",
        "A",
        "--margins",
        "36,54,36,18",
        "--pages",
        "L,R",
    ]);
    for page in ["page-1", "p2", "p3"] {
        ok(&["parent", &doc, "assign", page, "--parent", "A"]);
    }
    ok(&["spread", &doc, "arrange"]);
    let spreads = ok(&["spread", &doc, "list"]);
    assert_eq!(spreads["spreads"].as_array().unwrap().len(), 2);
    assert_eq!(spreads["spreads"][1]["pages"], json!(["p2", "p3"]));

    let right = ok(&["parent", &doc, "resolve", "p3"]);
    let left = ok(&["parent", &doc, "resolve", "p2"]);
    assert_eq!(left["side"], "left");
    assert_eq!(right["side"], "right");
    // [top, left, bottom, right]: inner margin faces the spine on both sides.
    assert_eq!(left["margins"], json!([36.0, 18.0, 36.0, 54.0]));
    assert_eq!(right["margins"], json!([36.0, 54.0, 36.0, 18.0]));
    // The cover is alone on its spread.
    assert_eq!(ok(&["parent", &doc, "resolve", "page-1"])["side"], "right");

    ok(&["spread", &doc, "split", "s2"]);
    assert_eq!(
        ok(&["spread", &doc, "list"])["spreads"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

fn rect(id: &str, x: u32, fill: &str) -> String {
    json!({
        "kind": "rect", "id": id, "x": x, "y": 10, "width": 40, "height": 20,
        "style": {"fill": {"fallback": fill}}
    })
    .to_string()
}

#[test]
fn parent_items_inherit_override_detach_and_report_orphans() {
    let ws = Workspace::new("parents");
    let doc = book(&ws, 2);
    ok(&["parent", &doc, "create", "Base"]);
    ok(&[
        "parent",
        &doc,
        "item-add",
        "Base",
        "--node",
        &rect("band", 0, "#111111"),
    ]);
    ok(&["parent", &doc, "create", "Chapter", "--based-on", "Base"]);
    ok(&[
        "parent",
        &doc,
        "item-add",
        "Chapter",
        "--node",
        &rect("rule", 5, "#222222"),
    ]);
    ok(&["parent", &doc, "assign", "page-1", "--parent", "Chapter"]);

    let resolved = ok(&["parent", &doc, "resolve", "page-1"]);
    assert_eq!(resolved["chain"], json!(["Base", "Chapter"]));
    let items: Vec<&str> = resolved["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(items, ["page-1/band", "page-1/rule"]);

    // A per-page override changes one property without touching the parent.
    ok(&["override", &doc, "set", "page-1", "band", "--set", "x=12"]);
    let resolved = ok(&["parent", &doc, "resolve", "page-1"]);
    assert_eq!(resolved["items"][0]["node"]["x"], 12);
    assert_eq!(resolved["items"][0]["overridden"], json!(["x"]));
    assert_eq!(
        ws.json()["layout"]["parents"][0]["layers"][0]["nodes"][0]["x"],
        0
    );

    // Detaching copies the overridden item onto the page and stops following.
    let detached = ok(&["override", &doc, "detach", "page-1", "band"]);
    assert_eq!(detached["node"], "band");
    let resolved = ok(&["parent", &doc, "resolve", "page-1"]);
    assert_eq!(resolved["items"].as_array().unwrap().len(), 1);
    assert_eq!(resolved["detached"], json!(["band"]));
    let page_nodes = &ws.json()["pages"][0]["layers"][0]["nodes"];
    assert_eq!(page_nodes[0]["id"], "band");
    assert_eq!(page_nodes[0]["x"], 12);

    // Removing the parent's item orphans (never drops) page overrides.
    ok(&["override", &doc, "set", "page-1", "rule", "--set", "x=9"]);
    let removed = ok(&["parent", &doc, "item-remove", "Chapter", "rule"]);
    assert_eq!(removed["orphaned_overrides_on"], json!(["page-1"]));
    let resolved = ok(&["parent", &doc, "resolve", "page-1"]);
    assert_eq!(resolved["orphaned_overrides"], json!(["rule"]));
    assert!(
        ws.json()["pages"][0]["layout"]["overrides"]
            .as_array()
            .unwrap()
            .len()
            == 1
    );

    // A parent in use or with derived parents cannot be removed.
    let err = fails(&["parent", &doc, "remove", "Chapter"]);
    assert!(err.contains("in-use"), "{err}");
    // The corrective hint must name the real command (`page parent` does not
    // exist; assignment lives under `parent <file> assign`).
    assert!(
        err.contains("`pentool parent <file> assign <page> --parent <id>`"),
        "{err}"
    );
    assert!(!err.contains("`page parent"), "{err}");
    assert!(fails(&["parent", &doc, "remove", "Base"]).contains("in-use"));
}

#[test]
fn failed_commands_dry_runs_and_undo_leave_the_document_consistent() {
    let ws = Workspace::new("safety");
    let doc = book(&ws, 2);
    let before = ws.bytes();

    for args in [
        vec!["page", &doc, "add", "page-1"],
        vec!["page", &doc, "add", "x", "--after", "nope"],
        vec![
            "page", &doc, "add", "x", "--after", "page-1", "--before", "p2",
        ],
        vec!["page", &doc, "add", "x", "--parent", "missing"],
        vec!["section", &doc, "add", "nope"],
        vec!["section", &doc, "add", "page-1", "--style", "klingon"],
        vec!["section", &doc, "add", "page-1", "--start-number", "0"],
        vec!["spread", &doc, "merge", "p2", "page-1"],
        vec!["parent", &doc, "assign", "page-1", "--parent", "missing"],
        vec!["override", &doc, "set", "page-1", "ghost", "--set", "x=1"],
    ] {
        let error = fails(&args);
        assert!(error.contains('['), "{args:?}: {error}");
        assert_eq!(ws.bytes(), before, "{args:?} changed the document");
    }

    ok(&["page", &doc, "--dry-run", "add", "p3"]);
    assert_eq!(ws.bytes(), before);

    ok(&["page", &doc, "add", "p3"]);
    assert_ne!(ws.bytes(), before);
    ok(&["undo", &doc]);
    assert_eq!(ws.bytes(), before);
}

#[test]
fn pre_v9_documents_reject_layout_commands_clearly() {
    let ws = Workspace::new("old");
    let doc = ws.doc();
    ok(&["new", &doc]);
    let before = ws.bytes();
    for args in [
        vec!["section", &doc, "add", "page-1"],
        vec!["spread", &doc, "arrange"],
        vec!["parent", &doc, "create", "A"],
        vec!["page", &doc, "add", "x", "--after", "page-1"],
    ] {
        let error = fails(&args);
        assert!(error.contains("version 9"), "{args:?}: {error}");
        assert_eq!(ws.bytes(), before);
    }
}

#[test]
fn page_list_carries_spread_parent_and_label_for_v9() {
    let ws = Workspace::new("list");
    let doc = book(&ws, 2);
    ok(&["section", &doc, "add", "page-1", "--prefix", "A-"]);
    let list = ok(&["page", &doc, "list"]);
    assert_eq!(list["pages"][1]["label"], "A-2");
    assert_eq!(list["pages"][1]["spread"], "s2");
    assert_eq!(list["pages"][0]["facing"], false);
}

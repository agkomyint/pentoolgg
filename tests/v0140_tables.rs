//! v0.14.0 task 9: tables.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-tables-{name}-{}", std::process::id()));
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
    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": [
        {"runs": [{"text": "intro"}]},
        {"runs": [{"text": "a"}, {"special": "tab"}, {"text": "b"}, {"special": "tab"}, {"text": "c"}]},
        {"runs": [{"text": "d"}, {"special": "tab"}, {"text": "e"}]},
        {"runs": [{"text": "outro"}]}
    ]}]);
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "text-frame", "id": "f", "story": "s", "next": null,
         "x": 0, "y": 0, "width": 200, "height": 100},
        {"kind": "rect", "id": "pic", "x": 0, "y": 0, "width": 10, "height": 10, "fill": "#336699"}
    ]);
    ws.write(&raw);
    doc
}

fn table(doc: &str, id: &str, rows: &str, cols: &str) {
    ok(&[
        "layout-table",
        doc,
        "create",
        id,
        "--rows",
        rows,
        "--columns",
        cols,
    ]);
}

fn cell(raw: &Value, row: u64, col: u64) -> Option<&Value> {
    raw["layout"]["tables"][0]["cells"]
        .as_array()?
        .iter()
        .find(|c| c["row"] == row && c["col"] == col)
}

#[test]
fn create_cells_and_formatting_round_trip() {
    let ws = Workspace::new("cells");
    let doc = book(&ws);
    table(&doc, "t", "3", "2");
    ok(&[
        "layout-table",
        &doc,
        "cell-set",
        "t",
        "--row",
        "0",
        "--col",
        "1",
        "--text",
        "Hello\\nWorld",
        "--inset",
        "2,3,4,5",
        "--align",
        "right",
        "--rotation",
        "90",
        "--fill",
        "#eeeeee",
        "--stroke-color",
        "#000000",
        "--stroke-width",
        "0.5",
        "--graphic",
        "pic",
    ]);
    let raw = ws.json();
    let c = cell(&raw, 0, 1).unwrap();
    assert_eq!(c["paragraphs"].as_array().unwrap().len(), 2);
    assert_eq!(c["inset"], json!([2.0, 3.0, 4.0, 5.0]));
    assert_eq!(c["rotation"], 90);
    assert_eq!(c["stroke"]["width"], 0.5);
    assert_eq!(c["graphic"], "pic");
    let shown = ok(&["layout-table", &doc, "show", "t"]);
    assert_eq!(shown["rows"], 3);
    assert_eq!(shown["page"]["matches"], 1);
    let before = ws.bytes();
    assert!(fails(&[
        "layout-table",
        &doc,
        "cell-set",
        "t",
        "--row",
        "9",
        "--col",
        "0"
    ])
    .contains("outside"));
    assert!(fails(&[
        "layout-table",
        &doc,
        "cell-set",
        "t",
        "--row",
        "0",
        "--col",
        "0",
        "--rotation",
        "45"
    ])
    .contains("rotation"));
    assert!(fails(&[
        "layout-table",
        &doc,
        "cell-set",
        "t",
        "--row",
        "0",
        "--col",
        "0",
        "--graphic",
        "ghost"
    ])
    .contains("ghost"));
    assert!(fails(&[
        "layout-table",
        &doc,
        "create",
        "t",
        "--rows",
        "1",
        "--columns",
        "1"
    ])
    .contains("already exists"));
    assert_eq!(before, ws.bytes());
}

#[test]
fn merge_concatenates_content_and_split_restores_cells() {
    let ws = Workspace::new("merge");
    let doc = book(&ws);
    table(&doc, "t", "2", "2");
    ok(&[
        "layout-table",
        &doc,
        "cell-set",
        "t",
        "--row",
        "0",
        "--col",
        "0",
        "--text",
        "one",
    ]);
    ok(&[
        "layout-table",
        &doc,
        "cell-set",
        "t",
        "--row",
        "0",
        "--col",
        "1",
        "--text",
        "two",
    ]);
    let r = ok(&[
        "layout-table",
        &doc,
        "merge",
        "t",
        "--row",
        "0",
        "--col",
        "0",
        "--rows",
        "1",
        "--cols",
        "2",
    ]);
    assert_eq!(r["cells_merged"], 2);
    let raw = ws.json();
    let c = cell(&raw, 0, 0).unwrap();
    assert_eq!(c["col_span"], 2);
    let text: Vec<&str> = c["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["runs"][0]["text"].as_str().unwrap())
        .collect();
    assert_eq!(text, vec!["one", "two"]);
    assert!(cell(&raw, 0, 1).is_none());
    assert!(fails(&[
        "layout-table",
        &doc,
        "merge",
        "t",
        "--row",
        "0",
        "--col",
        "1",
        "--rows",
        "2",
        "--cols",
        "1"
    ])
    .contains("cuts through"));
    assert!(fails(&[
        "layout-table",
        &doc,
        "cell-set",
        "t",
        "--row",
        "0",
        "--col",
        "1",
        "--text",
        "x"
    ])
    .contains("covered"));
    ok(&[
        "layout-table",
        &doc,
        "split",
        "t",
        "--row",
        "0",
        "--col",
        "0",
    ]);
    let raw = ws.json();
    assert!(cell(&raw, 0, 0).unwrap().get("col_span").is_none());
    assert_eq!(
        cell(&raw, 0, 0).unwrap()["paragraphs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn rows_and_columns_never_drop_content_silently() {
    let ws = Workspace::new("rows");
    let doc = book(&ws);
    table(&doc, "t", "3", "3");
    ok(&[
        "layout-table",
        &doc,
        "cell-set",
        "t",
        "--row",
        "1",
        "--col",
        "1",
        "--text",
        "keep",
    ]);
    let before = ws.bytes();
    assert!(fails(&["layout-table", &doc, "remove-row", "t", "--at", "1"]).contains("--discard"));
    assert!(
        fails(&["layout-table", &doc, "remove-column", "t", "--at", "1"]).contains("--discard")
    );
    assert_eq!(before, ws.bytes());
    ok(&["layout-table", &doc, "add-row", "t", "--at", "0"]);
    let raw = ws.json();
    assert_eq!(
        raw["layout"]["tables"][0]["rows"].as_array().unwrap().len(),
        4
    );
    assert!(cell(&raw, 2, 1).is_some());
    ok(&[
        "layout-table",
        &doc,
        "add-column",
        "t",
        "--at",
        "3",
        "--width",
        "30",
    ]);
    let r = ok(&[
        "layout-table",
        &doc,
        "remove-row",
        "t",
        "--at",
        "2",
        "--discard",
    ]);
    assert_eq!(r["cells_discarded"], 1);
    ok(&["layout-table", &doc, "remove-column", "t", "--at", "3"]);
    ok(&[
        "layout-table",
        &doc,
        "merge",
        "t",
        "--row",
        "0",
        "--col",
        "0",
        "--rows",
        "2",
        "--cols",
        "1",
    ]);
    assert!(
        fails(&["layout-table", &doc, "remove-row", "t", "--at", "0"]).contains("split it first")
    );
}

#[test]
fn csv_import_follows_rfc4180_reports_ragged_rows_and_limits() {
    let ws = Workspace::new("csv");
    let doc = book(&ws);
    let csv = ws.0.join("data.csv");
    std::fs::write(&csv, "name,note\r\n\"Ada, L\",\"said \"\"hi\"\"\"\nBob\n").unwrap();
    let r = ok(&[
        "layout-table",
        &doc,
        "import",
        "t",
        csv.to_str().unwrap(),
        "--header-rows",
        "1",
    ]);
    assert_eq!(r["rows"], 3);
    assert_eq!(r["columns"], 2);
    assert_eq!(r["ragged_rows"].as_array().unwrap().len(), 1);
    let raw = ws.json();
    assert_eq!(raw["layout"]["tables"][0]["header_rows"], 1);
    assert_eq!(
        cell(&raw, 1, 0).unwrap()["paragraphs"][0]["runs"][0]["text"],
        "Ada, L"
    );
    assert_eq!(
        cell(&raw, 1, 1).unwrap()["paragraphs"][0]["runs"][0]["text"],
        "said \"hi\""
    );
    let before = ws.bytes();
    let bad = ws.0.join("bad.csv");
    std::fs::write(&bad, "a,\"unterminated\n").unwrap();
    assert!(
        fails(&["layout-table", &doc, "import", "u", bad.to_str().unwrap()])
            .contains("never closed")
    );
    let wide = ws.0.join("wide.csv");
    std::fs::write(&wide, vec!["x"; 300].join(",")).unwrap();
    assert!(fails(&["layout-table", &doc, "import", "u", wide.to_str().unwrap()]).contains("256"));
    let tsv = ws.0.join("t.txt");
    std::fs::write(&tsv, "a\tb\n").unwrap();
    assert!(
        fails(&["layout-table", &doc, "import", "u", tsv.to_str().unwrap()])
            .contains("unsupported import format")
    );
    assert_eq!(before, ws.bytes());
    ok(&[
        "layout-table",
        &doc,
        "import",
        "tabs",
        tsv.to_str().unwrap(),
        "--format",
        "tsv",
    ]);
}

#[test]
fn text_converts_to_a_table_and_back_without_loss() {
    let ws = Workspace::new("convert");
    let doc = book(&ws);
    let r = ok(&[
        "layout-table",
        &doc,
        "from-text",
        "t",
        "--story",
        "s",
        "--paragraph",
        "1",
        "--count",
        "2",
    ]);
    assert_eq!(r["rows"], 2);
    assert_eq!(r["columns"], 3);
    assert_eq!(r["ragged_rows"].as_array().unwrap().len(), 1);
    let raw = ws.json();
    let paragraphs = raw["layout"]["stories"][0]["paragraphs"]
        .as_array()
        .unwrap();
    assert_eq!(paragraphs.len(), 3);
    assert_eq!(paragraphs[1]["runs"][0]["table"], "t");
    ok(&["layout-table", &doc, "to-text", "t"]);
    let raw = ws.json();
    let texts: Vec<String> = raw["layout"]["stories"][0]["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p["runs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["text"].as_str().unwrap_or("<tab>"))
                .collect::<String>()
        })
        .collect();
    assert_eq!(
        texts,
        vec!["intro", "a<tab>b<tab>c", "d<tab>e<tab>", "outro"]
    );
    assert!(raw["layout"]["tables"].as_array().unwrap().is_empty());
}

#[test]
fn placement_is_unique_and_planning_repeats_header_rows() {
    let ws = Workspace::new("plan");
    let doc = book(&ws);
    ok(&[
        "layout-table",
        &doc,
        "create",
        "t",
        "--rows",
        "6",
        "--columns",
        "2",
        "--header-rows",
        "1",
        "--row-height",
        "30",
    ]);
    ok(&[
        "layout-table",
        &doc,
        "place",
        "t",
        "--story",
        "s",
        "--paragraph",
        "0",
        "--at",
        "5",
    ]);
    assert!(fails(&[
        "layout-table",
        &doc,
        "place",
        "t",
        "--story",
        "s",
        "--paragraph",
        "3"
    ])
    .contains("already placed"));
    assert!(fails(&["layout-table", &doc, "delete", "t"]).contains("placed"));
    let r = ok(&[
        "layout-table",
        &doc,
        "plan",
        "t",
        "--heights",
        "100,100,100",
    ]);
    assert_eq!(r["estimated"], true);
    assert_eq!(r["row_heights"].as_array().unwrap().len(), 6);
    let pieces = r["pieces"].as_array().unwrap();
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0]["header_repeated"], false);
    assert_eq!(pieces[1]["header_repeated"], true);
    assert_eq!(r["overset"], false);
    let r = ok(&["layout-table", &doc, "plan", "t", "--heights", "100"]);
    assert_eq!(r["overset"], true);
    let mut raw = ws.json();
    raw["layout"]["tables"][0]["rows"][3] = json!({"height": 400});
    ws.write(&raw);
    let r = ok(&[
        "layout-table",
        &doc,
        "plan",
        "t",
        "--heights",
        "100,100,100,100,100,100",
    ]);
    assert!(r["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["kind"] == "row-split"));
}

#[test]
fn validation_rejects_overlaps_bad_styles_and_unknown_placements() {
    let ws = Workspace::new("validate");
    let doc = book(&ws);
    table(&doc, "t", "2", "2");
    let good = ws.json();
    let touch = |raw: &Value| {
        ws.write(raw);
        fails(&["layout-table", &doc, "set", "t", "--header-rows", "0"])
    };
    let mut raw = good.clone();
    raw["layout"]["tables"][0]["cells"] =
        json!([{"row": 0, "col": 0, "col_span": 2}, {"row": 0, "col": 1}]);
    assert!(touch(&raw).contains("overlap"));
    let mut raw = good.clone();
    raw["layout"]["tables"][0]["style"] = json!("nope");
    assert!(touch(&raw).contains("table style"));
    let mut raw = good.clone();
    raw["layout"]["stories"][0]["paragraphs"][0]["runs"] = json!([{"table": "ghost"}]);
    assert!(touch(&raw).contains("ghost"));
    let mut raw = good;
    raw["layout"]["tables"][0]["cells"] = json!([{"row": 5, "col": 0}]);
    assert!(touch(&raw).contains("outside"));
}

#[test]
fn downgrade_refuses_tables() {
    let ws = Workspace::new("downgrade");
    let doc = book(&ws);
    table(&doc, "t", "1", "1");
    let before = ws.bytes();
    let error = fails(&["migrate", &doc, "--target", "8"]);
    assert!(
        error.contains("unsupported-capability") || error.contains("cannot represent"),
        "{error}"
    );
    assert_eq!(before, ws.bytes());
}

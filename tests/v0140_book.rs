//! v0.14.0 task 11: `.penbook` books (manifest, numbering, synchronization,
//! book-wide TOC/index, preflight, packaging, PDF export, all-or-none writes).
use pentool::color::catalog;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pentool-book-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("chapters")).unwrap();
        Workspace(dir)
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn s(&self, rel: &str) -> String {
        self.p(rel).to_string_lossy().into_owned()
    }
    fn bytes(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.p(rel)).unwrap()
    }
    fn json(&self, rel: &str) -> Value {
        serde_json::from_slice(&self.bytes(rel)).unwrap()
    }
    fn write(&self, rel: &str, raw: &Value) {
        std::fs::write(self.p(rel), serde_json::to_vec_pretty(raw).unwrap()).unwrap();
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

/// A version 9 member with `pages` pages. Page `i` holds heading `<title> i`
/// (style `h1`) and one body line, in one threaded story `s`.
fn member(ws: &Workspace, rel: &str, title: &str, pages: usize) {
    let path = ws.s(rel);
    ok(&["new", &path, "--overwrite"]);
    ok(&["migrate", &path, "--target", "9"]);
    let mut raw = ws.json(rel);
    let mut paragraphs = Vec::new();
    for i in 1..=pages {
        paragraphs.push(json!({"style": "h1", "runs": [{"text": format!("{title} {i}")}]}));
        paragraphs.push(json!({"runs": [{"text": "Water flows."}]}));
    }
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": paragraphs}]);
    raw["layout"]["styles"] = json!({"paragraph": {
        "h1": {"size": 12, "leading": 14},
        "body": {"size": 10, "leading": 12}
    }});
    let template = raw["pages"][0].clone();
    let mut page_list = Vec::new();
    let mut spreads = Vec::new();
    for i in 0..pages {
        let mut page = template.clone();
        let id = format!("page-{}", i + 1);
        page["id"] = json!(id);
        page["name"] = json!(id);
        let next = if i + 1 < pages {
            json!(format!("f{}", i + 2))
        } else {
            Value::Null
        };
        page["layers"][0]["nodes"] = json!([{
            "kind": "text-frame", "id": format!("f{}", i + 1), "story": "s", "next": next,
            "x": 0, "y": 0, "width": 300, "height": 40
        }]);
        page_list.push(page);
        spreads.push(json!({"id": format!("sp-{}", i + 1), "pages": [id], "facing": false}));
    }
    raw["pages"] = json!(page_list);
    raw["layout"]["spreads"] = json!(spreads);
    ws.write(rel, &raw);
}

fn manifest(ws: &Workspace, numbering: Value, sync: Value) -> String {
    let raw = json!({
        "format": "penbook", "version": 1, "name": "Guide",
        "documents": [
            {"path": "01-intro.pen", "role": "chapter"},
            {"path": "chapters/02-body.pen", "role": "chapter"}
        ],
        "numbering": numbering,
        "synchronize": sync,
    });
    ws.write("guide.penbook", &raw);
    ws.s("guide.penbook")
}

fn standard(ws: &Workspace) -> String {
    member(ws, "01-intro.pen", "Intro", 3);
    member(ws, "chapters/02-body.pen", "Body", 2);
    manifest(
        ws,
        json!({"page": "continuous", "chapter": "continuous", "footnote": "continuous"}),
        json!({"source": "01-intro.pen", "styles": true, "swatches": true, "parents": false}),
    )
}

fn labels(doc: &str) -> Vec<String> {
    let info = ok(&["section", doc, "list"]);
    info["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["label"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn init_add_move_remove_and_refusals() {
    let ws = Workspace::new("manifest");
    let book = ws.s("new.penbook");
    ok(&[
        "book", &book, "init", "--name", "New", "--member", "a.pen", "--member", "b.pen",
    ]);
    let raw = ws.json("new.penbook");
    assert_eq!(raw["format"], "penbook");
    assert_eq!(raw["documents"].as_array().unwrap().len(), 2);
    assert!(
        fails(&["book", &book, "init", "--name", "x", "--member", "a.pen"])
            .contains("already exists")
    );
    ok(&["book", &book, "add", "c.pen", "--at", "0"]);
    assert_eq!(ws.json("new.penbook")["documents"][0]["path"], "c.pen");
    ok(&["book", &book, "move", "c.pen", "--to", "2"]);
    assert_eq!(ws.json("new.penbook")["documents"][2]["path"], "c.pen");
    ok(&["book", &book, "remove", "c.pen"]);
    assert_eq!(
        ws.json("new.penbook")["documents"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let before = ws.bytes("new.penbook");
    for bad in [
        "../outside.pen",
        "/abs.pen",
        "C:/x.pen",
        "a\\b.pen",
        "notes.txt",
    ] {
        let error = fails(&["book", &book, "add", bad]);
        assert!(
            error.contains("unsafe-path") || error.contains("invalid-input"),
            "{bad}: {error}"
        );
    }
    assert!(fails(&["book", &book, "add", "a.pen"]).contains("listed twice"));
    assert!(fails(&["book", &book, "remove", "ghost.pen"]).contains("missing-resource"));
    assert_eq!(ws.bytes("new.penbook"), before, "refusals must not write");
}

#[test]
fn hand_written_escape_paths_are_rejected() {
    let ws = Workspace::new("escape");
    member(&ws, "01-intro.pen", "Intro", 1);
    ws.write(
        "bad.penbook",
        &json!({"format": "penbook", "version": 1, "name": "x",
                "documents": [{"path": "01-intro.pen"}, {"path": "../secret.pen"}]}),
    );
    let error = fails(&["book", &ws.s("bad.penbook"), "status"]);
    assert!(error.contains("unsafe-path"), "{error}");
    ws.write(
        "bad.penbook",
        &json!({"format": "penbook", "version": 2, "name": "x",
                "documents": [{"path": "01-intro.pen"}]}),
    );
    assert!(fails(&["book", &ws.s("bad.penbook"), "status"]).contains("unsupported-version"));
}

#[test]
fn status_reports_members_pages_and_missing_files() {
    let ws = Workspace::new("status");
    let book = standard(&ws);
    let status = ok(&["book", &book, "status"]);
    assert_eq!(status["documents"], 2);
    assert_eq!(status["pages"], 5);
    assert_eq!(status["ok"], true);
    assert_eq!(status["members"][1]["first_label"], "1");
    std::fs::remove_file(ws.p("chapters/02-body.pen")).unwrap();
    let status = ok(&["book", &book, "status"]);
    assert_eq!(status["ok"], false);
    assert_eq!(status["missing"], 1);
}

#[test]
fn renumber_continues_pages_chapters_and_footnotes_and_is_idempotent() {
    let ws = Workspace::new("renumber");
    let book = standard(&ws);
    let second = ws.s("chapters/02-body.pen");
    let first_before = ws.bytes("01-intro.pen");

    let dry = ok(&["book", &book, "renumber", "--dry-run"]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(labels(&second), ["1", "2"], "dry run must not write");

    let done = ok(&["book", &book, "renumber"]);
    assert_eq!(done["changed"], true);
    assert_eq!(done["members"][1]["first_label"], "4");
    assert_eq!(done["members"][1]["last_label"], "5");
    assert_eq!(done["members"][1]["chapter_offset"], 1);
    assert_eq!(labels(&second), ["4", "5"]);
    assert_eq!(
        ws.bytes("01-intro.pen"),
        first_before,
        "first member keeps its numbers"
    );
    assert_eq!(
        ws.json("chapters/02-body.pen")["layout"]["book_numbering"]["chapter_offset"],
        1
    );

    let again = ok(&["book", &book, "renumber"]);
    assert_eq!(again["changed"], false);
    assert_eq!(again["written"], json!([]));

    // Restart-each puts the second member back at 1 and drops the offsets.
    ws.write(
        "guide.penbook",
        &json!({"format": "penbook", "version": 1, "name": "Guide",
                "documents": [{"path": "01-intro.pen"}, {"path": "chapters/02-body.pen"}],
                "numbering": {"page": "restart-each", "chapter": "restart-each", "footnote": "per-document"}}),
    );
    ok(&["book", &book, "renumber"]);
    assert_eq!(labels(&second), ["1", "2"]);
    assert!(ws.json("chapters/02-body.pen")["layout"]
        .get("book_numbering")
        .is_none());
}

#[test]
fn missing_or_old_members_abort_before_any_write() {
    let ws = Workspace::new("allornone");
    let book = standard(&ws);
    let a = ws.bytes("01-intro.pen");
    let b = ws.bytes("chapters/02-body.pen");
    // A version 8 member cannot take layout operations.
    let old = ws.s("chapters/02-body.pen");
    ok(&["new", &old, "--overwrite"]);
    let old_bytes = ws.bytes("chapters/02-body.pen");
    let error = fails(&["book", &book, "renumber"]);
    assert!(error.contains("version 9"), "{error}");
    assert_eq!(ws.bytes("01-intro.pen"), a);
    assert_eq!(ws.bytes("chapters/02-body.pen"), old_bytes);
    // A missing member aborts with the full list.
    std::fs::remove_file(ws.p("chapters/02-body.pen")).unwrap();
    let error = fails(&["book", &book, "sync"]);
    assert!(
        error.contains("missing") && error.contains("nothing was written"),
        "{error}"
    );
    assert_eq!(ws.bytes("01-intro.pen"), a);
    drop(b);
}

#[test]
fn interrupted_commit_is_rolled_back_by_the_next_command() {
    let ws = Workspace::new("journal");
    let book = standard(&ws);
    let before = ws.bytes("chapters/02-body.pen");
    // Simulate a crash after the journal was written and one member changed.
    let dir = ws.p(".guide.penbook.journal");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("0.before"), &before).unwrap();
    std::fs::write(
        dir.join("journal.json"),
        serde_json::to_vec(&json!({"operation": "book-renumber", "members": [
            {"path": "chapters/02-body.pen", "before": "0.before"}]}))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(ws.p("chapters/02-body.pen"), b"{\"half\": \"written\"").unwrap();
    let status = ok(&["book", &book, "status"]);
    assert_eq!(status["recovered_members"], json!(["chapters/02-body.pen"]));
    assert_eq!(ws.bytes("chapters/02-body.pen"), before);
    assert!(!dir.exists());
}

#[test]
fn sync_copies_styles_swatches_and_reports_local_replacements() {
    let ws = Workspace::new("sync");
    let book = standard(&ws);
    let mut source = ws.json("01-intro.pen");
    source["layout"]["styles"]["paragraph"]["quote"] = json!({"size": 11, "leading": 13});
    source["layout"]["styles"]["paragraph"]["h1"] = json!({"size": 20, "leading": 24});
    source["styles"] = json!({"brand": {"type": "color", "value": "#ff0000"}});
    ws.write("01-intro.pen", &source);
    let mut local = ws.json("chapters/02-body.pen");
    local["layout"]["styles"]["paragraph"]["mine"] = json!({"size": 9, "leading": 11});
    ws.write("chapters/02-body.pen", &local);
    let source_bytes = ws.bytes("01-intro.pen");

    let status = ok(&["book", &book, "status"]);
    assert_eq!(status["members"][1]["drift"]["styles_missing"], 1);
    assert_eq!(status["members"][1]["drift"]["styles_different"], 1);

    let dry = ok(&["book", &book, "sync", "--dry-run"]);
    assert_eq!(dry["members"][0]["added"], 2);
    assert_eq!(dry["members"][0]["replaced_local"], 1);
    assert_eq!(
        ws.json("chapters/02-body.pen")["layout"]["styles"]["paragraph"]["h1"]["size"],
        12
    );

    let done = ok(&["book", &book, "sync"]);
    assert_eq!(done["changed"], true);
    let details = done["members"][0]["details"].as_array().unwrap();
    assert!(details
        .iter()
        .any(|d| d["id"] == "h1" && d["change"] == "replaced-local"));
    let target = ws.json("chapters/02-body.pen");
    assert_eq!(target["layout"]["styles"]["paragraph"]["h1"]["size"], 20);
    assert_eq!(target["layout"]["styles"]["paragraph"]["quote"]["size"], 11);
    assert_eq!(
        target["layout"]["styles"]["paragraph"]["mine"]["size"], 9,
        "local-only styles survive"
    );
    assert_eq!(target["styles"]["brand"]["value"], "#ff0000");
    assert_eq!(
        ws.bytes("01-intro.pen"),
        source_bytes,
        "the source is untouched"
    );
    assert_eq!(ok(&["book", &book, "sync"])["changed"], false);
    ok(&["tree", &ws.s("chapters/02-body.pen"), "--limit", "1"]);
}

#[test]
fn book_toc_lists_every_member_with_continuous_labels() {
    let ws = Workspace::new("toc");
    let book = standard(&ws);
    let before = ok(&["book", &book, "toc", "--style", "h1", "--dry-run"]);
    assert_eq!(before["entries"], 5);
    assert_eq!(before["numbering_continuous"], false);
    ok(&["book", &book, "renumber"]);
    let done = ok(&["book", &book, "toc", "--style", "h1"]);
    assert_eq!(done["entries"], 5);
    assert_eq!(done["numbering_continuous"], true);
    assert_eq!(done["into"], "01-intro.pen");
    let raw = ws.json("01-intro.pen");
    let story = raw["layout"]["stories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "book-toc-story")
        .unwrap();
    let rows: Vec<String> = story["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p["runs"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|r| r["text"].as_str())
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    assert_eq!(
        rows,
        [
            "Intro 1|1",
            "Intro 2|2",
            "Intro 3|3",
            "Body 1|4",
            "Body 2|5"
        ]
    );
    let record = raw["layout"]["generated"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "book-toc")
        .unwrap();
    assert_eq!(record["options"]["scope"], "book");
    // The member's own generator refuses a record it cannot recompute alone.
    let error = fails(&["layout-doc", &ws.s("01-intro.pen"), "generate", "book-toc"]);
    assert!(error.contains("book-wide"), "{error}");
    // Regenerating is stable.
    let bytes = ws.bytes("01-intro.pen");
    assert_eq!(
        ok(&["book", &book, "toc", "--style", "h1"])["changed"],
        false
    );
    assert_eq!(ws.bytes("01-intro.pen"), bytes);
}

#[test]
fn book_index_merges_topics_and_uses_book_page_positions() {
    let ws = Workspace::new("index");
    let book = standard(&ws);
    ok(&["book", &book, "renumber"]);
    for rel in ["01-intro.pen", "chapters/02-body.pen"] {
        let doc = ws.s(rel);
        ok(&[
            "layout-doc",
            &doc,
            "index-topic",
            "water",
            "--text",
            "Water",
        ]);
    }
    // Mark the first body paragraph of intro page 1 and of body page 2.
    ok(&[
        "layout-doc",
        &ws.s("01-intro.pen"),
        "index-mark",
        "water",
        "--story",
        "s",
        "--paragraph",
        "1",
    ]);
    ok(&[
        "layout-doc",
        &ws.s("chapters/02-body.pen"),
        "index-mark",
        "water",
        "--story",
        "s",
        "--paragraph",
        "3",
    ]);
    let done = ok(&["book", &book, "index", "--rule", "all"]);
    assert_eq!(done["entries"], 1);
    let raw = ws.json("01-intro.pen");
    let story = raw["layout"]["stories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "book-index-story")
        .unwrap();
    let text = story["paragraphs"].to_string();
    assert!(text.contains("Water, 1, 5"), "{text}");
}

#[test]
fn preflight_flags_missing_members_and_overset_text() {
    let ws = Workspace::new("preflight");
    let book = standard(&ws);
    let clean = ok(&["book", &book, "preflight"]);
    assert_eq!(clean["ok"], true, "{clean}");
    assert_eq!(clean["mutated"], false);
    let mut long = ws.json("chapters/02-body.pen");
    long["layout"]["stories"][0]["paragraphs"] = json!((0..40)
        .map(|i| json!({"runs": [{"text": format!("A long line of text number {i} that overflows.")}]}))
        .collect::<Vec<_>>());
    ws.write("chapters/02-body.pen", &long);
    let report = ok(&["book", &book, "preflight"]);
    assert_eq!(report["ok"], false);
    assert!(report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "text-overset" && i["member"] == "chapters/02-body.pen"));
    std::fs::remove_file(ws.p("01-intro.pen")).unwrap();
    let report = ok(&["book", &book, "preflight"]);
    assert!(report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "member-missing"));
}

#[test]
fn package_collects_manifest_and_members_into_a_new_directory() {
    let ws = Workspace::new("package");
    let book = standard(&ws);
    let out = ws.s("pkg");
    let report = ok(&["book", &book, "package", &out]);
    assert_eq!(report["members"], 2);
    assert!(ws.p("pkg/guide.penbook").exists());
    assert!(ws.p("pkg/01-intro.pen").exists());
    assert!(ws.p("pkg/chapters/02-body.pen").exists());
    assert!(fails(&["book", &book, "package", &out]).contains("output-exists"));
    // The packaged book is itself a valid book.
    let status = ok(&["book", &ws.s("pkg/guide.penbook"), "status"]);
    assert_eq!(status["ok"], true);
}

fn print_member(ws: &Workspace, rel: &str, color: [f64; 3]) {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#ffffffff"});
    catalog::import(
        &mut raw,
        &ws.p(rel),
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"paint","x":0,"y":0,
        "width":200,"height":100,
        "fill":{"kind":"solid","color":{"space":"srgb","channels":color}}}]);
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":20,"y":10,"width":160,"height":80},"bleed":5});
    ws.write(rel, &raw);
}

#[test]
fn export_writes_one_deterministic_multi_page_pdf() {
    let ws = Workspace::new("export");
    print_member(&ws, "a.pen", [0.9, 0.2, 0.1]);
    print_member(&ws, "b.pen", [0.1, 0.3, 0.8]);
    ws.write(
        "print.penbook",
        &json!({"format": "penbook", "version": 1, "name": "Print",
                "documents": [{"path": "a.pen"}, {"path": "b.pen"}]}),
    );
    let book = ws.s("print.penbook");
    let out = ws.s("book.pdf");
    let again = ws.s("book2.pdf");
    let args = |o: &str| {
        vec![
            "book".to_string(),
            book.clone(),
            "export".into(),
            o.to_string(),
            "--ppi".into(),
            "72".into(),
            "--created".into(),
            "2026-10-10T12:30:00Z".into(),
        ]
    };
    let a = args(&out);
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    let report = ok(&refs);
    let b = args(&again);
    let refs: Vec<&str> = b.iter().map(String::as_str).collect();
    ok(&refs);
    assert_eq!(report["page_count"], 2);
    assert_eq!(report["scope"], "pdfx-4-flattened-cmyk-book-export");
    assert_eq!(report["independent_validation"]["status"], "not-run");
    let bytes = ws.bytes("book.pdf");
    assert_eq!(bytes, ws.bytes("book2.pdf"));
    pentool::color::pdfx::verify_structure(&bytes).unwrap();
    assert!(bytes.windows(b"/Count 2".len()).any(|w| w == b"/Count 2"));
    // Existing output requires --overwrite; EPUB export needs version 9 members.
    assert!(fails(&a.iter().map(String::as_str).collect::<Vec<_>>()).contains("--overwrite"));
    let epub = fails(&["book", &book, "export", &ws.s("b.epub"), "--format", "epub"]);
    assert!(epub.contains("version 9"), "{epub}");
    assert!(!Path::new(&ws.s("b.epub")).exists());
    // Missing members abort the export instead of skipping.
    std::fs::remove_file(ws.p("b.pen")).unwrap();
    let error = fails(&["book", &book, "export", &ws.s("c.pdf")]);
    assert!(error.contains("nothing was written"), "{error}");
    assert!(!ws.p("c.pdf").exists());
}

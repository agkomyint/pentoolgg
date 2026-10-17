//! v0.14.0 task 10: long-document features (notes, cross-references,
//! hyperlinks, variables, conditional text, generated stories).
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-longdoc-{name}-{}", std::process::id()));
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

/// Three pages, one frame each, one threaded story `s`:
/// page 1 "Rivers", page 2 "Lakes", page 3 "Åland" (all headings in `h1`).
fn book(ws: &Workspace) -> String {
    let doc = ws.doc();
    ok(&["new", &doc]);
    ok(&["migrate", &doc, "--target", "9"]);
    let mut raw = ws.json();
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": [
        {"style": "h1", "runs": [{"text": "Rivers"}]},
        {"runs": [{"text": "Water flows downhill and carries history along the valley."}]},
        {"style": "h1", "runs": [{"text": "Lakes"}]},
        {"runs": [{"text": "Still water."}]},
        {"style": "h1", "runs": [{"text": "\u{c5}land"}]},
        {"runs": [{"text": "Zebras."}]}
    ]}]);
    raw["layout"]["styles"] = json!({"paragraph": {
        "h1": {"size": 12, "leading": 14},
        "body": {"size": 10, "leading": 12}
    }});
    let template = raw["pages"][0].clone();
    let mut pages = Vec::new();
    let mut spreads = Vec::new();
    for i in 0..3 {
        let mut page = template.clone();
        let id = format!("page-{}", i + 1);
        page["id"] = json!(id);
        page["name"] = json!(id);
        let next = if i < 2 {
            json!(format!("f{}", i + 2))
        } else {
            Value::Null
        };
        page["layers"][0]["nodes"] = json!([{
            "kind": "text-frame", "id": format!("f{}", i + 1), "story": "s", "next": next,
            "x": 0, "y": 0, "width": 300, "height": 40
        }]);
        pages.push(page);
        spreads.push(json!({"id": format!("sp-{}", i + 1), "pages": [id], "facing": false}));
    }
    raw["pages"] = json!(pages);
    raw["layout"]["spreads"] = json!(spreads);
    ws.write(&raw);
    doc
}

fn story_text(ws: &Workspace, story: &str) -> Vec<String> {
    let raw = ws.json();
    raw["layout"]["stories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == story)
        .unwrap()["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p["runs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| match r["text"].as_str() {
                    Some(t) => t.to_string(),
                    None => " ".to_string(),
                })
                .collect::<String>()
        })
        .collect()
}

fn resolved(doc: &str, story: &str) -> Vec<String> {
    let result = ok(&["layout-doc", doc, "text", story]);
    result["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["text"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn footnotes_number_by_rule_and_report_fit() {
    let ws = Workspace::new("notes");
    let doc = book(&ws);
    for (id, paragraph) in [("n1", "0"), ("n2", "2"), ("n3", "4")] {
        ok(&[
            "layout-doc",
            &doc,
            "note-add",
            id,
            "--story",
            "s",
            "--paragraph",
            paragraph,
            "--at",
            "3",
            "--text",
            "A short note.",
        ]);
    }
    let labels = |ws: &Workspace| -> Vec<String> {
        ok(&["layout-doc", &ws.doc(), "note-list"])["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["label"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(labels(&ws), ["1", "2", "3"]);
    ok(&[
        "layout-doc",
        &doc,
        "note-settings",
        "--restart",
        "page",
        "--style",
        "lower-roman",
    ]);
    assert_eq!(
        labels(&ws),
        ["i", "i", "i"],
        "each note is alone on its page"
    );
    ok(&["layout-doc", &doc, "note-settings", "--restart", "story"]);
    assert_eq!(labels(&ws), ["i", "ii", "iii"]);
    let plan = ok(&["layout-doc", &doc, "note-plan"]);
    assert_eq!(plan["estimated"], true);
    assert_eq!(plan["summary"]["notes"], 3);
    assert_eq!(plan["summary"]["fit"], 3, "{plan}");
    assert_eq!(plan["summary"]["moved"], 0);

    // A note taller than the footnote area splits and the page is flagged.
    let long = "word ".repeat(400);
    ok(&[
        "layout-doc",
        &doc,
        "note-add",
        "big",
        "--story",
        "s",
        "--paragraph",
        "0",
        "--at",
        "0",
        "--text",
        &long,
    ]);
    let plan = ok(&["layout-doc", &doc, "note-plan"]);
    let big = plan["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "big")
        .unwrap();
    assert_eq!(big["status"], "splits");
    assert_eq!(plan["pages"][0]["over_budget"], true);
    assert_eq!(plan["summary"]["split"], 1);

    // Paginated listing describes exactly the emitted slice.
    let listed = ok(&[
        "layout-doc",
        &doc,
        "note-list",
        "--limit",
        "2",
        "--offset",
        "1",
    ]);
    assert_eq!(listed["matches"], 4);
    assert_eq!(listed["returned"], 2);
    assert_eq!(listed["has_more"], true);
    assert_eq!(listed["offset"], 1);
}

#[test]
fn endnotes_collect_into_a_generated_story_deterministically() {
    let ws = Workspace::new("endnotes");
    let doc = book(&ws);
    for (id, paragraph, text) in [("e1", "1", "Source A"), ("e2", "3", "Source B")] {
        ok(&[
            "layout-doc",
            &doc,
            "note-add",
            id,
            "--story",
            "s",
            "--paragraph",
            paragraph,
            "--text",
            text,
            "--kind",
            "endnote",
        ]);
    }
    ok(&[
        "layout-doc",
        &doc,
        "generated-add",
        "end",
        "--kind",
        "endnotes",
        "--story",
        "endnotes",
    ]);
    let first = ok(&["layout-doc", &doc, "generate", "end"]);
    assert_eq!(first["entries"], 2);
    assert_eq!(story_text(&ws, "endnotes"), ["1. Source A", "2. Source B"]);
    let before = ws.bytes();
    let second = ok(&["layout-doc", &doc, "generate", "end"]);
    assert_eq!(first["digest"], second["digest"]);
    assert_eq!(before, ws.bytes(), "regeneration is byte-identical");
    let state = ok(&["layout-doc", &doc, "generated-list"]);
    assert_eq!(state["stale"], 0);
    // The body stays editable: a footnote may not be an endnote story target.
    let err = fails(&[
        "layout-doc",
        &doc,
        "note-add",
        "e1",
        "--story",
        "s",
        "--paragraph",
        "0",
        "--text",
        "again",
    ]);
    assert!(err.contains("already exists"), "{err}");
}

#[test]
fn cross_references_resolve_and_broken_targets_are_reported() {
    let ws = Workspace::new("xrefs");
    let doc = book(&ws);
    ok(&[
        "layout-doc",
        &doc,
        "bookmark-add",
        "lakes",
        "--story",
        "s",
        "--paragraph",
        "2",
    ]);
    for (id, format, template) in [
        ("x-page", "page", None),
        ("x-text", "paragraph-text", None),
        ("x-num", "paragraph-number", None),
        ("x-chapter", "chapter", None),
        ("x-custom", "custom", Some("{text} on page {page}")),
    ] {
        let mut args = vec![
            "layout-doc",
            doc.as_str(),
            "xref-add",
            id,
            "--story",
            "s",
            "--paragraph",
            "5",
            "--target-bookmark",
            "lakes",
            "--format",
            format,
        ];
        if let Some(t) = template {
            args.extend(["--template", t]);
        }
        ok(&args);
    }
    let listed = ok(&["layout-doc", &doc, "xref-list"]);
    let text = |id: &str| -> String {
        listed["xrefs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id)
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(text("x-page"), "2");
    assert_eq!(text("x-text"), "Lakes");
    assert_eq!(text("x-num"), "2");
    assert_eq!(text("x-chapter"), "1");
    assert_eq!(text("x-custom"), "Lakes on page 2");
    assert_eq!(listed["estimated"], true);
    assert!(
        resolved(&doc, "s")[5].starts_with("Zebras."),
        "resolved text keeps the paragraph text"
    );

    // Removing the bookmark target breaks the reference: it shows `?`, is
    // reported, and nothing is silently dropped.
    let raw = {
        let mut raw = ws.json();
        raw["layout"]["bookmarks"] = json!([]);
        raw
    };
    ws.write(&raw);
    let listed = ok(&["layout-doc", &doc, "xref-list"]);
    let first = &listed["xrefs"][0];
    assert_eq!(first["status"], "broken");
    assert_eq!(first["text"], "?");
    let report = ok(&["layout-doc", &doc, "report"]);
    assert_eq!(report["ok"], false);
    assert!(report["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "xref-unresolved"));
}

#[test]
fn xref_and_link_add_validate_the_source_story_before_the_target() {
    let ws = Workspace::new("order");
    let doc = book(&ws);
    // Both ends bad: the source story is the shared precondition, so it must
    // be reported first (fixing the destination would only reveal a second
    // failure). Mirrors note-add/bookmark-add, and issue #170.
    let xref = fails(&[
        "layout-doc",
        &doc,
        "xref-add",
        "x1",
        "--story",
        "nope",
        "--paragraph",
        "99",
        "--target-page",
        "missing",
    ]);
    assert!(xref.contains("story not found: nope"), "{xref}");
    assert!(!xref.contains("page not found"), "{xref}");
    let para = fails(&[
        "layout-doc",
        &doc,
        "xref-add",
        "x2",
        "--story",
        "s",
        "--paragraph",
        "99",
        "--target-page",
        "missing",
    ]);
    assert!(para.contains("paragraph 99 does not exist"), "{para}");
    assert!(!para.contains("page not found"), "{para}");
    let link = fails(&[
        "layout-doc",
        &doc,
        "link-add",
        "l1",
        "--story",
        "nope",
        "--paragraph",
        "0",
        "--page",
        "missing",
        "--text",
        "x",
    ]);
    assert!(link.contains("story not found: nope"), "{link}");
    assert!(!link.contains("page not found"), "{link}");
    // With a valid source, the destination error still surfaces.
    let target = fails(&[
        "layout-doc",
        &doc,
        "xref-add",
        "x3",
        "--story",
        "s",
        "--paragraph",
        "0",
        "--target-page",
        "missing",
    ]);
    assert!(target.contains("page not found"), "{target}");
}

#[test]
fn links_refuse_unsafe_schemes_and_keep_text_when_removed() {
    let ws = Workspace::new("links");
    let doc = book(&ws);
    let err = fails(&[
        "layout-doc",
        &doc,
        "link-add",
        "bad",
        "--story",
        "s",
        "--paragraph",
        "1",
        "--text",
        "click",
        "--url",
        "javascript:alert(1)",
    ]);
    assert!(err.contains("not allowed"), "{err}");
    ok(&[
        "layout-doc",
        &doc,
        "link-add",
        "good",
        "--story",
        "s",
        "--paragraph",
        "1",
        "--text",
        "[site]",
        "--url",
        "https://example.com/a",
    ]);
    let listed = ok(&["layout-doc", &doc, "link-list"]);
    assert_eq!(listed["matches"], 1);
    let removed = ok(&["layout-doc", &doc, "link-remove", "good"]);
    assert_eq!(removed["runs_unwrapped"], 1);
    assert!(story_text(&ws, "s")[1].contains("[site]"));
}

#[test]
fn variables_resolve_without_reading_the_clock() {
    let ws = Workspace::new("variables");
    let doc = book(&ws);
    ok(&[
        "layout-doc",
        &doc,
        "variable-define",
        "today",
        "--kind",
        "output-date",
        "--format",
        "DD MMM YYYY",
    ]);
    let before = ws.bytes();
    let listed = ok(&["layout-doc", &doc, "variable-list"]);
    assert!(listed["variables"][0].get("unresolved").is_some());
    let listed = ok(&["layout-doc", &doc, "variable-list", "--date", "2026-03-04"]);
    assert_eq!(listed["variables"][0]["current"], "04 Mar 2026");
    assert_eq!(
        before,
        ws.bytes(),
        "read-only commands leave the file alone"
    );

    ok(&[
        "layout-doc",
        &doc,
        "variable-define",
        "head",
        "--kind",
        "running-header",
        "--style",
        "h1",
        "--pick",
        "first",
    ]);
    ok(&[
        "layout-doc",
        &doc,
        "variable-insert",
        "head",
        "--story",
        "s",
        "--paragraph",
        "1",
    ]);
    let text = resolved(&doc, "s");
    assert!(text[1].contains("Rivers"), "{:?}", text[1]);
}

#[test]
fn conditions_hide_text_and_sets_switch_it() {
    let ws = Workspace::new("conditions");
    let doc = book(&ws);
    ok(&["layout-doc", &doc, "condition-define", "print"]);
    ok(&["layout-doc", &doc, "condition-define", "web"]);
    ok(&[
        "layout-doc",
        &doc,
        "condition-wrap",
        "--story",
        "s",
        "--paragraph",
        "3",
        "--condition",
        "web",
        "--text",
        " (see site)",
    ]);
    ok(&[
        "layout-doc",
        &doc,
        "condition-set-define",
        "for-print",
        "--hide",
        "web",
        "--show",
        "print",
    ]);
    assert!(resolved(&doc, "s")[3].contains("(see site)"));
    ok(&["layout-doc", &doc, "condition-activate", "for-print"]);
    assert!(!resolved(&doc, "s")[3].contains("(see site)"));
    ok(&["layout-doc", &doc, "condition-activate", "--none"]);
    assert!(resolved(&doc, "s")[3].contains("(see site)"));
    let err = fails(&["layout-doc", &doc, "condition-remove", "web"]);
    assert!(err.contains("web"), "{err}");
}

#[test]
fn toc_and_index_generate_and_report_stale_state() {
    let ws = Workspace::new("generated");
    let doc = book(&ws);
    ok(&[
        "layout-doc",
        &doc,
        "generated-add",
        "toc",
        "--kind",
        "toc",
        "--story",
        "toc-story",
        "--styles",
        "h1",
    ]);
    let result = ok(&["layout-doc", &doc, "generate", "toc"]);
    assert_eq!(result["entries"], 3);
    let toc = story_text(&ws, "toc-story");
    assert_eq!(toc.len(), 3);
    assert!(
        toc[0].starts_with("Rivers") && toc[0].ends_with('1'),
        "{toc:?}"
    );
    assert!(
        toc[2].starts_with("\u{c5}land") && toc[2].ends_with('3'),
        "{toc:?}"
    );
    assert_eq!(ok(&["layout-doc", &doc, "generated-list"])["stale"], 0);

    // Editing a heading makes the TOC stale; check records it; generate
    // clears it and a second generate is byte-identical.
    let mut raw = ws.json();
    let stories = raw["layout"]["stories"].as_array_mut().unwrap();
    let s = stories.iter_mut().find(|s| s["id"] == "s").unwrap();
    s["paragraphs"][2]["runs"][0]["text"] = json!("Ponds");
    ws.write(&raw);
    let state = ok(&["layout-doc", &doc, "generated-check"]);
    assert_eq!(state["stale"], 1);
    assert_eq!(state["generated"][0]["reason"], "source-changed");
    assert_eq!(ws.json()["layout"]["generated"][0]["stale"], true);
    ok(&["layout-doc", &doc, "generate", "toc"]);
    assert!(story_text(&ws, "toc-story")[1].starts_with("Ponds"));
    assert_eq!(ws.json()["layout"]["generated"][0].get("stale"), None);
    let before = ws.bytes();
    ok(&["layout-doc", &doc, "generate", "toc"]);
    assert_eq!(before, ws.bytes());

    // A hand edit of the generated story is detected.
    let mut raw = ws.json();
    let stories = raw["layout"]["stories"].as_array_mut().unwrap();
    let t = stories.iter_mut().find(|s| s["id"] == "toc-story").unwrap();
    t["paragraphs"][0]["runs"][0]["text"] = json!("hacked");
    ws.write(&raw);
    let state = ok(&["layout-doc", &doc, "generated-list"]);
    assert_eq!(state["generated"][0]["reason"], "edited");
}

#[test]
fn index_sorts_by_locale_and_merges_page_ranges_and_see_references() {
    let ws = Workspace::new("index");
    let doc = book(&ws);
    for (id, text) in [("zeb", "Zebra"), ("aal", "\u{c5}land"), ("riv", "River")] {
        ok(&["layout-doc", &doc, "index-topic", id, "--text", text]);
    }
    ok(&[
        "layout-doc",
        &doc,
        "index-topic",
        "lake",
        "--text",
        "Lake",
        "--see",
        "riv",
    ]);
    for (topic, paragraph) in [("riv", "1"), ("riv", "3"), ("zeb", "4"), ("aal", "2")] {
        ok(&[
            "layout-doc",
            &doc,
            "index-mark",
            topic,
            "--story",
            "s",
            "--paragraph",
            paragraph,
        ]);
    }
    ok(&[
        "layout-doc",
        &doc,
        "generated-add",
        "ix",
        "--kind",
        "index",
        "--story",
        "ix-story",
        "--language",
        "sv",
        "--page-range",
        "contiguous",
    ]);
    ok(&["layout-doc", &doc, "generate", "ix"]);
    let text = story_text(&ws, "ix-story");
    let flat = text.join("|");
    let pos = |needle: &str| {
        flat.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {flat}"))
    };
    assert!(
        pos("Zebra") < pos("\u{c5}land"),
        "sv sorts å after z: {flat}"
    );
    assert!(flat.contains("River"), "{flat}");
    assert!(flat.contains("See River"), "see reference rendered: {flat}");
    assert!(
        flat.contains("River, 2\u{2013}3"),
        "contiguous range: {flat}"
    );
    let before = ws.bytes();
    ok(&["layout-doc", &doc, "generate", "ix"]);
    assert_eq!(before, ws.bytes());
}

#[test]
fn captions_come_from_metadata() {
    let ws = Workspace::new("captions");
    let doc = book(&ws);
    let mut raw = ws.json();
    raw["pages"][1]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "kind": "rect", "id": "pic", "x": 0, "y": 50, "width": 20, "height": 20,
            "caption": "A calm lake"
        }));
    ws.write(&raw);
    ok(&[
        "layout-doc",
        &doc,
        "generated-add",
        "lof",
        "--kind",
        "list-of-figures",
        "--story",
        "lof-story",
        "--label",
        "Figure",
        "--source",
        "metadata",
    ]);
    let result = ok(&["layout-doc", &doc, "generate", "lof"]);
    assert_eq!(result["entries"], 1);
    let text = story_text(&ws, "lof-story");
    assert!(text[0].contains("A calm lake"), "{text:?}");
    assert!(text[0].contains("Figure"), "{text:?}");
}

#[test]
fn invalid_mutations_leave_the_document_byte_identical() {
    let ws = Workspace::new("atomic");
    let doc = book(&ws);
    ok(&[
        "layout-doc",
        &doc,
        "note-add",
        "n1",
        "--story",
        "s",
        "--paragraph",
        "1",
        "--text",
        "x",
    ]);
    let before = ws.bytes();
    for args in [
        vec![
            "layout-doc",
            &doc,
            "note-add",
            "n1",
            "--story",
            "s",
            "--paragraph",
            "1",
            "--text",
            "dup",
        ],
        vec![
            "layout-doc",
            &doc,
            "note-add",
            "n2",
            "--story",
            "nope",
            "--paragraph",
            "0",
            "--text",
            "x",
        ],
        vec!["layout-doc", &doc, "note-settings", "--restart", "weekly"],
        vec![
            "layout-doc",
            &doc,
            "xref-add",
            "x",
            "--story",
            "s",
            "--paragraph",
            "0",
            "--target-page",
            "missing",
        ],
    ] {
        let args: Vec<&str> = args.iter().map(|a| a as &str).collect();
        fails(&args);
        assert_eq!(before, ws.bytes(), "{args:?} mutated the file");
    }
    // dry-run reports but does not write.
    ok(&[
        "layout-doc",
        &doc,
        "--dry-run",
        "note-add",
        "n9",
        "--story",
        "s",
        "--paragraph",
        "0",
        "--text",
        "x",
    ]);
    assert_eq!(before, ws.bytes());
}

//! v0.14.0 task 3: stories, threaded frames, frame options, overset, auto-flow.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-stories-{name}-{}", std::process::id()));
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
    fn write(&self, value: &Value) {
        std::fs::write(
            self.0.join("doc.pen"),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
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

/// A v9 document with `count` 300x200 pages (page-1, p2, ...) and 20pt margins.
fn book(ws: &Workspace, count: usize) -> String {
    let doc = ws.doc();
    ok(&["new", &doc, "--width", "300", "--height", "200"]);
    for n in 2..=count {
        ok(&["page", &doc, "add", &format!("p{n}")]);
    }
    ok(&["migrate", &doc, "--target", "9"]);
    ok(&[
        "parent",
        &doc,
        "create",
        "A",
        "--margins",
        "20,20,20,20",
        "--columns",
        "1",
    ]);
    for page in 1..=count {
        let id = if page == 1 {
            "page-1".to_string()
        } else {
            format!("p{page}")
        };
        ok(&["parent", &doc, "assign", &id, "--parent", "A"]);
    }
    doc
}

fn long_text(lines: usize) -> String {
    (0..lines)
        .map(|n| {
            format!("Paragraph {n} of a story long enough to wrap across several lines of text.")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn story(doc: &str, id: &str, text: &str) {
    ok(&["text", doc, "story", "add", id, "--text", text]);
}

fn frame(doc: &str, id: &str, story: &str, page: &str, extra: &[&str]) -> Value {
    let mut args = vec![
        "text", doc, "frame", "add", id, "--story", story, "--page", page, "--layer", "layer-1",
        "--x", "20", "--y", "20", "--width", "260", "--height", "60",
    ];
    args.extend_from_slice(extra);
    ok(&args)
}

fn thread(doc: &str, story_id: &str) -> Vec<String> {
    let listing = ok(&["text", doc, "story", "show", story_id]);
    listing["story"]["thread"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn an_unplaced_story_is_valid_and_reported() {
    let ws = Workspace::new("unplaced");
    let doc = book(&ws, 1);
    story(&doc, "notes", "Only text, no frame yet.");
    let list = ok(&["text", &doc, "story", "list"]);
    assert_eq!(list["matches"], 1);
    assert_eq!(list["returned"], 1);
    assert_eq!(list["has_more"], false);
    assert_eq!(list["stories"][0]["placed"], false);
    assert_eq!(list["stories"][0]["frames"], 0);
    assert!(list["stories"][0]["overset"].is_null());
    // The text is stored once, in the story, never in a frame.
    let raw = ws.json();
    assert_eq!(raw["layout"]["stories"][0]["id"], "notes");
}

#[test]
fn story_text_is_nfc_normalized_and_control_characters_are_refused() {
    let ws = Workspace::new("text");
    let doc = book(&ws, 1);
    // "e" + combining acute becomes the single precomposed character.
    story(&doc, "accent", "caf\u{65}\u{301}");
    let raw = ws.json();
    assert_eq!(
        raw["layout"]["stories"][0]["paragraphs"][0]["runs"][0]["text"],
        "caf\u{e9}"
    );
    let before = ws.bytes();
    let error = fails(&["text", &doc, "story", "add", "bad", "--text", "a\u{7}b"]);
    assert!(error.contains("control character"), "{error}");
    assert_eq!(before, ws.bytes());
}

#[test]
fn frames_thread_across_pages_and_insert_in_order() {
    let ws = Workspace::new("thread");
    let doc = book(&ws, 3);
    story(&doc, "s", &long_text(3));
    frame(&doc, "f1", "s", "page-1", &[]);
    frame(&doc, "f3", "s", "p3", &[]);
    assert_eq!(thread(&doc, "s"), ["f1", "f3"]);
    // Inserting after f1 puts the new frame in the middle.
    let added = frame(&doc, "f2", "s", "p2", &["--after", "f1"]);
    assert_eq!(added["position"], 1);
    assert_eq!(thread(&doc, "s"), ["f1", "f2", "f3"]);
    let raw = ws.json();
    let f1 = &raw["pages"][0]["layers"][0]["nodes"][0];
    assert_eq!(f1["kind"], "text-frame");
    assert_eq!(f1["next"], "f2");

    let frames = ok(&["text", &doc, "frame", "list", "--limit", "2"]);
    assert_eq!(frames["matches"], 3);
    assert_eq!(frames["returned"], 2);
    assert_eq!(frames["has_more"], true);
    let rest = ok(&[
        "text", &doc, "frame", "list", "--offset", "2", "--limit", "2",
    ]);
    assert_eq!(rest["returned"], 1);
    assert_eq!(rest["has_more"], false);
}

#[test]
fn a_frame_cannot_join_a_chain_of_another_story_or_form_a_cycle() {
    let ws = Workspace::new("invalid-thread");
    let doc = book(&ws, 2);
    story(&doc, "a", "alpha");
    story(&doc, "b", "beta");
    frame(&doc, "fa", "a", "page-1", &[]);
    frame(&doc, "fb", "b", "p2", &[]);
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["nodes"][0]["next"] = json!("fb");
    ws.write(&raw);
    let before = ws.bytes();
    let error = fails(&[
        "text",
        &doc,
        "--dry-run",
        "story",
        "add",
        "c",
        "--text",
        "x",
    ]);
    assert!(error.contains("different story"), "{error}");
    assert_eq!(before, ws.bytes());

    // A two-frame cycle.
    let mut raw = ws.json();
    raw["pages"][0]["layers"][0]["nodes"][0]["next"] = Value::Null;
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"text-frame","id":"fa2","story":"a","next":"fa","x":0,"y":0,"width":10,"height":10}));
    raw["pages"][0]["layers"][0]["nodes"][0]["next"] = json!("fa2");
    ws.write(&raw);
    let error = fails(&[
        "text",
        &doc,
        "--dry-run",
        "story",
        "add",
        "c",
        "--text",
        "x",
    ]);
    assert!(error.contains("cycle"), "{error}");
}

#[test]
fn frame_options_are_validated_and_applied() {
    let ws = Workspace::new("options");
    let doc = book(&ws, 1);
    story(&doc, "s", "text");
    frame(
        &doc,
        "f",
        "s",
        "page-1",
        &[
            "--columns",
            "2",
            "--gutter",
            "10",
            "--inset",
            "4,4,4,4",
            "--vertical-align",
            "center",
            "--first-baseline",
            "cap-height",
            "--ignore-wrap",
            "true",
        ],
    );
    let node = ws.json()["pages"][0]["layers"][0]["nodes"][0].clone();
    assert_eq!(node["columns"], 2);
    assert_eq!(node["gutter"], 10.0);
    assert_eq!(node["inset"], json!([4.0, 4.0, 4.0, 4.0]));
    assert_eq!(node["vertical_align"], "center");
    assert_eq!(node["first_baseline"], "cap-height");
    assert_eq!(node["ignore_wrap"], true);

    let before = ws.bytes();
    for bad in [
        vec!["--columns", "41"],
        vec!["--columns", "0"],
        vec!["--vertical-align", "middle"],
        vec!["--inset", "1,2,3"],
        vec!["--auto-size", "sideways"],
    ] {
        let mut args = vec!["text", doc.as_str(), "frame", "set", "f"];
        args.extend(bad.iter());
        fails(&args);
        assert_eq!(before, ws.bytes(), "{bad:?}");
    }
}

#[test]
fn overset_is_reported_with_story_frame_offset_and_paragraph() {
    let ws = Workspace::new("overset");
    let doc = book(&ws, 1);
    story(&doc, "s", &long_text(20));
    let added = frame(&doc, "f", "s", "page-1", &[]);
    assert_eq!(added["overset_count"], 1);
    let shown = ok(&["text", &doc, "story", "show", "s"]);
    let overset = &shown["story"]["overset"];
    assert_eq!(overset["story"], "s");
    assert_eq!(overset["frame"], "f");
    assert_eq!(overset["estimated"], true);
    assert!(overset["offset"].as_u64().unwrap() > 0);
    assert!(overset["paragraph"].as_u64().unwrap() < 20);
    let total = shown["story"]["characters"].as_u64().unwrap();
    assert_eq!(
        overset["offset"].as_u64().unwrap() + overset["characters_remaining"].as_u64().unwrap(),
        total + 1
    );
    // Tree and search report the same totals compactly.
    let tree = ok(&["tree", &doc]);
    assert_eq!(tree["stories"]["overset_count"], 1);
    assert_eq!(tree["layers"][0]["objects"][0]["story"], "s");
}

#[test]
fn autoflow_adds_pages_until_the_story_fits_and_is_deterministic() {
    let ws = Workspace::new("flow");
    let doc = book(&ws, 1);
    story(&doc, "s", &long_text(20));
    let first = ok(&["text", &doc, "story", "flow", "s", "--page", "page-1"]);
    assert_eq!(first["autoflow_truncated"], false);
    assert_eq!(first["overset_count"], 0);
    let pages = first["pages_added"].as_array().unwrap().len();
    assert!(pages >= 2, "{first}");
    assert_eq!(thread(&doc, "s").len(), pages + 1);
    // Flowing a fitting story again changes nothing.
    let again = ok(&["text", &doc, "story", "flow", "s"]);
    assert!(again["pages_added"].as_array().unwrap().is_empty());

    // Same input, same result.
    let other = Workspace::new("flow-repeat");
    let doc2 = book(&other, 1);
    story(&doc2, "s", &long_text(20));
    ok(&["text", &doc2, "story", "flow", "s", "--page", "page-1"]);
    assert_eq!(ws.json()["pages"], other.json()["pages"]);
}

#[test]
fn autoflow_is_bounded_by_max_pages_and_reports_truncation() {
    let ws = Workspace::new("flow-bound");
    let doc = book(&ws, 1);
    story(&doc, "s", &long_text(40));
    let result = ok(&[
        "text",
        &doc,
        "story",
        "flow",
        "s",
        "--page",
        "page-1",
        "--max-pages",
        "2",
    ]);
    assert_eq!(result["autoflow_truncated"], true);
    assert_eq!(result["pages_added"].as_array().unwrap().len(), 2);
    assert!(result["characters_remaining"].as_u64().unwrap() > 0);
    // The document stays consistent and the overset is still reported.
    assert_eq!(result["overset_count"], 1);
    let error = fails(&["text", &doc, "story", "flow", "s", "--max-pages", "5001"]);
    assert!(error.contains("--max-pages"), "{error}");
}

#[test]
fn autoflow_aborts_with_stalled_when_the_primary_frame_cannot_hold_a_line() {
    let ws = Workspace::new("flow-stalled");
    let doc = ws.doc();
    ok(&["new", &doc, "--width", "300", "--height", "200"]);
    ok(&["migrate", &doc, "--target", "9"]);
    ok(&["parent", &doc, "create", "tiny", "--margins", "99,20,99,20"]);
    ok(&["parent", &doc, "assign", "page-1", "--parent", "tiny"]);
    story(&doc, "s", &long_text(5));
    let before = ws.bytes();
    let error = fails(&["text", &doc, "story", "flow", "s", "--page", "page-1"]);
    assert!(error.contains("autoflow-stalled"), "{error}");
    assert_eq!(before, ws.bytes());
}

#[test]
fn unthread_splits_the_story_and_thread_merges_it_back() {
    let ws = Workspace::new("unthread");
    let doc = book(&ws, 2);
    story(&doc, "s", &long_text(12));
    frame(&doc, "f1", "s", "page-1", &[]);
    frame(&doc, "f2", "s", "p2", &[]);
    let before = ws.json()["layout"]["stories"][0].clone();
    let result = ok(&["text", &doc, "frame", "unthread", "f1"]);
    assert_eq!(result["tail_story"], "s-2");
    assert_eq!(thread(&doc, "s"), ["f1"]);
    assert_eq!(thread(&doc, "s-2"), ["f2"]);
    let raw = ws.json();
    assert_eq!(
        raw["pages"][0]["layers"][0]["nodes"][0]["next"],
        Value::Null
    );
    let head = raw["layout"]["stories"][0]["paragraphs"]
        .as_array()
        .unwrap()
        .len();
    let tail = raw["layout"]["stories"][1]["paragraphs"]
        .as_array()
        .unwrap()
        .len();
    assert!(head >= 1 && tail >= 1);
    let characters = |story: &Value| -> usize {
        story["paragraphs"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["runs"].as_array().unwrap())
            .map(|r| r["text"].as_str().map_or(0, |t| t.chars().count()))
            .sum()
    };
    let original = characters(&before);
    assert_eq!(
        characters(&raw["layout"]["stories"][0]) + characters(&raw["layout"]["stories"][1]),
        original,
        "no text is lost when a thread is split"
    );
    ok(&["text", &doc, "frame", "thread", "f1", "f2"]);
    assert_eq!(thread(&doc, "s"), ["f1", "f2"]);
    assert_eq!(ws.json()["layout"]["stories"].as_array().unwrap().len(), 1);
}

#[test]
fn removing_a_middle_frame_relinks_and_break_thread_keeps_the_tail() {
    let ws = Workspace::new("remove");
    let doc = book(&ws, 3);
    story(&doc, "s", &long_text(12));
    frame(&doc, "f1", "s", "page-1", &[]);
    frame(&doc, "f2", "s", "p2", &[]);
    frame(&doc, "f3", "s", "p3", &[]);
    let result = ok(&["text", &doc, "frame", "remove", "f2"]);
    assert_eq!(result["rethreaded"]["previous"], "f1");
    assert_eq!(result["rethreaded"]["next"], "f3");
    assert_eq!(thread(&doc, "s"), ["f1", "f3"]);

    frame(&doc, "f2b", "s", "p2", &["--after", "f1"]);
    let result = ok(&["text", &doc, "frame", "remove", "f2b", "--break-thread"]);
    assert_eq!(result["tail_story"], "s-2");
    assert_eq!(thread(&doc, "s"), ["f1"]);
    assert_eq!(thread(&doc, "s-2"), ["f3"]);
}

#[test]
fn the_last_frame_of_a_story_is_never_deleted_without_drop_story() {
    let ws = Workspace::new("last");
    let doc = book(&ws, 1);
    story(&doc, "s", "one frame only");
    frame(&doc, "f", "s", "page-1", &[]);
    let before = ws.bytes();
    let error = fails(&["text", &doc, "frame", "remove", "f"]);
    assert!(error.contains("story-last-frame"), "{error}");
    assert_eq!(before, ws.bytes());
    let result = ok(&["text", &doc, "frame", "remove", "f", "--drop-story"]);
    assert_eq!(result["story_dropped"], true);
    assert!(ws.json()["layout"]["stories"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn removing_a_page_with_threaded_frames_rethreads_and_reports() {
    let ws = Workspace::new("page-remove");
    let doc = book(&ws, 3);
    story(&doc, "s", &long_text(12));
    frame(&doc, "f1", "s", "page-1", &[]);
    frame(&doc, "f2", "s", "p2", &[]);
    frame(&doc, "f3", "s", "p3", &[]);
    let result = ok(&["page", &doc, "remove", "p2"]);
    assert_eq!(result["frames"]["rethreaded"][0]["previous"], "f1");
    assert_eq!(result["frames"]["rethreaded"][0]["next"], "f3");
    assert_eq!(result["reflow"]["stories_reflowed"], 1);
    assert_eq!(thread(&doc, "s"), ["f1", "f3"]);

    // A page holding every frame of a story is protected.
    story(&doc, "lone", "text");
    frame(&doc, "lf", "lone", "p3", &[]);
    let before = ws.bytes();
    let error = fails(&["page", &doc, "remove", "p3"]);
    assert!(
        error.contains("story-last-frame") || error.contains("drop-story"),
        "{error}"
    );
    assert_eq!(before, ws.bytes());
    // `lone` has all its frames on p3, so it blocks even though `s` could rethread.
    let result = ok(&["page", &doc, "remove", "p3", "--drop-story"]);
    assert_eq!(result["frames"]["stories_dropped"][0], "lone");
    assert_eq!(thread(&doc, "s"), ["f1"]);
}

#[test]
fn duplicating_a_page_gives_its_frames_fresh_ids_and_stories() {
    let ws = Workspace::new("page-duplicate");
    let doc = book(&ws, 1);
    story(&doc, "s", "text");
    frame(&doc, "f", "s", "page-1", &[]);
    let result = ok(&["page", &doc, "duplicate", "page-1", "copy"]);
    let copied = &result["frames"]["duplicated_frames"][0];
    assert_eq!(copied["frame"], "f");
    assert_eq!(copied["copy"], "f-2");
    assert_eq!(copied["story"], "s-2");
    assert_eq!(thread(&doc, "s"), ["f"]);
    assert_eq!(thread(&doc, "s-2"), ["f-2"]);
}

#[test]
fn removing_a_story_requires_with_frames_when_it_is_placed() {
    let ws = Workspace::new("story-remove");
    let doc = book(&ws, 1);
    story(&doc, "s", "text");
    frame(&doc, "f", "s", "page-1", &[]);
    let before = ws.bytes();
    fails(&["text", &doc, "story", "remove", "s"]);
    assert_eq!(before, ws.bytes());
    let result = ok(&["text", &doc, "story", "remove", "s", "--with-frames"]);
    assert_eq!(result["frames_removed"][0], "f");
    assert!(ws.json()["layout"]["stories"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn dry_run_matches_the_commit_and_undo_restores_the_document() {
    let ws = Workspace::new("dry-run");
    let doc = book(&ws, 1);
    story(&doc, "s", &long_text(20));
    let before = ws.bytes();
    let planned = ok(&[
        "text",
        &doc,
        "--dry-run",
        "story",
        "flow",
        "s",
        "--page",
        "page-1",
    ]);
    assert_eq!(before, ws.bytes(), "dry run must not write");
    let committed = ok(&["text", &doc, "story", "flow", "s", "--page", "page-1"]);
    assert_eq!(planned["pages_added"], committed["pages_added"]);
    assert_eq!(planned["frames_added"], committed["frames_added"]);
    assert_ne!(before, ws.bytes());
    ok(&["undo", &doc]);
    assert_eq!(before, ws.bytes());
}

#[test]
fn frames_and_stories_need_a_version_9_document() {
    let ws = Workspace::new("old");
    let doc = ws.doc();
    ok(&["new", &doc]);
    let error = fails(&["text", &doc, "story", "add", "s", "--text", "x"]);
    assert!(error.contains("version 9"), "{error}");
}

#[test]
fn story_list_paginates_exactly() {
    let ws = Workspace::new("paginate");
    let doc = book(&ws, 1);
    for n in 0..5 {
        story(&doc, &format!("s{n}"), "x");
    }
    let page = ok(&[
        "text", &doc, "story", "list", "--offset", "3", "--limit", "5",
    ]);
    assert_eq!(page["matches"], 5);
    assert_eq!(page["returned"], 2);
    assert_eq!(page["offset"], 3);
    assert_eq!(page["has_more"], false);
    let first = ok(&["text", &doc, "story", "list", "--limit", "2"]);
    assert_eq!(first["returned"], 2);
    assert_eq!(first["has_more"], true);
    let error = fails(&["text", &doc, "story", "list", "--limit", "0"]);
    assert!(error.contains("--limit"), "{error}");
}

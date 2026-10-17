//! v0.20.0 task 4: comments, mentions and review states stored outside the
//! scene. The sidecar must never change the document.
use pentool::comments::{self, Actor, ListOptions, Target};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-comments-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn scene(&self, name: &str) -> PathBuf {
        let from = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/fixtures/v4-scene.pen");
        let to = self.path(name);
        fs::copy(from, &to).unwrap();
        to
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn human() -> Actor {
    Actor {
        kind: "human".into(),
        id: "ana".into(),
        name: "Ana".into(),
        model: None,
        directed_by: None,
    }
}

fn agent() -> Actor {
    Actor {
        kind: "agent".into(),
        id: "bot-1".into(),
        name: "Painter".into(),
        model: Some("claude".into()),
        directed_by: Some("ana".into()),
    }
}

fn on_object(object: &str) -> Target {
    Target {
        object: Some(object.into()),
        ..Target::default()
    }
}

fn err(result: anyhow::Result<Value>) -> String {
    format!("{:#}", result.expect_err("expected an error"))
}

fn sidecar(doc: &Path) -> PathBuf {
    let name = doc.file_name().unwrap().to_string_lossy().into_owned();
    doc.parent()
        .unwrap()
        .join(".pentool/comments")
        .join(format!("{name}.json"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn add_reply_resolve_reopen_and_review_round_trip() {
    let ws = Workspace::new("flow");
    let doc = ws.scene("a.pen");
    let added = comments::add(&doc, on_object("title"), &human(), "Tighten this @bot-1").unwrap();
    let thread = &added["thread"];
    assert_eq!(thread["id"], "t-1");
    assert_eq!(thread["status"], "open");
    assert_eq!(thread["review_state"], "none");
    assert_eq!(thread["version"], 1);
    assert_eq!(thread["comments"][0]["id"], "c-1");
    assert_eq!(
        thread["comments"][0]["mentions"],
        serde_json::json!(["bot-1"])
    );

    let reply = comments::reply(&doc, "t-1", &agent(), "Done, see @ana", Some(1)).unwrap();
    assert_eq!(reply["thread"]["version"], 2);
    assert_eq!(reply["thread"]["comment_count"], 2);
    assert_eq!(reply["thread"]["comments"][1]["author"]["kind"], "agent");
    assert_eq!(
        reply["thread"]["comments"][1]["author"]["directed_by"],
        "ana"
    );

    let review = comments::set_review(&doc, "t-1", "requested", Some(2)).unwrap();
    assert_eq!(review["thread"]["review_state"], "requested");
    assert_eq!(review["thread"]["version"], 3);

    let resolved = comments::set_resolved(&doc, "t-1", true, None).unwrap();
    assert_eq!(resolved["thread"]["status"], "resolved");
    assert_eq!(resolved["changed"], true);

    // Repeating a state change is reported as unchanged and keeps the version.
    let again = comments::set_resolved(&doc, "t-1", true, None).unwrap();
    assert_eq!(again["changed"], false);
    assert_eq!(again["thread"]["version"], 4);

    let reopened = comments::set_resolved(&doc, "t-1", false, None).unwrap();
    assert_eq!(reopened["thread"]["status"], "open");
    assert_eq!(reopened["thread"]["version"], 5);
}

#[test]
fn mentions_are_parsed_deduplicated_and_bounded() {
    assert_eq!(
        comments::parse_mentions("hi @bob, @alice and @bob. mail a@b.c @carol-x."),
        vec!["alice", "bob", "carol-x"]
    );
    assert!(comments::parse_mentions("no mentions @ here").is_empty());

    let ws = Workspace::new("mentions");
    let doc = ws.scene("a.pen");
    let many: String = (0..=comments::MAX_MENTIONS)
        .map(|i| format!("@u{i} "))
        .collect();
    let message = err(comments::add(&doc, Target::default(), &human(), &many));
    assert!(message.contains("[limit-exceeded]"), "{message}");
    assert!(
        !sidecar(&doc).exists(),
        "a rejected comment must not create a sidecar"
    );
}

#[test]
fn limits_and_invalid_input_fail_with_named_errors() {
    let ws = Workspace::new("limits");
    let doc = ws.scene("a.pen");
    let huge = "x".repeat(comments::MAX_BODY_BYTES + 1);
    assert!(
        err(comments::add(&doc, Target::default(), &human(), &huge)).contains("[limit-exceeded]")
    );
    assert!(err(comments::add(&doc, Target::default(), &human(), "  ")).contains("[invalid-input]"));
    assert!(err(comments::add(&doc, on_object("nope"), &human(), "x")).contains("[not-found]"));
    let wrong_page = Target {
        page: Some("page-9".into()),
        ..Target::default()
    };
    assert!(err(comments::add(&doc, wrong_page, &human(), "x")).contains("[not-found]"));
    let mut bad = human();
    bad.kind = "robot".into();
    assert!(err(comments::add(&doc, Target::default(), &bad, "x")).contains("[invalid-input]"));
    let mut human_with_model = human();
    human_with_model.model = Some("m".into());
    assert!(err(comments::add(
        &doc,
        Target::default(),
        &human_with_model,
        "x"
    ))
    .contains("[invalid-input]"));
    let nan = Target {
        point: Some([f64::NAN, 0.0]),
        ..Target::default()
    };
    assert!(err(comments::add(&doc, nan, &human(), "x")).contains("[invalid-input]"));

    comments::add(&doc, Target::default(), &human(), "ok").unwrap();
    assert!(err(comments::reply(&doc, "t-9", &human(), "x", None)).contains("[not-found]"));
    assert!(err(comments::set_review(&doc, "t-1", "maybe", None)).contains("[invalid-input]"));
    assert!(err(comments::reply(&doc, "t-1", &human(), "x", Some(7))).contains("[conflict]"));
    let state = comments::list(&doc, &ListOptions::default()).unwrap();
    assert_eq!(
        state["threads"][0]["comment_count"], 1,
        "failed calls must not mutate"
    );
    assert!(err(comments::list(
        &doc,
        &ListOptions {
            limit: Some(0),
            ..ListOptions::default()
        }
    ))
    .contains("[limit-exceeded]"));
    assert!(err(comments::list(
        &doc,
        &ListOptions {
            status: Some("closed".into()),
            ..ListOptions::default()
        }
    ))
    .contains("[invalid-input]"));
}

#[test]
fn comment_count_per_thread_is_capped() {
    let ws = Workspace::new("cap");
    let doc = ws.scene("a.pen");
    comments::add(&doc, Target::default(), &human(), "first").unwrap();
    for _ in 1..comments::MAX_COMMENTS_PER_THREAD {
        comments::reply(&doc, "t-1", &human(), "more", None).unwrap();
    }
    let message = err(comments::reply(&doc, "t-1", &human(), "one too many", None));
    assert!(message.contains("[limit-exceeded]"), "{message}");
}

#[test]
fn malformed_sidecar_is_reported_and_left_untouched() {
    let ws = Workspace::new("bad");
    let doc = ws.scene("a.pen");
    let path = sidecar(&doc);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for bad in [
        "not json".to_string(),
        r#"{"schema":9,"next_thread":1,"next_comment":1,"threads":[]}"#.to_string(),
        r#"{"schema":1,"next_thread":3,"next_comment":3,"threads":[
            {"id":"t-1","target":{},"status":"open","review_state":"none","version":1,"comments":[]}]}"#
            .to_string(),
    ] {
        fs::write(&path, &bad).unwrap();
        let message = err(comments::add(&doc, Target::default(), &human(), "x"));
        assert!(message.contains("[invalid-input]"), "{message}");
        assert!(comments::list(&doc, &ListOptions::default()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), bad, "sidecar must be untouched");
    }
}

#[test]
fn commenting_never_changes_the_document_or_its_render() {
    let ws = Workspace::new("pure");
    let doc = ws.scene("a.pen");
    let before = fs::read(&doc).unwrap();
    let png_before = ws.path("before.png");
    assert!(run(&[
        "export",
        doc.to_str().unwrap(),
        png_before.to_str().unwrap()
    ])
    .status
    .success());

    comments::add(&doc, on_object("card-bg"), &human(), "bg looks flat").unwrap();
    comments::reply(&doc, "t-1", &agent(), "fixed", None).unwrap();
    comments::set_review(&doc, "t-1", "approved", None).unwrap();
    comments::set_resolved(&doc, "t-1", true, None).unwrap();

    assert_eq!(
        fs::read(&doc).unwrap(),
        before,
        "the .pen bytes must not change"
    );
    assert!(sidecar(&doc).exists());
    let png_after = ws.path("after.png");
    assert!(
        run(&["export", doc.to_str().unwrap(), png_after.to_str().unwrap()])
            .status
            .success()
    );
    assert_eq!(fs::read(png_before).unwrap(), fs::read(png_after).unwrap());
    // The commit boundary's history is untouched too.
    assert!(!doc.parent().unwrap().join(".pentool/history").exists());
}

#[test]
fn list_pagination_filters_and_exact_counts() {
    let ws = Workspace::new("list");
    let doc = ws.scene("a.pen");
    for i in 0..7 {
        let target = if i % 2 == 0 {
            on_object("title")
        } else {
            on_object("card")
        };
        comments::add(&doc, target, &human(), &format!("note {i} @rev")).unwrap();
    }
    comments::set_resolved(&doc, "t-2", true, None).unwrap();

    let page = |offset, limit| {
        comments::list(
            &doc,
            &ListOptions {
                offset,
                limit: Some(limit),
                ..ListOptions::default()
            },
        )
        .unwrap()
    };
    let first = page(0, 3);
    assert_eq!(first["matches"], 7);
    assert_eq!(first["returned"], 3);
    assert_eq!(first["offset"], 0);
    assert_eq!(first["limit"], 3);
    assert_eq!(first["has_more"], true);
    let last = page(6, 3);
    assert_eq!(last["returned"], 1);
    assert_eq!(last["has_more"], false);
    assert_eq!(last["threads"][0]["id"], "t-7");
    let beyond = page(20, 3);
    assert_eq!(beyond["returned"], 0);
    assert_eq!(beyond["has_more"], false);
    assert!(
        first["threads"][0].get("comments").is_none(),
        "list stays compact"
    );

    let open = comments::list(
        &doc,
        &ListOptions {
            status: Some("open".into()),
            object: Some("title".into()),
            ..ListOptions::default()
        },
    )
    .unwrap();
    assert_eq!(open["matches"], 4);
    let mentioned = comments::list(
        &doc,
        &ListOptions {
            mention: Some("@rev".into()),
            limit: Some(2),
            ..ListOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        (
            mentioned["matches"].as_u64(),
            mentioned["returned"].as_u64()
        ),
        (Some(7), Some(2))
    );
    let one = comments::list(
        &doc,
        &ListOptions {
            thread: Some("t-2".into()),
            ..ListOptions::default()
        },
    )
    .unwrap();
    assert_eq!(one["threads"][0]["comments"][0]["body"], "note 1 @rev");
    assert!(err(comments::list(
        &doc,
        &ListOptions {
            thread: Some("t-99".into()),
            ..ListOptions::default()
        }
    ))
    .contains("[not-found]"));
}

#[test]
fn deleted_targets_stay_as_dangling_threads() {
    let ws = Workspace::new("dangling");
    let doc = ws.scene("a.pen");
    comments::add(&doc, on_object("title"), &human(), "about the title").unwrap();
    comments::add(&doc, on_object("card-bg"), &human(), "about the bg").unwrap();

    let mut value: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let children = value["pages"][0]["layers"][0]["nodes"][0]["children"]
        .as_array_mut()
        .unwrap();
    children.retain(|c| c["id"] != "title");
    fs::write(&doc, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let listed = comments::list(&doc, &ListOptions::default()).unwrap();
    assert_eq!(listed["matches"], 2, "no thread is dropped");
    assert_eq!(listed["dangling"], 1);
    assert_eq!(listed["threads"][0]["dangling"], true);
    assert_eq!(listed["threads"][1]["dangling"], false);
    // A dangling thread can still be replied to and resolved.
    comments::reply(&doc, "t-1", &human(), "target was removed", None).unwrap();
    comments::set_resolved(&doc, "t-1", true, None).unwrap();
}

#[test]
fn concurrent_writers_serialize_through_the_lock() {
    let ws = Workspace::new("race");
    let doc = ws.scene("a.pen");
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let doc = doc.clone();
            std::thread::spawn(move || {
                comments::add(&doc, on_object("title"), &human(), &format!("note {i}")).unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let listed = comments::list(&doc, &ListOptions::default()).unwrap();
    assert_eq!(listed["matches"], 8);
    let mut ids: Vec<String> = listed["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 8, "thread ids are unique");
    assert!(
        !sidecar(&doc).with_extension("json.lock").exists(),
        "lock is released"
    );
}

#[test]
fn json_requests_share_the_same_code_path() {
    let ws = Workspace::new("apply");
    let doc = ws.scene("a.pen");
    let added = comments::apply(
        &doc,
        &serde_json::json!({
            "action": "add",
            "body": "from the server @ana",
            "target": {"object": "title", "point": [1.5, 2.0]},
            "actor": {"kind": "human", "id": "ana", "name": "Ana"}
        }),
    )
    .unwrap();
    assert_eq!(
        added["thread"]["target"]["point"],
        serde_json::json!([1.5, 2.0])
    );
    let review = comments::apply(
        &doc,
        &serde_json::json!({"action": "review", "thread": "t-1", "state": "changes-requested", "if_version": 1}),
    )
    .unwrap();
    assert_eq!(review["thread"]["review_state"], "changes-requested");
    assert!(err(comments::apply(
        &doc,
        &serde_json::json!({"action": "explode"})
    ))
    .contains("[invalid-input]"));
    assert!(err(comments::apply(
        &doc,
        &serde_json::json!({"action": "add", "body": "x"})
    ))
    .contains("[invalid-input]"));
}

#[test]
fn cli_commands_cover_the_workflow() {
    let ws = Workspace::new("cli");
    let doc = ws.scene("a.pen");
    let d = doc.to_str().unwrap();
    let json = |args: &[&str]| -> Value {
        let out = run(args);
        assert!(
            out.status.success(),
            "{args:?}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let added = json(&[
        "comment",
        "add",
        d,
        "check the card @ana",
        "--object",
        "card",
        "--author",
        "Ana",
        "--actor-id",
        "ana",
        "--json",
    ]);
    assert_eq!(added["thread"]["id"], "t-1");
    let at_point = json(&[
        "comment", "add", d, "here", "--x", "-4.5", "--y", "10", "--json",
    ]);
    assert_eq!(
        at_point["thread"]["target"]["point"],
        serde_json::json!([-4.5, 10.0])
    );
    json(&[
        "comment",
        "reply",
        d,
        "t-1",
        "on it",
        "--kind",
        "agent",
        "--actor-id",
        "bot",
        "--author",
        "Bot",
        "--model",
        "claude",
        "--directed-by",
        "ana",
        "--if-version",
        "1",
    ]);
    json(&["comment", "review", d, "t-1", "requested"]);
    json(&["comment", "resolve", d, "t-1"]);
    json(&["comment", "reopen", d, "t-1"]);
    let listed = json(&["comment", "list", d, "--status", "open", "--limit", "1"]);
    assert_eq!(listed["matches"], 2);
    assert_eq!(listed["returned"], 1);
    assert_eq!(listed["has_more"], true);

    let stale = run(&["comment", "reply", d, "t-1", "late", "--if-version", "1"]);
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("[conflict]"));
}

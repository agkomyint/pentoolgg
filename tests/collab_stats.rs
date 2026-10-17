//! Contribution statistics derived from the operation log: determinism,
//! permutation invariance, attribution of mixed human and agent work, the
//! summary embedded in the `.pen`, and attribution staying out of exports.
use pentool::collab::hlc::Hlc;
use pentool::collab::merge::{replay, Replayer};
use pentool::collab::op::{Actor, Draft, Op};
use pentool::collab::stats::{self, By};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

// ------------------------------------------------------------ test helpers

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

fn genesis() -> Value {
    json!({
        "format": "pentool", "version": 4, "name": "stats",
        "pages": [{
            "id": "page-1", "name": "Page 1",
            "canvas": {"width": 200, "height": 200, "background": "#ffffff"},
            "layers": [{"id": "content", "name": "Content", "visible": true, "locked": false,
                "nodes": [
                    {"kind": "rect", "id": "a", "x": 0, "y": 0, "width": 10, "height": 10},
                    {"kind": "rect", "id": "b", "x": 20, "y": 0, "width": 10, "height": 10}
                ]}]
        }],
        "styles": {}, "components": [], "fonts": []
    })
}

fn human() -> Actor {
    Actor {
        id: Actor::derive_id("human", "alice"),
        kind: "human".into(),
        display_name: Some("alice".into()),
        tool: None,
        model: None,
        directed_by: None,
    }
}

fn agent() -> Actor {
    Actor {
        id: Actor::derive_id("agent", "bot"),
        kind: "agent".into(),
        display_name: Some("bot".into()),
        tool: None,
        model: None,
        directed_by: Some(human().id),
    }
}

fn op(actor: &Actor, wall: u64, deps: &[&Op], draft: Draft) -> Op {
    Op::build(
        draft,
        &actor.id,
        Hlc {
            wall_ms: wall,
            counter: 0,
            peer: format!("peer-{}", actor.kind),
        },
        "sha256:test",
        deps.iter().map(|d| d.id.clone()).collect(),
        "test",
    )
    .expect("op builds")
}

fn set(id: &str, path: &str, value: Value) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "set-property",
        targets: vec![id.into()],
        payload: json!({"level": "n", "path": path, "value": value}),
    }
}

fn create(id: &str, key: &str) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "create",
        targets: vec![id.into()],
        payload: json!({
            "level": "n", "key": key,
            "parent": {"level": "l", "id": "content"},
            "node": {"kind": "rect", "id": id, "x": 0, "y": 0, "width": 4, "height": 4}
        }),
    }
}

fn delete(id: &str) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "delete",
        targets: vec![id.into()],
        payload: json!({"level": "n"}),
    }
}

/// An agent builds `h` and a throwaway `tmp`; a human restyles `h` and edits
/// the original `a`. Timestamps ascend in list order.
fn mixed_ops() -> Vec<Op> {
    let (alice, bot) = (human(), agent());
    let make = op(&bot, 5, &[], create("h", "t"));
    let tmp = op(&bot, 6, &[&make], create("tmp", "u"));
    let restyle = op(&alice, 7, &[&make], set("h", "fill", json!("#ff0000")));
    let edit_a = op(&alice, 8, &[], set("a", "x", json!(3)));
    let drop = op(&bot, 9, &[&tmp], delete("tmp"));
    vec![make, tmp, restyle, edit_a, drop]
}

fn registry() -> Vec<Actor> {
    vec![human(), agent()]
}

fn report_of(ops: &[Op], by: By) -> Value {
    let replayed = replay(&genesis(), ops).expect("replay");
    stats::report(&replayed.state.stats_raw(), &registry(), by, 0, 1000).expect("report")
}

fn row<'a>(report: &'a Value, key: &str) -> &'a Value {
    report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == json!(key))
        .unwrap_or_else(|| panic!("no row {key} in {report}"))
}

// ------------------------------------------------------------ library level

#[test]
fn shares_are_exact_and_sum_to_one_hundred_percent() {
    assert_eq!(stats::shares(&[1, 1, 1]), vec![3334, 3333, 3333]);
    assert_eq!(stats::shares(&[0, 0]), vec![0, 0]);
    assert_eq!(stats::shares(&[3, 1]), vec![7500, 2500]);
    for counts in [vec![7, 11, 13], vec![1, 2, 3, 4, 5], vec![99, 1]] {
        assert_eq!(stats::shares(&counts).iter().sum::<u64>(), 10_000);
    }
    assert_eq!(stats::percent(3334), "33.34%");
    assert_eq!(stats::percent(5), "0.05%");
}

#[test]
fn mixed_human_and_agent_work_is_attributed_to_both() {
    let report = report_of(&mixed_ops(), By::Actor);
    let (alice, bot) = (human().id, agent().id);
    let agent_row = row(&report, &bot);
    assert_eq!(agent_row["operations"], json!(3));
    assert_eq!(agent_row["objects_created"], json!(2));
    // `tmp` was deleted: it counts toward operations and creation, not survival
    assert_eq!(agent_row["net_authored"], json!(1));
    assert_eq!(
        agent_row["objects_modified"],
        json!(0),
        "own creations excluded"
    );
    assert_eq!(agent_row["surviving_properties"], json!(0));
    let human_row = row(&report, &alice);
    assert_eq!(human_row["operations"], json!(2));
    assert_eq!(human_row["objects_created"], json!(0));
    assert_eq!(human_row["objects_modified"], json!(2), "h and a");
    assert_eq!(human_row["properties_changed"], json!(2));
    assert_eq!(human_row["surviving_properties"], json!(2));

    assert_eq!(report["total_operations"], json!(5));
    assert_eq!(report["totals"]["agent"]["share_bp"], json!(6000));
    assert_eq!(report["totals"]["human"]["share"], json!("40.00%"));
    // the agent's work rolls up to the human who directs it
    let director = &report["by_director"][0];
    assert_eq!(director["key"], json!(alice));
    assert_eq!(director["operations"], json!(5));

    let by_kind = report_of(&mixed_ops(), By::Kind);
    assert_eq!(row(&by_kind, "agent")["net_authored"], json!(1));
    assert_eq!(row(&by_kind, "human")["surviving_properties"], json!(2));
    let by_page = report_of(&mixed_ops(), By::Page);
    assert_eq!(by_page["matches"], json!(1));
    assert_eq!(row(&by_page, "page-1")["operations"], json!(5));
}

#[test]
fn a_human_restyling_an_agent_object_keeps_both_contributions() {
    let report = report_of(&mixed_ops()[..3], By::Kind);
    assert_eq!(row(&report, "agent")["objects_created"], json!(2));
    assert_eq!(row(&report, "agent")["net_authored"], json!(2));
    assert_eq!(row(&report, "human")["surviving_properties"], json!(1));
    assert_eq!(row(&report, "human")["objects_modified"], json!(1));
}

#[test]
fn identical_logs_produce_byte_identical_reports() {
    let first = serde_json::to_string(&report_of(&mixed_ops(), By::Actor)).unwrap();
    let second = serde_json::to_string(&report_of(&mixed_ops(), By::Actor)).unwrap();
    assert_eq!(first, second);
}

#[test]
fn delivery_order_does_not_change_the_statistics() {
    let ops = mixed_ops();
    let expected = serde_json::to_string(&report_of(&ops, By::Actor)).unwrap();
    let mut rng = Rng(7);
    for _ in 0..25 {
        let mut shuffled = ops.clone();
        rng.shuffle(&mut shuffled);
        assert_eq!(
            serde_json::to_string(&report_of(&shuffled, By::Actor)).unwrap(),
            expected
        );
    }
}

#[test]
fn a_forged_actor_gains_no_share() {
    let mut ops = mixed_ops();
    // The agent's operation is rewritten to claim the human's identity.
    let mut forged = ops[0].clone();
    forged.actor = human().id;
    assert!(forged.check().is_err(), "ids are derived from content");
    ops[0] = forged;
    let replayed = replay(&genesis(), &ops).unwrap();
    let report =
        stats::report(&replayed.state.stats_raw(), &registry(), By::Actor, 0, 1000).unwrap();
    // the forged operation is rejected, so the human gained nothing from it
    assert!(row(&report, &human().id)["operations"].as_u64().unwrap() <= 2);
    assert!(report["total_operations"].as_u64().unwrap() < 5);
}

#[test]
fn incremental_replay_equals_full_replay() {
    // The ledger is folded into the replayed state, so stopping after a prefix
    // and continuing from that snapshot gives the same statistics as replaying
    // everything (the property log compaction relies on).
    let ops = mixed_ops();
    let full = serde_json::to_string(&report_of(&ops, By::Actor)).unwrap();
    for split in 0..=ops.len() {
        let mut prefix = Replayer::new(&genesis()).unwrap();
        for op in &ops[..split] {
            prefix.apply_op(op).unwrap();
        }
        let mut resumed = prefix.clone();
        for op in &ops[split..] {
            resumed.apply_op(op).unwrap();
        }
        let report = stats::report(&resumed.stats_raw(), &registry(), By::Actor, 0, 1000).unwrap();
        assert_eq!(
            serde_json::to_string(&report).unwrap(),
            full,
            "split {split}"
        );
        // the embedded summary is a pure function of the same state
        let a = stats::summary(&resumed.stats_raw(), &registry()).unwrap();
        let b = stats::summary(
            &replay(&genesis(), &ops).unwrap().state.stats_raw(),
            &registry(),
        )
        .unwrap();
        assert_eq!(a, b);
    }
}

#[test]
fn an_empty_log_reports_zero_without_dividing() {
    let report = report_of(&[], By::Actor);
    assert_eq!(report["total_operations"], json!(0));
    assert_eq!(report["matches"], json!(0));
    assert_eq!(report["has_more"], json!(false));
    assert_eq!(report["totals"]["all"]["share_bp"], json!(0));
    assert_eq!(report["totals"]["human"]["share"], json!("0.00%"));
}

#[test]
fn pagination_and_limits_describe_the_emitted_rows() {
    let replayed = replay(&genesis(), &mixed_ops()).unwrap();
    let raw = replayed.state.stats_raw();
    let first = stats::report(&raw, &registry(), By::Actor, 0, 1).unwrap();
    assert_eq!(first["matches"], json!(2));
    assert_eq!(first["returned"], json!(1));
    assert_eq!(first["has_more"], json!(true));
    let second = stats::report(&raw, &registry(), By::Actor, 1, 1).unwrap();
    assert_eq!(second["returned"], json!(1));
    assert_eq!(second["has_more"], json!(false));
    assert_ne!(first["rows"][0]["key"], second["rows"][0]["key"]);
    let past = stats::report(&raw, &registry(), By::Actor, 5, 10).unwrap();
    assert_eq!(past["returned"], json!(0));
    assert_eq!(past["has_more"], json!(false));
    assert!(stats::report(&raw, &registry(), By::Actor, 0, 0).is_err());
    assert!(stats::report(&raw, &registry(), By::Actor, 0, 1001).is_err());
    assert!(By::parse("team").is_err());
}

#[test]
fn the_embedded_block_is_bounded_and_validated() {
    let mut doc = genesis();
    doc["collab"] = json!({"schema": 1, "seq": 0, "head": "sha256:00", "stats_summary": {"actors": []}, "extra": {"keep": true}});
    pentool::scene::validate(&doc).expect("valid block");

    let mut many = doc.clone();
    many["collab"]["stats_summary"]["actors"] = Value::Array(vec![json!({}); 300]);
    let error = pentool::scene::validate(&many).unwrap_err().to_string();
    assert!(error.contains("limit-exceeded"), "{error}");

    let mut huge = doc.clone();
    huge["collab"]["extra"] = json!("x".repeat(stats::MAX_COLLAB_BYTES + 1));
    let error = pentool::scene::validate(&huge).unwrap_err().to_string();
    assert!(error.contains("limit-exceeded"), "{error}");

    let mut bad = doc.clone();
    bad["collab"]["schema"] = json!(2);
    assert!(pentool::scene::validate(&bad).is_err());
    let mut bad = doc;
    bad["collab"] = json!("nope");
    assert!(pentool::scene::validate(&bad).is_err());
}

#[test]
fn attribution_is_removed_from_exported_bytes_unless_requested() {
    let mut doc = genesis();
    doc["collab"] =
        json!({"schema": 1, "seq": 0, "head": "sha256:00", "stats_summary": {"actors": []}});
    let bytes = serde_json::to_vec_pretty(&doc).unwrap();
    let stripped = pentool::collab::export_bytes(bytes.clone(), false).unwrap();
    let value: Value = serde_json::from_slice(&stripped).unwrap();
    assert!(value.get("collab").is_none());
    assert_eq!(value["name"], json!("stats"));
    assert_eq!(
        pentool::collab::export_bytes(bytes.clone(), true).unwrap(),
        bytes
    );
}

// ------------------------------------------------------------------ CLI

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-stats-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn ok_json(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?} printed non-JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn batch(ws: &Workspace, doc: &str, name: &str, ops: Value, extra: &[&str]) -> Output {
    let file = ws.path(name);
    fs::write(&file, ops.to_string()).unwrap();
    let mut args = vec!["batch", doc, file.as_str()];
    args.extend_from_slice(extra);
    run(&args)
}

const AGENT: [&str; 6] = [
    "--actor-kind",
    "agent",
    "--actor-id",
    "bot",
    "--directed-by",
    "alice",
];

fn put_shape(id: &str) -> Value {
    json!([{"type":"put-shape","shape":"rect","id":id,"layer":"content","x":1,"y":1,"width":2,"height":2}])
}

fn restyle(id: &str) -> Value {
    json!([{"type":"set-property","id":id,"property":"fill","value":"#ff0000"}])
}

fn fixture(ws: &Workspace) -> String {
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    doc
}

fn read_doc(doc: &str) -> Value {
    serde_json::from_slice(&fs::read(doc).unwrap()).unwrap()
}

fn history_len(doc: &str) -> usize {
    ok_json(&["history", doc])["entries"]
        .as_array()
        .unwrap()
        .len()
}

/// A document with one agent-created and human-restyled rectangle.
fn mixed_document(ws: &Workspace) -> String {
    let doc = fixture(ws);
    ok_json(&["collab", "init", &doc]);
    let out = batch(ws, &doc, "agent.json", put_shape("r1"), &AGENT);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = batch(
        ws,
        &doc,
        "human.json",
        restyle("r1"),
        &["--actor-id", "alice"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    doc
}

#[test]
fn stats_without_a_log_suggests_collab_init() {
    let ws = Workspace::new("nolog");
    let doc = fixture(&ws);
    let out = run(&["stats", &doc]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("collab init"));
}

#[test]
fn stats_on_a_fresh_log_is_empty_and_valid() {
    let ws = Workspace::new("fresh");
    let doc = fixture(&ws);
    ok_json(&["collab", "init", &doc]);
    let report = ok_json(&["stats", &doc, "--json"]);
    assert_eq!(report["total_operations"], json!(0));
    assert_eq!(report["matches"], json!(0));
    assert_eq!(report["embedded_summary"]["status"], json!("missing"));
}

#[test]
fn cli_attributes_a_mixed_session_and_is_deterministic() {
    let ws = Workspace::new("mixed");
    let doc = mixed_document(&ws);
    let by_kind = ok_json(&["stats", &doc, "--by", "kind", "--json"]);
    let rows = by_kind["rows"].as_array().unwrap();
    let agent_row = rows.iter().find(|r| r["key"] == json!("agent")).unwrap();
    let human_row = rows.iter().find(|r| r["key"] == json!("human")).unwrap();
    assert_eq!(agent_row["objects_created"], json!(1));
    assert_eq!(agent_row["net_authored"], json!(1));
    assert_eq!(human_row["objects_modified"], json!(1));
    assert!(human_row["surviving_properties"].as_u64().unwrap() >= 1);
    let shares: u64 = rows.iter().map(|r| r["share_bp"].as_u64().unwrap()).sum();
    assert_eq!(shares, 10_000);

    let a = run(&["stats", &doc, "--json"]);
    let b = run(&["stats", &doc, "--json"]);
    assert_eq!(a.stdout, b.stdout, "byte-identical");
    let text = run(&["stats", &doc, "--by", "actor"]);
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("stats by actor"), "{text}");
    let pages = ok_json(&["stats", &doc, "--by", "page", "--json"]);
    assert_eq!(pages["by"], json!("page"));
    assert!(!run(&["stats", &doc, "--by", "team"]).status.success());
    assert!(!run(&["stats", &doc, "--limit", "0"]).status.success());
}

#[test]
fn the_summary_lives_in_the_document_and_commits_atomically() {
    let ws = Workspace::new("embedded");
    let doc = fixture(&ws);
    ok_json(&["collab", "init", &doc]);
    let history_before = history_len(&doc);
    assert!(batch(&ws, &doc, "o.json", put_shape("r1"), &[])
        .status
        .success());
    // one mutation is one history entry that already holds the summary
    assert_eq!(history_len(&doc), history_before + 1);
    let bytes = fs::read(&doc).unwrap();
    let entries = ok_json(&["history", &doc]);
    let last = entries["entries"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["after"], json!(pentool::transaction::revision(&bytes)));

    let value = read_doc(&doc);
    let verify = ok_json(&["collab", "verify", &doc]);
    assert_eq!(value["collab"]["head"], verify["head"]["hash"]);
    assert_eq!(value["collab"]["seq"], verify["head"]["seq"]);
    assert_eq!(verify["embedded_summary"]["status"], json!("verified"));
    assert_eq!(verify["replay"]["matches_document"], json!(true));
    let report = ok_json(&["stats", &doc, "--json"]);
    assert_eq!(
        value["collab"]["stats_summary"]["total_operations"],
        report["total_operations"]
    );
    assert_eq!(report["embedded_summary"]["status"], json!("verified"));

    // a failed mutation changes neither the document nor the log
    let before = fs::read(&doc).unwrap();
    let log_before = ok_json(&["collab", "log", &doc])["matches"].clone();
    let bad = batch(
        &ws,
        &doc,
        "bad.json",
        json!([{"type":"put-shape","shape":"rect","id":"x","layer":"nope","x":1,"y":1,"width":2,"height":2}]),
        &[],
    );
    assert!(!bad.status.success());
    assert_eq!(fs::read(&doc).unwrap(), before);
    assert_eq!(ok_json(&["collab", "log", &doc])["matches"], log_before);
    assert_eq!(history_len(&doc), history_before + 1);
}

#[test]
fn undo_and_redo_keep_the_embedded_summary_consistent() {
    let ws = Workspace::new("undo");
    let doc = mixed_document(&ws);
    ok_json(&["undo", &doc]);
    assert_eq!(
        ok_json(&["collab", "verify", &doc])["embedded_summary"]["status"],
        json!("verified")
    );
    ok_json(&["redo", &doc]);
    let verify = ok_json(&["collab", "verify", &doc]);
    assert_eq!(verify["embedded_summary"]["status"], json!("verified"));
    assert_eq!(verify["replay"]["matches_document"], json!(true));
}

#[test]
fn a_hand_edited_summary_is_detected_and_never_trusted() {
    let ws = Workspace::new("edited");
    let doc = mixed_document(&ws);
    let honest = ok_json(&["stats", &doc, "--json"]);

    let mut value = read_doc(&doc);
    value["collab"]["stats_summary"]["total_operations"] = json!(9999);
    value["collab"]["stats_summary"]["actors"][0]["operations"] = json!(9999);
    fs::write(&doc, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let report = ok_json(&["stats", &doc, "--json"]);
    assert_eq!(report["embedded_summary"]["status"], json!("mismatch"));
    assert_eq!(report["total_operations"], honest["total_operations"]);
    assert_eq!(report["rows"], honest["rows"], "derived from the log only");
    let verify = ok_json(&["collab", "verify", &doc]);
    assert_eq!(verify["embedded_summary"]["status"], json!("mismatch"));
    assert!(verify["warnings"]
        .to_string()
        .contains("embedded collab summary"));

    // the next recorded change rewrites the summary from the log
    assert!(batch(&ws, &doc, "o.json", put_shape("r2"), &[])
        .status
        .success());
    assert_eq!(
        ok_json(&["stats", &doc, "--json"])["embedded_summary"]["status"],
        json!("verified")
    );
}

#[test]
fn unknown_collab_fields_survive_a_recorded_change() {
    let ws = Workspace::new("unknown");
    let doc = fixture(&ws);
    ok_json(&["collab", "init", &doc]);
    let mut value = read_doc(&doc);
    value["collab"] = json!({"schema": 1, "extension": {"keep": [1, 2, 3]}});
    fs::write(&doc, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert!(batch(&ws, &doc, "o.json", put_shape("r1"), &[])
        .status
        .success());
    let after = read_doc(&doc);
    assert_eq!(after["collab"]["extension"], json!({"keep": [1, 2, 3]}));
    assert!(after["collab"]["stats_summary"].is_object());
}

#[test]
fn a_forged_actor_in_the_log_is_refused_by_stats() {
    let ws = Workspace::new("forged");
    let doc = mixed_document(&ws);
    let log_file =
        ws.0.join(".pentool")
            .join("collab")
            .join("scene.pen")
            .join("log.jsonl");
    let text = fs::read_to_string(&log_file).unwrap();
    let agent_id = Actor::derive_id("agent", "bot");
    let human_id = Actor::derive_id("human", "alice");
    assert!(text.contains(&agent_id));
    fs::write(&log_file, text.replacen(&agent_id, &human_id, 1)).unwrap();
    let out = run(&["stats", &doc, "--json"]);
    assert!(!out.status.success(), "tampered log must not yield stats");
    let message = String::from_utf8_lossy(&out.stderr);
    assert!(message.contains("tampered-log"), "{message}");
}

#[test]
fn attribution_does_not_change_rendering_and_strip_removes_it() {
    let ws = Workspace::new("export");
    let doc = mixed_document(&ws);
    assert!(read_doc(&doc).get("collab").is_some());
    let with = ws.path("with.svg");
    ok_json(&["export", &doc, &with]);

    let copy = ws.path("copy.pen");
    let mut value = read_doc(&doc);
    value.as_object_mut().unwrap().remove("collab");
    fs::write(&copy, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let without = ws.path("without.svg");
    ok_json(&["export", &copy, &without]);
    assert_eq!(fs::read(&with).unwrap(), fs::read(&without).unwrap());
    let svg = fs::read_to_string(&with).unwrap();
    assert!(!svg.contains("stats_summary") && !svg.contains("act_"));

    // `collab strip` removes the block in one history entry
    let before = history_len(&doc);
    let result = ok_json(&["collab", "strip", &doc]);
    assert_eq!(result["ok"], json!(true));
    assert!(read_doc(&doc).get("collab").is_none());
    assert_eq!(history_len(&doc), before + 1);
    // the log still knows who did what
    assert!(
        ok_json(&["stats", &doc, "--json"])["total_operations"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[test]
fn packages_strip_attribution_unless_asked() {
    use pentool::{asset, document::Document, package};
    let ws = Workspace::new("package");
    let source = ws.0.join("source.pen");
    fs::write(
        &source,
        serde_json::to_vec_pretty(&Document::new(64, 64)).unwrap(),
    )
    .unwrap();
    let library = ws.0.join("library");
    let output = library.join("assets").join("card.pen");
    asset::create(&asset::CreateOptions {
        input: source,
        output: output.clone(),
        page: None,
        layers: vec![],
        objects: vec![],
        rect: None,
        canvas_bounds: true,
        include_hidden: false,
        properties: vec![],
        id: "ui/card".into(),
        name: "Card".into(),
        description: "Card".into(),
        version: "1.0.0".into(),
        kind: "component".into(),
        author: "Pentool".into(),
        license: "MIT".into(),
        tags: vec![],
        category: "layout".into(),
        overwrite: false,
        dry_run: false,
    })
    .unwrap();
    let manifest =
        json!({"schema":1,"name":"open/ui","version":"1.0.0","license":"MIT","assets":{}});
    fs::write(
        library.join("pentool.package.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let plain = ws.0.join("plain.penpkg");
    package::pack(&library, &plain).unwrap();

    let mut value: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    value["collab"] = json!({"schema": 1, "seq": 1, "head": "sha256:00", "stats_summary": {"actors": [{"id": "act_x"}]}});
    fs::write(&output, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let default_pack = ws.0.join("default.penpkg");
    let opted_in = ws.0.join("opted.penpkg");
    package::pack(&library, &default_pack).unwrap();
    package::pack_with(&library, &opted_in, true).unwrap();
    assert_eq!(fs::read(&default_pack).unwrap(), fs::read(&plain).unwrap());
    assert_ne!(fs::read(&opted_in).unwrap(), fs::read(&plain).unwrap());
}

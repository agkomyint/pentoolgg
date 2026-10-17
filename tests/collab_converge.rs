//! Collaboration operation log: deterministic replay, convergence under any
//! delivery order, merge rules, chain integrity and the transaction hook.
use pentool::collab::hlc::Hlc;
use pentool::collab::merge::{replay, Replay};
use pentool::collab::op::{parse_op, Actor, Draft, Op};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

// ------------------------------------------------------------ test helpers

/// Small deterministic generator (splitmix64); no external dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

fn genesis() -> Value {
    json!({
        "format": "pentool", "version": 4, "name": "converge",
        "pages": [{
            "id": "page-1", "name": "Page 1",
            "canvas": {"width": 200, "height": 200, "background": "#ffffff"},
            "layers": [
                {"id": "content", "name": "Content", "visible": true, "locked": false,
                 "nodes": [
                    {"kind": "rect", "id": "a", "x": 0, "y": 0, "width": 10, "height": 10},
                    {"kind": "rect", "id": "b", "x": 20, "y": 0, "width": 10, "height": 10},
                    {"kind": "group", "id": "g", "children": [
                        {"kind": "rect", "id": "g1", "x": 1, "y": 1, "width": 5, "height": 5}
                    ]}
                 ]},
                {"id": "other", "name": "Other", "visible": true, "locked": false, "nodes": []}
            ]
        }],
        "styles": {}, "components": [], "fonts": []
    })
}

fn actor_id(handle: &str) -> String {
    Actor::derive_id("human", handle)
}

fn op(handle: &str, wall: u64, counter: u32, deps: &[&str], draft: Draft) -> Op {
    Op::build(
        draft,
        &actor_id(handle),
        Hlc {
            wall_ms: wall,
            counter,
            peer: format!("peer-{handle}"),
        },
        "sha256:test",
        deps.iter().map(|d| d.to_string()).collect(),
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

fn set_layer(id: &str, path: &str, value: Value) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "set-property",
        targets: vec![id.into()],
        payload: json!({"level": "l", "path": path, "value": value}),
    }
}

fn create(id: &str, parent_level: &str, parent: &str, key: &str) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "create",
        targets: vec![id.into()],
        payload: json!({
            "level": "n", "key": key,
            "parent": {"level": parent_level, "id": parent},
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

fn mv(id: &str, parent_level: &str, parent: &str, key: &str) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "move",
        targets: vec![id.into()],
        payload: json!({"level": "n", "key": key, "parent": {"level": parent_level, "id": parent}}),
    }
}

fn run_replay(ops: &[Op]) -> Replay {
    replay(&genesis(), ops).expect("replay")
}

fn find<'a>(doc: &'a Value, id: &str) -> Option<&'a Value> {
    match doc {
        Value::Object(map) => {
            if map.get("id").and_then(Value::as_str) == Some(id) {
                return Some(doc);
            }
            map.values().find_map(|v| find(v, id))
        }
        Value::Array(items) => items.iter().find_map(|v| find(v, id)),
        _ => None,
    }
}

fn codes(replay: &Replay) -> Vec<String> {
    replay
        .state
        .conflicts()
        .iter()
        .map(|c| c.code.clone())
        .collect()
}

// -------------------------------------------------------- random op sets

const HANDLES: [&str; 3] = ["alice", "bob", "carol"];
const KEYS: [&str; 5] = ["a0", "m", "t", "w", "z"];

fn random_ops(rng: &mut Rng, count: usize) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    let mut known: Vec<String> = vec!["a".into(), "b".into(), "g".into(), "g1".into()];
    for n in 0..count {
        let handle = *rng.pick(&HANDLES);
        let wall = 1_000 + rng.below(40) as u64;
        let counter = rng.below(3) as u32;
        let deps: Vec<String> = if !ops.is_empty() && rng.below(3) == 0 {
            vec![rng.pick(&ops).id.clone()]
        } else if rng.below(40) == 0 {
            vec!["op_missing".to_string()]
        } else {
            Vec::new()
        };
        let dep_refs: Vec<&str> = deps.iter().map(String::as_str).collect();
        let target = rng.pick(&known).clone();
        let draft = match rng.below(7) {
            0 | 1 => set(&target, "x", json!(rng.below(5))),
            2 => set(
                &target,
                "style/fill/fallback",
                json!(format!("#{:06x}", rng.below(4096))),
            ),
            3 => {
                // ids sometimes collide on purpose (duplicate-id)
                let id = if rng.below(4) == 0 {
                    "dup".to_string()
                } else {
                    format!("n{n}")
                };
                let (level, parent) = if rng.below(2) == 0 {
                    ("l", "content".to_string())
                } else {
                    ("n", rng.pick(&known).clone())
                };
                known.push(id.clone());
                create(&id, level, &parent, rng.pick(&KEYS))
            }
            4 => delete(&target),
            5 => mv(&target, "n", &rng.pick(&known).clone(), rng.pick(&KEYS)),
            _ => set_layer("content", "locked", json!(rng.below(2) == 0)),
        };
        ops.push(op(handle, wall, counter, &dep_refs, draft));
    }
    ops
}

#[test]
fn any_delivery_order_converges_to_identical_bytes() {
    for seed in 0..120u64 {
        let mut rng = Rng(seed.wrapping_mul(7919) + 1);
        let count = 3 + rng.below(28);
        let ops = random_ops(&mut rng, count);
        let reference = run_replay(&ops);
        let want = reference.bytes();
        let want_conflicts = reference.state.conflicts();
        for round in 0..6 {
            let mut shuffled = ops.clone();
            rng.shuffle(&mut shuffled);
            if round % 2 == 1 {
                // duplicate delivery of a random subset
                for _ in 0..rng.below(5) {
                    shuffled.push(rng.pick(&ops).clone());
                }
                rng.shuffle(&mut shuffled);
            }
            let got = run_replay(&shuffled);
            assert_eq!(
                got.bytes(),
                want,
                "seed {seed} round {round}: documents diverged"
            );
            assert_eq!(
                got.state.conflicts(),
                want_conflicts,
                "seed {seed} round {round}: conflict reports diverged"
            );
            assert_eq!(
                got.rejected, reference.rejected,
                "seed {seed} round {round}: rejections"
            );
            assert_eq!(
                got.pending, reference.pending,
                "seed {seed} round {round}: pending"
            );
            assert_eq!(
                got.applied, reference.applied,
                "seed {seed} round {round}: applied"
            );
        }
        // accepted + rejected + pending accounts for every distinct op sent
        let distinct = ops.len();
        assert_eq!(
            reference.applied + reference.rejected.len() + reference.pending.len(),
            distinct,
            "seed {seed}: every op is accepted, rejected or pending"
        );
    }
}

#[test]
fn replay_is_idempotent() {
    for seed in 200..230u64 {
        let mut rng = Rng(seed);
        let ops = random_ops(&mut rng, 20);
        let once = run_replay(&ops);
        let mut doubled = ops.clone();
        doubled.extend(ops.iter().cloned());
        let twice = run_replay(&doubled);
        assert_eq!(once.bytes(), twice.bytes());
        assert_eq!(once.state.conflicts(), twice.state.conflicts());
    }
}

// ------------------------------------------------------------ merge rules

#[test]
fn concurrent_property_edits_resolve_by_clock_and_name_the_conflict() {
    let first = op("alice", 10, 0, &[], set("a", "x", json!(1)));
    let second = op("bob", 11, 0, &[], set("a", "x", json!(2)));
    let replayed = run_replay(&[second.clone(), first.clone()]);
    assert_eq!(find(&replayed.document(), "a").unwrap()["x"], json!(2));
    let conflicts = replayed.state.conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].code, "overwritten-edit");
    assert_eq!(conflicts[0].losing_op, first.id);
    assert_eq!(conflicts[0].winning_op.as_deref(), Some(second.id.as_str()));
}

#[test]
fn causally_ordered_edits_do_not_conflict() {
    let first = op("alice", 10, 0, &[], set("a", "x", json!(1)));
    let second = op("bob", 11, 0, &[&first.id], set("a", "x", json!(2)));
    let replayed = run_replay(&[second, first]);
    assert!(replayed.state.conflicts().is_empty());
}

#[test]
fn edits_to_different_properties_both_survive() {
    let one = op("alice", 10, 0, &[], set("a", "x", json!(7)));
    let two = op(
        "bob",
        10,
        0,
        &[],
        set("a", "style/fill/fallback", json!("#ff0000")),
    );
    let replayed = run_replay(&[one, two]);
    let doc = replayed.document();
    let a = find(&doc, "a").unwrap();
    assert_eq!(a["x"], json!(7));
    assert_eq!(a["style"]["fill"]["fallback"], json!("#ff0000"));
    assert!(replayed.state.conflicts().is_empty());
}

#[test]
fn delete_beats_a_concurrent_edit_and_reports_it() {
    let edit = op("alice", 10, 0, &[], set("b", "x", json!(99)));
    let del = op("bob", 9, 0, &[], delete("b"));
    for ops in [[edit.clone(), del.clone()], [del.clone(), edit.clone()]] {
        let replayed = run_replay(&ops);
        assert!(find(&replayed.document(), "b").is_none());
        assert!(codes(&replayed).contains(&"edit-on-deleted".to_string()));
    }
}

#[test]
fn concurrent_reparenting_never_creates_a_cycle() {
    let make = op("alice", 5, 0, &[], create("h", "l", "content", "t"));
    let into_g = op("alice", 6, 0, &[&make.id], mv("g", "n", "h", "m"));
    let into_h = op("bob", 6, 1, &[&make.id], mv("h", "n", "g", "m"));
    let replayed = run_replay(&[make, into_g, into_h]);
    let doc = replayed.document();
    // exactly one of the two moves applied; the other is reported as a cycle
    assert!(codes(&replayed).contains(&"reparent-cycle".to_string()));
    let g_inside_h = find(find(&doc, "h").unwrap(), "g").is_some();
    let h_inside_g = find(find(&doc, "g").unwrap(), "h").is_some();
    assert!(g_inside_h ^ h_inside_g);
}

#[test]
fn duplicate_creates_keep_one_and_report_the_other() {
    let one = op("alice", 10, 0, &[], create("same", "l", "content", "m"));
    let two = op("bob", 10, 1, &[], create("same", "l", "other", "m"));
    let replayed = run_replay(&[two, one]);
    assert!(codes(&replayed).contains(&"duplicate-id".to_string()));
    let doc = replayed.document();
    assert_eq!(doc.to_string().matches("\"id\":\"same\"").count(), 1);
}

#[test]
fn concurrent_inserts_at_one_position_order_deterministically() {
    let one = op("alice", 10, 0, &[], create("n1", "l", "other", "m"));
    let two = op("bob", 10, 0, &[], create("n2", "l", "other", "m"));
    let forward = run_replay(&[one.clone(), two.clone()]).bytes();
    let backward = run_replay(&[two, one]).bytes();
    assert_eq!(forward, backward);
}

#[test]
fn locked_layer_rejects_concurrent_node_creation() {
    let lock = op(
        "alice",
        10,
        0,
        &[],
        set_layer("other", "locked", json!(true)),
    );
    let add = op("bob", 11, 0, &[&lock.id], create("late", "l", "other", "m"));
    let replayed = run_replay(&[add.clone(), lock]);
    assert!(find(&replayed.document(), "late").is_none());
    assert!(replayed
        .rejected
        .iter()
        .any(|(id, code)| id == &add.id && code == "locked"));
}

#[test]
fn unknown_target_is_rejected_not_ignored() {
    let ghost = op("alice", 10, 0, &[], set("nope", "x", json!(1)));
    let replayed = run_replay(std::slice::from_ref(&ghost));
    assert_eq!(
        replayed.rejected,
        vec![(ghost.id, "unknown-target".to_string())]
    );
}

#[test]
fn missing_dependency_leaves_the_op_pending() {
    let waiting = op("alice", 10, 0, &["op_absent"], set("a", "x", json!(1)));
    let replayed = run_replay(std::slice::from_ref(&waiting));
    assert_eq!(replayed.pending, vec![waiting.id]);
    assert_eq!(replayed.applied, 0);
}

// -------------------------------------------------- malformed and limits

#[test]
fn malformed_and_oversized_operations_are_rejected() {
    assert!(parse_op(b"{not json").is_err());
    let good = op("alice", 1, 0, &[], set("a", "x", json!(1)));
    let mut tampered = serde_json::to_value(&good).unwrap();
    tampered["payload"]["value"] = json!(2);
    let error = parse_op(tampered.to_string().as_bytes())
        .unwrap_err()
        .to_string();
    assert!(error.contains("[bad-id]"), "{error}");
    let mut bad_schema = serde_json::to_value(&good).unwrap();
    bad_schema["schema"] = json!(99);
    assert!(parse_op(bad_schema.to_string().as_bytes())
        .unwrap_err()
        .to_string()
        .contains("[unsupported-op]"));
    let huge = Op::build(
        set("a", "x", json!("y".repeat(300 * 1024))),
        &actor_id("alice"),
        Hlc {
            wall_ms: 1,
            counter: 0,
            peer: "p".into(),
        },
        "sha256:test",
        Vec::new(),
        "test",
    );
    assert!(huge.unwrap_err().to_string().contains("[limit-exceeded]"));
    assert!(parse_op(&vec![b' '; 400 * 1024]).is_err());
}

// ------------------------------------------------------------- CLI hook

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-collab-{label}-{}-{}",
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

fn put_shape(id: &str) -> Value {
    json!([{"type":"put-shape","shape":"rect","id":id,"layer":"content","x":1,"y":1,"width":2,"height":2}])
}

fn fixture(ws: &Workspace) -> String {
    let doc = ws.path("scene.pen");
    fs::copy("docs/fixtures/v4-scene.pen", &doc).unwrap();
    doc
}

#[test]
fn collab_is_a_no_op_until_initialised() {
    let ws = Workspace::new("noop");
    let doc = fixture(&ws);
    let out = batch(&ws, &doc, "ops.json", put_shape("r1"), &[]);
    assert!(out.status.success());
    assert!(!ws.0.join(".pentool").join("collab").exists());
    let log = run(&["collab", "log", &doc]);
    assert!(!log.status.success());
    assert!(String::from_utf8_lossy(&log.stderr).contains("collab init"));
}

#[test]
fn cli_mutations_are_recorded_and_replay_matches_the_document() {
    let ws = Workspace::new("record");
    let doc = fixture(&ws);
    let init = ok_json(&["collab", "init", &doc]);
    assert_eq!(init["enabled"], json!(true));
    assert!(batch(&ws, &doc, "o1.json", put_shape("r1"), &[])
        .status
        .success());
    assert!(batch(&ws, &doc, "o2.json", put_shape("r2"), &[])
        .status
        .success());

    let log = ok_json(&["collab", "log", &doc, "--limit", "1"]);
    assert_eq!(log["returned"], json!(1));
    assert_eq!(log["limit"], json!(1));
    assert_eq!(log["has_more"], json!(true));
    let total = log["matches"].as_u64().unwrap();
    assert!(total >= 2);
    let rest = ok_json(&["collab", "log", &doc, "--offset", "1", "--limit", "500"]);
    assert_eq!(rest["matches"].as_u64().unwrap(), total);
    assert_eq!(rest["returned"].as_u64().unwrap(), total - 1);
    assert_eq!(rest["has_more"], json!(false));
    let actor = log["operations"][0]["actor"].as_str().unwrap().to_string();
    assert!(actor.starts_with("act_"));
    let filtered = ok_json(&["collab", "log", &doc, "--actor", "act_nobody"]);
    assert_eq!(filtered["matches"], json!(0));

    let verify = ok_json(&["collab", "verify", &doc]);
    assert_eq!(verify["chain"], json!("valid"));
    assert_eq!(verify["replay"]["matches_document"], json!(true));

    let rebuilt = ws.path("rebuilt.pen");
    let replay = ok_json(&["collab", "replay", &doc, "--out", &rebuilt]);
    assert_eq!(replay["matches_document"], json!(true));
    let mut a: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    // the embedded attribution block is derived, not replayed content
    a.as_object_mut().unwrap().remove("collab");
    let b: Value = serde_json::from_slice(&fs::read(&rebuilt).unwrap()).unwrap();
    assert_eq!(a, b);
    // replay must never overwrite the document
    let refused = run(&["collab", "replay", &doc, "--out", &doc]);
    assert!(!refused.status.success());
}

#[test]
fn agent_actor_flags_are_recorded() {
    let ws = Workspace::new("agent");
    let doc = fixture(&ws);
    ok_json(&["collab", "init", &doc]);
    let out = batch(
        &ws,
        &doc,
        "o.json",
        put_shape("by-agent"),
        &[
            "--actor-kind",
            "agent",
            "--actor-id",
            "bot-1",
            "--actor-name",
            "Bot",
            "--directed-by",
            "alice",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = ok_json(&["collab", "log", &doc]);
    let agent = Actor::derive_id("agent", "bot-1");
    assert!(log["operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["actor"] == json!(agent)));
    let registry = log["actors"].as_array().unwrap();
    assert!(registry.iter().any(|a| a["id"] == json!(agent)
        && a["kind"] == json!("agent")
        && a["directed_by"].is_string()));
    // an agent without a director is refused and the document is untouched
    let before = fs::read(&doc).unwrap();
    let refused = batch(
        &ws,
        &doc,
        "o2.json",
        put_shape("orphan"),
        &["--actor-kind", "agent", "--actor-id", "bot-2"],
    );
    assert!(!refused.status.success());
    assert_eq!(fs::read(&doc).unwrap(), before);
}

#[test]
fn failed_mutation_leaves_document_and_log_unchanged() {
    let ws = Workspace::new("rollback");
    let doc = fixture(&ws);
    ok_json(&["collab", "init", &doc]);
    let before = fs::read(&doc).unwrap();
    let log_before = ok_json(&["collab", "log", &doc])["matches"].clone();
    let out = batch(
        &ws,
        &doc,
        "bad.json",
        json!([{"type":"put-shape","shape":"rect","id":"x","layer":"missing-layer","x":1,"y":1,"width":2,"height":2}]),
        &[],
    );
    assert!(!out.status.success());
    assert_eq!(fs::read(&doc).unwrap(), before);
    assert_eq!(ok_json(&["collab", "log", &doc])["matches"], log_before);
}

#[test]
fn tampered_chain_is_detected() {
    let ws = Workspace::new("tamper");
    let doc = fixture(&ws);
    ok_json(&["collab", "init", &doc]);
    assert!(batch(&ws, &doc, "o1.json", put_shape("r1"), &[])
        .status
        .success());
    assert!(batch(&ws, &doc, "o2.json", put_shape("r2"), &[])
        .status
        .success());
    ok_json(&["collab", "verify", &doc]);

    let log_file =
        ws.0.join(".pentool")
            .join("collab")
            .join("scene.pen")
            .join("log.jsonl");
    let original = fs::read_to_string(&log_file).unwrap();
    assert!(!original.is_empty());

    // 1) edit a payload byte
    fs::write(&log_file, original.replacen("r1", "rX", 1)).unwrap();
    let out = run(&["collab", "verify", &doc]);
    assert!(!out.status.success(), "edited line must fail verification");

    // 2) drop the first line (broken chain)
    let dropped: String = original.lines().skip(1).map(|l| format!("{l}\n")).collect();
    fs::write(&log_file, dropped).unwrap();
    assert!(!run(&["collab", "verify", &doc]).status.success());

    // 3) swap two lines
    let mut lines: Vec<&str> = original.lines().collect();
    assert!(lines.len() >= 2);
    lines.swap(0, 1);
    fs::write(&log_file, lines.join("\n") + "\n").unwrap();
    assert!(!run(&["collab", "verify", &doc]).status.success());

    // restoring the original makes it valid again
    fs::write(&log_file, original).unwrap();
    ok_json(&["collab", "verify", &doc]);
}

#[test]
fn undo_is_recorded_so_the_chain_stays_replayable() {
    let ws = Workspace::new("undo");
    let doc = fixture(&ws);
    ok_json(&["collab", "init", &doc]);
    assert!(batch(&ws, &doc, "o1.json", put_shape("r1"), &[])
        .status
        .success());
    assert!(run(&["undo", &doc]).status.success());
    let verify = ok_json(&["collab", "verify", &doc]);
    assert_eq!(verify["replay"]["matches_document"], json!(true));
    assert!(!fs::read_to_string(&doc).unwrap().contains("\"r1\""));
}

#[test]
fn out_of_band_edits_are_reconciled_by_a_catch_up_entry() {
    let ws = Workspace::new("catchup");
    let doc = fixture(&ws);
    ok_json(&["collab", "init", &doc]);
    let edited = fs::read_to_string(&doc)
        .unwrap()
        .replace("Semantic scene", "Edited outside");
    fs::write(&doc, edited).unwrap();
    assert!(batch(&ws, &doc, "o1.json", put_shape("r1"), &[])
        .status
        .success());
    let verify = ok_json(&["collab", "verify", &doc]);
    assert_eq!(verify["replay"]["matches_document"], json!(true));
    let log = ok_json(&["collab", "log", &doc]);
    assert!(log["operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["actor"] == json!("act_external")));
}

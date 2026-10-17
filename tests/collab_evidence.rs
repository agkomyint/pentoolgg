//! Collaboration evidence: seeded convergence sweeps over simulated peers,
//! a hostile-operation corpus, crash injection at every durability boundary,
//! stale-lock recovery and a never-panic smoke run of the fuzz entry points.
//!
//! Every invariant here is listed, with its test, in `docs/collaboration-v1.md`
//! ("Evidence and invariants").
use pentool::collab::evidence::{
    actor_id, create, delete, fuzz_log, fuzz_op, fuzz_replay, fuzz_sync_request, genesis, make_op,
    mv, random_ops, set, set_layer, Rng, HANDLES,
};
use pentool::collab::log::{self, CrashPoint};
use pentool::collab::merge::replay;
use pentool::collab::op::{Actor, Op};
use pentool::collab::sync::{
    build_actor, decode_ops_body, inject_fault, integrate, Fault, Limits, Origin, Outcome,
};
use pentool::history::{self, CrashPoint as HistoryCrash};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

// ------------------------------------------------------------ test helpers

struct Workspace(PathBuf);
impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-evidence-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn actors() -> Vec<Actor> {
    HANDLES
        .iter()
        .map(|handle| build_actor("human", handle, None, None).unwrap())
        .collect()
}

/// A collaborative document initialised from `genesis`.
fn new_doc(ws: &Workspace, name: &str, genesis: &Value) -> PathBuf {
    let doc = ws.path(name);
    fs::write(&doc, serde_json::to_vec_pretty(genesis).unwrap()).unwrap();
    pentool::collab::init(&doc).expect("collab init");
    doc
}

fn raw(ops: &[Op]) -> Vec<Value> {
    ops.iter()
        .map(|op| serde_json::to_value(op).unwrap())
        .collect()
}

fn deliver_raw(doc: &Path, ops: Vec<Value>, origin: Origin<'_>) -> Outcome {
    integrate(doc, ops, &actors(), origin, &Limits::default()).expect("integrate")
}

fn deliver(doc: &Path, ops: &[Op]) -> Outcome {
    deliver_raw(doc, raw(ops), Origin::Pull)
}

fn head_seq(doc: &Path) -> u64 {
    log::read_head(&log::dir(doc).unwrap()).unwrap().seq
}

/// The document without the peer-local chain position (`collab.head`/`seq`),
/// which legitimately differs between converged peers.
fn converged(doc: &Path) -> Value {
    let mut value = read_doc(doc);
    if let Some(block) = value.get_mut("collab").and_then(Value::as_object_mut) {
        block.remove("head");
        block.remove("seq");
    }
    value
}

fn log_ids(doc: &Path) -> BTreeSet<String> {
    let dir = log::dir(doc).unwrap();
    log::read_lines(&dir)
        .unwrap()
        .into_iter()
        .map(|line| line.op.id)
        .collect()
}

fn history_entries(doc: &Path) -> usize {
    history::list(doc).unwrap()["entries"]
        .as_array()
        .unwrap()
        .len()
}

/// The log verifies and replaying it reproduces the document byte for byte.
fn assert_verified(doc: &Path) {
    let report = pentool::collab::verify_report(doc).expect("collab verify");
    assert_eq!(report["chain"], json!("valid"), "{report}");
    assert_eq!(
        report["replay"]["matches_document"],
        json!(true),
        "{report}"
    );
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

fn count_id(doc: &Value, id: &str) -> usize {
    match doc {
        Value::Object(map) => {
            usize::from(map.get("id").and_then(Value::as_str) == Some(id))
                + map.values().map(|v| count_id(v, id)).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(|v| count_id(v, id)).sum(),
        _ => 0,
    }
}

fn depth(doc: &Value) -> usize {
    match doc {
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn finite_numbers(doc: &Value) -> bool {
    match doc {
        Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
        Value::Object(map) => map.values().all(finite_numbers),
        Value::Array(items) => items.iter().all(finite_numbers),
        _ => true,
    }
}

fn read_doc(doc: &Path) -> Value {
    serde_json::from_slice(&fs::read(doc).unwrap()).unwrap()
}

/// Names of crash debris anywhere under the workspace: staged appends, temp
/// files, moved-aside document copies and lock files.
fn leftovers(ws: &Workspace) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                walk(&path, out);
            } else if name == "append.tmp"
                || name == "lock"
                || name.ends_with(".tmp")
                || name.contains(".pentool-tmp-")
                || name.contains(".pentool-old-")
                || name.contains(".stale-")
            {
                out.push(path.to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(&ws.0, &mut out);
    out
}

fn age(path: &Path, by: Duration) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - by).unwrap();
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ------------------------------------------------------- convergence sweep

/// Invariant C1: peers that exchange operations in any order, with offline
/// partitions, duplicate delivery and reordering, end with byte-identical
/// documents, identical operation sets and a verifying log.
#[test]
fn simulated_peers_converge_through_partitions_and_reordering() {
    for seed in 0..8u64 {
        let ws = Workspace::new(&format!("sweep-{seed}"));
        let mut rng = Rng(seed.wrapping_mul(104_729) + 7);
        let peer_count = 3 + rng.below(3);
        let count = 6 + rng.below(10);
        let ops = random_ops(&mut rng, count);
        let docs: Vec<PathBuf> = (0..peer_count)
            .map(|n| new_doc(&ws, &format!("peer{n}.pen"), &genesis()))
            .collect();
        let mut has: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); peer_count];

        // Each operation is authored on one peer while the others are away.
        for (index, op) in ops.iter().enumerate() {
            let author = rng.below(peer_count);
            deliver(&docs[author], std::slice::from_ref(op));
            has[author].insert(index);
        }

        for _round in 0..5 {
            let offline: Vec<bool> = (0..peer_count).map(|_| rng.below(3) == 0).collect();
            for _ in 0..3 {
                let (from, to) = (rng.below(peer_count), rng.below(peer_count));
                if from == to || offline[from] || offline[to] {
                    continue;
                }
                let mut batch: Vec<usize> = has[from]
                    .iter()
                    .copied()
                    .filter(|_| rng.below(10) < 7)
                    .collect();
                for _ in 0..rng.below(3) {
                    if let Some(extra) = has[from]
                        .iter()
                        .copied()
                        .nth(rng.below(has[from].len().max(1)))
                    {
                        batch.push(extra); // duplicate delivery
                    }
                }
                rng.shuffle(&mut batch);
                let batch_ops: Vec<Op> = batch.iter().map(|i| ops[*i].clone()).collect();
                for chunk in batch_ops.chunks(4) {
                    deliver(&docs[to], chunk);
                }
                has[to].extend(batch);
            }
        }

        // Heal: every peer receives everything every other peer holds.
        for from in 0..peer_count {
            for to in 0..peer_count {
                if from == to {
                    continue;
                }
                let mut batch: Vec<usize> = has[from].iter().copied().collect();
                rng.shuffle(&mut batch);
                let batch_ops: Vec<Op> = batch.iter().map(|i| ops[*i].clone()).collect();
                deliver(&docs[to], &batch_ops);
                has[to].extend(batch);
            }
        }

        let want = converged(&docs[0]);
        let want_ids = log_ids(&docs[0]);
        assert_eq!(
            want_ids.len(),
            ops.len(),
            "seed {seed}: ops lost or invented"
        );
        for doc in &docs {
            assert_eq!(converged(doc), want, "seed {seed}: documents diverged");
            assert_eq!(
                log_ids(doc),
                want_ids,
                "seed {seed}: operation sets diverged"
            );
            assert_verified(doc);
        }
        // The pure replay of the whole set agrees with what the peers hold.
        let pure = replay(&genesis(), &ops).unwrap();
        let mut held = want.clone();
        held.as_object_mut().unwrap().remove("collab");
        assert_eq!(
            held,
            pure.document(),
            "seed {seed}: peers differ from a pure replay"
        );
        assert!(leftovers(&ws).is_empty(), "{:?}", leftovers(&ws));
    }
}

/// Invariant C2: delivery order never changes the result, even when two
/// concurrent reparents would form a cycle.
#[test]
fn concurrent_reparents_and_deletes_converge_in_both_orders() {
    let ws = Workspace::new("reparent");
    let a = make_op("alice", 10, 0, &[], mv("a", "n", "b", "m"));
    let b = make_op("bob", 10, 1, &[], mv("b", "n", "a", "m"));
    let c = make_op("carol", 11, 0, &[], delete("a"));
    let d = make_op("alice", 12, 0, &[], set("a", "x", json!(9)));
    let forward = new_doc(&ws, "forward.pen", &genesis());
    let backward = new_doc(&ws, "backward.pen", &genesis());
    let all = [a, b, c, d];
    deliver(&forward, &all);
    let reversed: Vec<Op> = all.iter().rev().cloned().collect();
    for op in &reversed {
        deliver(&backward, std::slice::from_ref(op));
    }
    assert_eq!(converged(&forward), converged(&backward));
    assert_verified(&forward);
    assert_verified(&backward);
}

// ----------------------------------------------------------- hostile corpus

fn hostile_setup(name: &str) -> (Workspace, PathBuf) {
    let ws = Workspace::new(name);
    let doc = new_doc(&ws, "doc.pen", &genesis());
    (ws, doc)
}

/// Deliver `ops`, then require the batch changed neither the document nor the log.
fn assert_inert(doc: &Path, ops: Vec<Value>, origin: Origin<'_>, code: &str) -> Outcome {
    let (bytes, seq) = (fs::read(doc).unwrap(), head_seq(doc));
    let outcome = deliver_raw(doc, ops, origin);
    assert!(
        outcome.accepted.is_empty(),
        "{code}: hostile operation was accepted: {outcome:?}"
    );
    assert!(
        outcome.rejected.iter().any(|r| r.code == code),
        "expected rejection code {code}, got {:?}",
        outcome.rejected
    );
    assert_eq!(fs::read(doc).unwrap(), bytes, "{code}: document changed");
    assert_eq!(head_seq(doc), seq, "{code}: log changed");
    assert_verified(doc);
    outcome
}

fn mutated(op: &Op, edit: impl FnOnce(&mut Value)) -> Value {
    let mut value = serde_json::to_value(op).unwrap();
    edit(&mut value);
    value
}

/// Invariant H1: oversized operations are rejected before any state is read.
#[test]
fn hostile_oversized_fields_are_rejected() {
    let (_ws, doc) = hostile_setup("oversized");
    let op = make_op("alice", 10, 0, &[], set("a", "x", json!(1)));
    let huge = mutated(&op, |v| v["payload"]["value"] = json!("x".repeat(300_000)));
    assert_inert(&doc, vec![huge], Origin::Pull, "limit-exceeded");
    // A long path, a long key and a huge dependency list.
    let long_path = mutated(&op, |v| v["payload"]["path"] = json!("p/".repeat(40)));
    let outcome = deliver_raw(&doc, vec![long_path], Origin::Pull);
    assert!(outcome.accepted.is_empty() && !outcome.rejected.is_empty());
    let deps = mutated(&op, |v| {
        v["deps"] = json!((0..200).map(|n| format!("op_{n}")).collect::<Vec<_>>())
    });
    let outcome = deliver_raw(&doc, vec![deps], Origin::Pull);
    assert!(outcome.accepted.is_empty() && !outcome.rejected.is_empty());
    assert_verified(&doc);
}

/// Invariant H2: a batch over the operation count limit is refused whole.
#[test]
fn hostile_oversized_batch_is_refused_before_any_work() {
    let (_ws, doc) = hostile_setup("batch");
    let op = make_op("alice", 10, 0, &[], set("a", "x", json!(1)));
    let limits = Limits::default();
    let flood = vec![serde_json::to_value(&op).unwrap(); limits.batch_ops + 1];
    let before = fs::read(&doc).unwrap();
    let error = integrate(&doc, flood, &actors(), Origin::Pull, &limits).unwrap_err();
    assert!(error.to_string().contains("limit-exceeded"), "{error}");
    assert_eq!(fs::read(&doc).unwrap(), before);
    assert_eq!(head_seq(&doc), 0);
}

/// Invariant H3: huge identifiers are rejected, never stored.
#[test]
fn hostile_huge_ids_are_rejected() {
    let (_ws, doc) = hostile_setup("hugeid");
    let op = make_op("alice", 10, 0, &[], set("a", "x", json!(1)));
    for edit in [
        mutated(&op, |v| v["targets"] = json!(["i".repeat(10_000)])),
        mutated(&op, |v| v["id"] = json!("o".repeat(10_000))),
        mutated(&op, |v| v["page"] = json!("p".repeat(10_000))),
        mutated(&op, |v| v["actor"] = json!("a".repeat(10_000))),
    ] {
        let (bytes, seq) = (fs::read(&doc).unwrap(), head_seq(&doc));
        let outcome = deliver_raw(&doc, vec![edit], Origin::Pull);
        assert!(outcome.accepted.is_empty(), "{outcome:?}");
        assert!(!outcome.rejected.is_empty(), "{outcome:?}");
        assert_eq!(fs::read(&doc).unwrap(), bytes);
        assert_eq!(head_seq(&doc), seq);
    }
    assert_verified(&doc);
}

/// Invariant H4: deeply nested payloads cannot grow the document past the
/// depth bound, and never break validation.
#[test]
fn hostile_deep_nesting_is_bounded() {
    let (_ws, doc) = hostile_setup("deep");
    let mut node = json!({"kind": "rect", "id": "leaf", "x": 0, "y": 0, "width": 1, "height": 1});
    for level in 0..200 {
        node = json!({"kind": "group", "id": format!("d{level}"), "children": [node]});
    }
    let draft = pentool::collab::op::Draft {
        page: "page-1".into(),
        kind: "create",
        targets: vec!["d199".into()],
        payload: json!({
            "level": "n", "key": "m",
            "parent": {"level": "l", "id": "content"},
            "node": node
        }),
    };
    let built = Op::build(
        draft,
        &actor_id("alice"),
        pentool::collab::hlc::Hlc {
            wall_ms: 10,
            counter: 0,
            peer: "peer-alice".into(),
        },
        "sha256:test",
        Vec::new(),
        "test",
    );
    if let Ok(op) = built {
        deliver(&doc, &[op]);
    }
    let value = read_doc(&doc);
    pentool::transaction::validate_value(&value).expect("document stays valid");
    assert!(depth(&value) < 140, "nesting grew to {}", depth(&value));
    assert_verified(&doc);
}

/// Invariant H5: two creations of one id leave exactly one node and a
/// `duplicate-id` conflict, whatever the arrival order.
#[test]
fn hostile_duplicate_ids_keep_one_node() {
    let ws = Workspace::new("dup");
    let first = make_op("alice", 10, 0, &[], create("dup", "l", "content", "m"));
    let second = make_op("bob", 11, 0, &[], create("dup", "l", "other", "m"));
    let forward = new_doc(&ws, "f.pen", &genesis());
    let backward = new_doc(&ws, "b.pen", &genesis());
    let outcome = deliver(&forward, &[first.clone(), second.clone()]);
    deliver(&backward, std::slice::from_ref(&second));
    deliver(&backward, std::slice::from_ref(&first));
    assert_eq!(count_id(&read_doc(&forward), "dup"), 1);
    assert_eq!(converged(&forward), converged(&backward));
    assert!(
        outcome.conflicts.iter().any(|c| c.code == "duplicate-id"),
        "{:?}",
        outcome.conflicts
    );
    assert_verified(&forward);
}

/// Invariant H6: ids that differ only by unicode normalization never produce
/// an invalid document or an order-dependent one. Ids compare as exact strings.
#[test]
fn hostile_unicode_normalization_collisions_are_deterministic() {
    let ws = Workspace::new("unicode");
    let composed = "caf\u{e9}";
    let decomposed = "cafe\u{301}";
    let one = make_op("alice", 10, 0, &[], create(composed, "l", "content", "m"));
    let two = make_op("bob", 11, 0, &[], create(decomposed, "l", "content", "t"));
    let forward = new_doc(&ws, "f.pen", &genesis());
    let backward = new_doc(&ws, "b.pen", &genesis());
    deliver(&forward, &[one.clone(), two.clone()]);
    deliver(&backward, &[two, one]);
    assert_eq!(converged(&forward), converged(&backward));
    let value = read_doc(&forward);
    pentool::transaction::validate_value(&value).expect("document stays valid");
    assert!(count_id(&value, composed) <= 1 && count_id(&value, decomposed) <= 1);
    assert_verified(&forward);
}

/// Invariant H7: non-finite and absurd numbers never reach the document.
#[test]
fn hostile_non_finite_numbers_are_rejected() {
    let limits = Limits::default();
    for body in [
        br#"{"ops":[{"payload":{"value":NaN}}]}"#.as_slice(),
        br#"{"ops":[{"payload":{"value":Infinity}}]}"#.as_slice(),
        br#"{"ops":[{"payload":{"value":1e999}}]}"#.as_slice(),
    ] {
        assert!(decode_ops_body(body, &limits).is_err(), "{body:?}");
    }
    let (_ws, doc) = hostile_setup("numbers");
    // A non-numeric stand-in for a number makes the merged document invalid:
    // the whole batch is refused and nothing is stored.
    let (bytes, seq) = (fs::read(&doc).unwrap(), head_seq(&doc));
    let op = make_op("alice", 10, 0, &[], set("a", "x", json!("NaN")));
    let refused = integrate(
        &doc,
        raw(&[op]),
        &actors(),
        Origin::Pull,
        &Limits::default(),
    );
    assert!(refused.is_err());
    assert_eq!(fs::read(&doc).unwrap(), bytes);
    assert_eq!(head_seq(&doc), seq);
    // Finite extremes are legal numbers and stay finite.
    let big = make_op("alice", 11, 0, &[], set("a", "width", json!(1e308)));
    let neg = make_op("alice", 12, 0, &[], set("a", "height", json!(-1e308)));
    let _ = integrate(
        &doc,
        raw(&[big, neg]),
        &actors(),
        Origin::Pull,
        &Limits::default(),
    );
    let value = read_doc(&doc);
    assert!(finite_numbers(&value));
    pentool::transaction::validate_value(&value).expect("document stays valid");
    assert_verified(&doc);
}

/// Invariant H8: a lock set by an operation protects its layer from later
/// edits at apply time; the edit is reported, not applied.
#[test]
fn hostile_edits_on_locked_layers_are_not_applied() {
    let (_ws, doc) = hostile_setup("locked");
    let lock = make_op(
        "alice",
        10,
        0,
        &[],
        set_layer("content", "locked", json!(true)),
    );
    let edit = make_op(
        "bob",
        20,
        0,
        std::slice::from_ref(&lock.id),
        set("a", "x", json!(7)),
    );
    let outcome = deliver(&doc, &[lock, edit.clone()]);
    let blocked = outcome
        .merge_rejected
        .iter()
        .chain(outcome.rejected.iter())
        .any(|r| r.id == edit.id && r.code == "locked");
    assert!(blocked, "{outcome:?}");
    assert_eq!(find(&read_doc(&doc), "a").unwrap()["x"], json!(0));
    assert_verified(&doc);
}

/// Invariant H9: layers attached to a component instance reject direct edits.
#[test]
fn hostile_edits_on_instance_protected_layers_are_not_applied() {
    let mut protected = genesis();
    protected["instances"] =
        json!([{"id": "inst-1", "page_id": "page-1", "layer_ids": ["other"], "component": "c"}]);
    let ws = Workspace::new("instance");
    let doc = ws.path("doc.pen");
    fs::write(&doc, serde_json::to_vec_pretty(&protected).unwrap()).unwrap();
    if pentool::collab::init(&doc).is_err() {
        // The scene validator needs a full component to accept an instance;
        // protection is then exercised through the merge engine directly.
        let op = make_op("alice", 10, 0, &[], create("sneak", "l", "other", "m"));
        let result = replay(&protected, &[op]);
        if let Ok(result) = result {
            assert!(find(&result.document(), "sneak").is_none());
        }
        return;
    }
    let sneak = make_op("alice", 10, 0, &[], create("sneak", "l", "other", "m"));
    let outcome = deliver(&doc, std::slice::from_ref(&sneak));
    assert!(
        outcome
            .merge_rejected
            .iter()
            .chain(outcome.rejected.iter())
            .any(|r| r.id == sneak.id && r.code == "locked"),
        "{outcome:?}"
    );
    assert!(find(&read_doc(&doc), "sneak").is_none());
    assert_verified(&doc);
}

/// Invariant H10: a reparent that would put a node under its own descendant
/// is rejected with `reparent-cycle` and leaves the tree acyclic.
#[test]
fn hostile_cycle_forming_reparent_is_rejected() {
    let (_ws, doc) = hostile_setup("cycle");
    let cycle = make_op("alice", 10, 0, &[], mv("g", "n", "g1", "m"));
    let outcome = deliver(&doc, std::slice::from_ref(&cycle));
    let flagged = outcome.conflicts.iter().any(|c| c.code == "reparent-cycle")
        || outcome
            .merge_rejected
            .iter()
            .any(|r| r.id == cycle.id && r.code == "reparent-cycle");
    assert!(flagged, "{outcome:?}");
    let value = read_doc(&doc);
    let g = find(&value, "g").unwrap();
    assert!(find(g, "g1").is_some(), "g1 must remain a descendant of g");
    assert_verified(&doc);
}

/// Invariant H11: the server never accepts an operation by another actor than
/// the token's, and never one by an actor with no identity record.
#[test]
fn hostile_forged_and_unknown_actors_are_rejected() {
    let (_ws, doc) = hostile_setup("forged");
    let people = actors();
    let alice = people.iter().find(|a| a.id == actor_id("alice")).unwrap();
    let by_bob = make_op("bob", 10, 0, &[], set("a", "x", json!(1)));
    assert_inert(
        &doc,
        raw(std::slice::from_ref(&by_bob)),
        Origin::Push { actor: alice },
        "actor-mismatch",
    );
    let stranger = make_op("mallory", 11, 0, &[], set("a", "x", json!(2)));
    let (bytes, seq) = (fs::read(&doc).unwrap(), head_seq(&doc));
    let outcome = integrate(
        &doc,
        raw(std::slice::from_ref(&stranger)),
        &people,
        Origin::Pull,
        &Limits::default(),
    )
    .unwrap();
    assert!(
        outcome.rejected.iter().any(|r| r.code == "unknown-actor"),
        "{outcome:?}"
    );
    assert_eq!(fs::read(&doc).unwrap(), bytes);
    assert_eq!(head_seq(&doc), seq);
}

/// Invariant H12: a clock more than ten minutes ahead is rejected, so a
/// far-future stamp can never win every later conflict.
#[test]
fn hostile_far_future_clocks_are_rejected() {
    let (_ws, doc) = hostile_setup("future");
    for wall in [
        now_ms() + 60 * 60 * 1000,
        now_ms() + 11 * 60 * 1000,
        u64::MAX / 2,
    ] {
        let op = make_op("alice", wall, 0, &[], set("a", "x", json!(5)));
        assert_inert(&doc, raw(&[op]), Origin::Pull, "clock-skew");
    }
}

/// Invariant H13: replaying already-integrated operations, in any order, is a
/// no-op for the document and the log.
#[test]
fn hostile_replay_of_old_operations_is_a_no_op() {
    let (_ws, doc) = hostile_setup("replayed");
    let ops: Vec<Op> = vec![
        make_op("alice", 10, 0, &[], set("a", "x", json!(1))),
        make_op("bob", 11, 0, &[], create("n1", "l", "content", "m")),
        make_op("carol", 12, 0, &[], delete("b")),
    ];
    deliver(&doc, &ops);
    let (bytes, seq) = (fs::read(&doc).unwrap(), head_seq(&doc));
    let entries = history_entries(&doc);
    let mut rng = Rng(99);
    for _ in 0..5 {
        let mut again = ops.clone();
        rng.shuffle(&mut again);
        let outcome = deliver(&doc, &again);
        assert!(outcome.accepted.is_empty());
        assert_eq!(outcome.duplicate.len(), ops.len());
    }
    assert_eq!(fs::read(&doc).unwrap(), bytes);
    assert_eq!(head_seq(&doc), seq);
    assert_eq!(history_entries(&doc), entries);
    assert_verified(&doc);
}

// ------------------------------------------------------------ crash recovery

#[derive(Debug, Clone, Copy)]
enum Crash {
    Sync(Fault),
    Log(CrashPoint),
    History(HistoryCrash),
    /// Killed between the two renames of the document write: the old copy is
    /// moved aside, the new bytes sit in the temp file, the document is gone.
    DocumentMidRename,
    /// The write-ahead record itself is cut short.
    StagedRecordTorn,
}

const CRASHES: [Crash; 11] = [
    Crash::Sync(Fault::AfterPrepare),
    Crash::StagedRecordTorn,
    Crash::History(HistoryCrash::AfterStore),
    Crash::History(HistoryCrash::AfterManifestTemp),
    Crash::DocumentMidRename,
    Crash::Sync(Fault::AfterDocument),
    Crash::Log(CrashPoint::AfterAtomicTemp),
    Crash::Log(CrashPoint::AfterActors),
    Crash::Log(CrashPoint::TornLine),
    Crash::Log(CrashPoint::AfterLines),
    Crash::Log(CrashPoint::AfterHead),
];

fn crash_batch() -> Vec<Op> {
    let first = make_op("alice", 100, 0, &[], create("c1", "l", "content", "m"));
    let second = make_op(
        "bob",
        101,
        0,
        std::slice::from_ref(&first.id),
        set("c1", "x", json!(3)),
    );
    let third = make_op("carol", 102, 0, &[], set("a", "x", json!(8)));
    vec![first, second, third]
}

fn arm(doc: &Path, crash: Crash) {
    let dir = log::dir(doc).unwrap();
    match crash {
        Crash::Sync(fault) => inject_fault(doc, fault),
        Crash::Log(point) => log::inject_crash(&dir, point),
        Crash::History(point) => history::inject_crash(doc, point),
        Crash::DocumentMidRename | Crash::StagedRecordTorn => {
            inject_fault(doc, Fault::AfterPrepare)
        }
    }
}

/// Leave the files a killed process would, beyond what the hook already did.
fn finish_crash(doc: &Path, crash: Crash, new_bytes: &[u8]) {
    let dir = log::dir(doc).unwrap();
    match crash {
        Crash::DocumentMidRename => {
            let name = doc.file_name().unwrap().to_string_lossy().into_owned();
            let parent = doc.parent().unwrap();
            fs::rename(doc, parent.join(format!(".{name}.pentool-old-424242"))).unwrap();
            fs::write(
                parent.join(format!(".{name}.pentool-tmp-424242")),
                new_bytes,
            )
            .unwrap();
        }
        Crash::StagedRecordTorn => {
            let staged = fs::read(dir.join("append.tmp")).unwrap();
            fs::write(dir.join("append.tmp"), &staged[..staged.len() / 2]).unwrap();
        }
        _ => {}
    }
}

/// Invariants D1-D5: at every durability boundary of one integration (staging,
/// history record, document commit, log append, head update) a crash leaves
/// the document entirely old or entirely new; the first operation after
/// restart recovers the log so it verifies, with no temp files left behind; and
/// re-running the interrupted batch ends at the bytes of an uninterrupted run.
#[test]
fn crash_at_every_durability_boundary_recovers_to_the_uninterrupted_result() {
    // The uninterrupted reference.
    let reference_ws = Workspace::new("crash-reference");
    let reference = new_doc(&reference_ws, "doc.pen", &genesis());
    let old_bytes = fs::read(&reference).unwrap();
    deliver(&reference, &crash_batch());
    let new_bytes = fs::read(&reference).unwrap();
    assert_ne!(old_bytes, new_bytes);
    let reference_history = history_entries(&reference);
    let reference_ids = log_ids(&reference);

    for crash in CRASHES {
        let ws = Workspace::new(&format!("crash-{crash:?}").replace(['(', ')', ':', ' '], "-"));
        let doc = new_doc(&ws, "doc.pen", &genesis());
        assert_eq!(
            fs::read(&doc).unwrap(),
            old_bytes,
            "genesis is deterministic"
        );

        arm(&doc, crash);
        let error = integrate(
            &doc,
            raw(&crash_batch()),
            &actors(),
            Origin::Pull,
            &Limits::default(),
        )
        .expect_err(&format!("{crash:?} must interrupt the integration"));
        assert!(
            error.to_string().contains("fault-injected"),
            "{crash:?}: {error}"
        );
        finish_crash(&doc, crash, &new_bytes);

        // Restart: an empty batch performs exactly the recovery step.
        deliver_raw(&doc, Vec::new(), Origin::Pull);
        let recovered = fs::read(&doc).unwrap();
        assert!(
            recovered == old_bytes || recovered == new_bytes,
            "{crash:?}: document is neither entirely old nor entirely new"
        );
        assert_verified(&doc);
        // History debris from a dead writer is cleaned by the next history write.
        let debris: Vec<String> = leftovers(&ws)
            .into_iter()
            .filter(|name| !(matches!(crash, Crash::History(_)) && name.contains("history")))
            .collect();
        assert_eq!(debris, Vec::<String>::new(), "{crash:?}");

        // Rerun the interrupted batch: it converges to the uninterrupted bytes.
        deliver(&doc, &crash_batch());
        assert_eq!(
            fs::read(&doc).unwrap(),
            new_bytes,
            "{crash:?}: bytes differ"
        );
        assert_eq!(log_ids(&doc), reference_ids, "{crash:?}");
        assert_eq!(head_seq(&doc), head_seq(&reference), "{crash:?}");
        assert_eq!(
            history_entries(&doc),
            reference_history,
            "{crash:?}: history"
        );
        assert_verified(&doc);
        assert_eq!(leftovers(&ws), Vec::<String>::new(), "{crash:?}");
    }
}

/// Invariant D6: a crash can be followed by another crash; recovery is
/// idempotent and still converges.
#[test]
fn repeated_crashes_still_converge() {
    let reference_ws = Workspace::new("crash2-reference");
    let reference = new_doc(&reference_ws, "doc.pen", &genesis());
    deliver(&reference, &crash_batch());
    let want = fs::read(&reference).unwrap();

    let ws = Workspace::new("crash2");
    let doc = new_doc(&ws, "doc.pen", &genesis());
    let dir = log::dir(&doc).unwrap();
    for point in [
        CrashPoint::AfterActors,
        CrashPoint::TornLine,
        CrashPoint::AfterLines,
        CrashPoint::AfterHead,
    ] {
        inject_fault(&doc, Fault::AfterDocument);
        let _ = integrate(
            &doc,
            raw(&crash_batch()),
            &actors(),
            Origin::Pull,
            &Limits::default(),
        );
        // Crash again while recovery finishes the append.
        log::inject_crash(&dir, point);
        let _ = integrate(
            &doc,
            Vec::new(),
            &actors(),
            Origin::Pull,
            &Limits::default(),
        );
        // Disarm anything left over by running to completion.
        deliver_raw(&doc, Vec::new(), Origin::Pull);
        assert_verified(&doc);
    }
    deliver(&doc, &crash_batch());
    assert_eq!(fs::read(&doc).unwrap(), want);
    assert_eq!(leftovers(&ws), Vec::<String>::new());
}

/// Regression (found by this suite): a torn final log line — the crash left
/// half a line, no newline — made every later append and recovery fail with
/// `[corrupt-log]`. The uncommitted tail is now cut; damage inside the
/// committed prefix is still an error.
#[test]
fn torn_log_tail_is_cut_but_committed_corruption_is_not() {
    let ws = Workspace::new("torn");
    let doc = new_doc(&ws, "doc.pen", &genesis());
    deliver(&doc, &crash_batch()[..1]);
    let dir = log::dir(&doc).unwrap();
    let log_path = dir.join("log.jsonl");
    let committed = fs::read(&log_path).unwrap();

    // Half of an uncommitted line, with no newline.
    let mut torn = committed.clone();
    torn.extend_from_slice(br#"{"seq":2,"op":{"schema":1,"id":"op_"#);
    fs::write(&log_path, &torn).unwrap();
    deliver_raw(&doc, Vec::new(), Origin::Pull);
    assert_eq!(
        fs::read(&log_path).unwrap(),
        committed,
        "torn tail was not cut"
    );
    assert_verified(&doc);
    deliver(&doc, &crash_batch()[1..2]);
    assert_verified(&doc);

    // Corruption inside the committed prefix must still be reported.
    let mut damaged = fs::read(&log_path).unwrap();
    damaged[10] = b'~';
    fs::write(&log_path, &damaged).unwrap();
    let before = fs::read(&doc).unwrap();
    let error = integrate(
        &doc,
        raw(&crash_batch()[2..]),
        &actors(),
        Origin::Pull,
        &Limits::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("corrupt-log"), "{error}");
    assert_eq!(
        fs::read(&doc).unwrap(),
        before,
        "failed command must not mutate"
    );
}

/// Invariant D7: a failed (not crashed) document commit aborts the staged
/// append and leaves no residue.
#[test]
fn failed_commit_leaves_no_staged_record() {
    let ws = Workspace::new("abort");
    let doc = new_doc(&ws, "doc.pen", &genesis());
    let before = fs::read(&doc).unwrap();
    // Block the document write: a directory where the temp file must go.
    let name = doc.file_name().unwrap().to_string_lossy().into_owned();
    let blocker =
        ws.0.join(format!(".{name}.pentool-tmp-{}", std::process::id()));
    fs::create_dir_all(&blocker).unwrap();
    let result = integrate(
        &doc,
        raw(&crash_batch()),
        &actors(),
        Origin::Pull,
        &Limits::default(),
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&doc).unwrap(), before);
    assert_eq!(head_seq(&doc), 0);
    fs::remove_dir_all(&blocker).unwrap();
    assert!(!log::dir(&doc).unwrap().join("append.tmp").exists());
    assert_verified(&doc);
    deliver(&doc, &crash_batch());
    assert_verified(&doc);
}

// ------------------------------------------------------------- stale locks

fn history_lock_path(doc: &Path) -> PathBuf {
    doc.parent()
        .unwrap()
        .join(".pentool")
        .join("history")
        .join(history::safe_name(doc).unwrap())
        .join("lock")
}

/// Invariant L1: a lock left by a dead process is reclaimed once it is older
/// than the stale bound, in both the collaboration log and the history.
#[test]
fn stale_locks_are_reclaimed() {
    let ws = Workspace::new("stale");
    let doc = new_doc(&ws, "doc.pen", &genesis());
    let dir = log::dir(&doc).unwrap();

    let collab_lock = dir.join("lock");
    fs::write(&collab_lock, b"").unwrap();
    age(
        &collab_lock,
        pentool::stale::STALE_AFTER + Duration::from_secs(5),
    );
    let outcome = deliver(&doc, &crash_batch()[..1]);
    assert_eq!(outcome.accepted.len(), 1);

    fs::create_dir_all(history_lock_path(&doc).parent().unwrap()).unwrap();
    let history_lock = history_lock_path(&doc);
    fs::write(&history_lock, b"").unwrap();
    age(
        &history_lock,
        pentool::stale::STALE_AFTER + Duration::from_secs(5),
    );
    let outcome = deliver(&doc, &crash_batch()[1..]);
    assert!(!outcome.accepted.is_empty());
    assert_verified(&doc);
    assert!(leftovers(&ws).is_empty(), "{:?}", leftovers(&ws));
}

/// Invariant L2: a lock that is not stale is still honoured, so two live
/// processes never write at once.
#[test]
fn fresh_locks_are_still_honoured() {
    let ws = Workspace::new("fresh-lock");
    let doc = new_doc(&ws, "doc.pen", &genesis());
    let dir = log::dir(&doc).unwrap();
    let held = log::lock(&dir).expect("first holder");
    let error = log::lock(&dir)
        .err()
        .expect("second holder must wait and fail");
    assert!(error.to_string().contains("locked by another"), "{error}");
    drop(held);
    assert!(log::lock(&dir).is_ok(), "released lock can be taken again");
}

/// Invariant L3: orphaned `*.<pid>.tmp` files are swept under the lock that
/// every writer holds (collaboration log, history), and the moved-aside and
/// temp copies of an interrupted document write are swept beside the document.
#[test]
fn orphaned_temp_files_are_swept_under_the_lock() {
    let ws = Workspace::new("orphans");
    let doc = new_doc(&ws, "doc.pen", &genesis());
    let dir = log::dir(&doc).unwrap();
    let orphans = [
        dir.join("head.4242.tmp"),
        dir.join("log.4243.tmp"),
        ws.path(".doc.pen.pentool-tmp-4244"),
        ws.path(".doc.pen.pentool-old-4245"),
    ];
    for orphan in &orphans {
        fs::write(orphan, b"x").unwrap();
    }
    // A file that merely looks similar must be left alone.
    let bystander = dir.join("notes.tmp.keep");
    fs::write(&bystander, b"x").unwrap();
    deliver_raw(&doc, Vec::new(), Origin::Pull);
    for orphan in &orphans {
        assert!(!orphan.exists(), "{} survived recovery", orphan.display());
    }
    assert!(bystander.exists());

    // History debris goes at the next history write.
    let root = history_lock_path(&doc).parent().unwrap().to_path_buf();
    fs::create_dir_all(&root).unwrap();
    let debris = root.join("manifest.4246.tmp");
    fs::write(&debris, b"x").unwrap();
    deliver(&doc, &crash_batch()[..1]);
    assert!(!debris.exists(), "history debris survived a history write");
}

// ------------------------------------------------------------ fuzz smoke

fn bytes_of(rng: &mut Rng, max: usize) -> Vec<u8> {
    (0..rng.below(max + 1)).map(|_| rng.next() as u8).collect()
}

/// Invariant F1: the four fuzz entry points never panic on seeded random
/// input or on mutations of valid operations, and replay stays convergent.
/// `cargo fuzz` (`.github/workflows/fuzz.yml`) runs the same functions
/// under a coverage-guided engine.
#[test]
fn fuzz_entry_points_never_panic_on_seeded_input() {
    let mut rng = Rng(0xFEED);
    let valid = serde_json::to_vec(&make_op("alice", 10, 0, &[], set("a", "x", json!(1)))).unwrap();
    for _ in 0..400 {
        fuzz_op(&bytes_of(&mut rng, 600));
        let mut mutated = valid.clone();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(mutated.len());
            mutated[at] = rng.next() as u8;
        }
        fuzz_op(&mutated);
        fuzz_sync_request(&bytes_of(&mut rng, 600));
    }
    let body = json!({"ops": [serde_json::from_slice::<Value>(&valid).unwrap()]}).to_string();
    fuzz_sync_request(body.as_bytes());
    for _ in 0..25 {
        fuzz_replay(&bytes_of(&mut rng, 200));
    }
    for _ in 0..30 {
        fuzz_log(&bytes_of(&mut rng, 200));
    }
}

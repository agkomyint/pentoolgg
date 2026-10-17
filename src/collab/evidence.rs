//! Byte-driven entry points shared by the `fuzz/` targets and the evidence
//! tests in `tests/collab_evidence.rs`.
//!
//! Every function here must never panic on any input; a panic, or a replay
//! that diverges between delivery orders, is a finding.
use super::{
    hlc::Hlc,
    log::{self, Head, Line},
    merge::replay,
    op::{parse_op, Actor, Draft, Op},
    sync::{decode_ops_body, Limits},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

/// Small deterministic generator (splitmix64); no external dependency.
pub struct Rng(pub u64);
impl Rng {
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

/// The fixed genesis document every generated operation targets.
pub fn genesis() -> Value {
    json!({
        "format": "pentool", "version": 4, "name": "evidence",
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

pub const HANDLES: [&str; 5] = ["alice", "bob", "carol", "dave", "erin"];
const KEYS: [&str; 5] = ["a0", "m", "t", "w", "z"];

pub fn actor_id(handle: &str) -> String {
    Actor::derive_id("human", handle)
}

/// Build an operation by `handle` at the given clock.
pub fn make_op(handle: &str, wall: u64, counter: u32, deps: &[String], draft: Draft) -> Op {
    Op::build(
        draft,
        &actor_id(handle),
        Hlc {
            wall_ms: wall,
            counter,
            peer: format!("peer-{handle}"),
        },
        "sha256:test",
        deps.to_vec(),
        "test",
    )
    .expect("generated operations are valid")
}

pub fn set(id: &str, path: &str, value: Value) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "set-property",
        targets: vec![id.into()],
        payload: json!({"level": "n", "path": path, "value": value}),
    }
}

pub fn set_layer(id: &str, path: &str, value: Value) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "set-property",
        targets: vec![id.into()],
        payload: json!({"level": "l", "path": path, "value": value}),
    }
}

pub fn create(id: &str, parent_level: &str, parent: &str, key: &str) -> Draft {
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

pub fn delete(id: &str) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "delete",
        targets: vec![id.into()],
        payload: json!({"level": "n"}),
    }
}

pub fn mv(id: &str, parent_level: &str, parent: &str, key: &str) -> Draft {
    Draft {
        page: "page-1".into(),
        kind: "move",
        targets: vec![id.into()],
        payload: json!({"level": "n", "key": key, "parent": {"level": parent_level, "id": parent}}),
    }
}

/// A random operation set over [`genesis`]: concurrent edits, creates with
/// colliding ids, deletes, reparents (some forming cycles), layer locks and
/// dependencies on earlier or missing operations.
pub fn random_ops(rng: &mut Rng, count: usize) -> Vec<Op> {
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
        let target = rng.pick(&known).clone();
        let draft = match rng.below(7) {
            0 | 1 => set(&target, "x", json!(rng.below(5))),
            2 => set(
                &target,
                "style/fill/fallback",
                json!(format!("#{:06x}", rng.below(4096))),
            ),
            3 => {
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
        ops.push(make_op(handle, wall, counter, &deps, draft));
    }
    ops
}

fn seed_of(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01B3)
    })
}

/// Fuzz target 1: operation parsing and validation. An accepted operation must
/// pass its own check and survive a canonical round trip.
pub fn fuzz_op(bytes: &[u8]) {
    if let Ok(op) = parse_op(bytes) {
        op.check().expect("a parsed operation passes its own check");
        let again = parse_op(&serde_json::to_vec(&op).expect("serializes"))
            .expect("a parsed operation re-parses");
        assert_eq!(again.id, op.id);
    }
}

/// Fuzz target 2: replay of an arbitrary operation set never panics and
/// converges to identical bytes whatever the delivery order or duplication.
pub fn fuzz_replay(bytes: &[u8]) {
    let mut rng = Rng(seed_of(bytes));
    let count = 1 + bytes.first().map_or(0, |b| usize::from(*b) % 24);
    let mut ops = random_ops(&mut rng, count);
    // Mix in operations decoded straight from the input when they parse.
    for chunk in bytes.chunks(97).take(4) {
        if let Ok(op) = parse_op(chunk) {
            ops.push(op);
        }
    }
    let genesis = genesis();
    let reference = replay(&genesis, &ops).expect("replay of a valid genesis succeeds");
    let want = reference.bytes();
    for _ in 0..3 {
        let mut shuffled = ops.clone();
        rng.shuffle(&mut shuffled);
        for _ in 0..rng.below(4) {
            shuffled.push(rng.pick(&ops).clone());
        }
        rng.shuffle(&mut shuffled);
        let got = replay(&genesis, &shuffled).expect("replay succeeds");
        assert_eq!(got.bytes(), want, "delivery order changed the document");
    }
}

static SCRATCH: AtomicU64 = AtomicU64::new(0);

fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pentool-collab-fuzz-{}-{}",
        std::process::id(),
        SCRATCH.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// Fuzz target 3: the log reader, chain verifier and recovery on arbitrary
/// bytes, and on a valid chain tampered at positions chosen by the input.
pub fn fuzz_log(bytes: &[u8]) {
    let dir = scratch_dir();
    let genesis_bytes = serde_json::to_vec(&genesis()).expect("genesis serializes");
    let _ = log::init(&dir, &genesis_bytes);

    // A valid chain over generated operations.
    let mut rng = Rng(seed_of(bytes));
    let count = 1 + rng.below(6);
    let ops = random_ops(&mut rng, count);
    let mut prev = log::genesis_hash(&genesis_bytes);
    let mut valid = Vec::new();
    let mut lines = Vec::new();
    for (index, op) in ops.into_iter().enumerate() {
        let line = Line::new(index as u64 + 1, op, "sha256:test", &prev);
        prev = line.hash.clone();
        valid.extend(serde_json::to_vec(&line).expect("line serializes"));
        valid.push(b'\n');
        lines.push(line);
    }
    let mut body = valid;
    // Tamper: overwrite, truncate or append input-chosen bytes.
    match bytes.first().map_or(0, |b| b % 4) {
        0 => body = bytes.get(1..).unwrap_or_default().to_vec(),
        1 => {
            for pair in bytes.get(1..).unwrap_or_default().chunks(3) {
                if let [at, new, ..] = pair {
                    let at = usize::from(*at) * 7 % body.len().max(1);
                    if let Some(slot) = body.get_mut(at) {
                        *slot = *new;
                    }
                }
            }
        }
        2 => body.truncate(bytes.get(1).map_or(0, |b| usize::from(*b) * 3) % (body.len() + 1)),
        _ => body.extend_from_slice(bytes.get(1..).unwrap_or_default()),
    }
    let head = Head {
        seq: lines.len() as u64,
        hash: lines.last().map_or_else(
            || log::genesis_hash(&genesis_bytes),
            |line| line.hash.clone(),
        ),
    };
    let _ = fs::write(dir.join("log.jsonl"), &body);
    let _ = fs::write(
        dir.join("head"),
        serde_json::to_vec(&head).expect("head serializes"),
    );
    if let Ok(read) = log::read_lines(&dir) {
        let _ = log::verify_chain(&dir, &read);
        let _ = log::check_tail(&dir, &read);
    }
    let _ = log::read_lines_prefix(&dir);
    let recovered = log::recover(&dir, "sha256:test").is_ok();
    // After a successful recovery the reader must agree with itself.
    if let (true, Ok((prefix, torn))) = (recovered, log::read_lines_prefix(&dir)) {
        assert!(torn.is_none(), "recovery left a torn tail");
        let _ = log::verify_chain(&dir, &prefix);
    }
    let _ = fs::remove_dir_all(&dir);
}

/// Fuzz target 4: the sync request decoder. Whatever decodes is fed to the
/// operation parser, as the endpoint does.
pub fn fuzz_sync_request(bytes: &[u8]) {
    let limits = Limits::default();
    if let Ok(raw_ops) = decode_ops_body(bytes, &limits) {
        assert!(raw_ops.len() <= limits.batch_ops);
        for raw in raw_ops {
            if let Ok(encoded) = serde_json::to_vec(&raw) {
                fuzz_op(&encoded);
            }
        }
    }
}

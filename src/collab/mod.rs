//! Collaboration operation log (roadmap v0.20.0 item 2; spec in
//! `docs/collaboration-v1.md`).
//!
//! Collaboration is opt in per document (`pentool collab init DOC`). Once
//! enabled, every document mutation that goes through the shared transaction
//! path (`transaction::commit_bytes`, history undo/redo/restore) is also
//! recorded as intent-level operations in a hash-chained log, and the document
//! can be rebuilt from that log by deterministic replay. Documents without a
//! log are untouched.
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod evidence;
pub mod hlc;
pub mod log;
pub mod merge;
pub mod op;
#[cfg(not(target_arch = "wasm32"))]
pub mod perm;
#[cfg(not(target_arch = "wasm32"))]
pub mod sync;
#[cfg(not(target_arch = "wasm32"))]
pub mod token;

pub mod stats;

use anyhow::{bail, Context, Result};
use op::{Actor, Draft, Op};
use serde_json::{json, Value};
use std::{path::Path, sync::Mutex};

/// Hard upper bound on operations one local mutation may produce.
pub const MAX_LOCAL_GROUP: usize = 10_000;
pub const EXTERNAL_ACTOR: &str = "act_external";

// ---------------------------------------------------------------- actors

/// Actor settings from the global CLI flags (`--actor-*`, `--directed-by`).
#[derive(Debug, Clone, Default)]
pub struct ActorSpec {
    pub kind: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub directed_by: Option<String>,
}

static FLAGS: Mutex<Option<ActorSpec>> = Mutex::new(None);
/// Actor of the request being served by `serve --collab`: set by the request
/// guard from the authenticated token, so a client can never pick its own.
static REQUEST_ACTOR: Mutex<Option<(Actor, Option<Actor>)>> = Mutex::new(None);

pub fn set_request_actor(actor: Option<(Actor, Option<Actor>)>) {
    if let Ok(mut guard) = REQUEST_ACTOR.lock() {
        *guard = actor;
    }
}

pub fn set_actor_flags(spec: ActorSpec) {
    if let Ok(mut guard) = FLAGS.lock() {
        *guard = Some(spec);
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn os_user() -> String {
    let user = env("USERNAME")
        .or_else(|| env("USER"))
        .or_else(|| env("LOGNAME"))
        .unwrap_or_else(|| "unknown".into());
    user.chars().filter(|c| !c.is_control()).take(64).collect()
}

/// The actor performing this process's mutations: CLI flags, then
/// `PENTOOL_ACTOR_*` environment, then the local OS user as a human.
pub fn current_actor() -> Result<(Actor, Option<Actor>)> {
    if let Some(actor) = REQUEST_ACTOR.lock().ok().and_then(|g| g.clone()) {
        return Ok(actor);
    }
    let flags = FLAGS
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default();
    let kind = flags
        .kind
        .or_else(|| env("PENTOOL_ACTOR_KIND"))
        .unwrap_or_else(|| "human".into());
    let handle = flags.id.or_else(|| env("PENTOOL_ACTOR_ID"));
    let name = flags.name.or_else(|| env("PENTOOL_ACTOR_NAME"));
    let directed_by = flags
        .directed_by
        .or_else(|| env("PENTOOL_ACTOR_DIRECTED_BY"));
    match kind.as_str() {
        "human" => {
            let handle = handle.unwrap_or_else(os_user);
            let actor = Actor {
                id: Actor::derive_id("human", &handle),
                kind: kind.clone(),
                display_name: name.or(Some(handle)),
                tool: env("PENTOOL_ACTOR_TOOL"),
                model: None,
                directed_by: None,
            };
            actor.validate()?;
            Ok((actor, None))
        }
        "agent" => {
            let handle = handle.context(
                "[invalid-input] agents must identify themselves: pass --actor-id (or PENTOOL_ACTOR_ID)",
            )?;
            let director = directed_by.context(
                "[invalid-input] agents must name the human they act for: pass --directed-by (or PENTOOL_ACTOR_DIRECTED_BY)",
            )?;
            let human = Actor {
                id: Actor::derive_id("human", &director),
                kind: "human".into(),
                display_name: Some(director),
                tool: None,
                model: None,
                directed_by: None,
            };
            let actor = Actor {
                id: Actor::derive_id("agent", &handle),
                kind: kind.clone(),
                display_name: name.or(Some(handle)),
                tool: env("PENTOOL_ACTOR_TOOL"),
                model: env("PENTOOL_ACTOR_MODEL"),
                directed_by: Some(human.id.clone()),
            };
            human.validate()?;
            actor.validate()?;
            Ok((actor, Some(human)))
        }
        other => bail!("[invalid-input] actor kind must be human or agent, got {other:?}"),
    }
}

pub(crate) fn external_actor() -> Actor {
    Actor {
        id: EXTERNAL_ACTOR.into(),
        kind: "human".into(),
        display_name: Some("external edit".into()),
        tool: None,
        model: None,
        directed_by: None,
    }
}

// ------------------------------------------------------------------ hook

/// A staged log append waiting for the document commit.
pub struct Pending(log::Prepared, Vec<u8>);

impl Pending {
    /// The exact bytes to write and record in history: the new document with
    /// its embedded `collab` block, so one commit carries both.
    pub fn bytes(&self) -> &[u8] {
        &self.1
    }

    /// The document is committed: make the log durable.
    pub fn commit(self) -> Result<()> {
        self.0.finalize()
    }
    /// The document commit failed: drop the staged lines.
    pub fn abort(self) {
        self.0.abort()
    }
}

/// Revision of a document ignoring its `collab` block, which is derived from
/// the log and therefore cannot take part in the log's own revisions.
pub fn content_revision(bytes: &[u8]) -> String {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(mut value) => {
            stats::strip(&mut value);
            crate::transaction::revision(op::canonical_json(&value).as_bytes())
        }
        Err(_) => crate::transaction::revision(bytes),
    }
}

/// The document bytes without attribution, unless `include` is set. Used by
/// every export and package path (spec section 7.5).
pub fn export_bytes(bytes: Vec<u8>, include: bool) -> Result<Vec<u8>> {
    if include {
        return Ok(bytes);
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(bytes);
    };
    if value.get("collab").is_none() {
        return Ok(bytes);
    }
    stats::strip(&mut value);
    Ok(serde_json::to_vec_pretty(&value)?)
}

fn parse_document(bytes: &[u8], what: &str) -> Result<Value> {
    serde_json::from_slice(bytes).with_context(|| format!("{what} is not valid JSON"))
}

fn require_v4(value: &Value, what: &str) -> Result<()> {
    if value.get("version").and_then(Value::as_u64) != Some(4) {
        bail!(
            "[migration-blocked] collaboration records version 4 documents; {what} is not version 4 (migrate before or after the session)"
        )
    }
    Ok(())
}

pub(crate) fn build_ops(
    drafts: Vec<Draft>,
    state: &mut merge::Replayer,
    clock: &mut hlc::Clock,
    actor: &str,
    base: &str,
    via: &str,
    out: &mut Vec<Op>,
) -> Result<()> {
    if drafts.len() > MAX_LOCAL_GROUP {
        bail!(
            "[limit-exceeded] this change needs {} operations; the limit is {MAX_LOCAL_GROUP} per change",
            drafts.len()
        )
    }
    for draft in drafts {
        let deps = state
            .last_applied()
            .map(|id| vec![id.to_string()])
            .unwrap_or_default();
        let op = Op::build(draft, actor, clock.tick(hlc::now_ms()), base, deps, via)?;
        if let Err(reject) = state.apply_op(&op) {
            bail!(
                "[{}] {} (operation {} {}): {}",
                reject.code,
                via,
                op.kind,
                op.targets.join(","),
                reject.message
            )
        }
        out.push(op);
    }
    Ok(())
}

/// Remove the temp and moved-aside copies an interrupted `atomic_write` left
/// beside the document. Only call this under the collaboration log lock, with
/// the document present: every writer of a collaborative document takes that
/// lock first, so any such sibling is an orphan of a dead process.
pub(crate) fn sweep_document_temps(document: &Path) {
    if !document.exists() {
        return;
    }
    let (Some(parent), Some(name)) = (document.parent(), document.file_name()) else {
        return;
    };
    let name = name.to_string_lossy();
    let (temp, old) = (
        format!(".{name}.pentool-tmp-"),
        format!(".{name}.pentool-old-"),
    );
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    crate::stale::sweep(parent, std::time::Duration::ZERO, |n| {
        n.starts_with(&temp) || n.starts_with(&old)
    });
}

/// Record a pending document change in the collaboration log, if enabled.
///
/// Called by the transaction path after validation and before the document is
/// written. Returns `None` (and does nothing) when the document has no log.
pub fn begin(
    document: &Path,
    operation: &str,
    before: &[u8],
    after: &[u8],
) -> Result<Option<Pending>> {
    let dir = log::dir(document)?;
    if !log::enabled(&dir) {
        return Ok(None);
    }
    let lock = log::lock(&dir)?;
    let before_revision = content_revision(before);
    log::recover(&dir, &before_revision)?;
    sweep_document_temps(document);
    let lines = log::read_lines(&dir)?;
    let head = log::check_tail(&dir, &lines)?;
    if lines.len() >= log::MAX_LOG_OPS {
        bail!(
            "[limit-exceeded] collaboration log holds {} operations (limit {}); compaction is not available yet",
            lines.len(),
            log::MAX_LOG_OPS
        )
    }
    let (_, mut genesis) = log::read_genesis(&dir)?;
    stats::strip(&mut genesis);
    let replayed = merge::replay(&genesis, &log::ops_of(&lines))?;
    let mut state = replayed.state;
    let mut before_value = parse_document(before, "existing document")?;
    let after_full = parse_document(after, "new document")?;
    require_v4(&after_full, "the new document")?;
    // The embedded `collab` block is derived output, not recorded content.
    stats::strip(&mut before_value);
    let mut after_value = after_full.clone();
    stats::strip(&mut after_value);
    let mut clock = hlc::Clock::new(&log::peer_id(&dir)?, state.max_stamp());
    let base = format!("sha256:{before_revision}");
    let (actor, director) = current_actor()?;
    let mut registry = vec![actor.clone()];
    registry.extend(director);
    let mut ops: Vec<Op> = Vec::new();

    // Edits made outside the transaction path since the last log entry are
    // attributed to the external actor so the chain stays continuous.
    if state.materialize() != before_value {
        require_v4(&before_value, "the existing document")?;
        let drafts = state.derive(&before_value)?;
        registry.push(external_actor());
        build_ops(
            drafts,
            &mut state,
            &mut clock,
            EXTERNAL_ACTOR,
            &base,
            "catch-up",
            &mut ops,
        )?;
        if state.materialize() != before_value {
            bail!("[collab-divergence] the existing document could not be reconciled with the operation log; run `pentool collab verify`")
        }
    }
    let drafts = state.derive(&after_value)?;
    build_ops(
        drafts, &mut state, &mut clock, &actor.id, &base, operation, &mut ops,
    )?;
    #[cfg(not(target_arch = "wasm32"))]
    sync::check_claims(&dir, &actor, ops.iter().filter(|op| op.actor == actor.id))?;
    if state.materialize() != after_value {
        bail!(
            "[collab-divergence] {operation}: the change could not be recorded as operations; the document was not modified"
        )
    }
    if ops.is_empty() {
        return Ok(None);
    }
    let after_revision = content_revision(after);
    let mut prev = head.hash;
    let mut seq = head.seq;
    let mut staged = Vec::with_capacity(ops.len());
    for op in ops {
        seq += 1;
        let line = log::Line::new(seq, op, &after_revision, &prev);
        prev = line.hash.clone();
        staged.push(line);
    }
    let (tip_seq, tip_hash) = staged
        .last()
        .map(|line| (line.seq, line.hash.clone()))
        .expect("operations produce lines");
    let mut known = log::read_actors(&dir)?;
    for actor in &registry {
        if !known.iter().any(|a| a.id == actor.id) {
            known.push(actor.clone());
        }
    }
    let summary = stats::summary(&state.stats_raw(), &known)?;
    let mut final_value = after_full;
    let block = stats::embed(final_value.get("collab"), tip_seq, &tip_hash, summary);
    final_value["collab"] = block;
    stats::validate_block(&final_value)?;
    let mut final_bytes = serde_json::to_vec_pretty(&final_value)?;
    if after.ends_with(b"\n") {
        final_bytes.push(b'\n');
    }
    let prepared = log::prepare(&dir, lock, staged, registry, &after_revision)?;
    Ok(Some(Pending(prepared, final_bytes)))
}

// ------------------------------------------------------------------- CLI

pub(crate) fn doc_dir(document: &Path) -> Result<std::path::PathBuf> {
    let dir = log::dir(document)?;
    if !log::enabled(&dir) {
        bail!(
            "[invalid-input] collaboration is not enabled for {}; run `pentool collab init {}` first",
            document.display(),
            document.display()
        )
    }
    Ok(dir)
}

/// `pentool collab init DOC`: start a chain at the document's current bytes.
pub fn init(document: &Path) -> Result<Value> {
    crate::editing::recover_interrupted_write(document)?;
    let bytes = std::fs::read(document)
        .with_context(|| format!("could not read {}", document.display()))?;
    let value = parse_document(&bytes, "document")?;
    crate::transaction::validate_value(&value)?;
    require_v4(&value, "this document")?;
    let dir = log::dir(document)?;
    let head = log::init(&dir, &bytes)?;
    Ok(json!({
        "ok": true,
        "operation": "collab-init",
        "file": document,
        "enabled": true,
        "revision": crate::transaction::revision(&bytes),
        "head": head,
    }))
}

fn op_summary(op: &Op) -> Value {
    let mut summary = json!({"level": op.payload.get("level")});
    match op.kind.as_str() {
        "set-property" => {
            summary["path"] = op.payload.get("path").cloned().unwrap_or(Value::Null);
            summary["remove"] =
                Value::Bool(op.payload.get("remove").and_then(Value::as_bool) == Some(true));
        }
        "create" | "move" => {
            summary["parent"] = op.payload.get("parent").cloned().unwrap_or(Value::Null);
        }
        "scene" => {
            summary["operations"] = json!(op
                .payload
                .get("operations")
                .and_then(Value::as_array)
                .map_or(0, Vec::len));
        }
        _ => {}
    }
    summary
}

/// `pentool collab log DOC`: compact, paginated listing.
pub fn log_page(
    document: &Path,
    offset: usize,
    limit: usize,
    actor: Option<&str>,
) -> Result<Value> {
    if !(1..=500).contains(&limit) {
        bail!("[invalid-input] log limit must be in 1..=500")
    }
    let dir = doc_dir(document)?;
    let lines = log::read_lines(&dir)?;
    let head = log::check_tail(&dir, &lines)?;
    let selected: Vec<&log::Line> = lines
        .iter()
        .filter(|line| actor.is_none_or(|a| line.op.actor == a))
        .collect();
    let matches = selected.len();
    let page: Vec<Value> = selected
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|line| {
            json!({
                "seq": line.seq,
                "op": line.op.id,
                "actor": line.op.actor,
                "kind": line.op.kind,
                "page": line.op.page,
                "targets": line.op.targets,
                "hlc": {"wall_ms": line.op.hlc.wall_ms, "counter": line.op.hlc.counter},
                "via": line.op.via(),
                "summary": op_summary(&line.op),
                "revision": line.applied_at_revision,
            })
        })
        .collect();
    let actors = log::read_actors(&dir)?;
    Ok(json!({
        "ok": true,
        "matches": matches,
        "returned": page.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset.saturating_add(page.len()) < matches,
        "head": head,
        "operations": page,
        "actors": actors,
    }))
}

fn replay_summary(
    document: &Path,
    dir: &Path,
    lines: &[log::Line],
) -> Result<(merge::Replay, Value)> {
    let (_, mut genesis) = log::read_genesis(dir)?;
    stats::strip(&mut genesis);
    let replay = merge::replay(&genesis, &log::ops_of(lines))?;
    let current = std::fs::read(document)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map(|mut value| {
            stats::strip(&mut value);
            value
        });
    let rebuilt = replay.document();
    let conflicts = replay.state.conflicts();
    let shown: Vec<&merge::Conflict> = conflicts.iter().take(50).collect();
    let summary = json!({
        "ops": lines.len(),
        "applied": replay.applied,
        "rejected": replay.rejected.iter().map(|(id, code)| json!({"op": id, "code": code})).collect::<Vec<_>>(),
        "pending": replay.pending,
        "conflicts_total": conflicts.len(),
        "conflicts": shown,
        "replay_digest": merge::document_digest(&rebuilt),
        "matches_document": current.as_ref().is_some_and(|c| *c == rebuilt),
    });
    Ok((replay, summary))
}

/// `pentool collab replay DOC [--out FILE]`: rebuild the document from the log.
pub fn replay_report(document: &Path, out: Option<&Path>) -> Result<Value> {
    let dir = doc_dir(document)?;
    let lines = log::read_lines(&dir)?;
    log::verify_chain(&dir, &lines)?;
    let (replay, mut summary) = replay_summary(document, &dir, &lines)?;
    summary["ok"] = Value::Bool(true);
    let bytes = replay.bytes();
    summary["revision"] = Value::String(crate::transaction::revision(&bytes));
    if let Some(out) = out {
        if out == document
            || (out.exists()
                && document.exists()
                && std::fs::canonicalize(out).ok() == std::fs::canonicalize(document).ok())
        {
            bail!("[invalid-input] --out must not be the document itself; replay never modifies the document")
        }
        crate::editing::atomic_write(out, &bytes)?;
        summary["written"] = json!(out);
    }
    Ok(summary)
}

/// `pentool collab verify DOC`: chain, head, op validity and replay.
pub fn verify_report(document: &Path) -> Result<Value> {
    let dir = doc_dir(document)?;
    let lines = log::read_lines(&dir)?;
    let head = log::verify_chain(&dir, &lines)?;
    let (_, summary) = replay_summary(document, &dir, &lines)?;
    let document_revision = std::fs::read(document)
        .ok()
        .map(|bytes| crate::transaction::revision(&bytes));
    let embedded = embedded_status(document, &dir, &lines, &head)?;
    let last_revision = lines.last().map(|l| l.applied_at_revision.clone());
    let mut warnings = Vec::new();
    if summary["matches_document"] != Value::Bool(true) {
        warnings.push("document differs from the replayed log (edited outside the transaction path); the next recorded change adds a catch-up entry");
    }
    if embedded["status"] == "mismatch" {
        warnings.push("the embedded collab summary disagrees with the operation log; it is not trusted (run `pentool stats` for the derived figures)");
    }
    Ok(json!({
        "ok": true,
        "chain": "valid",
        "ops": lines.len(),
        "head": head,
        "replay": summary,
        "document_revision": document_revision,
        "last_recorded_revision": last_revision,
        "embedded_summary": embedded,
        "warnings": warnings,
    }))
}

// ----------------------------------------------------------------- stats

/// Replay the verified log and return the statistics inputs, the actor
/// registry and the summary that the document must embed.
fn derive_stats(dir: &Path, lines: &[log::Line]) -> Result<(stats::Raw, Vec<Actor>, Value)> {
    let (_, mut genesis) = log::read_genesis(dir)?;
    stats::strip(&mut genesis);
    let replay = merge::replay(&genesis, &log::ops_of(lines))?;
    let registry = log::read_actors(dir)?;
    let raw = replay.state.stats_raw();
    let summary = stats::summary(&raw, &registry)?;
    Ok((raw, registry, summary))
}

fn embedded_status(
    document: &Path,
    dir: &Path,
    lines: &[log::Line],
    head: &log::Head,
) -> Result<Value> {
    let current = std::fs::read(document)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let Some(current) = current else {
        return Ok(json!({"status": "missing"}));
    };
    if current.get("collab").is_none() {
        return Ok(json!({"status": "missing"}));
    }
    let (_, _, summary) = derive_stats(dir, lines)?;
    Ok(stats::check_embedded(
        &current, head.seq, &head.hash, &summary,
    ))
}

/// `pentool stats DOC [--by actor|kind|page]`: contribution statistics from
/// the verified operation log. The embedded summary is checked, never trusted.
pub fn stats_report(document: &Path, by: stats::By, offset: usize, limit: usize) -> Result<Value> {
    let dir = doc_dir(document)?;
    let lines = log::read_lines(&dir)?;
    let head = log::verify_chain(&dir, &lines)?;
    let (raw, registry, _) = derive_stats(&dir, &lines)?;
    let mut report = stats::report(&raw, &registry, by, offset, limit)?;
    let embedded = embedded_status(document, &dir, &lines, &head)?;
    let object = report.as_object_mut().expect("report is an object");
    object.insert("ok".into(), json!(true));
    object.insert("operation".into(), json!("stats"));
    object.insert("file".into(), json!(document));
    object.insert("head".into(), json!(head));
    object.insert("embedded_summary".into(), embedded);
    Ok(report)
}

/// `pentool collab strip DOC`: remove the embedded attribution in place.
pub fn strip_document(document: &Path) -> Result<Value> {
    let bytes = std::fs::read(document)
        .with_context(|| format!("could not read {}", document.display()))?;
    let stripped = export_bytes(bytes.clone(), false)?;
    if stripped == bytes {
        return Ok(json!({"ok": true, "operation": "collab-strip", "changed": false}));
    }
    let summary =
        crate::transaction::commit_bytes(document, "collab-strip", false, None, &stripped)?;
    Ok(serde_json::to_value(summary)?)
}

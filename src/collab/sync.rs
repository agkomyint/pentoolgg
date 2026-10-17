//! Local-network sync (spec section 8): integrating foreign operations, the
//! `/api/collab/*` routes served by `pentool serve --collab`, claims, explicit
//! peers and the `collab sync` client.
//!
//! Peers exchange *sets of operations*. Replay is a pure function of that set
//! (see `merge::replay`), so reconciling two peers means each side gets the
//! operations it lacks, replays, and commits the resulting document through
//! the same write-ahead log/document/history unit that local edits use.
use super::{
    hlc::{now_ms, MAX_CLOCK_SKEW_MS},
    log::{self, Head, Line},
    merge,
    op::{check_id, parse_op, Actor, Op},
    perm::problem,
    token::{self, Principal, Role},
};
use anyhow::{anyhow, bail, Context, Result};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Query},
    http::{header, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    convert::Infallible,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub const PROTOCOL: u64 = 1;

// ---------------------------------------------------------------- limits

/// Section 10 limits. A server can lower but never raise them above the hard
/// ceilings.
#[derive(Debug, Clone)]
pub struct Limits {
    pub batch_ops: usize,
    pub batch_bytes: usize,
    pub ops_per_second: usize,
    pub max_pending: usize,
    pub max_peers: usize,
    pub max_claims: usize,
    pub claim_ttl_ms: u64,
    pub sse_total: usize,
    pub sse_per_principal: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            batch_ops: 500,
            batch_bytes: 4 * 1024 * 1024,
            ops_per_second: 50,
            max_pending: 1_000,
            max_peers: 16,
            max_claims: 256,
            claim_ttl_ms: 10 * 60 * 1000,
            sse_total: 64,
            sse_per_principal: 4,
        }
    }
}

impl Limits {
    pub const CEILING_BATCH_OPS: usize = 10_000;
    pub const CEILING_BATCH_BYTES: usize = 16 * 1024 * 1024;
    pub const CEILING_OPS_PER_SECOND: usize = 500;
    pub const CEILING_PENDING: usize = 10_000;
    pub const CEILING_PEERS: usize = 64;
    pub const CEILING_CLAIMS: usize = 2_048;
    pub const CEILING_CLAIM_TTL_MS: u64 = 3_600_000;

    pub fn clamped(mut self) -> Self {
        self.batch_ops = self.batch_ops.clamp(1, Self::CEILING_BATCH_OPS);
        self.batch_bytes = self.batch_bytes.clamp(1024, Self::CEILING_BATCH_BYTES);
        self.ops_per_second = self.ops_per_second.clamp(1, Self::CEILING_OPS_PER_SECOND);
        self.max_pending = self.max_pending.min(Self::CEILING_PENDING);
        self.max_peers = self.max_peers.clamp(1, Self::CEILING_PEERS);
        self.max_claims = self.max_claims.min(Self::CEILING_CLAIMS);
        self.claim_ttl_ms = self.claim_ttl_ms.clamp(1_000, Self::CEILING_CLAIM_TTL_MS);
        self
    }

    fn view(&self) -> Value {
        json!({
            "op_bytes": super::op::MAX_OP_BYTES,
            "batch_ops": self.batch_ops,
            "batch_bytes": self.batch_bytes,
            "ops_per_second": self.ops_per_second,
            "pending": self.max_pending,
            "peers": self.max_peers,
            "claims": self.max_claims,
            "claim_ttl_ms": self.claim_ttl_ms,
            "clock_skew_ms": MAX_CLOCK_SKEW_MS,
        })
    }
}

// ----------------------------------------------------------- shared state

/// Everything the collaboration routes and the request guard share.
pub struct Ctx {
    pub document: PathBuf,
    pub dir: PathBuf,
    pub limits: Limits,
    /// Tokenless requests from the host's own loopback browser act as owner.
    pub local_owner: bool,
    pub actor_gate: tokio::sync::Mutex<()>,
    rate: Mutex<HashMap<String, (Instant, usize)>>,
    streams: AtomicUsize,
    stream_owners: Mutex<HashMap<String, usize>>,
}

impl Ctx {
    pub fn new(document: &Path, limits: Limits, local_owner: bool) -> Result<Ctx> {
        let dir = super::doc_dir(document)?;
        Ok(Ctx {
            document: document.to_path_buf(),
            dir,
            limits: limits.clamped(),
            local_owner,
            actor_gate: tokio::sync::Mutex::new(()),
            rate: Mutex::new(HashMap::new()),
            streams: AtomicUsize::new(0),
            stream_owners: Mutex::new(HashMap::new()),
        })
    }

    /// Fixed one-second window per token.
    fn take_rate(&self, key: &str, ops: usize) -> bool {
        let Ok(mut map) = self.rate.lock() else {
            return false;
        };
        let now = Instant::now();
        if map.len() > 4096 {
            map.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(1));
        }
        let slot = map.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(slot.0) >= Duration::from_secs(1) {
            *slot = (now, 0);
        }
        if slot.1 + ops > self.limits.ops_per_second {
            return false;
        }
        slot.1 += ops;
        true
    }
}

struct StreamSlot {
    ctx: Arc<Ctx>,
    owner: String,
}

impl StreamSlot {
    fn acquire(ctx: &Arc<Ctx>, owner: &str) -> Option<StreamSlot> {
        let mut owners = ctx.stream_owners.lock().ok()?;
        let mine = owners.entry(owner.to_string()).or_insert(0);
        if *mine >= ctx.limits.sse_per_principal
            || ctx.streams.load(Ordering::SeqCst) >= ctx.limits.sse_total
        {
            return None;
        }
        *mine += 1;
        ctx.streams.fetch_add(1, Ordering::SeqCst);
        Some(StreamSlot {
            ctx: ctx.clone(),
            owner: owner.to_string(),
        })
    }
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.ctx.streams.fetch_sub(1, Ordering::SeqCst);
        if let Ok(mut owners) = self.ctx.stream_owners.lock() {
            if let Some(count) = owners.get_mut(&self.owner) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    owners.remove(&self.owner);
                }
            }
        }
    }
}

// ------------------------------------------------------------ fault hooks

/// Crash points used by the crash-during-sync tests: the integration returns
/// an error at that point and leaves exactly the files a killed process would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The write-ahead record is durable; the document is not yet written.
    AfterPrepare,
    /// The document is written; the log append is not finalized.
    AfterDocument,
}

static FAULTS: Mutex<Vec<(PathBuf, Fault)>> = Mutex::new(Vec::new());

/// Arm a one-shot fault for `document` (test support).
#[doc(hidden)]
pub fn inject_fault(document: &Path, fault: Fault) {
    if let Ok(mut faults) = FAULTS.lock() {
        faults.push((document.to_path_buf(), fault));
    }
}

fn take_fault(document: &Path, wanted: Fault) -> bool {
    let Ok(mut faults) = FAULTS.lock() else {
        return false;
    };
    match faults
        .iter()
        .position(|(path, fault)| path == document && *fault == wanted)
    {
        Some(index) => {
            faults.remove(index);
            true
        }
        None => false,
    }
}

// ----------------------------------------------------------------- claims

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub actor: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub page: String,
    pub targets: Vec<String>,
    pub expires_ms: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct ClaimStore {
    #[serde(default)]
    claims: Vec<Claim>,
}

const MAX_CLAIM_TARGETS: usize = 64;

fn load_claims(dir: &Path) -> Vec<Claim> {
    let now = now_ms();
    fs::read(dir.join("claims.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ClaimStore>(&bytes).ok())
        .map(|store| {
            store
                .claims
                .into_iter()
                .filter(|claim| claim.expires_ms > now)
                .collect()
        })
        .unwrap_or_default()
}

fn save_claims(dir: &Path, claims: Vec<Claim>) -> Result<()> {
    log::write_atomic(
        &dir.join("claims.json"),
        &serde_json::to_vec_pretty(&ClaimStore { claims })?,
    )
}

fn covers(claim: &Claim, page: &str, id: &str) -> bool {
    (claim.page.is_empty() || page.is_empty() || claim.page == page)
        && claim.targets.iter().any(|target| target == id)
}

/// Who blocks `actor` from touching `ids`? A human may always override an
/// agent's claim; an agent may not override a human's (spec 6.4).
fn blocking_claim<'a>(
    claims: &'a [Claim],
    actor: &Actor,
    page: &str,
    ids: &[&str],
) -> Option<&'a Claim> {
    claims.iter().find(|claim| {
        claim.actor != actor.id
            && !(actor.kind == "human" && claim.kind == "agent")
            && ids.iter().any(|id| covers(claim, page, id))
    })
}

fn op_ids(op: &Op) -> Vec<&str> {
    let mut ids: Vec<&str> = op.targets.iter().map(String::as_str).collect();
    if let Some(parent) = op
        .payload
        .get("parent")
        .and_then(|p| p.get("id"))
        .and_then(Value::as_str)
    {
        ids.push(parent);
    }
    ids
}

fn locked_message(claim: &Claim, op: &Op) -> String {
    format!(
        "operation {} touches objects claimed by {} until {}; wait for the claim to expire or ask the owner to release it",
        op.id,
        claim.name.as_deref().unwrap_or(&claim.actor),
        claim.expires_ms
    )
}

/// Reject local operations that touch another actor's active claim. Called by
/// the transaction hook so every mutation path honours claims.
pub fn check_claims<'a>(
    dir: &Path,
    actor: &Actor,
    ops: impl Iterator<Item = &'a Op>,
) -> Result<()> {
    let claims = load_claims(dir);
    if claims.is_empty() {
        return Ok(());
    }
    for op in ops {
        if let Some(claim) = blocking_claim(&claims, actor, &op.page, &op_ids(op)) {
            bail!("[locked] {}", locked_message(claim, op))
        }
    }
    Ok(())
}

pub fn claim(
    dir: &Path,
    limits: &Limits,
    actor: &Actor,
    page: &str,
    targets: Vec<String>,
    ttl_ms: Option<u64>,
) -> Result<Value> {
    if targets.is_empty() || targets.len() > MAX_CLAIM_TARGETS {
        bail!("[invalid-input] a claim names 1 to {MAX_CLAIM_TARGETS} targets")
    }
    for target in &targets {
        check_id("claim target", target)?;
    }
    if !page.is_empty() {
        check_id("page", page)?;
    }
    let ttl = ttl_ms.unwrap_or(limits.claim_ttl_ms);
    if ttl == 0 || ttl > limits.claim_ttl_ms {
        bail!(
            "[limit-exceeded] claim ttl_ms must be 1 to {} (the session limit)",
            limits.claim_ttl_ms
        )
    }
    let _lock = log::lock(dir)?;
    let mut claims = load_claims(dir);
    for target in &targets {
        if let Some(other) = blocking_claim(&claims, actor, page, &[target.as_str()]) {
            bail!(
                "[locked] {target} is already claimed by {} until {}",
                other.name.as_deref().unwrap_or(&other.actor),
                other.expires_ms
            )
        }
    }
    // Renewal replaces the actor's earlier claim on the same targets.
    claims.retain(|c| {
        !(c.actor == actor.id && c.page == page && c.targets.iter().any(|t| targets.contains(t)))
    });
    if claims.len() >= limits.max_claims {
        bail!(
            "[limit-exceeded] {} active claims is the limit; release some or wait for them to expire",
            limits.max_claims
        )
    }
    let entry = Claim {
        actor: actor.id.clone(),
        kind: actor.kind.clone(),
        name: actor.display_name.clone(),
        page: page.into(),
        targets,
        expires_ms: now_ms() + ttl,
    };
    claims.push(entry.clone());
    save_claims(dir, claims)?;
    Ok(json!({"claimed": entry}))
}

/// Release the actor's own claims on `targets` (all of them when empty). The
/// owner may break anyone's claim with `force`.
pub fn release(dir: &Path, actor: &Actor, targets: &[String], force: bool) -> Result<Value> {
    let _lock = log::lock(dir)?;
    let claims = load_claims(dir);
    let before = claims.len();
    let kept: Vec<Claim> = claims
        .into_iter()
        .filter(|c| {
            let mine = c.actor == actor.id || force;
            let named = targets.is_empty() || c.targets.iter().any(|t| targets.contains(t));
            !(mine && named)
        })
        .collect();
    let released = before - kept.len();
    save_claims(dir, kept)?;
    Ok(json!({"released": released}))
}

pub fn claims_view(dir: &Path) -> Vec<Claim> {
    load_claims(dir)
}

// ----------------------------------------------------------------- peers

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    pub url: String,
    /// Last remote log sequence integrated here.
    #[serde(default)]
    pub pull_seq: u64,
    /// Last local log sequence offered to the remote.
    #[serde(default)]
    pub pushed_seq: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct PeerStore {
    #[serde(default)]
    peers: Vec<Peer>,
}

fn load_peers(dir: &Path) -> Result<PeerStore> {
    match fs::read(dir.join("peers.json")) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).context("[corrupt-log] peers.json is malformed")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PeerStore::default()),
        Err(error) => Err(error.into()),
    }
}

fn save_peers(dir: &Path, store: &PeerStore) -> Result<()> {
    log::write_atomic(&dir.join("peers.json"), &serde_json::to_vec_pretty(store)?)
}

/// Peer URLs are `http(s)://host[:port][/prefix]`: no credentials (tokens are
/// never part of a URL), no query, fragment, dot segments, escapes or control
/// characters.
pub fn normalize_peer_url(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 256 || text.chars().any(|c| c.is_control() || c == '\\') {
        bail!("[invalid-input] peer URL must be 1 to 256 printable characters")
    }
    let url = url::Url::parse(text)
        .map_err(|error| anyhow!("[invalid-input] peer URL {text:?} is not valid: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!(
            "[invalid-input] peer URL must use http or https, got {}",
            url.scheme()
        )
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("[invalid-input] peer URLs must not contain credentials; pass the token with --token")
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("[invalid-input] peer URLs must not have a query or fragment")
    }
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| anyhow!("[invalid-input] peer URL has no host"))?;
    let path = url.path();
    if path.contains('%')
        || text.contains('%')
        || path
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
        || text.contains("/../")
        || text.ends_with("/..")
    {
        bail!("[invalid-input] peer URL path must not contain dot segments or escapes")
    }
    let mut out = format!("{}://{host}", url.scheme());
    if let Some(port) = url.port() {
        out.push_str(&format!(":{port}"));
    }
    let path = path.trim_end_matches('/');
    if !path.is_empty() {
        out.push_str(path);
    }
    Ok(out)
}

fn is_loopback_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .is_some_and(|h| matches!(h.as_str(), "localhost" | "127.0.0.1" | "[::1]" | "::1"))
}

pub fn peer_add(document: &Path, url: &str, name: Option<&str>, max_peers: usize) -> Result<Value> {
    let dir = super::doc_dir(document)?;
    let url = normalize_peer_url(url)?;
    let name = name.map(str::to_string).unwrap_or_else(|| url.clone());
    if name.len() > 64 || name.chars().any(char::is_control) || name.is_empty() {
        bail!("[invalid-input] peer --name must be 1 to 64 printable characters")
    }
    let _lock = log::lock(&dir)?;
    let mut store = load_peers(&dir)?;
    if store.peers.iter().any(|p| p.url == url || p.name == name) {
        bail!("[invalid-input] peer {name:?} ({url}) is already registered")
    }
    let limit = max_peers.min(Limits::CEILING_PEERS);
    if store.peers.len() >= limit {
        bail!("[limit-exceeded] a session is limited to {limit} peers; remove one first")
    }
    store.peers.push(Peer {
        name,
        url,
        pull_seq: 0,
        pushed_seq: 0,
    });
    save_peers(&dir, &store)?;
    Ok(json!({"peers": store.peers}))
}

pub fn peer_list(document: &Path) -> Result<Value> {
    let dir = super::doc_dir(document)?;
    let store = load_peers(&dir)?;
    Ok(
        json!({"matches": store.peers.len(), "returned": store.peers.len(), "offset": 0,
        "limit": Limits::CEILING_PEERS, "has_more": false, "peers": store.peers}),
    )
}

pub fn peer_remove(document: &Path, which: &str) -> Result<Value> {
    let dir = super::doc_dir(document)?;
    let _lock = log::lock(&dir)?;
    let mut store = load_peers(&dir)?;
    let before = store.peers.len();
    store.peers.retain(|p| p.name != which && p.url != which);
    if store.peers.len() == before {
        bail!("[not-found] no peer {which:?}; run `pentool collab peer list`")
    }
    save_peers(&dir, &store)?;
    Ok(json!({"removed": which, "peers": store.peers}))
}

// ------------------------------------------------------------ integration

#[derive(Debug, Clone, Serialize)]
pub struct Rejection {
    pub id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Outcome {
    pub accepted: Vec<String>,
    pub duplicate: Vec<String>,
    pub rejected: Vec<Rejection>,
    /// Stored but waiting for missing dependencies.
    pub pending: Vec<String>,
    /// Stored but not applied by the merge rules (for example `locked`).
    pub merge_rejected: Vec<Rejection>,
    pub conflicts: Vec<merge::Conflict>,
    pub head: Option<Head>,
    pub document_revision: String,
}

impl Outcome {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("outcome serializes")
    }
}

/// Where a batch comes from.
#[derive(Debug, Clone, Copy)]
pub enum Origin<'a> {
    /// Operations fetched from a peer we chose to pull from.
    Pull,
    /// Operations a token holder submitted: they must all be by `actor`.
    Push { actor: &'a Actor },
}

fn code_of(error: &anyhow::Error) -> (String, String) {
    let text = error.to_string();
    let code = text
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map(|(code, _)| code.to_string())
        .unwrap_or_else(|| "invalid-op".into());
    (code, text)
}

fn reject(id: &str, code: &str, message: impl Into<String>) -> Rejection {
    Rejection {
        id: id.into(),
        code: code.into(),
        message: message.into(),
    }
}

/// Validate, merge and durably commit a batch of foreign operations.
///
/// The batch is one recoverable unit (spec 8.4): the log lines are staged,
/// the document and its history entry are committed, then the log is
/// finalized. A failure before the document commit leaves the document and
/// log untouched; a crash after it is completed by `log::recover`.
pub fn integrate(
    document: &Path,
    raw_ops: Vec<Value>,
    actors: &[Actor],
    origin: Origin<'_>,
    limits: &Limits,
) -> Result<Outcome> {
    if raw_ops.len() > limits.batch_ops {
        bail!(
            "[limit-exceeded] {} operations in one batch; the limit is {}",
            raw_ops.len(),
            limits.batch_ops
        )
    }
    crate::editing::recover_interrupted_write(document)?;
    let dir = super::doc_dir(document)?;
    let mut outcome = Outcome::default();
    let now = now_ms();

    // Stage 1: per-operation checks that need no document state.
    let mut candidates: Vec<Op> = Vec::new();
    let mut seen = HashSet::new();
    for raw in raw_ops {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let bytes = serde_json::to_vec(&raw)?;
        let op = match parse_op(&bytes) {
            Ok(op) => op,
            Err(error) => {
                let (code, message) = code_of(&error);
                outcome.rejected.push(reject(&id, &code, message));
                continue;
            }
        };
        if let Origin::Push { actor } = origin {
            if op.actor != actor.id {
                outcome.rejected.push(reject(
                    &op.id,
                    "actor-mismatch",
                    format!(
                        "operation {} is by {} but this token acts as {}; the server never accepts another actor",
                        op.id, op.actor, actor.id
                    ),
                ));
                continue;
            }
        }
        if op.hlc.wall_ms > now.saturating_add(MAX_CLOCK_SKEW_MS) {
            outcome.rejected.push(reject(
                &op.id,
                "clock-skew",
                format!(
                    "operation {} is stamped more than {MAX_CLOCK_SKEW_MS} ms ahead of this clock",
                    op.id
                ),
            ));
            continue;
        }
        if !seen.insert(op.id.clone()) {
            outcome.duplicate.push(op.id);
            continue;
        }
        candidates.push(op);
    }

    // Stage 2: state under the log lock.
    let lock = log::lock(&dir)?;
    let before =
        fs::read(document).with_context(|| format!("could not read {}", document.display()))?;
    let before_revision = crate::transaction::revision(&before);
    log::recover(&dir, &before_revision)?;
    super::sweep_document_temps(document);
    let lines = log::read_lines(&dir)?;
    let head = log::check_tail(&dir, &lines)?;
    let known: HashSet<&str> = lines.iter().map(|l| l.op.id.as_str()).collect();
    let registry = log::read_actors(&dir)?;
    let mut fresh: Vec<Op> = Vec::new();
    let claims = load_claims(&dir);
    let mut provided: HashMap<&str, &Actor> = HashMap::new();
    for actor in actors {
        if actor.validate().is_ok() {
            provided.insert(actor.id.as_str(), actor);
        }
    }
    for op in candidates {
        if known.contains(op.id.as_str()) {
            outcome.duplicate.push(op.id);
            continue;
        }
        if !registry.iter().any(|a| a.id == op.actor) && !provided.contains_key(op.actor.as_str()) {
            outcome.rejected.push(reject(
                &op.id,
                "unknown-actor",
                format!(
                    "operation {} names actor {} with no identity record",
                    op.id, op.actor
                ),
            ));
            continue;
        }
        if let Origin::Push { actor } = origin {
            if let Some(claim) = blocking_claim(&claims, actor, &op.page, &op_ids(&op)) {
                outcome
                    .rejected
                    .push(reject(&op.id, "locked", locked_message(claim, &op)));
                continue;
            }
        }
        fresh.push(op);
    }
    outcome.head = Some(head.clone());
    outcome.document_revision = before_revision.clone();
    if fresh.is_empty() {
        outcome.duplicate.sort();
        outcome.duplicate.dedup();
        return Ok(outcome);
    }
    if lines.len() + fresh.len() > log::MAX_LOG_OPS {
        bail!(
            "[limit-exceeded] collaboration log would exceed {} operations; compaction is not available yet",
            log::MAX_LOG_OPS
        )
    }

    let (_, genesis) = log::read_genesis(&dir)?;
    let existing = log::ops_of(&lines);
    let replayed = merge::replay(&genesis, &existing)?;
    let mut state = replayed.state;
    let before_value: Value =
        serde_json::from_slice(&before).context("[invalid-input] existing document is not JSON")?;
    let mut all: Vec<Op> = existing;
    let mut staged_ops: Vec<Op> = Vec::new();
    let mut staged_actors: Vec<Actor> = Vec::new();

    // Edits made outside the transaction path are attributed to the external
    // actor first, exactly as a local mutation would, so nothing is lost.
    if state.materialize() != before_value {
        let drafts = state.derive(&before_value)?;
        let mut clock = super::hlc::Clock::new(&log::peer_id(&dir)?, state.max_stamp());
        let base = format!("sha256:{before_revision}");
        let mut catch_up = Vec::new();
        super::build_ops(
            drafts,
            &mut state,
            &mut clock,
            super::EXTERNAL_ACTOR,
            &base,
            "catch-up",
            &mut catch_up,
        )?;
        if state.materialize() != before_value {
            bail!("[collab-divergence] the existing document could not be reconciled with the operation log; run `pentool collab verify`")
        }
        staged_actors.push(super::external_actor());
        all.extend(catch_up.iter().cloned());
        staged_ops.extend(catch_up);
    }
    fresh.sort_by_key(|op| op.order_key());
    for op in &fresh {
        let record = registry
            .iter()
            .find(|a| a.id == op.actor)
            .or_else(|| provided.get(op.actor.as_str()).copied());
        if let Some(record) = record {
            if !staged_actors.iter().any(|a| a.id == record.id) {
                staged_actors.push(record.clone());
            }
        }
    }
    all.extend(fresh.iter().cloned());
    staged_ops.extend(fresh.iter().cloned());

    let merged = merge::replay(&genesis, &all)?;
    if merged.pending.len() > limits.max_pending {
        bail!(
            "[limit-exceeded] {} operations would wait for missing dependencies; the limit is {}",
            merged.pending.len(),
            limits.max_pending
        )
    }
    let after = merged.bytes();
    let after_value: Value = serde_json::from_slice(&after)?;
    crate::transaction::validate_value(&after_value)
        .context("[invalid-merge] the merged document is not valid; the batch was not applied")?;
    let after_revision = crate::transaction::revision(&after);

    // Report: which of the fresh ops were applied, parked or merge-rejected.
    let fresh_ids: HashSet<&str> = fresh.iter().map(|o| o.id.as_str()).collect();
    let pending: HashSet<&str> = merged.pending.iter().map(String::as_str).collect();
    for op in &fresh {
        outcome.accepted.push(op.id.clone());
        if pending.contains(op.id.as_str()) {
            outcome.pending.push(op.id.clone());
        }
    }
    for (id, code) in &merged.rejected {
        if fresh_ids.contains(id.as_str()) {
            outcome.merge_rejected.push(reject(
                id,
                code,
                format!("operation {id} is stored but did not take effect: {code}"),
            ));
        }
    }
    outcome.conflicts = merged
        .state
        .conflicts()
        .into_iter()
        .filter(|c| {
            fresh_ids.contains(c.losing_op.as_str())
                || c.winning_op
                    .as_deref()
                    .is_some_and(|w| fresh_ids.contains(w))
        })
        .collect();
    outcome.accepted.sort();
    outcome.duplicate.sort();
    outcome.duplicate.dedup();
    outcome.pending.sort();
    outcome.document_revision = after_revision.clone();

    // Stage 3: the write-ahead unit.
    let mut prev = head.hash.clone();
    let mut seq = head.seq;
    let mut staged_lines: Vec<Line> = Vec::with_capacity(staged_ops.len());
    for op in staged_ops {
        seq += 1;
        let line = Line::new(seq, op, &after_revision, &prev);
        prev = line.hash.clone();
        staged_lines.push(line);
    }
    outcome.head = Some(Head {
        seq,
        hash: prev.clone(),
    });
    let prepared = log::prepare(&dir, lock, staged_lines, staged_actors, &after_revision)?;
    if take_fault(document, Fault::AfterPrepare) {
        bail!("[fault-injected] stopped after the write-ahead record was staged");
    }
    if before != after {
        let written = crate::history::record(document, "collab-sync", &before, &after)
            .and_then(|_| crate::editing::atomic_write(document, &after));
        if let Err(error) = written {
            // An injected crash must leave the files a killed process would.
            if !error.to_string().contains("[fault-injected]") {
                prepared.abort();
            }
            return Err(error);
        }
    }
    if take_fault(document, Fault::AfterDocument) {
        bail!(
            "[fault-injected] stopped after the document was written, before the log was finalized"
        );
    }
    prepared.finalize()?;
    Ok(outcome)
}

// ------------------------------------------------------------- handlers

fn status_for(code: &str) -> StatusCode {
    match code {
        "limit-exceeded" => StatusCode::PAYLOAD_TOO_LARGE,
        "rate-limited" => StatusCode::TOO_MANY_REQUESTS,
        "permission-denied" => StatusCode::FORBIDDEN,
        "not-found" => StatusCode::NOT_FOUND,
        "locked" | "collab-divergence" | "tampered-log" | "corrupt-log" => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    }
}

fn error_response(error: anyhow::Error) -> Response {
    let (code, message) = code_of(&error);
    let code = if message.starts_with('[') {
        code
    } else {
        "error".into()
    };
    let message = message
        .strip_prefix(&format!("[{code}] "))
        .unwrap_or(&message)
        .to_string();
    problem(status_for(&code), &code, message)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| anyhow!("collaboration task failed: {error}"))?
}

fn principal_key(principal: &Principal) -> String {
    principal.token.clone().unwrap_or_else(|| "local".into())
}

async fn session(
    Extension(ctx): Extension<Arc<Ctx>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let result = blocking(move || {
        let head = log::read_head(&ctx.dir)?;
        let (genesis_bytes, _) = log::read_genesis(&ctx.dir)?;
        let store = load_peers(&ctx.dir)?;
        let role = principal.role;
        let peers: Value = if role == Role::Owner {
            json!(store
                .peers
                .iter()
                .map(|p| json!({"name": p.name, "url": p.url}))
                .collect::<Vec<_>>())
        } else {
            json!(store.peers.len())
        };
        Ok(json!({
            "protocol": PROTOCOL,
            "peer": log::peer_id(&ctx.dir)?,
            "genesis": log::genesis_hash(&genesis_bytes),
            "head": head,
            "role": role.name(),
            "actor": principal.actor,
            "may": {
                "read": true,
                "comment": role >= Role::Commenter,
                "edit": role >= Role::Editor,
                "manage": role >= Role::Owner,
            },
            "peers": peers,
            "claims": claims_view(&ctx.dir),
            "active_streams": ctx.streams.load(Ordering::SeqCst),
            "limits": ctx.limits.view(),
        }))
    })
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}

async fn genesis(Extension(ctx): Extension<Arc<Ctx>>) -> Response {
    let result = blocking(move || {
        let (bytes, _) = log::read_genesis(&ctx.dir)?;
        Ok(json!({
            "hash": log::genesis_hash(&bytes),
            "text": String::from_utf8(bytes).context("genesis is not UTF-8")?,
        }))
    })
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
struct OpsQuery {
    since: Option<u64>,
    limit: Option<usize>,
}

/// Pages are bounded by count and by bytes.
const PAGE_BYTES: usize = 3 * 1024 * 1024;

async fn ops_get(Extension(ctx): Extension<Arc<Ctx>>, Query(query): Query<OpsQuery>) -> Response {
    let result = blocking(move || {
        let limit = query.limit.unwrap_or(200).clamp(1, ctx.limits.batch_ops);
        let since = query.since.unwrap_or(0);
        let lines = log::read_lines(&ctx.dir)?;
        let head = log::check_tail(&ctx.dir, &lines)?;
        let registry = log::read_actors(&ctx.dir)?;
        let total = lines.len();
        let mut ops = Vec::new();
        let mut used = 0usize;
        let mut next = since;
        let mut referenced = Vec::<&str>::new();
        for line in lines.iter().skip(since as usize) {
            let size = serde_json::to_vec(&line.op)?.len();
            if !ops.is_empty() && (ops.len() >= limit || used + size > PAGE_BYTES) {
                break;
            }
            used += size;
            next = line.seq;
            if !referenced.contains(&line.op.actor.as_str()) {
                referenced.push(line.op.actor.as_str());
            }
            ops.push(serde_json::to_value(&line.op)?);
        }
        let returned = ops.len();
        let actors: Vec<&Actor> = registry
            .iter()
            .filter(|a| referenced.contains(&a.id.as_str()))
            .collect();
        Ok(json!({
            "matches": total.saturating_sub(since as usize),
            "returned": returned,
            "offset": since,
            "limit": limit,
            "has_more": (next as usize) < total,
            "next": next,
            "head": head,
            "ops": ops,
            "actors": actors,
        }))
    })
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
struct OpsBody {
    #[serde(default)]
    ops: Vec<Value>,
}

/// Decode and bound-check an `ops` request body. The error carries the status,
/// problem code and message the endpoint answers with; this is also the fuzz
/// entry point for the sync request decoder.
#[doc(hidden)]
pub fn decode_ops_body(
    body: &[u8],
    limits: &Limits,
) -> std::result::Result<Vec<Value>, (StatusCode, &'static str, String)> {
    if body.len() > limits.batch_bytes {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit-exceeded",
            format!(
                "batch is {} bytes; the limit is {}",
                body.len(),
                limits.batch_bytes
            ),
        ));
    }
    let parsed: OpsBody = serde_json::from_slice(body).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            "invalid-input",
            format!("malformed batch: {error}"),
        )
    })?;
    if parsed.ops.len() > limits.batch_ops {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit-exceeded",
            format!(
                "{} operations in one batch; the limit is {}",
                parsed.ops.len(),
                limits.batch_ops
            ),
        ));
    }
    Ok(parsed.ops)
}

async fn ops_post(
    Extension(ctx): Extension<Arc<Ctx>>,
    Extension(principal): Extension<Principal>,
    body: Bytes,
) -> Response {
    let parsed = match decode_ops_body(&body, &ctx.limits) {
        Ok(parsed) => parsed,
        Err((status, code, message)) => return problem(status, code, message),
    };
    if !ctx.take_rate(&principal_key(&principal), parsed.len()) {
        let mut response = problem(
            StatusCode::TOO_MANY_REQUESTS,
            "rate-limited",
            format!(
                "more than {} operations per second for this token; retry shortly",
                ctx.limits.ops_per_second
            ),
        );
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, header::HeaderValue::from_static("1"));
        return response;
    }
    let limits = ctx.limits.clone();
    let document = ctx.document.clone();
    let actor = principal.actor.clone();
    let result = blocking(move || {
        integrate(
            &document,
            parsed,
            std::slice::from_ref(&actor),
            Origin::Push { actor: &actor },
            &limits,
        )
    })
    .await;
    match result {
        Ok(outcome) => Json(outcome.to_json()).into_response(),
        Err(error) => error_response(error),
    }
}

async fn events(
    Extension(ctx): Extension<Arc<Ctx>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let Some(slot) = StreamSlot::acquire(&ctx, &principal_key(&principal)) else {
        return problem(
            StatusCode::TOO_MANY_REQUESTS,
            "rate-limited",
            "too many open event streams; close one and retry",
        );
    };
    let started = Instant::now();
    let stream = futures_util::stream::unfold(
        (ctx, slot, None::<u64>),
        move |(ctx, slot, last)| async move {
            loop {
                if started.elapsed() > Duration::from_secs(15 * 60) {
                    return None;
                }
                let head = log::read_head(&ctx.dir).ok();
                if let Some(head) = head {
                    if last != Some(head.seq) {
                        let event = Event::default()
                            .event("head")
                            .data(json!({"seq": head.seq, "hash": head.hash}).to_string());
                        let seq = head.seq;
                        return Some((Ok::<Event, Infallible>(event), (ctx, slot, Some(seq))));
                    }
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

#[derive(Deserialize)]
struct ClaimBody {
    #[serde(default)]
    page: String,
    #[serde(default)]
    targets: Vec<String>,
    ttl_ms: Option<u64>,
}

async fn claim_post(
    Extension(ctx): Extension<Arc<Ctx>>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<ClaimBody>,
) -> Response {
    let result = blocking(move || {
        claim(
            &ctx.dir,
            &ctx.limits,
            &principal.actor,
            &body.page,
            body.targets,
            body.ttl_ms,
        )
    })
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
struct ReleaseBody {
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    force: bool,
}

async fn release_post(
    Extension(ctx): Extension<Arc<Ctx>>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<ReleaseBody>,
) -> Response {
    if body.force && principal.role < Role::Owner {
        return problem(
            StatusCode::FORBIDDEN,
            "permission-denied",
            "only the owner can break another actor's claim",
        );
    }
    let result =
        blocking(move || release(&ctx.dir, &principal.actor, &body.targets, body.force)).await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}

/// An identity as typed by the owner when issuing a token.
#[derive(Deserialize)]
struct ActorSpecBody {
    #[serde(default = "human")]
    kind: String,
    id: String,
    name: Option<String>,
    directed_by: Option<String>,
}

fn human() -> String {
    "human".into()
}

pub fn build_actor(
    kind: &str,
    handle: &str,
    name: Option<&str>,
    directed_by: Option<&str>,
) -> Result<Actor> {
    let actor = Actor {
        id: Actor::derive_id(kind, handle),
        kind: kind.into(),
        display_name: name
            .map(str::to_string)
            .or_else(|| Some(handle.to_string())),
        tool: None,
        model: None,
        directed_by: directed_by.map(|d| Actor::derive_id("human", d)),
    };
    actor.validate()?;
    Ok(actor)
}

#[derive(Deserialize)]
struct TokenCreateBody {
    role: String,
    name: String,
    actor: ActorSpecBody,
    expires: Option<String>,
}

async fn tokens_get(Extension(ctx): Extension<Arc<Ctx>>) -> Response {
    match blocking(move || token::list(&ctx.dir)).await {
        Ok(records) => {
            let views: Vec<Value> = records.iter().map(|r| r.view()).collect();
            Json(
                json!({"matches": views.len(), "returned": views.len(), "offset": 0,
                "limit": token::MAX_TOKENS, "has_more": false, "tokens": views}),
            )
            .into_response()
        }
        Err(error) => error_response(error),
    }
}

async fn tokens_post(
    Extension(ctx): Extension<Arc<Ctx>>,
    Json(body): Json<TokenCreateBody>,
) -> Response {
    let result = blocking(move || {
        let actor = build_actor(
            &body.actor.kind,
            &body.actor.id,
            body.actor.name.as_deref(),
            body.actor.directed_by.as_deref(),
        )?;
        let expiry = body
            .expires
            .as_deref()
            .map(token::parse_duration_ms)
            .transpose()?;
        let (record, secret) = token::create(
            &ctx.dir,
            Role::parse(&body.role)?,
            actor,
            &body.name,
            expiry,
        )?;
        Ok(json!({"token": secret, "record": record.view(),
            "note": "this secret is shown once; only its hash is stored"}))
    })
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
struct TokenIdBody {
    id: String,
    role: Option<String>,
}

async fn tokens_revoke(
    Extension(ctx): Extension<Arc<Ctx>>,
    Json(body): Json<TokenIdBody>,
) -> Response {
    match blocking(move || token::revoke(&ctx.dir, &body.id)).await {
        Ok(record) => Json(json!({"revoked": record.view()})).into_response(),
        Err(error) => error_response(error),
    }
}

async fn tokens_role(
    Extension(ctx): Extension<Arc<Ctx>>,
    Json(body): Json<TokenIdBody>,
) -> Response {
    let result = blocking(move || {
        let role = Role::parse(body.role.as_deref().unwrap_or(""))?;
        token::set_role(&ctx.dir, &body.id, role)
    })
    .await;
    match result {
        Ok(record) => Json(json!({"updated": record.view()})).into_response(),
        Err(error) => error_response(error),
    }
}

/// The `/api/collab/*` routes. Authentication and roles are enforced by
/// `perm::guard`, layered over these by the server.
pub fn routes<S: Clone + Send + Sync + 'static>(limits: &Limits) -> Router<S> {
    let body_limit = limits.batch_bytes.saturating_add(1024);
    Router::new()
        .route("/api/collab/session", get(session))
        .route("/api/collab/genesis", get(genesis))
        .route("/api/collab/events", get(events))
        .route(
            "/api/collab/ops",
            get(ops_get)
                .post(ops_post)
                .layer(DefaultBodyLimit::max(body_limit)),
        )
        .route("/api/collab/claim", post(claim_post))
        .route("/api/collab/release", post(release_post))
        .route("/api/collab/tokens", get(tokens_get).post(tokens_post))
        .route("/api/collab/tokens/revoke", post(tokens_revoke))
        .route("/api/collab/tokens/role", post(tokens_role))
}

// ------------------------------------------------------------ CLI: tokens

/// `pentool collab token create DOC`: acts as the actor named by the global
/// `--actor-*` flags, which must be explicit so the owner does not mint a
/// token for their own identity by accident.
pub fn token_create(
    document: &Path,
    role: &str,
    name: &str,
    expires: Option<&str>,
    actor: Actor,
) -> Result<Value> {
    let dir = super::doc_dir(document)?;
    let expiry = expires.map(token::parse_duration_ms).transpose()?;
    let (record, secret) = token::create(&dir, Role::parse(role)?, actor, name, expiry)?;
    Ok(json!({
        "token": secret,
        "record": record.view(),
        "note": "this secret is shown once and only its hash is stored; pass it with --token or PENTOOL_COLLAB_TOKEN",
    }))
}

pub fn token_list(document: &Path) -> Result<Value> {
    let dir = super::doc_dir(document)?;
    let views: Vec<Value> = token::list(&dir)?.iter().map(|r| r.view()).collect();
    Ok(
        json!({"matches": views.len(), "returned": views.len(), "offset": 0,
        "limit": token::MAX_TOKENS, "has_more": false, "tokens": views}),
    )
}

pub fn token_revoke(document: &Path, id: &str) -> Result<Value> {
    let dir = super::doc_dir(document)?;
    Ok(json!({"revoked": token::revoke(&dir, id)?.view()}))
}

pub fn token_set_role(document: &Path, id: &str, role: &str) -> Result<Value> {
    let dir = super::doc_dir(document)?;
    Ok(json!({"updated": token::set_role(&dir, id, Role::parse(role)?)?.view()}))
}

/// Startup check for `serve --collab`: a non-loopback bind needs tokens.
pub fn check_bind(document: &Path, host: &str, allow_insecure_lan: bool) -> Result<()> {
    let dir = super::doc_dir(document)?;
    let loopback = host
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(host == "localhost");
    if loopback {
        return Ok(());
    }
    if token::count_active(&dir) == 0 {
        bail!("[permission-denied] refusing to listen on {host} without tokens: run `pentool collab token create {} --role editor --name NAME --actor-id ID` first", document.display())
    }
    if !allow_insecure_lan {
        bail!("[permission-denied] {host} is not a loopback address and the server speaks plain HTTP; pass --allow-insecure-lan to accept that on a trusted network")
    }
    Ok(())
}

// ------------------------------------------------------------ the client

struct Remote {
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl Remote {
    fn new(base: &str, token: &str, allow_insecure_lan: bool) -> Result<Remote> {
        if base.starts_with("http://") && !is_loopback_url(base) && !allow_insecure_lan {
            bail!("[permission-denied] {base} is plain HTTP off loopback; use https or pass --allow-insecure-lan on a trusted network")
        }
        if token.is_empty() || token.chars().any(|c| c.is_control() || c.is_whitespace()) {
            bail!("[invalid-input] the token is empty or malformed; pass --token or set PENTOOL_COLLAB_TOKEN")
        }
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Remote {
            base: base.into(),
            token: token.into(),
            http,
        })
    }

    fn read(&self, response: reqwest::blocking::Response) -> Result<Value> {
        let status = response.status();
        let limit = (Limits::CEILING_BATCH_BYTES + 1024) as u64;
        if response.content_length().is_some_and(|len| len > limit) {
            bail!("[limit-exceeded] peer response is larger than {limit} bytes")
        }
        let bytes = response.bytes()?;
        if bytes.len() as u64 > limit {
            bail!("[limit-exceeded] peer response is larger than {limit} bytes")
        }
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(value);
        }
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("peer answered HTTP {status}"));
        if message.starts_with('[') {
            Err(anyhow!("{message} (peer {})", self.base))
        } else {
            Err(anyhow!("[peer-error] {message} (peer {})", self.base))
        }
    }

    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let response = self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .query(query)
            .send()
            .map_err(|e| {
                anyhow!(
                    "[peer-unreachable] could not reach {}: {}",
                    self.base,
                    e.without_url()
                )
            })?;
        self.read(response)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(body)?)
            .send()
            .map_err(|e| {
                anyhow!(
                    "[peer-unreachable] could not reach {}: {}",
                    self.base,
                    e.without_url()
                )
            })?;
        self.read(response)
    }
}

fn find_peer(store: &PeerStore, which: &str) -> Result<Peer> {
    let normalized = normalize_peer_url(which).ok();
    store
        .peers
        .iter()
        .find(|p| p.name == which || Some(&p.url) == normalized.as_ref())
        .cloned()
        .ok_or_else(|| {
            anyhow!("[not-found] {which:?} is not a registered peer; run `pentool collab peer add DOC URL` first")
        })
}

fn update_peer(dir: &Path, name: &str, apply: impl FnOnce(&mut Peer)) -> Result<()> {
    let _lock = log::lock(dir)?;
    let mut store = load_peers(dir)?;
    if let Some(peer) = store.peers.iter_mut().find(|p| p.name == name) {
        apply(peer);
        save_peers(dir, &store)?;
    }
    Ok(())
}

fn merge_counts(total: &mut BTreeMap<&'static str, usize>, outcome: &Outcome) {
    *total.entry("accepted").or_default() += outcome.accepted.len();
    *total.entry("duplicate").or_default() += outcome.duplicate.len();
    *total.entry("rejected").or_default() += outcome.rejected.len();
    *total.entry("pending").or_default() += outcome.pending.len();
    *total.entry("merge_rejected").or_default() += outcome.merge_rejected.len();
}

/// `pentool collab sync DOC --peer P --token T`: pull what we lack, then push
/// what the peer lacks. Safe to repeat, interrupt and resume.
pub fn sync_with_peer(
    document: &Path,
    peer_ref: &str,
    token: &str,
    allow_insecure_lan: bool,
) -> Result<Value> {
    let dir = super::doc_dir(document)?;
    let limits = Limits::default();
    let peer = find_peer(&load_peers(&dir)?, peer_ref)?;
    let remote = Remote::new(&peer.url, token, allow_insecure_lan)?;

    let info = remote.get("/api/collab/session", &[])?;
    if info.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL) {
        bail!("[unsupported-op] the peer speaks collaboration protocol {:?}; this build speaks {PROTOCOL}", info.get("protocol"))
    }
    let (genesis_bytes, _) = log::read_genesis(&dir)?;
    let local_genesis = log::genesis_hash(&genesis_bytes);
    if info.get("genesis").and_then(Value::as_str) != Some(local_genesis.as_str()) {
        bail!("[genesis-mismatch] the peer's document does not share this document's history; use `pentool collab join` to start from the peer's copy")
    }
    let role = Role::parse(info.get("role").and_then(Value::as_str).unwrap_or("viewer"))?;
    let session_actor: Actor =
        serde_json::from_value(info.get("actor").cloned().unwrap_or(Value::Null))
            .context("[peer-error] the peer sent no actor identity")?;
    let remote_ops_per_batch = info
        .pointer("/limits/batch_ops")
        .and_then(Value::as_u64)
        .unwrap_or(limits.batch_ops as u64) as usize;
    let remote_head = info
        .pointer("/head/seq")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    // Pull.
    let mut since = if peer.pull_seq > remote_head {
        0
    } else {
        peer.pull_seq
    };
    let mut pulled_ids: HashSet<String> = HashSet::new();
    let mut pulled = BTreeMap::new();
    let mut conflicts: Vec<merge::Conflict> = Vec::new();
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 100_000 {
            bail!("[peer-error] the peer kept sending pages without finishing; stopping")
        }
        let page = remote.get(
            "/api/collab/ops",
            &[
                ("since", since.to_string()),
                ("limit", remote_ops_per_batch.min(200).to_string()),
            ],
        )?;
        let ops = page
            .get("ops")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let actors: Vec<Actor> =
            serde_json::from_value(page.get("actors").cloned().unwrap_or(json!([])))
                .context("[peer-error] the peer sent malformed actor records")?;
        let next = page.get("next").and_then(Value::as_u64).unwrap_or(since);
        let more = page
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if ops.is_empty() && more {
            bail!("[peer-error] the peer reported more operations but sent none")
        }
        for op in &ops {
            if let Some(id) = op.get("id").and_then(Value::as_str) {
                pulled_ids.insert(id.to_string());
            }
        }
        let outcome = integrate(document, ops, &actors, Origin::Pull, &limits)?;
        merge_counts(&mut pulled, &outcome);
        conflicts.extend(outcome.conflicts);
        // The cursor only moves after the page is durably integrated.
        since = next.max(since);
        update_peer(&dir, &peer.name, |p| p.pull_seq = since)?;
        if !more {
            break;
        }
    }

    // Push.
    let mut pushed = BTreeMap::new();
    let mut push_rejections: Vec<Value> = Vec::new();
    let mut pushed_to = peer.pushed_seq;
    if role >= Role::Editor {
        let lines = log::read_lines(&dir)?;
        let local_head = lines.last().map(|l| l.seq).unwrap_or(0);
        if pushed_to > local_head {
            pushed_to = 0;
        }
        let mine: Vec<&Line> = lines
            .iter()
            .filter(|l| {
                l.seq > pushed_to
                    && l.op.actor == session_actor.id
                    && !pulled_ids.contains(&l.op.id)
            })
            .collect();
        let batch_cap = remote_ops_per_batch.clamp(1, 200);
        let mut index = 0;
        while index < mine.len() {
            let mut batch: Vec<&Line> = Vec::new();
            let mut bytes = 0usize;
            while index < mine.len() && batch.len() < batch_cap {
                let size = serde_json::to_vec(&mine[index].op)?.len();
                if !batch.is_empty() && bytes + size > 3 * 1024 * 1024 {
                    break;
                }
                bytes += size;
                batch.push(mine[index]);
                index += 1;
            }
            let body = json!({"ops": batch.iter().map(|l| &l.op).collect::<Vec<_>>()});
            let reply = remote.post("/api/collab/ops", &body)?;
            for key in [
                "accepted",
                "duplicate",
                "rejected",
                "pending",
                "merge_rejected",
            ] {
                let count = reply.get(key).and_then(Value::as_array).map_or(0, Vec::len);
                *pushed.entry(key).or_insert(0usize) += count;
            }
            if let Some(items) = reply.get("rejected").and_then(Value::as_array) {
                push_rejections.extend(items.iter().cloned());
            }
            if let Some(items) = reply.get("conflicts").and_then(Value::as_array) {
                for item in items {
                    if let Ok(conflict) = serde_json::from_value(item.clone()) {
                        conflicts.push(conflict);
                    }
                }
            }
            let last = batch.last().map(|l| l.seq).unwrap_or(pushed_to);
            pushed_to = pushed_to.max(last);
            update_peer(&dir, &peer.name, |p| p.pushed_seq = pushed_to)?;
        }
        // Everything up to here is either pushed or not ours to push.
        pushed_to = pushed_to.max(local_head);
        update_peer(&dir, &peer.name, |p| p.pushed_seq = pushed_to)?;
    }
    let head = log::read_head(&dir)?;
    let revision = crate::transaction::revision(&fs::read(document)?);
    conflicts.sort_by(|a, b| a.id.cmp(&b.id));
    conflicts.dedup_by(|a, b| a.id == b.id);
    Ok(json!({
        "peer": peer.name,
        "role": role.name(),
        "pulled": pulled,
        "pushed": if role >= Role::Editor { json!(pushed) } else { json!("skipped: the token's role cannot submit operations") },
        "push_rejections": push_rejections,
        "conflicts": conflicts,
        "head": head,
        "document_revision": revision,
    }))
}

/// `pentool collab join DOC --peer URL --token T`: start a new local copy from
/// a peer's genesis and pull everything.
pub fn join(
    document: &Path,
    url: &str,
    name: Option<&str>,
    token: &str,
    allow_insecure_lan: bool,
) -> Result<Value> {
    if document.exists() {
        bail!(
            "[invalid-input] {} already exists; join creates a new copy (use `collab sync` for an existing one)",
            document.display()
        )
    }
    let url = normalize_peer_url(url)?;
    let remote = Remote::new(&url, token, allow_insecure_lan)?;
    let reply = remote.get("/api/collab/genesis", &[])?;
    let text = reply
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("[peer-error] the peer sent no genesis document"))?;
    let hash = reply.get("hash").and_then(Value::as_str).unwrap_or("");
    if log::genesis_hash(text.as_bytes()) != hash {
        bail!("[tampered-log] the genesis the peer sent does not match its hash")
    }
    let value: Value = serde_json::from_str(text).context("[peer-error] genesis is not JSON")?;
    crate::transaction::validate_value(&value)?;
    if let Some(parent) = document.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    crate::editing::atomic_write(document, text.as_bytes())?;
    super::init(document)?;
    peer_add(document, &url, name, Limits::default().max_peers)?;
    sync_with_peer(document, &url, token, allow_insecure_lan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_urls_are_strict() {
        assert_eq!(
            normalize_peer_url("http://127.0.0.1:4711/").unwrap(),
            "http://127.0.0.1:4711"
        );
        for bad in [
            "file:///etc/passwd",
            "http://user:pw@host:1",
            "http://host/../x",
            "http://host/a/%2e%2e/b",
            "http://host/?token=abc",
            "http://host/#x",
            "javascript:alert(1)",
            "http://",
            "",
            "http://host/a\\b",
        ] {
            assert!(normalize_peer_url(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn limits_are_clamped_to_ceilings() {
        let limits = Limits {
            batch_ops: 1_000_000,
            ops_per_second: 1_000_000,
            ..Limits::default()
        }
        .clamped();
        assert_eq!(limits.batch_ops, Limits::CEILING_BATCH_OPS);
        assert_eq!(limits.ops_per_second, Limits::CEILING_OPS_PER_SECOND);
    }

    #[test]
    fn collab_routes_are_classified() {
        assert_eq!(
            super::super::perm::unclassified_routes(include_str!("sync.rs")),
            Vec::<String>::new()
        );
    }
}

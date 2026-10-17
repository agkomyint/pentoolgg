//! v0.20.0 task 4: comments, mentions and review states stored outside the
//! scene (`docs/collaboration-comments.md`).
//!
//! Threads live in a sidecar at `.pentool/comments/<name>.json` next to the
//! document. The `.pen` file is never read for writing, so commenting cannot
//! change its bytes, revision, rendering or export. Writes use the shared
//! atomic writer under an inter-process lock; every thread carries a version
//! for optimistic concurrency. Targets that stop existing are kept and
//! reported as `dangling`, never dropped.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const SCHEMA: u64 = 1;
pub const MAX_BODY_BYTES: usize = 16 * 1024;
pub const MAX_THREADS: usize = 10_000;
pub const MAX_COMMENTS_PER_THREAD: usize = 500;
pub const MAX_MENTIONS: usize = 32;
pub const MAX_SIDECAR_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_LIST_LIMIT: usize = 1000;
pub const DEFAULT_LIST_LIMIT: usize = 50;
const MAX_ID_LEN: usize = 128;
const MAX_NAME_LEN: usize = 128;
const MAX_MENTION_LEN: usize = 64;

const KINDS: [&str; 2] = ["human", "agent"];
const REVIEW_STATES: [&str; 4] = ["none", "requested", "approved", "changes-requested"];

/// Who wrote a comment. Agents may record the model/tool and the directing human.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Actor {
    pub kind: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directed_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Target {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: Actor,
    pub body: String,
    pub mentions: Vec<String>,
    pub created_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub target: Target,
    pub status: String,
    pub review_state: String,
    pub version: u64,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Store {
    schema: u64,
    next_thread: u64,
    next_comment: u64,
    threads: Vec<Thread>,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            schema: SCHEMA,
            next_thread: 1,
            next_comment: 1,
            threads: Vec::new(),
        }
    }
}

/// Filters and window for [`list`].
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    pub status: Option<String>,
    pub review_state: Option<String>,
    pub page: Option<String>,
    pub object: Option<String>,
    pub mention: Option<String>,
    pub thread: Option<String>,
    pub comments: bool,
    pub offset: usize,
    pub limit: Option<usize>,
}

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn sidecar(document: &Path) -> Result<PathBuf> {
    let parent = document.parent().unwrap_or_else(|| Path::new("."));
    let name = document
        .file_name()
        .context("document path has no filename")?
        .to_string_lossy();
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(parent
        .join(".pentool")
        .join("comments")
        .join(format!("{safe}.json")))
}

fn lock(sidecar: &Path) -> Result<Lock> {
    let dir = sidecar.parent().context("sidecar has no directory")?;
    fs::create_dir_all(dir)?;
    let path = dir.join(format!(
        "{}.lock",
        sidecar.file_name().unwrap_or_default().to_string_lossy()
    ));
    for _ in 0..500 {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(Lock(path)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("[busy] comments for this document are locked by another pentool process")
}

fn load(sidecar: &Path) -> Result<Store> {
    if !sidecar.exists() {
        return Ok(Store::default());
    }
    let length = fs::metadata(sidecar)?.len();
    ensure!(
        length <= MAX_SIDECAR_BYTES,
        "[limit-exceeded] comments sidecar {} is {length} bytes; the limit is {MAX_SIDECAR_BYTES}",
        sidecar.display()
    );
    let store: Store = serde_json::from_slice(&fs::read(sidecar)?).map_err(|error| {
        anyhow::anyhow!(
            "[invalid-input] comments sidecar {} is malformed ({error}); repair or delete it, nothing was changed",
            sidecar.display()
        )
    })?;
    validate_store(&store, sidecar)?;
    Ok(store)
}

fn validate_store(store: &Store, sidecar: &Path) -> Result<()> {
    let bad = |reason: String| {
        anyhow::anyhow!(
            "[invalid-input] comments sidecar {} is invalid: {reason}; nothing was changed",
            sidecar.display()
        )
    };
    if store.schema != SCHEMA {
        return Err(bad(format!(
            "unsupported schema {} (this build reads {SCHEMA})",
            store.schema
        )));
    }
    if store.threads.len() > MAX_THREADS {
        return Err(bad(format!("more than {MAX_THREADS} threads")));
    }
    let mut threads = HashSet::new();
    let mut comments = HashSet::new();
    for thread in &store.threads {
        if !threads.insert(thread.id.as_str()) {
            return Err(bad(format!("duplicate thread id {}", thread.id)));
        }
        if !matches!(thread.status.as_str(), "open" | "resolved")
            || !REVIEW_STATES.contains(&thread.review_state.as_str())
        {
            return Err(bad(format!("thread {} has an unknown state", thread.id)));
        }
        if thread.comments.is_empty() || thread.comments.len() > MAX_COMMENTS_PER_THREAD {
            return Err(bad(format!("thread {} has a bad comment count", thread.id)));
        }
        for comment in &thread.comments {
            if !comments.insert(comment.id.as_str()) {
                return Err(bad(format!("duplicate comment id {}", comment.id)));
            }
        }
    }
    Ok(())
}

fn save(sidecar: &Path, store: &Store) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(store)?;
    ensure!(
        bytes.len() as u64 <= MAX_SIDECAR_BYTES,
        "[limit-exceeded] comments sidecar would exceed {MAX_SIDECAR_BYTES} bytes; resolve and prune threads"
    );
    crate::editing::atomic_write(sidecar, &bytes)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_LEN
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn validate_actor(actor: &Actor) -> Result<()> {
    ensure!(
        KINDS.contains(&actor.kind.as_str()),
        "[invalid-input] actor kind must be human or agent, not {:?}",
        actor.kind
    );
    ensure!(
        id_ok(&actor.id),
        "[invalid-input] actor id must be 1..={MAX_ID_LEN} characters without whitespace"
    );
    ensure!(
        !actor.name.trim().is_empty()
            && actor.name.len() <= MAX_NAME_LEN
            && !actor.name.chars().any(char::is_control),
        "[invalid-input] actor name must be 1..={MAX_NAME_LEN} printable bytes"
    );
    for text in [&actor.model, &actor.directed_by].into_iter().flatten() {
        ensure!(
            !text.is_empty() && text.len() <= MAX_NAME_LEN && !text.chars().any(char::is_control),
            "[invalid-input] actor model and directed_by must be 1..={MAX_NAME_LEN} printable bytes"
        );
    }
    ensure!(
        actor.kind == "agent" || (actor.model.is_none() && actor.directed_by.is_none()),
        "[invalid-input] model and directed_by apply only to agent actors"
    );
    Ok(())
}

/// `@name` tokens in order of first appearance, deduplicated and sorted.
pub fn parse_mentions(body: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let preceded_by_word = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
        if chars[i] == '@' && !preceded_by_word {
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '_' | '-' | '.'))
            {
                j += 1;
            }
            let name: String = chars[i + 1..j].iter().collect();
            let name = name.trim_end_matches(['.', '-']).to_string();
            if !name.is_empty() && name.len() <= MAX_MENTION_LEN {
                found.insert(name);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    found.into_iter().collect()
}

fn checked_body(body: &str) -> Result<Vec<String>> {
    ensure!(
        !body.trim().is_empty(),
        "[invalid-input] a comment needs a non-empty body"
    );
    ensure!(
        body.len() <= MAX_BODY_BYTES,
        "[limit-exceeded] comment body is {} bytes; the limit is {MAX_BODY_BYTES}",
        body.len()
    );
    let mentions = parse_mentions(body);
    ensure!(
        mentions.len() <= MAX_MENTIONS,
        "[limit-exceeded] comment mentions {} people; the limit is {MAX_MENTIONS}",
        mentions.len()
    );
    Ok(mentions)
}

fn collect_ids(value: &Value, into: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(id)) = map.get("id") {
                into.insert(id.clone());
            }
            map.values().for_each(|v| collect_ids(v, into));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_ids(v, into)),
        _ => {}
    }
}

/// Page ids and, per page, every object id. Documents without a `pages` array
/// (legacy v1-v3) have a single implicit page `page-1`.
pub(crate) struct Ids {
    pages: Vec<(String, HashSet<String>)>,
}

impl Ids {
    pub(crate) fn read(document: &Path) -> Result<Ids> {
        let bytes = fs::read(document)
            .with_context(|| format!("[not-found] could not read {}", document.display()))?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("[invalid-input] {} is not valid JSON", document.display()))?;
        let pages = match value.get("pages").and_then(Value::as_array) {
            Some(pages) => pages
                .iter()
                .enumerate()
                .map(|(index, page)| {
                    let id = page
                        .get("id")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .unwrap_or_else(|| format!("page-{}", index + 1));
                    let mut ids = HashSet::new();
                    if let Value::Object(map) = page {
                        map.iter()
                            .filter(|(key, _)| key.as_str() != "id")
                            .for_each(|(_, v)| collect_ids(v, &mut ids));
                    }
                    (id, ids)
                })
                .collect(),
            None => {
                let mut ids = HashSet::new();
                collect_ids(&value, &mut ids);
                vec![("page-1".into(), ids)]
            }
        };
        Ok(Ids { pages })
    }

    pub(crate) fn has_page(&self, page: &str) -> bool {
        self.pages.iter().any(|(id, _)| id == page)
    }

    pub(crate) fn has_object(&self, page: Option<&str>, object: &str) -> bool {
        self.pages
            .iter()
            .filter(|(id, _)| page.is_none_or(|p| p == id))
            .any(|(_, ids)| ids.contains(object))
    }

    fn target_resolves(&self, target: &Target) -> bool {
        target.page.as_deref().is_none_or(|p| self.has_page(p))
            && target
                .object
                .as_deref()
                .is_none_or(|o| self.has_object(target.page.as_deref(), o))
    }
}

fn validate_target(target: &Target, ids: &Ids) -> Result<()> {
    if let Some(page) = &target.page {
        ensure!(id_ok(page), "[invalid-input] page id is not a valid id");
        ensure!(
            ids.has_page(page),
            "[not-found] page {page:?} does not exist in the document; list pages with `pentool tree`"
        );
    }
    if let Some(object) = &target.object {
        ensure!(id_ok(object), "[invalid-input] object id is not a valid id");
        ensure!(
            ids.has_object(target.page.as_deref(), object),
            "[not-found] object {object:?} does not exist{}; find ids with `pentool tree`",
            target
                .page
                .as_deref()
                .map(|p| format!(" on page {p:?}"))
                .unwrap_or_default()
        );
    }
    if let Some([x, y]) = target.point {
        ensure!(
            x.is_finite() && y.is_finite() && x.abs() <= 1e9 && y.abs() <= 1e9,
            "[invalid-input] point coordinates must be finite and within 1e9"
        );
    }
    Ok(())
}

fn find<'a>(store: &'a mut Store, id: &str) -> Result<&'a mut Thread> {
    store
        .threads
        .iter_mut()
        .find(|t| t.id == id)
        .with_context(|| format!("[not-found] comment thread {id:?} does not exist; list threads with `pentool comment list`"))
}

fn check_version(thread: &Thread, expected: Option<u64>) -> Result<()> {
    if let Some(wanted) = expected {
        ensure!(
            wanted == thread.version,
            "[conflict] thread {} is at version {} but version {wanted} was expected; re-read it and retry",
            thread.id,
            thread.version
        );
    }
    Ok(())
}

fn new_comment(store: &mut Store, author: &Actor, body: &str, mentions: Vec<String>) -> Comment {
    let id = format!("c-{}", store.next_comment);
    store.next_comment += 1;
    Comment {
        id,
        author: author.clone(),
        body: body.into(),
        mentions,
        created_ms: now_ms(),
    }
}

/// Start a thread on a target (or on the document when the target is empty).
pub fn add(document: &Path, target: Target, author: &Actor, body: &str) -> Result<Value> {
    validate_actor(author)?;
    let mentions = checked_body(body)?;
    let ids = Ids::read(document)?;
    validate_target(&target, &ids)?;
    let path = sidecar(document)?;
    let _lock = lock(&path)?;
    let mut store = load(&path)?;
    ensure!(
        store.threads.len() < MAX_THREADS,
        "[limit-exceeded] the document already has {MAX_THREADS} comment threads"
    );
    let comment = new_comment(&mut store, author, body, mentions);
    let thread = Thread {
        id: format!("t-{}", store.next_thread),
        target,
        status: "open".into(),
        review_state: "none".into(),
        version: 1,
        comments: vec![comment],
    };
    store.next_thread += 1;
    store.threads.push(thread.clone());
    save(&path, &store)?;
    Ok(json!({"ok": true, "thread": thread_value(&thread, &ids, true)}))
}

pub fn reply(
    document: &Path,
    thread_id: &str,
    author: &Actor,
    body: &str,
    expected: Option<u64>,
) -> Result<Value> {
    validate_actor(author)?;
    let mentions = checked_body(body)?;
    let ids = Ids::read(document)?;
    let path = sidecar(document)?;
    let _lock = lock(&path)?;
    let mut store = load(&path)?;
    let comment = {
        let thread = find(&mut store, thread_id)?;
        check_version(thread, expected)?;
        ensure!(
            thread.comments.len() < MAX_COMMENTS_PER_THREAD,
            "[limit-exceeded] thread {thread_id} already has {MAX_COMMENTS_PER_THREAD} comments"
        );
        thread.id.clone()
    };
    let new = new_comment(&mut store, author, body, mentions);
    let thread = find(&mut store, &comment)?;
    thread.comments.push(new);
    thread.version += 1;
    let out = thread.clone();
    save(&path, &store)?;
    Ok(json!({"ok": true, "thread": thread_value(&out, &ids, true)}))
}

/// Mark a thread resolved (`resolved = true`) or open again.
pub fn set_resolved(
    document: &Path,
    thread_id: &str,
    resolved: bool,
    expected: Option<u64>,
) -> Result<Value> {
    mutate(document, thread_id, expected, |thread| {
        thread.status = if resolved { "resolved" } else { "open" }.into();
        Ok(())
    })
}

/// Set a thread's review state: none, requested, approved or changes-requested.
pub fn set_review(
    document: &Path,
    thread_id: &str,
    state: &str,
    expected: Option<u64>,
) -> Result<Value> {
    ensure!(
        REVIEW_STATES.contains(&state),
        "[invalid-input] review state must be one of {}, not {state:?}",
        REVIEW_STATES.join(", ")
    );
    mutate(document, thread_id, expected, |thread| {
        thread.review_state = state.into();
        Ok(())
    })
}

fn mutate(
    document: &Path,
    thread_id: &str,
    expected: Option<u64>,
    change: impl FnOnce(&mut Thread) -> Result<()>,
) -> Result<Value> {
    let ids = Ids::read(document)?;
    let path = sidecar(document)?;
    let _lock = lock(&path)?;
    let mut store = load(&path)?;
    let thread = find(&mut store, thread_id)?;
    check_version(thread, expected)?;
    let before = (thread.status.clone(), thread.review_state.clone());
    change(thread)?;
    let changed = before != (thread.status.clone(), thread.review_state.clone());
    if changed {
        thread.version += 1;
    }
    let out = thread.clone();
    if changed {
        save(&path, &store)?;
    }
    Ok(json!({"ok": true, "changed": changed, "thread": thread_value(&out, &ids, false)}))
}

fn thread_value(thread: &Thread, ids: &Ids, with_comments: bool) -> Value {
    let last = thread.comments.last();
    let mut value = json!({
        "id": thread.id,
        "target": thread.target,
        "status": thread.status,
        "review_state": thread.review_state,
        "version": thread.version,
        "comment_count": thread.comments.len(),
        "opened_by": thread.comments.first().map(|c| &c.author),
        "last_activity_ms": last.map(|c| c.created_ms),
        "dangling": !ids.target_resolves(&thread.target),
    });
    if with_comments {
        value["comments"] = json!(thread.comments);
    }
    value
}

fn matches(thread: &Thread, options: &ListOptions) -> bool {
    options.status.as_deref().is_none_or(|s| s == thread.status)
        && options
            .review_state
            .as_deref()
            .is_none_or(|s| s == thread.review_state)
        && options.thread.as_deref().is_none_or(|t| t == thread.id)
        && options
            .page
            .as_deref()
            .is_none_or(|p| thread.target.page.as_deref() == Some(p))
        && options
            .object
            .as_deref()
            .is_none_or(|o| thread.target.object.as_deref() == Some(o))
        && options.mention.as_deref().is_none_or(|m| {
            let m = m.trim_start_matches('@');
            thread
                .comments
                .iter()
                .any(|c| c.mentions.iter().any(|x| x == m))
        })
}

/// Compact, deterministic, paginated listing in creation order.
pub fn list(document: &Path, options: &ListOptions) -> Result<Value> {
    if let Some(status) = &options.status {
        ensure!(
            matches!(status.as_str(), "open" | "resolved"),
            "[invalid-input] status filter must be open or resolved, not {status:?}"
        );
    }
    if let Some(state) = &options.review_state {
        ensure!(
            REVIEW_STATES.contains(&state.as_str()),
            "[invalid-input] review-state filter must be one of {}, not {state:?}",
            REVIEW_STATES.join(", ")
        );
    }
    let limit = options.limit.unwrap_or(DEFAULT_LIST_LIMIT);
    ensure!(
        (1..=MAX_LIST_LIMIT).contains(&limit),
        "[limit-exceeded] --limit must be 1..={MAX_LIST_LIMIT}, not {limit}"
    );
    let ids = Ids::read(document)?;
    let path = sidecar(document)?;
    let store = load(&path)?;
    if let Some(thread) = &options.thread {
        ensure!(
            store.threads.iter().any(|t| &t.id == thread),
            "[not-found] comment thread {thread:?} does not exist"
        );
    }
    let selected: Vec<&Thread> = store
        .threads
        .iter()
        .filter(|t| matches(t, options))
        .collect();
    let window: Vec<Value> = selected
        .iter()
        .skip(options.offset)
        .take(limit)
        .map(|t| thread_value(t, &ids, options.comments || options.thread.is_some()))
        .collect();
    let dangling = store
        .threads
        .iter()
        .filter(|t| !ids.target_resolves(&t.target))
        .count();
    Ok(json!({
        "matches": selected.len(),
        "returned": window.len(),
        "offset": options.offset,
        "limit": limit,
        "has_more": options.offset + window.len() < selected.len(),
        "total_threads": store.threads.len(),
        "dangling": dangling,
        "threads": window,
    }))
}

fn text<'a>(request: &'a Value, key: &str) -> Result<&'a str> {
    request
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("[invalid-input] the request needs a string {key:?}"))
}

fn actor_from(request: &Value) -> Result<Actor> {
    let actor = request
        .get("actor")
        .context("[invalid-input] the request needs an \"actor\" object")?;
    serde_json::from_value(actor.clone())
        .map_err(|error| anyhow::anyhow!("[invalid-input] bad actor: {error}"))
}

/// Apply one JSON request (used by `POST /api/comments`).
pub fn apply(document: &Path, request: &Value) -> Result<Value> {
    let expected = request.get("if_version").and_then(Value::as_u64);
    match text(request, "action")? {
        "add" => {
            let target: Target = match request.get("target") {
                Some(target) => serde_json::from_value(target.clone())
                    .map_err(|e| anyhow::anyhow!("[invalid-input] bad target: {e}"))?,
                None => Target::default(),
            };
            add(document, target, &actor_from(request)?, text(request, "body")?)
        }
        "reply" => reply(
            document,
            text(request, "thread")?,
            &actor_from(request)?,
            text(request, "body")?,
            expected,
        ),
        "resolve" => set_resolved(document, text(request, "thread")?, true, expected),
        "reopen" => set_resolved(document, text(request, "thread")?, false, expected),
        "review" => set_review(
            document,
            text(request, "thread")?,
            text(request, "state")?,
            expected,
        ),
        other => bail!(
            "[invalid-input] unknown comments action {other:?}; use add, reply, resolve, reopen or review"
        ),
    }
}

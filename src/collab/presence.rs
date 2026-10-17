//! Best-effort presence (spec section 9.4; roadmap v0.20.0 item 5): who is on
//! which page, what they have selected, where their cursor is.
//!
//! Presence is held in memory only. It is never written to disk, the document,
//! history or the operation log, so it cannot change a revision, a render or an
//! export. An entry is keyed by the *token-derived* actor, never by anything
//! the client sends, expires a short time after its last update, and every
//! input is bounded and checked before any work happens. Under pressure it is
//! dropped (`rate-limited`, `limit-exceeded`) instead of queued.
use super::{
    op::{check_id, Actor},
    perm::problem,
    sync::{blocking, error_response, principal_key, Ctx, StreamSlot},
    token::Principal,
};
use crate::comments::Ids;
use anyhow::{anyhow, bail, Result};
use axum::{
    body::Bytes,
    extract::Query,
    http::{header, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant, SystemTime},
};

/// One presence update is a few hundred bytes; anything larger is refused
/// before it is parsed.
pub const MAX_BODY_BYTES: usize = 4 * 1024;
/// Selected object ids per update.
pub const MAX_SELECTION: usize = 64;
/// Coordinates must be finite and within this magnitude.
pub const COORD_LIMIT: f64 = 1e9;

/// Section 10 presence limits. A server can lower but never raise them above
/// the hard ceilings.
#[derive(Debug, Clone)]
pub struct PresenceLimits {
    /// Accepted updates per token per second (spec default 10, ceiling 30).
    pub updates_per_second: usize,
    /// Participants tracked at once.
    pub max_entries: usize,
    /// An entry disappears this long after its last update or heartbeat.
    pub ttl_ms: u64,
}

impl Default for PresenceLimits {
    fn default() -> Self {
        PresenceLimits {
            updates_per_second: 10,
            max_entries: 64,
            ttl_ms: 15_000,
        }
    }
}

impl PresenceLimits {
    pub const CEILING_UPDATES_PER_SECOND: usize = 30;
    pub const CEILING_ENTRIES: usize = 256;
    pub const CEILING_TTL_MS: u64 = 60_000;

    pub fn clamped(mut self) -> Self {
        self.updates_per_second = self
            .updates_per_second
            .clamp(1, Self::CEILING_UPDATES_PER_SECOND);
        self.max_entries = self.max_entries.clamp(1, Self::CEILING_ENTRIES);
        self.ttl_ms = self.ttl_ms.clamp(1_000, Self::CEILING_TTL_MS);
        self
    }

    pub fn view(&self) -> Value {
        json!({
            "updates_per_second": self.updates_per_second,
            "entries": self.max_entries,
            "ttl_ms": self.ttl_ms,
            "selection": MAX_SELECTION,
            "body_bytes": MAX_BODY_BYTES,
        })
    }
}

// ---------------------------------------------------------------- updates

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What a client may send. Every field is optional: an update replaces the
/// whole state with what it carries, and `{}` only keeps the entry alive.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    /// Remove this participant now.
    #[serde(default)]
    pub leave: bool,
    pub page: Option<String>,
    pub selection: Option<Vec<String>>,
    pub cursor: Option<Point>,
    pub viewport: Option<Viewport>,
    /// Never trusted: accepted only so a forged value can be refused by name.
    pub actor: Option<Value>,
}

fn finite(what: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value.abs() > COORD_LIMIT {
        bail!("[invalid-input] {what} must be a finite number within {COORD_LIMIT:e}")
    }
    Ok(())
}

impl Update {
    /// Size-check, parse and validate a request body. Nothing is stored.
    pub fn parse(body: &[u8]) -> Result<Update> {
        if body.len() > MAX_BODY_BYTES {
            bail!(
                "[limit-exceeded] presence update is {} bytes; the limit is {MAX_BODY_BYTES}",
                body.len()
            )
        }
        let mut update: Update = if body.iter().all(u8::is_ascii_whitespace) {
            Update::default()
        } else {
            serde_json::from_slice(body)
                .map_err(|error| anyhow!("[invalid-input] malformed presence update: {error}"))?
        };
        update.validate()?;
        Ok(update)
    }

    fn validate(&mut self) -> Result<()> {
        if let Some(page) = &self.page {
            check_id("presence page", page)?;
        }
        if let Some(selection) = &mut self.selection {
            if selection.len() > MAX_SELECTION {
                bail!(
                    "[limit-exceeded] {} selected ids in one presence update; the limit is {MAX_SELECTION}",
                    selection.len()
                )
            }
            for id in selection.iter() {
                check_id("selected object id", id)?;
            }
            selection.sort();
            selection.dedup();
        }
        if let Some(cursor) = &self.cursor {
            finite("cursor.x", cursor.x)?;
            finite("cursor.y", cursor.y)?;
        }
        if let Some(view) = &self.viewport {
            finite("viewport.x", view.x)?;
            finite("viewport.y", view.y)?;
            finite("viewport.width", view.width)?;
            finite("viewport.height", view.height)?;
            if view.width < 0.0 || view.height < 0.0 {
                bail!("[invalid-input] viewport width and height cannot be negative")
            }
        }
        if self.leave && self.carries_state() {
            bail!("[invalid-input] a leave update cannot also carry presence state")
        }
        Ok(())
    }

    fn carries_state(&self) -> bool {
        self.page.is_some()
            || self.selection.is_some()
            || self.cursor.is_some()
            || self.viewport.is_some()
    }

    /// `{}`: refresh the entry's expiry and change nothing else.
    pub fn is_heartbeat(&self) -> bool {
        !self.leave && !self.carries_state()
    }

    /// A client-supplied actor must be the token's own; presence is always
    /// recorded for the token-derived actor, whatever the body says.
    pub fn check_actor(&self, actor: &Actor) -> Result<()> {
        let claimed = match &self.actor {
            None => return Ok(()),
            Some(Value::String(id)) => Some(id.as_str()),
            Some(Value::Object(map)) => map.get("id").and_then(Value::as_str),
            Some(_) => None,
        };
        if claimed != Some(actor.id.as_str()) {
            bail!("[permission-denied] presence is published as the token's own actor; the request named a different one")
        }
        Ok(())
    }

    fn needs_flags(&self) -> bool {
        self.page.is_some() || self.selection.as_ref().is_some_and(|s| !s.is_empty())
    }
}

/// Ids a client named that the document does not contain. They are kept and
/// shown, but marked: presence never vouches for them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Flags {
    pub unknown_page: bool,
    pub unknown_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct State {
    page: Option<String>,
    selection: Vec<String>,
    cursor: Option<Point>,
    viewport: Option<Viewport>,
    flags: Flags,
}

struct Entry {
    actor: Actor,
    state: State,
    last_seen_ms: u64,
}

/// What `publish` did.
#[derive(Debug, Clone)]
pub struct Published {
    pub created: bool,
    pub version: u64,
    pub flags: Flags,
}

// ------------------------------------------------------------------ store

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

#[derive(Default)]
struct Inner {
    entries: BTreeMap<String, Entry>,
    /// Per token: start of the current one-second window and updates in it.
    rate: HashMap<String, (u64, usize)>,
}

struct Known {
    len: u64,
    modified: Option<SystemTime>,
    ids: Arc<Ids>,
}

/// The in-memory presence table of one served document.
pub struct Store {
    limits: PresenceLimits,
    clock: Clock,
    inner: Mutex<Inner>,
    /// Bumped whenever what a viewer would see changes (not on heartbeats).
    version: AtomicU64,
    known: Mutex<Option<Known>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Store {
    /// A store on a monotonic clock that starts now.
    pub fn new(limits: PresenceLimits) -> Store {
        let start = Instant::now();
        Store::with_clock(limits, Arc::new(move || start.elapsed().as_millis() as u64))
    }

    /// A store on a caller-supplied millisecond clock (tests drive expiry with
    /// it instead of sleeping).
    pub fn with_clock(limits: PresenceLimits, clock: Clock) -> Store {
        Store {
            limits: limits.clamped(),
            clock,
            inner: Mutex::new(Inner::default()),
            version: AtomicU64::new(0),
            known: Mutex::new(None),
        }
    }

    pub fn limits(&self) -> &PresenceLimits {
        &self.limits
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    fn bump(&self) -> u64 {
        self.version.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Fixed one-second window per token. Called before an update is parsed.
    pub fn allow(&self, key: &str) -> bool {
        let now = (self.clock)();
        let mut inner = lock(&self.inner);
        if inner.rate.len() > 4096 {
            inner
                .rate
                .retain(|_, (start, _)| now.saturating_sub(*start) < 1000);
        }
        let slot = inner.rate.entry(key.to_string()).or_insert((now, 0));
        if now.saturating_sub(slot.0) >= 1000 {
            *slot = (now, 0);
        }
        if slot.1 >= self.limits.updates_per_second {
            return false;
        }
        slot.1 += 1;
        true
    }

    fn evict(&self, inner: &mut Inner, now: u64) -> bool {
        let before = inner.entries.len();
        let ttl = self.limits.ttl_ms;
        inner
            .entries
            .retain(|_, entry| now.saturating_sub(entry.last_seen_ms) < ttl);
        inner.entries.len() != before
    }

    /// Mark ids the document does not contain. Reads (and caches by file
    /// length and mtime) the document's ids; a document that cannot be read
    /// flags nothing.
    pub fn flag(&self, document: &Path, update: &Update) -> Flags {
        if !update.needs_flags() {
            return Flags::default();
        }
        let Some(ids) = self.known_ids(document) else {
            return Flags::default();
        };
        let page = update.page.as_deref();
        Flags {
            unknown_page: page.is_some_and(|page| !ids.has_page(page)),
            unknown_ids: update
                .selection
                .iter()
                .flatten()
                .filter(|id| !ids.has_object(page, id))
                .cloned()
                .collect(),
        }
    }

    fn known_ids(&self, document: &Path) -> Option<Arc<Ids>> {
        let meta = std::fs::metadata(document).ok()?;
        let (len, modified) = (meta.len(), meta.modified().ok());
        let mut cache = lock(&self.known);
        if let Some(known) = cache.as_ref() {
            if known.len == len && known.modified == modified {
                return Some(known.ids.clone());
            }
        }
        let ids = Arc::new(Ids::read(document).ok()?);
        *cache = Some(Known {
            len,
            modified,
            ids: ids.clone(),
        });
        Some(ids)
    }

    /// Record an update for `actor` (the token's actor, never a client value).
    pub fn publish(&self, actor: &Actor, update: &Update, flags: Flags) -> Result<Published> {
        let now = (self.clock)();
        let mut inner = lock(&self.inner);
        let mut changed = self.evict(&mut inner, now);
        let exists = inner.entries.contains_key(&actor.id);
        if !exists && inner.entries.len() >= self.limits.max_entries {
            if changed {
                self.bump();
            }
            bail!(
                "[limit-exceeded] presence already tracks {} participants; an entry expires {} s after its last update",
                self.limits.max_entries,
                self.limits.ttl_ms / 1000
            )
        }
        let fresh = if update.is_heartbeat() {
            None
        } else {
            Some(State {
                page: update.page.clone(),
                selection: update.selection.clone().unwrap_or_default(),
                cursor: update.cursor,
                viewport: update.viewport,
                flags,
            })
        };
        let state = match inner.entries.get_mut(&actor.id) {
            Some(entry) => {
                entry.last_seen_ms = now;
                if entry.actor != *actor {
                    entry.actor = actor.clone();
                    changed = true;
                }
                if let Some(state) = fresh {
                    if entry.state != state {
                        entry.state = state;
                        changed = true;
                    }
                }
                entry.state.clone()
            }
            None => {
                let state = fresh.unwrap_or_default();
                inner.entries.insert(
                    actor.id.clone(),
                    Entry {
                        actor: actor.clone(),
                        state: state.clone(),
                        last_seen_ms: now,
                    },
                );
                changed = true;
                state
            }
        };
        let version = if changed { self.bump() } else { self.version() };
        Ok(Published {
            created: !exists,
            version,
            flags: state.flags,
        })
    }

    /// Remove a participant. Returns whether it was present.
    pub fn leave(&self, actor_id: &str) -> bool {
        let now = (self.clock)();
        let mut inner = lock(&self.inner);
        let mut changed = self.evict(&mut inner, now);
        let removed = inner.entries.remove(actor_id).is_some();
        changed |= removed;
        if changed {
            self.bump();
        }
        removed
    }

    fn entry_view(entry: &Entry, now: u64) -> Value {
        let mut out = Map::new();
        out.insert("actor".into(), json!(entry.actor));
        let state = &entry.state;
        if let Some(page) = &state.page {
            out.insert("page".into(), json!(page));
        }
        out.insert("selection".into(), json!(state.selection));
        if let Some(cursor) = &state.cursor {
            out.insert("cursor".into(), json!({"x": cursor.x, "y": cursor.y}));
        }
        if let Some(view) = &state.viewport {
            out.insert(
                "viewport".into(),
                json!({"x": view.x, "y": view.y, "width": view.width, "height": view.height}),
            );
        }
        if state.flags.unknown_page {
            out.insert("unknown_page".into(), json!(true));
        }
        if !state.flags.unknown_ids.is_empty() {
            out.insert("unknown_ids".into(), json!(state.flags.unknown_ids));
        }
        out.insert(
            "age_ms".into(),
            json!(now.saturating_sub(entry.last_seen_ms)),
        );
        Value::Object(out)
    }

    /// Live participants ordered by actor id, with exact pagination fields.
    pub fn snapshot(&self, offset: usize, limit: usize) -> Value {
        let now = (self.clock)();
        let mut inner = lock(&self.inner);
        if self.evict(&mut inner, now) {
            self.bump();
        }
        let limit = limit.clamp(1, self.limits.max_entries);
        let total = inner.entries.len();
        let entries: Vec<Value> = inner
            .entries
            .values()
            .skip(offset)
            .take(limit)
            .map(|entry| Store::entry_view(entry, now))
            .collect();
        json!({
            "matches": total,
            "returned": entries.len(),
            "offset": offset,
            "limit": limit,
            "has_more": offset.saturating_add(entries.len()) < total,
            "version": self.version(),
            "ttl_ms": self.limits.ttl_ms,
            "entries": entries,
        })
    }

    /// The full table when it differs from what a stream last sent.
    pub fn view_if_changed(&self, last: Option<u64>) -> Option<(u64, Value)> {
        let now = (self.clock)();
        {
            let mut inner = lock(&self.inner);
            if self.evict(&mut inner, now) {
                self.bump();
            }
        }
        let version = self.version();
        if last == Some(version) {
            return None;
        }
        Some((version, self.snapshot(0, self.limits.max_entries)))
    }
}

// --------------------------------------------------------------- handlers

fn rate_limited(limit: usize) -> Response {
    let mut response = problem(
        StatusCode::TOO_MANY_REQUESTS,
        "rate-limited",
        format!("more than {limit} presence updates per second for this token; send the latest state on the next tick"),
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

/// `POST /api/collab/presence`: publish, heartbeat (`{}`) or leave
/// (`{"leave": true}`) as the token's actor.
pub async fn publish(
    Extension(ctx): Extension<Arc<Ctx>>,
    Extension(principal): Extension<Principal>,
    body: Bytes,
) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit-exceeded",
            format!(
                "presence update is {} bytes; the limit is {MAX_BODY_BYTES}",
                body.len()
            ),
        );
    }
    if !ctx.presence.allow(&principal_key(&principal)) {
        return rate_limited(ctx.presence.limits().updates_per_second);
    }
    let update = match Update::parse(&body).and_then(|u| u.check_actor(&principal.actor).map(|_| u))
    {
        Ok(update) => update,
        Err(error) => return error_response(error),
    };
    if update.leave {
        let left = ctx.presence.leave(&principal.actor.id);
        return Json(json!({"ok": true, "left": left, "version": ctx.presence.version()}))
            .into_response();
    }
    let flags = if update.needs_flags() {
        let (ctx, update) = (ctx.clone(), update.clone());
        match blocking(move || Ok(ctx.presence.flag(&ctx.document, &update))).await {
            Ok(flags) => flags,
            Err(error) => return error_response(error),
        }
    } else {
        Flags::default()
    };
    match ctx.presence.publish(&principal.actor, &update, flags) {
        Ok(done) => Json(json!({
            "ok": true,
            "created": done.created,
            "version": done.version,
            "ttl_ms": ctx.presence.limits().ttl_ms,
            "unknown_page": done.flags.unknown_page,
            "unknown_ids": done.flags.unknown_ids,
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
pub struct PageQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

/// `GET /api/collab/presence`: the live participants.
pub async fn snapshot(
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(ctx.presence.limits().max_entries);
    Json(ctx.presence.snapshot(query.offset.unwrap_or(0), limit)).into_response()
}

/// `GET /api/collab/presence/events`: the table as server-sent events, sent
/// when it changes. The 250 ms poll coalesces bursts, so a stream carries at
/// most four events a second however fast peers publish, and it shares the op
/// stream's slots (per token and in total).
pub async fn events(
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
                if let Some((version, view)) = ctx.presence.view_if_changed(last) {
                    let event = Event::default().event("presence").data(view.to_string());
                    return Some((Ok::<Event, Infallible>(event), (ctx, slot, Some(version))));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

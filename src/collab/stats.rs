//! Contribution statistics derived only from the verified operation log
//! (spec section 7.3), and the bounded summary embedded in the `.pen`
//! document's top-level `collab` object.
//!
//! The replayer keeps a small ledger while it applies operations (who wrote
//! which property, who changed which object) and exposes the surviving state
//! (who created and last wrote what still exists). Everything here is a pure
//! function of that replay, so the same log yields byte-identical output on
//! every platform, ordered by actor id.
use super::op::{Actor, Level};
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const SUMMARY_SCHEMA: u64 = 1;
/// Bound on the embedded `stats_summary` (serialized bytes).
pub const MAX_SUMMARY_BYTES: usize = 64 * 1024;
/// Bound on the whole `collab` object, including preserved unknown fields.
pub const MAX_COLLAB_BYTES: usize = 128 * 1024;
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 1000;
const UNKNOWN_KIND: &str = "unknown";
const POSITION: &str = "$position";

/// Counters the replayer maintains while applying operations. Cloned with the
/// replayer, so a rolled back scene batch rolls its statistics back too.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    /// Accepted top-level operations per `(actor, page)`.
    pub ops: BTreeMap<(String, String), u64>,
    /// Objects that received a property or structure change, per actor.
    pub modified: BTreeMap<String, BTreeSet<String>>,
    /// Distinct `(object, property)` pairs written, per actor.
    pub props: BTreeMap<String, BTreeSet<(String, String)>>,
}

impl Ledger {
    pub fn note_op(&mut self, actor: &str, page: &str) {
        *self
            .ops
            .entry((actor.to_string(), page.to_string()))
            .or_insert(0) += 1;
    }

    /// A property write: counts as a modification and as a written property.
    pub fn note_property(&mut self, actor: &str, object: &str, path: &str) {
        self.note_object(actor, object);
        if path != POSITION {
            self.props
                .entry(actor.to_string())
                .or_default()
                .insert((object.to_string(), path.to_string()));
        }
    }

    pub fn note_object(&mut self, actor: &str, object: &str) {
        self.modified
            .entry(actor.to_string())
            .or_default()
            .insert(object.to_string());
    }
}

/// Ledger plus the surviving-state attribution, ready for aggregation.
#[derive(Debug, Default)]
pub struct Raw {
    pub ledger: Ledger,
    /// Every object ever created (tombstones included) by creator.
    pub created: BTreeMap<String, BTreeSet<String>>,
    /// Objects that still exist.
    pub live: BTreeSet<String>,
    /// Current `(object, property)` values by their last writer.
    pub surviving: BTreeMap<String, BTreeSet<(String, String)>>,
    /// Page of every object reference.
    pub page_of: BTreeMap<String, String>,
}

/// Where an object lives, as a page id ("" for document-level objects).
pub fn page_for(level: Level, page: &str, id: &str) -> String {
    match level {
        Level::Doc => String::new(),
        Level::Page => id.to_string(),
        _ => page.to_string(),
    }
}

// ------------------------------------------------------------ metrics

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metrics {
    pub operations: u64,
    pub objects_created: u64,
    pub objects_modified: u64,
    pub properties_changed: u64,
    pub net_authored: u64,
    pub surviving_properties: u64,
}

impl Metrics {
    fn to_json(&self, share_bp: u64) -> Value {
        json!({
            "operations": self.operations,
            "share_bp": share_bp,
            "share": percent(share_bp),
            "objects_created": self.objects_created,
            "objects_modified": self.objects_modified,
            "properties_changed": self.properties_changed,
            "net_authored": self.net_authored,
            "surviving_properties": self.surviving_properties,
        })
    }
}

/// Exact percentage from integer basis points: `1234` -> `12.34%`.
pub fn percent(bp: u64) -> String {
    format!("{}.{:02}%", bp / 100, bp % 100)
}

/// Largest-remainder apportionment of 10,000 basis points; the result always
/// sums to exactly 10,000 (or 0 when there is nothing to share).
pub fn shares(counts: &[u64]) -> Vec<u64> {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return vec![0; counts.len()];
    }
    let mut bp: Vec<u64> = counts.iter().map(|c| c * 10_000 / total).collect();
    let mut order: Vec<usize> = (0..counts.len()).collect();
    order.sort_by(|a, b| {
        let ra = counts[*a] * 10_000 % total;
        let rb = counts[*b] * 10_000 % total;
        rb.cmp(&ra).then(a.cmp(b))
    });
    let mut left = 10_000 - bp.iter().sum::<u64>();
    for index in order {
        if left == 0 {
            break;
        }
        bp[index] += 1;
        left -= 1;
    }
    bp
}

struct Member<'a> {
    id: &'a str,
}

/// Union-based aggregate over a set of actors, optionally restricted to a page.
fn aggregate(raw: &Raw, members: &[Member<'_>], page: Option<&str>) -> Metrics {
    let in_page = |object: &str| match page {
        None => true,
        Some(wanted) => raw.page_of.get(object).map(String::as_str) == Some(wanted),
    };
    let mut operations = 0;
    let mut created: BTreeSet<&str> = BTreeSet::new();
    let mut modified: BTreeSet<&str> = BTreeSet::new();
    let mut props: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut surviving: BTreeSet<(&str, &str)> = BTreeSet::new();
    for member in members {
        for ((actor, op_page), count) in &raw.ledger.ops {
            if actor == member.id && page.is_none_or(|wanted| wanted == op_page) {
                operations += count;
            }
        }
        for object in raw.created.get(member.id).into_iter().flatten() {
            if in_page(object) {
                created.insert(object);
            }
        }
        for object in raw.ledger.modified.get(member.id).into_iter().flatten() {
            if in_page(object) {
                modified.insert(object);
            }
        }
        for (object, path) in raw.ledger.props.get(member.id).into_iter().flatten() {
            if in_page(object) {
                props.insert((object, path));
            }
        }
        for (object, path) in raw.surviving.get(member.id).into_iter().flatten() {
            if in_page(object) {
                surviving.insert((object, path));
            }
        }
    }
    let net = created.iter().filter(|o| raw.live.contains(**o)).count();
    let modified_not_own = modified.iter().filter(|o| !created.contains(**o)).count();
    Metrics {
        operations,
        objects_created: created.len() as u64,
        objects_modified: modified_not_own as u64,
        properties_changed: props.len() as u64,
        net_authored: net as u64,
        surviving_properties: surviving.len() as u64,
    }
}

// -------------------------------------------------------------- report

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Actor,
    Kind,
    Page,
}

impl By {
    pub fn parse(text: &str) -> Result<By> {
        Ok(match text {
            "actor" => By::Actor,
            "kind" => By::Kind,
            "page" => By::Page,
            other => bail!("[invalid-input] --by must be actor, kind or page, got {other:?}"),
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            By::Actor => "actor",
            By::Kind => "kind",
            By::Page => "page",
        }
    }
}

fn actor_ids(raw: &Raw) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for (actor, _) in raw.ledger.ops.keys() {
        ids.insert(actor.clone());
    }
    ids.extend(raw.ledger.modified.keys().cloned());
    ids.extend(raw.ledger.props.keys().cloned());
    ids.extend(raw.created.keys().cloned());
    ids.extend(raw.surviving.keys().cloned());
    ids
}

fn kind_of<'a>(registry: &'a [Actor], id: &str) -> &'a str {
    registry
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.kind.as_str())
        .unwrap_or(UNKNOWN_KIND)
}

fn row(key: &str, extra: Value, metrics: &Metrics, share_bp: u64) -> Value {
    let mut value = metrics.to_json(share_bp);
    let object = value.as_object_mut().expect("object");
    object.insert("key".into(), json!(key));
    if let Some(extra) = extra.as_object() {
        for (name, item) in extra {
            object.insert(name.clone(), item.clone());
        }
    }
    value
}

/// Full deterministic report. Row order: actor id, kind name, or page id.
pub fn report(raw: &Raw, registry: &[Actor], by: By, offset: usize, limit: usize) -> Result<Value> {
    if limit == 0 || limit > MAX_LIMIT {
        bail!("[limit-exceeded] --limit must be between 1 and {MAX_LIMIT}")
    }
    let actors = actor_ids(raw);
    let all: Vec<Member<'_>> = actors.iter().map(|id| Member { id }).collect();
    let total = aggregate(raw, &all, None);

    // Groups as (key, extra fields, metrics).
    let mut groups: Vec<(String, Value, Metrics)> = Vec::new();
    match by {
        By::Actor => {
            for id in &actors {
                let metrics = aggregate(raw, &[Member { id }], None);
                let known = registry.iter().find(|a| a.id == *id);
                groups.push((
                    id.clone(),
                    json!({
                        "kind": kind_of(registry, id),
                        "name": known.and_then(|a| a.display_name.clone()),
                        "directed_by": known.and_then(|a| a.directed_by.clone()),
                    }),
                    metrics,
                ));
            }
        }
        By::Kind => {
            let mut kinds: BTreeMap<&str, Vec<Member<'_>>> = BTreeMap::new();
            for id in &actors {
                kinds
                    .entry(kind_of(registry, id))
                    .or_default()
                    .push(Member { id });
            }
            for (kind, members) in &kinds {
                groups.push((
                    kind.to_string(),
                    json!({"actors": members.len()}),
                    aggregate(raw, members, None),
                ));
            }
        }
        By::Page => {
            let mut pages: BTreeSet<&str> = BTreeSet::new();
            for (_, page) in raw.ledger.ops.keys() {
                pages.insert(page);
            }
            for object in raw
                .ledger
                .modified
                .values()
                .flatten()
                .chain(raw.created.values().flatten())
            {
                if let Some(page) = raw.page_of.get(object) {
                    pages.insert(page);
                }
            }
            for page in pages {
                groups.push((
                    page.to_string(),
                    json!({}),
                    aggregate(raw, &all, Some(page)),
                ));
            }
        }
    }
    let counts: Vec<u64> = groups.iter().map(|(_, _, m)| m.operations).collect();
    let bp = shares(&counts);
    let matches = groups.len();
    let rows: Vec<Value> = groups
        .iter()
        .zip(&bp)
        .skip(offset)
        .take(limit)
        .map(|((key, extra, metrics), share)| row(key, extra.clone(), metrics, *share))
        .collect();

    let kind_total = |kind: &str| {
        let members: Vec<Member<'_>> = actors
            .iter()
            .filter(|id| kind_of(registry, id) == kind)
            .map(|id| Member { id })
            .collect();
        aggregate(raw, &members, None)
    };
    let human = kind_total("human");
    let agent = kind_total("agent");
    let kind_bp = shares(&[human.operations, agent.operations]);

    // By directing human: a human's own work plus that of the agents they direct.
    let mut directors: BTreeMap<String, Vec<Member<'_>>> = BTreeMap::new();
    for id in &actors {
        let actor = registry.iter().find(|a| a.id == *id);
        match actor {
            Some(a) if a.kind == "agent" => {
                if let Some(director) = &a.directed_by {
                    directors
                        .entry(director.clone())
                        .or_default()
                        .push(Member { id });
                }
            }
            Some(a) if a.kind == "human" => {
                directors
                    .entry(a.id.clone())
                    .or_default()
                    .push(Member { id });
            }
            _ => {}
        }
    }
    let director_counts: Vec<(String, Metrics)> = directors
        .iter()
        .map(|(director, members)| (director.clone(), aggregate(raw, members, None)))
        .collect();
    let director_bp = shares(
        &director_counts
            .iter()
            .map(|(_, m)| m.operations)
            .collect::<Vec<_>>(),
    );
    let by_director: Vec<Value> = director_counts
        .iter()
        .zip(&director_bp)
        .take(MAX_LIMIT)
        .map(|((id, metrics), share)| row(id, json!({}), metrics, *share))
        .collect();

    Ok(json!({
        "by": by.name(),
        "matches": matches,
        "returned": rows.len(),
        "offset": offset,
        "limit": limit,
        "has_more": offset.saturating_add(rows.len()) < matches,
        "total_operations": total.operations,
        "totals": {
            "all": total.to_json(if total.operations == 0 { 0 } else { 10_000 }),
            "human": human.to_json(kind_bp[0]),
            "agent": agent.to_json(kind_bp[1]),
        },
        "by_director": by_director,
        "precision_reduced": false,
        "rows": rows,
    }))
}

// ------------------------------------------------------- embedded block

/// Remove the document's `collab` object (the replay domain excludes it).
pub fn strip(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        map.remove("collab");
    }
}

/// The bounded summary that is embedded in the document. It carries no page
/// breakdown and no names: only counters per actor id and per kind.
pub fn summary(raw: &Raw, registry: &[Actor]) -> Result<Value> {
    let actors = actor_ids(raw);
    let counts: Vec<u64> = actors
        .iter()
        .map(|id| aggregate(raw, &[Member { id }], None).operations)
        .collect();
    let bp = shares(&counts);
    let all: Vec<Member<'_>> = actors.iter().map(|id| Member { id }).collect();
    let total = aggregate(raw, &all, None);
    let rows: Vec<Value> = actors
        .iter()
        .zip(&bp)
        .map(|(id, share)| {
            let metrics = aggregate(raw, &[Member { id }], None);
            row(id, json!({"kind": kind_of(registry, id)}), &metrics, *share)
        })
        .collect();
    let human = report_kind(raw, registry, &actors, "human");
    let agent = report_kind(raw, registry, &actors, "agent");
    let mut value = json!({
        "schema": SUMMARY_SCHEMA,
        "total_operations": total.operations,
        "precision_reduced": false,
        "kinds": {"human": human, "agent": agent},
        "actors": rows,
    });
    if serde_json::to_vec(&value)?.len() > MAX_SUMMARY_BYTES {
        // Degrade to per-kind attribution and say so.
        value["actors"] = json!([]);
        value["precision_reduced"] = json!(true);
    }
    Ok(value)
}

fn report_kind(raw: &Raw, registry: &[Actor], actors: &BTreeSet<String>, kind: &str) -> Value {
    let members: Vec<Member<'_>> = actors
        .iter()
        .filter(|id| kind_of(registry, id) == kind)
        .map(|id| Member { id })
        .collect();
    let metrics = aggregate(raw, &members, None);
    json!({
        "operations": metrics.operations,
        "objects_created": metrics.objects_created,
        "objects_modified": metrics.objects_modified,
        "properties_changed": metrics.properties_changed,
        "net_authored": metrics.net_authored,
        "surviving_properties": metrics.surviving_properties,
    })
}

/// Build the `collab` object for a document: preserve unknown fields of the
/// existing block, replace the schema, head and summary.
pub fn embed(existing: Option<&Value>, seq: u64, head: &str, summary: Value) -> Value {
    let mut block: Map<String, Value> = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    block.insert("schema".into(), json!(SUMMARY_SCHEMA));
    block.insert("seq".into(), json!(seq));
    block.insert("head".into(), json!(head));
    block.insert("stats_summary".into(), summary);
    Value::Object(block)
}

/// Structural and size validation of a document's `collab` object. Run by
/// `scene::validate`; limits are checked before any deeper work.
pub fn validate_block(document: &Value) -> Result<()> {
    let Some(block) = document.get("collab") else {
        return Ok(());
    };
    let Some(map) = block.as_object() else {
        bail!("[invalid-input] collab must be an object")
    };
    if let Some(summary) = map.get("stats_summary") {
        let Some(actors) = summary.get("actors").and_then(Value::as_array) else {
            bail!("[invalid-input] collab.stats_summary.actors must be an array")
        };
        if actors.len() > super::log::MAX_ACTORS {
            bail!(
                "[limit-exceeded] collab.stats_summary lists {} actors; the limit is {}",
                actors.len(),
                super::log::MAX_ACTORS
            )
        }
        if serde_json::to_vec(summary)?.len() > MAX_SUMMARY_BYTES {
            bail!("[limit-exceeded] collab.stats_summary exceeds {MAX_SUMMARY_BYTES} bytes")
        }
    }
    if serde_json::to_vec(block)?.len() > MAX_COLLAB_BYTES {
        bail!("[limit-exceeded] collab exceeds {MAX_COLLAB_BYTES} bytes")
    }
    if map.get("schema").and_then(Value::as_u64) != Some(SUMMARY_SCHEMA) {
        bail!("[unsupported-op] collab.schema must be {SUMMARY_SCHEMA}")
    }
    for (name, expected) in [("head", true), ("seq", false)] {
        if let Some(item) = map.get(name) {
            let ok = if expected {
                item.as_str().is_some_and(|s| s.len() <= 80)
            } else {
                item.is_u64()
            };
            if !ok {
                bail!("[invalid-input] collab.{name} is malformed")
            }
        }
    }
    Ok(())
}

/// Compare a document's embedded block with the one derived from the log.
pub fn check_embedded(document: &Value, seq: u64, head: &str, derived: &Value) -> Value {
    let Some(block) = document.get("collab") else {
        return json!({"status": "missing"});
    };
    let mut problems = Vec::new();
    if block.get("head").and_then(Value::as_str) != Some(head)
        || block.get("seq").and_then(Value::as_u64) != Some(seq)
    {
        problems.push("head");
    }
    if block.get("stats_summary") != Some(derived) {
        problems.push("stats_summary");
    }
    if problems.is_empty() {
        json!({"status": "verified"})
    } else {
        json!({
            "status": "mismatch",
            "fields": problems,
            "note": "the embedded summary disagrees with the operation log and is not trusted; the next recorded change rewrites it",
        })
    }
}

/// Compact table for the terminal; the JSON report is the machine contract.
pub fn render_text(report: &Value) -> String {
    let mut out = String::new();
    let by = report["by"].as_str().unwrap_or("actor");
    out.push_str(&format!(
        "stats by {by}: {} of {} rows, {} accepted operations
",
        report["returned"], report["matches"], report["total_operations"]
    ));
    out.push_str(
        "key	share	ops	created	modified	props	net	surviving
",
    );
    let line = |row: &Value, label: &str| {
        format!(
            "{label}	{}	{}	{}	{}	{}	{}	{}
",
            row["share"].as_str().unwrap_or("0.00%"),
            row["operations"],
            row["objects_created"],
            row["objects_modified"],
            row["properties_changed"],
            row["net_authored"],
            row["surviving_properties"],
        )
    };
    for row in report["rows"].as_array().into_iter().flatten() {
        let label = match row["kind"].as_str() {
            Some(kind) if by == "actor" => {
                format!("{} ({kind})", row["key"].as_str().unwrap_or(""))
            }
            _ => row["key"].as_str().unwrap_or("").to_string(),
        };
        out.push_str(&line(row, &label));
    }
    out.push_str(
        "totals
",
    );
    for name in ["human", "agent", "all"] {
        out.push_str(&line(&report["totals"][name], name));
    }
    if report["has_more"] == Value::Bool(true) {
        out.push_str(
            "more rows: raise --offset
",
        );
    }
    if report["embedded_summary"]["status"] == "mismatch" {
        out.push_str(
            "warning: the embedded collab summary disagrees with the log and is ignored
",
        );
    }
    out
}

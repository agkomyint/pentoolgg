//! Library update planning: one pure planner shared by dry run and commit
//! (spec section 4). Three-way statuses compare base, current and incoming hashes.
use super::{glob_match, library::Snapshot};
use crate::tokens::{self, DesignError, Usage};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, DesignError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    KeepLocal,
    TakeSource,
    Detach,
    MapTarget,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::KeepLocal => "keep-local",
            Action::TakeSource => "take-source",
            Action::Detach => "detach",
            Action::MapTarget => "map-target",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Resolution {
    pub token: String,
    pub action: Action,
    pub to: Option<String>,
}

/// Parse a resolutions file: `[{"token","action","to"?}]`.
pub fn parse_resolutions(value: &Value) -> Result<Vec<Resolution>> {
    let rows = value.as_array().ok_or_else(|| {
        DesignError::new("invalid-input", "resolutions must be a JSON array")
            .action("use [{\"token\":\"color.accent\",\"action\":\"keep-local\"}]")
    })?;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for row in rows {
        let token = row.get("token").and_then(Value::as_str).ok_or_else(|| {
            DesignError::new("invalid-input", "every resolution needs a string `token`")
        })?;
        let action = match row.get("action").and_then(Value::as_str) {
            Some("keep-local") => Action::KeepLocal,
            Some("take-source") => Action::TakeSource,
            Some("detach") => Action::Detach,
            Some("map-target") => Action::MapTarget,
            other => {
                return Err(DesignError::new(
                    "invalid-input",
                    format!("resolution for {token}: unknown action {other:?}"),
                )
                .token(token)
                .action("use keep-local, take-source, detach or map-target"))
            }
        };
        let to = row.get("to").and_then(Value::as_str).map(str::to_owned);
        if (action == Action::MapTarget) != to.is_some() {
            return Err(DesignError::new(
                "invalid-input",
                format!(
                    "resolution for {token}: `to` is required for, and only valid with, map-target"
                ),
            )
            .token(token));
        }
        if !seen.insert(token.to_owned()) {
            return Err(DesignError::new(
                "invalid-input",
                format!("duplicate resolution for {token}"),
            )
            .token(token));
        }
        out.push(Resolution {
            token: token.into(),
            action,
            to,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Only these token globs (plus their alias closure) are vendored.
    pub selection: Option<Vec<String>>,
    pub continue_on_conflict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Write {
    None,
    /// Adopt the incoming value and record its source.
    Set,
    /// Keep the local value; advance the recorded base.
    Advance,
    Detach,
    Delete(Option<String>),
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub token: String,
    pub status: &'static str,
    token_type: String,
    base: Option<String>,
    current: Option<String>,
    incoming: Option<String>,
    from: Option<Value>,
    to: Option<Value>,
    reason: Option<String>,
    resolution: Option<Action>,
    resolution_to: Option<String>,
    nodes: Vec<String>,
    tokens: Vec<String>,
    write: Write,
    pub skipped: bool,
}

impl Entry {
    fn blocking(&self) -> bool {
        matches!(self.status, "conflict" | "removed") && self.resolution.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub library: String,
    pub from: Option<String>,
    pub to: String,
    pub package_hash: String,
    pub entries: Vec<Entry>,
    pub writes: Vec<String>,
    pub resolutions_needed: Vec<String>,
    pub conflicts: Vec<String>,
    pub selection: Option<Vec<String>>,
    pub continue_on_conflict: bool,
}

const STATUSES: [&str; 7] = [
    "added",
    "removed",
    "changed",
    "kept-override",
    "converged",
    "conflict",
    "unchanged",
];

impl Plan {
    /// True when something blocks a commit (and `--continue-on-conflict` is off).
    pub fn blocked(&self) -> bool {
        !self.continue_on_conflict && !self.resolutions_needed.is_empty()
    }
    pub fn counts(&self) -> Value {
        let mut counts = Map::new();
        for status in STATUSES {
            counts.insert(status.into(), json!(0));
        }
        for entry in &self.entries {
            let slot = counts.get_mut(entry.status).unwrap();
            *slot = json!(slot.as_u64().unwrap() + 1);
        }
        Value::Object(counts)
    }
    /// Sorted entries as JSON; `unchanged` ones only when `all`.
    pub fn token_rows(&self, all: bool) -> Vec<Value> {
        self.entries
            .iter()
            .filter(|e| all || e.status != "unchanged")
            .map(|e| {
                let mut row = json!({"token": e.token, "status": e.status, "type": e.token_type});
                let object = row.as_object_mut().unwrap();
                for (key, value) in [
                    ("base", &e.base),
                    ("current", &e.current),
                    ("incoming", &e.incoming),
                    ("reason", &e.reason),
                ] {
                    if let Some(value) = value {
                        object.insert(key.into(), json!(value));
                    }
                }
                if let Some(value) = &e.from {
                    object.insert("from".into(), value.clone());
                }
                if let Some(value) = &e.to {
                    object.insert("to".into(), value.clone());
                }
                if let Some(action) = e.resolution {
                    object.insert("resolution".into(), json!(action.name()));
                }
                if let Some(to) = &e.resolution_to {
                    object.insert("resolution_to".into(), json!(to));
                }
                if e.skipped {
                    object.insert("skipped".into(), json!(true));
                }
                if e.status != "unchanged" {
                    object.insert(
                        "dependents".into(),
                        json!({
                            "nodes": e.nodes.iter().take(20).collect::<Vec<_>>(),
                            "tokens": e.tokens.iter().take(20).collect::<Vec<_>>(),
                            "count": e.nodes.len() + e.tokens.len(),
                        }),
                    );
                }
                row
            })
            .collect()
    }
}

fn source_record(snapshot: &Snapshot, token: &str, hash: &str) -> Value {
    json!({
        "library": snapshot.name,
        "version": snapshot.version,
        "token": token,
        "hash": hash,
        "package_hash": snapshot.package_hash,
    })
}

fn selected(snapshot: &Snapshot, selection: &Option<Vec<String>>) -> BTreeSet<String> {
    let Some(globs) = selection else {
        return snapshot.tokens.keys().cloned().collect();
    };
    let mut chosen: BTreeSet<String> = snapshot
        .tokens
        .keys()
        .filter(|id| globs.iter().any(|g| glob_match(g, id)))
        .cloned()
        .collect();
    // Selected aliases pull in their alias closure.
    let mut queue: Vec<String> = chosen.iter().cloned().collect();
    while let Some(id) = queue.pop() {
        if let Some(target) = snapshot.tokens[&id]
            .value
            .get("ref")
            .and_then(Value::as_str)
        {
            if chosen.insert(target.to_owned()) {
                queue.push(target.to_owned());
            }
        }
    }
    chosen
}

/// Pure function of document bytes, verified library bytes and resolutions.
pub fn plan_update(
    doc: &Value,
    snapshot: &Snapshot,
    resolutions: &[Resolution],
    options: &Options,
) -> Result<Plan> {
    let styles: Map<String, Value> = doc
        .get("styles")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let recorded = doc.pointer(&format!(
        "/design/libraries/{}",
        snapshot.name.replace('/', "~1")
    ));
    let from = recorded
        .and_then(|r| r.get("version"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let selection = options.selection.clone().or_else(|| {
        recorded
            .and_then(|r| r.get("tokens"))
            .and_then(Value::as_array)
            .map(|globs| {
                globs
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
    });
    let chosen = selected(snapshot, &selection);
    let usage = Usage::build(doc);
    let mut by_token: BTreeMap<&str, &Resolution> = BTreeMap::new();
    for resolution in resolutions {
        by_token.insert(&resolution.token, resolution);
    }

    let sourced = |record: &Value| {
        record.pointer("/source/library").and_then(Value::as_str) == Some(snapshot.name.as_str())
    };
    let mut names: BTreeSet<String> = chosen.clone();
    for (name, record) in &styles {
        if sourced(record) {
            names.insert(name.clone());
        }
    }

    let mut entries = Vec::new();
    for name in &names {
        let lib = chosen.get(name).map(|_| &snapshot.tokens[name]);
        // Sourced from this library, still in the library, but not selected: leave alone.
        if lib.is_none() && snapshot.tokens.contains_key(name) {
            continue;
        }
        let record = styles.get(name);
        let (nodes, tokens_) = usage.dependents(name);
        let mut entry = Entry {
            token: name.clone(),
            status: "unchanged",
            token_type: lib
                .map(|l| l.token_type.clone())
                .or_else(|| record.and_then(|r| r["type"].as_str()).map(str::to_owned))
                .unwrap_or_default(),
            base: record
                .and_then(|r| r.pointer("/source/hash"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            current: record.and_then(tokens::record_hash),
            incoming: lib.map(|l| l.hash.clone()),
            from: record.and_then(|r| r.get("value").cloned()),
            to: lib.map(|l| l.value.clone()),
            reason: None,
            resolution: None,
            resolution_to: None,
            nodes,
            tokens: tokens_,
            write: Write::None,
            skipped: false,
        };
        match (lib, record) {
            (Some(_), None) => {
                entry.status = "added";
                entry.write = Write::Set;
            }
            (Some(l), Some(rec)) if !sourced(rec) => {
                entry.status = "conflict";
                entry.reason = Some("collision".into());
                entry.base = None;
                entry.token_type = l.token_type.clone();
            }
            (Some(l), Some(rec)) => {
                let same_type = rec["type"].as_str() == Some(l.token_type.as_str());
                let (base, current, incoming) = (
                    entry.base.clone().unwrap_or_default(),
                    entry.current.clone().unwrap_or_default(),
                    l.hash.clone(),
                );
                if !same_type {
                    entry.status = "conflict";
                    entry.reason = Some("retargeted".into());
                } else if current == base {
                    if incoming == base {
                        entry.status = "unchanged";
                    } else {
                        entry.status = "changed";
                        entry.write = Write::Set;
                    }
                } else if incoming == base {
                    entry.status = "kept-override";
                } else if incoming == current {
                    entry.status = "converged";
                    entry.write = Write::Advance;
                } else {
                    entry.status = "conflict";
                    entry.reason = Some("local value and library value both changed".into());
                }
            }
            (None, Some(_)) => {
                entry.status = "removed";
                entry.reason = Some("token no longer exists in the library".into());
            }
            (None, None) => continue,
        }
        entries.push(entry);
    }

    // Resolutions: only tokens that need or accept one.
    for (token, resolution) in &by_token {
        let Some(entry) = entries.iter_mut().find(|e| e.token == *token) else {
            return Err(DesignError::new(
                "unknown-token",
                format!("resolution names {token}, which is not part of this update"),
            )
            .token(*token)
            .action("remove the resolution or fix the token ID"));
        };
        apply_resolution(entry, resolution, &styles, snapshot)?;
    }

    let mut resolutions_needed = Vec::new();
    let mut conflicts = Vec::new();
    for entry in &mut entries {
        if entry.status == "conflict" {
            conflicts.push(entry.token.clone());
        }
        if entry.blocking() {
            resolutions_needed.push(entry.token.clone());
            if options.continue_on_conflict {
                entry.skipped = true;
                entry.write = Write::None;
            }
        }
    }
    let writes = entries
        .iter()
        .filter(|e| e.write != Write::None)
        .map(|e| e.token.clone())
        .collect();
    Ok(Plan {
        library: snapshot.name.clone(),
        from,
        to: snapshot.version.clone(),
        package_hash: snapshot.package_hash.clone(),
        entries,
        writes,
        resolutions_needed,
        conflicts,
        selection,
        continue_on_conflict: options.continue_on_conflict,
    })
}

fn apply_resolution(
    entry: &mut Entry,
    resolution: &Resolution,
    styles: &Map<String, Value>,
    snapshot: &Snapshot,
) -> Result<()> {
    let refuse = |entry: &Entry, why: String| {
        DesignError::new("invalid-input", why)
            .token(entry.token.clone())
            .action("choose a resolution valid for this status")
    };
    match (entry.status, resolution.action) {
        ("conflict", Action::KeepLocal) => {
            entry.write = if entry.reason.as_deref() == Some("collision") {
                Write::None
            } else {
                Write::Advance
            };
        }
        ("conflict" | "kept-override", Action::TakeSource) => entry.write = Write::Set,
        ("conflict", Action::Detach) => entry.write = Write::Detach,
        ("removed", Action::KeepLocal | Action::Detach) => entry.write = Write::Detach,
        ("removed", Action::TakeSource) => {
            if !entry.nodes.is_empty() || !entry.tokens.is_empty() {
                return Err(DesignError::new(
                    "conflict",
                    format!(
                        "removed token {} is still used by {} nodes and {} tokens",
                        entry.token,
                        entry.nodes.len(),
                        entry.tokens.len()
                    ),
                )
                .token(entry.token.clone())
                .action("use map-target with a replacement token, or keep-local"));
            }
            entry.write = Write::Delete(None);
        }
        ("removed", Action::MapTarget) => {
            let to = resolution.to.clone().unwrap();
            if to == entry.token {
                return Err(refuse(
                    entry,
                    "map-target cannot map a token to itself".into(),
                ));
            }
            let exists = styles.contains_key(&to) || snapshot.tokens.contains_key(&to);
            if !exists {
                return Err(DesignError::new(
                    "unknown-token",
                    format!("map-target replacement {to} does not exist"),
                )
                .token(to)
                .action("name an existing token"));
            }
            let target_type = styles
                .get(&to)
                .and_then(|r| r["type"].as_str())
                .or_else(|| snapshot.tokens.get(&to).map(|t| t.token_type.as_str()));
            if target_type != Some(entry.token_type.as_str()) {
                return Err(DesignError::new(
                    "token-type-mismatch",
                    format!("replacement {to} is not of type {}", entry.token_type),
                )
                .token(to)
                .action("map to a token of the same type"));
            }
            entry.resolution_to = Some(to.clone());
            entry.write = Write::Delete(Some(to));
        }
        (status, action) => {
            return Err(refuse(
                entry,
                format!("{} is not valid for a {status} token", action.name()),
            ))
        }
    }
    entry.resolution = Some(resolution.action);
    Ok(())
}

/// Apply a plan's `writes` to a copy of the document. Nothing else is touched.
pub fn apply_plan(doc: &Value, snapshot: &Snapshot, plan: &Plan) -> Result<Value> {
    let mut out = doc.clone();
    let object = out
        .as_object_mut()
        .ok_or_else(|| DesignError::new("invalid-input", "document root must be an object"))?;
    if !object.get("styles").is_some_and(Value::is_object) {
        object.insert("styles".into(), json!({}));
    }
    for entry in plan.entries.iter().filter(|e| !e.skipped) {
        let name = &entry.token;
        match &entry.write {
            Write::None => {}
            Write::Set => {
                let lib = &snapshot.tokens[name];
                if lib.token_type == "color"
                    && lib.value.is_object()
                    && lib.value.get("ref").is_none()
                    && !crate::color::has_v8(doc)
                {
                    return Err(DesignError::new(
                        "unsupported-capability",
                        format!("token {name} is a native color; the document predates native color support"),
                    )
                    .token(name.clone())
                    .action("migrate the document to the color-management version first"));
                }
                let styles = out["styles"].as_object_mut().unwrap();
                let record = styles.entry(name.clone()).or_insert_with(|| json!({}));
                record["type"] = json!(lib.token_type);
                record["value"] = lib.value.clone();
                record["source"] = source_record(snapshot, name, &lib.hash);
            }
            Write::Advance => {
                let incoming = entry.incoming.clone().unwrap_or_default();
                out["styles"][name]["source"] = source_record(snapshot, name, &incoming);
            }
            Write::Detach => {
                if let Some(record) = out["styles"][name].as_object_mut() {
                    record.remove("source");
                }
            }
            Write::Delete(map_to) => {
                if let Some(to) = map_to {
                    crate::style::rewrite_refs(&mut out, name, Some(to), false);
                    let styles = out["styles"].as_object_mut().unwrap();
                    for record in styles.values_mut() {
                        if record.pointer("/value/ref").and_then(Value::as_str) == Some(name) {
                            record["value"]["ref"] = json!(to);
                        }
                    }
                }
                out["styles"].as_object_mut().unwrap().remove(name);
            }
        }
    }
    // Record the library in the document (additive `design.libraries`).
    if !out.get("design").is_some_and(Value::is_object) {
        out["design"] = json!({});
    }
    if !out["design"].get("libraries").is_some_and(Value::is_object) {
        out["design"]["libraries"] = json!({});
    }
    let mut record = json!({"version": snapshot.version, "package_hash": snapshot.package_hash});
    if let Some(globs) = &plan.selection {
        record["tokens"] = json!(globs);
    }
    out["design"]["libraries"][&snapshot.name] = record;
    let styles = out["styles"].clone();
    tokens::check_write(&styles, Some(&plan.writes))?;
    Ok(out)
}

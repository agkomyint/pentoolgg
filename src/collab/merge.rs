//! Deterministic replay and merge rules (spec sections 4 and 5).
//!
//! Replay is a pure function of the set of accepted operations: operations
//! apply in the total order `(wall_ms, counter, actor, op_id)`, except that an
//! operation never applies before its `deps`. Every rule below is evaluated on
//! the replayed state in that fixed order, so every peer derives the same
//! document bytes and the same conflict set from the same operations.
use super::op::{
    canonical_json, check_id, draft, object_ref, parent_ref, sha256_hex, Draft, Level, Op,
};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Group depth limit shared with `pen-format-v4.md`.
pub const MAX_NODE_DEPTH: usize = 64;
const MAX_EDITS_PER_OBJECT: usize = 4096;
const MAX_FLAT_OBJECTS: usize = 2_000_000;
const KEY_DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

type OrdKey = (u64, u32, String, String);

// ---------------------------------------------------------------- keys

fn digit_index(c: u8) -> usize {
    KEY_DIGITS.iter().position(|d| *d == c).unwrap_or(0)
}

/// Midpoint of two base-62 fractions (`b = None` is 1). Neither may end in `0`.
fn midpoint(a: &str, b: Option<&str>) -> String {
    let (a, b) = (a.as_bytes(), b.map(str::as_bytes));
    mid_bytes(a, b)
}

fn mid_bytes(a: &[u8], b: Option<&[u8]>) -> String {
    if let Some(b) = b {
        let mut n = 0;
        while n < b.len() && *a.get(n).unwrap_or(&b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            let rest_a = if n <= a.len() { &a[n..] } else { &[][..] };
            let mut out = String::from_utf8_lossy(&b[..n]).into_owned();
            out.push_str(&mid_bytes(rest_a, Some(&b[n..])));
            return out;
        }
    }
    let digit_a = a.first().map_or(0, |c| digit_index(*c));
    let digit_b = b.map_or(KEY_DIGITS.len(), |b| digit_index(b[0]));
    if digit_b - digit_a > 1 {
        let mid = (digit_a + digit_b).div_ceil(2);
        return (KEY_DIGITS[mid] as char).to_string();
    }
    if let Some(b) = b {
        if b.len() > 1 {
            return (b[0] as char).to_string();
        }
    }
    let mut out = (KEY_DIGITS[digit_a] as char).to_string();
    out.push_str(&mid_bytes(if a.is_empty() { a } else { &a[1..] }, None));
    out
}

/// A key strictly between `a` and `b` (either side open). Falls back to
/// extending `a` if the bounds are not strictly ordered.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> String {
    match (a, b) {
        (Some(a), Some(b)) if a >= b => format!("{a}V"),
        _ => midpoint(a.unwrap_or(""), b),
    }
}

/// Evenly spaced initial keys for `n` siblings, ascending and never ending in `0`.
pub fn initial_keys(n: usize) -> Vec<String> {
    let mut width = 1;
    let mut capacity = 62usize;
    while capacity < 2 * n + 2 {
        width += 1;
        capacity = capacity.saturating_mul(62);
    }
    (0..n)
        .map(|i| {
            let mut value = 2 * i + 1;
            let mut digits = vec![b'0'; width];
            for slot in digits.iter_mut().rev() {
                *slot = KEY_DIGITS[value % 62];
                value /= 62;
            }
            String::from_utf8(digits).expect("ascii digits")
        })
        .collect()
}

pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= super::op::MAX_KEY_BYTES
        && !key.ends_with('0')
        && key.bytes().all(|c| KEY_DIGITS.contains(&c))
}

// ---------------------------------------------------------- conflicts

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub schema: u64,
    pub id: String,
    pub code: String,
    pub page: String,
    pub object: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winning_op: Option<String>,
    pub losing_op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winning_actor: Option<String>,
    pub losing_actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winning_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub losing_value: Option<Value>,
    pub status: String,
}

/// Why an operation did not take effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    pub code: &'static str,
    pub message: String,
}

fn reject<T>(code: &'static str, message: impl Into<String>) -> Result<T, Reject> {
    Err(Reject {
        code,
        message: message.into(),
    })
}

// ------------------------------------------------------------- objects

#[derive(Debug, Clone)]
struct Writer {
    op: String,
    actor: String,
    value: Option<Value>,
}

#[derive(Debug, Clone)]
struct Edit {
    op: String,
    actor: String,
    path: String,
    value: Option<Value>,
}

#[derive(Debug, Clone)]
struct Obj {
    level: Level,
    page: String,
    id: String,
    parent: Option<String>,
    key: String,
    born: OrdKey,
    props: Map<String, Value>,
    children: Vec<String>,
    deleted_by: Option<String>,
    create_op: String,
    writers: BTreeMap<String, Writer>,
    edits: Vec<Edit>,
    mover: Option<Writer>,
}

#[derive(Debug, Clone)]
struct OpMeta {
    deps: Vec<String>,
    index: u64,
}

/// Replayed state: an arena of objects addressed by `level:page:id`.
#[derive(Debug, Clone)]
pub struct Replayer {
    objs: BTreeMap<String, Obj>,
    metas: HashMap<String, OpMeta>,
    actors: HashMap<String, String>,
    conflicts: BTreeMap<String, Conflict>,
    rejected: BTreeMap<String, (String, String)>,
    applied: Vec<String>,
    max_stamp: Option<(u64, u32)>,
    next_index: u64,
}

const DOC_REF: &str = "d::$doc";

fn ref_of(level: Level, page: &str, id: &str) -> String {
    match level {
        Level::Doc => DOC_REF.into(),
        Level::Page => object_ref(level, "", id),
        _ => object_ref(level, page, id),
    }
}

fn child_level(level: Level) -> Option<Level> {
    match level {
        Level::Doc => Some(Level::Page),
        Level::Page => Some(Level::Layer),
        Level::Layer | Level::Node => Some(Level::Node),
    }
}

// ---------------------------------------------------- flat document view

struct FlatObj<'a> {
    level: Level,
    page: String,
    id: String,
    parent: Option<String>,
    value: &'a Value,
    props: Map<String, Value>,
}

struct Flat<'a> {
    order: Vec<String>,
    objs: HashMap<String, FlatObj<'a>>,
    children: HashMap<String, Vec<String>>,
}

fn flatten_subtree<'a>(
    value: &'a Value,
    level: Level,
    page: &str,
    id: &str,
    parent: Option<String>,
) -> Result<Flat<'a>> {
    let mut flat = Flat {
        order: Vec::new(),
        objs: HashMap::new(),
        children: HashMap::new(),
    };
    walk(value, level, page, id, parent, 0, &mut flat)?;
    Ok(flat)
}

fn walk<'a>(
    value: &'a Value,
    level: Level,
    page: &str,
    id: &str,
    parent: Option<String>,
    depth: usize,
    flat: &mut Flat<'a>,
) -> Result<()> {
    if depth > MAX_NODE_DEPTH + 8 {
        bail!("[limit-exceeded] document nesting exceeds depth {MAX_NODE_DEPTH}")
    }
    if flat.objs.len() >= MAX_FLAT_OBJECTS {
        bail!("[limit-exceeded] document has more than {MAX_FLAT_OBJECTS} objects")
    }
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("[invalid-input] {} {id:?} is not an object", level.code()))?;
    let rf = ref_of(level, page, id);
    let container = level.container();
    let mut props = object.clone();
    let mut kids: Vec<&'a Value> = Vec::new();
    if let Some(items) = props.remove(container) {
        props.insert("$c".into(), Value::Bool(true));
        let _ = items;
    }
    if let Some(items) = object.get(container) {
        let items = items
            .as_array()
            .ok_or_else(|| anyhow!("[invalid-input] {id:?} `{container}` is not an array"))?;
        kids.extend(items.iter());
    }
    if flat.objs.contains_key(&rf) {
        bail!(
            "[duplicate-id] {} {id:?} appears twice on page {page:?}",
            level.code()
        )
    }
    flat.order.push(rf.clone());
    flat.objs.insert(
        rf.clone(),
        FlatObj {
            level,
            page: page.into(),
            id: id.into(),
            parent,
            value,
            props,
        },
    );
    let Some(next) = child_level(level) else {
        return Ok(());
    };
    let mut list = Vec::new();
    for kid in kids {
        let kid_id = kid
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("[invalid-input] a child of {id:?} has no string id"))?;
        check_id("object id", kid_id)?;
        let kid_page = if level == Level::Doc { kid_id } else { page };
        walk(
            kid,
            next,
            kid_page,
            kid_id,
            Some(rf.clone()),
            depth + 1,
            flat,
        )?;
        list.push(ref_of(next, kid_page, kid_id));
    }
    flat.children.insert(rf, list);
    Ok(())
}

fn flatten(doc: &Value) -> Result<Flat<'_>> {
    flatten_subtree(doc, Level::Doc, "", "$doc", None)
}

// -------------------------------------------------------- property paths

fn escape_segment(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn split_path(path: &str) -> Result<Vec<String>, Reject> {
    let segments: Vec<String> = path
        .split('/')
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect();
    if path.is_empty()
        || segments.len() > super::op::MAX_PATH_SEGMENTS
        || segments.iter().any(|s| s.is_empty() || s.len() > 128)
    {
        return reject(
            "invalid-op",
            format!(
                "property path {path:?} must be 1 to 16 non-empty segments of at most 128 bytes"
            ),
        );
    }
    Ok(segments)
}

fn set_path(props: &mut Map<String, Value>, segments: &[String], value: Option<Value>) {
    let (last, parents) = segments.split_last().expect("path has a segment");
    let mut cursor = props;
    for segment in parents {
        if value.is_none() && !cursor.get(segment).is_some_and(Value::is_object) {
            return;
        }
        let entry = cursor
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        cursor = entry.as_object_mut().expect("object");
    }
    match value {
        Some(value) => {
            cursor.insert(last.clone(), value);
        }
        None => {
            cursor.remove(last);
        }
    }
}

fn diff_props(
    a: &Map<String, Value>,
    b: &Map<String, Value>,
    prefix: &str,
    out: &mut Vec<(String, Option<Value>)>,
) {
    let mut keys: BTreeSet<&String> = a.keys().collect();
    keys.extend(b.keys());
    for key in keys {
        let path = if prefix.is_empty() {
            escape_segment(key)
        } else {
            format!("{prefix}/{}", escape_segment(key))
        };
        match (a.get(key), b.get(key)) {
            (Some(Value::Object(x)), Some(Value::Object(y))) => diff_props(x, y, &path, out),
            (Some(x), Some(y)) if x == y => {}
            (_, Some(y)) => out.push((path, Some(y.clone()))),
            (Some(_), None) => out.push((path, None)),
            (None, None) => {}
        }
    }
}

// ----------------------------------------------------------- replayer

impl Replayer {
    /// Seed the state from the genesis document (a valid v4 scene).
    pub fn new(genesis: &Value) -> Result<Self> {
        let flat = flatten(genesis)?;
        let mut state = Replayer {
            objs: BTreeMap::new(),
            metas: HashMap::new(),
            actors: HashMap::new(),
            conflicts: BTreeMap::new(),
            rejected: BTreeMap::new(),
            applied: Vec::new(),
            max_stamp: None,
            next_index: 1,
        };
        state.insert_flat(&flat, "", &(0, 0, String::new(), String::new()), "genesis");
        Ok(state)
    }

    /// Insert every object of `flat`. Siblings take evenly spaced initial keys;
    /// the root takes `root_key`. The caller has already checked for collisions.
    fn insert_flat(&mut self, flat: &Flat<'_>, root_key: &str, born: &OrdKey, create_op: &str) {
        let mut keys: HashMap<&str, String> = HashMap::new();
        for kids in flat.children.values() {
            for (kid, key) in kids.iter().zip(initial_keys(kids.len())) {
                keys.insert(kid.as_str(), key);
            }
        }
        for rf in &flat.order {
            let source = &flat.objs[rf];
            self.objs.insert(
                rf.clone(),
                Obj {
                    level: source.level,
                    page: source.page.clone(),
                    id: source.id.clone(),
                    parent: source.parent.clone(),
                    key: keys
                        .get(rf.as_str())
                        .cloned()
                        .unwrap_or_else(|| root_key.into()),
                    born: born.clone(),
                    props: source.props.clone(),
                    children: flat.children.get(rf).cloned().unwrap_or_default(),
                    deleted_by: None,
                    create_op: create_op.into(),
                    writers: BTreeMap::new(),
                    edits: Vec::new(),
                    mover: None,
                },
            );
        }
    }

    pub fn conflicts(&self) -> Vec<Conflict> {
        self.conflicts.values().cloned().collect()
    }

    pub fn rejected(&self) -> Vec<(String, String, String)> {
        self.rejected
            .iter()
            .map(|(id, (code, message))| (id.clone(), code.clone(), message.clone()))
            .collect()
    }

    pub fn applied(&self) -> &[String] {
        &self.applied
    }

    pub fn max_stamp(&self) -> Option<(u64, u32)> {
        self.max_stamp
    }

    /// Latest applied op id, used as the causal predecessor of the next local op.
    pub fn last_applied(&self) -> Option<&str> {
        self.applied.last().map(String::as_str)
    }

    // ------------------------------------------------------ causality

    fn is_ancestor(&self, ancestor: &str, of: &str) -> bool {
        let ancestor = ancestor.split('#').next().unwrap_or(ancestor);
        let Some(target) = self.metas.get(ancestor) else {
            return false;
        };
        let mut stack = vec![of.to_string()];
        let mut seen = HashSet::new();
        while let Some(current) = stack.pop() {
            if current == ancestor {
                return true;
            }
            if !seen.insert(current.clone()) {
                continue;
            }
            if let Some(meta) = self.metas.get(&current) {
                if meta.index < target.index {
                    continue;
                }
                stack.extend(meta.deps.iter().cloned());
            }
        }
        false
    }

    // -------------------------------------------------------- conflicts

    #[allow(clippy::too_many_arguments)]
    fn record_conflict(
        &mut self,
        code: &str,
        _op: &Op,
        page: &str,
        object: &str,
        property: Option<&str>,
        winner: Option<(&str, &str, Option<&Value>)>,
        loser: (&str, &str, Option<&Value>),
    ) {
        let digest = sha256_hex(
            format!(
                "{code}|{}|{object}|{}|{page}",
                loser.0,
                property.unwrap_or("")
            )
            .as_bytes(),
        );
        let id = format!("cf_{}", &digest[..32]);
        self.conflicts.insert(
            id.clone(),
            Conflict {
                schema: 1,
                id,
                code: code.into(),
                page: page.into(),
                object: object.into(),
                property: property.map(str::to_string),
                winning_op: winner.map(|w| w.0.to_string()),
                losing_op: loser.0.to_string(),
                winning_actor: winner.map(|w| w.1.to_string()),
                losing_actor: loser.1.to_string(),
                winning_value: winner.and_then(|w| w.2.cloned()),
                losing_value: loser.2.cloned(),
                status: "open".into(),
            },
        );
    }

    // ------------------------------------------------------------ guards

    fn layer_of(&self, rf: &str) -> Option<&Obj> {
        let mut cursor = self.objs.get(rf)?;
        loop {
            if cursor.level == Level::Layer {
                return Some(cursor);
            }
            cursor = self.objs.get(cursor.parent.as_ref()?)?;
        }
    }

    /// Lock and attached-instance protection, enforced at apply time (spec 4.6).
    fn guard(&self, rf: &str, op: &Op, check_lock: bool) -> Result<(), Reject> {
        let Some(layer) = self.layer_of(rf) else {
            return Ok(());
        };
        if check_lock && layer.props.get("locked").and_then(Value::as_bool) == Some(true) {
            return reject(
                "locked",
                format!(
                    "{} on layer {} (page {}) is locked; unlock it or choose another layer",
                    op.kind, layer.id, layer.page
                ),
            );
        }
        if !op.via().starts_with("instance-") {
            if let Some(doc) = self.objs.get(DOC_REF) {
                for instance in doc
                    .props
                    .get("instances")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let page = instance
                        .get("page_id")
                        .and_then(Value::as_str)
                        .unwrap_or("page-1");
                    let attached = instance
                        .get("layer_ids")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .any(|id| id == layer.id);
                    if page == layer.page && attached {
                        return reject(
                            "locked",
                            format!(
                                "layer {} belongs to attached instance {}; use the instance operations",
                                layer.id,
                                instance.get("id").and_then(Value::as_str).unwrap_or("unknown")
                            ),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn node_depth(&self, rf: &str) -> usize {
        let mut depth = 0;
        let mut cursor = self.objs.get(rf);
        while let Some(obj) = cursor {
            if obj.level == Level::Node {
                depth += 1;
            }
            cursor = obj.parent.as_ref().and_then(|p| self.objs.get(p));
        }
        depth
    }

    fn subtree_height(&self, rf: &str) -> usize {
        let Some(obj) = self.objs.get(rf) else {
            return 0;
        };
        let own = usize::from(obj.level == Level::Node);
        own + obj
            .children
            .iter()
            .map(|child| self.subtree_height(child))
            .max()
            .unwrap_or(0)
    }

    fn is_descendant_or_self(&self, candidate: &str, ancestor: &str) -> bool {
        let mut cursor = Some(candidate.to_string());
        let mut steps = 0;
        while let Some(current) = cursor {
            if current == ancestor {
                return true;
            }
            steps += 1;
            if steps > self.objs.len() + 1 {
                return true;
            }
            cursor = self.objs.get(&current).and_then(|o| o.parent.clone());
        }
        false
    }

    // ------------------------------------------------------------- apply

    /// Apply one operation to the state. A rejected operation changes nothing
    /// but is remembered; its conflict (if the rule names one) is recorded.
    pub fn apply_op(&mut self, op: &Op) -> Result<(), Reject> {
        if self.metas.contains_key(&op.id) {
            return Ok(());
        }
        let index = self.next_index;
        self.next_index += 1;
        self.metas.insert(
            op.id.clone(),
            OpMeta {
                deps: op.deps.clone(),
                index,
            },
        );
        self.actors.insert(op.id.clone(), op.actor.clone());
        let result = match op.kind.as_str() {
            "set-property" => self.apply_set(op),
            "create" => self.apply_create(op),
            "delete" => self.apply_delete(op),
            "move" => self.apply_move(op),
            "scene" => self.apply_scene(op),
            other => reject(
                "unsupported-op",
                format!("unknown operation kind {other:?}"),
            ),
        };
        match &result {
            Ok(()) => {
                self.applied.push(op.id.clone());
                let stamp = (op.hlc.wall_ms, op.hlc.counter);
                self.max_stamp = Some(self.max_stamp.map_or(stamp, |m| m.max(stamp)));
            }
            Err(error) => {
                self.rejected
                    .insert(op.id.clone(), (error.code.into(), error.message.clone()));
            }
        }
        result
    }

    fn target<'a>(&self, op: &'a Op) -> Result<&'a str, Reject> {
        match op.targets.as_slice() {
            [one] => Ok(one),
            _ => reject(
                "invalid-op",
                format!(
                    "{} requires exactly one target, got {}",
                    op.kind,
                    op.targets.len()
                ),
            ),
        }
    }

    fn payload_level(op: &Op) -> Result<Level, Reject> {
        op.payload
            .get("level")
            .and_then(Value::as_str)
            .ok_or("missing level")
            .and_then(|code| Level::parse(code).map_err(|_| "unknown level"))
            .or_else(|message| reject("invalid-op", format!("{} payload: {message}", op.kind)))
    }

    fn parent_target(&self, op: &Op, level: Level) -> Result<String, Reject> {
        let parent = op.payload.get("parent").and_then(Value::as_object);
        let (Some(parent_level), Some(parent_id)) = (
            parent
                .and_then(|p| p.get("level"))
                .and_then(Value::as_str)
                .and_then(|c| Level::parse(c).ok()),
            parent.and_then(|p| p.get("id")).and_then(Value::as_str),
        ) else {
            return reject(
                "invalid-op",
                format!("{} payload needs a parent reference", op.kind),
            );
        };
        let valid = match level {
            Level::Doc => false,
            Level::Page => parent_level == Level::Doc,
            Level::Layer => parent_level == Level::Page,
            Level::Node => matches!(parent_level, Level::Layer | Level::Node),
        };
        if !valid {
            return reject(
                "invalid-op",
                format!(
                    "a {} cannot live under a {}",
                    level.code(),
                    parent_level.code()
                ),
            );
        }
        let parent_page = match parent_level {
            Level::Doc | Level::Page => "",
            _ => op.page.as_str(),
        };
        Ok(ref_of(parent_level, parent_page, parent_id))
    }

    fn deleted_target(
        &mut self,
        op: &Op,
        rf: &str,
        property: Option<&str>,
        value: Option<&Value>,
    ) -> Result<(), Reject> {
        let obj = &self.objs[rf];
        let Some(deleter) = obj.deleted_by.clone() else {
            return Ok(());
        };
        let (page, id) = (obj.page.clone(), obj.id.clone());
        if self.is_ancestor(&deleter, &op.id) {
            return reject(
                "unknown-target",
                format!(
                    "{} targets {id} on page {page}, which was deleted by {deleter}",
                    op.kind
                ),
            );
        }
        let deleter_actor = self.actor_of(&deleter);
        self.record_conflict(
            "edit-on-deleted",
            op,
            &page,
            &id,
            property,
            Some((&deleter, &deleter_actor, None)),
            (&op.id, &op.actor, value),
        );
        reject(
            "edit-on-deleted",
            format!(
                "{} on {id} (page {page}) lost to concurrent delete {deleter}",
                op.kind
            ),
        )
    }

    fn actor_of(&self, op_id: &str) -> String {
        self.actors
            .get(op_id.split('#').next().unwrap_or(op_id))
            .cloned()
            .unwrap_or_default()
    }

    fn apply_set(&mut self, op: &Op) -> Result<(), Reject> {
        let id = self.target(op)?;
        let level = Self::payload_level(op)?;
        let path = op
            .payload
            .get("path")
            .and_then(Value::as_str)
            .ok_or(())
            .or_else(|_| reject("invalid-op", "set-property needs a string path"))?;
        let segments = split_path(path)?;
        let value = if op.payload.get("remove").and_then(Value::as_bool) == Some(true) {
            None
        } else {
            match op.payload.get("value") {
                Some(value) => Some(value.clone()),
                None => return reject("invalid-op", "set-property needs a value or remove:true"),
            }
        };
        let rf = ref_of(level, &op.page, id);
        if !self.objs.contains_key(&rf) {
            return reject(
                "unknown-target",
                format!(
                    "set-property targets {id} ({}) on page {:?}, which does not exist",
                    level.code(),
                    op.page
                ),
            );
        }
        let first = segments[0].as_str();
        if first == "id" || first == level.container() {
            return reject("invalid-op", format!("property {first} cannot be set; ids are immutable and children use create/move/delete"));
        }
        if level == Level::Doc && matches!(first, "version" | "format") {
            return reject(
                "migration-blocked",
                "format migration is not a collaborative operation; run it before or after the session",
            );
        }
        if first == "$c"
            && (segments.len() != 1 || value.as_ref().is_some_and(|v| v != &Value::Bool(true)))
        {
            return reject("invalid-op", "$c may only be set to true or removed");
        }
        self.deleted_target(op, &rf, Some(path), value.as_ref())?;
        self.guard(&rf, op, false)?;
        let obj = self.objs.get_mut(&rf).expect("checked");
        let previous = obj.writers.get(path).cloned();
        set_path(&mut obj.props, &segments, value.clone());
        obj.writers.insert(
            path.to_string(),
            Writer {
                op: op.id.clone(),
                actor: op.actor.clone(),
                value: value.clone(),
            },
        );
        if obj.edits.len() < MAX_EDITS_PER_OBJECT {
            obj.edits.push(Edit {
                op: op.id.clone(),
                actor: op.actor.clone(),
                path: path.to_string(),
                value: value.clone(),
            });
        }
        let page = op.page.clone();
        if let Some(previous) = previous {
            if previous.op != op.id && !self.is_ancestor(&previous.op, &op.id) {
                self.record_conflict(
                    "overwritten-edit",
                    op,
                    &page,
                    id,
                    Some(path),
                    Some((&op.id, &op.actor, value.as_ref())),
                    (&previous.op, &previous.actor, previous.value.as_ref()),
                );
            }
        }
        Ok(())
    }

    fn apply_create(&mut self, op: &Op) -> Result<(), Reject> {
        let id = self.target(op)?;
        let level = Self::payload_level(op)?;
        let key = op
            .payload
            .get("key")
            .and_then(Value::as_str)
            .filter(|k| valid_key(k))
            .ok_or(())
            .or_else(|_| reject("invalid-op", "create needs a valid position key"))?
            .to_string();
        let node = op
            .payload
            .get("node")
            .ok_or(())
            .or_else(|_| reject("invalid-op", "create needs a node"))?;
        if node.get("id").and_then(Value::as_str) != Some(id) {
            return reject(
                "invalid-op",
                format!("create node id must equal target {id}"),
            );
        }
        let parent = self.parent_target(op, level)?;
        let page = if level == Level::Page {
            id
        } else {
            op.page.as_str()
        };
        let flat =
            flatten_subtree(node, level, page, id, Some(parent.clone())).map_err(|error| {
                Reject {
                    code: "invalid-op",
                    message: error.to_string(),
                }
            })?;
        let rf = ref_of(level, page, id);
        let Some(parent_obj) = self.objs.get(&parent) else {
            return reject(
                "unknown-target",
                format!("create of {id} names a parent that does not exist"),
            );
        };
        if let Some(deleter) = parent_obj.deleted_by.clone() {
            let parent_id = parent_obj.id.clone();
            let deleter_actor = self.actor_of(&deleter);
            self.record_conflict(
                "parent-deleted",
                op,
                &op.page,
                id,
                None,
                Some((&deleter, &deleter_actor, None)),
                (&op.id, &op.actor, Some(node)),
            );
            return reject(
                "parent-deleted",
                format!("create of {id} was rejected: parent {parent_id} is deleted"),
            );
        }
        if level == Level::Node {
            self.guard(&parent, op, true)?;
        }
        // Existing ids: a tombstone may be reused only by an operation that
        // causally follows its delete; anything else is a duplicate-id conflict.
        let mut reusable = Vec::new();
        for new_ref in &flat.order {
            if let Some(existing) = self.objs.get(new_ref) {
                let reusable_here = existing
                    .deleted_by
                    .as_ref()
                    .is_some_and(|deleter| self.is_ancestor(deleter, &op.id));
                if !reusable_here {
                    let (winner, winner_actor) = (
                        existing.create_op.clone(),
                        self.actor_of(&existing.create_op),
                    );
                    let existing_id = existing.id.clone();
                    self.record_conflict(
                        "duplicate-id",
                        op,
                        &op.page,
                        &existing_id,
                        None,
                        Some((&winner, &winner_actor, None)),
                        (&op.id, &op.actor, Some(node)),
                    );
                    return reject(
                        "duplicate-id",
                        format!("create of {id} was rejected: {existing_id} already exists on page {:?}", op.page),
                    );
                }
                reusable.push(new_ref.clone());
            }
        }
        // Depth: nodes below the parent plus the new subtree.
        let base_depth = self.node_depth(&parent);
        let mut depths: HashMap<&str, usize> = HashMap::new();
        for new_ref in &flat.order {
            let source = &flat.objs[new_ref];
            let parent_depth = match &source.parent {
                Some(p) if flat.objs.contains_key(p) => depths[p.as_str()],
                _ => base_depth,
            };
            let depth = parent_depth + usize::from(source.level == Level::Node);
            if depth > MAX_NODE_DEPTH {
                let message = format!("create of {id} would exceed group depth {MAX_NODE_DEPTH}");
                self.record_conflict(
                    "limit-exceeded",
                    op,
                    &op.page,
                    id,
                    None,
                    None,
                    (&op.id, &op.actor, None),
                );
                return reject("limit-exceeded", message);
            }
            depths.insert(new_ref.as_str(), depth);
        }
        for old in reusable {
            self.unlink(&old);
            self.objs.remove(&old);
        }
        let born = op.order_key();
        self.insert_flat(&flat, &key, &born, &op.id);
        self.objs
            .get_mut(&parent)
            .expect("parent checked")
            .children
            .push(rf);
        Ok(())
    }

    fn unlink(&mut self, rf: &str) {
        let parent = self.objs.get(rf).and_then(|o| o.parent.clone());
        if let Some(parent) = parent.and_then(|p| self.objs.get_mut(&p)) {
            parent.children.retain(|child| child != rf);
        }
    }

    fn apply_delete(&mut self, op: &Op) -> Result<(), Reject> {
        let id = self.target(op)?;
        let level = Self::payload_level(op)?;
        if level == Level::Doc {
            return reject("invalid-op", "the document root cannot be deleted");
        }
        let rf = ref_of(level, &op.page, id);
        let Some(obj) = self.objs.get(&rf) else {
            return reject(
                "unknown-target",
                format!(
                    "delete targets {id} on page {:?}, which does not exist",
                    op.page
                ),
            );
        };
        if obj.deleted_by.is_some() {
            return Ok(());
        }
        if level == Level::Node {
            self.guard(&rf, op, true)?;
        } else {
            self.guard(&rf, op, false)?;
        }
        let mut stack = vec![rf.clone()];
        let mut doomed = Vec::new();
        while let Some(current) = stack.pop() {
            if let Some(obj) = self.objs.get(&current) {
                stack.extend(obj.children.iter().cloned());
                doomed.push(current);
            }
        }
        doomed.sort();
        self.unlink(&rf);
        for current in doomed {
            let edits = {
                let obj = self.objs.get_mut(&current).expect("collected");
                obj.deleted_by = Some(op.id.clone());
                std::mem::take(&mut obj.edits)
            };
            let (page, object) = {
                let obj = &self.objs[&current];
                (obj.page.clone(), obj.id.clone())
            };
            for edit in edits {
                if !self.is_ancestor(&edit.op, &op.id) {
                    self.record_conflict(
                        "edit-on-deleted",
                        op,
                        &page,
                        &object,
                        Some(&edit.path),
                        Some((&op.id, &op.actor, None)),
                        (&edit.op, &edit.actor, edit.value.as_ref()),
                    );
                }
            }
        }
        Ok(())
    }

    fn apply_move(&mut self, op: &Op) -> Result<(), Reject> {
        let id = self.target(op)?;
        let level = Self::payload_level(op)?;
        let key = op
            .payload
            .get("key")
            .and_then(Value::as_str)
            .filter(|k| valid_key(k))
            .ok_or(())
            .or_else(|_| reject("invalid-op", "move needs a valid position key"))?
            .to_string();
        let parent = self.parent_target(op, level)?;
        let rf = ref_of(level, &op.page, id);
        if !self.objs.contains_key(&rf) {
            return reject(
                "unknown-target",
                format!(
                    "move targets {id} on page {:?}, which does not exist",
                    op.page
                ),
            );
        }
        self.deleted_target(op, &rf, Some("$position"), None)?;
        let Some(parent_obj) = self.objs.get(&parent) else {
            return reject(
                "unknown-target",
                format!("move of {id} names a missing parent"),
            );
        };
        if let Some(deleter) = parent_obj.deleted_by.clone() {
            let parent_id = parent_obj.id.clone();
            let deleter_actor = self.actor_of(&deleter);
            self.record_conflict(
                "parent-deleted",
                op,
                &op.page,
                id,
                None,
                Some((&deleter, &deleter_actor, None)),
                (&op.id, &op.actor, None),
            );
            return reject(
                "parent-deleted",
                format!("move of {id} was rejected: parent {parent_id} is deleted"),
            );
        }
        if self.is_descendant_or_self(&parent, &rf)
            || self.node_depth(&parent) + self.subtree_height(&rf) > MAX_NODE_DEPTH
        {
            let parent_id = self.objs[&parent].id.clone();
            let winner = self.objs[&rf].mover.clone();
            self.record_conflict(
                "reparent-cycle",
                op,
                &op.page,
                id,
                None,
                winner
                    .as_ref()
                    .map(|w| (w.op.as_str(), w.actor.as_str(), None)),
                (&op.id, &op.actor, None),
            );
            return reject(
                "reparent-cycle",
                format!("move of {id} under {parent_id} would create a cycle or exceed depth {MAX_NODE_DEPTH}"),
            );
        }
        if level == Level::Node {
            self.guard(&rf, op, true)?;
            self.guard(&parent, op, true)?;
        }
        self.unlink(&rf);
        let previous = self.objs[&rf].mover.clone();
        {
            let obj = self.objs.get_mut(&rf).expect("checked");
            obj.parent = Some(parent.clone());
            obj.key = key;
            obj.mover = Some(Writer {
                op: op.id.clone(),
                actor: op.actor.clone(),
                value: None,
            });
        }
        self.objs
            .get_mut(&parent)
            .expect("checked")
            .children
            .push(rf);
        if let Some(previous) = previous {
            if previous.op != op.id && !self.is_ancestor(&previous.op, &op.id) {
                self.record_conflict(
                    "overwritten-edit",
                    op,
                    &op.page,
                    id,
                    Some("$position"),
                    Some((&op.id, &op.actor, None)),
                    (&previous.op, &previous.actor, None),
                );
            }
        }
        Ok(())
    }

    /// A `scene` operation carries the existing scene batch vocabulary. It is
    /// run through `scene::apply_batch_at` on the replayed document and its
    /// effect is expanded into native operations at this position in the order.
    fn apply_scene(&mut self, op: &Op) -> Result<(), Reject> {
        let operations = op
            .payload
            .get("operations")
            .and_then(Value::as_array)
            .ok_or(())
            .or_else(|_| reject("invalid-op", "scene needs an operations array"))?;
        let page = op.payload.get("page").and_then(Value::as_str);
        let before = self.materialize();
        let mut after = before.clone();
        crate::scene::apply_batch_at(&mut after, page, operations, std::path::Path::new("."))
            .map_err(|error| Reject {
                code: "invalid-op",
                message: format!("scene batch failed: {error}"),
            })?;
        let drafts = self.derive(&after).map_err(|error| Reject {
            code: "invalid-op",
            message: error.to_string(),
        })?;
        let backup = self.clone();
        for (index, draft) in drafts.into_iter().enumerate() {
            let mut virtual_op = op.clone();
            virtual_op.id = format!("{}#{index}", op.id);
            virtual_op.page = draft.page;
            virtual_op.kind = draft.kind.into();
            virtual_op.targets = draft.targets;
            virtual_op.payload = draft.payload;
            virtual_op.deps = Vec::new();
            if let Err(error) = self.apply_virtual(&virtual_op, op) {
                *self = backup;
                return Err(error);
            }
        }
        Ok(())
    }

    fn apply_virtual(&mut self, virtual_op: &Op, parent: &Op) -> Result<(), Reject> {
        let index = self.next_index;
        self.next_index += 1;
        self.metas.insert(
            virtual_op.id.clone(),
            OpMeta {
                deps: vec![parent.id.clone()],
                index,
            },
        );
        self.actors
            .insert(virtual_op.id.clone(), parent.actor.clone());
        match virtual_op.kind.as_str() {
            "set-property" => self.apply_set(virtual_op),
            "create" => self.apply_create(virtual_op),
            "delete" => self.apply_delete(virtual_op),
            _ => self.apply_move(virtual_op),
        }
    }

    // --------------------------------------------------------- materialize

    /// Canonical document value for the current state.
    pub fn materialize(&self) -> Value {
        self.build(DOC_REF)
    }

    fn build(&self, rf: &str) -> Value {
        let obj = &self.objs[rf];
        let mut map = obj.props.clone();
        let marker = map.remove("$c").is_some();
        let mut kids: Vec<&Obj> = obj
            .children
            .iter()
            .filter_map(|child| self.objs.get(child))
            .collect();
        kids.sort_by(|a, b| (&a.key, &a.born, &a.id).cmp(&(&b.key, &b.born, &b.id)));
        if marker || !kids.is_empty() {
            let items: Vec<Value> = kids
                .into_iter()
                .map(|kid| self.build(&ref_of(kid.level, &kid.page, &kid.id)))
                .collect();
            map.insert(obj.level.container().into(), Value::Array(items));
        }
        Value::Object(map)
    }

    // ------------------------------------------------------------- derive

    /// Plan the native operations that turn the current state into `new`.
    /// Ordering is fixed so replay never sees a lock, parent or cycle problem
    /// that the target document does not have.
    pub fn derive(&self, new: &Value) -> Result<Vec<Draft>> {
        let current = self.materialize();
        let a = flatten(&current)?;
        let b = flatten(new)?;
        let mut unlocks = Vec::new();
        let mut creates = Vec::new();
        let mut sets = Vec::new();
        let mut moves = Vec::new();
        let mut deletes = Vec::new();
        let mut locks = Vec::new();

        for rf in &b.order {
            let target = &b.objs[rf];
            let Some(old) = a.objs.get(rf) else { continue };
            let mut changes = Vec::new();
            diff_props(&old.props, &target.props, "", &mut changes);
            for (path, value) in changes {
                let mut payload = json!({"level": target.level.code(), "path": path});
                match &value {
                    Some(value) => payload["value"] = value.clone(),
                    None => payload["remove"] = Value::Bool(true),
                }
                let page = if target.level == Level::Doc {
                    ""
                } else {
                    target.page.as_str()
                };
                let item = draft("set-property", page, &target.id, payload);
                if target.level == Level::Layer && path == "locked" {
                    if value == Some(Value::Bool(true)) {
                        locks.push(item);
                    } else {
                        unlocks.push(item);
                    }
                } else {
                    sets.push(item);
                }
            }
        }

        for parent in &b.order {
            let Some(kids) = b.children.get(parent) else {
                continue;
            };
            let parent_in_a = a.objs.contains_key(parent);
            let old_kids = a.children.get(parent);
            let position: HashMap<&str, usize> = old_kids
                .map(|list| {
                    list.iter()
                        .enumerate()
                        .map(|(i, k)| (k.as_str(), i))
                        .collect()
                })
                .unwrap_or_default();
            // Existing children that kept their parent: the longest increasing
            // run of old positions stays put, the rest are moves.
            let kept: Vec<(usize, usize)> = kids
                .iter()
                .enumerate()
                .filter_map(|(i, k)| position.get(k.as_str()).map(|p| (i, *p)))
                .collect();
            let stay = longest_increasing(&kept.iter().map(|(_, p)| *p).collect::<Vec<_>>());
            let stationary_old: HashSet<usize> = stay.into_iter().map(|s| kept[s].0).collect();
            let embedded_keys = if parent_in_a {
                Vec::new()
            } else {
                let fresh: Vec<&String> =
                    kids.iter().filter(|k| !a.objs.contains_key(*k)).collect();
                let keys = initial_keys(fresh.len());
                fresh.into_iter().cloned().zip(keys).collect::<Vec<_>>()
            };
            let embedded: HashMap<&str, &String> =
                embedded_keys.iter().map(|(k, v)| (k.as_str(), v)).collect();
            let mut keys: Vec<Option<String>> = Vec::with_capacity(kids.len());
            for (i, kid) in kids.iter().enumerate() {
                keys.push(if stationary_old.contains(&i) {
                    self.objs.get(kid).map(|o| o.key.clone())
                } else {
                    embedded.get(kid.as_str()).map(|k| (*k).clone())
                });
            }
            for i in 0..kids.len() {
                if keys[i].is_some() {
                    continue;
                }
                let left = if i > 0 { keys[i - 1].clone() } else { None };
                let right = keys[i + 1..].iter().flatten().next().cloned();
                keys[i] = Some(key_between(left.as_deref(), right.as_deref()));
            }
            for (i, kid) in kids.iter().enumerate() {
                let key = keys[i].clone().expect("assigned");
                let target = &b.objs[kid];
                let page = target.page.as_str();
                let parent_obj = &b.objs[parent];
                let parent_value = parent_ref(parent_obj.level, &parent_obj.id);
                match a.objs.get(kid) {
                    None if parent_in_a => {
                        let mut stripped = target.value.clone();
                        strip_existing(&mut stripped, target.level, &a);
                        creates.push(draft(
                            "create",
                            page,
                            &target.id,
                            json!({"level": target.level.code(), "parent": parent_value, "key": key, "node": stripped}),
                        ));
                    }
                    None => {}
                    Some(_) if stationary_old.contains(&i) => {}
                    Some(_) => {
                        moves.push(draft(
                            "move",
                            page,
                            &target.id,
                            json!({"level": target.level.code(), "parent": parent_value, "key": key}),
                        ));
                    }
                }
            }
        }

        for rf in &a.order {
            let old = &a.objs[rf];
            if b.objs.contains_key(rf) {
                continue;
            }
            if old.parent.as_ref().is_some_and(|p| b.objs.contains_key(p)) {
                deletes.push(draft(
                    "delete",
                    &old.page,
                    &old.id,
                    json!({"level": old.level.code()}),
                ));
            }
        }
        let mut all = unlocks;
        all.extend(creates);
        all.extend(sets);
        all.extend(moves);
        all.extend(deletes);
        all.extend(locks);
        Ok(all)
    }
}

/// Remove children that already exist elsewhere (they arrive as moves).
fn strip_existing(value: &mut Value, level: Level, existing: &Flat<'_>) {
    fn go(value: &mut Value, level: Level, page: &str, existing: &Flat<'_>) {
        let Some(next) = child_level(level) else {
            return;
        };
        let container = level.container();
        let Some(items) = value.get_mut(container).and_then(Value::as_array_mut) else {
            return;
        };
        items.retain(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .map(|id| {
                    let kid_page = if level == Level::Doc { id } else { page };
                    !existing.objs.contains_key(&ref_of(next, kid_page, id))
                })
                .unwrap_or(true)
        });
        for item in items.iter_mut() {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let kid_page = if level == Level::Doc {
                id.clone()
            } else {
                page.to_string()
            };
            go(item, next, &kid_page, existing);
        }
    }
    let page = if level == Level::Page {
        value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };
    go(value, level, &page, existing);
}

/// Indexes of one longest strictly increasing subsequence.
fn longest_increasing(values: &[usize]) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![usize::MAX; values.len()];
    let mut tail_index: Vec<usize> = Vec::new();
    for (i, value) in values.iter().enumerate() {
        let slot = tails.partition_point(|t| t < value);
        if slot == tails.len() {
            tails.push(*value);
            tail_index.push(i);
        } else {
            tails[slot] = *value;
            tail_index[slot] = i;
        }
        previous[i] = if slot > 0 {
            tail_index[slot - 1]
        } else {
            usize::MAX
        };
    }
    let mut out = Vec::new();
    let mut cursor = tail_index.last().copied();
    while let Some(i) = cursor {
        out.push(i);
        cursor = (previous[i] != usize::MAX).then_some(previous[i]);
    }
    out.reverse();
    out
}

// -------------------------------------------------------------- replay

#[derive(Debug)]
pub struct Replay {
    pub state: Replayer,
    pub applied: usize,
    pub rejected: Vec<(String, String)>,
    pub pending: Vec<String>,
}

impl Replay {
    pub fn document(&self) -> Value {
        self.state.materialize()
    }

    /// Canonical bytes of the replayed document (sorted keys, pretty-printed).
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(&self.document()).expect("document serializes");
        bytes.push(b'\n');
        bytes
    }
}

/// Replay `ops` over `genesis`. The result depends only on the *set* of
/// operations: duplicates collapse by id and delivery order is irrelevant.
pub fn replay(genesis: &Value, ops: &[Op]) -> Result<Replay> {
    let mut state = Replayer::new(genesis)?;
    let mut by_id: BTreeMap<&str, &Op> = BTreeMap::new();
    let mut rejected = Vec::new();
    for op in ops {
        if op.check().is_err() {
            rejected.push((op.id.clone(), "invalid-op".to_string()));
            continue;
        }
        by_id.insert(op.id.as_str(), op);
    }
    let mut remaining: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, op) in &by_id {
        let unique: BTreeSet<&str> = op.deps.iter().map(String::as_str).collect();
        let mut missing = false;
        for dep in &unique {
            if by_id.contains_key(dep) && dep != id {
                dependents.entry(dep).or_default().push(id);
            } else {
                missing = true;
            }
        }
        remaining.insert(id, if missing { usize::MAX } else { unique.len() });
    }
    let mut ready: BTreeSet<(OrdKey, &str)> = by_id
        .iter()
        .filter(|(id, _)| remaining[**id] == 0)
        .map(|(id, op)| (op.order_key(), *id))
        .collect();
    let mut applied = 0;
    while let Some(first) = ready.iter().next().cloned() {
        ready.remove(&first);
        let op = by_id[first.1];
        match state.apply_op(op) {
            Ok(()) => applied += 1,
            Err(error) => rejected.push((op.id.clone(), error.code.to_string())),
        }
        for dependent in dependents.get(first.1).into_iter().flatten() {
            let count = remaining.get_mut(dependent).expect("tracked");
            if *count == usize::MAX {
                continue;
            }
            *count -= 1;
            if *count == 0 {
                ready.insert((by_id[dependent].order_key(), dependent));
            }
        }
    }
    let pending = by_id
        .keys()
        .filter(|id| !state.metas.contains_key(**id))
        .map(|id| id.to_string())
        .collect();
    rejected.sort();
    Ok(Replay {
        state,
        applied,
        rejected,
        pending,
    })
}

/// Digest of a canonical document, for equality checks and reports.
pub fn document_digest(document: &Value) -> String {
    sha256_hex(canonical_json(document).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sort_between_their_bounds() {
        let a = key_between(None, None);
        let b = key_between(Some(&a), None);
        let mid = key_between(Some(&a), Some(&b));
        assert!(a < mid && mid < b, "{a} {mid} {b}");
        let before = key_between(None, Some(&a));
        assert!(before < a);
        let mut left = a.clone();
        for _ in 0..50 {
            let next = key_between(Some(&left), Some(&b));
            assert!(left < next && next < b && valid_key(&next));
            left = next;
        }
    }

    #[test]
    fn initial_keys_are_ascending_and_valid() {
        for n in [0, 1, 5, 61, 62, 400] {
            let keys = initial_keys(n);
            assert_eq!(keys.len(), n);
            assert!(keys.windows(2).all(|w| w[0] < w[1]));
            assert!(keys.iter().all(|k| valid_key(k)));
        }
    }

    #[test]
    fn longest_increasing_run() {
        assert_eq!(longest_increasing(&[3, 1, 2, 5, 4]).len(), 3);
        assert!(longest_increasing(&[]).is_empty());
    }
}

//! Operation and actor types, canonical bytes and content-derived ids
//! (spec sections 3.1, 3.4, 7.1 and 10).
use super::hlc::Hlc;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const SCHEMA: u64 = 1;
/// Default single-operation bound (canonical bytes).
pub const MAX_OP_BYTES: usize = 256 * 1024;
pub const MAX_OPS_PER_BATCH: usize = 500;
pub const MAX_ID_BYTES: usize = 256;
pub const MAX_DEPS: usize = 64;
pub const MAX_PATH_SEGMENTS: usize = 16;
pub const MAX_KEY_BYTES: usize = 64;
pub const MAX_ACTOR_TEXT: usize = 128;

pub const KINDS: [&str; 5] = ["set-property", "create", "delete", "move", "scene"];

/// Who authored an operation (spec section 7.1; signing keys arrive with the
/// actor/stats phase, so the id is derived from kind and handle for now).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directed_by: Option<String>,
}

impl Actor {
    /// `act_` + first 16 bytes of `sha256("kind:handle")`, hex.
    pub fn derive_id(kind: &str, handle: &str) -> String {
        let digest = Sha256::digest(format!("{kind}:{handle}").as_bytes());
        format!("act_{}", hex::encode(&digest[..16]))
    }

    pub fn validate(&self) -> Result<()> {
        if !matches!(self.kind.as_str(), "human" | "agent") {
            bail!(
                "[invalid-input] actor kind must be human or agent, got {:?}",
                self.kind
            )
        }
        check_id("actor id", &self.id)?;
        for text in [
            self.display_name.as_deref(),
            self.tool.as_deref(),
            self.model.as_deref(),
            self.directed_by.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if text.len() > MAX_ACTOR_TEXT || text.chars().any(char::is_control) {
                bail!("[limit-exceeded] actor text fields are limited to {MAX_ACTOR_TEXT} printable bytes")
            }
        }
        if self.kind == "agent" && self.directed_by.is_none() {
            bail!(
                "[invalid-input] agent actor {} requires --directed-by",
                self.id
            )
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Op {
    pub schema: u64,
    pub id: String,
    pub actor: String,
    pub hlc: Hlc,
    pub base: String,
    pub page: String,
    pub kind: String,
    pub targets: Vec<String>,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    /// Unknown fields survive and are ignored (spec section 3.1).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The unhashed parts of an operation, filled in by the planner.
#[derive(Debug, Clone)]
pub struct Draft {
    pub page: String,
    pub kind: &'static str,
    pub targets: Vec<String>,
    pub payload: Value,
}

impl Op {
    pub fn build(
        draft: Draft,
        actor: &str,
        hlc: Hlc,
        base: &str,
        deps: Vec<String>,
        via: &str,
    ) -> Result<Op> {
        let mut extra = BTreeMap::new();
        extra.insert("via".to_string(), Value::String(via.into()));
        let mut op = Op {
            schema: SCHEMA,
            id: String::new(),
            actor: actor.into(),
            hlc,
            base: base.into(),
            page: draft.page,
            kind: draft.kind.into(),
            targets: draft.targets,
            payload: draft.payload,
            deps,
            extra,
        };
        op.id = op.compute_id();
        op.check()?;
        Ok(op)
    }

    pub fn via(&self) -> &str {
        self.extra.get("via").and_then(Value::as_str).unwrap_or("")
    }

    /// Canonical bytes: sorted keys, no whitespace, `id` omitted.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut value = serde_json::to_value(self).expect("operation serializes");
        if let Some(map) = value.as_object_mut() {
            map.remove("id");
            map.remove("sig");
        }
        canonical_json(&value).into_bytes()
    }

    pub fn compute_id(&self) -> String {
        let digest = Sha256::digest(self.canonical_bytes());
        format!("op_{}", hex::encode(&digest[..16]))
    }

    /// Order used only to break ties between concurrent operations.
    pub fn order_key(&self) -> (u64, u32, String, String) {
        (
            self.hlc.wall_ms,
            self.hlc.counter,
            self.actor.clone(),
            self.id.clone(),
        )
    }

    /// Strict structural validation, run before an operation is stored or applied.
    pub fn check(&self) -> Result<()> {
        if self.schema != SCHEMA {
            bail!(
                "[unsupported-op] operation schema {} is not supported (expected {SCHEMA})",
                self.schema
            )
        }
        if !KINDS.contains(&self.kind.as_str()) {
            bail!("[unsupported-op] unknown operation kind {:?}", self.kind)
        }
        check_id("operation id", &self.id)?;
        check_id("actor", &self.actor)?;
        if !self.page.is_empty() {
            check_id("page", &self.page)?;
        }
        if self.targets.len() > MAX_OPS_PER_BATCH || self.deps.len() > MAX_DEPS {
            bail!(
                "[limit-exceeded] operation {} has too many targets or deps",
                self.id
            )
        }
        for target in &self.targets {
            check_id("target", target)?;
        }
        for dep in &self.deps {
            check_id("dependency", dep)?;
        }
        if self.base.len() > 80 || self.hlc.peer.len() > MAX_ID_BYTES {
            bail!(
                "[limit-exceeded] operation {} has an oversized base or peer",
                self.id
            )
        }
        let size = self.canonical_bytes().len();
        if size > MAX_OP_BYTES {
            bail!(
                "[limit-exceeded] operation {} is {size} canonical bytes; the limit is {MAX_OP_BYTES}",
                self.id
            )
        }
        if self.id != self.compute_id() {
            bail!(
                "[bad-id] operation id {} does not match its content (expected {})",
                self.id,
                self.compute_id()
            )
        }
        if !self.payload.is_object() {
            bail!(
                "[invalid-input] operation {} payload must be an object",
                self.id
            )
        }
        Ok(())
    }
}

/// Ids are opaque, bounded, printable and never path-like.
pub fn check_id(what: &str, id: &str) -> Result<()> {
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        bail!("[invalid-input] {what} must be 1 to {MAX_ID_BYTES} bytes")
    }
    if id.chars().any(char::is_control) || id.contains('\\') {
        bail!("[invalid-input] {what} {id:?} contains control or path characters")
    }
    Ok(())
}

pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("string serializes"));
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).expect("scalar serializes")),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Parse one stored/wire operation, enforcing the byte limit before parsing.
pub fn parse_op(bytes: &[u8]) -> Result<Op> {
    if bytes.len() > MAX_OP_BYTES + 4096 {
        bail!(
            "[limit-exceeded] operation is {} bytes; the limit is {MAX_OP_BYTES}",
            bytes.len()
        )
    }
    let op: Op = serde_json::from_slice(bytes)
        .map_err(|error| anyhow::anyhow!("[invalid-input] malformed operation: {error}"))?;
    op.check()?;
    Ok(op)
}

/// Document levels. Page, layer and node ids are namespaced per level and page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Doc,
    Page,
    Layer,
    Node,
}

impl Level {
    pub fn code(self) -> &'static str {
        match self {
            Level::Doc => "d",
            Level::Page => "p",
            Level::Layer => "l",
            Level::Node => "n",
        }
    }
    pub fn parse(code: &str) -> Result<Level> {
        Ok(match code {
            "d" => Level::Doc,
            "p" => Level::Page,
            "l" => Level::Layer,
            "n" => Level::Node,
            other => bail!("[invalid-input] unknown object level {other:?}"),
        })
    }
    /// JSON key holding this level's children.
    pub fn container(self) -> &'static str {
        match self {
            Level::Doc => "pages",
            Level::Page => "layers",
            Level::Layer => "nodes",
            Level::Node => "children",
        }
    }
}

/// Reference to an object: `level:page:id` (pages and the document use an
/// empty page component).
pub fn object_ref(level: Level, page: &str, id: &str) -> String {
    format!("{}:{page}:{id}", level.code())
}

pub fn draft(kind: &'static str, page: &str, id: &str, payload: Value) -> Draft {
    Draft {
        page: page.into(),
        kind,
        targets: vec![id.into()],
        payload,
    }
}

pub fn payload_object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("[invalid-input] payload must be an object"))
}

pub fn parent_ref(level: Level, id: &str) -> Value {
    json!({"level": level.code(), "id": id})
}

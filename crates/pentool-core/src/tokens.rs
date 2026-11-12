//! Design-token model: value schemas, hashing, bounded alias resolution, and the
//! shared structured error used by token validation and the `design` commands.
//!
//! Resolution is a pure function of the document bytes (`docs/design-systems-v1.md`
//! section 2.2): it never reads a library, `.pentool/`, or the network.
// DesignError is a structured, self-describing error; it is only built on failure paths.
#![allow(clippy::result_large_err)]
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// Longest alias chain (number of `ref` hops) that still resolves.
pub const MAX_ALIAS_DEPTH: usize = 8;
/// Most tokens a document or library may hold.
pub const MAX_TOKENS: usize = 5_000;
/// Longest token ID in bytes.
pub const MAX_ID_BYTES: usize = 128;
/// Largest canonical JSON of one token value.
pub const MAX_VALUE_BYTES: usize = 16 * 1024;
/// Token types a document store or a library may hold for rendering and update.
pub const TYPES: [&str; 6] = [
    "color",
    "stroke",
    "typography",
    "spacing",
    "radius",
    "shadow",
];

/// Structured failure: `{"error": {code, message, operation, page, token, path, action}}`.
#[derive(Debug, Clone)]
pub struct DesignError {
    pub code: &'static str,
    pub message: String,
    pub operation: Option<String>,
    pub page: Option<String>,
    pub token: Option<String>,
    pub path: Option<Value>,
    pub action: Option<String>,
}

impl DesignError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            operation: None,
            page: None,
            token: None,
            path: None,
            action: None,
        }
    }
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }
    pub fn path(mut self, path: Value) -> Self {
        self.path = Some(path);
        self
    }
    pub fn action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }
    pub fn operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }
    pub fn to_json(&self) -> Value {
        let mut error = Map::new();
        error.insert("code".into(), json!(self.code));
        error.insert("message".into(), json!(self.message));
        for (key, value) in [
            ("operation", &self.operation),
            ("page", &self.page),
            ("token", &self.token),
            ("action", &self.action),
        ] {
            if let Some(value) = value {
                error.insert(key.into(), json!(value));
            }
        }
        if let Some(path) = &self.path {
            error.insert("path".into(), path.clone());
        }
        json!({ "error": Value::Object(error) })
    }
}

impl fmt::Display for DesignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for DesignError {}

/// The cycle error of section 2.4; `path` starts and ends with the repeated token.
pub fn cycle_error(path: &[String]) -> DesignError {
    let first = path.first().cloned().unwrap_or_default();
    DesignError::new(
        "token-cycle",
        format!("style alias cycle: {}", path.join(" -> ")),
    )
    .token(first)
    .path(json!(path))
    .action("point one of the aliases at a literal value")
}

fn ref_of(value: &Value) -> Option<&str> {
    value.get("ref").and_then(Value::as_str)
}

/// Reject alias cycles in a token store. Existing documents are only checked for
/// cycles when opened; depth and count are enforced on write ([`check_write`]).
pub fn validate_cycles(styles: &Value) -> Result<(), DesignError> {
    let Some(object) = styles.as_object() else {
        return Ok(());
    };
    // Tokens already proven acyclic; every chain is walked at most once overall.
    let mut safe: BTreeSet<&str> = BTreeSet::new();
    for start in object.keys() {
        let mut stack: Vec<&str> = Vec::new();
        let mut current = start.as_str();
        loop {
            if safe.contains(current) {
                break;
            }
            if let Some(index) = stack.iter().position(|item| *item == current) {
                let mut path: Vec<String> = stack[index..].iter().map(|s| (*s).into()).collect();
                path.push(current.into());
                return Err(cycle_error(&path));
            }
            stack.push(current);
            match object
                .get(current)
                .and_then(|token| token.get("value"))
                .and_then(ref_of)
            {
                Some(next) if object.contains_key(next) => current = next,
                _ => break,
            }
        }
        safe.extend(stack);
    }
    Ok(())
}

/// Canonical JSON: keys sorted, no insignificant whitespace.
pub fn canonical(value: &Value) -> String {
    fn write(value: &Value, out: &mut String) {
        match value {
            Value::Object(map) => {
                let sorted: BTreeMap<&String, &Value> = map.iter().collect();
                out.push('{');
                for (index, (key, child)) in sorted.into_iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String(key.clone()).to_string());
                    out.push(':');
                    write(child, out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (index, child) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write(child, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// `sha256:` of the canonical `{"type","value"}` of a token record.
pub fn value_hash(token_type: &str, value: &Value) -> String {
    crate::asset::hash_bytes(canonical(&json!({"type": token_type, "value": value})).as_bytes())
}

/// Value hash of a stored token record, or `None` for a malformed record.
pub fn record_hash(record: &Value) -> Option<String> {
    Some(value_hash(
        record.get("type")?.as_str()?,
        record.get("value")?,
    ))
}

/// Token ID grammar of section 1.2 (used for library and token-file tokens).
pub fn valid_library_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return false;
    }
    let mut segments = id.split('.');
    let first = segments.next().unwrap_or("");
    let ok_first = first.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && first
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok_first {
        return false;
    }
    let mut count = 0;
    for segment in segments {
        count += 1;
        if count > 7
            || !segment
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            || !segment
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return false;
        }
    }
    true
}

fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

/// Check a non-alias value against its declared type. `strict` (library tokens)
/// additionally rejects unknown typography keys; documents stay lenient.
pub fn validate_value(token_type: &str, value: &Value, strict: bool) -> Result<(), String> {
    if canonical(value).len() > MAX_VALUE_BYTES {
        return Err(format!("token value exceeds {MAX_VALUE_BYTES} bytes"));
    }
    match token_type {
        "color" => match value {
            Value::String(color) if crate::scene::is_valid_color(color) => Ok(()),
            Value::String(color) => Err(format!(
                "{color:?} is not a color; use none, #RGB, #RGBA, #RRGGBB or #RRGGBBAA"
            )),
            Value::Object(object) if object.contains_key("space") => Ok(()),
            _ => Err("color must be a color string or a native color object".into()),
        },
        "typography" => {
            let object = value.as_object().ok_or("typography must be an object")?;
            if strict {
                for key in object.keys() {
                    if !matches!(
                        key.as_str(),
                        "font" | "size" | "weight" | "italic" | "leading" | "tracking" | "axes"
                    ) {
                        return Err(format!("unknown typography key {key:?}"));
                    }
                }
                if !object.get("font").is_some_and(Value::is_string) {
                    return Err("typography needs a string `font`".into());
                }
                match object.get("size").and_then(finite) {
                    Some(size) if size > 0.0 && size <= 10_000.0 => {}
                    _ => return Err("typography `size` must be a number in (0, 10000]".into()),
                }
                if let Some(weight) = object.get("weight") {
                    if !weight.as_u64().is_some_and(|w| (1..=1000).contains(&w)) {
                        return Err("typography `weight` must be an integer 1-1000".into());
                    }
                }
                if object.get("italic").is_some_and(|v| !v.is_boolean()) {
                    return Err("typography `italic` must be a boolean".into());
                }
                if let Some(leading) = object.get("leading") {
                    if !finite(leading).is_some_and(|n| n > 0.0) {
                        return Err("typography `leading` must be a positive number".into());
                    }
                }
                if let Some(tracking) = object.get("tracking") {
                    if finite(tracking).is_none() {
                        return Err("typography `tracking` must be a finite number".into());
                    }
                }
                if let Some(axes) = object.get("axes") {
                    let axes = axes
                        .as_object()
                        .ok_or("typography `axes` must be an object")?;
                    if axes.values().any(|v| finite(v).is_none()) {
                        return Err("typography `axes` values must be finite numbers".into());
                    }
                }
            }
            Ok(())
        }
        "spacing" => match finite(value) {
            Some(n) if (0.0..=1_000_000.0).contains(&n) => Ok(()),
            _ => Err("spacing must be a number in [0, 1000000]".into()),
        },
        "radius" => {
            let ok = |v: &Value| finite(v).is_some_and(|n| n >= 0.0);
            match value {
                Value::Array(pair) if pair.len() == 2 && pair.iter().all(ok) => Ok(()),
                other if ok(other) => Ok(()),
                _ => Err("radius must be a number >= 0 or [rx, ry]".into()),
            }
        }
        "shadow" => {
            let rows = value.as_array().ok_or("shadow must be an array")?;
            if rows.is_empty() || rows.len() > 8 {
                return Err("shadow needs 1-8 entries".into());
            }
            for row in rows {
                let object = row.as_object().ok_or("shadow entries must be objects")?;
                for key in ["dx", "dy", "blur", "spread"] {
                    match object.get(key).and_then(finite) {
                        Some(n) if n.abs() <= 100_000.0 && (key != "blur" || n >= 0.0) => {}
                        _ => return Err(format!("shadow `{key}` must be a bounded finite number")),
                    }
                }
                match object.get("color") {
                    Some(Value::String(color)) if crate::scene::is_valid_color(color) => {}
                    Some(Value::Object(_)) => {}
                    _ => return Err("shadow `color` must be a color".into()),
                }
            }
            Ok(())
        }
        // `stroke` and compositing token types keep their existing, unchecked shape.
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Resolved,
    Unresolved,
    Invalid,
}
impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Resolved => "resolved",
            Status::Unresolved => "unresolved",
            Status::Invalid => "invalid",
        }
    }
}

/// Result of following one reference through the document's own token store.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub chain: Vec<String>,
    pub status: Status,
    pub token_type: Option<String>,
    pub value: Option<Value>,
    pub reason: Option<String>,
}

/// Section 2.2 steps 1-3. Never fails: unresolved and invalid are statuses so a
/// renderer can keep using the literal fallback.
pub fn resolve(styles: &Value, name: &str) -> Resolution {
    let mut chain = vec![name.to_owned()];
    let mut declared: Option<String> = None;
    loop {
        let current = chain.last().unwrap().clone();
        let Some(record) = styles.get(&current).filter(|r| r.is_object()) else {
            return Resolution {
                chain,
                status: Status::Unresolved,
                token_type: declared,
                value: None,
                reason: Some(format!("token {current:?} does not exist")),
            };
        };
        let token_type = record.get("type").and_then(Value::as_str).unwrap_or("");
        let invalid = |chain: Vec<String>, reason: String| Resolution {
            chain,
            status: Status::Invalid,
            token_type: Some(token_type.to_owned()),
            value: None,
            reason: Some(reason),
        };
        if let Some(declared) = &declared {
            if declared != token_type {
                return invalid(
                    chain,
                    format!("token-type-mismatch: alias of {declared} points at {token_type}"),
                );
            }
        }
        declared = Some(token_type.to_owned());
        let Some(value) = record.get("value") else {
            return invalid(chain, "token value is missing".into());
        };
        if let Some(next) = ref_of(value) {
            if chain.iter().any(|item| item == next) {
                let mut path = chain.clone();
                path.push(next.into());
                return invalid(chain, cycle_error(&path).message);
            }
            if chain.len() > MAX_ALIAS_DEPTH {
                return invalid(chain, format!("alias depth exceeds {MAX_ALIAS_DEPTH}"));
            }
            chain.push(next.to_owned());
            continue;
        }
        return match validate_value(token_type, value, false) {
            Ok(()) => Resolution {
                chain,
                status: Status::Resolved,
                token_type: Some(token_type.to_owned()),
                value: Some(value.clone()),
                reason: None,
            },
            Err(reason) => invalid(chain, reason),
        };
    }
}

/// Write-time checks: cycles, alias depth, same-type aliases, token count, and
/// value schemas. `only` limits value/alias checks to tokens being written, so a
/// legacy token that is already odd never blocks an unrelated edit.
pub fn check_write(styles: &Value, only: Option<&[String]>) -> Result<(), DesignError> {
    let Some(object) = styles.as_object() else {
        return Ok(());
    };
    validate_cycles(styles)?;
    if object.len() > MAX_TOKENS {
        return Err(DesignError::new(
            "limit-exceeded",
            format!(
                "document has {} tokens; the limit is {MAX_TOKENS}",
                object.len()
            ),
        )
        .action("delete unused tokens before adding more"));
    }
    let names: Vec<&String> = match only {
        Some(list) => list
            .iter()
            .filter_map(|n| object.get_key_value(n))
            .map(|(k, _)| k)
            .collect(),
        None => object.keys().collect(),
    };
    for name in names {
        let resolution = resolve(styles, name);
        if resolution.status == Status::Invalid {
            let reason = resolution.reason.clone().unwrap_or_default();
            let code = if reason.starts_with("alias depth") {
                "limit-exceeded"
            } else if reason.starts_with("token-type-mismatch") {
                "token-type-mismatch"
            } else {
                "invalid-token-value"
            };
            return Err(DesignError::new(code, format!("token {name}: {reason}"))
                .token(name.clone())
                .path(json!(resolution.chain))
                .action(match code {
                    "limit-exceeded" => "shorten the alias chain",
                    "token-type-mismatch" => "alias a token of the same type",
                    _ => "fix the token value",
                }));
        }
    }
    Ok(())
}

/// Reverse index of who uses which token, built in one pass over the document.
pub struct Usage {
    nodes: BTreeMap<String, BTreeSet<String>>,
    aliased_by: BTreeMap<String, BTreeSet<String>>,
}

impl Usage {
    pub fn build(raw: &Value) -> Self {
        let mut usage = Usage {
            nodes: BTreeMap::new(),
            aliased_by: BTreeMap::new(),
        };
        if let Some(styles) = raw.get("styles").and_then(Value::as_object) {
            for (name, record) in styles {
                if let Some(target) = record.get("value").and_then(ref_of) {
                    usage
                        .aliased_by
                        .entry(target.to_owned())
                        .or_default()
                        .insert(name.clone());
                }
            }
        }
        if let Some(object) = raw.as_object() {
            for (key, child) in object {
                match key.as_str() {
                    "styles" | "design" | "design_library" => {}
                    "pages" => {
                        for page in child.as_array().into_iter().flatten() {
                            let id = page.get("id").and_then(Value::as_str);
                            usage.walk(page, id, None);
                        }
                    }
                    _ => usage.walk(child, None, None),
                }
            }
        }
        usage
    }

    fn walk(&mut self, value: &Value, page: Option<&str>, current: Option<&str>) {
        match value {
            Value::Object(object) => {
                let id = object.get("id").and_then(Value::as_str).or(current);
                let label = |id: Option<&str>| match (page, id) {
                    (Some(page), Some(id)) if page != id => format!("{page}/{id}"),
                    (_, Some(id)) => id.to_owned(),
                    (Some(page), None) => page.to_owned(),
                    (None, None) => "document".to_owned(),
                };
                for key in ["ref", "style_ref"] {
                    if let Some(name) = object.get(key).and_then(Value::as_str) {
                        self.nodes
                            .entry(name.to_owned())
                            .or_default()
                            .insert(label(id));
                    }
                }
                for child in object.values() {
                    self.walk(child, page, id);
                }
            }
            Value::Array(items) => {
                for child in items {
                    self.walk(child, page, current);
                }
            }
            _ => {}
        }
    }

    /// `(nodes, tokens)` whose resolved chain includes `token` (sorted, deduplicated).
    pub fn dependents(&self, token: &str) -> (Vec<String>, Vec<String>) {
        let mut tokens: BTreeSet<String> = BTreeSet::new();
        let mut queue = vec![token.to_owned()];
        while let Some(name) = queue.pop() {
            for parent in self.aliased_by.get(&name).into_iter().flatten() {
                if tokens.insert(parent.clone()) {
                    queue.push(parent.clone());
                }
            }
        }
        let mut nodes: BTreeSet<String> = self.nodes.get(token).cloned().unwrap_or_default();
        for alias in &tokens {
            if let Some(found) = self.nodes.get(alias) {
                nodes.extend(found.iter().cloned());
            }
        }
        (nodes.into_iter().collect(), tokens.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_grammar() {
        assert!(valid_library_id("color.accent"));
        assert!(valid_library_id("type.heading.1"));
        assert!(!valid_library_id("Color.accent"));
        assert!(!valid_library_id("color..x"));
        assert!(!valid_library_id("color accent"));
    }

    #[test]
    fn canonical_is_key_sorted() {
        assert_eq!(
            canonical(&json!({"b":1,"a":[2,{"d":1,"c":2}]})),
            r#"{"a":[2,{"c":2,"d":1}],"b":1}"#
        );
    }

    #[test]
    fn depth_cap_is_eight_hops() {
        let mut styles = Map::new();
        for i in 0..10 {
            styles.insert(
                format!("t{i}"),
                json!({"type":"spacing","value":{"ref":format!("t{}", i+1)}}),
            );
        }
        styles.insert("t10".into(), json!({"type":"spacing","value":4}));
        let styles = Value::Object(styles);
        assert_eq!(resolve(&styles, "t2").status, Status::Resolved);
        assert_eq!(resolve(&styles, "t1").status, Status::Invalid);
    }
}

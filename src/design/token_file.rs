//! Token file import/export (`docs/design-systems-v1.md` section 10): the native
//! `pentool.tokens` JSON format and a strict one-to-one W3C design-tokens subset.
//! Anything that does not map exactly is rejected, never approximated.
// DesignError is a structured, self-describing error; it is only built on failure paths.
#![allow(clippy::result_large_err)]
use super::{
    cli::read_document,
    library::{parse_tokens, LibToken},
    page_args, paginate,
};
use crate::{tokens as tok, tokens::DesignError, transaction};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fs, path::Path};

type Result<T> = std::result::Result<T, DesignError>;
pub type TokenSet = BTreeMap<String, LibToken>;

/// Largest token file read (bounded work before any parsing).
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> DesignError {
    DesignError::new("invalid-input", message)
}

fn unsupported(pointer: &str, construct: &str, why: &str) -> DesignError {
    DesignError::new(
        "unsupported-token-construct",
        format!("{pointer}: {construct} {why}"),
    )
    .path(json!(pointer))
    .action(
        "remove or rewrite this construct; Pentool does not approximate unsupported W3C features",
    )
}

fn pointer_join(base: &str, key: &str) -> String {
    format!("{base}/{}", key.replace('~', "~0").replace('/', "~1"))
}

// ---------------------------------------------------------------- formats --

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Native,
    W3c,
}

impl Format {
    pub fn parse(name: &str) -> Result<Format> {
        match name {
            "native" => Ok(Format::Native),
            "w3c" => Ok(Format::W3c),
            other => Err(invalid(format!(
                "unknown token file format {other:?}; use native or w3c"
            ))),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Format::Native => "native",
            Format::W3c => "w3c",
        }
    }
}

fn read_file(path: &Path) -> Result<Value> {
    let meta = fs::metadata(path)
        .map_err(|e| invalid(format!("could not read {}: {e}", path.display())))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(DesignError::new(
            "limit-exceeded",
            format!(
                "token file is {} bytes; the limit is {MAX_FILE_BYTES}",
                meta.len()
            ),
        )
        .action("split the token file"));
    }
    let bytes = fs::read(path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| invalid(format!("{} is not valid JSON: {e}", path.display())))
}

// ----------------------------------------------------------------- native --

pub fn parse_native(root: &Value) -> Result<TokenSet> {
    let object = root
        .as_object()
        .ok_or_else(|| invalid("a native token file must be a JSON object"))?;
    for key in object.keys() {
        if !matches!(key.as_str(), "schema" | "tokens") {
            return Err(unsupported(
                &pointer_join("", key),
                "unknown top-level key",
                "in a native token file (allowed: schema, tokens)",
            ));
        }
    }
    if object.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err(invalid("token file schema must be 1").action("set \"schema\": 1"));
    }
    let tokens = object
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("token file needs a \"tokens\" object"))?;
    parse_tokens(tokens)
}

fn native_file(set: &TokenSet) -> Value {
    let mut tokens = Map::new();
    for (id, token) in set {
        let mut record = Map::new();
        record.insert("type".into(), json!(token.token_type));
        record.insert("value".into(), token.value.clone());
        if !token.description.is_empty() {
            record.insert("description".into(), json!(token.description));
        }
        tokens.insert(id.clone(), Value::Object(record));
    }
    json!({"schema": 1, "tokens": tokens})
}

// -------------------------------------------------------------------- w3c --

fn fmt_px(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}px", n as i64)
    } else {
        format!("{n}px")
    }
}

fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

fn parse_px(value: &Value, pointer: &str) -> Result<f64> {
    let text = value
        .as_str()
        .ok_or_else(|| unsupported(pointer, "dimension value", "must be a \"<n>px\" string"))?;
    if text.contains('{') {
        return Err(unsupported(
            pointer,
            "an alias inside a composite value",
            "has no native equivalent",
        ));
    }
    let Some(digits) = text.strip_suffix("px") else {
        return Err(unsupported(
            pointer,
            &format!("dimension unit in {text:?}"),
            "is not px; only px maps to document units",
        ));
    };
    if digits.is_empty()
        || !digits
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return Err(unsupported(
            pointer,
            &format!("dimension {text:?}"),
            "is not a plain number of px",
        ));
    }
    digits
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| {
            unsupported(
                pointer,
                &format!("dimension {text:?}"),
                "is not a finite number",
            )
        })
}

fn alias_target(value: &Value) -> Option<&str> {
    let text = value.as_str()?;
    let inner = text.strip_prefix('{')?.strip_suffix('}')?;
    (!inner.is_empty() && !inner.contains(['{', '}'])).then_some(inner)
}

struct RawToken {
    id: String,
    pointer: String,
    w3c_type: Option<String>,
    value: Value,
    description: Option<String>,
}

fn walk(
    node: &Map<String, Value>,
    path: &[String],
    pointer: &str,
    inherited: Option<&str>,
    out: &mut Vec<RawToken>,
) -> Result<()> {
    let mut group_type = inherited;
    if let Some(kind) = node.get("$type") {
        group_type = Some(kind.as_str().ok_or_else(|| {
            unsupported(&pointer_join(pointer, "$type"), "$type", "must be a string")
        })?);
    }
    for (key, child) in node {
        let here = pointer_join(pointer, key);
        if key.starts_with('$') {
            match key.as_str() {
                "$type" => {}
                "$description" => {
                    return Err(unsupported(
                        &here,
                        "group $description",
                        "has no native equivalent (only token descriptions are kept)",
                    ))
                }
                other => {
                    return Err(unsupported(
                        &here,
                        other,
                        "is not supported (no native equivalent; not approximated)",
                    ))
                }
            }
            continue;
        }
        if key.is_empty() || key.contains(['.', '{', '}']) {
            return Err(DesignError::new(
                "invalid-token-id",
                format!("{here}: group or token name {key:?} is empty or contains . {{ }}"),
            )
            .path(json!(here)));
        }
        let child = child.as_object().ok_or_else(|| {
            unsupported(
                &here,
                "a non-object member",
                "is neither a group nor a token",
            )
        })?;
        let mut next = path.to_vec();
        next.push(key.clone());
        if child.contains_key("$value") {
            for member in child.keys() {
                if !matches!(member.as_str(), "$value" | "$type" | "$description") {
                    let what = if member.starts_with('$') {
                        member.clone()
                    } else {
                        format!("child {member:?} inside a token")
                    };
                    return Err(unsupported(
                        &pointer_join(&here, member),
                        &what,
                        "is not supported (no native equivalent; not approximated)",
                    ));
                }
            }
            let w3c_type = match child.get("$type") {
                Some(kind) => Some(
                    kind.as_str()
                        .ok_or_else(|| {
                            unsupported(&pointer_join(&here, "$type"), "$type", "must be a string")
                        })?
                        .to_owned(),
                ),
                None => group_type.map(str::to_owned),
            };
            let description = match child.get("$description") {
                None => None,
                Some(Value::String(text)) => Some(text.clone()),
                Some(_) => {
                    return Err(unsupported(
                        &pointer_join(&here, "$description"),
                        "$description",
                        "must be a string",
                    ))
                }
            };
            out.push(RawToken {
                id: next.join("."),
                pointer: here,
                w3c_type,
                value: child["$value"].clone(),
                description,
            });
            if out.len() > tok::MAX_TOKENS {
                return Err(DesignError::new(
                    "limit-exceeded",
                    format!("token file has more than {} tokens", tok::MAX_TOKENS),
                )
                .action("split the token file"));
            }
        } else {
            walk(child, &next, &here, group_type, out)?;
        }
    }
    Ok(())
}

fn dimension_kind(id: &str, pointer: &str) -> Result<&'static str> {
    match id.split('.').next().unwrap_or("") {
        "space" | "spacing" | "size" => Ok("spacing"),
        "radius" => Ok("radius"),
        other => Err(unsupported(
            pointer,
            &format!("dimension token in group {other:?}"),
            "has no native type; dimensions map only under space, spacing, size (spacing) or radius",
        )),
    }
}

fn convert(raw: &RawToken, w3c_type: &str) -> Result<(String, Value)> {
    let pointer = pointer_join(&raw.pointer, "$value");
    let native = match w3c_type {
        "color" => "color",
        "dimension" => dimension_kind(&raw.id, &raw.pointer)?,
        "typography" => "typography",
        "shadow" => "shadow",
        "fontFamily" | "fontWeight" => {
            return Err(unsupported(
                &raw.pointer,
                &format!("standalone $type {w3c_type:?}"),
                "has no native token type; use it inside a typography token",
            ))
        }
        other => {
            return Err(unsupported(
                &raw.pointer,
                &format!("$type {other:?}"),
                "has no native token type",
            ))
        }
    };
    if let Some(target) = alias_target(&raw.value) {
        return Ok((native.into(), json!({"ref": target})));
    }
    let value = match native {
        "color" => {
            let text = raw.value.as_str().ok_or_else(|| {
                unsupported(
                    &pointer,
                    "color object (colorSpace/components)",
                    "has no exact native equivalent; use a hex string",
                )
            })?;
            if !text.starts_with('#') || !crate::scene::is_valid_color(text) {
                return Err(unsupported(
                    &pointer,
                    &format!("color {text:?}"),
                    "is not a #rgb, #rgba, #rrggbb or #rrggbbaa hex string",
                ));
            }
            raw.value.clone()
        }
        "spacing" | "radius" => number(parse_px(&raw.value, &pointer)?),
        "typography" => typography_in(&raw.value, &pointer)?,
        _ => shadow_in(&raw.value, &pointer)?,
    };
    Ok((native.into(), value))
}

fn typography_in(value: &Value, pointer: &str) -> Result<Value> {
    let object = value
        .as_object()
        .ok_or_else(|| unsupported(pointer, "typography value", "must be an object"))?;
    let mut out = Map::new();
    for (key, member) in object {
        let here = pointer_join(pointer, key);
        match key.as_str() {
            "fontFamily" => {
                let family = match member {
                    Value::String(name) => name.clone(),
                    Value::Array(list) if list.len() == 1 && list[0].is_string() => {
                        list[0].as_str().unwrap().to_owned()
                    }
                    Value::Array(_) => {
                        return Err(unsupported(
                            &here,
                            "fontFamily fallback list",
                            "cannot be represented; keep exactly one family",
                        ))
                    }
                    _ => return Err(unsupported(&here, "fontFamily", "must be a string")),
                };
                if family.contains('{') {
                    return Err(unsupported(
                        &here,
                        "an alias inside a composite value",
                        "has no native equivalent",
                    ));
                }
                out.insert("font".into(), json!(family));
            }
            "fontSize" => {
                out.insert("size".into(), number(parse_px(member, &here)?));
            }
            "fontWeight" => match member.as_u64() {
                Some(weight) => {
                    out.insert("weight".into(), json!(weight));
                }
                None => {
                    return Err(unsupported(
                        &here,
                        "fontWeight",
                        "must be an integer number (keywords and aliases are not mapped)",
                    ))
                }
            },
            "lineHeight" => {
                if member.is_number() {
                    return Err(unsupported(
                        &here,
                        "unitless lineHeight",
                        "is ambiguous against absolute leading; use a \"<n>px\" dimension",
                    ));
                }
                out.insert("leading".into(), number(parse_px(member, &here)?));
            }
            "letterSpacing" => {
                out.insert("tracking".into(), number(parse_px(member, &here)?));
            }
            other => {
                return Err(unsupported(
                    &here,
                    &format!("typography member {other:?}"),
                    "has no native equivalent",
                ))
            }
        }
    }
    Ok(Value::Object(out))
}

fn shadow_in(value: &Value, pointer: &str) -> Result<Value> {
    let rows: Vec<(&Value, String)> = match value {
        Value::Array(list) => list
            .iter()
            .enumerate()
            .map(|(i, row)| (row, pointer_join(pointer, &i.to_string())))
            .collect(),
        other => vec![(other, pointer.to_owned())],
    };
    let mut out = Vec::new();
    for (row, here) in rows {
        let object = row.as_object().ok_or_else(|| {
            unsupported(
                &here,
                "shadow value",
                "must be an object or array of objects",
            )
        })?;
        let mut entry = Map::new();
        for (key, member) in object {
            let at = pointer_join(&here, key);
            match key.as_str() {
                "offsetX" => entry.insert("dx".into(), number(parse_px(member, &at)?)),
                "offsetY" => entry.insert("dy".into(), number(parse_px(member, &at)?)),
                "blur" => entry.insert("blur".into(), number(parse_px(member, &at)?)),
                "spread" => entry.insert("spread".into(), number(parse_px(member, &at)?)),
                "color" => {
                    let text = member
                        .as_str()
                        .filter(|t| t.starts_with('#') && crate::scene::is_valid_color(t));
                    match text {
                        Some(text) => entry.insert("color".into(), json!(text)),
                        None => {
                            return Err(unsupported(
                                &at,
                                "shadow color",
                                "must be a #rrggbb[aa] hex string",
                            ))
                        }
                    }
                }
                "inset" if member == &json!(false) => None,
                "inset" => {
                    return Err(unsupported(&at, "inset shadow", "has no native equivalent"))
                }
                other => {
                    return Err(unsupported(
                        &at,
                        &format!("shadow member {other:?}"),
                        "has no native equivalent",
                    ))
                }
            };
        }
        for needed in ["dx", "dy", "blur", "spread", "color"] {
            if !entry.contains_key(needed) {
                return Err(unsupported(
                    &here,
                    "shadow",
                    &format!("is missing the member that maps to {needed}"),
                ));
            }
        }
        out.push(Value::Object(entry));
    }
    Ok(Value::Array(out))
}

pub fn parse_w3c(root: &Value) -> Result<TokenSet> {
    let object = root
        .as_object()
        .ok_or_else(|| invalid("a W3C token file must be a JSON object"))?;
    let mut raws = Vec::new();
    walk(object, &[], "", None, &mut raws)?;
    // Untyped aliases take the type of the token they point at.
    let types: BTreeMap<String, Option<String>> = raws
        .iter()
        .map(|r| (r.id.clone(), r.w3c_type.clone()))
        .collect();
    let mut map = Map::new();
    for raw in &raws {
        let mut kind = raw.w3c_type.clone();
        let mut current = alias_target(&raw.value).map(str::to_owned);
        let mut hops = 0;
        while kind.is_none() {
            let Some(target) = current.take() else { break };
            hops += 1;
            if hops > tok::MAX_ALIAS_DEPTH {
                break;
            }
            match types.get(&target) {
                Some(Some(found)) => kind = Some(found.clone()),
                Some(None) => {
                    current = raws
                        .iter()
                        .find(|r| r.id == target)
                        .and_then(|r| alias_target(&r.value).map(str::to_owned));
                }
                None => break,
            }
        }
        let kind = kind.ok_or_else(|| {
            unsupported(
                &raw.pointer,
                "token without a resolvable $type",
                "cannot be mapped; add $type here or on its group",
            )
        })?;
        let (native, value) = convert(raw, &kind)?;
        let mut record = Map::new();
        record.insert("type".into(), json!(native));
        record.insert("value".into(), value);
        if let Some(text) = &raw.description {
            record.insert("description".into(), json!(text));
        }
        map.insert(raw.id.clone(), Value::Object(record));
    }
    parse_tokens(&map)
}

fn dim_value(n: f64) -> Value {
    json!(fmt_px(n))
}

fn num(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

/// Map one native token to its W3C token object, or say why it cannot be mapped.
fn to_w3c(id: &str, token: &LibToken) -> std::result::Result<Value, String> {
    let first = id.split('.').next().unwrap_or("");
    let (w3c_type, value) = match (token.token_type.as_str(), &token.value) {
        (kind, alias) if alias.get("ref").is_some() => {
            let target = alias["ref"]
                .as_str()
                .ok_or("alias target is not a string")?;
            let w3c = match kind {
                "color" => "color",
                "typography" => "typography",
                "shadow" => "shadow",
                "spacing" if matches!(first, "space" | "spacing" | "size") => "dimension",
                "radius" if first == "radius" => "dimension",
                "spacing" | "radius" => return Err(format!("{kind} token outside its W3C group")),
                other => return Err(format!("type {other} has no W3C equivalent")),
            };
            (w3c, json!(format!("{{{target}}}")))
        }
        ("color", Value::String(text)) if text != "none" => ("color", json!(text)),
        ("color", _) => return Err("only hex color strings map to W3C".into()),
        ("spacing", value) if matches!(first, "space" | "spacing" | "size") => (
            "dimension",
            dim_value(num(value).ok_or("spacing is not a number")?),
        ),
        ("spacing", _) => {
            return Err("spacing maps to W3C only under space, spacing or size".into())
        }
        ("radius", value) if first == "radius" && value.is_number() => (
            "dimension",
            dim_value(num(value).ok_or("radius is not finite")?),
        ),
        ("radius", _) => {
            return Err("only a single radius under the radius group maps to W3C".into())
        }
        ("typography", Value::Object(object)) => {
            let mut out = Map::new();
            for (key, member) in object {
                match key.as_str() {
                    "font" => out.insert("fontFamily".into(), member.clone()),
                    "size" => {
                        out.insert("fontSize".into(), dim_value(num(member).ok_or("bad size")?))
                    }
                    "weight" => out.insert("fontWeight".into(), member.clone()),
                    "leading" => out.insert(
                        "lineHeight".into(),
                        dim_value(num(member).ok_or("bad leading")?),
                    ),
                    "tracking" => out.insert(
                        "letterSpacing".into(),
                        dim_value(num(member).ok_or("bad tracking")?),
                    ),
                    other => return Err(format!("typography `{other}` has no W3C equivalent")),
                };
            }
            ("typography", Value::Object(out))
        }
        ("shadow", Value::Array(rows)) => {
            let mut out = Vec::new();
            for row in rows {
                let color = row
                    .get("color")
                    .and_then(Value::as_str)
                    .filter(|c| c.starts_with('#'))
                    .ok_or("only hex shadow colors map to W3C")?;
                let field = |k: &str| num(&row[k]).map(dim_value).ok_or("bad shadow number");
                out.push(json!({
                    "offsetX": field("dx")?, "offsetY": field("dy")?,
                    "blur": field("blur")?, "spread": field("spread")?,
                    "color": color,
                }));
            }
            ("shadow", Value::Array(out))
        }
        (other, _) => return Err(format!("type {other} has no W3C equivalent")),
    };
    let mut record = Map::new();
    record.insert("$type".into(), json!(w3c_type));
    record.insert("$value".into(), value);
    if !token.description.is_empty() {
        record.insert("$description".into(), json!(token.description));
    }
    Ok(Value::Object(record))
}

fn w3c_file(set: &TokenSet, skipped: &mut Vec<Value>) -> Result<Value> {
    let mut kept: BTreeMap<&String, Value> = BTreeMap::new();
    for (id, token) in set {
        match to_w3c(id, token) {
            Ok(record) => {
                kept.insert(id, record);
            }
            Err(reason) => skipped.push(json!({"token": id, "reason": reason})),
        }
    }
    // A token cannot also be a group; a skipped alias target orphans its aliases.
    loop {
        let ids: Vec<String> = kept.keys().map(|k| (*k).clone()).collect();
        let mut drop: Vec<(String, String)> = Vec::new();
        for id in &ids {
            let prefix = format!("{id}.");
            if ids.iter().any(|other| other.starts_with(&prefix)) {
                drop.push((id.clone(), "a W3C token cannot also be a group".into()));
                continue;
            }
            if let Some(target) = set[id].value.get("ref").and_then(Value::as_str) {
                if !kept.contains_key(&target.to_owned()) {
                    drop.push((
                        id.clone(),
                        format!("alias target {target} is not exportable"),
                    ));
                }
            }
        }
        if drop.is_empty() {
            break;
        }
        for (id, reason) in drop {
            kept.remove(&id);
            skipped.push(json!({"token": id, "reason": reason}));
        }
    }
    let mut root = Map::new();
    for (id, record) in kept {
        let segments: Vec<&str> = id.split('.').collect();
        let mut cursor = &mut root;
        for segment in &segments[..segments.len() - 1] {
            cursor = cursor
                .entry((*segment).to_owned())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .expect("group object");
        }
        cursor.insert(segments[segments.len() - 1].to_owned(), record);
    }
    skipped.sort_by(|a, b| a["token"].as_str().cmp(&b["token"].as_str()));
    Ok(Value::Object(root))
}

// ----------------------------------------------------------------- export --

/// Render a token set as a token file. Unmappable tokens are an error unless
/// `skip_unmappable`, in which case they are listed in the second result.
pub fn render(
    set: &TokenSet,
    format: Format,
    skip_unmappable: bool,
) -> Result<(Value, Vec<Value>)> {
    let mut skipped = Vec::new();
    let file = match format {
        Format::Native => native_file(set),
        Format::W3c => w3c_file(set, &mut skipped)?,
    };
    if !skipped.is_empty() && !skip_unmappable {
        let first = &skipped[0];
        return Err(DesignError::new(
            "unsupported-token-construct",
            format!(
                "{} token(s) cannot be exported as {}; first: {} ({})",
                skipped.len(),
                format.name(),
                first["token"].as_str().unwrap_or(""),
                first["reason"].as_str().unwrap_or("")
            ),
        )
        .token(first["token"].as_str().unwrap_or("").to_owned())
        .action("rerun with --skip-unmappable to list and omit them, or export as native"));
    }
    Ok((file, skipped))
}

/// Collect a document's exportable tokens (library ID grammar, no `stroke`).
pub fn document_tokens(raw: &Value, skip_unmappable: bool) -> Result<(TokenSet, Vec<Value>)> {
    let mut candidates = Map::new();
    let mut skipped = Vec::new();
    for (id, record) in raw
        .get("styles")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let problem = if !tok::valid_library_id(id) {
            Some((
                "invalid-token-id",
                format!("token ID {id:?} does not match the token file ID grammar"),
                "rename it with `pentool style rename`",
            ))
        } else if record.get("type").and_then(Value::as_str) == Some("stroke") {
            Some((
                "unsupported-capability",
                format!("stroke token {id} cannot be exported"),
                "stroke tokens are document-only",
            ))
        } else if !record.get("type").is_some_and(Value::is_string) || record.get("value").is_none()
        {
            Some((
                "invalid-token-value",
                format!("style {id} is not a token (needs type and value)"),
                "fix or delete the style",
            ))
        } else {
            None
        };
        match problem {
            Some((code, message, action)) => {
                if !skip_unmappable {
                    return Err(DesignError::new(code, message)
                        .token(id.clone())
                        .action(format!("{action}, or pass --skip-unmappable")));
                }
                skipped.push(json!({"token": id, "reason": message}));
            }
            None => {
                let mut out = Map::new();
                out.insert("type".into(), record["type"].clone());
                out.insert("value".into(), record["value"].clone());
                if let Some(text) = record.get("description").and_then(Value::as_str) {
                    out.insert("description".into(), json!(text));
                }
                candidates.insert(id.clone(), Value::Object(out));
            }
        }
    }
    // Strict validation; with --skip-unmappable, drop what fails (and what aliases it).
    loop {
        match parse_tokens(&candidates) {
            Ok(set) => return Ok((set, skipped)),
            Err(error) => {
                let bad = error.token.clone().filter(|t| candidates.contains_key(t));
                match (skip_unmappable, bad) {
                    (true, Some(id)) => {
                        candidates.remove(&id);
                        skipped.push(json!({"token": id, "reason": error.message}));
                    }
                    _ => return Err(error),
                }
            }
        }
    }
}

/// Write `file` to `output` (atomically) or return it for printing.
pub fn emit(
    file: Value,
    format: Format,
    count: usize,
    skipped: Vec<Value>,
    output: Option<&Path>,
) -> Result<Value> {
    let Some(path) = output else {
        if !skipped.is_empty() {
            eprintln!("{}", json!({"skipped": skipped}));
        }
        return Ok(file);
    };
    let mut text = serde_json::to_string_pretty(&file).map_err(|e| invalid(e.to_string()))?;
    text.push('\n');
    if path.is_dir() {
        return Err(invalid(format!("{} is a directory", path.display())));
    }
    crate::editing::atomic_write(path, text.as_bytes())
        .map_err(|e| invalid(format!("could not write {}: {e:#}", path.display())))?;
    Ok(json!({
        "ok": true, "format": format.name(), "output": path.to_string_lossy(),
        "tokens": count, "skipped": skipped,
    }))
}

pub fn export_document(
    input: &Path,
    format: Format,
    output: Option<&Path>,
    skip_unmappable: bool,
) -> Result<Value> {
    let (_, raw) = read_document(input)?;
    let (set, mut skipped) = document_tokens(&raw, skip_unmappable)?;
    let (file, more) = render(&set, format, skip_unmappable)?;
    let exported = set.len() - more.len();
    skipped.extend(more);
    skipped.sort_by(|a, b| a["token"].as_str().cmp(&b["token"].as_str()));
    emit(file, format, exported, skipped, output)
}

// ----------------------------------------------------------------- import --

pub fn load_set(file: &Path, format: Option<Format>) -> Result<(TokenSet, Format)> {
    let root = read_file(file)?;
    let detected = match format {
        Some(format) => format,
        None => {
            if root.get("schema").is_some_and(Value::is_number)
                && root.get("tokens").is_some_and(Value::is_object)
            {
                Format::Native
            } else {
                Format::W3c
            }
        }
    };
    let set = match detected {
        Format::Native => parse_native(&root)?,
        Format::W3c => parse_w3c(&root)?,
    };
    Ok((set, detected))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Collision {
    Fail,
    Skip,
    Overwrite,
}

impl Collision {
    pub fn parse(name: &str) -> Result<Collision> {
        match name {
            "fail" => Ok(Collision::Fail),
            "skip" => Ok(Collision::Skip),
            "overwrite" => Ok(Collision::Overwrite),
            other => Err(invalid(format!(
                "unknown --on-collision {other:?}; use fail, skip or overwrite"
            ))),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Collision::Fail => "fail",
            Collision::Skip => "skip",
            Collision::Overwrite => "overwrite",
        }
    }
}

pub struct ImportPlan {
    pub candidate: Value,
    pub rows: Vec<Value>,
    pub counts: Value,
    pub conflicts: Vec<Value>,
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
    pub blocked: bool,
}

/// Pure planner shared by dry run and commit: nothing is written here.
pub fn plan_import(raw: &Value, set: &TokenSet, mode: Collision) -> Result<ImportPlan> {
    let mut candidate = raw.clone();
    let root = candidate
        .as_object_mut()
        .ok_or_else(|| invalid("document root is not an object"))?;
    let styles = root
        .entry("styles")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| invalid("document styles is not an object"))?;
    let (mut added, mut unchanged, mut overwritten, mut skipped_n) =
        (0usize, 0usize, 0usize, 0usize);
    let mut rows = Vec::new();
    let mut conflicts = Vec::new();
    let mut imported = Vec::new();
    let mut skipped = Vec::new();
    let mut writes = Vec::new();
    let record_of = |token: &LibToken| {
        let mut record = Map::new();
        record.insert("type".into(), json!(token.token_type));
        record.insert("value".into(), token.value.clone());
        if !token.description.is_empty() {
            record.insert("description".into(), json!(token.description));
        }
        record
    };
    for (id, token) in set {
        let Some(existing) = styles.get(id).cloned() else {
            styles.insert(id.clone(), Value::Object(record_of(token)));
            added += 1;
            imported.push(id.clone());
            writes.push(id.clone());
            rows.push(json!({"token": id, "type": token.token_type, "action": "added"}));
            continue;
        };
        let local_hash = tok::record_hash(&existing);
        if local_hash.as_deref() == Some(token.hash.as_str()) {
            unchanged += 1;
            rows.push(json!({"token": id, "type": token.token_type, "action": "unchanged"}));
            continue;
        }
        let local = json!({"type": existing.get("type"), "value": existing.get("value")});
        let incoming = json!({"type": token.token_type, "value": token.value});
        let mut row =
            json!({"token": id, "type": token.token_type, "local": local, "incoming": incoming});
        match mode {
            Collision::Fail => {
                conflicts.push(json!({
                    "token": id, "local_hash": local_hash, "incoming_hash": token.hash,
                }));
                row["action"] = json!("conflict");
            }
            Collision::Skip => {
                skipped_n += 1;
                skipped.push(id.clone());
                row["action"] = json!("skipped");
            }
            Collision::Overwrite => {
                let mut record = existing.as_object().cloned().unwrap_or_default();
                for (key, value) in record_of(token) {
                    record.insert(key, value);
                }
                styles.insert(id.clone(), Value::Object(record));
                overwritten += 1;
                imported.push(id.clone());
                writes.push(id.clone());
                row["action"] = json!("overwritten");
            }
        }
        rows.push(row);
    }
    let blocked = !conflicts.is_empty();
    let counts = json!({
        "added": added, "unchanged": unchanged, "overwritten": overwritten,
        "skipped": skipped_n, "conflicts": conflicts.len(),
    });
    if !blocked {
        let usage = tok::Usage::build(&candidate);
        let mut check: Vec<String> = writes.clone();
        for id in &writes {
            check.extend(usage.dependents(id).1);
        }
        check.sort();
        check.dedup();
        tok::check_write(&candidate["styles"], Some(&check))?;
        crate::scene::validate(&candidate).map_err(|e| match e.downcast::<DesignError>() {
            Ok(design) => design,
            Err(other) => invalid(format!("{other:#}")),
        })?;
    }
    imported.sort();
    skipped.sort();
    Ok(ImportPlan {
        candidate: if blocked { raw.clone() } else { candidate },
        rows,
        counts,
        conflicts,
        imported,
        skipped,
        blocked,
    })
}

pub struct ImportOptions<'a> {
    pub input: &'a Path,
    pub file: &'a Path,
    pub format: Option<Format>,
    pub mode: Collision,
    pub dry_run: bool,
    pub if_revision: Option<&'a str>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

pub fn import(options: &ImportOptions) -> Result<Value> {
    let (offset, limit) = page_args(options.offset, options.limit)?;
    let (before, raw) = read_document(options.input)?;
    let revision = transaction::revision(&before);
    if options.if_revision.is_some_and(|wanted| wanted != revision) {
        return Err(DesignError::new(
            "conflict",
            format!(
                "revision mismatch: expected {:?}, current {revision}",
                options.if_revision
            ),
        )
        .action("re-read the document and plan again"));
    }
    let (set, format) = load_set(options.file, options.format)?;
    let plan = plan_import(&raw, &set, options.mode)?;
    let would_change = !plan.blocked && plan.candidate != raw;
    let writes: Vec<&String> = plan.imported.iter().collect();
    let (page, meta) = paginate(plan.rows.clone(), offset, limit);
    let mut report = json!({
        "ok": true, "format": format.name(), "on_collision": options.mode.name(),
        "dry_run": options.dry_run, "would_change": would_change, "blocked": plan.blocked,
        "counts": plan.counts, "tokens": page,
        "imported": plan.imported, "skipped": plan.skipped, "conflicts": plan.conflicts,
        "writes": if plan.blocked { json!([]) } else { json!(writes) },
        "change": {"revision_before": revision, "revision_after": Value::Null},
    });
    for (key, value) in meta.as_object().unwrap() {
        report[key] = value.clone();
    }
    if options.dry_run {
        return Ok(report);
    }
    if plan.blocked {
        let names: Vec<&str> = plan
            .conflicts
            .iter()
            .filter_map(|c| c["token"].as_str())
            .take(10)
            .collect();
        return Err(DesignError::new(
            "conflict",
            format!(
                "{} token(s) already exist with different values: {}",
                plan.conflicts.len(),
                names.join(", ")
            ),
        )
        .action("rerun with --on-collision skip or overwrite, or use --dry-run to inspect; the document was not changed"));
    }
    if !would_change {
        return Ok(report);
    }
    let summary = transaction::commit_value(
        options.input,
        "design-token-import",
        false,
        options.if_revision,
        &plan.candidate,
    )
    .map_err(|e| invalid(format!("{e:#}")).action("the document was left unchanged"))?;
    report["change"] = json!({
        "revision_before": summary.revision_before,
        "revision_after": summary.revision_after,
        "history": summary.history,
    });
    Ok(report)
}

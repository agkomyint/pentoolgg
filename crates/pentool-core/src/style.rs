//! Named v4 design tokens and reference maintenance.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Operation {
    List,
    Set,
    Usage,
    Rename,
    Delete,
    Detach,
    Replace,
}

pub fn apply(
    raw: &mut Value,
    operation: Operation,
    name: Option<&str>,
    token_type: Option<&str>,
    value: Option<&str>,
    to: Option<&str>,
) -> Result<Value> {
    crate::scene::validate(raw)?;
    let compositing = crate::composite::is_document(raw);
    if raw.get("styles").is_none() {
        raw["styles"] = json!({});
    }
    match operation {
        Operation::List => Ok(raw.get("styles").cloned().unwrap_or_else(|| json!({}))),
        Operation::Set => {
            let name = required(name, "style name")?;
            if name.trim().is_empty() {
                bail!("style name cannot be empty")
            }
            let kind = required(token_type, "--type")?;
            if !(matches!(
                kind,
                "color" | "stroke" | "typography" | "spacing" | "radius" | "shadow"
            ) || (compositing
                && matches!(
                    kind,
                    "effects" | "appearance" | "fill" | "adjustment" | "mask" | "operation-stack"
                )))
            {
                bail!("unsupported style type: {kind}")
            }
            let value = required(value, "--value")?;
            let parsed = parse_value(value);
            if kind == "color" {
                if let Some(color) = parsed.as_str() {
                    if !crate::scene::is_valid_color(color) {
                        bail!("style color value {color:?} is not a color; use none, #RGB, #RGBA, #RRGGBB or #RRGGBBAA")
                    }
                }
            }
            // Editing a vendored token keeps its provenance so the edit stays a
            // tracked local override of the library value.
            let source = raw["styles"][name].get("source").cloned();
            raw["styles"][name] = json!({"type":kind,"value":parsed});
            if let Some(source) = source {
                raw["styles"][name]["source"] = source;
            }
            validate_aliases(&raw["styles"])?;
            crate::tokens::check_write(&raw["styles"], Some(&[name.to_owned()]))?;
            Ok(json!({"style":name,"type":kind,"value":parse_value(value)}))
        }
        Operation::Usage => {
            let name = required(name, "style name")?;
            Ok(json!({"style":name,"ids":usage(raw,name)}))
        }
        Operation::Rename => {
            let name = required(name, "style name")?;
            let to = required(to, "--to")?;
            if to.trim().is_empty() {
                bail!("style name cannot be empty")
            }
            let styles = raw["styles"]
                .as_object_mut()
                .context("styles must be an object")?;
            if styles.contains_key(to) {
                bail!("style already exists: {to}")
            }
            let token = styles
                .remove(name)
                .with_context(|| format!("style not found: {name}"))?;
            styles.insert(to.into(), token);
            rewrite_refs(raw, name, Some(to), false);
            Ok(json!({"style":name,"renamed":to}))
        }
        Operation::Delete => {
            let name = required(name, "style name")?;
            let ids = usage(raw, name);
            if !ids.is_empty() {
                bail!("style is still used by: {}", ids.join(", "))
            }
            raw["styles"]
                .as_object_mut()
                .context("styles must be an object")?
                .remove(name)
                .with_context(|| format!("style not found: {name}"))?;
            Ok(json!({"deleted":name}))
        }
        Operation::Detach => {
            let name = required(name, "style name")?;
            let count = rewrite_refs(raw, name, None, true);
            Ok(json!({"style":name,"detached":count}))
        }
        Operation::Replace => {
            let name = required(name, "style name")?;
            let to = required(to, "--to")?;
            if raw["styles"].get(to).is_none() {
                bail!("replacement style not found: {to}")
            }
            let count = rewrite_refs(raw, name, Some(to), false);
            Ok(json!({"style":name,"replacement":to,"replaced":count}))
        }
    }
}

pub(crate) fn validate_aliases(styles: &Value) -> Result<()> {
    crate::tokens::validate_cycles(styles)?;
    Ok(())
}

fn required<'a>(value: Option<&'a str>, label: &str) -> Result<&'a str> {
    value.with_context(|| format!("{label} is required"))
}
fn parse_value(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.into()))
}
fn usage(raw: &Value, name: &str) -> Vec<String> {
    fn visit(value: &Value, name: &str, current: Option<&str>, out: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                let id = object.get("id").and_then(Value::as_str).or(current);
                if object.get("ref").and_then(Value::as_str) == Some(name) {
                    out.push(id.unwrap_or("document").into())
                }
                for (key, child) in object {
                    if key != "styles" {
                        visit(child, name, id, out)
                    }
                }
            }
            Value::Array(values) => {
                for child in values {
                    visit(child, name, current, out)
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    visit(raw, name, None, &mut out);
    out.sort();
    out.dedup();
    out
}
pub(crate) fn rewrite_refs(value: &mut Value, from: &str, to: Option<&str>, detach: bool) -> usize {
    fn visit(value: &mut Value, from: &str, to: Option<&str>, detach: bool, count: &mut usize) {
        match value {
            Value::Object(object) => {
                if object.get("ref").and_then(Value::as_str) == Some(from) {
                    *count += 1;
                    if detach {
                        object.remove("ref");
                    } else if let Some(to) = to {
                        object.insert("ref".into(), json!(to));
                    }
                }
                for (key, child) in object {
                    if key != "styles" {
                        visit(child, from, to, detach, count)
                    }
                }
            }
            Value::Array(values) => {
                for child in values {
                    visit(child, from, to, detach, count)
                }
            }
            _ => {}
        }
    }
    let mut count = 0;
    visit(value, from, to, detach, &mut count);
    count
}

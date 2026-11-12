//! Templates: parameter declarations, validation and `fill`
//! (`docs/design-systems-v1.md` section 7). A fill is planned as a list of
//! `set-property` batch operations and applied through `scene::apply_batch_at`,
//! so dry run and commit share one planner and the template is never touched.
use super::{page_args, paginate};
use crate::{
    editing, resource,
    tokens::{self, DesignError},
    transaction,
};
use anyhow::{Context, Result as AnyResult};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, DesignError>;

pub const MAX_PARAMETERS: usize = 64;
pub const MAX_TARGETS: usize = 16;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_ENUM: usize = 64;
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_LIST_SCAN: usize = 5000;

const TYPES: [&str; 6] = ["text", "number", "bool", "color", "token", "asset"];
const PROPERTIES: [&str; 10] = [
    "content",
    "fill",
    "stroke",
    "visible",
    "stroke_width",
    "opacity",
    "width",
    "height",
    "style_ref",
    "image",
];

fn invalid(message: impl Into<String>) -> DesignError {
    DesignError::new("invalid-input", message)
}

/// One declared parameter.
pub struct Param {
    pub name: String,
    pub kind: String,
    pub token_type: Option<String>,
    pub default: Option<Value>,
    pub targets: Vec<(String, String)>,
    pub constraints: Map<String, Value>,
}

pub struct Declaration {
    pub name: String,
    pub version: String,
    pub params: BTreeMap<String, Param>,
}

pub fn valid_param_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 64
        && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn property_supported(kind: &str, property: &str) -> bool {
    match property {
        "visible" | "style_ref" => true,
        "opacity" => true,
        "content" => kind == "text",
        "fill" => matches!(kind, "rect" | "ellipse" | "path" | "text"),
        "stroke" | "stroke_width" => matches!(kind, "rect" | "ellipse" | "line" | "path"),
        "width" | "height" => matches!(kind, "rect" | "image"),
        "image" => kind == "image",
        _ => false,
    }
}

fn type_property_ok(param: &Param, property: &str) -> bool {
    match param.kind.as_str() {
        "text" => property == "content",
        "number" => matches!(property, "stroke_width" | "opacity" | "width" | "height"),
        "bool" => property == "visible",
        "color" => matches!(property, "fill" | "stroke"),
        "token" => match param.token_type.as_deref() {
            Some("color") => matches!(property, "fill" | "stroke"),
            Some(_) => property == "style_ref",
            None => false,
        },
        "asset" => property == "image",
        _ => false,
    }
}

/// `(layer locked, kind)` of a node, treating a locked ancestor group as locked.
fn locate(page: &Value, id: &str) -> Option<(bool, bool, String)> {
    fn find(nodes: &[Value], id: &str, locked: bool) -> Option<(bool, String)> {
        for node in nodes {
            let here = locked || node.get("locked").and_then(Value::as_bool) == Some(true);
            if node.get("id").and_then(Value::as_str) == Some(id) {
                let kind = node.get("kind").and_then(Value::as_str).unwrap_or("group");
                return Some((here, kind.to_owned()));
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                if let Some(found) = find(children, id, here) {
                    return Some(found);
                }
            }
        }
        None
    }
    for layer in page.get("layers")?.as_array()? {
        let layer_locked = layer.get("locked").and_then(Value::as_bool) == Some(true);
        if let Some((node_locked, kind)) = find(layer.get("nodes")?.as_array()?, id, false) {
            return Some((layer_locked, node_locked, kind));
        }
    }
    None
}

fn select_page<'a>(raw: &'a Value, page: Option<&str>) -> Result<&'a Value> {
    let pages = raw
        .get("pages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("template documents need a version 4 or newer scene"))?;
    match page {
        Some(id) => pages
            .iter()
            .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
            .ok_or_else(|| invalid(format!("page not found: {id}"))),
        None => pages
            .first()
            .ok_or_else(|| invalid("document has no pages")),
    }
}

fn str_len_ok(value: &str) -> bool {
    value.len() <= MAX_TEXT_BYTES && crate::text::validate_content(value).is_ok()
}

/// Problems for `value` against `param` (empty when acceptable).
fn value_problems(param: &Param, value: &Value, raw: &Value, dir: &Path) -> Vec<String> {
    let name = &param.name;
    let mut out = Vec::new();
    match param.kind.as_str() {
        "text" => match value.as_str() {
            Some(s) if str_len_ok(s) => {
                if let Some(max) = param.constraints.get("max_length").and_then(Value::as_u64) {
                    if s.chars().count() as u64 > max {
                        out.push(format!("{name}: text is longer than max_length {max}"));
                    }
                }
            }
            Some(_) => out.push(format!(
                "{name}: text exceeds {MAX_TEXT_BYTES} bytes or has invalid characters"
            )),
            None => out.push(format!("{name}: expected a string")),
        },
        "number" => match value.as_f64().filter(|n| n.is_finite()) {
            Some(n) => {
                for (key, below) in [("min", true), ("max", false)] {
                    if let Some(limit) = param.constraints.get(key).and_then(Value::as_f64) {
                        if (below && n < limit) || (!below && n > limit) {
                            out.push(format!("{name}: {n} violates {key} {limit}"));
                        }
                    }
                }
            }
            None => out.push(format!("{name}: expected a finite number")),
        },
        "bool" => {
            if !value.is_boolean() {
                out.push(format!("{name}: expected true or false"));
            }
        }
        "color" => match value.as_str() {
            Some(s) if tokens::validate_value("color", &json!(s), true).is_ok() => {}
            _ => out.push(format!("{name}: expected a color string such as #rrggbb")),
        },
        "token" => match value.as_str() {
            None => out.push(format!("{name}: expected a token ID string")),
            Some(id) => {
                let styles = raw.get("styles").cloned().unwrap_or(Value::Null);
                let resolved = tokens::resolve(&styles, id);
                if resolved.status != tokens::Status::Resolved {
                    out.push(format!(
                        "{name}: token {id:?} does not resolve in the document"
                    ));
                } else if resolved.token_type.as_deref() != param.token_type.as_deref() {
                    out.push(format!(
                        "{name}: token {id:?} is {}, expected {}",
                        resolved.token_type.unwrap_or_default(),
                        param.token_type.clone().unwrap_or_default()
                    ));
                }
            }
        },
        "asset" => match value.as_str() {
            None => out.push(format!("{name}: expected a relative file path")),
            Some(rel) => match resource::safe_relative_path(Path::new(rel)) {
                Err(e) => out.push(format!("{name}: {e:#}")),
                Ok(path) => match fs::metadata(dir.join(&path)) {
                    Ok(meta) if meta.is_file() && meta.len() <= MAX_ASSET_BYTES => {}
                    Ok(meta) if meta.is_file() => out.push(format!(
                        "{name}: asset {rel:?} is larger than {MAX_ASSET_BYTES} bytes"
                    )),
                    _ => out.push(format!("{name}: asset file {rel:?} was not found")),
                },
            },
        },
        _ => {}
    }
    if let Some(allowed) = param.constraints.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            out.push(format!(
                "{name}: value is not one of the allowed enum values"
            ));
        }
    }
    out
}

/// Parse `design.template`; every problem is collected, not just the first.
pub fn parse(raw: &Value, page: Option<&str>, dir: &Path) -> Result<Declaration> {
    let mut problems: Vec<String> = Vec::new();
    let block = raw
        .get("design")
        .and_then(|d| d.get("template"))
        .ok_or_else(|| {
            invalid("document has no design.template declaration")
                .action("add a design.template block (docs/design-systems-v1.md section 7)")
        })?;
    let object = block
        .as_object()
        .ok_or_else(|| invalid("design.template must be an object"))?;
    for key in object.keys() {
        if ![
            "schema",
            "name",
            "version",
            "parameters",
            "filled_from",
            "values",
        ]
        .contains(&key.as_str())
        {
            problems.push(format!("design.template has unknown key {key:?}"));
        }
    }
    if object.get("schema").and_then(Value::as_u64) != Some(1) {
        problems.push("design.template.schema must be 1".into());
    }
    let text_of = |key: &str, max: usize| -> String {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= max)
            .unwrap_or_default()
            .to_owned()
    };
    let name = text_of("name", 128);
    let version = text_of("version", 64);
    if name.is_empty() {
        problems.push("design.template.name must be a nonempty string of at most 128 bytes".into());
    }
    if version.is_empty() {
        problems.push("design.template.version must be a nonempty string".into());
    }
    let page_value = select_page(raw, page)?;
    let styles = raw.get("styles").cloned().unwrap_or(Value::Null);
    let _ = &styles;
    let mut params = BTreeMap::new();
    match object.get("parameters").and_then(Value::as_object) {
        None => problems.push("design.template.parameters must be an object".into()),
        Some(map) if map.len() > MAX_PARAMETERS => problems.push(format!(
            "design.template declares {} parameters; the limit is {MAX_PARAMETERS}",
            map.len()
        )),
        Some(map) => {
            for (pname, def) in map {
                match parse_param(pname, def, page_value, &mut problems) {
                    Some(param) => {
                        params.insert(pname.clone(), param);
                    }
                    None => continue,
                }
            }
        }
    }
    for param in params.values() {
        if let Some(default) = &param.default {
            for problem in value_problems(param, default, raw, dir) {
                problems.push(format!("default of {problem}"));
            }
        }
    }
    if problems.is_empty() {
        Ok(Declaration {
            name,
            version,
            params,
        })
    } else {
        Err(problems_error("invalid-template", &problems)
            .action("fix the design.template block, then rerun `design template check`"))
    }
}

fn problems_error(code: &'static str, problems: &[String]) -> DesignError {
    DesignError::new(
        code,
        format!("{} problem(s): {}", problems.len(), problems.join("; ")),
    )
}

fn parse_param(
    pname: &str,
    def: &Value,
    page: &Value,
    problems: &mut Vec<String>,
) -> Option<Param> {
    let before = problems.len();
    let mut fail = |message: String| problems.push(format!("{pname}: {message}"));
    if !valid_param_name(pname) {
        fail("name must match ^[a-z][a-z0-9_]*$ and be at most 64 bytes".into());
    }
    let Some(object) = def.as_object() else {
        fail("declaration must be an object".into());
        return None;
    };
    for key in object.keys() {
        if ![
            "type",
            "label",
            "default",
            "targets",
            "token_type",
            "constraints",
        ]
        .contains(&key.as_str())
        {
            fail(format!("unknown key {key:?}"));
        }
    }
    let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
    if !TYPES.contains(&kind) {
        fail(format!("type must be one of {}", TYPES.join(", ")));
    }
    let token_type = object
        .get("token_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if kind == "token" {
        if !token_type
            .as_deref()
            .is_some_and(|t| tokens::TYPES.contains(&t))
        {
            fail(format!(
                "token parameters need token_type from {}",
                tokens::TYPES.join(", ")
            ));
        }
    } else if token_type.is_some() {
        fail("token_type is only valid for token parameters".into());
    }
    let mut constraints = Map::new();
    if let Some(value) = object.get("constraints") {
        match value.as_object() {
            None => fail("constraints must be an object".into()),
            Some(map) => {
                for (key, value) in map {
                    let ok = match (key.as_str(), kind) {
                        ("max_length", "text") => value.as_u64().is_some(),
                        ("min" | "max", "number") => value.as_f64().is_some_and(f64::is_finite),
                        ("required", _) => value.is_boolean(),
                        ("enum", _) => value
                            .as_array()
                            .is_some_and(|a| !a.is_empty() && a.len() <= MAX_ENUM),
                        _ => false,
                    };
                    if ok {
                        constraints.insert(key.clone(), value.clone());
                    } else {
                        fail(format!(
                            "constraint {key:?} is invalid for a {kind} parameter"
                        ));
                    }
                }
            }
        }
    }
    let mut targets = Vec::new();
    match object.get("targets").and_then(Value::as_array) {
        Some(list) if !list.is_empty() && list.len() <= MAX_TARGETS => {
            for (index, target) in list.iter().enumerate() {
                let obj = target.get("object").and_then(Value::as_str);
                let prop = target.get("property").and_then(Value::as_str);
                let (Some(obj), Some(prop)) = (obj, prop) else {
                    fail(format!("targets[{index}] needs string object and property"));
                    continue;
                };
                if !PROPERTIES.contains(&prop) {
                    fail(format!(
                        "targets[{index}] property {prop:?} is not supported"
                    ));
                    continue;
                }
                match locate(page, obj) {
                    None => fail(format!(
                        "targets[{index}] object {obj:?} does not exist on the selected page"
                    )),
                    Some((layer_locked, node_locked, node_kind)) => {
                        if layer_locked || node_locked {
                            fail(format!(
                                "targets[{index}] object {obj:?} is locked; unlock it or retarget"
                            ));
                        }
                        if !property_supported(&node_kind, prop) {
                            fail(format!("targets[{index}] property {prop:?} is not supported on a {node_kind} node"));
                        }
                    }
                }
                targets.push((obj.to_owned(), prop.to_owned()));
            }
        }
        _ => fail(format!("targets must list 1 to {MAX_TARGETS} entries")),
    }
    let param = Param {
        name: pname.to_owned(),
        kind: kind.to_owned(),
        token_type,
        default: object.get("default").cloned(),
        targets,
        constraints,
    };
    for (index, (_, prop)) in param.targets.iter().enumerate() {
        if TYPES.contains(&kind) && !type_property_ok(&param, prop) {
            problems.push(format!(
                "{pname}: a {kind} parameter cannot drive property {prop:?} (targets[{index}])"
            ));
        }
    }
    (problems.len() == before).then_some(param)
}

fn read_template(path: &Path) -> Result<(Vec<u8>, Value)> {
    let bytes =
        fs::read(path).map_err(|e| invalid(format!("could not read {}: {e}", path.display())))?;
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|e| invalid(format!("{} is not valid JSON: {e}", path.display())))?;
    if raw.get("version").and_then(Value::as_u64).unwrap_or(0) < 4 {
        return Err(DesignError::new(
            "unsupported-capability",
            "templates need a version 4 or newer scene document",
        )
        .action("migrate the document with `pentool migrate`"));
    }
    crate::scene::validate(&raw).map_err(|e| match e.downcast::<DesignError>() {
        Ok(design) => design,
        Err(other) => invalid(format!("{other:#}")),
    })?;
    Ok((bytes, raw))
}

fn dir_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// `design template check`.
pub fn check(input: &Path, page: Option<&str>) -> Result<Value> {
    let (bytes, raw) = read_template(input)?;
    let decl = parse(&raw, page, &dir_of(input))?;
    Ok(report_header(&decl, &bytes, input, true))
}

fn report_header(decl: &Declaration, bytes: &[u8], input: &Path, ok: bool) -> Value {
    let params: Vec<Value> = decl
        .params
        .values()
        .map(|p| {
            json!({"name": p.name, "type": p.kind,
                   "required": p.constraints.get("required") == Some(&json!(true)) && p.default.is_none(),
                   "targets": p.targets.len()})
        })
        .collect();
    json!({
        "ok": ok, "template": input.to_string_lossy(),
        "name": decl.name, "version": decl.version,
        "content_hash": resource::sha256(bytes),
        "parameter_count": decl.params.len(), "parameters": params,
    })
}

/// Most rows a `--data` batch fill accepts.
pub const MAX_FILL_ROWS: usize = 10_000;
/// Largest `--data` file read.
pub const MAX_DATA_BYTES: u64 = 64 * 1024 * 1024;
/// Most individual problems listed by a failed batch fill.
const MAX_LISTED_PROBLEMS: usize = 100;

pub struct FillOptions<'a> {
    pub input: &'a Path,
    pub values: Option<&'a Path>,
    pub output: Option<&'a Path>,
    pub data: Option<&'a Path>,
    pub output_dir: Option<&'a Path>,
    pub page: Option<&'a str>,
    pub dry_run: bool,
    pub if_revision: Option<&'a str>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// The single planner behind dry run and commit.
struct Planned {
    candidate: Value,
    changes: Vec<Value>,
    applied: BTreeMap<String, Value>,
    header: Value,
    revision: String,
}

/// A parsed, validated template shared by every row of a fill.
struct Loaded {
    bytes: Vec<u8>,
    raw: Value,
    revision: String,
    dir: PathBuf,
    decl: Declaration,
}

fn load(opts: &FillOptions) -> Result<Loaded> {
    let (bytes, raw) = read_template(opts.input)?;
    let revision = transaction::revision(&bytes);
    if opts.if_revision.is_some_and(|wanted| wanted != revision) {
        return Err(DesignError::new(
            "conflict",
            format!(
                "revision mismatch: expected {:?}, current {revision}",
                opts.if_revision
            ),
        )
        .action("re-read the template and plan again"));
    }
    let dir = dir_of(opts.input);
    let decl = parse(&raw, opts.page, &dir)?;
    Ok(Loaded {
        bytes,
        raw,
        revision,
        dir,
        decl,
    })
}

/// Apply defaults and constraints to one row of values; every problem is listed.
fn accept(ctx: &Loaded, values: &Map<String, Value>) -> (BTreeMap<String, Value>, Vec<String>) {
    let mut problems = Vec::new();
    for key in values.keys() {
        if !ctx.decl.params.contains_key(key) {
            problems.push(format!("{key}: unknown parameter"));
        }
    }
    let mut accepted: BTreeMap<String, Value> = BTreeMap::new();
    for (name, param) in &ctx.decl.params {
        let required = param.constraints.get("required") == Some(&json!(true));
        match values.get(name).or(param.default.as_ref()) {
            Some(value) => {
                let found = value_problems(param, value, &ctx.raw, &ctx.dir);
                if found.is_empty() {
                    accepted.insert(name.clone(), value.clone());
                }
                problems.extend(found);
            }
            None if required => problems.push(format!("{name}: required value is missing")),
            None => {}
        }
    }
    (accepted, problems)
}

fn build(ctx: &Loaded, opts: &FillOptions, accepted: BTreeMap<String, Value>) -> Result<Planned> {
    let mut operations = Vec::new();
    let mut applied = BTreeMap::new();
    for (name, value) in &accepted {
        let param = &ctx.decl.params[name];
        for (object, property) in &param.targets {
            operations.push(
                json!({"type":"set-property","id":object,"property":property,
                                   "value":value,"parameter":name}),
            );
        }
        applied.insert(
            name.clone(),
            json!({"applied": value,
                   "targets": param.targets.iter()
                       .map(|(o, p)| json!({"object": o, "property": p})).collect::<Vec<_>>()}),
        );
    }
    let mut candidate = ctx.raw.clone();
    let changes = if operations.is_empty() {
        Vec::new()
    } else {
        crate::scene::apply_batch_at(&mut candidate, opts.page, &operations, opts.input)
            .map_err(|e| invalid(format!("{e:#}")).action("the template was not changed"))?
    };
    let content_hash = resource::sha256(&ctx.bytes);
    let template = &mut candidate["design"]["template"];
    template["filled_from"] =
        json!({"name": ctx.decl.name, "version": ctx.decl.version, "content_hash": content_hash});
    template["values"] = json!(accepted);
    crate::scene::validate(&candidate).map_err(|e| match e.downcast::<DesignError>() {
        Ok(design) => design,
        Err(other) => invalid(format!("{other:#}")),
    })?;
    let header =
        json!({"name": ctx.decl.name, "version": ctx.decl.version, "content_hash": content_hash});
    Ok(Planned {
        candidate,
        changes,
        applied,
        header,
        revision: ctx.revision.clone(),
    })
}

fn plan(opts: &FillOptions, values_path: &Path) -> Result<Planned> {
    let ctx = load(opts)?;
    let value_bytes = fs::read(values_path)
        .map_err(|e| invalid(format!("could not read {}: {e}", values_path.display())))?;
    let values: Value = serde_json::from_slice(&value_bytes)
        .map_err(|e| invalid(format!("{} is not valid JSON: {e}", values_path.display())))?;
    let values = values
        .as_object()
        .ok_or_else(|| invalid("values must be a JSON object of {parameter: value}"))?;
    let (accepted, problems) = accept(&ctx, values);
    if !problems.is_empty() {
        return Err(problems_error("invalid-input", &problems)
            .action("fix values.json; every problem is listed, the template was not changed"));
    }
    build(&ctx, opts, accepted)
}

/// `design template fill`.
pub fn fill(opts: &FillOptions) -> Result<Value> {
    if opts.data.is_some() {
        if opts.values.is_some() || opts.output.is_some() || opts.output_dir.is_none() {
            return Err(invalid(
                "--data fills rows into --output-dir and excludes --values/--output",
            )
            .action("use either --values with --output, or --data with --output-dir"));
        }
        return fill_rows(opts);
    }
    let (Some(values), Some(output), None) = (opts.values, opts.output, opts.output_dir) else {
        return Err(
            invalid("fill needs --values and --output (or --data and --output-dir)")
                .action("see `pentool design template fill --help`"),
        );
    };
    let (offset, limit) = page_args(opts.offset, opts.limit)?;
    if same_file(opts.input, output) {
        return Err(invalid("--output must not be the template itself")
            .action("choose a new output path; in-place fill is not offered"));
    }
    let planned = plan(opts, values)?;
    let out_bytes = serde_json::to_vec_pretty(&planned.candidate)
        .map_err(|e| invalid(format!("could not serialize the output: {e}")))?;
    let would_change = fs::read(output)
        .map(|existing| existing != out_bytes)
        .unwrap_or(true);
    let (page, meta) = paginate(planned.changes.clone(), offset, limit);
    let mut report = json!({
        "ok": true, "dry_run": opts.dry_run,
        "output": output.to_string_lossy(),
        "would_change": would_change,
        "template": planned.header,
        "parameters": planned.applied,
        "changes": page,
        "revision_before": planned.revision,
        "output_revision": transaction::revision(&out_bytes),
    });
    for (key, value) in meta.as_object().unwrap() {
        report[key] = value.clone();
    }
    if opts.dry_run {
        return Ok(report);
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.is_dir() {
            return Err(invalid(format!(
                "output directory {} does not exist",
                parent.display()
            )));
        }
    }
    editing::atomic_write(output, &out_bytes)
        .map_err(|e| invalid(format!("{e:#}")).action("the template was not changed"))?;
    Ok(report)
}

/// Parse a CSV table (RFC 4180 quoting) into rows of cells.
fn parse_csv(text: &str) -> std::result::Result<Vec<Vec<String>>, String> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut pending = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        pending = true;
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    cell.push('"');
                }
                '"' => quoted = false,
                other => cell.push(other),
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => quoted = true,
            '"' => return Err("a quote inside an unquoted cell".into()),
            ',' => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                pending = false;
                if rows.len() > MAX_FILL_ROWS + 1 {
                    return Ok(rows);
                }
            }
            other => cell.push(other),
        }
    }
    if quoted {
        return Err("unterminated quoted cell".into());
    }
    if pending {
        row.push(cell);
        rows.push(row);
    }
    Ok(rows)
}

/// Read `--data` into row objects (JSON array of objects, or CSV with a header row
/// of parameter names; CSV cells are coerced by parameter type, empty = default).
fn read_rows(path: &Path, ctx: &Loaded) -> Result<Vec<Map<String, Value>>> {
    let meta = fs::metadata(path)
        .map_err(|e| invalid(format!("could not read {}: {e}", path.display())))?;
    if meta.len() > MAX_DATA_BYTES {
        return Err(DesignError::new(
            "limit-exceeded",
            format!(
                "--data is {} bytes; the limit is {MAX_DATA_BYTES}",
                meta.len()
            ),
        )
        .action("split the data file"));
    }
    let bytes = fs::read(path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    let too_many = |count: usize| {
        DesignError::new(
            "limit-exceeded",
            format!("--data has {count} rows; the limit is {MAX_FILL_ROWS}"),
        )
        .action("split the data into files of at most 10,000 rows")
    };
    let csv = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    if !csv {
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|e| invalid(format!("{} is not valid JSON: {e}", path.display())))?;
        let rows = value
            .as_array()
            .ok_or_else(|| invalid("--data must be a JSON array of {parameter: value} objects"))?;
        if rows.len() > MAX_FILL_ROWS {
            return Err(too_many(rows.len()));
        }
        return rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                row.as_object()
                    .cloned()
                    .ok_or_else(|| invalid(format!("row {}: must be a JSON object", index + 1)))
            })
            .collect();
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| invalid(format!("{} is not valid UTF-8", path.display())))?;
    let table = parse_csv(&text).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    let Some((header, body)) = table.split_first() else {
        return Ok(Vec::new());
    };
    let body: Vec<&Vec<String>> = body
        .iter()
        .filter(|cells| !(cells.len() == 1 && cells[0].is_empty()))
        .collect();
    if body.len() > MAX_FILL_ROWS {
        return Err(too_many(body.len()));
    }
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for (index, cells) in body.iter().enumerate() {
        let number = index + 1;
        if cells.len() != header.len() {
            problems.push(format!(
                "row {number}: has {} cells, the header has {}",
                cells.len(),
                header.len()
            ));
            continue;
        }
        let mut object = Map::new();
        for (name, cell) in header.iter().zip(cells.iter()) {
            if cell.is_empty() {
                continue;
            }
            let kind = ctx.decl.params.get(name).map(|p| p.kind.as_str());
            let value = match kind {
                Some("number") => match cell.trim().parse::<f64>() {
                    Ok(n) if n.is_finite() => json!(n),
                    _ => {
                        problems.push(format!("row {number}: {name}: {cell:?} is not a number"));
                        continue;
                    }
                },
                Some("bool") => match cell.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" => json!(true),
                    "false" | "0" => json!(false),
                    _ => {
                        problems.push(format!("row {number}: {name}: {cell:?} is not true/false"));
                        continue;
                    }
                },
                _ => json!(cell),
            };
            object.insert(name.clone(), value);
        }
        out.push(object);
        if problems.len() >= MAX_LISTED_PROBLEMS {
            break;
        }
    }
    if !problems.is_empty() {
        return Err(problems_error("invalid-input", &problems)
            .action("fix the CSV cells; nothing was written"));
    }
    Ok(out)
}

fn row_path(dir: &Path, template: &Path, index: usize) -> PathBuf {
    let stem = template
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("filled");
    dir.join(format!("{stem}-{:05}.pen", index + 1))
}

/// `design template fill --data`: one output document per row, all validated
/// before the first byte is written; the template is never modified.
fn fill_rows(opts: &FillOptions) -> Result<Value> {
    let (offset, limit) = page_args(opts.offset, opts.limit)?;
    let (data, dir) = (opts.data.unwrap(), opts.output_dir.unwrap());
    let ctx = load(opts)?;
    let rows = read_rows(data, &ctx)?;
    if rows.is_empty() {
        return Err(invalid("--data has no rows").action("add at least one row"));
    }
    if dir.exists() && !dir.is_dir() {
        return Err(invalid(format!("{} is not a directory", dir.display())));
    }
    // Pass 1: validate every row (all problems are listed, capped).
    let mut problems = Vec::new();
    let mut total_problems = 0;
    for (index, row) in rows.iter().enumerate() {
        let (_, found) = accept(&ctx, row);
        total_problems += found.len();
        for text in found {
            if problems.len() < MAX_LISTED_PROBLEMS {
                problems.push(format!("row {}: {text}", index + 1));
            }
        }
    }
    if total_problems > 0 {
        let mut error = problems_error("invalid-input", &problems);
        if total_problems > problems.len() {
            error
                .message
                .push_str(&format!("; and {} more", total_problems - problems.len()));
        }
        return Err(
            error.action("fix the data rows; every row was checked and nothing was written")
        );
    }
    // Plan every row, keeping only a revision per output (bounded memory).
    let mut report_rows = Vec::new();
    let mut conflicts = Vec::new();
    let mut would_change = false;
    let mut header = Value::Null;
    for (index, row) in rows.iter().enumerate() {
        let (accepted, _) = accept(&ctx, row);
        let planned = build(&ctx, opts, accepted).map_err(|mut e| {
            e.message = format!("row {}: {}", index + 1, e.message);
            e
        })?;
        let bytes = serde_json::to_vec_pretty(&planned.candidate)
            .map_err(|e| invalid(format!("could not serialize row {}: {e}", index + 1)))?;
        let path = row_path(dir, opts.input, index);
        if same_file(opts.input, &path) {
            return Err(invalid("an output file would be the template itself")
                .action("choose a different --output-dir"));
        }
        let existing = fs::read(&path).ok();
        let changes = existing.as_deref() != Some(bytes.as_slice());
        would_change |= changes;
        if existing.is_some() && changes {
            conflicts.push(path.to_string_lossy().into_owned());
        }
        if index == 0 {
            header = planned.header.clone();
        }
        report_rows.push(json!({
            "row": index + 1, "output": path.to_string_lossy(),
            "output_revision": transaction::revision(&bytes),
            "changes": planned.changes.len(), "would_change": changes,
        }));
    }
    if !conflicts.is_empty() {
        return Err(DesignError::new(
            "conflict",
            format!(
                "{} output file(s) already exist with different content: {}",
                conflicts.len(),
                conflicts
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .action("choose an empty --output-dir; existing files are never overwritten"));
    }
    let (page, meta) = paginate(report_rows, offset, limit);
    let mut report = json!({
        "ok": true, "dry_run": opts.dry_run, "rows": rows.len(),
        "output_dir": dir.to_string_lossy(), "would_change": would_change,
        "template": header, "outputs": page, "revision_before": ctx.revision,
    });
    for (key, value) in meta.as_object().unwrap() {
        report[key] = value.clone();
    }
    if opts.dry_run {
        return Ok(report);
    }
    fs::create_dir_all(dir)
        .map_err(|e| invalid(format!("could not create {}: {e}", dir.display())))?;
    // Regenerate each document and write it atomically; undo this run on failure.
    let mut written: Vec<PathBuf> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let path = row_path(dir, opts.input, index);
        let result = (|| -> Result<bool> {
            let (accepted, _) = accept(&ctx, row);
            let planned = build(&ctx, opts, accepted)?;
            let bytes = serde_json::to_vec_pretty(&planned.candidate)
                .map_err(|e| invalid(e.to_string()))?;
            if fs::read(&path).ok().as_deref() == Some(bytes.as_slice()) {
                return Ok(false);
            }
            editing::atomic_write(&path, &bytes).map_err(|e| invalid(format!("{e:#}")))?;
            Ok(true)
        })();
        match result {
            Ok(true) => written.push(path),
            Ok(false) => {}
            Err(error) => {
                for done in &written {
                    let _ = fs::remove_file(done);
                }
                return Err(error.action(format!(
                    "row {} failed; files written by this run were removed",
                    index + 1
                )));
            }
        }
    }
    Ok(report)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => std::path::absolute(a).ok() == std::path::absolute(b).ok(),
    }
}

/// `design template list DIR`: templates (`*.pen` with design.template), sorted, paginated.
pub fn list(dir: &Path, offset: Option<usize>, limit: Option<usize>) -> Result<Value> {
    let (offset, limit) = page_args(offset, limit)?;
    let entries = fs::read_dir(dir)
        .map_err(|e| invalid(format!("could not read directory {}: {e}", dir.display())))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "pen") && p.is_file())
        .collect();
    files.sort();
    if files.len() > MAX_LIST_SCAN {
        return Err(invalid(format!(
            "directory holds more than {MAX_LIST_SCAN} .pen files"
        )));
    }
    let mut rows = Vec::new();
    for path in files {
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(raw) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(block) = raw.get("design").and_then(|d| d.get("template")) else {
            continue;
        };
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        rows.push(json!({
            "file": file,
            "name": block.get("name"), "version": block.get("version"),
            "parameters": block.get("parameters").and_then(Value::as_object).map_or(0, |m| m.len()),
            "content_hash": resource::sha256(&bytes),
        }));
    }
    let (page, meta) = paginate(rows, offset, limit);
    let mut report = json!({"directory": dir.to_string_lossy(), "templates": page});
    for (key, value) in meta.as_object().unwrap() {
        report[key] = value.clone();
    }
    Ok(report)
}

/// Batch operation `set-property`: `{id, property, value}` applied to one node.
/// Refuses locked layers or groups and unsupported kind/property pairs.
pub fn batch_set_property(
    raw: &mut Value,
    page: Option<&str>,
    document: &Path,
    id: &str,
    operation: &Value,
) -> AnyResult<Value> {
    let property = operation
        .get("property")
        .and_then(Value::as_str)
        .context("property is missing")?;
    let value = operation.get("value").context("value is missing")?.clone();
    let (layer_locked, node_locked, kind) = {
        let page_ref = select_page(raw, page).map_err(|e| anyhow::anyhow!("{e}"))?;
        locate(page_ref, id).with_context(|| format!("object not found: {id}"))?
    };
    if layer_locked || node_locked {
        anyhow::bail!("[locked-node] object {id} is locked")
    }
    if !PROPERTIES.contains(&property) || !property_supported(&kind, property) {
        anyhow::bail!("property {property} is not supported on a {kind} node")
    }
    if property == "opacity" && !crate::composite::is_document(raw) {
        anyhow::bail!("[unsupported-capability] opacity needs `migrate --target 6`")
    }
    let styles = raw.get("styles").cloned().unwrap_or(Value::Null);
    let image = if property == "image" {
        Some(load_image(raw, document, &value)?)
    } else {
        None
    };
    if let Some((digest, entry)) = &image {
        raw["image_assets"][digest.as_str()] = entry.clone();
    }
    let page_value = crate::scene::page_mut(raw, page)?;
    let node = crate::scene::find_node_mut(page_value, id).context("object not found")?;
    match property {
        "content" => {
            let text = value.as_str().context("content must be a string")?;
            crate::text::validate_content(text)?;
            node["content"] = json!(text);
        }
        "fill" | "stroke" => {
            let entry = style_slot(node, property);
            match value.as_str() {
                Some(token)
                    if tokens::resolve(&styles, token).status == tokens::Status::Resolved =>
                {
                    let fallback = tokens::resolve(&styles, token)
                        .value
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .or_else(|| {
                            entry
                                .get("fallback")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_else(|| "#000000".into());
                    *entry = json!({"ref": token, "fallback": fallback});
                }
                Some(color) => *entry = json!({"fallback": color}),
                None => anyhow::bail!("{property} must be a color string or token ID"),
            }
        }
        "visible" => {
            node["visible"] = json!(value.as_bool().context("visible must be a boolean")?)
        }
        "stroke_width" => {
            let n = value.as_f64().filter(|n| n.is_finite() && *n >= 0.0);
            let n = n.context("stroke_width must be a finite number >= 0")?;
            *style_slot(node, property) = json!({"fallback": n});
        }
        "opacity" | "width" | "height" => {
            let n = value.as_f64().filter(|n| n.is_finite() && *n >= 0.0);
            node[property] =
                json!(n.with_context(|| format!("{property} must be a finite number >= 0"))?);
        }
        "style_ref" => {
            node["style_ref"] = json!(value.as_str().context("style_ref must be a token ID")?)
        }
        "image" => node["asset"] = json!(image.map(|(digest, _)| digest).unwrap_or_default()),
        _ => unreachable!(),
    }
    Ok(json!({"type":"set-property","id":id,"property":property}))
}

/// Batch operation `bind-token`: `{id, property: fill|stroke, token}` points the
/// node's paint at a resolved `color` token, keeping the token's current value as
/// the fallback. Refuses locked layers or groups and unresolved tokens.
pub fn batch_bind_token(
    raw: &mut Value,
    page: Option<&str>,
    id: &str,
    operation: &Value,
) -> AnyResult<Value> {
    let property = operation
        .get("property")
        .and_then(Value::as_str)
        .context("property is missing")?;
    let token = operation
        .get("token")
        .and_then(Value::as_str)
        .context("token is missing")?;
    if !matches!(property, "fill" | "stroke") {
        anyhow::bail!("bind-token supports the fill and stroke properties, not {property:?}")
    }
    let (layer_locked, node_locked, kind) = {
        let page_ref = select_page(raw, page).map_err(|e| anyhow::anyhow!("{e}"))?;
        locate(page_ref, id).with_context(|| format!("object not found: {id}"))?
    };
    if layer_locked || node_locked {
        anyhow::bail!("[locked-node] object {id} is locked")
    }
    if !property_supported(&kind, property) {
        anyhow::bail!("property {property} is not supported on a {kind} node")
    }
    let styles = raw.get("styles").cloned().unwrap_or(Value::Null);
    let resolution = tokens::resolve(&styles, token);
    if resolution.status != tokens::Status::Resolved {
        anyhow::bail!(
            "[{}-token] token {token:?} is {}: {}",
            resolution.status.as_str(),
            resolution.status.as_str(),
            resolution.reason.unwrap_or_default()
        )
    }
    if resolution.token_type.as_deref() != Some("color") {
        anyhow::bail!("[token-type-mismatch] token {token:?} is not a color token")
    }
    let fallback = resolution
        .value
        .and_then(|v| v.as_str().map(str::to_owned))
        .with_context(|| {
            format!("token {token:?} has a native color value; bind needs a hex color")
        })?;
    let page_value = crate::scene::page_mut(raw, page)?;
    let node = crate::scene::find_node_mut(page_value, id).context("object not found")?;
    *style_slot(node, property) = json!({"ref": token, "fallback": fallback});
    Ok(json!({"type":"bind-token","id":id,"property":property,"token":token}))
}

/// Batch operation `set-token`: `{name, token_type, value}` with a typed JSON
/// value (`set-style` stays string-valued). Keeps unknown record keys such as
/// `source`, and validates cycles, depth and the value schema before accepting.
pub fn batch_set_token(raw: &mut Value, operation: &Value) -> AnyResult<Value> {
    let name = operation
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty() && n.len() <= 128)
        .context("name must be a token ID of 1 to 128 bytes")?;
    let value = operation.get("value").context("value is missing")?.clone();
    let existing = raw
        .get("styles")
        .and_then(|s| s.get(name))
        .and_then(|r| r.get("type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let token_type = operation
        .get("token_type")
        .or_else(|| operation.get("style_type"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(existing)
        .context("token_type is missing for a new token")?;
    if !matches!(
        token_type.as_str(),
        "color" | "stroke" | "typography" | "spacing" | "radius" | "shadow"
    ) {
        anyhow::bail!("unknown token type {token_type:?}")
    }
    if !raw.get("styles").is_some_and(Value::is_object) {
        raw["styles"] = json!({});
    }
    let styles = raw["styles"].as_object_mut().unwrap();
    let record = styles
        .entry(name.to_owned())
        .or_insert_with(|| json!({"type": token_type, "value": Value::Null}));
    if !record.is_object() {
        *record = json!({"type": token_type, "value": Value::Null});
    }
    record["type"] = json!(token_type);
    record["value"] = value;
    tokens::check_write(&raw["styles"], Some(&[name.to_owned()]))
        .map_err(|e| anyhow::anyhow!("[{}] {}", e.code, e.message))?;
    Ok(json!({"type":"set-token","name":name,"token_type":token_type}))
}

fn style_slot<'a>(node: &'a mut Value, key: &str) -> &'a mut Value {
    if !node.get("style").is_some_and(Value::is_object) {
        node["style"] = json!({});
    }
    node["style"]
        .as_object_mut()
        .unwrap()
        .entry(key.to_owned())
        .or_insert_with(|| json!({"fallback": "none"}))
}

/// Read a document-relative image and return its digest and `image_assets` entry.
fn load_image(raw: &Value, document: &Path, value: &Value) -> AnyResult<(String, Value)> {
    if !crate::composite::is_document(raw) {
        anyhow::bail!("[unsupported-capability] image assets need `migrate --target 6`")
    }
    let relative = resource::safe_relative_path(Path::new(
        value.as_str().context("image must be a relative path")?,
    ))?;
    let path = dir_of(document).join(relative);
    let meta = fs::metadata(&path).with_context(|| format!("could not read {}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_ASSET_BYTES {
        anyhow::bail!(
            "[limit-exceeded] image {} is not a file of at most {MAX_ASSET_BYTES} bytes",
            path.display()
        )
    }
    let bytes = fs::read(&path)?;
    let source = crate::image::decode_source(&bytes)?;
    let entry = json!({
        "media_type": source.media_type.clone(),
        "byte_length": source.byte_length,
        "pixel_width": source.pixel_width,
        "pixel_height": source.pixel_height,
        "color_space": source.color_space,
        "orientation": source.orientation,
        "storage": crate::image::embedded_storage(&bytes),
    });
    Ok((source.digest.clone(), entry))
}

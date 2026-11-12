//! v0.14.0 task 6: layout styles (paragraph, character, object, table, cell).
//!
//! Styles live in `layout.styles.<kind>.<id>`. A style may be `based_on`
//! another of the same kind (depth <= 16, no cycles); paragraph styles also
//! name a `next` style. Resolution is deterministic: ancestors first, then the
//! style itself. Every mutation validates the whole document in memory before
//! anything is written by the shared transaction path.
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const KINDS: [&str; 5] = ["paragraph", "character", "object", "table", "cell"];
pub const MAX_DEPTH: usize = 16;
pub const MAX_STYLES: usize = 2000;
const MAX_ID_LEN: usize = 128;
const MAX_REPORT: usize = 200;

const PARAGRAPH_KEYS: &[&str] = &[
    "font",
    "weight",
    "italic",
    "axes",
    "size",
    "leading",
    "align",
    "left_indent",
    "right_indent",
    "first_indent",
    "last_indent",
    "space_before",
    "space_after",
    "same_style_space",
    "tabs",
    "drop_cap",
    "hyphenate",
    "hyphenation",
    "word_spacing",
    "letter_spacing",
    "glyph_scaling",
    "composer",
    "keep",
    "span_columns",
    "rule_above",
    "rule_below",
    "list",
    "align_to_grid",
    "language",
    "optical_margin",
    "balance",
    "hanging_punctuation",
    "character_style",
    "nested",
    "line_styles",
    "grep",
];
const CHARACTER_KEYS: &[&str] = &[
    "font",
    "weight",
    "italic",
    "axes",
    "size",
    "leading",
    "tracking",
    "kerning",
    "baseline_shift",
    "h_scale",
    "v_scale",
    "skew",
    "case",
    "position",
    "features",
    "fill",
    "stroke",
    "underline",
    "strikethrough",
    "no_break",
    "language",
    "condition",
];
const OBJECT_KEYS: &[&str] = &[
    "fill",
    "stroke",
    "effects",
    "frame_options",
    "wrap",
    "anchored",
    "fit",
    "paragraph_style",
    "opacity",
];
const TABLE_KEYS: &[&str] = &[
    "borders",
    "row_pattern",
    "column_pattern",
    "header_rows",
    "footer_rows",
    "space_before",
    "space_after",
];
const CELL_KEYS: &[&str] = &[
    "inset",
    "vertical_align",
    "rotation",
    "fill",
    "stroke",
    "paragraph_style",
];
const ALIGNS: &[&str] = &[
    "left",
    "center",
    "right",
    "justify",
    "justify-last-left",
    "justify-last-center",
    "justify-last-right",
    "justify-last-all",
    "towards-spine",
    "away-from-spine",
];

fn allowed(kind: &str) -> &'static [&'static str] {
    match kind {
        "paragraph" => PARAGRAPH_KEYS,
        "character" => CHARACTER_KEYS,
        "object" => OBJECT_KEYS,
        "table" => TABLE_KEYS,
        _ => CELL_KEYS,
    }
}

fn check_kind(kind: &str) -> Result<()> {
    ensure!(
        KINDS.contains(&kind),
        "[invalid-input] style kind `{kind}` must be one of {}",
        KINDS.join(", ")
    );
    Ok(())
}

fn check_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.chars().count() <= MAX_ID_LEN
            && !id.chars().any(|c| c.is_control() || c == '/' || c == '\\'),
        "[invalid-input] style ID {id:?} must be 1-{MAX_ID_LEN} characters without control characters, '/' or '\\'"
    );
    Ok(())
}

fn map<'a>(raw: &'a Value, kind: &str) -> Option<&'a Map<String, Value>> {
    raw.pointer(&format!("/layout/styles/{kind}"))
        .and_then(Value::as_object)
}

fn map_mut<'a>(raw: &'a mut Value, kind: &str) -> Result<&'a mut Map<String, Value>> {
    let layout = raw
        .get_mut("layout")
        .and_then(Value::as_object_mut)
        .context("[unsupported-version] styles need a version 9 document; run `pentool migrate --target 9` first")?;
    let styles = layout
        .entry("styles")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("[invalid-input] layout.styles must be an object")?;
    styles
        .entry(kind)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("[invalid-input] layout.styles.{kind} must be an object"))
}

fn require_v9(raw: &Value) -> Result<()> {
    ensure!(
        raw.get("layout").is_some_and(Value::is_object),
        "[unsupported-version] styles need a version 9 document; run `pentool migrate --target 9` first"
    );
    Ok(())
}

// ---------------------------------------------------------------- validation

fn number(v: &Value, what: &str, min: f64, max: f64, exclusive: bool) -> Result<f64> {
    let n = v
        .as_f64()
        .filter(|n| n.is_finite())
        .with_context(|| format!("[invalid-input] {what} must be a number"))?;
    ensure!(
        n <= max && if exclusive { n > min } else { n >= min },
        "[invalid-input] {what} must be {} {min} and at most {max}",
        if exclusive {
            "greater than"
        } else {
            "at least"
        }
    );
    Ok(n)
}

fn text(v: &Value, what: &str, max: usize) -> Result<()> {
    let s = v
        .as_str()
        .with_context(|| format!("[invalid-input] {what} must be a string"))?;
    ensure!(
        s.chars().count() <= max && !s.chars().any(char::is_control),
        "[invalid-input] {what} must be at most {max} characters without control characters"
    );
    Ok(())
}

fn boolean(v: &Value, what: &str) -> Result<()> {
    ensure!(
        v.is_boolean(),
        "[invalid-input] {what} must be true or false"
    );
    Ok(())
}

fn triple(v: &Value, what: &str) -> Result<()> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .with_context(|| format!("[invalid-input] {what} must be [min, desired, max]"))?;
    let n: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
    ensure!(
        n.len() == 3 && n.iter().all(|x| x.is_finite()) && n[0] <= n[1] && n[1] <= n[2],
        "[invalid-input] {what} must be three numbers with min <= desired <= max"
    );
    ensure!(
        n[0] >= 1.0 || what == "letter_spacing",
        "[invalid-input] {what} minimum must be at least 1"
    );
    Ok(())
}

fn one_of(v: &Value, what: &str, options: &[&str]) -> Result<()> {
    let s = v.as_str().unwrap_or_default();
    ensure!(
        options.contains(&s),
        "[invalid-input] {what} `{}` must be one of {}",
        v.as_str().unwrap_or("(not a string)"),
        options.join(", ")
    );
    Ok(())
}

fn validate_property(kind: &str, key: &str, v: &Value) -> Result<()> {
    let what = format!("{kind} style property `{key}`");
    match key {
        "font" | "language" | "fill" | "stroke" if v.is_string() => text(v, &what, 256),
        "font" | "language" => text(v, &what, 256),
        "weight" => number(v, &what, 1.0, 1000.0, false).map(|_| ()),
        "italic"
        | "hyphenate"
        | "optical_margin"
        | "balance"
        | "hanging_punctuation"
        | "no_break"
        | "underline"
        | "strikethrough" => boolean(v, &what),
        "size" | "h_scale" | "v_scale" => number(v, &what, 0.0, 10000.0, true).map(|_| ()),
        "leading" => {
            if v == "auto" {
                Ok(())
            } else {
                number(v, &what, 0.0, 10000.0, true).map(|_| ())
            }
        }
        "tracking" | "baseline_shift" | "skew" | "left_indent" | "right_indent"
        | "first_indent" | "last_indent" | "space_before" | "space_after" | "same_style_space" => {
            number(v, &what, -100_000.0, 100_000.0, false).map(|_| ())
        }
        "align" => one_of(v, &what, ALIGNS),
        "composer" => one_of(v, &what, &["single-line", "paragraph"]),
        "align_to_grid" => one_of(v, &what, &["none", "first-line", "all-lines"]),
        "case" => one_of(v, &what, &["normal", "all-caps", "small-caps"]),
        "position" => one_of(
            v,
            &what,
            &[
                "normal",
                "superscript",
                "subscript",
                "ordinal",
                "numerator",
                "denominator",
            ],
        ),
        "kerning" => {
            if v.is_number() {
                number(v, &what, -1000.0, 1000.0, false).map(|_| ())
            } else {
                one_of(v, &what, &["metrics", "optical", "none"])
            }
        }
        "word_spacing" | "letter_spacing" | "glyph_scaling" => triple(v, key),
        "features" => {
            let list = v.as_array().filter(|a| a.len() <= 64).with_context(|| {
                format!("[invalid-input] {what} must be an array of at most 64 tags")
            })?;
            for tag in list {
                text(tag, &what, 16)?;
            }
            Ok(())
        }
        "span_columns" => {
            if v == "none" || v == "all" {
                Ok(())
            } else {
                number(v, &what, 1.0, 40.0, false).map(|_| ())
            }
        }
        "tabs" => {
            let list = v.as_array().filter(|a| a.len() <= 64).with_context(|| {
                format!("[invalid-input] {what} must be an array of at most 64 tab stops")
            })?;
            for tab in list {
                let o = tab
                    .as_object()
                    .context("[invalid-input] a tab stop must be an object")?;
                number(
                    o.get("position").unwrap_or(&Value::Null),
                    "tab position",
                    0.0,
                    100_000.0,
                    false,
                )?;
                if let Some(a) = o.get("align") {
                    one_of(a, "tab alignment", &["left", "center", "right", "decimal"])?;
                }
                if let Some(l) = o.get("leader") {
                    text(l, "tab leader", 8)?;
                }
            }
            Ok(())
        }
        "drop_cap" => {
            let o = v
                .as_object()
                .with_context(|| format!("[invalid-input] {what} must be an object"))?;
            number(
                o.get("lines").unwrap_or(&Value::Null),
                "drop_cap lines",
                2.0,
                20.0,
                false,
            )?;
            if let Some(c) = o.get("chars") {
                number(c, "drop_cap chars", 1.0, 10.0, false)?;
            }
            Ok(())
        }
        "keep" => {
            let o = v
                .as_object()
                .with_context(|| format!("[invalid-input] {what} must be an object"))?;
            for (k, val) in o {
                match k.as_str() {
                    "together" | "with_next" => boolean(val, "keep option")?,
                    "start" => one_of(
                        val,
                        "keep start",
                        &[
                            "anywhere",
                            "next-column",
                            "next-frame",
                            "next-page",
                            "next-odd-page",
                            "next-even-page",
                        ],
                    )?,
                    "widows" | "orphans" => {
                        number(val, "keep lines", 0.0, 50.0, false)?;
                    }
                    other => bail!("[invalid-input] unknown keep option `{other}`"),
                }
            }
            Ok(())
        }
        "hyphenation" | "axes" | "rule_above" | "rule_below" | "list" | "effects"
        | "frame_options" | "wrap" | "anchored" | "borders" | "inset" | "condition" => {
            ensure!(
                v.is_object() || (key == "condition" && v.is_string()),
                "[invalid-input] {what} must be an object"
            );
            ensure!(
                v.to_string().len() <= 4096,
                "[limit-exceeded] {what} is larger than 4096 bytes"
            );
            Ok(())
        }
        "nested" | "line_styles" | "grep" => {
            let list = v.as_array().filter(|a| a.len() <= 32).with_context(|| {
                format!("[invalid-input] {what} must be an array of at most 32 rules")
            })?;
            ensure!(
                list.iter().all(Value::is_object),
                "[invalid-input] {what} entries must be objects"
            );
            crate::style_eval::validate_rules(key, v, &what)?;
            Ok(())
        }
        _ => {
            ensure!(
                v.to_string().len() <= 4096,
                "[limit-exceeded] {what} is larger than 4096 bytes"
            );
            Ok(())
        }
    }
}

fn base_of(style: &Value) -> Option<&str> {
    style.get("based_on").and_then(Value::as_str)
}

/// Validate every style definition, chain, reference and property.
pub fn validate(raw: &Value) -> Result<()> {
    crate::style_eval::validate_lists(raw)?;
    let Some(styles) = raw.pointer("/layout/styles") else {
        return Ok(());
    };
    let styles = styles
        .as_object()
        .context("[invalid-input] layout.styles must be an object")?;
    for (kind, group) in styles {
        check_kind(kind)?;
        let group = group
            .as_object()
            .with_context(|| format!("[invalid-input] layout.styles.{kind} must be an object"))?;
        ensure!(
            group.len() <= MAX_STYLES,
            "[limit-exceeded] {kind} styles: {} exceeds the limit of {MAX_STYLES}",
            group.len()
        );
        for (id, style) in group {
            check_id(id)?;
            let object = style.as_object().with_context(|| {
                format!("[invalid-input] {kind} style `{id}` must be an object")
            })?;
            for (key, value) in object {
                match key.as_str() {
                    "based_on" => {
                        if !value.is_null() {
                            let base = value.as_str().with_context(|| {
                                format!("[invalid-input] {kind} style `{id}`: based_on must be a style ID")
                            })?;
                            ensure!(
                                group.contains_key(base),
                                "[missing-resource] {kind} style `{id}` is based on missing style `{base}`; create it or remove based_on"
                            );
                        }
                    }
                    "next" => {
                        ensure!(
                            kind == "paragraph",
                            "[invalid-input] only paragraph styles have a next style (`{id}`)"
                        );
                        let next = value.as_str().with_context(|| {
                            format!(
                                "[invalid-input] paragraph style `{id}`: next must be a style ID"
                            )
                        })?;
                        ensure!(
                            group.contains_key(next),
                            "[missing-resource] paragraph style `{id}` names missing next style `{next}`"
                        );
                    }
                    "group" => text(value, &format!("{kind} style `{id}` group"), 256)?,
                    key => {
                        ensure!(
                            allowed(kind).contains(&key),
                            "[invalid-input] {kind} style `{id}` has unsupported property `{key}`; supported: {}",
                            allowed(kind).join(", ")
                        );
                        validate_property(kind, key, value)
                            .with_context(|| format!("{kind} style `{id}`"))?;
                    }
                }
            }
            chain_of(group, id)?;
        }
    }
    Ok(())
}

/// The `based_on` chain from `id` up to its root, child first.
fn chain_of<'a>(group: &'a Map<String, Value>, id: &str) -> Result<Vec<&'a Value>> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut at = Some(id);
    while let Some(current) = at {
        ensure!(
            seen.insert(current),
            "[invalid-input] style `{id}` is part of a based_on cycle through `{current}`"
        );
        ensure!(
            chain.len() < MAX_DEPTH,
            "[limit-exceeded] style `{id}` has a based_on chain deeper than {MAX_DEPTH}"
        );
        let Some(style) = group.get(current) else {
            bail!("[missing-resource] style `{current}` does not exist");
        };
        chain.push(style);
        at = base_of(style);
    }
    Ok(chain)
}

/// Resolve a style: engine defaults are the composer's own; `layout.defaults`
/// is applied first when present, then ancestors, then the style.
pub fn resolve(raw: &Value, kind: &str, id: &str) -> Result<Map<String, Value>> {
    check_kind(kind)?;
    let group =
        map(raw, kind).with_context(|| format!("[not-found] no {kind} styles are defined"))?;
    ensure!(
        group.contains_key(id),
        "[not-found] {kind} style `{id}` does not exist"
    );
    let chain = chain_of(group, id)?;
    let mut out = Map::new();
    if let Some(defaults) = raw
        .pointer(&format!("/layout/defaults/{kind}"))
        .and_then(Value::as_object)
    {
        for (k, v) in defaults {
            out.insert(k.clone(), v.clone());
        }
    }
    for style in chain.iter().rev() {
        if let Some(o) = style.as_object() {
            for (k, v) in o {
                if matches!(k.as_str(), "based_on" | "next" | "group") {
                    continue;
                }
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------- queries

fn paragraphs(raw: &Value) -> impl Iterator<Item = (&str, usize, &Value)> {
    raw.pointer("/layout/stories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|story| {
            let id = story["id"].as_str().unwrap_or_default();
            story
                .get("paragraphs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
                .map(move |(i, p)| (id, i, p))
        })
}

/// Stories referencing `id`, counting references through derived styles.
pub fn usage(raw: &Value, kind: &str, id: &str) -> Result<Value> {
    check_kind(kind)?;
    let group =
        map(raw, kind).with_context(|| format!("[not-found] no {kind} styles are defined"))?;
    ensure!(
        group.contains_key(id),
        "[not-found] {kind} style `{id}` does not exist"
    );
    // Styles that resolve through `id`.
    let mut affected: BTreeSet<&str> = BTreeSet::new();
    for other in group.keys() {
        let mut at = Some(other.as_str());
        let mut guard = 0;
        while let Some(current) = at {
            if current == id {
                affected.insert(other);
                break;
            }
            guard += 1;
            if guard > MAX_DEPTH {
                break;
            }
            at = group.get(current).and_then(base_of);
        }
    }
    let derived: Vec<&str> = affected.iter().copied().filter(|s| *s != id).collect();
    let mut by_story: BTreeMap<&str, usize> = BTreeMap::new();
    let mut references = 0usize;
    for (story, _, p) in paragraphs(raw) {
        let mut used = false;
        match kind {
            "paragraph" => {
                if p.get("style")
                    .and_then(Value::as_str)
                    .is_some_and(|s| affected.contains(s))
                {
                    used = true;
                }
            }
            "character" => {
                for run in p
                    .get("runs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if run
                        .get("style")
                        .and_then(Value::as_str)
                        .is_some_and(|s| affected.contains(s))
                    {
                        used = true;
                    }
                }
            }
            _ => {}
        }
        if used {
            references += 1;
            *by_story.entry(story).or_default() += 1;
        }
    }
    let frames: usize = by_story.len();
    let mut next_of: Vec<&str> = Vec::new();
    if kind == "paragraph" {
        for (other, style) in group {
            if style.get("next").and_then(Value::as_str) == Some(id) {
                next_of.push(other);
            }
        }
        next_of.sort_unstable();
    }
    Ok(json!({
        "kind": kind,
        "id": id,
        "derived": derived.iter().take(MAX_REPORT).collect::<Vec<_>>(),
        "derived_count": derived.len(),
        "next_of": next_of,
        "paragraph_references": references,
        "affected_stories": by_story.keys().take(MAX_REPORT).collect::<Vec<_>>(),
        "affected_story_count": frames,
        "has_more": by_story.len() > MAX_REPORT,
    }))
}

// ------------------------------------------------------------------ commands

fn default_limit() -> usize {
    100
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StyleAction {
    /// List styles (paginated), optionally one kind or group.
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Show a style: its definition, based_on chain and resolved properties.
    Show { kind: String, id: String },
    /// Create a style. Properties are `key=json` pairs: `--set size=12 --set align='"justify"'`.
    Create {
        kind: String,
        id: String,
        #[arg(long)]
        based_on: Option<String>,
        #[arg(long)]
        next: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long = "set")]
        #[serde(default)]
        set: Vec<String>,
    },
    /// Change a style's properties, base, next style or group.
    Set {
        kind: String,
        id: String,
        #[arg(long, conflicts_with = "no_base")]
        based_on: Option<String>,
        #[arg(long)]
        #[serde(default)]
        no_base: bool,
        #[arg(long)]
        next: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long = "set")]
        #[serde(default)]
        set: Vec<String>,
        #[arg(long = "unset")]
        #[serde(default)]
        unset: Vec<String>,
    },
    /// Remove a style. Referenced styles are refused unless `--reassign` names a replacement.
    Remove {
        kind: String,
        id: String,
        /// Style to use wherever the removed one was referenced.
        #[arg(long)]
        reassign: Option<String>,
    },
    /// Report derived styles, next-style links and stories that depend on a style.
    Usage { kind: String, id: String },
    /// Validate every style and report stories that reference undefined styles.
    Check,
    /// Evaluate nested, line and GREP character styles of a story into spans (read only).
    Evaluate {
        story: String,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Compute list labels (numbers and bullets) across stories (read only).
    Lists {
        #[arg(long)]
        story: Option<String>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Copy a paragraph's overrides into its style and clear them (redefine from selection).
    Redefine { story: String, paragraph: usize },
    /// Remove paragraph and run overrides (one paragraph, or the whole story).
    ClearOverrides {
        story: String,
        #[arg(long)]
        paragraph: Option<usize>,
    },
    /// Import styles from another document with a conflict policy.
    Import {
        from: String,
        /// rename, skip or replace.
        #[arg(long, default_value = "rename")]
        #[serde(default = "default_policy")]
        policy: String,
        #[arg(long)]
        kind: Option<String>,
    },
}

fn default_policy() -> String {
    "rename".into()
}

fn parse_pairs(set: &[String]) -> Result<Vec<(String, Value)>> {
    set.iter()
        .map(|pair| {
            let (key, value) = pair
                .split_once('=')
                .with_context(|| format!("[invalid-input] `{pair}` must be key=json"))?;
            let parsed =
                serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
            Ok((key.trim().to_string(), parsed))
        })
        .collect()
}

fn apply_fields(
    style: &mut Map<String, Value>,
    based_on: &Option<String>,
    next: &Option<String>,
    group: &Option<String>,
    set: &[String],
) -> Result<()> {
    if let Some(b) = based_on {
        style.insert("based_on".into(), json!(b));
    }
    if let Some(n) = next {
        style.insert("next".into(), json!(n));
    }
    if let Some(g) = group {
        style.insert("group".into(), json!(g));
    }
    for (k, v) in parse_pairs(set)? {
        ensure!(
            !matches!(k.as_str(), "based_on" | "next" | "group"),
            "[invalid-input] use --{} to change `{k}`",
            k.replace('_', "-")
        );
        style.insert(k, v);
    }
    Ok(())
}

fn summary_row(kind: &str, id: &str, style: &Value) -> Value {
    json!({
        "kind": kind,
        "id": id,
        "based_on": style.get("based_on").cloned().unwrap_or(Value::Null),
        "next": style.get("next").cloned().unwrap_or(Value::Null),
        "group": style.get("group").cloned().unwrap_or(Value::Null),
        "properties": style.as_object().map_or(0, |o| o.keys().filter(|k| !matches!(k.as_str(), "based_on" | "next" | "group")).count()),
    })
}

/// Rewrite references to `old` (of `kind`) to `new` across styles and stories.
fn rewrite_references(raw: &mut Value, kind: &str, old: &str, new: Option<&str>) -> usize {
    let mut changed = 0;
    if let Some(group) = raw
        .pointer_mut(&format!("/layout/styles/{kind}"))
        .and_then(Value::as_object_mut)
    {
        for style in group.values_mut().filter_map(Value::as_object_mut) {
            for key in ["based_on", "next"] {
                if style.get(key).and_then(Value::as_str) == Some(old) {
                    match new {
                        Some(n) => {
                            style.insert(key.into(), json!(n));
                        }
                        None => {
                            style.remove(key);
                        }
                    }
                    changed += 1;
                }
            }
        }
    }
    if let Some(stories) = raw
        .pointer_mut("/layout/stories")
        .and_then(Value::as_array_mut)
    {
        for story in stories {
            let Some(list) = story.get_mut("paragraphs").and_then(Value::as_array_mut) else {
                continue;
            };
            for p in list {
                if kind == "paragraph" && p.get("style").and_then(Value::as_str) == Some(old) {
                    match new {
                        Some(n) => p["style"] = json!(n),
                        None => {
                            if let Some(o) = p.as_object_mut() {
                                o.remove("style");
                            }
                        }
                    }
                    changed += 1;
                }
                if kind == "character" {
                    for run in p
                        .get_mut("runs")
                        .and_then(Value::as_array_mut)
                        .into_iter()
                        .flatten()
                    {
                        if run.get("style").and_then(Value::as_str) == Some(old) {
                            match new {
                                Some(n) => run["style"] = json!(n),
                                None => {
                                    if let Some(o) = run.as_object_mut() {
                                        o.remove("style");
                                    }
                                }
                            }
                            changed += 1;
                        }
                    }
                }
            }
        }
    }
    changed
}

/// Execute a style action. Returns (result, read_only).
pub fn run_style(raw: &mut Value, action: StyleAction) -> Result<(Value, bool)> {
    require_v9(raw)?;
    match action {
        StyleAction::List {
            kind,
            group,
            offset,
            limit,
        } => {
            ensure!(
                (1..=1000).contains(&limit),
                "[invalid-input] --limit must be between 1 and 1000"
            );
            if let Some(k) = &kind {
                check_kind(k)?;
            }
            let mut rows = Vec::new();
            for k in KINDS {
                if kind.as_deref().is_some_and(|w| w != k) {
                    continue;
                }
                if let Some(m) = map(raw, k) {
                    for (id, style) in m {
                        if group
                            .as_deref()
                            .is_some_and(|g| style.get("group").and_then(Value::as_str) != Some(g))
                        {
                            continue;
                        }
                        rows.push(summary_row(k, id, style));
                    }
                }
            }
            let page: Vec<Value> = rows.iter().skip(offset).take(limit).cloned().collect();
            Ok((
                json!({
                    "matches": rows.len(),
                    "returned": page.len(),
                    "offset": offset,
                    "limit": limit,
                    "has_more": offset + page.len() < rows.len(),
                    "styles": page,
                }),
                true,
            ))
        }
        StyleAction::Show { kind, id } => {
            let resolved = resolve(raw, &kind, &id)?;
            let group = map(raw, &kind).expect("resolve checked");
            let chain: Vec<String> = {
                let mut ids = Vec::new();
                let mut at = Some(id.as_str());
                while let Some(c) = at {
                    ids.push(c.to_string());
                    at = group.get(c).and_then(base_of);
                }
                ids
            };
            Ok((
                json!({
                    "kind": kind,
                    "id": id,
                    "style": group[&id],
                    "chain": chain,
                    "resolved": resolved,
                }),
                true,
            ))
        }
        StyleAction::Create {
            kind,
            id,
            based_on,
            next,
            group,
            set,
        } => {
            check_kind(&kind)?;
            check_id(&id)?;
            {
                let m = map_mut(raw, &kind)?;
                ensure!(
                    !m.contains_key(&id),
                    "[invalid-input] {kind} style `{id}` already exists; use `style set` to change it"
                );
                ensure!(
                    m.len() < MAX_STYLES,
                    "[limit-exceeded] {kind} styles are limited to {MAX_STYLES}"
                );
                let mut style = Map::new();
                apply_fields(&mut style, &based_on, &next, &group, &set)?;
                m.insert(id.clone(), Value::Object(style));
            }
            validate(raw)?;
            Ok((json!({"created": {"kind": kind, "id": id}}), false))
        }
        StyleAction::Set {
            kind,
            id,
            based_on,
            no_base,
            next,
            group,
            set,
            unset,
        } => {
            check_kind(&kind)?;
            {
                let m = map_mut(raw, &kind)?;
                let style = m
                    .get_mut(&id)
                    .and_then(Value::as_object_mut)
                    .with_context(|| format!("[not-found] {kind} style `{id}` does not exist"))?;
                apply_fields(style, &based_on, &next, &group, &set)?;
                if no_base {
                    style.remove("based_on");
                }
                for key in &unset {
                    ensure!(
                        style.remove(key).is_some(),
                        "[invalid-input] {kind} style `{id}` has no property `{key}` to unset"
                    );
                }
            }
            validate(raw)?;
            let usage = usage(raw, &kind, &id)?;
            Ok((
                json!({
                    "updated": {"kind": kind, "id": id},
                    "affected_styles": usage["derived_count"],
                    "affected_stories": usage["affected_story_count"],
                    "affected_story_ids": usage["affected_stories"],
                }),
                false,
            ))
        }
        StyleAction::Remove { kind, id, reassign } => {
            check_kind(&kind)?;
            let info = usage(raw, &kind, &id)?;
            let direct_users = info["paragraph_references"].as_u64().unwrap_or(0)
                + info["derived_count"].as_u64().unwrap_or(0)
                + info["next_of"].as_array().map_or(0, |a| a.len() as u64);
            if direct_users > 0 && reassign.is_none() {
                bail!(
                    "[in-use] {kind} style `{id}` is used by {} derived styles, {} next links and {} paragraph references; pass --reassign STYLE to move them",
                    info["derived_count"],
                    info["next_of"].as_array().map_or(0, Vec::len),
                    info["paragraph_references"]
                );
            }
            if let Some(target) = &reassign {
                ensure!(
                    target != &id && map(raw, &kind).is_some_and(|m| m.contains_key(target)),
                    "[missing-resource] reassign target `{target}` does not exist or is the removed style"
                );
            }
            // Derived styles inherit the removed style's own base so they keep resolving.
            let own_base = map(raw, &kind)
                .and_then(|m| m.get(&id))
                .and_then(base_of)
                .map(str::to_string);
            let new = reassign.clone().or(own_base);
            let rewritten = rewrite_references(raw, &kind, &id, new.as_deref());
            map_mut(raw, &kind)?.remove(&id);
            validate(raw)?;
            Ok((
                json!({"removed": {"kind": kind, "id": id}, "reassigned_to": reassign, "references_rewritten": rewritten}),
                false,
            ))
        }
        StyleAction::Usage { kind, id } => Ok((usage(raw, &kind, &id)?, true)),
        StyleAction::Evaluate {
            story,
            offset,
            limit,
        } => Ok((
            crate::style_eval::evaluate_story(raw, &story, limit, offset)?,
            true,
        )),
        StyleAction::Lists {
            story,
            offset,
            limit,
        } => Ok((
            crate::style_eval::list_labels(raw, story.as_deref(), limit, offset)?,
            true,
        )),
        StyleAction::Check => {
            validate(raw)?;
            let mut missing: BTreeMap<String, usize> = BTreeMap::new();
            let known: HashSet<&str> = map(raw, "paragraph")
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let known_char: HashSet<&str> = map(raw, "character")
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();
            for (_, _, p) in paragraphs(raw) {
                if let Some(s) = p.get("style").and_then(Value::as_str) {
                    if !known.contains(s) {
                        *missing.entry(format!("paragraph:{s}")).or_default() += 1;
                    }
                }
                for run in p
                    .get("runs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(s) = run.get("style").and_then(Value::as_str) {
                        if !known_char.contains(s) {
                            *missing.entry(format!("character:{s}")).or_default() += 1;
                        }
                    }
                }
            }
            let counts: BTreeMap<&str, usize> = KINDS
                .iter()
                .map(|k| (*k, map(raw, k).map_or(0, Map::len)))
                .collect();
            Ok((
                json!({"valid": true, "counts": counts, "undefined_references": missing}),
                true,
            ))
        }
        StyleAction::Redefine { story, paragraph } => {
            let (style_id, overrides) = {
                let p = paragraph_mut(raw, &story, paragraph)?;
                let style = p
                    .get("style")
                    .and_then(Value::as_str)
                    .context("[invalid-input] the paragraph has no style to redefine; apply a paragraph style first")?
                    .to_string();
                let o = p
                    .as_object_mut()
                    .and_then(|o| o.remove("overrides"))
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                (style, o)
            };
            ensure!(
                !overrides.is_empty(),
                "[invalid-input] paragraph {paragraph} of story `{story}` has no overrides to redefine from"
            );
            {
                let m = map_mut(raw, "paragraph")?;
                let style = m
                    .get_mut(&style_id)
                    .and_then(Value::as_object_mut)
                    .with_context(|| {
                        format!("[missing-resource] paragraph style `{style_id}` is not defined")
                    })?;
                for (k, v) in &overrides {
                    style.insert(k.clone(), v.clone());
                }
            }
            validate(raw)?;
            let info = usage(raw, "paragraph", &style_id)?;
            Ok((
                json!({
                    "redefined": style_id,
                    "properties": overrides.keys().collect::<Vec<_>>(),
                    "affected_stories": info["affected_stories"],
                }),
                false,
            ))
        }
        StyleAction::ClearOverrides { story, paragraph } => {
            let list = raw
                .pointer_mut("/layout/stories")
                .and_then(Value::as_array_mut)
                .and_then(|s| s.iter_mut().find(|s| s["id"] == story.as_str()))
                .with_context(|| format!("[not-found] story `{story}` does not exist"))?
                .get_mut("paragraphs")
                .and_then(Value::as_array_mut)
                .context("[invalid-input] story has no paragraphs")?;
            if let Some(i) = paragraph {
                ensure!(
                    i < list.len(),
                    "[not-found] story `{story}` has {} paragraphs; index {i} is out of range",
                    list.len()
                );
            }
            let mut cleared = 0;
            for (i, p) in list.iter_mut().enumerate() {
                if paragraph.is_some_and(|w| w != i) {
                    continue;
                }
                if let Some(o) = p.as_object_mut() {
                    if o.remove("overrides").is_some() {
                        cleared += 1;
                    }
                }
                for run in p
                    .get_mut("runs")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if let Some(o) = run.as_object_mut() {
                        if o.remove("overrides").is_some() {
                            cleared += 1;
                        }
                    }
                }
            }
            Ok((json!({"story": story, "cleared": cleared}), false))
        }
        StyleAction::Import { from, policy, kind } => {
            ensure!(
                matches!(policy.as_str(), "rename" | "skip" | "replace"),
                "[invalid-input] --policy must be rename, skip or replace"
            );
            if let Some(k) = &kind {
                check_kind(k)?;
            }
            let bytes = std::fs::read(&from)
                .with_context(|| format!("[missing-resource] cannot read {from}"))?;
            let other: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("[invalid-input] {from} is not a .pen document"))?;
            require_v9(&other).with_context(|| {
                format!("{from} has no layout styles (version 9 documents only)")
            })?;
            validate(&other)?;
            let mut report = Vec::new();
            for k in KINDS {
                if kind.as_deref().is_some_and(|w| w != k) {
                    continue;
                }
                let Some(incoming) = map(&other, k).cloned() else {
                    continue;
                };
                // Decide final IDs first so based_on/next can be remapped.
                let existing: HashSet<String> = map(raw, k)
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default();
                let mut taken = existing.clone();
                let mut mapping: HashMap<String, Option<String>> = HashMap::new();
                for id in incoming.keys() {
                    if !existing.contains(id) {
                        mapping.insert(id.clone(), Some(id.clone()));
                        taken.insert(id.clone());
                        continue;
                    }
                    match policy.as_str() {
                        "skip" => {
                            mapping.insert(id.clone(), None);
                            report.push(json!({"kind": k, "id": id, "action": "skipped"}));
                        }
                        "replace" => {
                            mapping.insert(id.clone(), Some(id.clone()));
                            report.push(json!({"kind": k, "id": id, "action": "replaced"}));
                        }
                        _ => {
                            let mut n = 2;
                            let fresh = loop {
                                let candidate = format!("{id}-{n}");
                                if !taken.contains(&candidate) && !incoming.contains_key(&candidate)
                                {
                                    break candidate;
                                }
                                n += 1;
                            };
                            taken.insert(fresh.clone());
                            report.push(
                                json!({"kind": k, "id": id, "action": "renamed", "as": fresh}),
                            );
                            mapping.insert(id.clone(), Some(fresh));
                        }
                    }
                }
                let target = map_mut(raw, k)?;
                for (id, style) in incoming {
                    let Some(Some(final_id)) = mapping.get(&id) else {
                        continue;
                    };
                    let mut style = style;
                    if let Some(o) = style.as_object_mut() {
                        for key in ["based_on", "next"] {
                            let Some(r) = o.get(key).and_then(Value::as_str).map(str::to_string)
                            else {
                                continue;
                            };
                            match mapping.get(&r) {
                                Some(Some(n)) => {
                                    o.insert(key.into(), json!(n));
                                }
                                _ => {
                                    // Skipped base: keep pointing at the existing style of that ID.
                                    if !existing.contains(&r) {
                                        o.remove(key);
                                    }
                                }
                            }
                        }
                    }
                    ensure!(
                        target.len() < MAX_STYLES || target.contains_key(final_id),
                        "[limit-exceeded] {k} styles are limited to {MAX_STYLES}"
                    );
                    target.insert(final_id.clone(), style);
                    if !report
                        .iter()
                        .any(|r| r["kind"] == k && r["id"] == id.as_str())
                    {
                        report.push(json!({"kind": k, "id": id, "action": "added"}));
                    }
                }
            }
            validate(raw)?;
            Ok((json!({"imported": report.len(), "report": report}), false))
        }
    }
}

fn paragraph_mut<'a>(raw: &'a mut Value, story: &str, index: usize) -> Result<&'a mut Value> {
    let list = raw
        .pointer_mut("/layout/stories")
        .and_then(Value::as_array_mut)
        .and_then(|s| s.iter_mut().find(|s| s["id"] == story))
        .with_context(|| format!("[not-found] story `{story}` does not exist"))?
        .get_mut("paragraphs")
        .and_then(Value::as_array_mut)
        .context("[invalid-input] story has no paragraphs")?;
    let len = list.len();
    list.get_mut(index).with_context(|| {
        format!("[not-found] story `{story}` has {len} paragraphs; index {index} is out of range")
    })
}

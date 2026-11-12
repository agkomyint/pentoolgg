//! v0.14.0 task 10: long-document features (`docs/layout-v1.md` section 10).
//!
//! Footnotes and endnotes, cross-references, hyperlinks and bookmarks, text
//! variables, conditional text, and generated tables of contents, indexes,
//! captions and endnote stories. Everything is stored as plain data in the
//! `layout` object and referenced from story runs. Nothing here reads the
//! wall clock or the network: dates come from the recorded
//! `layout.generated_at` (or an explicit `--date`), file names from the
//! caller. Page positions come from the task 3 estimator, so page-dependent
//! results are reported `estimated: true`.
use anyhow::{bail, ensure, Context as _, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use unicode_normalization::UnicodeNormalization;

pub const MAX_NOTES: usize = 100_000;
pub const MAX_XREFS: usize = 100_000;
pub const MAX_LINKS: usize = 100_000;
pub const MAX_BOOKMARKS: usize = 100_000;
pub const MAX_TOPICS: usize = 100_000;
pub const MAX_VARIABLES: usize = 1_000;
pub const MAX_CONDITIONS: usize = 1_000;
pub const MAX_GENERATED: usize = 1_000;
pub const MAX_ENTRIES: usize = 100_000;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_URL: usize = 2048;
const MAX_DEPTH: usize = 4;
const URL_SCHEMES: &[&str] = &["http", "https", "mailto"];
const NOTE_KINDS: &[&str] = &["footnote", "endnote"];
const RESTARTS: &[&str] = &["document", "story", "section", "page"];
const NUMBER_STYLES: &[&str] = &[
    "arabic",
    "lower-roman",
    "upper-roman",
    "lower-alpha",
    "upper-alpha",
];
const XREF_FORMATS: &[&str] = &[
    "page",
    "paragraph-text",
    "paragraph-number",
    "chapter",
    "custom",
];
const VARIABLE_KINDS: &[&str] = &[
    "chapter-number",
    "running-header",
    "creation-date",
    "modification-date",
    "output-date",
    "file-name",
    "last-page-number",
    "custom",
];
const GENERATED_KINDS: &[&str] = &["toc", "index", "captions", "list-of-figures", "endnotes"];
const PAGE_RANGES: &[&str] = &["first", "contiguous", "all"];
const CAPTION_SOURCES: &[&str] = &["paragraphs", "metadata", "both"];
const DATE_KEYS: &[(&str, &str)] = &[
    ("creation-date", "created_at"),
    ("modification-date", "modified_at"),
    ("output-date", "output_at"),
];

/// A running-header candidate: paragraph page and text.
type HeaderEntry = (Option<usize>, String);
/// A resolved target: optional (story, paragraph) and optional page index.
type Resolved = (Option<(String, usize)>, Option<usize>);

fn default_limit() -> usize {
    100
}

/// Caller supplied facts the document itself does not hold.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// File name for the `file-name` variable.
    pub file_name: Option<String>,
}

/// Where a run is inserted: story, paragraph index and top-level character.
#[derive(Debug, Clone, Args, Serialize, Deserialize)]
pub struct Place {
    #[arg(long)]
    pub story: String,
    #[arg(long)]
    pub paragraph: usize,
    /// Character offset in the paragraph (default: the end).
    #[arg(long)]
    #[serde(default)]
    pub at: Option<usize>,
}

/// Where a bookmark or a cross-reference points.
#[derive(Debug, Clone, Args, Serialize, Deserialize, Default)]
pub struct Target {
    /// Target story (with `--target-paragraph`).
    #[arg(long)]
    #[serde(default)]
    pub target_story: Option<String>,
    #[arg(long)]
    #[serde(default)]
    pub target_paragraph: Option<usize>,
    /// Target page ID.
    #[arg(long)]
    #[serde(default)]
    pub target_page: Option<String>,
    /// Target bookmark ID.
    #[arg(long)]
    #[serde(default)]
    pub target_bookmark: Option<String>,
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DocAction {
    /// Insert a footnote or endnote run with its body text.
    NoteAdd {
        id: String,
        #[command(flatten)]
        #[serde(flatten)]
        place: Place,
        /// Body text of the note.
        #[arg(long)]
        text: String,
        /// footnote (default) or endnote.
        #[arg(long, default_value = "footnote")]
        #[serde(default = "footnote")]
        kind: String,
        /// Let the note span all columns of the frame.
        #[arg(long)]
        #[serde(default)]
        span_columns: bool,
    },
    /// Remove a note and its run.
    NoteRemove { id: String },
    /// List notes (paginated) with their numbers, labels and estimated pages.
    NoteList {
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Show or set the numbering rules shared by all notes.
    NoteSettings {
        /// document, story, section or page (endnotes treat page as document).
        #[arg(long)]
        #[serde(default)]
        restart: Option<String>,
        /// arabic, lower-roman, upper-roman, lower-alpha or upper-alpha.
        #[arg(long)]
        #[serde(default)]
        style: Option<String>,
    },
    /// Number the notes and report which ones fit, move or split.
    NotePlan,
    /// Define a condition.
    ConditionDefine {
        id: String,
        /// Start hidden.
        #[arg(long)]
        #[serde(default)]
        hidden: bool,
    },
    /// Remove a condition that no run uses.
    ConditionRemove { id: String },
    /// Define a condition set (conditions it shows or hides).
    ConditionSetDefine {
        id: String,
        #[arg(long, value_delimiter = ',')]
        #[serde(default)]
        show: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        #[serde(default)]
        hide: Vec<String>,
    },
    /// Activate a condition set (or `--none`).
    ConditionActivate {
        #[arg(conflicts_with = "none")]
        #[serde(default)]
        set: Option<String>,
        #[arg(long)]
        #[serde(default)]
        none: bool,
    },
    /// Wrap new text in a condition at a position.
    ConditionWrap {
        #[command(flatten)]
        #[serde(flatten)]
        place: Place,
        #[arg(long)]
        condition: String,
        #[arg(long)]
        text: String,
    },
    /// List conditions, sets and the active set.
    ConditionList,
    /// Add a bookmark to a paragraph or a page.
    BookmarkAdd {
        id: String,
        #[arg(long)]
        #[serde(default)]
        name: Option<String>,
        #[arg(long)]
        #[serde(default)]
        story: Option<String>,
        #[arg(long)]
        #[serde(default)]
        paragraph: Option<usize>,
        #[arg(long)]
        #[serde(default)]
        page: Option<String>,
    },
    /// Remove a bookmark that nothing points at.
    BookmarkRemove { id: String },
    /// List bookmarks (paginated).
    BookmarkList {
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Insert a hyperlink run carrying visible text.
    LinkAdd {
        id: String,
        #[command(flatten)]
        #[serde(flatten)]
        place: Place,
        /// Visible text of the link.
        #[arg(long)]
        text: String,
        /// http, https or mailto URL (no scheme is ever fetched).
        #[arg(long)]
        #[serde(default)]
        url: Option<String>,
        /// Destination page ID.
        #[arg(long)]
        #[serde(default)]
        page: Option<String>,
        /// Destination bookmark ID.
        #[arg(long)]
        #[serde(default)]
        bookmark: Option<String>,
    },
    /// Remove a hyperlink; its visible text stays in the story.
    LinkRemove { id: String },
    /// List hyperlinks with their destination state (paginated).
    LinkList {
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Insert a cross-reference run.
    XrefAdd {
        id: String,
        #[command(flatten)]
        #[serde(flatten)]
        place: Place,
        #[command(flatten)]
        #[serde(flatten)]
        target: Target,
        /// page, paragraph-text, paragraph-number, chapter or custom.
        #[arg(long, default_value = "page")]
        #[serde(default = "page_format")]
        format: String,
        /// Template for `custom`: {page} {text} {number} {chapter}.
        #[arg(long)]
        #[serde(default)]
        template: Option<String>,
    },
    /// Remove a cross-reference and its run.
    XrefRemove { id: String },
    /// Resolve every cross-reference; broken targets are `?` and reported.
    XrefList {
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Define a text variable.
    VariableDefine {
        id: String,
        /// chapter-number, running-header, creation-date, modification-date,
        /// output-date, file-name, last-page-number or custom.
        #[arg(long)]
        kind: String,
        /// Paragraph style for running-header.
        #[arg(long)]
        #[serde(default)]
        style: Option<String>,
        /// first or last paragraph of the style on the page (running-header).
        #[arg(long)]
        #[serde(default)]
        pick: Option<String>,
        /// Text of a custom variable.
        #[arg(long)]
        #[serde(default)]
        value: Option<String>,
        /// Date pattern: YYYY YY MMMM MMM MM DD HH mm ss (default YYYY-MM-DD).
        #[arg(long)]
        #[serde(default)]
        format: Option<String>,
    },
    /// Remove a variable that no run uses.
    VariableRemove { id: String },
    /// Insert a variable run.
    VariableInsert {
        id: String,
        #[command(flatten)]
        #[serde(flatten)]
        place: Place,
    },
    /// List variables with their current values at the document level.
    VariableList {
        /// Explicit date (YYYY-MM-DD[THH:MM:SS[Z]]); never the wall clock.
        #[arg(long)]
        #[serde(default)]
        date: Option<String>,
    },
    /// Resolved plain text of a story (notes, xrefs, variables, conditions).
    Text {
        story: String,
        #[arg(long)]
        #[serde(default)]
        date: Option<String>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Define or update an index topic.
    IndexTopic {
        id: String,
        /// Display text of the topic.
        #[arg(long)]
        text: String,
        /// Sort text (default: the display text).
        #[arg(long)]
        #[serde(default)]
        sort: Option<String>,
        /// Make the topic a `see` cross-reference to another topic.
        #[arg(long)]
        #[serde(default)]
        see: Option<String>,
    },
    /// Insert an index marker run for a topic.
    IndexMark {
        topic: String,
        #[command(flatten)]
        #[serde(flatten)]
        place: Place,
    },
    /// List topics with their marker pages.
    IndexList,
    /// Record a generated story (TOC, index, captions, endnotes).
    GeneratedAdd {
        id: String,
        /// toc, index, captions, list-of-figures or endnotes.
        #[arg(long)]
        kind: String,
        /// Story that receives the generated paragraphs (created if missing).
        #[arg(long)]
        story: String,
        /// toc: paragraph style IDs in level order.
        #[arg(long, value_delimiter = ',')]
        #[serde(default)]
        styles: Vec<String>,
        /// captions: paragraph style holding caption text.
        #[arg(long)]
        #[serde(default)]
        style: Option<String>,
        /// captions: label such as `Figure`.
        #[arg(long)]
        #[serde(default)]
        label: Option<String>,
        /// captions: paragraphs, metadata (node `caption`) or both.
        #[arg(long)]
        #[serde(default)]
        source: Option<String>,
        /// index: first, contiguous or all.
        #[arg(long)]
        #[serde(default)]
        page_range: Option<String>,
        /// index: language used to tailor the collation (en, sv, da, ...).
        #[arg(long)]
        #[serde(default)]
        language: Option<String>,
    },
    /// Regenerate one generated story deterministically.
    Generate { id: String },
    /// Report each generated story's digest and live `stale` state.
    GeneratedList,
    /// Write the live `stale` flags into the document.
    GeneratedCheck,
    /// Remove a generated record (the story is kept).
    GeneratedRemove { id: String },
    /// Every unresolved cross-reference, link, variable and placement.
    Report,
}

fn footnote() -> String {
    "footnote".into()
}

fn page_format() -> String {
    "page".into()
}

// ------------------------------------------------------------ helpers

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn layout_list<'a>(raw: &'a Value, key: &str) -> &'a [Value] {
    raw.get("layout").map_or(&[][..], |l| list(l, key))
}

fn id_of(value: &Value) -> &str {
    value.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn one_of(value: &str, what: &str, allowed: &[&str]) -> Result<()> {
    ensure!(
        allowed.contains(&value),
        "[invalid-input] {what} `{value}` must be one of {}",
        allowed.join(", ")
    );
    Ok(())
}

fn check_id(what: &str, id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')),
        "[invalid-input] {what} ID `{id}` must be 1-128 characters of letters, digits, `_ - . :`"
    );
    Ok(())
}

fn check_text(text: &str, what: &str) -> Result<String> {
    ensure!(
        text.len() <= MAX_TEXT_BYTES,
        "[limit-exceeded] {what}: {} bytes exceeds the limit of {MAX_TEXT_BYTES}",
        text.len()
    );
    ensure!(
        !text.chars().any(char::is_control),
        "[invalid-input] {what} contains a control character; tabs and breaks are special runs"
    );
    Ok(text.nfc().collect())
}

fn check_url(url: &str) -> Result<()> {
    ensure!(
        url.len() <= MAX_URL,
        "[limit-exceeded] URL is {} bytes; the limit is {MAX_URL}",
        url.len()
    );
    ensure!(
        !url.chars().any(|c| c.is_control() || c.is_whitespace()),
        "[invalid-input] URL must not contain whitespace or control characters"
    );
    let scheme = url
        .split_once(':')
        .map(|(s, _)| s.to_ascii_lowercase())
        .unwrap_or_default();
    ensure!(
        URL_SCHEMES.contains(&scheme.as_str()),
        "[unsupported-capability] URL scheme `{scheme}` is not allowed; use one of {}",
        URL_SCHEMES.join(", ")
    );
    let rest = &url[scheme.len() + 1..];
    ensure!(
        !rest.is_empty(),
        "[invalid-input] URL `{url}` has no destination"
    );
    Ok(())
}

fn require_v9(raw: &Value) -> Result<()> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] long-document features need a version 9 document; run `pentool migrate --target 9` first"
    );
    Ok(())
}

fn layout_mut(raw: &mut Value) -> Result<&mut Map<String, Value>> {
    require_v9(raw)?;
    if !raw.get("layout").is_some_and(Value::is_object) {
        raw["layout"] = json!({"engine": 1});
    }
    raw["layout"]
        .as_object_mut()
        .context("layout must be an object")
}

fn array_mut<'a>(raw: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>> {
    let layout = layout_mut(raw)?;
    let entry = layout.entry(key.to_string()).or_insert_with(|| json!([]));
    entry
        .as_array_mut()
        .with_context(|| format!("layout.{key} must be an array"))
}

fn push_unique(raw: &mut Value, key: &str, what: &str, item: Value, max: usize) -> Result<()> {
    let id = id_of(&item).to_string();
    let items = array_mut(raw, key)?;
    ensure!(
        !items.iter().any(|v| id_of(v) == id),
        "[invalid-input] {what} `{id}` already exists"
    );
    ensure!(
        items.len() < max,
        "[limit-exceeded] {what}s per document: the limit is {max}"
    );
    items.push(item);
    Ok(())
}

fn find<'a>(raw: &'a Value, key: &str, what: &str, id: &str) -> Result<&'a Value> {
    layout_list(raw, key)
        .iter()
        .find(|v| id_of(v) == id)
        .with_context(|| format!("[missing-resource] {what} not found: {id}"))
}

fn page_index(raw: &Value, id: &str) -> Option<usize> {
    list(raw, "pages").iter().position(|p| id_of(p) == id)
}

fn story_ref<'a>(raw: &'a Value, id: &str) -> Option<&'a Value> {
    layout_list(raw, "stories").iter().find(|s| id_of(s) == id)
}

/// Validate that the insertion place exists before target resolution, so a bad
/// source story/paragraph is reported first (matching note-add and
/// bookmark-add); the destination is only worth fixing once the run can be
/// inserted at all.
fn check_place(raw: &Value, place: &Place) -> Result<()> {
    let story = raw
        .pointer("/layout/stories")
        .and_then(Value::as_array)
        .and_then(|stories| stories.iter().find(|s| id_of(s) == place.story))
        .with_context(|| format!("[missing-resource] story not found: {}", place.story))?;
    let count = list(story, "paragraphs").len();
    ensure!(
        place.paragraph < count,
        "[missing-resource] story {} has {count} paragraphs; paragraph {} does not exist",
        place.story,
        place.paragraph
    );
    Ok(())
}

fn insert_run(raw: &mut Value, place: &Place, run: Value) -> Result<()> {
    require_v9(raw)?;
    let story = raw
        .pointer_mut("/layout/stories")
        .and_then(Value::as_array_mut)
        .and_then(|stories| stories.iter_mut().find(|s| id_of(s) == place.story))
        .with_context(|| format!("[missing-resource] story not found: {}", place.story))?;
    let count = list(story, "paragraphs").len();
    let paragraph = story
        .get_mut("paragraphs")
        .and_then(Value::as_array_mut)
        .and_then(|p| p.get_mut(place.paragraph))
        .with_context(|| {
            format!(
                "[missing-resource] story {} has {count} paragraphs; paragraph {} does not exist",
                place.story, place.paragraph
            )
        })?;
    crate::wrap::insert_anchor_run(paragraph, place.at, run)
}

/// Every run of every story, nested conditional and link runs included, as
/// `(story, paragraph, run)`.
fn each_run<'a>(raw: &'a Value, mut visit: impl FnMut(&'a str, usize, &'a Value)) {
    fn rec<'a>(
        runs: &'a [Value],
        story: &'a str,
        paragraph: usize,
        visit: &mut impl FnMut(&'a str, usize, &'a Value),
    ) {
        for run in runs {
            visit(story, paragraph, run);
            if run.get("condition").is_some() || run.get("link").is_some() {
                rec(list(run, "runs"), story, paragraph, visit);
            }
        }
    }
    for story in layout_list(raw, "stories") {
        let sid = story.get("id").and_then(Value::as_str).unwrap_or_default();
        for (p, paragraph) in list(story, "paragraphs").iter().enumerate() {
            rec(list(paragraph, "runs"), sid, p, &mut visit);
        }
    }
}

fn used_by_runs(raw: &Value, key: &str, id: &str) -> bool {
    let mut used = false;
    each_run(raw, |_, _, run| {
        if run.get(key).and_then(Value::as_str) == Some(id) {
            used = true;
        }
    });
    used
}

/// Remove every run whose `key` is `id`; nested runs of a link are spliced
/// back into the paragraph so visible text is never lost.
fn remove_runs(raw: &mut Value, key: &str, id: &str, splice: bool) -> usize {
    fn rec(runs: &mut Vec<Value>, key: &str, id: &str, splice: bool) -> usize {
        let mut removed = 0;
        let mut out = Vec::with_capacity(runs.len());
        for mut run in std::mem::take(runs) {
            if run.get(key).and_then(Value::as_str) == Some(id) {
                removed += 1;
                if splice {
                    if let Some(nested) = run.get_mut("runs").and_then(Value::as_array_mut) {
                        out.append(nested);
                    }
                }
                continue;
            }
            if let Some(nested) = run.get_mut("runs").and_then(Value::as_array_mut) {
                removed += rec(nested, key, id, splice);
            }
            out.push(run);
        }
        *runs = out;
        removed
    }
    let mut removed = 0;
    if let Some(stories) = raw
        .pointer_mut("/layout/stories")
        .and_then(Value::as_array_mut)
    {
        for story in stories {
            if let Some(paragraphs) = story.get_mut("paragraphs").and_then(Value::as_array_mut) {
                for paragraph in paragraphs {
                    if let Some(runs) = paragraph.get_mut("runs").and_then(Value::as_array_mut) {
                        removed += rec(runs, key, id, splice);
                    }
                }
            }
        }
    }
    removed
}

fn page_slice(items: Vec<Value>, offset: usize, limit: usize) -> (usize, Vec<Value>, bool) {
    let matches = items.len();
    let slice: Vec<Value> = items.into_iter().skip(offset).take(limit).collect();
    let has_more = offset + slice.len() < matches;
    (matches, slice, has_more)
}

fn paged(items: Vec<Value>, offset: usize, limit: usize, key: &str) -> Result<Value> {
    ensure!(
        (1..=1000).contains(&limit),
        "[invalid-input] --limit must be between 1 and 1000"
    );
    let (matches, slice, has_more) = page_slice(items, offset, limit);
    Ok(json!({
        "matches": matches, "returned": slice.len(), "offset": offset,
        "limit": limit, "has_more": has_more, key: slice,
    }))
}

// ------------------------------------------------------------ dates

fn parse_date(text: &str) -> Result<(i64, u32, u32, u32, u32, u32)> {
    let bad = || {
        anyhow::anyhow!(
            "[invalid-input] date `{text}` must be YYYY-MM-DD, optionally followed by THH:MM:SS and Z"
        )
    };
    let (date, time) = match text.split_once('T') {
        Some((d, t)) => (d, Some(t.trim_end_matches('Z'))),
        None => (text, None),
    };
    let d: Vec<&str> = date.split('-').collect();
    ensure!(d.len() == 3 && d[0].len() == 4, bad());
    let year: i64 = d[0].parse().map_err(|_| bad())?;
    let month: u32 = d[1].parse().map_err(|_| bad())?;
    let day: u32 = d[2].parse().map_err(|_| bad())?;
    ensure!(d[1].len() == 2 && d[2].len() == 2, bad());
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => bail!(bad()),
    };
    ensure!((1..=days).contains(&day), bad());
    let (mut h, mut m, mut s) = (0, 0, 0);
    if let Some(t) = time {
        let p: Vec<&str> = t.split(':').collect();
        ensure!(p.len() == 3 && p.iter().all(|x| x.len() == 2), bad());
        h = p[0].parse().map_err(|_| bad())?;
        m = p[1].parse().map_err(|_| bad())?;
        s = p[2].parse().map_err(|_| bad())?;
        ensure!(h < 24 && m < 60 && s < 60, bad());
    }
    Ok((year, month, day, h, m, s))
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Locale-independent date pattern. Month names are always English so the
/// result never depends on the machine.
pub fn format_date(text: &str, pattern: &str) -> Result<String> {
    let (y, mo, d, h, mi, s) = parse_date(text)?;
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let starts = |i: usize, word: &str| {
        word.chars()
            .enumerate()
            .all(|(k, c)| chars.get(i + k) == Some(&c))
    };
    while i < chars.len() {
        let token = ["YYYY", "YY", "MMMM", "MMM", "MM", "DD", "HH", "mm", "ss"]
            .into_iter()
            .find(|t| starts(i, t));
        match token {
            Some("YYYY") => out.push_str(&format!("{y:04}")),
            Some("YY") => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            Some("MMMM") => out.push_str(MONTHS[mo as usize - 1]),
            Some("MMM") => out.push_str(&MONTHS[mo as usize - 1][..3]),
            Some("MM") => out.push_str(&format!("{mo:02}")),
            Some("DD") => out.push_str(&format!("{d:02}")),
            Some("HH") => out.push_str(&format!("{h:02}")),
            Some("mm") => out.push_str(&format!("{mi:02}")),
            Some("ss") => out.push_str(&format!("{s:02}")),
            _ => {
                out.push(chars[i]);
                i += 1;
                continue;
            }
        }
        i += token.map_or(1, str::len);
    }
    Ok(out)
}

// ------------------------------------------------------------ collation

/// Sort key: NFKD, combining marks removed, lower-cased; Swedish, Finnish,
/// Danish and Norwegian tailor their extra letters after `z`. Ties fall back
/// to code point order of the original text, so the order is total and
/// identical on every platform.
pub fn collation_key(text: &str, language: &str) -> String {
    let lang = language.split(['-', '_']).next().unwrap_or("");
    let tail: &[(char, &str)] = match lang {
        "sv" | "fi" => &[
            ('å', "z\u{ffff}a"),
            ('ä', "z\u{ffff}b"),
            ('ö', "z\u{ffff}c"),
        ],
        "da" | "nb" | "no" | "nn" => &[
            ('æ', "z\u{ffff}a"),
            ('ø', "z\u{ffff}b"),
            ('å', "z\u{ffff}c"),
        ],
        _ => &[],
    };
    let mut out = String::new();
    for c in text.chars().flat_map(|c| c.to_lowercase()) {
        if let Some((_, to)) = tail.iter().find(|(from, _)| *from == c) {
            out.push_str(to);
            continue;
        }
        for d in std::iter::once(c).nfkd() {
            if !unicode_normalization::char::is_combining_mark(d) {
                out.push(d);
            }
        }
    }
    out
}

/// Index heading for a collation key: tailored letters keep their own heading.
fn heading_letter(key: &str, language: &str) -> String {
    let mut chars = key.chars();
    let first = chars.next();
    if first == Some('z') && chars.next() == Some('\u{ffff}') {
        let lang = language.split(['-', '_']).next().unwrap_or("");
        let letters: &[char] = match lang {
            "sv" | "fi" => &['å', 'ä', 'ö'],
            "da" | "nb" | "no" | "nn" => &['æ', 'ø', 'å'],
            _ => &[],
        };
        if let Some(letter) = chars
            .next()
            .and_then(|c| letters.get((c as usize).wrapping_sub('a' as usize)))
        {
            return letter.to_uppercase().collect();
        }
    }
    first
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

/// `first`, `contiguous` or `all` rule over zero-based page indexes.
pub fn page_range(rule: &str, pages: &[(usize, String)]) -> String {
    let mut sorted: Vec<&(usize, String)> = pages.iter().collect();
    sorted.sort_by_key(|p| p.0);
    sorted.dedup_by_key(|p| p.0);
    let Some(first) = sorted.first() else {
        return String::new();
    };
    match rule {
        "first" => first.1.clone(),
        "all" => sorted
            .iter()
            .map(|p| p.1.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        _ => {
            let mut parts: Vec<String> = Vec::new();
            let mut i = 0;
            while i < sorted.len() {
                let mut j = i;
                while j + 1 < sorted.len() && sorted[j + 1].0 == sorted[j].0 + 1 {
                    j += 1;
                }
                if j > i {
                    parts.push(format!("{}\u{2013}{}", sorted[i].1, sorted[j].1));
                } else {
                    parts.push(sorted[i].1.clone());
                }
                i = j + 1;
            }
            parts.join(", ")
        }
    }
}

// ------------------------------------------------------------ validation

fn unique_ids<'a>(
    layout: &'a Value,
    key: &str,
    what: &str,
    max: usize,
) -> Result<HashSet<&'a str>> {
    let items = list(layout, key);
    ensure!(
        items.len() <= max,
        "[limit-exceeded] {what}s per document: {} exceeds the limit of {max}",
        items.len()
    );
    let mut ids = HashSet::new();
    for item in items {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|i| !i.is_empty())
            .with_context(|| format!("[malformed-resource] a {what} needs a nonempty `id`"))?;
        ensure!(ids.insert(id), "[invalid-input] duplicate {what} ID: {id}");
    }
    Ok(ids)
}

fn simple_runs(runs: &[Value], what: &str) -> Result<()> {
    ensure!(
        runs.len() <= 10_000,
        "[limit-exceeded] {what}: more than 10000 runs"
    );
    for run in runs {
        let ok = run.get("text").is_some_and(Value::is_string)
            || run.get("special").is_some_and(Value::is_string);
        ensure!(
            ok,
            "[malformed-resource] {what} holds only text and special runs"
        );
        if let Some(text) = run.get("text").and_then(Value::as_str) {
            ensure!(
                text.len() <= MAX_TEXT_BYTES && !text.chars().any(char::is_control),
                "[invalid-input] {what} has oversized text or a control character"
            );
        }
    }
    Ok(())
}

/// Structural checks of the long-document records and the runs that use them.
/// Broken *targets* (a deleted page or paragraph) are not validation errors;
/// they render `?` and are reported by `report`.
pub fn validate(raw: &Value) -> Result<()> {
    let Some(layout) = raw.get("layout") else {
        return Ok(());
    };
    if let Some(at) = layout.get("generated_at") {
        let text = at
            .as_str()
            .context("[invalid-input] layout.generated_at must be a string")?;
        parse_date(text)?;
    }
    for (_, key) in DATE_KEYS {
        if let Some(value) = layout.get(*key) {
            parse_date(
                value.as_str().with_context(|| {
                    format!("[invalid-input] layout.{key} must be a date string")
                })?,
            )?;
        }
    }
    if let Some(book) = layout.get("book_numbering") {
        let book = book
            .as_object()
            .context("[invalid-input] layout.book_numbering must be an object")?;
        for (key, value) in book {
            ensure!(
                matches!(key.as_str(), "chapter_offset" | "footnote_offset"),
                "[invalid-input] layout.book_numbering has unknown field {key}"
            );
            ensure!(
                value.as_u64().is_some_and(|n| n <= 1_000_000),
                "[invalid-input] layout.book_numbering.{key} must be an integer 0-1000000"
            );
        }
    }
    let notes = unique_ids(layout, "notes", "note", MAX_NOTES)?;
    for note in list(layout, "notes") {
        let id = id_of(note);
        one_of(
            note["kind"].as_str().unwrap_or_default(),
            "note kind",
            NOTE_KINDS,
        )?;
        let paragraphs = list(note, "paragraphs");
        ensure!(
            !paragraphs.is_empty(),
            "[malformed-resource] note {id} needs at least one paragraph"
        );
        for paragraph in paragraphs {
            simple_runs(list(paragraph, "runs"), &format!("note {id}"))?;
        }
    }
    if let Some(settings) = layout.get("note_settings") {
        if let Some(r) = settings.get("restart").and_then(Value::as_str) {
            one_of(r, "note restart", RESTARTS)?;
        }
        if let Some(s) = settings.get("style").and_then(Value::as_str) {
            one_of(s, "note numbering style", NUMBER_STYLES)?;
        }
    }
    let conditions = unique_ids(layout, "conditions", "condition", MAX_CONDITIONS)?;
    for condition in list(layout, "conditions") {
        ensure!(
            condition.get("visible").is_none_or(Value::is_boolean),
            "[malformed-resource] condition {} visible must be a boolean",
            id_of(condition)
        );
    }
    let sets = unique_ids(layout, "condition_sets", "condition set", MAX_CONDITIONS)?;
    for set in list(layout, "condition_sets") {
        let id = id_of(set);
        let mut seen = HashSet::new();
        for key in ["show", "hide"] {
            for entry in list(set, key) {
                let c = entry.as_str().unwrap_or_default();
                ensure!(
                    conditions.contains(c),
                    "[missing-resource] condition set {id} {key}s missing condition {c}"
                );
                ensure!(
                    seen.insert(c),
                    "[invalid-input] condition set {id} lists condition {c} more than once"
                );
            }
        }
    }
    if let Some(active) = layout.get("active_condition_set") {
        let active = active.as_str().unwrap_or_default();
        ensure!(
            sets.contains(active),
            "[missing-resource] active condition set not found: {active}"
        );
    }
    let bookmarks = unique_ids(layout, "bookmarks", "bookmark", MAX_BOOKMARKS)?;
    for bookmark in list(layout, "bookmarks") {
        let id = id_of(bookmark);
        let shaped = (bookmark.get("story").is_some_and(Value::is_string)
            && bookmark.get("paragraph").is_some_and(Value::is_u64))
            != bookmark.get("page").is_some_and(Value::is_string);
        ensure!(
            shaped,
            "[malformed-resource] bookmark {id} needs `story` and `paragraph`, or `page`"
        );
    }
    let links = unique_ids(layout, "hyperlinks", "hyperlink", MAX_LINKS)?;
    for link in list(layout, "hyperlinks") {
        let id = id_of(link);
        let kinds = ["url", "page", "bookmark"]
            .iter()
            .filter(|k| link.get(**k).is_some_and(Value::is_string))
            .count();
        ensure!(
            kinds == 1,
            "[malformed-resource] hyperlink {id} needs exactly one of url, page, bookmark"
        );
        if let Some(url) = link.get("url").and_then(Value::as_str) {
            check_url(url)?;
        }
        if let Some(b) = link.get("bookmark").and_then(Value::as_str) {
            ensure!(
                bookmarks.contains(b),
                "[missing-resource] hyperlink {id} points at missing bookmark {b}"
            );
        }
    }
    let xrefs = unique_ids(layout, "xrefs", "cross-reference", MAX_XREFS)?;
    for xref in list(layout, "xrefs") {
        let id = id_of(xref);
        let target = xref
            .get("target")
            .filter(|t| t.is_object())
            .with_context(|| {
                format!("[malformed-resource] cross-reference {id} needs a `target` object")
            })?;
        let shaped = [
            target.get("story").is_some_and(Value::is_string)
                && target.get("paragraph").is_some_and(Value::is_u64),
            target.get("page").is_some_and(Value::is_string),
            target.get("bookmark").is_some_and(Value::is_string),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        ensure!(
            shaped == 1,
            "[malformed-resource] cross-reference {id} target needs exactly one of story+paragraph, page, bookmark"
        );
        let format = xref["format"].as_str().unwrap_or_default();
        one_of(format, "cross-reference format", XREF_FORMATS)?;
        if format == "custom" {
            ensure!(
                xref["template"]
                    .as_str()
                    .is_some_and(|t| !t.is_empty() && t.len() <= 256),
                "[malformed-resource] cross-reference {id} custom format needs a template of at most 256 bytes"
            );
        }
    }
    let variables = unique_ids(layout, "variables", "variable", MAX_VARIABLES)?;
    for variable in list(layout, "variables") {
        let id = id_of(variable);
        let kind = variable["kind"].as_str().unwrap_or_default();
        one_of(kind, "variable kind", VARIABLE_KINDS)?;
        match kind {
            "running-header" => {
                ensure!(
                    variable["style"].as_str().is_some_and(|s| !s.is_empty()),
                    "[malformed-resource] variable {id} running-header needs a paragraph `style`"
                );
                one_of(
                    variable["pick"].as_str().unwrap_or("first"),
                    "running-header pick",
                    &["first", "last"],
                )?;
            }
            "custom" => {
                let value = variable["value"].as_str().with_context(|| {
                    format!("[malformed-resource] variable {id} custom needs a string `value`")
                })?;
                check_text(value, &format!("variable {id}"))?;
            }
            _ => {}
        }
        if let Some(format) = variable.get("format").and_then(Value::as_str) {
            ensure!(
                format.len() <= 64 && !format.chars().any(char::is_control),
                "[invalid-input] variable {id} date format is too long or has control characters"
            );
        }
    }
    let topics = unique_ids(layout, "index_topics", "index topic", MAX_TOPICS)?;
    for topic in list(layout, "index_topics") {
        let id = id_of(topic);
        ensure!(
            topic["text"].as_str().is_some_and(|t| !t.is_empty()),
            "[malformed-resource] index topic {id} needs `text`"
        );
        if let Some(see) = topic.get("see").and_then(Value::as_str) {
            ensure!(
                topics.contains(see) && see != id,
                "[missing-resource] index topic {id} `see` must name another topic: {see}"
            );
        }
    }
    let stories: HashSet<&str> = list(layout, "stories").iter().map(id_of).collect();
    let generated = unique_ids(layout, "generated", "generated item", MAX_GENERATED)?;
    let _ = generated;
    for item in list(layout, "generated") {
        let id = id_of(item);
        one_of(
            item["kind"].as_str().unwrap_or_default(),
            "generated kind",
            GENERATED_KINDS,
        )?;
        ensure!(
            item["story"].as_str().is_some_and(|s| stories.contains(s)),
            "[missing-resource] generated item {id} writes into a missing story"
        );
        if let Some(digest) = item.get("digest").and_then(Value::as_str) {
            ensure!(
                digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()),
                "[invalid-input] generated item {id} digest must be 64 hex digits"
            );
        }
        if let Some(rule) = item.pointer("/options/page_range").and_then(Value::as_str) {
            one_of(rule, "page range rule", PAGE_RANGES)?;
        }
        if let Some(source) = item.pointer("/options/source").and_then(Value::as_str) {
            one_of(source, "caption source", CAPTION_SOURCES)?;
        }
    }
    let mut note_uses: HashSet<&str> = HashSet::new();
    let mut failure: Option<anyhow::Error> = None;
    each_run(raw, |story, paragraph, run| {
        if failure.is_some() {
            return;
        }
        let checks: [(&str, &HashSet<&str>, &str); 6] = [
            ("note", &notes, "note"),
            ("xref", &xrefs, "cross-reference"),
            ("variable", &variables, "variable"),
            ("link", &links, "hyperlink"),
            ("condition", &conditions, "condition"),
            ("index", &topics, "index topic"),
        ];
        for (key, known, what) in checks {
            if let Some(id) = run.get(key).and_then(Value::as_str) {
                if !known.contains(id) {
                    failure = Some(anyhow::anyhow!(
                        "[missing-resource] story {story} paragraph {paragraph}: {what} not found: {id}"
                    ));
                } else if key == "note" && !note_uses.insert(id) {
                    failure = Some(anyhow::anyhow!(
                        "[invalid-input] note {id} is used by more than one run; add another note instead"
                    ));
                }
            }
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

// ------------------------------------------------------------ evaluation

use crate::stories::{condition_visible, placements, run_chars, Placed};

struct Section {
    page: usize,
    ordinal: usize,
    name: String,
}

struct Doc<'a> {
    raw: &'a Value,
    ctx: &'a Context,
    date: Option<String>,
    labels: Vec<Value>,
    sections: Vec<Section>,
    generated_stories: HashSet<String>,
    placed: RefCell<HashMap<String, Vec<Placed>>>,
    headers: RefCell<HashMap<String, Vec<HeaderEntry>>>,
    diags: RefCell<Vec<Value>>,
    notes: RefCell<Option<Rc<Vec<NoteInfo>>>>,
}

impl<'a> Doc<'a> {
    fn new(raw: &'a Value, ctx: &'a Context, date: Option<&str>) -> Result<Self> {
        if let Some(d) = date {
            parse_date(d)?;
        }
        let labels = crate::spreads::page_labels(raw);
        let mut sections: Vec<Section> = layout_list(raw, "sections")
            .iter()
            .filter_map(|s| {
                let page = page_index(raw, s.get("start_page")?.as_str()?)?;
                Some((page, s))
            })
            .map(|(page, s)| Section {
                page,
                ordinal: 0,
                name: s
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| id_of(s))
                    .to_string(),
            })
            .collect();
        sections.sort_by_key(|s| s.page);
        let chapter_offset = raw
            .pointer("/layout/book_numbering/chapter_offset")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        for (i, s) in sections.iter_mut().enumerate() {
            s.ordinal = i + 1 + chapter_offset;
        }
        let generated_stories = layout_list(raw, "generated")
            .iter()
            .filter_map(|g| g["story"].as_str().map(str::to_string))
            .collect();
        Ok(Doc {
            raw,
            ctx,
            date: date.map(str::to_string),
            labels,
            sections,
            generated_stories,
            placed: RefCell::new(HashMap::new()),
            headers: RefCell::new(HashMap::new()),
            diags: RefCell::new(Vec::new()),
            notes: RefCell::new(None),
        })
    }

    fn diag(&self, code: &str, id: &str, message: String) {
        self.diags
            .borrow_mut()
            .push(json!({"code": code, "id": id, "message": message}));
    }

    fn take_diags(&self) -> Vec<Value> {
        let mut all = std::mem::take(&mut *self.diags.borrow_mut());
        all.sort_by_key(Value::to_string);
        all.dedup();
        all
    }

    fn placed(&self, story: &str) -> Vec<Placed> {
        self.placed
            .borrow_mut()
            .entry(story.to_string())
            .or_insert_with(|| placements(self.raw, story))
            .clone()
    }

    fn page_of(&self, story: &str, offset: usize) -> Option<usize> {
        let placed = self.placed(story);
        placed
            .iter()
            .find(|p| offset >= p.start && offset < p.end)
            .or_else(|| placed.last().filter(|p| p.end > p.start && offset == p.end))
            .map(|p| p.page)
    }

    fn label(&self, page: usize) -> String {
        self.labels
            .get(page)
            .and_then(|l| l["label"].as_str())
            .unwrap_or("?")
            .to_string()
    }

    fn section(&self, page: usize) -> (usize, String) {
        self.sections
            .iter()
            .rev()
            .find(|s| s.page <= page)
            .map_or((1, String::new()), |s| (s.ordinal, s.name.clone()))
    }

    fn paragraph_starts(&self, story: &Value) -> Vec<usize> {
        let mut at = 0;
        list(story, "paragraphs")
            .iter()
            .map(|p| {
                let start = at;
                at += list(p, "runs")
                    .iter()
                    .map(|r| run_chars(self.raw, r))
                    .sum::<usize>()
                    + 1;
                start
            })
            .collect()
    }

    fn paragraph_page(&self, story: &Value, paragraph: usize) -> Option<usize> {
        let starts = self.paragraph_starts(story);
        self.page_of(id_of(story), *starts.get(paragraph)?)
    }

    fn text_of(&self, runs: &[Value], page: Option<usize>, depth: usize, out: &mut String) {
        for run in runs {
            if let Some(text) = run.get("text").and_then(Value::as_str) {
                out.push_str(text);
            } else if run.get("special").is_some() {
                out.push(' ');
            } else if let Some(c) = run.get("condition").and_then(Value::as_str) {
                if condition_visible(self.raw, c) {
                    self.text_of(list(run, "runs"), page, depth, out);
                }
            } else if run.get("link").is_some() {
                self.text_of(list(run, "runs"), page, depth, out);
            } else if let Some(v) = run.get("variable").and_then(Value::as_str) {
                match self.variable(v, page) {
                    Ok(text) => out.push_str(&text),
                    Err(reason) => {
                        self.diag("variable-unresolved", v, reason);
                        out.push('?');
                    }
                }
            } else if let Some(n) = run.get("note").and_then(Value::as_str) {
                if let Some(info) = self.notes().iter().find(|i| i.id == n) {
                    out.push_str(&info.label);
                }
            } else if let Some(x) = run.get("xref").and_then(Value::as_str) {
                match self.xref(x, depth + 1) {
                    Ok(text) => out.push_str(&text),
                    Err(reason) => {
                        self.diag("xref-unresolved", x, reason);
                        out.push('?');
                    }
                }
            }
        }
    }

    fn paragraph_text(&self, paragraph: &Value, page: Option<usize>, depth: usize) -> String {
        let mut out = String::new();
        self.text_of(list(paragraph, "runs"), page, depth, &mut out);
        out
    }

    /// `(story, paragraph, page)` of a bookmark, story+paragraph or page target.
    fn resolve_target(&self, target: &Value, depth: usize) -> Result<Resolved, String> {
        if depth > MAX_DEPTH {
            return Err("bookmark chain is too deep".into());
        }
        if let Some(b) = target.get("bookmark").and_then(Value::as_str) {
            let bookmark = layout_list(self.raw, "bookmarks")
                .iter()
                .find(|x| id_of(x) == b)
                .ok_or_else(|| format!("bookmark {b} does not exist"))?;
            return self.resolve_target(bookmark, depth + 1);
        }
        if let Some(page) = target.get("page").and_then(Value::as_str) {
            let index =
                page_index(self.raw, page).ok_or_else(|| format!("page {page} does not exist"))?;
            return Ok((None, Some(index)));
        }
        let story = target
            .get("story")
            .and_then(Value::as_str)
            .ok_or("target names no story, page or bookmark")?;
        let paragraph = target
            .get("paragraph")
            .and_then(Value::as_u64)
            .ok_or("target names no paragraph")? as usize;
        let source =
            story_ref(self.raw, story).ok_or_else(|| format!("story {story} does not exist"))?;
        let count = list(source, "paragraphs").len();
        if paragraph >= count {
            return Err(format!(
                "paragraph {paragraph} does not exist in story {story} ({count} paragraphs)"
            ));
        }
        let page = self.paragraph_page(source, paragraph);
        Ok((Some((story.to_string(), paragraph)), page))
    }

    fn xref(&self, id: &str, depth: usize) -> Result<String, String> {
        if depth > MAX_DEPTH {
            return Err("cross-references nest too deeply".into());
        }
        let xref = layout_list(self.raw, "xrefs")
            .iter()
            .find(|x| id_of(x) == id)
            .ok_or_else(|| format!("cross-reference {id} does not exist"))?;
        let (para, page) = self.resolve_target(&xref["target"], 0)?;
        let paragraph = para.as_ref().and_then(|(s, p)| {
            story_ref(self.raw, s)
                .and_then(|st| list(st, "paragraphs").get(*p).map(|v| (st, v, *p)))
        });
        let page_text = || -> Result<String, String> {
            page.map(|p| self.label(p))
                .ok_or_else(|| "the target is not placed on a page".to_string())
        };
        let text_part = || -> Result<String, String> {
            let (_, p, _) = paragraph.ok_or("the target is not a paragraph")?;
            let text = self.paragraph_text(p, page, depth).trim().to_string();
            if text.is_empty() {
                Err("the target paragraph has no text".into())
            } else {
                Ok(text)
            }
        };
        let number_part = || -> Result<String, String> {
            let (story, p, index) = paragraph.ok_or("the target is not a paragraph")?;
            let style = p.get("style");
            let n = list(story, "paragraphs")[..=index]
                .iter()
                .filter(|q| q.get("style") == style)
                .count();
            Ok(n.to_string())
        };
        let chapter_part = || -> Result<String, String> {
            let p = page.ok_or("the target is not placed on a page")?;
            let (ordinal, name) = self.section(p);
            Ok(if name.is_empty() {
                ordinal.to_string()
            } else {
                name
            })
        };
        match xref["format"].as_str().unwrap_or("page") {
            "page" => page_text(),
            "paragraph-text" => text_part(),
            "paragraph-number" => number_part(),
            "chapter" => chapter_part(),
            _ => {
                let template = xref["template"].as_str().unwrap_or_default();
                let mut out = template.to_string();
                for (token, part) in [
                    ("{page}", &page_text as &dyn Fn() -> Result<String, String>),
                    ("{text}", &text_part),
                    ("{number}", &number_part),
                    ("{chapter}", &chapter_part),
                ] {
                    if out.contains(token) {
                        out = out.replace(token, &part()?);
                    }
                }
                Ok(out)
            }
        }
    }

    fn date_text(&self, variable: &Value, kind: &str) -> Result<String, String> {
        let layout = self.raw.get("layout").unwrap_or(&Value::Null);
        let own = DATE_KEYS
            .iter()
            .find(|(k, _)| *k == kind)
            .and_then(|(_, key)| layout.get(*key).and_then(Value::as_str));
        let source = self
            .date
            .as_deref()
            .or(own)
            .or_else(|| layout.get("generated_at").and_then(Value::as_str))
            .ok_or(
                "the document records no generated_at; pass --date (the wall clock is never read)",
            )?;
        let pattern = variable
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("YYYY-MM-DD");
        format_date(source, pattern).map_err(|e| e.to_string())
    }

    fn header_list(&self, style: &str) -> Vec<(Option<usize>, String)> {
        if let Some(cached) = self.headers.borrow().get(style) {
            return cached.clone();
        }
        let mut out = Vec::new();
        for story in layout_list(self.raw, "stories") {
            if self.generated_stories.contains(id_of(story)) {
                continue;
            }
            let starts = self.paragraph_starts(story);
            for (i, p) in list(story, "paragraphs").iter().enumerate() {
                if p.get("style").and_then(Value::as_str) == Some(style) {
                    let page = self.page_of(id_of(story), starts[i]);
                    let mut text = String::new();
                    // Variables nested in a header paragraph resolve without a page
                    // so a header cannot depend on itself.
                    self.text_of(list(p, "runs"), None, MAX_DEPTH, &mut text);
                    out.push((page, text.trim().to_string()));
                }
            }
        }
        self.headers
            .borrow_mut()
            .insert(style.to_string(), out.clone());
        out
    }

    fn variable(&self, id: &str, page: Option<usize>) -> Result<String, String> {
        let variable = layout_list(self.raw, "variables")
            .iter()
            .find(|v| id_of(v) == id)
            .ok_or_else(|| format!("variable {id} does not exist"))?;
        let kind = variable["kind"].as_str().unwrap_or_default();
        let need_page =
            || page.ok_or_else(|| format!("variable {id} is used outside a placed frame"));
        match kind {
            "custom" => Ok(variable["value"].as_str().unwrap_or_default().to_string()),
            "file-name" => self
                .ctx
                .file_name
                .clone()
                .ok_or_else(|| "no file name was supplied for the file-name variable".into()),
            "last-page-number" => self
                .labels
                .last()
                .and_then(|l| l["label"].as_str())
                .map(str::to_string)
                .ok_or_else(|| "the document has no pages".into()),
            "chapter-number" => Ok(self.section(need_page()?).0.to_string()),
            "running-header" => {
                let p = need_page()?;
                let list = self.header_list(variable["style"].as_str().unwrap_or_default());
                let on_page: Vec<&(Option<usize>, String)> =
                    list.iter().filter(|(q, _)| *q == Some(p)).collect();
                let pick_last = variable["pick"].as_str() == Some("last");
                let chosen = if pick_last {
                    on_page.last()
                } else {
                    on_page.first()
                };
                Ok(chosen
                    .map(|(_, t)| t.clone())
                    .or_else(|| {
                        list.iter()
                            .rev()
                            .find(|(q, _)| q.is_some_and(|q| q < p))
                            .map(|(_, t)| t.clone())
                    })
                    .unwrap_or_default())
            }
            date_kind => self.date_text(variable, date_kind),
        }
    }
}

// ------------------------------------------------------------ numbering

/// Share of a frame's height that footnotes may use before they move on.
const NOTE_AREA: f64 = 0.4;

struct NoteInfo {
    id: String,
    kind: String,
    number: usize,
    label: String,
    story: String,
    paragraph: usize,
    page: Option<usize>,
    frame: Option<String>,
}

fn roman_label(mut n: usize, upper: bool) -> String {
    const TABLE: [(usize, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, text) in TABLE {
        while n >= value {
            out.push_str(text);
            n -= value;
        }
    }
    if upper {
        out.to_uppercase()
    } else {
        out
    }
}

fn alpha_label(mut n: usize, upper: bool) -> String {
    let mut letters = Vec::new();
    while n > 0 {
        n -= 1;
        letters.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    letters.reverse();
    let text: String = letters.into_iter().collect();
    if upper {
        text.to_uppercase()
    } else {
        text
    }
}

/// Label of the `n`th (1-based) note in a numbering style.
pub fn number_label(n: usize, style: &str) -> String {
    match style {
        "lower-roman" => roman_label(n, false),
        "upper-roman" => roman_label(n, true),
        "lower-alpha" => alpha_label(n, false),
        "upper-alpha" => alpha_label(n, true),
        _ => n.to_string(),
    }
}

fn digest_of(paragraphs: &[Value]) -> String {
    let bytes = serde_json::to_vec(paragraphs).unwrap_or_default();
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn para(style: &str, runs: Vec<Value>) -> Value {
    json!({"style": style, "runs": runs})
}

fn text_run(text: &str) -> Value {
    json!({"text": text})
}

impl<'a> Doc<'a> {
    fn page_id(&self, page: usize) -> Value {
        list(self.raw, "pages")
            .get(page)
            .and_then(|p| p.get("id"))
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Visit every visible run of a story as `(paragraph, offset, run)`,
    /// descending into visible conditional and link runs.
    fn walk_story(&self, story: &'a Value, visit: &mut dyn FnMut(usize, usize, &'a Value)) {
        fn rec<'a>(
            raw: &Value,
            runs: &'a [Value],
            paragraph: usize,
            offset: &mut usize,
            visit: &mut dyn FnMut(usize, usize, &'a Value),
        ) {
            for run in runs {
                if let Some(c) = run.get("condition").and_then(Value::as_str) {
                    if condition_visible(raw, c) {
                        visit(paragraph, *offset, run);
                        rec(raw, list(run, "runs"), paragraph, offset, visit);
                    }
                } else if run.get("link").is_some() {
                    visit(paragraph, *offset, run);
                    rec(raw, list(run, "runs"), paragraph, offset, visit);
                } else {
                    visit(paragraph, *offset, run);
                    *offset += run_chars(raw, run);
                }
            }
        }
        let mut offset = 0;
        for (p, paragraph) in list(story, "paragraphs").iter().enumerate() {
            rec(self.raw, list(paragraph, "runs"), p, &mut offset, visit);
            offset += 1;
        }
    }

    fn placed_at(&self, story: &str, offset: usize) -> Option<Placed> {
        let placed = self.placed(story);
        placed
            .iter()
            .find(|p| offset >= p.start && offset < p.end)
            .or_else(|| placed.last().filter(|p| p.end > p.start && offset == p.end))
            .cloned()
    }

    fn notes(&self) -> Rc<Vec<NoteInfo>> {
        if let Some(cached) = self.notes.borrow().as_ref() {
            return cached.clone();
        }
        let layout = self.raw.get("layout").unwrap_or(&Value::Null);
        let settings = layout.get("note_settings");
        let pick = |key: &str, default: &'static str| -> String {
            settings
                .and_then(|s| s.get(key))
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_string()
        };
        let restart = pick("restart", "document");
        let style = pick("style", "arabic");
        let foot_offset = layout
            .pointer("/book_numbering/footnote_offset")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let mut counters: HashMap<(String, String), usize> = HashMap::new();
        let mut out = Vec::new();
        for story in layout_list(self.raw, "stories") {
            let sid = id_of(story);
            let mut found: Vec<(usize, usize, &str)> = Vec::new();
            self.walk_story(story, &mut |p, offset, run| {
                if let Some(n) = run.get("note").and_then(Value::as_str) {
                    found.push((p, offset, n));
                }
            });
            for (paragraph, offset, id) in found {
                let Some(record) = layout_list(self.raw, "notes")
                    .iter()
                    .find(|r| id_of(r) == id)
                else {
                    self.diag(
                        "note-missing",
                        id,
                        format!(
                            "story {sid} paragraph {paragraph} uses note {id}, which has no record"
                        ),
                    );
                    continue;
                };
                let kind = record["kind"].as_str().unwrap_or("footnote").to_string();
                let at = self.placed_at(sid, offset);
                let page = at.as_ref().map(|p| p.page);
                let key = match (kind.as_str(), restart.as_str()) {
                    (_, "document") | ("endnote", "page") => String::new(),
                    (_, "story") => sid.to_string(),
                    (_, "section") => page.map_or(0, |p| self.section(p).0).to_string(),
                    _ => page.map_or_else(|| "?".to_string(), |p| p.to_string()),
                };
                let continued = kind == "footnote" && restart == "document";
                let counter = counters.entry((kind.clone(), key)).or_insert(0);
                *counter += 1;
                // A book continues footnote numbers across its members.
                let number = *counter + if continued { foot_offset } else { 0 };
                out.push(NoteInfo {
                    id: id.to_string(),
                    number,
                    label: number_label(number, &style),
                    kind,
                    story: sid.to_string(),
                    paragraph,
                    page,
                    frame: at.map(|p| p.frame),
                });
            }
        }
        let shared = Rc::new(out);
        *self.notes.borrow_mut() = Some(shared.clone());
        shared
    }

    /// Per-note rows plus per-page footnote budgets (estimated).
    fn note_plan(&self) -> (Vec<Value>, Vec<Value>) {
        let notes = self.notes();
        let mut used: HashMap<(usize, String), f64> = HashMap::new();
        let mut pages: BTreeMap<usize, (usize, f64, f64)> = BTreeMap::new();
        let mut rows = Vec::new();
        for info in notes.iter() {
            let record = layout_list(self.raw, "notes")
                .iter()
                .find(|r| id_of(r) == info.id);
            let mut row = json!({
                "id": info.id, "kind": info.kind, "number": info.number,
                "label": info.label, "story": info.story, "paragraph": info.paragraph,
                "page": info.page.map_or(Value::Null, |p| self.page_id(p)),
                "page_label": info.page.map_or(Value::Null, |p| json!(self.label(p))),
                "frame": info.frame,
            });
            if info.kind == "endnote" {
                row["status"] = json!("collected");
            } else if let (Some(page), Some(frame), Some(record)) =
                (info.page, info.frame.as_ref(), record)
            {
                let fb = crate::stories::frame_node(self.raw, frame).map(crate::stories::frame_box);
                let (size, leading) = story_ref(self.raw, &info.story)
                    .and_then(|s| list(s, "paragraphs").get(info.paragraph))
                    .map_or((12.0, 14.4), |p| {
                        crate::stories::paragraph_metrics(self.raw, p)
                    });
                let span = record
                    .get("span_columns")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let (width, budget) = fb.map_or((100.0, 0.0), |b| {
                    let w = if span { b.width } else { b.column_width };
                    (w.max(1.0), b.height * NOTE_AREA)
                });
                let lines: usize = list(record, "paragraphs")
                    .iter()
                    .map(|p| {
                        let chars: usize = list(p, "runs")
                            .iter()
                            .map(|r| r["text"].as_str().map_or(0, |t| t.chars().count()))
                            .sum();
                        (((chars as f64) * size * 0.5 / width).ceil() as usize).max(1)
                    })
                    .sum();
                let needed = lines as f64 * leading;
                let so_far = used.entry((page, frame.clone())).or_insert(0.0);
                let status = if *so_far + needed <= budget {
                    "fits"
                } else if *so_far < budget {
                    "splits"
                } else {
                    "moves"
                };
                *so_far += needed;
                let entry = pages.entry(page).or_insert((0, 0.0, 0.0));
                entry.0 += 1;
                entry.1 += needed;
                entry.2 = entry.2.max(budget);
                row["status"] = json!(status);
                row["estimated_height"] = json!((needed * 100.0).round() / 100.0);
            } else {
                row["status"] = json!("unplaced");
            }
            rows.push(row);
        }
        let summary = pages
            .into_iter()
            .map(|(page, (count, height, budget))| {
                json!({
                    "page": self.page_id(page), "label": self.label(page),
                    "footnotes": count,
                    "estimated_height": (height * 100.0).round() / 100.0,
                    "budget": (budget * 100.0).round() / 100.0,
                    "over_budget": height > budget,
                })
            })
            .collect();
        (rows, summary)
    }

    // -------------------------------------------------- generated stories

    fn source_stories(&self) -> Vec<&'a Value> {
        layout_list(self.raw, "stories")
            .iter()
            .filter(|s| !self.generated_stories.contains(id_of(s)))
            .collect()
    }

    fn caption_entries(&self, options: &Value) -> Vec<(usize, String)> {
        let source = options["source"].as_str().unwrap_or("paragraphs");
        let mut out: Vec<(usize, String)> = Vec::new();
        if source != "metadata" {
            if let Some(style) = options["style"].as_str() {
                for story in self.source_stories() {
                    let starts = self.paragraph_starts(story);
                    for (i, p) in list(story, "paragraphs").iter().enumerate() {
                        if p.get("style").and_then(Value::as_str) != Some(style) {
                            continue;
                        }
                        let page = self.page_of(id_of(story), starts[i]);
                        let text = self.paragraph_text(p, page, 0).trim().to_string();
                        if !text.is_empty() {
                            out.push((page.unwrap_or(usize::MAX), text));
                        }
                    }
                }
            }
        }
        if source != "paragraphs" {
            fn rec(nodes: &[Value], page: usize, out: &mut Vec<(usize, String)>) {
                for node in nodes {
                    let caption = node
                        .get("caption")
                        .and_then(Value::as_str)
                        .or_else(|| node.pointer("/metadata/caption").and_then(Value::as_str));
                    if let Some(c) = caption.map(str::trim).filter(|c| !c.is_empty()) {
                        out.push((page, c.to_string()));
                    }
                    rec(list(node, "children"), page, out);
                }
            }
            for (index, page) in list(self.raw, "pages").iter().enumerate() {
                for layer in list(page, "layers") {
                    rec(list(layer, "nodes"), index, &mut out);
                }
            }
        }
        out.sort_by_key(|(page, _)| *page);
        out
    }

    /// The paragraphs a generated record produces right now, and how many
    /// entries they hold.
    fn generate(&self, item: &Value) -> Result<(Vec<Value>, usize)> {
        let options = item.get("options").unwrap_or(&Value::Null);
        let kind = item["kind"].as_str().unwrap_or_default();
        let mut paragraphs: Vec<Value> = Vec::new();
        let page_label =
            |page: Option<usize>| page.map_or_else(|| "?".to_string(), |p| self.label(p));
        let count;
        match kind {
            "toc" => {
                let styles: Vec<&str> = list(options, "styles")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                ensure!(
                    !styles.is_empty(),
                    "[invalid-input] a table of contents needs paragraph styles; pass --styles h1,h2"
                );
                let mut entries = 0;
                for story in self.source_stories() {
                    let starts = self.paragraph_starts(story);
                    for (i, p) in list(story, "paragraphs").iter().enumerate() {
                        let Some(level) = p
                            .get("style")
                            .and_then(Value::as_str)
                            .and_then(|s| styles.iter().position(|x| *x == s))
                        else {
                            continue;
                        };
                        let page = self.page_of(id_of(story), starts[i]);
                        let text = self.paragraph_text(p, page, 0).trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        entries += 1;
                        paragraphs.push(para(
                            &format!("toc-{}", level + 1),
                            vec![
                                text_run(&text),
                                json!({"special": "tab"}),
                                text_run(&page_label(page)),
                            ],
                        ));
                    }
                }
                count = entries;
            }
            "captions" | "list-of-figures" => {
                let label = options["label"].as_str().unwrap_or("Figure");
                let entries = self.caption_entries(options);
                count = entries.len();
                for (n, (page, text)) in entries.into_iter().enumerate() {
                    let head = format!("{label} {}: {text}", n + 1);
                    if kind == "list-of-figures" {
                        let at = (page != usize::MAX).then_some(page);
                        paragraphs.push(para(
                            "figure-entry",
                            vec![
                                text_run(&head),
                                json!({"special": "tab"}),
                                text_run(&page_label(at)),
                            ],
                        ));
                    } else {
                        paragraphs.push(para("caption-entry", vec![text_run(&head)]));
                    }
                }
            }
            "index" => {
                let rule = options["page_range"].as_str().unwrap_or("contiguous");
                let language = options["language"].as_str().unwrap_or("en");
                let mut marks: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
                for story in self.source_stories() {
                    let sid = id_of(story);
                    let mut found: Vec<(usize, &str)> = Vec::new();
                    self.walk_story(story, &mut |_, offset, run| {
                        if let Some(t) = run.get("index").and_then(Value::as_str) {
                            found.push((offset, t));
                        }
                    });
                    for (offset, topic) in found {
                        match self.page_of(sid, offset) {
                            Some(page) => marks
                                .entry(topic.to_string())
                                .or_default()
                                .push((page, self.label(page))),
                            None => self.diag(
                                "index-unplaced",
                                topic,
                                format!(
                                    "a marker for topic {topic} in story {sid} is not placed on a page"
                                ),
                            ),
                        }
                    }
                }
                let mut rows: Vec<(String, String, String, String)> = Vec::new();
                for topic in layout_list(self.raw, "index_topics") {
                    let id = id_of(topic);
                    let text = topic["text"].as_str().unwrap_or_default();
                    let sort = topic["sort"].as_str().unwrap_or(text);
                    let body = if let Some(see) = topic.get("see").and_then(Value::as_str) {
                        let target = layout_list(self.raw, "index_topics")
                            .iter()
                            .find(|t| id_of(t) == see)
                            .and_then(|t| t["text"].as_str())
                            .unwrap_or("?");
                        format!("{text}. See {target}")
                    } else if let Some(pages) = marks.get(id) {
                        format!("{text}, {}", page_range(rule, pages))
                    } else {
                        continue;
                    };
                    rows.push((
                        collation_key(sort, language),
                        text.to_string(),
                        id.to_string(),
                        body,
                    ));
                }
                rows.sort();
                count = rows.len();
                let mut letter = String::new();
                for (key, _, _, body) in rows {
                    let first = heading_letter(&key, language);
                    if first != letter && !first.is_empty() {
                        paragraphs.push(para("index-letter", vec![text_run(&first)]));
                        letter = first;
                    }
                    paragraphs.push(para("index-entry", vec![text_run(&body)]));
                }
            }
            _ => {
                let mut entries = 0;
                let notes = self.notes();
                for info in notes.iter().filter(|n| n.kind == "endnote") {
                    let Some(record) = layout_list(self.raw, "notes")
                        .iter()
                        .find(|r| id_of(r) == info.id)
                    else {
                        continue;
                    };
                    entries += 1;
                    for (i, p) in list(record, "paragraphs").iter().enumerate() {
                        let body: String = list(p, "runs")
                            .iter()
                            .filter_map(|r| r["text"].as_str())
                            .collect();
                        let text = if i == 0 {
                            format!("{}. {body}", info.label)
                        } else {
                            body
                        };
                        paragraphs.push(para("endnote", vec![text_run(&text)]));
                    }
                }
                count = entries;
            }
        }
        if paragraphs.is_empty() {
            paragraphs.push(json!({"runs": []}));
        }
        Ok((paragraphs, count))
    }

    fn generated_state(&self, item: &Value) -> Result<Value> {
        if item.pointer("/options/scope").and_then(Value::as_str) == Some("book") {
            // Built from every member of a book; one member cannot recompute
            // it, so only hand edits are detectable here.
            let recorded = item.get("digest").and_then(Value::as_str);
            let current = story_ref(self.raw, item["story"].as_str().unwrap_or_default())
                .map(|s| digest_of(list(s, "paragraphs")));
            let edited = recorded.is_some() && current.as_deref() != recorded;
            let reason = match (recorded, edited) {
                (None, _) => "never-generated",
                (_, true) => "edited",
                _ => "book-scoped",
            };
            return Ok(json!({
                "id": item["id"], "kind": item["kind"], "story": item["story"],
                "entries": item.get("entries"), "digest": recorded,
                "stale": edited || recorded.is_none(), "reason": reason, "scope": "book",
            }));
        }
        let (paragraphs, entries) = self.generate(item)?;
        let now = digest_of(&paragraphs);
        let recorded = item.get("digest").and_then(Value::as_str);
        let current = story_ref(self.raw, item["story"].as_str().unwrap_or_default())
            .map(|s| digest_of(list(s, "paragraphs")));
        let (stale, reason) = match recorded {
            None => (true, "never-generated"),
            Some(d) if d != now => (true, "source-changed"),
            Some(d) if current.as_deref() != Some(d) => (true, "edited"),
            _ => (false, "current"),
        };
        Ok(json!({
            "id": item["id"], "kind": item["kind"], "story": item["story"],
            "entries": entries, "digest": recorded, "live_digest": now,
            "stale": stale, "reason": reason,
        }))
    }

    fn link_state(&self, link: &Value) -> (&'static str, String) {
        if link.get("url").is_some() {
            return ("external", "not fetched; checked for form only".into());
        }
        match self.resolve_target(link, 0) {
            Ok(_) => ("ok", String::new()),
            Err(reason) => ("broken", reason),
        }
    }

    fn report(&self) -> Value {
        let notes = self.notes();
        for record in layout_list(self.raw, "notes") {
            let id = id_of(record);
            if !notes.iter().any(|n| n.id == id) {
                self.diag(
                    "note-unused",
                    id,
                    format!("note {id} has no run in any story"),
                );
            }
        }
        for xref in layout_list(self.raw, "xrefs") {
            if let Err(reason) = self.xref(id_of(xref), 0) {
                self.diag("xref-unresolved", id_of(xref), reason);
            }
        }
        for link in layout_list(self.raw, "hyperlinks") {
            if let ("broken", reason) = self.link_state(link) {
                self.diag("link-broken", id_of(link), reason);
            }
        }
        for bookmark in layout_list(self.raw, "bookmarks") {
            if let Err(reason) = self.resolve_target(bookmark, 0) {
                self.diag("bookmark-broken", id_of(bookmark), reason);
            }
        }
        for story in layout_list(self.raw, "stories") {
            let sid = id_of(story);
            let mut found: Vec<(usize, &str, &str)> = Vec::new();
            self.walk_story(story, &mut |_, offset, run| {
                for key in ["variable", "index"] {
                    if let Some(id) = run.get(key).and_then(Value::as_str) {
                        found.push((offset, key, id));
                    }
                }
            });
            for (offset, key, id) in found {
                let page = self.page_of(sid, offset);
                if key == "variable" {
                    if let Err(reason) = self.variable(id, page) {
                        self.diag("variable-unresolved", id, reason);
                    }
                } else if page.is_none() && !self.generated_stories.contains(sid) {
                    self.diag(
                        "index-unplaced",
                        id,
                        format!("a marker for topic {id} in story {sid} is not placed on a page"),
                    );
                }
            }
        }
        for item in layout_list(self.raw, "generated") {
            if let Ok(state) = self.generated_state(item) {
                if state["stale"] == true {
                    self.diag(
                        "generated-stale",
                        id_of(item),
                        format!(
                            "generated {} {} is stale ({}); run `generate {}`",
                            item["kind"].as_str().unwrap_or_default(),
                            id_of(item),
                            state["reason"].as_str().unwrap_or_default(),
                            id_of(item)
                        ),
                    );
                }
            }
        }
        let items = self.take_diags();
        let problems = items.len();
        let (_, shown, has_more) = page_slice(items, 0, 1000);
        json!({
            "ok": problems == 0, "operation": "report", "estimated": true,
            "problems": problems, "has_more": has_more, "items": shown,
        })
    }
}

// ------------------------------------------------------------ actions

fn done(operation: &str, body: Value) -> Value {
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    out.insert("operation".into(), json!(operation));
    if let Value::Object(mut map) = body {
        out.append(&mut map);
    }
    Value::Object(out)
}

fn upsert(raw: &mut Value, key: &str, what: &str, item: Value, max: usize) -> Result<bool> {
    let id = id_of(&item).to_string();
    let items = array_mut(raw, key)?;
    if let Some(slot) = items.iter_mut().find(|v| id_of(v) == id) {
        *slot = item;
        return Ok(false);
    }
    ensure!(
        items.len() < max,
        "[limit-exceeded] {what}s per document: the limit is {max}"
    );
    items.push(item);
    Ok(true)
}

fn remove_record(raw: &mut Value, key: &str, what: &str, id: &str) -> Result<()> {
    let items = array_mut(raw, key)?;
    let before = items.len();
    items.retain(|v| id_of(v) != id);
    ensure!(
        items.len() < before,
        "[missing-resource] {what} not found: {id}"
    );
    Ok(())
}

fn require_unused(raw: &Value, key: &str, what: &str, id: &str) -> Result<()> {
    ensure!(
        !used_by_runs(raw, key, id),
        "[invalid-input] {what} {id} is still used by a run; remove the run or use the matching remove command"
    );
    Ok(())
}

fn check_page(raw: &Value, page: &str) -> Result<()> {
    ensure!(
        page_index(raw, page).is_some(),
        "[missing-resource] page not found: {page}"
    );
    Ok(())
}

fn check_paragraph(raw: &Value, story: &str, paragraph: usize) -> Result<()> {
    let source = story_ref(raw, story)
        .with_context(|| format!("[missing-resource] story not found: {story}"))?;
    let count = list(source, "paragraphs").len();
    ensure!(
        paragraph < count,
        "[missing-resource] story {story} has {count} paragraphs; paragraph {paragraph} does not exist"
    );
    Ok(())
}

/// A `story`+`paragraph`, `page` or `bookmark` target object from options.
fn target_object(raw: &Value, target: &Target) -> Result<Value> {
    let story = target.target_story.as_deref().zip(target.target_paragraph);
    let kinds = [
        story.is_some(),
        target.target_page.is_some(),
        target.target_bookmark.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    ensure!(
        kinds == 1,
        "[invalid-input] give exactly one target: --target-story with --target-paragraph, --target-page, or --target-bookmark"
    );
    if let Some((s, p)) = story {
        check_paragraph(raw, s, p)?;
        return Ok(json!({"story": s, "paragraph": p}));
    }
    if let Some(page) = &target.target_page {
        check_page(raw, page)?;
        return Ok(json!({"page": page}));
    }
    let b = target.target_bookmark.clone().unwrap_or_default();
    find(raw, "bookmarks", "bookmark", &b)?;
    Ok(json!({"bookmark": b}))
}

fn doc_for<'a>(raw: &'a Value, ctx: &'a Context, date: Option<&str>) -> Result<Doc<'a>> {
    Doc::new(raw, ctx, date)
}

/// Apply one long-document action to a v9 document in memory. Returns the
/// JSON result and whether the action left the document unchanged.
pub fn run(raw: &mut Value, action: DocAction, ctx: &Context) -> Result<(Value, bool)> {
    use DocAction as A;
    require_v9(raw)?;
    match action {
        A::NoteAdd {
            id,
            place,
            text,
            kind,
            span_columns,
        } => {
            check_id("note", &id)?;
            one_of(&kind, "note kind", NOTE_KINDS)?;
            let text = check_text(&text, "note text")?;
            ensure!(!text.is_empty(), "[invalid-input] note text is empty");
            let mut record = json!({
                "id": id, "kind": kind,
                "paragraphs": [{"runs": [{"text": text}]}],
            });
            if span_columns {
                ensure!(
                    kind == "footnote",
                    "[invalid-input] --span-columns applies to footnotes; endnotes are collected in their own story"
                );
                record["span_columns"] = json!(true);
            }
            push_unique(raw, "notes", "note", record, MAX_NOTES)?;
            insert_run(raw, &place, json!({"note": id}))?;
            Ok((
                done(
                    "note-add",
                    json!({"note": id, "kind": kind, "story": place.story, "paragraph": place.paragraph}),
                ),
                false,
            ))
        }
        A::NoteRemove { id } => {
            find(raw, "notes", "note", &id)?;
            let runs = remove_runs(raw, "note", &id, false);
            remove_record(raw, "notes", "note", &id)?;
            Ok((
                done("note-remove", json!({"note": id, "runs_removed": runs})),
                false,
            ))
        }
        A::NoteList { offset, limit } => {
            let doc = doc_for(raw, ctx, None)?;
            let (rows, _) = doc.note_plan();
            let mut out = paged(rows, offset, limit, "notes")?;
            out["estimated"] = json!(true);
            Ok((done("note-list", out), true))
        }
        A::NoteSettings { restart, style } => {
            let mut settings = raw
                .pointer("/layout/note_settings")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut changed = false;
            if let Some(r) = restart {
                one_of(&r, "note restart", RESTARTS)?;
                settings["restart"] = json!(r);
                changed = true;
            }
            if let Some(s) = style {
                one_of(&s, "note numbering style", NUMBER_STYLES)?;
                settings["style"] = json!(s);
                changed = true;
            }
            if changed {
                layout_mut(raw)?.insert("note_settings".into(), settings.clone());
            }
            Ok((
                done(
                    "note-settings",
                    json!({"settings": settings, "changed": changed}),
                ),
                !changed,
            ))
        }
        A::NotePlan => {
            let doc = doc_for(raw, ctx, None)?;
            let (rows, pages) = doc.note_plan();
            let count = |status: &str| rows.iter().filter(|r| r["status"] == status).count();
            let summary = json!({
                "notes": rows.len(), "fit": count("fits"), "split": count("splits"),
                "moved": count("moves"), "unplaced": count("unplaced"),
            });
            let (_, shown, has_more) = page_slice(rows, 0, 1000);
            Ok((
                done(
                    "note-plan",
                    json!({
                        "estimated": true, "summary": summary, "has_more": has_more,
                        "notes": shown, "pages": pages,
                        "diagnostics": doc.take_diags(),
                    }),
                ),
                true,
            ))
        }
        A::ConditionDefine { id, hidden } => {
            check_id("condition", &id)?;
            let created = upsert(
                raw,
                "conditions",
                "condition",
                json!({"id": id, "visible": !hidden}),
                MAX_CONDITIONS,
            )?;
            Ok((
                done(
                    "condition-define",
                    json!({"condition": id, "visible": !hidden, "created": created}),
                ),
                false,
            ))
        }
        A::ConditionRemove { id } => {
            find(raw, "conditions", "condition", &id)?;
            require_unused(raw, "condition", "condition", &id)?;
            for set in layout_list(raw, "condition_sets") {
                for key in ["show", "hide"] {
                    ensure!(
                        !list(set, key).iter().any(|c| c == id.as_str()),
                        "[invalid-input] condition {id} is used by condition set {}; redefine the set first",
                        id_of(set)
                    );
                }
            }
            remove_record(raw, "conditions", "condition", &id)?;
            Ok((done("condition-remove", json!({"condition": id})), false))
        }
        A::ConditionSetDefine { id, show, hide } => {
            check_id("condition set", &id)?;
            for c in show.iter().chain(hide.iter()) {
                find(raw, "conditions", "condition", c)?;
            }
            let created = upsert(
                raw,
                "condition_sets",
                "condition set",
                json!({"id": id, "show": show, "hide": hide}),
                MAX_CONDITIONS,
            )?;
            Ok((
                done(
                    "condition-set-define",
                    json!({"set": id, "created": created}),
                ),
                false,
            ))
        }
        A::ConditionActivate { set, none } => {
            ensure!(
                set.is_some() != none,
                "[invalid-input] give a condition set ID or --none"
            );
            let layout = layout_mut(raw)?;
            match &set {
                Some(id) => {
                    let known = list(&Value::Object(layout.clone()), "condition_sets")
                        .iter()
                        .any(|s| id_of(s) == id);
                    ensure!(known, "[missing-resource] condition set not found: {id}");
                    layout.insert("active_condition_set".into(), json!(id));
                }
                None => {
                    layout.remove("active_condition_set");
                }
            }
            Ok((done("condition-activate", json!({"active": set})), false))
        }
        A::ConditionWrap {
            place,
            condition,
            text,
        } => {
            find(raw, "conditions", "condition", &condition)?;
            let text = check_text(&text, "conditional text")?;
            ensure!(
                !text.is_empty(),
                "[invalid-input] conditional text is empty"
            );
            insert_run(
                raw,
                &place,
                json!({"condition": condition, "runs": [{"text": text}]}),
            )?;
            Ok((
                done(
                    "condition-wrap",
                    json!({"condition": condition, "story": place.story, "paragraph": place.paragraph}),
                ),
                false,
            ))
        }
        A::ConditionList => {
            let layout = raw.get("layout").unwrap_or(&Value::Null);
            let mut uses: HashMap<&str, usize> = HashMap::new();
            each_run(raw, |_, _, run| {
                if let Some(c) = run.get("condition").and_then(Value::as_str) {
                    *uses.entry(c).or_default() += 1;
                }
            });
            let conditions: Vec<Value> = list(layout, "conditions")
                .iter()
                .map(|c| {
                    json!({
                        "id": id_of(c),
                        "visible": condition_visible(raw, id_of(c)),
                        "default_visible": c.get("visible").and_then(Value::as_bool).unwrap_or(true),
                        "runs": uses.get(id_of(c)).copied().unwrap_or(0),
                    })
                })
                .collect();
            Ok((
                done(
                    "condition-list",
                    json!({
                        "conditions": conditions,
                        "sets": list(layout, "condition_sets"),
                        "active": layout.get("active_condition_set"),
                    }),
                ),
                true,
            ))
        }
        A::BookmarkAdd {
            id,
            name,
            story,
            paragraph,
            page,
        } => {
            check_id("bookmark", &id)?;
            let mut record = json!({"id": id});
            match (story, paragraph, page) {
                (Some(s), Some(p), None) => {
                    check_paragraph(raw, &s, p)?;
                    record["story"] = json!(s);
                    record["paragraph"] = json!(p);
                }
                (None, None, Some(page)) => {
                    check_page(raw, &page)?;
                    record["page"] = json!(page);
                }
                _ => bail!("[invalid-input] a bookmark needs --story with --paragraph, or --page"),
            }
            if let Some(name) = name {
                record["name"] = json!(check_text(&name, "bookmark name")?);
            }
            push_unique(raw, "bookmarks", "bookmark", record, MAX_BOOKMARKS)?;
            Ok((done("bookmark-add", json!({"bookmark": id})), false))
        }
        A::BookmarkRemove { id } => {
            find(raw, "bookmarks", "bookmark", &id)?;
            for (key, field) in [("hyperlinks", "bookmark"), ("xrefs", "target")] {
                for record in layout_list(raw, key) {
                    let points = match field {
                        "target" => record.pointer("/target/bookmark"),
                        f => record.get(f),
                    };
                    ensure!(
                        points.and_then(Value::as_str) != Some(id.as_str()),
                        "[invalid-input] bookmark {id} is the destination of {}; remove or retarget it first",
                        id_of(record)
                    );
                }
            }
            remove_record(raw, "bookmarks", "bookmark", &id)?;
            Ok((done("bookmark-remove", json!({"bookmark": id})), false))
        }
        A::BookmarkList { offset, limit } => {
            let doc = doc_for(raw, ctx, None)?;
            let rows: Vec<Value> = layout_list(raw, "bookmarks")
                .iter()
                .map(|b| {
                    let mut row = b.clone();
                    match doc.resolve_target(b, 0) {
                        Ok((_, page)) => {
                            row["status"] = json!("ok");
                            row["page_label"] = page.map_or(Value::Null, |p| json!(doc.label(p)));
                        }
                        Err(reason) => {
                            row["status"] = json!("broken");
                            row["reason"] = json!(reason);
                        }
                    }
                    row
                })
                .collect();
            Ok((
                done("bookmark-list", paged(rows, offset, limit, "bookmarks")?),
                true,
            ))
        }
        A::LinkAdd {
            id,
            place,
            text,
            url,
            page,
            bookmark,
        } => {
            check_id("hyperlink", &id)?;
            let given = [url.is_some(), page.is_some(), bookmark.is_some()]
                .iter()
                .filter(|b| **b)
                .count();
            ensure!(
                given == 1,
                "[invalid-input] a hyperlink needs exactly one of --url, --page, --bookmark"
            );
            let text = check_text(&text, "link text")?;
            ensure!(!text.is_empty(), "[invalid-input] link text is empty");
            check_place(raw, &place)?;
            let mut record = json!({"id": id});
            if let Some(url) = &url {
                check_url(url)?;
                record["url"] = json!(url);
            }
            if let Some(page) = &page {
                check_page(raw, page)?;
                record["page"] = json!(page);
            }
            if let Some(bookmark) = &bookmark {
                find(raw, "bookmarks", "bookmark", bookmark)?;
                record["bookmark"] = json!(bookmark);
            }
            push_unique(raw, "hyperlinks", "hyperlink", record, MAX_LINKS)?;
            insert_run(raw, &place, json!({"link": id, "runs": [{"text": text}]}))?;
            Ok((done("link-add", json!({"link": id})), false))
        }
        A::LinkRemove { id } => {
            find(raw, "hyperlinks", "hyperlink", &id)?;
            let runs = remove_runs(raw, "link", &id, true);
            remove_record(raw, "hyperlinks", "hyperlink", &id)?;
            Ok((
                done("link-remove", json!({"link": id, "runs_unwrapped": runs})),
                false,
            ))
        }
        A::LinkList { offset, limit } => {
            let doc = doc_for(raw, ctx, None)?;
            let rows: Vec<Value> = layout_list(raw, "hyperlinks")
                .iter()
                .map(|l| {
                    let (status, reason) = doc.link_state(l);
                    let mut row = l.clone();
                    row["status"] = json!(status);
                    if !reason.is_empty() {
                        row["reason"] = json!(reason);
                    }
                    row
                })
                .collect();
            Ok((
                done("link-list", paged(rows, offset, limit, "hyperlinks")?),
                true,
            ))
        }
        A::XrefAdd {
            id,
            place,
            target,
            format,
            template,
        } => {
            check_id("cross-reference", &id)?;
            one_of(&format, "cross-reference format", XREF_FORMATS)?;
            check_place(raw, &place)?;
            let target = target_object(raw, &target)?;
            let mut record = json!({"id": id, "target": target, "format": format});
            if format == "custom" {
                let template = template.context(
                    "[invalid-input] --format custom needs --template, e.g. \"see {text} on page {page}\"",
                )?;
                record["template"] = json!(check_text(&template, "template")?);
            } else {
                ensure!(
                    template.is_none(),
                    "[invalid-input] --template applies only to --format custom"
                );
            }
            push_unique(raw, "xrefs", "cross-reference", record, MAX_XREFS)?;
            insert_run(raw, &place, json!({"xref": id}))?;
            Ok((done("xref-add", json!({"xref": id})), false))
        }
        A::XrefRemove { id } => {
            find(raw, "xrefs", "cross-reference", &id)?;
            let runs = remove_runs(raw, "xref", &id, false);
            remove_record(raw, "xrefs", "cross-reference", &id)?;
            Ok((
                done("xref-remove", json!({"xref": id, "runs_removed": runs})),
                false,
            ))
        }
        A::XrefList { offset, limit } => {
            let doc = doc_for(raw, ctx, None)?;
            let rows: Vec<Value> = layout_list(raw, "xrefs")
                .iter()
                .map(|x| {
                    let mut row = x.clone();
                    match doc.xref(id_of(x), 0) {
                        Ok(text) => {
                            row["status"] = json!("ok");
                            row["text"] = json!(text);
                        }
                        Err(reason) => {
                            row["status"] = json!("broken");
                            row["text"] = json!("?");
                            row["reason"] = json!(reason);
                        }
                    }
                    row
                })
                .collect();
            let mut out = paged(rows, offset, limit, "xrefs")?;
            out["estimated"] = json!(true);
            Ok((done("xref-list", out), true))
        }
        A::VariableDefine {
            id,
            kind,
            style,
            pick,
            value,
            format,
        } => {
            check_id("variable", &id)?;
            one_of(&kind, "variable kind", VARIABLE_KINDS)?;
            let mut record = json!({"id": id, "kind": kind});
            for (key, v) in [
                ("style", style),
                ("pick", pick),
                ("value", value),
                ("format", format),
            ] {
                if let Some(v) = v {
                    record[key] = json!(v);
                }
            }
            let created = upsert(raw, "variables", "variable", record, MAX_VARIABLES)?;
            Ok((
                done(
                    "variable-define",
                    json!({"variable": id, "created": created}),
                ),
                false,
            ))
        }
        A::VariableRemove { id } => {
            find(raw, "variables", "variable", &id)?;
            require_unused(raw, "variable", "variable", &id)?;
            remove_record(raw, "variables", "variable", &id)?;
            Ok((done("variable-remove", json!({"variable": id})), false))
        }
        A::VariableInsert { id, place } => {
            find(raw, "variables", "variable", &id)?;
            insert_run(raw, &place, json!({"variable": id}))?;
            Ok((
                done(
                    "variable-insert",
                    json!({"variable": id, "story": place.story, "paragraph": place.paragraph}),
                ),
                false,
            ))
        }
        A::VariableList { date } => {
            let doc = doc_for(raw, ctx, date.as_deref())?;
            let rows: Vec<Value> = layout_list(raw, "variables")
                .iter()
                .map(|v| {
                    let kind = v["kind"].as_str().unwrap_or_default();
                    let mut row = v.clone();
                    if matches!(kind, "chapter-number" | "running-header") {
                        row["note"] = json!("depends on the page where it is used");
                    } else {
                        match doc.variable(id_of(v), None) {
                            Ok(value) => row["current"] = json!(value),
                            Err(reason) => row["unresolved"] = json!(reason),
                        }
                    }
                    row
                })
                .collect();
            Ok((done("variable-list", json!({"variables": rows})), true))
        }
        A::Text {
            story,
            date,
            offset,
            limit,
        } => {
            let source = story_ref(raw, &story)
                .with_context(|| format!("[missing-resource] story not found: {story}"))?;
            let doc = doc_for(raw, ctx, date.as_deref())?;
            let rows: Vec<Value> = list(source, "paragraphs")
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let page = doc.paragraph_page(source, i);
                    json!({
                        "paragraph": i,
                        "page": page.map_or(Value::Null, |p| json!(doc.label(p))),
                        "text": doc.paragraph_text(p, page, 0),
                    })
                })
                .collect();
            let mut out = paged(rows, offset, limit, "paragraphs")?;
            out["story"] = json!(story);
            out["estimated"] = json!(true);
            out["diagnostics"] = json!(doc.take_diags());
            Ok((done("text", out), true))
        }
        A::IndexTopic {
            id,
            text,
            sort,
            see,
        } => {
            check_id("index topic", &id)?;
            let text = check_text(&text, "topic text")?;
            let mut record = json!({"id": id, "text": text});
            if let Some(sort) = sort {
                record["sort"] = json!(check_text(&sort, "sort text")?);
            }
            if let Some(see) = see {
                ensure!(see != id, "[invalid-input] a topic cannot refer to itself");
                find(raw, "index_topics", "index topic", &see)?;
                record["see"] = json!(see);
            }
            let created = upsert(raw, "index_topics", "index topic", record, MAX_TOPICS)?;
            Ok((
                done("index-topic", json!({"topic": id, "created": created})),
                false,
            ))
        }
        A::IndexMark { topic, place } => {
            find(raw, "index_topics", "index topic", &topic)?;
            insert_run(raw, &place, json!({"index": topic}))?;
            Ok((
                done(
                    "index-mark",
                    json!({"topic": topic, "story": place.story, "paragraph": place.paragraph}),
                ),
                false,
            ))
        }
        A::IndexList => {
            let doc = doc_for(raw, ctx, None)?;
            let mut marks: BTreeMap<&str, Vec<String>> = BTreeMap::new();
            for story in layout_list(raw, "stories") {
                let sid = id_of(story);
                let mut found: Vec<(usize, &str)> = Vec::new();
                doc.walk_story(story, &mut |_, offset, run| {
                    if let Some(t) = run.get("index").and_then(Value::as_str) {
                        found.push((offset, t));
                    }
                });
                for (offset, topic) in found {
                    let label = doc
                        .page_of(sid, offset)
                        .map_or_else(|| "?".to_string(), |p| doc.label(p));
                    marks.entry(topic).or_default().push(label);
                }
            }
            let rows: Vec<Value> = layout_list(raw, "index_topics")
                .iter()
                .map(|t| {
                    let pages = marks.get(id_of(t)).cloned().unwrap_or_default();
                    let mut row = t.clone();
                    row["markers"] = json!(pages.len());
                    row["pages"] = json!(pages);
                    row
                })
                .collect();
            Ok((
                done("index-list", json!({"estimated": true, "topics": rows})),
                true,
            ))
        }
        A::GeneratedAdd {
            id,
            kind,
            story,
            styles,
            style,
            label,
            source,
            page_range,
            language,
        } => {
            check_id("generated item", &id)?;
            one_of(&kind, "generated kind", GENERATED_KINDS)?;
            check_id("story", &story)?;
            let mut options = Map::new();
            match kind.as_str() {
                "toc" => {
                    ensure!(
                        !styles.is_empty(),
                        "[invalid-input] a table of contents needs --styles h1,h2 (paragraph style IDs in level order)"
                    );
                    options.insert("styles".into(), json!(styles));
                }
                "captions" | "list-of-figures" => {
                    let source = source.unwrap_or_else(|| "paragraphs".into());
                    one_of(&source, "caption source", CAPTION_SOURCES)?;
                    ensure!(
                        source == "metadata" || style.is_some(),
                        "[invalid-input] captions from paragraphs need --style; use --source metadata for node captions"
                    );
                    if let Some(style) = style {
                        options.insert("style".into(), json!(style));
                    }
                    options.insert("source".into(), json!(source));
                    options.insert(
                        "label".into(),
                        json!(label.unwrap_or_else(|| "Figure".into())),
                    );
                }
                "index" => {
                    let rule = page_range.unwrap_or_else(|| "contiguous".into());
                    one_of(&rule, "page range rule", PAGE_RANGES)?;
                    options.insert("page_range".into(), json!(rule));
                    options.insert(
                        "language".into(),
                        json!(language.unwrap_or_else(|| "en".into())),
                    );
                }
                _ => {}
            }
            if story_ref(raw, &story).is_none() {
                array_mut(raw, "stories")?.push(json!({
                    "id": story, "paragraphs": [{"runs": []}],
                }));
            }
            push_unique(
                raw,
                "generated",
                "generated item",
                json!({"id": id, "kind": kind, "story": story, "options": options}),
                MAX_GENERATED,
            )?;
            Ok((
                done(
                    "generated-add",
                    json!({"generated": id, "kind": kind, "story": story, "note": "run `generate` to fill the story"}),
                ),
                false,
            ))
        }
        A::Generate { id } => {
            let item = find(raw, "generated", "generated item", &id)?.clone();
            ensure!(
                item.pointer("/options/scope").and_then(Value::as_str) != Some("book"),
                "[invalid-input] generated item {id} is book-wide; regenerate it with `pentool book toc` or `pentool book index`"
            );
            let (paragraphs, entries, diagnostics) = {
                let doc = doc_for(raw, ctx, None)?;
                let (paragraphs, entries) = doc.generate(&item)?;
                (paragraphs, entries, doc.take_diags())
            };
            let story = item["story"].as_str().unwrap_or_default().to_string();
            let target = raw
                .pointer_mut("/layout/stories")
                .and_then(Value::as_array_mut)
                .and_then(|s| s.iter_mut().find(|s| id_of(s) == story))
                .with_context(|| format!("[missing-resource] story not found: {story}"))?;
            let stored = digest_of(&paragraphs);
            target["paragraphs"] = json!(paragraphs);
            let record = array_mut(raw, "generated")?
                .iter_mut()
                .find(|g| id_of(g) == id)
                .context("generated item vanished")?;
            // The digest names the story content that was written, so a later
            // hand edit of the story is detected as `edited`.
            record["digest"] = json!(stored);
            record["entries"] = json!(entries);
            if let Some(map) = record.as_object_mut() {
                map.remove("stale");
            }
            Ok((
                done(
                    "generate",
                    json!({
                        "generated": id, "story": story, "entries": entries,
                        "digest": stored, "estimated": true, "diagnostics": diagnostics,
                    }),
                ),
                false,
            ))
        }
        A::GeneratedList => {
            let doc = doc_for(raw, ctx, None)?;
            let mut rows = Vec::new();
            for item in layout_list(raw, "generated") {
                rows.push(doc.generated_state(item)?);
            }
            let stale = rows.iter().filter(|r| r["stale"] == true).count();
            Ok((
                done(
                    "generated-list",
                    json!({"estimated": true, "stale": stale, "generated": rows}),
                ),
                true,
            ))
        }
        A::GeneratedCheck => {
            let states: Vec<Value> = {
                let doc = doc_for(raw, ctx, None)?;
                let mut states = Vec::new();
                for item in layout_list(raw, "generated") {
                    states.push(doc.generated_state(item)?);
                }
                states
            };
            let mut changed = false;
            for state in &states {
                let stale = state["stale"] == true;
                if let Some(record) = array_mut(raw, "generated")?
                    .iter_mut()
                    .find(|g| id_of(g) == state["id"].as_str().unwrap_or_default())
                {
                    let before = record.get("stale").and_then(Value::as_bool);
                    if before != Some(stale) {
                        record["stale"] = json!(stale);
                        changed = true;
                    }
                }
            }
            let stale = states.iter().filter(|s| s["stale"] == true).count();
            Ok((
                done(
                    "generated-check",
                    json!({"estimated": true, "stale": stale, "generated": states}),
                ),
                !changed,
            ))
        }
        A::GeneratedRemove { id } => {
            remove_record(raw, "generated", "generated item", &id)?;
            Ok((done("generated-remove", json!({"generated": id})), false))
        }
        A::Report => {
            let doc = doc_for(raw, ctx, None)?;
            Ok((doc.report(), true))
        }
    }
}

// ------------------------------------------------------------------ books

/// One member document's contribution to a book-wide TOC or index.
#[derive(Debug, Default, Clone)]
pub struct BookPart {
    /// Pages in the member.
    pub pages: usize,
    /// Headings as `(level, text, page label)` in story order.
    pub toc: Vec<(usize, String, String)>,
    /// Index topics defined in the member as `(id, text, sort, see)`.
    pub topics: Vec<(String, String, String, Option<String>)>,
    /// Index markers as `(topic, page position in the member, page label)`.
    pub marks: Vec<(String, usize, String)>,
    /// Unresolved references met while reading the member.
    pub diagnostics: Vec<Value>,
}

/// Read the headings and index markers of one v9 member document. The
/// member's own generated stories are excluded, exactly as for a single
/// document, and labels come from the member's computed page numbers.
pub fn book_part(raw: &Value, ctx: &Context, toc_styles: &[String]) -> Result<BookPart> {
    require_v9(raw)?;
    let doc = Doc::new(raw, ctx, None)?;
    let mut part = BookPart {
        pages: list(raw, "pages").len(),
        ..BookPart::default()
    };
    for story in doc.source_stories() {
        let starts = doc.paragraph_starts(story);
        if !toc_styles.is_empty() {
            for (i, p) in list(story, "paragraphs").iter().enumerate() {
                let Some(level) = p
                    .get("style")
                    .and_then(Value::as_str)
                    .and_then(|s| toc_styles.iter().position(|x| x == s))
                else {
                    continue;
                };
                let page = doc.page_of(id_of(story), starts[i]);
                let text = doc.paragraph_text(p, page, 0).trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let label = page.map_or_else(|| "?".to_string(), |p| doc.label(p));
                part.toc.push((level + 1, text, label));
            }
        }
        let sid = id_of(story);
        let mut found: Vec<(usize, &str)> = Vec::new();
        doc.walk_story(story, &mut |_, offset, run| {
            if let Some(t) = run.get("index").and_then(Value::as_str) {
                found.push((offset, t));
            }
        });
        for (offset, topic) in found {
            match doc.page_of(sid, offset) {
                Some(page) => part.marks.push((topic.to_string(), page, doc.label(page))),
                None => doc.diag(
                    "index-unplaced",
                    topic,
                    format!("a marker for topic {topic} in story {sid} is not placed on a page"),
                ),
            }
        }
    }
    for topic in layout_list(raw, "index_topics") {
        let text = topic["text"].as_str().unwrap_or_default().to_string();
        part.topics.push((
            id_of(topic).to_string(),
            text.clone(),
            topic["sort"].as_str().map_or(text, str::to_string),
            topic.get("see").and_then(Value::as_str).map(str::to_string),
        ));
    }
    part.diagnostics = doc.take_diags();
    Ok(part)
}

/// Chapters (sections, at least one) and footnotes a member contributes to a
/// book's continuing chapter and footnote numbers.
pub fn book_counts(raw: &Value, ctx: &Context) -> Result<(usize, usize)> {
    require_v9(raw)?;
    let doc = Doc::new(raw, ctx, None)?;
    let footnotes = doc.notes().iter().filter(|n| n.kind == "footnote").count();
    Ok((layout_list(raw, "sections").len().max(1), footnotes))
}

/// Book-wide TOC paragraphs in member order, and the entry count.
pub fn book_toc_paragraphs(parts: &[BookPart]) -> (Vec<Value>, usize) {
    let mut paragraphs = Vec::new();
    let mut entries = 0;
    for part in parts {
        for (level, text, label) in &part.toc {
            entries += 1;
            paragraphs.push(para(
                &format!("toc-{level}"),
                vec![text_run(text), json!({"special": "tab"}), text_run(label)],
            ));
        }
    }
    if paragraphs.is_empty() {
        paragraphs.push(json!({"runs": []}));
    }
    (paragraphs, entries)
}

/// Book-wide index paragraphs. Topics with the same ID merge across members
/// and page ranges use the position of the page within the whole book.
pub fn book_index_paragraphs(
    parts: &[BookPart],
    rule: &str,
    language: &str,
) -> Result<(Vec<Value>, usize)> {
    one_of(rule, "page range rule", PAGE_RANGES)?;
    let mut topics: BTreeMap<String, (String, String, Option<String>)> = BTreeMap::new();
    let mut marks: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    let mut offset = 0;
    for part in parts {
        for (id, text, sort, see) in &part.topics {
            topics
                .entry(id.clone())
                .or_insert_with(|| (text.clone(), sort.clone(), see.clone()));
        }
        for (topic, page, label) in &part.marks {
            marks
                .entry(topic.clone())
                .or_default()
                .push((offset + page, label.clone()));
        }
        offset += part.pages;
    }
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    for (id, (text, sort, see)) in &topics {
        let body = if let Some(see) = see {
            let target = topics.get(see).map_or("?", |t| t.0.as_str());
            format!("{text}. See {target}")
        } else if let Some(pages) = marks.get(id) {
            format!("{text}, {}", page_range(rule, pages))
        } else {
            continue;
        };
        rows.push((
            collation_key(sort, language),
            text.clone(),
            id.clone(),
            body,
        ));
    }
    rows.sort();
    let count = rows.len();
    let mut paragraphs = Vec::new();
    let mut letter = String::new();
    for (key, _, _, body) in rows {
        let first = heading_letter(&key, language);
        if first != letter && !first.is_empty() {
            paragraphs.push(para("index-letter", vec![text_run(&first)]));
            letter = first;
        }
        paragraphs.push(para("index-entry", vec![text_run(&body)]));
    }
    if paragraphs.is_empty() {
        paragraphs.push(json!({"runs": []}));
    }
    Ok((paragraphs, count))
}

/// Write book-generated paragraphs into a member's generated story, creating
/// the story and the record (scope `book`) when missing. Returns the digest.
pub fn store_book_generated(
    raw: &mut Value,
    id: &str,
    kind: &str,
    story: &str,
    options: Value,
    paragraphs: Vec<Value>,
    entries: usize,
) -> Result<String> {
    require_v9(raw)?;
    check_id("generated item", id)?;
    check_id("story", story)?;
    one_of(kind, "generated kind", GENERATED_KINDS)?;
    if story_ref(raw, story).is_none() {
        array_mut(raw, "stories")?.push(json!({"id": story, "paragraphs": [{"runs": []}]}));
    }
    let mut options = options;
    options["scope"] = json!("book");
    let existing = layout_list(raw, "generated").iter().any(|g| id_of(g) == id);
    if !existing {
        push_unique(
            raw,
            "generated",
            "generated item",
            json!({"id": id, "kind": kind, "story": story, "options": options}),
            MAX_GENERATED,
        )?;
    }
    let digest = digest_of(&paragraphs);
    let target = array_mut(raw, "stories")?
        .iter_mut()
        .find(|s| id_of(s) == story)
        .with_context(|| format!("[missing-resource] story not found: {story}"))?;
    target["paragraphs"] = json!(paragraphs);
    let record = array_mut(raw, "generated")?
        .iter_mut()
        .find(|g| id_of(g) == id)
        .context("generated item vanished")?;
    ensure!(
        record["kind"] == kind && record["story"] == story,
        "[invalid-input] generated item {id} already exists with another kind or story"
    );
    record["options"] = options;
    record["digest"] = json!(digest);
    record["entries"] = json!(entries);
    if let Some(map) = record.as_object_mut() {
        map.remove("stale");
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collation_tailoring_puts_swedish_letters_after_z() {
        assert!(collation_key("zebra", "sv") < collation_key("\u{e5}land", "sv"));
        assert!(collation_key("\u{e5}land", "sv") < collation_key("\u{e4}pple", "sv"));
        assert!(collation_key("\u{e5}land", "en") < collation_key("bat", "en"));
        assert_eq!(
            heading_letter(&collation_key("\u{e5}land", "sv"), "sv"),
            "\u{c5}"
        );
        assert_eq!(heading_letter(&collation_key("zebra", "sv"), "sv"), "Z");
    }

    #[test]
    fn page_ranges_follow_the_rule() {
        let pages: Vec<(usize, String)> = [0, 1, 2, 4, 4]
            .iter()
            .map(|p| (*p, (p + 1).to_string()))
            .collect();
        assert_eq!(page_range("first", &pages), "1");
        assert_eq!(page_range("all", &pages), "1, 2, 3, 5");
        assert_eq!(page_range("contiguous", &pages), "1\u{2013}3, 5");
        assert_eq!(page_range("all", &[]), "");
    }
}

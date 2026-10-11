//! Version 9 stories, text frames and threads (docs/layout-v1.md section 4).
//!
//! A story is text stored in `layout.stories`; a frame is an ordinary scene
//! node (`text-frame`) that shows part of one story. Threading, unthreading and
//! flowing only change frames and the thread, never the text, except where a
//! thread is deliberately split (`<story>-2`).
//!
//! Overset detection uses a provisional, deterministic estimator (average glyph
//! width of half the font size, one line per `leading`). Every figure it
//! produces carries `"estimated": true`; the paragraph composer replaces it.
use crate::page::PageAction;
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use unicode_normalization::UnicodeNormalization;

pub const MAX_STORIES: usize = 10_000;
pub const MAX_THREAD: usize = 4096;
pub const MAX_PARAGRAPHS: usize = 100_000;
pub const MAX_RUNS_PER_PARAGRAPH: usize = 10_000;
pub const MAX_STORY_CHARS: usize = 5_000_000;
pub const DEFAULT_MAX_PAGES: usize = 100;
const SUMMARY_OVERSET_LIMIT: usize = 20;
const MAX_PAGES: usize = crate::spreads::MAX_PAGES;
const MAX_ID_LEN: usize = 128;
const RUN_KINDS: [&str; 12] = [
    "text",
    "anchor",
    "note",
    "variable",
    "xref",
    "index",
    "link",
    "table",
    "condition",
    "special",
    "shy",
    "nobreak",
];
const SPECIALS: [&str; 17] = [
    "tab",
    "line-break",
    "column-break",
    "frame-break",
    "page-break",
    "em-space",
    "en-space",
    "thin-space",
    "hair-space",
    "flush-space",
    "right-indent-tab",
    "section-marker",
    "page-number",
    "next-page-number",
    "previous-page-number",
    "discretionary-hyphen",
    "nonbreaking-hyphen",
];
const VERTICAL_ALIGN: [&str; 4] = ["top", "center", "bottom", "justify"];
const FIRST_BASELINE: [&str; 5] = ["ascent", "cap-height", "leading", "x-height", "fixed"];
const AUTO_SIZE: [&str; 4] = ["off", "height", "width", "both"];
const FIT: [&str; 6] = [
    "fill",
    "fit",
    "fit-content-to-frame",
    "fit-frame-to-content",
    "center",
    "proportional",
];

// ------------------------------------------------------------------ helpers

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn pages(raw: &Value) -> &[Value] {
    list(raw, "pages")
}

fn check_id(what: &str, id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.chars().count() <= MAX_ID_LEN
            && !id.chars().any(|c| c.is_control() || c == '/' || c == '\\'),
        "[invalid-input] {what} ID {id:?} must be 1-{MAX_ID_LEN} characters without control characters, '/' or '\\'"
    );
    Ok(())
}

fn finite(value: Option<&Value>, what: &str, min: f64) -> Result<f64> {
    let n = value
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .with_context(|| format!("[malformed-resource] {what} must be a finite number"))?;
    ensure!(
        n >= min,
        "[malformed-resource] {what} must be at least {min}"
    );
    Ok(n)
}

fn one_of(value: Option<&Value>, what: &str, allowed: &[&str]) -> Result<()> {
    match value {
        None => Ok(()),
        Some(v) => {
            ensure!(
                v.as_str().is_some_and(|s| allowed.contains(&s)),
                "[malformed-resource] {what} must be one of {}",
                allowed.join(", ")
            );
            Ok(())
        }
    }
}

fn is_text_frame(node: &Value) -> bool {
    node.get("kind").and_then(Value::as_str) == Some("text-frame")
}

struct Frame<'a> {
    id: &'a str,
    page: usize,
    layer: &'a str,
    node: &'a Value,
}

fn walk<'a>(nodes: &'a [Value], page: usize, layer: &'a str, out: &mut Vec<Frame<'a>>) {
    for node in nodes {
        if is_text_frame(node) {
            out.push(Frame {
                id: node.get("id").and_then(Value::as_str).unwrap_or_default(),
                page,
                layer,
                node,
            });
        } else if node.get("kind").and_then(Value::as_str) == Some("group") {
            walk(list(node, "children"), page, layer, out);
        }
    }
}

/// Every text frame in document order (pages, layers, nodes).
fn frames(raw: &Value) -> Vec<Frame<'_>> {
    let mut out = Vec::new();
    for (page, value) in pages(raw).iter().enumerate() {
        for layer in list(value, "layers") {
            let id = layer.get("id").and_then(Value::as_str).unwrap_or_default();
            walk(list(layer, "nodes"), page, id, &mut out);
        }
    }
    out
}

fn story_list(raw: &Value) -> &[Value] {
    raw.get("layout")
        .map_or(&[], |layout| list(layout, "stories"))
}

fn find_story<'a>(raw: &'a Value, id: &str) -> Option<&'a Value> {
    story_list(raw).iter().find(|s| s["id"] == id)
}

fn require_v9(raw: &Value) -> Result<()> {
    ensure!(
        crate::spreads::is_v9(raw),
        "[unsupported-version] stories and frames need a version 9 document; run `pentool migrate --target 9` first"
    );
    Ok(())
}

// --------------------------------------------------------------- validation

/// Reject frame nodes in a document older than version 9.
pub fn reject_frames(raw: &Value) -> Result<()> {
    if let Some(frame) = frames(raw).first() {
        bail!(
            "[unsupported-capability] text frame {} needs a version 9 document",
            frame.id
        );
    }
    Ok(())
}

/// Per-node checks for `text-frame` and `graphic-frame` scene nodes.
pub fn validate_frame_node(o: &Map<String, Value>) -> Result<()> {
    let id = o.get("id").and_then(Value::as_str).unwrap_or_default();
    let kind = o.get("kind").and_then(Value::as_str).unwrap_or_default();
    let ctx = |what: &str| format!("{kind} {id}: {what}");
    for key in ["x", "y"] {
        finite(o.get(key), &ctx(key), f64::MIN)?;
    }
    for key in ["width", "height"] {
        finite(o.get(key), &ctx(key), 0.0)?;
    }
    if kind == "graphic-frame" {
        return one_of(o.get("fit"), &ctx("fit"), &FIT);
    }
    ensure!(
        o.get("story")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        "[malformed-resource] {}",
        ctx("story must be a nonempty story ID")
    );
    match o.get("next") {
        None | Some(Value::Null) => {}
        Some(Value::String(next)) => ensure!(
            !next.is_empty() && next != id,
            "[malformed-resource] {}",
            ctx("next must name a different frame")
        ),
        Some(_) => bail!(
            "[malformed-resource] {}",
            ctx("next must be a frame ID or null")
        ),
    }
    if let Some(columns) = o.get("columns") {
        ensure!(
            columns.as_u64().is_some_and(|c| (1..=40).contains(&c)),
            "[malformed-resource] {}",
            ctx("columns must be an integer 1-40")
        );
    }
    if o.contains_key("gutter") {
        finite(o.get("gutter"), &ctx("gutter"), 0.0)?;
    }
    if let Some(inset) = o.get("inset") {
        let values = inset.as_array().filter(|a| a.len() == 4);
        ensure!(
            values.is_some_and(|a| a
                .iter()
                .all(|n| n.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0))),
            "[malformed-resource] {}",
            ctx("inset must be four nonnegative numbers [top, right, bottom, left]")
        );
    }
    one_of(
        o.get("vertical_align"),
        &ctx("vertical_align"),
        &VERTICAL_ALIGN,
    )?;
    one_of(
        o.get("first_baseline"),
        &ctx("first_baseline"),
        &FIRST_BASELINE,
    )?;
    one_of(o.get("auto_size"), &ctx("auto_size"), &AUTO_SIZE)?;
    for key in ["ignore_wrap", "primary"] {
        if let Some(v) = o.get(key) {
            ensure!(
                v.is_boolean(),
                "[malformed-resource] {}",
                ctx(&format!("{key} must be true or false"))
            );
        }
    }
    for key in ["vertical_justify_max_spacing", "first_baseline_min"] {
        if o.contains_key(key) {
            finite(o.get(key), &ctx(key), 0.0)?;
        }
    }
    Ok(())
}

fn validate_text(text: &str, what: &str) -> Result<()> {
    ensure!(
        !text.chars().any(char::is_control),
        "[invalid-input] {what} contains a control character; use a special run for tabs and breaks"
    );
    Ok(())
}

fn validate_story(story: &Value) -> Result<usize> {
    let id = story.get("id").and_then(Value::as_str).unwrap_or_default();
    check_id("story", id)?;
    if let Some(language) = story.get("language") {
        ensure!(
            language
                .as_str()
                .is_some_and(|l| !l.is_empty() && l.len() <= 35),
            "[malformed-resource] story {id}: language must be a BCP-47 tag"
        );
    }
    let paragraphs = story
        .get("paragraphs")
        .and_then(Value::as_array)
        .with_context(|| format!("[malformed-resource] story {id}: paragraphs must be an array"))?;
    ensure!(
        paragraphs.len() <= MAX_PARAGRAPHS,
        "[limit-exceeded] story {id}: {} paragraphs exceeds the limit of {MAX_PARAGRAPHS}",
        paragraphs.len()
    );
    let mut characters = 0usize;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        let runs = paragraph
            .get("runs")
            .and_then(Value::as_array)
            .with_context(|| {
                format!("[malformed-resource] story {id} paragraph {index}: runs must be an array")
            })?;
        ensure!(
            runs.len() <= MAX_RUNS_PER_PARAGRAPH,
            "[limit-exceeded] story {id} paragraph {index}: {} runs exceeds the limit of {MAX_RUNS_PER_PARAGRAPH}",
            runs.len()
        );
        for run in runs {
            let object = run.as_object().with_context(|| {
                format!(
                    "[malformed-resource] story {id} paragraph {index}: a run must be an object"
                )
            })?;
            let kinds: Vec<&str> = RUN_KINDS
                .iter()
                .copied()
                .filter(|k| object.contains_key(*k))
                .collect();
            ensure!(
                kinds.len() == 1,
                "[malformed-resource] story {id} paragraph {index}: a run must be exactly one of {} (found {})",
                RUN_KINDS.join(", "),
                kinds.len()
            );
            match kinds[0] {
                "text" => {
                    let text = object["text"].as_str().with_context(|| {
                        format!("[malformed-resource] story {id} paragraph {index}: text must be a string")
                    })?;
                    validate_text(text, &format!("story {id} paragraph {index}"))?;
                    characters += text.chars().count();
                }
                "special" => {
                    ensure!(
                        object["special"]
                            .as_str()
                            .is_some_and(|s| SPECIALS.contains(&s)),
                        "[malformed-resource] story {id} paragraph {index}: unknown special character {}",
                        object["special"]
                    );
                    characters += 1;
                }
                _ => {}
            }
        }
        characters += 1;
        ensure!(
            characters <= MAX_STORY_CHARS,
            "[limit-exceeded] story {id}: more than {MAX_STORY_CHARS} characters"
        );
    }
    Ok(characters)
}

/// Structural checks of `layout.stories`, text frames and threads.
pub fn validate(raw: &Value) -> Result<()> {
    let stories = story_list(raw);
    ensure!(
        stories.len() <= MAX_STORIES,
        "[limit-exceeded] stories per document: {} exceeds the limit of {MAX_STORIES}",
        stories.len()
    );
    let mut ids = HashSet::new();
    for story in stories {
        validate_story(story)?;
        let id = story["id"].as_str().unwrap_or_default();
        ensure!(ids.insert(id), "[invalid-input] duplicate story ID: {id}");
    }
    let all = frames(raw);
    let mut by_id: HashMap<&str, &Frame> = HashMap::new();
    for frame in &all {
        ensure!(
            by_id.insert(frame.id, frame).is_none(),
            "[invalid-input] text frame ID {} is used more than once; frame IDs are unique in the document so threads can cross pages",
            frame.id
        );
    }
    let mut predecessor: HashMap<&str, &str> = HashMap::new();
    for frame in &all {
        let story = frame.node["story"].as_str().unwrap_or_default();
        ensure!(
            ids.contains(story),
            "[missing-resource] frame {} references missing story {story}",
            frame.id
        );
        if let Some(next) = frame.node.get("next").and_then(Value::as_str) {
            let target = by_id.get(next).with_context(|| {
                format!(
                    "[missing-resource] frame {} threads to missing frame {next}",
                    frame.id
                )
            })?;
            ensure!(
                target.node["story"] == frame.node["story"],
                "[invalid-input] frame {} (story {story}) cannot thread into frame {next} of story {}; a frame is never threaded into a different story's chain",
                frame.id,
                target.node["story"].as_str().unwrap_or_default()
            );
            ensure!(
                predecessor.insert(next, frame.id).is_none(),
                "[invalid-input] frame {next} has more than one predecessor"
            );
        }
    }
    let mut members: HashMap<&str, Vec<&Frame>> = HashMap::new();
    for frame in &all {
        members
            .entry(frame.node["story"].as_str().unwrap_or_default())
            .or_default()
            .push(frame);
    }
    let mut story_ids: Vec<&&str> = members.keys().collect();
    story_ids.sort();
    for story in story_ids {
        let group = &members[*story];
        ensure!(
            group.len() <= MAX_THREAD,
            "[limit-exceeded] thread of story {story}: {} frames exceeds the limit of {MAX_THREAD}",
            group.len()
        );
        let heads: Vec<&&Frame> = group
            .iter()
            .filter(|f| !predecessor.contains_key(f.id))
            .collect();
        ensure!(
            !heads.is_empty(),
            "[invalid-input] thread of story {story} is a cycle"
        );
        ensure!(
            heads.len() == 1,
            "[invalid-input] story {story} has {} separate chains of frames (heads: {}); thread them into one",
            heads.len(),
            heads.iter().map(|f| f.id).collect::<Vec<_>>().join(", ")
        );
        let mut length = 0;
        let mut at = Some(heads[0].id);
        while let Some(frame) = at {
            length += 1;
            ensure!(
                length <= group.len(),
                "[invalid-input] thread of story {story} is a cycle"
            );
            at = by_id[frame].node.get("next").and_then(Value::as_str);
        }
        ensure!(
            length == group.len(),
            "[invalid-input] thread of story {story} does not reach every frame"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- estimator

struct Paragraph {
    chars: usize,
    size: f64,
    leading: f64,
}

fn paragraph_style<'a>(raw: &'a Value, name: &str) -> Vec<&'a Value> {
    let styles = raw
        .get("layout")
        .and_then(|l| l.get("styles"))
        .and_then(|s| s.get("paragraph"));
    let mut chain = Vec::new();
    let mut at = Some(name.to_string());
    while let Some(current) = at {
        let Some(style) = styles.and_then(|s| s.get(&current)) else {
            break;
        };
        if chain.len() > 16 {
            break;
        }
        chain.push(style);
        at = style
            .get("based_on")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    chain
}

fn paragraphs_of(raw: &Value, story: &Value) -> Vec<Paragraph> {
    list(story, "paragraphs")
        .iter()
        .map(|p| {
            let chars = list(p, "runs")
                .iter()
                .map(|run| match run.get("text").and_then(Value::as_str) {
                    Some(text) => text.chars().count(),
                    None => usize::from(run.get("special").is_some()),
                })
                .sum();
            let chain = p
                .get("style")
                .and_then(Value::as_str)
                .map(|s| paragraph_style(raw, s))
                .unwrap_or_default();
            let overrides = p.get("overrides");
            let pick = |key: &str| {
                overrides
                    .and_then(|o| o.get(key))
                    .and_then(Value::as_f64)
                    .or_else(|| {
                        chain
                            .iter()
                            .find_map(|style| style.get(key).and_then(Value::as_f64))
                    })
                    .filter(|n| n.is_finite() && *n > 0.0)
            };
            let size = pick("size").unwrap_or(12.0);
            let leading = pick("leading").unwrap_or(size * 1.2);
            Paragraph {
                chars,
                size,
                leading,
            }
        })
        .collect()
}

#[derive(Default)]
struct Cursor {
    paragraph: usize,
    done: usize,
    offset: usize,
}

/// Fill one container (a column of `width` x `height`); returns the height used.
fn consume(paragraphs: &[Paragraph], cursor: &mut Cursor, width: f64, height: f64) -> f64 {
    let mut available = height;
    let mut used = 0.0;
    if width <= 0.0 || height <= 0.0 {
        return used;
    }
    while let Some(p) = paragraphs.get(cursor.paragraph) {
        let chars_per_line = ((width / (0.5 * p.size)).floor() as usize).max(1);
        let remaining = p.chars - cursor.done;
        let needed = remaining.div_ceil(chars_per_line).max(1);
        let fit = (available / p.leading).floor();
        if fit < 1.0 {
            break;
        }
        if fit >= needed as f64 {
            let height = needed as f64 * p.leading;
            available -= height;
            used += height;
            cursor.offset += remaining + 1;
            cursor.paragraph += 1;
            cursor.done = 0;
        } else {
            let lines = fit as usize;
            let taken = (lines * chars_per_line).min(remaining);
            cursor.done += taken;
            cursor.offset += taken;
            used += lines as f64 * p.leading;
            break;
        }
    }
    used
}

struct Geometry {
    columns: usize,
    column_width: f64,
    height: f64,
    auto_height: bool,
    inset_vertical: f64,
}

fn geometry(node: &Value) -> Geometry {
    let number = |key: &str, default: f64| node.get(key).and_then(Value::as_f64).unwrap_or(default);
    let columns = node.get("columns").and_then(Value::as_u64).unwrap_or(1) as usize;
    let gutter = number("gutter", 0.0);
    let inset: Vec<f64> = node
        .get("inset")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .filter(|a: &Vec<f64>| a.len() == 4)
        .unwrap_or_else(|| vec![0.0; 4]);
    let width = number("width", 0.0) - inset[1] - inset[3];
    let column_width = (width - gutter * (columns.saturating_sub(1)) as f64) / columns as f64;
    let auto_height = matches!(
        node.get("auto_size").and_then(Value::as_str),
        Some("height" | "both")
    );
    Geometry {
        columns,
        column_width,
        height: number("height", 0.0) - inset[0] - inset[2],
        auto_height,
        inset_vertical: inset[0] + inset[2],
    }
}

struct Flow {
    /// Per frame: characters (story offset) reached after filling it.
    reached: Vec<usize>,
    /// Per frame: height the text needed (auto-size height frames only).
    needed_height: Vec<Option<f64>>,
    overset: Option<Value>,
    characters: usize,
}

fn thread_of<'a>(frames: &[Frame<'a>], story: &str) -> Vec<&'a Value> {
    let members: HashMap<&str, &Frame> = frames
        .iter()
        .filter(|f| f.node["story"] == story)
        .map(|f| (f.id, f))
        .collect();
    let successors: HashSet<&str> = members
        .values()
        .filter_map(|f| f.node.get("next").and_then(Value::as_str))
        .collect();
    let mut order = Vec::new();
    let mut at = members.keys().copied().find(|id| !successors.contains(id));
    while let Some(id) = at {
        if order.len() >= members.len() {
            break;
        }
        let node = members[id].node;
        order.push(node);
        at = node.get("next").and_then(Value::as_str);
    }
    order
}

/// Every story's thread in one pass over the frames.
fn threads<'a>(frames: &[Frame<'a>]) -> HashMap<&'a str, Vec<&'a Value>> {
    let mut members: HashMap<&str, HashMap<&str, &Value>> = HashMap::new();
    for frame in frames {
        members
            .entry(frame.node["story"].as_str().unwrap_or_default())
            .or_default()
            .insert(frame.id, frame.node);
    }
    members
        .into_iter()
        .map(|(story, group)| {
            let successors: HashSet<&str> = group
                .values()
                .filter_map(|n| n.get("next").and_then(Value::as_str))
                .collect();
            let mut order = Vec::new();
            let mut at = group.keys().copied().find(|id| !successors.contains(id));
            while let Some(id) = at {
                if order.len() >= group.len() {
                    break;
                }
                let node = group[id];
                order.push(node);
                at = node.get("next").and_then(Value::as_str);
            }
            (story, order)
        })
        .collect()
}

fn flow(raw: &Value, story: &Value, thread: &[&Value]) -> Flow {
    let paragraphs = paragraphs_of(raw, story);
    let characters: usize = paragraphs.iter().map(|p| p.chars + 1).sum();
    let mut cursor = Cursor::default();
    let mut reached = Vec::new();
    let mut needed_height = Vec::new();
    for node in thread {
        let g = geometry(node);
        let mut used_total = 0.0;
        for _ in 0..g.columns {
            let height = if g.auto_height { 1.0e9 } else { g.height };
            used_total += consume(&paragraphs, &mut cursor, g.column_width, height);
            if cursor.paragraph >= paragraphs.len() {
                break;
            }
        }
        reached.push(cursor.offset);
        needed_height.push(g.auto_height.then_some(used_total + g.inset_vertical));
    }
    let overset = (cursor.paragraph < paragraphs.len() && !thread.is_empty()).then(|| {
        json!({
            "story": story["id"],
            "frame": thread.last().map(|n| n["id"].clone()),
            "offset": cursor.offset,
            "paragraph": cursor.paragraph,
            "characters_remaining": characters.saturating_sub(cursor.offset),
            "estimated": true,
        })
    });
    Flow {
        reached,
        needed_height,
        overset,
        characters,
    }
}

// ------------------------------------------------------------ node location

fn find_node_mut<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    for node in nodes.iter_mut() {
        if node.get("id").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            if let Some(found) = find_node_mut(children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn frame_mut<'a>(raw: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    for page in raw.get_mut("pages")?.as_array_mut()? {
        for layer in page.get_mut("layers")?.as_array_mut()? {
            if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                if let Some(node) = find_node_mut(nodes, id) {
                    return Some(node);
                }
            }
        }
    }
    None
}

fn remove_node(nodes: &mut Vec<Value>, id: &str) -> bool {
    if let Some(index) = nodes
        .iter()
        .position(|n| n.get("id").and_then(Value::as_str) == Some(id))
    {
        nodes.remove(index);
        return true;
    }
    nodes.iter_mut().any(|n| {
        n.get_mut("children")
            .and_then(Value::as_array_mut)
            .is_some_and(|c| remove_node(c, id))
    })
}

fn remove_frame_node(raw: &mut Value, id: &str) -> bool {
    let Some(pages) = raw.get_mut("pages").and_then(Value::as_array_mut) else {
        return false;
    };
    for page in pages {
        for layer in page
            .get_mut("layers")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                if remove_node(nodes, id) {
                    return true;
                }
            }
        }
    }
    false
}

fn stories_mut(raw: &mut Value) -> &mut Vec<Value> {
    let layout = raw
        .as_object_mut()
        .expect("document is an object")
        .entry("layout")
        .or_insert_with(|| json!({"engine": crate::spreads::ENGINE}));
    layout
        .as_object_mut()
        .expect("layout is an object")
        .entry("stories")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("stories is an array")
}

fn set_next(raw: &mut Value, frame: &str, next: Option<&str>) {
    if let Some(node) = frame_mut(raw, frame).and_then(Value::as_object_mut) {
        node.insert("next".into(), next.map_or(Value::Null, |n| json!(n)));
    }
}

fn set_story(raw: &mut Value, frame: &str, story: &str) {
    if let Some(node) = frame_mut(raw, frame).and_then(Value::as_object_mut) {
        node.insert("story".into(), json!(story));
    }
}

// -------------------------------------------------------- text split / join

/// Split a story's paragraphs at a character `offset` (paragraph separators
/// count one character). Runs keep their style and overrides on both sides.
fn split_paragraphs(paragraphs: &[Value], offset: usize) -> (Vec<Value>, Vec<Value>) {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let mut start = 0usize;
    for paragraph in paragraphs {
        let chars: usize = list(paragraph, "runs")
            .iter()
            .map(|run| match run.get("text").and_then(Value::as_str) {
                Some(t) => t.chars().count(),
                None => usize::from(run.get("special").is_some()),
            })
            .sum();
        let end = start + chars + 1;
        if offset >= end {
            head.push(paragraph.clone());
        } else if offset <= start {
            tail.push(paragraph.clone());
        } else {
            let mut at = start;
            let mut first = Vec::new();
            let mut second = Vec::new();
            for run in list(paragraph, "runs") {
                let length = match run.get("text").and_then(Value::as_str) {
                    Some(t) => t.chars().count(),
                    None => usize::from(run.get("special").is_some()),
                };
                let before = if length == 0 {
                    at < offset
                } else {
                    at + length <= offset
                };
                if before {
                    first.push(run.clone());
                } else if at >= offset {
                    second.push(run.clone());
                } else {
                    let text = run["text"].as_str().unwrap_or_default();
                    let cut = offset - at;
                    let left: String = text.chars().take(cut).collect();
                    let right: String = text.chars().skip(cut).collect();
                    let mut a = run.clone();
                    a["text"] = json!(left);
                    let mut b = run.clone();
                    b["text"] = json!(right);
                    first.push(a);
                    second.push(b);
                }
                at += length;
            }
            let mut a = paragraph.clone();
            a["runs"] = json!(first);
            let mut b = paragraph.clone();
            b["runs"] = json!(second);
            head.push(a);
            tail.push(b);
        }
        start = end;
    }
    (head, tail)
}

fn fresh_story_id(raw: &Value, base: &str) -> String {
    let taken: HashSet<&str> = story_list(raw)
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    let first = format!("{base}-2");
    if !taken.contains(first.as_str()) {
        return first;
    }
    (3..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c.as_str()))
        .expect("an unused story ID exists")
}

/// Split the thread of `story` after frame `after` (None: before every frame).
/// The text the earlier frames cannot show moves to a new story `<story>-2`,
/// which owns the frames after the cut. Returns the new story ID.
fn split_thread(raw: &mut Value, story: &str, after: Option<&str>) -> Result<Option<String>> {
    let (thread_ids, reached): (Vec<String>, Vec<usize>) = {
        let all = frames(raw);
        let thread = thread_of(&all, story);
        let source = find_story(raw, story).context("story is missing")?;
        let result = flow(raw, source, &thread);
        (
            thread
                .iter()
                .map(|n| n["id"].as_str().unwrap_or_default().to_string())
                .collect(),
            result.reached,
        )
    };
    let cut = match after {
        Some(frame) => thread_ids
            .iter()
            .position(|id| id == frame)
            .map(|i| i + 1)
            .with_context(|| format!("[missing-resource] frame {frame} is not in story {story}"))?,
        None => 0,
    };
    let downstream: Vec<String> = thread_ids[cut..].to_vec();
    let offset = if cut == 0 { 0 } else { reached[cut - 1] };
    let paragraphs = find_story(raw, story)
        .map(|s| list(s, "paragraphs").to_vec())
        .unwrap_or_default();
    let (head, tail) = split_paragraphs(&paragraphs, offset);
    if downstream.is_empty() && tail.is_empty() {
        return Ok(None);
    }
    let new_id = fresh_story_id(raw, story);
    let language = find_story(raw, story).and_then(|s| s.get("language").cloned());
    let mut created = json!({"id": new_id, "paragraphs": tail});
    if let Some(language) = language {
        created["language"] = language;
    }
    {
        let stories = stories_mut(raw);
        if let Some(original) = stories.iter_mut().find(|s| s["id"] == story) {
            original["paragraphs"] = json!(head);
        }
        stories.push(created);
    }
    for id in &downstream {
        set_story(raw, id, &new_id);
    }
    if cut > 0 {
        set_next(raw, &thread_ids[cut - 1], None);
    }
    Ok(Some(new_id))
}

// ------------------------------------------------------------------ reports

fn story_row(
    raw: &Value,
    story: &Value,
    thread: &[&Value],
    page_of: &HashMap<&str, usize>,
) -> Value {
    let id = story["id"].as_str().unwrap_or_default();
    let result = flow(raw, story, thread);
    let first = thread
        .first()
        .and_then(|n| page_of.get(n["id"].as_str().unwrap_or_default()))
        .map(|page| pages(raw)[*page]["id"].clone());
    json!({
        "id": id,
        "language": story.get("language"),
        "paragraphs": list(story, "paragraphs").len(),
        "characters": result.characters.saturating_sub(1),
        "frames": thread.len(),
        "thread": thread.iter().map(|n| n["id"].clone()).collect::<Vec<_>>(),
        "first_page": first,
        "placed": !thread.is_empty(),
        "overset": result.overset,
    })
}

/// Compact story/thread/overset totals for tree and search output.
pub fn summary(raw: &Value) -> Value {
    let all = frames(raw);
    let by_story = threads(&all);
    let stories = story_list(raw);
    let mut placed = 0;
    let mut overset_count = 0usize;
    let mut overset = Vec::new();
    for story in stories {
        let id = story["id"].as_str().unwrap_or_default();
        let Some(thread) = by_story.get(id).filter(|t| !t.is_empty()) else {
            continue;
        };
        placed += 1;
        if let Some(report) = flow(raw, story, thread).overset {
            overset_count += 1;
            if overset.len() < SUMMARY_OVERSET_LIMIT {
                overset.push(report);
            }
        }
    }
    json!({
        "stories": stories.len(),
        "placed": placed,
        "unplaced": stories.len() - placed,
        "frames": all.len(),
        "overset_count": overset_count,
        "overset_truncated": overset_count > overset.len(),
        "overset": overset,
    })
}

/// Number of stories whose estimated text does not fit their thread.
fn overset_count(raw: &Value) -> usize {
    summary(raw)["overset_count"].as_u64().unwrap_or(0) as usize
}

// ------------------------------------------------------------ page hooks

/// Remove the frames of a page that is being deleted, re-linking neighbors.
/// Returns a report; the caller deletes the page afterwards.
pub fn detach_page_frames(raw: &mut Value, page: &str, drop_story: bool) -> Result<Value> {
    let doomed: Vec<(String, String)> = frames(raw)
        .iter()
        .filter(|f| pages(raw)[f.page]["id"] == page)
        .map(|f| {
            (
                f.id.to_string(),
                f.node["story"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let mut rethreaded = Vec::new();
    let mut dropped = Vec::new();
    for (frame, story) in doomed {
        if dropped.contains(&story) {
            continue;
        }
        let mine = frames(raw)
            .iter()
            .filter(|f| f.node["story"] == story.as_str())
            .count();
        let on_page = frames(raw)
            .iter()
            .filter(|f| f.node["story"] == story.as_str() && pages(raw)[f.page]["id"] == page)
            .count();
        if on_page == mine {
            ensure!(
                drop_story,
                "[story-last-frame] page {page} holds every frame of story {story}; removing it would leave the story unplaced. Re-run with --drop-story to delete the story as well, or thread a frame onto another page first"
            );
            remove_frame_node(raw, &frame);
            // Remove the rest of this story's frames on the page too.
            let rest: Vec<String> = frames(raw)
                .iter()
                .filter(|f| f.node["story"] == story.as_str())
                .map(|f| f.id.to_string())
                .collect();
            for id in rest {
                remove_frame_node(raw, &id);
            }
            stories_mut(raw).retain(|s| s["id"] != story.as_str());
            dropped.push(story);
            continue;
        }
        let change = unlink_frame(raw, &frame)?;
        remove_frame_node(raw, &frame);
        rethreaded.push(change);
    }
    Ok(json!({"rethreaded": rethreaded, "stories_dropped": dropped}))
}

/// Re-link neighbors around `frame` (without deleting it); returns a report.
fn unlink_frame(raw: &mut Value, frame: &str) -> Result<Value> {
    let (story, previous, next) = {
        let all = frames(raw);
        let me = all
            .iter()
            .find(|f| f.id == frame)
            .with_context(|| format!("[missing-resource] text frame not found: {frame}"))?;
        let previous = all
            .iter()
            .find(|f| f.node.get("next").and_then(Value::as_str) == Some(frame))
            .map(|f| f.id.to_string());
        (
            me.node["story"].as_str().unwrap_or_default().to_string(),
            previous,
            me.node
                .get("next")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
    };
    if let Some(previous) = &previous {
        set_next(raw, previous, next.as_deref());
    }
    Ok(json!({"story": story, "frame": frame, "previous": previous, "next": next}))
}

/// Give copies of a page's frames fresh IDs and fresh stories: a duplicated
/// page never shares or silently forks a story.
pub fn duplicate_page_frames(raw: &mut Value, copy: &str) -> Result<Value> {
    let index = pages(raw)
        .iter()
        .position(|p| p["id"] == copy)
        .with_context(|| format!("[missing-resource] page not found: {copy}"))?;
    let originals: Vec<(String, String)> = {
        let mut out = Vec::new();
        for layer in list(&pages(raw)[index], "layers") {
            let mut found = Vec::new();
            walk(list(layer, "nodes"), index, "", &mut found);
            for f in found {
                out.push((
                    f.id.to_string(),
                    f.node["story"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }
        out
    };
    if originals.is_empty() {
        return Ok(json!({"duplicated_frames": []}));
    }
    // The copy still holds the original IDs; the original page holds them
    // too. Rename on the copy only.
    let mut taken: HashSet<String> = frames(raw).iter().map(|f| f.id.to_string()).collect();
    let mut story_map: HashMap<String, String> = HashMap::new();
    let mut previous_by_story: HashMap<String, String> = HashMap::new();
    let mut report = Vec::new();
    for (old, story) in originals {
        let new_id = (2..)
            .map(|n| format!("{old}-{n}"))
            .find(|c| !taken.contains(c))
            .expect("unused ID");
        taken.insert(new_id.clone());
        let new_story = match story_map.get(&story) {
            Some(s) => s.clone(),
            None => {
                let id = fresh_story_id(raw, &story);
                let language = find_story(raw, &story).and_then(|s| s.get("language").cloned());
                let mut created = json!({"id": id, "paragraphs": []});
                if let Some(language) = language {
                    created["language"] = language;
                }
                stories_mut(raw).push(created);
                story_map.insert(story.clone(), id.clone());
                id
            }
        };
        // The first match on the copy page is renamed (original IDs live
        // earlier in document order, so search only the copy's layers).
        let page = &mut raw["pages"][index];
        let mut renamed = false;
        for layer in page["layers"].as_array_mut().into_iter().flatten() {
            if let Some(nodes) = layer.get_mut("nodes").and_then(Value::as_array_mut) {
                if let Some(node) = find_node_mut(nodes, &old).and_then(Value::as_object_mut) {
                    node.insert("id".into(), json!(new_id));
                    node.insert("story".into(), json!(new_story));
                    node.insert("next".into(), Value::Null);
                    node.remove("primary");
                    renamed = true;
                    break;
                }
            }
        }
        if renamed {
            if let Some(prev) = previous_by_story.get(&new_story).cloned() {
                set_next(raw, &prev, Some(&new_id));
            }
            previous_by_story.insert(new_story.clone(), new_id.clone());
            report.push(json!({"frame": old, "copy": new_id, "story": new_story}));
        }
    }
    Ok(json!({"duplicated_frames": report}))
}

/// Counters for the page-operation reflow report.
pub fn reflow_counts(raw: &Value, stories_changed: usize) -> (usize, usize) {
    (stories_changed, overset_count(raw))
}

// ---------------------------------------------------------------- commands

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StoryAction {
    /// List stories with their thread and overset state (paginated).
    List {
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Show one story: paragraphs (truncated), thread and overset.
    Show { id: String },
    /// Create a story from text; each line becomes a paragraph.
    Add {
        id: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        language: Option<String>,
        /// Paragraph style applied to every paragraph.
        #[arg(long)]
        style: Option<String>,
    },
    /// Replace a story's text (every line becomes a paragraph) or language.
    Set {
        id: String,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        language: Option<String>,
        #[arg(long)]
        style: Option<String>,
    },
    /// Remove a story; refused while frames show it unless --with-frames.
    Remove {
        id: String,
        #[arg(long)]
        #[serde(default)]
        with_frames: bool,
    },
    /// Flow a story into its thread, adding pages while text is overset.
    Flow {
        id: String,
        /// Page that receives the first frame when the story is unplaced.
        #[arg(long)]
        page: Option<String>,
        #[arg(long)]
        layer: Option<String>,
        /// Parent for pages added by auto-flow (default: the last page's).
        #[arg(long)]
        parent: Option<String>,
        /// Upper bound on pages added (default 100, hard maximum 5000).
        #[arg(long)]
        max_pages: Option<usize>,
    },
}

fn default_limit() -> usize {
    100
}

#[derive(Debug, Clone, clap::Args, Serialize, Deserialize, Default)]
pub struct FrameOptions {
    #[arg(long)]
    pub x: Option<f64>,
    #[arg(long)]
    pub y: Option<f64>,
    #[arg(long)]
    pub width: Option<f64>,
    #[arg(long)]
    pub height: Option<f64>,
    #[arg(long)]
    pub columns: Option<u64>,
    #[arg(long)]
    pub gutter: Option<f64>,
    /// top,right,bottom,left
    #[arg(long)]
    pub inset: Option<String>,
    /// top, center, bottom or justify.
    #[arg(long)]
    pub vertical_align: Option<String>,
    #[arg(long)]
    pub vertical_justify_max_spacing: Option<f64>,
    /// ascent, cap-height, leading, x-height or fixed.
    #[arg(long)]
    pub first_baseline: Option<String>,
    #[arg(long)]
    pub first_baseline_min: Option<f64>,
    /// off, height, width or both.
    #[arg(long)]
    pub auto_size: Option<String>,
    #[arg(long)]
    pub ignore_wrap: Option<bool>,
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FrameAction {
    /// List text frames (paginated), optionally for one story or page.
    List {
        #[arg(long)]
        story: Option<String>,
        #[arg(long, default_value_t = 0)]
        #[serde(default)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Add a text frame to a story, inserted into its thread.
    Add {
        id: String,
        #[arg(long)]
        story: String,
        #[arg(long)]
        page: String,
        #[arg(long)]
        layer: String,
        /// Insert after this frame of the story (default: end of the thread).
        #[arg(long)]
        after: Option<String>,
        #[command(flatten)]
        options: FrameOptions,
    },
    /// Change frame options.
    Set {
        id: String,
        #[command(flatten)]
        options: FrameOptions,
    },
    /// Remove a frame; neighbors are re-linked unless --break-thread.
    Remove {
        id: String,
        /// Keep the frames after it as a new story <story>-2 with the overflow text.
        #[arg(long)]
        #[serde(default)]
        break_thread: bool,
        /// Allow removing a story's last frame by deleting the story too.
        #[arg(long)]
        #[serde(default)]
        drop_story: bool,
    },
    /// Thread frame `from` (a thread's last frame) into frame `to` (a thread's first).
    Thread { from: String, to: String },
    /// Cut the thread after a frame; the rest becomes story <story>-2.
    Unthread { id: String },
}

fn parse_inset(text: &str) -> Result<Value> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("[invalid-input] inset must be four numbers: {text:?}"))?;
    ensure!(
        parts.len() == 4,
        "[invalid-input] inset needs four numbers: top,right,bottom,left"
    );
    Ok(json!(parts))
}

fn apply_options(node: &mut Map<String, Value>, o: &FrameOptions) -> Result<()> {
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            node.insert(key.into(), value);
        }
    };
    put("x", o.x.map(|v| json!(v)));
    put("y", o.y.map(|v| json!(v)));
    put("width", o.width.map(|v| json!(v)));
    put("height", o.height.map(|v| json!(v)));
    put("columns", o.columns.map(|v| json!(v)));
    put("gutter", o.gutter.map(|v| json!(v)));
    put("inset", o.inset.as_deref().map(parse_inset).transpose()?);
    put(
        "vertical_align",
        o.vertical_align.as_ref().map(|v| json!(v)),
    );
    put(
        "vertical_justify_max_spacing",
        o.vertical_justify_max_spacing.map(|v| json!(v)),
    );
    put(
        "first_baseline",
        o.first_baseline.as_ref().map(|v| json!(v)),
    );
    put("first_baseline_min", o.first_baseline_min.map(|v| json!(v)));
    put("auto_size", o.auto_size.as_ref().map(|v| json!(v)));
    put("ignore_wrap", o.ignore_wrap.map(|v| json!(v)));
    Ok(())
}

fn paragraphs_from_text(text: &str, style: Option<&str>) -> Result<Vec<Value>> {
    let normalized: String = text.nfc().collect();
    ensure!(
        normalized.chars().count() <= MAX_STORY_CHARS,
        "[limit-exceeded] story text exceeds {MAX_STORY_CHARS} characters"
    );
    let mut out = Vec::new();
    for line in normalized.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        validate_text(line, "story text")?;
        let mut paragraph =
            json!({"runs": if line.is_empty() { json!([]) } else { json!([{"text": line}]) }});
        if let Some(style) = style {
            paragraph["style"] = json!(style);
        }
        out.push(paragraph);
    }
    ensure!(
        out.len() <= MAX_PARAGRAPHS,
        "[limit-exceeded] story paragraphs exceed {MAX_PARAGRAPHS}"
    );
    Ok(out)
}

fn page_index(raw: &Value, id: &str) -> Result<usize> {
    pages(raw)
        .iter()
        .position(|p| p["id"] == id)
        .with_context(|| format!("[missing-resource] page not found: {id}"))
}

/// Add a primary-style frame filling the page's margin box.
fn place_frame(
    raw: &mut Value,
    frame_id: &str,
    story: &str,
    page: &str,
    layer: Option<&str>,
    primary: bool,
) -> Result<Value> {
    let resolved = crate::spreads::resolve_page(raw, page)?;
    let margins: Vec<f64> = resolved["margins"]
        .as_array()
        .map(|m| m.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    ensure!(margins.len() == 4, "page margins are unavailable");
    let index = page_index(raw, page)?;
    let canvas = &pages(raw)[index]["canvas"];
    let width = canvas["width"].as_f64().unwrap_or(0.0);
    let height = canvas["height"].as_f64().unwrap_or(0.0);
    let node = json!({
        "kind": "text-frame",
        "id": frame_id,
        "story": story,
        "next": null,
        "x": margins[1],
        "y": margins[0],
        "width": (width - margins[1] - margins[3]).max(0.0),
        "height": (height - margins[0] - margins[2]).max(0.0),
        "columns": resolved["columns"],
        "gutter": resolved["gutter"],
        "inset": [0, 0, 0, 0],
        "primary": primary,
    });
    let layers = raw["pages"][index]["layers"]
        .as_array_mut()
        .context("page layers are missing")?;
    let target = match layer {
        Some(id) => layers
            .iter_mut()
            .find(|l| l["id"] == id)
            .with_context(|| format!("[missing-resource] layer not found on page {page}: {id}"))?,
        None => layers
            .iter_mut()
            .find(|l| !l["locked"].as_bool().unwrap_or(false))
            .with_context(|| format!("[locked] every layer on page {page} is locked"))?,
    };
    ensure!(
        !target["locked"].as_bool().unwrap_or(false),
        "[locked] layer {} on page {page} is locked",
        target["id"].as_str().unwrap_or_default()
    );
    target["nodes"]
        .as_array_mut()
        .context("layer nodes are missing")?
        .push(node.clone());
    Ok(node)
}

fn fresh_frame_id(raw: &Value, base: &str) -> String {
    let taken: HashSet<String> = frames(raw).iter().map(|f| f.id.to_string()).collect();
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c))
        .expect("unused ID")
}

fn fresh_page_id(raw: &Value) -> String {
    let taken: HashSet<&str> = pages(raw).iter().filter_map(|p| p["id"].as_str()).collect();
    (pages(raw).len() + 1..)
        .map(|n| format!("page-{n}"))
        .find(|c| !taken.contains(c.as_str()))
        .expect("unused ID")
}

/// Recompute auto-height frames after a mutation; returns the frames changed.
fn apply_auto_size(raw: &mut Value) -> Vec<String> {
    let mut updates: Vec<(String, f64)> = Vec::new();
    {
        let all = frames(raw);
        let by_story = threads(&all);
        for story in story_list(raw) {
            let id = story["id"].as_str().unwrap_or_default();
            let Some(thread) = by_story.get(id) else {
                continue;
            };
            if !thread.iter().any(|n| geometry(n).auto_height) {
                continue;
            }
            let result = flow(raw, story, thread);
            for (node, needed) in thread.iter().zip(result.needed_height) {
                if let Some(needed) = needed {
                    let current = node["height"].as_f64().unwrap_or(0.0);
                    let needed = needed.max(1.0);
                    if (needed - current).abs() > 1e-6 {
                        updates.push((node["id"].as_str().unwrap_or_default().to_string(), needed));
                    }
                }
            }
        }
    }
    let mut changed = Vec::new();
    for (id, height) in updates {
        if let Some(node) = frame_mut(raw, &id).and_then(Value::as_object_mut) {
            node.insert("height".into(), json!(height));
            changed.push(id);
        }
    }
    changed
}

fn finish(raw: &mut Value, mut result: Value) -> Result<Value> {
    let resized = apply_auto_size(raw);
    crate::scene::validate(raw)?;
    result["overset_count"] = json!(overset_count(raw));
    if !resized.is_empty() {
        result["auto_sized"] = json!(resized);
    }
    Ok(result)
}

pub fn run_story(raw: &mut Value, action: StoryAction) -> Result<Value> {
    require_v9(raw)?;
    match action {
        StoryAction::List { offset, limit } => {
            ensure!(
                (1..=1000).contains(&limit),
                "[invalid-input] --limit must be 1-1000"
            );
            let all = frames(raw);
            let by_story = threads(&all);
            let page_of: HashMap<&str, usize> = all.iter().map(|f| (f.id, f.page)).collect();
            let rows: Vec<Value> = story_list(raw)
                .iter()
                .skip(offset)
                .take(limit)
                .map(|s| {
                    let thread = by_story
                        .get(s["id"].as_str().unwrap_or_default())
                        .map_or(&[][..], Vec::as_slice);
                    story_row(raw, s, thread, &page_of)
                })
                .collect();
            let matches = story_list(raw).len();
            Ok(json!({
                "ok": true,
                "matches": matches,
                "returned": rows.len(),
                "offset": offset,
                "limit": limit,
                "has_more": offset + rows.len() < matches,
                "stories": rows,
            }))
        }
        StoryAction::Show { id } => {
            let story = find_story(raw, &id)
                .with_context(|| format!("[missing-resource] story not found: {id}"))?;
            let all = frames(raw);
            let by_story = threads(&all);
            let page_of: HashMap<&str, usize> = all.iter().map(|f| (f.id, f.page)).collect();
            let thread = by_story.get(id.as_str()).map_or(&[][..], Vec::as_slice);
            let mut row = story_row(raw, story, thread, &page_of);
            let preview: Vec<Value> = list(story, "paragraphs")
                .iter()
                .take(10)
                .map(|p| {
                    let text: String = list(p, "runs")
                        .iter()
                        .filter_map(|r| r.get("text").and_then(Value::as_str))
                        .collect();
                    json!({"style": p.get("style"), "text": text.chars().take(120).collect::<String>()})
                })
                .collect();
            row["preview"] = json!(preview);
            Ok(json!({"ok": true, "story": row}))
        }
        StoryAction::Add {
            id,
            text,
            language,
            style,
        } => {
            check_id("story", &id)?;
            ensure!(
                find_story(raw, &id).is_none(),
                "[invalid-input] story already exists: {id}"
            );
            ensure!(
                story_list(raw).len() < MAX_STORIES,
                "[limit-exceeded] stories per document: limit is {MAX_STORIES}"
            );
            let mut story =
                json!({"id": id, "paragraphs": paragraphs_from_text(&text, style.as_deref())?});
            if let Some(language) = language {
                story["language"] = json!(language);
            }
            stories_mut(raw).push(story);
            finish(
                raw,
                json!({"ok": true, "operation": "story-add", "story": id, "placed": false}),
            )
        }
        StoryAction::Set {
            id,
            text,
            language,
            style,
        } => {
            ensure!(
                text.is_some() || language.is_some() || style.is_some(),
                "[invalid-input] provide --text, --language or --style"
            );
            let paragraphs = text
                .as_deref()
                .map(|t| paragraphs_from_text(t, style.as_deref()))
                .transpose()?;
            let story = stories_mut(raw)
                .iter_mut()
                .find(|s| s["id"] == id.as_str())
                .with_context(|| format!("[missing-resource] story not found: {id}"))?;
            if let Some(paragraphs) = paragraphs {
                story["paragraphs"] = json!(paragraphs);
            } else if let Some(style) = &style {
                for p in story["paragraphs"].as_array_mut().into_iter().flatten() {
                    p["style"] = json!(style);
                }
            }
            if let Some(language) = language {
                story["language"] = json!(language);
            }
            finish(
                raw,
                json!({"ok": true, "operation": "story-set", "story": id}),
            )
        }
        StoryAction::Remove { id, with_frames } => {
            ensure!(
                find_story(raw, &id).is_some(),
                "[missing-resource] story not found: {id}"
            );
            let placed: Vec<String> = frames(raw)
                .iter()
                .filter(|f| f.node["story"] == id.as_str())
                .map(|f| f.id.to_string())
                .collect();
            ensure!(
                placed.is_empty() || with_frames,
                "[invalid-input] story {id} is shown by {} frame(s) ({}); re-run with --with-frames to delete them too",
                placed.len(),
                placed.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            );
            for frame in &placed {
                remove_frame_node(raw, frame);
            }
            stories_mut(raw).retain(|s| s["id"] != id.as_str());
            finish(
                raw,
                json!({"ok": true, "operation": "story-remove", "story": id, "frames_removed": placed}),
            )
        }
        StoryAction::Flow {
            id,
            page,
            layer,
            parent,
            max_pages,
        } => autoflow(
            raw,
            &id,
            page.as_deref(),
            layer.as_deref(),
            parent.as_deref(),
            max_pages,
        ),
    }
}

fn autoflow(
    raw: &mut Value,
    story_id: &str,
    page: Option<&str>,
    layer: Option<&str>,
    parent: Option<&str>,
    max_pages: Option<usize>,
) -> Result<Value> {
    let max_pages = max_pages.unwrap_or(DEFAULT_MAX_PAGES);
    ensure!(
        (1..=MAX_PAGES).contains(&max_pages),
        "[invalid-input] --max-pages must be 1-{MAX_PAGES}"
    );
    ensure!(
        find_story(raw, story_id).is_some(),
        "[missing-resource] story not found: {story_id}"
    );
    let mut frames_added: Vec<String> = Vec::new();
    let mut pages_added: Vec<String> = Vec::new();

    let has_frames = frames(raw).iter().any(|f| f.node["story"] == story_id);
    if !has_frames {
        let page = page.with_context(|| {
            format!("[invalid-input] story {story_id} is unplaced; pass --page to receive its first frame")
        })?;
        let frame_id = fresh_frame_id(raw, &format!("{story_id}-frame"));
        place_frame(raw, &frame_id, story_id, page, layer, true)?;
        frames_added.push(frame_id);
    }

    loop {
        let (overset, last, last_page) = {
            let all = frames(raw);
            let thread = thread_of(&all, story_id);
            let story = find_story(raw, story_id).context("story is missing")?;
            let result = flow(raw, story, &thread);
            let tail = all
                .iter()
                .find(|f| Some(f.id) == thread.last().and_then(|n| n["id"].as_str()))
                .context("thread has no frames")?;
            (
                result.overset,
                tail.id.to_string(),
                pages(raw)[tail.page]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        };
        let Some(overset) = overset else {
            break;
        };
        if pages_added.len() >= max_pages || pages(raw).len() >= MAX_PAGES {
            let remaining = overset["characters_remaining"].clone();
            let result = json!({
                "ok": true,
                "operation": "story-flow",
                "story": story_id,
                "pages_added": pages_added,
                "frames_added": frames_added,
                "autoflow_truncated": true,
                "characters_remaining": remaining,
                "max_pages": max_pages,
            });
            return finish(raw, result);
        }
        // A primary frame that cannot hold a single line never advances.
        let probe_page = &last_page;
        let resolved = crate::spreads::resolve_page(raw, probe_page)?;
        let canvas_height = pages(raw)[page_index(raw, probe_page)?]["canvas"]["height"]
            .as_f64()
            .unwrap_or(0.0);
        let margins: Vec<f64> = resolved["margins"]
            .as_array()
            .map(|m| m.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        let capacity =
            canvas_height - margins.first().unwrap_or(&0.0) - margins.get(2).unwrap_or(&0.0);
        let first_leading = {
            let story = find_story(raw, story_id).context("story is missing")?;
            let paragraphs = paragraphs_of(raw, story);
            let at = overset["paragraph"].as_u64().unwrap_or(0) as usize;
            paragraphs.get(at).map_or(12.0, |p| p.leading)
        };
        ensure!(
            capacity >= first_leading,
            "[autoflow-stalled] the page margin box of {probe_page} is {capacity} high, less than one line ({first_leading}); a primary frame there cannot advance story {story_id}. Reduce the margins or use a larger page; no pages were added"
        );
        let new_page = fresh_page_id(raw);
        let parent_for_page = parent.map(str::to_string).or_else(|| {
            pages(raw)
                .iter()
                .find(|p| p["id"] == last_page.as_str())
                .and_then(|p| p["layout"]["parent"].as_str().map(str::to_string))
        });
        crate::spreads::apply_page(
            raw,
            PageAction::Add {
                id: new_page.clone(),
                name: None,
                width: None,
                height: None,
                background: None,
                after: Some(last_page.clone()),
                before: None,
                parent: parent_for_page,
                count: None,
            },
        )?;
        let frame_id = fresh_frame_id(raw, &format!("{story_id}-frame"));
        let layer_for_page = pages(raw)[page_index(raw, &new_page)?]["layers"][0]["id"]
            .as_str()
            .map(str::to_string);
        place_frame(
            raw,
            &frame_id,
            story_id,
            &new_page,
            layer_for_page.as_deref(),
            true,
        )?;
        set_next(raw, &last, Some(&frame_id));
        pages_added.push(new_page);
        frames_added.push(frame_id);
        ensure!(
            frames(raw).iter().filter(|f| f.node["story"] == story_id).count() <= MAX_THREAD,
            "[limit-exceeded] thread of story {story_id} exceeds {MAX_THREAD} frames; raise nothing — split the story"
        );
    }
    finish(
        raw,
        json!({
            "ok": true,
            "operation": "story-flow",
            "story": story_id,
            "pages_added": pages_added,
            "frames_added": frames_added,
            "autoflow_truncated": false,
        }),
    )
}

fn frame_row(raw: &Value, frame: &Frame) -> Value {
    let node = frame.node;
    json!({
        "id": frame.id,
        "story": node["story"],
        "next": node.get("next").cloned().unwrap_or(Value::Null),
        "page": pages(raw)[frame.page]["id"],
        "layer": frame.layer,
        "bounds": [node["x"], node["y"], node["width"], node["height"]],
        "columns": node.get("columns").cloned().unwrap_or(json!(1)),
    })
}

pub fn run_frame(raw: &mut Value, action: FrameAction) -> Result<Value> {
    require_v9(raw)?;
    match action {
        FrameAction::List {
            story,
            offset,
            limit,
        } => {
            ensure!(
                (1..=1000).contains(&limit),
                "[invalid-input] --limit must be 1-1000"
            );
            let all = frames(raw);
            let matching: Vec<&Frame> = all
                .iter()
                .filter(|f| story.as_deref().is_none_or(|s| f.node["story"] == s))
                .collect();
            let rows: Vec<Value> = matching
                .iter()
                .skip(offset)
                .take(limit)
                .map(|f| frame_row(raw, f))
                .collect();
            Ok(json!({
                "ok": true,
                "matches": matching.len(),
                "returned": rows.len(),
                "offset": offset,
                "limit": limit,
                "has_more": offset + rows.len() < matching.len(),
                "frames": rows,
            }))
        }
        FrameAction::Add {
            id,
            story,
            page,
            layer,
            after,
            options,
        } => {
            check_id("frame", &id)?;
            ensure!(
                find_story(raw, &story).is_some(),
                "[missing-resource] story not found: {story}"
            );
            let all = frames(raw);
            ensure!(
                all.iter().all(|f| f.id != id),
                "[invalid-input] frame ID already exists: {id}"
            );
            let thread: Vec<String> = thread_of(&all, &story)
                .iter()
                .map(|n| n["id"].as_str().unwrap_or_default().to_string())
                .collect();
            ensure!(
                thread.len() < MAX_THREAD,
                "[limit-exceeded] thread of story {story} already has {MAX_THREAD} frames"
            );
            let position = match &after {
                Some(frame) => thread
                    .iter()
                    .position(|t| t == frame)
                    .map(|i| i + 1)
                    .with_context(|| {
                        format!("[invalid-input] --after {frame} is not a frame of story {story}")
                    })?,
                None => thread.len(),
            };
            let page = page_index(raw, &page)?;
            drop(all);
            let mut node = Map::new();
            node.insert("kind".into(), json!("text-frame"));
            node.insert("id".into(), json!(id));
            node.insert("story".into(), json!(story));
            node.insert("next".into(), Value::Null);
            for (key, default) in [
                ("x", 72.0),
                ("y", 72.0),
                ("width", 216.0),
                ("height", 360.0),
            ] {
                node.insert(key.into(), json!(default));
            }
            node.insert("columns".into(), json!(1));
            node.insert("gutter".into(), json!(12));
            node.insert("inset".into(), json!([0, 0, 0, 0]));
            apply_options(&mut node, &options)?;
            validate_frame_node(&node)?;
            let target = raw["pages"][page]["layers"]
                .as_array_mut()
                .and_then(|layers| layers.iter_mut().find(|l| l["id"] == layer.as_str()))
                .context("layer is missing")?;
            ensure!(
                !target["locked"].as_bool().unwrap_or(false),
                "[locked] layer {layer} is locked"
            );
            target["nodes"]
                .as_array_mut()
                .context("layer nodes are missing")?
                .push(Value::Object(node));
            // Splice into the thread at `position`.
            if position > 0 {
                let previous = thread[position - 1].clone();
                let following = thread.get(position).cloned();
                set_next(raw, &previous, Some(&id));
                set_next(raw, &id, following.as_deref());
            } else if let Some(head) = thread.first() {
                set_next(raw, &id, Some(head));
            }
            finish(
                raw,
                json!({
                    "ok": true,
                    "operation": "frame-add",
                    "frame": id,
                    "story": story,
                    "position": position,
                    "thread_length": thread.len() + 1,
                }),
            )
        }
        FrameAction::Set { id, options } => {
            let node = frame_mut(raw, &id)
                .and_then(Value::as_object_mut)
                .with_context(|| format!("[missing-resource] text frame not found: {id}"))?;
            let mut updated = node.clone();
            apply_options(&mut updated, &options)?;
            validate_frame_node(&updated)?;
            *node = updated;
            finish(
                raw,
                json!({"ok": true, "operation": "frame-set", "frame": id}),
            )
        }
        FrameAction::Remove {
            id,
            break_thread,
            drop_story,
        } => {
            let (story, count) = {
                let all = frames(raw);
                let me = all
                    .iter()
                    .find(|f| f.id == id)
                    .with_context(|| format!("[missing-resource] text frame not found: {id}"))?;
                let story = me.node["story"].as_str().unwrap_or_default().to_string();
                let count = all
                    .iter()
                    .filter(|f| f.node["story"] == story.as_str())
                    .count();
                (story, count)
            };
            if count == 1 {
                ensure!(
                    drop_story,
                    "[story-last-frame] frame {id} is the last frame of story {story}; deleting it would orphan the story. Re-run with --drop-story to delete the story as well"
                );
                remove_frame_node(raw, &id);
                stories_mut(raw).retain(|s| s["id"] != story.as_str());
                return finish(
                    raw,
                    json!({"ok": true, "operation": "frame-remove", "frame": id, "story": story, "story_dropped": true}),
                );
            }
            let mut result =
                json!({"ok": true, "operation": "frame-remove", "frame": id, "story": story});
            if break_thread {
                let (previous, next) = {
                    let all = frames(raw);
                    let me = all
                        .iter()
                        .find(|f| f.id == id)
                        .context("frame is missing")?;
                    (
                        all.iter()
                            .find(|f| {
                                f.node.get("next").and_then(Value::as_str) == Some(id.as_str())
                            })
                            .map(|f| f.id.to_string()),
                        me.node
                            .get("next")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    )
                };
                // Cut after the removed frame, then drop the frame itself.
                let created = split_thread(raw, &story, Some(&id))?;
                if let Some(previous) = &previous {
                    set_next(raw, previous, None);
                }
                remove_frame_node(raw, &id);
                result["previous"] = json!(previous);
                result["tail_story"] = json!(created);
                result["tail_head"] = json!(next);
            } else {
                let change = unlink_frame(raw, &id)?;
                remove_frame_node(raw, &id);
                result["rethreaded"] = change;
            }
            finish(raw, result)
        }
        FrameAction::Thread { from, to } => {
            let (from_story, to_story, from_next, to_prev) = {
                let all = frames(raw);
                let a = all
                    .iter()
                    .find(|f| f.id == from)
                    .with_context(|| format!("[missing-resource] text frame not found: {from}"))?;
                let b = all
                    .iter()
                    .find(|f| f.id == to)
                    .with_context(|| format!("[missing-resource] text frame not found: {to}"))?;
                (
                    a.node["story"].as_str().unwrap_or_default().to_string(),
                    b.node["story"].as_str().unwrap_or_default().to_string(),
                    a.node
                        .get("next")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    all.iter()
                        .any(|f| f.node.get("next").and_then(Value::as_str) == Some(to.as_str())),
                )
            };
            ensure!(
                from != to,
                "[invalid-input] a frame cannot thread into itself"
            );
            ensure!(
                from_next.is_none(),
                "[invalid-input] frame {from} already threads to {}; use `frame add --after {from}` to insert into a thread",
                from_next.unwrap_or_default()
            );
            ensure!(
                !to_prev,
                "[invalid-input] frame {to} already has a predecessor; unthread it first"
            );
            if from_story != to_story {
                // Merge the second chain's story into the first.
                let source = find_story(raw, &to_story)
                    .context("story is missing")?
                    .clone();
                {
                    let all = frames(raw);
                    let head = thread_of(&all, &to_story)
                        .first()
                        .map(|n| n["id"].as_str().unwrap_or_default().to_string());
                    ensure!(
                        head.as_deref() == Some(to.as_str()),
                        "[invalid-input] frame {to} is not the first frame of story {to_story}'s thread"
                    );
                }
                let moved: Vec<String> = {
                    let all = frames(raw);
                    thread_of(&all, &to_story)
                        .iter()
                        .map(|n| n["id"].as_str().unwrap_or_default().to_string())
                        .collect()
                };
                let extra = list(&source, "paragraphs").to_vec();
                let stories = stories_mut(raw);
                if let Some(target) = stories.iter_mut().find(|s| s["id"] == from_story.as_str()) {
                    target["paragraphs"]
                        .as_array_mut()
                        .context("paragraphs are missing")?
                        .extend(extra);
                }
                stories.retain(|s| s["id"] != to_story.as_str());
                for frame in &moved {
                    set_story(raw, frame, &from_story);
                }
            }
            set_next(raw, &from, Some(&to));
            finish(
                raw,
                json!({"ok": true, "operation": "frame-thread", "from": from, "to": to, "story": from_story, "merged_story": (from_story != to_story).then_some(to_story)}),
            )
        }
        FrameAction::Unthread { id } => {
            let story = {
                let all = frames(raw);
                let me = all
                    .iter()
                    .find(|f| f.id == id)
                    .with_context(|| format!("[missing-resource] text frame not found: {id}"))?;
                ensure!(
                    me.node.get("next").and_then(Value::as_str).is_some(),
                    "[invalid-input] frame {id} has no successor to unthread"
                );
                me.node["story"].as_str().unwrap_or_default().to_string()
            };
            let created = split_thread(raw, &story, Some(&id))?;
            finish(
                raw,
                json!({"ok": true, "operation": "frame-unthread", "frame": id, "story": story, "tail_story": created}),
            )
        }
    }
}

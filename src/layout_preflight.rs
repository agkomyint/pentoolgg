//! Layout preflight (v0.14.0 task 16): structured, read-only checks of a
//! version 9 document's composed text, fonts, links, generated stories,
//! frames, geometry, style discipline and accessibility metadata.
//!
//! Overset text is measured with the paragraph composer (`crate::typeset`),
//! not the task 3 estimator. Every issue names the page, frame, story and
//! character offset when they apply, plus a corrective action. The print
//! output checks stay in `pentool preflight` (v0.12.0).
use crate::longdoc::{self, Context as DocContext};
use crate::typeset;
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub const MAX_ISSUES: usize = 500;
pub const DEFAULT_MAX_OVERRIDE_PCT: f64 = 20.0;
/// Overrides are only judged once a story has at least this many paragraphs.
const MIN_PARAGRAPHS_FOR_OVERRIDES: usize = 5;

#[derive(Debug, Clone)]
pub struct Options {
    /// Text beyond the page edge by more than this many points is reported.
    pub bleed: f64,
    /// Percent of paragraphs with local overrides that triggers a warning.
    pub max_override_pct: f64,
    /// Report missing alternative text (images and graphic frames).
    pub require_alt: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bleed: 0.0,
            max_override_pct: DEFAULT_MAX_OVERRIDE_PCT,
            require_alt: true,
        }
    }
}

struct Sink {
    issues: Vec<Value>,
    total: usize,
}

impl Sink {
    fn push(&mut self, severity: &str, code: &str, message: String, action: &str, at: Value) {
        self.total += 1;
        if self.issues.len() >= MAX_ISSUES {
            return;
        }
        let mut issue = Map::new();
        issue.insert("severity".into(), json!(severity));
        issue.insert("code".into(), json!(code));
        if let Value::Object(map) = at {
            for (k, v) in map {
                issue.insert(k, v);
            }
        }
        issue.insert("message".into(), json!(message));
        issue.insert("action".into(), json!(action));
        self.issues.push(Value::Object(issue));
    }
}

fn page_id(raw: &Value, index: usize) -> Value {
    raw.pointer(&format!("/pages/{index}/id"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn number(node: &Value, key: &str) -> Option<f64> {
    node.get(key).and_then(Value::as_f64)
}

fn alt_of(node: &Value) -> Option<&str> {
    ["alt", "alt_text", "description"]
        .iter()
        .find_map(|k| node.get(*k).and_then(Value::as_str))
        .filter(|s| !s.trim().is_empty())
}

/// Run every layout check. `document` anchors linked-image paths.
pub fn run(raw: &Value, document: &Path, options: &Options) -> Result<Value> {
    if !crate::spreads::is_v9(raw) {
        bail!(
            "[unsupported-version] layout preflight needs a version 9 document; run `pentool migrate --target 9` first"
        );
    }
    if !(options.bleed.is_finite() && (0.0..=1000.0).contains(&options.bleed)) {
        bail!("[invalid-input] --bleed must be between 0 and 1000 points");
    }
    if !(options.max_override_pct.is_finite() && (0.0..=100.0).contains(&options.max_override_pct))
    {
        bail!("[invalid-input] --max-overrides must be between 0 and 100 percent");
    }
    let mut sink = Sink {
        issues: Vec::new(),
        total: 0,
    };
    let set = typeset::typeset(raw)?;

    // Overset text and flowed totals.
    for o in &set.overset {
        sink.push(
            "error",
            "text-overset",
            format!(
                "story {} has {} character(s) of overset text after frame {}",
                o.story, o.characters_remaining, o.frame
            ),
            "extend the thread, enlarge the frame, shrink the text or add pages (`pentool text FILE story flow`)",
            json!({"story": o.story, "frame": o.frame, "page": page_id(raw, o.page),
                   "paragraph": o.paragraph, "offset": o.offset}),
        );
    }

    // Fonts: substituted or unavailable.
    let mut seen = HashSet::new();
    for (family, reason) in &set.substituted {
        if seen.insert((family.clone(), reason.clone())) {
            sink.push(
                "warning",
                "font-substituted",
                format!("font {family} was substituted: {reason}"),
                "embed the font (`pentool text FILE font add`) or choose an available family",
                json!({"font": family}),
            );
        }
    }

    // Links: missing, modified (stale) or unsafe.
    if let Ok(report) = crate::linked::report(raw, document) {
        for dep in report["dependencies"].as_array().into_iter().flatten() {
            let status = dep["status"].as_str().unwrap_or("");
            let (severity, code, action) = match status {
                "missing-or-unsafe" => (
                    "error",
                    "link-missing",
                    "relink or restore the file (`pentool linked relink`)",
                ),
                "stale" => (
                    "warning",
                    "link-modified",
                    "update the placed image or relink it (`pentool linked`)",
                ),
                "not-checked-limit" => (
                    "warning",
                    "link-not-checked",
                    "reduce the number or size of linked files",
                ),
                _ => continue,
            };
            sink.push(
                severity,
                code,
                format!(
                    "linked image {} is {status}",
                    dep["asset"].as_str().unwrap_or("?")
                ),
                action,
                json!({"asset": dep["asset"], "nodes": dep["nodes"]}),
            );
        }
    }

    // Cross-references, links and generated stories.
    let ctx = DocContext {
        file_name: document
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
    };
    if let Ok((report, _)) = longdoc::run(&mut raw.clone(), longdoc::DocAction::Report, &ctx) {
        for item in report["items"].as_array().into_iter().flatten() {
            let code = item["code"].as_str().unwrap_or("reference-problem");
            let severity = if code == "note-unused" {
                "warning"
            } else {
                "error"
            };
            sink.push(
                severity,
                code,
                item["message"].as_str().unwrap_or_default().to_string(),
                "repair the reference or remove it (`pentool layout-doc FILE report`)",
                json!({"id": item["id"]}),
            );
        }
    }
    if let Ok((generated, _)) =
        longdoc::run(&mut raw.clone(), longdoc::DocAction::GeneratedList, &ctx)
    {
        for item in generated["generated"].as_array().into_iter().flatten() {
            if item["stale"] == true {
                sink.push(
                    "warning",
                    "generated-stale",
                    format!(
                        "generated {} {} is stale",
                        item["kind"].as_str().unwrap_or("story"),
                        item["id"].as_str().unwrap_or("?")
                    ),
                    "regenerate the table of contents, index or captions",
                    json!({"id": item["id"], "story": item["story"]}),
                );
            }
        }
    }

    // Frames, geometry and pasteboard.
    let frame_story: BTreeMap<String, String> = typeset::frame_list(raw)
        .iter()
        .filter_map(|(_, n)| {
            Some((
                n.get("id")?.as_str()?.to_string(),
                n.get("story")?.as_str()?.to_string(),
            ))
        })
        .collect();
    let mut with_text: HashSet<&str> = HashSet::new();
    for line in &set.lines {
        if !line.text.trim().is_empty() {
            with_text.insert(line.frame.as_str());
        }
    }
    let pages = raw["pages"].as_array().cloned().unwrap_or_default();
    for (index, page) in pages.iter().enumerate() {
        let w = page["canvas"]["width"].as_f64().unwrap_or(0.0);
        let h = page["canvas"]["height"].as_f64().unwrap_or(0.0);
        let page_name = page_id(raw, index);
        for layer in page["layers"].as_array().into_iter().flatten() {
            let mut stack: Vec<&Value> = layer["nodes"]
                .as_array()
                .map(|a| a.iter().collect())
                .unwrap_or_default();
            while let Some(node) = stack.pop() {
                let kind = node["kind"].as_str().unwrap_or("");
                let id = node["id"].as_str().unwrap_or("");
                if kind == "group" {
                    stack.extend(node["children"].as_array().into_iter().flatten());
                    continue;
                }
                if kind == "text-frame" && !with_text.contains(id) {
                    let story = frame_story.get(id).cloned().unwrap_or_default();
                    sink.push(
                        "warning",
                        "frame-empty",
                        format!("text frame {id} shows no text"),
                        "remove the frame, thread it, or add text to its story",
                        json!({"frame": id, "story": story, "page": page_name}),
                    );
                }
                if let (Some(x), Some(y), Some(fw), Some(fh)) = (
                    number(node, "x"),
                    number(node, "y"),
                    number(node, "width"),
                    number(node, "height"),
                ) {
                    if w > 0.0 && h > 0.0 && (x + fw <= 0.0 || y + fh <= 0.0 || x >= w || y >= h) {
                        sink.push(
                            "warning",
                            "pasteboard-object",
                            format!("{kind} {id} lies entirely outside page {}", page["id"]),
                            "move the object onto the page or delete it",
                            json!({"node": id, "page": page_name}),
                        );
                    }
                }
                if options.require_alt
                    && matches!(kind, "image" | "photo" | "raster" | "graphic-frame")
                    && alt_of(node).is_none()
                {
                    sink.push(
                        "warning",
                        "alt-text-missing",
                        format!("{kind} {id} has no alternative text"),
                        "set `alt` on the object, or mark it decorative with `\"decorative\": true`",
                        json!({"node": id, "page": page_name}),
                    );
                }
            }
        }
        // Text outside the page plus bleed.
        let mut reported: HashSet<&str> = HashSet::new();
        for line in set.page_lines(index) {
            if line.text.trim().is_empty() {
                continue;
            }
            let right = line.x + line.width.max(line.natural);
            let outside = line.x < -options.bleed
                || right > w + options.bleed
                || line.baseline - line.size > h + options.bleed
                || line.baseline < -options.bleed;
            if outside && reported.insert(line.frame.as_str()) {
                sink.push(
                    "error",
                    "text-outside-bleed",
                    format!(
                        "text of frame {} extends beyond the page edge plus {} pt of bleed",
                        line.frame, options.bleed
                    ),
                    "move or resize the frame so the text stays inside the page and bleed",
                    json!({"frame": line.frame, "story": line.story, "page": page_name,
                           "offset": line.story_offset + line.start}),
                );
            }
        }
    }

    // Local overrides per story.
    for story in raw
        .pointer("/layout/stories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let paragraphs = story["paragraphs"].as_array().cloned().unwrap_or_default();
        if paragraphs.len() < MIN_PARAGRAPHS_FOR_OVERRIDES {
            continue;
        }
        let overridden = paragraphs
            .iter()
            .filter(|p| {
                p.get("overrides")
                    .and_then(Value::as_object)
                    .is_some_and(|o| !o.is_empty())
                    || p.get("runs")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .any(|r| r.get("overrides").is_some())
            })
            .count();
        let pct = overridden as f64 * 100.0 / paragraphs.len() as f64;
        if pct > options.max_override_pct {
            sink.push(
                "warning",
                "style-overrides",
                format!(
                    "story {} has local overrides on {overridden} of {} paragraphs ({pct:.0}%)",
                    story["id"].as_str().unwrap_or("?"),
                    paragraphs.len()
                ),
                "move repeated formatting into paragraph or character styles",
                json!({"story": story["id"], "overridden": overridden,
                       "paragraphs": paragraphs.len(), "percent": (pct * 10.0).round() / 10.0}),
            );
        }
    }

    let errors = sink
        .issues
        .iter()
        .filter(|i| i["severity"] == "error")
        .count();
    let shown = sink.issues.len();
    Ok(json!({
        "operation": "layout-preflight",
        "ok": errors == 0,
        "errors": errors,
        "warnings": shown - errors,
        "issues_total": sink.total,
        "issues": sink.issues,
        "truncated": sink.total > shown,
        "options": {"bleed": options.bleed, "max_override_pct": options.max_override_pct,
                    "require_alt": options.require_alt},
        "composed": {"lines": set.lines.len(), "stories_overset": set.overset.len(),
                     "fonts": set.fonts.len()},
        "mutated": false,
    }))
}

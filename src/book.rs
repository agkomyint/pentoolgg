//! `.penbook` books: ordered member documents with continuous numbering,
//! synchronized styles and swatches, book-wide TOC and index, preflight,
//! packaging and one-file PDF/X export.
//!
//! Every operation plans in memory across all members first. Writes are
//! all-or-none: the pre-images are journaled under a hidden directory next to
//! the book before the first member changes, a failure restores every member
//! byte for byte, and an interrupted run is rolled back by the next command.

use crate::longdoc::{self, BookPart, Context as DocContext};
use crate::spreads::{self, SectionSettings};
use crate::transaction;
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_MEMBERS: usize = 500;
pub const MAX_TOTAL_BYTES: u64 = 2 << 30;
const MAX_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_NAME: usize = 256;
const MAX_PATH: usize = 1024;
const ROLES: &[&str] = &["front", "chapter", "back", "appendix", "index"];
const PAGE_NUMBERING: &[&str] = &["continuous", "restart-each"];
const CHAPTER_NUMBERING: &[&str] = &["continuous", "restart-each", "none"];
const FOOTNOTE_NUMBERING: &[&str] = &["per-document", "continuous"];
const DETAIL_CAP: usize = 200;
const PRINT_PAGE_CAP: usize = 200;

#[derive(Debug, Clone, Subcommand)]
pub enum BookAction {
    /// Create a new `.penbook` manifest.
    Init {
        /// Book name.
        #[arg(long)]
        name: String,
        /// Member document, relative to the book file (repeatable, in order).
        #[arg(long = "member")]
        members: Vec<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Members, page counts, labels, version and drift from the source.
    Status,
    /// Add a member document.
    Add {
        path: String,
        #[arg(long)]
        role: Option<String>,
        /// Zero-based position; the end when absent.
        #[arg(long)]
        at: Option<usize>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a member from the book (the file is untouched).
    Remove {
        path: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Move a member to a zero-based position.
    Move {
        path: String,
        #[arg(long)]
        to: usize,
        #[arg(long)]
        dry_run: bool,
    },
    /// Apply the manifest's page, chapter and footnote numbering to every member.
    Renumber {
        #[arg(long)]
        dry_run: bool,
    },
    /// Copy styles, swatches and parents from the source document.
    Sync {
        #[arg(long)]
        styles: bool,
        #[arg(long)]
        swatches: bool,
        #[arg(long)]
        parents: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate a book-wide table of contents into one member.
    Toc {
        /// Paragraph style IDs to list; the first is level 1 (repeatable).
        #[arg(long = "style", required = true)]
        styles: Vec<String>,
        /// Member that receives the generated story (default: the first).
        #[arg(long)]
        into: Option<String>,
        #[arg(long, default_value = "book-toc")]
        id: String,
        #[arg(long, default_value = "book-toc-story")]
        story: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate a book-wide index into one member.
    Index {
        /// Page range rule: first, contiguous or all.
        #[arg(long, default_value = "contiguous")]
        rule: String,
        #[arg(long, default_value = "en")]
        language: String,
        #[arg(long)]
        into: Option<String>,
        #[arg(long, default_value = "book-index")]
        id: String,
        #[arg(long, default_value = "book-index-story")]
        story: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Layout preflight of every member; `--print` adds the print preflight.
    Preflight {
        #[arg(long)]
        print: bool,
    },
    /// Copy the manifest, members and linked images into a new directory.
    Package { out: PathBuf },
    /// Export every page of every member as one PDF/X-4 draft.
    Export {
        out: PathBuf,
        #[arg(long, default_value = "pdf")]
        format: String,
        #[arg(long)]
        ppi: Option<f64>,
        #[arg(long)]
        title: Option<String>,
        /// Creation date `YYYY-MM-DDTHH:MM:SSZ` for reproducible output.
        #[arg(long)]
        created: Option<String>,
        #[arg(long)]
        overwrite: bool,
    },
}

// ------------------------------------------------------------- manifest

struct Member {
    rel: String,
    path: PathBuf,
}

struct Book {
    path: PathBuf,
    manifest: Value,
    members: Vec<Member>,
}

fn check_member_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= MAX_PATH,
        "[invalid-input] member path must be 1-{MAX_PATH} bytes"
    );
    let bytes = path.as_bytes();
    ensure!(
        !path.contains('\0')
            && !path.contains('\\')
            && !path.starts_with('/')
            && !(bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()),
        "[unsafe-path] member path {path:?} must be relative with `/` separators and no drive prefix"
    );
    ensure!(
        !path
            .split('/')
            .any(|s| s == ".." || s == "." || s.is_empty()),
        "[unsafe-path] member path {path:?} must not contain empty, `.` or `..` segments"
    );
    ensure!(
        path.ends_with(".pen") && path.len() > 4,
        "[invalid-input] member path {path:?} must name a .pen document"
    );
    Ok(())
}

fn validate_manifest(m: &Value) -> Result<()> {
    let object = m
        .as_object()
        .context("[malformed-resource] a .penbook must be a JSON object")?;
    ensure!(
        object.get("format").and_then(Value::as_str) == Some("penbook"),
        "[malformed-resource] format must be \"penbook\""
    );
    ensure!(
        object.get("version").and_then(Value::as_u64) == Some(1),
        "[unsupported-version] only .penbook version 1 is supported"
    );
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .context("[malformed-resource] book name is required")?;
    ensure!(
        name.len() <= MAX_NAME,
        "[invalid-input] book name exceeds {MAX_NAME} bytes"
    );
    let docs = object
        .get("documents")
        .and_then(Value::as_array)
        .context("[malformed-resource] documents must be an array")?;
    ensure!(
        !docs.is_empty() && docs.len() <= MAX_MEMBERS,
        "[limit-exceeded] a book lists 1-{MAX_MEMBERS} documents, found {}",
        docs.len()
    );
    let mut seen = BTreeSet::new();
    for (i, d) in docs.iter().enumerate() {
        let path = d
            .get("path")
            .and_then(Value::as_str)
            .with_context(|| format!("[malformed-resource] documents[{i}].path is required"))?;
        check_member_path(path)?;
        ensure!(
            seen.insert(path.to_ascii_lowercase()),
            "[invalid-input] member listed twice: {path}"
        );
        if let Some(role) = d.get("role") {
            ensure!(
                role.as_str().is_some_and(|r| ROLES.contains(&r)),
                "[invalid-input] documents[{i}].role must be one of {}",
                ROLES.join(", ")
            );
        }
    }
    if let Some(n) = object.get("numbering") {
        let n = n
            .as_object()
            .context("[malformed-resource] numbering must be an object")?;
        for (key, allowed) in [
            ("page", PAGE_NUMBERING),
            ("chapter", CHAPTER_NUMBERING),
            ("footnote", FOOTNOTE_NUMBERING),
        ] {
            if let Some(v) = n.get(key) {
                ensure!(
                    v.as_str().is_some_and(|s| allowed.contains(&s)),
                    "[invalid-input] numbering.{key} must be one of {}",
                    allowed.join(", ")
                );
            }
        }
    }
    if let Some(s) = object.get("synchronize") {
        let s = s
            .as_object()
            .context("[malformed-resource] synchronize must be an object")?;
        if let Some(source) = s.get("source") {
            let source = source
                .as_str()
                .context("[malformed-resource] synchronize.source must be a string")?;
            ensure!(
                docs.iter().any(|d| d["path"] == source),
                "[invalid-input] synchronize.source {source:?} is not a listed member"
            );
        }
        for key in ["styles", "swatches", "parents"] {
            if let Some(v) = s.get(key) {
                ensure!(
                    v.is_boolean(),
                    "[invalid-input] synchronize.{key} must be a boolean"
                );
            }
        }
    }
    Ok(())
}

fn journal_dir(book: &Path) -> PathBuf {
    let name = book
        .file_name()
        .map_or_else(|| "book".into(), |n| n.to_string_lossy().into_owned());
    book.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join(format!(".{name}.journal"))
}

/// Roll back an interrupted book commit. Returns the restored member paths.
fn recover(book: &Path) -> Result<Vec<String>> {
    let dir = journal_dir(book);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut restored = Vec::new();
    let manifest = dir.join("journal.json");
    if let Ok(bytes) = fs::read(&manifest) {
        let journal: Value = serde_json::from_slice(&bytes)
            .context("[malformed-resource] unreadable book journal")?;
        let root = book.parent().unwrap_or(Path::new("."));
        for entry in journal["members"].as_array().into_iter().flatten() {
            let rel = entry["path"].as_str().unwrap_or_default();
            check_member_path(rel)?;
            let target = root.join(rel);
            let before = fs::read(dir.join(entry["before"].as_str().unwrap_or_default()))
                .with_context(|| format!("[missing-resource] journal pre-image for {rel}"))?;
            let current = fs::read(&target).ok();
            if current.as_deref() != Some(before.as_slice()) {
                crate::editing::atomic_write(&target, &before)?;
                restored.push(rel.to_string());
            }
        }
    }
    fs::remove_dir_all(&dir).context("could not remove the book journal")?;
    Ok(restored)
}

fn load(book_path: &Path) -> Result<(Book, Vec<String>)> {
    let recovered = recover(book_path)?;
    let meta = fs::metadata(book_path)
        .with_context(|| format!("[missing-resource] could not read {}", book_path.display()))?;
    ensure!(
        meta.len() <= MAX_MANIFEST_BYTES,
        "[limit-exceeded] .penbook exceeds {MAX_MANIFEST_BYTES} bytes"
    );
    let manifest: Value = serde_json::from_slice(&fs::read(book_path)?)
        .context("[malformed-resource] invalid .penbook JSON")?;
    validate_manifest(&manifest)?;
    let parent = book_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let root = parent
        .canonicalize()
        .with_context(|| format!("could not resolve {}", parent.display()))?;
    let mut members = Vec::new();
    for d in manifest["documents"].as_array().into_iter().flatten() {
        let rel = d["path"].as_str().unwrap_or_default().to_string();
        let path = root.join(&rel);
        if let Ok(real) = path.canonicalize() {
            ensure!(
                real.starts_with(&root),
                "[unsafe-path] member {rel} resolves outside the book directory"
            );
        }
        members.push(Member { rel, path });
    }
    Ok((
        Book {
            path: book_path.to_path_buf(),
            manifest,
            members,
        },
        recovered,
    ))
}

struct Loaded {
    rel: String,
    path: PathBuf,
    bytes: Vec<u8>,
    value: Value,
}

/// Read every member, aborting with the full list when any is missing or
/// unreadable. Members are never skipped.
fn read_members(book: &Book) -> Result<Vec<Loaded>> {
    let mut problems = Vec::new();
    let mut total = 0u64;
    let mut out = Vec::new();
    for m in &book.members {
        match fs::metadata(&m.path) {
            Err(_) => {
                problems.push(format!("{}: missing", m.rel));
                continue;
            }
            Ok(meta) => {
                total += meta.len();
                if total > MAX_TOTAL_BYTES {
                    bail!("[limit-exceeded] book members exceed {MAX_TOTAL_BYTES} bytes combined");
                }
            }
        }
        let bytes = match fs::read(&m.path) {
            Ok(b) => b,
            Err(e) => {
                problems.push(format!("{}: {e}", m.rel));
                continue;
            }
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => out.push(Loaded {
                rel: m.rel.clone(),
                path: m.path.clone(),
                bytes,
                value,
            }),
            Err(e) => problems.push(format!("{}: invalid JSON: {e}", m.rel)),
        }
    }
    ensure!(
        problems.is_empty(),
        "[missing-resource] book {} cannot be processed; nothing was written. Problems: {}. Fix or remove these members (`pentool book remove`)",
        book.path.display(),
        problems.join("; ")
    );
    Ok(out)
}

fn require_layout(members: &[Loaded]) -> Result<()> {
    for m in members {
        ensure!(
            spreads::is_v9(&m.value),
            "[unsupported-version] member {} is not a version 9 document; run `pentool migrate --target 9` on it first",
            m.rel
        );
    }
    Ok(())
}

fn ctx_for(m: &Loaded) -> DocContext {
    DocContext {
        file_name: m.path.file_name().map(|n| n.to_string_lossy().into_owned()),
    }
}

// ------------------------------------------------------------- commit

fn write_manifest(book: &Path, manifest: &Value) -> Result<()> {
    validate_manifest(manifest)?;
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    crate::editing::atomic_write(book, &bytes)
}

/// Stage, verify and commit changed members. All members are written or none.
fn commit_members(
    book: &Book,
    label: &str,
    members: &[Loaded],
    changed: &BTreeMap<usize, Value>,
    dry_run: bool,
    fail_after: Option<usize>,
) -> Result<Vec<Value>> {
    let mut plan: Vec<(usize, Vec<u8>)> = Vec::new();
    for (&i, value) in changed {
        transaction::validate_value(value).with_context(|| {
            format!(
                "member {} failed validation; nothing was written",
                members[i].rel
            )
        })?;
        spreads::validate(value, spreads::VERSION).with_context(|| {
            format!(
                "member {} failed layout validation; nothing was written",
                members[i].rel
            )
        })?;
        let mut bytes = serde_json::to_vec_pretty(value)?;
        if members[i].bytes.last() == Some(&b'\n') {
            bytes.push(b'\n');
        }
        if bytes != members[i].bytes {
            plan.push((i, bytes));
        }
    }
    let report: Vec<Value> = plan
        .iter()
        .map(|(i, after)| {
            json!({
                "member": members[*i].rel,
                "bytes_before": members[*i].bytes.len(),
                "bytes_after": after.len(),
                "revision_after": transaction::revision(after),
            })
        })
        .collect();
    if dry_run || plan.is_empty() {
        return Ok(report);
    }
    // Journal the pre-images before the first write so a crash can roll back.
    let dir = journal_dir(&book.path);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    let mut entries = Vec::new();
    for (n, (i, after)) in plan.iter().enumerate() {
        let file = format!("{n}.before");
        fs::write(dir.join(&file), &members[*i].bytes)?;
        entries.push(json!({
            "path": members[*i].rel,
            "before": file,
            "sha_before": transaction::revision(&members[*i].bytes),
            "sha_after": transaction::revision(after),
        }));
    }
    fs::write(
        dir.join("journal.json"),
        serde_json::to_vec_pretty(&json!({"operation": label, "members": entries}))?,
    )?;
    let mut written = 0usize;
    let mut failure: Option<anyhow::Error> = None;
    for (i, after) in &plan {
        if fail_after == Some(written) {
            failure = Some(anyhow::anyhow!(
                "[injected] simulated failure after {written} members"
            ));
            break;
        }
        let m = &members[*i];
        match transaction::commit_bytes(&m.path, label, false, None, after) {
            Ok(_) => written += 1,
            Err(e) => {
                failure = Some(e.context(format!("member {} could not be committed", m.rel)));
                break;
            }
        }
    }
    if let Some(error) = failure {
        for (i, _) in plan.iter().take(written + 1) {
            let _ = crate::editing::atomic_write(&members[*i].path, &members[*i].bytes);
        }
        let _ = fs::remove_dir_all(&dir);
        return Err(error.context(format!(
            "book operation {label} failed; every member was restored byte for byte"
        )));
    }
    fs::remove_dir_all(&dir).context("could not remove the book journal")?;
    Ok(report)
}

// ------------------------------------------------------------ operations

fn first_page_id(raw: &Value) -> Option<String> {
    raw["pages"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["id"].as_str())
        .map(str::to_string)
}

fn last_number(raw: &Value) -> u64 {
    spreads::page_labels(raw)
        .last()
        .and_then(|l| l["number"].as_u64())
        .unwrap_or(0)
}

fn set_first_page_number(raw: &mut Value, number: u64) -> Result<bool> {
    let Some(first) = first_page_id(raw) else {
        return Ok(false);
    };
    let existing = raw
        .pointer("/layout/sections")
        .and_then(Value::as_array)
        .and_then(|s| s.iter().find(|s| s["start_page"] == first.as_str()))
        .map(|s| s["id"].as_str().unwrap_or_default().to_string());
    let settings = SectionSettings {
        start_number: Some(number),
        continue_numbering: Some(false),
        style: None,
        prefix: None,
    };
    let before = raw.clone();
    match existing {
        Some(id) => {
            spreads::section_set(raw, &id, settings)?;
        }
        None if number == 1 => return Ok(false),
        None => {
            spreads::section_add(raw, &first, None, settings)?;
        }
    }
    Ok(*raw != before)
}

fn set_book_numbering(raw: &mut Value, chapter: usize, footnote: usize) {
    let layout = raw.get_mut("layout").and_then(Value::as_object_mut);
    let Some(layout) = layout else { return };
    let mut map = Map::new();
    if chapter > 0 {
        map.insert("chapter_offset".into(), json!(chapter));
    }
    if footnote > 0 {
        map.insert("footnote_offset".into(), json!(footnote));
    }
    if map.is_empty() {
        layout.remove("book_numbering");
    } else {
        layout.insert("book_numbering".into(), Value::Object(map));
    }
}

fn numbering_choice<'a>(manifest: &'a Value, key: &str, default: &'a str) -> &'a str {
    manifest
        .pointer(&format!("/numbering/{key}"))
        .and_then(Value::as_str)
        .unwrap_or(default)
}

/// Plan renumbering; returns the changed members and a per-member report.
fn plan_renumber(book: &Book, members: &[Loaded]) -> Result<(BTreeMap<usize, Value>, Vec<Value>)> {
    require_layout(members)?;
    let page = numbering_choice(&book.manifest, "page", "continuous");
    let chapter = numbering_choice(&book.manifest, "chapter", "continuous");
    let footnote = numbering_choice(&book.manifest, "footnote", "per-document");
    let mut changed = BTreeMap::new();
    let mut report = Vec::new();
    let mut previous_last = 0u64;
    let mut chapters_before = 0usize;
    let mut footnotes_before = 0usize;
    for (i, m) in members.iter().enumerate() {
        let mut raw = m.value.clone();
        if i > 0 {
            let start = if page == "continuous" {
                previous_last + 1
            } else {
                1
            };
            set_first_page_number(&mut raw, start)?;
        }
        let (chapters, footnotes) = longdoc::book_counts(&raw, &ctx_for(m))?;
        let chapter_offset = if chapter == "continuous" {
            chapters_before
        } else {
            0
        };
        let footnote_offset = if footnote == "continuous" {
            footnotes_before
        } else {
            0
        };
        set_book_numbering(&mut raw, chapter_offset, footnote_offset);
        previous_last = last_number(&raw);
        chapters_before += chapters;
        footnotes_before += footnotes;
        let labels = spreads::page_labels(&raw);
        report.push(json!({
            "member": m.rel,
            "pages": labels.len(),
            "first_label": labels.first().map(|l| l["label"].clone()),
            "last_label": labels.last().map(|l| l["label"].clone()),
            "first_number": labels.first().map(|l| l["number"].clone()),
            "chapter_offset": chapter_offset,
            "footnote_offset": footnote_offset,
            "changed": raw != m.value,
        }));
        if raw != m.value {
            changed.insert(i, raw);
        }
    }
    Ok((changed, report))
}

fn style_maps(raw: &Value) -> BTreeMap<String, BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    if let Some(kinds) = raw.pointer("/layout/styles").and_then(Value::as_object) {
        for (kind, group) in kinds {
            if let Some(group) = group.as_object() {
                out.insert(
                    kind.clone(),
                    group.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                );
            }
        }
    }
    out
}

struct SyncCounts {
    added: usize,
    updated: usize,
    details: Vec<Value>,
}

impl SyncCounts {
    fn new() -> Self {
        SyncCounts {
            added: 0,
            updated: 0,
            details: Vec::new(),
        }
    }
    fn note(&mut self, what: &str, kind: &str, id: &str, added: bool) {
        if added {
            self.added += 1;
        } else {
            self.updated += 1;
        }
        if self.details.len() < DETAIL_CAP {
            self.details.push(json!({
                "what": what, "kind": kind, "id": id,
                "change": if added { "added" } else { "replaced-local" },
            }));
        }
    }
}

fn sync_styles(source: &Value, target: &mut Value, counts: &mut SyncCounts) -> Result<()> {
    for (kind, styles) in style_maps(source) {
        for (id, style) in styles {
            let current = target.pointer(&format!("/layout/styles/{kind}/{id}"));
            if current == Some(&style) {
                continue;
            }
            let added = current.is_none();
            let layout = target
                .get_mut("layout")
                .and_then(Value::as_object_mut)
                .context("[unsupported-version] member has no layout")?;
            let group = layout
                .entry("styles")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("[invalid-input] layout.styles must be an object")?
                .entry(kind.clone())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .with_context(|| {
                    format!("[invalid-input] layout.styles.{kind} must be an object")
                })?;
            group.insert(id.clone(), style);
            counts.note("style", &kind, &id, added);
        }
    }
    Ok(())
}

fn sync_swatches(source: &Value, target: &mut Value, counts: &mut SyncCounts) -> Result<()> {
    if let Some(tokens) = source.get("styles").and_then(Value::as_object) {
        for (id, token) in tokens {
            if token.get("type").and_then(Value::as_str) != Some("color") {
                continue;
            }
            let current = target.pointer(&format!("/styles/{}", pointer_escape(id)));
            if current == Some(token) {
                continue;
            }
            let added = current.is_none();
            let map = target
                .as_object_mut()
                .context("member is not an object")?
                .entry("styles")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("[invalid-input] styles must be an object")?;
            map.insert(id.clone(), token.clone());
            counts.note("swatch", "color", id, added);
        }
    }
    if let Some(inks) = source
        .pointer("/color_management/inks/inks")
        .and_then(Value::as_object)
    {
        let schema = source
            .pointer("/color_management/inks/schema")
            .cloned()
            .unwrap_or(json!(1));
        for (id, ink) in inks {
            let current = target.pointer(&format!(
                "/color_management/inks/inks/{}",
                pointer_escape(id)
            ));
            if current == Some(ink) {
                continue;
            }
            let added = current.is_none();
            let management = target
                .as_object_mut()
                .context("member is not an object")?
                .entry("color_management")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("[invalid-input] color_management must be an object")?;
            let library = management
                .entry("inks")
                .or_insert_with(|| json!({"schema": schema, "inks": {}}))
                .as_object_mut()
                .context("[invalid-input] color_management.inks must be an object")?;
            library
                .entry("inks")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("[invalid-input] color_management.inks.inks must be an object")?
                .insert(id.clone(), ink.clone());
            counts.note("swatch", "ink", id, added);
        }
    }
    Ok(())
}

fn pointer_escape(id: &str) -> String {
    id.replace('~', "~0").replace('/', "~1")
}

fn sync_parents(source: &Value, target: &mut Value, counts: &mut SyncCounts) -> Result<()> {
    let source_parents = source
        .pointer("/layout/parents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for parent in source_parents {
        let id = parent["id"].as_str().unwrap_or_default().to_string();
        let list = target
            .get_mut("layout")
            .and_then(Value::as_object_mut)
            .context("[unsupported-version] member has no layout")?
            .entry("parents")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("[invalid-input] layout.parents must be an array")?;
        match list.iter_mut().find(|p| p["id"] == id.as_str()) {
            Some(existing) if *existing == parent => {}
            Some(existing) => {
                *existing = parent;
                counts.note("parent", "parent", &id, false);
            }
            None => {
                list.push(parent);
                counts.note("parent", "parent", &id, true);
            }
        }
    }
    Ok(())
}

fn source_index(book: &Book) -> Result<usize> {
    let source = book
        .manifest
        .pointer("/synchronize/source")
        .and_then(Value::as_str)
        .context("[invalid-input] the book has no synchronize.source; set it in the manifest")?;
    book.members
        .iter()
        .position(|m| m.rel == source)
        .with_context(|| format!("[invalid-input] synchronize.source {source} is not a member"))
}

fn member_index(book: &Book, path: &str) -> Result<usize> {
    book.members
        .iter()
        .position(|m| m.rel == path)
        .with_context(|| {
            format!(
                "[missing-resource] {path} is not a member of this book; members: {}",
                book.members
                    .iter()
                    .map(|m| m.rel.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn target_member(book: &Book, into: Option<&str>) -> Result<usize> {
    match into {
        Some(p) => member_index(book, p),
        None => Ok(0),
    }
}

fn result(book: &Book, operation: &str, dry_run: bool, mut body: Value) -> Value {
    body["operation"] = json!(operation);
    body["book"] = json!(book.path);
    body["dry_run"] = json!(dry_run);
    body
}

fn manifest_result(
    book_path: &Path,
    manifest: &Value,
    operation: &str,
    dry_run: bool,
) -> Result<Value> {
    validate_manifest(manifest)?;
    if !dry_run {
        write_manifest(book_path, manifest)?;
    }
    Ok(json!({
        "operation": operation,
        "book": book_path,
        "dry_run": dry_run,
        "documents": manifest["documents"].as_array().map_or(0, Vec::len),
        "members": manifest["documents"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| d["path"].clone())
            .collect::<Vec<_>>(),
    }))
}

/// Run one book action against the `.penbook` at `book_path`.
pub fn run(book_path: &Path, action: BookAction) -> Result<Value> {
    run_with(book_path, action, None)
}

fn run_with(book_path: &Path, action: BookAction, fail_after: Option<usize>) -> Result<Value> {
    if let BookAction::Init {
        name,
        members,
        dry_run,
    } = action
    {
        ensure!(
            !book_path.exists(),
            "[output-exists] {} already exists",
            book_path.display()
        );
        ensure!(
            !members.is_empty(),
            "[invalid-input] a book needs at least one --member"
        );
        let manifest = json!({
            "format": "penbook",
            "version": 1,
            "name": name,
            "documents": members.iter().map(|m| json!({"path": m, "role": "chapter"})).collect::<Vec<_>>(),
            "numbering": {"page": "continuous", "chapter": "continuous", "footnote": "per-document"},
            "synchronize": {"source": members[0], "styles": true, "swatches": true, "parents": false},
        });
        return manifest_result(book_path, &manifest, "book-init", dry_run);
    }
    let (book, recovered) = load(book_path)?;
    let mut value = match action {
        BookAction::Init { .. } => unreachable!("handled above"),
        BookAction::Add {
            path,
            role,
            at,
            dry_run,
        } => {
            check_member_path(&path)?;
            let mut manifest = book.manifest.clone();
            let docs = manifest["documents"].as_array_mut().unwrap();
            let position = at.unwrap_or(docs.len());
            ensure!(
                position <= docs.len(),
                "[invalid-input] position {position} is past the end ({} members)",
                docs.len()
            );
            let mut entry = json!({"path": path});
            if let Some(role) = role {
                entry["role"] = json!(role);
            }
            docs.insert(position, entry);
            manifest_result(&book.path, &manifest, "book-add", dry_run)?
        }
        BookAction::Remove { path, dry_run } => {
            let index = member_index(&book, &path)?;
            let mut manifest = book.manifest.clone();
            let docs = manifest["documents"].as_array_mut().unwrap();
            ensure!(
                docs.len() > 1,
                "[invalid-input] a book needs at least one member"
            );
            docs.remove(index);
            if manifest
                .pointer("/synchronize/source")
                .and_then(Value::as_str)
                == Some(&path)
            {
                manifest["synchronize"]
                    .as_object_mut()
                    .map(|s| s.remove("source"));
            }
            manifest_result(&book.path, &manifest, "book-remove", dry_run)?
        }
        BookAction::Move { path, to, dry_run } => {
            let index = member_index(&book, &path)?;
            let mut manifest = book.manifest.clone();
            let docs = manifest["documents"].as_array_mut().unwrap();
            ensure!(
                to < docs.len(),
                "[invalid-input] position {to} is past the last member ({} members)",
                docs.len()
            );
            let entry = docs.remove(index);
            docs.insert(to, entry);
            manifest_result(&book.path, &manifest, "book-move", dry_run)?
        }
        BookAction::Status => status(&book)?,
        BookAction::Renumber { dry_run } => {
            let members = read_members(&book)?;
            let (changed, report) = plan_renumber(&book, &members)?;
            let written = commit_members(
                &book,
                "book-renumber",
                &members,
                &changed,
                dry_run,
                fail_after,
            )?;
            result(
                &book,
                "book-renumber",
                dry_run,
                json!({"members": report, "written": written, "changed": !written.is_empty()}),
            )
        }
        BookAction::Sync {
            styles,
            swatches,
            parents,
            dry_run,
        } => {
            let any = styles || swatches || parents;
            let from_manifest = |key: &str| {
                book.manifest
                    .pointer(&format!("/synchronize/{key}"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            };
            let (do_styles, do_swatches, do_parents) = if any {
                (styles, swatches, parents)
            } else {
                (
                    from_manifest("styles"),
                    from_manifest("swatches"),
                    from_manifest("parents"),
                )
            };
            ensure!(
                do_styles || do_swatches || do_parents,
                "[invalid-input] nothing to synchronize: pass --styles, --swatches or --parents, or enable them in the manifest"
            );
            let source = source_index(&book)?;
            let members = read_members(&book)?;
            require_layout(&members)?;
            let mut changed = BTreeMap::new();
            let mut report = Vec::new();
            for (i, m) in members.iter().enumerate() {
                if i == source {
                    continue;
                }
                let mut raw = m.value.clone();
                let mut counts = SyncCounts::new();
                if do_styles {
                    sync_styles(&members[source].value, &mut raw, &mut counts)?;
                }
                if do_swatches {
                    sync_swatches(&members[source].value, &mut raw, &mut counts)?;
                }
                if do_parents {
                    sync_parents(&members[source].value, &mut raw, &mut counts)?;
                }
                report.push(json!({
                    "member": m.rel,
                    "added": counts.added,
                    "replaced_local": counts.updated,
                    "details": counts.details,
                    "details_truncated": counts.added + counts.updated > DETAIL_CAP,
                }));
                if raw != m.value {
                    changed.insert(i, raw);
                }
            }
            let written =
                commit_members(&book, "book-sync", &members, &changed, dry_run, fail_after)?;
            result(
                &book,
                "book-sync",
                dry_run,
                json!({
                    "source": members[source].rel,
                    "styles": do_styles, "swatches": do_swatches, "parents": do_parents,
                    "members": report,
                    "written": written,
                    "changed": !written.is_empty(),
                }),
            )
        }
        BookAction::Toc {
            styles,
            into,
            id,
            story,
            dry_run,
        } => {
            let target = target_member(&book, into.as_deref())?;
            let members = read_members(&book)?;
            require_layout(&members)?;
            let (changed, renumber_report) = plan_renumber(&book, &members)?;
            let parts = members
                .iter()
                .map(|m| longdoc::book_part(&m.value, &ctx_for(m), &styles))
                .collect::<Result<Vec<BookPart>>>()?;
            let (paragraphs, entries) = longdoc::book_toc_paragraphs(&parts);
            let mut raw = members[target].value.clone();
            let digest = longdoc::store_book_generated(
                &mut raw,
                &id,
                "toc",
                &story,
                json!({"styles": styles}),
                paragraphs,
                entries,
            )?;
            let mut to_write = BTreeMap::new();
            to_write.insert(target, raw);
            let written =
                commit_members(&book, "book-toc", &members, &to_write, dry_run, fail_after)?;
            result(
                &book,
                "book-toc",
                dry_run,
                json!({
                    "into": members[target].rel, "id": id, "story": story,
                    "entries": entries, "digest": digest,
                    "diagnostics": parts.iter().flat_map(|p| p.diagnostics.clone()).collect::<Vec<_>>(),
                    "numbering_continuous": changed.is_empty(),
                    "numbering": if changed.is_empty() { Value::Null } else { json!({
                        "warning": "page labels in the TOC come from each member's current numbering; run `pentool book renumber` first so they are continuous",
                        "members": renumber_report,
                    }) },
                    "written": written,
                    "changed": !written.is_empty(),
                }),
            )
        }
        BookAction::Index {
            rule,
            language,
            into,
            id,
            story,
            dry_run,
        } => {
            let target = target_member(&book, into.as_deref())?;
            let members = read_members(&book)?;
            require_layout(&members)?;
            let (changed, _) = plan_renumber(&book, &members)?;
            let parts = members
                .iter()
                .map(|m| longdoc::book_part(&m.value, &ctx_for(m), &[]))
                .collect::<Result<Vec<BookPart>>>()?;
            let (paragraphs, entries) = longdoc::book_index_paragraphs(&parts, &rule, &language)?;
            let mut raw = members[target].value.clone();
            let digest = longdoc::store_book_generated(
                &mut raw,
                &id,
                "index",
                &story,
                json!({"page_range": rule, "language": language}),
                paragraphs,
                entries,
            )?;
            let mut to_write = BTreeMap::new();
            to_write.insert(target, raw);
            let written = commit_members(
                &book,
                "book-index",
                &members,
                &to_write,
                dry_run,
                fail_after,
            )?;
            result(
                &book,
                "book-index",
                dry_run,
                json!({
                    "into": members[target].rel, "id": id, "story": story,
                    "entries": entries, "digest": digest,
                    "diagnostics": parts.iter().flat_map(|p| p.diagnostics.clone()).collect::<Vec<_>>(),
                    "numbering_continuous": changed.is_empty(),
                    "written": written,
                    "changed": !written.is_empty(),
                }),
            )
        }
        BookAction::Preflight { print } => preflight(&book, print)?,
        BookAction::Package { out } => package(&book, &out)?,
        BookAction::Export {
            out,
            format,
            ppi,
            title,
            created,
            overwrite,
        } => export(&book, &out, &format, ppi, title, created, overwrite)?,
    };
    if !recovered.is_empty() {
        value["recovered_members"] = json!(recovered);
    }
    Ok(value)
}

fn status(book: &Book) -> Result<Value> {
    let source = source_index(book).ok();
    let source_raw = source.and_then(|i| {
        fs::read(&book.members[i].path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    });
    let source_styles = source_raw.as_ref().map(style_maps).unwrap_or_default();
    let mut rows = Vec::new();
    let mut missing = 0usize;
    let mut total_pages = 0usize;
    for (i, m) in book.members.iter().enumerate() {
        let role = book.manifest["documents"][i]["role"].clone();
        let Ok(bytes) = fs::read(&m.path) else {
            missing += 1;
            rows.push(json!({"member": m.rel, "role": role, "exists": false}));
            continue;
        };
        let raw: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                rows.push(json!({
                    "member": m.rel, "role": role, "exists": true,
                    "error": format!("[malformed-resource] {e}"),
                }));
                continue;
            }
        };
        let labels = if spreads::is_v9(&raw) {
            spreads::page_labels(&raw)
        } else {
            Vec::new()
        };
        let pages = raw["pages"].as_array().map_or(0, Vec::len);
        total_pages += pages;
        let drift = if Some(i) == source || source_styles.is_empty() {
            Value::Null
        } else {
            let mine = style_maps(&raw);
            let mut missing_ids = 0usize;
            let mut different = 0usize;
            for (kind, styles) in &source_styles {
                for (id, style) in styles {
                    match mine.get(kind).and_then(|k| k.get(id)) {
                        None => missing_ids += 1,
                        Some(s) if s != style => different += 1,
                        _ => {}
                    }
                }
            }
            json!({"styles_missing": missing_ids, "styles_different": different})
        };
        rows.push(json!({
            "member": m.rel,
            "role": role,
            "exists": true,
            "version": raw["version"],
            "pages": pages,
            "bytes": bytes.len(),
            "first_label": labels.first().map(|l| l["label"].clone()),
            "last_label": labels.last().map(|l| l["label"].clone()),
            "book_numbering": raw.pointer("/layout/book_numbering").cloned(),
            "source": Some(i) == source,
            "drift": drift,
        }));
    }
    Ok(json!({
        "operation": "book-status",
        "book": book.path,
        "name": book.manifest["name"],
        "numbering": book.manifest.get("numbering"),
        "synchronize": book.manifest.get("synchronize"),
        "documents": rows.len(),
        "missing": missing,
        "pages": total_pages,
        "members": rows,
        "ok": missing == 0,
    }))
}

fn preflight(book: &Book, print: bool) -> Result<Value> {
    let mut issues: Vec<Value> = Vec::new();
    let mut members_out = Vec::new();
    let mut loaded: Vec<Loaded> = Vec::new();
    for m in &book.members {
        match fs::read(&m.path) {
            Err(_) => issues.push(json!({
                "severity": "error", "code": "member-missing", "member": m.rel,
                "message": format!("member {} does not exist", m.rel),
                "action": "restore the file or run `pentool book remove`",
            })),
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Err(e) => issues.push(json!({
                    "severity": "error", "code": "member-unreadable", "member": m.rel,
                    "message": format!("member {} is not valid JSON: {e}", m.rel),
                    "action": "repair or restore the file",
                })),
                Ok(value) => loaded.push(Loaded {
                    rel: m.rel.clone(),
                    path: m.path.clone(),
                    bytes,
                    value,
                }),
            },
        }
    }
    let mut print_pages = 0usize;
    let mut print_truncated = false;
    for m in &loaded {
        let raw = &m.value;
        let mut member_issues = 0usize;
        let mut push =
            |issues: &mut Vec<Value>, severity: &str, code: &str, message: String, action: &str| {
                member_issues += 1;
                if issues.len() < crate::color::preflight::MAX_ISSUES {
                    issues.push(json!({
                        "severity": severity, "code": code, "member": m.rel,
                        "message": message, "action": action,
                    }));
                }
            };
        if let Err(e) = transaction::validate_value(raw) {
            push(
                &mut issues,
                "error",
                "member-invalid",
                format!("{}: {e}", m.rel),
                "fix the document before exporting the book",
            );
            members_out.push(json!({"member": m.rel, "issues": member_issues}));
            continue;
        }
        if !spreads::is_v9(raw) {
            push(
                &mut issues,
                "warning",
                "member-not-v9",
                format!(
                    "{} is version {}; layout checks are skipped",
                    m.rel, raw["version"]
                ),
                "run `pentool migrate --target 9`",
            );
        } else {
            let stories = crate::stories::summary(raw);
            let overset = stories["overset_count"].as_u64().unwrap_or(0);
            if overset > 0 {
                push(
                    &mut issues,
                    "error",
                    "text-overset",
                    format!("{} has {overset} story(ies) with overset text", m.rel),
                    "extend the thread, shrink the text or add pages",
                );
            }
            if let Ok(linked) = crate::linked::report(raw, &m.path) {
                if linked["portable"] == false {
                    let bad = linked["dependencies"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|d| d["status"] != "embedded" && d["status"] != "verified")
                        .count();
                    push(
                        &mut issues,
                        "error",
                        "linked-asset-unavailable",
                        format!(
                            "{} has {bad} linked image(s) that are missing, stale or unsafe",
                            m.rel
                        ),
                        "relink or restore the images (`pentool linked`)",
                    );
                }
            }
            if let Ok(generated) = longdoc::run(
                &mut raw.clone(),
                longdoc::DocAction::GeneratedList,
                &ctx_for(m),
            ) {
                let stale = generated.0["generated"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|g| g["stale"] == true)
                    .count();
                if stale > 0 {
                    push(
                        &mut issues,
                        "warning",
                        "generated-stale",
                        format!("{} has {stale} stale generated story(ies)", m.rel),
                        "regenerate TOC, index and captions after renumbering",
                    );
                }
            }
        }
        let mut print_found: Vec<Value> = Vec::new();
        if print && crate::color::has_v8(raw) {
            let pages = raw["pages"].as_array().cloned().unwrap_or_default();
            for page in pages {
                if print_pages >= PRINT_PAGE_CAP {
                    print_truncated = true;
                    break;
                }
                print_pages += 1;
                let id = page["id"].as_str().unwrap_or_default().to_string();
                match crate::color::preflight::run(
                    raw,
                    &m.path,
                    Some(&id),
                    &crate::color::preflight::Options::default(),
                ) {
                    Ok(report) => {
                        for issue in report["issues"].as_array().into_iter().flatten() {
                            let mut issue = issue.clone();
                            issue["member"] = json!(m.rel);
                            issue["page"] = json!(id);
                            print_found.push(issue);
                        }
                    }
                    Err(e) => push(
                        &mut issues,
                        "error",
                        "print-preflight-failed",
                        format!("{} page {id}: {e}", m.rel),
                        "fix the output role and production geometry",
                    ),
                }
            }
        }
        for issue in print_found {
            member_issues += 1;
            if issues.len() < crate::color::preflight::MAX_ISSUES {
                issues.push(issue);
            }
        }
        members_out.push(json!({"member": m.rel, "issues": member_issues}));
    }
    let errors = issues.iter().filter(|i| i["severity"] == "error").count();
    Ok(json!({
        "operation": "book-preflight",
        "book": book.path,
        "ok": errors == 0,
        "errors": errors,
        "warnings": issues.len() - errors,
        "members": members_out,
        "issues": issues,
        "print": print,
        "print_pages_checked": print_pages,
        "print_truncated": print_truncated,
        "mutated": false,
    }))
}

fn package(book: &Book, out: &Path) -> Result<Value> {
    ensure!(
        !out.exists(),
        "[output-exists] packaging requires a new directory: {}",
        out.display()
    );
    let members = read_members(book)?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0usize;
    let mut manifest_bytes = serde_json::to_vec_pretty(&book.manifest)?;
    manifest_bytes.push(b'\n');
    let book_name = book.path.file_name().map_or_else(
        || "book.penbook".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    files.push((book_name.clone(), manifest_bytes));
    let mut asset_count = 0usize;
    for m in &members {
        let mut raw = m.value.clone();
        let dir = Path::new(&m.rel)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| !p.is_empty());
        let prefix = |name: &str| match &dir {
            Some(d) => format!("{d}/{name}"),
            None => name.to_string(),
        };
        let ids: Vec<String> = raw
            .get("image_assets")
            .and_then(Value::as_object)
            .map(|a| a.keys().cloned().collect())
            .unwrap_or_default();
        for id in ids {
            if raw["image_assets"][&id]["storage"]["kind"] == "embedded" {
                continue;
            }
            let bytes =
                crate::image::load_asset_bytes(&raw, crate::resource::document_root(&m.path), &id)
                    .with_context(|| {
                        format!("[missing-resource] linked image {id} of {}", m.rel)
                    })?;
            total = total.saturating_add(bytes.len());
            ensure!(
                total as u64 <= MAX_TOTAL_BYTES,
                "[limit-exceeded] package exceeds {MAX_TOTAL_BYTES} bytes"
            );
            let relative = format!("assets/{}", id.strip_prefix("sha256:").unwrap_or(&id));
            raw["image_assets"][&id]["storage"] =
                crate::image::external_storage(Path::new(&relative))?;
            files.push((prefix(&relative), bytes));
            asset_count += 1;
        }
        transaction::validate_value(&raw)?;
        let mut bytes = serde_json::to_vec_pretty(&raw)?;
        if m.bytes.last() == Some(&b'\n') {
            bytes.push(b'\n');
        }
        total = total.saturating_add(bytes.len());
        files.push((m.rel.clone(), bytes));
    }
    let mut seen = BTreeMap::new();
    for (path, bytes) in &files {
        if let Some(existing) = seen.insert(path.clone(), bytes) {
            ensure!(
                existing == bytes,
                "[invalid-input] packaged path {path} is produced twice with different content"
            );
        }
    }
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let stage = parent.canonicalize()?.join(format!(
        ".pentool-book-package-{}",
        transaction::revision(out.to_string_lossy().as_bytes())
            .chars()
            .take(12)
            .collect::<String>()
    ));
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    let written = (|| -> Result<()> {
        for (path, bytes) in seen.iter() {
            check_package_path(path)?;
            let target = stage.join(path);
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            fs::write(&target, bytes)?;
        }
        fs::rename(&stage, out)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    written?;
    Ok(json!({
        "operation": "book-package",
        "book": book.path,
        "output": out,
        "manifest": book_name,
        "members": members.len(),
        "assets": asset_count,
        "files": seen.len(),
        "bytes": total,
    }))
}

fn check_package_path(path: &str) -> Result<()> {
    ensure!(
        !path.starts_with('/')
            && !path.contains('\\')
            && !path.split('/').any(|s| s == ".." || s.is_empty()),
        "[unsafe-path] refusing to package {path:?}"
    );
    Ok(())
}

fn export(
    book: &Book,
    out: &Path,
    format: &str,
    ppi: Option<f64>,
    title: Option<String>,
    created: Option<String>,
    overwrite: bool,
) -> Result<Value> {
    match format {
        "pdf" => {}
        "epub" => bail!(
            "[unsupported-capability] EPUB book export is part of the publishing exports (v0.14.0 task 15) and is not available yet; use --format pdf"
        ),
        other => bail!("[invalid-input] --format must be pdf or epub, not {other}"),
    }
    ensure!(
        overwrite || !out.exists(),
        "[output-exists] {} exists; pass --overwrite",
        out.display()
    );
    let members = read_members(book)?;
    let mut options = crate::color::pdfx::Options::default();
    if let Some(ppi) = ppi {
        options.ppi = ppi;
    }
    options.title = title.or_else(|| book.manifest["name"].as_str().map(str::to_string));
    options.created = created;
    let refs: Vec<(&Value, &Path)> = members
        .iter()
        .map(|m| (&m.value, m.path.as_path()))
        .collect();
    let exported = crate::color::pdfx::build_many(&refs, &options)?;
    crate::editing::atomic_write(out, &exported.bytes)?;
    let mut report = exported.report;
    report["operation"] = json!("book-export");
    report["book"] = json!(book.path);
    report["output"] = json!(out);
    report["members"] = json!(members.iter().map(|m| m.rel.clone()).collect::<Vec<_>>());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_paths_reject_escapes() {
        for bad in [
            "/abs.pen",
            "C:/x.pen",
            "../x.pen",
            "a/../x.pen",
            "a\\b.pen",
            "x.txt",
            "",
            ".pen",
            "a//b.pen",
        ] {
            assert!(check_member_path(bad).is_err(), "{bad}");
        }
        assert!(check_member_path("chapters/02-body.pen").is_ok());
    }

    #[test]
    fn fixture_manifest_is_valid() {
        let text = include_str!("../docs/fixtures/penbook-valid.penbook");
        let value: Value = serde_json::from_str(text).unwrap();
        validate_manifest(&value).unwrap();
    }

    #[test]
    fn failure_after_a_partial_commit_restores_every_member() {
        let dir = std::env::temp_dir().join(format!("pentool-book-unit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut docs = Vec::new();
        for name in ["a.pen", "b.pen", "c.pen"] {
            let raw = spreads::migrate(crate::scene::new_document(100, 100)).unwrap();
            let bytes = serde_json::to_vec_pretty(&raw).unwrap();
            fs::write(dir.join(name), &bytes).unwrap();
            docs.push((name, bytes));
        }
        let book = dir.join("t.penbook");
        let manifest = json!({"format": "penbook", "version": 1, "name": "T",
            "documents": [{"path": "a.pen"}, {"path": "b.pen"}, {"path": "c.pen"}],
            "numbering": {"page": "continuous"}});
        fs::write(&book, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let error = run_with(&book, BookAction::Renumber { dry_run: false }, Some(1))
            .unwrap_err()
            .to_string();
        assert!(error.contains("restored byte for byte"), "{error}");
        for (name, bytes) in &docs {
            assert_eq!(&fs::read(dir.join(name)).unwrap(), bytes, "{name}");
        }
        assert!(!journal_dir(&book).exists());

        let ok = run_with(&book, BookAction::Renumber { dry_run: false }, None).unwrap();
        assert_eq!(ok["changed"], true);
        assert_ne!(fs::read(dir.join("c.pen")).unwrap(), docs[2].1);
        let _ = fs::remove_dir_all(&dir);
    }
}

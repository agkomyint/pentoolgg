//! Project-local, content-addressed document history.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub operation: String,
    pub before: String,
    pub after: String,
    pub timestamp_ms: u128,
    /// Who made the change; absent in entries written before 0.22.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<EntryActor>,
}

/// The actor behind a recorded change, as Studio's version history shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryActor {
    /// `human` or `agent`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Display name of the human an agent acts for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

fn recorded_actor() -> Option<EntryActor> {
    // Attribution is best effort: an agent without --actor-id still records.
    let (actor, director) = crate::collab::current_actor().ok()?;
    Some(EntryActor {
        kind: actor.kind,
        name: actor.display_name,
        directed_by: director.and_then(|human| human.display_name),
        tool: actor.tool,
    })
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    entries: Vec<Entry>,
    cursor: usize,
}

type Lock = crate::stale::LockFile;

/// Crash points inside [`record`], used by the crash-recovery evidence tests.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashPoint {
    /// Revision blobs are stored; the manifest is not yet written.
    AfterStore,
    /// The manifest temp file is written; it is not yet renamed in.
    AfterManifestTemp,
}

static CRASHES: std::sync::Mutex<Vec<(PathBuf, CrashPoint)>> = std::sync::Mutex::new(Vec::new());

/// Arm a one-shot crash for `document`'s history (test support).
#[doc(hidden)]
pub fn inject_crash(document: &Path, point: CrashPoint) {
    if let (Ok(root), Ok(mut crashes)) = (root(document), CRASHES.lock()) {
        crashes.push((root, point));
    }
}

fn crash(root: &Path, point: CrashPoint) -> Result<()> {
    if let Ok(mut crashes) = CRASHES.lock() {
        if let Some(index) = crashes.iter().position(|(r, p)| r == root && *p == point) {
            crashes.remove(index);
            bail!("[fault-injected] history crashed at {point:?}");
        }
    }
    Ok(())
}

/// Filesystem-safe form of a document's file name, shared by the hidden
/// history and collaboration directories.
pub fn safe_name(document: &Path) -> Result<String> {
    let name = document
        .file_name()
        .context("document path has no filename")?
        .to_string_lossy();
    Ok(name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect())
}

fn root(document: &Path) -> Result<PathBuf> {
    let parent = document.parent().unwrap_or_else(|| Path::new("."));
    Ok(parent
        .join(".pentool")
        .join("history")
        .join(safe_name(document)?))
}

fn lock(root: &Path) -> Result<Lock> {
    fs::create_dir_all(root)?;
    let lock = crate::stale::acquire(&root.join("lock"), crate::stale::STALE_AFTER, "history")?;
    // Every writer holds this lock, so a temp file here is a dead writer's.
    crate::stale::sweep(root, std::time::Duration::ZERO, crate::stale::is_pid_temp);
    Ok(lock)
}

fn load(root: &Path) -> Result<Manifest> {
    let path = root.join("manifest.json");
    if !path.exists() {
        return Ok(Manifest::default());
    }
    serde_json::from_slice(&fs::read(&path)?).context("invalid history manifest")
}

fn save(root: &Path, manifest: &Manifest) -> Result<()> {
    let path = root.join("manifest.json");
    let temp = root.join(format!("manifest.{}.tmp", std::process::id()));
    fs::write(&temp, serde_json::to_vec_pretty(manifest)?)?;
    crash(root, CrashPoint::AfterManifestTemp)?;
    // Rename over the old manifest: removing it first would leave a window in
    // which a crash loses the whole history.
    fs::rename(temp, path)?;
    Ok(())
}

fn store(root: &Path, hash: &str, bytes: &[u8]) -> Result<()> {
    let path = root.join(format!("{hash}.pen"));
    if !path.exists() {
        // Write beside, then rename, so a crash never leaves a truncated blob
        // that a later `store` would trust because the file exists.
        let temp = root.join(format!("{hash}.{}.tmp", std::process::id()));
        fs::write(&temp, bytes)?;
        fs::rename(temp, path)?;
    }
    Ok(())
}

pub fn record(document: &Path, operation: &str, before: &[u8], after: &[u8]) -> Result<PathBuf> {
    let root = root(document)?;
    let _lock = lock(&root)?;
    let before_hash = crate::transaction::revision(before);
    let after_hash = crate::transaction::revision(after);
    store(&root, &before_hash, before)?;
    store(&root, &after_hash, after)?;
    crash(&root, CrashPoint::AfterStore)?;
    let mut manifest = load(&root)?;
    manifest.entries.truncate(manifest.cursor);
    manifest.entries.push(Entry {
        operation: operation.into(),
        before: before_hash,
        after: after_hash,
        timestamp_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        actor: recorded_actor(),
    });
    manifest.cursor = manifest.entries.len();
    save(&root, &manifest)?;
    Ok(root.join("manifest.json"))
}

pub fn list(document: &Path) -> Result<serde_json::Value> {
    let root = root(document)?;
    let manifest = load(&root)?;
    let revision = fs::read(document)
        .ok()
        .map(|bytes| crate::transaction::revision(&bytes));
    Ok(serde_json::json!({"cursor":manifest.cursor,"revision":revision,"entries":manifest.entries}))
}

/// History revisions are SHA-256 digests; anything else could name a path.
fn check_hash(hash: &str) -> Result<()> {
    ensure!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "[invalid-input] history revision must be a 64-digit SHA-256 hex digest, got {hash:?}"
    );
    Ok(())
}

/// The newest applied entry, when it produced exactly `current` (the bytes on
/// disk). `None` after an undo or an edit that bypassed history.
pub fn change_for(document: &Path, current: &[u8]) -> Option<Entry> {
    let manifest = load(&root(document).ok()?).ok()?;
    let entry = manifest.entries.get(manifest.cursor.checked_sub(1)?)?;
    (entry.after == crate::transaction::revision(current)).then(|| entry.clone())
}

/// Bytes of one stored revision, to preview a version without restoring it.
pub fn version(document: &Path, hash: &str) -> Result<Vec<u8>> {
    check_hash(hash)?;
    fs::read(root(document)?.join(format!("{hash}.pen")))
        .with_context(|| format!("[missing-resource] history revision not found: {hash}"))
}

fn restore_hash(document: &Path, root: &Path, hash: &str, cause: &str) -> Result<()> {
    check_hash(hash)?;
    let bytes = fs::read(root.join(format!("{hash}.pen")))
        .with_context(|| format!("history revision not found: {hash}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    crate::transaction::validate_value(&value)?;
    let before = fs::read(document)?;
    let before_value: serde_json::Value = serde_json::from_slice(&before)?;
    let collab = if before == bytes {
        None
    } else {
        crate::collab::begin(document, cause, &before, &bytes)?
    };
    // The re-derived `collab` block rides in the same write as the restore.
    let bytes = match &collab {
        Some(pending) => pending.bytes().to_vec(),
        None => bytes,
    };
    if let Err(error) = crate::editing::atomic_write(document, &bytes) {
        if let Some(pending) = collab {
            pending.abort();
        }
        return Err(error);
    }
    if let Some(pending) = collab {
        pending.commit()?;
    }
    crate::scene3d::node::invalidate_png_cache_for_document_change(&before_value, &value);
    Ok(())
}

/// The document matches a history revision exactly, or differs only in its
/// derived `collab` block (rewritten by the recorded undo/redo itself).
fn same_state(root: &Path, current: &[u8], hash: &str) -> bool {
    if crate::transaction::revision(current) == hash {
        return true;
    }
    fs::read(root.join(format!("{hash}.pen")))
        .map(|stored| {
            crate::collab::content_revision(current) == crate::collab::content_revision(&stored)
        })
        .unwrap_or(false)
}

pub fn undo(document: &Path) -> Result<serde_json::Value> {
    let root = root(document)?;
    let _lock = lock(&root)?;
    let mut manifest = load(&root)?;
    if manifest.cursor == 0 {
        bail!("nothing to undo")
    }
    let index = manifest.cursor - 1;
    if !same_state(&root, &fs::read(document)?, &manifest.entries[index].after) {
        bail!("document changed outside history; restore an explicit revision instead")
    }
    let hash = manifest.entries[index].before.clone();
    restore_hash(document, &root, &hash, "undo")?;
    manifest.cursor = index;
    save(&root, &manifest)?;
    Ok(serde_json::json!({"ok":true,"revision":hash,"cursor":manifest.cursor}))
}

pub fn redo(document: &Path) -> Result<serde_json::Value> {
    let root = root(document)?;
    let _lock = lock(&root)?;
    let mut manifest = load(&root)?;
    let entry = manifest
        .entries
        .get(manifest.cursor)
        .context("nothing to redo")?;
    if !same_state(&root, &fs::read(document)?, &entry.before) {
        bail!("document changed outside history; restore an explicit revision instead")
    }
    let hash = entry.after.clone();
    restore_hash(document, &root, &hash, "redo")?;
    manifest.cursor += 1;
    save(&root, &manifest)?;
    Ok(serde_json::json!({"ok":true,"revision":hash,"cursor":manifest.cursor}))
}

pub fn restore(document: &Path, hash: &str) -> Result<serde_json::Value> {
    let root = root(document)?;
    let _lock = lock(&root)?;
    restore_hash(document, &root, hash, "restore")?;
    Ok(serde_json::json!({"ok":true,"revision":hash}))
}

/// Restore a stored revision as a new history step, so the restore itself
/// shows in the version history and can be undone.
pub fn restore_as_step(document: &Path, hash: &str) -> Result<serde_json::Value> {
    let root = root(document)?;
    let before = fs::read(document)?;
    {
        let _lock = lock(&root)?;
        restore_hash(document, &root, hash, "restore")?;
    }
    let after = fs::read(document)?;
    if before != after {
        record(document, "restore", &before, &after)?;
    }
    Ok(serde_json::json!({"ok":true,"revision":hash,"changed":before != after}))
}

pub fn prune(document: &Path, keep: usize, dry_run: bool) -> Result<serde_json::Value> {
    let root = root(document)?;
    let _lock = lock(&root)?;
    let mut manifest = load(&root)?;
    let remove = manifest.entries.len().saturating_sub(keep);
    if !dry_run && remove > 0 {
        manifest.entries.drain(0..remove);
        manifest.cursor = manifest.cursor.saturating_sub(remove);
        save(&root, &manifest)?;
        let used: std::collections::HashSet<_> = manifest
            .entries
            .iter()
            .flat_map(|entry| [&entry.before, &entry.after])
            .cloned()
            .collect();
        for entry in fs::read_dir(&root)? {
            let path = entry?.path();
            if path.extension().and_then(|v| v.to_str()) == Some("pen")
                && path
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| !used.contains(v))
            {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(
        serde_json::json!({"ok":true,"dry_run":dry_run,"removed":remove,"kept":manifest.entries.len().saturating_sub(if dry_run { remove } else { 0 })}),
    )
}

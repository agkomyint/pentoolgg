//! Project-local, content-addressed document history.
use anyhow::{bail, Context, Result};
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
    });
    manifest.cursor = manifest.entries.len();
    save(&root, &manifest)?;
    Ok(root.join("manifest.json"))
}

pub fn list(document: &Path) -> Result<serde_json::Value> {
    let root = root(document)?;
    let manifest = load(&root)?;
    Ok(serde_json::json!({"cursor":manifest.cursor,"entries":manifest.entries}))
}

fn restore_hash(document: &Path, root: &Path, hash: &str, cause: &str) -> Result<()> {
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

pub fn undo(document: &Path) -> Result<serde_json::Value> {
    let root = root(document)?;
    let _lock = lock(&root)?;
    let mut manifest = load(&root)?;
    if manifest.cursor == 0 {
        bail!("nothing to undo")
    }
    let index = manifest.cursor - 1;
    let current = crate::transaction::revision(&fs::read(document)?);
    if current != manifest.entries[index].after {
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
    let current = crate::transaction::revision(&fs::read(document)?);
    if current != entry.before {
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

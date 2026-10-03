//! Project-local, content-addressed document history.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
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

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn root(document: &Path) -> Result<PathBuf> {
    let parent = document.parent().unwrap_or_else(|| Path::new("."));
    let name = document
        .file_name()
        .context("document path has no filename")?
        .to_string_lossy();
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(parent.join(".pentool").join("history").join(safe))
}

fn lock(root: &Path) -> Result<Lock> {
    fs::create_dir_all(root)?;
    let path = root.join("lock");
    for _ in 0..100 {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(Lock(path)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("history is locked by another pentool process")
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
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(temp, path)?;
    Ok(())
}

fn store(root: &Path, hash: &str, bytes: &[u8]) -> Result<()> {
    let path = root.join(format!("{hash}.pen"));
    if !path.exists() {
        fs::write(path, bytes)?;
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

fn restore_hash(document: &Path, root: &Path, hash: &str) -> Result<()> {
    let bytes = fs::read(root.join(format!("{hash}.pen")))
        .with_context(|| format!("history revision not found: {hash}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    crate::transaction::validate_value(&value)?;
    crate::editing::atomic_write(document, &bytes)
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
    restore_hash(document, &root, &hash)?;
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
    restore_hash(document, &root, &hash)?;
    manifest.cursor += 1;
    save(&root, &manifest)?;
    Ok(serde_json::json!({"ok":true,"revision":hash,"cursor":manifest.cursor}))
}

pub fn restore(document: &Path, hash: &str) -> Result<serde_json::Value> {
    let root = root(document)?;
    let _lock = lock(&root)?;
    restore_hash(document, &root, hash)?;
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

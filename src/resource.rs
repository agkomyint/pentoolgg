//! Offline, content-addressed resource primitives shared by packages and future nodes.
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

pub const MAX_RESOURCE_BYTES: usize = 512 * 1024 * 1024;

pub fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn safe_relative_path(path: &Path) -> Result<PathBuf> {
    let portable = path.to_string_lossy();
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || portable.starts_with('/')
        || portable.starts_with('\\')
        || portable.contains('\\')
        || portable.contains(':')
    {
        bail!("[unsafe-path] resource path must be nonempty and document-relative")
    }
    if path.components().any(|part| {
        matches!(
            part,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        bail!("[unsafe-path] resource path escapes its document or cache root")
    }
    Ok(path.to_path_buf())
}

pub fn verify(bytes: &[u8], expected: &str) -> Result<()> {
    if bytes.len() > MAX_RESOURCE_BYTES {
        bail!("[limit-exceeded] resource exceeds the 512 MiB encoded-byte limit")
    }
    let actual = sha256(bytes);
    if actual != expected {
        bail!("[hash-mismatch] expected {expected}, got {actual}")
    }
    Ok(())
}

/// Directory containing a document; a bare relative file name resolves to ".".
pub fn document_root(document: &Path) -> &Path {
    match document.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Project-local content-addressed cache directory (shared with package installs).
pub fn cache_dir(root: &Path) -> PathBuf {
    root.join(".pentool").join("cache").join("sha256")
}

fn cache_entry(root: &Path, digest: &str) -> Result<PathBuf> {
    let hex = digest
        .strip_prefix("sha256:")
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("[malformed-resource] cache digest must be sha256:<64 hex>")?;
    Ok(cache_dir(root).join(hex.to_ascii_lowercase()))
}

/// Store immutable bytes by digest. Idempotent and safe under concurrent writers:
/// each writer uses a private temporary file and an atomic rename of identical bytes.
pub fn cache_store(root: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let digest = sha256(bytes);
    verify(bytes, &digest)?;
    let entry = cache_entry(root, &digest)?;
    if entry.is_file() && cache_read(root, &digest).is_ok() {
        return Ok(entry);
    }
    std::fs::create_dir_all(cache_dir(root))?;
    let temp = entry.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temp, bytes)?;
    if let Err(error) = std::fs::rename(&temp, &entry) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).context("failed to publish cache entry");
    }
    Ok(entry)
}

/// Read a cache entry. A missing entry is `None`; a corrupt entry is a hard
/// `[hash-mismatch]` failure and is never treated as a miss.
pub fn cache_read(root: &Path, digest: &str) -> Result<Option<Vec<u8>>> {
    let entry = cache_entry(root, digest)?;
    match std::fs::read(&entry) {
        Ok(bytes) => {
            verify(&bytes, digest).with_context(|| {
                format!(
                    "cache entry {} is corrupt; delete it and re-add the source",
                    entry.display()
                )
            })?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("[missing-resource] {}", entry.display())),
    }
}

pub fn read_external_offline(document: &Path, relative: &Path, expected: &str) -> Result<Vec<u8>> {
    let relative = safe_relative_path(relative)?;
    let root = document_root(document);
    if !root.join(&relative).exists() {
        if let Some(bytes) = cache_read(root, expected)? {
            return Ok(bytes);
        }
    }
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("[missing-resource] {}", root.display()))?;
    let path = root.join(relative);
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("[missing-resource] {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        bail!("[unsafe-path] external resource resolves outside its document root")
    }
    let metadata = std::fs::metadata(&canonical_path)
        .with_context(|| format!("[missing-resource] {}", canonical_path.display()))?;
    if metadata.len() > MAX_RESOURCE_BYTES as u64 {
        bail!("[limit-exceeded] resource exceeds the 512 MiB encoded-byte limit")
    }
    let bytes = std::fs::read(&canonical_path)
        .with_context(|| format!("[missing-resource] {}", canonical_path.display()))?;
    verify(&bytes, expected)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_identity_is_deterministic_and_paths_are_offline_safe() {
        assert_eq!(sha256(b"same"), sha256(b"same"));
        assert_ne!(sha256(b"same"), sha256(b"different"));
        assert!(safe_relative_path(Path::new("assets/object.bin")).is_ok());
        assert!(safe_relative_path(Path::new("../secret")).is_err());
        assert!(safe_relative_path(Path::new("..\\secret")).is_err());
        assert!(safe_relative_path(Path::new("C:\\secret")).is_err());
        assert!(safe_relative_path(Path::new("\\\\server\\share")).is_err());
        assert!(safe_relative_path(Path::new("assets/object.bin:stream")).is_err());
        assert!(verify(b"changed", &sha256(b"expected"))
            .unwrap_err()
            .to_string()
            .contains("[hash-mismatch]"));
    }
}

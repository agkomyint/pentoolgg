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

pub fn read_external_offline(document: &Path, relative: &Path, expected: &str) -> Result<Vec<u8>> {
    let relative = safe_relative_path(relative)?;
    let root = document.parent().unwrap_or_else(|| Path::new("."));
    let path = root.join(relative);
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("[missing-resource] {}", path.display()))?;
    if metadata.len() > MAX_RESOURCE_BYTES as u64 {
        bail!("[limit-exceeded] resource exceeds the 512 MiB encoded-byte limit")
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("[missing-resource] {}", path.display()))?;
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

//! Shared validated, revision-guarded document transaction boundary.
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
pub struct ChangeSummary {
    pub ok: bool,
    pub operation: String,
    pub file: PathBuf,
    pub dry_run: bool,
    pub revision_before: String,
    pub revision_after: String,
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub backup: Option<PathBuf>,
    pub history: Option<PathBuf>,
}

pub fn revision(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn commit_value(
    path: &Path,
    operation: &str,
    dry_run: bool,
    expected: Option<&str>,
    value: &Value,
) -> Result<ChangeSummary> {
    validate_value(value)?;
    commit_bytes(
        path,
        operation,
        dry_run,
        expected,
        &serde_json::to_vec_pretty(value)?,
    )
}

pub fn commit_bytes(
    path: &Path,
    operation: &str,
    dry_run: bool,
    expected: Option<&str>,
    after: &[u8],
) -> Result<ChangeSummary> {
    let value: Value = serde_json::from_slice(after).context("invalid serialized document")?;
    validate_value(&value)?;
    let before = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let before_revision = revision(&before);
    if expected.is_some_and(|wanted| wanted != before_revision) {
        bail!("revision mismatch: expected {expected:?}, current {before_revision}")
    }
    let after_revision = revision(after);
    let history = if dry_run || before == after {
        None
    } else {
        let history = crate::history::record(path, operation, &before, after)?;
        crate::editing::atomic_write(path, after)?;
        Some(history)
    };
    Ok(ChangeSummary {
        ok: true,
        operation: operation.into(),
        file: path.into(),
        dry_run,
        revision_before: before_revision,
        revision_after: after_revision,
        bytes_before: before.len(),
        bytes_after: after.len(),
        backup: None,
        history,
    })
}

pub fn validate_value(value: &Value) -> Result<()> {
    if value.get("version").and_then(Value::as_u64) == Some(crate::scene::VERSION) {
        crate::scene::validate(value)
    } else {
        let doc: crate::document::Document =
            serde_json::from_value(value.clone()).context("invalid legacy document")?;
        doc.validate().map_err(anyhow::Error::msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_validation_and_revision_leave_bytes_unchanged() {
        let path =
            std::env::temp_dir().join(format!("pentool-transaction-{}.pen", std::process::id()));
        let doc = serde_json::to_value(crate::document::Document::new(10, 10)).unwrap();
        let bytes = serde_json::to_vec_pretty(&doc).unwrap();
        fs::write(&path, &bytes).unwrap();
        let mut invalid = doc.clone();
        invalid["format"] = Value::String("bad".into());
        assert!(commit_value(&path, "test", false, None, &invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(commit_value(&path, "test", false, Some("wrong"), &doc).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(path).unwrap();
    }
}

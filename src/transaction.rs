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

/// Commit related offline resources before the document and restore them if the
/// guarded document commit fails. Validation and revision checks happen before
/// any write, and the document commit remains the single visible history entry.
pub fn commit_bundle(
    path: &Path,
    operation: &str,
    dry_run: bool,
    expected: Option<&str>,
    value: &Value,
    resources: &[(PathBuf, Vec<u8>)],
) -> Result<ChangeSummary> {
    validate_value(value)?;
    let before = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let current = revision(&before);
    if expected.is_some_and(|wanted| wanted != current) {
        bail!("revision mismatch: expected {expected:?}, current {current}")
    }
    for (resource_path, bytes) in resources {
        crate::resource::safe_relative_path(resource_path)?;
        if bytes.len() > crate::resource::MAX_RESOURCE_BYTES {
            bail!("[limit-exceeded] staged resource exceeds the encoded-byte limit")
        }
    }
    let after = serde_json::to_vec_pretty(value)?;
    if dry_run {
        return commit_bytes(path, operation, true, expected, &after);
    }
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let snapshots = resources
        .iter()
        .map(|(relative, _)| {
            let target = root.join(relative);
            let old = fs::read(&target).ok();
            (target, old)
        })
        .collect::<Vec<_>>();
    let write_result = (|| -> Result<ChangeSummary> {
        for ((_, bytes), (target, _)) in resources.iter().zip(&snapshots) {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            crate::editing::atomic_write(target, bytes)?;
        }
        commit_bytes(path, operation, false, expected, &after)
    })();
    if write_result.is_err() {
        for (target, old) in snapshots.iter().rev() {
            if let Some(bytes) = old {
                let _ = crate::editing::atomic_write(target, bytes);
            } else if target.exists() {
                let _ = fs::remove_file(target);
            }
        }
    }
    write_result
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
    if !operation.starts_with("instance-") {
        let before_value: Value =
            serde_json::from_slice(&before).context("invalid existing document")?;
        protect_attached_instances(&before_value, &value)?;
    }
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

fn protect_attached_instances(before: &Value, after: &Value) -> Result<()> {
    for instance in before
        .get("instances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = instance
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let page = instance
            .get("page_id")
            .and_then(Value::as_str)
            .unwrap_or("page-1");
        let layer_ids = instance
            .get("layer_ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        for layer_id in layer_ids {
            let old = find_layer(before, page, layer_id);
            let new = find_layer(after, page, layer_id);
            if old != new {
                bail!(
                    "layer {layer_id} belongs to attached instance {id}; use `instance set` for an exposed property or detach the instance before editing"
                )
            }
        }
    }
    Ok(())
}

fn find_layer<'a>(document: &'a Value, page_id: &str, layer_id: &str) -> Option<&'a Value> {
    document
        .get("pages")?
        .as_array()?
        .iter()
        .find(|page| page.get("id").and_then(Value::as_str) == Some(page_id))?
        .get("layers")?
        .as_array()?
        .iter()
        .find(|layer| layer.get("id").and_then(Value::as_str) == Some(layer_id))
}

pub fn validate_value(value: &Value) -> Result<()> {
    match value.get("version").and_then(Value::as_u64) {
        Some(crate::scene::VERSION) => crate::scene::validate(value),
        Some(1..=3) => {
            let doc: crate::document::Document =
                serde_json::from_value(value.clone()).context("invalid legacy document")?;
            doc.validate().map_err(anyhow::Error::msg)
        }
        Some(version) => bail!(
            "[unsupported-capability] document format version {version} is newer than supported version {}",
            crate::scene::VERSION
        ),
        None => bail!("[malformed-resource] document version is missing or invalid"),
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

    #[test]
    fn generic_transactions_cannot_edit_attached_instance_layers() {
        let path = std::env::temp_dir().join(format!(
            "pentool-instance-protection-{}.pen",
            std::process::id()
        ));
        let mut doc = serde_json::to_value(crate::document::Document::new(100, 100)).unwrap();
        doc["pages"][0]["layers"][0]["texts"] = serde_json::json!([{
            "id":"label","content":"Before","x":0,"y":20,
            "font_family":"Atkinson Hyperlegible","font_size":16,"font_weight":400,
            "italic":false,"fill":"#000000","align":"left","letter_spacing":0,
            "line_height":1.2,"transform":[1,0,0,1,0,0]
        }]);
        let layer_id = doc["pages"][0]["layers"][0]["id"].clone();
        doc["instances"] = serde_json::json!([{
            "id":"instance-1","layer_ids":[layer_id],"page_id":"page-1"
        }]);
        let bytes = serde_json::to_vec_pretty(&doc).unwrap();
        fs::write(&path, &bytes).unwrap();
        let mut edited = doc.clone();
        edited["pages"][0]["layers"][0]["texts"][0]["content"] = Value::String("After".into());
        let error = commit_value(&path, "text", false, None, &edited)
            .unwrap_err()
            .to_string();
        assert!(error.contains("instance set"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        commit_value(&path, "instance-set", true, None, &edited).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn future_versions_and_unknown_required_nodes_fail_explicitly() {
        let mut future = crate::scene::new_document(10, 10);
        future["version"] = serde_json::json!(99);
        assert!(validate_value(&future)
            .unwrap_err()
            .to_string()
            .contains("[unsupported-capability]"));
        let mut unknown = crate::scene::new_document(10, 10);
        unknown["pages"][0]["layers"][0]["nodes"] =
            serde_json::json!([{"id":"future","kind":"future-node","transform":[1,0,0,1,0,0]}]);
        assert!(validate_value(&unknown)
            .unwrap_err()
            .to_string()
            .contains("unsupported node kind"));
    }

    #[test]
    fn staged_bundle_failure_leaves_document_resource_and_history_unchanged() {
        let root = std::env::temp_dir().join(format!(
            "pentool-bundle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let document = root.join("design.pen");
        let raw = crate::scene::new_document(10, 10);
        let bytes = serde_json::to_vec_pretty(&raw).unwrap();
        fs::write(&document, &bytes).unwrap();
        let resource = root.join("assets/data.bin");
        fs::create_dir_all(resource.parent().unwrap()).unwrap();
        fs::write(&resource, b"before").unwrap();
        let result = commit_bundle(
            &document,
            "bundle-test",
            false,
            Some("wrong-revision"),
            &raw,
            &[(PathBuf::from("assets/data.bin"), b"after".to_vec())],
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&document).unwrap(), bytes);
        assert_eq!(fs::read(&resource).unwrap(), b"before");
        assert!(!root.join(".pentool/history").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

//! v0.13.0 task 14: crash and failure recovery for plan commits. A plan commit
//! rides the shared transaction path, so these tests pin that a failed or
//! interrupted commit leaves the document intact and history can restore it.
use pentool::{paint_plan, raster, transaction};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-v0130-recovery-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn file(&self) -> PathBuf {
        self.0.join("doc.pen")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_document(path: &Path) -> Vec<u8> {
    let mut raw = pentool::scene::new_document(48, 48);
    raster::add(&mut raw, None, None, "p", 0.0, 0.0, 48, 48).unwrap();
    let bytes = serde_json::to_vec_pretty(&raw).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

fn plan(id: &str) -> Value {
    json!({"schema": 1, "engine": 1, "id": id, "passes": [
        {"id": "a", "target": "p", "action": "stroke-path",
         "args": {"path": "M4 10 L44 10", "color": "#336699",
                  "brush": {"size": 6, "hardness": 1}}}]})
}

fn load(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn commit_plan(path: &Path, id: &str) -> anyhow::Result<transaction::ChangeSummary> {
    let bytes = std::fs::read(path)?;
    let mut raw: Value = serde_json::from_slice(&bytes)?;
    paint_plan::execute(&mut raw, None, &plan(id))?;
    transaction::commit_value(
        path,
        "paint-plan",
        false,
        Some(&transaction::revision(&bytes)),
        &raw,
    )
}

#[test]
fn leftovers_from_a_killed_writer_neither_block_nor_corrupt_a_commit() {
    let dir = Scratch::new("leftovers");
    let file = dir.file();
    let original = write_document(&file);
    // What a process killed mid-write leaves behind: a truncated temp file and a
    // moved-aside previous copy, named for a pid that is not ours.
    let temp = dir.0.join(".doc.pen.pentool-tmp-999999");
    let old = dir.0.join(".doc.pen.pentool-old-999999");
    std::fs::write(&temp, &original[..original.len() / 2]).unwrap();
    std::fs::write(&old, b"stale").unwrap();
    // The document itself is untouched and still opens and validates.
    assert_eq!(std::fs::read(&file).unwrap(), original);
    transaction::validate_value(&load(&file)).unwrap();
    // A later commit succeeds and the stale files are left alone, not adopted.
    commit_plan(&file, "after-crash").unwrap();
    assert_ne!(std::fs::read(&file).unwrap(), original);
    transaction::validate_value(&load(&file)).unwrap();
    assert_eq!(std::fs::read(&old).unwrap(), b"stale");
    assert!(temp.exists());
}

#[test]
fn failed_commits_leave_the_document_byte_identical_and_unrecorded() {
    let dir = Scratch::new("failed");
    let file = dir.file();
    let original = write_document(&file);
    // Stale revision guard.
    let mut raw = load(&file);
    paint_plan::execute(&mut raw, None, &plan("stale")).unwrap();
    let stale = transaction::commit_value(&file, "paint-plan", false, Some("0000"), &raw);
    assert!(stale.is_err());
    assert_eq!(std::fs::read(&file).unwrap(), original);
    // A plan that fails mid-way (second pass has no target) never reaches commit.
    let mut broken = plan("broken");
    broken["passes"].as_array_mut().unwrap().push(
        json!({"id": "b", "target": "ghost", "action": "fill", "args": {"color": "#000000"}}),
    );
    let mut raw = load(&file);
    assert!(paint_plan::execute(&mut raw, None, &broken).is_err());
    assert_eq!(raw, load(&file), "failed plan mutated its working copy");
    assert_eq!(std::fs::read(&file).unwrap(), original);
    // An invalid document value is refused before any write or history entry.
    let mut invalid = load(&file);
    invalid["pages"] = json!("not pages");
    assert!(transaction::commit_value(&file, "paint-plan", false, None, &invalid).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), original);
    let entries = pentool::history::list(&file)
        .map(|v| v["entries"].as_array().map_or(0, Vec::len))
        .unwrap_or(0);
    assert_eq!(entries, 0, "a failed commit recorded history");
}

#[test]
fn history_restores_the_pre_plan_document_exactly() {
    let dir = Scratch::new("history");
    let file = dir.file();
    let original = write_document(&file);
    let summary = commit_plan(&file, "undoable").unwrap();
    let committed = std::fs::read(&file).unwrap();
    assert_ne!(committed, original);
    assert!(summary.history.is_some());
    // One undo reverts every pass at once, byte for byte.
    pentool::history::undo(&file).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), original);
    pentool::history::redo(&file).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), committed);
    // Restoring an explicit pre-commit revision also works after a "crash"
    // that lost the manifest cursor.
    pentool::history::restore(&file, &transaction::revision(&original)).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), original);
}

#[test]
fn a_resource_failure_rolls_the_whole_bundle_back() {
    let dir = Scratch::new("bundle");
    let file = dir.file();
    let original = write_document(&file);
    let mut raw = load(&file);
    paint_plan::execute(&mut raw, None, &plan("bundle")).unwrap();
    // The second resource's parent is the first resource's file, so its
    // directory cannot be created and the commit must unwind.
    let resources = vec![
        (PathBuf::from("assets/a.bin"), b"one".to_vec()),
        (PathBuf::from("assets/a.bin/b.bin"), b"two".to_vec()),
    ];
    let result = transaction::commit_bundle(&file, "paint-plan", false, None, &raw, &resources);
    assert!(result.is_err());
    assert_eq!(std::fs::read(&file).unwrap(), original);
    assert!(
        !dir.0.join("assets").join("a.bin").exists(),
        "a staged resource survived a failed bundle"
    );
}

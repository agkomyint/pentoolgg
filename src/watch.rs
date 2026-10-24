//! `pentool watch`: tell an agent the moment a `.pen` document changes, who
//! changed it, and which objects were touched. One JSON line per event.
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

/// Longest id list per category in one event; `truncated` says when it was cut.
pub const MAX_IDS: usize = 200;
/// Re-read the file at least this often even when size and time look unchanged.
const RECHECK: Duration = Duration::from_secs(2);

pub struct Options {
    pub poll: Duration,
    pub once: bool,
    pub since: Option<String>,
    pub timeout: Option<Duration>,
}

/// One readable state of the document.
pub struct Snapshot {
    pub revision: String,
    pub objects: BTreeMap<String, String>,
}

/// Parse the bytes on disk. The embedded `collab` block is derived data, so it
/// is ignored for both the revision and the object fingerprints.
pub fn snapshot(bytes: &[u8]) -> Option<Snapshot> {
    let mut value: Value = serde_json::from_slice(bytes).ok()?;
    let revision = crate::collab::content_revision(bytes);
    crate::collab::stats::strip(&mut value);
    let mut objects = BTreeMap::new();
    fingerprint(&value, &mut objects);
    Some(Snapshot { revision, objects })
}

/// Record a hash for every object that carries a string `id`, covering its own
/// fields but not the objects nested inside it (those get their own entry), so
/// editing a child does not mark every ancestor as changed.
fn fingerprint(value: &Value, out: &mut BTreeMap<String, String>) -> Value {
    match value {
        Value::Object(map) => {
            let mut shallow = Map::new();
            for (key, child) in map {
                shallow.insert(key.clone(), fingerprint(child, out));
            }
            let node = Value::Object(shallow);
            match map.get("id").and_then(Value::as_str) {
                Some(id) => {
                    let digest = Sha256::digest(node.to_string().as_bytes());
                    out.insert(id.to_owned(), hex::encode(&digest[..12]));
                    Value::String(format!("\u{0}ref:{id}"))
                }
                None => node,
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| fingerprint(v, out)).collect()),
        other => other.clone(),
    }
}

/// Ids added, removed and changed between two snapshots, each list sorted and
/// cut at [`MAX_IDS`].
pub fn objects_changed(before: &Snapshot, after: &Snapshot) -> Value {
    let mut added = Vec::new();
    let mut changed = Vec::new();
    let mut removed = Vec::new();
    for (id, hash) in &after.objects {
        match before.objects.get(id) {
            None => added.push(id.clone()),
            Some(previous) if previous != hash => changed.push(id.clone()),
            Some(_) => {}
        }
    }
    for id in before.objects.keys() {
        if !after.objects.contains_key(id) {
            removed.push(id.clone());
        }
    }
    let truncated = [&added, &changed, &removed]
        .iter()
        .any(|ids| ids.len() > MAX_IDS);
    for ids in [&mut added, &mut changed, &mut removed] {
        ids.truncate(MAX_IDS);
    }
    json!({"added": added, "changed": changed, "removed": removed, "truncated": truncated})
}

/// The history entry (operation, actor, time) that produced these bytes, if any.
fn who(path: &Path, bytes: &[u8]) -> Value {
    match crate::history::change_for(path, bytes) {
        Some(entry) => json!({
            "operation": entry.operation,
            "at_ms": entry.timestamp_ms,
            "actor": entry.actor,
        }),
        None => Value::Null,
    }
}

fn stamp(path: &Path) -> Option<(Option<SystemTime>, u64)> {
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.modified().ok(), meta.len()))
}

/// The change event for `current` replacing `previous`.
pub fn change_event(
    path: &Path,
    bytes: &[u8],
    previous: Option<&Snapshot>,
    current: &Snapshot,
) -> Value {
    let mut event = json!({
        "event": "change",
        "revision": current.revision,
        "previous": previous.map(|p| p.revision.clone()),
        "change": who(path, bytes),
    });
    if let Some(previous) = previous {
        event["objects"] = objects_changed(previous, current);
    }
    event
}

/// Watch `path` and hand each event to `emit`; `emit` returns false to stop.
/// The first event is `ready` (the starting revision). With `since`, a document
/// that already differs from that revision is reported as a change at once.
pub fn run(path: &Path, options: &Options, mut emit: impl FnMut(Value) -> bool) -> Result<()> {
    if !path.is_file() {
        bail!(
            "[missing-resource] {} is not a file; give the path of a .pen document",
            path.display()
        )
    }
    let started = Instant::now();
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut current = snapshot(&bytes).with_context(|| {
        format!(
            "[invalid-input] {} is not valid .pen JSON right now",
            path.display()
        )
    })?;
    if !emit(json!({"event": "ready", "revision": current.revision})) {
        return Ok(());
    }
    if options
        .since
        .as_deref()
        .is_some_and(|since| since != current.revision)
    {
        let event = change_event(path, &bytes, None, &current);
        if !emit(event) || options.once {
            return Ok(());
        }
    }
    let mut seen = stamp(path);
    let mut checked = Instant::now();
    loop {
        if let Some(limit) = options.timeout {
            if started.elapsed() >= limit {
                emit(json!({"event": "timeout", "revision": current.revision}));
                return Ok(());
            }
        }
        std::thread::sleep(options.poll);
        let now = stamp(path);
        if now == seen && checked.elapsed() < RECHECK {
            continue;
        }
        checked = Instant::now();
        // A file that is missing or half written is retried on the next tick.
        let Ok(fresh) = std::fs::read(path) else {
            continue;
        };
        let Some(next) = snapshot(&fresh) else {
            continue;
        };
        seen = now;
        if next.revision == current.revision {
            continue;
        }
        let event = change_event(path, &fresh, Some(&current), &next);
        current = next;
        if !emit(event) || options.once {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(color: &str, extra: bool) -> Vec<u8> {
        let mut layer = json!({"id": "l1", "objects": [
            {"id": "car", "kind": "rect", "fill": color},
            {"id": "wheel", "kind": "ellipse"}
        ]});
        if extra {
            layer["objects"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id": "data", "kind": "text"}));
        }
        serde_json::to_vec(
            &json!({"format": "pentool", "pages": [{"id": "p1", "layers": [layer]}]}),
        )
        .unwrap()
    }

    #[test]
    fn reports_exactly_the_objects_that_changed() {
        let before = snapshot(&doc("blue", false)).unwrap();
        let after = snapshot(&doc("red", true)).unwrap();
        assert_ne!(before.revision, after.revision);
        let changed = objects_changed(&before, &after);
        // The layer gained a child, so it changed too.
        assert_eq!(changed["changed"], json!(["car", "l1"]));
        assert_eq!(changed["added"], json!(["data"]));
        let recolor = objects_changed(&before, &snapshot(&doc("red", false)).unwrap());
        assert_eq!(recolor["changed"], json!(["car"]));
        assert_eq!(changed["removed"], json!([]));
        let same = objects_changed(&after, &snapshot(&doc("red", true)).unwrap());
        assert_eq!(same["changed"], json!([]));
    }

    #[test]
    fn watch_sees_a_write_and_stops_after_one_change() {
        let dir = std::env::temp_dir().join(format!("pentool-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("car.pen");
        std::fs::write(&path, doc("blue", false)).unwrap();
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                std::fs::write(&path, doc("red", false)).unwrap();
            })
        };
        let mut events = Vec::new();
        run(
            &path,
            &Options {
                poll: Duration::from_millis(10),
                once: true,
                since: None,
                timeout: Some(Duration::from_secs(10)),
            },
            |event| {
                events.push(event);
                true
            },
        )
        .unwrap();
        writer.join().unwrap();
        assert_eq!(events[0]["event"], "ready");
        assert_eq!(events[1]["event"], "change");
        assert_eq!(events[1]["objects"]["changed"], json!(["car"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn since_an_older_revision_reports_a_change_immediately() {
        let dir = std::env::temp_dir().join(format!("pentool-watch-since-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("car.pen");
        std::fs::write(&path, doc("blue", false)).unwrap();
        let mut events = Vec::new();
        run(
            &path,
            &Options {
                poll: Duration::from_millis(10),
                once: true,
                since: Some("stale".into()),
                timeout: None,
            },
            |event| {
                events.push(event);
                true
            },
        )
        .unwrap();
        assert_eq!(events[1]["event"], "change");
        assert!(events[1]["previous"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_a_clear_error() {
        let error = run(
            Path::new("definitely-missing.pen"),
            &Options {
                poll: Duration::from_millis(10),
                once: true,
                since: None,
                timeout: None,
            },
            |_| true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing-resource"));
    }
}

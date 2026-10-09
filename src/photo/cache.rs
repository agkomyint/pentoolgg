//! The disposable preview cache in `.pentool/cache/photo/` (photography
//! specification, "Caches"). An entry is a preview PNG with one private
//! `pnCk` chunk that holds the entry's key and report. The key is the SHA-256
//! of canonical JSON over everything a preview depends on, and the chunk is
//! stripped before the PNG is returned, so a hit is byte-identical to a fresh
//! render. The cache is an accelerator only: a miss, a corrupt or mismatched
//! entry, or any I/O failure falls back to rendering, and a write failure
//! never fails the command.

use super::catalog;
use super::png;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Default size bound of one document's cache: 2 GiB.
pub const DEFAULT_LIMIT: u64 = 2 << 30;
/// The environment variable that overrides [`DEFAULT_LIMIT`], in bytes.
pub const LIMIT_VARIABLE: &str = "PENTOOL_PHOTO_CACHE_BYTES";
/// Bumped whenever preview rendering changes its output for the same inputs.
const PREVIEW_VERSION: u64 = 1;
/// The private ancillary chunk holding `{key, report}`.
const CHUNK: [u8; 4] = *b"pnCk";

/// One document's preview cache.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    limit: u64,
}

/// What a lookup did, as reported in a preview's `cache` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Hit,
    Miss,
    Off,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Off => "off",
        }
    }
}

/// The cache directory of `document`.
pub fn directory(document: &Path) -> PathBuf {
    let parent = document
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    parent.join(".pentool").join("cache").join("photo")
}

/// The bound from [`LIMIT_VARIABLE`]: a decimal byte count. A missing or
/// unparsable value is [`DEFAULT_LIMIT`]; 0 keeps nothing.
pub fn configured_limit() -> u64 {
    std::env::var(LIMIT_VARIABLE)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_LIMIT)
}

impl Cache {
    /// The cache of `document`, bounded by [`configured_limit`].
    pub fn for_document(document: &Path) -> Self {
        Self::with_limit(document, configured_limit())
    }

    pub fn with_limit(document: &Path, limit: u64) -> Self {
        Self {
            dir: directory(document),
            limit,
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.png"))
    }

    /// The PNG and report stored under `key`, if the entry is intact and its
    /// embedded key matches. A hit refreshes the entry's recency.
    pub fn get(&self, key: &str) -> Option<(Vec<u8>, Value)> {
        let path = self.path(key);
        let bytes = fs::read(&path).ok()?;
        let (png, stored) = split(&bytes)?;
        let stored: Value = serde_json::from_slice(&stored).ok()?;
        if stored["key"] != key || !stored["report"].is_object() {
            return None;
        }
        if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some((png, stored["report"].clone()))
    }

    /// Store `png` and `report` under `key`, then evict least-recently-used
    /// entries beyond the bound. Returns whether the entry was written; any
    /// failure is swallowed.
    pub fn put(&self, key: &str, png: &[u8], report: &Value) -> bool {
        if self.limit == 0 {
            return false;
        }
        let Ok(payload) = serde_json::to_vec(&json!({"key": key, "report": report})) else {
            return false;
        };
        let Some(entry) = join(png, &payload) else {
            return false;
        };
        if entry.len() as u64 > self.limit || fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        let path = self.path(key);
        let temp = self.dir.join(format!("{key}.tmp-{}", std::process::id()));
        if fs::write(&temp, &entry).is_err() || fs::rename(&temp, &path).is_err() {
            let _ = fs::remove_file(&temp);
            return false;
        }
        self.evict(&path);
        true
    }

    /// Delete the oldest entries until the total is within the bound, never
    /// the entry just written.
    fn evict(&self, keep: &Path) {
        let mut entries: Vec<(SystemTime, PathBuf, u64)> = list(&self.dir)
            .into_iter()
            .filter(|(path, _)| path.extension().is_some_and(|e| e == "png"))
            .filter_map(|(path, meta)| Some((meta.modified().ok()?, path, meta.len())))
            .collect();
        let mut total: u64 = entries.iter().map(|e| e.2).sum();
        if total <= self.limit {
            return;
        }
        entries.sort();
        for (_, path, size) in entries {
            if total <= self.limit {
                break;
            }
            if path != keep && fs::remove_file(&path).is_ok() {
                total -= size;
            }
        }
    }
}

/// Regular files directly in `dir`, without following links.
fn list(dir: &Path) -> Vec<(PathBuf, fs::Metadata)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    read.filter_map(|entry| {
        let entry = entry.ok()?;
        let meta = fs::symlink_metadata(entry.path()).ok()?;
        meta.is_file().then(|| (entry.path(), meta))
    })
    .collect()
}

/// `photo cache clear`: remove every entry and leftover temporary file of
/// `document`'s cache. Only regular files named like entries are removed.
pub fn clear(document: &Path) -> Result<Value> {
    if !document.is_file() {
        anyhow::bail!(
            "[missing-resource] document {} does not exist",
            document.display()
        )
    }
    let dir = directory(document);
    let (mut removed, mut bytes) = (0u64, 0u64);
    for (path, meta) in list(&dir) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let stem = name.split('.').next().unwrap_or("");
        let ours = stem.len() == 64
            && stem.bytes().all(|b| b.is_ascii_hexdigit())
            && (name.ends_with(".png") || name.contains(".tmp-"));
        if ours {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            removed += 1;
            bytes += meta.len();
        }
    }
    Ok(json!({
        "directory": dir.display().to_string(),
        "removed_entries": removed,
        "removed_bytes": bytes,
    }))
}

/// The key of a studio preview: SHA-256 of canonical JSON over the engine,
/// process, source digest, referenced profile and mask digests, the develop
/// settings and the request. Fails when the photo or variant does not exist,
/// so the caller renders and reports the real error.
pub fn preview_key(raw: &Value, request: &super::studio::PreviewRequest) -> Result<String> {
    let catalog = &raw["photography"];
    let photo = catalog["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["id"] == request.photo)
        .context("photo")?;
    let develop = &photo["variants"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["id"] == request.variant)
        .context("variant")?["develop"];
    let mut digests = Vec::new();
    collect_digests(develop, &mut digests);
    digests.sort();
    digests.dedup();
    let (profiles, masks): (Vec<_>, Vec<_>) = digests
        .into_iter()
        .partition(|d| catalog["profiles"].get(d.as_str()).is_some());
    let canonical = json!({
        "cache": 1,
        "purpose": "studio-preview",
        "preview": PREVIEW_VERSION,
        "engine": catalog::ENGINE,
        "process": develop["process"],
        "source": photo["source"],
        "profiles": profiles,
        "masks": masks,
        "develop": develop,
        "space": request.space,
        "size": request.edge,
        "overlay": request.overlay.name(),
        "uncropped": request.uncropped,
    });
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&canonical)?)))
}

fn collect_digests(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) if s.starts_with("sha256:") => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| collect_digests(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect_digests(v, out)),
        _ => {}
    }
}

/// `png` with a `pnCk` chunk holding `payload` inserted before `IEND`.
fn join(png: &[u8], payload: &[u8]) -> Option<Vec<u8>> {
    let iend = png.len().checked_sub(12)?;
    if png.get(iend + 4..iend + 8)? != b"IEND" {
        return None;
    }
    let mut out = Vec::with_capacity(png.len() + payload.len() + 12);
    out.extend_from_slice(&png[..iend]);
    out.extend_from_slice(&(u32::try_from(payload.len()).ok()?).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(&CHUNK);
    out.extend_from_slice(payload);
    let crc = png::crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&png[iend..]);
    Some(out)
}

/// The PNG without its `pnCk` chunk, and that chunk's data. `None` unless
/// every chunk's CRC is valid and exactly one `pnCk` chunk is present.
fn split(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    png::chunks(bytes).ok()?;
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..8]);
    let mut payload = None;
    let mut at = 8;
    while at < bytes.len() {
        let length = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let end = at + 12 + length;
        if bytes[at + 4..at + 8] == CHUNK {
            if payload.is_some() {
                return None;
            }
            payload = Some(bytes[at + 8..end - 4].to_vec());
        } else {
            out.extend_from_slice(&bytes[at..end]);
        }
        at = end;
    }
    Some((out, payload?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo::pixels::{Raster, Samples};

    fn png() -> Vec<u8> {
        let raster = Raster::new(2, 1, false, Samples::Eight(vec![1, 2, 3, 4, 5, 6])).unwrap();
        png::write(&raster).unwrap()
    }

    #[test]
    fn entries_round_trip_and_strip_their_chunk() {
        let original = png();
        let joined = join(&original, b"{}").unwrap();
        assert!(png::chunks(&joined).is_ok());
        let (stripped, payload) = split(&joined).unwrap();
        assert_eq!(stripped, original);
        assert_eq!(payload, b"{}");
        // No chunk, a damaged CRC, or two chunks: not an entry.
        assert!(split(&original).is_none());
        let mut damaged = joined.clone();
        let at = joined.len() - 14;
        damaged[at] ^= 1;
        assert!(split(&damaged).is_none());
        let twice = join(&joined, b"{}").unwrap();
        assert!(split(&twice).is_none());
    }

    #[test]
    fn get_requires_the_embedded_key_and_put_respects_the_bound() {
        let dir = std::env::temp_dir().join(format!("pentool-cache-unit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let document = dir.join("doc.pen");
        let cache = Cache::with_limit(&document, 1 << 20);
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        assert!(cache.put(&a, &png(), &json!({"n": 1})));
        assert_eq!(cache.get(&a).unwrap(), (png(), json!({"n": 1})));
        // An entry renamed to another key does not match its embedded key.
        fs::copy(cache.path(&a), cache.path(&b)).unwrap();
        assert!(cache.get(&b).is_none());
        assert!(!Cache::with_limit(&document, 0).put(&a, &png(), &json!({})));
        let report = clear(&document);
        assert!(report.is_err(), "clear needs the document to exist");
        fs::write(&document, b"{}").unwrap();
        let report = clear(&document).unwrap();
        assert_eq!(report["removed_entries"], 2);
        assert!(cache.get(&a).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }
}

//! Render results kept in `DOC_DIR/.pentool/cache/` between CLI processes, so
//! the next preview or export of an edited document re-renders only what
//! changed. Entries are derived data: content-addressed by the renderer's own
//! sha256 keys, written atomically, checksummed, bounded in total size (oldest
//! first out), and never needed to open or render a document. A missing,
//! corrupt or unreadable entry is a miss.
//!
//! The directory name carries the entry format and the executable build, so a
//! different Pentool build never reads another build's pixels.

use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Layout of an entry file; bump when it or the meaning of an entry changes.
const FORMAT: u32 = 1;
const MAGIC: &[u8; 4] = b"PNRC";
/// Cache size per document directory; the least recently used entries go first.
pub const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
/// Larger entries are not stored.
pub const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;

static ENABLED: AtomicBool = AtomicBool::new(false);
static WRITTEN: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Turn the on-disk cache on or off for this process (off by default).
pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The cache directory for `document`, or `None` when the cache is off.
pub fn directory(document: &Path) -> Option<PathBuf> {
    if !enabled() {
        return None;
    }
    Some(
        crate::resource::document_root(document)
            .join(".pentool")
            .join("cache")
            .join(format!("render-{}", namespace())),
    )
}

/// Format version, package version and this executable's size and time.
fn namespace() -> &'static str {
    static NAMESPACE: OnceLock<String> = OnceLock::new();
    NAMESPACE.get_or_init(|| {
        let mut hasher = Sha256::new();
        hasher.update(FORMAT.to_le_bytes());
        hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
        if let Some(metadata) = std::env::current_exe()
            .ok()
            .and_then(|exe| std::fs::metadata(exe).ok())
        {
            hasher.update(metadata.len().to_le_bytes());
            if let Ok(since) = metadata
                .modified()
                .map(|time| time.duration_since(std::time::UNIX_EPOCH))
            {
                hasher.update(since.unwrap_or_default().as_nanos().to_le_bytes());
            }
        }
        hex::encode(&hasher.finalize()[..8])
    })
}

fn entry(directory: &Path, key: &[u8; 32]) -> PathBuf {
    directory.join(format!("{}.bin", hex::encode(key)))
}

/// A 64-bit checksum against torn or damaged files (not against tampering:
/// the cache is as trusted as the document beside it).
fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut words = bytes.chunks_exact(8);
    for word in &mut words {
        hash = (hash ^ u64::from_le_bytes(word.try_into().unwrap())).wrapping_mul(0x100_0000_01b3);
    }
    for &byte in words.remainder() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash ^ bytes.len() as u64
}

/// The payload stored under `key`, if it is present and intact. A damaged
/// entry is removed.
pub fn read(directory: &Path, key: &[u8; 32]) -> Option<Vec<u8>> {
    let path = entry(directory, key);
    let mut bytes = std::fs::read(&path).ok()?;
    let intact = bytes.len() >= 12 && &bytes[..4] == MAGIC && {
        let (body, sum) = bytes.split_at(bytes.len() - 8);
        checksum(&body[4..]) == u64::from_le_bytes(sum.try_into().unwrap())
    };
    if !intact {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    // Recently used entries survive pruning.
    if let Ok(file) = std::fs::File::options().write(true).open(&path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
    bytes.truncate(bytes.len() - 8);
    bytes.drain(..4);
    Some(bytes)
}

/// Store `payload` under `key`. Best effort: a cache that cannot be written
/// only means the next render is slower.
pub fn write(directory: &Path, key: &[u8; 32], payload: &[u8]) {
    if payload.len() > MAX_ENTRY_BYTES || std::fs::create_dir_all(directory).is_err() {
        return;
    }
    let path = entry(directory, key);
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    let mut bytes = Vec::with_capacity(payload.len() + 12);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&checksum(payload).to_le_bytes());
    if std::fs::write(&temporary, &bytes).is_ok() && std::fs::rename(&temporary, &path).is_ok() {
        WRITTEN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(directory.to_owned());
    } else {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Bound every cache directory this process wrote to: drop other builds'
/// directories and the least recently used entries above [`MAX_CACHE_BYTES`].
pub fn finish() {
    let written = std::mem::take(&mut *WRITTEN.lock().unwrap_or_else(|e| e.into_inner()));
    for directory in written {
        prune(&directory, MAX_CACHE_BYTES);
    }
}

fn prune(directory: &Path, limit: u64) {
    if let (Some(parent), Some(name)) = (directory.parent(), directory.file_name()) {
        for sibling in std::fs::read_dir(parent).into_iter().flatten().flatten() {
            let other = sibling.file_name();
            if other != name && other.to_string_lossy().starts_with("render-") {
                let _ = std::fs::remove_dir_all(sibling.path());
            }
        }
    }
    let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then_some(())?;
            Some((metadata.modified().ok()?, metadata.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
    entries.sort();
    for (_, len, path) in entries {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_and_damage_is_a_miss() {
        let dir = std::env::temp_dir().join(format!("pentool-render-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let key = [7u8; 32];
        assert_eq!(read(&dir, &key), None);
        write(&dir, &key, b"pixels and accounting");
        assert_eq!(
            read(&dir, &key).as_deref(),
            Some(&b"pixels and accounting"[..])
        );
        let path = entry(&dir, &key);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[6] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(read(&dir, &key), None);
        assert!(!path.exists(), "a damaged entry is removed");
        std::fs::write(&path, b"PN").unwrap();
        assert_eq!(read(&dir, &key), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_keeps_the_newest_entries_within_the_limit() {
        let root =
            std::env::temp_dir().join(format!("pentool-render-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("render-current");
        let stale = root.join("render-old-build");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("x.bin"), b"old").unwrap();
        let start = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        for n in 0..4u8 {
            write(&dir, &[n; 32], &[n; 100]);
            let file = std::fs::File::options()
                .write(true)
                .open(entry(&dir, &[n; 32]))
                .unwrap();
            file.set_modified(start + std::time::Duration::from_secs(u64::from(n)))
                .unwrap();
        }
        // Each entry is 112 bytes; two fit.
        prune(&dir, 250);
        assert!(!stale.exists());
        let kept: Vec<bool> = (0..4u8).map(|n| entry(&dir, &[n; 32]).exists()).collect();
        assert_eq!(kept, [false, false, true, true]);
        let _ = std::fs::remove_dir_all(&root);
    }
}

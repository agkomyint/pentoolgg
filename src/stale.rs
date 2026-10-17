//! Bounded stale-lock and stale-temp policy shared by the hidden history and
//! the collaboration log.
//!
//! A lock file is held for milliseconds. One that has not been modified for
//! [`STALE_AFTER`] belongs to a process that died, so it is reclaimed instead
//! of blocking every later writer forever. Reclaiming renames the lock to a
//! unique name first, so of several racing reclaimers only one succeeds. A
//! holder that stalls for longer than [`STALE_AFTER`] without dying can lose
//! its lock; that residual risk is documented in `docs/collaboration-v1.md`.
use anyhow::{bail, Result};
use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime},
};

/// Age after which an unmodified lock or temporary file is considered orphaned.
pub const STALE_AFTER: Duration = Duration::from_secs(60);

/// A held lock file, removed on drop.
pub struct LockFile(PathBuf);
impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    SystemTime::now().duration_since(modified).ok()
}

/// Whether `path` exists and has not been modified for `after`.
pub fn is_stale(path: &Path, after: Duration) -> bool {
    age(path).is_some_and(|age| age >= after)
}

/// Take `path` as an exclusive lock, reclaiming it once if it is stale.
pub fn acquire(path: &Path, stale_after: Duration, what: &str) -> Result<LockFile> {
    let mut reclaimed = false;
    for _ in 0..100 {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(_) => return Ok(LockFile(path.to_path_buf())),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if !reclaimed && is_stale(path, stale_after) {
                    reclaimed = true;
                    let aside = path.with_extension(format!("stale-{}", std::process::id()));
                    if fs::rename(path, &aside).is_ok() {
                        let _ = fs::remove_file(&aside);
                    }
                    continue;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("{what} is locked by another pentool process")
}

/// Remove files in `dir` whose name satisfies `matches` and that are older than
/// `after`. Returns the removed count. Missing directories are not an error.
pub fn sweep(dir: &Path, after: Duration, matches: impl Fn(&str) -> bool) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_file()
            && matches(&name)
            && is_stale(&path, after)
            && fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// `NAME.<digits>.tmp` and `NAME.stale-<digits>` leftovers of an interrupted
/// atomic file replacement or lock reclaim.
pub fn is_pid_temp(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(stem) = name.strip_suffix(".tmp") {
        return stem.rsplit('.').next().is_some_and(digits);
    }
    name.rsplit_once(".stale-")
        .is_some_and(|(_, pid)| digits(pid))
}

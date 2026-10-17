//! Append-only, hash-chained operation log (spec sections 3.5, 8.4 and 10).
//!
//! Layout under `<doc dir>/.pentool/collab/<safe-name>/`:
//!
//! ```text
//! genesis.pen   document bytes the chain starts from
//! log.jsonl     one accepted operation per line, in apply order
//! head          {"seq": N, "hash": "sha256:…"} committing the chain tip
//! actors.json   actor registry
//! peer          this installation's peer id
//! append.tmp    write-ahead record of an append whose document commit is pending
//! lock          held while a mutation and its log append are in flight
//! ```
use super::op::{canonical_json, parse_op, sha256_hex, Actor, Op, MAX_OP_BYTES};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

/// Log length at which compaction is advised; appends stop here until the
/// compaction phase ships.
pub const MAX_LOG_OPS: usize = 100_000;
/// Log directory size bound.
pub const MAX_LOG_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_ACTORS: usize = 256;
const LINE_SLACK: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub seq: u64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub seq: u64,
    pub op: Op,
    pub applied_at_revision: String,
    pub prev: String,
    pub hash: String,
}

impl Line {
    pub fn new(seq: u64, op: Op, applied_at_revision: &str, prev: &str) -> Line {
        let mut line = Line {
            seq,
            op,
            applied_at_revision: applied_at_revision.into(),
            prev: prev.into(),
            hash: String::new(),
        };
        line.hash = line.compute_hash();
        line
    }

    pub fn compute_hash(&self) -> String {
        let body = serde_json::json!({
            "seq": self.seq,
            "op": self.op,
            "applied_at_revision": self.applied_at_revision,
            "prev": self.prev,
        });
        format!("sha256:{}", sha256_hex(canonical_json(&body).as_bytes()))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Staged {
    after_revision: String,
    actors: Vec<Actor>,
    lines: Vec<Line>,
}

pub struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn dir(document: &Path) -> Result<PathBuf> {
    let parent = document.parent().unwrap_or_else(|| Path::new("."));
    Ok(parent
        .join(".pentool")
        .join("collab")
        .join(crate::history::safe_name(document)?))
}

pub fn enabled(dir: &Path) -> bool {
    dir.join("head").is_file() && dir.join("genesis.pen").is_file()
}

pub fn lock(dir: &Path) -> Result<Lock> {
    fs::create_dir_all(dir)?;
    let path = dir.join("lock");
    for _ in 0..100 {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(Lock(path)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("collaboration log is locked by another pentool process")
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temp, path)?;
    Ok(())
}

pub fn genesis_hash(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

/// Start a new chain at the document's current bytes.
pub fn init(dir: &Path, document_bytes: &[u8]) -> Result<Head> {
    let _lock = lock(dir)?;
    if enabled(dir) {
        bail!("[invalid-input] collaboration is already enabled for this document")
    }
    let head = Head {
        seq: 0,
        hash: genesis_hash(document_bytes),
    };
    write_atomic(&dir.join("genesis.pen"), document_bytes)?;
    write_atomic(&dir.join("log.jsonl"), b"")?;
    write_atomic(&dir.join("actors.json"), b"{\"actors\":[]}\n")?;
    write_atomic(&dir.join("peer"), super::hlc::fresh_peer_id().as_bytes())?;
    write_atomic(&dir.join("head"), &serde_json::to_vec(&head)?)?;
    Ok(head)
}

pub fn peer_id(dir: &Path) -> Result<String> {
    let text = fs::read_to_string(dir.join("peer")).context("collaboration peer id is missing")?;
    Ok(text.trim().to_string())
}

pub fn read_head(dir: &Path) -> Result<Head> {
    serde_json::from_slice(&fs::read(dir.join("head")).context("collaboration head is missing")?)
        .context("[corrupt-log] collaboration head is malformed")
}

pub fn read_genesis(dir: &Path) -> Result<(Vec<u8>, Value)> {
    let bytes = fs::read(dir.join("genesis.pen")).context("collaboration genesis is missing")?;
    let value = serde_json::from_slice(&bytes).context("[corrupt-log] genesis is not JSON")?;
    Ok((bytes, value))
}

pub fn read_actors(dir: &Path) -> Result<Vec<Actor>> {
    #[derive(Deserialize)]
    struct Registry {
        actors: Vec<Actor>,
    }
    let bytes = fs::read(dir.join("actors.json")).context("actor registry is missing")?;
    Ok(serde_json::from_slice::<Registry>(&bytes)
        .context("[corrupt-log] actor registry is malformed")?
        .actors)
}

/// Read every line, enforcing size bounds before parsing each one.
pub fn read_lines(dir: &Path) -> Result<Vec<Line>> {
    let path = dir.join("log.jsonl");
    let size = fs::metadata(&path)
        .context("collaboration log is missing")?
        .len();
    if size > MAX_LOG_BYTES {
        bail!("[limit-exceeded] collaboration log is {size} bytes; the limit is {MAX_LOG_BYTES}")
    }
    let text = fs::read_to_string(&path).context("[corrupt-log] log is not valid UTF-8")?;
    let mut lines = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        if raw.len() > MAX_OP_BYTES + LINE_SLACK {
            bail!(
                "[limit-exceeded] log line {} exceeds {MAX_OP_BYTES} bytes",
                index + 1
            )
        }
        let line: Line = serde_json::from_str(raw).map_err(|error| {
            anyhow::anyhow!("[corrupt-log] log line {} is malformed: {error}", index + 1)
        })?;
        lines.push(line);
    }
    Ok(lines)
}

/// Full chain verification: sequence, linkage, hashes, op validity, and head.
pub fn verify_chain(dir: &Path, lines: &[Line]) -> Result<Head> {
    let head = read_head(dir)?;
    let (genesis_bytes, _) = read_genesis(dir)?;
    let mut prev = genesis_hash(&genesis_bytes);
    let mut seen = std::collections::HashSet::new();
    for (index, line) in lines.iter().enumerate() {
        let expected = index as u64 + 1;
        if line.seq != expected {
            bail!(
                "[tampered-log] line {} has seq {}, expected {expected}: lines were reordered, removed or inserted",
                index + 1,
                line.seq
            )
        }
        if line.prev != prev {
            bail!(
                "[tampered-log] line seq {} does not link to the previous line (chain broken)",
                line.seq
            )
        }
        if line.compute_hash() != line.hash {
            bail!(
                "[tampered-log] line seq {} content does not match its hash",
                line.seq
            )
        }
        let text = serde_json::to_vec(&line.op)?;
        parse_op(&text)
            .with_context(|| format!("[tampered-log] operation at seq {} is invalid", line.seq))?;
        if !seen.insert(line.op.id.clone()) {
            bail!(
                "[tampered-log] operation {} appears twice (seq {})",
                line.op.id,
                line.seq
            )
        }
        prev = line.hash.clone();
    }
    let tip = Head {
        seq: lines.len() as u64,
        hash: prev,
    };
    if head != tip {
        bail!(
            "[tampered-log] head says seq {} {} but the log ends at seq {} {}",
            head.seq,
            head.hash,
            tip.seq,
            tip.hash
        )
    }
    Ok(head)
}

/// Cheap tail check used on every append: head matches the last line.
pub fn check_tail(dir: &Path, lines: &[Line]) -> Result<Head> {
    let head = read_head(dir)?;
    let tip = match lines.last() {
        Some(last) => Head {
            seq: last.seq,
            hash: last.compute_hash(),
        },
        None => head.clone(),
    };
    if lines.last().is_some_and(|last| last.hash != tip.hash) || head.seq != lines.len() as u64 {
        bail!(
            "[tampered-log] head (seq {}) does not match the log ({} lines); run `pentool collab verify`",
            head.seq,
            lines.len()
        )
    }
    if !lines.is_empty() && head.hash != tip.hash {
        bail!("[tampered-log] head hash does not match the last log line; run `pentool collab verify`")
    }
    Ok(head)
}

/// A prepared append: written ahead of the document commit.
pub struct Prepared {
    dir: PathBuf,
    staged: Staged,
    _lock: Lock,
}

/// Step 2 of section 8.4: stage the lines durably before the document commit.
pub fn prepare(
    dir: &Path,
    lock: Lock,
    lines: Vec<Line>,
    actors: Vec<Actor>,
    after_revision: &str,
) -> Result<Prepared> {
    let existing = read_actors(dir)?;
    let mut registry = existing;
    for actor in actors {
        if !registry.iter().any(|known| known.id == actor.id) {
            registry.push(actor);
        }
    }
    if registry.len() > MAX_ACTORS {
        bail!("[limit-exceeded] actor registry is limited to {MAX_ACTORS} actors")
    }
    let log_size = fs::metadata(dir.join("log.jsonl"))
        .map(|m| m.len())
        .unwrap_or(0);
    let added: usize = lines
        .iter()
        .map(|line| serde_json::to_vec(line).map(|v| v.len() + 1))
        .sum::<serde_json::Result<usize>>()?;
    if log_size + added as u64 > MAX_LOG_BYTES {
        bail!("[limit-exceeded] collaboration log would exceed {MAX_LOG_BYTES} bytes")
    }
    let staged = Staged {
        after_revision: after_revision.into(),
        actors: registry,
        lines,
    };
    let bytes = serde_json::to_vec(&staged)?;
    let mut file = File::create(dir.join("append.tmp"))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(Prepared {
        dir: dir.into(),
        staged,
        _lock: lock,
    })
}

impl Prepared {
    /// Step 4: the document is committed; make the log lines and head durable.
    pub fn finalize(self) -> Result<()> {
        apply_staged(&self.dir, &self.staged)
    }

    /// The document commit failed: forget the staged lines.
    pub fn abort(self) {
        let _ = fs::remove_file(self.dir.join("append.tmp"));
    }
}

fn apply_staged(dir: &Path, staged: &Staged) -> Result<()> {
    let registry = serde_json::json!({"actors": staged.actors});
    write_atomic(&dir.join("actors.json"), &serde_json::to_vec(&registry)?)?;
    let existing = read_lines(dir)?;
    let mut last = existing.len() as u64;
    let mut file = OpenOptions::new()
        .append(true)
        .open(dir.join("log.jsonl"))?;
    for line in &staged.lines {
        if line.seq <= last {
            continue;
        }
        let mut bytes = serde_json::to_vec(line)?;
        bytes.push(b'\n');
        file.write_all(&bytes)?;
        last = line.seq;
    }
    file.sync_all()?;
    if let Some(tip) = staged.lines.last() {
        let head = Head {
            seq: tip.seq,
            hash: tip.hash.clone(),
        };
        write_atomic(&dir.join("head"), &serde_json::to_vec(&head)?)?;
    }
    let _ = fs::remove_file(dir.join("append.tmp"));
    Ok(())
}

/// Reconcile an interrupted append (spec section 8.4). Run under the lock.
///
/// * staged record present and the document already has its revision: the
///   document commit happened, so finish the append;
/// * staged record present but the document does not match: the operation was
///   never applied, discard it;
/// * log lines beyond `head` are uncommitted and are cut off.
pub fn recover(dir: &Path, current_revision: &str) -> Result<()> {
    let staged_path = dir.join("append.tmp");
    if staged_path.is_file() {
        let staged: Option<Staged> = fs::read(&staged_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        match staged {
            Some(staged) if staged.after_revision == current_revision => {
                let head = read_head(dir)?;
                let continues = staged
                    .lines
                    .first()
                    .is_some_and(|first| first.seq <= head.seq + 1);
                if continues {
                    apply_staged(dir, &staged)?;
                } else {
                    let _ = fs::remove_file(&staged_path);
                }
            }
            _ => {
                let _ = fs::remove_file(&staged_path);
            }
        }
    }
    let head = read_head(dir)?;
    let lines = read_lines(dir)?;
    if lines.len() as u64 > head.seq {
        let kept: Vec<&Line> = lines.iter().take(head.seq as usize).collect();
        let mut bytes = Vec::new();
        for line in kept {
            bytes.extend(serde_json::to_vec(line)?);
            bytes.push(b'\n');
        }
        write_atomic(&dir.join("log.jsonl"), &bytes)?;
    }
    Ok(())
}

pub fn ops_of(lines: &[Line]) -> Vec<Op> {
    lines.iter().map(|line| line.op.clone()).collect()
}

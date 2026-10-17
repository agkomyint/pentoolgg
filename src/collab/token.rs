//! Access tokens and roles (spec sections 9.1 and 9.2).
//!
//! Tokens are random 256-bit secrets shown once. Only a SHA-256 hash, the role,
//! the actor the token acts as and the expiry are stored, in
//! `<collab dir>/tokens.json` (never in the `.pen`). A token is presented in an
//! `Authorization: Bearer` header and compared in constant time.
use super::{log, op::Actor};
use anyhow::{bail, Context, Result};
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub const MAX_TOKENS: usize = 1024;
const PREFIX: &str = "ptc";
const MAX_LABEL: usize = 64;
const MAX_EXPIRY_MS: u64 = 366 * 24 * 3600 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Commenter,
    Editor,
    Owner,
}

impl Role {
    pub fn parse(text: &str) -> Result<Role> {
        Ok(match text {
            "viewer" => Role::Viewer,
            "commenter" => Role::Commenter,
            "editor" => Role::Editor,
            "owner" => Role::Owner,
            other => bail!(
                "[invalid-input] role must be viewer, commenter, editor or owner, got {other:?}"
            ),
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Commenter => "commenter",
            Role::Editor => "editor",
            Role::Owner => "owner",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    /// `sha256` of the secret, hex. Never shown.
    pub hash: String,
    pub role: Role,
    pub name: String,
    pub actor: Actor,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_ms: Option<u64>,
}

impl Record {
    /// Listing view: everything except the hash.
    pub fn view(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "role": self.role.name(),
            "actor": self.actor,
            "created_ms": self.created_ms,
            "expires_ms": self.expires_ms,
            "revoked_ms": self.revoked_ms,
            "active": self.active(super::hlc::now_ms()),
        })
    }
    pub fn active(&self, now: u64) -> bool {
        self.revoked_ms.is_none() && self.expires_ms.is_none_or(|expiry| expiry > now)
    }
}

/// Who is making a request, derived from the token and nothing else.
#[derive(Debug, Clone)]
pub struct Principal {
    /// `None` for the local host user on a loopback server.
    pub token: Option<String>,
    pub role: Role,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    Missing,
    Invalid,
    Expired,
    Revoked,
}

impl AuthError {
    pub fn code(&self) -> &'static str {
        match self {
            AuthError::Missing => "unauthenticated",
            AuthError::Invalid => "invalid-token",
            AuthError::Expired => "token-expired",
            AuthError::Revoked => "token-revoked",
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    #[serde(default)]
    tokens: Vec<Record>,
}

fn path(dir: &Path) -> std::path::PathBuf {
    dir.join("tokens.json")
}

fn load(dir: &Path) -> Result<Store> {
    match fs::read(path(dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .context("[corrupt-log] token store is malformed; restore it or recreate tokens"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(error) => Err(error.into()),
    }
}

fn save(dir: &Path, store: &Store) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(store)?;
    log::write_atomic(&path(dir), &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path(dir), fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(
        format!("pentool-collab-token-v1:{secret}").as_bytes(),
    ))
}

/// Constant-time equality over equal-length byte strings.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand_core::OsRng.fill_bytes(&mut buffer);
    hex::encode(buffer)
}

/// Parse `30m`, `12h`, `7d` or plain seconds.
pub fn parse_duration_ms(text: &str) -> Result<u64> {
    let text = text.trim();
    let (digits, unit) = match text.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((index, _)) => text.split_at(index),
        None => (text, "s"),
    };
    let value: u64 = digits.parse().map_err(|_| {
        anyhow::anyhow!("[invalid-input] expiry {text:?} is not a duration like 30m, 12h or 7d")
    })?;
    let factor = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => bail!("[invalid-input] expiry {text:?} is not a duration like 30m, 12h or 7d"),
    };
    let ms = value.saturating_mul(factor);
    if ms == 0 || ms > MAX_EXPIRY_MS {
        bail!("[invalid-input] expiry must be between 1 second and 366 days")
    }
    Ok(ms)
}

/// Create a token. Returns the stored record and the secret (shown once).
pub fn create(
    dir: &Path,
    role: Role,
    actor: Actor,
    name: &str,
    expires_in_ms: Option<u64>,
) -> Result<(Record, String)> {
    actor.validate()?;
    if name.is_empty() || name.len() > MAX_LABEL || name.chars().any(char::is_control) {
        bail!("[invalid-input] token --name must be 1 to {MAX_LABEL} printable characters")
    }
    let _lock = log::lock(dir)?;
    let mut store = load(dir)?;
    if store.tokens.len() >= MAX_TOKENS {
        bail!("[limit-exceeded] a document is limited to {MAX_TOKENS} tokens; revoke unused ones")
    }
    let id = random_hex(4);
    let secret = format!("{PREFIX}_{id}_{}", random_hex(32));
    let now = super::hlc::now_ms();
    let record = Record {
        id,
        hash: hash_secret(&secret),
        role,
        name: name.into(),
        actor,
        created_ms: now,
        expires_ms: expires_in_ms.map(|ms| now.saturating_add(ms)),
        revoked_ms: None,
    };
    store.tokens.push(record.clone());
    save(dir, &store)?;
    Ok((record, secret))
}

pub fn list(dir: &Path) -> Result<Vec<Record>> {
    Ok(load(dir)?.tokens)
}

pub fn count_active(dir: &Path) -> usize {
    let now = super::hlc::now_ms();
    load(dir)
        .map(|store| store.tokens.iter().filter(|t| t.active(now)).count())
        .unwrap_or(0)
}

pub fn revoke(dir: &Path, id: &str) -> Result<Record> {
    let _lock = log::lock(dir)?;
    let mut store = load(dir)?;
    let record = store
        .tokens
        .iter_mut()
        .find(|t| t.id == id)
        .with_context(|| format!("[not-found] no token {id:?}; run `pentool collab token list`"))?;
    if record.revoked_ms.is_none() {
        record.revoked_ms = Some(super::hlc::now_ms());
    }
    let out = record.clone();
    save(dir, &store)?;
    Ok(out)
}

pub fn set_role(dir: &Path, id: &str, role: Role) -> Result<Record> {
    let _lock = log::lock(dir)?;
    let mut store = load(dir)?;
    let record = store
        .tokens
        .iter_mut()
        .find(|t| t.id == id)
        .with_context(|| format!("[not-found] no token {id:?}; run `pentool collab token list`"))?;
    record.role = role;
    let out = record.clone();
    save(dir, &store)?;
    Ok(out)
}

/// Authenticate a presented secret. Every stored hash is compared (no early
/// exit) so timing does not reveal which token matched.
pub fn authenticate(dir: &Path, secret: &str) -> std::result::Result<Principal, AuthError> {
    if secret.is_empty() || secret.len() > 256 {
        return Err(AuthError::Invalid);
    }
    let store = load(dir).map_err(|_| AuthError::Invalid)?;
    let presented = hash_secret(secret);
    let mut found: Option<&Record> = None;
    for record in &store.tokens {
        if ct_eq(record.hash.as_bytes(), presented.as_bytes()) {
            found = Some(record);
        }
    }
    let record = found.ok_or(AuthError::Invalid)?;
    if record.revoked_ms.is_some() {
        return Err(AuthError::Revoked);
    }
    if !record.active(super::hlc::now_ms()) {
        return Err(AuthError::Expired);
    }
    Ok(Principal {
        token: Some(record.id.clone()),
        role: record.role,
        actor: record.actor.clone(),
    })
}

/// Does `actor` hold an active token of at least `role`?
pub fn actor_has_role(dir: &Path, actor: &str, role: Role) -> bool {
    let now = super::hlc::now_ms();
    load(dir)
        .map(|store| {
            store
                .tokens
                .iter()
                .any(|t| t.actor.id == actor && t.role >= role && t.active(now))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Actor {
        Actor {
            id: Actor::derive_id("human", "bob"),
            kind: "human".into(),
            display_name: Some("Bob".into()),
            tool: None,
            model: None,
            directed_by: None,
        }
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration_ms("30m").unwrap(), 30 * 60_000);
        assert_eq!(parse_duration_ms("2h").unwrap(), 2 * 3_600_000);
        assert_eq!(parse_duration_ms("90").unwrap(), 90_000);
        assert!(parse_duration_ms("0").is_err());
        assert!(parse_duration_ms("9999d").is_err());
        assert!(parse_duration_ms("x").is_err());
    }

    #[test]
    fn secret_is_never_stored() {
        let dir = std::env::temp_dir().join(format!("pt-token-{}", super::random_hex(6)));
        fs::create_dir_all(&dir).unwrap();
        let (record, secret) = create(&dir, Role::Editor, actor(), "laptop", None).unwrap();
        let stored = fs::read_to_string(path(&dir)).unwrap();
        assert!(!stored.contains(&secret));
        assert!(!stored.contains(secret.rsplit('_').next().unwrap()));
        assert_eq!(authenticate(&dir, &secret).unwrap().role, Role::Editor);
        assert_eq!(
            authenticate(&dir, "ptc_zz_00").unwrap_err(),
            AuthError::Invalid
        );
        revoke(&dir, &record.id).unwrap();
        assert_eq!(authenticate(&dir, &secret).unwrap_err(), AuthError::Revoked);
        let _ = fs::remove_dir_all(&dir);
    }
}

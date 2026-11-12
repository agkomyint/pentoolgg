//! Claims (spec section 6.4): short-lived advisory locks on objects. They live
//! in the core because every mutation path (`collab::begin`) must honour them;
//! the HTTP routes that create and release them stay in `pentool::collab::sync`.
use super::{
    hlc::now_ms,
    log,
    op::{Actor, Op},
};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub actor: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub page: String,
    pub targets: Vec<String>,
    pub expires_ms: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct ClaimStore {
    #[serde(default)]
    pub claims: Vec<Claim>,
}

pub const MAX_CLAIM_TARGETS: usize = 64;

pub fn load_claims(dir: &Path) -> Vec<Claim> {
    let now = now_ms();
    fs::read(dir.join("claims.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ClaimStore>(&bytes).ok())
        .map(|store| {
            store
                .claims
                .into_iter()
                .filter(|claim| claim.expires_ms > now)
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_claims(dir: &Path, claims: Vec<Claim>) -> Result<()> {
    log::write_atomic(
        &dir.join("claims.json"),
        &serde_json::to_vec_pretty(&ClaimStore { claims })?,
    )
}

pub fn covers(claim: &Claim, page: &str, id: &str) -> bool {
    (claim.page.is_empty() || page.is_empty() || claim.page == page)
        && claim.targets.iter().any(|target| target == id)
}

/// Who blocks `actor` from touching `ids`? A human may always override an
/// agent's claim; an agent may not override a human's (spec 6.4).
pub fn blocking_claim<'a>(
    claims: &'a [Claim],
    actor: &Actor,
    page: &str,
    ids: &[&str],
) -> Option<&'a Claim> {
    claims.iter().find(|claim| {
        claim.actor != actor.id
            && !(actor.kind == "human" && claim.kind == "agent")
            && ids.iter().any(|id| covers(claim, page, id))
    })
}

pub fn op_ids(op: &Op) -> Vec<&str> {
    let mut ids: Vec<&str> = op.targets.iter().map(String::as_str).collect();
    if let Some(parent) = op
        .payload
        .get("parent")
        .and_then(|p| p.get("id"))
        .and_then(Value::as_str)
    {
        ids.push(parent);
    }
    ids
}

pub fn locked_message(claim: &Claim, op: &Op) -> String {
    format!(
        "operation {} touches objects claimed by {} until {}; wait for the claim to expire or ask the owner to release it",
        op.id,
        claim.name.as_deref().unwrap_or(&claim.actor),
        claim.expires_ms
    )
}

/// Reject local operations that touch another actor's active claim. Called by
/// the transaction hook so every mutation path honours claims.
pub fn check_claims<'a>(
    dir: &Path,
    actor: &Actor,
    ops: impl Iterator<Item = &'a Op>,
) -> Result<()> {
    let claims = load_claims(dir);
    if claims.is_empty() {
        return Ok(());
    }
    for op in ops {
        if let Some(claim) = blocking_claim(&claims, actor, &op.page, &op_ids(op)) {
            bail!("[locked] {}", locked_message(claim, op))
        }
    }
    Ok(())
}

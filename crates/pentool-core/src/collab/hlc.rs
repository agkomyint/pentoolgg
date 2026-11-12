//! Hybrid logical clock (spec section 3.2).
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// Operations stamped further ahead of the receiver's clock are rejected.
pub const MAX_CLOCK_SKEW_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
    pub peer: String,
}

impl Hlc {
    /// The `(wall_ms, counter)` pair used by the total order; the peer id is
    /// informational and never breaks ties (the actor id and op id do).
    pub fn stamp(&self) -> (u64, u32) {
        (self.wall_ms, self.counter)
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Monotonic local clock: every tick is strictly after the previous tick and
/// after every observed remote stamp.
#[derive(Debug, Clone)]
pub struct Clock {
    peer: String,
    wall_ms: u64,
    counter: u32,
}

impl Clock {
    pub fn new(peer: &str, last: Option<(u64, u32)>) -> Self {
        let (wall_ms, counter) = last.unwrap_or((0, 0));
        Self {
            peer: peer.into(),
            wall_ms,
            counter,
        }
    }

    pub fn tick(&mut self, now: u64) -> Hlc {
        if now > self.wall_ms {
            self.wall_ms = now;
            self.counter = 0;
        } else {
            self.counter = self.counter.saturating_add(1);
        }
        Hlc {
            wall_ms: self.wall_ms,
            counter: self.counter,
            peer: self.peer.clone(),
        }
    }

    /// Merge a received stamp. Stamps beyond the allowed skew are refused so a
    /// peer cannot win every future conflict with a far-future timestamp.
    pub fn observe(&mut self, remote: &Hlc, now: u64) -> Result<()> {
        if remote.wall_ms > now.saturating_add(MAX_CLOCK_SKEW_MS) {
            bail!(
                "[clock-skew] operation clock {} is more than {} ms ahead of local time {now}",
                remote.wall_ms,
                MAX_CLOCK_SKEW_MS
            )
        }
        if (remote.wall_ms, remote.counter) > (self.wall_ms, self.counter) {
            self.wall_ms = remote.wall_ms;
            self.counter = remote.counter;
        }
        Ok(())
    }
}

/// Random per-installation peer id for a log (not secret, only a tiebreak label).
pub fn fresh_peer_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    hasher.write_u32(std::process::id());
    let seed = hasher.finish();
    let digest = Sha256::digest(seed.to_be_bytes());
    format!("peer_{}", hex::encode(&digest[..4]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_strictly_increasing_even_with_a_stalled_wall_clock() {
        let mut clock = Clock::new("peer_a", None);
        let a = clock.tick(100);
        let b = clock.tick(100);
        let c = clock.tick(50);
        assert!(a.stamp() < b.stamp() && b.stamp() < c.stamp());
        assert_eq!(c.wall_ms, 100);
    }

    #[test]
    fn far_future_stamps_are_rejected() {
        let mut clock = Clock::new("peer_a", None);
        let remote = Hlc {
            wall_ms: 1_000_000 + MAX_CLOCK_SKEW_MS + 1,
            counter: 0,
            peer: "peer_b".into(),
        };
        assert!(clock.observe(&remote, 1_000_000).is_err());
        let near = Hlc {
            wall_ms: 1_000_000 + MAX_CLOCK_SKEW_MS,
            ..remote
        };
        clock.observe(&near, 1_000_000).unwrap();
        assert!(clock.tick(1_000_001).stamp() > near.stamp());
    }
}

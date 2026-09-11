use std::collections::VecDeque;
use std::time::{Duration, Instant};

use devboule_protocol::{DaemonMessage, IDEMPOTENCY_MAX_ENTRIES, IDEMPOTENCY_TTL_SECS};
use sha2::{Digest, Sha256};

/// SHA-256 of the caller's fingerprint text.
///
/// The text itself is caller-chosen and can be a 64 KiB prompt
/// (`send_fingerprint` appends the whole send text), and the table holds up
/// to `IDEMPOTENCY_MAX_ENTRIES` of them. Hashing here means one entry costs a
/// fixed 32 bytes regardless of what the caller passed, and every call site
/// is covered without touching it: receipt semantics are unchanged, only the
/// stored representative is shorter.
fn fingerprint_digest(fingerprint: &str) -> [u8; 32] {
    let digest = Sha256::digest(fingerprint.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// `Hit` stores a full reply frame. Boxing it would scatter clones on the
/// retry path for a cache of a few thousand entries.
#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum IdempotencyOutcome {
    Hit(DaemonMessage),
    Conflict,
    Miss,
}

struct Entry {
    owner: String,
    key: String,
    fingerprint: [u8; 32],
    response: DaemonMessage,
    inserted: Instant,
}

pub struct IdempotencyStore {
    entries: VecDeque<Entry>,
    ttl: Duration,
    cap: usize,
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            ttl: Duration::from_secs(IDEMPOTENCY_TTL_SECS),
            cap: IDEMPOTENCY_MAX_ENTRIES,
        }
    }
}

#[allow(dead_code)]
impl IdempotencyStore {
    pub fn check(
        &mut self,
        owner: &str,
        key: &str,
        fingerprint: &str,
        now: Instant,
    ) -> IdempotencyOutcome {
        let fingerprint = fingerprint_digest(fingerprint);
        self.evict(now);
        match self
            .entries
            .iter()
            .find(|entry| entry.owner == owner && entry.key == key)
        {
            Some(entry) if entry.fingerprint == fingerprint => {
                IdempotencyOutcome::Hit(entry.response.clone())
            }
            Some(_) => IdempotencyOutcome::Conflict,
            None => IdempotencyOutcome::Miss,
        }
    }

    pub fn remember(
        &mut self,
        owner: String,
        key: String,
        fingerprint: String,
        response: DaemonMessage,
        now: Instant,
    ) {
        self.evict(now);
        self.entries
            .retain(|entry| !(entry.owner == owner && entry.key == key));
        if self.entries.len() >= self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            owner,
            key,
            fingerprint: fingerprint_digest(&fingerprint),
            response,
            inserted: now,
        });
    }

    fn evict(&mut self, now: Instant) {
        while let Some(front) = self.entries.front() {
            if now.saturating_duration_since(front.inserted) > self.ttl {
                self.entries.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn pong(id: u64) -> DaemonMessage {
        DaemonMessage::Pong { id, ts_ms: 0 }
    }

    #[test]
    fn same_key_same_payload_replays() {
        let mut store = IdempotencyStore::default();
        let now = Instant::now();
        store.remember(
            "app-1".into(),
            "k".into(),
            "create:terminal".into(),
            pong(1),
            now,
        );
        match store.check("app-1", "k", "create:terminal", now) {
            IdempotencyOutcome::Hit(DaemonMessage::Pong { id, .. }) => assert_eq!(id, 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn same_key_different_payload_conflicts() {
        let mut store = IdempotencyStore::default();
        let now = Instant::now();
        store.remember("app-1".into(), "k".into(), "a".into(), pong(1), now);
        assert_eq!(
            store.check("app-1", "k", "b", now),
            IdempotencyOutcome::Conflict
        );
    }

    #[test]
    fn ttl_expiry_is_a_miss() {
        let mut store = IdempotencyStore {
            ttl: Duration::from_secs(1),
            ..IdempotencyStore::default()
        };
        let now = Instant::now();
        store.remember("app-1".into(), "k".into(), "a".into(), pong(1), now);
        let later = now + Duration::from_secs(2);
        assert_eq!(
            store.check("app-1", "k", "a", later),
            IdempotencyOutcome::Miss
        );
    }

    #[test]
    fn a_64_kib_fingerprint_is_stored_as_32_bytes() {
        let mut store = IdempotencyStore::default();
        let now = Instant::now();
        let text = "x".repeat(64 * 1024);
        let fingerprint = format!("send:s.a.1:0:{text}");
        assert_eq!(fingerprint.len(), 64 * 1024 + "send:s.a.1:0:".len());
        store.remember(
            "app-1".into(),
            "k".into(),
            fingerprint.clone(),
            pong(1),
            now,
        );
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].fingerprint.len(), 32);
        // Receipts are unchanged: the same text still hits, a different one
        // still conflicts.
        assert!(matches!(
            store.check("app-1", "k", &fingerprint, now),
            IdempotencyOutcome::Hit(_)
        ));
        assert_eq!(
            store.check("app-1", "k", "send:s.a.1:0:other", now),
            IdempotencyOutcome::Conflict
        );
    }
}

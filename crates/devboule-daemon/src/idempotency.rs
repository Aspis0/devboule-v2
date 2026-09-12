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

/// The largest reply the store will remember, in serialized bytes (F16).
///
/// The fingerprint is already a digest, but the *reply* was kept verbatim: a
/// bulk send's result or a create result with a long title sat in the table at
/// full size, up to `IDEMPOTENCY_MAX_ENTRIES` times. A large reply is rare,
/// and dropping it costs a retry rather than correctness: `check` then reports
/// `Miss` and the request runs again, which is exactly what a caller that
/// never sent a key would get.
pub const IDEMPOTENCY_MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// The serialized size of one reply frame. A frame that cannot be measured
/// counts as over the bound: a store that cannot say how big a value is has no
/// business keeping it.
fn serialized_len(response: &DaemonMessage) -> usize {
    serde_json::to_vec(response).map_or(usize::MAX, |bytes| bytes.len())
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
        // F16: a reply too large to be worth keeping is not kept. Recording
        // the key with no reply would report a `Conflict` on retry (the same
        // key, a fingerprint the store never held), which is a lie about the
        // request; a plain miss lets the request run again.
        if serialized_len(&response) > IDEMPOTENCY_MAX_RESPONSE_BYTES {
            return;
        }
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
    use devboule_protocol::{ErrorCode, WireError};
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
    fn the_store_is_bounded_by_entry_count() {
        // F5. The age bound is covered by `ttl_expiry_is_a_miss`; this is the
        // count bound, which is what stops a caller that invents a fresh
        // `idempotency_key` for every request from growing the table without
        // limit. Entries are evicted from the front, so the *newest* survive.
        let cap = 4;
        let mut store = IdempotencyStore {
            cap,
            ..IdempotencyStore::default()
        };
        let now = Instant::now();
        for index in 0..cap * 3 {
            store.remember(
                "app-1".into(),
                format!("k{index}"),
                format!("payload-{index}"),
                pong(index as u64),
                now + Duration::from_millis(index as u64),
            );
            assert!(
                store.entries.len() <= cap,
                "after {index} distinct keys the store holds {} entries",
                store.entries.len()
            );
        }
        assert_eq!(store.entries.len(), cap, "the store sits at its cap");
        // The oldest keys are gone and the newest still replay.
        assert_eq!(
            store.check("app-1", "k0", "payload-0", now),
            IdempotencyOutcome::Miss
        );
        assert!(matches!(
            store.check(
                "app-1",
                &format!("k{}", cap * 3 - 1),
                &format!("payload-{}", cap * 3 - 1),
                now
            ),
            IdempotencyOutcome::Hit(_)
        ));
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

    /// F16: the *reply* must not be kept verbatim without a bound. A reply
    /// over the bound is dropped, and the retry that follows is a plain miss.
    #[test]
    fn a_reply_larger_than_the_bound_is_not_remembered() {
        let mut store = IdempotencyStore::default();
        let now = Instant::now();
        let large =
            DaemonMessage::Error(WireError::new(ErrorCode::Internal, "x".repeat(300 * 1024)));
        assert!(serialized_len(&large) > IDEMPOTENCY_MAX_RESPONSE_BYTES);
        store.remember("app-1".into(), "big".into(), "a".into(), large, now);
        assert!(store.entries.is_empty(), "an oversized reply is not stored");
        assert_eq!(
            store.check("app-1", "big", "a", now),
            IdempotencyOutcome::Miss,
            "the retry runs the request again rather than replaying a frame \
             the store refused to keep"
        );
    }

    #[test]
    fn a_reply_inside_the_bound_still_replays() {
        let mut store = IdempotencyStore::default();
        let now = Instant::now();
        let small = DaemonMessage::Error(WireError::new(ErrorCode::Internal, "x".repeat(1024)));
        assert!(serialized_len(&small) < IDEMPOTENCY_MAX_RESPONSE_BYTES);
        store.remember("app-1".into(), "small".into(), "a".into(), small, now);
        assert_eq!(store.entries.len(), 1);
        assert!(matches!(
            store.check("app-1", "small", "a", now),
            IdempotencyOutcome::Hit(_)
        ));
    }
}

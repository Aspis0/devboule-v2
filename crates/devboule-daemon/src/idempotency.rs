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

/// How much remembered reply this store may hold in total, in serialized
/// bytes (H9).
///
/// The per-entry bound above bounds *one* reply; the count cap
/// (`IDEMPOTENCY_MAX_ENTRIES`) bounds how many. Neither bounds the product: a
/// caller that keeps the entries alive with replies just under the per-entry
/// bound could pin `IDEMPOTENCY_MAX_ENTRIES * IDEMPOTENCY_MAX_RESPONSE_BYTES`
/// of heap — tens of megabytes, all of it reachable from one connection's
/// request rate. Four mebibytes is sixteen maximum-size replies, far more than
/// a real retry window needs, and small enough to be a bound rather than a
/// hope.
///
/// Eviction is oldest-first, like the count cap: the newest receipt is the one
/// a retrying client is about to use.
pub const IDEMPOTENCY_MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;

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
    /// The serialized size of `response`, measured once at insert: the byte
    /// budget is maintained on this number rather than re-measuring every
    /// frame on every eviction pass.
    bytes: usize,
    inserted: Instant,
}

pub struct IdempotencyStore {
    entries: VecDeque<Entry>,
    ttl: Duration,
    cap: usize,
    /// Sum of `entries[*].bytes`, kept in step with every push and pop.
    total_bytes: usize,
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            ttl: Duration::from_secs(IDEMPOTENCY_TTL_SECS),
            cap: IDEMPOTENCY_MAX_ENTRIES,
            total_bytes: 0,
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
        self.evict_bytes();
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
        let bytes = serialized_len(&response);
        if bytes > IDEMPOTENCY_MAX_RESPONSE_BYTES {
            return;
        }
        let mut replaced = 0usize;
        self.entries.retain(|entry| {
            let same_receipt = entry.owner == owner && entry.key == key;
            if same_receipt {
                replaced += entry.bytes;
            }
            !same_receipt
        });
        self.total_bytes = self.total_bytes.saturating_sub(replaced);
        if self.entries.len() >= self.cap {
            if let Some(evicted) = self.entries.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.bytes);
            }
        }
        self.entries.push_back(Entry {
            owner,
            key,
            fingerprint: fingerprint_digest(&fingerprint),
            response,
            bytes,
            inserted: now,
        });
        self.total_bytes += bytes;
        // H9: the count cap is not a memory bound on its own.
        self.evict_bytes();
    }

    fn evict(&mut self, now: Instant) {
        while let Some(front) = self.entries.front() {
            if now.saturating_duration_since(front.inserted) > self.ttl {
                let expired = self.entries.pop_front().expect("front was just read");
                self.total_bytes = self.total_bytes.saturating_sub(expired.bytes);
            } else {
                break;
            }
        }
    }

    /// Drop the oldest replies until the retained frames fit the byte budget
    /// (H9). Oldest-first for the same reason the count cap is: the newest
    /// receipt is the one a retrying caller is about to present.
    fn evict_bytes(&mut self) {
        while self.total_bytes > IDEMPOTENCY_MAX_TOTAL_BYTES {
            let Some(evicted) = self.entries.pop_front() else {
                // Nothing left to evict: `total_bytes` can only be stale if a
                // pop forgot to subtract, which would be a bug rather than an
                // empty store.
                self.total_bytes = 0;
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.bytes);
        }
    }

    /// The bytes this store is holding, as it counts them. Test-only: the
    /// budget is the thing H9 is about, so a test that cannot read it can only
    /// assert evictions.
    #[cfg(test)]
    fn retained_bytes(&self) -> usize {
        self.total_bytes
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

    /// H9: the count cap alone is not a memory bound. Near-limit replies stay
    /// under `IDEMPOTENCY_MAX_RESPONSE_BYTES` one at a time and would still add
    /// up to the count cap times 256 KiB, so the store evicts by total bytes
    /// too — oldest first, and it never holds more than the budget.
    #[test]
    fn the_store_evicts_by_total_bytes_not_only_by_count() {
        let mut store = IdempotencyStore::default();
        let now = Instant::now();
        // Just under the *per-entry* bound, so every reply is legal on its own
        // and only the byte budget can evict: sixteen of these fit in
        // `IDEMPOTENCY_MAX_TOTAL_BYTES`, the seventeenth cannot.
        let payload = IDEMPOTENCY_MAX_RESPONSE_BYTES - 1024;
        let reply =
            || DaemonMessage::Error(WireError::new(ErrorCode::Internal, "x".repeat(payload)));
        assert!(
            serialized_len(&reply()) < IDEMPOTENCY_MAX_RESPONSE_BYTES,
            "each entry is inside the per-entry bound; the budget is what bounds the store"
        );
        let count_cap = store.cap;
        assert!(
            count_cap > 16,
            "the count cap must not be what evicts here ({count_cap})"
        );
        let fits = IDEMPOTENCY_MAX_TOTAL_BYTES / serialized_len(&reply());
        assert!(fits >= 16, "the budget fits {fits} replies");

        for index in 0..=fits {
            store.remember(
                "app-1".into(),
                format!("k{index}"),
                format!("payload-{index}"),
                reply(),
                now,
            );
            assert!(
                store.retained_bytes() <= IDEMPOTENCY_MAX_TOTAL_BYTES,
                "after {index} replies the store holds {} bytes",
                store.retained_bytes()
            );
        }
        assert_eq!(
            store.entries.len(),
            fits,
            "one reply past the budget evicted exactly the oldest"
        );
        // The oldest is gone and the newest still replays.
        assert_eq!(
            store.check("app-1", "k0", "payload-0", now),
            IdempotencyOutcome::Miss
        );
        assert!(matches!(
            store.check(
                "app-1",
                &format!("k{fits}"),
                &format!("payload-{fits}"),
                now
            ),
            IdempotencyOutcome::Hit(_)
        ));
        // Re-remembering the same key replaces rather than adds: the count
        // stays put and the budget never creeps under a retry loop.
        let before = store.retained_bytes();
        store.remember(
            "app-1".into(),
            format!("k{fits}"),
            format!("payload-{fits}"),
            reply(),
            now,
        );
        assert_eq!(
            store.retained_bytes(),
            before,
            "a replayed key is not a second entry"
        );
    }
}

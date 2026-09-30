//! The hook `seq` gate for agent announcements — one monotonic counter per
//! `(source, agent_session_id)`, so two identities of one source never gate
//! each other, and a sender that announces no identity keeps the one
//! counter per source it has always had. A `startup`/`resume`/`new`/`fork`/
//! `branch` report whose seq does not beat the stored one is a new process
//! counting from fresh and starts its own key over; any other
//! `session_start_source`, and any higher seq, leave the counter alone.
//! Known residuals: two live processes announcing the SAME identity share
//! one key — the one the user just started resets it and takes it, then the
//! incumbent's next higher seq reclaims it and the just-started process can
//! be the one whose seq gets rejected (a fork gives a new id; a resume of a
//! conversation that is still live does not). And a dead life's queued
//! `startup` re-opens the key it shared with the live one, so its stale
//! reports are accepted and take the headline until the live agent's next
//! report — wrong state while the live agent stays silent, never a pin. The
//! headline is the most recently accepted report of a source, whichever
//! identity sent it.
//!
//! The monotonic rule is adapted from herdr `terminal/state.rs`
//! `accept_hook_report` (Apache-2.0, commit 3150bd9, `:1652`). The
//! lifecycle handling is Devboule's own, not a translation: herdr removes
//! a source's sequence on a witnessed exit and re-anchors under process
//! confirmation (`:697-699`); this daemon observes no process, so the
//! start-source transition above states the new life instead. No
//! reporter is silenced for good: a stale seq is dropped with a
//! rate-bounded log line, and an evicted identity counts from fresh if it
//! reports again.

use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::Mutex;
use std::time::{Duration, Instant};

use devboule_protocol::{AgentActivityState, ErrorCode, WireError};

use super::{AcceptedAgentReport, AgentReport, MAX_HOOK_SOURCES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceSeq {
    Unsequenced,
    Number(u64),
}

/// One `(source, agent_session_id)` counter: the seq it last accepted, the
/// order of that accept (what the per-source eviction reads), and when its
/// reject line was last logged.
#[derive(Debug)]
struct Gate {
    seq: SourceSeq,
    accepted_at: u64,
    last_reject_log: Option<Instant>,
}

/// Last accepted report per hook key, plus the current visible state: the
/// per-`(source, identity)` counters and the headline — the most recently
/// accepted report of any identity.
#[derive(Debug, Default)]
pub struct AgentReportState {
    sequences: HashMap<(String, Option<String>), Gate>,
    last: Option<AcceptedAgentReport>,
    /// Accept-order stamps for eviction; each accepted report takes the
    /// next one.
    clock: u64,
}

impl AgentReportState {
    #[cfg(test)]
    pub fn last(&self) -> Option<&AcceptedAgentReport> {
        self.last.as_ref()
    }

    /// Headline hook facts without the transcript: state plus the hook's own
    /// seq. The message stays in the journaled event; the activity answer
    /// carries metadata only.
    pub fn last_state(&self) -> Option<(AgentActivityState, Option<u64>)> {
        self.last.as_ref().map(|last| (last.state, last.seq))
    }

    /// Apply `report` if its `seq` is fresh for its `(source,
    /// agent_session_id)` key. Returns whether the accepted state changed.
    pub fn apply(&mut self, report: AgentReport) -> Result<bool, WireError> {
        if report.seq == Some(u64::MAX) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Agent announcement seq u64::MAX is not a valid hook sequence.",
            ));
        }
        let key = (report.source.clone(), report.agent_session_id.clone());
        let mut last_seq = self.sequences.get(&key).map(|gate| gate.seq);
        // Read before any removal: the throttle must outlive a reset, so
        // the bound holds however the key is re-opened.
        let carried_reject_log = self
            .sequences
            .get(&key)
            .and_then(|gate| gate.last_reject_log);
        if matches!(
            report.session_start_source.as_deref(),
            Some("startup" | "resume" | "new" | "fork" | "branch")
        ) && !accept_hook_seq(last_seq, report.seq)
        {
            // A new-process start source (startup, resume, new, fork,
            // branch) counts from fresh: when its seq does not beat the
            // stored one, the previous life's numbers must not gate it, so
            // its key starts over. A higher seq is the same life continuing
            // and needs no reset, and every other start source (clear,
            // compact, select) is same-process — presence alone resets
            // nothing.
            self.sequences.remove(&key);
            last_seq = None;
        }
        if !accept_hook_seq(last_seq, report.seq) {
            let now = Instant::now();
            if let Some(gate) = self.sequences.get_mut(&key) {
                if reject_log_due(gate.last_reject_log, now) {
                    gate.last_reject_log = Some(now);
                    // Under the app the daemon's stderr is daemon.log; a
                    // launcher that gave stderr a real sink receives this
                    // line there instead (daemon_log.rs).
                    eprintln!(
                        "hook report seq rejected: source={} identity={:?} last={:?} incoming={:?}",
                        report.source, report.agent_session_id, last_seq, report.seq
                    );
                }
            }
            return Ok(false);
        }
        let source_tracked = self
            .sequences
            .keys()
            .any(|(tracked, _)| tracked == &report.source);
        if !source_tracked {
            let distinct = self
                .sequences
                .keys()
                .map(|(tracked, _)| tracked.as_str())
                .collect::<HashSet<_>>()
                .len();
            if distinct >= MAX_HOOK_SOURCES {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Agent announcement may track at most {MAX_HOOK_SOURCES} sources"),
                ));
            }
        }
        if !self.sequences.contains_key(&key) {
            self.evict_if_source_full(&report.source);
        }
        let mark = match report.seq {
            Some(seq) => SourceSeq::Number(seq),
            None => SourceSeq::Unsequenced,
        };
        self.clock = self.clock.saturating_add(1);
        // The reject-log throttle rides the key across accepts and across
        // resets: one line per key per minute, however the key was
        // re-opened.
        self.sequences.insert(
            key,
            Gate {
                seq: mark,
                accepted_at: self.clock,
                last_reject_log: carried_reject_log,
            },
        );
        // The headline is the most recently accepted report of this source,
        // whichever identity sent it: a late report from an older life can
        // show stale state only until the live agent's next report, so the
        // row heals itself and can never be pinned — and the event stream,
        // the journal row and this headline move together on every accept.
        self.last = Some(AcceptedAgentReport::from(&report));
        Ok(true)
    }

    /// Room for one more identity of `source`: past the bound its least
    /// recently accepted key makes room. The bound exists so a long-lived
    /// session's many identities cannot grow the map without end;
    /// eviction forgets a key, it never rejects a reporter — an evicted
    /// identity that reports again counts from fresh, at the cost of one
    /// open gate: its first report is believed whatever its seq, so a
    /// lower seq than it last accepted can be taken once.
    fn evict_if_source_full(&mut self, source: &str) {
        let lives = self
            .sequences
            .keys()
            .filter(|(tracked, _)| tracked.as_str() == source)
            .count();
        if lives < MAX_LIVES_PER_SOURCE {
            return;
        }
        let oldest = self
            .sequences
            .iter()
            .filter(|((tracked, _), _)| tracked.as_str() == source)
            .min_by_key(|(_, gate)| gate.accepted_at)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            self.sequences.remove(&oldest);
        }
    }
}

/// How long one key's reject line stays quiet after logging: the first
/// reject for a key, then at most one line a minute. Rejections can repeat
/// forever (two lives sharing one id), and `daemon.log` stops at its 5 MiB
/// cap with one notice — an unbounded flood would silence every other
/// daemon diagnostic along with itself.
const REJECT_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// Whether a reject on this key deserves its log line now: the first one,
/// then at most one per [`REJECT_LOG_INTERVAL`].
fn reject_log_due(last_log: Option<Instant>, now: Instant) -> bool {
    last_log.is_none_or(|at| now.saturating_duration_since(at) >= REJECT_LOG_INTERVAL)
}

/// Shared wrapper so concurrent applies serialize on one lock.
#[cfg(test)]
#[derive(Debug, Default)]
struct SharedAgentReportState {
    inner: Mutex<AgentReportState>,
}

#[cfg(test)]
impl SharedAgentReportState {
    fn apply(&self, report: AgentReport) -> Result<bool, WireError> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.apply(report)
    }

    fn last(&self) -> Option<AcceptedAgentReport> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .last()
            .cloned()
    }
}

/// Whether an incoming hook `seq` may replace the last accepted one for a
/// `(source, agent_session_id)` key.
///
/// A missing `seq` is accepted only before this key has any accepted
/// report. After that, only a strictly greater `seq` may apply. Duplicates
/// and older values are ignored so they cannot regress state.
fn accept_hook_seq(last: Option<SourceSeq>, incoming: Option<u64>) -> bool {
    match (last, incoming) {
        (None, None) | (None, Some(_)) => true,
        (Some(SourceSeq::Unsequenced), None) => false,
        (Some(SourceSeq::Unsequenced), Some(_)) => true,
        (Some(SourceSeq::Number(_)), None) => false,
        (Some(SourceSeq::Number(last)), Some(incoming)) => incoming > last,
    }
}

/// Identities tracked per source. One source can announce through several
/// agent lives over a long-lived session, and each life is a key; four
/// bounds the history — the most recently accepting keys stay, an active
/// life that has gone quiet is the first to go, and the least recently
/// accepted key makes room.
const MAX_LIVES_PER_SOURCE: usize = 4;

#[cfg(test)]
#[path = "agent_report_state_tests.rs"]
mod tests;

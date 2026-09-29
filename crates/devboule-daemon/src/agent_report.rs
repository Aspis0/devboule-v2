//! Agent announcement: peer identity, field validation, and the hook `seq`
//! gate — one monotonic counter per `(source, agent_session_id)`, so two
//! identities of one source never gate each other, and a sender that
//! announces no identity keeps the one counter per source it has always
//! had. A `startup`/`resume`/`new`/`fork`/`branch` report whose seq does
//! not beat the stored one is a new process counting from fresh and
//! starts its own key over; any other `session_start_source`, and any
//! higher seq, leave the counter alone. Known residuals: two live
//! processes announcing the SAME identity share one key — the one the
//! user just started resets it and takes it, then the incumbent's next
//! higher seq reclaims it and the just-started process can be the one
//! whose seq gets rejected (a fork gives a new id; a resume of a
//! conversation that is still live does not). And a dead life's queued
//! `startup` re-opens the key it shared with the live one, so its stale
//! reports are accepted and take the headline until the live agent's
//! next report — wrong state while the live agent stays silent, never a
//! pin. The headline is the most recently accepted report of a source,
//! whichever identity sent it.
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

/// OS-derived identity of a named-pipe peer. Never taken from a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    pub user: String,
    pub pid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentReport {
    pub source: String,
    pub agent: String,
    pub state: AgentActivityState,
    pub message: Option<String>,
    pub seq: Option<u64>,
    pub agent_session_id: Option<String>,
    pub agent_session_path: Option<String>,
    pub session_start_source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedAgentReport {
    pub source: String,
    pub agent: String,
    pub state: AgentActivityState,
    pub message: Option<String>,
    pub seq: Option<u64>,
    pub agent_session_id: Option<String>,
    pub agent_session_path: Option<String>,
    pub session_start_source: Option<String>,
}

impl From<&AgentReport> for AcceptedAgentReport {
    fn from(report: &AgentReport) -> Self {
        Self {
            source: report.source.clone(),
            agent: report.agent.clone(),
            state: report.state,
            message: report.message.clone(),
            seq: report.seq,
            agent_session_id: report.agent_session_id.clone(),
            agent_session_path: report.agent_session_path.clone(),
            session_start_source: report.session_start_source.clone(),
        }
    }
}

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

/// Verify the named-pipe peer before trusting a frame that names a session.
///
/// A missing identity is a failed check, not an open door. The SID must
/// come from the OS (`GetNamedPipeClientProcessId` + `process_user_sid`),
/// never from the frame.
pub fn verify_announcement_peer(
    peer: Option<&PeerIdentity>,
    session_user: &str,
) -> Result<(), WireError> {
    let Some(peer) = peer else {
        return Err(unauthorized_peer(
            "Could not verify the announcing process identity.",
        ));
    };
    if peer.user != session_user {
        return Err(unauthorized_peer(
            "The announcing process is not the session owner.",
        ));
    }
    Ok(())
}

pub fn unauthorized_peer(message: impl Into<String>) -> WireError {
    WireError::new(ErrorCode::Unauthorized, message)
}

/// Per-field cap on announcement strings. 4 KiB is far above any legitimate
/// agent id, source, or session path and far below the 1 MiB frame cap.
pub const MAX_ANNOUNCEMENT_FIELD_BYTES: usize = 4096;
/// Distinct hook `source` values tracked per session. Official sources are
/// `devboule:<id>` for each known agent plus the test stub, so this cap is
/// the actual number of sources `validate_announcement` can admit.
pub const MAX_HOOK_SOURCES: usize = crate::provider_catalog::KNOWN_AGENTS.len() + 1;
/// Identities tracked per source. One source can announce through several
/// agent lives over a long-lived session, and each life is a key; four
/// bounds the history — the most recently accepting keys stay, an active
/// life that has gone quiet is the first to go, and the least recently
/// accepted key makes room.
const MAX_LIVES_PER_SOURCE: usize = 4;

/// Official sources have the form `devboule:<agent>` and must name the
/// same agent they claim. Adapted from herdr `is_official_agent_source`
/// (Apache-2.0, commit 3150bd9).
pub fn is_official_agent_source(source: &str, agent: &str) -> bool {
    if !is_known_announcing_agent(agent) {
        return false;
    }
    source == format!("devboule:{agent}")
}

fn is_known_announcing_agent(agent: &str) -> bool {
    agent == "stub"
        || crate::provider_catalog::KNOWN_AGENTS
            .iter()
            .any(|known| known.id == agent)
}

pub fn validate_announcement(report: &AgentReport) -> Result<(), WireError> {
    check_field("source", &report.source)?;
    check_field("agent", &report.agent)?;
    if let Some(value) = &report.message {
        check_field("message", value)?;
    }
    if let Some(value) = &report.agent_session_id {
        check_field("agentSessionId", value)?;
    }
    if let Some(value) = &report.agent_session_path {
        check_field("agentSessionPath", value)?;
    }
    if let Some(value) = &report.session_start_source {
        check_field("sessionStartSource", value)?;
    }
    if report.source.trim().is_empty() || report.agent.trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "Agent announcement requires source and agent.",
        ));
    }
    if !is_official_agent_source(&report.source, &report.agent) {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "Agent announcement source '{}' is not an official Devboule source for agent '{}'.",
                report.source, report.agent
            ),
        ));
    }
    Ok(())
}

fn check_field(name: &str, value: &str) -> Result<(), WireError> {
    if value.len() > MAX_ANNOUNCEMENT_FIELD_BYTES {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "Agent announcement field '{name}' exceeds {MAX_ANNOUNCEMENT_FIELD_BYTES} bytes"
            ),
        ));
    }
    Ok(())
}

/// Used when this platform cannot identify a named-pipe peer. Must not be
/// reported as authorization failure: we did not decide the peer is the
/// wrong user, we could not tell who they are.
#[cfg_attr(windows, allow(dead_code))]
pub fn peer_identity_unavailable_on_platform() -> WireError {
    WireError::new(
        ErrorCode::Unimplemented,
        "Peer identity is not available on this platform.",
    )
}

/// herdr `normalize_session_start_source`: unknown values become `None`,
/// they do not reject the rest of the report.
pub fn normalize_session_start_source(value: Option<String>) -> Option<String> {
    match value.as_deref().map(str::trim) {
        Some(
            source @ ("startup" | "resume" | "clear" | "compact" | "branch" | "new" | "fork"
            | "select"),
        ) => Some(source.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    fn report(seq: Option<u64>, state: AgentActivityState) -> AgentReport {
        AgentReport {
            source: "devboule:stub".to_string(),
            agent: "stub".to_string(),
            state,
            message: None,
            seq,
            agent_session_id: Some("agent-1".to_string()),
            agent_session_path: None,
            session_start_source: None,
        }
    }

    #[test]
    fn out_of_order_seq_must_not_regress_state() {
        let mut state = AgentReportState::default();
        assert!(state
            .apply(report(Some(5), AgentActivityState::Working))
            .expect("apply"));
        assert_eq!(
            state.last().map(|last| last.state),
            Some(AgentActivityState::Working)
        );
        assert!(!state
            .apply(report(Some(3), AgentActivityState::Idle))
            .expect("stale"));
        assert_eq!(
            state.last().map(|last| last.state),
            Some(AgentActivityState::Working),
            "seq 3 arrived after seq 5 and must not replace Working with Idle"
        );
        assert_eq!(state.last().and_then(|last| last.seq), Some(5));
    }

    #[test]
    fn duplicate_seq_must_not_replace_state() {
        let mut state = AgentReportState::default();
        assert!(state
            .apply(report(Some(2), AgentActivityState::Working))
            .expect("apply"));
        assert!(!state
            .apply(report(Some(2), AgentActivityState::Blocked))
            .expect("duplicate"));
        assert_eq!(
            state.last().map(|last| last.state),
            Some(AgentActivityState::Working)
        );
    }

    #[test]
    fn later_seq_is_accepted() {
        let mut state = AgentReportState::default();
        assert!(state
            .apply(report(Some(1), AgentActivityState::Idle))
            .expect("apply"));
        assert!(state
            .apply(report(Some(2), AgentActivityState::Working))
            .expect("apply"));
        assert_eq!(
            state.last().map(|last| last.state),
            Some(AgentActivityState::Working)
        );
        assert_eq!(state.last().and_then(|last| last.seq), Some(2));
    }

    #[test]
    fn a_missing_seq_does_not_block_a_later_zero() {
        let mut state = AgentReportState::default();
        assert!(state
            .apply(report(None, AgentActivityState::Idle))
            .expect("unsequenced"));
        assert!(
            state
                .apply(report(Some(0), AgentActivityState::Working))
                .expect("zero after none"),
            "seq 0 must be distinct from a missing seq"
        );
        assert_eq!(
            state.last().map(|last| last.state),
            Some(AgentActivityState::Working)
        );
        assert_eq!(state.last().and_then(|last| last.seq), Some(0));
    }

    fn claude_report(
        identity: Option<&str>,
        seq: Option<u64>,
        state: AgentActivityState,
    ) -> AgentReport {
        let mut item = report(seq, state);
        item.source = "devboule:claude".to_string();
        item.agent = "claude".to_string();
        item.agent_session_id = identity.map(str::to_string);
        item
    }

    #[test]
    fn the_least_recently_accepted_identity_is_evicted_and_returns_fresh() {
        let mut state = AgentReportState::default();
        for identity in ["life-1", "life-2", "life-3", "life-4"] {
            assert!(state
                .apply(claude_report(
                    Some(identity),
                    Some(5),
                    AgentActivityState::Working
                ))
                .expect("apply"));
        }
        assert!(
            state
                .apply(claude_report(
                    Some("life-5"),
                    Some(5),
                    AgentActivityState::Working
                ))
                .expect("fifth identity"),
            "a fifth identity fits by evicting the least recently accepted one"
        );
        assert!(
            !state
                .apply(claude_report(
                    Some("life-2"),
                    Some(1),
                    AgentActivityState::Idle
                ))
                .expect("stale seq applied as false"),
            "the identities still tracked keep their counters, and a rejected \
             report does not refresh its recency"
        );
        assert!(
            state
                .apply(claude_report(
                    Some("life-1"),
                    Some(1),
                    AgentActivityState::Idle
                ))
                .expect("evicted identity applies"),
            "an evicted identity counts from fresh — eviction forgets, it never silences"
        );
    }

    #[test]
    fn an_identityless_sender_keeps_one_counter() {
        let mut state = AgentReportState::default();
        assert!(state
            .apply(claude_report(None, Some(5), AgentActivityState::Working))
            .expect("apply"));
        assert!(
            !state
                .apply(claude_report(None, Some(3), AgentActivityState::Idle))
                .expect("stale seq applied as false"),
            "a sender that announces no identity keeps the one counter per source it always had"
        );
        assert_eq!(state.last().and_then(|last| last.seq), Some(5));
    }

    #[test]
    fn the_reject_log_line_is_rate_bound_per_key() {
        let now = Instant::now();
        assert!(
            reject_log_due(None, now),
            "the first reject for a key always earns its line"
        );
        assert!(
            !reject_log_due(Some(now), now),
            "an immediate repeat stays quiet"
        );
        assert!(!reject_log_due(
            Some(now),
            now + REJECT_LOG_INTERVAL - Duration::from_secs(1)
        ));
        assert!(reject_log_due(Some(now), now + REJECT_LOG_INTERVAL));
    }

    #[test]
    fn the_reject_log_throttle_survives_a_reset() {
        let mut state = AgentReportState::default();
        let key = ("devboule:claude".to_string(), Some("life-a".to_string()));
        assert!(state
            .apply(claude_report(
                Some("life-a"),
                Some(5),
                AgentActivityState::Working
            ))
            .expect("apply"));
        assert!(
            !state
                .apply(claude_report(
                    Some("life-a"),
                    Some(3),
                    AgentActivityState::Idle
                ))
                .expect("stale seq applied as false"),
            "the first reject earns its line"
        );
        let armed = state
            .sequences
            .get(&key)
            .and_then(|gate| gate.last_reject_log)
            .expect("the first reject was logged");
        let mut reset = claude_report(Some("life-a"), Some(1), AgentActivityState::Idle);
        reset.session_start_source = Some("startup".to_string());
        assert!(state.apply(reset).expect("the reset accepts"));
        assert_eq!(
            state
                .sequences
                .get(&key)
                .and_then(|gate| gate.last_reject_log),
            Some(armed),
            "the throttle rides the reset: the bound is per key per minute across re-opens"
        );
        assert!(
            !state
                .apply(claude_report(
                    Some("life-a"),
                    Some(1),
                    AgentActivityState::Blocked
                ))
                .expect("stale seq applied as false"),
            "an immediate reject after the reset stays within the interval"
        );
        assert_eq!(
            state
                .sequences
                .get(&key)
                .and_then(|gate| gate.last_reject_log),
            Some(armed),
            "the reset/reject alternation does not log again inside the interval"
        );
    }

    #[test]
    fn u64_max_seq_is_rejected() {
        let mut state = AgentReportState::default();
        let error = state
            .apply(report(Some(u64::MAX), AgentActivityState::Working))
            .expect_err("MAX is not a usable hook seq");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(state.last().is_none());
    }

    #[test]
    fn unverifiable_peer_is_rejected_and_does_not_apply() {
        let error = verify_announcement_peer(None, "S-1-5-21-1-2-3-1001")
            .expect_err("a missing peer identity is not a verified peer");
        assert_eq!(error.code, ErrorCode::Unauthorized);
    }

    #[test]
    fn peer_sid_mismatch_is_rejected() {
        let peer = PeerIdentity {
            user: "S-1-5-21-1-2-3-1002".to_string(),
            pid: 4242,
        };
        let error = verify_announcement_peer(Some(&peer), "S-1-5-21-1-2-3-1001")
            .expect_err("a different user SID is not the session owner");
        assert_eq!(error.code, ErrorCode::Unauthorized);
    }

    #[test]
    fn matching_peer_sid_is_accepted() {
        let peer = PeerIdentity {
            user: "S-1-5-21-1-2-3-1001".to_string(),
            pid: 7,
        };
        verify_announcement_peer(Some(&peer), "S-1-5-21-1-2-3-1001")
            .expect("same-user peer is the authorized announcer");
    }

    #[test]
    fn concurrent_duplicate_seq_applies_exactly_once() {
        let state = Arc::new(SharedAgentReportState::default());
        let barrier = Arc::new(Barrier::new(2));
        let mut joins = Vec::new();
        for state_value in [AgentActivityState::Working, AgentActivityState::Blocked] {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            joins.push(thread::spawn(move || {
                barrier.wait();
                state.apply(report(Some(1), state_value))
            }));
        }
        let applied: Vec<bool> = joins
            .into_iter()
            .map(|join| join.join().expect("worker").expect("apply"))
            .collect();
        let wins = applied.iter().filter(|applied| **applied).count();
        assert_eq!(
            wins, 1,
            "duplicate seq=1 must apply exactly once, got {applied:?}"
        );
        let last = state.last().expect("one report");
        assert_eq!(last.seq, Some(1));
        assert!(
            last.state == AgentActivityState::Working || last.state == AgentActivityState::Blocked,
            "winner must be one of the two whole reports, got {:?}",
            last.state
        );
    }

    #[test]
    fn malformed_source_is_not_official() {
        assert!(
            !is_official_agent_source("freeform", "stub"),
            "a free-text source must be rejected"
        );
        assert!(
            !is_official_agent_source("devboule:claude", "codex"),
            "source must name the same agent it claims"
        );
        assert!(
            !is_official_agent_source("herdr:stub", "stub"),
            "a herdr source is not a Devboule source"
        );
    }

    #[test]
    fn validate_rejects_malformed_source() {
        let mut item = report(Some(1), AgentActivityState::Working);
        item.source = "freeform".to_string();
        let error = validate_announcement(&item).expect_err("malformed source");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn official_devboule_source_matches_the_agent() {
        assert!(is_official_agent_source("devboule:stub", "stub"));
        assert!(is_official_agent_source("devboule:claude", "claude"));
    }

    #[test]
    fn too_many_distinct_sources_are_rejected() {
        let mut state = AgentReportState::default();
        for index in 0..MAX_HOOK_SOURCES {
            let mut item = report(Some(1), AgentActivityState::Working);
            item.source = format!("src-{index}");
            assert!(
                state.apply(item).expect("within cap"),
                "source {index} should fit"
            );
        }
        let mut extra = report(Some(1), AgentActivityState::Idle);
        extra.source = "src-overflow".to_string();
        let error = state
            .apply(extra)
            .expect_err("a 33rd distinct source must not grow the map");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains(&MAX_HOOK_SOURCES.to_string()),
            "error must name the limit, got {}",
            error.message
        );
    }

    #[test]
    fn overlong_field_is_rejected_by_name() {
        let mut item = report(Some(1), AgentActivityState::Working);
        item.agent = "x".repeat(MAX_ANNOUNCEMENT_FIELD_BYTES + 1);
        let error = validate_announcement(&item).expect_err("overlong agent");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("agent"),
            "error must name the field, got {}",
            error.message
        );
        assert!(
            error
                .message
                .contains(&MAX_ANNOUNCEMENT_FIELD_BYTES.to_string()),
            "error must name the limit, got {}",
            error.message
        );
    }

    #[test]
    fn platform_without_peer_identity_is_not_unauthorized() {
        let error = peer_identity_unavailable_on_platform();
        assert_ne!(
            error.code,
            ErrorCode::Unauthorized,
            "missing platform support is not an authorization decision"
        );
        assert!(
            error.message.to_ascii_lowercase().contains("platform"),
            "error must name the platform cause, got {}",
            error.message
        );
    }
}

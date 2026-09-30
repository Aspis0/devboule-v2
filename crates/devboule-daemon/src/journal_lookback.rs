//! What a replay walk stopped mid-page to ask the journal, and the one
//! command that answers it: one ask site in the walk, one release/scan/
//! relock round in `event_pull`, per-lookback scan functions behind one
//! dispatch. A request carries only what its scan needs.

use std::collections::HashSet;

use rusqlite::Connection;

use crate::claude_view::CostBaseline;
use crate::journal::JournalError;

/// The journal read a stopped walk waits for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LookbackRequest {
    /// Approved plan turns, session-wide: turn ids are unique for the
    /// thread's life, so the scan takes no bound.
    CodexPlanMarks,
    /// The newest running total a Claude `result` carried at or before this
    /// cursor — the baseline a mid-generation pull seeds its latch from.
    ClaudeCostBaseline { generation: u64, before_seq: u64 },
}

/// What a lookback scan found. A failed read never becomes an answer here —
/// the runtime maps the failure per lookback (marks to an empty set, the
/// baseline to [`CostBaseline::Unknown`]).
#[derive(Debug, PartialEq)]
pub(crate) enum LookbackAnswer {
    PlanMarks(HashSet<String>),
    CostBaseline(CostBaseline),
}

impl LookbackRequest {
    /// The one door every lookback asks through; each variant owns its
    /// scan, with its own row kind and bound.
    pub(crate) fn scan_conn(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<LookbackAnswer, JournalError> {
        match self {
            Self::CodexPlanMarks => {
                crate::codex_plan_marks::scan_conn(conn, session_id).map(LookbackAnswer::PlanMarks)
            }
            Self::ClaudeCostBaseline {
                generation,
                before_seq,
            } => crate::claude_cost_baseline::scan_conn(conn, session_id, *generation, *before_seq),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use crate::claude_view::CostBaseline;
    use crate::journal::{acp_envelope_record, new_session_record, Journal};
    use crate::journal_lookback::{LookbackAnswer, LookbackRequest};
    use crate::session::SessionRuntime;

    #[test]
    fn a_total_the_wire_cannot_carry_is_not_a_baseline() {
        // A negative total in a journalled row is not a starting point a
        // delta may be measured from — it would inflate the next turn. The
        // scan skips it for the newest honest row.
        let dir = crate::test_dirs::test_temp_dir("devboule-cost-baseline-negative");
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        let session_id = "s.cost.baseline.negative";
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                devboule_protocol::SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let honest = json!({
            "type": "result", "subtype": "success", "stop_reason": "end_turn",
            "total_cost_usd": 0.0723335, "is_error": false,
            "usage": {"input_tokens": 2, "output_tokens": 4}
        });
        let negative = json!({
            "type": "result", "subtype": "success", "stop_reason": "end_turn",
            "total_cost_usd": -0.5, "is_error": false,
            "usage": {"input_tokens": 2, "output_tokens": 4}
        });
        journal
            .append_blocking(acp_envelope_record(session_id, 1, 1, &honest).unwrap())
            .unwrap();
        journal
            .append_blocking(acp_envelope_record(session_id, 1, 2, &negative).unwrap())
            .unwrap();
        let baseline = journal.lookback(
            session_id,
            LookbackRequest::ClaudeCostBaseline {
                generation: 1,
                before_seq: 2,
            },
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            baseline.unwrap(),
            LookbackAnswer::CostBaseline(CostBaseline::Known(Some(0.0723335)))
        );
    }

    #[test]
    fn a_failed_lookback_scan_is_unknown_not_absent() {
        // A journal read that fails (the rpc deadline among them) must not
        // answer "no baseline": the two are different facts, and only the
        // first lets the next turn cost a verified delta.
        let dir = crate::test_dirs::test_temp_dir("devboule-lookback-unknown");
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        journal
            .upsert_blocking(new_session_record(
                "s.lookback.dead",
                "S-1-5-21-1",
                None,
                devboule_protocol::SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let runtime = Arc::new(SessionRuntime::with_journal(
            "s.lookback.dead".to_string(),
            Some(Arc::clone(&journal)),
        ));
        journal.shutdown();
        let answer = runtime.journal_lookback(LookbackRequest::ClaudeCostBaseline {
            generation: 1,
            before_seq: 1,
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(answer, LookbackAnswer::CostBaseline(CostBaseline::Unknown));
    }
}

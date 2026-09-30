//! The cost baseline a mid-generation replay seeds its view from.
//!
//! The per-turn cost is a delta against the CLI's running `total_cost_usd`,
//! and a pull that attaches at a cursor inside the current generation never
//! saw the results that moved it. The journal keeps every raw result frame,
//! so the newest total at or before the cursor is one ordered read away —
//! the Codex plan marks are the sibling lookback.

use rusqlite::Connection;

use crate::claude_view::{total_cost_from_result, CostBaseline};
use crate::journal::JournalError;
use crate::journal_lookback::LookbackAnswer;

/// The newest `total_cost_usd` any `result` envelope of `session_id`
/// carried in `generation` at or before `before_seq`, or
/// [`CostBaseline::Known`](`CostBaseline::Known`) with no total when no such
/// result named one. `before_seq` is a replay cursor — the client has seen
/// that row, and `replay_agent_page` reads strictly after it — so the
/// boundary is inclusive. A total the wire cannot carry (negative,
/// non-finite) is not a baseline and is skipped for an older honest row.
/// The byte gate fails open, like the plan-mark gate: journal payloads are
/// always `serde_json`-written UTF-8, so it is a performance filter only.
pub(crate) fn scan_conn(
    conn: &Connection,
    session_id: &str,
    generation: u64,
    before_seq: u64,
) -> Result<LookbackAnswer, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM events
         WHERE session_id = ?1 AND kind = 'acp_envelope' AND generation = ?2 AND seq <= ?3
         ORDER BY seq DESC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![session_id, generation as i64, before_seq as i64],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    for row in rows {
        let payload = row?;
        let may_carry = std::str::from_utf8(&payload)
            .map(|text| text.contains("total_cost_usd"))
            .unwrap_or(true);
        if !may_carry {
            continue;
        }
        if let Ok(envelope) = serde_json::from_slice::<serde_json::Value>(&payload) {
            if let Some(total) =
                total_cost_from_result(&envelope).and_then(crate::usage_cost::finite_cost)
            {
                return Ok(LookbackAnswer::CostBaseline(CostBaseline::Known(Some(
                    total,
                ))));
            }
        }
    }
    Ok(LookbackAnswer::CostBaseline(CostBaseline::Known(None)))
}

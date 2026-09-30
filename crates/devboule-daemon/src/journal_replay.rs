use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};

use devboule_protocol::{SessionEvent, SessionKind};

use super::{
    crc32, decode_chunks, origin_from_columns, parse_kind, EventKind, EventRecord, JournalError,
    PersistStatus, Replay, SessionRecord,
};

#[derive(Debug)]
pub(crate) struct AgentReplayPage {
    /// The session's generation in the `sessions` row at page time — a
    /// freshness probe against the attach generation, never the generation
    /// of the rows inside `records`: a page legitimately spans every
    /// generation up to the one the attachment attached to.
    pub(crate) generation: u64,
    pub(crate) last_seq: u64,
    pub(crate) records: Vec<EventRecord>,
}

/// Read one bounded page of structured agent records. The live attach path
/// deliberately pages raw journal rows instead of calling `replay_session`:
/// rebuilding a long conversation into one Vec would merely move the memory
/// spike from `stream.pending` to the writer thread. View derivation happens
/// incrementally in `event_pull`, under the same pull budget as live events.
///
/// The range is lexicographic in `(generation, seq)` — the order the
/// `events_session` index serves — from just after `(from_generation,
/// from_seq)` through `(expected_generation, through_seq)`. A resume keeps
/// the session id and every earlier generation's events, so serving the
/// generations below the attach generation is how a Reopen shows the whole
/// conversation; rows past the attach generation belong to a stream this
/// attachment cannot see and are never served.
pub(super) fn replay_agent_page(
    conn: &Connection,
    session_id: &str,
    expected_generation: u64,
    from_generation: u64,
    from_seq: u64,
    through_seq: u64,
    limit: usize,
) -> Result<AgentReplayPage, JournalError> {
    let (generation, last_seq, closed): (u64, u64, bool) = conn
        .query_row(
            "SELECT generation, last_seq, closed FROM sessions WHERE id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? != 0,
                ))
            },
        )
        .optional()?
        .ok_or(JournalError::SessionNotFound)?;
    if closed {
        return Err(JournalError::SessionNotFound);
    }
    if generation != expected_generation {
        return Ok(AgentReplayPage {
            generation,
            last_seq,
            records: Vec::new(),
        });
    }

    // The bounds are bound as i64. A u64::MAX from-seq — the nothing-owed
    // sentinel — must clamp to the top of the representable range, not wrap
    // negative and widen the window; that must hold here at the bind, not
    // depend on a caller's short-circuit upstream.
    let from_seq = i64::try_from(from_seq).unwrap_or(i64::MAX);
    let through_seq = i64::try_from(through_seq).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(
        "SELECT generation, seq, kind, ts_ms, payload, checksum FROM events
         WHERE session_id = ?1
           AND (generation, seq) > (?2, ?3)
           AND (generation, seq) <= (?4, ?5)
           AND kind IN ('agent_report', 'acp_envelope')
         ORDER BY generation, seq LIMIT ?6",
    )?;
    let rows = statement.query_map(
        params![
            session_id,
            from_generation as i64,
            from_seq,
            expected_generation as i64,
            through_seq,
            limit as i64
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, i64>(5)? as u32,
            ))
        },
    )?;
    let mut records = Vec::new();
    for row in rows {
        let (row_generation, seq, kind, ts_ms, payload, checksum) = row?;
        if crc32(&payload) != checksum {
            return Err(JournalError::Checksum {
                session_id: session_id.to_string(),
                seq,
            });
        }
        let kind = EventKind::parse(&kind).ok_or_else(|| {
            JournalError::Corrupt(format!(
                "unknown agent event kind at {session_id} seq {seq}"
            ))
        })?;
        records.push(EventRecord {
            session_id: session_id.to_string(),
            generation: row_generation,
            seq,
            kind,
            ts_ms,
            payload,
        });
    }
    Ok(AgentReplayPage {
        generation,
        last_seq,
        records,
    })
}

/// Every open session row: the roster's read, with `closed = 0` in SQL —
/// closed rows accumulate for the daemon's whole life and are none of this
/// path's business (history rides the transcript, not the live roster).
pub(super) fn list_sessions(conn: &Connection) -> Result<Vec<SessionRecord>, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped,
                peer_session_id, provider, origin_kind, origin_device, origin_role,
                display_name, created_by, profile_id, context_id, unattended, unattended_state, labels,
                overlay, depth, disowned_peer_session_id, cwd, goal
         FROM sessions WHERE closed = 0 ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_session)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(JournalError::from)
}

/// One session row, by id, and only if it belongs to `owner` and was created
/// by `created_by`: the status fallback's stored read (`devboule_get_agent_status`).
/// One row filtered in SQL — never the roster's shape, never a scan of other
/// users' history, and never a name two rows could share: a closed child is
/// addressed by id alone.
pub(super) fn owned_child_record(
    conn: &Connection,
    session_id: &str,
    owner: &str,
    created_by: &str,
) -> Result<Option<SessionRecord>, JournalError> {
    conn.query_row(
        "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped,
                peer_session_id, provider, origin_kind, origin_device, origin_role,
                display_name, created_by, profile_id, context_id, unattended, unattended_state, labels,
                overlay, depth, disowned_peer_session_id, cwd, goal
         FROM sessions WHERE id = ?1 AND owner = ?2 AND created_by = ?3",
        params![session_id, owner, created_by],
        row_to_session,
    )
    .optional()
    .map_err(JournalError::from)
}

/// One **closed** row a sender may have named among `created_by`'s children:
/// the exact id first, then a title that exactly one of those closed rows
/// carries — the two forms the live target lookup accepts
/// (`mcp_broker/tools/messaging.rs`).
///
/// The roster's [`list_sessions`] stops at `closed = 0`, so this is the one
/// read that sees a closed row: the send's miss needs it to answer "closed"
/// rather than "not found" (`session_idle_close.rs`). Scoped by owner and
/// creator the way [`owned_child_record`] is, and a title two closed children
/// share answers nothing — without a live name to tell them apart the
/// sentence could name the wrong child.
pub(super) fn closed_child_record(
    conn: &Connection,
    target: &str,
    owner: &str,
    created_by: &str,
) -> Result<Option<SessionRecord>, JournalError> {
    let by_id = conn
        .query_row(
            "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                    generation, status, exit_code, closed, last_seq, degraded,
                    dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped,
                    peer_session_id, provider, origin_kind, origin_device, origin_role,
                    display_name, created_by, profile_id, context_id, unattended, unattended_state, labels,
                    overlay, depth, disowned_peer_session_id, cwd, goal
             FROM sessions WHERE id = ?1 AND owner = ?2 AND created_by = ?3 AND closed = 1",
            params![target, owner, created_by],
            row_to_session,
        )
        .optional()
        .map_err(JournalError::from)?;
    if by_id.is_some() {
        return Ok(by_id);
    }
    let mut stmt = conn.prepare(
        "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped,
                peer_session_id, provider, origin_kind, origin_device, origin_role,
                display_name, created_by, profile_id, context_id, unattended, unattended_state, labels,
                overlay, depth, disowned_peer_session_id, cwd, goal
         FROM sessions WHERE owner = ?1 AND created_by = ?2 AND title = ?3 AND closed = 1
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![owner, created_by, target], row_to_session)?;
    let mut matches = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(JournalError::from)?;
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => Ok(None),
    }
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        owner: row.get(1)?,
        workspace_id: row.get(2)?,
        // journal_usage can aggregate an unknown future kind, but replay must
        // materialize a typed session and therefore fails closed here.
        kind: parse_kind(&row.get::<_, String>(3)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        title: row.get(4)?,
        created_at_ms: row.get::<_, i64>(5)? as u64,
        updated_at_ms: row.get::<_, i64>(6)? as u64,
        generation: row.get::<_, i64>(7)? as u64,
        status: PersistStatus::parse(&row.get::<_, String>(8)?),
        exit_code: row.get::<_, Option<i64>>(9)?.map(|code| code as u32),
        closed: row.get::<_, i64>(10)? != 0,
        last_seq: row.get::<_, i64>(11)? as u64,
        degraded: row.get::<_, i64>(12)? != 0,
        dropped_frames: row.get::<_, i64>(13)? as u64,
        dropped_bytes: row.get::<_, i64>(14)? as u64,
        trimmed_bytes: row.get::<_, i64>(15)? as u64,
        payload_bytes: row.get::<_, i64>(16)? as u64,
        reaped: row.get::<_, i64>(17)? != 0,
        peer_session_id: row.get(18)?,
        provider: row.get(19)?,
        origin: origin_from_columns(row.get(20)?, row.get(21)?, row.get(22)?),
        display_name: row.get(23)?,
        created_by: row.get(24)?,
        profile_id: row.get(25)?,
        context_id: row.get(26)?,
        unattended_state: super::unattended_state_from_rank(row.get::<_, i64>(28)?),
        labels: deserialize_labels(row.get(29)?),
        overlay: deserialize_overlay(row.get(30)?),
        // A hand-written out-of-range depth must read as the cap, never as
        // a truncation toward the permissive end: `u32::try_from` fails
        // closed in both directions, where `as` would wrap 2³² to 0.
        depth: row
            .get::<_, Option<i64>>(31)?
            .map(|depth| u32::try_from(depth).unwrap_or(crate::session::MAX_AGENT_DEPTH)),
        disowned_peer_session_id: row.get(32)?,
        // NULL for every row that predates v15, and for every row nothing has
        // launched yet: the resume road reads it as "no directory to check",
        // which is the same behaviour the column's absence had.
        cwd: row.get(33)?,
        // NULL for every row that predates v16, and for every session with
        // no goal: both read as `None`.
        goal: row.get(34)?,
    })
}

/// The session's labels, as the JSON object the daemon wrote.
///
/// A column that cannot be parsed is an **empty** map rather than a failure: the
/// labels are display-only, and refusing to list a session because a
/// human-facing annotation is corrupt would take the whole roster down with it.
/// Nothing decides anything from a label, so there is nothing to fail closed
/// about.
fn deserialize_labels(raw: Option<String>) -> std::collections::BTreeMap<String, String> {
    raw.and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The session's overlay, as the JSON array the daemon wrote — or `None`
/// when the cell cannot be read as a deny list. NULL (every row that
/// predates v13) and an empty array both read as no overlay; anything else
/// unreadable — bit rot, a hand edit, a name no broker tool serves — is not
/// an empty restriction but an unreadable cell, and only the resume path
/// judges it: the roster reads the cell through this function but decides
/// nothing from it, so one bad cell cannot take the roster down with it.
/// (A storage class SQLite cannot convert to text still fails the row read
/// itself — the same pre-existing exposure `labels` has; this promise
/// covers malformed JSON, not a mistyped column, whose shape the schema
/// validators refuse at open.)
fn deserialize_overlay(raw: Option<String>) -> Option<crate::provider_catalog::ToolOverlay> {
    let Some(text) = raw else {
        return Some(crate::provider_catalog::ToolOverlay::NONE);
    };
    let names: Vec<String> = serde_json::from_str(&text).ok()?;
    if names.is_empty() {
        return Some(crate::provider_catalog::ToolOverlay::NONE);
    }
    // The store refuses a deny name the broker does not serve; the journal
    // re-checks at read, so a hand-added unknown cannot widen into a tool
    // tomorrow's broker serves under that name.
    let known = names.iter().all(|name| {
        crate::provider_catalog::MCP_BROKER_TOOLS
            .iter()
            .any(|(served, _)| *served == name)
    });
    known.then(|| crate::provider_catalog::ToolOverlay::from_profile_names(&names))
}

pub(crate) fn replay_session(conn: &Connection, session_id: &str) -> Result<Replay, JournalError> {
    let record = conn
        .query_row(
            "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                    generation, status, exit_code, closed, last_seq, degraded,
                    dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped,
                    peer_session_id, provider, origin_kind, origin_device, origin_role,
                    display_name, created_by, profile_id, context_id, unattended, unattended_state, labels,
                    overlay, depth, disowned_peer_session_id, cwd, goal
             FROM sessions WHERE id = ?1",
            [session_id],
            row_to_session,
        )
        .optional()?
        .ok_or(JournalError::SessionNotFound)?;
    if record.closed {
        return Err(JournalError::SessionNotFound);
    }
    let generation = record.generation;
    let mut events: Vec<SessionEvent> = Vec::new();
    let mut event_seqs: Vec<(u64, u64)> = Vec::new();
    let mut event_ts_ms: Vec<Option<u64>> = Vec::new();
    let mut exit_event: Option<SessionEvent> = None;
    // Turns that raised a plan approval mark, read once up front through the
    // shared scan: turn ids are unique across the thread's life, so one pass
    // serves every generation's replay view.
    let plan_turns = if record.kind == SessionKind::Codex {
        crate::codex_plan_marks::scan_conn(conn, session_id)?
    } else {
        HashSet::new()
    };
    let thread_id = record.peer_session_id.clone();
    // The checklist is session state: each generation's fresh views start
    // from the previous generation's list, the way a resumed CLI keeps its
    // own. Partial-stream state stays per generation — only the tasks carry.
    let mut carried_tasks: Option<crate::claude_task_state::ClaudeTaskState> = None;
    // The store holds the whole history: every generation up to the row's
    // own, read in journal order, so appending each one's seq-ordered rows
    // keeps the transcript in (generation, seq) order. What a reader is
    // owed is the pull's decision, never the read's.
    for replayed_generation in 1..=generation {
        let mut gen_events: Vec<SessionEvent> = Vec::new();
        let mut gen_seqs: Vec<u64> = Vec::new();
        let mut gen_ts_ms: Vec<Option<u64>> = Vec::new();
        let mut covered = 0;
        let mut claude_view = crate::claude_view::ClaudeView::new(None);
        if let Some(tasks) = carried_tasks.take() {
            claude_view.restore_task_state(tasks);
        }
        let mut codex_view = crate::codex_view::CodexView::new(None);
        // pi's withheld-finish marker state, consumed by the envelope the
        // marker owns (`pi_view::drive_replay`).
        let mut pi_withheld_finish = false;

        let mut snap_stmt = conn.prepare(
            "SELECT from_seq, up_to_seq, blob, checksum FROM snapshots
             WHERE session_id = ?1 AND generation = ?2
             ORDER BY up_to_seq",
        )?;
        let snaps =
            snap_stmt.query_map(params![session_id, replayed_generation as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)? as u32,
                ))
            })?;
        for snap in snaps {
            let (_from, up_to, blob, checksum) = snap?;
            if crc32(&blob) != checksum {
                return Err(JournalError::Checksum {
                    session_id: session_id.to_string(),
                    seq: up_to,
                });
            }
            let chunks = decode_chunks(&blob).ok_or_else(|| {
                JournalError::Corrupt(format!("snapshot blob for {session_id} up_to {up_to}"))
            })?;
            for (seq, data) in chunks {
                gen_events.push(SessionEvent::Output {
                    seq,
                    data: String::from_utf8_lossy(&data).into_owned(),
                });
                gen_seqs.push(seq);
                // A snapshot blob compresses many rows into one and keeps
                // none of their timestamps.
                gen_ts_ms.push(None);
            }
            covered = covered.max(up_to);
        }

        // Snapshots cover output seqs and raise `covered`, which would hide
        // agent_report rows that stay in `events` (they are not compacted).
        // Reload those rows independently and merge by stream sequence below.
        // The covered half feeds the shared views FIRST: the checklist is
        // one continuous machine per generation, so covered task tools must
        // land before the uncovered results that apply them.
        let mut covered_reports: Vec<(u64, SessionEvent, Option<u64>)> = Vec::new();
        if covered > 0 {
            let mut report_stmt = conn.prepare(
                "SELECT seq, kind, payload, ts_ms, checksum FROM events
                 WHERE session_id = ?1 AND generation = ?2
                   AND kind IN ('agent_report', 'acp_envelope')
                   AND seq <= ?3
                 ORDER BY seq",
            )?;
            let report_rows = report_stmt.query_map(
                params![session_id, replayed_generation as i64, covered as i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as u64,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)? as u64,
                        row.get::<_, i64>(4)? as u32,
                    ))
                },
            )?;
            for row in report_rows {
                let (seq, kind, payload, ts_ms, checksum) = row?;
                if crc32(&payload) != checksum {
                    return Err(JournalError::Checksum {
                        session_id: session_id.to_string(),
                        seq,
                    });
                }
                if kind == "agent_report" {
                    if let Ok(mut event) = serde_json::from_slice::<SessionEvent>(&payload) {
                        crate::plan_text::bound_permission_request(&mut event);
                        covered_reports.push((seq, event, Some(ts_ms)));
                    }
                } else if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&payload)
                {
                    if record.kind == SessionKind::Codex {
                        for view in crate::codex_view::drive_replay(
                            &mut codex_view,
                            thread_id.as_deref(),
                            &plan_turns,
                            &value,
                        ) {
                            covered_reports.push((seq, view, Some(ts_ms)));
                        }
                    } else if record.kind == SessionKind::Pi {
                        for view in crate::pi_view::drive_replay(&mut pi_withheld_finish, &value) {
                            covered_reports.push((seq, view, Some(ts_ms)));
                        }
                    } else {
                        for view in crate::claude_view::drive_replay(&mut claude_view, &mut value) {
                            covered_reports.push((seq, view, Some(ts_ms)));
                        }
                    }
                }
            }
        }

        let mut event_stmt = conn.prepare(
            "SELECT seq, kind, payload, ts_ms, checksum FROM events
             WHERE session_id = ?1 AND generation = ?2 AND seq > ?3
             ORDER BY seq",
        )?;
        let event_rows = event_stmt.query_map(
            params![session_id, replayed_generation as i64, covered as i64],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, i64>(4)? as u32,
                ))
            },
        )?;
        for row in event_rows {
            let (seq, kind, payload, ts_ms, checksum) = row?;
            if crc32(&payload) != checksum {
                return Err(JournalError::Checksum {
                    session_id: session_id.to_string(),
                    seq,
                });
            }
            match EventKind::parse(&kind) {
                Some(EventKind::Output) => {
                    gen_events.push(SessionEvent::Output {
                        seq,
                        data: String::from_utf8_lossy(&payload).into_owned(),
                    });
                    gen_seqs.push(seq);
                    gen_ts_ms.push(Some(ts_ms));
                }
                Some(EventKind::Exit) => {
                    let code = if payload.len() == 4 {
                        Some(u32::from_le_bytes(
                            payload.as_slice().try_into().unwrap_or([0; 4]),
                        ))
                    } else {
                        None
                    };
                    // Only the current generation's exit row speaks for the
                    // session: an earlier generation's exit predates the
                    // resume, and the sessions row's own exit_code is the
                    // authority when the current generation left no row.
                    if replayed_generation == generation {
                        exit_event = Some(SessionEvent::Exit { code });
                    }
                }
                Some(EventKind::AgentReport) => {
                    if let Ok(mut event) = serde_json::from_slice::<SessionEvent>(&payload) {
                        crate::plan_text::bound_permission_request(&mut event);
                        gen_events.push(event);
                        gen_seqs.push(seq);
                        gen_ts_ms.push(Some(ts_ms));
                    }
                }
                Some(EventKind::AcpEnvelope) => {
                    if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&payload) {
                        if record.kind == SessionKind::Codex {
                            for view in crate::codex_view::drive_replay(
                                &mut codex_view,
                                thread_id.as_deref(),
                                &plan_turns,
                                &value,
                            ) {
                                gen_events.push(view);
                                gen_seqs.push(seq);
                                gen_ts_ms.push(Some(ts_ms));
                            }
                        } else if record.kind == SessionKind::Pi {
                            for view in
                                crate::pi_view::drive_replay(&mut pi_withheld_finish, &value)
                            {
                                gen_events.push(view);
                                gen_seqs.push(seq);
                                gen_ts_ms.push(Some(ts_ms));
                            }
                        } else {
                            for view in
                                crate::claude_view::drive_replay(&mut claude_view, &mut value)
                            {
                                gen_events.push(view);
                                gen_seqs.push(seq);
                                gen_ts_ms.push(Some(ts_ms));
                            }
                        }
                    }
                }
                // A kind this binary does not know is a row written by a
                // writer it is not — a downgrade, most likely. The row has no
                // other reader: skipping it leaves `last_seq` claiming a
                // transcript that runs past a hole while `TranscriptIntegrity`
                // still reads `Complete`, so this fails closed, as the paged
                // agent read and `parse_kind` above already do.
                None => {
                    return Err(JournalError::Corrupt(format!(
                        "unknown agent event kind {kind:?} at {session_id} seq {seq}"
                    )))
                }
            }
        }

        if !covered_reports.is_empty() {
            for (seq, event, ts_ms) in covered_reports {
                gen_events.push(event);
                gen_seqs.push(seq);
                gen_ts_ms.push(ts_ms);
            }
            // The zips below truncate to the shortest vector; a writer that
            // drops one push would silently cut the generation's tail.
            debug_assert_eq!(gen_events.len(), gen_seqs.len());
            debug_assert_eq!(gen_seqs.len(), gen_ts_ms.len());
            let mut paired: Vec<(u64, SessionEvent, Option<u64>)> = gen_seqs
                .into_iter()
                .zip(gen_events)
                .zip(gen_ts_ms)
                .map(|((seq, event), ts_ms)| (seq, event, ts_ms))
                .collect();
            paired.sort_by_key(|(seq, _, _)| *seq);
            for (seq, event, ts_ms) in paired {
                events.push(event);
                event_seqs.push((replayed_generation, seq));
                event_ts_ms.push(ts_ms);
            }
        } else {
            debug_assert_eq!(gen_events.len(), gen_seqs.len());
            debug_assert_eq!(gen_seqs.len(), gen_ts_ms.len());
            for ((seq, event), ts_ms) in gen_seqs.into_iter().zip(gen_events).zip(gen_ts_ms) {
                events.push(event);
                event_seqs.push((replayed_generation, seq));
                event_ts_ms.push(ts_ms);
            }
        }
        // The next generation's fresh views start from this list: a resume
        // keeps the CLI's checklist, so the replay keeps it too.
        carried_tasks = Some(claude_view.snapshot_task_state());
    }

    let terminated = matches!(record.status, PersistStatus::Ended)
        || matches!(record.status, PersistStatus::Live) && record.reaped;
    let integrity = record.integrity(terminated);
    // The terminal marker sits at the current generation's end — the
    // session's end. Earlier generations' rows all sort before it. A tail
    // marker comes from the sessions row, not from an event row, so it
    // carries no row time.
    let tail_seq = (generation, record.last_seq);
    if terminated {
        if record.degraded {
            events.push(SessionEvent::JournalDegraded {
                dropped_frames: record.dropped_frames,
                dropped_bytes: record.dropped_bytes,
            });
            event_seqs.push(tail_seq);
            event_ts_ms.push(None);
        }
        events.push(exit_event.unwrap_or(SessionEvent::Exit {
            code: record.exit_code,
        }));
        event_seqs.push(tail_seq);
        event_ts_ms.push(None);
    } else {
        events.push(SessionEvent::Recovered { integrity });
        event_seqs.push(tail_seq);
        event_ts_ms.push(None);
    }

    Ok(Replay {
        generation,
        last_seq: record.last_seq,
        integrity,
        events,
        event_seqs,
        event_ts_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{
        acp_envelope_record, agent_report_record, new_session_record, tmp_journal, Journal,
    };
    use serde_json::{json, Value};

    const CLAUDE_TASKS: &str = include_str!("../fixtures/wire/claude-tasks-synthetic.jsonl");

    fn claude_fixture_envelopes() -> Vec<Value> {
        CLAUDE_TASKS
            .lines()
            .map(|line| serde_json::from_str(line).expect("fixture line"))
            .collect()
    }

    fn agent_tasks_of(events: &[SessionEvent]) -> Vec<Vec<devboule_protocol::AgentTaskItem>> {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentTasks { items } => Some(items.clone()),
                _ => None,
            })
            .collect()
    }

    fn notice_texts(events: &[SessionEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::SessionNotice { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn append_envelopes(journal: &Journal, id: &str, generation: u64, envelopes: &[Value]) {
        for (index, envelope) in envelopes.iter().enumerate() {
            journal
                .append_blocking(
                    acp_envelope_record(id, generation, index as u64 + 1, envelope)
                        .expect("record"),
                )
                .expect("append");
        }
    }

    /// One live Codex view over frames in order, with the per-frame settings
    /// the live reader applies: the capture follows the owned thread.
    fn live_codex(
        thread: &str,
        plan_mode: bool,
        frames: &[Value],
    ) -> Vec<Vec<devboule_protocol::AgentTaskItem>> {
        let mut view = crate::codex_view::CodexView::new(None);
        let mut out = Vec::new();
        for frame in frames {
            view.set_capture_plan(crate::codex_compaction::is_root_thread(
                frame.get("params").unwrap_or(&serde_json::Value::Null),
                thread,
            ));
            view.set_plan_mode(plan_mode);
            for event in view.ingest(frame) {
                if let SessionEvent::AgentTasks { items } = event {
                    out.push(items);
                }
            }
        }
        out
    }

    fn codex_session(journal: &Journal, id: &str, thread: &str) {
        let mut record = new_session_record(id, "owner", None, SessionKind::Codex, "Codex");
        record.peer_session_id = Some(thread.to_string());
        journal.create_session(record).expect("birth");
    }

    #[test]
    fn replay_matches_live_for_codex_plan_frames() {
        // A plan frame on a foreign thread is dropped live and must drop on
        // replay too; a bare second view over the same rows cannot see the
        // thread, so it emits for both — the acde9350 behaviour this pins.
        let started = json!({"method": "turn/started", "params": {
            "threadId": "t-1", "turn": {"id": "turn-A"}}});
        let root = json!({"method": "turn/plan/updated", "params": {
            "threadId": "t-1",
            "plan": [{"step": "One", "status": "pending"},
                       {"step": "Two", "status": "in_progress"}]}});
        let foreign = json!({"method": "turn/plan/updated", "params": {
            "threadId": "t-other",
            "plan": [{"step": "Foreign", "status": "pending"}]}});
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let id = "s.codex.replay";
        codex_session(&journal, id, "t-1");
        append_envelopes(
            &journal,
            id,
            1,
            &[started.clone(), root.clone(), foreign.clone()],
        );
        let replay = journal.replay(id).expect("replay");
        assert_eq!(
            agent_tasks_of(&replay.events),
            live_codex("t-1", false, &[started, root, foreign])
        );
        assert_eq!(agent_tasks_of(&replay.events).len(), 1);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_suppresses_codex_tasks_for_a_card_marked_turn() {
        // The turn's approval verdict journals under `{turn}-plan`; the
        // replay recovers the mode from that mark, so the turn's plan
        // frames emit nothing — as live with plan mode on.
        let started = json!({"method": "turn/started", "params": {
            "threadId": "t-1", "turn": {"id": "turn-T"}}});
        let update = json!({"method": "turn/plan/updated", "params": {
            "threadId": "t-1",
            "plan": [{"step": "Planned", "status": "pending"}]}});
        let verdict = SessionEvent::AgentToolUpdate {
            tool_call_id: "turn-T-plan".to_string(),
            status: Some("completed".to_string()),
            text: None,
            title: Some("Approved".to_string()),
            kind: Some("plan".to_string()),
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
        };
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let id = "s.codex.card";
        codex_session(&journal, id, "t-1");
        append_envelopes(&journal, id, 1, &[started, update]);
        journal
            .append_blocking(agent_report_record(id, 1, 3, &verdict).expect("record"))
            .expect("verdict");
        let replay = journal.replay(id).expect("replay");
        assert!(
            agent_tasks_of(&replay.events).is_empty(),
            "a card-marked turn replays suppressed: {:?}",
            replay.events
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_matches_live_for_claude_task_envelopes() {
        // The real replay path: envelopes journalled as rows, rebuilt by
        // `Journal::replay`, compared against one live view over the same
        // envelopes in order.
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let id = "s.tasks.replay";
        journal
            .create_session(new_session_record(
                id,
                "owner",
                None,
                SessionKind::Claude,
                "Tasks",
            ))
            .expect("birth");
        let envelopes = claude_fixture_envelopes();
        append_envelopes(&journal, id, 1, &envelopes);
        let replay = journal.replay(id).expect("replay");
        let replay_tasks = agent_tasks_of(&replay.events);

        let mut live = crate::claude_view::ClaudeView::new(None);
        let mut live_tasks = Vec::new();
        for envelope in &envelopes {
            for event in live.ingest(envelope) {
                if let SessionEvent::AgentTasks { items } = event {
                    live_tasks.push(items);
                }
            }
        }
        assert_eq!(replay_tasks, live_tasks);
        assert_eq!(live_tasks.len(), 4);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn plan_usages_of(events: &[SessionEvent]) -> Vec<SessionEvent> {
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::PlanUsage { .. }))
            .cloned()
            .collect()
    }

    #[test]
    fn replayed_claude_notices_derive_once_per_frame() {
        // The compaction marker and the slash-command answer are derived from
        // the journalled envelope, never journalled beside it — so the replay
        // shows each exactly once, and the boundary's back-to-back repeat
        // stays one marker there too.
        let boundary = json!({"type": "system", "subtype": "compact_boundary",
            "compact_metadata": {"trigger": "manual", "pre_tokens": 52345}});
        let slash = json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "text",
             "text": "<local-command-stdout>usage answer</local-command-stdout>"}]}});
        let envelopes = vec![boundary.clone(), boundary, slash];
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let id = "s.claude.notices.replay";
        journal
            .create_session(new_session_record(
                id,
                "owner",
                None,
                SessionKind::Claude,
                "Notices",
            ))
            .expect("birth");
        append_envelopes(&journal, id, 1, &envelopes);
        let replay = journal.replay(id).expect("replay");
        let replay_notices = notice_texts(&replay.events);
        assert_eq!(
            replay_notices,
            vec![
                "Context manually compacted".to_string(),
                "usage answer".to_string(),
            ],
            "one marker per compaction and one per slash answer, in order"
        );

        // Live derives the same, from the same frames in the same order.
        let mut live = crate::claude_view::ClaudeView::new(None);
        let mut live_notices = Vec::new();
        for envelope in &envelopes {
            live_notices.extend(notice_texts(&live.ingest(envelope)));
        }
        assert_eq!(replay_notices, live_notices);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replayed_expiry_and_late_answer_show_one_finish() {
        // The live expiry journals its error and finish rows, and the late
        // answer's envelope carries the suppression marker the live pass
        // used. Replay must show the same single finish — never the
        // envelope's second one.
        let error = SessionEvent::AgentError {
            message: "Claude produced no output for 300 s during the turn; the run was ended."
                .to_string(),
        };
        let finish = SessionEvent::AgentFinished {
            stop_reason: "error".to_string(),
            model_id: None,
            usage: None,
        };
        let marker = crate::claude_view::withheld_finish_marker();
        let late = json!({"type": "result", "subtype": "success", "is_error": true,
            "terminal_reason": "aborted by interrupt", "stop_reason": "end_turn"});
        let finishes = |id: &str, rows: Vec<EventRecord>| {
            let (dir, path) = tmp_journal();
            let journal = Journal::open(&path).expect("open");
            journal
                .create_session(new_session_record(
                    id,
                    "owner",
                    None,
                    SessionKind::Claude,
                    "N2",
                ))
                .expect("birth");
            for row in rows {
                journal.append_blocking(row).expect("append");
            }
            let replay = journal.replay(id).expect("replay");
            let count = replay
                .events
                .iter()
                .filter(|event| matches!(event, SessionEvent::AgentFinished { .. }))
                .count();
            journal.shutdown();
            let _ = std::fs::remove_dir_all(&dir);
            count
        };
        let reports = |id: &str, with_marker: bool| {
            let mut rows = vec![
                agent_report_record(id, 1, 1, &error).expect("row"),
                agent_report_record(id, 1, 2, &finish).expect("row"),
            ];
            let mut seq = 3;
            if with_marker {
                rows.push(acp_envelope_record(id, 1, seq, &marker).expect("row"));
                seq += 1;
            }
            rows.push(acp_envelope_record(id, 1, seq, &late).expect("row"));
            rows
        };
        assert_eq!(
            finishes("s.n2.marked", reports("s.n2.marked", true)),
            1,
            "marker plus late envelope replay to the watchdog's single finish"
        );
        assert_eq!(
            finishes("s.n2.unmarked", reports("s.n2.unmarked", false)),
            2,
            "without the marker the envelope's finish doubles: the marker is load-bearing"
        );
    }

    #[test]
    fn replayed_rate_limits_do_not_re_emit_plan_usage() {
        // The one exception to replay-equals-live: plan usage is the
        // account's LIVE state, not transcript — a replayed frame would
        // overwrite the provider's current reading in the app's per-provider
        // store. The same frames live still emit, pinned below.
        let envelopes: Vec<Value> =
            include_str!("../fixtures/wire/claude-rate-limits-synthetic.jsonl")
                .lines()
                .map(|line| serde_json::from_str(line).expect("fixture line"))
                .collect();
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let id = "s.rate.replay";
        journal
            .create_session(new_session_record(
                id,
                "owner",
                None,
                SessionKind::Claude,
                "Rate",
            ))
            .expect("birth");
        append_envelopes(&journal, id, 1, &envelopes);
        let replay = journal.replay(id).expect("replay");
        assert!(
            plan_usages_of(&replay.events).is_empty(),
            "replay must not re-emit plan usage: {:?}",
            plan_usages_of(&replay.events)
        );

        let mut live = crate::claude_view::ClaudeView::new(None);
        let mut live_events = Vec::new();
        for envelope in &envelopes {
            live_events.extend(live.ingest(envelope));
        }
        assert_eq!(
            plan_usages_of(&live_events).len(),
            3,
            "the same frames live emit one event each"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replayed_codex_rate_limits_do_not_re_emit_plan_usage() {
        // Same exception, Codex side: the journalled frame re-derives
        // nothing on replay, and live still maps it.
        let rate_limits = crate::codex_view::fixture_frames(include_str!(
            "../fixtures/wire/codex/E1-step1-handshake.jsonl"
        ))
        .into_iter()
        .find(|frame| {
            frame.get("method").and_then(Value::as_str) == Some("account/rateLimits/updated")
        })
        .expect("measured frame 26");
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let id = "s.codex.rate.replay";
        journal
            .create_session(new_session_record(
                id,
                "owner",
                None,
                SessionKind::Codex,
                "Rate",
            ))
            .expect("birth");
        append_envelopes(&journal, id, 1, std::slice::from_ref(&rate_limits));
        let replay = journal.replay(id).expect("replay");
        assert!(
            plan_usages_of(&replay.events).is_empty(),
            "replay must not re-emit plan usage: {:?}",
            plan_usages_of(&replay.events)
        );

        let mut live = crate::codex_view::CodexView::new(None);
        assert_eq!(
            plan_usages_of(&live.ingest(&rate_limits)).len(),
            1,
            "the same frame live emits one event"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_keeps_the_checklist_across_a_resume() {
        // A resume is a new generation with a fresh process: the replay must
        // carry the list, or the first post-resume update finds no task and
        // the pill freezes. A bare second view over only the new envelopes
        // derives nothing — that is the acde9350 behaviour this pins.
        let todo = json!({"type": "assistant", "message": {"id": "m1", "role": "assistant",
            "content": [{"type": "tool_use", "id": "todo-1", "name": "TodoWrite",
                "input": {"todos": [{"content": "Legacy", "status": "pending"}]}}]}});
        let create = json!({"type": "assistant", "message": {"id": "m2", "role": "assistant",
            "content": [{"type": "tool_use", "id": "create-1", "name": "TaskCreate",
                "input": {"subject": "Alpha"}}]}});
        let created = json!({"type": "user",
            "message": {"role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "create-1", "content": "ok"}]},
            "tool_use_result": {"task": {"id": "1", "subject": "Alpha"}}});
        let update = json!({"type": "assistant", "message": {"id": "m3", "role": "assistant",
            "content": [{"type": "tool_use", "id": "update-1", "name": "TaskUpdate",
                "input": {"taskId": "1", "status": "in_progress"}}]}});
        let updated = json!({"type": "user",
            "message": {"role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "update-1", "content": "ok"}]},
            "tool_use_result": {"success": true, "taskId": "1"}});
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let id = "s.tasks.resume";
        journal
            .create_session(new_session_record(
                id,
                "owner",
                None,
                SessionKind::Claude,
                "Tasks",
            ))
            .expect("birth");
        append_envelopes(
            &journal,
            id,
            1,
            &[todo.clone(), create.clone(), created.clone()],
        );
        journal.start_generation(id, 2).expect("resume");
        append_envelopes(&journal, id, 2, &[update.clone(), updated.clone()]);

        let replay = journal.replay(id).expect("replay");
        let replay_tasks = agent_tasks_of(&replay.events);

        // Live is one continuous view over generation 1 then 2 in order.
        let mut live = crate::claude_view::ClaudeView::new(None);
        let mut live_tasks = Vec::new();
        for envelope in &[todo, create, created, update, updated] {
            for event in live.ingest(envelope) {
                if let SessionEvent::AgentTasks { items } = event {
                    live_tasks.push(items);
                }
            }
        }
        assert_eq!(replay_tasks, live_tasks);
        assert_eq!(live_tasks.len(), 3);
        assert_eq!(live_tasks[2].len(), 2);
        assert_eq!(
            live_tasks[2][1].status,
            devboule_protocol::AgentTaskStatus::InProgress
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

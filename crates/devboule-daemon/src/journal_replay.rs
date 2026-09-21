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

pub(super) fn list_sessions(conn: &Connection) -> Result<Vec<SessionRecord>, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped,
                peer_session_id, provider, origin_kind, origin_device, origin_role,
                display_name, created_by, profile_id, context_id, unattended, unattended_state, labels,
                overlay, depth, disowned_peer_session_id, cwd
         FROM sessions WHERE closed = 0 ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_session)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(JournalError::from)
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

pub(super) fn replay_session(conn: &Connection, session_id: &str) -> Result<Replay, JournalError> {
    let record = conn
        .query_row(
            "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                    generation, status, exit_code, closed, last_seq, degraded,
                    dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped,
                    peer_session_id, provider, origin_kind, origin_device, origin_role,
                    display_name, created_by, profile_id, context_id, unattended, unattended_state, labels,
                    overlay, depth, disowned_peer_session_id, cwd
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
    let mut exit_event: Option<SessionEvent> = None;
    // The store holds the whole history: every generation up to the row's
    // own, read in journal order, so appending each one's seq-ordered rows
    // keeps the transcript in (generation, seq) order. What a reader is
    // owed is the pull's decision, never the read's.
    for replayed_generation in 1..=generation {
        let mut gen_events: Vec<SessionEvent> = Vec::new();
        let mut gen_seqs: Vec<u64> = Vec::new();
        let mut covered = 0;
        let mut claude_view = crate::claude_view::ClaudeView::new(None);
        let mut codex_view = crate::codex_view::CodexView::new(None);

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
            }
            covered = covered.max(up_to);
        }

        let mut event_stmt = conn.prepare(
            "SELECT seq, kind, payload, checksum FROM events
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
                    row.get::<_, i64>(3)? as u32,
                ))
            },
        )?;
        for row in event_rows {
            let (seq, kind, payload, checksum) = row?;
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
                    if let Ok(event) = serde_json::from_slice::<SessionEvent>(&payload) {
                        gen_events.push(event);
                        gen_seqs.push(seq);
                    }
                }
                Some(EventKind::AcpEnvelope) => {
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) {
                        if record.kind == SessionKind::Codex {
                            for view in codex_view.ingest(&value) {
                                gen_events.push(view);
                                gen_seqs.push(seq);
                            }
                        } else if record.kind == SessionKind::Pi {
                            for view in crate::pi_view::events_from_line(&value) {
                                gen_events.push(view);
                                gen_seqs.push(seq);
                            }
                        } else if let Some(view) = crate::acp_view::view_from_envelope(&value, "") {
                            gen_events.push(view);
                            gen_seqs.push(seq);
                        } else {
                            for view in claude_view.ingest(&value) {
                                gen_events.push(view);
                                gen_seqs.push(seq);
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

        // Snapshots cover output seqs and raise `covered`, which would hide
        // agent_report rows that stay in `events` (they are not compacted).
        // Reload those rows independently and merge by stream sequence.
        let mut covered_reports: Vec<(u64, SessionEvent)> = Vec::new();
        let mut covered_claude = crate::claude_view::ClaudeView::new(None);
        let mut covered_codex = crate::codex_view::CodexView::new(None);
        if covered > 0 {
            let mut report_stmt = conn.prepare(
                "SELECT seq, kind, payload, checksum FROM events
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
                        row.get::<_, i64>(3)? as u32,
                    ))
                },
            )?;
            for row in report_rows {
                let (seq, kind, payload, checksum) = row?;
                if crc32(&payload) != checksum {
                    return Err(JournalError::Checksum {
                        session_id: session_id.to_string(),
                        seq,
                    });
                }
                if kind == "agent_report" {
                    if let Ok(event) = serde_json::from_slice::<SessionEvent>(&payload) {
                        covered_reports.push((seq, event));
                    }
                } else if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) {
                    if record.kind == SessionKind::Codex {
                        for view in covered_codex.ingest(&value) {
                            covered_reports.push((seq, view));
                        }
                    } else if record.kind == SessionKind::Pi {
                        for view in crate::pi_view::events_from_line(&value) {
                            covered_reports.push((seq, view));
                        }
                    } else if let Some(view) = crate::acp_view::view_from_envelope(&value, "") {
                        covered_reports.push((seq, view));
                    } else {
                        for view in covered_claude.ingest(&value) {
                            covered_reports.push((seq, view));
                        }
                    }
                }
            }
        }
        if !covered_reports.is_empty() {
            for (seq, event) in covered_reports {
                gen_events.push(event);
                gen_seqs.push(seq);
            }
            let mut paired: Vec<(u64, SessionEvent)> =
                gen_seqs.into_iter().zip(gen_events).collect();
            paired.sort_by_key(|(seq, _)| *seq);
            for (seq, event) in paired {
                events.push(event);
                event_seqs.push((replayed_generation, seq));
            }
        } else {
            for (seq, event) in gen_seqs.into_iter().zip(gen_events) {
                events.push(event);
                event_seqs.push((replayed_generation, seq));
            }
        }
    }

    let terminated = matches!(record.status, PersistStatus::Ended)
        || matches!(record.status, PersistStatus::Live) && record.reaped;
    let integrity = record.integrity(terminated);
    // The terminal marker sits at the current generation's end — the
    // session's end. Earlier generations' rows all sort before it.
    let tail_seq = (generation, record.last_seq);
    if terminated {
        if record.degraded {
            events.push(SessionEvent::JournalDegraded {
                dropped_frames: record.dropped_frames,
                dropped_bytes: record.dropped_bytes,
            });
            event_seqs.push(tail_seq);
        }
        events.push(exit_event.unwrap_or(SessionEvent::Exit {
            code: record.exit_code,
        }));
        event_seqs.push(tail_seq);
    } else {
        events.push(SessionEvent::Recovered { integrity });
        event_seqs.push(tail_seq);
    }

    Ok(Replay {
        generation,
        last_seq: record.last_seq,
        integrity,
        events,
        event_seqs,
    })
}

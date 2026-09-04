use rusqlite::{params, Connection, OptionalExtension};

use devboule_protocol::SessionEvent;

use super::{
    crc32, decode_chunks, parse_kind, EventKind, JournalError, PersistStatus, Replay, SessionRecord,
};

pub(super) fn list_sessions(conn: &Connection) -> Result<Vec<SessionRecord>, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped
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
        kind: parse_kind(&row.get::<_, String>(3)?),
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
    })
}

pub(super) fn replay_session(
    conn: &Connection,
    session_id: &str,
    from_seq: u64,
) -> Result<Replay, JournalError> {
    let record = conn
        .query_row(
            "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                    generation, status, exit_code, closed, last_seq, degraded,
                    dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, reaped
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
    let mut covered = from_seq;

    let mut snap_stmt = conn.prepare(
        "SELECT from_seq, up_to_seq, blob, checksum FROM snapshots
         WHERE session_id = ?1 AND generation = ?2 AND up_to_seq > ?3
         ORDER BY up_to_seq",
    )?;
    let snaps = snap_stmt.query_map(
        params![session_id, generation as i64, from_seq as i64],
        |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)? as u32,
            ))
        },
    )?;
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
            if seq > from_seq {
                events.push(SessionEvent::Output {
                    seq,
                    data: String::from_utf8_lossy(&data).into_owned(),
                });
            }
        }
        covered = covered.max(up_to);
    }

    let mut event_stmt = conn.prepare(
        "SELECT seq, kind, payload, checksum FROM events
         WHERE session_id = ?1 AND generation = ?2 AND seq > ?3
         ORDER BY seq",
    )?;
    let event_rows = event_stmt.query_map(
        params![session_id, generation as i64, covered as i64],
        |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)? as u32,
            ))
        },
    )?;
    let mut exit_event: Option<SessionEvent> = None;
    for row in event_rows {
        let (seq, kind, payload, checksum) = row?;
        if crc32(&payload) != checksum {
            return Err(JournalError::Checksum {
                session_id: session_id.to_string(),
                seq,
            });
        }
        match EventKind::parse(&kind) {
            Some(EventKind::Output) => events.push(SessionEvent::Output {
                seq,
                data: String::from_utf8_lossy(&payload).into_owned(),
            }),
            Some(EventKind::Exit) => {
                let code = if payload.len() == 4 {
                    Some(u32::from_le_bytes(
                        payload.as_slice().try_into().unwrap_or([0; 4]),
                    ))
                } else {
                    None
                };
                exit_event = Some(SessionEvent::Exit { code });
            }
            Some(EventKind::AgentReport) => {
                if let Ok(event) = serde_json::from_slice::<SessionEvent>(&payload) {
                    events.push(event);
                }
            }
            Some(EventKind::AcpEnvelope) => {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) {
                    if let Some(view) = crate::acp_view::view_from_envelope(&value, "") {
                        events.push(view);
                    }
                }
            }
            None => {}
        }
    }

    // Snapshots cover output seqs and raise `covered`, which would hide
    // agent_report rows that stay in `events` (they are not compacted).
    // Reload those rows independently and merge by stream sequence.
    let mut covered_reports = Vec::new();
    if covered > from_seq {
        let mut report_stmt = conn.prepare(
            "SELECT seq, kind, payload, checksum FROM events
             WHERE session_id = ?1 AND generation = ?2
               AND kind IN ('agent_report', 'acp_envelope')
               AND seq > ?3 AND seq <= ?4
             ORDER BY seq",
        )?;
        let report_rows = report_stmt.query_map(
            params![
                session_id,
                generation as i64,
                from_seq as i64,
                covered as i64
            ],
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
                    covered_reports.push(event);
                }
            } else if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload) {
                if let Some(view) = crate::acp_view::view_from_envelope(&value, "") {
                    covered_reports.push(view);
                }
            }
        }
    }
    if !covered_reports.is_empty() {
        events.extend(covered_reports);
        events.sort_by_key(|event| match event {
            SessionEvent::Output { seq, .. } | SessionEvent::AgentReported { seq, .. } => *seq,
            _ => u64::MAX,
        });
    }

    let terminated = matches!(record.status, PersistStatus::Ended)
        || matches!(record.status, PersistStatus::Live) && record.reaped;
    let integrity = record.integrity(terminated);
    if terminated {
        if record.degraded {
            events.push(SessionEvent::JournalDegraded {
                dropped_frames: record.dropped_frames,
                dropped_bytes: record.dropped_bytes,
            });
        }
        events.push(exit_event.unwrap_or(SessionEvent::Exit {
            code: record.exit_code,
        }));
    } else {
        events.push(SessionEvent::Recovered { integrity });
    }

    Ok(Replay {
        generation,
        last_seq: record.last_seq,
        integrity,
        events,
    })
}

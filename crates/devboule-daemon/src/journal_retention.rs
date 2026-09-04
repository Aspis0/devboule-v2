use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};

use devboule_protocol::{JournalRetention, RetentionLimit, RetentionPatch, RetentionSource};

use super::{
    now_ms, parse_kind, JournalError, JournalLimits, JournalSessionUsage, JournalUsage,
    Unreclaimable,
};

/// Global retention scans all sessions, so the writer runs that work once at
/// startup or after the session set changes, then after each 1 MiB of output.
/// The affected session's per-session cap remains checked on every append.
const RETENTION_GLOBAL_SWEEP_BYTES: u64 = 1024 * 1024;

pub(super) struct RetentionState {
    bytes_since_global_sweep: u64,
    global_sweep_pending: bool,
}

impl Default for RetentionState {
    fn default() -> Self {
        Self {
            bytes_since_global_sweep: 0,
            global_sweep_pending: true,
        }
    }
}

impl RetentionState {
    pub(super) fn session_set_changed(&mut self) {
        self.global_sweep_pending = true;
    }

    pub(super) fn global_sweep_due(&self, appended_bytes: u64) -> bool {
        self.global_sweep_pending
            || self.bytes_since_global_sweep.saturating_add(appended_bytes)
                >= RETENTION_GLOBAL_SWEEP_BYTES
    }

    pub(super) fn append_committed(&mut self, appended_bytes: u64, global_sweep: bool) {
        if global_sweep {
            self.bytes_since_global_sweep = 0;
            self.global_sweep_pending = false;
        } else {
            self.bytes_since_global_sweep =
                self.bytes_since_global_sweep.saturating_add(appended_bytes);
        }
    }
}

pub(super) fn effective_limits(
    conn: &Connection,
    defaults: JournalLimits,
) -> Result<JournalLimits, JournalError> {
    let mut limits = defaults;
    let mut stmt = conn.prepare("SELECT key, value FROM journal_settings")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (key, value) = row?;
        match key.as_str() {
            "max_age_ms" => limits.max_age_ms = setting_u64(&key, value)?,
            "max_sessions" => limits.max_sessions = setting_usize(&key, value)?,
            "max_bytes" => limits.max_bytes = setting_u64(&key, value)?,
            "session_max_bytes" => limits.session_max_bytes = setting_u64(&key, value)?,
            _ => {}
        }
    }
    Ok(limits)
}

fn setting_u64(key: &str, value: i64) -> Result<u64, JournalError> {
    u64::try_from(value)
        .map_err(|_| JournalError::Corrupt(format!("journal setting {key} is negative")))
}

fn setting_usize(key: &str, value: i64) -> Result<usize, JournalError> {
    usize::try_from(value)
        .map_err(|_| JournalError::Corrupt(format!("journal setting {key} is invalid")))
}

pub(super) fn journal_retention(
    conn: &Connection,
    defaults: JournalLimits,
) -> Result<JournalRetention, JournalError> {
    let limits = effective_limits(conn, defaults)?;
    let mut user_settings = HashSet::new();
    let mut stmt = conn.prepare("SELECT key FROM journal_settings")?;
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        user_settings.insert(row?);
    }
    let setting = |key: &str, value| RetentionLimit {
        value,
        source: if user_settings.contains(key) {
            RetentionSource::User
        } else {
            RetentionSource::Default
        },
    };
    Ok(JournalRetention {
        session_max_bytes: setting("session_max_bytes", limits.session_max_bytes),
        max_bytes: setting("max_bytes", limits.max_bytes),
        max_sessions: setting("max_sessions", limits.max_sessions as u64),
        max_age_ms: setting("max_age_ms", limits.max_age_ms),
    })
}

pub(super) fn set_journal_retention(
    conn: &Connection,
    defaults: JournalLimits,
    patch: RetentionPatch,
) -> Result<JournalRetention, JournalError> {
    if patch.max_age_ms.is_none()
        && patch.max_bytes.is_none()
        && patch.max_sessions.is_none()
        && patch.session_max_bytes.is_none()
    {
        return Err(JournalError::InvalidRequest(
            "Set at least one retention limit.".to_string(),
        ));
    }
    for value in [
        patch.max_age_ms,
        patch.max_bytes,
        patch.max_sessions,
        patch.session_max_bytes,
    ]
    .into_iter()
    .flatten()
    {
        if value < 0 {
            return Err(JournalError::InvalidRequest(
                "Retention limits cannot be negative.".to_string(),
            ));
        }
    }
    let current = effective_limits(conn, defaults)?;
    let max_bytes = patch
        .max_bytes
        .map(|value| value as u64)
        .unwrap_or(current.max_bytes);
    let session_max_bytes = patch
        .session_max_bytes
        .map(|value| value as u64)
        .unwrap_or(current.session_max_bytes);
    // Zero disables either cap, so it is not a finite value that can make
    // the per-session limit inconsistent with the global byte limit.
    if max_bytes > 0 && session_max_bytes > 0 && session_max_bytes > max_bytes {
        return Err(JournalError::InvalidRequest(
            "session_max_bytes cannot be greater than max_bytes.".to_string(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    for (key, value) in [
        ("max_age_ms", patch.max_age_ms),
        ("max_bytes", patch.max_bytes),
        ("max_sessions", patch.max_sessions),
        ("session_max_bytes", patch.session_max_bytes),
    ] {
        if let Some(value) = value {
            tx.execute(
                "INSERT INTO journal_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
    }
    tx.commit()?;
    journal_retention(conn, defaults)
}

pub(super) fn retain(
    conn: &rusqlite::Transaction<'_>,
    pins: &HashSet<String>,
    now_ms: u64,
    limits: JournalLimits,
    session_id: &str,
    run_global_sweep: bool,
) -> Result<(), JournalError> {
    trim_session(conn, session_id, limits)?;
    if run_global_sweep {
        retain_global(conn, pins, now_ms, limits)?;
    }
    Ok(())
}

fn trim_session(
    conn: &rusqlite::Transaction<'_>,
    session_id: &str,
    limits: JournalLimits,
) -> Result<(), JournalError> {
    if limits.session_max_bytes == 0 {
        return Ok(());
    }
    let Some((kind, payload)) = conn
        .query_row(
            "SELECT kind, payload_bytes FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    if kind == "acp" {
        return Ok(());
    }
    let mut remaining = payload.max(0) as u64;
    while remaining > limits.session_max_bytes {
        let oldest: Option<(i64, i64)> = conn
            .query_row(
                "SELECT up_to_seq, payload_bytes FROM snapshots WHERE session_id = ?1 ORDER BY up_to_seq ASC LIMIT 1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((up_to, snap_bytes)) = oldest {
            conn.execute(
                "DELETE FROM snapshots WHERE session_id = ?1 AND up_to_seq = ?2",
                params![session_id, up_to],
            )?;
            conn.execute(
                "UPDATE sessions SET
                    payload_bytes = MAX(payload_bytes - ?1, 0),
                    trimmed_bytes = trimmed_bytes + ?1
                 WHERE id = ?2",
                params![snap_bytes, session_id],
            )?;
            remaining = remaining.saturating_sub(snap_bytes.max(0) as u64);
        } else {
            let oldest_event: Option<(i64, i64)> = conn
                .query_row(
                    "SELECT seq, LENGTH(payload) FROM events WHERE session_id = ?1 AND kind = 'output' ORDER BY seq ASC LIMIT 1",
                    [session_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((seq, bytes)) = oldest_event {
                conn.execute(
                    "DELETE FROM events WHERE session_id = ?1 AND seq = ?2 AND kind = 'output'",
                    params![session_id, seq],
                )?;
                conn.execute(
                    "UPDATE sessions SET
                        payload_bytes = MAX(payload_bytes - ?1, 0),
                        trimmed_bytes = trimmed_bytes + ?1
                     WHERE id = ?2",
                    params![bytes, session_id],
                )?;
                remaining = remaining.saturating_sub(bytes.max(0) as u64);
            } else {
                break;
            }
        }
    }
    Ok(())
}

fn retain_global(
    conn: &rusqlite::Transaction<'_>,
    pins: &HashSet<String>,
    now_ms: u64,
    limits: JournalLimits,
) -> Result<(), JournalError> {
    let ids: Vec<(String, String, String, i64, i64)> = {
        let mut stmt =
            conn.prepare("SELECT id, status, kind, updated_at_ms, payload_bytes FROM sessions")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    if limits.max_age_ms > 0 {
        let cutoff = now_ms.saturating_sub(limits.max_age_ms) as i64;
        let aged: Vec<String> = ids
            .iter()
            .filter(|(id, status, kind, updated, _payload)| {
                !pins.contains(id) && kind != "acp" && *status != "live" && *updated < cutoff
            })
            .map(|(id, ..)| id.clone())
            .collect();
        for id in aged {
            delete_session(conn, &id, DeleteReason::LimitAge)?;
        }
    }

    loop {
        let (count, total) = session_totals(conn)?;
        let over_sessions = limits.max_sessions > 0 && count > limits.max_sessions;
        let over_bytes = limits.max_bytes > 0 && total > limits.max_bytes;
        if !over_sessions && !over_bytes {
            break;
        }
        match pick_trim_victim(conn, pins)? {
            Some(id) => {
                // If both caps bind, bytes is the deliberate tie-break: it
                // identifies the payload overage that caused the pressure.
                let reason = if over_bytes {
                    DeleteReason::LimitBytes
                } else {
                    DeleteReason::LimitSessions
                };
                delete_session(conn, &id, reason)?;
            }
            None => break,
        }
    }
    Ok(())
}

fn session_totals(conn: &rusqlite::Transaction<'_>) -> Result<(usize, u64), JournalError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
    let total: i64 = conn.query_row(
        "SELECT COALESCE(SUM(payload_bytes), 0) FROM sessions",
        [],
        |row| row.get(0),
    )?;
    Ok((count as usize, total as u64))
}

fn pick_trim_victim(
    conn: &rusqlite::Transaction<'_>,
    pins: &HashSet<String>,
) -> Result<Option<String>, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM sessions
         WHERE status != 'live' AND kind != 'acp'
         ORDER BY closed DESC, updated_at_ms ASC",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if !pins.contains(&id) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

#[derive(Clone, Copy)]
enum DeleteReason {
    User,
    LimitBytes,
    LimitSessions,
    LimitAge,
}

impl DeleteReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::LimitBytes => "limit_bytes",
            Self::LimitSessions => "limit_sessions",
            Self::LimitAge => "limit_age",
        }
    }
}

fn delete_session(
    conn: &rusqlite::Transaction<'_>,
    id: &str,
    reason: DeleteReason,
) -> Result<(), JournalError> {
    let metadata: Option<(Option<String>, String, String, i64, String, i64)> = conn
        .query_row(
            "SELECT workspace_id, kind, title, created_at_ms, status, payload_bytes
             FROM sessions WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let (workspace_id, kind, title, created_at_ms, status, payload_bytes) =
        metadata.ok_or(JournalError::SessionNotFound)?;
    if matches!(reason, DeleteReason::User) && status == "live" {
        return Err(JournalError::LiveSession);
    }
    conn.execute("DELETE FROM events WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM snapshots WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM turns WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM permissions WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
    conn.execute(
        "INSERT INTO deleted_sessions (
            id, workspace_id, kind, title, created_at_ms, deleted_at_ms, reason, bytes_removed
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            workspace_id,
            kind,
            title,
            created_at_ms,
            now_ms() as i64,
            reason.as_str(),
            payload_bytes.max(0),
        ],
    )?;
    // Tombstones are deliberately bounded: the oldest records are the one
    // accepted loss of history, so retention cannot recurse forever.
    conn.execute(
        "DELETE FROM deleted_sessions WHERE id IN (
             SELECT id FROM deleted_sessions
             ORDER BY deleted_at_ms DESC, id DESC
             LIMIT -1 OFFSET 10000
         )",
        [],
    )?;
    Ok(())
}

pub(super) fn delete_session_user(conn: &Connection, id: &str) -> Result<(), JournalError> {
    let tx = conn.unchecked_transaction()?;
    delete_session(&tx, id, DeleteReason::User)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn journal_usage(
    conn: &Connection,
    pins: &HashSet<String>,
    defaults: JournalLimits,
) -> Result<JournalUsage, JournalError> {
    let limits = effective_limits(conn, defaults)?;
    let deleted_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM deleted_sessions", [], |row| {
            row.get(0)
        })?;
    let cutoff = (limits.max_age_ms > 0).then(|| now_ms().saturating_sub(limits.max_age_ms));
    let mut session_count = 0usize;
    let mut total_bytes = 0u64;
    let mut reclaimable_bytes = 0u64;
    let mut reclaimable_sessions = 0usize;
    let mut aged_out = 0usize;
    let mut per_session = Vec::new();
    let mut session_stmt = conn.prepare(
        "SELECT id, title, kind, payload_bytes, updated_at_ms, status
         FROM sessions ORDER BY updated_at_ms DESC, id",
    )?;
    let mut session_rows = session_stmt.query([])?;
    while let Some(row) = session_rows.next()? {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let kind_name: String = row.get(2)?;
        let bytes = row.get::<_, i64>(3)?.max(0) as u64;
        let updated_at_ms = row.get::<_, i64>(4)?.max(0) as u64;
        let status: String = row.get(5)?;
        let reclaimable = status != "live" && kind_name != "acp" && !pins.contains(&id);

        session_count += 1;
        total_bytes = total_bytes.saturating_add(bytes);
        if reclaimable {
            reclaimable_bytes = reclaimable_bytes.saturating_add(bytes);
            reclaimable_sessions += 1;
        }
        if cutoff.is_some_and(|value| updated_at_ms < value) && !reclaimable {
            aged_out += 1;
        }
        per_session.push(JournalSessionUsage {
            id,
            title,
            kind: parse_kind(&kind_name),
            bytes,
            updated_at_ms,
        });
    }

    // The ordered probe only needs to see past every pinned candidate: the
    // first non-pinned row must be within pins.len() + 1 rows. This keeps the
    // reclaimability check bounded without approximating its result.
    let has_reclaimable = {
        let mut stmt = conn.prepare(
            "SELECT id FROM sessions
             WHERE status != 'live' AND kind != 'acp'
             ORDER BY closed DESC, updated_at_ms ASC
             LIMIT ?1",
        )?;
        let probe_limit = pins.len().saturating_add(1) as i64;
        let mut rows = stmt.query([probe_limit])?;
        let mut found = false;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            if !pins.contains(&id) {
                found = true;
                break;
            }
        }
        found
    };
    let unreclaimable = Unreclaimable {
        bytes_over: if limits.max_bytes > 0 && total_bytes > limits.max_bytes {
            let over = total_bytes - limits.max_bytes;
            if has_reclaimable {
                over.saturating_sub(reclaimable_bytes)
            } else {
                over
            }
        } else {
            0
        },
        sessions_over: if limits.max_sessions > 0 && session_count > limits.max_sessions {
            let over = session_count - limits.max_sessions;
            if has_reclaimable {
                over.saturating_sub(reclaimable_sessions)
            } else {
                over
            }
        } else {
            0
        },
        aged_out,
    };
    Ok(JournalUsage {
        total_bytes,
        session_count,
        deleted_count: deleted_count.max(0) as usize,
        unreclaimable,
        limits,
        per_session,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use rusqlite::Connection;

    use devboule_protocol::{SessionEvent, SessionKind, TranscriptIntegrity};

    use super::super::{
        append_event, new_session_record, now_ms, output_record, sample_session, snapshot_limits,
        tiny_limits, tmp_journal, Journal, JournalError, JournalLimits, PersistStatus,
    };
    use super::RetentionState;

    /// The per-session trim is intentionally cheap on every `append_event`.
    /// Global retention scans all sessions only at startup, after a session
    /// set change, or after the bounded output budget is consumed, so this
    /// measures that design against journals of both sizes.
    ///
    /// Run: cargo test -p devboule-daemon --lib retain_append_cost -- --ignored --nocapture
    #[test]
    #[ignore = "measurement, not an assertion; run by hand with --nocapture"]
    fn retain_append_cost_scales_with_session_count() {
        for sessions in [50usize, 10_000usize] {
            let (dir, path) = tmp_journal();
            let journal = Journal::open_with_limits(
                &path,
                JournalLimits {
                    max_sessions: 20_000,
                    ..snapshot_limits()
                },
            )
            .expect("open");
            for index in 0..sessions {
                let mut record = sample_session(&format!("s.bulk.{index}"));
                record.status = PersistStatus::Ended;
                journal.upsert_blocking(record).expect("upsert");
            }
            journal
                .upsert_blocking(sample_session("s.hot"))
                .expect("upsert hot");

            const APPENDS: u64 = 200;
            let started = std::time::Instant::now();
            for seq in 1..=APPENDS {
                journal
                    .append_blocking(output_record("s.hot", 1, seq, b"payload"))
                    .expect("append");
            }
            let elapsed = started.elapsed();
            println!(
                "RETAIN_APPEND_COST sessions={sessions} appends={APPENDS} elapsed_ms={} per_append_us={} appends_per_s={:.0}",
                elapsed.as_millis(),
                elapsed.as_micros() / u128::from(APPENDS),
                f64::from(APPENDS as u32) / elapsed.as_secs_f64(),
            );
            journal.shutdown();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn zero_age_limit_never_expires_old_sessions() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_age_ms: 0,
                ..snapshot_limits()
            },
        )
        .expect("open");
        let mut record = sample_session("s.age.never");
        record.status = PersistStatus::Ended;
        record.updated_at_ms = 1;
        journal.upsert_blocking(record).expect("upsert");

        for (seq, timestamp) in [(1, 10), (2, 20), (3, 30), (4, 40)] {
            journal
                .append_blocking(output_record(
                    "s.age.never",
                    1,
                    seq,
                    format!("still here {timestamp}"),
                ))
                .expect("append");
            assert!(journal
                .list()
                .expect("list")
                .iter()
                .any(|session| session.id == "s.age.never"));
        }
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_get_reports_default_and_user_sources() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(&path, snapshot_limits()).expect("open");
        let defaults = journal.retention_get().expect("defaults");
        assert_eq!(
            defaults.max_age_ms.source,
            devboule_protocol::RetentionSource::Default
        );
        assert_eq!(defaults.max_age_ms.value, 0);
        journal
            .retention_set(devboule_protocol::RetentionPatch {
                max_age_ms: Some(0),
                max_bytes: Some(0),
                max_sessions: Some(0),
                session_max_bytes: Some(0),
            })
            .expect("set");
        let updated = journal.retention_get().expect("updated");
        assert_eq!(
            updated.max_age_ms.source,
            devboule_protocol::RetentionSource::User
        );
        assert_eq!(updated.max_age_ms.value, 0);
        assert_eq!(updated.max_bytes.value, 0);
        assert_eq!(updated.max_sessions.value, 0);
        assert_eq!(updated.session_max_bytes.value, 0);
        assert_eq!(
            updated.max_bytes.source,
            devboule_protocol::RetentionSource::User
        );
        assert_eq!(
            updated.max_sessions.source,
            devboule_protocol::RetentionSource::User
        );
        assert_eq!(
            updated.session_max_bytes.source,
            devboule_protocol::RetentionSource::User
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_persists_nonzero_limits_and_enforces_session_cap() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let updated = journal
            .retention_set(devboule_protocol::RetentionPatch {
                max_age_ms: Some(9_000_000_000_000),
                max_bytes: Some(20_000),
                max_sessions: Some(7),
                session_max_bytes: Some(100),
            })
            .expect("set nonzero limits");
        assert_eq!(updated.max_age_ms.value, 9_000_000_000_000);
        assert_eq!(updated.max_bytes.value, 20_000);
        assert_eq!(updated.max_sessions.value, 7);
        assert_eq!(updated.session_max_bytes.value, 100);
        assert_eq!(
            updated.max_age_ms.source,
            devboule_protocol::RetentionSource::User
        );
        assert_eq!(
            updated.max_bytes.source,
            devboule_protocol::RetentionSource::User
        );
        assert_eq!(
            updated.max_sessions.source,
            devboule_protocol::RetentionSource::User
        );
        assert_eq!(
            updated.session_max_bytes.source,
            devboule_protocol::RetentionSource::User
        );

        journal
            .retention_set(devboule_protocol::RetentionPatch {
                max_age_ms: None,
                max_bytes: None,
                max_sessions: Some(1),
                session_max_bytes: None,
            })
            .expect("set session cap");
        let mut old = sample_session("s.limit.enforce.old");
        old.status = PersistStatus::Ended;
        old.updated_at_ms = 1;
        let mut current = sample_session("s.limit.enforce.current");
        current.status = PersistStatus::Ended;
        current.updated_at_ms = 2;
        journal.upsert_blocking(old).expect("old row");
        journal.upsert_blocking(current).expect("current row");
        journal
            .append_blocking(output_record("s.limit.enforce.current", 1, 1, b"current"))
            .expect("trigger retention");
        let rows = journal.list().expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "s.limit.enforce.current");
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_set_rejects_each_negative_limit() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        for patch in [
            devboule_protocol::RetentionPatch {
                max_age_ms: Some(-1),
                max_bytes: None,
                max_sessions: None,
                session_max_bytes: None,
            },
            devboule_protocol::RetentionPatch {
                max_age_ms: None,
                max_bytes: Some(-1),
                max_sessions: None,
                session_max_bytes: None,
            },
            devboule_protocol::RetentionPatch {
                max_age_ms: None,
                max_bytes: None,
                max_sessions: Some(-1),
                session_max_bytes: None,
            },
            devboule_protocol::RetentionPatch {
                max_age_ms: None,
                max_bytes: None,
                max_sessions: None,
                session_max_bytes: Some(-1),
            },
            devboule_protocol::RetentionPatch {
                max_age_ms: None,
                max_bytes: None,
                max_sessions: None,
                session_max_bytes: None,
            },
        ] {
            let error = journal.retention_set(patch).expect_err("negative rejected");
            assert!(matches!(error, JournalError::InvalidRequest(_)));
        }
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_set_rejects_session_limit_above_byte_limit() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let error = journal
            .retention_set(devboule_protocol::RetentionPatch {
                max_age_ms: None,
                max_bytes: Some(10),
                max_sessions: None,
                session_max_bytes: Some(11),
            })
            .expect_err("inconsistent limits rejected");
        assert!(matches!(error, JournalError::InvalidRequest(_)));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn acp_sessions_are_exempt_from_age_retention() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_age_ms: 1,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut acp = new_session_record("s.acp.age", "owner", None, SessionKind::Acp, "Agent");
        acp.status = PersistStatus::Ended;
        acp.updated_at_ms = 1;
        journal.upsert_blocking(acp).expect("acp row");
        journal
            .upsert_blocking(sample_session("s.age.trigger"))
            .expect("trigger row");
        journal
            .append_blocking(output_record("s.age.trigger", 1, 1, b"trigger"))
            .expect("append");

        assert!(journal
            .list()
            .expect("list")
            .iter()
            .any(|row| row.id == "s.acp.age"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn acp_sessions_are_exempt_from_session_quota_victims() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_sessions: 1,
                max_bytes: 0,
                max_age_ms: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut acp = new_session_record("s.acp.quota", "owner", None, SessionKind::Acp, "Agent");
        acp.status = PersistStatus::Ended;
        acp.updated_at_ms = 1;
        let mut terminal = sample_session("s.terminal.quota");
        terminal.status = PersistStatus::Ended;
        terminal.updated_at_ms = 2;
        journal.upsert_blocking(acp).expect("acp row");
        journal.upsert_blocking(terminal).expect("terminal row");
        journal
            .append_blocking(output_record("s.terminal.quota", 1, 1, b"terminal"))
            .expect("append");

        let rows = journal.list().expect("list");
        assert!(rows.iter().any(|row| row.id == "s.acp.quota"));
        assert!(!rows.iter().any(|row| row.id == "s.terminal.quota"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_exempt_sessions_report_unreclaimable_bytes() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_bytes: 4,
                max_sessions: 0,
                max_age_ms: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut acp = new_session_record("s.acp.bytes", "owner", None, SessionKind::Acp, "Agent");
        acp.status = PersistStatus::Ended;
        journal.upsert_blocking(acp).expect("acp row");
        journal
            .append_blocking(output_record("s.acp.bytes", 1, 1, b"1234567890"))
            .expect("append");

        let usage = journal.usage().expect("usage");
        assert_eq!(usage.total_bytes, 10);
        assert_eq!(usage.unreclaimable.bytes_over, 6);
        assert_eq!(usage.unreclaimable.sessions_over, 0);
        assert_eq!(usage.unreclaimable.aged_out, 0);
        assert_eq!(usage.session_count, 1);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreclaimable_bytes_reports_only_exempt_byte_overage() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_bytes: 4,
                max_sessions: 0,
                max_age_ms: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut acp = new_session_record(
            "s.unreclaimable.bytes",
            "owner",
            None,
            SessionKind::Acp,
            "Agent",
        );
        acp.status = PersistStatus::Ended;
        journal.upsert_blocking(acp).expect("acp row");
        journal
            .append_blocking(output_record("s.unreclaimable.bytes", 1, 1, b"1234567890"))
            .expect("append");

        let unreclaimable = journal.usage().expect("usage").unreclaimable;
        assert_eq!(unreclaimable.bytes_over, 6);
        assert_eq!(unreclaimable.sessions_over, 0);
        assert_eq!(unreclaimable.aged_out, 0);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreclaimable_sessions_reports_only_exempt_session_overage() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_bytes: 0,
                max_sessions: 1,
                max_age_ms: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        for id in ["s.unreclaimable.sessions.1", "s.unreclaimable.sessions.2"] {
            let mut acp = new_session_record(id, "owner", None, SessionKind::Acp, "Agent");
            acp.status = PersistStatus::Ended;
            journal.upsert_blocking(acp).expect("acp row");
        }
        journal
            .append_blocking(output_record("s.unreclaimable.sessions.1", 1, 1, b""))
            .expect("append");

        let unreclaimable = journal.usage().expect("usage").unreclaimable;
        assert_eq!(unreclaimable.bytes_over, 0);
        assert_eq!(unreclaimable.sessions_over, 1);
        assert_eq!(unreclaimable.aged_out, 0);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreclaimable_age_reports_only_exempt_aged_sessions() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_bytes: 0,
                max_sessions: 0,
                max_age_ms: 60_000,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut old = new_session_record(
            "s.unreclaimable.age.old",
            "owner",
            None,
            SessionKind::Acp,
            "Agent",
        );
        old.status = PersistStatus::Ended;
        old.updated_at_ms = 1;
        journal.upsert_blocking(old).expect("old acp row");
        let now = now_ms();
        let mut current = new_session_record(
            "s.unreclaimable.age.current",
            "owner",
            None,
            SessionKind::Acp,
            "Agent",
        );
        current.status = PersistStatus::Ended;
        current.updated_at_ms = now;
        journal.upsert_blocking(current).expect("current acp row");
        journal
            .append_blocking(output_record("s.unreclaimable.age.current", 1, 1, b""))
            .expect("append");

        let unreclaimable = journal.usage().expect("usage").unreclaimable;
        assert_eq!(unreclaimable.bytes_over, 0);
        assert_eq!(unreclaimable.sessions_over, 0);
        assert_eq!(unreclaimable.aged_out, 1);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quota_trim_writes_a_limit_tombstone_atomically() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_sessions: 1,
                max_bytes: 0,
                max_age_ms: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut victim = sample_session("s.tombstone.victim");
        victim.status = PersistStatus::Ended;
        victim.updated_at_ms = 1;
        let mut survivor = sample_session("s.tombstone.survivor");
        survivor.status = PersistStatus::Ended;
        survivor.updated_at_ms = 2;
        journal.upsert_blocking(victim).expect("victim row");
        journal.upsert_blocking(survivor).expect("survivor row");
        journal
            .append_blocking(output_record("s.tombstone.survivor", 1, 1, b"survivor"))
            .expect("append");
        journal.flush().expect("flush");

        let conn = Connection::open(&path).expect("inspect");
        let reason: String = conn
            .query_row(
                "SELECT reason FROM deleted_sessions WHERE id = 's.tombstone.victim'",
                [],
                |row| row.get(0),
            )
            .expect("limit tombstone");
        assert_eq!(reason, "limit_sessions");
        assert!(journal
            .list()
            .expect("list")
            .iter()
            .all(|row| row.id != "s.tombstone.victim"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_delete_transaction_keeps_session_without_tombstone() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let mut record = sample_session("s.tombstone.rollback");
        record.status = PersistStatus::Ended;
        journal.upsert_blocking(record).expect("row");
        journal.shutdown();

        let conn = Connection::open(&path).expect("inspect");
        conn.execute_batch(
            "CREATE TRIGGER abort_tombstone BEFORE INSERT ON deleted_sessions
             BEGIN SELECT RAISE(ABORT, 'simulated crash'); END;",
        )
        .expect("crash trigger");
        drop(conn);

        let journal = Journal::open(&path).expect("reopen");
        assert!(journal.delete_session("s.tombstone.rollback").is_err());
        assert!(journal
            .list()
            .expect("row after rollback")
            .iter()
            .any(|row| row.id == "s.tombstone.rollback"));
        journal.shutdown();
        let conn = Connection::open(&path).expect("inspect after rollback");
        let tombstones: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM deleted_sessions WHERE id = 's.tombstone.rollback'",
                [],
                |row| row.get(0),
            )
            .expect("tombstones after rollback");
        assert_eq!(tombstones, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_delete_refuses_live_session_and_keeps_the_row() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.live.delete"))
            .expect("live row");

        assert!(matches!(
            journal.delete_session("s.live.delete"),
            Err(JournalError::LiveSession)
        ));
        assert!(journal
            .list()
            .expect("list")
            .iter()
            .any(|row| row.id == "s.live.delete"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_to_deleted_session_leaves_no_orphan_rows() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let mut record = sample_session("s.deleted.append");
        record.status = PersistStatus::Ended;
        journal.upsert_blocking(record).expect("row");
        journal
            .delete_session("s.deleted.append")
            .expect("delete session");
        journal
            .append_blocking(output_record("s.deleted.append", 1, 1, b"late"))
            .expect("append command");
        journal.shutdown();

        let conn = Connection::open(&path).expect("inspect");
        let mut retention_state = RetentionState::default();
        let error = append_event(
            &conn,
            &output_record("s.deleted.append", 1, 1, b"late"),
            &HashSet::new(),
            JournalLimits::default(),
            &mut retention_state,
        )
        .expect_err("deleted session must reject a late event");
        assert!(matches!(error, JournalError::SessionNotFound));
        for table in ["events", "snapshots", "sessions"] {
            let query = if table == "sessions" {
                "SELECT COUNT(*) FROM sessions WHERE id = ?1".to_string()
            } else {
                format!("SELECT COUNT(*) FROM {table} WHERE session_id = ?1")
            };
            let count: i64 = conn
                .query_row(&query, ["s.deleted.append"], |row| row.get(0))
                .expect("count rows");
            assert_eq!(count, 0, "orphan rows survived in {table}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_and_limit_deletions_keep_distinct_reasons() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_sessions: 1,
                max_bytes: 0,
                max_age_ms: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut victim = sample_session("s.reason.limit");
        victim.status = PersistStatus::Ended;
        victim.updated_at_ms = 1;
        let mut user = sample_session("s.reason.user");
        user.status = PersistStatus::Ended;
        user.updated_at_ms = 2;
        journal.upsert_blocking(victim).expect("limit row");
        journal.upsert_blocking(user).expect("user row");
        journal
            .append_blocking(output_record("s.reason.user", 1, 1, b"user"))
            .expect("quota append");
        journal
            .delete_session("s.reason.user")
            .expect("user delete");
        journal.shutdown();

        let conn = Connection::open(&path).expect("inspect");
        let mut stmt = conn
            .prepare("SELECT id, reason FROM deleted_sessions ORDER BY id")
            .expect("reasons");
        let reasons: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("reason rows")
            .collect::<Result<_, _>>()
            .expect("reason values");
        assert_eq!(
            reasons,
            vec![
                ("s.reason.limit".into(), "limit_sessions".into()),
                ("s.reason.user".into(), "user".into())
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn age_and_byte_limits_use_distinct_tombstone_reasons() {
        let (age_dir, age_path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &age_path,
            JournalLimits {
                max_age_ms: 1,
                max_bytes: 0,
                max_sessions: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open age journal");
        let mut old = sample_session("s.reason.age");
        old.status = PersistStatus::Ended;
        old.updated_at_ms = 1;
        journal.upsert_blocking(old).expect("old row");
        journal
            .upsert_blocking(sample_session("s.reason.age.trigger"))
            .expect("trigger");
        journal
            .append_blocking(output_record("s.reason.age.trigger", 1, 1, b"trigger"))
            .expect("age append");
        journal.shutdown();
        let conn = Connection::open(&age_path).expect("age inspect");
        let age_reason: String = conn
            .query_row(
                "SELECT reason FROM deleted_sessions WHERE id = 's.reason.age'",
                [],
                |row| row.get(0),
            )
            .expect("age reason");
        assert_eq!(age_reason, "limit_age");
        drop(conn);

        let (bytes_dir, bytes_path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &bytes_path,
            JournalLimits {
                max_bytes: 4,
                max_age_ms: 0,
                max_sessions: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open byte journal");
        let mut old = sample_session("s.reason.bytes");
        old.status = PersistStatus::Ended;
        old.payload_bytes = 8;
        journal.upsert_blocking(old).expect("byte row");
        journal
            .upsert_blocking(sample_session("s.reason.bytes.trigger"))
            .expect("trigger");
        journal
            .append_blocking(output_record("s.reason.bytes.trigger", 1, 1, b"trigger"))
            .expect("byte append");
        journal.shutdown();
        let conn = Connection::open(&bytes_path).expect("byte inspect");
        let byte_reason: String = conn
            .query_row(
                "SELECT reason FROM deleted_sessions WHERE id = 's.reason.bytes'",
                [],
                |row| row.get(0),
            )
            .expect("byte reason");
        assert_eq!(byte_reason, "limit_bytes");
        let _ = std::fs::remove_dir_all(&age_dir);
        let _ = std::fs::remove_dir_all(&bytes_dir);
    }

    #[test]
    fn head_trim_counts_exact_removed_payload_bytes() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                session_max_bytes: 5,
                max_bytes: 0,
                max_sessions: 0,
                max_age_ms: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        journal
            .upsert_blocking(sample_session("s.trim.exact"))
            .expect("row");
        journal
            .append_blocking(output_record("s.trim.exact", 1, 1, b"1234"))
            .expect("first output");
        journal
            .append_blocking(output_record("s.trim.exact", 1, 2, b"5678"))
            .expect("second output");

        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.trim.exact")
            .expect("session");
        assert_eq!(row.payload_bytes, 4);
        assert_eq!(row.trimmed_bytes, 4);
        let replay = journal.replay("s.trim.exact", 0).expect("replay");
        assert_eq!(
            replay.integrity,
            TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 4,
            }
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pinned_session_is_head_trimmed_but_not_deleted() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                session_max_bytes: 5,
                max_bytes: 0,
                max_sessions: 0,
                max_age_ms: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        journal
            .upsert_blocking(sample_session("s.trim.pinned"))
            .expect("row");
        journal.pin("s.trim.pinned").expect("pin");
        journal
            .append_blocking(output_record("s.trim.pinned", 1, 1, b"1234"))
            .expect("first output");
        journal
            .append_blocking(output_record("s.trim.pinned", 1, 2, b"5678"))
            .expect("second output");

        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.trim.pinned")
            .expect("session");
        assert_eq!(row.payload_bytes, 4);
        assert_eq!(row.trimmed_bytes, 4);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_ended_head_trim_is_truncated_not_complete() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                session_max_bytes: 5,
                max_bytes: 0,
                max_sessions: 0,
                max_age_ms: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        journal
            .upsert_blocking(sample_session("s.trim.ended"))
            .expect("row");
        journal
            .append_blocking(output_record("s.trim.ended", 1, 1, b"1234"))
            .expect("first output");
        journal
            .append_blocking(output_record("s.trim.ended", 1, 2, b"5678"))
            .expect("second output");
        journal
            .mark_ended_blocking("s.trim.ended", 1, None)
            .expect("end");

        assert_eq!(
            journal.replay("s.trim.ended", 0).expect("replay").integrity,
            TranscriptIntegrity::Truncated {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 4,
            }
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn byte_limit_wins_the_global_quota_tie_break() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                max_bytes: 4,
                max_sessions: 1,
                max_age_ms: 0,
                session_max_bytes: 0,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut victim = sample_session("s.tie.victim");
        victim.status = PersistStatus::Ended;
        victim.updated_at_ms = 1;
        victim.payload_bytes = 8;
        let mut survivor = sample_session("s.tie.survivor");
        survivor.status = PersistStatus::Ended;
        survivor.updated_at_ms = 2;
        journal.upsert_blocking(victim).expect("victim row");
        journal.upsert_blocking(survivor).expect("survivor row");
        journal
            .append_blocking(output_record("s.tie.survivor", 1, 1, b""))
            .expect("trigger append");
        journal.shutdown();

        let conn = Connection::open(&path).expect("inspect");
        let reason: String = conn
            .query_row(
                "SELECT reason FROM deleted_sessions WHERE id = 's.tie.victim'",
                [],
                |row| row.get(0),
            )
            .expect("tie tombstone");
        assert_eq!(reason, "limit_bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_drops_oldest_unpinned_and_skips_pinned() {
        let (dir, path) = tmp_journal();
        {
            let journal = Journal::open_with_limits(&path, tiny_limits()).expect("open");
            for (id, body) in [
                ("s.a.1", "alpha-payload-xxxx"),
                ("s.a.2", "bravo-payload-xxxx"),
            ] {
                journal.upsert_blocking(sample_session(id)).expect("upsert");
                journal
                    .append_blocking(output_record(id, 1, 1, body.as_bytes()))
                    .expect("append");
            }
        }
        // Reopen so leftover live rows become interrupted (the daemon-kill path).
        let journal = Journal::open_with_limits(&path, tiny_limits()).expect("reopen");
        journal.pin("s.a.2").expect("pin");
        journal
            .upsert_blocking(sample_session("s.a.3"))
            .expect("third");
        journal
            .append_blocking(output_record("s.a.3", 1, 1, b"charlie-payload-xxxx"))
            .expect("third body");
        let listed: Vec<String> = journal
            .list()
            .expect("list")
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert!(
            listed.contains(&"s.a.2".to_string()),
            "pinned session was trimmed: {listed:?}"
        );
        assert!(
            !listed.contains(&"s.a.1".to_string()),
            "retention should have dropped the oldest unpinned session: {listed:?}"
        );
        let replay = journal.replay("s.a.2", 0).expect("pinned replay");
        assert!(replay.events.iter().any(|event| matches!(
            event,
            SessionEvent::Output { data, .. } if data.contains("bravo")
        )));
        journal.unpin("s.a.2");
        journal.flush().expect("flush");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

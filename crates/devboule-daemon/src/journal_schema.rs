use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};

use super::{JournalError, JOURNAL_SCHEMA_VERSION};

pub(super) fn open_connection(path: &Path) -> Result<Connection, JournalError> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // NORMAL for the 13 MB/s-class append path. Durability of process
    // end is the checkpoint in mark_ended / Flush, not a fsync per frame.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > JOURNAL_SCHEMA_VERSION {
        return Err(JournalError::FutureSchema {
            found: version,
            supported: JOURNAL_SCHEMA_VERSION,
        });
    }
    if version > 0 {
        let check: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if check != "ok" {
            return Err(JournalError::Corrupt(check));
        }
    }
    if version < JOURNAL_SCHEMA_VERSION {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_SQL)?;
        if version < 2 {
            if !session_has_column(&tx, "dropped_frames")? {
                tx.execute(
                    "ALTER TABLE sessions ADD COLUMN dropped_frames INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            if !session_has_column(&tx, "dropped_bytes")? {
                tx.execute(
                    "ALTER TABLE sessions ADD COLUMN dropped_bytes INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
        }
        if version < 3 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS journal_settings (
                    key TEXT PRIMARY KEY,
                    value INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS deleted_sessions (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT,
                    kind TEXT NOT NULL,
                    title TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    deleted_at_ms INTEGER NOT NULL,
                    reason TEXT NOT NULL,
                    bytes_removed INTEGER NOT NULL
                 );",
            )?;
            if !session_has_column(&tx, "trimmed_bytes")? {
                tx.execute(
                    "ALTER TABLE sessions ADD COLUMN trimmed_bytes INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
        }
        if version < 4 && !session_has_column(&tx, "peer_session_id")? {
            tx.execute("ALTER TABLE sessions ADD COLUMN peer_session_id TEXT", [])?;
        }
        if version < 5 && !session_has_column(&tx, "provider")? {
            tx.execute("ALTER TABLE sessions ADD COLUMN provider TEXT", [])?;
        }
        if version < 6 {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS projects (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL UNIQUE,
                    git_state TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS workspaces (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    isolation TEXT NOT NULL,
                    path TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS workspaces_project
                    ON workspaces(project_id, updated_at_ms, id);",
            )?;
        }
        if version < 7 {
            let workspaces_exist: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'workspaces'",
                [],
                |row| row.get(0),
            )?;
            if workspaces_exist == 1 && !workspace_has_column(&tx, "branch")? {
                tx.execute("ALTER TABLE workspaces ADD COLUMN branch TEXT", [])?;
            }
        }
        if version < 8 {
            tx.execute_batch(PEERS_AUDIT_SQL)?;
        }
        if version < 9 {
            // The session origin (§8 R2/§8b A3). Three columns rather than one
            // JSON blob: they are read by the ownership check on every request
            // and written once, so the flat form is the one the query planner
            // and a human in `sqlite3` can both use. `DEFAULT 'local'` is the
            // rule for every row that predates the concept: a session that
            // existed before devices could pair was this person's.
            if !session_has_column(&tx, "origin_kind")? {
                tx.execute(
                    "ALTER TABLE sessions ADD COLUMN origin_kind TEXT NOT NULL DEFAULT 'local'",
                    [],
                )?;
            }
            if !session_has_column(&tx, "origin_device")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN origin_device TEXT", [])?;
            }
            if !session_has_column(&tx, "origin_role")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN origin_role TEXT", [])?;
            }
            // The schema half is not enough: `origin` is required on the wire,
            // so a permission payload stored before the field existed would be
            // dropped by hydration and flagged by the live replay. The old data
            // is made valid here, in this same transaction.
            let (rewritten, unreadable) = backfill_permission_origins(&tx)?;
            if rewritten > 0 || unreadable > 0 {
                // Counts only: no payload, no path, no prompt text.
                eprintln!(
                    "journal v9 migration gave {rewritten} stored permission payloads a local \
                     origin and left {unreadable} unreadable payloads alone"
                );
            }
        }
        tx.pragma_update(None, "user_version", JOURNAL_SCHEMA_VERSION)?;
        tx.commit()?;
    }
    validate_v6_schema(&conn)?;
    // A crash inside `sweep_audit` between dropping the triggers and
    // recreating them leaves the audit table writable, so the guarantee is
    // re-established on every open rather than trusted from the migration.
    ensure_audit_triggers(&conn)?;
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN reaped INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // The age floor alone is a disk sink: enforce the cap at every start too.
    // A failure here must not be silent: the operator would otherwise see a
    // clean open and an audit table that grows without bound. The line names
    // the stage and the error only, never a path or a secret.
    if let Err(error) = sweep_audit(&conn, unix_millis()) {
        eprintln!("daemon audit sweep at journal open failed: {error}");
    }
    // Reaped-but-still-live: the process was observed to exit, then the
    // daemon died during ConPTY drain. That is Ended (we saw the child),
    // not Recovered (we did not lose the process unobserved).
    conn.execute(
        "UPDATE sessions SET status = 'ended' WHERE status = 'live' AND reaped = 1",
        [],
    )?;
    conn.execute(
        "UPDATE sessions SET status = 'interrupted' WHERE status = 'live'",
        [],
    )?;
    Ok(conn)
}

fn session_has_column(conn: &Connection, column: &str) -> Result<bool, JournalError> {
    table_has_column(conn, "sessions", column)
}

/// Give every stored permission payload written before v9 the `local` origin it
/// was written without, and return `(rewritten, unreadable)`.
///
/// Only `events` rows of kind `agent_report` hold a serialized `SessionEvent`
/// (`journal.rs::agent_report_record`), so those are the rows both replay paths
/// deserialize. Bounded per row: one payload is parsed to decide, then re-read,
/// rewritten and written back one at a time — no pass over the whole table in
/// memory, and the payload a row carries never leaves its own iteration.
///
/// The row's `crc32` is recomputed with the payload, because both replay paths
/// verify that checksum (`journal_replay.rs` returns `JournalError::Checksum`
/// on a mismatch) and a rewritten payload under the old checksum would be new
/// corruption. A row whose bytes are not JSON at all is left exactly as it is
/// and counted: the replay paths already skip it, and inventing content for a
/// damaged row would hide the damage.
fn backfill_permission_origins(tx: &Connection) -> Result<(usize, usize), JournalError> {
    let (candidates, unreadable) = {
        let mut statement = tx.prepare(
            "SELECT session_id, generation, seq, payload FROM events WHERE kind = 'agent_report'",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        let mut candidates = Vec::new();
        let mut unreadable = 0usize;
        for row in rows {
            let (session_id, generation, seq, payload) = row?;
            match super::payload_with_origin(&payload) {
                super::OriginBackfill::Rewritten(_) => {
                    candidates.push((session_id, generation, seq))
                }
                // Already carries an origin, or is not a permission request.
                super::OriginBackfill::Nothing => {}
                super::OriginBackfill::Unreadable => unreadable += 1,
            }
        }
        (candidates, unreadable)
    };

    let mut rewritten = 0usize;
    for (session_id, generation, seq) in &candidates {
        let payload: Vec<u8> = tx.query_row(
            "SELECT payload FROM events
             WHERE session_id = ?1 AND generation = ?2 AND seq = ?3",
            rusqlite::params![session_id, generation, seq],
            |row| row.get(0),
        )?;
        let super::OriginBackfill::Rewritten(payload) = super::payload_with_origin(&payload) else {
            // A row that stopped being a pre-origin permission request between
            // the two passes is left alone rather than written blind.
            continue;
        };
        let checksum = super::crc32(&payload) as i64;
        tx.execute(
            "UPDATE events SET payload = ?1, checksum = ?2
             WHERE session_id = ?3 AND generation = ?4 AND seq = ?5",
            rusqlite::params![payload, checksum, session_id, generation, seq],
        )?;
        rewritten += 1;
    }
    Ok((rewritten, unreadable))
}

fn workspace_has_column(conn: &Connection, column: &str) -> Result<bool, JournalError> {
    table_has_column(conn, "workspaces", column)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, JournalError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_v6_schema(conn: &Connection) -> Result<(), JournalError> {
    validate_table_shape(
        conn,
        "projects",
        &[
            ("id", "TEXT", 0, 1),
            ("name", "TEXT", 1, 0),
            ("path", "TEXT", 1, 0),
            ("git_state", "TEXT", 1, 0),
            ("created_at_ms", "INTEGER", 1, 0),
            ("updated_at_ms", "INTEGER", 1, 0),
        ],
    )?;
    validate_table_shape(
        conn,
        "workspaces",
        &[
            ("id", "TEXT", 0, 1),
            ("project_id", "TEXT", 1, 0),
            ("title", "TEXT", 1, 0),
            ("isolation", "TEXT", 1, 0),
            ("path", "TEXT", 1, 0),
            ("created_at_ms", "INTEGER", 1, 0),
            ("updated_at_ms", "INTEGER", 1, 0),
            ("branch", "TEXT", 0, 0),
        ],
    )?;
    validate_table_shape(
        conn,
        "peers",
        &[
            ("device_id", "TEXT", 0, 1),
            ("display_name", "TEXT", 1, 0),
            ("role", "TEXT", 1, 0),
            ("public_key", "BLOB", 1, 0),
            ("paired_by_user", "TEXT", 0, 0),
            ("binding_kind", "TEXT", 1, 0),
            ("binding_stable_id", "TEXT", 0, 0),
            ("binding_node_name", "TEXT", 0, 0),
            ("binding_login_name", "TEXT", 0, 0),
            ("address", "TEXT", 1, 0),
            ("paired_at", "INTEGER", 1, 0),
            ("revoked_at", "INTEGER", 0, 0),
            ("caps", "TEXT", 1, 0),
        ],
    )?;
    validate_table_shape(
        conn,
        "audit",
        &[
            ("id", "INTEGER", 0, 1),
            ("at", "INTEGER", 1, 0),
            ("device_id", "TEXT", 1, 0),
            ("role", "TEXT", 1, 0),
            ("claimed_origin", "TEXT", 0, 0),
            ("action", "TEXT", 1, 0),
            ("session_id", "TEXT", 0, 0),
            ("outcome", "TEXT", 1, 0),
        ],
    )?;
    let project_index: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master
          WHERE type = 'index' AND name = 'workspaces_project'",
        [],
        |row| row.get(0),
    )?;
    if project_index != 1 {
        return Err(JournalError::Corrupt(
            "journal schema has an unexpected workspaces_project index".to_string(),
        ));
    }
    Ok(())
}

fn validate_table_shape(
    conn: &Connection,
    table: &str,
    expected: &[(&str, &str, i32, i32)],
) -> Result<(), JournalError> {
    let object_type: Option<String> = conn
        .query_row(
            "SELECT type FROM sqlite_master WHERE name = ?1",
            [table],
            |row| row.get(0),
        )
        .optional()?;
    if object_type.as_deref() != Some("table") {
        return Err(JournalError::Corrupt(format!(
            "journal schema has an unexpected {table} table"
        )));
    }

    let pragma = match table {
        "projects" => "PRAGMA table_info('projects')",
        "workspaces" => "PRAGMA table_info('workspaces')",
        "peers" => "PRAGMA table_info('peers')",
        "audit" => "PRAGMA table_info('audit')",
        _ => unreachable!("schema table is fixed above"),
    };
    let mut statement = conn.prepare(pragma)?;
    let actual = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, i32>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let required_columns_present = expected.iter().all(|expected| {
        actual.iter().any(|actual| {
            actual.0 == expected.0
                && actual.1.eq_ignore_ascii_case(expected.1)
                && actual.2 == expected.2
                && actual.3 == expected.3
        })
    });
    if !required_columns_present {
        return Err(JournalError::Corrupt(format!(
            "journal schema has an unexpected {table} table"
        )));
    }
    Ok(())
}

/// v8: the paired peers and the append-only audit trail. `caps` is a JSON
/// array in TEXT (SQLite has no array type); `paired_by_user` is the local
/// daemon's own SID at pairing time, written by this side and never received
/// from the peer.
const PEERS_AUDIT_SQL: &str = "
CREATE TABLE IF NOT EXISTS peers (
    device_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('client','daemon')),
    public_key BLOB NOT NULL,
    paired_by_user TEXT,
    binding_kind TEXT NOT NULL,
    binding_stable_id TEXT,
    binding_node_name TEXT,
    binding_login_name TEXT,
    address TEXT NOT NULL,
    paired_at INTEGER NOT NULL,
    revoked_at INTEGER,
    caps TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS audit (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    role TEXT NOT NULL,
    claimed_origin TEXT,
    action TEXT NOT NULL,
    session_id TEXT,
    outcome TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS audit_device ON audit(device_id, id);
CREATE INDEX IF NOT EXISTS audit_at ON audit(at);
CREATE TRIGGER IF NOT EXISTS audit_no_delete BEFORE DELETE ON audit
BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
CREATE TRIGGER IF NOT EXISTS audit_no_update BEFORE UPDATE ON audit
BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
";

const AUDIT_NO_DELETE_SQL: &str = "
CREATE TRIGGER IF NOT EXISTS audit_no_delete BEFORE DELETE ON audit
BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;";

const AUDIT_NO_UPDATE_SQL: &str = "
CREATE TRIGGER IF NOT EXISTS audit_no_update BEFORE UPDATE ON audit
BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;";

/// The trigger's meaning, as the fragments that must appear in its stored body.
///
/// SQLite stores a trigger's `sql` text with its own whitespace, so a byte
/// comparison against our own literal would fail on a database this daemon
/// created. What has to hold is the event, the table and the action: a body
/// that lost any of them is a tampered or neutered trigger and is replaced
/// (L2). `RAISE(IGNORE)` and an empty body both fail this.
const AUDIT_TRIGGER_REQUIRED: [&str; 3] = ["on audit", "raise(abort", "audit is append-only"];

/// `stored` is the trigger's own `sql` text; `event` is `"before delete"` or
/// `"before update"`, which is the one part that differs between the two.
fn trigger_sql_is_expected(stored: &str, event: &str) -> bool {
    let stored = stored.to_ascii_lowercase();
    stored.contains(event)
        && AUDIT_TRIGGER_REQUIRED
            .iter()
            .all(|fragment| stored.contains(fragment))
}

/// Minimum audit retention. A `const`, never a wire parameter.
pub(super) const AUDIT_FLOOR_DAYS: i64 = 90;
/// Per-device ceiling. The age floor alone is a disk sink; the cap wins over
/// the floor for that device, and the local device is subject to it too.
pub(super) const AUDIT_MAX_ROWS_PER_DEVICE: i64 = 20_000;

fn ensure_audit_triggers(conn: &Connection) -> Result<(), JournalError> {
    for (name, sql, event) in [
        ("audit_no_delete", AUDIT_NO_DELETE_SQL, "before delete"),
        ("audit_no_update", AUDIT_NO_UPDATE_SQL, "before update"),
    ] {
        // The **body** is checked, not just the name (L2). A trigger that
        // exists but whose body was replaced — by a tampered database, or by a
        // `DROP`+`CREATE` with `RAISE(IGNORE)` or no body at all — would pass a
        // name-only check while leaving the audit table deletable.
        let stored: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        let matches = stored
            .as_deref()
            .is_some_and(|stored| trigger_sql_is_expected(stored, event));
        if !matches {
            // `CREATE TRIGGER IF NOT EXISTS` would leave a wrong body in place,
            // so the old one goes first. Both statements are idempotent.
            conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {name};\n{}", sql.trim()))?;
        }
    }
    // The age floor is a range scan on `at`; without this index the hourly
    // sweep degrades to a full table scan on the writer thread.
    conn.execute_batch("CREATE INDEX IF NOT EXISTS audit_at ON audit(at);")?;
    Ok(())
}

/// The one place allowed to drop the append-only triggers: inside a single
/// transaction that deletes the aged rows, applies the per-device cap, and
/// recreates them. Returns `(deleted_by_age, deleted_by_cap)`.
///
/// Both deletes are single statements. The per-device cap used to be one
/// `DELETE ... NOT IN (...)` per distinct `device_id`, which is `O(devices)`
/// full scans and, on the writer thread, a stall proportional to the number
/// of peers; the window function does it in one pass and uses
/// `audit(device_id, id)` to partition.
pub(super) fn sweep_audit(conn: &Connection, now_ms: i64) -> Result<(u64, u64), JournalError> {
    let floor_ms = now_ms - AUDIT_FLOOR_DAYS * 24 * 60 * 60 * 1000;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "DROP TRIGGER IF EXISTS audit_no_delete;
         DROP TRIGGER IF EXISTS audit_no_update;",
    )?;
    let aged = tx.execute("DELETE FROM audit WHERE at < ?1", [floor_ms])?;
    let capped = tx.execute(
        "DELETE FROM audit
          WHERE id IN (
              SELECT id FROM (
                  SELECT id, ROW_NUMBER() OVER (
                      PARTITION BY device_id ORDER BY id DESC
                  ) AS rank
                  FROM audit
              ) WHERE rank > ?1
          )",
        rusqlite::params![AUDIT_MAX_ROWS_PER_DEVICE],
    )?;
    tx.execute_batch(&format!("{AUDIT_NO_DELETE_SQL}{AUDIT_NO_UPDATE_SQL}"))?;
    tx.commit()?;
    Ok((aged as u64, capped as u64))
}

fn unix_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(i64::MAX)
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    workspace_id TEXT,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    status TEXT NOT NULL,
    exit_code INTEGER,
    closed INTEGER NOT NULL DEFAULT 0,
    last_seq INTEGER NOT NULL DEFAULT 0,
    degraded INTEGER NOT NULL DEFAULT 0,
    -- M6 adds dropped_frames and dropped_bytes in the versioned migration
    -- above so this SQL remains the previous schema for migration tests.
    payload_bytes INTEGER NOT NULL DEFAULT 0,
    unsnapshotted_bytes INTEGER NOT NULL DEFAULT 0,
    reaped INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS events (
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    ts_ms INTEGER NOT NULL,
    payload BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    PRIMARY KEY (session_id, generation, seq)
);
CREATE TABLE IF NOT EXISTS snapshots (
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    from_seq INTEGER NOT NULL,
    up_to_seq INTEGER NOT NULL,
    ts_ms INTEGER NOT NULL,
    blob BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    payload_bytes INTEGER NOT NULL,
    PRIMARY KEY (session_id, generation, up_to_seq)
);
CREATE INDEX IF NOT EXISTS events_session ON events(session_id, generation, seq);
CREATE INDEX IF NOT EXISTS snapshots_session ON snapshots(session_id, generation, up_to_seq);
CREATE INDEX IF NOT EXISTS sessions_updated ON sessions(updated_at_ms);
CREATE TABLE IF NOT EXISTS turns (
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    turn_seq INTEGER NOT NULL,
    ts_ms INTEGER NOT NULL,
    role TEXT NOT NULL,
    payload BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    PRIMARY KEY (session_id, generation, turn_seq)
);
CREATE TABLE IF NOT EXISTS permissions (
    session_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    ts_ms INTEGER NOT NULL,
    outcome TEXT,
    payload BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    PRIMARY KEY (session_id, request_id)
);
";

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use devboule_protocol::{
        PeerRole, SessionEvent, SessionOrigin, SessionOriginKind, SessionState, TranscriptIntegrity,
    };

    use super::super::{
        crc32, sample_session, tmp_journal, AuditRecord, Journal, JournalError, PeerRecord,
        JOURNAL_MAX_AGE_MS, JOURNAL_MAX_SESSIONS, JOURNAL_SCHEMA_VERSION,
    };
    use super::SCHEMA_SQL;

    /// Every column the base schema does not carry yet, up to v8. Written out
    /// as SQL rather than replayed through `open_connection`, because a real
    /// v8 file is exactly this and the migration under test must start from
    /// the version it will find on disk.
    const V8_DDL: &str = "
ALTER TABLE sessions ADD COLUMN dropped_frames INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN dropped_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN trimmed_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN peer_session_id TEXT;
ALTER TABLE sessions ADD COLUMN provider TEXT;
CREATE TABLE IF NOT EXISTS journal_settings (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS deleted_sessions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    deleted_at_ms INTEGER NOT NULL,
    reason TEXT NOT NULL,
    bytes_removed INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    git_state TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    title TEXT NOT NULL,
    isolation TEXT NOT NULL,
    path TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS workspaces_project
    ON workspaces(project_id, updated_at_ms, id);
ALTER TABLE workspaces ADD COLUMN branch TEXT;
";

    #[test]
    fn a_v8_journal_migrates_to_v9_and_every_old_row_reads_as_local() {
        let (dir, path) = tmp_journal();
        let conn = Connection::open(&path).expect("v8 journal");
        conn.execute_batch(SCHEMA_SQL).expect("base schema");
        conn.execute_batch(V8_DDL).expect("v8 columns and tables");
        conn.execute_batch(super::PEERS_AUDIT_SQL)
            .expect("v8 peers and audit tables");
        conn.execute(
            "INSERT INTO sessions (
                id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes,
                unsnapshotted_bytes, reaped, peer_session_id, provider
             ) VALUES ('s.before-origin', 'owner', NULL, 'terminal', 'Terminal', 1, 2,
                       1, 'ended', 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, NULL, NULL)",
            [],
        )
        .expect("v8 session row");
        // A pre-origin permission payload, exactly what a v8 daemon wrote: the
        // event a provider client builds, serialized before the field existed.
        let legacy_permission = serde_json::to_vec(&serde_json::json!({
            "type": "permission_request",
            "toolCallId": "call-legacy",
            "title": "Run command",
            "options": []
        }))
        .expect("legacy payload");
        // A payload that already carries an origin, and bytes that are not JSON
        // at all: the migration must leave both exactly as they are.
        let already_origin = serde_json::to_vec(&serde_json::json!({
            "type": "permission_request",
            "toolCallId": "call-peer",
            "title": "Run command",
            "options": [],
            "origin": {"kind": "peer", "deviceId": "device-phone", "role": "client"}
        }))
        .expect("peer payload");
        let damaged = b"not a session event".to_vec();
        for (seq, kind, payload) in [
            (2_i64, "agent_report", legacy_permission.clone()),
            (3_i64, "agent_report", already_origin.clone()),
            (4_i64, "agent_report", damaged.clone()),
            (5_i64, "output", b"plain terminal bytes".to_vec()),
        ] {
            conn.execute(
                "INSERT INTO events (session_id, generation, seq, kind, ts_ms, payload, checksum)
                 VALUES ('s.before-origin', 1, ?1, ?2, 1, ?3, ?4)",
                rusqlite::params![seq, kind, payload, crc32(&payload) as i64],
            )
            .expect("v8 event row");
        }
        conn.pragma_update(None, "user_version", 8)
            .expect("v8 version");
        drop(conn);

        let journal = Journal::open(&path).expect("migrate");
        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.before-origin")
            .expect("the v8 row survived");
        assert_eq!(
            row.origin,
            devboule_protocol::SessionOrigin::local(),
            "a session that predates devices pairing as this person's"
        );
        assert_eq!(row.to_session().origin.kind, SessionOriginKind::Local);

        let check = Connection::open(&path).expect("check migrated schema");
        for column in ["origin_kind", "origin_device", "origin_role"] {
            let columns: i64 = check
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?1",
                    [column],
                    |row| row.get(0),
                )
                .expect("origin column");
            assert_eq!(columns, 1, "missing migrated column {column}");
        }
        let version: i32 = check
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, JOURNAL_SCHEMA_VERSION);

        // The migration is not only schema: the old *data* has to be valid too.
        let read_payload = |seq: i64| -> (Vec<u8>, i64) {
            check
                .query_row(
                    "SELECT payload, checksum FROM events
                     WHERE session_id = 's.before-origin' AND generation = 1 AND seq = ?1",
                    [seq],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("migrated payload")
        };

        // The pre-origin permission payload now parses as the event both replay
        // paths deserialize, and it says `local` — a fact about the daemon that
        // wrote it, which had no paired devices.
        let (payload, checksum) = read_payload(2);
        match serde_json::from_slice::<SessionEvent>(&payload) {
            Ok(SessionEvent::PermissionRequest { origin, .. }) => {
                assert_eq!(origin, SessionOrigin::local())
            }
            other => panic!("a migrated permission payload must parse: {other:?}"),
        }
        assert_eq!(
            checksum,
            crc32(&payload) as i64,
            "the rewrite must recompute the row's checksum"
        );

        // A payload that already carries an origin is not this migration's
        // business, and a damaged one is left alone rather than invented. Plain
        // output rows are not even looked at.
        assert_eq!(read_payload(3).0, already_origin);
        assert_eq!(read_payload(4).0, damaged);
        assert_eq!(read_payload(5).0, b"plain terminal bytes".to_vec());

        drop(check);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The write path: a peer origin lands in the three columns and comes back
    /// out of `list()` unchanged, which is what the ownership check reads.
    #[test]
    fn a_peer_origin_survives_a_journal_round_trip() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("journal");
        let mut record = sample_session("s.peer.1");
        record.origin = devboule_protocol::SessionOrigin::peer("device-phone", PeerRole::Client);
        journal.upsert_blocking(record).expect("store");

        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.peer.1")
            .expect("peer row");
        let origin = row.to_session().origin;
        assert_eq!(origin.kind, SessionOriginKind::Peer);
        assert_eq!(origin.device_id.as_deref(), Some("device-phone"));
        assert_eq!(origin.role, Some(PeerRole::Client));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn previous_schema_migrates_and_preserves_zero_loss_amount() {
        let (dir, path) = tmp_journal();
        let conn = Connection::open(&path).expect("old journal");
        conn.execute_batch(SCHEMA_SQL).expect("old schema");
        conn.execute(
            "INSERT INTO sessions (
                id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded, payload_bytes,
                unsnapshotted_bytes, reaped
             ) VALUES ('s.old', 'owner', NULL, 'terminal', 'Terminal', 1, 1,
                       1, 'ended', NULL, 0, 0, 1, 0, 0, 0)",
            [],
        )
        .expect("old row");
        conn.pragma_update(None, "user_version", 1)
            .expect("old version");
        drop(conn);

        let journal = Journal::open(&path).expect("migrate");
        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.old")
            .expect("migrated row");
        assert_eq!(row.dropped_frames, 0);
        assert_eq!(row.dropped_bytes, 0);
        assert_eq!(row.trimmed_bytes, 0);
        assert_eq!(
            row.to_session().state,
            SessionState::Ended {
                generation: 1,
                code: None,
                integrity: TranscriptIntegrity::Truncated {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    trimmed_bytes: 0,
                },
            }
        );
        let usage = journal.usage().expect("default limits");
        assert_eq!(usage.limits.max_age_ms, JOURNAL_MAX_AGE_MS);
        assert_eq!(usage.limits.max_sessions, JOURNAL_MAX_SESSIONS);
        let check = Connection::open(&path).expect("check migrated schema");
        let settings: i64 = check
            .query_row("SELECT COUNT(*) FROM journal_settings", [], |row| {
                row.get(0)
            })
            .expect("settings table");
        assert_eq!(settings, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_recovers_if_first_counter_column_was_committed() {
        let (dir, path) = tmp_journal();
        let conn = Connection::open(&path).expect("old journal");
        conn.execute_batch(SCHEMA_SQL).expect("old schema");
        conn.pragma_update(None, "user_version", 1)
            .expect("old version");
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN dropped_frames INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .expect("simulate the first committed ALTER");
        drop(conn);

        let journal = match Journal::open(&path) {
            Ok(journal) => journal,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
                panic!("crash-atomic migration should reopen: {error}");
            }
        };
        journal
            .upsert_blocking(sample_session("s.atomic"))
            .expect("write after migration");
        assert!(journal
            .list()
            .expect("read after migration")
            .iter()
            .any(|record| record.id == "s.atomic"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_schema_migrates_peer_session_id_and_preserves_old_rows() {
        let (dir, path) = tmp_journal();
        let conn = Connection::open(&path).expect("old journal");
        conn.execute_batch(SCHEMA_SQL).expect("old schema");
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN dropped_frames INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN dropped_bytes INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN trimmed_bytes INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN peer_session_id TEXT;",
        )
        .expect("v3 schema");
        conn.execute(
            "INSERT INTO sessions (
                id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes,
                unsnapshotted_bytes, reaped
             ) VALUES ('s.old-peer', 'owner', NULL, 'acp', 'Agent', 1, 1,
                       1, 'ended', NULL, 0, 0, 0, 0, 0, 0, 0, 0, 0)",
            [],
        )
        .expect("old row");
        conn.pragma_update(None, "user_version", 4)
            .expect("old version");
        drop(conn);

        let journal = Journal::open(&path).expect("migrate");
        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.old-peer")
            .expect("migrated row");
        assert_eq!(row.peer_session_id, None);
        assert_eq!(row.provider, None);
        let check = Connection::open(&path).expect("check migrated schema");
        let columns: i64 = check
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions')
                 WHERE name = 'peer_session_id'",
                [],
                |row| row.get(0),
            )
            .expect("peer session id column");
        assert_eq!(columns, 1);
        let provider_columns: i64 = check
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions')
                 WHERE name = 'provider'",
                [],
                |row| row.get(0),
            )
            .expect("provider column");
        assert_eq!(provider_columns, 1);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v5_journal_migrates_project_tables_without_losing_existing_rows() {
        let (dir, path) = tmp_journal();
        let conn = Connection::open(&path).expect("old journal");
        conn.execute_batch(SCHEMA_SQL).expect("old schema");
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN dropped_frames INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN dropped_bytes INTEGER NOT NULL DEFAULT 0;
             CREATE TABLE IF NOT EXISTS journal_settings (
                 key TEXT PRIMARY KEY,
                 value INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS deleted_sessions (
                 id TEXT PRIMARY KEY,
                 workspace_id TEXT,
                 kind TEXT NOT NULL,
                 title TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL,
                 deleted_at_ms INTEGER NOT NULL,
                 reason TEXT NOT NULL,
                 bytes_removed INTEGER NOT NULL
             );
             ALTER TABLE sessions ADD COLUMN trimmed_bytes INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN peer_session_id TEXT;
             ALTER TABLE sessions ADD COLUMN provider TEXT;",
        )
        .expect("v5 schema");
        conn.execute(
            "INSERT INTO sessions (
                id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes,
                unsnapshotted_bytes, reaped, peer_session_id, provider
             ) VALUES ('s.before-projects', 'owner', 'ws.before-projects',
                       'terminal', 'Terminal', 1, 2, 1, 'ended', 0, 0, 1, 0,
                       0, 0, 0, 4, 0, 0, NULL, NULL)",
            [],
        )
        .expect("old session row");
        conn.execute(
            "INSERT INTO events (
                session_id, generation, seq, kind, ts_ms, payload, checksum
             ) VALUES ('s.before-projects', 1, 1, 'output', 2, X'6F6B', 36355)",
            [],
        )
        .expect("old event row");
        conn.pragma_update(None, "user_version", 5)
            .expect("v5 version");
        drop(conn);

        let journal = Journal::open(&path).expect("migrate");
        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.before-projects")
            .expect("old session survived");
        assert_eq!(row.workspace_id.as_deref(), Some("ws.before-projects"));
        assert_eq!(row.payload_bytes, 4);

        let check = Connection::open(&path).expect("check migrated schema");
        let event_count: i64 = check
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = 's.before-projects'",
                [],
                |row| row.get(0),
            )
            .expect("old event survived");
        assert_eq!(event_count, 1);
        for table in ["projects", "workspaces"] {
            let count: i64 = check
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("new table exists");
            assert_eq!(count, 1, "missing migrated table {table}");
        }
        let version: i32 = check
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, JOURNAL_SCHEMA_VERSION);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v6_journal_with_a_divergent_project_shape_is_rejected() {
        let (dir, path) = tmp_journal();
        let conn = Connection::open(&path).expect("divergent journal");
        conn.execute_batch(SCHEMA_SQL).expect("base schema");
        conn.execute_batch(
            "CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 path TEXT NOT NULL
             );
             PRAGMA user_version = 6;",
        )
        .expect("divergent v6 schema");
        drop(conn);

        match Journal::open(&path) {
            Err(JournalError::Corrupt(message)) => {
                assert!(
                    message.contains("projects"),
                    "unexpected message: {message}"
                );
            }
            Err(other) => panic!("expected divergent schema error, got {other}"),
            Ok(_) => panic!("divergent v6 schema opened"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v6_journal_with_an_additive_project_column_is_accepted() {
        let (dir, path) = tmp_journal();
        let conn = Connection::open(&path).expect("divergent journal");
        conn.execute_batch(SCHEMA_SQL).expect("base schema");
        conn.execute_batch(
            "CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 path TEXT NOT NULL UNIQUE,
                 git_state TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL,
                 extra TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE workspaces (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL,
                 title TEXT NOT NULL,
                 isolation TEXT NOT NULL,
                 path TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX workspaces_project ON workspaces(project_id, updated_at_ms, id);
             PRAGMA user_version = 6;",
        )
        .expect("additive v6 schema");
        drop(conn);

        let journal = Journal::open(&path).expect("additive column must be accepted");
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_creates_schema_and_agent_tables() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal.flush().expect("flush");
        drop(journal);
        let conn = Connection::open(&path).expect("reopen");
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, JOURNAL_SCHEMA_VERSION);
        let turns: i64 = conn
            .query_row("SELECT COUNT(*) FROM turns", [], |row| row.get(0))
            .expect("turns");
        let perms: i64 = conn
            .query_row("SELECT COUNT(*) FROM permissions", [], |row| row.get(0))
            .expect("permissions");
        assert_eq!(turns, 0);
        assert_eq!(perms, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn future_schema_is_a_clear_error() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        drop(journal);
        let conn = Connection::open(&path).expect("bump");
        conn.pragma_update(None, "user_version", 99)
            .expect("version");
        drop(conn);
        match Journal::open(&path) {
            Err(JournalError::FutureSchema {
                found: 99,
                supported: JOURNAL_SCHEMA_VERSION,
            }) => {}
            Err(other) => panic!("expected future schema, got {other}"),
            Ok(_) => panic!("future schema opened"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_a_clear_error() {
        let (dir, path) = tmp_journal();
        std::fs::write(&path, b"this is not sqlite").expect("garbage");
        let error = match Journal::open(&path) {
            Err(error) => error,
            Ok(_) => panic!("corrupt journal opened"),
        };
        assert!(
            matches!(
                error,
                JournalError::Corrupt(_) | JournalError::Unavailable(_)
            ),
            "expected corrupt/unavailable, got {error}"
        );
        let message = error.to_string();
        assert!(
            message.contains("journal"),
            "error should name the journal: {message}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_creates_new() {
        let (dir, path) = tmp_journal();
        assert!(!path.exists());
        let journal = Journal::open(&path).expect("create");
        assert!(path.exists());
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_7_journal_migrates_to_v8_with_peers_audit_and_triggers() {
        let (dir, path) = tmp_journal();
        let conn = Connection::open(&path).expect("old journal");
        conn.execute_batch(SCHEMA_SQL).expect("old schema");
        conn.execute_batch(
            "ALTER TABLE sessions ADD COLUMN dropped_frames INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN dropped_bytes INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN trimmed_bytes INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN peer_session_id TEXT;
             ALTER TABLE sessions ADD COLUMN provider TEXT;
             CREATE TABLE journal_settings (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
             CREATE TABLE deleted_sessions (
                 id TEXT PRIMARY KEY, workspace_id TEXT, kind TEXT NOT NULL, title TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL, deleted_at_ms INTEGER NOT NULL,
                 reason TEXT NOT NULL, bytes_removed INTEGER NOT NULL
             );
             CREATE TABLE projects (
                 id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE,
                 git_state TEXT NOT NULL, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
             );
             CREATE TABLE workspaces (
                 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL,
                 isolation TEXT NOT NULL, path TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL, branch TEXT
             );
             CREATE INDEX workspaces_project ON workspaces(project_id, updated_at_ms, id);",
        )
        .expect("v7 schema");
        conn.pragma_update(None, "user_version", 7).expect("v7");
        drop(conn);

        let journal = Journal::open(&path).expect("migrate");
        journal
            .peer_upsert(PeerRecord {
                device_id: "dev-migrated".to_string(),
                display_name: "Host".to_string(),
                role: "client".to_string(),
                public_key: vec![1u8; 32],
                paired_by_user: Some("S-1-5-21-1".to_string()),
                binding_kind: "tailnet".to_string(),
                binding_stable_id: Some("nMIGRATED".to_string()),
                binding_node_name: None,
                binding_login_name: None,
                address: "100.64.0.9:47831".to_string(),
                paired_at: 1,
                revoked_at: None,
                caps: devboule_protocol::PEER_DEFAULT_CAPS
                    .iter()
                    .map(|cap| cap.to_string())
                    .collect(),
            })
            .expect("peers table is usable");
        journal
            .audit_append(AuditRecord {
                device_id: "dev-migrated".to_string(),
                role: "client".to_string(),
                claimed_origin: None,
                action: "Ping".to_string(),
                session_id: None,
                outcome: "ok".to_string(),
            })
            .expect("audit table is usable");
        let sweep = journal.audit_sweep().expect("sweep");
        assert_eq!((sweep.deleted_by_age, sweep.deleted_by_cap), (0, 0));
        journal.flush().expect("flush");
        drop(journal);

        let check = Connection::open(&path).expect("check migrated schema");
        let version: i32 = check
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, JOURNAL_SCHEMA_VERSION);
        for table in ["peers", "audit"] {
            let count: i64 = check
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("new table exists");
            assert_eq!(count, 1, "missing migrated table {table}");
        }
        for trigger in ["audit_no_delete", "audit_no_update"] {
            let count: i64 = check
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    [trigger],
                    |row| row.get(0),
                )
                .expect("trigger exists");
            assert_eq!(count, 1, "missing migrated trigger {trigger}");
        }
        let index: i64 = check
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'audit_device'",
                [],
                |row| row.get(0),
            )
            .expect("audit index");
        assert_eq!(index, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// L2: the check must be about the trigger's **body**, not its name.
    ///
    /// This calls `ensure_audit_triggers` directly rather than going through
    /// `Journal::open`, because an open also runs `sweep_audit`, which drops and
    /// recreates both triggers itself — a test at that level would pass whether
    /// or not the body is compared, and would therefore prove nothing about
    /// this function.
    #[test]
    fn ensure_audit_triggers_replaces_a_neutered_body() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal.flush().expect("flush");
        journal.shutdown();

        let conn = Connection::open(&path).expect("raw");
        // Same name, same event, same table — but it no longer aborts.
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS audit_no_delete;
             CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .expect("neuter the delete trigger");
        let neutered: String = stored_trigger(&conn, "audit_no_delete");
        assert!(
            !neutered.to_ascii_lowercase().contains("raise(abort"),
            "the fixture must start neutered: {neutered}"
        );

        super::ensure_audit_triggers(&conn).expect("re-ensure");

        let restored = stored_trigger(&conn, "audit_no_delete");
        let lower = restored.to_ascii_lowercase();
        assert!(
            lower.contains("raise(abort") && lower.contains("audit is append-only"),
            "the neutered body must be replaced: {restored}"
        );
        assert!(
            lower.contains("before delete"),
            "on the same event: {restored}"
        );
        assert!(
            lower.contains("on audit"),
            "against the same table: {restored}"
        );

        // The other trigger was intact and is left alone.
        let update = stored_trigger(&conn, "audit_no_update");
        assert!(update.to_ascii_lowercase().contains("raise(abort"));

        // The restored trigger really protects again.
        conn.execute(
            "INSERT INTO audit (at, device_id, role, action, outcome) VALUES (1, 'd', 'daemon', 'Ping', 'ok')",
            [],
        )
        .expect("seed a row");
        assert!(
            conn.execute("DELETE FROM audit", []).is_err(),
            "the restored trigger must abort a delete"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A healthy database is left untouched: the comparison must not rewrite an
    /// intact trigger, or a mistake in the required fragments would be invisible
    /// because the rewrite would succeed anyway.
    #[test]
    fn ensure_audit_triggers_leaves_intact_bodies_alone() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal.flush().expect("flush");
        journal.shutdown();

        let conn = Connection::open(&path).expect("raw");
        let before = (
            stored_trigger(&conn, "audit_no_delete"),
            stored_trigger(&conn, "audit_no_update"),
        );
        super::ensure_audit_triggers(&conn).expect("re-ensure");
        let after = (
            stored_trigger(&conn, "audit_no_delete"),
            stored_trigger(&conn, "audit_no_update"),
        );
        assert_eq!(
            before, after,
            "an intact trigger body must not be rewritten"
        );

        // And the two fragments a body must carry are the ones that matter:
        // a body keeping the event and losing the abort is refused.
        assert!(super::trigger_sql_is_expected(&before.0, "before delete"));
        assert!(!super::trigger_sql_is_expected(
            "CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit BEGIN SELECT RAISE(IGNORE); END;",
            "before delete"
        ));
        assert!(!super::trigger_sql_is_expected(
            "CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;",
            "before update"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn stored_trigger(conn: &Connection, name: &str) -> String {
        conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("trigger {name} is not in sqlite_master: {error}"))
    }
}

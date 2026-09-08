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
        tx.pragma_update(None, "user_version", JOURNAL_SCHEMA_VERSION)?;
        tx.commit()?;
    }
    validate_v6_schema(&conn)?;
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN reaped INTEGER NOT NULL DEFAULT 0",
        [],
    );
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
    let project_index: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master
          WHERE type = 'index' AND name = 'workspaces_project'",
        [],
        |row| row.get(0),
    )?;
    if project_index != 1 {
        return Err(JournalError::Corrupt(
            "journal schema v6 has an unexpected workspaces_project index".to_string(),
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
            "journal schema v6 has an unexpected {table} table"
        )));
    }

    let pragma = match table {
        "projects" => "PRAGMA table_info('projects')",
        "workspaces" => "PRAGMA table_info('workspaces')",
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
            "journal schema v6 has an unexpected {table} table"
        )));
    }
    Ok(())
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

    use devboule_protocol::{SessionState, TranscriptIntegrity};

    use super::super::{
        sample_session, tmp_journal, Journal, JournalError, JOURNAL_MAX_AGE_MS,
        JOURNAL_MAX_SESSIONS, JOURNAL_SCHEMA_VERSION,
    };
    use super::SCHEMA_SQL;

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
}

//! Tests for the journal schema: migrations, column defaults and row round-trips.

use rusqlite::Connection;

use super::validate_agent_columns;

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

/// A v8 journal file: the base schema, the pre-v9 columns, the peers/audit
/// tables, one session, and exactly the event rows the caller names.
fn v8_journal_with_events(
    rows: &[(i64, &str, Vec<u8>)],
) -> (std::path::PathBuf, std::path::PathBuf) {
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
    for (seq, kind, payload) in rows {
        conn.execute(
            "INSERT INTO events (session_id, generation, seq, kind, ts_ms, payload, checksum)
                 VALUES ('s.before-origin', 1, ?1, ?2, 1, ?3, ?4)",
            rusqlite::params![seq, kind, payload, crc32(payload) as i64],
        )
        .expect("v8 event row");
    }
    conn.pragma_update(None, "user_version", 8)
        .expect("v8 version");
    drop(conn);
    (dir, path)
}

/// One `events` row as it sits on disk: `(payload, checksum)`.
fn stored_event(path: &std::path::Path, seq: i64) -> (Vec<u8>, i64) {
    Connection::open(path)
        .expect("open journal")
        .query_row(
            "SELECT payload, checksum FROM events
                 WHERE session_id = 's.before-origin' AND generation = 1 AND seq = ?1",
            [seq],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("stored event row")
}

/// A pre-origin permission request, exactly what a v8 daemon wrote.
fn legacy_permission_payload() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "type": "permission_request",
        "toolCallId": "call-legacy",
        "title": "Run command",
        "options": []
    }))
    .expect("legacy payload")
}

/// H8: a second open at v9 runs no migration and moves nothing, and a
/// payload that is a permission request by *tag* but not a complete
/// `SessionEvent` is left byte-for-byte rather than handed an origin the
/// replay path would still drop.
#[test]
fn a_second_open_rewrites_nothing_and_incomplete_events_are_left_alone() {
    let legacy = legacy_permission_payload();
    // JSON, tagged `permission_request`, missing the tool call id its
    // variant requires: this is the shape the shape-only check used to
    // rewrite.
    let incomplete = serde_json::to_vec(&serde_json::json!({
        "type": "permission_request",
        "title": 7,
        "options": []
    }))
    .expect("incomplete payload");
    let (dir, path) = v8_journal_with_events(&[
        (2, "agent_report", legacy.clone()),
        (3, "agent_report", incomplete.clone()),
    ]);

    {
        let journal = Journal::open(&path).expect("first open migrates");
        journal.shutdown();
    }
    let migrated = stored_event(&path, 2);
    let left_alone = stored_event(&path, 3);
    assert_ne!(migrated.0, legacy, "the pre-origin payload is made valid");
    assert_eq!(
        left_alone,
        (incomplete.clone(), crc32(&incomplete) as i64),
        "an incomplete event keeps its bytes and its checksum"
    );

    // Second open: `user_version` is already v9, so the migration body
    // does not run at all.
    {
        let journal = Journal::open(&path).expect("second open");
        journal.shutdown();
    }
    assert_eq!(
        stored_event(&path, 2),
        migrated,
        "a second open rewrites nothing"
    );
    assert_eq!(stored_event(&path, 3), left_alone);
    let _ = std::fs::remove_dir_all(&dir);
}

/// H8: a candidate past the read bound is not read, not rewritten and
/// counted. The row keeps its bytes and its checksum.
#[test]
fn a_payload_past_the_read_bound_is_skipped_untouched() {
    let bound = crate::journal::MAX_ORIGIN_BACKFILL_PAYLOAD_BYTES;
    // A pre-origin permission request padded past the bound by its own
    // title: exactly the shape the backfill rewrites, and exactly the
    // shape the bound is there for.
    let oversized = serde_json::to_vec(&serde_json::json!({
        "type": "permission_request",
        "toolCallId": "call-huge",
        "title": "x".repeat(bound + 1),
        "options": []
    }))
    .expect("oversized payload");
    assert!(
        oversized.len() > bound,
        "the fixture must be past the bound"
    );
    let (dir, path) = v8_journal_with_events(&[(2, "agent_report", oversized.clone())]);

    {
        let journal = Journal::open(&path).expect("migrate");
        journal.shutdown();
    }
    assert_eq!(
        stored_event(&path, 2),
        (oversized.clone(), crc32(&oversized) as i64),
        "a row past the read bound is untouched byte for byte"
    );

    // And the backfill counts it rather than silently skipping it: nothing
    // to rewrite, nothing unreadable, one row past the bound.
    let conn = Connection::open(&path).expect("open migrated journal");
    let tx = conn.unchecked_transaction().expect("transaction");
    assert_eq!(
        super::backfill_permission_origins(&tx).expect("backfill"),
        (0, 0, 1)
    );
    drop(tx);
    drop(conn);
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

/// Audit S5-12: a journal written before the columns existed gets them as
/// NULL, and a row that predates the concept keeps the origin it had.
///
/// The default is deliberately absent rather than invented: a session
/// created before the daemon could name one has no name a human chose, and
/// a recovery that guessed one would show a name nobody ever saw.
#[test]
fn a_v9_journal_migrates_to_v10_with_null_display_name_and_creator() {
    let (dir, path) = tmp_journal();
    let conn = Connection::open(&path).expect("v9 journal");
    conn.execute_batch(SCHEMA_SQL).expect("base schema");
    conn.execute_batch(V8_DDL).expect("v8 columns and tables");
    conn.execute_batch(super::PEERS_AUDIT_SQL)
        .expect("v8 peers and audit tables");
    conn.execute_batch(
        "ALTER TABLE sessions ADD COLUMN origin_kind TEXT NOT NULL DEFAULT 'local';
             ALTER TABLE sessions ADD COLUMN origin_device TEXT;
             ALTER TABLE sessions ADD COLUMN origin_role TEXT;",
    )
    .expect("v9 origin columns");
    conn.execute(
        "INSERT INTO sessions (
                id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded,
                dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes,
                unsnapshotted_bytes, reaped, peer_session_id, provider,
                origin_kind, origin_device, origin_role
             ) VALUES ('s.before-name', 'owner', NULL, 'acp', 'Agent', 1, 2,
                       1, 'ended', 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, NULL, 'claude',
                       'peer', 'device-phone', 'client')",
        [],
    )
    .expect("v9 session row");
    conn.pragma_update(None, "user_version", 9)
        .expect("v9 version");
    drop(conn);

    let journal = Journal::open(&path).expect("migrate");
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == "s.before-name")
        .expect("the v9 row survived");
    assert!(
        row.display_name.is_none(),
        "a session from before the column has no name, and NULL says so"
    );
    assert!(row.created_by.is_none(), "and no creator");
    assert_eq!(
        row.origin.kind,
        SessionOriginKind::Peer,
        "the migration must not disturb what v9 already recorded"
    );
    assert_eq!(
        row.to_session().display_name,
        None,
        "a recovered transcript does not invent a name either"
    );

    let check = Connection::open(&path).expect("check migrated schema");
    for column in ["display_name", "created_by"] {
        let columns: i64 = check
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?1",
                [column],
                |row| row.get(0),
            )
            .expect("the migrated column");
        assert_eq!(columns, 1, "missing migrated column {column}");
    }
    let version: i32 = check
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, JOURNAL_SCHEMA_VERSION);
    drop(check);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
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

/// Audit S5B-07: a database whose `sessions.display_name` exists with the
/// wrong type (or the wrong nullability) is refused up front. A presence
/// check alone would accept it and every read afterwards would answer with
/// a value this daemon never wrote.
#[test]
fn a_v9_sessions_table_with_a_wrongly_typed_display_name_is_refused() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 display_name INTEGER,
                 created_by TEXT
             );",
    )
    .expect("a v9-shaped table with the wrong column type");
    let error = validate_agent_columns(&conn).expect_err("the wrong type is refused");
    assert!(
        matches!(error, JournalError::Corrupt(_)),
        "the corrupt-journal path is the one that refuses it: {error}"
    );
    assert!(
        error.to_string().contains("display_name"),
        "the message names the column: {error}"
    );
}

/// The other half of S5B-07: the shape this daemon writes (nullable TEXT,
/// no default) is accepted, so the check cannot pass by refusing everything.
#[test]
fn the_shape_this_daemon_writes_is_accepted() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 display_name TEXT,
                 created_by TEXT
             );",
    )
    .expect("the shape the daemon writes");
    validate_agent_columns(&conn).expect("the daemon's own shape is valid");
}

/// The v12 tri-state column is shape-checked too: a `TEXT` (or nullable,
/// or defaulting to `no`) `unattended_state` is a schema this daemon never
/// wrote and refuses rather than reads.
#[test]
fn a_sessions_table_with_a_wrongly_typed_unattended_state_is_refused() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 profile_id TEXT,
                 context_id TEXT,
                 labels TEXT,
                 unattended INTEGER NOT NULL DEFAULT 0,
                 unattended_state TEXT
             );",
    )
    .expect("a sessions table with the wrong tri-state column type");
    let error = super::validate_profile_columns(&conn).expect_err("the wrong type is refused");
    assert!(
        matches!(error, JournalError::Corrupt(_)),
        "the corrupt-journal path is the one that refuses it: {error}"
    );
    assert!(
        error.to_string().contains("unattended_state"),
        "the message names the column: {error}"
    );
}

/// And the shape the daemon actually writes for the tri-state — `INTEGER
/// NOT NULL DEFAULT 1`, the rank of `unknown` — is accepted, so the check
/// cannot pass by refusing everything.
#[test]
fn the_tri_state_shape_this_daemon_writes_is_accepted() {
    let conn = Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 profile_id TEXT,
                 context_id TEXT,
                 labels TEXT,
                 unattended INTEGER NOT NULL DEFAULT 0,
                 unattended_state INTEGER NOT NULL DEFAULT 1
             );",
    )
    .expect("the shape the daemon writes");
    super::validate_profile_columns(&conn).expect("the daemon's own shape is valid");
}

/// A v11 journal file: the base schema and every column through v11, so
/// the v12 migration starts from the version it will find on disk.
/// The four v11 profile columns come from the daemon's own DDL; the
/// colliding-shape test below supplies its own through
/// [`v11_journal_with_profile_ddl`].
fn v11_journal_with_rows(rows: &[(&str, i64, &str)]) -> (std::path::PathBuf, std::path::PathBuf) {
    v11_journal_with_profile_ddl(
        rows,
        "ALTER TABLE sessions ADD COLUMN profile_id TEXT;
             ALTER TABLE sessions ADD COLUMN context_id TEXT;
             ALTER TABLE sessions ADD COLUMN unattended INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE sessions ADD COLUMN labels TEXT;",
    )
}

/// A v11 journal file whose four v11 profile columns are built from the
/// caller's DDL instead of the daemon's: a v11 file that happens to carry
/// a colliding column in any of the four shapes the post-commit validation
/// refuses. SQLite's type affinity accepts the standard test rows into the
/// wrong-shaped columns, which is exactly the hazard — the data fits, the
/// shape lies.
fn v11_journal_with_profile_ddl(
    rows: &[(&str, i64, &str)],
    profile_ddl: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let (dir, path) = tmp_journal();
    let conn = Connection::open(&path).expect("v11 journal");
    conn.execute_batch(SCHEMA_SQL).expect("base schema");
    conn.execute_batch(V8_DDL).expect("v8 columns and tables");
    conn.execute_batch(super::PEERS_AUDIT_SQL)
        .expect("v8 peers and audit tables");
    conn.execute_batch(
        "ALTER TABLE sessions ADD COLUMN origin_kind TEXT NOT NULL DEFAULT 'local';
             ALTER TABLE sessions ADD COLUMN origin_device TEXT;
             ALTER TABLE sessions ADD COLUMN origin_role TEXT;
             ALTER TABLE sessions ADD COLUMN display_name TEXT;
             ALTER TABLE sessions ADD COLUMN created_by TEXT;",
    )
    .expect("v9-v10 columns");
    conn.execute_batch(profile_ddl)
        .expect("v11 profile columns");
    for (id, unattended, profile_id) in rows {
        conn.execute(
            "INSERT INTO sessions (
                    id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                    generation, status, exit_code, closed, last_seq, degraded,
                    dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes,
                    unsnapshotted_bytes, reaped, peer_session_id, provider,
                    origin_kind, origin_device, origin_role,
                    display_name, created_by, profile_id, context_id, unattended, labels
                 ) VALUES (?1, 'owner', NULL, 'acp', 'Agent', 1, 2,
                           1, 'ended', 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, NULL, NULL,
                           'local', NULL, NULL,
                           NULL, NULL, ?3, NULL, ?2, '{}')",
            rusqlite::params![id, unattended, profile_id],
        )
        .expect("v11 session row");
    }
    conn.pragma_update(None, "user_version", 11)
        .expect("v11 version");
    drop(conn);
    (dir, path)
}

/// The v12 backfill, and the direction it is forbidden to get wrong. An
/// old `unattended = 1` recorded a profile the daemon could see had
/// auto-accepted, so it reads `yes`; an old `0` was written by the deleted
/// provider table or the profile's feature tick, neither of which ever
/// distinguished "we knew a human was watching" from "we were not told" —
/// so it reads **`unknown`, never `no`**. Backfilling toward `no` would
/// manufacture the one certainty the old column never recorded.
#[test]
fn a_v11_journal_backfills_true_to_yes_and_false_to_unknown_never_no() {
    let (dir, path) = v11_journal_with_rows(&[
        ("s.before-tri.asking", 0, "profile-ask"),
        ("s.before-tri.unattended", 1, "profile-bypass"),
    ]);

    {
        let journal = Journal::open(&path).expect("migrate");
        let rows = journal.list().expect("list");
        let asking = rows
            .iter()
            .find(|row| row.id == "s.before-tri.asking")
            .expect("the old asking row survived");
        assert_eq!(
            asking.unattended_state,
            devboule_protocol::UnattendedState::Unknown,
            "an old false is unknown: nobody recorded that a human was watching"
        );
        assert_eq!(
            asking.to_session().unattended,
            devboule_protocol::UnattendedState::Unknown,
            "the wire row reads the same"
        );
        let unattended = rows
            .iter()
            .find(|row| row.id == "s.before-tri.unattended")
            .expect("the old unattended row survived");
        assert_eq!(
            unattended.unattended_state,
            devboule_protocol::UnattendedState::Yes,
            "an old true is yes: the profile it recorded had auto-accepted"
        );
        journal.shutdown();
    }

    // The stored ranks, not just the typed reads: the asking row sits at
    // the `unknown` rank (1) and never at `no` (0).
    let raw = |id: &str| {
        Connection::open(&path)
            .expect("open migrated journal")
            .query_row(
                "SELECT unattended_state FROM sessions WHERE id = ?1",
                [id],
                |row| row.get::<_, i64>(0),
            )
            .expect("stored rank")
    };
    assert_eq!(raw("s.before-tri.asking"), 1, "unknown, never no");
    assert_eq!(raw("s.before-tri.unattended"), 2, "yes");

    // A second open re-runs nothing and moves nothing.
    {
        let journal = Journal::open(&path).expect("second open");
        journal.shutdown();
    }
    assert_eq!(raw("s.before-tri.asking"), 1);
    assert_eq!(raw("s.before-tri.unattended"), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ordering the pre-stamp guard fixes (audit R2b-1 finding 2): a v11
/// file that already carries an `unattended_state` column of the wrong
/// shape is refused with `user_version` still **11**. The open returns
/// `Err` with or without the guard — the post-commit validation raises
/// the same `Corrupt` — so the assertion that proves the ordering is the
/// version left on disk: stamp-then-check leaves 12, and the file is then
/// openable by no build, old (`FutureSchema`) or new (`Corrupt`).
#[test]
fn a_v12_migration_does_not_stamp_a_colliding_column() {
    let (dir, path) = v11_journal_with_rows(&[("s.before-tri.asking", 1, "profile-bypass")]);
    {
        let conn = Connection::open(&path).expect("open the v11 journal");
        conn.execute("ALTER TABLE sessions ADD COLUMN unattended_state TEXT", [])
            .expect("the stray colliding column");
    }
    let error = match Journal::open(&path) {
        Err(error) => error,
        Ok(journal) => {
            journal.shutdown();
            panic!("the colliding column is refused");
        }
    };
    assert!(
        matches!(error, JournalError::Corrupt(_)),
        "the corrupt-journal path is the one that refuses it: {error}"
    );
    assert!(
        error.to_string().contains("unattended_state"),
        "the message names the column: {error}"
    );
    let version: i32 = Connection::open(&path)
        .expect("open the refused journal")
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version");
    assert_eq!(
        version, 11,
        "the stamp never commits: the file stays openable by an older build, \
             and the next open re-attempts the migration"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A v12 journal file: the v11 builder plus the v12 tri-state column, so
/// the v13 migration starts from the version it will find on disk.
fn v12_journal_with_rows(rows: &[(&str, i64, &str)]) -> (std::path::PathBuf, std::path::PathBuf) {
    let (dir, path) = v11_journal_with_rows(rows);
    let conn = Connection::open(&path).expect("v12 journal");
    conn.execute(
        "ALTER TABLE sessions ADD COLUMN unattended_state INTEGER NOT NULL DEFAULT 1",
        [],
    )
    .expect("v12 column");
    conn.pragma_update(None, "user_version", 12)
        .expect("v12 version");
    drop(conn);
    (dir, path)
}

/// The v13 file the v14 migration starts from the version it will find
/// on disk.
fn v13_journal_with_rows(rows: &[(&str, i64, &str)]) -> (std::path::PathBuf, std::path::PathBuf) {
    let (dir, path) = v12_journal_with_rows(rows);
    let conn = Connection::open(&path).expect("v13 journal");
    conn.execute("ALTER TABLE sessions ADD COLUMN overlay TEXT", [])
        .expect("v13 overlay column");
    conn.execute("ALTER TABLE sessions ADD COLUMN depth INTEGER", [])
        .expect("v13 depth column");
    conn.pragma_update(None, "user_version", 13)
        .expect("v13 version");
    drop(conn);
    (dir, path)
}

/// A v12 file gains the overlay column on open, and its rows — which
/// predate the column — read as no overlay, never as an unknown one.
/// NULL is the one representation of "no overlay": a birth with no
/// restriction writes the same bytes these rows already have.
#[test]
fn a_v12_journal_gains_the_overlay_column_and_old_rows_read_no_overlay() {
    let (dir, path) = v12_journal_with_rows(&[("s.before-overlay", 0, "profile-x")]);
    let journal = Journal::open(&path).expect("migrate");
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == "s.before-overlay")
        .expect("the old row survived");
    assert_eq!(
        row.overlay,
        Some(crate::provider_catalog::ToolOverlay::NONE),
        "a pre-column row reads as no overlay"
    );
    assert_eq!(row.depth, None, "a pre-column row records no depth");
    let check = Connection::open(&path).expect("check migrated schema");
    let shape = super::column_shape(&check, "overlay").expect("column shape");
    assert!(
        matches!(shape, Some((ref kind, 0, None)) if kind.eq_ignore_ascii_case("text")),
        "TEXT, nullable, no default — the shape the daemon writes: {shape:?}"
    );
    let depth_shape = super::column_shape(&check, "depth").expect("column shape");
    assert!(
        matches!(depth_shape, Some((ref kind, 0, None)) if kind.eq_ignore_ascii_case("integer")),
        "INTEGER, nullable, no default — the shape the daemon writes: {depth_shape:?}"
    );
    let raw: Option<String> = Connection::open(&path)
        .expect("open migrated journal")
        .query_row(
            "SELECT overlay FROM sessions WHERE id = 's.before-overlay'",
            [],
            |row| row.get(0),
        )
        .expect("raw cell");
    assert_eq!(raw, None, "no backfill manufactures a restriction");
    let version: i32 = Connection::open(&path)
        .expect("open migrated journal")
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, JOURNAL_SCHEMA_VERSION);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The v13 pre-stamp guard: a v12 file carrying an `overlay` column of
/// the wrong shape is refused with `user_version` still **12**, so the
/// file stays openable by the previous build instead of stamped 13 and
/// openable by none.
#[test]
fn a_v13_migration_does_not_stamp_a_colliding_column() {
    let (dir, path) = v12_journal_with_rows(&[("s.before-overlay", 0, "profile-x")]);
    {
        let conn = Connection::open(&path).expect("open the v12 journal");
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN overlay INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .expect("the stray colliding column");
    }
    let error = match Journal::open(&path) {
        Err(error) => error,
        Ok(journal) => {
            journal.shutdown();
            panic!("the colliding column is refused");
        }
    };
    assert!(
        matches!(error, JournalError::Corrupt(_)),
        "the corrupt-journal path is the one that refuses it: {error}"
    );
    assert!(
        error.to_string().contains("overlay"),
        "the message names the column: {error}"
    );
    let version: i32 = Connection::open(&path)
        .expect("open the refused journal")
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version");
    assert_eq!(version, 12);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The symmetric half the overlay guard does not cover: a v12 file
/// carrying a `depth` column of the wrong shape is refused with
/// `user_version` still **12**, for the same brick-the-file reason.
#[test]
fn a_v13_migration_does_not_stamp_a_colliding_depth_column() {
    let (dir, path) = v12_journal_with_rows(&[("s.before-overlay", 0, "profile-x")]);
    {
        let conn = Connection::open(&path).expect("open the v12 journal");
        conn.execute("ALTER TABLE sessions ADD COLUMN depth TEXT", [])
            .expect("the stray colliding column");
    }
    let error = match Journal::open(&path) {
        Err(error) => error,
        Ok(journal) => {
            journal.shutdown();
            panic!("the colliding column is refused");
        }
    };
    assert!(
        matches!(error, JournalError::Corrupt(_)),
        "the corrupt-journal path is the one that refuses it: {error}"
    );
    assert!(
        error.to_string().contains("depth"),
        "the message names the column: {error}"
    );
    let version: i32 = Connection::open(&path)
        .expect("open the refused journal")
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version");
    assert_eq!(version, 12);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The v14 migration on a real v13 file: the mark column arrives, and a
/// row that predates it reads as "no refusal recorded" — the same bytes
/// every fresh row writes.
#[test]
fn a_v13_journal_gains_the_disown_mark_and_old_rows_read_no_refusal() {
    let (dir, path) = v13_journal_with_rows(&[("s.before-mark", 0, "profile-x")]);
    let journal = Journal::open(&path).expect("migrate");
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == "s.before-mark")
        .expect("the old row survived");
    assert_eq!(
        row.disowned_peer_session_id, None,
        "a pre-column row records no refusal"
    );
    let check = Connection::open(&path).expect("check migrated schema");
    let shape = super::column_shape(&check, "disowned_peer_session_id").expect("column shape");
    assert!(
        matches!(shape, Some((ref kind, 0, None)) if kind.eq_ignore_ascii_case("text")),
        "TEXT, nullable, no default — the shape the daemon writes: {shape:?}"
    );
    let version: i32 = check
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version");
    assert_eq!(version, JOURNAL_SCHEMA_VERSION);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The v14 pre-stamp guard: a v13 file carrying a
/// `disowned_peer_session_id` column of the wrong shape is refused with
/// `user_version` still **13**. Stamped 14, the first list or replay
/// would die reading the column on a user's journal at runtime — the
/// convention exists to make it a clean refusal at open instead.
#[test]
fn a_v14_migration_does_not_stamp_a_colliding_column() {
    let (dir, path) = v13_journal_with_rows(&[("s.before-mark", 0, "profile-x")]);
    {
        let conn = Connection::open(&path).expect("open the v13 journal");
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN disowned_peer_session_id INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .expect("the stray colliding column");
    }
    let error = match Journal::open(&path) {
        Err(error) => error,
        Ok(journal) => {
            journal.shutdown();
            panic!("the colliding column is refused");
        }
    };
    assert!(
        matches!(error, JournalError::Corrupt(_)),
        "the corrupt-journal path is the one that refuses it: {error}"
    );
    assert!(
        error.to_string().contains("disowned_peer_session_id"),
        "the message names the column: {error}"
    );
    let version: i32 = Connection::open(&path)
        .expect("open the refused journal")
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version");
    assert_eq!(version, 13);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The guard above covers one column; the post-commit validation checks
/// five. Each of the other four v11 shapes below is a file the daemon
/// will refuse — and before the guard was widened to call the validation
/// itself, each was stamped 12 first: refused, then bricked. The `Err`
/// alone proves nothing (the post-commit check raises it either way), so
/// every case asserts the tripwire, `PRAGMA user_version == 11`.
#[test]
fn a_v12_migration_does_not_stamp_any_colliding_v11_column() {
    // One wrong shape per case, the other three columns exactly as the
    // daemon writes them — so the refusal names this column and no other.
    let colliding: &[(&str, &str)] = &[
        (
            "profile_id",
            "ALTER TABLE sessions ADD COLUMN profile_id INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE sessions ADD COLUMN context_id TEXT;
                 ALTER TABLE sessions ADD COLUMN unattended INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE sessions ADD COLUMN labels TEXT;",
        ),
        (
            "context_id",
            "ALTER TABLE sessions ADD COLUMN profile_id TEXT;
                 ALTER TABLE sessions ADD COLUMN context_id INTEGER;
                 ALTER TABLE sessions ADD COLUMN unattended INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE sessions ADD COLUMN labels TEXT;",
        ),
        (
            "unattended",
            "ALTER TABLE sessions ADD COLUMN profile_id TEXT;
                 ALTER TABLE sessions ADD COLUMN context_id TEXT;
                 ALTER TABLE sessions ADD COLUMN unattended TEXT;
                 ALTER TABLE sessions ADD COLUMN labels TEXT;",
        ),
        (
            "labels",
            "ALTER TABLE sessions ADD COLUMN profile_id TEXT;
                 ALTER TABLE sessions ADD COLUMN context_id TEXT;
                 ALTER TABLE sessions ADD COLUMN unattended INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE sessions ADD COLUMN labels TEXT NOT NULL DEFAULT 'x';",
        ),
    ];
    for (column, profile_ddl) in colliding {
        let (dir, path) = v11_journal_with_profile_ddl(
            &[("s.before-tri.asking", 1, "profile-bypass")],
            profile_ddl,
        );
        let error = match Journal::open(&path) {
            Err(error) => error,
            Ok(journal) => {
                journal.shutdown();
                panic!("the colliding {column} column is refused");
            }
        };
        assert!(
            matches!(error, JournalError::Corrupt(_)),
            "{column}: the corrupt-journal path is the one that refuses it: {error}"
        );
        assert!(
            error.to_string().contains(column),
            "{column}: the message names the column: {error}"
        );
        let version: i32 = Connection::open(&path)
            .expect("open the refused journal")
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("user_version");
        assert_eq!(
            version, 11,
            "{column}: the stamp never commits — refused, not bricked"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn stored_trigger(conn: &Connection, name: &str) -> String {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
        [name],
        |row| row.get(0),
    )
    .unwrap_or_else(|error| panic!("trigger {name} is not in sqlite_master: {error}"))
}
